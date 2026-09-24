//! The widget layer the panels and settings windows are built from. Nothing
//! here depends on the app's state, catalog, or windows.

use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{
    Action, AnyElement, App, Bounds, Context, Div, Element, ElementId, Entity, Focusable as _,
    GlobalElementId, InspectorElementId, LayoutId, MouseButton, MouseDownEvent, Pixels, Point,
    Rgba, SharedString, Stateful, Subscription, Window, canvas, div, prelude::*, px, svg,
};
use gpui_component::button::Button;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_component::tooltip::Tooltip;
use gpui_component::{Disableable, Icon, IconName, Sizable, h_flex};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use serde::{Deserialize, Serialize};

pub mod axis;

pub mod config;

pub mod expr;

pub mod fade;

pub mod grade;

pub mod wall;

mod font_picker;
pub use font_picker::font_picker;
mod icon_picker;
pub use icon_picker::icon_picker;
mod language_picker;
pub use language_picker::language_picker;
mod search_picker;
pub use search_picker::{PickRow, search_picker};

mod gesture;
pub use gesture::*;

mod motif;
pub use motif::motif;

mod tracked_load;
pub use tracked_load::TrackedImage;

pub mod ui;

mod window_buttons;
pub use window_buttons::{
    icon_button, icon_controls, maximize, maximize_icon, maximize_tip, traffic_lights,
};

mod window_chrome;
pub use window_chrome::{chrome_missing, resize_grips};

/// A control's tooltip and the id gpui keeps its hover timer under. Every
/// [`icon_control`] takes one, so no button ships without a name. Text that
/// changes live takes [`Tip::keyed`] so the id stays put.
pub struct Tip {
    id: gpui::ElementId,
    text: SharedString,
    action: Option<(Box<dyn Action>, Option<&'static str>)>,
}

impl Tip {
    pub fn keyed(id: impl Into<gpui::ElementId>, text: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            action: None,
        }
    }

    /// `context` is the key context the binding resolves in (`Workspace`),
    /// not its registration predicate, which finds nothing.
    pub fn action(mut self, action: &dyn Action, context: Option<&'static str>) -> Self {
        self.action = Some((action.boxed_clone(), context));
        self
    }

    pub fn apply(self, control: Div) -> Stateful<Div> {
        let Self { id, text, action } = self;
        control.id(id).tooltip(move |window, cx| {
            let mut tip = Tooltip::new(text.clone());
            if let Some((action, context)) = action.as_ref() {
                tip = tip.action(action.as_ref(), *context);
            }
            tip.build(window, cx)
        })
    }
}

impl From<&'static str> for Tip {
    fn from(text: &'static str) -> Self {
        Self::keyed(text, text)
    }
}

/// The flat icon button the transport panels share, so the style never forks.
pub fn icon_control<V: 'static>(
    icon: &'static str,
    color: Rgba,
    tip: impl Into<Tip>,
    on_click: impl Fn(&mut V, &mut Context<V>) + 'static,
    cx: &mut Context<V>,
) -> Stateful<Div> {
    icon_control_sized(icon, px(16.), color, tip, on_click, cx)
}
pub fn icon_control_sized<V: 'static>(
    icon: &'static str,
    size: Pixels,
    color: Rgba,
    tip: impl Into<Tip>,
    on_click: impl Fn(&mut V, &mut Context<V>) + 'static,
    cx: &mut Context<V>,
) -> Stateful<Div> {
    tip.into().apply(
        div()
            .p(tokens::ICON_PAD)
            .rounded(tokens::RADIUS)
            .hover(|d| d.bg(palette::bg_control()))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| on_click(this, cx)),
            )
            .child(svg().path(icon).size(size).text_color(color)),
    )
}

pub enum PatternNote {
    Preview(SharedString),
    Quiet(SharedString),
    Wrong(SharedString),
}

/// The pattern box used everywhere a pattern is typed. The placeholder
/// vocabulary lives in the info tip, with `notes` for what's true only here.
pub fn pattern_input(
    id: &'static str,
    input: &Entity<InputState>,
    placeholders: &[&str],
    notes: Vec<SharedString>,
    note: Option<PatternNote>,
) -> Div {
    let names = SharedString::from(placeholders.join(" "));

    div()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_XS)
        .child(
            h_flex()
                .items_center()
                .gap(tokens::SPACE_XS)
                .child(Input::new(input).small().flex_1())
                .child(placeholder_tip(id, names, notes)),
        )
        .when_some(note, |column, note| {
            let (text, color) = match note {
                PatternNote::Preview(text) => (text, palette::text_bright()),
                PatternNote::Quiet(text) => (text, palette::text_muted()),
                PatternNote::Wrong(text) => (text, palette::tone_warn()),
            };

            column.child(div().text_xs().text_color(color).child(text))
        })
}

fn placeholder_tip(
    id: &'static str,
    names: SharedString,
    notes: Vec<SharedString>,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .p(tokens::ICON_PAD)
        .rounded(tokens::RADIUS)
        .child(
            svg()
                .path(icons::INFO)
                .size(px(14.))
                .text_color(palette::text_faint()),
        )
        .tooltip(move |window, cx| {
            let names = names.clone();
            let notes = notes.clone();

            Tooltip::element(move |_, _| {
                div()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_XS)
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette::text_muted())
                            .child(rox_i18n::t!("tags-guess-placeholders")),
                    )
                    .child(div().text_xs().child(names.clone()))
                    .children(notes.iter().map(|note| {
                        div()
                            .text_xs()
                            .text_color(palette::text_muted())
                            .child(note.clone())
                    }))
            })
            .build(window, cx)
        })
}

pub fn title_text(custom: Option<&str>, default: impl Into<SharedString>) -> SharedString {
    match custom {
        Some(name) => SharedString::from(name.to_owned()),
        None => default.into(),
    }
}

/// `panel_name()` is an English serialization id, so translate it through
/// `panel-title-<kebab>`, falling back to title case.
pub fn display_name(name: &str) -> String {
    let key = format!("panel-title-{}", name.replace(' ', "-"));
    if let Some(title) = rox_i18n::try_translate(&key) {
        return title.to_string();
    }
    title_case(name)
}

/// No panel name contains an acronym, so per-word capitalizing is enough.
fn title_case(name: &str) -> String {
    name.split(' ')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
/// A flyout row whose tick re-reads the live panel value. Pair it with
/// [`follow_panel`]: hand-built submenus never dismiss on click, so a plain
/// `.checked(..)` tick goes stale.
///
/// Pass `icon` rather than drawing it in the element: the menu reserves a
/// left slot once any item has an icon, and a self-drawn one double-indents.
pub fn check_row<P: 'static>(
    label: impl Into<SharedString>,
    icon: Option<&'static str>,
    is_on: impl Fn(&P) -> bool + 'static,
    toggle: impl Fn(&mut P, &mut Context<P>) + 'static,
    panel: &Entity<P>,
) -> PopupMenuItem {
    let label: SharedString = label.into();
    let read = panel.clone();
    let weak = panel.downgrade();
    let has_icon = icon.is_some();
    let item = PopupMenuItem::element(move |_, cx| {
        let on = is_on(read.read(cx));
        if has_icon {
            // gap_3 matches the stock checked-item row.
            h_flex()
                .w_full()
                .gap_3()
                .items_center()
                .justify_between()
                .child(label.clone())
                .when(on, |row| row.child(Icon::new(IconName::Check).xsmall()))
        } else {
            h_flex()
                .gap_x_1()
                .items_center()
                .child(if on {
                    Icon::new(IconName::Check).xsmall().into_any_element()
                } else {
                    Icon::empty().xsmall().into_any_element()
                })
                .child(label.clone())
        }
    });
    let item = match icon {
        Some(icon) => item.icon(Icon::default().path(icon)),
        None => item,
    };
    item.on_click(move |_, _, cx| {
        let Some(this) = weak.upgrade() else { return };
        this.update(cx, |this, cx| {
            toggle(this, cx);
            cx.notify();
        });
    })
}

/// Call once in the submenu builder, where `cx` is the submenu's own.
pub fn follow_panel<P: 'static>(panel: &Entity<P>, cx: &mut Context<PopupMenu>) {
    cx.observe(panel, |_, _, cx| cx.notify()).detach();
}
/// Pushes a window's art tint through build and every paint phase, like the
/// panel wrapper's `Themed` one level up.
pub struct WindowTint {
    tint: palette::Tint,
    /// Keeps surfaces transparent over an always-painted backdrop. Child
    /// windows follow the All Windows switch instead.
    backdropped: bool,
    child: AnyElement,
}

pub fn window_body(player: gpui::EntityId, body: impl FnOnce() -> AnyElement) -> WindowTint {
    tinted_body(player, false, body)
}

/// A workspace window paints the backdrop whatever the All Windows switch says.
pub fn workspace_body(player: gpui::EntityId, body: impl FnOnce() -> AnyElement) -> WindowTint {
    tinted_body(player, true, body)
}

fn tinted_body(
    player: gpui::EntityId,
    backdropped: bool,
    body: impl FnOnce() -> AnyElement,
) -> WindowTint {
    let tint = palette::window_tint(player);
    let child = palette::backdropped(backdropped, || palette::tinted(tint, body));
    WindowTint {
        tint,
        backdropped,
        child,
    }
}

impl Element for WindowTint {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let layout_id = palette::backdropped(self.backdropped, || {
            palette::tinted(self.tint, || self.child.request_layout(window, cx))
        });
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        palette::backdropped(self.backdropped, || {
            palette::tinted(self.tint, || {
                self.child.prepaint(window, cx);
            });
        });
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        palette::backdropped(self.backdropped, || {
            palette::tinted(self.tint, || self.child.paint(window, cx));
        });
    }
}

impl IntoElement for WindowTint {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}
#[derive(Clone, Copy, PartialEq)]
pub enum Tone {
    Info,
    Good,
    /// Something is standing in for what was asked.
    Warn,
    Bad,
}

impl Tone {
    fn color(self) -> Rgba {
        match self {
            Tone::Info => palette::text_muted(),
            Tone::Good => palette::tone_good(),
            Tone::Warn => palette::tone_warn(),
            Tone::Bad => palette::tone_bad(),
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Tone::Info => icons::INFO,
            Tone::Good => icons::CHECK,
            Tone::Warn | Tone::Bad => icons::ALERT,
        }
    }
}

/// A callout for state a row can't show. The tint is the tone at low alpha
/// over the surface, so it reads on both themes and under the art wash.
pub fn banner(tone: Tone, headline: impl Into<SharedString>, lines: Vec<SharedString>) -> Div {
    banner_shaped(tone, headline, lines, false)
}

/// The same callout with the reasons beside the headline while they fit,
/// for a panel that has to earn its height.
pub fn banner_flow(tone: Tone, headline: impl Into<SharedString>, lines: Vec<SharedString>) -> Div {
    banner_shaped(tone, headline, lines, true)
}

fn banner_shaped(
    tone: Tone,
    headline: impl Into<SharedString>,
    lines: Vec<SharedString>,
    flow: bool,
) -> Div {
    let color = tone.color();
    // The face sits on the headline's row so it stays centered when a
    // reason wraps.
    let head = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_SM)
        // Only when flowing: in a content-sized block a zero minimum reads as
        // min-content and the headline goes one glyph per line.
        .when(flow, |head| head.min_w_0())
        .child(
            Icon::default()
                .path(tone.icon())
                .size_4()
                .text_color(color)
                .flex_none(),
        )
        .child(
            div()
                .min_w_0()
                .text_color(palette::text_bright())
                .child(headline.into()),
        );
    let reason = move |line: SharedString| {
        div()
            .when(flow, |line| line.min_w_0())
            .text_xs()
            .text_color(palette::text_muted())
            .child(line)
    };
    let shell = div()
        .flex()
        .gap(tokens::SPACE_SM)
        .p(tokens::SPACE_SM)
        .pl(tokens::SPACE_MD)
        .rounded(tokens::RADIUS)
        .bg(palette::alpha(color, 0x1c))
        .border_l(px(2.))
        .border_color(color);
    if flow {
        // Breaks fall on natural widths; min_w_0 only matters for a reason
        // too long for its own line.
        return shell
            .flex_row()
            .flex_wrap()
            .items_center()
            .child(head)
            .children(lines.into_iter().map(reason));
    }
    let body = div()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_SM)
        .min_w_0()
        .pl(px(16.) + tokens::SPACE_SM)
        .children(lines.into_iter().map(reason));
    shell.flex_col().child(head).child(body)
}
pub fn setting_row(
    label: impl Into<SharedString>,
    description: Option<SharedString>,
    control: impl IntoElement,
) -> Div {
    setting_row_dyn(label, description, control)
}

/// For a description with live numbers.
pub fn setting_row_dyn(
    label: impl Into<SharedString>,
    description: Option<SharedString>,
    control: impl IntoElement,
) -> Div {
    let label = label.into();
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(tokens::SPACE_MD)
                .child(label.clone())
                // Named after the row so the control inside gets a unique
                // id. See [`ui::control_focus`].
                .child(div().id(ElementId::Name(label)).flex_none().child(control)),
        )
        .when_some(description, |d, description| {
            d.child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(description),
            )
        })
}

/// Like [`setting_row`] with the control full width below. Wrapping controls
/// need this: an inline slot is content-sized and collapses a wrap container.
pub fn setting_block(
    label: impl Into<SharedString>,
    description: Option<SharedString>,
    trailing: Option<AnyElement>,
    control: impl IntoElement,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(tokens::SPACE_MD)
                .child(label.into())
                .when_some(trailing, |d, trailing| {
                    d.child(div().flex_none().child(trailing))
                }),
        )
        .when_some(description, |d, description| {
            d.child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(description),
            )
        })
        .child(div().mt(tokens::SPACE_XS).child(control))
}

pub struct ModeSpec<V> {
    pub label: SharedString,
    /// A full sentence: these options differ in kind.
    pub description: SharedString,
    pub value: V,
}

/// A pick-one list where every option explains itself, for modes that differ
/// in kind rather than degree. Unavailable options dim in place rather than
/// vanish.
pub fn mode_list<P: 'static, V: PartialEq + Copy + 'static>(
    options: &[ModeSpec<V>],
    current: V,
    available: impl Fn(V) -> bool,
    on_pick: impl Fn(&mut P, V, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    let mut list = div().flex().flex_col().gap(tokens::SPACE_XS);
    for option in options {
        let value = option.value;
        let picked = value == current;
        let usable = available(value);
        let on_pick = on_pick.clone();
        list = list.child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                // No explicit width: `w_full` against an unresolved parent falls
                // back to auto and the row shrinks to its longest line.
                .min_w_0()
                .p(tokens::SPACE_SM)
                .rounded(tokens::RADIUS)
                .border_1()
                .border_color(if picked {
                    palette::accent()
                } else {
                    palette::border()
                })
                .bg(if picked {
                    palette::alpha(palette::accent(), 0x20)
                } else {
                    palette::bg_control()
                })
                .when(!usable, |d| d.opacity(0.5))
                .when(usable && !picked, |d| {
                    d.hover(|d| d.bg(palette::bg_control_hover()))
                        .cursor_pointer()
                })
                .when(usable, |d| {
                    d.on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| on_pick(this, value, cx)),
                    )
                })
                // A dot says pick-one where a check would say on-and-off.
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(tokens::SPACE_SM)
                        .child(
                            div()
                                .flex_none()
                                .size(px(10.))
                                .rounded_full()
                                .border_1()
                                .border_color(if picked {
                                    palette::accent()
                                } else {
                                    palette::text_faint()
                                })
                                .when(picked, |d| d.bg(palette::accent())),
                        )
                        .child(
                            div()
                                .text_color(palette::text())
                                .child(option.label.clone()),
                        ),
                )
                .child(
                    div()
                        .pl(px(10.) + tokens::SPACE_SM)
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(option.description.clone()),
                ),
        );
    }
    list
}
pub const SLIDER_W: Pixels = px(150.);
pub const READOUT_W: Pixels = px(60.);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SliderWidth {
    /// Lines every slider on a settings page up in one column.
    Fixed,
    /// For a dialog, with no column to line up with.
    Fill,
}

/// A fortieth of the span, so a held key crosses in about a second.
pub const SLIDER_STEP: f32 = 0.025;

/// Focusable: arrows step, Home and End go to the ends.
fn slider_strip<P: 'static>(
    scrub: &ScrubState,
    fraction: f32,
    width: SliderWidth,
    step: f32,
    apply: impl Fn(&mut P, f32, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> SliderStrip<P> {
    SliderStrip {
        scrub: scrub.clone(),
        fraction,
        width,
        step,
        entity: cx.entity(),
        apply: Rc::new(apply),
    }
}

type ApplyFraction<P> = Rc<dyn Fn(&mut P, f32, &mut Context<P>)>;

#[derive(IntoElement)]
struct SliderStrip<P: 'static> {
    scrub: ScrubState,
    fraction: f32,
    width: SliderWidth,
    step: f32,
    /// A plain element renders with an `App`, so handlers go through the entity.
    entity: Entity<P>,
    apply: ApplyFraction<P>,
}

impl<P: 'static> RenderOnce for SliderStrip<P> {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = ui::control_focus(
            ElementId::NamedInteger("slider".into(), self.scrub.id() as u64),
            window,
            cx,
        );
        let focused = focus.is_focused(window);
        let SliderStrip {
            scrub,
            fraction,
            width,
            step,
            entity,
            apply,
        } = self;
        div()
            .map(|d| match width {
                SliderWidth::Fixed => d.w(SLIDER_W).flex_none(),
                SliderWidth::Fill => d.flex_1(),
            })
            .h(tokens::CONTROL_H)
            .key_context(ui::CONTROL_CONTEXT)
            .cursor_pointer()
            .track_focus(&focus.tab_index(0).tab_stop(true))
            .on_mouse_down(MouseButton::Left, {
                let scrub = scrub.clone();
                let apply = apply.clone();
                let entity = entity.clone();
                move |event: &MouseDownEvent, _, cx| {
                    // This slider wires its own mouse down rather than
                    // going through `pressable`, so mark the press here.
                    ui::note_pointer_press(cx);
                    scrub.begin();
                    if let Some(fraction) = scrub.fraction(event.position.x) {
                        entity.update(cx, |this, cx| {
                            apply(this, fraction, cx);
                            cx.notify();
                        });
                    }
                }
            })
            .on_key_down({
                let apply = apply.clone();
                let entity = entity.clone();
                move |event: &gpui::KeyDownEvent, _, cx| {
                    if event.keystroke.modifiers.modified() {
                        return;
                    }
                    let moved = match event.keystroke.key.as_str() {
                        "left" | "down" => fraction - step,
                        "right" | "up" => fraction + step,
                        "home" => 0.0,
                        "end" => 1.0,
                        _ => return,
                    };
                    entity.update(cx, |this, cx| {
                        apply(this, moved.clamp(0.0, 1.0), cx);
                        cx.notify();
                    });
                    cx.stop_propagation();
                }
            })
            .child(
                canvas(
                    {
                        let scrub = scrub.clone();
                        move |bounds, _, _| scrub.set_bounds(bounds)
                    },
                    {
                        let scrub = scrub.clone();
                        move |bounds, _, window, _| {
                            paint_slider(fraction, false, bounds, window);
                            scrub_on_paint(&scrub, window, {
                                let entity = entity.clone();
                                let apply = apply.clone();
                                move |fraction, cx| {
                                    entity.update(cx, |this, cx| apply(this, fraction, cx));
                                }
                            });
                        }
                    },
                )
                .size_full(),
            )
            .children(ui::focus_ring(focused, tokens::RADIUS, cx))
    }
}

/// The one in-flight readout edit across a panel's settings sliders, behind
/// Arcs like [`ScrubState`].
#[derive(Clone, Default)]
pub struct ValueEdit {
    inner: Arc<Mutex<ValueEditInner>>,
}

#[derive(Default)]
struct ValueEditInner {
    active: Option<usize>,
    input: Option<Entity<InputState>>,
    events: Option<Subscription>,
    /// For the click-outside cancel, which abandons without committing.
    bounds: Option<Bounds<Pixels>>,
}

impl ValueEdit {
    pub fn editing(&self, id: usize) -> Option<Entity<InputState>> {
        let inner = self.inner.lock().unwrap();
        if inner.active == Some(id) {
            inner.input.clone()
        } else {
            None
        }
    }

    fn active_id(&self) -> Option<usize> {
        self.inner.lock().unwrap().active
    }

    fn set_bounds(&self, bounds: Bounds<Pixels>) {
        self.inner.lock().unwrap().bounds = Some(bounds);
    }

    fn contains(&self, position: Point<Pixels>) -> bool {
        self.inner
            .lock()
            .unwrap()
            .bounds
            .is_some_and(|bounds| bounds.contains(&position))
    }

    fn begin(&self, id: usize, input: Entity<InputState>, events: Subscription) {
        let mut inner = self.inner.lock().unwrap();
        inner.active = Some(id);
        inner.input = Some(input);
        inner.events = Some(events);
        inner.bounds = None;
    }

    fn end(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.active = None;
        inner.input = None;
        inner.events = None;
        inner.bounds = None;
    }
}

/// A slider whose readout doubles as an input: Enter commits, blur cancels.
/// `to_fraction` maps the typed value through the row's own mapping.
#[allow(clippy::too_many_arguments)]
pub fn value_slider_edit<P: 'static>(
    scrub: &ScrubState,
    edit: &ValueEdit,
    fraction: f32,
    readout: String,
    edit_text: String,
    to_fraction: impl Fn(f32) -> f32 + Clone + 'static,
    apply: impl Fn(&mut P, f32, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    value_slider_edit_over(
        scrub,
        edit,
        fraction,
        readout,
        edit_text,
        1.0,
        to_fraction,
        apply,
        cx,
    )
}

/// `over` is the highest fraction a typed value may reach, for knobs whose
/// slider range is a guideline.
#[allow(clippy::too_many_arguments)]
pub fn value_slider_edit_over<P: 'static>(
    scrub: &ScrubState,
    edit: &ValueEdit,
    fraction: f32,
    readout: String,
    edit_text: String,
    over: f32,
    to_fraction: impl Fn(f32) -> f32 + Clone + 'static,
    apply: impl Fn(&mut P, f32, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    value_slider_edit_sized(
        scrub,
        edit,
        fraction,
        readout,
        edit_text,
        over,
        SliderWidth::Fixed,
        SLIDER_STEP,
        to_fraction,
        apply,
        cx,
    )
}

pub type ParseTyped = fn(&str) -> Option<f32>;

/// Either decimal mark, since the readout is written in the locale's own.
pub fn parse_number(text: &str) -> Option<f32> {
    text.trim().replace(',', ".").parse::<f32>().ok()
}

#[allow(clippy::too_many_arguments)]
pub fn value_slider_edit_sized<P: 'static>(
    scrub: &ScrubState,
    edit: &ValueEdit,
    fraction: f32,
    readout: String,
    edit_text: String,
    over: f32,
    width: SliderWidth,
    step: f32,
    to_fraction: impl Fn(f32) -> f32 + Clone + 'static,
    apply: impl Fn(&mut P, f32, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    value_slider_edit_typed(
        scrub,
        edit,
        fraction,
        readout,
        edit_text,
        over,
        width,
        step,
        parse_number,
        to_fraction,
        apply,
        cx,
    )
}

/// For a readout written in words, like `1 h 30 min`.
#[allow(clippy::too_many_arguments)]
pub fn value_slider_edit_typed<P: 'static>(
    scrub: &ScrubState,
    edit: &ValueEdit,
    fraction: f32,
    readout: String,
    edit_text: String,
    over: f32,
    width: SliderWidth,
    step: f32,
    parse: ParseTyped,
    to_fraction: impl Fn(f32) -> f32 + Clone + 'static,
    apply: impl Fn(&mut P, f32, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    let row = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_SM)
        .map(|d| match width {
            SliderWidth::Fixed => d,
            SliderWidth::Fill => d.w_full(),
        })
        .child(slider_strip(
            scrub,
            fraction,
            width,
            step,
            apply.clone(),
            cx,
        ));
    if let Some(input) = edit.editing(scrub.id()) {
        // A one-frame window handler cancels on a press outside the input:
        // nothing else in the settings window takes focus, so blur never fires.
        let id = scrub.id();
        let entity = cx.entity();
        return row.child(
            div()
                .w(READOUT_W)
                // The small input is 2px taller than CONTROL_H; left to size
                // the row it nudges the whole page.
                .h(tokens::CONTROL_H)
                .flex_none()
                .relative()
                .flex()
                .items_center()
                .child(
                    canvas(
                        {
                            let edit = edit.clone();
                            move |bounds, _, _| edit.set_bounds(bounds)
                        },
                        {
                            let edit = edit.clone();
                            move |_, _, window, _| {
                                let edit = edit.clone();
                                let entity = entity.clone();
                                window.on_mouse_event(
                                    move |event: &MouseDownEvent, phase, _, cx| {
                                        if !phase.bubble()
                                            || edit.active_id() != Some(id)
                                            || edit.contains(event.position)
                                        {
                                            return;
                                        }
                                        edit.end();
                                        entity.update(cx, |_, cx| cx.notify());
                                    },
                                );
                            }
                        },
                    )
                    .absolute()
                    .inset_0(),
                )
                .child(Input::new(&input).small().w_full()),
        );
    }
    let id = scrub.id();
    row.child(
        div()
            // A floor, not a width: a duration readout wrapped to two lines
            // reads as two settings.
            .min_w(READOUT_W)
            .flex_none()
            .whitespace_nowrap()
            .text_right()
            .text_color(palette::text_muted())
            // Never a text restyle on hover: it re-shapes the line and the
            // number shifts under the pointer.
            .rounded(tokens::RADIUS)
            .hover(|d| d.bg(palette::bg_control()))
            .cursor_text()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener({
                    let edit = edit.clone();
                    move |_: &mut P, _, window, cx| {
                        let input = cx
                            .new(|cx| InputState::new(window, cx).default_value(edit_text.clone()));
                        let events = cx.subscribe_in(&input, window, {
                            let edit = edit.clone();
                            let to_fraction = to_fraction.clone();
                            let apply = apply.clone();
                            move |this: &mut P, input, event: &InputEvent, _, cx| match event {
                                InputEvent::PressEnter { .. } => {
                                    let text = input.read(cx).value().to_string();
                                    if let Some(value) = parse(&text) {
                                        let ceiling = over.max(1.0);
                                        apply(this, to_fraction(value).clamp(0.0, ceiling), cx);
                                    }
                                    edit.end();
                                    cx.notify();
                                }
                                InputEvent::Blur => {
                                    edit.end();
                                    cx.notify();
                                }
                                _ => {}
                            }
                        });
                        window.focus(&input.read(cx).focus_handle(cx));
                        edit.begin(id, input, events);
                        cx.notify();
                    }
                }),
            )
            .child(readout),
    )
}
/// For menu rows that flip a switch from their own click.
pub fn toggle_face(on: bool) -> Div {
    toggle_track(on)
}

fn toggle_track(on: bool) -> Div {
    div()
        .w(px(34.))
        .h(px(18.))
        .flex_none()
        .rounded_full()
        .bg(palette::bg_control())
        .flex()
        .items_center()
        .when(on, |d| d.justify_end())
        .p(px(2.))
        .child(div().size(px(14.)).rounded_full().bg(if on {
            palette::accent()
        } else {
            palette::text_faint()
        }))
}

/// Takes its name from the surrounding [`setting_row`].
pub fn toggle<P: 'static>(
    on: bool,
    on_change: impl Fn(&mut P, bool, &mut Context<P>) + 'static,
    cx: &mut Context<P>,
) -> Toggle {
    Toggle {
        on,
        on_change: Rc::new(cx.listener(move |this, _, _, cx| on_change(this, !on, cx))),
    }
}

#[derive(IntoElement)]
pub struct Toggle {
    on: bool,
    on_change: ui::OnPress,
}

impl RenderOnce for Toggle {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = ui::control_focus("toggle", window, cx);
        let focused = focus.is_focused(window);
        let base = toggle_track(self.on)
            .key_context(ui::CONTROL_CONTEXT)
            .cursor_pointer()
            .track_focus(&focus.tab_index(0).tab_stop(true));
        ui::pressable(base, self.on_change).children(ui::focus_ring(focused, px(9.), cx))
    }
}

pub fn toggle_locked(on: bool) -> Div {
    toggle_track(on).opacity(0.5)
}

pub const TYPE_AHEAD: Duration = Duration::from_millis(1000);

/// Scopes the space-bound playback binding out while a phrase is taking
/// keystrokes. Gated on the window, not the phrase, or playback would stay
/// carved out long after the typing stopped.
pub const TYPE_AHEAD_CONTEXT: &str = "TypeAhead";

/// Where the tab cycle bindings live. Outlives [`TYPE_AHEAD_CONTEXT`]:
/// cycling stays available until the phrase is dropped.
pub const TYPE_AHEAD_CYCLE_CONTEXT: &str = "TypeAheadCycle";

/// gpui parses a space-separated context as several identifiers.
pub fn type_ahead_context(phrase: &str, at: Option<Instant>) -> Option<&'static str> {
    if phrase.is_empty() {
        None
    } else if type_ahead_live(at) {
        Some("TypeAhead TypeAheadCycle")
    } else {
        Some(TYPE_AHEAD_CYCLE_CONTEXT)
    }
}

/// For a panel whose arrows mean something horizontally. The workspace binds
/// bare left and right to seek, and the binding would eat the keystroke.
pub const PANEL_NAV_CONTEXT: &str = "PanelNav";

/// One string, because a second `key_context` call drops the first.
pub fn panel_nav_context(phrase: &str, at: Option<Instant>) -> &'static str {
    match type_ahead_context(phrase, at) {
        None => "PanelNav",
        Some(TYPE_AHEAD_CYCLE_CONTEXT) => "PanelNav TypeAheadCycle",
        Some(_) => "PanelNav TypeAhead TypeAheadCycle",
    }
}

pub fn type_ahead_live(at: Option<Instant>) -> bool {
    at.is_some_and(|last| last.elapsed() < TYPE_AHEAD)
}

/// Returns whether the phrase grew, so the caller re-tests the current row
/// rather than stepping past it.
pub fn type_ahead_grow(buffer: &mut String, at: &mut Option<Instant>, text: String) -> bool {
    let now = Instant::now();
    let grown = at.is_some_and(|last| now.duration_since(last) < TYPE_AHEAD);
    if grown {
        buffer.push_str(&text);
    } else {
        *buffer = text;
    }
    *at = Some(now);
    grown
}

/// A word start: "beat" finds "Beat It" and "The Beatles". ASCII
/// case-insensitive; callers with pre-lowered tables pass a lowered needle.
pub fn type_ahead_hit(text: &str, needle: &str) -> bool {
    let mut boundary = true;
    let mut at = 0;
    for c in text.chars() {
        if boundary
            && text
                .get(at..at + needle.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(needle))
        {
            return true;
        }
        boundary = !c.is_alphanumeric();
        at += c.len_utf8();
    }
    false
}

/// Every index once, starting just past `from`, wrapping, backwards when
/// `back`.
pub fn type_ahead_scan(len: usize, from: Option<usize>, back: bool) -> impl Iterator<Item = usize> {
    let start = match (from, back) {
        _ if len == 0 => 0,
        (Some(ix), false) => (ix + 1) % len,
        (Some(ix), true) => (ix + len - 1) % len,
        (None, false) => 0,
        (None, true) => len - 1,
    };
    (0..len).map(move |i| {
        if back {
            (start + len - i) % len
        } else {
            (start + i) % len
        }
    })
}

/// Pair it with [`type_ahead_fade`] so the badge leaves on time.
pub fn type_ahead_overlay(phrase: &str, at: Option<Instant>) -> Option<Div> {
    if phrase.is_empty() || !type_ahead_live(at) {
        return None;
    }
    Some(
        div()
            .absolute()
            .top(tokens::SPACE_SM)
            .right(tokens::SPACE_SM)
            .px(tokens::SPACE_SM)
            .py(tokens::SPACE_XS)
            .rounded(tokens::RADIUS)
            .bg(palette::bg_menu())
            .border_1()
            .border_color(palette::border())
            .text_sm()
            .text_color(palette::text())
            .child(SharedString::from(phrase.to_string())),
    )
}

pub fn letter_initial(name: &str) -> String {
    match name.chars().next() {
        Some(c) if c.is_ascii_alphabetic() => c.to_ascii_uppercase().to_string(),
        Some(c) if !c.is_ascii() => c.to_uppercase().to_string(),
        _ => "#".to_string(),
    }
}

/// None under two letters. `compact` keeps one scrolling line instead of
/// wrapping.
pub fn letter_rail<P: 'static>(
    letters: &[(SharedString, usize)],
    active: usize,
    horizontal: bool,
    compact: bool,
    pick: impl Fn(&mut P, usize, &mut Context<P>) + Copy + 'static,
    cx: &mut Context<P>,
) -> Option<Div> {
    if letters.len() < 2 {
        return None;
    }
    let mut strip = if horizontal {
        div().flex().flex_row().items_center()
    } else {
        div().flex().flex_col().items_center()
    };
    for (i, (letter, first)) in letters.iter().enumerate() {
        let first = *first;
        strip = strip.child(
            div()
                .id(("letter-rail", i))
                .px(px(3.))
                .text_xs()
                .cursor_pointer()
                .text_color(if i == active {
                    palette::accent()
                } else {
                    palette::text_muted()
                })
                .hover(|d| d.text_color(palette::text()))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        // A rail press is a jump; it must not fall through
                        // to the host's scrub or drag.
                        cx.stop_propagation();
                        pick(this, first, cx);
                    }),
                )
                .child(letter.clone()),
        );
    }
    Some(if compact {
        let scroll = strip.id("letter-rail-scroll");
        if horizontal {
            div()
                .w_full()
                .flex()
                .flex_row()
                .justify_center()
                .child(scroll.overflow_x_scroll().max_w(gpui::relative(1.)))
        } else {
            div()
                .h_full()
                .flex()
                .flex_col()
                .justify_center()
                .child(scroll.overflow_y_scroll().max_h(gpui::relative(1.)))
        }
    } else {
        let strip = strip.flex_wrap().justify_center();
        if horizontal {
            strip.w_full()
        } else {
            strip.h_full()
        }
    })
}

/// Repaint when the type-ahead window lapses, so the badge leaves and the key
/// context releases space and tab on time.
pub fn type_ahead_fade<P: 'static>(cx: &mut Context<P>) {
    cx.spawn(async move |this, cx| {
        cx.background_executor().timer(TYPE_AHEAD).await;
        this.update(cx, |_, cx| cx.notify()).ok();
    })
    .detach();
}

#[allow(clippy::too_many_arguments)]
pub fn tracking_section<P: 'static>(
    follow: bool,
    follow_desc: SharedString,
    on_follow: impl Fn(&mut P, bool, &mut Context<P>) + 'static,
    resume: bool,
    resume_desc: SharedString,
    on_resume: impl Fn(&mut P, bool, &mut Context<P>) + 'static,
    smooth: bool,
    smooth_desc: SharedString,
    on_smooth: impl Fn(&mut P, bool, &mut Context<P>) + 'static,
    cx: &mut Context<P>,
) -> AnyElement {
    let mut body = div()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_MD)
        .child(setting_row(
            rox_i18n::t!("tracking-follow"),
            Some(follow_desc),
            toggle(follow, on_follow, cx),
        ))
        .child(setting_row(
            rox_i18n::t!("tracking-resume"),
            Some(resume_desc),
            toggle(resume, on_resume, cx),
        ));
    if follow || resume {
        body = body.child(setting_row(
            rox_i18n::t!("tracking-smooth"),
            Some(smooth_desc),
            toggle(smooth, on_smooth, cx),
        ));
    }
    ui::section(rox_i18n::t!("tracking-title"), None, body).into_any_element()
}
/// A dropdown for lists too long for [`choices`].
// `use<..>` keeps the element off the `cx` borrow so callers can use `cx`
// before mounting it. The capture list has to name every type parameter,
// hence the named `A`.
pub fn picker<P, K, A>(
    id: &'static str,
    current: K,
    options: Vec<(K, SharedString)>,
    disabled: bool,
    apply: A,
    cx: &mut Context<P>,
) -> impl IntoElement + use<P, K, A>
where
    P: 'static,
    K: PartialEq + Clone + 'static,
    A: Fn(&mut P, K, &mut Context<P>) + Clone + 'static,
{
    // An id missing from the list labels and ticks the head row: an unplugged
    // device reads as the default it will actually open.
    let picked = options
        .iter()
        .find(|(key, _)| *key == current)
        .or_else(|| options.first());
    let label = picked.map(|(_, label)| label.clone()).unwrap_or_default();
    let current = picked.map(|(key, _)| key.clone());
    let weak = cx.entity().downgrade();
    // gpui-component only scrolls menus built with `with_menu_items`, so
    // enable it here past the same threshold upstream uses.
    let scrollable = options.len() > 20;
    Button::new(id)
        .label(label)
        .small()
        .outline()
        .disabled(disabled)
        .dropdown_menu(move |mut menu, _, _| {
            menu = menu.scrollable(scrollable);
            for (key, label) in options.iter() {
                let checked = current.as_ref() == Some(key);
                let key = key.clone();
                let pick = weak.clone();
                let apply = apply.clone();
                menu = menu.item(PopupMenuItem::new(label.clone()).checked(checked).on_click(
                    move |_, _, cx| {
                        let key = key.clone();
                        let apply = apply.clone();
                        if let Some(this) = pick.upgrade() {
                            this.update(cx, |this, cx| apply(this, key, cx));
                        }
                    },
                ));
            }
            menu
        })
}

fn segments<P: 'static, L: Clone, V: PartialEq + Copy + 'static>(
    options: &[(L, V)],
    picked: impl Fn(V) -> bool,
    available: impl Fn(V) -> bool,
    render: impl Fn(L, bool) -> AnyElement,
    on_pick: impl Fn(&mut P, V, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    let last = options.len().saturating_sub(1);
    let mut group = div().flex().flex_row().flex_none().items_center();
    for (i, (key, value)) in options.iter().enumerate() {
        let value = *value;
        let picked = picked(value);
        let available = available(value);
        let on_pick = on_pick.clone();
        group = group.child(
            div()
                .px(tokens::SPACE_SM)
                .py(tokens::SPACE_XS)
                .when(i > 0, |d| d.ml(px(1.)))
                .when(i == 0, |d| d.rounded_l(tokens::RADIUS))
                .when(i == last, |d| d.rounded_r(tokens::RADIUS))
                .bg(if picked {
                    palette::accent()
                } else {
                    palette::bg_control()
                })
                .when(!available, |d| d.opacity(0.5))
                .when(available, |d| {
                    d.when(!picked, |d| d.hover(|d| d.bg(palette::bg_control_hover())))
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| on_pick(this, value, cx)),
                        )
                })
                .child(render(key.clone(), picked)),
        );
    }
    group
}

pub fn choices<P: 'static, V: PartialEq + Copy + 'static>(
    options: &'static [(&'static str, V)],
    current: V,
    on_pick: impl Fn(&mut P, V, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    // Literal call sites are still migrating to the owned-label path.
    let owned: Vec<(SharedString, V)> = options
        .iter()
        .map(|(label, value)| (SharedString::from(*label), *value))
        .collect();
    choices_gated(&owned, current, |_| true, on_pick, cx)
}

pub fn choices_shared<P: 'static, V: PartialEq + Copy + 'static>(
    options: &[(SharedString, V)],
    current: V,
    on_pick: impl Fn(&mut P, V, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    segments(
        options,
        move |value| value == current,
        |_| true,
        |label, picked| {
            div()
                .text_color(if picked {
                    palette::text_on_accent()
                } else {
                    palette::text()
                })
                .child(label)
                .into_any_element()
        },
        on_pick,
        cx,
    )
}

pub fn choices_icons<P: 'static, V: PartialEq + Copy + 'static>(
    options: &[(&'static str, V)],
    current: V,
    on_pick: impl Fn(&mut P, V, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    segments(
        options,
        move |value| value == current,
        |_| true,
        |icon, picked| {
            Icon::default()
                .path(icon)
                .xsmall()
                .text_color(if picked {
                    palette::text_on_accent()
                } else {
                    palette::text()
                })
                .into_any_element()
        },
        on_pick,
        cx,
    )
}

/// Blocked options dim and take no press, so the row can still say what's
/// missing.
pub fn choices_gated<P: 'static, V: PartialEq + Copy + 'static>(
    options: &[(SharedString, V)],
    current: V,
    available: impl Fn(V) -> bool,
    on_pick: impl Fn(&mut P, V, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    segments(
        options,
        move |value| value == current,
        available,
        |label, picked| {
            div()
                .text_color(if picked {
                    palette::text_on_accent()
                } else {
                    palette::text()
                })
                .child(label)
                .into_any_element()
        },
        on_pick,
        cx,
    )
}

pub fn icon_choices<P: 'static, V: PartialEq + Copy + 'static>(
    options: &'static [(&'static str, V)],
    current: V,
    on_pick: impl Fn(&mut P, V, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    segments(
        options,
        move |value| value == current,
        |_| true,
        icon_segment,
        on_pick,
        cx,
    )
}

pub fn icon_toggles<P: 'static, V: PartialEq + Copy + 'static>(
    options: &'static [(&'static str, V)],
    active: impl Fn(V) -> bool,
    on_toggle: impl Fn(&mut P, V, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    segments(options, active, |_| true, icon_segment, on_toggle, cx)
}

fn icon_segment(icon: &'static str, picked: bool) -> AnyElement {
    svg()
        .path(icon)
        .size_4()
        .text_color(if picked {
            palette::text_on_accent()
        } else {
            palette::text()
        })
        .into_any_element()
}
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

pub fn justify(d: Div, align: Align) -> Div {
    match align {
        Align::Left => d.justify_start(),
        Align::Center => d.justify_center(),
        Align::Right => d.justify_end(),
    }
}

pub fn items(d: Div, align: Align) -> Div {
    match align {
        Align::Left => d.items_start(),
        Align::Center => d.items_center(),
        Align::Right => d.items_end(),
    }
}

pub fn align_row<P: 'static>(
    current: Align,
    on_pick: impl Fn(&mut P, Align, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    setting_row(
        rox_i18n::t!("align-row"),
        Some(rox_i18n::t!("align-row.description")),
        icon_choices(
            &[
                (icons::ALIGN_LEFT, Align::Left),
                (icons::ALIGN_CENTER, Align::Center),
                (icons::ALIGN_RIGHT, Align::Right),
            ],
            current,
            on_pick,
            cx,
        ),
    )
}

#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VAlign {
    Top,
    #[default]
    Middle,
    Bottom,
}

pub fn justify_v(d: Div, align: VAlign) -> Div {
    match align {
        VAlign::Top => d.justify_start(),
        VAlign::Middle => d.justify_center(),
        VAlign::Bottom => d.justify_end(),
    }
}

pub fn valign_row<P: 'static>(
    current: VAlign,
    on_pick: impl Fn(&mut P, VAlign, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    setting_row(
        rox_i18n::t!("valign-row"),
        Some(rox_i18n::t!("valign-row.description")),
        choices_shared(
            &[
                (rox_i18n::t!("valign-top"), VAlign::Top),
                (rox_i18n::t!("valign-middle"), VAlign::Middle),
                (rox_i18n::t!("valign-bottom"), VAlign::Bottom),
            ],
            current,
            on_pick,
            cx,
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::{type_ahead_hit, type_ahead_scan};

    #[test]
    fn scan_steps_past_the_cursor_and_wraps() {
        let forward: Vec<_> = type_ahead_scan(4, Some(1), false).collect();
        assert_eq!(forward, [2, 3, 0, 1]);
        let back: Vec<_> = type_ahead_scan(4, Some(1), true).collect();
        assert_eq!(back, [0, 3, 2, 1]);
    }

    #[test]
    fn scan_without_a_cursor_starts_at_the_edges() {
        let forward: Vec<_> = type_ahead_scan(3, None, false).collect();
        assert_eq!(forward, [0, 1, 2]);
        let back: Vec<_> = type_ahead_scan(3, None, true).collect();
        assert_eq!(back, [2, 1, 0]);
        assert_eq!(type_ahead_scan(0, None, true).count(), 0);
    }

    #[test]
    fn word_starts_match() {
        assert!(type_ahead_hit("Beat It", "beat"));
        assert!(type_ahead_hit("The Beatles", "beat"));
        assert!(type_ahead_hit("The Beatles", "the bea"));
        assert!(type_ahead_hit("Daft Punk", "punk"));
        assert!(type_ahead_hit("AC/DC", "dc"));
    }

    #[test]
    fn mid_word_does_not() {
        assert!(!type_ahead_hit("The Beatles", "eat"));
        assert!(!type_ahead_hit("Weekend", "end"));
        assert!(!type_ahead_hit("Daft Punk", "aft"));
    }
}
