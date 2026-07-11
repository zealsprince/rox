# Vision

If Foobar2000 was made this year.

## The problem

Foobar2000's real magic was never playback. It was two things stacked on top of each
other: a panel-composition UI where you build your own interface out of parts, and a
theme community (CaTRoX, NekoRoX, Georgia, Eole) that turned that system into things
that looked genuinely beautiful. Underneath both sat fast, deep tag and library
management that held up on libraries with tens of thousands of tracks.

That whole stack is stranded. Foobar is Windows in practice. The macOS build is a thin
beta, there's no native Linux, and Wine makes the theming fragile. The theming itself
is showing its age: panels wired together through a maze of dialogs, behavior scripted
in aging JScript panels that break between versions. You can still build a NekoRoX, but
you're maintaining a house of cards on an OS you may have left.

Nothing on Linux or Mac fills the gap. The alternatives each drop one of the three legs:

- **Tauon** is the closest in spirit and looks good, but its tagging and library
  management are shallow. For a large, meticulously tagged library it doesn't hold up.
- **Strawberry / Clementine** are solid players but generic. The UI is fixed, not
  composable, and not themeable in any real sense.
- **Quod Libet** is strong on tags and querying but ugly, and the interface isn't
  something you compose or theme.

So the gap is specific: no modern, cross-platform, GPU-accelerated player that puts
Foobar-grade tagging and library management behind a composable, themeable, pop-out-able
panel UI, with first-class visualizers, and stays fast on a huge library.

## Who it's for

**The Foobar refugee who moved off Windows.** Spent years building a CaTRoX or NekoRoX
setup, now lives on Linux or Mac, and has found nothing that feels like home. Andrew is
exactly this person: a library in the thousands of albums, a NekoRoX layout he still
misses, and Tauon on the Linux box that "just doesn't cut it."

**The library obsessive.** Tens of thousands of tracks, tags they actually care about,
who wants to browse an album-art grid, filter and query fast, and fix metadata in bulk
without the app choking. The value here is the library holding up at scale and the
tagging being good enough to trust.

**The aesthete.** Wants the player to look like something they chose, not a default. A
green NekoRoX-style build, a live visualizer on a second monitor, a layout tuned to how
they actually listen. For this person the look and the composability are the product,
not decoration on top of it.

These overlap heavily. The same person often is all three. First and most concrete is
Andrew, and building the thing he'd switch to is the sharpest test of whether it works.

## What success looks like

rox is the player Andrew switches to and stops missing NekoRoX. A large library loads
fast and browses without lag, the tagging is good enough to trust with a real collection,
and the window looks like something worth keeping open. If it clears that bar for him,
it's ready to share. Building it in that order keeps the scope honest.

Past that bar sits a longer life: sources as extensions. The same panels, visualizers,
and playback surface working against a Spotify, YouTube Music, or Tidal library view,
each maintained by the community rather than by rox itself. That's what keeps rox from
being only a tool for people with large local collections, and none of it displaces the
local core that earns the switch. [Scope](03-scope.md) carries the detail.
