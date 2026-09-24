//! Raw declarations for the part of libprojectM's C API the Milkdrop panel
//! uses. The safe wrapping lives in `rox-milkdrop`.
//!
//! Hand-written rather than bindgen, which would add libclang on three
//! platforms and in nix for about twenty stable functions. Transcribed from
//! `vendor/projectm/src/api/include/projectM-4/`; the header wins on any
//! disagreement. C's `_Bool` is one byte on every target, so `bool` matches.
//!
//! libprojectM is LGPL-2.1-only, linked statically into an AGPL-3.0-only
//! binary whose source ships, which satisfies the relink clause.

// Kept in C spelling so they grep against projectM's headers.
#![allow(non_camel_case_types)]

use core::ffi::{c_char, c_void};

/// The header's `struct projectm*`.
pub type projectm_handle = *mut c_void;

pub type projectm_load_proc =
    Option<unsafe extern "C" fn(name: *const c_char, user_data: *mut c_void) -> *mut c_void>;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum projectm_channels {
    PROJECTM_MONO = 1,
    PROJECTM_STEREO = 2,
}

pub type projectm_preset_switch_requested_event =
    Option<unsafe extern "C" fn(is_hard_cut: bool, user_data: *mut c_void)>;

/// Both strings are only valid for the duration of the call.
pub type projectm_preset_switch_failed_event = Option<
    unsafe extern "C" fn(
        preset_filename: *const c_char,
        message: *const c_char,
        user_data: *mut c_void,
    ),
>;

/// A plain C enum crosses as an `int`: an out-of-range value would be UB in
/// a Rust enum.
pub type projectm_log_level = core::ffi::c_int;
pub const PROJECTM_LOG_LEVEL_NOTSET: projectm_log_level = 0;
pub const PROJECTM_LOG_LEVEL_TRACE: projectm_log_level = 1;
pub const PROJECTM_LOG_LEVEL_DEBUG: projectm_log_level = 2;
pub const PROJECTM_LOG_LEVEL_INFO: projectm_log_level = 3;
pub const PROJECTM_LOG_LEVEL_WARN: projectm_log_level = 4;
pub const PROJECTM_LOG_LEVEL_ERROR: projectm_log_level = 5;
pub const PROJECTM_LOG_LEVEL_FATAL: projectm_log_level = 6;

/// Without one registered, every `LOG_*` in libprojectM is a no-op. The
/// message is only valid for the call and may hold line breaks.
pub type projectm_log_callback = Option<
    unsafe extern "C" fn(
        message: *const c_char,
        log_level: projectm_log_level,
        user_data: *mut c_void,
    ),
>;

unsafe extern "C" {
    // core.h
    pub fn projectm_create_with_opengl_load_proc(
        load_proc: projectm_load_proc,
        user_data: *mut c_void,
    ) -> projectm_handle;
    pub fn projectm_destroy(instance: projectm_handle);
    pub fn projectm_load_preset_file(
        instance: projectm_handle,
        filename: *const c_char,
        smooth_transition: bool,
    );
    pub fn projectm_load_preset_data(
        instance: projectm_handle,
        data: *const c_char,
        smooth_transition: bool,
    );
    /// Free with [`projectm_free_string`].
    pub fn projectm_get_version_string() -> *mut c_char;

    // memory.h
    pub fn projectm_free_string(string: *const c_char);

    // render_opengl.h
    pub fn projectm_opengl_render_frame_fbo(instance: projectm_handle, framebuffer_object_id: u32);

    // parameters.h
    pub fn projectm_set_window_size(instance: projectm_handle, width: usize, height: usize);
    pub fn projectm_set_fps(instance: projectm_handle, fps: i32);
    pub fn projectm_set_beat_sensitivity(instance: projectm_handle, sensitivity: f32);
    pub fn projectm_set_preset_duration(instance: projectm_handle, seconds: f64);
    pub fn projectm_set_hard_cut_enabled(instance: projectm_handle, enabled: bool);
    pub fn projectm_set_preset_locked(instance: projectm_handle, lock: bool);
    pub fn projectm_set_aspect_correction(instance: projectm_handle, enabled: bool);
    pub fn projectm_set_mesh_size(instance: projectm_handle, width: usize, height: usize);
    pub fn projectm_set_texture_search_paths(
        instance: projectm_handle,
        texture_search_paths: *const *const c_char,
        count: usize,
    );

    // audio.h
    pub fn projectm_pcm_get_max_samples() -> u32;
    pub fn projectm_pcm_add_float(
        instance: projectm_handle,
        samples: *const f32,
        count: u32,
        channels: projectm_channels,
    );

    // logging.h
    /// Process-wide unless `current_thread_only`. Pass `None` to remove it.
    pub fn projectm_set_log_callback(
        callback: projectm_log_callback,
        current_thread_only: bool,
        user_data: *mut c_void,
    );

    // callbacks.h
    pub fn projectm_set_preset_switch_requested_event_callback(
        instance: projectm_handle,
        callback: projectm_preset_switch_requested_event,
        user_data: *mut c_void,
    );
    pub fn projectm_set_preset_switch_failed_event_callback(
        instance: projectm_handle,
        callback: projectm_preset_switch_failed_event,
        user_data: *mut c_void,
    );
}
