// rox preset: Badge.
//
// The cover as a small card parked in a corner: the working example for
// putting an image at a place and size of your choosing. The placement is
// plain rect math in device pixels, so the card stays square whatever the
// panel's shape; the corner slot walks it around the panel a quarter turn
// at a time, and everything past the card and its shadow stays
// transparent, so it rides any panel.
//
// @overlay
// @slot 0: bass
// @slot 1: corner
// @slot 2: size
// @slot 3: fade
// @asset art: @cover

fn fs_user(uv: vec2<f32>) -> vec4<f32> {
    let bass = clamp(params.signals[0].x, 0.0, 1.0);
    let corner = clamp(params.signals[0].y, 0.0, 1.0);
    let size = clamp(params.signals[0].z, 0.0, 1.0);
    let fade = clamp(params.signals[0].w, 0.0, 1.0);

    let res = max(params.resolution, vec2<f32>(1.0));
    let p = uv * res;
    let short = min(res.x, res.y);

    // The card's box: a square off the short edge, grown by the size slot,
    // breathing on the bass, held off the rim by a fixed margin.
    let edge = short * (0.24 + 0.38 * size) * (1.0 + 0.03 * bass);
    let margin = short * 0.05;

    // Which corner: quarters of the slot. Unrouted reads zero, which is
    // bottom right, then the quarters walk it counterclockwise.
    let pick = min(u32(corner * 4.0), 3u);
    let right = pick == 0u || pick == 3u;
    let bottom = pick < 2u;
    let origin = vec2<f32>(
        select(margin, res.x - margin - edge, right),
        select(margin, res.y - margin - edge, bottom),
    );
    let local = (p - origin) / edge;
    let inside = step(0.0, local.x) * step(local.x, 1.0) * step(0.0, local.y) * step(local.y, 1.0);

    // The art letterboxed inside the square card, dark bands where the
    // aspects disagree.
    let dims = vec2<f32>(textureDimensions(art));
    let arta = dims.x / max(dims.y, 1.0);
    var span = vec2<f32>(1.0, 1.0);
    if (arta > 1.0) {
        span.y = 1.0 / arta;
    } else {
        span.x = arta;
    }
    let auv = (local - 0.5) / span + 0.5;
    let framed = step(0.0, auv.x) * step(auv.x, 1.0) * step(0.0, auv.y) * step(auv.y, 1.0);
    let sample =
        textureSampleLevel(art, samp, clamp(auv, vec2<f32>(0.0), vec2<f32>(1.0)), 0.0);
    let card = mix(vec3<f32>(0.06), sample.rgb, framed);

    // A soft shadow past the rim, alpha only, so the card reads as sitting
    // on the panel rather than painted into it.
    let center = origin + vec2<f32>(edge * 0.5);
    let outside = length(max(abs(p - center) - vec2<f32>(edge * 0.5), vec2<f32>(0.0)));
    let shadow = exp(-outside / max(short * 0.02, 1.0)) * 0.35 * (1.0 - inside);

    let hold = 1.0 - fade;
    let alpha = clamp(inside + shadow, 0.0, 1.0) * hold;
    return vec4<f32>(card * inside * hold, alpha);
}
