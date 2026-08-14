// rox preset: Shadow.
//
// A drop shadow cast by the panel's own content. The `mask` binding is the
// body's draws isolated on transparency, so the shadow hugs glyphs, icons
// and controls exactly, whatever the backdrop looks like: the soft pass
// blurs that coverage at half size, and the out pass lays it down dark,
// shifted, and cut away under the content itself, so the ink stays exactly
// as bright as it was drawn.
//
// Output is premultiplied, and most of the rect stays transparent: this is
// an overlay in the same sense Sheen is, riding a panel (or a whole group)
// that already draws.
//
// @overlay
// @slot 0: soften
// @slot 1: strength
// @slot 2: drop
// @slot 3: lift

// A knob resting at zero means "unrouted, unset", so each one has a spot
// the shadow defaults to rather than vanishing.
fn shadow_knob(value: f32, rest: f32) -> f32 {
    return mix(rest, value, step(0.001, value));
}

// @pass soft: 0.5

fn fs_user(uv: vec2<f32>) -> vec4<f32> {
    let soften = shadow_knob(params.signals[0].x, 0.35);
    // Tap spread in uv, off the pass's own half resolution: the half-size
    // target and its bilinear reads are the inner half of the blur.
    let res = max(params.resolution, vec2<f32>(1.0));
    let reach = vec2<f32>(soften * 12.0) / res;

    var cover = textureSampleLevel(mask, samp, uv, 0.0).a;
    var weight = 1.0;
    for (var i = 0u; i < 8u; i = i + 1u) {
        let a = f32(i) * 0.78539816;
        let dir = vec2<f32>(cos(a), sin(a));
        cover = cover + textureSampleLevel(mask, samp, uv + dir * reach * 0.5, 0.0).a * 0.75;
        cover = cover + textureSampleLevel(mask, samp, uv + dir * reach, 0.0).a * 0.35;
        weight = weight + 1.1;
    }
    return vec4<f32>(0.0, 0.0, 0.0, cover / weight);
}

// @pass out

fn fs_user(uv: vec2<f32>) -> vec4<f32> {
    let strength = shadow_knob(params.signals[0].y, 0.55);
    let drop = shadow_knob(params.signals[0].z, 0.18);
    let lift = clamp(params.signals[0].w, 0.0, 1.0);

    // The shadow sits below its caster, so its coverage is read from above
    // the pixel in hand.
    let res = max(params.resolution, vec2<f32>(1.0));
    let offset = vec2<f32>(0.0, drop * 14.0) / res;
    let spread = textureSampleLevel(soft, samp, uv - offset, 0.0).a;

    // The cutout: this pixel's own ink wins over any shadow behind it, and
    // an anti-aliased edge wins by exactly its coverage.
    let ink = textureSampleLevel(mask, samp, uv, 0.0).a;
    let shade = smoothstep(0.0, 0.85, spread) * strength * (1.0 - ink);

    // Black shadow at rest; the lift walks it toward white for the looks
    // where a dark backdrop wants a halo instead.
    let tone = vec3<f32>(lift);
    return vec4<f32>(tone * shade, shade);
}
