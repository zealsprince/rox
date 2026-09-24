//! The AppImage's menu entry. Packages register with the desktop when they
//! install; an AppImage is one self-mounting file, so nothing tells the app
//! menu it exists. This module writes the launcher entry and the logo into
//! the user's XDG data folder and nowhere else, and removes the same two
//! files.
//!
//! The entry names the .AppImage by absolute path, and people move that
//! file, so [`heal`] compares the entry's `Exec=` against `$APPIMAGE` at each
//! launch and rewrites it when they differ. It also refreshes the version
//! after an in-place update.
//!
//! The offer is made once, on the welcome window, and the answer lives in
//! the session file. Outside an AppImage everything here reports
//! [`Status::Unavailable`], so callers need no platform checks.

use std::path::{Path, PathBuf};

use rox_core::install;
use rox_core::settings::Settings;

/// The shipped entry; only its `Exec=` and `Icon=` lines are rewritten.
const TEMPLATE: &str = include_str!("../../assets/app/rox.desktop");

const ENTRY_STEM: &str = install::APP_ID_RDNS;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Not an AppImage, or no data folder to write into.
    Unavailable,
    /// No entry and no answer yet: the welcome window asks.
    NotOffered,
    /// Turned down; only the settings row offers it again.
    Declined,
    /// The entry exists and names this executable.
    Integrated { exec: PathBuf },
}

/// Resolved once per call and handed down, so tests can point both paths
/// at a temp dir.
struct Layout {
    data_home: PathBuf,
    appimage: PathBuf,
}

impl Layout {
    fn applications(&self) -> PathBuf {
        self.data_home.join("applications")
    }

    fn entry(&self) -> PathBuf {
        self.applications().join(format!("{ENTRY_STEM}.desktop"))
    }

    /// The scalable hicolor slot, which every icon theme falls back to.
    fn icon(&self) -> PathBuf {
        self.data_home
            .join("icons/hicolor/scalable/apps")
            .join(format!("{ENTRY_STEM}.svg"))
    }
}

fn layout() -> Option<Layout> {
    let appimage = install::appimage()?;
    let data_home = dirs::data_dir()?;

    Some(Layout {
        data_home,
        appimage: appimage.to_path_buf(),
    })
}

pub fn status() -> Status {
    let Some(layout) = layout() else {
        return Status::Unavailable;
    };

    status_in(&layout, Settings::load().session.appimage_menu_declined)
}

pub fn install() -> Result<(), String> {
    let layout = layout().ok_or_else(|| "not running from an AppImage".to_string())?;
    install_in(&layout)
}

pub fn remove() -> Result<(), String> {
    let layout = layout().ok_or_else(|| "not running from an AppImage".to_string())?;
    remove_in(&layout)
}

/// Rewrite an entry whose path or version is stale. Called once at launch.
pub fn heal() {
    let Some(layout) = layout() else {
        return;
    };

    match heal_in(&layout) {
        Ok(Some(was)) => log::info!(
            "desktop entry: rewritten for {} (was {was})",
            layout.appimage.display()
        ),
        Ok(None) => {}
        Err(reason) => log::warn!("desktop entry: {reason}"),
    }
}

fn status_in(layout: &Layout, declined: bool) -> Status {
    let installed = std::fs::read_to_string(layout.entry())
        .ok()
        .and_then(|text| installed_exec(&text));

    match installed {
        Some(exec) => Status::Integrated { exec },
        None if declined => Status::Declined,
        None => Status::NotOffered,
    }
}

fn install_in(layout: &Layout) -> Result<(), String> {
    // Icon first, so the entry never shows a blank tile between the writes.
    let icon = rox_design::assets::Assets::get("app/rox-music.svg")
        .ok_or_else(|| "the logo is missing from the build".to_string())?;
    write_atomic(&layout.icon(), &icon.data)?;
    write_atomic(&layout.entry(), entry_text(&layout.appimage).as_bytes())?;

    refresh_database(&layout.applications());
    Ok(())
}

fn remove_in(layout: &Layout) -> Result<(), String> {
    remove_if_present(&layout.entry())?;
    remove_if_present(&layout.icon())?;

    refresh_database(&layout.applications());
    Ok(())
}

/// `Some(previous)` when the entry was rewritten.
fn heal_in(layout: &Layout) -> Result<Option<String>, String> {
    let Ok(text) = std::fs::read_to_string(layout.entry()) else {
        return Ok(None);
    };
    let Some(exec) = installed_exec(&text) else {
        return Ok(None);
    };

    let version = installed_version(&text);
    let stale = if exec != layout.appimage {
        Some(exec.display().to_string())
    } else if version != Some(env!("CARGO_PKG_VERSION")) {
        Some(format!("version {}", version.unwrap_or("unset")))
    } else {
        None
    };
    let Some(was) = stale else {
        return Ok(None);
    };

    install_in(layout)?;
    Ok(Some(was))
}

/// `TryExec=` makes a menu hide the entry while the file is gone.
fn entry_text(appimage: &Path) -> String {
    let exec = exec_arg(appimage);
    let mut out = String::new();

    for line in TEMPLATE.lines() {
        if let Some(rest) = line.strip_prefix("Exec=rox") {
            out.push_str(&format!("Exec={exec}{rest}\n"));
        } else if line.starts_with("Icon=") {
            out.push_str(&format!("Icon={ENTRY_STEM}\n"));
        } else {
            out.push_str(line);
            out.push('\n');
        }

        // `Type=` only appears in the main group, so the extras never land in the
        // action group.
        if line == "Type=Application" {
            out.push_str(&format!("TryExec={}\n", appimage.display()));
            out.push_str(concat!(
                "X-AppImage-Version=",
                env!("CARGO_PKG_VERSION"),
                "\n"
            ));
        }
    }

    out
}

/// The desktop entry spec reserves backslash, double quote, dollar and
/// backtick inside a quoted argument, and `%` everywhere as a field code.
fn exec_arg(path: &Path) -> String {
    let mut out = String::from("\"");

    for c in path.to_string_lossy().chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '%' => out.push_str("%%"),
            c => out.push(c),
        }
    }

    out.push('"');
    out
}

/// [`exec_arg`] run backwards. An unquoted hand-edited line reads up to its
/// first space.
fn installed_exec(text: &str) -> Option<PathBuf> {
    let value = text.lines().find_map(|line| line.strip_prefix("Exec="))?;

    let mut path = String::new();
    match value.strip_prefix('"') {
        Some(quoted) => {
            let mut chars = quoted.chars();
            loop {
                match chars.next()? {
                    '\\' => path.push(chars.next()?),
                    '"' => break,
                    c => path.push(c),
                }
            }
        }
        None => path.push_str(value.split(' ').next()?),
    }

    Some(PathBuf::from(path.replace("%%", "%")))
}

fn installed_version(text: &str) -> Option<&str> {
    text.lines()
        .find_map(|line| line.strip_prefix("X-AppImage-Version="))
}

/// Temp file and rename, so the menu never reads a half-written entry.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| format!("{}: no parent folder", path.display()))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;

    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = path.with_file_name(format!("{name}.tmp"));
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;

    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{}: {e}", path.display()));
    }
    Ok(())
}

fn remove_if_present(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Rebuild the MIME cache "Open With" reads. Best effort.
fn refresh_database(applications: &Path) {
    use std::process::{Command, Stdio};

    let _ = Command::new("update-desktop-database")
        .arg(applications)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The AppImage path carries a space on purpose.
    fn scratch(name: &str) -> Layout {
        let root =
            std::env::temp_dir().join(format!("rox-desktop-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        Layout {
            data_home: root.join("share"),
            appimage: root.join("My Apps/rox.AppImage"),
        }
    }

    fn cleanup(layout: &Layout) {
        let _ = std::fs::remove_dir_all(layout.data_home.parent().unwrap());
    }

    #[test]
    fn install_writes_both_files_with_quoted_exec_lines() {
        let layout = scratch("install");

        install_in(&layout).unwrap();

        let entry = std::fs::read_to_string(layout.entry()).unwrap();
        let exec = format!("\"{}\"", layout.appimage.display());
        assert!(entry.contains(&format!("Exec={exec} %F\n")), "{entry}");
        assert!(
            entry.contains(&format!("Exec={exec} --enqueue %F\n")),
            "{entry}"
        );
        assert!(entry.contains("Icon=com.zealsprince.rox\n"), "{entry}");
        assert!(
            entry.contains(&format!("TryExec={}\n", layout.appimage.display())),
            "{entry}"
        );
        assert!(
            entry.contains(concat!(
                "X-AppImage-Version=",
                env!("CARGO_PKG_VERSION"),
                "\n"
            )),
            "{entry}"
        );
        assert!(entry.contains("MimeType=audio/flac;"), "{entry}");
        assert!(entry.contains("[Desktop Action Enqueue]\n"), "{entry}");
        assert!(!entry.contains("Exec=rox"), "{entry}");

        let icon = std::fs::read(layout.icon()).unwrap();
        assert!(
            icon.starts_with(b"<") || icon.starts_with(b"\xef\xbb\xbf<"),
            "an svg"
        );

        cleanup(&layout);
    }

    #[test]
    fn status_round_trips_the_path_and_reads_the_flag() {
        let layout = scratch("status");

        assert_eq!(status_in(&layout, false), Status::NotOffered);
        assert_eq!(status_in(&layout, true), Status::Declined);

        install_in(&layout).unwrap();
        let integrated = Status::Integrated {
            exec: layout.appimage.clone(),
        };
        assert_eq!(status_in(&layout, false), integrated);
        // An entry on disk outranks the declined flag.
        assert_eq!(status_in(&layout, true), integrated);

        cleanup(&layout);
    }

    #[test]
    fn heal_rewrites_after_the_file_moves() {
        let layout = scratch("heal");
        install_in(&layout).unwrap();
        assert_eq!(heal_in(&layout).unwrap(), None, "current entry, no rewrite");

        let moved = Layout {
            data_home: layout.data_home.clone(),
            appimage: layout
                .data_home
                .parent()
                .unwrap()
                .join("Elsewhere/rox.AppImage"),
        };
        let was = heal_in(&moved).unwrap();
        assert_eq!(was.as_deref(), Some(layout.appimage.to_str().unwrap()));
        assert_eq!(
            status_in(&moved, false),
            Status::Integrated {
                exec: moved.appimage.clone()
            }
        );

        let entry = std::fs::read_to_string(moved.entry()).unwrap();
        let old = entry.replace(
            concat!("X-AppImage-Version=", env!("CARGO_PKG_VERSION")),
            "X-AppImage-Version=0.0.1",
        );
        std::fs::write(moved.entry(), old).unwrap();
        assert_eq!(heal_in(&moved).unwrap().as_deref(), Some("version 0.0.1"));
        assert_eq!(
            installed_version(&std::fs::read_to_string(moved.entry()).unwrap()),
            Some(env!("CARGO_PKG_VERSION"))
        );

        remove_in(&moved).unwrap();
        assert_eq!(heal_in(&moved).unwrap(), None);

        cleanup(&layout);
    }

    #[test]
    fn remove_leaves_nothing_behind() {
        let layout = scratch("remove");
        install_in(&layout).unwrap();
        assert!(layout.entry().exists() && layout.icon().exists());

        remove_in(&layout).unwrap();
        assert!(!layout.entry().exists());
        assert!(!layout.icon().exists());
        assert!(
            !layout
                .entry()
                .with_file_name("com.zealsprince.rox.desktop.tmp")
                .exists()
        );
        assert_eq!(status_in(&layout, false), Status::NotOffered);

        remove_in(&layout).unwrap();

        cleanup(&layout);
    }

    #[test]
    fn exec_quoting_round_trips_reserved_characters() {
        let awkward = Path::new("/mnt/Zeal/100% \"mine\" $HOME/`rox`\\.AppImage");
        let text = entry_text(awkward);
        assert!(
            text.contains(r#"Exec="/mnt/Zeal/100%% \"mine\" \$HOME/\`rox\`\\.AppImage" %F"#),
            "{text}"
        );
        assert_eq!(installed_exec(&text).as_deref(), Some(awkward));

        assert_eq!(
            installed_exec("[Desktop Entry]\nExec=/opt/rox.AppImage %F\n").as_deref(),
            Some(Path::new("/opt/rox.AppImage"))
        );
        assert_eq!(installed_exec("[Desktop Entry]\nName=rox\n"), None);
        assert_eq!(installed_exec("Exec=\"/opt/rox"), None);
    }
}
