//! The shader panel: a WGSL fragment stage that owns a panel's whole body,
//! driven by the app's shared signal pool. The author writes one function,
//! `fs_user(uv)`, against the uniform block gpui binds; rox fills the
//! sixteen signal slots from routes and the eight `user_meta` floats from
//! the player, so an unrouted shader still moves with the music.
//!
//! Two paint paths, picked by what the source turns out to reference. A
//! shader reading nothing but its uniforms draws as an in-scene quad. One
//! reading `screen` (what's under the panel) or `prev` (its own last
//! frame) needs the region pass, keyed by this panel's entity id so two
//! shader panels each get their own feedback texture. Registration works
//! out which; getting it wrong paints nothing, since each call skips what
//! it can't run.
//!
//! Distinct from [`crate::panel::shader`], which is the surface shader any
//! panel can draw over its own body. That module owns the pieces both
//! share: the slot targets, the `// @slot n: name` convention, and the meta
//! floats. This one is the panel whose entire point is the shader.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gpui::{
    canvas, div, prelude::*, px, AnyElement, App, Context, Div, EntityId, EventEmitter,
    FocusHandle, Focusable, PathPromptOptions, SharedString, Subscription, UserShaderId,
    WeakEntity, Window,
};
use gpui_component::menu::PopupMenu;
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use rox_viz::signal::Route;
use rox_viz::AudioFeed;

use crate::assets::icons;
use crate::design::{palette, tokens};
// The surface-shader module, whose helpers this panel shares. Aliased
// because this file is `panels::shader` and that one is `panel::shader`,
// one letter apart.
use crate::panel::shader::{self as surface, SlotTargets, SourceWatch};
use crate::panel::{
    self, setting_row, toggle, AppState, PanelChrome, PanelSettings, ScrubState, ValueEdit,
};
use crate::panel_settings;
use crate::settings::ui::{self as settings_ui, section, SECTION_GAP};
use crate::signal_ui::{self, routes::RouteEditState};

/// The builtin shaders, so a fresh panel draws something before anyone has
/// written a line of WGSL. They're defined beside the surface shader's
/// pieces because the approval gate has to know them: what ships with the
/// binary runs without anybody agreeing to it a second time.
use surface::{PLASMA, PRESETS};

/// How much of a compile message the panel body shows. naga points at the
/// offending span with a caret line, which is the useful part; the rest is
/// context that would fill a small panel.
const ERROR_LINES: usize = 8;

/// The shader panel's per-view config: what a saved layout restores and
/// what the settings window edits.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ShaderConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    /// The switch, the same one a panel's surface shader has. Off keeps
    /// the source, the bindings and the bookmark exactly where they are and
    /// paints nothing, which is how saying no to an unread shader works:
    /// parking a look isn't the same as throwing it away.
    pub enabled: bool,
    /// The fragment stage itself, stored inline so a shader can travel
    /// inside a workspace bundle: a config with only an absolute path
    /// would import as a dead panel on anyone else's machine.
    pub source: String,
    /// A name in the workspace's shader pool. Set, the pool's copy runs and
    /// the inline source above goes unused, so one shader can dress
    /// several panels and the bundle's author edits it once. The rule is
    /// [`surface::resolve_source`]'s: a name the pool doesn't hold runs
    /// nothing rather than falling back to the inline text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Where the source was last read from. A bookmark for Reload and the
    /// file watch, never the thing that runs.
    pub path: Option<PathBuf>,
    /// Attachments of the app's shared signals onto the shader's slots. A
    /// route whose signal is gone from the pool leaves its slot at zero.
    pub routes: Vec<Route>,
    /// Hand-set slot values, from the Bindings page's slot rows: what a
    /// slot reads with no route driving it, which is how a shader's named
    /// parameters get tweaked without a signal in sight. A route on the
    /// same slot wins while it's there; the hand-set value comes back when
    /// it goes.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub manual: Vec<(u8, f32)>,
    /// Keep asking for frames while the audio is silent. Off, the shader
    /// parks where it stands and the panel costs nothing, the same
    /// freeze-on-pause the other visualizers do.
    pub run_when_idle: bool,
}

impl Default for ShaderConfig {
    fn default() -> Self {
        ShaderConfig {
            chrome: PanelChrome::default(),
            enabled: true,
            source: PLASMA.to_string(),
            name: None,
            path: None,
            routes: Vec::new(),
            manual: Vec::new(),
            run_when_idle: false,
        }
    }
}

/// What the panel puts over its own body when there's no shader on screen,
/// and the ways out of it. Every state that paints nothing goes through
/// here, so an off, unread or broken shader reads as a panel waiting on a
/// decision rather than as a black rectangle.
struct BodyNote {
    lines: Vec<String>,
    /// The buttons under the text, in the order they're drawn.
    actions: Vec<NoteAction>,
    /// A compiler message, which is laid out as it came: left aligned at
    /// the top, where its caret lines still mean something.
    raw: bool,
}

/// A button under the note.
#[derive(Clone, Copy)]
enum NoteAction {
    /// Open the Source settings page, where the whole source is listed
    /// with where it says it came from and its hash.
    Inspect,
    /// Run it: the approval an imported source is waiting on, the switch,
    /// or both.
    Enable,
    /// The same page, for a panel with nothing to enable yet.
    Pick,
}

impl NoteAction {
    fn label(self) -> SharedString {
        match self {
            NoteAction::Inspect => rox_i18n::t!("shader-panel-inspect"),
            NoteAction::Enable => rox_i18n::t!("shader-panel-enable"),
            NoteAction::Pick => rox_i18n::t!("shader-panel-pick"),
        }
    }

    fn icon(self) -> &'static str {
        match self {
            NoteAction::Inspect => icons::EYE,
            NoteAction::Enable => icons::PLAY,
            NoteAction::Pick => icons::BLEND,
        }
    }
}

/// What the last registration made of the current source. Shared with the
/// paint closure, which is where registration happens: it needs the window,
/// and the panel only has one while it's drawing.
#[derive(Default)]
struct Compiled {
    /// The program this ran against, so a change re-registers and nothing
    /// else does. See [`program_hash`] for what counts as a change: it's
    /// more than the text, since a program's images can move under it.
    key: u64,
    /// Whether an attempt has happened at all. A fresh panel and a panel
    /// whose shader hashes to zero are otherwise the same thing.
    ran: bool,
    /// What paints: the current source's registration, or the last one that
    /// compiled while a fresh edit is broken. An authoring loop saves
    /// half-written files constantly, and a panel that blanks on each of
    /// them is unusable.
    shader: Option<UserShaderId>,
    /// What registration said, verbatim from naga. None on a clean compile.
    error: Option<String>,
}

/// The last time the config's pool name was resolved: which name, at which
/// pool generation, and what came back. Kept because this panel re-renders
/// on every frame the audio moves, and resolution takes a lock and copies a
/// page of WGSL; the generation is one atomic load to check instead. Shaped
/// like [`Compiled`] above, down to the `ran` flag, since a name that
/// resolves to nothing and a name nobody has looked up yet are otherwise
/// the same thing.
#[derive(Default)]
struct Resolved {
    name: String,
    rev: u64,
    source: Option<String>,
    ran: bool,
}

/// What a registration is memoized under: the source, where its images
/// come from, and the pool's generation.
///
/// The generation is in there because a program can be wrong about its
/// images rather than about its code, and replacing an image changes no
/// source text at all. The pool watch pulls the new bytes in and bumps the
/// generation, and that re-registers here. It's the same key
/// [`surface`]'s own driver keeps, for the same reason.
fn program_hash(source: &str, ctx: &surface::ProgramCtx, cover: u64) -> u64 {
    use std::hash::{Hash as _, Hasher as _};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    ctx.name.hash(&mut hasher);
    ctx.path.hash(&mut hasher);
    rox_core::settings::shader_pool_rev().hash(&mut hasher);
    cover.hash(&mut hasher);
    hasher.finish()
}

pub struct ShaderPanel {
    state: AppState,
    config: ShaderConfig,
    feed: Arc<AudioFeed>,
    compiled: Arc<Mutex<Compiled>>,
    /// What the config's pool name last resolved to. A cell because every
    /// reader of the running source is a `&self` render path, and the panel
    /// is single-threaded like every other view.
    resolved: RefCell<Resolved>,
    /// The hot-reload watch on the config's path, the same one a panel's
    /// surface shader uses.
    watch: SourceWatch,
    /// The Bindings page's route editor state: span sliders and which rows
    /// stand open. Not config: the fold is where you are in the page.
    routes_ui: RouteEditState,
    /// One scrub per slot row, for the hand-set values on unrouted slots.
    slot_scrubs: Vec<ScrubState>,
    /// The name a save into the workspace's shaders would use, while
    /// it's being typed.
    shader_name: panel_settings::ShaderNameField,
    /// The one readout being typed into across all the settings sliders.
    value_edit: ValueEdit,
    focus: FocusHandle,
    /// The tab panel this panel is currently in, for duplicate and
    /// pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// Wakes the panel on every pump tick, so the shader's frames run on
    /// the same clock the audio arrives on.
    _player_changed: Subscription,
}

impl ShaderPanel {
    pub fn new(state: AppState, config: ShaderConfig, cx: &mut Context<Self>) -> Self {
        let _player_changed = cx.observe(&state.player, |_, _, cx| cx.notify());
        ShaderPanel {
            feed: state.player.read(cx).feed(),
            state,
            config,
            compiled: Arc::new(Mutex::new(Compiled::default())),
            resolved: RefCell::new(Resolved::default()),
            watch: SourceWatch::default(),
            routes_ui: RouteEditState::default(),
            slot_scrubs: (0..surface::SLOTS).map(|_| ScrubState::default()).collect(),
            shader_name: panel_settings::ShaderNameField::default(),
            value_edit: ValueEdit::default(),
            focus: cx.focus_handle().tab_stop(true),
            tab_panel: None,
            _player_changed,
        }
    }

    /// Pick up edits to the file the source came from. This runs off the
    /// render, which the player's pump drives, so the watch runs while
    /// there's anything to watch it for; a parked panel reloads on the
    /// button instead.
    ///
    /// A source still waiting on approval doesn't reload: the path arrived
    /// with it, and reading a file a bundle chose would be trusting the
    /// bundle by the back door.
    fn poll_reload(&mut self, cx: &mut Context<Self>) {
        // The pool's watch first, since a named panel has no file of its own
        // to poll and this is where its edits come from. It's throttled and
        // app-wide, so several shader panels cost one sweep between them.
        surface::poll_pool();
        let Some(path) = self.config.path.clone() else {
            return;
        };
        // A named panel doesn't watch a file. Its text belongs to the
        // workspace's pool, and the bookmark points at whatever was inlined
        // before the name went on, so a reload would pull the pool's source
        // out from under the panel. The pool entry does its own watching.
        if self.config.name.is_some() {
            return;
        }
        if self.pending() {
            return;
        }
        if let Some(source) = self.watch.poll(&path) {
            if source != self.config.source {
                self.set_source(source, Some(path), cx);
            }
        }
    }

    /// Put a new source in place and forget everything about the last one:
    /// its compile message was about text that just left, and its file
    /// stamp would have the watch pull the old file back over it. The
    /// registration stands until the new source compiles, so a save from an
    /// editor mid-edit shows its error over the shader that was running
    /// rather than a blank panel.
    ///
    /// Every caller is the user putting the source there (a preset, a file
    /// they picked, a reload, an edit under a file they pointed rox at), so
    /// this is where a source earns its approval.
    ///
    /// It's also where a panel comes off a pool shader. Choosing a source is
    /// choosing it for this panel, and a name left on would keep running the
    /// workspace's copy over the top of what was just picked.
    fn set_source(&mut self, source: String, path: Option<PathBuf>, cx: &mut Context<Self>) {
        surface::approve(&source);
        let cleared = source.trim().is_empty();
        // Picking a source is asking to see it. A panel parked by an earlier
        // Turn Off would otherwise take the new shader and stay dark.
        self.config.enabled = true;
        self.config.source = source;
        self.config.name = None;
        self.config.path = path.clone();
        self.watch = SourceWatch::seeded(path.as_deref());
        {
            let mut compiled = self.compiled.lock().unwrap();
            // A cleared source leaves nothing to keep on screen; any other
            // one holds the last good registration until it has its own.
            let keep = if cleared { None } else { compiled.shader };
            *compiled = Compiled {
                shader: keep,
                ..Compiled::default()
            };
        }
        cx.notify();
    }

    /// The WGSL this panel actually runs: the workspace pool's copy when the
    /// config names one, its own inline source otherwise. Everything that
    /// reasons about what's on screen goes through here, while the settings
    /// pages keep editing `config.source`: a nameless panel runs it, and a
    /// named one keeps it waiting for when the name comes off.
    fn running(&self) -> String {
        self.resolved().unwrap_or_default()
    }

    /// [`running`](Self::running) before the missing case is flattened
    /// away: None means the config names a shader this workspace's pool
    /// doesn't hold, which is a different problem from an empty source and
    /// gets its own line in the body.
    fn resolved(&self) -> Option<String> {
        let Some(name) = self.config.name.as_deref() else {
            return Some(self.config.source.clone());
        };
        let rev = rox_core::settings::shader_pool_rev();
        let mut cache = self.resolved.borrow_mut();
        if !cache.ran || cache.rev != rev || cache.name != name {
            *cache = Resolved {
                name: name.to_string(),
                rev,
                source: surface::resolve_source(Some(name), &self.config.source),
                ran: true,
            };
        }
        cache.source.clone()
    }

    /// Whether the panel is using a pool shader the workspace doesn't
    /// hold. Nothing paints in that state and no compile ever ran, so the
    /// body has to be the one that says why.
    fn pool_missing(&self) -> bool {
        self.resolved().is_none()
    }

    /// Whether the source is waiting on approval: it arrived inside a
    /// layout or a workspace bundle and nobody on this machine has agreed
    /// to run it yet. Asked of what runs rather than of the config, so a
    /// shader pulled from the pool goes through the same gate an inline one
    /// does instead of slipping in behind an empty `source`.
    fn pending(&self) -> bool {
        !surface::approved(&self.running())
    }

    /// Agree to run what the config holds. The one button that puts a
    /// hash in the approved list without the source having come from a file
    /// or a preset.
    ///
    /// The path goes: it named a file on whichever machine wrote the
    /// bundle, and if this one happens to have something at that path, the
    /// watch would pull it straight over the text just approved. Picking a
    /// file again is how an imported shader gets a local one.
    ///
    /// It's the switch too: approving is saying run it, so a panel an
    /// earlier Turn Off parked comes back on here rather than staying dark
    /// with its approval quietly granted.
    fn approve(&mut self, cx: &mut Context<Self>) {
        // What runs, so a panel using a pool shader agrees to the text the
        // pool holds rather than to the inline copy it isn't running.
        surface::approve(&self.running());
        self.config.enabled = true;
        self.config.path = None;
        self.watch = SourceWatch::default();
        *self.compiled.lock().unwrap() = Compiled::default();
        cx.notify();
    }

    /// Turn the panel back on, from the button the body shows while it
    /// isn't. An unread source needs its approval on the way, an approved
    /// one just needs the switch: the path a bundle wrote is only worth
    /// dropping on the approval, and a local file this machine picked should
    /// keep hot reloading after a trip through the switch.
    fn enable(&mut self, cx: &mut Context<Self>) {
        if self.pending() {
            self.approve(cx);
        } else {
            self.config.enabled = true;
            cx.notify();
        }
    }

    /// Say no to the pending source: park it rather than delete it. The
    /// source, the pool name, the bookmark and the routes all stay put with
    /// the switch off, so the panel still says what it was given and turning
    /// it back on is one toggle plus the approval.
    fn turn_off(&mut self, cx: &mut Context<Self>) {
        self.config.enabled = false;
        cx.notify();
    }

    /// Write the shader out to a file and hand it to whatever opens `.wgsl`
    /// on this machine. rox has no editor of its own, so this plus the file
    /// watch is the authoring loop.
    ///
    /// An inline shader keeps the bookmark and watches its own file. A named
    /// one ejects through its pool entry and the bookmark is recorded there,
    /// since the source belongs to the workspace and so do the edits.
    fn eject(&mut self, cx: &mut Context<Self>) {
        let ejected = match self.config.name.as_deref() {
            Some(name) => surface::eject_pool_entry(name),
            None => {
                let label = self.config.chrome.title.clone().unwrap_or_default();
                surface::eject(
                    &surface::eject_name(&label, &self.config.source),
                    &self.config.source,
                )
            }
        };
        match ejected {
            Ok(path) => {
                if self.config.name.is_none() {
                    self.config.path = Some(path.clone());
                    // Seeded: the file was just written from this source, so
                    // only the next edit should wake the watch.
                    self.watch = SourceWatch::seeded(Some(path.as_path()));
                }
                cx.open_with_system(&path);
                cx.notify();
            }
            Err(error) => {
                *self.compiled.lock().unwrap() = Compiled {
                    error: Some(
                        rox_i18n::t!("shader-eject-failed", error = error.to_string()).to_string(),
                    ),
                    ..Compiled::default()
                };
                cx.notify();
            }
        }
    }

    /// Open the in-app editor over what runs. A named panel edits the pool
    /// entry, so every panel on the name follows an apply; an inline one
    /// edits its own text through [`apply_edit`](Self::apply_edit).
    fn open_editor(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        use surface::edit::{EditKey, ShaderEditTarget};

        let target = match self.config.name.as_deref() {
            Some(name) => ShaderEditTarget::pool(name),
            None => {
                let title = self
                    .config
                    .chrome
                    .title
                    .clone()
                    .unwrap_or_else(|| panel::display_name(self.panel_name()));
                let panel = cx.entity().downgrade();
                Some(ShaderEditTarget {
                    key: EditKey::Panel(cx.entity_id()),
                    title: title.into(),
                    source: self.config.source.clone(),
                    ctx: surface::ProgramCtx::of(None, self.config.path.as_deref()),
                    path: self.config.path.clone(),
                    write: Arc::new(move |source, cx| {
                        if let Some(panel) = panel.upgrade() {
                            panel.update(cx, |this, cx| this.apply_edit(source, cx));
                        }
                    }),
                })
            }
        };
        if let Some(target) = target {
            rox_panel_api::openers::shader_editor(self.state.clone(), target, cx);
        }
    }

    /// Take an applied buffer from the editor. Unlike [`set_source`]
    /// (Self::set_source) the bookmark stays: the editor wrote the file
    /// from this same text, so the watch reseeds on it and only the next
    /// outside edit wakes it. The approval already happened on the way in.
    /// The registration re-runs on its own, since the program's hash moved.
    fn apply_edit(&mut self, source: String, cx: &mut Context<Self>) {
        // Applying an edit is asking to see it.
        self.config.enabled = true;
        self.config.source = source;
        self.watch = SourceWatch::seeded(self.config.path.as_deref());
        cx.notify();
    }

    /// Take a copy of the pool shader this panel is using and stop using
    /// it. The text is the one that was already running, so its approval
    /// still holds; no bookmark comes across, since the pool entry's file
    /// belongs to the pool and a second watcher would drift the two apart.
    fn detach(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = self
            .config
            .name
            .as_deref()
            .and_then(rox_core::settings::shader_pool_get)
        else {
            return;
        };
        self.set_source(entry.source, None, cx);
    }

    /// Point the panel at one of the workspace's shaders. The opposite of
    /// [`detach`](Self::detach), and it clears the same fields for the same
    /// reason: the inline source and the bookmark both go, because the
    /// workspace holds what runs from here and a second copy on the panel
    /// would only be the one that's wrong after the next edit to the
    /// shared entry.
    ///
    /// Nothing is approved on the way through. A workspace shader that came
    /// in with a bundle still has to be read before it runs, which is the
    /// same gate a bundle-applied name goes through.
    fn use_pool_name(&mut self, name: String, cx: &mut Context<Self>) {
        // Same as picking any other source: choosing one is asking to see it.
        self.config.enabled = true;
        self.config.name = Some(name);
        self.config.source = String::new();
        self.config.path = None;
        self.watch = SourceWatch::default();
        *self.compiled.lock().unwrap() = Compiled::default();
        cx.notify();
    }

    /// Promote the panel's source into the workspace's shaders and use it
    /// by name from there. The inline copy goes: the pool holds the source
    /// now, and a second copy on the panel would only be the one that's
    /// wrong after the next pool edit.
    fn save_to_pool(&mut self, name: String, cx: &mut Context<Self>) {
        let name = name.trim().to_string();
        if name.is_empty() || self.config.source.trim().is_empty() {
            return;
        }
        // The panel's own bookmark goes with it, so a shader that was being
        // edited in a file goes on hot reloading through the pool's watch.
        surface::save_to_pool(&name, &self.config.source, self.config.path.clone());
        self.config.name = Some(name);
        self.config.source = String::new();
        self.config.path = None;
        self.watch = SourceWatch::default();
        // The registration stands: the pool holds the same text that was
        // running a moment ago, so there's nothing to recompile.
        cx.notify();
    }

    /// Snapshot a file into the panel's source. A file that won't read
    /// shows in the same readout a failed compile does, since from the
    /// panel's side they're the same problem.
    fn load_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        match std::fs::read_to_string(&path) {
            Ok(source) => self.set_source(source, Some(path), cx),
            Err(error) => {
                *self.compiled.lock().unwrap() = Compiled {
                    error: Some(format!("reading {}: {error}", path.display())),
                    ..Compiled::default()
                };
                cx.notify();
            }
        }
    }

    /// Browse for a shader file.
    fn pick_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.pop() else {
                return;
            };
            this.update(cx, |this, cx| this.load_file(path, cx)).ok();
        })
        .detach();
    }

    /// Re-read the file the source came from, for an edit the watch hasn't
    /// caught yet or a panel that's been parked.
    fn reload(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.config.path.clone() {
            self.load_file(path, cx);
        }
    }

    /// Load one of the shipped examples. The path goes with it: an example
    /// has no file behind it, and leaving the old one recorded would have
    /// the watch overwrite it a second later.
    fn use_preset(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(preset) = PRESETS.get(index) {
            self.set_source(preset.source.to_string(), None, cx);
        }
    }

    /// What the panel says instead of running: nothing loaded, a source
    /// nobody has read yet, a switch that's off, or what registration made
    /// of what's there. None while the shader draws.
    fn body_note(&self) -> Option<BodyNote> {
        let note = |lines: Vec<String>, actions: Vec<NoteAction>| {
            Some(BodyNote {
                lines,
                actions,
                raw: false,
            })
        };
        // A name the workspace's pool doesn't hold. Nothing paints and
        // nothing else in the app would say why, so the panel does.
        if let Some(name) = self.config.name.as_deref().filter(|_| self.pool_missing()) {
            return note(
                vec![
                    rox_i18n::t!("shader-panel-note-missing-title", name = name.to_string())
                        .to_string(),
                    rox_i18n::t!("shader-panel-note-missing-body").to_string(),
                ],
                vec![NoteAction::Pick],
            );
        }
        if self.running().trim().is_empty() {
            return note(
                vec![
                    rox_i18n::t!("shader-panel-note-empty-title").to_string(),
                    rox_i18n::t!("shader-panel-note-empty-body").to_string(),
                ],
                vec![NoteAction::Pick],
            );
        }
        if self.pending() {
            return note(
                vec![
                    rox_i18n::t!("shader-panel-note-pending-title").to_string(),
                    rox_i18n::t!("shader-panel-note-pending-body").to_string(),
                ],
                vec![NoteAction::Inspect, NoteAction::Enable],
            );
        }
        if !self.config.enabled {
            return note(
                vec![
                    rox_i18n::t!("shader-panel-note-off-title").to_string(),
                    rox_i18n::t!("shader-panel-note-off-body").to_string(),
                ],
                vec![NoteAction::Inspect, NoteAction::Enable],
            );
        }
        let error = self.compiled.lock().unwrap().error.clone()?;
        // A backend with no shader pipeline turns every source down the same
        // way, so it gets the plain note the other non-running states get
        // rather than a compiler readout for a compile that never ran.
        if surface::unsupported(&error) {
            return note(
                vec![
                    format!("{}.", surface::NO_PIPELINE_TITLE),
                    surface::NO_PIPELINE_NOTE.to_string(),
                ],
                vec![NoteAction::Inspect],
            );
        }
        // naga's message runs several lines, with a caret under the span it
        // is complaining about. They have to stay lines, and they stay left
        // aligned at the top: centred, the carets point at the wrong
        // columns and a long message clips at both ends.
        Some(BodyNote {
            lines: std::iter::once(rox_i18n::t!("shader-panel-compile-error").to_string())
                .chain(error.lines().take(ERROR_LINES).map(str::to_string))
                .collect(),
            actions: vec![NoteAction::Inspect],
            raw: true,
        })
    }
}

impl PanelSettings for ShaderPanel {
    fn state(&self) -> AppState {
        self.state.clone()
    }

    // The body already is a shader; offering a second one over it reads
    // as a mistake.
    fn surface_shader(&self) -> bool {
        false
    }

    fn chrome(&self) -> &PanelChrome {
        &self.config.chrome
    }

    fn chrome_mut(&mut self) -> &mut PanelChrome {
        &mut self.config.chrome
    }

    fn set_custom_title(&mut self, title: Option<String>, cx: &mut Context<Self>) {
        self.config.chrome.title = title;
        panel::refresh_tab_panel(&self.tab_panel, cx);
        cx.notify();
    }

    fn pages(&self) -> &'static [(&'static str, &'static str)] {
        // No Signals page: the pool is app-global and edits in the Signals
        // window, same as everywhere else. Bindings points there when the
        // pool is empty.
        &[("Source", icons::BLEND), ("Bindings", icons::SLIDERS)]
    }

    fn page(
        &mut self,
        page: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match page {
            "Bindings" => self.bindings_page(cx).into_any_element(),
            _ => self.source_page(window, cx).into_any_element(),
        }
    }
}

impl ShaderPanel {
    /// The Source page: one picker for where the shader comes from, the
    /// rows that selection needs under it, and what registration made of
    /// the result.
    fn source_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let path = self.config.path.clone();
        let named = self.config.name.clone();
        // What runs, which for a named panel is the workspace's copy. The
        // approval block reads it too, so a shader that arrived in a bundle
        // gets read before it runs whichever way it got here.
        let resolved = self.resolved();
        let running = resolved.clone().unwrap_or_default();
        let error = self.compiled.lock().unwrap().error.clone();
        let run_when_idle = self.config.run_when_idle;
        let enabled = self.config.enabled;
        let pending = self.pending().then(|| {
            panel_settings::pending_shader(
                "shader-panel-pending",
                &running,
                path.as_deref(),
                cx.listener(|this, _, _, cx| this.approve(cx)),
                cx.listener(|this, _, _, cx| this.turn_off(cx)),
            )
        });

        // The name a save would use, read before the field goes out
        // on loan to the picker block.
        let fallback = {
            let label = self.config.chrome.title.clone().unwrap_or_default();
            surface::eject_name(&label, &self.config.source)
        };
        let picked = panel_settings::ShaderSource {
            id: "shader-panel",
            name: named.as_deref(),
            path: path.as_deref(),
            resolved: resolved.as_deref(),
            // No None entry here: this panel's whole body is the shader, so
            // an empty one is a mistake rather than a state to pick. And
            // covering that body is the point, so every shader is offered.
            clear: None,
            overlays_only: false,
            use_example: |this: &mut Self, index, cx| this.use_preset(index, cx),
            use_named: |this: &mut Self, name, cx| this.use_pool_name(name, cx),
            choose_file: |this: &mut Self, window, cx| this.pick_file(window, cx),
            edit: |this: &mut Self, window, cx| this.open_editor(window, cx),
            eject: |this: &mut Self, cx| this.eject(cx),
            detach: |this: &mut Self, cx| this.detach(cx),
            reload: |this: &mut Self, cx| this.reload(cx),
            save: |this: &mut Self, name, cx| this.save_to_pool(name, cx),
            field: &mut self.shader_name,
            fallback: &fallback,
        }
        .render(window, cx);

        let mut shader = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(setting_row(
                rox_i18n::t!("shader-panel-run-shader"),
                Some(rox_i18n::t!("shader-panel-run-shader.description")),
                // The switch and nothing else. An unread source still has
                // the approval block above to get through, so flicking this
                // on can't be the way past it.
                toggle(
                    enabled,
                    |this: &mut Self, on, cx| {
                        this.config.enabled = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(picked);
        if let Some(error) = error {
            // The callout the app's Overlay Shader section uses, for the
            // same reason: the switch right above reads as on, and a muted
            // block under it isn't enough to say that nothing behind it is
            // running.
            shader = shader.child(match surface::unsupported(&error) {
                true => panel::banner(
                    panel::Tone::Bad,
                    surface::NO_PIPELINE_TITLE,
                    vec![surface::NO_PIPELINE_NOTE.into()],
                ),
                false => panel::banner(
                    panel::Tone::Bad,
                    rox_i18n::t!("shader-panel-compile-title"),
                    error
                        .lines()
                        .take(ERROR_LINES)
                        .map(|line| SharedString::from(line.to_string()))
                        .collect(),
                ),
            });
        }
        shader = shader.child(setting_row(
            rox_i18n::t!("panel-run-when-idle"),
            Some(
                "Keep drawing while the audio is silent. Off, the shader parks where it \
                 stands and the panel costs nothing"
                    .into(),
            ),
            toggle(
                run_when_idle,
                |this: &mut Self, on, cx| {
                    this.config.run_when_idle = on;
                    cx.notify();
                },
                cx,
            ),
        ));

        div()
            .flex()
            .flex_col()
            .gap(SECTION_GAP)
            .children(
                pending.map(|body| section(rox_i18n::t!("panel-awaiting-approval"), None, body)),
            )
            .child(section(rox_i18n::t!("panel-section-shader"), None, shader))
    }

    /// The Bindings page: the routes filling the shader's slots, in the
    /// same editor the panel Shader page and the app's screen shader use,
    /// over a live readout of all sixteen slots. The names come off the
    /// source's `// @slot n: name` comments where it declares them.
    fn bindings_page(&mut self, cx: &mut Context<Self>) -> Div {
        // Off what runs, so a panel using a pool shader reads the pool's
        // slot names rather than the inline copy it left behind.
        let running = self.running();
        let labels = surface::slot_labels(&running);
        self.routes_ui.sync(self.config.routes.len());

        let hub = self.state.signals.clone();
        let editor = signal_ui::routes::RouteEditor {
            id: "shader-panel-route",
            hub: &hub,
            routes: &self.config.routes,
            labels: &labels,
            value_edit: &self.value_edit,
            ui: &self.routes_ui,
            ui_mut: |this: &mut Self| &mut this.routes_ui,
            mutate: Arc::new(
                |this: &mut Self, edit: &mut dyn FnMut(&mut Vec<Route>), cx: &mut Context<Self>| {
                    edit(&mut this.config.routes);
                    cx.notify();
                },
            ),
        };
        let add = editor.add_button(cx);

        let slots = signal_ui::slots::SlotList {
            hub: &hub,
            routes: &self.config.routes,
            manual: &self.config.manual,
            labels: &labels,
            value_edit: &self.value_edit,
            scrubs: &self.slot_scrubs,
            set: Arc::new(|this: &mut Self, slot, value, cx| {
                surface::set_manual_value(&mut this.config.manual, slot, value);
                cx.notify();
            }),
        }
        .render(cx);

        div()
            .flex()
            .flex_col()
            .gap(SECTION_GAP)
            .child(section(
                rox_i18n::t!("shader-panel-section-routes"),
                Some(add.into_any_element()),
                editor.list(cx),
            ))
            .child(section(rox_i18n::t!("panel-section-slots"), None, slots))
    }
}

impl EventEmitter<PanelEvent> for ShaderPanel {}

impl Focusable for ShaderPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for ShaderPanel {
    fn panel_name(&self) -> &'static str {
        "shader"
    }

    rox_panel_api::opens_settings!();

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("panel-title-shader"),
        )
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.config.chrome.title.clone().map(SharedString::from)
    }

    fn locked(&self, _cx: &App) -> bool {
        self.config.chrome.locked
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }

    fn min_size(&self, _cx: &App) -> gpui::Size<gpui::Pixels> {
        panel::chrome_min_size(
            &self.config.chrome,
            gpui::size(
                rox_dock::resizable::PANEL_MIN_SIZE,
                rox_dock::resizable::PANEL_MIN_SIZE,
            ),
        )
    }

    fn max_size(&self, cx: &App) -> gpui::Size<gpui::Pixels> {
        panel::chrome_max_size(&self.config.chrome, self.min_size(cx))
    }

    /// The layout dump stores the panel's config, source and all; the
    /// builder registered in `workspace::register_panels` reads it back.
    fn dump(&self, _cx: &App) -> rox_dock::PanelState {
        let mut state = rox_dock::PanelState::new(self);
        state.info = rox_dock::PanelInfo::panel(
            serde_json::to_value(self.config.clone()).unwrap_or(serde_json::Value::Null),
        );
        state
    }

    fn on_added_to(
        &mut self,
        tab_panel: WeakEntity<TabPanel>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tab_panel = Some(tab_panel.clone());
        self.state
            .tab_hosts
            .update(cx, |hosts, _| hosts.report(tab_panel));
    }

    fn on_removed(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.tab_panel = None;
    }

    fn dropdown_menu(
        &mut self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        // Icon on the row so it lines up with Rename and the rest of the tail
        // and the tick shows on the right, the way every other top-level
        // check row in the app reads. The icon-less form is for flyouts.
        let menu = menu.item(panel::check_row(
            rox_i18n::t!("panel-run-when-idle"),
            Some(icons::CLOCK),
            |this: &Self| this.config.run_when_idle,
            |this: &mut Self, _| this.config.run_when_idle = !this.config.run_when_idle,
            &cx.entity(),
        ));
        let menu =
            panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity(), cx);
        let menu = panel::duplicate_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            |this, _window, cx| {
                let (state, config) = {
                    let panel = this.read(cx);
                    (panel.state.clone(), panel.config.clone())
                };
                ShaderPanel::new(state, config, cx)
            },
        );
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
            window,
        )
    }
}

impl Render for ShaderPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        // The panel is a focus stop: a click puts the keyboard here and
        // tab walks to it, which is also what puts its tab group on the
        // focus path for the tab-cycle chord.
        let focus = self.focus.clone();
        panel::themed(&chrome, || self.body(cx).track_focus(&focus))
    }
}

impl ShaderPanel {
    fn body(&mut self, cx: &mut Context<Self>) -> Div {
        self.poll_reload(cx);
        // Read here rather than in the paint closure, which has no cx: the
        // hub needs it to spot a song change for the aggregates that reset
        // on one, and a render happens every frame audio moves.
        let track = self.state.player.read(cx).playing_entry();
        let note = self.body_note().map(|note| self.note_overlay(note, cx));

        // A shader that's off or still waiting to be read never gets
        // registered: the canvas paints nothing and the note above says
        // what the panel is waiting on.
        let source = if self.pending() || !self.config.enabled {
            String::new()
        } else {
            self.running()
        };
        // Where the images a program declares are read from. A named panel
        // holds no path: its bookmark points at whatever was inlined before
        // the name went on, and the pool entry keeps its own, which the
        // resolve falls back to.
        let ctx = surface::ProgramCtx::of(
            self.config.name.as_deref(),
            match self.config.name {
                Some(_) => None,
                None => self.config.path.as_deref(),
            },
        );
        let routes = self.config.routes.clone();
        let manual = self.config.manual.clone();
        let run_when_idle = self.config.run_when_idle;
        let hub = self.state.signals.clone();
        let feed = self.feed.clone();
        let compiled = self.compiled.clone();
        let panel = cx.entity().entity_id();

        div()
            .size_full()
            .relative()
            .bg(palette::bg_root())
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        paint(
                            bounds,
                            window,
                            cx,
                            &source,
                            &ctx,
                            &routes,
                            &manual,
                            run_when_idle,
                            &hub,
                            &feed,
                            track,
                            &compiled,
                            panel,
                        );
                    },
                )
                .size_full(),
            )
            .children(note)
    }

    /// The note over the panel's own body: what it's waiting on, centred,
    /// with the buttons that act on it. A compiler message keeps its lines
    /// left aligned, since its carets only line up that way, but the block
    /// is still centred in the panel like every other note.
    fn note_overlay(&self, note: BodyNote, cx: &Context<Self>) -> Div {
        let raw = note.raw;
        let mut buttons = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .justify_center()
            .gap(tokens::SPACE_SM);
        for action in note.actions {
            let click = cx.listener(move |this: &mut Self, _, _, cx| match action {
                // The Source page is where the whole source is listed, with
                // where it says it came from and its hash under it.
                //
                // Deferred: opening reads the panel for its name and its
                // shared state, and this handler is running inside the
                // panel's own update, which is a second read of a borrow
                // that's already out.
                NoteAction::Inspect | NoteAction::Pick => {
                    let panel = cx.entity();
                    cx.defer(move |cx| panel_settings::open_page(panel, "Source", cx));
                }
                NoteAction::Enable => this.enable(cx),
            });
            buttons = buttons.child(settings_ui::small_button(
                action.label(),
                action.icon(),
                false,
                click,
            ));
        }

        let lines = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            // A naga message wraps rather than running off both edges: the
            // block is centred, so anything wider than the panel would lose
            // its left end as readily as its right.
            .max_w_full()
            .when(!raw, |lines| lines.items_center().text_center())
            .children(note.lines);

        div()
            .absolute()
            .inset_0()
            .p(tokens::SPACE_MD)
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .items_center()
            .justify_center()
            .overflow_hidden()
            .text_xs()
            .text_color(palette::text_muted())
            .child(lines)
            .child(buttons)
    }
}

/// One frame of the shader: register what the config holds, resolve the
/// routes, and record the right kind of pass.
#[allow(clippy::too_many_arguments)]
fn paint(
    bounds: gpui::Bounds<gpui::Pixels>,
    window: &mut Window,
    cx: &mut App,
    source: &str,
    ctx: &surface::ProgramCtx,
    routes: &[Route],
    manual: &[(u8, f32)],
    run_when_idle: bool,
    hub: &Arc<rox_viz::signal::SignalHub>,
    feed: &AudioFeed,
    track: Option<u64>,
    compiled: &Mutex<Compiled>,
    panel: EntityId,
) {
    if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) || source.trim().is_empty() {
        return;
    }
    // A program using the track's art follows the track: the poll moves
    // the cover feed when the playing file turns over, and the moved rev
    // re-registers the program below.
    let cover = if surface::uses_cover(source) {
        surface::poll_cover(window, cx)
    } else {
        0
    };
    let hash = program_hash(source, ctx, cover);
    let shader = {
        let mut compiled = compiled.lock().unwrap();
        if !compiled.ran || compiled.key != hash {
            // Registration caches by content, but only what compiled: a
            // rejection re-runs naga on every call, and this closure runs
            // every frame. So a broken program is tried once and the result
            // kept until something about it moves.
            let previous = compiled.error.take();
            // What's on screen stays on screen through a failed compile:
            // the message shows in the body over a shader that still runs,
            // which makes saving from an editor bearable.
            let good = compiled.shader;
            // The whole program: the text splits into its passes here and
            // its images are read from the pool entry or from beside the
            // source, so a bad plate reads out like a bad line of WGSL.
            *compiled = match surface::register_program(window, source, ctx) {
                Ok(shader) => Compiled {
                    key: hash,
                    ran: true,
                    shader: Some(shader),
                    error: None,
                },
                Err(message) => Compiled {
                    key: hash,
                    ran: true,
                    shader: good,
                    error: Some(message),
                },
            };
            if previous != compiled.error {
                // The body renders this message and was built before this
                // ran, so the panel needs another pass to show it. Without
                // the nudge a broken shader asks for no frames and the
                // message never shows.
                cx.notify(panel);
            }
        }
        compiled.shader
    };
    // Nothing to draw, and the message is already on its way to the body.
    let Some(shader) = shader else {
        return;
    };

    let mut targets = SlotTargets::default();
    surface::seed_manual(&mut targets, manual);
    // The tick is deduped inside the hub, so several panels on the same
    // pool cost one.
    hub.tick(feed, track);
    signal_ui::apply_routes(routes, hub, &mut targets);
    let meta = surface::meta_slots(window, cx);
    // A shader that reads the pointer keeps asking for frames while the
    // pointer counts for anything, so presence eases off on a panel that
    // would otherwise be parked, and the watch wakes the panel when the
    // hand comes back to a shader that already faded out.
    let cursor = surface::reads_cursor(source);
    if cursor {
        surface::watch_cursor(window);
    }

    // Caps decide the path: a program that reads the screen under it, its
    // own last frame, an image, or runs more than one pass needs the region
    // pass, and one that draws from nothing but its uniforms is a plain
    // in-scene quad. Getting this backwards paints nothing at all, since
    // each call skips what it can't run.
    let screen = window
        .user_shader_caps(shader)
        .is_some_and(|caps| caps.screen_pass_only());
    if screen {
        // The entity id keys the feedback texture, so two shader panels
        // running the same source each smear their own.
        window.paint_screen_shader(bounds, shader, panel.as_u64(), targets.slots, meta);
    } else {
        window.paint_user_shader(bounds, shader, targets.slots, meta);
    }

    // A docked panel renders cached: a clean frame replays the recorded
    // primitive with the values it was recorded with, so an animating
    // shader needs this view dirtied every frame. `request_animation_frame`
    // notifies exactly this view, which is the cheap wake: a window
    // refresh would rebuild every view in the window uncached.
    // Settling as well as live: the release runs on after the audio stops,
    // and a panel that parks on the last live frame holds its fade halfway
    // down instead of playing it out.
    if hub.live() || hub.settling() || run_when_idle || (cursor && meta[6] > 0.0) {
        window.request_animation_frame();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // The other builtin, checked here for the paint path it covers. Both
    // are defined with the gate now, since the gate has to know them.
    use surface::TRAILS;

    /// The one thing a source has to define, in the shape the template
    /// calls it by.
    const ENTRY: &str = "fn fs_user(uv: vec2<f32>) -> vec4<f32>";

    /// The shader pool is app-global and the tests below swap it out from
    /// under themselves, so anything that touches it takes this first.
    /// Same guard the surface module keeps over its own pool tests.
    static POOL_GUARD: Mutex<()> = Mutex::new(());

    fn config_with_routes() -> ShaderConfig {
        ShaderConfig {
            chrome: PanelChrome {
                title: Some("Wall".to_string()),
                locked: true,
                ..PanelChrome::default()
            },
            enabled: true,
            source: "// @slot 0: bass\nfn fs_user(uv: vec2<f32>) -> vec4<f32> \
                     { return vec4<f32>(params.signals[0].x); }"
                .to_string(),
            name: None,
            path: Some("/tmp/wall.wgsl".into()),
            routes: vec![
                Route {
                    enabled: true,
                    signal: 7,
                    target: surface::slot_target(0),
                    from: 0.0,
                    to: 1.0,
                },
                Route {
                    enabled: false,
                    signal: 9,
                    target: surface::slot_target(11),
                    from: 0.25,
                    to: 2.0,
                },
            ],
            manual: vec![(3, 0.5)],
            run_when_idle: true,
        }
    }

    #[test]
    fn config_round_trips_through_a_dump() {
        let config = config_with_routes();
        let dumped = serde_json::to_value(config.clone()).expect("dump");
        let read: ShaderConfig = serde_json::from_value(dumped).expect("read back");

        assert_eq!(read.chrome.title.as_deref(), Some("Wall"));
        assert!(read.chrome.locked);
        // The source is stored in the config, so a shader can travel
        // inside a workspace bundle.
        assert_eq!(read.source, config.source);
        assert_eq!(read.path, config.path);
        assert!(read.run_when_idle);
        assert_eq!(read.routes.len(), 2);
        assert_eq!(read.routes[0].target, "slot0");
        assert_eq!(read.routes[0].signal, 7);
        assert!(!read.routes[1].enabled);
        assert_eq!(read.routes[1].target, "slot11");
        assert_eq!(read.routes[1].to, 2.0);
        assert_eq!(read.manual, vec![(3, 0.5)]);
    }

    /// The switch defaults on, and a dump written before it existed reads
    /// back on rather than as a panel that silently stopped painting.
    #[test]
    fn a_config_without_the_switch_reads_as_on() {
        let mut dumped = serde_json::to_value(config_with_routes()).expect("dump");
        assert_eq!(dumped["enabled"], true);

        dumped
            .as_object_mut()
            .expect("object")
            .remove("enabled")
            .expect("the switch was written");
        let read: ShaderConfig = serde_json::from_value(dumped).expect("read back");
        assert!(read.enabled);
        // And the shader it held is still there to run.
        assert!(read.source.contains("fs_user"));
    }

    #[test]
    fn hand_set_values_hold_slots_no_route_feeds() {
        let mut manual = Vec::new();
        surface::set_manual_value(&mut manual, 3, 0.5);
        surface::set_manual_value(&mut manual, 0, 2.0);
        // A second write replaces, and typed values clamp to the slot's
        // 0..1.
        surface::set_manual_value(&mut manual, 3, 0.75);
        assert_eq!(surface::manual_value(&manual, 3), Some(0.75));
        assert_eq!(surface::manual_value(&manual, 0), Some(1.0));
        assert_eq!(surface::manual_value(&manual, 5), None);

        // Seeded under the routes: a live route writes over its slot, the
        // hand-set value holds the ones no route drives.
        let hub = rox_viz::signal::SignalHub::new(Vec::new());
        let routes = vec![Route {
            enabled: true,
            signal: 1,
            target: surface::slot_target(0),
            from: 0.0,
            to: 1.0,
        }];
        let mut targets = SlotTargets::default();
        surface::seed_manual(&mut targets, &manual);
        signal_ui::apply_routes(&routes, &hub, &mut targets);
        // The route's signal is gone from the pool, so it contributes
        // nothing and the seed is kept even on the routed slot.
        assert_eq!(targets.slots[0], 1.0);
        assert_eq!(targets.slots[3], 0.75);
        assert_eq!(targets.slots[5], 0.0);
    }

    /// A panel pointing into the workspace's pool writes the name, and one
    /// with a source of its own writes no key, so no layout dump written
    /// before the pool existed grows a line.
    #[test]
    fn a_pool_name_rides_the_panel_config() {
        let config = ShaderConfig {
            name: Some("Grain".to_string()),
            ..ShaderConfig::default()
        };
        let dumped = serde_json::to_value(config).expect("dump");
        assert_eq!(dumped["name"], "Grain");
        let read: ShaderConfig = serde_json::from_value(dumped).expect("read back");
        assert_eq!(read.name.as_deref(), Some("Grain"));

        let nameless = serde_json::to_value(ShaderConfig::default()).expect("dump");
        assert!(
            nameless.get("name").is_none(),
            "a panel with its own source writes no name: {nameless}"
        );
    }

    /// The panel runs what the pool holds under its name, and a name the
    /// pool doesn't hold runs nothing rather than the inline copy it still
    /// has. The gate reads the resolved text too, so a shader that
    /// arrived in a bundle can't slip past it behind an empty `source`.
    #[test]
    fn a_named_panel_runs_the_pools_copy() {
        let _pool = POOL_GUARD.lock().unwrap_or_else(|held| held.into_inner());
        let pool_source = format!("// from the pool\n{ENTRY} {{ return vec4<f32>(1.0); }}");
        rox_core::settings::note_shader_pool(vec![rox_core::settings::NamedShader {
            name: "Grain".to_string(),
            source: pool_source.clone(),
            path: None,
            assets: Vec::new(),
        }]);

        assert_eq!(
            surface::resolve_source(Some("Grain"), "// the panel's own"),
            Some(pool_source.clone())
        );
        assert!(
            !surface::approved(&pool_source),
            "a shader out of a bundle waits for this machine to agree"
        );

        // Nothing under that name is nothing to run, whatever the config
        // still has inline.
        rox_core::settings::note_shader_pool(Vec::new());
        assert_eq!(surface::resolve_source(Some("Grain"), &pool_source), None);
    }

    /// What a registration is kept under. A program's images can be
    /// replaced without a character of its source changing, so a key made
    /// of the text alone would leave a panel painting the plate it just
    /// swapped out. Where the source came from and the pool's generation
    /// are part of the key for that reason.
    #[test]
    fn the_program_key_moves_with_the_origin_and_the_pool() {
        let _pool = POOL_GUARD.lock().unwrap_or_else(|held| held.into_inner());
        let source = format!("// @asset plate: plate.png\n{ENTRY} {{ return vec4<f32>(1.0); }}");
        let named = surface::ProgramCtx::named("Grain");
        let key = program_hash(&source, &named, 0);
        assert_ne!(
            key,
            program_hash(&source, &surface::ProgramCtx::detached(), 0),
            "the same text out of the pool and out of a layout find their images in \
             different places, so they aren't the same program"
        );
        assert_ne!(
            key,
            program_hash(&source, &surface::ProgramCtx::file("/tmp/grain.wgsl"), 0)
        );
        assert_ne!(
            key,
            program_hash(&source, &named, 1),
            "a moved cover feed is a different program, or the art never follows the track"
        );

        // A new plate under the same name bumps the pool and edits no
        // source at all, which is the case the text alone can't see.
        rox_core::settings::note_shader_pool(vec![rox_core::settings::NamedShader {
            name: "Grain".to_string(),
            source: source.clone(),
            path: None,
            assets: Vec::new(),
        }]);
        assert_ne!(
            key,
            program_hash(&source, &named, 0),
            "a pool bump has to re-register, or an image hot reload never lands"
        );
        rox_core::settings::note_shader_pool(Vec::new());
    }

    /// The export scrub traverses layout dumps as raw JSON, well below the
    /// crate that knows what a panel config looks like, so it gets checked
    /// against a dump the real serialization produces rather than one
    /// written by hand to match it.
    #[test]
    fn the_export_scrub_finds_both_bookmarks_in_a_real_dump() {
        use rox_core::settings::{NamedLayout, WorkspaceBundle};
        use rox_dock::{PanelInfo, PanelState};

        // The Shader panel, whose own config is the shader.
        let shader_panel = ShaderConfig {
            source: "// the panel's own".to_string(),
            path: Some("/home/someone/panel.wgsl".into()),
            ..ShaderConfig::default()
        };
        // Any other panel, with a surface shader as chrome.
        let folder = crate::folder_tree::FolderTreeConfig {
            chrome: PanelChrome {
                shader: Some(surface::PanelShader {
                    source: "// the surface one".to_string(),
                    path: Some("/home/someone/surface.wgsl".into()),
                    ..surface::PanelShader::default()
                }),
                ..PanelChrome::default()
            },
            ..crate::folder_tree::FolderTreeConfig::default()
        };

        let dump = serde_json::to_value(PanelState {
            panel_name: "StackPanel".to_string(),
            children: vec![
                PanelState {
                    panel_name: "shader".to_string(),
                    children: Vec::new(),
                    info: PanelInfo::panel(serde_json::to_value(shader_panel).expect("dump")),
                },
                PanelState {
                    panel_name: "folder tree".to_string(),
                    children: Vec::new(),
                    info: PanelInfo::panel(serde_json::to_value(folder).expect("dump")),
                },
            ],
            info: PanelInfo::stack(Vec::new(), gpui::Axis::Vertical),
        })
        .expect("dump the dock state");
        // The bookmarks are really in there, or the assertions below would
        // pass over a dump shaped nothing like the walk expects.
        assert!(dump.to_string().contains("/home/someone/panel.wgsl"));
        assert!(dump.to_string().contains("/home/someone/surface.wgsl"));

        let mut bundle = WorkspaceBundle {
            layouts: vec![NamedLayout {
                name: "one".to_string(),
                dump,
                size: None,
            }],
            ..WorkspaceBundle::default()
        };
        bundle.scrub_paths();

        let scrubbed = bundle.layouts[0].dump.to_string();
        assert!(
            !scrubbed.contains("/home/someone/"),
            "no bookmark should have survived: {scrubbed}"
        );
        // The sources are the half that has to travel.
        assert!(scrubbed.contains("// the panel's own"));
        assert!(scrubbed.contains("// the surface one"));

        // And they read back as configs, with the bookmarks gone.
        let read: PanelState =
            serde_json::from_value(bundle.layouts[0].dump.clone()).expect("read the dock state");
        let PanelInfo::Panel(shader_config) = &read.children[0].info else {
            panic!("the shader panel's config should still be a panel dump");
        };
        let shader_config: ShaderConfig =
            serde_json::from_value(shader_config.clone()).expect("read the shader config");
        assert!(shader_config.path.is_none());
        assert_eq!(shader_config.source, "// the panel's own");

        let PanelInfo::Panel(folder_config) = &read.children[1].info else {
            panic!("the folder panel's config should still be a panel dump");
        };
        let folder_config: crate::folder_tree::FolderTreeConfig =
            serde_json::from_value(folder_config.clone()).expect("read the folder config");
        let worn = folder_config
            .chrome
            .shader
            .expect("the surface shader stays");
        assert!(worn.path.is_none());
        assert_eq!(worn.source, "// the surface one");
    }

    #[test]
    fn an_empty_dump_falls_back_to_the_preset() {
        // A panel added from the catalog dumps nothing of its own until it
        // is edited, and a config written before a field existed is the
        // same shape.
        let read: ShaderConfig = serde_json::from_value(serde_json::json!({})).expect("read");
        assert_eq!(read.source, PLASMA);
        assert!(read.path.is_none());
        assert!(read.routes.is_empty());
        assert!(read.manual.is_empty());
        assert!(!read.run_when_idle);
    }

    #[test]
    fn an_emptied_source_is_respected() {
        // Distinct from the case above: the key is there and empty, which
        // is a panel someone cleared rather than one that never had a
        // source. `serde(default)` fills only what's missing.
        let read: ShaderConfig =
            serde_json::from_value(serde_json::json!({ "source": "" })).expect("read");
        assert!(read.source.is_empty());
    }

    /// Registration composes the source into gpui's template, so a preset
    /// has to obey the contract's rules. The compose-and-validate path
    /// itself is inside the vendored crate and can't be reached from
    /// here; only the shape is checkable from this side.
    #[test]
    fn presets_are_shaped_like_the_contract() {
        for surface::Preset { label, source, .. } in PRESETS {
            assert!(
                source.contains(ENTRY),
                "{label} has to define the entry point the template calls"
            );
            // `meta` is a reserved word in naga 25, hence `user_meta` on
            // the WGSL side. A preset written against the Rust argument
            // name would be rejected at registration.
            assert!(
                !source.contains("params.meta"),
                "{label} reads params.meta; the WGSL field is user_meta"
            );
            for line in source.lines() {
                // Module scope is column zero here: everything the presets
                // declare of their own is a function.
                let declaration = !line.starts_with(char::is_whitespace);
                let binding = line.starts_with("var")
                    || line.starts_with("@group")
                    || line.starts_with("@binding");
                assert!(
                    !(declaration && binding),
                    "{label} declares a module-scope binding, which registration rejects: {line}"
                );
            }
        }
    }

    /// The picker groups the examples under a Scenes label and an Overlays
    /// label, so each side has to hold something or a heading sits over an
    /// empty run.
    ///
    /// The split is also the app's one guard against handing a whole window
    /// to something that hides it, so the two shapes that claim it are
    /// pinned by name here: Sheen leaves the frame visible by being
    /// transparent, Tube by reading `screen` and printing it back. A preset
    /// that quietly stopped declaring itself would move into Scenes and
    /// warn about a coverage it doesn't cause.
    #[test]
    fn the_examples_offer_scenes_and_overlays() {
        let named = |label: &str| {
            PRESETS
                .iter()
                .find(|preset| preset.label == label)
                .unwrap_or_else(|| panic!("{label} should be a shipped example"))
        };
        for label in ["Sheen", "Badge", "Lamp", "Cube", "Tube"] {
            assert!(
                surface::overlay(named(label).source),
                "{label} leaves the surface under it usable, so it has to say so"
            );
        }
        for label in ["Plasma", "Trails", "Cover", "Bloom"] {
            assert!(
                !surface::overlay(named(label).source),
                "{label} covers what's under it and mustn't read as an overlay"
            );
        }
    }

    /// The directive reads like the others: a bare marker, and prose that
    /// merely starts with the same letters isn't one.
    #[test]
    fn the_overlay_directive_isnt_fooled_by_prose() {
        assert!(surface::overlay("// @overlay\nfn fs_user() {}"));
        assert!(surface::overlay("  // @overlay  "));
        assert!(!surface::overlay("// @overlayed the whole window"));
        assert!(!surface::overlay("// this one is an overlay, honest"));
        assert!(!surface::overlay(""));
    }

    /// One of each kind, which is the point of shipping two: the pure one
    /// exercises the in-scene quad and the other the region pass. The fork
    /// itself is `screen_pass_only`, so the caps each preset's shape earns
    /// are checked against it here.
    #[test]
    fn the_presets_cover_both_paint_paths() {
        assert!(
            !PLASMA.contains("textureSample"),
            "plasma has to stay pure, or it loses the primitive path"
        );
        assert!(
            TRAILS.contains("prev"),
            "trails has to read prev, or it never reaches the region pass"
        );

        let pure = gpui::UserShaderCaps {
            samples_screen: false,
            uses_prev: false,
            multi_pass: false,
            uses_assets: false,
            uses_mask: false,
        };
        assert!(
            !pure.screen_pass_only(),
            "a shader drawing from its uniforms alone stays an in-scene quad"
        );
        assert!(gpui::UserShaderCaps {
            uses_prev: true,
            ..pure
        }
        .screen_pass_only());
    }

    /// The fork is what registration made of the program, not what the
    /// text looks like. A chain that reads neither the screen nor its own
    /// last frame still needs the region pass, since intermediate targets
    /// and image bindings only exist there, and a shader panel taking the
    /// quad path for one of those paints nothing at all.
    #[test]
    fn a_multi_pass_program_takes_the_screen_path() {
        let source = format!("// @pass half: 0.5\n{PLASMA}\n// @pass out\n{PLASMA}");
        let spec = surface::parse_chain(&source).expect("two passes");
        assert_eq!(spec.passes.len(), 2);
        assert!(
            !spec.plain(),
            "a multi-pass text can't take the single-source registration"
        );

        let quad = gpui::UserShaderCaps {
            samples_screen: false,
            uses_prev: false,
            multi_pass: false,
            uses_assets: false,
            uses_mask: false,
        };
        assert!(gpui::UserShaderCaps {
            multi_pass: true,
            ..quad
        }
        .screen_pass_only());
        assert!(gpui::UserShaderCaps {
            uses_assets: true,
            ..quad
        }
        .screen_pass_only());
    }

    /// The `// @slot n: name` convention gives the Bindings page something
    /// to call a slot, so a preset that ships without names is a preset
    /// nobody can read the bindings of.
    #[test]
    fn presets_name_their_slots() {
        for surface::Preset { label, source, .. } in PRESETS {
            let labels = surface::slot_labels(source);
            assert!(
                labels[0].is_some(),
                "{label} should name at least its first slot"
            );
            let named = labels.iter().filter(|name| name.is_some()).count();
            assert!(named >= 4, "{label} names only {named} slots");
        }
    }

    /// Where a slot maps into the uniform block, the mapping the Bindings
    /// page prints beside each row.
    #[test]
    fn slot_accessors_walk_the_uniform_block() {
        assert_eq!(surface::slot_accessor(0), "params.signals[0].x");
        assert_eq!(surface::slot_accessor(3), "params.signals[0].w");
        assert_eq!(surface::slot_accessor(4), "params.signals[1].x");
        assert_eq!(
            surface::slot_accessor(surface::SLOTS - 1),
            "params.signals[3].w"
        );
    }
}
