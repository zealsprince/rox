//! The shell the app settings window and every panel settings window share:
//! sidebar, sections, buttons, the scalar slider, and the role grid. Page
//! content stays with each window.

use std::rc::Rc;

use gpui::{
    AnyElement, App, Context, Div, ElementId, FocusHandle, Global, Interactivity, KeyDownEvent,
    MouseButton, Pixels, ScrollHandle, SharedString, Stateful, StyleRefinement, Window, div,
    prelude::*, px, svg,
};
use gpui_component::Selectable;
use gpui_component::scroll::Scrollbar;

use rox_design::assets::icons;
use rox_design::palette::{self, ROLES, Side, Sides};
use rox_design::tokens;

/// A control was pressed, by pointer or keyboard. Not gpui's `ClickEvent`:
/// these controls take no ids, and two same-named buttons under one parent
/// would share gpui's click state and lose a press.
pub struct Press;

pub type OnPress = Rc<dyn Fn(&Press, &mut Window, &mut App)>;

use crate as panel;
use crate::ScrubState;

/// The caller wires the click on the surrounding row.
pub fn checkbox(on: bool) -> Div {
    div()
        .size(px(16.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(tokens::RADIUS)
        .border_1()
        .border_color(if on {
            palette::accent()
        } else {
            palette::border()
        })
        .bg(if on {
            palette::accent()
        } else {
            palette::bg_control()
        })
        .when(on, |d| {
            d.child(
                svg()
                    .path(icons::CHECK)
                    .size(px(11.))
                    .text_color(palette::text_on_accent()),
            )
        })
}

pub const SIDEBAR_W: Pixels = px(160.);

/// The swatch, its gap, and the longest role label.
pub const COLOR_CELL_MIN_W: Pixels = px(150.);

pub const SECTION_GAP: Pixels = px(20.);

pub const MIN_SIZE: gpui::Size<Pixels> = gpui::Size {
    width: px(560.),
    height: px(400.),
};

/// Two at the window floor, up to four.
pub fn grid_columns(window: &Window) -> usize {
    let page_w = window.viewport_size().width - SIDEBAR_W - tokens::SPACE_MD * 2.;
    usize::clamp((page_w / COLOR_CELL_MIN_W) as usize, 2, 4)
}

pub fn sidebar() -> Div {
    div()
        .w(SIDEBAR_W)
        .flex_none()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_XS)
        .p(tokens::SPACE_SM)
        .bg(palette::bg_panel())
        .border_r_1()
        .border_color(palette::border())
}

/// The sidebar is fixed to the window height, so the nav scrolls or its
/// last pages are unreachable.
pub fn nav_scroll(
    id: impl Into<ElementId>,
    scroll: &ScrollHandle,
    build: impl FnOnce(Stateful<Div>) -> Stateful<Div>,
) -> Div {
    div()
        .flex_1()
        .min_h_0()
        .relative()
        .child(build(
            div()
                .id(id)
                .size_full()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_XS)
                .overflow_y_scroll()
                .track_scroll(scroll),
        ))
        .child(
            div()
                .absolute()
                .inset_0()
                .child(Scrollbar::vertical(scroll)),
        )
}

pub fn nav_item<P: 'static>(
    label: impl Into<SharedString>,
    icon: &'static str,
    picked: bool,
    on_pick: impl Fn(&mut P, &mut Window, &mut Context<P>) + 'static,
    cx: &mut Context<P>,
) -> NavRow {
    nav_row(label, icon, picked, false, on_pick, cx)
}

/// A shade back, for a page that isn't one of the subjects.
pub fn nav_item_quiet<P: 'static>(
    label: impl Into<SharedString>,
    icon: &'static str,
    picked: bool,
    on_pick: impl Fn(&mut P, &mut Window, &mut Context<P>) + 'static,
    cx: &mut Context<P>,
) -> NavRow {
    nav_row(label, icon, picked, true, on_pick, cx)
}

fn nav_row<P: 'static>(
    label: impl Into<SharedString>,
    icon: &'static str,
    picked: bool,
    quiet: bool,
    on_pick: impl Fn(&mut P, &mut Window, &mut Context<P>) + 'static,
    cx: &mut Context<P>,
) -> NavRow {
    let label = label.into();
    NavRow {
        id: ElementId::Name(format!("nav:{label}").into()),
        base: div()
            .px(tokens::SPACE_MD)
            .py(tokens::SPACE_XS)
            .rounded(tokens::RADIUS)
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .cursor_pointer()
            .when(picked, |d| d.bg(palette::bg_control_active()))
            .when(quiet, |d| d.text_color(palette::text_muted())),
        label,
        icon,
        picked,
        quiet,
        on_pick: Rc::new(cx.listener(move |this, _, window, cx| on_pick(this, window, cx))),
    }
}

#[derive(IntoElement)]
pub struct NavRow {
    id: ElementId,
    base: Div,
    label: SharedString,
    icon: &'static str,
    picked: bool,
    quiet: bool,
    on_pick: OnPress,
}

impl Styled for NavRow {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl InteractiveElement for NavRow {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl RenderOnce for NavRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let ink = if self.quiet {
            palette::text_muted()
        } else {
            palette::text()
        };
        let focus = control_focus(self.id.clone(), window, cx);
        let focused = focus.is_focused(window);
        let picked = self.picked;
        let base = self
            .base
            .key_context(CONTROL_CONTEXT)
            .track_focus(&focus.tab_index(0).tab_stop(true))
            .when(!picked, |d| d.hover(|d| d.bg(palette::bg_menu_hover())));
        pressable(base, self.on_pick)
            .child(
                svg()
                    .path(self.icon)
                    .size(px(14.))
                    .flex_none()
                    .text_color(ink),
            )
            .child(self.label)
            .children(focus_ring(focused, tokens::RADIUS, cx))
    }
}

/// Inset to the row text rather than cutting the whole sidebar.
pub fn nav_divider() -> Div {
    div()
        .mx(tokens::SPACE_MD)
        .my(tokens::SPACE_XS)
        .h(px(1.))
        .flex_none()
        .bg(palette::border())
}

pub fn header(label: impl Into<SharedString>) -> Div {
    div()
        .pt(tokens::SPACE_SM)
        .text_xs()
        .text_color(palette::text_muted())
        .child(label.into())
}

pub fn chord(key: &str) -> SharedString {
    if cfg!(target_os = "macos") {
        format!("Cmd+{key}")
    } else {
        format!("Ctrl+{key}")
    }
    .into()
}

pub enum Seg {
    Text(SharedString),
    Key(SharedString),
}

pub fn kbd(label: SharedString) -> Div {
    div()
        .flex_none()
        .px(px(5.))
        .rounded(px(4.))
        .border_1()
        .border_color(palette::border())
        .bg(palette::bg_control())
        .text_xs()
        .text_color(palette::text())
        .child(label)
}

/// Splits the copy into words so the line wraps like prose around the chips.
pub fn kbd_line(segs: impl IntoIterator<Item = Seg>) -> Div {
    div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_x(px(4.))
        .gap_y(px(4.))
        .text_color(palette::text_muted())
        .children(segs.into_iter().flat_map(|seg| {
            match seg {
                Seg::Text(text) => text
                    .split_whitespace()
                    .map(|word| div().child(word.to_string()))
                    .collect(),
                Seg::Key(label) => vec![kbd(label)],
            }
        }))
}

pub fn section(
    label: impl Into<SharedString>,
    trailing: Option<AnyElement>,
    body: impl IntoElement,
) -> Stateful<Div> {
    section_with_icon(None, label, trailing, body)
}

pub fn section_with_control(
    label: impl Into<SharedString>,
    control: AnyElement,
    trailing: Option<AnyElement>,
    body: impl IntoElement,
) -> Stateful<Div> {
    build_section(None, label, Some(control), trailing, body)
}

/// Required on the settings window's sealed path so no section ships bare.
pub fn section_with_icon(
    icon: Option<&'static str>,
    label: impl Into<SharedString>,
    trailing: Option<AnyElement>,
    body: impl IntoElement,
) -> Stateful<Div> {
    build_section(icon, label, None, trailing, body)
}

fn build_section(
    icon: Option<&'static str>,
    label: impl Into<SharedString>,
    control: Option<AnyElement>,
    trailing: Option<AnyElement>,
    body: impl IntoElement,
) -> Stateful<Div> {
    let label = label.into();
    div()
        // Named after its heading, which scopes the ids of everything inside:
        // same-named buttons in two sections stay separate controls.
        .id(ElementId::Name(label.clone()))
        .flex()
        .flex_col()
        .gap(tokens::SPACE_SM)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .pb(tokens::SPACE_XS)
                .border_b_1()
                .border_color(palette::border())
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(tokens::SPACE_XS)
                        .text_xs()
                        .text_color(palette::text_muted())
                        .when_some(icon, |d, icon| {
                            d.child(
                                svg()
                                    .path(icon)
                                    .size(px(12.))
                                    .flex_none()
                                    .text_color(palette::text_muted()),
                            )
                        })
                        .child(label)
                        .when_some(control, |d, control| d.child(control)),
                )
                .when_some(trailing, |d, trailing| d.child(trailing)),
        )
        .child(body)
}

/// A row matches when every term appears in its label, description, or
/// keywords, case folded.
pub struct Query {
    terms: Vec<String>,
}

impl Query {
    pub fn parse(text: &str) -> Self {
        Self {
            terms: text.split_whitespace().map(rox_i18n::fold).collect(),
        }
    }

    pub fn active(&self) -> bool {
        !self.terms.is_empty()
    }

    fn hits(&self, texts: &[&str]) -> bool {
        // Fold once per candidate: this runs over the whole settings tree on
        // every keystroke.
        let folded: Vec<String> = texts.iter().map(|text| rox_i18n::fold(text)).collect();
        self.terms
            .iter()
            .all(|term| folded.iter().any(|text| text.contains(term.as_str())))
    }
}

/// The only shape the settings window takes back from a page builder, so no
/// row can go on a page without declaring its search terms.
///
/// Search builds every page each keystroke, so page builders must stay pure
/// reads: no spawns, no entity updates outside listeners.
pub struct PageBody {
    body: Div,
    hits: usize,
}

impl PageBody {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            body: div().flex().flex_col().gap(SECTION_GAP),
            hits: 0,
        }
    }

    pub fn section(mut self, section: Section) -> Self {
        if let Some(body) = section.body {
            self.body = self.body.child(body);
            self.hits += section.hits;
        }
        self
    }

    pub fn when(self, condition: bool, then: impl FnOnce(Self) -> Self) -> Self {
        if condition { then(self) } else { self }
    }

    /// Zero drops the page from the results and dims its sidebar entry.
    pub fn hits(&self) -> usize {
        self.hits
    }

    pub fn element(self) -> AnyElement {
        self.body.into_any_element()
    }
}

pub struct Section {
    body: Option<Stateful<Div>>,
    hits: usize,
}

impl Section {
    /// A query hitting the section's own label keeps the whole section.
    pub fn new(
        q: &Query,
        icon: &'static str,
        label: impl Into<SharedString>,
        trailing: Option<AnyElement>,
        build: impl FnOnce(Rows) -> Rows,
    ) -> Self {
        let label = label.into();
        let all = !q.active() || q.hits(&[label.as_ref()]);
        let rows = build(Rows {
            q,
            all,
            body: div().flex().flex_col().gap(tokens::SPACE_MD),
            hits: 0,
        });
        if rows.hits == 0 {
            return Self {
                body: None,
                hits: 0,
            };
        }
        Self {
            body: Some(section_with_icon(Some(icon), label, trailing, rows.body)),
            hits: rows.hits,
        }
    }
}

/// `all` short-circuits the checks while no search is on or the section's
/// own name matched.
pub struct Rows<'a> {
    q: &'a Query,
    all: bool,
    body: Div,
    hits: usize,
}

impl Rows<'_> {
    pub fn row(
        mut self,
        label: impl Into<SharedString>,
        description: Option<SharedString>,
        control: impl IntoElement,
    ) -> Self {
        let label = label.into();
        if self.keep(&[], &label, description.as_ref().map(|d| d.as_ref())) {
            self.body = self
                .body
                .child(crate::setting_row_dyn(label, description, control));
            self.hits += 1;
        }
        self
    }

    /// [`Rows::row`] from a message key, so the row matches the active
    /// locale's `.keywords` too. `keywords` stays English and always matches:
    /// audio terms like "gapless" travel untranslated.
    pub fn keyed(
        mut self,
        key: &'static str,
        keywords: &[&str],
        control: impl IntoElement,
    ) -> Self {
        let label = rox_i18n::t!(key);
        let description = rox_i18n::try_translate(&format!("{key}.description"));
        let local = rox_i18n::try_translate(&format!("{key}.keywords"));
        let mut terms: Vec<&str> = keywords.to_vec();
        if let Some(local) = local.as_deref() {
            terms.extend(local.split_whitespace());
        }
        if self.keep(&terms, &label, description.as_ref().map(|d| d.as_ref())) {
            self.body = self
                .body
                .child(crate::setting_row_dyn(label, description, control));
            self.hits += 1;
        }
        self
    }

    /// Matches the label and keywords only, never text that moves.
    pub fn row_dyn(
        mut self,
        keywords: &[&str],
        label: impl Into<SharedString>,
        description: Option<SharedString>,
        control: impl IntoElement,
    ) -> Self {
        let label = label.into();
        if self.keep(keywords, &label, None) {
            self.body = self
                .body
                .child(crate::setting_row_dyn(label, description, control));
            self.hits += 1;
        }
        self
    }

    /// For content that isn't a plain row. `build` only runs when kept.
    pub fn custom(mut self, keywords: &[&str], build: impl FnOnce() -> AnyElement) -> Self {
        if self.all || self.q.hits(keywords) {
            self.body = self.body.child(build());
            self.hits += 1;
        }
        self
    }

    pub fn when(self, condition: bool, then: impl FnOnce(Self) -> Self) -> Self {
        if condition { then(self) } else { self }
    }

    pub fn when_some<T>(self, value: Option<T>, then: impl FnOnce(Self, T) -> Self) -> Self {
        match value {
            Some(value) => then(self, value),
            None => self,
        }
    }

    fn keep(&self, keywords: &[&str], label: &str, description: Option<&str>) -> bool {
        if self.all {
            return true;
        }
        let mut texts = Vec::with_capacity(keywords.len() + 2);
        texts.push(label);
        if let Some(description) = description {
            texts.push(description);
        }
        texts.extend_from_slice(keywords);
        self.q.hits(&texts)
    }
}

/// A block header inside a section, ruled lighter than the section's own.
pub fn block_header(label: impl IntoElement, trailing: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(tokens::SPACE_MD)
        .pb(tokens::SPACE_XS)
        .border_b_1()
        .border_color(palette::alpha(palette::border(), 0x80))
        .child(label)
        .child(trailing)
}

/// A block nested under the row that owns it, like a route's editor under its
/// knob.
pub fn nested(body: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_row()
        .gap(tokens::SPACE_SM)
        .pt(tokens::SPACE_XS)
        .child(
            div()
                .flex_none()
                .w(px(2.))
                .rounded_full()
                .bg(palette::alpha(palette::accent(), 0x55)),
        )
        .child(div().flex_1().child(body))
}

/// A control's focus handle, kept by the window under the control's own name.
///
/// Every call site chains `.tab_index(0).tab_stop(true)` before
/// `track_focus`: the index alone inserts the handle but Tab skips it. Two
/// controls under the same name share a handle, so a page that repeats a
/// label names the control through the builder's `keyed`.
pub fn control_focus(id: impl Into<ElementId>, window: &mut Window, cx: &mut App) -> FocusHandle {
    window
        .use_keyed_state(id.into(), cx, |_, cx| cx.focus_handle())
        .read(cx)
        .clone()
}

/// Carves the keys a focused control needs out of the bare playback chords.
/// Only in the dispatch path while the control holds focus.
pub const CONTROL_CONTEXT: &str = "FocusedControl";

/// Pointer on the way down, Enter or Space while focused. The keys are taken
/// here because gpui's keyboard click needs an id; see [`Press`].
pub(crate) fn pressable<E: InteractiveElement>(element: E, on_press: OnPress) -> E {
    let keys = on_press.clone();
    element
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            // gpui focuses on this same press, so mark it a pointer focus
            // before that lands.
            note_pointer_press(cx);
            on_press(&Press, window, cx)
        })
        .on_key_down(move |event: &KeyDownEvent, window, cx| {
            if event.keystroke.modifiers.modified() {
                return;
            }
            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                keys(&Press, window, cx);
                cx.stop_propagation();
            }
        })
}

/// gpui's stand-in for `:focus-visible`: a keystroke sets it, a pointer press
/// clears it. App-wide, since every window's controls read the one flag.
struct FocusVisible(bool);

impl Global for FocusVisible {}

/// Call once at startup.
pub fn init(cx: &mut App) {
    cx.set_global(FocusVisible(true));
    cx.intercept_keystrokes(|_, _, cx| cx.set_global(FocusVisible(true)))
        .detach();
}

/// True before [`init`] runs, since showing a ring is the safe default.
fn focus_visible(cx: &App) -> bool {
    cx.try_global::<FocusVisible>().map(|v| v.0).unwrap_or(true)
}

/// For a control that wires its own mouse down instead of going through
/// `pressable`.
pub(crate) fn note_pointer_press(cx: &mut App) {
    cx.set_global(FocusVisible(false));
}

/// Sits outside the bounds so focusing never shifts the layout.
pub fn focus_ring(focused: bool, radius: Pixels, cx: &App) -> Option<Div> {
    (focused && focus_visible(cx)).then(|| {
        div()
            .absolute()
            .flex_none()
            .inset(-RING)
            .border(RING)
            .rounded(radius + RING)
            .border_color(palette::alpha(palette::accent(), 0xaa))
    })
}

const RING: Pixels = px(1.5);

pub fn small_button(
    label: impl Into<SharedString>,
    icon: &'static str,
    inert: bool,
    on_click: impl Fn(&Press, &mut Window, &mut App) + 'static,
) -> SmallButton {
    let label = label.into();
    SmallButton {
        id: ElementId::Name(format!("{label}:{icon}").into()),
        base: div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(tokens::SPACE_XS)
            .px(tokens::SPACE_SM)
            .py(px(2.))
            .text_xs()
            .rounded(tokens::RADIUS)
            .bg(palette::bg_control())
            .when(inert, |d| d.opacity(0.5)),
        label,
        icon,
        inert,
        on_click: Rc::new(on_click),
    }
}

#[derive(IntoElement)]
pub struct SmallButton {
    id: ElementId,
    base: Div,
    label: SharedString,
    icon: &'static str,
    inert: bool,
    on_click: OnPress,
}

impl SmallButton {
    /// For a page with more than one button saying the same thing. See
    /// [`control_focus`].
    pub fn keyed(mut self, id: impl Into<ElementId>) -> Self {
        self.id = id.into();
        self
    }
}

impl Styled for SmallButton {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl InteractiveElement for SmallButton {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl RenderOnce for SmallButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = control_focus(self.id.clone(), window, cx);
        let focused = focus.is_focused(window);
        let inert = self.inert;
        self.base
            .key_context(CONTROL_CONTEXT)
            .map(|d| {
                if inert {
                    d
                } else {
                    pressable(
                        d.track_focus(&focus.tab_index(0).tab_stop(true))
                            .hover(|d| d.bg(palette::bg_control_hover()))
                            .cursor_pointer(),
                        self.on_click,
                    )
                }
            })
            .child(
                svg()
                    .path(self.icon)
                    .size(px(12.))
                    .flex_none()
                    .text_color(palette::text()),
            )
            .child(self.label)
            .children(focus_ring(focused, tokens::RADIUS, cx))
    }
}

/// The primary one fills with the accent.
pub fn dialog_button(
    label: impl Into<SharedString>,
    primary: bool,
    on_click: impl Fn(&Press, &mut Window, &mut App) + 'static,
) -> DialogButton {
    let label = label.into();
    DialogButton {
        id: ElementId::Name(label.clone()),
        base: div()
            .flex_none()
            .px(tokens::SPACE_MD)
            .py(tokens::SPACE_XS)
            .rounded(tokens::RADIUS)
            .cursor_pointer()
            .map(|d| {
                if primary {
                    d.bg(palette::accent())
                        .text_color(palette::text_on_accent())
                } else {
                    d.bg(palette::bg_control())
                }
            }),
        label,
        primary,
        on_click: Rc::new(on_click),
    }
}

#[derive(IntoElement)]
pub struct DialogButton {
    id: ElementId,
    base: Div,
    label: SharedString,
    primary: bool,
    on_click: OnPress,
}

impl DialogButton {
    pub fn keyed(mut self, id: impl Into<ElementId>) -> Self {
        self.id = id.into();
        self
    }
}

impl Styled for DialogButton {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl InteractiveElement for DialogButton {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl RenderOnce for DialogButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = control_focus(self.id.clone(), window, cx);
        let focused = focus.is_focused(window);
        let primary = self.primary;
        let base = self
            .base
            .key_context(CONTROL_CONTEXT)
            .track_focus(&focus.tab_index(0).tab_stop(true))
            .map(|d| {
                if primary {
                    d.hover(|d| d.opacity(0.9))
                } else {
                    d.hover(|d| d.bg(palette::bg_control_hover()))
                }
            });
        pressable(base, self.on_click)
            .child(self.label)
            .children(focus_ring(focused, tokens::RADIUS, cx))
    }
}

/// For the secondary action in a dialog's footer beside the confirm pair.
pub fn dialog_icon_button(
    label: impl Into<SharedString>,
    icon: &'static str,
    inert: bool,
    on_click: impl Fn(&Press, &mut Window, &mut App) + 'static,
) -> DialogIconButton {
    let label = label.into();
    DialogIconButton {
        id: ElementId::Name(format!("{label}:{icon}").into()),
        base: div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(tokens::SPACE_XS)
            .px(tokens::SPACE_MD)
            .py(tokens::SPACE_XS)
            .rounded(tokens::RADIUS)
            .bg(palette::bg_control())
            .when(inert, |d| d.opacity(0.5)),
        label,
        icon,
        inert,
        on_click: Rc::new(on_click),
    }
}

#[derive(IntoElement)]
pub struct DialogIconButton {
    id: ElementId,
    base: Div,
    label: SharedString,
    icon: &'static str,
    inert: bool,
    on_click: OnPress,
}

impl DialogIconButton {
    pub fn keyed(mut self, id: impl Into<ElementId>) -> Self {
        self.id = id.into();
        self
    }
}

impl Styled for DialogIconButton {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl InteractiveElement for DialogIconButton {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl RenderOnce for DialogIconButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = control_focus(self.id.clone(), window, cx);
        let focused = focus.is_focused(window);
        let inert = self.inert;
        self.base
            .key_context(CONTROL_CONTEXT)
            .map(|d| {
                if inert {
                    d
                } else {
                    pressable(
                        d.track_focus(&focus.tab_index(0).tab_stop(true))
                            .hover(|d| d.bg(palette::bg_control_hover()))
                            .cursor_pointer(),
                        self.on_click,
                    )
                }
            })
            .child(
                svg()
                    .path(self.icon)
                    .size(px(14.))
                    .flex_none()
                    .text_color(palette::text()),
            )
            .child(self.label)
            .children(focus_ring(focused, tokens::RADIUS, cx))
    }
}

/// Room for a signal's derived name without pushing the row's label off its
/// line.
pub const SELECT_W: Pixels = px(190.);

/// A bordered field that drops its list, styled as a value the row holds
/// rather than an action. Attach the list with `DropdownMenu::dropdown_menu`.
///
/// The popup defers, so never host one inside another deferred overlay:
/// gpui 0.2.2 panics on nested deferred.
pub fn select_field(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    // A prompt rather than a pick, drawn muted like a placeholder.
    placeholder: bool,
) -> SelectField {
    SelectField {
        base: div()
            .id(id)
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(tokens::SPACE_XS)
            .w(SELECT_W)
            .h(tokens::CONTROL_H)
            .px(tokens::SPACE_SM)
            .text_xs()
            .overflow_hidden()
            .rounded(tokens::RADIUS)
            .bg(palette::bg_control())
            .border_1()
            .cursor_pointer(),
        label: label.into(),
        placeholder,
        open: false,
    }
}

#[derive(IntoElement)]
pub struct SelectField {
    base: Stateful<Div>,
    label: SharedString,
    placeholder: bool,
    open: bool,
}

impl Styled for SelectField {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl InteractiveElement for SelectField {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl Selectable for SelectField {
    fn selected(mut self, selected: bool) -> Self {
        self.open = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.open
    }
}

impl gpui_component::menu::DropdownMenu for SelectField {}

impl RenderOnce for SelectField {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let SelectField {
            base,
            label,
            placeholder,
            open,
        } = self;
        base.border_color(if open {
            palette::accent()
        } else {
            palette::border()
        })
        .hover(|d| d.bg(palette::bg_control_hover()))
        .child(
            div()
                .flex_1()
                .overflow_hidden()
                .when(placeholder, |d| d.text_color(palette::text_muted()))
                .child(label),
        )
        // No up chevron in the icon set; the lit border says it's open.
        .child(
            svg()
                .path(icons::CHEVRON_DOWN)
                .size(px(12.))
                .flex_none()
                .text_color(if open {
                    palette::accent()
                } else {
                    palette::text_muted()
                }),
        )
    }
}

/// An action with kinds to pick between. There's no inert state since the
/// popover owns the click: disable the menu items instead.
pub fn menu_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    icon: &'static str,
) -> MenuButton {
    MenuButton {
        base: div()
            .id(id)
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(tokens::SPACE_XS)
            .px(tokens::SPACE_SM)
            .py(px(2.))
            .text_xs()
            .rounded(tokens::RADIUS)
            .bg(palette::bg_control())
            .cursor_pointer(),
        label: label.into(),
        icon,
        open: false,
    }
}

#[derive(IntoElement)]
pub struct MenuButton {
    base: Stateful<Div>,
    label: SharedString,
    icon: &'static str,
    open: bool,
}

impl Styled for MenuButton {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl InteractiveElement for MenuButton {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl Selectable for MenuButton {
    fn selected(mut self, selected: bool) -> Self {
        self.open = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.open
    }
}

impl gpui_component::menu::DropdownMenu for MenuButton {}

impl RenderOnce for MenuButton {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let MenuButton {
            base,
            label,
            icon,
            open,
        } = self;

        base.hover(|d| d.bg(palette::bg_control_hover()))
            .when(open, |d| d.bg(palette::bg_control_active()))
            .child(
                svg()
                    .path(icon)
                    .size(px(14.))
                    .flex_none()
                    .text_color(palette::text()),
            )
            .when(!label.is_empty(), |d| d.child(label))
            .child(
                svg()
                    .path(icons::CHEVRON_DOWN)
                    .size(px(10.))
                    .flex_none()
                    .text_color(if open {
                        palette::accent()
                    } else {
                        palette::text_muted()
                    }),
            )
    }
}

pub fn icon_button(
    icon: &'static str,
    inert: bool,
    on_click: impl Fn(&Press, &mut Window, &mut App) + 'static,
) -> IconButton {
    IconButton {
        id: ElementId::Name(icon.into()),
        base: div()
            .flex_none()
            .p(tokens::SPACE_XS)
            .rounded(tokens::RADIUS)
            .when(inert, |d| d.opacity(0.5)),
        icon,
        inert,
        filled: false,
        on_click: Rc::new(on_click),
    }
}

#[derive(IntoElement)]
pub struct IconButton {
    id: ElementId,
    base: Div,
    icon: &'static str,
    inert: bool,
    filled: bool,
    on_click: OnPress,
}

impl IconButton {
    pub fn keyed(mut self, id: impl Into<ElementId>) -> Self {
        self.id = id.into();
        self
    }

    /// For one beside filled controls, where a bare glyph reads as decoration.
    pub fn filled(mut self) -> Self {
        self.filled = true;
        self.base = self.base.bg(palette::bg_control());
        self
    }
}

impl Styled for IconButton {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl InteractiveElement for IconButton {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl RenderOnce for IconButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = control_focus(self.id.clone(), window, cx);
        let focused = focus.is_focused(window);
        let inert = self.inert;
        let hover = if self.filled {
            palette::bg_control_hover()
        } else {
            palette::bg_control()
        };
        self.base
            .key_context(CONTROL_CONTEXT)
            .map(|d| {
                if inert {
                    d
                } else {
                    pressable(
                        d.track_focus(&focus.tab_index(0).tab_stop(true))
                            .hover(move |d| d.bg(hover))
                            .cursor_pointer(),
                        self.on_click,
                    )
                }
            })
            .child(
                svg()
                    .path(self.icon)
                    .size(px(14.))
                    .flex_none()
                    .text_color(palette::text()),
            )
            .children(focus_ring(focused, tokens::RADIUS, cx))
    }
}

/// How far past a strip's top a typed value may go, as a multiple of the span.
pub const OVER: f32 = 4.0;

/// A press must be worth a whole rounding step anywhere on the strip or the
/// key reads dead. The tightest case, 30 s on the 30 s to 12 h buffer, needs a
/// ratio of 7 to 6: a fortieth of the strip.
const LOG_STEP: f32 = 0.025;

/// `unit` includes its leading space where it wants one: `" px"`, `"%"`.
#[derive(Clone, Copy)]
pub struct Span {
    min: f32,
    max: f32,
    unit: &'static str,
    decimals: usize,
    over: f32,
    duration: bool,
    log: bool,
}

/// Where a saved knob is clamped on load. Folding to the strip's own top
/// would drop every typed value on restart.
pub fn ceiling(min: f32, max: f32) -> f32 {
    min + (max - min) * OVER
}

pub fn span(min: f32, max: f32, unit: &'static str) -> Span {
    Span {
        min,
        max,
        unit,
        decimals: 0,
        over: OVER,
        duration: false,
        log: false,
    }
}

/// Seconds read as `45 s`, `10 min`, `1 h 30 min`. Values round to a step
/// that grows with size, which also makes readout and input exact inverses.
pub fn span_secs(min: f32, max: f32) -> Span {
    Span {
        min,
        max,
        unit: " s",
        decimals: 0,
        over: OVER,
        duration: true,
        log: false,
    }
}

/// Two units at most, the way a duration is read off a slider. The unit
/// symbols are the same in every locale rox ships.
pub fn fmt_duration_secs(secs: f32) -> String {
    let total = secs.max(0.0).round() as u64;
    let (hours, minutes, seconds) = (total / 3600, total / 60 % 60, total % 60);

    if hours > 0 {
        return match minutes {
            0 => format!("{hours} h"),
            _ => format!("{hours} h {minutes} min"),
        };
    }

    if minutes > 0 {
        return match seconds {
            0 => format!("{minutes} min"),
            _ => format!("{minutes} min {seconds} s"),
        };
    }

    format!("{seconds} s")
}

/// Number-and-unit pairs summed; a bare number is seconds. Anything else is
/// refused, so a typo leaves the setting alone.
pub fn parse_duration_secs(text: &str) -> Option<f32> {
    let text = text.trim().replace(',', ".").to_ascii_lowercase();
    let mut rest = text.as_str();
    let mut total = 0.0;
    let mut read = false;

    while !rest.trim_start().is_empty() {
        rest = rest.trim_start();

        let digits = rest
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .unwrap_or(rest.len());
        let value = rest[..digits].parse::<f32>().ok()?;
        rest = rest[digits..].trim_start();
        let letters = rest
            .find(|c: char| !c.is_ascii_alphabetic())
            .unwrap_or(rest.len());
        let scale = match &rest[..letters] {
            "" | "s" | "sec" | "secs" => 1.0,
            "m" | "min" | "mins" => 60.0,
            "h" | "hr" | "hrs" => 3600.0,
            _ => return None,
        };
        rest = &rest[letters..];

        total += value * scale;
        read = true;
    }

    read.then_some(total)
}

/// The ladder climbs gently: a jump from five seconds straight to a minute
/// would leave an arrow key unable to move the value above the boundary.
fn duration_step(secs: f32) -> f32 {
    match secs {
        s if s < 120.0 => 5.0,
        s if s < 900.0 => 15.0,
        s if s < 3600.0 => 60.0,
        s if s < 10800.0 => 300.0,
        _ => 900.0,
    }
}

impl Span {
    pub fn decimals(mut self, n: usize) -> Self {
        self.decimals = n;
        self
    }

    /// Typed values clamp to the strip, for knobs whose top means something.
    pub fn hard(mut self) -> Self {
        self.over = 1.0;
        self
    }

    /// Lay the strip out by ratio, for spans whose ends are orders apart. Only
    /// the mapping changes; values stay in real units.
    ///
    /// Also makes the span hard: [`OVER`] multiplies the fraction, which on a
    /// log strip multiplies the ratio and would allow absurd values. `min`
    /// must be above zero.
    pub fn log(mut self) -> Self {
        self.log = true;
        self.over = 1.0;
        self
    }

    fn fraction(&self, value: f32) -> f32 {
        self.unclamped(value).clamp(0.0, 1.0)
    }

    /// Past the top included; the input's headroom is applied downstream.
    fn unclamped(&self, value: f32) -> f32 {
        if self.log {
            // Floored above zero so a typed nothing lands off the bottom
            // rather than at infinity.
            let ratio = (value / self.min).max(f32::MIN_POSITIVE);

            return ratio.ln() / (self.max / self.min).ln();
        }

        (value - self.min) / (self.max - self.min)
    }

    /// The smallest step the readout can show, so no press reads dead, but no
    /// finer than a hundredth of the span.
    fn step(&self) -> f32 {
        if self.log {
            return LOG_STEP;
        }

        let smallest = 10f32.powi(-(self.decimals as i32)) / (self.max - self.min);
        smallest.max(0.01)
    }

    /// Rounded to what the readout shows, so the applied value matches.
    fn value(&self, fraction: f32) -> f32 {
        let raw = match self.log {
            true => self.min * (self.max / self.min).powf(fraction),
            false => self.min + fraction * (self.max - self.min),
        };

        if self.duration {
            let step = duration_step(raw);

            return (raw / step).round() * step;
        }

        let step = 10f32.powi(self.decimals as i32);
        (raw * step).round() / step
    }

    fn readout(&self, value: f32) -> String {
        if self.duration {
            return fmt_duration_secs(value);
        }

        // The unit carries its own leading space, so this concatenates
        // rather than going through format_unit.
        format!(
            "{}{}",
            rox_i18n::format::format_float(value as f64, self.decimals as u8),
            self.unit
        )
    }

    /// A duration seeds with its readout; anything else with a bare ASCII
    /// number, since the localized readout wouldn't parse back.
    fn edit_text(&self, value: f32) -> String {
        if self.duration {
            return fmt_duration_secs(value);
        }

        format!("{:.*}", self.decimals, value)
    }

    fn parse(&self) -> panel::ParseTyped {
        match self.duration {
            true => parse_duration_secs,
            false => panel::parse_number,
        }
    }
}

/// `value` and what `apply` receives are in the setting's own unit.
pub fn scalar<P: 'static>(
    scrub: &ScrubState,
    edit: &panel::ValueEdit,
    value: f32,
    span: Span,
    apply: impl Fn(&mut P, f32, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    scalar_sized(
        scrub,
        edit,
        value,
        span,
        panel::SliderWidth::Fixed,
        apply,
        cx,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn scalar_sized<P: 'static>(
    scrub: &ScrubState,
    edit: &panel::ValueEdit,
    value: f32,
    span: Span,
    width: panel::SliderWidth,
    apply: impl Fn(&mut P, f32, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    panel::value_slider_edit_typed(
        scrub,
        edit,
        span.fraction(value),
        span.readout(value),
        span.edit_text(value),
        span.over,
        width,
        span.step(),
        span.parse(),
        move |typed| span.unclamped(typed),
        move |this, fraction, cx| apply(this, span.value(fraction), cx),
        cx,
    )
}

/// The two sets never draw at once, so each strip owns its own bounds.
#[derive(Default)]
pub struct SidesScrub {
    linked: ScrubState,
    sides: [ScrubState; 4],
}

const SIDE_ICONS: [&str; 4] = [
    icons::PANEL_TOP,
    icons::PANEL_RIGHT,
    icons::PANEL_BOTTOM,
    icons::PANEL_LEFT,
];

/// `apply` takes the side that moved, or None for the linked strip. `split`
/// is the caller's state, not read off the value: matching sides can still be
/// open split.
#[allow(clippy::too_many_arguments)]
pub fn sides_control<P: 'static>(
    scrub: &SidesScrub,
    edit: &panel::ValueEdit,
    value: Sides,
    split: bool,
    span: Span,
    on_split: impl Fn(&mut P, bool, &mut Context<P>) + Clone + 'static,
    apply: impl Fn(&mut P, Option<Side>, f32, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    const LINK: &[(&str, ())] = &[(icons::LINK, ())];
    let link = panel::icon_toggles(
        LINK,
        move |_| !split,
        move |this: &mut P, _, cx| on_split(this, !split, cx),
        cx,
    );
    let control = if split {
        let mut column = div().flex().flex_col().gap(tokens::SPACE_XS);
        for (i, side) in Side::ALL.into_iter().enumerate() {
            let apply = apply.clone();
            column = column.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_XS)
                    .child(
                        svg()
                            .path(SIDE_ICONS[i])
                            .size(px(14.))
                            .flex_none()
                            .text_color(palette::text_muted()),
                    )
                    .child(scalar(
                        &scrub.sides[i],
                        edit,
                        value.get(side),
                        span,
                        move |this: &mut P, v, cx| apply(this, Some(side), v, cx),
                        cx,
                    )),
            );
        }
        column
    } else {
        scalar(
            &scrub.linked,
            edit,
            value.top,
            span,
            move |this: &mut P, v, cx| apply(this, None, v, cx),
            cx,
        )
    };
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_XS)
        .child(link)
        .child(control)
}

pub fn slider_edit<P: 'static>(
    scrub: &ScrubState,
    edit: &panel::ValueEdit,
    value: f32,
    apply: impl Fn(&mut P, f32, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Div {
    panel::value_slider_edit(
        scrub,
        edit,
        value,
        format!("{}%", (value * 100.0).round() as u32),
        format!("{}", (value * 100.0).round() as u32),
        |v| v / 100.0,
        apply,
        cx,
    )
}

pub fn color_cell(
    control: AnyElement,
    label: impl Into<SharedString>,
    marked: bool,
    trailing: Option<AnyElement>,
) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_XS)
        .child(control)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_xs()
                .text_color(if marked {
                    palette::text()
                } else {
                    palette::text_muted()
                })
                .child(label.into()),
        )
        .when_some(trailing, |d, trailing| d.child(trailing))
}

pub fn role_grid(columns: usize, mut cell: impl FnMut(usize) -> AnyElement) -> Div {
    let mut body = div().flex().flex_col().gap(tokens::SPACE_XS);
    let mut i = 0;
    while i < ROLES.len() {
        let group = ROLES[i].group;
        let end = ROLES[i..]
            .iter()
            .position(|role| role.group != group)
            .map(|n| i + n)
            .unwrap_or(ROLES.len());
        body = body.child(header(group));
        for row_start in (i..end).step_by(columns) {
            let mut row = div().flex().flex_row().gap(tokens::SPACE_SM);
            for j in row_start..row_start + columns {
                row = row.child(if j < end {
                    cell(j)
                } else {
                    div().flex_1().into_any_element()
                });
            }
            body = body.child(row);
        }
        i = end;
    }
    body
}

#[cfg(test)]
mod tests {
    use super::{OVER, Query, ceiling, fmt_duration_secs, parse_duration_secs, span, span_secs};

    #[test]
    fn a_duration_reads_back_as_the_seconds_it_was_written_from() {
        for secs in [30.0, 45.0, 120.0, 600.0, 3600.0, 5400.0, 10800.0, 43200.0] {
            let written = fmt_duration_secs(secs);
            assert_eq!(parse_duration_secs(&written), Some(secs), "{written}");
        }
        assert_eq!(fmt_duration_secs(30.0), "30 s");
        assert_eq!(fmt_duration_secs(120.0), "2 min");
        assert_eq!(fmt_duration_secs(600.0), "10 min");
        assert_eq!(fmt_duration_secs(3600.0), "1 h");
        assert_eq!(fmt_duration_secs(5400.0), "1 h 30 min");
        assert_eq!(fmt_duration_secs(43200.0), "12 h");
        assert_eq!(fmt_duration_secs(95.0), "1 min 35 s");
    }

    #[test]
    fn the_duration_input_takes_what_a_person_would_type() {
        assert_eq!(parse_duration_secs("600"), Some(600.0));
        assert_eq!(parse_duration_secs("600s"), Some(600.0));
        assert_eq!(parse_duration_secs(" 10min "), Some(600.0));
        assert_eq!(parse_duration_secs("1,5 min"), Some(90.0));
        assert_eq!(parse_duration_secs("1h30min"), Some(5400.0));
        assert_eq!(parse_duration_secs("2 HRS"), Some(7200.0));
        assert_eq!(parse_duration_secs(""), None);
        assert_eq!(parse_duration_secs("later"), None);
        assert_eq!(parse_duration_secs("10 fortnights"), None);
    }

    #[test]
    fn a_log_strip_puts_the_middle_at_the_geometric_mean() {
        let span = span_secs(30.0, 43200.0).log();
        assert_eq!(span.value(0.0), 30.0);
        assert_eq!(span.value(1.0), 43200.0);

        // sqrt(30 * 43200) is 1138 s, rounded to the nearest minute.
        assert_eq!(span.value(0.5), 1140.0);
        assert!((span.fraction(1140.0) - 0.5).abs() < 0.01);

        for secs in [30.0, 300.0, 1800.0, 7200.0, 43200.0] {
            assert_eq!(span.value(span.fraction(secs)), secs);
        }
    }

    #[test]
    fn an_arrow_press_moves_a_log_strip_everywhere_on_it() {
        let span = span_secs(30.0, 43200.0).log();
        let presses = (1.0 / span.step()).round() as u32;
        for press in 0..presses {
            let here = span.value(press as f32 * span.step());
            let next = span.value(((press + 1) as f32 * span.step()).min(1.0));
            assert!(next > here, "stuck at {here} s");
        }
    }

    #[test]
    fn a_query_needs_every_term_in_some_text() {
        let q = Query::parse("  Cross Fade ");
        assert!(q.active());
        assert!(q.hits(&["Crossfade", "fade between tracks"]));
        assert!(q.hits(&["fade", "cross"]));
        assert!(!q.hits(&["Cross only"]));
        assert!(!q.hits(&[]));
    }

    #[test]
    fn the_empty_query_matches_everything() {
        let q = Query::parse("   ");
        assert!(!q.active());
        assert!(q.hits(&["whatever"]));
        assert!(q.hits(&[]));
    }

    #[test]
    fn typed_values_round_trip_through_the_strip() {
        let px = span(0., 24., " px");
        for typed in [0., 1., 12., 24., 60., 96.] {
            assert_eq!(px.value(px.unclamped(typed)), typed);
        }

        let offset = span(18., 72., " px");
        for typed in [18., 30., 72., 200.] {
            assert_eq!(offset.value(offset.unclamped(typed)), typed);
        }

        let tenths = span(0.5, 4., " s").decimals(1);
        for typed in [0.5, 1.4, 4.0, 9.3] {
            assert_eq!(tenths.value(tenths.unclamped(typed)), typed);
        }
    }

    #[test]
    fn the_strip_pins_at_its_ends() {
        let px = span(0., 24., " px");
        assert_eq!(px.fraction(96.), 1.0);
        assert_eq!(px.fraction(24.), 1.0);
        assert_eq!(px.fraction(-8.), 0.0);
    }

    #[test]
    fn the_ceiling_is_the_input_headroom() {
        let px = span(18., 72., " px");
        assert_eq!(ceiling(18., 72.), px.value(OVER));
        assert_eq!(ceiling(0., 24.), 96.);
    }

    #[test]
    fn hard_spans_stop_at_the_top() {
        let percent = span(0., 100., "%").hard();
        assert_eq!(percent.over, 1.0);
        assert_eq!(percent.value(percent.over), 100.);
    }
}

#[cfg(test)]
mod search_tests {
    use super::Query;

    #[test]
    fn accents_do_not_have_to_be_typed() {
        assert!(Query::parse("prereglages").hits(&["Préréglages"]));
        assert!(Query::parse("uberblenden").hits(&["Überblenden"]));
        assert!(Query::parse("GROSSE").hits(&["Größe"]));
    }

    #[test]
    fn every_term_has_to_land_somewhere() {
        assert!(Query::parse("row height").hits(&["Row Height"]));
        assert!(Query::parse("row gap").hits(&["Row Height", "gap spacing"]));
        assert!(!Query::parse("row missing").hits(&["Row Height", "gap spacing"]));
    }

    #[test]
    fn an_empty_query_is_not_a_filter() {
        assert!(!Query::parse("   ").active());
    }
}
