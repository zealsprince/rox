# ADR 1: GPUI as the UI framework

**Status:** Decided

Decision: GPUI. This came down as a founder/product choice; the architecture records the
trade rather than relitigating it.

Alternatives: Tauri (web UI in a native shell), egui (immediate-mode), Iced, Slint.

Trade: GPUI is GPU-accelerated, Rust-native, and proven at scale inside Zed, with real
strengths this product leans on, virtualized lists for a huge library and multi-window
with shared state for pop-out. The price is real: it's pre-1.0 with frequent breaking
changes, so every upgrade is a budgeted task and versions must be pinned exactly. Its
biggest gap is custom GPU rendering ([ADR 8](08-adr-visualizer-rendering.md)). Tauri
would have given an easier UI and a mature ecosystem at the cost of binary size, idle
memory, and a web runtime between us and the audio, egui would have been simpler but
weaker for a heavily composed, themed desktop app. GPUI fits the "cutting-edge, native,
beautiful" goal and forces us to own the panel system and the visualizer rendering path.
