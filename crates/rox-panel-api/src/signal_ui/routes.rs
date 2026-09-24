//! The shader route editor, shared by every surface that fills slots.
//!
//! Hosts differ only in where routes are stored and how a write lands, so
//! [`RouteEditor`] reads a borrowed slice and writes back through one
//! [`RouteMutate`] closure. No trait: the settings window can't satisfy one
//! without duplicating state it doesn't own.
//!
//! The row fold lives in [`RouteEditState`] on the host window, never in
//! config: which row you left open isn't a setting.

use std::collections::HashSet;
use std::sync::Arc;

use gpui::{Context, Div, MouseButton, SharedString, div, prelude::*, px, svg};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};

use rox_viz::signal::{Route, Signal, SignalHub};

use crate::panel::shader::{SLOTS, slot_label, slot_target, target_slot};
use crate::panel::{self, ScrubState, ValueEdit};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_kit::ui::{self as settings_ui, OVER};

use super::{gate_mark, meter};

/// How a host takes one edit to its route list. The editor never touches
/// the routes it renders. Its own listeners notify, so a plain write is
/// enough.
pub type RouteMutate<P> = Arc<dyn Fn(&mut P, &mut dyn FnMut(&mut Vec<Route>), &mut Context<P>)>;

/// The editor's ephemeral state, embedded in the hosting view. Call
/// [`sync`](Self::sync) at the top of a render.
#[derive(Default)]
pub struct RouteEditState {
    scrubs: Vec<(ScrubState, ScrubState)>,
    open: HashSet<usize>,
}

impl RouteEditState {
    pub fn sync(&mut self, count: usize) {
        if self.scrubs.len() != count {
            self.scrubs.resize_with(count, Default::default);
        }
        self.open.retain(|index| *index < count);
    }

    fn expand_only(&mut self, index: usize) {
        self.open.clear();
        self.open.insert(index);
    }

    /// Shift the fold and sliders up past a deleted row.
    fn removed(&mut self, index: usize) {
        self.open = self
            .open
            .iter()
            .filter(|open| **open != index)
            .map(|open| if *open > index { open - 1 } else { *open })
            .collect();
        if index < self.scrubs.len() {
            self.scrubs.remove(index);
        }
    }

    fn toggle(&mut self, index: usize) {
        if !self.open.remove(&index) {
            self.open.insert(index);
        }
    }

    fn is_open(&self, index: usize) -> bool {
        self.open.contains(&index)
    }
}

/// The field's label, and whether it's a placeholder: a signal the pool no
/// longer has reads as a prompt.
fn pick_label(route: &Route, pool: &[Signal]) -> (String, bool) {
    match pool.iter().find(|signal| signal.id == route.signal) {
        Some(signal) => (signal.label(), false),
        None => (rox_i18n::t!("route-pick-signal").to_string(), true),
    }
}

fn ride_summary(route: &Route, pool: &[Signal]) -> String {
    match pool.iter().find(|signal| signal.id == route.signal) {
        Some(signal) => signal.label(),
        None => rox_i18n::t!("route-no-signal").to_string(),
    }
}

/// The lowest slot no route fills yet, where a fresh route starts.
/// Duplicates stay legal through the slot picker; the last resolved wins.
pub fn next_free_slot(routes: &[Route]) -> Option<usize> {
    let taken: Vec<usize> = routes
        .iter()
        .filter_map(|route| target_slot(&route.target))
        .collect();
    (0..SLOTS).find(|slot| !taken.contains(slot))
}

/// `id` scopes the rows' element ids, so two editors in one window never
/// share a dropdown's state. `labels` are the shader's `// @slot 0: bass`
/// names; empty reads by number.
pub struct RouteEditor<'a, P: 'static> {
    pub id: &'static str,
    pub hub: &'a Arc<SignalHub>,
    pub routes: &'a [Route],
    pub labels: &'a [Option<String>],
    pub value_edit: &'a ValueEdit,
    pub ui: &'a RouteEditState,
    pub ui_mut: fn(&mut P) -> &mut RouteEditState,
    pub mutate: RouteMutate<P>,
}

impl<P: 'static> RouteEditor<'_, P> {
    pub fn add_button(&self, cx: &mut Context<P>) -> settings_ui::SmallButton {
        let full = next_free_slot(self.routes).is_none();
        let signal = self.hub.pool().first().map(|signal| signal.id);
        let mutate = self.mutate.clone();
        let ui_mut = self.ui_mut;
        settings_ui::small_button(
            rox_i18n::t!("route-add"),
            icons::PLUS,
            full,
            cx.listener(move |this: &mut P, _, _, cx| {
                let mut landed = None;
                mutate(
                    this,
                    &mut |routes| {
                        let slot = next_free_slot(routes).unwrap_or(0);
                        routes.push(Route {
                            enabled: true,
                            signal: signal.unwrap_or(0),
                            target: slot_target(slot),
                            from: 0.0,
                            to: 1.0,
                        });
                        landed = Some(routes.len() - 1);
                    },
                    cx,
                );
                if let Some(index) = landed {
                    ui_mut(this).sync(index + 1);
                    ui_mut(this).expand_only(index);
                }
                cx.notify();
            }),
        )
    }

    pub fn list(&self, cx: &mut Context<P>) -> Div {
        let mut list = div().flex().flex_col().gap(tokens::SPACE_MD);
        if self.routes.is_empty() {
            list = list.child(note(
                "Nothing routed: every slot reads zero until a route feeds it a signal. \
                 A shader can name its slots with `// @slot 0: bass` comments and the \
                 names show up here."
                    .into(),
            ));
        }
        for index in 0..self.routes.len() {
            list = list.child(self.row(index, cx));
        }
        if next_free_slot(self.routes).is_none() {
            list = list.child(note(
                "All sixteen slots are routed. Point one somewhere else or delete it to \
                 add another."
                    .into(),
            ));
        }
        list
    }

    fn row(&self, index: usize, cx: &mut Context<P>) -> Div {
        let Some(route) = self.routes.get(index) else {
            return div();
        };
        let pool = self.hub.pool();
        let slot = target_slot(&route.target);
        let open = self.ui.is_open(index);
        let signal = pool.iter().find(|signal| signal.id == route.signal);
        let ui_mut = self.ui_mut;
        let mutate = self.mutate.clone();

        // Only the summary takes the fold's click, or a press on its way to
        // the trash would fold the row shut under it.
        let summary = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .cursor_pointer()
            .child(
                svg()
                    .path(if open {
                        icons::CHEVRON_DOWN
                    } else {
                        icons::CHEVRON_RIGHT
                    })
                    .size(px(12.))
                    .flex_none()
                    .text_color(palette::text_muted()),
            )
            .child(div().text_xs().child(match slot {
                Some(slot) => slot_label(self.labels, slot),
                None => rox_i18n::t!("route-unrouted").to_string(),
            }))
            .child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(ride_summary(route, &pool)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this: &mut P, _, _, cx| {
                    ui_mut(this).toggle(index);
                    cx.notify();
                }),
            );

        let header = settings_ui::block_header(
            summary,
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_XS)
                .child(panel::toggle(
                    route.enabled,
                    {
                        let mutate = mutate.clone();
                        move |this: &mut P, on, cx| {
                            mutate(
                                this,
                                &mut |routes| {
                                    if let Some(route) = routes.get_mut(index) {
                                        route.enabled = on;
                                    }
                                },
                                cx,
                            );
                            cx.notify();
                        }
                    },
                    cx,
                ))
                .child(settings_ui::icon_button(
                    icons::TRASH,
                    false,
                    cx.listener({
                        let mutate = mutate.clone();
                        move |this: &mut P, _, _, cx| {
                            mutate(
                                this,
                                &mut |routes| {
                                    if index < routes.len() {
                                        routes.remove(index);
                                    }
                                },
                                cx,
                            );
                            ui_mut(this).removed(index);
                            cx.notify();
                        }
                    }),
                )),
        );

        let mut block = div().flex().flex_col().gap(tokens::SPACE_SM).child(header);
        if open {
            block = block
                .child(panel::setting_row(
                    rox_i18n::t!("route-slot"),
                    Some(rox_i18n::t!("route-slot.description")),
                    self.slot_field(index, slot, cx),
                ))
                .child(self.signal_row(index, &pool, cx));
            if let Some(signal) = signal {
                block = block.child(meter(
                    self.hub.clone(),
                    signal.id,
                    palette::accent(),
                    gate_mark(signal),
                ));
            }
            block = block.child(spans(self, index, route, cx));
        }
        settings_ui::nested(block)
    }

    fn slot_field(&self, index: usize, slot: Option<usize>, cx: &mut Context<P>) -> Div {
        let label = match slot {
            Some(slot) => slot_label(self.labels, slot),
            None => rox_i18n::t!("route-pick-slot").to_string(),
        };
        let names: Vec<String> = (0..SLOTS)
            .map(|option| slot_label(self.labels, option))
            .collect();
        let weak = cx.entity().downgrade();
        let mutate = self.mutate.clone();
        div().child(
            settings_ui::select_field(
                SharedString::from(format!("{}-slot-{index}", self.id)),
                label,
                slot.is_none(),
            )
            .dropdown_menu(move |mut menu, _, _| {
                for (option, name) in names.iter().enumerate() {
                    let (host, mutate) = (weak.clone(), mutate.clone());
                    menu = menu.item(
                        PopupMenuItem::new(name.clone())
                            .checked(slot == Some(option))
                            .on_click(move |_, _, cx| {
                                let Some(host) = host.upgrade() else {
                                    return;
                                };
                                host.update(cx, |this: &mut P, cx| {
                                    mutate(
                                        this,
                                        &mut |routes| {
                                            if let Some(route) = routes.get_mut(index) {
                                                route.target = slot_target(option);
                                            }
                                        },
                                        cx,
                                    );
                                    cx.notify();
                                });
                            }),
                    );
                }
                menu
            }),
        )
    }

    /// An empty pool gets no dead control, just a pointer to the Signals
    /// window.
    fn signal_row(&self, index: usize, pool: &[Signal], cx: &mut Context<P>) -> Div {
        let Some(route) = self.routes.get(index) else {
            return div();
        };
        if pool.is_empty() {
            return div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(panel::setting_row_dyn(
                    rox_i18n::t!("route-signal"),
                    Some(rox_i18n::t!("route-slot-signal-description")),
                    div(),
                ))
                .child(note(rox_i18n::t!("route-no-signals-yet")))
                .child(div().child(settings_ui::small_button(
                    rox_i18n::t!("route-open-signals"),
                    icons::AUDIO_WAVEFORM,
                    false,
                    |_, _, cx| crate::openers::signals_window(cx),
                )));
        }
        let current = route.signal;
        let (label, placeholder) = pick_label(route, pool);
        let options: Vec<(u64, String)> = pool
            .iter()
            .map(|signal| (signal.id, signal.label()))
            .collect();
        let weak = cx.entity().downgrade();
        let mutate = self.mutate.clone();
        let field = settings_ui::select_field(
            SharedString::from(format!("{}-signal-{index}", self.id)),
            label,
            placeholder,
        )
        .dropdown_menu(move |mut menu, _, _| {
            for (id, label) in &options {
                let (id, host, mutate) = (*id, weak.clone(), mutate.clone());
                menu = menu.item(
                    PopupMenuItem::new(label.clone())
                        .checked(id == current)
                        .on_click(move |_, _, cx| {
                            let Some(host) = host.upgrade() else {
                                return;
                            };
                            host.update(cx, |this: &mut P, cx| {
                                mutate(
                                    this,
                                    &mut |routes| {
                                        if let Some(route) = routes.get_mut(index) {
                                            route.signal = id;
                                        }
                                    },
                                    cx,
                                );
                                cx.notify();
                            });
                        }),
                );
            }
            menu.separator().item(
                PopupMenuItem::new(rox_i18n::t!("route-create-signal"))
                    .on_click(|_, _, cx| crate::openers::signals_window(cx)),
            )
        });
        let mut block = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(panel::setting_row_dyn(
                rox_i18n::t!("route-signal"),
                Some(rox_i18n::t!("route-slot-signal-description")),
                field,
            ));
        if placeholder {
            block = block.child(note(rox_i18n::t!("route-slot-signal-gone")));
        }
        block
    }
}

/// The span this route sweeps between silence and full signal. Per route,
/// so one signal can pull a slot all the way and nudge another.
fn spans<P: 'static>(
    editor: &RouteEditor<P>,
    index: usize,
    route: &Route,
    cx: &mut Context<P>,
) -> Div {
    let Some((from_scrub, to_scrub)) = editor.ui.scrubs.get(index) else {
        return div();
    };
    let from = route.from.clamp(0.0, OVER);
    let to = route.to.clamp(0.0, OVER);
    let quiet = editor.mutate.clone();
    let loud = editor.mutate.clone();
    div()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_SM)
        .child(panel::setting_row(
            rox_i18n::t!("route-quiet"),
            Some(rox_i18n::t!("route-slot-quiet-description")),
            panel::value_slider_edit_over(
                from_scrub,
                editor.value_edit,
                from,
                format!("{}%", (from * 100.0).round() as i32),
                format!("{}", (from * 100.0).round() as i32),
                OVER,
                |v| v / 100.0,
                move |this: &mut P, fraction, cx| {
                    quiet(
                        this,
                        &mut |routes| {
                            if let Some(route) = routes.get_mut(index) {
                                route.from = fraction.clamp(0.0, OVER);
                            }
                        },
                        cx,
                    );
                    cx.notify();
                },
                cx,
            ),
        ))
        .child(panel::setting_row(
            rox_i18n::t!("route-loud"),
            Some(rox_i18n::t!("route-slot-loud-description")),
            panel::value_slider_edit_over(
                to_scrub,
                editor.value_edit,
                to,
                format!("{}%", (to * 100.0).round() as i32),
                format!("{}", (to * 100.0).round() as i32),
                OVER,
                |v| v / 100.0,
                move |this: &mut P, fraction, cx| {
                    loud(
                        this,
                        &mut |routes| {
                            if let Some(route) = routes.get_mut(index) {
                                route.to = fraction.clamp(0.0, OVER);
                            }
                        },
                        cx,
                    );
                    cx.notify();
                },
                cx,
            ),
        ))
}

fn note(text: SharedString) -> Div {
    div()
        .text_xs()
        .text_color(palette::text_muted())
        .child(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    use rox_viz::signal::Source;

    fn route(target: &str) -> Route {
        Route {
            target: target.to_string(),
            ..Route::default()
        }
    }

    fn signal(id: u64, name: &str) -> Signal {
        Signal {
            id,
            name: name.to_string(),
            source: Source::Level,
            ..Signal::default()
        }
    }

    #[test]
    fn the_field_names_what_the_route_is_on() {
        // Another test in this binary flips the locale under the lock.
        let _guard = rox_i18n::LOCALE_TEST_LOCK.lock().unwrap();
        rox_i18n::set_locale(Some("en-CA"));
        let pool = vec![signal(1, "Kick")];
        let mut riding = route("slot0");
        riding.signal = 1;
        assert_eq!(pick_label(&riding, &pool), ("Kick".to_string(), false));
        assert_eq!(ride_summary(&riding, &pool), "Kick");

        let mut orphan = route("slot1");
        orphan.signal = 99;
        assert_eq!(pick_label(&orphan, &pool), ("Pick a signal".into(), true));
        assert_eq!(ride_summary(&orphan, &pool), "no signal");
    }

    #[test]
    fn next_free_slot_takes_the_lowest_gap() {
        assert_eq!(next_free_slot(&[]), Some(0));
        assert_eq!(next_free_slot(&[route("slot0")]), Some(1));
        assert_eq!(next_free_slot(&[route("slot1"), route("slot0")]), Some(2));
        assert_eq!(
            next_free_slot(&[route("slot0"), route("slot2"), route("slot1")]),
            Some(3)
        );
        // Targets that name no slot at all don't take one.
        assert_eq!(
            next_free_slot(&[route("nowhere"), route("slot16")]),
            Some(0)
        );
        // Duplicates take one slot between them, not two.
        assert_eq!(next_free_slot(&[route("slot0"), route("slot0")]), Some(1));
    }

    #[test]
    fn every_slot_taken_leaves_nothing_to_add() {
        let full: Vec<Route> = (0..SLOTS).map(|slot| route(&slot_target(slot))).collect();
        assert_eq!(next_free_slot(&full), None);
        let mut freed = full.clone();
        freed.remove(4);
        assert_eq!(next_free_slot(&freed), Some(4));
    }

    #[test]
    fn fold_state_follows_the_list() {
        let mut ui = RouteEditState::default();
        ui.sync(3);
        assert_eq!(ui.scrubs.len(), 3);
        ui.expand_only(2);
        assert!(ui.is_open(2) && !ui.is_open(0));
        ui.toggle(0);
        assert!(ui.is_open(0) && ui.is_open(2));
        ui.toggle(0);
        assert!(!ui.is_open(0));
        ui.removed(1);
        assert_eq!(ui.scrubs.len(), 2);
        assert!(ui.is_open(1) && !ui.is_open(2));
        ui.removed(1);
        assert!(!ui.is_open(0) && !ui.is_open(1));
        ui.expand_only(0);
        ui.sync(0);
        assert!(!ui.is_open(0));
        assert!(ui.scrubs.is_empty());
    }
}
