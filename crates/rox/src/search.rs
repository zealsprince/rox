//! The search box the searching views share: one wrapper over
//! gpui-component's input carrying the behaviors every host wants and
//! none of the reactions, which stay per-host. The behaviors: the
//! `SearchInput` key context that scopes playback bindings out while
//! focused, and the escape ladder - first escape clears the query,
//! a second one hands control back to the host. Hosts embed the element,
//! size it themselves, and subscribe to [`SearchEvent`]; a host whose box
//! sits in a tab title row must notify its tab panel on `Changed` and
//! `FocusChanged`, since that row only repaints when the tab panel is
//! notified. Query semantics (what a string matches) stay in the
//! projection; this is only the box.

use gpui::{
    div, App, AppContext, Context, Div, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, KeyDownEvent, ParentElement, Styled, Subscription, Window,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::Sizable;

/// What the box tells its host; the host reads the query back through
/// [`SearchBox::query`].
pub enum SearchEvent {
    /// The query text changed.
    Changed,
    /// Enter pressed inside the box.
    Submitted,
    /// Focus entered or left the box.
    FocusChanged,
    /// Escape on an empty query: the host takes focus back (and a modal
    /// host closes).
    Dismissed,
}

pub struct SearchBox {
    input: Entity<InputState>,
    /// The input's value, mirrored on change events so reads never dig
    /// through the widget.
    query: String,
    /// Render the compact input, the title-row fit.
    small: bool,
    _input_events: Subscription,
}

impl EventEmitter<SearchEvent> for SearchBox {}

impl SearchBox {
    pub fn new(
        placeholder: &'static str,
        initial: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .default_value(initial.to_string())
        });
        let _input_events = cx.subscribe(&input, |this: &mut Self, input, event, cx| {
            match event {
                InputEvent::Change => {
                    this.query = input.read(cx).value().to_string();
                    cx.emit(SearchEvent::Changed);
                }
                InputEvent::PressEnter { .. } => cx.emit(SearchEvent::Submitted),
                InputEvent::Focus | InputEvent::Blur => cx.emit(SearchEvent::FocusChanged),
            }
            cx.notify();
        });
        SearchBox {
            input,
            query: initial.to_string(),
            small: false,
            _input_events,
        }
    }

    /// Use the compact input size, for title-bar hosts.
    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// The input's focus handle, so a host can focus the box or make it
    /// the host's own focus target.
    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }

    pub fn is_focused(&self, window: &Window, cx: &App) -> bool {
        self.focus_handle(cx).is_focused(window)
    }

    /// The rendered box; the host sizes it (`.w()`, `.flex_1()`). Built
    /// through the entity so the key handler can reach the state:
    /// `search.update(cx, |search, cx| search.element(cx))`.
    pub fn element(&self, cx: &mut Context<Self>) -> Div {
        let mut input = Input::new(&self.input).w_full();
        if self.small {
            input = input.small();
        }
        div()
            // Scopes the workspace's playback key bindings out while the
            // input is focused, so space and arrows type instead.
            .key_context("SearchInput")
            // The escape ladder. The widget propagates escape when it has
            // nothing of its own (IME, context menu) to close, so it lands
            // here; stopped either way so a host's own escape handler
            // never fires over a handled one.
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key != "escape" {
                    return;
                }
                cx.stop_propagation();
                if this.query.is_empty() {
                    cx.emit(SearchEvent::Dismissed);
                } else {
                    this.input
                        .update(cx, |input, cx| input.set_value("", window, cx));
                }
            }))
            .child(input)
    }
}
