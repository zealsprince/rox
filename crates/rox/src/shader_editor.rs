//! The shader editor window: one window over one surface's WGSL, opened from
//! the Edit button every shader picker carries. Modest on purpose: the
//! eject-and-watch loop serves real editors, and this is for quick tweaks and
//! the hints a file can't offer (uniforms, named slots, the signal pool with
//! live meters).
//!
//! The buffer validates a beat after each edit but never registers, since
//! registration keeps a pipeline per check. Apply registers, writes the text
//! where the target says, and approves its hash. The surface keeps its last
//! good registration through a broken apply.
//!
//! An unapproved source opens here to be read, and nothing runs it until the
//! first edit vouches for it. One window per target.

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    App, Bounds, Context, Div, Entity, Focusable, Global, KeyBinding, KeyDownEvent, SharedString,
    Stateful, Subscription, Window, WindowHandle, actions, div, prelude::*, px, size,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{Root, Sizable};

use crate::matching::{WindowRegistry, open_or_focus};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel::AppState;
use rox_panel_api::panel::shader::edit::{EditKey, ShaderEditTarget};
use rox_panel_api::panel::shader::{self as surface};
use rox_panel_api::signal_ui;
use rox_panel_kit::ui::{self as settings_ui, Seg, kbd_line, section};
use rox_services::backdrop::{NowPlayingArt, WindowBackdrop};
use rox_viz::signal::SignalHub;

const DEFAULT_SIZE: (f32, f32) = (940., 660.);

const HINTS_W: f32 = 250.;

/// A verdict per keystroke would flicker through every half-typed line.
const CHECK_AFTER: Duration = Duration::from_millis(350);

/// naga's caret line is the useful part.
const ERROR_LINES: usize = 12;

actions!(shader_editor, [Apply]);

const CONTEXT: &str = "ShaderEditor";

// A multi-line input takes plain enter as a newline, so apply uses the primary
// modifier.
#[cfg(target_os = "macos")]
const APPLY_CHORD: &str = "cmd-enter";

#[cfg(not(target_os = "macos"))]
const APPLY_CHORD: &str = "ctrl-enter";

/// Bound on the input, not the window: the input binds the same chord itself
/// and the focused element's bindings win. At the same depth the later binding
/// wins, and the keymap lays this one down after the component library's.
pub fn bindings() -> Vec<KeyBinding> {
    vec![KeyBinding::new(
        APPLY_CHORD,
        Apply,
        Some(&format!("{CONTEXT} > Input")),
    )]
}

#[derive(Default)]
struct OpenEditors(Vec<(EditKey, WindowHandle<Root>)>);

impl Global for OpenEditors {}

impl WindowRegistry for OpenEditors {
    type Key = EditKey;
    fn entries(&mut self) -> &mut Vec<(EditKey, WindowHandle<Root>)> {
        &mut self.0
    }
}

pub fn open(state: AppState, target: ShaderEditTarget, cx: &mut App) {
    open_or_focus::<OpenEditors>(
        target.key.clone(),
        move |cx| {
            let bounds = Bounds::centered(None, size(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1)), cx);
            rox_panel_api::panel::open_child_window(
                cx,
                rox_i18n::t!("shader-editor-window-title"),
                bounds,
                Some(settings_ui::MIN_SIZE),
                move |window, cx| cx.new(|cx| ShaderEditor::new(state, target, window, cx)),
            )
        },
        cx,
    );
}

enum Check {
    Unchecked,
    /// Unapproved and not vouched for, so nothing has compiled it.
    Pending,
    Ok,
    Err(String),
    Unsupported,
}

struct Uniform {
    insert: &'static str,
    kind: &'static str,
    blurb: &'static str,
}

/// The `meta` meanings are rox's convention (see `meta_slots` in the surface
/// module); the block only calls them user_meta.
const UNIFORMS: &[Uniform] = &[
    Uniform {
        insert: "params.time",
        kind: "f32",
        blurb: "shader-editor-uniform-time",
    },
    Uniform {
        insert: "params.delta",
        kind: "f32",
        blurb: "shader-editor-uniform-delta",
    },
    Uniform {
        insert: "params.resolution",
        kind: "vec2<f32>",
        blurb: "shader-editor-uniform-resolution",
    },
    Uniform {
        insert: "params.mouse",
        kind: "vec4<f32>",
        blurb: "shader-editor-uniform-mouse",
    },
    Uniform {
        insert: "params.user_meta[0]",
        kind: "vec4<f32>",
        blurb: "shader-editor-uniform-meta-0",
    },
    Uniform {
        insert: "params.user_meta[1]",
        kind: "vec4<f32>",
        blurb: "shader-editor-uniform-meta-1",
    },
];

const TEXTURES: &[Uniform] = &[
    Uniform {
        insert: "textureSample(screen, samp, uv)",
        kind: "screen",
        blurb: "shader-editor-texture-screen",
    },
    Uniform {
        insert: "textureSample(prev, samp, uv)",
        kind: "prev",
        blurb: "shader-editor-texture-prev",
    },
];

struct ShaderEditor {
    target: ShaderEditTarget,
    input: Entity<InputState>,
    /// What Revert restores and a clean buffer is compared against.
    applied: String,
    check: Check,
    check_gen: u64,
    /// An edit vouches for the text, the same act as picking a file.
    vouched: bool,
    /// Sticky: only a registration finds this out, and later checks don't
    /// register.
    no_pipeline: bool,
    warning: Option<SharedString>,
    hub: Arc<SignalHub>,
    now_art: Entity<NowPlayingArt>,
    backdrop: WindowBackdrop,
    _subscriptions: Vec<Subscription>,
}

impl ShaderEditor {
    fn new(
        state: AppState,
        target: ShaderEditTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .code_editor("wgsl")
                .line_number(true)
        });
        input.update(cx, |input, cx| {
            input.set_value(target.source.clone(), window, cx);
        });
        window.focus(&input.read(cx).focus_handle(cx));
        let mut subscriptions = vec![
            cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    // A source that opened pending starts being checked from
                    // the first keystroke.
                    this.vouched = true;
                    this.check_soon(window, cx);
                }
            }),
            cx.observe(&state.now_art, |_, _, cx| cx.notify()),
            // The meters move with the music, and this window pumps its own
            // frames.
            cx.observe(&state.player, |_, _, cx| cx.notify()),
        ];
        subscriptions.shrink_to_fit();
        let hub = state.signals.clone();
        let now_art = state.now_art.clone();
        let applied = target.source.clone();
        let mut this = ShaderEditor {
            target,
            input,
            applied,
            check: Check::Unchecked,
            check_gen: 0,
            vouched: false,
            no_pipeline: false,
            warning: None,
            hub,
            now_art,
            backdrop: WindowBackdrop::default(),
            _subscriptions: subscriptions,
        };
        // An unapproved source gets no upfront verdict: checking compiles it,
        // which is what the approval gate exists to stop.
        if surface::approved(&this.target.source) {
            this.check(true, window, cx);
        } else {
            this.check = Check::Pending;
        }
        this
    }

    fn text(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    /// Trimmed like the approval hash, so a trailing newline isn't an edit.
    fn dirty(&self, cx: &App) -> bool {
        self.text(cx).trim() != self.applied.trim()
    }

    fn check_soon(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.check_gen = self.check_gen.wrapping_add(1);
        let generation = self.check_gen;
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(CHECK_AFTER).await;
            this.update_in(cx, |this, window, cx| {
                if this.check_gen == generation {
                    this.check(false, window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// `register` compiles into this window, the surface's own path, for apply.
    /// The debounced check validates instead: registration never evicts, so it
    /// would leave a pipeline per pause.
    fn check(&mut self, register: bool, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.text(cx);
        // Nothing has vouched for this text, so nothing compiles it.
        if !self.vouched && !surface::approved(&text) {
            self.check = Check::Pending;
            cx.notify();
            return;
        }
        self.check = if text.trim().is_empty() {
            Check::Unchecked
        } else if self.no_pipeline {
            // Validation can't reach this verdict: it lives in the renderer,
            // not the WGSL.
            Check::Unsupported
        } else {
            let checked = if register {
                surface::register_program(window, &text, &self.target.ctx).map(|_| ())
            } else {
                surface::validate_program(&text, &self.target.ctx)
            };
            match checked {
                Ok(()) => Check::Ok,
                Err(error) if surface::unsupported(&error) => {
                    self.no_pipeline = true;
                    Check::Unsupported
                }
                Err(error) => Check::Err(error),
            }
        };
        cx.notify();
    }

    /// A broken buffer applies too: the surface keeps its last good
    /// registration and shows the error itself.
    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.text(cx);
        if text.trim().is_empty() {
            return;
        }
        self.warning = self.target.apply(text.clone(), cx).map(Into::into);
        self.applied = text;
        // The one check that registers, so the status describes what the
        // surface got.
        self.vouched = true;
        self.check(true, window, cx);
    }

    fn revert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let applied = self.applied.clone();
        self.input.update(cx, |input, cx| {
            input.set_value(applied, window, cx);
        });
        self.warning = None;
        cx.notify();
    }

    fn insert(
        &mut self,
        text: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = text.into();
        self.input.update(cx, |input, cx| {
            input.insert(text, window, cx);
        });
        window.focus(&self.input.read(cx).focus_handle(cx));
    }

    fn hint_row(
        &self,
        id: SharedString,
        name: SharedString,
        kind: Option<SharedString>,
        blurb: Option<SharedString>,
        insert: String,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let head = div()
            .flex()
            .flex_row()
            .items_baseline()
            .gap(tokens::SPACE_XS)
            .min_w_0()
            .child(
                div()
                    .truncate()
                    .text_color(palette::text_bright())
                    .child(name),
            )
            .children(kind.map(|kind| {
                div()
                    .flex_none()
                    .text_color(palette::text_muted())
                    .child(kind)
            }));
        div()
            .id(id)
            .flex()
            .flex_col()
            .gap(px(1.))
            .px(tokens::SPACE_SM)
            .py(px(3.))
            .rounded(tokens::RADIUS)
            .cursor_pointer()
            .hover(|d| d.bg(palette::bg_control()))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.insert(insert.clone(), window, cx);
            }))
            .child(head)
            .children(blurb.map(|blurb| div().text_color(palette::text_muted()).child(blurb)))
    }

    fn hints(&mut self, cx: &mut Context<Self>) -> Stateful<Div> {
        let text = self.text(cx);
        let labels = surface::slot_labels(&text);

        let uniforms = div()
            .flex()
            .flex_col()
            .children(UNIFORMS.iter().map(|uniform| {
                self.hint_row(
                    uniform.insert.into(),
                    uniform.insert.into(),
                    Some(uniform.kind.into()),
                    Some(rox_i18n::t!(uniform.blurb)),
                    uniform.insert.to_string(),
                    cx,
                )
            }));

        let textures = div()
            .flex()
            .flex_col()
            .children(TEXTURES.iter().map(|texture| {
                self.hint_row(
                    texture.insert.into(),
                    texture.kind.into(),
                    None,
                    Some(rox_i18n::t!(texture.blurb)),
                    texture.insert.to_string(),
                    cx,
                )
            }));

        // Each slot is one lane of a vec4.
        let slots = div()
            .flex()
            .flex_col()
            .children((0..surface::SLOTS).map(|slot| {
                let lane = ["x", "y", "z", "w"][slot % 4];
                let insert = format!("params.signals[{}].{lane}", slot / 4);
                let name: SharedString = match labels.get(slot).and_then(|name| name.clone()) {
                    Some(name) => name.into(),
                    None => rox_i18n::t!("shader-editor-slot-unnamed", n = slot as u64),
                };
                self.hint_row(
                    format!("slot-{slot}").into(),
                    name,
                    Some(insert.clone().into()),
                    None,
                    insert,
                    cx,
                )
            }));

        // Clicking a signal declares it on the first unnamed slot with an
        // `@slot` line; the route itself is made on the Bindings page.
        let pool = self.hub.pool();
        let next_free = labels.iter().position(Option::is_none);
        let signals = if pool.is_empty() {
            div()
                .px(tokens::SPACE_SM)
                .text_color(palette::text_muted())
                .child(rox_i18n::t!("shader-editor-signals-empty"))
        } else {
            div().flex().flex_col().children(pool.iter().map(|signal| {
                let label = signal.label();
                let insert = match next_free {
                    Some(slot) => format!("// @slot {slot}: {label}\n"),
                    None => String::new(),
                };
                let meter = signal_ui::meter(self.hub.clone(), signal.id, palette::accent(), None);
                self.hint_row(
                    format!("signal-{}", signal.id).into(),
                    label.into(),
                    None,
                    None,
                    insert,
                    cx,
                )
                .child(meter)
            }))
        };

        div()
            .id("shader-editor-hints")
            .flex_none()
            .w(px(HINTS_W))
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .text_xs()
            .child(section(
                rox_i18n::t!("shader-editor-section-uniforms"),
                None,
                uniforms,
            ))
            .child(section(
                rox_i18n::t!("shader-editor-section-textures"),
                None,
                textures,
            ))
            .child(section(
                rox_i18n::t!("shader-editor-section-slots"),
                None,
                slots,
            ))
            .child(section(
                rox_i18n::t!("shader-editor-section-signals"),
                None,
                signals,
            ))
    }

    fn status(&self) -> Div {
        let line =
            |tone: gpui::Rgba, text: SharedString| div().text_xs().text_color(tone).child(text);
        let readout = match &self.check {
            Check::Unchecked => line(
                palette::text_muted(),
                rox_i18n::t!("shader-editor-status-unchecked"),
            ),
            // The panel's own wording for the same state.
            Check::Pending => div()
                .flex()
                .flex_col()
                .child(line(
                    palette::tone_warn(),
                    rox_i18n::t!("shader-panel-note-pending-title"),
                ))
                .child(line(
                    palette::text_muted(),
                    rox_i18n::t!("shader-panel-note-pending-body"),
                )),
            Check::Ok => line(
                palette::tone_good(),
                rox_i18n::t!("shader-editor-status-ok"),
            ),
            Check::Unsupported => div()
                .flex()
                .flex_col()
                .child(line(
                    palette::tone_warn(),
                    surface::NO_PIPELINE_TITLE.into(),
                ))
                .child(line(
                    palette::text_muted(),
                    surface::NO_PIPELINE_NOTE.into(),
                )),
            // naga's lines stay left-aligned: the caret points at a column.
            Check::Err(error) => div()
                .flex()
                .flex_col()
                .child(line(
                    palette::tone_bad(),
                    rox_i18n::t!("shader-editor-status-error"),
                ))
                .children(
                    error
                        .lines()
                        .take(ERROR_LINES)
                        .map(|l| line(palette::text_muted(), l.to_string().into())),
                ),
        };
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .min_w_0()
            .overflow_hidden()
            .child(readout)
            .children(
                self.warning
                    .clone()
                    .map(|warning| line(palette::tone_warn(), warning)),
            )
    }

    fn footer(&self, dirty: bool, cx: &mut Context<Self>) -> Div {
        let hint = kbd_line([
            Seg::Text(rox_i18n::t!("shader-editor-hint-press")),
            Seg::Key(settings_ui::chord("Enter")),
            Seg::Text(rox_i18n::t!("shader-editor-hint-apply")),
        ])
        .text_xs();
        div()
            .flex()
            .flex_row()
            .items_start()
            .justify_between()
            .gap(tokens::SPACE_MD)
            .px(tokens::SPACE_MD)
            .py(tokens::SPACE_SM)
            .border_t_1()
            .border_color(palette::border())
            .bg(palette::bg_panel())
            .child(self.status().flex_1())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_none()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .child(hint)
                    .child(settings_ui::small_button(
                        rox_i18n::t!("shader-editor-apply"),
                        icons::CHECK,
                        !dirty,
                        cx.listener(|this, _, window, cx| this.apply(window, cx)),
                    ))
                    .child(settings_ui::small_button(
                        rox_i18n::t!("shader-editor-revert"),
                        icons::REFRESH_CW,
                        !dirty,
                        cx.listener(|this, _, window, cx| this.revert(window, cx)),
                    ))
                    .child(settings_ui::small_button(
                        rox_i18n::t!("shader-editor-close"),
                        icons::CLOSE,
                        false,
                        cx.listener(|_, _, window, _| window.remove_window()),
                    )),
            )
    }

    fn header(&self) -> Div {
        let origin: SharedString = match (&self.target.key, &self.target.path) {
            (EditKey::Pool(_), Some(path)) => {
                rox_i18n::t!(
                    "shader-editor-origin-pool-file",
                    path = path.display().to_string()
                )
            }
            (EditKey::Pool(_), None) => rox_i18n::t!("shader-editor-origin-pool"),
            (_, Some(path)) => {
                rox_i18n::t!(
                    "shader-editor-origin-file",
                    path = path.display().to_string()
                )
            }
            (_, None) => rox_i18n::t!("shader-editor-origin-inline"),
        };
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(2.))
            .child(
                div()
                    .truncate()
                    .text_color(palette::text_bright())
                    .child(self.target.title.clone()),
            )
            .child(
                div()
                    .truncate()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(origin),
            )
    }
}

impl Render for ShaderEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dirty = self.dirty(cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette::bg_elevated())
            .text_color(palette::text_bright())
            .text_sm()
            // SearchInput scopes playback bindings out; ShaderEditor scopes the
            // apply in.
            .key_context("SearchInput ShaderEditor")
            .on_action(cx.listener(|this, _: &Apply, window, cx| this.apply(window, cx)))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                // Escape only closes a clean window: a dirty buffer isn't saved
                // anywhere yet.
                if event.keystroke.key != "escape" || this.dirty(cx) {
                    return;
                }
                window.remove_window();
            }))
            .children(self.backdrop.layer(&self.now_art, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .bg(palette::bg_elevated())
                    .gap(tokens::SPACE_SM)
                    .p(tokens::SPACE_MD)
                    .child(self.header())
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .flex_row()
                            .gap(tokens::SPACE_MD)
                            .child(
                                // The input frames itself transparent, so the buffer gets
                                // its own card.
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .rounded(tokens::RADIUS)
                                    .border_1()
                                    .border_color(palette::border())
                                    .bg(palette::bg_root())
                                    .overflow_hidden()
                                    .child(
                                        Input::new(&self.input).appearance(false).h_full().small(),
                                    ),
                            )
                            .child(self.hints(cx)),
                    ),
            )
            .child(self.footer(dirty, cx))
    }
}
