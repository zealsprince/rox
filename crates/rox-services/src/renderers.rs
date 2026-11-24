//! Renderers (ADR 37): packages whose manifest declares `renderer`, run one
//! process per render target and handed the frame mapping that target made.
//! Underneath a renderer is ADR 30's package. It installs, hashes, approves
//! and switches on through [`crate::plugins`], and nothing here writes an
//! approval or a switch.
//!
//! A renderer is never a plugin host. It gets no `Host` and no plugin wire,
//! answers no requests, and is started only when a render target asks, never
//! with the plugins at launch. What it gets is its pipes, three variables,
//! the `hello` line and one lent handle (fd 3 on Unix). Restarting is the
//! render target's job, on the plugin host's schedule through [`due`] and
//! [`note_exit`].

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::process::{ChildStdin, ChildStdout};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::time::{Duration, Instant};

use gpui::App;
use serde_json::json;

use rox_core::settings::{self, PluginRecord, Settings};
use rox_plugins::host::{BACKOFF, CRASH_WINDOW};
use rox_plugins::process::{self, Extra, Lent, Process};
use rox_plugins::{Loaded, manifest};

/// What `ROX_RENDER_API` and the `hello` line say.
pub const API: u32 = 1;

/// The sizes a render target can ask for.
pub const WIDTHS: std::ops::RangeInclusive<u32> = 16..=3840;
pub const HEIGHTS: std::ops::RangeInclusive<u32> = 16..=2160;

/// How long [`Session::stop`] waits for a renderer to leave once its stdin
/// closes.
const GRACE: Duration = Duration::from_millis(200);

/// A renderer as the Plugins page lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Renderer {
    pub id: String,
    /// The manifest's display name.
    pub name: String,
    /// The manifest's renderer label.
    pub label: String,
    /// This exact folder was approved.
    pub approved: bool,
    /// Its record is switched on.
    pub enabled: bool,
}

/// Why [`start`] won't run a renderer. The page and the toast word it; the
/// `Display` text is for logs and the socket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// No package has the id.
    Missing,
    /// The package is there and doesn't load, with the scan's reason.
    Broken(String),
    /// The package doesn't declare `renderer`.
    NotRenderer,
    /// The Plugins switch is off.
    PluginsOff,
    /// Never approved, or changed on disk since.
    NotApproved,
    /// Approved and switched off.
    Off,
    /// Five exits inside ten minutes; switching it off and on clears it.
    Stopped,
    /// A size outside [`WIDTHS`] by [`HEIGHTS`].
    Size,
    /// It was allowed to run and didn't start.
    Start(String),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::Missing => write!(f, "no renderer has that id"),
            Refused::Broken(why) => write!(f, "the package doesn't load: {why}"),
            Refused::NotRenderer => write!(f, "the package isn't a renderer"),
            Refused::PluginsOff => write!(f, "plugins are off"),
            Refused::NotApproved => write!(f, "the renderer isn't approved"),
            Refused::Off => write!(f, "the renderer is switched off"),
            Refused::Stopped => write!(f, "stopped after repeated exits"),
            Refused::Size => write!(
                f,
                "the size is outside {}..={} by {}..={}",
                WIDTHS.start(),
                WIDTHS.end(),
                HEIGHTS.start(),
                HEIGHTS.end()
            ),
            Refused::Start(why) => write!(f, "could not start: {why}"),
        }
    }
}

/// Every loaded package that declares `renderer`, by id.
pub fn all() -> Vec<Renderer> {
    let records = Settings::load().accounts.plugins;

    listed(
        &crate::plugins::loaded(),
        &records,
        &settings::plugin_approved,
    )
}

fn listed(
    folders: &[Loaded],
    records: &[PluginRecord],
    approved: &dyn Fn(&str, &str) -> bool,
) -> Vec<Renderer> {
    folders
        .iter()
        .filter_map(|folder| {
            let manifest = folder.manifest.as_ref()?;
            let cap = manifest.capabilities.renderer.as_ref()?;

            Some(Renderer {
                id: folder.id.clone(),
                name: manifest.name.clone(),
                label: cap.label.clone(),
                approved: approved(&folder.id, &folder.hash),
                enabled: records.iter().any(|r| r.id == folder.id && r.enabled),
            })
        })
        .collect()
}

/// The gate, read from what's on disk and in the records now. Every start
/// passes through it, and so does every running session when the plugins
/// apply.
fn check<'a>(
    id: &str,
    folders: &'a [Loaded],
    records: &[PluginRecord],
    approved: &dyn Fn(&str, &str) -> bool,
    allowed: bool,
) -> Result<&'a Loaded, Refused> {
    // The first row for an id is the one that holds it, as in the scan.
    let folder = folders
        .iter()
        .find(|folder| folder.id == id)
        .ok_or(Refused::Missing)?;

    let Some(manifest) = folder.manifest.as_ref().filter(|_| folder.runs()) else {
        return Err(Refused::Broken(folder.error.clone().unwrap_or_default()));
    };
    if !manifest.is_renderer() {
        return Err(Refused::NotRenderer);
    }

    if !allowed {
        return Err(Refused::PluginsOff);
    }
    if !approved(&folder.id, &folder.hash) {
        return Err(Refused::NotApproved);
    }
    if !records.iter().any(|r| r.id == id && r.enabled) {
        return Err(Refused::Off);
    }

    Ok(folder)
}

/// Why [`start`] would turn renderer `id` away now, read through the same
/// gate and exit count without starting anything. None when it would run.
pub fn refusal(id: &str) -> Option<Refused> {
    let folders = crate::plugins::loaded();
    let records = Settings::load().accounts.plugins;
    let gate = check(
        id,
        &folders,
        &records,
        &settings::plugin_approved,
        crate::plugins::allowed(),
    );

    match gate {
        Err(refused) => Some(refused),
        Ok(_) if due(id).is_none() => Some(Refused::Stopped),
        Ok(_) => None,
    }
}

/// Forgets renderer `id`'s exits, as switching it off and on does. A render
/// target's Start Again, which approves and switches on nothing.
pub fn forgive(id: &str) {
    forget_exits(&|kept| kept != id);
}

/// One run of a renderer. Dropping it stops the renderer.
pub struct Session {
    id: String,
    /// rox's lines to the renderer, `hello` already sent. Closing it is the
    /// signal to exit.
    pub stdin: Option<ChildStdin>,
    /// The renderer's announce lines.
    pub stdout: Option<ChildStdout>,
    run: Arc<Run>,
}

/// What the session and the live list share, so a switch-off or a quit
/// can end a session nobody is asking about.
struct Run {
    id: String,
    process: Mutex<Process>,
    /// rox ended it, so its exit isn't the renderer's doing.
    stopped: AtomicBool,
}

impl Run {
    /// The renderer and its process group, reaped. Safe to call twice.
    fn halt(&self) {
        self.stopped.store(true, Ordering::Release);

        if let Ok(mut process) = self.process.lock() {
            process.kill();
        }
    }
}

/// Every session that may still run, for [`enforce`] and [`stop_all`].
static LIVE: Mutex<Vec<Weak<Run>>> = Mutex::new(Vec::new());

/// Exit times per id inside [`CRASH_WINDOW`].
static EXITS: LazyLock<Mutex<HashMap<String, VecDeque<Instant>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Starts renderer `id` drawing at `width` by `height` into the mapping
/// behind `handle`, after the gate and the exit count agree. Blocking: it
/// may probe an interpreter, so never on the UI thread.
pub fn start(id: &str, handle: Lent<'_>, width: u32, height: u32) -> Result<Session, Refused> {
    if !WIDTHS.contains(&width) || !HEIGHTS.contains(&height) {
        return Err(Refused::Size);
    }

    let folders = crate::plugins::loaded();
    let records = Settings::load().accounts.plugins;
    let folder = check(
        id,
        &folders,
        &records,
        &settings::plugin_approved,
        crate::plugins::allowed(),
    )?;

    if due(id).is_none() {
        return Err(Refused::Stopped);
    }

    // A renderer that can't start counts as one that exits, as a plugin's
    // failed start does, so a broken one stops rather than looping.
    let started = launch(folder, handle, width, height);
    if started.is_err() {
        note_exit(id);
    }

    let (process, stdin, stdout) = started.map_err(Refused::Start)?;

    let run = Arc::new(Run {
        id: id.to_string(),
        process: Mutex::new(process),
        stopped: AtomicBool::new(false),
    });
    if let Ok(mut live) = LIVE.lock() {
        live.retain(|run| run.strong_count() > 0);
        live.push(Arc::downgrade(&run));
    }

    log::info!("renderer {id}: started at {width}x{height}");

    Ok(Session {
        id: id.to_string(),
        stdin: Some(stdin),
        stdout: Some(stdout),
        run,
    })
}

type Launched = (Process, ChildStdin, ChildStdout);

/// Spawn and `hello`. The command is the plugin entry's own: the same
/// interpreter lookup, the same folder checks.
fn launch(folder: &Loaded, handle: Lent, width: u32, height: u32) -> Result<Launched, String> {
    let Some(manifest) = folder.manifest.as_ref() else {
        return Err("no manifest".into());
    };

    let mut command = manifest::entry_for(manifest, &folder.dir)?;
    command
        .env("ROX_RENDER_API", API.to_string())
        .env("ROX_FRAMES_WIDTH", width.to_string())
        .env("ROX_FRAMES_HEIGHT", height.to_string());
    #[cfg(unix)]
    command.env("ROX_FRAMES_FD", "3");

    let lent = Extra::Handle(handle);
    let (mut process, pipes) = process::spawn_with(&folder.id, command, &folder.dir, lent)?;
    let mut stdin = pipes.stdin;

    let hello = json!({
        "hello": API,
        "width": width,
        "height": height,
        "handle": process.handle().unwrap_or(0),
    });
    let line = format!("{hello}\n");
    if let Err(e) = stdin.write_all(line.as_bytes()).and_then(|_| stdin.flush()) {
        process.kill();
        return Err(format!("hello: {e}"));
    }

    Ok((process, stdin, pipes.stdout))
}

impl Session {
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The pid while it runs, for a test or a person killing it by hand.
    pub fn pid(&self) -> Option<u32> {
        self.run.process.lock().ok().map(|process| process.pid())
    }

    /// Whether the process is gone. Asking reaps it, so a renderer that
    /// died leaves no zombie behind for long.
    pub fn exited(&self) -> bool {
        self.run
            .process
            .lock()
            .is_ok_and(|mut process| process.exited().is_some())
    }

    /// Whether rox ended it: [`stop`](Self::stop), a switch-off or a quit.
    /// An exit that isn't is the one [`note_exit`] counts.
    pub fn stopped(&self) -> bool {
        self.run.stopped.load(Ordering::Acquire)
    }

    /// Closes stdin, gives the renderer a moment to leave, then kills it and
    /// its process group. Safe to call twice.
    pub fn stop(&mut self) {
        let asked = self.stdin.take().is_some();

        if asked
            && !self.stopped()
            && let Ok(mut process) = self.run.process.lock()
        {
            process.wait_for(GRACE);
        }

        self.run.halt();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
    }
}

/// When renderer `id` may start again: now, after a wait, or None once it
/// exited five times inside ten minutes. The plugin host's schedule.
pub fn due(id: &str) -> Option<Duration> {
    let mut exits = EXITS.lock().unwrap_or_else(|e| e.into_inner());
    let times = exits.entry(id.to_string()).or_default();

    wait_after(times, Instant::now())
}

/// Counts an exit rox didn't ask for.
pub fn note_exit(id: &str) {
    let mut exits = EXITS.lock().unwrap_or_else(|e| e.into_inner());
    let times = exits.entry(id.to_string()).or_default();
    let now = Instant::now();

    times.push_back(now);
    times.retain(|at| now.duration_since(*at) < CRASH_WINDOW);

    if times.len() > BACKOFF.len() {
        log::warn!(
            "renderer {id}: {} exits in ten minutes, stopping it",
            times.len()
        );
    }
}

/// Drops the exit count of every renderer `passes` turns away.
fn forget_exits(passes: &dyn Fn(&str) -> bool) {
    if let Ok(mut exits) = EXITS.lock() {
        exits.retain(|id, _| passes(id));
    }
}

fn wait_after(times: &mut VecDeque<Instant>, now: Instant) -> Option<Duration> {
    times.retain(|at| now.duration_since(*at) < CRASH_WINDOW);

    match times.len() {
        0 => Some(Duration::ZERO),
        n if n > BACKOFF.len() => None,
        n => Some(BACKOFF[n - 1]),
    }
}

/// Ends every session the gate no longer passes, and forgets the exits of
/// every renderer it doesn't, so switching one off and on starts it fresh.
/// The plugins' apply runs it, after anything that moves the gate.
pub(crate) fn enforce(cx: &mut App) {
    let folders = crate::plugins::loaded();
    let records = Settings::load().accounts.plugins;
    let allowed = crate::plugins::allowed();
    let passes =
        |id: &str| check(id, &folders, &records, &settings::plugin_approved, allowed).is_ok();

    forget_exits(&passes);

    let ending: Vec<Arc<Run>> = match LIVE.lock() {
        Ok(mut live) => {
            live.retain(|run| run.strong_count() > 0);
            live.iter()
                .filter_map(Weak::upgrade)
                .filter(|run| !passes(&run.id))
                .collect()
        }
        Err(_) => Vec::new(),
    };

    if ending.is_empty() {
        return;
    }

    cx.background_executor()
        .spawn(async move {
            for run in ending {
                log::info!("renderer {}: stopped, no longer switched on", run.id);
                run.halt();
            }
        })
        .detach();
}

/// Every session, for rox quitting.
pub(crate) fn stop_all() {
    let running: Vec<Arc<Run>> = LIVE
        .lock()
        .map(|live| live.iter().filter_map(Weak::upgrade).collect())
        .unwrap_or_default();

    for run in running {
        run.halt();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use rox_plugins::Origin;

    fn package(id: &str, capabilities: serde_json::Value) -> Loaded {
        let document = json!({
            "id": id,
            "name": id.to_uppercase(),
            "version": "1",
            "api": 1,
            "entry": { "script": { "path": "draw.sh", "interpreter": "sh" } },
            "capabilities": capabilities,
        });
        let manifest = manifest::parse(&document.to_string()).unwrap();

        Loaded {
            id: id.into(),
            dir: std::path::PathBuf::from(id),
            origin: Origin::Folder,
            manifest: Some(manifest),
            document,
            hash: format!("{id}-hash"),
            programs: Vec::new(),
            error: None,
            icon: None,
            action_icons: Vec::new(),
        }
    }

    fn renderer(id: &str) -> Loaded {
        package(id, json!({ "renderer": { "label": "Bars" } }))
    }

    fn record(id: &str, enabled: bool) -> PluginRecord {
        PluginRecord {
            id: id.into(),
            enabled,
            hash: format!("{id}-hash"),
            ..Default::default()
        }
    }

    fn all_approved(_: &str, _: &str) -> bool {
        true
    }

    fn none_approved(_: &str, _: &str) -> bool {
        false
    }

    #[test]
    fn only_renderer_packages_are_listed() {
        let folders = [
            renderer("bars"),
            package("tones", json!({ "source": { "label": "Tones" } })),
            package("words", json!({ "providers": { "lyrics": true } })),
        ];
        let records = [record("bars", true), record("tones", true)];

        assert_eq!(
            listed(&folders, &records, &all_approved),
            [Renderer {
                id: "bars".into(),
                name: "BARS".into(),
                label: "Bars".into(),
                approved: true,
                enabled: true,
            }]
        );

        let unapproved = listed(&folders, &[], &none_approved);
        assert!(!unapproved[0].approved && !unapproved[0].enabled);
    }

    #[test]
    fn a_renderer_switched_off_wont_start() {
        let folders = [
            renderer("bars"),
            package("tones", json!({ "source": { "label": "Tones" } })),
        ];
        let on = [record("bars", true), record("tones", true)];
        let off = [record("bars", false)];

        let gate =
            |id, records: &[PluginRecord], approved: &dyn Fn(&str, &str) -> bool, allowed| {
                check(id, &folders, records, approved, allowed).map(|folder| folder.id.clone())
            };

        assert_eq!(gate("bars", &on, &all_approved, true), Ok("bars".into()));
        assert_eq!(gate("bars", &off, &all_approved, true), Err(Refused::Off));
        assert_eq!(gate("bars", &[], &all_approved, true), Err(Refused::Off));
        assert_eq!(
            gate("bars", &on, &none_approved, true),
            Err(Refused::NotApproved),
            "switched on but not approved for this folder is a changed one"
        );
        assert_eq!(
            gate("bars", &on, &all_approved, false),
            Err(Refused::PluginsOff)
        );

        assert_eq!(
            gate("tones", &on, &all_approved, true),
            Err(Refused::NotRenderer),
            "a plugin switched on and approved still isn't a renderer"
        );
        assert_eq!(
            gate("gone", &on, &all_approved, true),
            Err(Refused::Missing)
        );

        // Approval is for one exact folder.
        let other_hash = |_: &str, hash: &str| hash == "an-older-hash";
        assert_eq!(
            gate("bars", &on, &other_hash, true),
            Err(Refused::NotApproved)
        );

        let mut broken = renderer("bars");
        broken.error = Some("entry draw.sh is missing".into());
        assert_eq!(
            check("bars", &[broken], &on, &all_approved, true).map(|_| ()),
            Err(Refused::Broken("entry draw.sh is missing".into()))
        );
    }

    #[test]
    fn the_fifth_exit_stops_it() {
        let mut times = VecDeque::new();
        let now = Instant::now();
        assert_eq!(wait_after(&mut times, now), Some(Duration::ZERO));

        let mut waits = Vec::new();
        for _ in 0..4 {
            times.push_back(now);
            waits.push(wait_after(&mut times, now));
        }
        assert_eq!(waits, BACKOFF.map(Some), "the host's schedule");

        times.push_back(now);
        assert_eq!(wait_after(&mut times, now), None, "the fifth exit");

        let later = now + CRASH_WINDOW;
        assert_eq!(
            wait_after(&mut times, later),
            Some(Duration::ZERO),
            "exits past ten minutes don't count"
        );

        // Through the shared count, under an id no other test uses.
        for _ in 0..5 {
            note_exit("renderers-test-fifth");
        }
        assert_eq!(due("renderers-test-fifth"), None);
    }

    #[test]
    fn switching_it_on_again_clears_the_count() {
        let id = "renderers-test-again";
        for _ in 0..5 {
            note_exit(id);
        }
        assert_eq!(due(id), None);

        // Switched on, enforce keeps the count.
        let folders = [renderer(id)];
        let on = [record(id, true)];
        let off = [record(id, false)];
        let passes = |kept: &str, records: &[PluginRecord]| {
            kept != id || check(kept, &folders, records, &all_approved, true).is_ok()
        };

        forget_exits(&|kept| passes(kept, &on));
        assert_eq!(due(id), None, "still on, still stopped");

        // Switched off, the gate turns it away and its count goes, so the
        // next switch on starts it fresh. Other tests' ids pass untouched.
        forget_exits(&|kept| passes(kept, &off));
        assert_eq!(due(id), Some(Duration::ZERO));
    }

    #[test]
    fn start_again_forgets_only_its_own_exits() {
        let (mine, other) = ("renderers-test-mine", "renderers-test-other");
        for _ in 0..5 {
            note_exit(mine);
            note_exit(other);
        }

        forgive(mine);
        assert_eq!(due(mine), Some(Duration::ZERO));
        assert_eq!(due(other), None, "another renderer's count stands");
    }
}
