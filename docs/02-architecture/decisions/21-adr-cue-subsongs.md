# ADR 21: CUE sheets as subsongs, spans in a side table

**Status:** Decided

Decision: a cue rip's tracks are ordinary rows in `tracks` under the subsong identity
`(source, path, sub)`, where `sub` is 0 for a plain file and the sheet's 1-based TRACK
number for a span of an image. The span itself lives in a `cue_tracks` side table keyed
by track id, rows existing only for cue tracks. The engine plays a span by opening the
image, seeking to the start, and treating the end boundary exactly as end-of-file.

A cue rip is one audio file, usually a whole-disc FLAC, split into tracks by timestamps
in a sidecar sheet. Supporting it means a track stops being a file, and the question is
where that break in the file-equals-track assumption gets absorbed.

Making cue tracks real rows absorbs it in one place. Playlists snapshot by track id,
listens attach by id, search and sort walk the projection's rows, and none of them care
that three rows share a path. The alternatives absorb it everywhere: synthetic fragment
paths (`album.flac#3` as the stored path) keep the row shape untouched but move the
burden onto every consumer that opens the file, where a missed fragment strip is a
silent bug that reads tag bytes out of nothing.

The `sub` column rides `tracks` itself because identity can't live in a side table: the
rescan upsert needs a conflict target, and a unique constraint doesn't span tables. A
nullable span column in the key fails quietly instead, since SQLite treats NULLs as
distinct in unique indexes, so every plain file would stop conflicting with itself and
rescans would duplicate the whole library. An integer defaulting to 0 keeps the upsert
honest and costs one header byte per row in SQLite's record format.

Everything bulky stays out of the main table. The projection is the read path at the 10
million track scale ADR 5 was validated at, and dense span columns there would be about
160MB of RAM paid by every library that owns no cue sheets. Sparse is the rule: a map
keyed by row, populated only for cue rows, which nothing on the hot paths reads. Search
and sort never touch spans; the player resolves one per track at insert time, beside the
album group and ReplayGain it already looks up.

The engine takes the span as the track's whole world: an accurate seek to the start at
open, a sample-accurate trim at both edges, and the end boundary taking the natural EOF
path so gapless, crossfade, stop-after, and loop semantics hold without knowing spans
exist. The head trim matters because an accurate seek lands on a packet boundary, and
without dropping the frames between the landing and the span start, each track replays
the tail of the one before it. Consecutive cue tracks of one image share an album group,
which is what keeps the crossfade rule from fading over a rip's gapless splices.

Scan-side, the sheet claims its image: the image file gets no row of its own while a cue
lists it, and rows key their freshness off the later of the sheet's and the image's
mtime, so editing either re-cuts. Deleting the sheet returns the image to one plain row
on the next scan. Metadata prefers the sheet and falls back to the image's tags, except
ReplayGain, where only the album pair is carried, since a whole-disc image's track tags
describe the disc rather than any one span.

Ratings and tag edits for a cue row never write to the file. The image is shared by
every track of the disc, so a per-track write would stamp them all; the writer refuses
the file half and the database keeps the value. Cue sheet editing, per-span waveform
peaks, per-span ReplayGain measurement, and embedded cuesheets (the FLAC CUESHEET block)
are all deliberately out: each is additive on top of this identity and none of them
bends it.
