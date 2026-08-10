// rox preset: Tube.
//
// Reads the panel under it back through a curved CRT face. `screen` is the
// composed frame inside this shader's own rect, which is what puts the
// program on the region pass: barrel warp normalized to fill the rect, so
// the picture stays where the controls under it sit, scanlines off the
// device rows, and a little color fringe. It reads `screen` and never
// `prev`, so nothing compounds frame over frame the way feeding a panel
// back into itself would.
//
// The slots are the face's dimensions, not a response: nothing here reads
// the music unless you route it that way, and there's no level slot for it
// to read with. Hand-set them and the tube sits where you put it. Route
// them and it moves, which is a choice rather than the default the way a
// bass-lift built into the source would be.
//
// Every slot rests at a mild tube, so untouched it's still a look.
//
// @overlay
// @slot 0: bend
// @slot 1: lines
// @slot 2: fringe
// @slot 3: rim

// Fill-normalized barrel: the corner maps exactly to the corner, so the
// face fills the rect with no dead border and the picture stays close to
// where the real controls under it sit. dot(c, c) at a corner is 0.5,
// which is the normalizer.
fn tube_warp(uv: vec2<f32>, bend: f32) -> vec2<f32> {
    let c = uv - 0.5;
    return 0.5 + c * ((1.0 + bend * dot(c, c)) / (1.0 + bend * 0.5));
}

fn fs_user(uv: vec2<f32>) -> vec4<f32> {
    let bend = 0.10 + 0.35 * clamp(params.signals[0].x, 0.0, 1.0);
    let lines = 0.10 + 0.30 * clamp(params.signals[0].y, 0.0, 1.0);
    let rim = 0.30 + 0.40 * clamp(params.signals[0].w, 0.0, 1.0);
    let res = max(params.resolution, vec2<f32>(1.0));
    let fringe = (1.0 + 2.5 * clamp(params.signals[0].z, 0.0, 1.0)) / res.x;

    let w = tube_warp(uv, bend);
    let wc = clamp(w, vec2<f32>(0.0), vec2<f32>(1.0));

    // The fringe: red and blue pulled apart a pixel or three, green read
    // straight, the way a misconverged tube smears its edges.
    let r = textureSampleLevel(screen, samp, wc + vec2<f32>(fringe, 0.0), 0.0).r;
    let g = textureSampleLevel(screen, samp, wc, 0.0).g;
    let b = textureSampleLevel(screen, samp, wc - vec2<f32>(fringe, 0.0), 0.0).b;
    var rgb = vec3<f32>(r, g, b);

    // Scanlines on the warped rows so they bow with the face, and a slow
    // soft flicker off the clock the way mains ripple shows on a real one.
    let row = wc.y * res.y;
    let scan = 1.0 - lines * (0.5 + 0.5 * sin(row * 3.14159265));
    let flicker = 1.0 + 0.015 * sin(params.time * 9.0);
    rgb = rgb * scan * flicker;

    // Vignette toward the rim.
    let shade = 1.0 - rim * smoothstep(0.5, 1.4, length((uv - 0.5) * 2.0));
    rgb = rgb * shade;
    return vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
