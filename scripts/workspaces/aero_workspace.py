#!/usr/bin/env python3
"""Writes the Aero workspace bundle from the compact description below.

    python3 scripts/workspaces/aero_workspace.py

It reads every file in crates/rox/assets/workspaces/Aero/assets for the
asset table's sizes and hashes, so run aero_assets.py first whenever an image
changes. The whole look is one canvas filling the window, laid out at
1920x1080 with anchors, fractions and size limits so it reflows, and it
imports aero_assets.py for the panes' nine-slice insets.
"""

import hashlib
import json
import os

from aero_assets import PANES, pane_insets

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
SHIPPED = os.path.join(ROOT, "crates", "rox", "assets", "workspaces")
ASSETS = os.path.join(SHIPPED, "Aero", "assets")

# Pool signal ids.
LOW, MID, HIGH, LEVEL, BEAT = range(1, 6)


# ── Items ──


def S(f):
    return {"stage": f}


def U(n):
    return {"units": n}


def _axis(v):
    return v if isinstance(v, dict) else float(v)


def item(id, content, *, anchor="center", pos=(0, 0, 0), size=(U(100), U(100)), rot=(0, 0, 0),
         scale=1.0, opacity=1.0, sway=None, action="", state="", cases=None, name="", min_size=None,
         max_size=None, hover=None, pressed=None):
    place = {
        "anchor": anchor,
        "position": [_axis(pos[0]), _axis(pos[1]), float(pos[2])],
        "size": list(size),
    }
    if min_size:
        place["min_size"] = [None if v is None else float(v) for v in min_size]
    if max_size:
        place["max_size"] = [None if v is None else float(v) for v in max_size]
    place["rotation"] = [float(v) for v in rot]
    place["scale"] = scale

    out = {
        "id": id,
        "name": name,
        "place": place,
        "visible": True,
        "locked": False,
        "opacity": opacity,
        "content": content,
    }
    if sway:
        out["sway"] = {
            "position": [float(v) for v in sway.get("position", (0, 0, 0))],
            "rotation": [float(v) for v in sway.get("rotation", (0, 0, 0))],
        }
    if action:
        out["action"] = action
    if state:
        out["state"] = state
        out["cases"] = cases or []
    if hover:
        out["hover"] = hover
    if pressed:
        out["pressed"] = pressed
    return out


def case(when, visible=True, opacity=1.0, asset="", color=""):
    out = {"when": when, "visible": visible, "opacity": opacity}
    if asset:
        out["asset"] = asset
    if color:
        out["color"] = color
    return out


def image(asset, fit="stretch"):
    return {"kind": "image", "asset": asset, "fit": fit}


def sliced(asset):
    """A pane cut nine ways, so its corners and its gloss keep their size
    whatever box it fills. The night copy is drawn to the same insets."""
    return {"kind": "image", "asset": asset, "fit": "stretch", "slice": pane_insets(*PANES[asset][:3])}


def text(line, size, color, align="center"):
    return {"kind": "text", "text": line, "font": None, "size": size, "color": color, "align": align}


def group(items):
    return {"kind": "group", "items": items}


def themed(night):
    """Follows the theme: the night picture in dark mode."""
    return dict(state="app.theme", cases=[case("dark", asset=night)])


def tabs(*panels, active=0):
    return {"panel_name": "TabPanel", "children": list(panels), "info": {"tabs": {"active_index": active}}}


def panel(name, config):
    return {"panel_name": name, "children": [], "info": {"panel": config}}


def grid(*panels):
    """A grid holding one tab group. A group of one panel draws no tabs."""
    return {
        "kind": "grid",
        "dock": {
            "version": 1,
            "center": {
                "panel_name": "StackPanel",
                "children": [tabs(*panels)],
                "info": {"stack": {"sizes": [], "axis": 1}},
            },
        },
    }


def shader(name, routes):
    return {
        "kind": "shader",
        "shader": {"enabled": True, "source": "", "name": name, "path": None, "routes": routes,
                   "run_when_idle": False},
    }


def slot(signal, n, lo=0.0, hi=1.0):
    return {"enabled": True, "signal": signal, "target": f"slot{n}", "from": lo, "to": hi}


ROUTES = []


def route(target, signal, lo, hi):
    ROUTES.append({"enabled": True, "signal": signal, "target": target, "from": lo, "to": hi})


def drift(target, period, shape="sine", phase=0.0, lo=0.0, hi=1.0, curve=None):
    """A route on the canvas's own clock. It reads no signal, so it moves at
    rest too, and while one is on the canvas draws every frame it shows.
    A route reads from 0 up, never below, so anything meant to swing both
    ways sits half its sway back from where it should centre."""
    out = {"enabled": True, "signal": 0, "target": target, "from": lo, "to": hi,
           "time": {"period": float(period), "shape": shape, "phase": float(phase)}}
    if curve:
        out["curve"] = curve
    ROUTES.append(out)


# ── Scenery ──


def scenery():
    sky = item(
        "sky-box",
        group([
            item("sky", image("sky_day", "cover"), size=(S(1), S(1)), **themed("sky_night")),
            item("rays", shader("Aero Rays", [slot(MID, 0), slot(LEVEL, 1)]), size=(S(1), S(1)),
                 pos=(0, 0, 1)),
            # The sun sits at (0.33, 0.03) of the sky picture: a box from the
            # corner to twice that has its centre there at any window size.
            item("sun", group([
                item("flare", image("flare", "contain"), size=(U(820), U(820)), opacity=0.6, pos=(0, 0, 2),
                     sway={"rotation": (0, 0, 360)}),
            ]), anchor="top_left", size=(S(0.66), S(0.06)), state="app.theme",
                cases=[case("dark", opacity=0.3)]),
        ]),
        name="Sky",
        pos=(0, -40, 0),
        # Wide enough to cover the window at 420 units deep however short it
        # is, with room left for the pan and the music's sway.
        size=(S(1.5), S(1.5)),
        sway={"position": (18, 6, 0)},
    )
    route("sky-box.x", MID, 0.0, 1.0)
    route("sky-box.y", LOW, 0.0, 1.0)
    route("flare.opacity", BEAT, 1.0, 1.65)
    route("flare.scale", BEAT, 1.0, 1.18)
    # The rays turn once in four minutes. A saw on a whole turn wraps where
    # the picture is the same, so it never jumps.
    drift("flare.rz", 240, "saw")

    # The music moves the sky box, so the slow pan rides a box around it.
    # Two minutes there and back across 60 units: felt more than seen. The
    # depth sits on the outer box, since a group paints at its own depth.
    drifting_sky = item("sky-drift", group([sky]), name="Sky drift", pos=(-30, 0, -420), size=(S(1), S(1)),
                        sway={"position": (60, 0, 0)})
    drift("sky-drift.x", 130, "sine", 0.25)

    aurora = item(
        "aurora",
        group([
            item("aurora-a", image("aurora_a"), anchor="top", pos=(S(-0.19), 30, 0), size=(S(0.66), S(0.32)),
                 opacity=0.6, sway={"position": (60, 0, 0)}),
            item("aurora-b", image("aurora_b"), anchor="top", pos=(S(0.22), 150, 20), size=(S(0.58), S(0.27)),
                 opacity=0.5, sway={"position": (-50, 0, 0)}),
        ]),
        name="Aurora",
        pos=(-24, 0, -340),
        size=(S(1.2), S(1.2)),
        sway={"position": (48, 0, 0)},
        state="app.theme",
        cases=[case("light", opacity=0.45), case("dark", opacity=1.0)],
    )
    for ribbon in ("aurora-a", "aurora-b"):
        route(f"{ribbon}.opacity", MID, 1.0, 1.9)
        route(f"{ribbon}.scale", MID, 1.0, 1.07)
        route(f"{ribbon}.x", MID, 0.0, 1.0)
    drift("aurora.x", 70, "sine", 0.6)
    drift("aurora.opacity", 26, "sine", 0.1, 0.7, 1.0)

    hills = [
        item("hills-back", image("hills_back"), anchor="bottom", pos=(0, 110, -230),
             size=(S(1.34), S(0.40)), name="Far hills", **themed("hills_back_night")),
        item("hills-front", image("hills_front"), anchor="bottom", pos=(0, 90, -120),
             size=(S(1.24), S(0.30)), name="Near hills", **themed("hills_front_night")),
    ]

    bokeh = []
    # (id, picture, anchor, home as stage fractions and depth, size, opacity,
    #  wander in units, periods in seconds)
    spots = [
        ("bokeh-1", "bokeh", "top_left", (0.16, 0.19, -260), 120, 0.35, (40, 26), (47, 61)),
        ("bokeh-2", "bokeh_aqua", "top", (0.02, 0.11, -240), 70, 0.45, (30, 20), (39, 53)),
        ("bokeh-3", "bokeh", "top_right", (-0.22, 0.24, -220), 90, 0.3, (36, 24), (57, 43)),
        ("bokeh-4", "bokeh_aqua", "right", (-0.08, -0.06, -200), 54, 0.4, (28, 30), (34, 49)),
        ("bokeh-5", "bokeh", "left", (0.09, 0.04, -250), 64, 0.3, (32, 22), (51, 37)),
        ("ghost-1", "ghost", "top", (-0.06, 0.28, -180), 80, 0.5, (24, 18), (44, 58)),
        ("ghost-2", "ghost", "center", (0.04, 0.13, -160), 46, 0.5, (20, 16), (36, 63)),
    ]
    for n, (id, asset, anchor, (fx, fy, z), size, opacity, (wx, wy), (px, py)) in enumerate(spots):
        bokeh.append(item(id, image(asset, "contain"), anchor=anchor, pos=(S(fx), S(fy), z),
                          size=(U(size), U(size)), opacity=opacity, sway={"position": (wx, wy, 0)}))
        route(f"{id}.opacity", BEAT if id.startswith("ghost") else HIGH, 1.0, 1.8)
        # Two unrelated periods per spot trace a slow loop that never repeats
        # in step with its neighbours.
        drift(f"{id}.x", px, "sine", round(n * 0.37 % 1.0, 3))
        drift(f"{id}.y", py, "sine", round((n * 0.61 + 0.2) % 1.0, 3))

    bubbles = []
    # (id, picture, x as a stage fraction from the middle, depth, size, rise
    #  in units, seconds to rise, phase, wobble in units, music drift)
    floats = [
        ("bubble-1", "bubble_l", -0.44, -60, 150, 1250, 64, 0.05, 34, 36),
        ("bubble-2", "bubble_s", -0.30, -140, 56, 1150, 47, 0.62, 18, -24),
        ("bubble-3", "bubble_m", -0.14, -90, 96, 1200, 55, 0.33, 26, 30),
        ("bubble-4", "bubble_s", 0.01, -30, 44, 1100, 41, 0.81, 14, -18),
        ("bubble-5", "bubble_l", 0.10, -180, 120, 1300, 71, 0.47, 30, 28),
        ("bubble-6", "bubble_m", 0.21, -110, 84, 1250, 58, 0.14, 24, -30),
        ("bubble-7", "bubble_s", 0.33, -70, 38, 1100, 44, 0.71, 12, 20),
        ("bubble-8", "bubble_m", 0.41, -150, 70, 1200, 61, 0.92, 22, -26),
        ("bubble-9", "bubble_s", -0.05, -40, 34, 1050, 38, 0.27, 10, 16),
    ]
    for id, asset, fx, z, size, rise, period, phase, wobble, sway in floats:
        # The rise is a saw on the outer box: up the window, then back to the
        # bottom on the wrap. The fade shares its period and phase, a triangle
        # read four times over so the bubble is solid for the middle three
        # quarters and clear at both ends, which hides the wrap.
        ball = item(f"{id}-ball", image(asset, "contain"), size=(S(1), S(1)), opacity=0.95,
                    sway={"position": (sway, -40, 0)})
        bubbles.append(item(id, group([ball]), anchor="bottom", pos=(S(fx), round(size * 0.6), z),
                            size=(U(size), U(size)), sway={"position": (wobble, -rise, 0)}))
        drift(f"{id}.y", period, "saw", phase)
        drift(f"{id}.opacity", period, "triangle", phase, 0.0, 4.0)
        drift(f"{id}.x", round(period / 6.3, 1), "sine", round(phase * 3.1 % 1.0, 3))
        route(f"{id}-ball.y", LOW, 0.0, 1.0)
        route(f"{id}-ball.x", MID, 0.0, 1.0)
        route(f"{id}-ball.scale", LOW, 1.0, 1.16)

    return [
        drifting_sky,
        aurora,
        item("air", group(bokeh), name="Bokeh", size=(S(1), S(1))),
        *hills,
        item("bubbles", group(bubbles), name="Bubbles", size=(S(1), S(1))),
    ]


# ── Deck: what's playing and the buttons ──


# The click layer's looks: the overlays aero_assets.py draws to the gel's own
# outline, so hovering brightens and pressing sinks without anything moving.
LIT = {"opacity": 1.0, "asset": "gel_lit"}
DOWN = {"opacity": 1.0, "asset": "gel_down"}


def gel_button(id, action, width, side, pos, glyph, *, anchor="center", body="gel_round_aqua", body_state=None,
               glyph_state=None, name=""):
    """A gel, its glyph and a clear cap over both that takes the click. The
    cap carries the command and the looks because a look only reaches the
    item the pointer hits, and only swaps that item's own image: on the
    group it could fade the button, never light it. The body and glyph
    still follow their states under it.

    The box's width is a share of the row, held to `side`, and its height is
    `side`: a narrow deck shrinks the buttons rather than overlapping them,
    and every picture fits the box's shorter side."""
    body_item = item(f"{id}-body", image(body, "contain"), size=(S(1), S(1)), **(body_state or {}))
    glyph_item = item(f"{id}-glyph", image(glyph, "contain"), size=(S(0.6), S(0.6)), pos=(0, 0, 1),
                      **(glyph_state or {}))
    cap = item(f"{id}-cap", image("gel_cap", "contain"), size=(S(1), S(1)), pos=(0, 0, 2), action=action,
               hover=LIT, pressed=DOWN, name=name)
    return item(id, group([body_item, glyph_item, cap]), anchor=anchor, pos=pos, size=(S(width), U(side)),
                max_size=(side, None), name=name)


def deck():
    # The deck's box takes in the pane's shadow, 24 units each side, so the
    # glass shows 24 units in from every edge.
    cover = item(
        "cover-box",
        group([
            item("cover-shadow", image("shadow"), size=(S(0.74), S(0.96)), max_size=(520, 520),
                 pos=(0, 10, -2)),
            # Its width follows the window's width and its height the
            # window's height, and the canvas has no aspect lock, so the box
            # is only square near 16:9. The art letterboxes inside it rather
            # than stretch.
            item("cover", grid(panel("cover art", {
                "theme": {"rounding": 14.0, "surface_opacity": 0.0},
                "source": "playing", "align": "center", "stretch": False,
            })), size=(S(0.66), S(0.86)), max_size=(460, 460)),
            item("cover-gloss", image("cover_gloss"), size=(S(0.66), S(0.86)), max_size=(460, 460),
                 pos=(0, 0, 2)),
        ]),
        anchor="top",
        pos=(0, 48, 0),
        size=(S(1), S(0.46)),
        name="Cover",
    )

    # The seek bar, the buttons and the volume are fixed in units off the
    # bottom of the deck. The lines ride a fraction of its height, which
    # keeps them between the cover and the seek bar from 560 units up.
    line = item("lines", group([
        item("title", text("%title%", 26, "text_bright"), pos=(0, -34, 0), size=(S(0.86), U(38))),
        item("artist", text("%artist%", 19, "text"), pos=(0, 2, 0), size=(S(0.86), U(28))),
        item("album", text("%album% - %year%", 15, "text_dim"), pos=(0, 30, 0), size=(S(0.86), U(24))),
    ]), pos=(0, S(0.09), 0), size=(S(1), U(104)), name="Lines")

    seek = item("seek", grid(panel("seek", {
        "theme": {"surface_opacity": 0.0},
        "show_total": False, "thickness": 6.0, "rounding": 6.0, "playhead_max": 16.0,
        "items": ["elapsed", "strip", "ending"],
    })), anchor="bottom", pos=(0, -172, 0), size=(S(0.8), U(30)))

    playing = dict(state="player.playback", cases=[case("playing", asset="gel_round_green")])
    pause = dict(state="player.playback", cases=[case("playing", asset="glyph_pause")])
    shuffle_lit = dict(state="player.shuffle_on", cases=[case("on", asset="gel_round_green")])
    repeat_lit = dict(state="player.repeat",
                      cases=[case("all", asset="gel_round_green"), case("one", asset="gel_round_green")])
    repeat_one = dict(state="player.repeat", cases=[case("one", asset="glyph_repeat1")])

    buttons = item(
        "buttons",
        group([
            gel_button("shuffle", "toggle_shuffle", 0.14, 44, (0, 4, 0), "glyph_shuffle", anchor="left",
                       body="gel_round_clear", body_state=shuffle_lit, name="Shuffle"),
            gel_button("previous", "previous_track", 0.17, 54, (S(-0.25), 2, 0), "glyph_prev", name="Previous"),
            gel_button("play", "toggle_playback", 0.26, 84, (0, 0, 0), "glyph_play", body_state=playing,
                       glyph_state=pause, name="Play"),
            gel_button("next", "next_track", 0.17, 54, (S(0.25), 2, 0), "glyph_next", name="Next"),
            gel_button("repeat", "cycle_loop", 0.14, 44, (0, 4, 0), "glyph_repeat", anchor="right",
                       body="gel_round_clear", body_state=repeat_lit, glyph_state=repeat_one, name="Repeat"),
        ]),
        anchor="bottom",
        pos=(0, -74, 0),
        size=(S(0.8), U(92)),
        max_size=(400, None),
        name="Buttons",
    )
    route("play.scale", BEAT, 1.0, 1.06)

    volume = item("volume", grid(panel("volume", {
        "theme": {"surface_opacity": 0.0},
        "align": "center", "stretch": True, "items": ["icon", "slider", "percent"],
    })), anchor="bottom", pos=(0, -40, 0), size=(S(0.72), U(28)), max_size=(420, None))

    droplet = item("droplet", image("droplet", "contain"), anchor="top_right", pos=(-50, 54, 4),
                   size=(U(30), U(40)), opacity=0.9, state="player.playback",
                   cases=[case("playing", visible=False)], name="Paused drop")

    frame = item("deck-frame", sliced("glass_pane"), size=(S(1), S(1)), pos=(0, 0, -4),
                 **themed("glass_pane_night"))

    return item("deck", group([frame, cover, line, seek, buttons, volume, droplet]), anchor="left",
                pos=(S(0.018), 8, 0), size=(S(0.31), S(0.90)), min_size=(290, 560), name="Deck")


# ── Sheet: library, queue and playlists, the spectrum and the screen ──


def sheet():
    # Like the deck, the box takes in the pane's 24-unit shadow.
    frame = item("sheet-frame", sliced("glass_pane"), size=(S(1), S(1)), pos=(0, 0, -4),
                 **themed("glass_pane_night"))
    shimmer = item("shimmer", shader("Aero Shimmer", [slot(HIGH, 0), slot(LOW, 1)]), size=(S(1), S(1)),
                   pos=(0, 0, -3))

    library = item("browser", grid(
        panel("library", {
            "theme": {"surface_opacity": 0.0},
            "density": "compact", "headers": "expanded", "group_by": "album", "row_height": 26.0,
            "follow_playing": True, "smooth_follow": True, "art_rounding": 8.0,
            "header_art": True, "header_year": True, "header_details": True,
        }),
        panel("queue", {"theme": {"surface_opacity": 0.0}, "headers": "off",
                        "columns": ["number", "cover", "name", "artist", "duration"]}),
        panel("playlists", {"theme": {"surface_opacity": 0.0}, "headers": "off",
                            "columns": ["cover", "number", "name", "artist", "rating", "favourite"]}),
    ), anchor="top", pos=(0, 48, 0), size=(S(0.9), S(0.55)), name="Library")

    well = item("well", group([
        item("well-frame", sliced("glass_well"), size=(S(1), S(1)), **themed("glass_well_night")),
        item("spectrum", grid(panel("spectrum", {
            "theme": {"surface_opacity": 0.0},
            "style": "bars", "orientation": "bottom", "symmetry": "none",
            "freq_lo": 30.0, "freq_hi": 16000.0, "bar_width": 6.0, "bar_gap": 3.0,
            "fft_size": 8192, "gradient": "custom", "gradient_lo": "#2fd06a", "gradient_hi": "#2fb8ff",
            "gradient_curve": 2.0, "caps": True, "freeze": False, "cap_gravity": 0.05, "labels": False,
        })), size=(S(0.95), S(0.84))),
    ]), anchor="left", size=(S(0.62), S(1)), name="Spectrum")

    screen = item("screen", group([
        item("screen-frame", sliced("glass_screen"), size=(S(1.07), S(1.1)), opacity=0.8,
             **themed("glass_screen_night")),
        # 1.4 to 1, the pane's shape at 1920x1080, so a renderer's picture isn't squeezed. It
        # doesn't rest with the music: a renderer says so itself if it wants to.
        item("renderer", {"kind": "target", "owner": "", "width": 560, "height": 400, "rests": False,
                          "config": None}, size=(S(0.93), S(0.88)), pos=(0, 0, 1), name="Render target"),
        item("screen-gloss", image("cover_gloss"), size=(S(0.95), S(0.92)), pos=(0, 0, 2), opacity=0.55,
             state="app.theme", cases=[case("dark", opacity=0.4)]),
    ]), anchor="right", pos=(-16, 0, 24), size=(S(0.35), S(0.96)), rot=(-1.5, -12, 0),
        sway={"rotation": (3, 4, 0)}, name="Screen")
    route("screen.ry", MID, 0.0, 1.0)
    # A slow nod, as if the pane hung in moving air.
    drift("screen.rx", 41, "sine", 0.4, curve="ease_in_out")

    lower = item("lower", group([well, screen]), anchor="bottom", pos=(0, -44, 0),
                 size=(S(0.9), S(0.29)), min_size=(None, 180), name="Lower")

    return item("sheet", group([frame, shimmer, library, lower]), anchor="right",
                pos=(S(-0.018), 8, 0), size=(S(0.635), S(0.90)), min_size=(540, 560), name="Sheet")


def shard():
    """A loose pane of glass leaning out from behind the sheet's top-left
    corner. It sits on the stage behind both panes, so a narrow window that
    brings the sheet up against the deck tucks it away instead of laying it
    over the cover."""
    out = item("shard", image("glass_shard"), anchor="top_left", pos=(S(0.30), S(0.005), -40),
               size=(S(0.216), S(0.306)), rot=(10, -24, -9), opacity=0.75, sway={"rotation": (0, 5, 4)},
               name="Glass shard")
    route("shard.ry", MID, 0.0, 1.0)
    drift("shard.rz", 37, "sine", 0.8, curve="ease_in_out")
    return out


def chrome():
    controls = item("window", grid(panel("window controls", {
        "theme": {"surface_opacity": 0.0},
        "anchor": True, "style": "icons", "items": ["minimize", "maximize", "close"],
        "pin_mini_only": False, "align": "right",
    })), anchor="top_right", pos=(-12, 8, 0), size=(U(112), U(28)), name="Window controls")
    handle = item("handle", grid(panel("drag anchor", {"theme": {"surface_opacity": 0.0}, "align": "center"})),
                  anchor="top", pos=(0, 6, 0), size=(S(0.3), U(30)), name="Drag handle")
    return [handle, controls]


# ── Shaders ──


RAYS = """// Aero: soft rays fanning down from the sun at the top of the sky, turning
// slowly while music plays and brighter with the mids. At night they thin
// to a cool haze. Pure uniforms, so it stays on the cheap in-scene path.
//
// @overlay
// @slot 0: mids
// @slot 1: level

fn fs_user(uv: vec2<f32>) -> vec4<f32> {
    let mids = params.signals[0].x;
    let level = params.signals[0].y;
    let light = params.user_meta[1].y;

    let aspect = max(params.resolution.x, 1.0) / max(params.resolution.y, 1.0);
    let d = (uv - vec2<f32>(0.33, 0.03)) * vec2<f32>(aspect, 1.0);
    let r = length(d);
    let a = atan2(d.x, d.y);
    let t = params.time * 0.02;

    // Two fans at different counts beating against each other, so the rays
    // break up instead of reading as a pinwheel.
    var rays = 0.5 + 0.5 * sin(a * 11.0 + t * 6.2831);
    rays = rays * (0.55 + 0.45 * sin(a * 27.0 - t * 9.0 + 1.3));
    rays = rays * rays * rays;

    let fall = exp(-r * 1.4) * smoothstep(0.02, 0.12, r);
    let strength = rays * fall * (0.07 + 0.16 * mids + 0.06 * level) * mix(0.3, 1.0, light);
    let tint = mix(vec3<f32>(0.55, 0.85, 1.0), vec3<f32>(1.0, 0.98, 0.9), light);
    return vec4<f32>(tint * strength, 0.0);
}
"""

SHIMMER = """// Aero: caustic light drifting across a glass sheet, the web water throws
// on a pane. The highs sharpen it and the bass brightens it. Added light
// only, so it never darkens what's on the glass.
//
// @overlay
// @slot 0: highs
// @slot 1: bass

fn fs_user(uv: vec2<f32>) -> vec4<f32> {
    let highs = params.signals[0].x;
    let bass = params.signals[0].y;
    let light = params.user_meta[1].y;

    let aspect = max(params.resolution.x, 1.0) / max(params.resolution.y, 1.0);
    let p = uv * vec2<f32>(aspect, 1.0) * 4.0;
    let t = params.time * 0.12;

    let warp = vec2<f32>(sin(p.y * 1.3 + t), cos(p.x * 1.1 - t * 0.8)) * 0.7;
    let q = p + warp;
    let wave = sin(q.x * 2.1 + t) * sin(q.y * 1.9 - t * 1.3);
    let web = pow(1.0 - abs(wave), 10.0 + 10.0 * highs);

    // Strongest near the top, where the gloss already says light comes in.
    let fade = (1.0 - smoothstep(0.0, 0.9, uv.y)) * smoothstep(0.0, 0.04, uv.x)
        * smoothstep(0.0, 0.04, 1.0 - uv.x) * smoothstep(0.0, 0.03, uv.y);
    let strength = web * fade * (0.035 + 0.07 * bass);
    let tint = mix(vec3<f32>(0.45, 0.9, 1.0), vec3<f32>(1.0, 1.0, 1.0), light);
    return vec4<f32>(tint * strength, 0.0);
}
"""


# ── Bundle ──


def assets_table():
    out = []
    for file in sorted(os.listdir(ASSETS)):
        if not file.endswith(".png"):
            continue
        data = open(os.path.join(ASSETS, file), "rb").read()
        out.append({
            "id": file[:-4],
            "file": file,
            "kind": "image",
            "sha256": hashlib.sha256(data).hexdigest(),
            "bytes": len(data),
        })
    return out


def signals():
    def band(id, lo, hi, smooth, threshold=0.0):
        return {"id": id, "name": "", "source": {"kind": "band", "lo": lo, "hi": hi},
                "smooth": smooth, "threshold": threshold, "reset_on_track": False}

    return [
        band(LOW, 30.0, 150.0, 0.45, 0.1),
        band(MID, 250.0, 2000.0, 0.7),
        band(HIGH, 4000.0, 12000.0, 0.45, 0.05),
        {"id": LEVEL, "name": "", "source": {"kind": "level"}, "smooth": 0.6, "threshold": 0.0,
         "reset_on_track": False},
        {"id": BEAT, "name": "", "source": {"kind": "beat", "lo": 40.0, "hi": 160.0}, "smooth": 0.55,
         "threshold": 0.0, "reset_on_track": False},
    ]


LIGHT = {
    "accent": "#0f9fd8", "accent_hover": "#0b86b8", "bg_control": "#e6f4fc",
    "bg_control_active": "#cfe8f7", "bg_control_hover": "#d9eefa", "bg_elevated": "#f2faff",
    "bg_header": "#e8f5fd", "bg_input": "#ffffff", "bg_menu": "#f4fbff", "bg_menu_hover": "#d6eefb",
    "bg_menubar": "#eaf6fd", "bg_panel": "#eef8fe", "bg_root": "#dff1fb", "bg_toolbar": "#e8f5fd",
    "border": "#9cc8e4", "border_light": "#b9daee", "gridline": "#c4e0f1", "highlight": "#c9ecff",
    "text": "#0d2c48", "text_bright": "#04182c", "text_dim": "#36617f", "text_faint": "#6f94b0",
    "text_muted": "#4d7391", "text_on_accent": "#ffffff", "text_secondary": "#1d4466",
}

DARK = {
    "accent": "#45d4ff", "accent_hover": "#7be2ff", "bg_control": "#12304f",
    "bg_control_active": "#1b416a", "bg_control_hover": "#173a5f", "bg_elevated": "#0f2a47",
    "bg_header": "#0d2440", "bg_input": "#081a30", "bg_menu": "#0e2643", "bg_menu_hover": "#1a3d63",
    "bg_menubar": "#0a1f38", "bg_panel": "#0b2139", "bg_root": "#071a30", "bg_toolbar": "#0d2440",
    "border": "#2b5b85", "border_light": "#3b6d98", "gridline": "#1d4469", "highlight": "#1f4e7a",
    "text": "#e4f4ff", "text_bright": "#ffffff", "text_dim": "#a7cbe6", "text_faint": "#6c93b4",
    "text_muted": "#86acca", "text_on_accent": "#03203a", "text_secondary": "#c6e2f6",
}


def bundle():
    ROUTES.clear()
    items = [*scenery(), shard(), deck(), sheet(), *chrome()]
    canvas = {
        "version": 1,
        "stage": {"width": 1920.0, "height": 1080.0, "fit": "anchor"},
        "camera": {"fov": 35.0, "parallax": 22.0},
        "items": items,
        "routes": list(ROUTES),
    }
    dump = {
        "version": 1,
        "center": {
            "panel_name": "StackPanel",
            "children": [tabs(panel("canvas", canvas))],
            "info": {"stack": {"sizes": [], "axis": 1}},
        },
    }
    return {
        "version": 2,
        "name": "Aero",
        "layouts": [{"name": "Aero", "dump": dump}],
        "primary_layout": "Aero",
        "palette_dark": DARK,
        "palette_light": LIGHT,
        "signals": signals(),
        "shaders": [
            {"name": "Aero Rays", "source": RAYS},
            {"name": "Aero Shimmer", "source": SHIMMER},
        ],
        "assets": assets_table(),
        "meta": {
            "author": "zealsprince",
            "description": "A sunny sky-blue morning drawn on one canvas: glass panels over clouds and "
                           "green hills, gel buttons that light up under the pointer, bubbles drifting up "
                           "past the glass, an aurora that breathes with the mids and a sun that flares on "
                           "the beat. Dark mode turns it to night.",
            "created": "2026-11-26",
            "updated": "2026-11-26",
        },
        "appearance": {
            "surface_opacity": 0.0,
            "backdrop_strength": 1.0,
            "frame": {"margin": 0.0, "padding": 2.0, "rounding": 10.0, "border": 0.0},
            "seams": False,
            "keep_theme": True,
            "art_theming": False,
            "hide_menubar": True,
            "os_decorations": False,
        },
    }


def main():
    out = os.path.join(SHIPPED, "Aero.json")
    with open(out, "w") as f:
        json.dump(bundle(), f, indent=2)
        f.write("\n")
    print(out)


if __name__ == "__main__":
    main()
