//! The shader slot list, shared by every surface that fills slots.
//!
//! The companion to [`routes`](super::routes), split the same way: hosts
//! differ by a borrowed slice and one write-back closure, not a trait.
//!
//! Every slot gets a row whether anything drives it or not. A routed slot
//! shows its live value; an unrouted one is a hand-set knob, which is how a
//! shader's named parameters get exposed without a signal.

use std::sync::Arc;

use gpui::{Context, Div, div, prelude::*, px, svg};

use rox_viz::signal::{Route, SignalHub};

use crate::panel::shader::{
    SLOTS, SlotTargets, seed_manual, slot_accessor, slot_label, target_slot,
};
use crate::panel::{self, ScrubState, ValueEdit};
use rox_design::assets::icons;
use rox_design::{palette, tokens};

/// How a host takes one hand-set slot edit. The list never touches the
/// config it renders. The host is expected to notify.
pub type SlotSet<P> = Arc<dyn Fn(&mut P, usize, f32, &mut Context<P>)>;

/// One host's slots as the shader sees them.
///
/// `labels` are the shader's `// @slot 0: bass` names; empty reads by
/// number. `scrubs` should be sized [`SLOTS`]; a slot without one falls
/// back to a readout rather than panicking.
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
    /// Values resolve here rather than being passed in, so every surface
    /// shows what the shader gets this frame, not what was set.
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

/// A routed slot's live value: a readout, since the route is the whole value.
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
