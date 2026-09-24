//! The console window: the app's log live, read from the in-memory ring the
//! backend also writes to stderr and the rolling file ([`rox_core::logging`]).
//! A window rather than a panel so it's reachable from any failure without
//! rearranging the workspace. The logger has no entity to observe, so a
//! light poll repaints only when the ring's sequence moves.

use std::time::Duration;

use gpui::{
    App, Bounds, ClipboardItem, Context, Div, EntityId, Global, MouseButton, MouseDownEvent,
    Pixels, Rgba, ScrollHandle, ScrollWheelEvent, SharedString, Window, WindowHandle, div, point,
    prelude::*, px, size,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::scroll::Scrollbar;
use gpui_component::{Icon, Root, Sizable as _};
use log::Level;

use rox_core::logging;
use rox_core::settings::{LayoutSize, Settings};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel;
use rox_panel_kit::ui as settings_ui;

const POLL: Duration = Duration::from_millis(250);

/// Kept clear of the text so the thumb never sits over a message.
const LANE: Pixels = px(16.);

struct OpenConsole(WindowHandle<Root>);

impl Global for OpenConsole {}

/// Deferred: the menu action runs inside the workspace's update, and reading
/// the front workspace for the tint mid-update would panic.
pub fn open(cx: &mut App) {
    cx.defer(open_now);
}

fn open_now(cx: &mut App) {
    if let Some(open) = cx.try_global::<OpenConsole>() {
        let handle = open.0;
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    let player =
        rox_panel_api::windows::front_workspace(cx).map(|(_, state)| state.player.entity_id());
    let min = settings_ui::MIN_SIZE;
    let (width, height) = Settings::load()
        .windows
        .console
        .filter(|s| s.width >= f32::from(min.width) && s.height >= f32::from(min.height))
        .map(|s| (s.width, s.height))
        .unwrap_or((720., 480.));
    let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
    let handle = rox_panel_api::panel::open_child_window(
        cx,
        rox_i18n::t!("console-window-title"),
        bounds,
        Some(min),
        move |window, cx| cx.new(|cx| ConsoleWindow::new(player, window, cx)),
    );
    cx.set_global(OpenConsole(handle));
}

pub fn open_button() -> impl IntoElement {
    Button::new("open-console")
        .icon(Icon::default().path(icons::FILE_TEXT))
        .label(rox_i18n::t!("console-open-button"))
        .small()
        .ghost()
        .on_click(|_, _, cx| open(cx))
}

/// The shared failed-lookup state: the sanitized reason
/// ([`rox_net::providers::net_reason`]) over a button into the console.
pub fn notice(message: impl Into<SharedString>) -> Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(tokens::SPACE_SM)
        .p(tokens::SPACE_MD)
        .child(
            div()
                .text_color(palette::text_faint())
                .child(message.into()),
        )
        .child(open_button())
}

struct ConsoleWindow {
    player: Option<EntityId>,
    lines: Vec<logging::Line>,
    seen: u64,
    /// Pin to the newest line. Scrolling or grabbing the scrollbar turns it off.
    follow: bool,
    show_error: bool,
    show_warn: bool,
    show_info: bool,
    scroll: ScrollHandle,
}

impl ConsoleWindow {
    fn new(player: Option<EntityId>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // The OS close button never runs remove_window, so the size persists here.
        window.on_window_should_close(cx, |window, _| {
            let frame = window.window_bounds().get_bounds();
            Settings::update(move |s| {
                s.windows.console = Some(LayoutSize {
                    width: frame.size.width.into(),
                    height: frame.size.height.into(),
                });
            });
            true
        });
        cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let alive = view.update(cx, |this, cx| {
                    let seq = logging::seq();
                    if seq != this.seen {
                        this.seen = seq;
                        this.lines = logging::snapshot();
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        ConsoleWindow {
            player,
            lines: logging::snapshot(),
            seen: logging::seq(),
            follow: true,
            show_error: true,
            show_warn: true,
            show_info: true,
            scroll: ScrollHandle::new(),
        }
    }

    /// Debug and trace pass: the backend caps at info, and no toggle shows for them.
    fn shows(&self, level: Level) -> bool {
        match level {
            Level::Error => self.show_error,
            Level::Warn => self.show_warn,
            Level::Info => self.show_info,
            Level::Debug | Level::Trace => true,
        }
    }

    fn shown(&self) -> Vec<&logging::Line> {
        self.lines.iter().filter(|l| self.shows(l.level)).collect()
    }

    fn as_text(&self) -> String {
        let mut out = String::new();
        for line in self.shown() {
            out.push_str(&format!(
                "{} {:>5} {}\n",
                line.time, line.level, line.message
            ));
        }
        out
    }

    fn toggle(
        &self,
        id: &'static str,
        label: &'static str,
        on: bool,
        set: fn(&mut Self, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        div()
            .id(id)
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(tokens::SPACE_XS)
            .text_xs()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| set(this, cx)),
            )
            .child(settings_ui::checkbox(on))
            .child(div().text_color(palette::text_muted()).child(label))
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> Div {
        let count = self.shown().len();
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .px(tokens::SPACE_MD)
            .py(tokens::SPACE_SM)
            .flex_none()
            .border_b_1()
            .border_color(palette::border())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!("console-line-count", count = count as u64)),
            )
            .child(self.toggle(
                "console-error",
                rox_i18n::t_static("console-filter-error"),
                self.show_error,
                |this, cx| {
                    this.show_error = !this.show_error;
                    cx.notify();
                },
                cx,
            ))
            .child(self.toggle(
                "console-warn",
                rox_i18n::t_static("console-filter-warn"),
                self.show_warn,
                |this, cx| {
                    this.show_warn = !this.show_warn;
                    cx.notify();
                },
                cx,
            ))
            .child(self.toggle(
                "console-info",
                rox_i18n::t_static("console-filter-info"),
                self.show_info,
                |this, cx| {
                    this.show_info = !this.show_info;
                    cx.notify();
                },
                cx,
            ))
            .child(self.toggle(
                "console-follow",
                rox_i18n::t_static("console-follow"),
                self.follow,
                |this, cx| {
                    this.follow = !this.follow;
                    cx.notify();
                },
                cx,
            ))
            .child(settings_ui::small_button(
                rox_i18n::t!("console-copy"),
                icons::COPY,
                false,
                cx.listener(|this, _, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(this.as_text()));
                }),
            ))
            .child(settings_ui::small_button(
                rox_i18n::t!("console-reveal"),
                icons::FILE_TEXT,
                false,
                cx.listener(|_, _, _, cx| {
                    cx.reveal_path(&logging::log_path());
                }),
            ))
            .child(settings_ui::small_button(
                rox_i18n::t!("console-clear"),
                icons::TRASH,
                false,
                cx.listener(|this, _, _, cx| {
                    logging::clear();
                    this.lines.clear();
                    this.seen = logging::seq();
                    cx.notify();
                }),
            ))
    }

    fn body(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let shown = self.shown();
        if shown.is_empty() {
            let empty = if self.lines.is_empty() {
                rox_i18n::t!("console-empty-none")
            } else {
                rox_i18n::t!("console-empty-filtered")
            };
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(div().text_color(palette::text_faint()).child(empty))
                .into_any_element();
        }
        let rows = div()
            .flex()
            .flex_col()
            .w_full()
            .p(tokens::SPACE_MD)
            .pr(tokens::SPACE_MD + LANE)
            .text_xs()
            .children(shown.into_iter().map(line_row));
        // The container clamps a huge negative offset to the real maximum, so this
        // pins the tail without measuring. It runs every frame while Follow is on,
        // so both gestures below drop Follow before the view moves.
        if self.follow {
            self.scroll.set_offset(point(px(0.), px(-1_000_000.)));
        }
        div()
            .size_full()
            .relative()
            .child(
                div()
                    .id("console-body")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    // Drop the pin in the same dispatch that scrolls, or the notch is yanked back.
                    .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                        if !this.follow || event.delta.pixel_delta(window.line_height()).y == px(0.)
                        {
                            return;
                        }
                        this.follow = false;
                        cx.notify();
                    }))
                    .child(rows),
            )
            .child(
                // Only a grab on the bar's lane drops Follow. Capture phase, because the bar
                // stops propagation on its own mouse down.
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .bottom_0()
                    .w(LANE)
                    .capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        if event.button != MouseButton::Left || !this.follow {
                            return;
                        }
                        this.follow = false;
                        cx.notify();
                    }))
                    .child(Scrollbar::vertical(&self.scroll)),
            )
            .into_any_element()
    }
}

impl Render for ConsoleWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let player = self.player.unwrap_or_else(|| cx.entity().entity_id());
        palette::note_focus(player, window.is_window_active(), cx);
        panel::window_body(player, || {
            div()
                .size_full()
                .flex()
                .flex_col()
                .bg(palette::bg_elevated())
                .text_color(palette::text_bright())
                .text_sm()
                .child(self.toolbar(cx))
                .child(self.body(cx))
                .into_any_element()
        })
    }
}

fn line_row(line: &logging::Line) -> Div {
    div()
        .flex()
        .flex_row()
        .items_start()
        .gap(tokens::SPACE_SM)
        .py(px(1.))
        .child(
            div()
                .flex_none()
                .text_color(palette::text_faint())
                .child(SharedString::from(line.time.clone())),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(level_color(line.level))
                .child(SharedString::from(line.message.clone())),
        )
}

fn level_color(level: Level) -> Rgba {
    match level {
        Level::Error => palette::tone_bad(),
        Level::Warn => palette::tone_warn(),
        Level::Info => palette::text(),
        Level::Debug | Level::Trace => palette::text_muted(),
    }
}
