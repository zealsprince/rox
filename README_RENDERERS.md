# Renderers

A renderer is a program that draws a picture inside rox. rox starts it, hands it a block
of shared memory to draw frames into, and sends it the music's signals and the playing
position as it plays. It draws in a Renderer panel or in a render target on a canvas, and
the user decides which renderers run and what each one gets.

A renderer is packaged the way a [plugin](README_PLUGINS.md) is: a manifest and a program,
shipped as one `.roxp` file or kept as a folder while you write it, in the same packages
folder. Past that the two have little in common. A plugin brings a music source into the
library and speaks JSON-RPC. A renderer adds nothing to the library and speaks a wire of
its own: JSON lines in, frame announcements out, pixels through the shared memory. It
lists on the Renderers settings page with a switch of its own, the Plugins page's Enable
Plugins doesn't reach it, and on Linux it runs in a sandbox, which a plugin doesn't.

This page is for installing and writing one. The full contract, every line on the wire
and every byte of the shared memory, is the
[renderer guide](docs/03-implementation/11-renderers.md).

## Where a renderer draws

- A Renderer panel shows one renderer filling a panel. An empty window offers one, and in
  design mode the Panels menu lists it under Visualizers. It docks, tabs and pops out like
  any panel. Pick a Renderer, on the panel itself or in its menu, chooses which one draws.
- A render target is a canvas item that shows a renderer's picture. With design mode on,
  right-click the empty stage, or press the plus in the canvas inspector's header, and
  pick Add Render Target. A target moves, scales and tilts with the rest of the canvas,
  and its Size, Render Scale and Fit rows in the inspector choose how many pixels the
  renderer draws and how the picture fills the box.

Each panel and each target runs its own copy of the renderer, and only while it's on
screen. One the window hides, or a target off the stage, is paused, and rox never starts
a renderer at launch.

A panel or a target with no picture says why where the picture would be: no renderer
picked, none installed, or the status of one that stopped, with a button that fixes it.
A canvas target does this outside design mode, and one meant to stay blank turns it off
with Explain When Empty in the inspector.

## Installing one

1. Open Settings > Renderers and press Install, then pick the renderer's `.roxp`. Or
   press Reveal Folder and put the package, or a renderer's folder, in the folder it
   opens. The renderer lists on the page switched off. Installing runs nothing.
2. Switch it on, with the switch on its row, or by picking it in a Renderer panel or a
   render target. Either way its enable card comes up first, and only the card's Switch
   On switches it on.

To update a renderer, install its new `.roxp`. It replaces the old one, and the renderer
switches off until you switch it on again, when the card lists what changed. Install
refuses a package whose id a renderer folder already holds.

Remove on its row drops the renderer's settings and its cache folder, and an installed
`.roxp` with them. A renderer folder stays where it is. Panels and targets that named
the renderer keep its name and say it isn't installed.

## The enable card

The card is the one place rox asks. It names the renderer, its author and version and
its description, then a line for everything it will get:

- what it runs as: in the sandbox, outside it, or with your permissions on a system rox
  can't sandbox (see [The sandbox](#the-sandbox));
- that it draws in a Renderer panel or a render target on a canvas;
- that it gets the music's signals and the playing position, and the pointer while it's
  over its own picture. Every renderer gets these two;
- each thing its manifest asks for on top: the theme's colours, the playing track's
  cover as a picture, the music's spectrum (how loud each frequency is, never the sound
  itself), signals it would like added to your pool, the network, or folders to read.

Switch On approves exactly the files in the package, by a hash over every one of them,
on this machine only. When any file changes, the renderer switches off and its row reads
Changed on disk. When the manifest changes, its card shows what changed since you last
switched it on, and a new ask gets a line of its own. When what rox can enforce on this
system changes, its row reads Off: the sandbox changed, and the card says what it runs as
now. Nothing switches a renderer back on except Switch On.

## The sandbox

On Linux a renderer runs confined from its first instruction. It can read and run its own
package, write its own cache folder, read the system's libraries and use the graphics
card. It can't read your home folder or rox's data folder, open a network connection or
any local socket, reach your desktop or its session bus, or signal or trace rox and your
other programs. A kernel too old for part of that still sandboxes the rest, and the card
names each thing it can't stop.

A renderer can ask for more in its manifest, and each ask is a line on its card and a
mark on its row:

```json
"renderer": {
  "label": "Gallery",
  "access": { "network": true, "read": ["library", "pictures"] }
}
```

`network` opens TCP and UDP to any address. Unix sockets stay closed, so the session bus
and the display are still out of reach. `read` takes `library` for your library's
folders and `music`, `pictures`, `videos`, `documents` or `downloads` for your own, all
read-only. `"unconfined": true` asks to run outside the sandbox with your permissions,
and its card says so in plain words.

On macOS and Windows, and on a Linux whose kernel can't sandbox, a renderer runs as a
program with your permissions. The line at the top of the Renderers page says which one
your system is, and why.

## Writing your first renderer

[`examples/renderers`](examples/renderers/) has two to copy.
[`bars`](examples/renderers/bars/) is one Python file on the standard library: a bar for
each signal, a row of thin bars from the spectrum, the theme's accent. Start there.
[`plasma`](examples/renderers/plasma/) is a Rust program on rox's own `rox-frames` crate,
with settings a target lets the user change. Each one's README says how to try it.

The smallest manifest that loads, as `plugin.json` in a folder named for its `id`:

```json
{
  "id": "dots",
  "name": "Dots",
  "version": "0.1.0",
  "api": 1,
  "entry": {
    "script": { "path": "dots.py", "interpreter": "python3" }
  },
  "capabilities": {
    "renderer": { "label": "Dots" }
  }
}
```

`label` is what the pickers list. A renderer can't also declare a source, panels or
providers, and a native program takes an `entry.native` table by platform instead of a
script. The program reads `hello` from stdin, which gives the size to draw at, then
writes each frame into one of the shared memory's two slots and announces it on stdout
as `f <frame> <slot>`. The guide's [Writing a frame](docs/03-implementation/11-renderers.md#writing-a-frame)
and [The wire](docs/03-implementation/11-renderers.md#the-wire) give the exact steps, and
the bars example follows them.

A renderer can be run and checked outside rox with
[`examples/renderers/standin.py`](examples/renderers/standin.py), which plays rox's part
and reports what rox would have refused:

```sh
python3 examples/renderers/standin.py --palette --spectrum path/to/dots
```

The stand-in grants everything the flags ask for and runs the renderer unconfined, so
run it only on code you trust, and test the sandbox in rox itself.

While you work on a renderer in its folder, Developer mode on its row approves your saves
until rox quits, as long as `plugin.json` stays as you approved it. Pack, beside it,
writes the folder to a `.roxp` to share. What the renderer prints to stderr is in rox's
log and the Console window (F12), along with rox's own lines about it.

## The full contract

[docs/03-implementation/11-renderers.md](docs/03-implementation/11-renderers.md) holds
the rest: every key the manifest takes, the shared memory's layout, each line rox sends
and when, the size, the cover and spectrum mappings, the GPU texture a renderer can draw
into on Linux, the limits that stop a misbehaving one, what the sandbox allows path by
path, and the stand-in host's options.
