//! The playlist name modal: a small window with one name field. Enter creates
//! a playlist (adding any tracks the caller passed, the Add to Playlist menu's
//! "New Playlist...") or renames an existing one. Modeled on the panel rename
//! window.

use gpui::{
    div, prelude::*, px, size, App, Bounds, Context, Entity, FocusHandle, Focusable, SharedString,
    Subscription, Window,
};
use gpui_component::input::{Input, InputEvent, InputState};

use crate::backdrop::WindowBackdrop;
use crate::design::{palette, tokens};
use crate::panel::AppState;

/// What the modal commits on Enter.
enum Action {
    /// Create a playlist and add these tracks (empty to just create one).
    Create(Vec<i64>),
    /// Rename this playlist.
    Rename(i64),
}

/// Open the create modal. `ids` are tracks to add to the new playlist.
pub fn open(state: AppState, ids: Vec<i64>, cx: &mut App) {
    open_modal(
        state,
        Action::Create(ids),
        "New Playlist",
        String::new(),
        cx,
    );
}

/// Open the rename modal, seeded with the current name.
pub fn open_rename(state: AppState, id: i64, current: String, cx: &mut App) {
    open_modal(state, Action::Rename(id), "Rename Playlist", current, cx);
}

fn open_modal(state: AppState, action: Action, verb: &str, current: String, cx: &mut App) {
    let title = SharedString::from(format!("rox - {verb}"));
    let bounds = Bounds::centered(None, size(px(380.), px(116.)), cx);
    crate::panel::open_child_window(cx, title, bounds, None, move |window, cx| {
        cx.new(|cx| PlaylistNameWindow::new(state, action, current, window, cx))
    });
}

struct PlaylistNameWindow {
    state: AppState,
    action: Action,
    input: Entity<InputState>,
    backdrop: WindowBackdrop,
    _input_events: Subscription,
    /// This window pumps its own frames, so the backdrop needs its own wake on
    /// a new bake.
    _backdrop_changed: Subscription,
}

impl PlaylistNameWindow {
    fn new(
        state: AppState,
        action: Action,
        current: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Playlist name")
                .default_value(current)
        });
        let _input_events = cx.subscribe_in(
            &input,
            window,
            |this, input, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    let name = input.read(cx).value().trim().to_string();
                    this.commit(name, window, cx);
                }
            },
        );
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        window.focus(&input.read(cx).focus_handle(cx));
        PlaylistNameWindow {
            state,
            action,
            input,
            backdrop: WindowBackdrop::default(),
            _input_events,
            _backdrop_changed,
        }
    }

    /// Commit the name and close. An empty name does nothing, so Enter on a
    /// blank field just waits.
    fn commit(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        if name.is_empty() {
            return;
        }
        self.state
            .library
            .update(cx, |library, cx| match &self.action {
                Action::Create(ids) => {
                    if let Some(id) = library.create_playlist(&name, cx) {
                        if !ids.is_empty() {
                            library.add_to_playlist(id, ids, cx);
                        }
                    }
                }
                Action::Rename(id) => library.rename_playlist(*id, &name, cx),
            });
        window.remove_window();
    }
}

impl Focusable for PlaylistNameWindow {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }
}

impl Render for PlaylistNameWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_XS)
            .p(tokens::SPACE_MD)
            .bg(palette::bg_elevated())
            .text_color(palette::text_bright())
            .text_sm()
            .children(self.backdrop.layer(&self.state.now_art, window, cx))
            .child(Input::new(&self.input).w_full())
            .child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child("Press Enter to create"),
            )
    }
}
