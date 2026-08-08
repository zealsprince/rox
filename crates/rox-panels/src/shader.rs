//! The shader panel: a WGSL fragment stage that owns a panel's whole body,
//! driven by the app's shared signal pool. The author writes one function,
//! `fs_user(uv)`, against the uniform block gpui binds; rox fills the
//! sixteen signal slots from routes and the eight `user_meta` floats from
//! the player, so an unrouted shader still moves with the music.
//!
//! Two paint paths, picked by what the source turns out to reference. A
//! shader reading nothing but its uniforms draws as an in-scene quad. One
//! reading `screen` (what sits under the panel) or `prev` (its own last
//! frame) needs the region pass, keyed by this panel's entity id so two
//! shader panels each get their own feedback texture. Registration works
//! out which; getting it wrong paints nothing, since each call skips what
//! it can't run.
//!
//! Distinct from [`crate::panel::shader`], which is the surface shader any
//! panel can wear over its own body. That module owns the pieces both
//! share: the slot targets, the `// @slot n: name` convention, and the meta
//! floats. This one is the panel whose entire point is the shader.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gpui::{
    canvas, div, prelude::*, px, svg, AnyElement, App, Context, Div, EntityId, EventEmitter,
    FocusHandle, Focusable, PathPromptOptions, SharedString, Subscription, UserShaderId,
    WeakEntity, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
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
use crate::signal_ui::{self, routes::RouteEditState, RouteTargets};

/// The builtin shaders, so a fresh panel draws something before anyone has
/// written a line of WGSL. They live beside the surface shader's pieces
/// because the approval gate has to know them: what ships with the binary
/// runs without anybody agreeing to it a second time.
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
    /// The fragment stage itself, stored inline. This is what makes a
    /// shader ride a workspace bundle: a config carrying only an absolute
    /// path would import as a dead panel on anyone else's machine.
    pub source: String,
    /// Where the source was last read from. A bookmark for Reload and the
    /// file watch, never the thing that runs.
    pub path: Option<PathBuf>,
    /// Attachments of the app's shared signals onto the shader's slots. A
    /// route whose signal is gone from the pool leaves its slot at zero.
    pub routes: Vec<Route>,
    /// Hand-set slot values, from the Bindings page's slot rows: what a
    /// slot reads with no route feeding it, which is how a shader's named
    /// parameters get tweaked without a signal in sight. A route on the
    /// same slot wins while it's there; the hand-set value comes back when
    /// it goes.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub manual: Vec<(u8, f32)>,
    /// Keep asking for frames while the audio is silent. Off, the shader
    /// parks where it stands and the panel costs nothing, which is the
    /// freeze-on-pause value the other visualizers hold.
    pub run_when_idle: bool,
}

impl Default for ShaderConfig {
    fn default() -> Self {
        ShaderConfig {
            chrome: PanelChrome::default(),
            source: PLASMA.to_string(),
            path: None,
            routes: Vec::new(),
            manual: Vec::new(),
            run_when_idle: false,
        }
    }
}

/// The hand-set value a slot holds, if one was set. The page reads slots
/// through the resolver, which sees these as seeds, so only the tests ask
/// directly.
#[cfg(test)]
fn manual_value(manual: &[(u8, f32)], slot: usize) -> Option<f32> {
    manual
        .iter()
        .find(|(at, _)| *at as usize == slot)
        .map(|(_, value)| *value)
}

/// Set or replace a slot's hand-set value.
fn set_manual_value(manual: &mut Vec<(u8, f32)>, slot: usize, value: f32) {
    let value = value.clamp(0.0, 1.0);
    match manual.iter_mut().find(|(at, _)| *at as usize == slot) {
        Some(entry) => entry.1 = value,
        None => manual.push((slot as u8, value)),
    }
}

/// Lay the hand-set values into the slots before the routes resolve over
/// them: a route wins while it's there, and the hand-set value holds the
/// slot when it isn't.
fn seed_manual(targets: &mut SlotTargets, manual: &[(u8, f32)]) {
    for (slot, value) in manual {
        if let Some(entry) = targets.slots.get_mut(*slot as usize) {
            *entry = value.clamp(0.0, 1.0);
        }
    }
}

/// What the last registration made of the current source. Shared with the
/// paint closure, which is where registration happens: it needs the window,
/// and the panel only has one while it's drawing.
#[derive(Default)]
struct Compiled {
    /// The source this ran against, so a change re-registers and nothing
    /// else does.
    source: u64,
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

fn source_hash(source: &str) -> u64 {
    use std::hash::{Hash as _, Hasher as _};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    hasher.finish()
}

pub struct ShaderPanel {
    state: AppState,
    config: ShaderConfig,
    feed: Arc<AudioFeed>,
    compiled: Arc<Mutex<Compiled>>,
    /// The hot-reload watch on the config's path, the same one a panel's
    /// surface shader wears.
    watch: SourceWatch,
    /// The Bindings page's route editor state: span sliders and which rows
    /// stand open. Not config - the fold is where you are in the page.
    routes_ui: RouteEditState,
    /// One scrub per slot row, for the hand-set values on unrouted slots.
    slot_scrubs: Vec<ScrubState>,
    /// The one readout being typed into across all the settings sliders.
    value_edit: ValueEdit,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and
    /// pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// Wakes the panel on every pump tick, so the shader's frames ride the
    /// same clock the audio arrives on.
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
            watch: SourceWatch::default(),
            routes_ui: RouteEditState::default(),
            slot_scrubs: (0..surface::SLOTS).map(|_| ScrubState::default()).collect(),
            value_edit: ValueEdit::default(),
            focus: cx.focus_handle(),
            tab_panel: None,
            _player_changed,
        }
    }

    /// Pick up edits to the file the source came from. This rides the
    /// render, which the player's pump drives, so the watch runs while
    /// there is anything to watch it for; a parked panel reloads on the
    /// button instead.
    ///
    /// A source still waiting on approval doesn't reload: the path arrived
    /// with it, and reading a file a bundle chose would be trusting the
    /// bundle by the back door.
    fn poll_reload(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.config.path.clone() else {
            return;
        };
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
    /// Every caller is the user putting the source there - a preset, a file
    /// they picked, a reload, an edit under a file they pointed rox at - so
    /// this is where a source earns its approval.
    fn set_source(&mut self, source: String, path: Option<PathBuf>, cx: &mut Context<Self>) {
        surface::approve(&source);
        let cleared = source.trim().is_empty();
        self.config.source = source;
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

    /// Whether the source is waiting on approval: it arrived inside a
    /// layout or a workspace bundle and nobody on this machine has agreed
    /// to run it yet.
    fn pending(&self) -> bool {
        !surface::approved(&self.config.source)
    }

    /// Agree to run what the config carries. The one button that puts a
    /// hash in the approved list without the source having come from a file
    /// or a preset.
    ///
    /// The path goes: it named a file on whichever machine wrote the
    /// bundle, and if this one happens to have something at that path, the
    /// watch would pull it straight over the text just approved. Picking a
    /// file again is how an imported shader gets a local one.
    fn approve(&mut self, cx: &mut Context<Self>) {
        surface::approve(&self.config.source);
        self.config.path = None;
        self.watch = SourceWatch::default();
        *self.compiled.lock().unwrap() = Compiled::default();
        cx.notify();
    }

    /// Throw the pending source away. The path goes with it: it points at
    /// whatever the bundle pointed at, and keeping it would leave Reload
    /// aimed there.
    fn discard(&mut self, cx: &mut Context<Self>) {
        self.set_source(String::new(), None, cx);
    }

    /// Snapshot a file into the panel's source. A file that won't read
    /// lands in the same readout a failed compile does, since from the
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
    /// caught yet or a panel that has been sitting parked.
    fn reload(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.config.path.clone() {
            self.load_file(path, cx);
        }
    }

    /// Load a builtin. The path goes with it: a preset has no file behind
    /// it, and leaving the old one recorded would have the watch overwrite
    /// the preset a second later.
    fn use_preset(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some((_, source)) = PRESETS.get(index) {
            self.set_source((*source).to_string(), None, cx);
        }
    }

    /// What the panel says instead of running: nothing loaded, or what
    /// registration made of what is. None while the shader draws.
    fn body_note(&self) -> Option<Vec<String>> {
        if self.config.source.trim().is_empty() {
            return Some(vec![
                "No shader loaded.".to_string(),
                "Pick a preset or a .wgsl file on this panel's Source settings page.".to_string(),
            ]);
        }
        if self.pending() {
            return Some(vec![
                "This shader is awaiting approval.".to_string(),
                "It arrived with a layout or a workspace rather than from this machine, \
                 so it doesn't run until you've read it."
                    .to_string(),
                "Read it and approve it on this panel's Source settings page.".to_string(),
            ]);
        }
        let error = self.compiled.lock().unwrap().error.clone()?;
        // naga's message runs several lines, with a caret under the span it
        // is complaining about. They have to stay lines: one text element
        // holding the whole string would run them together.
        Some(
            std::iter::once("This shader didn't compile:".to_string())
                .chain(error.lines().take(ERROR_LINES).map(str::to_string))
                .collect(),
        )
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
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match page {
            "Bindings" => self.bindings_page(cx).into_any_element(),
            _ => self.source_page(cx).into_any_element(),
        }
    }
}

impl ShaderPanel {
    /// The Source page: where the shader comes from, what it said about
    /// itself, and the conventions it can count on.
    fn source_page(&mut self, cx: &mut Context<Self>) -> Div {
        let source = self.config.source.clone();
        let path = self.config.path.clone();
        let error = self.compiled.lock().unwrap().error.clone();
        let run_when_idle = self.config.run_when_idle;
        let pending = self.pending().then(|| {
            panel_settings::pending_shader(
                "shader-panel-pending",
                &source,
                path.as_deref(),
                cx.listener(|this, _, _, cx| this.approve(cx)),
                cx.listener(|this, _, _, cx| this.discard(cx)),
            )
        });

        let mut presets = div().flex().flex_row().flex_wrap().gap(px(1.));
        for (index, (label, preset)) in PRESETS.iter().enumerate() {
            presets = presets.child(signal_ui::scope_chip(
                (*label).to_string(),
                source == **preset,
                move |this: &mut Self, cx| this.use_preset(index, cx),
                cx,
            ));
        }

        let file = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .child(settings_ui::small_button(
                "Reload",
                icons::REFRESH_CW,
                path.is_none(),
                cx.listener(|this, _, _, cx| this.reload(cx)),
            ))
            .child(settings_ui::small_button(
                "Choose File",
                icons::FOLDER,
                false,
                cx.listener(|this, _, window, cx| this.pick_file(window, cx)),
            ));
        let note: SharedString = match (&path, source.trim().is_empty()) {
            (Some(path), _) => format!(
                "{}. Edits reload while the panel is drawing, and the source is copied \
                 into the layout, so the panel keeps its shader on a machine that never \
                 had the file",
                path.display()
            )
            .into(),
            (None, true) => "Pick a preset above, or a .wgsl file with a fragment stage \
                             defining fs_user(uv)"
                .into(),
            (None, false) => {
                "Running a source that rides the layout, with no file behind it".into()
            }
        };

        let mut shader = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(panel::setting_block(
                "Preset",
                Some(
                    "The builtins. Plasma draws from its uniforms alone; Trails reads its \
                     own last frame, which puts it on the screen pass",
                ),
                None,
                presets,
            ))
            .child(panel::setting_row_dyn("Shader File", Some(note), file));
        if let Some(error) = error {
            shader = shader.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .text_xs()
                    .text_color(palette::text_muted())
                    .children(error.lines().take(ERROR_LINES).map(str::to_string)),
            );
        }
        shader = shader.child(setting_row(
            "Run When Idle",
            Some(
                "Keep drawing while the audio is silent. Off, the shader parks where it \
                 stands and the panel costs nothing",
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
            .children(pending.map(|body| section("Awaiting Approval", None, body)))
            .child(section("Shader", None, shader))
            .child(section("Writing One", None, conventions()))
    }

    /// The Bindings page: the routes filling the shader's slots, in the
    /// same editor the panel Shader page and the app's screen shader wear,
    /// over a live readout of all sixteen slots. The names come off the
    /// source's `// @slot n: name` comments where it declares them.
    fn bindings_page(&mut self, cx: &mut Context<Self>) -> Div {
        let labels = surface::slot_labels(&self.config.source);
        // This frame's resolved values, so the readout says what is
        // actually reaching the shader rather than what was set.
        let mut resolved = SlotTargets::default();
        seed_manual(&mut resolved, &self.config.manual);
        signal_ui::apply_routes(&self.config.routes, &self.state.signals, &mut resolved);
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

        // Every slot the shader can read is worth showing whether anything
        // feeds it or not - that's what says where a value lands in the
        // WGSL. A slot a route feeds shows the live value it's getting; one
        // nothing feeds is a hand-set knob, typed or dragged, which is how
        // a shader's named parameters get exposed without a signal.
        let mut slots = div().flex().flex_col().gap(tokens::SPACE_MD);
        for (slot, (_, label)) in SlotTargets::labelled(&self.config.source)
            .targets()
            .into_iter()
            .enumerate()
        {
            let value = resolved.slots.get(slot).copied().unwrap_or(0.0);
            let routed =
                self.config.routes.iter().any(|route| {
                    route.enabled && surface::target_slot(&route.target) == Some(slot)
                });
            let control = match (routed, self.slot_scrubs.get(slot)) {
                (false, Some(scrub)) => panel::value_slider_edit(
                    scrub,
                    &self.value_edit,
                    value,
                    format!("{value:.2}"),
                    format!("{value:.2}"),
                    |typed| typed,
                    move |this: &mut Self, fraction, cx| {
                        set_manual_value(&mut this.config.manual, slot, fraction);
                        cx.notify();
                    },
                    cx,
                ),
                _ => slot_readout(value),
            };
            slots = slots.child(panel::setting_row_dyn(
                label,
                Some(slot_accessor(slot).into()),
                control,
            ));
        }

        div()
            .flex()
            .flex_col()
            .gap(SECTION_GAP)
            .child(section(
                "Routes",
                Some(add.into_any_element()),
                editor.list(cx),
            ))
            .child(section("Slots", None, slots))
    }

    /// The panel's own dropdown entries: the builtins, and the idle switch
    /// for a quick flip without opening the settings window.
    fn preset_menu(
        &self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let panel = cx.entity();
        let submenu = PopupMenu::build(window, cx, move |mut submenu, _, cx| {
            panel::follow_panel(&panel, cx);
            for (index, (label, _)) in PRESETS.iter().enumerate() {
                submenu = submenu.item(panel::check_row(
                    *label,
                    None,
                    move |this: &Self| this.config.source == PRESETS[index].1,
                    move |this: &mut Self, cx| this.use_preset(index, cx),
                    &panel,
                ));
            }
            submenu
        });
        menu.item(PopupMenuItem::submenu("Preset", submenu))
    }
}

/// The WGSL accessor a slot arrives on, so the settings page says where to
/// read it rather than leaving the mapping to be counted out by hand.
fn slot_accessor(slot: usize) -> String {
    let lane = ["x", "y", "z", "w"][slot % 4];
    format!("params.signals[{}].{lane}", slot / 4)
}

/// A routed slot's live value. While a route feeds the slot, the route is
/// the whole value, so what belongs here is a readout rather than a
/// control; the unrouted slots get the hand-set slider instead. The signal
/// glyph up front is what says "connected" at a glance against the sliders
/// around it.
fn slot_readout(value: f32) -> Div {
    const BAR: f32 = 64.0;
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_XS)
        .child(
            svg()
                .path(icons::AUDIO_WAVEFORM)
                .size(px(12.))
                .flex_none()
                .text_color(palette::accent()),
        )
        .child(
            div()
                .w(px(28.))
                .text_xs()
                .text_color(palette::text_faint())
                .child(format!("{value:.2}")),
        )
        .child(
            div()
                .w(px(BAR))
                .h(px(6.))
                .rounded(px(3.))
                .bg(palette::bg_control())
                .child(
                    div()
                        .h_full()
                        .w(px(BAR * value.clamp(0.0, 1.0)))
                        .rounded(px(3.))
                        .bg(palette::accent()),
                ),
        )
}

/// What a shader author can count on, spelled out where they'd look for it.
/// The uniform block is gpui's, pinned by the shaders contract; the meta
/// floats are rox's own convention and live nowhere else in the UI.
fn conventions() -> Div {
    let line = |text: &'static str| {
        div()
            .text_xs()
            .text_color(palette::text_muted())
            .child(text)
    };
    div()
        .flex()
        .flex_col()
        .gap(px(3.))
        .child(line(
            "Define fn fs_user(uv: vec2<f32>) -> vec4<f32>. Output is premultiplied alpha.",
        ))
        .child(line(
            "In scope: params.time, params.delta, params.resolution, params.mouse, \
             params.signals (16 slots) and params.user_meta (8 floats).",
        ))
        .child(line(
            "Reading `screen` shades what sits under the panel; reading `prev` gives the \
             shader its own last frame. Either one moves it to the screen pass.",
        ))
        .child(line(
            "user_meta[0] is volume, track position 0..1, 1 while playing, and track \
             length in seconds. user_meta[1] is reserved and reads zero.",
        ))
        .child(line(
            "Module-scope declarations of your own are rejected: the template already \
             binds everything there is.",
        ))
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

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(self.config.chrome.title.as_deref(), "Shader")
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

    /// The layout dump carries the panel's config, source and all; the
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
        let menu = self.preset_menu(menu, window, cx);
        // Icon on the row so it lines up with Rename and the rest of the tail
        // and the tick lands on the right, the way every other top-level
        // check row in the app reads. The icon-less form is for flyouts.
        let menu = menu.item(panel::check_row(
            "Run When Idle",
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
        )
    }
}

impl Render for ShaderPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(cx))
    }
}

impl ShaderPanel {
    fn body(&mut self, cx: &mut Context<Self>) -> Div {
        self.poll_reload(cx);
        // Read here rather than in the paint closure, which has no cx: the
        // hub wants it to spot a song change for the aggregates that reset
        // on one, and a render happens every frame audio moves.
        let track = self.state.player.read(cx).playing_entry();
        let note = self.body_note();

        // A source waiting on approval never reaches registration: the
        // canvas paints nothing and the body carries the note above.
        let source = if self.pending() {
            String::new()
        } else {
            self.config.source.clone()
        };
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
            .when_some(note, |root, lines| {
                root.child(
                    div()
                        .absolute()
                        .inset_0()
                        .p(tokens::SPACE_MD)
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .overflow_hidden()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .children(lines),
                )
            })
    }
}

/// One frame of the shader: register what the config carries, resolve the
/// routes, and record the right kind of pass.
#[allow(clippy::too_many_arguments)]
fn paint(
    bounds: gpui::Bounds<gpui::Pixels>,
    window: &mut Window,
    cx: &mut App,
    source: &str,
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
    let hash = source_hash(source);
    let shader = {
        let mut compiled = compiled.lock().unwrap();
        if !compiled.ran || compiled.source != hash {
            // Registration caches by content, but only what compiled: a
            // rejection re-runs naga on every call, and this closure runs
            // every frame. So a broken source is tried once and the answer
            // kept until the text moves.
            let previous = compiled.error.take();
            // What's on screen stays on screen through a failed compile:
            // the message lands in the body over a shader that still runs,
            // which is what makes saving from an editor bearable.
            let good = compiled.shader;
            *compiled = match window.register_user_shader(source) {
                Ok(shader) => Compiled {
                    source: hash,
                    ran: true,
                    shader: Some(shader),
                    error: None,
                },
                Err(message) => Compiled {
                    source: hash,
                    ran: true,
                    shader: good,
                    error: Some(message),
                },
            };
            if previous != compiled.error {
                // The body renders this message and was built before this
                // ran, so the panel needs another pass to show it. Without
                // the nudge a broken shader asks for no frames and the
                // message never lands.
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
    seed_manual(&mut targets, manual);
    // The tick is deduped inside the hub, so several panels riding the pool
    // cost one.
    hub.tick(feed, track);
    signal_ui::apply_routes(routes, hub, &mut targets);
    let meta = surface::meta_slots(window, cx);

    // Caps decide the path: a shader that reads the screen under it or its
    // own last frame needs the region pass, and one that draws from nothing
    // but its uniforms is a plain in-scene quad. Getting this backwards
    // paints nothing at all, since each call skips what it can't run.
    let screen = window
        .user_shader_caps(shader)
        .is_some_and(|caps| caps.samples_screen || caps.uses_prev);
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
    // notifies exactly this view, which is the cheap wake - a window
    // refresh would rebuild every view in the window uncached.
    if hub.live() || run_when_idle {
        window.request_animation_frame();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // The other builtin, checked here for the paint path it covers. Both
    // live with the gate now, since the gate is what has to know them.
    use surface::TRAILS;

    /// The one thing a source has to define, in the shape the template
    /// calls it by.
    const ENTRY: &str = "fn fs_user(uv: vec2<f32>) -> vec4<f32>";

    fn config_with_routes() -> ShaderConfig {
        ShaderConfig {
            chrome: PanelChrome {
                title: Some("Wall".to_string()),
                locked: true,
                ..PanelChrome::default()
            },
            source: "// @slot 0: bass\nfn fs_user(uv: vec2<f32>) -> vec4<f32> \
                     { return vec4<f32>(params.signals[0].x); }"
                .to_string(),
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
        // The source rides the config, which is what lets a shader travel
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

    #[test]
    fn hand_set_values_hold_slots_no_route_feeds() {
        let mut manual = Vec::new();
        set_manual_value(&mut manual, 3, 0.5);
        set_manual_value(&mut manual, 0, 2.0);
        // A second write replaces, and typed values clamp to the slot's
        // 0..1.
        set_manual_value(&mut manual, 3, 0.75);
        assert_eq!(manual_value(&manual, 3), Some(0.75));
        assert_eq!(manual_value(&manual, 0), Some(1.0));
        assert_eq!(manual_value(&manual, 5), None);

        // Seeded under the routes: a live route writes over its slot, the
        // hand-set value holds the ones nothing feeds.
        let hub = rox_viz::signal::SignalHub::new(Vec::new());
        let routes = vec![Route {
            enabled: true,
            signal: 1,
            target: surface::slot_target(0),
            from: 0.0,
            to: 1.0,
        }];
        let mut targets = SlotTargets::default();
        seed_manual(&mut targets, &manual);
        signal_ui::apply_routes(&routes, &hub, &mut targets);
        // The route's signal is gone from the pool, so it contributes
        // nothing and the seed survives even on the routed slot.
        assert_eq!(targets.slots[0], 1.0);
        assert_eq!(targets.slots[3], 0.75);
        assert_eq!(targets.slots[5], 0.0);
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
    /// itself lives inside the vendored crate and can't be reached from
    /// here; what's checkable from this side is the shape.
    #[test]
    fn presets_are_shaped_like_the_contract() {
        for (label, source) in PRESETS {
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

    /// One of each kind, which is the point of shipping two: the pure one
    /// exercises the in-scene quad and the other the region pass.
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
    }

    /// The `// @slot n: name` convention is what gives the Bindings page
    /// something to call a slot, so a preset that ships without names is a
    /// preset nobody can read the bindings of.
    #[test]
    fn presets_name_their_slots() {
        for (label, source) in PRESETS {
            let labels = surface::slot_labels(source);
            assert!(
                labels[0].is_some(),
                "{label} should name at least its first slot"
            );
            let named = labels.iter().filter(|name| name.is_some()).count();
            assert!(named >= 4, "{label} names only {named} slots");
        }
    }

    /// Where a slot lands in the uniform block, the mapping the Bindings
    /// page prints beside each row.
    #[test]
    fn slot_accessors_walk_the_uniform_block() {
        assert_eq!(slot_accessor(0), "params.signals[0].x");
        assert_eq!(slot_accessor(3), "params.signals[0].w");
        assert_eq!(slot_accessor(4), "params.signals[1].x");
        assert_eq!(slot_accessor(surface::SLOTS - 1), "params.signals[3].w");
    }
}
