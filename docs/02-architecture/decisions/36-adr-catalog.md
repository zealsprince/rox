# ADR 36: The catalog is a static index on one origin

**Status:** Decided 2026-11-15

Proposal: rox reads a catalog of workspaces, plugins and objects from one origin,
`https://catalog.rox.music`, compiled into the app. The catalog is a static JSON index plus a
record per package, the package files and their previews. An entry gets into the index
by review: a maintainer merges it, and the site's build computes each file's size and
sha256 from the committed file, so no author supplies a hash. A published file never
changes, and a new version is a new file.

rox browses the index in a catalog window, shows each package's card and previews, and
installs from it. An install downloads the file, checks it against the index's size and
hash, and hands it to the same import or install a file from disk takes, with the same
confirms ([ADR 35](35-adr-package-security.md)). An install link on the website is
`rox://install/<id>`, which opens the catalog window on that package. A link carries a
catalog id and never a URL, so rox only ever downloads what its own index names.

rox tracks what it installed from the catalog by the sha256 it verified, and a package
has an update when the index's latest hash differs from that one. No package version is
compared anywhere: a version is the author's own string, shown and never parsed. A file
imported by hand is never tracked. Previews and the oldest rox a package runs on are
kept in the catalog's record, so packages carry no catalog-only fields.

rox fetches the index when the catalog window opens. It also fetches it in the
background alongside the app's own update check, as long as something installed from the
catalog is on record. The last good index is kept on disk, and a failed fetch leaves it
in place for the window to show as a saved copy.

That shape comes from three things rox already holds to. Every request goes through one
blocking agent run on the background executor ([ADR 14](14-adr-online-providers.md)).
The update check runs at most once a day, only behind its switch, and a failed check
leaves the old cache for the next launch (`rox/src/startup/updates.rs:177-224`). And rox
works fully offline, with the network only adding
([scope](../../01-product/03-scope.md)). The catalog keeps all three: its index is one
more blocking GET, its background request waits on the same switch and the same
interval, and with no network it degrades to the saved copy or to nothing, with nothing
else in rox depending on it. rox stays a client. The install link is the OS handing rox
a URL, and rox opens no port for it ([ADR 22](22-adr-control-surface.md)).

Alternatives: a dynamic API with accounts. It brings server-side search, ratings and
uploads. It's a service to run and secure, and accounts mean requests that identify the
user, which ADR 35's network rule refuses.

Open submissions, published without review. They take review load off one person, and
they need moderation, takedown and publisher signatures before they're safe to point an
install button at.

GitHub releases as the catalog, each package a release in its author's repository. No
site to run. Every author's repository becomes an origin rox talks to, there's no review
between a release and an install, and browsing means an API call per repository.

No in-app browsing, links only, with the website as the whole catalog. It's the smallest
thing rox could do. The update check still needs an index to compare hashes against, and
installing would be a download followed by a file dialog, where an index lets rox verify
before anything reaches the import path.

Costs. A site to run and keep serving the same bytes for as long as a rox that reads its
layout is in use. Review load on one person, since every entry is merged by hand. A
second background request beside the update check for people who installed from the
catalog. The origin is compiled in, so moving it takes a rox release. And the hash is
only as trustworthy as the origin it came from, which ADR 35 records as the open gap.
