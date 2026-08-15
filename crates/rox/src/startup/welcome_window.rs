//! The welcome window: one OS window opened over the primary workspace on
//! the first launch (no settings file yet), and any time from the
//! Application menu's Welcome entry. Two stages, each filling the window on
//! its own: a card per thing worth knowing, every one a pointer rather than
//! a manual, then the quick start, the shipped workspaces as picture tiles
//! with one click dressing the main window in a whole look.

use std::time::Duration;

use gpui::{
    canvas, div, img, point, prelude::*, px, size, svg, Animation, AnimationExt, AnyElement, App,
    Bounds, Context, Div, FocusHandle, Global, KeyDownEvent, MouseButton, ObjectFit, Pixels,
    ScrollHandle, SharedString, Subscription, Window, WindowHandle,
};
use gpui_component::scroll::{Scrollbar, ScrollbarShow};
use gpui_component::Root;

use rox_core::settings::app_font;
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel::{self, AppState};
use rox_panel_kit::ui::{chord, kbd_line, small_button, Seg, SECTION_GAP};
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
    // Wide enough for three cards across without any of them turning into a
    // column of short lines, which is the shelf's three tile columns too.
    let bounds = Bounds::centered(None, size(px(1240.), px(660.)), cx);
    let handle = rox_panel_api::panel::open_child_window(
        cx,
        "rox - Welcome",
        bounds,
        Some(size(px(700.), px(460.))),
        move |window, cx| cx.new(|cx| WelcomeWindow::new(state, window, cx)),
    );
    cx.set_global(OpenWelcome(handle));
}

/// The tour's stages, in the order they're taken. Two of them: a headline
/// over the cards, then the shelf, which is the only one with enough in it
/// to scroll.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Welcome,
    Workspaces,
}

const STAGES: [Stage; 2] = [Stage::Welcome, Stage::Workspaces];

impl Stage {
    /// The stage's headline.
    fn title(self) -> &'static str {
        match self {
            Stage::Welcome => "Welcome to rox",
            Stage::Workspaces => "Quick Start",
        }
    }

    /// The line under the headline, the one thing the stage is about.
    fn lead(self) -> &'static str {
        match self {
            Stage::Welcome => "Foobar if it was made in 20XX.",
            Stage::Workspaces => {
                "Pick a workspace and the main window puts it on: layouts, palette, the whole look."
            }
        }
    }
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
    /// The tile grid's laid-out width, measured by a probe canvas each
    /// paint. The grid splits it into however many tile columns fit, and
    /// the hover pan's pixel math needs the resulting tile width. Seeded
    /// with the default window's share; the first paint corrects it.
    tiles_width: f32,
    /// Which stage of [`STAGES`] is up, the tour's whole position.
    stage: usize,
    /// The stage body's scroll position, shared with its scrollbar. One
    /// handle for every stage, wound back to the top on each step so a long
    /// stage can't hand the next one its own offset.
    scroll: ScrollHandle,
    /// The window root's own focus. Nothing here takes typing; it's what
    /// puts the arrow keys on the dispatch path.
    focus: FocusHandle,
    /// This window pumps its own frames, so the backdrop needs its own
    /// wake on a new bake.
    _backdrop_changed: Subscription,
}

impl WelcomeWindow {
    fn new(state: AppState, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        // The tour steps on the arrow keys, and nothing else in the window
        // wants the keyboard, so the root takes focus as it opens.
        let focus = cx.focus_handle();
        window.focus(&focus);
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
            stage: 0,
            scroll: ScrollHandle::new(),
            focus,
            _backdrop_changed,
        }
    }

    /// Move the tour by `delta` stages, stopping at either end.
    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let last = STAGES.len() as isize - 1;
        let next = (self.stage as isize + delta).clamp(0, last) as usize;
        self.go_to(next, cx);
    }

    /// Land on a stage: the body starts at the top, and no tile is under
    /// the pointer until the pointer says so.
    fn go_to(&mut self, stage: usize, cx: &mut Context<Self>) {
        if stage == self.stage {
            return;
        }
        self.stage = stage;
        self.scroll.set_offset(point(px(0.), px(0.)));
        self.hovered_tile = None;
        cx.notify();
    }
}

/// A section's body line, the pages' muted copy register.
fn line(text: impl Into<SharedString>) -> Div {
    div().text_color(palette::text_muted()).child(text.into())
}

/// What a card asks for before its row shares out what's left. Three of
/// these plus their gaps is what the window opens wide enough to hold, and
/// it's the width the card's copy is measured at, which is what keeps a
/// card as tall as its own text.
const CARD_BASIS: f32 = 300.0;

/// The narrowest a tile column gets before the shelf drops one.
const MIN_TILE_W: f32 = 400.0;

/// The most tile columns the quick-start shelf ever lays out. Past three
/// the shelf reads as a contact sheet rather than a gallery, and a window
/// with room for more spends it on bigger pictures instead.
const MAX_TILE_COLUMNS: f32 = 3.0;

/// The widest a tile gets. The shipped previews are about 1400px across, so
/// this is roughly where a fullscreen shelf starts upscaling them on a 2x
/// display; a window wider than three of these centers its grid.
const MAX_TILE_W: f32 = 900.0;

/// One card on a stage: an icon and name over whatever the card is telling
/// you, on the panel surface so it sits proud of the page. Cards share a
/// row and split it evenly, so a stage reads as a few things beside each
/// other rather than one column of copy.
fn card(icon: &'static str, title: &'static str, body: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        // Grow and shrink from a real basis rather than from zero. The
        // basis is what the row wraps on and what the copy is measured at,
        // so a card comes out as tall as its own text; the zero floor is
        // what keeps a long line from setting the card's width instead.
        .flex_grow()
        .flex_shrink()
        .flex_basis(px(CARD_BASIS))
        .min_w_0()
        .gap(tokens::SPACE_SM)
        .p(tokens::SPACE_MD)
        .rounded(tokens::RADIUS)
        .border_1()
        .border_color(palette::border())
        .bg(palette::bg_panel())
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_XS)
                .child(
                    svg()
                        .path(icon)
                        .size(px(14.))
                        .flex_none()
                        .text_color(palette::text_muted()),
                )
                .child(title),
        )
        .child(body)
}

/// One row of cards, split evenly across the stage. A row rather than a
/// wrapping grid: a wrapped flex line takes its height from the container
/// instead of from its own cards, which leaves the first row of a grid
/// stretched to half the page. No cross-axis alignment, so the row stretches
/// its cards to the tallest of them and a row reads as a row.
fn cards(cards: impl IntoIterator<Item = Div>) -> Div {
    div()
        .flex()
        .flex_row()
        .gap(tokens::SPACE_MD)
        .children(cards)
}

/// The width the scrollbar rides in. The bar is an overlay, so a column
/// keeps this much clear on its right or the thumb sits under the content.
const SCROLL_LANE: f32 = 16.0;

/// A scrolling column paired with its scrollbar, the same overlay the about
/// window and the settings pages use. The caller sizes the wrapper into its
/// page and keeps the lane clear inside the column.
fn scroll_lane(column: impl IntoElement, scroll: &ScrollHandle) -> Div {
    div().relative().child(column).child(
        div()
            .absolute()
            .inset_0()
            .child(Scrollbar::vertical(scroll).scrollbar_show(ScrollbarShow::Always)),
    )
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
        .child(
            // Somebody made this look; their name rides on the same line as
            // the workspace's, quieter and off the baseline it sits on.
            div()
                .flex()
                .flex_row()
                .items_baseline()
                .gap(tokens::SPACE_XS)
                .child(div().text_color(palette::text_muted()).child(name))
                .when_some(author, |d, author| {
                    d.child(
                        div()
                            .text_xs()
                            .text_color(palette::text_faint())
                            .child(SharedString::from(format!("by {author}"))),
                    )
                }),
        )
}

impl WelcomeWindow {
    /// The stage's content under its headline. Every stage builds one of
    /// these; the shell around them is the same.
    fn stage_body(&self, stage: Stage, cx: &mut Context<Self>) -> AnyElement {
        match stage {
            Stage::Welcome => div()
                .flex()
                .flex_col()
                .gap(SECTION_GAP)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(tokens::SPACE_SM)
                        .child(line(
                            "A quick tour of where music comes in and where the look \
                             lives. It ends at the shelf of shipped workspaces, one \
                             click each.",
                        ))
                        .child(div().text_color(palette::text_faint()).child(kbd_line([
                            Seg::Text("Step through it with".into()),
                            Seg::Key("Left".into()),
                            Seg::Text("and".into()),
                            Seg::Key("Right".into()),
                            Seg::Text(", or the buttons below.".into()),
                        ]))),
                )
                .child(cards([
                    card(
                        icons::MUSIC,
                        "Music",
                        div()
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap(tokens::SPACE_SM)
                            .child(small_button(
                                "Add Folder",
                                icons::FOLDER_PLUS,
                                false,
                                cx.listener(|this, _, _, cx| {
                                    rox_services::catalog::browse(&this.state.library, cx);
                                }),
                            ))
                            .child(line(
                                "rox scans it into the library and the files stay where \
                                 they are. More folders go in settings under library.",
                            )),
                    ),
                    card(
                        icons::PLAY,
                        "Playback",
                        div()
                            .flex()
                            .flex_col()
                            .gap(tokens::SPACE_SM)
                            .child(kbd_line([
                                Seg::Key(chord("P")),
                                Seg::Text("opens quick play: type a track, hit".into()),
                                Seg::Key("Enter".into()),
                                Seg::Text("and it plays.".into()),
                            ]))
                            .child(kbd_line([
                                Seg::Key("Space".into()),
                                Seg::Text("toggles playback;".into()),
                                Seg::Key("Left".into()),
                                Seg::Text("and".into()),
                                Seg::Key("Right".into()),
                                Seg::Text("seek.".into()),
                            ])),
                    ),
                    card(
                        icons::SETTINGS,
                        "Settings",
                        div()
                            .flex()
                            .flex_col()
                            .gap(tokens::SPACE_SM)
                            .child(kbd_line([
                                Seg::Key(chord(",")),
                                Seg::Text(
                                    "opens settings: the palette, transparency, and \
                                     behavior."
                                        .into(),
                                ),
                            ]))
                            .child(line(
                                "Save an arrangement as a layout; a workspace bundles \
                                 layouts and palette into one shareable look.",
                            )),
                    ),
                ]))
                .child(cards([
                    card(
                        icons::LAYOUT_DASHBOARD,
                        "Panels",
                        div()
                            .flex()
                            .flex_col()
                            .gap(tokens::SPACE_SM)
                            .child(line(
                                "Every surface is a panel, and the menubar's Panels menu \
                                 opens more of them.",
                            ))
                            .child(line(
                                "Rearranging needs Design Mode, on by default at the top \
                                 of that menu. Off locks the layout, so a finished setup \
                                 can't be nudged.",
                            )),
                    ),
                    card(
                        icons::MOVE_HORIZONTAL,
                        "Rearranging",
                        div()
                            .flex()
                            .flex_col()
                            .gap(tokens::SPACE_SM)
                            .child(kbd_line([
                                Seg::Text("Drag a tab, or hold".into()),
                                Seg::Key("Middle Mouse".into()),
                                Seg::Text("or".into()),
                                Seg::Key("Alt".into()),
                                Seg::Text("+".into()),
                                Seg::Key("Left Click".into()),
                                Seg::Text("anywhere in a panel to move it.".into()),
                            ]))
                            .child(line(
                                "Drop it on a panel's edge to split there, on the middle \
                                 to share a tab group, or outside the window to make it \
                                 its own window.",
                            )),
                    ),
                    card(
                        icons::KEYBOARD,
                        "Menubar",
                        kbd_line([
                            Seg::Text("With the menubar hidden, hold".into()),
                            Seg::Key("Alt".into()),
                            Seg::Text("to float it back over the dock, or tap".into()),
                            Seg::Key("Alt".into()),
                            Seg::Text("twice to leave it up.".into()),
                        ]),
                    ),
                ]))
                .into_any_element(),
            Stage::Workspaces => self.shelf(cx),
        }
    }

    /// The quick-start shelf: every shipped workspace as a picture tile,
    /// wrapping into however many columns the window affords. Applying goes
    /// through the frontmost workspace window at app level, since this
    /// window has no workspace of its own.
    fn shelf(&self, cx: &mut Context<Self>) -> AnyElement {
        // The tiles size to the shelf but the pan math needs pixels, so a
        // probe measures the laid-out width every paint and wakes the view
        // when a resize moves it. Next frame renders at the corrected
        // width; one frame of lag during a live drag.
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

        // The measured width splits into however many tile columns fit at a
        // comfortable size, so a narrow window drops to one or two across
        // instead of squeezing three. Up to the column cap the tiles take
        // the whole width between them; the tile cap only bites on a window
        // wider than the shelf has any use for.
        let gap = f32::from(tokens::SPACE_SM);
        let columns = (tiles_width / MIN_TILE_W)
            .floor()
            .clamp(1., MAX_TILE_COLUMNS);
        let tile_width = (((tiles_width - gap * (columns - 1.)) / columns).min(MAX_TILE_W)).floor();
        // The grid is only ever as wide as the columns it holds, so a window
        // wider than that leaves the slack at the edge instead of letting
        // flex wrap a fourth tile in behind the cap's back.
        let grid_width = tile_width * columns + gap * (columns - 1.);

        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .child(
                // The probe measures the room the shelf has; the grid inside
                // takes only what the column cap allows, centered in it so a
                // window wider than the cap splits the slack instead of
                // pushing the gallery to one side.
                div()
                    .relative()
                    .w_full()
                    .justify_center()
                    .child(probe)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            // Wrapped flex lines stretch apart to fill the
                            // shelf by default; pack them at the top instead.
                            .content_start()
                            .w(px(grid_width))
                            .gap(tokens::SPACE_SM)
                            .children(self.workspaces.iter().enumerate().map(|(i, tile)| {
                                let apply = tile.name.clone();
                                workspace_tile(
                                    tile.name.clone(),
                                    tile.author.clone(),
                                    tile.previews.pick(palette::mode()),
                                    self.hovered_tile == Some(i),
                                    tile_width,
                                    cx.listener(move |_, _, window, cx| {
                                        crate::workspace::apply_workspace_to_front(&apply, cx);
                                        // Picking a look is the end of the
                                        // tour, so close out to the freshly
                                        // dressed window.
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
                            })),
                    ),
            )
            .child(div().text_xs().text_color(palette::text_faint()).child(
                "Picking one replaces the main window's look and closes the tour. \
                         This window is here any time under Application > Welcome.",
            ))
            .into_any_element()
    }

    /// The stage row in the footer: one dot per stage, the one that's up
    /// lit, any of them a click away.
    fn dots(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .children(STAGES.iter().enumerate().map(|(i, _)| {
                let here = i == self.stage;
                div()
                    .size(px(8.))
                    .flex_none()
                    .rounded_full()
                    .cursor_pointer()
                    .bg(if here {
                        palette::accent()
                    } else {
                        palette::bg_control()
                    })
                    .when(!here, |d| d.hover(|d| d.bg(palette::bg_control_hover())))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.go_to(i, cx)),
                    )
            }))
    }
}

impl Render for WelcomeWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The window renders under the player's art tint like the
        // workspace it opened over, and claims the widget theme while it
        // holds focus, so the tour reads in the playing track's colors.
        let player = self.state.player.entity_id();
        palette::note_focus(player, window.is_window_active(), cx);
        let index = self.stage;
        let stage = STAGES[index];
        let first = index == 0;
        let last = index + 1 == STAGES.len();

        panel::window_body(player, || {
            let heading = div()
                .flex()
                .flex_col()
                .flex_none()
                .gap(tokens::SPACE_XS)
                // The logo leads the tour and then gets out of the way; the
                // stages after it are all copy and controls.
                .when(first, |d| {
                    d.child(
                        svg()
                            .path(icons::LOGO)
                            .size(px(44.))
                            .text_color(palette::text_bright())
                            .mb(tokens::SPACE_SM),
                    )
                })
                .child(div().text_lg().child(stage.title()))
                .child(line(stage.lead()));

            let body = div()
                .id("welcome-stage")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                // The content stops short of the scrollbar's lane rather
                // than running under the thumb.
                .pr(px(SCROLL_LANE))
                .child(self.stage_body(stage, cx));

            let page = div()
                .flex()
                .flex_col()
                .size_full()
                .gap(SECTION_GAP)
                .child(heading)
                .child(scroll_lane(body, &self.scroll).flex_1().min_h_0());

            let buttons = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_SM)
                .when(!last, |d| {
                    d.child(small_button(
                        "Close",
                        icons::CLOSE,
                        false,
                        cx.listener(|_, _, window, _| window.remove_window()),
                    ))
                })
                .child(small_button(
                    "Back",
                    icons::CHEVRON_LEFT,
                    first,
                    cx.listener(|this, _, _, cx| this.step(-1, cx)),
                ))
                .child(if last {
                    small_button(
                        "Done",
                        icons::CHECK,
                        false,
                        cx.listener(|_, _, window, _| window.remove_window()),
                    )
                } else {
                    small_button(
                        "Next",
                        icons::CHEVRON_RIGHT,
                        false,
                        cx.listener(|this, _, _, cx| this.step(1, cx)),
                    )
                });

            let footer = div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(tokens::SPACE_SM)
                .px(tokens::SPACE_MD)
                .py(tokens::SPACE_SM)
                .border_t_1()
                .border_color(palette::border())
                .bg(palette::bg_panel())
                .child(self.dots(cx))
                .child(buttons);

            div()
                .size_full()
                .flex()
                .flex_col()
                .track_focus(&self.focus)
                // The arrows step the tour; anything else the window sees is
                // somebody else's. Modified keystrokes pass through so the
                // app's own chords keep working over the top.
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                    if event.keystroke.modifiers.modified() {
                        return;
                    }
                    match event.keystroke.key.as_str() {
                        "left" => this.step(-1, cx),
                        "right" => this.step(1, cx),
                        _ => {}
                    }
                }))
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
                .child(footer)
                .into_any_element()
        })
    }
}
