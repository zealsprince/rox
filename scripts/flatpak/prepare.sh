#!/usr/bin/env bash
# Writes the tree Flathub builds one release from: the manifest with the rox
# source pinned to the release tag and commit, flathub.json, the crate list
# flatpak-cargo-generator derives from that commit's Cargo.lock. The
# metainfo isn't part of it: the build installs the one in the rox source,
# which Flathub requires, with its release list committed at each version
# bump. The checked-in manifest is
# never touched; it keeps pointing at the newest published tag so it builds
# as committed.
#
# Two callers: .github/workflows/flatpak.yml lints and builds the tree, and
# .github/workflows/flathub.yml regenerates it after the release exists and
# pushes it. Same script, same inputs, same files, so nothing is handed
# between the two.
#
# The copy under build/ is what flatpak-builder gets. It drops the tag line:
# on a release run the bundle is built before the release job creates the
# tag, and flatpak-builder refuses a git source whose tag it can't fetch.
# The commit alone pins the same tree.
#
# The lock is fetched at the commit rather than read from the checkout: on a
# manual dispatch the checkout is whatever ref the workflow ran from and may
# carry a newer lock than the version being rebuilt.
#
# Usage: scripts/flatpak/prepare.sh <version> <commit> <out-dir>
# Needs python3 with aiohttp and tomlkit.
set -euo pipefail

version=${1:?usage: $0 <version> <commit> <out-dir>}
commit=${2:?usage: $0 <version> <commit> <out-dir>}
out=${3:?usage: $0 <version> <commit> <out-dir>}

here=$(cd "$(dirname "$0")" && pwd)
manifest=com.zealsprince.rox.yml

# flatpak/flatpak-builder-tools has no tags; master as of 2026-09-12. The
# generator is a single file with no local imports.
tools_sha=de2225a6dee4818c1339b3cdbf29f90c471fcb7e

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$out/build"

# The crate list. gpui and gpui-component aren't in it: the lock carries them
# as path deps with no source, which is why the manifest fetches those two
# tarballs itself.
curl -fsSLo "$tmp/flatpak-cargo-generator.py" \
    "https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/$tools_sha/cargo/flatpak-cargo-generator.py"
curl -fsSLo "$tmp/Cargo.lock" \
    "https://raw.githubusercontent.com/zealsprince/rox/$commit/Cargo.lock"
python3 "$tmp/flatpak-cargo-generator.py" "$tmp/Cargo.lock" -o "$out/cargo-sources.json"

cp "$here/flathub.json" "$out/"

# The rox source is the block from its url line down to its commit line;
# projectM further down has a commit line of its own, which the range
# leaves alone.
rox='\|url: https://github.com/zealsprince/rox.git|,/commit:/'
sed -e "$rox{s|tag: .*|tag: v$version|;s|commit: .*|commit: $commit|;}" \
    "$here/$manifest" > "$out/$manifest"
sed -e "$rox{/tag: /d;}" "$out/$manifest" > "$out/build/$manifest"

# The crate list resolves beside the manifest, so the build copy needs it too.
cp "$out/flathub.json" "$out/cargo-sources.json" "$out/build/"
