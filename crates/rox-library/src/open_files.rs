//! OS-handed paths (command line, window drop) filtered to what the engine
//! can decode. A playlist file expands to its tracks as an open, never an
//! import: nothing lands in the library.
//!
//! Known gap: a cue subsong entry (`image.flac#3`) opens the whole image,
//! because this returns paths, not `TrackKey`s.

use std::path::{Path, PathBuf};

/// Shallow, and filtered by the scanner's own [`crate::scanner::is_audio`]
/// so macOS `._name` sidecars stay out.
fn audio_files_in_dir(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && crate::scanner::is_audio(p))
            .collect(),
        Err(_) => return Vec::new(),
    };
    files.sort();
    files
}

/// The decodable files a playlist names, relative entries resolved against
/// its folder. Stream URLs and stale paths drop.
fn audio_files_in_playlist(path: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let base = path.parent().unwrap_or_else(|| Path::new(""));
    let mut out: Vec<PathBuf> = Vec::new();
    for entry in crate::playlist_file::parse(&text) {
        let resolve = |name: &str| {
            let name = Path::new(name);
            if name.is_absolute() {
                name.to_path_buf()
            } else {
                base.join(name)
            }
        };
        let mut full = resolve(&entry);
        if !full.is_file() {
            // A cue subsong. The literal name won above if it really exists.
            let Some(image) = entry
                .rsplit_once('#')
                .filter(|(_, sub)| sub.parse::<u16>().is_ok_and(|sub| sub > 0))
                .map(|(image, _)| resolve(image))
            else {
                continue;
            };
            // Every subsong points at the same image; push it once.
            if out.last() == Some(&image) {
                continue;
            }
            full = image;
        }
        if full.is_file() && crate::scanner::is_audio(&full) {
            out.push(full);
        }
    }
    out
}

/// Files pass, directories and playlists expand, everything else drops.
/// Order is preserved.
pub fn resolve_audio_paths<I, P>(paths: I) -> Vec<PathBuf>
where
    I: IntoIterator<Item = P>,
    P: Into<PathBuf>,
{
    let mut out = Vec::new();
    for path in paths {
        let path = path.into();
        if path.is_dir() {
            out.extend(audio_files_in_dir(&path));
        } else if path.is_file() && crate::playlist_file::is_playlist_file(&path) {
            out.extend(audio_files_in_playlist(&path));
        } else if path.is_file() && crate::scanner::is_audio(&path) {
            out.push(path);
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchMode {
    Play,
    Enqueue,
}

/// The launch mode and audio files off argv. A leading `--enqueue`/`-e`
/// flips to queue mode.
pub fn from_args() -> (LaunchMode, Vec<PathBuf>) {
    let mut mode = LaunchMode::Play;
    let mut args = Vec::new();
    for arg in std::env::args_os().skip(1) {
        if arg == "--enqueue" || arg == "-e" {
            mode = LaunchMode::Enqueue;
            continue;
        }
        args.push(arg);
    }
    (mode, resolve_audio_paths(args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dropped_folder_skips_os_junk() {
        let dir = std::env::temp_dir().join("rox-open-files-junk");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in [
            "one.flac",
            "._one.flac",
            "two.flac",
            "._two.flac",
            ".DS_Store",
        ] {
            std::fs::write(dir.join(name), b"").unwrap();
        }

        assert_eq!(
            resolve_audio_paths([&dir]),
            [dir.join("one.flac"), dir.join("two.flac")],
            "only the real tracks, in sorted order"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_playlist_expands_to_its_tracks() {
        let dir = std::env::temp_dir().join("rox-open-files-playlist");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["one.flac", "two.flac"] {
            std::fs::write(dir.join(name), b"").unwrap();
        }
        let absolute = dir.join("two.flac");
        let pls = dir.join("set.pls");
        std::fs::write(
            &pls,
            format!(
                "[playlist]\n\
                 File1=one.flac\n\
                 File2={}\n\
                 File3={}\n\
                 NumberOfEntries=3\nVersion=2\n",
                absolute.display(),
                dir.join("gone.flac").display(),
            ),
        )
        .unwrap();

        assert_eq!(
            resolve_audio_paths([&pls]),
            [dir.join("one.flac"), absolute],
            "playlist order, relative against the file's folder, misses dropped"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
