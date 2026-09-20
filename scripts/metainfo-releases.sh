#!/usr/bin/env bash
# Fills the <releases> element of an AppStream metainfo file from the stable
# tags: one <release> per vX.Y.Z tag, newest first, each linking its GitHub
# release page. The source tree keeps a single marker line in there
# (<!-- rox:releases -->) so the file has one shape at every commit; this
# script splices the list over the marker in place.
#
# Run by the Linux package steps in .github/workflows/release.yml before
# the tarball, deb, and AppImage are assembled, and by
# scripts/flatpak/prepare.sh for the Flathub tree. All of them run on a
# checkout with no tags, so the tags are fetched here first. They also run
# before the release job has tagged the version being packaged, which is
# why it comes in as an argument: a stable version with no tag yet goes in
# at the top, dated today. A candidate never gets an entry either way.
#
# Idempotent: a file whose marker is already gone is left alone and the
# script exits 0, so running it twice on the same tree is safe.
#
# Usage: scripts/metainfo-releases.sh <path/to/rox.metainfo.xml> [version]
set -euo pipefail

file=${1:?usage: $0 <metainfo.xml> [version]}
version=${2:-}
marker='<!-- rox:releases -->'

if [ ! -f "$file" ]; then
    echo "$0: no such file: $file" >&2
    exit 1
fi

# Already filled: nothing to do.
if ! awk -v marker="$marker" 'index($0, marker) { found = 1 } END { exit !found }' "$file"; then
    exit 0
fi

# The release job's Linux leg runs in a container that checks the tree out
# as one uid and runs this as another, and git refuses to read a tree it
# doesn't own. The workflow steps that call git themselves mark the
# workspace safe first; this script runs git on its own, so it does too.
# Only under Actions, so a local run never edits anyone's global config.
if [ -n "${GITHUB_ACTIONS:-}" ]; then
    git config --global --add safe.directory "$PWD"
fi

git fetch --tags --quiet origin

# Stable tags only. Candidates carry a hyphen (v1.25.0-rc.1) and never get
# a release entry, the same filter release.yml uses to pick the previous
# release for its generated notes.
releases=$(
    git for-each-ref --sort=-creatordate \
        --format='%(refname:short) %(creatordate:short)' 'refs/tags/v[0-9]*' \
        | awk '$1 !~ /-/'
)

# The version being packaged, when its tag isn't there yet.
case "$version" in
    '' | *-*) ;;
    *)
        if ! git rev-parse --quiet --verify "refs/tags/v$version" >/dev/null; then
            releases=$(printf 'v%s %s\n%s' "$version" "$(date -u +%F)" "$releases")
        fi
        ;;
esac

# One element per tag, indented to sit where the marker sat. The indent is
# read off the marker line itself so the output matches whatever the file
# uses, and awk substitutes the block for that one line.
awk -v marker="$marker" -v releases="$releases" '
    index($0, marker) {
        match($0, /^[ \t]*/)
        indent = substr($0, 1, RLENGTH)
        n = split(releases, lines, "\n")
        for (i = 1; i <= n; i++) {
            if (lines[i] == "") continue
            split(lines[i], parts, " ")
            version = substr(parts[1], 2)
            date = parts[2]
            printf "%s<release version=\"%s\" date=\"%s\">\n", indent, version, date
            printf "%s  <url>https://github.com/zealsprince/rox/releases/tag/v%s</url>\n", indent, version
            printf "%s</release>\n", indent
        }
        next
    }
    { print }
' "$file" > "$file.tmp"

mv "$file.tmp" "$file"
