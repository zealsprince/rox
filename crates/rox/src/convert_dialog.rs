//! The convert dialog: a selection, a format, a destination folder, and a
//! naming pattern, with every output name previewed before the run. The
//! rename dialog's twin, on the same [`crate::tags::guess::Pattern`].
//!
//! Nothing here touches the library; outputs under a library root arrive
//! through the watcher. The run belongs to [`crate::convert`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    App, Bounds, Context, Div, Entity, Focusable as _, Global, KeyBinding, PathPromptOptions,
    ScrollHandle, SharedString, Subscription, Task, Window, WindowHandle, actions, div, prelude::*,
    px, size,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_component::scroll::Scrollbar;
use gpui_component::{Root, Sizable};

use rox_core::settings::{LayoutSize, Settings};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_library::writer::Field;
use rox_panel_api::panel::{self, AppState};
use rox_panel_kit::ui::{self as settings_ui, Seg, kbd_line, section};
use rox_services::backdrop::{NowPlayingArt, WindowBackdrop};

use crate::convert::{self, Custom, Entry, Format, Preset, Row, Span};
use crate::matching::{WindowRegistry, open_or_focus};
use crate::tags::guess;

#[derive(Default)]
struct OpenConverters(Vec<(Vec<i64>, WindowHandle<Root>)>);

impl Global for OpenConverters {}

impl WindowRegistry for OpenConverters {
    type Key = Vec<i64>;
    fn entries(&mut self) -> &mut Vec<(Vec<i64>, WindowHandle<Root>)> {
        &mut self.0
    }
}

actions!(convert_dialog, [Convert]);

const CONTEXT: &str = "ConvertDialog";

/// On the window root. Single-line inputs see Enter first and propagate it,
/// so Enter in a custom field runs the ffmpeg check and the next converts.
pub fn bindings() -> Vec<KeyBinding> {
    vec![KeyBinding::new("enter", Convert, Some(CONTEXT))]
}

/// No ffmpeg opens nothing: the backstop behind the menus' own gate.
pub fn open(state: AppState, ids: Vec<i64>, cx: &mut App) {
    if ids.is_empty() || !convert::available() {
        return;
    }
    let mut key = ids.clone();
    key.sort_unstable();
    open_or_focus::<OpenConverters>(
        key,
        move |cx| {
            let (width, height) = Settings::load()
                .windows
                .convert_dialog
                .filter(|s| s.width >= 400. && s.height >= 300.)
                .map(|s| (s.width, s.height))
                .unwrap_or((900., 600.));
            let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
            rox_panel_api::panel::open_child_window(
                cx,
                rox_i18n::t!("convert-dialog-window-title"),
                bounds,
                Some(settings_ui::MIN_SIZE),
                move |window, cx| cx.new(|cx| ConvertDialog::new(state, ids, window, cx)),
            )
        },
        cx,
    );
}

/// Read off the projection, never the file, so the preview keeps up with typing.
struct Track {
    row: Row,
    name: SharedString,
}

const CHECK_SETTLE: Duration = Duration::from_millis(600);

/// Convert is inert for anything but [`Check::Passed`], or a missing encoder
/// shows up one failed file at a time.
#[derive(Clone, PartialEq)]
enum Check {
    Waiting,
    Checking,
    Passed,
    /// The tokenizer's reason, or ffmpeg's own words.
    Failed(SharedString),
}

impl From<Result<(), String>> for Check {
    fn from(answer: Result<(), String>) -> Check {
        match answer {
            Ok(()) => Check::Passed,
            Err(reason) => Check::Failed(reason.into()),
        }
    }
}

pub struct ConvertDialog {
    tracks: Vec<Track>,
    preset: Preset,
    custom: bool,
    custom_ext: Entity<InputState>,
    custom_args: Entity<InputState>,
    check: Check,
    /// Replacing it drops the timer and spawn, so a burst of typing costs one process.
    check_task: Option<Task<()>>,
    dest: Option<PathBuf>,
    pattern: Entity<InputState>,
    /// Rebuilt on input changes, never per frame: it stats every output.
    plan: Vec<Entry>,
    parse_error: Option<SharedString>,
    scroll: ScrollHandle,
    now_art: Entity<NowPlayingArt>,
    backdrop: WindowBackdrop,
    _input_events: Vec<Subscription>,
    _backdrop_changed: Subscription,
}

impl ConvertDialog {
    fn new(state: AppState, ids: Vec<i64>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let projection = state.library.read(cx).projection().cloned();
        let tracks = {
            let library = state.library.read(cx);
            let row_of: HashMap<i64, u32> = projection
                .as_ref()
                .map(|projection| {
                    projection
                        .db_id
                        .iter()
                        .enumerate()
                        .filter(|(row, _)| !projection.is_dead(*row as u32))
                        .map(|(row, &id)| (id, row as u32))
                        .collect()
                })
                .unwrap_or_default();
            let mut tracks: Vec<Track> = Vec::with_capacity(ids.len());
            for &id in &ids {
                let Some(src) = library
                    .paths_for(&[id])
                    .ok()
                    .and_then(|mut paths| paths.pop())
                else {
                    continue;
                };
                let (values, span, title) = projection
                    .as_ref()
                    .and_then(|projection| {
                        let row = *row_of.get(&id)?;
                        let v = projection.resolve(row);
                        let mut values = vec![
                            (Field::Title, v.title.to_owned()),
                            (Field::Artist, v.artist.to_owned()),
                            (Field::AlbumArtist, v.album_artist.to_owned()),
                            (Field::Album, v.album.to_owned()),
                            (Field::Genre, v.genre.to_owned()),
                        ];
                        // Zero means no number: render it missing, not "00" or year 0.
                        for (field, number) in [
                            (Field::Year, v.year),
                            (Field::TrackNo, v.track_no),
                            (Field::DiscNo, v.disc_no),
                        ] {
                            if number > 0 {
                                values.push((field, number.to_string()));
                            }
                        }
                        // A span makes this a trim of the image rather than a whole file.
                        let span = projection.span(row).map(|span| Span {
                            start_ms: span.start_ms,
                            end_ms: span.end_ms,
                        });
                        Some((values, span, v.title.to_owned()))
                    })
                    .unwrap_or_default();
                let name = if span.is_some() && !title.is_empty() {
                    title
                } else {
                    src.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| src.display().to_string())
                };
                tracks.push(Track {
                    row: Row { src, span, values },
                    name: name.into(),
                });
            }
            tracks
        };
        let saved = Settings::load().convert;
        let custom = saved.preset == Format::CUSTOM_KEY;
        let preset = Preset::from_key(&saved.preset).unwrap_or_default();
        let seed = if saved.pattern.trim().is_empty() {
            if saved.mirror {
                convert::MIRROR_PATTERN.to_owned()
            } else {
                convert::DEFAULT_PATTERN.to_owned()
            }
        } else {
            saved.pattern.clone()
        };
        // A vanished destination is worse than none: every output would read as
        // free and the run would fail on the first file.
        let dest = saved.destination.filter(|dir| dir.is_dir());
        let pattern = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(seed)
                .placeholder(convert::DEFAULT_PATTERN)
        });
        let custom_ext = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(saved.custom_ext.clone())
                .placeholder("ogg")
        });
        let custom_args = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(saved.custom_args.clone())
                .placeholder("-c:a libvorbis -q:a 6")
        });
        let mut _input_events = Vec::new();
        _input_events.push(cx.subscribe_in(
            &pattern,
            window,
            |this: &mut Self, _, event: &InputEvent, _window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.replan(cx);
                }
            },
        ));
        for input in [&custom_ext, &custom_args] {
            _input_events.push(cx.subscribe_in(
                input,
                window,
                |this: &mut Self, _, event: &InputEvent, _window, cx| match event {
                    // Enter checks now; the root binding converts on the next press.
                    InputEvent::PressEnter { .. } => this.check_soon(false, cx),
                    InputEvent::Change => {
                        this.replan(cx);
                        this.check_soon(true, cx);
                    }
                    _ => {}
                },
            ));
        }
        window.focus(&pattern.read(cx).focus_handle(cx));
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            if let Some(this) = this.upgrade() {
                this.update(cx, |this, cx| this.persist_frame(window, cx));
            }
            true
        });
        let mut this = ConvertDialog {
            tracks,
            preset,
            custom,
            custom_ext,
            custom_args,
            check: Check::Waiting,
            check_task: None,
            dest,
            pattern,
            plan: Vec::new(),
            parse_error: None,
            scroll: ScrollHandle::new(),
            now_art: state.now_art,
            backdrop: WindowBackdrop::default(),
            _input_events,
            _backdrop_changed,
        };
        this.replan(cx);
        if this.custom {
            this.check_soon(false, cx);
        }
        this
    }

    fn replan(&mut self, cx: &mut Context<Self>) {
        let Some(dest) = self.dest.clone() else {
            self.plan.clear();
            self.parse_error = None;
            cx.notify();
            return;
        };
        match guess::parse(self.pattern.read(cx).value().trim()) {
            Ok(pattern) => {
                let rows: Vec<Row> = self
                    .tracks
                    .iter()
                    .map(|track| Row {
                        src: track.row.src.clone(),
                        span: track.row.span,
                        values: track.row.values.clone(),
                    })
                    .collect();
                let ext = self.ext(cx);
                self.plan = convert::plan(&rows, &dest, &pattern, &ext, &|path| path.exists());
                self.parse_error = None;
            }
            Err(e) => {
                self.plan.clear();
                self.parse_error = Some(e.into());
            }
        }
        cx.notify();
    }

    fn ext(&self, cx: &App) -> String {
        match self.format(cx) {
            Some(format) => format.ext().to_owned(),
            // A custom that doesn't parse yet still names a container for the preview.
            None => self.typed_ext(cx),
        }
    }

    fn typed_ext(&self, cx: &App) -> String {
        self.custom_ext
            .read(cx)
            .value()
            .trim()
            .trim_start_matches('.')
            .trim()
            .to_ascii_lowercase()
    }

    fn pair(&self, cx: &App) -> Result<Custom, String> {
        Custom::parse(
            &self.custom_ext.read(cx).value(),
            &self.custom_args.read(cx).value(),
        )
    }

    fn format(&self, cx: &App) -> Option<Format> {
        if self.custom {
            self.pair(cx).ok().map(Format::Custom)
        } else {
            Some(Format::Preset(self.preset))
        }
    }

    fn format_ready(&self) -> bool {
        !self.custom || self.check == Check::Passed
    }

    /// Nothing spawns for a pair the tokenizer rejects or one already checked
    /// this session.
    fn check_soon(&mut self, debounce: bool, cx: &mut Context<Self>) {
        let custom = match self.pair(cx) {
            Ok(custom) => custom,
            Err(reason) => {
                self.check_task = None;
                self.check = Check::Failed(reason.into());
                cx.notify();
                return;
            }
        };
        if let Some(known) = convert::checked(&custom) {
            self.check_task = None;
            self.check = Check::from(known);
            cx.notify();
            return;
        }
        self.check = if debounce {
            Check::Waiting
        } else {
            Check::Checking
        };
        cx.notify();
        self.check_task = Some(cx.spawn(async move |this, cx| {
            if debounce {
                cx.background_executor().timer(CHECK_SETTLE).await;
                this.update(cx, |this, cx| {
                    this.check = Check::Checking;
                    cx.notify();
                })
                .ok();
            }
            let answer = cx
                .background_executor()
                .spawn({
                    let custom = custom.clone();
                    async move { convert::check(&custom) }
                })
                .await;
            this.update(cx, |this, cx| {
                // A stale answer: the pair changed while ffmpeg ran.
                if this.pair(cx).as_ref() == Ok(&custom) {
                    this.check = Check::from(answer);
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn converting(&self) -> usize {
        self.plan.iter().filter(|entry| entry.converts()).count()
    }

    fn spans(&self) -> usize {
        self.tracks
            .iter()
            .filter(|track| track.row.span.is_some())
            .count()
    }

    /// Read off the pattern so a hand-edited one can't leave the tick lying.
    fn mirroring(&self, cx: &App) -> bool {
        self.pattern.read(cx).value().contains('/')
    }

    fn set_mirror(&mut self, mirror: bool, window: &mut Window, cx: &mut Context<Self>) {
        let pattern = if mirror {
            convert::MIRROR_PATTERN
        } else {
            convert::DEFAULT_PATTERN
        };
        self.pattern
            .update(cx, |input, cx| input.set_value(pattern, window, cx));
        self.replan(cx);
    }

    fn set_preset(&mut self, preset: Preset, cx: &mut Context<Self>) {
        self.preset = preset;
        self.custom = false;
        self.check_task = None;
        self.replan(cx);
    }

    fn set_custom(&mut self, cx: &mut Context<Self>) {
        self.custom = true;
        self.replan(cx);
        self.check_soon(false, cx);
    }

    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = rx.await
                && let Some(dir) = paths.pop()
            {
                this.update(cx, |this, cx| {
                    this.dest = Some(dir);
                    this.replan(cx);
                })
                .ok();
            }
        })
        .detach();
    }

    fn convert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dest) = self.dest.clone() else {
            return;
        };
        // The same lock the inert button has, for the Enter key.
        if !self.format_ready() {
            return;
        }
        let Some(format) = self.format(cx) else {
            return;
        };
        let items: Vec<convert::Item> = self
            .plan
            .iter()
            .filter(|entry| entry.converts())
            .map(|entry| entry.item.clone())
            .collect();
        if items.is_empty() {
            return;
        }
        let skipped = self.plan.len() - items.len();
        self.remember(&format, dest.clone(), cx);
        self.persist_frame(window, cx);
        convert::start(items, format, dest, skipped, cx);
        window.remove_window();
    }

    fn remember(&self, format: &Format, dest: PathBuf, cx: &App) {
        let preset = format.key().to_owned();
        // Saved whichever format ran, so a round trip through a preset keeps the typing.
        let ext = self.typed_ext(cx);
        let args = self.custom_args.read(cx).value().trim().to_owned();
        let pattern = self.pattern.read(cx).value().trim().to_owned();
        let mirror = self.mirroring(cx);
        Settings::update(move |s| {
            s.convert.preset = preset;
            s.convert.destination = Some(dest);
            s.convert.pattern = pattern;
            s.convert.mirror = mirror;
            s.convert.custom_ext = ext;
            s.convert.custom_args = args;
        });
    }

    fn persist_frame(&self, window: &Window, _cx: &App) {
        let frame = window.window_bounds().get_bounds();
        Settings::update(move |s| {
            s.windows.convert_dialog = Some(LayoutSize {
                width: frame.size.width.into(),
                height: frame.size.height.into(),
            });
        });
    }

    fn preview_row(&self, entry: &Entry, track: &Track) -> Div {
        let dest = self.dest.clone().unwrap_or_default();
        let (line, color) = match &entry.skip {
            Some(skip) => (SharedString::from(skip.label()), palette::text_faint()),
            None => (
                SharedString::from(
                    entry
                        .item
                        .dest
                        .strip_prefix(&dest)
                        .unwrap_or(&entry.item.dest)
                        .to_string_lossy()
                        .into_owned(),
                ),
                palette::text_bright(),
            ),
        };
        div()
            .flex()
            .flex_row()
            .items_start()
            .gap(tokens::SPACE_MD)
            .py(px(2.))
            .text_xs()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(palette::text_muted())
                    .child(track.name.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(palette::text_faint())
                    .child(if entry.converts() { "→" } else { "·" }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(color)
                    .child(line),
            )
    }

    fn check_note(&self) -> Div {
        let (line, color): (SharedString, gpui::Rgba) = match &self.check {
            Check::Waiting => (
                rox_i18n::t!("convert-dialog-check-waiting"),
                palette::text_muted(),
            ),
            Check::Checking => (
                rox_i18n::t!("convert-dialog-checking"),
                palette::text_muted(),
            ),
            Check::Passed => (
                rox_i18n::t!("convert-dialog-check-passed"),
                palette::tone_good(),
            ),
            Check::Failed(reason) => (reason.clone(), palette::tone_bad()),
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .child(div().w(px(84.)).flex_none())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .text_xs()
                    .child(div().text_color(color).child(line))
                    .child(
                        div()
                            .text_color(palette::text_faint())
                            .child(rox_i18n::t!("convert-dialog-custom-note")),
                    ),
            )
    }

    fn control_row(label: impl Into<SharedString>, control: impl IntoElement) -> Div {
        let label = label.into();
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .child(
                div()
                    .w(px(84.))
                    .flex_none()
                    .text_color(palette::text_muted())
                    .child(label),
            )
            .child(div().flex_1().min_w_0().child(control))
    }

    fn controls(&self, cx: &mut Context<Self>) -> Div {
        let current = self.preset;
        let custom = self.custom;
        let label: SharedString = if custom {
            rox_i18n::t!("convert-dialog-custom-label")
        } else {
            current.label()
        };
        let host = cx.entity().downgrade();
        let picker = settings_ui::select_field("convert-preset", label, false).dropdown_menu(
            move |mut menu, _, _| {
                for preset in Preset::ALL {
                    let host = host.clone();
                    menu = menu.item(
                        PopupMenuItem::new(preset.label())
                            .checked(!custom && preset == current)
                            .on_click(move |_, _, cx| {
                                if let Some(host) = host.upgrade() {
                                    host.update(cx, |this, cx| this.set_preset(preset, cx));
                                }
                            }),
                    );
                }
                let host = host.clone();
                menu.item(
                    PopupMenuItem::new(rox_i18n::t!("convert-dialog-custom-menu-item"))
                        .checked(custom)
                        .on_click(move |_, _, cx| {
                            if let Some(host) = host.upgrade() {
                                host.update(cx, |this, cx| this.set_custom(cx));
                            }
                        }),
                )
            },
        );
        let dest: SharedString = match &self.dest {
            Some(dir) => dir.display().to_string().into(),
            None => rox_i18n::t!("convert-dialog-choose-folder"),
        };
        let dest_color = if self.dest.is_some() {
            palette::text_bright()
        } else {
            palette::text_muted()
        };
        let mirroring = self.mirroring(cx);
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .child(Self::control_row(
                rox_i18n::t!("convert-dialog-label-format"),
                picker,
            ))
            .when(custom, |controls| {
                controls
                    .child(Self::control_row(
                        rox_i18n::t!("convert-dialog-label-extension"),
                        Input::new(&self.custom_ext).small(),
                    ))
                    .child(Self::control_row(
                        "ffmpeg",
                        Input::new(&self.custom_args).small(),
                    ))
                    .child(self.check_note())
            })
            .child(Self::control_row(
                rox_i18n::t!("convert-dialog-label-into"),
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(dest_color)
                            .child(dest),
                    )
                    .child(settings_ui::small_button(
                        rox_i18n::t!("convert-dialog-browse"),
                        icons::FOLDER,
                        false,
                        cx.listener(|this, _, window, cx| this.browse(window, cx)),
                    )),
            ))
            .child(Self::control_row(
                rox_i18n::t!("convert-dialog-label-named"),
                panel::pattern_input(
                    "convert-pattern",
                    &self.pattern,
                    guess::PLACEHOLDERS,
                    vec![rox_i18n::t!("convert-dialog-pattern-help")],
                    None,
                ),
            ))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .child(div().w(px(84.)).flex_none())
                    .child(
                        div()
                            .id("convert-mirror")
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            .cursor_pointer()
                            .child(settings_ui::checkbox(mirroring))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(palette::text_muted())
                                    .child(rox_i18n::t!("convert-dialog-mirror")),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.set_mirror(!mirroring, window, cx);
                            })),
                    ),
            )
    }

    /// None once a press would convert, when the footer shows the shortcut instead.
    fn status(&self) -> Option<(SharedString, gpui::Rgba)> {
        if let Some(e) = &self.parse_error {
            return Some((e.clone(), palette::tone_bad()));
        }
        if self.dest.is_none() {
            return Some((
                rox_i18n::t!("convert-dialog-pick-folder"),
                palette::tone_warn(),
            ));
        }
        if !self.format_ready() {
            return Some((
                rox_i18n::t!("convert-dialog-format-not-ready"),
                palette::tone_warn(),
            ));
        }
        if self.converting() == 0 {
            return Some((
                rox_i18n::t!("convert-dialog-nothing-to-convert"),
                palette::tone_warn(),
            ));
        }
        None
    }

    fn span_note(&self) -> Option<SharedString> {
        let spans = self.spans();
        (spans > 0).then(|| rox_i18n::t!("convert-dialog-span-note", count = spans as u64))
    }

    fn footer(&self, ready: bool, cx: &mut Context<Self>) -> Div {
        let hint = match self.status() {
            Some((line, color)) => div()
                .text_xs()
                .text_color(color)
                .child(line)
                .into_any_element(),
            None => kbd_line([
                Seg::Text("Press".into()),
                Seg::Key("Enter".into()),
                Seg::Text("to convert".into()),
            ])
            .text_xs()
            .into_any_element(),
        };
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
                    .child(settings_ui::small_button(
                        rox_i18n::t!("convert-dialog-convert-button"),
                        icons::AUDIO_LINES,
                        !ready,
                        cx.listener(|this, _, window, cx| this.convert(window, cx)),
                    ))
                    .child(settings_ui::small_button(
                        rox_i18n::t!("settings-common-cancel"),
                        icons::CLOSE,
                        false,
                        cx.listener(|this, _, window, cx| {
                            this.persist_frame(window, cx);
                            window.remove_window();
                        }),
                    )),
            )
    }
}

impl Render for ConvertDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ready = self.converting() > 0 && self.format_ready();
        let rows = self
            .plan
            .iter()
            .zip(&self.tracks)
            .map(|(entry, track)| self.preview_row(entry, track))
            .collect::<Vec<_>>();
        let count = div()
            .text_xs()
            .text_color(palette::text())
            .child(rox_i18n::t!(
                "convert-dialog-will-convert",
                count = self.converting() as u64,
                total = self.tracks.len() as u64
            ))
            .into_any_element();
        let preview = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_XS)
            .when_some(self.span_note(), |preview, note| {
                preview.child(
                    div()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(note),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(
                        div()
                            .id("convert-preview")
                            .size_full()
                            .flex()
                            .flex_col()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .children(rows),
                    )
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .child(Scrollbar::vertical(&self.scroll)),
                    ),
            );

        div()
            .size_full()
            .flex()
            .flex_col()
            .key_context(CONTEXT)
            .on_action(cx.listener(|this, _: &Convert, window, cx| this.convert(window, cx)))
            .bg(palette::bg_elevated())
            .text_color(palette::text_bright())
            .text_sm()
            .children(self.backdrop.layer(&self.now_art, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .bg(palette::bg_elevated())
                    .gap(tokens::SPACE_MD)
                    .p(tokens::SPACE_MD)
                    .child(section(
                        rox_i18n::t!("convert-section-output"),
                        None,
                        self.controls(cx),
                    ))
                    .child(
                        section(
                            rox_i18n::t!("convert-section-preview"),
                            Some(count),
                            preview,
                        )
                        .flex_1()
                        .min_h_0(),
                    ),
            )
            .child(self.footer(ready, cx))
    }
}
