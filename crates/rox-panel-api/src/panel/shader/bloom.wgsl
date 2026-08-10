// rox preset: Bloom.
//
// The pass chain in miniature. `// @pass` splits this one text into two
// stages: everything above the first cut is a prelude both share, the glow
// pass renders the scene bright at half size, and the out pass samples it
// back by its name, where the bilinear read of a half-size target is half
// the blur for free. Chains always run on the region pass, whatever they
// sample.
//
// @slot 0: bass
// @slot 1: pace
// @slot 2: hue
// @slot 3: lift

fn bloom_tint(t: f32) -> vec3<f32> {
    let phase = vec3<f32>(0.0, 0.33, 0.67);
    return 0.5 + 0.5 * cos(6.28318530718 * (t + phase));
}

// The scene both passes agree on: three orbs walking lissajous paths in
// aspect-true space, sized by the bass. Incommensurate rates per orb, so
// the trio never lines up twice.
fn bloom_orbs(p: vec2<f32>, time: f32, bass: f32, hue: f32) -> vec3<f32> {
    var light = vec3<f32>(0.0);
    for (var i = 0u; i < 3u; i = i + 1u) {
        let f = f32(i);
        let seat = vec2<f32>(
            0.36 * sin(time * (0.50 + 0.13 * f) + f * 2.1),
            0.30 * sin(time * (0.73 + 0.11 * f) + f * 4.2),
        );
        let reach = 0.018 + 0.040 * bass;
        let d = length(p - seat) / reach;
        light = light + bloom_tint(hue + f * 0.33) * exp(-d * d);
    }
    return light;
}

// @pass glow: 0.5

fn fs_user(uv: vec2<f32>) -> vec4<f32> {
    let bass = clamp(params.signals[0].x, 0.0, 1.0);
    let pace = clamp(params.signals[0].y, 0.0, 1.0);
    let hue = params.signals[0].z;

    let res = max(params.resolution, vec2<f32>(1.0));
    let p = (uv - 0.5) * vec2<f32>(res.x / res.y, 1.0);
    let light = bloom_orbs(p, params.time * (0.4 + 1.1 * pace), bass, hue);
    return vec4<f32>(light, 1.0);
}

// @pass out

fn fs_user(uv: vec2<f32>) -> vec4<f32> {
    let bass = clamp(params.signals[0].x, 0.0, 1.0);
    let pace = clamp(params.signals[0].y, 0.0, 1.0);
    let hue = params.signals[0].z;
    let lift = clamp(params.signals[0].w, 0.0, 1.0);

    let res = max(params.resolution, vec2<f32>(1.0));
    let p = (uv - 0.5) * vec2<f32>(res.x / res.y, 1.0);
    let sharp = bloom_orbs(p, params.time * (0.4 + 1.1 * pace), bass, hue);

    // Eight taps in a ring off the glow pass, widening what its half size
    // already softened.
    var haze = vec3<f32>(0.0);
    for (var i = 0u; i < 8u; i = i + 1u) {
        let a = f32(i) * 0.78539816;
        let off = vec2<f32>(cos(a), sin(a)) * 0.02;
        haze = haze + textureSampleLevel(glow, samp, uv + off, 0.0).rgb;
    }
    haze = haze / 8.0;

    let rgb = vec3<f32>(0.01, 0.01, 0.02) + sharp + haze * (0.6 + 1.2 * lift);
    return vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
