//! OS-handed paths (command line, window drop) filtered to what the engine
//! can decode. A playlist file expands to its tracks as an open, never an
//! import: nothing lands in the library.
//!
//! rox's own documents (workspaces, plugins, objects, `rox://` links) are
//! split off first and handed back as [`Document`]s for the app to route.
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

/// A file or link rox opens as itself rather than plays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Document {
    Workspace(PathBuf),
    Plugin(PathBuf),
    Object(PathBuf),
    /// A `rox://` link, kept as written.
    Link(String),
}

const LINK_PREFIX: &str = "rox://";

/// Whether an argument is a `rox://` link. The scheme folds case, as every
/// URL scheme does.
fn is_link(text: &str) -> bool {
    text.get(..LINK_PREFIX.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(LINK_PREFIX))
}

/// The document a name says it is, before anything asks the filesystem.
fn named(path: &Path) -> Option<Document> {
    if let Some(text) = path.to_str()
        && is_link(text)
    {
        return Some(Document::Link(text.to_owned()));
    }

    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let kind = match ext.as_str() {
        "roxw" => Document::Workspace,
        "roxp" => Document::Plugin,
        "roxo" => Document::Object,
        _ => return None,
    };
    Some(kind(path.to_path_buf()))
}

/// Documents off the front of the audio filter, by extension (case folded)
/// and by the `rox://` prefix. Order is kept on both sides. A document file
/// has to exist to count, and one that doesn't is dropped rather than left
/// for the audio side. A link never touches the filesystem.
pub fn split_documents<I, P>(paths: I) -> (Vec<Document>, Vec<PathBuf>)
where
    I: IntoIterator<Item = P>,
    P: Into<PathBuf>,
{
    let mut documents = Vec::new();
    let mut rest = Vec::new();
    for path in paths {
        let path = path.into();
        match named(&path) {
            Some(link @ Document::Link(_)) => documents.push(link),
            Some(document) if path.is_file() => documents.push(document),
            Some(_) => {}
            None => rest.push(path),
        }
    }
    (documents, rest)
}

/// Whether a raw launch argument names a document, decoded the way
/// [`from_args`] decodes it. A restart that re-execs with its arguments
/// leaves these out, or it would open them a second time.
pub fn is_document_arg(arg: &std::ffi::OsStr) -> bool {
    named(&from_file_url(arg.to_owned())).is_some()
}

/// What a launch was asked to open.
pub struct LaunchArgs {
    pub mode: LaunchMode,
    /// Decodable audio, playlists and folders already expanded.
    pub files: Vec<PathBuf>,
    pub documents: Vec<Document>,
}

/// The launch mode, the documents and the audio files off argv. An
/// `--enqueue`/`-e` ahead of any link flips to queue mode.
pub fn from_args() -> LaunchArgs {
    from_arg_list(std::env::args_os().skip(1))
}

fn from_arg_list(args: impl IntoIterator<Item = std::ffi::OsString>) -> LaunchArgs {
    let mut mode = LaunchMode::Play;
    let mut paths = Vec::new();
    let mut flags = true;
    for arg in args {
        if flags && (arg == "--enqueue" || arg == "-e") {
            mode = LaunchMode::Enqueue;
            continue;
        }
        // A link reaches rox from a browser or another app. Windows hands it
        // over as `"%1"`, so a crafted one can carry more arguments after it:
        // nothing from there on is read as a flag.
        if arg.to_str().is_some_and(is_link) {
            flags = false;
        }
        paths.push(from_file_url(arg));
    }

    let (documents, rest) = split_documents(paths);
    LaunchArgs {
        mode,
        files: resolve_audio_paths(rest),
        documents,
    }
}

/// A `file://` URL as its path. The desktop entry takes `%U` so links can
/// reach rox, and a file manager then hands files over as URLs. Anything
/// else, an unparsable URL included, comes back as it was.
pub fn from_file_url(arg: std::ffi::OsString) -> PathBuf {
    let decoded = arg
        .to_str()
        .filter(|text| text.len() > 7 && text[..7].eq_ignore_ascii_case("file://"))
        .and_then(|text| url::Url::parse(text).ok())
        .and_then(|url| url.to_file_path().ok());

    decoded.unwrap_or_else(|| PathBuf::from(arg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_url_becomes_its_path() {
        let url = |text: &str| from_file_url(text.into());

        #[cfg(unix)]
        {
            assert_eq!(
                url("file:///m/Bad%20&%20%C3%9Cber/one%20two.flac"),
                PathBuf::from("/m/Bad & \u{dc}ber/one two.flac")
            );
            assert_eq!(url("FILE:///m/one.flac"), PathBuf::from("/m/one.flac"));
        }
        #[cfg(windows)]
        assert_eq!(
            url("file:///C:/Music/one%20two.flac"),
            PathBuf::from(r"C:\Music\one two.flac")
        );

        // Not a file URL, or not one that names a local path: kept as given.
        assert_eq!(url("/m/one.flac"), PathBuf::from("/m/one.flac"));
        assert_eq!(url("rox://install/x"), PathBuf::from("rox://install/x"));
        assert_eq!(url("file://"), PathBuf::from("file://"));
    }

    /// A scratch folder holding empty files under `names`.
    fn scratch(name: &str, names: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in names {
            std::fs::write(dir.join(name), b"").unwrap();
        }
        dir
    }

    #[test]
    fn documents_split_off_before_the_audio_filter() {
        let dir = scratch(
            "rox-open-files-split",
            &["one.flac", "Look.roxw", "two.flac", "tones.roxp"],
        );

        let args = from_arg_list(
            ["one.flac", "Look.roxw", "two.flac", "tones.roxp"].map(|name| dir.join(name).into()),
        );

        assert_eq!(
            args.documents,
            [
                Document::Workspace(dir.join("Look.roxw")),
                Document::Plugin(dir.join("tones.roxp")),
            ]
        );
        assert_eq!(args.files, [dir.join("one.flac"), dir.join("two.flac")]);
        assert_eq!(args.mode, LaunchMode::Play);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_extension_matches_in_any_case() {
        let dir = scratch("rox-open-files-case", &["A.ROXW", "b.RoxP", "c.roxO"]);

        let (documents, rest) =
            split_documents(["A.ROXW", "b.RoxP", "c.roxO"].map(|n| dir.join(n)));

        assert_eq!(
            documents,
            [
                Document::Workspace(dir.join("A.ROXW")),
                Document::Plugin(dir.join("b.RoxP")),
                Document::Object(dir.join("c.roxO")),
            ]
        );
        assert!(rest.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_document_is_dropped() {
        let dir = scratch("rox-open-files-missing", &[]);

        let (documents, rest) = split_documents([dir.join("Gone.roxw")]);

        assert!(documents.is_empty());
        assert!(rest.is_empty(), "nor is it left for the audio side");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_document_url_becomes_its_path() {
        let dir = scratch("rox-open-files-url", &["My Look.roxw"]);
        let url = url::Url::from_file_path(dir.join("My Look.roxw")).unwrap();

        let args = from_arg_list([url.as_str().into()]);

        assert_eq!(
            args.documents,
            [Document::Workspace(dir.join("My Look.roxw"))]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_link_is_kept_as_written() {
        let (documents, rest) = split_documents(["rox://install/Some%20Id", "ROX://install/x"]);

        assert_eq!(
            documents,
            [
                Document::Link("rox://install/Some%20Id".into()),
                Document::Link("ROX://install/x".into()),
            ]
        );
        assert!(rest.is_empty());
        assert!(is_document_arg("rox://install/x".as_ref()));
    }

    #[test]
    fn nothing_after_a_link_is_a_flag() {
        let args = from_arg_list(["rox://install/x".into(), "--enqueue".into()]);
        assert_eq!(args.mode, LaunchMode::Play);
        assert_eq!(args.documents, [Document::Link("rox://install/x".into())]);

        let args = from_arg_list(["-e".into(), "rox://install/x".into()]);
        assert_eq!(
            args.mode,
            LaunchMode::Enqueue,
            "a flag before it still counts"
        );
    }

    #[test]
    fn an_object_is_a_document() {
        let dir = scratch("rox-open-files-object", &["Lamp.roxo"]);

        let (documents, _) = split_documents([dir.join("Lamp.roxo")]);

        assert_eq!(documents, [Document::Object(dir.join("Lamp.roxo"))]);
        assert!(is_document_arg(dir.join("Lamp.roxo").as_os_str()));
        assert!(!is_document_arg("one.flac".as_ref()));
        let _ = std::fs::remove_dir_all(&dir);
    }

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
