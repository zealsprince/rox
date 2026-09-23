# ADR 4: lofty for tags, with an atomic-write safety layer we own

**Status:** Decided

Decision: lofty as the single read/write metadata layer, wrapped in a copy-verify-rename
write path and per-file panic isolation.

Alternatives: stitch per-format crates (`id3`, `metaflac`, `mp4ameta`, `ape`), or use
Symphonia's metadata, which is read-only and so can't back a tag editor at all.

Trade: lofty is the only maintained crate that writes across the whole format matrix
behind one API. It covers ID3v2, Vorbis comments, MP4 atoms, and APE, including
multi-picture album art and text that survives CJK round-tripping. The per-format crates
are individually mature, `id3` especially, but taking them means four or five separate
APIs on their own release cadences, plus a dispatch layer to pick between them that
amounts to writing lofty's front end anyway.

What we take on in exchange is a real data-loss exposure. lofty rewrites tags in place
rather than through a temporary file, and it isn't crash-atomic; the maintainer has
confirmed that a failure partway through a write can leave a file unrecoverable. One
file lost that way is bad. Bulk editing is the feature this component exists for, so the
realistic case is a batch of several thousand files. A crash in the middle of that takes
whichever file was open at the time.

That's why the safety layer is part of this component's definition rather than something
bolted on later. A write goes to a copy. The copy is verified on both the metadata and a
hash of its audio stream, so a mangled write can't pass. It's then renamed over the
original in one atomic operation. A failure anywhere in that sequence unlinks the copy
and leaves the original untouched. Reads are isolated per file for the same
reason at a smaller scale: a malformed file that panics lofty's parser takes down one
worker rather than the batch around it. We keep `id3` in reserve for ID3 edge cases
lofty handles poorly.
