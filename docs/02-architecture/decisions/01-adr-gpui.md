# ADR 1: gpui as the UI framework

**Status:** Decided

Decision: gpui. This came down as a founder/product choice; the architecture records the
trade rather than relitigating it.

Alternatives: Tauri (web UI in a native shell), egui (immediate-mode), Iced, Slint.

Trade: gpui is GPU-accelerated, Rust-native, and proven at scale inside Zed. Two of its
strengths are ones this product needs directly. Its lists virtualize, so a library view
renders only the rows actually on screen and a hundred thousand tracks cost about what a
hundred do. And it opens several OS windows that share entity state, which is
what a popped-out panel is: a second window whose views read the same entities as the
first, so playback and selection stay in step without any cross-window messaging to
write.

gpui is pre-1.0 and changes its API often, so an upgrade is a
budgeted task rather than a version bump, and the version has to be pinned exactly. Its
biggest gap is custom GPU rendering. There's no public way to hand it a shader or to
composite a surface of our own into its scene, which is the whole subject of
[ADR 8](08-adr-visualizer-rendering.md).

The alternatives each give something up that matters more. Tauri would have meant an
easier UI and a mature ecosystem, paid for in binary size, idle memory, and a web
runtime between the interface and the audio engine. egui would have been simpler to work
in but weaker for a heavily composed, themed desktop app. Immediate mode gives the
framework nowhere to hold per-panel state and styling, so the app rebuilds both every
frame.

gpui fits the "cutting-edge, native, beautiful" goal, and what it charges for that is
the panel system and the visualizer rendering path, which are ours to build.
