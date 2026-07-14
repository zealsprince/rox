# Playback

How the playback engine is wired: threads, rings, the gapless boundary, seek, and the
position clock. This makes the playback contract from
[components](../02-architecture/02-components.md#playback-engine) concrete, within the
calls made in [ADR 2](../02-architecture/decisions/02-adr-audio-stack.md) (cpal +
Symphonia directly), [ADR 3](../02-architecture/decisions/03-adr-gapless.md)
(single-stream swap-decoder gapless), and
[ADR 9](../02-architecture/decisions/09-adr-audio-output.md) (swappable output layer).
Version-sensitive: the trim semantics below are symphonia 0.6, the stream API is cpal
0.18, the rings are rtrb.

## Thread and channel wiring

Three threads and two SPSC rings. The decode thread owns everything that can block; the
output callback owns nothing.

```
 UI / control thread                decode thread                     RT output callback
 ────────────────────               ─────────────                     ──────────────────
 Cmd over mpsc channel  ──────────▶ Symphonia decode                  pop stereo frames
 (play/pause/seek/next/             stereo fold + resample            apply volume
  prev/volume/loop/quit)            push f32 frames ────sample ring──▶ write device format
                                                                       count frames played
 read atomics + segments ◀───────── shared state (Arc) ◀──────────────
 drain PCM tap ◀────────────────────────────────────────tap ring────── push post-volume copy
```

- **Sample ring**: rtrb SPSC, `f32` interleaved stereo at the device rate, allocated
  once at stream open. Capacity 500 ms (`device_rate` / 2 frames, so 24,000 at 48 kHz).
  Deep enough that a 3 ms decode-thread nap or a metadata hiccup never reaches the
  callback; shallow enough that it drains fast on flush.
- **PCM tap**: second rtrb SPSC, 16,384 samples. The callback pushes a post-volume copy
  of every frame it plays and ignores push failure. A slow visualizer loses samples,
  never slows audio.
- **Commands**: `std::sync::mpsc` into the decode thread, drained with `try_recv` at
  the top of every loop iteration. The decode loop naps 3 ms when the ring is full and
  20 ms when idle, so worst-case command latency stays under one video frame.

The shared state is one `Arc`: atomics the callback may touch (`playing`, `flush`,
`volume_bits` as f32 bits in an `AtomicU32`, `frames_consumed`, `ended`) and two
mutex-guarded lists (`segments`, per-track display info) the callback never touches.

## The real-time callback

The hard line from the components spec, as actual rules:

- Pops the sample ring only in whole stereo frames (`slots() >= 2`), so interleave
  can't slip. A dry ring means underrun or end of queue: emit silence, count nothing.
- Paused: emit silence, pop nothing. The position clock freezes on the exact sample.
- Flush flagged: drain and discard the whole ring, emit silence, count nothing.
- Applies user volume (one atomic load, one multiply per sample) and converts f32 to
  the device sample format via cpal's `FromSample`. The stream is built generically
  for f32, i16, u16, and i32 devices.
- Folds stereo onto the device layout: mono devices get (L+R)/2, wider devices get L/R
  in the first two channels and silence in the rest.
- Increments `frames_consumed` by frames actually played. That counter is the global
  output clock everything else derives from.

No allocation, no lock, no logging, no I/O. The whole callback lives in one module
(`output.rs`) because ADR 9 wants a bit-perfect backend to be able to replace the
file, not the engine.

## The position clock

Positions are derived, never tracked separately, so they can't drift from what the
device actually played.

The decode thread appends a segment on every track open and every seek:

```
Segment { at_frame: u64, track: usize, track_frame: u64 }
```

`at_frame` is the value `frames_consumed` will have when the segment's first frame
plays; `track_frame` is where in the track that frame sits, in device-rate frames.
Current position = find the last segment with `at_frame <= frames_consumed`, then
`track_frame + (frames_consumed - at_frame)`. UI reads are two atomic loads and a
short lock on the segment list.

The decode thread can predict `at_frame` because it maintains `pushed_playable`, its
count of frames pushed on the same clock, resynced to `frames_consumed` after every
flush (when the ring is empty and the two are provably equal).

## Gapless boundary

At end of stream the decode thread drops the finished Symphonia reader/decoder pair,
opens the next track's, registers a segment, and keeps pushing into the same ring under
the same live stream. No flush, no stream teardown, nothing at all happens at the
output layer. That is the entire mechanism.

Encoder delay and padding live under the reader in symphonia 0.6. The MP3 demuxer
parses the Xing/LAME header into `Track::delay` / `Track::padding`, stamps every packet
with `trim_start` / `trim_end` in decoded frames, and the decoder applies the trim
before the engine sees samples. `Track::num_frames` already excludes trimmed frames.
FLAC needs none of this.

The boundary is checkable, not assumed: `--count` decodes a file through the same code
path with no audio device and compares decoded frames against `Track::num_frames`.
Exact equality means the trim is exact and the boundary is sample-accurate by
construction; a LAME-encoded 3.000 s file at 44.1 kHz counts 132,300 both sides. A file
that misses (odd LAME variants, other encoders) gets trimmed in the decode loop from
`Track::delay` / `Track::padding` instead, the fallback ADR 3 budgeted for.

## Seek

Seek must discard queued audio the callback hasn't played yet, and the producer of an
SPSC ring can't remove what it already pushed. The flush protocol:

1. Decode thread clears its own pending buffer and sets the `flush` atomic.
2. Callback, on its next run, drains and discards the entire ring, emits silence.
3. Decode thread waits until producer free slots equal capacity (ring empty), then
   25 ms more, one callback quantum of grace, then clears `flush` and resyncs
   `pushed_playable = frames_consumed`.
4. `FormatReader::seek(Accurate, ...)`, `decoder.reset()`, new segment registered at
   the timestamp actually landed on (`SeekedTo::actual_ts`, which can differ from the
   request), decode resumes.

The grace sleep leaves a hole: a callback that read `flush = true` just before step
3's clear can discard the first few milliseconds of post-seek audio. Worst case is one
callback quantum, on a user-initiated discontinuity. An acknowledged handoff (a flush
epoch the callback echoes back) closes it at the cost of one more atomic in the
callback; the sleep is the accepted trade until that cost is justified. Track skip
(next/prev) is the same protocol with a track open in place of the seek call.

## Decode loop and conversion

Per iteration: drain commands, push pending samples until the ring fills, refill by
decoding packets until one yields frames. Decoded audio is copied out interleaved,
folded to stereo (mono duplicated, more-than-stereo takes the first two channels), and
resampled to the device rate.

Resampling sits behind a push-a-chunk seam on the decode thread: linear interpolation
with one carried frame for chunk-boundary continuity, swappable for a windowed-sinc
resampler (rubato) without anything outside the decode thread noticing. Real
multichannel downmix slots into the same fold step.

ReplayGain folds into samples on the decode thread before the ring, so it rides
through flush and gapless like any other sample data; user volume stays the callback
atomic. The two never meet in the same multiply.

Failure shapes:

- Unreadable or unprobeable file: log, fall forward to the next queue entry.
- Corrupt packet (`DecodeError` / `IoError`): skip the packet, keep the track.
- Any other decode error: end the track, boundary logic takes over.
- Seek failure (unseekable source): position unchanged, error logged, playback
  continues.

`enqueue` rides the same command channel and appends to the decode thread's queue; it
never touches the ring, so it can't disturb what's already playing.

## Device switching

`set_output_device` tears down the stream and rebuilds against the new device: new
stream, sample ring re-allocated at the new device rate, resampler re-anchored. The
consumed clock and the segment list restart together, with one fresh segment at the
current track position, since old segments are denominated in the old device rate.
Hot-unplug surfaces as cpal's `ErrorKind::DeviceChanged` on the error callback and
lands on the same rebuild path.

## Reference

The engine lives in `crates/rox-playback`: `output.rs` (stream + callback), `engine.rs`
(decode thread, gapless, seek, plus the offline decoders `decode_peaks` and
`count_frames`), `resample.rs`, `shared.rs` (atomics, segments).
`crates/rox-prototype-playback` was the CLI harness over it (git history, commit
bd22dc1): `cargo run -p rox-prototype-playback -- <files>` plays with stdin
commands; `--count <files>` runs the silent gapless verification.
