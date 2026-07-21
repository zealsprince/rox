//! The lyrics edit window: one OS window opened from the lyrics panel's
//! pencil, so editing the raw sheet always has room even when the panel is
//! docked narrow. It reads the file's current words off the UI thread into
//! a multi-line input, stamps the cursor line with the live playback
//! position on Shift+Enter for a play-along tag pass, and Save writes back
//! where the sheet came from: the embedded tag through the writer's atomic
//! layer, or the `.lrc` sidecar or app lyrics store as a plain file. On a
//! save it pokes the panel to re-read and closes. Nothing is written until
//! Save; closing walks away clean.
//!
//! One window per track path, registered like the match window, so asking
//! again focuses the open one instead of stacking a twin.

use std::path::PathBuf;

use gpui::{
    div, prelude::*, px, size, App, Bounds, Context, Entity, Focusable, Global, KeyDownEvent,
    SharedString, Subscription, TitlebarOptions, WeakEntity, Window, WindowBounds, WindowHandle,
    WindowOptions,
};
use gpui_component::input::{Input, InputState, Position};
use gpui_component::{Root, Sizable};

use rox_library::lyrics::{self, Source};

use crate::assets::icons;
use crate::backdrop::{NowPlayingArt, WindowBackdrop};
use crate::design::{palette, tokens};
use crate::panel::AppState;
use crate::panels::lyrics::{LyricsPanel, StampLine};
use crate::player::fmt_time;
use crate::settings::lyrics_dir;
use crate::settings_ui;

/// The default window size: tall enough for a verse or two at a glance,
/// narrow since the sheet reads one line to a row.
const DEFAULT_SIZE: (f32, f32) = (460., 620.);

/// The open edit windows, keyed by track path, so a second request for the
/// same track focuses the first - the match window's registry shape.
#[derive(Default)]
struct OpenEditors(Vec<(PathBuf, WindowHandle<Root>)>);

impl Global for OpenEditors {}

/// Open a lyrics edit window on `path`, or focus the one already on it. The
/// panel handle is weak: a save pokes it to re-read, and a closed panel
/// just no-ops.
pub fn open(state: AppState, panel: WeakEntity<LyricsPanel>, path: PathBuf, cx: &mut App) {
    let entries = cx
        .try_global::<OpenEditors>()
        .map(|open| open.0.clone())
        .unwrap_or_default();
    // Closed windows fall out of the list as a side effect of the probe.
    let mut alive = Vec::with_capacity(entries.len() + 1);
    let mut focused = false;
    for (entry_path, handle) in entries {
        let matches = entry_path == path;
        if handle
            .update(cx, |_, window, _| {
                if matches {
                    window.activate_window();
                }
            })
            .is_ok()
        {
            focused |= matches;
            alive.push((entry_path, handle));
        }
    }
    if focused {
        cx.set_global(OpenEditors(alive));
        return;
    }
    let bounds = Bounds::centered(None, size(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(settings_ui::MIN_SIZE),
        titlebar: Some(TitlebarOptions {
            title: Some("rox - Edit Lyrics".into()),
            ..Default::default()
        }),
        app_id: Some(crate::APP_ID.into()),
        ..Default::default()
    };
    let opened = path.clone();
    let handle = cx
        .open_window(options, |window, cx| {
            // The Wayland backend ignores the creation-time titlebar title;
            // only set_window_title reaches the compositor.
            window.set_window_title("rox - Edit Lyrics");
            let view = cx.new(|cx| LyricsEdit::new(state, panel, path, window, cx));
            cx.new(|cx| Root::new(view, window, cx))
        })
        .expect("failed to open the lyrics edit window");
    alive.push((opened, handle));
    cx.set_global(OpenEditors(alive));
}

struct LyricsEdit {
    state: AppState,
    /// The panel that opened this, to re-read after a save. Weak, so the
    /// window never keeps a closed panel alive.
    panel: WeakEntity<LyricsPanel>,
    /// The track the words save back to.
    path: PathBuf,
    /// The track as the header shows it.
    line: SharedString,
    input: Entity<InputState>,
    /// Where a save lands, resolved once the baseline read reports the
    /// source; the tag until then, so a brand-new sheet writes a tag.
    target: Source,
    /// The text the file held, what save diffs against; None until the read
    /// lands, and save stays inert without it.
    baseline: Option<String>,
    /// A failed read or save, shown inline over the buttons.
    error: Option<SharedString>,
    /// A save is in flight; the buttons hold still until it lands.
    saving: bool,
    now_art: Entity<NowPlayingArt>,
    backdrop: WindowBackdrop,
    _backdrop_changed: Subscription,
}

impl LyricsEdit {
    fn new(
        state: AppState,
        panel: WeakEntity<LyricsPanel>,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).multi_line(true));
        window.focus(&input.read(cx).focus_handle(cx));
        // The header names the track off its library tags, so the window
        // says what it is even before the file read lands.
        let query = crate::lyrics_match::query_for(&state, &path, cx);
        let line = if query.artist.is_empty() {
            query.title.clone()
        } else {
            format!("{} - {}", query.title, query.artist)
        };
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        let now_art = state.now_art.clone();
        let this = LyricsEdit {
            state,
            panel,
            path,
            line: line.into(),
            input,
            target: Source::Tag,
            baseline: None,
            error: None,
            saving: false,
            now_art,
            backdrop: WindowBackdrop::default(),
            _backdrop_changed,
        };
        this.load(window, cx);
        this
    }

    /// Fill the input from the file off the UI thread, pinning the save
    /// target to the source the read reports. A track with no words starts
    /// blank and writes a tag.
    fn load(&self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.path.clone();
        cx.spawn_in(window, async move |this, cx| {
            let read = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move { lyrics::load(&path, Some(&lyrics_dir())) }
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                if this.path != path {
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

    /// Where playback sits within the edited track, or None when a
    /// different track (or nothing) is playing. The stamp button keys off
    /// this.
    fn playback_position(&self, cx: &App) -> Option<f64> {
        self.state
            .player
            .read(cx)
            .now_playing()
            .filter(|now| now.path == self.path)
            .map(|now| now.position_secs)
    }

    /// Advance to the next line, stamping the current one with the playback
    /// position on the way if a position is available: strip whatever
    /// leading time tag the line has and prepend a fresh one, so a
    /// play-along tags line by line. The step down always happens, even with
    /// nothing to stamp, and it adds a blank line when there is none below,
    /// so Shift+Enter never dead-ends at the last line.
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
        // Make sure there is a line below to land on, so the last line grows
        // a fresh one instead of pinning the cursor in place.
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

    /// Save the edited text back where it came from, off the UI thread.
    /// Nothing moved closes the window; a failed save keeps it open with the
    /// error inline, the file untouched. Success pokes the panel to re-read.
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
        let path = self.path.clone();
        let target = self.target.clone();
        let panel = self.panel.clone();
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    let target = target.clone();
                    let text = text.clone();
                    async move { lyrics::save(&path, &target, &text) }
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(()) => {
                        // The panel caches lyrics off the projection, so a
                        // save it did not make needs a poke to re-read.
                        panel.update(cx, |panel, cx| panel.reload(&path, cx)).ok();
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
}

impl Render for LyricsEdit {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ready = !self.saving && self.baseline.is_some();
        // The stamp button carries the live position it will write, so the
        // rhythm is visible; inert until the edited track is the one
        // playing, since there is nothing to stamp with otherwise.
        let position = self.playback_position(cx);
        let stamp_label = match position {
            Some(secs) => format!("Stamp {}", fmt_time(secs)),
            None => "Stamp".to_owned(),
        };
        let stamp = settings_ui::small_button(
            SharedString::from(stamp_label),
            icons::CLOCK,
            !ready || position.is_none(),
            cx.listener(|this, _, window, cx| this.stamp_line(window, cx)),
        );

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette::bg_elevated())
            .text_color(palette::text_bright())
            .text_sm()
            // SearchInput scopes the workspace's playback key bindings out
            // while the input is focused; LyricsEdit scopes in the
            // Shift+Enter stamp binding (see workspace::init).
            .key_context("SearchInput LyricsEdit")
            .on_action(cx.listener(|this, _: &StampLine, window, cx| {
                cx.stop_propagation();
                this.stamp_line(window, cx);
            }))
            .on_key_down(cx.listener(|_, event: &KeyDownEvent, window, _| {
                if event.keystroke.key != "escape" {
                    return;
                }
                window.remove_window();
            }))
            // The backdrop paints first, under the page, so translucent
            // surfaces back with the playing track's art like every window.
            .children(self.backdrop.layer(&self.now_art, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_SM)
                    .p(tokens::SPACE_MD)
                    .child(
                        div()
                            .flex_none()
                            .truncate()
                            .text_color(palette::text_muted())
                            .child(self.line.clone()),
                    )
                    .child(
                        // The input frames itself transparent, and its
                        // editor background thins to nothing under surface
                        // opacity, so the sheet needs its own card to read
                        // as a surface, the match window's preview idiom.
                        div()
                            .flex_1()
                            .min_h_0()
                            .rounded(tokens::RADIUS)
                            .border_1()
                            .border_color(palette::border())
                            .bg(palette::bg_root())
                            .overflow_hidden()
                            .child(Input::new(&self.input).appearance(false).h_full().small()),
                    )
                    .when_some(self.error.clone(), |d, error| {
                        d.child(div().text_color(palette::text_muted()).child(error))
                    })
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            // Stamp on the left where the play-along attention
                            // is, its shortcut spelled out beside it, save and
                            // cancel pushed to the right.
                            .child(stamp)
                            .when(ready && position.is_some(), |d| {
                                d.child(
                                    div()
                                        .text_xs()
                                        .text_color(palette::text_faint())
                                        .child("Shift + Enter"),
                                )
                            })
                            .child(div().flex_1())
                            .child(settings_ui::small_button(
                                "Save",
                                icons::CHECK,
                                !ready,
                                cx.listener(|this, _, window, cx| this.save(window, cx)),
                            ))
                            .child(settings_ui::small_button(
                                "Cancel",
                                icons::CLOSE,
                                self.saving,
                                cx.listener(|_, _, window, _| window.remove_window()),
                            )),
                    ),
            )
    }
}
