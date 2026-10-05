//! `plugin.json`, manifest v0 (ADR 30). An unknown top-level key refuses
//! the plugin, and so does one inside `entry`, since that's how the plugin
//! runs: a manifest a newer host would read differently never half-loads
//! here. Inside `meta` and `capabilities` unknown keys are ignored, so a
//! field added there later stays additive for plugins built against it.
//!
//! The entry resolves to a [`Command`] and nothing else. The paths it names
//! stay inside the plugin folder; an interpreter comes off the search path
//! ([`crate::search`]) through a fixed alias table. A `python3` candidate
//! only counts once it answers `--version` as Python 3.

use std::collections::{BTreeMap, HashMap};
use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::ops::RangeInclusive;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

use serde::Deserialize;

/// The `api` values this host speaks.
pub const SUPPORTED_API: RangeInclusive<u32> = 1..=1;

pub const FILE: &str = "plugin.json";

/// A manifest is a few hundred bytes; this only stops a huge file being read.
const MAX_BYTES: u64 = 256 * 1024;

/// A logo is a few KiB.
const MAX_ICON_BYTES: u64 = 64 * 1024;

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api: u32,
    pub entry: Entry,
    #[serde(default)]
    pub meta: Meta,
    #[serde(default)]
    pub capabilities: Capabilities,
    /// Shown to the user as found or missing on the search path; rox
    /// enforces nothing.
    #[serde(default)]
    pub programs: Vec<String>,
    #[serde(default)]
    pub config_schema: serde_json::Value,
}

/// Exactly one kind: serde refuses a map naming both or neither.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Entry {
    Script(Script),
    /// `<os>-<arch>` to a path in the folder.
    Native(BTreeMap<String, String>),
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Script {
    pub path: String,
    pub interpreter: String,
}

/// `WorkspaceMeta`'s card minus its dates.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Meta {
    pub author: String,
    pub description: String,
    pub website: String,
    pub version: String,
    pub license: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Capabilities {
    pub source: Option<SourceCap>,
    pub panels: Vec<DeclaredPanel>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct SourceCap {
    pub label: String,
    /// Off unless the plugin asks: plugin rows don't scrobble by default.
    #[serde(default)]
    pub scrobble: bool,
    /// A square SVG in the plugin's folder, drawn as a mask in the theme's
    /// text colour where the source's name would go. Empty for none.
    #[serde(default)]
    pub icon: String,
    /// Answers `source.radio`: a station seeded from a track or a node,
    /// which Play Similar starts and continuation draws from while its
    /// context plays.
    #[serde(default)]
    pub radio: bool,
    /// Answers `source.link`: the web page of a track or a node, which rox
    /// opens or copies on a click.
    #[serde(default)]
    pub links: bool,
    /// Answers `source.lyrics`: a track's words, which rox only asks for
    /// once the user switches them on for this plugin.
    #[serde(default)]
    pub lyrics: bool,
    /// Answers `source.action`: things the plugin does with its items,
    /// listed in rox's own menus.
    #[serde(default)]
    pub actions: Vec<ActionDecl>,
    /// The track actions that add to and take from the service's own
    /// favourites, which a heart in rox runs while Sync Favourites is on.
    #[serde(default)]
    pub favourites: Option<FavouritesDecl>,
}

/// Names two of the plugin's own track actions. Rows say they're in the
/// service's favourites with the [`FAVOURITE_FLAG`] flag.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct FavouritesDecl {
    pub add: String,
    pub remove: String,
}

/// The row flag a heart reads for the service's side.
pub const FAVOURITE_FLAG: &str = "favourite";

/// One entry a plugin adds to rox's menus. Nothing in it runs in rox:
/// picking it calls `source.action`, and the work happens in the plugin.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ActionDecl {
    pub id: String,
    pub label: String,
    /// Where it's offered: `track`, `node`, `source` (no item). A name this
    /// rox doesn't know is skipped, so a later target doesn't refuse the
    /// plugin.
    #[serde(default)]
    pub on: Vec<String>,
    /// The choices rox asks for before the call, as a JSON Schema object in
    /// the config page's subset. Null asks nothing.
    #[serde(default)]
    pub params: serde_json::Value,
    /// An SVG in the plugin's folder, checked like the source icon. Empty
    /// draws the plug.
    #[serde(default)]
    pub icon: String,
    /// A flag the items' rows carry (`favourite`) or lack (`!favourite`) for
    /// the action to be offered. Empty offers it on every row.
    #[serde(default)]
    pub when: String,
}

impl ActionDecl {
    pub fn offered_on(&self, target: &str) -> bool {
        self.on.iter().any(|on| on == target)
    }

    /// Whether a row with these flags can take the action. A row whose
    /// flags are unknown can take any.
    pub fn applies_to(&self, flags: Option<&[String]>) -> bool {
        let Some(flags) = flags else {
            return true;
        };

        match self.when.strip_prefix('!') {
            _ if self.when.is_empty() => true,
            Some(flag) => !flags.iter().any(|f| f == flag),
            None => flags.contains(&self.when),
        }
    }

    /// `(key, schema)` for each param, in key order as serde_json keeps it.
    pub fn param_fields(&self) -> Vec<(String, serde_json::Value)> {
        self.params["properties"]
            .as_object()
            .map(|props| props.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default()
    }
}

/// A menu with more than this many of one plugin's entries stops being a menu.
pub const MAX_ACTIONS: usize = 16;

/// A preset of a core panel kind, listed under the plugin in Add Panel.
/// Nothing in it executes: it's the same dump a saved panel preset holds.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct DeclaredPanel {
    /// The Add Panel entry.
    pub name: String,
    /// A dock `PanelState`: `panel_name`, and `info` as `{ "panel": config }`.
    pub preset: serde_json::Value,
}

impl DeclaredPanel {
    pub fn panel_name(&self) -> Option<&str> {
        self.preset.get("panel_name")?.as_str()
    }
}

/// What a panel's `source` has to be when it names one.
pub const SOURCE_PREFIX: &str = "plugin:";

/// `^[a-z0-9][a-z0-9-]{1,63}$`, spelled out rather than pulling in a regex
/// engine for one pattern.
pub fn valid_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    let plain = |b: &u8| b.is_ascii_lowercase() || b.is_ascii_digit();

    (2..=64).contains(&bytes.len())
        && plain(&bytes[0])
        && bytes[1..].iter().all(|b| plain(b) || *b == b'-')
}

/// Reads and checks `<dir>/plugin.json`. The error is the reason the Plugins
/// page shows.
pub fn load(dir: &Path) -> Result<Manifest, String> {
    let path = dir.join(FILE);
    let meta = std::fs::symlink_metadata(&path).map_err(|e| format!("{FILE}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("{FILE} is not a plain file"));
    }
    if meta.len() > MAX_BYTES {
        return Err(format!("{FILE} is larger than {MAX_BYTES} bytes"));
    }

    let text = std::fs::read_to_string(&path).map_err(|e| format!("{FILE}: {e}"))?;
    parse(&text)
}

pub fn parse(text: &str) -> Result<Manifest, String> {
    let manifest: Manifest = serde_json::from_str(text).map_err(|e| format!("{FILE}: {e}"))?;

    if !valid_id(&manifest.id) {
        return Err(format!(
            "{FILE}: id {:?} is not a valid plugin id",
            manifest.id
        ));
    }
    if !SUPPORTED_API.contains(&manifest.api) {
        return Err(format!(
            "{FILE}: api {} is outside what this rox supports ({}..={})",
            manifest.api,
            SUPPORTED_API.start(),
            SUPPORTED_API.end()
        ));
    }
    check_panels(&manifest)?;
    check_actions(&manifest)?;

    Ok(manifest)
}

fn check_actions(manifest: &Manifest) -> Result<(), String> {
    let Some(source) = &manifest.capabilities.source else {
        return Ok(());
    };

    if source.actions.len() > MAX_ACTIONS {
        return Err(format!("{FILE}: more than {MAX_ACTIONS} actions"));
    }

    let mut ids = std::collections::HashSet::new();
    for action in &source.actions {
        let refuse = |why: &str| Err(format!("{FILE}: action {:?} {why}", action.id));

        if action.id.is_empty() || action.id.len() > 64 {
            return Err(format!("{FILE}: an action's id is empty or over 64 bytes"));
        }
        if !ids.insert(action.id.as_str()) {
            return refuse("is declared twice");
        }
        if action.label.trim().is_empty() {
            return refuse("has no label");
        }
        if action.on.is_empty() {
            return refuse("is offered nowhere; `on` is empty");
        }
        if !action.when.is_empty() && !flag_name(action.when.trim_start_matches('!')) {
            return refuse("has a `when` that isn't a flag name or one negated with `!`");
        }

        let params_ok = match &action.params {
            serde_json::Value::Null => true,
            serde_json::Value::Object(schema) => schema
                .get("properties")
                .is_none_or(|props| props.is_object()),
            _ => false,
        };
        if !params_ok {
            return refuse("has params that aren't a JSON Schema object");
        }
    }

    let Some(favourites) = &source.favourites else {
        return Ok(());
    };

    for id in [&favourites.add, &favourites.remove] {
        let declared = source.actions.iter().find(|action| action.id == *id);
        if !declared.is_some_and(|action| action.offered_on("track")) {
            return Err(format!(
                "{FILE}: favourites names {id:?}, which isn't an action on tracks"
            ));
        }
    }
    if favourites.add == favourites.remove {
        return Err(format!(
            "{FILE}: favourites adds and removes with one action"
        ));
    }

    Ok(())
}

/// The shape a declared panel can take. Whether its kind exists is the
/// app's call, since only the app has the catalog: a kind this rox doesn't
/// know is skipped there, not refused here, so a plugin written for a
/// later rox still runs.
fn check_panels(manifest: &Manifest) -> Result<(), String> {
    let mut names = std::collections::HashSet::new();
    for panel in &manifest.capabilities.panels {
        let refuse = |why: &str| Err(format!("{FILE}: panel {:?} {why}", panel.name));

        if panel.name.trim().is_empty() {
            return Err(format!("{FILE}: a declared panel has no name"));
        }
        if !names.insert(panel.name.as_str()) {
            return refuse("is declared twice");
        }

        let Some(kind) = panel.panel_name().filter(|kind| !kind.is_empty()) else {
            return refuse("has no panel_name");
        };

        // One panel, not an arrangement: a container would carry panels of
        // any kind inside it, past the check below.
        let no_children = match panel.preset.get("children") {
            None => true,
            Some(children) => children.as_array().is_some_and(|c| c.is_empty()),
        };
        if !no_children {
            return refuse("has children; a declared panel is a single panel");
        }

        let info = panel.preset.get("info").and_then(|info| info.as_object());
        let config = match info {
            Some(info) if info.len() == 1 => info.get("panel").and_then(|c| c.as_object()),
            _ => None,
        };
        let Some(config) = config else {
            return refuse("needs info as { \"panel\": { ... } }");
        };

        // A plugin's panel shows its own source, never another plugin's.
        if kind == "source browser" {
            let own = format!("{SOURCE_PREFIX}{}", manifest.id);
            if config.get("source").and_then(|s| s.as_str()) != Some(own.as_str()) {
                return refuse(&format!("has to name its own source, {own}"));
            }
        }
    }

    Ok(())
}

/// The key a native entry is looked up under, `<os>-<arch>`. Rust's own
/// names for the three are the contract's.
pub fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// The command that starts this plugin: its own binary, or the interpreter
/// with the script as its first argument. Nothing about cwd, pipes or
/// environment; the process module sets those.
pub fn entry_for(manifest: &Manifest, dir: &Path) -> Result<Command, String> {
    let path = crate::search::search_path(Some(dir));
    entry_with(manifest, dir, &platform(), &path, &probed)
}

/// Whether a candidate interpreter, run with its args, is the one it claims.
type Probe<'a> = &'a dyn Fn(&Path, &[&str]) -> bool;

fn entry_with(
    manifest: &Manifest,
    dir: &Path,
    platform: &str,
    path_var: &OsStr,
    probe: Probe,
) -> Result<Command, String> {
    match &manifest.entry {
        Entry::Script(script) => {
            let file = inside(dir, &script.path, "entry")?;
            let (program, args) = interpreter(&script.interpreter, path_var, probe)
                .ok_or_else(|| format!("interpreter {} not found", script.interpreter))?;

            let mut command = Command::new(program);
            command.args(args).arg(file);

            Ok(command)
        }

        Entry::Native(builds) => {
            let rel = builds
                .get(platform)
                .ok_or_else(|| format!("no build for {platform}"))?;

            Ok(Command::new(inside(dir, rel, "entry")?))
        }
    }
}

/// A plugin's own path, refused if it could name anything outside its folder.
fn inside(dir: &Path, rel: &str, what: &str) -> Result<PathBuf, String> {
    let rel_path = Path::new(rel);
    let plain = rel_path
        .components()
        .all(|c| matches!(c, Component::Normal(_)));
    if rel.is_empty() || !plain {
        return Err(format!("{what} path {rel:?} leaves the plugin folder"));
    }

    let full = dir.join(rel_path);
    if !full.is_file() {
        return Err(format!("{what} {rel} is missing"));
    }

    Ok(full)
}

/// The source icon's bytes, None when the manifest names none.
pub fn icon_for(manifest: &Manifest, dir: &Path) -> Result<Option<Vec<u8>>, String> {
    let Some(rel) = manifest
        .capabilities
        .source
        .as_ref()
        .map(|cap| cap.icon.as_str())
        .filter(|rel| !rel.is_empty())
    else {
        return Ok(None);
    };

    svg_in(dir, rel).map(Some)
}

/// Each action's icon by action id, for the actions that name one.
pub fn action_icons_for(manifest: &Manifest, dir: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    let Some(source) = manifest.capabilities.source.as_ref() else {
        return Ok(Vec::new());
    };

    source
        .actions
        .iter()
        .filter(|action| !action.icon.is_empty())
        .map(|action| {
            svg_in(dir, &action.icon)
                .map(|bytes| (action.id.clone(), bytes))
                .map_err(|e| format!("action {:?}: {e}", action.id))
        })
        .collect()
}

/// An icon's bytes: an SVG inside the folder, small, drawing nothing from
/// outside itself.
fn svg_in(dir: &Path, rel: &str) -> Result<Vec<u8>, String> {
    if !rel.to_ascii_lowercase().ends_with(".svg") {
        return Err(format!("icon {rel} isn't an .svg file"));
    }

    let path = inside(dir, rel, "icon")?;
    let size = std::fs::metadata(&path)
        .map_err(|e| format!("icon {rel}: {e}"))?
        .len();
    if size > MAX_ICON_BYTES {
        return Err(format!("icon {rel} is over {} KiB", MAX_ICON_BYTES / 1024));
    }

    let bytes = std::fs::read(&path).map_err(|e| format!("icon {rel}: {e}"))?;

    // The SVG renderer reads an image an <image> or <feImage> names from
    // disk, so an icon that holds one could draw any file the user can read.
    let text = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
    if ["<image", "<feimage", "<foreignobject"]
        .iter()
        .any(|tag| text.contains(tag))
    {
        return Err(format!("icon {rel} embeds an image, which an icon can't"));
    }

    Ok(bytes)
}

/// What a row flag and an action's `when` may be named.
fn flag_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// What to try, in order, for an interpreter name. Any other name is looked
/// up as written.
fn aliases(name: &str) -> Vec<(&str, &'static [&'static str])> {
    match name {
        // The py launcher first on Windows: `python3` and `python` there are
        // often the Store stub, and `python` can be Python 2 anywhere.
        "python3" => {
            let mut tries: Vec<(&str, &'static [&'static str])> = Vec::new();
            if cfg!(windows) {
                tries.push(("py", &["-3"]));
            }
            tries.extend([("python3", &[] as &[&str]), ("python", &[])]);

            tries
        }

        "node" => vec![("node", &[])],

        other => vec![(other, &[])],
    }
}

/// A program's full path, looked up the way a plugin's PATH is built: the
/// plugin's `bin/` when there's a plugin, the user's extra folders, then PATH.
pub fn find_program(program: &str, plugin_dir: Option<&Path>) -> Option<PathBuf> {
    on_path(program, &crate::search::search_path(plugin_dir))
}

/// Whether a program the manifest lists is on the plugin's search path.
/// Information for the Plugins page; nothing is enforced with it.
pub fn program_found(program: &str, plugin_dir: &Path) -> bool {
    find_program(program, Some(plugin_dir)).is_some()
}

fn interpreter(name: &str, path_var: &OsStr, probe: Probe) -> Option<(PathBuf, Vec<&'static str>)> {
    // Only the python3 aliases have known impostors (the Store stub, Python
    // 2); anything else is taken as found.
    let checked = name == "python3";

    aliases(name).into_iter().find_map(|(program, args)| {
        let found = on_path(program, path_var)?;
        let usable = !checked || probe(&found, args);

        usable.then(|| (found, args.to_vec()))
    })
}

/// A hung candidate is as good as a missing one.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// A candidate: the program found and the args it runs with.
type Candidate = (PathBuf, Vec<String>);

/// One probe per candidate per session.
static PROBED: OnceLock<Mutex<HashMap<Candidate, bool>>> = OnceLock::new();

/// Drops every remembered answer, so the next resolve probes again. The
/// Plugins page's Rescan calls this, which is how a Python installed while
/// rox runs gets found.
pub fn forget_probes() {
    if let Some(cache) = PROBED.get() {
        cache.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

/// [`speaks_python3`], remembered.
fn probed(program: &Path, args: &[&str]) -> bool {
    let key: Candidate = (
        program.to_path_buf(),
        args.iter().map(|arg| arg.to_string()).collect(),
    );
    let cache = PROBED.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(&known) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return known;
    }

    let answer = speaks_python3(program, args, PROBE_TIMEOUT);
    cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, answer);

    answer
}

/// Runs `<program> <args> --version` and wants a clean exit and a "Python 3"
/// on stdout or stderr (Python 2 prints its version to stderr). That rules
/// out the Store stub and a `python` that's Python 2.
fn speaks_python3(program: &Path, args: &[&str], within: Duration) -> bool {
    let deadline = Instant::now() + within;

    let mut command = Command::new(program);
    command
        .args(args)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(crate::process::CREATE_NO_WINDOW);
    }

    let Ok(mut child) = command.spawn() else {
        return false;
    };

    // Read off-thread so a candidate that never closes its pipes can't hold
    // the scan past the deadline.
    let (Some(mut stdout), Some(mut stderr)) = (child.stdout.take(), child.stderr.take()) else {
        let _ = child.kill();
        let _ = child.wait();
        return false;
    };
    let (sent, output) = mpsc::channel();
    std::thread::spawn(move || {
        let mut out = String::new();
        let mut err = String::new();
        let _ = stdout.read_to_string(&mut out);
        let _ = stderr.read_to_string(&mut err);
        let _ = sent.send((out, err));
    });

    let remaining = deadline.saturating_duration_since(Instant::now());
    let Ok((out, err)) = output.recv_timeout(remaining) else {
        let _ = child.kill();
        let _ = child.wait();
        return false;
    };

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => break None,
        }
    };
    let Some(status) = status else {
        let _ = child.kill();
        let _ = child.wait();
        return false;
    };

    let says = |text: &str| text.trim_start().starts_with("Python 3");
    status.success() && (says(&out) || says(&err))
}

/// The first `PATH` entry holding `program`, trying Windows' executable
/// extensions after the bare name.
fn on_path(program: &str, path_var: &OsStr) -> Option<PathBuf> {
    let exts: Vec<OsString> = match cfg!(windows) {
        true => std::env::var_os("PATHEXT")
            .unwrap_or_else(|| ".EXE;.CMD;.BAT;.COM".into())
            .to_string_lossy()
            .split(';')
            .filter(|ext| !ext.is_empty())
            .map(OsString::from)
            .collect(),
        false => Vec::new(),
    };

    for dir in std::env::split_paths(path_var) {
        let bare = dir.join(program);
        if bare.is_file() {
            return Some(bare);
        }

        for ext in &exts {
            let mut name = OsString::from(program);
            name.push(ext);

            let with = dir.join(name);
            if with.is_file() {
                return Some(with);
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let dir = std::env::temp_dir()
                .join(format!("rox-plugins-manifest-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();

            Scratch(dir)
        }

        fn file(&self, rel: &str) -> PathBuf {
            let path = self.0.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "x").unwrap();

            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn accept(_: &Path, _: &[&str]) -> bool {
        true
    }

    fn script_manifest(extra: &str) -> String {
        format!(
            r#"{{
                "id": "example-tones",
                "name": "Tones",
                "version": "0.1.0",
                "api": 1,
                "entry": {{ "script": {{ "path": "tones.py", "interpreter": "python3" }} }},
                "meta": {{ "author": "", "description": "", "website": "", "license": "" }},
                "capabilities": {{ "source": {{ "label": "Tones", "scrobble": false }} }},
                "programs": [],
                "config_schema": {{ "type": "object", "properties": {{}} }}
                {extra}
            }}"#
        )
    }

    fn with_entry(entry: &str) -> String {
        format!(r#"{{ "id": "tones", "name": "T", "version": "1", "api": 1, "entry": {entry} }}"#)
    }

    #[test]
    fn the_contract_example_parses() {
        let manifest = parse(&script_manifest("")).expect("the example is valid");

        assert_eq!(manifest.id, "example-tones");
        assert_eq!(
            manifest.entry,
            Entry::Script(Script {
                path: "tones.py".into(),
                interpreter: "python3".into()
            })
        );
        assert_eq!(
            manifest.capabilities.source,
            Some(SourceCap {
                label: "Tones".into(),
                scrobble: false,
                icon: String::new(),
                radio: false,
                links: false,
                lyrics: false,
                actions: Vec::new(),
                favourites: None,
            })
        );
    }

    fn with_actions(actions: &str) -> String {
        script_manifest("").replace(
            r#""scrobble": false"#,
            &format!(r#""scrobble": false, "actions": {actions}"#),
        )
    }

    #[test]
    fn actions_parse_with_their_params() {
        let manifest = parse(&with_actions(
            r#"[{ "id": "export", "label": "Export WAV", "on": ["track", "node", "later"],
                  "params": { "type": "object", "properties": {
                      "seconds": { "type": "integer" }, "loud": { "type": "boolean" } } } }]"#,
        ))
        .expect("a declared action loads");

        let action = &manifest.capabilities.source.unwrap().actions[0];
        assert!(action.offered_on("track") && action.offered_on("node"));
        assert!(!action.offered_on("source"));

        let mut fields: Vec<String> = action.param_fields().into_iter().map(|f| f.0).collect();
        fields.sort();
        assert_eq!(fields, ["loud", "seconds"]);
    }

    #[test]
    fn an_actions_when_reads_a_rows_flags() {
        let manifest = parse(&with_actions(
            r#"[{ "id": "add", "label": "Add", "on": ["track"], "when": "!favourite" },
                { "id": "drop", "label": "Drop", "on": ["track"], "when": "favourite" },
                { "id": "any", "label": "Any", "on": ["track"] }]"#,
        ))
        .unwrap();
        let actions = manifest.capabilities.source.unwrap().actions;
        let (add, drop, any) = (&actions[0], &actions[1], &actions[2]);

        let favourite = ["favourite".to_string(), "offline".to_string()];
        let plain = ["offline".to_string()];

        assert!(!add.applies_to(Some(&favourite)) && add.applies_to(Some(&plain)));
        assert!(drop.applies_to(Some(&favourite)) && !drop.applies_to(Some(&plain)));
        assert!(any.applies_to(Some(&favourite)) && any.applies_to(Some(&[])));
        assert!(
            add.applies_to(None) && drop.applies_to(None),
            "a row whose flags nobody knows can take either"
        );
    }

    fn with_favourites(favourites: &str) -> String {
        with_actions(
            r#"[{ "id": "add", "label": "Add", "on": ["track"], "when": "!favourite" },
                { "id": "drop", "label": "Drop", "on": ["track"], "when": "favourite" },
                { "id": "save", "label": "Save", "on": ["node"] }]"#,
        )
        .replace(
            r#""scrobble": false"#,
            &format!(r#""scrobble": false, "favourites": {favourites}"#),
        )
    }

    #[test]
    fn favourites_name_two_track_actions() {
        let manifest = parse(&with_favourites(r#"{ "add": "add", "remove": "drop" }"#)).unwrap();
        assert_eq!(
            manifest.capabilities.source.unwrap().favourites,
            Some(FavouritesDecl {
                add: "add".into(),
                remove: "drop".into()
            })
        );

        let cases = [
            (r#"{ "add": "add", "remove": "gone" }"#, "\"gone\""),
            (r#"{ "add": "save", "remove": "drop" }"#, "on tracks"),
            (r#"{ "add": "add", "remove": "add" }"#, "one action"),
        ];
        for (favourites, why) in cases {
            let err = parse(&with_favourites(favourites)).unwrap_err();
            assert!(err.contains(why), "{favourites}: {err}");
        }
    }

    #[test]
    fn a_broken_action_refuses_the_plugin() {
        let cases = [
            (r#"[{ "id": "", "label": "X", "on": ["track"] }]"#, "id"),
            (r#"[{ "id": "a", "label": " ", "on": ["track"] }]"#, "label"),
            (r#"[{ "id": "a", "label": "X", "on": [] }]"#, "nowhere"),
            (
                r#"[{ "id": "a", "label": "X", "on": ["track"] }, { "id": "a", "label": "Y", "on": ["node"] }]"#,
                "twice",
            ),
            (
                r#"[{ "id": "a", "label": "X", "on": ["track"], "params": [1] }]"#,
                "JSON Schema",
            ),
            (
                r#"[{ "id": "a", "label": "X", "on": ["track"], "when": "Fav ourite" }]"#,
                "flag name",
            ),
            (
                r#"[{ "id": "a", "label": "X", "on": ["track"], "when": "!" }]"#,
                "flag name",
            ),
        ];

        for (actions, why) in cases {
            let err = parse(&with_actions(actions)).unwrap_err();
            assert!(err.contains(why), "{actions}: {err}");
        }
    }

    #[test]
    fn an_unknown_key_is_refused() {
        let err = parse(&script_manifest(r#", "tools": []"#)).unwrap_err();
        assert!(err.contains("unknown field"), "{err}");

        let entry = script_manifest("").replace(
            r#""interpreter": "python3""#,
            r#""interpreter": "python3", "args": ["-u"]"#,
        );
        assert!(
            parse(&entry).is_err(),
            "the entry is strict: it's how the plugin runs"
        );
    }

    #[test]
    fn unknown_keys_inside_capabilities_and_meta_are_ignored() {
        let text = script_manifest("")
            .replace(r#""scrobble": false"#, r#""scrobble": false, "later": 1"#)
            .replace(r#""license": """#, r#""license": "", "funding": "x""#)
            .replace(r#""source": {"#, r#""lyrics": [], "source": {"#);

        let manifest = parse(&text).expect("additions below the top level are additive");
        assert!(manifest.capabilities.source.is_some());
    }

    #[test]
    fn a_bad_id_is_refused() {
        for id in [
            "",
            "a",
            "Tones",
            "-tones",
            "to nes",
            "tönes",
            &"a".repeat(65),
        ] {
            let text = script_manifest("").replace("example-tones", id);
            assert!(parse(&text).is_err(), "{id:?} should be refused");
        }

        assert!(valid_id("ab"));
        assert!(valid_id("0-9"));
        assert!(valid_id(&"a".repeat(64)));
    }

    #[test]
    fn an_api_out_of_range_is_refused() {
        // The prototype's version is refused too, now that version 1 is fixed.
        for api in [0, 2] {
            let text = script_manifest("").replace(r#""api": 1"#, &format!(r#""api": {api}"#));
            let err = parse(&text).unwrap_err();
            assert!(err.contains(&format!("api {api}")), "{err}");
        }

        let negative = script_manifest("").replace(r#""api": 1"#, r#""api": -1"#);
        assert!(parse(&negative).is_err());
    }

    #[test]
    fn an_entry_names_exactly_one_kind() {
        let both = with_entry(
            r#"{ "script": { "path": "a.py", "interpreter": "python3" }, "native": { "linux-x86_64": "a" } }"#,
        );
        assert!(parse(&both).is_err(), "both kinds");

        assert!(parse(&with_entry("{}")).is_err(), "neither kind");

        let native = parse(&with_entry(r#"{ "native": { "linux-x86_64": "bin/a" } }"#)).unwrap();
        assert!(matches!(native.entry, Entry::Native(_)));
    }

    #[test]
    fn a_native_entry_with_no_build_for_this_platform_is_refused() {
        let scratch = Scratch::new("native");
        scratch.file("bin/a");
        let manifest = parse(&with_entry(r#"{ "native": { "linux-x86_64": "bin/a" } }"#)).unwrap();

        let err = entry_with(
            &manifest,
            &scratch.0,
            "windows-x86_64",
            OsStr::new(""),
            &accept,
        )
        .unwrap_err();
        assert_eq!(err, "no build for windows-x86_64");

        let command = entry_with(
            &manifest,
            &scratch.0,
            "linux-x86_64",
            OsStr::new(""),
            &accept,
        )
        .unwrap();
        assert_eq!(command.get_program(), scratch.0.join("bin/a").as_os_str());
    }

    #[test]
    fn a_path_escaping_the_folder_is_refused() {
        let scratch = Scratch::new("escape");
        for rel in ["../x", "a/../../x", "/etc/passwd", ""] {
            let entry = format!(r#"{{ "native": {{ "linux-x86_64": {rel:?} }} }}"#);
            let manifest = parse(&with_entry(&entry)).unwrap();

            let err = entry_with(
                &manifest,
                &scratch.0,
                "linux-x86_64",
                OsStr::new(""),
                &accept,
            )
            .unwrap_err();
            assert!(err.contains("leaves the plugin folder"), "{rel}: {err}");
        }
    }

    #[test]
    fn a_script_runs_under_the_first_alias_on_path() {
        let scratch = Scratch::new("alias");
        scratch.file("tones.py");
        let bin = scratch.0.join("bin");
        let python = scratch.file("bin/python");
        let manifest = parse(&script_manifest("")).unwrap();

        // No python3 on this PATH, so the table falls through to python.
        let command = entry_with(
            &manifest,
            &scratch.0,
            "linux-x86_64",
            bin.as_os_str(),
            &accept,
        )
        .unwrap();
        assert_eq!(command.get_program(), python.as_os_str());

        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args, vec![scratch.0.join("tones.py").as_os_str()]);

        let err = entry_with(
            &manifest,
            &scratch.0,
            "linux-x86_64",
            OsStr::new(""),
            &accept,
        )
        .unwrap_err();
        assert_eq!(err, "interpreter python3 not found");
    }

    #[test]
    fn python3_tries_the_launcher_first_on_windows() {
        let names: Vec<&str> = aliases("python3").iter().map(|(name, _)| *name).collect();

        match cfg!(windows) {
            true => assert_eq!(names, ["py", "python3", "python"]),
            false => assert_eq!(names, ["python3", "python"]),
        }
        assert_eq!(aliases("python3")[0].1.is_empty(), !cfg!(windows));
    }

    #[test]
    fn a_candidate_the_probe_rejects_is_skipped() {
        let scratch = Scratch::new("probe");
        scratch.file("tones.py");
        let bin = scratch.0.join("bin");
        let python3 = scratch.file("bin/python3");
        let python = scratch.file("bin/python");
        let manifest = parse(&script_manifest("")).unwrap();

        let asked = std::cell::RefCell::new(Vec::new());
        let not_python3 = |program: &Path, _: &[&str]| {
            asked.borrow_mut().push(program.to_path_buf());
            program != python3
        };

        let command = entry_with(
            &manifest,
            &scratch.0,
            "linux-x86_64",
            bin.as_os_str(),
            &not_python3,
        )
        .unwrap();
        assert_eq!(command.get_program(), python.as_os_str());
        assert_eq!(*asked.borrow(), [python3.clone(), python.clone()]);

        let none = |_: &Path, _: &[&str]| false;
        let err = entry_with(
            &manifest,
            &scratch.0,
            "linux-x86_64",
            bin.as_os_str(),
            &none,
        )
        .unwrap_err();
        assert_eq!(err, "interpreter python3 not found");
    }

    #[test]
    fn other_interpreters_are_not_probed() {
        let scratch = Scratch::new("node");
        scratch.file("tones.py");
        let node = scratch.file("bin/node");
        let manifest = parse(
            &script_manifest("").replace(r#""interpreter": "python3""#, r#""interpreter": "node""#),
        )
        .unwrap();

        let refuse = |_: &Path, _: &[&str]| -> bool { panic!("node is never probed") };
        let command = entry_with(
            &manifest,
            &scratch.0,
            "linux-x86_64",
            scratch.0.join("bin").as_os_str(),
            &refuse,
        )
        .unwrap();
        assert_eq!(command.get_program(), node.as_os_str());
    }

    #[test]
    fn the_plugin_bin_comes_before_the_extra_folders() {
        let scratch = Scratch::new("extras");
        scratch.file("tones.py");
        let extra = scratch.0.join("extra");
        let from_extra = scratch.file("extra/python3");
        let manifest = parse(&script_manifest("")).unwrap();

        let path = crate::search::build(
            Some(&scratch.0),
            std::slice::from_ref(&extra),
            OsStr::new(""),
        );
        let command = entry_with(&manifest, &scratch.0, "linux-x86_64", &path, &accept).unwrap();
        assert_eq!(command.get_program(), from_extra.as_os_str());

        let bundled = scratch.file("bin/python3");
        let command = entry_with(&manifest, &scratch.0, "linux-x86_64", &path, &accept).unwrap();
        assert_eq!(command.get_program(), bundled.as_os_str());
    }

    #[test]
    fn a_program_in_the_plugin_bin_is_found() {
        let scratch = Scratch::new("program");
        let tool = scratch.file("bin/rox-plugins-test-only-tool");

        assert_eq!(
            find_program("rox-plugins-test-only-tool", Some(&scratch.0)),
            Some(tool)
        );
        assert!(program_found("rox-plugins-test-only-tool", &scratch.0));
        assert_eq!(find_program("rox-plugins-test-only-tool", None), None);
    }

    #[cfg(unix)]
    fn script(scratch: &Scratch, rel: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let path = scratch.file(rel);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();

        path
    }

    #[cfg(unix)]
    #[test]
    fn the_probe_wants_a_clean_python_3() {
        let scratch = Scratch::new("real-probe");
        let two = script(&scratch, "two/python", "echo 'Python 2.7.18' >&2");
        let failing = script(&scratch, "failing/python", "echo 'Python 3.12.1'; exit 1");
        let three = script(&scratch, "three/python", "echo 'Python 3.12.1'");
        let hangs = script(&scratch, "hangs/python", "exec sleep 10");

        let within = Duration::from_secs(5);
        assert!(!speaks_python3(&two, &[], within), "Python 2");
        assert!(!speaks_python3(&failing, &[], within), "a failed run");
        assert!(speaks_python3(&three, &[], within));

        let began = Instant::now();
        assert!(!speaks_python3(&hangs, &[], Duration::from_millis(200)));
        assert!(
            began.elapsed() < Duration::from_secs(5),
            "killed at the deadline"
        );

        // Through the cached probe: python3 here is Python 2, so python wins.
        scratch.file("tones.py");
        let bin = scratch.0.join("bin");
        script(&scratch, "bin/python3", "echo 'Python 2.7.18' >&2");
        let python = script(&scratch, "bin/python", "echo 'Python 3.12.1'");
        let manifest = parse(&script_manifest("")).unwrap();

        let command = entry_with(
            &manifest,
            &scratch.0,
            "linux-x86_64",
            bin.as_os_str(),
            &probed,
        )
        .unwrap();
        assert_eq!(command.get_program(), python.as_os_str());
    }

    #[cfg(unix)]
    #[test]
    fn a_forgotten_probe_asks_again() {
        let scratch = Scratch::new("forget-probe");
        let python = script(&scratch, "python3", "echo 'Python 2.7.18' >&2");
        assert!(!probed(&python, &[]));

        // Python 3 installed at the same path while rox runs.
        script(&scratch, "python3", "echo 'Python 3.12.1'");
        assert!(!probed(&python, &[]), "remembered until forgotten");

        forget_probes();
        assert!(probed(&python, &[]));
    }

    fn with_panels(panels: &str) -> String {
        script_manifest("").replace(
            r#""capabilities": {"#,
            &format!(r#""capabilities": {{ "panels": {panels},"#),
        )
    }

    fn source_browser(name: &str, source: &str) -> String {
        format!(
            r#"{{ "name": {name:?}, "preset": {{ "panel_name": "source browser", "children": [], "info": {{ "panel": {{ "source": {source:?} }} }} }} }}"#
        )
    }

    #[test]
    fn declared_panels_parse() {
        let text = with_panels(&format!(
            r#"[{}, {{ "name": "Scope", "preset": {{ "panel_name": "oscilloscope", "info": {{ "panel": {{}} }} }} }}]"#,
            source_browser("Tones", "plugin:example-tones")
        ));
        let manifest = parse(&text).expect("both are single panels of a known shape");

        let panels = &manifest.capabilities.panels;
        assert_eq!(panels.len(), 2);
        assert_eq!(panels[0].name, "Tones");
        assert_eq!(panels[0].panel_name(), Some("source browser"));
        assert_eq!(panels[1].panel_name(), Some("oscilloscope"));
    }

    #[test]
    fn a_source_browser_on_another_source_is_refused() {
        for source in ["plugin:someone-else", "", "local"] {
            let text = with_panels(&format!("[{}]", source_browser("Tones", source)));
            let err = parse(&text).unwrap_err();
            assert!(err.contains("its own source"), "{source:?}: {err}");
        }

        let missing = with_panels(
            r#"[{ "name": "Tones", "preset": { "panel_name": "source browser", "info": { "panel": {} } } }]"#,
        );
        assert!(parse(&missing).is_err(), "no source at all");
    }

    #[test]
    fn a_declared_panel_with_children_is_refused() {
        let text = with_panels(
            r#"[{ "name": "Pair", "preset": { "panel_name": "group", "children": [{ "panel_name": "library", "children": [], "info": { "panel": {} } }], "info": { "panel": {} } } }]"#,
        );
        let err = parse(&text).unwrap_err();
        assert!(err.contains("single panel"), "{err}");
    }

    #[test]
    fn a_declared_panel_that_isnt_a_panel_is_refused() {
        for preset in [
            r#"{ "info": { "panel": {} } }"#,
            r#"{ "panel_name": "", "info": { "panel": {} } }"#,
            r#"{ "panel_name": "library" }"#,
            r#"{ "panel_name": "library", "info": { "tabs": { "active_index": 0 } } }"#,
            r#"{ "panel_name": "library", "info": { "panel": {}, "tabs": {} } }"#,
            r#"{ "panel_name": "library", "info": { "panel": 3 } }"#,
        ] {
            let text = with_panels(&format!(r#"[{{ "name": "X", "preset": {preset} }}]"#));
            assert!(parse(&text).is_err(), "{preset} should be refused");
        }
    }

    #[test]
    fn declared_panel_names_are_present_and_unique() {
        let blank = with_panels(&format!(
            "[{}]",
            source_browser(" ", "plugin:example-tones")
        ));
        assert!(parse(&blank).is_err());

        let twice = with_panels(&format!(
            "[{0}, {0}]",
            source_browser("Tones", "plugin:example-tones")
        ));
        let err = parse(&twice).unwrap_err();
        assert!(err.contains("twice"), "{err}");
    }

    #[test]
    fn a_kind_the_host_doesnt_know_still_parses() {
        let text = with_panels(
            r#"[{ "name": "Later", "preset": { "panel_name": "hologram", "info": { "panel": {} } } }]"#,
        );
        assert!(
            parse(&text).is_ok(),
            "the app skips it; the plugin still runs"
        );
    }

    #[test]
    fn a_missing_script_is_refused() {
        let scratch = Scratch::new("missing");
        let manifest = parse(&script_manifest("")).unwrap();

        let err = entry_with(
            &manifest,
            &scratch.0,
            "linux-x86_64",
            OsStr::new(""),
            &accept,
        )
        .unwrap_err();
        assert_eq!(err, "entry tones.py is missing");
    }
}
