//! The search box the searching views share: one wrapper over
//! gpui-component's input with the `SearchInput` key context and the escape
//! ladder, where the first escape clears and the second hands control back.
//! Reactions stay per-host, and query semantics stay in the projection. A
//! host whose box sits in a tab title row must notify its tab panel on
//! `Changed` and `FocusChanged`, since that row only repaints then.

use std::rc::Rc;

use gpui::{
    Action, AnyElement, App, AppContext, Context, Div, Entity, EntityInputHandler, EventEmitter,
    FocusHandle, Focusable, InteractiveElement, KeyDownEvent, ParentElement, SharedString, Styled,
    Subscription, Window, div,
};
use gpui_component::input::{
    CompletionProvider, Enter, IndentInline, Input, InputEvent, InputState, MoveDown, MoveUp,
};
use gpui_component::{ActiveTheme, Icon, Sizable};

use rox_design::assets::icons;

/// The host reads the query back through [`SearchBox::query`].
pub enum SearchEvent {
    Changed,
    Submitted,
    FocusChanged,
    /// Escape on an empty query: the host takes focus back.
    Dismissed,
}

pub struct SearchBox {
    input: Entity<InputState>,
    query: String,
    small: bool,
    xsmall: bool,
    bare: bool,
    icon: bool,
    _input_events: Subscription,
}

impl EventEmitter<SearchEvent> for SearchBox {}

impl SearchBox {
    pub fn new(
        placeholder: impl Into<SharedString>,
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
            xsmall: false,
            bare: false,
            icon: false,
            _input_events,
        }
    }

    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }

    pub fn xsmall(mut self) -> Self {
        self.xsmall = true;
        self
    }

    /// For a host that frames the box itself.
    pub fn bare(mut self) -> Self {
        self.bare = true;
        self
    }

    pub fn icon(mut self) -> Self {
        self.icon = true;
        self
    }

    pub fn set_completions(
        &mut self,
        provider: Option<Rc<dyn CompletionProvider>>,
        cx: &mut Context<Self>,
    ) {
        self.input
            .update(cx, |input, _| input.lsp.completion_provider = provider);
    }

    /// Replace the text, cursor to the end. Change still fires so the host
    /// reconciles as if typed. Guard on drift before calling, so a box being
    /// typed in keeps its cursor.
    pub fn set_value(&mut self, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| {
            input.set_value(value.to_string(), window, cx)
        });
    }

    /// Append a term (a hint chip's `artist:`) through the typing path, so the
    /// suggestion menu opens on it.
    pub fn append_term(&mut self, term: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| {
            let value = input.value().to_string();
            let sep = if value.is_empty() || value.ends_with(char::is_whitespace) {
                ""
            } else {
                " "
            };
            // The silent set parks the cursor at the end; the non-silent
            // insert there fires the completion trigger.
            input.set_value(format!("{value}{sep}"), window, cx);
            window.focus(&input.focus_handle(cx));
            input.replace_text_in_range(None, term, window, cx);
        });
    }

    /// Give the suggestion menu first claim on an action; true when it took
    /// it. A host that captures arrows for its own list calls this first.
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

    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }

    pub fn is_focused(&self, window: &Window, cx: &App) -> bool {
        self.focus_handle(cx).is_focused(window)
    }

    /// The host sizes it. Built through the entity so the key handler can
    /// reach the state: `search.update(cx, |search, cx| search.element(cx))`.
    pub fn element(&self, cx: &mut Context<Self>) -> Div {
        self.element_with_suffix(None, cx)
    }

    /// [`Self::element`] with a host control inside the box past the clear
    /// glyph, like the settings window's page-scope button: a 160px sidebar
    /// has no width for a neighbour. It arrives built because its listeners
    /// need the host's context.
    pub fn element_with_suffix(&self, suffix: Option<AnyElement>, cx: &mut Context<Self>) -> Div {
        // Clearing fires Change like a keystroke, so followers and the shared
        // query reset the same way escape does.
        let mut input = Input::new(&self.input).w_full().cleanable(true);
        if let Some(suffix) = suffix {
            input = input.suffix(suffix);
        }
        if self.xsmall {
            input = input.xsmall();
        } else if self.small {
            input = input.small();
        }
        if self.bare {
            input = input.appearance(false);
        }
        if self.icon {
            input = input.prefix(
                Icon::default()
                    .path(icons::SEARCH)
                    .small()
                    .text_color(cx.theme().muted_foreground),
            );
        }
        div()
            // Scopes the playback bindings out while focused, so space and
            // arrows type.
            .key_context("SearchInput")
            // Tab accepts the highlighted suggestion. The input binds tab to
            // IndentInline, which the menu ignores, so translate it.
            .capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
                if !this.menu_action(Box::new(Enter { secondary: false }), window, cx) {
                    cx.propagate();
                }
            }))
            // The input only hands up/down to its menu on a multi-line box, so
            // do it here. With the menu closed they propagate to the host.
            .capture_action(cx.listener(|this, _: &MoveUp, window, cx| {
                if !this.menu_action(Box::new(MoveUp), window, cx) {
                    cx.propagate();
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, window, cx| {
                if !this.menu_action(Box::new(MoveDown), window, cx) {
                    cx.propagate();
                }
            }))
            // The escape ladder. Escape arrives when the widget has nothing of
            // its own to close; stop it so a host's escape handler doesn't fire
            // over this one.
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
