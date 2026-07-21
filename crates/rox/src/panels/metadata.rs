//! The metadata panel: the current track's tags laid out as a sheet -
//! title and artist up top, then the labeled fields the library carries
//! (album, genre, year, duration, codec, bitrate). Which track is per-view
//! config through [`crate::source::TrackSource`], the cover panel's knob,
//! so a duplicate can watch each. The background can carry the track's
//! cover art, cropped to fill and dimmed under a scrim so the fields keep
//! reading; art comes off the file on a background thread like the cover
//! panel's and is retired the same way when the track moves on.
//!
//! The sheet has an edit face, the pencil in the title row: the tag
//! fields become inputs over a baseline read off the file itself, and a
//! save commits only the fields that moved against it, through the
//! writer's atomic layer. A successful commit lands in the catalog too,
//! so the library shows the edit without a rescan.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    div, img, prelude::*, px, App, Context, Div, Entity, EventEmitter, FocusHandle, Focusable,
    Image, ImageFormat, KeyDownEvent, ObjectFit, SharedString, Subscription, WeakEntity, Window,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::{Icon, Sizable};
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::writer::{self, Change, Field};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{self, align_row, justify, Align, AppState, PanelChrome, PanelSettings};
use crate::panel_settings;
use crate::panels::library::LibraryEvent;
use crate::player::fmt_time;
use crate::providers;
use crate::selection::SelectionEvent;
use crate::settings_ui;
use crate::source::{self, ResolvedTrack, TrackSource};

/// The metadata panel's per-view config: what a saved layout restores, and
/// what the settings window edits. Missing fields take the defaults, so a
/// layout dumped before a knob existed still loads.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MetadataConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub source: TrackSource,
    pub align: Align,
    /// The track's cover art behind the fields, dimmed under a scrim.
    pub cover: bool,
}

impl Default for MetadataConfig {
    fn default() -> Self {
        MetadataConfig {
            chrome: PanelChrome::default(),
            source: TrackSource::default(),
            align: Align::default(),
            cover: true,
        }
    }
}

/// The shown track's full projection row, owned so it outlives the borrow
/// of the library.
#[derive(Clone)]
struct Details {
    title: String,
    artist: String,
    album_artist: String,
    album: String,
    genre: String,
    year: u16,
    disc_no: u16,
    track_no: u16,
    duration_ms: u32,
    codec: String,
    bitrate_kbps: u16,
}

/// The editable fields in sheet order, each with its input row's label:
/// the tags the panel shows plus the comment, which only lives in the
/// file. Duration, codec, and bitrate stay display-only, they describe
/// the stream.
const EDIT_FIELDS: &[(Field, &str)] = &[
    (Field::Title, "Title"),
    (Field::Artist, "Artist"),
    (Field::Album, "Album"),
    (Field::AlbumArtist, "Album Artist"),
    (Field::DiscNo, "Disc"),
    (Field::TrackNo, "Track"),
    (Field::Genre, "Genre"),
    (Field::Year, "Year"),
    (Field::Comment, "Comment"),
];

/// One in-progress edit: the pinned track, the baseline read off its
/// file, and one input per entry of [`EDIT_FIELDS`]. Lives only while
/// edit mode is on.
struct EditState {
    path: PathBuf,
    /// The named fields as the writer read them, what save diffs
    /// against; None until the read lands (or never, on a file the
    /// writer cannot parse), and save stays inert without it.
    baseline: Option<Vec<(Field, String)>>,
    inputs: Vec<Entity<InputState>>,
    /// A failed read or commit, shown inline over the buttons.
    error: Option<SharedString>,
    /// A commit is in flight; the buttons hold still until it lands.
    saving: bool,
    _input_events: Vec<Subscription>,
}

pub struct MetadataPanel {
    state: AppState,
    config: MetadataConfig,
    /// The in-progress edit while the sheet shows its edit face.
    edit: Option<EditState>,
    /// The shown path's row, or None inside for a file the library does
    /// not know. Cached because the pump notifies every frame and the row
    /// lookup scans the projection; cleared when the catalog changes.
    details: Option<(PathBuf, Option<Details>)>,
    /// The loaded background art keyed by the track it belongs to; None
    /// inside means the track has no art.
    art: Option<(PathBuf, Option<Arc<Image>>)>,
    /// The track a load is running for, so a render can tell "already
    /// fetching" from "needs a fetch".
    pending: Option<PathBuf>,
    /// The cached source resolve, so the pump's per-frame notifies never
    /// turn into selection lookups.
    resolved: ResolvedTrack,
    /// Discards stale load results when the track changes mid-read.
    generation: u64,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    _player_changed: Subscription,
    _selection_changed: Subscription,
    _library_changed: Subscription,
}

impl MetadataPanel {
    pub fn new(state: AppState, config: MetadataConfig, cx: &mut Context<Self>) -> Self {
        // The tags and details turn over with the track, not as it plays,
        // so the gated observe skips the pump's per-tick repaints.
        let _player_changed = crate::player::observe_view(&state.player, cx);
        let _selection_changed = cx.subscribe(
            &state.selection,
            |this: &mut Self, _, _: &SelectionEvent, cx| {
                this.resolved.invalidate();
                cx.notify();
            },
        );
        // A rescan can rewrite tags, art files, and id -> path mappings;
        // drop the caches so the resolve, the row, and the art re-read.
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if !matches!(event, LibraryEvent::Updated) {
                    return;
                }
                this.resolved.invalidate();
                this.details = None;
                let old = this.art.take().and_then(|(_, art)| art);
                this.retire(old, cx);
                cx.notify();
            },
        );
        MetadataPanel {
            state,
            config,
            edit: None,
            details: None,
            art: None,
            pending: None,
            resolved: ResolvedTrack::default(),
            generation: 0,
            focus: cx.focus_handle(),
            tab_panel: None,
            _player_changed,
            _selection_changed,
            _library_changed,
        }
    }

    /// The shown path's row, from the cache or one projection scan on a
    /// miss. None for a file the library does not know or while the
    /// projection is still loading.
    fn details_for(&mut self, path: &Path, cx: &App) -> Option<&Details> {
        if self.details.as_ref().map(|(p, _)| p.as_path()) != Some(path) {
            let library = self.state.library.read(cx);
            let details = library.id_for(path).and_then(|id| {
                let projection = library.projection()?;
                let row = projection.db_id.iter().position(|&db_id| db_id == id)?;
                let v = projection.resolve(row as u32);
                Some(Details {
                    title: v.title.to_owned(),
                    artist: v.artist.to_owned(),
                    album_artist: v.album_artist.to_owned(),
                    album: v.album.to_owned(),
                    genre: v.genre.to_owned(),
                    year: v.year,
                    disc_no: v.disc_no,
                    track_no: v.track_no,
                    duration_ms: v.duration_ms,
                    codec: v.codec.to_owned(),
                    bitrate_kbps: v.bitrate_kbps,
                })
            });
            self.details = Some((path.to_path_buf(), details));
        }
        self.details
            .as_ref()
            .and_then(|(_, details)| details.as_ref())
    }

    /// Make sure the background art for `path` is cached or on its way:
    /// read the file off the UI thread and swap the result in when done.
    fn ensure_art(&mut self, path: &Path, cx: &mut Context<Self>) {
        if self.art.as_ref().map(|(p, _)| p.as_path()) == Some(path)
            || self.pending.as_deref() == Some(path)
        {
            return;
        }
        self.pending = Some(path.to_path_buf());
        self.generation += 1;
        let generation = self.generation;
        let path = path.to_path_buf();
        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move {
                        rox_library::art::cover_art(&path).and_then(|(bytes, mime)| {
                            let format = ImageFormat::from_mime_type(&mime)?;
                            Some(Arc::new(Image::from_bytes(format, bytes)))
                        })
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.pending = None;
                let old = this.art.take().and_then(|(_, art)| art);
                this.art = Some((path, loaded));
                this.retire(old, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Drop a replaced background's decoded bitmap from gpui's asset cache,
    /// unless the same art is what the panel holds now. Same reason as the
    /// cover panel's retire: `img` keeps every distinct decode in the
    /// process-wide asset cache and never evicts on its own.
    fn retire(&self, art: Option<Arc<Image>>, cx: &mut App) {
        let Some(old) = art else { return };
        if let Some((_, Some(current))) = &self.art {
            if current.id() == old.id() {
                return;
            }
        }
        old.remove_asset(cx);
    }

    /// The panel's own dropdown entries: the source pick and the cover
    /// background toggle, the same knobs the customize window edits.
    fn config_menu(
        &self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let menu = source::source_flyout(
            menu,
            |this: &Self| this.config.source,
            &cx.entity(),
            |this, source, cx| {
                this.config.source = source;
                cx.notify();
            },
            window,
            cx,
        );
        let weak = cx.entity().downgrade();
        menu.separator().item(
            PopupMenuItem::new("Cover Background")
                .checked(self.config.cover)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.config.cover = !this.config.cover;
                        cx.notify();
                    });
                }),
        )
    }

    /// The title-row pencil: into edit mode, or back out of it.
    fn toggle_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit.is_some() {
            self.close_edit(cx);
        } else {
            self.start_edit(window, cx);
        }
    }

    /// Open edit mode on the shown track: one input per field, filled
    /// once the writer's read lands off the UI thread. The path pins
    /// here, so a Playing source that moves on mid-edit does not steal
    /// the form.
    fn start_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit.is_some() {
            return;
        }
        let Some(path) = self.resolved.get(self.config.source, &self.state, cx) else {
            return;
        };
        let inputs: Vec<Entity<InputState>> = EDIT_FIELDS
            .iter()
            .map(|_| cx.new(|cx| InputState::new(window, cx)))
            .collect();
        // Enter in any input saves; Escape lands on the sheet's wrapper.
        let _input_events = inputs
            .iter()
            .map(|input| {
                cx.subscribe(input, |this: &mut Self, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.save_edit(cx);
                    }
                })
            })
            .collect();
        window.focus(&inputs[0].read(cx).focus_handle(cx));
        self.edit = Some(EditState {
            path: path.clone(),
            baseline: None,
            inputs,
            error: None,
            saving: false,
            _input_events,
        });
        panel::refresh_tab_panel(&self.tab_panel, cx);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let read = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move { writer::read(&path) }
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                let Some(edit) = &mut this.edit else { return };
                if edit.path != path {
                    return;
                }
                match read {
                    Ok(fields) => {
                        for ((field, _), input) in EDIT_FIELDS.iter().zip(&edit.inputs) {
                            // Multi-value tags show their first item, the
                            // same one the writer's verify reads back.
                            let value = fields
                                .iter()
                                .find(|(f, _)| f == field)
                                .map(|(_, v)| v.clone())
                                .unwrap_or_default();
                            input.update(cx, |input, cx| input.set_value(value, window, cx));
                        }
                        edit.baseline = Some(fields);
                    }
                    Err(e) => edit.error = Some(e.into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Drop the edit face without writing anything.
    fn close_edit(&mut self, cx: &mut Context<Self>) {
        self.edit = None;
        panel::refresh_tab_panel(&self.tab_panel, cx);
        cx.notify();
    }

    /// Commit the fields that moved against the baseline, through the
    /// writer's atomic layer off the UI thread. Nothing moved closes the
    /// form; a failed commit keeps it open with the error inline, the
    /// file untouched. Success hands the changes to the catalog, so the
    /// projection follows without a rescan.
    fn save_edit(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = &mut self.edit else { return };
        // No baseline means nothing safe to diff against: the read is
        // still running, or the file defeated it.
        let (Some(baseline), false) = (&edit.baseline, edit.saving) else {
            return;
        };
        let mut changes = Vec::new();
        for ((field, _), input) in EDIT_FIELDS.iter().zip(&edit.inputs) {
            let value = input.read(cx).value().to_string();
            let original = baseline
                .iter()
                .find(|(f, _)| f == field)
                .map(|(_, v)| v.as_str())
                .unwrap_or("");
            if value == original {
                continue;
            }
            changes.push(Change {
                field: field.clone(),
                value: (!value.is_empty()).then_some(value),
            });
        }
        if changes.is_empty() {
            self.close_edit(cx);
            return;
        }
        edit.saving = true;
        edit.error = None;
        let path = edit.path.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    let changes = changes.clone();
                    async move { writer::commit(&path, &changes) }
                })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        if this.edit.as_ref().is_some_and(|edit| edit.path == path) {
                            this.edit = None;
                            panel::refresh_tab_panel(&this.tab_panel, cx);
                        }
                        this.state
                            .library
                            .update(cx, |library, cx| library.apply_edit(&path, &changes, cx));
                    }
                    Err(e) => {
                        if let Some(edit) = &mut this.edit {
                            if edit.path == path {
                                edit.saving = false;
                                edit.error = Some(e.into());
                            }
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

impl PanelSettings for MetadataPanel {
    fn state(&self) -> AppState {
        self.state.clone()
    }

    fn chrome(&self) -> &PanelChrome {
        &self.config.chrome
    }

    fn chrome_mut(&mut self) -> &mut PanelChrome {
        &mut self.config.chrome
    }

    fn set_custom_title(&mut self, title: Option<String>, cx: &mut Context<Self>) {
        self.config.chrome.title = title;
        panel::refresh_tab_panel(&self.tab_panel, cx);
        cx.notify();
    }

    fn pages(&self) -> &'static [(&'static str, &'static str)] {
        &[("Content", icons::FILE_TEXT)]
    }

    fn page(
        &mut self,
        _page: &'static str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(source::source_row(
                self.config.source,
                |this: &mut Self, source, cx| {
                    this.config.source = source;
                    cx.notify();
                },
                cx,
            ))
            .child(align_row(
                self.config.align,
                |this: &mut Self, align, cx| {
                    this.config.align = align;
                    cx.notify();
                },
                cx,
            ))
            .child(panel::setting_row(
                "Cover Background",
                Some("The track's cover art behind the fields"),
                panel::toggle(
                    self.config.cover,
                    |this: &mut Self, on, cx| {
                        this.config.cover = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .into_any_element()
    }
}

impl EventEmitter<PanelEvent> for MetadataPanel {}

impl Focusable for MetadataPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for MetadataPanel {
    fn panel_name(&self) -> &'static str {
        "metadata"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(self.config.chrome.title.as_deref(), "Metadata")
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.config.chrome.title.clone().map(SharedString::from)
    }

    /// The edit toggle shares the title bar row, the library's move.
    /// Hidden while the panel shows no track; lit while an edit is open.
    fn title_suffix(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let editing = self.edit.is_some();
        if !editing
            && self
                .resolved
                .get(self.config.source, &self.state, cx)
                .is_none()
        {
            return None;
        }
        let weak = cx.entity().downgrade();
        Some(
            settings_ui::icon_button(icons::PENCIL, false, move |_, window, cx| {
                let Some(this) = weak.upgrade() else { return };
                this.update(cx, |this, cx| this.toggle_edit(window, cx));
            })
            .when(editing, |d| d.bg(palette::bg_control_active())),
        )
    }

    fn locked(&self, _cx: &App) -> bool {
        self.config.chrome.locked
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }

    /// The layout dump carries the panel's config; the builder registered
    /// in `workspace::register_panels` reads it back.
    fn min_size(&self, _cx: &App) -> gpui::Size<gpui::Pixels> {
        crate::panel::chrome_min_size(
            &self.config.chrome,
            gpui::size(
                rox_dock::resizable::PANEL_MIN_SIZE,
                rox_dock::resizable::PANEL_MIN_SIZE,
            ),
        )
    }

    fn max_size(&self, cx: &App) -> gpui::Size<gpui::Pixels> {
        crate::panel::chrome_max_size(&self.config.chrome, self.min_size(cx))
    }

    fn dump(&self, _cx: &App) -> rox_dock::PanelState {
        let mut state = rox_dock::PanelState::new(self);
        state.info = rox_dock::PanelInfo::panel(
            serde_json::to_value(self.config.clone()).unwrap_or(serde_json::Value::Null),
        );
        state
    }

    fn on_added_to(
        &mut self,
        tab_panel: WeakEntity<TabPanel>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tab_panel = Some(tab_panel.clone());
        self.state
            .tab_hosts
            .update(cx, |hosts, _| hosts.report(tab_panel));
    }

    fn on_removed(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.tab_panel = None;
    }

    fn dropdown_menu(
        &mut self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        // The config block: the panel's quick entries and the settings
        // window, apart from the core panel items.
        let menu = self.config_menu(menu, window, cx);
        // The online lookup, gated with the provider toggle so the menu
        // never offers a search that can't run. Opens the compare window;
        // the write waits for a confirmed, field-by-field pick.
        let menu = match (
            providers::metadata_online(),
            self.resolved.get(self.config.source, &self.state, cx),
        ) {
            (true, Some(path)) => {
                let library = self.state.library.clone();
                let now_art = self.state.now_art.clone();
                menu.separator().item(
                    PopupMenuItem::new("Find Metadata Online...")
                        .icon(Icon::default().path(icons::DOWNLOAD))
                        .on_click(move |_, _, cx| {
                            crate::tag_match::open(
                                library.clone(),
                                now_art.clone(),
                                path.clone(),
                                cx,
                            );
                        }),
                )
            }
            _ => menu,
        };
        let menu = panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity());
        // Duplicate hand-rolled rather than through `panel::duplicate_item`
        // because the copy takes the config along, like the transports'.
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new("Duplicate")
                .icon(Icon::default().path(icons::COPY))
                .on_click(move |_, window, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    let (state, config, tabs) = {
                        let panel = this.read(cx);
                        (
                            panel.state.clone(),
                            panel.config.clone(),
                            panel.tab_panel.clone(),
                        )
                    };
                    let Some(tabs) = tabs.and_then(|tabs| tabs.upgrade()) else {
                        return;
                    };
                    let dup = cx.new(|cx| MetadataPanel::new(state, config, cx));
                    tabs.update(cx, |tabs, cx| tabs.add_panel(Arc::new(dup), window, cx));
                }),
        );
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
        )
    }
}

/// One labeled field of the sheet: the tag's name dimmed in a fixed
/// column, its value truncating beside it.
fn field(label: &'static str, value: String) -> Div {
    div()
        .flex()
        .flex_row()
        .gap(tokens::SPACE_SM)
        .child(
            div()
                .w(px(84.))
                .flex_none()
                .text_color(palette::text_muted())
                .child(label),
        )
        .child(div().min_w_0().truncate().child(value))
}

impl Render for MetadataPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(cx))
    }
}

impl MetadataPanel {
    fn body(&mut self, cx: &mut Context<Self>) -> Div {
        // The edit toggle lives in the tab bar via title_suffix while the
        // panel shares a group; solo or popped out there is no header at
        // all, so it renders as a toolbar in the body instead, the
        // library's move.
        let headerless = self
            .tab_panel
            .as_ref()
            .and_then(|tabs| tabs.upgrade())
            .is_none_or(|tabs| tabs.read(cx).panels_count() < 2);
        // Same show rule as the suffix: hidden while the panel shows no
        // track, unless an edit is already open.
        let show_toggle = self.edit.is_some()
            || self
                .resolved
                .get(self.config.source, &self.state, cx)
                .is_some();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette::bg_root())
            .when(headerless && show_toggle, |d| d.child(self.toolbar(cx)))
            .child(self.sheet_body(cx).flex_1().min_h_0())
    }

    /// Solo or popped out there is no title bar to host the edit toggle,
    /// so it renders as a toolbar row above the sheet instead, the
    /// library's move.
    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let editing = self.edit.is_some();
        let weak = cx.entity().downgrade();
        div()
            .flex_none()
            .h(px(36.))
            .px(tokens::SPACE_SM)
            .flex()
            .flex_row()
            .items_center()
            .justify_end()
            .bg(palette::bg_toolbar())
            .border_b_1()
            .border_color(palette::border())
            .child(
                settings_ui::icon_button(icons::PENCIL, false, move |_, window, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| this.toggle_edit(window, cx));
                })
                .when(editing, |d| d.bg(palette::bg_control_active())),
            )
    }

    /// The sheet under the toolbar: the display face, or the edit face
    /// while an edit is open.
    fn sheet_body(&mut self, cx: &mut Context<Self>) -> Div {
        let align = self.config.align;
        let root = div().relative();

        // An open edit pins its track; the source only drives the sheet
        // while nothing is being edited.
        let Some(path) = self
            .edit
            .as_ref()
            .map(|edit| edit.path.clone())
            .or_else(|| self.resolved.get(self.config.source, &self.state, cx))
        else {
            // The source points at no track: a quiet line where the sheet
            // would sit.
            return root.child(
                justify(div().absolute().inset_0().flex().items_center(), align)
                    .p(tokens::SPACE_MD)
                    .child(div().text_color(palette::text_faint()).child("No track")),
            );
        };

        // The background layer: the track's art cropped to fill, a scrim
        // over it so the fields keep reading over busy covers. Until the
        // load lands the plain background stands in; no fade, the sheet's
        // text swaps in the same frame anyway.
        if self.config.cover {
            self.ensure_art(&path, cx);
        }
        let backdrop = self
            .config
            .cover
            .then(|| {
                self.art
                    .as_ref()
                    .filter(|(cached, _)| *cached == path)
                    .and_then(|(_, art)| art.clone())
            })
            .flatten();
        let root = root.when_some(backdrop, |root, image| {
            root.child(
                div()
                    .absolute()
                    .inset_0()
                    .child(img(image).object_fit(ObjectFit::Cover).size_full()),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(palette::alpha(palette::bg_root(), 0xB8)),
            )
        });

        if self.edit.is_some() {
            return root.child(
                justify(div().absolute().inset_0().flex().items_center(), align)
                    .child(self.edit_sheet(cx)),
            );
        }

        // An untagged file still shows something: its file name for the
        // title, no fields.
        let details = self.details_for(&path, cx).cloned();
        let title = details
            .as_ref()
            .map(|d| d.title.clone())
            .unwrap_or_else(|| {
                path.file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string())
            });

        let mut fields: Vec<(&'static str, String)> = Vec::new();
        if let Some(d) = &details {
            if !d.album.is_empty() {
                fields.push(("Album", d.album.clone()));
            }
            if !d.album_artist.is_empty() && d.album_artist != d.artist {
                fields.push(("Album Artist", d.album_artist.clone()));
            }
            if d.disc_no > 0 {
                fields.push(("Disc", d.disc_no.to_string()));
            }
            if d.track_no > 0 {
                fields.push(("Track", format!("{:02}", d.track_no)));
            }
            if !d.genre.is_empty() {
                fields.push(("Genre", d.genre.clone()));
            }
            if d.year > 0 {
                fields.push(("Year", d.year.to_string()));
            }
            if d.duration_ms > 0 {
                fields.push(("Duration", fmt_time(d.duration_ms as f64 / 1000.0)));
            }
            if !d.codec.is_empty() {
                fields.push(("Codec", d.codec.clone()));
            }
            if d.bitrate_kbps > 0 {
                fields.push(("Bitrate", format!("{} kbps", d.bitrate_kbps)));
            }
        }
        let artist = details
            .as_ref()
            .map(|d| d.artist.clone())
            .filter(|a| !a.is_empty());

        // The sheet: title over artist, the fields below, placed by the
        // alignment knob and centered vertically like the cover.
        let sheet = div()
            .max_w_full()
            .min_w_0()
            .p(tokens::SPACE_MD)
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                div()
                    .text_lg()
                    .text_color(palette::text_bright())
                    .max_w_full()
                    .truncate()
                    .child(title),
            )
            .when_some(artist, |d, artist| {
                d.child(
                    div()
                        .text_color(palette::text_muted())
                        .max_w_full()
                        .truncate()
                        .child(artist),
                )
            })
            .when(!fields.is_empty(), |d| {
                d.child(
                    div()
                        .mt(tokens::SPACE_MD)
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .children(fields.into_iter().map(|(label, value)| field(label, value))),
                )
            });

        root.child(justify(div().absolute().inset_0().flex().items_center(), align).child(sheet))
    }

    /// The sheet's edit face: one input per editable field, the save and
    /// cancel row under them, and whatever error the last read or commit
    /// left. Enter saves through the inputs' own event; Escape cancels
    /// here, where the widget propagates it.
    fn edit_sheet(&self, cx: &mut Context<Self>) -> Div {
        let Some(edit) = &self.edit else {
            return div();
        };
        let rows = EDIT_FIELDS
            .iter()
            .zip(&edit.inputs)
            .map(|((_, label), input)| {
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .child(
                        div()
                            .w(px(84.))
                            .flex_none()
                            .text_color(palette::text_muted())
                            .child(*label),
                    )
                    .child(div().flex_1().min_w_0().child(Input::new(input).small()))
            });
        div()
            // Scopes the workspace's playback key bindings out while an
            // input is focused, so space and arrows type instead.
            .key_context("SearchInput")
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key != "escape" {
                    return;
                }
                cx.stop_propagation();
                this.close_edit(cx);
            }))
            .w_full()
            .max_w(px(420.))
            .p(tokens::SPACE_MD)
            .flex()
            .flex_col()
            .gap(px(2.))
            .children(rows)
            .when_some(edit.error.clone(), |d, error| {
                d.child(
                    div()
                        .mt(tokens::SPACE_XS)
                        .text_color(palette::text_muted())
                        .child(error),
                )
            })
            .child(
                div()
                    .mt(tokens::SPACE_XS)
                    .flex()
                    .flex_row()
                    .gap(tokens::SPACE_SM)
                    .child(settings_ui::small_button(
                        "Save",
                        icons::CHECK,
                        edit.saving || edit.baseline.is_none(),
                        cx.listener(|this, _, _, cx| this.save_edit(cx)),
                    ))
                    .child(settings_ui::small_button(
                        "Cancel",
                        icons::CLOSE,
                        edit.saving,
                        cx.listener(|this, _, _, cx| this.close_edit(cx)),
                    )),
            )
    }
}
