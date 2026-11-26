# ADR 38: Renderers are confined by what they ask for

**Status:** Decided 2026-11-26, on spike LL-S; built for Linux, with [the guide](../../03-implementation/11-renderers.md#the-sandbox). macOS and Windows aren't built.

Decision: a renderer ([ADR 37](37-adr-render-targets.md)) runs in a sandbox by default.
Where the platform allows, rox starts it with no network of any kind and no filesystem
beyond its own package folder, read-only; a cache folder rox makes for it; and the
system's libraries and GPU device nodes. It can't read the user's files, rox's data or
rox's process, and can't reach the session bus or the display server. A renderer asks
for more in its manifest, the card names each ask, and a renderer that wants to run as
today asks to run unconfined.

**Context.** A package is a stranger's program that rox runs: a plugin or a renderer
([ADR 30](30-adr-plugins.md), ADR 37). Until now the model was consent. A card names
what rox gives the package, the user approves it at an exact hash ([ADR
35](35-adr-package-security.md)), and the program then runs with the user's full
permissions. The card said so: "It runs as a program on this computer with your
permissions. rox doesn't sandbox it." Measured on 2026-11-26, an approved renderer could
read rox's own environment through `/proc`, the user's SSH and config folders and rox's
data folder, and open any socket.

Three things rox does are hygiene, and none of them confines anything: a child inherits
only its pipes and the handles rox lends it; it gets an allowlisted environment; it dies
with rox.

ADR 30 refused sandboxing for plugins because a source or provider needs the network and
often runs helper programs. A renderer needs neither. It needs the GPU, its own folder,
the handles rox lends, and its pipes. Spike LL-S showed a renderer runs at full speed
with exactly that, on Linux, with no helper program and no privileges.

**1. A renderer is sandboxed by default.** A renderer that asks for nothing extra runs
confined. Andrew's call for 2.0, over shipping it opt-in first.

**2. The card says what is enforced.** A sandboxed renderer's card reads "Runs in a
sandbox. It can use the graphics card and its own cache, and can't read your files or
reach the network." The line is shown only when rox can enforce it on this system. On a
kernel that enforces less, the card adds a line for each thing it can't stop.

**3. A package asks for more in its manifest, and the card names each ask.** Under
`capabilities.renderer`: `"access": {"network": true, "read": ["pictures", "music",
"videos", "documents", "downloads", "library"]}`. Each is a grant like the palette or the
cover: named on the card, re-approved when it changes. `library` is every folder in
rox's library when the renderer starts, and the others are the user's standard folders
as the platform names them. Every read is read-only. Network access is TCP and UDP to
any address, this machine's included, and never a unix socket, so never the session bus
or the display server.

**4. Unconfined is a tier a package must ask for.** `"unconfined": true` gets today's
behaviour and a stronger card line. Nothing is forbidden; it is visible. The Renderers
page marks which tier each renderer is in.

**5. Where rox can't sandbox, it says so and asks.** On a system without the mechanism,
a renderer that would be sandboxed elsewhere shows a card that says it will run with
the user's permissions here. A renderer approved as sandboxed never silently runs
unconfined: if the mechanism goes away, or applying it fails, it switches off and asks
again. The approval records the tier the card read out, every grant and every gap in it,
so a manifest that adds an ask or `unconfined`, and a system that can enforce more or
less than it could, each need a fresh yes. A renderer approved before this record keeps
its approval and runs in the sandbox, which only takes access away.

**6. Plugins stay as they are for now.** They keep the consent model and gain what is
cheap: the allowlisted environment (done), and filesystem confinement to their package
and data folders once their declared `programs` are mapped to paths. Enforcing which
hosts a plugin may reach needs a proxy inside rox or a network namespace per plugin;
that is its own decision and is not made here.

**7. Linux first.** Landlock for files, TCP and signals, plus a seccomp filter that
denies `socket(2)`, which closes what Landlock on current kernels leaves open (UDP,
netlink, unix sockets by path). For a renderer granted the network the filter allows
only the internet families. macOS and Windows follow with their own mechanisms (a
Seatbelt profile; an AppContainer), each needing a spike. Until then they are case 5:
the card says the renderer runs with the user's permissions on this system.

**Consequences.** An author who wants a renderer to "just run" as before adds one line
to the manifest and gets a stronger card. Everyone else gets confinement for free.

A renderer can't keep config in the user's home, use an interpreter environment that
lives there, or call a local service over a unix socket. It vendors what it needs, which
ADR 30 already advises for the hash. Its home, caches and temp files point at its cache
folder.

A script renderer's interpreter can read the system's library tree. The boundary is the
same as a native renderer's: not home, not rox's data, not the network.

What the sandbox does not stop: a renderer hanging or resetting the GPU, which takes
rox's device with it (rox restarts on device loss); learning that a path exists through
`stat`; drawing anything it likes in its own target. A renderer granted the network can
reach any service on this machine that listens on a TCP or UDP port.

The catalog can require the sandboxed tier for what it lists by default, and show
unconfined packages behind a filter. Provenance is separate and still open: a publisher
key per catalog entry, so an update is known to come from whoever published the version
the user approved.

**Alternatives.** Opt-in for 2.0, a renderer asking to be sandboxed, with the default
flipped later. Every renderer written in the meantime would assume the user's home, and
flipping the default would break them all at once.

A network namespace for the no-network case. It takes away TCP and UDP and leaves unix
sockets by path reachable, which is how the session bus and the display are reached, so
the filter is needed anyway.

**Costs.** Nothing measurable in frame rate or frame time on the machine the spike ran
on, and none again for the five renderers it was built against: each drew at the same
rate confined and unconfined. Starting confined costs under a millisecond more. A cold
shader cache on a renderer's first start, since its cache folder is its own.
A renderer that needs something the profile doesn't name fails until its author asks
for it or ships it, and the profile's system paths are tested on NixOS only.

**Open.**

- Should the catalog refuse unconfined packages, or only mark them?
