//! The welcome window, opened on the first launch and from the Application
//! menu. Two stages: cards pointing at what's worth knowing, then the quick
//! start, the shipped workspaces as picture tiles that apply a whole look in
//! one click.

use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, App, Bounds, Context, Div, FocusHandle, Global,
    KeyDownEvent, MouseButton, ObjectFit, Pixels, ScrollHandle, SharedString, Subscription, Window,
    WindowHandle, canvas, div, img, point, prelude::*, px, size, svg,
};
use gpui_component::Root;
use gpui_component::scroll::{Scrollbar, ScrollbarShow};

use crate::startup::desktop_integration::{self, Status as MenuStatus};
use rox_core::settings::{Settings, app_font, set_language};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel::{self, AppState};
use rox_panel_kit::ui::{SECTION_GAP, Seg, chord, kbd_line, small_button};
use rox_services::backdrop::WindowBackdrop;

struct OpenWelcome(WindowHandle<Root>);

impl Global for OpenWelcome {}

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
    // Wide enough for three cards across without squeezing their copy.
    let bounds = Bounds::centered(None, size(px(1240.), px(660.)), cx);
    let handle = rox_panel_api::panel::open_child_window(
        cx,
        rox_i18n::t!("welcome-window-title"),
        bounds,
        Some(size(px(700.), px(460.))),
        move |window, cx| cx.new(|cx| WelcomeWindow::new(state, window, cx)),
    );
    cx.set_global(OpenWelcome(handle));
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Welcome,
    Workspaces,
}

const STAGES: [Stage; 2] = [Stage::Welcome, Stage::Workspaces];

impl Stage {
    fn title(self) -> SharedString {
        match self {
            Stage::Welcome => rox_i18n::t!("welcome-stage-title-welcome"),
            Stage::Workspaces => rox_i18n::t!("welcome-stage-title-quick-start"),
        }
    }

    fn lead(self) -> SharedString {
        match self {
            Stage::Welcome => rox_i18n::t!("welcome-stage-lead-welcome"),
            Stage::Workspaces => rox_i18n::t!("welcome-stage-lead-quick-start"),
        }
    }
}

struct WelcomeWindow {
    state: AppState,
    backdrop: WindowBackdrop,
    /// Read once on open, so render never reparses the embedded bundles.
    workspaces: Vec<Tile>,
    hovered_tile: Option<usize>,
    /// Measured by a probe canvas each paint, for the hover pan's pixel math.
    /// The seed is corrected by the first paint.
    tiles_width: f32,
    stage: usize,
    language: Option<String>,
    /// Read on open and again once the offer is answered.
    menu: MenuStatus,
    menu_error: Option<String>,
    /// One handle for every stage, reset on each step.
    scroll: ScrollHandle,
    /// Nothing here takes typing; this puts the arrow keys on the dispatch path.
    focus: FocusHandle,
    /// This window pumps its own frames, so the backdrop needs its own wake.
    _backdrop_changed: Subscription,
}

impl WelcomeWindow {
    fn new(state: AppState, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        let focus = cx.focus_handle();
        window.focus(&focus);
        // An unparseable header falls back to the frame's aspect, which renders
        // the picture static.
        fn sized(path: SharedString) -> (SharedString, f32) {
            let aspect = rox_design::assets::png_aspect(&path).unwrap_or(FRAME_ASPECT);
            (path, aspect)
        }
        let workspaces = crate::workspaces::shipped()
            .into_iter()
            .map(|entry| Tile {
                name: SharedString::from(entry.name.clone()),
                title: entry.title.clone(),
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
            language: Settings::load().language.clone(),
            menu: desktop_integration::status(),
            menu_error: None,
            scroll: ScrollHandle::new(),
            focus,
            _backdrop_changed,
        }
    }

    fn set_language(&mut self, language: Option<String>, cx: &mut Context<Self>) {
        set_language(language.as_deref(), cx);
        self.language = language.clone();
        Settings::update(move |s| s.language = language);
        cx.notify();
    }

    fn add_menu_entry(&mut self, cx: &mut Context<Self>) {
        self.menu_error = desktop_integration::install().err();
        self.menu = desktop_integration::status();
        cx.notify();
    }

    fn decline_menu_entry(&mut self, cx: &mut Context<Self>) {
        Settings::update(|s| s.session.appimage_menu_declined = true);
        self.menu = MenuStatus::Declined;
        cx.notify();
    }

    fn menu_offer(&self, cx: &mut Context<Self>) -> Option<Div> {
        if self.menu != MenuStatus::NotOffered {
            return None;
        }

        let banner = match &self.menu_error {
            Some(reason) => panel::banner_flow(
                panel::Tone::Bad,
                rox_i18n::t!("welcome-menu-failed"),
                vec![reason.clone().into()],
            ),
            None => panel::banner_flow(
                panel::Tone::Info,
                rox_i18n::t!("welcome-menu-title"),
                vec![rox_i18n::t!("welcome-menu-note")],
            ),
        };

        Some(
            div()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_SM)
                .child(banner)
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(tokens::SPACE_SM)
                        .child(small_button(
                            rox_i18n::t!("welcome-menu-add"),
                            icons::PLUS,
                            false,
                            cx.listener(|this, _, _, cx| this.add_menu_entry(cx)),
                        ))
                        .child(small_button(
                            rox_i18n::t!("welcome-menu-not-now"),
                            icons::CLOSE,
                            false,
                            cx.listener(|this, _, _, cx| this.decline_menu_entry(cx)),
                        )),
                ),
        )
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let last = STAGES.len() as isize - 1;
        let next = (self.stage as isize + delta).clamp(0, last) as usize;
        self.go_to(next, cx);
    }

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

fn line(text: impl Into<SharedString>) -> Div {
    div().text_color(palette::text_muted()).child(text.into())
}

/// The card's copy is measured at this basis, which keeps a card as tall as
/// its own text.
const CARD_BASIS: f32 = 300.0;

const MIN_TILE_W: f32 = 400.0;

/// Past three the shelf reads as a contact sheet; more room goes to bigger
/// pictures instead.
const MAX_TILE_COLUMNS: f32 = 3.0;

/// The shipped previews are about 1400px across, so this is roughly where a
/// fullscreen shelf starts upscaling them on a 2x display.
const MAX_TILE_W: f32 = 900.0;

fn card(icon: &'static str, title: impl Into<SharedString>, body: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        // Grow from a real basis rather than zero, so a card comes out as tall as
        // its own text and a long line can't set its width.
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
                .child(title.into()),
        )
        .child(body)
}

/// A row rather than a wrapping grid: a wrapped flex line takes its height
/// from the container, which stretches the first row to half the page.
fn cards(cards: impl IntoIterator<Item = Div>) -> Div {
    div()
        .flex()
        .flex_row()
        .gap(tokens::SPACE_MD)
        .children(cards)
}

const SCROLL_LANE: f32 = 16.0;

/// The caller keeps [`SCROLL_LANE`] clear inside the column.
fn scroll_lane(column: impl IntoElement, scroll: &ScrollHandle) -> Div {
    div().relative().child(column).child(
        div()
            .absolute()
            .inset_0()
            .child(Scrollbar::vertical(scroll).scrollbar_show(ScrollbarShow::Always)),
    )
}

struct Tile {
    name: SharedString,
    title: SharedString,
    author: Option<SharedString>,
    previews: TilePreviews,
}

struct TilePreviews {
    dark: Option<(SharedString, f32)>,
    light: Option<(SharedString, f32)>,
}

impl TilePreviews {
    fn pick(&self, mode: palette::Mode) -> Option<(SharedString, f32)> {
        match mode {
            palette::Mode::Dark => self.dark.clone(),
            palette::Mode::Light => self.light.clone(),
        }
    }
}

/// Every preview crops to 16:9 of the column's width, so the shelf reads as
/// a uniform reel.
const FRAME_ASPECT: f32 = 16. / 9.;

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
            // The picture renders at its real height and the hover pans down and back.
            // The raised-cosine easing starts and ends at the top, so the loop never
            // jumps.
            Some((path, aspect)) if width / aspect > frame_height => {
                let height = (width / aspect).round();
                let pan = height - frame_height;
                // Fill at the picture's exact aspect, not Cover: Cover's ratio check sits
                // on a knife edge here and jitters horizontally while panning. The offset
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
                    // Sweep time scales with distance, so every shot drifts at the same pace.
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
                            .child(rox_i18n::t!("welcome-tile-by", author = author.to_string())),
                    )
                }),
        )
}

impl WelcomeWindow {
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
                        .child(line(rox_i18n::t!("welcome-tour-intro")))
                        .child(div().text_color(palette::text_faint()).child(kbd_line([
                            Seg::Text(rox_i18n::t!("welcome-step-hint-before")),
                            Seg::Key("Left".into()),
                            Seg::Text(rox_i18n::t!("welcome-and")),
                            Seg::Key("Right".into()),
                            Seg::Text(rox_i18n::t!("welcome-step-hint-after")),
                        ]))),
                )
                .when_some(self.menu_offer(cx), |d, offer| d.child(offer))
                .child(cards([
                    card(
                        icons::MUSIC,
                        rox_i18n::t!("welcome-card-music-title"),
                        div()
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap(tokens::SPACE_SM)
                            .child(small_button(
                                rox_i18n::t!("welcome-add-folder"),
                                icons::FOLDER_PLUS,
                                false,
                                cx.listener(|this, _, _, cx| {
                                    rox_services::catalog::browse(&this.state.library, cx);
                                }),
                            ))
                            .child(line(rox_i18n::t!("welcome-music-note"))),
                    ),
                    card(
                        icons::PLAY,
                        rox_i18n::t!("welcome-card-playback-title"),
                        div()
                            .flex()
                            .flex_col()
                            .gap(tokens::SPACE_SM)
                            .child(kbd_line([
                                Seg::Key(chord("P")),
                                Seg::Text(rox_i18n::t!("welcome-quickplay-before")),
                                Seg::Key("Enter".into()),
                                Seg::Text(rox_i18n::t!("welcome-quickplay-after")),
                            ]))
                            .child(kbd_line([
                                Seg::Key("Space".into()),
                                Seg::Text(rox_i18n::t!("welcome-playback-before")),
                                Seg::Key("Left".into()),
                                Seg::Text(rox_i18n::t!("welcome-and")),
                                Seg::Key("Right".into()),
                                Seg::Text(rox_i18n::t!("welcome-playback-after")),
                            ])),
                    ),
                    card(
                        icons::SETTINGS,
                        rox_i18n::t!("welcome-card-settings-title"),
                        div()
                            .flex()
                            .flex_col()
                            .gap(tokens::SPACE_SM)
                            .child(kbd_line([
                                Seg::Key(chord(",")),
                                Seg::Text(rox_i18n::t!("welcome-settings-hint-after")),
                            ]))
                            .child(line(rox_i18n::t!("welcome-layout-note"))),
                    ),
                ]))
                .child(cards([
                    card(
                        icons::LAYOUT_DASHBOARD,
                        rox_i18n::t!("welcome-card-panels-title"),
                        div()
                            .flex()
                            .flex_col()
                            .gap(tokens::SPACE_SM)
                            .child(line(rox_i18n::t!("welcome-panels-note")))
                            .child(line(rox_i18n::t!("welcome-design-mode-note"))),
                    ),
                    card(
                        icons::MOVE_HORIZONTAL,
                        rox_i18n::t!("welcome-card-rearranging-title"),
                        div()
                            .flex()
                            .flex_col()
                            .gap(tokens::SPACE_SM)
                            .child(kbd_line([
                                Seg::Text(rox_i18n::t!("welcome-rearrange-before")),
                                Seg::Key(rox_i18n::t!("welcome-key-middle-mouse")),
                                Seg::Text(rox_i18n::t!("welcome-or")),
                                Seg::Key("Alt".into()),
                                Seg::Text("+".into()),
                                Seg::Key(rox_i18n::t!("welcome-key-left-click")),
                                Seg::Text(rox_i18n::t!("welcome-rearrange-after")),
                            ]))
                            .child(line(rox_i18n::t!("welcome-drop-note"))),
                    ),
                    card(
                        icons::KEYBOARD,
                        rox_i18n::t!("welcome-card-menubar-title"),
                        kbd_line([
                            Seg::Text(rox_i18n::t!("welcome-menubar-before")),
                            Seg::Key("Alt".into()),
                            Seg::Text(rox_i18n::t!("welcome-menubar-mid")),
                            Seg::Key("Alt".into()),
                            Seg::Text(rox_i18n::t!("welcome-menubar-after")),
                        ]),
                    ),
                ]))
                .into_any_element(),
            Stage::Workspaces => self.shelf(cx),
        }
    }

    /// Applying goes through the frontmost workspace, since this window has
    /// none of its own.
    fn shelf(&self, cx: &mut Context<Self>) -> AnyElement {
        // The pan math needs pixels, so a probe measures the laid-out width every
        // paint and wakes the view when it moves. One frame of lag during a drag.
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

        let gap = f32::from(tokens::SPACE_SM);
        let columns = (tiles_width / MIN_TILE_W)
            .floor()
            .clamp(1., MAX_TILE_COLUMNS);
        let tile_width = (((tiles_width - gap * (columns - 1.)) / columns).min(MAX_TILE_W)).floor();
        // Size the grid to its columns, or flex would wrap a fourth tile in past
        // the cap.
        let grid_width = tile_width * columns + gap * (columns - 1.);

        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .child(
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
                            .content_start()
                            .w(px(grid_width))
                            .gap(tokens::SPACE_SM)
                            .children(self.workspaces.iter().enumerate().map(|(i, tile)| {
                                let apply = tile.name.clone();
                                workspace_tile(
                                    tile.title.clone(),
                                    tile.author.clone(),
                                    tile.previews.pick(palette::mode()),
                                    self.hovered_tile == Some(i),
                                    tile_width,
                                    cx.listener(move |_, _, window, cx| {
                                        crate::workspace::apply_workspace_to_front(&apply, cx);
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
            .child(
                div()
                    .text_xs()
                    .text_color(palette::text_faint())
                    .child(rox_i18n::t!("welcome-shelf-caption")),
            )
            .into_any_element()
    }

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
        let player = self.state.player.entity_id();
        palette::note_focus(player, window.is_window_active(), cx);
        let index = self.stage;
        let stage = STAGES[index];
        let first = index == 0;
        let last = index + 1 == STAGES.len();

        panel::window_body(player, || {
            let heading_copy = div()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_XS)
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

            // The language switch sits on the first page: nobody should have to find
            // the settings window in a language that isn't theirs.
            let heading = div()
                .flex()
                .flex_row()
                .items_start()
                .justify_between()
                .flex_none()
                .gap(tokens::SPACE_MD)
                .child(heading_copy)
                .when(first, |d| {
                    d.child(div().flex_none().child(panel::language_picker(
                        "welcome-language",
                        self.language.clone(),
                        Self::set_language,
                        cx,
                    )))
                });

            let body = div()
                .id("welcome-stage")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
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
                        rox_i18n::t!("welcome-close"),
                        icons::CLOSE,
                        false,
                        cx.listener(|_, _, window, _| window.remove_window()),
                    ))
                })
                .child(small_button(
                    rox_i18n::t!("welcome-back"),
                    icons::CHEVRON_LEFT,
                    first,
                    cx.listener(|this, _, _, cx| this.step(-1, cx)),
                ))
                .child(if last {
                    small_button(
                        rox_i18n::t!("welcome-done"),
                        icons::CHECK,
                        false,
                        cx.listener(|_, _, window, _| window.remove_window()),
                    )
                } else {
                    small_button(
                        rox_i18n::t!("welcome-next"),
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
                // Modified keystrokes pass through so the app's chords keep working.
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
                .children(self.backdrop.layer(&self.state.now_art, window, cx))
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .bg(palette::bg_elevated())
                        .p(tokens::SPACE_MD)
                        .child(page),
                )
                .child(footer)
                .into_any_element()
        })
    }
}
