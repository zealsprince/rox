//! The about window: one OS window opened from the Application menu beside
//! Welcome. The build's identity - logo, name, running version - a link
//! back to the project, and the update check. The check is notify only: it
//! reports a newer release and links to its page, it never downloads or
//! installs. The daily launch check has its own toggle over in settings
//! under Behavior; the button here checks now either way.

use gpui::{
    div, prelude::*, px, size, svg, AnyElement, App, Bounds, Context, Div, Global, MouseButton,
    SharedString, Subscription, Window, WindowHandle,
};
use gpui_component::Root;

use crate::assets::icons;
use crate::backdrop::WindowBackdrop;
use crate::design::{palette, tokens};
use crate::panel::AppState;
use crate::settings::ui::{small_button, SECTION_GAP};
use crate::settings::{self, Settings};
use crate::startup::updates;

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
    let bounds = Bounds::centered(None, size(px(820.), px(240.)), cx);
    let handle = crate::panel::open_fixed_window(cx, "rox - About", bounds, move |_window, cx| {
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
        match &settings.update_cache {
            Some(cache) => {
                let release = updates::Release {
                    version: cache.latest.clone(),
                    url: cache.url.clone(),
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
    /// This window pumps its own frames, so the backdrop needs its own
    /// wake on a new bake.
    _backdrop_changed: Subscription,
}

impl AboutWindow {
    fn new(state: AppState, cx: &mut Context<Self>) -> Self {
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        AboutWindow {
            state,
            backdrop: WindowBackdrop::default(),
            update_check: UpdateCheck::from_cache(&Settings::load()),
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
                        Settings::update(move |s| s.update_cache = Some(entry));
                        if release.is_new() {
                            UpdateCheck::Available(release)
                        } else {
                            UpdateCheck::UpToDate
                        }
                    }
                    Err(e) => {
                        eprintln!("update check: {e}");
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
        let checking = matches!(self.update_check, UpdateCheck::Checking);

        // The status line beside the button, one wording per check state.
        // The available state hangs a link to the release page off its tail.
        let status: Option<AnyElement> = match &self.update_check {
            UpdateCheck::Idle => None,
            UpdateCheck::Checking => Some(line("Checking...").into_any_element()),
            UpdateCheck::UpToDate => Some(line("You're on the latest version").into_any_element()),
            UpdateCheck::Failed => Some(line("Couldn't reach GitHub").into_any_element()),
            UpdateCheck::Available(release) => {
                let url = release.url.clone();
                Some(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(tokens::SPACE_SM)
                        .child(line(format!("Version {} is available", release.version)))
                        .child(small_button(
                            "Get It",
                            icons::EXTERNAL_LINK,
                            false,
                            cx.listener(move |_, _, _, cx| cx.open_url(&url)),
                        ))
                        .into_any_element(),
                )
            }
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
                    .size(px(192.))
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
                    // The page's own surface over the backdrop, the same
                    // one the settings pages sit on: opaque at full
                    // surface opacity, so the art only reads through as
                    // the surfaces thin, never straight under the copy.
                    .bg(palette::bg_elevated())
                    .p(tokens::SPACE_MD)
                    .child(page),
            )
    }
}
