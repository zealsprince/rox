//! The tag repair window: find and rewrite the files carrying tag shapes
//! lofty (through 0.24) reads mangled or refuses to write: the ID3v2.4
//! double-unsync shape, the stray null on a UTF-16 text frame, and zero
//! padding left outside the declared tag size. Reads already tolerate
//! these through `tag_source`, so the library shows such files right; the
//! bytes on disk stay broken, and any tool without the same workarounds
//! trips on them. A commit through the writer repairs a file for good, so
//! this window is the way to run that repair across a selection without
//! editing a field by hand.
//!
//! Scope is the whole library (every remembered folder) or one folder the
//! user picks. A scan walks the scope, flags each file the writer's
//! rewrite would repair (the `tag_source::needs_repair` gate), and lists
//! the hits with a checkbox each. A file whose tags fail to parse even
//! through the sanitiser is listed too, unchecked, with its error in
//! place of its folder: the rewrite cannot mend it yet, but it should
//! not pass a repair scan silently. Repair commits a no-op edit to every
//! checked file through the writer's atomic layer, so the
//! copy-verify-rename safety guards every rewrite. Repaired files that
//! live under a library root reindex so their stored mtime and size
//! match the rewrite and the next scan leaves them alone.

use std::collections::HashSet;
use std::path::PathBuf;

use gpui::{
    actions, div, prelude::*, px, size, uniform_list, App, Bounds, Context, Div, Entity,
    FocusHandle, Global, KeyBinding, PathPromptOptions, SharedString, Stateful, Subscription,
    UniformListScrollHandle, Window, WindowHandle,
};
use gpui_component::scroll::Scrollbar;
use gpui_component::spinner::Spinner;
use gpui_component::{Root, Sizable, Size};

use rox_library::writer::{self, Edit};

use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_kit::ui::{checkbox, kbd_line, section, small_button, Seg, MIN_SIZE};
use rox_services::backdrop::{NowPlayingArt, WindowBackdrop};
use rox_services::catalog::Library;

/// How many files each detection hop reads before the count moves. Big
/// enough that the UI thread is not woken per file on a large library,
/// small enough that the "Scanning n/m" count still tracks a slow disk.
const CHUNK: usize = 256;

/// One file row's height. The list is a uniform_list, so every row agrees;
/// two lines fit, the name over its containing folder.
const ROW_H: f32 = 42.;

actions!(tag_repair, [Repair]);

/// The key context the window's own bindings scope to.
const CONTEXT: &str = "TagRepair";

/// The window's repair binding; call once at startup. Nothing here takes
/// typing, so the binding sits on the window root and the root holds the
/// focus, which is what puts it on the dispatch path.
pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("enter", Repair, Some(CONTEXT))]);
}

/// What a scan walks: every remembered library folder, or one folder the
/// user pointed at.
enum Scope {
    Library,
    Folder(PathBuf),
}

/// One affected file: the path to repair, its file name, and the folder it
/// sits in, so the list disambiguates the many "01. ....mp3" that share a
/// name across albums. A file the rewrite cannot repair carries its parse
/// error instead, shown in the folder's place.
struct RepairRow {
    path: PathBuf,
    name: SharedString,
    folder: SharedString,
    issue: Option<SharedString>,
}

impl RepairRow {
    fn from_path(path: PathBuf, issue: Option<String>) -> Self {
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
            issue: issue.map(Into::into),
        }
    }

    /// Whether the writer's rewrite can repair this file; an unparseable
    /// one is listed for visibility but starts unchecked, since its
    /// commit would only fail.
    fn repairable(&self) -> bool {
        self.issue.is_none()
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
    let handle = rox_panel_api::panel::open_child_window(
        cx,
        rox_i18n::t!("tags-repair-window-title"),
        bounds,
        Some(MIN_SIZE),
        move |window, cx| cx.new(|cx| TagRepair::new(library, now_art, window, cx)),
    );
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
    /// The window root's own focus. No field here takes typing, so without
    /// it the enter binding would have nothing to hang off.
    focus: FocusHandle,
    now_art: Entity<NowPlayingArt>,
    backdrop: WindowBackdrop,
    /// This window pumps its own frames, so the backdrop needs its own wake
    /// on a new bake.
    _backdrop_changed: Subscription,
}

impl TagRepair {
    fn new(
        library: Entity<Library>,
        now_art: Entity<NowPlayingArt>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let _backdrop_changed = cx.observe(&now_art, |_, _, cx| cx.notify());
        let focus = cx.focus_handle();
        window.focus(&focus);
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
            focus,
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
            self.error = Some(rox_i18n::t!("tags-repair-no-folder"));
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
                            .filter_map(|path| {
                                // The repairable shapes first, then the
                                // files whose tags fail to parse even
                                // sanitised: those cannot be rewritten yet,
                                // but a repair scan must not pass them
                                // silently.
                                if rox_library::tag_source::needs_repair(&path) {
                                    return Some((path, None));
                                }
                                match rox_library::writer::readable(&path) {
                                    Ok(()) => None,
                                    Err(e) => Some((path, Some(e))),
                                }
                            })
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
                        for (path, issue) in hits {
                            let row = RepairRow::from_path(path, issue);
                            this.checked.push(row.repairable());
                            this.found.push(row);
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
                    // The repair rewrites a file's header and names no columns, so
                    // there is no per-row sub for it to land on.
                    library.update(cx, |library, cx| library.apply_edits(&edits, &[], cx));
                }
                let done: HashSet<PathBuf> = repaired.into_iter().collect();
                let kept: Vec<RepairRow> = std::mem::take(&mut this.found)
                    .into_iter()
                    .filter(|row| !done.contains(&row.path))
                    .collect();
                this.found = kept;
                this.checked = this.found.iter().map(RepairRow::repairable).collect();
                this.repairing = false;
                let n = done.len();
                this.result = Some(if failures > 0 {
                    rox_i18n::t!(
                        "tags-repair-result-failed",
                        count = n as u64,
                        failed = failures as u64
                    )
                } else {
                    rox_i18n::t!("tags-repair-result", count = n as u64)
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
            Scope::Library => rox_i18n::t!("tags-repair-pick-folder"),
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
                    .child(rox_i18n::t!("tags-repair-label-scope")),
            )
            .child(pill(
                rox_i18n::t!("tags-repair-whole-library"),
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
    /// when a scan came up clean, or the count and select-all riding the
    /// heading over the virtualized list. Rows stream in during a scan, so the list
    /// shows as soon as `found` has anything, before the scan finishes.
    fn results(&self, cx: &mut Context<Self>) -> Div {
        if self.found.is_empty() {
            let message = if !self.scanned {
                rox_i18n::t!("tags-repair-scan-hint")
            } else {
                rox_i18n::t!("tags-repair-no-affected")
            };
            return section(
                rox_i18n::t!("tags-repair-affected-files"),
                None,
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(palette::text_muted())
                    .child(message),
            )
            .flex_1()
            .min_h_0();
        }
        let all = self.checked.iter().all(|&c| c);
        let count = self.found.len();
        // The count reads "so far" while rows are still streaming, so it is
        // honest about a scan that is not done yet.
        let count_label = if self.scanning {
            rox_i18n::t!("tags-repair-count-so-far", count = count as u64)
        } else {
            rox_i18n::t!("tags-repair-count", count = count as u64)
        };
        let this = cx.entity().downgrade();
        let trailing = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .text_xs()
            .text_color(palette::text_muted())
            .child(count_label)
            .child(small_button(
                if all {
                    rox_i18n::t!("tags-repair-select-none")
                } else {
                    rox_i18n::t!("tags-repair-select-all")
                },
                icons::CHECK,
                self.repairing,
                cx.listener(move |this, _, _, cx| this.select_all(!all, cx)),
            ))
            .into_any_element();
        let list = div()
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
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .child(Scrollbar::vertical(&self.scroll)),
            )
            // The list locks while a repair runs: a transparent occluder
            // over it swallows clicks so nothing checks or unchecks out
            // from under the commits.
            .when(self.repairing, |d| {
                d.child(div().absolute().inset_0().occlude())
            });
        section(
            rox_i18n::t!("tags-repair-affected-files"),
            Some(trailing),
            list,
        )
        .flex_1()
        .min_h_0()
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
                        .h(palette::scaled_px(ROW_H))
                        .px(tokens::SPACE_XS)
                        .rounded(tokens::RADIUS)
                        .cursor_pointer()
                        .hover(|d| d.bg(palette::bg_control_hover()))
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle(i, cx)))
                        .child(checkbox(checked))
                        .child({
                            // The parse error outranks the folder on the
                            // second line: an unrepairable file's row has
                            // to say why it sits here unchecked.
                            let detail = row.issue.clone().unwrap_or_else(|| row.folder.clone());
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(div().truncate().child(row.name.clone()))
                                .when(!detail.is_empty(), |d| {
                                    d.child(
                                        div()
                                            .text_xs()
                                            .text_color(palette::text_muted())
                                            .truncate()
                                            .child(detail),
                                    )
                                })
                        }),
                )
            })
            .collect()
    }

    /// The scope pills under a heading that carries Scan, and the count a
    /// running scan moves.
    fn header(&self, cx: &mut Context<Self>) -> Div {
        let busy = self.scanning || self.repairing;
        let controls = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .when(self.scanning, |d| {
                let label = if self.scan_total > 0 {
                    format!("Scanning {}/{}...", self.scan_done, self.scan_total)
                } else {
                    "Scanning...".to_string()
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
            .into_any_element();
        section("Repair", Some(controls), self.scope_row(cx))
    }

    /// The window's actions, and what the shortcut is doing or why it is
    /// off, over what the last repair left behind.
    fn footer(&self, cx: &mut Context<Self>) -> Div {
        let busy = self.scanning || self.repairing;
        let count = self.checked_count();
        let hint = if self.repairing {
            let at = (self.repair_done + 1).min(self.repair_total);
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_XS)
                .text_xs()
                .text_color(palette::text_muted())
                .child(Spinner::new().with_size(Size::Small))
                .child(rox_i18n::t!(
                    "tags-repair-progress",
                    done = at as u64,
                    total = self.repair_total as u64
                ))
                .into_any_element()
        } else if !self.scanned || count == 0 {
            div()
                .text_xs()
                .text_color(palette::tone_warn())
                .child(if self.scanned {
                    rox_i18n::t!("tags-repair-check-to-repair")
                } else {
                    rox_i18n::t!("tags-repair-scan-first")
                })
                .into_any_element()
        } else {
            kbd_line([
                Seg::Text("Press".into()),
                Seg::Key("Enter".into()),
                Seg::Text("to repair".into()),
            ])
            .text_xs()
            .into_any_element()
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
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .gap(tokens::SPACE_XS)
                    .child(hint)
                    .when_some(self.result.clone(), |d, result| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(palette::text_muted())
                                .child(result),
                        )
                    })
                    .when_some(self.error.clone(), |d, error| {
                        d.child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(palette::tone_bad())
                                .child(error),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_none()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .child(small_button(
                        rox_i18n::t!("tags-repair-repair-button", count = count as u64),
                        icons::CHECK,
                        busy || count == 0,
                        cx.listener(|this, _, window, cx| this.repair(window, cx)),
                    ))
                    .child(small_button(
                        "Cancel",
                        icons::CLOSE,
                        false,
                        cx.listener(|_, _, window, _| window.remove_window()),
                    )),
            )
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
                d.bg(palette::bg_control_active())
                    .text_color(palette::text())
            } else {
                d.bg(palette::bg_control())
                    .text_color(palette::text_muted())
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

impl Render for TagRepair {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The scope and the footer stay fixed; only the file list scrolls,
        // and it virtualizes, so a scan of the whole library stays
        // responsive no matter how many files it turns up.
        let page = div()
            .id("tag-repair-page")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .p(tokens::SPACE_MD)
            // The page's own surface, a second elevated layer over the
            // window's, the same as the settings page. Two layers is what
            // the backdrop reads through everywhere.
            .bg(palette::bg_elevated())
            .child(self.header(cx))
            .child(self.results(cx));

        div()
            .size_full()
            .flex()
            .flex_col()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &Repair, window, cx| this.repair(window, cx)))
            .bg(palette::bg_elevated())
            .text_color(palette::text_bright())
            .text_sm()
            // The backdrop paints first, under the page, so translucent
            // surfaces sink into the playing track's art like every window.
            .children(self.backdrop.layer(&self.now_art, window, cx))
            .child(page)
            .child(self.footer(cx))
    }
}
