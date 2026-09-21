#!/usr/bin/env bash
# Wraps the release binaries into a single-file AppImage. The release
# workflow's "Package (Linux AppImage)" step calls this after `cargo build
# --release`; run it by hand from the repo root to
# reproduce that locally. Nothing gets bundled beyond rox itself: every
# library rox links is on the AppImage excludelist or is glibc, so the host
# supplies Vulkan, ALSA, fontconfig and the rest.
# Usage: scripts/appimage/build.sh <version> <out-dir>
set -euo pipefail

VERSION=${1:?usage: build.sh <version> <out-dir>}
OUT=${2:?usage: build.sh <version> <out-dir>}

# Resolve the output dir against the caller's cwd before moving to the repo
# root, so a relative "." still means where the workflow ran us.
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
cd "$(dirname "$0")/../.."

APP_ID="com.zealsprince.rox"
ASSETS="crates/rox/assets/app"
TOOLS="${RUNNER_TEMP:-/tmp}"

# appimagetool and the static type2 runtime, pinned by hash. The runtime
# is the piece a user actually runs; the static build drops the libfuse2
# host requirement the old runtime had.
APPIMAGETOOL_URL="https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage"
APPIMAGETOOL_SHA="ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0"
RUNTIME_URL="https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64"
RUNTIME_SHA="2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d"

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
APPDIR="$WORK/AppDir"

# Downloads once into the tools dir and refuses to run anything whose hash
# doesn't match the pin above.
fetch() {
    local url=$1 sha=$2 dest=$3
    if [ ! -f "$dest" ]; then
        curl -fsSL --retry 3 -o "$dest" "$url"
    fi
    echo "$sha  $dest" | sha256sum -c -
}

for bin in rox rox-mcp; do
    if [ ! -x "target/release/$bin" ]; then
        echo "target/release/$bin is missing; build first" >&2
        exit 1
    fi
done

# Binaries and the launcher.
mkdir -p "$APPDIR/usr/bin"
cp target/release/rox target/release/rox-mcp "$APPDIR/usr/bin/"
install -m 755 scripts/appimage/AppRun "$APPDIR/AppRun"

# Desktop entry under the reverse-DNS id, with the icon name to match. Both
# Exec= lines stay as shipped: AppRun is what runs, appimagetool only wants
# the file well formed. A copy at the root is what appimagetool reads.
mkdir -p "$APPDIR/usr/share/applications"
sed "s/^Icon=rox$/Icon=$APP_ID/" "$ASSETS/rox.desktop" \
    > "$APPDIR/usr/share/applications/$APP_ID.desktop"
ln -s "usr/share/applications/$APP_ID.desktop" "$APPDIR/$APP_ID.desktop"

# Icons: the SVG as is, and a 256px PNG when ImageMagick is around to make
# one. Without it the 2048px source goes in unchanged; menus scale it.
mkdir -p "$APPDIR/usr/share/icons/hicolor/scalable/apps" \
    "$APPDIR/usr/share/icons/hicolor/256x256/apps"
cp "$ASSETS/rox-music.svg" "$APPDIR/usr/share/icons/hicolor/scalable/apps/$APP_ID.svg"
png="$APPDIR/usr/share/icons/hicolor/256x256/apps/$APP_ID.png"
# ImageMagick 7 ships the tool as `magick` and 6 as `convert`, so both
# names are tried; the first release build checked only `convert` and
# shipped the 2048px source into the 256px slot.
if command -v magick >/dev/null 2>&1; then
    magick "$ASSETS/rox.png" -resize 256x256 "$png"
elif command -v convert >/dev/null 2>&1; then
    convert "$ASSETS/rox.png" -resize 256x256 "$png"
else
    echo "ImageMagick not found, shipping the 2048px icon as is"
    cp "$ASSETS/rox.png" "$png"
fi
cp "$png" "$APPDIR/$APP_ID.png"
ln -s "$APP_ID.png" "$APPDIR/.DirIcon"

# Metainfo: the committed file, release list and all. The id is already
# right; only the launchable has to follow the desktop file's
# new name, and only in this copy.
mkdir -p "$APPDIR/usr/share/metainfo"
sed "s|rox\.desktop</launchable>|$APP_ID.desktop</launchable>|" \
    "$ASSETS/rox.metainfo.xml" \
    > "$APPDIR/usr/share/metainfo/$APP_ID.metainfo.xml"

cp LICENSE scripts/dist/README.txt "$APPDIR/"

fetch "$APPIMAGETOOL_URL" "$APPIMAGETOOL_SHA" "$TOOLS/appimagetool-x86_64.AppImage"
fetch "$RUNTIME_URL" "$RUNTIME_SHA" "$TOOLS/runtime-x86_64"
chmod +x "$TOOLS/appimagetool-x86_64.AppImage"

# --appimage-extract-and-run because the build container has no FUSE. No
# -u update information on purpose: the in-app updater handles updates.
ARCH=x86_64 "$TOOLS/appimagetool-x86_64.AppImage" --appimage-extract-and-run \
    --runtime-file "$TOOLS/runtime-x86_64" --comp zstd \
    "$APPDIR" "$OUT/rox-v$VERSION-linux-x86_64.AppImage"
