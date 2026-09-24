//! What presets the engine has to choose from: a plain walk for `.milk`
//! files over user-dropped packs under `milkdrop_dir()/presets`. No parsing
//! or validation; libprojectM's parser reports a broken preset at load time.
//!
//! The one structure read back is the directory layout, since packs organise
//! themselves in category folders (Cream of the Crop's ~9800 presets sit in
//! `Fractal`, `Dancer`, ...). [`PresetLibrary::folders`] offers those as
//! [`Rotation`]s.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use walkdir::WalkDir;

/// What Next, Previous and the timed switch walk. An explicit pick from the
/// full list still loads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Rotation {
    #[default]
    All,
    /// At any depth below it.
    Folder(PathBuf),
    /// The user's favorites. Paths the library doesn't hold are skipped, so a
    /// deleted favorite drops out rather than breaking the rotation.
    Set(Vec<PathBuf>),
}

/// Presets under a set of roots, plus the texture directories projectM
/// searches. `PartialEq` so a rescan can skip pushing an unchanged list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PresetLibrary {
    roots: Vec<PathBuf>,
    presets: Vec<PathBuf>,
    textures: Vec<PathBuf>,
}

impl PresetLibrary {
    /// Walk `roots` for `*.milk` (case-insensitive), sorted by path. Missing
    /// roots are skipped: the default directory doesn't exist until the user
    /// fills it. Symlinks aren't followed, so a loop can't hang the scan.
    ///
    /// Every `textures/` directory found joins the search list after
    /// `textures`: projectM only looks where it's told, and big packs ship
    /// their images inside the pack.
    pub fn scan(roots: &[PathBuf], textures: Option<PathBuf>) -> PresetLibrary {
        let mut presets = Vec::new();
        let mut found_textures = Vec::new();
        for root in roots {
            for entry in WalkDir::new(root).follow_links(false).into_iter().flatten() {
                let file_type = entry.file_type();
                if file_type.is_file() && is_preset(entry.path()) {
                    presets.push(entry.into_path());
                } else if file_type.is_dir() && is_textures_dir(entry.path()) {
                    found_textures.push(entry.into_path());
                }
            }
        }
        presets.sort();
        // Overlapping roots would repeat a preset in the shuffle.
        presets.dedup();
        found_textures.sort();
        found_textures.dedup();

        // The app's own folder goes first: projectM keeps the first file it
        // finds under a name.
        let mut textures: Vec<PathBuf> = textures.into_iter().collect();
        for found in found_textures {
            if !textures.contains(&found) {
                textures.push(found);
            }
        }

        PresetLibrary {
            roots: roots.to_vec(),
            presets,
            textures,
        }
    }

    pub fn presets(&self) -> &[PathBuf] {
        &self.presets
    }

    /// Fold favorites into the scan: the list is app-wide, but each panel and
    /// the backdrop scan their own roots. Non-`.milk` and missing paths drop.
    pub fn extend(&mut self, extra: &[PathBuf]) {
        let missing: Vec<PathBuf> = extra
            .iter()
            .filter(|path| is_preset(path) && path.is_file() && self.index_of(path).is_none())
            .cloned()
            .collect();
        if missing.is_empty() {
            return;
        }
        self.presets.extend(missing);
        self.presets.sort();
        self.presets.dedup();
    }

    pub fn is_empty(&self) -> bool {
        self.presets.is_empty()
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// In search order.
    pub fn textures(&self) -> &[PathBuf] {
        &self.textures
    }

    pub fn index_of(&self, path: &Path) -> Option<usize> {
        self.presets
            .binary_search_by(|p| p.as_path().cmp(path))
            .ok()
    }

    /// Directories that directly hold a preset. A pack root holding only
    /// category folders would just be All under another name.
    pub fn folders(&self) -> Vec<PathBuf> {
        let mut folders: Vec<PathBuf> = self
            .presets
            .iter()
            .filter_map(|preset| preset.parent())
            .map(Path::to_path_buf)
            .collect();
        // Sorting by full path doesn't group parents in order: `a/b/x` sorts before `a/x`.
        folders.sort();
        folders.dedup();
        folders
    }

    /// Empty means nothing matched, which the worker treats as All. Matches by
    /// path component, so `Fractal` leaves `Fractal Extras` alone.
    pub fn rotation_indices(&self, rotation: &Rotation) -> Vec<usize> {
        match rotation {
            Rotation::All => (0..self.presets.len()).collect(),
            Rotation::Folder(folder) => self
                .presets
                .iter()
                .enumerate()
                .filter(|(_, preset)| preset.starts_with(folder))
                .map(|(index, _)| index)
                .collect(),
            Rotation::Set(paths) => {
                let mut indices: Vec<usize> = paths
                    .iter()
                    .filter_map(|path| self.index_of(path))
                    .collect();
                // Once each, in library order.
                indices.sort_unstable();
                indices.dedup();
                indices
            }
        }
    }
}

fn is_preset(path: &Path) -> bool {
    !is_junk(path)
        && path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("milk"))
}

/// Finder's .DS_Store and macOS's AppleDouble `._name` sidecars, which keep
/// the `.milk` extension and would fill the rotation with unparseable files.
/// Same rule as `rox_library::scanner::is_junk`, duplicated because this crate
/// reaches no higher than rox-viz (ADR 28).
fn is_junk(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };

    name == ".DS_Store" || name.starts_with("._")
}

fn is_textures_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("textures"))
}

/// xorshift64* off the clock: the tree carries `rand` only transitively, in
/// three major versions, and preset picks don't need quality randomness.
pub struct Shuffle {
    state: u64,
}

impl Shuffle {
    pub fn new() -> Shuffle {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x2545_F491_4F6C_DD1D);
        Shuffle::from_seed(nanos)
    }

    pub fn from_seed(seed: u64) -> Shuffle {
        // Zero is xorshift's fixed point.
        Shuffle { state: seed | 1 }
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Avoids `except` with one retry, not a loop.
    pub fn pick(&mut self, len: usize, except: Option<usize>) -> Option<usize> {
        if len == 0 {
            return None;
        }
        let mut index = (self.next_u64() % len as u64) as usize;
        if Some(index) == except && len > 1 {
            index = (index + 1) % len;
        }
        Some(index)
    }
}

impl Default for Shuffle {
    fn default() -> Self {
        Shuffle::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, b"").unwrap();
    }

    #[test]
    fn scan_finds_nested_presets_and_ignores_everything_else() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("b.milk"));
        touch(&root.join("nested/deeper/a.MILK"));
        touch(&root.join("nested/notes.txt"));
        touch(&root.join("milk"));

        let library = PresetLibrary::scan(&[root.to_path_buf()], None);
        assert_eq!(
            library.presets(),
            &[root.join("b.milk"), root.join("nested/deeper/a.MILK")]
        );
        assert!(!library.is_empty());
    }

    /// A pack unzipped off a stick or SMB share on macOS.
    #[test]
    fn scan_skips_the_junk_macos_leaves_beside_presets() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("a.milk"));
        touch(&root.join("._a.milk"));
        touch(&root.join(".DS_Store"));
        touch(&root.join("nested/._b.MILK"));

        let library = PresetLibrary::scan(&[root.to_path_buf()], None);
        assert_eq!(library.presets(), &[root.join("a.milk")]);

        // Favorites take the same filter.
        let mut library = library;
        library.extend(&[root.join("._a.milk")]);
        assert_eq!(library.presets(), &[root.join("a.milk")]);
    }

    #[test]
    fn scan_sorts_by_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for name in ["c.milk", "a.milk", "b.milk"] {
            touch(&root.join(name));
        }

        let library = PresetLibrary::scan(&[root.to_path_buf()], None);
        assert_eq!(
            library.presets(),
            &[
                root.join("a.milk"),
                root.join("b.milk"),
                root.join("c.milk")
            ]
        );
    }

    #[test]
    fn scan_of_an_empty_or_missing_root_is_empty_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let library = PresetLibrary::scan(
            &[dir.path().to_path_buf(), dir.path().join("does-not-exist")],
            None,
        );
        assert!(library.is_empty());
        assert_eq!(library.presets(), &[] as &[PathBuf]);
    }

    #[test]
    fn overlapping_roots_yield_each_preset_once() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("pack/one.milk"));

        let library = PresetLibrary::scan(&[root.to_path_buf(), root.join("pack")], None);
        assert_eq!(library.presets(), &[root.join("pack/one.milk")]);
    }

    #[test]
    fn textures_and_roots_are_kept_for_projectm() {
        let dir = tempfile::tempdir().unwrap();
        let textures = dir.path().join("textures");
        let library = PresetLibrary::scan(&[dir.path().to_path_buf()], Some(textures.clone()));
        assert_eq!(library.textures(), &[textures]);
        assert_eq!(library.roots(), &[dir.path().to_path_buf()]);
    }

    #[test]
    fn scan_picks_up_texture_folders_shipped_inside_packs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let own = root.join("own-textures");
        touch(&root.join("pack/presets/a.milk"));
        touch(&root.join("pack/textures/001.jpg"));
        touch(&root.join("other/Textures/b.png"));
        touch(&root.join("other/textures.txt"));

        let library = PresetLibrary::scan(&[root.to_path_buf()], Some(own.clone()));
        assert_eq!(
            library.textures(),
            &[own, root.join("other/Textures"), root.join("pack/textures")]
        );
        assert_eq!(library.presets(), &[root.join("pack/presets/a.milk")]);
    }

    #[test]
    fn a_texture_folder_named_explicitly_and_found_by_the_walk_is_listed_once() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let textures = root.join("textures");
        touch(&textures.join("001.jpg"));

        let library = PresetLibrary::scan(&[root.to_path_buf()], Some(textures.clone()));
        assert_eq!(library.textures(), &[textures]);
    }

    #[test]
    fn index_of_locates_a_preset_in_the_sorted_list() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for name in ["a.milk", "b.milk"] {
            touch(&root.join(name));
        }

        let library = PresetLibrary::scan(&[root.to_path_buf()], None);
        assert_eq!(library.index_of(&root.join("b.milk")), Some(1));
        assert_eq!(library.index_of(&root.join("gone.milk")), None);
    }

    #[test]
    fn folders_lists_only_directories_that_hold_presets() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("Fractal/one.milk"));
        touch(&root.join("Fractal/deep/two.milk"));
        touch(&root.join("Dancer/three.milk"));
        touch(&root.join("Geometric/textures/tile.png"));
        touch(&root.join("Geometric/inner/four.milk"));

        let library = PresetLibrary::scan(&[root.to_path_buf()], None);
        assert_eq!(
            library.folders(),
            vec![
                root.join("Dancer"),
                root.join("Fractal"),
                root.join("Fractal/deep"),
                root.join("Geometric/inner"),
            ]
        );
    }

    #[test]
    fn folders_lists_a_directory_once_however_many_presets_it_holds() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for name in ["a.milk", "b.milk", "c.milk"] {
            touch(&root.join("Fractal").join(name));
        }

        let library = PresetLibrary::scan(&[root.to_path_buf()], None);
        assert_eq!(library.folders(), vec![root.join("Fractal")]);
    }

    #[test]
    fn rotation_all_selects_the_whole_library() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("Fractal/one.milk"));
        touch(&root.join("Dancer/two.milk"));

        let library = PresetLibrary::scan(&[root.to_path_buf()], None);
        assert_eq!(library.rotation_indices(&Rotation::All), vec![0, 1]);
        assert_eq!(library.rotation_indices(&Rotation::default()), vec![0, 1]);
    }

    #[test]
    fn a_folder_rotation_takes_everything_below_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("Dancer/a.milk"));
        touch(&root.join("Fractal/b.milk"));
        touch(&root.join("Fractal/deep/c.milk"));

        let library = PresetLibrary::scan(&[root.to_path_buf()], None);
        let indices = library.rotation_indices(&Rotation::Folder(root.join("Fractal")));
        let picked: Vec<&PathBuf> = indices.iter().map(|i| &library.presets()[*i]).collect();
        assert_eq!(
            picked,
            vec![
                &root.join("Fractal/b.milk"),
                &root.join("Fractal/deep/c.milk")
            ]
        );
    }

    #[test]
    fn a_folder_rotation_that_matches_nothing_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("Fractal/a.milk"));

        let library = PresetLibrary::scan(&[root.to_path_buf()], None);
        assert!(
            library
                .rotation_indices(&Rotation::Folder(root.join("Deleted")))
                .is_empty()
        );
    }

    #[test]
    fn a_folder_rotation_does_not_swallow_a_sibling_with_the_same_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("Fractal/a.milk"));
        touch(&root.join("Fractal Extras/b.milk"));
        touch(&root.join("Fractalish/c.milk"));

        let library = PresetLibrary::scan(&[root.to_path_buf()], None);
        let indices = library.rotation_indices(&Rotation::Folder(root.join("Fractal")));
        let picked: Vec<&PathBuf> = indices.iter().map(|i| &library.presets()[*i]).collect();
        assert_eq!(picked, vec![&root.join("Fractal/a.milk")]);
    }

    #[test]
    fn a_set_rotation_keeps_only_what_the_library_holds() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("Dancer/a.milk"));
        touch(&root.join("Fractal/b.milk"));
        touch(&root.join("Fractal/c.milk"));

        let library = PresetLibrary::scan(&[root.to_path_buf()], None);
        let indices = library.rotation_indices(&Rotation::Set(vec![
            root.join("Fractal/c.milk"),
            root.join("Dancer/a.milk"),
            root.join("Fractal/c.milk"),
            root.join("Fractal/deleted.milk"),
        ]));
        assert_eq!(indices, vec![0, 2]);
        assert!(
            library
                .rotation_indices(&Rotation::Set(Vec::new()))
                .is_empty()
        );
    }

    #[test]
    fn extend_folds_in_files_from_outside_the_roots() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("pack/a.milk"));
        touch(&root.join("elsewhere/z.milk"));
        touch(&root.join("elsewhere/notes.txt"));

        let mut library = PresetLibrary::scan(&[root.join("pack")], None);
        library.extend(&[
            root.join("elsewhere/z.milk"),
            root.join("pack/a.milk"),
            root.join("elsewhere/notes.txt"),
            root.join("elsewhere/gone.milk"),
        ]);
        assert_eq!(
            library.presets(),
            &[root.join("elsewhere/z.milk"), root.join("pack/a.milk")]
        );
        assert_eq!(library.index_of(&root.join("pack/a.milk")), Some(1));
    }

    #[test]
    fn shuffle_stays_in_range_and_avoids_the_current_pick() {
        let mut shuffle = Shuffle::from_seed(12345);
        for _ in 0..200 {
            let index = shuffle.pick(8, Some(3)).unwrap();
            assert!(index < 8);
            assert_ne!(index, 3);
        }
        assert_eq!(shuffle.pick(1, Some(0)), Some(0));
        assert_eq!(shuffle.pick(0, None), None);
    }

    #[test]
    fn shuffle_does_not_get_stuck_on_one_value() {
        let mut shuffle = Shuffle::from_seed(1);
        let first = shuffle.pick(64, None).unwrap();
        assert!((0..200).any(|_| shuffle.pick(64, None).unwrap() != first));
    }
}
