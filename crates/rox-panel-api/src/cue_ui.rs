//! Session cues on a strip: the chevrons hanging off the top edge, the
//! hit layer over them, and the menu the strip's right click opens. The
//! counterpart to [`crate::bookmark_ui`], which draws the persisted marks
//! along the bottom.
//!
//! The split down the middle of the strip is the whole visual grammar.
//! Bottom means kept, a ribbon in the mark's own colour, written to the
//! library and still there next month. Top means this listen only, a
//! chevron in the theme accent, gone when rox closes. Neither needs a
//! legend, because the two never share an edge and never share a shape.
//!
//! Cues carry no name and no colour of their own, so there's none of the
//! quick-pick and rename machinery here. A cue is a position and an id.

use gpui::{
    App, Bounds, Context, Div, MouseButton, MouseDownEvent, MouseMoveEvent, Path, Pixels, Window,
    div, prelude::*, px, relative,
};
use gpui_component::Icon;
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use rox_core::fmt::fmt_time;
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_library::cue::TrackKey;
use rox_services::cues::Cue;

use crate::openers;
use crate::panel::{AppState, ScrubState};
use crate::position_bound;

/// One cue placed along a strip.
#[derive(Clone, Copy)]
pub struct CueMark {
    pub id: u64,
    /// Where along the track, 0 to 1.
    pub fraction: f32,
    pub position_ms: u32,
}

/// Place a track's cues along its strip. Nothing without a duration: a
/// fraction of an unknown length points nowhere.
pub fn marks(cues: &[Cue], duration_secs: Option<f64>) -> Vec<CueMark> {
    let Some(duration) = duration_secs.filter(|d| *d > 0.0) else {
        return Vec::new();
    };

    cues.iter()
        .map(|cue| CueMark {
            id: cue.id,
            fraction: ((cue.position_ms as f64 / 1000.0) / duration).clamp(0.0, 1.0) as f32,
            position_ms: cue.position_ms,
        })
        .collect()
}

/// The chevron's footprint: its base width and height in px, and the
/// stroke it's drawn with. Same budget as the bookmark ribbon opposite it,
/// so a strip with both ends marked reads as one row of tabs rather than
/// two sizes of thing.
pub const MARK_W: f32 = 10.0;
pub const MARK_H: f32 = 6.0;
const MARK_STROKE: f32 = 2.5;
/// The hit target around a chevron, wider than the drawing so a pointer
/// finds it without aiming.
const HIT_W: f32 = 16.0;
/// The chevron's alpha at full weight.
const MARK_ALPHA: u8 = 0xe6;

/// Paint the cues over a strip. `weight` scales the alpha, for a strip
/// fading its shape in or out. The chevrons hang off the top edge pointing
/// down at the line, the mirror of where the bookmarks sit.
pub fn paint_marks(marks: &[CueMark], weight: f32, bounds: Bounds<Pixels>, window: &mut Window) {
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    if w <= 0.0 || h <= MARK_H || marks.is_empty() {
        return;
    }

    let alpha = (MARK_ALPHA as f32 * weight.clamp(0.0, 1.0)) as u8;
    if alpha == 0 {
        return;
    }

    let (x0, y0) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    let at = |x: f32, y: f32| gpui::point(px(x0 + x), px(y0 + y));
    let solid = (
        gpui::point(0., 1.),
        gpui::point(0., 1.),
        gpui::point(0., 1.),
    );
    let color = palette::alpha(palette::accent(), alpha);

    let top = 1.0;
    let apex_y = top + MARK_H;
    let half = MARK_W / 2.0;
    // The inner edge's inset along the base: the stroke measured across
    // the slope, so the band reads the same thickness up its whole arm.
    let inset = MARK_STROKE * (half * half + MARK_H * MARK_H).sqrt() / MARK_H;

    for mark in marks {
        let x = mark.fraction.clamp(0.0, 1.0) * w;
        // A chevron band: the outer triangle with its inner triangle taken
        // out, as four triangles round the ring.
        let apex = at(x, apex_y);
        let tl = at(x - half, top);
        let tr = at(x + half, top);
        let apex_in = at(x, apex_y - MARK_STROKE);
        let tl_in = at(x - half + inset, top);
        let tr_in = at(x + half - inset, top);

        let mut path = Path::new(apex);
        path.push_triangle((apex, tr, tr_in), solid);
        path.push_triangle((apex, tr_in, apex_in), solid);
        path.push_triangle((apex, apex_in, tl_in), solid);
        path.push_triangle((apex, tl_in, tl), solid);
        window.paint_path(path, color);
    }
}

/// Jump the playing track to its next cue, or the one before the playhead.
/// Nothing playing, a station playing, or no cue that way, does nothing.
pub fn step(state: &AppState, forward: bool, cx: &mut App) {
    if !position_bound::allowed(state, cx) {
        return;
    }
    let Some(now) = state.player.read(cx).now_playing() else {
        return;
    };

    let at = (now.position_secs.max(0.0) * 1000.0).round() as u32;
    let cues = state.cues.read(cx);
    let target = if forward {
        cues.next_after(&now.key, at)
    } else {
        cues.prev_before(&now.key, at)
    };

    if let Some(cue) = target {
        state
            .player
            .read(cx)
            .seek_to(cue.position_ms as f64 / 1000.0);
    }
}

/// Take every session mark off the playing track in one go, the counterpart
/// to dropping them one press at a time.
///
/// No position-bound gate, unlike [`step`] and the drop. Clearing doesn't
/// need a position to point at, and a station that can't hold marks in the
/// first place has nothing here to take, so the guard would only ever
/// refuse a no-op.
pub fn clear(state: &AppState, cx: &mut App) {
    let Some(now) = state.player.read(cx).now_playing() else {
        return;
    };

    state.cues.update(cx, |cues, cx| cues.clear(&now.key, cx));
}

/// The interactive layer over a strip's cues: a hit target per chevron
/// that seeks on a click, reports its hover, and offers the removal on a
/// right click, plus the readout over the hovered one. Laid over the
/// strip's own hover layer, so a pointer on a chevron reads the cue and
/// not the time under it.
///
/// `hovered` is the panel's record of which cue the pointer is on, kept by
/// the panel because it outlives one render; `on_hover` is how the layer
/// updates it.
#[allow(clippy::too_many_arguments)]
pub fn overlay<V: 'static>(
    state: &AppState,
    key: &TrackKey,
    marks: &[CueMark],
    hovered: Option<u64>,
    scrub: &ScrubState,
    on_hover: impl Fn(&mut V, Option<u64>, &mut Context<V>) + Clone + 'static,
    cx: &mut Context<V>,
) -> Div {
    let mut layer = div().absolute().inset_0();

    for mark in marks {
        let id = mark.id;
        let secs = mark.position_ms as f64 / 1000.0;
        let player = state.player.clone();
        let cues = state.cues.clone();
        let menu_key = key.clone();
        let hover_scrub = scrub.clone();
        let on_hover = on_hover.clone();
        let hit = div()
            .id(("cue-mark", id))
            .size_full()
            .cursor_pointer()
            // The strip's own readout would keep tracking the pointer under
            // the chevron; clearing it here and stopping the move leaves
            // the cue's readout as the only one showing.
            .on_mouse_move(cx.listener(move |_, _: &MouseMoveEvent, _, cx| {
                hover_scrub.set_hover(None);
                cx.stop_propagation();
            }))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                on_hover(this, hovered.then_some(id), cx);
                cx.notify();
            }))
            // A click lands exactly on the cue, not on the pixel under the
            // pointer, and the strip's own seek stays out of it.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_, _: &MouseDownEvent, _, cx| {
                    player.read(cx).seek_to(secs);
                    cx.stop_propagation();
                }),
            )
            // The cue's own menu opens off the window-level handler the
            // wrapper below registers, which runs ahead of this; stopping
            // here keeps the press from the strip's insert menu, which
            // would otherwise drop a second cue on top of this one.
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
            )
            .context_menu(move |menu, _, _| {
                let cues = cues.clone();
                let key = menu_key.clone();
                menu.item(
                    PopupMenuItem::new(rox_i18n::t!("cue-menu-remove"))
                        .icon(Icon::default().path(icons::TRASH))
                        .on_click(move |_, _, cx| {
                            cues.update(cx, |cues, cx| cues.remove(&key, id, cx));
                        }),
                )
            });

        // Each slot carries its own id so the context menus inside them,
        // which all share one, get element state of their own.
        //
        // The top half only, matching where the chevrons hang. The
        // bookmark ribbons keep the bottom half, so a cue and a bookmark
        // closer together than a hit width still each have a column a
        // pointer can land in.
        layer = layer.child(
            div()
                .id(("cue-slot", id))
                .absolute()
                .top_0()
                .bottom(relative(0.5))
                .left(relative(mark.fraction))
                .w(px(HIT_W))
                .ml(px(-HIT_W / 2.0))
                .child(hit),
        );
    }

    if let Some(mark) = hovered.and_then(|id| marks.iter().find(|m| m.id == id)) {
        layer = layer.child(readout(mark));
    }
    layer
}

/// The hovered cue's readout: its time in the seek preview's pill, under
/// the chevron rather than over it, since the chevron is already sitting
/// on the top edge.
fn readout(mark: &CueMark) -> Div {
    div()
        .absolute()
        .bottom(tokens::SPACE_XS)
        .left(relative(mark.fraction))
        .w_0()
        .flex()
        .flex_col()
        .items_center()
        .child(
            div()
                .flex_none()
                .whitespace_nowrap()
                .px(tokens::SPACE_SM)
                .py(px(2.))
                .rounded(tokens::RADIUS)
                .bg(palette::bg_menu_opaque())
                .border_1()
                .border_color(palette::border())
                .text_sm()
                .text_color(palette::text())
                .child(fmt_time(mark.position_ms as f64 / 1000.0)),
        )
}

/// The strip's own right click, over bare track rather than over a mark:
/// drop a cue where the pointer is, or open the bookmark prompt at the
/// same spot. One press, both kinds of mark, because the position under
/// the pointer is the only thing either of them needs and asking twice
/// for it would be silly.
///
/// Both rows grey out while a station plays, since neither has a position
/// to attach to then.
pub fn insert_menu(
    menu: PopupMenu,
    state: AppState,
    key: TrackKey,
    position_ms: u32,
    _window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let cue_label = rox_i18n::t!("cue-menu-insert");
    let bookmark_label = rox_i18n::t!("cue-menu-bookmark");

    if !position_bound::allowed(&state, cx) {
        return menu
            .item(position_bound::locked_item(cue_label, icons::LOCATE))
            .item(position_bound::locked_item(bookmark_label, icons::BOOKMARK));
    }

    let cue_state = state.clone();
    let cue_key = key.clone();
    let menu = menu.item(
        PopupMenuItem::new(cue_label)
            .icon(Icon::default().path(icons::LOCATE))
            .on_click(move |_, _, cx| {
                cue_state
                    .cues
                    .update(cx, |cues, cx| cues.add(&cue_key, position_ms, cx));
            }),
    );

    menu.item(
        PopupMenuItem::new(bookmark_label)
            .icon(Icon::default().path(icons::BOOKMARK))
            .on_click(move |_, _, cx| {
                openers::bookmark_new(state.clone(), key.clone(), position_ms as f64 / 1000.0, cx);
            }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cue(id: u64, position_ms: u32) -> Cue {
        Cue { id, position_ms }
    }

    #[test]
    fn marks_place_along_the_duration_and_need_one() {
        let set = vec![cue(1, 30_000), cue(2, 150_000)];
        let placed = marks(&set, Some(120.0));
        assert_eq!(placed.len(), 2);
        assert!((placed[0].fraction - 0.25).abs() < 1e-6);
        // Past the end clamps onto the strip rather than off it.
        assert_eq!(placed[1].fraction, 1.0);
        assert!(marks(&set, None).is_empty());
        assert!(marks(&set, Some(0.0)).is_empty());
    }

    /// The ids come through untouched: the hit layer and the hover record
    /// both key off them, and a mark that renumbered itself per render
    /// would lose its hover the moment a cue ahead of it was removed.
    #[test]
    fn a_mark_keeps_its_cues_id() {
        let placed = marks(&[cue(7, 10_000)], Some(100.0));
        assert_eq!(placed[0].id, 7);
        assert_eq!(placed[0].position_ms, 10_000);
    }
}
