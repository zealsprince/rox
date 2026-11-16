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
use std::sync::{Arc, Mutex};

use gpui::{
    AnyElement, Context, ElementId, Entity, Focusable, KeyContext, MouseButton, Render,
    SharedString, Subscription, Window, anchored, deferred, div, prelude::*, px, svg,
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
use rox_panel_api::signal_ui::{self, bindable_row, routes::RouteEditor, slots::SlotList};
use rox_panel_kit::ui as settings_ui;
use rox_panel_kit::{PickRow, search_picker};
use rox_scene::{Align, Anchor, Content, Fit, ImageFit, Length, SceneItem, SceneState};

use crate::keymap;
use crate::objects::{self, Dialog};
use crate::panels::controls;
use crate::panels::scene::{self, ScenePanel};
use crate::scene::shader::{self as shader_item, RowState};
use crate::scene::text;
use crate::scene::{cost, edit};

const WIDTH: f32 = 340.;

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
    /// The selected text item's pattern box.
    pattern: Option<PatternField>,
    /// The weight line, dropped by every edit and worked out again on the
    /// next render.
    weighed: Option<Weighed>,
}

impl Inspector {
    pub(crate) fn on_left(&self) -> bool {
        self.left
    }

    pub(crate) fn value_edit(&self) -> &ValueEdit {
        &self.edit
    }

    pub(crate) fn reweigh(&mut self) {
        self.weighed = None;
    }
}

struct Weighed {
    counts: SharedString,
    warnings: Vec<SharedString>,
}

struct Typing {
    field: String,
    input: Entity<InputState>,
    _events: Subscription,
}

struct PatternField {
    id: String,
    input: Entity<InputState>,
    /// The item's text as the box last saw it.
    held: String,
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
    signal_ui::sync(panel);
    sync_pattern(panel, window, cx);
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
    let weight = weight(panel);
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
        .child(weight)
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

/// The scene's counts under the tree, then a line for each count that's
/// over. Grid docks are read as last saved, which every edit refreshes.
fn weight(panel: &mut ScenePanel) -> AnyElement {
    let weighed = panel.inspector.weighed.get_or_insert_with(|| {
        let weight = serde_json::to_value(&panel.config)
            .map(cost::weigh_scene)
            .unwrap_or_default();
        Weighed {
            counts: cost::counts(&weight),
            warnings: cost::warnings(&weight),
        }
    });

    // The tone is a fixed yellow that washes out on a light theme, so it
    // marks the line and the words keep the theme's own text color.
    let warnings = weighed.warnings.iter().map(|line| {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .text_color(palette::text())
            .child(
                svg()
                    .path(icons::ALERT)
                    .flex_none()
                    .size(px(12.))
                    .text_color(palette::tone_warn()),
            )
            .child(div().flex_1().min_w_0().child(line.clone()))
    });

    div()
        .id("scene-weight")
        .flex()
        .flex_col()
        .gap(px(1.))
        .px(tokens::SPACE_XS)
        .text_xs()
        .text_color(palette::text_muted())
        .child(weighed.counts.clone())
        .children(warnings)
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
        Content::Text { .. } => icons::A_LARGE_SMALL,
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

    let bound = |property: &str| {
        let target = format!("{id}.{property}");
        panel
            .config
            .routes
            .iter()
            .any(|route| route.target == target)
    };

    let mut rows = div()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_SM)
        .child(setting_row(
            rox_i18n::t!("scene-item-anchor"),
            None,
            anchor_picker(&id, place.anchor, viewport, cx),
        ));

    let axes = [
        rox_i18n::t!("scene-item-position-x"),
        rox_i18n::t!("scene-item-position-y"),
        rox_i18n::t!("scene-item-position-z"),
    ];
    for (axis, label) in axes.into_iter().enumerate() {
        let property = ["x", "y", "z"][axis];
        let setter = id.clone();
        let value = number(
            panel,
            &format!("{property}:{id}"),
            place.position[axis],
            false,
            move |scene, v| {
                if let Some(item) = scene.find_mut(&setter) {
                    item.place.position[axis] = v;
                }
            },
            cx,
        );

        rows = rows.child(bindable_row(
            panel,
            label,
            None,
            format!("{id}.{property}"),
            div().child(value),
            cx,
        ));
        if bound(property) {
            rows = rows.child(sway_row(panel, &id, SwayAxis::Position(axis), item, cx));
        }
    }

    rows = rows
        .child(length_row(panel, &id, 0, place.size[0], viewport, cx))
        .child(length_row(panel, &id, 1, place.size[1], viewport, cx));

    // A flat item stores its turn and scale and draws neither, so those rows
    // stay inert and take no route.
    let axes = [
        rox_i18n::t!("scene-item-rotation-x"),
        rox_i18n::t!("scene-item-rotation-y"),
        rox_i18n::t!("scene-item-rotation-z"),
    ];
    for (axis, label) in axes.into_iter().enumerate() {
        let degrees = place.rotation[axis];
        let readout = format!("{}°", whole(degrees));

        if flat {
            rows = rows.child(setting_row(label, None, inert_readout(readout)));
            continue;
        }

        let property = ["rx", "ry", "rz"][axis];
        let setter = id.clone();
        let slider = panel::value_slider_edit(
            &scrubs.rotation[axis],
            &panel.inspector.edit,
            (degrees + TURN_RANGE) / (2. * TURN_RANGE),
            readout,
            format!("{degrees:.1}"),
            |v| (v + TURN_RANGE) / (2. * TURN_RANGE),
            move |this: &mut ScenePanel, f, cx| {
                let id = setter.clone();
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
        );

        rows = rows.child(bindable_row(
            panel,
            label,
            None,
            format!("{id}.{property}"),
            slider,
            cx,
        ));
        if bound(property) {
            rows = rows.child(sway_row(panel, &id, SwayAxis::Rotation(axis), item, cx));
        }
    }

    let scale_readout = format!("{:.2}", place.scale);
    rows = rows.child(if flat {
        setting_row(
            rox_i18n::t!("scene-item-scale"),
            None,
            inert_readout(scale_readout),
        )
    } else {
        let setter = id.clone();
        let slider = panel::value_slider_edit(
            &scrubs.scale,
            &panel.inspector.edit,
            (place.scale - SCALE_MIN) / (SCALE_MAX - SCALE_MIN),
            scale_readout,
            format!("{:.2}", place.scale),
            |v| (v - SCALE_MIN) / (SCALE_MAX - SCALE_MIN),
            move |this: &mut ScenePanel, f, cx| {
                let id = setter.clone();
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
        );
        bindable_row(
            panel,
            rox_i18n::t!("scene-item-scale"),
            None,
            format!("{id}.scale"),
            slider,
            cx,
        )
    });

    if flat {
        // A shader is flat only because of what its program reads.
        let note = match item.content {
            Content::Shader { .. } => rox_i18n::t!("scene-shader-flat-note"),
            Content::Text { .. } => rox_i18n::t!("scene-text-flat-note"),
            _ => rox_i18n::t!("scene-item-flat-note"),
        };
        rows = rows.child(
            div()
                .text_xs()
                .text_color(palette::text_muted())
                .child(note),
        );
    }

    let opacity_id = id.clone();
    let opacity = panel::value_slider_edit(
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
    );
    rows = rows.child(bindable_row(
        panel,
        rox_i18n::t!("scene-item-opacity"),
        None,
        format!("{id}.opacity"),
        opacity,
        cx,
    ));

    if !matches!(item.content, Content::Grid { .. }) {
        rows = rows.child(command_row(item, cx));
    }

    rows.child(actions(item, cx))
        .child(content_rows(panel, item, window, cx))
        .into_any_element()
}

/// Custom Controls' command list, headed by a row that clears the command.
/// Held against the head row's label, which follows the language where the
/// command labels don't.
fn command_choices() -> Arc<Vec<PickRow>> {
    static ROWS: Mutex<Option<(SharedString, Arc<Vec<PickRow>>)>> = Mutex::new(None);

    let none = rox_i18n::t!("scene-item-on-click-none");
    let mut held = ROWS.lock().unwrap();
    if let Some((label, rows)) = &*held
        && *label == none
    {
        return rows.clone();
    }

    let mut rows = vec![PickRow {
        label: none.clone(),
        value: None,
        terms: Vec::new(),
        icon: None,
    }];
    rows.extend(controls::command_rows().iter().cloned());

    let rows = Arc::new(rows);
    *held = Some((none, rows.clone()));
    rows
}

/// What a click on the item runs off design mode. A grid has no row: its
/// panels own their input.
fn command_row(item: &SceneItem, cx: &mut Context<ScenePanel>) -> AnyElement {
    // A saved id this build doesn't have reads as none, and a click on it
    // does nothing.
    let command = keymap::COMMANDS
        .iter()
        .find(|command| !item.action.is_empty() && command.id == item.action);
    let label = command.map_or_else(
        || rox_i18n::t!("scene-item-on-click-none"),
        |command| SharedString::from(command.label),
    );
    let current = command.map(|command| SharedString::from(command.id));

    let id = item.id.clone();
    let picker = search_picker(
        "scene-item-action",
        command_choices(),
        label,
        current,
        rox_i18n::t!("button-editor-action-search"),
        rox_i18n::t!("button-editor-action-empty"),
        move |this: &mut ScenePanel, picked: Option<String>, cx| {
            let id = id.clone();
            this.change_later(
                "item.action",
                move |scene| {
                    if let Some(item) = scene.find_mut(&id) {
                        item.action = picked.unwrap_or_default();
                    }
                },
                cx,
            )
        },
        cx,
    );

    setting_row(
        rox_i18n::t!("scene-item-on-click"),
        Some(rox_i18n::t!("scene-item-on-click.description")),
        picker,
    )
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
        .child(button(
            rox_i18n::t!("scene-export-object"),
            icons::UPLOAD,
            "scene-export-object",
            |this, window, cx| {
                if let Some(id) = this.selected.clone() {
                    objects::open_export(this, &id, window, cx);
                }
            },
        ))
        .into_any_element()
}

#[derive(Clone, Copy)]
enum SwayAxis {
    Position(usize),
    Rotation(usize),
}

/// Under a bound position or rotation row: how far a full signal carries
/// the item on that axis.
fn sway_row(
    panel: &ScenePanel,
    id: &str,
    sway: SwayAxis,
    item: &SceneItem,
    cx: &mut Context<ScenePanel>,
) -> AnyElement {
    let (field, value, description) = match sway {
        SwayAxis::Position(axis) => (
            format!("sway.{axis}:{id}"),
            item.sway.position[axis],
            rox_i18n::t!("scene-item-sway.units"),
        ),
        SwayAxis::Rotation(axis) => (
            format!("sway.r{axis}:{id}"),
            item.sway.rotation[axis],
            rox_i18n::t!("scene-item-sway.degrees"),
        ),
    };

    let setter = id.to_string();
    let amount = number(
        panel,
        &field,
        value,
        false,
        move |scene, v| {
            if let Some(item) = scene.find_mut(&setter) {
                match sway {
                    SwayAxis::Position(axis) => item.sway.position[axis] = v,
                    SwayAxis::Rotation(axis) => item.sway.rotation[axis] = v,
                }
            }
        },
        cx,
    );

    // Every sway row shares its label, so the id keeps their controls apart.
    div()
        .id(SharedString::from(format!("scene-{field}")))
        .child(settings_ui::nested(setting_row(
            rox_i18n::t!("scene-item-sway"),
            Some(description),
            amount,
        )))
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
        Content::Text {
            font,
            size,
            color,
            align,
            ..
        } => text_rows(panel, &item.id, font.clone(), *size, color, *align, cx),
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

/// Keeps the pattern box on the selected text item. An undo that changes the
/// text puts the new text in the box. Only a change to the item does: a
/// render can land between a keystroke and its event, and comparing against
/// the box then would throw the keystroke away.
fn sync_pattern(panel: &mut ScenePanel, window: &mut Window, cx: &mut Context<ScenePanel>) {
    let selected = panel
        .selected
        .as_ref()
        .and_then(|id| panel.config.scene.find(id))
        .and_then(|item| match &item.content {
            Content::Text { text, .. } => Some((item.id.clone(), text.clone())),
            _ => None,
        });
    let Some((id, held)) = selected else {
        panel.inspector.pattern = None;
        return;
    };

    if let Some(field) = &mut panel.inspector.pattern
        && field.id == id
    {
        if field.held != held {
            field.held = held.clone();
            field
                .input
                .update(cx, |input, cx| input.set_value(held, window, cx));
        }
        return;
    }

    let input = cx.new(|cx| InputState::new(window, cx).default_value(held.clone()));
    let target = id.clone();
    let events = cx.subscribe_in(
        &input,
        window,
        move |this: &mut ScenePanel, input, event: &InputEvent, window, cx| {
            if !matches!(event, InputEvent::Change) {
                return;
            }

            let typed = input.read(cx).value().to_string();
            if let Some(field) = &mut this.inspector.pattern
                && field.id == target
            {
                field.held = typed.clone();
            }

            // The box echoes what an undo put in it, which is no edit.
            let same = this.config.scene.find(&target).is_some_and(
                |item| matches!(&item.content, Content::Text { text, .. } if *text == typed),
            );
            if same {
                return;
            }

            let target = target.clone();
            this.change_keyed(
                Some("text.pattern"),
                move |scene| {
                    if let Some(SceneItem {
                        content: Content::Text { text, .. },
                        ..
                    }) = scene.find_mut(&target)
                    {
                        *text = typed;
                    }
                },
                window,
                cx,
            );
        },
    );

    panel.inspector.pattern = Some(PatternField {
        id,
        input,
        held,
        _events: events,
    });
}

/// The pattern, its font, size, color and alignment.
fn text_rows(
    panel: &ScenePanel,
    id: &str,
    font: Option<String>,
    size: f32,
    color: &str,
    align: Align,
    cx: &mut Context<ScenePanel>,
) -> AnyElement {
    fn edit_text(
        id: &str,
        f: impl Fn(&mut Option<String>, &mut f32, &mut String, &mut Align) + 'static,
    ) -> impl FnOnce(&mut SceneState) + 'static {
        let id = id.to_string();
        move |scene| {
            if let Some(SceneItem {
                content:
                    Content::Text {
                        font,
                        size,
                        color,
                        align,
                        ..
                    },
                ..
            }) = scene.find_mut(&id)
            {
                f(font, size, color, align);
            }
        }
    }

    let note = panel
        .text_error(id)
        .map(|error| panel::PatternNote::Wrong(error.into()));
    let pattern = panel.inspector.pattern.as_ref().map(|field| {
        div()
            .key_context("SearchInput")
            .child(panel::pattern_input(
                "scene-text-pattern",
                &field.input,
                text::PLACEHOLDERS,
                Vec::new(),
                note,
            ))
            .into_any_element()
    });

    let font_id = id.to_string();
    let font_row = panel::font_picker(
        "scene-text-font",
        font,
        move |this: &mut ScenePanel, family: Option<String>, cx| {
            this.change_later(
                "text.font",
                edit_text(&font_id, move |font, _, _, _| *font = family.clone()),
                cx,
            )
        },
        cx,
    );

    let size_id = id.to_string();
    let size_field = number(
        panel,
        &format!("text.size:{id}"),
        size,
        false,
        move |scene, v| edit_text(&size_id, move |_, size, _, _| *size = v.max(1.))(scene),
        cx,
    );

    // A hex color typed into the file shows as its own head row.
    let mut roles: Vec<(String, SharedString)> = Vec::new();
    if !color.is_empty() && !palette::ROLES.iter().any(|role| role.name == color) {
        roles.push((color.to_string(), SharedString::from(color.to_string())));
    }
    roles.extend(
        palette::ROLES
            .iter()
            .map(|role| (role.name.to_string(), SharedString::from(role.label))),
    );
    let color_id = id.to_string();
    let color_row = panel::picker(
        "scene-text-color",
        color.to_string(),
        roles,
        false,
        move |this: &mut ScenePanel, role: String, cx| {
            this.change_later(
                "text.color",
                edit_text(&color_id, move |_, _, color, _| *color = role.clone()),
                cx,
            )
        },
        cx,
    );

    let align_id = id.to_string();
    let align_row = panel::icon_choices(
        &[
            (icons::ALIGN_LEFT, Align::Left),
            (icons::ALIGN_CENTER, Align::Center),
            (icons::ALIGN_RIGHT, Align::Right),
        ],
        align,
        move |this: &mut ScenePanel, picked: Align, cx| {
            this.change_later(
                "text.align",
                edit_text(&align_id, move |_, _, _, align| *align = picked),
                cx,
            )
        },
        cx,
    );

    div()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_SM)
        .children(
            pattern.map(|pattern| setting_block(rox_i18n::t!("scene-text"), None, None, pattern)),
        )
        .child(setting_row(rox_i18n::t!("scene-text-font"), None, font_row))
        .child(setting_row(
            rox_i18n::t!("scene-text-size"),
            None,
            size_field,
        ))
        .child(setting_row(
            rox_i18n::t!("scene-text-color"),
            None,
            color_row,
        ))
        .child(setting_row(rox_i18n::t!("align-row"), None, align_row))
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

// ── Objects ──

/// The Add menu's Objects list: each installed object, with Remove on its
/// row, then Add File.
pub(crate) fn objects_menu(window: &mut Window, cx: &mut Context<ScenePanel>) -> Entity<PopupMenu> {
    let installed = objects::all();
    let me = cx.entity().downgrade();

    PopupMenu::build(window, cx, move |mut menu, _, _| {
        if installed.is_empty() {
            menu = menu.item(PopupMenuItem::label(rox_i18n::t!("scene-objects-empty")));
        }

        for (name, _) in &installed {
            let label = SharedString::from(name.clone());
            let removing = name.clone();
            let placing = name.clone();
            let gone = me.clone();
            let offer = me.clone();

            let row = PopupMenuItem::element(move |_, _| {
                let removing = removing.clone();
                let gone = gone.clone();

                // Taken on the press, which stops there, so the row's own
                // click doesn't place what's being removed.
                let remove = div()
                    .id(SharedString::from(format!(
                        "scene-object-remove-{removing}"
                    )))
                    .flex_none()
                    .p(px(2.))
                    .rounded(tokens::RADIUS)
                    .hover(|d| d.bg(palette::bg_control_hover()))
                    .child(
                        svg()
                            .path(icons::TRASH)
                            .size_3()
                            .text_color(palette::text_muted()),
                    )
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        cx.stop_propagation();
                        if let Err(e) = objects::remove(&removing) {
                            log::warn!("objects: removing {removing}: {e}");
                        }
                        gone.update(cx, |this, cx| this.close_menu(cx)).ok();
                    });

                div()
                    .flex()
                    .flex_1()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .child(div().flex_1().truncate().child(label.clone()))
                    .child(remove)
            })
            .on_click(move |_, window, cx| {
                offer
                    .update(cx, |this, cx| {
                        this.close_menu(cx);
                        objects::offer(this, &placing, window, cx);
                    })
                    .ok();
            });
            menu = menu.item(row);
        }

        let me = me.clone();
        menu.separator().item(
            PopupMenuItem::new(rox_i18n::t!("scene-add-file"))
                .icon(gpui_component::Icon::default().path(icons::FOLDER))
                .on_click(move |_, window, cx| {
                    me.update(cx, |this, cx| this.add_object_file(window, cx))
                        .ok();
                }),
        )
    })
}

/// The export card or the place confirm, centered over the whole window on
/// a scrim, the way a menu sits over it.
pub(crate) fn object_dialog(
    panel: &ScenePanel,
    window: &Window,
    cx: &mut Context<ScenePanel>,
) -> Option<AnyElement> {
    let dialog = panel.object_dialog.as_ref()?;
    let size = window.bounds().size;

    let line = |text: SharedString| {
        div()
            .text_xs()
            .text_color(palette::text_muted())
            .child(text)
    };
    let cancel = settings_ui::dialog_button(
        rox_i18n::t!("workspace-dialog-cancel"),
        false,
        cx.listener(|this, _, window, cx| this.close_object_dialog(window, cx)),
    );

    let (body, buttons, width) = match dialog {
        Dialog::Export { item, fields } => {
            let title = panel
                .config
                .scene
                .find(item)
                .map(|found| {
                    if found.name.is_empty() {
                        found.id.clone()
                    } else {
                        found.name.clone()
                    }
                })
                .unwrap_or_default();

            let mut body = div()
                .key_context("SearchInput")
                .flex()
                .flex_col()
                .gap(tokens::SPACE_SM)
                .child(div().child(rox_i18n::t!("scene-object-export-title", name = title)))
                .child(line(rox_i18n::t!("scene-object-export-hint")));
            for ((label, _), input) in objects::EXPORT_FIELDS.iter().zip(fields) {
                body = body.child(setting_row(
                    rox_i18n::t!(*label),
                    None,
                    Input::new(input).small().w(px(220.)),
                ));
            }

            let export = settings_ui::dialog_button(
                rox_i18n::t!("workspace-dialog-export"),
                true,
                cx.listener(|this, _, window, cx| objects::confirm_export(this, window, cx)),
            );

            (body, vec![cancel, export], 400.)
        }

        Dialog::Place { card, .. } => {
            let shaders = card.shader_line();
            let body = div()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_SM)
                .child(div().child(rox_i18n::t!(
                    "scene-object-place-title",
                    name = card.title.as_str()
                )))
                .children(card.byline.clone().map(line))
                .children(card.description.clone().map(line))
                .children(card.requires.clone().map(line))
                .children(card.heavy.clone().map(line))
                .children(card.weight.iter().cloned().map(line))
                .child(line(rox_i18n::t!("scene-object-place-body")))
                .children(shaders.clone().map(line))
                // A stranger's shaders are code, so the yes that runs them
                // says so, and the yes that doesn't sits beside it.
                .children(
                    shaders
                        .is_some()
                        .then(|| line(rox_i18n::t!("scene-object-shaders-body"))),
                );

            let mut buttons = vec![
                cancel,
                settings_ui::dialog_button(
                    rox_i18n::t!("scene-object-place"),
                    shaders.is_none(),
                    cx.listener(|this, _, window, cx| {
                        objects::confirm_place(this, false, window, cx)
                    }),
                ),
            ];
            if shaders.is_some() {
                buttons.push(settings_ui::dialog_button(
                    rox_i18n::t!("scene-object-approve-place"),
                    true,
                    cx.listener(|this, _, window, cx| {
                        objects::confirm_place(this, true, window, cx)
                    }),
                ));
            }

            (body, buttons, 380.)
        }
    };

    let footer = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .justify_end()
        .gap(tokens::SPACE_SM)
        .children(buttons);

    Some(
        deferred(
            // Pinned to the window's corner: left at the panel's origin, the
            // scrim would stop short of the window's top edge.
            anchored().position(gpui::point(px(0.), px(0.))).child(
                div()
                    .id("scene-object-dialog")
                    .when_some(
                        match dialog {
                            Dialog::Place { focus, .. } => Some(focus),
                            Dialog::Export { .. } => None,
                        },
                        |d, focus| d.track_focus(focus),
                    )
                    .w(size.width)
                    .h(size.height)
                    .occlude()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p(tokens::SPACE_MD)
                    .bg(gpui::rgba(0x00000066))
                    .text_sm()
                    .text_color(palette::text())
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                    .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                        if event.keystroke.key == "escape" {
                            this.close_object_dialog(window, cx);
                            cx.stop_propagation();
                        }
                    }))
                    .child(
                        settings_ui::dialog_card("scene-object-dialog-body", body, footer)
                            .w(px(width)),
                    ),
            ),
        )
        .with_priority(2)
        .into_any_element(),
    )
}
