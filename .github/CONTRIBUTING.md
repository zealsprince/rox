# Contributing

Glad you're here. Fixes, features, workspaces, docs, all welcome.

## Getting set up

The [Development section of the README](../README.md#development) has the whole
setup: the Nix shell or system packages, the gpui vendoring script, and the
optional `.env` for service keys. `cargo run -- --fresh` runs against a scratch
data directory, so you can break things without touching your real library.

## Before you build something big

For fixes and small improvements, just open a PR. For anything with real scope,
open an issue first so we can talk before you sink a weekend into it. If you'd
rather talk it through live, there's `#rox` on `irc.hivecom.net`
([web chat](https://hivecom.net/chat?channel=rox)). You're
free to fork and make feature branches, I just don't want to give the false
hope that it will instantly become mainline.

## Pull requests

- PRs target `main`, the only long-lived branch. Bleeding edge means building main.
- PRs are squash merged, so keep each one to a single logical change. The PR
  title becomes the commit message on main, write it like one.
- Run `cargo fmt` and `cargo clippy` before pushing. CI runs both plus the
  tests on every PR and has to go green before merge.
- Say how you tested it. For UI changes that means running the app, not just
  `cargo test`.
- Reviews need one approval and all threads resolved before merge.

## On AI

rox is largely written with AI tools, and the README says why. Contribute by
hand or with your own agent, whatever you work best with. Review treats both
the same.
