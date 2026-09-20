//! The AutoEq profile browser: search and apply 10-band equalizer settings
//! for thousands of headphones and IEMs directly from the AutoEq database.
//!
//! A dedicated window opened from the Equalizer window or the Application menu.
//! Caches the AutoEq master results index locally in the user data directory,
//! provides instant multi-word fuzzy search across 8,800+ profiles, and fetches
//! the optimal 10-band graphic EQ filter curve from GitHub on demand.
//!
//! Applying a profile immediately updates the live equalizer parameters,
//! enables the EQ node in the audio chain, and reflects on the open Equalizer
//! curve and any playing audio in real time.

use std::sync::Arc;

use gpui::{
    AnyElement, App, Bounds, Context, Div, Entity, FocusHandle, Focusable as _, Global,
    PathPromptOptions, SharedString, Subscription, Window, WindowHandle, div, prelude::*, px, size,
};
use gpui_component::button::Button;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::spinner::Spinner;
use gpui_component::{Icon, Root, Sizable as _};

use rox_core::settings;
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_net::sources::autoeq::{self, AutoEqEntry, BandSetting};
use rox_services::player;

use crate::eq_presets;

/// Default window size: tall enough for search input, status line, and a comfortable hit list.
const DEFAULT_SIZE: (f32, f32) = (560., 680.);

/// Minimum window bounds.
const MIN_SIZE: gpui::Size<gpui::Pixels> = gpui::Size {
    width: px(420.),
    height: px(320.),
};

/// Maximum search results rendered at once for snappy interaction.
const RESULT_LIMIT: usize = 100;

/// What a fetched profile is wanted for: dropped on the live curve, or
/// filed in the preset folder to reach for offline. The fetch is the same
/// either way and is the whole cost of both, so the two buttons share it.
#[derive(Clone, Copy, PartialEq)]
enum Wanted {
    Apply,
    Save,
}

/// What a saved profile is filed under: the model and the measurement it came
/// from. The index carries seven Sennheiser HD 600s off seven rigs, so a
/// preset named after the model alone would be whichever of them was saved
/// last, and the list couldn't say which one you kept.
fn preset_name(entry: &AutoEqEntry) -> String {
    format!("{} ({})", entry.name, entry.source)
}

/// The singleton handle for the open AutoEq window.
struct OpenAutoEq(WindowHandle<Root>);

impl Global for OpenAutoEq {}

/// Open the AutoEq browser window, or raise the open one to the front.
pub fn open(cx: &mut App) {
    if let Some(open) = cx.try_global::<OpenAutoEq>() {
        let handle = open.0;
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }

    let bounds = Bounds::centered(None, size(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1)), cx);
    let handle = rox_panel_api::panel::open_child_window(
        cx,
        rox_i18n::t!("autoeq-window-title"),
        bounds,
        Some(MIN_SIZE),
        move |window, cx| cx.new(|cx| AutoEqWindow::new(window, cx)),
    );

    cx.set_global(OpenAutoEq(handle));
}

struct AutoEqWindow {
    find: Entity<InputState>,
    entries: Arc<Vec<AutoEqEntry>>,
    filtered: Vec<usize>,
    loading_index: bool,
    error: Option<SharedString>,
    applying: Option<usize>,
    saving: Option<usize>,
    applied_path: Option<String>,
    applied_info: Option<SharedString>,
    /// The presets already on disk, so a profile that's been saved shows as
    /// saved rather than offering the download again. Read when the window
    /// opens and after each save; a row draws from this rather than from the
    /// folder, which is a directory read a hundred rows can't each afford.
    saved: Vec<String>,
    searched_query: String,
    focus: FocusHandle,
    _find_events: Subscription,
    /// Re-reads the folder when a preset is written or dropped anywhere
    /// else, so a preset deleted from the equalizer window stops reading as
    /// saved here.
    _presets_changed: Subscription,
}

impl AutoEqWindow {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let find = cx
            .new(|cx| InputState::new(window, cx).placeholder(rox_i18n::t!("autoeq-placeholder")));
        window.focus(&find.focus_handle(cx));

        let _find_events = cx.subscribe_in(
            &find,
            window,
            |this: &mut Self, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change | InputEvent::PressEnter { .. }) {
                    this.on_search_change(cx);
                }
            },
        );

        let mut this = AutoEqWindow {
            find,
            entries: Arc::new(Vec::new()),
            filtered: Vec::new(),
            loading_index: false,
            error: None,
            applying: None,
            saving: None,
            applied_path: None,
            applied_info: None,
            saved: eq_presets::list(),
            searched_query: String::new(),
            focus: cx.focus_handle(),
            _find_events,
            _presets_changed: eq_presets::observe(cx, |this, cx| {
                this.saved = eq_presets::list();
                cx.notify();
            }),
        };

        this.init_index(cx);
        this
    }

    /// Path to the cached AutoEq index file on disk.
    fn cache_path() -> std::path::PathBuf {
        settings::data_dir().join("autoeq_index.txt")
    }

    /// Load the AutoEq index from local cache, or fetch it from GitHub in the background.
    fn init_index(&mut self, cx: &mut Context<Self>) {
        let cache_path = Self::cache_path();

        if let Ok(content) = std::fs::read_to_string(&cache_path) {
            let parsed = autoeq::parse_index(&content);
            if !parsed.is_empty() {
                self.entries = Arc::new(parsed);
                self.on_search_change(cx);
                return;
            }
        }

        // Fetch from GitHub
        self.loading_index = true;
        self.error = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { autoeq::fetch_index() })
                .await;

            this.update(cx, |this, cx| {
                this.loading_index = false;
                match result {
                    Ok(text) => {
                        let _ = std::fs::create_dir_all(settings::data_dir());
                        let _ = std::fs::write(&cache_path, &text);
                        let parsed = autoeq::parse_index(&text);
                        this.entries = Arc::new(parsed);
                        this.on_search_change(cx);
                    }
                    Err(err) => {
                        this.error = Some(rox_i18n::t!("autoeq-failed", reason = err));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Re-filter the loaded entries when the search input changes.
    fn on_search_change(&mut self, cx: &mut Context<Self>) {
        let query = self.find.read(cx).value().trim().to_string();
        self.searched_query = query.clone();

        if self.entries.is_empty() {
            self.filtered.clear();
            cx.notify();
            return;
        }

        let hits = autoeq::filter_entries(&self.entries, &query, RESULT_LIMIT);
        // Find indices in self.entries for the matched references
        self.filtered = hits
            .into_iter()
            .filter_map(|hit| {
                let ptr = hit as *const AutoEqEntry;
                let base = self.entries.as_ptr();
                let offset =
                    (ptr as usize).wrapping_sub(base as usize) / std::mem::size_of::<AutoEqEntry>();
                (offset < self.entries.len()).then_some(offset)
            })
            .collect();

        cx.notify();
    }

    /// Fetch an entry from the search results and either put it on the
    /// curve or file it as a preset. Both take the same trip to GitHub, so
    /// what the profile is for only decides the last step.
    fn fetch_entry(&mut self, ix: usize, wanted: Wanted, cx: &mut Context<Self>) {
        let Some(&entry_ix) = self.filtered.get(ix) else {
            return;
        };
        let Some(entry) = self.entries.get(entry_ix).cloned() else {
            return;
        };

        match wanted {
            Wanted::Apply => self.applying = Some(ix),
            Wanted::Save => self.saving = Some(ix),
        }
        self.error = None;
        cx.notify();

        let path = entry.path.clone();
        let name = entry.name.clone();
        let source = entry.source.clone();

        let path_for_fetch = path.clone();
        let name_for_fetch = name.clone();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { autoeq::fetch_profile(&path_for_fetch, &name_for_fetch) })
                .await;

            this.update(cx, |this, cx| {
                this.applying = None;
                this.saving = None;
                let profile = match result {
                    Ok(profile) => profile,
                    Err(err) => {
                        this.error = Some(rox_i18n::t!("autoeq-failed", reason = err));
                        cx.notify();
                        return;
                    }
                };

                match wanted {
                    Wanted::Apply => {
                        player::apply_graphic_eq(&profile.gains_db, cx);
                        player::set_eq_enabled(true, cx);

                        this.applied_path = Some(path);
                        // Two whole messages rather than a line with a
                        // fragment glued on the end: where a preamp reading
                        // sits in the sentence is the translator's call.
                        this.applied_info = Some(match profile.preamp_db {
                            Some(db) => rox_i18n::t!(
                                "autoeq-applied-info-preamp",
                                name = name,
                                source = source,
                                db = format!("{db:+.1}"),
                            ),
                            None => {
                                rox_i18n::t!("autoeq-applied-info", name = name, source = source)
                            }
                        });
                    }
                    Wanted::Save => this.store_profile(&entry, &profile, cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// File a fetched profile in the preset folder under [`preset_name`]. A
    /// profile is ten gains against the ISO octaves, so the preset is written
    /// the way applying it would leave the window: each band on its own
    /// octave, one octave wide. The profile's preamp rides along for whatever
    /// else reads the file, since rox has no trim of its own.
    fn store_profile(
        &mut self,
        entry: &AutoEqEntry,
        profile: &autoeq::AutoEqProfile,
        cx: &mut Context<Self>,
    ) {
        let bands: Vec<BandSetting> = autoeq::BAND_HZ
            .iter()
            .zip(profile.gains_db)
            .map(|(&hz, gain_db)| BandSetting {
                hz,
                gain_db,
                q: autoeq::Q_OCTAVE,
            })
            .collect();

        match eq_presets::save(&preset_name(entry), &bands, profile.preamp_db, cx) {
            Some(saved) => {
                self.applied_info = Some(rox_i18n::t!("autoeq-saved-info", name = saved.clone()));
                self.saved = eq_presets::list();
            }
            None => self.error = Some(rox_i18n::t!("autoeq-save-failed")),
        }
    }

    /// Whether this entry is already sitting in the preset folder. Matched on
    /// the name the save would use, since that's what the folder holds.
    fn is_saved(&self, entry: &AutoEqEntry) -> bool {
        let stem = rox_core::settings::safe_file_stem(&preset_name(entry), "preset");
        self.saved.contains(&stem)
    }

    /// Prompt to import a local preset file (Equalizer APO .txt, GraphicEQ, or CSV).
    fn import_file(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });

        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = rx.await
                && let Some(path) = paths.pop()
            {
                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| rox_i18n::t!("autoeq-import-untitled").to_string());

                let text = match std::fs::read_to_string(&path) {
                    Ok(t) => t,
                    Err(e) => {
                        this.update(cx, |this, cx| {
                            this.error =
                                Some(rox_i18n::t!("autoeq-read-failed", reason = e.to_string()));
                            cx.notify();
                        })
                        .ok();
                        return;
                    }
                };

                let profile = match autoeq::parse_profile(&name, &text) {
                    Ok(p) => p,
                    Err(e) => {
                        this.update(cx, |this, cx| {
                            this.error = Some(rox_i18n::t!("autoeq-parse-failed", reason = e));
                            cx.notify();
                        })
                        .ok();
                        return;
                    }
                };

                this.update(cx, |this, cx| {
                    player::apply_graphic_eq(&profile.gains_db, cx);
                    player::set_eq_enabled(true, cx);
                    this.applied_path = None;
                    this.applied_info = Some(match profile.preamp_db {
                        Some(db) => rox_i18n::t!(
                            "autoeq-imported-info-preamp",
                            name = name,
                            db = format!("{db:+.1}"),
                        ),
                        None => rox_i18n::t!("autoeq-imported-info", name = name),
                    });
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Header search bar with input, search icon, and file import action.
    fn search_row(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(tokens::SPACE_XS)
            .p(tokens::SPACE_SM)
            .border_b_1()
            .border_color(palette::border())
            .child(div().flex_1().min_w_0().child(Input::new(&self.find)))
            .child(
                Button::new("autoeq-search-btn")
                    .icon(Icon::default().path(icons::SEARCH))
                    .tooltip(rox_i18n::t!("autoeq-search"))
                    .on_click(cx.listener(|this, _, _, cx| this.on_search_change(cx))),
            )
            .child(
                Button::new("autoeq-import-btn")
                    .icon(Icon::default().path(icons::DOWNLOAD))
                    .tooltip(rox_i18n::t!("autoeq-import-file"))
                    .on_click(cx.listener(|this, _, _, cx| this.import_file(cx))),
            )
    }

    /// Status banner when loading, applied, or errored.
    fn status_row(&self) -> Option<Div> {
        if self.loading_index {
            return Some(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .px(tokens::SPACE_SM)
                    .py(tokens::SPACE_XS)
                    .bg(palette::bg_control())
                    .border_b_1()
                    .border_color(palette::border())
                    .child(Spinner::new())
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette::text_muted())
                            .child(rox_i18n::t!("autoeq-loading")),
                    ),
            );
        }

        if let Some(err) = &self.error {
            return Some(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(tokens::SPACE_XS)
                    .px(tokens::SPACE_SM)
                    .py(tokens::SPACE_XS)
                    .bg(palette::alpha(palette::tone_bad(), 0x18))
                    .border_b_1()
                    .border_color(palette::border())
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette::tone_bad())
                            .child(err.clone()),
                    ),
            );
        }

        if let Some(info) = &self.applied_info {
            return Some(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(tokens::SPACE_XS)
                    .px(tokens::SPACE_SM)
                    .py(tokens::SPACE_XS)
                    .bg(palette::alpha(palette::accent(), 0x14))
                    .border_b_1()
                    .border_color(palette::border())
                    .child(
                        div()
                            .text_color(palette::accent())
                            .child(Icon::default().path(icons::CHECK)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette::accent())
                            .child(info.clone()),
                    ),
            );
        }

        None
    }

    /// Render the filtered list of headphone profile results.
    fn results(&self, cx: &mut Context<Self>) -> Div {
        let centered = |line: SharedString| {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .p(tokens::SPACE_MD)
                .text_sm()
                .text_center()
                .text_color(palette::text_muted())
                .child(line)
        };

        if self.loading_index && self.entries.is_empty() {
            return centered(rox_i18n::t!("autoeq-loading"));
        }

        if self.filtered.is_empty() {
            if self.searched_query.is_empty() {
                return centered(rox_i18n::t!("autoeq-placeholder"));
            }
            return centered(rox_i18n::t!("autoeq-none", text = &self.searched_query));
        }

        let rows: Vec<AnyElement> = self
            .filtered
            .iter()
            .enumerate()
            .filter_map(|(pos, &entry_ix)| {
                let entry = self.entries.get(entry_ix)?;
                Some(self.result_row(pos, entry, cx))
            })
            .collect();

        div().flex_1().min_h_0().w_full().flex().flex_col().child(
            div()
                .id("autoeq-results")
                .size_full()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .children(rows),
        )
    }

    /// One row representing an AutoEq entry: what it is, and the two things
    /// to do with it. Save is beside Apply rather than behind it because the
    /// reason to keep one is to have it without the network, and that's a
    /// decision made while looking at the list.
    fn result_row(&self, pos: usize, entry: &AutoEqEntry, cx: &mut Context<Self>) -> AnyElement {
        let is_applied = self.applied_path.as_deref() == Some(&entry.path);
        let is_applying = self.applying == Some(pos);
        let is_saving = self.saving == Some(pos);

        let spinner = || {
            div()
                .flex_none()
                .flex()
                .items_center()
                .px(tokens::SPACE_SM)
                .child(Spinner::new())
                .into_any_element()
        };
        let done = |label: SharedString| {
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(tokens::SPACE_XS)
                .text_xs()
                .text_color(palette::accent())
                .child(Icon::default().path(icons::CHECK))
                .child(label)
                .into_any_element()
        };

        let apply: AnyElement = if is_applying {
            spinner()
        } else if is_applied {
            done(rox_i18n::t!("autoeq-applied"))
        } else {
            Button::new(("autoeq-apply", pos))
                .label(rox_i18n::t!("autoeq-apply"))
                .small()
                .outline()
                .on_click(
                    cx.listener(move |this, _, _, cx| this.fetch_entry(pos, Wanted::Apply, cx)),
                )
                .into_any_element()
        };
        let save: AnyElement = if is_saving {
            spinner()
        } else if self.is_saved(entry) {
            done(rox_i18n::t!("autoeq-saved"))
        } else {
            Button::new(("autoeq-save", pos))
                .label(rox_i18n::t!("autoeq-save"))
                .small()
                .outline()
                .on_click(
                    cx.listener(move |this, _, _, cx| this.fetch_entry(pos, Wanted::Save, cx)),
                )
                .into_any_element()
        };
        let action = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(tokens::SPACE_XS)
            .child(apply)
            .child(save);

        div()
            .id(("autoeq-row", pos))
            .flex()
            .items_center()
            .justify_between()
            .gap(tokens::SPACE_SM)
            .px(tokens::SPACE_SM)
            .py(tokens::SPACE_XS)
            .w_full()
            .min_w_0()
            .hover(|row| row.bg(palette::bg_control_hover()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        div()
                            .truncate()
                            .text_sm()
                            .text_color(palette::text())
                            .child(SharedString::from(entry.name.clone())),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(palette::text_muted())
                            .child(SharedString::from(entry.source.clone())),
                    ),
            )
            .child(action)
            .into_any_element()
    }
}

impl gpui::Render for AutoEqWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut base = div()
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(palette::bg_root())
            .child(self.search_row(cx));

        if let Some(status) = self.status_row() {
            base = base.child(status);
        }

        base.child(self.results(cx))
    }
}
