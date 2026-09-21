#!/usr/bin/env bash
# Sets the workspace version for a release, in the one commit that makes it.
# Three things move together: the version in Cargo.toml, the workspace
# crates' entries in Cargo.lock, and, for a stable version, a <release> entry
# at the top of the metainfo's release list dated today.
#
# The release list is committed rather than filled at build time because
# Flathub builds from the tagged source and requires the metainfo it
# installs to come from upstream, complete: a list filled from beside the
# manifest doesn't count, and a list that's only a placeholder fails the
# linter. So the tag has to hold the finished file, and the bump is the last
# commit before the tag. .github/workflows/release.yml refuses a stable
# release whose version isn't the newest entry.
#
# A candidate (a version with a hyphen, 1.28.0-rc.1) gets no entry: Flathub
# tracks stable and rejects a metainfo whose latest release is a prerelease.
#
# Usage: scripts/bump-version.sh <version>
# Run it from the dev shell: it needs cargo to refresh the lock.
set -euo pipefail

version=${1:?usage: $0 <version>}
root=$(cd "$(dirname "$0")/.." && pwd)
metainfo="$root/crates/rox/assets/app/rox.metainfo.xml"

if ! printf '%s' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$'; then
    echo "$0: not a version: $version" >&2
    exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
    echo "$0: cargo isn't on the PATH; run this inside nix develop" >&2
    exit 1
fi

# The release entry first, so a refusal leaves Cargo.toml alone.
case "$version" in
    *-*)
        echo "candidate $version: no release entry"
        ;;
    *)
        if grep -q "<release version=\"$version\"" "$metainfo"; then
            echo "$0: the metainfo already lists $version" >&2
            exit 1
        fi

        # Newer than every entry already there, or Flathub's order check
        # fails at the next submission rather than here.
        newest=$(grep -m1 -o '<release version="[^"]*"' "$metainfo" | cut -d '"' -f2 || true)
        if [ -n "$newest" ] &&
            [ "$(printf '%s\n%s\n' "$newest" "$version" | sort -V | tail -n1)" != "$version" ]; then
            echo "$0: $version is not newer than $newest, the latest release listed" >&2
            exit 1
        fi

        # One entry under <releases>, indented to match the entries below it.
        awk -v version="$version" -v date="$(date -u +%F)" '
            /<releases>/ && !done {
                print
                match($0, /^[ \t]*/)
                indent = substr($0, 1, RLENGTH) "  "
                printf "%s<release version=\"%s\" date=\"%s\">\n", indent, version, date
                printf "%s  <url type=\"details\">https://github.com/zealsprince/rox/releases/tag/v%s</url>\n", indent, version
                printf "%s</release>\n", indent
                done = 1
                next
            }
            { print }
        ' "$metainfo" > "$metainfo.tmp"
        mv "$metainfo.tmp" "$metainfo"
        echo "metainfo: added $version"
        ;;
esac

# The workspace version is the first version line in the root manifest.
sed -i "0,/^version = \".*\"/s//version = \"$version\"/" "$root/Cargo.toml"
(cd "$root" && cargo update --workspace --offline --quiet)
echo "Cargo.toml and Cargo.lock: $version"
