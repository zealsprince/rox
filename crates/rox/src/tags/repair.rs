//! The tag repair window: find and rewrite the files carrying the ID3v2.4
//! double-unsync tag shape lofty (through 0.24) reads mangled. Reads
//! already tolerate the shape through `tag_source`, so the library shows
//! these files right; the bytes on disk stay broken, and any tool without
//! the same workaround trips on them. A commit through the writer repairs a
//! file for good (it clears the header unsync flag on write), so this
//! window is the way to run that repair across a selection without editing
//! a field by hand.
//!
//! Scope is the whole library (every remembered folder) or one folder the
//! user picks. A scan walks the scope, flags each file the same gate
//! `tag_source::open` clears the header flag for, and lists the hits with a
//! checkbox each. Repair commits a no-op edit to every checked file through
//! the writer's atomic layer, so the copy-verify-rename safety guards every
//! rewrite. Repaired files that live under a library root reindex so their
//! stored mtime and size match the rewrite and the next scan leaves them
//! alone.

use std::collections::HashSet;
use std::path::PathBuf;

use gpui::{
    div, prelude::*, px, size, svg, uniform_list, App, Bounds, Context, Div, Entity, Global,
    PathPromptOptions, SharedString, Stateful, Subscription,
    UniformListScrollHandle, Window, WindowHandle,
};
use gpui_component::scroll::Scrollbar;
use gpui_component::spinner::Spinner;
use gpui_component::{Root, Sizable, Size};

use rox_library::writer::{self, Edit};

use crate::assets::icons;
use crate::backdrop::{NowPlayingArt, WindowBackdrop};
use crate::design::{palette, tokens};
use crate::panels::library::Library;
use crate::settings::ui::{small_button, MIN_SIZE};

/// How many files each detection hop reads before the count moves. Big
/// enough that the UI thread is not woken per file on a large library,
/// small enough that the "Scanning n/m" count still tracks a slow disk.
const CHUNK: usize = 256;

/// One file row's height. The list is a uniform_list, so every row agrees;
/// two lines fit, the name over its containing folder.
const ROW_H: f32 = 42.;

/// What a scan walks: every remembered library folder, or one folder the
/// user pointed at.
enum Scope {
    Library,
    Folder(PathBuf),
}

/// One affected file: the path to repair, its file name, and the folder it
/// sits in, so the list disambiguates the many "01. ....mp3" that share a
/// name across albums.
struct RepairRow {
    path: PathBuf,
    name: SharedString,
    folder: SharedString,
}

impl RepairRow {
    fn from_path(path: PathBuf) -> Self {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let folder = path
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        RepairRow {
            path,
            name: name.into(),
            folder: folder.into(),
        }
    }
}

/// The open repair window, if any. Only one makes sense at a time, and a
/// scan or repair in flight is not worth losing to a second one, so asking
/// again just brings this one forward.
#[derive(Default)]
struct OpenTagRepair(Option<WindowHandle<Root>>);

impl Global for OpenTagRepair {}

/// Open the tag repair window, or bring the open one forward. Takes the
/// shared catalog it scans and repairs into and the art bake it backs with,
/// so the settings window can open it from what it already holds.
pub fn open(library: Entity<Library>, now_art: Entity<NowPlayingArt>, cx: &mut App) {
    if let Some(handle) = cx.try_global::<OpenTagRepair>().and_then(|o| o.0) {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    let bounds = Bounds::centered(None, size(px(720.), px(600.)), cx);
    let handle = crate::panel::open_child_window(cx, "rox - Tag Repair", bounds, Some(MIN_SIZE), move |_window, cx| {
        cx.new(|cx| TagRepair::new(library, now_art, cx))
    });
    cx.set_global(OpenTagRepair(Some(handle)));
}

pub struct TagRepair {
    library: Entity<Library>,
    scope: Scope,
    /// A scan is walking and reading; the controls lock and the count moves
    /// as each chunk lands.
    scanning: bool,
    scan_done: usize,
    scan_total: usize,
    /// Whether a scan has finished at least once, so the list can say "none
    /// found" rather than an empty page before the first scan.
    scanned: bool,
    /// The affected files this scan found, each with its checkbox.
    found: Vec<RepairRow>,
    checked: Vec<bool>,
    /// A repair is committing; the list locks under an occluder and the
    /// count moves per file.
    repairing: bool,
    repair_done: usize,
    repair_total: usize,
    /// The last repair's summary, held over the list after it lands.
    result: Option<SharedString>,
    /// A scan or repair failure, shown inline.
    error: Option<SharedString>,
    scroll: UniformListScrollHandle,
    now_art: Entity<NowPlayingArt>,
    backdrop: WindowBackdrop,
    /// This window pumps its own frames, so the backdrop needs its own wake
    /// on a new bake.
    _backdrop_changed: Subscription,
}

impl TagRepair {
    fn new(library: Entity<Library>, now_art: Entity<NowPlayingArt>, cx: &mut Context<Self>) -> Self {
        let _backdrop_changed = cx.observe(&now_art, |_, _, cx| cx.notify());
        TagRepair {
            library,
            scope: Scope::Library,
            scanning: false,
            scan_done: 0,
            scan_total: 0,
            scanned: false,
            found: Vec::new(),
            checked: Vec::new(),
            repairing: false,
            repair_done: 0,
            repair_total: 0,
            result: None,
            error: None,
            scroll: UniformListScrollHandle::new(),
            now_art,
            backdrop: WindowBackdrop::default(),
            _backdrop_changed,
        }
    }

    /// Point the scope at the whole library and clear any prior results, so
    /// the next scan reads the folders fresh.
    fn set_scope_library(&mut self, cx: &mut Context<Self>) {
        self.scope = Scope::Library;
        self.reset_results();
        cx.notify();
    }

    /// Open the native folder picker; a pick sets the scope to that folder
    /// and clears prior results.
    fn pick_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = rx.await {
                if let Some(root) = paths.pop() {
                    this.update(cx, |this, cx| {
                        this.scope = Scope::Folder(root);
                        this.reset_results();
                        cx.notify();
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    /// Forget the last scan's hits and summary; a scope change or a fresh
    /// scan starts from nothing.
    fn reset_results(&mut self) {
        self.scanned = false;
        self.found.clear();
        self.checked.clear();
        self.result = None;
        self.error = None;
    }

    /// Walk the scope and flag every file carrying the broken tag shape.
    /// The walk and the per-file reads run off the UI thread; the count
    /// advances a chunk at a time so a slow disk still shows progress.
    fn scan(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.scanning || self.repairing {
            return;
        }
        let roots = match &self.scope {
            Scope::Library => self.library.read(cx).roots(),
            Scope::Folder(path) => vec![path.clone()],
        };
        if roots.is_empty() {
            self.error = Some("No folder to scan; add one to the library or pick one.".into());
            cx.notify();
            return;
        }
        self.scanning = true;
        self.reset_results();
        self.scan_done = 0;
        self.scan_total = 0;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            // The filesystem walk first, so the total is known before the
            // reads that show progress against it.
            let paths = cx
                .background_executor()
                .spawn(async move {
                    let mut out = Vec::new();
                    for root in &roots {
                        out.extend(rox_library::scanner::audio_files(root));
                    }
                    out
                })
                .await;
            if this
                .update(cx, |this, cx| {
                    this.scan_total = paths.len();
                    cx.notify();
                })
                .is_err()
            {
                return;
            }
            for chunk in paths.chunks(CHUNK) {
                let chunk: Vec<PathBuf> = chunk.to_vec();
                let n = chunk.len();
                let hits = cx
                    .background_executor()
                    .spawn(async move {
                        chunk
                            .into_iter()
                            .filter(|path| rox_library::tag_source::needs_unsync_repair(path))
                            .collect::<Vec<_>>()
                    })
                    .await;
                // Land the chunk's hits into the list as it goes, so the
                // first affected files show while the rest of the scan runs
                // instead of only at the end. A closed window (the user gave
                // up) drops the handle; stop rather than keep reading into
                // nothing.
                if this
                    .update(cx, |this, cx| {
                        for path in hits {
                            this.found.push(RepairRow::from_path(path));
                            this.checked.push(true);
                        }
                        this.scan_done = (this.scan_done + n).min(this.scan_total);
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
            this.update(cx, |this, cx| {
                this.scanning = false;
                this.scanned = true;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Flip one file's checkbox.
    fn toggle(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(c) = self.checked.get_mut(i) {
            *c = !*c;
            cx.notify();
        }
    }

    /// Check or uncheck every file at once.
    fn select_all(&mut self, on: bool, cx: &mut Context<Self>) {
        self.checked.iter_mut().for_each(|c| *c = on);
        cx.notify();
    }

    /// How many files are checked for repair.
    fn checked_count(&self) -> usize {
        self.checked.iter().filter(|&&c| c).count()
    }

    /// Repair every checked file: a no-op commit through the writer rewrites
    /// its tag clean, one file per background hop so the count moves and a
    /// slow file is visibly the one holding things up. A repaired file that
    /// lives under a library root reindexes so its stored mtime and size
    /// match the rewrite; the repaired rows drop off the list, and any that
    /// failed stay so the user sees which.
    fn repair(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.repairing || self.scanning {
            return;
        }
        let targets: Vec<PathBuf> = self
            .found
            .iter()
            .zip(&self.checked)
            .filter(|(_, &c)| c)
            .map(|(row, _)| row.path.clone())
            .collect();
        if targets.is_empty() {
            return;
        }
        self.repairing = true;
        self.repair_done = 0;
        self.repair_total = targets.len();
        self.result = None;
        self.error = None;
        cx.notify();
        let library = self.library.clone();
        cx.spawn_in(window, async move |this, cx| {
            let mut repaired: Vec<PathBuf> = Vec::new();
            let mut failures = 0usize;
            let mut first_error: Option<String> = None;
            for path in targets {
                // Note the write before it lands so the watch batch it
                // triggers is suppressed, not reindexed. The apply_edits at
                // the end notes too, but by then the suppression window has
                // long passed for all but the last few files of a big run.
                if library
                    .update(cx, |library, _| library.note_self_write([path.clone()]))
                    .is_err()
                {
                    return;
                }
                let (path, result) = cx
                    .background_executor()
                    .spawn(async move {
                        // The no-op edit that repairs: the writer re-reads
                        // through the sanitiser and writes the header flag
                        // cleared, so the saved file no longer carries the
                        // shape at all, all behind copy-verify-rename.
                        let r = writer::commit_with(&path, &[], &[]);
                        (path, r)
                    })
                    .await;
                match result {
                    Ok(()) => repaired.push(path),
                    Err(e) => {
                        failures += 1;
                        if first_error.is_none() {
                            let name = path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| path.display().to_string());
                            first_error = Some(format!("{name}: {e}"));
                        }
                    }
                }
                if this
                    .update(cx, |this, cx| {
                        this.repair_done += 1;
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
            this.update(cx, |this, cx| {
                // Reindex the repaired files under a library root so the
                // catalog agrees with the rewrite; a file outside every
                // root is repaired on disk but not pulled into the catalog.
                let roots = library.read(cx).roots();
                let edits: Vec<Edit> = repaired
                    .iter()
                    .filter(|path| roots.iter().any(|root| path.starts_with(root)))
                    .map(|path| Edit {
                        path: path.clone(),
                        changes: Vec::new(),
                        pictures: Vec::new(),
                    })
                    .collect();
                if !edits.is_empty() {
                    library.update(cx, |library, cx| library.apply_edits(&edits, cx));
                }
                let done: HashSet<PathBuf> = repaired.into_iter().collect();
                let kept: Vec<RepairRow> = std::mem::take(&mut this.found)
                    .into_iter()
                    .filter(|row| !done.contains(&row.path))
                    .collect();
                this.found = kept;
                this.checked = vec![true; this.found.len()];
                this.repairing = false;
                let n = done.len();
                this.result = Some(if failures > 0 {
                    format!("Repaired {n}, {failures} failed").into()
                } else if n == 1 {
                    "Repaired 1 file".into()
                } else {
                    format!("Repaired {n} files").into()
                });
                this.error = first_error.map(Into::into);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The scope row: the whole-library pill beside the folder pill, the
    /// active one lit like a picked control.
    fn scope_row(&self, cx: &mut Context<Self>) -> Div {
        let busy = self.scanning || self.repairing;
        let folder_label: SharedString = match &self.scope {
            Scope::Folder(path) => path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string())
                .into(),
            Scope::Library => "Pick a folder...".into(),
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .child(
                div()
                    .w(px(56.))
                    .flex_none()
                    .text_color(palette::text_muted())
                    .child("scope"),
            )
            .child(pill(
                "Whole library",
                0,
                matches!(self.scope, Scope::Library),
                busy,
                cx.listener(|this, _, _, cx| this.set_scope_library(cx)),
            ))
            .child(pill(
                folder_label,
                1,
                matches!(self.scope, Scope::Folder(_)),
                busy,
                cx.listener(|this, _, window, cx| this.pick_folder(window, cx)),
            ))
    }

    /// The results region under the scope row, filling the rest of the
    /// window: a centered hint before the first scan, a "none found" line
    /// when a scan came up clean, or the select-all header over the
    /// virtualized file list. Rows stream in during a scan, so the list
    /// shows as soon as `found` has anything, before the scan finishes.
    fn results(&self, cx: &mut Context<Self>) -> Div {
        let region = div().flex_1().min_h_0().flex().flex_col();
        if self.found.is_empty() {
            let message = if !self.scanned {
                "Scan to find files carrying the broken ID3v2.4 tag shape."
            } else {
                "No affected files found."
            };
            return region.child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(palette::text_muted())
                    .child(message),
            );
        }
        let all = self.checked.iter().all(|&c| c);
        let count = self.found.len();
        // The count reads "so far" while rows are still streaming, so it is
        // honest about a scan that is not done yet.
        let count_label = match (self.scanning, count) {
            (true, n) => format!("{n} so far"),
            (false, 1) => "1 file".to_string(),
            (false, n) => format!("{n} files"),
        };
        let this = cx.entity().downgrade();
        region
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .pb(tokens::SPACE_XS)
                    .border_b_1()
                    .border_color(palette::border())
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(count_label)
                    .child(small_button(
                        if all { "Select none" } else { "Select all" },
                        icons::CHECK,
                        self.repairing,
                        cx.listener(move |this, _, _, cx| this.select_all(!all, cx)),
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(
                        uniform_list("repair-files", count, move |range, _, cx| {
                            this.upgrade()
                                .map(|this| this.update(cx, |this, cx| this.file_rows(range, cx)))
                                .unwrap_or_default()
                        })
                        .track_scroll(self.scroll.clone())
                        .size_full(),
                    )
                    .child(div().absolute().inset_0().child(Scrollbar::vertical(&self.scroll)))
                    // The list locks while a repair runs: a transparent
                    // occluder over it swallows clicks so nothing checks or
                    // unchecks out from under the commits.
                    .when(self.repairing, |d| {
                        d.child(div().absolute().inset_0().occlude())
                    }),
            )
    }

    /// The visible slice of file rows for the virtualized list: each a
    /// checkbox and the file name over its folder, the whole row a click
    /// target so the box is easy to hit.
    fn file_rows(
        &self,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        range
            .filter_map(|i| {
                let row = self.found.get(i)?;
                let checked = self.checked.get(i).copied().unwrap_or(false);
                Some(
                    div()
                        .id(("repair-file", i))
                        .w_full()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(tokens::SPACE_SM)
                        .h(px(ROW_H))
                        .px(tokens::SPACE_XS)
                        .rounded(tokens::RADIUS)
                        .cursor_pointer()
                        .hover(|d| d.bg(palette::bg_control_hover()))
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle(i, cx)))
                        .child(checkbox(checked))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(div().truncate().child(row.name.clone()))
                                .when(!row.folder.is_empty(), |d| {
                                    d.child(
                                        div()
                                            .text_xs()
                                            .text_color(palette::text_muted())
                                            .truncate()
                                            .child(row.folder.clone()),
                                    )
                                }),
                        ),
                )
            })
            .collect()
    }

    /// The section header: the "Repair" label with the scan and repair
    /// controls trailing it, on the same border the settings sections wear.
    fn header(&self, cx: &mut Context<Self>) -> Div {
        let busy = self.scanning || self.repairing;
        let count = self.checked_count();
        let controls = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .when(busy, |d| {
                let label = if self.scanning {
                    if self.scan_total > 0 {
                        format!("Scanning {}/{}...", self.scan_done, self.scan_total)
                    } else {
                        "Scanning...".to_string()
                    }
                } else {
                    let at = (self.repair_done + 1).min(self.repair_total);
                    format!("Repairing {}/{}...", at, self.repair_total)
                };
                d.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(tokens::SPACE_XS)
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(Spinner::new().with_size(Size::Small))
                        .child(label),
                )
            })
            .child(small_button(
                if self.scanned { "Rescan" } else { "Scan" },
                icons::SEARCH,
                busy,
                cx.listener(|this, _, window, cx| this.scan(window, cx)),
            ))
            .child(small_button(
                if count > 0 {
                    format!("Repair ({count})")
                } else {
                    "Repair".to_string()
                },
                icons::CHECK,
                busy || count == 0,
                cx.listener(|this, _, window, cx| this.repair(window, cx)),
            ));
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .pb(tokens::SPACE_XS)
            .border_b_1()
            .border_color(palette::border())
            .child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child("Repair"),
            )
            .child(controls)
    }
}

/// A scope pill: an active one lit like a picked control, the rest a plain
/// hoverable chip; both drop the click while a scan or repair runs.
fn pill(
    label: impl Into<SharedString>,
    id: usize,
    active: bool,
    inert: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(("repair-scope", id))
        .flex_none()
        .px(tokens::SPACE_SM)
        .py(px(2.))
        .rounded(tokens::RADIUS)
        .text_xs()
        .map(|d| {
            if active {
                d.bg(palette::bg_control_active()).text_color(palette::text())
            } else {
                d.bg(palette::bg_control()).text_color(palette::text_muted())
            }
        })
        .map(|d| {
            if inert {
                d.opacity(0.5)
            } else {
                d.cursor_pointer()
                    .when(!active, |d| d.hover(|d| d.bg(palette::bg_control_hover())))
                    .on_click(on_click)
            }
        })
        .child(label.into())
}

/// A checkbox glyph: an accent-filled box with a check when set, a hollow
/// bordered box when clear.
fn checkbox(checked: bool) -> Div {
    div()
        .size(px(16.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(tokens::RADIUS)
        .border_1()
        .map(|d| {
            if checked {
                d.bg(palette::accent()).border_color(palette::accent())
            } else {
                d.border_color(palette::border())
            }
        })
        .when(checked, |d| {
            d.child(
                svg()
                    .path(icons::CHECK)
                    .size(px(11.))
                    .text_color(palette::text_on_accent()),
            )
        })
}

impl Render for TagRepair {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The header, scope, and summary stay fixed; only the file list
        // scrolls, and it virtualizes, so a scan of the whole library stays
        // responsive no matter how many files it turns up.
        let page = div()
            .id("tag-repair-page")
            .size_full()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .p(tokens::SPACE_MD)
            .child(self.header(cx))
            .child(self.scope_row(cx))
            .child(self.results(cx))
            .when_some(self.result.clone(), |d, result| {
                d.child(div().text_color(palette::text_muted()).child(result))
            })
            .when_some(self.error.clone(), |d, error| {
                d.child(div().text_color(palette::text_muted()).child(error))
            });

        div()
            .size_full()
            .flex()
            .flex_row()
            .bg(palette::bg_elevated())
            .text_color(palette::text_bright())
            .text_sm()
            // The backdrop paints first, under the page, so translucent
            // surfaces sink into the playing track's art like every window.
            .children(self.backdrop.layer(&self.now_art, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .bg(palette::bg_elevated())
                    .child(page),
            )
    }
}
