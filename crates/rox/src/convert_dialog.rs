//! The convert dialog: a selection, a format, a folder to write into, and
//! the names the files come out under.
//!
//! It's the rename dialog's twin turned outward. Renaming moves the
//! library's own files and shows every move before it happens; this writes
//! new files somewhere else and shows every name before it happens. Both
//! run on [`crate::tags::guess::Pattern`], so a naming scheme learned in
//! one works in the other.
//!
//! Nothing here touches the library. Outputs land wherever the destination
//! says, and if that happens to sit under a library root the watcher picks
//! them up like any other files that appeared - there's no import step and
//! no second copy of a row.
//!
//! The run itself belongs to [`crate::convert`], which is app-global: the
//! dialog closes on the press and the tasks window carries the progress and
//! the Stop, the same as starting an analysis pass.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    actions, div, prelude::*, px, size, App, Bounds, Context, Div, Entity, Focusable as _, Global,
    KeyBinding, PathPromptOptions, ScrollHandle, SharedString, Subscription, Task, Window,
    WindowHandle,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_component::scroll::Scrollbar;
use gpui_component::{Root, Sizable};

use rox_core::settings::{LayoutSize, Settings};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_library::writer::Field;
use rox_panel_api::panel::AppState;
use rox_panel_kit::ui::{self as settings_ui, kbd_line, section, Seg};
use rox_services::backdrop::{NowPlayingArt, WindowBackdrop};

use crate::convert::{self, Custom, Entry, Format, Preset, Row, Span};
use crate::matching::{open_or_focus, WindowRegistry};
use crate::tags::guess;

/// The open convert dialogs, keyed by their selection.
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

/// The key context the window's own bindings scope to.
const CONTEXT: &str = "ConvertDialog";

/// The dialog's convert binding; call once at startup. It sits on the
/// window root, so enter converts wherever focus is rather than only in the
/// pattern field. The inputs still see the key first, since their own
/// binding is deeper along the focus path: a single-line input propagates
/// it up to here, which is how enter in a custom field can ask ffmpeg now
/// and convert on the press after.
pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("enter", Convert, Some(CONTEXT))]);
}

/// Open the convert dialog on `ids`, or bring the one already on that
/// selection to the front. An empty selection opens nothing, and neither
/// does a machine with no ffmpeg - every menu that gets here is gated on
/// the same probe, so this is the backstop rather than the gate.
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

/// One selected track, as the dialog reads it off the catalog's projection:
/// no file is opened here, so the preview keeps up with typing.
struct Track {
    row: Row,
    /// What the row calls itself in the preview's left column.
    name: SharedString,
}

/// How long the two custom inputs sit still before the combination goes to
/// ffmpeg. A check is a process, so it waits out typing the way the online
/// searches do.
const CHECK_SETTLE: Duration = Duration::from_millis(600);

/// Where a custom format's check stands. Convert is inert for anything but
/// [`Check::Passed`]: the alternative is learning that libvorbis isn't in
/// this build one failed file at a time.
#[derive(Clone, PartialEq)]
enum Check {
    /// The pair changed and the wait hasn't run out.
    Waiting,
    /// ffmpeg has it.
    Checking,
    Passed,
    /// Why it can't run, in whoever's words said it: the tokenizer's for
    /// something this module owns, ffmpeg's own for anything else.
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
    /// Whether the format is the typed one rather than one of the five.
    custom: bool,
    custom_ext: Entity<InputState>,
    custom_args: Entity<InputState>,
    /// Where the current custom pair stands with ffmpeg. Meaningless while
    /// `custom` is false, and never read there.
    check: Check,
    /// The running check. Held so that storing a new one drops the timer
    /// and the spawn under it, which is how a burst of typing costs one
    /// process rather than one per keystroke.
    check_task: Option<Task<()>>,
    /// Where the files go. None until one is picked, which is also the one
    /// thing that keeps the Convert button inert.
    dest: Option<PathBuf>,
    pattern: Entity<InputState>,
    /// The current plan, rebuilt when the pattern, the preset or the
    /// destination changes rather than per frame: it stats the disk for
    /// every output, which is not something a repaint should pay for.
    plan: Vec<Entry>,
    /// What is wrong with the pattern itself, when nothing parses.
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
                        // A zero is the catalog's way of saying the file
                        // carries no number, so it renders as missing
                        // rather than as "00" or the year 0.
                        for (field, number) in [
                            (Field::Year, v.year),
                            (Field::TrackNo, v.track_no),
                            (Field::DiscNo, v.disc_no),
                        ] {
                            if number > 0 {
                                values.push((field, number.to_string()));
                            }
                        }
                        // The span is what makes this a trim rather than a
                        // whole file. A row that says it's a subsong but
                        // has no span in the projection is a rip mid-scan,
                        // and converting the whole image under its name
                        // would be a surprise, so it renders as a plain
                        // file only when it really is one.
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
        // A remembered destination that has since been unplugged or deleted
        // is worse than none: the plan would read every output as free and
        // the run would fail on the first file.
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
                    // Enter is the impatient version of the settle: check
                    // now, and the root's binding converts on the press
                    // after, once the answer is in.
                    InputEvent::PressEnter { .. } => this.check_soon(false, cx),
                    InputEvent::Change => {
                        // The extension is half of every destination in the
                        // preview, so the plan moves with it.
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
            // Straight to it rather than after a settle: nothing has been
            // typed, and if this pair passed earlier in the session the
            // cache answers without a spawn.
            this.check_soon(false, cx);
        }
        this
    }

    /// Rebuild the plan from the pattern, preset and destination as they
    /// stand. Runs on every keystroke in the pattern, so it does the disk
    /// probing the render must not.
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

    /// The extension the outputs take, which the preview needs whether or
    /// not the rest of a custom format holds together. Typed with or
    /// without the dot and in whatever case; the plan gets it the one way.
    fn ext(&self, cx: &App) -> String {
        match self.format(cx) {
            Some(format) => format.ext().to_owned(),
            // A custom that doesn't hold together yet still names a
            // container, and the preview is more use showing it than
            // showing nothing.
            None => self.typed_ext(cx),
        }
    }

    /// What the extension input holds, tidied the one way: typed with or
    /// without the dot and in whatever case, remembered and rendered the
    /// same either way.
    fn typed_ext(&self, cx: &App) -> String {
        self.custom_ext
            .read(cx)
            .value()
            .trim()
            .trim_start_matches('.')
            .trim()
            .to_ascii_lowercase()
    }

    /// The custom pair as the run would take it, or the sentence saying why
    /// it isn't one yet.
    fn pair(&self, cx: &App) -> Result<Custom, String> {
        Custom::parse(
            &self.custom_ext.read(cx).value(),
            &self.custom_args.read(cx).value(),
        )
    }

    /// What a run would encode to, once everything about it holds. None
    /// while a custom doesn't parse, which is also when Convert is inert.
    fn format(&self, cx: &App) -> Option<Format> {
        if self.custom {
            self.pair(cx).ok().map(Format::Custom)
        } else {
            Some(Format::Preset(self.preset))
        }
    }

    /// Whether the format is one this machine has agreed to. A preset is,
    /// always; a custom is once ffmpeg has encoded something with it.
    fn format_ready(&self) -> bool {
        !self.custom || self.check == Check::Passed
    }

    /// Put the custom pair to ffmpeg. With `debounce`, wait out a beat of
    /// quiet first, so typing an argument list costs one process rather
    /// than one per keystroke; storing the task drops whatever the last
    /// call left running.
    ///
    /// Nothing spawns for a pair the tokenizer already refuses, or for one
    /// this session has an answer for.
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
                // A pair that changed while ffmpeg was busy has its own
                // check running, and this answer is about the old one.
                if this.pair(cx).as_ref() == Ok(&custom) {
                    this.check = Check::from(answer);
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// How many of the selection the current plan actually converts.
    fn converting(&self) -> usize {
        self.plan.iter().filter(|entry| entry.converts()).count()
    }

    /// How many of the selection are spans inside an image rather than
    /// files of their own, the count behind the dialog's one note.
    fn spans(&self) -> usize {
        self.tracks
            .iter()
            .filter(|track| track.row.span.is_some())
            .count()
    }

    /// Whether the pattern in the input builds folders, which is what the
    /// mirror toggle means. Read off the pattern rather than kept beside
    /// it, so a hand-edited pattern can't leave the tick lying.
    fn mirroring(&self, cx: &App) -> bool {
        self.pattern.read(cx).value().contains('/')
    }

    /// Flip between the flat default and the library's folder shape. Both
    /// are just patterns, so this writes one into the input and the
    /// preview follows.
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
        // Nothing to check any more, and a check still in flight would
        // answer about a format nobody picked.
        self.check_task = None;
        // The extension comes off the preset, so every destination in the
        // plan just changed and with it every skip decision.
        self.replan(cx);
    }

    /// Switch to the typed format. The two inputs appear, and whatever is
    /// already in them goes to ffmpeg right away rather than waiting for a
    /// keystroke that may never come.
    fn set_custom(&mut self, cx: &mut Context<Self>) {
        self.custom = true;
        self.replan(cx);
        self.check_soon(false, cx);
    }

    /// Ask for the folder to write into. The platform's picker, the same
    /// one that adds a library root.
    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = rx.await {
                if let Some(dir) = paths.pop() {
                    this.update(cx, |this, cx| {
                        this.dest = Some(dir);
                        this.replan(cx);
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    /// Hand the plan to the job and get out of the way. The run is
    /// app-global, so the window closes on the press and the tasks window
    /// takes over the counting.
    fn convert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dest) = self.dest.clone() else {
            return;
        };
        // A custom that hasn't passed doesn't run, whichever way the press
        // arrived. The button is already inert; this is the same lock on
        // the Enter key.
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

    /// Keep what was just run as what the next dialog opens on. A custom
    /// rides as its key plus the two fields it reads back out of, so the
    /// next open is the same format rather than a fallback to FLAC.
    fn remember(&self, format: &Format, dest: PathBuf, cx: &App) {
        let preset = format.key().to_owned();
        // The two custom fields ride along whichever format ran, so
        // switching to a preset and back doesn't cost what was typed.
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

    /// Write the window frame into the settings file, the restore for the
    /// next dialog.
    fn persist_frame(&self, window: &Window, _cx: &App) {
        let frame = window.window_bounds().get_bounds();
        Settings::update(move |s| {
            s.windows.convert_dialog = Some(LayoutSize {
                width: frame.size.width.into(),
                height: frame.size.height.into(),
            });
        });
    }

    /// One preview row: the track on the left, the file it produces on the
    /// right, relative to the destination so the pattern's own shape is
    /// what shows. A row that produces nothing says why instead.
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

    /// What the custom format is doing, under its two inputs: where the
    /// check stands, and the two things about this path someone has to know
    /// before they type into it.
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

    /// A labelled row of the form the dialog's three controls share.
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

    /// The format pick, the folder to write into, the naming pattern, and
    /// the mirror toggle under it.
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
                Input::new(&self.pattern).small(),
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
            .child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!(
                        "convert-dialog-pattern-help",
                        placeholders = guess::PLACEHOLDERS
                            .iter()
                            .filter(|p| **p != "%skip%")
                            .copied()
                            .collect::<Vec<_>>()
                            .join(" ")
                    )),
            )
    }

    /// Why Convert won't run yet, when it won't: the pattern that doesn't
    /// parse, the folder nobody has picked, a typed format ffmpeg hasn't
    /// agreed to, or a plan with nothing left in it. None once the press
    /// would do something, which is when the footer offers the shortcut
    /// instead.
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

    /// The one thing about a cue selection worth saying before the run:
    /// these are the rows that come out of a rip as real files for the
    /// first time, and the tags on them are the library's rather than the
    /// image's.
    fn span_note(&self) -> Option<SharedString> {
        let spans = self.spans();
        (spans > 0).then(|| rox_i18n::t!("convert-dialog-span-note", count = spans as u64))
    }

    /// The window's own actions, and the shortcut for them or the reason
    /// there isn't one.
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
                        "Cancel",
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
                    // The page's own surface over the root's, the same second
                    // pass the settings page takes: the backdrop reads through
                    // only as the surfaces thin.
                    .bg(palette::bg_elevated())
                    .gap(tokens::SPACE_MD)
                    .p(tokens::SPACE_MD)
                    .child(section("Output", None, self.controls(cx)))
                    .child(section("Preview", Some(count), preview).flex_1().min_h_0()),
            )
            .child(self.footer(ready, cx))
    }
}
