//! The bookmark modal: a name field and a color row, for a mark about to
//! be dropped (Shift+M, taking the position at the press) or one being
//! edited from a strip's chevron or the bookmarks panel. Modeled on the
//! playlist name window. A blank name is fine: the mark shows its time.

use gpui::{
    actions, div, prelude::*, px, size, App, Bounds, Context, Div, Entity, FocusHandle, Focusable,
    KeyBinding, Rgba, SharedString, Stateful, Subscription, Window,
};
use gpui_component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::Sizable as _;

use rox_core::fmt::fmt_time;
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_library::cue::TrackKey;
use rox_panel_api::bookmark_ui::{self, QUICK_COLORS};
use rox_panel_api::panel::AppState;
use rox_panel_kit::ui::{kbd_line, section, small_button, Seg};
use rox_services::backdrop::WindowBackdrop;

actions!(bookmark_dialog, [Save, Cancel]);

/// The key context the window's own bindings scope to.
const CONTEXT: &str = "BookmarkName";

/// The modal's save and dismiss bindings; call once at startup. Bound on
/// the window root so Enter commits and Escape closes wherever focus is.
/// The name field and the color picker each pass an idle Escape through,
/// so the binding only sees the press once neither had a use for it.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", Save, Some(CONTEXT)),
        KeyBinding::new("escape", Cancel, Some(CONTEXT)),
    ]);
}

/// What the modal commits on Enter.
enum Action {
    /// Drop a mark this many milliseconds into the track.
    New { key: TrackKey, position_ms: u32 },
    /// Rename and recolor this mark.
    Edit(i64),
}

/// Open the new-mark modal, the position already taken.
pub fn open_new(state: AppState, key: TrackKey, secs: f64, cx: &mut App) {
    let position_ms = (secs.max(0.0) * 1000.0).round() as u32;
    open_modal(
        state,
        Action::New { key, position_ms },
        rox_i18n::t!("bookmark-new-title"),
        String::new(),
        None,
        cx,
    );
}

/// Open the edit modal over an existing mark, seeded with its name and
/// color. A mark that has since been removed opens nothing.
pub fn open_edit(state: AppState, id: i64, cx: &mut App) {
    let Some(mark) = state.library.read(cx).bookmark(id) else {
        return;
    };
    open_modal(
        state,
        Action::Edit(id),
        rox_i18n::t!("bookmark-edit-title"),
        mark.name,
        mark.color,
        cx,
    );
}

fn open_modal(
    state: AppState,
    action: Action,
    verb: SharedString,
    name: String,
    color: Option<String>,
    cx: &mut App,
) {
    let title = rox_i18n::t!("bookmark-window-title", verb = verb.to_string());
    let bounds = Bounds::centered(None, size(px(400.), px(250.)), cx);
    rox_panel_api::panel::open_child_window(cx, title, bounds, None, move |window, cx| {
        cx.new(|cx| BookmarkWindow::new(state, action, name, color, window, cx))
    });
}

struct BookmarkWindow {
    state: AppState,
    action: Action,
    input: Entity<InputState>,
    /// The chosen color as `#rrggbb`, None for the theme accent.
    color: Option<String>,
    picker: Entity<ColorPickerState>,
    backdrop: WindowBackdrop,
    _input_events: Subscription,
    _picker_events: Subscription,
    /// This window pumps its own frames, so the backdrop needs its own wake on
    /// a new bake.
    _backdrop_changed: Subscription,
}

impl BookmarkWindow {
    fn new(
        state: AppState,
        action: Action,
        name: String,
        color: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(rox_i18n::t!("bookmark-name-placeholder"))
                .default_value(name)
        });
        let _input_events = cx.subscribe_in(&input, window, |_, _, event: &InputEvent, _, cx| {
            if let InputEvent::Change = event {
                cx.notify();
            }
        });
        // The picker seeds from the current color so a custom pick starts
        // near where the mark already is rather than at a stock value.
        let seed: Rgba = bookmark_ui::color_of(color.as_deref());
        let picker = cx.new(|cx| ColorPickerState::new(window, cx).default_value(seed));
        let _picker_events = cx.subscribe_in(
            &picker,
            window,
            |this, _, event: &ColorPickerEvent, _, cx| {
                let ColorPickerEvent::Change(color) = event;
                if let Some(color) = color {
                    this.color = Some(palette::to_hex(Rgba::from(*color)));
                    cx.notify();
                }
            },
        );
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        window.focus(&input.read(cx).focus_handle(cx));
        BookmarkWindow {
            state,
            action,
            input,
            color,
            picker,
            backdrop: WindowBackdrop::default(),
            _input_events,
            _picker_events,
            _backdrop_changed,
        }
    }

    /// Commit and close.
    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.input.read(cx).value().trim().to_string();
        let color = self.color.clone();
        self.state
            .library
            .update(cx, |library, cx| match &self.action {
                Action::New { key, position_ms } => {
                    library.add_bookmark(key, *position_ms, &name, color.as_deref(), cx);
                }
                Action::Edit(id) => {
                    library.rename_bookmark(*id, &name, cx);
                    library.set_bookmark_color(*id, color.as_deref(), cx);
                }
            });
        window.remove_window();
    }

    /// The swatch row: the accent, the quick picks, and the picker for
    /// anything else. The chosen one wears a ring; a custom color off the
    /// quick set shows as the picker's own swatch being the ring's.
    fn colors(&self, cx: &mut Context<Self>) -> Div {
        let current = self.color.as_deref().map(str::to_ascii_lowercase);
        let custom = current
            .as_deref()
            .is_some_and(|c| !QUICK_COLORS.iter().any(|(_, hex)| *hex == c));
        let mut row = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(tokens::SPACE_SM)
            .child(self.swatch(0, palette::accent(), current.is_none(), None, cx));
        for (ix, (_, hex)) in QUICK_COLORS.iter().enumerate() {
            let Some(color) = palette::parse_hex(hex) else {
                continue;
            };
            row = row.child(self.swatch(
                ix as u64 + 1,
                color,
                current.as_deref() == Some(*hex),
                Some(hex),
                cx,
            ));
        }
        row.child(
            div()
                .flex_none()
                .ml(tokens::SPACE_SM)
                .rounded(tokens::RADIUS)
                .when(custom, |d| {
                    d.border_2().border_color(palette::text_bright())
                })
                .child(ColorPicker::new(&self.picker).small()),
        )
    }

    /// One round swatch; `pick` is the hex it sets, None for the accent.
    fn swatch(
        &self,
        ix: u64,
        color: Rgba,
        selected: bool,
        pick: Option<&'static str>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        div()
            .id(("bookmark-swatch", ix))
            .flex_none()
            .size(px(22.))
            .rounded_full()
            .bg(color)
            .border_2()
            .border_color(if selected {
                palette::text_bright()
            } else {
                palette::alpha(palette::border(), 0x80)
            })
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.color = pick.map(str::to_string);
                cx.notify();
            }))
    }

    /// The window's own actions: the save, and the shortcut for it.
    fn footer(&self, cx: &mut Context<Self>) -> Div {
        let hint = kbd_line([
            Seg::Text(rox_i18n::t!("bookmark-hint-before")),
            Seg::Key(rox_i18n::t!("bookmark-hint-key")),
            Seg::Text(rox_i18n::t!("bookmark-hint-after")),
        ])
        .text_xs();
        div()
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
            .child(hint)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .child(small_button(
                        rox_i18n::t!("bookmark-save"),
                        icons::CHECK,
                        false,
                        cx.listener(|this, _, window, cx| this.commit(window, cx)),
                    ))
                    .child(small_button(
                        rox_i18n::t!("settings-common-cancel"),
                        icons::CLOSE,
                        false,
                        cx.listener(|_, _, window, _| window.remove_window()),
                    )),
            )
    }

    /// Where the mark sits, for the new-mark modal's heading: the time
    /// the press took, so what gets saved is never a surprise.
    fn position_line(&self) -> Option<Div> {
        let Action::New { position_ms, .. } = &self.action else {
            return None;
        };
        Some(
            div()
                .text_sm()
                .text_color(palette::text_muted())
                .child(rox_i18n::t!(
                    "bookmark-position",
                    time = fmt_time(*position_ms as f64 / 1000.0)
                )),
        )
    }
}

impl Focusable for BookmarkWindow {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }
}

impl Render for BookmarkWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .key_context(CONTEXT)
            .on_action(cx.listener(|this, _: &Save, window, cx| this.commit(window, cx)))
            .on_action(cx.listener(|_, _: &Cancel, window, _| window.remove_window()))
            .bg(palette::bg_elevated())
            .text_color(palette::text_bright())
            .text_sm()
            .children(self.backdrop.layer(&self.state.now_art, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .p(tokens::SPACE_MD)
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_MD)
                    .bg(palette::bg_elevated())
                    .child(section(
                        rox_i18n::t!("bookmark-name"),
                        None,
                        div()
                            .flex()
                            .flex_col()
                            .gap(tokens::SPACE_XS)
                            .child(Input::new(&self.input).w_full())
                            .children(self.position_line()),
                    ))
                    .child(section(
                        rox_i18n::t!("bookmark-color"),
                        None,
                        self.colors(cx),
                    )),
            )
            .child(self.footer(cx))
    }
}

/// Drop a mark at the playing position: silently with the M key, or
/// through the modal for a named one. Nothing playing means nothing to
/// mark, and a track from outside the library has no row to hang one on,
/// which the library reports by adding nothing.
pub fn drop_here(state: AppState, named: bool, cx: &mut App) {
    let Some(now) = state.player.read(cx).now_playing() else {
        return;
    };
    if named {
        open_new(state, now.key, now.position_secs, cx);
        return;
    }
    let position_ms = (now.position_secs.max(0.0) * 1000.0).round() as u32;
    state.library.update(cx, |library, cx| {
        library.add_bookmark(&now.key, position_ms, "", None, cx);
    });
}
