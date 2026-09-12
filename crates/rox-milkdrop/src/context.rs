//! A current OpenGL context with no window under it.
//!
//! libprojectM is a GL renderer that asks for two things: a context that's
//! current on the calling thread, and a function that resolves GL symbols by
//! name. It doesn't want a window and never presents anything, because
//! everything it draws lands in an FBO we read back. So this module's whole
//! job is to talk each platform's context API into handing over a context
//! with nothing attached to it.
//!
//! Every platform has an answer and they're all slightly different. EGL says
//! surfaceless out loud, and glutin's EGL backend takes an X11 display handle
//! with a null display pointer as "give me `EGL_DEFAULT_DISPLAY`", which is
//! what a machine with no X server still resolves through Mesa. CGL treats
//! surfaceless as normal. WGL is the awkward one: it can't produce a context
//! without a device context, and a device context comes from a window, so the
//! worker makes a 1x1 window nobody ever shows and throws it away afterwards.
//!
//! Where surfaceless is refused anyway, and some drivers do refuse it, the
//! fallback is a 1x1 pbuffer that gets made current and then ignored. Nothing
//! is ever drawn into it.
//!
//! Errors are strings that name the platform and the step that failed. They
//! travel up into `Status::Failed` and end up as the panel's body text, so
//! they're written to be read by whoever is looking at a black panel and
//! wondering why.

use std::ffi::{c_void, CStr};
use std::num::NonZeroU32;
use std::sync::OnceLock;

use glutin::config::{Config, ConfigSurfaceTypes, ConfigTemplateBuilder};
use glutin::context::{
    ContextApi, ContextAttributesBuilder, GlProfile, NotCurrentContext, PossiblyCurrentContext,
    Version,
};
use glutin::display::{Display, DisplayApiPreference};
use glutin::prelude::*;
use glutin::surface::{PbufferSurface, Surface, SurfaceAttributesBuilder};
use raw_window_handle::RawWindowHandle;

/// The GL profile projectM's shaders are compiled against. 3.3 core is
/// libprojectM's own floor everywhere but macOS, where CGL's core profile
/// tops out at 4.1 and won't advertise anything lower.
#[cfg(target_os = "macos")]
const GL_VERSION: Version = Version { major: 4, minor: 1 };
#[cfg(not(target_os = "macos"))]
const GL_VERSION: Version = Version { major: 3, minor: 3 };

/// A current context and the display that can resolve GL symbols for it.
///
/// Not `Send`: it's created on the worker thread, used there, and dropped
/// there. A GL context that changes threads has to be released first, and the
/// worker has no reason to ever want that.
pub struct HeadlessGl {
    context: PossiblyCurrentContext,
    /// Kept alive because the context is current on it. Only present when
    /// surfaceless was refused.
    _surface: Option<Surface<PbufferSurface>>,
    display: Display,
    #[cfg(windows)]
    _window: windows_helper::HiddenWindow,
}

/// The display projectM resolves symbols through, for the life of the
/// process.
///
/// libprojectM's `GLResolver` is a process singleton that latches the first
/// resolver callback and user pointer it's handed and returns early from
/// every later `Initialize` (`Renderer/Platform/GLResolver.cpp`, the
/// `if (m_loaded) return true;` at the top). So a pointer to any one
/// engine's `HeadlessGl` is the wrong thing to give it: close the panel that
/// owns that engine, open another, and projectM resolves through a pointer
/// to freed memory on the next instance's glad probe. That's a segfault
/// inside `projectm_create_with_opengl_load_proc`, and it's what this static
/// exists to stop.
///
/// A clone of the display is safe to keep forever. glutin refcounts it, and
/// on EGL it never calls `eglTerminate` because the underlying display is
/// itself a process singleton. Symbol resolution doesn't read per-context
/// state, so the first engine's display answers correctly for every later
/// engine on the same driver.
static RESOLVER: OnceLock<Display> = OnceLock::new();

/// Resolve a GL entry point through the process-lifetime display. Returns
/// null before any context has been built, which is projectM's own "not
/// found" and can't happen in practice: the resolver is only ever called
/// from inside a `projectm_create_*` that a live worker is making.
pub fn resolve(name: &CStr) -> *const c_void {
    match RESOLVER.get() {
        Some(display) => display.get_proc_address(name),
        None => std::ptr::null(),
    }
}

impl HeadlessGl {
    /// Resolve a GL entry point. This is what loads our own small GL function
    /// table; projectM goes through [`resolve`] instead, for the lifetime
    /// reason documented there.
    pub fn get_proc_address(&self, name: &CStr) -> *const c_void {
        self.display.get_proc_address(name)
    }

    /// True while this context is the one the calling thread would render
    /// through. The worker checks it once after setup.
    pub fn is_current(&self) -> bool {
        self.context.is_current()
    }
}

/// Build a headless context, or explain why not.
pub fn create() -> Result<HeadlessGl, String> {
    #[cfg(windows)]
    let window = windows_helper::HiddenWindow::create()?;

    #[cfg(windows)]
    let window_handle = Some(window.raw_handle());
    #[cfg(not(windows))]
    let window_handle: Option<RawWindowHandle> = None;

    let display = create_display(window_handle)?;
    // First engine wins and every later one resolves through it. See RESOLVER.
    let _ = RESOLVER.set(display.clone());
    let config = find_config(&display)?;

    let (context, attributes) = create_context(&display, &config, window_handle)?;

    match make_current_surfaceless(context) {
        Ok(context) => Ok(HeadlessGl {
            context,
            _surface: None,
            display,
            #[cfg(windows)]
            _window: window,
        }),
        // Some drivers only pretend to be surfaceless. A 1x1 pbuffer nobody
        // draws into satisfies them, and costs a few kilobytes.
        Err(surfaceless_error) => {
            let context = unsafe { display.create_context(&config, &attributes) }
                .map_err(|e| step_error("recreating the OpenGL context for a pbuffer", &e))?;
            let one = NonZeroU32::new(1).expect("1 is not zero");
            let surface_attributes = SurfaceAttributesBuilder::<PbufferSurface>::new()
                .with_single_buffer(true)
                .build(one, one);
            let surface = unsafe { display.create_pbuffer_surface(&config, &surface_attributes) }
                .map_err(|e| {
                format!("{surfaceless_error}, and the 1x1 pbuffer fallback failed too: {e}")
            })?;
            let context = context
                .make_current(&surface)
                .map_err(|e| step_error("making the context current on a pbuffer", &e))?;
            Ok(HeadlessGl {
                context,
                _surface: Some(surface),
                display,
                #[cfg(windows)]
                _window: window,
            })
        }
    }
}

/// A context at the profile projectM was built for, or failing that, whatever
/// the driver will give.
///
/// The first ask is 3.3 core, which is libprojectM's floor. Some drivers turn
/// that exact request down and still hand out a usable context when asked
/// for nothing in particular: the Intel Windows drivers of the Haswell era
/// answer a versioned core request with an error code that isn't in any
/// header, and the report that led here was one of those, a black panel
/// under `wglCreateContextAttribsARB` failing with `0xC007000D`. A second
/// ask with no version and the compatibility profile gets the driver's
/// default, which under the ARB rules is the highest version it has that
/// is backward compatible. projectM accepts a compatibility context of 3.3
/// or newer, and it probes the context itself, so a driver that can only
/// do 3.1 is refused there with the version in the log rather than here
/// with an error code nobody can look up.
///
/// Not on macOS: CGL's compatibility profile is stuck at 2.1 by design,
/// so the retry would only trade one refusal for another.
fn create_context(
    display: &Display,
    config: &Config,
    window_handle: Option<RawWindowHandle>,
) -> Result<(NotCurrentContext, glutin::context::ContextAttributes), String> {
    let core = ContextAttributesBuilder::new()
        .with_context_api(ContextApi::OpenGl(Some(GL_VERSION)))
        .with_profile(GlProfile::Core)
        .build(window_handle);
    let core_error = match unsafe { display.create_context(config, &core) } {
        Ok(context) => return Ok((context, core)),
        Err(error) => error,
    };

    #[cfg(target_os = "macos")]
    {
        return Err(step_error("creating the OpenGL context", &core_error));
    }

    #[cfg(not(target_os = "macos"))]
    {
        log::warn!(
            "milkdrop: {}; asking the driver for its default context instead",
            step_error("creating the OpenGL context", &core_error)
        );
        let any = ContextAttributesBuilder::new()
            .with_context_api(ContextApi::OpenGl(None))
            .with_profile(GlProfile::Compatibility)
            .build(window_handle);
        match unsafe { display.create_context(config, &any) } {
            Ok(context) => Ok((context, any)),
            Err(error) => Err(format!(
                "{}, and the driver's default context failed too: {error}",
                step_error(
                    &format!(
                        "creating an OpenGL {}.{} core context",
                        GL_VERSION.major, GL_VERSION.minor
                    ),
                    &core_error
                )
            )),
        }
    }
}

fn create_display(window_handle: Option<RawWindowHandle>) -> Result<Display, String> {
    let _ = window_handle;

    #[cfg(target_os = "macos")]
    let (handle, preference) = (
        raw_window_handle::RawDisplayHandle::AppKit(raw_window_handle::AppKitDisplayHandle::new()),
        DisplayApiPreference::Cgl,
    );

    // WGL wants the window's device context, which it takes from the handle
    // passed here rather than from the one on the context attributes.
    #[cfg(windows)]
    let (handle, preference) = (
        raw_window_handle::RawDisplayHandle::Windows(raw_window_handle::WindowsDisplayHandle::new()),
        DisplayApiPreference::Wgl(window_handle),
    );

    // A null Xlib display is glutin's spelling of `EGL_DEFAULT_DISPLAY`: its
    // EGL backend maps `display: None` straight onto it, both on the
    // platform-display path and on the legacy `eglGetDisplay` fallback. So
    // this works with an X server, under Wayland, and on a machine with
    // neither, where Mesa answers with llvmpipe.
    #[cfg(not(any(target_os = "macos", windows)))]
    let (handle, preference) = (
        raw_window_handle::RawDisplayHandle::Xlib(raw_window_handle::XlibDisplayHandle::new(
            None, 0,
        )),
        DisplayApiPreference::Egl,
    );

    unsafe { Display::new(handle, preference) }
        .map_err(|e| step_error("opening the graphics display", &e))
}

fn find_config(display: &Display) -> Result<Config, String> {
    // Ask for a pbuffer-capable config first: it keeps the fallback below
    // open, and every driver that can do surfaceless can also do this. A
    // driver that offers neither gets the surfaceless-only template.
    for surface_types in [ConfigSurfaceTypes::PBUFFER, ConfigSurfaceTypes::empty()] {
        let template = ConfigTemplateBuilder::new()
            .with_alpha_size(8)
            .with_depth_size(24)
            .with_stencil_size(8)
            .with_surface_type(surface_types)
            .build();
        let found = unsafe { display.find_configs(template) };
        if let Ok(configs) = found {
            // Hardware first, then anything: llvmpipe is a fine last resort
            // and is what a headless CI runner has.
            if let Some(config) = configs.max_by_key(|c| c.hardware_accelerated() as u8) {
                return Ok(config);
            }
        }
    }
    Err(format!(
        "{} offers no OpenGL configuration this machine can render into",
        PLATFORM
    ))
}

/// glutin exposes `make_current_surfaceless` on each backend's own context
/// type but not on the enum that wraps them, so this unwraps the enum and
/// puts it back together. The catch-all covers backends we don't ask for,
/// like GLX arriving from a preference we never pass.
fn make_current_surfaceless(context: NotCurrentContext) -> Result<PossiblyCurrentContext, String> {
    match context {
        #[cfg(not(any(target_os = "macos", target_os = "ios")))]
        NotCurrentContext::Egl(context) => context
            .make_current_surfaceless()
            .map(PossiblyCurrentContext::Egl)
            .map_err(|e| step_error("making the EGL context current without a surface", &e)),
        #[cfg(windows)]
        NotCurrentContext::Wgl(context) => context
            .make_current_surfaceless()
            .map(PossiblyCurrentContext::Wgl)
            .map_err(|e| step_error("making the WGL context current without a surface", &e)),
        #[cfg(target_os = "macos")]
        NotCurrentContext::Cgl(context) => context
            .make_current_surfaceless()
            .map(PossiblyCurrentContext::Cgl)
            .map_err(|e| step_error("making the CGL context current without a surface", &e)),
        #[allow(unreachable_patterns)]
        _ => Err(format!(
            "{PLATFORM} returned a graphics backend rox did not ask for"
        )),
    }
}

#[cfg(target_os = "macos")]
const PLATFORM: &str = "macOS (CGL)";
#[cfg(windows)]
const PLATFORM: &str = "Windows (WGL)";
#[cfg(not(any(target_os = "macos", windows)))]
const PLATFORM: &str = "Linux (EGL)";

fn step_error(step: &str, error: &impl std::fmt::Display) -> String {
    format!("{PLATFORM} failed at {step}: {error}")
}

/// WGL's chicken and egg: `wglCreateContext` needs an HDC, an HDC comes from
/// a window, and we don't want a window. So we make the smallest, quietest
/// one Windows will give us, never show it, and destroy it when the worker
/// shuts down.
#[cfg(windows)]
mod windows_helper {
    use std::num::NonZeroIsize;

    use raw_window_handle::{RawWindowHandle, Win32WindowHandle};
    use windows_sys::core::PCWSTR;
    use windows_sys::Win32::Foundation::{HINSTANCE, HWND};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, CS_OWNDC, CW_USEDEFAULT,
        WNDCLASSW, WS_OVERLAPPED,
    };

    const CLASS_NAME: &[u16] = &[
        b'r' as u16,
        b'o' as u16,
        b'x' as u16,
        b'-' as u16,
        b'm' as u16,
        b'i' as u16,
        b'l' as u16,
        b'k' as u16,
        b'd' as u16,
        b'r' as u16,
        b'o' as u16,
        b'p' as u16,
        0,
    ];

    pub struct HiddenWindow {
        hwnd: HWND,
        instance: HINSTANCE,
    }

    impl HiddenWindow {
        pub fn create() -> Result<HiddenWindow, String> {
            unsafe {
                let instance = GetModuleHandleW(std::ptr::null());
                if instance.is_null() {
                    return Err(super::step_error(
                        "looking up the module handle for the helper window",
                        &std::io::Error::last_os_error(),
                    ));
                }

                // CS_OWNDC so the device context WGL takes stays valid for
                // the window's whole life rather than being handed back to
                // the system cache after every paint.
                let class = WNDCLASSW {
                    style: CS_OWNDC,
                    lpfnWndProc: Some(DefWindowProcW),
                    cbClsExtra: 0,
                    cbWndExtra: 0,
                    hInstance: instance,
                    hIcon: std::ptr::null_mut(),
                    hCursor: std::ptr::null_mut(),
                    hbrBackground: std::ptr::null_mut(),
                    lpszMenuName: std::ptr::null(),
                    lpszClassName: CLASS_NAME.as_ptr(),
                };
                // A second engine registers the same class, which returns
                // zero with ERROR_CLASS_ALREADY_EXISTS. That's fine, so the
                // result is deliberately not checked.
                RegisterClassW(&class);

                let hwnd = CreateWindowExW(
                    0,
                    CLASS_NAME.as_ptr() as PCWSTR,
                    CLASS_NAME.as_ptr() as PCWSTR,
                    WS_OVERLAPPED,
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    1,
                    1,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    instance,
                    std::ptr::null(),
                );
                if hwnd.is_null() {
                    return Err(super::step_error(
                        "creating the hidden helper window WGL needs",
                        &std::io::Error::last_os_error(),
                    ));
                }

                Ok(HiddenWindow { hwnd, instance })
            }
        }

        pub fn raw_handle(&self) -> RawWindowHandle {
            // raw-window-handle wants the HWND as a non-zero integer, not a
            // pointer; `create` already refused a null one.
            let mut handle = Win32WindowHandle::new(
                NonZeroIsize::new(self.hwnd as isize).expect("checked non-null at creation"),
            );
            handle.hinstance = NonZeroIsize::new(self.instance as isize);
            RawWindowHandle::Win32(handle)
        }
    }

    impl Drop for HiddenWindow {
        fn drop(&mut self) {
            unsafe {
                DestroyWindow(self.hwnd);
            }
        }
    }
}
