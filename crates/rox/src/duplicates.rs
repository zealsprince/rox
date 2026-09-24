//! The duplicates window: tracks the library holds more than once, matched
//! on title and artist within a duration tolerance over the projection, with
//! the spare copies moved to the OS trash.
//!
//! A group whose copies span albums is never auto-checked: trashing one would
//! leave a hole in an album. No group can have every copy checked, so the
//! tool can't take a track's last copy. Trash only, never a plain unlink.

use std::collections::HashSet;
use std::path::PathBuf;

use gpui::{
    App, Bounds, Context, Div, Entity, Global, ObjectFit, SharedString, Stateful, Subscription,
    UniformListScrollHandle, Window, WindowHandle, div, img, prelude::*, px, size, svg,
    uniform_list,
};
use gpui_component::button::Button;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_component::scroll::Scrollbar;
use gpui_component::spinner::Spinner;
use gpui_component::{Root, Sizable, Size};
use rox_core::fmt::fmt_ms;
use rox_library::duplicates::match_duplicates;

use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_kit::ui::{MIN_SIZE, block_header, checkbox, section, small_button};
use rox_services::backdrop::{NowPlayingArt, WindowBackdrop};
use rox_services::catalog::Library;
use rox_services::thumbs::{Thumb, Thumbs};

const ROW_H: f32 = 42.;

const COVER: f32 = 32.;

#[derive(Clone, Copy, PartialEq, Eq)]
enum KeepPolicy {
    /// Highest bitrate, earliest added on a tie.
    Quality,
    /// Earliest added, highest bitrate on a tie.
    Oldest,
    /// Latest added, highest bitrate on a tie.
    Newest,
}

impl KeepPolicy {
    fn label(self) -> SharedString {
        match self {
            KeepPolicy::Quality => rox_i18n::t!("duplicates-policy-quality"),
            KeepPolicy::Oldest => rox_i18n::t!("duplicates-policy-oldest"),
            KeepPolicy::Newest => rox_i18n::t!("duplicates-policy-newest"),
        }
    }
}

struct DupMember {
    path: PathBuf,
    name: SharedString,
    /// The full parent path: duplicates often sit in folders named alike.
    folder: SharedString,
    codec: SharedString,
    bitrate_kbps: u16,
    added: i64,
}

struct DupGroup {
    title: SharedString,
    artist: SharedString,
    duration_ms: u32,
    same_album: bool,
    members: Vec<DupMember>,
}

#[derive(Clone, Copy)]
enum RowKind {
    Header(usize),
    Member(usize, usize),
}

/// One at a time: a scan or delete in flight isn't worth losing to a second copy.
#[derive(Default)]
struct OpenDuplicates(Option<WindowHandle<Root>>);

impl Global for OpenDuplicates {}

pub fn open(
    library: Entity<Library>,
    thumbs: Entity<Thumbs>,
    now_art: Entity<NowPlayingArt>,
    cx: &mut App,
) {
    if let Some(handle) = cx.try_global::<OpenDuplicates>().and_then(|o| o.0)
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let bounds = Bounds::centered(None, size(px(760.), px(600.)), cx);
    let handle = rox_panel_api::panel::open_child_window(
        cx,
        rox_i18n::t!("duplicates-window-title"),
        bounds,
        Some(MIN_SIZE),
        move |window, cx| cx.new(|cx| Duplicates::new(library, thumbs, now_art, window, cx)),
    );
    cx.set_global(OpenDuplicates(Some(handle)));
}

pub struct Duplicates {
    library: Entity<Library>,
    thumbs: Entity<Thumbs>,
    scanning: bool,
    scanned: bool,
    groups: Vec<DupGroup>,
    checked: Vec<Vec<bool>>,
    policy: KeepPolicy,
    /// `query` is kept lowercased.
    query_input: Entity<InputState>,
    query: String,
    /// Rebuilt whenever `groups` or the filter changes.
    rows: Vec<RowKind>,
    trashing: bool,
    trash_done: usize,
    trash_total: usize,
    result: Option<SharedString>,
    error: Option<SharedString>,
    scroll: UniformListScrollHandle,
    now_art: Entity<NowPlayingArt>,
    backdrop: WindowBackdrop,
    _backdrop_changed: Subscription,
    _query_changed: Subscription,
}

impl Duplicates {
    fn new(
        library: Entity<Library>,
        thumbs: Entity<Thumbs>,
        now_art: Entity<NowPlayingArt>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let _backdrop_changed = cx.observe(&now_art, |_, _, cx| cx.notify());
        let query_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(rox_i18n::t!("duplicates-filter-placeholder"))
        });
        let _query_changed = cx.subscribe_in(
            &query_input,
            window,
            |this, input, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.query = input.read(cx).value().trim().to_lowercase();
                    this.rebuild_rows();
                    cx.notify();
                }
            },
        );
        Duplicates {
            library,
            thumbs,
            scanning: false,
            scanned: false,
            groups: Vec::new(),
            checked: Vec::new(),
            policy: KeepPolicy::Quality,
            query_input,
            query: String::new(),
            rows: Vec::new(),
            trashing: false,
            trash_done: 0,
            trash_total: 0,
            result: None,
            error: None,
            scroll: UniformListScrollHandle::new(),
            now_art,
            backdrop: WindowBackdrop::default(),
            _backdrop_changed,
            _query_changed,
        }
    }

    fn rebuild_rows(&mut self) {
        self.rows.clear();
        for (g, group) in self.groups.iter().enumerate() {
            if !self.query.is_empty() && !group_matches(group, &self.query) {
                continue;
            }
            self.rows.push(RowKind::Header(g));
            for m in 0..group.members.len() {
                self.rows.push(RowKind::Member(g, m));
            }
        }
    }

    fn scan(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.scanning || self.trashing {
            return;
        }
        let Some(projection) = self.library.read(cx).projection().cloned() else {
            self.error = Some(rox_i18n::t!("duplicates-library-loading"));
            cx.notify();
            return;
        };
        self.scanning = true;
        self.scanned = false;
        self.groups.clear();
        self.checked.clear();
        self.rows.clear();
        self.result = None;
        self.error = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let specs = cx
                .background_executor()
                .spawn(async move { match_duplicates(&projection) })
                .await;
            this.update(cx, |this, cx| {
                // A member whose row vanished mid-scan drops out, and a group under two
                // copies with it.
                let library = this.library.read(cx);
                for spec in specs {
                    let members: Vec<DupMember> = spec
                        .members
                        .into_iter()
                        .filter_map(|m| {
                            let path = library.paths_for(&[m.id]).ok().and_then(|mut p| p.pop())?;
                            let name = path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| path.display().to_string());
                            let folder = path
                                .parent()
                                .map(|p| p.display().to_string())
                                .unwrap_or_default();
                            Some(DupMember {
                                path,
                                name: name.into(),
                                folder: folder.into(),
                                codec: m.codec.into(),
                                bitrate_kbps: m.bitrate_kbps,
                                added: m.added,
                            })
                        })
                        .collect();
                    if members.len() < 2 {
                        continue;
                    }
                    this.groups.push(DupGroup {
                        title: spec.title.into(),
                        artist: spec.artist.into(),
                        duration_ms: spec.duration_ms,
                        same_album: spec.same_album,
                        members,
                    });
                }
                this.apply_policy();
                this.rebuild_rows();
                this.scanning = false;
                this.scanned = true;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn apply_policy(&mut self) {
        let policy = self.policy;
        for group in &mut self.groups {
            group.members.sort_by(|a, b| match policy {
                KeepPolicy::Quality => b
                    .bitrate_kbps
                    .cmp(&a.bitrate_kbps)
                    .then(a.added.cmp(&b.added)),
                KeepPolicy::Oldest => a
                    .added
                    .cmp(&b.added)
                    .then(b.bitrate_kbps.cmp(&a.bitrate_kbps)),
                KeepPolicy::Newest => b
                    .added
                    .cmp(&a.added)
                    .then(b.bitrate_kbps.cmp(&a.bitrate_kbps)),
            });
        }
        self.auto_select();
    }

    /// Held while a delete runs so the targets can't shift under it.
    fn set_policy(&mut self, policy: KeepPolicy, cx: &mut Context<Self>) {
        if self.trashing || policy == self.policy {
            return;
        }
        self.policy = policy;
        self.apply_policy();
        self.rebuild_rows();
        cx.notify();
    }

    /// The keeper stays unchecked; groups spanning albums stay untouched.
    fn auto_select(&mut self) {
        self.checked = self
            .groups
            .iter()
            .map(|g| {
                if !g.same_album {
                    return vec![false; g.members.len()];
                }
                let mut marks = vec![true; g.members.len()];
                if let Some(first) = marks.first_mut() {
                    *first = false;
                }
                marks
            })
            .collect();
    }

    fn select_none(&mut self) {
        for marks in &mut self.checked {
            marks.iter_mut().for_each(|c| *c = false);
        }
    }

    /// Checking a group's last unchecked copy swaps the keeper instead, so a
    /// group always keeps one.
    fn toggle(&mut self, g: usize, m: usize, cx: &mut Context<Self>) {
        let Some(marks) = self.checked.get_mut(g) else {
            return;
        };
        let Some(&on) = marks.get(m) else { return };
        if !on && marks.iter().enumerate().all(|(i, &c)| c || i == m) {
            marks[m] = true;
            if let Some(keeper) = (0..marks.len()).find(|&i| i != m) {
                marks[keeper] = false;
            }
        } else {
            marks[m] = !on;
        }
        cx.notify();
    }

    fn checked_count(&self) -> usize {
        self.checked
            .iter()
            .map(|marks| marks.iter().filter(|&&c| c).count())
            .sum()
    }

    /// Skips groups the filter hides, the same gate [`Self::trash`] applies.
    fn visible_checked_count(&self) -> usize {
        self.groups
            .iter()
            .enumerate()
            .filter(|(_, group)| self.query.is_empty() || group_matches(group, &self.query))
            .map(|(g, _)| {
                self.checked
                    .get(g)
                    .map(|marks| marks.iter().filter(|&&c| c).count())
                    .unwrap_or(0)
            })
            .sum()
    }

    /// One file per background hop so the count moves. Trashed files prune out
    /// of the catalog; failures stay listed.
    fn trash(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.trashing || self.scanning {
            return;
        }
        let mut targets: Vec<(usize, usize, PathBuf)> = Vec::new();
        for (g, group) in self.groups.iter().enumerate() {
            // Never trash marks the filter hides from view.
            if !self.query.is_empty() && !group_matches(group, &self.query) {
                continue;
            }
            for (m, member) in group.members.iter().enumerate() {
                let marked = self
                    .checked
                    .get(g)
                    .and_then(|marks| marks.get(m))
                    .copied()
                    .unwrap_or(false);
                if marked {
                    targets.push((g, m, member.path.clone()));
                }
            }
        }
        if targets.is_empty() {
            return;
        }
        self.trashing = true;
        self.trash_done = 0;
        self.trash_total = targets.len();
        self.result = None;
        self.error = None;
        cx.notify();
        let library = self.library.clone();
        cx.spawn_in(window, async move |this, cx| {
            let mut removed: HashSet<(usize, usize)> = HashSet::new();
            let mut trashed: Vec<PathBuf> = Vec::new();
            let mut failures = 0usize;
            let mut first_error: Option<String> = None;
            for (g, m, path) in targets {
                let (path, result) = cx
                    .background_executor()
                    .spawn(async move {
                        let r = trash::delete(&path);
                        (path, r)
                    })
                    .await;
                match result {
                    Ok(()) => {
                        removed.insert((g, m));
                        trashed.push(path);
                    }
                    Err(e) => {
                        failures += 1;
                        if first_error.is_none() {
                            let name = path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| path.display().to_string());
                            first_error = Some(
                                rox_i18n::t!(
                                    "duplicates-trash-error",
                                    name = name,
                                    error = e.to_string()
                                )
                                .to_string(),
                            );
                        }
                    }
                }
                if this
                    .update(cx, |this, cx| {
                        this.trash_done += 1;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
            let n = trashed.len();
            if !trashed.is_empty() {
                library
                    .update(cx, |library, cx| library.remove_files(trashed, cx))
                    .ok();
            }
            this.update(cx, |this, cx| {
                let groups = std::mem::take(&mut this.groups);
                this.groups = groups
                    .into_iter()
                    .enumerate()
                    .filter_map(|(g, group)| {
                        let members: Vec<DupMember> = group
                            .members
                            .into_iter()
                            .enumerate()
                            .filter(|(m, _)| !removed.contains(&(g, *m)))
                            .map(|(_, member)| member)
                            .collect();
                        (members.len() >= 2).then_some(DupGroup {
                            title: group.title,
                            artist: group.artist,
                            duration_ms: group.duration_ms,
                            same_album: group.same_album,
                            members,
                        })
                    })
                    .collect();
                this.auto_select();
                this.rebuild_rows();
                this.trashing = false;
                this.result = Some(if failures > 0 {
                    rox_i18n::t!(
                        "duplicates-trash-result-failed",
                        count = n as u64,
                        failed = failures as u64
                    )
                } else {
                    rox_i18n::t!("duplicates-trash-result", count = n as u64)
                });
                this.error = first_error.map(Into::into);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn scan_controls(&self, cx: &mut Context<Self>) -> Div {
        let busy = self.scanning || self.trashing;
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .when(self.scanning, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(tokens::SPACE_XS)
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(Spinner::new().with_size(Size::Small))
                        .child(rox_i18n::t!("duplicates-scanning")),
                )
            })
            .child(small_button(
                if self.scanned { "Rescan" } else { "Scan" },
                icons::SEARCH,
                busy,
                cx.listener(|this, _, window, cx| this.scan(window, cx)),
            ))
    }

    fn footer(&self, cx: &mut Context<Self>) -> Div {
        let busy = self.scanning || self.trashing;
        let count = self.visible_checked_count();
        // Worst news first: a failure, then a run in flight, then the count.
        let warn: Option<SharedString> = if let Some(error) = self.error.clone() {
            Some(error)
        } else if self.trashing {
            let at = (self.trash_done + 1).min(self.trash_total);
            Some(rox_i18n::t!(
                "duplicates-trashing",
                done = at as u64,
                total = self.trash_total as u64
            ))
        } else if count == 0 && !self.groups.is_empty() {
            Some(rox_i18n::t!("duplicates-check-to-trash"))
        } else {
            None
        };
        let result = self.result.clone().filter(|_| !self.trashing);
        let left = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .text_xs()
            .when(count > 0, |d| {
                d.child(div().text_color(palette::text_muted()).child(rox_i18n::t!(
                    "duplicates-selected-count",
                    count = count as u64
                )))
            })
            .when_some(result, |d, result| {
                d.child(div().text_color(palette::text_muted()).child(result))
            })
            .when_some(warn, |d, warn| {
                d.child(div().text_color(palette::tone_warn()).child(warn))
            });
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
            .child(left)
            .child(small_button(
                rox_i18n::t!("duplicates-trash-button", count = count as u64),
                icons::TRASH,
                busy || count == 0,
                cx.listener(|this, _, window, cx| this.trash(window, cx)),
            ))
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> Div {
        let policy = self.policy;
        let weak = cx.entity().downgrade();
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .child(div().flex_1().child(Input::new(&self.query_input).small()))
            .child(
                Button::new("dup-policy")
                    .label(policy.label())
                    .small()
                    .outline()
                    .dropdown_menu(move |mut menu, _, _| {
                        for pick in [KeepPolicy::Quality, KeepPolicy::Oldest, KeepPolicy::Newest] {
                            let this = weak.clone();
                            menu = menu.item(
                                PopupMenuItem::new(pick.label())
                                    .checked(policy == pick)
                                    .on_click(move |_, _, cx| {
                                        if let Some(this) = this.upgrade() {
                                            this.update(cx, |this, cx| this.set_policy(pick, cx));
                                        }
                                    }),
                            );
                        }
                        menu
                    }),
            )
    }

    fn results(&self, cx: &mut Context<Self>) -> Div {
        let region = div().flex_1().min_h_0().flex().flex_col();
        if self.scanning {
            return region;
        }
        if self.groups.is_empty() {
            let message = if !self.scanned {
                rox_i18n::t!("duplicates-scan-hint")
            } else {
                rox_i18n::t!("duplicates-no-duplicates")
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
        let count = self.checked_count();
        let groups = self.groups.len();
        let extras: usize = self.groups.iter().map(|g| g.members.len() - 1).sum();
        let this = cx.entity().downgrade();
        region
            .gap(tokens::SPACE_SM)
            .child(block_header(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!(
                        "duplicates-groups-summary",
                        groups = groups as u64,
                        extras = extras as u64
                    )),
                small_button(
                    if count > 0 {
                        rox_i18n::t!("duplicates-select-none")
                    } else {
                        rox_i18n::t!("duplicates-auto-select")
                    },
                    icons::CHECK,
                    self.trashing,
                    cx.listener(move |this, _, _, cx| {
                        if this.checked_count() > 0 {
                            this.select_none();
                        } else {
                            this.auto_select();
                        }
                        cx.notify();
                    }),
                ),
            ))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .map(|d| {
                        if self.rows.is_empty() {
                            d.child(
                                div()
                                    .size_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_color(palette::text_muted())
                                    .child(rox_i18n::t!("duplicates-no-filter-matches")),
                            )
                        } else {
                            d.child(
                                uniform_list(
                                    "duplicate-rows",
                                    self.rows.len(),
                                    move |range, _, cx| {
                                        this.upgrade()
                                            .map(|this| {
                                                this.update(cx, |this, cx| {
                                                    this.list_rows(range, cx)
                                                })
                                            })
                                            .unwrap_or_default()
                                    },
                                )
                                .track_scroll(self.scroll.clone())
                                .size_full(),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .child(Scrollbar::vertical(&self.scroll)),
                            )
                        }
                    })
                    // Lock the list while a delete runs so no mark changes under it.
                    .when(self.trashing, |d| {
                        d.child(div().absolute().inset_0().occlude())
                    }),
            )
    }

    fn list_rows(
        &self,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        range
            .filter_map(|i| {
                let kind = *self.rows.get(i)?;
                Some(match kind {
                    RowKind::Header(g) => self.header_row(i, self.groups.get(g)?),
                    RowKind::Member(g, m) => {
                        let group = self.groups.get(g)?;
                        self.member_row(i, g, m, group.members.get(m)?, cx)
                    }
                })
            })
            .collect()
    }

    fn header_row(&self, i: usize, group: &DupGroup) -> Stateful<Div> {
        let n = group.members.len();
        div()
            .id(("dup-row", i))
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .h(palette::scaled_px(ROW_H))
            .px(tokens::SPACE_XS)
            .when(i > 0, |d| d.border_t_1().border_color(palette::border()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().truncate().child(group.title.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette::text_muted())
                            .truncate()
                            .child(if group.artist.is_empty() {
                                fmt_ms(group.duration_ms)
                            } else {
                                format!("{} - {}", group.artist, fmt_ms(group.duration_ms))
                            }),
                    ),
            )
            .when(!group.same_album, |d| {
                d.child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(rox_i18n::t!("duplicates-different-albums")),
                )
            })
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!("duplicates-copy-count", count = n as u64)),
            )
    }

    fn member_row(
        &self,
        i: usize,
        g: usize,
        m: usize,
        member: &DupMember,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let checked = self
            .checked
            .get(g)
            .and_then(|marks| marks.get(m))
            .copied()
            .unwrap_or(false);
        let thumb = self
            .thumbs
            .update(cx, |thumbs, cx| thumbs.get(&member.path, cx));
        let quality = {
            let codec = member.codec.to_uppercase();
            if member.bitrate_kbps > 0 {
                format!(
                    "{codec} {}",
                    rox_i18n::format::format_unit(member.bitrate_kbps as f64, 0, "kbps")
                )
            } else {
                codec
            }
        };
        div()
            .id(("dup-row", i))
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .h(palette::scaled_px(ROW_H))
            .pl(px(24.))
            .pr(tokens::SPACE_XS)
            .rounded(tokens::RADIUS)
            .cursor_pointer()
            .hover(|d| d.bg(palette::bg_control_hover()))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(g, m, cx)))
            .child(checkbox(checked))
            .child(cover_tile(thumb))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().truncate().child(member.name.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette::text_muted())
                            .truncate()
                            .child(member.folder.clone()),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(110.))
                    .flex()
                    .justify_end()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(quality),
            )
    }
}

/// Title, artist, or any copy's file name or folder, so a path fragment narrows.
fn group_matches(group: &DupGroup, query: &str) -> bool {
    group.title.to_lowercase().contains(query)
        || group.artist.to_lowercase().contains(query)
        || group.members.iter().any(|m| {
            m.name.to_lowercase().contains(query) || m.folder.to_lowercase().contains(query)
        })
}

fn cover_tile(thumb: Thumb) -> Div {
    let side = px(COVER);
    div().flex_none().flex().items_center().child(match thumb {
        Thumb::Ready(image) => div()
            .size(side)
            .overflow_hidden()
            .child(
                img(image)
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .rounded(px(3.)),
            )
            .into_any_element(),
        _ => div()
            .size(side)
            .rounded(px(3.))
            .bg(palette::bg_control())
            .flex()
            .items_center()
            .justify_center()
            .child(
                svg()
                    .path(icons::MUSIC)
                    .size(px(14.))
                    .text_color(palette::text_faint()),
            )
            .into_any_element(),
    })
}

impl Render for Duplicates {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .child(self.toolbar(cx))
            .child(self.results(cx));
        let page = div()
            .id("duplicates-page")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .p(tokens::SPACE_MD)
            // Stops at the page so the footer composes against one layer, not two.
            .bg(palette::bg_elevated())
            .child(
                section(
                    "Duplicates",
                    Some(self.scan_controls(cx).into_any_element()),
                    body,
                )
                .flex_1()
                .min_h_0(),
            );

        div()
            .size_full()
            .flex()
            .flex_row()
            .bg(palette::bg_elevated())
            .text_color(palette::text_bright())
            .text_sm()
            .children(self.backdrop.layer(&self.now_art, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(page)
                    .child(self.footer(cx)),
            )
    }
}
