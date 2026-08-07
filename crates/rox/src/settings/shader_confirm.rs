//! The screen shader confirm: one small OS window opened after a risky
//! apply from the Shader settings page (the enable toggle, a file pick), the
//! display-settings pattern. Keep locks the change in; Revert, the
//! countdown running out, or closing the window restores the state from
//! before the apply and persists it. Hot reloads and the toggle hotkey
//! never come through here: the reload is the authoring loop, and the
//! hotkey is the escape hatch. The window registers itself with the
//! workspace's shading machinery so it is never shaded, whatever the
//! all-windows option says: it has to stay readable under exactly the
//! shader it exists to undo.

use gpui::{
    div, prelude::*, px, size, App, Bounds, Context, EntityId, Global, SharedString, Task,
    WeakEntity, Window, WindowHandle,
};
use gpui_component::Root;

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel;
use crate::settings::ui::small_button;
use crate::settings::{PostShaderConfig, Settings};

/// How long the shader stays on trial before it reverts on its own.
const COUNTDOWN_SECS: u32 = 12;

/// The caller's after-revert refresh, boxed for the entity to carry.
type OnReverted = Box<dyn FnOnce(&mut App)>;

/// The open confirm, if any: a second risky apply reuses it, resetting the
/// countdown while keeping the first dialog's prior as the baseline, so a
/// run of quick changes still reverts to the last state the user actually
/// confirmed. Weak, or the global itself would keep the entity from ever
/// releasing.
#[derive(Default)]
struct OpenConfirm(Option<(WindowHandle<Root>, WeakEntity<ShaderConfirm>)>);

impl Global for OpenConfirm {}

/// Open the confirm for a change whose pre-apply state was `prior`, or
/// reset the open one's countdown. `on_reverted` runs after a revert so
/// the caller can refresh whatever mirrors the reverted fields.
pub fn open(
    prior: PostShaderConfig,
    player: EntityId,
    on_reverted: impl FnOnce(&mut App) + 'static,
    cx: &mut App,
) {
    if let Some((handle, confirm)) = cx.default_global::<OpenConfirm>().0.clone() {
        if let Some(confirm) = confirm.upgrade() {
            confirm.update(cx, |confirm, cx| {
                confirm.remaining = COUNTDOWN_SECS;
                cx.notify();
            });
            handle
                .update(cx, |_, window, _| window.activate_window())
                .ok();
            return;
        }
    }
    let bounds = Bounds::centered(None, size(px(420.), px(170.)), cx);
    let view = std::rc::Rc::new(std::cell::RefCell::new(None));
    let handle = {
        let view = view.clone();
        crate::panel::open_fixed_window(cx, "rox - Screen Shader", bounds, move |_, cx| {
            let entity = cx.new(|cx| ShaderConfirm::new(prior, player, on_reverted, cx));
            *view.borrow_mut() = Some(entity.clone());
            entity
        })
    };
    let entity = view.borrow_mut().take().expect("build ran synchronously");
    // Registered before any shading sweep can run, and torn down with the
    // entity below.
    crate::workspace::note_confirm_window(Some(handle.into()), cx);
    cx.default_global::<OpenConfirm>().0 = Some((handle, entity.downgrade()));
    // Every close lands here: Keep and Revert close the window, the OS
    // close button too. Only Keep marks the entity, everything else is a
    // revert, so a dismissed dialog fails safe.
    cx.observe_release(&entity, |confirm, cx| {
        crate::workspace::note_confirm_window(None, cx);
        cx.default_global::<OpenConfirm>().0 = None;
        if confirm.kept {
            return;
        }
        let prior = confirm.prior.clone();
        Settings::update(move |s| {
            s.post_shader.enabled = prior.enabled;
            s.post_shader.path = prior.path.clone();
        });
        crate::workspace::apply_post_shader(cx);
        if let Some(on_reverted) = confirm.on_reverted.take() {
            on_reverted(cx);
        }
    })
    .detach();
}

struct ShaderConfirm {
    /// The config from before the apply, what a revert restores. The
    /// all-windows option rides along untouched; only the enable switch
    /// and the path are on trial here.
    prior: PostShaderConfig,
    /// The front workspace's player, for the window tint.
    player: EntityId,
    /// Seconds left on the trial; the countdown task walks it down.
    remaining: u32,
    /// Set by Keep alone. The release hook reads it to tell a confirmed
    /// close from every other way the window can go away.
    kept: bool,
    /// The caller's after-revert refresh, taken by the release hook.
    on_reverted: Option<OnReverted>,
    _countdown: Task<()>,
}

impl ShaderConfirm {
    fn new(
        prior: PostShaderConfig,
        player: EntityId,
        on_reverted: impl FnOnce(&mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> Self {
        let _countdown = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(1))
                    .await;
                let finished = this
                    .update(cx, |this, cx| {
                        this.remaining = this.remaining.saturating_sub(1);
                        cx.notify();
                        this.remaining == 0
                    })
                    .unwrap_or(true);
                if finished {
                    break;
                }
            }
            this.update(cx, |this, cx| this.close(cx)).ok();
        });
        ShaderConfirm {
            prior,
            player,
            remaining: COUNTDOWN_SECS,
            kept: false,
            on_reverted: Some(Box::new(on_reverted)),
            _countdown,
        }
    }

    /// Close the window; the release hook decides what the close means.
    /// Deferred, since the buttons run inside this window's own update.
    fn close(&mut self, cx: &mut Context<Self>) {
        if let Some((handle, _)) = cx.default_global::<OpenConfirm>().0.clone() {
            cx.defer(move |cx| {
                handle
                    .update(cx, |_, window, _| window.remove_window())
                    .ok();
            });
        }
    }
}

impl Render for ShaderConfirm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Tinted and focus-claimed like every other child window.
        let player = self.player;
        palette::note_focus(player, window.is_window_active(), cx);
        let remaining = self.remaining;
        let hotkey = if cfg!(target_os = "macos") {
            "Cmd+Shift+X"
        } else {
            "Ctrl+Shift+X"
        };
        panel::window_body(player, || {
            div()
                .flex()
                .flex_col()
                .size_full()
                .p(tokens::SPACE_MD)
                .gap(tokens::SPACE_MD)
                .bg(palette::bg_elevated())
                .text_color(palette::text_bright())
                .text_sm()
                .when_some(crate::settings::app_font(), |d, font| d.font_family(font))
                .child("Keep this screen shader?")
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(SharedString::from(format!(
                            "A shader can make windows hard to use. Without a Keep, \
                             everything reverts in {remaining}s. {hotkey} toggles the \
                             shader from anywhere.",
                        ))),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_end()
                        .gap(tokens::SPACE_SM)
                        .child(small_button(
                            "Revert",
                            icons::CLOSE,
                            false,
                            cx.listener(|this, _, _, cx| this.close(cx)),
                        ))
                        .child(small_button(
                            "Keep",
                            icons::CHECK,
                            false,
                            cx.listener(|this, _, _, cx| {
                                this.kept = true;
                                this.close(cx);
                            }),
                        )),
                )
                .into_any_element()
        })
    }
}
