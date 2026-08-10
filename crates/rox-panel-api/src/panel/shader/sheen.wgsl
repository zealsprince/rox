// rox preset: Sheen.
//
// An overlay, not a scene: most of the rect stays transparent, so it only
// makes sense riding a panel that already draws something - the art shelf
// is the panel it was tuned on. A soft vignette pools in the corners, and
// a slow diagonal gleam sweeps light across, lifted by whatever drives
// slot 0. Like every rox surface it reads nothing but its uniforms, so it
// stays on the cheap in-scene path.
//
// Output is premultiplied, which is what lets one pass both darken and
// lighten: the vignette rides the alpha as scrim, and the gleam rides the
// color at alpha zero, which composites as pure added light.
//
// @overlay
// @slot 0: bass
// @slot 1: gleam
// @slot 2: shade
// @slot 3: sweep

fn fs_user(uv: vec2<f32>) -> vec4<f32> {
    let bass = params.signals[0].x;
    let gleam = params.signals[0].y;
    let shade = params.signals[0].z;
    let phase = params.signals[0].w;
    let volume = params.user_meta[0].x;

    // Square the space off the panel's aspect, so the vignette pools in
    // the corners instead of hugging the short edges.
    let aspect = max(params.resolution.x, 1.0) / max(params.resolution.y, 1.0);
    let p = (uv - vec2<f32>(0.5, 0.5)) * vec2<f32>(aspect, 1.0);
    let reach = max(length(vec2<f32>(aspect, 1.0)) * 0.5, 0.001);

    // The vignette: nothing over the middle, deepening past it. Slot 2
    // widens the pool; unrouted it rests at a light frame.
    let r = length(p) / reach;
    let vig = smoothstep(0.55, 1.05, r) * (0.28 + 0.35 * shade);

    // The gleam: a soft diagonal band drifting across and wrapping, warm
    // white so it reads as light rather than haze. The bass lifts it, the
    // volume keeps a quiet player's sweep from flashing over silence, and
    // slot 3 walks the band by hand when a route drives it.
    let along = dot(uv, vec2<f32>(0.66, 0.34));
    let sweep = fract(params.time * 0.04 + phase) * 1.6 - 0.3;
    let off = (along - sweep) / 0.09;
    let band = exp(-off * off);
    let lift = (0.04 + 0.10 * bass + 0.06 * gleam) * (0.35 + 0.65 * clamp(volume, 0.0, 1.0));
    let light = vec3<f32>(1.0, 0.97, 0.90) * band * lift;

    // Premultiplied compose, gleam under scrim: the added light dims where
    // the vignette stands, so the corners stay corners.
    return vec4<f32>(light * (1.0 - vig), vig);
}
