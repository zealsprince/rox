//! User exclusion patterns: folders and files kept out of the library while
//! their parent stays in. One compiled list, applied by the scan walks and
//! the watcher's sync.
//!
//! Deliberately kept out of `scanner::is_audio`/`is_junk`: those answer for
//! explicit opens too, and a file handed to rox by name plays anyway.
//!
//! Without a `/` a pattern matches a name anywhere; with one it matches the
//! path relative to the root. A trailing `/` is dropped. Matching folds case,
//! like the volumes these trees live on.

use std::path::Path;

use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};

#[derive(Clone, Default)]
pub struct Exclusions {
    names: Option<GlobSet>,
    paths: Option<GlobSet>,
}

enum Shape {
    Name,
    Path,
}

impl Exclusions {
    /// A pattern that won't parse is logged and skipped, not fatal: the settings
    /// page refuses bad ones, so this is a hand-edit, and one typo shouldn't
    /// cost the rest.
    pub fn new(patterns: &[String]) -> Exclusions {
        let mut names = GlobSetBuilder::new();
        let mut paths = GlobSetBuilder::new();
        let (mut any_name, mut any_path) = (false, false);

        for pattern in patterns {
            match parse(pattern) {
                Ok(Some((Shape::Name, glob))) => {
                    names.add(glob);
                    any_name = true;
                }

                Ok(Some((Shape::Path, glob))) => {
                    paths.add(glob);
                    any_path = true;
                }

                Ok(None) => {}

                Err(e) => log::warn!("library exclusion {pattern:?} skipped: {e}"),
            }
        }

        // Nothing excluded beats nothing scanned.
        let build = |any: bool, builder: GlobSetBuilder| {
            any.then(|| builder.build()).and_then(|built| {
                built
                    .map_err(|e| log::warn!("library exclusions: {e}"))
                    .ok()
            })
        };
        Exclusions {
            names: build(any_name, names),
            paths: build(any_path, paths),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_none() && self.paths.is_none()
    }

    /// Whether `path` itself matches, ignoring its ancestors (a walk never
    /// descends past an excluded folder). The root never matches.
    pub fn matches(&self, root: &Path, path: &Path) -> bool {
        if self.is_empty() {
            return false;
        }

        below(root, path).is_some_and(|rel| self.hit(rel))
    }

    /// Whether `path` or any folder between it and `root` matches, for paths
    /// that arrive without a walk, like watch events.
    pub fn covers(&self, root: &Path, path: &Path) -> bool {
        if self.is_empty() {
            return false;
        }

        below(root, path).is_some_and(|rel| {
            rel.ancestors()
                .take_while(|part| !part.as_os_str().is_empty())
                .any(|part| self.hit(part))
        })
    }

    fn hit(&self, rel: &Path) -> bool {
        let name = rel.file_name();
        self.names
            .as_ref()
            .zip(name)
            .is_some_and(|(set, name)| set.is_match(name))
            || self.paths.as_ref().is_some_and(|set| set.is_match(rel))
    }
}

pub fn check(pattern: &str) -> Result<(), String> {
    parse(pattern).map(|_| ()).map_err(|e| e.kind().to_string())
}

fn below<'a>(root: &Path, path: &'a Path) -> Option<&'a Path> {
    path.strip_prefix(root)
        .ok()
        .filter(|rel| !rel.as_os_str().is_empty())
}

fn parse(pattern: &str) -> Result<Option<(Shape, Glob)>, globset::Error> {
    #[cfg(windows)]
    let pattern = pattern.replace('\\', "/");
    let pattern = pattern.trim().trim_end_matches('/');

    let (shape, glob) = if pattern.contains('/') {
        (Shape::Path, pattern.trim_start_matches('/'))
    } else {
        (Shape::Name, pattern)
    };
    if glob.is_empty() {
        return Ok(None);
    }

    // Keeps `*` inside one component: `Live/*` is only Live's direct children.
    let glob = GlobBuilder::new(glob)
        .case_insensitive(true)
        .literal_separator(true)
        .build()?;
    Ok(Some((shape, glob)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(patterns: &[&str]) -> Exclusions {
        Exclusions::new(&patterns.iter().map(|p| p.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn a_bare_pattern_matches_names_anywhere() {
        let ex = set(&["*.tmp", "@Recently-Snapshot"]);
        let root = Path::new("/m");

        assert!(ex.matches(root, Path::new("/m/a.tmp")));
        assert!(ex.matches(root, Path::new("/m/Artist/Album/b.tmp")));
        assert!(ex.matches(root, Path::new("/m/Artist/@Recently-Snapshot")));
        assert!(!ex.matches(root, Path::new("/m/Artist/a.flac")));
    }

    #[test]
    fn a_slashed_pattern_anchors_at_the_root() {
        let ex = set(&["Live/**", "/Artist/Bootlegs"]);
        let root = Path::new("/m");

        assert!(ex.matches(root, Path::new("/m/Live/set.flac")));
        assert!(ex.matches(root, Path::new("/m/Artist/Bootlegs")));
        assert!(
            !ex.matches(root, Path::new("/m/Artist/Live/set.flac")),
            "a slashed pattern is relative to the root, not to any folder"
        );
        assert!(!ex.matches(root, Path::new("/m/Other/Artist/Bootlegs")));
    }

    #[test]
    fn matching_folds_case() {
        let ex = set(&["live/**", "*.TMP"]);
        let root = Path::new("/m");

        assert!(ex.matches(root, Path::new("/m/Live/set.flac")));
        assert!(ex.matches(root, Path::new("/m/a.tmp")));
    }

    #[test]
    fn the_root_is_never_excluded() {
        let ex = set(&["m", "*"]);
        assert!(!ex.matches(Path::new("/m"), Path::new("/m")));
        assert!(!ex.covers(Path::new("/m"), Path::new("/m")));
        assert!(!ex.matches(Path::new("/m"), Path::new("/elsewhere/a.flac")));
    }

    #[test]
    fn covers_asks_every_folder_on_the_way_down() {
        let ex = set(&["Live", "Artist/Bootlegs"]);
        let root = Path::new("/m");

        assert!(!ex.matches(root, Path::new("/m/Live/2019/set.flac")));
        assert!(ex.covers(root, Path::new("/m/Live/2019/set.flac")));
        assert!(ex.covers(root, Path::new("/m/Artist/Bootlegs/a.flac")));
        assert!(!ex.covers(root, Path::new("/m/Artist/Album/a.flac")));
    }

    #[test]
    fn slashes_at_the_ends_are_trimmed() {
        let ex = set(&["Live/", "/Artist/Bootlegs/"]);
        let root = Path::new("/m");

        assert!(
            ex.matches(root, Path::new("/m/Artist/Live")),
            "Live/ is a name"
        );
        assert!(ex.matches(root, Path::new("/m/Artist/Bootlegs")));
    }

    #[test]
    fn bad_and_blank_patterns_are_dropped() {
        assert!(check("[unclosed").is_err());
        assert!(check("*.tmp").is_ok());

        let ex = set(&["[unclosed", "  ", "/", "*.tmp"]);
        assert!(ex.matches(Path::new("/m"), Path::new("/m/a.tmp")));
        assert!(set(&["", "/"]).is_empty());
    }
}
