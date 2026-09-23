# ADR 17: Queue continuation via a single provider feeding the live timeline

**Status:** Decided

Decision: when the upcoming portion of the timeline runs dry, playback continues by
default instead of stopping. A single active continuation provider, a trait over the
library and history stores, returns an ordered batch of tracks for "what plays next",
and the player appends that batch into the running engine as context entries through the
queue commands from [ADR 16](16-adr-play-queue.md). A provider is a selection strategy
(continue the browse order, shuffle the library, later a pool built from history), not a
source of audio. Remote sources (a streaming service) are the extension host's question
([ADR 29](29-adr-source-contract.md)); if one ever exists it implements this same trait
behind a layer that produces playable paths.

The trigger is in the player, on the pump's clock. The pump already ticks every 16 ms
and reads the queue snapshot and the position clock; when the audible cursor comes within
a small floor of the end of the upcoming portion (two tracks) and no loop mode is on, the
player asks the active provider for a batch on the background executor and applies the
result with the existing insert command, flagged context. An in-flight guard plus the
queue revision keeps one dry-out from firing twice. The engine isn't the trigger even
though it gets to the end first: its `pos` is the decode cursor and runs up to a ring
ahead of the speakers ([ADR 16](16-adr-play-queue.md)), and triggering there would mean
the audio thread calling into library stores, inverting the one dependency this design
keeps clean. The engine stays a decoder stepping through a list, and it doesn't know
continuation exists.

Growth is an append into the running session, never a successor session. The gapless
boundary ([ADR 3](03-adr-gapless.md)) holds because appending is just more entries in
`order` behind the append-only pool; the engine opens the next track for the boundary
exactly as it does mid-album. A successor session is a stream teardown and rebuild, the
same glitch ADR 16 refused for queue edits, plus a handoff of position, volume and
shuffle state that append gets for free.

**The contract.** A provider is handed a seed and a requested count, and returns an
ordered batch of track ids, each with an optional group id.

The seed holds two things. One is the scope the context was seeded from, meaning the
album, playlist, or library view that play started in. The other is the recent plays,
including any tracks that were explicitly queued. They're in there because they steer
the pool: queue a run of metal on top of a country context and the continuation
should follow the metal rather than the country.

Calls are blocking store queries on the background executor, which is the execution model
[ADR 14](14-adr-online-providers.md) already established. The player resolves the returned
ids to paths through the store's `paths_for` at insert time, the same hop the browse
panels make.

Layer one uses almost none of this. Its provider resumes the browse order of the view
play started in, and a library-shuffle provider is the obvious second. The
history-driven pools, meaning genre, artist, and the rollups from
[ADR 11](11-adr-play-history.md), come later and find their inputs already in the
contract. That's why it's specified in full now.

Exactly one provider is active at a time, and that's a real difference from ADR 14's
fallback chain rather than a coincidence of the current roster. There, an empty result
means "ask the next service." Here it means there's nothing left to continue with and
playback ends. If continuation fell through to a second provider, an empty batch would
silently become a change of taste, which is the one thing it must never do. Which
strategies exist beyond these, and how a user picks between them, is #36's question; this
ADR fixes the seam they plug into.

Loop suppresses the trigger entirely. Turning on loop is the user saying to stay here, in
this song or this album, so it narrows the selection range down to the list that already
exists and there's nothing for a provider to add. The pump doesn't fire while any loop
mode is on.

Shuffle folds appends in rather than suppressing them. With shuffle on, an appended batch
joins the upcoming permutation instead of sitting at the tail in the order the provider
returned it, so continuation doesn't visibly break the shuffle.

Play orders stay in the engine, flat, with grouping as metadata on the entries. This is
the answer album shuffle (#42) builds on: an entry optionally has a group id,
supplied by the player at insert time from the projection, which has album membership
where the engine sees bare paths and never will. When group ids are present the engine's
shuffle permutes groups as units and keeps each group's internal order; without them it
permutes entries as today. The alternative, computing orders in the player and pushing a
full permutation down, reopens the dual-copy problem ADR 16 already rejected;
grouping-as-metadata keeps the single owner and teaches the permutation one new trick.

Alternatives: stop when the context ends and make continuation an opt-in mode. That was
rejected as the product call. A local player that goes silent mid-flow feels broken, and
what continuation appends is ordinary context, visible in the timeline and removable.
Successor sessions, rejected above for the boundary. Triggering in the engine, rejected
above for the dependency and the decode-ahead clock. A provider fallback chain like ADR
14's, rejected because continuation is one strategy at a time, not a lookup racing
services for the best answer.

Trade: the trigger races the boundary. A slow provider near the last track can miss the
gapless window, the queue drains, and the batch arrives after playback has ended. The
worst case is a short gap rather than a wrong state, and the two-track floor makes it rare
since the queries are local. Continuation by default means rox plays things the user
didn't pick. What bounds that is the visibility argument above, plus the strategy being
the user's choice.

Resolved at implementation: a batch that arrives after the queue drained auto-resumes. It
needed no new mechanism. From the ended state the engine holds no open source, and its
insert path already routes the first of a batch through the nav path when there is
nothing playing, which reopens and clears the ended flag on the way. Waiting for a press
would have meant teaching the engine a second kind of insert, and it would have made the
race in the trade above audible as a stop rather than a gap. The trigger stays gated on
the session reading as playing, so a paused queue and an armed stop-after don't grow.

The floor and the batch size stay constants until real use argues otherwise.

**Amendment: the roster is a mode crossed with the play order.** #36's question is
answered by a `Mode` enum in `rox-playback/src/continuation.rs` that the playback
settings page enumerates. Off ends the queue when it ends, which is how rox behaved
before any of this. Continue resumes the browse order play started in and then the rest
of the library, and it's the default, so continuation is on out of the box. Weighted
draws from the whole library with never-played first and recent listens last, which is
what the play history ([ADR 11](11-adr-play-history.md)) is for.

What the ADR didn't anticipate is that the mode alone can't pick the provider. The
queue's own order has to win the draw, because a queue ordered by what sounds alike and
then refilled from browse order would answer two different questions in one session. So
`provider(mode, order)` takes both: Similar shuffle draws a radio batch whatever the
mode says, Random shuffle under Continue refills with a shuffle of the rest rather than
the next album down, and Weighted is left alone since its draw is already a shuffle over
history. A "radio" mode the listener picks separately was dropped on the way, since
turning on Similar shuffle is turning on radio, which is what it looked like it did
anyway. Still exactly one provider per draw, and it's built at the point of use so a
mode switched mid-query can't leave a live one behind.
