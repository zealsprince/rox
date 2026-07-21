# Visualizer

How the spectrum analyzer and the waveform seekbar are wired: the PCM tap the engine
feeds, the analysis feed the UI drains into, the FFT, the per-track peaks cache format,
and the pacing between the feed and the paint callback. This makes the visualizer
contract from
[components](../02-architecture/02-components.md#visualizer-subsystem) concrete, within
the call made in [ADR 8](../02-architecture/decisions/08-adr-visualizer-rendering.md)
(spectrum and waveform draw with gpui primitives; the generative visual waits on a real
GPU shader). Version-sensitive: the tap ring is rtrb, the FFT is hand-rolled, the paint
path is gpui's `canvas()`.

## From the tap to the feed

The engine's PCM tap is the input. It is a second rtrb SPSC ring beside the sample
ring, 16,384 `f32` samples (`TAP_SAMPLES` in `rox-playback/src/output.rs`), and the RT
callback pushes a post-volume copy of every stereo frame it plays and ignores push
failure. Lossy by design: a slow visualizer loses samples, never slows audio. This is
the same tap the [playback doc](01-playback.md#thread-and-channel-wiring) describes from
the producer side.

```
 RT output callback            UI pump (60 Hz)                 paint callback (canvas)
 ──────────────────            ───────────────                 ───────────────────────
 push post-volume  ──tap ring──▶ drain_tap: read all slots
 stereo frames                   push into AudioFeed
                                 (interleaved stereo)  ──feed──▶ latest_mono window
                                                                  Hann + FFT per zone
                                                                  fold bins to bar levels
                                                                  paint quads
```

The consumer side is one drain on the UI pump. `Player::drain_tap` (in
`crates/rox/src/player.rs`) runs on the pump timer, `PUMP_INTERVAL` = 16 ms so about
60 Hz, reads every available slot in one `read_chunk`, and pushes the two ring slices
into the `AudioFeed`. Nothing here is real-time; the RT boundary is the tap ring itself.

`AudioFeed` (`crates/rox-viz/src/feed.rs`) is the seam. A `Mutex<VecDeque<f32>>` of
interleaved stereo, newest at the back, capped at `KEEP_SAMPLES` = `MAX_FFT_SIZE * 2 * 2`
= 65,536 samples (the largest window with slack), older samples dropped off the front on
every push. Two atomics ride alongside: `sample_rate` (`AtomicU32`, set per session,
48,000 default) and `written` (`AtomicU64`, total samples ever pushed), which lets a
view tell silence, nothing new, from a repeat of the same window. `latest_mono(out)`
copies the newest `out.len()` frames folded to mono ((L+R)/2), newest last, and returns
how many landed; short means not enough buffered yet.

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
  lands near 1.0. Only the lower half-spectrum is returned (`size / 2` bins); the
  mirror above Nyquist is dropped.
- **Band mapping**: `log_bands(bands, lo_hz, hi_hz, sample_rate, half)` maps
  log-spaced bands across `lo_hz..hi_hz` to half-spectrum bin ranges, each at least one
  bin wide, so neighbours share bins where the FFT is too coarse to split them.

## Spectrum panel

`SpectrumPanel` (`crates/rox/src/panels/spectrum.rs`) owns the config and the bar state.
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
   and reset the level vectors. Each `Zone` carries its own `Analyzer`, a mono scratch
   buffer, and its slice of band bin-ranges.
3. If there is new audio since last tick (`written` moved), pull `latest_mono` per zone,
   run the analyzer, and set each bar's target from the band's peak magnitude in dB:
   `20 log10(peak)`, normalized from `FLOOR_DB` = -66 to `MAX_DB` = -12 and clamped to
   0..1. No new audio holds the targets until the feed has sat still past `SILENT_AFTER`
   = 0.15 s, then the bars fall to silence.
4. Ease each level toward its target, `ATTACK` = 40/s rising, `RELEASE` = 10/s falling.
   Peak-hold caps ride up with the bar and fall back under `cap_gravity`.
5. Set `alive` if any bar or cap is still above `EPSILON`.

`paint` draws the frame with gpui quads in a `canvas()` callback: dB gridlines, then per
bar a filled bar (flat accent, or a loudness gradient when `gradient` is on) or a hollow
outline, plus a peak-hold cap when `caps` is on. Both `step` and `paint` run inside the
paint callback on the UI thread. This is where implementation and the components boundary
part: the contract reads "analysis runs off the UI thread," but the spectrum FFT is cheap
enough per frame that it runs inline in paint; only the offline decodes below leave the
UI thread. The one exception the contract still holds for is the waveform precompute.

## Frame pacing

Two clocks drive redraws, so the panel animates while playing and settles cleanly when
it stops:

- **Playing**: the pump drains the tap and notifies, which repaints on the next frame.
  The feed's `written` counter moving is what `step` reads as fresh audio.
- **Not playing but still moving**: `body` calls `request_animation_frame()` while
  `bars.alive`, so the decay and the falling caps finish animating after audio stops
  without holding a frame loop open once they settle.
- **Frozen**: with `freeze` on and playback paused, `step` parks the levels and holds
  exactly where they are and stops animating; paint keeps showing the standing frame. A
  settings edit that remaps the bars still lands, because the feed keeps the last window
  and the frame re-analyzes at the new mapping.

A track loaded paused has never fed the tap, so the frozen bars would have nothing to
stand on. `Player::prime_feed` closes that: it decodes one window at the current
position off-thread (`engine::decode_window`, resampled to the device rate, interleaved
stereo) and pushes it into the feed so the frozen frame is real.

## Waveform peaks cache

The waveform seekbar draws from a min/max peak reduction of the whole track, cached to
disk so the strip comes back instantly after the first play instead of re-decoding.

`engine::decode_peaks(path, bins)` (`rox-playback/src/engine.rs`) is the reducer. It
decodes the whole file through the same path playback uses, no audio device, and folds
it to at most `bins` (min, max) mono pairs: a coarse pass of one pair per `BLOCK_FRAMES`
= 2048 frames keeps memory flat whatever the track length, then folds down to `bins`
keeping each bucket's extremes so transients survive. Pairs are normalized so the
loudest hits 1, with a `pow(0.7)` perceptual curve so quiet passages stay visible. The
waveform panel asks for `PEAK_BINS` = 2048 pairs and resamples that down to the drawn
bar count at paint time.

The cache is one small binary file per track under `waveforms/` in the app's data dir
(`crates/rox/src/peaks.rs`). The entry name is `{fnv1a(path):016x}.peaks`, an FNV-1a
hash of the path; the path stored inside disambiguates a hash collision. The layout,
little-endian throughout:

```
offset  bytes  field
0       8      magic  b"roxwave1"
8       8      source size   (u64)
16      8      source mtime  (u64, unix seconds)
24      4      path length N (u32)
28      N      path bytes
28+N    4      pair count C  (u32)
32+N    C*8    C pairs of (min, max) f32
```

The magic is the format version, bumped when the layout changes so old entries read as
misses and get rewritten. Load re-derives the source's `(size, mtime)`, and a mismatch
on size, mtime, path, magic, or a truncated body is a miss, not an error: the file was
edited or replaced, or the entry is stale or garbage, and the panel decodes fresh and
overwrites. `store` failures log and move on; a lost entry only costs a re-decode next
time.

The panel's load is one background task: `peaks::load` first, and on a miss
`decode_peaks` then `peaks::store`, all on the background executor so a long track's full
decode never touches the UI thread. A generation counter drops a result that lands after
the track already changed.

## Reference

The shared analysis lives in `crates/rox-viz`: `feed.rs` (`AudioFeed`, the tap-to-view
seam), `analysis.rs` (`Analyzer`, the Hann-windowed FFT and `log_bands`), `lib.rs`
(exports). The panels and the on-disk pieces live in `crates/rox`: `panels/spectrum.rs`
(`SpectrumPanel`, `SpectrumConfig`, the `Bars` state machine), `panels/waveform.rs`
(`WaveformPanel`, the peaks load and the morphing strip), `peaks.rs` (the cache format),
`player.rs` (`drain_tap`, `prime_feed`). The tap producer and the offline decoders
(`decode_peaks`, `decode_window`) are in `crates/rox-playback`: `output.rs`, `engine.rs`.
