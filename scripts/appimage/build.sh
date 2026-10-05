#!/usr/bin/env bash
# Wraps the release binaries into a single-file AppImage. The release
# workflow's "Package (Linux AppImage)" step calls this after `cargo build
# --release`; run it by hand from the repo root to
# reproduce that locally. The host supplies glibc, the Vulkan loader, ALSA,
# fontconfig and everything else on the AppImage excludelist. The xkbcommon
# libraries rox links aren't on it and stock installs can lack the X11 one,
# so those get bundled, along with a software Vulkan driver for hosts that
# have none.
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

# Bundled libraries. ldd resolves them from the build container, which sets
# the same glibc floor as the binary. A RUNPATH only covers the object that
# carries it, so each bundled lib gets $ORIGIN too: that's how
# libxkbcommon-x11 finds the bundled libxcb-xkb.
mkdir -p "$APPDIR/usr/lib"
for lib in libxkbcommon.so.0 libxkbcommon-x11.so.0 libxcb-xkb.so.1; do
    path=$(ldd target/release/rox | awk -v lib="$lib" '$1 == lib { print $3 }')
    if [ ! -f "$path" ]; then
        echo "$lib didn't resolve against target/release/rox" >&2
        exit 1
    fi

    cp -L "$path" "$APPDIR/usr/lib/$lib"
    patchelf --set-rpath '$ORIGIN' "$APPDIR/usr/lib/$lib"
done
patchelf --set-rpath '$ORIGIN/../lib' "$APPDIR/usr/bin/rox"

# Mesa's software Vulkan driver, for hosts with no Vulkan driver at all: VMs
# without 3D, and CI boxes like the AppImage catalog's. gpui can't open a
# window without one, and AppRun only points the loader here when the host
# has no driver manifest. Its deps come along minus the excludelist ones, in
# a dir of their own so they never shadow what rox resolves from the host.
LVP_SRC=/usr/lib/x86_64-linux-gnu/libvulkan_lvp.so
LVP_MANIFEST=/usr/share/vulkan/icd.d/lvp_icd.x86_64.json
LVP_DIR="$APPDIR/usr/lib/lavapipe"
if [ ! -f "$LVP_SRC" ] || [ ! -f "$LVP_MANIFEST" ]; then
    echo "lavapipe is missing; install mesa-vulkan-drivers" >&2
    exit 1
fi

mkdir -p "$LVP_DIR"
for path in "$LVP_SRC" $(ldd "$LVP_SRC" | awk '$3 ~ /^\// { print $3 }'); do
    lib=$(basename "$path")
    case "$lib" in
        libc.so.* | libm.so.* | libpthread.so.* | libstdc++.so.* | libgcc_s.so.* | \
            libz.so.* | libdrm.so.* | libexpat.so.* | libwayland-client.so.* | \
            libxcb.so.* | libxcb-dri3.so.* | libX11-xcb.so.*)
            continue
            ;;
    esac

    cp -L "$path" "$LVP_DIR/$lib"
    patchelf --set-rpath '$ORIGIN' "$LVP_DIR/$lib"
done

# A relative library_path resolves against the manifest's own dir, which is
# the only way to name a path inside the mount point ahead of time.
sed 's|"library_path": *"[^"]*"|"library_path": "./libvulkan_lvp.so"|' \
    "$LVP_MANIFEST" > "$LVP_DIR/lvp_icd.json"
if ! grep -q '"./libvulkan_lvp.so"' "$LVP_DIR/lvp_icd.json"; then
    echo "couldn't rewrite library_path in $LVP_MANIFEST" >&2
    exit 1
fi

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

# Update information for AppImageUpdate and the launchers built on it. They
# resolve "latest" through GitHub's latest endpoint, which skips
# prereleases, so a candidate would be offered the previous stable as an
# update. Candidates take "latest-all" instead and follow the newest
# release of either kind.
case "$VERSION" in
    *-*) CHANNEL=latest-all ;;
    *) CHANNEL=latest ;;
esac
UPDATE_INFO="gh-releases-zsync|zealsprince|rox|$CHANNEL|rox-v*-linux-x86_64.AppImage.zsync"

# --appimage-extract-and-run because the build container has no FUSE. -u
# makes appimagetool run its bundled zsyncmake, which writes the .zsync into
# the working directory, so it runs from the output dir to keep the pair
# together. The .zsync points at the AppImage by bare filename, which
# resolves against the release's download folder.
NAME="rox-v$VERSION-linux-x86_64.AppImage"
(
    cd "$OUT"
    ARCH=x86_64 "$TOOLS/appimagetool-x86_64.AppImage" --appimage-extract-and-run \
        --runtime-file "$TOOLS/runtime-x86_64" --comp zstd \
        -u "$UPDATE_INFO" "$APPDIR" "$NAME"
)

if [ ! -f "$OUT/$NAME.zsync" ]; then
    echo "appimagetool didn't write $NAME.zsync" >&2
    exit 1
fi
