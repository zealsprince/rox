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
    actions, div, prelude::*, px, size, AnyElement, App, Bounds, Context, Div, Entity, EntityId,
    Focusable as _, Global, Hsla, KeyBinding, MouseDownEvent, PathPromptOptions, ScrollHandle,
    SharedString, Subscription, WeakEntity, Window, WindowHandle,
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
use rox_design::palette::{self, Palette, PanelTheme, Side, Sides, ROLES};
use rox_design::tokens;
use rox_services::backdrop::WindowBackdrop;
// The frame sliders' ceilings live in settings, shared with the app
// settings window so the per-panel and app-wide frames scrub the same
// range, every knob running from zero (off) up to its own, in px.
use crate::signal_ui::{self, routes::RouteEditState};
use rox_core::settings::{BORDER_MAX, MARGIN_MAX, PADDING_MAX, ROUNDING_MAX};
use rox_dock::TabPanel;
use rox_panel_kit::ui::{
    self as settings_ui, grid_columns, kbd_line, section, sidebar, small_button, Seg, SidesScrub,
    SECTION_GAP,
};
use rox_viz::signal::Route;

actions!(panel_settings, [Rename, SavePreset]);

/// The key contexts the rename and save-as-preset windows scope their
/// own bindings to.
const RENAME_CONTEXT: &str = "PanelRename";
const PRESET_CONTEXT: &str = "PanelSavePreset";

/// The two dialogs' enter bindings; call once at startup. They sit on
/// each window's root rather than its field, so enter commits wherever
/// focus is - a single-line input sees the key first and propagates it
/// up.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", Rename, Some(RENAME_CONTEXT)),
        KeyBinding::new("enter", SavePreset, Some(PRESET_CONTEXT)),
    ]);
}

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
        PopupMenuItem::new(rox_i18n::t!("panel-settings"))
            .icon(Icon::default().path(icons::SETTINGS))
            .on_click(move |_, _, cx| {
                open(panel.clone(), cx);
            }),
    );
    crate::openers::host_settings_item(menu, child, cx)
}

/// The page a settings window should land on, keyed by the panel it
/// edits. Set by [`open_page`] and taken by the window's next render, so a
/// window that was already open jumps to the page too instead of sitting
/// on whatever was last read.
#[derive(Default)]
struct RequestedPage(HashMap<EntityId, usize>);

impl Global for RequestedPage {}

/// Open a panel's settings window on one of the panel's own pages, named
/// by the label it declares in [`PanelSettings::pages`]. What a panel body
/// points at when it has something to say about its own config: an
/// Inspect button lands on the page holding the thing rather than on
/// Appearance, which is where a plain [`open`] starts.
///
/// A label the panel doesn't declare opens the window as usual.
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
///
/// Saying no parks the shader rather than deleting it: the source, the
/// name and the routes stay on the config with the switch off, so a look
/// somebody wasn't sure about is one toggle away rather than gone.
pub fn pending_shader(
    id: &'static str,
    source: &str,
    path: Option<&Path>,
    approve: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    turn_off: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    let lines: Vec<String> = source
        .lines()
        .take(PENDING_LINES)
        .map(str::to_string)
        .collect();
    let clipped = source.lines().count().saturating_sub(lines.len());
    let origin: SharedString = match path {
        Some(path) => format!("Said to come from {}", path.display()).into(),
        None => "No file behind it; the source travelled inside the layout".into(),
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
                .child(small_button("Turn Off", icons::CLOSE, false, turn_off)),
        )
}

/// The name field behind the save row, on whichever surface is showing
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

/// The save row: a name, and the button that puts this surface's own
/// shader into the workspace's shaders under it. Saving is how a shader
/// stops belonging to one panel - the workspace holds the source from
/// there, any other panel can use the same name, and one edit reaches all
/// of them.
///
/// It stays a plain row rather than an entry in the picker above. Inputs
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

/// The runs the shader picker lists, as `(overlay, examples heading,
/// workspace heading)`. Pulled out of the menu closure so the one rule that
/// decides what a surface may wear is a value a test can read.
///
/// Unfiltered, the headings name what a shader does to the surface under it,
/// because that's the question a panel's picker leaves open. Filtered to
/// overlays there's only one kind left to offer, so the split has nothing to
/// tell apart and the headings go back to naming where a shader came from.
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

/// The picker both shader surfaces lead with, and the rows that follow from
/// whatever it's showing.
///
/// A shader arrives one of a few ways - a shipped example, one of the
/// workspace's shaders, a file, or text that rode in on a layout - and each
/// of those wants a different sentence and different buttons under it. One
/// row picks, and only the rows that selection needs come after, instead of
/// every path's controls stacking on the page at once.
///
/// The actions are plain `fn` pointers rather than closures: every caller
/// passes a method call on its own surface, and the picker's popup has to
/// carry them across a `'static` menu closure.
pub struct ShaderSource<'a, P: 'static> {
    /// Element id prefix. Two surfaces can have their settings open at
    /// once, and a shared id would put them on one popup's state.
    pub id: &'static str,
    /// The workspace shader this config names, if it names one.
    pub name: Option<&'a str>,
    /// The file the source was last read from, a bookmark for reloads.
    pub path: Option<&'a Path>,
    /// What actually runs: the workspace's copy under a name, the config's
    /// own source otherwise, None when a name resolves to nothing.
    pub resolved: Option<&'a str>,
    /// Clearing the shader, for a surface where having none is a state
    /// worth offering. Some puts a None entry at the top of the list.
    pub clear: Option<fn(&mut P, &mut Context<P>)>,
    /// Offer only shaders that declare `// @overlay`. Set by every surface
    /// that has an app underneath it to lose: the whole window, and a
    /// panel whose own body is the thing a scene would paint over. The
    /// Shader panel is the one caller that leaves this false, because
    /// covering that body is the entire point of it.
    ///
    /// It filters what can be picked, never what's installed. A config that
    /// arrived holding a scene keeps running and keeps its name on the
    /// closed picker; it just isn't a thing this list offers again.
    pub overlays_only: bool,
    pub use_example: fn(&mut P, usize, &mut Context<P>),
    pub use_named: fn(&mut P, String, &mut Context<P>),
    pub choose_file: fn(&mut P, &mut Window, &mut Context<P>),
    pub eject: fn(&mut P, &mut Context<P>),
    pub detach: fn(&mut P, &mut Context<P>),
    pub reload: fn(&mut P, &mut Context<P>),
    pub save: fn(&mut P, String, &mut Context<P>),
    /// The half-typed name a save would land under.
    pub field: &'a mut ShaderNameField,
    /// The name a save lands on with that field left empty.
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
            eject,
            detach,
            reload,
            save,
            field,
            fallback,
        } = self;
        let choice = shader::pick(name, path, resolved);

        // The list, grouped the way the app's other grouped menus are: a
        // label item over each run of entries. From File sits at the top
        // with the None entry rather than under the long example list,
        // since pointing rox at a file of your own is the authoring loop's
        // front door and shouldn't take a scroll to reach.
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
                        PopupMenuItem::new("From File...")
                            .icon(Icon::default().path(icons::FOLDER))
                            .on_click(move |_, window, cx| {
                                if let Some(host) = host.upgrade() {
                                    host.update(cx, |this, cx| choose_file(this, window, cx));
                                }
                            }),
                    )
                    .separator();
            }
            // Both lists split the same way, by what the shader does to
            // the surface under it: a scene replaces it, an overlay leaves
            // it usable. Surfacing that here is what keeps "where did my
            // library go" from being how anyone learns the difference, and
            // it's why a workspace's own shaders get the split too rather
            // than one flat run somebody has to have read the WGSL to sort.
            //
            // Filtered to overlays there's only one kind left, so the split
            // has nothing to tell apart and the headings go back to saying
            // where a shader came from, which is the question still open.
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
                // A file is already the editing surface, so a second copy
                // of it would only be a way to drift the two apart.
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
        // Nothing to hand over when there's no source, and a shader that
        // already belongs to the workspace is where saving would put it.
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

/// The open rename windows, keyed by the panel they rename; the same
/// replace-a-stale-handle story as [`OpenPanelSettings`].
#[derive(Default)]
struct OpenRenames(HashMap<EntityId, WindowHandle<Root>>);

impl Global for OpenRenames {}

/// The head of a panel's dropdown tail: the Add Panel flyout above the
/// Panel-section divider, then the section's "Panel" header, then Save As
/// Preset and Rename. Every panel routes into its tail through here, so this
/// one call opens the section for all of them - which is why it owns the
/// leading separator (callers pass their content items straight in, no
/// separator of their own) and why Add Panel, a sibling into this group
/// rather than an op on this panel, sits above the divider that starts the
/// section. Save As Preset is the first thing under it: it reads as the
/// answer to the flyout above, and it is an op on this panel, so it belongs
/// below the divider rather than beside the flyout.
pub fn rename_item<P: PanelSettings>(
    menu: PopupMenu,
    panel: &Entity<P>,
    tab_panel: Option<WeakEntity<TabPanel>>,
    window: &mut Window,
    cx: &mut App,
) -> PopupMenu {
    // Out of design mode the section opens on Panel Settings alone: the
    // flyout that adds a sibling and the two rows that reshape this panel
    // are layout edits, and the divider and header still earn their place
    // separating the panel's own rows from the one that survives.
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
    let bounds = Bounds::centered(None, size(px(380.), px(195.)), cx);
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

    /// The window's own actions: the name already landed on the panel as
    /// it was typed, so committing is closing.
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
                        "Cancel",
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
            // The backdrop paints first, under the input, like every
            // other window over the shared state.
            .children(self.backdrop.layer(&self.state.now_art, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    // The page's own surface over the root's, the same second
                    // pass the settings page takes: the backdrop reads through
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

/// The open save-as-preset windows, keyed by the panel being saved; the same
/// replace-a-stale-handle story as [`OpenPanelSettings`].
#[derive(Default)]
struct OpenPresetSaves(HashMap<EntityId, WindowHandle<Root>>);

impl Global for OpenPresetSaves {}

/// Open a panel's save-as-preset window, or bring its open one to the front.
/// Holds the panel weakly like the rename window, so closing the panel
/// underneath the dialog leaves a window that saves nothing rather than a
/// dangling entity.
fn open_save_preset<P: PanelSettings>(panel: Entity<P>, cx: &mut App) {
    let id = panel.entity_id();
    if let Some(handle) = cx
        .try_global::<OpenPresetSaves>()
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
        "rox - save {} as preset",
        panel::display_name(panel.read(cx).panel_name())
    ));
    let bounds = Bounds::centered(None, size(px(420.), px(230.)), cx);
    let state = panel.read(cx).state();
    let handle = crate::panel::open_child_window(cx, title, bounds, None, move |window, cx| {
        cx.new(|cx| SavePresetWindow::new(panel, state, window, cx))
    });
    cx.default_global::<OpenPresetSaves>().0.insert(id, handle);
}

/// The save-as-preset window: one name field over the panel it was opened
/// from. Committing dumps the panel exactly the way a layout save does and
/// files that dump in the workspace's presets, so adding the preset back
/// anywhere rebuilds this panel with its config, its rename, and whatever
/// children a composite holds.
struct SavePresetWindow<P: PanelSettings> {
    panel: WeakEntity<P>,
    input: Entity<InputState>,
    /// The panel's built-in name, what an empty field saves under.
    fallback: SharedString,
    /// The preset names the workspace already carries, read once at open so
    /// the "this replaces one" note costs a lookup rather than a file read
    /// every time the window paints.
    taken: Vec<String>,
    /// The shared state, for the window's own backdrop.
    state: AppState,
    backdrop: WindowBackdrop,
    _input_events: Subscription,
    /// This window pumps its own frames, so the backdrop needs its own
    /// wake on a new bake.
    _backdrop_changed: Subscription,
}

impl<P: PanelSettings> SavePresetWindow<P> {
    fn new(panel: Entity<P>, state: AppState, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Start from what the panel is called: a renamed panel already
        // carries the name its preset wants, and an unnamed one gets its
        // kind, which is at least a name you can edit rather than a blank.
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
            // The footer says whether this name replaces a preset, so
            // it has to re-read on every keystroke.
            if let InputEvent::Change = event {
                cx.notify()
            }
        });
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        window.focus(&input.read(cx).focus_handle(cx));
        let taken = settings::panel_presets::all(&settings::Settings::load())
            .into_iter()
            .map(|preset| preset.name)
            .collect();
        SavePresetWindow {
            panel: panel.downgrade(),
            input,
            fallback: fallback.into(),
            taken,
            state,
            backdrop: WindowBackdrop::default(),
            _input_events,
            _backdrop_changed,
        }
    }

    /// The name a save lands under: what's typed, or the built-in name when
    /// the field is empty, matching what the placeholder promises.
    fn name(&self, cx: &App) -> String {
        let typed = self.input.read(cx).value().trim().to_string();
        if typed.is_empty() {
            self.fallback.to_string()
        } else {
            typed
        }
    }

    /// Dump the panel under the typed name and close. A panel that went away
    /// while the dialog stood open closes without writing.
    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name(cx);
        if let Some(panel) = self.panel.upgrade() {
            let dump = panel.read(cx).dump(cx);
            match serde_json::to_value(&dump) {
                Ok(value) => settings::panel_presets::save(name, value),
                Err(e) => log::warn!("panel presets: {name} would not serialize: {e}"),
            }
        }
        window.remove_window();
    }

    /// The window's own actions: the save, and either the shortcut for it
    /// or the warning that this name lands on a preset that already
    /// exists.
    fn footer(&self, replaces: bool, name: &str, cx: &mut Context<Self>) -> Div {
        let hint = if replaces {
            div()
                .text_xs()
                .text_color(palette::tone_warn())
                .child(rox_i18n::t!("preset-save-replaces", name = name))
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
                        "Cancel",
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
        let replaces = self.taken.iter().any(|taken| taken == &name);
        div()
            .size_full()
            .flex()
            .flex_col()
            .key_context(PRESET_CONTEXT)
            .on_action(cx.listener(|this, _: &SavePreset, window, cx| this.commit(window, cx)))
            .bg(palette::bg_elevated())
            .text_color(palette::text_bright())
            .text_sm()
            // The backdrop paints first, under the input, like every
            // other window over the shared state.
            .children(self.backdrop.layer(&self.state.now_art, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    // The page's own surface over the root's, the same second
                    // pass the settings page takes: the backdrop reads through
                    // only as the surfaces thin.
                    .bg(palette::bg_elevated())
                    .p(tokens::SPACE_MD)
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
                                    // The menu path it comes back through, in
                                    // keycaps so the two labels read as things
                                    // to click rather than prose.
                                    .child(kbd_line([
                                        Seg::Text(rox_i18n::t!("preset-back-from")),
                                        Seg::Key(rox_i18n::t!("preset-back-add-panel")),
                                        Seg::Text(rox_i18n::t!("preset-back-then")),
                                        Seg::Key(rox_i18n::t!("preset-back-presets")),
                                        Seg::Text(rox_i18n::t!("preset-back-tail")),
                                    ])),
                            ),
                    )),
            )
            .child(self.footer(replaces, &name, cx))
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

/// Flatten a knob's sides onto the widest of them. A knob already
/// uniform stays exactly as it was, override or not, so linking one that
/// was only ever linked doesn't quietly fork it off the app default.
fn link_knob(own: &mut Option<Sides>, app: Sides) {
    let shown = own.unwrap_or(app);
    if shown.uniform().is_none() {
        *own = Some(shown.linked());
    }
}

/// Whether a knob opens split: the sides already differ, so the row has
/// to show them apart or the numbers on screen would be a lie.
fn split_knob(value: Sides) -> bool {
    value.uniform().is_none()
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
    margin_scrub: SidesScrub,
    padding_scrub: SidesScrub,
    rounding_scrub: ScrubState,
    border_scrub: SidesScrub,
    /// Which four-sided knobs the user has open per side. The window's
    /// own state, not the panel's: a knob whose sides happen to match is
    /// still split while it's being edited that way. Seeded from the
    /// knobs that already differ, so reopening shows what's set.
    margin_split: bool,
    padding_split: bool,
    border_split: bool,
    font_scale_scrub: ScrubState,
    /// The Shader page's route editor state: span sliders and which rows
    /// stand open, kept in step with the panel's route list. Ephemeral on
    /// purpose - a fold is where you are, not what you set.
    shader_routes: RouteEditState,
    /// One drag state per shader slot, for the Shader page's hand-set
    /// knobs. Sized once at [`SLOTS`](shader::SLOTS), since the slot count
    /// is the uniform block's width rather than anything the config says.
    shader_slots: Vec<ScrubState>,
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
                    .placeholder(rox_i18n::t!("panel-size-off"))
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
        // The frame rows open split where the knob's sides already differ,
        // whether the panel set them or inherited them.
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
    // as this panel's own override. A side of None comes off the linked
    // strip and sets all four; a side names the one that moved, and the
    // rest fork at whatever the row was already showing. Zero is a real
    // override, not a clear - it squares the panel back off over a
    // rounded app default; the reset button is the way back to following
    // the app.

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

    /// The border's setter, and where an older config's edge mask stops
    /// being a mask: what it was trimming bakes into the widths that land
    /// here, so the panel keeps its look with nothing left to fold.
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

    // The link toggles: splitting only opens the sides up, so nothing
    // moves until one does. Linking is a real edit - it flattens the
    // sides onto the widest of them, forking the knob if the panel was
    // still following a split app default.

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
        self.update_theme(
            |theme| {
                theme.border = None;
                theme.legacy_border_edges = None;
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

    /// One four-sided frame knob's row: the link toggle and its slider or
    /// sliders, with the reset that drops the whole knob back to
    /// following the app once it's forked. `shown` is what the row draws,
    /// the panel's own knob or the app default under it.
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
            // The strip's top is the everyday reach, not the law: a typed
            // value runs past it and the setters take what lands.
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
                theme.legacy_border_edges = None;
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
                rox_i18n::t!("panel-locked"),
                Some(rox_i18n::t!("panel-locked.description")),
                panel::toggle(locked, Self::set_panel_locked, cx),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("panel-drag-anchor"),
                Some(rox_i18n::t!("panel-drag-anchor.description")),
                panel::toggle(anchor, Self::set_panel_anchor, cx),
            ))
            // Only the composition hosts draw these, so the row would be a
            // dead switch on a leaf panel.
            .when(composite, |d| {
                d.child(panel::setting_row(
                    rox_i18n::t!("panel-slot-controls"),
                    Some(rox_i18n::t!("panel-slot-controls.description")),
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
                rox_i18n::t!("panel-min-width"),
                Some(rox_i18n::t!("panel-min-width.description")),
                self.size_limit_row(
                    &self.min_width_input,
                    limits.min_width.is_some(),
                    Self::reset_min_width,
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("panel-max-width"),
                Some(rox_i18n::t!("panel-max-width.description")),
                self.size_limit_row(
                    &self.max_width_input,
                    limits.max_width.is_some(),
                    Self::reset_max_width,
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("panel-min-height"),
                Some(rox_i18n::t!("panel-min-height.description")),
                self.size_limit_row(
                    &self.min_height_input,
                    limits.min_height.is_some(),
                    Self::reset_min_height,
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("panel-max-height"),
                Some(rox_i18n::t!("panel-max-height.description")),
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
            .child(section(
                rox_i18n::t!("panel-section-placement"),
                None,
                placement,
            ))
            .child(section(rox_i18n::t!("panel-section-size"), None, size))
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
        let resolved = shader::resolve_source(shader.name.as_deref(), &shader.source);
        let running = resolved.clone().unwrap_or_default();
        // A source that arrived inside a layout or a bundle doesn't run
        // until it's read and approved here.
        let pending = (!running.trim().is_empty() && !shader::approved(&running)).then(|| {
            let approving = running.clone();
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
                    //
                    // Approving is saying run it, so it also flips a switch
                    // an earlier Turn Off left down.
                    this.edit_shader(
                        |shader| {
                            shader.path = None;
                            shader.enabled = true;
                        },
                        cx,
                    );
                }),
                cx.listener(move |this, _, _, cx| {
                    // Saying no parks the shader, it doesn't delete it. The
                    // source, the name and the routes all stay where they
                    // are with the switch off, so the picker still says what
                    // this panel was wearing and turning it back on is one
                    // toggle plus the approval above.
                    this.edit_shader(move |shader| shader.enabled = false, cx)
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

        // The name a save would land under, read before the field is
        // borrowed for the picker block below.
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
            // A panel is allowed to carry no surface shader at all, unlike
            // the Shader panel, whose whole body is the thing.
            clear: Some(|this: &mut Self, cx| this.clear_shader(cx)),
            // A shader here rides a panel that's already drawing something
            // - a queue, a cover, a set of transport buttons - so a scene
            // doesn't decorate that body, it hides it. The Shader panel is
            // where a full-cover look belongs, and it offers every shader.
            overlays_only: true,
            use_example: |this: &mut Self, index, cx| this.use_shader_example(index, cx),
            use_named: |this: &mut Self, name, cx| this.use_pool_shader(name, cx),
            choose_file: |this: &mut Self, window, cx| this.pick_shader_file(window, cx),
            eject: |this: &mut Self, cx| this.eject_shader(cx),
            detach: |this: &mut Self, cx| this.detach_shader(cx),
            reload: |this: &mut Self, cx| this.reload_shader(cx),
            save: |this: &mut Self, name, cx| this.save_shader_to_pool(name, cx),
            field: &mut self.shader_name,
            fallback: &fallback,
        }
        .render(window, cx);
        let mut source = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(panel::setting_row(
                rox_i18n::t!("panel-surface-shader"),
                Some(rox_i18n::t!("panel-surface-shader.description")),
                panel::toggle(
                    enabled,
                    |this: &mut Self, on, cx| {
                        this.edit_shader(move |shader| shader.enabled = on, cx)
                    },
                    cx,
                ),
            ))
            .child(picked);
        // The filter above only decides what can be picked, so a config
        // that arrived from a bundle or an older build can still be wearing
        // a scene. Say so rather than leaving someone to wonder why the
        // panel under it went missing.
        if enabled && !running.trim().is_empty() && !shader::overlay(&running) {
            source = source.child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!("panel-shader-is-scene")),
            );
        }
        if let Some(error) = error {
            source = source.child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(error),
            );
        }
        source = source.child(panel::setting_row(
            rox_i18n::t!("panel-run-when-idle"),
            Some(rox_i18n::t!("panel-run-when-idle.description")),
            panel::toggle(
                shader.run_when_idle,
                |this: &mut Self, on, cx| {
                    this.edit_shader(move |shader| shader.run_when_idle = on, cx)
                },
                cx,
            ),
        ));

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

        // The same slot list the Shader panel's Bindings page and the app's
        // Overlay Shader section wear, over this panel's own config: the
        // routed slots read out live, the rest are hand-set knobs.
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

        div()
            .flex()
            .flex_col()
            .gap(SECTION_GAP)
            .children(
                pending.map(|body| section(rox_i18n::t!("panel-awaiting-approval"), None, body)),
            )
            .child(section(rox_i18n::t!("panel-section-shader"), None, source))
            .child(section(
                rox_i18n::t!("panel-section-signals"),
                Some(add.into_any_element()),
                editor.list(cx),
            ))
            .child(section(rox_i18n::t!("panel-section-slots"), None, slots))
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

    /// One hand-set slot edit, straight onto the panel's shader config.
    /// Not through [`edit_shader`](Self::edit_shader): that clears the
    /// stored compile message because the source moved under it, and a knob
    /// drag moves no source - a broken shader would go quiet mid-drag and
    /// have nothing to make it speak up again.
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

    /// Point the panel at one of the workspace's shaders. The inline source
    /// goes with the bookmark: the workspace holds what runs from here, and
    /// a second copy sitting on the panel would only be the one that's
    /// wrong after the next edit to the shared entry.
    ///
    /// Nothing is approved on the way through. A workspace shader that came
    /// in with a bundle still has to be read before it runs, and this is
    /// the same choice picking a name in a bundle's config would have made.
    fn use_pool_shader(&mut self, name: String, cx: &mut Context<Self>) {
        self.edit_shader(
            move |shader| {
                shader.name = Some(name);
                shader.source = String::new();
                shader.path = None;
                // Picking a shader is asking to see it. A panel an earlier
                // Turn Off parked would otherwise take the new one and go on
                // painting nothing.
                shader.enabled = true;
            },
            cx,
        );
    }

    /// Load one of the shipped examples. Sits beside the file pick rather
    /// than beside detach because it does the same thing: the panel comes
    /// off whatever it was on and carries this source itself. The bookmark
    /// goes, since an example has no file behind it and a stale one would
    /// have the watch overwrite it a moment later.
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

    /// Take the shader off this panel: no name, no source, no bookmark. The
    /// switch is left alone, since it's the row above and its own decision.
    /// A workspace shader the panel was using stays in the workspace for
    /// whatever else uses it.
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
                rox_i18n::t!("panel-own-opacity"),
                Some(rox_i18n::t!("panel-own-opacity.description")),
                panel::toggle(
                    theme.surface_opacity.is_some(),
                    Self::set_opacity_override,
                    cx,
                ),
            ))
            .when_some(theme.surface_opacity, |d, value| {
                d.child(panel::setting_row(
                    rox_i18n::t!("panel-surface-opacity"),
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
                rox_i18n::t!("panel-margin"),
                Some(rox_i18n::t!("panel-margin.description")),
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
            ))
            .child(panel::setting_row(
                rox_i18n::t!("panel-padding"),
                Some(rox_i18n::t!("panel-padding.description")),
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
            ))
            .child(panel::setting_row(
                rox_i18n::t!("panel-rounding"),
                Some(rox_i18n::t!("panel-rounding.description")),
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
                rox_i18n::t!("panel-border"),
                Some(rox_i18n::t!("panel-border.description")),
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
            rox_i18n::t!("panel-reset"),
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

        // The generic font override: any panel that does not draw its own
        // font control gets a family picker here, resolving to the app font
        // when unset. Panels with their own (the lyrics panel) opt out
        // through `has_own_font` so the page never shows two.
        let font_section = (!own_font).then(|| {
            // The section Reset drops both font overrides at once, inert
            // until one is set; the size row also carries its own inline
            // reset, the frame sliders' pattern.
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
            let body = div()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_MD)
                .child(panel::setting_row(
                    rox_i18n::t!("panel-font"),
                    Some(rox_i18n::t!("panel-font.description")),
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
                    rox_i18n::t!("panel-font-size"),
                    Some(rox_i18n::t!("panel-font-size.description")),
                    self.font_scale_row(theme.font_scale, cx),
                ));
            section(
                rox_i18n::t!("panel-section-font"),
                Some(reset.into_any_element()),
                body,
            )
            .into_any_element()
        });

        div()
            .flex()
            .flex_col()
            .gap(SECTION_GAP)
            .child(section(
                rox_i18n::t!("panel-section-opacity"),
                None,
                opacity,
            ))
            .child(section(
                rox_i18n::t!("panel-section-frame"),
                Some(frame_controls.into_any_element()),
                frame,
            ))
            .children(font_section)
            // The panel's own appearance rows, when it has any: knobs
            // that live on its config rather than its theme, like the
            // grid's art rounding.
            .children(extra)
            .child(section(
                rox_i18n::t!("panel-section-colors"),
                Some(color_controls.into_any_element()),
                body,
            ))
    }
}

impl<P: PanelSettings> Render for PanelSettingsWindow<P> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let columns = grid_columns(window);

        // A page somebody asked for from the panel's own body, taken here
        // rather than at construction so it works on a window that was
        // already open.
        if let Some(panel) = self.panel.upgrade() {
            if let Some(page) = cx
                .default_global::<RequestedPage>()
                .0
                .remove(&panel.entity_id())
            {
                self.page = page;
            }
        }

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
                        .child(rox_i18n::t!("panel-was-closed"))
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
                            rox_i18n::t!("panel-page-appearance"),
                            icons::PALETTE,
                            picked == 0,
                            move |this: &mut Self, _window, cx| {
                                this.page = 0;
                                cx.notify();
                            },
                            cx,
                        ))
                        .child(settings_ui::nav_item(
                            rox_i18n::t!("panel-page-behavior"),
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
                            rox_i18n::t!("panel-page-shader"),
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
                            panel::page_label(label),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The window is the one surface a scene can't be picked for, and this
    /// is the rule that enforces it. Read off the group table rather than
    /// off the menu, which needs a window to build.
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
        // A panel is welcome to wear either, so its picker still lists all
        // of them, and every example lands in exactly one run.
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

    /// Filtered, the split has one side left, so a heading naming it would
    /// be labelling a distinction the list no longer draws.
    #[test]
    fn the_filtered_list_drops_the_split() {
        assert_eq!(shader_groups(true).len(), 1);
        // Against the key rather than the English, so the assertion holds
        // whatever locale the machine running the suite negotiated to.
        assert_eq!(
            shader_groups(true)[0].1,
            rox_i18n::t!("shader-group-examples")
        );
        assert_eq!(shader_groups(false).len(), 2);
    }
}
