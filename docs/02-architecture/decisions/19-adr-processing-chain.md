# ADR 19: Processing chain on the decode thread, output modes behind the backend seam

**Status:** Accepted

Decision: audio processing runs on the decode thread, after the stereo fold and
resample, immediately before the push into the sample ring, in two parts. Per-source
gain (ReplayGain, crossfade's fade curve) applies to each decoded source individually;
the chain of DSP nodes (EQ, any future effect) processes the single stream after
sources mix. The real-time callback is untouched: it keeps exactly its two jobs,
draining the ring and applying user volume. Crossfade is not a chain node; it is a
second decoded source mixed in the engine before the chain, so the ring keeps its
single producer. Exclusive output, deferred by [ADR 9](09-adr-audio-output.md), comes
into scope as a second backend behind the output seam that ADR 9 kept, and a bypass
rule ties the two halves together: what "bit-perfect" means is defined here, once, and
holds in both modes.

The callback's contract (no allocation, no locks, no I/O,
[ADR 2](02-adr-audio-stack.md)) rules out running DSP there, and the decode thread already owns every transform the samples go through: fold,
resample, trim. The chain slots in as the last step before the ring, which buys three
things. Chain output rides through flush, seek, and the gapless boundary like any other
sample data, with no new protocol. The PCM tap keeps working unchanged, so visualizers
see what the chain produced. And chain state carries continuously across the gapless
splice ([ADR 3](03-adr-gapless.md)), which is what an EQ needs at a track boundary,
filter history intact, no click.

The chain runs at the device rate, after the resampler. The alternative, running at the
source rate before resampling, means stateful nodes re-anchor on every track whose rate
differs, and an EQ shaped against a rate the device never plays. At device rate the
chain sees one stable rate for the life of the stream; the events that change it
(device switch, an exclusive-mode rate follow) are already stream rebuilds, and the
chain resets with the resampler.

A node's contract: process an interleaved stereo f32 buffer in place, same length out
as in, at the rate it was told at reset. `reset(rate)` is called at stream open and on
every discontinuity the engine already knows, the seek flush and the device rebuild,
and not at the gapless boundary. Nodes allocate at construction and reset, never in
process. Parameters are atomics shared with the UI, so a knob write is a store, no
command round trip; structural edits, adding, removing, reordering nodes, ride the
existing command channel like queue edits do. Nodes are zero-latency by contract:
anything that needs lookahead or introduces group delay (convolution, a limiter) is out
until a latency-reporting extension is worth designing, and the position clock stays
honest for free.

The bypass rule, which makes bit-perfect a checkable claim instead of a label: with the
chain empty or disabled, the samples pushed into the ring are the decoder's output
unchanged. The fold and resampler already honor this, a stereo source folds to itself
and the resampler is a passthrough at equal rates. That leaves the callback's volume
multiply, the one-node chain that predates this ADR. It stays in the callback: volume
must respond instantly, and a chain-side volume would lag by ring depth, up to 500 ms.
Instead, unity short-circuits, at `volume == 1.0` the callback skips the multiply
entirely; that skip is new work this ADR adds, today the multiply always runs. So the
honest claim is: chain off, volume at 100%, device rate equal to
source rate, and the device receives bit-identical samples. The UI states those three
conditions truthfully rather than showing a decoration; ReplayGain on is processing on
and reads as such.

Crossfade feeds the single-producer ring by never being a second producer. During a
fade window the engine holds two open sources, pulls chunks from both, folds and
resamples each, applies the fade gains, and pushes one summed stream; the chain then
processes the mix, so an EQ shapes the fade like anything else. Which boundaries fade
is adjacency the engine can already see: entries carry the group metadata
[ADR 17](17-adr-queue-continuation.md) introduced, same group means the gapless splice
untouched, different or absent group means fade, and a manual skip always fades since
it arrives as a command. ADR 17 decided that field but the engine's queue entries do
not carry it yet; plumbing it through is a prerequisite for crossfade, not part of this
ADR. The position clock flips inside the fade window: the new
track's segment registers at the fade midpoint, the frame the mix crosses half, so
MPRIS and the panels never announce a track before it is audible. The midpoint is a
constant to tune at implementation; the principle, one flip inside the window, is
fixed here.

ReplayGain is the first thing the processing layer ships, and it lives at the source
stage, not in the chain. The reason falls out of crossfade: a fade window has two
tracks live at once, each needing its own gain, so a single chain node multiplying the
mix would apply one track's gain to both. Each source multiplies by its own RG gain
(track or album gain per a setting, the tag's peak clamping the result), and during a
fade that factor folds with the fade gain into one multiply per source before the sum.
The gain rides with the track open, so it changes exactly where the source does.
Reading the RG tags at scan, surfacing them in the tag editor, and the gain-mode
setting are library work outside this ADR; the contract here only assumes a
gain-per-track arrives with the source.

Output modes: ADR 9's seam becomes two backends behind one contract. A backend receives
the ring, the shared atomics, and the tap, and reports what it negotiated: mode, rate,
format. Shared stays cpal as today. Exclusive is per-platform, ALSA `hw` direct on
Linux, WASAPI exclusive on Windows, CoreAudio hog mode on macOS, and follows the source
rate where the device allows, reopening the stream on a rate change; a boundary between
tracks of different rates costs an audible gap there, inherent to rate following, and
gapless within a rate is untouched. Failing to acquire the device (busy, unsupported)
falls back to shared with the state visible, never silence. The engine does not know
which backend runs; the bypass rule above is the part of the contract both must keep.

Alternatives: DSP in the callback, rejected on the callback contract and because every
node would inherit real-time constraints that Rust dependencies (biquad crates,
convolution) do not promise. A chain at source rate before the resampler, rejected
above on rate churn. Crossfade as a chain node, rejected because a node is 1:1 on one
stream and a fade needs two; putting the mix in the engine's source layer keeps the
node contract trivial. ReplayGain as a chain node, rejected above on the two-live-gains
problem a fade creates. Moving user volume into the chain to purify the callback,
rejected on the 500 ms knob lag; the unity short-circuit gets the same bit-perfect
result without it. A second ring and mixing in the callback, rejected as a rewrite of
the output layer's one-producer simplicity for no audible difference.

Trade: parameter changes are audible only after the ring drains, up to 500 ms behind
the knob. That is the price of the pre-ring placement and it is accepted; an EQ slider
feels slightly laggy where a callback-side chain would feel live, and in exchange the
callback stays provably allocation- and lock-free. Fade-window decoding runs two
decoders at once, a CPU bump bounded by the fade length. Exclusive output is
per-platform FFI beyond cpal, the cost ADR 9 deferred, now spent knowingly, and it
drags platform quirk surface (device claim failures, format negotiation) into the
support burden.

Open: whether the ring shortens while a chain editor is in the foreground to tighten
parameter latency, decided if the 500 ms lag annoys in practice; expect it to, half a
second between slider and ear is well past where adjusting by ear feels connected, so
this likely becomes real work when the EQ editor lands. The exact fade curve
and the midpoint constant. Whether exclusive mode on Linux targets the ALSA device
directly or through PipeWire's pro-audio profile, decided at implementation against
what devices actually expose.
