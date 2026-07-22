use std::ops::Range;

use gpui::{
    Along, App, Axis, Bounds, Context, ElementId, EventEmitter, IsZero, Pixels, Window, px,
};

use gpui_component::PixelsExt;

mod panel;
mod resize_handle;
pub use panel::*;
pub(crate) use resize_handle::*;

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
    sizes: Vec<Pixels>,
    pub(crate) resizing_panel_ix: Option<usize>,
    bounds: Bounds<Pixels>,
}

impl Default for ResizableState {
    fn default() -> Self {
        Self {
            axis: Axis::Horizontal,
            panels: vec![],
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

        if let Some(ix) = ix {
            self.panels.insert(ix, panel_state);
            self.sizes.insert(ix, size);
        } else {
            self.panels.push(panel_state);
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
            self.sizes.extend(vec![PANEL_MIN_SIZE; diff]);
            changed = true;
        }

        if panels_count < self.panels.len() {
            self.panels.truncate(panels_count);
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
        // This check is only necessary to stop the very first panel from resizing on its own
        // it needs to be passed when the panel is freshly created so we get the initial size,
        // but its also fine when it sometimes passes later.
        if self.sizes[panel_ix].as_f32() == PANEL_MIN_SIZE.as_f32() {
            self.sizes[panel_ix] = size;
            self.panels[panel_ix].size = Some(size);
        }
        self.panels[panel_ix].bounds = bounds;
        self.panels[panel_ix].size_range = size_range;
        cx.notify();
    }

    /// Move a panel's state to another index, its size riding along, so
    /// an app-level reorder of a stack's children keeps every split width
    /// with its panel.
    pub(crate) fn move_panel(&mut self, from_ix: usize, to_ix: usize, cx: &mut Context<Self>) {
        if from_ix == to_ix || from_ix >= self.panels.len() || to_ix >= self.panels.len() {
            return;
        }
        let panel = self.panels.remove(from_ix);
        self.panels.insert(to_ix, panel);
        let size = self.sizes.remove(from_ix);
        self.sizes.insert(to_ix, size);
        cx.notify();
    }

    pub(crate) fn remove_panel(&mut self, panel_ix: usize, cx: &mut Context<Self>) {
        self.panels.remove(panel_ix);
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
        // mirror. `grow` is ordered from the handle outward so the nearest
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
        self.sizes = new;
        cx.notify();
    }

    /// Adjust panel sizes according to the container size.
    ///
    /// When the container size changes, the panels keep the same share of the
    /// space, except that each panel's [`size_range`](ResizablePanelState::size_range)
    /// is honored: a panel already at its cap doesn't grow when the window
    /// grows, and the space it would have taken is handed to the panels that
    /// can still use it. That's what pins a toolbar or footer to its size
    /// while the content panels stretch. Panels that hit a bound drop out of
    /// the split and the rest are redistributed, repeating until nothing new
    /// clamps.
    fn adjust_to_container_size(&mut self, cx: &mut Context<Self>) {
        if self.container_size().is_zero() {
            return;
        }

        let container = self.container_size().as_f32();
        let n = self.panels.len();
        if n == 0 {
            return;
        }

        // The current sizes act as the proportional weights; the ranges are
        // the bounds each panel gets clamped to.
        let weights: Vec<f32> = self.sizes.iter().map(|s| s.as_f32()).collect();
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
