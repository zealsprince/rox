//! The screen shader confirm: one small OS window opened after a risky
//! apply from the Shader settings page (the enable toggle, a file pick), the
//! display-settings pattern. Keep locks the change in; Revert or closing
//! the window restores the state from before the apply and persists it.
//! There's no countdown: a timer that reverts on its own is easy to miss,
//! and then the shader is off with nothing saying why. The window just
//! stays until it's answered, and since it's never shaded it remains the
//! way back however bad the shader looks. Hot reloads and the toggle hotkey
//! never come through here: the reload is the authoring loop, and the
//! hotkey is the escape hatch. The window registers itself with the
//! workspace's shading machinery so it is never shaded, whatever the
//! all-windows option says: it has to stay readable under exactly the
//! shader it exists to undo.

use gpui::{
    div, prelude::*, px, size, App, Bounds, Context, Entity, EntityId, Global, Subscription,
    WeakEntity, Window, WindowHandle,
};
use gpui_component::Root;

use rox_core::settings::{PostShaderConfig, Settings};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel;
use rox_panel_kit::ui::{chord, kbd_line, small_button, Seg};
use rox_services::backdrop::{NowPlayingArt, WindowBackdrop};

/// The caller's after-revert refresh, boxed for the entity to carry.
type OnReverted = Box<dyn FnOnce(&mut App)>;

/// The open confirm, if any: a second risky apply reuses it, keeping the
/// first dialog's prior as the baseline, so a run of quick changes still
/// reverts to the last state the user actually confirmed. Weak, or the
/// global itself would keep the entity from ever releasing.
#[derive(Default)]
struct OpenConfirm(Option<(WindowHandle<Root>, WeakEntity<ShaderConfirm>)>);

impl Global for OpenConfirm {}

/// Open the confirm for a change whose pre-apply state was `prior`, or
/// bring the open one forward. `now_art` is the shared art bake this
/// window's backdrop paints from. `on_reverted` runs after a revert so the
/// caller can refresh whatever mirrors the reverted fields.
pub fn open(
    prior: PostShaderConfig,
    player: EntityId,
    now_art: Entity<NowPlayingArt>,
    on_reverted: impl FnOnce(&mut App) + 'static,
    cx: &mut App,
) {
    if let Some((handle, confirm)) = cx.default_global::<OpenConfirm>().0.clone() {
        if confirm.upgrade().is_some() {
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
            // The source and the pool name come back too, or a workspace
            // apply's revert would put the old switch over the new look's
            // shader and run the very thing it was reverting.
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

struct ShaderConfirm {
    /// The config from before the apply, what a revert restores: the enable
    /// switch and the three ways a source gets picked (the file, the inline
    /// copy, the pool name). The all-windows option and the routes ride
    /// along untouched, so a route dragged while the window sits open
    /// survives a revert.
    prior: PostShaderConfig,
    /// The front workspace's player, for the window tint.
    player: EntityId,
    /// The shared art bake and this window's slice of the backdrop, so it
    /// backs with the playing track's art like every other window.
    now_art: Entity<NowPlayingArt>,
    backdrop: WindowBackdrop,
    /// Set by Keep alone. The release hook reads it to tell a confirmed
    /// close from every other way the window can go away.
    kept: bool,
    /// The caller's after-revert refresh, taken by the release hook.
    on_reverted: Option<OnReverted>,
    /// This window pumps its own frames, so the backdrop needs its own
    /// wake on a new bake.
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
            on_reverted: Some(Box::new(on_reverted)),
            _backdrop_changed,
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
        panel::window_body(player, || {
            div()
                .flex()
                .flex_col()
                .size_full()
                .bg(palette::bg_elevated())
                .text_color(palette::text_bright())
                .text_sm()
                .when_some(rox_core::settings::app_font(), |d, font| {
                    d.font_family(font)
                })
                // The backdrop paints first, under the copy, so a thinned
                // surface backs with the playing track's art like every
                // other window rather than sinking into black.
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
