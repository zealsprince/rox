# ADR 26: Last.fm sessions filed by the api key that minted them

**Status:** Decided

Decision: `accounts.json` holds a map of Last.fm sessions keyed by api key, not one
session. A build reads the entry for the identity it signs with, and finds nothing
where another identity connected.

Last.fm binds a session to the api key that authorized it. rox has more than one:
the nix package ships a pair minted for that channel, the release workflow signs
with the repository secret, a local `.env` build uses whatever the developer
registered, and a build shipping no identity at all takes the user's own pair from
the settings page. All of them read the same data directory. One session field
meant the last build to connect owned the file, and every other one sent a
perfectly well-formed call that Last.fm answered with error 9, forever, because
nothing about a rejected session gets better by waiting.

Filed by key, moving between installs costs one connect each and nothing after.
The pair the user types into the settings page falls out of the same rule: it's a
different identity, so it gets its own session instead of silently invalidating
the built-in one.

Three states have to be told apart, which is why an entry can be present and
empty. A key with a session is connected. A key with no entry has never asked. A
key with an empty entry asked and was refused, and that's worth writing down: the
alternative is trying a session already known not to be ours on every launch
for the life of the install.

The upgrade brings one session with no record of who minted it, so it goes into an
unattributed slot that any key may use. The first call that succeeds claims it, which
is the only proof of ownership available without asking the service; the first
call that comes back with error 9 files that key's refusal and leaves the session
for whichever build it belongs to. Someone running two installs keeps the one they
authorized and connects the other once.

A refusal is also the app's to notice rather than the log's. `track.scrobble` and
`track.updateNowPlaying` are fire and forget (a track that failed to
send is gone, and retrying a scrobble against the wrong clock is worse than
dropping it), but error 9 says something about the connection rather than the
track, so the result comes back far enough to drop the session and move the
settings page to Rejected. Before this, a dead session read as "Connected as
<name>" with a scrobble marker on the seek bar and nothing actually sent.

Alternatives: keep one session and re-authorize whenever it's refused, which works
until two installs take turns and each connect breaks the other. Key the sessions
by channel name rather than api key, which needs a build to know what channel it
is and still breaks when a channel rotates its pair. Ask the service to attribute
the carried-over session at load, which costs a network call on every launch to
answer a question the first scrobble answers for free.

Trade: the file grows an entry per identity that has ever connected on the
machine, and a build that rotates its api key reads as never connected rather than
as disconnected. Both are the same fact from the service's side, and a reconnect
settles it.
