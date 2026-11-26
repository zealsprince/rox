//! Renderers (ADR 37): packages whose manifest declares `renderer`, run one
//! process per render target and handed the frame mapping that target made.
//! Underneath a renderer is ADR 30's package. It installs, hashes, approves
//! and switches on through [`crate::plugins`], and nothing here writes an
//! approval or a switch.
//!
//! A renderer is never a plugin host. It gets no `Host` and no plugin wire,
//! answers no requests, and is started only when a render target asks, never
//! with the plugins at launch. What it gets is its pipes, the plugin
//! environment with `ROX_RENDER_API`, the frame size and a variable for each
//! handle lent, the `hello` line and one lent handle (fd 3 on Unix), then the
//! cover's when its approved manifest asks for one, then a GPU texture's when
//! it asks for that and the render target could make one, then the
//! spectrum's when it asks for the music's spectrum. Restarting is the render
//! target's job, on the plugin host's schedule through [`due`] and
//! [`note_exit`].
//!
//! A renderer runs in the sandbox unless its manifest asks to run unconfined
//! (ADR 38). What it runs as is its [`Tier`], and the gate holds it to the
//! tier its approval recorded: one approved as sandboxed never runs when
//! the sandbox can't be applied, and stays off until the user says yes to
//! what this system can give it.

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
use rox_plugins::manifest::{RendererCap, SignalKind, WantedSignal};
use rox_plugins::process::{self, Extra, Lent, Process};
use rox_plugins::sandbox::{self, Support, Tier};
use rox_plugins::{Loaded, manifest};
use rox_viz::signal::{Signal, Source};

/// What `ROX_RENDER_API` and the `hello` line say.
pub const API: u32 = 1;

/// The sizes a render target can ask for.
pub const WIDTHS: std::ops::RangeInclusive<u32> = 16..=3840;
pub const HEIGHTS: std::ops::RangeInclusive<u32> = 16..=2160;

/// How long [`Session::stop`] waits for a renderer to leave once its stdin
/// closes.
const GRACE: Duration = Duration::from_millis(200);

/// A renderer as the Renderers page lists it.
#[derive(Clone, Debug, PartialEq)]
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
    /// The signals its manifest asks for.
    pub signals: Vec<WantedSignal>,
    /// Its manifest suggests pausing while the music is paused or stopped.
    /// Only a new render target's starting value; the user's setting wins.
    pub rests: bool,
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
    /// Never approved, changed on disk since, or approved to run as another
    /// tier than its manifest and this system give it now.
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
                signals: cap.signals.clone(),
                rests: cap.rests,
            })
        })
        .collect()
}

/// What a renderer with these asks runs as on this system now.
pub fn tier(cap: &RendererCap) -> Tier {
    tier_in(cap, &sandbox::support())
}

fn tier_in(cap: &RendererCap, support: &Support) -> Tier {
    Tier::of(cap.unconfined, &cap.access, support)
}

/// Whether `record`'s approval covers the tier `cap` runs as now. An
/// approval from before the sandbox recorded none: it was given for a
/// renderer running with the user's permissions, and whatever this system
/// gives it now is at most that.
fn sandbox_holds(record: &PluginRecord, cap: &RendererCap, support: &Support) -> bool {
    record.sandbox.is_empty() || record.sandbox == tier_in(cap, support).key()
}

/// [`sandbox_holds`] for the settings page and the switch, which raise the
/// card when it doesn't. True for anything that isn't a renderer.
pub fn sandbox_approved(folder: &Loaded, record: Option<&PluginRecord>) -> bool {
    let Some(cap) = folder
        .manifest
        .as_ref()
        .and_then(|m| m.capabilities.renderer.as_ref())
    else {
        return true;
    };

    record.is_none_or(|record| sandbox_holds(record, cap, &sandbox::support()))
}

/// The gate, read from what's on disk and in the records now. Every start
/// passes through it, and so does every running session when the plugins
/// apply.
///
/// The Plugins switch isn't part of it (ADR 37, amended 2026-11-26): a
/// renderer runs on its own switch and its approval of these exact files,
/// and a package that declares `renderer` can't declare anything that would
/// give it a plugin host, so nothing here can start a plugin.
fn check<'a>(
    id: &str,
    folders: &'a [Loaded],
    records: &[PluginRecord],
    approved: &dyn Fn(&str, &str) -> bool,
    support: &Support,
) -> Result<&'a Loaded, Refused> {
    // The first row for an id is the one that holds it, as in the scan.
    let folder = folders
        .iter()
        .find(|folder| folder.id == id)
        .ok_or(Refused::Missing)?;

    let Some(manifest) = folder.manifest.as_ref().filter(|_| folder.runs()) else {
        return Err(Refused::Broken(folder.error.clone().unwrap_or_default()));
    };
    let Some(cap) = manifest.capabilities.renderer.as_ref() else {
        return Err(Refused::NotRenderer);
    };

    if !approved(&folder.id, &folder.hash) {
        return Err(Refused::NotApproved);
    }
    let Some(record) = records.iter().find(|r| r.id == id && r.enabled) else {
        return Err(Refused::Off);
    };

    // Approved as sandboxed and the sandbox can't be applied here now, or
    // the other way round: the user's yes was for another tier.
    if !sandbox_holds(record, cap, support) {
        return Err(Refused::NotApproved);
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
        &sandbox::support(),
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

/// What renderer `id`'s manifest asks for, read through the gate as
/// [`start`] reads it. None when the gate would turn it away.
pub fn asks(id: &str) -> Option<RendererCap> {
    let folders = crate::plugins::loaded();
    let records = Settings::load().accounts.plugins;
    let folder = check(
        id,
        &folders,
        &records,
        &settings::plugin_approved,
        &sandbox::support(),
    )
    .ok()?;

    folder.manifest.as_ref()?.capabilities.renderer.clone()
}

/// One run of a renderer. Dropping it stops the renderer.
pub struct Session {
    id: String,
    /// What the approved manifest asks for, as this session got it: the
    /// cover only when it was lent one.
    asks: RendererCap,
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
/// behind `handle`, after the gate and the exit count agree. `cover` is
/// lent only to a renderer whose approved manifest asks for the cover,
/// `spectrum` only to one that asks for the spectrum, and `texture` only to
/// one whose approved manifest says `gpu`. Blocking: it may probe an
/// interpreter, so never on the UI thread.
pub fn start(
    id: &str,
    handle: Lent<'_>,
    cover: Option<Lent<'_>>,
    spectrum: Option<Lent<'_>>,
    texture: Option<&gpui::ExternalTexture>,
    width: u32,
    height: u32,
) -> Result<Session, Refused> {
    if !WIDTHS.contains(&width) || !HEIGHTS.contains(&height) {
        return Err(Refused::Size);
    }

    let folders = crate::plugins::loaded();
    let records = Settings::load().accounts.plugins;
    let support = sandbox::support();
    let folder = check(id, &folders, &records, &settings::plugin_approved, &support)?;

    if due(id).is_none() {
        return Err(Refused::Stopped);
    }

    // The grants are the manifest's the gate just passed, so a session never
    // gets what the approved files don't ask for.
    let mut asks = folder
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.capabilities.renderer.clone())
        .unwrap_or_default();
    let cover = cover.filter(|_| asks.cover);
    asks.cover = cover.is_some();
    let spectrum = spectrum.filter(|_| asks.spectrum);
    asks.spectrum = spectrum.is_some();
    let texture = texture.filter(|_| asks.gpu);
    asks.gpu = texture.is_some();

    // A renderer that can't start counts as one that exits, as a plugin's
    // failed start does, so a broken one stops rather than looping.
    let tier = tier_in(&asks, &support);
    let started = launch(
        folder, &tier, handle, cover, spectrum, texture, width, height,
    );
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

    let lent = match asks.gpu {
        true => ", lent a texture",
        false => "",
    };
    log::info!(
        "renderer {id}: started at {width}x{height}{lent}, {}",
        tier.key()
    );

    Ok(Session {
        id: id.to_string(),
        asks,
        stdin: Some(stdin),
        stdout: Some(stdout),
        run,
    })
}

type Launched = (Process, ChildStdin, ChildStdout);

/// Spawn and `hello`. The command is the plugin entry's own: the same
/// interpreter lookup, the same folder checks.
#[allow(clippy::too_many_arguments)]
fn launch(
    folder: &Loaded,
    tier: &Tier,
    handle: Lent,
    cover: Option<Lent>,
    spectrum: Option<Lent>,
    texture: Option<&gpui::ExternalTexture>,
    width: u32,
    height: u32,
) -> Result<Launched, String> {
    let Some(manifest) = folder.manifest.as_ref() else {
        return Err("no manifest".into());
    };

    let mut command = manifest::entry_for(manifest, &folder.dir)?;

    // Every renderer gets the folder, sandboxed or not, so one written for
    // the sandbox runs the same outside it.
    let cache = settings::renderer_cache_dir(&folder.id);
    std::fs::create_dir_all(&cache).map_err(|e| format!("cache folder: {e}"))?;
    command
        .env("ROX_CACHE_DIR", &cache)
        .env("ROX_RENDER_API", API.to_string())
        .env("ROX_FRAMES_WIDTH", width.to_string())
        .env("ROX_FRAMES_HEIGHT", height.to_string());

    // The frames first, always: they're the fallback a renderer that can't
    // import the texture draws into.
    let texture_fd = texture.and_then(lent_texture);
    let fds = places(cover.is_some(), texture_fd.is_some(), spectrum.is_some());
    let mut lent = vec![handle];
    lent.extend(cover);
    lent.extend(texture_fd);
    lent.extend(spectrum);

    #[cfg(unix)]
    {
        command.env("ROX_FRAMES_FD", fds.frames.to_string());
        if let Some(fd) = fds.cover {
            command.env("ROX_COVER_FD", fd.to_string());
        }
        if let Some(fd) = fds.texture {
            command.env("ROX_TEXTURE_FD", fd.to_string());
        }
        if let Some(fd) = fds.spectrum {
            command.env("ROX_SPECTRUM_FD", fd.to_string());
        }
    }

    let (mut process, pipes) = spawn(folder, tier, &cache, command, Extra::Handles(lent))?;
    let mut stdin = pipes.stdin;

    let mut hello = json!({
        "hello": API,
        "width": width,
        "height": height,
        "handle": process.handle().unwrap_or(0),
    });
    if let Some(fd) = fds.cover {
        hello["cover"] = cover_hello(&process, fd);
    }
    if let (Some(texture), Some(fd)) = (texture, fds.texture) {
        hello["texture"] = texture_hello(texture, fd);
    }
    if let Some(fd) = fds.spectrum {
        hello["spectrum"] = spectrum_hello(&process, fd);
    }
    let line = format!("{hello}\n");
    if let Err(e) = stdin.write_all(line.as_bytes()).and_then(|_| stdin.flush()) {
        process.kill();
        return Err(format!("hello: {e}"));
    }

    Ok((process, stdin, pipes.stdout))
}

/// Starts the renderer as `tier` says: confined by its profile, or as a
/// program with the user's permissions when its approval says so. A
/// sandboxed tier is never started any other way.
#[cfg(target_os = "linux")]
fn spawn(
    folder: &Loaded,
    tier: &Tier,
    cache: &std::path::Path,
    command: std::process::Command,
    extra: Extra,
) -> Result<(Process, process::Pipes), String> {
    let Tier::Sandboxed {
        network, folders, ..
    } = tier
    else {
        return process::spawn_with("renderer", &folder.id, command, &folder.dir, extra);
    };

    let Support::Sandbox { abi, .. } = sandbox::support() else {
        return Err("the sandbox can't be applied here".into());
    };

    let reads = read_folders(folders);
    let program = std::path::PathBuf::from(command.get_program());
    let profile = sandbox::Profile::renderer(&folder.dir, &program, cache, &reads, *network);

    process::spawn_confined(
        "renderer",
        &folder.id,
        command,
        &folder.dir,
        extra,
        &profile,
        abi,
    )
}

/// No sandbox off Linux: [`sandbox::support`] answers Platform there, so
/// every tier but Unconfined is Permissions and runs as today.
#[cfg(not(target_os = "linux"))]
fn spawn(
    folder: &Loaded,
    tier: &Tier,
    _cache: &std::path::Path,
    command: std::process::Command,
    extra: Extra,
) -> Result<(Process, process::Pipes), String> {
    if tier.sandboxed() {
        return Err("the sandbox can't be applied here".into());
    }

    process::spawn_with("renderer", &folder.id, command, &folder.dir, extra)
}

/// The folders a renderer's read asks name, as this system has them when
/// it starts: the library's folders from the settings, the rest from the
/// user's XDG folders. A running renderer keeps what it started with; a
/// folder added to the library reaches it at its next start, and one
/// removed stays readable until then, since Landlock adds rules and never
/// takes one back.
#[cfg(target_os = "linux")]
fn read_folders(folders: &[sandbox::Folder]) -> Vec<std::path::PathBuf> {
    use sandbox::Folder;

    let home = dirs::home_dir().and_then(|home| home.canonicalize().ok());
    let data = settings::data_dir().canonicalize().ok();

    let mut reads = Vec::new();
    for folder in folders {
        let named: Vec<std::path::PathBuf> = match folder {
            Folder::Library => Settings::load().library_roots,
            Folder::Music => dirs::audio_dir().into_iter().collect(),
            Folder::Pictures => dirs::picture_dir().into_iter().collect(),
            Folder::Videos => dirs::video_dir().into_iter().collect(),
            Folder::Documents => dirs::document_dir().into_iter().collect(),
            Folder::Downloads => dirs::download_dir().into_iter().collect(),
        };

        for path in named {
            match readable(&path, home.as_deref(), data.as_deref()) {
                Some(path) => reads.push(path),
                None => log::warn!(
                    "renderer: not granting {} for {}: it holds the home or rox's data folder",
                    path.display(),
                    folder.key()
                ),
            }
        }
    }

    reads
}

/// `path` as a read grant, or None when granting it would grant more than
/// the folder the card named. A user folder that xdg-user-dirs set to home
/// itself, or a library folder at home or above it, would hand over every
/// file the user has, and one around or inside rox's data folder its
/// accounts and approvals.
#[cfg(target_os = "linux")]
fn readable(
    path: &std::path::Path,
    home: Option<&std::path::Path>,
    data: Option<&std::path::Path>,
) -> Option<std::path::PathBuf> {
    let path = path.canonicalize().ok()?;

    let holds_home = home.is_some_and(|home| home.starts_with(&path));
    let near_data = data.is_some_and(|data| data.starts_with(&path) || path.starts_with(data));
    if holds_home || near_data {
        return None;
    }

    Some(path)
}

/// Where each lent handle lands in a renderer on Unix. The order is fixed:
/// the frames always, then the cover when it's lent, then the texture when
/// it's lent, then the spectrum when it's lent. Each is named in the
/// environment and in `hello`, so a renderer never works one out by
/// position. On Windows the lending order is the same and `hello` carries
/// the values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Places {
    // Read where the environment names it, which Windows has no use for.
    #[cfg_attr(windows, allow(dead_code))]
    frames: u32,
    cover: Option<u32>,
    texture: Option<u32>,
    spectrum: Option<u32>,
}

/// The fd the first lent handle arrives as on Unix.
const FIRST_FD: u32 = 3;

fn places(cover: bool, texture: bool, spectrum: bool) -> Places {
    let cover = cover.then_some(FIRST_FD + 1);
    let texture = texture.then_some(FIRST_FD + 1 + u32::from(cover.is_some()));
    let spectrum = spectrum
        .then_some(FIRST_FD + 1 + u32::from(cover.is_some()) + u32::from(texture.is_some()));

    Places {
        frames: FIRST_FD,
        cover,
        texture,
        spectrum,
    }
}

/// The cover's side in pixels: `rox_frames::cover::SIZE`, which this crate
/// doesn't link. A test in the render target pins the two equal.
pub const COVER_SIDE: u32 = 256;

/// Where the cover is and its size, for `hello`.
#[cfg(unix)]
fn cover_hello(_process: &Process, fd: u32) -> serde_json::Value {
    json!({ "fd": fd, "width": COVER_SIDE, "height": COVER_SIDE })
}

/// Never compiled: no Windows target builds here. The cover's handle is
/// the second one lent.
#[cfg(windows)]
fn cover_hello(process: &Process, _fd: u32) -> serde_json::Value {
    let handle = process.handles().get(1).copied().unwrap_or(0);
    json!({ "handle": handle, "width": COVER_SIDE, "height": COVER_SIDE })
}

/// The spectrum mapping's size and rows, for `hello`: the constants of
/// `rox_frames::spectrum`, which this crate doesn't link. A test in the
/// render target pins them equal.
pub const SPECTRUM_BYTES: u32 = 4096;
pub const SPECTRUM_LOG_BINS: u32 = 128;
pub const SPECTRUM_LINEAR_BINS: u32 = 512;
pub const SPECTRUM_RATE: f32 = 62.5;

/// Where the spectrum is and what it holds, for `hello`.
#[cfg(unix)]
fn spectrum_hello(_process: &Process, fd: u32) -> serde_json::Value {
    json!({
        "fd": fd,
        "bytes": SPECTRUM_BYTES,
        "log_bins": SPECTRUM_LOG_BINS,
        "linear_bins": SPECTRUM_LINEAR_BINS,
        "rate": SPECTRUM_RATE,
    })
}

/// Never compiled: no Windows target builds here. The handles are lent in
/// the order [`places`] gives, so the spectrum's is at its place past the
/// first fd.
#[cfg(windows)]
fn spectrum_hello(process: &Process, fd: u32) -> serde_json::Value {
    let at = (fd - FIRST_FD) as usize;
    let handle = process.handles().get(at).copied().unwrap_or(0);
    json!({
        "handle": handle,
        "bytes": SPECTRUM_BYTES,
        "log_bins": SPECTRUM_LOG_BINS,
        "linear_bins": SPECTRUM_LINEAR_BINS,
        "rate": SPECTRUM_RATE,
    })
}

/// The texture's memory, borrowed for the spawn.
#[cfg(unix)]
fn lent_texture(texture: &gpui::ExternalTexture) -> Option<Lent<'_>> {
    use std::os::fd::AsFd;

    Some(texture.fd.as_fd())
}

/// gpui makes no external texture off Linux, so there's never one to lend.
#[cfg(windows)]
fn lent_texture(_texture: &gpui::ExternalTexture) -> Option<Lent<'_>> {
    None
}

/// What a renderer needs to import the texture, for `hello`: the image's
/// create info as raw Vulkan values, the allocation it repeats, and the
/// device it has to be on. rox describes its own allocation here and reads
/// nothing back about it.
fn texture_hello(texture: &gpui::ExternalTexture, fd: u32) -> serde_json::Value {
    json!({
        "fd": fd,
        "width": texture.width,
        "height": texture.height,
        "allocation_size": texture.allocation_size,
        "memory_type": texture.memory_type,
        "handle_type": "opaque-fd",
        "format": texture.format,
        "view_formats": texture.view_formats,
        "flags": texture.flags,
        "usage": texture.usage,
        "tiling": texture.tiling,
        "layout": texture.layout,
        "device_uuid": hex(&texture.device_uuid),
        "driver_uuid": hex(&texture.driver_uuid),
    })
}

fn hex(bytes: &[u8; 16]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

impl Session {
    pub fn id(&self) -> &str {
        &self.id
    }

    /// What this session was granted.
    pub fn asks(&self) -> &RendererCap {
        &self.asks
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
    let support = sandbox::support();
    let passes =
        |id: &str| check(id, &folders, &records, &settings::plugin_approved, &support).is_ok();

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

/// Every session, for rox quitting and for the re-exec after a lost GPU,
/// which would otherwise leave them running outside it.
pub fn stop_all() {
    let running: Vec<Arc<Run>> = LIVE
        .lock()
        .map(|live| live.iter().filter_map(Weak::upgrade).collect())
        .unwrap_or_default();

    for run in running {
        run.halt();
    }
}

// ── The signals a renderer asks for ──
//
// Pure: the canvas offers what [`signals_to_add`] answers and adds it only
// on the user's yes, the way an object's signals are merged.

/// The pool signal a wanted one would read, worded as the pool words it.
/// None for a kind this rox doesn't know, and for an aggregate, whose
/// input is another entry.
fn wanted_source(wanted: &WantedSignal) -> Option<Source> {
    let band = || Some((wanted.low?, wanted.high?));

    match wanted.kind()? {
        SignalKind::Band => band().map(|(lo, hi)| Source::Band { lo, hi }),
        SignalKind::Onset => band().map(|(lo, hi)| Source::Onset { lo, hi }),
        SignalKind::Beat => band().map(|(lo, hi)| Source::Beat { lo, hi }),
        SignalKind::Trigger => band().map(|(lo, hi)| Source::Trigger { lo, hi }),
        SignalKind::Level => Some(Source::Level),
        SignalKind::Aggregate => None,
    }
}

/// A source's kind and its band in whole hertz, as the hub hears it, so a
/// wanted 30 to 120 matches a pool band saved as 30.2 to 119.8.
fn heard(source: &Source) -> (std::mem::Discriminant<Source>, Option<(u32, u32)>) {
    let band = source
        .band()
        .map(|(lo, hi)| (lo.round() as u32, hi.round() as u32));

    (std::mem::discriminant(source), band)
}

/// Which pool signal answers each wanted one, by kind and band and never
/// by name. An aggregate is answered by a total of whatever answers its
/// input.
fn answered(wanted: &[WantedSignal], pool: &[Signal]) -> Vec<Option<u64>> {
    let mut found: Vec<Option<u64>> = wanted
        .iter()
        .map(|want| {
            let source = wanted_source(want)?;
            pool.iter()
                .find(|signal| heard(&signal.source) == heard(&source))
                .map(|signal| signal.id)
        })
        .collect();

    for (at, want) in wanted.iter().enumerate() {
        if want.kind() != Some(SignalKind::Aggregate) {
            continue;
        }

        let input = want.of.and_then(|of| found.get(of).copied().flatten());
        found[at] = input.and_then(|input| {
            pool.iter()
                .find(|signal| matches!(signal.source, Source::Aggregate { of, .. } if of == input))
                .map(|signal| signal.id)
        });
    }

    found
}

/// Whether rox could add this entry: a kind it knows, and for an aggregate
/// an input it knows too.
fn addable(wanted: &[WantedSignal], at: usize) -> bool {
    let Some(want) = wanted.get(at) else {
        return false;
    };

    match want.kind() {
        Some(SignalKind::Aggregate) => want
            .of
            .filter(|&of| of != at)
            .and_then(|of| wanted.get(of))
            .is_some_and(|input| wanted_source(input).is_some()),
        Some(_) => wanted_source(want).is_some(),
        None => false,
    }
}

/// The wanted signals the pool has nothing for, by index in the list.
pub fn missing_signals(wanted: &[WantedSignal], pool: &[Signal]) -> Vec<usize> {
    answered(wanted, pool)
        .into_iter()
        .enumerate()
        .filter(|&(at, found)| found.is_none() && addable(wanted, at))
        .map(|(at, _)| at)
        .collect()
}

/// The signals that fill those gaps, in list order, with the renderer's
/// names and suggested Response and Threshold. Their ids are numbered past
/// every pool id, and an aggregate over another added one names that one's
/// id, so the pool's merge renumbers them as it does an object's.
pub fn signals_to_add(wanted: &[WantedSignal], pool: &[Signal]) -> Vec<Signal> {
    let found = answered(wanted, pool);
    let missing = missing_signals(wanted, pool);
    let base = pool.iter().map(|signal| signal.id).max().unwrap_or(0) + 1;
    let local = |at: usize| base + at as u64;

    missing
        .iter()
        .filter_map(|&at| {
            let want = &wanted[at];
            let source = match want.kind()? {
                SignalKind::Aggregate => {
                    let of = want.of?;
                    Source::Aggregate {
                        of: found[of].unwrap_or(local(of)),
                        rate: want.rate.unwrap_or(1.0),
                    }
                }
                _ => wanted_source(want)?,
            };

            let defaults = Signal::default();
            // A trigger at threshold 0 never fires, so the Signals window
            // seeds one the same way.
            let threshold = match (want.threshold, source) {
                (Some(n), _) => f32::from(n) / 100.,
                (None, Source::Trigger { .. }) => TRIGGER_SEED,
                (None, _) => defaults.threshold,
            };

            Some(Signal {
                id: local(at),
                name: want.given_name().unwrap_or_default().to_string(),
                source,
                smooth: want
                    .response
                    .map_or(defaults.smooth, |n| f32::from(n) / 100.),
                threshold,
                ..defaults
            })
        })
        .collect()
}

/// The Signals window's threshold for a new trigger.
const TRIGGER_SEED: f32 = 0.5;

#[cfg(test)]
mod tests {
    use super::*;

    /// `NAME = value` from the stand-in host, which keeps its own copy of
    /// these numbers.
    fn stand_in(name: &str) -> u64 {
        let source = include_str!("../../../examples/renderers/standin.py");
        source
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{name} = ")))
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(|| panic!("standin.py has no {name}"))
    }

    #[test]
    fn the_stand_in_starts_a_renderer_as_rox_does() {
        assert_eq!(stand_in("API"), u64::from(API));
        assert_eq!(stand_in("FIRST_FD"), u64::from(FIRST_FD));
        assert_eq!(stand_in("GRACE_MS"), GRACE.as_millis() as u64);
    }

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

    use rox_plugins::sandbox::{Gaps, Unavailable};

    /// A system that sandboxes with every rule.
    fn full() -> Support {
        Support::Sandbox {
            abi: 7,
            gaps: Gaps::default(),
        }
    }

    #[test]
    fn an_approval_holds_for_the_tier_it_was_given() {
        let folders = [renderer("bars")];
        let approved_as = |sandbox: &str| PluginRecord {
            sandbox: sandbox.into(),
            ..record("bars", true)
        };
        let gate = |record: PluginRecord, support: &Support| {
            check("bars", &folders, &[record], &all_approved, support).map(|_| ())
        };
        let lost = Support::Unavailable(Unavailable::NoLandlock);
        let failed = Support::Unavailable(Unavailable::Failed("EPERM".into()));
        let older = Support::Sandbox {
            abi: 3,
            gaps: Gaps::of_abi(3),
        };

        assert_eq!(gate(approved_as("sandboxed"), &full()), Ok(()));

        // Approved as sandboxed, and the sandbox is gone or failed: it
        // stays off rather than run with the user's permissions.
        assert_eq!(
            gate(approved_as("sandboxed"), &lost),
            Err(Refused::NotApproved)
        );
        assert_eq!(
            gate(approved_as("sandboxed"), &failed),
            Err(Refused::NotApproved)
        );

        // What the kernel can stop moved, either way: a fresh yes.
        assert_eq!(
            gate(approved_as("sandboxed"), &older),
            Err(Refused::NotApproved)
        );
        assert_eq!(
            gate(approved_as("permissions"), &full()),
            Err(Refused::NotApproved)
        );
        assert_eq!(gate(approved_as("permissions"), &lost), Ok(()));

        // An approval from before the sandbox runs as whatever it gets now.
        for support in [full(), lost, older] {
            assert_eq!(gate(approved_as(""), &support), Ok(()));
        }
    }

    #[test]
    fn a_manifest_that_asks_more_is_another_tier() {
        let asking = package(
            "bars",
            json!({ "renderer": { "label": "Bars", "access": { "network": true } } }),
        );
        let loose = package(
            "bars",
            json!({ "renderer": { "label": "Bars", "unconfined": true } }),
        );
        let sandboxed = PluginRecord {
            sandbox: "sandboxed".into(),
            ..record("bars", true)
        };

        for folder in [asking, loose] {
            assert_eq!(
                check(
                    "bars",
                    &[folder],
                    std::slice::from_ref(&sandboxed),
                    &all_approved,
                    &full()
                )
                .map(|_| ()),
                Err(Refused::NotApproved)
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_read_grant_never_holds_home_or_the_data_folder() {
        let root = std::env::temp_dir().join(format!("rox-renderers-read-{}", std::process::id()));
        let home = root.join("home");
        let data = home.join(".local/share/rox");
        let music = home.join("Music");
        for dir in [&data, &music] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let (home, data, music) = (
            home.canonicalize().unwrap(),
            data.canonicalize().unwrap(),
            music.canonicalize().unwrap(),
        );

        let grant = |path: &std::path::Path| readable(path, Some(&home), Some(&data));
        assert_eq!(grant(&music), Some(music.clone()));
        assert_eq!(grant(&home), None, "home itself, as xdg-user-dirs can set");
        assert_eq!(grant(&root), None, "above home");
        assert_eq!(grant(std::path::Path::new("/")), None);
        assert_eq!(grant(&data.join("..")), None, "around the data folder");
        assert_eq!(grant(&data), None);
        assert_eq!(grant(&root.join("missing")), None, "not there");

        let _ = std::fs::remove_dir_all(&root);
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
                signals: Vec::new(),
                rests: false,
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

        let gate = |id, records: &[PluginRecord], approved: &dyn Fn(&str, &str) -> bool| {
            check(id, &folders, records, approved, &full()).map(|folder| folder.id.clone())
        };

        assert_eq!(gate("bars", &on, &all_approved), Ok("bars".into()));
        assert_eq!(gate("bars", &off, &all_approved), Err(Refused::Off));
        assert_eq!(gate("bars", &[], &all_approved), Err(Refused::Off));
        assert_eq!(
            gate("bars", &on, &none_approved),
            Err(Refused::NotApproved),
            "switched on but not approved for this folder is a changed one"
        );

        assert_eq!(
            gate("tones", &on, &all_approved),
            Err(Refused::NotRenderer),
            "a plugin switched on and approved still isn't a renderer"
        );
        assert_eq!(gate("gone", &on, &all_approved), Err(Refused::Missing));

        // Approval is for one exact folder.
        let other_hash = |_: &str, hash: &str| hash == "an-older-hash";
        assert_eq!(gate("bars", &on, &other_hash), Err(Refused::NotApproved));

        let mut broken = renderer("bars");
        broken.error = Some("entry draw.sh is missing".into());
        assert_eq!(
            check("bars", &[broken], &on, &all_approved, &full()).map(|_| ()),
            Err(Refused::Broken("entry draw.sh is missing".into()))
        );
    }

    /// The gate a session starts through and stays running through, with the
    /// Plugins switch off as it is for every test in this process.
    #[test]
    fn a_renderer_starts_with_plugins_off() {
        assert!(
            !crate::plugins::allowed(),
            "the tests run with Enable Plugins off"
        );

        let folders = [renderer("bars")];
        let on = [record("bars", true)];
        assert_eq!(
            check("bars", &folders, &on, &all_approved, &full()).map(|folder| folder.id.as_str()),
            Ok("bars"),
            "its own switch and its approval are the whole gate"
        );

        // Still nothing runs without both.
        assert_eq!(
            check("bars", &folders, &on, &none_approved, &full()).map(|_| ()),
            Err(Refused::NotApproved)
        );
        assert_eq!(
            check(
                "bars",
                &folders,
                &[record("bars", false)],
                &all_approved,
                &full()
            )
            .map(|_| ()),
            Err(Refused::Off)
        );
    }

    /// A package that would be both is refused at the manifest, so the gate
    /// above never meets one: a renderer can't bring a plugin host with it.
    #[test]
    fn a_package_cant_be_a_renderer_and_a_plugin() {
        let document = |capabilities: serde_json::Value| {
            json!({
                "id": "both",
                "name": "Both",
                "version": "1",
                "api": 1,
                "entry": { "script": { "path": "draw.sh", "interpreter": "sh" } },
                "capabilities": capabilities,
            })
            .to_string()
        };

        for also in [
            json!({ "source": { "label": "Both" } }),
            json!({ "providers": { "lyrics": true } }),
            json!({ "panels": [{ "name": "P", "preset": { "panel_name": "library", "info": { "panel": {} } } }] }),
        ] {
            let mut capabilities = also.clone();
            capabilities["renderer"] = json!({ "label": "Both" });

            let refused = manifest::parse(&document(capabilities)).unwrap_err();
            assert!(
                refused.contains("declares nothing else"),
                "{also}: {refused}"
            );
        }
    }

    #[test]
    fn a_renderer_suggests_resting_and_the_list_says_so() {
        let folders = [package(
            "calm",
            json!({ "renderer": { "label": "Calm", "rests": true } }),
        )];

        assert!(listed(&folders, &[], &none_approved)[0].rests);
        assert!(!listed(&[renderer("bars")], &[], &none_approved)[0].rests);
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
            kept != id || check(kept, &folders, records, &all_approved, &full()).is_ok()
        };

        forget_exits(&|kept| passes(kept, &on));
        assert_eq!(due(id), None, "still on, still stopped");

        // Switched off, the gate turns it away and its count goes, so the
        // next switch on starts it fresh. Other tests' ids pass untouched.
        forget_exits(&|kept| passes(kept, &off));
        assert_eq!(due(id), Some(Duration::ZERO));
    }

    fn wanted(json: serde_json::Value) -> Vec<WantedSignal> {
        serde_json::from_value(json).unwrap()
    }

    fn pool_signal(id: u64, source: Source) -> Signal {
        Signal {
            id,
            source,
            ..Signal::default()
        }
    }

    #[test]
    fn a_wanted_signal_matches_by_kind_and_band_not_name() {
        let asks = wanted(json!([
            { "kind": "band", "low": 30, "high": 120, "name": "Kick" },
            { "kind": "level" },
            { "kind": "onset", "low": 30, "high": 120 },
            { "kind": "aggregate", "of": 0, "rate": 2 },
            { "kind": "sparkle" },
        ]));

        // A renamed band, saved a hair off whole hertz, still answers.
        let mut bass = pool_signal(
            4,
            Source::Band {
                lo: 30.2,
                hi: 119.8,
            },
        );
        bass.name = "Something else".into();
        let pool = vec![bass.clone(), pool_signal(9, Source::Level)];
        assert_eq!(
            missing_signals(&asks, &pool),
            [2, 3],
            "the onset and the total are missing; an unknown kind never is"
        );

        // The same band as another kind isn't the same signal.
        let only_band = vec![pool_signal(1, Source::Onset { lo: 30., hi: 121. })];
        assert_eq!(missing_signals(&asks, &only_band), [0, 1, 2, 3]);

        // A total over the band that answers is the total wanted.
        let all = vec![
            bass,
            pool_signal(9, Source::Level),
            pool_signal(11, Source::Onset { lo: 30., hi: 120. }),
            pool_signal(12, Source::Aggregate { of: 4, rate: 7. }),
        ];
        assert!(missing_signals(&asks, &all).is_empty());

        // A total over another signal isn't.
        let other_total = vec![
            all[0].clone(),
            all[1].clone(),
            all[2].clone(),
            pool_signal(12, Source::Aggregate { of: 9, rate: 2. }),
        ];
        assert_eq!(missing_signals(&asks, &other_total), [3]);
    }

    #[test]
    fn the_signals_added_carry_the_renderers_suggestions() {
        let asks = wanted(json!([
            { "kind": "band", "low": 40, "high": 90, "name": " Kick ", "response": 20 },
            { "kind": "trigger", "low": 2000, "high": 8000 },
            { "kind": "aggregate", "of": 0 },
            { "kind": "level", "threshold": 25 },
        ]));
        let pool = vec![pool_signal(7, Source::Level)];

        let added = signals_to_add(&asks, &pool);
        assert_eq!(added.len(), 3, "the level is already there");

        assert_eq!(added[0].id, 8, "numbered past the pool");
        assert_eq!(added[0].name, "Kick");
        assert!(matches!(added[0].source, Source::Band { lo: 40., hi: 90. }));
        assert_eq!(added[0].smooth, 0.2);

        assert_eq!(added[1].threshold, TRIGGER_SEED, "a trigger that can fire");
        assert_eq!(added[1].smooth, Signal::default().smooth);

        assert!(
            matches!(added[2].source, Source::Aggregate { of: 8, rate: 1. }),
            "a total over the band added with it"
        );

        // Over a band the pool already has, the total adds up that one.
        let with_band = vec![pool_signal(3, Source::Band { lo: 40., hi: 90. })];
        let added = signals_to_add(&asks, &with_band);
        assert!(
            added
                .iter()
                .any(|s| matches!(s.source, Source::Aggregate { of: 3, .. }))
        );
        assert!(signals_to_add(&[], &pool).is_empty());
    }

    /// A texture as gpui describes one, its memory the file at `path`.
    #[cfg(unix)]
    fn external(path: &std::path::Path) -> gpui::ExternalTexture {
        gpui::ExternalTexture {
            fd: std::fs::File::open(path).unwrap().into(),
            width: 1920,
            height: 1080,
            allocation_size: 8_355_840,
            memory_type: 7,
            format: 37,
            view_formats: vec![43, 37],
            flags: 8,
            usage: 0x1f,
            tiling: 0,
            layout: 1,
            device_uuid: [0xab; 16],
            driver_uuid: *b"0123456789abcdef",
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_texture_in_hello_says_everything_an_importer_repeats() {
        let path = std::env::temp_dir().join(format!("rox-renderers-hello-{}", std::process::id()));
        std::fs::write(&path, "texture").unwrap();
        let texture = external(&path);
        let _ = std::fs::remove_file(&path);

        assert_eq!(
            texture_hello(&texture, 5),
            json!({
                "fd": 5,
                "width": 1920,
                "height": 1080,
                "allocation_size": 8_355_840,
                "memory_type": 7,
                "handle_type": "opaque-fd",
                "format": 37,
                "view_formats": [43, 37],
                "flags": 8,
                "usage": 31,
                "tiling": 0,
                "layout": 1,
                "device_uuid": "abababababababababababababababab",
                "driver_uuid": "30313233343536373839616263646566",
            })
        );
    }

    #[test]
    fn the_handles_land_in_a_fixed_order() {
        let at = |cover, texture, spectrum| {
            let places = places(cover, texture, spectrum);
            (places.frames, places.cover, places.texture, places.spectrum)
        };

        assert_eq!(at(false, false, false), (3, None, None, None));
        assert_eq!(at(true, false, false), (3, Some(4), None, None));
        assert_eq!(at(false, true, false), (3, None, Some(4), None));
        assert_eq!(at(true, true, false), (3, Some(4), Some(5), None));

        // The spectrum goes after every handle there was before it.
        assert_eq!(at(false, false, true), (3, None, None, Some(4)));
        assert_eq!(at(true, false, true), (3, Some(4), None, Some(5)));
        assert_eq!(at(false, true, true), (3, None, Some(4), Some(5)));
        assert_eq!(at(true, true, true), (3, Some(4), Some(5), Some(6)));
    }

    /// Starts a renderer package whose script prints `hello`, the four fd
    /// variables and what each lent fd holds, with the frames, and the
    /// cover, the texture and the spectrum when asked, each a file holding
    /// its own name.
    #[cfg(unix)]
    fn spawned(
        tag: &str,
        cover: bool,
        texture: bool,
        spectrum: bool,
    ) -> (serde_json::Value, String, String) {
        use std::io::Read as _;
        use std::os::fd::AsFd;

        let dir = std::env::temp_dir().join(format!("rox-renderers-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("draw.sh"),
            r#"read hello
printf '%s\n' "$hello"
printf '%s|%s|%s|%s\n' "$ROX_FRAMES_FD" "${ROX_COVER_FD-}" "${ROX_TEXTURE_FD-}" "${ROX_SPECTRUM_FD-unset}"
n=3; while [ $n -le 24 ]; do
  if [ -e /dev/fd/$n ]; then printf '%s=' $n; eval "cat <&$n"; fi
  n=$((n+1))
done
"#,
        )
        .unwrap();

        let file = |name: &str| {
            let path = dir.join(name);
            std::fs::write(&path, format!("{name}\n")).unwrap();
            std::fs::File::open(&path).unwrap()
        };
        let frames = file("frames");
        let cover_file = file("cover");
        let spectrum_file = file("spectrum");
        drop(file("texture"));
        let memory = texture.then(|| external(&dir.join("texture")));

        let mut folder = package(tag, json!({ "renderer": { "label": "Probe" } }));
        folder.dir = dir.clone();
        let (mut process, stdin, mut stdout) = launch(
            &folder,
            &Tier::Unconfined,
            frames.as_fd(),
            cover.then(|| cover_file.as_fd()),
            spectrum.then(|| spectrum_file.as_fd()),
            memory.as_ref(),
            64,
            32,
        )
        .unwrap();

        let mut out = String::new();
        stdout.read_to_string(&mut out).unwrap();
        drop(stdin);
        process.wait_for(Duration::from_secs(5));
        let _ = std::fs::remove_dir_all(&dir);

        let mut lines = out.splitn(3, '\n');
        let hello = serde_json::from_str(lines.next().unwrap()).unwrap();
        let vars = lines.next().unwrap().to_string();
        let held = lines.next().unwrap_or_default().to_string();
        (hello, vars, held)
    }

    #[cfg(unix)]
    #[test]
    fn each_handle_is_named_where_it_lands() {
        let cases = [
            (false, false, false, "3|||unset", "3=frames\n"),
            (true, false, false, "3|4||unset", "3=frames\n4=cover\n"),
            (false, true, false, "3||4|unset", "3=frames\n4=texture\n"),
            (
                true,
                true,
                false,
                "3|4|5|unset",
                "3=frames\n4=cover\n5=texture\n",
            ),
            (false, false, true, "3|||4", "3=frames\n4=spectrum\n"),
            (
                true,
                false,
                true,
                "3|4||5",
                "3=frames\n4=cover\n5=spectrum\n",
            ),
            (
                false,
                true,
                true,
                "3||4|5",
                "3=frames\n4=texture\n5=spectrum\n",
            ),
            (
                true,
                true,
                true,
                "3|4|5|6",
                "3=frames\n4=cover\n5=texture\n6=spectrum\n",
            ),
        ];

        for (n, (cover, texture, spectrum, vars, held)) in cases.into_iter().enumerate() {
            let (hello, got_vars, got_held) =
                spawned(&format!("order-{n}"), cover, texture, spectrum);
            let case = format!("cover {cover}, texture {texture}, spectrum {spectrum}");

            assert_eq!(got_vars, vars, "{case}");
            assert_eq!(got_held, held, "{case}: nothing else is inherited");
            assert_eq!(hello["hello"], API, "{case}");
            assert_eq!(
                (hello["width"].clone(), hello["height"].clone()),
                (json!(64), json!(32))
            );

            let cover_fd = hello.get("cover").map(|cover| cover["fd"].clone());
            assert_eq!(cover_fd, cover.then(|| json!(4)), "{case}");

            let texture_fd = hello.get("texture").map(|texture| texture["fd"].clone());
            let expected = texture.then(|| json!(if cover { 5 } else { 4 }));
            assert_eq!(texture_fd, expected, "{case}: hello names the texture's fd");

            let spectrum_fd = hello.get("spectrum").map(|spectrum| spectrum["fd"].clone());
            let expected = spectrum.then(|| json!(4 + u32::from(cover) + u32::from(texture)));
            assert_eq!(
                spectrum_fd, expected,
                "{case}: hello names the spectrum's fd"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_renderer_that_asks_for_nothing_is_lent_no_spectrum() {
        let (hello, vars, held) = spawned("no-spectrum", false, false, false);

        assert_eq!(held, "3=frames\n", "no fd past the frames");
        assert!(vars.ends_with("|unset"), "no ROX_SPECTRUM_FD: {vars}");
        assert_eq!(
            hello,
            json!({ "hello": API, "width": 64, "height": 32, "handle": 0 }),
            "hello is what it was before any grant"
        );

        let (hello, _, _) = spawned("spectrum-hello", false, false, true);
        assert_eq!(
            hello["spectrum"],
            json!({ "fd": 4, "bytes": 4096, "log_bins": 128, "linear_bins": 512, "rate": 62.5 })
        );
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
