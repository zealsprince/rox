//! Two rows on a selected item: where it takes the pointer off design mode,
//! and how long its case changes ease. Each pick is one undo step.

use gpui::{AnyElement, Context, IntoElement, SharedString};
use rox_canvas::{CanvasItem, Content, Hit};
use rox_panel_api::panel::{self, setting_row};

use crate::canvas::{fades, states};
use crate::panels::canvas::CanvasPanel;

/// The pointer row. None on a grid, whose panels take their own input.
pub(super) fn hit_row(item: &CanvasItem, cx: &mut Context<CanvasPanel>) -> Option<AnyElement> {
    if matches!(item.content, Content::Grid { .. }) {
        return None;
    }

    let key = |hit: Hit| match hit {
        Hit::Box => "box",
        Hit::Off => "off",
        Hit::Picture => "picture",
    };

    let mut options = vec![
        (key(Hit::Box), rox_i18n::t!("canvas-hit-box")),
        (key(Hit::Off), rox_i18n::t!("canvas-hit-off")),
    ];
    // Only an image has a picture to test against.
    if matches!(item.content, Content::Image { .. }) {
        options.push((key(Hit::Picture), rox_i18n::t!("canvas-hit-picture")));
    }

    let id = item.id.clone();
    let picker = panel::picker(
        "canvas-item-hit",
        key(item.hit),
        options,
        false,
        move |this: &mut CanvasPanel, picked: &'static str, cx| {
            let id = id.clone();
            let hit = match picked {
                "off" => Hit::Off,
                "picture" => Hit::Picture,
                _ => Hit::Box,
            };
            this.change_later(
                "item.hit",
                move |canvas| {
                    if let Some(item) = canvas.find_mut(&id) {
                        item.hit = hit;
                    }
                },
                cx,
            )
        },
        cx,
    );

    Some(
        setting_row(
            rox_i18n::t!("canvas-item-hit"),
            Some(rox_i18n::t!("canvas-item-hit.description")),
            picker,
        )
        .into_any_element(),
    )
}

/// The lengths offered, in milliseconds. A file can hold any other, which
/// the row shows as it is.
const EASES: [u32; 4] = [100, 250, 500, 1000];

fn ms(ms: u32) -> SharedString {
    rox_i18n::t!(
        "canvas-ease-ms",
        ms = rox_i18n::format::format_int(i64::from(ms))
    )
}

/// The ease row, for an item that follows a state.
pub(super) fn ease_row(item: &CanvasItem, cx: &mut Context<CanvasPanel>) -> AnyElement {
    // What leaving it out gives this item: the look's theme ease on a theme
    // item and a cut on the shape, not only the plain default.
    let probe = CanvasItem {
        state: item.state.clone(),
        ..CanvasItem::default()
    };
    let default = states::ease_length(&probe, false).as_millis() as u32;
    let mut options = vec![
        (
            None,
            rox_i18n::t!(
                "canvas-ease-default",
                ms = rox_i18n::format::format_int(i64::from(default))
            ),
        ),
        (Some(0), rox_i18n::t!("canvas-ease-cut")),
    ];
    options.extend(EASES.map(|n| (Some(n), ms(n))));
    if let Some(held) = item.ease.filter(|n| *n != 0 && !EASES.contains(n)) {
        options.push((Some(held), ms(held.min(fades::MAX_EASE_MS))));
    }

    let id = item.id.clone();
    let picker = panel::picker(
        "canvas-item-ease",
        item.ease,
        options,
        false,
        move |this: &mut CanvasPanel, ease: Option<u32>, cx| {
            let id = id.clone();
            this.change_later(
                "item.ease",
                move |canvas| {
                    if let Some(item) = canvas.find_mut(&id) {
                        item.ease = ease;
                    }
                },
                cx,
            )
        },
        cx,
    );

    setting_row(
        rox_i18n::t!("canvas-ease"),
        Some(rox_i18n::t!("canvas-ease.description")),
        picker,
    )
    .into_any_element()
}
