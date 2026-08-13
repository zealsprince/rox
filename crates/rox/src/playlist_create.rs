//! The playlist name modal: a small window with one name field. Enter creates
//! a playlist (adding any tracks the caller passed, the Add to Playlist menu's
//! "New Playlist...") or renames an existing one. Modeled on the panel rename
//! window.

use gpui::{
    actions, div, prelude::*, px, size, App, Bounds, Context, Div, Entity, FocusHandle, Focusable,
    KeyBinding, SharedString, Subscription, Window,
};
use gpui_component::input::{Input, InputEvent, InputState};

use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel::AppState;
use rox_panel_kit::ui::{kbd_line, section, small_button, Seg};
use rox_services::backdrop::WindowBackdrop;

actions!(playlist_create, [Save]);

/// The key context the window's own bindings scope to.
const CONTEXT: &str = "PlaylistName";

/// The modal's save binding; call once at startup. It rides the window
/// root rather than the field, so enter commits wherever focus sits - the
/// single-line input sees the key first and propagates it up here.
pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("enter", Save, Some(CONTEXT))]);
}

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
    let bounds = Bounds::centered(None, size(px(380.), px(170.)), cx);
    rox_panel_api::panel::open_child_window(cx, title, bounds, None, move |window, cx| {
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
        // The name is what gates the save, so the footer follows it
        // keystroke by keystroke.
        let _input_events = cx.subscribe_in(&input, window, |_, _, event: &InputEvent, _, cx| {
            if let InputEvent::Change = event {
                cx.notify();
            }
        });
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

    /// Whether the name is enough to save. A blank field is the only
    /// refusal there is.
    fn savable(&self, cx: &App) -> bool {
        !self.input.read(cx).value().trim().is_empty()
    }

    /// Commit the name and close. An empty name does nothing, which the
    /// footer says in place of the shortcut so the refusal isn't silent.
    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.input.read(cx).value().trim().to_string();
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

    /// The window's own actions: the save, and the shortcut for it.
    fn footer(&self, savable: bool, cx: &mut Context<Self>) -> Div {
        let hint = if savable {
            kbd_line([
                Seg::Text("Press".into()),
                Seg::Key("Enter".into()),
                Seg::Text("to save".into()),
            ])
            .text_xs()
            .into_any_element()
        } else {
            div()
                .text_xs()
                .text_color(palette::tone_warn())
                .child("Name the playlist to save it")
                .into_any_element()
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(tokens::SPACE_SM)
            .px(tokens::SPACE_MD)
            .py(tokens::SPACE_SM)
            .border_t_1()
            .border_color(palette::border())
            .bg(palette::bg_panel())
            .child(hint)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .child(small_button(
                        "Save",
                        icons::CHECK,
                        !savable,
                        cx.listener(|this, _, window, cx| this.commit(window, cx)),
                    ))
                    .child(small_button(
                        "Cancel",
                        icons::CLOSE,
                        false,
                        cx.listener(|_, _, window, _| window.remove_window()),
                    )),
            )
    }
}

impl Focusable for PlaylistNameWindow {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }
}

impl Render for PlaylistNameWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let savable = self.savable(cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .key_context(CONTEXT)
            .on_action(cx.listener(|this, _: &Save, window, cx| this.commit(window, cx)))
            .bg(palette::bg_elevated())
            .text_color(palette::text_bright())
            .text_sm()
            .children(self.backdrop.layer(&self.state.now_art, window, cx))
            // The body's own surface, a second elevated layer over the
            // window's, the same as the settings page. Two layers is what
            // the backdrop reads through everywhere.
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .p(tokens::SPACE_MD)
                    .bg(palette::bg_elevated())
                    .child(section("Name", None, Input::new(&self.input).w_full())),
            )
            .child(self.footer(savable, cx))
    }
}
