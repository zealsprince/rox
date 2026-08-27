# ADR 22: External control over a local socket, schemas make the disk editable

**Status:** Decided

Decision: rox's machine interface is newline-delimited JSON-RPC over a local socket, a
Unix domain socket on Linux and macOS and a named pipe on Windows, living in a
`rox-ipc` crate. The protocol opens with a version handshake and carries two kinds of
traffic: request/response for transport, queue edits, library queries, and now-playing
metadata, and an event subscription that pushes playback state, track changes, and
queue revision bumps. MCP support is a separate thin stdio binary, `rox-mcp`, that
proxies the socket, gated behind an opt-in "Enable AI features" setting. Workspace
files gain a JSON Schema derived from the Rust bundle types, stamped into every saved
file via `$schema`, and the workspaces folder is watched so an edited file re-applies
live. An icecast broadcast sink completes the surface on the audio side: rox pushes
the stream, it doesn't serve it. rox never hosts an HTTP server.

The capability being adopted is the one other players get from bundling a web server:
outside programs can read the library, pull metadata, and drive playback, so anyone
can build their own front end on their own machine. The bundled-server delivery of it
is refused. A web UI throws away rox's theming, a server means ports, an auth story,
and an attack surface that a music player has no business having, and the streaming
case those servers imply is a separate concern anyway.

A D-Bus extension was the other candidate and lost on platform reach: idiomatic on
Linux, foreign on Windows and macOS, and rox ships on all three. MPRIS stays as the
standard desktop shim via souvlaki; the socket is the real surface behind it. The
socket's auth is filesystem permissions, its prior art is mpv's JSON IPC and mpd's
protocol, and its cost is that consumers need a socket client where a server would
have offered curl. That's acceptable because the consumers are programs, and a small
bundled CLI covers the shell case while doubling as the reference client.

Push matters as much as pull. A front end that can't subscribe will poll, so the
event stream is part of the contract from the first version, reusing the queue
revision and engine command machinery that already exists internally rather than new
plumbing. The surface also includes a debug scope, things like a panel-tree dump and a
settings snapshot that no external consumer needs, because the socket doubles as the
runtime test surface: state-level verification against a live instance stops needing
a human's eyes, whether the client is a script or an agent. Pixels stay a screenshot
job.

MCP layers on cleanly because of a constraint in how clients work: they spawn stdio
servers as child processes, and a long-running GUI can't be one. Embedding MCP in rox
would force the HTTP transport this ADR refuses, so `rox-mcp` is a proxy binary
instead, stdio on one side, the socket on the other. Its tool surface is by
construction a subset of the native one, which keeps a single contract to version and
makes the MCP impossible to drift ahead of what the socket can do.

The "Enable AI features" toggle is in behavior settings, off by default, and
reveals the MCP page and the ML models page. It gates what talks to AI tooling: the
MCP, and any future LLM-facing feature. The built-in acoustic analysis stands on its
own and keeps running either way; with the toggle off the user stays on the
built-in version, and enablement only ever layers AI capability on top. Nothing an
existing library depends on changes when the toggle moves.

The icecast sink is the audio half of the refused web server. rox connects out to an
icecast server as a source client, encoding the processed stream beside ADR 19's
output modes, and everything downstream, the mount, the listeners, the network face,
belongs to icecast. Paired with the socket this completes the homegrown front end
story end to end: control over the socket, audio embedded from the stream, and rox
still owning no HTTP surface. The trade against serving audio directly is a required
external icecast instance, which is the point, since running one is a choice made
by someone who wants to broadcast rather than a port every rox user carries.

Workspaces are already one JSON file each on disk, so machine-editability is a schema
and a watch away. The schema is derived from the bundle types with schemars rather
than written by hand, because a hand-written schema drifts and a derived one can be
held to the types by a test comparing the committed file against the derive output.
It describes the current write shape only; the read side's legacy folding accepts
old shapes the writer never produces, and the schema owes them nothing. With
`$schema` in every saved file, editors validate and autocomplete for free, and the
same hinting makes agent edits reliable. The watch on the workspaces folder
closes the loop: edit on disk, see it apply.

Out of scope: remote access to the socket (anyone who wants it can proxy it; rox
keeps the surface local), and the Jellyfin, Spotify, and YouTube integrations, which
point the opposite direction, rox as a client of remote services rather than a
service to local clients.
