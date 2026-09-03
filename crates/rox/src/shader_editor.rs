//! The shader editor window: one OS window over one surface's WGSL, opened
//! from the Edit button every shader picker carries. It's a modest editor
//! on purpose. The external-editor loop (eject, save, watch) already serves
//! anyone with a real editor; this one is for tweaking a preset without
//! leaving rox, and for the hints, which a file on disk can't offer: the
//! uniform block's fields, the slots the source names, and the signal pool
//! with live meters, each a click from landing in the buffer.
//!
//! The buffer checks itself a beat after each edit, so naga's message
//! shows while typing. That check validates rather than registers: it
//! compiles nothing into the window, since one pipeline per pause in
//! typing is a session's worth of them for a verdict. Apply takes the
//! registering path, writes the text where the target says (the pool
//! entry, the panel's config, the screen), the bookmarked file with it,
//! and approves the hash, since applying is the user vouching for the text
//! the way picking a file is. Revert puts the last applied text back. The
//! surface keeps painting its last good registration through a broken
//! apply, so half-written shaders cost nothing on screen.
//!
//! A source that arrived unapproved opens here to be read, and nothing
//! runs it: the first edit is the vouch, and the checking starts there.
//!
//! One window per target, registered like the lyrics editor: a second Edit
//! on the same surface focuses the open one.

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    actions, div, prelude::*, px, size, App, Bounds, Context, Div, Entity, Focusable, Global,
    KeyBinding, KeyDownEvent, SharedString, Stateful, Subscription, Window, WindowHandle,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{Root, Sizable};

use crate::matching::{open_or_focus, WindowRegistry};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel::shader::edit::{EditKey, ShaderEditTarget};
use rox_panel_api::panel::shader::{self as surface};
use rox_panel_api::panel::AppState;
use rox_panel_api::signal_ui;
use rox_panel_kit::ui::{self as settings_ui, kbd_line, section, Seg};
use rox_services::backdrop::{NowPlayingArt, WindowBackdrop};
use rox_viz::signal::SignalHub;

/// The default window size: room for a shader's width beside the hint
/// column, and enough lines that a preset fits without scrolling.
const DEFAULT_SIZE: (f32, f32) = (940., 660.);

/// The hint column's width: a uniform's name and type on one line.
const HINTS_W: f32 = 250.;

/// How long typing has to pause before the buffer is checked. naga is
/// quick, but a check registers a pipeline in this window, which isn't a
/// per-keystroke cost.
const CHECK_AFTER: Duration = Duration::from_millis(350);

/// How much of a compile message the readout shows. naga points at the
/// span with a caret line, which is the useful part.
const ERROR_LINES: usize = 12;

actions!(shader_editor, [Apply]);

/// The key context the window's bindings scope to.
const CONTEXT: &str = "ShaderEditor";

// The buffer is a multi-line input, where plain enter is a newline, so the
// apply uses the platform's primary modifier, the same fork the lyrics
// editor's save takes.
#[cfg(target_os = "macos")]
const APPLY_CHORD: &str = "cmd-enter";

#[cfg(not(target_os = "macos"))]
const APPLY_CHORD: &str = "ctrl-enter";

/// The editor's apply binding; call once at startup, before
/// [`crate::keymap::init`] snapshots what's bound.
///
/// Bound on the input inside this window rather than on the window: the
/// input binds the same chord itself (secondary-enter, a newline in a
/// multi-line buffer), and the focused element's bindings win over its
/// ancestors', so a window-level binding never sees the key. At the same
/// depth the later binding wins, and this one registers after the
/// component library's, so the apply takes the chord here and nowhere else.
pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        APPLY_CHORD,
        Apply,
        Some(&format!("{CONTEXT} > Input")),
    )]);
}

/// The open editors, keyed by what they edit, so a second Edit on the
/// same surface focuses the first.
#[derive(Default)]
struct OpenEditors(Vec<(EditKey, WindowHandle<Root>)>);

impl Global for OpenEditors {}

impl WindowRegistry for OpenEditors {
    type Key = EditKey;
    fn entries(&mut self) -> &mut Vec<(EditKey, WindowHandle<Root>)> {
        &mut self.0
    }
}

/// Open an editor over `target`, or focus the one already on it.
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

/// What the last check made of the buffer.
enum Check {
    /// No check has run on this text yet, or there's no text to check.
    Unchecked,
    /// The text arrived unapproved and nobody has vouched for it, so
    /// nothing has compiled it and nothing will until it's edited or
    /// applied.
    Pending,
    /// Registration took it.
    Ok,
    /// naga's message, verbatim.
    Err(String),
    /// This build has no shader pipeline, so nothing here can be checked.
    Unsupported,
}

/// One field of the uniform block, as the hint column lists it: what to
/// type, its WGSL type, and the one-line meaning.
struct Uniform {
    insert: &'static str,
    kind: &'static str,
    blurb: &'static str,
}

/// The uniform block, in declaration order. The meanings of the `meta`
/// floats are rox's convention (see `meta_slots` in the surface module),
/// spelled out here because the block itself only calls them user_meta.
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

/// The textures a screen pass can sample, with the call that reads them.
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
    state: AppState,
    target: ShaderEditTarget,
    input: Entity<InputState>,
    /// The text the surface holds, what Revert goes back to and what a
    /// clean buffer is compared against.
    applied: String,
    check: Check,
    /// Bumped on every edit; the check scheduled for an edit runs only if
    /// nothing has moved since.
    check_gen: u64,
    /// Whether the user has vouched for what's in the buffer, which an
    /// edit is: an unapproved source opens here without being run, and
    /// touching it is the same act as picking a file.
    vouched: bool,
    /// Whether this window's renderer turned a registration down for
    /// having no shader pipeline. Sticky, because only a registration can
    /// find that out and the checks after it don't register.
    no_pipeline: bool,
    /// A file that didn't take the last apply's write, shown beside the
    /// status. The surface took the text regardless.
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
            // Every edit schedules a check; the one that runs is the one
            // nothing followed within the pause.
            cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    // An edit is the user vouching for the text, the same
                    // act as picking a file, so a source that opened
                    // pending starts being checked from the first
                    // keystroke.
                    this.vouched = true;
                    this.check_soon(window, cx);
                }
            }),
            cx.observe(&state.now_art, |_, _, cx| cx.notify()),
            // The meters in the hint column move with the music, and this
            // window pumps its own frames, so the player's tick wakes it
            // the way it wakes the signals window.
            cx.observe(&state.player, |_, _, cx| cx.notify()),
        ];
        subscriptions.shrink_to_fit();
        let hub = state.signals.clone();
        let now_art = state.now_art.clone();
        let applied = target.source.clone();
        let mut this = ShaderEditor {
            state,
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
        // What the surface runs gets its verdict up front, so a shader
        // that's already broken says so before the first keystroke. An
        // unapproved source gets none: the check compiles it, and
        // compiling somebody else's WGSL because a window opened over it
        // is the thing the approval gate exists to stop.
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

    /// Whether the buffer differs from what the surface holds. Trimmed,
    /// the way the approval hash reads it, so a trailing newline isn't an
    /// edit.
    fn dirty(&self, cx: &App) -> bool {
        self.text(cx).trim() != self.applied.trim()
    }

    /// Check the buffer once typing pauses. Each edit moves the generation
    /// on, and only the timer that still matches it runs.
    fn check_soon(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.check_gen = self.check_gen.wrapping_add(1);
        let gen = self.check_gen;
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(CHECK_AFTER).await;
            this.update_in(cx, |this, window, cx| {
                if this.check_gen == gen {
                    this.check(false, window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Put a verdict on the buffer. Both routes split the chain and
    /// resolve the images the same way, so they agree on everything a
    /// text can get wrong.
    ///
    /// `register` compiles it into this window, which is what an apply
    /// wants: it's the surface's own path, so what passes here runs there,
    /// and the pipeline it leaves behind is this window's and goes with
    /// it. The debounced check validates instead. Registration is keyed by
    /// content and never evicts, so checking that way would leave a
    /// compiled pipeline per pause in typing, which is a lot of pipelines
    /// for a status line.
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
            // Already asked and answered. Validation can't reach that
            // verdict: it lives in the window's renderer, not in the WGSL.
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

    /// Put the buffer into the surface. A broken buffer applies too: the
    /// surface keeps painting its last good registration and shows the
    /// message in its own readout, the same as a save from an external
    /// editor mid-edit, so the window never has to refuse.
    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.text(cx);
        if text.trim().is_empty() {
            return;
        }
        self.warning = self.target.apply(text.clone(), cx).map(Into::into);
        self.applied = text;
        // The verdict on what was just applied, without waiting for the
        // pause: the status line should describe what the surface got, so
        // this is the one check that registers.
        self.vouched = true;
        self.check(true, window, cx);
    }

    /// Put the last applied text back in the buffer. The input reports
    /// the change, which schedules the check.
    fn revert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let applied = self.applied.clone();
        self.input.update(cx, |input, cx| {
            input.set_value(applied, window, cx);
        });
        self.warning = None;
        cx.notify();
    }

    /// Drop a hint into the buffer at the cursor and hand focus back to it.
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

    /// One row of the hint column: a name in the code face over its
    /// meaning, inserting `insert` on click.
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

    /// The hint column: the uniform block, the textures a screen pass can
    /// read, the sixteen slots under the names the buffer gives them, and
    /// the signal pool with its meters. Everything here is known at edit
    /// time and nothing on disk could tell the author.
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

        // Each slot reads as one lane of a vec4, which is the expression
        // that lands in the buffer; the row shows the name the source gave
        // the slot, or its number.
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

        // A signal reaches a shader through a slot, so clicking one
        // declares it on the first slot the source hasn't named yet: the
        // `@slot` line the Bindings page reads, which is the half of the
        // hookup a file can't do. The route itself is still made there.
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

    /// The status line: what the last check said, and a file that didn't
    /// take the last write.
    fn status(&self) -> Div {
        let line =
            |tone: gpui::Rgba, text: SharedString| div().text_xs().text_color(tone).child(text);
        let readout = match &self.check {
            Check::Unchecked => line(
                palette::text_muted(),
                rox_i18n::t!("shader-editor-status-unchecked"),
            ),
            // The panel's own wording for a source waiting on a read,
            // since it's the same source in the same state.
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
            // naga's lines stay lines and stay left: the caret under the
            // span points at a column.
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

    /// The window's own actions: the status on the left, Apply, Revert
    /// and Close on the right with the apply chord spelled out.
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

    /// The header: which surface this is over, and where its text lives.
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
        // The meters read the hub at paint, and the hub only moves when
        // something ticks it; with no visualizer open this window is that
        // something, the way the signals window is.
        {
            let player = self.state.player.read(cx);
            self.hub.tick(&player.feed(), player.playing_entry());
        }
        let dirty = self.dirty(cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette::bg_elevated())
            .text_color(palette::text_bright())
            .text_sm()
            // SearchInput scopes the workspace's playback bindings out
            // while the input is focused; ShaderEditor scopes the apply in.
            .key_context("SearchInput ShaderEditor")
            .on_action(cx.listener(|this, _: &Apply, window, cx| this.apply(window, cx)))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                // Escape closes a clean window. A dirty one stays: the
                // text isn't anywhere else yet, and a key that throws it
                // away is the wrong key to have next to the buffer.
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
                                // The input frames itself transparent, so
                                // the buffer gets its own card to read as a
                                // surface, the lyrics editor's idiom.
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
