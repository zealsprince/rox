# ADR 29: Sources as rows under a source id, in-process first, host deferred

**Status:** Decided

Decision: a source is a catalog that becomes library rows plus a way to play them, the
pair the [scope doc](../../01-product/03-scope.md) names as a library provider and a
playback provider. Both halves are written in-process. The trait that would put them
behind one interface waits for a second implementor, and so does the extension host.

The library half is a source client in `rox-net` that speaks one server's API, blocks
like everything else in that crate, and returns plain data: `SourceTrack`,
`SourcePlaylist`, `SourceStation`. It never writes a file, never touches SQLite, and
never knows what a library row looks like. The service layer
(`rox-services/src/sources.rs`) maps that data onto rows filed under the source's own
id. A sync is a reconcile rather than an import: every track the server lists upserts
under the source id, and any row still filed under that id that the server no longer
lists is pruned. Both halves are scoped to the source string, so a sync can't reach a
local row whatever the server sends back.

The playback half is a playable reference stored on the row. A remote row keeps its
stream URL in `remote_url` and whether the stream ever ends in `remote_live`, and
`store::locators_for` hands the engine a `Locator::Remote` with that bare URL instead of
a path. Credentials are never stored with it. The request is finished per call through
an authorize table (`rox-services/src/sources_registry.rs`) that the binary fills at
startup, the same shape as `rox-panel-api`'s `openers`: the live source adds whatever
the request needs from settings, a fresh salt and token in Subsonic's case, headers for
a source that authorizes that way. Transport and decode stay in core. A source never
opens an audio device, never touches the ring, and never runs on the decode thread.

A PCM contract exists on paper for the case a reference can't express. librespot
decrypts and decodes Spotify's stream inside the extension, so what comes back is
samples plus a format description rather than a container rox can hand to symphonia.
Designing that half now means designing against one imagined implementor, so it waits
until a source forces it. When it arrives it has to plug into the single-stream engine
from [ADR 3](03-adr-gapless.md) rather than run beside it as a second playback path.

**Sources start in-process.** [ADR 14](14-adr-online-providers.md) made this argument
for its own domain and it transfers whole: "First-party HTTP fetchers written by us
don't need a sandbox, so making them wait on one would be paying for isolation nobody
asked for."

**The trait waits for its second implementor.** The proposal had both halves behind one
trait, written against the first two sources. Only one of them turned out to need a
client. Subsonic is a source client. Radio isn't: a station is an ordinary row under
`source = 'radio'` with the stream URL as its path, added by hand, found through the
radio-browser directory, or listed by a Subsonic server, and the queue, playlists,
history, search, and every panel handle it with no new code. With one client, a trait
would be a guess about what the second one needs. The client's types are the contract
until then, and they become the trait when a second client lands.

**Identity is source-qualified in memory as well as on disk.** The unique key is
`(source, path, sub)`. Local files are `local`, stations are `radio`, and each Subsonic
server digests to its own `subsonic:<digest>`, so two servers' catalogs never share a
row. The projection loads `source` as an interned column, and `TrackKey` is
`{ source, path, sub }`, so the currency the player and panels trade in stays
unambiguous when two sources hold rows with the same path. Every local write path in
`rox-library/src/store.rs` scopes itself with `source = 'local'` rather than assuming
it.

This is the part of the decision that gets expensive if it slips. With every row in
every library local, adding the column to the projection and the field to `TrackKey`
touches browse views and queue entries once, mechanically. Doing it after a source ships
turns it into a data migration plus an audit of every call site that quietly assumed a
path was unique, which is the retrofit the scope doc's "don't paint sources into a
corner" constraint was written to avoid.

**Two first sources: Subsonic, then radio.** Subsonic and its OpenSubsonic extensions
are the library-provider case. A real catalog with browse, search, artwork and
playlists, over a documented API, against a server the user runs. That last part earns
it the first slot: when it breaks, it broke because our client is wrong, not because a
company changed something overnight. The fragility that put Spotify and YouTube behind
extensions in the first place doesn't apply to a server the user administers. Several
servers can be configured at once, each an account with its own switch.

Web radio is the transport case. No catalog, an unbounded stream, metadata in band.
Between them the two exercise both halves of the contract, which one source alone can't.

The alternatives were building the host first, and making radio the first example
extension. Building the host first means designing a boundary with no implementor, and a
boundary with no implementor is wrong in ways nobody finds out about until something has
to live inside it. Radio first looks cheap because it's small. It's also the least
representative part of the surface: an unbounded stream with in-band metadata is the
hardest case in the transport half, and it proves nothing at all about browse, search,
or identity, which is where the retrofit cost is.

**The host mechanism stays open.** It gets its own ADR, decided on what a second source
client needs beyond the first one's types, how big the payloads are, and whether
anything needs audio bytes rather than a reference to them.
[ADR 24](24-adr-script-panels.md) says "WASM stays the right answer for the source and
playback extension host, which is a different problem with different constraints". This
narrows that line rather than contradicting it. WASM stays the likely answer for the
host once there is a host. What's added here is that the host is not what ships first,
and the contract it would expose gets written and used before the mechanism is chosen.

**The HTTP transport is in `rox-playback`, as a stated exception.** The layering says
all wire calls go in `rox-net`, blocking, on the background executor.
`rox-playback/src/http.rs` is the exception: an `HttpSource` that turns a seek into a
ranged GET for a remote file, and a `LiveSource` reading a station off a tape that a
feed thread keeps filling. `icy.rs` beside it strips the in-band station titles out of
the byte stream before the decoder sees them. It uses ureq, the same client `rox-net`
uses, so the workspace carries one HTTP stack.

It goes there because it isn't a wire call in the sense that rule is about. It's a byte
transport the decode loop pulls from synchronously, so it can't run on the background
executor at all, and pretending otherwise would put a channel hop in the middle of the
decode path. `rox-net` also can't host it as things stand. Doing so means either a
dependency on `rox-playback`, when `rox-net` depends on `rox-core` alone, or taking on a
symphonia trait it has no other reason to know about.

The alternative, written down so flipping it stays cheap: the reader moves to a
`rox-net::stream` module, `rox-playback` gains a dependency on `rox-net`, and only the
`MediaSource` impl stays behind. That's a file move and one Cargo.toml line. The types
on either side of the seam are the same in both arrangements.

**Both sources are Full tier, and radio shows the tier model is incomplete.** The scope
doc grades sources Full, Tapped, Remote by capability, where Full means the source
provides decodable audio and therefore gets gapless, ReplayGain, and visualizers. Radio
is Full by that definition: it plays through the engine and the visualizers work.
Gapless and ReplayGain have nothing to act on, because there's no next track to prepare
and no per-track measurement to apply. The tier grades what the source hands over, not
what the content supports, and for a live stream those come apart. That's an amendment
to what Full means, not a fourth tier. A "Full, live" tier would have one member and no
second axis behind it.
