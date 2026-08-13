use std::{
    cell::Cell,
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
};

use gpui::{
    AnyElement, App, Axis, Element, ElementId, Entity, GlobalElementId, InteractiveElement,
    IntoElement, MouseDownEvent, MouseUpEvent, ParentElement as _, Pixels, Point, Render,
    StatefulInteractiveElement, Styled as _, Window, div, prelude::FluentBuilder as _, px,
    transparent_black,
};

use crate::DockPlacement;
use gpui_component::{ActiveTheme as _, AxisExt as _};

pub(crate) const HANDLE_PADDING: Pixels = px(4.);
pub(crate) const HANDLE_SIZE: Pixels = px(1.);

/// rox addition: whether the resting 1px seam between panel tiles paints.
/// Off leaves the handles invisible but still draggable, for looks that
/// want panels to sit flush; an active drag still shows its line. A static
/// like the host app's own appearance flags, since the handle render has
/// no path to app settings; the host seeds it and repaints.
static SEAMS: AtomicBool = AtomicBool::new(true);

pub fn seams() -> bool {
    SEAMS.load(Ordering::Relaxed)
}

/// Set seam visibility. Repainting open windows is the caller's.
pub fn set_seams(on: bool) {
    SEAMS.store(on, Ordering::Relaxed);
}

/// Create a resize handle for a resizable panel.
pub(crate) fn resize_handle<T: 'static, E: 'static + Render>(
    id: impl Into<ElementId>,
    axis: Axis,
) -> ResizeHandle<T, E> {
    ResizeHandle::new(id, axis)
}

pub(crate) struct ResizeHandle<T: 'static, E: 'static + Render> {
    id: ElementId,
    axis: Axis,
    drag_value: Option<Rc<T>>,
    placement: Option<DockPlacement>,
    on_drag: Option<Rc<dyn Fn(&Point<Pixels>, &mut Window, &mut App) -> Entity<E>>>,
}

impl<T: 'static, E: 'static + Render> ResizeHandle<T, E> {
    fn new(id: impl Into<ElementId>, axis: Axis) -> Self {
        let id = id.into();
        Self {
            id: id.clone(),
            on_drag: None,
            drag_value: None,
            placement: None,
            axis,
        }
    }

    pub(crate) fn on_drag(
        mut self,
        value: T,
        f: impl Fn(Rc<T>, &Point<Pixels>, &mut Window, &mut App) -> Entity<E> + 'static,
    ) -> Self {
        let value = Rc::new(value);
        self.drag_value = Some(value.clone());
        self.on_drag = Some(Rc::new(move |p, window, cx| {
            f(value.clone(), p, window, cx)
        }));
        self
    }

    pub(crate) fn placement(mut self, placement: DockPlacement) -> Self {
        self.placement = Some(placement);
        self
    }
}

#[derive(Default, Debug, Clone)]
struct ResizeHandleState {
    active: Cell<bool>,
}

impl ResizeHandleState {
    fn set_active(&self, active: bool) {
        self.active.set(active);
    }

    fn is_active(&self) -> bool {
        self.active.get()
    }
}

impl<T: 'static, E: 'static + Render> IntoElement for ResizeHandle<T, E> {
    type Element = ResizeHandle<T, E>;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl<T: 'static, E: 'static + Render> Element for ResizeHandle<T, E> {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui::LayoutId, Self::RequestLayoutState) {
        let neg_offset = -HANDLE_PADDING;
        let axis = self.axis;

        // rox addition: while the resize lock holds, the handle is only
        // the seam. No cursor, no drag, no occluding hit strip, so a
        // pointer crossing a panel edge neither flickers nor grabs the
        // layout.
        let live = !crate::resize_locked();

        window.with_element_state(id.unwrap(), |state, window| {
            let state = state.unwrap_or(ResizeHandleState::default());

            let bg_color = if state.is_active() {
                cx.theme().drag_border
            } else if seams() {
                cx.theme().border
            } else {
                transparent_black()
            };

            let mut el = div()
                .id(self.id.clone())
                .when(live, |this| this.occlude())
                .absolute()
                .flex_shrink_0()
                .group("handle")
                .when(live, |this| {
                    this.when_some(self.on_drag.clone(), |this, on_drag| {
                        this.on_drag(
                            self.drag_value.clone().unwrap(),
                            move |_, position, window, cx| on_drag(&position, window, cx),
                        )
                    })
                })
                .map(|this| match self.placement {
                    Some(DockPlacement::Left) => {
                        // Special for Left Dock
                        //  FIXME: Improve this to let the scroll bar have px(HANDLE_PADDING)
                        this.when(live, |this| this.cursor_col_resize())
                            .top_0()
                            .right(px(1.))
                            .h_full()
                            .w(HANDLE_SIZE)
                            .pl(HANDLE_PADDING)
                    }
                    _ => this
                        .when(axis.is_horizontal(), |this| {
                            this.when(live, |this| this.cursor_col_resize())
                                .top_0()
                                .left(neg_offset)
                                .h_full()
                                .w(HANDLE_SIZE)
                                .px(HANDLE_PADDING)
                        })
                        .when(axis.is_vertical(), |this| {
                            this.when(live, |this| this.cursor_row_resize())
                                .top(neg_offset)
                                .left_0()
                                .w_full()
                                .h(HANDLE_SIZE)
                                .py(HANDLE_PADDING)
                        }),
                })
                .child(
                    div()
                        .bg(bg_color)
                        .group_hover("handle", |this| this.bg(bg_color))
                        .when(axis.is_horizontal(), |this| this.h_full().w(HANDLE_SIZE))
                        .when(axis.is_vertical(), |this| this.w_full().h(HANDLE_SIZE)),
                )
                .into_any_element();

            let layout_id = el.request_layout(window, cx);

            ((layout_id, el), state)
        })
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: gpui::Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        request_layout.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: gpui::Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        request_layout.paint(window, cx);

        // rox addition: a locked handle takes no part in the mouse, so a
        // click landing where the strip would be doesn't flash the seam
        // into its drag color.
        if crate::resize_locked() {
            return;
        }

        window.with_element_state(id.unwrap(), |state: Option<ResizeHandleState>, window| {
            let state = state.unwrap_or(ResizeHandleState::default());

            window.on_mouse_event({
                let state = state.clone();
                move |ev: &MouseDownEvent, phase, window, _| {
                    if bounds.contains(&ev.position) && phase.bubble() {
                        state.set_active(true);
                        window.refresh();
                    }
                }
            });

            window.on_mouse_event({
                let state = state.clone();
                move |_: &MouseUpEvent, _, window, _| {
                    if state.is_active() {
                        state.set_active(false);
                        window.refresh();
                    }
                }
            });

            ((), state)
        });
    }
}
