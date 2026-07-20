# ADR 8: Spectrum and waveform on gpui primitives, no generative visual

**Status:** Decided, supersedes the original call (a CPU-simulated generative visual)
after the [prototype](../../0R-research/01-generative-visualizer.md)

Decision: the spectrum analyzer and waveform seekbar draw with gpui primitives, quads
and paths in a `canvas()` paint callback. The generative visual doesn't ship in CPU
form; it returns only as a real GPU shader. gpui exposes no public custom-surface or
shader API, and its internal move to wgpu opened no door (no public device handle),
so the return is gated on the framework.

Alternatives: the original call, a curl-noise flow field simulated on a worker thread
and drawn as polylines or a per-frame image blit; a custom WGSL shader handed to
gpui; a separate wgpu surface composited into the window.

Trade: the prototype settled feasibility, not worth. Both CPU paths hold a 60fps
budget at 12,000 particles, and the blit keeps the UI thread flat at 0.1 ms. What the
working version costs is a worker thread rasterizing and copying a framebuffer for
every frame the panel is visible, a standing tax for a decoration, and the output is
a fixed-resolution buffer GPU-scaled to the panel, soft at large sizes on hidpi. A
shader gets the same look sharp at any size for near nothing, so the CPU version is
the worse form of the feature carried as maintenance while the right form stays
possible. The spectrum and waveform lose nothing here: a handful of shapes per frame
is exactly what gpui's primitives are for. The escape hatch, a separate wgpu surface,
still has no clean embedding API and stays a last resort.
