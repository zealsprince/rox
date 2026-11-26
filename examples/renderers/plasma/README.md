# Plasma

An example rox renderer in Rust. It draws a plasma on the CPU at 60 frames a second off
its own clock. The music's signals turn it faster, the playing position shifts its
colours, and a ring follows the pointer and widens while a button is held. The frame
mapping goes through `rox_frames::Writer`, rox's own crate, so the counter protocol is
the crate's and not this program's.

Its manifest says `"resizes": true`, so a target that follows its box tells it each new
size on a `size` line and it draws the next frame at that size with
`Writer::frame_at`, without starting again. It declares two settings, which show as rows
in the canvas inspector and in a Renderer panel's settings: Look, a palette it switches
to as soon as the `settings` line comes, and Frames a Second, which it reads once at its
start, so its `apply` is `restart`. Copy those two entries in `plugin.json` to give a
renderer of your own settings.

The contract it follows is in [the renderers doc](../../../docs/03-implementation/11-renderers.md).

It's a Cargo project of its own with its own lockfile, outside rox's workspace, so
rox's build never compiles it. It depends on std and `rox-frames` with the `writer`
feature, and reads rox's lines with a few string searches instead of a JSON parser.

## Building it

```sh
cargo build --release --manifest-path examples/renderers/plasma/Cargo.toml
```

A renderer package is a folder holding `plugin.json` and what it runs. Make one in rox's
plugins folder (Reveal Folder on the Renderers page opens it) and copy the build into it:

```sh
mkdir <plugins>/plasma
cp examples/renderers/plasma/plugin.json <plugins>/plasma/
cp examples/renderers/plasma/target/release/plasma <plugins>/plasma/
```

On Windows the build is `plasma.exe`, which is the name `plugin.json` gives for
`windows-x86_64`. The plugins folder has to be on a filesystem that lets programs run:
on one mounted `noexec` the start fails with "Permission denied".

## Trying it

1. With the folder above in place, Plasma lists on the Settings > Renderers page,
   switched off.
2. Add a Renderer panel (Panels > Visualizers > Renderer), press Pick a Renderer and pick
   Plasma, then confirm its card. Or switch it on from the Renderers page.
3. Play something with a few signals in the pool (Application > Signals).

A canvas takes it too: with Panels > Design Mode on, right-click the empty stage, or
press the plus in the canvas inspector's header, and pick Add Render Target > Plasma.

Switched on, Developer mode on the renderer's row shows Pack, which writes the folder
to a `.roxp` to share.

## Where a GPU renderer differs

This one fills the slot from the CPU. What changes when a frame is drawn on the GPU and
read back is in the renderers doc, under
[A GPU renderer](../../../docs/03-implementation/11-renderers.md#a-gpu-renderer).
