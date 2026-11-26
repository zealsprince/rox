#!/usr/bin/env python3
"""A stand-in for rox, for running a renderer outside it while you write one.

This is a development tool. It grants the renderer everything its flags ask
for, the cover and the spectrum included, with no enable card and no
approval, and it runs whatever command it's given as you. Run it only on a
renderer you trust.

It plays rox's part as docs/03-implementation/11-renderers.md specifies it:
it makes the frame mapping with its header, and with --cover and --spectrum
the cover mapping (a generated test picture) and the spectrum mapping (a
synthetic spectrum). It starts the renderer with those at fd 3 and up and the
environment rox gives one, writes `hello`, a pool line, signals lines about 60
a second from a scripted drive (a rest, a bass swell, then beats), palette and
cover lines, a pause and a resume, pointer events, and the music stopping,
then closes stdin. It reads the renderer's announces and copies slots the way
rox does, and reports what rox would have made of them.

Linux and macOS, Python 3.8 or later, the standard library alone.

    standin.py [options] <renderer folder>
    standin.py [options] -- <command> [args...]

A folder is started from its plugin.json the way rox starts it. After `--`
the command runs as given, in the current folder. `--help` lists the options.
"""

import argparse
import fcntl
import hashlib
import json
import math
import mmap
import os
import platform
import queue
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import time

# ── The layout ─────────────────────────────────────────────────────────────
#
# Every number below is one of rox-frames' constants, named after it. A test
# in crates/rox-frames/tests/standin.rs reads these lines and fails when one
# drifts from the crate, so keep them one assignment a line.

# crates/rox-frames/src/layout.rs
FRAME_MAGIC = b"RXF1"  # MAGIC
FRAME_VERSION = 1  # VERSION
HEADER_BYTES = 4096  # HEADER_BYTES
SLOTS = 2  # SLOTS
PAGE = 4096  # PAGE
PIXEL = 4  # PIXEL
WIDTH_MIN = 16  # WIDTH.start()
WIDTH_MAX = 3840  # WIDTH.end()
HEIGHT_MIN = 16  # HEIGHT.start()
HEIGHT_MAX = 2160  # HEIGHT.end()
H_MAGIC = 0  # header::MAGIC
H_VERSION = 4  # header::VERSION
H_WIDTH = 8  # header::WIDTH
H_HEIGHT = 12  # header::HEIGHT
H_STRIDE = 16  # header::STRIDE
H_SLOTS = 20  # header::SLOTS
H_SLOT_BYTES = 24  # header::SLOT_BYTES
S_SEQ = 0  # slot::SEQ
S_STAMP_NS = 8  # slot::STAMP_NS
S_WIDTH = 16  # slot::WIDTH
S_HEIGHT = 20  # slot::HEIGHT
S_PIXELS = 64  # slot::PIXELS

# crates/rox-frames/src/lib.rs
ANNOUNCE_MAX = 64  # ANNOUNCE_MAX

# crates/rox-frames/src/cover.rs
COVER_MAGIC = b"RXC1"  # cover::MAGIC
COVER_VERSION = 1  # cover::VERSION
COVER_SIZE = 256  # cover::SIZE
COVER_BYTES = 266240  # cover::BYTES
COVER_WIDTH = 8  # cover::header::WIDTH
COVER_HEIGHT = 12  # cover::header::HEIGHT
COVER_SEQ = 16  # cover::header::SEQ
COVER_PIXELS = 64  # cover::header::PIXELS

# crates/rox-frames/src/spectrum.rs
SPECTRUM_MAGIC = b"RXS1"  # spectrum::MAGIC
SPECTRUM_VERSION = 1  # spectrum::VERSION
SPECTRUM_BYTES = 4096  # spectrum::BYTES
LOG_BINS = 128  # spectrum::LOG_BINS
LOG_LOW_HZ = 20.0  # spectrum::LOG_LOW_HZ
LOG_HIGH_HZ = 20000.0  # spectrum::LOG_HIGH_HZ
LINEAR_BINS = 512  # spectrum::LINEAR_BINS
LINEAR_BIN_HZ = 21.533203125  # spectrum::LINEAR_BIN_HZ
FLOOR_DB = -66.0  # spectrum::FLOOR_DB
MAX_DB = -12.0  # spectrum::MAX_DB
SPECTRUM_RATE = 62.5  # spectrum::RATE
PLAYING = 1  # spectrum::PLAYING
SP_SEQ = 8  # spectrum::header::SEQ
SP_STAMP_NS = 16  # spectrum::header::STAMP_NS
SP_FLAGS = 24  # spectrum::header::FLAGS
SP_SAMPLE_RATE = 28  # spectrum::header::SAMPLE_RATE
SP_RATE = 32  # spectrum::header::RATE
SP_LOG_BINS = 36  # spectrum::header::LOG_BINS
SP_LOG_LOW_HZ = 40  # spectrum::header::LOG_LOW_HZ
SP_LOG_HIGH_HZ = 44  # spectrum::header::LOG_HIGH_HZ
SP_LINEAR_BINS = 48  # spectrum::header::LINEAR_BINS
SP_LINEAR_BIN_HZ = 52  # spectrum::header::LINEAR_BIN_HZ
SP_FLOOR_DB = 56  # spectrum::header::FLOOR_DB
SP_MAX_DB = 60  # spectrum::header::MAX_DB
SP_LOG_AT = 64  # spectrum::header::LOG_AT
SP_LINEAR_AT = 68  # spectrum::header::LINEAR_AT
SP_LOG_BYTES_AT = 72  # spectrum::header::LOG_BYTES_AT
SP_LINEAR_BYTES_AT = 76  # spectrum::header::LINEAR_BYTES_AT
ROW_LOG = 128  # spectrum::rows::LOG
ROW_LINEAR = 640  # spectrum::rows::LINEAR
ROW_LOG_BYTES = 2688  # spectrum::rows::LOG_BYTES
ROW_LINEAR_BYTES = 2816  # spectrum::rows::LINEAR_BYTES

# ── rox's side of the session ──────────────────────────────────────────────
#
# These come from the renderer host and the render target, and tests in
# crates/rox-services/src/renderers.rs and crates/rox/src/canvas/target.rs
# compare them against this file.

# crates/rox-services/src/renderers.rs
API = 1  # API: `ROX_RENDER_API` and hello's number
FIRST_FD = 3  # FIRST_FD: the frame mapping's fd, with the cover's and the spectrum's after it
GRACE_MS = 200  # GRACE: after stdin closes, then the kill

# crates/rox/src/canvas/target.rs
FLOOD_LINES = 500  # LIMITS.flood_lines in one span
FLOOD_SPAN_MS = 1000  # LIMITS.flood_span
FLOOD_SPANS = 2  # LIMITS.flood_spans in a row stop it
REFUSED_RUN = 60  # LIMITS.refused_run refused frames in a row stop it
MOVE_GAP_MS = 8  # MOVE_GAP: moves go at most this often

STDERR_LINE = 4096  # a stderr line is cut here (STDERR_LINE, rox-plugins/src/process.rs)

# The names a renderer inherits from rox when they're set, ALLOWED in
# crates/rox-plugins/src/env.rs, which a test there compares with this list.
# Every LC_* passes too, and the names ROX_PLUGIN_ENV lists.
ALLOWED = [
    "HOME", "USER", "LOGNAME", "LANG", "LANGUAGE", "LOCALE_ARCHIVE", "TZ",
    "TMPDIR", "TEMP", "TMP", "XDG_RUNTIME_DIR", "XDG_DATA_HOME",
    "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_DIRS", "XDG_CONFIG_DIRS",
    "WAYLAND_DISPLAY", "DISPLAY", "XAUTHORITY", "XDG_SESSION_TYPE",
    "XDG_CURRENT_DESKTOP", "DBUS_SESSION_BUS_ADDRESS", "LD_LIBRARY_PATH",
    "VK_ICD_FILENAMES", "VK_DRIVER_FILES", "VK_ADD_DRIVER_FILES",
    "VK_LAYER_PATH", "VK_ADD_LAYER_PATH", "__EGL_VENDOR_LIBRARY_FILENAMES",
    "__EGL_VENDOR_LIBRARY_DIRS", "LIBGL_DRIVERS_PATH", "DRI_PRIME",
    "MESA_VK_DEVICE_SELECT", "__NV_PRIME_RENDER_OFFLOAD",
    "__VK_LAYER_NV_optimus", "HTTP_PROXY", "HTTPS_PROXY", "NO_PROXY",
    "ALL_PROXY", "http_proxy", "https_proxy", "no_proxy", "all_proxy",
    "SSL_CERT_FILE", "SSL_CERT_DIR", "NIX_SSL_CERT_FILE", "SystemRoot",
    "SystemDrive", "windir", "ComSpec", "PATHEXT", "USERPROFILE", "USERNAME",
    "HOMEDRIVE", "HOMEPATH", "APPDATA", "LOCALAPPDATA", "ProgramData",
    "ProgramFiles", "ProgramFiles(x86)", "ProgramW6432", "CommonProgramFiles",
    "NUMBER_OF_PROCESSORS", "PROCESSOR_ARCHITECTURE", "OS",
    "__CF_USER_TEXT_ENCODING",
]

# Linux's seals, by number where this Python's fcntl doesn't name them.
F_ADD_SEALS = getattr(fcntl, "F_ADD_SEALS", 1033)
F_SEAL_SEAL = getattr(fcntl, "F_SEAL_SEAL", 0x1)
F_SEAL_SHRINK = getattr(fcntl, "F_SEAL_SHRINK", 0x2)
F_SEAL_GROW = getattr(fcntl, "F_SEAL_GROW", 0x4)
F_SEAL_FUTURE_WRITE = getattr(fcntl, "F_SEAL_FUTURE_WRITE", 0x10)

SIGNALS_RATE = 60  # lines a second: one a paint, at a 60 Hz display
DISPLAY_RATE = 60  # how often a newest announce is taken and copied

# The pool every run sends: one signal of each kind, worded the way rox
# words a pool line (`pool_line` and `wire_name` in target.rs).
POOL = [
    {"id": 1, "name": "Kick", "kind": "band", "low": 30, "high": 120},
    {"id": 2, "name": "onset 30-120 Hz", "kind": "onset", "low": 30, "high": 120},
    {"id": 3, "name": "beat 30-120 Hz", "kind": "beat", "low": 30, "high": 120},
    {"id": 4, "name": "trigger 2000-8000 Hz", "kind": "trigger", "low": 2000, "high": 8000},
    {"id": 5, "name": "level", "kind": "level"},
    {"id": 6, "name": "total of 1", "kind": "aggregate"},
]

PALETTE = {
    "background": "#121212", "surface": "#181818", "text": "#c0c0c0",
    "muted": "#808080", "accent": "#ffb300", "highlight": "#facc15",
    "dark": True, "stage": "#121212",
}


def page_up(n):
    return (n + PAGE - 1) // PAGE * PAGE


def now_ns():
    """The clock a slot's `stamp_ns` is read against on Unix (`clock_ns`)."""
    return time.clock_gettime_ns(time.CLOCK_MONOTONIC)


# ── Shared memory ──────────────────────────────────────────────────────────


class Block:
    """One block of shared memory: an fd to lend and this end's view of it.

    On Linux it's a memfd, sealed the way rox seals it (`seal` and
    `seal_cover` in rox-frames/src/map/unix.rs). macOS has no memfd, so it's
    an unlinked file, and a block the renderer only reads is lent as a second
    read-only open of it, as rox's macOS design does with its shm object.
    """

    def __init__(self, name, size, readonly_for_renderer):
        self.size = size
        path = None
        if hasattr(os, "memfd_create"):
            flags = os.MFD_CLOEXEC | getattr(os, "MFD_ALLOW_SEALING", 2)
            self.fd = os.memfd_create(name, flags)
        else:
            self.fd, path = tempfile.mkstemp(prefix=f"{name}-")
        os.ftruncate(self.fd, size)
        self.view = mmap.mmap(self.fd, size)

        self.lent = self.fd
        if path:
            if readonly_for_renderer:
                self.lent = os.open(path, os.O_RDONLY | os.O_CLOEXEC)
            os.unlink(path)
        else:
            self.seal(readonly_for_renderer)

    def seal(self, readonly):
        seals = F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_SEAL
        if readonly:
            try:
                fcntl.fcntl(self.fd, F_ADD_SEALS, seals | F_SEAL_FUTURE_WRITE)
                return
            except OSError:
                pass  # before Linux 5.1: the renderer could write, as with rox
        fcntl.fcntl(self.fd, F_ADD_SEALS, seals)

    def u32(self, at, value=None):
        if value is None:
            return struct.unpack_from("<I", self.view, at)[0]
        struct.pack_into("<I", self.view, at, value)

    def u64(self, at, value=None):
        if value is None:
            return struct.unpack_from("<Q", self.view, at)[0]
        struct.pack_into("<Q", self.view, at, value)

    def f32(self, at, value):
        struct.pack_into("<f", self.view, at, value)


class Frames:
    """The frame mapping (`Spec` and `write_header` in layout.rs)."""

    def __init__(self, width, height):
        self.width, self.height = width, height
        self.stride = width * PIXEL
        self.pixel_bytes = self.stride * height
        self.slot_bytes = page_up(S_PIXELS + self.pixel_bytes)
        self.block = Block("rox-frames", HEADER_BYTES + SLOTS * self.slot_bytes, False)

        view = self.block.view
        view[H_MAGIC:H_MAGIC + 4] = FRAME_MAGIC
        self.block.u32(H_VERSION, FRAME_VERSION)
        self.block.u32(H_WIDTH, width)
        self.block.u32(H_HEIGHT, height)
        self.block.u32(H_STRIDE, self.stride)
        self.block.u32(H_SLOTS, SLOTS)
        self.block.u64(H_SLOT_BYTES, self.slot_bytes)

    def copy(self, slot):
        """rox's copy of a slot (`copy_slot` in read.rs): ("whole", seq,
        pixels), or the reason it isn't one. Every length is this end's own;
        the header the renderer may have rewritten is never read back."""
        at = HEADER_BYTES + slot * self.slot_bytes
        before = self.block.u64(at + S_SEQ)
        if before == 0:
            return ("unwritten",)
        if before & 1:
            return ("torn",)

        size = (self.block.u32(at + S_WIDTH), self.block.u32(at + S_HEIGHT))
        if size != (self.width, self.height):
            return ("wrong size", size)

        start = at + S_PIXELS
        pixels = bytes(self.block.view[start:start + self.pixel_bytes])
        if self.block.u64(at + S_SEQ) != before:
            return ("torn",)
        return ("whole", before, pixels)


class CoverBlock:
    """The cover mapping, which this end writes and the renderer only reads
    (`Cover::write` in cover.rs)."""

    def __init__(self):
        self.block = Block("rox-cover", COVER_BYTES, True)
        self.block.view[0:4] = COVER_MAGIC
        self.block.u32(4, COVER_VERSION)
        self.block.u32(COVER_WIDTH, COVER_SIZE)
        self.block.u32(COVER_HEIGHT, COVER_SIZE)
        self.seq = 0

    def write(self, pixels):
        """Odd counter, the pixels, the even counter. Answers the even one.
        None is the transparent square rox writes with no cover."""
        pixels = pixels or bytes(COVER_SIZE * COVER_SIZE * 4)
        self.block.u64(COVER_SEQ, self.seq + 1)
        self.block.view[COVER_PIXELS:COVER_PIXELS + len(pixels)] = pixels
        self.seq += 2
        self.block.u64(COVER_SEQ, self.seq)
        return self.seq


def test_cover():
    """A 2:1 picture letterboxed into the square the way rox fits one
    (`fit` and `letterbox`): transparent bands, then a gradient with a
    white frame, rows 64 to 191."""
    side = COVER_SIZE
    top, bottom = side // 4, side - side // 4
    rows = []
    for y in range(side):
        if y < top or y >= bottom:
            rows.append(bytes(4 * side))
            continue
        row = bytearray()
        for x in range(side):
            edge = x in (0, side - 1) or y in (top, bottom - 1)
            if edge:
                row += b"\xff\xff\xff\xff"
            else:
                row += bytes((x, (y - top) * 2 % 256, 255 - x, 255))
        rows.append(bytes(row))
    return b"".join(rows)


class SpectrumBlock:
    """The spectrum mapping, which this end writes and every granted
    renderer reads (`Spectrum::write` in spectrum.rs, `Writer::step` in
    rox/src/canvas/spectrum.rs). Like rox it writes zeros once when it's
    made, then rows while the music plays, then zeros once when it stops,
    and leaves the counter alone between."""

    def __init__(self):
        self.block = Block("rox-spectrum", SPECTRUM_BYTES, True)

        # Everything from the stamp to the rows' end, built here and copied
        # in with one slice assignment between the two counter stores.
        self.page = bytearray(ROW_LINEAR_BYTES + LINEAR_BINS - SP_STAMP_NS)
        put = lambda fmt, at, *values: struct.pack_into(fmt, self.page, at - SP_STAMP_NS, *values)
        put("<f", SP_RATE, SPECTRUM_RATE)
        put("<I", SP_LOG_BINS, LOG_BINS)
        put("<f", SP_LOG_LOW_HZ, LOG_LOW_HZ)
        put("<f", SP_LOG_HIGH_HZ, LOG_HIGH_HZ)
        put("<I", SP_LINEAR_BINS, LINEAR_BINS)
        put("<f", SP_LINEAR_BIN_HZ, LINEAR_BIN_HZ)
        put("<f", SP_FLOOR_DB, FLOOR_DB)
        put("<f", SP_MAX_DB, MAX_DB)
        put("<I", SP_LOG_AT, ROW_LOG)
        put("<I", SP_LINEAR_AT, ROW_LINEAR)
        put("<I", SP_LOG_BYTES_AT, ROW_LOG_BYTES)
        put("<I", SP_LINEAR_BYTES_AT, ROW_LINEAR_BYTES)
        self.put = put

        self.block.view[0:4] = SPECTRUM_MAGIC
        self.block.u32(4, SPECTRUM_VERSION)
        self.seq = 0
        self.write(None, None, 0)

    def write(self, log_row, linear_row, sample_rate):
        """Rows while music plays, or None for zeros with the playing bit
        clear."""
        playing = log_row is not None
        if not playing:
            log_row, linear_row, sample_rate = [0.0] * LOG_BINS, [0.0] * LINEAR_BINS, 0

        put = self.put
        put(f"<{LOG_BINS}f", ROW_LOG, *log_row)
        put(f"<{LINEAR_BINS}f", ROW_LINEAR, *linear_row)
        start = ROW_LOG_BYTES - SP_STAMP_NS
        self.page[start:start + LOG_BINS] = bytes(round(v * 255) for v in log_row)
        start = ROW_LINEAR_BYTES - SP_STAMP_NS
        self.page[start:start + LINEAR_BINS] = bytes(round(v * 255) for v in linear_row)
        put("<I", SP_FLAGS, PLAYING if playing else 0)
        put("<I", SP_SAMPLE_RATE, sample_rate)
        put("<Q", SP_STAMP_NS, now_ns())

        # Python has no fence. Three stores in program order, which x86-64
        # keeps, are as near atomic as it gets.
        view = self.block.view
        struct.pack_into("<Q", view, SP_SEQ, self.seq + 1)
        view[SP_STAMP_NS:SP_STAMP_NS + len(self.page)] = self.page
        struct.pack_into("<Q", view, SP_SEQ, self.seq + 2)

        self.seq += 2
        self.playing = playing

    def quiet(self):
        """Zeros once, as rox writes them when the music stops."""
        if self.playing:
            self.write(None, None, 0)


# ── The drive ──────────────────────────────────────────────────────────────


class Drive:
    """What the music does over the run, as fractions of it: a rest, a bass
    swell, beats at 120 a minute with hi-hats between them, then a stop. The
    values are made up, shaped like the hub's: a band and the level ease, a
    pulse jumps to 1 and decays, the total wraps at 1."""

    def __init__(self, seconds):
        self.seconds = seconds
        self.start = 0.12 * seconds
        self.beats = 0.45 * seconds
        self.stop = 0.9 * seconds
        self.values = [0.0] * len(POOL)
        self.position = 0.0
        self.last_beat = -1
        self.last_hat = -1

    def playing(self, t):
        return self.start <= t < self.stop

    def step(self, t, dt):
        """The pool's values at `t`, in pool order."""
        band, onset, beat, trigger, level, total = self.values
        decay = math.exp(-16 * dt)

        if self.playing(t):
            self.position += dt
            if t < self.beats:
                swell = (t - self.start) / (self.beats - self.start)
                band = math.sin(math.pi * swell) ** 2
            else:
                beat_at = (t - self.beats) * 2
                n = int(beat_at)
                band = 0.9 * math.exp(-(beat_at - n) / 0.25)
                hit = n != self.last_beat
                hat = int(beat_at + 0.5) != self.last_hat
                self.last_beat, self.last_hat = n, int(beat_at + 0.5)
                onset = 1.0 if hit else onset * decay
                beat = 1.0 if hit else beat * decay
                trigger = 1.0 if hat and not hit else trigger * decay
            level = 0.4 + 0.5 * band
        else:
            # Stopped: everything falls to rest at the bands' own pace.
            band *= math.exp(-6 * dt)
            level *= math.exp(-6 * dt)
            onset, beat, trigger = onset * decay, beat * decay, trigger * decay

        total = (total + band * dt) % 1.0
        self.values = [band, onset, beat, trigger, level, total]
        return [round(v, 4) for v in self.values]

    def spectrum(self):
        """Rows shaped like music: a slope falling toward the top, the bass
        bins following the band, the highs following the trigger."""
        band, _, _, trigger, level, _ = self.values
        log_row = []
        for i in range(LOG_BINS):
            slope = 0.55 - 0.25 * i / LOG_BINS
            bass = band * max(0.0, 1.0 - abs(i - 26) / 12)
            highs = trigger * max(0.0, 1.0 - abs(i - 105) / 15)
            log_row.append(min(1.0, slope * level + 0.45 * bass + 0.4 * highs))
        linear_row = []
        for j in range(LINEAR_BINS):
            hz = j * LINEAR_BIN_HZ
            at = 0 if hz < LOG_LOW_HZ else min(LOG_BINS - 1, int(LOG_BINS * math.log(hz / LOG_LOW_HZ) / math.log(1000)))
            linear_row.append(log_row[at] if j else 0.0)
        return log_row, linear_row


def pointer_events(seconds):
    """(time, event) pairs: moves across the surface, a press and release, a
    scroll, then a leave."""
    events = []
    t0, t1 = 0.2 * seconds, 0.4 * seconds
    t = t0
    while t < t1:
        f = (t - t0) / (t1 - t0)
        events.append((t, {"kind": "move", "x": round(f, 4), "y": round(0.5 + 0.3 * math.sin(6 * f), 4)}))
        t += MOVE_GAP_MS / 1000 * 2
    middle = (t0 + t1) / 2
    events.append((middle, {"kind": "down", "x": 0.5, "y": 0.5, "button": "left"}))
    events.append((middle + 0.1, {"kind": "up", "x": 0.5, "y": 0.5, "button": "left"}))
    events.append((middle + 0.2, {"kind": "scroll", "x": 0.5, "y": 0.5, "dx": 0.0, "dy": -1.0}))
    events.append((t1, {"kind": "leave"}))
    return sorted(events, key=lambda e: e[0])


# ── Starting it ────────────────────────────────────────────────────────────


def platform_key():
    """`<os>-<arch>` as rox's `platform` names it."""
    system = {"linux": "linux", "darwin": "macos"}.get(sys.platform, sys.platform)
    machine = platform.machine().lower()
    arch = {"amd64": "x86_64", "arm64": "aarch64"}.get(machine, machine)
    return f"{system}-{arch}"


def on_path(name, path):
    """The first folder on `path` holding a file `name`. Like rox, this
    checks the file exists, not that it can run (`on_path`)."""
    for folder in path.split(os.pathsep):
        candidate = os.path.join(folder, name)
        if folder and os.path.isfile(candidate):
            return candidate
    return None


def speaks_python3(program):
    """rox's probe (`speaks_python3`): a clean exit and "Python 3" first on
    stdout or stderr, within 5 seconds."""
    try:
        done = subprocess.run([program, "--version"], capture_output=True, timeout=5, text=True)
    except (OSError, subprocess.TimeoutExpired):
        return False
    says = lambda text: text.lstrip().startswith("Python 3")
    return done.returncode == 0 and (says(done.stdout) or says(done.stderr))


def entry(folder, path):
    """The command rox would start for the renderer in `folder`
    (`entry_for` in rox-plugins/src/manifest.rs)."""
    with open(os.path.join(folder, "plugin.json"), encoding="utf-8") as f:
        manifest = json.load(f)

    if "script" in manifest["entry"]:
        script = manifest["entry"]["script"]
        names = ["python3", "python"] if script["interpreter"] == "python3" else [script["interpreter"]]
        for name in names:
            found = on_path(name, path)
            if found and (script["interpreter"] != "python3" or speaks_python3(found)):
                return [found, os.path.join(folder, script["path"])], manifest
        sys.exit(f"standin: interpreter {script['interpreter']} not found")

    builds = manifest["entry"]["native"]
    key = platform_key()
    if key not in builds:
        sys.exit(f"standin: no build for {key}")
    return [os.path.join(folder, builds[key])], manifest


def environment(folder, program_folders, extra):
    """The environment rox builds from an empty one (`prepare` in
    rox-plugins/src/env.rs), then the renderer's own variables."""
    passes = set(ALLOWED) | {n.strip() for n in os.environ.get("ROX_PLUGIN_ENV", "").split(",") if n.strip()}
    env = {k: v for k, v in os.environ.items() if k in passes or k.startswith("LC_")}

    search = ([os.path.join(folder, "bin")] if folder else []) + program_folders
    search.append(os.environ.get("PATH", ""))
    env["PATH"] = os.pathsep.join(p for p in search if p)
    env.update(PYTHONDONTWRITEBYTECODE="1", PYTHONUNBUFFERED="1", PYTHONUTF8="1")
    env.update(extra)
    return env


def spawn(command, cwd, env, lent):
    """Starts `command` leading its own process group with `lent` at fd 3
    and up, in order. Each is moved past fd 20 first and placed by the shell
    that execs the renderer, so no fd this process holds can land on one."""
    high = [fcntl.fcntl(fd, getattr(fcntl, "F_DUPFD_CLOEXEC", fcntl.F_DUPFD), 20) for fd in lent]
    moves = " ".join(f"{FIRST_FD + i}<&{fd} {fd}<&-" for i, fd in enumerate(high))
    process = subprocess.Popen(
        ["/bin/sh", "-c", f'exec "$@" {moves}', "sh", *command],
        cwd=cwd,
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        pass_fds=high,
        start_new_session=True,
    )
    for fd in high:
        os.close(fd)
    return process


# ── The session ────────────────────────────────────────────────────────────


class Report:
    def __init__(self):
        self.copied = 0
        self.torn = 0
        self.unwritten = 0
        self.wrong_size = 0
        self.texture = 0
        self.old = 0
        self.lines = 0
        self.announces = 0
        self.other_lines = 0
        self.refused_run = 0
        self.longest_refused_run = 0
        self.last_seq = 0
        self.protocol = []
        self.hashes = set()
        self.waiting_at_resume = 0
        self.flooded = False
        self.exit = None
        self.exited_in_grace = None

    def refused(self):
        return self.unwritten + self.wrong_size + self.texture

    def as_dict(self, seconds, seconds_running):
        """`seconds_running` leaves the pause out: nothing is copied in it."""
        return {
            "frames": self.copied,
            "distinct": len(self.hashes),
            "torn": self.torn,
            "refused": self.refused(),
            "unwritten": self.unwritten,
            "wrong_size": self.wrong_size,
            "texture_without_texture": self.texture,
            "old": self.old,
            "lines": self.lines,
            "announces": self.announces,
            "other_lines": self.other_lines,
            "announces_per_second": round(self.announces / seconds, 1) if seconds else 0,
            "fps": round(self.copied / seconds_running, 1) if seconds_running else 0,
            "longest_refused_run": self.longest_refused_run,
            "flooded": self.flooded,
            "protocol": self.protocol[:20],
            "bytes_waiting_at_resume": self.waiting_at_resume,
            "exit": self.exit,
            "exited_within_grace": self.exited_in_grace,
        }


def parse_announce(line):
    """`f <n> <slot>` or `t <n>` as rox reads them (`parse_announce` and
    `parse_texture_announce` in lib.rs): at most ANNOUNCE_MAX bytes with the
    newline, digits and nothing else."""
    if len(line) > ANNOUNCE_MAX:
        return None
    body = line[:-1] if line.endswith(b"\n") else line
    parts = body.split(b" ")
    digits = lambda b: b != b"" and all(48 <= c <= 57 for c in b)
    if len(parts) == 3 and parts[0] == b"f" and digits(parts[1]) and parts[2] in (b"0", b"1"):
        return ("f", int(parts[1]), int(parts[2]))
    if len(parts) == 2 and parts[0] == b"t" and digits(parts[1]):
        return ("t", int(parts[1]), None)
    return None


def run(args):
    width, height = args.size
    frames = Frames(width, height)
    cover = CoverBlock() if args.cover else None
    spectrum = SpectrumBlock() if args.spectrum else None

    # The handles in rox's order (`places` in rox-services/src/renderers.rs).
    lent = [frames.block.lent]
    extra = {
        "ROX_RENDER_API": str(API),
        "ROX_FRAMES_WIDTH": str(width),
        "ROX_FRAMES_HEIGHT": str(height),
        "ROX_FRAMES_FD": str(FIRST_FD),
    }
    hello = {"hello": API, "width": width, "height": height, "handle": 0}
    if cover:
        fd = FIRST_FD + len(lent)
        lent.append(cover.block.lent)
        extra["ROX_COVER_FD"] = str(fd)
        hello["cover"] = {"fd": fd, "width": COVER_SIZE, "height": COVER_SIZE}
    if spectrum:
        fd = FIRST_FD + len(lent)
        lent.append(spectrum.block.lent)
        extra["ROX_SPECTRUM_FD"] = str(fd)
        hello["spectrum"] = {
            "fd": fd, "bytes": SPECTRUM_BYTES, "log_bins": LOG_BINS,
            "linear_bins": LINEAR_BINS, "rate": SPECTRUM_RATE,
        }

    folder = os.path.abspath(args.target[0]) if not args.command else None
    env = environment(folder, args.program_folders, extra)
    if folder:
        command, _ = entry(folder, env["PATH"])
    else:
        command = args.target
    process = spawn(command, folder or os.getcwd(), env, lent)

    report = Report()
    announced = queue.Queue()
    reading = threading.Event()
    reading.set()
    flood = {"since": None, "lines": 0, "over": 0}

    def note_line(at):
        # The flood rule, counted the way `Rate::note` counts it.
        span = FLOOD_SPAN_MS / 1000
        since = flood["since"] if flood["since"] is not None else at
        spans = int((at - since) / span)
        if spans > 0:
            over = flood["lines"] > FLOOD_LINES
            flood["over"] = flood["over"] + 1 if over and spans == 1 else 0
            flood["lines"] = 0
            since += span * spans
        flood["since"] = since
        flood["lines"] += 1
        if flood["over"] + (flood["lines"] > FLOOD_LINES) >= FLOOD_SPANS:
            report.flooded = True

    def read_stdout():
        # While paused rox reads nothing, so a renderer that keeps announcing
        # fills its own pipe.
        while True:
            reading.wait()
            line = process.stdout.readline()
            if not line:
                return
            report.lines += 1
            note_line(time.monotonic())
            parsed = parse_announce(line)
            if parsed:
                report.announces += 1
                announced.put(parsed)
            else:
                report.other_lines += 1

    def read_stderr():
        for line in process.stderr:
            text = line[:STDERR_LINE].decode("utf-8", "replace").rstrip("\n")
            print(f"renderer: {text}", file=sys.stderr)

    threading.Thread(target=read_stdout, daemon=True).start()
    threading.Thread(target=read_stderr, daemon=True).start()

    stdin = process.stdin.fileno()
    os.set_blocking(stdin, False)
    # Whole lines with their messages. The first may be partly written.
    pending = []
    partly = [0]

    def send(message):
        pending.append((message, json.dumps(message, separators=(",", ":")).encode() + b"\n"))

    def flush():
        # rox never waits on a renderer's stdin: what doesn't fit waits.
        while pending:
            try:
                n = os.write(stdin, pending[0][1][partly[0]:])
            except BlockingIOError:
                return
            except BrokenPipeError:
                pending.clear()
                partly[0] = 0
                return
            partly[0] += n
            if partly[0] == len(pending[0][1]):
                pending.pop(0)
                partly[0] = 0

    def stale_for_pause():
        # What rox drops at a pause (`Outbox::pause` in target.rs): waiting
        # pointer events and signals, but for a line saying playback stopped,
        # and never a line it began writing.
        newest = next((m for m, _ in reversed(pending) if "signals" in m), None)
        keep = pending[:1] if partly[0] else []
        for message, line in pending[len(keep):]:
            stale = "pointer" in message or ("signals" in message and (message is not newest or message["playing"]))
            if not stale:
                keep.append((message, line))
        pending[:] = keep

    drive = Drive(args.seconds)
    pointer = pointer_events(args.seconds)
    pause_at, resume_at = 0.6 * args.seconds, 0.6 * args.seconds + 0.5
    paused = False
    settle_until = drive.stop + min(1.0, 0.1 * args.seconds)
    dumped = set()

    # The first paint's lines in rox's order: the cover, transparent with
    # nothing playing yet, goes ahead of the pool (`drive` in target.rs).
    send(hello)
    if cover:
        cover.write(None)
        send({"cover": None})
    send({"pool": [] if args.empty_pool else POOL})
    if args.palette:
        send({"palette": PALETTE})
    flush()

    started = time.monotonic()
    next_signals = next_frame = next_spectrum = started
    covered = False
    last_step = started
    running_time = 0.0

    while True:
        now = time.monotonic()
        t = now - started
        if t >= args.seconds or process.poll() is not None:
            break

        # A pause, as when nothing shows the target: told, then nothing goes
        # and nothing is read until the resume.
        if not paused and pause_at <= t < resume_at:
            paused = True
            stale_for_pause()
            send({"pause": True})
            flush()
            time.sleep(0.01)
            reading.clear()
        elif paused and t >= resume_at:
            paused = False
            waiting = struct.unpack("i", fcntl.ioctl(process.stdout.fileno(), termios.FIONREAD, b"\0" * 4))[0]
            report.waiting_at_resume = waiting
            reading.set()
            send({"pause": False})

        playing = drive.playing(t)
        if not paused:
            running_time += now - last_step
        dt, last_step = now - last_step, now
        values = drive.step(t, dt)

        # The cover follows the track, and waits out a pause as rox's does.
        if cover and playing != covered and not paused:
            covered = playing
            square = cover.write(test_cover() if playing else None)
            send({"cover": square if playing else None})

        # A target's pause doesn't stop it: rox writes for every session.
        if spectrum and not playing:
            spectrum.quiet()
        elif spectrum and now >= next_spectrum:
            next_spectrum = now + 1 / SPECTRUM_RATE
            log_row, linear_row = drive.spectrum()
            spectrum.write(log_row, linear_row, 48000)

        # Signals go with each paint while music plays, then for a moment
        # while they settle, saying playback stopped, then none.
        if not paused and now >= next_signals:
            next_signals = now + 1 / SIGNALS_RATE
            if playing or drive.stop <= t < settle_until:
                send({
                    "signals": [] if args.empty_pool else values,
                    "time": round(drive.position, 3),
                    "playing": playing,
                })

        while not paused and pointer and pointer[0][0] <= t:
            send({"pointer": pointer.pop(0)[1]})

        # The display: the newest announce waiting, copied once a frame.
        if not paused and now >= next_frame:
            next_frame = now + 1 / DISPLAY_RATE
            newest = None
            while True:
                try:
                    newest = announced.get_nowait()
                except queue.Empty:
                    break
            if newest:
                take(newest, frames, report, args, dumped)

        flush()
        time.sleep(0.002)

    # Closing stdin asks it to go. After the grace it's killed, with its group.
    os.set_blocking(stdin, True)
    try:
        process.stdin.close()
    except BrokenPipeError:
        pass
    try:
        process.wait(timeout=GRACE_MS / 1000)
        report.exited_in_grace = True
    except subprocess.TimeoutExpired:
        report.exited_in_grace = False
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait()
    report.exit = process.returncode
    return report, time.monotonic() - started, running_time


def take(announce, frames, report, args, dumped):
    """One announce as rox's supervisor handles it (`copy` and
    `show_texture` in target.rs)."""
    kind, n, slot = announce
    if kind == "t":
        # No texture is lent here, so a `t` is refused as rox refuses it.
        report.texture += 1
        note_refused(report, True)
        return

    read = frames.copy(slot)
    if read[0] == "torn":
        report.torn += 1
        return
    if read[0] != "whole":
        if read[0] == "unwritten":
            report.unwritten += 1
        else:
            report.wrong_size += 1
            report.protocol.append(f"frame {n}: size words {read[1]}, the target is {frames.width}x{frames.height}")
        note_refused(report, True)
        return

    _, seq, pixels = read
    note_refused(report, False)
    if seq <= report.last_seq:
        report.old += 1
        return
    report.last_seq = seq

    # rox goes by the counter alone. The guide's protocol says more, and a
    # renderer that breaks it works by luck.
    if seq != 2 * n + 2 or slot != n % 2:
        report.protocol.append(f"frame {n} in slot {slot}: counter {seq}, the guide's is {2 * n + 2} in slot {n % 2}")

    report.copied += 1
    report.hashes.add(hashlib.blake2b(pixels, digest_size=16).digest())

    if n in args.dump and n not in dumped:
        dumped.add(n)
        path = os.path.join(args.dump_dir, f"frame-{n}.ppm")
        rgb = bytearray()
        for i in range(0, len(pixels), 4):
            rgb += pixels[i:i + 3]
        with open(path, "wb") as f:
            f.write(f"P6\n{frames.width} {frames.height}\n255\n".encode() + bytes(rgb))


def note_refused(report, refused):
    report.refused_run = report.refused_run + 1 if refused else 0
    report.longest_refused_run = max(report.longest_refused_run, report.refused_run)


def size(text):
    width, _, height = text.partition("x")
    width, height = int(width), int(height)
    if not (WIDTH_MIN <= width <= WIDTH_MAX and HEIGHT_MIN <= height <= HEIGHT_MAX):
        raise argparse.ArgumentTypeError(f"from {WIDTH_MIN}x{HEIGHT_MIN} to {WIDTH_MAX}x{HEIGHT_MAX}")
    return width, height


def main():
    argv = sys.argv[1:]
    command = "--" in argv
    if command:
        at = argv.index("--")
        argv, target = argv[:at], argv[at + 1:]

    parser = argparse.ArgumentParser(
        description="Run a rox renderer against a stand-in for rox. A development tool: it grants everything the flags ask for, with no approval.",
    )
    parser.add_argument("target", nargs="?" if command else 1, help="a renderer folder holding plugin.json")
    parser.add_argument("--size", type=size, default=(1280, 720), help="the target's pixels, WxH (1280x720)")
    parser.add_argument("--seconds", type=float, default=8.0, help="how long the session runs (8)")
    parser.add_argument("--cover", action="store_true", help="lend the cover mapping with a test picture")
    parser.add_argument("--spectrum", action="store_true", help="lend the spectrum mapping")
    parser.add_argument("--palette", action="store_true", help="send palette lines")
    parser.add_argument("--empty-pool", action="store_true", help="send an empty pool")
    parser.add_argument("--program-folder", dest="program_folders", action="append", default=[], help="a Program Folder, searched after the package's bin")
    parser.add_argument("--dump", type=lambda s: {int(n) for n in s.split(",")}, default=set(), help="frame numbers to save as PPM, comma-separated")
    parser.add_argument("--dump-dir", default=".", help="where --dump writes")
    parser.add_argument("--json", action="store_true", help="print the report as one JSON line")
    args = parser.parse_args(argv)
    args.command = command
    if command:
        if not target:
            parser.error("nothing after --")
        args.target = target

    report, seconds, running = run(args)
    summary = report.as_dict(seconds, running)

    if args.json:
        print(json.dumps(summary))
    else:
        for key, value in summary.items():
            print(f"{key.replace('_', ' ')}: {value}")

    stopped = summary["flooded"] or summary["longest_refused_run"] >= REFUSED_RUN
    if stopped:
        print("standin: rox would have stopped this renderer", file=sys.stderr)
    return 1 if stopped or summary["frames"] == 0 else 0


if __name__ == "__main__":
    sys.exit(main())
