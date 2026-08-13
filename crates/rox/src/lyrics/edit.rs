//! The lyrics edit window: one OS window opened from the lyrics panel's
//! pencil, so editing the raw sheet always has room even when the panel is
//! docked narrow. It reads the file's current words off the UI thread into
//! a multi-line input, stamps the cursor line with the live playback
//! position on Shift+Enter for a play-along tag pass, and Save writes back
//! where the sheet came from: the embedded tag through the writer's atomic
//! layer, or the `.lrc` sidecar or app lyrics store as a plain file. On a
//! save it rings the app-wide lyrics signal so every panel re-reads, then
//! closes. Nothing is written until Save; closing walks away clean.
//!
//! One window per track path, registered like the match window, so asking
//! again focuses the open one instead of stacking a twin.

use std::path::PathBuf;

use gpui::{
    actions, div, prelude::*, px, size, App, Bounds, Context, Div, Entity, Focusable, Global,
    KeyBinding, KeyDownEvent, SharedString, Subscription, Window, WindowHandle,
};
use gpui_component::input::{Input, InputState, Position};
use gpui_component::{Root, Sizable};

use rox_library::cue::TrackKey;
use rox_library::lyrics::{self, Source};

use crate::matching::{open_or_focus, WindowRegistry};
use rox_core::settings::lyrics_dir;
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel::AppState;
use rox_panel_kit::ui::{self as settings_ui, kbd_line, section, Seg};
use rox_panels::lyrics::StampLine;
use rox_services::backdrop::{NowPlayingArt, WindowBackdrop};
use rox_services::player::fmt_time;

/// The default window size: tall enough for a verse or two at a glance,
/// and wide enough that a timestamped line rarely wraps.
const DEFAULT_SIZE: (f32, f32) = (575., 620.);

actions!(lyrics_edit, [Save]);

/// The key context the window's bindings scope to. The stamp binding in
/// [`crate::keymap`] already names it, so the save joins it rather than
/// opening a second context over the same window.
const CONTEXT: &str = "LyricsEdit";

// The sheet is a multi-line input, where plain enter is a newline, so the
// save rides the platform's primary modifier: Cmd on macOS, Ctrl
// everywhere else, the fork every app-level chord takes.
#[cfg(target_os = "macos")]
const SAVE_CHORD: &str = "cmd-enter";

#[cfg(not(target_os = "macos"))]
const SAVE_CHORD: &str = "ctrl-enter";

/// The editor's save binding; call once at startup, before
/// [`crate::keymap::init`] snapshots what's bound.
pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(SAVE_CHORD, Save, Some(CONTEXT))]);
}

/// The open edit windows, keyed by track path, so a second request for the
/// same track focuses the first - the match window's registry shape.
#[derive(Default)]
struct OpenEditors(Vec<(PathBuf, WindowHandle<Root>)>);

impl Global for OpenEditors {}

impl WindowRegistry for OpenEditors {
    type Key = PathBuf;
    fn entries(&mut self) -> &mut Vec<(PathBuf, WindowHandle<Root>)> {
        &mut self.0
    }
}

/// Open a lyrics edit window on `path`, or focus the one already on it. A
/// save broadcasts through [`crate::lyrics::saved`], so the window never
/// holds a panel of its own.
pub fn open(state: AppState, path: PathBuf, cx: &mut App) {
    open_or_focus::<OpenEditors>(
        path.clone(),
        move |cx| {
            let bounds = Bounds::centered(None, size(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1)), cx);
            rox_panel_api::panel::open_child_window(
                cx,
                "rox - Edit Lyrics",
                bounds,
                Some(settings_ui::MIN_SIZE),
                move |window, cx| cx.new(|cx| LyricsEdit::new(state, path, window, cx)),
            )
        },
        cx,
    );
}

struct LyricsEdit {
    state: AppState,
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
    fn new(state: AppState, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).multi_line(true));
        window.focus(&input.read(cx).focus_handle(cx));
        // The header names the track off its library tags, so the window
        // says what it is even before the file read lands.
        let query =
            rox_services::lyrics::query_for(&state.library, &TrackKey::from(path.clone()), cx);
        let line = if query.artist.is_empty() {
            query.title.clone()
        } else {
            format!("{} - {}", query.title, query.artist)
        };
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        let now_art = state.now_art.clone();
        let this = LyricsEdit {
            state,
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
            .filter(|now| now.path() == self.path)
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
    /// error inline, the file untouched. Success pokes every panel to
    /// re-read.
    ///
    /// Saving an empty sheet says the track has no lyrics rather than just
    /// emptying its home, so the automatic lookup leaves it alone from then
    /// on. The panel's "No Lyrics for This Track" takes that back.
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
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    let target = target.clone();
                    let text = text.clone();
                    async move { lyrics::save(&path, &target, &text, Some(&lyrics_dir())) }
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(()) => {
                        // Panels cache lyrics off the projection, so every
                        // one of them needs a poke to re-read.
                        crate::lyrics::saved(&path, cx);
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

    /// The window's own actions: the stamp with the position it would
    /// write, the shortcuts for both of them, and the save.
    fn footer(&self, ready: bool, cx: &mut Context<Self>) -> Div {
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
        // What's holding the save up, when something is, in place of the
        // shortcut it would otherwise spell out.
        let reason = if self.baseline.is_none() {
            Some("Loading the sheet...")
        } else if self.saving {
            Some("Saving...")
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
                // The stamp chord only earns a mention while there's a
                // position to stamp with.
                if position.is_some() {
                    segs.push(Seg::Text("or".into()));
                    segs.push(Seg::Key("Shift+Enter".into()));
                    segs.push(Seg::Text("to stamp".into()));
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
                // Stamp on the left where the play-along attention is, its
                // shortcut spelled out beside it.
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
                        "Cancel",
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
            // SearchInput scopes the workspace's playback key bindings out
            // while the input is focused; LyricsEdit scopes in the
            // Shift+Enter stamp binding (see workspace::init) and this
            // window's own save.
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
            // The backdrop paints first, under the page, so translucent
            // surfaces back with the playing track's art like every window.
            .children(self.backdrop.layer(&self.now_art, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    // The page's own surface over the root's, the same second
                    // pass the settings page takes: the backdrop reads through
                    // only as the surfaces thin.
                    .bg(palette::bg_elevated())
                    .gap(tokens::SPACE_SM)
                    .p(tokens::SPACE_MD)
                    .child(
                        section(
                            "Lyrics",
                            None,
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
                                    // The input frames itself transparent, and
                                    // its editor background thins to nothing
                                    // under surface opacity, so the sheet needs
                                    // its own card to read as a surface, the
                                    // match window's preview idiom.
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
