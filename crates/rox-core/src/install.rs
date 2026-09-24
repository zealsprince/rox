//! How this process was installed: bare, a Flatpak, or an AppImage. The
//! last two are where the executable's own folder can't be trusted: a
//! Flatpak's `/app/bin` is unreachable from the host, and an AppImage runs
//! from a read-only squashfs mount that's gone after exit. Decided once per
//! process, so the stores can't split mid-run.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Bare covers a distro package, the tarball, nix, and dev builds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Bare,
    Flatpak,
    AppImage,
}

pub const APP_ID_RDNS: &str = "com.zealsprince.rox";

pub fn kind() -> Kind {
    static KIND: OnceLock<Kind> = OnceLock::new();

    *KIND.get_or_init(|| {
        let exe = std::env::current_exe().unwrap_or_default();
        let flatpak_info = Path::new("/.flatpak-info").exists();
        // A closure: the fn item `var_os` doesn't satisfy `Fn(&str)` for
        // every lifetime.
        detect(|name: &str| std::env::var_os(name), flatpak_info, &exe)
    })
}

fn detect(env: impl Fn(&str) -> Option<OsString>, flatpak_info: bool, exe: &Path) -> Kind {
    if flatpak_info || env("FLATPAK_ID").is_some() {
        return Kind::Flatpak;
    }

    // A shell opened from an AppImage inherits both variables, so the
    // executable has to be inside the mount too.
    let (Some(_), Some(appdir)) = (env("APPIMAGE"), env("APPDIR")) else {
        return Kind::Bare;
    };
    if exe.starts_with(PathBuf::from(appdir)) {
        return Kind::AppImage;
    }

    Kind::Bare
}

/// Stable across launches, unlike `current_exe()`, whose mount path changes
/// every run.
pub fn appimage() -> Option<&'static Path> {
    static APPIMAGE: OnceLock<Option<PathBuf>> = OnceLock::new();

    APPIMAGE
        .get_or_init(|| {
            if kind() != Kind::AppImage {
                return None;
            }
            std::env::var_os("APPIMAGE").map(PathBuf::from)
        })
        .as_deref()
}

/// Only ever true inside a Flatpak that lacks access to the picked folder.
pub fn is_portal_path(path: &Path) -> bool {
    dirs::runtime_dir().is_some_and(|dir| path.starts_with(dir.join("doc")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: HashMap<String, OsString> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), OsString::from(v)))
            .collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn nothing_set_is_bare() {
        assert_eq!(
            detect(env(&[]), false, Path::new("/usr/bin/rox")),
            Kind::Bare
        );
    }

    #[test]
    fn flatpak_by_variable() {
        let e = env(&[("FLATPAK_ID", "com.zealsprince.rox")]);
        assert_eq!(detect(e, false, Path::new("/app/bin/rox")), Kind::Flatpak);
    }

    #[test]
    fn flatpak_by_info_file() {
        assert_eq!(
            detect(env(&[]), true, Path::new("/app/bin/rox")),
            Kind::Flatpak
        );
    }

    #[test]
    fn flatpak_outranks_appimage_variables() {
        let e = env(&[
            ("FLATPAK_ID", "com.zealsprince.rox"),
            ("APPIMAGE", "/home/me/rox.AppImage"),
            ("APPDIR", "/tmp/.mount_rox1234"),
        ]);
        assert_eq!(
            detect(e, false, Path::new("/tmp/.mount_rox1234/usr/bin/rox")),
            Kind::Flatpak
        );
    }

    #[test]
    fn appimage_with_exe_inside_the_mount() {
        let e = env(&[
            ("APPIMAGE", "/home/me/rox.AppImage"),
            ("APPDIR", "/tmp/.mount_rox1234"),
        ]);
        assert_eq!(
            detect(e, false, Path::new("/tmp/.mount_rox1234/usr/bin/rox")),
            Kind::AppImage
        );
    }

    #[test]
    fn appimage_variables_without_the_exe_inside_are_bare() {
        let e = env(&[
            ("APPIMAGE", "/home/me/rox.AppImage"),
            ("APPDIR", "/tmp/.mount_rox1234"),
        ]);
        assert_eq!(detect(e, false, Path::new("/usr/bin/rox")), Kind::Bare);
    }

    #[test]
    fn one_appimage_variable_alone_is_bare() {
        let e = env(&[("APPDIR", "/tmp/.mount_rox1234")]);
        assert_eq!(
            detect(e, false, Path::new("/tmp/.mount_rox1234/usr/bin/rox")),
            Kind::Bare
        );
    }

    /// A sibling mount whose name merely extends APPDIR's doesn't match.
    #[test]
    fn appdir_prefix_is_by_component() {
        let e = env(&[
            ("APPIMAGE", "/home/me/rox.AppImage"),
            ("APPDIR", "/tmp/.mount_rox"),
        ]);
        assert_eq!(
            detect(e, false, Path::new("/tmp/.mount_rox1234/usr/bin/rox")),
            Kind::Bare
        );
    }
}
