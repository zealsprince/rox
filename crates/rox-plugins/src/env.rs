//! The environment a package starts with. A plugin or renderer is a program
//! from someone else, and rox's own environment can hold tokens and keys it
//! happened to be started with, so a package gets a fixed list of variables
//! from it and nothing else.
//!
//! This is no sandbox. A plugin still runs as the user, reads their files
//! and reaches their session bus, which is what the card says it can do. The
//! list only keeps what rox was handed from being handed on. What confines a
//! renderer is [`crate::sandbox`], which drops more of these for one.

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

use crate::search;

/// Names a package inherits from rox when rox has them set, each with what
/// breaks without it. Every `LC_*` passes too (`passes`).
pub const ALLOWED: &[&str] = &[
    // The user's files and identity. Without HOME, programs that keep a
    // cache or config under it fail or write to the working directory;
    // shell scripts read USER and LOGNAME and get nothing.
    "HOME",
    "USER",
    "LOGNAME",
    // Messages, sorting and how file names decode. Without them a C
    // program runs in the C locale.
    "LANG",
    "LANGUAGE",
    // A Nix-built program reads locales from /run/current-system, which
    // only NixOS has: under Nix elsewhere this is how it finds them.
    "LOCALE_ARCHIVE",
    // A zone the user picked for rox; without it times show in the system's.
    "TZ",
    // Where temporary files go. On macOS TMPDIR is the user's private
    // folder, and without it they land in the shared /tmp.
    "TMPDIR",
    "TEMP",
    "TMP",
    // XDG_RUNTIME_DIR holds the Wayland and session bus sockets. The rest
    // say where the user keeps data, config and cache when it isn't the
    // default, and XDG_DATA_DIRS is where icons, MIME data and schemas
    // are found on NixOS.
    "XDG_RUNTIME_DIR",
    "XDG_DATA_HOME",
    "XDG_CONFIG_HOME",
    "XDG_CACHE_HOME",
    "XDG_DATA_DIRS",
    "XDG_CONFIG_DIRS",
    // The desktop session, for a package that opens a browser to sign in,
    // shows a notification or keeps a token in the keyring. The session bus
    // reaches the user's session as rox does; see the module docs.
    "WAYLAND_DISPLAY",
    "DISPLAY",
    "XAUTHORITY",
    "XDG_SESSION_TYPE",
    "XDG_CURRENT_DESKTOP",
    "DBUS_SESSION_BUS_ADDRESS",
    // A GPU renderer opens libvulkan.so.1 by name. On NixOS no default
    // library path holds it, so without this one the load fails.
    "LD_LIBRARY_PATH",
    // Which driver and GPU the Vulkan and EGL loaders pick, set by a
    // wrapper such as nixGL on another distro or by a user choosing a GPU.
    // Without them a renderer loads another driver or none, and a texture
    // rox made on one GPU can't be imported on another.
    "VK_ICD_FILENAMES",
    "VK_DRIVER_FILES",
    "VK_ADD_DRIVER_FILES",
    "VK_LAYER_PATH",
    "VK_ADD_LAYER_PATH",
    "__EGL_VENDOR_LIBRARY_FILENAMES",
    "__EGL_VENDOR_LIBRARY_DIRS",
    "LIBGL_DRIVERS_PATH",
    "DRI_PRIME",
    "MESA_VK_DEVICE_SELECT",
    "__NV_PRIME_RENDER_OFFLOAD",
    "__VK_LAYER_NV_optimus",
    // A network plugin behind a proxy reaches nothing without these.
    // Tools read either case, so both pass.
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "all_proxy",
    // Where the trusted certificates are when they aren't at the system
    // default. A Nix-built program on macOS or another distro reads
    // NIX_SSL_CERT_FILE, and without it every TLS connection fails.
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NIX_SSL_CERT_FILE",
    // Windows. A process without SystemRoot can't load Winsock or the
    // crypto provider; ComSpec runs `cmd`, PATHEXT is how a bare program
    // name is found, and the profile and program folders are where Python
    // and installers look for the user's and the system's files. The
    // processor and OS names are what Python's `platform` reads.
    "SystemRoot",
    "SystemDrive",
    "windir",
    "ComSpec",
    "PATHEXT",
    "USERPROFILE",
    "USERNAME",
    "HOMEDRIVE",
    "HOMEPATH",
    "APPDATA",
    "LOCALAPPDATA",
    "ProgramData",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "ProgramW6432",
    "CommonProgramFiles",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
    "OS",
    // macOS: the user's text encoding, which CoreFoundation reads for
    // every string it converts.
    "__CF_USER_TEXT_ENCODING",
];

/// Extra names to pass for a developer testing a package, comma-separated.
pub const ESCAPE: &str = "ROX_PLUGIN_ENV";

/// The names [`ESCAPE`] lists, read once.
pub fn escape() -> &'static [String] {
    static NAMES: OnceLock<Vec<String>> = OnceLock::new();

    NAMES.get_or_init(|| {
        std::env::var(ESCAPE)
            .map(|list| parse_escape(&list))
            .unwrap_or_default()
    })
}

/// Logs the escape at startup when it's set, so a run that hands packages
/// more than the list says so.
pub fn announce() {
    let names = escape();
    if !names.is_empty() {
        log::info!("{ESCAPE} is set: packages also get {}", names.join(", "));
    }
}

fn parse_escape(list: &str) -> Vec<String> {
    list.split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

/// Gives `command` a package's environment: the allowed names rox has set,
/// the escape's, the plugin search path as PATH and the Python three. What
/// the caller already set on `command` comes last and wins. `dir` is the
/// package's folder, None for the interpreter probe.
pub fn prepare(command: &mut Command, dir: Option<&Path>) {
    prepare_from(command, dir, std::env::vars_os(), escape());
}

/// [`prepare`] from a given parent environment, so a test never reads or
/// writes the process's own.
pub(crate) fn prepare_from(
    command: &mut Command,
    dir: Option<&Path>,
    parent: impl IntoIterator<Item = (OsString, OsString)>,
    escape: &[String],
) {
    // env_clear drops what was set on the command too, so it's kept aside
    // and put back after.
    let own: Vec<(OsString, Option<OsString>)> = command
        .get_envs()
        .map(|(name, value)| (name.to_os_string(), value.map(OsStr::to_os_string)))
        .collect();

    command.env_clear();
    command.envs(parent.into_iter().filter(|(name, _)| passes(name, escape)));

    command
        .env("PATH", search::search_path(dir))
        // A Python plugin's first import would write `__pycache__` into its
        // own folder and change its own hash.
        .env("PYTHONDONTWRITEBYTECODE", "1")
        // A piped stdout is block-buffered, so answers would sit unsent.
        .env("PYTHONUNBUFFERED", "1")
        // Windows reads piped text in the ANSI code page, not UTF-8.
        .env("PYTHONUTF8", "1");

    for (name, value) in own {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
}

/// Whether a variable of rox's reaches a package.
fn passes(name: &OsStr, escape: &[String]) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };

    // Windows names are case-insensitive: `Path` and `PATH` are one.
    let same = |a: &str, b: &str| match cfg!(windows) {
        true => a.eq_ignore_ascii_case(b),
        false => a == b,
    };

    name.starts_with("LC_")
        || ALLOWED.iter().any(|allowed| same(allowed, name))
        || escape.iter().any(|extra| same(extra, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stand-in host hands a renderer this environment too, from a
    /// copy of the list.
    #[test]
    fn the_stand_in_passes_the_same_names() {
        let source = include_str!("../../../examples/renderers/standin.py");
        let start = source
            .find("\nALLOWED = [")
            .expect("standin.py lists ALLOWED");
        let list = &source[start..];
        let list = &list[..list.find(']').expect("the list ends")];

        let names: Vec<&str> = list.split('"').skip(1).step_by(2).collect();
        assert_eq!(names, ALLOWED);
    }

    #[test]
    fn the_escape_splits_on_commas_and_drops_blanks() {
        assert_eq!(
            parse_escape(" RUST_BACKTRACE, ,MY_KEY,"),
            ["RUST_BACKTRACE", "MY_KEY"]
        );
        assert!(parse_escape("").is_empty());
    }

    #[test]
    fn only_listed_names_and_locale_categories_pass() {
        let none: &[String] = &[];

        for name in ["HOME", "LANG", "LC_ALL", "LC_TIME", "XDG_RUNTIME_DIR"] {
            assert!(passes(OsStr::new(name), none), "{name}");
        }
        for name in [
            "SECRET_TOKEN",
            "AWS_SECRET_ACCESS_KEY",
            "PYTHONPATH",
            "ROX_DEBUG",
        ] {
            assert!(!passes(OsStr::new(name), none), "{name}");
        }

        assert!(passes(OsStr::new("MY_KEY"), &["MY_KEY".to_string()]));
    }

    #[test]
    fn what_the_caller_set_survives_the_clear() {
        let mut command = Command::new("true");
        command.env("ROX_RENDER_API", "1").env("HOME", "/caller");

        let parent = [
            (OsString::from("HOME"), OsString::from("/parent")),
            (OsString::from("SECRET_TOKEN"), OsString::from("s3cret")),
        ];
        prepare_from(&mut command, None, parent, &[]);

        let envs: Vec<(String, Option<String>)> = command
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();

        let get = |name: &str| {
            envs.iter()
                .find(|(k, _)| k == name)
                .and_then(|(_, v)| v.clone())
        };
        assert_eq!(get("ROX_RENDER_API").as_deref(), Some("1"));
        assert_eq!(get("HOME").as_deref(), Some("/caller"));
        assert_eq!(get("SECRET_TOKEN"), None);
    }
}
