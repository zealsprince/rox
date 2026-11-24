# Plasma

An example rox renderer in Rust. It draws a plasma on the CPU at 60 frames a second off
its own clock. The music's signals turn it faster, the playing position shifts its
colours, and a ring follows the pointer and widens while a button is held. The frame
mapping goes through `rox_frames::Writer`, rox's own crate, so the counter protocol is
the crate's and not this program's.

The contract it follows is in [the renderers doc](../../../docs/03-implementation/11-renderers.md).

It's a Cargo project of its own with its own lockfile, outside rox's workspace, so
rox's build never compiles it. It depends on std and `rox-frames` with the `writer`
feature, and reads rox's lines with a few string searches instead of a JSON parser.

## Building it

```sh
cargo build --release --manifest-path examples/renderers/plasma/Cargo.toml
```

A renderer package is a folder holding `plugin.json` and what it runs. Make one in rox's
plugins folder (Reveal Folder on the Plugins page opens it) and copy the build into it:

```sh
mkdir <plugins>/plasma
cp examples/renderers/plasma/plugin.json <plugins>/plasma/
cp examples/renderers/plasma/target/release/plasma <plugins>/plasma/
```

On Windows the build is `plasma.exe`, which is the name `plugin.json` gives for
`windows-x86_64`. The plugins folder has to be on a filesystem that lets programs run:
on one mounted `noexec` the start fails with "Permission denied".

## Trying it

1. Turn on Enable Plugins at the top of Settings > Plugins. With the folder above in
   place, Plasma lists under Renderers, switched off.
2. Switch Plasma on and confirm the card.
3. Add a Canvas panel (Panels > Arrangement > Canvas). With Panels > Design Mode on,
   right-click the empty stage, or press the plus in the canvas inspector's header, and
   pick Add Render Target > Plasma.
4. Leave design mode and play something with a few signals in the pool (Application >
   Signals).

Switched on, Developer mode on the renderer's row shows Pack, which writes the folder
to a `.roxp` to share.

## Where a GPU renderer differs

This one fills the slot from the CPU. A renderer that draws on the GPU reads its frame
back into the slot instead: the readback's destination is the closure `Writer::frame`
hands over, and the counter goes even only once that copy is done. Finish the GPU work
before announcing: rox may copy the slot as soon as it reads the line.
