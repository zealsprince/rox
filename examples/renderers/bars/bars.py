"""A rox renderer in plain Python: one bar per signal, a stripe for the
playing position, and a dot under the pointer. Its manifest asks for the
palette, so the bars take the theme's accent once rox sends it, and the
background takes the stage's colour when rox names one.

rox starts this program for a render target, hands it a block of shared
memory as fd 3 and writes one JSON object per line to its stdin. The program
draws RGBA pixels into one of the block's two slots and prints `f <frame>
<slot>` on stdout when a frame is whole. The full contract is in
docs/03-implementation/11-renderers.md.

The fd handoff is Unix only. On Windows rox hands over a section handle
instead, which Python's mmap can't map, so this example exits there.
"""

import json
import mmap
import os
import select
import struct
import sys
import time

FPS = 30
HEADER = 4096  # the first slot starts here
PIXELS = 64  # a frame's pixels start this far into its slot
COLOURS = [(255, 179, 0), (0, 200, 170), (230, 70, 120), (110, 140, 255)]
BACKGROUND = bytes((18, 18, 24, 255))
TRACK = bytes((40, 40, 52, 255))


def fail(why):
    print(f"bars: {why}", file=sys.stderr)  # stderr goes to rox's log
    sys.exit(1)


if "ROX_FRAMES_FD" not in os.environ:
    fail("no ROX_FRAMES_FD: this example maps fd 3 and runs on Linux and macOS only")

width = int(os.environ["ROX_FRAMES_WIDTH"])
height = int(os.environ["ROX_FRAMES_HEIGHT"])

# rox made the block and sized it. Length 0 maps all of it, read and write.
mem = mmap.mmap(int(os.environ["ROX_FRAMES_FD"]), 0)

# The header says what rox made. Trust nothing that doesn't match.
magic, version, w, h, stride, slots, slot_bytes = struct.unpack_from("<4sIIIIIQ", mem, 0)
if (magic, version, w, h, stride, slots) != (b"RXF1", 1, width, height, width * 4, 2):
    fail("the mapping's header isn't the one this example knows")
if len(mem) != HEADER + 2 * slot_bytes:
    fail("the mapping isn't the size its header says")


def hex_colour(text):
    """`#rrggbb` as an (r, g, b) tuple, or None for anything else."""
    if not (isinstance(text, str) and len(text) == 7 and text[0] == "#"):
        return None
    try:
        return tuple(bytes.fromhex(text[1:]))
    except ValueError:
        return None


def draw(levels, position, pointer, pressed, accent, background):
    """One frame as bytes, top row first: four bytes a pixel, R G B A."""
    count = max(len(levels), 1)
    edges = [width * i // count for i in range(count + 1)]
    stripe = int(width * (position % 10.0) / 10.0)  # one sweep every 10 s played
    dot = height // 24

    rows = []
    for y in range(height):
        if y >= height - 6:
            rows.append(bytes((255, 255, 255, 255)) * stripe + background * (width - stripe))
            continue

        row = bytearray()
        for i in range(count):
            span = edges[i + 1] - edges[i]
            gap = max(span // 8, 1)
            lit = i < len(levels) and (height - y) <= levels[i] * (height - 12)
            hue = accent or COLOURS[i % len(COLOURS)]
            colour = bytes(hue + (255,)) if lit else TRACK
            row += background * gap + colour * (span - 2 * gap) + background * gap

        if pointer and abs(y - pointer[1] * height) < dot:
            x = int(pointer[0] * width)
            x0, x1 = max(x - dot, 0), min(x + dot, width)
            ink = (255, 255, 255, 255) if pressed else (255, 255, 255, 160)
            row[x0 * 4 : x1 * 4] = bytes(ink) * (x1 - x0)
        rows.append(bytes(row))

    return b"".join(rows)


def publish(n, pixels):
    """Writes frame `n` into slot n % 2 and announces it."""
    slot = n % 2
    base = HEADER + slot * slot_bytes

    # Odd first: rox throws away any copy it makes from here on.
    struct.pack_into("<Q", mem, base, 2 * n + 1)
    mem[base + PIXELS : base + PIXELS + len(pixels)] = pixels
    stamp = time.clock_gettime_ns(time.CLOCK_MONOTONIC)
    struct.pack_into("<QII", mem, base + 8, stamp, width, height)
    # Even last: the slot holds one whole frame again.
    struct.pack_into("<Q", mem, base, 2 * n + 2)

    sys.stdout.write(f"f {n} {slot}\n")
    sys.stdout.flush()


levels, position = [], 0.0
pointer, pressed, paused = None, False, False
accent = None  # the theme's, once a palette line comes
background = BACKGROUND  # the stage's, while a palette line names one
pending = b""
frame = 0
due = time.monotonic()

while True:
    # Wait for a line or the next frame, whichever comes first. Paused, only
    # a line can wake it. select says stdin has bytes, so os.read never blocks.
    wait = None if paused else max(due - time.monotonic(), 0.0)
    if select.select([0], [], [], wait)[0]:
        chunk = os.read(0, 65536)
        if not chunk:
            sys.exit(0)  # stdin closed: rox wants this renderer gone

        # A line can be longer than one read, so keep the tail for next time.
        pending += chunk
        *lines, pending = pending.split(b"\n")
        for line in lines:
            message = json.loads(line)
            if "pause" in message:
                paused = message["pause"]
            elif "pool" in message:  # the signals by name, before any value arrives
                levels = [0.0] * len(message["pool"])
            elif "signals" in message:
                levels, position = message["signals"], message["time"]
            elif "palette" in message:
                accent = hex_colour(message["palette"].get("accent"))
                stage = hex_colour(message["palette"].get("stage"))
                background = bytes(stage + (255,)) if stage else BACKGROUND
            elif "pointer" in message:
                event = message["pointer"]
                if event["kind"] == "leave":  # off the surface, and any press ends
                    pointer, pressed = None, False
                else:
                    pointer = (event["x"], event["y"])
                    pressed = {"down": True, "up": False}.get(event["kind"], pressed)
            # hello needs nothing here, and a line this doesn't know is skipped

    now = time.monotonic()
    if paused or now < due:
        continue

    publish(frame, draw(levels, position, pointer, pressed, accent, background))
    frame += 1
    due = max(due + 1 / FPS, now)  # fall behind, and it doesn't race to catch up
