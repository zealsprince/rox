//! The settings window: one OS window opened from the menubar, a sidebar
//! of pages on the left and the picked page's controls on the right.
//! Appearance holds the song-theming switch and ADR 10's transparency
//! pair; Colors is the palette editor, a labeled swatch grid per listing
//! group. Edits land live through the palette setters and persist to the
//! settings file per change, the volume slider's cadence. The window
//! edits a working copy of the user palette, so the swatches show the
//! base even while a playing track's seed tints the app over it; while
//! song theming is on the editor locks, because the track is driving.

use gpui::{
    canvas, div, prelude::*, px, size, AnyElement, App, Bounds, Context, Div, Entity, Global,
    Hsla, MouseButton, MouseDownEvent, Pixels, Subscription, TitlebarOptions, Window,
    WindowBounds, WindowHandle, WindowOptions,
};
use gpui_component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_component::{Root, Sizable as _};

use crate::design::palette::{self, Palette, Role, ROLES};
use crate::design::tokens;
use crate::panel::{self, ScrubState};
use crate::settings::Settings;

/// The scalar sliders' strip width; the percent readout rides beside it.
const SLIDER_W: Pixels = px(140.);

/// The sidebar's width, room for a page name and no more.
const SIDEBAR_W: Pixels = px(140.);

/// The colors page's row width, cells to a row.
const GRID_COLUMNS: usize = 4;

/// The open settings window, if any: opening again focuses it instead
/// of stacking a second editor over the same file.
struct OpenSettings(WindowHandle<Root>);

impl Global for OpenSettings {}

/// Open the settings window, or bring the open one to the front.
pub fn open(cx: &mut App) {
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
        titlebar: Some(TitlebarOptions {
            title: Some("rox - settings".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let handle = cx
        .open_window(options, |window, cx| {
            let view = cx.new(|cx| SettingsWindow::new(window, cx));
            cx.new(|cx| Root::new(view, window, cx))
        })
        .expect("failed to open the settings window");
    cx.set_global(OpenSettings(handle));
}

/// The sidebar's pages.
#[derive(Clone, Copy, PartialEq)]
enum Page {
    Appearance,
    Colors,
}

const PAGES: &[(Page, &str)] = &[(Page::Appearance, "Appearance"), (Page::Colors, "Colors")];

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
    _picker_changes: Vec<Subscription>,
}

impl SettingsWindow {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = Settings::load();
        let base = settings.palette();
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
            _picker_changes,
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

    /// Back to the stock palette: the pickers, the live palette, and the
    /// file's map, which empties rather than filling with defaults.
    fn reset_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.base = Palette::default();
        for (role, picker) in ROLES.iter().zip(&self.pickers) {
            let color = (role.get)(&self.base);
            picker.update(cx, |picker, cx| picker.set_value(color, window, cx));
        }
        palette::set(self.base, cx);
        Settings::update(|s| s.palette.clear());
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

    fn appearance_page(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(panel::setting_row(
                "song theming",
                Some("tint the palette and back windows with the playing track's cover art"),
                panel::toggle(self.art_theming, Self::set_art_theming, cx),
            ))
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
            ))
    }

    /// One cell of the color grid: the picker with its label under it,
    /// or a dimmed inert swatch while song theming drives the palette.
    fn color_cell(&self, role: &Role, picker: &Entity<ColorPickerState>, locked: bool) -> Div {
        let control: AnyElement = if locked {
            div()
                .size_5()
                .rounded(tokens::RADIUS)
                .border_1()
                .border_color(palette::border())
                .bg((role.get)(&self.base))
                .opacity(0.5)
                .into_any_element()
        } else {
            ColorPicker::new(picker).small().into_any_element()
        };
        div()
            .flex_1()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_XS)
            .child(control)
            .child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(role.label),
            )
    }

    fn colors_page(&self, cx: &mut Context<Self>) -> Div {
        let locked = self.art_theming;
        let mut body = div().flex().flex_col().gap(tokens::SPACE_SM);
        if locked {
            body = body.child(div().text_xs().text_color(palette::text_muted()).child(
                "song theming is on, so the playing track drives these colors; \
                 turn it off under appearance to edit them",
            ));
        }

        // The grid: each listing group under its header, [`GRID_COLUMNS`]
        // cells to a row, the last row padded so cells keep their width.
        let mut i = 0;
        while i < ROLES.len() {
            let group = ROLES[i].group;
            let end = ROLES[i..]
                .iter()
                .position(|role| role.group != group)
                .map(|n| i + n)
                .unwrap_or(ROLES.len());
            body = body.child(header(group));
            for row_start in (i..end).step_by(GRID_COLUMNS) {
                let mut row = div().flex().flex_row().gap(tokens::SPACE_SM);
                for j in row_start..row_start + GRID_COLUMNS {
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

        // Reset locks with the rest of the editor: it changes the palette
        // too.
        body.child(
            div()
                .pt(tokens::SPACE_SM)
                .flex()
                .flex_row()
                .justify_end()
                .child(
                    div()
                        .px(tokens::SPACE_MD)
                        .py(tokens::SPACE_XS)
                        .rounded(tokens::RADIUS)
                        .bg(palette::bg_control())
                        .map(|d| {
                            if locked {
                                d.opacity(0.5)
                            } else {
                                d.hover(|d| d.bg(palette::bg_control_hover()))
                                    .cursor_pointer()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, window, cx| {
                                            this.reset_palette(window, cx)
                                        }),
                                    )
                            }
                        })
                        .child("reset colors"),
                ),
        )
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

impl Render for SettingsWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            );

        let page = match self.page {
            Page::Appearance => self.appearance_page(cx),
            Page::Colors => self.colors_page(cx),
        };

        div()
            .size_full()
            .flex()
            .flex_row()
            .bg(palette::bg_elevated())
            .text_color(palette::text_bright())
            .text_sm()
            .child(sidebar)
            .child(
                div()
                    .id("settings-page")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .p(tokens::SPACE_MD)
                    .child(page),
            )
    }
}
