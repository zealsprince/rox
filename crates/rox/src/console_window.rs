//! The console window: one OS window opened from the Application menu that
//! shows the app's log live - the same lines the backend writes to stderr
//! and the rolling file on disk (see [`crate::logging`]). Standalone rather
//! than a dock panel so it's one click from the menu whatever the layout,
//! and reachable from a panel or a match window that hit an error without
//! rearranging the workspace. It reads the in-memory ring; Reveal opens the
//! file on disk for a report, Copy grabs the visible lines, Clear tidies the
//! pane without touching the file, and the level toggles filter the view.
//!
//! Global, so it takes no state of its own: it themes to the front
//! workspace's playback tint if there is one, and [`open_button`]/[`notice`]
//! let any failing surface offer a way in without threading state through.
//! The logger writes from any thread, so there's no entity to observe; a
//! light poll while the window is open picks up new lines and repaints only
//! when the ring's sequence moved, so an idle console costs nothing.

use std::time::Duration;

use gpui::{
    div, point, prelude::*, px, size, App, Bounds, ClipboardItem, Context, Div, EntityId, Global,
    Rgba, ScrollHandle, SharedString, Window, WindowHandle,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{Icon, Root, Sizable as _};
use log::Level;

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::logging;
use crate::panel;
use crate::settings::ui as settings_ui;
use crate::settings::{LayoutSize, Settings};

/// How often the open window checks the ring for new lines. Fast enough to
/// read live, slow enough that an idle console never shows up in a profile.
const POLL: Duration = Duration::from_millis(250);

/// The open console window, if any: opening again focuses it instead of
/// stacking a second one, the stats window's move.
struct OpenConsole(WindowHandle<Root>);

impl Global for OpenConsole {}

/// Open the console window, or bring the open one to the front. Global, so
/// it reads the front workspace itself for the playback tint to theme to,
/// and a caller needs nothing to hand it in.
///
/// Deferred, because the menu action that opens it runs inside the
/// workspace's own update: reading the front workspace for the tint mid-update
/// would panic, so the read and the window open wait for the cycle to settle.
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
    // Theme to the front workspace's player if one is up; the console is a
    // global window, so it borrows whatever song tint is showing rather than
    // owning one.
    let player = crate::workspace::front_workspace(cx).map(|(_, state)| state.player.entity_id());
    let min = settings_ui::MIN_SIZE;
    let (width, height) = Settings::load()
        .windows
        .console
        .filter(|s| s.width >= f32::from(min.width) && s.height >= f32::from(min.height))
        .map(|s| (s.width, s.height))
        .unwrap_or((720., 480.));
    let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
    let handle = crate::panel::open_child_window(
        cx,
        "rox - Console",
        bounds,
        Some(min),
        move |window, cx| cx.new(|cx| ConsoleWindow::new(player, window, cx)),
    );
    cx.set_global(OpenConsole(handle));
}

/// A small button that opens the console, for a panel or window to sit
/// beside a failure message so the log is one click away. The mark matches
/// the menu entry's.
pub fn open_button() -> impl IntoElement {
    Button::new("open-console")
        .icon(Icon::default().path(icons::FILE_TEXT))
        .label("Open Console")
        .small()
        .ghost()
        .on_click(|_, _, cx| open(cx))
}

/// A failed-lookup state every online surface shares: the plain reason (no
/// URL, no key - the provider sanitizes those, see
/// [`crate::providers::net_reason`]) centered over a button into the console,
/// where the same line and the rest of the session's log sit for a report.
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
    /// The workspace player the window themes to, if one was up when it
    /// opened; None themes to the base palette.
    player: Option<EntityId>,
    /// The ring as of the last poll, newest last.
    lines: Vec<logging::Line>,
    /// The ring sequence the shown lines were read at, so the poll repaints
    /// only when it moved.
    seen: u64,
    /// Pin to the newest line as it lands. On while reading live; flip it off
    /// to scroll back through history without the tail yanking the view down.
    follow: bool,
    /// The level filter: each toggle hides its level from the pane. All on by
    /// default, so the console opens showing everything.
    show_error: bool,
    show_warn: bool,
    show_info: bool,
    scroll: ScrollHandle,
}

impl ConsoleWindow {
    fn new(player: Option<EntityId>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // The frame persists on the OS close button, which never runs
        // remove_window, so write the size in the should-close hook, the
        // settings window's move.
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
        cx.spawn(async move |view, cx| loop {
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

    /// Whether a line's level passes the filter. Debug and trace are never
    /// emitted (the backend caps at info), so they ride through if they ever
    /// appear rather than vanishing behind a toggle that isn't shown.
    fn shows(&self, level: Level) -> bool {
        match level {
            Level::Error => self.show_error,
            Level::Warn => self.show_warn,
            Level::Info => self.show_info,
            Level::Debug | Level::Trace => true,
        }
    }

    /// The lines the filter lets through, newest last.
    fn shown(&self) -> Vec<&logging::Line> {
        self.lines.iter().filter(|l| self.shows(l.level)).collect()
    }

    /// The shown lines as one block of text, the shape Copy hands the
    /// clipboard.
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

    /// One level filter toggle: outlined while its level shows, ghost when
    /// hidden, so the row reads which levels are on at a glance.
    fn level_toggle(
        &self,
        id: &'static str,
        label: &'static str,
        on: bool,
        set: fn(&mut Self, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) -> Button {
        let button = Button::new(id)
            .label(label)
            .small()
            .on_click(cx.listener(move |this, _, _, cx| set(this, cx)));
        if on {
            button.outline()
        } else {
            button.ghost()
        }
    }

    /// The action row: the level filters, the follow toggle, and the copy,
    /// reveal, and clear buttons, over the shown-line count.
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
                    .child(SharedString::from(match count {
                        1 => "1 line".to_string(),
                        n => format!("{n} lines"),
                    })),
            )
            .child(self.level_toggle(
                "console-error",
                "Error",
                self.show_error,
                |this, cx| {
                    this.show_error = !this.show_error;
                    cx.notify();
                },
                cx,
            ))
            .child(self.level_toggle(
                "console-warn",
                "Warn",
                self.show_warn,
                |this, cx| {
                    this.show_warn = !this.show_warn;
                    cx.notify();
                },
                cx,
            ))
            .child(self.level_toggle(
                "console-info",
                "Info",
                self.show_info,
                |this, cx| {
                    this.show_info = !this.show_info;
                    cx.notify();
                },
                cx,
            ))
            .child({
                // Outlined while following, ghost when off, so the button
                // reads its own state.
                let follow = Button::new("console-follow")
                    .icon(Icon::default().path(icons::ARROW_DOWN))
                    .label("Follow")
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.follow = !this.follow;
                        cx.notify();
                    }));
                if self.follow {
                    follow.outline()
                } else {
                    follow.ghost()
                }
            })
            .child(
                Button::new("console-copy")
                    .icon(Icon::default().path(icons::COPY))
                    .label("Copy")
                    .small()
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(this.as_text()));
                    })),
            )
            .child(
                Button::new("console-reveal")
                    .icon(Icon::default().path(icons::FILE_TEXT))
                    .label("Reveal")
                    .small()
                    .ghost()
                    .on_click(cx.listener(|_, _, _, cx| {
                        cx.reveal_path(&logging::log_path());
                    })),
            )
            .child(
                Button::new("console-clear")
                    .icon(Icon::default().path(icons::TRASH))
                    .label("Clear")
                    .small()
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| {
                        logging::clear();
                        this.lines.clear();
                        this.seen = logging::seq();
                        cx.notify();
                    })),
            )
    }

    /// The scrolling log body: one row per shown line, the time muted and the
    /// message colored by level. Pinned to the bottom while Follow is on.
    fn body(&mut self) -> gpui::AnyElement {
        let shown = self.shown();
        if shown.is_empty() {
            let empty = if self.lines.is_empty() {
                "Nothing logged yet"
            } else {
                "Nothing at these levels"
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
            .text_xs()
            .children(shown.into_iter().map(line_row));
        // A huge negative offset lands at the bottom: the scroll container
        // clamps it to the real maximum at paint, so Follow pins the tail
        // without measuring the content height here.
        if self.follow {
            self.scroll.set_offset(point(px(0.), px(-1_000_000.)));
        }
        div()
            .id("console-body")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(rows)
            .into_any_element()
    }
}

impl Render for ConsoleWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // With no workspace player to theme to, tint to the window's own id,
        // which the palette map doesn't know, so it reads the base palette.
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
                .child(self.body())
                .into_any_element()
        })
    }
}

/// One log line as a row: the time in a fixed muted column, then the
/// message wrapping after it in the level's color.
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

/// The message color per level: error reads red, warning amber, everything
/// else the plain and muted text roles. The first two are the shared status
/// tones, so a red line here and a red banner elsewhere are the same red.
fn level_color(level: Level) -> Rgba {
    match level {
        Level::Error => palette::tone_bad(),
        Level::Warn => palette::tone_warn(),
        Level::Info => palette::text(),
        Level::Debug | Level::Trace => palette::text_muted(),
    }
}
