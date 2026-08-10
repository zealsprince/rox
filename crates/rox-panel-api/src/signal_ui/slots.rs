//! The shader slot list, shared by every surface that fills slots.
//!
//! The companion to [`routes`](super::routes), and split off the same way:
//! three windows show the same sixteen rows over a different config, so the
//! difference is a borrowed slice and one write-back closure rather than a
//! host trait. The Shader panel's Bindings page, a panel's Shader page and
//! the app's Overlay Shader section all wear this.
//!
//! Every slot the shader can read gets a row whether anything feeds it or
//! not - that's what says where a value lands in the WGSL. A slot a route
//! feeds shows the live value it's getting; one nothing feeds is a hand-set
//! knob, typed or dragged, which is how a shader's named parameters get
//! exposed without a signal in sight.

use std::sync::Arc;

use gpui::{div, prelude::*, px, svg, Context, Div};

use rox_viz::signal::{Route, SignalHub};

use crate::panel::shader::{
    seed_manual, slot_accessor, slot_label, target_slot, SlotTargets, SLOTS,
};
use crate::panel::{self, ScrubState, ValueEdit};
use rox_design::assets::icons;
use rox_design::{palette, tokens};

/// How a host takes one hand-set slot edit. Same shape as
/// [`RouteMutate`](super::routes::RouteMutate) and for the same reason: the
/// list never touches the config it renders, so a panel field, a chrome
/// write and the settings file plus a live driver all plug in the same way.
///
/// The host is expected to notify.
pub type SlotSet<P> = Arc<dyn Fn(&mut P, usize, f32, &mut Context<P>)>;

/// One host's slots as they reach the shader: what feeds them and how a
/// hand-set value lands.
///
/// `labels` is the shader's own slot names where it declares them
/// (`// @slot 0: bass`); a host with nothing to say passes an empty slice
/// and the slots read by number. `scrubs` holds one drag state per slot,
/// sized [`SLOTS`] by the host - a slot without one falls back to a
/// readout, so a short list costs a knob rather than a panic.
pub struct SlotList<'a, P: 'static> {
    pub hub: &'a Arc<SignalHub>,
    pub routes: &'a [Route],
    pub manual: &'a [(u8, f32)],
    pub labels: &'a [Option<String>],
    pub value_edit: &'a ValueEdit,
    pub scrubs: &'a [ScrubState],
    pub set: SlotSet<P>,
}

impl<P: 'static> SlotList<'_, P> {
    /// The rows, for whatever section the host hangs them under. Values are
    /// resolved here rather than passed in, so every surface reads what
    /// actually reaches the shader this frame instead of what was set.
    pub fn render(self, cx: &mut Context<P>) -> Div {
        let mut resolved = SlotTargets::default();
        seed_manual(&mut resolved, self.manual);
        super::apply_routes(self.routes, self.hub, &mut resolved);

        let mut list = div().flex().flex_col().gap(tokens::SPACE_MD);
        for slot in 0..SLOTS {
            let value = resolved.slots.get(slot).copied().unwrap_or(0.0);
            let routed = self
                .routes
                .iter()
                .any(|route| route.enabled && target_slot(&route.target) == Some(slot));
            let control = match (routed, self.scrubs.get(slot)) {
                (false, Some(scrub)) => {
                    let set = self.set.clone();
                    panel::value_slider_edit(
                        scrub,
                        self.value_edit,
                        value,
                        format!("{value:.2}"),
                        format!("{value:.2}"),
                        |typed| typed,
                        move |this: &mut P, fraction, cx| set(this, slot, fraction, cx),
                        cx,
                    )
                }
                _ => readout(value),
            };
            list = list.child(panel::setting_row_dyn(
                slot_label(self.labels, slot),
                Some(slot_accessor(slot).into()),
                control,
            ));
        }
        list
    }
}

/// A routed slot's live value. While a route feeds the slot, the route is
/// the whole value, so what belongs here is a readout rather than a
/// control; the unrouted slots get the hand-set slider instead. The signal
/// glyph up front is what says "connected" at a glance against the sliders
/// around it.
fn readout(value: f32) -> Div {
    const BAR: f32 = 64.0;
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(tokens::SPACE_XS)
        .child(
            svg()
                .path(icons::AUDIO_WAVEFORM)
                .size(px(12.))
                .flex_none()
                .text_color(palette::accent()),
        )
        .child(
            div()
                .w(px(28.))
                .text_xs()
                .text_color(palette::text_faint())
                .child(format!("{value:.2}")),
        )
        .child(
            div()
                .w(px(BAR))
                .h(px(6.))
                .rounded(px(3.))
                .bg(palette::bg_control())
                .child(
                    div()
                        .h_full()
                        .w(px(BAR * value.clamp(0.0, 1.0)))
                        .rounded(px(3.))
                        .bg(palette::accent()),
                ),
        )
}
