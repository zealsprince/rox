//! The cover match window: searches the art providers for the cover
//! editor's album and hands the picked image to the editor's front slot.
//! The editor stays the one writer; nothing is written until it saves.

use std::sync::Arc;

use gpui::{
    App, Bounds, Context, Div, Entity, EntityId, Global, Image, ObjectFit, ScrollHandle,
    SharedString, Subscription, Task, WeakEntity, Window, WindowHandle, div, img, prelude::*, px,
    size,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{Root, Sizable as _};

use crate::cover::editor::{CoverEditor, decode, sniff_mime};
use crate::matching::{Phase, WindowRegistry, note, open_or_focus};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_net::providers::{self, ArtCandidate, TrackQuery};
use rox_panel_kit::ui::{self as settings_ui, SECTION_GAP, section};
use rox_services::backdrop::{NowPlayingArt, WindowBackdrop};

const DEFAULT_SIZE: (f32, f32) = (720., 560.);

const TILE: f32 = 132.0;

const SEARCH_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(350);

/// Keyed by the opening editor plus the query: apply fills that editor's
/// front slot, so two editors on one album each need their own window.
#[derive(Default)]
struct OpenMatchers(Vec<((EntityId, String), WindowHandle<Root>)>);

impl Global for OpenMatchers {}

impl WindowRegistry for OpenMatchers {
    type Key = (EntityId, String);
    fn entries(&mut self) -> &mut Vec<((EntityId, String), WindowHandle<Root>)> {
        &mut self.0
    }
}

pub fn open(
    now_art: Entity<NowPlayingArt>,
    editor: WeakEntity<CoverEditor>,
    artist: String,
    album: String,
    cx: &mut App,
) {
    let key = (editor.entity_id(), format!("{artist}\u{0}{album}"));
    open_or_focus::<OpenMatchers>(
        key,
        move |cx| {
            let bounds = Bounds::centered(None, size(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1)), cx);
            rox_panel_api::panel::open_child_window(
                cx,
                rox_i18n::t!("cover-matcher-window-title"),
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

struct Loaded {
    candidate: ArtCandidate,
    thumb: Option<Arc<Image>>,
}

struct CoverMatch {
    /// Weak, so a closed editor drops the result.
    editor: WeakEntity<CoverEditor>,
    artist_input: Entity<InputState>,
    album_input: Entity<InputState>,
    /// Replacing it cancels the last timer and any request in flight.
    search_task: Option<Task<()>>,
    phase: Phase<Loaded>,
    selected: Option<usize>,
    applying: bool,
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
                .placeholder(rox_i18n::t!("head-piece-artist"))
                .default_value(artist)
        });
        let album_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(rox_i18n::t!("head-piece-album"))
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

    /// The subject is the album, so the title stays empty.
    fn query(&self, cx: &App) -> TrackQuery {
        TrackQuery {
            artist: self.artist_input.read(cx).value().trim().to_string(),
            title: String::new(),
            album: self.album_input.read(cx).value().trim().to_string(),
            duration_secs: None,
        }
    }

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
            Err(e) => {
                log::warn!("cover search: {e}");
                self.phase = Phase::Failed(rox_i18n::t!("cover-matcher-search-failed", error = e));
            }
        }
        cx.notify();
    }

    /// Each load checks the tile still holds its URL, so a newer search never
    /// takes a stale thumbnail.
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
                    if let Phase::Ready(loaded) = &mut this.phase
                        && let Some(slot) = loaded.get_mut(i)
                        && slot.candidate.thumb_url == url
                    {
                        slot.thumb = Some(image);
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
    }

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
                    let mime = sniff_mime(&bytes).ok_or_else(|| {
                        rox_i18n::t!("cover-matcher-unsupported-format").to_string()
                    })?;
                    Ok((bytes, mime.to_string()))
                }) {
                    Ok((bytes, mime)) => {
                        let set = editor
                            .update(cx, |editor, cx| editor.set_front(bytes, mime, cx))
                            .is_ok();
                        if set {
                            window.remove_window();
                        } else {
                            this.applying = false;
                            this.error = Some(rox_i18n::t!("cover-matcher-editor-closed"));
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

    fn search_fields(&self) -> Div {
        let field = |label: SharedString, input: &Entity<InputState>| {
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
            .child(field(rox_i18n::t!("head-piece-artist"), &self.artist_input))
            .child(field(rox_i18n::t!("head-piece-album"), &self.album_input))
    }

    fn grid(&self, loaded: &[Loaded], cx: &mut Context<Self>) -> Div {
        let mut grid = div().flex().flex_row().flex_wrap().gap(tokens::SPACE_MD);
        for (ix, slot) in loaded.iter().enumerate() {
            let selected = self.selected == Some(ix);
            let preview = match &slot.thumb {
                Some(image) => div()
                    .size_full()
                    .child(
                        img(image.clone())
                            .size_full()
                            .overflow_hidden()
                            .object_fit(ObjectFit::Cover),
                    )
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
            let source = rox_i18n::t!(
                "cover-matcher-tile-info",
                provider = slot.candidate.provider,
                width = slot.candidate.width as u64
            );
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
                            .child(source),
                    ),
            );
        }
        grid
    }

    /// Ordered the way a search clears them, so the footer names the next step.
    fn blocker(&self) -> Option<SharedString> {
        if !matches!(self.phase, Phase::Ready(ref l) if !l.is_empty()) {
            return Some(match self.phase {
                Phase::Searching => "Searching...".into(),
                _ => rox_i18n::t!("cover-matcher-blocked-no-cover"),
            });
        }
        if self.selected.is_none() {
            return Some(rox_i18n::t!("cover-matcher-blocked-pick"));
        }
        if self.applying {
            return Some(rox_i18n::t!("cover-matcher-blocked-fetching"));
        }
        None
    }

    /// No Enter binding: the query boxes use Enter to search now, and a window
    /// binding would apply off results that search is about to replace.
    fn footer(&self, can_apply: bool, cx: &mut Context<Self>) -> Div {
        let blocker = self.blocker();
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
            .child(match blocker {
                Some(reason) => div()
                    .text_xs()
                    .text_color(palette::tone_warn())
                    .child(reason),
                None => div(),
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .child(settings_ui::small_button(
                        if self.applying {
                            rox_i18n::t!("cover-matcher-setting")
                        } else {
                            rox_i18n::t!("cover-matcher-set-cover")
                        },
                        icons::CHECK,
                        !can_apply,
                        cx.listener(|this, _, window, cx| this.apply(window, cx)),
                    ))
                    .child(settings_ui::small_button(
                        rox_i18n::t!("settings-common-cancel"),
                        icons::CLOSE,
                        self.applying,
                        cx.listener(|_, _, window, _| window.remove_window()),
                    )),
            )
    }
}

impl Render for CoverMatch {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let can_apply = self.blocker().is_none();
        let count = match &self.phase {
            Phase::Ready(loaded) if !loaded.is_empty() => Some(
                div()
                    .text_xs()
                    .text_color(palette::text())
                    .child(rox_i18n::t!(
                        "cover-matcher-cover-count",
                        count = loaded.len() as u64
                    ))
                    .into_any_element(),
            ),
            _ => None,
        };

        let content = match &self.phase {
            Phase::Searching => note("Searching...").into_any_element(),
            Phase::Failed(e) => crate::console_window::notice(e.clone()).into_any_element(),
            Phase::Ready(loaded) if loaded.is_empty() => {
                note(rox_i18n::t!("cover-matcher-no-covers")).into_any_element()
            }
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
            .children(self.backdrop.layer(&self.now_art, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .bg(palette::bg_elevated())
                    .gap(SECTION_GAP)
                    .p(tokens::SPACE_MD)
                    .child(section(
                        rox_i18n::t!("query-search"),
                        None,
                        self.search_fields(),
                    ))
                    .when_some(self.error.clone(), |d, error| {
                        d.child(div().text_color(palette::text_muted()).child(error))
                    })
                    .child(
                        section(
                            rox_i18n::t!("art-covers-section"),
                            count,
                            div().flex_1().min_h_0().child(content),
                        )
                        .flex_1()
                        .min_h_0(),
                    ),
            )
            .child(self.footer(can_apply, cx))
    }
}
