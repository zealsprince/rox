# Renderers

How a renderer is built and how to write one: the package, the process rox starts for a
render target, the shared memory its frames arrive through, the GPU texture it can draw
into instead on Linux, the lines it reads and writes, and the rules that stop it. This
makes [ADR 37](../02-architecture/decisions/37-adr-render-targets.md) concrete, with its
amendments of 2026-11-16 (renderers are listed apart from plugins), 2026-11-23 (shared
memory is rox's own channel), 2026-11-25 (a GPU texture, Linux first) and 2026-11-26 (a
page and a switch of their own, and the Renderer panel). Underneath, a renderer is the package of
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
(`API`, `rox-services/src/renderers.rs:33`). The shared memory is layout version 1 behind
the magic `RXF1` (`VERSION` and `MAGIC`, `rox-frames/src/layout.rs:10-12`), and the cover
mapping is layout version 1 behind `RXC1` (`rox-frames/src/cover.rs`). The manifest
is plugin API 1 (`SUPPORTED_API`, `rox-plugins/src/manifest.rs:24`). `rox-frames` uses
libc 0.2 on Unix (0.2.186 in rox's lockfile, 0.2.190 in the plasma example's) and
windows-sys 0.61 (0.61.2) on Windows. What [A GPU renderer](#a-gpu-renderer) and
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
   pointer while it's over the render target, and gets the colours of the current theme,
   since its manifest asks for the palette. Switch On approves exactly the files in the
   package and switches it on, as the switch on the Renderers page does, and the panel
   starts it.
3. Or on a canvas: with design mode on, right-click empty stage, or press the
   plus in the canvas inspector's header, and pick Add Render Target > Bars
   (`open_add_menu`, `rox/src/panels/canvas.rs`). The same card comes up for a renderer
   that isn't switched on, and the signals offer after it. The new target draws at 1280
   by 720 pixels on a 640 by 360 unit item (`TARGET_PIXELS` and `TARGET_UNITS`). The
   inspector's Resolution row changes the pixel size, and Status says whether the
   renderer runs, with a button that fixes what it can.
4. Play something. Bars draws a bar for each signal in the pool (Application > Signals
   edits it) in the theme's accent, so a pool with nothing in it draws one empty bar.

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
    "renderer": { "label": "Bars", "palette": true }
  }
}
```

`capabilities.renderer.label` is what the Add Render Target menu lists. A blank one
refuses the manifest with "the renderer has no label". A renderer that also declares a
`source`, any `panels` or a true flag under `providers` is refused with "a renderer
declares nothing else, and this one declares ..." (`check_renderer`,
`rox-plugins/src/manifest.rs:419`), because those would give it a plugin host and the
plugin wire. Unknown keys inside `capabilities` are ignored, as they are for a plugin.

### What a renderer asks for

Three more keys under `capabilities.renderer` ask for something past the signals and
the pointer every renderer gets. Each is a grant: the enable card says it, and rox sends
or offers it only to a renderer whose approved manifest asks (`grants`,
`rox-services/src/plugins.rs:1090`). Left out, each is off, and the card says nothing of
it.

| Key | Type | What it gets | The card says |
|---|---|---|---|
| `signals` | a list | rox offers to add these to the user's pool | Follows these signals: Kick (Band 30-120 Hz), Level. When you place a render target for it, rox offers to add the ones you don't have yet. |
| `palette` | `true` or `false` | the theme's colours, on [`palette` lines](#rox-to-the-renderer) | Gets the colours of the current theme, which follow the cover when song theming is on. |
| `cover` | `true` or `false` | the playing track's cover as pixels, through [the cover mapping](#the-cover-mapping) | Gets the cover of the playing track, as a picture. |

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

A manifest asking for everything:

```json
"capabilities": {
  "renderer": {
    "label": "Pulse",
    "palette": true,
    "cover": true,
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
ranges above (`check_wanted`, `rox-plugins/src/manifest.rs:453`).

A renderer's name for a signal is a stranger's text, so it's held to that size and
shown only after the signal's kind and band: the card and the canvas editor list
"Kick (Band 30-120 Hz)", never "Kick" alone, so a name can't pass for another kind of
signal.

`id`, `name`, `version`, `api` and `entry` are required, and a key at the top level the
manifest doesn't know refuses it. `meta` can be left out, and so can each key in it
(`Meta`, `manifest.rs:73`). The enable card shows `meta.author` and `meta.description`
when they're there, with the top-level `version`. rox reads `meta.website`,
`meta.version` and `meta.license` and shows none of them.

The entry resolves the way a plugin's does (`entry_for`, `manifest.rs:668`). A script
entry runs `<interpreter> <path>`. For `"interpreter": "python3"` rox tries `python3`,
then `python`, with the `py -3` launcher ahead of both on Windows, and takes the first
that answers `--version` with "Python 3" inside 5 seconds (`aliases`, `manifest.rs:797`;
`speaks_python3`, `:883`). The user needs a Python 3 on PATH, or in a folder listed under
Program Folders on the Plugins page. A native entry maps `<os>-<arch>` to a file in the
package:

```json
"entry": {
  "native": {
    "linux-x86_64": "plasma",
    "macos-aarch64": "plasma",
    "windows-x86_64": "plasma.exe"
  }
}
```

A renderer gets no settings from rox: nothing in `hello` comes from `config_schema`.

## Lifecycle

A renderer runs one process per render target. Two targets naming the same renderer are
two processes. A target's `owner` is `plugin:<id>` (`renderer_id`,
`rox/src/canvas/target.rs:1960`), and the canvas starts its renderer the first time it
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
| `ROX_FRAMES_WIDTH`, `ROX_FRAMES_HEIGHT` | The target's size in pixels |
| `ROX_FRAMES_FD` | `3`, on Unix only |
| `ROX_COVER_FD` | `4`, on Unix only, and only for a renderer granted the cover |
| `ROX_TEXTURE_FD` | The texture's fd, on Linux only, and only for a renderer lent one: `4`, or `5` when the cover is lent too |

On Unix the renderer leads its own process group and gets its handles from fd 3 up in a
fixed order: the frame mapping at 3 always, then the cover mapping when it's granted,
then the GPU texture when it's lent (`places`, `rox-services/src/renderers.rs`). Each is
named in the environment and in `hello`, so read the number from there rather than
counting. Every fd past the last one lent is marked to close at exec (`lend` and
`close_from`, `process.rs:204-288`).
rox writes `hello` as soon as the process runs. A renderer that can't be spawned, or
whose `hello` can't be written, counts as an exit (`launch`, `renderers.rs:333`).

A target pauses its renderer when nothing shows it. On a canvas that draws, a target
item the canvas skips is paused straight away with `{"pause":true}` (`hold`,
`target.rs:645`): one that's hidden, one a state takes off the stage outside design
mode, and one with a corner behind the camera or its quad off the stage. A target inside
a group that's hidden, or that a state takes off the stage, isn't paused that way. It
waits, as a target does when its canvas isn't drawn at all, as with a hidden window or a
tab behind another: a frame left untaken for 500 ms pauses it (`LIMITS.idle`,
`target.rs:109`). Shown again, it gets `{"pause":false}`.

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
fifth stops it (`BACKOFF` and `CRASH_WINDOW`, `rox-plugins/src/host.rs:26-34`; `due`
and `note_exit`, `renderers.rs:458-466`). A stopped target draws empty, with a dashed
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

- Deleting the item, changing its resolution and closing the canvas close the
  renderer's stdin, wait up to 200 ms for it to exit, then kill its process group
  (`Session::stop` and `GRACE`, `renderers.rs:41` and `:436`). A new resolution is a new
  session with a new mapping.
- Switching the renderer off, removing it and a change to its files kill it at once
  (`enforce`, `renderers.rs`). So does quitting rox (`stop_all`). Turning Enable Plugins
  off doesn't: it stops the plugins and leaves renderers running.

Kill is `SIGKILL` to the process group on Unix. A renderer has no shutdown to answer
and nothing to save. Exiting when stdin closes is all rox asks.

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

The session's size is fixed while it runs, as a target's is. Size in the panel's settings
picks how:

| Size | What the panel shows |
|---|---|
| Fixed, the default, 1280 by 720 | That resolution, chosen from the same presets and fields as the canvas inspector, scaled to Fit (the whole picture, with the panel's background in the bands) or Fill (the panel covered, the overflow cropped through the texture's own crop) |
| Match the Panel | The panel's size in device pixels, held to 16 to 3840 by 16 to 2160 and rounded down to even. After a resize the renderer starts again at the new size once the size has held for half a second with no mouse button down, and the last frame shows stretched meanwhile (`SETTLE` and `matched_size`) |

The pointer over the picture goes to the renderer, mapped through the fit or the crop, as
over a canvas target outside design mode. The right button is the panel's: its press opens
the panel menu and never reaches the renderer. A pointer over the bands around a fitted
picture is off the surface, and the renderer gets `leave`.

A panel behind another tab, or in a minimised window, isn't drawn, so its session pauses
after the untaken frame's 500 ms, as a canvas target's does. Pop Out and Dock Back carry
the session as they carry a canvas's targets: one on shared memory goes on with the same
process, and one lent a texture starts again in the new window.

## The mapping

One block of shared memory per session, little-endian, made by rox
(`Mapping::create`, `rox-frames/src/map.rs:53`). A 4096-byte header is followed by two
slots. Offsets are bytes from the start of the block or the slot.

| Header offset | Field | Type | Value |
|---|---|---|---|
| 0 | magic | 4 bytes | `RXF1` |
| 4 | version | u32 | 1 |
| 8 | width | u32 | pixels across |
| 12 | height | u32 | pixels down |
| 16 | stride | u32 | `width * 4` |
| 20 | slots | u32 | 2 |
| 24 | slot_bytes | u64 | `64 + stride * height`, rounded up to a multiple of 4096 |

| Slot offset | Field | Type | Meaning |
|---|---|---|---|
| 0 | seq | u64 | The frame counter: 0 never written, odd while written, even when whole |
| 8 | stamp_ns | u64 | The renderer's clock when the frame was finished, in nanoseconds |
| 16 | width | u32 | The frame's width, which has to equal the header's |
| 20 | height | u32 | The frame's height, which has to equal the header's |
| 24 to 63 | | | Unused |
| 64 | pixels | `stride * height` bytes | RGBA8, straight alpha, sRGB, top row first, rows packed with no padding |

Slot `n` starts at `4096 + n * slot_bytes`, and the block is `4096 + 2 * slot_bytes`
long (`Spec`, `rox-frames/src/layout.rs:63`). The 4096 is the layout's page on every
platform, independent of the OS page size. At 1280 by 720 the stride is 5,120 bytes, a
frame 3,686,400, a slot 3,690,496 and the block 7,385,088.

The rules on rox's side:

- rox makes the block, writes the header, seals it where the platform can, and keeps
  only a read-only view. On Linux it's a `memfd` sealed with `F_SEAL_SHRINK`,
  `F_SEAL_GROW` and `F_SEAL_SEAL` (`seal`, `rox-frames/src/map/unix.rs:203`), so a
  renderer can't resize it under rox's reads. It never maps memory a renderer made, and
  never maps anything executable.
- Every length and offset rox uses comes from the size it asked for. It never reads the
  header back, so a renderer may write the whole block, header included, and changes
  nothing by it. The header is there for the renderer to check.
- No lock is shared. The counter in each slot is the whole protocol.

The pixel format is the texture's: `Rgba8UnormSrgb` (`patches/gpui/c6-dynamic-user-textures.patch`),
fed straight-alpha bytes (`Frame`, `rox/src/canvas/texture.rs:24`). An alpha below 255
shows the canvas behind the target.

## Writing a frame

Frame `n` counts from 0 and goes to slot `n % 2`:

1. Store `2n + 1` in the slot's `seq`. An odd counter marks the slot as mid-write.
2. Store the frame's width and height at 16 and 20, and its pixels from 64.
3. Store the clock in `stamp_ns`: `CLOCK_MONOTONIC` on Unix and the system clock since
   the Unix epoch on Windows (`clock_ns`, `rox-frames/src/lib.rs:119`). It's for
   latency figures, and nothing about what rox shows depends on it.
4. Store `2n + 2` in `seq`. An even counter marks one whole frame.
5. Write `f <n> <slot>\n` to stdout and flush.

Another process has to see those stores in that order: the odd counter before any
pixel, and every pixel before the even counter. `rox_frames::Writer` puts a release
fence after each counter store (`Writer::frame`, `rox-frames/src/write.rs:45`). Plain
stores in program order give the same on x86-64. On ARM a store can become visible out
of order without a fence. A language with no fence leans on its runtime there, and the
worst that comes of it is one frame shown with some of the previous frame's pixels. No
order a renderer writes in can fault rox.

When an announce arrives, rox reads the slot's counter. It throws the frame away as torn
when the counter is odd, or when it moved by the time the copy finished. It refuses the
frame when the counter is 0 or the slot's width and height aren't the target's. A whole
frame is shown only if its counter is higher than the last one shown in this session,
so the counter only ever goes up (`copy`, `rox/src/canvas/target.rs:1180`). The frame
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
the slot, and `rox_frames::announce(n, slot)` makes the line.

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
`COVER_MEMO`, `rox/src/canvas/target.rs:1424-1451`).

The handle a renderer gets can only read:

- On Linux the block is a `memfd`. rox maps its own writable view, then seals the file
  with `F_SEAL_FUTURE_WRITE` along with `F_SEAL_SHRINK`, `F_SEAL_GROW` and `F_SEAL_SEAL`
  (`seal_cover`, `rox-frames/src/map/unix.rs:136`). From then on a `write` and a shared
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

## The wire

### rox to the renderer

One JSON object a line on stdin, UTF-8. Read whole lines of any length: a pool line can
run past a pipe's atomic write size, and rox finishes a line it wrote in part before it
starts the next. Read each line as JSON. Key order and number spelling aren't part of
the contract, and a line or key a renderer doesn't know should be skipped.

These are every line rox writes (`target.rs:1327-1592`):

```json
{"hello":1,"width":1280,"height":720,"handle":0}
{"hello":1,"width":1280,"height":720,"handle":0,"cover":{"fd":4,"width":256,"height":256}}
{"hello":1,"width":1920,"height":1080,"handle":0,"texture":{"fd":4,"width":1920,"height":1080,"allocation_size":8847360,"memory_type":0,"handle_type":"opaque-fd","format":37,"view_formats":[43,37],"flags":8,"usage":31,"tiling":0,"layout":1,"device_uuid":"00000000100000000000000000000000","driver_uuid":"414d442d4d4553412d44525600000000"}}
{"pool":[{"id":1,"name":"Kick","kind":"band","low":30,"high":120},{"id":2,"name":"onset 30-120 Hz","kind":"onset","low":30,"high":120},{"id":3,"name":"level","kind":"level"},{"id":4,"name":"total of 1","kind":"aggregate"}]}
{"signals":[0.3958,0.0,1.0],"time":4.224}
{"pointer":{"kind":"move","x":0.27,"y":0.36}}
{"palette":{"background":"#121212","surface":"#181818","text":"#c0c0c0","muted":"#808080","accent":"#ffb300","highlight":"#facc15","dark":true,"stage":"#121212"}}
{"cover":2}
{"cover":null}
{"pause":true}
```

`hello` is always first. Its width and height are the target's, and `handle` is the
mapping's handle value on Windows and 0 elsewhere. A renderer granted the cover finds it
under `cover`: `fd` 4 on Unix, `handle` on Windows, the section's handle value in the
renderer, and the square's `width` and `height`, both 256. Without the grant `hello` has no
`cover` key. A renderer lent the GPU texture finds what it needs to import it under
`texture` ([hello's texture](#hellos-texture)). One that wasn't lent it, for any reason,
gets no `texture` key.

`pool` describes the signal pool by id, in pool order. A name is the user's, trimmed
and cut at 64 characters, or one worded in English from the signal's source: `band
30-120 Hz`, `onset 30-120 Hz`, `beat 30-120 Hz`, `trigger 30-120 Hz`, `level`, or
`total of <id>` (`wire_name`, `target.rs:1784`). Read what a signal is from `kind`, since
a renamed one says nothing about it in its name: `band`, `level`, `onset`, `beat`,
`trigger` or `aggregate`, a running total of another signal that wraps at 1
(`wire_signal`, `target.rs:1752`). A `band`, `onset`, `beat` or `trigger` also has `low`
and `high`, the frequency range it listens to in whole hertz, kept between 20 and 20,000
(`Source::band`, `rox-viz/src/signal.rs:65`). Past the name an entry holds only those
fixed words and numbers, and nothing about how rox shapes the value. The line comes once
the session runs and the canvas draws, and again whenever the pool changes. An empty pool
is `{"pool":[]}`.

`signals` holds each pool signal's value in pool order, from 0 to 1, to 4 decimals, the
value routes read (`signals_line`, `target.rs:1799`). `time` is the playing position in
seconds, to 3 decimals, and 0 with nothing playing. A line goes with each frame the
canvas paints while music plays. When playback pauses or stops, lines keep coming for
about a second while the signals settle, then none come until music plays again
(`Targets::read`, `target.rs:2049`). The canvas paints when a renderer publishes a
frame, so a renderer that stops drawing stops getting signals. A reading rox hasn't sent
yet is replaced by a newer one.

`pointer` is the pointer over the target's surface (`Pointer::line`, `target.rs:1557`):

| `kind` | Fields |
|---|---|
| `move` | `x`, `y` |
| `down`, `up` | `x`, `y`, `button`: `left`, `right`, `middle`, `back` or `forward` |
| `scroll` | `x`, `y`, `dx`, `dy` in lines, 20 pixels a line for a wheel that reports pixels |
| `leave` | none |

`x` and `y` run from 0 to 1 across the surface from its top-left, to 4 decimals, mapped
back through the item's tilt the way it's drawn. rox sends them only outside design
mode, and only while the target is the front item under the pointer with no On Click
command rox can run on it or on a group around it (`hit`, `rox/src/panels/canvas.rs:2710`). Moves
go at most every 8 ms with the newest position. At most 64 events wait for a renderer
that's behind: moves are dropped first, then scrolls, then leaves, and presses last
(`Outbox`, `target.rs:1599`). `leave` comes when the pointer goes to another item or off
the canvas, and when design mode comes on, and it ends any press. An `up` can come with
no `down` before it, from a press that started elsewhere.

`palette` goes only to a renderer granted it: the theme's colours as `#rrggbb`, under
fixed names, and whether the theme is dark (`palette_line`, `target.rs:1620`):

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
square is transparent (`cover_line`, `target.rs:1418`). It comes when the session
starts, then each time the playing track changes, between frames, without a redraw. The
gap between two tracks of one queue doesn't count: the last cover holds, as the
backdrop's does.

`pause` is `true` when nothing shows the target, or while it rests with the music, and
`false` once it's shown and not resting again.
While paused, the palette and the cover wait too: rox writes nothing into the cover
mapping, and on the resume the current palette and cover go once if they changed.

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
(`offer_signals`, `rox/src/panels/canvas.rs:2423`). Add wears the accent, as the Place
confirm for an object does, since adding is undone by deleting the signals in the Signals
window. Escape and Not Now leave the pool as it was. While any are still missing, the
target's inspector shows a Signals row, "Not in the pool yet:" and the list, with an Add
Signals button. Both add them the way an object's signals are merged: fresh ids, and a
name already taken gets a number after it (`add_wanted_signals`, `:2464`).

What's missing is worked out by kind and band, never by name: a wanted `band` from 30 to
120 Hz is answered by any pool band from 30 to 120 Hz, whatever the user called it, with
the band rounded to whole hertz as the pool line carries it. An `aggregate` is answered by
a total of whatever answers its input (`missing_signals` and `signals_to_add`,
`rox-services/src/renderers.rs:627-640`). Nothing is added outside design mode, nothing
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

Once the audio has been silent for 0.15 s (`SILENT_AFTER`), a band and the level fall
to 0 at their falling rate, an onset's average falls at 2 a second and it re-arms, a
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
texture. A line is at most 64 bytes, newline included, and `\r\n` is refused
(`ANNOUNCE_MAX`, `parse_announce` and `parse_texture_announce`, `rox-frames/src/lib.rs`).
Any other line, or a longer one, is dropped without a log line, and still counts toward
the flood rule below, as `t` lines do. Of the lines waiting when rox reads, only the
newest announce is acted on, `f` or `t` (`read`, `target.rs`).

stderr goes to rox's log a line at a time as `renderer <id>: <line>`, cut at 4 KiB
(`drain_stderr` and `STDERR_LINE`, `rox-plugins/src/process.rs:22` and `:336`).

## Limits

| | Limit | Where |
|---|---|---|
| Resolution | 16 to 3840 by 16 to 2160 pixels, 1280 by 720 for a new target | `WIDTHS`, `HEIGHTS`, `renderers.rs:36`; `WIDTH`, `HEIGHT`, `layout.rs:24-26` |
| An announce line | 64 bytes, newline included | `ANNOUNCE_MAX`, `lib.rs:39` |
| stdout lines | More than 500 in a second, two seconds in a row, stops the renderer | `LIMITS`, `target.rs:109`; `Rate`, `:1799` |
| Refused frames | 60 in a row stop the renderer | `LIMITS.refused_run` |
| An untaken frame | 500 ms, then the renderer is paused | `LIMITS.idle` |
| Restarts | 0, 1, 2, 4 s; the fifth exit in ten minutes stops it | `BACKOFF`, `host.rs:26` |
| Stop | 200 ms after stdin closes, then killed | `GRACE`, `renderers.rs:41` |
| A pool name | 64 characters | `NAME_MAX`, `target.rs:90` |
| Pointer events waiting | 64; moves 8 ms apart | `POINTER_QUEUE`, `MOVE_GAP`, `target.rs:82-86` |
| A stderr line | 4 KiB | `STDERR_LINE`, `process.rs:22` |
| Wanted signals | 16 | `MAX_WANTED_SIGNALS`, `manifest.rs:176` |
| A wanted signal's name | 32 characters | `WANTED_NAME_MAX` |
| The cover | 256 by 256 pixels | `SIZE`, `rox-frames/src/cover.rs` |
| The GPU texture | One per target, at its resolution; Linux only; the canvas's wait to make it is 2 s, then the session runs on slots | `LIMITS.texture_wait`, `target.rs` |
| `t` from a session lent no texture | Counts as a refused frame: 60 in a row stop the renderer | `show_texture`, `target.rs` |

The flood rule counts lines in one-second spans from the first line, and stops a
renderer whose current span passes 500 right after one that did. A renderer that
announces once a frame is far inside it at any display rate.

## What a renderer is sent

`hello`, `pause`, `pool`, `signals` with `time`, and `pointer`, and nothing else unless
its approved manifest asks. The enable card names the two grants behind them: "Gets the
music's signals and the playing position" and "Gets the pointer while it's over the
render target" (`plugins::grants`, `rox-services/src/plugins.rs`). A renderer that asks
for the palette also gets `palette` lines, and one that asks for the cover gets the cover
mapping, fd 4 and `ROX_COVER_FD`, `cover` in `hello`, and `cover` lines. Each has its own
card line. The grants are read from the manifest the gate approved when the session
starts (`Session::asks`, `rox-services/src/renderers.rs`), so a renderer whose files
change can't widen them without the card. A renderer that asks for nothing gets exactly
what it got before these grants existed.

A renderer isn't told the track, a path, a key press, or anything about another item,
and its wire has no way to call into rox. The cover is pixels with a counter. The
palette says something about the track too: with song theming on, the theme's colours
follow the playing track's cover. A renderer granted the palette gets a new `palette`
line whenever a cover brings other colours, which tells it the album changed, and gets
that cover's own two colours as `art_primary` and `art_secondary`. It isn't told which
album. Its card line says so: the theme's colours follow the cover when song theming is
on. It runs with the user's permissions, unsandboxed, as a plugin does, and the card says so.

A manifest that gains a grant is a changed manifest. The renderer switches off, and its
card lists the new grant under "Changed since you last switched it on" (`Change::Grant`).
Asking for other signals than before counts the same way.

A workspace or an object with a render target lists the renderer in `requires`, the way
it lists a plugin, and never includes it (`named_renderers`, `target.rs:1971`). An
exported object carries the pool signals its routes read, and the ones those totals
follow, but not the pool a render target's renderer reads (`lift`, `rox/src/objects.rs`).
Placed in another rox, the renderer reads that rox's pool.

## Platforms

On Linux the block is a sealed `memfd`, lent as fd 3 with `ROX_FRAMES_FD=3`, and
`hello`'s `handle` is 0. The cover is a second `memfd`, future-write sealed, lent as fd 4
with `ROX_COVER_FD=4`. The GPU texture is lent on Linux alone, after the cover. On macOS
and Windows gpui answers unsupported, rox never asks the canvas for one, and every target
draws through the slots. That code compiles on both by `cfg`, and no macOS or Windows
build of it has been made.

On macOS the design is the same fd 3 handoff, with the block a POSIX shared memory
object under a random name that's unlinked as soon as it's open. macOS has no seals.
The design relies on a shared memory object refusing a second `ftruncate`, which would
keep a renderer from shrinking it (`open` and `seal`, `rox-frames/src/map/unix.rs`).

On Windows the design is an unnamed section backed by the page file, duplicated into
the renderer with map read and map write rights only (`CHILD_ACCESS`,
`rox-frames/src/map.rs:35`; `lend`, `rox-plugins/src/process.rs:525`). There's no
`ROX_FRAMES_FD`. The section's handle value in the renderer is `hello`'s `handle`, which
a renderer maps with `MapViewOfFile` for the block's whole length. rox's pipe to a
renderer's stdin is blocking there, so the lines go through a writer thread that holds
up to 16 before rox counts the pipe as full (`WRITER_LINES`, `target.rs:334`). The
renderer starts with no console window and inside a job object, as a plugin does. The
cover is a second section, duplicated with map read alone, and its value is `hello`'s
`cover.handle`.
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

The canvas paints for a target when it publishes a frame, so a renderer that stops
publishing stops getting signals, and they start again with its next frame.

### What a native renderer starts with

The process starts in the package's folder in an environment built from an empty one,
and nothing else of rox's reaches it (`prepare`, `rox-plugins/src/env.rs`; `spawn_with`,
`rox-plugins/src/process.rs`; `launch`, `rox-services/src/renderers.rs`). It isn't a
sandbox: the renderer runs as the user, as a plugin does. It only keeps a token or key
rox was started with from being handed on.

| Variable | Where it comes from |
|---|---|
| The names in [the plugin environment table](10-plugins.md#process-lifecycle) | rox's own value, for each one rox has set |
| Names listed in `ROX_PLUGIN_ENV` | rox's own value, while a developer [passes more](10-plugins.md#passing-more-while-testing) |
| `PATH` | The plugin search path: the package's `bin` folder, the Program Folders from the Plugins page, then rox's own `PATH` (`search_path`, `rox-plugins/src/search.rs`) |
| `PYTHONDONTWRITEBYTECODE`, `PYTHONUNBUFFERED`, `PYTHONUTF8` | `1` each |
| `ROX_RENDER_API`, `ROX_FRAMES_WIDTH`, `ROX_FRAMES_HEIGHT` | rox, always |
| `ROX_FRAMES_FD`, `ROX_COVER_FD`, `ROX_TEXTURE_FD` | rox, on Unix, each only when that handle is lent. The [Lifecycle](#lifecycle) table has their values |

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
with no copy through the CPU. rox makes the texture in the window that shows the target,
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

One texture is made per target and lent to every session of it, so a restart imports the
same memory. A new resolution, a move to another window and a switch-off each let it go,
and the next session is lent a new one.

### hello's texture

The values are the image's create info as raw Vulkan numbers, for an importer to repeat
exactly (`texture_hello`, `rox-services/src/renderers.rs`):

| Key | Vulkan | Value |
|---|---|---|
| `fd` | The memory, `VK_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_FD_BIT` | the same number as `ROX_TEXTURE_FD` |
| `width`, `height` | `VkImageCreateInfo::extent` | the target's resolution |
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
2. Write every pixel. Either store into the `Rgba8Unorm` image with the sRGB curve applied
   in the shader, since a storage image can't be sRGB, or render through an
   `R8G8B8A8_SRGB` view and let the hardware apply it. rox samples through an sRGB view
   either way, and both were pixel-exact in the spike. Alpha is straight, as in a slot.
3. Release: the barrier back to `VK_QUEUE_FAMILY_EXTERNAL`, after your last write,
   leaving the image in `GENERAL`, the layout rox samples it in. A render pass leaves it
   in `COLOR_ATTACHMENT_OPTIMAL`, so either move it back to `GENERAL` first or make that
   the release barrier's old layout.
4. Submit, with the acquire ahead of the draw and the release after it in submission
   order, and wait on your own fence for that submission.
5. Write `t <n>\n` and flush.

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
| A new resolution | The target is new: the old texture is released and a new one made |
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
| `renderer <id>: started at <w>x<h>` | The process runs and `hello` went |
| `renderer <id>: started at <w>x<h>, lent a texture` | The same, with the GPU texture lent |
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
| `render target <id>: session ended, <n> frames copied, <n> texture frames, <n> torn, <n> refused (<n> unwritten, <n> wrong size, <n> texture frames with no texture), <n> lines read` | A session ended, for any reason, and what its frames came to |

The inspector's Status row words the same states for the user, with the gate's reason
when it refused. rox writes no line for a frame it drops, only a count in the line each
session ends with (`log_tally`, `target.rs:1240`). Frames copied are whole frames read
out of a slot. Torn ones had an odd counter, or one that moved during the copy.
Unwritten ones were announced while the slot's counter was still 0, and wrong size ones
had size words other than `ROX_FRAMES_WIDTH` and `ROX_FRAMES_HEIGHT`. Texture frames are
`t` lines rox showed, and texture frames with no texture are `t` lines from a session that
wasn't lent one. Lines read counts
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

A renderer can't run without a block rox made, so testing one outside rox takes a
stand-in host that does what this doc describes: make the block, write the header, put
it at fd 3, start the renderer with the four variables, write `hello`, and read the
announce lines. One granted the cover also wants the cover block at fd 4.

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
`crates/rox/src/canvas/cost.rs`).

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

## Reference

The shared memory is `crates/rox-frames`: `layout.rs` (the offsets, `Spec`, the ranges),
`map.rs` with `map/unix.rs` and `map/windows.rs` (making, sealing and lending the block),
`read.rs` (`copy_slot`, `Read`, `Refusal`), `write.rs` (`Writer`, behind the `writer`
feature), `cover.rs` (the cover mapping: its layout, `fit`, `letterbox`, `Cover`, and
`CoverReader` behind the `cover-reader` feature) and `lib.rs` (`ANNOUNCE_MAX`,
`parse_announce`, `announce`, `clock_ns`).

The package side is `crates/rox-services/src/renderers.rs` (the listing, the gate,
`asks`, `start`, `Session`, the exit count, `enforce`, and the wanted signals' matching)
on top of `crates/rox-plugins`: `manifest.rs` (`capabilities.renderer`, `check_renderer`,
`check_wanted`, the entry and the interpreter table), `process.rs` (the spawn, fd 3 and up,
the Windows handles, stderr, `never_block`) and `host.rs` (`BACKOFF`, `CRASH_WINDOW`). The
approval card's grants are `grants` in `crates/rox-services/src/plugins.rs`, with the
re-approval diff in `changes`.

The item is `crates/rox/src/canvas/target.rs` (the supervisor, the limits, the reader,
the copy, the texture's handoff in `Lend` and `Supervisor::texture`, every line sent, the
cover's decode, the status), drawn through `crates/rox/src/canvas/texture.rs` (`Picture`,
`Lender`, and the external texture's registration and paint). The texture itself is
`Window::register_external_texture` in the vendored gpui (`patches/gpui/e5-external-user-textures.patch`)
over blade's `export_texture` (`patches/blade-graphics/a0-shared-textures.patch`). `crates/rox/src/panels/canvas.rs` places it, reads
the signals, the palette and the stage for it each frame, follows the playing track for its cover,
sends it the pointer (`hit`, `point_at`, `button_name`, `scroll_lines`), adds it from the
Add menu, and offers its wanted signals (`offer_signals`). The prompt and the inspector's
Signals row are in `crates/rox/src/canvas/overlay.rs`. A pool entry's kind and band come from `Source` in
`crates/rox-viz/src/signal.rs`. The Renderer panel is `crates/rox/src/panels/renderer.rs`,
over the same `Shown` session the canvas uses. The Renderers settings page is
`crates/rox/src/settings/window/renderers_page.rs`, drawing its rows with the row code in
`plugins_page.rs`, and the enable card both pages and both hosts raise is
`crates/rox/src/enable_card.rs`.

The examples are `examples/renderers/bars/bars.py` and
`examples/renderers/plasma/src/main.rs`, each with a `plugin.json` and a README.
