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
    canvas, div, prelude::*, px, size, svg, AnyElement, App, Bounds, Context, Div, Entity, Global,
    Hsla, MouseButton, MouseDownEvent, PathPromptOptions, Pixels, ScrollHandle, SharedString,
    Subscription, TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions,
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

/// The scalar sliders' strip width; the percent readout rides beside it.
const SLIDER_W: Pixels = px(140.);

/// The sidebar's width, room for a page name and no more.
const SIDEBAR_W: Pixels = px(140.);

/// The narrowest a color cell renders whole: the swatch, its gap, and
/// the longest role label. The grid fits as many columns as these allow,
/// two at the window floor up to four when the window has the room.
const COLOR_CELL_MIN_W: Pixels = px(150.);

/// The folder table's fixed columns: the track counts and the remove
/// control, sized to [`icon_button`]'s footprint so the header aligns.
const TRACKS_COL_W: Pixels = px(64.);
const ACTION_COL_W: Pixels = px(22.);

/// The gap between a page's sections, a step over the row rhythm so a
/// boundary reads as one.
const SECTION_GAP: Pixels = px(20.);

/// The floor under the window: the sidebar plus a colors row that still
/// fits its labels, and enough height for a page to breathe.
const MIN_SIZE: gpui::Size<Pixels> = gpui::Size {
    width: px(560.),
    height: px(400.),
};

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
        window_min_size: Some(MIN_SIZE),
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
    Library,
}

const PAGES: &[(Page, &str)] = &[(Page::Appearance, "Appearance"), (Page::Library, "Library")];

struct SettingsWindow {
    page: Page,
    /// The working copy of the user palette: what the swatches show and
    /// what edits write through [`palette::set`].
    base: Palette,
    art_theming: bool,
    surface_opacity: f32,
    backdrop_strength: f32,
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
    /// The folder list with per-folder track counts, recounted on every
    /// library event rather than per frame.
    root_counts: Vec<(PathBuf, u64)>,
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
        let root_counts = library.read(cx).root_counts();
        let _library_changed = cx.subscribe(
            &library,
            |this: &mut Self, library, _: &LibraryEvent, cx| {
                this.root_counts = library.read(cx).root_counts();
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
            pickers,
            surface_scrub: ScrubState::default(),
            backdrop_scrub: ScrubState::default(),
            scroll: ScrollHandle::new(),
            library,
            now_art: state.now_art,
            backdrop: WindowBackdrop::default(),
            root_counts,
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

    /// One scalar's slider: the shared slider chrome over a scrub strip,
    /// applying live on click and drag, with the percent alongside.
    fn slider(
        &self,
        scrub: &ScrubState,
        value: f32,
        apply: fn(&mut Self, f32, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) -> Div {
        let entity = cx.entity();
        let strip = div()
            .w(SLIDER_W)
            .h(tokens::CONTROL_H)
            .flex_none()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener({
                    let scrub = scrub.clone();
                    move |this: &mut Self, event: &MouseDownEvent, _, cx| {
                        scrub.begin();
                        if let Some(fraction) = scrub.fraction(event.position.x) {
                            apply(this, fraction, cx);
                        }
                        cx.notify();
                    }
                }),
            )
            .child(
                canvas(
                    {
                        let scrub = scrub.clone();
                        move |bounds, _, _| scrub.set_bounds(bounds)
                    },
                    {
                        let scrub = scrub.clone();
                        move |bounds, _, window, _| {
                            panel::paint_slider(value, false, bounds, window);
                            panel::scrub_on_paint(&scrub, window, {
                                let entity = entity.clone();
                                move |fraction, cx| {
                                    entity.update(cx, |this, cx| apply(this, fraction, cx));
                                }
                            });
                        }
                    },
                )
                .size_full(),
            );
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .child(strip)
            .child(
                div()
                    .w(px(40.))
                    .flex_none()
                    .text_center()
                    .text_color(palette::text_muted())
                    .child(format!("{}%", (value * 100.0).round() as u32)),
            )
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
                        self.slider(
                            &self.surface_scrub,
                            self.surface_opacity,
                            Self::set_surface,
                            cx,
                        ),
                    ))
                    .child(panel::setting_row(
                        "backdrop strength",
                        Some("how strongly the cover backdrop shows behind them"),
                        self.slider(
                            &self.backdrop_scrub,
                            self.backdrop_strength,
                            Self::set_backdrop,
                            cx,
                        ),
                    )),
            ))
            .child(self.colors_section(columns, cx))
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
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .child(control)
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(role.label),
            )
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

        // The grid: each listing group under its header, `columns` cells
        // to a row, the last row padded so cells keep their width.
        let mut i = 0;
        while i < ROLES.len() {
            let group = ROLES[i].group;
            let end = ROLES[i..]
                .iter()
                .position(|role| role.group != group)
                .map(|n| i + n)
                .unwrap_or(ROLES.len());
            body = body.child(header(group));
            for row_start in (i..end).step_by(columns) {
                let mut row = div().flex().flex_row().gap(tokens::SPACE_SM);
                for j in row_start..row_start + columns {
                    row = row.child(if j < end {
                        self.color_cell(&ROLES[j], &self.pickers[j], locked)
                    } else {
                        div().flex_1()
                    });
                }
                body = body.child(row);
            }
            i = end;
        }

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

    /// One row of the folder table: the path, its track count, and a
    /// remove control, inert while a scan runs.
    fn folder_row(&self, root: &Path, count: u64, scanning: bool, cx: &mut Context<Self>) -> Div {
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
            .child(
                div()
                    .w(TRACKS_COL_W)
                    .flex_none()
                    .text_right()
                    .text_color(palette::text_muted())
                    .child(count.to_string()),
            )
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
                .child(div().w(ACTION_COL_W).flex_none()),
        );
        if self.root_counts.is_empty() {
            table = table.child(
                div()
                    .py(tokens::SPACE_XS)
                    .text_color(palette::text_muted())
                    .child("no folders yet"),
            );
        }
        for (root, count) in &self.root_counts {
            table = table.child(self.folder_row(root, *count, scanning, cx));
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
                scanning || self.root_counts.is_empty(),
                cx.listener(|this, _, _, cx| {
                    this.library.update(cx, |library, cx| library.rescan(cx));
                }),
            ));
        section("folders", Some(controls.into_any_element()), body)
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

    /// A sidebar row; the picked page reads like an active control.
    fn nav_item(&self, page: Page, label: &'static str, cx: &mut Context<Self>) -> Div {
        let picked = self.page == page;
        div()
            .px(tokens::SPACE_MD)
            .py(tokens::SPACE_XS)
            .rounded(tokens::RADIUS)
            .cursor_pointer()
            .when(picked, |d| d.bg(palette::bg_control_active()))
            .when(!picked, |d| d.hover(|d| d.bg(palette::bg_menu_hover())))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.page = page;
                    cx.notify();
                }),
            )
            .child(label)
    }
}

/// A header between setting groups, the palette listing's block names.
fn header(label: &'static str) -> Div {
    div()
        .pt(tokens::SPACE_SM)
        .text_xs()
        .text_color(palette::text_muted())
        .child(label)
}

/// A titled section of a page: the name over a hairline, an optional
/// control riding the header's right edge, the rows under it.
fn section(label: &'static str, trailing: Option<AnyElement>, body: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_SM)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .pb(tokens::SPACE_XS)
                .border_b_1()
                .border_color(palette::border())
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(label),
                )
                .when_some(trailing, |d, trailing| d.child(trailing)),
        )
        .child(body)
}

/// The settings window's text button, at the section header's scale
/// where every one of them rides: an icon leading its label; inert ones
/// dim and drop the click.
fn small_button(
    label: &'static str,
    icon: &'static str,
    inert: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(tokens::SPACE_XS)
        .px(tokens::SPACE_SM)
        .py(px(2.))
        .text_xs()
        .rounded(tokens::RADIUS)
        .bg(palette::bg_control())
        .map(|d| {
            if inert {
                d.opacity(0.5)
            } else {
                d.hover(|d| d.bg(palette::bg_control_hover()))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, on_click)
            }
        })
        .child(svg().path(icon).size(px(12.)).text_color(palette::text()))
        .child(label)
}

/// A flat icon-only button for table rows: the glyph alone at rest, a
/// soft pill behind it on hover, dimmed and inert like the text buttons.
fn icon_button(
    icon: &'static str,
    inert: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .flex_none()
        .p(tokens::SPACE_XS)
        .rounded(tokens::RADIUS)
        .map(|d| {
            if inert {
                d.opacity(0.5)
            } else {
                d.hover(|d| d.bg(palette::bg_control()))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, on_click)
            }
        })
        .child(svg().path(icon).size(px(14.)).text_color(palette::text()))
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The color grid fits as many columns as whole cells fit the
        // page: the window minus the sidebar and the body's insets.
        let page_w = window.viewport_size().width - SIDEBAR_W - tokens::SPACE_MD * 2.;
        let columns = usize::clamp((page_w / COLOR_CELL_MIN_W) as usize, 2, 4);

        let sidebar = div()
            .w(SIDEBAR_W)
            .flex_none()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_XS)
            .p(tokens::SPACE_SM)
            .bg(palette::bg_panel())
            .border_r_1()
            .border_color(palette::border())
            .children(
                PAGES
                    .iter()
                    .map(|(page, label)| self.nav_item(*page, label, cx)),
            )
            // The escape hatches sink to the bottom: the raw file this
            // window edits and the folder it lives in.
            .child(div().flex_1())
            .child(self.sidebar_action("settings file", icons::FILE_TEXT, settings_path, cx))
            .child(self.sidebar_action("data folder", icons::FOLDER, data_dir, cx));

        let page = match self.page {
            Page::Appearance => self.appearance_page(columns, cx),
            Page::Library => self.library_page(cx),
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
