// rox preset: Cover.
//
// The plain way to draw the playing track's art. `// @asset art: @cover`
// binds the cover (rox's flat dark plate when nothing plays or the track
// has none), and the rest is the letterbox fit every art panel does, laid
// over a wash averaged from the art itself so the bands aren't dead black.
// Binding an image is what puts a program on the region pass, so this
// costs a screen copy where Plasma doesn't.
//
// The zoom slot walks the fit from letterboxed to filled, which crops; the
// bass breathes the whole frame a little either way.
//
// @slot 0: bass
// @slot 1: glow
// @slot 2: dim
// @slot 3: zoom
// @asset art: @cover

// The letterboxed span: the fraction of the panel the fitted image covers
// on each axis, from the two aspect ratios.
fn cover_span(panel: f32, art: f32) -> vec2<f32> {
    if (panel > art) {
        return vec2<f32>(art / panel, 1.0);
    }
    return vec2<f32>(1.0, panel / art);
}

fn fs_user(uv: vec2<f32>) -> vec4<f32> {
    let bass = clamp(params.signals[0].x, 0.0, 1.0);
    let glow = clamp(params.signals[0].y, 0.0, 1.0);
    let dim = clamp(params.signals[0].z, 0.0, 1.0);
    let zoom = clamp(params.signals[0].w, 0.0, 1.0);

    let res = max(params.resolution, vec2<f32>(1.0));
    let dims = vec2<f32>(textureDimensions(art));
    let span = cover_span(res.x / res.y, dims.x / max(dims.y, 1.0));

    // The fit: letterboxed at rest, spread until both axes fill as the
    // zoom slot comes up, breathing a hair on the bass.
    let fit = mix(span, span / min(span.x, span.y), zoom) * (1.0 + 0.02 * bass);
    let auv = (uv - 0.5) / fit + 0.5;
    let inside = step(0.0, auv.x) * step(auv.x, 1.0) * step(0.0, auv.y) * step(auv.y, 1.0);
    let sample =
        textureSampleLevel(art, samp, clamp(auv, vec2<f32>(0.0), vec2<f32>(1.0)), 0.0);

    // The wash behind the bands: the art averaged down to a handful of
    // taps (the texture ships one mip level, so a real blur isn't free),
    // sunk dark and pooling toward the middle.
    let wash = (textureSampleLevel(art, samp, vec2<f32>(0.25, 0.25), 0.0).rgb
        + textureSampleLevel(art, samp, vec2<f32>(0.75, 0.25), 0.0).rgb
        + textureSampleLevel(art, samp, vec2<f32>(0.5, 0.5), 0.0).rgb
        + textureSampleLevel(art, samp, vec2<f32>(0.25, 0.75), 0.0).rgb
        + textureSampleLevel(art, samp, vec2<f32>(0.75, 0.75), 0.0).rgb)
        / 5.0;
    let middle = 1.0 - 0.6 * clamp(length(uv - 0.5) * 1.6, 0.0, 1.0);
    let backdrop = wash * (0.10 + 0.30 * glow) * middle;

    let rgb = mix(backdrop, sample.rgb, inside) * (1.0 - 0.7 * dim);
    return vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
