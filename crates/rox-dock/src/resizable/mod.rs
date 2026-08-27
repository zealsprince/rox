use std::ops::Range;

use gpui::{
    Along, App, Axis, Bounds, Context, ElementId, EventEmitter, IsZero, Pixels, Window, px,
};

use gpui_component::PixelsExt;

mod panel;
mod resize_handle;
pub use panel::*;
pub(crate) use resize_handle::*;
// rox addition: the seam visibility flag, the one piece of the handle the
// host app drives.
pub use resize_handle::{seams, set_seams};

// Small enough that a panel can shrink to a tab bar plus one line of
// controls; upstream ships 100, far too coarse for compact strips. Panels
// that need more room say so through `Panel::min_size`.
pub const PANEL_MIN_SIZE: Pixels = px(40.);

/// Create a [`ResizablePanelGroup`] with horizontal resizing
pub fn h_resizable(id: impl Into<ElementId>) -> ResizablePanelGroup {
    ResizablePanelGroup::new(id).axis(Axis::Horizontal)
}

/// Create a [`ResizablePanelGroup`] with vertical resizing
pub fn v_resizable(id: impl Into<ElementId>) -> ResizablePanelGroup {
    ResizablePanelGroup::new(id).axis(Axis::Vertical)
}

/// Create a [`ResizablePanel`].
pub fn resizable_panel() -> ResizablePanel {
    ResizablePanel::new()
}

/// State for a [`ResizablePanel`]
#[derive(Debug, Clone)]
pub struct ResizableState {
    /// The `axis` will sync to actual axis of the ResizablePanelGroup in use.
    axis: Axis,
    panels: Vec<ResizablePanelState>,
    /// The authoritative shares: what each panel is owed, set when a panel is
    /// inserted with an explicit size, captured from the first paint when it
    /// wasn't (`None` until then), and rewritten by a drag. Container fits
    /// solve display sizes *from* these and never write back, so squeezing a
    /// layout through a small window and restoring it is lossless.
    weights: Vec<Option<Pixels>>,
    /// The solved display sizes for the current container: the weights scaled
    /// to fit, with each panel's min/max honored. The flex layout renders
    /// from these.
    sizes: Vec<Pixels>,
    pub(crate) resizing_panel_ix: Option<usize>,
    bounds: Bounds<Pixels>,
}

impl Default for ResizableState {
    fn default() -> Self {
        Self {
            axis: Axis::Horizontal,
            panels: vec![],
            weights: vec![],
            sizes: vec![],
            resizing_panel_ix: None,
            bounds: Bounds::default(),
        }
    }
}

impl ResizableState {
    /// Get the size of the panels.
    pub fn sizes(&self) -> &Vec<Pixels> {
        &self.sizes
    }

    /// The panel shares to persist: the weights, with panels that never got
    /// one (not painted yet) falling back to their solved size. Dumps read
    /// this rather than [`sizes`](Self::sizes) so a layout saved while the
    /// window is squeezed still records the real proportions.
    pub fn weights(&self) -> Vec<Pixels> {
        self.weights
            .iter()
            .zip(&self.sizes)
            .map(|(w, s)| w.unwrap_or(*s))
            .collect()
    }

    pub(crate) fn insert_panel(
        &mut self,
        size: Option<Pixels>,
        ix: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        let panel_state = ResizablePanelState {
            size,
            ..Default::default()
        };

        let weight = size;
        let size = size.unwrap_or(PANEL_MIN_SIZE);

        // We make sure that the size always sums up to the container size
        // by reducing the size of all other panels first.
        let container_size = self.container_size().max(px(1.));
        let total_leftover_size = (container_size - size).max(px(1.));

        for (i, panel) in self.panels.iter_mut().enumerate() {
            let ratio = self.sizes[i] / container_size;
            self.sizes[i] = total_leftover_size * ratio;
            panel.size = Some(self.sizes[i]);
        }

        // A structural change re-shares the split at the live container, so
        // the solved sizes become the new weights. During a layout rebuild the
        // container has no bounds yet and the loop above was a no-op, so this
        // keeps the dump's sizes as the weights verbatim. Uncaptured panels
        // stay `None` and get their weight from their first paint.
        for (w, s) in self.weights.iter_mut().zip(&self.sizes) {
            if w.is_some() {
                *w = Some(*s);
            }
        }

        if let Some(ix) = ix {
            self.panels.insert(ix, panel_state);
            self.weights.insert(ix, weight);
            self.sizes.insert(ix, size);
        } else {
            self.panels.push(panel_state);
            self.weights.push(weight);
            self.sizes.push(size);
        };

        cx.notify();
    }

    pub(crate) fn sync_panels_count(
        &mut self,
        axis: Axis,
        panels_count: usize,
        cx: &mut Context<Self>,
    ) {
        let mut changed = self.axis != axis;
        self.axis = axis;

        if panels_count > self.panels.len() {
            let diff = panels_count - self.panels.len();
            self.panels
                .extend(vec![ResizablePanelState::default(); diff]);
            // No weight yet: the first paint captures the rendered size.
            self.weights.extend(vec![None; diff]);
            self.sizes.extend(vec![PANEL_MIN_SIZE; diff]);
            changed = true;
        }

        if panels_count < self.panels.len() {
            self.panels.truncate(panels_count);
            self.weights.truncate(panels_count);
            self.sizes.truncate(panels_count);
            changed = true;
        }

        if changed {
            // We need to make sure the total size is in line with the container size.
            self.adjust_to_container_size(cx);
        }
    }

    pub(crate) fn update_panel_size(
        &mut self,
        panel_ix: usize,
        bounds: Bounds<Pixels>,
        size_range: Range<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let size = bounds.size.along(self.axis);
        // A panel created without an explicit size gets its weight from its
        // first paint: whatever the flex layout gave it becomes its share
        // from here on. Upstream keyed this off the size still being exactly
        // PANEL_MIN_SIZE, which misfired on panels whose real share happens
        // to equal it; the explicit `None` weight is the reliable marker.
        if self.weights[panel_ix].is_none() {
            self.weights[panel_ix] = Some(size);
            self.sizes[panel_ix] = size;
            self.panels[panel_ix].size = Some(size);
        }
        self.panels[panel_ix].bounds = bounds;
        self.panels[panel_ix].size_range = size_range;
        cx.notify();
    }

    /// Move a panel's state to another index, its size moving with it, so
    /// an app-level reorder of a stack's children keeps every split width
    /// with its panel.
    pub(crate) fn move_panel(&mut self, from_ix: usize, to_ix: usize, cx: &mut Context<Self>) {
        if from_ix == to_ix || from_ix >= self.panels.len() || to_ix >= self.panels.len() {
            return;
        }
        let panel = self.panels.remove(from_ix);
        self.panels.insert(to_ix, panel);
        let weight = self.weights.remove(from_ix);
        self.weights.insert(to_ix, weight);
        let size = self.sizes.remove(from_ix);
        self.sizes.insert(to_ix, size);
        cx.notify();
    }

    pub(crate) fn remove_panel(&mut self, panel_ix: usize, cx: &mut Context<Self>) {
        self.panels.remove(panel_ix);
        self.weights.remove(panel_ix);
        self.sizes.remove(panel_ix);
        if let Some(resizing_panel_ix) = self.resizing_panel_ix {
            if resizing_panel_ix > panel_ix {
                self.resizing_panel_ix = Some(resizing_panel_ix - 1);
            }
        }
        self.adjust_to_container_size(cx);
    }

    pub(crate) fn replace_panel(
        &mut self,
        panel_ix: usize,
        panel: ResizablePanelState,
        cx: &mut Context<Self>,
    ) {
        let old_size = self.sizes[panel_ix];

        self.panels[panel_ix] = panel;
        self.sizes[panel_ix] = old_size;
        self.adjust_to_container_size(cx);
    }

    pub(crate) fn clear(&mut self) {
        self.panels.clear();
        self.weights.clear();
        self.sizes.clear();
    }

    #[inline]
    pub(crate) fn container_size(&self) -> Pixels {
        self.bounds.size.along(self.axis)
    }

    pub(crate) fn done_resizing(&mut self, cx: &mut Context<Self>) {
        self.resizing_panel_ix = None;
        cx.emit(ResizablePanelEvent::Resized);
    }

    fn panel_size_range(&self, ix: usize) -> Range<Pixels> {
        let Some(panel) = self.panels.get(ix) else {
            return PANEL_MIN_SIZE..Pixels::MAX;
        };

        panel.size_range.clone()
    }

    fn sync_real_panel_sizes(&mut self, _: &App) {
        for (i, panel) in self.panels.iter().enumerate() {
            self.sizes[i] = panel.bounds.size.along(self.axis);
        }
    }

    /// The `ix`` is the index of the panel to resize,
    /// and the `size` is the new size for the panel.
    ///
    /// A drag moves the boundary between panel `ix` and `ix + 1`. The side it
    /// moves into grows and the other side shrinks by the same amount, each
    /// cascading outward from the handle: the panel nearest the handle takes
    /// the change first, then the next, and so on. Growth respects every
    /// panel's max and shrink respects every panel's min, so a pinned panel
    /// (min == max) can't change size but is pushed along while the resizable
    /// panels beyond it absorb the drag. The move is capped at whatever the
    /// growing side can take and the shrinking side can give, so nothing ever
    /// crosses a bound and the total stays put.
    fn resize_panel(&mut self, ix: usize, size: Pixels, _: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.sizes.len().saturating_sub(1) {
            return;
        }
        self.sync_real_panel_sizes(cx);
        let old = self.sizes.clone();
        let n = old.len();

        let delta = size - old[ix];
        if delta == px(0.) {
            return;
        }

        let min_of = |i: usize| self.panel_size_range(i).start;
        let max_of = |i: usize| self.panel_size_range(i).end;
        let grow_room = |i: usize| (max_of(i) - old[i]).max(px(0.));
        let shrink_room = |i: usize| (old[i] - min_of(i)).max(px(0.));

        // The two clusters either side of the boundary, and which one grows.
        // Positive delta drags the boundary toward the end: the left cluster
        // (through `ix`) grows, the right cluster shrinks. Negative is the
        // reverse. `grow` is ordered from the handle outward so the nearest
        // panel moves first.
        let (grow, shrink, want) = if delta > px(0.) {
            let grow: Vec<usize> = (0..=ix).rev().collect();
            let shrink: Vec<usize> = (ix + 1..n).collect();
            (grow, shrink, delta)
        } else {
            let grow: Vec<usize> = (ix + 1..n).collect();
            let shrink: Vec<usize> = (0..=ix).rev().collect();
            (grow, shrink, -delta)
        };

        let growable = grow.iter().fold(px(0.), |acc, &i| acc + grow_room(i));
        let shrinkable = shrink.iter().fold(px(0.), |acc, &i| acc + shrink_room(i));
        let applied = want.min(growable).min(shrinkable);
        if applied <= px(0.) {
            return;
        }

        let mut new = old.clone();
        let mut remaining = applied;
        for &i in &grow {
            if remaining <= px(0.) {
                break;
            }
            let take = remaining.min(grow_room(i));
            new[i] += take;
            remaining -= take;
        }
        let mut remaining = applied;
        for &i in &shrink {
            if remaining <= px(0.) {
                break;
            }
            let take = remaining.min(shrink_room(i));
            new[i] -= take;
            remaining -= take;
        }

        for (i, s) in new.iter().enumerate() {
            self.panels[i].size = Some(*s);
        }
        // A drag re-shares the whole split at the live container: what you
        // see when you let go becomes every panel's share from now on.
        self.weights = new.iter().map(|s| Some(*s)).collect();
        self.sizes = new;
        cx.notify();
    }

    /// Solve the display sizes for the current container size.
    ///
    /// When the container size changes, the panels keep the same share of the
    /// space, except that each panel's [`size_range`](ResizablePanelState::size_range)
    /// is honored: a panel already at its cap doesn't grow when the window
    /// grows, and the space it would have taken is handed to the panels that
    /// can still use it. That's what pins a toolbar or footer to its size
    /// while the content panels stretch. Panels that hit a bound drop out of
    /// the split and the rest are redistributed, repeating until nothing new
    /// clamps.
    ///
    /// The solve reads the weights and writes only the display sizes, never
    /// back into the weights. Clamping is lossy: a squeezed panel pinned at
    /// its min has given its share to its siblings, and feeding that result
    /// back in (as upstream does, one `sizes` vec doing both jobs) rewrote
    /// the split for good on every pass through a small window. The mini
    /// player toggle was the worst case: restore the main layout while the
    /// window is still mini-sized and the splits came back wherever the
    /// clamps left them.
    fn adjust_to_container_size(&mut self, cx: &mut Context<Self>) {
        if self.container_size().is_zero() {
            return;
        }

        let container = self.container_size().as_f32();
        let n = self.panels.len();
        if n == 0 {
            return;
        }

        // The weights are the proportional shares; the ranges are the bounds
        // each panel gets clamped to. A panel with no weight yet (first paint
        // pending) weighs in at its current solved size.
        let weights: Vec<f32> = self
            .weights
            .iter()
            .zip(&self.sizes)
            .map(|(w, s)| w.unwrap_or(*s).as_f32())
            .collect();
        let ranges: Vec<Range<Pixels>> = (0..n).map(|i| self.panel_size_range(i)).collect();

        // None means "still flexible"; Some means the panel clamped to a
        // bound and is fixed for the rest of the passes.
        let mut fixed: Vec<Option<f32>> = vec![None; n];
        loop {
            let fixed_total: f32 = fixed.iter().flatten().sum();
            let flex: Vec<usize> = (0..n).filter(|i| fixed[*i].is_none()).collect();
            if flex.is_empty() {
                break;
            }
            let remaining = (container - fixed_total).max(0.);
            let flex_weight: f32 = flex.iter().map(|i| weights[*i]).sum();

            let mut clamped_any = false;
            for &i in &flex {
                let ratio = if flex_weight > 0. {
                    weights[i] / flex_weight
                } else {
                    1. / flex.len() as f32
                };
                let target = remaining * ratio;
                let min = ranges[i].start.as_f32();
                let max = ranges[i].end.as_f32();
                if target < min {
                    fixed[i] = Some(min);
                    clamped_any = true;
                } else if target > max {
                    fixed[i] = Some(max);
                    clamped_any = true;
                }
            }

            if !clamped_any {
                // Nothing else clamps: hand the flexible panels their share.
                for &i in &flex {
                    let ratio = if flex_weight > 0. {
                        weights[i] / flex_weight
                    } else {
                        1. / flex.len() as f32
                    };
                    fixed[i] = Some(remaining * ratio);
                }
                break;
            }
        }

        for i in 0..n {
            let size = px(fixed[i].unwrap_or(weights[i]));
            self.sizes[i] = size;
            self.panels[i].size = Some(size);
        }
        cx.notify();
    }
}

impl EventEmitter<ResizablePanelEvent> for ResizableState {}

#[derive(Debug, Clone)]
pub(crate) struct ResizablePanelState {
    pub size: Option<Pixels>,
    pub size_range: Range<Pixels>,
    bounds: Bounds<Pixels>,
}

impl Default for ResizablePanelState {
    fn default() -> Self {
        Self {
            size: None,
            // The derived Default's px(0.)..px(0.) range made
            // adjust_to_container_size clamp a fresh entry to 0 before its
            // first paint filled the real range in; start unbounded like
            // ResizablePanel::new() instead.
            size_range: PANEL_MIN_SIZE..Pixels::MAX,
            bounds: Bounds::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext as _, TestAppContext, point, size};

    // Sum of the solved sizes, in raw pixels.
    fn total(state: &ResizableState) -> f32 {
        state.sizes.iter().map(|s| s.as_f32()).sum()
    }

    // Build a state with the given per-panel sizes and ranges, at a container
    // size along the horizontal axis. Bounds and panels are set directly since
    // the test is inside the module.
    fn make_state(sizes: &[f32], ranges: &[Range<Pixels>], container: f32) -> ResizableState {
        let panels = sizes
            .iter()
            .zip(ranges)
            .map(|(&s, r)| ResizablePanelState {
                size: Some(px(s)),
                size_range: r.clone(),
                bounds: Bounds::default(),
            })
            .collect();
        ResizableState {
            axis: Axis::Horizontal,
            panels,
            weights: sizes.iter().map(|&s| Some(px(s))).collect(),
            sizes: sizes.iter().map(|&s| px(s)).collect(),
            resizing_panel_ix: None,
            bounds: Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(container), px(100.)),
            },
        }
    }

    // Point the state at a new container size, as the group's bounds canvas
    // does when the window resizes.
    fn set_container(state: &mut ResizableState, container: f32) {
        state.bounds.size.width = px(container);
    }

    fn open() -> Range<Pixels> {
        PANEL_MIN_SIZE..Pixels::MAX
    }

    #[gpui::test]
    fn adjust_keeps_total_at_container_size(cx: &mut TestAppContext) {
        let entity = cx.new(|_| make_state(&[100., 300.], &[open(), open()], 1000.));
        entity.update(cx, |state, cx| {
            state.adjust_to_container_size(cx);
            // Everything is flexible, so the split fills the container exactly
            // and keeps the 1:3 proportion.
            assert!((total(state) - 1000.).abs() < 0.5);
            assert!((state.sizes[0].as_f32() - 250.).abs() < 0.5);
            assert!((state.sizes[1].as_f32() - 750.).abs() < 0.5);
        });
    }

    #[gpui::test]
    fn adjust_pins_a_fixed_range_panel(cx: &mut TestAppContext) {
        // A pinned toolbar (min == max) holds its size while the container
        // grows; the flexible panel absorbs the rest. This is the pinned-panel
        // path the recent fix cared about.
        let entity = cx.new(|_| make_state(&[80., 200.], &[px(80.)..px(80.), open()], 1000.));
        entity.update(cx, |state, cx| {
            state.adjust_to_container_size(cx);
            assert!((state.sizes[0].as_f32() - 80.).abs() < 0.5);
            assert!((state.sizes[1].as_f32() - 920.).abs() < 0.5);
            assert!((total(state) - 1000.).abs() < 0.5);
        });
    }

    #[gpui::test]
    fn adjust_respects_max_and_hands_slack_to_others(cx: &mut TestAppContext) {
        // First panel caps at 150; the second takes everything else.
        let entity = cx.new(|_| make_state(&[100., 100.], &[px(40.)..px(150.), open()], 1000.));
        entity.update(cx, |state, cx| {
            state.adjust_to_container_size(cx);
            assert!(state.sizes[0].as_f32() <= 150.5);
            assert!((total(state) - 1000.).abs() < 0.5);
        });
    }

    #[gpui::test]
    fn adjust_round_trip_restores_shares(cx: &mut TestAppContext) {
        // The mini toggle bug: a 934/320 main split restored while the window
        // is still mini-sized. At 320 wide the right panel's share falls
        // below its 120px min, clamps, and the left panel absorbs the rest.
        // When the window comes back to full size the split must return to
        // exactly 934/320; the solver used to feed the clamped result back
        // into its weights, so it came back wherever the squeeze left it.
        let entity = cx.new(|_| {
            make_state(
                &[934., 320.],
                &[px(40.)..Pixels::MAX, px(120.)..Pixels::MAX],
                1254.,
            )
        });
        entity.update(cx, |state, cx| {
            set_container(state, 320.);
            state.adjust_to_container_size(cx);
            // The right panel pinned at its min, the left took the leftover.
            assert!((state.sizes[1].as_f32() - 120.).abs() < 0.5);
            assert!((state.sizes[0].as_f32() - 200.).abs() < 0.5);

            set_container(state, 1254.);
            state.adjust_to_container_size(cx);
            assert!((state.sizes[0].as_f32() - 934.).abs() < 0.5);
            assert!((state.sizes[1].as_f32() - 320.).abs() < 0.5);
        });
    }

    #[gpui::test]
    fn drag_reanchors_weights(cx: &mut TestAppContext) {
        // A drag is a deliberate re-share: afterwards the weights are the
        // dragged sizes, so a dump records them and a later container change
        // scales from them.
        let entity = cx.new(|_| {
            let mut s = make_state(&[300., 700.], &[open(), open()], 1000.);
            for (i, p) in s.panels.iter_mut().enumerate() {
                let x = [0., 300.][i];
                let w = [300., 700.][i];
                p.bounds = Bounds {
                    origin: point(px(x), px(0.)),
                    size: size(px(w), px(100.)),
                };
            }
            s
        });
        let window = cx.add_window(|_, _| DragPanel);
        window
            .update(cx, |_, window, cx| {
                entity.update(cx, |state, cx| {
                    state.resize_panel(0, px(400.), window, cx);
                    let weights = state.weights();
                    assert_eq!(weights.len(), 2);
                    assert!((weights[0].as_f32() - 400.).abs() < 0.5);
                    assert!((weights[1].as_f32() - 600.).abs() < 0.5);
                });
            })
            .unwrap();
    }

    #[gpui::test]
    fn adjust_on_zero_container_is_a_noop(cx: &mut TestAppContext) {
        // No bounds yet: nothing should be touched, and no divide-by-zero.
        let entity = cx.new(|_| {
            let mut s = make_state(&[100., 200.], &[open(), open()], 0.);
            s.bounds = Bounds::default();
            s
        });
        entity.update(cx, |state, cx| {
            let before = state.sizes.clone();
            state.adjust_to_container_size(cx);
            assert_eq!(state.sizes, before);
        });
    }

    #[gpui::test]
    fn adjust_with_all_pinned_does_not_divide_by_zero(cx: &mut TestAppContext) {
        // Every panel is pinned, so the flex set empties on the first pass.
        // The solver must terminate and honor the pins, container be damned.
        let entity =
            cx.new(|_| make_state(&[80., 120.], &[px(80.)..px(80.), px(120.)..px(120.)], 1000.));
        entity.update(cx, |state, cx| {
            state.adjust_to_container_size(cx);
            assert!((state.sizes[0].as_f32() - 80.).abs() < 0.5);
            assert!((state.sizes[1].as_f32() - 120.).abs() < 0.5);
        });
    }

    #[gpui::test]
    fn insert_panel_keeps_total_at_container(cx: &mut TestAppContext) {
        let entity = cx.new(|_| make_state(&[400., 600.], &[open(), open()], 1000.));
        entity.update(cx, |state, cx| {
            state.insert_panel(Some(px(200.)), Some(1), cx);
            assert_eq!(state.panels.len(), 3);
            assert_eq!(state.sizes.len(), 3);
            // The inserted panel gets its requested size, the rest shrink
            // proportionally so the total still fills the container.
            assert!((state.sizes[1].as_f32() - 200.).abs() < 1.0);
            assert!((total(state) - 1000.).abs() < 1.0);
        });
    }

    #[gpui::test]
    fn sync_panels_count_grows_and_shrinks(cx: &mut TestAppContext) {
        let entity = cx.new(|_| make_state(&[500., 500.], &[open(), open()], 1000.));
        entity.update(cx, |state, cx| {
            state.sync_panels_count(Axis::Horizontal, 4, cx);
            assert_eq!(state.panels.len(), 4);
            assert_eq!(state.sizes.len(), 4);
            // Growing re-solves against the container.
            assert!((total(state) - 1000.).abs() < 1.0);

            state.sync_panels_count(Axis::Horizontal, 1, cx);
            assert_eq!(state.panels.len(), 1);
            assert_eq!(state.sizes.len(), 1);
        });
    }

    #[gpui::test]
    fn remove_panel_reindexes_resizing_ix(cx: &mut TestAppContext) {
        let entity = cx.new(|_| {
            let mut s = make_state(&[300., 300., 400.], &[open(), open(), open()], 1000.);
            // Pretend the third handle is mid-drag.
            s.resizing_panel_ix = Some(2);
            s
        });
        entity.update(cx, |state, cx| {
            // Removing a panel before the dragged one shifts its index down.
            state.remove_panel(0, cx);
            assert_eq!(state.panels.len(), 2);
            assert_eq!(state.resizing_panel_ix, Some(1));
        });
    }

    #[gpui::test]
    fn move_panel_carries_size_along(cx: &mut TestAppContext) {
        let entity = cx.new(|_| make_state(&[100., 200., 300.], &[open(), open(), open()], 600.));
        entity.update(cx, |state, cx| {
            state.move_panel(0, 2, cx);
            // The 100px panel moved to the end with its size intact.
            assert_eq!(state.sizes[2].as_f32(), 100.);
            assert_eq!(state.sizes[0].as_f32(), 200.);
            assert_eq!(state.sizes[1].as_f32(), 300.);
        });
    }

    #[gpui::test]
    fn move_panel_out_of_range_is_a_noop(cx: &mut TestAppContext) {
        let entity = cx.new(|_| make_state(&[100., 200.], &[open(), open()], 300.));
        entity.update(cx, |state, cx| {
            let before = state.sizes.clone();
            state.move_panel(0, 5, cx);
            state.move_panel(0, 0, cx);
            assert_eq!(state.sizes, before);
        });
    }

    #[gpui::test]
    fn resize_moves_the_boundary_and_conserves_total(cx: &mut TestAppContext) {
        // resize_panel takes a &mut Window (it never reads it), so run the
        // drag from inside a headless test window that hands one over.
        let entity = cx.new(|_| {
            // Give panels real bounds so sync_real_panel_sizes reads them back.
            let mut s = make_state(&[300., 300., 400.], &[open(), open(), open()], 1000.);
            for (i, p) in s.panels.iter_mut().enumerate() {
                let x = [0., 300., 600.][i];
                let w = [300., 300., 400.][i];
                p.bounds = Bounds {
                    origin: point(px(x), px(0.)),
                    size: size(px(w), px(100.)),
                };
            }
            s
        });
        let window = cx.add_window(|_, _| DragPanel);
        let before = 1000.;
        window
            .update(cx, |_, window, cx| {
                entity.update(cx, |state, cx| {
                    // Grow panel 0 by dragging its boundary out to 400.
                    state.resize_panel(0, px(400.), window, cx);
                    assert!(
                        (total(state) - before).abs() < 1.0,
                        "resize changed the total"
                    );
                    // Panel 0 grew, a panel on the shrink side gave the room back.
                    assert!(state.sizes[0].as_f32() > 300.);
                });
            })
            .unwrap();
    }

    #[gpui::test]
    fn resize_cannot_shrink_a_pinned_neighbor(cx: &mut TestAppContext) {
        let entity = cx.new(|_| {
            let mut s = make_state(
                &[300., 200., 500.],
                // Middle panel is pinned; a drag must push past it to the
                // flexible panel beyond.
                &[open(), px(200.)..px(200.), open()],
                1000.,
            );
            for (i, p) in s.panels.iter_mut().enumerate() {
                let x = [0., 300., 500.][i];
                let w = [300., 200., 500.][i];
                p.bounds = Bounds {
                    origin: point(px(x), px(0.)),
                    size: size(px(w), px(100.)),
                };
            }
            s
        });
        let window = cx.add_window(|_, _| DragPanel);
        window
            .update(cx, |_, window, cx| {
                entity.update(cx, |state, cx| {
                    state.resize_panel(0, px(400.), window, cx);
                    // The pinned middle panel never changed size.
                    assert_eq!(state.sizes[1].as_f32(), 200.);
                    assert!((total(state) - 1000.).abs() < 1.0);
                });
            })
            .unwrap();
    }
}
