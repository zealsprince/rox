//! Turning OS-handed paths into playable ones. Both the command line
//! (`rox song.flac`) and an external file drop onto the window land here to
//! get filtered down to files the engine can actually decode before they hit
//! the path-based player queue.

use std::path::{Path, PathBuf};

/// True for a file whose extension rox recognizes as audio. Shares the
/// scanner's list so an opened or dropped file is judged the same way a
/// scanned one is.
fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| {
            rox_library::scanner::EXTENSIONS
                .iter()
                .any(|x| e.eq_ignore_ascii_case(x))
        })
}

/// Every audio file directly under a directory, sorted so a dropped folder
/// enqueues in a stable order. Shallow on purpose - a folder drop grabs the
/// tracks sitting in it, not a whole recursive tree.
fn audio_files_in_dir(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && is_audio_file(p))
            .collect(),
        Err(_) => return Vec::new(),
    };
    files.sort();
    files
}

/// Resolve OS-handed paths into a flat, ordered list the player can take:
/// existing audio files pass through, existing directories expand to the
/// audio files sitting in them, everything else drops. Order is preserved so
/// `rox a.flac b.flac` plays a then b.
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
        } else if path.is_file() && is_audio_file(&path) {
            out.push(path);
        }
    }
    out
}

/// What the OS asked us to do with the files on the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchMode {
    /// Play the files now, replacing what was loaded. The .desktop default
    /// action and a bare `rox song.flac`.
    Play,
    /// Append the files to the up-next queue. The .desktop "Add to Queue"
    /// action passes `--enqueue`.
    Enqueue,
}

/// The launch mode and audio files the app was opened with, parsed off the real
/// argv. A leading `--enqueue`/`-e` flips to queue mode; everything else is
/// treated as a path, filtered to decodable audio and expanded. Files are
/// empty on a plain launch, so nothing routes into playback then.
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
