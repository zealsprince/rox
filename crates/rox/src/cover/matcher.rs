//! The cover match window: one OS window opened from the cover editor to
//! find album art online. It searches the art providers off the UI
//! thread, shows the results as a thumbnail grid, and on apply fetches the
//! full image and hands it to the editor's front slot rather than writing,
//! so the editor stays the one writer. The query seeds from the album's
//! artist and album and is editable, so a wrong tag can be corrected;
//! typing re-searches after a debounce, Enter at once. Nothing is written
//! until the editor saves.
//!
//! Art is picked by eye, so the grid is the whole story: each cell shows
//! the preview, the provider, and the pixel size, the biggest first.

use std::sync::Arc;

use gpui::{
    div, img, prelude::*, px, size, App, Bounds, Context, Div, Entity, Global, Image, ObjectFit,
    ScrollHandle, SharedString, Subscription, Task, WeakEntity, Window, WindowHandle,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{Root, Sizable as _};

use crate::assets::icons;
use crate::backdrop::{NowPlayingArt, WindowBackdrop};
use crate::cover::editor::{decode, sniff_mime, CoverEditor};
use crate::design::{palette, tokens};
use crate::matching::{note, open_or_focus, Phase, WindowRegistry};
use crate::providers::{self, ArtCandidate, TrackQuery};
use crate::settings::ui::{self as settings_ui, section, SECTION_GAP};

/// The default window size: room for a few rows of preview tiles beside
/// the query.
const DEFAULT_SIZE: (f32, f32) = (720., 560.);

/// One grid tile's square side.
const TILE: f32 = 132.0;

/// How long the query rests before an edit fires a search.
const SEARCH_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(350);

/// The open match windows, keyed by the query they opened on, so asking
/// again for the same album focuses the first.
#[derive(Default)]
struct OpenMatchers(Vec<(String, WindowHandle<Root>)>);

impl Global for OpenMatchers {}

impl WindowRegistry for OpenMatchers {
    type Key = String;
    fn entries(&mut self) -> &mut Vec<(String, WindowHandle<Root>)> {
        &mut self.0
    }
}

/// Open a cover search for `artist` and `album`, filling `editor`'s front
/// slot on apply, or focus the one already on that query.
pub fn open(
    now_art: Entity<NowPlayingArt>,
    editor: WeakEntity<CoverEditor>,
    artist: String,
    album: String,
    cx: &mut App,
) {
    let key = format!("{artist}\u{0}{album}");
    open_or_focus::<OpenMatchers>(
        key,
        move |cx| {
            let bounds = Bounds::centered(None, size(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1)), cx);
            crate::panel::open_child_window(
                cx,
                "rox - Find Cover Art",
                bounds,
                Some(settings_ui::MIN_SIZE),
                move |window, cx| {
                    cx.new(|cx| CoverMatch::new(now_art, editor, artist, album, window, cx))
                },
            )
        },
        cx,
    );
}

/// A candidate and its preview once the thumbnail lands. None while the
/// preview is still downloading.
struct Loaded {
    candidate: ArtCandidate,
    thumb: Option<Arc<Image>>,
}

struct CoverMatch {
    /// The cover editor whose front slot apply fills. Weak, so a closed
    /// editor drops the result.
    editor: WeakEntity<CoverEditor>,
    /// The editable query, seeded from the album's tags.
    artist_input: Entity<InputState>,
    album_input: Entity<InputState>,
    /// The pending debounced search; replacing it cancels the last timer
    /// and any request in flight.
    search_task: Option<Task<()>>,
    phase: Phase<Loaded>,
    /// The highlighted tile, an index into the ready list.
    selected: Option<usize>,
    /// A full-image fetch is in flight for apply; the buttons hold still.
    applying: bool,
    /// A failed search or fetch, shown inline over the buttons.
    error: Option<SharedString>,
    scroll: ScrollHandle,
    now_art: Entity<NowPlayingArt>,
    backdrop: WindowBackdrop,
    _input_events: Vec<Subscription>,
    _backdrop_changed: Subscription,
}

impl CoverMatch {
    fn new(
        now_art: Entity<NowPlayingArt>,
        editor: WeakEntity<CoverEditor>,
        artist: String,
        album: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let artist_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Artist")
                .default_value(artist)
        });
        let album_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Album")
                .default_value(album)
        });
        let _input_events = [&artist_input, &album_input]
            .map(|input| {
                cx.subscribe_in(
                    input,
                    window,
                    |this, _, event: &InputEvent, _, cx| match event {
                        InputEvent::Change => this.search_soon(true, cx),
                        InputEvent::PressEnter { .. } => this.search_soon(false, cx),
                        _ => {}
                    },
                )
            })
            .into_iter()
            .collect::<Vec<_>>();
        let _backdrop_changed = cx.observe(&now_art, |_, _, cx| cx.notify());
        let mut this = CoverMatch {
            editor,
            artist_input,
            album_input,
            search_task: None,
            phase: Phase::Searching,
            selected: None,
            applying: false,
            error: None,
            scroll: ScrollHandle::new(),
            now_art,
            backdrop: WindowBackdrop::default(),
            _input_events,
            _backdrop_changed,
        };
        this.search_soon(false, cx);
        this
    }

    /// The query as the boxes stand: the album is the art subject, so it
    /// rides the query's album field with the title left empty.
    fn query(&self, cx: &App) -> TrackQuery {
        TrackQuery {
            artist: self.artist_input.read(cx).value().trim().to_string(),
            title: String::new(),
            album: self.album_input.read(cx).value().trim().to_string(),
            duration_secs: None,
        }
    }

    /// Search the art providers for the current query, debounced when a
    /// keystroke drove it. Storing the task cancels the previous timer and
    /// any request still running.
    fn search_soon(&mut self, debounce: bool, cx: &mut Context<Self>) {
        let query = self.query(cx);
        self.phase = Phase::Searching;
        self.selected = None;
        cx.notify();
        self.search_task = Some(cx.spawn(async move |this, cx| {
            if debounce {
                cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            }
            let result = cx
                .background_executor()
                .spawn(async move { providers::search_art(&query) })
                .await;
            this.update(cx, |this, cx| this.fill(result, cx)).ok();
        }));
    }

    /// Fold a finished search into the grid and kick off the thumbnail
    /// downloads, the first tile pre-selected.
    fn fill(&mut self, result: Result<Vec<ArtCandidate>, String>, cx: &mut Context<Self>) {
        match result {
            Ok(found) => {
                self.selected = (!found.is_empty()).then_some(0);
                self.phase = Phase::Ready(
                    found
                        .into_iter()
                        .map(|candidate| Loaded {
                            candidate,
                            thumb: None,
                        })
                        .collect(),
                );
                self.load_thumbs(cx);
            }
            Err(e) => self.phase = Phase::Failed(format!("Search failed: {e}").into()),
        }
        cx.notify();
    }

    /// Fetch each result's preview off the UI thread and swap it in when it
    /// lands. Each load checks the tile still holds the same URL, so a
    /// newer search's grid never takes a stale thumbnail.
    fn load_thumbs(&self, cx: &mut Context<Self>) {
        let Phase::Ready(loaded) = &self.phase else {
            return;
        };
        for (i, slot) in loaded.iter().enumerate() {
            let url = slot.candidate.thumb_url.clone();
            cx.spawn(async move |this, cx| {
                let fetch = url.clone();
                let bytes = cx
                    .background_executor()
                    .spawn(async move { providers::fetch_image(&fetch) })
                    .await;
                let Ok(bytes) = bytes else { return };
                let Some(mime) = sniff_mime(&bytes) else {
                    return;
                };
                let Some(image) = decode(&bytes, mime) else {
                    return;
                };
                this.update(cx, |this, cx| {
                    if let Phase::Ready(loaded) = &mut this.phase {
                        if let Some(slot) = loaded.get_mut(i) {
                            if slot.candidate.thumb_url == url {
                                slot.thumb = Some(image);
                                cx.notify();
                            }
                        }
                    }
                })
                .ok();
            })
            .detach();
        }
    }

    /// Fetch the selected candidate's full image and hand it to the
    /// editor's front slot, off the UI thread, then close. A failed fetch
    /// keeps the window open with the error.
    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.applying {
            return;
        }
        let Phase::Ready(loaded) = &self.phase else {
            return;
        };
        let Some(url) = self
            .selected
            .and_then(|ix| loaded.get(ix))
            .map(|slot| slot.candidate.full_url.clone())
        else {
            return;
        };
        self.applying = true;
        self.error = None;
        cx.notify();
        let editor = self.editor.clone();
        cx.spawn_in(window, async move |this, cx| {
            let fetch = url.clone();
            let bytes = cx
                .background_executor()
                .spawn(async move { providers::fetch_image(&fetch) })
                .await;
            this.update_in(cx, |this, window, cx| {
                match bytes.and_then(|bytes| {
                    let mime =
                        sniff_mime(&bytes).ok_or_else(|| "Unsupported image format".to_string())?;
                    Ok((bytes, mime.to_string()))
                }) {
                    Ok((bytes, mime)) => {
                        let set = editor
                            .update(cx, |editor, cx| editor.set_front(bytes, mime, cx))
                            .is_ok();
                        if set {
                            window.remove_window();
                        } else {
                            // The editor closed under us; nothing to fill.
                            this.applying = false;
                            this.error = Some("The cover editor was closed".into());
                            cx.notify();
                        }
                    }
                    Err(e) => {
                        this.applying = false;
                        this.error = Some(e.into());
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// The editable query: artist and album, the two fields that steer the
    /// art search.
    fn search_fields(&self) -> Div {
        let field = |label: &'static str, input: &Entity<InputState>| {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_XS)
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(label),
                )
                .child(Input::new(input).small())
        };
        div()
            .flex()
            .flex_row()
            .gap(tokens::SPACE_SM)
            .child(field("Artist", &self.artist_input))
            .child(field("Album", &self.album_input))
    }

    /// The results as a wrapping grid of preview tiles, biggest first, the
    /// selected one ringed. A tile still downloading shows a quiet
    /// placeholder in its place.
    fn grid(&self, loaded: &[Loaded], cx: &mut Context<Self>) -> Div {
        let mut grid = div().flex().flex_row().flex_wrap().gap(tokens::SPACE_MD);
        for (ix, slot) in loaded.iter().enumerate() {
            let selected = self.selected == Some(ix);
            let preview = match &slot.thumb {
                Some(image) => div()
                    .size_full()
                    .child(img(image.clone()).size_full().object_fit(ObjectFit::Cover))
                    .into_any_element(),
                None => div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(palette::text_faint())
                    .child(gpui::svg().path(icons::IMAGE).size(px(22.)))
                    .into_any_element(),
            };
            let source = format!("{}  {}px", slot.candidate.provider, slot.candidate.width);
            grid = grid.child(
                div()
                    .id(("cover", ix))
                    .w(px(TILE))
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_XS)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = Some(ix);
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(TILE))
                            .h(px(TILE))
                            .rounded(tokens::RADIUS)
                            .border_2()
                            .border_color(if selected {
                                palette::accent()
                            } else {
                                palette::border()
                            })
                            .bg(palette::bg_root())
                            .overflow_hidden()
                            .child(preview),
                    )
                    .when(!slot.candidate.album.is_empty(), |d| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(palette::text_bright())
                                .truncate()
                                .child(SharedString::from(slot.candidate.album.clone())),
                        )
                    })
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette::text_muted())
                            .truncate()
                            .child(SharedString::from(source)),
                    ),
            );
        }
        grid
    }
}

impl Render for CoverMatch {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let can_apply = matches!(self.phase, Phase::Ready(ref l) if !l.is_empty())
            && self.selected.is_some()
            && !self.applying;
        let buttons = div()
            .flex()
            .flex_row()
            .gap(tokens::SPACE_SM)
            .child(settings_ui::small_button(
                if self.applying {
                    "Setting..."
                } else {
                    "Set Cover"
                },
                icons::CHECK,
                !can_apply,
                cx.listener(|this, _, window, cx| this.apply(window, cx)),
            ))
            .child(settings_ui::small_button(
                "Cancel",
                icons::CLOSE,
                self.applying,
                cx.listener(|_, _, window, _| window.remove_window()),
            ))
            .into_any_element();

        let content = match &self.phase {
            Phase::Searching => note("Searching...").into_any_element(),
            Phase::Failed(e) => note(e.clone()).into_any_element(),
            Phase::Ready(loaded) if loaded.is_empty() => note("No covers found").into_any_element(),
            Phase::Ready(loaded) => div()
                .id("cover-grid")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .child(self.grid(loaded, cx))
                .into_any_element(),
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette::bg_elevated())
            .text_color(palette::text_bright())
            .text_sm()
            // The backdrop paints first, under the page, so translucent
            // surfaces back with the playing track's art like every window.
            .children(self.backdrop.layer(&self.now_art, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap(SECTION_GAP)
                    .p(tokens::SPACE_MD)
                    .child(section("Search", Some(buttons), self.search_fields()))
                    .when_some(self.error.clone(), |d, error| {
                        d.child(div().text_color(palette::text_muted()).child(error))
                    })
                    .child(div().flex_1().min_h_0().child(content)),
            )
    }
}
