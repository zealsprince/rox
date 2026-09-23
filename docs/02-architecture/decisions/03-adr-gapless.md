# ADR 3: Gapless via an own single-stream, swap-decoder queue

**Status:** Decided

Decision: keep one long-lived cpal output stream and swap the Symphonia decoder
underneath at track boundaries, feeding a ring buffer the callback drains.

Alternatives: rodio's queue, the `playback-rs` crate.

Trade: gapless needs one stream for the whole session. Closing the stream at the end of
a track and opening a new one for the next puts a device teardown and re-acquisition
between them, and that takes long enough to hear. The only way to get a true gapless
boundary is to never break the stream and change what's feeding it instead.
`playback-rs` packages this pattern and is worth reading, but adopting it hands the core
playback loop, the part of rox that has to be right, to a small dependency.

Two costs come with owning it. The first is that the sample-accurate boundary is ours
to get right, since nothing under us is tracking where one track ends and the next
begins. The second is trimming. Lossy encoders pad the start and end of a file with
frames that aren't music. Playing them makes a "gapless" album click between tracks, so
the padding has to be found and dropped. Symphonia's support for that
is fragile today, including a known gap reading the LAME header where MP3 encoders
record it. So we read the delay and padding out of the tags ourselves and test the
result against real LAME and iTunes files rather than synthetic ones.
