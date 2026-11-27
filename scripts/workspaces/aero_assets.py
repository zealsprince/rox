#!/usr/bin/env python3
"""Draws every image the Aero workspace ships, from nothing but math.

Standard library only: the PNGs are written with zlib and struct, and every
soft edge is an analytic falloff, so nothing needs a blur pass. The output is
deterministic, so a rerun reproduces the committed files byte for byte.

    python3 scripts/workspaces/aero_assets.py [out_dir]

The default out_dir is crates/rox/assets/workspaces/Aero/assets. Run
aero_workspace.py afterwards: it reads the sizes and hashes from the files.
"""

import math
import os
import random
import struct
import sys
import zlib

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
DEFAULT_OUT = os.path.join(ROOT, "crates", "rox", "assets", "workspaces", "Aero", "assets")


# ── Raster ──


class Image:
    """Premultiplied RGBA floats, 0 to 1, row-major."""

    def __init__(self, w, h):
        self.w = w
        self.h = h
        self.px = [0.0] * (w * h * 4)

    def paint(self, shader):
        """Composites `shader(x, y)` over every pixel. `x` and `y` are
        normalized to 0..1 across the image; the shader returns straight
        (r, g, b, a) or None for nothing."""
        w, h, px = self.w, self.h, self.px
        for j in range(h):
            v = (j + 0.5) / h
            row = j * w * 4
            for i in range(w):
                c = shader((i + 0.5) / w, v)
                if c is None:
                    continue
                r, g, b, a = c
                if a <= 0.0:
                    continue
                if a > 1.0:
                    a = 1.0
                k = row + i * 4
                keep = 1.0 - a
                px[k] = r * a + px[k] * keep
                px[k + 1] = g * a + px[k + 1] * keep
                px[k + 2] = b * a + px[k + 2] * keep
                px[k + 3] = a + px[k + 3] * keep

    def add(self, shader):
        """Adds straight light `shader(x, y) -> (r, g, b, strength)` on top,
        lifting alpha by the brightest channel so light reads on a clear
        pixel too."""
        w, h, px = self.w, self.h, self.px
        for j in range(h):
            v = (j + 0.5) / h
            row = j * w * 4
            for i in range(w):
                c = shader((i + 0.5) / w, v)
                if c is None:
                    continue
                r, g, b, s = c
                if s <= 0.0:
                    continue
                k = row + i * 4
                px[k] += r * s
                px[k + 1] += g * s
                px[k + 2] += b * s
                px[k + 3] += max(r, g, b) * s * (1.0 - px[k + 3])

    def upscaled(self, w, h):
        """Bilinear resize, for layers drawn at a fraction of the size."""
        out = Image(w, h)
        src, sw, sh = self.px, self.w, self.h
        dst = out.px
        for j in range(h):
            fy = min(max((j + 0.5) * sh / h - 0.5, 0.0), sh - 1.0)
            y0 = int(fy)
            y1 = min(y0 + 1, sh - 1)
            ty = fy - y0
            for i in range(w):
                fx = min(max((i + 0.5) * sw / w - 0.5, 0.0), sw - 1.0)
                x0 = int(fx)
                x1 = min(x0 + 1, sw - 1)
                tx = fx - x0
                a = (y0 * sw + x0) * 4
                b = (y0 * sw + x1) * 4
                c = (y1 * sw + x0) * 4
                d = (y1 * sw + x1) * 4
                k = (j * w + i) * 4
                for ch in range(4):
                    top = src[a + ch] + (src[b + ch] - src[a + ch]) * tx
                    bot = src[c + ch] + (src[d + ch] - src[c + ch]) * tx
                    dst[k + ch] = top + (bot - top) * ty
        return out

    def under(self, other):
        """`self` composited over `other`, both the same size."""
        a, b = self.px, other.px
        for k in range(0, len(a), 4):
            keep = 1.0 - a[k + 3]
            a[k] += b[k] * keep
            a[k + 1] += b[k + 1] * keep
            a[k + 2] += b[k + 2] * keep
            a[k + 3] += b[k + 3] * keep

    def save(self, path, opaque=False):
        rows = bytearray()
        w, h, px = self.w, self.h, self.px
        channels = 3 if opaque else 4
        for j in range(h):
            line = bytearray()
            for i in range(w):
                k = (j * w + i) * 4
                a = min(max(px[k + 3], 0.0), 1.0)
                if opaque:
                    line += bytes(_byte(c) for c in px[k:k + 3])
                    continue
                if a < 1.0 / 510:
                    line += b"\0\0\0\0"
                    continue
                line += bytes(
                    (_byte(px[k] / a), _byte(px[k + 1] / a), _byte(px[k + 2] / a), _byte(a))
                )
            rows += line
        write_png(path, w, h, bytes(rows), channels)


def _byte(v):
    if v <= 0.0:
        return 0
    if v >= 1.0:
        return 255
    return int(v * 255.0 + 0.5)


def write_png(path, w, h, raw, channels):
    """Adaptive filtering per row (the smallest absolute sum wins), which is
    what keeps smooth gradients small."""
    stride = w * channels
    out = bytearray()
    prev = bytes(stride)
    for j in range(h):
        row = raw[j * stride:(j + 1) * stride]
        best = None
        for kind in (0, 1, 2, 4):
            f = _filter(kind, row, prev, channels)
            score = sum(b if b < 128 else 256 - b for b in f)
            if best is None or score < best[0]:
                best = (score, kind, f)
        out.append(best[1])
        out += best[2]
        prev = row

    def chunk(tag, data):
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    color = 6 if channels == 4 else 2
    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, color, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(out), 9))
    png += chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(png)


def _filter(kind, row, prev, bpp):
    n = len(row)
    if kind == 0:
        return row
    out = bytearray(n)
    if kind == 1:
        for i in range(n):
            left = row[i - bpp] if i >= bpp else 0
            out[i] = (row[i] - left) & 0xFF
        return out
    if kind == 2:
        for i in range(n):
            out[i] = (row[i] - prev[i]) & 0xFF
        return out
    for i in range(n):
        a = row[i - bpp] if i >= bpp else 0
        b = prev[i]
        c = prev[i - bpp] if i >= bpp else 0
        p = a + b - c
        pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
        pred = a if pa <= pb and pa <= pc else (b if pb <= pc else c)
        out[i] = (row[i] - pred) & 0xFF
    return out


# ── Math ──


def smooth(e0, e1, x):
    t = (x - e0) / (e1 - e0)
    t = 0.0 if t < 0.0 else (1.0 if t > 1.0 else t)
    return t * t * (3.0 - 2.0 * t)


def mix(a, b, t):
    return tuple(x + (y - x) * t for x, y in zip(a, b))


def hexc(s):
    s = s.lstrip("#")
    return tuple(int(s[i:i + 2], 16) / 255.0 for i in (0, 2, 4))


def ramp(stops, t):
    """Piecewise-linear colour across (position, colour) stops."""
    if t <= stops[0][0]:
        return stops[0][1]
    for (p0, c0), (p1, c1) in zip(stops, stops[1:]):
        if t <= p1:
            return mix(c0, c1, (t - p0) / (p1 - p0))
    return stops[-1][1]


def hash2(ix, iy, seed):
    n = (ix * 374761393 + iy * 668265263 + seed * 1442695041) & 0xFFFFFFFF
    n = ((n ^ (n >> 13)) * 1274126177) & 0xFFFFFFFF
    n ^= n >> 16
    return (n & 0xFFFFFF) / float(0xFFFFFF)


def vnoise(x, y, seed):
    ix, iy = math.floor(x), math.floor(y)
    fx, fy = x - ix, y - iy
    ux = fx * fx * (3 - 2 * fx)
    uy = fy * fy * (3 - 2 * fy)
    a = hash2(ix, iy, seed)
    b = hash2(ix + 1, iy, seed)
    c = hash2(ix, iy + 1, seed)
    d = hash2(ix + 1, iy + 1, seed)
    return a + (b - a) * ux + (c - a) * uy + (a - b - c + d) * ux * uy


def fbm(x, y, seed, octaves=5):
    total, amp, norm = 0.0, 0.5, 0.0
    for o in range(octaves):
        total += vnoise(x, y, seed + o * 17) * amp
        norm += amp
        x, y = x * 2.03 + 1.7, y * 2.03 - 0.9
        amp *= 0.5
    return total / norm


def rounded_box(px, py, hw, hh, r):
    """Signed distance to a rounded rectangle centred on the origin."""
    qx = abs(px) - hw + r
    qy = abs(py) - hh + r
    outside = math.hypot(max(qx, 0.0), max(qy, 0.0))
    return outside + min(max(qx, qy), 0.0) - r


def hsv(h, s, v):
    i = int(h * 6) % 6
    f = h * 6 - math.floor(h * 6)
    p, q, t = v * (1 - s), v * (1 - f * s), v * (1 - (1 - f) * s)
    return [(v, t, p), (q, v, p), (p, v, t), (p, q, v), (t, p, v), (v, p, q)][i]


# ── Sky ──


def sky(w, h, night):
    if night:
        stops = [
            (0.0, hexc("#060d2e")),
            (0.42, hexc("#13306b")),
            (0.70, hexc("#1d6a96")),
            (0.86, hexc("#3fa7b8")),
            (1.0, hexc("#1c5a74")),
        ]
        glow, cloud_lit, cloud_shade = hexc("#8be9ff"), hexc("#3c5f99"), hexc("#14284f")
    else:
        stops = [
            (0.0, hexc("#0f6fd6")),
            (0.38, hexc("#3aa2ee")),
            (0.68, hexc("#8fd4fa")),
            (0.84, hexc("#d6f4ff")),
            (1.0, hexc("#b5e8fb")),
        ]
        glow, cloud_lit, cloud_shade = hexc("#fffbea"), hexc("#ffffff"), hexc("#b4d3ec")

    img = Image(w, h)
    sun = (0.33, 0.03)

    def base(x, y):
        c = ramp(stops, y)
        # The horizon glow is a wide band; the sun's halo adds light, since a
        # mix toward white at part strength reads grey against the blue.
        band = math.exp(-((y - 0.83) / 0.09) ** 2) * (0.55 if night else 0.75)
        c = mix(c, glow, band)
        dx, dy = (x - sun[0]) * (w / h), y - sun[1]
        r2 = dx * dx + dy * dy
        halo = math.exp(-r2 / 0.008) * 0.55 + math.exp(-r2 / 0.08) * 0.25
        halo *= 0.25 if night else 1.0
        c = tuple(min(ch + g * halo, 1.0) for ch, g in zip(c, glow))
        return (c[0], c[1], c[2], 1.0)

    img.paint(base)

    if night:
        rng = random.Random(11)
        for _ in range(240):
            sx, sy, m = rng.random(), rng.random() * 0.62, rng.random()
            cx, cy = int(sx * w), int(sy * h)
            r = 0.7 + 1.4 * m * m
            fade = (0.35 + 0.65 * m) * (1.0 - smooth(0.4, 0.66, sy))
            for j in range(max(cy - 3, 0), min(cy + 4, h)):
                for i in range(max(cx - 3, 0), min(cx + 4, w)):
                    d = math.hypot(i - cx, j - cy) / r
                    s = math.exp(-d * d * 2.2) * fade
                    k = (j * w + i) * 4
                    for ch in range(3):
                        img.px[k + ch] = min(img.px[k + ch] + s, 1.0)

    # Cumulus: each cloud a cluster of round puffs over a flat base, the edge
    # roughened with noise. Drawn at half size and scaled up, since they're
    # soft and the noise is the slow part.
    rng = random.Random(4)
    aspect = w / h
    puffs = []
    for cx, base_y, width in [(0.08, 0.80, 0.30), (0.46, 0.78, 0.26), (0.86, 0.82, 0.34),
                              (0.66, 0.30, 0.16), (0.18, 0.36, 0.12)]:
        count = 5 + int(width * 30)
        for _ in range(count):
            px = cx + (rng.random() - 0.5) * width * 0.85
            edge = 1.0 - abs(px - cx) / (width / 2)
            r = (0.03 + 0.07 * edge * rng.random() + 0.02) * (width / 0.26) ** 0.5
            puffs.append((px * aspect, base_y - r * 0.55 - rng.random() * 0.03 * edge, r, base_y))

    cw, ch = w // 2, h // 2
    clouds = Image(cw, ch)

    def field(xa, y):
        total, base_y = 0.0, 1.0
        for px, py, r, by in puffs:
            dx, dy = (xa - px) / r, (y - py) / r
            d2 = dx * dx + dy * dy
            if d2 < 2.0:
                v = math.exp(-d2 * 3.2)
                total += v
                if v > 0.05:
                    base_y = by
        return total, base_y

    def cloud(x, y):
        xa = x * aspect
        f, base_y = field(xa, y)
        if f < 0.02:
            return None
        rough = fbm(x * 18.0, y * 18.0, 3, 4)
        density = smooth(0.30, 0.52, f + (rough - 0.5) * 0.25)
        density *= 1.0 - smooth(base_y - 0.004, base_y + 0.012, y)
        if density <= 0.003:
            return None
        # Lit from above: where the field grows downward, the surface faces up.
        above, _ = field(xa, y - 0.035)
        light = smooth(-0.5, 0.45, f - above + (rough - 0.5) * 0.25)
        light *= 1.0 - 0.45 * smooth(base_y - 0.06, base_y, y)
        c = mix(cloud_shade, cloud_lit, light)
        return (c[0], c[1], c[2], density * (0.6 if night else 0.96))

    clouds.paint(cloud)
    clouds = clouds.upscaled(w, h)
    clouds.under(img)
    return clouds


# ── Hills ──


def hills(w, h, front, night):
    """A strip of rolling hills with a glossy rim along the crest. Each
    crest is a sum of slow sines, so it tiles nowhere and stays smooth."""
    img = Image(w, h)
    if front:
        top, bot = (hexc("#7ee04a"), hexc("#1f9a3a")) if not night else (hexc("#1f6f5c"), hexc("#07263a"))
        waves = [(0.42, 1.3, 0.0), (0.16, 3.1, 1.2), (0.05, 7.0, 2.1)]
        base = 0.52
    else:
        top, bot = (hexc("#b8f0b0"), hexc("#55c2a0")) if not night else (hexc("#2a5f7a"), hexc("#123552"))
        waves = [(0.30, 1.8, 2.4), (0.12, 4.3, 0.4), (0.04, 9.0, 1.0)]
        base = 0.38

    def crest(x):
        return base - sum(a * 0.5 * math.sin(f * math.pi * 2 * x + p) for a, f, p in waves) * 0.55

    def shade(x, y):
        c0 = crest(x)
        if y < c0 - 0.01:
            return None
        edge = smooth(c0 - 0.008, c0 + 0.008, y)
        depth = (y - c0) / max(1.0 - c0, 1e-3)
        c = mix(top, bot, smooth(0.0, 1.0, depth) ** 0.8)
        # The gloss: a bright film just under the crest, then a softer sheen.
        rim = math.exp(-((y - c0 - 0.025) / 0.022) ** 2)
        sheen = math.exp(-((y - c0 - 0.11) / 0.07) ** 2) * 0.35
        lift = (rim * (0.55 if not night else 0.25) + sheen * (1.0 if not night else 0.4))
        c = mix(c, (1.0, 1.0, 1.0) if not night else hexc("#8fe8ff"), min(lift, 0.9))
        return (c[0], c[1], c[2], edge)

    img.paint(shade)
    return img


# ── Aurora ──


def aurora(w, h, seed, hues):
    """A curtain: a ribbon that winds across the sprite with fine vertical
    streaks, brightest along its lower edge, fading up into the air."""
    img = Image(w, h)
    rng = random.Random(seed)
    phases = [rng.random() * 6.28 for _ in range(3)]

    def path(x):
        return (
            0.62
            + 0.14 * math.sin(x * 4.1 + phases[0])
            + 0.06 * math.sin(x * 9.3 + phases[1])
            + 0.02 * math.sin(x * 23.0 + phases[2])
        )

    def light(x, y):
        p = path(x)
        below = (y - p) / 0.05
        above = (p - y) / 0.42
        if below > 0:
            body = math.exp(-below * below)
        else:
            body = math.exp(-above * above * 1.4)
        streak = 0.55 + 0.45 * vnoise(x * 90.0, y * 2.0, seed)
        ends = smooth(0.0, 0.18, x) * smooth(0.0, 0.18, 1.0 - x) * smooth(0.0, 0.3, y)
        s = body * streak * ends
        if s < 0.003:
            return None
        t = min(max((p - y) / 0.5 + 0.15, 0.0), 1.0)
        c = ramp(hues, t)
        return (c[0], c[1], c[2], s * 0.9)

    img.paint(light)
    return img


# ── Bubbles, bokeh, droplet, flare ──


def bubble(size):
    img = Image(size, size)
    hue0 = 0.48

    def shade(x, y):
        dx, dy = x * 2 - 1, y * 2 - 1
        r = math.hypot(dx, dy)
        if r > 1.0:
            return None
        edge = 1.0 - smooth(0.965, 1.0, r)
        # Thin film: a faint body, a rim that thickens toward the edge, and
        # the colour bands a soap film throws, swirling with the angle.
        angle = math.atan2(dy, dx)
        film = hsv((hue0 + 0.5 * math.sin(angle * 1.5 + r * 5.0) + r * 0.6) % 1.0, 0.55, 1.0)
        rim = smooth(0.72, 0.985, r) ** 2
        a = 0.06 + rim * 0.55
        c = mix((0.86, 0.96, 1.0), film, 0.35 + 0.45 * rim)
        return (c[0], c[1], c[2], a * edge)

    img.paint(shade)

    def gleam(x, y):
        dx, dy = x * 2 - 1, y * 2 - 1
        # The window reflection, upper left, and a small one lower right.
        hx, hy = (dx + 0.38) / 0.30, (dy + 0.42) / 0.18
        main = math.exp(-(hx * hx + hy * hy) * 1.6)
        bx, by = (dx - 0.45) / 0.12, (dy - 0.48) / 0.08
        back = math.exp(-(bx * bx + by * by) * 1.6) * 0.5
        arc_r = math.hypot(dx, dy)
        arc = math.exp(-((arc_r - 0.86) / 0.05) ** 2) * smooth(0.2, -0.6, dy + dx * 0.3) * 0.35
        s = main * 0.95 + back + arc
        return (1.0, 1.0, 1.0, s) if s > 0.004 else None

    img.add(gleam)

    # The bubble's 0.95 is drawn in rather than set on the item: the fade
    # that hides its wrap reads past full, and an item's opacity share stops
    # only once the item itself is fully opaque.
    img.px = [v * 0.95 for v in img.px]
    return img


def bokeh(size, tint):
    img = Image(size, size)

    def shade(x, y):
        r = math.hypot(x * 2 - 1, y * 2 - 1)
        if r > 1.0:
            return None
        edge = 1.0 - smooth(0.88, 1.0, r)
        body = 0.32 + 0.25 * smooth(0.55, 0.92, r)
        return (tint[0], tint[1], tint[2], body * edge)

    img.paint(shade)
    return img


def droplet(w, h):
    """A bead of water sitting on glass: a rounded drop, dark at its lower
    rim where it bends the light, a bright caustic below, a glint above."""
    img = Image(w, h)

    def inside(x, y):
        # A circle stretched into a teardrop toward the top.
        dx = (x - 0.5) * 2
        dy = (y - 0.62) / 0.38
        taper = 1.0 + max(0.0, -dy) * 0.55
        return math.hypot(dx * taper, dy)

    def shade(x, y):
        r = inside(x, y)
        if r > 1.0:
            return None
        edge = 1.0 - smooth(0.93, 1.0, r)
        lower = smooth(0.1, 0.9, y)
        dark = smooth(0.55, 1.0, r) * lower
        c = mix((0.78, 0.94, 1.0), (0.05, 0.30, 0.52), dark * 0.85)
        a = 0.22 + 0.55 * smooth(0.6, 1.0, r)
        return (c[0], c[1], c[2], a * edge)

    img.paint(shade)

    def light(x, y):
        gx, gy = (x - 0.40) / 0.12, (y - 0.48) / 0.08
        glint = math.exp(-(gx * gx + gy * gy))
        cx, cy = (x - 0.5) / 0.26, (y - 0.86) / 0.05
        caustic = math.exp(-(cx * cx + cy * cy)) * 0.7 if inside(x, y) < 1.0 else 0.0
        s = glint + caustic
        return (1.0, 1.0, 1.0, s) if s > 0.004 else None

    img.add(light)
    return img


def flare(size):
    img = Image(size, size)
    rng = random.Random(5)
    rays = [(rng.random() * math.pi, 0.4 + rng.random() * 0.6) for _ in range(14)]

    def light(x, y):
        dx, dy = x * 2 - 1, y * 2 - 1
        r = math.hypot(dx, dy)
        if r > 1.0:
            return None
        fade = 1.0 - smooth(0.6, 1.0, r)
        core = math.exp(-r * r / 0.004) + math.exp(-r * r / 0.03) * 0.6
        glow = math.exp(-r * r / 0.12) * 0.35
        angle = math.atan2(dy, dx)
        streaks = 0.0
        for a, strength in rays:
            d = abs(math.sin(angle - a))
            streaks += math.exp(-(d / 0.012) ** 2) * strength * math.exp(-r * 3.2)
        streaks += math.exp(-(dy / 0.008) ** 2) * math.exp(-abs(dx) * 2.2) * 0.9
        ring = math.exp(-((r - 0.62) / 0.025) ** 2) * 0.16
        warm = mix((1.0, 0.98, 0.92), (0.75, 0.92, 1.0), smooth(0.0, 0.7, r))
        ring_c = hsv((0.55 + r * 0.9) % 1.0, 0.45, 1.0)
        s = (core + glow + streaks) * fade
        c = mix(warm, ring_c, ring / max(s + ring, 1e-6))
        return (c[0], c[1], c[2], (s + ring) * fade)

    img.add(light)
    return img


def ghost(size, tint):
    """A lens ghost: a faint ring-edged disc that rides the flare's line."""
    img = Image(size, size)

    def light(x, y):
        r = math.hypot(x * 2 - 1, y * 2 - 1)
        if r > 1.0:
            return None
        edge = 1.0 - smooth(0.9, 1.0, r)
        s = (0.10 + 0.22 * smooth(0.7, 0.95, r)) * edge
        return (tint[0], tint[1], tint[2], s)

    img.add(light)
    return img


# ── Glass ──


def glass(w, h, radius, pad, night, well=False):
    """A sheet of glass with its shadow. `pad` pixels of the sprite are the
    shadow's margin, so an item sized to the sprite sits a pad wider than
    the panel it backs. The gloss covers the top half under a soft curve."""
    img = Image(w, h)
    hw, hh = w / 2 - pad, h / 2 - pad

    if night:
        body, body_a = hexc("#0d2444"), 0.62 if not well else 0.42
        rim_c, rim_a = hexc("#7fe3ff"), 0.55
        hair_c = hexc("#020a18")
        gloss_a = 0.16
    else:
        body, body_a = hexc("#f4fbff"), 0.50 if not well else 0.30
        rim_c, rim_a = (1.0, 1.0, 1.0), 0.95
        hair_c = hexc("#2a6d9e")
        gloss_a = 0.55

    def shadow(x, y):
        px, py = x * w - w / 2, y * h - h / 2 - pad * 0.25
        d = rounded_box(px, py, hw, hh, radius)
        if d <= 0 or well:
            return None
        s = math.exp(-((d / (pad * 0.45)) ** 2)) * (0.30 if not night else 0.5)
        return (0.02, 0.12, 0.25, s)

    img.paint(shadow)

    def sheet(x, y):
        px, py = x * w - w / 2, y * h - h / 2
        d = rounded_box(px, py, hw, hh, radius)
        if d > 1.0:
            return None
        cover = 1.0 - smooth(-0.5, 1.0, d)
        # A cool tint toward the bottom, the way thick glass looks.
        t = (py + hh) / (2 * hh)
        c = mix(body, hexc("#cfe9fb") if not night else hexc("#081a33"), t * 0.6)
        inner = math.exp(-((-d) / 22.0) ** 2) * (0.18 if not night else 0.22)
        c = mix(c, (1.0, 1.0, 1.0) if not night else hexc("#2c74a8"), inner)
        return (c[0], c[1], c[2], (body_a + inner * 0.5) * cover)

    img.paint(sheet)

    def gloss(x, y):
        px, py = x * w - w / 2, y * h - h / 2
        d = rounded_box(px, py, hw - 3, hh - 3, max(radius - 3, 1))
        if d > 0:
            return None
        # The highlight's lower edge bows down toward the middle.
        u = px / hw
        line = -hh + 2 * hh * (0.40 + 0.06 * (1 - u * u)) if not well else -hh + 2 * hh * 0.30
        if py > line + 2:
            return None
        t = (py + hh) / max(line + hh, 1)
        s = (1.0 - smooth(0.0, 1.0, t)) * gloss_a + gloss_a * 0.25
        s *= 1.0 - smooth(line - 2, line + 2, py)
        s *= smooth(0.0, 6.0, -d)
        return (1.0, 1.0, 1.0, s * 0.55)

    img.paint(gloss)

    def rims(x, y):
        px, py = x * w - w / 2, y * h - h / 2
        d = rounded_box(px, py, hw, hh, radius)
        hair = math.exp(-((d - 0.2) / 0.8) ** 2) * (0.30 if not night else 0.7)
        top = 1.0 - smooth(-hh, hh, py) * 0.7
        rim = math.exp(-((d + 1.6) / 0.9) ** 2) * rim_a * top
        if hair + rim < 0.004:
            return None
        if rim >= hair:
            return (rim_c[0], rim_c[1], rim_c[2], rim)
        return (hair_c[0], hair_c[1], hair_c[2], hair)

    img.paint(rims)
    return img


# What a pane draws, back to front.
WHOLE = ("shadow", "sheet", "shine", "rims")

# Nine-sliced panes: radius, shadow pad, gloss band height, whether it's a
# shadowless well, all in pixels, and the layers it draws. Each also ships a
# `_night` copy.
PANES = {
    # The deck and the sheet run 510 to 1250 units tall, so a band this deep
    # reads as gloss on the tall ones without filling the short ones.
    "glass_pane": (26, 24, 210, False, WHOLE),
    "glass_well": (18, 6, 64, True, WHOLE),
    # The screen's sheet sits behind its render target and its shine and
    # rims in front of it. Both are cut to the same insets and sized alike,
    # so their outlines are one.
    "glass_screen": (18, 16, 80, False, ("shadow", "sheet")),
    "glass_screen_gloss": (18, 16, 80, False, ("shine", "rims")),
}

# The stretchable middle of a pane, both ways. Wide enough that the gloss's
# bow and the tint's ramp resample smoothly when it spans a whole pane.
PANE_MIDDLE = 32


def pane_insets(radius, pad, gloss):
    """Left, top, right, bottom, in pixels: the corners hold the rounding,
    the shadow and the inner glow, the top inset the gloss band."""
    side = pad + radius + 14
    return [side, pad + gloss + 10, side, pad + radius + 18]


def pane(radius, pad, gloss, night, well=False, layers=WHOLE):
    """A sheet of glass cut for nine-slicing at `pane_insets`. The gloss is a
    band of `gloss` pixels down from the top edge, so it keeps its height on
    any pane. Its lower edge bows only across the middle columns, which the
    cut stretches, so the bow spans whatever width the pane takes. Only the
    layers named in `layers` are drawn."""
    side, top, _, bottom = pane_insets(radius, pad, gloss)
    mid = PANE_MIDDLE
    w, h = side * 2 + mid, top + mid + bottom
    img = Image(w, h)
    hw, hh = w / 2 - pad, h / 2 - pad
    cx = w / 2

    if night:
        body, body_a = hexc("#0d2444"), 0.62 if not well else 0.42
        rim_c, rim_a = hexc("#7fe3ff"), 0.55
        hair_c = hexc("#020a18")
        gloss_a = 0.16
    else:
        body, body_a = hexc("#f4fbff"), 0.50 if not well else 0.30
        rim_c, rim_a = (1.0, 1.0, 1.0), 0.95
        hair_c = hexc("#2a6d9e")
        gloss_a = 0.55

    def shadow(x, y):
        px, py = x * w - w / 2, y * h - h / 2 - pad * 0.25
        d = rounded_box(px, py, hw, hh, radius)
        if d <= 0 or well:
            return None
        s = math.exp(-((d / (pad * 0.45)) ** 2)) * (0.30 if not night else 0.5)
        return (0.02, 0.12, 0.25, s)

    if "shadow" in layers:
        img.paint(shadow)

    def sheet(x, y):
        px, py = x * w - w / 2, y * h - h / 2
        d = rounded_box(px, py, hw, hh, radius)
        if d > 1.0:
            return None
        cover = 1.0 - smooth(-0.5, 1.0, d)
        # The cool tint deepens toward the bottom across the whole source, so
        # the stretched middle carries it as one even ramp.
        t = (py + hh) / (2 * hh)
        c = mix(body, hexc("#cfe9fb") if not night else hexc("#081a33"), t * 0.6)
        inner = math.exp(-((-d) / 22.0) ** 2) * (0.18 if not night else 0.22)
        c = mix(c, (1.0, 1.0, 1.0) if not night else hexc("#2c74a8"), inner)
        return (c[0], c[1], c[2], (body_a + inner * 0.5) * cover)

    if "sheet" in layers:
        img.paint(sheet)

    def shine(x, y):
        px, py = x * w - w / 2, y * h - h / 2
        d = rounded_box(px, py, hw - 3, hh - 3, max(radius - 3, 1))
        if d > 0:
            return None
        # Flat through the corners, bowed across the stretched middle.
        u = min(max((x * w - cx) / (mid / 2), -1.0), 1.0)
        line = pad + gloss * (0.86 + 0.14 * (1 - u * u)) if not well else pad + gloss
        y_px = y * h
        if y_px > line + 2:
            return None
        t = (y_px - pad) / max(line - pad, 1)
        s = (1.0 - smooth(0.0, 1.0, t)) * gloss_a + gloss_a * 0.25
        s *= 1.0 - smooth(line - 2, line + 2, y_px)
        s *= smooth(0.0, 6.0, -d)
        return (1.0, 1.0, 1.0, s * 0.7)

    if "shine" in layers:
        img.paint(shine)

    def rims(x, y):
        px, py = x * w - w / 2, y * h - h / 2
        d = rounded_box(px, py, hw, hh, radius)
        hair = math.exp(-((d - 0.2) / 0.8) ** 2) * (0.30 if not night else 0.7)
        # Brightest along the top, fading over the top inset and the middle.
        fade = 1.0 - smooth(pad, top + mid, y * h) * 0.7
        rim = math.exp(-((d + 1.6) / 0.9) ** 2) * rim_a * fade
        if hair + rim < 0.004:
            return None
        if rim >= hair:
            return (rim_c[0], rim_c[1], rim_c[2], rim)
        return (hair_c[0], hair_c[1], hair_c[2], hair)

    if "rims" in layers:
        img.paint(rims)
    return img


def cover_gloss(size, radius):
    """Laid over the cover art: the gel's top-half shine and a bright rim,
    clear everywhere else so the art reads through untouched."""
    img = Image(size, size)
    half = size / 2

    def shade(x, y):
        px, py = x * size - half, y * size - half
        d = rounded_box(px, py, half - 1, half - 1, radius)
        if d > 1.0:
            return None
        u = px / half
        line = -half + size * (0.42 + 0.05 * (1 - u * u))
        s = 0.0
        if py < line:
            t = (py + half) / (line + half)
            s = (1.0 - smooth(0.0, 1.0, t)) * 0.30 + 0.05
            s *= 1.0 - smooth(line - 3, line, py)
        rim = math.exp(-((d + 1.2) / 1.0) ** 2) * (0.75 - 0.5 * (py + half) / size)
        hair = math.exp(-((d - 0.3) / 0.8) ** 2) * 0.35
        if rim + s < hair:
            return (0.04, 0.2, 0.35, hair)
        return (1.0, 1.0, 1.0, min(s + rim, 1.0))

    img.paint(shade)
    return img


def shadow(w, h, radius, pad):
    img = Image(w, h)
    hw, hh = w / 2 - pad, h / 2 - pad

    def shade(x, y):
        d = rounded_box(x * w - w / 2, y * h - h / 2 - pad * 0.2, hw, hh, radius)
        s = math.exp(-((max(d, 0.0) / (pad * 0.5)) ** 2)) * 0.45
        return (0.0, 0.08, 0.18, s) if s > 0.003 else None

    img.paint(shade)
    return img


# ── Buttons ──


GELS = {
    "aqua": ("#7fe9ff", "#0aa0dc", "#055e9c"),
    "green": ("#c4ff8a", "#3fc23a", "#147a2a"),
    "clear": ("#ffffff", "#cfe8f7", "#8fb7d3"),
}


def gel(w, h, hue, pill, look=None):
    """A gel button: a saturated body lit from below, a top-half gloss
    under a curve, a dark hairline and a soft glow beneath. `look` draws
    the copy a button swaps in under the pointer ("lit") or while pressed
    ("down")."""
    light, mid, deep = (hexc(c) for c in GELS[hue])
    img = Image(w, h)
    pad = h * 0.12
    hw, hh = w / 2 - pad, h / 2 - pad
    r = hh if pill else min(hw, hh)
    clear = hue == "clear"

    def body(x, y):
        px, py = x * w - w / 2, y * h - h / 2
        if pill:
            d = rounded_box(px, py, hw, hh, r)
        else:
            d = math.hypot(px, py) - r
        if d > 1.0:
            # Lighter above than below, blended so the halves don't meet in a
            # hard line across the sprite.
            sh = math.exp(-((d / (pad * 0.55)) ** 2)) * 0.35
            sh *= 0.5 + 0.5 * smooth(-hh * 0.5, hh * 0.5, py)
            return (0.0, 0.12, 0.25, sh) if sh > 0.003 else None
        cover = 1.0 - smooth(-0.6, 0.8, d)
        t = (py + hh) / (2 * hh)
        c = ramp([(0.0, deep), (0.55, mid), (1.0, light)], t)
        # The inner glow pooling at the bottom, as light passes through gel.
        glow = math.exp(-(((py - hh * 0.75) / (hh * 0.45)) ** 2) - (px / (hw * 0.8)) ** 2)
        c = mix(c, light, glow * 0.6)
        a = 0.55 if clear else 1.0
        return (c[0], c[1], c[2], a * cover)

    img.paint(body)

    def shine(x, y):
        px, py = x * w - w / 2, y * h - h / 2
        if pill:
            d = rounded_box(px, py, hw - 2, hh - 2, max(r - 2, 1))
        else:
            d = math.hypot(px, py) - (r - 2)
        if d > 0:
            hair = math.exp(-((d - 2.4) / 1.0) ** 2) * 0.5
            return (deep[0] * 0.5, deep[1] * 0.5, deep[2] * 0.5, hair) if hair > 0.01 else None
        line = -hh * 0.02
        if py > line:
            return None
        t = (py + hh) / (line + hh)
        s = 0.85 - 0.6 * t
        s *= 1.0 - smooth(line - 3, line, py)
        s *= smooth(0.0, 3.0, -d)
        return (1.0, 1.0, 1.0, s * 0.8)

    img.paint(shine)
    if look:
        gel_look(img, look == "lit", clear)
    return img


def gel_look(img, lit, clear):
    """Paints a round gel's hover or pressed look into its own copy: lit
    adds a brighter gloss, a bloom at the bottom and a glow ringing the
    outline; pressed shades the body, sinks a shadow under the top rim and
    lights the lower one. Both keep the gel's outline, so the button's
    picture hit test and the swap leave nothing moving."""
    w, h = img.w, img.h
    pad = h * 0.12
    r = min(w / 2 - pad, h / 2 - pad)

    if lit and clear:
        # The clear gel is half see-through, and the white light below reads
        # as milk on it, so its lit copy takes an aqua wash first.
        def wash(x, y):
            px, py = x * w - w / 2, y * h - h / 2
            d = math.hypot(px, py) - r
            if d > 0.0:
                return None
            cover = 1.0 - smooth(-1.2, 0.0, d)
            depth = (py + r) / (2 * r)
            return (0.42, 0.86, 1.0, (0.30 + 0.25 * depth) * cover)

        img.paint(wash)

    def shade(x, y):
        px, py = x * w - w / 2, y * h - h / 2
        d = math.hypot(px, py) - r
        if lit:
            if d > 0.0:
                # Cyan rather than white, so the glow still shows on the pale
                # glass of the light theme.
                glow = math.exp(-((d / (pad * 0.5)) ** 2)) * 0.75
                return (0.30, 0.82, 1.0, glow) if glow > 0.003 else None
            cover = 1.0 - smooth(-1.2, 0.0, d)
            ring = math.exp(-((d + 2.6) / 1.4) ** 2) * 0.55
            gloss = 0.0
            if py < -r * 0.06:
                t = (py + r) / (r * 0.94)
                gloss = (0.18 - 0.12 * t) * smooth(0.0, 4.0, -d - 2.0)
            bloom = math.exp(-(((py - r * 0.62) / (r * 0.42)) ** 2) - (px / (r * 0.7)) ** 2) * 0.5
            s = min(ring + gloss + bloom, 1.0)
            # Tinted toward the bloom's cyan so lit reads as more light, not milk.
            c = mix((1.0, 1.0, 1.0), (0.72, 0.98, 1.0), bloom / max(s, 1e-6))
            return (c[0], c[1], c[2], s * cover)

        if d > 0.6:
            return None
        cover = 1.0 - smooth(-0.6, 0.6, d)
        # An inset: dark pooling under the top rim, the whole body dimmed.
        depth = (py + r) / (2 * r)
        sink = math.exp(-((-d) / (r * 0.22)) ** 2) * (1.0 - smooth(0.0, 0.7, depth)) * 0.5
        s = 0.22 + sink
        return (0.0, 0.07, 0.16, min(s, 0.85) * cover)

    img.paint(shade)

    if not lit:
        def under(x, y):
            px, py = x * w - w / 2, y * h - h / 2
            d = math.hypot(px, py) - r
            if d > 0.0:
                return None
            lip = math.exp(-((d + 3.0) / 1.6) ** 2) * smooth(0.1, 0.8, py / r) * 0.5
            return (0.75, 0.95, 1.0, lip) if lip > 0.003 else None

        img.paint(under)


def glyph(size, kind):
    """White glyphs with a soft dark halo, so they read on any gel."""
    img = Image(size, size)
    s = size

    def tri(px, py, ax, ay, bx, by, cx, cy):
        # Either winding: the sign of the area turns the edges outward.
        turn = -1.0 if (bx - ax) * (cy - ay) - (by - ay) * (cx - ax) < 0 else 1.0

        def edge(x0, y0, x1, y1):
            nx, ny = (y1 - y0) * turn, (x0 - x1) * turn
            ln = math.hypot(nx, ny)
            return ((px - x0) * nx + (py - y0) * ny) / ln

        return max(edge(ax, ay, bx, by), edge(bx, by, cx, cy), edge(cx, cy, ax, ay))

    def bar(px, py, x0, y0, x1, y1, r=0.02):
        return rounded_box(px - (x0 + x1) / 2, py - (y0 + y1) / 2, (x1 - x0) / 2, (y1 - y0) / 2, r)

    def loop_arrow(px, py, one):
        # A rounded loop with a break at the top right, an arrowhead closing
        # it, and a small 1 inside for repeat-one.
        d = abs(rounded_box(px, py, 0.25, 0.15, 0.12)) - 0.04
        if px > 0.0 and py < 0.0 and px < 0.17:
            d = max(d, 0.04 - min(abs(py + 0.15), 1.0) + 0.5)
        head = tri(px, py, 0.02, -0.26, 0.02, -0.04, 0.17, -0.15)
        dist = min(d, head)
        if one:
            dist = min(dist, bar(px, py, -0.025, -0.07, 0.025, 0.08, 0.01))
        return dist

    def shape(px, py, kind=kind):
        if kind == "play":
            return tri(px, py, -0.16, -0.25, -0.16, 0.25, 0.26, 0.0) - 0.02
        if kind == "pause":
            return min(bar(px, py, -0.19, -0.24, -0.05, 0.24), bar(px, py, 0.05, -0.24, 0.19, 0.24))
        if kind == "next":
            return min(
                tri(px, py, -0.24, -0.20, -0.24, 0.20, 0.06, 0.0) - 0.015,
                tri(px, py, 0.0, -0.20, 0.0, 0.20, 0.26, 0.0) - 0.015,
                bar(px, py, 0.24, -0.20, 0.30, 0.20),
            )
        if kind == "prev":
            return shape(-px, py, "next")
        if kind == "shuffle":
            def strand(px, py, flip):
                yy = py * flip
                wave = 0.13 * math.tanh((px) * 9.0)
                d = abs(yy - wave) - 0.035
                d = max(d, abs(px) - 0.22)
                head = tri(px, yy, 0.20, wave - 0.10, 0.20, wave + 0.10, 0.33, wave)
                return min(d, head)
            return min(strand(px, py, 1), strand(px, py, -1))
        if kind == "repeat":
            return loop_arrow(px, py, False)
        if kind == "repeat1":
            return loop_arrow(px, py, True)
        return 1.0

    def shade(x, y):
        d = shape(x - 0.5, y - 0.5) * s
        halo = math.exp(-((max(d, 0.0) / (s * 0.05)) ** 2)) * 0.35
        if d > 0.0:
            return (0.02, 0.18, 0.32, halo) if halo > 0.004 else None
        cover = smooth(0.0, 1.4, -d)
        return (1.0, 1.0, 1.0, cover + halo * (1 - cover))

    img.paint(shade)
    return img


# ── Out ──


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_OUT
    os.makedirs(out, exist_ok=True)

    written = set()

    def save(name, img, opaque=False):
        path = os.path.join(out, name)
        img.save(path, opaque)
        written.add(name)
        print(f"{name:24} {img.w}x{img.h:<5} {os.path.getsize(path) / 1024:7.1f} KiB")

    save("sky_day.png", sky(1280, 720, False), opaque=True)
    save("sky_night.png", sky(1280, 720, True), opaque=True)
    save("hills_back.png", hills(1280, 220, False, False))
    save("hills_front.png", hills(1280, 200, True, False))
    save("hills_back_night.png", hills(1280, 220, False, True))
    save("hills_front_night.png", hills(1280, 200, True, True))

    green = [(0.0, hexc("#9dffb4")), (0.45, hexc("#38f2c4")), (1.0, hexc("#8a7dff"))]
    cyan = [(0.0, hexc("#7cf7ff")), (0.5, hexc("#45c8ff")), (1.0, hexc("#c58bff"))]
    save("aurora_a.png", aurora(768, 288, 3, green))
    save("aurora_b.png", aurora(768, 288, 9, cyan))

    save("bubble_l.png", bubble(256))
    save("bubble_m.png", bubble(160))
    save("bubble_s.png", bubble(96))
    save("bokeh.png", bokeh(96, (1.0, 1.0, 1.0)))
    save("bokeh_aqua.png", bokeh(96, hexc("#8ff3ff")))
    save("droplet.png", droplet(96, 128))
    save("flare.png", flare(512))
    save("ghost.png", ghost(96, hexc("#bff7ff")))

    # The panes are nine-sliced, so each is drawn once at its corner size and
    # serves every frame and window size.
    for name, (radius, pad, gloss, well, layers) in PANES.items():
        save(f"{name}.png", pane(radius, pad, gloss, False, well, layers))
        save(f"{name}_night.png", pane(radius, pad, gloss, True, well, layers))

    # The shard only ever shows tilted at one size, so it stays a picture.
    save("glass_shard.png", glass(520, 380, 30, 20, False, well=True))
    save("cover_gloss.png", cover_gloss(512, 14))
    save("shadow.png", shadow(160, 160, 40, 28))

    # Each gel with the whole copies its hover and pressed looks swap in.
    for hue in ("aqua", "green", "clear"):
        save(f"gel_round_{hue}.png", gel(208, 208, hue, False))
        save(f"gel_round_{hue}_lit.png", gel(208, 208, hue, False, "lit"))
        save(f"gel_round_{hue}_down.png", gel(208, 208, hue, False, "down"))
    for kind in ("play", "pause", "next", "prev", "shuffle", "repeat", "repeat1"):
        save(f"glyph_{kind}.png", glyph(128, kind))

    # The workspace script lists every file in the folder, so a picture this
    # run no longer draws has to go.
    for name in sorted(os.listdir(out)):
        if name.endswith(".png") and name not in written:
            os.remove(os.path.join(out, name))
            print(f"{name:24} removed")


if __name__ == "__main__":
    main()
