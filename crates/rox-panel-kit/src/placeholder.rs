//! Input placeholders worded from a message key. An `InputState` keeps the
//! text it was built with, so without this a language switch would leave
//! every open input's hint in the old language until its window reopened.

use gpui::{AnyWindowHandle, App, Context, Global, WeakEntity, Window};
use gpui_component::input::InputState;

#[derive(Default)]
struct Keyed(Vec<(WeakEntity<InputState>, AnyWindowHandle, &'static str)>);

impl Global for Keyed {}

pub trait PlaceholderKey {
    /// The placeholder `key` names, set now and again by [`reword_placeholders`].
    fn placeholder_key(
        self,
        key: &'static str,
        window: &Window,
        cx: &mut Context<InputState>,
    ) -> Self;
}

impl PlaceholderKey for InputState {
    fn placeholder_key(
        self,
        key: &'static str,
        window: &Window,
        cx: &mut Context<InputState>,
    ) -> Self {
        let input = cx.weak_entity();
        let held = &mut cx.default_global::<Keyed>().0;

        // Dialogs come and go, so closed inputs are dropped as new ones join.
        held.retain(|(input, _, _)| input.upgrade().is_some());
        held.push((input, window.window_handle(), key));

        self.placeholder(rox_i18n::t!(key))
    }
}

/// Re-word every live keyed placeholder in the active language.
pub fn reword_placeholders(cx: &mut App) {
    let held = std::mem::take(&mut cx.default_global::<Keyed>().0);
    let mut kept = Vec::with_capacity(held.len());

    for (input, window, key) in held {
        let Some(entity) = input.upgrade() else {
            continue;
        };

        let text = rox_i18n::t!(key);
        let reworded = window.update(cx, |_, window, cx| {
            entity.update(cx, |input, cx| input.set_placeholder(text, window, cx));
        });
        if reworded.is_ok() {
            kept.push((input, window, key));
        }
    }

    // An input built while the closures above ran registered itself meanwhile.
    cx.default_global::<Keyed>().0.extend(kept);
}
