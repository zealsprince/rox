//! The Windows file associations for `.roxw`, `.roxp`, `.roxo` and `rox://`
//! links, written under `HKCU\Software\Classes` and pointed at the running
//! exe. The installer writes the same keys (`scripts/rox.iss`), but the
//! self-update swaps only the exe files, so an install that updated itself
//! into a version with new keys never sees them. [`heal`] closes that gap at
//! each launch, the way the AppImage's menu entry heals on Linux.
//!
//! A portable run writes nothing, since it leaves no trace on the machine.
//! The ProgIDs and the scheme are shared with the installer, so they change
//! together.

use std::path::Path;

/// Extension, ProgID and the type name Explorer shows.
const DOCUMENTS: [(&str, &str, &str); 3] = [
    (".roxw", "rox.workspace", "rox workspace"),
    (".roxp", "rox.plugin", "rox plugin"),
    (".roxo", "rox.object", "rox object"),
];

/// One string value. `key` is relative to `Software\Classes`, and an empty
/// `name` is the key's default value.
#[derive(Debug, PartialEq, Eq)]
struct Entry {
    key: String,
    name: &'static str,
    data: String,
}

fn entry(key: &str, name: &'static str, data: &str) -> Entry {
    Entry {
        key: key.to_string(),
        name,
        data: data.to_string(),
    }
}

fn entries(exe: &Path) -> Vec<Entry> {
    let exe = exe.display();
    let icon = format!("{exe},0");
    let open = format!("\"{exe}\" \"%1\"");
    let mut out = Vec::new();

    for (extension, prog_id, type_name) in DOCUMENTS {
        out.push(entry(extension, "", prog_id));
        out.push(entry(prog_id, "", type_name));
        out.push(entry(&format!("{prog_id}\\DefaultIcon"), "", &icon));
        out.push(entry(
            &format!("{prog_id}\\shell\\open\\command"),
            "",
            &open,
        ));
    }

    out.push(entry("rox", "", "URL:rox"));
    out.push(entry("rox", "URL Protocol", ""));
    out.push(entry("rox\\shell\\open\\command", "", &open));
    out
}

/// Rewrite every value that differs from what this exe wants. Called once
/// at launch.
#[cfg(windows)]
pub fn heal() {
    if rox_core::settings::portable() {
        return;
    }

    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            log::warn!("file types: no path for the running exe: {e}");
            return;
        }
    };

    let mut written = 0;
    for entry in entries(&exe) {
        if registry::read(&entry).as_deref() == Some(entry.data.as_str()) {
            continue;
        }

        match registry::write(&entry) {
            Ok(()) => written += 1,
            Err(code) => log::warn!(
                "file types: writing {}\\{} failed with error {code}",
                entry.key,
                entry.name
            ),
        }
    }

    if written > 0 {
        registry::notify_shell();
        log::info!(
            "file types: wrote {written} registry values for {}",
            exe.display()
        );
    }
}

#[cfg(windows)]
mod registry {
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegGetValueW, RegSetKeyValueW,
    };
    use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
    use windows::core::PCWSTR;

    use super::Entry;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn subkey(entry: &Entry) -> Vec<u16> {
        wide(&format!("Software\\Classes\\{}", entry.key))
    }

    /// A null name addresses the key's default value.
    fn name(entry: &Entry, buffer: &[u16]) -> PCWSTR {
        if entry.name.is_empty() {
            PCWSTR::null()
        } else {
            PCWSTR(buffer.as_ptr())
        }
    }

    /// The stored string, or `None` when it's missing, isn't a string, or
    /// is longer than the one we'd write. Each of those means a rewrite.
    pub(super) fn read(entry: &Entry) -> Option<String> {
        let subkey = subkey(entry);
        let value_name = wide(entry.name);

        // Two spare units: the terminator and one more, so an equal-length
        // prefix of a longer value can't pass as a match.
        let mut buffer = vec![0u16; entry.data.encode_utf16().count() + 2];
        let mut bytes = (buffer.len() * 2) as u32;

        // SAFETY: both names are NUL-terminated buffers that outlive the
        // call, and `bytes` is the size of `buffer` in bytes.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey.as_ptr()),
                name(entry, &value_name),
                RRF_RT_REG_SZ,
                None,
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut bytes),
            )
        };
        if status != ERROR_SUCCESS {
            return None;
        }

        // RRF_RT_REG_SZ guarantees the terminator, and `bytes` counts it.
        let units = (bytes as usize / 2).saturating_sub(1);
        Some(String::from_utf16_lossy(&buffer[..units]))
    }

    /// Creates the key when it's missing. The error is the Win32 code.
    pub(super) fn write(entry: &Entry) -> Result<(), u32> {
        let subkey = subkey(entry);
        let value_name = wide(entry.name);
        let data = wide(&entry.data);

        // SAFETY: every pointer is a NUL-terminated buffer that outlives the
        // call, and the byte count covers `data` with its terminator.
        let status = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey.as_ptr()),
                name(entry, &value_name),
                REG_SZ.0,
                Some(data.as_ptr().cast()),
                (data.len() * 2) as u32,
            )
        };

        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(status.0)
        }
    }

    /// Explorer caches icons and handlers until told the associations moved.
    pub(super) fn notify_shell() {
        // SAFETY: SHCNE_ASSOCCHANGED takes no items.
        unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value<'a>(entries: &'a [Entry], key: &str, name: &str) -> Option<&'a str> {
        entries
            .iter()
            .find(|e| e.key == key && e.name == name)
            .map(|e| e.data.as_str())
    }

    #[test]
    fn every_document_points_at_the_exe() {
        let exe = Path::new(r"C:\Users\Some One\AppData\Local\Programs\rox\rox.exe");
        let entries = entries(exe);
        let open = r#""C:\Users\Some One\AppData\Local\Programs\rox\rox.exe" "%1""#;

        for (extension, prog_id, type_name) in DOCUMENTS {
            assert_eq!(value(&entries, extension, ""), Some(prog_id));
            assert_eq!(value(&entries, prog_id, ""), Some(type_name));
            assert_eq!(
                value(&entries, &format!("{prog_id}\\DefaultIcon"), ""),
                Some(r"C:\Users\Some One\AppData\Local\Programs\rox\rox.exe,0")
            );
            assert_eq!(
                value(&entries, &format!("{prog_id}\\shell\\open\\command"), ""),
                Some(open)
            );
        }

        assert_eq!(
            DOCUMENTS.map(|(extension, prog_id, _)| (extension, prog_id)),
            [
                (".roxw", "rox.workspace"),
                (".roxp", "rox.plugin"),
                (".roxo", "rox.object"),
            ]
        );
    }

    #[test]
    fn the_scheme_is_a_url_protocol() {
        let entries = entries(Path::new(r"C:\rox\rox.exe"));

        assert_eq!(value(&entries, "rox", ""), Some("URL:rox"));
        assert_eq!(value(&entries, "rox", "URL Protocol"), Some(""));
        assert_eq!(
            value(&entries, "rox\\shell\\open\\command", ""),
            Some(r#""C:\rox\rox.exe" "%1""#)
        );
    }

    #[test]
    fn no_value_is_listed_twice() {
        let entries = entries(Path::new(r"C:\rox\rox.exe"));

        for (i, a) in entries.iter().enumerate() {
            assert!(
                !entries[i + 1..]
                    .iter()
                    .any(|b| b.key == a.key && b.name == a.name),
                "{a:?}"
            );
        }
    }
}
