# Panels

How the panel system is built: the layout tree and how it serializes, the panel config
model, the customize windows, and pop-out into OS windows. This makes the UI shell
contract from
[components](../02-architecture/02-components.md#ui-shell-and-panel-system) concrete: a
duplicated panel is a second view with its own config over the same entities, a
popped-out panel is a second OS window over those same entities, and a layout serializes
to disk as a shareable artifact. Version-sensitive: the dock is the vendored
`rox-dock` crate, layouts serialize as JSON with serde, saved workspaces are zip archives
written through `rox-package` on `zip` 9, and the whole thing is gpui entities and
windows.

## The layout tree

rox runs one dock tree rather than the usual center-plus-side-docks arrangement. The root
is a vertical `StackPanel`: the center `TabPanel` over a horizontal transport row
(`StackPanel`) holding the info, playback, seek, and volume panels, the row pinned at
`TRANSPORT_ROW_H` = 120 px. One tree means closing or moving everything in one region
collapses the rest up into the space. It's built in
`crates/rox/src/workspace.rs` with `DockItem::split_with_sizes` and `DockItem::tabs`.

The live tree is `DockItem` nodes (Split, Tabs, Panel, Tiles); `StackPanel` is a
resizable split, `TabPanel` a tab group, and the leaves are the panels themselves.
Split ratios are dragged at the split level, tabs are dragged between groups, and a
middle-drag out of the window pops a panel into its own OS window (see below).

## Design mode

Design mode is one app-wide flag, `rox_dock::design_mode`, set through
`rox_core::settings::set_design_mode`. Every way of flipping it goes there: the toggle
at the left end of the menubar, the Window menu's Design Mode row, the keymap's
`toggle_design_mode` (Ctrl+Shift+D, Cmd+Shift+D on macOS), the socket's
`rox::ToggleDesignMode`, and the Appearance page's switch. Off, the layout is
furniture. `TabPanel::is_locked` reports every group locked, so tabs don't drag and
nothing drops in, and `can_close_panel` refuses every close. A panel's own menu drops
its Add Panel flyout, Save As Preset, Rename, Duplicate, Pop Out and Close, and a
container hides the controls it floats over its slots.

The menubar's Panels menu only shows while it's on, since every row in it but the first
puts a panel into the layout. It sits right after the toggle and ahead of Application on
the in-window bar and in the menu panel (`bar_menus` and `menu_shows` in
`workspace.rs`), while `MENUS` keeps it last, the order the macOS bar uses. The macOS
bar can't add and drop a menu by state or grey a row, so there the Panels menu stays and,
with design mode off, holds only Composition and a row that turns design mode on
(`gated_items`, `workspace/native_menu.rs`). An empty window keeps its own panel list in
either state, since an empty window has nothing else to do.

The Panels menu's first row opens the Composition window (`composition_window.rs`): the
window's dock as a tree of splits, tab groups and panels, with arrows to reorder a row,
a lock and a gear on each panel, and Convert to Canvas (see Canvases below). The
Workspace settings page's Composition section embeds the same `CompositionView`, so the
two always show the same thing.

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

Container nodes set `panel_name` to `"StackPanel"` or `"TabPanel"` and store their split
sizes or active tab in `info`; leaf nodes store the panel's own name (`"library"`,
`"spectrum"`, `"seek"`) and its config as a raw JSON blob in `PanelInfo::Panel`.
Restoring walks the tree: containers rebuild from their `Stack` / `Tabs` info, and a
leaf routes its name through the `PanelRegistry` to build the panel, feeding the config
blob back in. An unknown name comes back as an invalid-panel placeholder rather than
failing the whole restore.

The dump is stored raw, in `workspace.json` rather than the settings file
([ADR 20](../02-architecture/decisions/20-adr-settings-split.md)). The live working
layout is `LookState::layout` (`serde_json::Value`); saved presets are
`LookState::bundle.layouts`, a `Vec<NamedLayout>` (a `name`, a raw `dump`, and an
optional window `size`); unsaved edits per preset are `LookState::layout_edits`. The dump
stays raw JSON so the file still reads after a layout-schema move, and the workspace
validates it against `LAYOUT_VERSION` = 1 on apply, falling back to the default layout
when the version doesn't match.

Persistence is debounced. `save_layout_soon` waits out `SAVE_DEBOUNCE` = 500 ms of quiet,
then `persist` writes `to_value(dock.dump())` into `LookState::layout`, along with the
window frame, the last track, and the queue, which are written to their own files. With
several windows open the last writer wins.

## The panel config model

A panel is anything implementing the `Panel` trait (`rox-dock/src/panel.rs`). Its
identity is `panel_name() -> &'static str`, an immutable string; the `PanelRegistry`
maps that name to a builder, so the whole system identifies panels by string, never a
type or id enum. The trait also declares the tab title, min/max size, lock and close
policy, and `dump`, which produces the panel's `PanelState`.

Per-view config is per panel. Each panel owns a config struct that serializes into its
`PanelInfo::Panel` blob. The shared frame knobs are in `PanelChrome`
(`crates/rox-panel-api/src/panel.rs`), flattened into every config with `#[serde(flatten)]`:

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
    pub owner: String,            // plugin:<id> for a panel from a plugin's entry
}
```

Panel-specific fields go on the panel's own struct beside the flattened chrome, so a
spectrum's bands or a grid's tile size end up next to the rename and theme in one flat
JSON object. `dump` writes `to_value(config)` into the leaf node; restore reads it back
with `config_from_info`, which deserializes the blob and falls back to `Default` on any
mismatch, so a config from an older shape still opens.

A composable strip's `items` list (track info's pieces, for one) names each item. An item
whose catalog declares variants writes its bare name while it's on the default variant,
and `{ "item": ..., "variant": ... }` on any other, so a list saved before the item had
variants reads unchanged:

```json
"items": ["title", { "item": "artistscrobbles", "variant": "badge" }, "trackscrobbles"]
```

`SavedItem` and `serialize_items` (`crates/rox-panel-api/src/panel/arrange.rs`) do the
reshaping for any panel whose item enum implements `ArrangeItem`. An unknown item or
variant name fails the decode, which puts the panel back on its default config.

`PanelTheme` (`crates/rox-design/src/palette.rs`) is the per-panel look override: a
`colors` map of role name to `#rrggbb`, plus optional `surface_opacity`, `margin`,
`padding`, `rounding`, `border`, and `font`. Empty means inherit the app palette, so a
panel only stores what it overrides.

## Customize windows

Settings split by scope. The app settings window edits the app-wide state, which is
itself split across files by what each one is for
([ADR 20](../02-architecture/decisions/20-adr-settings-split.md)): preferences and
library folders in `settings.json`, the look in `workspace.json`, window shapes in
`windows.json`, playback state in `session.json`, connections in `accounts.json`, and
global panel presets in `presets.json`.

A per-panel customize window edits that one panel's config: `panel_settings::open`
(`crates/rox/src/panel_settings.rs`) opens an OS window keyed to the panel's entity id,
reusing the existing one if already open, sized around 640x480. It shows the panel's own
pages (from `PanelSettings::pages`) then a shared Appearance page editing the chrome's
palette and frame override. Edits apply live to the
panel and are written into its next layout dump.

The sidebar opens with the Presets row (see [Panel presets](#panel-presets)) and a
search box. A query runs against every page's setting labels, descriptions, and keywords
at once and lists the pages with hits under their own headings. Picking a page clears
the search, the way the app settings window's does.

## Pop-out and entity sharing

A panel pops out into its own OS window without duplicating any state. `pop_out`
(`crates/rox-panel-api/src/panel.rs`) detaches the panel from its tab group, then
`pop_out_view` opens a new window, around 900x600, hosting a `PopoutHost`:

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
the main window, and it observes the now-playing art so its backdrop redraws on a new
bake (a popped-out window pumps its own frames).

Coming back is symmetric. The host's context menu offers Dock Back, which re-adds the
panel to the last live tab host and removes the window; the dock's middle-drag-out hook
sends a panel dragged out of the window straight into `pop_out_view`.

## Workspaces

A layout is one arrangement of panels and their configs. A workspace is the wider
shareable unit: a `WorkspaceBundle` (`crates/rox-core/src/settings.rs`) holding a set of
named layout presets with their mini-player roles, the palette and appearance applied
over them, the signal and shader pools, and a table of the files the look carries.

```rust
pub struct WorkspaceBundle {
    pub version: u32,
    pub name: String,
    pub layouts: Vec<NamedLayout>,
    pub panel_presets: Vec<PanelPreset>,          // saved single panels
    pub primary_layout: Option<String>,
    pub mini_layout: Option<String>,
    pub palette_dark: BTreeMap<String, String>,  // role name -> #rrggbb
    pub palette_light: BTreeMap<String, String>,
    pub signals: Vec<Signal>,                     // the pool the looks route from
    pub shaders: Vec<NamedShader>,                // the shader pool
    pub assets: Vec<Asset>,                       // id, file, kind, sha256, bytes
    pub meta: WorkspaceMeta,                      // the card: author, description, ...
    pub post_shader: Option<PostShaderConfig>,
    pub backdrop_shader: Option<PostShaderConfig>,
    pub appearance: AppearanceBundle,             // opacity, frame, fonts, rating style, ...
    pub requires: Vec<String>,                    // plugin ids the panels came from
}
```

`WORKSPACE_VERSION` = 2, independent of the dock `LAYOUT_VERSION` the dumps inside use,
so a reader can refuse a bundle from a newer format while the layouts still validate on
their own version. The bundle is pure look: applying it replaces the palettes, signals,
shaders, appearance, files, and both kinds of preset wholesale and leaves machine- and
account-bound state (library folders, Last.fm, window frames) untouched, so a bundle
travels between installs without dragging along another machine's setup.

A light and dark switch fades every window (`palette::set_mode`). Each window's art tint,
and the base palette a window with no tint draws, eases from where it visibly is to the new
theme, and the 16 ms palette pump projects the widget theme and refreshes every window
until they land. So every panel fades together, docked in the window or inside a canvas
grid, whether a cover tints it or not. The switch takes 350 ms, the app's one pace
(`tokens::EASE_SECS`), unless the look's `theme_ease` names another length in
milliseconds, held to 5000. A look that leaves it out writes nothing new:

```json
"appearance": { "keep_theme": true, "theme_ease": 600 }
```

Reduce Motion cuts the switch, and so does the one at launch, which lands before any window
is open. A second switch mid-fade turns from where the palette visibly is. A song or a
palette edit eases a tint over 350 ms and lands the base palette at once. Every step
of the fade re-renders every view in every window, cached ones included, which is the cost
to weigh against a long `theme_ease`. Measured 2026-11-27 in the release build in a
headless sway at 1920x1080, design mode off, nothing playing, `theme_ease` 600,
`debug.frames` over 1.5 s from 0.2 s before the switch, three switches each way, load under
6: the shipped Default look drew 38 or 39 frames across a switch at 8.4 to 9.2 ms each
(worst 12), where a switch that cut drew 2 at 9.2 to 10.1 ms. Aero drew 57 or 58 at 4.8
to 5.3 ms (worst 9), against 31 at rest, its paced drift, and 56 to 58 at 1.7 to 2.5 ms
when only its canvas items faded and the panels cut. Under Reduce Motion it drew 2 across a
switch and none at rest.

Saved workspaces are `.roxw` files under `workspaces/` in the data directory, which makes
the folder the collection: drop a shared file in and it's in the list, delete it and it's
gone. A `.roxw` is a zip holding `workspace.json`, the bundle, and one flat
`assets/<file>` per entry in the bundle's asset table. Other entries are ignored and
dropped on the next save. `rox_core::settings::roxw` reads the manifest alone, one asset,
or every file the table names, each held to the size and SHA-256 its entry states, and
writes an archive from a bundle and a folder of its files. The zip rules themselves
(names, case folds, entry counts, the unpacked-to-packed ratio) live in `rox-package`.
`workspaces::saved` names each entry off the filename and reads only the directory, so a
manifest is read when a workspace is applied rather than every time a menu draws.

The live look nests the same struct in `workspace.json`, and its files sit in
`<data>/assets/`, the asset store (`rox_core::settings::assets`). The store caches the
table the way the shader pool is cached, with a revision counter that moves on any table
change and when the pool watch sees one of the files change on disk. A file name with a
separator in it has no path in the store, which is what keeps a bundle's JSON from
naming a file outside the folder.

- Saving (`workspaces::store`) packs the live bundle with the files its table names out
  of the store. A file the store doesn't hold comes out of the archive already saved
  under that name, so a card edit on a workspace that isn't live keeps its files.
- Export copies the saved archive, so an exported workspace and a saved one are the same
  bytes. Past 4 MiB the export logs the size and still writes.
- Applying (`Workspace::apply_workspace`) stages the incoming files in
  `<data>/assets.incoming/` first: extracted from the archive, or written from the
  app's embedded copy for a shipped workspace. `assets::swap_in` then swaps the folder
  in whole and sets the table. A stage that fails stops the apply with a toast before
  anything is replaced. `workspaces::apply_look` writes the rest of the look after.
- Import (`workspaces::import_file`) takes a `.roxw` or a version 1 `.json` export,
  dedupes the name against the shipped and saved lists, and stores a fresh archive under
  it. A bundle carrying code nobody on this machine approved, or more than
  `roxw::WARN_BYTES` (256 MiB) unpacked, opens the apply confirm, whose card then shows
  the size. A heavy workspace still imports.

`workspace.json` is the file to hand-edit (ADR 22). `workspaces::watch_live` watches its
folder, and an outside edit re-applies the look after a 500 ms debounce: the palettes,
appearance, pools and asset table go live, and the front window rebuilds its dock only
when the file's `layout` dump moved. rox records the text of each of its own writes in
`rox_core::settings::live_look`, so its own saves never come back as a reload. A file
that doesn't parse or carries a newer version is logged and left alone, and a load that
meets one keeps the last look that parsed, so a half-saved edit never resets the look. A
table entry whose file is missing from `assets/` or doesn't match its hash is logged,
and the look applies without it.

A pool shader's `// @asset name: file` line resolves through its pool entry: the entry's
`ShaderAsset` maps `file` to an asset id and the store gives the bytes. A shader read
from a file falls back to a file beside it. Ejecting a pool shader writes only its
source, and the ejected file keeps resolving its images through the entry. Images are
data and never go through the shader approval gate (ADR 23).

Version 1 bundles carried a shader's images inline, base64 in the `ShaderAsset`. At
launch `workspaces::migrate_v1_files` turns each `workspaces/<stem>.json` into
`<stem>.roxw` and moves the JSON to `workspaces/v1-backup/`. A stem that already has an
archive holding the same bundle is a replay and only moves the JSON. A different
workspace there sends the converted one to a numbered name. `Settings::load` drains a
version 1 look's images into the store after copying the shard to
`workspace.json.bak-v1`. An inline image whose extension no asset kind lists is dropped
from the look and survives only in that backup.

Shipped workspaces are JSON in `crates/rox/assets/workspaces/`, the manifest of an
archive on its own, with their files in a sibling `<Stem>/assets/` folder. Both are
embedded through `rox_design::assets`, and applying one checks each file against its
table entry the way an archive's are checked. Aero is the one written by a script rather
than exported: a single canvas filling the window, generated with its images by
`scripts/workspaces/aero_workspace.py` and `aero_assets.py`. It draws every canvas item
kind, follows playback, whether anything is loaded, shuffle, repeat and the theme through
cases, lights its buttons through hover and pressed looks, stacks its now-playing card,
sizes its panes as their glass plus a fixed shadow, and routes a beat, three bands, the
level and the canvas's clock, so it doubles as the canvas's test bed, with tests in
`workspaces.rs` holding it to one canvas, its assets, a beat, buttons that light, the
weight limits, one theme fade, panes whose boxes are their glass and a card that fits at
every window shape.

## Panel presets

Between one panel with its stock config and a whole saved layout is the panel preset: a
`PanelPreset` is one panel's `PanelState` under a name, the same leaf a layout dump
stores per node. A panel's dropdown saves one (Save As Preset, in
`rox_panel_api::panel_settings`), and `crate::panel_presets` builds it back through the
`PanelRegistry` the way a layout restores that node, including config, rename, and a
composite's children. Presets show as a Presets group in every Add Panel menu, in the
Panels menu, and in the Window menu's New Window from Panel, which opens one through
`pop_out_view`.

A panel kind's presets come from four tiers, listed in this order:

| Tier | Stored in | Writable |
| --- | --- | --- |
| Built-in | Code: each panel module's `builtin_presets()`, named by i18n key | No |
| Global | `presets.json` in the data directory (`PresetsState`) | Yes |
| Workspace | The live `WorkspaceBundle::panel_presets` | Yes |
| Other workspaces | Each saved `.roxw` under `workspaces/` except the live one | No |

`rox_core::settings::panel_presets` stores the two writable tiers and tags each listed
entry with its `PresetScope`. The built-ins and the other workspaces are gathered up in
`crate::preset_sources`, because the built-ins are configs typed by the panel crates and
only the app crate reads the other bundles. Flat menus list global before workspace.

The workspace tier lives in the bundle because a panel can name a shader out of the pool
beside it (`shader::Pick::Named`), and that name only resolves while this workspace's
pool is the live one. Everything that reads what a look would paint
(`unapproved_shaders`, `wears_shaders`, `without_shaders`) walks those presets' dumps
alongside the layouts'. A global preset follows the user across workspaces, so a save to
that tier detaches the dump against the live pool first: each named shader becomes its
inline source, and nothing in `presets.json` names a pool. A preset read from another
workspace is detached against that bundle's pool the same way. An inline source arriving
this way goes through the shader approval gate like any other.

Every panel's customize window carries the Presets row: a picker grouped by tier, Save As
Preset, which asks for the workspace or the global tier, and Reset Changes. A pick goes
through the `apply_panel_preset` opener, which builds the new panel and swaps it into the
old one's slot. In a tab group the new panel is inserted before the old one is removed,
since a group emptied even for a moment takes itself out of its stack. A Group, Drawer,
Overlay, or Slide holds its children in slots instead, and swaps them through
`composite::Hosting::replace_child`. A popped-out panel sits in neither, so its window
shows no Presets row. The rebuild is a new entity, so the customize window follows it to
the new id.

`PresetMarks` keeps, per panel id, the preset that built it, and the picker names the
panel after it while the panel's dump still equals the preset's. The comparison is
against the new panel's own dump of the fresh state rather than the preset's JSON,
because a panel folds and normalizes what it loads. Reset Changes rebuilds from the dump
the customize window took when it opened, along with the name that dump carried, so it
undoes every pick and edit since then. It's inert while the panel still matches that
dump.

A preset read from another workspace comes from that archive's manifest, with nothing
extracted. A global preset detached from a pool shader keeps the shader's source and
loses its images, since presets have no place to keep files.

## Canvases

A canvas is a composite panel, registry name `canvas`, listed under Arrangement in the
catalog ([ADR 33](../02-architecture/decisions/33-adr-canvas-panel.md)). It places items
in 3D space under its own camera. It docks, tabs and pops out like any panel, and the
window's root stays the dock: a full-window canvas is a dock holding one canvas panel in
one tab group, which draws no tab chrome. Convert to Canvas, in the Composition window
and the Workspace page's Composition section, builds exactly that from the current
layout (`Workspace::convert_layout_to_canvas`; the keymap's `convert_layout_to_canvas`
and the socket's `rox::ConvertLayoutToCanvas` reach the same call). Its confirm offers
two forms, and the window looks the same afterwards in either.

- As One Grid puts the whole dump in one grid that fills the stage (`wrap_layout`), so
  its splits still resize the groups beside them.
- Each Group as an Item makes each tab group a grid item of its own (`split_layout`).
  Each group's box comes off the split tree, with every split's sizes as laid out at that
  moment rather than the weights a dump saves, so a pinned row counts at the size it
  shows (`solved_dump`), against the dock's bounds (`group_boxes`). The item gets that
  box, the group's tabs in their order, its active tab and its panels' settings. The
  dividers between groups are gone: each item moves and resizes on its own.

Each item sits on the top-left anchor with its position and its size as fractions of
the stage, the group's edges as the split shares them out. The dock keeps every split's
shares when the window resizes, so at any size the items land where the dock would put
the groups, with no gap or overlap between neighbours. The one place they part is a
panel at its own minimum or maximum: the dock holds it there and hands the difference
to its siblings, while a converted item keeps its fraction. A pinned row scales with the
window like the rest, since the pin was the split's. The stage takes the window's size,
so switching the canvas to Contain afterwards shows the same picture.

Its panels lose their locks on the way in, since a locked panel can't be dragged inside a
grid. There's no undo, so it asks first, with Cancel as the default, and the only way
back is a layout saved beforehand. A window with no panels, or one that's a canvas
already, gets a toast saying so instead. Once converted, design mode is on and the first
grid comes up selected with the inspector open: a full-window grid has no strip to click,
and without that the convert reads as nothing having happened.

The saved shape is `CanvasConfig` (`crates/rox/src/panels/canvas.rs`): the panel chrome,
a `rox_canvas::CanvasState` flattened beside it, and the canvas's signal routes. A canvas
holds a stage size and fit, a camera (vertical field of view, with 0 for orthographic,
and pointer parallax), and a list of items. Each item has an id that's unique across the
whole canvas, a place (an anchor on its parent's box, a position, a size, limits on the
size, a rotation and a scale), visibility, a lock, an opacity, and its content. Grid,
group, image, text and shader content draw. A kind this build doesn't know and a known
kind whose body doesn't parse are both kept as read and written back unchanged.

Each of a size's sides and each of a position's x and y is units or a fraction of the
parent's box along that axis: the stage's, or the group's. A position's z is units only,
since the box has no depth. A fraction reads the same in both, and a position in units
saves as a bare number, so a place saved before fractional positions reads and writes
the same bytes (`crates/rox-canvas/src/place.rs`):

```json
"place": {
  "anchor": "top_left",
  "position": [{ "stage": 0.25 }, 40.0, 0.0],
  "size": [{ "stage": 0.5 }, { "units": 300.0 }],
  "min_size": [320.0, null],
  "max_size": [960.0, null]
}
```

A fractional position measures from the anchor's point, so `0.25` on a top-left anchor
puts the left edge a quarter of the way across, and on a centred one a quarter of the
width right of the middle. `min_size` and `max_size` are units, `null` for no limit, and
a side resolves to its length held between them, so a fractional width stops shrinking
or growing at its limits. A minimum over its maximum wins, as CSS settles it. A place
with no limit writes neither key. The fields are additive with serde defaults, and no
released build reads a canvas, so neither `CANVAS_VERSION` nor the workspace version
moves for them.

A place can lock its shape with `"aspect"`, width over height. The sizes and limits then
give the space, and the item's box is the largest box of that shape inside it
(`Place::size_in`, `place::fitted`). The box sits at the anchor's spot inside the space,
since an anchor places a box by its own matching point: a top-left anchor keeps the
space's top-left corner, a centred one centres it. A cover sized as fractions of a window
stays square in a wide, a square and a tall window alike:

```json
"place": {
  "anchor": "top",
  "size": [{ "stage": 0.66 }, { "stage": 0.86 }],
  "max_size": [460.0, 460.0],
  "aspect": 1.0
}
```

The limits bound the space, and the shape fits inside it afterwards. That order keeps
the shape exact and every maximum held. A minimum can't always hold as well: a space
widened to its minimum width but short in height draws a box narrower than that minimum.
Fitting first and limiting after would have broken the shape, which is the one promise a
lock makes. A place without one writes no key, and a zero, a negative or a NaN reads as
no lock. Everything that sizes an item reads the locked box: the projection, a group's box
for its children, a sliced image's face, an image's fit, a grid's dock, a render target's
rest size when it follows its box, the align, fill, fit and distribute tools, and the
drag snapping. Fill on a locked item fills as far as the shape allows, centred. Moving a
set is unchanged, since it moves positions only.

The inspector's Keep Shape row sits under the width and height
(`crates/rox/src/canvas/overlay/place_rows.rs`). Its switch locks the shape the space has
in this window, so nothing moves until the window does, and switching it off drops the
key. While locked the row shows the ratio as a number to type, and an image whose
picture has decoded gets a From Picture press that takes the picture's own width over
height. Each is one undo step.

In the editor every axis keeps its unit. A drag works out the move in the parent's
units at the item's depth and adds it to each axis in its own unit, so dragging a
fractional axis writes a fraction, and so do a resize from a handle, an arrow-key nudge
(one unit, ten with shift, divided by the parent's length on a fractional axis), the
align, fill and fit tools, a change of anchor, and grouping or moving into a group. A
resize stops at a side's limits, measured from the edge not dragged. A locked item
resizes by one factor (`edit::locked_box`): a corner by the drag along the box's diagonal,
keeping the opposite corner, and an edge by the drag across it, keeping the opposite edge
and the middle of the two beside it. Both sides of the space scale by that factor, each
in its own unit, so the item answers a new window shape as it did before the drag. The
factor stops where a side of the space meets a limit, and where the box would pass the
floor. The inspector's x
and y rows carry the Units or Percent toggle the size rows have, which re-expresses the
axis at the same place, and a fractional side, or one with a limit, shows its minimum
and maximum under it, where an empty field clears the limit. A route on `x` or `y` keeps
its meaning on a fractional axis: its sway is units added on top of wherever the
fraction puts the item.

A length can also be a fraction plus units, saved with both keys: a frame that's its pane
plus a fixed 48-unit shadow margin, a box that's its parent less a fixed strip, or a
position a fraction of the way across with a fixed offset (`Length::Mixed`):

```json
"place": {
  "position": [{ "stage": -0.44, "units": -17.0 }, 90.0, -60.0],
  "size": [{ "stage": 1.0, "units": 48.0 }, { "stage": 1.0, "units": -214.0 }]
}
```

It's a form of its own, kept even while its units are zero, so a drag through zero never
turns it into a bare fraction, and a length saved as one key reads and writes the same
bytes as before. Limits and the aspect lock work on what it resolves to, as on any size.
Every edit that moves or resizes a mixed axis keeps its fraction and puts the change on
its units: a drag, a nudge, a resize from a handle, the align, fill and fit tools, a
change of anchor, and grouping or moving into a group. The item goes on following its
parent the same way. The inspector's unit choice is Units, Percent or Both, and a switch
keeps the length where it is: units or a fraction gaining the other term starts it at
zero, and Both shows the percentage and the units as two numbers. Convert to Canvas
doesn't write it, since a converted layout is fractions of the window and nothing more.

A group can stack its children: `"stack"` on the group item lays them out one after
another along `"axis"` (`"x"` or `"y"`), with `"gap"` units between neighbours,
`"justify"` placing the run along the axis and `"align"` placing each child across it,
each `"start"`, `"center"` or `"end"`, and `"between"` for `"justify"` too
(`crates/rox-canvas/src/stack.rs`). `"padding"` keeps the children in from the group's
edges by units per side, left, top, right, bottom, and `"reverse": true` runs them from
the far end, the first child there, so `"start"` is that end. Fields left out take a
column from the start with no gap, no padding and no reversal, and a stack that doesn't
parse reads as none.

A stacked child carries three fields of its own on its place, each ignored outside a
stack: `"grow"`, its weight in the room the stack has left along the axis, `"margin"`,
units before and after it along the axis, and `"align_self"`, where it sits across in
place of the stack's `"align"`. They live on the place because they're more of what the
place already says, where the box goes and how big, and because a stack already reads
each child's place, so nothing new has to be threaded through the editor or a case. A
card whose cover grows square into whatever its lines, seek bar, buttons and volume
leave, with margins for the air between them:

```json
{
  "id": "card",
  "place": { "size": [{ "stage": 1.0 }, { "stage": 1.0 }] },
  "stack": { "axis": "y", "gap": 0.0, "align": "center", "justify": "center", "padding": [20.0, 20.0, 20.0, 20.0] },
  "content": { "kind": "group", "items": [
    { "id": "cover", "place": { "size": [{ "stage": 0.8, "units": 40.0 }, { "units": 0.0 }], "max_size": [540.0, 540.0], "aspect": 1.0, "grow": 1.0, "margin": [0.0, 12.0] }, "content": { "kind": "image", "asset": "cover" } },
    { "id": "title", "place": { "size": [{ "stage": 1.0 }, { "units": 38.0 }], "margin": [6.0, 4.0] }, "content": { "kind": "text", "text": "%title%", "size": 26.0, "color": "text" } },
    { "id": "seek", "place": { "size": [{ "stage": 1.0, "units": 16.0 }, { "units": 30.0 }], "max_size": [480.0, null], "margin": [0.0, 6.0] }, "content": { "kind": "image", "asset": "seek" } }
  ] }
}
```

The stack decides where each child's box goes and, for one that grows, how long it is.
Every child keeps its own size, limits and aspect against the box inside the group's
padding, so a fraction along the axis is a fraction of that box and not of what's left.
A margin adds to the gap on its side rather than collapsing into it, the way flex items'
margins do, and a child that takes no room takes no margin. The run's length is every
child's length and margins plus the gaps. When it's shorter than the box, the room left
goes to the children that grow, by weight: a child's own size along the axis is the base
it grows from, and its limits still hold, so one that reaches its maximum keeps it and
hands the rest round the others. A child with an aspect grows into the largest box of its
shape that fits both the length it's offered and the space across its own sizes give it,
so a square cover is as large as the card's height and width both allow, capped by its
maximum. Whatever no grower takes goes to `"justify"`, which places the run as before;
`"between"` shares it out into the gaps, the first child at the start and the last at the
end, and with one child or nothing left over reads as `"start"`. With growth there's
nothing left for `"between"` unless every grower stopped short. Nothing grows when the
run is already longer than the box, and nothing shrinks: such a run runs past it, and
the group's inspector says by how many units. The box's top-left goes to the child's cell
whatever its anchor, and the child's own x and y then move it from there, as do its
routes and sway on `x` and `y`. Depth, turns and scale don't take room, so a pulse on a
child's scale never shifts its neighbours, and z order and perspective are as for any
group: the stack lays out the flat boxes before anything is projected
(`project::child_frames`, the cell on `Frame` with its size, which `project::box_size`
reads in place of the child's own).

A child switched off takes no room, no gap and no margin. Off design mode a child whose
case hides it gives its room up over the same ease its fade takes, sitting in the middle
of the share it has left, so its neighbours glide closed while it fades and open again as
it comes back (`crates/rox/src/canvas/stack.rs`, a second set of fades kept for room). A
grower fading out keeps its grown box and takes a share of its room by its presence. In
design mode every child that's switched on keeps its room whatever its case, so a hidden
one draws as a ghost in its own slot and the layout holds still while it's edited.

In the editor a drag along a stack's axis reorders: the child takes the slot past every
sibling whose middle its own has crossed, and its offset along the axis stays as it was.
A reversed stack reads the drag against the list, so the slot still follows the pointer.
Across the axis the drag moves its offset, with snapping only across. A set dragged in a
stack doesn't reorder and moves only across. Arrow keys nudge the offset on both axes. A
resize changes only the size and leaves the stack to move the box, and a side no handle
moved keeps its length as saved. On a child that grows, the stack sets its length along
the axis, and both sides for one with an aspect, so the handles that would move those
sides are greyed and their tooltip says why. A change of anchor moves nothing, and the
aligns along the axis, Fill and Fit are off with a tooltip saying why, as are the set's
aligns and distribute along it. Switching a stack on puts every child's x and y back to
zero so the stack alone places them, and switching it off re-places each child where the
stack had it, a grown child at its grown size. An item moved into a stack, lifted into
one by an ungroup, or a group made from stacked siblings starts with zero offsets in the
first one's slot. The group's inspector carries the stack's switch and its rows, padding,
`"between"` and the reversal among them, and a stacked child's rows say its x and y are
offsets and carry Grow, Margin and its own Alignment. Each choice there is its own undo
step.

The editor can hold several items at once. The selection is one ordered set on the panel
(`Selection`, `crates/rox/src/canvas/selection.rs`), and the last item picked is its
primary: the inspector's single-item rows, the handles and Step Out read the primary, so
a selection of one behaves as it always has. A selection lives inside one parent, the
stage or one group, since a set drags, aligns and reorders in one parent's units and one
paint order. Picking an item under another parent starts a new selection with it.

- On the stage, Shift or Ctrl (Cmd on macOS) adds an item to the set or takes it out.
  Both work there, since the stage has no row order to take a run along. Under a
  modifier the press picks at the set's own level, so a set inside a group grows from
  the stage.
- A plain press on what's selected, or on something inside it, leaves the selection as
  it is, so a drag from it moves what's selected: a nested child once it's picked, or the
  whole set when the press lands on one of several. Only letting go without moving (4
  pixels) steps the selection, one group further in on a second press at the same spot
  and round to the outermost from the innermost, and on one of a set it leaves just that
  one. A plain press anywhere else picks the outermost group under it at once and drags
  that (`selection::press`).
- In the inspector's tree the list modifiers apply, as in rox's lists: Shift takes the
  run of siblings from the primary to the row, Ctrl or Cmd toggles a row, both add the
  run to what's held. Every selected row is lit.
- A drag on empty stage draws a marquee that picks every unlocked item whose projected
  quad it touches, tested against the real quad by separating axes, so a tilted item
  counts only where it draws. Touch rather than enclose: a marquee only starts on empty
  stage, and a stage of large or tilted items leaves little room to start outside one.
  A plain marquee picks among the stage's own items; under a modifier it adds to the set
  among the set's siblings.
- Select All (`canvas_select_all`, Ctrl+A, Cmd+A on macOS) takes every item in the
  primary's parent, or on the stage with nothing selected. Escape clears a set of
  several, and still steps out to the group from a single item.

A set moves together in a drag, each item by the same distance on screen at its own
depth and each axis in its own unit, so a fractional item writes a fraction and one in
units writes units. The arrow keys nudge every item in the set, and Delete removes them
all. Group wraps the set in one new group whose box is the one around their boxes before
any turn, at the depth of the one nearest the viewer, with every child re-placed so
nothing moves on screen (`edit::group_many`). Ungroup lifts every group in the set. The
order tools move the set through its siblings' paint order together and keep its own
order, changing only the depths that have to change (`selection::reordered`). Locked
items in a set don't move. Each of these is one undo step. The set draws each item's
outline and a box around them all, with the resize and turn handles hidden: a set
doesn't resize or turn. The canvas has no copy, paste or duplicate for one item, so
there's none for a set either.

With two or more selected, the six align tools line the set up to its own bounds: left
edges on the leftmost, middles on the middle of the span, and so on, by the same box
before any turn the single-item tools use. With three or more, Distribute Horizontally
and Distribute Vertically make the gaps between neighbours equal, neighbours taken in the
order of their middles and the two outermost staying put. That's by gaps rather than by
middles because items of different sizes read as evenly spaced when the space between
them is even. Only positions move, a fractional axis written back as a fraction. With one
item selected the tools keep their meaning against its parent's box, and every tool's
tooltip says which it's using: the stage, the group, or the selection's bounds.

The inspector for a set shows the count, the aligns and distributes, the order tools,
and three rows the items share in the same sense: the anchor, the opacity and whether
it's visible. Where the items differ the row says Mixed, and an edit on it goes to every
item as one undo step, a slider drag merging into one. Changing the anchor keeps each item
where it is. Position and size rows stay hidden, since each item keeps its own units or
fractions and no one number stands for all of them. Group, Ungroup (while a group is in
the set) and Delete follow. Selection is editor state and never saved or undone.

`rox_canvas` does the placing. An item's transform is its parent frame's times its own,
and it turns and scales about its centre whatever its anchor. A point at depth `z`
scales by `focal / (focal - z)` about the view centre. Siblings paint back to front by
depth, with no depth buffer, so the order the panel adds its children is the whole
depth story. A group takes one slot in its parent's order at its own depth and lays its
children out inside it under its transform.

Live UI is flat. A grid's centre is projected through every parent, and its box is
drawn screen-aligned at its own size: its rotation and scale are stored and unused,
since a live panel can't be drawn tilted. Images take real perspective through
`paint_image_quad`. An image decodes off the UI thread from the asset store, and the
header is read first so an image over the pixel cap (`MAX_PIXELS`, `MAX_SIDE` in
`crates/rox/src/canvas/image.rs`) is refused before its body is decoded. A missing
image shows as a dashed outline with its asset id, only in design mode.

An image can be nine-sliced: `"slice": [left, top, right, bottom]` in its content, insets
in the image's own pixels. The cut (`crates/rox/src/canvas/slice.rs`) keeps the corners at
their size, stretches the edges along their side and the middle both ways, and a sliced
image fills its box whatever its fit. An inset draws as that many of the item's units,
so a corner follows the item's scale and the stage's and never the stretch. A box
smaller than two corners side by side shrinks every inset by the same factor, so the
corners keep their shape and meet, and insets that pass the image are given back in
proportion, leaving a texel between them to stretch. The face's sixteen grid points are
projected once (`rox_canvas::project_points`) and each patch is its own quad from those
shared corners, sampling its own texels through `Window::paint_image_patch`. That's the
gpui patch `e7-image-patch-inner-edges.patch`: a polychrome sprite fades its alpha over
the half pixel inside each side, which on a seam between two patches leaves a faint line
wherever the seam misses a pixel boundary, and on a tilted face it always does. A patch
draws the sides it shares hard, so each pixel along a seam is covered once. Each sampler
clamps to its own patch's texels, so the edge texels of neighbouring patches don't blend
across a seam. The preview of the picture the item draws now, its case's or its own,
with the cut drawn over it sits under the four inset fields in the inspector, and each inset is one undo step. The weight line counts items,
not quads, so a sliced image weighs what any image does.

An item can follow a piece of state from the table Custom Controls picks from (`STATES`
in `crates/rox-panel-api/src/buttons.rs`). It names one state id, and for each of that
state's cases it can say whether it shows and an opacity that multiplies its own. An
image can take another asset in a case and a text item another color. A case with no
entry draws the item as it is. A state id or a case id the build doesn't have changes
nothing, so a canvas from a newer build draws those items as if they followed nothing.
The ids are kept as saved. A hidden case takes the item out of the click walk at once and
off the stage once its fade is done (below), and a group's children go with it. The
case's opacity rides the same product
a route's does, so it reaches a group's children and composes with routes and with the
Rest setting. A shader item's opacity doesn't reach its quad, so a shader takes only the
hiding. In design mode a hidden item draws as a ghost at a quarter of its opacity and can
still be picked, and the inspector can preview any case on the selected item while
playback stays as it is. The preview is editor state and never saved.

The states are read once per render, and the playing track's marks only while an item
follows the favourite, through the same mark cache Custom Controls holds. The canvas
watches what Custom Controls watches (the player, the scrobbler, the presence service,
the EQ and the library's mark events) plus the library itself, whose watch switch
notifies without an event. It redraws only when a followed state's case moves, since the
player notifies on every command and state change. A command run from a click looks at the cases again
once its flip has landed. A canvas where nothing follows a state reads none of it and
redraws for none of it. A case's image is an asset in the store like the
item's own: an object exports it with the item, and placing rewrites it to the store's id.

A change of case eases (`crates/rox/src/canvas/fades.rs`). The canvas keeps each drawn
item's last case, and when it moves the item eases from the look it showed to the new one
over 180 ms with a smoothstep (`fades::EASE`), or over its own `ease` in milliseconds,
held to 5000. An item following the theme (`app.theme`) that leaves `ease` out takes the
look's theme ease instead, 350 ms unless the look's `theme_ease` says otherwise (see
Workspaces), so it lands with the panels and the role-coloured text. `"ease": 0` keeps the
cut, for an item where a hard switch is the point. An item that leaves it out writes
nothing new:

```json
{ "id": "play-body", "state": "player.playback", "ease": 250,
  "cases": [{ "when": "playing", "asset": "gel_green" }],
  "content": { "kind": "image", "asset": "gel_aqua", "fit": "contain" } }
```

Opacity blends and a text color blends. Two images crossfade as two draws of the item,
each with its own fit and nine-slice cut, since a case's image has its own size in texels.
How the two draws weigh depends on the incoming picture, measured once as it decodes
(`image::body_alpha`). An opaque one, where at least a tenth of the pixels that show are
fully opaque, comes in over the first half of the ease while the outgoing one goes over the
second, so two opaque pictures never dip through each other (`Mix::cross`). A translucent
one, glass, comes in at the ease's weight `t` while the outgoing one draws at
`(1 - t) / (1 - t * a)`, with `a` the incoming picture's coverage-weighted mean alpha
(`fades::mix_draws`). For two panes of one even alpha that's the true mix, so the glass
keeps its density through the fade, where the opaque rule had both whole at the middle:
at alpha 0.44 that hides 69% of what's behind instead of 44%. It's one opacity per draw,
not per pixel: where the incoming picture's alpha runs above its mean the pair shows a
little less than the mix, and below it a little more. An exact mix would need the lower
draw to follow the upper picture's alpha pixel by pixel, a quad that samples both images,
which the image path doesn't have. Aero's left pane, crossfaded day to night over 5 s in
the release build and sampled over the sky at five points, came within 2.2 RGB levels of
the true mix at every point; under the opaque rule it was 48 levels off a quarter of the
way in and 98 at the middle, already near the night pane. The top picture keeps the item's
animation clock. A case that hides the item fades it out, and the
item leaves the click walk as the fade starts: its shape passes the pointer, and a grid's
panels sit under an occluding blocker for the length of the fade. A case that shows it
takes the pointer at once and fades in. A change that lands mid-fade carries on from the
opacity it had reached, starting from whichever look was showing more, so a color or an
image can step once there. A canvas opens on its cases as they are: an item drawn for the
first time, or again after its group hid it, cuts.

A grid and a render target ease only their opacity, since a case gives neither an image
and a grid can't crossfade two layouts. A shader item cuts, since its quad takes no
opacity. Design mode, the inspector's previews and Reduce Motion cut as before. A fade asks
for frames only while it runs, and every item one change moves starts its fade in the same
render, so a theme switch is one burst of frames, not one per item.

A theme switch eases the app's palette (see Workspaces), so the panels inside a grid fade
with every other panel in the window, and a text color named by a role reads the palette
as it paints that frame (`palette::eased`), so it fades with them. With the theme cases
left on the look's theme ease, panes, rows, text and scenery move as one. A grid needs
nothing of its own for this: its dock renders cached, and the palette's pump refreshes the
window on every step, which re-renders cached views too, the last step included, so no
grid keeps a frame of the old theme. A waveform's role-linked bar colors and the
backdrop's Milkdrop grade read the eased palette too, and the gpui-component tokens a
fade can see are projected from it: the active tab's label, the primary fill on a linked
swatch, a selected or hovered button, a focused field's caret, a disabled field and a link.
What still lands at once: the stock baseline's light or dark pick, which sets which way a
ghost button's hover shades; the stock text selection color, seen only while text is
selected; the tokens of widgets rox doesn't use; and the colors a render target is sent,
which are where the ease will land, sent once.

An item a click command covers can also carry a `hover` look, for while the pointer is
over it, and a `pressed` look, for while a left press that landed on it is held and the
pointer is still over it (`crates/rox/src/canvas/looks.rs`). The command can be its own or
a group's around it: the click walk runs the nearest command in the item's chain, and the
looks follow the click, so hovering a group with a command shows the look on every item
inside it that has no command of its own. A child with its own command is a button of its
own and shows its looks only when it's the one hovered. A look holds what a case can change
but whether the item shows, since a hover that hid its item would lose the pointer: an
opacity that multiplies, and an image or a text color in place of the one its case gives.
Each is optional, saved only when it changes something, and one that doesn't parse costs
only itself:

```json
{ "id": "play", "action": "toggle_playback",
  "hover": { "opacity": 0.9, "asset": "play-lit" },
  "pressed": { "asset": "play-down" },
  "content": { "kind": "image", "asset": "play" } }
```

The looks go on top of whichever case is active, the pressed look's over the hover
look's, so a pressed look that leaves the image alone keeps the hover's. A case can carry
its own `hover` and `pressed`, which stand in for the item's while that case is drawn, so
a button whose body turns green while playing can light a green body under the pointer.
A case with no look of its own keeps the item's. A button is one group:

```json
{ "id": "play", "action": "toggle_playback", "content": { "kind": "group", "items": [
    { "id": "play-body", "state": "player.playback",
      "hover": { "asset": "gel_aqua_lit" }, "pressed": { "asset": "gel_aqua_down" },
      "cases": [{ "when": "playing", "asset": "gel_green",
                  "hover": { "asset": "gel_green_lit" }, "pressed": { "asset": "gel_green_down" } }],
      "hit": "picture",
      "content": { "kind": "image", "asset": "gel_aqua", "fit": "contain" } },
    { "id": "play-glyph", "hit": false, "state": "player.playback",
      "cases": [{ "when": "playing", "asset": "glyph_pause" }],
      "content": { "kind": "image", "asset": "glyph_play", "fit": "contain" } } ] } }
```

The looks name no other item: a look on a group naming its children by id would break
when placing an object renames them, so each item carries its own look and the group's
command decides when it shows. They show off design mode only, and only on what the click
walk would run (`hit`): the front item under the pointer, by its projected corners so a
tilted item hovers only inside its quad, or the nearest group around it that has a
command, with everything that group covers. A render target that takes the pointer
has no command over it, so it never shows one. The pointer leaving the window clears
them, through gpui's exit event, which the click layer also uses to tell a render target
the pointer has gone. Only the pointer hovers: there's no keyboard focus on canvas items.
Each look eases in and out over 140 ms with a smoothstep. Opacity and a color blend, and
an image crossfades in over what the case draws, the same way a case change does. A canvas asks for frames only while an ease runs: in
a debug build in a headless sway, a hover drew 9 frames in 800 ms and none in the 1.5 s
before or after it. Under Reduce Motion (below) a look lands in the frame its pointer
moved, with no ease and no frames after (`Looks::shown`).

An item says where it takes the pointer with `hit`. Left out, it takes its whole box, as
every item has. `false` lets the pointer through to whatever is behind it, as if the item
weren't there: a glyph over a button's body passes the pointer to the body, and the
group's command covers the body as before. Where nothing else of the group lies behind,
the pointer goes on past the group. `false` on a group lets the pointer through every item
inside it. `"picture"` takes it only where an image item draws at least half opaque
(`image::Picture`): the pointer's point is mapped back through the projected face with the
same inverse `unit_point` gives a render target, then through the fit's crop or the
nine-slice cut to one texel, whose alpha decides. It's one texel read per image under the
pointer per move, from the decoded copy the image keeps, on the frame showing now. During
a crossfade either picture counts. A hover or pressed look doesn't move it: the hit reads
the picture the case shows at rest, so a lit copy's glow can't hold a hover past the edge
the pointer came in at. An image still decoding takes nothing. Anything but an
image reads `"picture"` as its box, and a grid ignores `hit`, since its panels take their
own input. Design mode picks every item by its box whatever it says.

In design mode the inspector shows a While Hovered and a While Pressed block on an item a
command covers, with the case block's opacity slider and image or color picker, and an
eye that previews the look on the canvas as the case eye does. Each case block that shows
its item carries the same two blocks for the case's own looks, as While Hovered in This
Case and While Pressed in This Case, each with an eye that previews that case under that
look. A Takes the Pointer row picks Whole box, Lets it through, or on an image Its
picture. An item that follows a state has a Case Change row under its
state: the default, a cut, or 100, 250, 500 or 1000 ms, and a length a file holds outside
those shows as it is. Each edit is one undo step, and a slider drag merges into one. A
look edited back to changing nothing is dropped, and a case left with nothing goes with
it. Asset collection and rewriting walk the looks' images with the cases', the cases' own
looks included (`assets`, `assets_mut`), so an object exports them and placing rewrites
them.

A canvas's routes drive `<item id>.<property>` targets: opacity and scale take a share of
the item's own setting, a position or rotation axis a share of its sway. Music routes
follow playback through the canvas's Rest or Hold setting (`crates/rox/src/canvas/routes.rs`).
A time route follows the canvas's own clock instead, which starts at the canvas's first
render and is never the wall clock or the playing position (`Motion::clock`). It applies
at full weight whatever playback does. The canvas is the one host that reads time routes,
so only its route rows offer Time.

Every route on one field of one item counts (`Lives::apply`). Sway shares add, so a time
route and a music route on `bubble.y` move it by both, and one sway serves both: with a
1250 unit rise as the sway, a music nudge of 40 units is a span to `0.032`. Opacity and
scale shares multiply, and each holds at zero, since a share under it means nothing there.
The opacity product stops where the item reaches full, once every route has applied
(`Lives::finish`), so the order the routes are listed in doesn't change the picture. Music
routes compose first and ease to and from rest as one, then the time routes compose over
that eased pose. Paused, the music's part eases away under Rest and the drift carries on;
under Hold the held music pose stays under the drift. A frame with a music route moving
asks for the next at once, so the pacing below only runs when time is all that's left.
Under Reduce Motion each time route rests as below and the music's part keeps moving on
top of it. Position and rotation spans are signed, so `"from": -1.0, "to": 1.0` on a
time route swings round home instead of sitting half its sway off to one side. A canvas with a time route switched on keeps drawing while it's drawn, and
stops when gpui stops rendering it, a hidden tab or a hidden window, the same thing that
pauses a render target. The inspector's weight line says in words that the canvas keeps
drawing while it shows and that this costs power, or that Reduce Motion is holding it
still (`cost::animates`).

Motion that only time drives is paced (`crates/rox/src/canvas/pace.rs`). A frame where a
time route moved something compares every item's projected corners and routed opacity
with the frame before, and wakes the canvas for the next once the fastest corner will
have moved `STEP`, one pixel, or an opacity two of 255 levels, at the speed just
measured. It never waits more than 250 ms, so a sine turning slowly is looked at again
before it picks up speed, and a saw that wraps draws its jump at once. A music route that
moves or eases, a hover ease and the pointer's parallax ask for the next frame straight
away as before, so a canvas following playback still draws every vsync. Aero's fastest
drift, the rising bubbles, comes out near 21 frames a second.

Reduce Motion is one app-wide setting, saved as `reduce_motion` and set through
`rox_core::settings::set_reduce_motion` from the Appearance page's Interface section or
the socket's `settings set reduce_motion system|on|off`. On Linux and Windows it has three
states, Follow the System, On and Off, and follows the system by default. On macOS it's
On and Off, since rox has no read of the preference there without an Objective-C bridge
it doesn't carry, and a saved System reads as Off (`SYSTEM_REPORTS_MOTION`). Linux asks
the desktop portal's Settings interface with `ReadOne` on the session bus: first
`org.freedesktop.appearance` `reduced-motion`, where a value of 1 means reduce (Plasma 6's
portal backend offers it; xdg-desktop-portal 1.20 doesn't standardize it), then GNOME's
`org.gnome.desktop.interface` `enable-animations`. Windows reads the Show Animations switch,
`SPI_GETCLIENTAREAANIMATION`. Both run at startup and again whenever a workspace window
comes to the front (`crates/rox/src/integrations/motion.rs`). No bus, no portal, or no
backend offering either key reads as no preference, logged at debug only.

While it holds, every time route rests (`routes::at_rest`). Each wave sits where its cycle
starts, at its own phase, so the canvas shows the picture it opens on, staggered as its
author set the phases. An opacity route takes the brighter of that and its wave's middle,
so nothing a fade carries is held invisible: Aero's bubbles pair a rising saw with a
triangle fade from 0, and a bubble whose phase sits near the start of its cycle would
otherwise rest unseen. A canvas whose only motion is time then asks for no frames. Music
routes, render targets, the visualizers and Milkdrop are content and keep moving.

Measured 2026-11-26 on a Ryzen 9 5950X with a Navi 21 Radeon, release build, a 1920x1080
window in a headless sway, design mode off, nothing playing, a canvas of 50 image items
with routes on the first N of them. `debug.frames` over 3 s, three runs each:

| Time routes | Frames in 3 s | Mean ms | Worst ms | CPU, 5 s |
|---|---|---|---|---|
| 0, at rest | 0 | | | 1.0% of a core |
| 1 | 186 | 0.73 to 0.79 | 1.49 | 5.4% |
| 10 | 185 to 186 | 0.72 to 0.84 | 1.89 | |
| 50 | 184 to 185 | 1.03 to 1.48 | 3.53 | 6.2% |

Another tree was compiling during the 50-route runs and the CPU readings. Most of the cost
is having any frame at all; each route adds little.

Aero measured the same way, its library holding 61 short test tracks, CPU over 10 s,
with the machine's load under 6, before pacing and the cached grid docks and after. The
at-rest rows span two sets of three runs:

| Aero | Frames in 3 s | Mean ms | CPU, 10 s |
|---|---|---|---|
| At rest, before | 185 to 186 | 1.98 to 2.24 | 12.5% to 14.8% of a core |
| At rest, after | 62 to 63 | 1.80 to 2.21 | 4.1% to 5.3% |
| At rest, Reduce Motion on | 0 | | 0.1% to 0.3% |
| Playing, before | 185 | 2.51 to 2.82 | 16.6% to 17.6% |
| Playing, after | 186 | 1.93 to 2.32 | 14.0% to 16.1% |

The "before" build carried frame pointers for profiling, which a stripped release build
of the same commit matched within the spread (2.04 to 2.14 ms). A frame's cost is mostly
gpui's whole-window frame rather than the canvas: in a `perf` profile of Aero at rest the
canvas's own render was 11% of the main thread, while layout, replaying every cached
view's recorded paint into the new scene, sorting it, moving element state across and the
GPU submit made up most of the rest. A canvas of one image and a grid holding only the
library cost 1.2 ms a frame where the image alone cost 0.35 ms, with the library
re-rendering nothing: its cached frame still replays every glyph.

A grid item is a dock tree of its own: one `DockArea` per item, built and gated the way
the window's dock is (the layout version and a split at the root), with the middle-drag
pop-out hook. Its tab groups are real tab groups, so they report into the workspace's
`TabHosts`, and Add Panel and focus lookups reach them. A grid emptied of its last
panel keeps one empty tab group to drop onto. It draws nothing to click, so Add Panel
reaches it while its grid item is selected in design mode. Zoom covers only the grid it
happens in. Each grid's dump sits in its item's `dock` value inside the canvas's
`info.panel`, never in `PanelState.children`: the dock's restore builds every child
before handing a panel its state, so children there would be built twice. A dump that
doesn't restore keeps its item as saved. The dump walkers (shader sources, shader
wearing, stripping, scrubbing, plugin owners) recurse every object and array, so they
reach a grid inside a canvas with no change. A grid's own layout events go to the
workspace's debounced save, since the canvas emits none of its own. Its dock renders as a
cached view (`grid_element`), so a frame the canvas draws for items that moved replays a
grid that didn't. It re-renders when its box or the mask over it changes, when it or
anything inside it notifies, or on a window refresh.

Canvases nest: a canvas panel can sit in a grid inside another canvas, and it projects its
own items inside its bounds under its own camera. Canvases inside grids and groups inside
groups count together against `rox_canvas::MAX_DEPTH` (8), which bounds how deep reading,
building and drawing recurse and how deep a saved layout's JSON nests. Canvases alone
also count against `rox_canvas::MAX_CANVAS_DEPTH` (4), since each one is a panel with
docks of its own, where a group only hands its children a frame. A group or a
canvas-holding grid past either is kept as saved and not built, so a hand-edited layout
can't recurse (`canvas::Depth` carries both counts through a grid's restore).
`Workspace::docks` is the window's dock followed by every dock a canvas in it hosts,
nested ones included. Focus Search and the control socket's Milkdrop lookup walk it,
and the Composition view lists each grid under its canvas.

## Reference

The dock is the vendored `crates/rox-dock`: `panel.rs` (the `Panel` / `PanelView`
traits, the registry), `state.rs` (`DockAreaState`, `PanelState`, `PanelInfo`). The app
wires it in `crates/rox/src/workspace.rs` (the layout tree, persist and restore, the
menus and the convert), `crates/rox/src/workspace/menubar.rs` (the bar and its design
mode toggle), `crates/rox/src/composition_window.rs` (the Composition view and window),
`crates/rox-panel-api/src/panel.rs` (`PanelChrome`, `AppState`, `pop_out`,
`PopoutHost`), `crates/rox/src/panel_settings.rs` (the customize windows),
`crates/rox-core/src/settings.rs` (`NamedLayout`, `WorkspaceBundle`, `Asset`,
`ShaderAsset`), `crates/rox-design/src/palette.rs` (`PanelTheme`, the eased theme switch
and `theme_ease`), and
`crates/rox/src/workspaces.rs` (store, import, the stage before apply, `apply_look`,
`presets_elsewhere`, the version 1 folder migration). Workspace files span
`crates/rox-core/src/settings/roxw.rs` (the archive, `upgrade`, `read_v1`),
`crates/rox-core/src/settings/assets.rs` (the asset store, `swap_in`, `rev`),
`crates/rox-core/src/settings/live_look.rs` (own writes, the last good look),
`crates/rox-package/src/lib.rs` (the zip rules), `crates/rox-design/src/assets.rs`
(`shipped_workspaces`, `shipped_workspace_asset`), and
`crates/rox-panel-api/src/panel/shader/chain.rs` (`@asset` resolution). Each panel's config is defined beside it under
`crates/rox-panels/src/`, with its `builtin_presets()`. Panel presets span
`crates/rox-core/src/settings/panel_presets.rs` (the writable tiers, `PresetScope`),
`crates/rox/src/preset_sources.rs` (the tiers gathered per kind),
`crates/rox-panel-api/src/panel_settings.rs` (the Presets row, `PresetMarks`), and
`crates/rox/src/composite.rs` (`Hosting`, `replace_hosted`, `hosted_docks`). Canvases span
`crates/rox-canvas/src/` (the saved shape in `state.rs`, lengths, positions and size
limits in `place.rs`, the projection in `project.rs`, a stack's layout in `stack.rs`),
`crates/rox/src/panels/canvas.rs` (`CanvasPanel`, `CanvasConfig`, `wrap_layout`,
`split_layout`),
`crates/rox/src/canvas/mod.rs` (the live bodies and the nesting cap),
`crates/rox/src/canvas/grid.rs` (a grid's dock area), `crates/rox/src/canvas/image.rs`
(the decode, the quad and the nine patches), `crates/rox/src/canvas/slice.rs` (the
nine-slice cut), `crates/rox/src/canvas/edit.rs` and `align.rs` (the editor's
arithmetic), `crates/rox/src/canvas/selection.rs` (the selection and the set tools'
arithmetic), `crates/rox/src/panels/canvas/set.rs` (the marquee and the set's edits),
`crates/rox/src/canvas/overlay.rs` with `overlay/place_rows.rs`, `overlay/slice_rows.rs`,
`overlay/set_rows.rs` and `overlay/stack_rows.rs` (the inspector),
`crates/rox/src/canvas/stack.rs` (a stacked child's room as its case fades),
`patches/gpui/e7-image-patch-inner-edges.patch` (`paint_image_patch`), `crates/rox/src/canvas/states.rs` (items that follow state),
`crates/rox/src/canvas/looks.rs` (hover and pressed looks),
`crates/rox/src/canvas/fades.rs` (eased case changes and the crossfade's draw weights),
`overlay/motion_rows.rs` (the
pointer and ease rows),
`crates/rox/src/canvas/routes.rs` (routes on items, the canvas clock, time routes at
rest), `crates/rox/src/canvas/pace.rs` (frames for time-only motion),
`crates/rox/src/integrations/motion.rs` (the desktop's reduced-motion preference),
`crates/rox/src/canvas/cost.rs` (the weight line),
and `Workspace::docks` in `crates/rox/src/workspace.rs`.
