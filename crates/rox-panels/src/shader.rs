//! The shader panel: a WGSL fragment stage that owns a panel's whole body,
//! driven by the app's shared signal pool. The author writes
//! `fs_user(uv)`; rox fills the sixteen signal slots from routes and the
//! eight `user_meta` floats from the player.
//!
//! A shader reading only its uniforms draws as an in-scene quad; one that
//! reads `screen`, `prev`, an image, or runs several passes needs the region
//! pass, keyed by this panel's entity id. Getting the path wrong paints
//! nothing.
//!
//! Distinct from [`crate::panel::shader`], the surface shader any panel can
//! wear, which owns the pieces both share.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gpui::{
    AnyElement, App, Context, Div, EntityId, EventEmitter, FocusHandle, Focusable,
    PathPromptOptions, SharedString, Subscription, UserShaderId, WeakEntity, Window, canvas, div,
    prelude::*, px,
};
use gpui_component::menu::PopupMenu;
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use rox_viz::signal::Route;

use crate::assets::icons;
use crate::design::{palette, tokens};
// Aliased: this file is `panels::shader` and that one `panel::shader`.
use crate::panel::shader::{self as surface, SlotTargets, SourceWatch};
use crate::panel::{
    self, AppState, PanelChrome, PanelSettings, ScrubState, ValueEdit, setting_row, toggle,
};
use crate::panel_settings;
use crate::settings::ui::{self as settings_ui, SECTION_GAP, section};
use crate::signal_ui::{self, routes::RouteEditState};

/// Defined beside the approval gate, which has to know them: what ships
/// with the binary runs without a second agreement.
use surface::{PLASMA, PRESETS};

/// naga's caret line is the useful part; the rest would fill a small
/// panel.
const ERROR_LINES: usize = 8;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ShaderConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    /// Off keeps the source, bindings, and bookmark and paints nothing: how
    /// saying no to an unread shader parks it without throwing it away.
    pub enabled: bool,
    /// Stored inline so a shader travels inside a workspace bundle.
    pub source: String,
    /// A name in the workspace's shader pool; set, the pool's copy runs. A
    /// name the pool doesn't hold runs nothing, never the inline text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A bookmark for Reload and the file watch, never the thing that runs.
    pub path: Option<PathBuf>,
    /// A route whose signal is gone leaves its slot at zero.
    pub routes: Vec<Route>,
    /// What a slot reads with no route driving it. A route on the same slot
    /// wins while it's there.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub manual: Vec<(u8, f32)>,
    /// Off, the shader parks while the audio is silent.
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

/// Every state that paints nothing goes through here, so the panel reads
/// as waiting on a decision rather than a black rectangle.
struct BodyNote {
    lines: Vec<String>,
    actions: Vec<NoteAction>,
    /// A compiler message: left aligned at the top, where its carets line up.
    raw: bool,
}

#[derive(Clone, Copy)]
enum NoteAction {
    /// The Source settings page, with the full source, its origin, and hash.
    Inspect,
    /// The approval an imported source waits on, the switch, or both.
    Enable,
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

/// Shared with the paint closure, where registration happens: it needs
/// the window.
#[derive(Default)]
struct Compiled {
    /// Re-register only when this moves. See [`program_hash`]: images can
    /// change under unchanged text.
    key: u64,
    /// Tells a fresh panel from one whose shader hashes to zero.
    ran: bool,
    /// The last good registration stays up while a fresh edit is broken; an
    /// authoring loop saves half-written files constantly.
    shader: Option<UserShaderId>,
    error: Option<String>,
}

/// Cached because resolving takes a lock and copies a page of WGSL on a
/// panel that re-renders every audio frame; the generation is one atomic
/// load.
#[derive(Default)]
struct Resolved {
    name: String,
    rev: u64,
    source: Option<String>,
    ran: bool,
}

/// Includes the pool generation: replacing an image changes no source
/// text, and the pool watch bumps the generation. [`surface`]'s driver
/// keys the same way.
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
    compiled: Arc<Mutex<Compiled>>,
    /// A cell because every reader of the running source is a `&self` render
    /// path.
    resolved: RefCell<Resolved>,
    /// The same watch a panel's surface shader uses.
    watch: SourceWatch,
    /// Not config: which rows stand open is where you are in the page.
    routes_ui: RouteEditState,
    slot_scrubs: Vec<ScrubState>,
    shader_name: panel_settings::ShaderNameField,
    value_edit: ValueEdit,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    _player_changed: Subscription,
}

impl ShaderPanel {
    pub fn new(state: AppState, config: ShaderConfig, cx: &mut Context<Self>) -> Self {
        let _player_changed = cx.observe(&state.player, |_, _, cx| cx.notify());
        ShaderPanel {
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

    /// Runs off the render, so a parked panel reloads on the button instead.
    /// Never reload a source still waiting on approval: the path came with it,
    /// and reading a file a bundle chose trusts the bundle by the back door.
    fn poll_reload(&mut self, cx: &mut Context<Self>) {
        // The pool's watch is throttled and app-wide, so several panels cost one
        // sweep.
        surface::poll_pool();
        let Some(path) = self.config.path.clone() else {
            return;
        };
        // A named panel doesn't watch a file: the bookmark points at the old
        // inline text and would pull it over the pool's source.
        if self.config.name.is_some() {
            return;
        }
        if self.pending() {
            return;
        }
        if let Some(source) = self.watch.poll(&path)
            && source != self.config.source
        {
            self.set_source(source, Some(path), cx);
        }
    }

    /// Every caller is the user choosing the source, so this is where a
    /// source earns its approval. The last registration stays up until the
    /// new source compiles. Choosing a source also takes the panel off a pool
    /// name.
    fn set_source(&mut self, source: String, path: Option<PathBuf>, cx: &mut Context<Self>) {
        surface::approve(&source);
        let cleared = source.trim().is_empty();
        // Picking a source is asking to see it, even after an earlier Turn Off.
        self.config.enabled = true;
        self.config.source = source;
        self.config.name = None;
        self.config.path = path.clone();
        self.watch = SourceWatch::seeded(path.as_deref());
        {
            let mut compiled = self.compiled.lock().unwrap();
            // A cleared source leaves nothing to keep on screen.
            let keep = if cleared { None } else { compiled.shader };
            *compiled = Compiled {
                shader: keep,
                ..Compiled::default()
            };
        }
        cx.notify();
    }

    /// The pool's copy when the config names one, its inline source
    /// otherwise. The settings pages keep editing `config.source`, which a
    /// named panel holds for when the name comes off.
    fn running(&self) -> String {
        self.resolved().unwrap_or_default()
    }

    /// None when the config names a shader the pool doesn't hold, which the
    /// body reports separately from an empty source.
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

    fn pool_missing(&self) -> bool {
        self.resolved().is_none()
    }

    /// Asked of what runs rather than of the config, so a pool shader goes
    /// through the same gate as an inline one.
    fn pending(&self) -> bool {
        !surface::approved(&self.running())
    }

    /// The one path that approves without a file or preset. The path goes:
    /// it named a file on the bundle author's machine, and a file at that path
    /// here would get pulled over the approved text. Approving also turns the
    /// panel on.
    fn approve(&mut self, cx: &mut Context<Self>) {
        // Approve the text the pool holds, not the inline copy.
        surface::approve(&self.running());
        self.config.enabled = true;
        self.config.path = None;
        self.watch = SourceWatch::default();
        *self.compiled.lock().unwrap() = Compiled::default();
        cx.notify();
    }

    /// An approved local file keeps hot reloading through the switch; only
    /// the approval drops a bundle's path.
    fn enable(&mut self, cx: &mut Context<Self>) {
        if self.pending() {
            self.approve(cx);
        } else {
            self.config.enabled = true;
            cx.notify();
        }
    }

    /// Parks the pending source rather than deleting it.
    fn turn_off(&mut self, cx: &mut Context<Self>) {
        self.config.enabled = false;
        cx.notify();
    }

    /// rox has no editor of its own, so this plus the file watch is the
    /// authoring loop. A named shader ejects through its pool entry.
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
                    // Seeded, so only the next edit wakes the watch.
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

    /// A named panel edits the pool entry, so every panel on the name
    /// follows.
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

    /// Unlike [`set_source`](Self::set_source) the bookmark stays: the editor
    /// wrote the file from this same text. The approval already happened.
    fn apply_edit(&mut self, source: String, cx: &mut Context<Self>) {
        self.config.enabled = true;
        self.config.source = source;
        self.watch = SourceWatch::seeded(self.config.path.as_deref());
        cx.notify();
    }

    /// The running text keeps its approval. No bookmark comes across: the
    /// pool entry's file belongs to the pool.
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

    /// Clears the inline source and bookmark, since the pool holds what runs.
    /// Nothing is approved on the way: a bundle's pool shader still has to be
    /// read first.
    fn use_pool_name(&mut self, name: String, cx: &mut Context<Self>) {
        self.config.enabled = true;
        self.config.name = Some(name);
        self.config.source = String::new();
        self.config.path = None;
        self.watch = SourceWatch::default();
        *self.compiled.lock().unwrap() = Compiled::default();
        cx.notify();
    }

    /// The inline copy goes, since the pool holds the source now.
    fn save_to_pool(&mut self, name: String, cx: &mut Context<Self>) {
        let name = name.trim().to_string();
        if name.is_empty() || self.config.source.trim().is_empty() {
            return;
        }
        // The bookmark goes with it, so a file being edited keeps hot reloading
        // through the pool's watch.
        surface::save_to_pool(&name, &self.config.source, self.config.path.clone());
        self.config.name = Some(name);
        self.config.source = String::new();
        self.config.path = None;
        self.watch = SourceWatch::default();
        // The registration stands: the pool holds the same text.
        cx.notify();
    }

    /// A file that won't read shows in the compile readout.
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

    fn reload(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.config.path.clone() {
            self.load_file(path, cx);
        }
    }

    /// The path goes: an example has no file, and the old one would be
    /// reloaded over it.
    fn use_preset(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(preset) = PRESETS.get(index) {
            self.set_source(preset.source.to_string(), None, cx);
        }
    }

    fn body_note(&self) -> Option<BodyNote> {
        let note = |lines: Vec<String>, actions: Vec<NoteAction>| {
            Some(BodyNote {
                lines,
                actions,
                raw: false,
            })
        };
        // Nothing else in the app would say why a missing pool name paints
        // nothing.
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
        // A backend with no shader pipeline gets the plain note, not a compiler
        // readout.
        if surface::unsupported(&error) {
            return note(
                vec![
                    format!("{}.", surface::NO_PIPELINE_TITLE),
                    surface::NO_PIPELINE_NOTE.to_string(),
                ],
                vec![NoteAction::Inspect],
            );
        }
        // Keep naga's lines left aligned: centred, the carets point at the wrong
        // columns.
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

    // The body already is a shader.
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
        // window.
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
    fn source_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let path = self.config.path.clone();
        let named = self.config.name.clone();
        // What runs, so a bundle's pool shader gets read before it runs.
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

        // Read before the field goes out on loan to the picker block.
        let fallback = {
            let label = self.config.chrome.title.clone().unwrap_or_default();
            surface::eject_name(&label, &self.config.source)
        };
        let picked = panel_settings::ShaderSource {
            id: "shader-panel",
            name: named.as_deref(),
            path: path.as_deref(),
            resolved: resolved.as_deref(),
            // No None entry: this panel's whole body is the shader.
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
                // Just the switch; an unread source still has the approval block above.
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
            // A callout, since the switch above reads as on while nothing runs.
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

    fn bindings_page(&mut self, cx: &mut Context<Self>) -> Div {
        // Off what runs, so a pool shader's slot names show.
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
        let focus = self.focus.clone();
        panel::themed(&chrome, || self.body(cx).track_focus(&focus))
    }
}

impl ShaderPanel {
    fn body(&mut self, cx: &mut Context<Self>) -> Div {
        self.poll_reload(cx);
        let note = self.body_note().map(|note| self.note_overlay(note, cx));

        // Off or unread never registers; the note says why.
        let source = if self.pending() || !self.config.enabled {
            String::new()
        } else {
            self.running()
        };
        // A named panel holds no path; the pool entry keeps its own.
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
                // Deferred: opening reads the panel, and this handler runs inside the
                // panel's own update.
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
            // Wraps rather than clipping, since the block is centred.
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
    compiled: &Mutex<Compiled>,
    panel: EntityId,
) {
    if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) || source.trim().is_empty() {
        return;
    }
    // A program using the cover re-registers when the poll moves the cover
    // feed.
    let cover = if surface::uses_cover(source) {
        surface::poll_cover(window, cx)
    } else {
        0
    };
    let hash = program_hash(source, ctx, cover);
    let shader = {
        let mut compiled = compiled.lock().unwrap();
        if !compiled.ran || compiled.key != hash {
            // Registration only caches what compiled, and this runs every frame, so
            // a broken program is tried once and kept until something moves.
            let previous = compiled.error.take();
            // The last good shader stays on screen through a failed compile.
            let good = compiled.shader;
            // Images are read here too, so a bad plate reads out like a bad line of
            // WGSL.
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
                // The body was built before this ran; without the nudge a broken shader
                // asks for no frames and the message never shows.
                cx.notify(panel);
            }
        }
        compiled.shader
    };
    let Some(shader) = shader else {
        return;
    };

    let mut targets = SlotTargets::default();
    surface::seed_manual(&mut targets, manual);
    // Reading advances the hub, deduped to once per frame.
    signal_ui::apply_routes(routes, hub, &mut targets);
    let meta = surface::meta_slots(window, cx);
    // A pointer-reading shader keeps asking for frames while presence eases
    // off, and the watch wakes it when the hand comes back.
    let cursor = surface::reads_cursor(source);
    if cursor {
        surface::watch_cursor(window);
    }

    // A program that reads the screen, its last frame, an image, or has
    // several passes needs the region pass. Backwards paints nothing.
    let screen = window
        .user_shader_caps(shader)
        .is_some_and(|caps| caps.screen_pass_only());
    if screen {
        // The entity id keys the feedback texture, so two panels on one source
        // each keep their own.
        window.paint_screen_shader(bounds, shader, panel.as_u64(), targets.slots, meta);
    } else {
        window.paint_user_shader(bounds, shader, targets.slots, meta);
    }

    // A docked panel renders cached, so an animating shader needs this view
    // dirtied every frame; `request_animation_frame` is the cheap wake.
    // Settling counts too, or the release's fade would stop halfway.
    if hub.live() || hub.settling() || run_when_idle || (cursor && meta[6] > 0.0) {
        window.request_animation_frame();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surface::TRAILS;

    const ENTRY: &str = "fn fs_user(uv: vec2<f32>) -> vec4<f32>";

    /// The pool is app-global and these tests swap it, so anything touching
    /// it takes this first.
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
        // Stored in the config, so a shader travels inside a bundle.
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
        assert!(read.source.contains("fs_user"));
    }

    #[test]
    fn hand_set_values_hold_slots_no_route_feeds() {
        let mut manual = Vec::new();
        surface::set_manual_value(&mut manual, 3, 0.5);
        surface::set_manual_value(&mut manual, 0, 2.0);
        // A second write replaces, and typed values clamp to 0..1.
        surface::set_manual_value(&mut manual, 3, 0.75);
        assert_eq!(surface::manual_value(&manual, 3), Some(0.75));
        assert_eq!(surface::manual_value(&manual, 0), Some(1.0));
        assert_eq!(surface::manual_value(&manual, 5), None);

        // Seeded under the routes: a live route writes over its slot.
        let hub =
            rox_viz::signal::SignalHub::with_feed(Vec::new(), Arc::new(rox_viz::AudioFeed::new()));
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
        // The route's signal is gone, so the seed stays.
        assert_eq!(targets.slots[0], 1.0);
        assert_eq!(targets.slots[3], 0.75);
        assert_eq!(targets.slots[5], 0.0);
    }

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

    /// The gate reads the resolved text, so a bundle's shader can't slip
    /// past it behind an empty `source`.
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

        // Nothing under that name runs nothing, whatever is inline.
        rox_core::settings::note_shader_pool(Vec::new());
        assert_eq!(surface::resolve_source(Some("Grain"), &pool_source), None);
    }

    /// Images can be replaced without a character of source changing.
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

        // A new plate bumps the pool and edits no source.
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

    /// Checked against a real dump, since the scrub walks raw JSON far from
    /// the types.
    #[test]
    fn the_export_scrub_finds_both_bookmarks_in_a_real_dump() {
        use rox_core::settings::{NamedLayout, WorkspaceBundle};
        use rox_dock::{PanelInfo, PanelState};

        let shader_panel = ShaderConfig {
            source: "// the panel's own".to_string(),
            path: Some("/home/someone/panel.wgsl".into()),
            ..ShaderConfig::default()
        };
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
        // Guard that the fixture really holds the bookmarks.
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
        assert!(scrubbed.contains("// the panel's own"));
        assert!(scrubbed.contains("// the surface one"));

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
        // A panel added from the catalog dumps nothing until edited.
        let read: ShaderConfig = serde_json::from_value(serde_json::json!({})).expect("read");
        assert_eq!(read.source, PLASMA);
        assert!(read.path.is_none());
        assert!(read.routes.is_empty());
        assert!(read.manual.is_empty());
        assert!(!read.run_when_idle);
    }

    #[test]
    fn an_emptied_source_is_respected() {
        // The key present and empty is a cleared panel; `serde(default)` fills
        // only what's missing.
        let read: ShaderConfig =
            serde_json::from_value(serde_json::json!({ "source": "" })).expect("read");
        assert!(read.source.is_empty());
    }

    /// The compose-and-validate path lives in the vendored crate, so only the
    /// shape is checkable here.
    #[test]
    fn presets_are_shaped_like_the_contract() {
        for surface::Preset { label, source, .. } in PRESETS {
            assert!(
                source.contains(ENTRY),
                "{label} has to define the entry point the template calls"
            );
            // `meta` is a reserved word in naga 25, hence `user_meta` in WGSL.
            assert!(
                !source.contains("params.meta"),
                "{label} reads params.meta; the WGSL field is user_meta"
            );
            for line in source.lines() {
                // Module scope is column zero: the presets declare only functions.
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

    /// Each group needs something, or a heading sits over an empty run. The
    /// split is also the guard against a preset hiding a whole window, so
    /// the overlay shapes are pinned by name.
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

    #[test]
    fn the_overlay_directive_isnt_fooled_by_prose() {
        assert!(surface::overlay("// @overlay\nfn fs_user() {}"));
        assert!(surface::overlay("  // @overlay  "));
        assert!(!surface::overlay("// @overlayed the whole window"));
        assert!(!surface::overlay("// this one is an overlay, honest"));
        assert!(!surface::overlay(""));
    }

    /// The pure one exercises the in-scene quad, the other the region pass.
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
        assert!(
            gpui::UserShaderCaps {
                uses_prev: true,
                ..pure
            }
            .screen_pass_only()
        );
    }

    /// A chain that reads neither the screen nor its last frame still needs
    /// the region pass: intermediate targets and images only exist there.
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
        assert!(
            gpui::UserShaderCaps {
                multi_pass: true,
                ..quad
            }
            .screen_pass_only()
        );
        assert!(
            gpui::UserShaderCaps {
                uses_assets: true,
                ..quad
            }
            .screen_pass_only()
        );
    }

    /// A preset without slot names leaves the Bindings page unreadable.
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
