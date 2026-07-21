# Panels

How the panel system is built: the layout tree and how it serializes, the panel config
model, the customize windows, and pop-out into OS windows. This makes the UI shell
contract from
[components](../02-architecture/02-components.md#ui-shell-and-panel-system) concrete: a
duplicated panel is a second view with its own config over the same entities, a
popped-out panel is a second OS window over those same entities, and a layout serializes
to disk as a shareable artifact. Version-sensitive: the dock lives in the vendored
`rox-dock` crate, layouts serialize as JSON with serde, the whole thing is gpui entities
and windows.

## The layout tree

rox runs one dock tree, not the usual center-plus-side-docks arrangement. The root is a
vertical `StackPanel`: the center `TabPanel` over a horizontal transport row
(`StackPanel`) holding the info, playback, seek, and volume panels, the row pinned at
`TRANSPORT_ROW_H` = 120 px. One tree rather than center-plus-bottom-dock, so closing or
moving everything in one region collapses the rest up into the space. It is built in
`crates/rox/src/workspace.rs` with `DockItem::split_with_sizes` and `DockItem::tabs`.

The live tree is `DockItem` nodes (Split, Tabs, Panel, Tiles); `StackPanel` is a
resizable split, `TabPanel` a tab group, and the leaves are the panels themselves.
Split ratios are dragged at the split level, tabs are dragged between groups, and a
middle-drag out of the window pops a panel into its own OS window (see below).

## Layout serialization

A layout dumps to `DockAreaState` (`rox-dock/src/state.rs`), serialized as JSON. rox uses
the `center` field only; `left_dock` / `right_dock` / `bottom_dock` stay `None` and are
skipped:

```rust
pub struct DockAreaState {
    pub version: Option<usize>,
    pub center: PanelState,
    pub left_dock: Option<DockState>,
    pub right_dock: Option<DockState>,
    pub bottom_dock: Option<DockState>,
}
```

`PanelState` is the recursive tree node:

```rust
pub struct PanelState {
    pub panel_name: String,       // "StackPanel", "TabPanel", or a leaf's name
    pub children: Vec<PanelState>,
    pub info: PanelInfo,
}

pub enum PanelInfo {
    #[serde(rename = "stack")] Stack { sizes: Vec<Pixels>, axis: usize }, // axis 0 = horizontal
    #[serde(rename = "tabs")]  Tabs { active_index: usize },
    #[serde(rename = "panel")] Panel(serde_json::Value),                  // the leaf's config
    #[serde(rename = "tiles")] Tiles { metas: Vec<TileMeta> },
}
```

Container nodes carry `panel_name` `"StackPanel"` or `"TabPanel"` and their split sizes
or active tab in `info`; leaf nodes carry the panel's own name (`"library"`, `"spectrum"`,
`"seek"`) and its config as a raw JSON blob in `PanelInfo::Panel`. Restoring walks the
tree: containers rebuild from their `Stack` / `Tabs` info, and a leaf routes its name
through the `PanelRegistry` to build the panel, feeding the config blob back in. An
unknown name comes back as an invalid-panel placeholder rather than failing the whole
restore.

The dump is stored raw. The live working layout is `Settings::layout`
(`serde_json::Value`) in the settings file; saved presets are `Settings::layouts`, a
`Vec<NamedLayout>` (a `name`, a raw `dump`, and an optional window `size`); unsaved edits
per preset are `Settings::layout_edits`. Keeping the dump as raw JSON is deliberate: the
file survives a layout-schema move, and the workspace validates a dump against
`LAYOUT_VERSION` = 1 on apply, falling back to the default layout when the version
doesn't match.

Persistence is debounced. `save_layout_soon` waits out `SAVE_DEBOUNCE` = 500 ms of quiet,
then `persist` writes `to_value(dock.dump())` into `Settings::layout` alongside the
window frame, the last track, and the queue. With several windows open the last writer
wins.

## The panel config model

A panel is anything implementing the `Panel` trait (`rox-dock/src/panel.rs`). Its
identity is `panel_name() -> &'static str`, an immutable string; the `PanelRegistry`
maps that name to a builder, so the whole system identifies panels by string, never a
type or id enum. The trait also declares the tab title, min/max size, lock and close
policy, and `dump`, which produces the panel's `PanelState`.

Per-view config is per panel. Each panel owns a config struct that serializes into its
`PanelInfo::Panel` blob. The shared frame knobs live in `PanelChrome`
(`crates/rox/src/panel.rs`), flattened into every config with `#[serde(flatten)]`:

```rust
pub struct PanelChrome {
    pub title: Option<String>,   // the rename shown as tab and title
    pub theme: PanelTheme,        // palette and frame override
    pub locked: bool,             // pin against drag and rearrange
    pub anchor: bool,             // turn the body into a window-move handle
    pub max_width: Option<f32>,   // dock size caps, px
    pub max_height: Option<f32>,
    pub min_width: Option<f32>,
    pub min_height: Option<f32>,
}
```

Panel-specific fields sit on the panel's own struct beside the flattened chrome, so a
spectrum's bands or a grid's tile size live next to the rename and theme in one flat JSON
object. `dump` writes `to_value(config)` into the leaf node; restore reads it back with
`config_from_info`, which deserializes the blob and falls back to `Default` on any
mismatch, so a config from an older shape still opens.

`PanelTheme` (`crates/rox/src/design/palette.rs`) is the per-panel look override: a
`colors` map of role name to `#rrggbb`, plus optional `surface_opacity`, `margin`,
`padding`, `rounding`, `border`, and `font`. Empty means inherit the app palette, so a
panel only carries what it overrides.

## Customize windows

Settings split by scope. The app settings window edits the settings file (appearance,
behavior, library folders, scrobbling, storage). A per-panel customize window edits that
one panel's config: `panel_settings::open` (`crates/rox/src/panel_settings.rs`) opens an
OS window keyed to the panel's entity id, reusing the existing one if already open, sized
around 640x480. It shows the panel's own pages (from `PanelSettings::pages`) then a shared
Appearance page editing the chrome's palette and frame override. Edits apply live to the
panel and ride into its next layout dump.

## Pop-out and entity sharing

A panel pops out into its own OS window without duplicating any state. `pop_out`
(`crates/rox/src/panel.rs`) detaches the panel from its tab group, then `pop_out_view`
opens a new window, around 900x600, hosting a `PopoutHost`:

```rust
struct PopoutHost {
    panel_view: Arc<dyn PanelView>,  // the same entity, type-erased
    state: AppState,                  // the shared player, library, selection
    backdrop: WindowBackdrop,
    context_menu: Option<...>,
    focus: FocusHandle,
    _backdrop_changed: Subscription,
}
```

The panel is the same gpui entity, moved into the new window's host, so its views point
at the same underlying state as the main window. `AppState` is a bundle of shared
entities (player, library, selection), cloned by handle, so playback, library, and
selection stay shared with no cross-window messaging. The host tracks focus under the
`"Workspace"` key context so the playback keybindings dispatch in the popout the same as
the main window, and it observes the now-playing art so its backdrop wakes on a new bake
(a popped-out window pumps its own frames).

Coming back is symmetric. The host's context menu offers Dock Back, which re-adds the
panel to the last live tab host and removes the window; the dock's middle-drag-out hook
sends a panel dragged out of the window straight into `pop_out_view`.

## Workspaces

A layout is one arrangement of panels and their configs. A workspace is the wider
shareable unit: a `WorkspaceBundle` (`crates/rox/src/settings.rs`) carrying a set of
named layout presets with their mini-player roles, the palette, and the appearance that
dress them.

```rust
pub struct WorkspaceBundle {
    pub version: u32,
    pub name: String,
    pub layouts: Vec<NamedLayout>,
    pub primary_layout: Option<String>,
    pub mini_layout: Option<String>,
    pub palette: BTreeMap<String, String>,   // role name -> #rrggbb
    pub appearance: AppearanceBundle,         // opacity, frame, fonts, rating style, ...
}
```

`WORKSPACE_VERSION` = 1, independent of the dock `LAYOUT_VERSION` the dumps inside carry,
so a reader can refuse a bundle from a newer format while the layouts still validate on
their own version. The bundle is pure look: applying it (`workspaces::apply_look`)
replaces the palette, appearance, and layout presets wholesale and leaves machine- and
account-bound state (library folders, last.fm, window frames) untouched, so a bundle
travels between installs without dragging along another machine's setup.

## Reference

The dock is the vendored `crates/rox-dock`: `panel.rs` (the `Panel` / `PanelView` traits,
the registry), `state.rs` (`DockAreaState`, `PanelState`, `PanelInfo`). The app wires it
in `crates/rox/src/workspace.rs` (the layout tree, persist and restore), `panel.rs`
(`PanelChrome`, `AppState`, `pop_out`, `PopoutHost`), `panel_settings.rs` (the customize
windows), `settings.rs` (`NamedLayout`, `WorkspaceBundle`), `design/palette.rs`
(`PanelTheme`), and `workspaces.rs` (apply). Panel configs live on each panel under
`crates/rox/src/panels/`.
