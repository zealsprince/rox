//! The scene inspector (ADR 33): a panel floating over the scene in design
//! mode, along its right or left edge, with the item tree and every number
//! the selected item has. With nothing selected it holds the stage and the
//! camera. The panel's own chrome stays in its settings window; this holds
//! only the scene.
//!
//! Every edit here goes through [`ScenePanel::change`], so it shows live,
//! lands in undo and saves. Each kind of content adds its rows in a
//! function of its own below [`content_rows`].

use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, Context, ElementId, Entity, Focusable, KeyContext, MouseButton, Render,
    SharedString, Subscription, Window, div, prelude::*, px, svg,
};
use gpui_component::Sizable as _;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel::shader as surface;
use rox_panel_api::panel::{
    self, PanelSettings, ScrubState, Tone, ValueEdit, setting_block, setting_row,
};
use rox_panel_api::panel_settings::{self, ShaderSource};
use rox_panel_api::signal_ui::{routes::RouteEditor, slots::SlotList};
use rox_panel_kit::ui as settings_ui;
use rox_scene::{Anchor, Content, Fit, ImageFit, Length, SceneItem, SceneState};

use crate::panels::scene::{self, ScenePanel};
use crate::scene::edit;
use crate::scene::shader::{self as shader_item, RowState};

const WIDTH: f32 = 320.;

/// Each level of the item tree steps in this far.
const INDENT: f32 = 12.;

/// Rotation sliders run a half turn each way; a typed value past that
/// clamps.
const TURN_RANGE: f32 = 180.;

const SCALE_MIN: f32 = 0.1;
const SCALE_MAX: f32 = 4.;
const FOV_MAX: f32 = 120.;
const PARALLAX_MAX: f32 = 200.;

/// A compile message shows this many of its lines.
const ERROR_LINES: usize = 6;

#[derive(Default)]
pub(crate) struct Inspector {
    left: bool,
    /// The one number or name being typed, by field.
    typing: Option<Typing>,
    edit: ValueEdit,
    scrubs: Scrubs,
    shader: RowState,
}

impl Inspector {
    pub(crate) fn on_left(&self) -> bool {
        self.left
    }
}

struct Typing {
    field: String,
    input: Entity<InputState>,
    _events: Subscription,
}

#[derive(Default)]
struct Scrubs {
    rotation: [ScrubState; 3],
    scale: ScrubState,
    opacity: ScrubState,
    fov: ScrubState,
    parallax: ScrubState,
}

/// A tree row on its way onto a group.
#[derive(Clone)]
struct RowDrag {
    id: String,
    label: SharedString,
}

struct RowDragView(SharedString);

impl Render for RowDragView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(tokens::SPACE_SM)
            .py(px(2.))
            .rounded(tokens::RADIUS)
            .bg(palette::bg_control())
            .text_xs()
            .child(self.0.clone())
    }
}

/// The inspector, absolutely placed over the scene. It takes every press,
/// wheel and hover over it, so nothing reaches the stage underneath.
pub(crate) fn render(
    panel: &mut ScenePanel,
    window: &mut Window,
    cx: &mut Context<ScenePanel>,
) -> AnyElement {
    let left = panel.inspector.left;

    // A press on one of these controls takes focus off the scene, so the
    // inspector carries the scene's keys too. A field typing here sits deeper
    // and keeps its own.
    let mut context = KeyContext::default();
    context.add(scene::KEY_CONTEXT);
    if panel.selected.is_some() {
        context.add(scene::SELECTED_CONTEXT);
    }

    let shell = div()
        .id("scene-inspector")
        .key_context(context)
        .absolute()
        .top(tokens::SPACE_SM)
        .when(left, |d| d.left(tokens::SPACE_SM))
        .when(!left, |d| d.right(tokens::SPACE_SM))
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
        .rounded(tokens::RADIUS)
        .bg(palette::bg_menu_opaque())
        .border_1()
        .border_color(palette::border())
        .text_sm()
        .text_color(palette::text());

    let height = (f32::from(panel_height(panel)) - 16.).max(120.);
    let selected = panel
        .selected
        .as_ref()
        .and_then(|id| panel.config.scene.find(id))
        .cloned();

    let body = div()
        .id("scene-inspector-body")
        .flex()
        .flex_col()
        .gap(tokens::SPACE_SM)
        .p(tokens::SPACE_SM)
        .overflow_y_scroll()
        .flex_1()
        .min_h_0()
        .child(tree(panel, cx))
        .child(div().h(px(1.)).bg(palette::border()))
        .child(match &selected {
            Some(item) => item_rows(panel, item, window, cx),
            None => stage_rows(panel, cx),
        });

    shell
        .w(px(WIDTH))
        .max_h(px(height))
        .flex()
        .flex_col()
        .child(header(panel, cx))
        .child(body)
        .into_any_element()
}

fn panel_height(panel: &ScenePanel) -> gpui::Pixels {
    px(panel.viewport().height)
}

fn header(panel: &ScenePanel, cx: &mut Context<ScenePanel>) -> AnyElement {
    let me = cx.entity().downgrade();
    let on = |f: fn(&mut ScenePanel, &mut Window, &mut Context<ScenePanel>)| {
        let me = me.clone();
        move |_: &settings_ui::Press, window: &mut Window, cx: &mut gpui::App| {
            me.update(cx, |this, cx| f(this, window, cx)).ok();
        }
    };

    let side = if panel.inspector.left {
        icons::PANEL_RIGHT
    } else {
        icons::PANEL_LEFT
    };

    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_XS)
        .px(tokens::SPACE_SM)
        .py(tokens::SPACE_XS)
        .border_b_1()
        .border_color(palette::border())
        .child(
            div()
                .flex_1()
                .truncate()
                .text_color(palette::text_muted())
                .child(rox_i18n::t!("scene-inspector-title")),
        )
        .child(
            settings_ui::icon_button(icons::PLUS, false, {
                let me = me.clone();
                move |_, window, cx| {
                    let position = window.mouse_position();
                    me.update(cx, |this, cx| {
                        this.open_add_menu(position, None, window, cx)
                    })
                    .ok();
                }
            })
            .keyed("scene-inspector-add"),
        )
        .child(
            settings_ui::icon_button(icons::ARROW_LEFT, !panel.can_undo(), on(ScenePanel::undo))
                .keyed("scene-inspector-undo"),
        )
        .child(
            settings_ui::icon_button(icons::ARROW_RIGHT, !panel.can_redo(), on(ScenePanel::redo))
                .keyed("scene-inspector-redo"),
        )
        .child(
            settings_ui::icon_button(
                side,
                false,
                on(|this, _, cx| {
                    this.inspector.left = !this.inspector.left;
                    cx.notify();
                }),
            )
            .keyed("scene-inspector-side"),
        )
        .child(
            settings_ui::icon_button(icons::MINIMIZE, false, on(|this, _, cx| this.put_away(cx)))
                .keyed("scene-inspector-collapse"),
        )
        .into_any_element()
}

// ── The item tree ──

fn tree(panel: &ScenePanel, cx: &mut Context<ScenePanel>) -> AnyElement {
    let mut rows = Vec::new();
    panel
        .config
        .scene
        .walk(&mut |item, level| rows.push((item.clone(), level)));

    // The heading takes a row dropped on it back to the top level.
    let heading = div()
        .id("scene-tree-top")
        .px(tokens::SPACE_XS)
        .rounded(tokens::RADIUS)
        .text_xs()
        .text_color(palette::text_muted())
        .child(rox_i18n::t!("scene-inspector-items"))
        .drag_over::<RowDrag>(|style, _, _, _| style.bg(palette::alpha(palette::accent(), 0x1a)))
        .on_drop(cx.listener(|this, drag: &RowDrag, window, cx| {
            this.move_into(&drag.id, None, window, cx);
        }));

    let list = rows
        .into_iter()
        .enumerate()
        .map(|(ix, (item, level))| tree_row(panel, ix, &item, level, cx));

    div()
        .flex()
        .flex_col()
        .gap(px(1.))
        .child(heading)
        .children(list)
        .into_any_element()
}

fn tree_row(
    panel: &ScenePanel,
    ix: usize,
    item: &SceneItem,
    level: usize,
    cx: &mut Context<ScenePanel>,
) -> AnyElement {
    let id = item.id.clone();
    let selected = panel.selected.as_deref() == Some(item.id.as_str());
    let is_group = matches!(item.content, Content::Group { .. });
    let label = SharedString::from(if item.name.is_empty() {
        item.id.clone()
    } else {
        item.name.clone()
    });
    let field = format!("name:{id}");

    let name: AnyElement = match typing_input(panel, &field) {
        Some(input) => div()
            .flex_1()
            .min_w_0()
            .child(typing_field(&input))
            .into_any_element(),
        None => div()
            .flex_1()
            .min_w_0()
            .truncate()
            .when(!item.visible, |d| d.text_color(palette::text_faint()))
            .child(label.clone())
            .into_any_element(),
    };

    let me = cx.entity().downgrade();
    let toggle = |icon: &'static str, key: &str, flip: fn(&mut SceneItem)| {
        let me = me.clone();
        let id = id.clone();
        settings_ui::icon_button(icon, false, move |_, window, cx| {
            let id = id.clone();
            me.update(cx, |this, cx| {
                this.change(
                    |scene| {
                        if let Some(item) = scene.find_mut(&id) {
                            flip(item);
                        }
                    },
                    window,
                    cx,
                )
            })
            .ok();
        })
        .keyed(ElementId::Name(format!("{key}:{}", item.id).into()))
    };

    let row = div()
        .id(("scene-tree-row", ix))
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_XS)
        .pl(px(level as f32 * INDENT + 4.))
        .pr(px(2.))
        .rounded(tokens::RADIUS)
        .when(selected, |d| d.bg(palette::alpha(palette::accent(), 0x30)))
        .hover(|d| d.bg(palette::bg_control_hover()))
        .child(
            svg()
                .path(kind_icon(&item.content))
                .size(px(12.))
                .flex_none()
                .text_color(palette::text_muted()),
        )
        .child(name)
        .child(toggle(
            if item.visible {
                icons::EYE
            } else {
                icons::EYE_OFF
            },
            "visible",
            |item| item.visible = !item.visible,
        ))
        .child(toggle(
            if item.locked {
                icons::LOCK
            } else {
                icons::LOCK_OPEN
            },
            "locked",
            |item| item.locked = !item.locked,
        ))
        .on_mouse_down(MouseButton::Left, {
            let id = id.clone();
            let name = item.name.clone();
            cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                if event.click_count > 1 {
                    let id = id.clone();
                    begin_typing(
                        this,
                        format!("name:{id}"),
                        name.clone(),
                        Rc::new(move |scene: &mut SceneState, text: &str| {
                            if let Some(item) = scene.find_mut(&id) {
                                item.name = text.trim().to_string();
                            }
                        }),
                        window,
                        cx,
                    );
                    return;
                }

                this.select(Some(id.clone()), window, cx);
            })
        })
        .on_drag(
            RowDrag {
                id: id.clone(),
                label: label.clone(),
            },
            |drag, _, _, cx| cx.new(|_| RowDragView(drag.label.clone())),
        );

    if !is_group {
        return row.into_any_element();
    }

    let group = id.clone();
    row.drag_over::<RowDrag>(|style, _, _, _| style.bg(palette::alpha(palette::accent(), 0x1a)))
        .on_drop(cx.listener(move |this, drag: &RowDrag, window, cx| {
            if drag.id != group {
                this.move_into(&drag.id, Some(&group), window, cx);
            }
        }))
        .into_any_element()
}

fn kind_icon(content: &Content) -> &'static str {
    match content {
        Content::Grid { .. } => icons::LAYOUT_GRID,
        Content::Group { .. } => icons::LAYERS,
        Content::Image { .. } => icons::IMAGE,
        Content::Shader { .. } => icons::BLEND,
        _ => icons::SQUARE_DASHED,
    }
}

// ── With nothing selected ──

fn stage_rows(panel: &ScenePanel, cx: &mut Context<ScenePanel>) -> AnyElement {
    let scene = &panel.config.scene;
    let stage = scene.stage;
    let camera = scene.camera;
    let scrubs = &panel.inspector.scrubs;

    let fits = [
        (rox_i18n::t!("scene-fit-anchor"), Fit::Anchor),
        (rox_i18n::t!("scene-fit-contain"), Fit::Contain),
        (rox_i18n::t!("scene-fit-cover"), Fit::Cover),
    ];

    div()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_SM)
        .child(section_label(rox_i18n::t!("scene-stage")))
        .child(setting_row(
            rox_i18n::t!("scene-stage-size"),
            None,
            div()
                .flex()
                .gap(tokens::SPACE_XS)
                .child(number(
                    panel,
                    "stage.w",
                    stage.width,
                    false,
                    |s, v| s.stage.width = v.max(1.),
                    cx,
                ))
                .child(number(
                    panel,
                    "stage.h",
                    stage.height,
                    false,
                    |s, v| s.stage.height = v.max(1.),
                    cx,
                )),
        ))
        .child(setting_row(
            rox_i18n::t!("scene-stage-fit"),
            None,
            panel::choices_shared(
                &fits,
                stage.fit,
                |this: &mut ScenePanel, fit, cx| {
                    this.change_later("stage.fit", move |s| s.stage.fit = fit, cx)
                },
                cx,
            ),
        ))
        .child(section_label(rox_i18n::t!("scene-camera")))
        .child(setting_row(
            rox_i18n::t!("scene-camera-fov"),
            None,
            panel::value_slider_edit(
                &scrubs.fov,
                &panel.inspector.edit,
                camera.fov / FOV_MAX,
                format!("{}°", whole(camera.fov)),
                format!("{:.0}", camera.fov),
                |v| v / FOV_MAX,
                |this: &mut ScenePanel, f, cx| {
                    this.change_later("camera.fov", move |s| s.camera.fov = f * FOV_MAX, cx)
                },
                cx,
            ),
        ))
        .child(setting_row(
            rox_i18n::t!("scene-camera-parallax"),
            None,
            panel::value_slider_edit(
                &scrubs.parallax,
                &panel.inspector.edit,
                camera.parallax / PARALLAX_MAX,
                format!("{:.0}", camera.parallax),
                format!("{:.0}", camera.parallax),
                |v| v / PARALLAX_MAX,
                |this: &mut ScenePanel, f, cx| {
                    this.change_later(
                        "camera.parallax",
                        move |s| s.camera.parallax = f * PARALLAX_MAX,
                        cx,
                    )
                },
                cx,
            ),
        ))
        .into_any_element()
}

// ── With an item selected ──

fn item_rows(
    panel: &ScenePanel,
    item: &SceneItem,
    window: &mut Window,
    cx: &mut Context<ScenePanel>,
) -> AnyElement {
    let flat = panel.is_flat(item, cx);
    let place = item.place;
    let id = item.id.clone();
    let scrubs = &panel.inspector.scrubs;
    let viewport = panel.viewport();

    // Setters for one field of the selected item, by id.
    let on_item = |f: fn(&mut SceneItem, f32)| {
        let id = id.clone();
        move |scene: &mut SceneState, v: f32| {
            if let Some(item) = scene.find_mut(&id) {
                f(item, v);
            }
        }
    };

    let mut axis = |label: &str, ix: usize, f: fn(&mut SceneItem, f32)| {
        div().flex_1().min_w_0().child(number(
            panel,
            &format!("{label}:{id}"),
            place.position[ix],
            false,
            on_item(f),
            cx,
        ))
    };
    let position = div()
        .flex()
        .gap(tokens::SPACE_XS)
        .child(axis("x", 0, |i, v| i.place.position[0] = v))
        .child(axis("y", 1, |i, v| i.place.position[1] = v))
        .child(axis("z", 2, |i, v| i.place.position[2] = v));

    let mut rows = div()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_SM)
        .child(setting_row(
            rox_i18n::t!("scene-item-anchor"),
            None,
            anchor_picker(&id, place.anchor, viewport, cx),
        ))
        .child(setting_block(
            rox_i18n::t!("scene-item-position"),
            None,
            None,
            position,
        ))
        .child(length_row(panel, &id, 0, place.size[0], viewport, cx))
        .child(length_row(panel, &id, 1, place.size[1], viewport, cx));

    let axes = [
        rox_i18n::t!("scene-item-rotation-x"),
        rox_i18n::t!("scene-item-rotation-y"),
        rox_i18n::t!("scene-item-rotation-z"),
    ];
    for (axis, label) in axes.into_iter().enumerate() {
        let degrees = place.rotation[axis];
        let readout = format!("{}°", whole(degrees));

        rows = rows.child(if flat {
            setting_row(label, None, inert_readout(readout))
        } else {
            let id = id.clone();
            setting_row(
                label,
                None,
                panel::value_slider_edit(
                    &scrubs.rotation[axis],
                    &panel.inspector.edit,
                    (degrees + TURN_RANGE) / (2. * TURN_RANGE),
                    readout,
                    format!("{degrees:.1}"),
                    |v| (v + TURN_RANGE) / (2. * TURN_RANGE),
                    move |this: &mut ScenePanel, f, cx| {
                        let id = id.clone();
                        let degrees = f * 2. * TURN_RANGE - TURN_RANGE;
                        this.change_later(
                            ["rotation.x", "rotation.y", "rotation.z"][axis],
                            move |s| {
                                if let Some(item) = s.find_mut(&id) {
                                    item.place.rotation[axis] = degrees;
                                }
                            },
                            cx,
                        )
                    },
                    cx,
                ),
            )
        });
    }

    let scale_readout = format!("{:.2}", place.scale);
    rows = rows.child(if flat {
        setting_row(
            rox_i18n::t!("scene-item-scale"),
            None,
            inert_readout(scale_readout),
        )
    } else {
        let id = id.clone();
        setting_row(
            rox_i18n::t!("scene-item-scale"),
            None,
            panel::value_slider_edit(
                &scrubs.scale,
                &panel.inspector.edit,
                (place.scale - SCALE_MIN) / (SCALE_MAX - SCALE_MIN),
                scale_readout,
                format!("{:.2}", place.scale),
                |v| (v - SCALE_MIN) / (SCALE_MAX - SCALE_MIN),
                move |this: &mut ScenePanel, f, cx| {
                    let id = id.clone();
                    this.change_later(
                        "scale",
                        move |s| {
                            if let Some(item) = s.find_mut(&id) {
                                item.place.scale = SCALE_MIN + f * (SCALE_MAX - SCALE_MIN);
                            }
                        },
                        cx,
                    )
                },
                cx,
            ),
        )
    });

    if flat {
        // A shader is flat only because of what its program reads.
        let note = if matches!(item.content, Content::Shader { .. }) {
            rox_i18n::t!("scene-shader-flat-note")
        } else {
            rox_i18n::t!("scene-item-flat-note")
        };
        rows = rows.child(
            div()
                .text_xs()
                .text_color(palette::text_muted())
                .child(note),
        );
    }

    let opacity_id = id.clone();
    rows = rows.child(setting_row(
        rox_i18n::t!("scene-item-opacity"),
        None,
        panel::value_slider_edit(
            &scrubs.opacity,
            &panel.inspector.edit,
            item.opacity,
            format!("{:.0}%", item.opacity * 100.),
            format!("{:.0}", item.opacity * 100.),
            |v| v / 100.,
            move |this: &mut ScenePanel, f, cx| {
                let id = opacity_id.clone();
                this.change_later(
                    "opacity",
                    move |s| {
                        if let Some(item) = s.find_mut(&id) {
                            item.opacity = f;
                        }
                    },
                    cx,
                )
            },
            cx,
        ),
    ));

    rows.child(actions(item, cx))
        .child(content_rows(panel, item, window, cx))
        .into_any_element()
}

/// Depth, grouping and delete, as buttons.
fn actions(item: &SceneItem, cx: &mut Context<ScenePanel>) -> AnyElement {
    let me = cx.entity().downgrade();
    let button = |label: SharedString,
                  icon: &'static str,
                  key: &'static str,
                  f: fn(&mut ScenePanel, &mut Window, &mut Context<ScenePanel>)| {
        let me = me.clone();
        settings_ui::small_button(label, icon, false, move |_, window, cx| {
            me.update(cx, |this, cx| f(this, window, cx)).ok();
        })
        .keyed(key)
    };

    let is_group = matches!(item.content, Content::Group { .. });

    div()
        .flex()
        .flex_row()
        .flex_wrap()
        .gap(tokens::SPACE_XS)
        .child(button(
            rox_i18n::t!("scene-bring-forward"),
            icons::ARROW_UP,
            "scene-forward",
            |this, window, cx| this.step_depth(true, window, cx),
        ))
        .child(button(
            rox_i18n::t!("scene-send-back"),
            icons::ARROW_DOWN,
            "scene-back",
            |this, window, cx| this.step_depth(false, window, cx),
        ))
        .child(if is_group {
            button(
                rox_i18n::t!("scene-ungroup"),
                icons::LAYERS,
                "scene-ungroup",
                ScenePanel::ungroup_selected,
            )
        } else {
            button(
                rox_i18n::t!("scene-group"),
                icons::LAYERS,
                "scene-group",
                ScenePanel::group_selected,
            )
        })
        .child(button(
            rox_i18n::t!("scene-delete"),
            icons::TRASH,
            "scene-delete",
            ScenePanel::delete_selected,
        ))
        .into_any_element()
}

/// The content's own rows. A grid and a group have none.
fn content_rows(
    panel: &ScenePanel,
    item: &SceneItem,
    window: &mut Window,
    cx: &mut Context<ScenePanel>,
) -> AnyElement {
    match &item.content {
        Content::Image { asset, fit } => image_rows(panel, &item.id, asset, *fit, window, cx),
        Content::Shader { .. } => shader_rows(panel, item, window, cx),
        _ => div().into_any_element(),
    }
}

fn image_rows(
    _panel: &ScenePanel,
    id: &str,
    asset: &str,
    fit: ImageFit,
    _window: &mut Window,
    cx: &mut Context<ScenePanel>,
) -> AnyElement {
    let fits = [
        (rox_i18n::t!("scene-image-fit-contain"), ImageFit::Contain),
        (rox_i18n::t!("scene-image-fit-cover"), ImageFit::Cover),
        (rox_i18n::t!("scene-image-fit-stretch"), ImageFit::Stretch),
    ];

    let me = cx.entity().downgrade();
    let picker_id = id.to_string();
    let pick = settings_ui::small_button(
        SharedString::from(asset.to_string()),
        icons::IMAGE,
        false,
        move |_, window, cx| {
            let position = window.mouse_position();
            let id = picker_id.clone();
            let me = me.clone();
            let menu = PopupMenu::build(window, cx, {
                let me = me.clone();
                move |mut menu, _, _| {
                    for (asset, file) in scene::image_assets() {
                        let me = me.clone();
                        let id = id.clone();
                        menu = menu.item(PopupMenuItem::new(SharedString::from(file)).on_click(
                            move |_, window, cx| {
                                let id = id.clone();
                                let asset = asset.clone();
                                me.update(cx, |this, cx| {
                                    this.change(
                                        |scene| {
                                            if let Some(SceneItem {
                                                content: Content::Image { asset: held, .. },
                                                ..
                                            }) = scene.find_mut(&id)
                                            {
                                                *held = asset;
                                            }
                                        },
                                        window,
                                        cx,
                                    )
                                })
                                .ok();
                            },
                        ));
                    }
                    menu
                }
            });
            me.update(cx, |this, cx| this.open_menu(position, menu, window, cx))
                .ok();
        },
    )
    .keyed("scene-image-asset");

    let fit_id = id.to_string();
    div()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_SM)
        .child(setting_row(rox_i18n::t!("scene-image-asset"), None, pick))
        .child(setting_row(
            rox_i18n::t!("scene-image-fit"),
            None,
            panel::choices_shared(
                &fits,
                fit,
                move |this: &mut ScenePanel, picked, cx| {
                    let id = fit_id.clone();
                    this.change_later(
                        "image.fit",
                        move |s| {
                            if let Some(SceneItem {
                                content: Content::Image { fit, .. },
                                ..
                            }) = s.find_mut(&id)
                            {
                                *fit = picked;
                            }
                        },
                        cx,
                    )
                },
                cx,
            ),
        ))
        .into_any_element()
}

/// The parts of a panel's Shader page, as the Shader panel composes them:
/// the approval gate, the source picker, the switches, the routes and the
/// slots. The gate is the shared one, so a shader that came in with a scene
/// is read here before it runs.
fn shader_rows(
    panel: &ScenePanel,
    item: &SceneItem,
    window: &mut Window,
    cx: &mut Context<ScenePanel>,
) -> AnyElement {
    let Some(shader) = shader_item::of(item) else {
        return div().into_any_element();
    };

    // What runs, so a pool shader is read before it runs and its slot names
    // show.
    let resolved = surface::resolve_source(shader.name.as_deref(), &shader.source);
    let running = resolved.clone().unwrap_or_default();
    let labels = surface::slot_labels(&running);
    let view = panel.shaders.get(&item.id).map(Entity::entity_id);

    let pending = (!running.trim().is_empty() && !surface::approved(&running)).then(|| {
        panel_settings::pending_shader(
            "scene-shader-pending",
            &running,
            shader.path.as_deref(),
            cx.listener(|this, _, _, cx| shader_item::approve(this, cx)),
            cx.listener(|this, _, _, cx| shader_item::set_enabled(this, false, cx)),
        )
    });

    // A compile message from before the switch went off is noise.
    let error = (shader.enabled && shader.runnable())
        .then(|| view.and_then(surface::error))
        .flatten();
    let banner = error.map(|error| {
        if surface::unsupported(&error) {
            panel::banner(
                Tone::Bad,
                surface::NO_PIPELINE_TITLE,
                vec![surface::NO_PIPELINE_NOTE.into()],
            )
        } else {
            panel::banner(
                Tone::Bad,
                rox_i18n::t!("shader-panel-compile-title"),
                error
                    .lines()
                    .take(ERROR_LINES)
                    .map(|line| SharedString::from(line.to_string()))
                    .collect(),
            )
        }
    });

    let rows = &panel.inspector.shader;
    let fallback = surface::eject_name(&shader_item::label(item), &shader.source);
    let picked = ShaderSource {
        id: "scene-shader",
        name: shader.name.as_deref(),
        path: shader.path.as_deref(),
        resolved: resolved.as_deref(),
        clear: Some(shader_item::clear),
        // The item's whole body is the shader, as on the Shader panel.
        overlays_only: false,
        use_example: shader_item::use_example,
        use_named: shader_item::use_named,
        choose_file: shader_item::choose_file,
        edit: shader_item::open_editor,
        eject: shader_item::eject,
        detach: shader_item::detach,
        reload: shader_item::reload,
        save: shader_item::save,
        field: &mut rows.name.borrow_mut(),
        fallback: &fallback,
    }
    .render(window, cx);

    let hub = PanelSettings::state(panel).signals.clone();
    rows.routes.borrow_mut().sync(shader.routes.len());
    let routes_ui = rows.routes.borrow();
    let editor = RouteEditor {
        id: "scene-shader-route",
        hub: &hub,
        routes: &shader.routes,
        labels: &labels,
        value_edit: &panel.inspector.edit,
        ui: &routes_ui,
        ui_mut: |this: &mut ScenePanel| this.inspector.shader.routes.get_mut(),
        mutate: Arc::new(|this: &mut ScenePanel, edit, cx| {
            shader_item::edit_selected(this, "shader.routes", |shader| edit(&mut shader.routes), cx)
        }),
    };
    let add = editor.add_button(cx);
    let routes = editor.list(cx);

    let slots = SlotList {
        hub: &hub,
        routes: &shader.routes,
        manual: &shader.manual,
        labels: &labels,
        value_edit: &panel.inspector.edit,
        scrubs: &rows.slots,
        set: Arc::new(|this: &mut ScenePanel, slot, value, cx| {
            shader_item::set_manual(this, slot, value, cx)
        }),
    }
    .render(cx);

    let gate = pending.map(|body| {
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_XS)
            .child(section_label(rox_i18n::t!("panel-awaiting-approval")))
            .child(body)
    });

    div()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_SM)
        .children(gate)
        .child(section_label(rox_i18n::t!("panel-section-shader")))
        .child(setting_row(
            rox_i18n::t!("shader-panel-run-shader"),
            None,
            panel::toggle(
                shader.enabled,
                |this: &mut ScenePanel, on, cx| shader_item::set_enabled(this, on, cx),
                cx,
            ),
        ))
        .child(picked)
        .children(banner)
        .child(setting_row(
            rox_i18n::t!("panel-run-when-idle"),
            None,
            panel::toggle(
                shader.run_when_idle,
                |this: &mut ScenePanel, on, cx| shader_item::set_idle(this, on, cx),
                cx,
            ),
        ))
        .child(setting_block(
            rox_i18n::t!("panel-section-signals"),
            None,
            Some(add.into_any_element()),
            routes,
        ))
        .child(setting_block(
            rox_i18n::t!("panel-section-slots"),
            None,
            None,
            slots,
        ))
        .into_any_element()
}

// ── Controls ──

fn section_label(label: SharedString) -> AnyElement {
    div()
        .text_xs()
        .text_color(palette::text_muted())
        .child(label)
        .into_any_element()
}

fn inert_readout(text: String) -> AnyElement {
    div()
        .text_color(palette::text_faint())
        .child(text)
        .into_any_element()
}

/// The nine anchors as a three by three grid. Switching keeps the item
/// where it is.
fn anchor_picker(
    id: &str,
    current: Anchor,
    viewport: rox_scene::Viewport,
    cx: &mut Context<ScenePanel>,
) -> AnyElement {
    const ORDER: [Anchor; 9] = [
        Anchor::TopLeft,
        Anchor::Top,
        Anchor::TopRight,
        Anchor::Left,
        Anchor::Center,
        Anchor::Right,
        Anchor::BottomLeft,
        Anchor::Bottom,
        Anchor::BottomRight,
    ];

    let rows = ORDER.chunks(3).map(|row| {
        div()
            .flex()
            .flex_row()
            .gap(px(2.))
            .children(row.iter().map(|&anchor| {
                let id = id.to_string();
                let lit = anchor == current;

                div()
                    .id(SharedString::from(format!("scene-anchor-{anchor:?}")))
                    .size(px(10.))
                    .rounded(px(2.))
                    .border_1()
                    .border_color(if lit {
                        palette::accent()
                    } else {
                        palette::border_light()
                    })
                    .when(lit, |d| d.bg(palette::accent()))
                    .hover(|d| d.bg(palette::bg_control_hover()))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            let id = id.clone();
                            this.change(
                                |scene| {
                                    let Some(parent) = edit::parent_frame(scene, &id, viewport)
                                    else {
                                        return;
                                    };
                                    if let Some(item) = scene.find_mut(&id) {
                                        item.place = edit::with_anchor(item, &parent, anchor);
                                    }
                                },
                                window,
                                cx,
                            );
                        }),
                    )
            }))
    });

    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .children(rows)
        .into_any_element()
}

/// Width or height: the number in its unit, and the unit. A stage length
/// reads as a percentage of the parent's box.
fn length_row(
    panel: &ScenePanel,
    id: &str,
    axis: usize,
    length: Length,
    viewport: rox_scene::Viewport,
    cx: &mut Context<ScenePanel>,
) -> AnyElement {
    let label = if axis == 0 {
        rox_i18n::t!("scene-item-width")
    } else {
        rox_i18n::t!("scene-item-height")
    };
    let (value, stage) = match length {
        Length::Units(n) => (n, false),
        Length::Stage(f) => (f * 100., true),
    };

    let setter_id = id.to_string();
    let field = format!("size{axis}:{id}");
    let value_field = number(
        panel,
        &field,
        value,
        false,
        move |scene, v| {
            if let Some(item) = scene.find_mut(&setter_id) {
                item.place.size[axis] = match item.place.size[axis] {
                    Length::Stage(_) => Length::Stage((v / 100.).max(0.)),
                    Length::Units(_) => Length::Units(v.max(0.)),
                };
            }
        },
        cx,
    );

    let units = [
        (rox_i18n::t!("scene-length-units"), false),
        (rox_i18n::t!("scene-length-percent"), true),
    ];
    let unit_id = id.to_string();
    let unit = panel::choices_shared(
        &units,
        stage,
        move |this: &mut ScenePanel, want: bool, cx| {
            if want == stage {
                return;
            }

            let id = unit_id.clone();
            this.change_later(
                "size.unit",
                move |scene| {
                    let Some(parent) = edit::parent_frame(scene, &id, viewport) else {
                        return;
                    };
                    if let Some(item) = scene.find_mut(&id) {
                        item.place.size[axis] =
                            edit::other_unit(item.place.size[axis], parent.size[axis]);
                    }
                },
                cx,
            )
        },
        cx,
    );

    setting_block(label, None, Some(unit.into_any_element()), value_field).into_any_element()
}

/// A number that turns into a field on a press: Enter applies it, leaving
/// the field drops it.
fn number(
    panel: &ScenePanel,
    field: &str,
    value: f32,
    whole: bool,
    apply: impl Fn(&mut SceneState, f32) + 'static,
    cx: &mut Context<ScenePanel>,
) -> AnyElement {
    if let Some(input) = typing_input(panel, field) {
        return div()
            .w(px(64.))
            .child(typing_field(&input))
            .into_any_element();
    }

    let text = if whole {
        format!("{value:.0}")
    } else {
        trimmed(value)
    };
    let apply = Rc::new(apply);
    let field = field.to_string();

    div()
        .id(SharedString::from(format!("scene-number-{field}")))
        .min_w(px(48.))
        .px(tokens::SPACE_XS)
        .rounded(tokens::RADIUS)
        .bg(palette::bg_control())
        .text_right()
        .whitespace_nowrap()
        .cursor_text()
        .hover(|d| d.bg(palette::bg_control_hover()))
        .child(text.clone())
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, window, cx| {
                let apply = apply.clone();
                begin_typing(
                    this,
                    field.clone(),
                    text.clone(),
                    Rc::new(move |scene: &mut SceneState, typed: &str| {
                        if let Some(v) = rox_panel_kit::parse_number(typed)
                            && v.is_finite()
                        {
                            apply(scene, v);
                        }
                    }),
                    window,
                    cx,
                );
            }),
        )
        .into_any_element()
}

type Commit = Rc<dyn Fn(&mut SceneState, &str)>;

fn begin_typing(
    this: &mut ScenePanel,
    field: String,
    text: String,
    commit: Commit,
    window: &mut Window,
    cx: &mut Context<ScenePanel>,
) {
    let input = cx.new(|cx| InputState::new(window, cx).default_value(text));
    let events = cx.subscribe_in(
        &input,
        window,
        move |this: &mut ScenePanel, input, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { .. } => {
                let typed = input.read(cx).value().to_string();
                let commit = commit.clone();
                this.inspector.typing = None;
                this.change(move |scene| commit(scene, &typed), window, cx);
            }
            InputEvent::Blur => {
                this.inspector.typing = None;
                cx.notify();
            }
            _ => {}
        },
    );

    window.focus(&input.read(cx).focus_handle(cx));
    this.inspector.typing = Some(Typing {
        field,
        input,
        _events: events,
    });
    cx.notify();
}

/// The playback chords bind bare letters workspace-wide, and a binding beats
/// typing, so the field scopes them out the way the search box does.
fn typing_field(input: &Entity<InputState>) -> AnyElement {
    div()
        .key_context("SearchInput")
        .child(Input::new(input).small())
        .into_any_element()
}

fn typing_input(panel: &ScenePanel, field: &str) -> Option<Entity<InputState>> {
    panel
        .inspector
        .typing
        .as_ref()
        .filter(|typing| typing.field == field)
        .map(|typing| typing.input.clone())
}

/// Rounded, without a negative zero.
fn whole(value: f32) -> f32 {
    value.round() + 0.
}

/// Up to two decimals, without trailing zeros.
fn trimmed(value: f32) -> String {
    let text = format!("{value:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".to_string()
    } else {
        text.to_string()
    }
}
