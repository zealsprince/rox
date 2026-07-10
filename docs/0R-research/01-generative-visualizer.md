# Generative visualizer prototype

Answers the question [ADR 8](../02-architecture/decisions/08-adr-visualizer-rendering.md)
left open: does a curl-noise flow field driven by spectrum bands hold a frame budget
with CPU-side rendering, and does it draw better as GPUI polylines or as a per-frame
image blit?

The prototype lives in `crates/rox-viz-proto`. It runs the sim on a worker thread the
way the real visualizer will: particles advected by the curl of 3D Perlin noise,
driven by a synthetic 16-band spectrum (a kick on the lows, shimmer on the highs)
standing in for the FFT of the PCM tap. Frames reach the UI through a latest-wins
slot, so a slow UI sees fewer frames instead of back-pressuring the sim.

```sh
cargo run -p rox-viz-proto --release
```

Left click toggles the render mode, right click cycles the particle count, and the
HUD shows sim and paint cost live. `ROX_VIZ_AUTOCYCLE=1` steps through every mode
and count combination, prints averaged stats to stdout, and quits.

## Numbers

From an autocycle pass in a release build. Absolute times shift with the machine;
the scaling is the finding. Sim time is the worker thread's full tick, which in blit
mode includes rasterizing and copying the 960x540 framebuffer. Paint time is the
UI-thread cost inside the canvas paint callback.

| mode  | particles | sim (worker) | paint (UI) |
| ----- | --------- | ------------ | ---------- |
| lines | 1,500     | 0.20 ms      | 1.66 ms    |
| lines | 3,000     | 0.40 ms      | 3.36 ms    |
| lines | 6,000     | 0.76 ms      | 2.70 ms    |
| lines | 12,000    | 1.51 ms      | 4.22 ms    |
| blit  | 1,500     | 0.95 ms      | 0.10 ms    |
| blit  | 3,000     | 1.18 ms      | 0.11 ms    |
| blit  | 6,000     | 1.84 ms      | 0.11 ms    |
| blit  | 12,000    | 2.67 ms      | 0.10 ms    |

## Reading

Both modes hold a 60fps budget at 12,000 particles, so the CPU-side approach ADR 8
was forced into works. The difference is where the cost sits.

Polylines tessellate on the UI thread. Every paint rebuilds stroke paths through
lyon, and the cost scales with particle count; at 12,000 particles it's 4ms of every
UI frame, budget the rest of the app (a scrolling library view, a spectrum panel)
will want. Batching trails into one path per brightness bucket is what keeps it even
this low.

The blit keeps the UI thread flat at 0.1ms regardless of count. The worker
rasterizes into a persistent framebuffer (fade toward black, additive splats along
each particle's last step), and the UI wraps the buffer in a `RenderImage`, paints
it, and drops the previous one from the sprite atlas. The fade gives the smoke-like
trails of the reference look for free, where the polylines read more like a vector
field diagram.

Image blit for the generative visual. Polylines stay the right tool for the spectrum
analyzer and waveform, where the geometry is a handful of shapes.

## What this doesn't settle

- The look is in the right family (green flow field over near-black), but palette,
  density, and band response need tuning against real music, which waits on the
  playback engine's PCM tap.
- The blit buffer is fixed at 960x540 and GPU-scaled to the panel. At large panel
  sizes on a hidpi monitor that may read soft; the buffer wants to track the panel's
  device pixels up to a cap.
- Memory held steady over short runs with `drop_image` evicting the previous frame's
  atlas entry. Nobody has watched a multi-hour session.
- Perlin noise is hand-rolled to keep the measurement free of dependency choices.
  The real visualizer can keep it or take a crate; simplex would soften the
  axis-aligned bias.
