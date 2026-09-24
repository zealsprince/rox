//! The about window: the build's identity, a link back to the project, and
//! the update check with the updater behind it. Where the install can replace
//! itself the check offers a download and a restart; everywhere else it's a
//! link to the release page.

use gpui::{
    App, Bounds, Context, Div, Global, MouseButton, ScrollHandle, SharedString, Subscription,
    Window, WindowHandle, div, prelude::*, px, size, svg,
};
use gpui_component::Root;
use gpui_component::scroll::{Scrollbar, ScrollbarShow};

use std::time::Duration;

use crate::startup::{updater, updates};
use rox_core::settings::{self, Settings};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel::{self, AppState};
use rox_panel_kit::ui::{SECTION_GAP, SmallButton, small_button};
use rox_services::backdrop::WindowBackdrop;

const REPO: &str = "https://github.com/zealsprince/rox";

const SITE: &str = "https://zealsprince.com";
const PROFILE: &str = "https://github.com/zealsprince";
const LICENSE_URL: &str = "https://www.gnu.org/licenses/";

struct OpenAbout(WindowHandle<Root>);

impl Global for OpenAbout {}

pub fn open(state: AppState, cx: &mut App) {
    if let Some(open) = cx.try_global::<OpenAbout>() {
        let handle = open.0;
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    // Size the fixed window against the app font: it was tuned at 960x240 on
    // the stock 16px rem, and a larger font would push the copy offscreen.
    let scale = palette::font_scale();
    let bounds = Bounds::centered(None, size(px(960. * scale), px(240. * scale)), cx);
    let handle = rox_panel_api::panel::open_fixed_window(
        cx,
        rox_i18n::t!("about-window-title"),
        bounds,
        move |_window, cx| cx.new(|cx| AboutWindow::new(state, cx)),
    );
    cx.set_global(OpenAbout(handle));
}

enum UpdateCheck {
    Idle,
    Checking,
    UpToDate,
    Available(updates::Release),
    Failed,
}

impl UpdateCheck {
    fn from_cache(settings: &Settings) -> Self {
        match &settings.session.update_cache {
            Some(cache) => {
                let release = updates::Release {
                    version: cache.latest.clone(),
                    url: cache.url.clone(),
                    assets: Vec::new(),
                };
                if release.offered(settings) {
                    UpdateCheck::Available(release)
                } else {
                    UpdateCheck::UpToDate
                }
            }
            None => UpdateCheck::Idle,
        }
    }
}

struct AboutWindow {
    state: AppState,
    backdrop: WindowBackdrop,
    update_check: UpdateCheck,
    /// A fallback for fonts large enough to outgrow the sized window.
    scroll: ScrollHandle,
    /// This window pumps its own frames, so the backdrop needs its own wake.
    _backdrop_changed: Subscription,
}

impl AboutWindow {
    fn new(state: AppState, cx: &mut Context<Self>) -> Self {
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        // A launch-check auto-download may already be running.
        if matches!(updater::status(), updater::Status::Downloading(_)) {
            Self::poll_update(cx);
        }
        AboutWindow {
            state,
            backdrop: WindowBackdrop::default(),
            update_check: UpdateCheck::from_cache(&Settings::load()),
            scroll: ScrollHandle::new(),
            _backdrop_changed,
        }
    }

    /// Also refreshes the cache so a launch treats this check as recent.
    fn check_for_updates(&mut self, cx: &mut Context<Self>) {
        if matches!(self.update_check, UpdateCheck::Checking) {
            return;
        }
        self.update_check = UpdateCheck::Checking;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { updates::fetch_latest() })
                .await;
            this.update(cx, |this, cx| {
                this.update_check = match result {
                    Ok(release) => {
                        let entry = updates::cache(&release);
                        Settings::update(move |s| s.session.update_cache = Some(entry));
                        // The menubar chip reads a static off this cache, so recompute and repaint.
                        updates::refresh_available(&Settings::load());
                        cx.refresh_windows();
                        if release.is_new() {
                            UpdateCheck::Available(release)
                        } else {
                            UpdateCheck::UpToDate
                        }
                    }
                    Err(e) => {
                        log::warn!("update check: {e}");
                        UpdateCheck::Failed
                    }
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn download(release: &updates::Release, cx: &mut Context<Self>) {
        if let Some(job) = updater::begin(release) {
            cx.background_executor()
                .spawn(async move { job() })
                .detach();
        }
        Self::poll_update(cx);
        cx.notify();
    }

    fn release_buttons(release: &updates::Release, cx: &mut Context<Self>) -> Vec<SmallButton> {
        let url = release.url.clone();
        if updater::can_update() {
            let release = release.clone();
            vec![
                small_button(
                    rox_i18n::t!("about-release-notes"),
                    icons::EXTERNAL_LINK,
                    false,
                    cx.listener(move |_, _, _, cx| cx.open_url(&url)),
                ),
                small_button(
                    rox_i18n::t!("about-download"),
                    icons::DOWNLOAD,
                    false,
                    cx.listener(move |_, _, _, cx| Self::download(&release, cx)),
                ),
            ]
        } else {
            vec![small_button(
                rox_i18n::t!("about-get-it"),
                icons::EXTERNAL_LINK,
                false,
                cx.listener(move |_, _, _, cx| cx.open_url(&url)),
            )]
        }
    }

    fn check_button(&self, cx: &mut Context<Self>) -> SmallButton {
        small_button(
            rox_i18n::t!("about-check-for-updates"),
            icons::REFRESH_CW,
            false,
            cx.listener(|this, _, _, cx| this.check_for_updates(cx)),
        )
    }

    /// The progress lives in atomics, so the window only needs frames until the
    /// updater settles.
    fn poll_update(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
                let live = this.update(cx, |_, cx| {
                    cx.notify();
                    matches!(updater::status(), updater::Status::Downloading(_))
                });
                if !matches!(live, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }
}

fn line(text: impl Into<SharedString>) -> Div {
    div().text_color(palette::text_muted()).child(text.into())
}

fn link(text: impl Into<SharedString>, url: &'static str) -> Div {
    div()
        .text_color(palette::accent())
        .underline()
        .cursor_pointer()
        .hover(|d| d.text_color(palette::accent_hover()))
        .on_mouse_down(MouseButton::Left, move |_, _, cx: &mut App| {
            cx.open_url(url)
        })
        .child(text.into())
}

/// The period sits in a gapless row with the link, so no space opens before it.
fn link_end(text: impl Into<SharedString>, url: &'static str) -> Div {
    div().flex().flex_row().child(link(text, url)).child(".")
}

fn prose() -> Div {
    div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_x(px(4.))
        .text_color(palette::text_muted())
}

impl Render for AboutWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let player = self.state.player.entity_id();
        palette::note_focus(player, window.is_window_active(), cx);

        panel::window_body(player, || {
            // The updater's state outranks the check's: once a download runs or lands,
            // that's what the row says.
            let (note, buttons): (Option<SharedString>, Vec<SmallButton>) = match updater::status()
            {
                updater::Status::Applied { version } => (
                    Some(rox_i18n::t!("about-version-ready", version = version)),
                    vec![small_button(
                        rox_i18n::t!("about-restart-now"),
                        icons::POWER,
                        false,
                        cx.listener(|_, _, _, cx| updater::relaunch(cx)),
                    )],
                ),
                updater::Status::Downloading(progress) => (
                    match &self.update_check {
                        UpdateCheck::Available(release) => Some(rox_i18n::t!(
                            "about-version-available",
                            version = release.version.clone()
                        )),
                        _ => None,
                    },
                    vec![small_button(
                        rox_i18n::t!(
                            "about-downloading",
                            percent = (progress.fraction() * 100.).round() as u64
                        ),
                        icons::DOWNLOAD,
                        true,
                        |_, _, _| {},
                    )],
                ),
                updater::Status::Failed { error } => (
                    Some(rox_i18n::t!("about-update-failed", error = error)),
                    match &self.update_check {
                        UpdateCheck::Available(release) => Self::release_buttons(release, cx),
                        _ => vec![self.check_button(cx)],
                    },
                ),
                updater::Status::Idle => match &self.update_check {
                    UpdateCheck::Idle => (None, vec![self.check_button(cx)]),
                    UpdateCheck::Checking => (
                        None,
                        vec![small_button(
                            rox_i18n::t!("about-checking"),
                            icons::REFRESH_CW,
                            true,
                            |_, _, _| {},
                        )],
                    ),
                    UpdateCheck::UpToDate => (
                        Some(rox_i18n::t!("about-up-to-date")),
                        vec![self.check_button(cx)],
                    ),
                    UpdateCheck::Failed => (
                        Some(rox_i18n::t!("about-check-failed")),
                        vec![self.check_button(cx)],
                    ),
                    UpdateCheck::Available(release) => (
                        Some(rox_i18n::t!(
                            "about-version-available",
                            version = release.version.clone()
                        )),
                        Self::release_buttons(release, cx),
                    ),
                },
            };

            let update_control = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_SM)
                .when_some(note, |d, note| d.child(line(note)))
                .children(buttons);

            let identity = div()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_MD)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(tokens::SPACE_XS)
                        .child(
                            div()
                                .text_xl()
                                .text_color(palette::text_bright())
                                .child("rox"),
                        )
                        .child(line(rox_i18n::t!(
                            "about-version",
                            version = updates::CURRENT.to_string()
                        ))),
                )
                .child(
                    prose()
                        .child(rox_i18n::t!("about-copyright"))
                        .child(link("Andrew Lake", SITE))
                        .child(link("(@zealsprince)", PROFILE)),
                )
                .child(
                    prose()
                        .child(rox_i18n::t!("about-license-lead"))
                        .child(link_end("GitHub", REPO)),
                )
                .child(
                    prose()
                        .child(rox_i18n::t!("about-notice-lead"))
                        .child(link_end("gnu.org/licenses", LICENSE_URL)),
                )
                .child(update_control);

            let page = div()
                .flex()
                .flex_row()
                .items_start()
                .gap(SECTION_GAP)
                .child(
                    svg()
                        .path(icons::LOGO)
                        // Fixed px, so scale it with the font to keep pace with the copy.
                        .size(px(192. * palette::font_scale()))
                        .flex_none()
                        .text_color(palette::text_bright()),
                )
                .child(identity);

            div()
                .size_full()
                .flex()
                .flex_col()
                .bg(palette::bg_elevated())
                .text_color(palette::text_bright())
                .text_sm()
                .when_some(settings::app_font(), |d, font| d.font_family(font))
                .children(self.backdrop.layer(&self.state.now_art, window, cx))
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .relative()
                        .bg(palette::bg_elevated())
                        .child(
                            div()
                                .id("about-page")
                                .size_full()
                                .overflow_y_scroll()
                                .track_scroll(&self.scroll)
                                .p(tokens::SPACE_MD)
                                .child(page),
                        )
                        .child(div().absolute().inset_0().child(
                            Scrollbar::vertical(&self.scroll).scrollbar_show(ScrollbarShow::Always),
                        )),
                )
                .into_any_element()
        })
    }
}
