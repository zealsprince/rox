//! Bookmarks on a strip: the ribbons the seek strip and the waveform draw
//! along the bottom edge, the hover readout and right-click menu over each,
//! and the color set behind them. The top edge belongs to [`crate::cue_ui`].

use gpui::{
    App, Bounds, Context, Div, MouseButton, MouseDownEvent, MouseMoveEvent, Path, Pixels, Rgba,
    SharedString, Window, div, prelude::*, px, relative,
};
use gpui_component::Icon;
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use rox_core::fmt::fmt_time;
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_library::bookmarks::Bookmark;
use rox_library::cue::TrackKey;

use crate::openers;
use crate::panel::{AppState, ScrubState};
use crate::position_bound;

/// Mid-saturation hues that hold up on dark and light surfaces. The accent is
/// the default, stored as no color so it follows the palette.
pub const QUICK_COLORS: &[(&str, &str)] = &[
    ("red", "#e5484d"),
    ("orange", "#f76b15"),
    ("yellow", "#f5d90a"),
    ("green", "#30a46c"),
    ("teal", "#12a594"),
    ("blue", "#0090ff"),
    ("purple", "#8e4ec6"),
    ("pink", "#e93d82"),
];

pub fn quick_color_label(key: &str) -> SharedString {
    match key {
        "red" => rox_i18n::t!("bookmark-color-red"),
        "orange" => rox_i18n::t!("bookmark-color-orange"),
        "yellow" => rox_i18n::t!("bookmark-color-yellow"),
        "green" => rox_i18n::t!("bookmark-color-green"),
        "teal" => rox_i18n::t!("bookmark-color-teal"),
        "blue" => rox_i18n::t!("bookmark-color-blue"),
        "purple" => rox_i18n::t!("bookmark-color-purple"),
        "pink" => rox_i18n::t!("bookmark-color-pink"),
        other => SharedString::from(other.to_string()),
    }
}

pub fn color_of(color: Option<&str>) -> Rgba {
    color
        .and_then(palette::parse_hex)
        .unwrap_or_else(palette::accent)
}

#[derive(Clone)]
pub struct Mark {
    pub id: i64,
    /// Where along the track, 0 to 1.
    pub fraction: f32,
    pub position_ms: u32,
    pub name: String,
    pub color: Rgba,
}

pub fn marks(bookmarks: &[Bookmark], duration_secs: Option<f64>) -> Vec<Mark> {
    let Some(duration) = duration_secs.filter(|d| *d > 0.0) else {
        return Vec::new();
    };
    bookmarks
        .iter()
        .map(|b| Mark {
            id: b.id,
            fraction: ((b.position_ms as f64 / 1000.0) / duration).clamp(0.0, 1.0) as f32,
            position_ms: b.position_ms,
            name: b.name.clone(),
            color: color_of(b.color.as_deref()),
        })
        .collect()
}

pub fn mark_label(name: &str, position_ms: u32) -> String {
    let name = name.trim();
    if name.is_empty() {
        fmt_time(position_ms as f64 / 1000.0)
    } else {
        name.to_string()
    }
}

/// Narrow and taller than wide, the bookmark glyph's proportions. A wider tab
/// reads as a flag.
pub const MARK_W: f32 = 6.0;
pub const MARK_H: f32 = 9.0;
const NOTCH_H: f32 = 3.0;
/// Under this the notch closes up and the shape smears, so the strip draws
/// nothing.
const MIN_MARK_H: f32 = 4.0;
const HIT_W: f32 = 16.0;
const MARK_ALPHA: u8 = 0xe6;

/// `weight` scales the alpha, for a strip fading its shape in or out. Paint
/// after the played fill and before the playhead, so the head crosses over a
/// mark it reaches.
pub fn paint_marks(marks: &[Mark], weight: f32, bounds: Bounds<Pixels>, window: &mut Window) {
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    if w <= 0.0 || h <= MIN_MARK_H + 1.0 || marks.is_empty() {
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

    let bottom = h - 1.0;
    // A strip too short for the full height gets a shorter ribbon, notch
    // scaled with it.
    let mark_h = MARK_H.min(bottom);
    let notch_h = NOTCH_H * mark_h / MARK_H;
    let top = bottom - mark_h;
    let half = MARK_W / 2.0;

    for mark in marks {
        let x = mark.fraction.clamp(0.0, 1.0) * w;
        // A pentagon: the rectangle with the notch's point pushed up between
        // the bottom corners.
        let tl = at(x - half, top);
        let tr = at(x + half, top);
        let br = at(x + half, bottom);
        let bl = at(x - half, bottom);
        let notch = at(x, bottom - notch_h);

        let mut path = Path::new(tl);
        path.push_triangle((tl, tr, br), solid);
        path.push_triangle((tl, br, notch), solid);
        path.push_triangle((tl, notch, bl), solid);
        window.paint_path(path, palette::alpha(mark.color, alpha));
    }
}

/// So a second Previous steps past the mark just landed on, the way Previous
/// on a track works.
const PREV_GRACE_SECS: f64 = 1.5;

/// Seek the playing track to its next bookmark, or the one before the playhead.
pub fn step(state: &AppState, forward: bool, cx: &mut App) {
    if !position_bound::allowed(state, cx) {
        return;
    }
    let Some(now) = state.player.read(cx).now_playing() else {
        return;
    };
    let marks = state.library.read(cx).bookmarks_for(&now.key);
    let target = step_target(
        marks.iter().map(|m| m.position_ms as f64 / 1000.0),
        now.position_secs,
        forward,
    );
    if let Some(secs) = target {
        state.player.read(cx).seek_to(secs);
    }
}

fn step_target(marks: impl Iterator<Item = f64>, at: f64, forward: bool) -> Option<f64> {
    if forward {
        marks.filter(|&secs| secs > at + 0.05).reduce(f64::min)
    } else {
        marks
            .filter(|&secs| secs < at - PREV_GRACE_SECS)
            .reduce(f64::max)
    }
}

/// The hit layer over a strip's marks, laid over the strip's own hover layer
/// so a pointer on a ribbon reads the mark and not the time under it.
/// `hovered` lives on the panel because it outlives one render.
#[allow(clippy::too_many_arguments)]
pub fn overlay<V: 'static>(
    state: &AppState,
    key: &TrackKey,
    marks: &[Mark],
    hovered: Option<i64>,
    scrub: &ScrubState,
    on_hover: impl Fn(&mut V, Option<i64>, &mut Context<V>) + Clone + 'static,
    cx: &mut Context<V>,
) -> Div {
    let mut layer = div().absolute().inset_0();
    for mark in marks {
        let id = mark.id;
        let secs = mark.position_ms as f64 / 1000.0;
        let player = state.player.clone();
        let menu_state = state.clone();
        let menu_key = key.clone();
        let hover_scrub = scrub.clone();
        let on_hover = on_hover.clone();
        let hit = div()
            .id(("bookmark-mark", id as u64))
            .size_full()
            .cursor_pointer()
            // Clear the strip's readout and stop the move so only the mark's shows.
            .on_mouse_move(cx.listener(move |_, _: &MouseMoveEvent, _, cx| {
                hover_scrub.set_hover(None);
                cx.stop_propagation();
            }))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                on_hover(this, hovered.then_some(id), cx);
                cx.notify();
            }))
            // Seek exactly to the mark and keep the strip's own seek out of it.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_, _: &MouseDownEvent, _, cx| {
                    player.read(cx).seek_to(secs);
                    cx.stop_propagation();
                }),
            )
            // Stop the press so the dock's body handler doesn't open the panel
            // dropdown. The mark's menu opens off a window-level handler that
            // runs first.
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
            )
            .context_menu(move |menu, window, cx| {
                menu_for(menu, menu_state.clone(), menu_key.clone(), id, window, cx)
            });
        // Each slot gets its own id so the context menus inside get their own
        // element state. Bottom half only: a full-height column would swallow
        // the cue slots in the top half.
        layer = layer.child(
            div()
                .id(("bookmark-slot", id as u64))
                .absolute()
                .top(relative(0.5))
                .bottom_0()
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

fn readout(mark: &Mark) -> Div {
    let time = fmt_time(mark.position_ms as f64 / 1000.0);
    let name = mark.name.trim();
    div()
        .absolute()
        .top(tokens::SPACE_XS)
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
                .flex()
                .flex_col()
                .items_center()
                .when(!name.is_empty(), |d| d.child(name.to_string()))
                .child(
                    div()
                        .when(!name.is_empty(), |d| {
                            d.text_xs().text_color(palette::text_muted())
                        })
                        .child(time),
                ),
        )
}

/// Shared by the strips and the bookmarks panel. A caller with a play row of
/// its own puts that ahead of this.
pub fn menu_for(
    menu: PopupMenu,
    state: AppState,
    key: TrackKey,
    id: i64,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let rename_state = state.clone();
    let menu = menu.item(
        PopupMenuItem::new(rox_i18n::t!("bookmark-menu-rename"))
            .icon(Icon::default().path(icons::PENCIL))
            .on_click(move |_, _, cx| openers::bookmark_edit(rename_state.clone(), id, cx)),
    );
    let menu = color_submenu(menu, state.clone(), vec![id], window, cx);
    // Move only offers itself while the mark's track is playing, since the
    // playhead is the target. A station shows it greyed so the lockout
    // explains itself.
    let move_label = rox_i18n::t!("bookmark-menu-move");
    let menu = if !position_bound::allowed(&state, cx) {
        menu.item(position_bound::locked_item(move_label, icons::LOCATE))
    } else {
        let playing = state
            .player
            .read(cx)
            .now_playing()
            .filter(|now| now.key == key);
        match playing {
            Some(now) => {
                let move_state = state.clone();
                let position_ms = (now.position_secs.max(0.0) * 1000.0).round() as u32;
                menu.item(
                    PopupMenuItem::new(move_label)
                        .icon(Icon::default().path(icons::LOCATE))
                        .on_click(move |_, _, cx| {
                            move_state.library.update(cx, |library, cx| {
                                library.move_bookmark(id, position_ms, cx)
                            });
                        }),
                )
            }
            None => menu,
        }
    };

    let remove_state = state;
    menu.separator().item(
        PopupMenuItem::new(rox_i18n::t!("bookmark-menu-remove"))
            .icon(Icon::default().path(icons::TRASH))
            .on_click(move |_, _, cx| {
                remove_state
                    .library
                    .update(cx, |library, cx| library.remove_bookmark(id, cx));
            }),
    )
}

/// A pick lands on every id at once. The custom picker only shows for a
/// single mark.
pub fn color_submenu(
    menu: PopupMenu,
    state: AppState,
    ids: Vec<i64>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    menu.submenu_with_icon(
        Some(Icon::default().path(icons::PALETTE)),
        rox_i18n::t!("bookmark-menu-color"),
        window,
        cx,
        move |menu, _, cx| color_menu(menu, state.clone(), ids.clone(), cx),
    )
}

fn color_menu(menu: PopupMenu, state: AppState, ids: Vec<i64>, cx: &App) -> PopupMenu {
    let current = ids
        .first()
        .and_then(|&id| state.library.read(cx).bookmark(id))
        .and_then(|b| b.color)
        .map(|c| c.to_ascii_lowercase());
    let recolor = |state: &AppState, color: Option<&'static str>, ids: &[i64], cx: &mut App| {
        state.library.update(cx, |library, cx| {
            for &id in ids {
                library.set_bookmark_color(id, color, cx);
            }
        });
    };
    let accent_state = state.clone();
    let accent_ids = ids.clone();
    let mut menu = menu.item(
        PopupMenuItem::new(rox_i18n::t!("bookmark-color-accent"))
            .checked(current.is_none())
            .on_click(move |_, _, cx| recolor(&accent_state, None, &accent_ids, cx)),
    );
    for (key, hex) in QUICK_COLORS {
        let pick_state = state.clone();
        let pick_ids = ids.clone();
        menu = menu.item(
            PopupMenuItem::new(quick_color_label(key))
                .checked(current.as_deref() == Some(*hex))
                .on_click(move |_, _, cx| recolor(&pick_state, Some(hex), &pick_ids, cx)),
        );
    }
    // The picker edits one mark's row, so a set gets the quick picks only.
    let [id] = ids[..] else {
        return menu;
    };
    let custom_state = state;
    menu.separator().item(
        PopupMenuItem::new(rox_i18n::t!("bookmark-color-custom"))
            .checked(current.is_some_and(|c| !QUICK_COLORS.iter().any(|(_, hex)| *hex == c)))
            .on_click(move |_, _, cx| openers::bookmark_edit(custom_state.clone(), id, cx)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(position_ms: u32, name: &str, color: Option<&str>) -> Bookmark {
        Bookmark {
            id: position_ms as i64,
            track_id: 1,
            position_ms,
            name: name.into(),
            color: color.map(str::to_string),
            created: 0,
        }
    }

    #[test]
    fn marks_place_along_the_duration_and_need_one() {
        let set = vec![
            mark(30_000, "", None),
            mark(150_000, "Drop", Some("#0090ff")),
        ];
        let placed = marks(&set, Some(120.0));
        assert_eq!(placed.len(), 2);
        assert!((placed[0].fraction - 0.25).abs() < 1e-6);
        assert_eq!(placed[1].fraction, 1.0);
        assert!(marks(&set, None).is_empty());
        assert!(marks(&set, Some(0.0)).is_empty());
    }

    #[test]
    fn a_nameless_mark_is_called_by_its_time() {
        assert_eq!(mark_label("", 65_000), "1:05");
        assert_eq!(mark_label("  Chorus ", 65_000), "Chorus");
    }

    #[test]
    fn a_step_finds_the_neighbouring_mark_with_a_grace_going_back() {
        let marks = [10.0, 40.0, 70.0];
        let step = |at, forward| step_target(marks.iter().copied(), at, forward);
        assert_eq!(step(0.0, true), Some(10.0));
        assert_eq!(step(40.0, true), Some(70.0));
        assert_eq!(step(70.0, true), None);
        // Just past a mark, Previous goes to it; landed on it, to the one before.
        assert_eq!(step(45.0, false), Some(40.0));
        assert_eq!(step(40.5, false), Some(10.0));
        assert_eq!(step(5.0, false), None);
    }

    #[test]
    fn a_bad_color_falls_back_to_the_accent() {
        let accent = palette::accent();
        let read = |c: Option<&str>| {
            let c = color_of(c);
            (c.r, c.g, c.b)
        };
        assert_eq!(read(None), (accent.r, accent.g, accent.b));
        assert_eq!(read(Some("nonsense")), (accent.r, accent.g, accent.b));
        assert_ne!(read(Some("#0090ff")), (accent.r, accent.g, accent.b));
    }
}
