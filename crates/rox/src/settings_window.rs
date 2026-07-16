//! The settings window: one OS window opened from the menubar, a sidebar
//! of pages on the left and the picked page's sections on the right.
//! Appearance holds the song-theming switch, ADR 10's transparency pair,
//! and the palette editor, a labeled swatch grid per listing group;
//! Library manages the scanned folders over the shared catalog entity.
//! Edits land live through the palette setters and persist to the
//! settings file per change, the volume slider's cadence. The window
//! edits a working copy of the user palette, so the swatches show the
//! base even while a playing track's seed tints the app over it; while
//! song theming is on the editor locks, because the track is driving.
//! Palettes import and export as the settings map's role-to-hex JSON,
//! so a file, the settings entry, and a shared theme are one shape.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use gpui::{
    div, prelude::*, px, size, svg, AnyElement, App, Bounds, Context, Div, Entity, Global, Hsla,
    MouseButton, PathPromptOptions, Pixels, ScrollHandle, SharedString, Subscription,
    TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions,
};
use gpui_component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_component::scroll::{Scrollbar, ScrollbarShow};
use gpui_component::{Root, Sizable as _};

use crate::assets::icons;
use crate::backdrop::{NowPlayingArt, WindowBackdrop};
use crate::design::palette::{self, Palette, Role, ROLES};
use crate::design::tokens;
use crate::panel::{self, AppState, ScrubState};
use crate::panels::library::{Library, LibraryEvent};
use crate::settings::{data_dir, settings_path, Settings};
use crate::thumbs::Thumbs;
use rox_library::store::Stats;
use crate::settings_ui::{
    self, grid_columns, icon_button, section, sidebar, small_button, SECTION_GAP,
};

/// The folder table's fixed columns: the rollup numbers and the remove
/// control, the last sized to [`icon_button`]'s footprint so the header
/// aligns.
const TRACKS_COL_W: Pixels = px(56.);
const ALBUMS_COL_W: Pixels = px(56.);
const SIZE_COL_W: Pixels = px(72.);
const ACTION_COL_W: Pixels = px(22.);

/// The open settings window, if any: opening again focuses it instead
/// of stacking a second editor over the same file.
struct OpenSettings(WindowHandle<Root>);

impl Global for OpenSettings {}

/// Open the settings window, or bring the open one to the front. The
/// state carries the library for the Library page, which edits it live,
/// and the shared art bake for the window's own backdrop.
pub fn open(state: AppState, cx: &mut App) {
    if let Some(open) = cx.try_global::<OpenSettings>() {
        let handle = open.0;
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    let bounds = Bounds::centered(None, size(px(720.), px(520.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(settings_ui::MIN_SIZE),
        titlebar: Some(TitlebarOptions {
            title: Some("rox - settings".into()),
            ..Default::default()
        }),
        app_id: Some(crate::APP_ID.into()),
        ..Default::default()
    };
    let handle = cx
        .open_window(options, |window, cx| {
            // The Wayland backend ignores the creation-time titlebar
            // title; only set_window_title reaches the compositor.
            window.set_window_title("rox - settings");
            let view = cx.new(|cx| SettingsWindow::new(state, window, cx));
            cx.new(|cx| Root::new(view, window, cx))
        })
        .expect("failed to open the settings window");
    cx.set_global(OpenSettings(handle));
}

/// The sidebar's pages.
#[derive(Clone, Copy, PartialEq)]
enum Page {
    Appearance,
    Behavior,
    Library,
    Storage,
}

const PAGES: &[(Page, &str)] = &[
    (Page::Appearance, "Appearance"),
    (Page::Behavior, "Behavior"),
    (Page::Library, "Library"),
    (Page::Storage, "Storage"),
];

/// The storage page's measurements, taken entering the page and after a
/// clear rather than per frame: the stats and the cache walk are cheap
/// once, not every paint.
#[derive(Clone, Copy, Default)]
struct StorageInfo {
    /// The whole library's rollup: tracks, albums, bytes of music.
    music: Stats,
    /// library.db with its WAL sidecars.
    catalog: u64,
    /// thumbs.db with its WAL sidecars.
    thumbs: u64,
    /// Everything under waveforms/.
    waveforms: u64,
}

struct SettingsWindow {
    page: Page,
    /// The working copy of the user palette: what the swatches show and
    /// what edits write through [`palette::set`].
    base: Palette,
    art_theming: bool,
    surface_opacity: f32,
    backdrop_strength: f32,
    restore_last_track: bool,
    /// One picker per palette role, in [`ROLES`] order.
    pickers: Vec<Entity<ColorPickerState>>,
    surface_scrub: ScrubState,
    backdrop_scrub: ScrubState,
    /// The page body's scroll position, shared with the scrollbar so it
    /// can show how much page hangs below the fold.
    scroll: ScrollHandle,
    /// The shared catalog, the Library page's subject.
    library: Entity<Library>,
    /// The shared art bake and this window's slice of the backdrop, so
    /// the window backs with the playing track's art like every other.
    now_art: Entity<NowPlayingArt>,
    backdrop: WindowBackdrop,
    /// The shared thumbnail service, whose durable store the storage
    /// page sizes and clears.
    thumbs: Entity<Thumbs>,
    /// The storage page's numbers; None until the page is first opened.
    storage: Option<StorageInfo>,
    /// The folder list with per-folder rollups, recounted on every
    /// library event rather than per frame.
    root_stats: Vec<(PathBuf, Stats)>,
    _picker_changes: Vec<Subscription>,
    _library_changed: Subscription,
    /// Scan progress ticks notify the library without emitting Updated;
    /// the Library page's busy line needs those repaints too.
    _library_repaint: Subscription,
    /// This window pumps its own frames, so the backdrop needs its own
    /// wake on a new bake.
    _backdrop_changed: Subscription,
}

impl SettingsWindow {
    fn new(state: AppState, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let library = state.library;
        let settings = Settings::load();
        let base = settings.palette();
        let root_stats = library.read(cx).root_stats();
        let _library_changed = cx.subscribe(
            &library,
            |this: &mut Self, library, _: &LibraryEvent, cx| {
                this.root_stats = library.read(cx).root_stats();
                // A finished scan moves the storage numbers too; remeasure
                // if they are on screen.
                if this.page == Page::Storage {
                    this.refresh_storage(cx);
                }
                cx.notify();
            },
        );
        let _library_repaint = cx.observe(&library, |_, _, cx| cx.notify());
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        let mut pickers = Vec::with_capacity(ROLES.len());
        let mut _picker_changes = Vec::with_capacity(ROLES.len());
        for (index, role) in ROLES.iter().enumerate() {
            let picker =
                cx.new(|cx| ColorPickerState::new(window, cx).default_value((role.get)(&base)));
            _picker_changes.push(cx.subscribe_in(
                &picker,
                window,
                move |this, picker, event: &ColorPickerEvent, window, cx| {
                    let ColorPickerEvent::Change(color) = event;
                    this.role_edited(index, *color, picker, window, cx);
                },
            ));
            pickers.push(picker);
        }
        SettingsWindow {
            page: Page::Appearance,
            base,
            art_theming: settings.art_theming,
            surface_opacity: settings.surface_opacity,
            backdrop_strength: settings.backdrop_strength,
            restore_last_track: settings.restore_last_track,
            pickers,
            surface_scrub: ScrubState::default(),
            backdrop_scrub: ScrubState::default(),
            scroll: ScrollHandle::new(),
            library,
            now_art: state.now_art,
            backdrop: WindowBackdrop::default(),
            thumbs: state.thumbs,
            storage: None,
            root_stats,
            _picker_changes,
            _library_changed,
            _library_repaint,
            _backdrop_changed,
        }
    }

    /// A picker's change: the role into the working palette, out through
    /// the one setter, into the file. Clearing the hex field reads as
    /// back to the role's default.
    fn role_edited(
        &mut self,
        index: usize,
        color: Option<Hsla>,
        picker: &Entity<ColorPickerState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let role = &ROLES[index];
        match color {
            Some(color) => (role.set)(&mut self.base, color.to_rgb()),
            None => {
                let default = (role.get)(&Palette::default());
                (role.set)(&mut self.base, default);
                picker.update(cx, |picker, cx| picker.set_value(default, window, cx));
            }
        }
        palette::set(self.base, cx);
        let map = self.base.to_map();
        Settings::update(move |s| s.palette = map);
    }

    /// The song-theming switch: through the palette pipe, which also
    /// gates the backdrop layers, and into the file.
    fn set_art_theming(&mut self, on: bool, cx: &mut Context<Self>) {
        self.art_theming = on;
        palette::set_art_theming(on, cx);
        Settings::update(move |s| s.art_theming = on);
        cx.notify();
    }

    /// The restore switch: straight into the file. Launch reads it there,
    /// so the flip is live for the next start without touching playback.
    fn set_restore_last_track(&mut self, on: bool, cx: &mut Context<Self>) {
        self.restore_last_track = on;
        Settings::update(move |s| s.restore_last_track = on);
        cx.notify();
    }

    fn set_surface(&mut self, value: f32, cx: &mut Context<Self>) {
        self.surface_opacity = value;
        self.scalars_edited(cx);
    }

    fn set_backdrop(&mut self, value: f32, cx: &mut Context<Self>) {
        self.backdrop_strength = value;
        self.scalars_edited(cx);
    }

    fn scalars_edited(&mut self, cx: &mut Context<Self>) {
        palette::set_scalars(self.surface_opacity, self.backdrop_strength, cx);
        let (surface, backdrop) = (self.surface_opacity, self.backdrop_strength);
        Settings::update(move |s| {
            s.surface_opacity = surface;
            s.backdrop_strength = backdrop;
        });
        cx.notify();
    }

    /// A whole palette into the editor at once: the working copy, every
    /// picker, and the live palette. Persisting is the caller's, because
    /// reset writes an empty map where import writes a full one.
    fn apply_palette(&mut self, palette: Palette, window: &mut Window, cx: &mut Context<Self>) {
        self.base = palette;
        for (role, picker) in ROLES.iter().zip(&self.pickers) {
            let color = (role.get)(&self.base);
            picker.update(cx, |picker, cx| picker.set_value(color, window, cx));
        }
        palette::set(self.base, cx);
    }

    /// Back to the stock palette; the file's map empties rather than
    /// filling with defaults.
    fn reset_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_palette(Palette::default(), window, cx);
        Settings::update(|s| s.palette.clear());
    }

    /// Pick a palette file and load it: the same role-to-hex map the
    /// settings file holds, so exports, settings, and shared themes are
    /// one shape. Unknown roles and bad values fall away silently, a
    /// file that isn't a map at all is ignored.
    fn import_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.pop() else {
                return;
            };
            let Some(map) = std::fs::read_to_string(path)
                .ok()
                .and_then(|json| serde_json::from_str::<BTreeMap<String, String>>(&json).ok())
            else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                this.apply_palette(Palette::from_map(&map), window, cx);
                let map = this.base.to_map();
                Settings::update(move |s| s.palette = map);
            })
            .ok();
        })
        .detach();
    }

    /// Save a palette file, [`Palette::to_map`]'s shape: the working
    /// palette, or the derived one while song theming drives the colors,
    /// so a look a track built can leave as a theme.
    fn export_palette(&mut self, cx: &mut Context<Self>) {
        let map = if self.art_theming {
            palette::resolved().to_map()
        } else {
            self.base.to_map()
        };
        let home = dirs::home_dir().unwrap_or_default();
        let rx = cx.prompt_for_new_path(&home, Some("palette.json"));
        cx.spawn(async move |_, _| {
            let Ok(Ok(Some(path))) = rx.await else {
                return;
            };
            if let Ok(json) = serde_json::to_string_pretty(&map) {
                std::fs::write(path, json).ok();
            }
        })
        .detach();
    }

    fn appearance_page(&self, columns: usize, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(SECTION_GAP)
            .child(section(
                "theming",
                None,
                panel::setting_row(
                    "song theming",
                    Some("tint the palette and back windows with the playing track's cover art"),
                    panel::toggle(self.art_theming, Self::set_art_theming, cx),
                ),
            ))
            .child(section(
                "transparency",
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_MD)
                    .child(panel::setting_row(
                        "surface opacity",
                        Some("how opaque the app's surfaces read over the backdrop"),
                        settings_ui::slider(
                            &self.surface_scrub,
                            self.surface_opacity,
                            Self::set_surface,
                            cx,
                        ),
                    ))
                    .child(panel::setting_row(
                        "backdrop strength",
                        Some("how strongly the cover backdrop shows behind them"),
                        settings_ui::slider(
                            &self.backdrop_scrub,
                            self.backdrop_strength,
                            Self::set_backdrop,
                            cx,
                        ),
                    )),
            ))
            .child(self.colors_section(columns, cx))
    }

    fn behavior_page(&self, cx: &mut Context<Self>) -> Div {
        div().flex().flex_col().gap(SECTION_GAP).child(section(
            "startup",
            None,
            panel::setting_row(
                "restore last track",
                Some("launch with the last playing track loaded, paused where it left off"),
                panel::toggle(self.restore_last_track, Self::set_restore_last_track, cx),
            ),
        ))
    }

    /// One cell of the color grid: the picker with its label beside it,
    /// or a dimmed inert swatch while song theming drives the palette.
    /// The inert swatch shows the derived color the track landed on, the
    /// same values export saves, not the base underneath.
    fn color_cell(&self, role: &Role, picker: &Entity<ColorPickerState>, locked: bool) -> Div {
        let control: AnyElement = if locked {
            div()
                .size_5()
                .rounded(tokens::RADIUS)
                .border_1()
                .border_color(palette::border())
                .bg((role.get)(&palette::resolved()))
                .opacity(0.5)
                .into_any_element()
        } else {
            // The picker pads a 4px margin around its swatch square; the
            // counter-margin keeps the live cell the same 20px footprint
            // as the locked one, so the grid doesn't loosen when editable.
            ColorPicker::new(picker)
                .small()
                .m(px(-4.))
                .into_any_element()
        };
        settings_ui::color_cell(control, role.label, false, None)
    }

    fn colors_section(&self, columns: usize, cx: &mut Context<Self>) -> Div {
        let locked = self.art_theming;
        let mut body = div().flex().flex_col().gap(tokens::SPACE_XS);
        if locked {
            body = body.child(div().text_xs().text_color(palette::text_muted()).child(
                "song theming is on, so the playing track drives these colors \
                 and export saves them; turn it off above to edit them",
            ));
        }
        body = body.child(settings_ui::role_grid(columns, |j| {
            self.color_cell(&ROLES[j], &self.pickers[j], locked)
                .into_any_element()
        }));

        // Import and reset lock with the rest of the editor: they change
        // the palette too. Export stays live; unlocked it saves the base
        // palette, locked the derived one the swatches show.
        let controls = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .child(small_button(
                "import",
                icons::DOWNLOAD,
                locked,
                cx.listener(|this, _, window, cx| this.import_palette(window, cx)),
            ))
            .child(small_button(
                "export",
                icons::UPLOAD,
                false,
                cx.listener(|this, _, _, cx| this.export_palette(cx)),
            ))
            .child(small_button(
                "reset",
                icons::REFRESH_CW,
                locked,
                cx.listener(|this, _, window, cx| this.reset_palette(window, cx)),
            ));
        section("colors", Some(controls.into_any_element()), body)
    }

    /// One row of the folder table: the path, its rollup numbers, and a
    /// remove control, inert while a scan runs.
    fn folder_row(&self, root: &Path, stats: Stats, scanning: bool, cx: &mut Context<Self>) -> Div {
        let path: SharedString = root.to_string_lossy().into_owned().into();
        let remove = icon_button(icons::CLOSE, scanning, {
            let root = root.to_path_buf();
            cx.listener(move |this, _, _, cx| {
                this.library
                    .update(cx, |library, cx| library.remove_root(&root, cx));
            })
        });
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_MD)
            .py(tokens::SPACE_XS)
            .border_b_1()
            .border_color(palette::border())
            .child(div().flex_1().min_w_0().truncate().child(path))
            .child(number_cell(TRACKS_COL_W, stats.tracks.to_string()))
            .child(number_cell(ALBUMS_COL_W, stats.albums.to_string()))
            .child(number_cell(SIZE_COL_W, human_size(stats.bytes)))
            .child(remove)
    }

    fn library_page(&self, cx: &mut Context<Self>) -> Div {
        let busy = self.library.read(cx).busy();
        let scanning = busy.is_some();
        let mut body = div().flex().flex_col().gap(tokens::SPACE_SM).child(
            div().text_xs().text_color(palette::text_muted()).child(
                "folders scanned into the library; removing one drops its \
                 tracks from the catalog and leaves the files alone",
            ),
        );
        // The folder table: a column header line, then a hairlined row
        // per folder.
        let mut table = div().flex().flex_col().child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_MD)
                .pb(tokens::SPACE_XS)
                .border_b_1()
                .border_color(palette::border())
                .text_xs()
                .text_color(palette::text_muted())
                .child(div().flex_1().child("folder"))
                .child(
                    div()
                        .w(TRACKS_COL_W)
                        .flex_none()
                        .text_right()
                        .child("tracks"),
                )
                .child(
                    div()
                        .w(ALBUMS_COL_W)
                        .flex_none()
                        .text_right()
                        .child("albums"),
                )
                .child(div().w(SIZE_COL_W).flex_none().text_right().child("size"))
                .child(div().w(ACTION_COL_W).flex_none()),
        );
        if self.root_stats.is_empty() {
            table = table.child(
                div()
                    .py(tokens::SPACE_XS)
                    .text_color(palette::text_muted())
                    .child("no folders yet"),
            );
        }
        for (root, stats) in &self.root_stats {
            table = table.child(self.folder_row(root, *stats, scanning, cx));
        }
        body = body.child(table);

        // The library's badge and the file under the scan cursor, or the
        // resting status, under the table.
        let note: Option<SharedString> = busy.or_else(|| {
            let status = self.library.read(cx).status();
            (!status.is_empty()).then_some(status)
        });
        body = body.when_some(note, |d, note| {
            d.child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(note),
            )
        });

        // Add folder and rescan ride the section header like the colors
        // controls do.
        let controls = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .child(small_button(
                "add folder",
                icons::FOLDER_PLUS,
                scanning,
                cx.listener(|this, _, _, cx| {
                    this.library.update(cx, |library, cx| library.browse(cx));
                }),
            ))
            .child(small_button(
                "rescan",
                icons::REFRESH_CW,
                scanning || self.root_stats.is_empty(),
                cx.listener(|this, _, _, cx| {
                    this.library.update(cx, |library, cx| library.rescan(cx));
                }),
            ));
        section("folders", Some(controls.into_any_element()), body)
    }

    /// Measure everything the storage page shows: the library rollup on
    /// the UI-side connection, the databases and the waveform cache by
    /// stat. Cheap enough to run whole on page entry, too heavy per frame.
    fn refresh_storage(&mut self, cx: &mut Context<Self>) {
        let data = data_dir();
        self.storage = Some(StorageInfo {
            music: self.library.read(cx).stats(),
            catalog: db_size(&data.join("library.db")),
            thumbs: db_size(&data.join("thumbs.db")),
            waveforms: dir_size(&crate::peaks::cache_dir()),
        });
        cx.notify();
    }

    /// Empty the thumbnail store. The delete runs off the UI thread on
    /// the service's own connection, so it serializes against in-flight
    /// loads; the sizes refresh when it lands.
    fn clear_thumbs(&mut self, cx: &mut Context<Self>) {
        let Some(conn) = self.thumbs.read(cx).store_conn() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move { rox_library::thumbs::clear(&conn) })
                .await;
            this.update(cx, |this, cx| this.refresh_storage(cx)).ok();
        })
        .detach();
    }

    /// Drop the waveform cache; strips re-decode on their next play.
    fn clear_waveforms(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move { crate::peaks::clear() })
                .await;
            this.update(cx, |this, cx| this.refresh_storage(cx)).ok();
        })
        .detach();
    }

    fn storage_page(&self, cx: &mut Context<Self>) -> Div {
        let info = self.storage.unwrap_or_default();
        let music = format!(
            "{} tracks, {} albums, {}",
            info.music.tracks,
            info.music.albums,
            human_size(info.music.bytes)
        );
        div()
            .flex()
            .flex_col()
            .gap(SECTION_GAP)
            .child(section(
                "library",
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_MD)
                    .child(panel::setting_row(
                        "music files",
                        Some("what the scanned folders hold; the files stay where they are"),
                        readout(music),
                    ))
                    .child(panel::setting_row(
                        "catalog",
                        Some("the track index scans build (library.db)"),
                        readout(human_size(info.catalog)),
                    )),
            ))
            .child(section(
                "caches",
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_MD)
                    .child(panel::setting_row(
                        "cover thumbnails",
                        Some("small covers kept after their first render (thumbs.db); cleared ones rebuild as they scroll into view"),
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            .child(readout(human_size(info.thumbs)))
                            .child(small_button(
                                "clear",
                                icons::TRASH,
                                false,
                                cx.listener(|this, _, _, cx| this.clear_thumbs(cx)),
                            )),
                    ))
                    .child(panel::setting_row(
                        "waveforms",
                        Some("each track's peak strip, kept after its first play; cleared ones re-decode next play"),
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            .child(readout(human_size(info.waveforms)))
                            .child(small_button(
                                "clear",
                                icons::TRASH,
                                false,
                                cx.listener(|this, _, _, cx| this.clear_waveforms(cx)),
                            )),
                    )),
            ))
    }

    /// A sidebar footer row: hands something to the system - the raw
    /// settings file, the data folder - so it reads quieter than the
    /// pages above.
    fn sidebar_action(
        &self,
        label: &'static str,
        icon: &'static str,
        open: fn() -> PathBuf,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .px(tokens::SPACE_MD)
            .py(tokens::SPACE_XS)
            .rounded(tokens::RADIUS)
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .text_xs()
            .text_color(palette::text_muted())
            .cursor_pointer()
            .hover(|d| d.bg(palette::bg_menu_hover()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_, _, _, cx| cx.open_with_system(&open())),
            )
            .child(
                svg()
                    .path(icon)
                    .size(px(12.))
                    .text_color(palette::text_muted()),
            )
            .child(label)
    }
}

/// One right-aligned numeric cell of the folder table.
fn number_cell(width: Pixels, value: String) -> Div {
    div()
        .w(width)
        .flex_none()
        .text_right()
        .text_color(palette::text_muted())
        .child(value)
}

/// A setting row's value where a control would sit.
fn readout(value: String) -> Div {
    div().text_color(palette::text_muted()).child(value)
}

/// Bytes as a short human size: whole numbers through KB, one decimal
/// from MB up, decimal units like the file managers show.
fn human_size(bytes: u64) -> String {
    let mut value = bytes as f64;
    let mut unit = "B";
    for next in ["KB", "MB", "GB", "TB"] {
        if value < 1000. {
            break;
        }
        value /= 1000.;
        unit = next;
    }
    match unit {
        "B" => format!("{bytes} B"),
        "KB" => format!("{} KB", value.round()),
        _ => format!("{value:.1} {unit}"),
    }
}

/// A SQLite database's weight on disk: the file plus its -wal and -shm
/// sidecars, which hold real data between checkpoints.
fn db_size(db: &Path) -> u64 {
    ["", "-wal", "-shm"]
        .iter()
        .map(|suffix| {
            let mut file = db.as_os_str().to_owned();
            file.push(suffix);
            std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0)
        })
        .sum()
}

/// Every file directly under one folder; the waveform cache is flat.
fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|entry| entry.metadata().ok())
        .map(|meta| meta.len())
        .sum()
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let columns = grid_columns(window);

        let sidebar = sidebar()
            .children(PAGES.iter().map(|&(page, label)| {
                settings_ui::nav_item(
                    label,
                    self.page == page,
                    // Entering Storage measures the files fresh, so the
                    // numbers are current without a per-frame stat.
                    move |this: &mut Self, cx| {
                        this.page = page;
                        if page == Page::Storage {
                            this.refresh_storage(cx);
                        }
                        cx.notify();
                    },
                    cx,
                )
            }))
            // The escape hatches sink to the bottom: the raw file this
            // window edits and the folder it lives in.
            .child(div().flex_1())
            .child(self.sidebar_action("settings file", icons::FILE_TEXT, settings_path, cx))
            .child(self.sidebar_action("data folder", icons::FOLDER, data_dir, cx));

        let page = match self.page {
            Page::Appearance => self.appearance_page(columns, cx),
            Page::Behavior => self.behavior_page(cx),
            Page::Library => self.library_page(cx),
            Page::Storage => self.storage_page(cx),
        };

        div()
            .size_full()
            .flex()
            .flex_row()
            .bg(palette::bg_elevated())
            .text_color(palette::text_bright())
            .text_sm()
            // The backdrop paints first, under the pages; without it
            // translucent surfaces would sink into the window's own
            // black instead of the playing track's art.
            .children(self.backdrop.layer(&self.now_art, window, cx))
            .child(sidebar)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .relative()
                    // The page's own surface, the window base the sidebar
                    // sits beside: opaque at full surface opacity so the
                    // backdrop only reads through as the surfaces thin,
                    // never at 100% like the sidebar already holds.
                    .bg(palette::bg_elevated())
                    .child(
                        div()
                            .id("settings-page")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .p(tokens::SPACE_MD)
                            .child(page),
                    )
                    // Always visible, not fading in on scroll: the thumb
                    // is what says more page hangs below the fold. The
                    // absolute wrapper gives the scrollbar its bounds; on
                    // its own it lays out to nothing.
                    .child(div().absolute().inset_0().child(
                        Scrollbar::vertical(&self.scroll).scrollbar_show(ScrollbarShow::Always),
                    )),
            )
    }
}
