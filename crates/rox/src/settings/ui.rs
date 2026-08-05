//! The chrome the settings windows share: the app settings window and
//! every panel's settings window draw their shell from one set, so a
//! page reads the same wherever it opens - the sidebar with its nav
//! rows, titled sections, group headers, the small header buttons, the
//! scalar slider, and the palette editor's role grid. Page content stays
//! with each window; only the shell lives here.

use gpui::{
    div, prelude::*, px, svg, AnyElement, App, Context, Div, MouseButton, MouseDownEvent, Pixels,
    SharedString, Window,
};

use crate::assets::icons;
use crate::design::palette::{self, ROLES};
use crate::design::tokens;
use crate::panel::{self, ScrubState};

/// A checklist tick box: a square that fills with the accent and shows a
/// check while on, an empty control-colored box while off. The caller wires
/// the click on the surrounding row.
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

/// The sidebar's width, room for a page name and no more.
pub const SIDEBAR_W: Pixels = px(160.);

/// The narrowest a color cell renders whole: the swatch, its gap, and
/// the longest role label.
pub const COLOR_CELL_MIN_W: Pixels = px(150.);

/// The gap between a page's sections, a step over the row rhythm so a
/// boundary reads as one.
pub const SECTION_GAP: Pixels = px(20.);

/// The floor under a settings window: the sidebar plus a colors row that
/// still fits its labels, and enough height for a page to breathe.
pub const MIN_SIZE: gpui::Size<Pixels> = gpui::Size {
    width: px(560.),
    height: px(400.),
};

/// How many color-grid columns fit the page beside the sidebar: as many
/// whole cells as the window minus the sidebar and the body's insets
/// allows, two at the window floor up to four.
pub fn grid_columns(window: &Window) -> usize {
    let page_w = window.viewport_size().width - SIDEBAR_W - tokens::SPACE_MD * 2.;
    usize::clamp((page_w / COLOR_CELL_MIN_W) as usize, 2, 4)
}

/// The sidebar shell: the nav rows go in at the top; a window with
/// footer actions sinks them after its own spacer.
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

/// A sidebar row: the page's icon leading its name; the picked page
/// reads like an active control.
pub fn nav_item<P: 'static>(
    label: &'static str,
    icon: &'static str,
    picked: bool,
    on_pick: impl Fn(&mut P, &mut Window, &mut Context<P>) + 'static,
    cx: &mut Context<P>,
) -> Div {
    div()
        .px(tokens::SPACE_MD)
        .py(tokens::SPACE_XS)
        .rounded(tokens::RADIUS)
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_SM)
        .cursor_pointer()
        .when(picked, |d| d.bg(palette::bg_control_active()))
        .when(!picked, |d| d.hover(|d| d.bg(palette::bg_menu_hover())))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, window, cx| on_pick(this, window, cx)),
        )
        .child(
            svg()
                .path(icon)
                .size(px(14.))
                .flex_none()
                .text_color(palette::text()),
        )
        .child(label)
}

/// A header between setting groups, the palette listing's block names.
pub fn header(label: &'static str) -> Div {
    div()
        .pt(tokens::SPACE_SM)
        .text_xs()
        .text_color(palette::text_muted())
        .child(label)
}

/// A titled section of a page: the name over a hairline, an optional
/// control riding the header's right edge, the rows under it.
pub fn section(label: &'static str, trailing: Option<AnyElement>, body: impl IntoElement) -> Div {
    section_with_icon(None, label, trailing, body)
}

/// [`section`] led by a header icon, the sidebar rows' grammar. The
/// settings window's sealed path always passes one; the icon-less
/// callers across the app stay on [`section`].
pub fn section_with_icon(
    icon: Option<&'static str>,
    label: &'static str,
    trailing: Option<AnyElement>,
    body: impl IntoElement,
) -> Div {
    div()
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
                        .child(label),
                )
                .when_some(trailing, |d, trailing| d.child(trailing)),
        )
        .child(body)
}

/// The settings search query: the box's text lowercased and split on
/// whitespace. A row matches when every term lands somewhere in its
/// label, description, or keywords; the empty query matches everything,
/// which is the closed-search path.
pub struct Query {
    terms: Vec<String>,
}

impl Query {
    pub fn parse(text: &str) -> Self {
        Self {
            terms: text
                .split_whitespace()
                .map(|term| term.to_lowercase())
                .collect(),
        }
    }

    /// Whether there's anything to filter by.
    pub fn active(&self) -> bool {
        !self.terms.is_empty()
    }

    /// Whether every term appears in some of `texts`, case folded.
    fn hits(&self, texts: &[&str]) -> bool {
        self.terms.iter().all(|term| {
            texts
                .iter()
                .any(|text| text.to_lowercase().contains(term.as_str()))
        })
    }
}

/// A page under search: the only shape the settings window's render
/// takes back from a page builder, and it only takes [`Section`]s, whose
/// rows all declare the words that find them. The chain is the point: a
/// setting can't land on a page without stating its search terms, so
/// search never silently misses a new row.
///
/// Search builds every page each keystroke, so page builders must stay
/// pure reads: no spawns, no entity updates outside listeners.
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

    /// Add a section; one the query emptied adds nothing.
    pub fn section(mut self, section: Section) -> Self {
        if let Some(body) = section.body {
            self.body = self.body.child(body);
            self.hits += section.hits;
        }
        self
    }

    /// Chain conditionally, gpui's own `when` shape.
    pub fn when(self, condition: bool, then: impl FnOnce(Self) -> Self) -> Self {
        if condition {
            then(self)
        } else {
            self
        }
    }

    /// How many rows the page kept; zero drops it from the results stack
    /// and dims its sidebar entry.
    pub fn hits(&self) -> usize {
        self.hits
    }

    pub fn element(self) -> AnyElement {
        self.body.into_any_element()
    }
}

/// One titled section of a page, already filtered: the body is `None`
/// when the query dropped every row.
pub struct Section {
    body: Option<Div>,
    hits: usize,
}

impl Section {
    /// Build a section against the query. A query hitting the section's
    /// own label keeps the whole section; otherwise rows survive one by
    /// one and an emptied section drops. The icon leads the header the
    /// way the sidebar's do, and it's required here so no section on the
    /// sealed path ships bare.
    pub fn new(
        q: &Query,
        icon: &'static str,
        label: &'static str,
        trailing: Option<AnyElement>,
        build: impl FnOnce(Rows) -> Rows,
    ) -> Self {
        let all = !q.active() || q.hits(&[label]);
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

/// A section's rows, each declaring what finds it. `all` short-circuits
/// the checks while no search is on or the section's own name matched.
pub struct Rows<'a> {
    q: &'a Query,
    all: bool,
    body: Div,
    hits: usize,
}

impl Rows<'_> {
    /// A standard labeled row; the label and description are the terms.
    pub fn row(
        self,
        label: &'static str,
        description: Option<&'static str>,
        control: impl IntoElement,
    ) -> Self {
        self.keyed(&[], label, description, control)
    }

    /// [`Rows::row`] with extra terms the copy doesn't carry: "gapless"
    /// on the crossfade row, "normalization" on the gain mode.
    pub fn keyed(
        mut self,
        keywords: &[&str],
        label: &'static str,
        description: Option<&'static str>,
        control: impl IntoElement,
    ) -> Self {
        if self.keep(keywords, label, description) {
            self.body = self
                .body
                .child(crate::panel::setting_row(label, description, control));
            self.hits += 1;
        }
        self
    }

    /// A row whose description carries live numbers: the query matches
    /// the label and keywords only, never text that moves under it.
    pub fn row_dyn(
        mut self,
        keywords: &[&str],
        label: &'static str,
        description: Option<SharedString>,
        control: impl IntoElement,
    ) -> Self {
        if self.keep(keywords, label, None) {
            self.body = self
                .body
                .child(crate::panel::setting_row_dyn(label, description, control));
            self.hits += 1;
        }
        self
    }

    /// Anything that isn't a plain row - a table, a grid, a block with
    /// its own chrome - declaring its terms outright. The closure only
    /// runs when the content survives, so a heavy section costs nothing
    /// while filtered out.
    pub fn custom(mut self, keywords: &[&str], build: impl FnOnce() -> AnyElement) -> Self {
        if self.all || self.q.hits(keywords) {
            self.body = self.body.child(build());
            self.hits += 1;
        }
        self
    }

    /// Chain conditionally, gpui's own `when` shape.
    pub fn when(self, condition: bool, then: impl FnOnce(Self) -> Self) -> Self {
        if condition {
            then(self)
        } else {
            self
        }
    }

    /// [`Rows::when`] over an option, for the row that only exists while
    /// there's something to put in it.
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

/// One block's header inside a section's list: the label with whatever
/// acts on the whole block riding its right edge, ruled off from the rows
/// beneath the way [`section`] rules its own. The rule is lighter than a
/// section's, so the two levels read apart rather than alike.
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

/// A block nested under the row that owns it: an accent rail down the
/// left edge with the content inset from it, so the block reads as
/// belonging to the row above instead of carrying on the list. What a
/// route's editor sits in, under the knob it drives.
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

/// The settings windows' text button, at the section header's scale
/// where every one of them rides: an icon leading its label; inert ones
/// dim and drop the click.
pub fn small_button(
    label: impl Into<SharedString>,
    icon: &'static str,
    inert: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
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
        .map(|d| {
            if inert {
                d.opacity(0.5)
            } else {
                d.hover(|d| d.bg(palette::bg_control_hover()))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, on_click)
            }
        })
        .child(
            svg()
                .path(icon)
                .size(px(12.))
                .flex_none()
                .text_color(palette::text()),
        )
        .child(label.into())
}

/// A confirm-dialog button: the primary one reads as a filled accent
/// control, the rest as plain controls. Shared with the pass prompt, which
/// is a dialog the settings window no longer owns alone.
pub fn dialog_button(
    label: &'static str,
    primary: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .flex_none()
        .px(tokens::SPACE_MD)
        .py(tokens::SPACE_XS)
        .rounded(tokens::RADIUS)
        .cursor_pointer()
        .map(|d| {
            if primary {
                d.bg(palette::accent())
                    .text_color(palette::text_on_accent())
                    .hover(|d| d.opacity(0.9))
            } else {
                d.bg(palette::bg_control())
                    .hover(|d| d.bg(palette::bg_control_hover()))
            }
        })
        .on_mouse_down(MouseButton::Left, on_click)
        .child(label)
}

/// A flat icon-only button for table rows: the glyph alone at rest, a
/// soft pill behind it on hover, dimmed and inert like the text buttons.
pub fn icon_button(
    icon: &'static str,
    inert: bool,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .flex_none()
        .p(tokens::SPACE_XS)
        .rounded(tokens::RADIUS)
        .map(|d| {
            if inert {
                d.opacity(0.5)
            } else {
                d.hover(|d| d.bg(palette::bg_control()))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, on_click)
            }
        })
        .child(
            svg()
                .path(icon)
                .size(px(14.))
                .flex_none()
                .text_color(palette::text()),
        )
}

/// How far past a strip's top a typed value may reach, as a multiple of
/// the span: the strip covers the sensible everyday range, the input
/// covers conviction.
pub const OVER: f32 = 4.0;

/// A scalar knob's span and how its number reads: the range the strip
/// scrubs across, the suffix trailing the value (its leading space
/// included, so `" px"` stands off the number and `"%"` glues to it), the
/// decimals the readout and the landed value keep, and how far a typed
/// value may run past the top.
#[derive(Clone, Copy)]
pub struct Span {
    min: f32,
    max: f32,
    unit: &'static str,
    decimals: usize,
    over: f32,
}

/// The highest a typed value may reach over a strip running `min` to
/// `max`. What a saved knob has to be read back inside: folded to the
/// strip's own top on load, every typed value would drop the moment the
/// app restarts.
pub fn ceiling(min: f32, max: f32) -> f32 {
    min + (max - min) * OVER
}

/// A span from `min` to `max` reading in whole `unit`s, with the typed
/// headroom a soft ceiling gets. [`Span::decimals`] and [`Span::hard`]
/// refine it.
pub fn span(min: f32, max: f32, unit: &'static str) -> Span {
    Span {
        min,
        max,
        unit,
        decimals: 0,
        over: OVER,
    }
}

impl Span {
    /// Keep `n` decimals, in the readout and in the value that lands.
    pub fn decimals(mut self, n: usize) -> Self {
        self.decimals = n;
        self
    }

    /// The strip's range is the law rather than a reach: a typed value
    /// clamps to it. For the knobs whose top means something, a full
    /// percent or a circle, instead of a comfortable ceiling.
    pub fn hard(mut self) -> Self {
        self.over = 1.0;
        self
    }

    /// Where `value` rides the strip. Values past the top pin it full;
    /// the readout still reads the real number.
    fn fraction(&self, value: f32) -> f32 {
        self.unclamped(value).clamp(0.0, 1.0)
    }

    /// The typed value's place on the strip, past the top included: the
    /// input's own headroom is applied downstream, against `over`.
    fn unclamped(&self, value: f32) -> f32 {
        (value - self.min) / (self.max - self.min)
    }

    /// The value a strip fraction stands for, rounded to the decimals the
    /// readout shows so what lands is what reads.
    fn value(&self, fraction: f32) -> f32 {
        let step = 10f32.powi(self.decimals as i32);
        ((self.min + fraction * (self.max - self.min)) * step).round() / step
    }
}

/// A scalar setting's control: the strip scrubbing its span with the
/// readout beside it doubling as an input. Click the number, type, Enter.
/// `value` and what `apply` receives are both in the setting's own unit,
/// so no caller maps a fraction by hand.
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

/// [`scalar`] with the strip's width said out loud. Pages take the fixed
/// control column; a dialog builds its own row and asks the strip to fill it.
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
    panel::value_slider_edit_sized(
        scrub,
        edit,
        span.fraction(value),
        format!("{:.*}{}", span.decimals, value, span.unit),
        format!("{:.*}", span.decimals, value),
        span.over,
        width,
        move |typed| span.unclamped(typed),
        move |this, fraction, cx| apply(this, span.value(fraction), cx),
        cx,
    )
}

/// A percent slider whose readout doubles as an input: click, type,
/// Enter. Percent knobs stay bounded at 100; the strip's range is the law
/// here.
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

/// One cell of a color grid: the swatch control with its role label
/// beside it. `marked` brightens the label, how the panel editor points
/// out the roles it overrides. `trailing` rides the cell's right edge,
/// where the panel editor hangs a role's reset button.
pub fn color_cell(
    control: AnyElement,
    label: &'static str,
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
                .child(label),
        )
        .when_some(trailing, |d, trailing| d.child(trailing))
}

/// The color grid's frame: each listing group under its header,
/// `columns` cells to a row, the last row padded so cells keep their
/// width. The cell for a role index is the caller's.
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
    use super::{ceiling, span, Query, OVER};

    /// Every term must land somewhere, any field counts, case folded.
    #[test]
    fn a_query_needs_every_term_in_some_text() {
        let q = Query::parse("  Cross Fade ");
        assert!(q.active());
        assert!(q.hits(&["Crossfade", "fade between tracks"]));
        assert!(q.hits(&["fade", "cross"]));
        assert!(!q.hits(&["Cross only"]));
        assert!(!q.hits(&[]));
    }

    /// The empty query is search-off: inactive, and it matches anything.
    #[test]
    fn the_empty_query_matches_everything() {
        let q = Query::parse("   ");
        assert!(!q.active());
        assert!(q.hits(&["whatever"]));
        assert!(q.hits(&[]));
    }

    /// A number typed into a readout comes back as itself: the strip
    /// fraction it maps to lands on the same value, inside the range and
    /// out in the input's headroom.
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

    /// The strip pins full past its top while the readout keeps the real
    /// number, and a value under the floor pins empty.
    #[test]
    fn the_strip_pins_at_its_ends() {
        let px = span(0., 24., " px");
        assert_eq!(px.fraction(96.), 1.0);
        assert_eq!(px.fraction(24.), 1.0);
        assert_eq!(px.fraction(-8.), 0.0);
    }

    /// The read-back ceiling matches the headroom the input actually
    /// reaches, so nothing typed is folded away on the next load.
    #[test]
    fn the_ceiling_is_the_input_headroom() {
        let px = span(18., 72., " px");
        assert_eq!(ceiling(18., 72.), px.value(OVER));
        assert_eq!(ceiling(0., 24.), 96.);
    }

    /// A hard span holds the input to the strip's own top.
    #[test]
    fn hard_spans_stop_at_the_top() {
        let percent = span(0., 100., "%").hard();
        assert_eq!(percent.over, 1.0);
        assert_eq!(percent.value(percent.over), 100.);
    }
}
