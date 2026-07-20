//! Every panel's settings window, the paged shape the app settings
//! window set: one OS window per panel, the panel's own pages in a left
//! sidebar, and the shared Appearance page under them editing the
//! panel's palette override (ADR 13). Opened from the panel's dropdown; opening
//! again focuses the existing window. Edits land in the panel's config
//! live - the next render picks the override up through the palette
//! scope - and persist through the layout dump like every other
//! per-view knob.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{
    div, prelude::*, px, size, AnyElement, App, Bounds, Context, Div, Entity, EntityId,
    Focusable as _, Global, Hsla, ScrollHandle, SharedString, Subscription, TitlebarOptions,
    WeakEntity, Window, WindowBounds, WindowHandle, WindowOptions,
};
use gpui_component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::scroll::{Scrollbar, ScrollbarShow};
use gpui_component::{Icon, Root, Sizable as _};

use crate::assets::icons;
use crate::backdrop::WindowBackdrop;
use crate::design::palette::{self, PanelTheme, ROLES};
use crate::design::tokens;
use crate::panel::{self, AppState, PanelSettings, ScrubState};
use crate::panels::art::ArtPanel;
use crate::panels::biography::BiographyPanel;
use crate::panels::cover::CoverArtPanel;
use crate::panels::depth::DepthPanel;
use crate::panels::drag_anchor::DragAnchorPanel;
use crate::panels::filter::FilterPanel;
use crate::panels::grid::GridPanel;
use crate::panels::group::GroupPanel;
use crate::panels::history::HistoryPanel;
use crate::panels::library::LibraryPanel;
use crate::panels::lyrics::LyricsPanel;
use crate::panels::menu::MenuPanel;
use crate::panels::metadata::MetadataPanel;
use crate::panels::playlists::PlaylistsPanel;
use crate::panels::queue::QueuePanel;
use crate::panels::queue_widget::QueueWidgetPanel;
use crate::panels::search::SearchPanel;
use crate::panels::slide::SlidePanel;
use crate::panels::spectrum::SpectrumPanel;
use crate::panels::transport::{SeekStripPanel, TrackInfoPanel, TransportPanel, VolumePanel};
use crate::panels::waveform::WaveformPanel;
use crate::panels::window_controls::WindowControlsPanel;
use crate::settings;
use crate::settings_ui::{self, grid_columns, section, sidebar, small_button, SECTION_GAP};
use rox_dock::{PanelView, TabPanel};

/// The open panel settings windows, keyed by the panel they edit:
/// opening a panel's settings again focuses its window instead of
/// stacking a second editor over the same config. Closed windows leave a
/// stale handle whose activate fails, so the next open falls through and
/// replaces it, same as [`settings_window`](crate::settings_window).
/// The frame sliders' ceilings; every knob runs from zero (off) up to
/// its own, in px.
const MARGIN_MAX: f32 = 24.0;
const PADDING_MAX: f32 = 24.0;
const ROUNDING_MAX: f32 = 24.0;
const BORDER_MAX: f32 = 6.0;

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
    let title = SharedString::from(format!(
        "rox - {} settings",
        panel::display_name(panel.read(cx).panel_name())
    ));
    // The last closed panel settings window's size, floored at MIN_SIZE so a
    // stale small frame never opens under the layout's minimum.
    let min = settings_ui::MIN_SIZE;
    let (width, height) = settings::Settings::load()
        .panel_settings_window
        .filter(|s| s.width >= f32::from(min.width) && s.height >= f32::from(min.height))
        .map(|s| (s.width, s.height))
        .unwrap_or((640., 480.));
    let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
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
            let view = cx.new(|cx| PanelSettingsWindow::new(panel.downgrade(), state, window, cx));
            cx.new(|cx| Root::new(view, window, cx))
        })
        .expect("failed to open the panel settings window");
    cx.default_global::<OpenPanelSettings>()
        .0
        .insert(id, handle);
}

/// Dispatch a type-erased panel view to its concrete settings-capable
/// type: try each downcast until one lands and run the body with the
/// typed entity bound. The type list mirrors the workspace's panel
/// registry; a type missing here just no-ops on the type-erased routes
/// (the layout tree's gear and lock).
macro_rules! with_settings_panel {
    ($view:expr, |$panel:ident| $body:expr) => {
        with_settings_panel!(
            @try $view, $panel, $body,
            LibraryPanel,
            SearchPanel,
            FilterPanel,
            GridPanel,
            ArtPanel,
            PlaylistsPanel,
            QueuePanel,
            HistoryPanel,
            CoverArtPanel,
            MetadataPanel,
            LyricsPanel,
            BiographyPanel,
            TrackInfoPanel,
            TransportPanel,
            SeekStripPanel,
            VolumePanel,
            QueueWidgetPanel,
            SpectrumPanel,
            WaveformPanel,
            MenuPanel,
            DragAnchorPanel,
            WindowControlsPanel,
            GroupPanel,
            DepthPanel,
            SlidePanel,
        )
    };
    (@try $view:expr, $panel:ident, $body:expr, $($ty:ty),+ $(,)?) => {
        $(
            if let Ok($panel) = $view.view().downcast::<$ty>() {
                $body;
                return;
            }
        )+
    };
}

/// Open the settings window for a type-erased panel, the settings
/// window's layout tree route in.
pub fn open_for_view(panel: &Arc<dyn PanelView>, cx: &mut App) {
    with_settings_panel!(panel, |panel| open(panel, cx));
}

/// Flip a type-erased panel's placement lock, the layout tree's lock
/// toggle. The dock reads the flag through `Panel::locked` on its next
/// paint, so the flip settles on its own.
pub fn toggle_locked_for_view(panel: &Arc<dyn PanelView>, cx: &mut App) {
    with_settings_panel!(panel, |panel| panel.update(cx, |panel, cx| {
        let on = !panel.chrome().locked;
        panel.set_locked(on, cx);
    }));
}

/// The open rename windows, keyed by the panel they rename; the same
/// replace-a-stale-handle story as [`OpenPanelSettings`].
#[derive(Default)]
struct OpenRenames(HashMap<EntityId, WindowHandle<Root>>);

impl Global for OpenRenames {}

/// The head of a panel's dropdown tail: the Add Panel flyout above the
/// Panel-section divider, then the section's "Panel" header, then Rename.
/// Every panel routes into its tail through here, so this one call opens
/// the section for all of them - which is why it owns the leading
/// separator (callers pass their content items straight in, no separator
/// of their own) and why Add Panel, a sibling into this group rather than
/// an op on this panel, sits above the divider that starts the section.
pub fn rename_item<P: PanelSettings>(
    menu: PopupMenu,
    panel: &Entity<P>,
    tab_panel: Option<WeakEntity<TabPanel>>,
    window: &mut Window,
    cx: &mut App,
) -> PopupMenu {
    let menu = crate::workspace::add_panel_submenu(menu, tab_panel, window, cx);
    let panel = panel.clone();
    menu.separator().label("Panel").item(
        PopupMenuItem::new("Rename")
            .icon(Icon::default().path(icons::PENCIL))
            .on_click(move |_, _, cx| {
                open_rename(panel.clone(), cx);
            }),
    )
}

/// Open a panel's rename window, or bring its open one to the front. The
/// window holds the panel weakly, like the settings window.
fn open_rename<P: PanelSettings>(panel: Entity<P>, cx: &mut App) {
    let id = panel.entity_id();
    if let Some(handle) = cx
        .try_global::<OpenRenames>()
        .and_then(|open| open.0.get(&id).copied())
    {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    let title = SharedString::from(format!(
        "rox - rename {}",
        panel::display_name(panel.read(cx).panel_name())
    ));
    let bounds = Bounds::centered(None, size(px(380.), px(112.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
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
            let view = cx.new(|cx| RenameWindow::new(panel, state, window, cx));
            cx.new(|cx| Root::new(view, window, cx))
        })
        .expect("failed to open the rename window");
    cx.default_global::<OpenRenames>().0.insert(id, handle);
}

/// The rename window's content: one input over the panel's title. Edits
/// land as they are typed - the tab follows along - and Enter closes the
/// window; clearing the field goes back to the built-in name.
struct RenameWindow<P: PanelSettings> {
    panel: WeakEntity<P>,
    input: Entity<InputState>,
    /// The shared state, for the window's own backdrop.
    state: AppState,
    backdrop: WindowBackdrop,
    _input_events: Subscription,
    /// This window pumps its own frames, so the backdrop needs its own
    /// wake on a new bake.
    _backdrop_changed: Subscription,
}

impl<P: PanelSettings> RenameWindow<P> {
    fn new(panel: Entity<P>, state: AppState, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // The built-in name sits as the placeholder, so an empty field
        // reads as what it does: fall back to that name.
        let (current, placeholder) = {
            let panel = panel.read(cx);
            (
                panel.custom_title().unwrap_or_default().to_owned(),
                panel::display_name(panel.panel_name()),
            )
        };
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .default_value(current)
        });
        let _input_events = cx.subscribe_in(
            &input,
            window,
            |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    let value = input.read(cx).value().trim().to_string();
                    let title = (!value.is_empty()).then_some(value);
                    if let Some(panel) = this.panel.upgrade() {
                        panel.update(cx, |panel, cx| panel.set_custom_title(title, cx));
                    }
                }
                InputEvent::PressEnter { .. } => window.remove_window(),
                _ => {}
            },
        );
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        window.focus(&input.read(cx).focus_handle(cx));
        RenameWindow {
            panel: panel.downgrade(),
            input,
            state,
            backdrop: WindowBackdrop::default(),
            _input_events,
            _backdrop_changed,
        }
    }
}

impl<P: PanelSettings> Render for RenameWindow<P> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_XS)
            .p(tokens::SPACE_MD)
            .bg(palette::bg_elevated())
            .text_color(palette::text_bright())
            .text_sm()
            // The backdrop paints first, under the input, like every
            // other window over the shared state.
            .children(self.backdrop.layer(&self.state.now_art, window, cx))
            .child(Input::new(&self.input).w_full())
            .child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child("Shown as the panel's tab; empty goes back to the built-in name"),
            )
    }
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
    margin_scrub: ScrubState,
    padding_scrub: ScrubState,
    rounding_scrub: ScrubState,
    border_scrub: ScrubState,
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
        // The OS close button never runs a teardown of ours, so save the
        // frame through the should-close hook. Shared across panels, so the
        // last closed window wins.
        window.on_window_should_close(cx, move |window, _| {
            let frame = window.window_bounds().get_bounds();
            settings::Settings::update(move |s| {
                s.panel_settings_window = Some(settings::LayoutSize {
                    width: frame.size.width.into(),
                    height: frame.size.height.into(),
                });
            });
            true
        });
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
            margin_scrub: ScrubState::default(),
            padding_scrub: ScrubState::default(),
            rounding_scrub: ScrubState::default(),
            border_scrub: ScrubState::default(),
            scroll: ScrollHandle::new(),
            state,
            backdrop: WindowBackdrop::default(),
            _picker_changes,
            _panel_changed,
            _backdrop_changed,
        }
    }

    /// Pin or unpin the panel in the dock, through the same panel entity
    /// the theme edits flow through.
    fn set_panel_locked(&mut self, on: bool, cx: &mut Context<Self>) {
        if let Some(panel) = self.panel.upgrade() {
            panel.update(cx, |panel, cx| panel.set_locked(on, cx));
        }
    }

    /// Turn the panel's window-move handle on or off.
    fn set_panel_anchor(&mut self, on: bool, cx: &mut Context<Self>) {
        if let Some(panel) = self.panel.upgrade() {
            panel.update(cx, |panel, cx| panel.set_anchor(on, cx));
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

    // The frame setters: the strip fraction mapped onto whole px, zero
    // clearing the knob so an untouched frame serializes away.

    fn set_margin(&mut self, fraction: f32, cx: &mut Context<Self>) {
        let value = (fraction * MARGIN_MAX).round();
        self.update_theme(|theme| theme.margin = (value > 0.0).then_some(value), cx);
    }

    fn set_padding(&mut self, fraction: f32, cx: &mut Context<Self>) {
        let value = (fraction * PADDING_MAX).round();
        self.update_theme(|theme| theme.padding = (value > 0.0).then_some(value), cx);
    }

    fn set_rounding(&mut self, fraction: f32, cx: &mut Context<Self>) {
        let value = (fraction * ROUNDING_MAX).round();
        self.update_theme(|theme| theme.rounding = (value > 0.0).then_some(value), cx);
    }

    fn set_border(&mut self, fraction: f32, cx: &mut Context<Self>) {
        let value = (fraction * BORDER_MAX).round();
        self.update_theme(|theme| theme.border = (value > 0.0).then_some(value), cx);
    }

    /// One frame knob's slider row: the value over its 0 to `max` range,
    /// the px readout alongside, unset reading as zero.
    fn frame_slider(
        &self,
        scrub: &ScrubState,
        value: Option<f32>,
        max: f32,
        apply: fn(&mut Self, f32, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) -> Div {
        let value = value.unwrap_or(0.0);
        panel::value_slider(scrub, value / max, format!("{value:.0} px"), apply, cx)
    }

    /// Drop every color override: the panel follows the app palette
    /// whole again, and the swatches show the inherited colors. The
    /// frame and opacity keep their own resets, so recoloring can start
    /// over without flattening the geometry.
    fn reset_colors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.update_theme(|theme| theme.colors.clear(), cx);
        for (role, picker) in ROLES.iter().zip(&self.pickers) {
            let inherited = (role.get)(&palette::resolved());
            picker.update(cx, |picker, cx| picker.set_value(inherited, window, cx));
        }
    }

    /// Drop the frame knobs: the panel sits flush in its cell again,
    /// square and borderless, colors untouched.
    fn reset_frame(&mut self, cx: &mut Context<Self>) {
        self.update_theme(
            |theme| {
                theme.margin = None;
                theme.padding = None;
                theme.rounding = None;
                theme.border = None;
            },
            cx,
        );
    }

    /// The shared Behavior page: the lock and anchor toggles every panel
    /// carries, then the panel's own behavior rows when it has any. Sits
    /// second in the nav on every panel, so how a panel acts always lives
    /// in the same spot.
    fn behavior_page(
        &mut self,
        locked: bool,
        anchor: bool,
        extra: Option<AnyElement>,
        cx: &mut Context<Self>,
    ) -> Div {
        let placement = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(panel::setting_row(
                "Locked",
                Some("Pin the panel in place; the dock won't let it be dragged or rearranged"),
                panel::toggle(locked, Self::set_panel_locked, cx),
            ))
            .child(panel::setting_row(
                "Drag Anchor",
                Some("A drag anywhere on the panel moves the window, for decorations-off layouts"),
                panel::toggle(anchor, Self::set_panel_anchor, cx),
            ));
        div()
            .flex()
            .flex_col()
            .gap(SECTION_GAP)
            .child(section("Placement", None, placement))
            .children(extra)
    }

    /// The shared Appearance page: the panel's opacity fork, the frame
    /// knobs, the panel's own appearance section when it has one, and
    /// the override grid, the app palette editor's shape with inherit
    /// as the resting state.
    fn appearance_page(
        &mut self,
        theme: &PanelTheme,
        extra: Option<AnyElement>,
        own_font: bool,
        columns: usize,
        cx: &mut Context<Self>,
    ) -> Div {
        let opacity = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(panel::setting_row(
                "Own Surface Opacity",
                Some("Give this panel its own opacity over the backdrop instead of the app's"),
                panel::toggle(
                    theme.surface_opacity.is_some(),
                    Self::set_opacity_override,
                    cx,
                ),
            ))
            .when_some(theme.surface_opacity, |d, value| {
                d.child(panel::setting_row(
                    "Surface Opacity",
                    None,
                    settings_ui::slider(&self.opacity_scrub, value, Self::set_opacity, cx),
                ))
            });

        let frame = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(panel::setting_row(
                "Margin",
                Some("Pull the panel in from its cell, the backdrop showing through the gap"),
                self.frame_slider(
                    &self.margin_scrub,
                    theme.margin,
                    MARGIN_MAX,
                    Self::set_margin,
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Padding",
                Some("Space inside the panel's edge, kept in its own background"),
                self.frame_slider(
                    &self.padding_scrub,
                    theme.padding,
                    PADDING_MAX,
                    Self::set_padding,
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Rounding",
                Some("Round the panel's corners off into the backdrop"),
                self.frame_slider(
                    &self.rounding_scrub,
                    theme.rounding,
                    ROUNDING_MAX,
                    Self::set_rounding,
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Border",
                Some("A line around the panel's edge, in the Border role's color"),
                self.frame_slider(
                    &self.border_scrub,
                    theme.border,
                    BORDER_MAX,
                    Self::set_border,
                    cx,
                ),
            ));

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
                "Overrides recolor just this panel and hold still under song \
                 theming; reset a swatch or clear its hex field to follow \
                 the app palette again",
            ))
            .child(grid);
        // Each section resets its own knobs: recoloring can start over
        // without flattening the frame, and the other way around.
        let frame_controls = small_button(
            "Reset",
            icons::REFRESH_CW,
            false,
            cx.listener(|this, _, _, cx| this.reset_frame(cx)),
        );
        let color_controls = small_button(
            "Reset",
            icons::REFRESH_CW,
            false,
            cx.listener(|this, _, window, cx| this.reset_colors(window, cx)),
        );

        // The generic font override: any panel that does not draw its own
        // font control gets a family picker here, resolving to the app font
        // when unset. Panels with their own (the lyrics panel) opt out
        // through `has_own_font` so the page never shows two.
        let font_section = (!own_font).then(|| {
            let reset = small_button(
                "Reset",
                icons::REFRESH_CW,
                theme.font.is_none(),
                cx.listener(|this, _, _, cx| this.update_theme(|theme| theme.font = None, cx)),
            );
            section(
                "Font",
                Some(reset.into_any_element()),
                panel::setting_row(
                    "Font",
                    Some("The panel's typeface; default follows the app font"),
                    panel::font_picker(
                        "panel-font",
                        theme.font.clone(),
                        |this: &mut Self, font, cx| {
                            this.update_theme(|theme| theme.font = font, cx)
                        },
                        cx,
                    ),
                ),
            )
            .into_any_element()
        });

        div()
            .flex()
            .flex_col()
            .gap(SECTION_GAP)
            .child(section("Opacity", None, opacity))
            .child(section(
                "Frame",
                Some(frame_controls.into_any_element()),
                frame,
            ))
            .children(font_section)
            // The panel's own appearance rows, when it has any: knobs
            // that live on its config rather than its theme, like the
            // grid's art rounding.
            .children(extra)
            .child(section(
                "Colors",
                Some(color_controls.into_any_element()),
                body,
            ))
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
                    .child("The panel was closed")
                    .into_any_element(),
            ),
            Some(panel) => {
                let pages = panel.read(cx).pages();
                // Appearance and Behavior lead the nav on every panel, the
                // app settings window's order, so how a panel looks and how
                // it acts always sit in the same two spots no matter what
                // pages it brings. `page` 0 is Appearance, 1 is Behavior,
                // and the panel's own pages follow at 2..
                let picked = self.page.min(pages.len() + 1);
                let mut nav = sidebar()
                    .child(settings_ui::nav_item(
                        "Appearance",
                        icons::PALETTE,
                        picked == 0,
                        move |this: &mut Self, cx| {
                            this.page = 0;
                            cx.notify();
                        },
                        cx,
                    ))
                    .child(settings_ui::nav_item(
                        "Behavior",
                        icons::SLIDERS,
                        picked == 1,
                        move |this: &mut Self, cx| {
                            this.page = 1;
                            cx.notify();
                        },
                        cx,
                    ));
                for (i, &(label, icon)) in pages.iter().enumerate() {
                    let page = i + 2;
                    nav = nav.child(settings_ui::nav_item(
                        label,
                        icon,
                        picked == page,
                        move |this: &mut Self, cx| {
                            this.page = page;
                            cx.notify();
                        },
                        cx,
                    ));
                }
                let body = match picked {
                    0 => {
                        let theme = panel.read(cx).theme();
                        let own_font = panel.read(cx).has_own_font();
                        let extra = panel.update(cx, |panel, cx| panel.appearance(window, cx));
                        self.appearance_page(&theme, extra, own_font, columns, cx)
                            .into_any_element()
                    }
                    1 => {
                        // Read through chrome() so the call isn't ambiguous
                        // between PanelSettings::locked and the dock's
                        // Panel::locked, which share the name.
                        let locked = panel.read(cx).chrome().locked;
                        let anchor = panel.read(cx).chrome().anchor;
                        let extra = panel.update(cx, |panel, cx| panel.behavior(window, cx));
                        self.behavior_page(locked, anchor, extra, cx)
                            .into_any_element()
                    }
                    _ => panel.update(cx, |panel, cx| panel.page(pages[picked - 2].0, window, cx)),
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
            .when_some(settings::app_font(), |d, font| d.font_family(font))
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
