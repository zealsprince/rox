//! An image item's nine-slice rows: the four insets in the image's pixels,
//! and a small copy of the image with the cut drawn over it once the image
//! has decoded. Each inset is one undo step. All four at zero is no slice.

use gpui::{AnyElement, Context, ObjectFit, SharedString, div, img, prelude::*, px};
use rox_canvas::{CanvasItem, Content};
use rox_design::{palette, tokens};
use rox_panel_api::panel::{setting_block, setting_row};

use super::number;
use crate::canvas::slice;
use crate::panels::canvas::CanvasPanel;

/// The preview's longest side, in pixels.
const PREVIEW: f32 = 160.;

pub(super) fn rows(
    panel: &CanvasPanel,
    id: &str,
    asset: &str,
    insets: Option<[u32; 4]>,
    cx: &mut Context<CanvasPanel>,
) -> AnyElement {
    let shown = insets.unwrap_or([0; 4]);
    let sides = [
        rox_i18n::t!("canvas-image-slice-left"),
        rox_i18n::t!("canvas-image-slice-top"),
        rox_i18n::t!("canvas-image-slice-right"),
        rox_i18n::t!("canvas-image-slice-bottom"),
    ];

    let mut fields = div().flex().flex_col().gap(px(2.));
    for (side, label) in sides.into_iter().enumerate() {
        let setter = id.to_string();
        let value = number(
            panel,
            &format!("slice{side}:{id}"),
            shown[side] as f32,
            true,
            move |canvas, v| set_inset(canvas, &setter, side, v),
            cx,
        );
        fields = fields.child(rox_panel_kit::ui::nested(setting_row(label, None, value)));
    }

    let body = div()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_SM)
        .child(fields)
        .when_some(preview(panel, asset, shown), |d, preview| d.child(preview));

    div()
        .id(SharedString::from(format!("canvas-slice:{id}")))
        .child(setting_block(
            rox_i18n::t!("canvas-image-slice"),
            Some(rox_i18n::t!("canvas-image-slice.description")),
            None,
            body,
        ))
        .into_any_element()
}

fn set_inset(canvas: &mut rox_canvas::CanvasState, id: &str, side: usize, value: f32) {
    let Some(CanvasItem {
        content: Content::Image { slice, .. },
        ..
    }) = canvas.find_mut(id)
    else {
        return;
    };

    let mut insets = slice.unwrap_or([0; 4]);
    insets[side] = value.round().clamp(0., u32::MAX as f32) as u32;
    *slice = insets.iter().any(|n| *n > 0).then_some(insets);
}

/// The image at a small size with a line at each inset, held to the image
/// the way the canvas holds them. None until the image has decoded.
fn preview(panel: &CanvasPanel, asset: &str, insets: [u32; 4]) -> Option<AnyElement> {
    let image = panel.images().ready(asset)?;
    let pixels = image.size(0);
    let texels = [pixels.width.0.max(1) as u32, pixels.height.0.max(1) as u32];

    let k = PREVIEW / texels[0].max(texels[1]) as f32;
    let size = [texels[0] as f32 * k, texels[1] as f32 * k];
    let [left, top, right, bottom] = slice::held(insets, texels).map(|n| n as f32 * k);

    let line = |vertical: bool, at: f32| {
        let d = div().absolute().bg(palette::accent());
        match vertical {
            true => d.top_0().bottom_0().w(px(1.)).left(px(at)),
            false => d.left_0().right_0().h(px(1.)).top(px(at)),
        }
    };

    Some(
        div()
            .relative()
            .w(px(size[0]))
            .h(px(size[1]))
            .bg(palette::bg_control())
            .child(
                img(gpui::ImageSource::Render(image))
                    .size_full()
                    .object_fit(ObjectFit::Fill),
            )
            .when(left > 0., |d| d.child(line(true, left)))
            .when(right > 0., |d| d.child(line(true, size[0] - right)))
            .when(top > 0., |d| d.child(line(false, top)))
            .when(bottom > 0., |d| d.child(line(false, size[1] - bottom)))
            .into_any_element(),
    )
}
