//! The about window: one OS window opened from the Application menu beside
//! Welcome. The build's identity - logo, name, running version - a link
//! back to the project, and the update check with the updater behind it:
//! where the install can replace itself the announcement grows a Download
//! button, progress while the download runs, and a restart prompt once the
//! new build is in place; everywhere else it stays a link to the release
//! page. The daily launch check has its own toggle over in settings under
//! Application; the button here checks now either way.

use gpui::{
    div, prelude::*, px, size, svg, AnyElement, App, Bounds, Context, Div, Global, MouseButton,
    ScrollHandle, SharedString, Subscription, Window, WindowHandle,
};
use gpui_component::scroll::{Scrollbar, ScrollbarShow};
use gpui_component::Root;

use std::time::Duration;

use crate::startup::{updater, updates};
use rox_core::settings::{self, Settings};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel::{self, AppState};
use rox_panel_kit::ui::{small_button, SECTION_GAP};
use rox_services::backdrop::WindowBackdrop;

/// The project's home, where the source and the releases live.
const REPO: &str = "https://github.com/zealsprince/rox";

/// The author's site and profile, and the license text the copyleft notice
/// points at.
const SITE: &str = "https://zealsprince.com";
const PROFILE: &str = "https://github.com/zealsprince";
const LICENSE_URL: &str = "https://www.gnu.org/licenses/";

/// The open about window, if any: opening again focuses it instead of
/// stacking a second one, same as the welcome and settings windows.
struct OpenAbout(WindowHandle<Root>);

impl Global for OpenAbout {}

/// Open the about window, or bring the open one to the front. The state
/// carries the shared art bake the backdrop paints from.
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
    // Size the fixed window against the current font. The page is one set
    // shape at the stock 16px rem; a larger app font grows the rem-based text
    // past the 960x240 it was tuned at and strands the tail of the copy
    // offscreen. Growing the bounds with the text keeps the whole page in view.
    let scale = palette::font_scale();
    let bounds = Bounds::centered(None, size(px(960. * scale), px(240. * scale)), cx);
    let handle =
        rox_panel_api::panel::open_fixed_window(cx, "rox - About", bounds, move |_window, cx| {
            cx.new(|cx| AboutWindow::new(state, cx))
        });
    cx.set_global(OpenAbout(handle));
}

/// The update check as it moves along: nothing asked yet, the request in
/// flight, or a landed result. The result variants carry what the status
/// line beside the button shows.
enum UpdateCheck {
    Idle,
    Checking,
    UpToDate,
    Available(updates::Release),
    Failed,
}

impl UpdateCheck {
    /// What a freshly opened window shows: the last cached check mapped to
    /// up-to-date or an available release against the running build, or Idle
    /// when nothing has been checked yet.
    fn from_cache(settings: &Settings) -> Self {
        match &settings.session.update_cache {
            Some(cache) => {
                let release = updates::Release {
                    version: cache.latest.clone(),
                    url: cache.url.clone(),
                    assets: Vec::new(),
                };
                if release.is_new() {
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
    /// The shared state: the art bake the backdrop paints from.
    state: AppState,
    backdrop: WindowBackdrop,
    /// The update check, the status line's subject.
    update_check: UpdateCheck,
    /// The page scrolls as a fallback: the window sizes itself to the font,
    /// but a large enough font still outgrows the fixed titlebar and padding,
    /// so this keeps the tail of the copy reachable instead of clipped.
    scroll: ScrollHandle,
    /// This window pumps its own frames, so the backdrop needs its own
    /// wake on a new bake.
    _backdrop_changed: Subscription,
}

impl AboutWindow {
    fn new(state: AppState, cx: &mut Context<Self>) -> Self {
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        // A launch-check auto-download may already be running when the
        // window opens; pick up its progress the same as one started here.
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

    /// Kick off the update check on the background executor, landing the
    /// result on the status line and refreshing the cache so it persists and
    /// a launch treats it as recent. Ignored while one is already in flight.
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

    /// Hand the release to the updater on the background executor and
    /// start mirroring its progress. The updater holds the one download
    /// slot, so a second ask while one runs is a no-op.
    fn download(release: &updates::Release, cx: &mut Context<Self>) {
        if let Some(job) = updater::begin(release) {
            cx.background_executor()
                .spawn(async move { job() })
                .detach();
        }
        Self::poll_update(cx);
        cx.notify();
    }

    /// The announcement row for a newer release. Where the install can
    /// replace itself it offers the download with the release page demoted
    /// to notes; everywhere else - a distro package, a read-only home, a
    /// platform without an artifact - the page link is the whole offer,
    /// notify-only as before.
    fn release_row(release: &updates::Release, note: String, cx: &mut Context<Self>) -> AnyElement {
        let url = release.url.clone();
        let row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .child(line(note));
        if updater::can_update() {
            let release = release.clone();
            row.child(small_button(
                "Download",
                icons::DOWNLOAD,
                false,
                cx.listener(move |_, _, _, cx| Self::download(&release, cx)),
            ))
            .child(small_button(
                "Release Notes",
                icons::EXTERNAL_LINK,
                false,
                cx.listener(move |_, _, _, cx| cx.open_url(&url)),
            ))
            .into_any_element()
        } else {
            row.child(small_button(
                "Get It",
                icons::EXTERNAL_LINK,
                false,
                cx.listener(move |_, _, _, cx| cx.open_url(&url)),
            ))
            .into_any_element()
        }
    }

    /// Repaint on a timer while the download runs: the progress lives in
    /// atomics the render reads, so the window just needs frames until the
    /// updater settles. The last tick paints the settled state on its way
    /// out.
    fn poll_update(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
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
        })
        .detach();
    }
}

/// A muted body line, the pages' copy register.
fn line(text: impl Into<SharedString>) -> Div {
    div().text_color(palette::text_muted()).child(text.into())
}

/// An inline link: accent, underlined, opening its URL on click. Sits in a
/// wrapping row beside the muted prose around it.
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

/// A muted paragraph that wraps text and inline links together, the license
/// prose's line register.
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
        // The window renders under the player's art tint like the
        // workspace it was opened from, and claims the widget theme while
        // it holds focus.
        let player = self.state.player.entity_id();
        palette::note_focus(player, window.is_window_active(), cx);

        panel::window_body(player, || {
            let checking = matches!(self.update_check, UpdateCheck::Checking);

            // The status line beside the button, one wording per state. The
            // updater's state outranks the check's: once a download moves or
            // lands, that's the story, whatever the last check said.
            let status: Option<AnyElement> = match updater::status() {
                updater::Status::Applied { version } => Some(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(tokens::SPACE_SM)
                        .child(line(format!("Version {version} is ready")))
                        .child(small_button(
                            "Restart Now",
                            icons::REFRESH_CW,
                            false,
                            cx.listener(|_, _, _, cx| cx.restart()),
                        ))
                        .into_any_element(),
                ),
                updater::Status::Downloading(progress) => Some(
                    line(format!("Downloading... {:.0}%", progress.fraction() * 100.))
                        .into_any_element(),
                ),
                updater::Status::Failed { error } => match &self.update_check {
                    // The release the download failed for is still the cached
                    // one, so the retry rides beside the reason.
                    UpdateCheck::Available(release) => Some(Self::release_row(
                        release,
                        format!("The update failed: {error}"),
                        cx,
                    )),
                    _ => Some(line(format!("The update failed: {error}")).into_any_element()),
                },
                updater::Status::Idle => match &self.update_check {
                    UpdateCheck::Idle => None,
                    UpdateCheck::Checking => Some(line("Checking...").into_any_element()),
                    UpdateCheck::UpToDate => {
                        Some(line("You're on the latest version").into_any_element())
                    }
                    UpdateCheck::Failed => Some(line("Couldn't reach GitHub").into_any_element()),
                    UpdateCheck::Available(release) => Some(Self::release_row(
                        release,
                        format!("Version {} is available", release.version),
                        cx,
                    )),
                },
            };

            let update_control = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_SM)
                .child(small_button(
                    "Check for Updates",
                    icons::REFRESH_CW,
                    checking,
                    cx.listener(|this, _, _, cx| this.check_for_updates(cx)),
                ))
                .when_some(status, |d, status| d.child(status));

            // The identity column beside the logo: name and version up top, then
            // the copyright, the copyleft notice, and where the source lives.
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
                    .child(line(format!("Version {}", updates::CURRENT))),
            )
            .child(
                prose()
                    .child("Copyright © 2026")
                    .child(link("Andrew Lake", SITE))
                    .child(link("(@zealsprince)", PROFILE)),
            )
            .child(
                prose()
                    .child("rox is free software under the GNU AGPLv3. The source is on")
                    .child(link("GitHub", REPO))
                    .child("."),
            )
            .child(
                prose()
                    .child("You should have received a copy of the license with this program. If not, see")
                    .child(link("gnu.org/licenses", LICENSE_URL))
                    .child("."),
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
                        // The logo is fixed px, not rem, so it holds while the
                        // copy beside it grows with the font. Track the app
                        // scale so it keeps pace and the balance holds.
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
                // The backdrop paints first, under the page; without it
                // translucent surfaces would sink into the window's own black
                // instead of the playing track's art.
                .children(self.backdrop.layer(&self.state.now_art, window, cx))
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .relative()
                        // The page's own surface over the backdrop, the same
                        // one the settings pages sit on: opaque at full
                        // surface opacity, so the art only reads through as
                        // the surfaces thin, never straight under the copy.
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
                        // The scrollbar rides over the page, the same overlay
                        // the settings pages use: it only bites when a large
                        // font pushes the copy past the window.
                        .child(div().absolute().inset_0().child(
                            Scrollbar::vertical(&self.scroll).scrollbar_show(ScrollbarShow::Always),
                        )),
                )
                .into_any_element()
        })
    }
}
