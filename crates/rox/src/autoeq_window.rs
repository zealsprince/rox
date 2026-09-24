//! The AutoEq profile browser, opened from the Equalizer window: search the
//! cached AutoEq index locally, then fetch a profile's 10-band curve from
//! GitHub to apply or save as a preset.

use std::sync::Arc;

use gpui::{
    AnyElement, App, Bounds, Context, Div, Entity, EntityId, FocusHandle, Focusable as _, Global,
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
use rox_panel_api::panel;
use rox_services::player;

use crate::eq_presets;

const DEFAULT_SIZE: (f32, f32) = (560., 680.);

const MIN_SIZE: gpui::Size<gpui::Pixels> = gpui::Size {
    width: px(420.),
    height: px(320.),
};

const RESULT_LIMIT: usize = 100;

#[derive(Clone, Copy, PartialEq)]
enum Wanted {
    Apply,
    Save,
}

/// Model plus measurement source: the index holds several rigs per model, and
/// a preset named after the model alone would overwrite the others.
fn preset_name(entry: &AutoEqEntry) -> String {
    format!("{} ({})", entry.name, entry.source)
}

struct OpenAutoEq(WindowHandle<Root>);

impl Global for OpenAutoEq {}

/// Deferred: the opening action runs inside another window's update, and
/// reading the front workspace for the tint mid-update would panic.
pub fn open(cx: &mut App) {
    cx.defer(open_now);
}

fn open_now(cx: &mut App) {
    if let Some(open) = cx.try_global::<OpenAutoEq>() {
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
    let bounds = Bounds::centered(None, size(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1)), cx);
    let handle = rox_panel_api::panel::open_child_window(
        cx,
        rox_i18n::t!("autoeq-window-title"),
        bounds,
        Some(MIN_SIZE),
        move |window, cx| cx.new(|cx| AutoEqWindow::new(player, window, cx)),
    );

    cx.set_global(OpenAutoEq(handle));
}

struct AutoEqWindow {
    player: Option<EntityId>,
    find: Entity<InputState>,
    entries: Arc<Vec<AutoEqEntry>>,
    filtered: Vec<usize>,
    loading_index: bool,
    error: Option<SharedString>,
    applying: Option<usize>,
    saving: Option<usize>,
    applied_path: Option<String>,
    applied_info: Option<SharedString>,
    /// The preset folder's stems, read on open and after each save, so rows
    /// don't each read the directory.
    saved: Vec<String>,
    searched_query: String,
    focus: FocusHandle,
    _find_events: Subscription,
    _presets_changed: Subscription,
}

impl AutoEqWindow {
    fn new(player: Option<EntityId>, window: &mut Window, cx: &mut Context<Self>) -> Self {
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
            player,
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

    fn cache_path() -> std::path::PathBuf {
        settings::data_dir().join("autoeq_index.txt")
    }

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

    fn on_search_change(&mut self, cx: &mut Context<Self>) {
        let query = self.find.read(cx).value().trim().to_string();
        self.searched_query = query.clone();

        if self.entries.is_empty() {
            self.filtered.clear();
            cx.notify();
            return;
        }

        let hits = autoeq::filter_entries(&self.entries, &query, RESULT_LIMIT);
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
                        // Two whole messages: where the preamp sits in the sentence is the
                        // translator's call.
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

    /// Written as applying would leave the window: each gain on its ISO octave,
    /// one octave wide. The preamp rides along for other readers of the file.
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

    fn is_saved(&self, entry: &AutoEqEntry) -> bool {
        let stem = rox_core::settings::safe_file_stem(&preset_name(entry), "preset");
        self.saved.contains(&stem)
    }

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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // No workspace player: this window's own id reads the base palette.
        let player = self.player.unwrap_or_else(|| cx.entity().entity_id());
        palette::note_focus(player, window.is_window_active(), cx);
        // Build inside the closure: anything built outside paints untinted.
        panel::window_body(player, || {
            let mut base = div()
                .track_focus(&self.focus)
                .size_full()
                .flex()
                .flex_col()
                .bg(palette::bg_root())
                .text_color(palette::text())
                .text_sm()
                .child(self.search_row(cx));

            if let Some(status) = self.status_row() {
                base = base.child(status);
            }

            base.child(self.results(cx)).into_any_element()
        })
    }
}
