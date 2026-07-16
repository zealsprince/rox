//! Every panel's settings window, the paged shape the app settings
//! window set: one OS window per panel, the panel's own pages in a left
//! sidebar, and the shared Appearance page under them editing the
//! panel's palette override (ADR 13). Opened from the panel's dropdown; opening
//! again focuses the existing window. Edits land in the panel's config
//! live - the next render picks the override up through the palette
//! scope - and persist through the layout dump like every other
//! per-view knob.

use std::collections::HashMap;

use gpui::{
    div, prelude::*, px, size, AnyElement, App, Bounds, Context, Div, Entity, EntityId, Global,
    Hsla, ScrollHandle, SharedString, Subscription, TitlebarOptions, WeakEntity, Window,
    WindowBounds, WindowHandle, WindowOptions,
};
use gpui_component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::scroll::{Scrollbar, ScrollbarShow};
use gpui_component::{Icon, Root, Sizable as _};

use crate::assets::icons;
use crate::backdrop::WindowBackdrop;
use crate::design::palette::{self, PanelTheme, ROLES};
use crate::design::tokens;
use crate::panel::{self, AppState, PanelSettings, ScrubState};
use crate::settings_ui::{self, grid_columns, section, sidebar, small_button, SECTION_GAP};

/// The open panel settings windows, keyed by the panel they edit:
/// opening a panel's settings again focuses its window instead of
/// stacking a second editor over the same config. Closed windows leave a
/// stale handle whose activate fails, so the next open falls through and
/// replaces it, same as [`settings_window`](crate::settings_window).
#[derive(Default)]
struct OpenPanelSettings(HashMap<EntityId, WindowHandle<Root>>);

impl Global for OpenPanelSettings {}

/// The Panel Settings entry for a panel's dropdown menu: opens the
/// panel's settings window. Sits in the panel section, above Duplicate.
pub fn settings_item<P: PanelSettings>(menu: PopupMenu, panel: &Entity<P>) -> PopupMenu {
    let panel = panel.clone();
    menu.item(
        PopupMenuItem::new("Panel Settings")
            .icon(Icon::default().path(icons::SETTINGS))
            .on_click(move |_, _, cx| {
                open(panel.clone(), cx);
            }),
    )
}

/// Open a panel's settings window, or bring its open one to the front.
/// The window holds the panel weakly, so it never keeps a closed panel
/// alive.
pub fn open<P: PanelSettings>(panel: Entity<P>, cx: &mut App) {
    let id = panel.entity_id();
    if let Some(handle) = cx
        .try_global::<OpenPanelSettings>()
        .and_then(|open| open.0.get(&id).copied())
    {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    let title = SharedString::from(format!("rox - {} settings", panel.read(cx).panel_name()));
    let bounds = Bounds::centered(None, size(px(640.), px(480.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(settings_ui::MIN_SIZE),
        titlebar: Some(TitlebarOptions {
            title: Some(title.clone()),
            ..Default::default()
        }),
        app_id: Some(crate::APP_ID.into()),
        ..Default::default()
    };
    let state = panel.read(cx).state();
    let handle = cx
        .open_window(options, move |window, cx| {
            // The Wayland backend ignores the creation-time titlebar title;
            // only set_window_title reaches the compositor.
            window.set_window_title(&title);
            let view =
                cx.new(|cx| PanelSettingsWindow::new(panel.downgrade(), state, window, cx));
            cx.new(|cx| Root::new(view, window, cx))
        })
        .expect("failed to open the panel settings window");
    cx.default_global::<OpenPanelSettings>().0.insert(id, handle);
}

/// The window content: the panel's own pages, then the shared Appearance
/// page the window itself provides.
struct PanelSettingsWindow<P: PanelSettings> {
    panel: WeakEntity<P>,
    /// The picked page: an index into the panel's pages, one past the
    /// end for Appearance. A panel with no pages of its own opens
    /// straight on Appearance.
    page: usize,
    /// One picker per palette role, in [`ROLES`] order: the override
    /// when one is set, the app palette's resolved color otherwise.
    pickers: Vec<Entity<ColorPickerState>>,
    opacity_scrub: ScrubState,
    /// The page body's scroll position, shared with the scrollbar so it
    /// can show how much page hangs below the fold.
    scroll: ScrollHandle,
    /// The shared state, for the window's own backdrop.
    state: AppState,
    backdrop: WindowBackdrop,
    _picker_changes: Vec<Subscription>,
    /// Repaints this window when the panel changes from anywhere else.
    _panel_changed: Option<Subscription>,
    /// This window pumps its own frames, so the backdrop needs its own
    /// wake on a new bake.
    _backdrop_changed: Subscription,
}

impl<P: PanelSettings> PanelSettingsWindow<P> {
    fn new(
        panel: WeakEntity<P>,
        state: AppState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let theme = panel
            .upgrade()
            .map(|panel| panel.read(cx).theme())
            .unwrap_or_default();
        let _panel_changed = panel
            .upgrade()
            .map(|panel| cx.observe(&panel, |_, _, cx| cx.notify()));
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        let mut pickers = Vec::with_capacity(ROLES.len());
        let mut _picker_changes = Vec::with_capacity(ROLES.len());
        for (index, role) in ROLES.iter().enumerate() {
            let color = theme
                .color(role.name)
                .unwrap_or_else(|| (role.get)(&palette::resolved()));
            let picker = cx.new(|cx| ColorPickerState::new(window, cx).default_value(color));
            _picker_changes.push(cx.subscribe_in(
                &picker,
                window,
                move |this, _picker, event: &ColorPickerEvent, window, cx| {
                    let ColorPickerEvent::Change(color) = event;
                    this.role_edited(index, *color, window, cx);
                },
            ));
            pickers.push(picker);
        }
        PanelSettingsWindow {
            panel,
            page: 0,
            pickers,
            opacity_scrub: ScrubState::default(),
            scroll: ScrollHandle::new(),
            state,
            backdrop: WindowBackdrop::default(),
            _picker_changes,
            _panel_changed,
            _backdrop_changed,
        }
    }

    /// Every theme edit goes through here: read the panel's override,
    /// change it, hand it back. The panel notifies, which repaints it and
    /// this window both.
    fn update_theme(&mut self, edit: impl FnOnce(&mut PanelTheme), cx: &mut Context<Self>) {
        let Some(panel) = self.panel.upgrade() else {
            return;
        };
        panel.update(cx, |panel, cx| {
            let mut theme = panel.theme();
            edit(&mut theme);
            panel.set_theme(theme, cx);
        });
    }

    /// A picker's change: the role into the override. Clearing the hex
    /// field reads the same as the cell's reset button, back to following
    /// the app palette, so both land in [`reset_role`](Self::reset_role).
    fn role_edited(
        &mut self,
        index: usize,
        color: Option<Hsla>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match color {
            Some(color) => {
                let name = ROLES[index].name;
                self.update_theme(|theme| theme.set_color(name, Some(color.to_rgb())), cx);
            }
            None => self.reset_role(index, window, cx),
        }
    }

    /// Drop one role's override: the panel follows the app palette for
    /// that role again, and its swatch shows the inherited color. The
    /// cell's reset button and a cleared hex field both come here.
    fn reset_role(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let role = &ROLES[index];
        self.update_theme(|theme| theme.set_color(role.name, None), cx);
        let inherited = (role.get)(&palette::resolved());
        self.pickers[index].update(cx, |picker, cx| picker.set_value(inherited, window, cx));
    }

    /// The opacity override's switch: forking starts from the app's
    /// current value, so nothing visibly jumps until the slider moves.
    fn set_opacity_override(&mut self, on: bool, cx: &mut Context<Self>) {
        let value = on.then(palette::app_surface_opacity);
        self.update_theme(|theme| theme.surface_opacity = value, cx);
    }

    fn set_opacity(&mut self, value: f32, cx: &mut Context<Self>) {
        self.update_theme(|theme| theme.surface_opacity = Some(value), cx);
    }

    /// Drop every override: the panel follows the app palette whole
    /// again, and the swatches show the inherited colors.
    fn reset_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.update_theme(|theme| *theme = PanelTheme::default(), cx);
        for (role, picker) in ROLES.iter().zip(&self.pickers) {
            let inherited = (role.get)(&palette::resolved());
            picker.update(cx, |picker, cx| picker.set_value(inherited, window, cx));
        }
    }

    /// The shared Appearance page: the panel's opacity fork and the
    /// override grid, the app palette editor's shape with inherit as the
    /// resting state.
    fn appearance_page(&mut self, theme: &PanelTheme, columns: usize, cx: &mut Context<Self>) -> Div {
        let opacity = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(panel::setting_row(
                "own surface opacity",
                Some("give this panel its own opacity over the backdrop instead of the app's"),
                panel::toggle(
                    theme.surface_opacity.is_some(),
                    Self::set_opacity_override,
                    cx,
                ),
            ))
            .when_some(theme.surface_opacity, |d, value| {
                d.child(panel::setting_row(
                    "surface opacity",
                    None,
                    settings_ui::slider(&self.opacity_scrub, value, Self::set_opacity, cx),
                ))
            });

        let overridden = |name: &str| theme.colors.contains_key(name);
        let grid = settings_ui::role_grid(columns, |j| {
            let role = &ROLES[j];
            // The picker pads a 4px margin around its swatch square; the
            // counter-margin keeps the cell at the grid's 20px footprint.
            let control = ColorPicker::new(&self.pickers[j])
                .small()
                .m(px(-4.))
                .into_any_element();
            // Overridden roles carry a reset button on the cell's right
            // edge, so it reads at a glance which colors have forked from
            // the app palette; the rest of the cell just follows along.
            let reset = overridden(role.name).then(|| {
                settings_ui::icon_button(
                    icons::REFRESH_CW,
                    false,
                    cx.listener(move |this, _, window, cx| this.reset_role(j, window, cx)),
                )
                .into_any_element()
            });
            settings_ui::color_cell(control, role.label, overridden(role.name), reset)
                .into_any_element()
        });
        let body = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_XS)
            .child(div().text_xs().text_color(palette::text_muted()).child(
                "overrides recolor just this panel and hold still under song \
                 theming; reset a swatch or clear its hex field to follow \
                 the app palette again",
            ))
            .child(grid);
        let controls = small_button(
            "reset",
            icons::REFRESH_CW,
            false,
            cx.listener(|this, _, window, cx| this.reset_theme(window, cx)),
        );

        div()
            .flex()
            .flex_col()
            .gap(SECTION_GAP)
            .child(section("opacity", None, opacity))
            .child(section("colors", Some(controls.into_any_element()), body))
    }
}

impl<P: PanelSettings> Render for PanelSettingsWindow<P> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let columns = grid_columns(window);

        let (nav, body): (Div, AnyElement) = match self.panel.upgrade() {
            None => (
                sidebar(),
                div()
                    .text_color(palette::text_muted())
                    .child("the panel was closed")
                    .into_any_element(),
            ),
            Some(panel) => {
                let pages = panel.read(cx).pages();
                let appearance = pages.len();
                let picked = self.page.min(appearance);
                let mut nav = sidebar();
                for (i, label) in pages.iter().enumerate() {
                    nav = nav.child(settings_ui::nav_item(
                        label,
                        picked == i,
                        move |this: &mut Self, cx| {
                            this.page = i;
                            cx.notify();
                        },
                        cx,
                    ));
                }
                nav = nav.child(settings_ui::nav_item(
                    "Appearance",
                    picked == appearance,
                    move |this: &mut Self, cx| {
                        this.page = appearance;
                        cx.notify();
                    },
                    cx,
                ));
                let body = if picked < appearance {
                    panel.update(cx, |panel, cx| panel.page(pages[picked], window, cx))
                } else {
                    let theme = panel.read(cx).theme();
                    self.appearance_page(&theme, columns, cx).into_any_element()
                };
                (nav, body)
            }
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
            .children(self.backdrop.layer(&self.state.now_art, window, cx))
            .child(nav)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .relative()
                    // The page's own surface, the window base the sidebar
                    // sits beside: opaque at full surface opacity so the
                    // backdrop only reads through as the surfaces thin.
                    .bg(palette::bg_elevated())
                    .child(
                        div()
                            .id("panel-settings-page")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .p(tokens::SPACE_MD)
                            .child(body),
                    )
                    // Always visible, not fading in on scroll: the thumb
                    // is what says more page hangs below the fold.
                    .child(div().absolute().inset_0().child(
                        Scrollbar::vertical(&self.scroll).scrollbar_show(ScrollbarShow::Always),
                    )),
            )
    }
}
