//! A current OpenGL context with no window under it, which is all
//! libprojectM needs: it draws into an FBO we read back.
//!
//! Linux tries the X11 display (a null display pointer) and an EGL device
//! display on a render node for machines with no X server. CGL does
//! surfaceless natively. WGL needs a device context, so the worker makes a
//! hidden 1x1 window. Where a driver refuses surfaceless, a 1x1 pbuffer is made
//! current and ignored.
//!
//! Errors name the platform and step; they end up as the panel's body text.

use std::ffi::{CStr, c_void};
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

/// libprojectM's floor is 3.3 core; macOS's CGL core profile only advertises
/// 4.1.
#[cfg(target_os = "macos")]
const GL_VERSION: Version = Version { major: 4, minor: 1 };
#[cfg(not(target_os = "macos"))]
const GL_VERSION: Version = Version { major: 3, minor: 3 };

/// Not `Send`: created, used and dropped on the worker thread.
pub struct HeadlessGl {
    context: PossiblyCurrentContext,
    /// Only when surfaceless was refused; the context is current on it.
    _surface: Option<Surface<PbufferSurface>>,
    display: Display,
    #[cfg(windows)]
    _window: windows_helper::HiddenWindow,
}

/// The display projectM resolves symbols through, for the life of the
/// process.
///
/// libprojectM's `GLResolver` is a process singleton that latches the first
/// callback and user pointer it's handed (`Renderer/Platform/GLResolver.cpp`,
/// `if (m_loaded) return true;`). Handing it a pointer to one engine's
/// `HeadlessGl` segfaults in `projectm_create_with_opengl_load_proc` once that
/// engine is dropped and another opens. A display clone is safe forever:
/// glutin refcounts it, EGL's display is itself a process singleton, and
/// symbol resolution reads no per-context state.
static RESOLVER: OnceLock<Display> = OnceLock::new();

/// Null before any context exists, which can't happen from inside projectM.
pub fn resolve(name: &CStr) -> *const c_void {
    match RESOLVER.get() {
        Some(display) => display.get_proc_address(name),
        None => std::ptr::null(),
    }
}

impl HeadlessGl {
    /// For our own GL table; projectM goes through [`resolve`] instead.
    pub fn get_proc_address(&self, name: &CStr) -> *const c_void {
        self.display.get_proc_address(name)
    }

    pub fn is_current(&self) -> bool {
        self.context.is_current()
    }
}

pub fn create() -> Result<HeadlessGl, String> {
    #[cfg(windows)]
    let window = windows_helper::HiddenWindow::create()?;

    #[cfg(windows)]
    let window_handle = Some(window.raw_handle());
    #[cfg(not(windows))]
    let window_handle: Option<RawWindowHandle> = None;

    let display = create_display(window_handle)?;
    // First engine wins; see RESOLVER.
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
        // Some drivers only pretend to be surfaceless.
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

/// 3.3 core first, then the driver's default compatibility context. Haswell
/// era Intel Windows drivers refuse the versioned request with
/// `0xC007000D` from `wglCreateContextAttribsARB` but hand out a usable
/// default; projectM probes the version itself and logs a refusal. Not on
/// macOS, where the compatibility profile is stuck at 2.1.
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
    {
        let handle = raw_window_handle::RawDisplayHandle::AppKit(
            raw_window_handle::AppKitDisplayHandle::new(),
        );
        return unsafe { Display::new(handle, DisplayApiPreference::Cgl) }
            .map_err(|e| step_error("opening the graphics display", &e));
    }

    // WGL takes the window's device context from this handle, not the one on
    // the context attributes.
    #[cfg(windows)]
    {
        let handle = raw_window_handle::RawDisplayHandle::Windows(
            raw_window_handle::WindowsDisplayHandle::new(),
        );
        return unsafe { Display::new(handle, DisplayApiPreference::Wgl(window_handle)) }
            .map_err(|e| step_error("opening the graphics display", &e));
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    {
        create_egl_display()
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
#[derive(Clone, Copy)]
enum EglPlatform {
    /// Maps to `EGL_DEFAULT_DISPLAY`, which Mesa resolves through X11, so it
    /// fails with `EGL_NOT_INITIALIZED` without a reachable X server.
    X11,
    /// `EGL_PLATFORM_DEVICE_EXT` on a render node: works in Flatpak, over SSH,
    /// and on headless CI.
    Device,
}

/// X11 first when `DISPLAY` names a server, the path desktops already take;
/// device first otherwise, with X11 behind it for drivers without device
/// extensions.
#[cfg(not(any(target_os = "macos", windows)))]
fn create_egl_display() -> Result<Display, String> {
    let has_x11 = std::env::var_os("DISPLAY").is_some_and(|display| !display.is_empty());
    let order = if has_x11 {
        [EglPlatform::X11, EglPlatform::Device]
    } else {
        [EglPlatform::Device, EglPlatform::X11]
    };

    let mut failures = Vec::new();
    for platform in order {
        match open_egl_display(platform) {
            Ok(display) => {
                announce(platform);
                return Ok(display);
            }
            Err(error) => failures.push(error),
        }
    }

    // The failure the machine was expected to succeed at leads.
    Err(failures.join(", and "))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn open_egl_display(platform: EglPlatform) -> Result<Display, String> {
    match platform {
        EglPlatform::X11 => {
            let handle = raw_window_handle::RawDisplayHandle::Xlib(
                raw_window_handle::XlibDisplayHandle::new(None, 0),
            );
            unsafe { Display::new(handle, DisplayApiPreference::Egl) }
                .map_err(|e| step_error("opening the graphics display", &e))
        }
        EglPlatform::Device => open_device_display(),
    }
}

/// Hardware devices sort ahead of llvmpipe (`EGL_MESA_device_software`).
/// glutin 0.32 can't reach `EGL_PLATFORM_SURFACELESS_MESA`, which behaves the
/// same for our purposes.
#[cfg(not(any(target_os = "macos", windows)))]
fn open_device_display() -> Result<Display, String> {
    use glutin::api::egl::device::Device;
    use glutin::api::egl::display::Display as EglDisplay;

    let mut devices: Vec<Device> = Device::query_devices()
        .map_err(|e| step_error("asking EGL which devices it can render on", &e))?
        .collect();
    devices.sort_by_key(|device| device.extensions().contains("EGL_MESA_device_software"));

    let mut failure = format!("{PLATFORM} found no offscreen device to render on");
    for device in &devices {
        match unsafe { EglDisplay::with_device(device, None) } {
            Ok(display) => return Ok(Display::Egl(display)),
            Err(error) => failure = step_error("opening an offscreen EGL device display", &error),
        }
    }

    Err(failure)
}

/// Once per process: which platform answered is the first thing a black
/// panel bug report needs.
#[cfg(not(any(target_os = "macos", windows)))]
fn announce(platform: EglPlatform) {
    static ANNOUNCED: std::sync::Once = std::sync::Once::new();

    ANNOUNCED.call_once(|| {
        let path = match platform {
            EglPlatform::X11 => "the X11 display",
            EglPlatform::Device => "an offscreen EGL device",
        };
        log::info!("milkdrop: opened {PLATFORM} through {path}");
    });
}

fn find_config(display: &Display) -> Result<Config, String> {
    // Pbuffer-capable first, to keep the fallback open; surfaceless-only
    // otherwise.
    for surface_types in [ConfigSurfaceTypes::PBUFFER, ConfigSurfaceTypes::empty()] {
        let template = ConfigTemplateBuilder::new()
            .with_alpha_size(8)
            .with_depth_size(24)
            .with_stencil_size(8)
            .with_surface_type(surface_types)
            .build();
        let found = unsafe { display.find_configs(template) };
        if let Ok(configs) = found {
            // Hardware first; llvmpipe is a fine last resort.
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

/// glutin exposes `make_current_surfaceless` per backend but not on the
/// wrapping enum.
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

/// WGL needs an HDC, which needs a window: the smallest one Windows gives,
/// never shown.
#[cfg(windows)]
mod windows_helper {
    use std::num::NonZeroIsize;

    use raw_window_handle::{RawWindowHandle, Win32WindowHandle};
    use windows_sys::Win32::Foundation::{HINSTANCE, HWND};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CS_OWNDC, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW,
        WNDCLASSW, WS_OVERLAPPED,
    };
    use windows_sys::core::PCWSTR;

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

                // CS_OWNDC keeps the DC WGL takes valid for the window's life.
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
                // A second engine gets ERROR_CLASS_ALREADY_EXISTS, which is
                // fine, so the result isn't checked.
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
