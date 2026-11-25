# Renderers

How a renderer is built and how to write one: the package, the process rox starts for a
render target, the shared memory its frames arrive through, the lines it reads and
writes, and the rules that stop it. This makes
[ADR 37](../02-architecture/decisions/37-adr-render-targets.md) concrete, with its
amendments of 2026-11-16 (renderers are listed apart from plugins) and 2026-11-23 (shared
memory is rox's own channel). Underneath, a renderer is the package of
[ADR 30](../02-architecture/decisions/30-adr-plugins.md) under its renderer amendment:
the same folder or `.roxp`, the same hash, approval and switch, so
[10-plugins.md](10-plugins.md) holds for everything this doc doesn't say otherwise. A
frame is a stranger's bytes under
[ADR 35](../02-architecture/decisions/35-adr-package-security.md). It belongs to the
plugin host in [components](../02-architecture/02-components.md#plugin-host). Two
examples are there to copy: [`examples/renderers/bars`](../../examples/renderers/bars/)
in Python's standard library, and [`examples/renderers/plasma`](../../examples/renderers/plasma/)
in Rust on rox's own `rox-frames`.

Version-sensitive: rox starts a renderer with `ROX_RENDER_API=1`, and `hello` says 1
(`API`, `rox-services/src/renderers.rs:30`). The shared memory is layout version 1 behind
the magic `RXF1` (`VERSION` and `MAGIC`, `rox-frames/src/layout.rs:10-12`). The manifest
is plugin API 1 (`SUPPORTED_API`, `rox-plugins/src/manifest.rs:24`). `rox-frames` uses
libc 0.2 on Unix (0.2.186 in rox's lockfile, 0.2.190 in the plasma example's) and
windows-sys 0.61 (0.61.2) on Windows. What [A GPU renderer](#a-gpu-renderer) says of wgpu
was read in wgpu 30.0.2, which rox doesn't depend on.

## Trying the examples

1. Turn on Enable Plugins at the top of Settings > Plugins. Renderers are behind the
   same switch, and none starts while it's off (`check`,
   `rox-services/src/renderers.rs:137`).
2. Copy `examples/renderers/bars` into the plugins folder (Reveal Folder on the Plugins
   page opens it), or install a `.roxp` packed from it. It lists under Renderers,
   switched off.
3. Switch it on. The card says it draws in render targets on a canvas, gets the music's
   signals and the playing position, and gets the pointer while it's over the render
   target. Switch On approves exactly the files in the package, as it does for a plugin.
4. Add a Canvas panel: an empty window offers one, and the Panels menu lists it under
   Arrangement. With Panels > Design Mode on, right-click empty stage, or press the plus
   in the canvas inspector's header, and pick Add Render Target > Bars (`open_add_menu`,
   `rox/src/panels/canvas.rs:2124`). The new target draws at 1280 by 720 pixels on a
   640 by 360 unit item (`TARGET_PIXELS` and `TARGET_UNITS`, `canvas.rs:2971-2972`).
   The inspector's Resolution row changes the pixel size, and Status says whether the
   renderer runs.
5. Leave design mode and play something. Bars draws a bar for each signal in the pool
   (Application > Signals edits it), so a pool with nothing in it draws one empty bar.

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
    "renderer": { "label": "Bars" }
  }
}
```

`capabilities.renderer.label` is what the Add Render Target menu lists. A blank one
refuses the manifest with "the renderer has no label". A renderer that also declares a
`source`, any `panels` or a true flag under `providers` is refused with "a renderer
declares nothing else, and this one declares ..." (`check_renderer`,
`rox-plugins/src/manifest.rs:331`), because those would give it a plugin host and the
plugin wire. Unknown keys inside `capabilities` are ignored, as they are for a plugin.

`id`, `name`, `version`, `api` and `entry` are required, and a key at the top level the
manifest doesn't know refuses it. `meta` can be left out, and so can each key in it
(`Meta`, `manifest.rs:73`). The enable card shows `meta.author` and `meta.description`
when they're there, with the top-level `version`. rox reads `meta.website`,
`meta.version` and `meta.license` and shows none of them.

The entry resolves the way a plugin's does (`entry_for`, `manifest.rs:483`). A script
entry runs `<interpreter> <path>`. For `"interpreter": "python3"` rox tries `python3`,
then `python`, with the `py -3` launcher ahead of both on Windows, and takes the first
that answers `--version` with "Python 3" inside 5 seconds (`aliases`, `manifest.rs:612`;
`speaks_python3`, `:698`). The user needs a Python 3 on PATH, or in a folder listed under
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
`rox/src/canvas/target.rs:1688`), and the canvas starts its renderer the first time it
draws the item. The start goes through the gate on a thread of the target's own: Enable
Plugins on, the package approved for its current hash, switched on, and not stopped by
exits (`start`, `rox-services/src/renderers.rs:237`). rox never starts a renderer at
launch, only when a target asks.

The process starts in the package's folder (for a `.roxp`, rox's unpacked copy under
`plugin-cache`) with rox's environment, PATH set to the plugin search path, and
`PYTHONDONTWRITEBYTECODE=1`, `PYTHONUNBUFFERED=1` and `PYTHONUTF8=1`
(`spawn_with`, `rox-plugins/src/process.rs:72`). [What a native renderer starts
with](#what-a-native-renderer-starts-with) has the whole of it. On top of those:

| Variable | Value |
|---|---|
| `ROX_RENDER_API` | `1` |
| `ROX_FRAMES_WIDTH`, `ROX_FRAMES_HEIGHT` | The target's size in pixels |
| `ROX_FRAMES_FD` | `3`, on Unix only |

On Unix the renderer leads its own process group, gets the mapping as fd 3, and every
fd past 3 is marked to close at exec (`lend` and `close_from`, `process.rs:185-262`).
rox writes `hello` as soon as the process runs. A renderer that can't be spawned, or
whose `hello` can't be written, counts as an exit (`launch`, `renderers.rs:289`).

A target pauses its renderer when nothing shows it. On a canvas that draws, a target
item the canvas skips is paused straight away with `{"pause":true}` (`hold`,
`target.rs:583`): one that's hidden, one a state takes off the stage outside design
mode, and one with a corner behind the camera or its quad off the stage. A target inside
a group that's hidden, or that a state takes off the stage, isn't paused that way. It
waits, as a target does when its canvas isn't drawn at all, as with a hidden window or a
tab behind another: a frame left untaken for 500 ms pauses it (`LIMITS.idle`,
`target.rs:104`). Shown again, it gets `{"pause":false}`. While paused rox sends nothing
else, drops what it had queued, and reads nothing from the renderer's stdout. A renderer
that keeps announcing frames fills that pipe and blocks in its own write. rox looks for
an exit once a second while it's paused.

An exit rox didn't ask for leaves the last frame showing, and rox starts the renderer
again after 0, 1, 2 and 4 seconds for the first to fourth exit inside ten minutes. The
fifth stops it (`BACKOFF` and `CRASH_WINDOW`, `rox-plugins/src/host.rs:26-34`; `due`
and `note_exit`, `renderers.rs:371-379`). A stopped target draws empty, with a dashed
box and the renderer's name in design mode, until the user presses Start Again under
its Status, or switches the renderer off and on. A failed start, a flood of stdout
lines and a run of refused frames each count as an exit. A restarted renderer gets
`hello` and the pool again, and its frame numbers can start again from 0.

rox stops a session in two ways:

- Deleting the item, changing its resolution and closing the canvas close the
  renderer's stdin, wait up to 200 ms for it to exit, then kill its process group
  (`Session::stop` and `GRACE`, `renderers.rs:38` and `:349`). A new resolution is a new
  session with a new mapping.
- Switching the renderer off, removing it, a change to its files and turning Enable
  Plugins off kill it at once (`enforce`, `renderers.rs:415`). So does quitting rox
  (`stop_all`, `:450`).

Kill is `SIGKILL` to the process group on Unix. A renderer has no shutdown to answer
and nothing to save. Exiting when stdin closes is all rox asks.

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
  `F_SEAL_GROW` and `F_SEAL_SEAL` (`seal`, `rox-frames/src/map/unix.rs:118`), so a
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
   the Unix epoch on Windows (`clock_ns`, `rox-frames/src/lib.rs:114`). It's for
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
so the counter only ever goes up (`copy`, `rox/src/canvas/target.rs:1080`). The frame
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

## The wire

### rox to the renderer

One JSON object a line on stdin, UTF-8. Read whole lines of any length: a pool line can
run past a pipe's atomic write size, and rox finishes a line it wrote in part before it
starts the next. Read each line as JSON. Key order and number spelling aren't part of
the contract, and a line or key a renderer doesn't know should be skipped.

These are every line rox writes (`target.rs:1227-1370`):

```json
{"hello":1,"width":1280,"height":720,"handle":0}
{"pool":[{"id":1,"name":"Kick","kind":"band","low":30,"high":120},{"id":2,"name":"onset 30-120 Hz","kind":"onset","low":30,"high":120},{"id":3,"name":"level","kind":"level"},{"id":4,"name":"total of 1","kind":"aggregate"}]}
{"signals":[0.3958,0.0,1.0],"time":4.224}
{"pointer":{"kind":"move","x":0.27,"y":0.36}}
{"pause":true}
```

`hello` is always first. Its width and height are the target's, and `handle` is the
mapping's handle value on Windows and 0 elsewhere.

`pool` describes the signal pool by id, in pool order. A name is the user's, trimmed
and cut at 64 characters, or one worded in English from the signal's source: `band
30-120 Hz`, `onset 30-120 Hz`, `trigger 30-120 Hz`, `level`, or `total of <id>`
(`wire_name`, `target.rs:1284`). Read what a signal is from `kind`, since a renamed one
says nothing about it in its name: `band`, `level`, `onset`, `trigger` or `aggregate`, a
running total of another signal that wraps at 1 (`wire_signal`, `target.rs:1253`). A
`band`, `onset` or `trigger` also has `low` and `high`, the frequency range it listens
to in whole hertz, kept between 20 and 20,000 (`Source::band`,
`rox-viz/src/signal.rs:62`). Past the name an entry holds only those fixed words and
numbers, and nothing about how rox shapes the value. The line comes once the session runs
and the canvas draws, and again whenever the pool changes. An empty pool is
`{"pool":[]}`.

`signals` holds each pool signal's value in pool order, from 0 to 1, to 4 decimals, the
value routes read (`signals_line`, `target.rs:1299`). `time` is the playing position in
seconds, to 3 decimals, and 0 with nothing playing. A line goes with each frame the
canvas paints while music plays. When playback pauses or stops, lines keep coming for
about a second while the signals settle, then none come until music plays again
(`Targets::read`, `target.rs:1775`). The canvas paints when a renderer publishes a
frame, so a renderer that stops drawing stops getting signals. A reading rox hasn't sent
yet is replaced by a newer one.

`pointer` is the pointer over the target's surface (`Pointer::line`, `target.rs:1335`):

| `kind` | Fields |
|---|---|
| `move` | `x`, `y` |
| `down`, `up` | `x`, `y`, `button`: `left`, `right`, `middle`, `back` or `forward` |
| `scroll` | `x`, `y`, `dx`, `dy` in lines, 20 pixels a line for a wheel that reports pixels |
| `leave` | none |

`x` and `y` run from 0 to 1 across the surface from its top-left, to 4 decimals, mapped
back through the item's tilt the way it's drawn. rox sends them only outside design
mode, and only while the target is the front item under the pointer with no On Click
command rox can run on it or on a group around it (`hit`, `rox/src/panels/canvas.rs:2600`). Moves
go at most every 8 ms with the newest position. At most 64 events wait for a renderer
that's behind: moves are dropped first, then scrolls, then leaves, and presses last
(`Outbox`, `target.rs:1376`). `leave` comes when the pointer goes to another item or off
the canvas, and when design mode comes on, and it ends any press. An `up` can come with
no `down` before it, from a press that started elsewhere.

`pause` is `true` when nothing shows the target and `false` when something does again.

Closing stdin tells the renderer to exit.

### The pool

The pool is the user's, and there's one for the whole app. It's edited under
Application > Signals and saved with the live look in `workspace.json` in rox's data
folder, as `bundle.signals` (`LookState` and `WorkspaceBundle`,
`rox-core/src/settings.rs`). Every canvas and every render target reads that one pool,
and applying a workspace replaces it with the one the workspace carries. A renderer
can't add a signal or ask for one, since nothing on its wire goes that way. It gets the
pool the user made.

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
reads 0, and above it the output climbs from 0 again. On a trigger the threshold is the
fire level instead.

- `band` is the loudest bin of a 2048-point FFT of the mono mix between `low` and `high`,
  a peak and not a mean. It's in decibels against a full-scale sine, mapped from -66 dB
  at 0 to -12 dB at 1 and held between them (`FLOOR_DB` and `MAX_DB`). A loud master
  peaks above -12 dB in its bass much of the time, so a band there sits at or near 1.
  Response sets how fast it follows: it rises at 50 a second at Response 0 down to 3 at
  100, and falls at 12 down to 1.
- `level` is the same measure across the whole spectrum, 20 Hz to 20 kHz.
- `onset` is a pulse. When the band jumps more than 0.12 above its own slow average, and
  is above 0.15, the value goes to 1. It then decays each frame, at 16 a second at
  Response 0 down to 1.5 at 100. At 60 frames a second that's about an eighth of a
  second to fall to a tenth at Response 0, and about a second and a half at 100. A hit
  reads as a jump and a tail over many frames, never a single 1 between zeroes. It fires
  again only once the band has fallen back close to its average.
- `trigger` has the same shape, a jump to 1 and the same decay. It fires when the band
  reaches the Threshold and can't fire again until the band falls under three quarters
  of it. At Threshold 0 it never fires and reads 0.
- `aggregate`, Total in the Signals window, adds up another signal: each frame it grows
  by that signal's value times its Rate, at most 8 laps a second, and wraps from 1 to 0.
  A renderer using it as a phase has to unwrap it. When a value is more than half below
  the one before, it wrapped, so add 1. A smaller fall is a drain to 0, from Reset on
  Track at a new song or from Flush, which takes a moment and doesn't snap.

Once the audio has been silent for 0.15 s (`SILENT_AFTER`), a band and the level fall
to 0 at their falling rate, and a pulse finishes its decay.

### The renderer to rox

A renderer writes announce lines to stdout and nothing else: `f`, a space, the frame number in
decimal digits, a space, the slot `0` or `1`, and `\n`. A line is at most 64 bytes,
newline included, and `\r\n` is refused (`ANNOUNCE_MAX` and `parse_announce`,
`rox-frames/src/lib.rs:34-81`). Any other line, or a longer one, is dropped without a
log line, and still counts toward the flood rule below. Of the lines waiting when rox
reads, only the newest announce is acted on (`read`, `target.rs:1466`).

stderr goes to rox's log a line at a time as `renderer <id>: <line>`, cut at 4 KiB
(`drain_stderr` and `STDERR_LINE`, `rox-plugins/src/process.rs:22` and `:308`).

## Limits

| | Limit | Where |
|---|---|---|
| Resolution | 16 to 3840 by 16 to 2160 pixels, 1280 by 720 for a new target | `WIDTHS`, `HEIGHTS`, `renderers.rs:33`; `WIDTH`, `HEIGHT`, `layout.rs:24-26` |
| An announce line | 64 bytes, newline included | `ANNOUNCE_MAX`, `lib.rs:34` |
| stdout lines | More than 500 in a second, two seconds in a row, stops the renderer | `LIMITS`, `target.rs:104`; `Rate`, `:1531` |
| Refused frames | 60 in a row stop the renderer | `LIMITS.refused_run` |
| An untaken frame | 500 ms, then the renderer is paused | `LIMITS.idle` |
| Restarts | 0, 1, 2, 4 s; the fifth exit in ten minutes stops it | `BACKOFF`, `host.rs:26` |
| Stop | 200 ms after stdin closes, then killed | `GRACE`, `renderers.rs:38` |
| A pool name | 64 characters | `NAME_MAX`, `target.rs:85` |
| Pointer events waiting | 64; moves 8 ms apart | `POINTER_QUEUE`, `MOVE_GAP`, `target.rs:77-81` |
| A stderr line | 4 KiB | `STDERR_LINE`, `process.rs:22` |

The flood rule counts lines in one-second spans from the first line, and stops a
renderer whose current span passes 500 right after one that did. A renderer that
announces once a frame is far inside it at any display rate.

## What a renderer is sent

`hello`, `pause`, `pool`, `signals` with `time`, and `pointer`, and nothing else. The
enable card names the two grants behind them: "Gets the music's signals and the playing
position" and "Gets the pointer while it's over the render target" (`plugins::grants`,
`rox-services/src/plugins.rs`). A renderer isn't told the track, a path, a key press, or
anything about another item, and its wire has no way to call into rox. It runs with the
user's permissions, unsandboxed, as a plugin does, and the card says so.

A workspace or an object with a render target lists the renderer in `requires`, the way
it lists a plugin, and never includes it (`named_renderers`, `target.rs:1699`). An
exported object carries the pool signals its routes read, and the ones those totals
follow, but not the pool a render target's renderer reads (`lift`, `rox/src/objects.rs`).
Placed in another rox, the renderer reads that rox's pool.

## Platforms

On Linux the block is a sealed `memfd`, lent as fd 3 with `ROX_FRAMES_FD=3`, and
`hello`'s `handle` is 0.

On macOS the design is the same fd 3 handoff, with the block a POSIX shared memory
object under a random name that's unlinked as soon as it's open. macOS has no seals.
The design relies on a shared memory object refusing a second `ftruncate`, which would
keep a renderer from shrinking it (`open` and `seal`, `rox-frames/src/map/unix.rs`).

On Windows the design is an unnamed section backed by the page file, duplicated into
the renderer with map read and map write rights only (`CHILD_ACCESS`,
`rox-frames/src/map.rs:35`; `lend`, `rox-plugins/src/process.rs:491`). There's no
`ROX_FRAMES_FD`. The section's handle value in the renderer is `hello`'s `handle`, which
a renderer maps with `MapViewOfFile` for the block's whole length. rox's pipe to a
renderer's stdin is blocking there, so the lines go through a writer thread that holds
up to 16 before rox counts the pipe as full (`WRITER_LINES`, `target.rs:272`). The
renderer starts with no console window and inside a job object, as a plugin does.
Python's `mmap` maps a file or a named mapping and has no way to take a section handle,
so the Python example logs that and exits on Windows. The Rust example takes the handle
from `hello`.

## A GPU renderer

Both examples fill their slot from the CPU. A renderer that draws on the GPU reads each
frame back into the slot, and the contract above leaves it a few things to get right.

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

An onset or a trigger is a pulse: it jumps on a hit and falls back at the rate its
Response sets. Keep the highest value of each since the last step, and start again from
0 after the step, or a pulse that came and went between two steps is lost. A band, the
level and a total can take the newest value. The `kind` on the pool line says which is
which.

The canvas paints for a target when it publishes a frame, so a renderer that stops
publishing stops getting signals, and they start again with its next frame.

### What a native renderer starts with

The process starts in the package's folder with rox's own environment and these changes,
and no others (`spawn_with`, `rox-plugins/src/process.rs`; `launch`,
`rox-services/src/renderers.rs`):

- `PATH` is the plugin search path: the package's `bin` folder, the Program Folders from
  the Plugins page, then rox's own `PATH` (`search_path`, `rox-plugins/src/search.rs`).
- `PYTHONDONTWRITEBYTECODE`, `PYTHONUNBUFFERED` and `PYTHONUTF8`, each `1`.
- `ROX_RENDER_API`, `ROX_FRAMES_WIDTH` and `ROX_FRAMES_HEIGHT`, and on Unix
  `ROX_FRAMES_FD`.

rox adds nothing to `LD_LIBRARY_PATH` or `DYLD_LIBRARY_PATH` and sets no GPU variable. A
renderer finds its own GPU libraries, the Vulkan loader among them: from the system's
library path, from an rpath of its own, or through a launcher script of its own.

What it inherits depends on how rox was started. The Nix package in `flake.nix` finds
its own libraries through an rpath patched onto the `rox` binary (`patchelf --add-rpath`
in `postFixup`) and leaves the environment as it was. NixOS has no default library path
holding the Vulkan loader (`/run/opengl-driver/lib` holds the drivers, not
`libvulkan.so.1`), so there a renderer started by the packaged rox gets no loader from it.
The flake's dev shell puts the loader on `LD_LIBRARY_PATH`, and a rox run from that shell
passes it on. A renderer that runs under a dev build can fail to load `libvulkan.so.1`
under the package.

## Debugging

A renderer's stderr is in rox's log: `<data>/logs/rox.log`, rox's own stderr, and the
Console window (F12). rox's own lines about a target name the renderer's id:

| Line | Meaning |
|---|---|
| `renderer <id>: started at <w>x<h>` | The process runs and `hello` went |
| `render target <id>: didn't start: <why>` | The spawn or `hello` failed, as an exit |
| `render target <id>: not started: <why>` | The gate refused it |
| `render target <id>: the renderer exited` | stdout closed |
| `render target <id>: stopped, more than 500 lines a second` | The flood rule |
| `render target <id>: stopped, 60 refused frames in a row` | Wrong sizes or an unwritten slot, every time |
| `render target <id>: stopped after repeated exits` | The fifth exit |
| `render target <id>: stopped by rox` | A switch-off or a quit |
| `renderer <id>: stopped, no longer switched on` | The gate closed on a running session |
| `render target <id>: session ended, <n> frames copied, <n> torn, <n> refused (<n> unwritten, <n> wrong size), <n> lines read` | A session ended, for any reason, and what its frames came to |

The inspector's Status row words the same states for the user, with the gate's reason
when it refused. rox writes no line for a frame it drops, only a count in the line each
session ends with (`log_tally`, `target.rs:1140`). Frames copied are whole frames read
out of a slot. Torn ones had an odd counter, or one that moved during the copy.
Unwritten ones were announced while the slot's counter was still 0, and wrong size ones
had size words other than `ROX_FRAMES_WIDTH` and `ROX_FRAMES_HEIGHT`. Lines read counts
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
quits, and Pack beside it writes a `.roxp`. A running session keeps the code it started
with. Switching the renderer off and on starts the new code.

A renderer can't run without a block rox made, so testing one outside rox takes a
stand-in host that does what this doc describes: make the block, write the header, put
it at fd 3, start the renderer with the four variables, write `hello`, and read the
announce lines.

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

## Reference

The shared memory is `crates/rox-frames`: `layout.rs` (the offsets, `Spec`, the ranges),
`map.rs` with `map/unix.rs` and `map/windows.rs` (making, sealing and lending the block),
`read.rs` (`copy_slot`, `Read`, `Refusal`), `write.rs` (`Writer`, behind the `writer`
feature) and `lib.rs` (`ANNOUNCE_MAX`, `parse_announce`, `announce`, `clock_ns`).

The package side is `crates/rox-services/src/renderers.rs` (the listing, the gate,
`start`, `Session`, the exit count, `enforce`) on top of `crates/rox-plugins`:
`manifest.rs` (`capabilities.renderer`, `check_renderer`, the entry and the interpreter
table), `process.rs` (the spawn, fd 3, the Windows handle, stderr, `never_block`) and
`host.rs` (`BACKOFF`, `CRASH_WINDOW`). The approval card's grants are `grants` in
`crates/rox-services/src/plugins.rs`.

The item is `crates/rox/src/canvas/target.rs` (the supervisor, the limits, the reader,
the copy, every line sent, the status), drawn through `crates/rox/src/canvas/texture.rs`.
`crates/rox/src/panels/canvas.rs` places it, reads the signals for it each frame and
sends it the pointer (`hit`, `point_at`, `button_name`, `scroll_lines`), and adds it
from the Add menu. A pool entry's kind and band come from `Source` in
`crates/rox-viz/src/signal.rs`. The Renderers section of the Plugins page is
`crates/rox/src/settings/window/plugins_page.rs`.

The examples are `examples/renderers/bars/bars.py` and
`examples/renderers/plasma/src/main.rs`, each with a `plugin.json` and a README.
