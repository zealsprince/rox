# Implementation

Real and runnable: schemas, serialization formats, exact sequences, thread and channel
wiring, config. These docs consume the contracts in [architecture](../02-architecture/)
and make them concrete. Nothing here gets to move a boundary; when a contract doesn't
survive implementation, that goes back up to architecture, it doesn't get quietly
redesigned here.

An implementation doc gets written when its detail is real, prototyped or built, not
speculated ahead of the code. The set, one per domain:

- [01-playback.md](01-playback.md) - decode thread and RT callback wiring, ring buffer
  sizing, the gapless boundary swap and LAME delay/padding trimming, the flush protocol,
  the position clock, device switching, where ReplayGain is applied in the sample path
- [02-library.md](02-library.md) - the SQLite schema, the in-memory projection layout
  and interning, the scanner pipeline, the sharded cold-open load, and the
  rebuild-and-swap sequence that keeps store and projection consistent
- `03-metadata.md` - the copy-verify-rename sequence step by step, per-format tag field
  mapping (ID3v2 / Vorbis), batch semantics and failure shapes
- `04-artwork.md` - thumbnail DB schema and content-addressed keying, worker pool and
  texture LRU budgets, cancellation
- `05-visualizer.md` - PCM tap ring format, FFT configuration, the waveform cache file
  format, frame pacing between worker and paint callback
- `06-panels.md` - the layout and theme file formats, the panel config model,
  pop-out window mechanics and entity sharing
- `07-workspace.md` - crate layout, build commands, CI, the GPUI version pin policy
- `08-play-history.md` - the events schema and tag snapshot, listen-rule wiring against
  the position clock, rollup queries and their indexes
