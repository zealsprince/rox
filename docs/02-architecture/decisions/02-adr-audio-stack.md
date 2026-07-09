# ADR 2: Audio on cpal + Symphonia directly, not rodio

**Status:** Decided

Decision: build the playback pipeline on cpal (output) and Symphonia (decode) directly,
with our own decode thread, ring buffer, and mixer.

Alternatives: rodio (wraps cpal, adds a Sink/mixer/decoder), GStreamer, kira.

Trade: rodio is the fast path for "play a file, set volume," but it abstracts the frame
clock away, so no sample-accurate scheduling, and its seeking is young. For Foobar-grade
control, gapless, precise seek, a custom DSP and ReplayGain path, a visualizer tap, we
need the layer under rodio. The cost is that we build the queue and mixer ourselves.
GStreamer would give the widest format support for free but drags in a heavy C dependency
and a different threading model; kira is game-oriented and precise but built around a
different use case. cpal + Symphonia is the same stack Psst and termusic use, which
de-risks it.
