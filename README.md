# rox

If Foobar2000 was made in the current year.

rox is a desktop music player for people with large, carefully tagged local libraries.
The UI is panels you compose yourself, duplicate with independent configs, and pop out
into real OS windows. Themes are token sets a person can share. Tagging is deep enough
to trust with a real collection, and the whole thing stays fast at tens of thousands of
tracks. Rust, built on gpui, with Linux, Mac, and Windows all first-class. If it doesn't
start in under a second, it isn't rox.

Some quick benchmarks, taken against the same 50k-track library on the same
Linux machine (Ryzen 9 5950X, Wayland), warm start, release build:

|                          | rox              | Tauon Music Box |
| ------------------------ | ---------------- | --------------- |
| Launch to workspace up   | 0.25 s           | 8.2 s           |
| Memory with library open | ~250 MB          | ~3.0 GB         |
| Ships as                 | one 54 MB binary | [135 MB Flatpak](https://flathub.org/apps/com.github.taiko2k.tauonmb) |

For scale elsewhere: Quod Libet users report [~600 MB at 31k tracks](https://programming.dev/post/47625759)
and a [64 second start at 45k](https://github.com/quodlibet/quodlibet/issues/3042),
and the Spotify client idles between 300 and 900 MB. Foobar2000 itself stays
lean, but you knew that, that's the point.

## Why

I loved Foobar2000 because of panel UI you build yourself, a theme community on top of it
(CaTRoX, NekoRoX, Georgia), and tag and library management that held up at scale. That
stack is stranded on Windows, and nothing on Linux or Mac covers all three legs: Tauon's
tagging is shallow, Strawberry isn't composable, Quod Libet isn't something you'd theme.

Oh right, and it needs to be native, everywhere.

I've been working on and off on variations of rox over the years through Golang and its
fragmented GUI ecosystem. I landed on Wails but work on it was sporadic and I still had
my Foobar setup. Progress took ages because making a GUI app on Go maybe isn't the best
or maybe I'm just bad at it, but as I progressed, I also noticed that I was just doing
another webapp which killed a lot of my motiviation to make a true spiritual successor
to Foobar2000 in my mind. It had to be fast, it had to be native, it had to look as
close as possible to CaTRoX.

Jump to the start of 2026 and I did a full-time move to Linux. Because of how CaTRoX
(and my NekoRoX fork) is built with tons of random work arounds, Internet Explorer
essentially running in panels and so on and so forth, it's just a pain to run through
Wine and it really surfaces just how hacky everything is put together. It feels like
the foundation isn't solid and everything is just one OS change away from exploding again.

I've been working on [Orbit](https://github.com/hivecom/orbit) on the side and wanted to
do an evaluation of [gpui](https://gpui.rs/) since I use [Zed](https://zed.dev/) and I
very much love the vision of going back native. I've been loving working with it for
prototypes and I realized I had most of the foundation written and all I had to do
is start mapping it over. So that's what I did. And now we have a new native player.

## Development

With Nix:

```sh
nix develop
cargo run
```

The dev shell carries the Rust toolchain and the Linux libraries gpui loads at runtime.
If you use direnv, `direnv allow` gets you the same shell on cd. The first build
compiles the whole gpui tree and takes a few minutes.

On a Mac you also need Xcode installed, nix or not: gpui compiles Metal shaders at
build time and nix can't ship Apple's Metal toolchain. On Xcode 26 that toolchain is
a separate one-time download: `xcodebuild -downloadComponent MetalToolchain`.

Without Nix you need stable Rust and gpui's system libraries from your distro (Wayland,
X11, Vulkan, xkbcommon, fontconfig, alsa); every Rust dependency comes from crates.io.
Run `./scripts/vendor-gpui.sh` once before building: it fetches gpui and gpui-component
and applies the small patches under `patches/` (the nix shell does this on entry).

## 

## Spec

Check out the [docs index](docs/README.md) which lists the altitude spec for how `rox` is built.

## AI

rox is written with AI tools because I'm building it for myself and I can only deliberately work on so
many projects at the same time. If you want to contribute high quality hand written code and take
over the development of rox instead of me using AI tools; be my guest.
