//! The welcome window: one OS window opened over the primary workspace on
//! the first launch (no settings file yet), and any time from the
//! Application menu's Welcome entry. A short tour, each section a pointer
//! rather than a manual: where music comes in, how panels move, the
//! quick-play chord, and where the look lives. Beside the tour, the
//! quick-start column: the shipped workspaces as picture tiles, one click
//! dressing the main window in a whole look.

use std::time::Duration;

use gpui::{
    canvas, div, img, prelude::*, px, size, svg, Animation, AnimationExt, App, Bounds, Context,
    Div, Global, MouseButton, ObjectFit, Pixels, SharedString, Subscription, Window, WindowHandle,
};
use gpui_component::Root;

use rox_core::settings::app_font;
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel::{self, AppState};
use rox_panel_kit::ui::{chord, kbd_line, section, small_button, Seg, SECTION_GAP};
use rox_services::backdrop::WindowBackdrop;

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
    let bounds = Bounds::centered(None, size(px(1160.), px(740.)), cx);
    let handle = rox_panel_api::panel::open_child_window(
        cx,
        "rox - Welcome",
        bounds,
        Some(size(px(720.), px(480.))),
        move |_window, cx| cx.new(|cx| WelcomeWindow::new(state, cx)),
    );
    cx.set_global(OpenWelcome(handle));
}

struct WelcomeWindow {
    /// The shared state: the library the add-folder button scans into and
    /// the art bake the backdrop paints from.
    state: AppState,
    backdrop: WindowBackdrop,
    /// The shipped workspaces as the quick-start tiles show them: name, the
    /// author their card credits, and when previews ship, their asset paths
    /// and aspect ratios per theme side. Read once on open; the render loop
    /// must not reparse the embedded bundles per frame. Render picks the
    /// live theme's side, so the tiles follow a flip while the window is up.
    workspaces: Vec<Tile>,
    /// The tile the pointer is over, if any: its preview shows in color
    /// while the rest sit desaturated.
    hovered_tile: Option<usize>,
    /// The tile column's laid-out width, measured by a probe canvas each
    /// paint. The grid splits it into however many tile columns fit, and
    /// the hover pan's pixel math needs the resulting tile width. Seeded
    /// with the default window's share; the first paint corrects it.
    tiles_width: f32,
    /// This window pumps its own frames, so the backdrop needs its own
    /// wake on a new bake.
    _backdrop_changed: Subscription,
}

impl WelcomeWindow {
    fn new(state: AppState, cx: &mut Context<Self>) -> Self {
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        // A header that doesn't parse falls back to the frame's own
        // aspect, which renders the picture static.
        fn sized(path: SharedString) -> (SharedString, f32) {
            let aspect = rox_design::assets::png_aspect(&path).unwrap_or(FRAME_ASPECT);
            (path, aspect)
        }
        let workspaces = crate::workspaces::shipped()
            .into_iter()
            .map(|entry| Tile {
                name: SharedString::from(entry.name.clone()),
                // The list already parsed the bundle to build itself, so the
                // credit costs nothing on top.
                author: entry.author.map(SharedString::from),
                previews: TilePreviews {
                    dark: entry.preview_dark.map(sized),
                    light: entry.preview_light.map(sized),
                },
            })
            .collect();
        WelcomeWindow {
            state,
            backdrop: WindowBackdrop::default(),
            workspaces,
            hovered_tile: None,
            tiles_width: 458.,
            _backdrop_changed,
        }
    }
}

/// A section's body line, the pages' muted copy register.
fn line(text: impl Into<SharedString>) -> Div {
    div().text_color(palette::text_muted()).child(text.into())
}

/// One quick-start tile as the window holds it: what the workspace is
/// called, who made it when their card says, and the pictures it shows.
struct Tile {
    name: SharedString,
    author: Option<SharedString>,
    previews: TilePreviews,
}

/// One tile's preview pair: the asset path and aspect ratio per theme
/// side, resolved once on open. Both sides fall back to a bundle's plain
/// unthemed picture in the asset lookup, so a pair is either both set or
/// both None until themed shots ship.
struct TilePreviews {
    dark: Option<(SharedString, f32)>,
    light: Option<(SharedString, f32)>,
}

impl TilePreviews {
    /// The side a theme mode shows.
    fn pick(&self, mode: palette::Mode) -> Option<(SharedString, f32)> {
        match mode {
            palette::Mode::Dark => self.dark.clone(),
            palette::Mode::Light => self.light.clone(),
        }
    }
}

/// The tile frame's shape: every preview crops to a 16:9 window of the
/// column's width, so the column reads as a uniform reel whatever each
/// screenshot's own proportions are.
const FRAME_ASPECT: f32 = 16. / 9.;

/// A quick-start tile: the workspace's preview picture over its name, one
/// click applying the whole look to the main window. The tile fills the
/// column and `width` is that column's measured width, which the pixel
/// math for the hover pan needs. A workspace without a picture keeps the
/// tile's shape with a quiet placeholder block.
fn workspace_tile(
    name: SharedString,
    author: Option<SharedString>,
    preview: Option<(SharedString, f32)>,
    hovered: bool,
    width: f32,
    on_click: impl Fn(&gpui::MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Div {
    let frame_height = (width / FRAME_ASPECT).round();
    let picture = div()
        .w_full()
        .h(px(frame_height))
        .flex_none()
        .relative()
        .rounded(tokens::RADIUS)
        .overflow_hidden()
        .bg(palette::bg_control())
        .map(|d| match preview {
            // The preview reads in color only under the pointer; the rest
            // sit desaturated so the hovered look stands out. The picture
            // renders at its real scaled height with the top edge showing,
            // and hovering pans it down its full extent and back; the
            // raised-cosine easing starts and ends at that resting top, so
            // the drift picks up and loops without a jump.
            Some((path, aspect)) if width / aspect > frame_height => {
                let height = (width / aspect).round();
                let pan = height - frame_height;
                // The element carries the picture's exact aspect, so Fill
                // paints it edge to edge. Cover's ratio comparison would
                // sit on a knife edge here and flip its centering between
                // frames, a horizontal jitter while panning; the offset
                // rounds to whole pixels for the same reason.
                let frame = move |offset: f32| {
                    img(path.clone())
                        .absolute()
                        .left(px(0.))
                        .top(px(-offset))
                        .w_full()
                        .h(px(height))
                        .object_fit(ObjectFit::Fill)
                        .grayscale(!hovered)
                        .rounded(tokens::RADIUS)
                };
                if hovered {
                    // Sweep time scales with the distance so a tall
                    // portrait shot drifts at the same pace as a squat one.
                    let duration = Duration::from_secs_f32((pan / 12.).clamp(4., 16.));
                    d.child(
                        frame(0.).with_animation(
                            "pan",
                            Animation::new(duration)
                                .repeat()
                                .with_easing(|t| 0.5 - 0.5 * (t * std::f32::consts::TAU).cos()),
                            move |el, delta| el.top(px(-(delta * pan).round())),
                        ),
                    )
                } else {
                    d.child(frame(0.))
                }
            }
            // A picture wider than the frame, or one whose header didn't
            // parse: nothing to pan through, a static cover crop.
            Some((path, _)) => d.child(
                img(path)
                    .size_full()
                    .overflow_hidden()
                    .object_fit(ObjectFit::Cover)
                    .grayscale(!hovered)
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
        .w(px(width))
        .flex()
        .flex_col()
        .flex_none()
        .gap(tokens::SPACE_XS)
        .cursor_pointer()
        .hover(|d| d.opacity(0.85))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(picture)
        .child(div().text_color(palette::text_muted()).child(name))
        // Somebody made this look; their name rides under it wherever it
        // shows, quieter than the workspace's own.
        .when_some(author, |d, author| {
            d.child(
                div()
                    .text_xs()
                    .text_color(palette::text_faint())
                    .child(SharedString::from(format!("by {author}"))),
            )
        })
}

impl Render for WelcomeWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The window renders under the player's art tint like the
        // workspace it opened over, and claims the widget theme while it
        // holds focus, so the tour reads in the playing track's colors.
        let player = self.state.player.entity_id();
        palette::note_focus(player, window.is_window_active(), cx);

        panel::window_body(player, || {
            let add_folder = small_button(
                "Add Folder",
                icons::FOLDER_PLUS,
                false,
                cx.listener(|this, _, _, cx| {
                    rox_services::catalog::browse(&this.state.library, cx);
                }),
            );

            let tour = div()
                .id("welcome-tour")
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                // Prose stops stretching past a readable measure; past the
                // cap the window's extra room all goes to the tiles.
                .max_w(px(560.))
                .h_full()
                .min_h_0()
                .overflow_y_scroll()
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
                    "Get Started",
                    None,
                    line(
                        "Pick a workspace from the quick start on the right, or close \
                     this window and build your own player from scratch.",
                    ),
                ))
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
                        .child(kbd_line([
                            Seg::Text(
                                "Every surface is a panel, and the menubar's Panels menu \
                             opens more of them. If the menubar is hidden, hold"
                                    .into(),
                            ),
                            Seg::Key("Alt".into()),
                            Seg::Text("to bring it back.".into()),
                        ]))
                        .child(kbd_line([
                            Seg::Text("Drag a tab to rearrange, or hold middle mouse or".into()),
                            Seg::Key("Alt".into()),
                            Seg::Text(
                                "+ left click anywhere in a panel. Drop one outside the \
                             window and it becomes its own window."
                                    .into(),
                            ),
                        ])),
                ))
                .child(section(
                    "Playback",
                    None,
                    kbd_line([
                        Seg::Key(chord("P")),
                        Seg::Text("opens quick play: type a track, hit".into()),
                        Seg::Key("Enter".into()),
                        Seg::Text("and it plays.".into()),
                        Seg::Key("Space".into()),
                        Seg::Text("toggles playback;".into()),
                        Seg::Key("Left".into()),
                        Seg::Text("and".into()),
                        Seg::Key("Right".into()),
                        Seg::Text("seek.".into()),
                    ]),
                ))
                .child(section(
                    "Make It Yours",
                    None,
                    kbd_line([
                        Seg::Key(chord(",")),
                        Seg::Text(
                            "opens settings: the palette, transparency, and behavior. \
                         Save an arrangement as a layout; a workspace bundles layouts \
                         and palette into one shareable look."
                                .into(),
                        ),
                    ]),
                ))
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::text_faint())
                        .child("This window is here any time under Application > Welcome."),
                );

            // The tiles size to the column but the pan math needs pixels,
            // so a probe measures the laid-out width every paint and wakes
            // the view when a resize moves it. Next frame renders at the
            // corrected width; one frame of lag during a live drag.
            let tiles_width = self.tiles_width;
            let entity = cx.entity().downgrade();
            let probe = canvas(
                |_, _, _| {},
                move |bounds: Bounds<Pixels>, _, window, _| {
                    let measured = f32::from(bounds.size.width);
                    if (measured - tiles_width).abs() > 0.5 {
                        let entity = entity.clone();
                        window.on_next_frame(move |_, cx| {
                            entity
                                .update(cx, |this, cx| {
                                    this.tiles_width = measured;
                                    cx.notify();
                                })
                                .ok();
                        });
                    }
                },
            )
            .absolute()
            .inset_0();

            // The measured width splits into however many tile columns fit
            // at a comfortable size, so a wide window reflows the reel
            // side by side instead of inflating one giant column. A lone
            // column still caps where the screenshots would upscale past
            // their sources and turn soft.
            let gap = f32::from(tokens::SPACE_SM);
            let columns = (tiles_width / 400.).floor().max(1.);
            let tile_width = (((tiles_width - gap * (columns - 1.)) / columns).min(640.)).floor();

            // The quick-start column: every shipped workspace as a picture
            // tile, wrapping into the columns computed above, the reel
            // scrolling when it outgrows the window. Applying goes through
            // the frontmost workspace window at app level, since this
            // window has no workspace of its own.
            let tiles = div()
                .id("welcome-workspaces")
                .relative()
                .flex()
                .flex_row()
                .flex_wrap()
                // Wrapped flex lines stretch apart to fill the column by
                // default; pack them at the top instead.
                .content_start()
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .gap(tokens::SPACE_SM)
                .child(probe)
                .children(
                    self.workspaces
                        .iter()
                        .enumerate()
                        .map(|(i, tile)| {
                            let apply = tile.name.clone();
                            workspace_tile(
                                tile.name.clone(),
                                tile.author.clone(),
                                tile.previews.pick(palette::mode()),
                                self.hovered_tile == Some(i),
                                tile_width,
                                cx.listener(move |_, _, window, cx| {
                                    crate::workspace::apply_workspace_to_front(&apply, cx);
                                    // Picking a look is the end of the tour, so close
                                    // out to the freshly dressed main window.
                                    window.remove_window();
                                }),
                            )
                            .id(("welcome-tile", i))
                            .on_hover(cx.listener(
                                move |this, hovered: &bool, _, cx| {
                                    if *hovered {
                                        this.hovered_tile = Some(i);
                                    } else if this.hovered_tile == Some(i) {
                                        this.hovered_tile = None;
                                    }
                                    cx.notify();
                                },
                            ))
                        }),
                );

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

            // The tiles take whatever the tour doesn't, growing with the
            // window; the grid math above decides how the room is spent.
            let quick_start = section("Quick Start", None, body)
                .flex_1()
                .min_w_0()
                .h_full()
                .min_h_0();

            let page = div()
                .flex()
                .flex_row()
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
                .when_some(app_font(), |d, font| d.font_family(font))
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
                .into_any_element()
        })
    }
}
