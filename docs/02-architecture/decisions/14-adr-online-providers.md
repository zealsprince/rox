# ADR 14: Per-domain provider traits for online enrichment

**Status:** Decided

Decision: online enrichment (lyrics, tag lookup, cover art) is a set of per-domain
traits, a lyrics provider, a metadata provider, an art provider, each implemented by
per-service modules in the app crate. Calls are blocking, run on the background
executor, and return plain data; every write goes through the paths that already
exist (the metadata writer, the lyrics save, the picture commit). A provider never
touches a file.

Alternatives: one service-shaped trait with capability flags (a "Musixmatch module"
that answers whatever it can), no trait at all with each service hardcoded where it's
used (the Last.fm shape today), or provider extensions behind the planned extension
host.

Trade: per-domain traits match how the UI asks, a panel wants lyrics for a track and
doesn't care who answers, and they make fallback a loop over implementors instead of
an if-chain per call site. Service-shaped modules would mirror the APIs better
(MusicBrainz answers metadata and, through Cover Art Archive, art) but push the
which-service question into every panel. Hardcoding is fine for one service and
already showed its limit with Last.fm: the second service per domain means either a
trait or copy-paste. The extension host is #8's question, sandboxing untrusted code
for audio sources; first-party HTTP fetchers don't need a sandbox and shouldn't wait
on one. If providers later ship as extensions, the per-domain trait is the surface
the host would expose anyway.

HTTP is one shared blocking agent (ureq, already in the tree for scrobbling) with an
app User-Agent on every request, which MusicBrainz requires and every service
appreciates. An async client would add a second runtime beside gpui's executor to no
end; enrichment traffic is a handful of requests on user action. Rate limiting lives
inside the service module (MusicBrainz's 1 req/s is the forcing case) so callers and
the trait never see it. Caching starts in-memory per session, keyed by query,
negative results included; a persistent cache is a schema and invalidation story that
nothing needs until bulk operations exist.

Enable state and credentials live in a providers section of the settings file, the
same shape and the same exposure as the Last.fm keys already there; the OS keyring
would be a per-platform dependency guarding keys a music service hands out freely.
Provider order is fixed in code and users toggle providers on or off. Re-orderable
priority lists are settings surface that buys nothing at two or three providers a
domain; revisit if the list grows.

A lookup returns ranked candidates, not one answer. Each provider hands back every
result it found; the aggregate scores them all against the track's own tags with one
confidence function (title, artist, album similarity, plus duration proximity) and
sorts best first, so the ranking is the match quality rather than which service
answered. The write is a separate, confirmed step: a picker shows the candidates, the
top score preselected, and only an explicit apply saves the pick through the existing
write path. Auto-applying the best guess is a wrong tag or the wrong sheet written
silently, and the whole point of enrichment is a library the user trusts. The scorer
is shared across domains for the same reason the traits are: the tag lookup ranks
releases the same way lyrics ranks sheets.

Lyrics carve out one exception to confirm-before-write, on purpose, because the UX is
better for it. A fetched sheet saves to rox's own store by default, or an `.lrc` sidecar,
and only writes into the file's tags when the user picks the tag destination explicitly, so
the auto path never touches the audio and never writes a tag the user did not ask for. On
that footing the lyrics panel may auto-save a single high-confidence match, a strong score
against the track's own tags, without the picker: the worst case is a wrong sidecar the next
fetch overwrites, not a wrong tag written silently into the library. Tag lookup and cover
art keep the confirmed picker, since those writes touch the file.

Last.fm scrobbling stays standalone. It pushes listens out on the player's clock;
providers pull data in on user action. The shared agent is the only common ground. If
Last.fm ever answers tag lookups, that's a metadata provider module reusing its
credentials, not a reason to fold the scrobbler into this shape.
