//! The couple dozen OpenGL calls this crate makes for itself: the FBO and
//! texture projectM renders into, the two readback pixel buffers, and
//! `glGetString` for the log. projectM loads its own GL through glad.
//!
//! No `gl` or `glow` crate: both generate tens of thousands of lines for a
//! couple dozen functions, and glutin already supplies the loader.

use std::ffi::{CStr, CString, c_void};

pub type GLenum = u32;
pub type GLuint = u32;
pub type GLint = i32;
pub type GLsizei = i32;
pub type GLbitfield = u32;
pub type GLboolean = u8;
pub type GLintptr = isize;
pub type GLsizeiptr = isize;

pub const TEXTURE_2D: GLenum = 0x0DE1;
pub const UNSIGNED_BYTE: GLenum = 0x1401;
pub const RGBA: GLenum = 0x1908;
pub const RGBA8: GLenum = 0x8058;
pub const LINEAR: GLint = 0x2601;
pub const CLAMP_TO_EDGE: GLint = 0x812F;
pub const TEXTURE_MAG_FILTER: GLenum = 0x2800;
pub const TEXTURE_MIN_FILTER: GLenum = 0x2801;
pub const TEXTURE_WRAP_S: GLenum = 0x2802;
pub const TEXTURE_WRAP_T: GLenum = 0x2803;
pub const FRAMEBUFFER: GLenum = 0x8D40;
pub const FRAMEBUFFER_COMPLETE: GLenum = 0x8CD5;
pub const COLOR_ATTACHMENT0: GLenum = 0x8CE0;
pub const DEPTH_STENCIL_ATTACHMENT: GLenum = 0x821A;
pub const RENDERBUFFER: GLenum = 0x8D41;
pub const DEPTH24_STENCIL8: GLenum = 0x88F0;
pub const PIXEL_PACK_BUFFER: GLenum = 0x88EB;
pub const STREAM_READ: GLenum = 0x88E1;
pub const MAP_READ_BIT: GLbitfield = 0x0001;
pub const PACK_ALIGNMENT: GLenum = 0x0D05;
pub const COLOR_BUFFER_BIT: GLbitfield = 0x0000_4000;
pub const DEPTH_BUFFER_BIT: GLbitfield = 0x0000_0100;
pub const RENDERER: GLenum = 0x1F01;
pub const VERSION: GLenum = 0x1F02;

/// Resolved once. A null means the driver isn't GL 3.0 and nothing
/// downstream would work, so `load` refuses.
#[allow(non_snake_case)]
pub struct Gl {
    pub GenFramebuffers: unsafe extern "system" fn(GLsizei, *mut GLuint),
    pub DeleteFramebuffers: unsafe extern "system" fn(GLsizei, *const GLuint),
    pub BindFramebuffer: unsafe extern "system" fn(GLenum, GLuint),
    pub FramebufferTexture2D: unsafe extern "system" fn(GLenum, GLenum, GLenum, GLuint, GLint),
    pub FramebufferRenderbuffer: unsafe extern "system" fn(GLenum, GLenum, GLenum, GLuint),
    pub CheckFramebufferStatus: unsafe extern "system" fn(GLenum) -> GLenum,
    pub GenRenderbuffers: unsafe extern "system" fn(GLsizei, *mut GLuint),
    pub DeleteRenderbuffers: unsafe extern "system" fn(GLsizei, *const GLuint),
    pub BindRenderbuffer: unsafe extern "system" fn(GLenum, GLuint),
    pub RenderbufferStorage: unsafe extern "system" fn(GLenum, GLenum, GLsizei, GLsizei),
    pub GenTextures: unsafe extern "system" fn(GLsizei, *mut GLuint),
    pub DeleteTextures: unsafe extern "system" fn(GLsizei, *const GLuint),
    pub BindTexture: unsafe extern "system" fn(GLenum, GLuint),
    pub TexImage2D: unsafe extern "system" fn(
        GLenum,
        GLint,
        GLint,
        GLsizei,
        GLsizei,
        GLint,
        GLenum,
        GLenum,
        *const c_void,
    ),
    pub TexParameteri: unsafe extern "system" fn(GLenum, GLenum, GLint),
    pub GenBuffers: unsafe extern "system" fn(GLsizei, *mut GLuint),
    pub DeleteBuffers: unsafe extern "system" fn(GLsizei, *const GLuint),
    pub BindBuffer: unsafe extern "system" fn(GLenum, GLuint),
    pub BufferData: unsafe extern "system" fn(GLenum, GLsizeiptr, *const c_void, GLenum),
    pub MapBufferRange:
        unsafe extern "system" fn(GLenum, GLintptr, GLsizeiptr, GLbitfield) -> *mut c_void,
    pub UnmapBuffer: unsafe extern "system" fn(GLenum) -> GLboolean,
    pub ReadPixels:
        unsafe extern "system" fn(GLint, GLint, GLsizei, GLsizei, GLenum, GLenum, *mut c_void),
    pub PixelStorei: unsafe extern "system" fn(GLenum, GLint),
    pub Viewport: unsafe extern "system" fn(GLint, GLint, GLsizei, GLsizei),
    pub ClearColor: unsafe extern "system" fn(f32, f32, f32, f32),
    pub Clear: unsafe extern "system" fn(GLbitfield),
    pub GetError: unsafe extern "system" fn() -> GLenum,
    pub GetString: unsafe extern "system" fn(GLenum) -> *const u8,
}

impl Gl {
    /// Names every symbol that came back null: one missing is a driver story,
    /// ten is "you have no GL".
    // No turbofish on the transmutes: each target type is the field it's
    // assigned to, and spelling 28 signatures twice only lets them disagree.
    #[allow(clippy::missing_transmute_annotations)]
    pub fn load(resolve: impl Fn(&CStr) -> *const c_void) -> Result<Gl, String> {
        let mut missing = Vec::new();
        let mut get = |name: &str| -> *const c_void {
            let c_name = CString::new(name).expect("GL names have no interior nul");
            let pointer = resolve(&c_name);
            if pointer.is_null() {
                missing.push(name.to_string());
            }
            pointer
        };

        // Read every pointer before checking `missing`, so the error lists them all.
        macro_rules! entry {
            ($name:literal) => {
                unsafe { std::mem::transmute(get($name)) }
            };
        }

        let gl = Gl {
            GenFramebuffers: entry!("glGenFramebuffers"),
            DeleteFramebuffers: entry!("glDeleteFramebuffers"),
            BindFramebuffer: entry!("glBindFramebuffer"),
            FramebufferTexture2D: entry!("glFramebufferTexture2D"),
            FramebufferRenderbuffer: entry!("glFramebufferRenderbuffer"),
            CheckFramebufferStatus: entry!("glCheckFramebufferStatus"),
            GenRenderbuffers: entry!("glGenRenderbuffers"),
            DeleteRenderbuffers: entry!("glDeleteRenderbuffers"),
            BindRenderbuffer: entry!("glBindRenderbuffer"),
            RenderbufferStorage: entry!("glRenderbufferStorage"),
            GenTextures: entry!("glGenTextures"),
            DeleteTextures: entry!("glDeleteTextures"),
            BindTexture: entry!("glBindTexture"),
            TexImage2D: entry!("glTexImage2D"),
            TexParameteri: entry!("glTexParameteri"),
            GenBuffers: entry!("glGenBuffers"),
            DeleteBuffers: entry!("glDeleteBuffers"),
            BindBuffer: entry!("glBindBuffer"),
            BufferData: entry!("glBufferData"),
            MapBufferRange: entry!("glMapBufferRange"),
            UnmapBuffer: entry!("glUnmapBuffer"),
            ReadPixels: entry!("glReadPixels"),
            PixelStorei: entry!("glPixelStorei"),
            Viewport: entry!("glViewport"),
            ClearColor: entry!("glClearColor"),
            Clear: entry!("glClear"),
            GetError: entry!("glGetError"),
            GetString: entry!("glGetString"),
        };

        if missing.is_empty() {
            Ok(gl)
        } else {
            Err(format!(
                "the graphics driver is missing {} OpenGL entry points, starting with {}",
                missing.len(),
                missing.join(", ")
            ))
        }
    }

    pub fn string(&self, name: GLenum) -> String {
        unsafe {
            let pointer = (self.GetString)(name);
            if pointer.is_null() {
                "unknown".to_string()
            } else {
                CStr::from_ptr(pointer as *const std::ffi::c_char)
                    .to_string_lossy()
                    .into_owned()
            }
        }
    }

    /// Only at the few points where an error means a wrong frame: `glGetError`
    /// flushes the pipeline on some drivers.
    pub fn take_error(&self) -> Option<GLenum> {
        unsafe {
            let mut last = None;
            while let error @ 1.. = (self.GetError)() {
                last = Some(error);
            }
            last
        }
    }
}
