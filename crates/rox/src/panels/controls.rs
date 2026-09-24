//! The Custom Controls panel: a strip of buttons the user builds. Each fires a
//! global command, and its glyph, colour and tooltip follow a piece of live
//! player state through a case table picked from lists, never expressions or
//! scripts.
//!
//! It lives in the binary because it calls [`keymap::dispatch`], and
//! `rox-panels` can't depend on the binary.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use gpui::{
    AnyElement, App, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, MouseButton, Rgba,
    SharedString, Stateful, Subscription, WeakEntity, Window, div, prelude::*, px, svg,
};
use gpui_component::Sizable as _;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::PopupMenu;
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::buttons::{self, StateSpec};
use rox_panel_api::panel::{self, AppState, PanelChrome, PanelSettings};
use rox_panel_api::panel_settings;
use rox_panel_api::position_bound;
use rox_panel_kit::ui::{self as settings_ui, SECTION_GAP, section};
use rox_panel_kit::{
    Align, PickRow, Tip, icon_picker, justify, picker, search_picker, setting_block, setting_row,
};

use crate::keymap::{self, Group};

const GLYPH: f32 = 16.0;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ControlsConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    /// Every defined button. Defining one is not placing it: see `items`.
    pub buttons: Vec<CustomButton>,
    /// The strip in display order, buttons by id with furniture between. A
    /// defined button missing here sits in the Layout page's tray.
    pub items: Vec<WellItem>,
    pub align: Align,
}

impl Default for ControlsConfig {
    /// Centred: a few buttons hugging one edge reads as unfinished.
    fn default() -> Self {
        ControlsConfig {
            chrome: PanelChrome::default(),
            buttons: Vec::new(),
            items: Vec::new(),
            align: Align::Center,
        }
    }
}

/// Untagged so a saved well reads as `[3, "spacer", 7]`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WellItem {
    Button(u64),
    Furniture(Furniture),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Furniture {
    Spacer,
    Divider,
}

enum Placed<'a> {
    Button(&'a CustomButton),
    Furniture(Furniture),
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CustomButton {
    /// Stable and persisted, so per-button editor state survives a reorder. 0
    /// is unassigned; the panel assigns on load and on add.
    pub id: u64,
    /// A `keymap::COMMANDS` id. Empty means unconfigured.
    pub action: String,
    /// A `buttons::STATES` id. Empty means stateless, and `cases` then
    /// holds exactly one entry used for every draw.
    pub state: String,
    pub cases: Vec<ButtonCase>,
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ButtonCase {
    /// The `StateCase` id this draws for. Empty for the stateless entry.
    pub when: String,
    /// An `icons::CATALOG` path.
    pub icon: String,
    /// A `palette::ROLES` name. Empty falls back to `text`.
    pub color: String,
    /// User text, not translated.
    pub tip: String,
}

/// Give zero and duplicate ids fresh ones, keeping the rest.
fn assign_button_ids(buttons: &mut [CustomButton]) {
    let mut next = buttons.iter().map(|def| def.id).max().unwrap_or(0) + 1;
    for i in 0..buttons.len() {
        let taken = buttons[..i].iter().any(|def| def.id == buttons[i].id);
        if buttons[i].id == 0 || taken {
            buttons[i].id = next;
            next += 1;
        }
    }
}

/// Drop ids no button carries and repeat mentions, so a hand-edited layout
/// can't hold a place nothing draws.
fn normalize_items(items: &[WellItem], buttons: &[CustomButton]) -> Vec<WellItem> {
    let mut out: Vec<WellItem> = Vec::with_capacity(items.len());
    for item in items {
        match item {
            WellItem::Furniture(_) => out.push(*item),

            WellItem::Button(id) => {
                let defined = buttons.iter().any(|def| def.id == *id);
                if defined && !out.contains(item) {
                    out.push(*item);
                }
            }
        }
    }

    out
}

fn placed<'a>(items: &[WellItem], buttons: &'a [CustomButton]) -> Vec<Placed<'a>> {
    items
        .iter()
        .filter_map(|item| match item {
            WellItem::Furniture(kind) => Some(Placed::Furniture(*kind)),
            WellItem::Button(id) => buttons.iter().find(|def| def.id == *id).map(Placed::Button),
        })
        .collect()
}

fn remove_at(buttons: &mut Vec<CustomButton>, items: &mut Vec<WellItem>, index: usize) {
    if index >= buttons.len() {
        return;
    }

    let gone = WellItem::Button(buttons.remove(index).id);
    items.retain(|item| *item != gone);
}

/// Built per render: a button's command and icon change while the page is open.
fn well_registry(buttons: &[CustomButton]) -> Vec<panel::ArrangeEntry<WellItem>> {
    let mut entries: Vec<_> = buttons
        .iter()
        .map(|def| {
            let label = if def.action.is_empty() {
                rox_i18n::t!("button-editor-unnamed")
            } else {
                command_label(&def.action)
            };

            // Keyed by button id, not label: two buttons can fire one command.
            panel::ArrangeEntry {
                id: SharedString::from(format!("b{}", def.id)),
                label,
                icon: Some(icon_for(
                    def.cases
                        .first()
                        .map(|case| case.icon.as_str())
                        .unwrap_or(""),
                )),
                value: WellItem::Button(def.id),
                repeats: false,
            }
        })
        .collect();

    entries.push(panel::ArrangeEntry {
        id: SharedString::new_static("spacer"),
        label: rox_i18n::t!("head-piece-spacer"),
        icon: Some(icons::MOVE_HORIZONTAL),
        value: WellItem::Furniture(Furniture::Spacer),
        repeats: true,
    });
    entries.push(panel::ArrangeEntry {
        id: SharedString::new_static("divider"),
        label: rox_i18n::t!("head-piece-divider"),
        icon: Some(icons::MINUS),
        value: WellItem::Furniture(Furniture::Divider),
        repeats: true,
    });

    entries
}

/// A path missing from the catalog degrades to the placeholder.
fn icon_for(path: &str) -> &'static str {
    icons::CATALOG
        .iter()
        .find(|entry| **entry == path)
        .copied()
        .unwrap_or(icons::SQUARE_DASHED)
}

fn color_for(role: &str) -> Rgba {
    let Some(entry) = palette::ROLES.iter().find(|entry| entry.name == role) else {
        return palette::text();
    };

    (entry.get)(&palette::resolved())
}

fn state_spec(id: &str) -> Option<&'static StateSpec> {
    buttons::STATES.iter().find(|spec| spec.id == id)
}

/// Commands that need a position in a track, so their buttons lock while a
/// station plays. Held here rather than as a field on `keymap::COMMANDS`; the
/// test below keeps the two in step.
const POSITION_BOUND: &[&str] = &[
    "ab_repeat",
    "bookmark",
    "bookmark_named",
    "cue",
    "cue_next",
    "cue_prev",
    "next_bookmark",
    "prev_bookmark",
];

fn needs_position(action: &str) -> bool {
    POSITION_BOUND.contains(&action)
}

fn command_label(id: &str) -> SharedString {
    keymap::COMMANDS
        .iter()
        .find(|command| command.id == id)
        .map(|command| SharedString::from(command.label))
        .unwrap_or_else(|| SharedString::from(id.to_owned()))
}

struct ButtonLook {
    icon: &'static str,
    color: Rgba,
    tip: SharedString,
}

impl ButtonLook {
    fn placeholder() -> Self {
        ButtonLook {
            icon: icons::SQUARE_DASHED,
            color: palette::text_muted(),
            tip: rox_i18n::t!("panel-controls-unconfigured"),
        }
    }
}

fn look(def: &CustomButton, case: Option<&str>) -> ButtonLook {
    if def.action.is_empty() {
        return ButtonLook::placeholder();
    }

    let row = case
        .and_then(|id| def.cases.iter().find(|row| row.when == id))
        .or_else(|| def.cases.first());

    let Some(row) = row else {
        return ButtonLook::placeholder();
    };

    let tip = if row.tip.is_empty() {
        command_label(&def.action)
    } else {
        SharedString::from(row.tip.clone())
    };

    ButtonLook {
        icon: icon_for(&row.icon),
        color: color_for(&row.color),
        tip,
    }
}

/// Keeps the rows for cases the new state still has, so re-picking a state
/// never discards icon work.
fn seed_cases(spec: Option<&StateSpec>, held: &[ButtonCase]) -> Vec<ButtonCase> {
    let Some(spec) = spec else {
        let kept = held.iter().find(|row| row.when.is_empty()).cloned();
        return vec![kept.unwrap_or_else(|| ButtonCase {
            // Named rather than empty: the colour picker labels an unknown role
            // with its head option.
            color: "text".to_string(),
            ..ButtonCase::default()
        })];
    };

    spec.cases
        .iter()
        .map(|case| {
            held.iter()
                .find(|row| row.when == case.id)
                .cloned()
                .unwrap_or(ButtonCase {
                    when: case.id.to_string(),
                    icon: case.icon.to_string(),
                    color: case.color.to_string(),
                    tip: String::new(),
                })
        })
        .collect()
}

/// Never overwrites an action the user already chose.
fn prefill_action(action: &mut String, spec: Option<&StateSpec>) {
    if !action.is_empty() {
        return;
    }

    if let Some(spec) = spec.filter(|spec| !spec.action.is_empty()) {
        *action = spec.action.to_string();
    }
}

/// Built once: `COMMANDS` is a `LazyLock`, and a settings render shouldn't
/// rebuild the list every frame.
fn command_rows() -> Arc<Vec<PickRow>> {
    static ROWS: OnceLock<Arc<Vec<PickRow>>> = OnceLock::new();

    ROWS.get_or_init(|| {
        let mut rows = Vec::new();
        for group in Group::ALL {
            for command in keymap::global_commands().filter(|c| c.group == *group) {
                rows.push(command_row(command));
            }
        }
        Arc::new(rows)
    })
    .clone()
}

/// The id goes in the search terms so a command is findable by its settings
/// key.
fn command_row(command: &'static keymap::Command) -> PickRow {
    let label = command.label.to_lowercase();

    let mut terms: Vec<SharedString> = vec![command.id.to_lowercase().into()];
    terms.extend(
        label
            .split_whitespace()
            .map(|word| SharedString::from(word.to_owned())),
    );
    terms.push(label.into());

    PickRow {
        label: SharedString::from(command.label),
        value: Some(SharedString::from(command.id)),
        terms,
        icon: Some(SharedString::from(command.group.icon())),
    }
}

/// An unconfigured button opens the panel's settings rather than swallowing the
/// press.
fn press(
    action: String,
) -> impl Fn(&mut ControlsPanel, &mut Window, &mut Context<ControlsPanel>) + 'static {
    move |_, window, cx| {
        if action.is_empty() {
            // Deferred: `open` reads this entity back, and it's still leased
            // here.
            let panel = cx.entity();
            cx.defer(move |cx| panel_settings::open(panel, cx));
            return;
        }

        keymap::dispatch(&action, window, cx);
    }
}

/// One button as it draws, shared with the settings preview.
///
/// Rebuilt rather than calling `panel::icon_control`, which drops the window
/// [`keymap::dispatch`] needs. `.id()` rather than `.track_focus()`, so a press
/// doesn't pull focus out of a search box.
///
/// A `locked` button keeps its place and glyph, stops answering, and tips the
/// reason, so the strip doesn't shift when a station starts.
fn button_element(
    id: SharedString,
    look: &ButtonLook,
    locked: bool,
    on_click: impl Fn(&mut ControlsPanel, &mut Window, &mut Context<ControlsPanel>) + 'static,
    cx: &mut Context<ControlsPanel>,
) -> Stateful<Div> {
    let color = if locked {
        palette::text_muted()
    } else {
        look.color
    };
    let icon = look.icon;

    let body = div()
        .p(tokens::ICON_PAD)
        .rounded(tokens::RADIUS)
        .map(|d| {
            if locked {
                return d;
            }

            d.hover(|d| d.bg(palette::bg_control()))
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| on_click(this, window, cx)),
                )
        })
        .child(svg().path(icon).size(px(GLYPH)).text_color(color));

    let tip = if locked {
        position_bound::reason()
    } else {
        look.tip.clone()
    };

    Tip::keyed(id, tip).apply(body)
}

pub struct ControlsPanel {
    state: AppState,
    config: ControlsConfig,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    tips: HashMap<(u64, String), (Entity<InputState>, Subscription)>,
    /// Unfolded blocks on the settings page, never saved.
    open: HashSet<u64>,
    _player_changed: Subscription,
}

impl ControlsPanel {
    pub fn new(state: AppState, mut config: ControlsConfig, cx: &mut Context<Self>) -> Self {
        // Before the editor keys state on ids: a duplicate would share one
        // tooltip field.
        assign_button_ids(&mut config.buttons);

        config.items = normalize_items(&config.items, &config.buttons);

        let _player_changed = cx.observe(&state.player, |_, _, cx| cx.notify());

        ControlsPanel {
            state,
            config,
            focus: cx.focus_handle(),
            tab_panel: None,
            tips: HashMap::new(),
            open: HashSet::new(),
            _player_changed,
        }
    }

    fn live_cases(&self, placed: &[Placed<'_>], cx: &App) -> Vec<Option<&'static str>> {
        let player = self.state.player.read(cx);
        placed
            .iter()
            .map(|slot| match slot {
                Placed::Button(def) => buttons::read_state(&def.state, player),
                Placed::Furniture(_) => None,
            })
            .collect()
    }

    fn body(&mut self, cx: &mut Context<Self>) -> Div {
        let placed = placed(&self.config.items, &self.config.buttons);
        let cases = self.live_cases(&placed, cx);
        let unbound = !position_bound::allowed(&self.state, cx);

        let strip = div()
            .size_full()
            .bg(palette::bg_root())
            .flex()
            .items_center()
            .map(|d| justify(d, self.config.align))
            .gap(tokens::SPACE_XS)
            .px(tokens::SPACE_SM);

        // An empty well draws the placeholder, so a new panel is never blank.
        if placed.is_empty() {
            let blank = ButtonLook::placeholder();
            return strip.child(button_element(
                "custom-button-empty".into(),
                &blank,
                false,
                press(String::new()),
                cx,
            ));
        }

        let mut children = Vec::with_capacity(placed.len());
        for (index, slot) in placed.iter().enumerate() {
            let child = match slot {
                Placed::Button(def) => {
                    let resolved = look(def, cases[index]);
                    let id = SharedString::from(format!("custom-button-{}", def.id));
                    let locked = unbound && needs_position(&def.action);
                    button_element(id, &resolved, locked, press(def.action.clone()), cx)
                        .into_any_element()
                }

                Placed::Furniture(Furniture::Spacer) => div().flex_1().into_any_element(),

                Placed::Furniture(Furniture::Divider) => div()
                    .flex_1()
                    .h(px(1.))
                    .bg(palette::border())
                    .into_any_element(),
            };
            children.push(child);
        }

        strip.children(children)
    }
}

impl ControlsPanel {
    fn buttons_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        self.sync_tips(window, cx);

        let add = settings_ui::small_button(
            rox_i18n::t!("button-editor-add"),
            icons::PLUS,
            false,
            cx.listener(|this, _, _, cx| this.add_button(cx)),
        );

        let mut list = div().flex().flex_col().gap(tokens::SPACE_MD);
        if self.config.buttons.is_empty() {
            list = list.child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!("button-editor-empty")),
            );
        }
        for index in 0..self.config.buttons.len() {
            list = list.child(self.button_block(index, cx));
        }

        div().flex().flex_col().gap(SECTION_GAP).child(section(
            rox_i18n::t!("button-editor-section-buttons"),
            Some(add.into_any_element()),
            list,
        ))
    }

    /// The Layout page: alignment, then the well. Taking a button off the strip
    /// is a drag to the tray; deleting lives on the Content page.
    fn layout_page(&mut self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(panel::align_row(
                self.config.align,
                |this: &mut Self, align, cx| {
                    this.config.align = align;
                    cx.notify();
                },
                cx,
            ))
            .child(setting_block(
                rox_i18n::t!("button-editor-section-layout"),
                Some(rox_i18n::t!("button-editor-section-layout.description")),
                None,
                panel::arrange_editor(
                    "controls-items",
                    well_registry(&self.config.buttons),
                    &self.config.items,
                    |this: &mut Self, items, cx| {
                        this.config.items = items;
                        cx.notify();
                    },
                    cx,
                ),
            ))
    }

    /// Keyed by button id and case, not position, so a reorder doesn't hand one
    /// field's typing to its neighbour.
    fn sync_tips(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let wanted: Vec<(u64, String)> = self
            .config
            .buttons
            .iter()
            .flat_map(|def| {
                def.cases
                    .iter()
                    .map(move |case| (def.id, case.when.clone()))
            })
            .collect();

        self.tips.retain(|key, _| wanted.contains(key));

        for key in wanted {
            if self.tips.contains_key(&key) {
                continue;
            }

            let current = self
                .case(key.0, &key.1)
                .map(|case| case.tip.clone())
                .unwrap_or_default();
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(rox_i18n::t!("button-editor-tip-placeholder"))
                    .default_value(current)
            });
            let (id, when) = key.clone();
            let events = cx.subscribe(
                &input,
                move |this: &mut Self, input, event: &InputEvent, cx| {
                    if !matches!(event, InputEvent::Change) {
                        return;
                    }

                    let text = input.read(cx).value().to_string();
                    if let Some(case) = this.case_mut(id, &when) {
                        case.tip = text;
                        cx.notify();
                    }
                },
            );
            self.tips.insert(key, (input, events));
        }
    }

    fn case(&self, id: u64, when: &str) -> Option<&ButtonCase> {
        self.button(id)?.cases.iter().find(|case| case.when == when)
    }

    fn case_mut(&mut self, id: u64, when: &str) -> Option<&mut ButtonCase> {
        self.button_mut(id)?
            .cases
            .iter_mut()
            .find(|case| case.when == when)
    }

    fn button(&self, id: u64) -> Option<&CustomButton> {
        self.config.buttons.iter().find(|def| def.id == id)
    }

    fn button_mut(&mut self, id: u64) -> Option<&mut CustomButton> {
        self.config.buttons.iter_mut().find(|def| def.id == id)
    }

    fn add_button(&mut self, cx: &mut Context<Self>) {
        self.config.buttons.push(CustomButton::default());
        assign_button_ids(&mut self.config.buttons);

        if let Some(added) = self.config.buttons.last() {
            self.open.insert(added.id);
        }
        cx.notify();
    }

    fn toggle_open(&mut self, id: u64, cx: &mut Context<Self>) {
        if !self.open.remove(&id) {
            self.open.insert(id);
        }
        cx.notify();
    }

    fn remove_button(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.config.buttons.len() {
            remove_at(&mut self.config.buttons, &mut self.config.items, index);
            cx.notify();
        }
    }

    fn move_button(&mut self, index: usize, delta: isize, cx: &mut Context<Self>) {
        let Some(to) = index.checked_add_signed(delta) else {
            return;
        };

        if to < self.config.buttons.len() {
            self.config.buttons.swap(index, to);
            cx.notify();
        }
    }

    /// The block's own element id keeps two buttons' pickers from sharing one
    /// popup, since the ids below it repeat per button.
    fn button_block(&self, index: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        let def = &self.config.buttons[index];
        let id = def.id;
        let last = self.config.buttons.len() - 1;
        let spec = state_spec(&def.state);

        let name = if def.action.is_empty() {
            rox_i18n::t!("button-editor-unnamed")
        } else {
            command_label(&def.action)
        };

        let open = self.open.contains(&id);
        let header = settings_ui::block_header(
            div()
                .id(SharedString::from(format!("button-fold-{id}")))
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_XS)
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| this.toggle_open(id, cx)),
                )
                .child(
                    svg()
                        .path(if open {
                            icons::CHEVRON_DOWN
                        } else {
                            icons::CHEVRON_RIGHT
                        })
                        .size(px(12.))
                        .text_color(palette::text_muted()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(name),
                ),
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_XS)
                .child(settings_ui::icon_button(
                    icons::ARROW_UP,
                    index == 0,
                    cx.listener(move |this, _, _, cx| this.move_button(index, -1, cx)),
                ))
                .child(settings_ui::icon_button(
                    icons::ARROW_DOWN,
                    index == last,
                    cx.listener(move |this, _, _, cx| this.move_button(index, 1, cx)),
                ))
                .child(settings_ui::icon_button(
                    icons::TRASH,
                    false,
                    cx.listener(move |this, _, _, cx| this.remove_button(index, cx)),
                )),
        );

        let mut block = div()
            .id(SharedString::from(format!("button-{id}")))
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .child(header);

        if !open {
            return block;
        }

        block = block.child(setting_row(
            rox_i18n::t!("button-editor-action"),
            Some(rox_i18n::t!("button-editor-action.description")),
            self.command_field(id, def, cx),
        ));

        if def.action.is_empty() {
            return block;
        }

        block = block
            .child(setting_row(
                rox_i18n::t!("button-editor-state"),
                Some(rox_i18n::t!("button-editor-state.description")),
                self.state_field(id, def, cx),
            ))
            .child(self.preview_row(def, spec, cx));

        for case in &def.cases {
            block = block.child(self.case_block(id, case, spec, cx));
        }

        block
    }

    fn command_field(&self, id: u64, def: &CustomButton, cx: &mut Context<Self>) -> AnyElement {
        let current: Option<SharedString> = (!def.action.is_empty())
            .then(|| SharedString::from(def.action.clone()))
            .filter(|action| keymap::COMMANDS.iter().any(|command| *action == command.id));
        let label = match &current {
            Some(action) => command_label(action),
            None => rox_i18n::t!("button-editor-action-none"),
        };

        search_picker(
            "button-action",
            command_rows(),
            label,
            current,
            rox_i18n::t!("button-editor-action-search"),
            rox_i18n::t!("button-editor-action-empty"),
            move |this: &mut Self, value, cx| {
                let Some(value) = value else {
                    return;
                };

                if let Some(def) = this.button_mut(id) {
                    def.action = value;

                    if def.cases.is_empty() {
                        def.cases = seed_cases(None, &[]);
                    }
                }
                cx.notify();
            },
            cx,
        )
        .into_any_element()
    }

    fn state_field(&self, id: u64, def: &CustomButton, cx: &mut Context<Self>) -> AnyElement {
        let mut options = vec![(String::new(), rox_i18n::t!("button-editor-state-none"))];
        options.extend(
            buttons::STATES
                .iter()
                .map(|spec| (spec.id.to_string(), rox_i18n::t!(spec.label_key))),
        );

        picker(
            "button-state",
            def.state.clone(),
            options,
            false,
            move |this: &mut Self, state: String, cx| {
                let spec = state_spec(&state);
                if let Some(def) = this.button_mut(id) {
                    def.state = state;
                    def.cases = seed_cases(spec, &def.cases);
                    prefill_action(&mut def.action, spec);
                }
                cx.notify();
            },
            cx,
        )
        .into_any_element()
    }

    /// The button drawn once per case, so the whole state machine is on screen.
    /// A preview press fires the command like the real one.
    fn preview_row(
        &self,
        def: &CustomButton,
        spec: Option<&'static StateSpec>,
        cx: &mut Context<Self>,
    ) -> Div {
        // No wrap: the slot is measured at one line, and a wrapped row would
        // paint over the block below.
        let mut row = div().flex().flex_row().items_start().gap(tokens::SPACE_MD);

        for (index, case) in def.cases.iter().enumerate() {
            let resolved = look(def, Some(&case.when));
            let id = SharedString::from(format!("preview-{}-{index}", def.id));
            row = row.child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(2.))
                    // Never locks: the preview draws the case table, not the
                    // session.
                    .child(button_element(
                        id,
                        &resolved,
                        false,
                        press(def.action.clone()),
                        cx,
                    ))
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette::text_muted())
                            .child(case_label(spec, &case.when)),
                    ),
            );
        }

        setting_row(rox_i18n::t!("button-editor-preview"), None, row)
    }

    fn case_block(
        &self,
        id: u64,
        case: &ButtonCase,
        spec: Option<&'static StateSpec>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let when = case.when.clone();
        let roles: Vec<(String, SharedString)> = palette::ROLES
            .iter()
            .map(|role| (role.name.to_string(), SharedString::from(role.label)))
            .collect();

        let icon = {
            let when = when.clone();
            icon_picker(
                "case-icon",
                &case.icon,
                move |this: &mut Self, path: SharedString, cx| {
                    if let Some(case) = this.case_mut(id, &when) {
                        case.icon = path.to_string();
                        cx.notify();
                    }
                },
                cx,
            )
        };

        let color = {
            let when = when.clone();
            let current = if case.color.is_empty() {
                "text".to_string()
            } else {
                case.color.clone()
            };
            picker(
                "case-color",
                current,
                roles,
                false,
                move |this: &mut Self, role: String, cx| {
                    if let Some(case) = this.case_mut(id, &when) {
                        case.color = role;
                        cx.notify();
                    }
                },
                cx,
            )
        };

        let mut body = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .child(settings_ui::block_header(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(case_label(spec, &when)),
                div(),
            ))
            .child(setting_row(rox_i18n::t!("button-editor-icon"), None, icon))
            .child(setting_row(
                rox_i18n::t!("button-editor-color"),
                None,
                color,
            ));

        if let Some((input, _)) = self.tips.get(&(id, when.clone())) {
            body = body.child(setting_row(
                rox_i18n::t!("button-editor-tip"),
                None,
                div().w(px(180.)).child(Input::new(input).small()),
            ));
        }

        div()
            .id(SharedString::from(format!("case-{id}-{when}")))
            .child(settings_ui::nested(body))
    }
}

fn case_label(spec: Option<&'static StateSpec>, when: &str) -> SharedString {
    let Some(spec) = spec else {
        return rox_i18n::t!("button-editor-case-always");
    };

    spec.cases
        .iter()
        .find(|case| case.id == when)
        .map(|case| rox_i18n::t!(case.label_key))
        .unwrap_or_else(|| rox_i18n::t!("button-editor-case-always"))
}

impl PanelSettings for ControlsPanel {
    fn state(&self) -> AppState {
        self.state.clone()
    }

    fn chrome(&self) -> &PanelChrome {
        &self.config.chrome
    }

    fn chrome_mut(&mut self) -> &mut PanelChrome {
        &mut self.config.chrome
    }

    fn set_custom_title(&mut self, title: Option<String>, cx: &mut Context<Self>) {
        self.config.chrome.title = title;
        panel::refresh_tab_panel(&self.tab_panel, cx);
        cx.notify();
    }

    fn pages(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("Layout", icons::ALIGN_LEFT),
            ("Content", icons::LAYOUT_GRID),
        ]
    }

    fn page(
        &mut self,
        page: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match page {
            "Layout" => self.layout_page(cx).into_any_element(),

            _ => self.buttons_page(window, cx).into_any_element(),
        }
    }
}

impl EventEmitter<PanelEvent> for ControlsPanel {}

impl Focusable for ControlsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for ControlsPanel {
    fn panel_name(&self) -> &'static str {
        "custom controls"
    }

    rox_panel_api::opens_settings!();

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("panel-title-custom-controls"),
        )
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.config.chrome.title.clone().map(SharedString::from)
    }

    fn locked(&self, _cx: &App) -> bool {
        self.config.chrome.locked
    }

    /// Edge to edge: the body pads itself.
    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }

    fn min_size(&self, _cx: &App) -> gpui::Size<gpui::Pixels> {
        rox_panel_api::panel::chrome_min_size(
            &self.config.chrome,
            gpui::size(
                rox_dock::resizable::PANEL_MIN_SIZE,
                rox_dock::resizable::PANEL_MIN_SIZE,
            ),
        )
    }

    fn max_size(&self, cx: &App) -> gpui::Size<gpui::Pixels> {
        rox_panel_api::panel::chrome_max_size(&self.config.chrome, self.min_size(cx))
    }

    fn dump(&self, _cx: &App) -> rox_dock::PanelState {
        let mut state = rox_dock::PanelState::new(self);
        state.info = rox_dock::PanelInfo::panel(
            serde_json::to_value(self.config.clone()).unwrap_or(serde_json::Value::Null),
        );
        state
    }

    fn on_added_to(
        &mut self,
        tab_panel: WeakEntity<TabPanel>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tab_panel = Some(tab_panel.clone());
        self.state
            .tab_hosts
            .update(cx, |hosts, _| hosts.report(tab_panel));
    }

    fn on_removed(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.tab_panel = None;
    }

    fn dropdown_menu(
        &mut self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let menu =
            panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity(), cx);
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
            window,
        )
    }
}

impl Render for ControlsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(id: u64) -> WellItem {
        WellItem::Button(id)
    }

    fn button(id: u64, action: &str, state: &str, cases: &[(&str, &str, &str)]) -> CustomButton {
        CustomButton {
            id,
            action: action.to_string(),
            state: state.to_string(),
            cases: cases
                .iter()
                .map(|(when, icon, color)| ButtonCase {
                    when: when.to_string(),
                    icon: icon.to_string(),
                    color: color.to_string(),
                    tip: String::new(),
                })
                .collect(),
        }
    }

    #[test]
    fn config_round_trips() {
        let config = ControlsConfig {
            buttons: vec![
                button(
                    3,
                    "toggle_playback",
                    "player.playback",
                    &[
                        ("playing", icons::PAUSE, "text"),
                        ("paused", icons::PLAY, "text"),
                    ],
                ),
                button(7, "cycle_loop", "", &[("", icons::REPEAT, "accent")]),
            ],
            items: vec![b(7), WellItem::Furniture(Furniture::Spacer), b(3)],
            align: Align::Center,
            ..ControlsConfig::default()
        };

        let json = serde_json::to_value(config.clone()).expect("the config serializes");
        let back: ControlsConfig = serde_json::from_value(json).expect("and reads back");

        // `assert!` rather than `assert_eq!`: the config types don't derive
        // `Debug`.
        assert!(
            back.buttons == config.buttons,
            "the buttons came back changed"
        );
        assert_eq!(back.items, config.items, "the well came back changed");
        assert!(back.align == Align::Center);
    }

    #[test]
    fn items_normalize() {
        let buttons = vec![
            button(3, "toggle_playback", "", &[]),
            button(7, "cycle_loop", "", &[]),
        ];

        let spacer = WellItem::Furniture(Furniture::Spacer);
        assert_eq!(
            normalize_items(&[b(9), b(7), spacer, b(7), b(3), spacer, b(0)], &buttons),
            vec![b(7), spacer, b(3), spacer]
        );
        assert_eq!(normalize_items(&[], &buttons), Vec::<WellItem>::new());
        assert_eq!(normalize_items(&[b(3), b(7)], &[]), Vec::<WellItem>::new());
    }

    #[test]
    fn the_well_reads_numbers_and_words() {
        let old: Vec<WellItem> = serde_json::from_str("[3, 7]").expect("the old shape parses");
        assert_eq!(old, vec![b(3), b(7)]);

        let mixed: Vec<WellItem> =
            serde_json::from_str(r#"[3, "spacer", 7, "divider"]"#).expect("the mixed shape parses");
        assert_eq!(
            mixed,
            vec![
                b(3),
                WellItem::Furniture(Furniture::Spacer),
                b(7),
                WellItem::Furniture(Furniture::Divider),
            ]
        );

        let json = serde_json::to_string(&mixed).expect("and writes back");
        assert_eq!(json, r#"[3,"spacer",7,"divider"]"#);
    }

    #[test]
    fn removing_a_button_drops_it_from_the_well() {
        let mut buttons = vec![
            button(3, "toggle_playback", "", &[]),
            button(7, "cycle_loop", "", &[]),
        ];
        let mut items = vec![b(7), b(3)];

        remove_at(&mut buttons, &mut items, 1);

        assert_eq!(items, vec![b(3)]);
        assert_eq!(buttons.len(), 1);
        assert_eq!(buttons[0].id, 3);

        remove_at(&mut buttons, &mut items, 4);
        assert_eq!(buttons.len(), 1);
    }

    #[test]
    fn the_strip_follows_the_well_not_the_list() {
        let buttons = vec![
            button(3, "toggle_playback", "", &[]),
            button(7, "cycle_loop", "", &[]),
            button(9, "play_random", "", &[]),
        ];

        let drawn: Vec<Option<u64>> = placed(
            &[b(7), WellItem::Furniture(Furniture::Divider), b(3)],
            &buttons,
        )
        .iter()
        .map(|slot| match slot {
            Placed::Button(def) => Some(def.id),
            Placed::Furniture(_) => None,
        })
        .collect();
        assert_eq!(
            drawn,
            vec![Some(7), None, Some(3)],
            "the strip sorted itself by definition"
        );
        assert!(
            !drawn.contains(&Some(9)),
            "a button off the well reached the strip"
        );

        assert!(placed(&[], &buttons).is_empty());
        assert!(placed(&[b(42)], &buttons).is_empty());
    }

    #[test]
    fn ids_normalize() {
        let mut buttons = vec![
            button(0, "", "", &[]),
            button(4, "", "", &[]),
            button(4, "", "", &[]),
            button(0, "", "", &[]),
            button(1, "", "", &[]),
        ];
        assign_button_ids(&mut buttons);

        let ids: Vec<u64> = buttons.iter().map(|def| def.id).collect();
        assert!(
            ids.iter().all(|id| *id != 0),
            "{ids:?} kept an unassigned id"
        );

        let mut unique = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), ids.len(), "{ids:?} repeats an id");

        assert_eq!(ids[1], 4);
        assert_eq!(ids[4], 1);
    }

    #[test]
    fn an_unknown_icon_falls_back() {
        assert_eq!(icon_for(icons::PLAY), icons::PLAY);
        assert_eq!(icon_for("icons/does-not-exist.svg"), icons::SQUARE_DASHED);
        assert_eq!(icon_for(""), icons::SQUARE_DASHED);
    }

    #[test]
    fn an_unknown_colour_falls_back() {
        assert_eq!(color_for("accent"), palette::accent());
        assert_eq!(color_for("not-a-role"), palette::text());
        assert_eq!(color_for(""), palette::text());
    }

    #[test]
    fn picking_a_state_seeds_every_case() {
        let spec = state_spec("player.repeat").expect("repeat is in the catalog");
        let seeded = seed_cases(Some(spec), &[]);

        assert_eq!(seeded.len(), spec.cases.len());
        for (case, stock) in seeded.iter().zip(spec.cases) {
            assert_eq!(case.when, stock.id);
            assert_eq!(case.icon, stock.icon);
            assert_eq!(case.color, stock.color);
            assert!(case.tip.is_empty(), "the stock look carries no tooltip");
        }
    }

    #[test]
    fn reseeding_keeps_matching_cases() {
        let repeat = state_spec("player.repeat").expect("repeat is in the catalog");
        let mut held = seed_cases(Some(repeat), &[]);
        held[1].icon = icons::INFINITY.to_string();
        held[1].tip = "mine".to_string();
        held.push(ButtonCase {
            when: "gone".to_string(),
            icon: icons::DICE.to_string(),
            color: "accent".to_string(),
            tip: String::new(),
        });

        let again = seed_cases(Some(repeat), &held);

        assert_eq!(again.len(), repeat.cases.len());
        assert_eq!(again[1].icon, icons::INFINITY, "an edited icon survived");
        assert_eq!(again[1].tip, "mine");
        assert!(
            !again.iter().any(|case| case.when == "gone"),
            "a case the state can't be in kept its row"
        );

        let stateless = seed_cases(None, &again);
        assert_eq!(stateless.len(), 1);
        assert!(stateless[0].when.is_empty());
    }

    #[test]
    fn picking_a_state_prefills_an_empty_action_only() {
        let repeat = state_spec("player.repeat").expect("repeat is in the catalog");

        let mut empty = String::new();
        prefill_action(&mut empty, Some(repeat));
        assert_eq!(empty, "cycle_loop");

        let mut chosen = "play_random".to_string();
        prefill_action(&mut chosen, Some(repeat));
        assert_eq!(chosen, "play_random", "a chosen action was overwritten");

        let mut stateless = String::new();
        prefill_action(&mut stateless, None);
        assert!(stateless.is_empty());
    }

    /// The one place the state catalog's prefills meet `keymap`: a bad id would
    /// seed a button that does nothing.
    #[test]
    fn every_state_action_is_a_global_command() {
        for spec in buttons::STATES {
            if spec.action.is_empty() {
                continue;
            }

            let Some(command) = keymap::COMMANDS.iter().find(|c| c.id == spec.action) else {
                panic!("{} prefills unknown command {}", spec.id, spec.action);
            };
            assert!(
                command.reach == keymap::Reach::Global,
                "{} prefills the panel-scoped command {}",
                spec.id,
                spec.action
            );
        }
    }

    /// Clearing A-B and the transport work over a stream and must not lock.
    #[test]
    fn only_the_position_bound_commands_lock() {
        for action in [
            "bookmark",
            "bookmark_named",
            "prev_bookmark",
            "next_bookmark",
            "cue",
            "cue_prev",
            "cue_next",
            "ab_repeat",
        ] {
            assert!(needs_position(action), "{action} should lock");
        }

        for action in [
            "ab_clear",
            "toggle_playback",
            "next_track",
            "cycle_loop",
            "",
        ] {
            assert!(!needs_position(action), "{action} should not lock");
        }
    }

    #[test]
    fn every_position_bound_id_is_a_command() {
        for action in POSITION_BOUND {
            assert!(
                keymap::COMMANDS.iter().any(|c| c.id == *action),
                "{action} is not a command"
            );
        }
    }
}
