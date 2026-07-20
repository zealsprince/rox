# ADR 17: Queue continuation via a single provider feeding the live timeline

**Status:** Decided

Decision: when the upcoming portion of the timeline runs dry, playback continues by
default instead of stopping. A single active continuation provider, a trait over the
library and history stores, answers "what plays next" with an ordered batch of tracks,
and the player appends that batch into the running engine as context entries through the
queue commands from [ADR 16](16-adr-play-queue.md). A provider is a selection strategy,
continue the browse order, shuffle the library, later a pool built from history, not a
source of audio. Remote sources (a streaming service) are the extension host's question
(#8); if one ever exists it implements this same trait behind a layer that produces
playable paths.

The trigger lives in the player, on the pump's clock. The pump already ticks every 16 ms
and reads the queue snapshot and the position clock; when the audible cursor comes within
a small floor of the end of the upcoming portion (two tracks) and no loop mode is on, the
player asks the active provider for a batch on the background executor and lands the
result with the existing insert command, flagged context. An in-flight guard plus the
queue revision keeps one dry-out from firing twice. The engine is not the trigger even
though it reaches the end first: its `pos` is the decode cursor and runs up to a ring
ahead of the speakers ([ADR 16](16-adr-play-queue.md)), and triggering there would mean
the audio thread reaching into library stores, inverting the one dependency this design
keeps clean. The engine stays a decoder walking a list; it does not know continuation
exists.

Growth is an append into the running session, never a successor session. The gapless
boundary ([ADR 3](03-adr-gapless.md)) holds because appending is just more entries in
`order` behind the append-only pool; the engine opens the next track for the boundary
exactly as it does mid-album. A successor session is a stream teardown and rebuild, the
same glitch ADR 16 refused for queue edits, plus a handoff of position, volume and
shuffle state that append gets for free.

The contract: the provider receives a seed, the scope the context was seeded from (the
album, playlist, or library view play started in) plus the recent plays including
explicitly queued tracks, and a requested count. It returns an ordered batch of track
ids, each with an optional group id. Calls are blocking store queries on the background
executor, the execution model [ADR 14](14-adr-online-providers.md) already set. The
player resolves ids to paths through the store (`paths_for`) at insert time, the same way
the browse panels do. Recent plays are in the seed on purpose: queue metal over a country
context and the pool should follow the metal. Layer one ignores most of this, its
provider resumes the browse order of the originating view, and a library-shuffle provider
is the obvious second; the history-driven pools (genre, artist, the rollups from
[ADR 11](11-adr-play-history.md)) come later and find their inputs already in the
contract. Exactly one provider is active at a time. This is not ADR 14's fallback chain:
an empty batch means there is nothing left to continue with and playback ends, it must
not mean "try a different taste". Which strategies exist beyond these and how the user
switches between them is #36's question; this ADR fixes the seam they plug into.

Loop suppresses the trigger. Loop is the user saying remain here, in this song or this
album; it narrows the selection range to the list that already exists, so the pump does
not fire while a loop mode is on. Shuffle folds appends in: when shuffle is on, a landed
batch joins the upcoming permutation instead of sitting in provider order at the tail.
Shuffle on means shuffle everywhere.

Play orders stay in the engine, flat, with grouping as metadata on the entries. This is
the answer album shuffle (#42) builds on: an entry optionally carries a group id,
supplied by the player at insert time from the projection, which knows album membership
where the engine sees bare paths and never will. When group ids are present the engine's
shuffle permutes groups as units and keeps each group's internal order; without them it
permutes entries as today. The alternative, computing orders in the player and pushing a
full permutation down, reopens the dual-copy problem ADR 16 already rejected;
grouping-as-metadata keeps the single owner and teaches the permutation one new trick.

Alternatives: stop when the context ends and make continuation an opt-in mode, rejected
as the product call, a local player that goes silent mid-flow feels broken, and what
continuation appends is ordinary context, visible in the timeline and removable, not
hidden state. Successor sessions, rejected above for the boundary. Triggering in the
engine, rejected above for the dependency and the decode-ahead clock. A provider
fallback chain like ADR 14's, rejected because continuation is one strategy at a time,
not a lookup racing services for the best answer.

Trade: the trigger races the boundary. A slow provider near the last track can miss the
gapless window, the queue drains, and the batch lands after playback has ended; the worst
case is a short gap, not a wrong state, and the two-track floor makes it rare since the
queries are local. Continuation by default means rox plays things the user did not pick;
the bound is the same visibility argument as above plus the strategy being the user's
choice.

Open: the floor and the batch size are constants until real use argues otherwise. Whether
a batch that lands after the queue drained auto-resumes or waits for a press is decided
at implementation. The provider roster and its selection UI is #36.
