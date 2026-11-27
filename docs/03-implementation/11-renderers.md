# Renderers

How a renderer is built and how to write one: the package, the process rox starts for a
render target, the shared memory its frames arrive through, the GPU texture it can draw
into instead on Linux, the lines it reads and writes, and the rules that stop it. This
makes [ADR 37](../02-architecture/decisions/37-adr-render-targets.md) concrete, with its
amendments of 2026-11-16 (renderers are listed apart from plugins), 2026-11-23 (shared
memory is rox's own channel), 2026-11-25 (a GPU texture, Linux first) and 2026-11-26 (a
page and a switch of their own, the Renderer panel, and a size that follows the box with
settings of the renderer's own). A renderer runs in the sandbox of
[ADR 38](../02-architecture/decisions/38-adr-renderer-sandbox.md) on Linux. Underneath, a renderer is the package of
[ADR 30](../02-architecture/decisions/30-adr-plugins.md) under its renderer amendment:
the same folder or `.roxp`, the same hash and the same approval, so
[10-plugins.md](10-plugins.md) holds for everything this doc doesn't say otherwise. The
Plugins page's Enable Plugins switch is the plugins' alone and doesn't reach a renderer. A
frame is a stranger's bytes under
[ADR 35](../02-architecture/decisions/35-adr-package-security.md). It belongs to the
plugin host in [components](../02-architecture/02-components.md#plugin-host). Two
examples are there to copy: [`examples/renderers/bars`](../../examples/renderers/bars/)
in Python's standard library, and [`examples/renderers/plasma`](../../examples/renderers/plasma/)
in Rust on rox's own `rox-frames`.

Version-sensitive: rox starts a renderer with `ROX_RENDER_API=1`, and `hello` says 1
(`API`, `rox-services/src/renderers.rs`). The shared memory is layout version 1 behind
the magic `RXF1` (`VERSION` and `MAGIC`, `rox-frames/src/layout.rs`), and the cover
mapping is layout version 1 behind `RXC1` (`rox-frames/src/cover.rs`), and the spectrum
mapping is layout version 1 behind `RXS1` (`rox-frames/src/spectrum.rs`). The manifest
is plugin API 1 (`SUPPORTED_API`, `rox-plugins/src/manifest.rs`). `rox-frames` uses
libc 0.2 on Unix (0.2.186 in rox's lockfile, 0.2.190 in the plasma example's) and
windows-sys 0.61 (0.61.2) on Windows. The sandbox speaks Landlock's ABI 1 to 6 and was
run at ABI 7 on Linux 6.18 (`sandbox.rs`, `rox-plugins/src`), with its seccomp filter
written for x86_64 and aarch64. What [A GPU renderer](#a-gpu-renderer) and
[A GPU texture](#a-gpu-texture) say of wgpu was read in wgpu, wgpu-core and wgpu-hal
30.0.2, which rox doesn't depend on. The texture comes from blade-graphics 0.7.1, vendored with
`patches/blade-graphics/a0-shared-textures.patch`, through gpui's
`Window::register_external_texture` (`patches/gpui/e5-external-user-textures.patch`).

## Trying the examples

1. Copy `examples/renderers/bars` into the plugins folder, which the Renderers settings
   page calls the packages folder and opens with Reveal Folder, or install a `.roxp`
   packed from it there. It lists on the Renderers page, switched off. The page follows
   the folder while it's open, so a copied folder shows without Rescan.
2. Add a Renderer panel: an empty window offers one, and in design mode (the toggle at
   the left end of the menubar) the Panels menu lists it under Visualizers. Press Pick a Renderer and pick Bars, marked "switched off". Its enable
   card comes up over the window: it says Bars draws in a Renderer panel or a render
   target on a canvas, gets the music's signals and the playing position, gets the
   pointer while it's over the render target, gets the colours of the current theme,
   since its manifest asks for the palette, and gets the music's spectrum, since it asks
   for that too. Switch On approves exactly the files in the
   package and switches it on, as the switch on the Renderers page does, and the panel
   starts it.
3. Or on a canvas: with design mode on, right-click empty stage, or press the
   plus in the canvas inspector's header, and pick Add Render Target > Bars
   (`open_add_menu`, `rox/src/panels/canvas.rs`). The same card comes up for a renderer
   that isn't switched on, and the signals offer after it. The new target is a 640 by 360
   unit item that follows its own size on screen (`TARGET_UNITS`): its Size row says
   Follow the Item, its Render Scale 100%, its Fit Cover ([The size](#the-size)). Fixed
   gives it a resolution of its own instead, 1280 by 720 to start (`TARGET_PIXELS`).
   Status says whether the renderer runs, with a button that fixes what it can.
4. Play something. Bars draws a bar for each signal in the pool (Application > Signals
   edits it) in the theme's accent, so a pool with nothing in it draws one empty bar. Along the top it draws 64 thin bars
   from the spectrum.

The Python example needs Python 3.7 or later and a Unix that lends it fd 3, so it exits
on Windows (see [Platforms](#platforms)). Plasma is a Cargo project. Its README says how to build it and put the build in a package folder.

## The package

`plugin.json`, held to every rule of a plugin's manifest, with `capabilities.renderer`
and nothing else. The bars example's:

```json
{
  "id": "bars",
  "name": "Bars",
  "version": "1.0.0",
  "api": 1,
  "entry": {
    "script": { "path": "bars.py", "interpreter": "python3" }
  },
  "meta": {
    "author": "rox",
    "description": "An example renderer in Python's standard library. ...",
    "website": "https://github.com/zealsprince/rox",
    "license": "AGPL-3.0-only"
  },
  "capabilities": {
    "renderer": { "label": "Bars", "palette": true, "spectrum": true }
  }
}
```

`capabilities.renderer.label` is what the Add Render Target menu lists. A blank one
refuses the manifest with "the renderer has no label". A renderer that also declares a
`source`, any `panels` or a true flag under `providers` is refused with "a renderer
declares nothing else, and this one declares ..." (`check_renderer`,
`rox-plugins/src/manifest.rs`), because those would give it a plugin host and the
plugin wire. Unknown keys inside `capabilities` are ignored, as they are for a plugin.

### What a renderer asks for

Four more keys under `capabilities.renderer` ask for something past the signals and
the pointer every renderer gets. Each is a grant: the enable card says it, and rox sends
or offers it only to a renderer whose approved manifest asks (`grants`,
`rox-services/src/plugins.rs`). Left out, each is off, and the card says nothing of
it.

| Key | Type | What it gets | The card says |
|---|---|---|---|
| `signals` | a list | rox offers to add these to the user's pool | Follows these signals: Kick (Band 30-120 Hz), Level. When you place a render target for it, rox offers to add the ones you don't have yet. |
| `palette` | `true` or `false` | the theme's colours, on [`palette` lines](#rox-to-the-renderer) | Gets the colours of the current theme, which follow the cover when song theming is on. |
| `cover` | `true` or `false` | the playing track's cover as pixels, through [the cover mapping](#the-cover-mapping) | Gets the cover of the playing track, as a picture. |
| `spectrum` | `true` or `false` | the music's frequency magnitudes, through [the spectrum mapping](#the-spectrum) | Gets the music's spectrum: how loud each frequency is as it plays, never the sound itself. |

A fourth key, `gpu`, says the renderer can draw into a GPU texture rox lends it (see
[A GPU texture](#a-gpu-texture)). It's `true` or `false`, off when left out, and refused
like the others when it's anything else. It isn't a grant, and the card says nothing of
it: the card lists what a renderer is given about the music, the user and the theme, and
the texture gives it none of that. It's memory for the target's own pixels, which every
renderer already draws. A manifest that gains `gpu` is still a changed manifest, so the
renderer switches off until it's approved again.

A fifth key, `rests`, suggests that a new render target pause the renderer while the
music is paused or stopped. It's `true` or `false`, off when left out, and refused like
the others when it's anything else. It isn't a grant either: it only sets the starting
value of the target's Pause With the Music setting, in a canvas or a Renderer panel, and
the user's setting wins from then on ([Lifecycle](#lifecycle)).

Two more aren't grants, and the card says nothing of either. `resizes`, `true` or
`false` and off when left out, says the renderer reads a size on the wire and draws at
it, so a target that follows its box tells it the new size instead of starting it again
([The size](#the-size)). `settings` lists what a target lets the user set for the
renderer ([Settings](#settings)). A value tells the renderer only what the user picked
for it, nothing about the music, the user or the theme. A manifest that gains either is
still a changed manifest, so the renderer switches off until it's approved again.

A manifest asking for everything:

```json
"capabilities": {
  "renderer": {
    "label": "Pulse",
    "palette": true,
    "cover": true,
    "spectrum": true,
    "gpu": true,
    "signals": [
      { "kind": "band", "low": 30, "high": 120, "name": "Kick", "response": 20 },
      { "kind": "trigger", "low": 2000, "high": 8000, "threshold": 60 },
      { "kind": "level" },
      { "kind": "aggregate", "of": 0, "rate": 2 }
    ]
  }
}
```

Each entry under `signals` is one signal the renderer follows:

| Field | Meaning |
|---|---|
| `kind` | `band`, `level`, `onset`, `beat`, `trigger` or `aggregate`, the kinds on the [pool line](#the-pool). A kind this rox doesn't know is skipped, so a renderer written for a later rox still loads. Required. |
| `low`, `high` | The band in Hz, for `band`, `onset`, `beat` and `trigger`, required for those and refused on the others. From 20 to 20,000, `low` below `high`. |
| `name` | What the signal is called if it's added. At most 32 characters, with no control character and no bidi mark, embedding, override or isolate. Optional. |
| `response` | The Signals window's Response, a whole number from 0 to 100, for the added signal. Optional: 30, the pool's own default. |
| `threshold` | The Signals window's Threshold, a whole number from 0 to 100. Optional: 0, or 50 for a trigger, which never fires at 0. |
| `of` | An `aggregate`'s input: the index of another entry in this list, counted from 0, that isn't an aggregate. Required for an aggregate and refused on the others. |
| `rate` | An `aggregate`'s Rate, above 0 and at most 8. Optional: 1. |

A renderer can suggest how an added signal is shaped and nothing else: Response and
Threshold are where the Signals window puts its own defaults, and a renderer that
expects a sharp pulse or a gated band can say so. Reset on Track, Flush and the routes
stay the user's. The manifest is refused with one reason, on the Renderers page, for more
than 16 entries, two entries a pool signal would answer alike, and any field outside the
ranges above (`check_wanted`, `rox-plugins/src/manifest.rs`).

A renderer's name for a signal is a stranger's text, so it's held to that size and
shown only after the signal's kind and band: the card and the canvas editor list
"Kick (Band 30-120 Hz)", never "Kick" alone, so a name can't pass for another kind of
signal.

`id`, `name`, `version`, `api` and `entry` are required, and a key at the top level the
manifest doesn't know refuses it. `meta` can be left out, and so can each key in it
(`Meta`, `manifest.rs`). The enable card shows `meta.author` and `meta.description`
when they're there, with the top-level `version`. rox reads `meta.website`,
`meta.version` and `meta.license` and shows none of them.

The entry resolves the way a plugin's does (`entry_for`, `manifest.rs`). A script
entry runs `<interpreter> <path>`, with the script's full path as the one argument after
the interpreter's own. For `"interpreter": "python3"` rox tries `python3`, then `python`,
with the `py -3` launcher ahead of both on Windows, and takes the first that answers
`--version` with "Python 3" inside 5 seconds (`aliases` and `speaks_python3`,
`manifest.rs`). The user needs a Python 3 on PATH, or in a folder listed under Program
Folders, which the Plugins page and the Renderers page both show. Where that interpreter
may live under the sandbox is in [The interpreter](#the-interpreter). A native entry maps
`<os>-<arch>` to a file in the package:

```json
"entry": {
  "native": {
    "linux-x86_64": "plasma",
    "macos-aarch64": "plasma",
    "windows-x86_64": "plasma.exe"
  }
}
```

### Other interpreters

Every interpreter name is looked up on the search path a renderer's `PATH` is set to:
the package's own `bin` folder first, then the Program Folders, then rox's own `PATH`
(`search_path`, `rox-plugins/src/search.rs`; `interpreter` and `on_path`,
`rox-plugins/src/manifest.rs`). On Windows each folder is also tried with every
extension in `PATHEXT`, `.EXE;.CMD;.BAT;.COM` when that isn't set.

Only `python3` is probed. Any other name is taken as found as soon as a file of that name
is on the path: rox doesn't run it with `--version` and reads nothing it prints. The
lookup checks that the file exists, not that it can run, so a script in `bin` without its
executable bit is found and then fails to start with "Permission denied", which counts as
an exit. A name found nowhere fails the package when the folder is scanned, with
"interpreter <name> not found" on its row on the Renderers page (`load`,
`rox-plugins/src/loader.rs`).

A runtime that needs flags gets a launcher of its own in `bin`. With
`"interpreter": "run-deno"` and this as `bin/run-deno`, executable:

```sh
#!/bin/sh
# The script's path comes in as $1. deno itself is looked up on the same search
# path, so a deno in a Program Folder is found too.
exec deno run --allow-env --allow-read "$1"
```

In the sandbox the launcher runs, since it's in the package, and so does whatever it
starts from the system's folders. Anything else it starts from outside them is refused,
a `deno` under `~/.deno` included: only the program the entry resolves to is granted its
install ([The interpreter](#the-interpreter)), and here that program is the launcher. A
renderer that needs a deno from the user's home names `deno` as its interpreter instead
and takes deno's defaults, since the entry passes no flags. A Deno renderer gets
`DENO_DIR` in its cache folder.

On Windows the launcher would be `bin/run-deno.cmd`, found through `PATHEXT`, ending in
`deno run --allow-env --allow-read %*`. Rust's standard library starts a `.cmd` through
`cmd.exe`. That's read from the lookup and the standard library's documentation, and
hasn't been run.

A `.roxp` keeps a file's executable bit. Pack stores each file's Unix mode in the archive
(`write_archive`, `rox-package/src/lib.rs`), and the unpack into `plugin-cache` sets the
mode masked to `0755`: the owner's, group's and others' read and execute bits and the
owner's write bit survive, and setuid, setgid, sticky and the group's and others' write
bits go (`extract`, `rox-package/src/lib.rs`). A file the archive holds no Unix mode for,
as a zip made on Windows can, unpacks as `0644` and won't run. Pack on Linux or macOS
from a folder whose launcher has its bit set.

### Settings

`capabilities.renderer.settings` is a list of what a render target lets the user set
for the renderer. Each target keeps its own values: a canvas target in its item's
`config`, a Renderer panel in its own `settings`, so two targets of one renderer can
differ, and a workspace or an object that travels carries them. Plasma's:

```json
"settings": [
  {
    "key": "look", "type": "enum", "title": "Look",
    "choices": [
      { "value": "rainbow", "label": "Rainbow" },
      { "value": "ember", "label": "Ember" },
      { "value": "ice", "label": "Ice" }
    ],
    "default": "rainbow", "apply": "line"
  },
  {
    "key": "rate", "type": "number", "title": "Frames a Second",
    "min": 10, "max": 60, "step": 10, "default": 60, "apply": "restart"
  }
]
```

| Field | Meaning |
|---|---|
| `key` | The name its value goes under on the wire: a lowercase letter, then lowercase letters, digits and underscores, 32 bytes at most, and no two alike. Required. |
| `type` | `enum`, `bool` or `number`. A type this rox doesn't know is skipped, never shown and never sent, so a renderer written for a later rox still loads. Required. |
| `title` | The row's name in the inspector and the panel's settings. Required. |
| `choices` | An `enum`'s choices in the order they're offered, 1 to 32 of `{ "value", "label" }`: `value` is what the renderer is sent, a string of at most 64 bytes, and `label` what the user is shown. Refused on the other types. |
| `min`, `max` | A `number`'s range, both required for it, `min` below `max`. |
| `step` | A `number`'s step from `min`, above 0. Left out, any number in the range. A value off its step lands on the nearest one. |
| `default` | The value a target holds until the user picks one. Left out: an enum's first choice, `false`, or a number's `min`. One the setting doesn't take refuses the manifest. |
| `apply` | `line`, the default, sends a change as a [`settings` line](#rox-to-the-renderer). `restart` starts the target's session again with the new value in `hello`, for a setting the renderer reads only at its start. Any other value refuses the manifest with "has an apply that isn't line or restart". |

A field a setting doesn't know is ignored, so a renderer written for a later rox loads
here with what this rox can show.

A number goes out on the wire the way its declaration is written. One with a step whose
`min`, `max` and `step` are all whole numbers is always sent as an integer: `50`, never
`50.0`. A stepped number is snapped to its step from `min`, then rounded to as many
decimals as `min` or `step` is written with, whichever has more, so a step of `0.1` sends
`0.6` and never `0.6000000000000001` (`accept`, `renderer_settings.rs`). A number with no
step is sent as the user left it, as a decimal. Read every number as a float anyway: JSON
doesn't tell an integer from a float, and a parser that wants one or the other will meet
both from other hosts.

Each type has one control, in the canvas inspector and in a Renderer panel's settings
(`setting_rows`, `rox/src/canvas/target_settings.rs`):

| Type | Shown as |
|---|---|
| `enum` with up to 4 choices | A row of chips, one a choice, side by side |
| `enum` with 5 or more | A dropdown |
| `number` | A slider with its value beside it, which can be typed into |
| `bool` | A switch |

Chips sit side by side, beside the title in a Renderer panel's settings and under it in
the inspector, so four long labels run past the row's width. Keep the labels of an enum
with four choices or fewer to a word or two, and give longer ones to an enum with more,
which a dropdown lists one a line. A title shows in full in the inspector, where the
control goes under it, and beside the control in the panel's settings, where a long one
leaves the control less room.

A title and a label are shown as the package gives them, untranslated, as a plugin's
`config_schema` titles are on the Plugins page: rox can't translate a stranger's words.
They're held to plain text a row can show: at most 48 characters, not blank, with no
control character and no bidi mark, embedding, override or isolate. The manifest is
refused with one reason, on the Renderers page, for more than 32 settings or any field
outside the rules above (`check`, `rox-plugins/src/renderer_settings.rs`).

rox holds every value to the declaration before the renderer hears it (`values`,
`renderer_settings.rs`): an enum's value is one of its choices, a bool is `true` or
`false`, a number is inside its range and on its step. A stored value the declaration
doesn't take reads as the setting's default, and a stored key the manifest no longer
declares is left out of what's sent, with a log line at the session's start
(`renderer <id>: set aside settings its manifest doesn't take: ...`). The target keeps
it stored, so a renderer that declares it again finds it. A renderer is only ever sent
the keys it declares, each with a value it takes.

A target keeps each renderer's values apart. Pointing a canvas target or a Renderer panel
at another renderer puts the values it held aside under the old renderer's id, in the
item's or the panel's `aside`, and brings back the ones it held for the new renderer, or
none (`switch`, `target_settings.rs`). Two renderers that both declare a `look` never
read each other's, and switching back finds the old values. On a canvas the switch is one
undo step. A target saved before `aside` existed holds its renderer's values as it did.

A changed value reaches a running renderer the way its `apply` says: a `settings` line
with the whole set again, or for a `restart` setting a new session, which doesn't count
as an exit. A paused renderer hears a change when it resumes. On a canvas each change is
one undo step.

## Lifecycle

A renderer runs one process per render target. Two targets naming the same renderer are
two processes. A target's `owner` is `plugin:<id>` (`renderer_id`,
`rox/src/canvas/target.rs`), and the canvas starts its renderer the first time it
draws the item. The start goes through the gate on a thread of the target's own: the
package approved for its current hash, switched on, and not stopped by exits (`check`
and `start`, `rox-services/src/renderers.rs`). Enable Plugins isn't part of it, and a
package that declares `renderer` can't declare a source, panels or providers, so the
gate can't start a plugin. rox never starts a renderer at launch, only when a target
asks.

The process starts in the package's folder (for a `.roxp`, rox's unpacked copy under
`plugin-cache`) in the environment a plugin gets: a fixed list of rox's variables, PATH
set to the plugin search path, and `PYTHONDONTWRITEBYTECODE=1`, `PYTHONUNBUFFERED=1` and
`PYTHONUTF8=1` (`spawn_with`, `rox-plugins/src/process.rs`). [What a native renderer
starts with](#what-a-native-renderer-starts-with) has the whole of it. On top of those:

| Variable | Value |
|---|---|
| `ROX_RENDER_API` | `1` |
| `ROX_CACHE_DIR` | Its [cache folder](#the-cache-folder), on every platform and in every tier |
| `ROX_FRAMES_WIDTH`, `ROX_FRAMES_HEIGHT` | The frame mapping's size in pixels. For a renderer that doesn't resize, the size it draws at; for one that does, the most it can draw at ([The size](#the-size)) |
| `ROX_FRAMES_FD` | `3`, on Unix only |
| `ROX_COVER_FD` | `4`, on Unix only, and only for a renderer granted the cover |
| `ROX_TEXTURE_FD` | The texture's fd, on Linux only, and only for a renderer lent one: `4`, or `5` when the cover is lent too |
| `ROX_SPECTRUM_FD` | The spectrum's fd, on Unix only, and only for a renderer granted it: the next one after every handle above that was lent, so `4`, `5` or `6` |

On Unix the renderer leads its own process group and gets its handles from fd 3 up in a
fixed order: the frame mapping at 3 always, then the cover mapping when it's granted,
then the GPU texture when it's lent, then the spectrum mapping when it's granted
(`places`, `rox-services/src/renderers.rs`). A handle added to the contract later goes
after the ones before it, so no handle's number moves for a renderer that doesn't ask
for the new one. Each is named in the environment and in `hello`, so read the number
from there rather than counting. Every fd past the last one lent is marked to close at exec (`lend` and
`close_from`, `process.rs`).
rox writes `hello` as soon as the process runs. A renderer that can't be spawned, or
whose `hello` can't be written, counts as an exit (`launch`, `renderers.rs`).

A target pauses its renderer when nothing shows it. On a canvas that draws, a target
item the canvas skips is paused straight away with `{"pause":true}` (`hold`,
`target.rs`): one that's hidden, one a state takes off the stage outside design
mode, and one with a corner behind the camera or its quad off the stage. A target inside
a group that's hidden, or that a state takes off the stage, isn't paused that way. It
waits, as a target does when its canvas isn't drawn at all, as with a hidden window or a
tab behind another: a frame left untaken for 500 ms pauses it (`LIMITS.idle`,
`target.rs`). Shown again, it gets `{"pause":false}`.

A target can also rest with the music. Its Pause With the Music setting, in the canvas
inspector and in a Renderer panel's settings, pauses the renderer while playback is
paused or stopped, with the same `{"pause":true}`, and keeps its last frame on show
(`rest`, `target.rs`). It's off for a new target unless the renderer's manifest suggests
it with `rests` ([What a renderer asks for](#what-a-renderer-asks-for)), and the user's
setting wins from then on. The causes add up: a target hidden and resting is paused until
it's both shown and the music plays, and the first of those that ends while the other
holds sends nothing. Playback going on wakes a resting target at once, since nothing it
draws would. One that never drew before it rested keeps showing nothing.

While paused rox sends nothing
else, drops what it had queued, and reads nothing from the renderer's stdout. A renderer
that keeps announcing frames fills that pipe and blocks in its own write. rox looks for
an exit once a second while it's paused.

An exit rox didn't ask for leaves the last frame showing, and rox starts the renderer
again after 0, 1, 2 and 4 seconds for the first to fourth exit inside ten minutes. The
fifth stops it (`BACKOFF` and `CRASH_WINDOW`, `rox-plugins/src/host.rs`; `due`
and `note_exit`, `renderers.rs`). A stopped target draws empty, with a dashed
box and the renderer's name in design mode, until the user presses Start Again under
its Status, or switches the renderer off and on. A failed start, a flood of stdout
lines and a run of refused frames each count as an exit. A restarted renderer gets
`hello` and the pool again, and its frame numbers can start again from 0.

A Pop Out or a Dock Back moves the canvas with its targets. A session on the slots
carries on, process and all, so a renderer keeps whatever it has built up. One lent a
texture starts again, since the texture belongs to the window that made it
([When the texture goes](#when-the-texture-goes)). rox frees what the old window held as
the canvas leaves it (`Targets::carry`, `rox/src/canvas/target.rs`).

rox stops a session in two ways:

- Deleting the item, changing how its size is chosen (Follow or Fixed, the render
  scale, a fixed resolution), a change to a `restart` setting and closing the canvas
  close the renderer's stdin, wait up to 200 ms for it to exit, then kill its process
  group (`Session::stop` and `GRACE`, `renderers.rs`). Each of those but the last is a
  new session with a new mapping. Resizing a box that's followed never is.
- Switching the renderer off, removing it and a change to its files kill it at once
  (`enforce`, `renderers.rs`). So does quitting rox (`stop_all`). Turning Enable Plugins
  off doesn't: it stops the plugins and leaves renderers running.

Kill is `SIGKILL` to the process group on Unix. A renderer has no shutdown to answer
and nothing to save. Exiting when stdin closes is all rox asks.

## The size

A target's size is chosen one of two ways, in the canvas inspector and in a Renderer
panel's settings:

| Size | Draws at |
|---|---|
| Follow, the default for a new target | The box's size on screen at rest, in device pixels, times the Render Scale: 50, 75, 100, 150 or 200% |
| Fixed | Its own width and height, from 16 by 16 to 3840 by 2160, whatever the box |

The box is a Renderer panel's whole body, or a canvas item's size at rest: its box and
its own scale, and the own scale of each group it's inside, through the canvas's pixels
per unit (`rest_size`, `rox-canvas/src/project.rs`). Tilt, the camera's perspective and
what routes and sway do to an item or its groups don't count, so a target pulsing to the
bass keeps its size. A following size is rounded to a multiple of 16 pixels and
held to 16 by 16 at the least (`follow_size` and `SIZE_STEP`,
`rox/src/canvas/target.rs`). While the box is being resized, rox waits until its size has
held for 250 ms, with no mouse button down over a Renderer panel's window, then tells the
renderer once (`SETTLE`).

What the renderer gets depends on its manifest:

- One whose manifest says `"resizes": true` gets a frame mapping at the target's ceiling
  for the session's life, and the size to draw at in `hello`. When the box settles at
  another size, rox writes a [`size` line](#rox-to-the-renderer) and the session goes
  on, process and all. The renderer draws each frame at the newest size it holds into
  the start of its slot, and the slot's size words say which ([Writing a
  frame](#writing-a-frame)). With the GPU texture, the texture is made at the ceiling
  too, and the renderer draws into its top-left corner ([A GPU
  texture](#a-gpu-texture)). Its draw size comes from the wire alone, `hello` and then
  each `size` line. The mapping's header, `ROX_FRAMES_WIDTH` and `ROX_FRAMES_HEIGHT`,
  `frames` in `hello` and the texture's extent all give the ceiling, the most it may
  draw at, which is seldom the size to draw at.
- One that doesn't say it gets one size for the session: the box's when the session
  starts, so its shape is right at the start, in a mapping exactly that size. A resize
  after that is shown by the target's Fit and tells the renderer nothing. Its next
  session, after an exit or a setting that restarts it, starts at the box's size then.

The ceiling is the largest connected display's pixels on each side, times the Render
Scale, held to 3840 by 2160 (`ceiling`, `target.rs`). It's worked out when the target's
session is made, from gpui's displays and the window's scale factor, and holds for the
target's life. A box that outgrows it, as a window spanning two displays can, draws at
the largest size of its own shape that fits inside the ceiling and is scaled up
(`held_to`). A Fixed target's ceiling is its own size.

The picture fits the box by the target's Fit when their shapes differ: a renderer that
doesn't resize after a resize, a size rounded to 16, a frame drawn at the size before
while the renderer catches up, or a fixed resolution of another shape.

| Fit | The picture |
|---|---|
| Contain | All of it, with bands of what's behind the target around it |
| Cover, the default for a new target | Fills the box, the middle kept and the overflow cropped |
| Stretch | Fills the box, squeezed to it |

Cover is the default because a following target's picture is the box's own shape but for
the rounding and the moment after a resize, and Cover shows those with nothing squeezed
and no bands. A target saved before Size existed keeps its width and height as Fixed and
reads as Stretch, which is how it drew, so no workspace changes on load. The pointer is
mapped through the fit: over a contained picture's bands it's off the surface, and the
renderer gets `leave`.

## The Renderer panel

A Renderer panel shows one renderer filling a panel: add it from Visualizers, dock it,
tab it, pop it out (`RendererPanel`, `rox/src/panels/renderer.rs`). It runs the same
session as a canvas target (`Shown`, `Target` and `Reading`, `rox/src/canvas/target.rs`),
so everything above holds for it: the gate, the frame paths, the grants, the signals, the
palette, the cover and the pointer. A renderer can't tell the two apart.

Its menu and its settings pick the renderer from every installed one, marked when it
isn't switched on or doesn't load, and picking one that isn't switched on raises its
enable card over the panel's window. The panel saves the renderer as `plugin:<id>` in its
`owner`, the key a plugin's panel uses, so a workspace holding it lists the renderer in
`requires` and its apply card names it when it won't run (`named_renderers`). With no
renderer picked the panel says so and offers Pick a Renderer. With none installed it
offers Open Renderers Settings. A stopped session shows its status with the same button
the canvas inspector has: Switch On... for one switched off or not approved, Open
Renderers Settings for one missing or broken, Start Again for one stopped after its exits.

The panel's settings choose its size the way the canvas inspector does ([The
size](#the-size)): Follow the Panel, the default for a new panel, with its Render Scale,
or Fixed, with the canvas's presets and a width and height. Fit is Contain, Cover or
Stretch, saved under the names the panel first had, `fit` and `fill`, for the first two.
A panel saved with a Fixed resolution keeps it and its fit. One saved on Match the Panel
now follows the panel without starting again. Its settings rows follow, when its
renderer declares any ([Settings](#settings)), stored with the panel. Every change to
the panel's settings, the renderer it picks included, saves the layout half a second
later, as a dock change does.

A Renderer panel paints the theme's background around the picture, so its `palette` line
has `stage`, that colour, whenever Surface Opacity is 100%, docked or popped out alike
(`read`, `rox/src/panels/renderer.rs`). Under 100% it leaves `stage` out, as a canvas
does.

The pointer over the picture goes to the renderer, mapped through the fit, as over a
canvas target outside design mode. The right button is the panel's: its press opens the
panel menu and never reaches the renderer. A pointer over the bands around a contained
picture is off the surface, and the renderer gets `leave`.

A panel behind another tab, or in a minimised window, isn't drawn, so its session pauses
after the untaken frame's 500 ms, as a canvas target's does. Pop Out and Dock Back carry
the session as they carry a canvas's targets: one on shared memory goes on with the same
process, and one lent a texture starts again in the new window.

## The mapping

One block of shared memory per session, little-endian, made by rox
(`Mapping::create`, `rox-frames/src/map.rs`). A 4096-byte header is followed by two
slots. Offsets are bytes from the start of the block or the slot.

| Header offset | Field | Type | Value |
|---|---|---|---|
| 0 | magic | 4 bytes | `RXF1` |
| 4 | version | u32 | 1 |
| 8 | width | u32 | pixels across: the most a frame can be |
| 12 | height | u32 | pixels down: the most a frame can be |
| 16 | stride | u32 | `width * 4` |
| 20 | slots | u32 | 2 |
| 24 | slot_bytes | u64 | `64 + stride * height`, rounded up to a multiple of 4096 |

| Slot offset | Field | Type | Meaning |
|---|---|---|---|
| 0 | seq | u64 | The frame counter: 0 never written, odd while written, even when whole |
| 8 | stamp_ns | u64 | The renderer's clock when the frame was finished, in nanoseconds: `CLOCK_MONOTONIC` on Unix, the system clock since the Unix epoch on Windows |
| 16 | width | u32 | The frame's width: a size the session was told, at most the header's |
| 20 | height | u32 | The frame's height, the same way |
| 24 to 63 | | | Unused |
| 64 | pixels | `width * 4 * height` bytes, of the frame's own width and height | RGBA8, straight alpha, sRGB, top row first, rows packed with no padding at the frame's own width |

Slot `n` starts at `4096 + n * slot_bytes`, and the block is `4096 + 2 * slot_bytes`
long (`Spec`, `rox-frames/src/layout.rs`). For a renderer that resizes, the header's
`width`, `height` and `stride` are the ceiling's and only size the slots: a frame drawn
smaller packs its rows at its own `width * 4` from the slot's pixels, never at `stride`. The 4096 is the layout's page on every
platform, independent of the OS page size. At 1280 by 720 the stride is 5,120 bytes, a
frame 3,686,400, a slot 3,690,496 and the block 7,385,088.

The rules on rox's side:

- rox makes the block, writes the header, seals it where the platform can, and keeps
  only a read-only view. On Linux it's a `memfd` sealed with `F_SEAL_SHRINK`,
  `F_SEAL_GROW` and `F_SEAL_SEAL` (`seal`, `rox-frames/src/map/unix.rs`), so a
  renderer can't resize it under rox's reads. It never maps memory a renderer made, and
  never maps anything executable.
- Every length and offset rox uses comes from the size it asked for. It never reads the
  header back, so a renderer may write the whole block, header included, and changes
  nothing by it. The header is there for the renderer to check.
- No lock is shared. The counter in each slot is the whole protocol.

The pixel format is the texture's: `Rgba8UnormSrgb` (`patches/gpui/c6-dynamic-user-textures.patch`),
fed straight-alpha bytes (`Frame`, `rox/src/canvas/texture.rs`). An alpha below 255
shows the canvas behind the target.

## Writing a frame

Frame `n` counts from 0 and goes to slot `n % 2`:

1. Store `2n + 1` in the slot's `seq`. An odd counter marks the slot as mid-write.
2. Store the frame's width and height at 16 and 20, and its pixels from 64. The size is
   the one the renderer draws at now: `hello`'s, or the newest `size` line's. A renderer
   that doesn't resize draws at the mapping's size, which is `hello`'s too. The rows are
   packed at the frame's width, so a frame smaller than the mapping fills the start of
   the slot and leaves the rest alone.
3. Store the clock in `stamp_ns`: `CLOCK_MONOTONIC` in nanoseconds on Unix and the
   system clock since the Unix epoch on Windows (`clock_ns`, `rox-frames/src/lib.rs`).
   It's for latency figures. rox reads it with the frame and does nothing with it: no
   frame is refused, dropped or reordered over it, and nothing rox shows uses it
   (`copy`, `rox/src/canvas/target.rs`). A runtime that can't read that clock writes the
   closest monotonic count it has, in nanoseconds, or 0.
4. Store `2n + 2` in `seq`. An even counter marks one whole frame.
5. Write `f <n> <slot>\n` to stdout and flush.

Another process has to see those stores in that order: the odd counter before any
pixel, and every pixel before the even counter. `rox_frames::Writer` puts a release
fence after each counter store (`Writer::frame`, `rox-frames/src/write.rs`). Plain
stores in program order give the same on x86-64. On ARM a store can become visible out
of order without a fence. A language with no fence leans on its runtime there, and the
worst that comes of it is one frame shown with some of the previous frame's pixels. No
order a renderer writes in can fault rox.

When an announce arrives, rox reads the slot's counter. It throws the frame away as torn
when the counter is odd, or when it moved by the time the copy finished. It refuses the
frame when the counter is 0, or when the slot's width and height aren't a size the
session was told: the one it was told last, or the one before that, since a frame drawn
just before the renderer read a `size` line is still whole. A size past the mapping's is
refused whatever it was told (`copy_slot_sized`, `rox-frames/src/read.rs`). rox copies
`width * 4 * height` bytes from the slot's pixels, every length worked out from its own
mapping, and shows the frame at its own size, fitted to the box. A whole frame is shown
only if its counter is higher than the last one shown in this session, so the counter
only ever goes up (`copy` and `Told::takes`, `rox/src/canvas/target.rs`). The frame
number in the announce line isn't checked: rox goes by the counter.

rox copies a slot only once the canvas has taken the frame before it, and only the
newest announce waiting. A renderer faster than the screen loses frames it never sees,
at no cost to rox. With two slots, writing frame `n + 1` while rox copies frame `n` is
safe. Writing frame `n + 2` into the slot rox is copying gets that copy thrown away.

`rox_frames::Writer` is the renderer's side in Rust. It's behind the crate's `writer`
feature, which rox's own build never turns on. `Writer::open(handle, spec)` maps the
block and checks its size and header against `Spec::new(width, height)` before it
writes. It's `unsafe`, since one writer per block is the caller's promise.
`writer.frame(n, |pixels| ...)` does the five steps above around the closure and returns
the slot, and `rox_frames::announce(n, slot)` makes the line. `writer.frame_at(n, width,
height, |pixels| ...)` does the same for a frame drawn at another size, handing the
closure `width * 4 * height` bytes, and holds the size to the mapping's.

## The cover mapping

A renderer granted the cover gets a second block of shared memory, the playing track's
cover as pixels. The trust runs the other way from the frame mapping: rox makes it, rox
writes it, and the renderer only reads it (`Cover`, `rox-frames/src/cover.rs`). It never
gets a path to the file, and nothing about the track comes with the pixels. One block per
session, little-endian, 266,240 bytes:

| Offset | Field | Type | Value |
|---|---|---|---|
| 0 | magic | 4 bytes | `RXC1` |
| 4 | version | u32 | 1 |
| 8 | width | u32 | 256 |
| 12 | height | u32 | 256 |
| 16 | seq | u64 | The cover counter: 0 never written, odd while rox writes, even when whole |
| 24 to 63 | | | Unused |
| 64 | pixels | 262,144 bytes | RGBA8, straight alpha, sRGB, top row first, 1,024 bytes a row with no padding |

The block ends at 64 + 262,144 rounded up to 4,096. The square is always 256 by 256. A
cover that isn't square is scaled to fit, keeping its shape, and centred, and the bands
around it are transparent: a 2:1 cover fills rows 64 to 191 (`fit` and `letterbox`).
With nothing playing, or a track with no cover, the pixels are all transparent.

rox writes the pixels the way a renderer writes a frame. It stores the next odd counter,
then the pixels, then the even counter after it, with a release fence after each counter
store (`Cover::write`), and then writes a [`cover` line](#rox-to-the-renderer) naming the
even counter. A renderer reads it the way rox reads a slot:

1. Read `seq`. If it's 0, nothing was written. If it's odd, rox is writing: read again
   later, the next line is on its way.
2. Copy the 262,144 bytes from 64.
3. Read `seq` again. If it moved, the copy is torn: throw it away and read again. If it's
   the same even number, the copy is one whole cover.

`rox_frames::CoverReader` does that in Rust (`read` answers `Whole(seq)`, `Torn` or
`Empty`). It's behind the crate's `cover-reader` feature, which rox's own build never
turns on. `CoverReader::open(handle)` maps the block read-only and checks its size and
header first.

The pixels come from the library's own art cache, the 256-pixel thumbnail every cover
view reads (`rox_library::thumbs::thumbnail`). A remote row's cover comes only from what
that cache already holds, so writing a cover never reaches the network. The decode and
the scale happen on the target's supervisor thread, and once per track for every target:
a second target for the same track copies the first's square (`cover_pixels` and
`COVER_MEMO`, `rox/src/canvas/target.rs`).

The handle a renderer gets can only read:

- On Linux the block is a `memfd`. rox maps its own writable view, then seals the file
  with `F_SEAL_FUTURE_WRITE` along with `F_SEAL_SHRINK`, `F_SEAL_GROW` and `F_SEAL_SEAL`
  (`seal_cover`, `rox-frames/src/map/unix.rs`). From then on a `write` and a shared
  writable `mmap` fail through any fd, and `mprotect` can't make a read-only mapping
  writable. Reopening `/proc/self/fd/4` read-write gives nothing either, since the seal is
  the file's. The renderer's fd 4 is the memfd itself, so `mmap(..., PROT_READ,
  MAP_SHARED, 4, 0)` is how to map it. The future-write seal came in Linux 5.1. On an
  older kernel rox adds the other three seals and the renderer could write the pixels,
  which costs rox nothing: it never reads the block back, and keeps the counter on its
  own side.
- On macOS the design opens the shared memory object a second time by its name,
  read-only, before unlinking it, and lends that fd. Never compiled or run.
- On Windows the section's copy is duplicated with map read alone
  (`COVER_CHILD_ACCESS`, `rox-frames/src/map/windows.rs`). Never compiled or run.

## The spectrum

A renderer granted the spectrum gets a third block of shared memory: the music's
frequency magnitudes, worked out from the same FFT the signal pool reads. The trust runs
the cover's way. rox makes it, rox writes it, and the renderer only reads it
(`Spectrum`, `rox-frames/src/spectrum.rs`). It's magnitudes only, never samples and
never phase. A full-rate waveform is the audio itself, and a renderer could record what
the user plays with it.

There's one block for the whole app, lent to every renderer granted it, since they all
want the same numbers and none of them can write any. rox makes it the first time a
granted session starts and keeps it until it quits (`lease`,
`rox/src/canvas/spectrum.rs`). It's one 4,096-byte page, little-endian:

| Offset | Field | Type | Value |
|---|---|---|---|
| 0 | magic | 4 bytes | `RXS1` |
| 4 | version | u32 | 1 |
| 8 | seq | u64 | The write counter: 0 never written, odd while rox writes, even when whole |
| 16 | stamp_ns | u64 | rox's clock when the write finished, in nanoseconds: `CLOCK_MONOTONIC` on Linux and macOS, the system clock since the Unix epoch on Windows, the clock a slot's `stamp_ns` uses (`clock_ns`, `rox-frames/src/lib.rs`) |
| 24 | flags | u32 | Bit 0 set when the rows are music, clear on the zeros rox writes when it stops |
| 28 | sample_rate | u32 | The output device's rate, which the FFT ran at, 0 when the playing bit is clear |
| 32 | rate | f32 | 62.5, the most writes a second |
| 36 | log_bins | u32 | 128 |
| 40 | log_low | f32 | 20.0, the log row's lowest edge in Hz |
| 44 | log_high | f32 | 20000.0, its highest edge |
| 48 | linear_bins | u32 | 512 |
| 52 | linear_bin_hz | f32 | 21.533203, the linear row's spacing |
| 56 | floor_db | f32 | -66.0 |
| 60 | max_db | f32 | -12.0 |
| 64 | log_at | u32 | 128, where the log row starts |
| 68 | linear_at | u32 | 640, where the linear row starts |
| 72 | log_bytes_at | u32 | 2688, where the log row's bytes start |
| 76 | linear_bytes_at | u32 | 2816, where the linear row's bytes start |
| 80 to 127 | | | Unused |
| 128 | log | 128 f32 | The log row |
| 640 | linear | 512 f32 | The linear row |
| 2688 | log bytes | 128 u8 | The log row as bytes |
| 2816 | linear bytes | 512 u8 | The linear row as bytes |

The rows end at 3,328. Version 1 always has these bin counts, edges, spacing, dB window
and offsets. A later version may only add rows after 3,328, with a higher `version`, and
never changes any of them. So check `magic` is `RXS1`, `version` is at least 1 and the
block is at least 4,096 bytes long. Then read the counts, edges and offsets from the
header anyway, so that what the renderer draws against is what the block says, and
refuse a block whose header names other values than the ones the renderer was written
for: rox didn't write it. `SpectrumReader::open` does that, and answers `Mismatch`.

### The rows

Every value runs from 0 to 1 in the hub's own dB window: a magnitude `m`, with a
full-scale sine at 0 dB, reads `(20 * log10(m) + 66) / 54`, held between 0 and 1. That's
the mapping a pool `band` reads, so -66 dB is 0, -39 dB is 0.5 and -12 dB is 1. The byte
copy of each value is `round(value * 255)`, there so a shader can upload a row as a
texture in one call.

The log row is 128 bins spaced evenly in pitch from 20 Hz to 20 kHz. Bin `i` runs from
`20 * 1000^(i / 128)` to `20 * 1000^((i + 1) / 128)` Hz, about 5.5% wide, and a
frequency `f` lands in bin `floor(128 * ln(f / 20) / ln(1000))`:

| Bin | From | To | Centre |
|---|---|---|---|
| 0 | 20.0 Hz | 21.1 Hz | 20.5 Hz |
| 16 | 47.4 Hz | 50.1 Hz | 48.7 Hz |
| 32 | 112.5 Hz | 118.7 Hz | 115.5 Hz |
| 48 | 266.7 Hz | 281.5 Hz | 274.0 Hz |
| 64 | 632.5 Hz | 667.5 Hz | 649.8 Hz |
| 72 | 973.9 Hz | 1027.9 Hz | 1000.6 Hz |
| 96 | 3556.6 Hz | 3753.8 Hz | 3653.8 Hz |
| 112 | 8433.9 Hz | 8901.6 Hz | 8664.6 Hz |
| 127 | 18949.3 Hz | 20000.0 Hz | 19467.5 Hz |

The FFT under it is the pool's: 2048 points of the mono mix with a Hann window, so at
48 kHz an FFT bin is 23.4 Hz wide (`HUB_FFT`, `rox-viz/src/signal.rs`). The FFT runs at
the output device's rate, the header's `sample_rate`, not the file's. A log bin at least
an FFT bin wide reads the loudest of the FFT bins whose centres fall in `[from, to)`, a
peak as a pool band's is. That's every bin from 57, at 433 Hz, up at 48 kHz, and from
55, at 389 Hz, at 44.1 kHz: the crossover. Below it, several log bins share one FFT bin,
and each reads a parabola fitted in dB through the three FFT bins nearest its centre, so
the row moves smoothly instead of in 23 Hz steps (`Rows::fill`,
`rox-viz/src/spectrum.rs`). That's interpolation, not resolution: a tone at 100 Hz peaks
in its own bin, 29, and lights its neighbours for about 45 Hz either side. Under about 40
Hz the window holds less than two cycles of the tone, and anything there reads as one
hump around 35 Hz.

Nothing reads above the audio's Nyquist frequency, half of `sample_rate`. A log bin whose
lower edge is at or above it reads 0, and one that straddles it takes its peak from the FFT
bins below it. In shared mode the device runs at the mixer's rate, or the one pinned in
rox's output settings, and rox resamples every file to it. So a 44.1 kHz file on a 48 kHz
device and a 22.05 kHz file on one both read at 48 kHz: every bin up to 20 kHz is read,
and those above a 22.05 kHz file's 11,025 Hz read whatever the resampler leaves there,
near 0 but not held to it. In exclusive mode, with no rate pinned, rox opens the device at
the file's rate where the device takes it. A 22.05 kHz file's `sample_rate` is then
22,050: bin 116, from 10,466 to 11,046 Hz, reads up to 11,025 Hz, and bins 117 to 127 read
exactly 0.

A peak reads a sine at its own level whatever the bin's width, but it reads noise higher
in a wide bin than in a narrow one, since the loudest of several noisy FFT bins sits above
a typical one. rox measures both, averaged over hundreds of windows
(`pink_noise_falls_3_db_an_octave_below_the_crossover_and_half_that_above` and
`white_noise_reads_higher_where_the_bins_are_wider`, `rox-viz/src/spectrum.rs`).
White noise reads flat below the crossover, and about 7 dB higher in the top third of the
row than in the bottom third. Pink noise carries the same energy in every octave, so each
FFT bin holds 3 dB less power an octave up. Below the crossover a log bin reads one FFT
bin's level, and the row falls the full 3 dB an octave. Above it the peak takes in twice
as many FFT bins each octave, which makes up about half of that, and the row falls about
1.5 dB an octave: nearer 0.8 dB up to 2 kHz and 1.8 dB from there.

To flatten pink noise, take the crossover's edge `c`, 433 Hz at 48 kHz or 389 Hz at 44.1
kHz, and add `0.056 * log2(f / c)` to bin `f`'s value below it and `0.028 * log2(f / c)`
from it up, 3 dB and 1.5 dB an octave in this window. That holds every bin from 40 Hz up
within 0.035 of the row's mean, under 2 dB. No single slope flattens it. A 3 dB slope
across the whole row leaves the top third more than 0.1 above the bottom third, over 5
dB.

The linear row is the layout ShaderToy-style audio shaders read: 512 bins of 44,100 /
2048 = 21.533 Hz each, from 0 Hz. Bin `j` is the magnitude at exactly `j * 21.533` Hz, a
reading at one frequency, not a peak over a width, so bin 0 is DC, bin 46 is 990.5 Hz and
bin 511 is 11,003.5 Hz. At 44.1 kHz that's the hub's FFT bins exactly. At any other rate
rox interpolates between the two FFT bins either side, in magnitude, so the row means the
same frequencies on every device. A bin past the audio's Nyquist frequency reads 0, which
never happens at 22,050 Hz and up, since the row stops at 11,003.5 Hz. What it doesn't
match is the dB window: a WebAudio `AnalyserNode` maps -100 to -30 dB onto its bytes. A shader
written for that reads `(dB + 100) / 70`, where `dB = -66 + 54 * value`.

Both rows are each FFT as it is, unsmoothed. A bar that falls as fast as the music does
flickers, so smooth on your own clock: keep the higher of the new value and the old one
times about 0.85 a frame at 30 frames a second, or move toward the new value at the pool's
own rates, rising at about 21 a second and falling at about 6, which is what a band at
the pool's default Response does.

### Reading it

rox writes the block the way it writes the cover: the next odd counter, then the rows,
the flags, the rate and the stamp, then the even counter after it, with a release fence
after each counter store (`Spectrum::write`). There's no line on the wire: read it
whenever you draw.

1. Read `seq`. If it's 0, nothing was written: draw silence, as for zeros with the
   playing bit clear. If it's odd, rox is writing: read again.
2. Copy what you need, from the header's flags to the rows.
3. Read `seq` again. If it moved, the copy is torn: throw it away and read again. If it's
   the same even number, the copy is one whole write.

Under rox a reader never finds 0. rox writes zeros once with the playing bit clear when
it makes the block, before any renderer is started with it, so the first counter a
renderer reads is 2 or higher (`Shared::create`, `rox/src/canvas/spectrum.rs`). The
counter never goes back to 0 either: when the last granted session ends, rox writes zeros
at the next even counter. A 0 means a block nothing has written, which only another host
can hand over.

From the odd counter to the even one a write takes rox about a microsecond: 1.1 to 1.4
microseconds at the median and under 2.1 in 99 writes of 100, in a release build on a
Ryzen 9 5950X (`write_window`, `rox-frames/src/spectrum.rs`, an ignored test that prints
it). A write comes at most every 16 ms. A read that takes `r` microseconds is torn about
once in `16,000 / (1.3 + r)` tries, and the retry after it lands well past the write, so
a retry or two finds a whole one.

`rox_frames::spectrum::SpectrumReader` does that in Rust. `read` fills a `Snapshot`, both
rows as f32 and as bytes with the counter, the stamp, the rate and the playing bit, and
answers `Whole(seq)`, `Torn` or `Empty`. It's behind the crate's `cover-reader` feature,
which rox's own build never turns on, and `SpectrumReader::open(handle)` checks the size
and header first. The layout is public there as constants: `rows::LOG`, `LOG_BINS`,
`LINEAR_BIN_HZ` and the rest.

In Python, with the fd from `ROX_SPECTRUM_FD`:

```python
import mmap, os, struct

spectrum = mmap.mmap(int(os.environ["ROX_SPECTRUM_FD"]), 0, prot=mmap.PROT_READ)
magic, version = struct.unpack_from("<4sI", spectrum, 0)
assert magic == b"RXS1" and version >= 1 and len(spectrum) >= 4096
log_bins, low, high = struct.unpack_from("<Iff", spectrum, 36)
(log_at,) = struct.unpack_from("<I", spectrum, 64)
assert (log_bins, low, high) == (128, 20.0, 20000.0)

def log_row():
    """128 values from 0 to 1, or None if rox was writing all three times."""
    for _ in range(3):
        (seq,) = struct.unpack_from("<Q", spectrum, 8)
        if seq & 1:
            continue
        row = struct.unpack_from(f"<{log_bins}f", spectrum, log_at)
        if struct.unpack_from("<Q", spectrum, 8)[0] == seq:
            return row
    return None
```

The bars example reads it the same way (`read_spectrum`, `examples/renderers/bars/bars.py`).

### When it's written

rox writes it on the player's pump, the 16 ms tick that hands the visualizers new audio,
on rox's UI thread and never on the audio thread (`follow`, `rox/src/canvas/spectrum.rs`).
That's one write for each fresh window of audio, which came to 61 a second in rox's own
measurement. It doesn't ride on the canvas's paints the way a `signals` line does, so a
renderer gets the spectrum at that rate whatever its own frame rate, and whether or not
it publishes frames. The FFT is the one the pool and a spectrum panel share, so with one
of those showing, the spectrum costs rox no FFT of its own. The audio comes from before
the volume control: at volume 0 the spectrum still moves, as the signals do.

rox writes it only while a session granted it runs. With none, a pump tick costs one
atomic load and no FFT. With music playing, each fresh window of audio is a write, quiet
or not. When the player pauses or stops, or no fresh audio has reached the visualizers
for 0.15 s (`SILENT_AFTER`), rox writes zeros once with the playing bit clear and then
leaves the block alone, so the counter stops (`Writer::step`). The first fresh audio after
that is written at once. What a renderer reads:

- Paused or stopped, or at the end of the queue: zeros with the bit clear, and a counter
  that stays put until the music goes on.
- A silent passage in a playing track: the bit set and the counter moving at the usual
  rate, with rows at or near 0. Digital silence is still audio coming through the output,
  so rox goes on writing it.
- Between two tracks of one queue: nothing changes, since the next track's first frame
  follows the last one's. If the output runs dry for longer than 0.15 s, as a slow source
  can make it, that reads as zeros with the bit clear until the next window comes.

A paused target's renderer doesn't read it, and rox goes on writing while any granted
session runs. When the last one ends, rox leaves zeros behind with the playing bit clear,
so the next renderer to start never reads old music as playing.

The handle a renderer gets can only read, on the cover's terms: a `memfd` named
`rox-spectrum` with `F_SEAL_FUTURE_WRITE` on Linux 5.1 and later, a second read-only open
of the shm object on macOS, and a section duplicated with map read alone on Windows
(`create_readable`, `rox-frames/src/map/unix.rs` and `windows.rs`). The macOS and Windows
halves have never been compiled or run. On a Linux before 5.1 a renderer could write the
numbers every other renderer reads. rox never reads them back.

## The wire

### rox to the renderer

One JSON object a line on stdin, UTF-8. Read whole lines of any length: a pool line can
run past a pipe's atomic write size, and rox finishes a line it wrote in part before it
starts the next. Read each line as JSON. Key order and number spelling aren't part of
the contract, and a line or key a renderer doesn't know should be skipped.

These are every line rox writes (`launch` in `rox-services/src/renderers.rs` for
`hello`; `pause_line`, `pool_line`, `signals_line`, `Pointer::line`, `palette_line`,
`cover_line`, `size_line` and `settings_line` in `target.rs`):

```json
{"hello":1,"width":1280,"height":720,"handle":0}
{"hello":1,"width":1008,"height":560,"handle":0,"frames":{"width":1920,"height":1080}}
{"hello":1,"width":1008,"height":560,"handle":0,"frames":{"width":1920,"height":1080},"settings":{"look":"rainbow","rate":60}}
{"hello":1,"width":1280,"height":720,"handle":0,"cover":{"fd":4,"width":256,"height":256}}
{"hello":1,"width":1280,"height":720,"handle":0,"spectrum":{"fd":4,"bytes":4096,"log_bins":128,"linear_bins":512,"rate":62.5}}
{"hello":1,"width":1920,"height":1080,"handle":0,"texture":{"fd":4,"width":1920,"height":1080,"allocation_size":8847360,"memory_type":0,"handle_type":"opaque-fd","format":37,"view_formats":[43,37],"flags":8,"usage":31,"tiling":0,"layout":1,"device_uuid":"00000000100000000000000000000000","driver_uuid":"414d442d4d4553412d44525600000000"}}
{"pool":[{"id":1,"name":"Kick","kind":"band","low":30,"high":120},{"id":2,"name":"onset 30-120 Hz","kind":"onset","low":30,"high":120},{"id":3,"name":"level","kind":"level"},{"id":4,"name":"total of 1","kind":"aggregate"}]}
{"signals":[0.3958,0.0,1.0],"time":4.224,"playing":true}
{"pointer":{"kind":"move","x":0.27,"y":0.36}}
{"palette":{"background":"#121212","surface":"#181818","text":"#c0c0c0","muted":"#808080","accent":"#ffb300","highlight":"#facc15","dark":true,"stage":"#121212"}}
{"cover":2}
{"cover":null}
{"size":{"width":1600,"height":896}}
{"settings":{"look":"ember","rate":60}}
{"pause":true}
```

`hello` is always first. Its `width` and `height` are the size to draw at, and `handle`
is the mapping's handle value on Windows and 0 elsewhere. A renderer whose manifest says
`resizes` finds the mapping's own size under `frames`, the same as
`ROX_FRAMES_WIDTH` and `ROX_FRAMES_HEIGHT`: the most it can draw at. One that doesn't
gets no `frames` key, and its `width` and `height` are the mapping's. A renderer whose
manifest declares settings finds every one it declares under `settings`, each with the
value the target holds or its default ([Settings](#settings)). One that declares none
gets no `settings` key. A renderer granted the cover finds it
under `cover`: `fd` 4 on Unix, `handle` on Windows, the section's handle value in the
renderer, and the square's `width` and `height`, both 256. Without the grant `hello` has no
`cover` key. A renderer lent the GPU texture finds what it needs to import it under
`texture` ([hello's texture](#hellos-texture)). One that wasn't lent it, for any reason,
gets no `texture` key. A renderer granted the spectrum finds it under `spectrum`: `fd`,
the same number as `ROX_SPECTRUM_FD` on Unix, or `handle` on Windows, then the block's
`bytes`, the two rows' bin counts and the most writes a second, each also in [the
block's header](#the-spectrum). Without the grant `hello` has no `spectrum` key.

`pool` describes the signal pool by id, in pool order. A name is the user's, trimmed
and cut at 64 characters, or one worded in English from the signal's source: `band
30-120 Hz`, `onset 30-120 Hz`, `beat 30-120 Hz`, `trigger 30-120 Hz`, `level`, or
`total of <id>` (`wire_name`, `target.rs`). Read what a signal is from `kind`, since
a renamed one says nothing about it in its name: `band`, `level`, `onset`, `beat`,
`trigger` or `aggregate`, a running total of another signal that wraps at 1
(`wire_signal`, `target.rs`). A `band`, `onset`, `beat` or `trigger` also has `low`
and `high`, the frequency range it listens to in whole hertz, kept between 20 and 20,000
(`Source::band`, `rox-viz/src/signal.rs`). Past the name an entry holds only those
fixed words and numbers, and nothing about how rox shapes the value. The line comes once
the session runs and the canvas draws, and again whenever the pool changes. An empty pool
is `{"pool":[]}`.

`signals` holds each pool signal's value in pool order, from 0 to 1, to 4 decimals, the
value routes read (`signals_line`, `target.rs`). `time` is the playing position in
seconds, to 3 decimals, and 0 with nothing playing. `playing` is `true` while playback
runs and `false` once it's paused or stopped. A line goes with each frame the canvas
paints while music plays. When playback pauses or stops, lines keep coming for about a
second while the signals settle, each with `"playing":false`, then none come until music
plays again (`Reading::read`, `target.rs`). A target that rests with the music is sent
one line with `"playing":false` before its `{"pause":true}`, so the last line a renderer
holds always says whether music was playing. The canvas paints when a renderer publishes
a frame, so a renderer that stops drawing stops getting signals. A reading rox hasn't
sent yet is replaced by a newer one.

`pointer` is the pointer over the target's surface (`Pointer::line`, `target.rs`):

| `kind` | Fields |
|---|---|
| `move` | `x`, `y` |
| `down`, `up` | `x`, `y`, `button`: `left`, `right`, `middle`, `back` or `forward` |
| `scroll` | `x`, `y`, `dx`, `dy` in lines, 20 pixels a line for a wheel that reports pixels |
| `leave` | none |

`x` and `y` run from 0 to 1 across the surface from its top-left, to 4 decimals, mapped
back through the item's tilt the way it's drawn. rox sends them only outside design
mode, and only while the target is the front item under the pointer with no On Click
command rox can run on it or on a group around it (`hit`, `rox/src/panels/canvas.rs`). Moves
go at most every 8 ms with the newest position. At most 64 events wait for a renderer
that's behind: moves are dropped first, then scrolls, then leaves, and presses last
(`Outbox`, `target.rs`). `leave` comes when the pointer goes to another item or off
the canvas, and when design mode comes on, and it ends any press. An `up` can come with
no `down` before it, from a press that started elsewhere.

`palette` goes only to a renderer granted it: the theme's colours as `#rrggbb`, under
fixed names, and whether the theme is dark (`palette_line`, `target.rs`):

| Key | The theme's |
|---|---|
| `background` | Window background, the colour of the target's window |
| `surface` | Panel surface |
| `text` | Body text |
| `muted` | Muted text |
| `accent` | Accent |
| `highlight` | The mark drawn over accent fills. With song theming on it's the cover's second colour |
| `dark` | `true` when the background wants light text over it |
| `art_primary`, `art_secondary` | The cover's two colours, only while song theming is on and the cover has them |
| `stage` | The solid colour around the canvas's items, only when there is one (below) |

`stage` is what shows behind and around the canvas's items where none covers the stage,
so a renderer can fill its background with it and blend into what surrounds the target.
A canvas has no background of its own: what shows is what its container paints under it.
For a canvas held directly in a tab group, that's the theme's background as the widget
theme has it, which follows the focused window's colours and can differ from
`background` in a window that isn't focused (`theme_target`, `rox-design/src/palette.rs`).
rox sends `stage` only when it's one solid colour, and leaves it out:

- while Surface Opacity is under 100%, since the tab group's background is then
  transparent and the backdrop shows through: the cover, the backdrop shader, or the
  window's own transparency;
- for a popped-out canvas, and a canvas inside a Group, Overlay, Slide or Drawer, which
  sit on surfaces rox doesn't name.

A Renderer panel sends `stage` under the first rule alone, popped out too, since it paints
the background around its picture itself ([The Renderer panel](#the-renderer-panel)).

Another item behind the target isn't the stage either. With no `stage`, a renderer that
wants to blend in has to stay see-through: alpha below 255 shows whatever is behind it.

The colours are the theme's as the target's window shows it once a change has eased in,
so a theme that eases over a third of a second sends one line, not one a frame. The line
comes once the session runs and the canvas draws, then again whenever a colour changes:
a theme switch, song theming switched on or off, Surface Opacity crossing 100%, and with
song theming on, a track whose cover gives other colours. It rides on the redraw the theme's own change already makes,
and asks for none. With song theming on, the palette changing tells a renderer that the
album changed, though not which album.

`cover` goes only to a renderer granted it, after rox writes the cover mapping: the even
counter of that write, or `null` when nothing plays or the track has no cover and the
square is transparent (`cover_line`, `target.rs`). It comes when the session
starts, then each time the playing track changes, between frames, without a redraw. The
gap between two tracks of one queue doesn't count: the last cover holds, as the
backdrop's does.

`size` goes only to a renderer whose manifest says `resizes`, when the box it follows
settles at another size ([The size](#the-size)): draw at this width and height from the
next frame on. It's never more than `frames`. A frame already drawn at the size before
it still shows, and one drawn at any older size is refused. Paused, it waits for the
resume. A renderer that doesn't resize is never sent one.

`settings` goes only to a renderer that declares settings, when the user changes one
whose `apply` is `line`. It carries the whole set every time, every declared key with
its value, and nothing else. A change to a `restart` setting starts the session again
instead, with the new value in `hello`. Paused, it waits for the resume.

`pause` is `true` when nothing shows the target, or while it rests with the music, and
`false` once it's shown and not resting again.
While paused, the palette and the cover wait too: rox writes nothing into the cover
mapping.

The resume sends `{"pause":false}`, then a `size` line if the box settled at another
size and a `settings` line if a setting changed meanwhile, and nothing else of its own.
What follows comes with the canvas's next paint of the target, as it would have without
the pause: the `pool`
line if the pool changed while it was paused, ahead of anything else, then the
`palette` line if a colour changed, the cover written again with its `cover` line if the
track changed, and `signals` lines again once music plays (`Outbox` and `Source::feed`,
`target.rs`). A pool, palette or cover that didn't change isn't sent again, and neither
is `hello`. Pointer events and a `signals` line from playing music that were waiting when
the pause came are dropped, since they're stale by the resume. A target that rests with the music wakes
as playback goes on, and its next paint names the pool as it is then, so a renderer never
goes on with a pool edited during the pause.

Closing stdin tells the renderer to exit.

### The pool

The pool is the user's, and there's one for the whole app. It's edited under
Application > Signals and saved with the live look in `workspace.json` in rox's data
folder, as `bundle.signals` (`LookState` and `WorkspaceBundle`,
`rox-core/src/settings.rs`). Every canvas and every render target reads that one pool,
and applying a workspace replaces it with the one the workspace carries. A renderer can't
add a signal, since nothing on its wire goes that way. Its manifest can ask for some
(see [What a renderer asks for](#what-a-renderer-asks-for)), and the user says whether
they go in.

When the user adds a render target in design mode for an approved renderer whose wanted
signals aren't all in the pool, rox asks over the canvas: "Add the signals Pulse
follows?", the ones the pool lacks listed by name and kind, with Not Now and Add
(`offer_signals`, `rox/src/panels/canvas.rs`). Add wears the accent, as the Place
confirm for an object does, since adding is undone by deleting the signals in the Signals
window. Escape and Not Now leave the pool as it was. While any are still missing, the
target's inspector shows a Signals row, "Not in the pool yet:" and the list, with an Add
Signals button. Both add them the way an object's signals are merged: fresh ids, and a
name already taken gets a number after it (`add_wanted_signals`, `rox/src/panels/canvas.rs`).

What's missing is worked out by kind and band, never by name: a wanted `band` from 30 to
120 Hz is answered by any pool band from 30 to 120 Hz, whatever the user called it, with
the band rounded to whole hertz as the pool line carries it. An `aggregate` is answered by
a total of whatever answers its input (`missing_signals` and `signals_to_add`,
`rox-services/src/renderers.rs`). Nothing is added outside design mode, nothing
is offered for a renderer that isn't approved as it stands, and nothing is added without
the click. A renderer still meets the pool the user made, so it reads what it got by
`kind` and band from the pool line, as below.

A renderer meets pools it wasn't written for, then. With an empty pool it gets
`{"pool":[]}` and `signals` lines with no values, and it should still draw something:
the bars example draws one empty bar, and plasma turns at its resting speed. Choose what
to follow by `kind` and band. A name is the user's to change, and an id means nothing
outside the pool it came in. Leave a `kind` the renderer doesn't know out of whatever it
works out from the rest, and expect the pool line again whenever the user edits it.

### The signal values

Each value is the signal's output once rox has shaped it, the number a route reads
(`SignalHub::value` and `Signals::step`, `rox-viz/src/signal.rs`). The signal's Response
has already smoothed it, so a renderer that smooths again lags behind everything else
following that signal. Its Threshold has already gated it too: under the threshold it
reads 0, and above it the output climbs from 0 again. On a trigger and a beat the
threshold is a level the band has to reach to fire instead.

The numbers below are the ones a renderer needs to tell what it's looking at, or to
imitate a kind on audio of its own. Response and Threshold are the Signals window's 0 to
100, read as 0 to 1. Every rate is per second and is an exponential approach, not a
linear ramp: each step moves the value toward where it's going by `rate * dt` of the
distance left, at most all of it, with `dt` the seconds since the hub last stepped, at
most 0.1. A
rate that Response sets runs from its value at Response 0 to its value at 100
geometrically, `fast * (slow / fast) ^ (response / 100)`, so Response 50 on a band's rise
is about 12 a second, not 26.5.

Every kind but a total reads one FFT, the hub's (`Hub::advance` and
`Signals::step`, `rox-viz/src/signal.rs`; `Analyzer`, `rox-viz/src/analysis.rs`;
`AudioFeed::magnitudes`, `rox-viz/src/feed.rs`):

- The audio is the output's stereo, taken before the volume control and after the DSP
  chain, folded to mono as `(left + right) / 2`.
- It runs at the output device's sample rate, not the file's. An FFT bin is that rate
  over 2048: 23.44 Hz at 48 kHz, 21.53 Hz at 44.1 kHz.
- The window is a symmetric Hann over the newest 2048 frames,
  `0.5 - 0.5 * cos(2 * pi * i / 2047)`.
- There's no fixed hop. A new FFT is taken whenever the player has handed the
  visualizers new audio, on its 16 ms pump, over whatever the newest 2048 frames are
  then. At 48 kHz that's about 768 new frames a window, so successive windows overlap by
  about 60%, and the pump's timing makes the step uneven.
- The magnitudes are scaled by `2 / sum(window)`, the window's coherent gain, so a
  full-scale sine centred on a bin reads 1.0, 0 dB. One between two bins reads up to
  1.4 dB lower, the Hann window's scalloping.
- A band reads the FFT bins whose centres fall in `[low, high)`: bin `k`, centred on
  `k * width` with `width` the bin width above, from `ceil(low / width)` up to but not
  including `ceil(high / width)`, and never fewer than one bin (`band_bins`,
  `rox-viz/src/analysis.rs`). That's the rule the log row's peaks use too. At 48 kHz a
  30 to 120 Hz band reads bins 2 to 5, 46.9 to 117.2 Hz. A tone just under `high` whose
  nearest bin centre sits just over it reads off the bin below, up to about 6 dB under
  its level, the Hann window's fall one bin out.

- `band` is the loudest bin of a 2048-point FFT of the mono mix between `low` and `high`,
  a peak and not a mean (`HUB_FFT`). It's in decibels against a full-scale sine, mapped
  from -66 dB at 0 to -12 dB at 1 and held between them (`FLOOR_DB` and `MAX_DB`). It
  rises toward that reading at 50 a second at Response 0 down to 3 at 100, and falls at
  12 down to 1 (`ATTACK_FAST`, `ATTACK_SLOW`, `RELEASE_FAST`, `RELEASE_SLOW`).
- `level` is the same measure across the whole spectrum, 20 Hz to 20 kHz.
- `onset` is a pulse. It reads the band's reading before any smoothing, never the smoothed
  `band` value, and keeps a slow average of that reading, which rises toward it at 2.5 a
  second and falls at 2 (`ONSET_REF_ATTACK`, `ONSET_REF_RELEASE`). When the reading is
  above 0.15 and more than 0.12 above the average, the value goes to 1 (`ONSET_FLOOR`,
  `ONSET_MARGIN`). The test comes before the average moves, so a jump counts whole. It
  then decays toward 0 at 16 a second at Response 0 down to 1.5 at 100 (`ONSET_DECAY_FAST`,
  `ONSET_DECAY_SLOW`). At 60 frames a second that's about an eighth of a second to fall to
  a tenth at Response 0, and about a second and a half at 100. A hit reads as a jump and a
  tail over many frames, never a single 1 between zeroes. It fires again only once the
  reading has fallen under the average plus 0.06, half the jump.
- `beat` is a pulse with the onset's decay, for music an onset goes quiet on. It reads
  the band before any smoothing, and without the top of the window: past -12 dB the
  reading goes on above 1, so a kick on a loud master whose bass pins the `band` value at
  1 still climbs. Each reading's climb is how far it sits above a running average of the
  readings, which moves toward each one at 10 a second (`BEAT_AVERAGE`), and 0 when the
  reading is below it. The usual climb is an average of the climbs that moves at 0.4 a
  second (`BEAT_USUAL`), counted as no less than 0.006 (`BEAT_USUAL_FLOOR`). The beat
  also holds the band's peak, which falls at 0.25 a second (`BEAT_PEAK_RELEASE`) and takes
  each reading one reading late, so a kick rising across two readings counts whole. A
  reading fires when its climb is at least twice the usual climb (`BEAT_JUMP`), it tops
  the held peak by at least 0.05 (`BEAT_OVER`), and it's at least 0.35, or at least the
  Threshold when that's higher (`BEAT_FLOOR`). The average, the usual climb and the peak
  are tested before they move, as an onset's average is, and move by the time since the
  previous reading rather than the previous step, at most 0.1 s (`BEAT_GAP_MAX`). After a
  beat it can't fire again for 0.125 s, so never more than 8 times a second
  (`BEAT_REFRACTORY`), and not until a climb falls under 0.9 of the fire level
  (`BEAT_REARM`). The value then decays exactly as an onset's does, at the rate Response
  sets. A kick that rings through the beat still climbs well over where the band sat a
  tenth of a second before, and on dense music that's several times the usual climb, so
  a beat fires on each one. A held tone climbs once, at its start, and two bass notes
  beating against each other crest at one height, which rarely tops the held peak. The
  Threshold gates nothing: the output is the pulse, at 1 on the reading it fires.
- `trigger` has the same pulse and the same decay, and also reads the band before any
  smoothing. It fires when the reading reaches the Threshold, and can't fire again until
  the reading falls under three quarters of it (`TRIGGER_REARM`). At Threshold 0 it never
  fires and reads 0. Its Threshold gates nothing.
- `aggregate`, Total in the Signals window, adds up another signal: each step it grows by
  that signal's output times its Rate times `dt`, and wraps from 1 to 0. The Rate is held
  between 0 and 8 (`AGGREGATE_RATE_MAX`), so a total laps at most 8 times a second. A total
  of another total can read it a step late, depending on their order in the pool. A
  renderer using it as a
  phase has to unwrap it. When a value is more than half below the one before, it
  wrapped, so add 1. A smaller fall is a drain to 0, from Reset on Track at a new song or
  from Flush, at 8 a second until it's under 0.002 (`FLUSH_DRAIN`, `FLUSH_DONE`), so it
  takes a moment and doesn't snap.

The gate is a smoothstep. With the threshold `t`, `x = (value - t) / (1 - t)` held
between 0 and 1, and the output is `x * x * (3 - 2 * x)`. A threshold of 0 passes the
value as it is. The gate applies to every kind but the trigger and the beat, a total
included.

Once no fresh audio has reached the visualizers for 0.15 s (`SILENT_AFTER`), as when
playback pauses or stops, a band and the level fall to 0 at their falling rate, an onset's average falls at 2 a second and it re-arms, a
beat's average falls at 10 a second and it re-arms, a trigger re-arms, and a pulse
finishes its decay. Between two readings of the audio a band
holds its value: the hub only takes a new FFT when the player has handed the visualizers
new audio, every 16 ms (`PUMP_INTERVAL`, `rox-services/src/player.rs`).

Plan for what music does to these. A loud master peaks above -12 dB in its bass much of
the time, so a band there pins near 1 for minutes and moves little: give it a narrower
band, a higher Threshold, or read it as a rough level rather than a beat. An onset needs
the reading to drop back near its average between hits. A kick that rings through most
of a beat keeps the band up, the average climbs to meet it, and the next kick jumps less
than 0.12 over it, so a rolling or sustained kick rarely fires an onset. A beat on the
same band fires on that music, once a kick, and a trigger with a Threshold under the
kick's peak and over its tail does too.

### The renderer to rox

A renderer writes announce lines to stdout and nothing else: `f`, a space, the frame number in
decimal digits, a space, the slot `0` or `1`, and `\n`. A renderer lent the GPU texture
can also write `t`, a space, the frame number and `\n`, for a frame it drew in the
whole texture, or `t <n> <width> <height>\n` for one it drew in the texture's top-left
`width` by `height`. A renderer that resizes has to name the size: rox can't read one off
GPU memory, and the rest of the texture holds whatever was drawn there before. A line is
at most 64 bytes, newline included, and `\r\n` is refused (`ANNOUNCE_MAX`,
`parse_announce`, `parse_texture_announce` and `parse_sized_texture_announce`,
`rox-frames/src/lib.rs`).
Any other line, or a longer one, is dropped without a log line, and still counts toward
the flood rule below, as `t` lines do. Of the lines waiting when rox reads, only the
newest announce is acted on, `f` or `t` (`read`, `target.rs`).

stderr goes to rox's log a line at a time as `renderer <id>: <line>`, cut at 4 KiB
(`drain_stderr` and `STDERR_LINE`, `rox-plugins/src/process.rs`).

## Limits

| | Limit | Where |
|---|---|---|
| Resolution | 16 to 3840 by 16 to 2160 pixels | `WIDTHS`, `HEIGHTS`, `renderers.rs`; `WIDTH`, `HEIGHT`, `layout.rs` |
| A following size | The box's device pixels times 50, 75, 100, 150 or 200%, in steps of 16, inside the ceiling | `SCALES`, `SIZE_STEP`, `follow_size`, `target.rs` |
| The ceiling | The largest display's pixels times the scale, at most 3840 by 2160 | `ceiling`, `target.rs` |
| A resize | Told once the box's size has held 250 ms | `SETTLE`, `target.rs` |
| Settings | 32, each enum with 1 to 32 choices; a key of 32 bytes, a title or label of 48 characters, a choice value of 64 bytes | `MAX_SETTINGS`, `MAX_CHOICES`, `KEY_MAX`, `TEXT_MAX`, `VALUE_MAX`, `renderer_settings.rs` |
| An announce line | 64 bytes, newline included | `ANNOUNCE_MAX`, `lib.rs` |
| stdout lines | More than 500 in a second, two seconds in a row, stops the renderer | `LIMITS` and `Rate`, `target.rs` |
| Refused frames | 60 in a row stop the renderer | `LIMITS.refused_run` |
| An untaken frame | 500 ms, then the renderer is paused | `LIMITS.idle` |
| Restarts | 0, 1, 2, 4 s; the fifth exit in ten minutes stops it | `BACKOFF`, `host.rs` |
| Stop | 200 ms after stdin closes, then killed | `GRACE`, `renderers.rs` |
| A pool name | 64 characters | `NAME_MAX`, `target.rs` |
| Pointer events waiting | 64; moves 8 ms apart | `POINTER_QUEUE`, `MOVE_GAP`, `target.rs` |
| A stderr line | 4 KiB | `STDERR_LINE`, `process.rs` |
| Wanted signals | 16 | `MAX_WANTED_SIGNALS`, `manifest.rs` |
| A wanted signal's name | 32 characters | `WANTED_NAME_MAX` |
| The cover | 256 by 256 pixels | `SIZE`, `rox-frames/src/cover.rs` |
| The spectrum | 128 log bins and 512 linear bins, at most 62.5 writes a second | `LOG_BINS`, `LINEAR_BINS`, `RATE`, `rox-frames/src/spectrum.rs` |
| The GPU texture | One per target, at its mapping's size; Linux only; the canvas's wait to make it is 2 s, then the session runs on slots | `LIMITS.texture_wait`, `target.rs` |
| `t` from a session lent no texture | Counts as a refused frame: 60 in a row stop the renderer | `show_texture`, `target.rs` |

The flood rule counts lines in one-second spans from the first line, and stops a
renderer whose current span passes 500 right after one that did. A renderer that
announces once a frame is far inside it at any display rate.

## What a renderer is sent

`hello`, `pause`, `pool`, `signals` with `time` and `playing`, and `pointer`, and nothing
else unless its approved manifest asks. The enable card names the two grants behind them: "Gets the
music's signals and the playing position" and "Gets the pointer while it's over the
render target" (`plugins::grants`, `rox-services/src/plugins.rs`). Whether music plays is
part of what the playing position says, so `playing` comes under the first. A renderer
that asks
for the palette also gets `palette` lines, and one that asks for the cover gets the cover
mapping, fd 4 and `ROX_COVER_FD`, `cover` in `hello`, and `cover` lines. One that asks
for the spectrum gets the spectrum mapping, its fd and `ROX_SPECTRUM_FD`, and `spectrum`
in `hello`. Each has its own card line. The grants are read from the manifest the gate approved when the session
starts (`Session::asks`, `rox-services/src/renderers.rs`), so a renderer whose files
change can't widen them without the card. A renderer that asks for nothing gets exactly
what it got before these grants existed.

A renderer that says `resizes` also gets `frames` in `hello` and `size` lines, and one
that declares settings gets `settings` in `hello` and `settings` lines. Neither is a
grant and neither has a card line: a size is its own target's, which it already draws,
and a setting's value is one the user picked for it from the choices it declared.

A renderer isn't told the track, a path, a key press, or anything about another item,
and its wire has no way to call into rox. The cover is pixels with a counter.

The spectrum says how loud each frequency was, about 61 times a second, and nothing else
about the audio. From it a renderer can tell the tempo, the loudness and the timbre, and
could match it against fingerprints of known recordings to guess the track. It can't get
the recording back. There's no phase, nothing above 11 kHz in the linear row, nothing
outside the 54 dB window, and the windows come at the pump's uneven pace. What a hostile
renderer could synthesize from it is a rough sketch of the music, the way a vocoder
sounds: a melody and a rhythm someone could recognize, never a copy of the sound. The
palette says something about the track too: with song theming on, the theme's colours
follow the playing track's cover. A renderer granted the palette gets a new `palette`
line whenever a cover brings other colours, which tells it the album changed, and gets
that cover's own two colours as `art_primary` and `art_secondary`. It isn't told which
album. Its card line says so: the theme's colours follow the cover when song theming is
on. On Linux it runs in [the sandbox](#the-sandbox) unless its manifest asks to run
unconfined, and elsewhere with the user's permissions, and the card says which.

A manifest that gains a grant is a changed manifest. The renderer switches off, and its
card lists the new grant under "Changed since you last switched it on" (`Change::Grant`).
Asking for other signals than before counts the same way.

A workspace or an object with a render target lists the renderer in `requires`, the way
it lists a plugin, and never includes it (`named_renderers`, `target.rs`). An
exported object carries the pool signals its routes read, and the ones those totals
follow, but not the pool a render target's renderer reads (`lift`, `rox/src/objects.rs`).
Placed in another rox, the renderer reads that rox's pool.

## The sandbox

On Linux a renderer runs confined unless its manifest asks to run unconfined
([ADR 38](../02-architecture/decisions/38-adr-renderer-sandbox.md)). rox applies the
sandbox in the child between fork and exec, so the renderer and everything it starts
run inside it from their first instruction: no_new_privs, a seccomp filter, then a
Landlock ruleset (`Prepared::install`, `rox-plugins/src/sandbox.rs`). There's nothing to
link and nothing to call. A renderer finds out it's confined when an open or a socket
fails with `EACCES` or `EPERM`.

### What a confined renderer reaches

| | Reaches | Why |
|---|---|---|
| Its package folder | Read and run | Its own code. Never written: a write would change its hash |
| Its cache folder | Read and write, nothing in it runs | See [The cache folder](#the-cache-folder) |
| The interpreter its entry resolves to | Read and run its install | Only when it's outside the system's folders and the package: see [The interpreter](#the-interpreter) |
| `/nix/store`, `/run/opengl-driver`, `/usr`, `/lib`, `/lib64`, `/lib32`, `/libx32`, `/bin`, `/sbin`, `/opt`, `/app` | Read and run | The system's libraries and programs, wherever the distro keeps them. NixOS keeps them in the store and names the GPU driver at `/run/opengl-driver`; a Flatpak runtime under `/usr` and `/app` |
| `/etc/ld.so.cache`, `ld.so.preload`, `ld.so.conf` and `ld.so.conf.d`, `localtime`, `vulkan`, `glvnd`, `egl`, `drirc`, `OpenCL`, `fonts` | Read | What the loader, the Vulkan loader, Mesa and fontconfig read. Not `passwd`, `machine-id` or `hostname` |
| `/sys/devices/system/cpu`, `/dev/urandom`, `/dev/random` | Read | Core counts for thread pools, and randomness |
| `/dev/null`, `/dev/zero`, `/dev/full` | Read, write, ioctl | A shell redirect and most runtimes open them |
| Each `/dev/dri/renderD*` | Read, write, ioctl | The GPU. Without ioctl a Vulkan driver falls back to software |
| `/dev/dri` | Listed | How libdrm finds the render nodes. The card nodes aren't opened |
| The sysfs folder of each render node's PCI device | Read | Where Mesa and libdrm read the GPU's vendor and bus |
| `/dev/nvidia*`, `/proc/driver/nvidia` | Read and write; read | NVIDIA's proprietary driver. Listed when present, and never run on one |
| The handles rox lends | Whatever they are | Inherited, never opened by path, so the rules don't reach them |

Everything else is refused: the user's home, rox's data folder, another package's
folder, `/tmp`, `/dev/shm`, `/run`, and all of `/proc`, the renderer's own entries
included. A path that isn't on this system is skipped. A rule follows symlinks, so it
lands on what the path names. The paths are opened, the rules added and the filter
built before the fork, and the child makes three system calls on them.

The seccomp filter answers `socket(2)` with `EACCES` for every family, so there's no
TCP, no UDP, no netlink and no unix socket, by path or by abstract name. The session bus
and the display server are unix sockets, so neither is reachable. `io_uring_setup`
answers `EPERM`, since a ring could make a socket past the filter, and so do `keyctl`,
`add_key` and `request_key`, since the user's session keyring isn't a file. A system
call from another architecture's table kills the renderer.

Landlock also refuses a TCP bind or connect (ABI 4), a connect to an abstract unix
socket made outside the sandbox, and a signal to any process outside it, rox included
(ABI 6). `ptrace` of rox, and with it `process_vm_readv`, is refused at every ABI.

The display, session bus and runtime folder variables are dropped from the
environment: `WAYLAND_DISPLAY`, `DISPLAY`, `XAUTHORITY`, `DBUS_SESSION_BUS_ADDRESS` and
`XDG_RUNTIME_DIR`, with `TEMP` and `TMP` (`Profile::environment`). The rules would stop
them anyway, and a library that sees them tries and logs a failure.

A renderer's log line says what it got:

```
renderer pulse: sandboxed at Landlock ABI 7: Run /…/plugins/pulse, Run /nix/store, …
renderer pulse: started at 1280x720, lent a texture, sandboxed
```

### The cache folder

`<data>/renderer-cache/<id>`, made before every start, outside the package so a write
there never changes its hash. Its path is `ROX_CACHE_DIR`, in every tier and on every
platform, so a renderer written for the sandbox runs the same outside it. For a
sandboxed renderer rox also points what programs write on their own there, `HOME`
included: a sandboxed renderer's home is its cache folder, and the user's home isn't
named anywhere in its environment.

| Variable | Value |
|---|---|
| `HOME` | The cache folder |
| `XDG_CACHE_HOME`, `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_STATE_HOME` | `cache`, `config`, `data` and `state` under it |
| `TMPDIR` | `tmp` under it, emptied at each start |
| `MESA_SHADER_CACHE_DIR` | `cache/mesa_shader_cache` under it |
| `__GL_SHADER_DISK_CACHE_PATH` | `cache/nvidia` under it |
| `DENO_DIR` | `cache/deno` under it |

So the GPU driver's shader cache is the renderer's own, and starts cold the first time
it runs. Nothing in the folder runs: a renderer can't write a program there and start
it, though an interpreter can still read a script from it. Removing the renderer on
the Renderers page deletes the folder, after its sessions are stopped (`remove` and
`remove_cache`, `rox-services/src/plugins.rs`).

### The interpreter

A native renderer runs from its package. A script renderer runs on the program its entry
resolves to ([The package](#the-package)), and in the sandbox that program has to be
reachable. One in the system's folders above, or in the package's `bin`, always is. One
anywhere else, as under the user's home, is granted its install and nothing more
(`interpreter_install`, `rox-plugins/src/sandbox.rs`):

- the file the program resolves to, read and run;
- its install's `bin` and `lib`, read and run, when the program sits in a folder named
  `bin` whose parent has a `lib` holding something named for the program, as
  `lib/python3.12` is for `bin/python3`. A venv's `pyvenv.cfg` beside its `bin` comes too,
  since Python reads it to find the venv's packages.

The path as found and every path its symlinks lead through are each looked at. A venv's
`bin/python` is a link to the Python it was made from, so a venv under home over a pyenv
Python under home grants both installs, and a venv over the system's Python grants the
venv alone. Neither an install nor any folder in it is granted when it's the user's home,
a folder above home, or a folder holding or inside rox's data folder, the rule that holds
a read ask ([The asks](#the-asks)). A Python in `~/.local/bin` with packages in
`~/.local/lib` gets its file alone, since `~/.local` holds rox's data folder.

| Interpreter | Granted |
|---|---|
| A `python3` or `deno` in the Nix store, `/usr` or another system folder | Nothing more |
| `~/.deno/bin/deno`, a single binary | That file |
| `~/.pyenv/versions/3.12.1/bin/python3` | That install's `bin` and `lib` |
| `~/project/.venv/bin/python`, over that pyenv | The venv's `bin`, `lib` and `pyvenv.cfg`, and the pyenv install's `bin` and `lib` |
| `~/.pyenv/shims/python3` | That file. A shim is a script that runs pyenv from `~/.pyenv/libexec`, which is refused, so list the version's own `bin` under Program Folders instead |

The Program Folders as a whole are never granted. They're where rox looks for an
interpreter, and one can be any folder the user has, their home included, so granting
them would hand over whatever else they hold. A file beside an install is still refused:
a renderer on `~/.pyenv/versions/3.12.1/bin/python3` can't read
`~/.pyenv/versions/notes.txt`.

The paths granted are part of the tier. The card names them in one line, "Can read and
run its interpreter from outside the system's folders:" and the paths, and the approval
records them. When the entry resolves to an interpreter at another path, as after a
change to Program Folders or PATH, the renderer switches off, and its card asks again
with "Its interpreter is at another path than when you last switched it on." A start
whose program resolved somewhere else since the gate passed gets only the approved
paths, and doesn't run.

### The asks

A renderer asks for more under `capabilities.renderer`. Each ask is a grant: its card
names it, and a manifest that adds one is a changed manifest that switches the renderer
off until the user approves it again.

```json
"renderer": {
  "label": "Gallery",
  "access": { "network": true, "read": ["library", "pictures"] }
}
```

| Key | What it reaches | The card says |
|---|---|---|
| `access.network` | TCP and UDP sockets to any address, this machine's included. Name lookups and TLS read `/etc/resolv.conf`, `hosts`, `nsswitch.conf`, `ssl` and the rest | Can use the network: it can connect to any computer, on the internet, on your local network or this one. It still can't reach your desktop or its session bus. |
| `access.read: ["library"]` | Every folder in rox's library, read-only | Can read the files in your library's folders, without changing them. |
| `access.read: ["music", "pictures", "videos", "documents", "downloads"]` | The user's standard folders as XDG names them, read-only | Can read these folders of yours, without changing them: Pictures. |
| `unconfined: true` | Everything the user can, as before the sandbox | See [The tiers](#the-tiers) |

`network` is a bool, `read` a list of names, and anything else refuses the manifest. A
name rox doesn't know is skipped, so a renderer written for a later rox loads with what
this one can name, and its card names only that. A renderer that asks for `unconfined`
can't also ask under `access`.

The network ask is the internet families and nothing else. The filter still answers
`EACCES` for a unix socket, so the session bus, the display server, a local database on
a unix socket and systemd's resolver socket stay out of reach, and a name lookup goes
through `/etc/resolv.conf`. Any service on this machine that listens on a TCP or UDP
port is reachable, as it is to any program the user runs.

The library is read when the renderer starts. A folder added to the library while it
runs reaches it at its next start. A folder removed stays readable until then, since
Landlock adds rules and never takes one back. A folder that would hold more than the
card says is never granted, with a log line: the user's home, anything above it, and
anything holding or inside rox's data folder. An XDG folder set to home itself, which
`xdg-user-dirs` does when the folder is missing, is one of those.

### The tiers

What a renderer runs as is its tier (`Tier`, `sandbox.rs`). Its card reads it out, the
Renderers page marks it beside the version, and the approval records it.

| Tier | When | The card says |
|---|---|---|
| Sandboxed | Linux with Landlock and seccomp, and no `unconfined` | Runs in a sandbox. It can use the graphics card and its own cache, and can't read your files or reach the network. With an ask: Runs in a sandbox. It can use the graphics card and its own cache, and reaches only what's listed below. |
| Unconfined | The manifest says `"unconfined": true` | Asks to run outside the sandbox, as a program on this computer with your permissions. It can read and change your files, use the network, and reach your desktop and the other programs you run. |
| Your permissions | macOS and Windows | It runs as a program on this computer with your permissions. rox has no sandbox for renderers on this system. |
| Your permissions | Linux that can't sandbox: no Landlock, Landlock off, no seccomp, or applying it failed this session | rox can't sandbox it on this system: the kernel has no Landlock. If you switch it on, it runs as a program with your permissions. (or: Landlock is switched off in the kernel; the kernel can't filter what a program asks of it; rox couldn't sandbox it: the reason, until rox quits) |

A kernel older than the newest Landlock still sandboxes, with the rules it knows. Before
ABI 6 a renderer can send signals to rox and the user's other programs, and before ABI
3 it can empty a file the user can write by calling `truncate`. The card adds a line for
each: "This system's kernel is too old to stop it sending signals to rox and your other
programs." and "This system's kernel is too old to stop it emptying files you can
change." Network and unix sockets are the filter's on every ABI.

The approval records the tier as the card read it, every ask and every gap in it
(`PluginRecord::sandbox`). The gate refuses a renderer whose recorded tier isn't the one
its manifest and this system give it now, and it switches off and asks again. That
covers a manifest that adds an ask or `unconfined`, which changes its hash anyway; a
kernel that gains or loses a Landlock ABI; and a sandbox that couldn't be applied. Its
Status reads "Stopped: this system now sandboxes it differently than when it was
switched on, and it needs switching on again" (`Refused::Tier`), and Switch On raises its
card with the tier it gets now. A renderer approved as sandboxed is never started
unconfined in its place. When the
apply fails, the renderer doesn't start, every sandboxed renderer stays off for the rest
of the session, and its card says why. A renderer approved before the sandbox existed
recorded no tier: it was approved to run with the user's permissions, and runs as its
manifest and this system allow now, which is at most that.

### Testing it confined

Run it in rox on Linux. Its `sandboxed at Landlock ABI` line lists every rule it got,
and what it was refused comes back to it as `EACCES` or `EPERM` and usually reaches its
stderr in rox's log. `tests/fixtures/sandbox-probe` in `rox-plugins` is a renderer that
tries every way out and logs what stopped it, and `tests/sandbox.rs` runs it confined.
A debug build of rox reads `ROX_SANDBOX_SIMULATE`: `no-landlock`, `landlock-off`,
`no-seccomp` and `abi=N` show the card and the gate a lesser system gets, and
`apply-fails` fails the apply for real in the child. A release build never reads it.

### What breaks

A renderer written before the sandbox can need changes:

- Config, a cache or a font it keeps in the user's home. Its home is the cache folder,
  so what it writes there works and what it expects to find there isn't.
- An interpreter environment under home that isn't the entry's own interpreter: a
  `node_modules` outside the package, a pyenv shim, a program its launcher starts from
  home. The entry's interpreter itself is granted its install
  ([The interpreter](#the-interpreter)). Vendor the rest into the package.
- A local service: a unix socket of any kind, the session bus, a portal, a notification,
  a keyring. TCP on this machine needs `access.network`.
- A window of its own on the display: there's no display to reach.
- `/tmp` and `/dev/shm`: use `TMPDIR`.
- Files it reads from the user's folders: ask under `access.read`.
- A helper it runs from home or from a Program Folder outside the system's folders.

What the sandbox does not stop: a renderer hanging or resetting the GPU, which takes
rox's device with it; learning that a path exists, and its size, through `stat`;
drawing anything it likes in its own target; using as much CPU and memory as the user
may; and, granted the network, reaching any port on this machine.

## Platforms

On Linux the block is a sealed `memfd`, lent as fd 3 with `ROX_FRAMES_FD=3`, and
`hello`'s `handle` is 0. The cover is a second `memfd`, future-write sealed, lent as fd 4
with `ROX_COVER_FD=4`. The GPU texture is lent on Linux alone, after the cover. The
spectrum is a third `memfd`, future-write sealed, lent after all of those with its
number in `ROX_SPECTRUM_FD`. On macOS
and Windows gpui answers unsupported, rox never asks the canvas for one, and every target
draws through the slots. That code compiles on both by `cfg`, and no macOS or Windows
build of it has been made.

On macOS the design is the same fd 3 handoff, with the block a POSIX shared memory
object under a random name that's unlinked as soon as it's open. macOS has no seals.
The design relies on a shared memory object refusing a second `ftruncate`, which would
keep a renderer from shrinking it (`open` and `seal`, `rox-frames/src/map/unix.rs`).

On Windows the design is an unnamed section backed by the page file, duplicated into
the renderer with map read and map write rights only (`CHILD_ACCESS`,
`rox-frames/src/map.rs`; `lend`, `rox-plugins/src/process.rs`). There's no
`ROX_FRAMES_FD`. The section's handle value in the renderer is `hello`'s `handle`, which
a renderer maps with `MapViewOfFile` for the block's whole length. rox's pipe to a
renderer's stdin is blocking there, so the lines go through a writer thread that holds
up to 16 before rox counts the pipe as full (`WRITER_LINES`, `target.rs`). The
renderer starts with no console window and inside a job object, as a plugin does. The
cover is a second section, duplicated with map read alone, and its value is `hello`'s
`cover.handle`. The spectrum is a third, duplicated the same way, at `spectrum.handle`.
Python's `mmap` maps a file or a named mapping and has no way to take a section handle,
so the Python example logs that and exits on Windows. The Rust example takes the handle
from `hello`.

## A GPU renderer

Both examples fill their slot from the CPU. A renderer that draws on the GPU reads each
frame back into the slot, and the contract above leaves it a few things to get right. On
Linux it can skip the readback and draw into a texture rox lends it
([A GPU texture](#a-gpu-texture)), and it still needs the slots for the times it can't.

### Reading a frame back

Draw, copy the picture into a buffer the CPU can map, and wait for the GPU to finish
both. Only then start the frame: store the odd counter, copy the buffer into the slot,
store the even counter, announce. With `rox_frames::Writer` the copy is the closure
`Writer::frame` hands over. The counter is odd for the copy alone, about a millisecond at
1920x1080 from the right memory (below). Waiting on the GPU inside the odd window keeps
the slot mid-write for the whole draw, and rox throws away as torn any copy it starts
meanwhile. Announce only once the counter is even: rox can copy the slot as soon as it
reads the line.

Read back from memory the CPU caches. On a discrete GPU with resizable BAR, a buffer the
CPU can map can land in device-local memory, which the CPU reads uncached across the
bus. Measured on 2026-11-25 on a Radeon RX 6800 XT (RADV, Mesa 26.1.8), one
GPU-written 1920x1080 frame took this long to memcpy out of each kind:

| Memory | One frame |
|---|---|
| Device-local, host-visible | 226 ms |
| Host-visible, uncached | 44.6 ms |
| Host-visible, host-cached | 1.16 ms |

In Vulkan that's a memory type with `HOST_CACHED` among its flags. wgpu asks for one for a
buffer whose usage maps it for reading and not for writing. Adding `MAP_WRITE` to that
buffer moves it to memory chosen for uploads, which prefers device-local (`MemoryLocation`
in `wgpu-hal` 30.0.2, `src/vulkan/device.rs`). A copy that takes tens of milliseconds is
reading uncached memory.

A slot's rows are packed: each is `width * 4` bytes, right after the one before. A GPU
copy from a texture into a buffer often wants padded rows. wgpu starts each on a multiple
of 256 bytes (`COPY_BYTES_PER_ROW_ALIGNMENT`), as Direct3D 12 does
(`D3D12_TEXTURE_DATA_PITCH_ALIGNMENT`). At a width that's a multiple of 64 the rows come
out packed and one copy fills the slot. At any other width copy row by row, `width * 4`
bytes from the start of each padded row. The Resolution presets are all multiples of 64,
and the Width in Pixels row takes any width from 16 to 3840.

### How often to announce

Announce at most once per display frame. rox copies only the newest announce waiting,
and only once the canvas has taken the frame before it, so a frame past that rate is
drawn for nothing. rox doesn't tell a renderer the display's rate, so pace the loop on
the renderer's own clock, as the plasma example does at 60 frames a second (`RATE`).

A GPU loop left to run as fast as it can draws hundreds or thousands of frames a second.
Announcing each one breaks the flood rule: more than 500 stdout lines in a one-second
span, then more than 500 in the next, stops the renderer as soon as the second span
passes 500 (`Rate::note`, `target.rs`). That counts as an exit, so the renderer starts
again and is stopped again, and the fifth time it stays stopped.

### The pixels

The slot holds sRGB-encoded RGBA8, and rox uploads it as `Rgba8UnormSrgb`. A storage image
usually can't have an sRGB format: wgpu offers storage on `Rgba8Unorm` and not on
`Rgba8UnormSrgb`. Write `Rgba8Unorm` and apply the sRGB curve in the shader before the
store. Linear values written as they are show too dark.

Alpha is straight, not premultiplied, and anything below 255 shows the canvas behind the
target. An opaque picture writes 255 into every alpha byte. A pass that clears to
transparent black and never writes alpha leaves the target see-through.

The top row comes first, the way a texture is laid out in wgpu, Vulkan, Direct3D and
Metal. OpenGL's `glReadPixels` starts at the bottom row, so a renderer on OpenGL flips
the rows.

### Signals on the renderer's clock

Step a simulation on the renderer's own clock, with the newest signal values it holds.
Signals come on rox's paints, not the renderer's: a `signals` line goes out with each
frame the canvas paints while music plays, about one a frame. Its values are the hub's at
that paint, from audio the player hands the visualizers every 16 ms (`PUMP_INTERVAL`,
`rox-services/src/player.rs`), so each is about a frame old when it's written. A renderer
that steps faster than lines arrive steps more than once on the same values. One that
steps slower gets several lines between steps.

An onset, a beat or a trigger is a pulse: it jumps on a hit and falls back at the rate
its Response sets. Keep the highest value of each since the last step, and start again from
0 after the step, or a pulse that came and went between two steps is lost. A band, the
level and a total can take the newest value. The `kind` on the pool line says which is
which.

A pulse is still high for many frames after its hit, so its value alone doesn't say a new
hit came. A renderer that fires an effect at, say, a beat over 0.5 fires it again on every
step that reads the same tail, and on every step it takes with no new line in between.
Act on a rise instead: a hit is a line whose value is above the one the line before it
held. Or keep the maximum since the last step, as above, fed only by lines as they
arrive and cleared once a step has used it, and act on it only when a new line came in
since that step. A step with no new line has nothing new to act on, whatever the held
value reads.

The canvas paints for a target when it publishes a frame, so a renderer that stops
publishing stops getting signals, and they start again with its next frame.

### What a native renderer starts with

The process starts in the package's folder in an environment built from an empty one,
and nothing else of rox's reaches it (`prepare`, `rox-plugins/src/env.rs`; `spawn_with`,
`rox-plugins/src/process.rs`; `launch`, `rox-services/src/renderers.rs`). The list only
keeps a token or key rox was started with from being handed on. What confines a renderer
is [the sandbox](#the-sandbox), which also changes some of these for a sandboxed one.

| Variable | Where it comes from |
|---|---|
| The names in [the plugin environment table](10-plugins.md#process-lifecycle) | rox's own value, for each one rox has set |
| Names listed in `ROX_PLUGIN_ENV` | rox's own value, while a developer [passes more](10-plugins.md#passing-more-while-testing) |
| `PATH` | The plugin search path: the package's `bin` folder, the Program Folders, then rox's own `PATH` (`search_path`, `rox-plugins/src/search.rs`) |
| `PYTHONDONTWRITEBYTECODE`, `PYTHONUNBUFFERED`, `PYTHONUTF8` | `1` each |
| `ROX_RENDER_API`, `ROX_CACHE_DIR`, `ROX_FRAMES_WIDTH`, `ROX_FRAMES_HEIGHT` | rox, always |
| `ROX_FRAMES_FD`, `ROX_COVER_FD`, `ROX_TEXTURE_FD`, `ROX_SPECTRUM_FD` | rox, on Unix, each only when that handle is lent. The [Lifecycle](#lifecycle) table has their values |

The GPU variables pass through as rox has them and rox adds none of its own:
`LD_LIBRARY_PATH`, the Vulkan loader's `VK_ICD_FILENAMES`, `VK_DRIVER_FILES`,
`VK_ADD_DRIVER_FILES`, `VK_LAYER_PATH` and `VK_ADD_LAYER_PATH`, glvnd's
`__EGL_VENDOR_LIBRARY_FILENAMES` and `__EGL_VENDOR_LIBRARY_DIRS`, Mesa's
`LIBGL_DRIVERS_PATH`, `DRI_PRIME` and `MESA_VK_DEVICE_SELECT`, and NVIDIA's
`__NV_PRIME_RENDER_OFFLOAD` and `__VK_LAYER_NV_optimus`. They decide which driver and
GPU a renderer gets, and a texture rox lends can only be imported on the GPU rox made it
on. `DYLD_LIBRARY_PATH` doesn't pass. A renderer finds its own GPU libraries, the Vulkan
loader among them: from the system's library path, from an rpath of its own, or through
a launcher script of its own.

What it inherits depends on how rox was started. The Nix package in `flake.nix` finds
its own libraries through an rpath patched onto the `rox` binary (`patchelf --add-rpath`
in `postFixup`) and leaves the environment as it was. NixOS has no default library path
holding the Vulkan loader (`/run/opengl-driver/lib` holds the drivers, not
`libvulkan.so.1`), so there a renderer started by the packaged rox gets no loader from it.
The flake's dev shell puts the loader on `LD_LIBRARY_PATH`, and a rox run from that shell
passes it on. A renderer that runs under a dev build can fail to load `libvulkan.so.1`
under the package.

## A GPU texture

On Linux a renderer can draw into a GPU texture the render target owns, and rox shows it
with no copy through the CPU. Importing it takes Vulkan's external memory calls, which
the WebGPU API doesn't have, so a runtime that offers only WebGPU can't import it and
draws into the slots. rox makes the texture in the window that shows the target,
exports its memory as an opaque fd and lends it at spawn beside the frame mapping
(`register_external_texture`, `patches/gpui/e5-external-user-textures.patch`; `Lend` and
`Supervisor::texture`, `rox/src/canvas/target.rs`). The frame mapping is always made and
lent too, so the slots are there whenever the texture isn't.

A session is lent the texture when all of these hold:

- the approved manifest says `"gpu": true`;
- rox runs on Linux, where gpui's Vulkan windows can make one (everywhere else it answers
  unsupported, and so does a device that can't share the image as an opaque fd);
- the canvas made it within 2 s of the session's start. Only a paint of the target can
  make it, since a texture belongs to a window, so a window that isn't drawn starts the
  session on slots. A texture made later goes to the next session.

One texture is made per target at its mapping's size and lent to every session of it,
so a restart imports the same memory. For a renderer that resizes that's the ceiling
([The size](#the-size)): it draws into the top-left corner at the size it was told and
announces `t <n> <width> <height>`, and rox shows that corner alone, sampled no closer
than half a texel to its edge so nothing past it filters in (the texture quad's clamp,
`patches/gpui/e3-texture-quads.patch`). A new session at another size, which a renderer
that doesn't resize gets after a resize, a new size mode, a move to another window and a
switch-off each let it go, and the next session is lent a new one.

### hello's texture

The values are the image's create info as raw Vulkan numbers, for an importer to repeat
exactly (`texture_hello`, `rox-services/src/renderers.rs`):

| Key | Vulkan | Value |
|---|---|---|
| `fd` | The memory, `VK_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_FD_BIT` | the same number as `ROX_TEXTURE_FD` |
| `width`, `height` | `VkImageCreateInfo::extent` | the mapping's size: `frames` for a renderer that resizes, `hello`'s own otherwise |
| `allocation_size` | `VkMemoryAllocateInfo::allocationSize` | rox's dedicated allocation, in bytes |
| `memory_type` | `VkMemoryAllocateInfo::memoryTypeIndex` | the type rox allocated from |
| `handle_type` | `VkExternalMemoryHandleTypeFlagBits` | `"opaque-fd"` |
| `format` | `VkImageCreateInfo::format` | 37, `VK_FORMAT_R8G8B8A8_UNORM` |
| `view_formats` | `VkImageFormatListCreateInfo::pViewFormats`, in order | 43 and 37: `VK_FORMAT_R8G8B8A8_SRGB`, then the image's own format |
| `flags` | `VkImageCreateFlags` | 8, `VK_IMAGE_CREATE_MUTABLE_FORMAT_BIT` |
| `usage` | `VkImageUsageFlags` | 31: `TRANSFER_SRC`, `TRANSFER_DST`, `SAMPLED`, `STORAGE` and `COLOR_ATTACHMENT` |
| `tiling` | `VkImageTiling` | 0, `VK_IMAGE_TILING_OPTIMAL` |
| `layout` | `VkImageLayout` the image is in, and has to be in whenever rox samples it | 1, `VK_IMAGE_LAYOUT_GENERAL` |
| `device_uuid`, `driver_uuid` | `VkPhysicalDeviceIDProperties::deviceUUID` and `driverUUID` | 32 lowercase hex digits each |

The rest of the image is fixed: 2D, one mip level, one array layer, one sample,
`VK_SHARING_MODE_EXCLUSIVE`, `initialLayout` `VK_IMAGE_LAYOUT_UNDEFINED`. Copy the numbers
from `hello` rather than writing your own: an image that differs from rox's in any field
is a different image, and importing memory into it is undefined
(VUID-VkMemoryDedicatedAllocateInfo-image-01878).

### Importing it

In Vulkan terms, once, at start:

1. Choose the physical device before making a device: compare `device_uuid` and
   `driver_uuid` with each physical device's `VkPhysicalDeviceIDProperties`, read with
   `vkGetPhysicalDeviceProperties2`. That call is Vulkan 1.1, so the instance and the
   physical device both have to be 1.1 or later. An opaque fd only means something to
   the same driver on the same device: make your device on the one whose UUIDs match, and
   when none does, draw into the slots. On a machine with more than one GPU the default
   device can be the wrong one.
2. The device needs `VK_KHR_external_memory_fd`, and Vulkan 1.2 or
   `VK_KHR_image_format_list` for the view format list.
3. `vkCreateImage` with the create info above, chaining `VkExternalMemoryImageCreateInfo`
   with `handleTypes` `VK_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_FD_BIT` and
   `VkImageFormatListCreateInfo` with `view_formats`.
4. `vkGetImageMemoryRequirements`. If `size` isn't `allocation_size`, or `memoryTypeBits`
   doesn't have bit `memory_type` set, don't import: draw into the slots.
5. `vkAllocateMemory` with `allocation_size` and `memory_type`, chaining
   `VkImportMemoryFdInfoKHR` (the handle type and the fd) and `VkMemoryDedicatedAllocateInfo`
   naming the image. A successful import takes the fd; a failed one leaves it yours to
   close. Then `vkBindImageMemory` at offset 0.
6. The image is in `GENERAL` and released to `VK_QUEUE_FAMILY_EXTERNAL` when you get it.

#### In wgpu 30

These are notes on one version of one library: wgpu 30.0.2, with the wgpu-core and
wgpu-hal 30.0.2 it's built on. The Vulkan steps in this section are the contract. Another
version or another library gets there its own way.

wgpu has no import of its own, so a wgpu renderer does steps 1 to 5 in `ash` through
wgpu-hal, on the Vulkan backend only. Use the `ash` wgpu-hal depends on, 0.38.

- Step 1 comes before a device exists, so it takes the adapter's raw handle, not the
  device's. `Instance::enumerate_adapters(Backends::VULKAN)` lists the adapters (it's
  async), and `Adapter::as_hal::<Vulkan>()` gives each one's hal adapter: its
  `raw_physical_device()`, its `physical_device_capabilities().properties().api_version`
  for the device's Vulkan version, and `shared_instance()`, whose `raw_instance()` is the
  `ash::Instance` that has `get_physical_device_properties2` and whose
  `instance_api_version()` is the instance's (`wgpu/src/api/adapter.rs`,
  `wgpu-hal/src/vulkan/adapter.rs` and `instance.rs`). Request the device from the adapter
  whose UUIDs match. `Device::as_hal::<Vulkan>()` is for the steps after.
- wgpu-hal turns on `VK_KHR_external_memory_fd` on any device that has it, and
  `VK_KHR_image_format_list` on one below Vulkan 1.2 that has it (`adapter.rs`). The hal
  device's `enabled_device_extensions()` says whether it did.
- Make the image and import the memory with the hal device's `raw_device()`. Wrap it with
  `texture_from_raw(image, &desc, None, TextureMemory::Dedicated(memory))`, which takes
  both and frees them when the texture goes, then `Device::create_texture_from_hal::<Vulkan>`
  with format `Rgba8Unorm`, view formats `[Rgba8UnormSrgb]` and the usages matching the
  flags above (`COPY_SRC`, `COPY_DST`, `TEXTURE_BINDING`, `STORAGE_BINDING`,
  `RENDER_ATTACHMENT`). An `Rgba8UnormSrgb` view has to set
  `usage: Some(RENDER_ATTACHMENT)`, since a storage usage can't carry over to an sRGB view
  (VUID-VkImageViewCreateInfo-usage-02275).
- `create_texture_from_hal`'s last argument is the state wgpu takes the image to be in.
  wgpu-core puts the texture in its tracker in that state (`create_texture_from_hal`,
  `wgpu-core/src/device/resource.rs`), and the first barrier wgpu records on it starts
  from the layout that state maps to (`derive_image_layout`, `wgpu-hal/src/vulkan/conv.rs`).
  `TextureUses::UNINITIALIZED` maps to `VK_IMAGE_LAYOUT_UNDEFINED`, which takes the image
  out of the layout rox left it in and lets the driver throw its contents away. Every
  storage use maps to `GENERAL` on a colour format, so pass
  `TextureUses::STORAGE_READ_WRITE`. wgpu-core counts a texture made this way as written
  already and never clears it.
- A wgpu `CommandEncoder` is used either raw or through wgpu, never both. The first raw
  access through `CommandEncoder::as_hal_mut` or the first wgpu command settles which,
  and the other then panics: "Mixing the wgpu encoding API with the raw encoding API is
  not permitted" (`EncodingApi::set`, `wgpu-core/src/command/mod.rs`). wgpu also keeps
  its commands and passes as a list and records them only at `finish`
  (`encode_commands`), so nothing raw could go between them anyway. The barriers each go
  in an encoder of their own, used raw and for nothing else: `as_hal_mut::<Vulkan, _, _>`
  hands over the hal encoder, whose `raw_handle()` is the `VkCommandBuffer`, open. Record
  the barrier with the `ash::Device` from the hal device and leave the buffer open, since
  `finish` ends it. Your queue family is the hal device's `queue_family_index()`.
- A frame is then three command buffers in one `Queue::submit`, in this order: the
  acquire encoder's, the draw's own, and the release encoder's. A pipeline barrier orders
  against the commands before and after it in submission order on the queue, across
  command buffers. At the submit wgpu puts a command buffer of its own transitions in
  front of any of yours that needs one ("(wgpu internal) Transit",
  `wgpu-core/src/device/queue.rs`), so a layout change wgpu makes for the draw comes after
  the acquire.
- Storing into the image keeps it in `GENERAL` throughout. A render pass on it makes wgpu
  move it to `COLOR_ATTACHMENT_OPTIMAL` and track it there. Ending the draw's encoder with
  `CommandEncoder::transition_resources` to `TextureUses::STORAGE_READ_WRITE` has wgpu
  move it back to `GENERAL` and know it, so the release barrier is `GENERAL` to `GENERAL`
  and the next frame's pass starts from the right layout.

The storage path with these three command buffers and `STORAGE_READ_WRITE` has been run
against rox's texture by a renderer outside rox. The render pass path with
`transition_resources` hasn't been run. Spike GF-S imported the texture with
`UNINITIALIZED` and no barriers in about 120 lines and showed it pixel-exact.

### Each frame

1. Acquire: an image memory barrier from `VK_QUEUE_FAMILY_EXTERNAL` to your queue family,
   `GENERAL` to `GENERAL`, before your first write.
2. Write every pixel of the frame: the whole image for a renderer that doesn't resize, and
   only the top-left corner at the size it was told for one that does. That size comes
   from `hello` and the newest `size` line, never from the image's extent, which is the
   ceiling. rox shows the corner alone, so whatever the rest of the image holds never
   reaches the screen. Either store into the `Rgba8Unorm` image with the sRGB curve
   applied in the shader, since a storage image can't be sRGB, or render through an
   `R8G8B8A8_SRGB` view and let the hardware apply it. rox samples through an sRGB view
   either way, and both were pixel-exact in the spike. Alpha is straight, as in a slot.
3. Release: the barrier back to `VK_QUEUE_FAMILY_EXTERNAL`, after your last write,
   leaving the image in `GENERAL`, the layout rox samples it in. A render pass leaves it
   in `COLOR_ATTACHMENT_OPTIMAL`, so either move it back to `GENERAL` first or make that
   the release barrier's old layout.
4. Submit, with the acquire ahead of the draw and the release after it in submission
   order, and wait on your own fence for that submission.
5. Write `t <n>\n` and flush, or `t <n> <width> <height>\n` for a frame drawn in the
   top-left corner at a size rox told it, which a renderer that resizes always writes.

rox never waits on your fence and never looks at your queue: whenever it paints, it
samples whatever the memory holds. Announcing only after the wait is what keeps a
half-drawn frame off the screen. The pacing rule is the slots': announce at most once per
display frame (see [How often to announce](#how-often-to-announce)), since a `t` line
counts toward the flood rule like any other.

What rox does with the image's ownership: at registration it takes the image out of
`UNDEFINED` into `GENERAL` and releases it to `VK_QUEUE_FAMILY_EXTERNAL`, and waits on its
own fence before the fd leaves, so your first acquire can't race it. It doesn't acquire
the image back before it samples, or release it after, which the Vulkan spec asks of both
sides of an ownership transfer. Doing that on every frame would need a hook in gpui's
draw, and handing ownership back and forth would need rox to tell the renderer when its
sampling finished, which means a wait or a new line. Both sides work in `GENERAL` on one
device, and on the driver this was built and run on (RADV, Mesa 26.1.8, a Radeon RX 6800
XT) the frames show pixel-exact without it. Other drivers haven't been tried. If one
needed rox's acquire, the symptom would be wrong pixels in the target, since rox only
ever samples the image.

### Falling back

Draw into the slots and announce `f`, as described above, whenever:

- `hello` has no `texture`: not Linux, the manifest doesn't say `gpu`, the window couldn't
  make one, or the canvas didn't paint in time;
- the UUIDs differ;
- the image's requirements don't match `allocation_size` and `memory_type`, or the import
  fails;
- a submit or a wait fails mid-session.

A renderer can switch between the two in one session, both ways, and whichever was
announced last is what the target shows (`Announced`, `target.rs`). A `t` from a session
lent no texture is refused like a slot that lies, and 60 refusals in a row stop the
renderer.

### What rox trusts

- rox allocates and exports, and never imports a renderer's memory. On RADV an import
  with a lying shape crashed the importing process in the driver's own error path, so the
  process that has to survive is the one that allocates.
- rox shares no fence, semaphore or lock with a renderer and waits on nothing it controls.
  The only wait in the path is rox's own fence at registration.
- rox believes nothing a renderer says about the texture. A `t` line carries a number rox
  doesn't read, and all the line can do is have the canvas paint rox's own texture again:
  it can't move, resize or reformat it, or reach anything outside it.
- What changes from the slots is that rox can't check a counter on GPU memory. A renderer
  can change what its target shows whenever rox paints, announced or not, and a frame
  written while rox samples can show half old and half new. It's only ever pixels, and
  only its own target's. A renderer that forces a GPU reset can take rox's device with it
  on either channel: rox restarts itself then (gpui patch c5), stopping every renderer
  first.

### When the texture goes

| Case | What happens |
|---|---|
| A restart after an exit | The same texture, lent again |
| A new size mode, scale or fixed resolution | The target is new: the old texture is released and a new one made |
| A resize, for a renderer that resizes | Kept: it's at the ceiling, and the renderer draws the new size into its corner |
| A resize, for one that doesn't | Kept until its next session, which is lent a new one at the box's size then |
| Pop Out, Dock Back, or a move to another window | The texture belongs to the window that made it, so rox lets it go, makes one in the new window and starts the renderer again with its fd. That doesn't count as an exit, and the log says `moved window, starting again` for each move |
| Switching the renderer off, a changed package, the fifth exit | The session ends and the target gives its texture back. The next session gets a new one |
| Deleting the item, closing the canvas, applying a workspace, quitting | Released with the item |
| Paused, hidden, or a window not drawn | Kept, and not painted. The renderer gets `pause` as with slots |
| A lost GPU device | rox stops every renderer and restarts itself. Its fds close at the exec |

Each release frees rox's image (`release_user_texture`) and closes rox's fd. The memory
itself goes once the renderer's import goes too, when it exits. The image holds nothing
defined until a renderer writes it, so a target shows nothing on this path until its first
`t`, and nothing again after a move until the new texture's first `t`. On RADV a texture
announced without ever being written showed as transparent, which is the driver's doing
and not a promise.

## Debugging

A renderer's stderr is in rox's log: `<data>/logs/rox.log`, rox's own stderr, and the
Console window (F12). rox's own lines about a target name the renderer's id:

| Line | Meaning |
|---|---|
| `renderer <id>: started at <w>x<h>, <tier>` | The process runs and `hello` went, in that tier: `sandboxed` and its asks and gaps, `unconfined`, or `permissions` |
| `renderer <id>: started at <w>x<h>, resizes up to <W>x<H>, ...` | The same, for a renderer that resizes, with the mapping's size |
| `renderer <id>: set aside settings its manifest doesn't take: <keys>` | Stored values `hello` left out, undeclared or out of range |
| `render target <id>: a setting it reads at its start changed, starting again` | A `restart` setting moved |
| `renderer <id>: started at <w>x<h>, lent a texture, <tier>` | The same, with the GPU texture lent |
| `renderer <id>: sandboxed at Landlock ABI <n>: <rules>` | Every rule the sandbox added |
| `sandbox: couldn't be applied, renderers stay off until approved again: <why>` | The apply failed, and the renderer didn't start |
| `render target <id>: no texture from the canvas, this session uses shared memory` | The canvas didn't make the texture within 2 s |
| `canvas: no GPU texture at <w>x<h>, shared memory it is: <why>` | The window couldn't make one, for the target's life |
| `render target <id>: moved window, starting again` | The target was lent a texture and its canvas moved to another window, by Pop Out or Dock Back, so it starts again with a texture made there. A session on the slots carries on through either, with the same process |
| `render target <id>: didn't start: <why>` | The spawn or `hello` failed, as an exit |
| `render target <id>: not started: <why>` | The gate refused it |
| `render target <id>: the renderer exited` | stdout closed |
| `render target <id>: stopped, more than 500 lines a second` | The flood rule |
| `render target <id>: stopped, 60 refused frames in a row` | Wrong sizes or an unwritten slot, every time |
| `render target <id>: stopped after repeated exits` | The fifth exit |
| `render target <id>: stopped by rox` | A switch-off or a quit |
| `renderer <id>: stopped, no longer switched on` | The gate closed on a running session |
| `render target <id>: told to draw at <w>x<h>` | A `size` line went to a renderer that resizes |
| `render target <id>: session ended, <n> frames copied, <n> texture frames, <n> torn, <n> refused (<n> unwritten, <n> wrong size, <n> texture frames with no texture), <n> lines read, <n> sizes told` | A session ended, for any reason, and what its frames and its size lines came to |

The inspector's Status row words the same states for the user, with the gate's reason
when it refused. rox writes no line for a frame it drops, only a count in the line each
session ends with (`log_tally`, `target.rs`). Frames copied are whole frames read
out of a slot. Torn ones had an odd counter, or one that moved during the copy.
Unwritten ones were announced while the slot's counter was still 0, and wrong size ones
had size words that weren't a size the session was told, the newest or the one before
it, or for a texture frame a size it wasn't told or none from a renderer that resizes.
Texture frames are `t` lines rox showed, and texture frames with no texture are `t` lines
from a session that wasn't lent one. Lines read counts
every line rox read from the renderer's stdout while the session ran, announce or not.
The line is written as the session ends and before the renderer's stop, so a new
resolution's `started at` comes after it. Only 60 refusals in a
row stop a renderer, and a torn frame never does, so a target that stays blank while
Status says Running shows up here: check that `seq` goes `2n + 1` then `2n + 2`, that
the slot's size words are right, that the slot in the line is `n % 2`, and that the line
is flushed. Lines read far above frames copied is a renderer announcing faster than the
screen takes frames, which costs it nothing, or writing lines that aren't announces.

A native renderer's package has to be on a filesystem that lets programs run, and for
a `.roxp` that's rox's data directory, where the unpacked copy goes. On one mounted
`noexec` the start fails with "Permission denied" five times and stops.

Developer mode, on a switched-on renderer folder's row, approves its saves until rox
quits, and Pack beside it writes a `.roxp`. It approves only a save that leaves
`plugin.json` exactly as it was last approved. A change to the manifest, even its
description or version, switches the renderer off and puts the card up, since a new
grant lives there. A running session keeps the code it started with. Switching the
renderer off and on starts the new code.

### The stand-in host

A renderer can't run without a block rox made. `examples/renderers/standin.py` plays
rox's part, so a renderer can be run and checked outside rox. It's a development tool:
it grants the renderer everything its flags ask for, with no enable card and no
approval, and runs it unconfined, so run it only on a renderer you trust. Test the
sandbox in rox ([Testing it confined](#testing-it-confined)). It's Python's standard library alone,
on Linux and macOS:

```sh
python3 examples/renderers/standin.py --spectrum --palette --cover path/to/renderer
python3 examples/renderers/standin.py --size 640x360 -- ./my-renderer --verbose
python3 examples/renderers/standin.py --size 320x180 --frames 1280x720 --resize 640x360,1280x720 path/to/renderer
python3 examples/renderers/standin.py --settings '{"look":"ember"}' --set look=ice@3 --set rate=30@5 path/to/renderer
```

Given a folder, it starts the renderer from its `plugin.json` as rox would, the
interpreter looked up in its `bin`, then each `--program-folder`, then `PATH`. After `--`
it runs the command as given. It makes the frame mapping, and with `--cover` and
`--spectrum` the cover mapping with a test picture and the spectrum mapping with a
synthetic spectrum, lends them from fd 3 up in rox's order with rox's environment, a
`ROX_CACHE_DIR` included, `rox-standin-cache` in the temp folder unless `--cache DIR`
names another, and
writes `hello`, a pool of one signal of each kind (`--empty-pool` for none), and
`signals` lines about 60 a second from a scripted drive: a rest, a bass swell, then beats
at 120 a minute, then the music stopping. With `--palette` it sends a `palette` line.
With `--cover` it writes the transparent square and sends `{"cover":null}` ahead of the
pool, as rox does with nothing playing, then the test picture and its counter once the
music starts, and the transparent square and `null` again when it stops. With
`--spectrum` it writes the block as rox does: zeros once when it makes it, rows with the
playing bit 62.5 times a second while the music plays, then zeros once when it stops, and
nothing between, so the counter stands still at rest. Each write builds the whole page
first and copies it in with one slice assignment between the two counter stores, which
holds the counter odd for about half a microsecond. Partway through it pauses the
renderer for half a second and reads nothing meanwhile, dropping the pointer events and
`signals` lines still waiting as rox does, and it sends pointer moves, a press and
release, a scroll and a leave. At the end it closes stdin and kills the renderer's
process group 200 ms later.

For a renderer whose manifest says `resizes`, or a command after `--` with `--resizes`,
it makes the mapping at `--frames`, says `--size` in `hello` with `frames` beside it, and
sends each size in `--resize` on a `size` line, spread evenly across the run and held to
the mapping with its shape kept. It accepts a frame at the size it told last and the one
before, as rox does. A renderer that doesn't resize gets one mapping at `--size` and no
`size` line, whatever `--resize` says.

Given a folder, it checks the manifest's `settings` against rox's rules first, and stops
with rox's own reason for one rox would refuse, as `standin: rox refuses this manifest:
plugin.json: setting 1 is a number without a min and a max`. `--settings` is the
target's stored settings, held to what the manifest declares and sent in `hello` as rox
sends them, numbers included, with a line on stderr for what it set aside. Each `--set
KEY=VALUE@SECONDS` changes one at that many seconds into the run, on a `settings` line
with the whole set. VALUE is read as JSON where it parses and as text where it doesn't,
so `look=ice@3` and `rate=30@5` both work. A change to a `restart` setting gets a line on
stderr instead, since rox would start the session again. `--ready` holds the drive back
until the renderer prints a line `ready`, and gives it as long to exit at the end, each up
to 30 seconds, for a test that has to see the rest before the music.

It copies the newest announce once a display frame, as rox does, and reports frames
copied, distinct pictures among them, torn and refused frames, lines read, announces and
frames a second, the longest run of refused frames, the size lines it sent and every size
frames were drawn at, whether the flood rule would have
stopped it, frames whose counter or slot isn't the one this doc gives (rox goes by the
counter alone, and the stand-in says so), the bytes left waiting in the renderer's stdout
when the pause ended (anything above 0 means it announced while paused), and whether it
exited within 200 ms of stdin closing. It exits with 1 when rox would have stopped the
renderer or no frame came. `--dump 0,30` saves those frame numbers as PPM files in
`--dump-dir`, and `--json` prints the report as one line.

Its layout numbers are copied from `rox-frames` and its limits from the render target.
Tests hold each copy to the code it came from (`crates/rox-frames/tests/standin.rs`,
`the_stand_in_starts_a_renderer_as_rox_does` in `rox-services/src/renderers.rs`,
`the_stand_in_holds_a_renderer_to_these_limits` in `rox/src/canvas/target.rs`,
`the_stand_in_passes_the_same_names` in `rox-plugins/src/env.rs`, and
`the_stand_in_holds_settings_to_these_rules` in `rox-plugins/src/renderer_settings.rs`,
which also runs its settings check and its values against rox's own). One runs it against
the bars example and wants frames with nothing refused, another watches its spectrum
and cover mappings through a run and wants them written as rox writes them, and a third
resizes the bars example three times and runs a renderer written before sizes beside it,
which has to come back with no size line and nothing refused.

## Costs

Measured 2026-11-16 on a prototype of this channel, in a release build on Linux 6.18,
a Ryzen 9 5950X and a Radeon RX 6800 XT, gpui's blade renderer over Vulkan (RADV), in a
headless compositor at 1920x1080 drawing at 61.9 Hz, one renderer at 60 frames a second:

| Size | rox UI thread, the upload | rox, the copy off the UI thread | Renderer, the write into the slot |
|---|---|---|---|
| 1280x720 | 0.26 ms | 0.25 ms | 0.23 ms |
| 1920x1080 | 0.66 ms | 0.77 ms | 0.74 ms |
| 3840x2160 | 2.66 ms | 7.26 ms | 2.24 ms |

The upload is gpui's copy into its staging memory, the same cost Milkdrop's frames pay
(0.53 ms at 1920x1080 on that machine). At 1920x1080 rox's memory grew 32 MB with a
target, the block and the frame buffers, and 127 MB at 3840x2160, beside the texture on
the GPU (8.3 MB and 33 MB). The renderer's figure is the copy alone. A GPU renderer
reads its frame back into the slot, and that readback is the copy. Whole frames down a
pipe were measured as well: 4.96 ms to read one at 1920x1080, against the 0.77 ms copy.
ADR 37's amendment of 2026-11-23 has the decision those numbers made.

Many targets at once were measured 2026-11-24 on the shipped channel, on the same
machine (RADV from Mesa 26.1.8, Linux 6.18.64), in a release build with a 1920x1080
window and design mode off. Every target ran the Plasma example, which published 60
frames a second on 14% of a core at 1920x1080 and 51% at 3840x2160. The figure is
`debug.frames`, the main thread's time per frame from the workspace's render to the
present, as the mean of three runs of 3 s:

| Targets | Megapixels | Mean | Worst | Frames a second | rox CPU |
|---|---|---|---|---|---|
| 1 at 1920x1080 | 2.1 | 1.1 ms | 2.0 ms | 60 | 11% |
| 2 at 1920x1080 | 4.1 | 1.8 ms | 3.0 ms | 62 | 21% |
| 3 at 1920x1080 | 6.2 | 2.5 ms | 6.4 ms | 62 | 35% |
| 4 at 1920x1080 | 8.3 | 3.2 ms | 5.1 ms | 62 | 46% |
| 6 at 1920x1080 | 12.4 | 5.0 ms | 8.8 ms | 61 | 86% |
| 1 at 3840x2160 | 8.3 | 3.0 ms | 4.8 ms | 60 | 32% |
| 2 at 3840x2160 | 16.6 | 6.6 ms | 11.6 ms | 62 | 80% |
| 2 at 3840x2160, 1 at 1280x720 | 17.5 | 7.1 ms | 11.1 ms | 62 | 88% |
| 2 at 3840x2160, 1 at 1920x1080 | 18.7 | 8.3 ms | 12.0 ms | 62 | 113% |
| 2 at 3840x2160, 1 at 2240x1260 | 19.4 | 8.7 ms | 13.3 ms | 62 | 128% |
| 2 at 3840x2160, 1 at 2560x1440 | 20.3 | 9.7 ms | 16.6 ms | 62 | 139% |
| 2 at 3840x2160, 1 at 3200x1800 | 22.3 | 17.5 ms | 29.2 ms | 55 | 281% |
| 3 at 3840x2160 | 24.9 | 20.6 ms | 28.8 ms | 47 | 278% |
| 4 at 3840x2160 | 33.2 | 28.0 ms | 75.3 ms | 31 | 371% |

rox CPU counts every rox thread, 100% being one core. The cost follows the megapixels,
not the count: four 1920x1080 targets cost what one 3840x2160 target does, and each
further 1920x1080 target adds about 0.7 ms. Past 20 megapixels it more than doubles
within 2 megapixels. At 33.2 the renderers fell short of 60 frames a second on nearly a
core each, so that row is partly their cost. The mean crosses 8.3 ms, half a 60 Hz frame,
at about 18.8 megapixels, read from nine runs at 18.7 (8.2 ms) and six at 19.4 (8.8 ms).
That's where the canvas's weight line starts warning (`TARGET_PIXELS` in
`crates/rox/src/canvas/cost.rs`). The inspector counts a target that follows its box at
the size its session draws at now, and a fixed one at its own size (`drawing`). A workspace
or object apply card has no session to ask, so it counts a following target without its
megapixels, and when the fixed ones warn, the line says how many follow and aren't in its
number.

The GPU texture was measured 2026-11-25 on the same machine (Linux 6.18.64, Ryzen 9
5950X, Radeon RX 6800 XT, RADV from Mesa 26.1.8), in a release build with a 1920x1080
headless compositor and design mode off, against the slots on the same day. Every target
ran one test renderer at 60 frames a second, drawing the same frames either way: on the
texture it copies a frame made up front from GPU memory into the texture, and on the
slots it copies the same frame into a slot. The figure is `debug.frames` again, the mean
of three runs of 3 s after 2 s to settle, with rox's CPU over the same runs. The one-minute
load average was 0.7 to 2.1, with no other rox or renderer running.

| Targets | Texture: mean | worst | frames a second | rox CPU | Slots: mean | worst | frames a second | rox CPU |
|---|---|---|---|---|---|---|---|---|
| 1 at 1920x1080 | 0.49 ms | 0.91 ms | 60 | 3.4% | 1.13 ms | 2.04 ms | 60 | 11% |
| 1 at 3840x2160 | 0.49 ms | 0.87 ms | 60 | 3.3% | 2.69 ms | 5.02 ms | 60 | 30% |
| 4 at 1920x1080 | 0.53 ms | 0.91 ms | 62 | 4.2% | 3.21 ms | 4.66 ms | 62 | 40% |
| 4 at 3840x2160 | 0.50 ms | 0.93 ms | 62 | 4.2% | 23.7 ms | 56.4 ms | 41 | 267% |

On the texture rox's cost doesn't follow the pixels: a frame costs it the paint of one
quad per target. In the last row the slot renderers ran at 97 to 99% of a core each and
fell short of 60 frames a second, so that row is partly their cost. Making the texture
takes the UI thread 0.7 to 4 ms once per target, resolution and window (GF-2's figure),
and a restart reuses it. The texture is the image's dedicated allocation in VRAM, 8.8 MB
at 1920x1080 and 33 MB at 3840x2160 (`allocation_size`), and a renderer's import shares
it rather than adding another.

The sandbox was measured 2026-11-26 on the same machine, in a release build, each
renderer run confined and unconfined in turn in a Renderer panel at 1280x720 for 30 s
at a time. Every one drew at the same rate either way: Ember 60 frames a second into
the texture with the same simulate and draw times, Plasma 60, Three Field and Aero 60
with the same render times, and Bars its 30. Starting confined took 0.2 to 0.4 ms more
for `sh -c true` and 0.2 to 1.1 ms more for `python3 -c pass`, as medians of 30 starts
(`what_starting_confined_costs`, `rox-plugins/tests/sandbox.rs`). A renderer's first
start compiles its shaders into its own cache folder.

A size that follows the box was measured 2026-11-26 on the same machine, in a release
build in a headless compositor at 1920x1080, with a Renderer panel in a floating window
resized about 30 times a second for 10 s. A renderer that resizes was sent one `size`
line for the whole drag, once it held still, and kept its process; its session ended with
0 torn and 0 refused frames. One that doesn't resize was sent none and kept its process
and its one size. rox's frame time over the drag was a mean of 0.64 ms and a worst of
1.43 ms with the first, 0.58 ms and 5.81 ms with the second, against 0.76 ms and 0.59 ms
standing still. rox's memory didn't move across either drag.

The mapping at the ceiling costs the pages a session has written, not its whole length.
At a 200% Render Scale the ceiling is 3840 by 2160 and the mapping 63.3 MB. A session
started at 1280x720 held 7.0 MB of it resident, its two slots' frames; after the box grew
to draw at 3840x2096 it held 61.4 MB, and back at 1280x720 it still did, since a page
written once stays until the session ends. Proportional set size splits that between rox
and the renderer, 30.7 MB each at the largest. A session drawing on the GPU texture
writes no slot, and its 7.1 MB mapping held nothing resident. The texture is made at the
ceiling for a renderer that resizes, a dedicated allocation of `width * height * 4`
bytes, about 8.3 MB at 1920x1080 and 33.2 MB at 3840x2160.

## Reference

The shared memory is `crates/rox-frames`: `layout.rs` (the offsets, `Spec`, the ranges),
`map.rs` with `map/unix.rs` and `map/windows.rs` (making, sealing and lending the block),
`read.rs` (`copy_slot`, `copy_slot_sized`, `Read`, `Refusal`), `write.rs` (`Writer`,
`frame_at`, behind the `writer` feature), `cover.rs` (the cover mapping: its layout, `fit`, `letterbox`, `Cover`, and
`CoverReader` behind the `cover-reader` feature), `spectrum.rs` (the spectrum mapping:
its layout, `Spectrum`, and `SpectrumReader` behind the same feature) and `lib.rs` (`ANNOUNCE_MAX`,
`parse_announce`, `parse_texture_announce`, `parse_sized_texture_announce`, `announce`,
`clock_ns`).

The package side is `crates/rox-services/src/renderers.rs` (the listing, the gate,
`asks`, `start`, `Session`, the exit count, `enforce`, and the wanted signals' matching)
on top of `crates/rox-plugins`: `manifest.rs` (`capabilities.renderer`, `check_renderer`,
`check_wanted`, the entry and the interpreter table), `renderer_settings.rs` (a setting's
declaration, `check`, `values`, `restarts`), `process.rs` (the spawn, fd 3 and up,
the Windows handles, stderr, `never_block`, `spawn_confined`), `sandbox.rs` (`Access`,
`Tier`, `Support`, `Profile`, `interpreter_install`, `grantable`, the ruleset and the
filter) and `host.rs` (`BACKOFF`,
`CRASH_WINDOW`). The tier's gate and the read asks' folders are `sandbox_holds` and
`read_folders` in `renderers.rs`. The
approval card's grants are `grants` in `crates/rox-services/src/plugins.rs`, with the
re-approval diff in `changes`.

The spectrum's rows are `crates/rox-viz/src/spectrum.rs` (`Rows`, `log_edge`,
`normalise`), and the one shared mapping, its leases and its writes on the pump are
`crates/rox/src/canvas/spectrum.rs`.

The item is `crates/rox/src/canvas/target.rs` (the supervisor, the limits, the reader,
the copy, the texture's handoff in `Lend` and `Supervisor::texture`, every line sent, the
cover's decode, the status, and the size: `Sizing`, `ceiling`, `follow_size`, `fit_box`,
`Shown::want`, `Told`), drawn through `crates/rox/src/canvas/texture.rs` (`Picture`,
`Lender`, and the external texture's registration and paint). The texture itself is
`Window::register_external_texture` in the vendored gpui (`patches/gpui/e5-external-user-textures.patch`)
over blade's `export_texture` (`patches/blade-graphics/a0-shared-textures.patch`). `crates/rox/src/panels/canvas.rs` places it, reads
the signals, the palette and the stage for it each frame, follows the playing track for its cover,
sends it the pointer (`hit`, `point_at`, `button_name`, `scroll_lines`), adds it from the
Add menu, and offers its wanted signals (`offer_signals`). The prompt and the inspector's
Signals row are in `crates/rox/src/canvas/overlay.rs`, the Size, Render Scale, Fit and
settings rows in `crates/rox/src/canvas/overlay/target_rows.rs`, and the settings'
controls both hosts draw in `crates/rox/src/canvas/target_settings.rs`. A pool entry's kind and band come from `Source` in
`crates/rox-viz/src/signal.rs`. The Renderer panel is `crates/rox/src/panels/renderer.rs`,
over the same `Shown` session the canvas uses. The Renderers settings page is
`crates/rox/src/settings/window/renderers_page.rs`, drawing its rows with the row code in
`plugins_page.rs`, and the enable card both pages and both hosts raise is
`crates/rox/src/enable_card.rs`.

The examples are `examples/renderers/bars/bars.py` and
`examples/renderers/plasma/src/main.rs`, each with a `plugin.json` and a README. The
stand-in host is `examples/renderers/standin.py`.
