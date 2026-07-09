# ADR 3: Gapless via an own single-stream, swap-decoder queue

**Status:** Decided

Decision: keep one long-lived cpal output stream and swap the Symphonia decoder
underneath at track boundaries, feeding a ring buffer the callback drains.

Alternatives: rodio's queue, the `playback-rs` crate.

Trade: this is the only way to get true gapless without tearing down and rebuilding the
stream between tracks. `playback-rs` packages the pattern and is worth reading, but
adopting it hands the core playback loop to a small dependency. The cost is that the
sample-accurate boundary is ours to get right, and the encoder delay/padding trimming that
real gapless needs is fragile in Symphonia today (a known MP3 LAME-header gap), so we trim
from tags ourselves and test against real LAME and iTunes files.
