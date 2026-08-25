#!/usr/bin/env bash
# Packages a built (and already signed) rox.app into a styled drag-to-install
# DMG: dark background in the app palette, app icon on the left, amber arrow,
# /Applications on the right. Signing and notarization of the DMG itself stay
# with the caller.
# Usage: scripts/make-dmg.sh <path/to/rox.app> <output.dmg>
set -euo pipefail
cd "$(dirname "$0")/.."

APP=${1:?usage: make-dmg.sh <app-bundle> <output.dmg>}
OUT=${2:?usage: make-dmg.sh <app-bundle> <output.dmg>}
VOLUME="rox"
STAGING=""
WORK=""
MOUNT=""

cleanup() {
    if [[ -n $MOUNT ]]; then
        hdiutil detach "$MOUNT" -quiet 2>/dev/null \
            || hdiutil detach "$MOUNT" -force -quiet 2>/dev/null \
            || true
    fi
    [[ -n $STAGING ]] && rm -rf "$STAGING"
    [[ -n $WORK ]] && rm -rf "$WORK"
}
trap cleanup EXIT

if [[ ! -d $APP ]]; then
    echo "make-dmg: $APP not found" >&2
    exit 1
fi

WORK=$(mktemp -d)

# The /Applications symlink is what makes the mounted image a drag-to-install.
STAGING=$(mktemp -d)
cp -R "$APP" "$STAGING/"
ln -s /Applications "$STAGING/Applications"
mkdir "$STAGING/.background"
cp crates/rox/assets/app/dmg-background.png "$STAGING/.background/background.png"

# Handed just -srcfolder, hdiutil works out the image size itself and
# undershoots on the signed bundle: create dies with ENOSPC copying into
# /Volumes/rox while the machine still has plenty free. Give it the number
# instead, with slack for volume overhead. The UDZO convert at the end
# squeezes the unused blocks back out, so the shipped DMG doesn't grow.
size=$(( $(du -sm "$STAGING" | cut -f1) + 200 ))

# Clear any stale mount left by a previous attempt on the same machine, then
# create a writable image Finder can style. hdiutil can fail transiently on CI
# runners, so retry a few times, loudly.
hdiutil detach "/Volumes/$VOLUME" -force 2>/dev/null || true
RW="$WORK/rw.dmg"
created=0
for attempt in 1 2 3; do
    if hdiutil create -volname "$VOLUME" -srcfolder "$STAGING" -fs HFS+ \
        -size "${size}m" -format UDRW -ov "$RW"; then
        created=1
        break
    fi
    echo "make-dmg: hdiutil create failed (attempt $attempt of 3), retrying" >&2
    rm -f "$RW"
    sleep 3
done
if [[ $created -ne 1 ]]; then
    echo "make-dmg: could not create the disk image after 3 attempts" >&2
    exit 1
fi

attach_output=$(hdiutil attach "$RW" -nobrowse)
MOUNT=$(printf '%s\n' "$attach_output" \
    | awk '/\/Volumes\// {print substr($0, index($0, "/Volumes/")); exit}')
if [[ -z $MOUNT || ! -d $MOUNT ]]; then
    echo "make-dmg: could not find mounted volume in hdiutil output" >&2
    printf '%s\n' "$attach_output" >&2
    exit 1
fi

# Finder automation lays out the window (icon view, background, positions).
# Best-effort with a timeout: a headless hiccup never fails the release, the
# DMG is still valid that once, just unstyled.
osascript <<APPLESCRIPT &
tell application "Finder"
    tell disk "$VOLUME"
        open
        set current view of container window to icon view
        set toolbar visible of container window to false
        set statusbar visible of container window to false
        set the bounds of container window to {200, 120, 800, 520}
        set theOptions to the icon view options of container window
        set arrangement of theOptions to not arranged
        set icon size of theOptions to 128
        set text size of theOptions to 13
        set background picture of theOptions to file ".background:background.png"
        set position of item "$(basename "$APP")" of container window to {150, 200}
        set position of item "Applications" of container window to {450, 200}
        update without registering applications
        delay 1
        close
    end tell
end tell
APPLESCRIPT
style_pid=$!
style_status=0
style_timed_out=0
for _ in $(seq 25); do
    if ! kill -0 "$style_pid" 2>/dev/null; then
        wait "$style_pid" || style_status=$?
        break
    fi
    sleep 1
done
if kill -0 "$style_pid" 2>/dev/null; then
    style_timed_out=1
    kill "$style_pid" 2>/dev/null || true
    sleep 1
    kill -9 "$style_pid" 2>/dev/null || true
    wait "$style_pid" 2>/dev/null || true
fi
if (( style_timed_out )); then
    echo "make-dmg: window styling timed out, shipping a valid unstyled DMG" >&2
elif (( style_status != 0 )); then
    echo "make-dmg: window styling skipped" >&2
fi

sync
hdiutil detach "$MOUNT" -quiet \
    || hdiutil detach "$MOUNT" -force -quiet
MOUNT=""

rm -f "$OUT"
hdiutil convert "$RW" -format UDZO -imagekey zlib-level=9 -o "$OUT" -quiet
echo "make-dmg: $OUT"
