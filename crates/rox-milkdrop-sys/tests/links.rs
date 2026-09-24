//! Proves the static libraries linked. The version string is the one call
//! that needs no GL context, which CI doesn't have.

use std::ffi::CStr;

#[test]
fn reports_a_version_from_the_linked_library() {
    unsafe {
        let raw = rox_milkdrop_sys::projectm_get_version_string();
        assert!(!raw.is_null(), "projectm_get_version_string returned null");
        let version = CStr::from_ptr(raw).to_string_lossy().into_owned();
        rox_milkdrop_sys::projectm_free_string(raw);
        // The FBO render entry point is 4.2.0; a system 3.x would be a silent wrong-link.
        assert!(
            version.starts_with("4."),
            "expected a projectM 4 library, got {version}"
        );
    }
}
