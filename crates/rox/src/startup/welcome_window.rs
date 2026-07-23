//! The welcome window: one OS window opened over the primary workspace on
//! the first launch (no settings file yet), and any time from the
//! Application menu's Welcome entry. A short tour, each section a pointer
//! rather than a manual: where music comes in, how panels move, the
//! quick-play chord, and where the look lives. Beside the tour, the
//! quick-start column: the shipped workspaces as picture tiles, one click
//! dressing the main window in a whole look.

use gpui::{
    div, img, prelude::*, px, size, svg, App, Bounds, Context, Div, Global, MouseButton, ObjectFit,
    SharedString, Subscription, Window, WindowHandle,
};
use gpui_component::Root;

use crate::assets::icons;
use crate::backdrop::WindowBackdrop;
use crate::design::{palette, tokens};
use crate::panel::AppState;
use crate::settings;
use crate::settings::ui::{section, small_button, SECTION_GAP};

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
    let bounds = Bounds::centered(None, size(px(960.), px(640.)), cx);
    let handle = crate::panel::open_child_window(
        cx,
        "rox - Welcome",
        bounds,
        Some(size(px(720.), px(480.))),
        move |_window, cx| cx.new(|cx| WelcomeWindow::new(state, cx)),
    );
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
    /// The shipped workspaces as the quick-start tiles show them: name and
    /// the preview picture's asset path, when one ships. Read once on open;
    /// the render loop must not reparse the embedded bundles per frame.
    workspaces: Vec<(SharedString, Option<SharedString>)>,
    /// This window pumps its own frames, so the backdrop needs its own
    /// wake on a new bake.
    _backdrop_changed: Subscription,
}

impl WelcomeWindow {
    fn new(state: AppState, cx: &mut Context<Self>) -> Self {
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        let workspaces = crate::workspaces::shipped()
            .into_iter()
            .map(|entry| (SharedString::from(entry.bundle.name.clone()), entry.preview))
            .collect();
        WelcomeWindow {
            state,
            backdrop: WindowBackdrop::default(),
            workspaces,
            _backdrop_changed,
        }
    }
}

/// A section's body line, the pages' muted copy register.
fn line(text: impl Into<SharedString>) -> Div {
    div().text_color(palette::text_muted()).child(text.into())
}

/// A quick-start tile: the workspace's preview picture over its name, one
/// click applying the whole look to the main window. A workspace without a
/// picture keeps the tile's shape with a quiet placeholder block.
fn workspace_tile(
    name: SharedString,
    preview: Option<SharedString>,
    on_click: impl Fn(&gpui::MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    let picture = div()
        .w_full()
        .h(px(100.))
        .flex_none()
        .rounded(tokens::RADIUS)
        .overflow_hidden()
        .bg(palette::bg_control())
        .map(|d| match preview {
            Some(path) => d.child(
                img(path)
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .rounded(tokens::RADIUS),
            ),
            None => d.flex().items_center().justify_center().child(
                svg()
                    .path(icons::APP_WINDOW)
                    .size(px(20.))
                    .text_color(palette::text_faint()),
            ),
        });
    div()
        .w(px(178.))
        .flex()
        .flex_col()
        .flex_none()
        .gap(tokens::SPACE_XS)
        .cursor_pointer()
        .hover(|d| d.opacity(0.85))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(picture)
        .child(div().text_color(palette::text_muted()).child(name))
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

        let tour = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
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

        // The quick-start column: every shipped workspace as a picture
        // tile, two to a row so they all show without scrolling; the
        // scroll only kicks in when the list outgrows the window anyway.
        // Applying goes through the frontmost workspace window at app
        // level, since this window has no workspace of its own.
        let tiles = div()
            .id("welcome-workspaces")
            .flex()
            .flex_row()
            .flex_wrap()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .gap(tokens::SPACE_SM)
            .children(self.workspaces.iter().map(|(name, preview)| {
                let apply = name.clone();
                workspace_tile(
                    name.clone(),
                    preview.clone(),
                    cx.listener(move |_, _, _, cx| {
                        crate::workspace::apply_workspace_to_front(&apply, cx);
                    }),
                )
            }));

        let body = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap(tokens::SPACE_SM)
            .child(tiles)
            .child(
                div()
                    .text_xs()
                    .text_color(palette::text_faint())
                    .child("Picking one replaces the main window's look."),
            );

        let quick_start = section("Quick Start", None, body)
            .w(px(364.))
            .flex_none()
            .h_full()
            .min_h_0();

        let page = div()
            .flex()
            .flex_row()
            .items_start()
            .h_full()
            .gap(SECTION_GAP)
            .child(tour)
            .child(quick_start);

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
