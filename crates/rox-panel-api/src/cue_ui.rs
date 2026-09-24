//! Session cues on a strip: the chevrons hanging off the top edge, the hit
//! layer over them, and the strip's right-click menu. The counterpart to
//! [`crate::bookmark_ui`], which draws the persisted marks along the bottom.
//! Top means this listen only, bottom means kept, and the two never share an
//! edge or a shape.

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

#[derive(Clone, Copy)]
pub struct CueMark {
    pub id: u64,
    /// Where along the track, 0 to 1.
    pub fraction: f32,
    pub position_ms: u32,
}

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

/// Same footprint as the bookmark ribbon opposite, so both ends of a strip read
/// as one row of tabs.
pub const MARK_W: f32 = 10.0;
pub const MARK_H: f32 = 6.0;
const MARK_STROKE: f32 = 2.5;
const HIT_W: f32 = 16.0;
const MARK_ALPHA: u8 = 0xe6;

/// `weight` scales the alpha, for a strip fading its shape in or out.
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
    // Inset along the base that keeps the stroke constant across the slope.
    let inset = MARK_STROKE * (half * half + MARK_H * MARK_H).sqrt() / MARK_H;

    for mark in marks {
        let x = mark.fraction.clamp(0.0, 1.0) * w;
        // The outer triangle minus the inner one, as four triangles.
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

/// Seek the playing track to its next cue, or the one before the playhead.
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

/// Take every session mark off the playing track. No position-bound gate:
/// a station can't hold marks, so there's never anything to refuse.
pub fn clear(state: &AppState, cx: &mut App) {
    let Some(now) = state.player.read(cx).now_playing() else {
        return;
    };

    state.cues.update(cx, |cues, cx| cues.clear(&now.key, cx));
}

/// The hit layer over a strip's cues, laid over the strip's own hover layer
/// so a pointer on a chevron reads the cue and not the time under it.
/// `hovered` lives on the panel because it outlives one render.
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
            // Clear the strip's readout and stop the move so only the cue's shows.
            .on_mouse_move(cx.listener(move |_, _: &MouseMoveEvent, _, cx| {
                hover_scrub.set_hover(None);
                cx.stop_propagation();
            }))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                on_hover(this, hovered.then_some(id), cx);
                cx.notify();
            }))
            // Seek exactly to the cue and keep the strip's own seek out of it.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_, _: &MouseDownEvent, _, cx| {
                    player.read(cx).seek_to(secs);
                    cx.stop_propagation();
                }),
            )
            // Stop the press so the strip's insert menu doesn't drop a second
            // cue on this one. The cue's menu opens off a window-level handler
            // that runs first.
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

        // Each slot gets its own id so the context menus inside get their own
        // element state. Top half only: the bookmark ribbons own the bottom,
        // so a close cue and bookmark each keep a column a pointer can hit.
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

/// Sits under the chevron, since the chevron already sits on the top edge.
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

/// The strip's right click over bare track: drop a cue or open the bookmark
/// prompt at the pointer. Both grey out while a station plays.
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
        assert_eq!(placed[1].fraction, 1.0);
        assert!(marks(&set, None).is_empty());
        assert!(marks(&set, Some(0.0)).is_empty());
    }

    /// The hover record keys off the id, so a renumbered mark would lose its
    /// hover when a cue ahead of it was removed.
    #[test]
    fn a_mark_keeps_its_cues_id() {
        let placed = marks(&[cue(7, 10_000)], Some(100.0));
        assert_eq!(placed[0].id, 7);
        assert_eq!(placed[0].position_ms, 10_000);
    }
}
