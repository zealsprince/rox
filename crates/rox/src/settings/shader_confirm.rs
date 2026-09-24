//! The screen shader confirm: a small window opened after a risky apply from
//! the Shader settings page. Keep locks the change in; Revert or closing the
//! window restores the state from before the apply.
//!
//! No countdown: a timer that reverts on its own is easy to miss. Hot reloads
//! and the toggle hotkey never come through here. The window is never shaded,
//! whatever the all-windows option says, so it stays readable under the shader
//! it exists to undo.

use gpui::{
    App, Bounds, Context, Entity, EntityId, FocusHandle, Global, KeyBinding, Subscription,
    WeakEntity, Window, WindowHandle, actions, div, prelude::*, px, size,
};
use gpui_component::Root;

use rox_core::settings::{PostShaderConfig, Settings};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel;
use rox_panel_kit::ui::{Seg, chord, kbd_line, small_button};
use rox_services::backdrop::{NowPlayingArt, WindowBackdrop};

type OnReverted = Box<dyn FnOnce(&mut App)>;

/// A second risky apply reuses the open confirm, keeping the first prior as the
/// baseline. Weak, or the global would keep the entity alive.
#[derive(Default)]
struct OpenConfirm(Option<(WindowHandle<Root>, WeakEntity<ShaderConfirm>)>);

impl Global for OpenConfirm {}

pub fn open(
    prior: PostShaderConfig,
    player: EntityId,
    now_art: Entity<NowPlayingArt>,
    on_reverted: impl FnOnce(&mut App) + 'static,
    cx: &mut App,
) {
    if let Some((handle, confirm)) = cx.default_global::<OpenConfirm>().0.clone()
        && confirm.upgrade().is_some()
    {
        handle
            .update(cx, |_, window, _| window.activate_window())
            .ok();
        return;
    }
    let bounds = Bounds::centered(None, size(px(420.), px(170.)), cx);
    let view = std::rc::Rc::new(std::cell::RefCell::new(None));
    let handle = {
        let view = view.clone();
        rox_panel_api::panel::open_fixed_window(
            cx,
            rox_i18n::t!("shader-confirm-window-title"),
            bounds,
            move |_, cx| {
                let entity =
                    cx.new(|cx| ShaderConfirm::new(prior, player, now_art, on_reverted, cx));
                *view.borrow_mut() = Some(entity.clone());
                entity
            },
        )
    };
    let entity = view.borrow_mut().take().expect("build ran synchronously");
    // Registered before any shading sweep can run.
    crate::workspace::note_confirm_window(Some(handle.into()), cx);
    cx.default_global::<OpenConfirm>().0 = Some((handle, entity.downgrade()));
    // Every close ends up here. Only Keep marks the entity, so a dismissed
    // dialog fails safe.
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
            // Source and pool name come back too, or a workspace apply's revert
            // would run the new look's shader under the old switch.
            s.post_shader.source = prior.source.clone();
            s.post_shader.name = prior.name.clone();
        });
        crate::workspace::apply_post_shader(cx);
        if let Some(on_reverted) = confirm.on_reverted.take() {
            on_reverted(cx);
        }
    })
    .detach();
}

const CONTEXT: &str = "ShaderConfirm";

actions!(shader_confirm, [Keep, Revert]);

pub fn bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("enter", Keep, Some(CONTEXT)),
        KeyBinding::new("escape", Revert, Some(CONTEXT)),
    ]
}

struct ShaderConfirm {
    /// A revert restores the switch and the source trio. The all-windows
    /// option, routes and hand-set slots are never written back, so a route
    /// dragged meanwhile survives.
    prior: PostShaderConfig,
    player: EntityId,
    now_art: Entity<NowPlayingArt>,
    backdrop: WindowBackdrop,
    /// Set by Keep alone; the release hook reads it.
    kept: bool,
    focus: FocusHandle,
    on_reverted: Option<OnReverted>,
    _backdrop_changed: Subscription,
}

impl ShaderConfirm {
    fn new(
        prior: PostShaderConfig,
        player: EntityId,
        now_art: Entity<NowPlayingArt>,
        on_reverted: impl FnOnce(&mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> Self {
        let _backdrop_changed = cx.observe(&now_art, |_, _, cx| cx.notify());
        ShaderConfirm {
            prior,
            player,
            now_art,
            backdrop: WindowBackdrop::default(),
            kept: false,
            focus: cx.focus_handle(),
            on_reverted: Some(Box::new(on_reverted)),
            _backdrop_changed,
        }
    }

    /// Deferred: the buttons run inside this window's own update.
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
        let player = self.player;
        palette::note_focus(player, window.is_window_active(), cx);
        window.focus(&self.focus);
        panel::window_body(player, || {
            div()
                .flex()
                .flex_col()
                .size_full()
                .track_focus(&self.focus)
                .key_context(CONTEXT)
                .on_action(cx.listener(|this, _: &Keep, _, cx| {
                    this.kept = true;
                    this.close(cx);
                }))
                .on_action(cx.listener(|this, _: &Revert, _, cx| this.close(cx)))
                .bg(palette::bg_elevated())
                .text_color(palette::text_bright())
                .text_sm()
                .when_some(rox_core::settings::app_font(), |d, font| {
                    d.font_family(font)
                })
                .children(self.backdrop.layer(&self.now_art, window, cx))
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .p(tokens::SPACE_MD)
                        .gap(tokens::SPACE_MD)
                        .child(rox_i18n::t!("shader-confirm-question"))
                        .child(
                            kbd_line([
                                Seg::Text(rox_i18n::t!("shader-confirm-hint-before")),
                                Seg::Key(chord("Shift+X")),
                                Seg::Text(rox_i18n::t!("shader-confirm-hint-after")),
                            ])
                            .text_xs(),
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
                                    rox_i18n::t!("shader-confirm-revert"),
                                    icons::CLOSE,
                                    false,
                                    cx.listener(|this, _, _, cx| this.close(cx)),
                                ))
                                .child(small_button(
                                    rox_i18n::t!("shader-confirm-keep"),
                                    icons::CHECK,
                                    false,
                                    cx.listener(|this, _, _, cx| {
                                        this.kept = true;
                                        this.close(cx);
                                    }),
                                )),
                        ),
                )
                .into_any_element()
        })
    }
}
