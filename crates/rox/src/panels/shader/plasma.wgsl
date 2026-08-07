// rox preset: Plasma.
//
// A pure primitive. It reads nothing but its own uniforms, so registration
// classifies it as an in-scene quad and it draws inside the main pass with
// no screen copy and no feedback buffer - the cheap path.
//
// Every slot is optional: unrouted ones read zero and the shader still
// moves, because `user_meta` carries volume and track position whatever the
// routing looks like.
//
// @slot 0: bass
// @slot 1: mids
// @slot 2: highs
// @slot 3: sweep

// The cosine palette every color in here comes out of. Feeding it a running
// phase is what makes the bands travel rather than sit.
fn plasma_tint(t: f32) -> vec3<f32> {
    let phase = vec3<f32>(0.0, 0.33, 0.67);
    return 0.5 + 0.5 * cos(6.28318530718 * (t + phase));
}

fn fs_user(uv: vec2<f32>) -> vec4<f32> {
    let bass = params.signals[0].x;
    let mids = params.signals[0].y;
    let highs = params.signals[0].z;
    let sweep = params.signals[0].w;
    // rox always fills these: volume, then how far into the track we are.
    let volume = params.user_meta[0].x;
    let progress = params.user_meta[0].y;

    // Square the space off the panel's aspect, or the rings go oval on a
    // wide panel.
    let aspect = max(params.resolution.x, 1.0) / max(params.resolution.y, 1.0);
    let p = (uv - vec2<f32>(0.5, 0.5)) * vec2<f32>(aspect, 1.0) * 3.0;
    let r = length(p);
    let t = params.time * (0.25 + 0.75 * mids) + sweep * 6.28318530718;

    // Three plane waves and one ring, the plain way to a plasma. The
    // signals bend their frequencies, so a kick widens the rings and the
    // highs tighten the vertical banding.
    var v = sin(p.x * (2.0 + 4.0 * bass) + t);
    v = v + sin(p.y * (2.5 + 3.0 * highs) - t * 0.8);
    v = v + sin((p.x + p.y) * 1.7 + t * 0.6);
    v = v + sin(r * (4.0 + 8.0 * bass) - t * 1.4);
    v = v * 0.25;

    let tint = plasma_tint(v * 0.5 + 0.5 + progress * 0.2);
    let lift = 0.30 + 0.70 * clamp(bass + volume * 0.25, 0.0, 1.0);
    let vignette = 1.0 - 0.55 * clamp(r * 0.42, 0.0, 1.0);
    let rgb = clamp(tint * lift * vignette, vec3<f32>(0.0), vec3<f32>(1.0));
    // Opaque, and output is premultiplied: with alpha at 1 the two agree.
    return vec4<f32>(rgb, 1.0);
}
