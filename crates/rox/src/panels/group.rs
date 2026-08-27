//! The group panel: a run of panels sharing one dock slot as a resizable
//! split, so several can ride a single tab. The dock's own splits can't
//! live inside a tab, so the group hosts its children itself through
//! [`crate::composite`]; the divider drags are the group's own, not the
//! dock's resize machinery. A fresh group opens as a pair, the menu grows
//! it a slot at a time, and there is no cap.

use gpui::{
    canvas, div, prelude::*, px, relative, App, Axis, Context, Div, EventEmitter, FocusHandle,
    Focusable, SharedString, WeakEntity, Window,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::{Icon, Sizable as _};
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::composite::{self, DividerState, Slot};
use crate::workspace::Workspace;
use rox_design::assets::icons;
use rox_design::palette;
use rox_panel_api::panel::{self, AppState, PanelChrome, PanelSettings};
use rox_panel_api::panel_settings;

/// The divider's hit strip, wide enough to grab without reading as a gap.
const DIVIDER_W: f32 = 5.0;

/// The closest a seam can ride to its neighbor or the edge; keeps every
/// slot grabbable.
const SHARE_MIN: f32 = 0.05;

fn default_ratio() -> f32 {
    0.5
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GroupConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    /// Stacked (top over bottom) instead of side by side.
    pub stacked: bool,
    /// The first seam's position, the whole story back when a group was a
    /// fixed pair. Dumps from then carry only this, and it shadows
    /// `dividers[0]` on the way out so those builds still read ours.
    #[serde(default = "default_ratio")]
    pub ratio: f32,
    /// The seam positions as fractions of the span, ascending, one per
    /// divider; empty falls back to `ratio`.
    #[serde(default)]
    pub dividers: Vec<f32>,
}

impl Default for GroupConfig {
    fn default() -> Self {
        Self {
            chrome: PanelChrome::default(),
            stacked: false,
            ratio: default_ratio(),
            dividers: Vec::new(),
        }
    }
}

/// The seam positions for `count` slots: the stored list when it fits,
/// the pair era's single ratio, an even spread otherwise. Ascending is
/// enforced, so a hand-edited dump can't fold the split over itself.
fn normalized_dividers(config: &GroupConfig, count: usize) -> Vec<f32> {
    let seams = count.saturating_sub(1);
    let mut dividers = config.dividers.clone();
    if dividers.is_empty() && seams == 1 {
        dividers = vec![config.ratio];
    }
    if dividers.len() != seams {
        return (1..count).map(|ix| ix as f32 / count as f32).collect();
    }
    let mut prev = 0.0;
    for seam in dividers.iter_mut() {
        *seam = seam.clamp(prev, 1.0);
        prev = *seam;
    }
    dividers
}

pub struct GroupPanel {
    state: AppState,
    workspace: WeakEntity<Workspace>,
    config: GroupConfig,
    slots: Vec<Slot>,
    /// One drag state per seam, indices matching `config.dividers`.
    dividers: Vec<DividerState>,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// Whether the hosted children have been told which tab panel this
    /// group sits under; see [`composite::introduce_slots`].
    introduced: bool,
}

impl GroupPanel {
    pub fn new(
        state: AppState,
        workspace: WeakEntity<Workspace>,
        config: GroupConfig,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::restore(state, workspace, config, Vec::new(), cx)
    }

    /// Build with already-restored children, the layout-dump route in.
    /// The dump's children set the slot count, floored at a pair for a
    /// short or hand-edited one; seams that don't line up re-derive.
    pub fn restore(
        state: AppState,
        workspace: WeakEntity<Workspace>,
        mut config: GroupConfig,
        mut slots: Vec<Slot>,
        cx: &mut Context<Self>,
    ) -> Self {
        if slots.len() < 2 {
            slots.resize_with(2, || None);
        }
        config.dividers = normalized_dividers(&config, slots.len());
        let dividers = (1..slots.len()).map(|_| DividerState::default()).collect();
        GroupPanel {
            state,
            workspace,
            config,
            slots,
            dividers,
            focus: cx.focus_handle(),
            tab_panel: None,
            introduced: false,
        }
    }

    /// The hosted slots in split order, for the settings window's layout
    /// tree.
    pub fn slots(&self) -> &[Slot] {
        &self.slots
    }

    fn axis(&self) -> Axis {
        if self.config.stacked {
            Axis::Vertical
        } else {
            Axis::Horizontal
        }
    }

    fn set_slot(&mut self, ix: usize, slot: Slot, cx: &mut Context<Self>) {
        self.slots[ix] = slot;
        self.introduced = false;
        cx.notify();
    }

    /// Keep the pair era's `ratio` riding the first seam, so an older
    /// build reading this dump still splits where it was left.
    fn sync_ratio(&mut self) {
        if let Some(first) = self.config.dividers.first() {
            self.config.ratio = *first;
        }
    }

    /// Grow the split by an empty slot at the end, halving the last
    /// share to make its room.
    fn add_slot(&mut self, cx: &mut Context<Self>) {
        let last = self.config.dividers.last().copied().unwrap_or(0.0);
        self.config.dividers.push((last + 1.0) / 2.0);
        self.slots.push(None);
        self.dividers.push(DividerState::default());
        cx.notify();
    }

    /// Drop the empty slot at `ix`, its seam folding into a neighbor's
    /// share. A pair is the floor, and a filled slot leaves through its
    /// own menu first.
    fn remove_slot(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.slots.len() <= 2 || ix >= self.slots.len() || self.slots[ix].is_some() {
            return;
        }
        self.slots.remove(ix);
        let seam = ix.min(self.config.dividers.len() - 1);
        self.config.dividers.remove(seam);
        self.dividers.pop();
        self.sync_ratio();
        cx.notify();
    }

    /// Move slot `ix` one step toward either end; the shares stay where
    /// they are, the contents move.
    fn shift(&mut self, ix: usize, forward: bool, cx: &mut Context<Self>) {
        let other = if forward { ix + 1 } else { ix.wrapping_sub(1) };
        if ix >= self.slots.len() || other >= self.slots.len() {
            return;
        }
        self.slots.swap(ix, other);
        cx.notify();
    }

    /// Pin seam `ix` to `fraction`, held off its neighbors so no slot
    /// pinches shut. Crowd enough slots in and the seams simply have no
    /// room left to move.
    fn drag_seam(&mut self, ix: usize, fraction: f32, cx: &mut Context<Self>) {
        let seams = self.config.dividers.len();
        if ix >= seams {
            return;
        }
        let lo = if ix == 0 {
            0.0
        } else {
            self.config.dividers[ix - 1]
        } + SHARE_MIN;
        let hi = if ix + 1 == seams {
            1.0
        } else {
            self.config.dividers[ix + 1]
        } - SHARE_MIN;
        self.config.dividers[ix] = fraction.clamp(lo, hi.max(lo));
        self.sync_ratio();
        cx.notify();
    }

    /// One cell of the split: the child's view, or the empty add
    /// affordance, under the floating slot controls.
    fn cell(&self, ix: usize, cx: &mut Context<Self>) -> Div {
        let content = match &self.slots[ix] {
            // A child that serves its own content menu keeps the
            // right-click; for the rest the slot routes it to the hosting
            // tab panel's fallback menu with the child as its subject,
            // since the group opted the dock's own fallback out for its
            // whole body.
            Some(child) => composite::menu_routed_slot(child, &self.tab_panel, cx),
            None => {
                let weak = cx.entity().downgrade();
                composite::empty_slot(
                    ("group-add", ix),
                    self.state.clone(),
                    self.workspace.clone(),
                    move |panel, _, cx| {
                        if let Some(this) = weak.upgrade() {
                            this.update(cx, |this, cx| this.set_slot(ix, Some(panel), cx));
                        }
                    },
                )
            }
        };
        // A layout that ships as finished furniture drops the builder's
        // buttons; its slots are still swapped from the tree on the
        // Workspace settings page.
        let controls = if self.config.chrome.controls_hidden() {
            None
        } else if let Some(child) = self.slots[ix].clone() {
            let count = self.slots.len();
            let stacked = self.config.stacked;
            Some(composite::corner_controls().child(composite::slot_button(
                ("group-slot", ix),
                child,
                self.state.clone(),
                self.workspace.clone(),
                move |this: &mut Self, panel, cx| this.set_slot(ix, Some(panel), cx),
                move |this: &mut Self, cx| this.set_slot(ix, None, cx),
                // A pair swaps whole from the group's own menu; a longer
                // split reorders a slot at a time from here.
                move |menu, weak| {
                    if count <= 2 {
                        return menu;
                    }
                    let (back_label, back_icon) = if stacked {
                        (rox_i18n::t!("group-panel-move-up"), icons::ARROW_UP)
                    } else {
                        (rox_i18n::t!("composite-move-left"), icons::CHEVRON_LEFT)
                    };
                    let (fwd_label, fwd_icon) = if stacked {
                        (rox_i18n::t!("group-panel-move-down"), icons::ARROW_DOWN)
                    } else {
                        (rox_i18n::t!("composite-move-right"), icons::CHEVRON_RIGHT)
                    };
                    let back = weak.clone();
                    let forward = weak;
                    menu.item(
                        PopupMenuItem::new(back_label)
                            .icon(Icon::default().path(back_icon))
                            .disabled(ix == 0)
                            .on_click(move |_, _, cx| {
                                if let Some(this) = back.upgrade() {
                                    this.update(cx, |this, cx| this.shift(ix, false, cx));
                                }
                            }),
                    )
                    .item(
                        PopupMenuItem::new(fwd_label)
                            .icon(Icon::default().path(fwd_icon))
                            .disabled(ix + 1 >= count)
                            .on_click(move |_, _, cx| {
                                if let Some(this) = forward.upgrade() {
                                    this.update(cx, |this, cx| this.shift(ix, true, cx));
                                }
                            }),
                    )
                    .separator()
                },
                cx,
            )))
        } else if self.slots.len() > 2 {
            // An empty slot on a grown split can leave: the x drops the
            // hole and hands its share back.
            let weak = cx.entity().downgrade();
            Some(
                composite::corner_controls().child(
                    Button::new(("group-drop", ix))
                        .icon(Icon::default().path(icons::CLOSE))
                        .small()
                        .ghost()
                        .tooltip(rox_i18n::t!("group-panel-remove-slot"))
                        .on_click(move |_, _, cx| {
                            if let Some(this) = weak.upgrade() {
                                this.update(cx, |this, cx| this.remove_slot(ix, cx));
                            }
                        }),
                ),
            )
        } else {
            None
        };
        let cell = div()
            .relative()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .child(content)
            .children(controls);
        // The child's own size settings, applied where the split can still
        // act on them: a capped cell keeps its size and the seams spend
        // what's left on the other slots.
        composite::clamp_to_panel(cell, &self.slots[ix], cx)
    }

    fn body(&mut self, cx: &mut Context<Self>) -> Div {
        // Let the children reach this host from their own menus; the
        // dock never sees a hosted panel, so nothing else offers it.
        let group_title = rox_i18n::t!("group-panel-title");
        composite::report_hosted(
            self.slots.iter().flatten(),
            self.config.chrome.title.as_deref().unwrap_or(&group_title),
            cx,
        );

        let axis = self.axis();
        let seams = self.config.dividers.clone();
        let weak = cx.entity().downgrade();

        // Every slot sizes off its share as a flex basis rather than hard
        // shares of the span: a slot held to a size by its own settings
        // then gives the space it refuses back to its neighbors instead
        // of leaving a gap. A hosted child's own size cap applies along
        // the split, so a card of fixed-height panels reads at those
        // heights instead of stretching each to its share.
        let share = |cell: Div, basis: f32, cap: Option<gpui::Size<gpui::Pixels>>| {
            cell.map(|d| match axis {
                Axis::Horizontal => d.h_full(),
                Axis::Vertical => d.w_full(),
            })
            .when_some(cap, |d, cap| {
                let along = match axis {
                    Axis::Horizontal => cap.width,
                    Axis::Vertical => cap.height,
                };
                if along < gpui::Pixels::MAX {
                    match axis {
                        Axis::Horizontal => d.max_w(along),
                        Axis::Vertical => d.max_h(along),
                    }
                } else {
                    d
                }
            })
            .flex_basis(relative(basis))
            .flex_grow()
            .flex_shrink()
        };

        // The seams draw at the panel's own frame border width, so a
        // bordered group divides in the same stroke (and the same border
        // role color, which the panel's theme can recolor). A border
        // that differs side to side lends its widest, since a divider
        // is one line and has to pick. Borderless groups keep the 1px
        // hairline.
        let split = self
            .config
            .chrome
            .theme
            .border_sides(rox_core::settings::app_frame().border)
            .max()
            .clamp(1.0, DIVIDER_W);
        // While the resize lock holds the dividers are only the lines: no
        // resize cursor, no drag, the same lock the dock's own handles
        // follow.
        let live = !rox_dock::resize_locked();

        let count = self.slots.len();
        // Centered, for the run that refuses space: slots that fill make
        // this a no-op, but a group of capped panels clusters in the
        // middle of its span instead of packing toward the start.
        let mut row = div()
            .size_full()
            .flex()
            .justify_center()
            .map(|d| match axis {
                Axis::Horizontal => d.flex_row(),
                Axis::Vertical => d.flex_col(),
            });
        for ix in 0..count {
            let start = if ix == 0 { 0.0 } else { seams[ix - 1] };
            let end = if ix + 1 == count { 1.0 } else { seams[ix] };
            let cap = self.slots[ix].as_ref().map(|child| child.max_size(cx));
            row = row.child(share(self.cell(ix, cx), (end - start).max(0.0), cap));
            if ix + 1 < count {
                let line = div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .map(|d| match axis {
                        Axis::Horizontal => d.w(px(DIVIDER_W)).h_full(),
                        Axis::Vertical => d.h(px(DIVIDER_W)).w_full(),
                    })
                    .when(live, |d| {
                        d.map(|d| match axis {
                            Axis::Horizontal => d.cursor_col_resize(),
                            Axis::Vertical => d.cursor_row_resize(),
                        })
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                if let Some(seam) = this.dividers.get(ix) {
                                    seam.begin();
                                }
                                cx.notify();
                            }),
                        )
                    })
                    // The line follows the app's seams toggle the way the
                    // dock's handles do: off leaves the grab strip alone,
                    // so a flush look loses the group's lines too.
                    .child(
                        div()
                            .when(rox_core::settings::seams(), |d| d.bg(palette::border()))
                            .map(|d| match axis {
                                Axis::Horizontal => d.w(px(split)).h_full(),
                                Axis::Vertical => d.h(px(split)).w_full(),
                            }),
                    );
                row = row.child(line);
            }
        }

        let parent = (!self.config.chrome.controls_hidden()).then(|| {
            composite::parent_controls().child(composite::parent_button(
                rox_i18n::t!("group-panel-title"),
                cx,
            ))
        });
        div()
            .size_full()
            .relative()
            .bg(palette::bg_root())
            .track_focus(&self.focus)
            // The drag layer: records where the slots span painted and,
            // while a drag is live, keeps window-level handlers moving that
            // seam. No hitbox of its own, so it never eats the slots'
            // clicks.
            .child(
                canvas(
                    {
                        let states = self.dividers.clone();
                        move |bounds, _, _| {
                            for seam in &states {
                                seam.set_bounds(bounds);
                            }
                        }
                    },
                    {
                        let states = self.dividers.clone();
                        move |_, _, window, _| {
                            for (ix, seam) in states.iter().enumerate() {
                                let weak = weak.clone();
                                composite::divider_on_paint(
                                    seam,
                                    axis,
                                    window,
                                    move |fraction, cx| {
                                        if let Some(this) = weak.upgrade() {
                                            this.update(cx, |this, cx| {
                                                this.drag_seam(ix, fraction, cx)
                                            });
                                        }
                                    },
                                );
                            }
                        }
                    },
                )
                .absolute()
                .size_full(),
            )
            .child(row)
            .children(parent)
    }
}

impl PanelSettings for GroupPanel {
    fn composite(&self) -> bool {
        true
    }

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
}

impl EventEmitter<PanelEvent> for GroupPanel {}

impl Focusable for GroupPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for GroupPanel {
    fn panel_name(&self) -> &'static str {
        "group"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("group-panel-title"),
        )
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.config.chrome.title.clone().map(SharedString::from)
    }

    fn locked(&self, _cx: &App) -> bool {
        self.config.chrome.locked
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }

    fn content_context_menu(&self, _cx: &App) -> bool {
        true
    }

    fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        for slot in self.slots.iter().flatten() {
            slot.set_active(active, window, cx);
        }
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

    fn dump(&self, cx: &App) -> rox_dock::PanelState {
        let mut state = rox_dock::PanelState::new(self);
        state.info = rox_dock::PanelInfo::panel(
            serde_json::to_value(self.config.clone()).unwrap_or(serde_json::Value::Null),
        );
        state.children = composite::dump_slots(&self.slots, cx);
        state
    }

    fn on_added_to(
        &mut self,
        tab_panel: WeakEntity<TabPanel>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tab_panel = Some(tab_panel.clone());
        self.introduced = false;
        self.state
            .tab_hosts
            .update(cx, |hosts, _| hosts.report(tab_panel));
    }

    fn on_removed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tab_panel = None;
        self.introduced = false;
        for child in self.slots.iter().flatten() {
            child.on_removed(window, cx);
        }
    }

    fn dropdown_menu(
        &mut self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        // The toggle names the arrangement a click lands on, not the
        // current one.
        let (flip_label, flip_icon) = if self.config.stacked {
            (
                rox_i18n::t!("group-panel-split-side-by-side"),
                icons::MOVE_HORIZONTAL,
            )
        } else {
            (
                rox_i18n::t!("group-panel-split-stacked"),
                icons::MOVE_VERTICAL,
            )
        };
        let flip = cx.entity().downgrade();
        let add = cx.entity().downgrade();
        let menu = menu
            .item(
                PopupMenuItem::new(flip_label)
                    .icon(Icon::default().path(flip_icon))
                    .on_click(move |_, _, cx| {
                        if let Some(this) = flip.upgrade() {
                            this.update(cx, |this, cx| {
                                this.config.stacked = !this.config.stacked;
                                cx.notify();
                            });
                        }
                    }),
            )
            .item(
                PopupMenuItem::new(rox_i18n::t!("group-panel-add-slot"))
                    .icon(Icon::default().path(icons::PLUS))
                    .on_click(move |_, _, cx| {
                        if let Some(this) = add.upgrade() {
                            this.update(cx, |this, cx| this.add_slot(cx));
                        }
                    }),
            );
        // A pair swaps whole; a longer split reorders a slot at a time
        // from the slot menus.
        let menu = if self.slots.len() == 2 {
            let swap = cx.entity().downgrade();
            menu.item(
                PopupMenuItem::new(rox_i18n::t!("group-panel-swap-panels"))
                    .icon(Icon::default().path(icons::REFRESH_CW))
                    .on_click(move |_, _, cx| {
                        if let Some(this) = swap.upgrade() {
                            this.update(cx, |this, cx| {
                                this.slots.swap(0, 1);
                                cx.notify();
                            });
                        }
                    }),
            )
        } else {
            menu
        };
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

impl Render for GroupPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        composite::introduce_slots(
            self.slots.iter().flatten(),
            &self.tab_panel,
            &mut self.introduced,
            window,
            cx,
        );
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(cx))
    }
}
