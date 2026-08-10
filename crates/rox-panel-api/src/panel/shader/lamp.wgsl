// rox preset: Lamp.
//
// A light that follows the cursor: `params.mouse` carries the pointer in
// the same device pixels as `resolution`, free to sit outside the rect,
// with the primary and secondary buttons in zw. Pressing the primary
// focuses the beam, the secondary spreads it. Transparent overlay like
// Sheen, so it rides a panel that already draws, and pure uniforms, so it
// stays on the cheap in-scene path.
//
// Premultiplied compose: the shade rides the alpha as scrim, the light
// rides the color at alpha zero, which composites as pure added light.
//
// @overlay
// @slot 0: bass
// @slot 1: reach
// @slot 2: warmth
// @slot 3: shade

fn fs_user(uv: vec2<f32>) -> vec4<f32> {
    let bass = clamp(params.signals[0].x, 0.0, 1.0);
    let reach = clamp(params.signals[0].y, 0.0, 1.0);
    let warmth = clamp(params.signals[0].z, 0.0, 1.0);
    let shade = clamp(params.signals[0].w, 0.0, 1.0);

    // The cursor in uv space, aspect-corrected so the pool stays round. A
    // pointer off in some other panel just pulls the light off the edge.
    let res = max(params.resolution, vec2<f32>(1.0));
    let aspect = vec2<f32>(res.x / res.y, 1.0);
    let cursor = params.mouse.xy / res;
    let d = length((uv - cursor) * aspect);

    let press = params.mouse.z;
    let spread = params.mouse.w;
    let radius = (0.20 + 0.28 * reach + 0.08 * bass) * (1.0 - 0.35 * press)
        * (1.0 + 0.8 * spread);
    let beam = exp(-(d * d) / max(radius * radius, 1e-5));

    // Cool by default, warmed by slot 2; the press feeds the beam more
    // than the bass does, so a click reads as turning the lamp up.
    let tint = mix(vec3<f32>(0.85, 0.92, 1.0), vec3<f32>(1.0, 0.86, 0.62), warmth);
    let light = tint * beam * (0.10 + 0.18 * press + 0.08 * bass);

    // The scrim: everything past the pool sinks, deeper as the shade slot
    // comes up, so the light reads as light rather than fog.
    let scrim = (0.12 + 0.55 * shade) * (1.0 - beam);
    return vec4<f32>(light * (1.0 - scrim), scrim);
}
