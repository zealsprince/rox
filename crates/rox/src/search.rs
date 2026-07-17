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

use std::rc::Rc;

use gpui::{
    div, Action, App, AppContext, Context, Div, Entity, EntityInputHandler, EventEmitter,
    FocusHandle, Focusable, InteractiveElement, KeyDownEvent, ParentElement, Styled, Subscription,
    Window,
};
use gpui_component::input::{
    CompletionProvider, Enter, IndentInline, Input, InputEvent, InputState,
};
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

    /// Attach or swap the input's completion provider, the suggestion
    /// menu over the query syntax's tag values.
    pub fn set_completions(
        &mut self,
        provider: Option<Rc<dyn CompletionProvider>>,
        cx: &mut Context<Self>,
    ) {
        self.input
            .update(cx, |input, _| input.lsp.completion_provider = provider);
    }

    /// Append a term to the query - a hint chip's `artist:` - space
    /// separated, cursor at the end, focus back on the box. The term
    /// itself lands through the input's typing path, so the suggestion
    /// menu opens on it like it would for a keystroke.
    pub fn append_term(&mut self, term: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| {
            let value = input.value().to_string();
            let sep = if value.is_empty() || value.ends_with(char::is_whitespace) {
                ""
            } else {
                " "
            };
            // The silent set parks the cursor at the end; the non-silent
            // insert there is what fires the completion trigger.
            input.set_value(format!("{value}{sep}"), window, cx);
            window.focus(&input.focus_handle(cx));
            input.replace_text_in_range(None, term, window, cx);
        });
    }

    /// Give the input's suggestion menu first claim on an action; true
    /// when the menu was open and took it. A host that captures arrows
    /// for its own list calls this first, so an open menu keeps its
    /// navigation.
    pub fn menu_action(
        &mut self,
        action: Box<dyn Action>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.input.update(cx, |input, cx| {
            input.handle_action_for_context_menu(action, window, cx)
        })
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
            // Tab accepts the highlighted suggestion. The input binds tab
            // to IndentInline, which the menu ignores, so translate it to
            // the menu's accept; with the menu closed, tab keeps its
            // default meaning.
            .capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
                if !this.menu_action(Box::new(Enter { secondary: false }), window, cx) {
                    cx.propagate();
                }
            }))
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
