# ADR 4: lofty for tags, with an atomic-write safety layer we own

**Status:** Decided

Decision: lofty as the single read/write metadata layer, wrapped in a copy-verify-rename
write path and per-file panic isolation.

Alternatives: stitch per-format crates (`id3`, `metaflac`, `mp4ameta`, `ape`), or use
Symphonia's metadata (read-only, so unusable for a tag editor).

Trade: lofty is the only maintained crate that writes across our whole format matrix
(ID3v2, Vorbis comments, MP4 atoms, APE) behind one API, including multi-picture album art
and CJK-safe text. The per-format crates are individually mature (`id3` especially) but
mean five APIs, five release cadences, and a dispatch layer we'd write anyway. The cost we
take on: lofty writes in place and is not crash-atomic, the maintainer confirms a failure
mid-write can leave a file unrecoverable. For bulk editing thousands of files that's a
real data-loss exposure, so the atomic-write layer (write to a copy, verify metadata plus
an audio-stream hash, atomically rename over the original, unlink on failure) is not
optional, it's part of this component's definition. We keep `id3` in reserve for ID3 edge
cases.
