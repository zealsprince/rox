# Playback

How the playback engine is wired: threads, rings, the gapless boundary, seek, and the
position clock. This makes the playback contract from
[components](../02-architecture/02-components.md#playback-engine) concrete, within the
calls made in [ADR 2](../02-architecture/decisions/02-adr-audio-stack.md) (cpal +
Symphonia directly), [ADR 3](../02-architecture/decisions/03-adr-gapless.md)
(single-stream swap-decoder gapless),
[ADR 9](../02-architecture/decisions/09-adr-audio-output.md) (swappable output layer),
and [ADR 19](../02-architecture/decisions/19-adr-processing-chain.md) (the chain pre-ring
and the exclusive backends behind that seam).
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
  prev/volume/loop/quit)            processing chain                  write device format
                                    push f32 frames ────sample ring──▶ count frames played
 read atomics + segments ◀───────── shared state (Arc) ◀──────────────
 drain PCM tap ◀────────────────────────────────────────tap ring────── push pre-volume copy
```

- **Sample ring**: rtrb SPSC, `f32` interleaved stereo at the device rate, allocated
  once at stream open. Capacity 500 ms (`device_rate` / 2 frames, so 24,000 at 48 kHz).
  Deep enough that a 3 ms decode-thread nap or a metadata hiccup never starves the
  callback; shallow enough that it drains fast on flush. The capacity is fixed for the
  life of the stream. How full the decode thread lets it get isn't; see the fill gate
  under the decode loop.
- **PCM tap**: second rtrb SPSC, 16,384 samples. The callback pushes a pre-volume copy
  of every frame it plays and ignores push failure. A slow visualizer loses samples,
  never slows audio. Pre-volume, so the spectrum and signals track the program
  material, not the listening level; chain DSP still shows because it runs upstream.
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
  for f32, i16, u16, and i32 devices. At exactly 1.0 the multiply is skipped, which lets
  ADR 19's bit-perfect claim be checked instead of asserted.
- Folds stereo onto the device layout: mono devices get (L+R)/2, wider devices get L/R
  in the first two channels and silence in the rest.
- Increments `frames_consumed` by frames actually played. That counter is the global
  output clock everything else derives from.

No allocation, no lock, no logging, no I/O. All of it is one function, `output::fill`,
which every backend calls with a device buffer. No backend gets to do any of it
differently: two of them drifting on the unity short-circuit would make "bit-perfect"
mean two things, and ADR 19 defines it once.

## Output backends

ADR 9 kept the output layer swappable and ADR 19 spends the option, so `output.rs` holds
the seam plus the cpal implementation of it. A backend takes a `Request` (mode, device
id, rate), claims a device, and reports a `Negotiated` back: mode, device name, rate,
channels, format, and the reason it fell back if it did. The engine never sees which one
ran; it gets a ring to push into and a rate to resample toward.

- **Shared** is cpal, the default host, the picked device or the system default, and
  whatever config that device already runs at. `Request::rate` is ignored: the mixer
  owns the rate here.
- **Exclusive** is per-platform, one file each, picked by `cfg` in `output.rs`.
  - Linux is `output/alsa.rs`, the card claimed as `hw:CARD=x,DEV=n` with
    `set_rate_resample(false)`, the one ALSA setting that stops it from quietly
    resampling. Formats are taken best-first from float, s32, s16; packed 24-bit is
    skipped because rox has no three-byte sample type and the cards that offer it
    offer s32 as well. Instead of a callback, a writer thread blocks on `writei` per
    period (10 ms, four in the buffer) and calls the same `fill`.
  - Windows is `output/wasapi.rs`, the endpoint opened in
    `AUDCLNT_SHAREMODE_EXCLUSIVE` against the API directly, since cpal has no
    exclusive path. COM interfaces are apartment-bound and not `Send`, so one thread
    we own creates them, negotiates, and writes; `open` starts that thread and waits
    for it to report what it got. The format ladder is plain Rust above the FFI, so
    it compiles and gets tested on every host.
  - macOS is `output/coreaudio.rs`, the device taken in hog mode, which is the one
    thing CoreAudio offers that means this process and nobody else. With the HAL's
    mixer out of the path the nominal rate we set is the rate the converter runs at.
    There's no writer thread: CoreAudio pulls, so the HAL calls an IO proc on its own
    real-time thread and that proc calls `fill`.

  Anywhere else the seam returns a clear unsupported error and the settings page says
  "not on this platform" rather than offering a toggle that always falls back.

ALSA and CoreAudio have both been run on real hardware. WASAPI is written from the
platform contract and ships for testers, but no card has heard it here yet, so the
Audio page badges exclusive mode experimental on Windows and offers a prefilled issue
so a report arrives with the details a tester would forget.

A claim that fails (busy, no such device) opens shared instead and records the reason
in `Negotiated::fallback`, which the Audio page shows. Never an error, never silence.

Exclusive follows the file's rate, which isn't known until the decode thread opens the
file. So a session opens at the device's rate, and the player's pump compares the
playing track's rate against the running one and rebuilds when they differ, at the
cost of the gap ADR 19 budgeted for. Rates the device rejects are remembered, so a
card that can't match doesn't get asked again every tick. Two tests under `--ignored`
cover the hardware path, since they claim a real device: one checks the claim
negotiates and the output clock runs, one checks a busy device falls back to shared.

## The position clock

Positions are derived, never tracked separately, so they can't drift from what the
device actually played.

The decode thread appends a segment on every track open and every seek:

```
Segment { at_frame: u64, track: usize, track_frame: u64 }
```

`at_frame` is the value `frames_consumed` will have when the segment's first frame
plays; `track_frame` is where in the track that frame is, in device-rate frames.
Current position = find the last segment with `at_frame <= frames_consumed`, then
`track_frame + (frames_consumed - at_frame)`. UI reads are two atomic loads and a
short lock on the segment list.

The decode thread can predict `at_frame` because it maintains `pushed_playable`, its
count of frames pushed on the same clock, resynced to `frames_consumed` after every
flush (when the ring is empty and the two are provably equal).

## Gapless boundary

At end of stream the decode thread drops the finished Symphonia reader/decoder pair,
opens the next track's, registers a segment, and keeps pushing into the same ring under
the same live stream. No flush, no stream teardown, nothing at the output layer at all.
That's the whole mechanism.

Encoder delay and padding are handled inside the reader in symphonia 0.6. The MP3 demuxer
parses the Xing/LAME header into `Track::delay` / `Track::padding`, stamps every packet
with `trim_start` / `trim_end` in decoded frames, and the decoder applies the trim
before the engine sees samples. `Track::num_frames` already excludes trimmed frames.
FLAC needs none of this.

The boundary is checkable: `--count` decodes a file through the same code
path with no audio device and compares decoded frames against `Track::num_frames`.
Exact equality means the trim is exact and the boundary is sample-accurate by
construction; a LAME-encoded 3.000 s file at 44.1 kHz counts 132,300 both sides. A file
that misses (odd LAME variants, other encoders) gets trimmed in the decode loop from
`Track::delay` / `Track::padding` instead, the fallback ADR 3 budgeted for.

## Crossfade

The fade is not a chain node. During a window the decode thread holds two open
sources: the incoming one drives the loop as always, the outgoing one decodes
alongside in `Fade` and is mixed underneath before the chain runs, so the ring keeps
its single producer and an EQ shapes the fade like anything else. Per-frame gains come
from `gain.rs`, the source-gain stage ADR 19 put ahead of the mix; it's also where a
source's own constant gain applies, ReplayGain included.

The curve is equal power, `sin`/`cos` over the window, so two unrelated tracks hold
their level through the middle where a linear pair would sag. `Settings.crossfade_secs`
sets the length and zero disables it: no window ever opens, and every boundary is the
gapless splice above, unchanged. The engine caps a window at half the outgoing track,
so a fade longer than the track it leaves can't start before that track got going.

The album group (ADR 17) already on the queue entries decides which boundaries fade: two
tracks of the same album keep their splice, everything else fades, and repeat-one never
overlaps a track with itself. `Settings.crossfade_albums` overrides the group rule for
a listener who wants every boundary soft; repeat-one stays out even then.

A manual skip always fades. The flush protocol below still runs on a skip, so the fade
starts at the press rather than a ring later; the outgoing source is wound back to the
position clock, since the decode cursor had run up to a ring ahead of what was heard.
That wind-back happens before the flush, so its seek doesn't hold the silence open,
which leaves the clock a callback period further on than the spot it aimed at; the
difference is decoded and dropped on the far side, so the fade still starts on the
sample the cut landed on. Where the open source isn't the audible track (the gapless
preroll already swapped it, or another fade is halfway through) there's nothing to wind
back and the skip cuts. So does a skip while paused: nobody is hearing the old track,
and its tail would arrive as a surprise on the next Play.

The new track's segment registers at the fade midpoint rather than its first sample:
`open_at_from(pos, len / 2)`. The position clock, the track-change notification, and
MPRIS all flip there, so nothing announces a track before it's audible.

The window is published to the transport the same way, as the output frame it becomes
audible at plus its length, so the button and the ear get there together rather than the
button running a ring ahead. The skip control that started the fade shows an accent sweep
across it while the overlap runs, in the direction the queue moved; a boundary fade reads
as forward. Progress is quantized to 64ths in `PlayerView`, so a panel on the gated
observe is notified once per visible step instead of on every pump tick.

Two decoders run for the length of a window, a CPU bump bounded by the fade length.
The engine tests cover the boundary rule, the window math, and the loop-mode targets;
the mix and the curve are tested in `gain.rs`.

## ReplayGain

One multiply per source, applied where the fade curve is (`gain.rs`), before that
source's samples meet any other's. A crossfade forces that placement: a window has two
tracks live at once, and a single node over the mix would level both by one track's
number.

The scan pulls the four standard values off whatever tag the file has
(`replaygain.rs`; lofty maps them the same across ID3v2 TXXX frames, Vorbis comments,
and MP4 atoms), and they're written into SQLite as nullable columns, because 0 dB is a
measurement and a defaulted column couldn't tell it from an untagged file. The player
resolves them per path in the same lookup that resolves album groups and hands them to
the engine with the queue, so the engine still sees nothing but paths plus what the
library says about them.

Files no tagger ever analyzed get measured here. `analysis.rs` decodes a file end to end
through Symphonia and meters it per EBU R128 with `ebur128`, gated integrated loudness
against RG2's -18 LUFS reference, plus an oversampled true peak so an intersample peak
counts. `store::albums_missing_replaygain` hands the work back grouped by album and
`rox/src/replaygain_job.rs` steps through it one album at a time on a background worker,
polled for cancel every quarter second of decoded audio. An album is metered as one
program: the per-track histories merge before the gate runs, so the record's quiet
interlude drops out of the album figure the same way a quiet passage drops out of a track's.
Measuring only part of an album gets track values only, since a gain gated over half a
record is a number for a record that doesn't exist, and the tracks that already have
tags bring their own album figures.

A setting picks where the numbers go. The default writes them to the library database
through `store::set_measured_replaygain`, marked in `rg_source` as rox's own, so nothing
rewrites a file or bumps an mtime. The opt-in writes the four tags into the files
themselves through `writer::commit_replay_gain`, the tag editor's atomic layer, and then
reindexes the written paths so the row converges from disk. The precedence is one SQL
condition in the scanner's upsert: tags win over a measurement whenever a file has
them, and a measurement is kept through a rescan that still finds no tags. So a
library measured into the database stays measured, and a file someone later tags with
foobar2000 takes the tagger's numbers on the next scan.

`GainRule` turns the tags into the factor: which gain to read (off, track, album, each
falling back to the other where a file has only one), a preamp added to every
tagged gain, and a separate number for files with no tags at all. The tagged peak
clamps the result, so a boost never pushes a track past full scale, and a cut is left
alone. Off returns exactly 1.0 rather than a rounded one, so `gain::apply`
short-circuits and the samples reach the ring the bits the decoder produced.

The rule is sent over the command channel rather than shared as an atomic, so the engine
reads it when a source opens instead of per sample. A change relevels every source in
hand, both sides of a fade included, so switching mode is heard on the track playing
rather than the one after it, behind the same ring depth as every other parameter change.

The Audio page states what the library actually has
(`store::replaygain_breakdown`), split into tagged, measured, and missing, counted on
library events rather than per frame. The missing count is the measurement pass's work
list, and the button beside it starts the pass and turns into its progress.

## Seek

Seek must discard queued audio the callback hasn't played yet, and the producer of an
SPSC ring can't remove what it already pushed. The flush protocol:

1. Decode thread clears its own pending buffer and bumps `flush_seq`.
2. The backend, on its next run, sees an epoch it hasn't handled, discards the entire
   ring, emits silence, and stores the epoch in `flush_ack`.
3. Decode thread waits for the ack, then resyncs `pushed_playable = frames_consumed`
   and starts pushing.
4. `FormatReader::seek(Accurate, ...)`, `decoder.reset()`, new segment registered at
   the timestamp actually landed on (`SeekedTo::actual_ts`, which can differ from the
   request), decode resumes.

An epoch rather than a flag, because a flag has to be cleared and the clearing races
the callback already inside it: whichever way that race falls, the first milliseconds
of the new track get eaten or the ring holds stale audio. Handled-once has no such
window, and the ack replaces the fixed grace sleep that used to stand in for one.

What's left of the gap is one callback period, so the work either side is arranged
around that. Everything that doesn't depend on the cut happens before it, while the
ring is still playing: the next file's open, probe, and decoder build; the seek itself;
the wind-back of a track a crossfade is leaving. What runs after the ack is arithmetic
and a decode. Step 4's ordering above is logical rather than temporal for that reason:
the seek call is made before the flush, and only the segment it registers waits.

Track skip (next/prev) is the same protocol with a track open in place of the seek
call.

## Decode loop and conversion

Per iteration: drain commands, push pending samples until the ring fills, refill by
decoding packets until one yields frames. Decoded audio is copied out interleaved,
folded to stereo (mono duplicated, more-than-stereo takes the first two channels), and
resampled to the device rate.

Resampling is behind a push-a-chunk seam on the decode thread: linear interpolation
with one carried frame for chunk-boundary continuity, swappable for a windowed-sinc
resampler (rubato) without anything outside the decode thread noticing. Real
multichannel downmix slots into the same fold step.

The processing chain (ADR 19) runs last, after the fold and resample and immediately
before the push, so chain output goes through flush, seek, and the gapless boundary
like any other sample data and the PCM tap sees what the chain produced.
`Chain::reset(rate)` fires at stream open and on a device rebuild, never at the
gapless boundary, so filter history persists across a splice. An empty chain leaves the
buffer untouched: that's the bypass rule, held structurally rather than behind a flag.
User volume stays the callback atomic, so the two never meet in the same multiply.

Parameter latency falls out of that placement: a knob change is audible only once the
samples already in the ring drain past it, so the fill is gated (`latency.rs`). An open
chain editor takes a process-global hold, refcounted so a second editor can hold it
alongside the first, and while one is out the push loop stops at 120 ms of buffered
audio instead of the ring's full 500 ms. Nothing is resized or reallocated:
the excess drains once when the hold is taken, the depth comes back when the last one
drops, and the underrun cushion is thinner only for as long as someone is turning a
knob. The EQ window holds it for its lifetime, so the OS close button releases it the
same as the menu item does.

Failure shapes:

- Unreadable or unprobeable file: log, fall forward to the next queue entry.
- Corrupt packet (`DecodeError` / `IoError`): skip the packet, keep the track.
- Any other decode error: end the track, boundary logic takes over.
- Seek failure (unseekable source): position unchanged, error logged, playback
  continues.

`enqueue` is sent over the same command channel and appends to the decode thread's
queue; it never touches the ring, so it can't disturb what's already playing.

## Device loss and rebuild

When the device drops out or the backend faults, cpal calls the stream's error
function, which logs and sets `device_lost` on the shared state; every exclusive backend
sets the same flag its own way: the ALSA and WASAPI writers on a write they can't
recover, the CoreAudio side from a device-is-alive listener. Nothing else recovers on
the audio side, since neither the callback nor the writer runs again.

The player's pump notices the flag and calls `reopen_device`, which rebuilds the whole
session rather than swapping a stream: it pulls order, cursor, and position off the
dying session the same way the close-time persist does, then starts fresh against the
current output settings, whose default device is the reconnected or newly default one.
An output mode or device switch runs through the same rebuild, so both apply without a
restart. Everything denominated in the old device rate goes with it: the sample ring,
the resampler, the consumed clock, and the segment list. A disconnect mid-playback
resumes; a restore-shaped
start would otherwise come up paused. Album groups aren't persisted with the queue
because `start_session` re-derives them from the library on every start, restores
included. If no queue can be resolved the session is dropped and the transport falls
back to idle with an error, never a frozen "playing".

## Reference

The engine is in `crates/rox-playback`: `output.rs` (the backend seam, the shared
cpal backend, the callback) with `output/alsa.rs` (Linux exclusive),
`output/wasapi.rs` (Windows exclusive) and `output/coreaudio.rs` (macOS hog mode) under
it, `engine.rs` (decode thread, gapless, crossfade, seek, plus the offline decoders
`decode_peaks` and `count_frames`), `chain.rs` (the `Node` trait and the chain),
`eq.rs` (the ten-band parametric EQ node and the parameters the UI shares with it),
`gain.rs` (the source-gain stage, the ReplayGain rule, and the fade curve),
`latency.rs` (the refcounted hold that keeps the ring shallow while an editor is open),
`analysis.rs` (the R128 loudness and true-peak measurement, per track and per album),
`resample.rs`, `shared.rs` (atomics, segments). The tag side is in
`crates/rox-library/src/replaygain.rs`, and the app drives measurement from
`crates/rox/src/replaygain_job.rs`.
`crates/rox-prototype-playback` was the CLI harness over it (git history, commit
bd22dc1): `cargo run -p rox-prototype-playback -- <files>` plays with stdin
commands; `--count <files>` runs the silent gapless verification.
