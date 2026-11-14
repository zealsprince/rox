//! The arrangement editor the composable strips share: the shown items as
//! chips in display order over a tray of the hidden ones. Drags and the
//! chips' plus and x move items between them. The config is one ordered
//! list per row, and an item off the list is hidden. Most items show at
//! most once per row; one the catalog marks repeatable keeps its tray chip
//! while shown.
//!
//! A catalog is either a static slice of [`ArrangeSpec`] or, for a panel
//! whose items are user-made (the custom controls strip), a `Vec` of
//! [`ArrangeEntry`] with labels already resolved. Both convert into
//! [`Arrangement`], which everything below works over.
//!
//! An item may declare variants, each its own value of the item enum. A
//! placed chip picks between them from a menu, and every rule here treats
//! the variants of one item as that item. A saved list writes an item on
//! its default as the bare name and any other variant as
//! `{ "item": ..., "variant": ... }`, through [`SavedItem`].

use std::rc::Rc;

use gpui::{
    App, Context, DismissEvent, Div, ElementId, Entity, Focusable, MouseButton, MouseDownEvent,
    Pixels, Point, SharedString, Stateful, Subscription, Window, anchored, deferred, div,
    prelude::*, px, svg,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use serde::de::{self, DeserializeOwned};
use serde::{Deserialize, Deserializer, Serialize, Serializer, ser};
use serde_json::Value;

use rox_design::assets::icons;
use rox_design::{palette, tokens};

/// One item of a strip's catalog. Catalogs are declared in stock order,
/// which is where a re-shown item slots back in.
pub struct ArrangeSpec<V: 'static> {
    /// The message key for the chip label, and the chip's element id: a key is
    /// stable across locales, so a drag survives a language change.
    pub key: &'static str,
    pub icon: Option<&'static str>,
    /// The item on its default variant, which the tray places.
    pub value: V,
    /// Whether one row may hold more than one (spacers, dividers). A repeatable
    /// item keeps its tray chip while shown.
    pub repeats: bool,
    /// Every way the item can draw, the default among them. Empty for an item
    /// with one look, which gets no picker.
    pub variants: &'static [ArrangeVariant<V>],
}

/// One choice in a placed chip's variant menu.
pub struct ArrangeVariant<V: 'static> {
    /// The message key for the name the chip and its menu show.
    pub key: &'static str,
    pub value: V,
}

/// An item built at runtime, with its text resolved instead of a message key.
#[derive(Clone)]
pub struct ArrangeEntry<V> {
    /// Stable across locales and registry rebuilds, or a drag loses its state
    /// mid-gesture. Derive it from the item's identity, never its label.
    pub id: SharedString,
    pub label: SharedString,
    pub icon: Option<&'static str>,
    pub value: V,
    pub repeats: bool,
    /// Named like [`ArrangeSpec::variants`], with the labels resolved.
    pub variants: Vec<(SharedString, V)>,
}

impl<V: PartialEq + Copy> ArrangeSpec<V> {
    fn owns(&self, value: V) -> bool {
        self.value == value || self.variants.iter().any(|variant| variant.value == value)
    }
}

impl<V: PartialEq + Copy> ArrangeEntry<V> {
    fn owns(&self, value: V) -> bool {
        self.value == value || self.variants.iter().any(|(_, variant)| *variant == value)
    }
}

/// The catalog in either form. The built side is refcounted because the
/// drag and drop handlers outlive the render that made them.
pub enum Arrangement<V: 'static> {
    Stock(&'static [ArrangeSpec<V>]),
    Built(Rc<[ArrangeEntry<V>]>),
}

impl<V: 'static> Clone for Arrangement<V> {
    /// By hand so the item type doesn't have to be `Clone`.
    fn clone(&self) -> Self {
        match self {
            Arrangement::Stock(specs) => Arrangement::Stock(specs),

            Arrangement::Built(entries) => Arrangement::Built(entries.clone()),
        }
    }
}

impl<V: 'static> From<&'static [ArrangeSpec<V>]> for Arrangement<V> {
    fn from(specs: &'static [ArrangeSpec<V>]) -> Self {
        Arrangement::Stock(specs)
    }
}

impl<V: 'static> From<Vec<ArrangeEntry<V>>> for Arrangement<V> {
    fn from(entries: Vec<ArrangeEntry<V>>) -> Self {
        Arrangement::Built(entries.into())
    }
}

impl<V: PartialEq + Copy + 'static> Arrangement<V> {
    /// The catalog item `value` belongs to, on whichever variant.
    fn place(&self, value: V) -> Option<usize> {
        match self {
            Arrangement::Stock(specs) => specs.iter().position(|spec| spec.owns(value)),

            Arrangement::Built(entries) => entries.iter().position(|entry| entry.owns(value)),
        }
    }

    /// Two variants of one item are the same item. A value off the catalog is
    /// only ever itself.
    fn same(&self, a: V, b: V) -> bool {
        a == b
            || self
                .place(a)
                .is_some_and(|place| self.place(b) == Some(place))
    }

    fn holds(&self, items: &[V], value: V) -> bool {
        items.iter().any(|item| self.same(*item, value))
    }

    fn repeats(&self, value: V) -> bool {
        let Some(place) = self.place(value) else {
            return false;
        };

        match self {
            Arrangement::Stock(specs) => specs[place].repeats,

            Arrangement::Built(entries) => entries[place].repeats,
        }
    }

    fn rank(&self, value: V) -> usize {
        self.place(value).unwrap_or(usize::MAX)
    }

    fn entries(&self) -> Vec<ArrangeEntry<V>> {
        match self {
            Arrangement::Stock(specs) => specs
                .iter()
                .map(|spec| ArrangeEntry {
                    id: SharedString::new_static(spec.key),
                    label: rox_i18n::t!(spec.key),
                    icon: spec.icon,
                    value: spec.value,
                    repeats: spec.repeats,
                    variants: spec
                        .variants
                        .iter()
                        .map(|variant| (rox_i18n::t!(variant.key), variant.value))
                        .collect(),
                })
                .collect(),

            Arrangement::Built(entries) => entries.to_vec(),
        }
    }
}

/// A panel's item enum, tied to its stock catalog so a saved list can tell
/// which variant a bare name means.
pub trait ArrangeItem: Serialize + DeserializeOwned + PartialEq + Copy + 'static {
    fn catalog() -> &'static [ArrangeSpec<Self>];
}

/// One entry of a saved item list. The item enum keeps serde's derived
/// shape, a unit variant as its name and a variant carrying a draw choice
/// as `{ name: choice }`; this reshapes that so a list from before an item
/// had variants still reads, and only a non-default variant writes the
/// richer form.
pub struct SavedItem<V>(pub V);

#[derive(Serialize, Deserialize)]
struct RichItem {
    item: String,
    variant: Value,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ItemWire {
    Bare(String),
    Rich(RichItem),
}

/// The name serde gives the item, whatever variant it's on.
fn derived_name<V: Serialize>(value: V) -> Option<String> {
    match serde_json::to_value(value).ok()? {
        Value::String(name) => Some(name),

        Value::Object(map) => map.into_iter().next().map(|(name, _)| name),

        _ => None,
    }
}

impl<V: ArrangeItem> Serialize for SavedItem<V> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let derived = serde_json::to_value(self.0).map_err(ser::Error::custom)?;
        let on_default = V::catalog().iter().any(|spec| spec.value == self.0);

        let tagged = match &derived {
            Value::Object(map) if map.len() == 1 => map.iter().next(),
            _ => None,
        };

        match tagged {
            Some((item, _)) if on_default => serializer.serialize_str(item),

            Some((item, variant)) => RichItem {
                item: item.clone(),
                variant: variant.clone(),
            }
            .serialize(serializer),

            None => derived.serialize(serializer),
        }
    }
}

impl<'de, V: ArrangeItem> Deserialize<'de> for SavedItem<V> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = match ItemWire::deserialize(deserializer)? {
            // serde only reads a variant carrying a choice in its tagged form, so
            // a bare name for one falls back to the catalog's default.
            ItemWire::Bare(name) => match V::deserialize(Value::String(name.clone())) {
                Ok(value) => value,

                Err(e) => V::catalog()
                    .iter()
                    .map(|spec| spec.value)
                    .find(|value| derived_name(*value).as_deref() == Some(name.as_str()))
                    .ok_or_else(|| de::Error::custom(e))?,
            },

            // An unknown variant name fails like an unknown item name does.
            ItemWire::Rich(RichItem { item, variant }) => {
                let tagged = Value::Object([(item.clone(), variant)].into_iter().collect());
                let value = V::deserialize(tagged).map_err(de::Error::custom)?;
                if !V::catalog().iter().any(|spec| spec.owns(value)) {
                    return Err(de::Error::custom(format!(
                        "{item} declares no such variant"
                    )));
                }
                value
            }
        };

        Ok(SavedItem(value))
    }
}

/// `serialize_with` for an item list, each entry written as a [`SavedItem`].
pub fn serialize_items<V: ArrangeItem, S: Serializer>(
    items: &[V],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.collect_seq(items.iter().map(|item| SavedItem(*item)))
}

/// Generic over the item enum, so a drop only reaches editors of the same
/// panel kind; `editor` covers two windows of one kind. `from` is the
/// (row, index) the drag left, None off the tray, since a repeated value
/// alone isn't an identity.
#[derive(Clone)]
struct ArrangeDrag<V: Clone + 'static> {
    editor: &'static str,
    value: V,
    from: Option<(usize, usize)>,
    label: SharedString,
    icon: Option<&'static str>,
}

struct ChipPreview {
    label: SharedString,
    icon: Option<&'static str>,
}

impl Render for ChipPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        chip(self.label.clone(), self.icon, false)
            .border_1()
            .border_color(palette::border_light())
    }
}

type MenuBuild = Rc<dyn Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu>;

/// The open variant menu: where it was asked for, the menu, and the dismiss
/// subscription that clears it.
#[derive(Default)]
struct VariantMenu(Option<(Point<Pixels>, Entity<PopupMenu>, Subscription)>);

/// A placed chip whose item has variants. A left or a right click opens the
/// variant menu at the pointer. gpui-component's `context_menu` only answers
/// the right button, so the chip holds its own menu the way `PopoutHost`
/// does, with the same occluding layer that dismisses on an outside press.
#[derive(IntoElement)]
struct VariantChip {
    /// The chip's own id, unique per place, so each chip keys its own menu
    /// state instead of every chip sharing one.
    id: ElementId,
    chip: Stateful<Div>,
    menu: MenuBuild,
}

impl RenderOnce for VariantChip {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id, cx, |_, _| VariantMenu::default());

        let open = {
            let state = state.clone();
            let build = self.menu;
            Rc::new(
                move |position: Point<Pixels>, window: &mut Window, cx: &mut App| {
                    let build = build.clone();
                    let menu = PopupMenu::build(window, cx, move |menu, window, cx| {
                        build(menu, window, cx)
                    });
                    menu.focus_handle(cx).focus(window);

                    let dismiss = state.clone();
                    let subscription =
                        window.subscribe(&menu, cx, move |_, _: &DismissEvent, _, cx| {
                            dismiss.update(cx, |open, cx| {
                                open.0 = None;
                                cx.notify();
                            });
                        });
                    state.update(cx, |open, cx| {
                        open.0 = Some((position, menu, subscription));
                        cx.notify();
                    });
                },
            )
        };

        let overlay = state.read(cx).0.as_ref().map(|(position, menu, _)| {
            let close = state.clone();
            let shut = move |_: &MouseDownEvent, _: &mut Window, cx: &mut App| {
                close.update(cx, |open, cx| {
                    open.0 = None;
                    cx.notify();
                });
            };
            deferred(
                anchored().child(
                    div()
                        .w(window.bounds().size.width)
                        .h(window.bounds().size.height)
                        .occlude()
                        .on_mouse_down(MouseButton::Left, shut.clone())
                        .on_mouse_down(MouseButton::Right, shut.clone())
                        .on_mouse_down(MouseButton::Middle, shut)
                        .child(
                            anchored()
                                .position(*position)
                                .snap_to_window_with_margin(px(8.))
                                .child(menu.clone()),
                        ),
                ),
            )
            .with_priority(1)
        });

        let left = open.clone();
        self.chip
            .on_click(move |event, window, cx| left(event.position(), window, cx))
            // Stopped so the press doesn't reach a right-click handler further up.
            .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                cx.stop_propagation();
                open(event.position, window, cx);
            })
            .children(overlay)
    }
}

fn chip(label: SharedString, icon: Option<&'static str>, dimmed: bool) -> Div {
    div()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(tokens::SPACE_XS)
        .px(tokens::SPACE_SM)
        .py(tokens::SPACE_XS)
        .rounded(tokens::RADIUS)
        .bg(palette::bg_control())
        .children(icon.map(|icon| {
            svg().path(icon).size(px(14.)).text_color(if dimmed {
                palette::text_faint()
            } else {
                palette::text_muted()
            })
        }))
        .child(
            div()
                .text_sm()
                .whitespace_nowrap()
                .text_color(if dimmed {
                    palette::text_muted()
                } else {
                    palette::text()
                })
                .child(label),
        )
}

fn chip_action(icon: &'static str) -> Div {
    div()
        .flex_none()
        .rounded(tokens::RADIUS)
        .cursor_pointer()
        .child(
            svg()
                .path(icon)
                .size(px(12.))
                .text_color(palette::text_faint())
                .hover(|s| s.text_color(palette::text())),
        )
}

/// Holds its height while empty so it stays a drop target.
fn well() -> Div {
    div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(tokens::SPACE_XS)
        .p(tokens::SPACE_XS)
        .rounded(tokens::RADIUS)
        .border_1()
        .border_color(palette::border())
        .min_h(px(36.))
}

fn caption(text: gpui::SharedString) -> Div {
    div()
        .text_xs()
        .text_color(palette::text_faint())
        .child(text)
}

/// Uniqueness is per row: a non-repeatable item already on the row leaves
/// first, whatever its variant, pulling `at` along, so a drop replaces the
/// row's copy.
fn insert_row_unique<V: PartialEq + Copy>(
    registry: &Arrangement<V>,
    row: &mut Vec<V>,
    value: V,
    at: usize,
    unique: bool,
) {
    let mut at = at.min(row.len());
    if unique {
        let mut ix = 0;
        row.retain(|v| {
            let keep = !registry.same(*v, value);
            if !keep && ix < at {
                at -= 1;
            }
            ix += 1;
            keep
        });
    }
    row.insert(at.min(row.len()), value);
}

/// `from` and `to` are places in the rows as they stand before the move.
fn moved_at<V: PartialEq + Copy>(
    registry: &Arrangement<V>,
    rows: &[Vec<V>],
    from: (usize, usize),
    to: (usize, usize),
) -> Vec<Vec<V>> {
    let mut rows = rows.to_vec();
    let Some(value) = rows.get(from.0).and_then(|row| row.get(from.1)).copied() else {
        return rows;
    };
    rows[from.0].remove(from.1);
    let (to_row, mut to_ix) = to;
    if to_row == from.0 && from.1 < to_ix {
        to_ix -= 1;
    }
    let unique = !registry.repeats(value);
    if let Some(row) = rows.get_mut(to_row) {
        insert_row_unique(registry, row, value, to_ix, unique);
    }
    rows
}

fn inserted<V: PartialEq + Copy>(
    registry: &Arrangement<V>,
    rows: &[Vec<V>],
    value: V,
    to: (usize, usize),
) -> Vec<Vec<V>> {
    let mut rows = rows.to_vec();
    let unique = !registry.repeats(value);
    if let Some(row) = rows.get_mut(to.0) {
        insert_row_unique(registry, row, value, to.1, unique);
    }
    rows
}

fn removed_at<V: Copy>(rows: &[Vec<V>], at: (usize, usize)) -> Vec<Vec<V>> {
    let mut rows = rows.to_vec();
    if let Some(row) = rows.get_mut(at.0)
        && at.1 < row.len()
    {
        row.remove(at.1);
    }
    rows
}

fn removed_row<V: Copy>(rows: &[Vec<V>], at: usize) -> Vec<Vec<V>> {
    let mut rows = rows.to_vec();
    if rows.len() > 1 && at < rows.len() {
        rows.remove(at);
    }
    rows
}

fn without<V: PartialEq + Copy>(registry: &Arrangement<V>, items: &[V], value: V) -> Vec<V> {
    items
        .iter()
        .copied()
        .filter(|v| !registry.same(*v, value))
        .collect()
}

/// Slot `value` after every shown item that precedes it in the catalog.
fn insert_stock<V: PartialEq + Copy>(registry: &Arrangement<V>, items: &[V], value: V) -> Vec<V> {
    let target = registry.rank(value);
    let at = items.iter().filter(|v| registry.rank(**v) < target).count();
    let mut items = items.to_vec();
    items.insert(at.min(items.len()), value);
    items
}

/// Show or hide `value`, slotting a re-shown item at its stock position.
/// Hiding takes the item off on any variant.
pub fn toggled<V: PartialEq + Copy + 'static>(
    registry: impl Into<Arrangement<V>>,
    items: &[V],
    value: V,
) -> Vec<V> {
    let registry = registry.into();

    if registry.holds(items, value) {
        without(&registry, items, value)
    } else {
        insert_stock(&registry, items, value)
    }
}

/// The stash, if it still describes the row: minus `hidden`, it has to
/// match the live list exactly, variants included.
fn restored<V: PartialEq + Copy>(
    registry: &Arrangement<V>,
    stash: &[V],
    items: &[V],
    hidden: &[V],
) -> Option<Vec<V>> {
    let kept: Vec<V> = stash
        .iter()
        .copied()
        .filter(|v| !registry.holds(hidden, *v))
        .collect();
    (kept == items).then(|| stash.to_vec())
}

/// [`toggled`] that keeps a hand-arranged row: hiding stashes the row, so
/// showing puts it back whole instead of at catalog rank. `values` moves as
/// one group. There's one stash slot per toggle, and [`restored`] falls
/// back to the stock insert when the row changed in between.
pub fn toggled_stashed<V: PartialEq + Copy + 'static>(
    registry: impl Into<Arrangement<V>>,
    items: &[V],
    stash: &mut Option<Vec<V>>,
    values: &[V],
) -> Vec<V> {
    let registry = registry.into();

    if values.iter().any(|value| registry.holds(items, *value)) {
        *stash = Some(items.to_vec());
        return items
            .iter()
            .copied()
            .filter(|value| !registry.holds(values, *value))
            .collect();
    }
    if let Some(kept) = stash
        .take()
        .and_then(|stash| restored(&registry, &stash, items, values))
    {
        return kept;
    }
    let mut out = items.to_vec();
    for value in values {
        if !registry.holds(&out, *value) {
            out = insert_stock(&registry, &out, *value);
        }
    }
    out
}

/// Drop repeats from a hand-edited dump, keeping first positions.
/// Repeatable items pass through, and two variants of one item are a repeat.
pub fn dedup<V: PartialEq + Copy + 'static>(
    registry: impl Into<Arrangement<V>>,
    items: Vec<V>,
) -> Vec<V> {
    let registry = registry.into();

    let mut out: Vec<V> = Vec::with_capacity(items.len());
    for item in items {
        if registry.repeats(item) || !registry.holds(&out, item) {
            out.push(item);
        }
    }
    out
}

/// [`arrange_rows_editor`] capped at one well, handing `apply` the single row.
pub fn arrange_editor<P: 'static, V: PartialEq + Copy + 'static>(
    id: &'static str,
    registry: impl Into<Arrangement<V>>,
    items: &[V],
    apply: impl Fn(&mut P, Vec<V>, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Stateful<Div> {
    arrange_rows_editor(
        id,
        registry,
        &[items.to_vec()],
        Some(1),
        move |this, rows, cx| apply(this, rows.into_iter().next().unwrap_or_default(), cx),
        cx,
    )
}

/// One well per row over the hidden tray, plus an add-row button while
/// `max_rows` allows. `id` keeps a drag from landing in another window's
/// copy and scopes the subtree's element ids: chips key on catalog ids, so
/// two editors on one page would otherwise share drag state.
pub fn arrange_rows_editor<P: 'static, V: PartialEq + Copy + 'static>(
    id: &'static str,
    registry: impl Into<Arrangement<V>>,
    rows: &[Vec<V>],
    max_rows: Option<usize>,
    apply: impl Fn(&mut P, Vec<Vec<V>>, &mut Context<P>) + Clone + 'static,
    cx: &mut Context<P>,
) -> Stateful<Div> {
    // At least one well, so an emptied config still has a drop target.
    let rows: Vec<Vec<V>> = if rows.is_empty() {
        vec![Vec::new()]
    } else {
        rows.to_vec()
    };

    let registry = registry.into();
    let entries = registry.entries();

    // A drop lands before the chip under it; the tail catches the row's end.
    let mut wells = div().flex().flex_col().gap(tokens::SPACE_XS);
    for (row_ix, row) in rows.iter().enumerate() {
        let mut bar = well();
        for (ix, value) in row.iter().copied().enumerate() {
            let Some(entry) = entries.iter().find(|entry| entry.owns(value)) else {
                continue;
            };
            let drag = ArrangeDrag {
                editor: id,
                value,
                from: Some((row_ix, ix)),
                label: entry.label.clone(),
                icon: entry.icon,
            };
            let picks = !entry.variants.is_empty();
            let variant_label = entry
                .variants
                .iter()
                .find(|(_, variant)| *variant == value)
                .map(|(label, _)| label.clone());
            // Keyed by place too: two spacers sharing an id would share drag state.
            let chip_id = ElementId::from((entry.id.clone(), (row_ix << 16) | ix));
            let drop_registry = registry.clone();
            let drop_rows = rows.clone();
            let drop_apply = apply.clone();
            let hide_rows = rows.clone();
            let hide_apply = apply.clone();
            let placed = chip(entry.label.clone(), entry.icon, false)
                .id(chip_id.clone())
                .cursor_pointer()
                .on_drag(drag, |drag, _pos, _window, cx| {
                    cx.new(|_| ChipPreview {
                        label: drag.label.clone(),
                        icon: drag.icon,
                    })
                })
                .drag_over::<ArrangeDrag<V>>(move |style, drag, _, _| {
                    if drag.editor == id {
                        style.bg(palette::alpha(palette::accent(), 0x33))
                    } else {
                        style
                    }
                })
                .on_drop(cx.listener(move |this, drag: &ArrangeDrag<V>, _, cx| {
                    if drag.editor != id {
                        return;
                    }
                    let rows = match drag.from {
                        Some(from) => moved_at(&drop_registry, &drop_rows, from, (row_ix, ix)),
                        None => inserted(&drop_registry, &drop_rows, drag.value, (row_ix, ix)),
                    };
                    drop_apply(this, rows, cx);
                }))
                .when_some(variant_label, |chip, label| {
                    chip.child(
                        div()
                            .text_xs()
                            .whitespace_nowrap()
                            .text_color(palette::text_faint())
                            .child(label),
                    )
                })
                .child(chip_action(icons::CLOSE).on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        // Kept off the chip, whose click would open the picker.
                        if picks {
                            cx.stop_propagation();
                        }
                        hide_apply(this, removed_at(&hide_rows, (row_ix, ix)), cx);
                    }),
                ));
            if !picks {
                bar = bar.child(placed);
                continue;
            }

            // A pick swaps this place's value and keeps everything else.
            let weak = cx.entity().downgrade();
            let pick_rows = rows.clone();
            let pick_apply = apply.clone();
            let variants = entry.variants.clone();
            let menu: MenuBuild = Rc::new(move |mut menu, _, _| {
                for (label, variant) in &variants {
                    let variant = *variant;
                    let weak = weak.clone();
                    let rows = pick_rows.clone();
                    let apply = pick_apply.clone();
                    menu = menu.item(
                        PopupMenuItem::new(label.clone())
                            .checked(variant == value)
                            .on_click(move |_, _, cx| {
                                let mut rows = rows.clone();
                                if let Some(slot) =
                                    rows.get_mut(row_ix).and_then(|row| row.get_mut(ix))
                                {
                                    *slot = variant;
                                }
                                if let Some(this) = weak.upgrade() {
                                    this.update(cx, |this, cx| apply(this, rows, cx));
                                }
                            }),
                    );
                }
                menu
            });
            bar = bar.child(VariantChip {
                id: chip_id,
                chip: placed,
                menu,
            });
        }
        let tail_registry = registry.clone();
        let tail_rows = rows.clone();
        let tail_apply = apply.clone();
        let tail_to = (row_ix, row.len());
        bar = bar.child(
            div()
                .flex_1()
                .min_w(px(24.))
                .h(px(26.))
                .rounded(tokens::RADIUS)
                .drag_over::<ArrangeDrag<V>>(move |style, drag, _, _| {
                    if drag.editor == id {
                        style.bg(palette::alpha(palette::accent(), 0x33))
                    } else {
                        style
                    }
                })
                .on_drop(cx.listener(move |this, drag: &ArrangeDrag<V>, _, cx| {
                    if drag.editor != id {
                        return;
                    }
                    let rows = match drag.from {
                        Some(from) => moved_at(&tail_registry, &tail_rows, from, tail_to),
                        None => inserted(&tail_registry, &tail_rows, drag.value, tail_to),
                    };
                    tail_apply(this, rows, cx);
                })),
        );
        if row.is_empty() && rows.len() > 1 {
            let drop_rows = rows.clone();
            let drop_apply = apply.clone();
            bar = bar.child(chip_action(icons::CLOSE).on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    drop_apply(this, removed_row(&drop_rows, row_ix), cx);
                }),
            ));
        }
        wells = wells.child(bar);
    }
    if max_rows.is_none_or(|max| rows.len() < max) {
        let add_rows = rows.clone();
        let add_apply = apply.clone();
        let drop_registry = registry.clone();
        let drop_rows = rows.clone();
        let drop_apply = apply.clone();
        wells = wells.child(
            div()
                .id("add-row")
                .flex()
                .flex_row()
                .items_center()
                .justify_center()
                .gap(tokens::SPACE_XS)
                .p(tokens::SPACE_XS)
                .min_h(px(36.))
                .rounded(tokens::RADIUS)
                .border_1()
                .border_dashed()
                .border_color(palette::border())
                .cursor_pointer()
                .hover(|s| s.border_color(palette::border_light()))
                .child(
                    svg()
                        .path(icons::PLUS)
                        .size(px(12.))
                        .text_color(palette::text_faint()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::text_faint())
                        .child(rox_i18n::t!("arrange-add-row")),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let mut rows = add_rows.clone();
                    rows.push(Vec::new());
                    add_apply(this, rows, cx);
                }))
                .drag_over::<ArrangeDrag<V>>(move |style, drag, _, _| {
                    if drag.editor == id {
                        style.bg(palette::alpha(palette::accent(), 0x33))
                    } else {
                        style
                    }
                })
                .on_drop(cx.listener(move |this, drag: &ArrangeDrag<V>, _, cx| {
                    if drag.editor != id {
                        return;
                    }
                    let mut rows = drop_rows.clone();
                    rows.push(Vec::new());
                    let to = (rows.len() - 1, 0);
                    let rows = match drag.from {
                        Some(from) => moved_at(&drop_registry, &rows, from, to),
                        None => inserted(&drop_registry, &rows, drag.value, to),
                    };
                    drop_apply(this, rows, cx);
                })),
        );
    }

    // The tray: chips some row still lacks, and the repeatable ones always. A
    // plus shows the item at its stock position on the first row without it.
    let tray_rows = rows.clone();
    let tray_apply = apply.clone();
    let mut tray = well()
        .drag_over::<ArrangeDrag<V>>(move |style, drag, _, _| {
            if drag.editor == id {
                style.bg(palette::alpha(palette::accent(), 0x1a))
            } else {
                style
            }
        })
        .on_drop(cx.listener(move |this, drag: &ArrangeDrag<V>, _, cx| {
            if drag.editor != id {
                return;
            }
            let Some(from) = drag.from else {
                return;
            };
            tray_apply(this, removed_at(&tray_rows, from), cx);
        }));
    for entry in entries
        .iter()
        .filter(|entry| entry.repeats || rows.iter().any(|row| !row.iter().any(|v| entry.owns(*v))))
    {
        let drag = ArrangeDrag {
            editor: id,
            value: entry.value,
            from: None,
            label: entry.label.clone(),
            icon: entry.icon,
        };
        let show_registry = registry.clone();
        let show_rows = rows.clone();
        let show_apply = apply.clone();
        let value = entry.value;
        tray = tray.child(
            chip(entry.label.clone(), entry.icon, true)
                .id(entry.id.clone())
                .cursor_pointer()
                .on_drag(drag, |drag, _pos, _window, cx| {
                    cx.new(|_| ChipPreview {
                        label: drag.label.clone(),
                        icon: drag.icon,
                    })
                })
                .child(chip_action(icons::PLUS).on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        let mut rows = show_rows.clone();
                        let target = rows
                            .iter()
                            .position(|row| !show_registry.holds(row, value))
                            .unwrap_or(0);
                        rows[target] = insert_stock(&show_registry, &rows[target], value);
                        show_apply(this, rows, cx);
                    }),
                )),
        );
    }

    div()
        .id(id)
        .flex()
        .flex_col()
        .gap(tokens::SPACE_XS)
        .child(caption(rox_i18n::t!("arrange-shown")))
        .child(wells)
        .child(caption(rox_i18n::t!("arrange-hidden")))
        .child(tray)
}

#[cfg(test)]
mod tests {
    use gpui::SharedString;

    use super::{
        ArrangeEntry, ArrangeSpec, ArrangeVariant, Arrangement, dedup, insert_stock, inserted,
        moved_at, removed_at, removed_row, toggled, toggled_stashed, without,
    };

    fn moved(rows: &[Vec<u8>], from: (usize, usize), to: (usize, usize)) -> Vec<Vec<u8>> {
        moved_at(&Arrangement::Stock(REGISTRY), rows, from, to)
    }
    fn insert(rows: &[Vec<u8>], value: u8, to: (usize, usize)) -> Vec<Vec<u8>> {
        inserted(&Arrangement::Stock(REGISTRY), rows, value, to)
    }
    fn stock(items: &[u8], value: u8) -> Vec<u8> {
        insert_stock(&Arrangement::Stock(REGISTRY), items, value)
    }

    /// Value 3 stands in for a spacer: the one repeatable entry.
    const REGISTRY: &[ArrangeSpec<u8>] = &[
        ArrangeSpec {
            key: "a",
            icon: None,
            value: 0,
            repeats: false,
            variants: &[],
        },
        ArrangeSpec {
            key: "b",
            icon: None,
            value: 1,
            repeats: false,
            variants: &[],
        },
        ArrangeSpec {
            key: "c",
            icon: None,
            value: 2,
            repeats: false,
            variants: &[],
        },
        ArrangeSpec {
            key: "d",
            icon: None,
            value: 3,
            repeats: true,
            variants: &[],
        },
    ];

    /// Value 4 is one item drawn two ways: 4 its default, 5 the other look.
    const VARIED: &[ArrangeSpec<u8>] = &[
        ArrangeSpec {
            key: "a",
            icon: None,
            value: 0,
            repeats: false,
            variants: &[],
        },
        ArrangeSpec {
            key: "e",
            icon: None,
            value: 4,
            repeats: false,
            variants: &[
                ArrangeVariant {
                    key: "e-plain",
                    value: 4,
                },
                ArrangeVariant {
                    key: "e-badge",
                    value: 5,
                },
            ],
        },
        ArrangeSpec {
            key: "b",
            icon: None,
            value: 1,
            repeats: false,
            variants: &[],
        },
    ];

    #[test]
    fn place_ops_reorder_insert_and_remove() {
        assert_eq!(moved(&[vec![0, 1, 2]], (0, 0), (0, 2)), [vec![1, 0, 2]]);
        assert_eq!(moved(&[vec![0, 1, 2]], (0, 2), (0, 0)), [vec![2, 0, 1]]);
        assert_eq!(moved(&[vec![0, 1, 2]], (0, 0), (0, 3)), [vec![1, 2, 0]]);
        // Two copies keep their identities.
        assert_eq!(moved(&[vec![3, 0, 3]], (0, 2), (0, 0)), [vec![3, 3, 0]]);
        assert_eq!(insert(&[vec![0, 3]], 3, (0, 1)), [vec![0, 3, 3]]);
        assert_eq!(removed_at(&[vec![3, 0, 3]], (0, 2)), [vec![3, 0]]);
    }

    #[test]
    fn row_ops_move_across_and_drop_rows() {
        assert_eq!(
            moved(&[vec![0, 1], vec![2]], (0, 1), (1, 0)),
            [vec![0], vec![1, 2]]
        );
        assert_eq!(
            moved(&[vec![0], vec![1, 2]], (1, 0), (0, 1)),
            [vec![0, 1], vec![2]]
        );
        assert_eq!(removed_row(&[vec![0], vec![]], 1), [vec![0]]);
        assert_eq!(removed_row::<u8>(&[vec![0]], 0), [vec![0]]);
    }

    #[test]
    fn row_landings_keep_a_row_unique() {
        assert_eq!(
            moved(&[vec![0, 1], vec![0]], (1, 0), (0, 2)),
            [vec![1, 0], vec![]]
        );
        assert_eq!(
            insert(&[vec![0, 1], vec![0]], 0, (0, 2)),
            [vec![1, 0], vec![0]]
        );
        assert_eq!(insert(&[vec![3, 0]], 3, (0, 2)), [vec![3, 0, 3]]);
    }

    #[test]
    fn stock_insert_restores_catalog_order() {
        assert_eq!(stock(&[0, 1, 3], 2), vec![0, 1, 2, 3]);
        assert_eq!(stock(&[1, 2], 0), vec![0, 1, 2]);
    }

    #[test]
    fn dedup_keeps_first_positions_and_repeats() {
        assert_eq!(dedup(REGISTRY, vec![2, 0, 2, 1, 0]), vec![2, 0, 1]);
        assert_eq!(dedup(REGISTRY, vec![3, 0, 3, 3]), vec![3, 0, 3, 3]);
        assert_eq!(
            without(&Arrangement::Stock(REGISTRY), &[0, 1, 2], 1),
            vec![0, 2]
        );
    }

    #[test]
    fn stashed_toggle_returns_a_rearranged_row() {
        let row = vec![2, 0, 1];
        let mut stash = None;
        let hidden = toggled_stashed(REGISTRY, &row, &mut stash, &[0]);
        assert_eq!(hidden, vec![2, 1]);
        assert_eq!(toggled_stashed(REGISTRY, &hidden, &mut stash, &[0]), row);
        assert_eq!(stock(&hidden, 0), vec![0, 2, 1]);
        assert!(stash.is_none(), "the stash is spent once it's used");
    }

    #[test]
    fn stashed_toggle_moves_a_group() {
        let row = vec![2, 0, 3, 1];
        let mut stash = None;
        let hidden = toggled_stashed(REGISTRY, &row, &mut stash, &[0, 1]);
        assert_eq!(hidden, vec![2, 3]);
        assert_eq!(toggled_stashed(REGISTRY, &hidden, &mut stash, &[0, 1]), row);
    }

    #[test]
    fn an_edit_under_the_stash_falls_back_to_stock() {
        let mut stash = None;
        let hidden = toggled_stashed(REGISTRY, &[2, 0, 1], &mut stash, &[0]);
        assert_eq!(hidden, vec![2, 1]);
        let edited = vec![1, 2];
        assert_eq!(
            toggled_stashed(REGISTRY, &edited, &mut stash, &[0]),
            vec![0, 1, 2],
            "stock rank, not the stale stash"
        );
    }

    #[test]
    fn a_cold_stash_matches_the_plain_toggle() {
        let mut stash = None;
        assert_eq!(
            toggled_stashed(REGISTRY, &[2, 1], &mut stash, &[0]),
            toggled(REGISTRY, &[2, 1], 0)
        );
    }

    #[test]
    fn a_runtime_registry_behaves_like_a_static_one() {
        let built: Vec<ArrangeEntry<u8>> = [(0u8, false), (1, false), (2, false), (3, true)]
            .iter()
            .map(|(value, repeats)| ArrangeEntry {
                id: SharedString::from(format!("b{value}")),
                label: SharedString::from(format!("Button {value}")),
                icon: None,
                value: *value,
                repeats: *repeats,
                variants: Vec::new(),
            })
            .collect();
        let registry = Arrangement::from(built);

        assert_eq!(dedup(registry.clone(), vec![2, 0, 2, 1, 0]), vec![2, 0, 1]);
        assert_eq!(dedup(registry.clone(), vec![3, 0, 3, 3]), vec![3, 0, 3, 3]);
        assert_eq!(
            inserted(&registry, &[vec![0, 1]], 2, (0, 1)),
            [vec![0, 2, 1]]
        );
        assert_eq!(insert_stock(&registry, &[0, 1, 3], 2), vec![0, 1, 2, 3]);
    }

    #[test]
    fn variants_of_one_item_keep_a_row_unique() {
        let registry = Arrangement::Stock(VARIED);

        // The tray's default lands over the row's badge, not beside it.
        assert_eq!(inserted(&registry, &[vec![0, 5]], 4, (0, 0)), [vec![4, 0]]);
        // A move keeps the dragged item's variant.
        assert_eq!(
            moved_at(&registry, &[vec![5, 0], vec![4]], (0, 0), (1, 1)),
            [vec![0], vec![5]]
        );
        assert_eq!(dedup(VARIED, vec![5, 0, 4, 1]), vec![5, 0, 1]);
    }

    #[test]
    fn hiding_takes_any_variant_and_the_stash_brings_it_back() {
        assert_eq!(toggled(VARIED, &[0, 5, 1], 4), vec![0, 1]);
        assert_eq!(toggled(VARIED, &[0, 1], 4), vec![0, 4, 1]);

        let row = vec![1, 5, 0];
        let mut stash = None;
        let hidden = toggled_stashed(VARIED, &row, &mut stash, &[4]);
        assert_eq!(hidden, vec![1, 0]);
        assert_eq!(toggled_stashed(VARIED, &hidden, &mut stash, &[4]), row);

        // A cold stash shows the default at stock rank.
        assert_eq!(
            toggled_stashed(VARIED, &[0, 1], &mut None, &[4]),
            vec![0, 4, 1]
        );
    }
}
