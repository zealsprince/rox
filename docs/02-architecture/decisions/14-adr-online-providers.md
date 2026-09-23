# ADR 14: Per-domain provider traits for online enrichment

**Status:** Decided

Decision: online enrichment covers lyrics, tag lookup, and cover art, and each of those
gets its own trait: a lyrics provider, a metadata provider, an art provider. Every
service that can serve one implements it as a module in the app crate. Calls are
blocking and run on the background executor, and they return plain data. Every write
goes through a path that already exists, meaning the metadata writer, the lyrics save,
or the picture commit, so a provider never touches a file itself.

Alternatives: one service-shaped trait with capability flags, so a "Musixmatch module"
serves whatever it happens to support; no trait at all, with each service hardcoded
where it's used, which is the shape Last.fm has today; or provider extensions behind the
planned extension host.

Trade: splitting by domain matches the shape of the question the UI actually asks. A
lyrics panel needs lyrics for a track and has no opinion about which service supplies
them, so a trait per domain lets it ask once. It also turns falling back from one
service to another into a loop over the implementors, where the service-shaped
alternative would need an if-chain written out at every call site.

Service-shaped modules would model the APIs more faithfully, since a real service often
spans domains: MusicBrainz serves metadata and, through the Cover Art Archive, art too.
The cost is that it pushes the which-service question outward into every panel, which is
the decision panels shouldn't be making.

Hardcoding is fine while there's one service per domain, and Last.fm already showed
where that ends. The moment a domain has a second service, you need either a trait or a
copy of the first integration, and the copy is the thing that drifts.

The extension host is a different problem, [ADR 29](29-adr-source-contract.md)'s:
sandboxing untrusted code so it can act as an audio source. First-party HTTP fetchers
written by us don't need a sandbox, so making them wait on one would be paying for
isolation nobody asked for. If providers do eventually ship as extensions, the per-domain
trait is the surface the host would expose anyway, so nothing here is wasted.

HTTP goes through one shared blocking agent, ureq, which is already in the tree for
scrobbling. Every request carries an app User-Agent, which MusicBrainz requires outright
and every other service appreciates. An async client would mean a second runtime running
beside gpui's executor, and enrichment traffic is a handful of requests fired when a user
asks for something, so there's nothing for that runtime to do.

Rate limiting goes inside each service module rather than in the trait, so neither the
callers nor the trait ever see it. MusicBrainz's one-request-per-second limit is the case
that forces it, and it's a fact about MusicBrainz rather than about enrichment, so it
belongs where the other MusicBrainz facts are.

Caching starts in-memory and per-session, keyed by the query, and it caches misses as
well as hits so a repeated lookup for something nobody has doesn't re-ask. A persistent
cache would need a schema and an invalidation story, and nothing needs one until bulk
operations exist.

Enable state and credentials go in a providers section of the settings file, in the same
shape and with the same exposure as the Last.fm keys already stored there. The OS
keyring would mean a per-platform dependency, and what it would be guarding is a set of
API keys that these services hand out to anyone who asks.

Provider order is fixed in code, and what users get is a switch per provider. A
re-orderable priority list is settings surface that buys nothing while a domain has two
or three providers, and it's worth revisiting if that list ever grows.

A lookup produces a ranked list of candidates rather than a single answer. Each provider
hands back every result it found, and the aggregate scores all of them against the
track's own tags using one confidence function, built from title, artist, and album
similarity plus how close the durations are. Sorting on that score means the order
reflects how well a candidate matches the track, rather than which service happened to
return it, so a good match from a small service outranks a poor one from a large one.

Writing is a separate step that the user confirms. A picker shows the candidates with
the top score preselected, and nothing is saved until an explicit apply sends the pick
through the existing write path. The reason for the extra step is what the failure looks
like without it: auto-applying the best guess writes a wrong tag or the wrong lyric sheet
into the library silently, and a library the user can't trust is the opposite of what
enrichment is for. The scorer is shared across the domains for the same reason the traits
are split by them, since ranking releases and ranking lyric sheets are the same operation
over different candidates.

Lyrics take the one exception to confirm-before-write, because the destination makes it
safe. A fetched sheet saves into rox's own store by default, or as an `.lrc` sidecar
beside the file, and it only goes into the file's own tags when the user chooses that
destination explicitly. So the automatic path never touches the audio file and can't
write a tag nobody asked for. On that footing the lyrics panel is allowed to save a
single high-confidence match without showing the picker, where high-confidence means a
strong score against the track's own tags. The worst case there is a wrong sidecar that
the next fetch overwrites, which is recoverable in a way a silently wrong tag is not.
Tag lookup and cover art keep the picker, because those writes do touch the file.

Last.fm scrobbling stays outside this shape. It pushes listens outward on the player's
clock, with no user action involved, where a provider pulls data inward when someone asks
for it. The shared HTTP agent is the only thing the two have in common. If Last.fm ever
serves tag lookups, that would be a metadata provider module reusing its credentials,
which still isn't a reason to fold the scrobbler in.
