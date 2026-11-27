//! Eased case changes (ADR 33). When the state an item follows moves to
//! another case, the item eases from the look it had to the new one over
//! [`EASE`], or its own `ease` in milliseconds, where 0 keeps the cut. An
//! item following the theme with no `ease` of its own takes the look's theme
//! ease, the length the palette fades over.
//! Opacity blends, a text color blends, and two images crossfade, each drawn
//! with its own fit and nine-slice cut. A case that hides its item fades it
//! out and lets go of the pointer as the fade starts; one that shows it takes
//! the pointer at once and fades it in.
//!
//! A theme switch moves every themed item in the same render, so their fades
//! share one start and one burst of frames. Frames are asked for only while
//! a fade runs. Design mode, Reduce Motion, and a shader item, whose quad
//! takes no opacity, all cut as before.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use rox_canvas::{CanvasItem, Content};
use rox_design::palette;

use crate::canvas::states::Applied;

/// How long a case change takes when the item doesn't say. A little longer
/// than a hover, since nothing under the pointer is waiting on it.
pub(crate) const EASE: Duration = Duration::from_millis(180);

/// The longest an item's own `ease` runs, whatever a file says.
pub(crate) const MAX_EASE_MS: u32 = 5000;

/// The state a theme switch moves.
const THEME: &str = "app.theme";

/// How long `item`'s case changes take. Zero cuts. An item following the
/// theme takes the look's theme ease when it doesn't say, so it lands with
/// the panels and the role-coloured text, which ease with the palette.
pub(crate) fn length(item: &CanvasItem, still: bool) -> Duration {
    if still || matches!(item.content, Content::Shader { .. }) {
        return Duration::ZERO;
    }

    match item.ease {
        Some(ms) => Duration::from_millis(u64::from(ms.min(MAX_EASE_MS))),
        None if item.state == THEME => palette::theme_ease(),
        None => EASE,
    }
}

/// Weighted names: the images an item draws this frame, or the colors its
/// text blends. None names the item's own. Rarely more than two.
///
/// Each part's weight is its share of a true mix, the shares summing to one.
/// A color blends by them; a picture's draw opacities are worked out from
/// them ([`mix_draws`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Mix<'a> {
    parts: [(Option<&'a str>, f32); 4],
    len: usize,
}

impl<'a> Mix<'a> {
    pub(crate) fn one(name: Option<&'a str>) -> Mix<'a> {
        Mix {
            parts: [(name, 1.); 4],
            len: 1,
        }
    }

    /// A mix a fade held on to. Past four parts the rest go.
    fn of(held: &'a [(Option<String>, f32)]) -> Mix<'a> {
        let mut mix = Mix::one(None);
        for (ix, (name, share)) in held.iter().take(mix.parts.len()).enumerate() {
            mix.parts[ix] = (name.as_deref(), *share);
            mix.len = ix + 1;
        }
        mix
    }

    pub(crate) fn parts(&self) -> &[(Option<&'a str>, f32)] {
        &self.parts[..self.len]
    }

    /// The name drawn on top, the one a settled ease lands on.
    #[cfg(test)]
    pub(crate) fn top(&self) -> Option<&'a str> {
        self.parts[self.len - 1].0
    }

    /// Moves toward `name` at `weight`, 0 to 1: the parts already there
    /// keep `1 - weight` of their shares and the new one takes `weight`.
    pub(crate) fn blend(&mut self, name: Option<&'a str>, weight: f32) {
        if weight <= 0. || (self.len == 1 && self.parts[0].0 == name) {
            return;
        }

        if weight >= 1. {
            *self = Mix::one(name);
            return;
        }

        for part in &mut self.parts[..self.len] {
            part.1 *= 1. - weight;
        }

        // Full, the faintest part goes to make room.
        if self.len == self.parts.len() {
            let faintest = (0..self.len)
                .min_by(|a, b| self.parts[*a].1.total_cmp(&self.parts[*b].1))
                .unwrap_or(0);
            self.parts.copy_within(faintest + 1..self.len, faintest);
            self.len -= 1;
        }

        self.parts[self.len] = (name, weight);
        self.len += 1;
    }
}

/// Cells a side in a picture's [`Coverage`].
const COVER_CELLS: usize = 32;

/// A picture's mean alpha over a coarse grid laid across its own extent,
/// gathered once as it decodes. Two pictures of one item fill one box, so
/// the grids laid cell on cell say how much of one the other covers.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Coverage(Box<[f32]>);

impl Coverage {
    /// From four-byte pixels, alpha last, row by row. A picture smaller
    /// than the grid spreads each pixel over the cells it spans.
    pub(crate) fn of(width: usize, height: usize, pixels: &[u8]) -> Coverage {
        if width == 0 || height == 0 || pixels.len() < width * height * 4 {
            return Coverage(vec![0.; COVER_CELLS * COVER_CELLS].into());
        }

        let span = |cell: usize, side: usize| {
            let from = cell * side / COVER_CELLS;
            from..((cell + 1) * side / COVER_CELLS).max(from + 1)
        };

        let mut cells = Vec::with_capacity(COVER_CELLS * COVER_CELLS);
        for row in 0..COVER_CELLS {
            for column in 0..COVER_CELLS {
                let (mut sum, mut count) = (0u32, 0u32);
                for y in span(row, height) {
                    for x in span(column, width) {
                        sum += u32::from(pixels[(y * width + x) * 4 + 3]);
                        count += 1;
                    }
                }
                cells.push(sum as f32 / (count as f32 * 255.));
            }
        }

        Coverage(cells.into())
    }

    /// How opaque `over` is, on average, where this picture shows: 1 where
    /// it covers this one whole, 0 where they share nothing. 1 for a picture
    /// with nothing in it, whose draw doesn't matter.
    fn under(&self, over: &Coverage) -> f32 {
        let (mut shared, mut mine) = (0., 0.);
        for (a, b) in self.0.iter().zip(over.0.iter()) {
            shared += a * b;
            mine += a;
        }

        if mine > 0. { shared / mine } else { 1. }
    }
}

/// The opacity each picture of `mix` draws at, back to front, given each
/// one's coverage (None for one not decoded yet).
///
/// With the upper picture drawn at its share `t`, the lower one comes out as
/// its share `1 - t` of the true mix where it's drawn at `(1 - t) / (1 - t * b)`,
/// `b` the upper picture's alpha at that pixel. Two opaque pictures of one
/// shape have `b` 1, so the lower one draws whole under the upper at `t`,
/// which is the exact mix over the whole ease. Where they share nothing `b`
/// is 0 and each draws at its share, also exact. One opacity per draw can't
/// follow `b` pixel by pixel, so the upper picture's alpha averaged over the
/// lower one's coverage ([`Coverage::under`]) stands in: exact for a pair of
/// one shape, a full-frame picture over anything, glass of an even alpha or
/// pictures that share nothing, and close between. Neither dips at the middle
/// nor holds a picture whole until it cuts at the end, the way drawing the
/// outgoing one whole under every pair would where their shapes differ.
pub(crate) fn mix_draws(mix: &Mix, covers: &[Option<&Coverage>]) -> Vec<f32> {
    let parts = mix.parts();

    // Unknown coverage stands in as whole: an upper picture still decoding
    // draws nothing, so the one under it draws whole.
    let under = |lower: usize, upper: usize| match (
        covers.get(lower).copied().flatten(),
        covers.get(upper).copied().flatten(),
    ) {
        (Some(lower), Some(upper)) => lower.under(upper),
        _ => 1.,
    };

    // From the top down, each part drawn so that what reaches the screen of
    // it, under the parts above, is its share.
    let mut draws = vec![0.; parts.len()];
    for ix in (0..parts.len()).rev() {
        let through: f32 = (ix + 1..parts.len())
            .map(|above| 1. - draws[above] * under(ix, above))
            .product();

        // Under an opaque part drawn whole nothing shows, so nothing draws.
        draws[ix] = if through > 0. {
            (parts[ix].1 / through).min(1.)
        } else {
            0.
        };
    }

    draws
}

/// A case's look as a fade holds it.
#[derive(Clone, Debug, PartialEq)]
struct Pose {
    visible: bool,
    opacity: f32,
    asset: Option<String>,
    color: Option<String>,
}

impl Pose {
    fn of(applied: &Applied) -> Pose {
        Pose {
            visible: applied.visible,
            opacity: applied.opacity,
            asset: applied.asset.map(str::to_string),
            color: applied.color.map(str::to_string),
        }
    }

    /// Compared without building a pose, since every item asks every render.
    fn is(&self, applied: &Applied) -> bool {
        self.visible == applied.visible
            && self.opacity == applied.opacity
            && self.asset.as_deref() == applied.asset
            && self.color.as_deref() == applied.color
    }

    /// How much of the item shows: its opacity, or none while hidden.
    fn weight(&self) -> f32 {
        if self.visible { self.opacity } else { 0. }
    }
}

/// A mix's parts, owned, for a fade to carry on from.
type Held = Vec<(Option<String>, f32)>;

fn held(mix: &Mix) -> Held {
    mix.parts()
        .iter()
        .map(|(name, share)| (name.map(str::to_string), *share))
        .collect()
}

struct Fade {
    to: Pose,
    /// The images and the colors it showed when the change came: a mix of
    /// its own when the change landed mid-fade, so it eases on from there.
    from_pictures: Held,
    from_colors: Held,
    /// The opacity it showed when the change came, which a change that
    /// lands mid-fade eases on from.
    from_weight: f32,
    at: Option<Instant>,
    length: Duration,
    seen: bool,
}

impl Fade {
    /// 0 to 1 along the fade, smoothstepped. 1 once it has landed.
    fn progress(&self, now: Instant) -> f32 {
        let Some(at) = self.at else {
            return 1.;
        };

        let t = now.saturating_duration_since(at).as_secs_f32() / self.length.as_secs_f32();
        if t >= 1. {
            return 1.;
        }

        t * t * (3. - 2. * t)
    }

    /// The images and the colors it draws at `t` along.
    fn mixes(&self, t: f32) -> (Mix<'_>, Mix<'_>) {
        let mut pictures = Mix::of(&self.from_pictures);
        pictures.blend(self.to.asset.as_deref(), t);
        let mut colors = Mix::of(&self.from_colors);
        colors.blend(self.to.color.as_deref(), t);
        (pictures, colors)
    }
}

/// What a case does to an item this frame, eased.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Faded<'a> {
    /// Multiplies the item's opacity: the case's, and none once hidden.
    pub opacity: f32,
    /// Whether anything of it draws, which a hidden case keeps true until
    /// its fade is done.
    pub shown: bool,
    /// Whether it takes the pointer: only while its case shows it.
    pub pointer: bool,
    pub pictures: Mix<'a>,
    pub colors: Mix<'a>,
}

impl<'a> Faded<'a> {
    /// The case as it is, with no ease.
    #[cfg(test)]
    pub(crate) fn at_once(applied: &Applied<'a>) -> Faded<'a> {
        Faded {
            opacity: if applied.visible { applied.opacity } else { 0. },
            shown: applied.visible,
            pointer: applied.visible,
            pictures: Mix::one(applied.asset),
            colors: Mix::one(applied.color),
        }
    }
}

/// Every item's last case and any fade still running, for one canvas.
#[derive(Default)]
pub(crate) struct Fades {
    items: HashMap<String, Fade>,
}

impl Fades {
    /// Item `id`'s case this frame, eased from the one it last showed over
    /// `length`. The first sight of an item cuts, so a canvas opens on its
    /// cases as they are.
    pub(crate) fn step(
        &mut self,
        id: &str,
        applied: &Applied,
        length: Duration,
        now: Instant,
    ) -> Faded<'_> {
        // Looked up before it's keyed, so a render allocates only for an
        // item it hasn't seen.
        if !self.items.contains_key(id) {
            let fade = Fade {
                to: Pose::of(applied),
                from_pictures: vec![(applied.asset.map(str::to_string), 1.)],
                from_colors: vec![(applied.color.map(str::to_string), 1.)],
                from_weight: 0.,
                at: None,
                length,
                seen: true,
            };
            self.items.insert(id.to_string(), fade);
        }
        let fade = self.items.get_mut(id).expect("held a line above");
        fade.seen = true;

        if !fade.to.is(applied) {
            let t = fade.progress(now);
            let showing = fade.from_weight + (fade.to.weight() - fade.from_weight) * t;

            // A change mid-fade eases on from the mix and the opacity it
            // had reached, so nothing steps.
            let (pictures, colors) = fade.mixes(t);
            let (pictures, colors) = (held(&pictures), held(&colors));
            fade.from_pictures = pictures;
            fade.from_colors = colors;
            fade.from_weight = showing;
            fade.to = Pose::of(applied);
            fade.at = (!length.is_zero()).then_some(now);
            fade.length = length;
        } else if length.is_zero() {
            fade.at = None;
        }

        let t = fade.progress(now);
        if t >= 1. {
            fade.at = None;
        }

        let (pictures, colors) = fade.mixes(t);
        let to = &fade.to;

        Faded {
            opacity: fade.from_weight + (to.weight() - fade.from_weight) * t,
            shown: to.visible || fade.at.is_some(),
            pointer: to.visible,
            pictures,
            colors,
        }
    }

    /// After a render has placed every item: drops what it didn't see, and
    /// answers whether a fade still runs, which is when to ask for a frame.
    pub(crate) fn settle(&mut self) -> bool {
        self.items.retain(|_, fade| std::mem::take(&mut fade.seen));
        self.items.values().any(|fade| fade.at.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(visible: bool, opacity: f32, asset: Option<&'static str>) -> Applied<'static> {
        Applied {
            visible,
            opacity,
            asset,
            ..Applied::default()
        }
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_first_sight_cuts_and_asks_for_nothing() {
        let mut fades = Fades::default();
        let now = Instant::now();

        let faded = fades.step("a", &case(true, 0.5, Some("x")), EASE, now);
        assert_eq!(faded.opacity, 0.5);
        assert_eq!(faded.pictures.parts(), &[(Some("x"), 1.)]);
        assert!(!fades.settle());
    }

    #[test]
    fn a_case_change_eases_and_then_asks_for_nothing() {
        let mut fades = Fades::default();
        let start = Instant::now();
        fades.step("a", &case(true, 1., Some("play")), EASE, start);
        fades.settle();

        let pause = case(true, 0.5, Some("pause"));
        let first = fades.step("a", &pause, EASE, start);
        assert_eq!(first.opacity, 1., "the change starts where it stood");
        assert_eq!(first.pictures.parts(), &[(Some("play"), 1.)]);
        assert!(fades.settle(), "a running fade wants frames");

        let midway = fades.step("a", &pause, EASE, start + EASE / 2);
        assert!((midway.opacity - 0.75).abs() < 1e-6);
        assert_eq!(
            midway.pictures.parts(),
            &[(Some("play"), 0.5), (Some("pause"), 0.5)],
            "half each at the middle, the new one on top"
        );
        fades.settle();

        let done = fades.step("a", &pause, EASE, start + EASE);
        assert_eq!(done.opacity, 0.5);
        assert_eq!(done.pictures.parts(), &[(Some("pause"), 1.)]);
        assert!(!fades.settle());
    }

    #[test]
    fn a_hidden_case_fades_out_and_lets_go_of_the_pointer_at_once() {
        let mut fades = Fades::default();
        let start = Instant::now();
        fades.step("a", &case(true, 1., None), EASE, start);

        let hidden = case(false, 1., None);
        let leaving = fades.step("a", &hidden, EASE, start + ms(1));
        assert!(leaving.shown && !leaving.pointer);

        let gone = fades.step("a", &hidden, EASE, start + ms(1) + EASE);
        assert!(!gone.shown && !gone.pointer);
        assert_eq!(gone.opacity, 0.);

        // Shown again, it takes the pointer before it has faded in.
        let back = fades.step("a", &case(true, 1., None), EASE, start + EASE * 3);
        assert!(back.shown && back.pointer);
        assert_eq!(back.opacity, 0.);
    }

    #[test]
    fn a_cut_lands_in_the_frame_it_came() {
        let mut fades = Fades::default();
        let now = Instant::now();
        fades.step("a", &case(true, 1., Some("play")), Duration::ZERO, now);

        let faded = fades.step("a", &case(false, 1., Some("pause")), Duration::ZERO, now);
        assert!(!faded.shown);
        assert_eq!(faded.pictures.parts(), &[(Some("pause"), 1.)]);
        assert!(!fades.settle());
    }

    #[test]
    fn a_change_mid_fade_carries_on_from_where_it_stood() {
        let mut fades = Fades::default();
        let start = Instant::now();
        fades.step("a", &case(true, 1., None), EASE, start);
        fades.step("a", &case(false, 1., None), EASE, start);

        let midway = start + EASE / 2;
        let stood = fades
            .step("a", &case(false, 1., None), EASE, midway)
            .opacity;
        let turned = fades.step("a", &case(true, 1., None), EASE, midway).opacity;
        assert!((stood - turned).abs() < 1e-6, "{stood} then {turned}");
    }

    #[test]
    fn a_change_mid_crossfade_turns_back_from_the_mix_it_showed() {
        let mut fades = Fades::default();
        let start = Instant::now();
        fades.step("a", &case(true, 1., Some("day")), EASE, start);
        fades.step("a", &case(true, 1., Some("night")), EASE, start);

        // Past the middle, turned back: the mix it showed is where the
        // new fade starts, not either picture whole.
        let late = start + EASE * 3 / 4;
        let night = case(true, 1., Some("night"));
        let stood = held(&fades.step("a", &night, EASE, late).pictures);
        let day = case(true, 1., Some("day"));
        let turned = held(&fades.step("a", &day, EASE, late).pictures);
        assert_eq!(stood, turned);
        assert_eq!(turned.len(), 2);

        // Midway back, three parts sum to one, and it lands on day alone.
        let back = fades.step("a", &day, EASE, late + EASE / 2).pictures;
        let sum: f32 = back.parts().iter().map(|(_, share)| share).sum();
        assert!((sum - 1.).abs() < 1e-6, "{back:?}");
        assert_eq!(back.top(), Some("day"));
        let landed = fades.step("a", &day, EASE, late + EASE).pictures;
        assert_eq!(landed.parts(), &[(Some("day"), 1.)]);
    }

    #[test]
    fn items_not_drawn_are_forgotten() {
        let mut fades = Fades::default();
        let now = Instant::now();
        fades.step("a", &case(true, 1., None), EASE, now);
        fades.settle();
        fades.settle();
        assert!(fades.items.is_empty());
    }

    #[test]
    fn an_items_own_ease_is_held_in_range() {
        let mut item = CanvasItem::default();
        assert_eq!(length(&item, false), EASE);
        assert_eq!(length(&item, true), Duration::ZERO, "Reduce Motion cuts");

        item.ease = Some(0);
        assert_eq!(length(&item, false), Duration::ZERO);
        item.ease = Some(u32::MAX);
        assert_eq!(length(&item, false), ms(u64::from(MAX_EASE_MS)));

        item.ease = None;
        item.content = Content::Shader {
            shader: serde_json::Value::Null,
        };
        assert_eq!(length(&item, false), Duration::ZERO);
    }

    #[test]
    fn a_blend_sums_to_one() {
        for step in 0..=10 {
            let weight = step as f32 / 10.;
            let mut colors = Mix::one(None);
            colors.blend(Some("accent"), weight);
            let sum: f32 = colors.parts().iter().map(|(_, w)| w).sum();
            assert!((sum - 1.).abs() < 1e-6);
        }

        let mut same = Mix::one(Some("a"));
        same.blend(Some("a"), 0.5);
        assert_eq!(same.parts(), &[(Some("a"), 1.)]);
    }

    #[test]
    fn a_full_mix_lets_its_faintest_go() {
        let mut mix = Mix::one(Some("a"));
        for (name, weight) in [("b", 0.9), ("c", 0.8), ("d", 0.7), ("e", 0.6)] {
            mix.blend(Some(name), weight);
        }
        assert_eq!(mix.parts().len(), 4);
        assert_eq!(mix.top(), Some("e"));
    }

    #[test]
    fn a_theme_item_takes_the_theme_ease_unless_it_says() {
        let mut item = CanvasItem {
            state: THEME.to_string(),
            ..CanvasItem::default()
        };
        assert_eq!(length(&item, false), palette::theme_ease());
        assert_eq!(length(&item, true), Duration::ZERO, "Reduce Motion cuts");

        item.ease = Some(120);
        assert_eq!(length(&item, false), ms(120));

        item.ease = None;
        item.state = "player.playback".to_string();
        assert_eq!(length(&item, false), EASE);
    }

    /// A 32 square picture, `alpha` at each cell.
    fn cover(alpha: impl Fn(usize, usize) -> u8) -> Coverage {
        let side = COVER_CELLS;
        let pixels: Vec<u8> = (0..side * side)
            .flat_map(|ix| [0, 0, 0, alpha(ix % side, ix / side)])
            .collect();
        Coverage::of(side, side, &pixels)
    }

    /// One pixel of pictures drawn back to front over `backdrop`, each as
    /// (color, alpha) at its draw opacity, against the true mix of the same
    /// pictures by their shares, each composited alone.
    fn composite(layers: &[(f32, f32)], draws: &[f32], shares: &[f32], backdrop: f32) -> [f32; 2] {
        let drawn = layers
            .iter()
            .zip(draws)
            .fold(backdrop, |under, ((color, alpha), draw)| {
                draw * alpha * color + (1. - draw * alpha) * under
            });
        let truth = layers
            .iter()
            .zip(shares)
            .map(|((color, alpha), share)| share * (alpha * color + (1. - alpha) * backdrop))
            .sum();
        [drawn, truth]
    }

    fn shares(mix: &Mix) -> Vec<f32> {
        mix.parts().iter().map(|(_, share)| *share).collect()
    }

    #[test]
    fn two_opaque_pictures_of_one_shape_mix_exactly_the_whole_way() {
        // A gel button's day and night: one disc, clear round it.
        let disc = |x: usize, y: usize| {
            let off = (x as f32 - 15.5).hypot(y as f32 - 15.5);
            if off < 12. { 255 } else { 0 }
        };
        let (day, night) = (cover(disc), cover(disc));
        let sky = cover(|_, _| 255);

        for t in [0.1, 0.25, 0.5, 0.75, 0.9] {
            let mut pictures = Mix::one(Some("day"));
            pictures.blend(Some("night"), t);
            let shares = shares(&pictures);

            let draws = mix_draws(&pictures, &[Some(&day), Some(&night)]);
            assert!(
                (draws[0] - 1.).abs() < 1e-5 && (draws[1] - t).abs() < 1e-6,
                "{t}: {draws:?}"
            );
            let [drawn, truth] = composite(&[(0.9, 1.), (0.1, 1.)], &draws, &shares, 0.5);
            assert!((drawn - truth).abs() < 1e-5, "{t}: {drawn} against {truth}");

            // A full-frame picture coming in covers whatever was there.
            let draws = mix_draws(&pictures, &[Some(&day), Some(&sky)]);
            assert!((draws[0] - 1.).abs() < 1e-5, "{t}: {draws:?}");
        }
    }

    #[test]
    fn pictures_that_share_nothing_each_draw_at_their_share() {
        let left = cover(|x, _| if x < 16 { 255 } else { 0 });
        let right = cover(|x, _| if x >= 16 { 255 } else { 0 });

        for t in [0.1, 0.5, 0.9] {
            let mut pictures = Mix::one(Some("left"));
            pictures.blend(Some("right"), t);
            let draws = mix_draws(&pictures, &[Some(&left), Some(&right)]);
            assert!((draws[0] - (1. - t)).abs() < 1e-5, "{t}: {draws:?}");

            // Where only the outgoing one lies it fades on the true mix,
            // with no hold to cut at the end.
            let shares = shares(&pictures);
            let [drawn, truth] = composite(&[(0.9, 1.), (0.1, 0.)], &draws, &shares, 0.5);
            assert!((drawn - truth).abs() < 1e-5, "{t}: {drawn} against {truth}");
        }
    }

    #[test]
    fn a_partial_overlap_neither_cuts_nor_dips_far() {
        // The outgoing picture's left half is under the incoming one.
        let wide = cover(|_, _| 255);
        let half = cover(|x, _| if x < 16 { 255 } else { 0 });

        let mut last = 1.;
        for step in 1..=20 {
            let t = step as f32 / 20.;
            let mut pictures = Mix::one(Some("wide"));
            pictures.blend(Some("half"), t);
            let draws = mix_draws(&pictures, &[Some(&wide), Some(&half)]);
            if step == 20 {
                assert_eq!(draws, [1.], "landed");
                break;
            }

            // The outgoing draw only ever falls, and it's already small at
            // the end, so it never holds whole and cuts.
            assert!(draws[0] <= last + 1e-6, "{t}: {draws:?}");
            last = draws[0];

            // Where both lie, the density stays within a fifth of whole.
            let shared = draws[1] + (1. - draws[1]) * draws[0];
            assert!(shared > 0.8, "{t}: {shared}");
        }
        assert!(last < 0.1, "{last}");
    }

    #[test]
    fn a_translucent_crossfade_holds_its_density() {
        let glass = cover(|_, _| 112);
        let alpha = 112. / 255.;
        for t in [0.1, 0.25, 0.5, 0.75, 0.9] {
            let mut pictures = Mix::one(Some("day"));
            pictures.blend(Some("night"), t);

            // Two panes of one alpha mix to that alpha all the way through.
            let draws = mix_draws(&pictures, &[Some(&glass), Some(&glass)]);
            let shares = shares(&pictures);
            let [drawn, truth] = composite(&[(1., alpha), (1., alpha)], &draws, &shares, 0.);
            assert!((drawn - truth).abs() < 1e-5, "{t}: {drawn} against {truth}");
        }
    }

    #[test]
    fn a_change_mid_crossfade_still_mixes_to_the_glass() {
        let glass = cover(|_, _| 128);
        let alpha = 128. / 255.;
        let mut pictures = Mix::one(Some("a"));
        pictures.blend(Some("b"), 0.4);
        pictures.blend(Some("c"), 0.3);
        assert_eq!(pictures.parts().len(), 3);

        let shares = shares(&pictures);
        assert!((shares.iter().sum::<f32>() - 1.).abs() < 1e-6);

        let draws = mix_draws(&pictures, &[Some(&glass); 3]);
        let [drawn, truth] = composite(&[(1., alpha); 3], &draws, &shares, 0.);
        assert!((drawn - truth).abs() < 1e-5, "{drawn} against {truth}");

        // A settled picture draws whole, and one still decoding counts as
        // covering, so the picture under it holds.
        assert_eq!(mix_draws(&Mix::one(Some("a")), &[Some(&glass)]), [1.]);
        let mut pending = Mix::one(Some("a"));
        pending.blend(Some("b"), 0.5);
        assert_eq!(mix_draws(&pending, &[Some(&glass), None]), [1., 0.5]);
    }

    #[test]
    fn coverage_is_the_mean_alpha_by_cell() {
        let pixels: Vec<u8> = (0..64 * 64)
            .flat_map(|ix| [0, 0, 0, if ix % 64 < 32 { 255 } else { 0 }])
            .collect();
        let half = Coverage::of(64, 64, &pixels);
        let full = cover(|_, _| 255);
        assert!((full.under(&half) - 0.5).abs() < 1e-6);
        assert!((half.under(&full) - 1.).abs() < 1e-6);

        let clear = cover(|_, _| 0);
        assert_eq!(clear.under(&full), 1., "nothing to cover");
        assert_eq!(full.under(&clear), 0.);
    }
}
