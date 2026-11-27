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
/// Each part carries two weights. Its draw weight is the opacity a picture
/// draws at under the crossfade rule, back to front. Its share is its part
/// of a true mix, the shares summing to one, which a color blends by and a
/// translucent picture's draw weights are worked out from ([`mix_draws`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Mix<'a> {
    parts: [(Option<&'a str>, f32); 4],
    shares: [f32; 4],
    len: usize,
}

impl<'a> Mix<'a> {
    pub(crate) fn one(name: Option<&'a str>) -> Mix<'a> {
        Mix {
            parts: [(name, 1.); 4],
            shares: [1.; 4],
            len: 1,
        }
    }

    pub(crate) fn parts(&self) -> &[(Option<&'a str>, f32)] {
        &self.parts[..self.len]
    }

    pub(crate) fn shares(&self) -> &[f32] {
        &self.shares[..self.len]
    }

    /// The name drawn on top, the one a settled ease lands on.
    #[cfg(test)]
    pub(crate) fn top(&self) -> Option<&'a str> {
        self.parts[self.len - 1].0
    }

    /// Fades toward `name` at `weight`, 0 to 1. Each side stays whole for
    /// half the way, the incoming one coming in over the first half and the
    /// outgoing one going over the second, so two opaque pictures never dip
    /// through each other. For pictures.
    pub(crate) fn cross(&mut self, name: Option<&'a str>, weight: f32) {
        let outgoing = (2. * (1. - weight)).min(1.);
        let incoming = (2. * weight).min(1.);
        self.push(name, [outgoing, incoming], [1. - weight, weight]);
    }

    /// Blends toward `name` at `weight`, the parts' weights summing to one.
    /// For colors.
    pub(crate) fn blend(&mut self, name: Option<&'a str>, weight: f32) {
        self.push(name, [1. - weight, weight], [1. - weight, weight]);
    }

    /// `draw` and `share` each hold what the parts already there keep and
    /// what the new one takes.
    fn push(&mut self, name: Option<&'a str>, draw: [f32; 2], share: [f32; 2]) {
        let [keep, weight] = draw;
        if weight <= 0. || (self.len == 1 && self.parts[0].0 == name) {
            return;
        }

        if weight >= 1. && keep <= 0. {
            *self = Mix::one(name);
            return;
        }

        for (part, held) in self.parts[..self.len]
            .iter_mut()
            .zip(&mut self.shares[..self.len])
        {
            part.1 *= keep;
            *held *= share[0];
        }

        // Full, the faintest part goes to make room.
        if self.len == self.parts.len() {
            let faintest = (0..self.len)
                .min_by(|a, b| self.parts[*a].1.total_cmp(&self.parts[*b].1))
                .unwrap_or(0);
            self.parts.copy_within(faintest + 1..self.len, faintest);
            self.shares.copy_within(faintest + 1..self.len, faintest);
            self.len -= 1;
        }

        self.parts[self.len] = (name, weight);
        self.shares[self.len] = share[1];
        self.len += 1;
    }
}

/// The opacity each picture of `mix` draws at, back to front, given each
/// one's body alpha (None for an opaque picture or one not decoded yet).
///
/// Two source-over draws can't give a true mix of two translucent pictures
/// per pixel: that needs the lower one drawn at (1 - t) / (1 - t * a) with
/// `a` the upper one's alpha at that pixel. With one opacity per draw, a
/// picture's coverage-weighted mean alpha stands in for `a`, which is exact
/// for glass of an even alpha and close for most. An opaque incoming picture
/// keeps the crossfade rule, since standing 1 in for `a` would hold the old
/// picture whole until the end and cut it where the two shapes differ.
pub(crate) fn mix_draws(mix: &Mix, bodies: &[Option<f32>]) -> Vec<f32> {
    let parts = mix.parts();
    let body = |ix: usize| bodies.get(ix).copied().flatten();
    if parts.len() < 2 || body(parts.len() - 1).is_none() {
        return parts.iter().map(|(_, weight)| *weight).collect();
    }

    // From the top down, each part drawn so that what reaches the screen of
    // it, under the parts above, is its share.
    let mut draws = vec![0.; parts.len()];
    let mut through = 1.;
    for ix in (0..parts.len()).rev() {
        // Under an opaque part drawn whole nothing shows, so nothing draws.
        let draw = if through > 0. {
            (mix.shares()[ix] / through).min(1.)
        } else {
            0.
        };
        draws[ix] = draw;
        through *= 1. - draw * body(ix).unwrap_or(1.);
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

struct Fade {
    to: Pose,
    from: Pose,
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
                from: Pose::of(applied),
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

            // A change mid-fade eases on from the side that was showing
            // more, at the opacity it had reached.
            let left = if t < 0.5 { &fade.from } else { &fade.to };
            fade.from = left.clone();
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

        let (to, from) = (&fade.to, &fade.from);
        let mut pictures = Mix::one(from.asset.as_deref());
        pictures.cross(to.asset.as_deref(), t);
        let mut colors = Mix::one(from.color.as_deref());
        colors.blend(to.color.as_deref(), t);

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
            &[(Some("play"), 1.), (Some("pause"), 1.)],
            "both whole at the middle, the new one on top"
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
    fn a_crossfade_never_dips_and_a_blend_sums_to_one() {
        for step in 0..=10 {
            let weight = step as f32 / 10.;
            let mut pictures = Mix::one(Some("a"));
            pictures.cross(Some("b"), weight);
            let cover = pictures
                .parts()
                .iter()
                .fold(0., |under: f32, (_, w)| under + w * (1. - under));
            assert!(cover >= 1. - 1e-6, "{weight}: {cover}");

            let mut colors = Mix::one(None);
            colors.blend(Some("accent"), weight);
            let sum: f32 = colors.parts().iter().map(|(_, w)| w).sum();
            assert!((sum - 1.).abs() < 1e-6);
        }

        let mut same = Mix::one(Some("a"));
        same.cross(Some("a"), 0.5);
        assert_eq!(same.parts(), &[(Some("a"), 1.)]);
    }

    #[test]
    fn a_full_mix_lets_its_faintest_go() {
        let mut mix = Mix::one(Some("a"));
        for (name, weight) in [("b", 0.9), ("c", 0.8), ("d", 0.7), ("e", 0.6)] {
            mix.cross(Some(name), weight);
        }
        assert_eq!(mix.parts().len(), 4);
        assert_eq!(mix.top(), Some("e"));
        assert_eq!(mix.shares().len(), 4);
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

    /// How much of the backdrop a stack of pictures of one even alpha hides,
    /// drawn back to front at `draws`.
    fn coverage(draws: &[f32], alpha: f32) -> f32 {
        draws
            .iter()
            .fold(0., |under: f32, draw| under + draw * alpha * (1. - under))
    }

    #[test]
    fn a_translucent_crossfade_holds_its_density_and_an_opaque_one_never_dips() {
        let glass = 0.44;
        for t in [0.1, 0.25, 0.5, 0.75, 0.9] {
            let mut pictures = Mix::one(Some("day"));
            pictures.cross(Some("night"), t);

            // Two panes of one alpha mix to that alpha all the way through.
            let mixed = mix_draws(&pictures, &[Some(glass), Some(glass)]);
            assert!((coverage(&mixed, glass) - glass).abs() < 1e-5, "{t}");

            // The crossfade rule thickens the glass mid-way.
            let crossed = mix_draws(&pictures, &[Some(glass), None]);
            assert!(coverage(&crossed, glass) > glass + 0.04, "{t}");

            // An opaque incoming picture keeps the rule that never dips.
            assert_eq!(crossed, [(2. * (1. - t)).min(1.), (2. * t).min(1.)]);
        }
    }

    #[test]
    fn a_change_mid_crossfade_still_mixes_to_the_glass() {
        let glass = 0.5;
        let mut pictures = Mix::one(Some("a"));
        pictures.cross(Some("b"), 0.4);
        pictures.cross(Some("c"), 0.3);
        assert_eq!(pictures.parts().len(), 3);

        let sum: f32 = pictures.shares().iter().sum();
        assert!((sum - 1.).abs() < 1e-6);

        let draws = mix_draws(&pictures, &[Some(glass); 3]);
        assert!((coverage(&draws, glass) - glass).abs() < 1e-5);

        // A settled picture draws whole.
        assert_eq!(mix_draws(&Mix::one(Some("a")), &[Some(glass)]), [1.]);
    }
}
