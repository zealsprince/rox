// rox preset: Trails.
//
// Reads `prev`, its own output from the last frame, which is what puts it on
// the region pass: gpui hands it a persistent feedback texture and rox paints
// it through paint_screen_shader instead of the plain quad. Resizing the
// panel clears that texture, so the smear restarts.
//
// It deliberately doesn't sample `screen`. The region pass replaces its rect
// outright, and feeding the panel's own background back in through `prev`
// would compound it frame after frame until the whole rect went white. This
// preset owns its rect; a surface shader riding some other panel's body is
// where blending `screen` back in belongs.
//
// @slot 0: bass
// @slot 1: swirl
// @slot 2: fade
// @slot 3: hue

fn trails_spin(v: vec2<f32>, angle: f32) -> vec2<f32> {
    let c = cos(angle);
    let s = sin(angle);
    return vec2<f32>(v.x * c - v.y * s, v.x * s + v.y * c);
}

fn trails_tint(t: f32) -> vec3<f32> {
    let phase = vec3<f32>(0.0, 0.33, 0.67);
    return 0.5 + 0.5 * cos(6.28318530718 * (t + phase));
}

fn fs_user(uv: vec2<f32>) -> vec4<f32> {
    let bass = params.signals[0].x;
    let swirl = params.signals[0].y;
    let fade = params.signals[0].z;
    let hue = params.signals[0].w;
    // Everything below is per-second, scaled by the frame step, so the smear
    // runs the same at 60 and at 144. The cap matches the uniform block's own,
    // which keeps a stalled frame from jumping the whole trail at once.
    let step = clamp(params.delta, 0.0, 0.1);

    // Last frame, pulled in toward the center and turned a little. That drift
    // is the whole trick: a still image sampled off its own slightly-moved
    // self smears along the direction it moved.
    let center = vec2<f32>(0.5, 0.5);
    let pull = 1.0 - (0.15 + 0.60 * bass) * step;
    let spin = (0.20 + 2.00 * swirl) * step;
    let source = center + trails_spin((uv - center) * pull, spin);
    let carried = textureSample(prev, samp, clamp(source, vec2<f32>(0.0), vec2<f32>(1.0)));
    let keep = max(1.0 - (2.0 + 6.0 * fade) * step, 0.0);

    // The head: a blob walking a lissajous path, lit by whatever drives slot
    // 0. Two incommensurate rates, so the path never closes into a loop.
    let aspect = vec2<f32>(max(params.resolution.x, 1.0) / max(params.resolution.y, 1.0), 1.0);
    let head = center
        + vec2<f32>(0.34 * sin(params.time * 0.70), 0.28 * sin(params.time * 1.10 + 1.20));
    let reach = 0.020 + 0.060 * bass;
    let falloff = length((uv - head) * aspect) / max(reach, 0.001);
    let blob = exp(-falloff * falloff);

    let tint = trails_tint(hue + params.time * 0.05);
    let lit = carried.rgb * keep + tint * blob * (0.35 + 0.65 * bass);
    // The clamp is load-bearing: an additive trail against a decay under 1
    // has a fixed point well above white, so without it the head would run
    // away instead of settling into a bright core.
    return vec4<f32>(clamp(lit, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
