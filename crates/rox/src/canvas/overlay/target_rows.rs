//! A render target's size rows and its renderer's settings in the canvas
//! inspector (ADR 37, amended 2026-11-26). Size follows the item or is
//! fixed, a following one has a render scale, and either has a fit. Each
//! change is one undo step. A new size mode or scale is a new session; a new
//! fit only moves the picture, and a setting reaches the running renderer as
//! its manifest says.

use gpui::{AnyElement, Context, Div, div, prelude::*};
use rox_canvas::{CanvasItem, CanvasState, Content, ImageFit, TargetSize};
use rox_design::tokens;
use rox_panel_api::panel::{self, setting_block, setting_row};
use serde_json::Value;

use crate::canvas::target_settings::{self, fit_choices, named, scale_choices};
use crate::panels::canvas::CanvasPanel;

/// Edits the target item `id`, if it's still one.
fn edit(
    id: &str,
    f: impl FnOnce(&mut TargetSize, &mut u32, &mut ImageFit, &mut Value) + 'static,
) -> impl FnOnce(&mut CanvasState) + 'static {
    let id = id.to_string();
    move |canvas| {
        if let Some(CanvasItem {
            content:
                Content::Target {
                    sizing,
                    scale,
                    fit,
                    config,
                    ..
                },
            ..
        }) = canvas.find_mut(&id)
        {
            f(sizing, scale, fit, config);
        }
    }
}

/// Size, then the fixed resolution or the render scale, then the fit.
/// `fixed` is the resolution block the inspector already builds.
pub(super) fn sizing(item: &CanvasItem, fixed: Div, cx: &mut Context<CanvasPanel>) -> AnyElement {
    let Content::Target {
        sizing, scale, fit, ..
    } = &item.content
    else {
        return div().into_any_element();
    };

    let modes = [
        (
            rox_i18n::t!("canvas-target-size-follow"),
            TargetSize::Follow,
        ),
        (rox_i18n::t!("canvas-target-panel-fixed"), TargetSize::Fixed),
    ];
    let id = item.id.clone();
    let mode = panel::choices_shared(
        &modes,
        *sizing,
        move |this: &mut CanvasPanel, picked, cx| {
            this.change_later(
                "target.sizing",
                edit(&id, move |sizing, _, _, _| *sizing = picked),
                cx,
            )
        },
        cx,
    );

    let id = item.id.clone();
    let scales = panel::choices_shared(
        &scale_choices(),
        *scale,
        move |this: &mut CanvasPanel, picked, cx| {
            this.change_later(
                "target.scale",
                edit(&id, move |_, scale, _, _| *scale = picked),
                cx,
            )
        },
        cx,
    );

    let id = item.id.clone();
    let fits = panel::choices_shared(
        &fit_choices(),
        *fit,
        move |this: &mut CanvasPanel, picked, cx| {
            this.change_later(
                "target.fit",
                edit(&id, move |_, _, fit, _| *fit = picked),
                cx,
            )
        },
        cx,
    );

    let follows = *sizing == TargetSize::Follow;
    div()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_SM)
        .child(setting_row(
            rox_i18n::t!("canvas-target-size"),
            rox_i18n::try_translate("canvas-target-size.description"),
            mode,
        ))
        .when(!follows, |d| d.child(fixed))
        .when(follows, |d| {
            // Under its label: five chips don't fit beside it at the
            // inspector's width.
            d.child(setting_block(
                rox_i18n::t!("canvas-target-scale"),
                rox_i18n::try_translate("canvas-target-scale.description"),
                None,
                scales,
            ))
        })
        .child(setting_row(
            rox_i18n::t!("canvas-target-fit"),
            rox_i18n::try_translate("canvas-target-fit.description"),
            fits,
        ))
        .into_any_element()
}

/// The settings the target's renderer declares, a row each, or nothing for
/// one that declares none or isn't installed.
pub(super) fn settings(
    panel: &CanvasPanel,
    item: &CanvasItem,
    owner: &str,
    cx: &mut Context<CanvasPanel>,
) -> Option<AnyElement> {
    let Content::Target { config, .. } = &item.content else {
        return None;
    };
    let (_, _, renderers) = panel.inspector.renderers.as_ref()?;
    let renderer = renderers
        .iter()
        .find(|renderer| crate::canvas::target::owner_of(&renderer.id) == owner)?;
    if renderer.settings.is_empty() {
        return None;
    }

    let id = item.id.clone();
    let set =
        move |this: &mut CanvasPanel, key: &str, value: Value, cx: &mut Context<CanvasPanel>| {
            let key = key.to_string();
            this.change_later(
                named("target.setting.", &key),
                edit(&id, move |_, _, _, config| {
                    *config = target_settings::with(config, &key, value);
                }),
                cx,
            )
        };

    let scrubs = &panel.inspector.case_scrubs;
    let mut scrub = |key: &str| {
        scrubs
            .borrow_mut()
            .entry(format!("target-setting:{}:{key}", item.id))
            .or_default()
            .clone()
    };
    let rows = target_settings::setting_rows(
        &renderer.settings,
        config,
        &mut scrub,
        &panel.inspector.edit,
        set,
        cx,
    );
    if rows.is_empty() {
        return None;
    }

    // Each control under its title, which is the package's and can run to
    // any length its manifest allows.
    let body = div().flex().flex_col().gap(tokens::SPACE_SM).children(
        rows.into_iter()
            .map(|row| setting_block(row.title, None, None, row.control)),
    );
    Some(
        setting_block(
            rox_i18n::t!("canvas-target-settings"),
            rox_i18n::try_translate("canvas-target-settings.description"),
            None,
            body,
        )
        .into_any_element(),
    )
}
