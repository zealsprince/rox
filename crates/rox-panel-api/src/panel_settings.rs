//! Every panel's settings window, the paged shape the app settings
//! window set: one OS window per panel, the panel's own pages in a left
//! sidebar, and the shared Appearance page under them editing the
//! panel's palette override (ADR 13). Opened from the panel's dropdown; opening
//! again focuses the existing window. Edits land in the panel's config
//! live - the next render picks the override up through the palette
//! scope - and persist through the layout dump like every other
//! per-view knob.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    div, prelude::*, px, size, AnyElement, App, Bounds, Context, Div, Entity, EntityId,
    Focusable as _, Global, Hsla, MouseDownEvent, PathPromptOptions, ScrollHandle, SharedString,
    Subscription, WeakEntity, Window, WindowHandle,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_component::scroll::Scrollbar;
use gpui_component::{Icon, Root, Sizable as _};

use crate::panel::{self, shader, AppState, PanelSettings, ScrubState};
use rox_core::settings;
use rox_design::assets::icons;
use rox_design::palette::{self, BorderEdge, BorderEdges, Palette, PanelTheme, ROLES};
use rox_design::tokens;
use rox_services::backdrop::WindowBackdrop;
// The frame sliders' ceilings live in settings, shared with the app
// settings window so the per-panel and app-wide frames scrub the same
// range, every knob running from zero (off) up to its own, in px.
use crate::signal_ui::{self, routes::RouteEditState};
use rox_core::settings::{BORDER_MAX, MARGIN_MAX, PADDING_MAX, ROUNDING_MAX};
use rox_dock::TabPanel;
use rox_panel_kit::ui::{
    self as settings_ui, grid_columns, section, sidebar, small_button, SECTION_GAP,
};
use rox_viz::signal::Route;

/// The open panel settings windows, keyed by the panel they edit:
/// opening a panel's settings again focuses its window instead of
/// stacking a second editor over the same config. Closed windows leave a
/// stale handle whose activate fails, so the next open falls through and
/// replaces it, same as the app settings window.
#[derive(Default)]
struct OpenPanelSettings(HashMap<EntityId, WindowHandle<Root>>);

impl Global for OpenPanelSettings {}

/// The Panel Settings entry for a panel's dropdown menu: opens the
/// panel's settings window. Sits in the panel section, above Duplicate.
/// A panel hosted in a composite gets its host's settings row right after
/// its own, so the container is reachable from the child sitting in it.
pub fn settings_item<P: PanelSettings>(menu: PopupMenu, panel: &Entity<P>, cx: &App) -> PopupMenu {
    let child = panel.entity_id();
    let panel = panel.clone();
    let menu = menu.item(
        PopupMenuItem::new("Panel Settings")
            .icon(Icon::default().path(icons::SETTINGS))
            .on_click(move |_, _, cx| {
                open(panel.clone(), cx);
            }),
    );
    crate::openers::host_settings_item(menu, child, cx)
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
        .windows
        .panel_settings
        .filter(|s| s.width >= f32::from(min.width) && s.height >= f32::from(min.height))
        .map(|s| (s.width, s.height))
        .unwrap_or((640., 480.));
    let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
    let state = panel.read(cx).state();
    let handle = crate::panel::open_child_window(
        cx,
        title,
        bounds,
        Some(settings_ui::MIN_SIZE),
        move |window, cx| {
            cx.new(|cx| PanelSettingsWindow::new(panel.downgrade(), state, window, cx))
        },
    );
    cx.default_global::<OpenPanelSettings>()
        .0
        .insert(id, handle);
}

/// How much of a pending source the approval block prints. Long enough to
/// read a real shader, short enough that a file someone pasted a novel into
/// doesn't build ten thousand elements.
const PENDING_LINES: usize = 400;

/// The approval block both shader surfaces wear: what arrived, where it says
/// it came from, and the two ways out. Shaders travel inside layout dumps
/// and workspace bundles as plain WGSL, so applying somebody's look hands
/// rox their code; this is where a person reads it before it runs.
///
/// Read-only on purpose. rox has no code editor, and a box that let the
/// source be edited before approving would only be a slower way to reach
/// the same yes.
pub fn pending_shader(
    id: &'static str,
    source: &str,
    path: Option<&Path>,
    approve: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    discard: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    let lines: Vec<String> = source
        .lines()
        .take(PENDING_LINES)
        .map(str::to_string)
        .collect();
    let clipped = source.lines().count().saturating_sub(lines.len());
    let origin: SharedString = match path {
        Some(path) => format!("Said to come from {}", path.display()).into(),
        None => "No file behind it; the source rode the layout".into(),
    };
    let listing = div()
        .id(id)
        .max_h(px(280.))
        .overflow_y_scroll()
        .p(tokens::SPACE_SM)
        .rounded(tokens::RADIUS)
        .bg(palette::bg_control())
        .flex()
        .flex_col()
        .text_xs()
        .text_color(palette::text_muted())
        .children(lines)
        .when(clipped > 0, |listing| {
            listing.child(
                div()
                    .text_color(palette::text_faint())
                    .child(format!("... {clipped} more lines")),
            )
        });
    div()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_MD)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .text_xs()
                .text_color(palette::text_muted())
                .child(
                    "This shader arrived inside a layout or a workspace. It doesn't run \
                     until you've read it and approved it on this machine.",
                )
                .child(origin)
                .child(format!("Hash {}", shader::fingerprint(source))),
        )
        .child(listing)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_SM)
                .child(small_button("Approve", icons::CHECK, false, approve))
                .child(small_button("Discard", icons::TRASH, false, discard)),
        )
}

/// The block a surface wearing a pool shader shows: which shader it's on,
/// that the file behind it is shared, and the two ways off it. Both shader
/// surfaces wear this, the way they share the approval block above.
///
/// A name the pool doesn't hold says so outright. The surface paints
/// nothing in that state and there's no error to read anywhere else, so
/// without this the panel is just blank for no stated reason.
pub fn pool_shader_block<P: 'static>(
    name: &str,
    missing: bool,
    edit: impl Fn(&mut P, &mut Context<P>) + 'static,
    detach: impl Fn(&mut P, &mut Context<P>) + 'static,
    cx: &mut Context<P>,
) -> Div {
    let note: SharedString = if missing {
        format!(
            "{name} isn't in this workspace's shaders, so nothing paints. Choosing a \
             file or a preset detaches this panel and gives it a source of its own."
        )
        .into()
    } else {
        format!(
            "Wearing {name} from this workspace's shaders. The source is shared, so an \
             edit lands on every panel wearing the name."
        )
        .into()
    };
    let controls = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_SM)
        .child(small_button(
            "Edit as File",
            icons::EXTERNAL_LINK,
            missing,
            cx.listener(move |this, _, _, cx| edit(this, cx)),
        ))
        .child(small_button(
            "Detach Copy",
            icons::COPY,
            missing,
            cx.listener(move |this, _, _, cx| detach(this, cx)),
        ));
    div()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_MD)
        .child(panel::setting_row_dyn("Pool Shader", Some(note), controls))
}

/// The name field behind "Save to Shaders", on whichever surface is wearing
/// it. The input builds the first time the block renders rather than when
/// the surface is constructed: a panel has a `Window` at render and not
/// before. It lives on the surface from then on, so a half-typed name
/// survives the repaint a recompile brings.
#[derive(Default)]
pub struct ShaderNameField {
    input: Option<Entity<InputState>>,
    /// The placeholder the input was last given. Kept so the field can
    /// follow a rename without writing one every render, which would notify
    /// the input into a frame of its own each time.
    placeholder: String,
}

impl ShaderNameField {
    /// The input, built on first ask against the window it renders in. The
    /// placeholder is the name a save would land on with the field left
    /// empty, so an untouched field already says what it will do.
    fn input(
        &mut self,
        placeholder: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<InputState> {
        if let Some(input) = self.input.clone() {
            if self.placeholder != placeholder {
                self.placeholder = placeholder.to_string();
                let text = SharedString::from(placeholder.to_string());
                input.update(cx, |input, cx| input.set_placeholder(text, window, cx));
            }
            return input;
        }
        self.placeholder = placeholder.to_string();
        let text = SharedString::from(placeholder.to_string());
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(text));
        self.input = Some(input.clone());
        input
    }

    /// What's typed in, trimmed. Empty is the caller's fallback name.
    fn value(&self, cx: &App) -> String {
        self.input
            .as_ref()
            .map(|input| input.read(cx).value().trim().to_string())
            .unwrap_or_default()
    }
}

/// The save-to-pool block both shader surfaces wear: a name, and the button
/// that puts this surface's inline shader into the workspace's shaders
/// under it. Promotion is how a shader stops belonging to one panel: from
/// here the pool holds the source, any other panel can wear the same name,
/// and one edit reaches all of them.
pub fn save_to_pool_block<P: 'static>(
    field: &mut ShaderNameField,
    fallback: &str,
    inert: bool,
    save: impl Fn(&mut P, String, &mut Context<P>) + 'static,
    window: &mut Window,
    cx: &mut Context<P>,
) -> Div {
    let input = field.input(fallback, window, cx);
    let typed = field.value(cx);
    let name = if typed.is_empty() {
        fallback.to_string()
    } else {
        typed
    };
    let replaces = settings::shader_pool_get(&name).is_some();
    let note: SharedString = if inert {
        "Nothing to save yet: pick a preset or a file first".into()
    } else if replaces {
        format!(
            "Saving replaces the shader this workspace already calls {name}, so every \
             panel wearing that name changes with it"
        )
        .into()
    } else {
        format!(
            "Hands the source to the workspace as {name}. The panel wears it by name \
             from then on, and so can any other panel"
        )
        .into()
    };
    let button = {
        let input = input.clone();
        let fallback = fallback.to_string();
        small_button(
            if replaces { "Replace" } else { "Save" },
            icons::PLUS,
            inert,
            cx.listener(move |this, _, _, cx| {
                let typed = input.read(cx).value().trim().to_string();
                let name = if typed.is_empty() {
                    fallback.clone()
                } else {
                    typed
                };
                if name.is_empty() {
                    return;
                }
                save(this, name, cx);
            }),
        )
    };
    let row = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_SM)
        .child(div().flex_1().min_w_0().child(Input::new(&input).small()))
        .child(button);
    div().flex().flex_col().gap(px(2.)).child(row).child(
        div()
            .text_xs()
            .text_color(palette::text_muted())
            .child(note),
    )
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
    let menu = crate::openers::add_panel_submenu(menu, tab_panel, window, cx);
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
    let state = panel.read(cx).state();
    let handle = crate::panel::open_child_window(cx, title, bounds, None, move |window, cx| {
        cx.new(|cx| RenameWindow::new(panel, state, window, cx))
    });
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
    /// The one readout being typed into across this window's sliders.
    value_edit: panel::ValueEdit,
    margin_scrub: ScrubState,
    padding_scrub: ScrubState,
    rounding_scrub: ScrubState,
    border_scrub: ScrubState,
    font_scale_scrub: ScrubState,
    /// The Shader page's route editor state: span sliders and which rows
    /// stand open, kept in step with the panel's route list. Ephemeral on
    /// purpose - a fold is where you are, not what you set.
    shader_routes: RouteEditState,
    /// The name a save-to-pool would land under, while it's being typed.
    shader_name: ShaderNameField,
    /// The size limit fields, typed in px; empty means no limit.
    min_width_input: Entity<InputState>,
    min_height_input: Entity<InputState>,
    max_width_input: Entity<InputState>,
    max_height_input: Entity<InputState>,
    _min_width_events: Subscription,
    _min_height_events: Subscription,
    _max_width_events: Subscription,
    _max_height_events: Subscription,
    /// The palette the swatches were last seeded from, the change check
    /// that keeps [`sync_swatches`](Self::sync_swatches) from re-seeding
    /// every frame. None until the appearance page first renders under
    /// the window's tint.
    swatch_resolve: Option<Palette>,
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
                s.windows.panel_settings = Some(settings::LayoutSize {
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
            value_edit: panel::ValueEdit::default(),
            margin_scrub: ScrubState::default(),
            padding_scrub: ScrubState::default(),
            rounding_scrub: ScrubState::default(),
            border_scrub: ScrubState::default(),
            font_scale_scrub: ScrubState::default(),
            shader_routes: RouteEditState::default(),
            shader_name: ShaderNameField::default(),
            min_width_input,
            min_height_input,
            max_width_input,
            max_height_input,
            _min_width_events,
            _min_height_events,
            _max_width_events,
            _max_height_events,
            swatch_resolve: None,
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

    /// Show or hide a composition host's corner slot controls. The toggle
    /// reads as "show", so it stores the inverse.
    fn set_panel_controls(&mut self, shown: bool, cx: &mut Context<Self>) {
        if let Some(panel) = self.panel.upgrade() {
            panel.update(cx, |panel, cx| panel.set_hide_controls(!shown, cx));
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

    /// Keep the swatches on the live palette: a swatch showing an
    /// inherited or linked color re-seeds when the resolve it was seeded
    /// from moves, the panel-window mirror of the app editor's side
    /// sync. Literal overrides hold as written. Runs from the appearance
    /// page's render, inside the window tint, since every palette change
    /// path (song theming, theme switches, palette edits) repaints all
    /// windows; the stored resolve keeps a settled palette from
    /// re-seeding every frame.
    fn sync_swatches(&mut self, theme: &PanelTheme, window: &mut Window, cx: &mut Context<Self>) {
        let resolve = palette::resolved();
        let moved = |last: &Palette| {
            ROLES
                .iter()
                .any(|role| (role.get)(last) != (role.get)(&resolve))
        };
        if self
            .swatch_resolve
            .as_ref()
            .is_some_and(|last| !moved(last))
        {
            return;
        }
        self.swatch_resolve = Some(resolve);
        for (role, picker) in ROLES.iter().zip(&self.pickers) {
            if theme.colors.contains_key(role.name) && theme.reference(role.name).is_none() {
                continue;
            }
            let color = theme
                .color(role.name)
                .unwrap_or_else(|| (role.get)(&resolve));
            picker.update(cx, |picker, cx| picker.set_value(color, window, cx));
        }
    }

    /// Point one role at another app color instead of a literal. The
    /// reference keeps tracking the live palette; the swatch takes the
    /// target's current resolve so the cell shows what now renders.
    fn set_role_reference(
        &mut self,
        index: usize,
        target: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let role = &ROLES[index];
        self.update_theme(|theme| theme.set_reference(role.name, target), cx);
        if let Some(target) = ROLES.iter().find(|role| role.name == target) {
            let color = (target.get)(&palette::resolved());
            self.pickers[index].update(cx, |picker, cx| picker.set_value(color, window, cx));
        }
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

    fn set_margin(&mut self, value: f32, cx: &mut Context<Self>) {
        self.update_theme(|theme| theme.margin = Some(value), cx);
    }

    fn set_padding(&mut self, value: f32, cx: &mut Context<Self>) {
        self.update_theme(|theme| theme.padding = Some(value), cx);
    }

    fn set_rounding(&mut self, value: f32, cx: &mut Context<Self>) {
        self.update_theme(|theme| theme.rounding = Some(value), cx);
    }

    fn set_border(&mut self, value: f32, cx: &mut Context<Self>) {
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

    /// Flip one side of the border mask. All four on is the default
    /// look, so it stores as no mask at all and the config stays clean.
    fn toggle_border_edge(&mut self, edge: BorderEdge, cx: &mut Context<Self>) {
        self.update_theme(
            |theme| {
                let edges = theme.border_edges.unwrap_or(BorderEdges::ALL).toggled(edge);
                theme.border_edges = (edges != BorderEdges::ALL).then_some(edges);
            },
            cx,
        );
    }

    /// The panel font size: the percent off the strip back into the
    /// multiplier the theme carries, forked as this panel's own override
    /// over the app size. The reset below sends it back to following the
    /// app.
    fn set_font_scale(&mut self, percent: f32, cx: &mut Context<Self>) {
        let scale = percent / 100.0;
        self.update_theme(|theme| theme.font_scale = Some(scale), cx);
    }

    fn reset_font_scale(&mut self, cx: &mut Context<Self>) {
        self.update_theme(|theme| theme.font_scale = None, cx);
    }

    /// The panel font-size row: the multiplier over its range, a percent
    /// readout alongside. Unset, the slider rests at 100% (follow the app
    /// size); once the panel forks its own, a reset joins on the left,
    /// where the row grows without nudging the slider or readout.
    fn font_scale_row(&self, value: Option<f32>, cx: &mut Context<Self>) -> Div {
        let scale = value.unwrap_or(1.0);
        let slider = settings_ui::scalar(
            &self.font_scale_scrub,
            &self.value_edit,
            scale * 100.0,
            settings_ui::span(
                palette::PANEL_FONT_SCALE_MIN * 100.0,
                palette::PANEL_FONT_SCALE_MAX * 100.0,
                "%",
            )
            .hard(),
            Self::set_font_scale,
            cx,
        );
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .when(value.is_some(), |row| {
                row.child(settings_ui::icon_button(
                    icons::REFRESH_CW,
                    false,
                    cx.listener(|this, _, _, cx| this.reset_font_scale(cx)),
                ))
            })
            .child(slider)
    }

    /// One frame knob's slider row: the value over its 0 to `max` range,
    /// the px readout alongside. Unset, the slider rests at the app-wide
    /// default the panel inherits; once the panel forks its own, a reset
    /// joins on the left of the strip, the size rows' placement, so the
    /// slider and readout hold still when it appears.
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
        // The strip's top is the everyday reach, not the law: a typed
        // value runs past it and the setters take what lands.
        let slider = settings_ui::scalar(
            scrub,
            &self.value_edit,
            shown,
            settings_ui::span(0., max, " px"),
            apply,
            cx,
        );
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .when(value.is_some(), |row| {
                row.child(settings_ui::icon_button(
                    icons::REFRESH_CW,
                    false,
                    cx.listener(move |this, _, _, cx| reset(this, cx)),
                ))
            })
            .child(slider)
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
                theme.border_edges = None;
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
        hide_controls: bool,
        composite: bool,
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
                Some(
                    "A drag anywhere on the panel moves the window, while plain clicks still \
                     land on its controls; for decorations-off layouts",
                ),
                panel::toggle(anchor, Self::set_panel_anchor, cx),
            ))
            // Only the composition hosts draw these, so the row would be a
            // dead switch on a leaf panel.
            .when(composite, |d| {
                d.child(panel::setting_row(
                    "Slot Controls",
                    Some(
                        "Show the corner buttons for swapping and removing the panels this one hosts. \
                         Hidden, the layout is still edited from the tree on the Workspace page in Settings",
                    ),
                    panel::toggle(!hide_controls, Self::set_panel_controls, cx),
                ))
            });
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

    /// The shared Shader page: a WGSL fragment stage over this panel's own
    /// surface, and the routes feeding its sixteen signal slots.
    ///
    /// No countdown confirm here, unlike the app-wide screen shader. That
    /// one exists because a hostile whole-window shader can bury the very
    /// control that would undo it; a panel shader leaves this window, the
    /// menus, and every other panel exactly where they were.
    fn shader_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let Some(panel) = self.panel.upgrade() else {
            return div();
        };
        // An edit picked up from the file while this window was closed is
        // running but not yet written down; fold it back before anything
        // reads the config, so what the page shows and what a layout dump
        // saves are both the text on screen.
        self.absorb_hot_source(cx);
        let configured = panel.read(cx).chrome().shader.clone();
        // A panel that has never been given a shader reads as off, whatever
        // the default a fresh config would carry.
        let enabled = configured.as_ref().is_some_and(|shader| shader.enabled);
        let shader = configured.unwrap_or_default();
        // What actually runs, which for a named panel is the pool's copy
        // rather than the inline text. The gate and the approval block both
        // read this, so a shader that arrived in a bundle can't slip past
        // them by riding in under a name with an empty source behind it.
        let running = shader::resolve_source(shader.name.as_deref(), &shader.source);
        let missing = shader.name.is_some() && running.is_none();
        let running = running.unwrap_or_default();
        // A source that arrived inside a layout or a bundle doesn't run
        // until it's read and approved here.
        let pending = (!running.trim().is_empty() && !shader::approved(&running)).then(|| {
            let approving = running.clone();
            let named = shader.name.is_some();
            pending_shader(
                "panel-shader-pending",
                &running,
                shader.path.as_deref(),
                cx.listener(move |this, _, _, cx| {
                    shader::approve(&approving);
                    // The path named a file on whichever machine wrote the
                    // bundle. If this one happens to have something there,
                    // the watch would pull it over the text just approved,
                    // so an imported shader keeps no bookmark.
                    this.edit_shader(|shader| shader.path = None, cx);
                }),
                cx.listener(move |this, _, _, cx| {
                    this.edit_shader(
                        move |shader| {
                            // Saying no to a pool shader takes the panel off
                            // it rather than emptying an inline source it
                            // wasn't running anyway. The entry stays in the
                            // workspace for whatever else wears it.
                            if named {
                                shader.name = None;
                            }
                            shader.source = String::new();
                            shader.path = None;
                        },
                        cx,
                    )
                }),
            )
        });
        // Only speak up about a compile while the thing is meant to run;
        // a message from before the switch went off is just noise.
        let error = (enabled && shader.runnable())
            .then(|| shader::error(panel.entity_id()))
            .flatten();
        // The slot names come off what runs, so a named panel reads the
        // pool's `// @slot n:` comments rather than the inline copy it left
        // behind when the name went on.
        let labels = shader::slot_labels(&running);
        self.shader_routes.sync(shader.routes.len());

        let named = shader.name.clone();
        let controls = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .child(small_button(
                "Reload",
                icons::REFRESH_CW,
                // A named panel's bookmark points at whatever it had inlined
                // before the name went on, so re-reading it would pull that
                // back over the pool's shader. The pool entry reloads itself.
                named.is_some() || shader.path.is_none(),
                cx.listener(|this, _, _, cx| this.reload_shader(cx)),
            ))
            .child(small_button(
                "Choose File",
                icons::FOLDER,
                false,
                cx.listener(|this, _, window, cx| this.pick_shader_file(window, cx)),
            ))
            // An inline shader ejects to a file from here; a named one ejects
            // through its pool entry, on the block above.
            .when(named.is_none(), |controls| {
                controls.child(small_button(
                    "Edit as File",
                    icons::EXTERNAL_LINK,
                    shader.source.trim().is_empty(),
                    cx.listener(|this, _, _, cx| this.eject_shader(cx)),
                ))
            });
        let source_note: SharedString = match (&named, &shader.path, shader.source.is_empty()) {
            (Some(name), _, _) => format!(
                "Choosing a file detaches this panel from {name} and gives it a source \
                 of its own"
            )
            .into(),
            (None, Some(path), _) => format!(
                "{}. The source is copied into the layout, so the panel keeps its \
                 shader on a machine that never had the file; Reload picks up edits",
                path.display()
            )
            .into(),
            (None, None, false) => "Loaded from a file that is no longer recorded; the source \
                                    rides the layout. Edit as File writes it back out and \
                                    picks the edits up as you save"
                .into(),
            (None, None, true) => "Pick a WGSL file with a fragment stage defining fs_user(uv). \
                                   Reading `screen` shades what the panel drew, `prev` gives it \
                                   a frame of feedback, and neither draws a plain quad"
                .into(),
        };
        let mut source = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(panel::setting_row(
                "Surface Shader",
                Some("Run a WGSL shader over this panel's body, under the app's screen shader"),
                panel::toggle(
                    enabled,
                    |this: &mut Self, on, cx| {
                        this.edit_shader(move |shader| shader.enabled = on, cx)
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row_dyn(
                "Shader File",
                Some(source_note),
                controls,
            ));
        if let Some(error) = error {
            source = source.child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(error),
            );
        }
        source = source.child(panel::setting_row(
            "Run When Idle",
            Some(
                "Keep drawing frames while the audio is silent. Off, the shader parks \
                 where it stands and the panel costs nothing",
            ),
            panel::toggle(
                shader.run_when_idle,
                |this: &mut Self, on, cx| {
                    this.edit_shader(move |shader| shader.run_when_idle = on, cx)
                },
                cx,
            ),
        ));

        // Wearing a pool shader, or the offer to hand this one over. Never
        // both: a panel is either pointing at the workspace's copy or
        // carrying its own.
        let pool = named.as_ref().map(|name| {
            pool_shader_block(
                name,
                missing,
                |this: &mut Self, cx| this.eject_shader(cx),
                |this: &mut Self, cx| this.detach_shader(cx),
                cx,
            )
        });
        let save = named.is_none().then(|| {
            let label = panel
                .read(cx)
                .custom_title()
                .map(str::to_string)
                .unwrap_or_else(|| panel::display_name(panel.read(cx).panel_name()));
            let fallback = shader::eject_name(&label, &shader.source);
            let empty = shader.source.trim().is_empty();
            save_to_pool_block(
                &mut self.shader_name,
                &fallback,
                empty,
                |this: &mut Self, name, cx| this.save_shader_to_pool(name, cx),
                window,
                cx,
            )
        });

        // The one route editor every shader surface wears, over this
        // panel's own list: the write goes back through `edit_shader`, so
        // the panel's config stays the only copy.
        let hub = self.state.signals.clone();
        let editor = signal_ui::routes::RouteEditor {
            id: "panel-shader-route",
            hub: &hub,
            routes: &shader.routes,
            labels: &labels,
            value_edit: &self.value_edit,
            ui: &self.shader_routes,
            ui_mut: |this: &mut Self| &mut this.shader_routes,
            mutate: Arc::new(
                |this: &mut Self, edit: &mut dyn FnMut(&mut Vec<Route>), cx: &mut Context<Self>| {
                    this.edit_shader(|shader| edit(&mut shader.routes), cx);
                },
            ),
        };
        let add = editor.add_button(cx);

        div()
            .flex()
            .flex_col()
            .gap(SECTION_GAP)
            .children(pending.map(|body| section("Awaiting Approval", None, body)))
            .children(pool.map(|body| section("Workspace Shader", None, body)))
            .child(section("Shader", None, source))
            .children(save.map(|body| section("Save to Shaders", None, body)))
            .child(section(
                "Signals",
                Some(add.into_any_element()),
                editor.list(cx),
            ))
    }

    /// Write a hot-reloaded source back into the panel's config, so the
    /// layout dump carries what's actually running. The wrapper paints from
    /// a file it can't write back through - it has the panel's id and
    /// nothing else - so the fold happens here, where the panel is typed.
    fn absorb_hot_source(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self.panel.upgrade() else {
            return;
        };
        let Some(hot) = shader::hot_source(panel.entity_id()) else {
            return;
        };
        let stale = panel
            .read(cx)
            .chrome()
            .shader
            .as_ref()
            .is_some_and(|shader| shader.source != hot);
        if stale {
            panel.update(cx, |panel, cx| {
                if let Some(shader) = panel.chrome_mut().shader.as_mut() {
                    shader.source = hot;
                }
                cx.notify();
            });
        }
    }

    /// Edit the panel's shader config, seeding a default one on first
    /// touch. The stored compile message goes with it: whatever it said
    /// was about a source that just moved.
    fn edit_shader(&mut self, edit: impl FnOnce(&mut shader::PanelShader), cx: &mut Context<Self>) {
        let Some(panel) = self.panel.upgrade() else {
            return;
        };
        shader::note_error(panel.entity_id(), None);
        panel.update(cx, |panel, cx| {
            edit(
                panel
                    .chrome_mut()
                    .shader
                    .get_or_insert_with(shader::PanelShader::default),
            );
            cx.notify();
        });
        cx.notify();
        // The compile message is written where the shader paints, which is
        // the panel's window drawing after this one - so the readout here
        // would sit a frame behind, and with a broken shader asking for no
        // frames, sit there. One nudge once the draw has landed.
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
    }

    /// Browse for a shader file. The source is copied into the panel's
    /// config on the way in, so the path is only ever a bookmark for
    /// Reload - a layout or a workspace bundle carries the shader itself.
    fn pick_shader_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
            this.update(cx, |this, cx| this.load_shader_file(path, cx))
                .ok();
        })
        .detach();
    }

    /// Re-read the file the shader came from. The panel watches it while it
    /// draws, so this is for a panel that has been sitting parked, or an
    /// edit that landed between stats and shouldn't have to wait.
    fn reload_shader(&mut self, cx: &mut Context<Self>) {
        let path = self
            .panel
            .upgrade()
            .and_then(|panel| panel.read(cx).chrome().shader.as_ref()?.path.clone());
        if let Some(path) = path {
            self.load_shader_file(path, cx);
        }
    }

    /// Write the panel's shader out to a file and hand it to whatever opens
    /// `.wgsl` on this machine. rox has no editor of its own, so this plus
    /// the file watch is the authoring loop.
    ///
    /// An inline shader keeps the bookmark, which is what puts its file
    /// under the panel's own watch. A named one ejects through its pool
    /// entry instead, and the bookmark lands there: the panel is wearing the
    /// workspace's shader, so the edits belong to every panel that is.
    fn eject_shader(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self.panel.upgrade() else {
            return;
        };
        let shader = panel.read(cx).chrome().shader.clone().unwrap_or_default();
        let ejected = match shader.name.as_deref() {
            Some(name) => shader::eject_pool_entry(name),
            None => {
                let label = panel
                    .read(cx)
                    .custom_title()
                    .map(str::to_string)
                    .unwrap_or_else(|| panel::display_name(panel.read(cx).panel_name()));
                shader::eject(&shader::eject_name(&label, &shader.source), &shader.source)
            }
        };
        match ejected {
            Ok(path) => {
                if shader.name.is_none() {
                    let bookmark = path.clone();
                    self.edit_shader(move |shader| shader.path = Some(bookmark), cx);
                }
                cx.open_with_system(&path);
            }
            Err(error) => {
                shader::note_error(panel.entity_id(), Some(format!("ejecting: {error}")));
                cx.notify();
            }
        }
    }

    /// Take a copy of the pool shader this panel is wearing and stop
    /// wearing it. The text is the same one that was already running, so
    /// its approval carries and nothing has to be agreed to twice.
    ///
    /// No bookmark comes across. The pool entry's file belongs to the pool,
    /// and a second watcher on it would have this panel and the workspace's
    /// shader drift apart on the next save.
    fn detach_shader(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self.panel.upgrade() else {
            return;
        };
        let name = panel
            .read(cx)
            .chrome()
            .shader
            .as_ref()
            .and_then(|shader| shader.name.clone());
        let Some(entry) = name.as_deref().and_then(settings::shader_pool_get) else {
            return;
        };
        self.edit_shader(
            move |shader| {
                shader.source = entry.source;
                shader.name = None;
                shader.path = None;
            },
            cx,
        );
    }

    /// Promote the panel's inline shader into the workspace's shaders and
    /// wear it by name from there. The inline copy goes: the pool holds the
    /// source now, and a second copy sitting in the panel would only be the
    /// one that's wrong after the next pool edit.
    fn save_shader_to_pool(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(panel) = self.panel.upgrade() else {
            return;
        };
        let Some(shader) = panel.read(cx).chrome().shader.clone() else {
            return;
        };
        let name = name.trim().to_string();
        if name.is_empty() || shader.source.trim().is_empty() {
            return;
        }
        // The panel's own bookmark rides along, so a shader that was being
        // edited in a file goes on hot reloading through the pool's watch.
        crate::panel::shader::save_to_pool(&name, &shader.source, shader.path.clone());
        self.edit_shader(
            move |shader| {
                shader.name = Some(name);
                shader.source = String::new();
                shader.path = None;
            },
            cx,
        );
    }

    /// Snapshot a file into the panel's shader source. A file that won't
    /// read lands in the same readout a failed compile does.
    ///
    /// Picking a file is the user putting the source there, so it approves
    /// itself on the way in; the gate is for sources that arrive inside a
    /// layout or a workspace bundle without anyone choosing them. It's also
    /// how a panel comes off a pool shader by picking a different one: you
    /// asked for this file, so the name goes and the panel carries its own
    /// source from here.
    fn load_shader_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        match std::fs::read_to_string(&path) {
            Ok(source) => self.edit_shader(
                move |shader| {
                    crate::panel::shader::approve(&source);
                    shader.source = source;
                    shader.name = None;
                    shader.path = Some(path);
                },
                cx,
            ),
            Err(error) => {
                if let Some(panel) = self.panel.upgrade() {
                    shader::note_error(
                        panel.entity_id(),
                        Some(format!("reading {}: {error}", path.display())),
                    );
                }
                cx.notify();
            }
        }
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
                    settings_ui::slider_edit(
                        &self.opacity_scrub,
                        &self.value_edit,
                        value,
                        Self::set_opacity,
                        cx,
                    ),
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
            ))
            .child(panel::setting_row(
                "Border edges",
                Some("Which sides the border draws on"),
                {
                    let edges = theme.border_edges.unwrap_or(BorderEdges::ALL);
                    panel::icon_toggles(
                        &[
                            (icons::PANEL_LEFT, BorderEdge::Left),
                            (icons::PANEL_TOP, BorderEdge::Top),
                            (icons::PANEL_BOTTOM, BorderEdge::Bottom),
                            (icons::PANEL_RIGHT, BorderEdge::Right),
                        ],
                        move |edge| edges.get(edge),
                        Self::toggle_border_edge,
                        cx,
                    )
                },
            ));

        let overridden = |name: &str| theme.colors.contains_key(name);
        let weak = cx.entity().downgrade();
        let grid = settings_ui::role_grid(columns, |j| {
            let role = &ROLES[j];
            // The picker pads a 4px margin around its swatch square; the
            // counter-margin keeps the cell at the grid's 20px footprint.
            let control = ColorPicker::new(&self.pickers[j])
                .small()
                .m(px(-4.))
                .into_any_element();
            // The link ties this role to another app color: its menu lists
            // the palette by group, the current target checked. Linked
            // cells fill the button with the accent so a reference reads
            // apart from a literal fork at a glance.
            let linked = theme.reference(role.name);
            let pick = weak.clone();
            let link = Button::new(("role-link", j))
                .icon(Icon::default().path(icons::LINK))
                .xsmall()
                .map(|b| {
                    if linked.is_some() {
                        b.primary()
                    } else {
                        b.ghost()
                    }
                })
                .dropdown_menu(move |mut menu, _, _| {
                    menu = menu.scrollable(true).max_h(px(320.));
                    let mut group = "";
                    for target in ROLES {
                        // A role following itself would just read as the
                        // app value, so the cell's own role stays out.
                        if target.name == ROLES[j].name {
                            continue;
                        }
                        if target.group != group {
                            group = target.group;
                            menu = menu.item(PopupMenuItem::label(group));
                        }
                        let pick = pick.clone();
                        menu = menu.item(
                            PopupMenuItem::new(target.label)
                                .checked(linked == Some(target.name))
                                .on_click(move |_, window, cx| {
                                    if let Some(this) = pick.upgrade() {
                                        this.update(cx, |this, cx| {
                                            this.set_role_reference(j, target.name, window, cx)
                                        });
                                    }
                                }),
                        );
                    }
                    menu
                })
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
            let trailing = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(2.))
                .child(link)
                .when_some(reset, |d, reset| d.child(reset))
                .into_any_element();
            settings_ui::color_cell(control, role.label, overridden(role.name), Some(trailing))
                .into_any_element()
        });
        let body = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_XS)
            .child(div().text_xs().text_color(palette::text_muted()).child(
                "Overrides recolor just this panel and hold still under song \
                 theming; a link follows another app color instead, moving \
                 with the theme. Reset a swatch or clear its hex field to \
                 follow the app palette again",
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
            // The section Reset drops both font overrides at once, inert
            // until one is set; the size row also carries its own inline
            // reset, the frame sliders' pattern.
            let reset = small_button(
                "Reset",
                icons::REFRESH_CW,
                theme.font.is_none() && theme.font_scale.is_none(),
                cx.listener(|this, _, _, cx| {
                    this.update_theme(
                        |theme| {
                            theme.font = None;
                            theme.font_scale = None;
                        },
                        cx,
                    )
                }),
            );
            let body = div()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_MD)
                .child(panel::setting_row(
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
                ))
                .child(panel::setting_row(
                    "Font Size",
                    Some("The panel's text size relative to the app font; rows scale with it"),
                    self.font_scale_row(theme.font_scale, cx),
                ));
            section("Font", Some(reset.into_any_element()), body).into_any_element()
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

        // The window renders under the player's art tint like the
        // workspace that opened it, and claims the widget theme while it
        // holds focus, so the panel's settings read in the playing track's
        // colors. Everything builds inside the tint: color reads resolve
        // eagerly as the div chains build, so a nav or page built before
        // the wrap would carry the untinted base palette instead.
        let player = self.state.player.entity_id();
        palette::note_focus(player, window.is_window_active(), cx);
        panel::window_body(player, || {
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
                    // Appearance, Behavior and Shader lead the nav on every
                    // panel, the app settings window's order, so how a panel
                    // looks and how it acts always sit in the same spots no
                    // matter what pages it brings. `page` 0 is Appearance, 1
                    // is Behavior, 2 is Shader, and the panel's own pages
                    // follow at 3..
                    let surface_shader = panel.read(cx).surface_shader();
                    let mut picked = self.page.min(pages.len() + 2);
                    // A panel that opted out of the shared page (the Shader
                    // panel, whose body already is one) keeps the numbering so
                    // its own pages stay at 3.., but slot 2 falls back to
                    // Appearance instead of a nav item it doesn't show.
                    if !surface_shader && picked == 2 {
                        picked = 0;
                    }
                    let mut nav = sidebar()
                        .child(settings_ui::nav_item(
                            "Appearance",
                            icons::PALETTE,
                            picked == 0,
                            move |this: &mut Self, _window, cx| {
                                this.page = 0;
                                cx.notify();
                            },
                            cx,
                        ))
                        .child(settings_ui::nav_item(
                            "Behavior",
                            icons::SLIDERS,
                            picked == 1,
                            move |this: &mut Self, _window, cx| {
                                this.page = 1;
                                cx.notify();
                            },
                            cx,
                        ));
                    if surface_shader {
                        nav = nav.child(settings_ui::nav_item(
                            "Shader",
                            icons::BLEND,
                            picked == 2,
                            move |this: &mut Self, _window, cx| {
                                this.page = 2;
                                cx.notify();
                            },
                            cx,
                        ));
                    }
                    for (i, &(label, icon)) in pages.iter().enumerate() {
                        let page = i + 3;
                        nav = nav.child(settings_ui::nav_item(
                            label,
                            icon,
                            picked == page,
                            move |this: &mut Self, _window, cx| {
                                this.page = page;
                                cx.notify();
                            },
                            cx,
                        ));
                    }
                    let body = match picked {
                        0 => {
                            let theme = panel.read(cx).theme();
                            self.sync_swatches(&theme, window, cx);
                            let own_font = panel.read(cx).has_own_font();
                            let extra = panel.update(cx, |panel, cx| panel.appearance(window, cx));
                            self.appearance_page(&theme, extra, own_font, columns, cx)
                                .into_any_element()
                        }
                        1 => {
                            // Read through chrome() so the call isn't ambiguous
                            // between PanelSettings::locked and the dock's
                            // Panel::locked, which share the name.
                            let composite = panel.read(cx).composite();
                            let (locked, anchor, hide_controls, limits) = {
                                let chrome = panel.read(cx).chrome();
                                (
                                    chrome.locked,
                                    chrome.anchor,
                                    chrome.hide_controls,
                                    SizeLimits {
                                        min_width: chrome.min_width,
                                        min_height: chrome.min_height,
                                        max_width: chrome.max_width,
                                        max_height: chrome.max_height,
                                    },
                                )
                            };
                            let extra = panel.update(cx, |panel, cx| panel.behavior(window, cx));
                            self.behavior_page(
                                locked,
                                anchor,
                                hide_controls,
                                composite,
                                limits,
                                extra,
                                cx,
                            )
                            .into_any_element()
                        }
                        2 => self.shader_page(window, cx).into_any_element(),
                        _ => panel
                            .update(cx, |panel, cx| panel.page(pages[picked - 3].0, window, cx)),
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
                        // Fades out when idle, same as the panels.
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .child(Scrollbar::vertical(&self.scroll)),
                        ),
                )
                .into_any_element()
        })
    }
}
