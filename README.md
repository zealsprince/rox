# rox

If Foobar2000 was made in the current year.

rox is a desktop music player for people with large, carefully tagged local libraries.
The UI is panels you compose yourself, duplicate with independent configs, and pop out
into real OS windows. Themes are token sets a person can share. Tagging is deep enough
to trust with a real collection, and the whole thing stays fast at tens of thousands of
tracks. Rust, built on GPUI, with Linux, Mac, and Windows all first-class.

## Why

Foobar2000's magic was a panel UI you build yourself, a theme community on top of it
(CaTRoX, NekoRoX, Georgia), and tag and library management that held up at scale. That
stack is stranded on Windows, and nothing on Linux or Mac covers all three legs: Tauon's
tagging is shallow, Strawberry isn't composable, Quod Libet isn't something you'd theme.
rox goes after that gap.

## Run it

With Nix:

```sh
nix develop
cargo run
```

The dev shell carries the Rust toolchain and the Linux libraries GPUI loads at runtime.
If you use direnv, `direnv allow` gets you the same shell on cd. The first build
compiles the whole GPUI tree and takes a few minutes.

On a Mac you also need Xcode installed, nix or not: GPUI compiles Metal shaders at
build time and nix can't ship Apple's Metal toolchain. On Xcode 26 that toolchain is
a separate one-time download: `xcodebuild -downloadComponent MetalToolchain`.

Without Nix you need stable Rust and GPUI's system libraries from your distro (Wayland,
X11, Vulkan, xkbcommon, fontconfig, alsa); every Rust dependency comes from crates.io.

## Docs

Check out the [docs index](docs/README.md) which lists the altitude spec for how `rox` is built.
