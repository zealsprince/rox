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
kind, follows playback, shuffle, repeat and the theme through cases, and routes a beat,
three bands and three running totals, so it doubles as the canvas's test bed, with
tests in `workspaces.rs` holding it to one canvas, its assets, a beat and the weight
limits.

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

An item's size is a fraction of the stage, and its position can only be units, so on
each axis it takes the anchor that keeps its edges where the dock would put them at any
size (`place_axis`): the start for a group against the left or top edge, the end for one
against the right or bottom, the centre for a centred one, each with no offset. Those
follow a window resize exactly. A group that touches neither edge and isn't centred, the
middle one of three uneven columns, takes the anchor that drifts least and an offset in
units, so a resize moves it by that offset's share of the change, and it can gap or
overlap a neighbour. A pinned row scales with the window like the rest, since the pin
was the split's. The stage takes the window's size, so switching the canvas to Contain
afterwards shows the same picture.

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
whole canvas, a place (an anchor on its parent's box, a position, a size in units or as a
fraction of the parent, a rotation and a scale), visibility, a lock, an opacity, and its
content. Grid, group, image, text and shader content draw. A kind this build doesn't
know and a known kind whose body doesn't parse are both kept as read and written back
unchanged.

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

An item can follow a piece of state from the table Custom Controls picks from (`STATES`
in `crates/rox-panel-api/src/buttons.rs`). It names one state id, and for each of that
state's cases it can say whether it shows and an opacity that multiplies its own. An
image can take another asset in a case and a text item another color. A case with no
entry draws the item as it is. A state id or a case id the build doesn't have changes
nothing, so a canvas from a newer build draws those items as if they followed nothing.
The ids are kept as saved. A hidden case takes the item off the stage and out of the
click walk, and a group's children go with it. The case's opacity rides the same product
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

An item with a click command can also carry a `hover` look, for while the pointer is over
it, and a `pressed` look, for while a left press that landed on it is held and the pointer
is still over it (`crates/rox/src/canvas/looks.rs`). A look holds what a case can change
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
look's, so a pressed look that leaves the image alone keeps the hover's. They show off
design mode only, and only on the item the click walk would run (`hit`): the front item
under the pointer, by its projected corners so a tilted item hovers only inside its quad,
or the nearest group around it that has a command. A render target that takes the pointer
has no command over it, so it never shows one. The pointer leaving the window clears
them, through gpui's exit event, which the click layer also uses to tell a render target
the pointer has gone. Only the pointer hovers: there's no keyboard focus on canvas items.
Each look eases in and out over 140 ms with a smoothstep. Opacity blends, and an image or
a color switches halfway through. A canvas asks for frames only while an ease runs: in
a debug build in a headless sway, a hover drew 9 frames in 800 ms and none in the 1.5 s
before or after it.

In design mode the inspector shows a While Hovered and a While Pressed block on an item
with a command, with the case block's opacity slider and image or color picker, and an eye
that previews the look on the canvas as the case eye does. Each edit is one undo step,
and a slider drag merges into one. A look edited back to changing nothing is dropped.
Asset collection and rewriting walk the looks' images with the cases' (`assets`,
`assets_mut`), so an object exports them and placing rewrites them.

A canvas's routes drive `<item id>.<property>` targets: opacity and scale take a share of
the item's own setting, a position or rotation axis a share of its sway. Music routes
follow playback through the canvas's Rest or Hold setting (`crates/rox/src/canvas/routes.rs`).
A time route follows the canvas's own clock instead, which starts at the canvas's first
render and is never the wall clock or the playing position (`Motion::clock`). It applies
at full weight whatever playback does, and on a target a music route also drives, the time
route wins. The canvas is the one host that reads time routes, so only its route rows
offer Time. A canvas with a time route switched on asks for a frame on every render while
it's drawn, and stops when gpui stops rendering it, a hidden tab or a hidden window, the
same thing that pauses a render target. rox has no reduced-motion setting, so nothing else
gates it. The inspector's weight line says in words that the canvas draws every frame
while it shows and that this costs power (`cost::animates`).

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
workspace's debounced save, since the canvas emits none of its own.

Canvases nest: a canvas panel can sit in a grid inside another canvas, and it projects its
own items inside its bounds under its own camera. Canvases inside grids and groups inside
groups count together against `rox_canvas::MAX_DEPTH`, and a group or a canvas-holding grid
past it is kept as saved and not built, so a hand-edited layout can't recurse.
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
`ShaderAsset`), `crates/rox-design/src/palette.rs` (`PanelTheme`), and
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
`crates/rox-canvas/src/` (the saved shape in `state.rs`, the projection in `project.rs`),
`crates/rox/src/panels/canvas.rs` (`CanvasPanel`, `CanvasConfig`, `wrap_layout`,
`split_layout`),
`crates/rox/src/canvas/mod.rs` (the live bodies and the nesting cap),
`crates/rox/src/canvas/grid.rs` (a grid's dock area), `crates/rox/src/canvas/image.rs`
(the decode and the quad), `crates/rox/src/canvas/states.rs` (items that follow state),
`crates/rox/src/canvas/looks.rs` (hover and pressed looks),
`crates/rox/src/canvas/routes.rs` (routes on items, the canvas clock),
`crates/rox/src/canvas/cost.rs` (the weight line),
and `Workspace::docks` in `crates/rox/src/workspace.rs`.
