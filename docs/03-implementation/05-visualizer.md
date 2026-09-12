# Visualizer

How the spectrum analyzer and the waveform seekbar are wired: the PCM tap the engine
feeds, the analysis feed the UI drains into, the FFT, the per-track peaks cache format,
and the pacing between the feed and the paint callback. This makes the visualizer
contract from
[components](../02-architecture/02-components.md#visualizer-subsystem) concrete, within
the call made in [ADR 8](../02-architecture/decisions/08-adr-visualizer-rendering.md)
(spectrum and waveform draw with gpui primitives; the generative visual waits on a real
GPU shader), plus the one panel that takes the other side of that trade: Milkdrop, under
[ADR 28](../02-architecture/decisions/28-adr-milkdrop.md). Version-sensitive: the tap
ring is rtrb, the FFT is hand-rolled, the paint path is gpui's `canvas()`, and the
Milkdrop engine is libprojectM pinned to master commit `88f23c76` (CMake version 4.2.0;
the pin is in `scripts/vendor-projectm.sh` and `flake.nix`, bumped together), with the
patches under `patches/projectm/` applied on top by both.

## From the tap to the feed

The engine's PCM tap is the input. It's a second rtrb SPSC ring beside the sample
ring, 16,384 `f32` samples (`TAP_SAMPLES` in `rox-playback/src/output.rs`), and the RT
callback pushes a pre-volume copy of every stereo frame it plays and ignores push
failure, so the visuals track the program material rather than the listening level.
Lossy: a slow visualizer loses samples, never slows audio. This is
the same tap the [playback doc](01-playback.md#thread-and-channel-wiring) describes from
the producer side.

```
 RT output callback            UI pump (60 Hz)                 paint callback (canvas)
 ──────────────────            ───────────────                 ───────────────────────
 push pre-volume   ──tap ring──▶ drain_tap: read all slots
 stereo frames                   push into AudioFeed
                                 (interleaved stereo)  ──feed──▶ latest_mono window
                                                                  Hann + FFT per zone
                                                                  fold bins to bar levels
                                                                  paint quads
```

The consumer side is one drain on the UI pump. `Player::drain_tap` (in
`crates/rox-services/src/player.rs`) runs on the pump timer, `PUMP_INTERVAL` = 16 ms so about
60 Hz, reads every available slot in one `read_chunk`, and pushes the two ring slices
into the `AudioFeed`. Nothing here is real-time; the RT boundary is the tap ring itself.

`AudioFeed` (`crates/rox-viz/src/feed.rs`) is the seam. A `Mutex<VecDeque<f32>>` of
interleaved stereo, newest at the back, capped at `KEEP_SAMPLES` = `MAX_FFT_SIZE * 2 * 2`
= 65,536 samples (the largest window with slack), older samples dropped off the front on
every push. The feed also holds two atomics: `sample_rate` (`AtomicU32`, set per session,
48,000 default) and `written` (`AtomicU64`, total samples ever pushed), which lets a
view tell silence, nothing new, from a repeat of the same window. `latest_mono(out)`
copies the newest `out.len()` frames folded to mono ((L+R)/2), newest last, and returns
how many it copied; short means not enough buffered yet.

## FFT

`Analyzer` (`crates/rox-viz/src/analysis.rs`) is one window's worth of transform.
Hand-rolled and dependency-free: an FFT at these sizes at 60 Hz is nothing, and it keeps
the crate free of a DSP dependency until one is justified.

- **Window sizes**: `FFT_SIZE` = 4096 default, between `MIN_FFT_SIZE` = 512 and
  `MAX_FFT_SIZE` = 16,384. Must be a power of two; `new` asserts it. Short windows react
  fast, long ones resolve finer.
- **Window function**: a precomputed Hann window (`0.5 - 0.5 cos(2 pi t)`), with its sum
  cached for amplitude normalization.
- **Transform**: an in-place iterative radix-2 Cooley-Tukey FFT (bit-reversal
  permutation, then butterflies), real input in `re`, `im` zeroed each call.
- **Magnitudes**: `sqrt(re^2 + im^2) * 2 / window_sum` per bin, so a full-scale sine
  comes out near 1.0. Only the lower half-spectrum is returned (`size / 2` bins); the
  mirror above Nyquist is dropped.
- **Band mapping**: `log_bands(bands, lo_hz, hi_hz, sample_rate, half)` maps
  log-spaced bands across `lo_hz..hi_hz` to half-spectrum bin ranges, each at least one
  bin wide, so neighbours share bins where the FFT is too coarse to split them.

## Spectrum panel

`SpectrumPanel` (`crates/rox-panels/src/spectrum.rs`) owns the config and the bar state.
`SpectrumConfig` is the per-view config, serialized into the panel's layout node (see
[panels](06-panels.md#the-panel-config-model)): `freq_lo` / `freq_hi` (analyzed range,
default 30 Hz to 16 kHz), `bar_width`, `bar_gap`, `fft_size` (default 8192), `gradient`,
`outline`, `caps`, `freeze`, `cap_gravity`, `labels`, and the split-zoning knobs
`split` / `split_hz` / `fft_size_hi`. Split zoning analyzes below and above `split_hz`
at different window sizes, so each end of the range trades reactivity for resolution on
its own.

`Bars` is the state machine between the feed and the paint. One `step` per frame:

1. Derive the bar count from the width, `(width / (bar_w + bar_gap))` clamped to
   `MIN_BARS` = 16, `MAX_BARS` = 512.
2. If the mapping changed (bar count, rate, range, fft sizes, split), rebuild the zones
   and reset the level vectors. Each `Zone` has its own `Analyzer`, a mono scratch
   buffer, and its slice of band bin-ranges.
3. If there's new audio since last tick (`written` moved), pull `latest_mono` per zone,
   run the analyzer, and set each bar's target from the band's peak magnitude in dB:
   `20 log10(peak)`, normalized from `FLOOR_DB` = -66 to `MAX_DB` = -12 and clamped to
   0..1. With no new audio the targets hold until the feed has been idle past
   `SILENT_AFTER` = 0.15 s, then the bars fall to silence.
4. Ease each level toward its target, `ATTACK` = 40/s rising, `RELEASE` = 10/s falling.
   Peak-hold caps rise with the bar and fall back under `cap_gravity`.
5. Set `alive` if any bar or cap is still above `EPSILON`.

`paint` draws the frame with gpui quads in a `canvas()` callback: dB gridlines, then per
bar a filled bar (flat accent, or a loudness gradient when `gradient` is on) or a hollow
outline, plus a peak-hold cap when `caps` is on. Both `step` and `paint` run inside the
paint callback on the UI thread. This is where implementation and the components boundary
part: the contract reads "analysis runs off the UI thread," but the spectrum FFT is cheap
enough per frame that it runs inline in paint. Only the offline decodes below, the
waveform precompute among them, leave the UI thread.

## Frame pacing

Two clocks drive redraws, so the panel animates while playing and settles cleanly when
it stops:

- **Playing**: the pump drains the tap and notifies, which repaints on the next frame.
  `step` reads the feed's `written` counter moving as fresh audio.
- **Not playing but still moving**: `body` calls `request_animation_frame()` while
  `bars.alive`, so the decay and the falling caps finish animating after audio stops
  without holding a frame loop open once they settle.
- **Frozen**: with `freeze` on and playback paused, `step` parks the levels exactly where
  they are and stops animating; paint keeps showing the standing frame. A settings edit
  that remaps the bars still applies, because the feed keeps the last window and the
  frame re-analyzes at the new mapping.

A track loaded paused has never pushed anything into the tap, so the frozen bars would
have nothing to show. `Player::prime_feed` closes that: it decodes one window at the
current position off-thread (`engine::decode_window`, resampled to the device rate,
interleaved stereo) and pushes it into the feed so the frozen frame is real.

## Waveform peaks cache

The waveform seekbar draws from a peak reduction of the whole track, cached to disk so
the strip comes back instantly after the first play instead of re-decoding. Each bin is a
`PeakBin` (`rox-library/src/peaks.rs`): the sample extremes over its frames, which draw
the outer envelope, and the RMS across them, which draws the flatter loudness band
inside it.

`engine::decode_peaks(path, bins)` (`rox-playback/src/engine.rs`) is the reducer. It
decodes the whole file through the same path playback uses, no audio device, and folds
it to at most `bins` bins per lane: a coarse pass of one bin per `BLOCK_FRAMES` = 2048
frames keeps memory flat whatever the track length, then folds down to `bins` keeping
each bucket's extremes so transients aren't averaged away and taking the root mean
square of the bucket's RMS values. Bins are normalized so the loudest extreme hits 1,
with a `pow(0.7)` perceptual curve so quiet passages stay visible. The RMS goes through
the same scale and curve so it never leaves the envelope. The waveform panel asks for
`PEAK_BINS` = 2048 bins and resamples that down to the drawn bar count at paint time.

The cache is one small binary file per track under `waveforms/` in the app's data dir
(`crates/rox-library/src/peaks.rs`). The entry name is `{fnv1a(path):016x}.peaks`, an FNV-1a
hash of the path; the path stored inside disambiguates a hash collision. The layout,
little-endian throughout:

```
offset  bytes  field
0       8      magic  b"roxwave3"
8       8      source size   (u64)
16      8      source mtime  (u64, unix seconds)
24      4      path length N (u32)
28      N      path bytes
28+N    4      bin count C   (u32)
32+N    C*12   C triples of (min, max, rms) f32
```

The magic is the format version, bumped when the layout changes so old entries read as
misses and get rewritten. Load re-derives the source's `(size, mtime)`, and a mismatch
on size, mtime, path, magic, or a truncated body is a miss, not an error: the file was
edited or replaced, or the entry is stale or garbage, and the panel decodes fresh and
overwrites. `store` failures log and move on; a lost entry only costs a re-decode next
time.

The panel's load is one background task: `peaks::load` first, and on a miss
`decode_peaks` then `peaks::store`, all on the background executor so a long track's full
decode never touches the UI thread. A generation counter drops a result that arrives after
the track already changed.

## Milkdrop panel

A MilkDrop preset is a program, not a shader: a per-frame equation block, a warp mesh,
custom waves and shapes, and hand-written GLSL for the composite. Twenty years of them
exist and nothing but libprojectM runs them, so rox links libprojectM in and gives it
the three things it asks for: a current OpenGL context, a framebuffer, and audio. None
of rox's renderers (blade on Vulkan and Metal, Direct3D 11 on Windows) will let a
second renderer into their swapchain, which is why the engine runs on a thread with a
context of its own and the frame comes back over the CPU. That readback is the cost
ADR 8 refused for the generative visual and ADR 28 takes here, because the alternative
is porting MilkDrop to WGSL.

```
 rox-milkdrop worker (own GL context)            UI thread (MilkdropPanel canvas)
 ────────────────────────────────────            ───────────────────────────────
 drain commands
 feed.since(cursor) ──▶ projectm_pcm_add_float
 projectm_opengl_render_frame_fbo(fbo)
 glReadPixels ──▶ PBO[n]      (async)
 map PBO[n-1], flip rows ──▶ Frame { seq }  ──slot──▶ frame_after(last_seq)
 sleep to the next 1/fps deadline                     update_user_texture(id, rgba8)
                                                      paint_screen_shader(one-pass chain)
```

Three crates split the work. `rox-milkdrop-sys` is the C API: `build.rs` runs cmake on
the vendored projectM source and links the static library and the C++ runtime, and
`src/lib.rs` is hand-written `extern "C"` declarations checked against the headers, no
bindgen. `rox-milkdrop` is the engine, DSP-adjacent like `rox-viz` and drawing nothing.
The panel is in `rox-panels` beside every other panel.

**The worker.** `Engine::spawn` starts one thread named `rox-milkdrop` and returns at
once, since a cold driver can take a second to hand over a context. The thread makes a
windowless context (`context.rs`: EGL surfaceless on Linux through glutin, CGL on macOS,
WGL on Windows behind a 1x1 window nobody shows, a 1x1 pbuffer where surfaceless is
refused), creates the projectM instance with a load proc that resolves GL through
glutin, and loops on a wall-clock deadline at `1/fps`. A slow frame is absorbed rather
than drifting the schedule. Each iteration drains the command channel, pulls what
arrived on the `AudioFeed` since its cursor (`AudioFeed::since`, the one addition this
made to `rox-viz`; a slow reader gets a gap, never a repeat), pushes it as stereo PCM
in chunks capped at `projectm_pcm_get_max_samples`, and renders into an FBO. The
readback runs through two pixel buffer objects: frame N's `glReadPixels` starts into one
while frame N-1 is mapped out of the other, one frame of latency spent on not stalling
the pipeline. The mapped rows are flipped top-first into a publish buffer and swapped
into the shared slot with a sequence number. Failure at any step is a
`Status::Failed(message)` the panel shows as body text, never a panic: a machine with
no usable GL keeps every other panel working. A readback the driver refuses to map is
a failure too, after `MAP_MISS_LIMIT` refusals in a row (thirty, half a second at sixty
frames), since one refusal is a hiccup the next frame covers and a run of them is a
driver that never will. The status a running worker publishes carries `GL_RENDERER`
and `GL_VERSION` beside the projectM version.

libprojectM's own log goes through `projectm_set_log_callback`, set once per process
before the first engine and forwarded to `log` under a `projectm:` prefix, error and
fatal as `error`, warn as `warn`, info as `info`. Without the callback every `LOG_*` in
the library is a no-op, and the GL probe's summary line, shader compile errors and
texture loads that failed went nowhere; a Windows release build has no stderr to catch
them either.

The panel says something in two states that are not failures but look like one from
the chair. A worker still `Starting` past `STALL_GRACE` (five seconds, counted from
the spawn or the latest resume) gets "still starting" over the body; a `Running`
worker that has never put a frame up as a texture gets "no frame has arrived" with the
renderer and GL version named. Both keep the transport controls, since pressing Next
and watching a preset name land is how a reader tells a live worker from a dead one.
Until this, both states were a black panel with no word, which on the machine it
happens on looks exactly like the feature working with the lights off.

The context ask is 3.3 core first, then, off macOS, the driver's default (no version,
compatibility profile). The retry exists because of one Windows report: an Intel
driver refused the versioned core request with `0xC007000D`, a code no header names,
and the same code shows up in id Tech game logs on Haswell-era Intel parts right after
they hand out 3.1. projectM accepts a 3.3+ compatibility context and probes the version
itself, so a driver that can only do 3.1 is refused there, with the version in the log.

Everything the panel paints over the frame (the preset banner, the transport strip, the
failure and stall overlays) is a deferred draw. The frame is a shader region, and the
DirectX renderer runs regions once at the deferred-draw boundary, after every ordinary
primitive, so text painted in tree order sat under the opaque frame. That is why the
same Windows report showed a black panel and not the failure message the panel had
been drawing all along. Blade runs regions in paint order and is indifferent.

The GL the crate calls for itself (framebuffer, texture storage, the PBOs, `glGetString`
for the log) is twenty-eight `extern "system"` pointers in `gl.rs`, resolved by name off
the same load proc. No `gl` or `glow` crate for twenty-eight functions.

Preset switches are the one reentrant path: projectM's callbacks fire from inside its
own render call, so they only record what happened and the actual load runs on the
next loop iteration. The switch-requested callback picks the next preset from the
library's rotation (random unless locked), the failed callback queues an
`Event::PresetFailed` the panel drains each frame.

**Frames are shared, not copied.** `frame_after` used to clone the buffer out of the
slot, and the texture upload copied it again; at a 776x1049 panel that measured 2.6 ms
of UI thread per frame, almost all of it two three-megabyte allocations faulting in a
page at a time. The pixels now live behind an `Arc`, the panel hands the same handle to
the upload, and the worker takes the buffer back once the last handle drops, so the
steady state allocates nothing on either side.

**The panel.** `MilkdropPanel` (`crates/rox-panels/src/milkdrop.rs`) spawns the engine
lazily on the first paint with a real size, hands the saved preset in with the spawn
so the worker comes up on it instead of shuffling one first, and pushes the rest of its
config down as commands (duration, beat sensitivity, hard cuts, lock, rotation) so a
restored layout comes up as it was left. The paint closure runs in order:

1. Work out the render size: the panel's device pixels times the config's `scale`,
   each side clamped to 128..=4096. A new size is only acted on when it's seen for a
   second consecutive frame (one `request_animation_frame` of debounce), so a drag
   doesn't thrash the FBO. When it commits: release the old dynamic texture, register a
   new one at the new size, send `Command::Resize`, drop the chain so it re-registers.
2. `engine.frame_after(last_seq)`: a newer frame at the texture's size goes up through
   `update_user_texture`; one from before a resize is dropped, its seq still advancing so
   it's dropped once.
3. Register the one-pass chain if missing: `register_user_shader_chain` with `FRAME_WGSL`
   as `main` and the texture bound as the asset `frame`. A chain with an asset can only
   run as a screen pass, so it's painted with `paint_screen_shader` keyed by the panel's
   entity id, the same branch the shader panel's feedback buffer takes. Fade, hue turn,
   tint, the grade and the flips reach the pass through the signal slots.
4. `request_animation_frame` while animating: a docked panel renders cached, and the
   recorded pass replays with stale values unless the view is dirtied every frame.

Texture and chain are keyed by the window that issued them, because a compiled
pipeline and an uploaded texture both belong to one window's renderer. Popping the
panel out registers a fresh pair in the new window; the pair left behind dies with the
old window, the same life a registered image has there.

Why a dynamic texture and a chain rather than `img()` with a fresh `RenderImage` per
frame: that path runs through the sprite atlas and needs an allocation and a `drop_image`
every frame for what is really a video stream. The chain path also means a Milkdrop
frame composes like any other shader surface, so the panel takes a surface shader over
the top. The three window calls it relies on (`register_dynamic_texture`,
`update_user_texture`, `release_user_texture`) are the `z4-dynamic-user-textures.patch`
addition to the vendored gpui, on both the blade and DirectX backends.

**Parking.** What a pause or a stop does to the picture is the config's `fade` switch,
hold by default in the panel. Under hold the panel sends `Command::Pause` and keeps the
last frame up: the worker stops rendering and reading back, and every UI frame after that
samples a texture that's already there. Under fade the visual goes down to the panel
background first and parks at the bottom of the fade, since a parked worker publishes
nothing and pausing on the event would freeze the picture the fade is taking away. A
panel that has never run holds nothing, so a hold before the first play reads as a cut.
Play sends `Resume`. Under hold, the `run_focused` switch, off by default, lets the
panel's own focus keep the worker running so presets can be browsed in silence. It's
opt-in because focus is sticky (the dock focuses the active tab, any click on the panel
takes it), and a paused track with the visual still cycling under it reads as the pause
not having taken. The backdrop carries the same hold-or-fade switch on the Appearance
page, fade by default, and parks on whether any window's player is playing rather than on
focus, which it hasn't got.

**Presets on disk.** `settings::milkdrop_dir()` is `data_dir()/milkdrop`, with
`presets/` and `textures/` under it, plus any extra roots from the config. Nothing
creates it; the packs are the user's download, and the settings page names the three
worth having. `PresetLibrary::scan` walks the roots for `*.milk` case-insensitively and
sorts them, reading nothing inside the files: only libprojectM's parser can tell a
broken preset from a working one, and that answer arrives as `PresetFailed`. The same
walk collects every `textures/` directory it passes, and the worker hands projectM the
app's own `textures/` folder first and then those, because libprojectM searches only
the paths it's given and never beside the preset file, and the big packs ship their
images inside the pack. The directory layout is the one structure it keeps, since the
packs organise themselves by category folder, and a `Rotation` narrows what Next,
Previous and the timed switch walk to one folder or to the favourites list. With no
presets found projectM's built-in idle preset renders, so the panel is never blank.

**Driving it from the socket.** `debug.milkdrop` (ADR 22's debug scope, `roxctl
milkdrop` from a shell) reaches the first Milkdrop panel in the front workspace by verb
rather than by pixel: `status` reads back the preset that's up, the lock, the scan
count, the rotation, the engine state, and projectM's message for the last preset it
refused; `load <file>` puts a preset up from any path, scanned or not, and re-reads the
file on a repeat call; `lock`, `rescan`, `next`, and `prev` do what the context menu
does. `frame <out.png>` hands back the worker's newest readback as PNG. That's engine
output, the frame before the panel's tint, grade, and flips, not a capture of the
window, which is what keeps it on the data side of the ADR's "pixels stay a screenshot
job" line. Together they make the preset-authoring loop a script: write the file, load
it, read the snapshot for a compile failure, dump a frame.

**What it costs.** `cargo run -p rox-milkdrop --release --example headless -- <presets>`
runs the engine for ten seconds with no UI and prints the baseline the zero-copy
follow-up is judged against. At 1920x1080 and 60 fps over the Cream of the Crop pack
on a desktop GPU: 600 frames delivered in 10.0 s, average readback (map and flip) of
1.7 ms per frame. The cmake build of libprojectM inside `cargo build` is about 18 s
cold on a 32-thread machine and cached after that.

## Reference

The shared analysis is in `crates/rox-viz`: `feed.rs` (`AudioFeed`, the tap-to-view
seam), `analysis.rs` (`Analyzer`, the Hann-windowed FFT and `log_bands`), `lib.rs`
(exports). The panels and the on-disk pieces sit across three crates:
`crates/rox-panels/src/spectrum.rs` (`SpectrumPanel`, `SpectrumConfig`, the `Bars` state
machine), `crates/rox-panels/src/waveform.rs` (`WaveformPanel`, the peaks load and the
morphing strip), `crates/rox-library/src/peaks.rs` (the cache format), and
`crates/rox-services/src/player.rs` (`drain_tap`, `prime_feed`). The tap producer and the offline decoders
(`decode_peaks`, `decode_window`) are in `crates/rox-playback`: `output.rs`, `engine.rs`.
Milkdrop is three crates: `crates/rox-milkdrop-sys` (`build.rs` runs cmake on
`vendor/projectm`, `src/lib.rs` is the FFI surface), `crates/rox-milkdrop` (`lib.rs` for
`Engine`, `Frame`, `Command`, `Status`; `worker.rs` the render thread; `context.rs` the
headless GL context per platform; `gl.rs` the twenty-eight raw GL calls; `library.rs`
`PresetLibrary` and `Rotation`; `examples/headless.rs` the cost baseline), and
`crates/rox-panels/src/milkdrop.rs` (`MilkdropPanel`, `MilkdropConfig`, `FRAME_WGSL`, the
paint closure and the settings pages). `AudioFeed::since` in `crates/rox-viz/src/feed.rs`
is the worker's audio pull, `patches/gpui/z4-dynamic-user-textures.patch` adds
the three window calls the panel draws through, and
`patches/gpui/z6-dynamic-texture-registration-errors.patch` makes the registration
report a device that refused the allocation instead of keeping an id over nothing.
