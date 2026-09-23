# ADR 2: Audio on cpal + Symphonia directly, not rodio

**Status:** Decided

Decision: build the playback pipeline on cpal (output) and Symphonia (decode) directly,
with our own decode thread, ring buffer, and mixer.

Alternatives: rodio (wraps cpal, adds a Sink/mixer/decoder), GStreamer, kira.

Trade: rodio is the fast path if what you want is "play a file, set volume." What it
costs is the frame clock. Its Sink hands you a play/pause/volume API and keeps the
output frame counter to itself, so there's no way to say "do this thing at exactly that
sample," and its seeking is young on top of that. Everything on the Foobar-grade list
needs that counter: gapless has to know the exact frame a track ends on, precise seek
has to land on a frame rather than near one, a DSP and ReplayGain path has to run at a
known point in the sample flow, and a visualizer tap has to pull the same samples the
device is about to play. So we work at the layer under rodio and build the queue and
the mixer ourselves.

The other two lose on different grounds. GStreamer would give the widest format support
for free, but it brings a heavy C dependency and its own threading model to get there,
which is a large thing to fit around a Rust app that already has a decode thread and a
real-time callback. kira is precise and well built, but it's aimed at game audio, where
the job is firing many short sounds with low latency rather than streaming long tracks
end to end with sample-exact boundaries between them.

cpal + Symphonia is the same stack Psst and termusic run on, so the combination has been
exercised by real players before us.
