//! The widget layer the panels and the settings windows are built from:
//! the rows, toggles, pickers, sliders, banners, and the gesture and
//! scroll mechanics under them. Nothing here knows about the app's state,
//! its catalog, or its windows - a builder takes what it draws and a
//! handler to call, and the caller owns everything else.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{
    canvas, div, prelude::*, px, svg, Action, AnyElement, App, Bounds, Context, Div, Element,
    Entity, Focusable as _, GlobalElementId, InspectorElementId, LayoutId, MouseButton,
    MouseDownEvent, Pixels, Point, Rgba, SharedString, Stateful, Subscription, Window,
};
use gpui_component::button::Button;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_component::tooltip::Tooltip;
use gpui_component::{h_flex, Disableable, Icon, IconName, Sizable};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use serde::{Deserialize, Serialize};

pub mod axis;

pub mod config;

pub mod wall;

mod font_picker;
pub use font_picker::font_picker;
mod language_picker;
pub use language_picker::language_picker;
mod search_picker;
pub use search_picker::{search_picker, PickRow};

mod gesture;
pub use gesture::*;

mod motif;
pub use motif::motif;

mod tracked_load;
pub use tracked_load::TrackedImage;

pub mod ui;

mod window_buttons;
pub use window_buttons::{maximize, maximize_icon, maximize_tip, traffic_lights};

/// What a control's hover tooltip says, and the identity gpui parks its
/// timing under. Every [`icon_control`] takes one: a glyph on its own says
/// nothing to anyone who doesn't already know the app, so a new button
/// can't ship without naming what it does.
///
/// gpui keeps the hover timer in element state, which only elements with
/// an id get, so a tipped control needs an id too. A static label is its
/// own id. Anything whose words read live (the loop button's mode, a
/// per-row play button) takes [`Tip::keyed`] instead, so the id stays put
/// while the text moves and two rows never share one timer.
pub struct Tip {
    id: gpui::ElementId,
    text: SharedString,
    action: Option<(Box<dyn Action>, Option<&'static str>)>,
}

impl Tip {
    /// A tip whose words change, under an id that doesn't.
    pub fn keyed(id: impl Into<gpui::ElementId>, text: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            action: None,
        }
    }

    /// Trail the shortcut that does the same thing. `context` is the key
    /// context the binding resolves in (`Workspace`), not the predicate it
    /// was registered with, which parses as a context and finds nothing.
    pub fn action(mut self, action: &dyn Action, context: Option<&'static str>) -> Self {
        self.action = Some((action.boxed_clone(), context));
        self
    }

    /// Hang the tip off a control that builds itself, for the buttons the
    /// shared [`icon_control`] has no room for.
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

/// The flat icon button the transport panels share so the button style
/// never forks: the icon alone at rest, a soft pill behind it on hover,
/// and a [`Tip`] naming it once the pointer settles. Icon paths come from
/// [`rox_design::assets::icons`].
pub fn icon_control<V: 'static>(
    icon: &'static str,
    color: Rgba,
    tip: impl Into<Tip>,
    on_click: impl Fn(&mut V, &mut Context<V>) + 'static,
    cx: &mut Context<V>,
) -> Stateful<Div> {
    icon_control_sized(icon, px(16.), color, tip, on_click, cx)
}
/// [`icon_control`] with the icon size exposed, for spots like the menubar
/// where the transport-scale glyph reads too heavy.
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
/// A panel's tab and title text: the rename when one is set, the built-in
/// name otherwise.
pub fn title_text(custom: Option<&str>, default: impl Into<SharedString>) -> SharedString {
    match custom {
        Some(name) => SharedString::from(name.to_owned()),
        None => default.into(),
    }
}

/// Title-case a panel's built-in name for display. The name is a
/// serialized identifier (lowercase, space separated); tab and window
/// titles want it capitalized. No panel name contains "rox" or an
/// acronym, so a plain per-word capitalize is right here.
pub fn display_name(name: &str) -> String {
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
/// A checkable flyout row whose tick tracks the live panel value instead of
/// one baked in when the menu was built. Pair it with [`follow_panel`] in the
/// submenu builder: the flyout re-renders on the click, this row re-reads the
/// value, and the tick swaps in place.
///
/// Plain `.checked(..)` rows go stale in an open flyout, our hand-built
/// submenus never dismiss on click (they carry no link back to the root menu,
/// so there is no reopen to rebuild them), so a static tick would sit wrong
/// until the whole menu is closed and reopened.
///
/// `is_on` reads the state each render, `toggle` flips it. An `icon` rides on
/// the item rather than inside our element, so it lands in the same reserved
/// left slot the plain rows use and the row lines up with its neighbours;
/// the tick then sits on the right, matching `check_side(Side::Right)`.
/// Without an icon the tick takes the left slot, matching the default check
/// side, which is the shape flyouts of bare toggles use.
///
/// Drawing the icon inside the element instead would double-indent the row:
/// the menu reserves a left slot as soon as any item carries an icon, so a
/// self-drawn icon sits one slot further in than everything around it.
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
            // gap_3 mirrors the stock checked-item row, so the widest row
            // still gets the same label-to-tick breathing room.
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

/// Re-render an open flyout whenever `panel` changes, so its [`check_row`]s
/// pick up the flip without the menu closing. Call once in the submenu
/// builder, where `cx` is the submenu's own context.
pub fn follow_panel<P: 'static>(panel: &Entity<P>, cx: &mut Context<PopupMenu>) {
    cx.observe(panel, |_, _, cx| cx.notify()).detach();
}
/// Wraps a window's whole body in its player's art tint, the mirror of
/// [`Themed`] one level up: the palette accessors answer from the tint
/// while the tree is built and again through every paint phase, so a
/// window's panels and canvases read its own playback's colors. Built with
/// [`window_body`], which snapshots the tint and runs the body inside it.
pub struct WindowTint {
    tint: palette::Tint,
    /// Whether this window always paints the cover backdrop, pushed
    /// through the phases beside the tint so the surface accessors know
    /// to keep their transparency. Children leave it off and follow the
    /// All Windows switch.
    backdropped: bool,
    child: AnyElement,
}

/// Build a window body under its player's art tint. The body closure runs
/// with the tint pushed so render-time color reads see it, and the tint
/// rides along into the paint phases through the returned element.
pub fn window_body(player: gpui::EntityId, body: impl FnOnce() -> AnyElement) -> WindowTint {
    tinted_body(player, false, body)
}

/// [`window_body`] for a workspace window, which paints the backdrop
/// whatever the All Windows switch says, so its surfaces keep their
/// transparency over it.
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
/// One labeled row of a customize window: the setting's name and its
/// control on one line, an optional dimmed description wrapping below.
/// What a [`banner`] is telling you, which picks its color and its face.
#[derive(Clone, Copy, PartialEq)]
pub enum Tone {
    /// Just so you know. The state is fine and unremarkable.
    Info,
    /// The good outcome, called out because it's the one worth confirming.
    Good,
    /// Something is standing in for what was asked.
    Warn,
    /// Something failed.
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

/// A callout: a tinted box with a rule down its edge, a face, a headline,
/// and however many lines of detail under it. For state a row can't carry,
/// where what happened needs more than a value and the difference between
/// fine and not fine should be visible before anything is read.
///
/// The tint is the tone at low alpha over whatever the surface already is,
/// so it reads on both themes and under the art wash without a second set
/// of colors.
pub fn banner(tone: Tone, headline: impl Into<SharedString>, lines: Vec<SharedString>) -> Div {
    banner_shaped(tone, headline, lines, false)
}

/// The same callout, flowing: the reasons ride beside the headline while
/// there's width for them and drop under it when there isn't. For a panel
/// that has to earn its height, where a block stacked three lines deep to
/// say two short things wastes the strip it's parked in.
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
    // The face rides the headline's own row rather than the whole block, so
    // it centers against that one line however many lines follow and however
    // far they wrap. Hanging it off the block instead left it floating high
    // the moment a reason wrapped to two lines.
    let head = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_SM)
        // Only where the row it sits in has a width to shrink against. In a
        // block sized by its own content, a zero minimum is read as
        // min-content and the headline comes out one glyph per line.
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
        // Roomier on the left than the other three sides: the rule and the
        // face are stacked up against that edge, and at even padding they
        // crowd it.
        .pl(tokens::SPACE_MD)
        .rounded(tokens::RADIUS)
        .bg(palette::alpha(color, 0x1c))
        .border_l(px(2.))
        .border_color(color);
    if flow {
        // One wrapping row. Where a line breaks is decided on the items'
        // natural widths, so the reasons ride along beside the headline
        // until they stop fitting and take their own line; min_w_0 is only
        // for the reason too long for even that, which wraps inside itself
        // the way it does stacked.
        return shell
            .flex_row()
            .flex_wrap()
            .items_center()
            .child(head)
            .children(lines.into_iter().map(reason));
    }
    // Detail hangs under the headline's text, clear of the face: the icon
    // plus the gap it sits behind.
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

/// [`setting_row`] with a built description, for the rare row whose note
/// carries live numbers rather than fixed copy.
pub fn setting_row_dyn(
    label: impl Into<SharedString>,
    description: Option<SharedString>,
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
                .child(div().flex_none().child(control)),
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

/// A labeled block of a customize window: like [`setting_row`] but the
/// control spans the full width below the description instead of sitting
/// inline. Wrapping controls need this - the row's control slot is
/// content-sized, and a wrap container without a definite width collapses
/// to one item per line. An optional trailing control rides the label
/// row's right edge, where a section's reset button lives.
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

/// One option in a [`mode_list`]: what it's called, what it does, and the
/// value it stands for.
pub struct ModeSpec<V> {
    pub label: SharedString,
    /// A sentence, not a phrase. The whole reason this control exists rather
    /// than a segmented picker is that these options differ in kind, and a
    /// picker leaves every option but the one you're looking at unexplained.
    pub description: SharedString,
    pub value: V,
}

/// A pick-one list where every option explains itself: a stacked row per
/// option, the chosen one marked and accented.
///
/// For modes that differ in kind rather than degree, where the difference is
/// the thing that needs saying. [`choices`] is still right for a short row of
/// obvious alternatives; this is for the ones that need a sentence each.
///
/// `available` refuses an option the way [`choices_gated`] does: it dims and
/// takes no press, since a mode that can't do anything yet should say so from
/// where it sits rather than vanish and leave nothing to explain.
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
                // No width of its own: the row stretches to the list, which
                // stretches to the page column, and that's what gives the
                // description a width to wrap inside. An explicit `w_full`
                // here is worse than nothing, since a percentage against a
                // parent that hasn't resolved its own width falls back to
                // auto, and the row shrinks to its longest line. `min_w_0` is
                // the CSS one: it stops long copy pushing the row wider than
                // what it was stretched to.
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
                // The dot rides the label's own line rather than the whole
                // row, so it centers on the text at any app font size instead
                // of floating against the top of a description that wrapped.
                // It says pick-one where a check would say on-and-off, which
                // is the wrong promise for a list only one row can win.
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
                // Indented past the dot so the description reads as the
                // label's, not as another row.
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
/// The settings-page sliders' strip width and the readout beside them.
pub const SLIDER_W: Pixels = px(150.);
pub const READOUT_W: Pixels = px(60.);

/// How wide a scrub strip draws.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SliderWidth {
    /// [`SLIDER_W`], so every slider down a settings page lines up in one
    /// control column whatever its label.
    Fixed,
    /// Whatever room the parent gives it. For a dialog, where there's no
    /// column to line up with and a short strip adrift in a wide box reads
    /// as a layout mistake rather than a choice.
    Fill,
}

/// The scrub strip alone: the shared slider chrome over a drag surface,
/// applying the strip fraction live on click and drag. The row builders
/// below pair it with their readout.
fn slider_strip<P: 'static>(
    scrub: &ScrubState,
    fraction: f32,
    width: SliderWidth,
    apply: impl Fn(&mut P, f32, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    let entity = cx.entity();
    div()
        .map(|d| match width {
            SliderWidth::Fixed => d.w(SLIDER_W).flex_none(),
            SliderWidth::Fill => d.flex_1(),
        })
        .h(tokens::CONTROL_H)
        .cursor_pointer()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener({
                let scrub = scrub.clone();
                let apply = apply.clone();
                move |this: &mut P, event: &MouseDownEvent, _, cx| {
                    scrub.begin();
                    if let Some(fraction) = scrub.fraction(event.position.x) {
                        apply(this, fraction, cx);
                    }
                    cx.notify();
                }
            }),
        )
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
}

/// One in-flight readout edit across a panel's settings sliders: which
/// strip is being typed into and the input holding the text. One per
/// panel, behind Arcs like [`ScrubState`], so the row builders only need
/// a read and a second click simply moves the edit.
#[derive(Clone, Default)]
pub struct ValueEdit {
    inner: Arc<Mutex<ValueEditInner>>,
}

#[derive(Default)]
struct ValueEditInner {
    active: Option<usize>,
    input: Option<Entity<InputState>>,
    /// Keeps the enter/blur subscription alive exactly as long as the
    /// edit; replaced wholesale when the edit moves to another strip.
    events: Option<Subscription>,
    /// Where the input painted, for the click-outside cancel: a press
    /// anywhere else abandons the edit without committing.
    bounds: Option<Bounds<Pixels>>,
}

impl ValueEdit {
    /// The input to render for strip `id` while it is the one being
    /// edited.
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

/// [`value_slider`] whose readout doubles as an input: click the number,
/// type, Enter commits, blur cancels. `edit_text` seeds the field with the
/// bare number, no unit; `to_fraction` maps the typed value back into the
/// strip's 0..1 through the row's own mapping (linear, log, whatever the
/// slider itself runs), and the result clamps to the strip before it
/// applies.
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

/// [`value_slider_edit`] with typed headroom past the strip's top: `over`
/// is the highest fraction a typed value may reach, for knobs whose
/// slider range is a sensible reach rather than a law. The strip still
/// scrubs its own span and pins full while the value sits beyond it.
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
        to_fraction,
        apply,
        cx,
    )
}

/// The same, with the strip's width said out loud. The settings pages want
/// [`SliderWidth::Fixed`] and get it from the wrapper above; a dialog builds
/// its own row and asks for [`SliderWidth::Fill`].
#[allow(clippy::too_many_arguments)]
pub fn value_slider_edit_sized<P: 'static>(
    scrub: &ScrubState,
    edit: &ValueEdit,
    fraction: f32,
    readout: String,
    edit_text: String,
    over: f32,
    width: SliderWidth,
    to_fraction: impl Fn(f32) -> f32 + Clone + 'static,
    apply: impl Fn(&mut P, f32, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    let row = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_SM)
        // A filling strip only fills if the row it sits in does too.
        .map(|d| match width {
            SliderWidth::Fixed => d,
            SliderWidth::Fill => d.w_full(),
        })
        .child(slider_strip(scrub, fraction, width, apply.clone(), cx));
    if let Some(input) = edit.editing(scrub.id()) {
        // While the edit is live, a one-frame window handler (the
        // scrub_on_paint idiom) watches for a press outside the input and
        // abandons the edit uncommitted: nothing else in the settings
        // window takes focus, so blur alone never fires.
        let id = scrub.id();
        let entity = cx.entity();
        return row.child(
            div()
                .w(READOUT_W)
                // Pinned to the strip's height with the input centered: the
                // small input is 2px taller than CONTROL_H (its border), and
                // left to size the row it nudges the whole page on toggle.
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
            .w(READOUT_W)
            .flex_none()
            .text_right()
            .text_color(palette::text_muted())
            // The hover cue is a background, never a text restyle: a hover
            // text refinement re-shapes the line with its own metrics and
            // the number visibly shifts under the pointer.
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
                                    let text = input.read(cx).value().trim().replace(',', ".");
                                    if let Ok(value) = text.parse::<f32>() {
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
/// The switch pill and knob without any interaction, shared by [`toggle`],
/// [`toggle_locked`], and the menu rows that flip a switch from their own
/// click rather than from the widget.
pub fn toggle_face(on: bool) -> Div {
    toggle_track(on)
}

/// The switch pill and knob without any interaction, shared by [`toggle`] and
/// [`toggle_locked`].
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

/// An on/off switch: a pill track, the knob in the accent on the far side
/// while on.
pub fn toggle<P: 'static>(
    on: bool,
    on_change: impl Fn(&mut P, bool, &mut Context<P>) + 'static,
    cx: &mut Context<P>,
) -> Div {
    toggle_track(on).cursor_pointer().on_mouse_down(
        MouseButton::Left,
        cx.listener(move |this, _, _, cx| on_change(this, !on, cx)),
    )
}

/// A [`toggle`] the user cannot flip: dimmed and inert, the same shape as the
/// live switch. For a setting the app is holding at a value, like the watch
/// switch a library grows too large to arm.
pub fn toggle_locked(on: bool) -> Div {
    toggle_track(on).opacity(0.5)
}

/// How long a run of keystrokes stays one type-ahead phrase: a pause past
/// this starts the buffer over. Shared by every panel that jumps by prefix.
pub const TYPE_AHEAD: Duration = Duration::from_millis(1000);

/// Grow or restart a type-ahead buffer for the keystroke `text`: within the
/// window since the last stroke the letters build one phrase, past it the
/// phrase starts fresh. Stamps `at` with now and returns whether the phrase
/// grew, which the callers use to decide the match re-tests the current row
/// or steps past it. The prefix match and the scroll that follow stay per
/// panel, since the list widget and what a row's text is differ.
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
/// The shared "tracking" section for a panel's Behavior page: the
/// follow-playing toggle and, while it is on, the smooth-scrolling toggle,
/// under one header so the library, the grids, and the art shelf all read
/// the same. The wording of what it follows (a row, an album, the center)
/// differs per panel, so both descriptions are passed in; the toggles carry
/// each panel's own follow and glide handlers.
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
    // Both the follow and the resume ride the same glide, so the motion
    // toggle earns its place the moment either is on.
    if follow || resume {
        body = body.child(setting_row(
            rox_i18n::t!("tracking-smooth"),
            Some(smooth_desc),
            toggle(smooth, on_smooth, cx),
        ));
    }
    ui::section(rox_i18n::t!("tracking-title"), None, body).into_any_element()
}
/// A dropdown over a list of choices: a small button labeled with whichever
/// option is current, its menu the whole list with a tick on that one. Use
/// it where [`choices`] would run out of room, a picker whose list is
/// however many the machine happens to have rather than a fixed two or
/// three. `disabled` draws it inert, for a knob whose mode doesn't apply.
pub fn picker<P, K>(
    id: &'static str,
    current: K,
    options: Vec<(K, SharedString)>,
    disabled: bool,
    apply: impl Fn(&mut P, K, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> impl IntoElement
where
    P: 'static,
    K: PartialEq + Clone + 'static,
{
    // An id that isn't in the list still has to label the button, so fall
    // back to the head rather than drawing an empty one: a device that was
    // unplugged since the pick reads as the default it will actually open.
    // The tick follows the same fallback, so the open menu points at the row
    // the button is already naming instead of checking nothing.
    let picked = options
        .iter()
        .find(|(key, _)| *key == current)
        .or_else(|| options.first());
    let label = picked.map(|(_, label)| label.clone()).unwrap_or_default();
    let current = picked.map(|(key, _)| key.clone());
    let weak = cx.entity().downgrade();
    // A list past a screenful runs off the bottom of the window and clips,
    // with nothing to reach the rest. gpui-component only
    // turns the scrollbar on for menus built through its own `with_menu_items`,
    // which the builder below doesn't go through, so cap the height and hand it
    // a scrollbar here. Same threshold upstream uses.
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

/// The chrome shared by the segmented pickers and the toggle groups: a
/// joined group of segments, the picked ones filled with the accent,
/// hairline gaps between the rest. The predicate says which segments
/// read as on; the exclusive pickers pass equality with the current
/// value, the toggle groups each flag's own state.
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
        // A segment nothing can pick is dimmed and inert, `toggle_locked`'s
        // treatment: it keeps its place in the group so the choice still
        // reads as a choice, and says without a click that it isn't one
        // right now.
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

/// A segmented picker of exclusive choices, labeled with text.
pub fn choices<P: 'static, V: PartialEq + Copy + 'static>(
    options: &'static [(&'static str, V)],
    current: V,
    on_pick: impl Fn(&mut P, V, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    choices_gated(options, current, |_| true, on_pick, cx)
}

/// [`choices`] with owned labels, for options translated at render time
/// rather than written as literals. New rows whose labels go through
/// rox-i18n land here; [`choices`] keeps the static shape until its call
/// sites migrate with their pages.
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

/// [`choices`] where some options can't be taken yet: whatever `available`
/// refuses is dimmed and swallows no press.
///
/// For a choice that exists but needs something first, where dropping the
/// option entirely would leave the row unable to say what's missing. The
/// description beside it is what explains why; this only stops the press
/// that would otherwise land and appear to do nothing.
pub fn choices_gated<P: 'static, V: PartialEq + Copy + 'static>(
    options: &'static [(&'static str, V)],
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

/// A segmented picker of exclusive choices, labeled with icons; each option
/// pairs an icon path from [`rox_design::assets::icons`] with its value.
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

/// A joined group of independent icon toggles: the segmented pickers'
/// chrome, but each segment flips its own flag instead of one pick
/// excluding the rest.
pub fn icon_toggles<P: 'static, V: PartialEq + Copy + 'static>(
    options: &'static [(&'static str, V)],
    active: impl Fn(V) -> bool,
    on_toggle: impl Fn(&mut P, V, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    segments(options, active, |_| true, icon_segment, on_toggle, cx)
}

/// One icon segment's face, shared by the exclusive picker and the
/// toggle group.
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
/// Where a panel's content sits horizontally, the cross-panel
/// customization knob.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// Apply an alignment along a row's main axis.
pub fn justify(d: Div, align: Align) -> Div {
    match align {
        Align::Left => d.justify_start(),
        Align::Center => d.justify_center(),
        Align::Right => d.justify_end(),
    }
}

/// Apply an alignment along the cross axis, so a column's children sit
/// left, center, or right the way `justify` places a row's.
pub fn items(d: Div, align: Align) -> Div {
    match align {
        Align::Left => d.items_start(),
        Align::Center => d.items_center(),
        Align::Right => d.items_end(),
    }
}

/// The alignment setting row the panels' customize windows share.
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

/// Where a panel's content sits vertically, the companion to [`Align`]
/// for a panel that has height to spare.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VAlign {
    Top,
    #[default]
    Middle,
    Bottom,
}

/// Apply a vertical alignment along a column's main axis.
pub fn justify_v(d: Div, align: VAlign) -> Div {
    match align {
        VAlign::Top => d.justify_start(),
        VAlign::Middle => d.justify_center(),
        VAlign::Bottom => d.justify_end(),
    }
}

/// The vertical alignment setting row, the companion to [`align_row`].
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
