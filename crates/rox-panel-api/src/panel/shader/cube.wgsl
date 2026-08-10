// rox preset: Cube.
//
// Fake 3D on a fragment stage: eight corners rotated and perspective
// projected by hand, then the twelve edges drawn as distance-to-segment
// glow, nearer ones brighter. Pure uniforms, so it stays an in-scene
// quad, and the lines land as added light at alpha zero, so it can ride a
// panel that draws or stand alone over the panel's own dark.
//
// @overlay
// @slot 0: bass
// @slot 1: spin
// @slot 2: hue
// @slot 3: glow

fn cube_tint(t: f32) -> vec3<f32> {
    let phase = vec3<f32>(0.0, 0.33, 0.67);
    return 0.5 + 0.5 * cos(6.28318530718 * (t + phase));
}

fn cube_seg(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let ab = b - a;
    let t = clamp(dot(p - a, ab) / max(dot(ab, ab), 1e-6), 0.0, 1.0);
    return length(p - a - ab * t);
}

fn fs_user(uv: vec2<f32>) -> vec4<f32> {
    let bass = clamp(params.signals[0].x, 0.0, 1.0);
    let spin = clamp(params.signals[0].y, 0.0, 1.0);
    let hue = params.signals[0].z;
    let glow = clamp(params.signals[0].w, 0.0, 1.0);

    let res = max(params.resolution, vec2<f32>(1.0));
    let p = (uv - 0.5) * vec2<f32>(res.x / res.y, 1.0);

    // Two incommensurate rates, so the tumble never settles into a loop;
    // the spin slot leans the whole thing faster.
    let t = params.time * (0.5 + 1.5 * spin);
    let ax = t * 0.79;
    let ay = t * 0.51;
    let cx = cos(ax);
    let sx = sin(ax);
    let cy = cos(ay);
    let sy = sin(ay);

    // The corners off their index bits, turned around x then y, pushed
    // back from the camera, divided by depth. The bass punches the whole
    // cube a size up.
    let scale = 0.30 * (1.0 + 0.20 * bass);
    var flat: array<vec2<f32>, 8>;
    var depth: array<f32, 8>;
    for (var i = 0u; i < 8u; i = i + 1u) {
        var v = vec3<f32>(
            f32(i & 1u) * 2.0 - 1.0,
            f32((i >> 1u) & 1u) * 2.0 - 1.0,
            f32((i >> 2u) & 1u) * 2.0 - 1.0,
        );
        v = vec3<f32>(v.x, v.y * cx - v.z * sx, v.y * sx + v.z * cx);
        v = vec3<f32>(v.x * cy + v.z * sy, v.y, -v.x * sy + v.z * cy);
        let z = v.z + 3.4;
        flat[i] = v.xy * (scale * 3.4 / z);
        depth[i] = z;
    }

    // Twelve edges without a table: every corner connects to the neighbor
    // one axis bit up, and only the corner with the bit clear draws it.
    var lit = 0.0;
    let width = 0.006 + 0.010 * glow;
    for (var i = 0u; i < 8u; i = i + 1u) {
        for (var axis = 0u; axis < 3u; axis = axis + 1u) {
            let n = i | (1u << axis);
            if (n == i) {
                continue;
            }
            let d = cube_seg(p, flat[i], flat[n]);
            let near = 1.0 - 0.35 * clamp((depth[i] + depth[n]) * 0.5 - 2.4, 0.0, 2.0);
            lit = lit + exp(-(d * d) / (width * width)) * near;
        }
    }

    let light = cube_tint(hue + params.time * 0.03) * min(lit, 1.3) * (0.6 + 0.4 * bass);
    return vec4<f32>(min(light, vec3<f32>(1.0)), 0.0);
}
