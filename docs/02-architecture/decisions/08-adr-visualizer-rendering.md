# ADR 8: Visualizers render CPU-side, forced by GPUI

**Status:** Decided, validated by the
[visualizer prototype](../../0R-research/01-generative-visualizer.md): the generative
visual draws as a per-frame image blit, polylines stay for spectrum and waveform

Decision: draw the spectrum and waveform with GPUI primitives in a `canvas()` paint
callback, and run the generative visual as a CPU simulation, drawn either as GPUI polylines
(the path builder) or blitted as a per-frame image. No custom GPU shaders.

Alternatives: a custom WGSL shader handed to GPUI, or a separate wgpu surface composited
into the window.

Trade: this is the one product requirement GPUI doesn't cleanly satisfy. GPUI exposes no
public custom-GPU-surface or shader API. Its recent internal move to wgpu did not open that
door, it's an implementation detail with no public device handle. So a shader-driven fluid
visual isn't available. The spectrum analyzer and waveform are easy and native: quads and
paths at 60fps is exactly what GPUI is built for. The generative "green flow-field" look is
a curl-noise flow field driven by FFT bands, run on a worker thread; if the look is lines,
we advect points and draw polylines with no texture upload, which fits the reference best.
The cost is a CPU budget for the sim and no access to true GPU-shader fluidity. The escape
hatch, a separate wgpu surface, has no clean embedding API and is a last resort. This is the
first thing to prototype, because it's the one place the framework fights the product.
