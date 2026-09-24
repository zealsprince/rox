//! The go-to modal: a timestamp field and a scrub strip over the workspace,
//! hosted as an overlay the way quick-play is.

use std::sync::{Arc, LazyLock};

use gpui::{
    App, Context, DismissEvent, Div, Entity, EventEmitter, FocusHandle, Focusable, FontFeatures,
    KeyDownEvent, MouseButton, MouseDownEvent, Subscription, Window, canvas, div, prelude::*, px,
};
use gpui_component::input::{Input, InputEvent, InputState};

use rox_core::fmt::fmt_time;
use rox_design::{palette, tokens};
use rox_panel_api::panel::{self, AppState, ScrubState};
use rox_panel_kit::ui::{Seg, kbd_line};
use rox_services::player::NowPlaying;

const WIDTH: f32 = 420.;

const STRIP_H: f32 = 24.;

pub struct GoTo {
    state: AppState,
    input: Entity<InputState>,
    scrub: ScrubState,
    _input_events: Subscription,
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
        // Empty rather than seeded with the current time, which would have to be
        // cleared before typing. The clocks show where playback is.
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

    /// Clamped inside the track. None leaves Enter inert.
    fn target(&self, cx: &App) -> Option<f64> {
        let secs = parse_time(&self.input.read(cx).value())?;
        let duration = self
            .state
            .player
            .read(cx)
            .now_playing()
            .and_then(|now| now.duration_secs);
        // Past the end lands on the end: tagged durations can be short by a second.
        Some(match duration {
            Some(duration) => secs.min(duration),
            None => secs,
        })
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        let Some(secs) = self.target(cx) else {
            return;
        };
        self.state.player.read(cx).seek_to(secs);
        cx.emit(DismissEvent);
    }

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

/// Built once: [`clock`] runs twice per pump tick while playing.
static TNUM: LazyLock<FontFeatures> =
    LazyLock::new(|| FontFeatures(Arc::new(vec![("tnum".into(), 1)])));

/// Tabular so a tick never changes the width and shifts the strip.
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
            // Keeps space and arrows in the field instead of the playback bindings.
            .key_context("SearchInput")
            // The field passes an idle escape through. Stop it here so the workspace's
            // escape ladder doesn't fire too.
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

/// "83", "1:23", or "1:02:03", with a fraction on the last field. Fields over
/// 60 read as written ("0:90" is 90s). Anything else is None.
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
        // A fractional minute is far more often a typo than an intent.
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
        assert_eq!(parse_time("1.5:23"), None);
    }

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
