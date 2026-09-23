# ADR 22: External control over a local socket, schemas make the disk editable

**Status:** Decided

Decision: rox's machine interface is newline-delimited JSON-RPC over a local socket,
which is a Unix domain socket on Linux and macOS and a named pipe on Windows,
implemented in a `rox-ipc` crate. The protocol opens with a version handshake and then
carries two kinds of traffic. Request/response covers transport, queue edits, library
queries, and now-playing metadata. An event subscription pushes playback state, track
changes, and queue revision bumps out to whoever is listening.

MCP support is a separate thin stdio binary, `rox-mcp`, that proxies the socket, gated
behind an opt-in "Enable AI features" setting. Workspace files gain a JSON Schema derived
from the Rust bundle types, stamped into every saved file through `$schema`, and the
workspaces folder is watched so a file edited on disk re-applies live. An icecast
broadcast sink completes the surface on the audio side, where rox pushes the stream out
rather than serving it. rox never hosts an HTTP server.

The capability being adopted here is the one other players get by bundling a web server:
outside programs can read the library, pull metadata, and drive playback, so anyone can
build their own front end on their own machine. What's refused is the delivery mechanism,
for three separate reasons. A web UI would throw away rox's theming, which is most of
what the product is. A server means opening a port and then owning an authentication
story and an attack surface, which is a lot of security work for a music player to be
carrying. And the streaming that those servers usually exist to do is a different problem
with a different answer, covered by the icecast sink below.

A D-Bus extension was the other candidate, and it lost on platform reach. It's the
idiomatic answer on Linux and a foreign one on Windows and macOS, and rox ships on all
three, so choosing it would mean either a second mechanism for the other two platforms or
treating them as second-class. MPRIS still exists as the standard desktop shim through
souvlaki; the socket is the real surface behind it.

The socket authenticates through filesystem permissions, which is the same model mpv's
JSON IPC and mpd's protocol use, so the prior art is well worn. Its one real cost is that
a consumer needs a socket client where a web server would have let someone reach for
curl. That's acceptable because the consumers are programs rather than people at a
prompt, and the small bundled CLI covers the shell case while doubling as a reference
client for anyone writing their own.

Push matters as much as pull. A front end that can't subscribe to changes will poll for
them instead, which is wasteful and always slightly behind, so the event stream is part
of the contract from the first version rather than something added once someone
complains. It reuses the queue revision counter and engine command machinery that already
exist internally, so it's a new exposure rather than new plumbing.

The surface also includes a debug scope, things like a panel-tree dump and a settings
snapshot, which no external consumer has any use for. Those exist because the socket
doubles as the runtime test surface: with them, verifying what state the app is actually
in can be done against a live instance by a script or an agent instead of by a person
looking at the window. Anything about pixels stays a screenshot job.

MCP layers on cleanly because of a constraint in how clients work: they spawn stdio
servers as child processes, and a long-running GUI can't be one. Embedding MCP in rox
would force the HTTP transport this ADR refuses, so `rox-mcp` is a proxy binary
instead, stdio on one side, the socket on the other. Its tool surface is by
construction a subset of the native one, which keeps a single contract to version and
makes the MCP impossible to drift ahead of what the socket can do.

The "Enable AI features" toggle is on the Application settings page, off by default, and
reveals the MCP page and the ML models page. It gates what talks to AI tooling: the
MCP, and any future LLM-facing feature. The built-in acoustic analysis keeps running
either way, so nothing an existing library depends on changes when the toggle moves.

The icecast sink is the audio half of the refused web server. rox connects out to an
icecast server as a source client, encoding the processed stream beside ADR 19's
output modes. Everything downstream (the mount, the listeners, the network face) belongs
to icecast. Paired with the socket this completes the homegrown front end
story end to end: control over the socket, audio embedded from the stream, and rox
still owning no HTTP surface. The trade against serving audio directly is a required
external icecast instance, which is the point, since running one is a choice made
by someone who wants to broadcast rather than a port open on every rox install.

Workspaces are already one JSON file each on disk, so machine-editability is a schema
and a watch away. The schema is derived from the bundle types with schemars rather
than written by hand, because a hand-written schema drifts and a derived one can be
held to the types by a test comparing the committed file against the derive output.
It describes the current write shape only. The read side's legacy folding accepts old
shapes the writer never produces, and the schema doesn't cover them. With
`$schema` in every saved file, editors validate and autocomplete for free, and the
same hinting makes agent edits reliable. The watch on the workspaces folder
closes the loop: edit on disk, see it apply.

Out of scope: remote access to the socket (anyone who wants it can proxy it; rox
keeps the surface local), and the Jellyfin, Spotify, and YouTube integrations, which
point the opposite direction, rox as a client of remote services rather than a
service to local clients.
