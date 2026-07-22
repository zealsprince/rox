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
use crate::design::palette::{self, Palette, PanelTheme, ROLES};
use crate::design::tokens;
use crate::panel::{self, AppState, PanelSettings, ScrubState};
use crate::panels::art::ArtPanel;
use crate::panels::biography::BiographyPanel;
use crate::panels::cover::CoverArtPanel;
use crate::panels::depth::DepthPanel;
use crate::panels::drag_anchor::DragAnchorPanel;
use crate::panels::filter::FilterPanel;
use crate::panels::folder_tree::FolderTreePanel;
use crate::panels::grid::GridPanel;
use crate::panels::group::GroupPanel;
use crate::panels::history::HistoryPanel;
use crate::panels::library::LibraryPanel;
use crate::panels::lyrics::LyricsPanel;
use crate::panels::menu::MenuPanel;
use crate::panels::metadata::MetadataPanel;
use crate::panels::mini::MiniTogglePanel;
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
// The frame sliders' ceilings live in settings, shared with the app
// settings window so the per-panel and app-wide frames scrub the same
// range, every knob running from zero (off) up to its own, in px.
use crate::settings::{BORDER_MAX, MARGIN_MAX, PADDING_MAX, ROUNDING_MAX};
use crate::settings_ui::{self, grid_columns, section, sidebar, small_button, SECTION_GAP};
use rox_dock::{PanelView, TabPanel};

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
            FolderTreePanel,
            MiniTogglePanel,
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

/// The panel's four optional size limits, read off its chrome to render the
/// Behavior page's Size rows (each field's reset shows only when its limit is
/// set). None means that edge is free.
#[derive(Clone, Copy, Default)]
struct SizeLimits {
    min_width: Option<f32>,
    min_height: Option<f32>,
    max_width: Option<f32>,
    max_height: Option<f32>,
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
    /// The size limit fields, typed in px; empty means no limit.
    min_width_input: Entity<InputState>,
    min_height_input: Entity<InputState>,
    max_width_input: Entity<InputState>,
    max_height_input: Entity<InputState>,
    _min_width_events: Subscription,
    _min_height_events: Subscription,
    _max_width_events: Subscription,
    _max_height_events: Subscription,
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
        // The size limit fields, seeded from the panel's current min and max.
        // Empty reads as no limit; "Off" sits as the placeholder to say so.
        let chrome = panel
            .upgrade()
            .map(|panel| panel.read(cx).chrome().clone())
            .unwrap_or_default();
        let field = |value: Option<f32>, window: &mut Window, cx: &mut Context<Self>| {
            let seed = value.map(|n| format!("{n:.0}")).unwrap_or_default();
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Off")
                    .default_value(seed)
            })
        };
        let min_width_input = field(chrome.min_width, window, cx);
        let min_height_input = field(chrome.min_height, window, cx);
        let max_width_input = field(chrome.max_width, window, cx);
        let max_height_input = field(chrome.max_height, window, cx);
        // Each field parses to px on edit and applies through its own setter.
        let watch = |input: &Entity<InputState>,
                     apply: fn(&mut Self, Option<f32>, &mut Context<Self>),
                     window: &mut Window,
                     cx: &mut Context<Self>| {
            cx.subscribe_in(
                input,
                window,
                move |this, input, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.size_limit_edited(input, apply, window, cx);
                    }
                },
            )
        };
        let _min_width_events = watch(&min_width_input, Self::apply_min_width, window, cx);
        let _min_height_events = watch(&min_height_input, Self::apply_min_height, window, cx);
        let _max_width_events = watch(&max_width_input, Self::apply_max_width, window, cx);
        let _max_height_events = watch(&max_height_input, Self::apply_max_height, window, cx);
        PanelSettingsWindow {
            panel,
            page: 0,
            pickers,
            opacity_scrub: ScrubState::default(),
            margin_scrub: ScrubState::default(),
            padding_scrub: ScrubState::default(),
            rounding_scrub: ScrubState::default(),
            border_scrub: ScrubState::default(),
            min_width_input,
            min_height_input,
            max_width_input,
            max_height_input,
            _min_width_events,
            _min_height_events,
            _max_width_events,
            _max_height_events,
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

    // The size limits, typed straight in px and stored on the panel's chrome.
    // A field edit strips non-digits and parses what's left; empty or zero
    // clears the limit so the axis is free again. Each field routes its
    // parsed value through its own setter, passed in as `apply`.

    fn size_limit_edited(
        &mut self,
        input: &Entity<InputState>,
        apply: fn(&mut Self, Option<f32>, &mut Context<Self>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let raw = input.read(cx).value().to_string();
        let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
        // Rewrite the field only when it held non-digits, so a stray letter
        // vanishes; the follow-up Change lands on clean digits and stops.
        if digits != raw {
            input.update(cx, |state, cx| state.set_value(digits.clone(), window, cx));
        }
        let value = digits.parse::<f32>().ok().filter(|n| *n > 0.);
        apply(self, value, cx);
    }

    fn apply_min_width(&mut self, value: Option<f32>, cx: &mut Context<Self>) {
        if let Some(panel) = self.panel.upgrade() {
            panel.update(cx, |panel, cx| panel.set_min_width(value, cx));
        }
    }

    fn apply_min_height(&mut self, value: Option<f32>, cx: &mut Context<Self>) {
        if let Some(panel) = self.panel.upgrade() {
            panel.update(cx, |panel, cx| panel.set_min_height(value, cx));
        }
    }

    fn apply_max_width(&mut self, value: Option<f32>, cx: &mut Context<Self>) {
        if let Some(panel) = self.panel.upgrade() {
            panel.update(cx, |panel, cx| panel.set_max_width(value, cx));
        }
    }

    fn apply_max_height(&mut self, value: Option<f32>, cx: &mut Context<Self>) {
        if let Some(panel) = self.panel.upgrade() {
            panel.update(cx, |panel, cx| panel.set_max_height(value, cx));
        }
    }

    fn reset_min_width(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.min_width_input
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.apply_min_width(None, cx);
    }

    fn reset_min_height(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.min_height_input
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.apply_min_height(None, cx);
    }

    fn reset_max_width(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.max_width_input
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.apply_max_width(None, cx);
    }

    fn reset_max_height(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.max_height_input
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.apply_max_height(None, cx);
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

    // The frame setters: the strip fraction mapped onto whole px, forked
    // as this panel's own override. Zero is a real override, not a clear -
    // it squares the panel back off over a rounded app default; the reset
    // button is the way back to following the app.

    fn set_margin(&mut self, fraction: f32, cx: &mut Context<Self>) {
        let value = (fraction * MARGIN_MAX).round();
        self.update_theme(|theme| theme.margin = Some(value), cx);
    }

    fn set_padding(&mut self, fraction: f32, cx: &mut Context<Self>) {
        let value = (fraction * PADDING_MAX).round();
        self.update_theme(|theme| theme.padding = Some(value), cx);
    }

    fn set_rounding(&mut self, fraction: f32, cx: &mut Context<Self>) {
        let value = (fraction * ROUNDING_MAX).round();
        self.update_theme(|theme| theme.rounding = Some(value), cx);
    }

    fn set_border(&mut self, fraction: f32, cx: &mut Context<Self>) {
        let value = (fraction * BORDER_MAX).round();
        self.update_theme(|theme| theme.border = Some(value), cx);
    }

    // The per-knob resets: drop just this knob's override so it follows
    // the app frame again, the color cells' reset for geometry.

    fn reset_margin(&mut self, cx: &mut Context<Self>) {
        self.update_theme(|theme| theme.margin = None, cx);
    }

    fn reset_padding(&mut self, cx: &mut Context<Self>) {
        self.update_theme(|theme| theme.padding = None, cx);
    }

    fn reset_rounding(&mut self, cx: &mut Context<Self>) {
        self.update_theme(|theme| theme.rounding = None, cx);
    }

    fn reset_border(&mut self, cx: &mut Context<Self>) {
        self.update_theme(|theme| theme.border = None, cx);
    }

    /// One frame knob's slider row: the value over its 0 to `max` range,
    /// the px readout alongside. Unset, the slider rests at the app-wide
    /// default the panel inherits; once the panel forks its own, a reset
    /// button rides the row to send it back to following the app.
    #[allow(clippy::too_many_arguments)]
    fn frame_slider(
        &self,
        scrub: &ScrubState,
        value: Option<f32>,
        inherited: f32,
        max: f32,
        apply: fn(&mut Self, f32, &mut Context<Self>),
        reset: fn(&mut Self, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) -> Div {
        let shown = value.unwrap_or(inherited);
        let slider = panel::value_slider(scrub, shown / max, format!("{shown:.0} px"), apply, cx);
        let row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .child(slider);
        if value.is_some() {
            row.child(settings_ui::icon_button(
                icons::REFRESH_CW,
                false,
                cx.listener(move |this, _, _, cx| reset(this, cx)),
            ))
        } else {
            row
        }
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

    /// The palette this panel currently shows: the app's resolved palette
    /// with the panel's own overrides laid over it, role for role. What
    /// the swatches read, so Inverse starts from what's on screen.
    fn effective_palette(&self, cx: &Context<Self>) -> Palette {
        let mut palette = palette::resolved();
        let theme = self
            .panel
            .upgrade()
            .map(|panel| panel.read(cx).theme())
            .unwrap_or_default();
        for role in ROLES {
            if let Some(color) = theme.color(role.name) {
                (role.set)(&mut palette, color);
            }
        }
        palette
    }

    /// Pin a whole palette onto the panel as color overrides, every role,
    /// and refresh the swatches to match. The shared tail of Inverse and
    /// Apply Song Theme: both freeze a computed palette onto the panel so
    /// it holds under song theming and app edits.
    fn override_all(&mut self, palette: Palette, window: &mut Window, cx: &mut Context<Self>) {
        self.update_theme(
            |theme| {
                for role in ROLES {
                    theme.set_color(role.name, Some((role.get)(&palette)));
                }
            },
            cx,
        );
        for (role, picker) in ROLES.iter().zip(&self.pickers) {
            let color = (role.get)(&palette);
            picker.update(cx, |picker, cx| picker.set_value(color, window, cx));
        }
    }

    /// Flip the panel's colors light for dark, the accents held: the
    /// panel's current look inverted and frozen onto it as overrides.
    fn inverse_colors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let inverted = self.effective_palette(cx).inverse();
        self.override_all(inverted, window, cx);
    }

    /// Freeze the song theme onto the panel: the colors the playing track
    /// derives become this panel's own overrides, so they hold after song
    /// theming turns off or moves to another track. Only offered while
    /// song theming drives the colors.
    fn apply_song_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let themed = palette::resolved();
        self.override_all(themed, window, cx);
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

    /// One size-limit field's row: the px input, a "px" tag, and a reset to
    /// its left that clears the limit. The reset only rides the row once a
    /// limit is set, matching the frame knobs' resets.
    fn size_limit_row(
        &self,
        input: &Entity<InputState>,
        has_limit: bool,
        reset: fn(&mut Self, &mut Window, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .when(has_limit, |row| {
                row.child(settings_ui::icon_button(
                    icons::REFRESH_CW,
                    false,
                    cx.listener(move |this, _, window, cx| reset(this, window, cx)),
                ))
            })
            .child(Input::new(input).small().w(px(64.)))
            .child(
                div()
                    .flex_none()
                    .text_color(palette::text_muted())
                    .child("px"),
            )
    }

    /// The shared Behavior page: the lock and anchor toggles every panel
    /// carries, the size limits, then the panel's own behavior rows when it
    /// has any. Sits second in the nav on every panel, so how a panel acts
    /// always lives in the same spot.
    #[allow(clippy::too_many_arguments)]
    fn behavior_page(
        &mut self,
        locked: bool,
        anchor: bool,
        limits: SizeLimits,
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
        // The size limits: type a px value to hold the panel to a floor or a
        // cap, empty to leave it free. Only the axis the panel is resized
        // along takes effect, but both are offered since a panel can sit in a
        // row or a column. The min and max of each axis sit together.
        let size = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(panel::setting_row(
                "Min Width",
                Some("Hold the panel's width so a resize can't squeeze it narrower"),
                self.size_limit_row(
                    &self.min_width_input,
                    limits.min_width.is_some(),
                    Self::reset_min_width,
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Max Width",
                Some("Cap the panel's width so it doesn't stretch when the window widens"),
                self.size_limit_row(
                    &self.max_width_input,
                    limits.max_width.is_some(),
                    Self::reset_max_width,
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Min Height",
                Some("Hold the panel's height so a resize can't squeeze it shorter"),
                self.size_limit_row(
                    &self.min_height_input,
                    limits.min_height.is_some(),
                    Self::reset_min_height,
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Max Height",
                Some("Cap the panel's height so it doesn't stretch when the window grows taller"),
                self.size_limit_row(
                    &self.max_height_input,
                    limits.max_height.is_some(),
                    Self::reset_max_height,
                    cx,
                ),
            ));
        div()
            .flex()
            .flex_col()
            .gap(SECTION_GAP)
            .child(section("Placement", None, placement))
            .child(section("Size", None, size))
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

        let app = settings::app_frame();
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
                    app.margin,
                    MARGIN_MAX,
                    Self::set_margin,
                    Self::reset_margin,
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Padding",
                Some("Space inside the panel's edge, kept in its own background"),
                self.frame_slider(
                    &self.padding_scrub,
                    theme.padding,
                    app.padding,
                    PADDING_MAX,
                    Self::set_padding,
                    Self::reset_padding,
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Rounding",
                Some("Round the panel's corners off into the backdrop"),
                self.frame_slider(
                    &self.rounding_scrub,
                    theme.rounding,
                    app.rounding,
                    ROUNDING_MAX,
                    Self::set_rounding,
                    Self::reset_rounding,
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Border",
                Some("A line around the panel's edge, in the Border role's color"),
                self.frame_slider(
                    &self.border_scrub,
                    theme.border,
                    app.border,
                    BORDER_MAX,
                    Self::set_border,
                    Self::reset_border,
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
        // Apply Song Theme lives only while song theming drives the colors
        // it would freeze in; Inverse and Reset stay open.
        let song_on = palette::art_theming();
        let color_controls = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .child(small_button(
                "Inverse",
                icons::CONTRAST,
                false,
                cx.listener(|this, _, window, cx| this.inverse_colors(window, cx)),
            ))
            .child(small_button(
                "Apply Song Theme",
                icons::DISC,
                !song_on,
                cx.listener(|this, _, window, cx| this.apply_song_theme(window, cx)),
            ))
            .child(small_button(
                "Reset",
                icons::REFRESH_CW,
                false,
                cx.listener(|this, _, window, cx| this.reset_colors(window, cx)),
            ));

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
                        let (locked, anchor, limits) = {
                            let chrome = panel.read(cx).chrome();
                            (
                                chrome.locked,
                                chrome.anchor,
                                SizeLimits {
                                    min_width: chrome.min_width,
                                    min_height: chrome.min_height,
                                    max_width: chrome.max_width,
                                    max_height: chrome.max_height,
                                },
                            )
                        };
                        let extra = panel.update(cx, |panel, cx| panel.behavior(window, cx));
                        self.behavior_page(locked, anchor, limits, extra, cx)
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
