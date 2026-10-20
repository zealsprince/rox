//! Every panel's settings window: one OS window per panel, with a sidebar
//! that leads with the shared Appearance, Behavior and Shader pages and
//! follows with the panel's own. Appearance edits the panel's palette
//! override (ADR 13). Edits apply live and persist through the layout dump.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyElement, App, Bounds, Context, Div, Entity, EntityId, Focusable as _, Global, Hsla,
    KeyBinding, MouseButton, PathPromptOptions, ScrollHandle, SharedString, Subscription,
    WeakEntity, Window, WindowHandle, actions, div, prelude::*, px, size, svg,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_component::scroll::Scrollbar;
use gpui_component::{Icon, Root, Sizable as _};

use crate::openers::{PresetApply, Rebuilt};
use crate::panel::{self, AppState, PanelSettings, ScrubState, shader};
use crate::query::search::{SearchBox, SearchEvent};
use rox_core::settings;
use rox_design::assets::icons;
use rox_design::palette::{self, Palette, PanelTheme, ROLES, Side, Sides};
use rox_design::tokens;
use rox_services::backdrop::WindowBackdrop;
// The frame sliders' ceilings are shared with the app settings window, so
// both frames scrub the same range.
use crate::signal_ui::{self, routes::RouteEditState};
use rox_core::settings::panel_presets::{PresetEntry, PresetScope, SaveScope};
use rox_core::settings::{BORDER_MAX, MARGIN_MAX, PADDING_MAX, PanelPreset, ROUNDING_MAX};
use rox_dock::{PanelView, TabPanel};
use rox_panel_kit::ui::{
    self as settings_ui, PageBody, Query, SECTION_GAP, Section, Seg, SidesScrub, grid_columns,
    kbd_line, section, sidebar, small_button,
};
use rox_viz::signal::Route;

actions!(panel_settings, [Rename, SavePreset]);

/// The [`Panel::open_settings`](rox_dock::Panel::open_settings) override for
/// a panel with settings. Drop `opens_settings!();` into the panel's `impl
/// Panel` block. A macro because [`open`] needs the concrete type, and the
/// trait's default can't specialize on [`PanelSettings`].
#[macro_export]
macro_rules! opens_settings {
    () => {
        fn open_settings(&mut self, _: &mut gpui::Window, cx: &mut gpui::Context<Self>) {
            // Deferred: this runs while the entity is leased for this update, and
            // `open` reads it, which would panic.
            let panel = cx.entity();
            cx.defer(move |cx| $crate::panel_settings::open(panel, cx));
        }
    };
}

const RENAME_CONTEXT: &str = "PanelRename";
const PRESET_CONTEXT: &str = "PanelSavePreset";

/// The dialogs' enter bindings. They sit on each window's root rather than
/// its field, so enter commits wherever focus is.
pub fn bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("enter", Rename, Some(RENAME_CONTEXT)),
        KeyBinding::new("enter", SavePreset, Some(PRESET_CONTEXT)),
    ]
}

/// Open panel settings windows by panel, so opening again focuses the
/// window. A closed window's stale handle fails to activate, and the next
/// open replaces it.
#[derive(Default)]
struct OpenPanelSettings(HashMap<EntityId, WindowHandle<Root>>);

impl Global for OpenPanelSettings {}

/// Per rebuilt panel, by its id: what it replaced, so its settings window
/// offers one step back, and the preset that built it, so the picker can
/// name it while the panel still matches. Undo takes the step; no redo.
#[derive(Default)]
struct PresetMarks {
    undo: HashMap<EntityId, PanelPreset>,
    applied: HashMap<EntityId, PanelPreset>,
}

impl Global for PresetMarks {}

impl PresetMarks {
    fn note(&mut self, id: EntityId, previous: Option<PanelPreset>, applied: Option<PanelPreset>) {
        match previous {
            Some(previous) => self.undo.insert(id, previous),
            None => self.undo.remove(&id),
        };
        match applied {
            Some(applied) => self.applied.insert(id, applied),
            None => self.applied.remove(&id),
        };
    }

    fn take_undo(&mut self, id: EntityId) -> Option<PanelPreset> {
        self.undo.remove(&id)
    }

    fn has_undo(&self, id: EntityId) -> bool {
        self.undo.contains_key(&id)
    }

    fn applied(&self, id: EntityId) -> Option<&PanelPreset> {
        self.applied.get(&id)
    }
}

/// Record a rebuild: what `rebuilt` replaced, for Undo Preset, and the preset
/// it was built from, for the picker's label. None clears either.
pub fn note_preset_apply(
    rebuilt: EntityId,
    previous: Option<PanelPreset>,
    applied: Option<PanelPreset>,
    cx: &mut App,
) {
    cx.default_global::<PresetMarks>()
        .note(rebuilt, previous, applied);
}

/// The Panel Settings menu entry. A panel hosted in a composite gets its
/// host's settings row right after, so the container is reachable.
pub fn settings_item<P: PanelSettings>(menu: PopupMenu, panel: &Entity<P>, cx: &App) -> PopupMenu {
    let child = panel.entity_id();
    let panel = panel.clone();
    let menu = menu.item(
        PopupMenuItem::new(rox_i18n::t!("panel-settings"))
            .icon(Icon::default().path(icons::SETTINGS))
            .on_click(move |_, _, cx| {
                open(panel.clone(), cx);
            }),
    );
    crate::openers::host_settings_item(menu, child, cx)
}

/// The page a settings window should open on, taken by its next render so
/// an already-open window jumps too.
#[derive(Default)]
struct RequestedPage(HashMap<EntityId, usize>);

impl Global for RequestedPage {}

/// Open a panel's settings on one of its own pages, by its
/// [`PanelSettings::pages`] label. An unknown label opens the window as usual.
pub fn open_page<P: PanelSettings>(panel: Entity<P>, page: &str, cx: &mut App) {
    let index = panel
        .read(cx)
        .pages()
        .iter()
        .position(|(label, _)| *label == page);
    if let Some(index) = index {
        // The shared pages lead the nav, so a panel's own pages start at 3.
        cx.default_global::<RequestedPage>()
            .0
            .insert(panel.entity_id(), index + 3);
    }
    open(panel, cx);
}

/// Open a panel's settings window, or bring its open one forward. The
/// window holds the panel weakly.
pub fn open<P: PanelSettings>(panel: Entity<P>, cx: &mut App) {
    let id = panel.entity_id();
    if let Some(handle) = cx
        .try_global::<OpenPanelSettings>()
        .and_then(|open| open.0.get(&id).copied())
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let title = SharedString::from(format!(
        "rox - {} settings",
        panel::display_name(panel.read(cx).panel_name())
    ));
    // Floored at MIN_SIZE so a stale small frame never opens under the minimum.
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

/// The app settings page a panel asked for, taken by that window so an
/// already-open one jumps too.
#[derive(Default)]
struct RequestedAppPage(Option<&'static str>);

impl Global for RequestedAppPage {}

/// gpui's name for the action that opens the app settings window, which is
/// declared in the workspace. The settings window tests it against its own
/// type so a rename can't quietly break the jump.
pub const SETTINGS_ACTION: &str = "rox::OpenSettings";

/// Open the app settings window on the page `key` names
/// ("settings-page-radio" and its neighbours). The window lives in the
/// binary, so this leaves the page behind and dispatches the action by
/// name. A key the window doesn't know leaves it where it was.
pub fn open_app_page(key: &'static str, window: &mut Window, cx: &mut App) {
    cx.set_global(RequestedAppPage(Some(key)));

    let Ok(action) = cx.build_action(SETTINGS_ACTION, None) else {
        log::warn!("the settings window's action is not registered");
        return;
    };

    window.dispatch_action(action, cx);
}

/// Cleared as it's read, so the next open lands where the user left it.
pub fn requested_app_page(cx: &mut App) -> Option<&'static str> {
    cx.default_global::<RequestedAppPage>().0.take()
}

/// Long enough to read a real shader, short enough that a file someone
/// pasted a novel into doesn't build ten thousand elements.
const PENDING_LINES: usize = 400;

/// The approval block both shader surfaces show. Shaders travel inside
/// layout dumps and workspace bundles as plain WGSL, so applying somebody's
/// look hands rox their code; this is where a person reads it before it
/// runs. Read-only. Saying no parks the shader with its switch off rather
/// than deleting it.
pub fn pending_shader(
    id: &'static str,
    source: &str,
    path: Option<&Path>,
    approve: impl Fn(&settings_ui::Press, &mut Window, &mut App) + 'static,
    turn_off: impl Fn(&settings_ui::Press, &mut Window, &mut App) + 'static,
) -> Div {
    let lines: Vec<String> = source
        .lines()
        .take(PENDING_LINES)
        .map(str::to_string)
        .collect();
    let clipped = source.lines().count().saturating_sub(lines.len());
    let origin: SharedString = match path {
        Some(path) => rox_i18n::t!(
            "shader-pending-origin-file",
            path = path.display().to_string()
        ),
        None => rox_i18n::t!("shader-pending-origin-inline"),
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
            listing.child(div().text_color(palette::text_faint()).child(rox_i18n::t!(
                "shader-pending-more-lines",
                count = clipped as u64
            )))
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
                .child(small_button(
                    rox_i18n::t!("panel-approve"),
                    icons::CHECK,
                    false,
                    approve,
                ))
                .child(small_button(
                    rox_i18n::t!("panel-turn-off"),
                    icons::CLOSE,
                    false,
                    turn_off,
                )),
        )
}

/// The name field behind the save row. Built at first render because a
/// panel only has a `Window` then, and kept so a half-typed name survives
/// the repaint a recompile brings.
#[derive(Default)]
pub struct ShaderNameField {
    input: Option<Entity<InputState>>,
    /// Kept so the field follows a rename without setting it every render,
    /// which would notify the input into a frame of its own each time.
    placeholder: String,
}

impl ShaderNameField {
    /// The placeholder is the name a save would use with the field left empty.
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

    fn value(&self, cx: &App) -> String {
        self.input
            .as_ref()
            .map(|input| input.read(cx).value().trim().to_string())
            .unwrap_or_default()
    }
}

/// The save row: a name, and a button that puts this surface's shader into
/// the workspace's shaders under it, where any panel can use it and one
/// edit reaches all of them. A plain row rather than a picker entry: inputs
/// defer and so does the picker's popup, and gpui 0.2.2 panics on a
/// deferred element that defers again.
fn save_block<P: 'static>(
    field: &mut ShaderNameField,
    fallback: &str,
    save: fn(&mut P, String, &mut Context<P>),
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
    let note: SharedString = if replaces {
        rox_i18n::t!("shader-save-replaces", name = name.clone())
    } else {
        rox_i18n::t!("shader-save-adds", name = name.clone())
    };
    let button = {
        let input = input.clone();
        let fallback = fallback.to_string();
        small_button(
            if replaces {
                rox_i18n::t!("shader-save-replace")
            } else {
                rox_i18n::t!("shader-save-to-workspace")
            },
            icons::PLUS,
            false,
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

/// The picker's runs as `(overlay, examples heading, workspace heading)`,
/// pulled out so a test can read the rule. Unfiltered, the headings name
/// what a shader does to the surface; filtered to overlays, they name where
/// it came from.
fn shader_groups(overlays_only: bool) -> Vec<(bool, SharedString, SharedString)> {
    if overlays_only {
        vec![(
            true,
            rox_i18n::t!("shader-group-examples"),
            rox_i18n::t!("shader-group-this-workspace"),
        )]
    } else {
        vec![
            (
                false,
                rox_i18n::t!("shader-group-scenes"),
                rox_i18n::t!("shader-group-workspace-scenes"),
            ),
            (
                true,
                rox_i18n::t!("shader-group-overlays"),
                rox_i18n::t!("shader-group-workspace-overlays"),
            ),
        ]
    }
}

/// The picker both shader surfaces lead with, then only the rows the
/// selection needs (an example, a workspace shader, a file, or text that
/// came in on a layout). The actions are `fn` pointers because the popup
/// holds them across a `'static` menu closure.
pub struct ShaderSource<'a, P: 'static> {
    /// Element id prefix. Two surfaces can have settings open at once, and a
    /// shared id would put them on one popup's state.
    pub id: &'static str,
    /// The workspace shader this config names, if it names one.
    pub name: Option<&'a str>,
    /// The file the source was last read from, a bookmark for reloads.
    pub path: Option<&'a Path>,
    /// What runs: the workspace's copy under a name, the config's own source
    /// otherwise, None when a name resolves to nothing.
    pub resolved: Option<&'a str>,
    /// Some puts a None entry at the top of the list.
    pub clear: Option<fn(&mut P, &mut Context<P>)>,
    /// Offer only shaders that declare `// @overlay`, for every surface with an
    /// app underneath to lose. The Shader panel is the one caller that leaves
    /// this false. It filters what can be picked, never what's installed: a
    /// config that arrived holding a scene keeps running.
    pub overlays_only: bool,
    pub use_example: fn(&mut P, usize, &mut Context<P>),
    pub use_named: fn(&mut P, String, &mut Context<P>),
    pub choose_file: fn(&mut P, &mut Window, &mut Context<P>),
    /// Open the in-app editor over what runs.
    pub edit: fn(&mut P, &mut Window, &mut Context<P>),
    pub eject: fn(&mut P, &mut Context<P>),
    pub detach: fn(&mut P, &mut Context<P>),
    pub reload: fn(&mut P, &mut Context<P>),
    pub save: fn(&mut P, String, &mut Context<P>),
    pub field: &'a mut ShaderNameField,
    /// The name a save uses with that field left empty.
    pub fallback: &'a str,
}

impl<P: 'static> ShaderSource<'_, P> {
    pub fn render(self, window: &mut Window, cx: &mut Context<P>) -> Div {
        let ShaderSource {
            id,
            name,
            path,
            resolved,
            clear,
            overlays_only,
            use_example,
            use_named,
            choose_file,
            edit,
            eject,
            detach,
            reload,
            save,
            field,
            fallback,
        } = self;
        let choice = shader::pick(name, path, resolved);

        // From File sits at the top with None rather than under the long example
        // list: it's the authoring loop's front door.
        let pool = settings::shader_pool();
        let current = choice.clone();
        let host = cx.entity().downgrade();
        let picker = settings_ui::select_field(
            SharedString::from(format!("{id}-source")),
            shader::pick_label(&choice),
            matches!(choice, shader::Pick::Empty),
        )
        .dropdown_menu(move |mut menu, _, _| {
            menu = menu.scrollable(true).max_h(px(320.));
            if let Some(clear) = clear {
                let host = host.clone();
                menu = menu.item(
                    PopupMenuItem::new(rox_i18n::t!("shader-pick-none"))
                        .checked(matches!(current, shader::Pick::Empty))
                        .on_click(move |_, _, cx| {
                            if let Some(host) = host.upgrade() {
                                host.update(cx, clear);
                            }
                        }),
                );
            }
            {
                let host = host.clone();
                menu = menu
                    .item(
                        PopupMenuItem::new(rox_i18n::t!("shader-from-file"))
                            .icon(Icon::default().path(icons::FOLDER))
                            .on_click(move |_, window, cx| {
                                if let Some(host) = host.upgrade() {
                                    host.update(cx, |this, cx| choose_file(this, window, cx));
                                }
                            }),
                    )
                    .separator();
            }
            // Both lists split by what the shader does to the surface under it: a
            // scene replaces it, an overlay leaves it usable. That keeps "where did my
            // library go" from being how anyone learns the difference.
            for (overlay, examples, workspace) in shader_groups(overlays_only) {
                menu = menu.item(PopupMenuItem::label(examples));
                for (index, preset) in shader::PRESETS.iter().enumerate() {
                    if shader::overlay(preset.source) != overlay {
                        continue;
                    }
                    let host = host.clone();
                    menu = menu.item(
                        PopupMenuItem::new(preset.label)
                            .checked(current == shader::Pick::Example(index))
                            .on_click(move |_, _, cx| {
                                if let Some(host) = host.upgrade() {
                                    host.update(cx, |this, cx| use_example(this, index, cx));
                                }
                            }),
                    );
                }
                let worn: Vec<_> = pool
                    .iter()
                    .filter(|entry| shader::overlay(&entry.source) == overlay)
                    .collect();
                if worn.is_empty() {
                    continue;
                }
                menu = menu.item(PopupMenuItem::label(workspace));
                for entry in worn {
                    let host = host.clone();
                    let entry_name = entry.name.clone();
                    let checked = matches!(
                        &current,
                        shader::Pick::Named { name, .. } if name == &entry_name
                    );
                    menu = menu.item(
                        PopupMenuItem::new(entry.name.clone())
                            .checked(checked)
                            .on_click(move |_, _, cx| {
                                let entry_name = entry_name.clone();
                                if let Some(host) = host.upgrade() {
                                    host.update(cx, |this, cx| use_named(this, entry_name, cx));
                                }
                            }),
                    );
                }
            }
            menu
        });

        let note: SharedString = match &choice {
            shader::Pick::Empty => rox_i18n::t!("shader-note-empty"),
            shader::Pick::Example(index) => shader::pick_blurb(*index),
            shader::Pick::Named {
                name,
                missing: true,
            } => rox_i18n::t!("shader-note-missing", name = name.clone()),
            shader::Pick::Named { .. } => rox_i18n::t!("shader-note-shared"),
            shader::Pick::File(path) => {
                rox_i18n::t!("shader-note-file", path = path.display().to_string())
            }
            shader::Pick::Custom => rox_i18n::t!("shader-note-custom"),
        };

        let empty = matches!(choice, shader::Pick::Empty);
        let named = matches!(choice, shader::Pick::Named { .. });
        let missing = matches!(choice, shader::Pick::Named { missing: true, .. });
        let file = matches!(choice, shader::Pick::File(_));
        let actions = (!empty).then(|| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_SM)
                // The editor comes first: it's the way in for someone with no editor of
                // their own. A name the pool doesn't hold has no text to open on.
                .child(small_button(
                    rox_i18n::t!("shader-edit-here"),
                    icons::PENCIL,
                    missing,
                    cx.listener(move |this, _, window, cx| edit(this, window, cx)),
                ))
                // A file is already the editing surface; a second copy would only drift.
                .when(file, |row| {
                    row.child(small_button(
                        rox_i18n::t!("shader-reload"),
                        icons::REFRESH_CW,
                        false,
                        cx.listener(move |this, _, _, cx| reload(this, cx)),
                    ))
                })
                .when(!file, |row| {
                    row.child(small_button(
                        rox_i18n::t!("shader-edit-as-file"),
                        icons::EXTERNAL_LINK,
                        missing,
                        cx.listener(move |this, _, _, cx| eject(this, cx)),
                    ))
                })
                .when(named, |row| {
                    row.child(small_button(
                        rox_i18n::t!("shader-make-private-copy"),
                        icons::COPY,
                        missing,
                        cx.listener(move |this, _, _, cx| detach(this, cx)),
                    ))
                })
        });
        // Nothing to save without a source, and a workspace shader is already
        // where saving would put it.
        let save = (!empty && !named).then(|| save_block(field, fallback, save, window, cx));

        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(panel::setting_row_dyn(
                rox_i18n::t!("shader-source"),
                Some(note),
                picker,
            ))
            .children(actions)
            .children(save)
    }
}

/// The open rename windows, same stale-handle story as [`OpenPanelSettings`].
#[derive(Default)]
struct OpenRenames(HashMap<EntityId, WindowHandle<Root>>);

impl Global for OpenRenames {}

/// The head of a panel's dropdown tail: the Add Panel flyout, the Panel
/// section's divider and header, then Save As Preset and Rename. Every
/// panel's tail starts here, so this owns the leading separator; callers
/// pass their content items in with no separator of their own.
pub fn rename_item<P: PanelSettings>(
    menu: PopupMenu,
    panel: &Entity<P>,
    tab_panel: Option<WeakEntity<TabPanel>>,
    window: &mut Window,
    cx: &mut App,
) -> PopupMenu {
    // Out of design mode the section opens on Panel Settings alone: the flyout
    // and the two rows below are layout edits.
    if !settings::design_mode() {
        return menu.separator().label(rox_i18n::t!("panel-menu-label"));
    }
    let menu = crate::openers::add_panel_submenu(menu, tab_panel, window, cx);
    let saving = panel.clone();
    let panel = panel.clone();
    menu.separator()
        .label(rox_i18n::t!("panel-menu-label"))
        .item(
            PopupMenuItem::new(rox_i18n::t!("panel-save-as-preset"))
                .icon(Icon::default().path(icons::DOWNLOAD))
                .on_click(move |_, _, cx| {
                    open_save_preset(saving.clone(), cx);
                }),
        )
        .item(
            PopupMenuItem::new(rox_i18n::t!("panel-rename"))
                .icon(Icon::default().path(icons::PENCIL))
                .on_click(move |_, _, cx| {
                    open_rename(panel.clone(), cx);
                }),
        )
}

fn open_rename<P: PanelSettings>(panel: Entity<P>, cx: &mut App) {
    let id = panel.entity_id();
    if let Some(handle) = cx
        .try_global::<OpenRenames>()
        .and_then(|open| open.0.get(&id).copied())
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let title = SharedString::from(format!(
        "rox - rename {}",
        panel::display_name(panel.read(cx).panel_name())
    ));
    let bounds = Bounds::centered(None, size(px(380.), px(195.)), cx);
    let state = panel.read(cx).state();
    let handle = crate::panel::open_child_window(cx, title, bounds, None, move |window, cx| {
        cx.new(|cx| RenameWindow::new(panel, state, window, cx))
    });
    cx.default_global::<OpenRenames>().0.insert(id, handle);
}

/// One input over the panel's title. Edits apply as they're typed, Enter
/// closes, and clearing the field goes back to the built-in name.
struct RenameWindow<P: PanelSettings> {
    panel: WeakEntity<P>,
    input: Entity<InputState>,
    state: AppState,
    backdrop: WindowBackdrop,
    _input_events: Subscription,
    /// Its own wake on a new bake, since this window pumps its own frames.
    _backdrop_changed: Subscription,
}

impl<P: PanelSettings> RenameWindow<P> {
    fn new(panel: Entity<P>, state: AppState, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // The built-in name is the placeholder, so an empty field reads as the fallback.
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
        let _input_events =
            cx.subscribe_in(&input, window, |this, input, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    let value = input.read(cx).value().trim().to_string();
                    let title = (!value.is_empty()).then_some(value);
                    if let Some(panel) = this.panel.upgrade() {
                        panel.update(cx, |panel, cx| panel.set_custom_title(title, cx));
                    }
                }
            });
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

    /// The name was written to the panel as it was typed, so committing is closing.
    fn footer(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(tokens::SPACE_SM)
            .px(tokens::SPACE_MD)
            .py(tokens::SPACE_SM)
            .border_t_1()
            .border_color(palette::border())
            .bg(palette::bg_panel())
            .child(
                kbd_line([
                    Seg::Text(rox_i18n::t!("hint-press")),
                    Seg::Key(rox_i18n::t!("hint-key-enter")),
                    Seg::Text(rox_i18n::t!("panel-rename-hint-after")),
                ])
                .text_xs(),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .child(small_button(
                        rox_i18n::t!("panel-rename"),
                        icons::CHECK,
                        false,
                        cx.listener(|_, _, window, _| window.remove_window()),
                    ))
                    .child(small_button(
                        rox_i18n::t!("bake-cancel"),
                        icons::CLOSE,
                        false,
                        cx.listener(|_, _, window, _| window.remove_window()),
                    )),
            )
    }
}

impl<P: PanelSettings> Render for RenameWindow<P> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .key_context(RENAME_CONTEXT)
            .on_action(cx.listener(|_, _: &Rename, window, _| window.remove_window()))
            .bg(palette::bg_elevated())
            .text_color(palette::text_bright())
            .text_sm()
            .children(self.backdrop.layer(&self.state.now_art, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    // The page's own surface over the root's, so the backdrop reads through
                    // only as the surfaces thin.
                    .bg(palette::bg_elevated())
                    .p(tokens::SPACE_MD)
                    .child(section(
                        rox_i18n::t!("panel-rename-name"),
                        None,
                        div()
                            .flex()
                            .flex_col()
                            .gap(tokens::SPACE_XS)
                            .child(Input::new(&self.input).w_full())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(palette::text_muted())
                                    .child(rox_i18n::t!("panel-rename-note")),
                            ),
                    )),
            )
            .child(self.footer(cx))
    }
}

/// The open save-as-preset windows, same stale-handle story as [`OpenPanelSettings`].
#[derive(Default)]
struct OpenPresetSaves(HashMap<EntityId, WindowHandle<Root>>);

impl Global for OpenPresetSaves {}

/// Holds the panel weakly, so closing the panel under the dialog leaves a
/// window that saves nothing.
fn open_save_preset<P: PanelSettings>(panel: Entity<P>, cx: &mut App) {
    let id = panel.entity_id();
    if let Some(handle) = cx
        .try_global::<OpenPresetSaves>()
        .and_then(|open| open.0.get(&id).copied())
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let title = SharedString::from(format!(
        "rox - save {} as preset",
        panel::display_name(panel.read(cx).panel_name())
    ));
    let bounds = Bounds::centered(None, size(px(420.), px(300.)), cx);
    let state = panel.read(cx).state();
    let handle = crate::panel::open_child_window(cx, title, bounds, None, move |window, cx| {
        cx.new(|cx| SavePresetWindow::new(panel, state, window, cx))
    });
    cx.default_global::<OpenPresetSaves>().0.insert(id, handle);
}

/// Committing dumps the panel the way a layout save does and files it in
/// the workspace's presets, so adding the preset back rebuilds the panel
/// with its config, its rename and a composite's children.
struct SavePresetWindow<P: PanelSettings> {
    panel: WeakEntity<P>,
    input: Entity<InputState>,
    /// What an empty field saves under.
    fallback: SharedString,
    scope: SaveScope,
    /// Both tiers' names, read once at open, so the replaces note is a lookup
    /// rather than a file read per paint.
    taken_workspace: Vec<String>,
    taken_global: Vec<String>,
    state: AppState,
    backdrop: WindowBackdrop,
    _input_events: Subscription,
    /// Its own wake on a new bake, since this window pumps its own frames.
    _backdrop_changed: Subscription,
}

impl<P: PanelSettings> SavePresetWindow<P> {
    fn new(panel: Entity<P>, state: AppState, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // A renamed panel already has its preset's name; an unnamed one gets its kind.
        let (current, fallback) = {
            let panel = panel.read(cx);
            let fallback = panel::display_name(panel.panel_name());
            (
                panel
                    .custom_title()
                    .map(str::to_owned)
                    .unwrap_or_else(|| fallback.clone()),
                fallback,
            )
        };
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(fallback.clone())
                .default_value(current)
        });
        let _input_events = cx.subscribe_in(&input, window, |_, _, event: &InputEvent, _, cx| {
            // The footer's replaces note has to re-read on every keystroke.
            if let InputEvent::Change = event {
                cx.notify()
            }
        });
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        window.focus(&input.read(cx).focus_handle(cx));
        let saved = settings::Settings::load();
        let names = |presets: Vec<PanelPreset>| presets.into_iter().map(|p| p.name).collect();
        SavePresetWindow {
            panel: panel.downgrade(),
            input,
            fallback: fallback.into(),
            scope: SaveScope::Workspace,
            taken_workspace: names(settings::panel_presets::all(&saved)),
            taken_global: names(settings::panel_presets::global(&saved)),
            state,
            backdrop: WindowBackdrop::default(),
            _input_events,
            _backdrop_changed,
        }
    }

    fn taken(&self) -> &[String] {
        match self.scope {
            SaveScope::Workspace => &self.taken_workspace,
            SaveScope::Global => &self.taken_global,
        }
    }

    fn name(&self, cx: &App) -> String {
        let typed = self.input.read(cx).value().trim().to_string();
        if typed.is_empty() {
            self.fallback.to_string()
        } else {
            typed
        }
    }

    /// A panel that went away while the dialog stood open closes without writing.
    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name(cx);
        if let Some(panel) = self.panel.upgrade() {
            let dump = panel.read(cx).dump(cx);
            match serde_json::to_value(&dump) {
                Ok(value) => settings::panel_presets::save_to(self.scope, name, value),
                Err(e) => log::warn!("panel presets: {name} would not serialize: {e}"),
            }
        }
        window.remove_window();
    }

    fn footer(&self, replaces: bool, name: &str, cx: &mut Context<Self>) -> Div {
        let hint = if replaces {
            let note = match self.scope {
                SaveScope::Workspace => "preset-save-replaces",
                SaveScope::Global => "preset-save-replaces-global",
            };
            div()
                .text_xs()
                .text_color(palette::tone_warn())
                .child(rox_i18n::t!(note, name = name))
                .into_any_element()
        } else {
            kbd_line([
                Seg::Text(rox_i18n::t!("hint-press")),
                Seg::Key(rox_i18n::t!("hint-key-enter")),
                Seg::Text(rox_i18n::t!("preset-save-hint-after")),
            ])
            .text_xs()
            .into_any_element()
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(tokens::SPACE_SM)
            .px(tokens::SPACE_MD)
            .py(tokens::SPACE_SM)
            .border_t_1()
            .border_color(palette::border())
            .bg(palette::bg_panel())
            .child(hint)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .child(small_button(
                        if replaces {
                            rox_i18n::t!("shader-save-replace")
                        } else {
                            rox_i18n::t!("preset-save")
                        },
                        icons::DOWNLOAD,
                        false,
                        cx.listener(|this, _, window, cx| this.commit(window, cx)),
                    ))
                    .child(small_button(
                        rox_i18n::t!("bake-cancel"),
                        icons::CLOSE,
                        false,
                        cx.listener(|_, _, window, _| window.remove_window()),
                    )),
            )
    }
}

impl<P: PanelSettings> Render for SavePresetWindow<P> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let name = self.name(cx);
        let replaces = self.taken().iter().any(|taken| taken == &name);
        let tail = match self.scope {
            SaveScope::Workspace => "preset-back-tail",
            SaveScope::Global => "preset-back-tail-global",
        };
        let scope = panel::setting_row(
            rox_i18n::t!("preset-save-scope"),
            Some(rox_i18n::t!("preset-save-scope.description")),
            panel::choices_shared(
                &[
                    (rox_i18n::t!("preset-scope-workspace"), SaveScope::Workspace),
                    (rox_i18n::t!("preset-scope-global"), SaveScope::Global),
                ],
                self.scope,
                |this: &mut Self, scope, cx| {
                    this.scope = scope;
                    cx.notify();
                },
                cx,
            ),
        );
        div()
            .size_full()
            .flex()
            .flex_col()
            .key_context(PRESET_CONTEXT)
            .on_action(cx.listener(|this, _: &SavePreset, window, cx| this.commit(window, cx)))
            .bg(palette::bg_elevated())
            .text_color(palette::text_bright())
            .text_sm()
            .children(self.backdrop.layer(&self.state.now_art, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    // The page's own surface over the root's, so the backdrop reads through
                    // only as the surfaces thin.
                    .bg(palette::bg_elevated())
                    .p(tokens::SPACE_MD)
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_MD)
                    .child(section(
                        rox_i18n::t!("preset-save-name"),
                        None,
                        div()
                            .flex()
                            .flex_col()
                            .gap(tokens::SPACE_XS)
                            .child(Input::new(&self.input).w_full())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(palette::text_muted())
                                    // Keycaps, so the two menu labels read as things to click.
                                    .child(kbd_line([
                                        Seg::Text(rox_i18n::t!("preset-back-from")),
                                        Seg::Key(rox_i18n::t!("preset-back-add-panel")),
                                        Seg::Text(rox_i18n::t!("preset-back-then")),
                                        Seg::Key(rox_i18n::t!("preset-back-presets")),
                                        Seg::Text(rox_i18n::t!(tail)),
                                    ])),
                            ),
                    ))
                    .child(scope),
            )
            .child(self.footer(replaces, &name, cx))
    }
}

/// The chrome's size limits, for the Behavior page's Size rows.
#[derive(Clone, Copy, Default)]
struct SizeLimits {
    min_width: Option<f32>,
    min_height: Option<f32>,
    max_width: Option<f32>,
    max_height: Option<f32>,
}

/// Flatten a knob's sides onto the widest. A knob already uniform stays as
/// it was, so linking one doesn't fork it off the app default.
fn link_knob(own: &mut Option<Sides>, app: Sides) {
    let shown = own.unwrap_or(app);
    if shown.uniform().is_none() {
        *own = Some(shown.linked());
    }
}

/// Open split where the sides already differ, or the numbers on screen would lie.
fn split_knob(value: Sides) -> bool {
    value.uniform().is_none()
}

struct PanelSettingsWindow<P: PanelSettings> {
    panel: WeakEntity<P>,
    /// 0 is Appearance, 1 Behavior, 2 Shader, and the panel's own pages follow from 3.
    page: usize,
    /// One per palette role, in [`ROLES`] order.
    pickers: Vec<Entity<ColorPickerState>>,
    opacity_scrub: ScrubState,
    /// The one readout being typed into across this window's sliders.
    value_edit: panel::ValueEdit,
    margin_scrub: SidesScrub,
    padding_scrub: SidesScrub,
    rounding_scrub: ScrubState,
    border_scrub: SidesScrub,
    /// Which four-sided knobs are open per side. Window state, not the panel's,
    /// so a knob whose sides happen to match stays split while it's edited.
    margin_split: bool,
    padding_split: bool,
    border_split: bool,
    font_scale_scrub: ScrubState,
    slider_rounding_scrub: ScrubState,
    /// The Shader page's span sliders and open rows. Ephemeral: a fold is
    /// where you are, not what you set.
    shader_routes: RouteEditState,
    /// One drag state per shader slot, sized at [`SLOTS`](shader::SLOTS): the
    /// uniform block's width, not anything the config says.
    shader_slots: Vec<ScrubState>,
    shader_name: ShaderNameField,
    /// Typed in px; empty means no limit.
    min_width_input: Entity<InputState>,
    min_height_input: Entity<InputState>,
    max_width_input: Entity<InputState>,
    max_height_input: Entity<InputState>,
    _min_width_events: Subscription,
    _min_height_events: Subscription,
    _max_width_events: Subscription,
    _max_height_events: Subscription,
    /// The palette the swatches were last seeded from, so
    /// [`sync_swatches`](Self::sync_swatches) doesn't re-seed every frame.
    swatch_resolve: Option<Palette>,
    scroll: ScrollHandle,
    nav_scroll: ScrollHandle,
    search: Entity<SearchBox>,
    _search_changes: Subscription,
    state: AppState,
    backdrop: WindowBackdrop,
    _picker_changes: Vec<Subscription>,
    _panel_changed: Option<Subscription>,
    /// Its own wake on a new bake, since this window pumps its own frames.
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
        // The OS close button runs no teardown of ours, so save the frame from the
        // should-close hook. Shared across panels: the last closed window wins.
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
        // Empty reads as no limit, which the "Off" placeholder says.
        let chrome = panel
            .upgrade()
            .map(|panel| panel.read(cx).chrome().clone())
            .unwrap_or_default();
        let field = |value: Option<f32>, window: &mut Window, cx: &mut Context<Self>| {
            let seed = value.map(|n| format!("{n:.0}")).unwrap_or_default();
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(rox_i18n::t!("panel-size-off"))
                    .default_value(seed)
            })
        };
        let min_width_input = field(chrome.min_width, window, cx);
        let min_height_input = field(chrome.min_height, window, cx);
        let max_width_input = field(chrome.max_width, window, cx);
        let max_height_input = field(chrome.max_height, window, cx);
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
        let search = cx.new(|cx| {
            SearchBox::new(rox_i18n::t!("query-search"), "", window, cx)
                .small()
                .icon()
        });
        let _search_changes = cx.subscribe_in(
            &search,
            window,
            |_: &mut Self, _, event, window, cx| match event {
                SearchEvent::Changed => cx.notify(),
                SearchEvent::Dismissed => {
                    window.blur();
                    cx.notify();
                }
                SearchEvent::Submitted | SearchEvent::FocusChanged => {}
            },
        );

        // The frame rows open split where the sides already differ, set or inherited.
        let app_frame = settings::app_frame();
        PanelSettingsWindow {
            panel,
            page: 0,
            pickers,
            opacity_scrub: ScrubState::default(),
            value_edit: panel::ValueEdit::default(),
            margin_scrub: SidesScrub::default(),
            padding_scrub: SidesScrub::default(),
            rounding_scrub: ScrubState::default(),
            border_scrub: SidesScrub::default(),
            margin_split: split_knob(chrome.theme.margin.unwrap_or(app_frame.margin)),
            padding_split: split_knob(chrome.theme.padding.unwrap_or(app_frame.padding)),
            border_split: split_knob(chrome.theme.border_sides(app_frame.border)),
            font_scale_scrub: ScrubState::default(),
            slider_rounding_scrub: ScrubState::default(),
            shader_routes: RouteEditState::default(),
            shader_slots: (0..shader::SLOTS).map(|_| ScrubState::default()).collect(),
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
            nav_scroll: ScrollHandle::new(),
            search,
            _search_changes,
            state,
            backdrop: WindowBackdrop::default(),
            _picker_changes,
            _panel_changed,
            _backdrop_changed,
        }
    }

    fn set_panel_locked(&mut self, on: bool, cx: &mut Context<Self>) {
        if let Some(panel) = self.panel.upgrade() {
            panel.update(cx, |panel, cx| panel.set_locked(on, cx));
        }
    }

    fn set_panel_anchor(&mut self, on: bool, cx: &mut Context<Self>) {
        if let Some(panel) = self.panel.upgrade() {
            panel.update(cx, |panel, cx| panel.set_anchor(on, cx));
        }
    }

    /// The toggle reads as "show", so it stores the inverse.
    fn set_panel_controls(&mut self, shown: bool, cx: &mut Context<Self>) {
        if let Some(panel) = self.panel.upgrade() {
            panel.update(cx, |panel, cx| panel.set_hide_controls(!shown, cx));
        }
    }

    // The size limits, in px on the panel's chrome. Non-digits are stripped,
    // and empty or zero clears the limit.

    fn size_limit_edited(
        &mut self,
        input: &Entity<InputState>,
        apply: fn(&mut Self, Option<f32>, &mut Context<Self>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let raw = input.read(cx).value().to_string();
        let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
        // Rewrite only when it held non-digits; the follow-up Change sees clean
        // digits and stops.
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

    /// The panel notifies, which repaints it and this window both.
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

    /// Clearing the hex field reads the same as the cell's reset button.
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

    /// Drop one role's override, so the panel follows the app palette for it again.
    fn reset_role(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let role = &ROLES[index];
        self.update_theme(|theme| theme.set_color(role.name, None), cx);
        let inherited = (role.get)(&palette::resolved());
        self.pickers[index].update(cx, |picker, cx| picker.set_value(inherited, window, cx));
    }

    /// Re-seed the swatches showing an inherited or linked color when the
    /// palette they were seeded from moves; literal overrides hold. Runs from
    /// the appearance page's render, inside the window tint, since every
    /// palette change repaints all windows.
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

    /// Point one role at another app color. The reference tracks the live
    /// palette; the swatch takes the target's current resolve.
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

    /// Forking starts from the app's current value, so nothing visibly jumps
    /// until the slider moves.
    fn set_opacity_override(&mut self, on: bool, cx: &mut Context<Self>) {
        let value = on.then(palette::app_surface_opacity);
        self.update_theme(|theme| theme.surface_opacity = value, cx);
    }

    fn set_opacity(&mut self, value: f32, cx: &mut Context<Self>) {
        self.update_theme(|theme| theme.surface_opacity = Some(value), cx);
    }

    // The frame setters fork the knob as this panel's own override. A side of
    // None comes off the linked strip and sets all four. Zero is a real
    // override, not a clear; the reset button goes back to following the app.

    fn set_margin(&mut self, side: Option<Side>, value: f32, cx: &mut Context<Self>) {
        let app = settings::app_frame().margin;
        self.update_theme(
            move |theme| theme.margin = Some(theme.margin.unwrap_or(app).edited(side, value)),
            cx,
        );
    }

    fn set_padding(&mut self, side: Option<Side>, value: f32, cx: &mut Context<Self>) {
        let app = settings::app_frame().padding;
        self.update_theme(
            move |theme| theme.padding = Some(theme.padding.unwrap_or(app).edited(side, value)),
            cx,
        );
    }

    fn set_rounding(&mut self, value: f32, cx: &mut Context<Self>) {
        self.update_theme(|theme| theme.rounding = Some(value), cx);
    }

    /// Where an older config's edge mask stops being a mask: what it trimmed
    /// bakes into the widths written here.
    fn set_border(&mut self, side: Option<Side>, value: f32, cx: &mut Context<Self>) {
        let app = settings::app_frame().border;
        self.update_theme(
            move |theme| {
                theme.border = Some(theme.border_sides(app).edited(side, value));
                theme.legacy_border_edges = None;
            },
            cx,
        );
    }

    // Splitting only opens the sides up. Linking flattens them onto the widest,
    // forking the knob if the panel was following a split app default.

    fn split_margin(&mut self, split: bool, cx: &mut Context<Self>) {
        self.margin_split = split;
        let app = settings::app_frame().margin;
        if !split {
            self.update_theme(move |theme| link_knob(&mut theme.margin, app), cx);
        }
        cx.notify();
    }

    fn split_padding(&mut self, split: bool, cx: &mut Context<Self>) {
        self.padding_split = split;
        let app = settings::app_frame().padding;
        if !split {
            self.update_theme(move |theme| link_knob(&mut theme.padding, app), cx);
        }
        cx.notify();
    }

    fn split_border(&mut self, split: bool, cx: &mut Context<Self>) {
        self.border_split = split;
        let app = settings::app_frame().border;
        if !split {
            self.update_theme(
                move |theme| {
                    let shown = theme.border_sides(app);
                    if shown.uniform().is_none() {
                        theme.border = Some(shown.linked());
                    }
                    theme.legacy_border_edges = None;
                },
                cx,
            );
        }
        cx.notify();
    }

    // The per-knob resets drop just this knob's override, back to the app frame.

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
        self.update_theme(
            |theme| {
                theme.border = None;
                theme.legacy_border_edges = None;
            },
            cx,
        );
    }

    /// The strip's percent back into the multiplier the theme stores.
    fn set_font_scale(&mut self, percent: f32, cx: &mut Context<Self>) {
        let scale = percent / 100.0;
        self.update_theme(|theme| theme.font_scale = Some(scale), cx);
    }

    fn reset_font_scale(&mut self, cx: &mut Context<Self>) {
        self.update_theme(|theme| theme.font_scale = None, cx);
    }

    // Zero rounding and a hidden knob are real overrides; the resets go back
    // to the workspace's slider look.

    fn set_slider_rounding(&mut self, value: f32, cx: &mut Context<Self>) {
        self.update_theme(|theme| theme.slider_rounding = Some(value), cx);
    }

    fn reset_slider_rounding(&mut self, cx: &mut Context<Self>) {
        self.update_theme(|theme| theme.slider_rounding = None, cx);
    }

    fn set_slider_knob(&mut self, on: bool, cx: &mut Context<Self>) {
        self.update_theme(|theme| theme.slider_knob = Some(on), cx);
    }

    fn reset_slider_knob(&mut self, cx: &mut Context<Self>) {
        self.update_theme(|theme| theme.slider_knob = None, cx);
    }

    /// Unset, the toggle shows the workspace's knob. The reset joins on the
    /// left, matching the frame sliders.
    fn slider_knob_row(&self, value: Option<bool>, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .when(value.is_some(), |row| {
                row.child(settings_ui::icon_button(
                    icons::REFRESH_CW,
                    false,
                    cx.listener(|this, _, _, cx| this.reset_slider_knob(cx)),
                ))
            })
            .child(panel::toggle(
                value.unwrap_or(tokens::app_slider_look().knob),
                Self::set_slider_knob,
                cx,
            ))
    }

    /// Unset, the slider rests at 100%. The reset joins on the left, so the
    /// slider and readout hold still when it appears.
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

    /// Unset, the slider rests at the inherited app default. The reset joins on
    /// the left, so the slider and readout hold still when it appears.
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
        // The strip's top is the everyday range, not the law: a typed value runs
        // past it and the setters take it.
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

    /// `shown` is what the row draws: the panel's own knob or the app default under it.
    #[allow(clippy::too_many_arguments)]
    fn frame_sides(
        &self,
        scrub: &SidesScrub,
        shown: Sides,
        overridden: bool,
        split: bool,
        max: f32,
        on_split: fn(&mut Self, bool, &mut Context<Self>),
        apply: fn(&mut Self, Option<Side>, f32, &mut Context<Self>),
        reset: fn(&mut Self, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) -> Div {
        let control = settings_ui::sides_control(
            scrub,
            &self.value_edit,
            shown,
            split,
            settings_ui::span(0., max, " px"),
            on_split,
            apply,
            cx,
        );
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .when(overridden, |row| {
                row.child(settings_ui::icon_button(
                    icons::REFRESH_CW,
                    false,
                    cx.listener(move |this, _, _, cx| reset(this, cx)),
                ))
            })
            .child(control)
    }

    /// Drop every color override. The frame and opacity keep their own resets,
    /// so recoloring can start over without flattening the geometry.
    fn reset_colors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.update_theme(|theme| theme.colors.clear(), cx);
        for (role, picker) in ROLES.iter().zip(&self.pickers) {
            let inherited = (role.get)(&palette::resolved());
            picker.update(cx, |picker, cx| picker.set_value(inherited, window, cx));
        }
    }

    /// The app's resolved palette with the panel's overrides laid over it, so
    /// Inverse starts from what's on screen.
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

    /// Pin a whole palette onto the panel as overrides, so it holds under song
    /// theming and app edits. Shared by Inverse and Apply Song Theme.
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

    /// Flip the colors light for dark, accents held, frozen on as overrides.
    fn inverse_colors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let inverted = self.effective_palette(cx).inverse();
        self.override_all(inverted, window, cx);
    }

    /// Freeze the song theme onto the panel, so it holds after song theming
    /// turns off or moves on. Only offered while song theming drives the colors.
    fn apply_song_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let themed = palette::resolved();
        self.override_all(themed, window, cx);
    }

    fn reset_frame(&mut self, cx: &mut Context<Self>) {
        self.update_theme(
            |theme| {
                theme.margin = None;
                theme.padding = None;
                theme.rounding = None;
                theme.border = None;
                theme.legacy_border_edges = None;
            },
            cx,
        );
    }

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

    /// Second in the nav on every panel, so how a panel acts is always in the
    /// same spot.
    #[allow(clippy::too_many_arguments)]
    fn behavior_page(
        &mut self,
        q: &Query,
        locked: bool,
        anchor: bool,
        hide_controls: bool,
        composite: bool,
        limits: SizeLimits,
        extra: Vec<Section>,
        cx: &mut Context<Self>,
    ) -> PageBody {
        let page = PageBody::new()
            .section(Section::new(
                q,
                icons::MOVE,
                rox_i18n::t!("panel-section-placement"),
                None,
                |rows| {
                    rows.keyed(
                        "panel-locked",
                        &["lock", "pin", "fixed", "drag", "dock"],
                        panel::toggle(locked, Self::set_panel_locked, cx),
                    )
                    .keyed(
                        "panel-drag-anchor",
                        &["drag", "move", "window", "decorations", "titlebar"],
                        panel::toggle(anchor, Self::set_panel_anchor, cx),
                    )
                    .when(composite, |rows| {
                        rows.keyed(
                            "panel-slot-controls",
                            &["slot", "corner", "buttons", "swap", "remove"],
                            panel::toggle(!hide_controls, Self::set_panel_controls, cx),
                        )
                    })
                },
            ))
            // Only the axis the panel resizes along takes effect, but a panel can
            // sit in a row or a column, so both are offered.
            .section(Section::new(
                q,
                icons::MAXIMIZE,
                rox_i18n::t!("panel-section-size"),
                None,
                |rows| {
                    rows.keyed(
                        "panel-min-width",
                        &["size", "limit", "narrow", "floor"],
                        self.size_limit_row(
                            &self.min_width_input,
                            limits.min_width.is_some(),
                            Self::reset_min_width,
                            cx,
                        ),
                    )
                    .keyed(
                        "panel-max-width",
                        &["size", "limit", "wide", "cap"],
                        self.size_limit_row(
                            &self.max_width_input,
                            limits.max_width.is_some(),
                            Self::reset_max_width,
                            cx,
                        ),
                    )
                    .keyed(
                        "panel-min-height",
                        &["size", "limit", "short", "floor"],
                        self.size_limit_row(
                            &self.min_height_input,
                            limits.min_height.is_some(),
                            Self::reset_min_height,
                            cx,
                        ),
                    )
                    .keyed(
                        "panel-max-height",
                        &["size", "limit", "tall", "cap"],
                        self.size_limit_row(
                            &self.max_height_input,
                            limits.max_height.is_some(),
                            Self::reset_max_height,
                            cx,
                        ),
                    )
                },
            ));

        extra.into_iter().fold(page, PageBody::section)
    }

    /// No countdown confirm here, unlike the app-wide screen shader. That one
    /// guards against a shader burying the control that would undo it; a panel
    /// shader leaves this window, the menus and every other panel alone.
    fn shader_page(&mut self, q: &Query, window: &mut Window, cx: &mut Context<Self>) -> PageBody {
        let Some(panel) = self.panel.upgrade() else {
            return PageBody::new();
        };
        // Fold a hot edit picked up while this window was closed back in before
        // anything reads the config, so the page and a layout dump both hold the
        // text on screen.
        self.absorb_hot_source(cx);
        let configured = panel.read(cx).chrome().shader.clone();
        // A panel never given a shader reads as off, whatever a fresh config defaults to.
        let enabled = configured.as_ref().is_some_and(|shader| shader.enabled);
        let shader = configured.unwrap_or_default();
        // For a named panel, what runs is the pool's copy. The gate and the
        // approval block both read this, so a bundled shader can't slip past them
        // under a name with an empty source behind it.
        let resolved = shader::resolve_source(shader.name.as_deref(), &shader.source);
        let running = resolved.clone().unwrap_or_default();
        let pending = (!running.trim().is_empty() && !shader::approved(&running)).then(|| {
            let approving = running.clone();
            pending_shader(
                "panel-shader-pending",
                &running,
                shader.path.as_deref(),
                cx.listener(move |this, _, _, cx| {
                    shader::approve(&approving);
                    // The path named a file on the machine that wrote the bundle. A file
                    // there on this one would get pulled over the approved text, so an
                    // imported shader keeps no bookmark. Approving also flips a switch an
                    // earlier Turn Off left down.
                    this.edit_shader(
                        |shader| {
                            shader.path = None;
                            shader.enabled = true;
                        },
                        cx,
                    );
                }),
                cx.listener(move |this, _, _, cx| {
                    this.edit_shader(move |shader| shader.enabled = false, cx)
                }),
            )
        });
        // A compile message from before the switch went off is just noise.
        let error = (enabled && shader.runnable())
            .then(|| shader::error(panel.entity_id()))
            .flatten();
        // Slot names come off what runs, so a named panel reads the pool's
        // `// @slot n:` comments.
        let labels = shader::slot_labels(&running);
        self.shader_routes.sync(shader.routes.len());

        // Read before the field is borrowed for the picker below.
        let fallback = {
            let label = panel
                .read(cx)
                .custom_title()
                .map(str::to_string)
                .unwrap_or_else(|| panel::display_name(panel.read(cx).panel_name()));
            shader::eject_name(&label, &shader.source)
        };
        let picked = ShaderSource {
            id: "panel-shader",
            name: shader.name.as_deref(),
            path: shader.path.as_deref(),
            resolved: resolved.as_deref(),
            // A panel may have no surface shader at all, unlike the Shader panel.
            clear: Some(|this: &mut Self, cx| this.clear_shader(cx)),
            // A scene here would hide the panel's body rather than decorate it. The
            // Shader panel is where a full-cover look belongs.
            overlays_only: true,
            use_example: |this: &mut Self, index, cx| this.use_shader_example(index, cx),
            use_named: |this: &mut Self, name, cx| this.use_pool_shader(name, cx),
            choose_file: |this: &mut Self, window, cx| this.pick_shader_file(window, cx),
            edit: |this: &mut Self, window, cx| this.edit_shader_in_app(window, cx),
            eject: |this: &mut Self, cx| this.eject_shader(cx),
            detach: |this: &mut Self, cx| this.detach_shader(cx),
            reload: |this: &mut Self, cx| this.reload_shader(cx),
            save: |this: &mut Self, name, cx| this.save_shader_to_pool(name, cx),
            field: &mut self.shader_name,
            fallback: &fallback,
        }
        .render(window, cx);
        // The filter only decides what can be picked, so a config from a bundle or
        // an older build can still be running a scene. Say so.
        let scene = enabled && !running.trim().is_empty() && !shader::overlay(&running);
        let note = |text: SharedString| {
            div()
                .text_xs()
                .text_color(palette::text_muted())
                .child(text)
                .into_any_element()
        };

        // The write goes back through `edit_shader`, so the panel's config stays
        // the only copy.
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

        // The routed slots read out live; the rest are hand-set knobs.
        let slots = signal_ui::slots::SlotList {
            hub: &hub,
            routes: &shader.routes,
            manual: &shader.manual,
            labels: &labels,
            value_edit: &self.value_edit,
            scrubs: &self.shader_slots,
            set: Arc::new(|this: &mut Self, slot, value, cx| {
                this.set_shader_manual(slot, value, cx)
            }),
        }
        .render(cx);

        let mut page = PageBody::new();
        if let Some(body) = pending {
            page = page.section(Section::new(
                q,
                icons::ALERT,
                rox_i18n::t!("panel-awaiting-approval"),
                None,
                |rows| {
                    rows.custom(
                        &["shader", "approve", "approval", "pending", "wgsl"],
                        || body.into_any_element(),
                    )
                },
            ));
        }

        page.section(Section::new(
            q,
            icons::BLEND,
            rox_i18n::t!("panel-section-shader"),
            None,
            |rows| {
                rows.keyed(
                    "panel-surface-shader",
                    &["shader", "wgsl", "effect", "overlay", "surface"],
                    panel::toggle(
                        enabled,
                        |this: &mut Self, on, cx| {
                            this.edit_shader(move |shader| shader.enabled = on, cx)
                        },
                        cx,
                    ),
                )
                .custom(
                    &[
                        "shader",
                        "wgsl",
                        "file",
                        "reload",
                        "source",
                        "example",
                        "preset",
                        "workspace",
                    ],
                    || picked.into_any_element(),
                )
                .when(scene, |rows| {
                    rows.custom(&["shader", "scene", "covers", "overlay"], || {
                        note(rox_i18n::t!("panel-shader-is-scene"))
                    })
                })
                .when_some(error, |rows, error| {
                    rows.custom(&["shader", "error", "compile"], || note(error.into()))
                })
                .keyed(
                    "panel-run-when-idle",
                    &["shader", "idle", "pause", "freeze", "silent"],
                    panel::toggle(
                        shader.run_when_idle,
                        |this: &mut Self, on, cx| {
                            this.edit_shader(move |shader| shader.run_when_idle = on, cx)
                        },
                        cx,
                    ),
                )
            },
        ))
        .section(Section::new(
            q,
            icons::ACTIVITY,
            rox_i18n::t!("panel-section-signals"),
            Some(add.into_any_element()),
            |rows| {
                rows.custom(&["shader", "signal", "route", "bind", "modulation"], || {
                    editor.list(cx).into_any_element()
                })
            },
        ))
        .section(Section::new(
            q,
            icons::GAUGE,
            rox_i18n::t!("panel-section-slots"),
            None,
            |rows| {
                rows.custom(&["shader", "slot", "knob", "manual"], || {
                    slots.into_any_element()
                })
            },
        ))
    }

    /// Write a hot-reloaded source back into the config, so the layout dump
    /// holds what runs. The wrapper only has the panel's id and can't write
    /// back, so the fold happens here where the panel is typed.
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

    /// Edit the panel's shader config, seeding a default on first touch. The
    /// stored compile message goes too, since the source just moved.
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
        // The compile message is written where the shader paints, in the panel's
        // window after this one, and a broken shader requests no frames. One nudge
        // once the draw has finished.
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
    }

    /// A named shader edits the pool entry, so every panel on the name follows
    /// an apply. An inline one edits this panel's text through the same config
    /// write the picker's actions take.
    fn edit_shader_in_app(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        use shader::edit::{EditKey, ShaderEditTarget};

        let Some(panel) = self.panel.upgrade() else {
            return;
        };
        let shader = panel.read(cx).chrome().shader.clone().unwrap_or_default();
        let target = match shader.name.as_deref() {
            Some(name) => ShaderEditTarget::pool(name),
            None => {
                let title = panel
                    .read(cx)
                    .custom_title()
                    .map(str::to_string)
                    .unwrap_or_else(|| panel::display_name(panel.read(cx).panel_name()));
                let weak = panel.downgrade();
                Some(ShaderEditTarget {
                    key: EditKey::Panel(panel.entity_id()),
                    title: title.into(),
                    source: shader.source.clone(),
                    ctx: shader::ProgramCtx::of(None, shader.path.as_deref()),
                    path: shader.path.clone(),
                    write: Arc::new(move |source, cx| {
                        let Some(panel) = weak.upgrade() else {
                            return;
                        };
                        // The old compile message was about text that just left.
                        shader::note_error(panel.entity_id(), None);
                        panel.update(cx, |panel, cx| {
                            let shader = panel
                                .chrome_mut()
                                .shader
                                .get_or_insert_with(shader::PanelShader::default);
                            shader.source = source;
                            // Applying an edit is asking to see it.
                            shader.enabled = true;
                            cx.notify();
                        });
                    }),
                })
            }
        };
        let Some(target) = target else {
            return;
        };
        let state = panel.read(cx).state();
        crate::openers::shader_editor(state, target, cx);
    }

    /// Not through [`edit_shader`](Self::edit_shader): that clears the compile
    /// message, and a knob drag moves no source, so a broken shader would go
    /// quiet mid-drag with nothing to bring the message back.
    fn set_shader_manual(&mut self, slot: usize, value: f32, cx: &mut Context<Self>) {
        let Some(panel) = self.panel.upgrade() else {
            return;
        };
        panel.update(cx, |panel, cx| {
            let shader = panel
                .chrome_mut()
                .shader
                .get_or_insert_with(shader::PanelShader::default);
            shader::set_manual_value(&mut shader.manual, slot, value);
            cx.notify();
        });
        cx.notify();
    }

    /// The source is copied into the config on the way in, so the path is only
    /// a bookmark for Reload.
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

    /// The panel watches the file while it draws, so this is for a parked panel
    /// or an edit that shouldn't have to wait.
    fn reload_shader(&mut self, cx: &mut Context<Self>) {
        let path = self
            .panel
            .upgrade()
            .and_then(|panel| panel.read(cx).chrome().shader.as_ref()?.path.clone());
        if let Some(path) = path {
            self.load_shader_file(path, cx);
        }
    }

    /// Write the shader out and hand it to whatever opens `.wgsl`, the
    /// external-editor authoring loop. An inline shader keeps the bookmark. A
    /// named one ejects through its pool entry, since its edits belong to every
    /// panel running it.
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
                shader::note_error(
                    panel.entity_id(),
                    Some(
                        rox_i18n::t!("shader-eject-failed", error = error.to_string()).to_string(),
                    ),
                );
                cx.notify();
            }
        }
    }

    /// Copy the running pool shader in and stop using it by name. It's the same
    /// text, so its approval still applies. No bookmark comes across: a second
    /// watcher on the pool's file would drift the two apart on the next save.
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

    /// The inline source and bookmark go, since the workspace holds what runs
    /// from here. Nothing is approved on the way through: a workspace shader
    /// that came in with a bundle still has to be read before it runs.
    fn use_pool_shader(&mut self, name: String, cx: &mut Context<Self>) {
        self.edit_shader(
            move |shader| {
                shader.name = Some(name);
                shader.source = String::new();
                shader.path = None;
                // Picking is asking to see it, or a panel an earlier Turn Off parked would
                // go on painting nothing.
                shader.enabled = true;
            },
            cx,
        );
    }

    /// The bookmark goes: an example has no file behind it, and a stale one
    /// would have the watch overwrite it a moment later.
    fn use_shader_example(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(preset) = shader::PRESETS.get(index) else {
            return;
        };
        let source = preset.source.to_string();
        self.edit_shader(
            move |shader| {
                shader.source = source;
                shader.name = None;
                shader.path = None;
                shader.enabled = true;
            },
            cx,
        );
    }

    /// Take the shader off this panel. The switch is its own decision and
    /// stays, and a workspace shader stays in the workspace.
    fn clear_shader(&mut self, cx: &mut Context<Self>) {
        self.edit_shader(
            |shader| {
                shader.name = None;
                shader.source = String::new();
                shader.path = None;
            },
            cx,
        );
    }

    /// Promote the inline shader into the workspace's shaders and use it by
    /// name. The inline copy goes, so the pool holds the only one.
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
        // The bookmark goes with it, so a file being edited keeps hot reloading
        // through the pool's watch.
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

    /// Snapshot a file into the panel's shader source; a read failure shows
    /// where a compile error would. Picking a file approves it, since the gate
    /// is for sources nobody chose. The name goes, so the panel holds its own
    /// source from here.
    fn load_shader_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        match std::fs::read_to_string(&path) {
            Ok(source) => self.edit_shader(
                move |shader| {
                    crate::panel::shader::approve(&source);
                    shader.source = source;
                    shader.name = None;
                    shader.path = Some(path);
                    shader.enabled = true;
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

    /// The opacity fork, the frame knobs, the panel's own appearance section,
    /// and the override grid, with inherit as the resting state.
    #[allow(clippy::too_many_arguments)]
    fn appearance_page(
        &mut self,
        q: &Query,
        theme: &PanelTheme,
        extra: Vec<Section>,
        own_font: bool,
        has_slider: bool,
        columns: usize,
        cx: &mut Context<Self>,
    ) -> PageBody {
        let opacity = Section::new(
            q,
            icons::EYE,
            rox_i18n::t!("panel-section-opacity"),
            None,
            |rows| {
                rows.keyed(
                    "panel-own-opacity",
                    &["transparency", "translucent", "backdrop", "override"],
                    panel::toggle(
                        theme.surface_opacity.is_some(),
                        Self::set_opacity_override,
                        cx,
                    ),
                )
                .when_some(theme.surface_opacity, |rows, value| {
                    rows.keyed(
                        "panel-surface-opacity",
                        &["transparency", "translucent", "backdrop"],
                        settings_ui::slider_edit(
                            &self.opacity_scrub,
                            &self.value_edit,
                            value,
                            Self::set_opacity,
                            cx,
                        ),
                    )
                })
            },
        );

        let app = settings::app_frame();
        let frame_controls = small_button(
            rox_i18n::t!("panel-reset"),
            icons::REFRESH_CW,
            false,
            cx.listener(|this, _, _, cx| this.reset_frame(cx)),
        );
        let frame = Section::new(
            q,
            icons::SQUARE_DASHED,
            rox_i18n::t!("panel-section-frame"),
            Some(frame_controls.into_any_element()),
            |rows| {
                rows.keyed(
                    "panel-margin",
                    &["spacing", "gap", "outside", "sides"],
                    self.frame_sides(
                        &self.margin_scrub,
                        theme.margin.unwrap_or(app.margin),
                        theme.margin.is_some(),
                        self.margin_split,
                        MARGIN_MAX,
                        Self::split_margin,
                        Self::set_margin,
                        Self::reset_margin,
                        cx,
                    ),
                )
                .keyed(
                    "panel-padding",
                    &["spacing", "inset", "inside", "sides"],
                    self.frame_sides(
                        &self.padding_scrub,
                        theme.padding.unwrap_or(app.padding),
                        theme.padding.is_some(),
                        self.padding_split,
                        PADDING_MAX,
                        Self::split_padding,
                        Self::set_padding,
                        Self::reset_padding,
                        cx,
                    ),
                )
                .keyed(
                    "panel-rounding",
                    &["corner radius", "rounded"],
                    self.frame_slider(
                        &self.rounding_scrub,
                        theme.rounding,
                        app.rounding,
                        ROUNDING_MAX,
                        Self::set_rounding,
                        Self::reset_rounding,
                        cx,
                    ),
                )
                .keyed(
                    "panel-border",
                    &["outline", "stroke", "edge", "sides"],
                    self.frame_sides(
                        &self.border_scrub,
                        theme.border_sides(app.border),
                        theme.border.is_some() || theme.legacy_border_edges.is_some(),
                        self.border_split,
                        BORDER_MAX,
                        Self::split_border,
                        Self::set_border,
                        Self::reset_border,
                        cx,
                    ),
                )
            },
        );

        let font = (!own_font).then(|| {
            // The section Reset drops both font overrides; the size row has its own too.
            let reset = small_button(
                rox_i18n::t!("panel-reset"),
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
            Section::new(
                q,
                icons::ALIGN_LEFT,
                rox_i18n::t!("panel-section-font"),
                Some(reset.into_any_element()),
                |rows| {
                    rows.keyed(
                        "panel-font",
                        &["typeface", "family", "text"],
                        panel::font_picker(
                            "panel-font",
                            theme.font.clone(),
                            |this: &mut Self, font, cx| {
                                this.update_theme(|theme| theme.font = font, cx)
                            },
                            cx,
                        ),
                    )
                    .keyed(
                        "panel-font-size",
                        &["text size", "scale", "zoom"],
                        self.font_scale_row(theme.font_scale, cx),
                    )
                },
            )
        });

        let slider = has_slider.then(|| {
            let reset = small_button(
                rox_i18n::t!("panel-reset"),
                icons::REFRESH_CW,
                theme.slider_rounding.is_none() && theme.slider_knob.is_none(),
                cx.listener(|this, _, _, cx| {
                    this.update_theme(
                        |theme| {
                            theme.slider_rounding = None;
                            theme.slider_knob = None;
                        },
                        cx,
                    )
                }),
            );
            Section::new(
                q,
                icons::SLIDERS,
                rox_i18n::t!("panel-section-slider"),
                Some(reset.into_any_element()),
                |rows| {
                    rows.keyed(
                        "panel-slider-rounding",
                        &["corner radius", "rounded", "volume", "square"],
                        self.frame_slider(
                            &self.slider_rounding_scrub,
                            theme.slider_rounding,
                            tokens::app_slider_look().rounding,
                            tokens::SLIDER_ROUNDING_MAX,
                            Self::set_slider_rounding,
                            Self::reset_slider_rounding,
                            cx,
                        ),
                    )
                    .keyed(
                        "panel-slider-knob",
                        &["handle", "thumb", "volume", "drag"],
                        self.slider_knob_row(theme.slider_knob, cx),
                    )
                },
            )
        });

        let colors = self.colors_section(q, theme, columns, cx);

        let mut page = PageBody::new().section(opacity).section(frame);
        for section in [font, slider].into_iter().flatten().chain(extra) {
            page = page.section(section);
        }
        page.section(colors)
    }

    /// The override grid. Built only when kept: it's a picker per role.
    fn colors_section(
        &self,
        q: &Query,
        theme: &PanelTheme,
        columns: usize,
        cx: &mut Context<Self>,
    ) -> Section {
        let song_on = palette::art_theming();
        let color_controls = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .child(small_button(
                rox_i18n::t!("panel-inverse"),
                icons::CONTRAST,
                false,
                cx.listener(|this, _, window, cx| this.inverse_colors(window, cx)),
            ))
            .child(small_button(
                rox_i18n::t!("panel-apply-song-theme"),
                icons::DISC,
                !song_on,
                cx.listener(|this, _, window, cx| this.apply_song_theme(window, cx)),
            ))
            .child(small_button(
                rox_i18n::t!("panel-reset"),
                icons::REFRESH_CW,
                false,
                cx.listener(|this, _, window, cx| this.reset_colors(window, cx)),
            ));

        Section::new(
            q,
            icons::PALETTE,
            rox_i18n::t!("panel-section-colors"),
            Some(color_controls.into_any_element()),
            |rows| {
                rows.custom(
                    &[
                        "palette", "accent", "swatch", "role", "override", "link", "inverse",
                    ],
                    || {
                        let overridden = |name: &str| theme.colors.contains_key(name);
                        let weak = cx.entity().downgrade();
                        let grid = settings_ui::role_grid(columns, |j| {
                            let role = &ROLES[j];
                            // The picker pads 4px around its swatch; the counter-margin keeps
                            // the cell at the grid's 20px footprint.
                            let control = ColorPicker::new(&self.pickers[j])
                                .small()
                                .m(px(-4.))
                                .into_any_element();
                            // Linked cells fill the button with the accent, so a reference
                            // reads apart from a literal fork at a glance.
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
                                                            this.set_role_reference(
                                                                j,
                                                                target.name,
                                                                window,
                                                                cx,
                                                            )
                                                        });
                                                    }
                                                }),
                                        );
                                    }
                                    menu
                                })
                                .into_any_element();
                            let reset = overridden(role.name).then(|| {
                                settings_ui::icon_button(
                                    icons::REFRESH_CW,
                                    false,
                                    cx.listener(move |this, _, window, cx| {
                                        this.reset_role(j, window, cx)
                                    }),
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
                            settings_ui::color_cell(
                                control,
                                role.label,
                                overridden(role.name),
                                Some(trailing),
                            )
                            .into_any_element()
                        });
                        div()
                            .flex()
                            .flex_col()
                            .gap(tokens::SPACE_XS)
                            .child(div().text_xs().text_color(palette::text_muted()).child(
                                "Overrides recolor just this panel and hold still under song \
                                 theming; a link follows another app color instead, moving \
                                 with the theme. Reset a swatch or clear its hex field to \
                                 follow the app palette again",
                            ))
                            .child(grid)
                            .into_any_element()
                    },
                )
            },
        )
    }
}

/// Whether any of these tab groups holds the panel. A popped-out panel or a
/// composite's slot has no tab slot for a preset to rebuild into.
fn hosted_in(groups: &[Vec<EntityId>], id: EntityId) -> bool {
    groups.iter().any(|group| group.contains(&id))
}

/// The picker's runs for one panel kind: built-in, global and this workspace
/// first, then one per other workspace in the order the sources name them.
/// Another kind's entries drop out, and so do empty runs.
fn preset_groups(entries: Vec<PresetEntry>, kind: &str) -> Vec<(PresetScope, Vec<PanelPreset>)> {
    let mut groups: Vec<(PresetScope, Vec<PanelPreset>)> = [
        PresetScope::Builtin,
        PresetScope::Global,
        PresetScope::Workspace,
    ]
    .into_iter()
    .map(|scope| (scope, Vec::new()))
    .collect();

    for entry in entries {
        if entry.preset.panel_name() != Some(kind) {
            continue;
        }
        match groups.iter_mut().find(|(scope, _)| *scope == entry.scope) {
            Some((_, presets)) => presets.push(entry.preset),
            None => groups.push((entry.scope, vec![entry.preset])),
        }
    }

    groups.retain(|(_, presets)| !presets.is_empty());
    groups
}

/// Built-in names are i18n keys; every other tier's name is the user's own
/// text. The name is resolved here, so the applied mark reads as a label.
fn preset_item<P: PanelSettings>(
    mut preset: PanelPreset,
    scope: &PresetScope,
    panel: WeakEntity<P>,
    state: AppState,
    me: WeakEntity<PanelSettingsWindow<P>>,
) -> PopupMenuItem {
    if *scope == PresetScope::Builtin {
        preset.name = rox_i18n::t!(&preset.name).to_string();
    }
    let label: SharedString = preset.name.clone().into();
    PopupMenuItem::new(label).on_click(move |_, window, cx| {
        let Some(panel) = panel.upgrade() else {
            return;
        };
        crate::openers::apply_panel_preset(
            Arc::new(panel),
            preset.clone(),
            PresetApply::Pick,
            state.clone(),
            follow_rebuild(me.clone(), window),
            cx,
        );
    })
}

/// The apply's callback: point this settings window at the rebuilt panel
/// instead of closing and reopening it, which flashed.
fn follow_rebuild<P: PanelSettings>(
    me: WeakEntity<PanelSettingsWindow<P>>,
    window: &Window,
) -> Rebuilt {
    let handle = window.window_handle();
    Box::new(move |rebuilt, cx| {
        let _ = handle.update(cx, |_, window, cx| {
            let _ = me.update(cx, |this, cx| this.retarget(rebuilt, window, cx));
        });
    })
}

impl<P: PanelSettings> PanelSettingsWindow<P> {
    /// A search ends on a page pick, the way the app settings window's does.
    fn open_page(&mut self, page: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.page = page;
        if !self.search.read(cx).query().is_empty() {
            self.search
                .update(cx, |search, cx| search.set_value("", window, cx));
        }
        cx.notify();
    }

    /// 0 is Appearance, 1 Behavior, 2 Shader, and the panel's own pages follow.
    fn build_page(
        &mut self,
        panel: &Entity<P>,
        page: usize,
        q: &Query,
        columns: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PageBody {
        match page {
            0 => {
                let theme = panel.read(cx).theme();
                self.sync_swatches(&theme, window, cx);
                let own_font = panel.read(cx).has_own_font();
                let has_slider = panel.read(cx).has_slider();
                let extra = panel.update(cx, |panel, cx| panel.appearance_section(q, window, cx));
                self.appearance_page(q, &theme, extra, own_font, has_slider, columns, cx)
            }

            1 => {
                // Read through chrome(), since PanelSettings::locked and the dock's
                // Panel::locked share the name.
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
                let extra = panel.update(cx, |panel, cx| panel.behavior_section(q, window, cx));
                self.behavior_page(
                    q,
                    locked,
                    anchor,
                    hide_controls,
                    composite,
                    limits,
                    extra,
                    cx,
                )
            }

            2 => self.shader_page(q, window, cx),

            _ => {
                let pages = panel.read(cx).pages();
                let Some(&(name, _)) = pages.get(page - 3) else {
                    return PageBody::new();
                };
                panel.update(cx, |panel, cx| panel.page_body(name, q, window, cx))
            }
        }
    }

    /// Every page with hits under its own label, which opens the page.
    fn search_results(
        text: &str,
        entries: &[(usize, SharedString, &'static str)],
        pages: Vec<PageBody>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if pages.iter().all(|body| body.hits() == 0) {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(palette::text_muted())
                .child(rox_i18n::t!("settings-search-no-matches", text = text))
                .into_any_element();
        }

        div()
            .flex()
            .flex_col()
            .gap(SECTION_GAP)
            .children(
                entries
                    .iter()
                    .zip(pages)
                    .filter(|(_, body)| body.hits() > 0)
                    .map(|((page, label, icon), body)| {
                        let page = *page;
                        let rule = || div().flex_1().h(px(1.)).bg(palette::border());
                        div()
                            .flex()
                            .flex_col()
                            .gap(tokens::SPACE_SM)
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(tokens::SPACE_SM)
                                    .cursor_pointer()
                                    .text_color(palette::text_muted())
                                    .hover(|d| d.text_color(palette::text()))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, window, cx| {
                                            this.open_page(page, window, cx);
                                        }),
                                    )
                                    .child(rule())
                                    .child(svg().path(*icon).size(px(14.)).flex_none())
                                    .child(label.clone())
                                    .child(rule()),
                            )
                            .child(body.element())
                    }),
            )
            .into_any_element()
    }

    /// The head of the nav: the preset picker, Save As Preset, and Undo
    /// Preset. None when the panel has no tab group.
    fn presets_head(&self, panel: &Entity<P>, me: WeakEntity<Self>, cx: &App) -> Option<Div> {
        let groups: Vec<Vec<EntityId>> = self
            .state
            .tab_hosts
            .read(cx)
            .groups()
            .iter()
            .map(|tabs| {
                tabs.read(cx)
                    .panels()
                    .iter()
                    .map(|hosted| hosted.view().entity_id())
                    .collect()
            })
            .collect();
        if !hosted_in(&groups, panel.entity_id()) {
            return None;
        }

        let id = panel.entity_id();
        let marks = cx.try_global::<PresetMarks>();
        // Named only while the panel still is that preset: an edit since drops
        // the label back to the plain word.
        let applied = marks
            .and_then(|marks| marks.applied(id))
            .filter(|applied| {
                serde_json::to_value(panel.read(cx).dump(cx)).ok().as_ref() == Some(&applied.panel)
            })
            .map(|applied| SharedString::from(applied.name.clone()));
        let can_undo = marks.is_some_and(|marks| marks.has_undo(id));

        // Sources are read when the menu opens, so a preset saved from another
        // window shows up without this one repainting.
        let (picking, state, me_pick) = (self.panel.clone(), self.state.clone(), me.clone());
        let picker = settings_ui::menu_button(
            "panel-presets",
            applied.unwrap_or_else(|| rox_i18n::t!("menu-panels-presets")),
            icons::COPY,
        )
        .w_full()
        .dropdown_menu(move |mut menu, window, cx| {
            let Some(panel) = picking.upgrade() else {
                return menu;
            };
            let kind = panel.read(cx).panel_name();
            let groups = preset_groups(crate::openers::preset_sources(kind), kind);
            if groups.is_empty() {
                return menu
                    .item(PopupMenuItem::new(rox_i18n::t!("preset-none-for-kind")).disabled(true));
            }

            for (scope, presets) in groups {
                let heading = match &scope {
                    PresetScope::Builtin => rox_i18n::t!("preset-group-builtin"),
                    PresetScope::Global => rox_i18n::t!("preset-group-global"),
                    PresetScope::Workspace => rox_i18n::t!("preset-group-workspace"),
                    PresetScope::Other(workspace) => {
                        let (picking, state, me) =
                            (picking.clone(), state.clone(), me_pick.clone());
                        menu = menu.submenu_with_icon(
                            Some(Icon::default().path(icons::LAYERS)),
                            rox_i18n::t!("preset-group-other", workspace = workspace.clone()),
                            window,
                            cx,
                            move |mut sub, _, _| {
                                for preset in &presets {
                                    sub = sub.item(preset_item(
                                        preset.clone(),
                                        &scope,
                                        picking.clone(),
                                        state.clone(),
                                        me.clone(),
                                    ));
                                }
                                sub
                            },
                        );
                        continue;
                    }
                };
                menu = menu.item(PopupMenuItem::label(heading));
                for preset in presets {
                    menu = menu.item(preset_item(
                        preset,
                        &scope,
                        picking.clone(),
                        state.clone(),
                        me_pick.clone(),
                    ));
                }
            }
            menu
        });

        let saving = self.panel.clone();
        let save = settings_ui::icon_button(icons::DOWNLOAD, false, move |_, _, cx| {
            if let Some(panel) = saving.upgrade() {
                open_save_preset(panel, cx);
            }
        })
        .keyed("preset-save");

        // Always drawn, so the row never shifts; inert until there's a step back.
        let (undoing, state, me_undo) = (self.panel.clone(), self.state.clone(), me);
        let undo = settings_ui::icon_button(icons::SEEK_BACK, !can_undo, move |_, window, cx| {
            let Some(panel) = undoing.upgrade() else {
                return;
            };
            let Some(previous) = cx
                .default_global::<PresetMarks>()
                .take_undo(panel.entity_id())
            else {
                return;
            };
            crate::openers::apply_panel_preset(
                Arc::new(panel),
                previous,
                PresetApply::Undo,
                state.clone(),
                follow_rebuild(me_undo.clone(), window),
                cx,
            );
        })
        .keyed("preset-undo");

        // The menu wraps the button in its own element, so the shrink rules go
        // on a box around it. Without that the wrapper sizes to the name and a
        // long one pushes the icons out past the sidebar's edge.
        let picker = div().flex_1().min_w_0().overflow_hidden().child(picker);

        Some(
            div()
                .flex_none()
                .w_full()
                .min_w_0()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_XS)
                .child(picker)
                .child(save)
                .child(undo),
        )
    }

    /// Follow a preset rebuild: the old entity is gone, so every handle and
    /// seeded control moves to the new one, and the open-windows table keeps
    /// finding this window under the new id.
    fn retarget(
        &mut self,
        rebuilt: Arc<dyn PanelView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Ok(panel) = rebuilt.view().downcast::<P>() else {
            return;
        };

        let was = self.panel.entity_id();
        let open = cx.default_global::<OpenPanelSettings>();
        if let Some(handle) = open.0.remove(&was) {
            open.0.insert(panel.entity_id(), handle);
        }

        self._panel_changed = Some(cx.observe(&panel, |_, _, cx| cx.notify()));
        self.panel = panel.downgrade();

        // Re-seed what the constructor seeds, off the rebuilt panel's chrome.
        let chrome = panel.read(cx).chrome().clone();
        let seed = |value: Option<f32>| value.map(|n| format!("{n:.0}")).unwrap_or_default();
        for (input, value) in [
            (&self.min_width_input, chrome.min_width),
            (&self.min_height_input, chrome.min_height),
            (&self.max_width_input, chrome.max_width),
            (&self.max_height_input, chrome.max_height),
        ] {
            input.update(cx, |state, cx| state.set_value(seed(value), window, cx));
        }
        let app_frame = settings::app_frame();
        self.margin_split = split_knob(chrome.theme.margin.unwrap_or(app_frame.margin));
        self.padding_split = split_knob(chrome.theme.padding.unwrap_or(app_frame.padding));
        self.border_split = split_knob(chrome.theme.border_sides(app_frame.border));

        let theme = panel.read(cx).theme();
        let resolve = palette::resolved();
        for (role, picker) in ROLES.iter().zip(&self.pickers) {
            let color = theme
                .color(role.name)
                .unwrap_or_else(|| (role.get)(&resolve));
            picker.update(cx, |picker, cx| picker.set_value(color, window, cx));
        }
        self.swatch_resolve = Some(resolve);

        cx.notify();
    }
}

impl<P: PanelSettings> Render for PanelSettingsWindow<P> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let columns = grid_columns(window);

        if let Some(panel) = self.panel.upgrade()
            && let Some(page) = cx
                .default_global::<RequestedPage>()
                .0
                .remove(&panel.entity_id())
        {
            self.page = page;
        }

        // Everything builds inside the tint: color reads resolve eagerly as the div
        // chains build, so a nav or page built before the wrap would use the
        // untinted base palette.
        let player = self.state.player.entity_id();
        palette::note_focus(player, window.is_window_active(), cx);
        let text = self.search.read(cx).query().trim().to_string();
        let q = Query::parse(&text);

        panel::window_body(player, || {
            let me = cx.entity().downgrade();
            let presets = self
                .panel
                .upgrade()
                .and_then(|panel| self.presets_head(&panel, me, cx));
            let search = self.search.update(cx, |search, cx| search.element(cx));
            let search = div()
                .pb(tokens::SPACE_SM)
                .border_b_1()
                .border_color(palette::border())
                // Only while the box holds focus, so an outside click never blurs
                // another input mid-edit.
                .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                    if this.search.read(cx).is_focused(window, cx) {
                        window.blur();
                    }
                }))
                .child(search);
            let (nav, body): (Div, AnyElement) =
                match self.panel.upgrade() {
                    None => (
                        sidebar(),
                        div()
                            .text_color(palette::text_muted())
                            .child(rox_i18n::t!("panel-was-closed"))
                            .into_any_element(),
                    ),
                    Some(panel) => {
                        let pages = panel.read(cx).pages();
                        // Appearance, Behavior and Shader lead the nav on every
                        // panel, the app settings window's order.
                        let surface_shader = panel.read(cx).surface_shader();
                        let mut picked = self.page.min(pages.len() + 2);
                        // A panel that opted out of the shared Shader page keeps the numbering, so
                        // its own pages stay at 3, and slot 2 falls back to Appearance.
                        if !surface_shader && picked == 2 {
                            picked = 0;
                        }

                        let mut entries: Vec<(usize, SharedString, &'static str)> = vec![
                            (0, rox_i18n::t!("panel-page-appearance"), icons::PALETTE),
                            (1, rox_i18n::t!("panel-page-behavior"), icons::SLIDERS),
                        ];
                        if surface_shader {
                            entries.push((2, rox_i18n::t!("panel-page-shader"), icons::BLEND));
                        }
                        entries.extend(
                            pages
                                .iter()
                                .enumerate()
                                .map(|(i, &(label, icon))| (i + 3, panel::page_label(label), icon)),
                        );

                        // Search builds every page, so the nav can dim the ones with no hits.
                        let results: Option<Vec<PageBody>> = q.active().then(|| {
                            entries
                                .iter()
                                .map(|&(page, ..)| {
                                    self.build_page(&panel, page, &q, columns, window, cx)
                                })
                                .collect()
                        });

                        let rows: Vec<AnyElement> = entries
                            .iter()
                            .enumerate()
                            .map(|(index, (page, label, icon))| {
                                let page = *page;
                                let empty = results
                                    .as_ref()
                                    .is_some_and(|results| results[index].hits() == 0);
                                settings_ui::nav_item(
                                    label.clone(),
                                    icon,
                                    picked == page,
                                    move |this: &mut Self, window, cx| {
                                        this.open_page(page, window, cx);
                                    },
                                    cx,
                                )
                                .when(empty, |d| d.opacity(0.4))
                                .into_any_element()
                            })
                            .collect();
                        // The nav scrolls, so a long page list doesn't hide its tail in a short window.
                        let nav = sidebar().children(presets).child(search).child(
                            settings_ui::nav_scroll(
                                "panel-settings-nav",
                                &self.nav_scroll,
                                move |col| col.children(rows),
                            ),
                        );

                        let body = match results {
                            Some(results) => Self::search_results(&text, &entries, results, cx),
                            None => self
                                .build_page(&panel, picked, &q, columns, window, cx)
                                .element(),
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
                // Without it, translucent surfaces sink into the window's own black
                // instead of the playing track's art.
                .children(self.backdrop.layer(&self.state.now_art, window, cx))
                .child(nav)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .relative()
                        // Opaque at full surface opacity, so the backdrop only reads through as
                        // the surfaces thin.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Read off the group table rather than the menu, which needs a window to build.
    #[test]
    fn the_window_is_only_offered_overlays() {
        let offered = |overlays_only| -> Vec<&'static str> {
            shader_groups(overlays_only)
                .iter()
                .flat_map(|&(overlay, _, _)| {
                    shader::PRESETS
                        .iter()
                        .filter(move |preset| shader::overlay(preset.source) == overlay)
                        .map(|preset| preset.label)
                })
                .collect()
        };
        let window = offered(true);
        assert!(
            window.contains(&"Sheen"),
            "an overlay is offered: {window:?}"
        );
        assert!(window.contains(&"Tube"));
        for scene in ["Plasma", "Trails", "Cover", "Bloom"] {
            assert!(
                !window.contains(&scene),
                "{scene} covers the window and mustn't be offered for it"
            );
        }
        // The Shader panel lists every example, each in exactly one run.
        let panel = offered(false);
        assert_eq!(panel.len(), shader::PRESETS.len());
        for preset in shader::PRESETS {
            assert!(
                panel.contains(&preset.label),
                "{} went missing",
                preset.label
            );
        }
    }

    #[test]
    fn the_filtered_list_drops_the_split() {
        assert_eq!(shader_groups(true).len(), 1);
        // Against the key rather than the English, so it holds in any locale.
        assert_eq!(
            shader_groups(true)[0].1,
            rox_i18n::t!("shader-group-examples")
        );
        assert_eq!(shader_groups(false).len(), 2);
    }

    #[test]
    fn preset_marks_undo_once_and_name_until_cleared() {
        let mut marks = PresetMarks::default();
        let id = EntityId::from(7u64);
        let preset = |name: &str| PanelPreset {
            name: name.into(),
            panel: serde_json::json!({ "panel_name": "lyrics" }),
        };
        assert!(!marks.has_undo(id));

        marks.note(id, Some(preset("before")), Some(preset("Karaoke")));
        assert!(marks.has_undo(id));
        assert_eq!(marks.applied(id).map(|p| p.name.as_str()), Some("Karaoke"));
        assert_eq!(
            marks.take_undo(id).map(|p| p.name),
            Some("before".to_string())
        );

        assert!(!marks.has_undo(id), "taking the step clears it");
        assert!(marks.take_undo(id).is_none());

        // An undo rebuild notes nothing, which also clears the name.
        marks.note(id, None, None);
        assert!(marks.applied(id).is_none());
    }

    #[test]
    fn presets_row_hides_without_a_tab_group() {
        let (ours, other) = (EntityId::from(7), EntityId::from(9));
        assert!(hosted_in(&[vec![other], vec![other, ours]], ours));
        assert!(!hosted_in(&[vec![other]], ours), "popped out or in a slot");
        assert!(!hosted_in(&[], ours));
    }

    #[test]
    fn preset_groups_run_in_picker_order_for_one_kind() {
        let entry = |scope: PresetScope, name: &str, kind: &str| PresetEntry {
            scope,
            preset: PanelPreset {
                name: name.into(),
                panel: serde_json::json!({ "panel_name": kind }),
            },
        };
        let groups = preset_groups(
            vec![
                entry(PresetScope::Other("Night".into()), "Dim", "lyrics"),
                entry(PresetScope::Workspace, "Mine", "lyrics"),
                entry(PresetScope::Global, "Scope", "spectrum"),
                entry(PresetScope::Builtin, "lyrics-preset-quiet", "lyrics"),
                entry(PresetScope::Other("Day".into()), "Bright", "lyrics"),
                entry(PresetScope::Other("Night".into()), "Dimmer", "lyrics"),
            ],
            "lyrics",
        );
        let order: Vec<_> = groups
            .iter()
            .map(|(scope, presets)| {
                let names: Vec<_> = presets.iter().map(|p| p.name.as_str()).collect();
                (scope.clone(), names)
            })
            .collect();
        assert_eq!(
            order,
            [
                (PresetScope::Builtin, vec!["lyrics-preset-quiet"]),
                (PresetScope::Workspace, vec!["Mine"]),
                (PresetScope::Other("Night".into()), vec!["Dim", "Dimmer"]),
                (PresetScope::Other("Day".into()), vec!["Bright"]),
            ],
            "the global run held only another kind, so it drops out"
        );
        assert!(preset_groups(Vec::new(), "lyrics").is_empty());
    }
}
