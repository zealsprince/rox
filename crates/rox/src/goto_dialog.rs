//! The go-to modal: Ctrl+G, or the Playback menu's own row, drops a
//! timestamp field and a scrub strip over the workspace. Type a time you
//! already know, off a tracklist or a cue sheet or a note you took, and
//! Enter lands on it; drag the strip for the "somewhere around there"
//! case. Escape or a click outside closes.
//!
//! A view over the same player the panels use, hosted as an overlay the
//! way quick-play is: the workspace owns one at most and drops it on
//! dismiss.

use std::sync::{Arc, LazyLock};

use gpui::{
    canvas, div, prelude::*, px, App, Context, DismissEvent, Div, Entity, EventEmitter,
    FocusHandle, Focusable, FontFeatures, KeyDownEvent, MouseButton, MouseDownEvent, Subscription,
    Window,
};
use gpui_component::input::{Input, InputEvent, InputState};

use rox_core::fmt::fmt_time;
use rox_design::{palette, tokens};
use rox_panel_api::panel::{self, AppState, ScrubState};
use rox_panel_kit::ui::{kbd_line, Seg};
use rox_services::player::NowPlaying;

/// The modal's width, narrower than quick-play's: one field and one strip.
const WIDTH: f32 = 420.;

/// The scrub strip's height, room for the slider knob and the hover pill.
const STRIP_H: f32 = 24.;

pub struct GoTo {
    state: AppState,
    input: Entity<InputState>,
    scrub: ScrubState,
    _input_events: Subscription,
    /// The strip and the clocks read the player, so it needs the player's
    /// own notify to stay live rather than freezing at the time the modal
    /// opened. Same raw observe the seek strip runs on.
    _player: Subscription,
}

impl EventEmitter<DismissEvent> for GoTo {}

impl Focusable for GoTo {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }
}

impl GoTo {
    pub fn new(state: AppState, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Empty rather than seeded with the current time: a seeded field
        // puts the caret after four characters you have to clear before you
        // can type the one time you came here to type. The clocks beside
        // the strip say where you are instead.
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder(rox_i18n::t!("goto-placeholder")));
        let _input_events = cx.subscribe_in(
            &input,
            window,
            |this, _, event: &InputEvent, _, cx| match event {
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } => this.commit(cx),
                _ => {}
            },
        );
        let _player = cx.observe(&state.player, |_, _, cx| cx.notify());
        window.focus(&input.read(cx).focus_handle(cx));
        GoTo {
            state,
            input,
            scrub: ScrubState::default(),
            _input_events,
            _player,
        }
    }

    /// The time the field currently reads, clamped inside the track. None
    /// while the field is empty or holds something that isn't a time, which
    /// is what leaves Enter inert.
    fn target(&self, cx: &App) -> Option<f64> {
        let secs = parse_time(&self.input.read(cx).value())?;
        let duration = self
            .state
            .player
            .read(cx)
            .now_playing()
            .and_then(|now| now.duration_secs);
        // A time past the end lands on the end rather than refusing: the
        // intent is legible, and a track whose tagged duration is short by
        // a second shouldn't reject the last second of itself.
        Some(match duration {
            Some(duration) => secs.min(duration),
            None => secs,
        })
    }

    /// Seek and close.
    fn commit(&mut self, cx: &mut Context<Self>) {
        let Some(secs) = self.target(cx) else {
            return;
        };
        self.state.player.read(cx).seek_to(secs);
        cx.emit(DismissEvent);
    }

    /// The scrub strip between the two clocks: the playhead as a slider, a
    /// press or drag seeks, a hover previews the time under the pointer.
    /// The same strip state and handlers the seek panel runs on.
    fn strip(&self, now: &NowPlaying, cx: &mut Context<Self>) -> Div {
        let duration = now.duration_secs.filter(|d| *d > 0.0);
        let progress = duration
            .map(|d| (now.position_secs / d) as f32)
            .unwrap_or(0.0);
        let scrub = self.scrub.clone();
        let player = self.state.player.clone();
        let track = div()
            .flex_1()
            .min_w_0()
            .h(px(STRIP_H))
            .relative()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.scrub.begin();
                    if let Some(fraction) = this.scrub.fraction(event.position.x) {
                        panel::seek_fraction(&this.state.player, fraction, cx);
                    }
                    cx.notify();
                }),
            )
            .child(
                canvas(
                    {
                        let scrub = scrub.clone();
                        move |bounds, _, _| scrub.set_bounds(bounds)
                    },
                    move |bounds, _, window, _| {
                        panel::paint_slider(progress, false, bounds, window);
                        panel::scrub_on_paint(&scrub, window, {
                            let player = player.clone();
                            move |fraction, cx| panel::seek_fraction(&player, fraction, cx)
                        });
                    },
                )
                .size_full(),
            )
            // The preview shows once the duration resolves; before that a
            // fraction maps to nothing.
            .when_some(duration, |d, duration| {
                d.child(panel::seek_hover(&self.scrub, duration, cx))
            });
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .child(clock(fmt_time(now.position_secs)))
            .child(track)
            .children(duration.map(|d| clock(fmt_time(d))))
    }

    /// What Enter would do with the field as it stands: the time it
    /// resolves to, a note that it doesn't, or nothing while it's empty.
    fn target_line(&self, cx: &App) -> Option<Div> {
        let text = self.input.read(cx).value();
        if text.trim().is_empty() {
            return None;
        }
        let (text, color) = match self.target(cx) {
            Some(secs) => (
                rox_i18n::t!("goto-target", time = fmt_time(secs)),
                palette::text_bright(),
            ),
            None => (rox_i18n::t!("goto-unreadable"), palette::tone_bad()),
        };
        Some(div().text_sm().text_color(color).child(text))
    }

    /// The footer: the one shortcut, the way quick-play's footer names its
    /// syntax.
    fn hint_row(&self) -> Div {
        div()
            .px(tokens::SPACE_SM)
            .py(tokens::SPACE_XS)
            .border_t_1()
            .border_color(palette::border())
            .text_xs()
            .text_color(palette::text_muted())
            .child(kbd_line([
                Seg::Text(rox_i18n::t!("goto-hint-before")),
                Seg::Key(rox_i18n::t!("goto-hint-key")),
                Seg::Text(rox_i18n::t!("goto-hint-after")),
            ]))
    }
}

/// Tabular digits for the clocks, built once: [`clock`] runs twice per
/// pump tick while playing, so the feature list shouldn't reallocate
/// every call. Same feature the seek strip's clocks use.
static TNUM: LazyLock<FontFeatures> =
    LazyLock::new(|| FontFeatures(Arc::new(vec![("tnum".into(), 1)])));

/// One clock beside the strip: muted, sized to its digits, and tabular so
/// a tick never changes the text width and shifts the strip.
fn clock(text: String) -> Div {
    let mut clock = div()
        .flex_none()
        .text_sm()
        .text_color(palette::text_muted());
    clock
        .text_style()
        .get_or_insert_with(Default::default)
        .font_features = Some(TNUM.clone());
    clock.child(text)
}

impl Render for GoTo {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = self.state.player.read(cx).now_playing();
        div()
            .w(px(WIDTH))
            .flex()
            .flex_col()
            .bg(palette::bg_menu_opaque())
            .rounded(tokens::RADIUS)
            .border_1()
            .border_color(palette::border_light())
            .shadow_md()
            .occlude()
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(DismissEvent)))
            // Scopes the workspace's playback key bindings out while the
            // modal is up, so space and arrows work the field instead.
            .key_context("SearchInput")
            // The field passes an idle escape through (it only keeps one
            // that closes its IME or context menu), so it arrives here;
            // stopped so the workspace's own escape ladder never fires
            // over a handled one.
            .on_key_down(cx.listener(|_, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key != "escape" {
                    return;
                }
                cx.stop_propagation();
                cx.emit(DismissEvent);
            }))
            .child(
                div()
                    .p(tokens::SPACE_SM)
                    .border_b_1()
                    .border_color(palette::border())
                    .child(Input::new(&self.input).w_full()),
            )
            .child(
                div()
                    .p(tokens::SPACE_SM)
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_XS)
                    .children(now.as_ref().map(|now| self.strip(now, cx)))
                    .when(now.is_none(), |d| {
                        d.child(
                            div()
                                .text_sm()
                                .text_color(palette::text_muted())
                                .child(rox_i18n::t!("goto-nothing-playing")),
                        )
                    })
                    .children(self.target_line(cx)),
            )
            .child(self.hint_row())
    }
}

/// A typed timestamp as seconds: plain seconds ("83"), minutes and seconds
/// ("1:23"), or hours in front of both ("1:02:03"), with a fraction allowed
/// on the last field either way ("1:23.5"). Fields over 60 read as written,
/// so "0:90" is a minute and a half rather than an error.
///
/// Anything else is None, which keeps a half-typed entry inert instead of
/// resolving it to a time nobody asked for.
fn parse_time(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let fields: Vec<&str> = text.split(':').collect();
    if fields.len() > 3 {
        return None;
    }
    let mut secs = 0.0f64;
    for (ix, field) in fields.iter().enumerate() {
        let field = field.trim();
        // Only the last field takes a fraction. A fractional minute is a
        // typo far more often than it's an intent.
        let value: f64 = if ix + 1 == fields.len() {
            field.parse().ok()?
        } else {
            field.parse::<u32>().ok()? as f64
        };
        if !value.is_finite() || value < 0.0 {
            return None;
        }
        secs = secs * 60.0 + value;
    }
    Some(secs)
}

#[cfg(test)]
mod tests {
    use super::parse_time;

    #[test]
    fn plain_seconds_read_as_seconds() {
        assert_eq!(parse_time("83"), Some(83.0));
        assert_eq!(parse_time("  83  "), Some(83.0));
        assert_eq!(parse_time("0"), Some(0.0));
    }

    #[test]
    fn colons_read_as_minutes_and_hours() {
        assert_eq!(parse_time("1:23"), Some(83.0));
        assert_eq!(parse_time("1:02:03"), Some(3723.0));
        assert_eq!(parse_time("0:07"), Some(7.0));
    }

    #[test]
    fn a_fraction_rides_the_last_field() {
        assert_eq!(parse_time("1:23.5"), Some(83.5));
        assert_eq!(parse_time("83.25"), Some(83.25));
        // Not on a minute, where it reads as a slip rather than a time.
        assert_eq!(parse_time("1.5:23"), None);
    }

    /// A field over 60 is unambiguous, so it's taken at face value rather
    /// than refused: "0:90" is a minute and a half.
    #[test]
    fn oversized_fields_read_as_written() {
        assert_eq!(parse_time("0:90"), Some(90.0));
        assert_eq!(parse_time("0:90:00"), Some(5400.0));
    }

    #[test]
    fn anything_that_isnt_a_time_is_nothing() {
        assert_eq!(parse_time(""), None);
        assert_eq!(parse_time("   "), None);
        assert_eq!(parse_time("abc"), None);
        assert_eq!(parse_time("1:"), None);
        assert_eq!(parse_time(":30"), None);
        assert_eq!(parse_time("1:2:3:4"), None);
        assert_eq!(parse_time("-30"), None);
        assert_eq!(parse_time("1:-30"), None);
        assert_eq!(parse_time("inf"), None);
        assert_eq!(parse_time("NaN"), None);
    }
}
