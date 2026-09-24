//! The lyrics edit window: the raw sheet in a multi-line input, with
//! Shift+Enter stamping the cursor line at the playback position and an
//! offset control that shifts every stamp. Save writes back where the sheet
//! came from (tag, `.lrc` sidecar, or app store) and rings the save signal.
//!
//! Every edit goes to the lyrics panels as an unsaved draft, dropped when the
//! window closes. Keyed by subject rather than path: a station's words belong
//! to the song it announced, not the stream URL.

use gpui::{
    AnyElement, App, Bounds, Context, Div, Entity, Focusable, Global, KeyBinding, KeyDownEvent,
    SharedString, Subscription, Window, WindowHandle, actions, div, prelude::*, px, size,
};
use gpui_component::input::{Input, InputEvent, InputState, Position};
use gpui_component::{Root, Sizable};

use rox_library::lyrics::{self, Source, Subject};

use crate::matching::{WindowRegistry, open_or_focus};
use rox_core::settings::lyrics_dir;
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel::AppState;
use rox_panel_kit::ui::{self as settings_ui, Seg, icon_button, kbd_line, section};
use rox_panels::lyrics::StampLine;
use rox_services::backdrop::{NowPlayingArt, WindowBackdrop};
use rox_services::lyrics::{LyricsTarget, playing_subject, save_target};
use rox_services::player::{fmt_time, song_clock};

const DEFAULT_SIZE: (f32, f32) = (575., 620.);

const MARK_ALPHA: u8 = 48;

const DEFAULT_STEP: &str = "0.25";

actions!(lyrics_edit, [Save]);

/// Shared with the stamp binding in [`crate::keymap`].
const CONTEXT: &str = "LyricsEdit";

// Plain Enter is a newline in the sheet, so save takes the primary modifier.
#[cfg(target_os = "macos")]
const SAVE_CHORD: &str = "cmd-enter";

#[cfg(not(target_os = "macos"))]
const SAVE_CHORD: &str = "ctrl-enter";

pub fn bindings() -> Vec<KeyBinding> {
    vec![KeyBinding::new(SAVE_CHORD, Save, Some(CONTEXT))]
}

#[derive(Default)]
struct OpenEditors(Vec<(Subject, WindowHandle<Root>)>);

impl Global for OpenEditors {}

impl WindowRegistry for OpenEditors {
    type Key = Subject;
    fn entries(&mut self) -> &mut Vec<(Subject, WindowHandle<Root>)> {
        &mut self.0
    }
}

pub fn open(state: AppState, target: LyricsTarget, cx: &mut App) {
    open_or_focus::<OpenEditors>(
        target.subject.clone(),
        move |cx| {
            let bounds = Bounds::centered(None, size(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1)), cx);
            rox_panel_api::panel::open_child_window(
                cx,
                rox_i18n::t!("lyrics-edit-window-title"),
                bounds,
                Some(settings_ui::MIN_SIZE),
                move |window, cx| cx.new(|cx| LyricsEdit::new(state, target, window, cx)),
            )
        },
        cx,
    );
}

struct LyricsEdit {
    state: AppState,
    subject: Subject,
    line: SharedString,
    input: Entity<InputState>,
    /// Repointed by the baseline read. Until then the tag, or the store for a
    /// track with no file.
    target: Source,
    /// None until the read lands; save stays inert until then.
    baseline: Option<String>,
    error: Option<SharedString>,
    saving: bool,
    /// (row, time) per stamp, kept current with the text.
    rows: Vec<(usize, f64)>,
    step: Entity<InputState>,
    /// So a tick repaints only when the stamp readout changes.
    shown_secs: Option<u64>,
    now_art: Entity<NowPlayingArt>,
    backdrop: WindowBackdrop,
    _backdrop_changed: Subscription,
    _player_changed: Subscription,
    _input_changed: Subscription,
    _step_changed: Subscription,
    _draft_dropped: Subscription,
}

impl LyricsEdit {
    fn new(
        state: AppState,
        target: LyricsTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let subject = target.subject.clone();
        let input = cx.new(|cx| InputState::new(window, cx).multi_line(true));
        window.focus(&input.read(cx).focus_handle(cx));
        let step = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(DEFAULT_STEP)
                .validate(|s, _| s.trim().is_empty() || s.trim().parse::<f64>().is_ok())
        });
        let line = target.label();
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        let _player_changed = cx.observe(&state.player, |this: &mut Self, _, cx| this.tick(cx));
        let _input_changed = cx.subscribe(&input, |this: &mut Self, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.reindex(cx);
                this.publish(cx);
            }
        });
        let _step_changed = cx.subscribe(&step, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        let _draft_dropped = cx.on_release({
            let subject = subject.clone();
            move |_, cx| crate::lyrics::preview(&subject, None, cx)
        });
        let save_to = match subject.file() {
            Some(_) => Source::Tag,
            None => save_target(&subject),
        };
        let now_art = state.now_art.clone();
        let this = LyricsEdit {
            state,
            subject,
            line: line.into(),
            input,
            target: save_to,
            baseline: None,
            error: None,
            saving: false,
            rows: Vec::new(),
            step,
            shown_secs: None,
            now_art,
            backdrop: WindowBackdrop::default(),
            _backdrop_changed,
            _player_changed,
            _input_changed,
            _step_changed,
            _draft_dropped,
        };
        this.load(window, cx);
        this
    }

    /// Held back until the baseline lands, so the empty input doesn't blank the
    /// panels for a frame.
    fn publish(&self, cx: &mut Context<Self>) {
        if self.baseline.is_none() {
            return;
        }
        let text = self.input.read(cx).value().to_string();
        let subject = self.subject.clone();
        cx.defer(move |cx| crate::lyrics::preview(&subject, Some(&text), cx));
    }

    fn load(&self, window: &mut Window, cx: &mut Context<Self>) {
        let subject = self.subject.clone();
        cx.spawn_in(window, async move |this, cx| {
            let read = cx
                .background_executor()
                .spawn({
                    let subject = subject.clone();
                    async move { lyrics::load(&subject, Some(&lyrics_dir())) }
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                if this.subject != subject {
                    return;
                }
                let text = read.as_ref().map(|l| l.text.clone()).unwrap_or_default();
                if let Some(loaded) = &read {
                    this.target = loaded.source.clone();
                }
                this.input
                    .update(cx, |input, cx| input.set_value(text.clone(), window, cx));
                this.baseline = Some(text);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Matched on the subject, not the track, so a station's announced song lines
    /// up. A station's own clock counts the listen, so this reads the song clock.
    fn playback_position(&self, cx: &App) -> Option<f64> {
        let player = self.state.player.read(cx);
        if playing_subject(player).as_ref() != Some(&self.subject) {
            return None;
        }
        let now = player.now_playing()?;

        Some(song_clock(now.position_secs, now.song_start_secs))
    }

    fn reindex(&mut self, cx: &mut Context<Self>) {
        self.rows = lyrics::stamp_rows(&self.input.read(cx).value());
        self.mark(self.playback_position(cx), cx);
        cx.notify();
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        // One clock read: resolving what's playing takes the station's title lock.
        let position = self.playback_position(cx);
        self.mark(position, cx);
        let secs = position.map(|secs| secs as u64);
        if secs != self.shown_secs {
            self.shown_secs = secs;
            cx.notify();
        }
    }

    /// The input repaints itself, so this costs the window no frame.
    fn mark(&mut self, position: Option<f64>, cx: &mut Context<Self>) {
        let row = position.and_then(|position| lyrics::row_at(&self.rows, position));
        let wash = palette::alpha(palette::accent(), MARK_ALPHA);
        self.input.update(cx, |input, cx| {
            input.set_marked_line(row.map(|row| (row, wash.into())), cx);
        });
    }

    fn step_secs(&self, cx: &App) -> Option<f64> {
        let secs: f64 = self.step.read(cx).value().trim().parse().ok()?;
        (secs.is_finite() && secs > 0.0).then_some(secs)
    }

    /// Shift every stamp by the step, for a sheet that runs early or late as a whole.
    fn nudge(&mut self, direction: f64, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.baseline.is_none() || self.rows.is_empty() {
            return;
        }
        let Some(step) = self.step_secs(cx) else {
            return;
        };
        let delta = step * direction;
        let input = self.input.clone();
        let (text, cursor) = {
            let state = input.read(cx);
            (state.value(), state.cursor_position())
        };
        let shifted = lyrics::shift_stamps(&text, delta);
        input.update(cx, |state, cx| {
            state.set_value(shifted, window, cx);
            state.set_cursor_position(cursor, window, cx);
        });
    }

    fn offset_control(&self, ready: bool, cx: &mut Context<Self>) -> AnyElement {
        let inert = !ready || self.rows.is_empty() || self.step_secs(cx).is_none();
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .text_xs()
            .child(
                div()
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!("lyrics-edit-offset")),
            )
            .child(div().w(px(56.)).child(Input::new(&self.step).xsmall()))
            .child(
                div()
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!("lyrics-edit-offset-unit")),
            )
            .child(icon_button(
                icons::CHEVRON_LEFT,
                inert,
                cx.listener(|this, _, window, cx| this.nudge(-1.0, window, cx)),
            ))
            .child(icon_button(
                icons::CHEVRON_RIGHT,
                inert,
                cx.listener(|this, _, window, cx| this.nudge(1.0, window, cx)),
            ))
            .into_any_element()
    }

    /// Stamp the current line (when there's a position) and step down, growing a
    /// blank line at the end so Shift+Enter never dead-ends.
    fn stamp_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let position = self.playback_position(cx);
        let input = self.input.clone();
        let (text, line_ix) = {
            let state = input.read(cx);
            (
                state.value().to_string(),
                state.cursor_position().line as usize,
            )
        };
        let mut lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        let ix = line_ix.min(lines.len() - 1);
        if let Some(position) = position {
            let body = lyrics::strip_leading_stamps(&lines[ix]).to_owned();
            lines[ix] = format!("{}{body}", lyrics::format_stamp(position));
        }
        if ix + 1 >= lines.len() {
            lines.push(String::new());
        }
        let next = (ix + 1) as u32;
        let new_text = lines.join("\n");
        input.update(cx, |state, cx| {
            state.set_value(new_text, window, cx);
            state.set_cursor_position(Position::new(next, 0), window, cx);
        });
        cx.notify();
    }

    /// An empty sheet saves as "no lyrics" rather than an empty home, so the
    /// automatic lookup leaves the track alone.
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(baseline), false) = (&self.baseline, self.saving) else {
            return;
        };
        let text = self.input.read(cx).value().to_string();
        if &text == baseline {
            window.remove_window();
            return;
        }
        self.saving = true;
        self.error = None;
        let subject = self.subject.clone();
        let target = self.target.clone();
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn({
                    let subject = subject.clone();
                    let target = target.clone();
                    let text = text.clone();
                    async move { lyrics::save(&subject, &target, &text, Some(&lyrics_dir())) }
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(()) => {
                        // Lyrics aren't in the projection, so every panel needs a poke.
                        crate::lyrics::saved(&subject, cx);
                        window.remove_window();
                    }
                    Err(e) => {
                        this.saving = false;
                        this.error = Some(e.into());
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    fn footer(&self, ready: bool, cx: &mut Context<Self>) -> Div {
        let position = self.playback_position(cx);
        let stamp_label = match position {
            Some(secs) => rox_i18n::t!("lyrics-edit-stamp-time", time = fmt_time(secs)),
            None => rox_i18n::t!("lyrics-edit-stamp"),
        };
        let stamp = settings_ui::small_button(
            stamp_label,
            icons::CLOCK,
            !ready || position.is_none(),
            cx.listener(|this, _, window, cx| this.stamp_line(window, cx)),
        );
        let reason = if self.baseline.is_none() {
            Some(rox_i18n::t!("lyrics-edit-loading"))
        } else if self.saving {
            Some(rox_i18n::t!("lyrics-edit-saving"))
        } else {
            None
        };
        let hint = match reason {
            Some(reason) => div()
                .text_xs()
                .text_color(palette::tone_warn())
                .child(reason)
                .into_any_element(),
            None => {
                let mut segs = vec![
                    Seg::Text("Press".into()),
                    Seg::Key(settings_ui::chord("Enter")),
                    Seg::Text("to save".into()),
                ];
                if position.is_some() {
                    segs.push(Seg::Text(rox_i18n::t!("lyrics-edit-hint-or")));
                    segs.push(Seg::Key("Shift+Enter".into()));
                    segs.push(Seg::Text(rox_i18n::t!("lyrics-edit-hint-after-stamp")));
                }
                kbd_line(segs).text_xs().into_any_element()
            }
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(tokens::SPACE_SM)
            .px(tokens::SPACE_MD)
            .py(tokens::SPACE_SM)
            .border_t_1()
            .border_color(palette::border())
            .bg(palette::bg_panel())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .min_w_0()
                    .gap(tokens::SPACE_SM)
                    .child(stamp)
                    .child(hint),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .child(settings_ui::small_button(
                        "Save",
                        icons::CHECK,
                        !ready,
                        cx.listener(|this, _, window, cx| this.save(window, cx)),
                    ))
                    .child(settings_ui::small_button(
                        rox_i18n::t!("settings-common-cancel"),
                        icons::CLOSE,
                        self.saving,
                        cx.listener(|_, _, window, _| window.remove_window()),
                    )),
            )
    }
}

impl Render for LyricsEdit {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ready = !self.saving && self.baseline.is_some();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette::bg_elevated())
            .text_color(palette::text_bright())
            .text_sm()
            // SearchInput keeps the playback bindings out of the input; LyricsEdit
            // scopes in the stamp and save bindings.
            .key_context("SearchInput LyricsEdit")
            .on_action(cx.listener(|this, _: &StampLine, window, cx| {
                cx.stop_propagation();
                this.stamp_line(window, cx);
            }))
            .on_action(cx.listener(|this, _: &Save, window, cx| this.save(window, cx)))
            .on_key_down(cx.listener(|_, event: &KeyDownEvent, window, _| {
                if event.keystroke.key != "escape" {
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
                    .child(
                        section(
                            rox_i18n::t!("lyrics-edit-section"),
                            Some(self.offset_control(ready, cx)),
                            div()
                                .flex_1()
                                .min_h_0()
                                .flex()
                                .flex_col()
                                .gap(tokens::SPACE_SM)
                                .child(
                                    div()
                                        .flex_none()
                                        .truncate()
                                        .text_color(palette::text_muted())
                                        .child(self.line.clone()),
                                )
                                .child(
                                    // The input's background thins to nothing, so the
                                    // sheet needs its own card.
                                    div()
                                        .flex_1()
                                        .min_h_0()
                                        .rounded(tokens::RADIUS)
                                        .border_1()
                                        .border_color(palette::border())
                                        .bg(palette::bg_root())
                                        .overflow_hidden()
                                        .child(
                                            Input::new(&self.input)
                                                .appearance(false)
                                                .h_full()
                                                .small(),
                                        ),
                                ),
                        )
                        .flex_1()
                        .min_h_0(),
                    )
                    .when_some(self.error.clone(), |d, error| {
                        d.child(div().text_color(palette::text_muted()).child(error))
                    }),
            )
            .child(self.footer(ready, cx))
    }
}
