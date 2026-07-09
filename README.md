# rox

If Foobar2000 was made in the current year.

rox is a desktop music player for people with large, carefully tagged local libraries.
The UI is panels you compose yourself, duplicate with independent configs, and pop out
into real OS windows. Themes are token sets a person can share. Tagging is deep enough
to trust with a real collection, and the whole thing stays fast at tens of thousands of
tracks. Rust, built on GPUI, with Linux, Mac, and Windows all first-class.

## Why

Foobar2000's magic was a panel UI you build yourself, a theme community on top of it
(CaTRoX, NekoRoX, Georgia), and tag and library management that held up at scale. That
stack is stranded on Windows, and nothing on Linux or Mac covers all three legs: Tauon's
tagging is shallow, Strawberry isn't composable, Quod Libet isn't something you'd theme.
rox goes after that gap.

## Docs

| Section | Contents |
|---------|----------|
| [Product](docs/01-product/) | The problem, who it's for, the experience, scope |
| [Architecture](docs/02-architecture/) | The four domains, contracts, non-functional model, ADRs |
| [Implementation](docs/03-implementation/) | Per-domain schemas, formats, and sequences, written as they become real |
| [Research](docs/0R-research/) | Explorations that need a prototype before a decision |
| [Open Questions](docs/OPEN-QUESTIONS.md) | What's still undecided |

The [docs index](docs/README.md) lists every page with a one-line description.

## Status

Design phase. The spec is ahead of the code.
