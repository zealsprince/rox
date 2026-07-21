//! The welcome window: one OS window opened over the primary workspace on
//! the first launch (no settings file yet), and any time from the
//! Application menu's Welcome entry. A short tour, each section a pointer
//! rather than a manual: where music comes in, how panels move, the
//! quick-play chord, and where the look lives.

use gpui::{
    div, prelude::*, px, size, svg, App, Bounds, Context, Div, Global, SharedString, Subscription,
    TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions,
};
use gpui_component::Root;

use crate::assets::icons;
use crate::backdrop::WindowBackdrop;
use crate::design::{palette, tokens};
use crate::panel::AppState;
use crate::settings;
use crate::settings_ui::{section, small_button, SECTION_GAP};

/// The open welcome window, if any: opening again focuses it instead of
/// stacking a second one, same as the settings window.
struct OpenWelcome(WindowHandle<Root>);

impl Global for OpenWelcome {}

/// Open the welcome window, or bring the open one to the front. The state
/// carries the library the add-folder button scans into and the shared
/// art bake for the backdrop.
pub fn open(state: AppState, cx: &mut App) {
    if let Some(open) = cx.try_global::<OpenWelcome>() {
        let handle = open.0;
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    let bounds = Bounds::centered(None, size(px(520.), px(560.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(400.), px(400.))),
        titlebar: Some(TitlebarOptions {
            title: Some("rox - Welcome".into()),
            ..Default::default()
        }),
        app_id: Some(crate::APP_ID.into()),
        ..Default::default()
    };
    let handle = cx
        .open_window(options, |window, cx| {
            // The Wayland backend ignores the creation-time titlebar
            // title; only set_window_title reaches the compositor.
            window.set_window_title("rox - Welcome");
            let view = cx.new(|cx| WelcomeWindow::new(state, cx));
            cx.new(|cx| Root::new(view, window, cx))
        })
        .expect("failed to open the welcome window");
    cx.set_global(OpenWelcome(handle));
}

/// The platform's primary modifier as the shortcut labels show it.
fn chord(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("Cmd+{key}")
    } else {
        format!("Ctrl+{key}")
    }
}

struct WelcomeWindow {
    /// The shared state: the library the add-folder button scans into and
    /// the art bake the backdrop paints from.
    state: AppState,
    backdrop: WindowBackdrop,
    /// This window pumps its own frames, so the backdrop needs its own
    /// wake on a new bake.
    _backdrop_changed: Subscription,
}

impl WelcomeWindow {
    fn new(state: AppState, cx: &mut Context<Self>) -> Self {
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        WelcomeWindow {
            state,
            backdrop: WindowBackdrop::default(),
            _backdrop_changed,
        }
    }
}

/// A section's body line, the pages' muted copy register.
fn line(text: impl Into<SharedString>) -> Div {
    div().text_color(palette::text_muted()).child(text.into())
}

impl Render for WelcomeWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let add_folder = small_button(
            "Add Folder",
            icons::FOLDER_PLUS,
            false,
            cx.listener(|this, _, _, cx| {
                this.state
                    .library
                    .update(cx, |library, cx| library.browse(cx));
            }),
        );

        let page = div()
            .flex()
            .flex_col()
            .gap(SECTION_GAP)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_SM)
                    .child(
                        svg()
                            .path(icons::LOGO)
                            .size(px(44.))
                            .text_color(palette::text_bright()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(tokens::SPACE_XS)
                            .child(div().text_lg().child("Welcome to rox"))
                            .child(line("Foobar if it was made in 20XX.")),
                    ),
            )
            .child(section(
                "Music",
                Some(add_folder.into_any_element()),
                line(
                    "Add a folder and rox scans it into the library; the files \
                     stay where they are. Folders live in settings under library.",
                ),
            ))
            .child(section(
                "Panels",
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_SM)
                    .child(line(
                        "Every surface is a panel, and the menubar's Panels menu \
                         opens more of them.",
                    ))
                    .child(line(
                        "Drag a tab to rearrange, or hold middle mouse or Alt+left \
                         click anywhere in a panel. Drop one outside the window and \
                         it becomes its own window.",
                    )),
            ))
            .child(section(
                "Playback",
                None,
                line(format!(
                    "{} opens quick play: type a track, hit Enter, it plays. \
                     Space toggles playback, left and right seek.",
                    chord("P")
                )),
            ))
            .child(section(
                "Make It Yours",
                None,
                line(format!(
                    "Settings ({}) holds the palette, transparency, and behavior. \
                     Save an arrangement as a layout; a workspace bundles layouts \
                     and palette into one shareable look.",
                    chord(",")
                )),
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(palette::text_faint())
                    .child("This window is here any time under Application > Welcome."),
            );

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette::bg_elevated())
            .text_color(palette::text_bright())
            .text_sm()
            .when_some(settings::app_font(), |d, font| d.font_family(font))
            // The backdrop paints first, under the page; without it
            // translucent surfaces would sink into the window's own
            // black instead of the playing track's art.
            .children(self.backdrop.layer(&self.state.now_art, window, cx))
            .child(div().flex_1().min_h_0().p(tokens::SPACE_MD).child(page))
    }
}
