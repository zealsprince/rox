//! The tag repair window: find and rewrite files whose tags lofty (through
//! 0.24) reads mangled or refuses to write: the ID3v2.4 double-unsync shape,
//! a stray null on a UTF-16 frame, and junk padding past the declared tag.
//! `tag_source` already tolerates these on read, but the bytes on disk stay
//! broken for every other tool. A no-op commit through the writer repairs a
//! file for good, behind its copy-verify-rename.
//!
//! A file whose tags fail to parse even sanitised is listed too, unchecked,
//! with its error: the rewrite can't mend it, but a repair scan shouldn't
//! pass it silently. Repaired files under a library root reindex so the next
//! scan leaves them alone.

use std::collections::HashSet;
use std::path::PathBuf;

use gpui::{
    App, Bounds, Context, Div, Entity, FocusHandle, Global, KeyBinding, PathPromptOptions,
    SharedString, Stateful, Subscription, UniformListScrollHandle, Window, WindowHandle, actions,
    div, prelude::*, px, size, uniform_list,
};
use gpui_component::scroll::Scrollbar;
use gpui_component::spinner::Spinner;
use gpui_component::{Root, Sizable, Size};

use rox_library::writer::{self, Edit};

use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_kit::ui::{MIN_SIZE, Seg, checkbox, kbd_line, section, small_button};
use rox_services::backdrop::{NowPlayingArt, WindowBackdrop};
use rox_services::catalog::Library;

/// Big enough not to wake the UI thread per file, small enough that the
/// count still tracks a slow disk.
const CHUNK: usize = 256;

const ROW_H: f32 = 42.;

actions!(tag_repair, [Repair]);

const CONTEXT: &str = "TagRepair";

/// Nothing here takes typing, so the root holds focus and the binding.
pub fn bindings() -> Vec<KeyBinding> {
    vec![KeyBinding::new("enter", Repair, Some(CONTEXT))]
}

enum Scope {
    Library,
    Folder(PathBuf),
}

/// `issue` holds the parse error of a file the rewrite can't repair.
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

    /// An unparseable file starts unchecked, since its commit would only fail.
    fn repairable(&self) -> bool {
        self.issue.is_none()
    }
}

/// One at a time: a scan or repair in flight isn't worth losing to a second.
#[derive(Default)]
struct OpenTagRepair(Option<WindowHandle<Root>>);

impl Global for OpenTagRepair {}

pub fn open(library: Entity<Library>, now_art: Entity<NowPlayingArt>, cx: &mut App) {
    if let Some(handle) = cx.try_global::<OpenTagRepair>().and_then(|o| o.0)
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
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
    scanning: bool,
    scan_done: usize,
    scan_total: usize,
    scanned: bool,
    found: Vec<RepairRow>,
    checked: Vec<bool>,
    repairing: bool,
    repair_done: usize,
    repair_total: usize,
    result: Option<SharedString>,
    error: Option<SharedString>,
    scroll: UniformListScrollHandle,
    /// No field takes typing, so the enter binding needs this to attach to.
    focus: FocusHandle,
    now_art: Entity<NowPlayingArt>,
    backdrop: WindowBackdrop,
    /// This window pumps its own frames, so the backdrop needs its own wake.
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

    fn set_scope_library(&mut self, cx: &mut Context<Self>) {
        self.scope = Scope::Library;
        self.reset_results();
        cx.notify();
    }

    fn pick_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = rx.await
                && let Some(root) = paths.pop()
            {
                this.update(cx, |this, cx| {
                    this.scope = Scope::Folder(root);
                    this.reset_results();
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn reset_results(&mut self) {
        self.scanned = false;
        self.found.clear();
        self.checked.clear();
        self.result = None;
        self.error = None;
    }

    fn scan(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.scanning || self.repairing {
            return;
        }
        // The library scope skips what the scan skips; a hand-picked folder walks
        // whole.
        let (roots, exclude) = match &self.scope {
            Scope::Library => {
                let library = self.library.read(cx);
                (library.roots(), library.exclusions())
            }

            Scope::Folder(path) => (vec![path.clone()], Default::default()),
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
            let paths = cx
                .background_executor()
                .spawn(async move {
                    let mut out = Vec::new();
                    for root in &roots {
                        out.extend(rox_library::scanner::audio_files(root, &exclude));
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
                // A closed window drops the handle; stop reading.
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

    fn toggle(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(c) = self.checked.get_mut(i) {
            *c = !*c;
            cx.notify();
        }
    }

    fn select_all(&mut self, on: bool, cx: &mut Context<Self>) {
        self.checked.iter_mut().for_each(|c| *c = on);
        cx.notify();
    }

    fn checked_count(&self) -> usize {
        self.checked.iter().filter(|&&c| c).count()
    }

    /// One file per background hop, so the count moves and a slow file shows.
    /// Failed rows stay on the list.
    fn repair(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.repairing || self.scanning {
            return;
        }
        let targets: Vec<PathBuf> = self
            .found
            .iter()
            .zip(&self.checked)
            .filter(|&(_, &c)| c)
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
                // Note each write just before it happens: the apply_edits at the end is
                // too late for the suppression window on all but the last few files.
                if library
                    .update(cx, |library, _| library.note_self_write([path.clone()]))
                    .is_err()
                {
                    return;
                }
                let (path, result) = cx
                    .background_executor()
                    .spawn(async move {
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
                // A file outside every root is repaired on disk but not pulled into the
                // catalog.
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

    fn results(&self, cx: &mut Context<Self>) -> Stateful<Div> {
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
            // Lock the list while the commits run.
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

    fn header(&self, cx: &mut Context<Self>) -> Stateful<Div> {
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
        section(
            rox_i18n::t!("tags-repair-section"),
            Some(controls),
            self.scope_row(cx),
        )
    }

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
                        rox_i18n::t!("settings-common-cancel"),
                        icons::CLOSE,
                        false,
                        cx.listener(|_, _, window, _| window.remove_window()),
                    )),
            )
    }
}

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
        let page = div()
            .id("tag-repair-page")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .p(tokens::SPACE_MD)
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
            .children(self.backdrop.layer(&self.now_art, window, cx))
            .child(page)
            .child(self.footer(cx))
    }
}
