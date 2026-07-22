//! The duplicates window: find tracks the library carries more than once
//! and move the spare copies to the OS trash. A duplicate here is a tag
//! identity, the same title and artist within a small duration tolerance,
//! matched over the in-memory projection, so a scan never walks the disk;
//! the same album ripped twice or copied into two folders shows up whatever
//! the files are named. Groups list every copy with its cover, codec, and
//! bitrate so the user can see which version is which before deciding, and
//! a filter box narrows a long result to one artist or folder.
//!
//! The keep policy picks each group's default keeper - best quality,
//! oldest, or newest copy - and checks the rest. A group whose copies
//! belong to different albums is never auto-checked: those are one song on
//! several releases, and trashing a copy would leave a hole in an album,
//! so touching them stays a hand decision. A group can never have every
//! member checked - checking the last unchecked copy swaps the mark onto
//! the old keeper instead - so the tool cannot take a track's last copy.
//! Trashing goes through the `trash` crate, never a plain unlink, and the
//! catalog rows drop through the library's prune so the panels converge
//! without a rescan.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use gpui::{
    div, img, prelude::*, px, size, svg, App, Bounds, Context, Div, Entity, Global, ObjectFit,
    SharedString, Stateful, Subscription, UniformListScrollHandle, Window, WindowHandle, uniform_list,
};
use gpui_component::button::Button;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_component::scroll::Scrollbar;
use gpui_component::spinner::Spinner;
use gpui_component::{Root, Sizable, Size};

use crate::assets::icons;
use crate::backdrop::{NowPlayingArt, WindowBackdrop};
use crate::design::{palette, tokens};
use crate::panels::library::{fmt_ms, Library};
use crate::settings_ui::{checkbox, small_button, MIN_SIZE};
use crate::thumbs::{Thumb, Thumbs};

/// One row's height. The list is a uniform_list, so headers and members
/// agree; two lines and a cover fit either way.
const ROW_H: f32 = 42.;

/// A member row's cover tile, sized to sit inside the row with room to
/// breathe.
const COVER: f32 = 32.;

/// How far apart two durations can sit and still read as the same
/// recording. Rips and transcodes of one track drift by padding and
/// encoder delay, not by seconds; anything past this is a different take.
const DUR_TOLERANCE_MS: u32 = 1500;

/// Which copy of a group the auto-selection keeps; everything else in the
/// group gets checked for the trash.
#[derive(Clone, Copy, PartialEq, Eq)]
enum KeepPolicy {
    /// The highest bitrate, the earliest added on a tie.
    Quality,
    /// The earliest added, the highest bitrate on a tie.
    Oldest,
    /// The latest added, the highest bitrate on a tie.
    Newest,
}

impl KeepPolicy {
    fn label(self) -> &'static str {
        match self {
            KeepPolicy::Quality => "Keep best quality",
            KeepPolicy::Oldest => "Keep oldest",
            KeepPolicy::Newest => "Keep newest",
        }
    }
}

/// One copy of a duplicated track: the row identity for the delete and
/// the fields that tell the copies apart in the list.
struct DupMember {
    path: PathBuf,
    name: SharedString,
    /// The full parent directory, not just its name: duplicates often sit
    /// in folders named alike, and the path is what tells them apart.
    folder: SharedString,
    codec: SharedString,
    bitrate_kbps: u16,
    /// When the library first saw this copy, the newest/oldest policies'
    /// key.
    added: i64,
}

/// One duplicated track: the shared identity on the header and every copy
/// under it, the keeper first per the active policy.
struct DupGroup {
    title: SharedString,
    artist: SharedString,
    duration_ms: u32,
    /// Whether every copy carries the same album tag. Copies spread over
    /// different albums are one song on several releases; auto-selection
    /// leaves those alone so no album loses a track by default.
    same_album: bool,
    members: Vec<DupMember>,
}

/// What one flattened list row shows: a group's header or one member,
/// each addressed into `groups` by index.
#[derive(Clone, Copy)]
enum RowKind {
    Header(usize),
    Member(usize, usize),
}

/// What the background match hands back per member, before the UI thread
/// resolves ids to paths: everything else comes straight off the
/// projection.
struct MemberSpec {
    id: i64,
    codec: String,
    bitrate_kbps: u16,
    added: i64,
}

/// One matched group off the background pass, paths still unresolved.
struct GroupSpec {
    title: String,
    artist: String,
    duration_ms: u32,
    same_album: bool,
    members: Vec<MemberSpec>,
}

/// The open duplicates window, if any. One at a time for the same reason
/// as tag repair: a scan or delete in flight is not worth losing to a
/// second copy, so asking again brings this one forward.
#[derive(Default)]
struct OpenDuplicates(Option<WindowHandle<Root>>);

impl Global for OpenDuplicates {}

/// Open the duplicates window, or bring the open one forward. Takes the
/// shared catalog it matches over and prunes into, the thumbnail service
/// for the member covers, and the art bake it backs with, so the settings
/// window can open it from what it already holds.
pub fn open(
    library: Entity<Library>,
    thumbs: Entity<Thumbs>,
    now_art: Entity<NowPlayingArt>,
    cx: &mut App,
) {
    if let Some(handle) = cx.try_global::<OpenDuplicates>().and_then(|o| o.0) {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    let bounds = Bounds::centered(None, size(px(760.), px(600.)), cx);
    let handle = crate::panel::open_child_window(cx, "rox - Duplicates", bounds, Some(MIN_SIZE), move |window, cx| {
        cx.new(|cx| Duplicates::new(library, thumbs, now_art, window, cx))
    });
    cx.set_global(OpenDuplicates(Some(handle)));
}

pub struct Duplicates {
    library: Entity<Library>,
    thumbs: Entity<Thumbs>,
    /// A scan is matching over the projection; the controls lock while it
    /// runs.
    scanning: bool,
    /// Whether a scan has finished at least once, so the list can say
    /// "none found" rather than an empty page before the first scan.
    scanned: bool,
    /// The duplicate groups the scan found, keeper first per the policy.
    groups: Vec<DupGroup>,
    /// Per group, per member: whether that copy is marked for the trash.
    checked: Vec<Vec<bool>>,
    /// Which copy the auto-selection keeps.
    policy: KeepPolicy,
    /// The filter box and its current text, kept lowercased for the
    /// matching.
    query_input: Entity<InputState>,
    query: String,
    /// The flattened list the uniform_list renders: one header row per
    /// group the filter matches, one row per member. Rebuilt whenever
    /// `groups` or the filter changes.
    rows: Vec<RowKind>,
    /// A delete is moving files to the trash; the list locks under an
    /// occluder and the count moves per file.
    trashing: bool,
    trash_done: usize,
    trash_total: usize,
    /// The last delete's summary, held over the list after it lands.
    result: Option<SharedString>,
    /// A scan or delete failure, shown inline.
    error: Option<SharedString>,
    scroll: UniformListScrollHandle,
    now_art: Entity<NowPlayingArt>,
    backdrop: WindowBackdrop,
    /// This window pumps its own frames, so the backdrop needs its own
    /// wake on a new bake.
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
        let query_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter by title, artist, or folder"));
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

    /// Rebuild the flattened row list from the groups the filter matches.
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

    /// Match the projection for duplicate identities. The grouping runs
    /// off the UI thread over the shared projection; the id-to-path
    /// resolution lands back on it, where the library connection lives.
    fn scan(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.scanning || self.trashing {
            return;
        }
        let Some(projection) = self.library.read(cx).projection().cloned() else {
            self.error = Some("The library is still loading; try again shortly.".into());
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
                // Resolve each member's id to its path on the library
                // connection. A member whose row vanished mid-scan drops
                // out; a group thinned under two copies is no longer a
                // duplicate and drops with it.
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

    /// Order every group's members keeper-first per the active policy and
    /// reapply the default selection.
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

    /// Change the keep policy: reorder the groups and reset the marks to
    /// its defaults. Held while a delete runs so the commits' targets
    /// cannot shift under them.
    fn set_policy(&mut self, policy: KeepPolicy, cx: &mut Context<Self>) {
        if self.trashing || policy == self.policy {
            return;
        }
        self.policy = policy;
        self.apply_policy();
        self.rebuild_rows();
        cx.notify();
    }

    /// Apply the keep policy's default marks: the first member (the keeper
    /// per the active ordering) stays, the rest are checked - except in a
    /// group whose copies span different albums, which stays untouched so
    /// no album loses a track without a hand pick.
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

    /// Clear every mark.
    fn select_none(&mut self) {
        for marks in &mut self.checked {
            marks.iter_mut().for_each(|c| *c = false);
        }
    }

    /// Flip one member's mark. Checking what would be a group's last
    /// unchecked copy swaps instead: this copy joins the trash picks and
    /// the best of the others becomes the keeper, so a group always keeps
    /// one and picking a different keeper is one click, not two.
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

    /// How many copies are marked for the trash.
    fn checked_count(&self) -> usize {
        self.checked
            .iter()
            .map(|marks| marks.iter().filter(|&&c| c).count())
            .sum()
    }

    /// Move every marked copy to the OS trash, one file per background hop
    /// so the count moves and a slow disk is visibly the holdup. Trashed
    /// files prune out of the catalog through the library; their rows drop
    /// off the list, a group left with one copy dissolves, and failures
    /// stay put so the user sees which.
    fn trash(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.trashing || self.scanning {
            return;
        }
        let mut targets: Vec<(usize, usize, PathBuf)> = Vec::new();
        for (g, group) in self.groups.iter().enumerate() {
            // Only trash what the user can see. A group the filter hides is
            // off the list, so its marks (from a prior auto-select) must not
            // slip through here and delete copies out of view.
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
                            first_error = Some(format!("{name}: {e}"));
                        }
                    }
                }
                // A closed window (the user gave up) drops the handle; the
                // files already trashed still need their prune, so fall
                // through to it rather than return.
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
                // Drop the trashed members; a group down to one copy is no
                // longer a duplicate and leaves the list with them.
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
                    format!("Moved {n} to trash, {failures} failed").into()
                } else if n == 1 {
                    "Moved 1 file to trash".into()
                } else {
                    format!("Moved {n} files to trash").into()
                });
                this.error = first_error.map(Into::into);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The section header: the "Duplicates" label with the scan and trash
    /// controls trailing it, on the border the settings sections wear.
    fn header(&self, cx: &mut Context<Self>) -> Div {
        let busy = self.scanning || self.trashing;
        let count = self.checked_count();
        let controls = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .when(busy, |d| {
                let label = if self.scanning {
                    "Scanning...".to_string()
                } else {
                    let at = (self.trash_done + 1).min(self.trash_total);
                    format!("Trashing {}/{}...", at, self.trash_total)
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
                    format!("Trash ({count})")
                } else {
                    "Trash".to_string()
                },
                icons::TRASH,
                busy || count == 0,
                cx.listener(|this, _, window, cx| this.trash(window, cx)),
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
                    .child("Duplicates"),
            )
            .child(controls)
    }

    /// The toolbar under the header: the filter box beside the keep-policy
    /// dropdown.
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

    /// The results region under the toolbar, filling the rest of the
    /// window: a centered hint before the first scan, a "none found" line
    /// when a scan came up clean, or the count-and-select header over the
    /// virtualized group list.
    fn results(&self, cx: &mut Context<Self>) -> Div {
        let region = div().flex_1().min_h_0().flex().flex_col();
        // Mid-scan the header's spinner already says what is happening;
        // the hint would just contradict it.
        if self.scanning {
            return region;
        }
        if self.groups.is_empty() {
            let message = if !self.scanned {
                "Scan the library for tracks that appear more than once."
            } else {
                "No duplicates found."
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
                    .child(if groups == 1 {
                        format!("1 group, {extras} extra copies")
                    } else {
                        format!("{groups} groups, {extras} extra copies")
                    })
                    .child(small_button(
                        if count > 0 { "Select none" } else { "Auto-select" },
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
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .map(|d| {
                        if self.rows.is_empty() {
                            // Every group filtered out; say so rather than
                            // show a blank pane under a live count.
                            d.child(
                                div()
                                    .size_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_color(palette::text_muted())
                                    .child("No groups match the filter."),
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
                    // The list locks while a delete runs: a transparent
                    // occluder over it swallows clicks so nothing checks or
                    // unchecks out from under the trash hops.
                    .when(self.trashing, |d| {
                        d.child(div().absolute().inset_0().occlude())
                    }),
            )
    }

    /// The visible slice of list rows: group headers carrying the shared
    /// identity, member rows each a click target around their checkbox.
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

    /// One group's header row: the title over the artist and duration, a
    /// note when the copies span albums, and the copy count trailing. All
    /// but the first header wear a top border so groups read apart.
    fn header_row(&self, i: usize, group: &DupGroup) -> Stateful<Div> {
        let n = group.members.len();
        div()
            .id(("dup-row", i))
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .h(px(ROW_H))
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
                        .child("different albums"),
                )
            })
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(if n == 2 {
                        "2 copies".to_string()
                    } else {
                        format!("{n} copies")
                    }),
            )
    }

    /// One copy's row: checkbox, the file's cover, its name over its
    /// folder, and the codec and bitrate trailing right-aligned so the
    /// versions line up. The whole row is the click target so the box is
    /// easy to hit.
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
                format!("{} {} kbps", codec, member.bitrate_kbps)
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
            .h(px(ROW_H))
            // Indented under the group header so the copies read as its.
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

/// Whether a group matches the lowercased filter text: on its title or
/// artist, or any copy's file name or folder, so a path fragment narrows
/// to the release it names.
fn group_matches(group: &DupGroup, query: &str) -> bool {
    group.title.to_lowercase().contains(query)
        || group.artist.to_lowercase().contains(query)
        || group.members.iter().any(|m| {
            m.name.to_lowercase().contains(query) || m.folder.to_lowercase().contains(query)
        })
}

/// A member's cover tile: the thumbnail once it is ready, a placeholder
/// note glyph while it loads or when the file has none. The thumbnail
/// service's tiles are square, so covers line the rows up.
fn cover_tile(thumb: Thumb) -> Div {
    let side = px(COVER);
    div().flex_none().flex().items_center().child(match thumb {
        Thumb::Ready(image) => img(image)
            .size(side)
            .object_fit(ObjectFit::Cover)
            .rounded(px(3.))
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

/// Group the projection's rows into duplicate identities: the same artist
/// and case-folded title, clustered within the duration tolerance. Each
/// cluster of two or more becomes a group; the caller orders members per
/// its keep policy. Blocking; run it off the UI thread.
fn match_duplicates(projection: &rox_library::projection::Projection) -> Vec<GroupSpec> {
    // Bucket by identity first; the map borrows the projection's strings,
    // nothing is owned until a bucket proves duplicated. Key on the folded
    // artist, not the case-sensitive symbol, so "ABBA" and "Abba" land in one
    // bucket like the folded title does; distinct symbols share a lower form.
    let mut by_key: HashMap<(&str, &str), Vec<usize>> = HashMap::new();
    for i in 0..projection.db_id.len() {
        let artist_lower = projection.artists.lower[projection.artist[i] as usize].as_str();
        by_key
            .entry((artist_lower, projection.title_lower.get(i)))
            .or_default()
            .push(i);
    }
    let mut out: Vec<GroupSpec> = Vec::new();
    for ((_, _), mut rows) in by_key {
        if rows.len() < 2 {
            continue;
        }
        // Cluster by duration inside the bucket: sorted, a row joins the
        // open cluster while it sits within tolerance of the cluster's
        // start, so drift never chains far past it.
        rows.sort_by_key(|&i| projection.duration_ms[i]);
        let mut start = 0;
        for end in 1..=rows.len() {
            let split = end == rows.len()
                || projection.duration_ms[rows[end]] - projection.duration_ms[rows[start]]
                    > DUR_TOLERANCE_MS;
            if !split {
                continue;
            }
            if end - start >= 2 {
                let cluster = &rows[start..end];
                let lead = cluster[0];
                let same_album = cluster
                    .iter()
                    .all(|&i| projection.album[i] == projection.album[lead]);
                out.push(GroupSpec {
                    title: projection.title.get(lead).to_owned(),
                    artist: projection.artists.strings[projection.artist[lead] as usize].clone(),
                    duration_ms: projection.duration_ms[lead],
                    same_album,
                    members: cluster
                        .iter()
                        .map(|&i| MemberSpec {
                            id: projection.db_id[i],
                            codec: projection.codecs.strings[projection.codec[i] as usize].clone(),
                            bitrate_kbps: projection.bitrate_kbps[i],
                            added: projection.added[i],
                        })
                        .collect(),
                });
            }
            start = end;
        }
    }
    // The map's order is arbitrary; artist then title keeps the list
    // stable across rescans.
    out.sort_by(|a, b| {
        (a.artist.to_lowercase(), &a.title)
            .cmp(&(b.artist.to_lowercase(), &b.title))
    });
    out
}

impl Render for Duplicates {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The header, toolbar, and summary stay fixed; only the group list
        // scrolls, and it virtualizes, so a library-wide result stays
        // responsive however many copies it turns up.
        let page = div()
            .id("duplicates-page")
            .size_full()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .p(tokens::SPACE_MD)
            .child(self.header(cx))
            .child(self.toolbar(cx))
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
