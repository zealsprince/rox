//! The title modal F2 opens: one field holding the selected track's title.
//! Enter writes it the way the metadata panel's quick edit does, so a cue
//! track's title stays in the library instead of titling the whole image.

use gpui::{
    App, Bounds, Context, Div, Entity, FocusHandle, Focusable, Global, KeyBinding, SharedString,
    Subscription, Window, WindowHandle, actions, div, prelude::*, px, size,
};
use gpui_component::Root;
use gpui_component::input::{Input, InputEvent, InputState, SelectAll};

use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_library::cue::TrackKey;
use rox_library::writer::{self, Change, Field};
use rox_panel_api::panel::AppState;
use rox_panel_kit::ui::{Seg, kbd_line, section, small_button};
use rox_services::backdrop::WindowBackdrop;

use crate::matching::{WindowRegistry, open_or_focus};

actions!(track_title, [Save]);

const CONTEXT: &str = "TrackTitle";

/// Bound on the window root so Enter commits wherever focus is; the single-line
/// input propagates it up.
pub fn bindings() -> Vec<KeyBinding> {
    vec![KeyBinding::new("enter", Save, Some(CONTEXT))]
}

#[derive(Default)]
struct OpenTitles(Vec<(TrackKey, WindowHandle<Root>)>);

impl Global for OpenTitles {}

impl WindowRegistry for OpenTitles {
    type Key = TrackKey;
    fn entries(&mut self) -> &mut Vec<(TrackKey, WindowHandle<Root>)> {
        &mut self.0
    }
}

/// Only files on disk carry tags rox writes, so a plugin row or a station
/// opens nothing.
pub fn open(state: AppState, id: i64, cx: &mut App) {
    let library = state.library.read(cx);
    let Some(key) = library
        .keys_for(&[id])
        .ok()
        .and_then(|keys| keys.into_iter().next())
        .filter(TrackKey::is_local)
    else {
        return;
    };
    let Some(current) = library.meta_for_key(&key).map(|meta| meta.title) else {
        return;
    };

    open_or_focus::<OpenTitles>(
        key.clone(),
        move |cx| {
            let bounds = Bounds::centered(None, size(px(420.), px(170.)), cx);
            rox_panel_api::panel::open_child_window(
                cx,
                rox_i18n::t!("track-title-window-title"),
                bounds,
                None,
                move |window, cx| cx.new(|cx| TitleWindow::new(state, key, current, window, cx)),
            )
        },
        cx,
    );
}

struct TitleWindow {
    state: AppState,
    key: TrackKey,
    current: String,
    input: Entity<InputState>,
    saving: bool,
    error: Option<SharedString>,
    backdrop: WindowBackdrop,
    _input_events: Subscription,
    _backdrop_changed: Subscription,
}

impl TitleWindow {
    fn new(
        state: AppState,
        key: TrackKey,
        current: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(rox_i18n::t!("info-item-title"))
                .default_value(current.clone())
        });
        let _input_events = cx.subscribe_in(&input, window, |_, _, event: &InputEvent, _, cx| {
            if let InputEvent::Change = event {
                cx.notify();
            }
        });
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());

        // Selected whole, so typing replaces the title the way F2 does
        // elsewhere. The input's select-all is private and only reachable as
        // an action, which needs a drawn frame to find the focused input.
        window.focus(&input.read(cx).focus_handle(cx));
        window.on_next_frame(|window, cx| window.dispatch_action(Box::new(SelectAll), cx));

        TitleWindow {
            state,
            key,
            current,
            input,
            saving: false,
            error: None,
            backdrop: WindowBackdrop::default(),
            _input_events,
            _backdrop_changed,
        }
    }

    fn title(&self, cx: &App) -> String {
        self.input.read(cx).value().trim().to_string()
    }

    fn savable(&self, cx: &App) -> bool {
        !self.saving && !self.title(cx).is_empty()
    }

    /// A failed write keeps the window open with the error and the file
    /// untouched.
    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.savable(cx) {
            return;
        }

        let title = self.title(cx);
        if title == self.current {
            window.remove_window();
            return;
        }

        let changes = vec![Change {
            field: Field::Title,
            value: Some(title),
        }];
        let key = self.key.clone();
        self.saving = true;
        self.error = None;
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn({
                    let key = key.clone();
                    let changes = changes.clone();
                    async move { writer::commit_key(&key.path, key.sub, &changes, &[]) }
                })
                .await;

            this.update_in(cx, |this, window, cx| match result {
                Ok(()) => {
                    this.state
                        .library
                        .update(cx, |library, cx| library.apply_edit(&key, &changes, cx));
                    window.remove_window();
                }

                Err(e) => {
                    this.saving = false;
                    this.error = Some(e.into());
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

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
            let reason = match self.saving {
                true => rox_i18n::t!("track-title-saving"),
                false => rox_i18n::t!("track-title-not-savable"),
            };
            div()
                .text_xs()
                .text_color(palette::tone_warn())
                .child(reason)
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
                        rox_i18n::t!("settings-common-cancel"),
                        icons::CLOSE,
                        self.saving,
                        cx.listener(|_, _, window, _| window.remove_window()),
                    )),
            )
    }
}

impl Focusable for TitleWindow {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }
}

impl Render for TitleWindow {
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
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_SM)
                    .p(tokens::SPACE_MD)
                    .bg(palette::bg_elevated())
                    .child(section(
                        rox_i18n::t!("info-item-title"),
                        None,
                        Input::new(&self.input).w_full(),
                    ))
                    .when_some(self.error.clone(), |d, error| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(palette::tone_warn())
                                .child(error),
                        )
                    }),
            )
            .child(self.footer(savable, cx))
    }
}
