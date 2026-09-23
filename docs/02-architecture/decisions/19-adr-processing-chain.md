# ADR 19: Processing chain on the decode thread, output modes behind the backend seam

**Status:** Decided

Decision: audio processing runs on the decode thread, after the stereo fold and the
resample, immediately before samples are pushed into the ring. It comes in two parts
that run at different points.

Per-source gain, meaning ReplayGain and the crossfade curve, applies to each decoded
source on its own, while the sources are still separate. The chain of DSP nodes, meaning
the EQ and any future effect, processes the single stream after the sources have mixed
down to one.

The real-time callback is untouched by all of this. It keeps the two jobs it
already had: drain the ring, and apply user volume.

Crossfade isn't a node in the chain. It's a second decoded source that the engine mixes
in before the chain runs. That keeps the ring a single-producer structure, since a
second producer writing into it would break the one property the output layer is built
on.

Exclusive output, which [ADR 9](09-adr-audio-output.md) deferred, comes into scope here
as a second backend behind the output seam ADR 9 kept for this. A bypass rule
ties the two halves of this ADR together, by defining once what "bit-perfect" means so
that it means the same thing in both output modes.

**Why the decode thread.** The callback's contract, no allocation, no locks, no I/O
([ADR 2](02-adr-audio-stack.md)), rules out running DSP inside it. The decode thread is
where the samples already get transformed anyway, since it owns the fold, the resample,
and the trim, so the chain is joining work that's already happening rather than starting a
new stage somewhere.

Putting it last, immediately before the ring, buys three things. Chain output travels
through flush, seek, and the gapless boundary like any other sample data, so none of those
need a new protocol to handle processed audio. The PCM tap keeps working unchanged, and
because it's downstream, visualizers show what the chain actually produced rather than
what went into it. And chain state survives the gapless splice
([ADR 3](03-adr-gapless.md)), which an EQ needs: a filter keeps history, and resetting
it at a track boundary is audible as a click.

**Why the device rate.** The chain runs after the resampler, so it sees the rate the
device is playing at. The alternative is running before the resampler at the source rate,
and that costs twice. Stateful nodes would re-anchor on every track whose rate differs
from the last, so a filter's history would reset at arbitrary boundaries. And the EQ
would be shaped against a rate the device never actually plays, since everything gets
resampled after it.

At device rate the chain sees one stable rate for the life of the stream. The events that
can change it, a device switch or an exclusive-mode rate follow, are already stream
rebuilds, so the chain resets alongside the resampler and there's no new case to
handle.

**A node's contract.** Process an interleaved stereo f32 buffer in place, returning the
same number of samples that came in, at the rate it was given at reset. `reset(rate)` is
called at stream open and at every discontinuity the engine already recognizes, which
means the seek flush and the device rebuild. It isn't called at the gapless
boundary, since that's the case where a filter's history has to carry over. Nodes
allocate at construction and at reset, and never inside process.

Parameters are atomics shared with the UI, so turning a knob is a single store with no
command round trip in the way. Structural edits are different, since adding, removing, or
reordering nodes changes the shape of the thing being iterated, so those go through the
existing command channel the same way queue edits do.

Nodes are zero-latency by contract. Anything that needs lookahead or introduces group
delay, like a convolution reverb or a limiter, is out until a latency-reporting extension
is worth designing. In exchange, the position clock stays accurate without anyone
maintaining an offset: if no node delays the signal, the frame
being processed is the frame that will be heard.

**What the placement costs.** Running before the ring means a parameter write applies to
the samples being decoded right now, which are behind however much already-processed
audio the ring is holding. So a knob turn is heard after that audio drains, not
immediately.

The ring keeps its 500 ms of capacity, allocated once at stream open, because that's the
underrun cushion and shrinking it would mean reallocating underneath a live stream. What
changes instead is how full the decode thread allows it to get. While a chain editor is
open it holds a process-global refcount, and the push loop stops at 120 ms of buffered
audio, so the wait between moving a slider and hearing it is 120 ms rather than half a
second.

That leaves a thinner underrun cushion for as long as an editor is up. It's accepted
because an open editor is the moment a knob needs to respond, and the fill goes
back to the brim when it closes, with no stream torn down in either direction.

**The bypass rule** turns bit-perfect from a label into something you can check.
With the chain empty or disabled, the samples pushed into the ring are the decoder's
output unchanged. The fold and the resampler already satisfy this without special casing,
since a stereo source folds to itself and the resampler is a passthrough when the rates
match.

That leaves the callback's volume multiply, which is a one-node chain that predates this
ADR. It stays in the callback, because volume has to respond instantly and a chain-side
volume would lag by the full ring depth, up to 500 ms. The multiply is short-circuited at
unity instead: when `volume == 1.0` the callback skips it entirely, so the sample is
untouched rather than multiplied by one.

So the claim has three conditions: chain off, volume at 100%, and device rate equal to
source rate. Meet all three and the device receives bit-identical samples. The UI states
those conditions rather than showing a badge. ReplayGain counts as processing, and the
UI reports it that way.

Crossfade feeds the single-producer ring by never being a second producer. During a fade
window the engine holds two open sources, pulls chunks from both, folds and resamples
each, applies the fade gains, and pushes one summed stream; the chain then processes the
mix, so an EQ shapes the fade like anything else. The engine can already tell which
boundaries fade from adjacency. Entries hold the group metadata
[ADR 17](17-adr-queue-continuation.md) introduced. The same group keeps the gapless splice
untouched, a different or absent group means a fade, and a manual skip always fades
since it arrives as a command. The position clock flips inside the fade window: the new
track's segment registers at the fade midpoint, the frame the mix crosses half, so MPRIS
and the panels never announce a track before it's audible. The midpoint is a constant to
tune at implementation; the principle, one flip inside the window, is fixed here.

ReplayGain is the first thing the processing layer ships, and it runs at the source
stage, not in the chain. The reason falls out of crossfade: a fade window has two
tracks live at once, each needing its own gain, so a single chain node multiplying the
mix would apply one track's gain to both. Each source multiplies by its own RG gain
(track or album gain per a setting, the tag's peak clamping the result), and during a
fade that factor folds with the fade gain into one multiply per source before the sum.
The gain belongs to the open source, so it changes exactly where the source does.
Reading the RG tags at scan, surfacing them in the tag editor, and the gain-mode
setting are library work outside this ADR; the contract here only assumes a
gain-per-track arrives with the source.

Output modes: ADR 9's seam becomes two backends behind one contract. A backend receives
the ring, the shared atomics, and the tap, and reports what it negotiated: mode, rate,
format. Shared stays on cpal. Exclusive is per-platform (ALSA `hw` direct on Linux,
WASAPI exclusive on Windows, CoreAudio hog mode on macOS) and follows the source rate
where the device allows, reopening the stream on a rate change. A boundary between
tracks of different rates costs an audible gap there, which rate following can't avoid.
Gapless within a rate is untouched. Failing to acquire the device (busy, unsupported)
falls back to shared with the state visible, never silence. The engine doesn't know
which backend runs; the bypass rule above is the part of the contract both must keep.

Alternatives: DSP in the callback, rejected on the callback contract and because every
node would inherit real-time constraints that Rust dependencies (biquad crates,
convolution) don't promise. A chain at source rate before the resampler, rejected
above on rate churn. Crossfade as a chain node, rejected because a node is 1:1 on one
stream and a fade needs two; putting the mix in the engine's source layer keeps the
node contract trivial. ReplayGain as a chain node, rejected above on the two-live-gains
problem a fade creates. Moving user volume into the chain to purify the callback,
rejected on the 500 ms knob lag; the unity short-circuit gets the same bit-perfect
result without it. A second ring and mixing in the callback, rejected as a rewrite of
the output layer's one-producer simplicity for no audible difference.

Trade: parameter changes are audible only after the ring drains, 120 ms with an editor
open and up to 500 ms without. That's the price of the pre-ring placement. An EQ slider
feels a touch behind where a callback-side chain would feel live, and in exchange the
callback stays provably allocation- and lock-free. Fade-window decoding runs two decoders
at once, a CPU bump bounded by the fade length. Exclusive output is per-platform FFI
beyond cpal, the cost ADR 9 deferred. It also adds platform quirks (device claim
failures, format negotiation) to the support burden.

Open: whether exclusive mode on Linux targets the ALSA device directly or through
PipeWire's pro-audio profile, decided at implementation against what devices actually
expose.

**Amendment:** Linux went to ALSA directly, claiming `hw:CARD=x,DEV=n` with
`set_rate_resample(false)`, since that's the one path that works whether or not
PipeWire is in the picture. The fade midpoint the ADR left as a constant is half the
window: the new track's segment registers the frame the mix crosses it.
