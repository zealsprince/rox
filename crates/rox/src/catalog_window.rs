//! The catalog window (ADR 36): the index in tabs by kind, searched
//! locally, with a package's card beside the list and an Install button
//! that hands it to [`crate::catalog::install`]. It also takes the
//! `rox://install/<id>` link, which only ever opens this window on a package.
//!
//! Everything shown here is a stranger's text and a stranger's pictures.
//! Text is plain, stripped of control and bidi characters and cut to a
//! length, so one entry can't forge a line or swamp the layout. A preview
//! comes only through the client's capped fetch and is decoded under the
//! canvas's pixel cap. A homepage opens only as an `https://` link to a
//! plain host, named beside it. Every fetch and decode runs on the
//! background executor.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyElement, AnyWindowHandle, App, Bounds, Context, Div, Entity, EntityId, FocusHandle,
    Focusable as _, Global, KeyDownEvent, MouseButton, ObjectFit, Pixels, RenderImage,
    SharedString, Subscription, WeakEntity, Window, div, img, prelude::*, px, relative, size, svg,
};
use gpui_component::button::Button;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::spinner::Spinner;
use gpui_component::{Disableable as _, Icon, Sizable as _};

use rox_core::fmt::fmt_bytes;
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_net::sources::catalog::{Index, Kind, Record, Summary, is_slug};
use rox_panel_api::panel;
use rox_panel_api::toast::Toast;
use rox_services::plugins as host;

use crate::catalog;
use crate::startup::updates::CURRENT;
use rox_panel_kit::PlaceholderKey as _;

const DEFAULT_SIZE: (f32, f32) = (920., 640.);

const MIN_SIZE: gpui::Size<Pixels> = gpui::Size {
    width: px(640.),
    height: px(420.),
};

/// Past this a list stops being browsed and starts being searched.
const RESULT_LIMIT: usize = 200;

/// Previews fetched at once, so a long list doesn't open dozens of
/// requests and decodes together.
const PREVIEW_JOBS: usize = 4;

/// A record's previews shown on its card.
const RECORD_PREVIEWS: usize = 4;

const POLL: Duration = Duration::from_millis(200);

// Caps on catalog text, in characters. Placeholders.
const NAME_MAX: usize = 80;
const AUTHOR_MAX: usize = 60;
const SUMMARY_MAX: usize = 200;
const TAG_MAX: usize = 32;
const TAGS_MAX: usize = 8;
const LICENSE_MAX: usize = 60;
const VERSION_MAX: usize = 40;
const DATE_MAX: usize = 32;
const DESCRIPTION_MAX: usize = 4000;
const DESCRIPTION_LINES: usize = 60;
const NOTES_MAX: usize = 1000;
const NOTES_LINES: usize = 20;
const HOMEPAGE_MAX: usize = 300;
const REASON_MAX: usize = 200;

// ── Opening ──

struct OpenCatalog {
    window: AnyWindowHandle,
    view: WeakEntity<CatalogWindow>,
}

impl Global for OpenCatalog {}

pub(crate) fn open(cx: &mut App) {
    open_on(None, false, cx);
}

/// The updates chip: open on the Updates tab once the listing shows one.
pub(crate) fn open_updates(cx: &mut App) {
    open_on(None, true, cx);
}

/// A refresh or an install this window may not have started landed, so
/// the listing reloads from the saved index and the record.
pub(crate) fn refreshed(cx: &mut App) {
    let Some(open) = cx.try_global::<OpenCatalog>() else {
        return;
    };
    let (handle, view) = (open.window, open.view.clone());

    let _ = handle.update(cx, |_, window, cx| {
        if let Some(view) = view.upgrade() {
            view.update(cx, |this, cx| this.reload(window, cx));
        }
    });
}

/// The link handler. Any web page can make one of these, so a link only
/// ever opens this window and selects a package rox's own index names. It
/// starts no download, installs nothing and switches nothing on.
pub(crate) fn open_link(link: &str, cx: &mut App) {
    let Some(id) = install_id(link) else {
        log::info!(
            "catalog: dropped a link that isn't rox://install/<id>: {:?}",
            link.chars().take(80).collect::<String>()
        );
        return;
    };

    open_on(Some(id.to_string()), false, cx);
}

/// `rox://install/<id>`, with one optional trailing slash, and nothing else.
/// The scheme folds case, as the OS may hand it over; the rest is exact, so
/// a query, a fragment, another path or another action is no match. The id
/// is the client's own slug rule, which is `[a-z0-9-]{1,64}` with no
/// leading hyphen.
fn install_id(link: &str) -> Option<&str> {
    let scheme = link.get(..6)?;
    if !scheme.eq_ignore_ascii_case("rox://") {
        return None;
    }

    let rest = link[6..].strip_prefix("install/")?;
    let id = rest.strip_suffix('/').unwrap_or(rest);
    is_slug(id).then_some(id)
}

/// Deferred: the opening action runs inside another window's update, and
/// reading the front workspace for the tint mid-update would panic.
fn open_on(select: Option<String>, updates: bool, cx: &mut App) {
    cx.defer(move |cx| open_now(select, updates, cx));
}

fn open_now(select: Option<String>, updates: bool, cx: &mut App) {
    if let Some(open) = cx.try_global::<OpenCatalog>() {
        let (handle, view) = (open.window, open.view.clone());
        let link = select.clone();

        let raised = handle.update(cx, |_, window, cx| {
            window.activate_window();

            if let (Some(id), Some(view)) = (link, view.upgrade()) {
                view.update(cx, |this, cx| this.follow_link(id, window, cx));
            }
            if updates && let Some(view) = view.upgrade() {
                view.update(cx, |this, cx| this.want_updates(cx));
            }
        });
        if raised.is_ok() {
            return;
        }
    }

    let player =
        rox_panel_api::windows::front_workspace(cx).map(|(_, state)| state.player.entity_id());
    let bounds = Bounds::centered(None, size(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1)), cx);

    panel::open_child_window(
        cx,
        || rox_i18n::t!("catalog-window-title"),
        bounds,
        Some(MIN_SIZE),
        move |window, cx| cx.new(|cx| CatalogWindow::new(player, select, updates, window, cx)),
    );
}

// ── What the window shows ──

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Workspaces,
    Plugins,
    Objects,
    Updates,
}

impl Tab {
    fn of(kind: Kind) -> Option<Tab> {
        match kind {
            Kind::Workspace => Some(Tab::Workspaces),
            Kind::Plugin => Some(Tab::Plugins),
            Kind::Object => Some(Tab::Objects),
            Kind::Unknown => None,
        }
    }

    fn label(self) -> SharedString {
        match self {
            Tab::Workspaces => rox_i18n::t!("catalog-tab-workspaces"),
            Tab::Plugins => rox_i18n::t!("catalog-tab-plugins"),
            Tab::Objects => rox_i18n::t!("catalog-tab-objects"),
            Tab::Updates => rox_i18n::t!("catalog-tab-updates"),
        }
    }
}

/// Updates only shows while there is one.
fn tabs(updates: usize) -> Vec<Tab> {
    let mut tabs = vec![Tab::Workspaces, Tab::Plugins, Tab::Objects];
    if updates > 0 {
        tabs.push(Tab::Updates);
    }

    tabs
}

/// One package as the window shows it, every field made plain once, off
/// the main thread.
#[derive(Clone, Debug)]
struct Entry {
    id: String,
    kind: Kind,
    name: String,
    author: String,
    summary: String,
    tags: Vec<String>,
    license: String,
    version: String,
    bytes: u64,
    sha256: String,
    requires: Vec<String>,
    preview: Option<String>,
    /// Name, author, summary and tags, lowercased, for the search.
    haystack: String,
}

fn entry(summary: &Summary) -> Entry {
    let name = match line(&summary.name, NAME_MAX) {
        name if name.is_empty() => summary.id.clone(),
        name => name,
    };
    let author = line(&summary.author, AUTHOR_MAX);
    let text = line(&summary.summary, SUMMARY_MAX);
    let tags: Vec<String> = summary
        .tags
        .iter()
        .map(|tag| line(tag, TAG_MAX))
        .filter(|tag| !tag.is_empty())
        .take(TAGS_MAX)
        .collect();

    let haystack = [&name, &author, &text]
        .into_iter()
        .chain(&tags)
        .map(|field| field.to_lowercase())
        .collect::<Vec<_>>()
        .join("\n");

    Entry {
        id: summary.id.clone(),
        kind: summary.kind,
        name,
        author,
        summary: text,
        tags,
        license: line(&summary.license, LICENSE_MAX),
        version: line(&summary.latest.version, VERSION_MAX),
        bytes: summary.latest.bytes,
        sha256: summary.latest.sha256.clone(),
        requires: summary.latest.requires.clone(),
        preview: summary.preview.clone(),
        haystack,
    }
}

/// Every word of the query has to turn up in one of the fields.
fn matches(entry: &Entry, query: &str) -> bool {
    query
        .split_whitespace()
        .all(|word| entry.haystack.contains(&word.to_lowercase()))
}

fn in_tab(entry: &Entry, tab: Tab, updates: &HashSet<String>) -> bool {
    match tab {
        Tab::Updates => updates.contains(&entry.id),
        tab => Tab::of(entry.kind) == Some(tab),
    }
}

#[derive(Default)]
struct Listing {
    entries: Vec<Entry>,
    /// Left out because this build is too old for them, with their names,
    /// so a link to one can say why it shows nothing.
    too_new: HashMap<String, String>,
    /// On record with its file still there. A record whose file was deleted
    /// shows Update when the index moved, and Install when it didn't.
    installed: HashSet<String>,
    updates: HashSet<String>,
    /// Updates whose Update asks Replace or Keep Both.
    asking: HashSet<String>,
}

/// Reads the record of installs, so it's for the background executor.
fn listing(index: Index) -> Listing {
    let mut listing = Listing::default();

    for summary in &index.packages {
        let entry = entry(summary);
        if catalog::runs_here(&summary.latest.min_rox, CURRENT).is_err() {
            listing.too_new.insert(entry.id, entry.name);
            continue;
        }

        listing.entries.push(entry);
    }

    listing.installed = catalog::installed()
        .into_iter()
        .filter(catalog::on_disk)
        .map(|installed| installed.id)
        .collect();
    let updates = catalog::updates();
    listing.asking = catalog::asking(&updates);
    listing.updates = updates.into_iter().map(|summary| summary.id).collect();

    listing
}

/// The part of a record the card shows, made plain.
#[derive(Clone, Debug)]
struct Detail {
    description: String,
    homepage: Option<Homepage>,
    previews: Vec<String>,
    published: String,
    notes: String,
}

/// The version the index points at is the one whose notes show: a record
/// that disagrees with its index gets no notes rather than another
/// version's.
fn detail(record: Record, sha256: &str) -> Detail {
    let latest = record
        .versions
        .iter()
        .find(|version| version.sha256 == sha256);

    Detail {
        description: block(&record.description, DESCRIPTION_MAX, DESCRIPTION_LINES),
        homepage: homepage(&record.homepage),
        previews: record.previews.into_iter().take(RECORD_PREVIEWS).collect(),
        published: latest
            .map(|version| line(&version.published, DATE_MAX))
            .unwrap_or_default(),
        notes: latest
            .map(|version| block(&version.notes, NOTES_MAX, NOTES_LINES))
            .unwrap_or_default(),
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Homepage {
    url: String,
    host: String,
}

/// Only an `https://` link opens, to a plain ASCII host with an optional
/// port. Refused: any other scheme, credentials before the host, an IP
/// literal in brackets, a non-ASCII host, and a space, a control character
/// or anything non-ASCII anywhere in the link.
fn homepage(text: &str) -> Option<Homepage> {
    if text.len() > HOMEPAGE_MAX || !text.bytes().all(|b| b.is_ascii_graphic()) {
        return None;
    }

    let rest = text.strip_prefix("https://")?;
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    let (host, port) = match authority.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };

    let plain_host = !host.is_empty()
        && !host.starts_with(['.', '-'])
        && !host.ends_with(['.', '-'])
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    let plain_port = port.is_none_or(|port| {
        (1..=5).contains(&port.len()) && port.bytes().all(|b| b.is_ascii_digit())
    });
    if !plain_host || !plain_port {
        return None;
    }

    Some(Homepage {
        url: text.to_string(),
        host: host.to_ascii_lowercase(),
    })
}

/// Characters that draw nothing or reorder what's around them, so a name
/// could pass for another one or hide a line.
fn hidden(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061C}'
                | '\u{200B}'
                | '\u{200E}'
                | '\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2060}'..='\u{2069}'
                | '\u{FEFF}'
        )
}

/// One line of catalog text: breaks and runs of space fold to one space,
/// hidden characters go, and it's cut to `max` characters.
fn line(text: &str, max: usize) -> String {
    let visible: String = text
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|&c| !hidden(c))
        .collect();

    cut(
        &visible.split_whitespace().collect::<Vec<_>>().join(" "),
        max,
    )
}

/// A failure as a person reads it, in their language. Only what's open-ended
/// (a parser's complaint, an OS error, a path) stays English, as detail. The
/// log keeps the English `Display`.
fn why(problem: &catalog::Problem) -> SharedString {
    use catalog::Problem;
    use rox_net::download::Failure;

    let detail = |text: &str| line(text, REASON_MAX);
    match problem {
        Problem::Net(failure) => match failure {
            Failure::NoConnection => rox_i18n::t!("catalog-why-no-connection"),
            Failure::TimedOut => rox_i18n::t!("catalog-why-timed-out"),
            Failure::Refused(status) => {
                rox_i18n::t!("catalog-why-refused", status = u64::from(*status))
            }
            Failure::Redirected(_) => rox_i18n::t!("catalog-why-redirected"),
            Failure::OverCap(_) => rox_i18n::t!("catalog-why-too-large"),
            Failure::SizeOffered { .. } | Failure::RanPast(_) | Failure::Short { .. } => {
                rox_i18n::t!("catalog-why-wrong-size")
            }
            Failure::WrongChecksum { .. } => rox_i18n::t!("catalog-why-wrong-checksum"),
            Failure::Cancelled => rox_i18n::t!("catalog-why-cancelled"),
            Failure::Unreadable(text) => {
                rox_i18n::t!("catalog-why-unreadable", detail = detail(text))
            }
            Failure::BadChecksum | Failure::Other(_) => detail(&failure.to_string()).into(),
        },
        Problem::NoIndex => rox_i18n::t!("catalog-why-no-index"),
        Problem::NoSuchPackage(id) => rox_i18n::t!("catalog-why-no-package", id = id.clone()),
        Problem::TooNew { need, .. } => {
            rox_i18n::t!("catalog-why-too-new", version = need.to_string())
        }
        Problem::Busy(_) => rox_i18n::t!("catalog-why-busy"),
        Problem::Package(text) => rox_i18n::t!("catalog-why-not-package", detail = detail(text)),
        Problem::Interrupted(_) => rox_i18n::t!("catalog-why-interrupted"),
        Problem::Other(text) => detail(text).into(),
    }
}

/// Several lines with their breaks kept: each line made plain, runs of
/// blank lines kept to one, capped in lines and in characters.
fn block(text: &str, max: usize, max_lines: usize) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut more = false;

    let text = text.replace("\r\n", "\n");
    for raw in text.split(['\n', '\r', '\u{2028}', '\u{2029}']) {
        let folded = line(raw, max);
        if folded.is_empty() && lines.last().is_none_or(|last| last.is_empty()) {
            continue;
        }

        if lines.len() == max_lines {
            more = true;
            break;
        }
        lines.push(folded);
    }

    while lines.last().is_some_and(|last| last.is_empty()) {
        lines.pop();
    }
    if more {
        lines.push("...".into());
    }

    cut(&lines.join("\n"), max)
}

fn cut(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((at, _)) => format!("{}...", text[..at].trim_end()),
        None => text.to_string(),
    }
}

fn kind_icon(kind: Kind) -> &'static str {
    match kind {
        Kind::Workspace => icons::APP_WINDOW,
        Kind::Plugin => icons::PLUG,
        Kind::Object | Kind::Unknown => icons::LAYERS,
    }
}

fn kind_label(kind: Kind) -> SharedString {
    match kind {
        Kind::Workspace => rox_i18n::t!("catalog-kind-workspace"),
        Kind::Plugin => rox_i18n::t!("catalog-kind-plugin"),
        Kind::Object | Kind::Unknown => rox_i18n::t!("catalog-kind-object"),
    }
}

enum Shot {
    Loading,
    Ready(Arc<RenderImage>),
    /// Missing, not an image, over a cap, or broken: the kind's icon shows.
    Failed,
}

enum Card {
    Loading,
    Ready(Detail),
    Failed(SharedString),
}

enum Outcome {
    Failed(SharedString),
    /// Installed with no workspace window up, so there was no confirm to show.
    Quiet,
    /// An object's update, stored and offered to a canvas like an install.
    ObjectUpdated,
}

// ── The window ──

struct CatalogWindow {
    player: Option<EntityId>,
    find: Entity<InputState>,
    listing: Option<Listing>,
    /// The listing came from this open's own refresh, not the saved copy.
    fresh: bool,
    refreshing: bool,
    /// Bumped per refresh, so a slow earlier answer can't land over a newer one.
    generation: u64,
    failed: Option<SharedString>,
    tab: Tab,
    query: String,
    shown: Vec<usize>,
    matched: usize,
    selected: Option<String>,
    /// A link's id, waiting for an index to look it up in.
    link: Option<String>,
    /// Opened from the updates chip, waiting for a listing with updates.
    updates_wanted: bool,
    cards: HashMap<String, Card>,
    previews: HashMap<String, Shot>,
    queue: VecDeque<String>,
    fetching: usize,
    outcome: Option<(String, Outcome)>,
    cancelled: Option<String>,
    polling: bool,
    focus: FocusHandle,
    _find_events: Subscription,
}

impl CatalogWindow {
    fn new(
        player: Option<EntityId>,
        link: Option<String>,
        updates_wanted: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let find = cx.new(|cx| {
            InputState::new(window, cx).placeholder_key("catalog-placeholder", window, cx)
        });
        window.focus(&find.focus_handle(cx));

        let _find_events = cx.subscribe_in(
            &find,
            window,
            |this: &mut Self, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change | InputEvent::PressEnter { .. }) {
                    this.refilter(cx);
                }
            },
        );

        let view = cx.weak_entity();
        cx.set_global(OpenCatalog {
            window: window.window_handle(),
            view,
        });

        let mut this = CatalogWindow {
            player,
            find,
            listing: None,
            fresh: false,
            refreshing: false,
            generation: 0,
            failed: None,
            tab: Tab::Workspaces,
            query: String::new(),
            shown: Vec::new(),
            matched: 0,
            selected: None,
            link,
            updates_wanted,
            cards: HashMap::new(),
            previews: HashMap::new(),
            queue: VecDeque::new(),
            fetching: 0,
            outcome: None,
            cancelled: None,
            polling: false,
            focus: cx.focus_handle(),
            _find_events,
        };

        this.load_saved(window, cx);
        this.refresh(window, cx);

        // An install started before this window opened is still running.
        if catalog::running(cx).is_some() {
            this.poll_install(cx);
        }

        this
    }

    // ── Loading ──

    /// The saved copy, shown while the refresh is out. A refresh that
    /// already landed is newer, so this one stands down.
    fn load_saved(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let saved = cx
                .background_executor()
                .spawn(async move { catalog::load().map(listing) })
                .await;

            this.update_in(cx, |this, window, cx| {
                if this.fresh {
                    return;
                }

                if let Some(saved) = saved {
                    this.take(saved, false, window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.refreshing = true;
        self.failed = None;
        cx.notify();

        let refreshing = catalog::refresh(cx);
        cx.spawn_in(window, async move |this, cx| {
            let fetched = refreshing.await;
            let fetched = cx
                .background_executor()
                .spawn(async move { fetched.map(listing) })
                .await;

            this.update_in(cx, |this, window, cx| {
                if this.generation != generation {
                    return;
                }

                this.refreshing = false;
                match fetched {
                    Ok(fresh) => {
                        this.fresh = true;

                        // A record that failed while the catalog was out of
                        // reach gets another go.
                        this.cards
                            .retain(|_, card| !matches!(card, Card::Failed(_)));
                        this.take(fresh, true, window, cx);
                        if let Some(id) = this.selected.clone() {
                            this.select(id, cx);
                        }
                    }
                    Err(reason) => {
                        log::warn!("catalog: refreshing: {reason}");
                        this.failed = Some(why(&reason));
                        this.settle_link(true, window, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// `settled` once this open's refresh has answered, either way: only
    /// then does a link that isn't listed say so.
    fn take(
        &mut self,
        listing: Listing,
        settled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.listing = Some(listing);

        if self.updates_wanted && self.tabs().contains(&Tab::Updates) {
            self.updates_wanted = false;
            self.tab = Tab::Updates;
        } else if settled {
            self.updates_wanted = false;
        }

        if !self.tabs().contains(&self.tab) {
            self.tab = Tab::Workspaces;
        }

        self.refilter(cx);
        self.settle_link(settled, window, cx);
    }

    /// A refresh or an install landed. A refresh of this window's own that's
    /// still out brings its own answer, so this stands down for it.
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.refreshing {
            return;
        }

        cx.spawn_in(window, async move |this, cx| {
            let saved = cx
                .background_executor()
                .spawn(async move { catalog::load().map(listing) })
                .await;

            this.update_in(cx, |this, window, cx| {
                let Some(saved) = saved.filter(|_| !this.refreshing) else {
                    return;
                };

                this.fresh = true;
                this.failed = None;
                this.take(saved, true, window, cx);
            })
            .ok();
        })
        .detach();
    }

    fn want_updates(&mut self, cx: &mut Context<Self>) {
        self.updates_wanted = true;

        if self.tabs().contains(&Tab::Updates) {
            self.updates_wanted = false;
            self.pick_tab(Tab::Updates, cx);
        }
    }

    fn tabs(&self) -> Vec<Tab> {
        tabs(
            self.listing
                .as_ref()
                .map_or(0, |listing| listing.updates.len()),
        )
    }

    fn follow_link(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.link = Some(id);
        self.settle_link(false, window, cx);
        self.refresh(window, cx);
    }

    fn settle_link(&mut self, settled: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.link.clone() else {
            return;
        };
        let Some(listing) = &self.listing else {
            return;
        };

        if let Some(kind) = listing.entries.iter().find(|e| e.id == id).map(|e| e.kind) {
            self.link = None;
            if let Some(tab) = Tab::of(kind) {
                self.tab = tab;
            }

            self.find
                .update(cx, |find, cx| find.set_value("", window, cx));
            self.refilter(cx);
            self.select(id, cx);
            return;
        }

        // The saved copy may be older than the package.
        if !settled {
            return;
        }

        self.link = None;
        let message = match listing.too_new.get(&id) {
            Some(name) => rox_i18n::t!("catalog-link-too-new", name = name.clone()),
            None => rox_i18n::t!("catalog-link-unknown", id = id),
        };
        Toast::new(panel::Tone::Info, message).show(window, cx);
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        self.query = self.find.read(cx).value().trim().to_string();

        let Some(listing) = &self.listing else {
            self.shown.clear();
            self.matched = 0;
            cx.notify();
            return;
        };

        let hits: Vec<usize> = listing
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                in_tab(entry, self.tab, &listing.updates) && matches(entry, &self.query)
            })
            .map(|(ix, _)| ix)
            .collect();
        self.matched = hits.len();
        self.shown = hits.into_iter().take(RESULT_LIMIT).collect();

        let wanted: Vec<String> = self
            .shown
            .iter()
            .filter_map(|&ix| listing.entries[ix].preview.clone())
            .collect();
        for path in wanted {
            self.want_preview(path, cx);
        }

        cx.notify();
    }

    fn pick_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        self.refilter(cx);
    }

    fn entry(&self, id: &str) -> Option<&Entry> {
        self.listing
            .as_ref()?
            .entries
            .iter()
            .find(|entry| entry.id == id)
    }

    fn select(&mut self, id: String, cx: &mut Context<Self>) {
        self.selected = Some(id.clone());
        cx.notify();

        if matches!(
            self.cards.get(&id),
            Some(Card::Loading) | Some(Card::Ready(_))
        ) {
            return;
        }
        let Some(sha256) = self.entry(&id).map(|entry| entry.sha256.clone()) else {
            return;
        };

        self.cards.insert(id.clone(), Card::Loading);
        let fetching = catalog::fetch_record(&id, cx);

        cx.spawn(async move |this, cx| {
            let fetched = fetching.await;
            let card = cx
                .background_executor()
                .spawn(async move { fetched.map(|record| detail(record, &sha256)) })
                .await;

            this.update(cx, |this, cx| {
                let card = match card {
                    Ok(detail) => {
                        for path in detail.previews.clone() {
                            this.want_preview(path, cx);
                        }
                        Card::Ready(detail)
                    }
                    Err(reason) => {
                        log::warn!("catalog: record {id}: {reason}");
                        Card::Failed(rox_i18n::t!(
                            "catalog-record-failed",
                            reason = why(&reason).to_string()
                        ))
                    }
                };
                this.cards.insert(id, card);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ── Previews ──

    fn want_preview(&mut self, path: String, cx: &mut Context<Self>) {
        if self.previews.contains_key(&path) {
            return;
        }

        self.previews.insert(path.clone(), Shot::Loading);
        self.queue.push_back(path);
        self.pump(cx);
    }

    /// The bytes come only through the client's capped fetch, and they
    /// decode under the canvas's pixel cap before gpui sees them.
    fn pump(&mut self, cx: &mut Context<Self>) {
        while self.fetching < PREVIEW_JOBS {
            let Some(path) = self.queue.pop_front() else {
                break;
            };
            self.fetching += 1;

            cx.spawn(async move |this, cx| {
                let decoded = cx
                    .background_executor()
                    .spawn({
                        let path = path.clone();
                        async move {
                            catalog::preview(&path)
                                .and_then(|bytes| crate::canvas::image::decode_still(&bytes))
                        }
                    })
                    .await;

                this.update(cx, |this, cx| {
                    this.fetching -= 1;

                    let shot = match decoded {
                        Ok(image) => Shot::Ready(image),
                        Err(e) => {
                            log::info!("catalog: preview {path}: {e}");
                            Shot::Failed
                        }
                    };
                    this.previews.insert(path, shot);

                    this.pump(cx);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    // ── Install ──

    fn install(&mut self, id: String, choice: catalog::Choice, cx: &mut Context<Self>) {
        self.outcome = None;
        self.cancelled = None;

        let object_update = self
            .entry(&id)
            .is_some_and(|entry| entry.kind == Kind::Object)
            && self
                .listing
                .as_ref()
                .is_some_and(|listing| listing.updates.contains(&id));

        let installing = catalog::install(&id, choice, cx);
        self.poll_install(cx);
        cx.notify();

        cx.spawn(async move |this, cx| {
            let outcome = installing.await;
            let quiet = cx
                .update(|cx| rox_panel_api::windows::front_workspace(cx).is_none())
                .unwrap_or(false);

            this.update(cx, |this, cx| {
                match outcome {
                    Ok(installed) => {
                        if let Some(listing) = &mut this.listing {
                            listing.updates.remove(&installed.id);
                            listing.installed.insert(installed.id);
                        }
                        if !this.tabs().contains(&this.tab) {
                            this.tab = Tab::Workspaces;
                        }
                        this.refilter(cx);

                        if let Some(said) = settled(object_update, quiet) {
                            this.outcome = Some((id, said));
                        }
                    }

                    // The user's own Cancel isn't a failure to report.
                    Err(_) if this.cancelled.as_deref() == Some(id.as_str()) => {}

                    // Every failure on a Replace lands before the old file is
                    // touched, so the line can say it's unchanged.
                    Err(reason) => {
                        let reason = why(&reason).to_string();
                        let said = match choice {
                            catalog::Choice::Replace => {
                                rox_i18n::t!("catalog-replace-failed", reason = reason)
                            }
                            catalog::Choice::KeepBoth => {
                                rox_i18n::t!("catalog-install-failed", reason = reason)
                            }
                        };
                        this.outcome = Some((id, Outcome::Failed(said)));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some((id, _)) = catalog::running(cx) {
            self.cancelled = Some(id);
        }

        catalog::cancel(cx);
        cx.notify();
    }

    /// The progress lives in atomics, so the window only needs frames until
    /// the install settles.
    fn poll_install(&mut self, cx: &mut Context<Self>) {
        if self.polling {
            return;
        }
        self.polling = true;

        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;

                let live = this.update(cx, |_, cx| {
                    cx.notify();
                    catalog::running(cx).is_some()
                });
                if !matches!(live, Ok(true)) {
                    break;
                }
            }

            this.update(cx, |this, _| this.polling = false).ok();
        })
        .detach();
    }

    // ── Drawing ──

    fn search_row(&self) -> Div {
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(tokens::SPACE_XS)
            .p(tokens::SPACE_SM)
            .border_b_1()
            .border_color(palette::border())
            .child(div().flex_1().min_w_0().child(Input::new(&self.find)))
    }

    fn tab_row(&self, cx: &mut Context<Self>) -> Div {
        let options: Vec<(SharedString, Tab)> = self
            .tabs()
            .into_iter()
            .map(|tab| (tab.label(), tab))
            .collect();

        div()
            .flex_none()
            .flex()
            .items_center()
            .px(tokens::SPACE_SM)
            .py(tokens::SPACE_XS)
            .border_b_1()
            .border_color(palette::border())
            .child(panel::choices_shared(
                &options,
                self.tab,
                |this: &mut Self, tab, cx| this.pick_tab(tab, cx),
                cx,
            ))
    }

    /// Only once there's a list to keep showing: the saved copy's line.
    fn status_row(&self, cx: &mut Context<Self>) -> Option<Div> {
        self.failed.as_ref()?;
        self.listing.as_ref()?;

        Some(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(tokens::SPACE_SM)
                .px(tokens::SPACE_SM)
                .py(tokens::SPACE_XS)
                .bg(palette::alpha(palette::tone_warn(), 0x18))
                .border_b_1()
                .border_color(palette::border())
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_xs()
                        .text_color(palette::text())
                        .child(rox_i18n::t!("catalog-saved-copy")),
                )
                .child(self.retry_button(cx)),
        )
    }

    fn retry_button(&self, cx: &mut Context<Self>) -> Button {
        Button::new("catalog-retry")
            .icon(Icon::default().path(icons::REFRESH_CW))
            .label(rox_i18n::t!("catalog-retry"))
            .small()
            .outline()
            .disabled(self.refreshing)
            .on_click(cx.listener(|this, _, window, cx| this.refresh(window, cx)))
    }

    fn centered(line: SharedString) -> Div {
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(tokens::SPACE_SM)
            .p(tokens::SPACE_MD)
            .text_sm()
            .text_center()
            .text_color(palette::text_muted())
            .child(line)
    }

    /// With no list at all: loading, or the failure with Retry.
    fn nothing_yet(&self, cx: &mut Context<Self>) -> Div {
        if let Some(reason) = self.failed.clone() {
            return Self::centered(rox_i18n::t!("catalog-unreachable"))
                .child(
                    div()
                        .max_w(px(420.))
                        .text_xs()
                        .text_color(palette::text_faint())
                        .child(reason),
                )
                .child(self.retry_button(cx));
        }

        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .gap(tokens::SPACE_SM)
            .text_sm()
            .text_color(palette::text_muted())
            .child(Spinner::new())
            .child(rox_i18n::t!("catalog-loading"))
    }

    fn list(&self, listing: &Listing, cx: &mut Context<Self>) -> Div {
        let column = div()
            .flex_none()
            .w(relative(0.42))
            .min_w(px(260.))
            .max_w(px(400.))
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(palette::border());

        if self.shown.is_empty() {
            let said = if self.query.is_empty() {
                rox_i18n::t!("catalog-empty")
            } else {
                rox_i18n::t!("catalog-none", text = self.query.clone())
            };
            return column.child(Self::centered(said));
        }

        let mut rows: Vec<AnyElement> = self
            .shown
            .iter()
            .map(|&ix| self.row(ix, &listing.entries[ix], listing, cx))
            .collect();

        if self.matched > self.shown.len() {
            rows.push(
                div()
                    .p(tokens::SPACE_SM)
                    .text_xs()
                    .text_color(palette::text_faint())
                    .child(rox_i18n::t!("catalog-more", shown = self.shown.len()))
                    .into_any_element(),
            );
        }

        column.child(
            div()
                .id("catalog-list")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .children(rows),
        )
    }

    fn row(
        &self,
        ix: usize,
        entry: &Entry,
        listing: &Listing,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.selected.as_deref() == Some(entry.id.as_str());
        let id = entry.id.clone();

        div()
            .id(("catalog-row", ix))
            .flex()
            .items_center()
            .gap(tokens::SPACE_SM)
            .px(tokens::SPACE_SM)
            .py(tokens::SPACE_XS)
            .w_full()
            .min_w_0()
            .cursor_pointer()
            .when(selected, |row| {
                row.bg(palette::alpha(palette::accent(), 0x22))
            })
            .when(!selected, |row| {
                row.hover(|row| row.bg(palette::bg_control_hover()))
            })
            .on_click(cx.listener(move |this, _, _, cx| this.select(id.clone(), cx)))
            .child(
                self.picture(entry.preview.as_deref(), entry.kind, ObjectFit::Cover)
                    .w(px(56.))
                    .h(px(42.)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(tokens::SPACE_XS)
                            .min_w_0()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .text_color(palette::text_bright())
                                    .child(SharedString::from(entry.name.clone())),
                            )
                            .map(|d| {
                                if listing.updates.contains(&entry.id) {
                                    d.child(update_mark())
                                } else if listing.installed.contains(&entry.id) {
                                    d.child(installed_mark())
                                } else {
                                    d
                                }
                            }),
                    )
                    .when(!entry.author.is_empty(), |d| {
                        d.child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(palette::text_muted())
                                .child(rox_i18n::t!("catalog-by", author = entry.author.clone())),
                        )
                    })
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(palette::text_faint())
                            .child(SharedString::from(entry.summary.clone())),
                    ),
            )
            .into_any_element()
    }

    /// A preview, or the kind's icon while there's none to show.
    fn picture(&self, path: Option<&str>, kind: Kind, fit: ObjectFit) -> Div {
        let frame = div()
            .flex_none()
            .rounded(tokens::RADIUS)
            .overflow_hidden()
            .bg(palette::bg_control())
            .flex()
            .items_center()
            .justify_center();

        match path.and_then(|path| self.previews.get(path)) {
            Some(Shot::Ready(image)) => frame.child(img(image.clone()).size_full().object_fit(fit)),
            _ => frame.child(
                svg()
                    .path(kind_icon(kind))
                    .size(px(18.))
                    .text_color(palette::text_faint()),
            ),
        }
    }

    fn detail_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        let pane = div().flex_1().min_w_0().h_full().flex().flex_col();

        let Some(entry) = self.selected.as_deref().and_then(|id| self.entry(id)) else {
            return pane
                .child(Self::centered(rox_i18n::t!("catalog-pick")))
                .into_any_element();
        };
        let card = self.cards.get(&entry.id);
        let detail = match card {
            Some(Card::Ready(detail)) => Some(detail),
            _ => None,
        };

        // The record's previews, or the index's one until the record lands.
        let previews: Vec<&str> = match detail {
            Some(detail) if !detail.previews.is_empty() => {
                detail.previews.iter().map(String::as_str).collect()
            }
            _ => entry.preview.as_deref().into_iter().collect(),
        };

        let mut page = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .p(tokens::SPACE_MD)
            .children(self.banner(&previews, entry.kind))
            .child(
                div()
                    .text_lg()
                    .text_color(palette::text_bright())
                    .child(SharedString::from(entry.name.clone())),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(tokens::SPACE_XS)
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(kind_label(entry.kind))
                    .when(!entry.author.is_empty(), |d| {
                        d.child(rox_i18n::t!("catalog-by", author = entry.author.clone()))
                    }),
            )
            .child(self.action(entry, cx));

        if !entry.summary.is_empty() {
            page = page.child(
                div()
                    .text_sm()
                    .text_color(palette::text())
                    .child(SharedString::from(entry.summary.clone())),
            );
        }

        page = match card {
            Some(Card::Ready(detail)) if !detail.description.is_empty() => page.child(
                div()
                    .text_sm()
                    .text_color(palette::text_muted())
                    .child(SharedString::from(detail.description.clone())),
            ),
            Some(Card::Loading) => page.child(
                div()
                    .flex()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(Spinner::new())
                    .child(rox_i18n::t!("catalog-record-loading")),
            ),
            Some(Card::Failed(line)) => page.child(
                div()
                    .text_xs()
                    .text_color(palette::tone_bad())
                    .child(line.clone()),
            ),
            _ => page,
        };

        page = page.child(self.facts(entry, detail));

        if let Some(detail) = detail.filter(|detail| !detail.notes.is_empty()) {
            page = page
                .child(
                    div()
                        .pt(tokens::SPACE_SM)
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(rox_i18n::t!("catalog-notes")),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(palette::text())
                        .child(SharedString::from(detail.notes.clone())),
                );
        }

        pane.child(
            div()
                .id("catalog-detail")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(page),
        )
        .into_any_element()
    }

    /// The first preview large and whole, the rest small beneath it, and no
    /// box at all for a package that names none.
    fn banner(&self, previews: &[&str], kind: Kind) -> Option<Div> {
        let (first, rest) = previews.split_first()?;

        let mut banner = div().flex().flex_col().gap(tokens::SPACE_XS).child(
            self.picture(Some(first), kind, ObjectFit::Contain)
                .w_full()
                .h(px(200.)),
        );

        if !rest.is_empty() {
            banner = banner.child(div().flex().flex_wrap().gap(tokens::SPACE_XS).children(
                rest.iter().map(|path| {
                    self.picture(Some(path), kind, ObjectFit::Cover)
                        .w(px(96.))
                        .h(px(72.))
                }),
            ));
        }

        Some(banner)
    }

    fn action(&self, entry: &Entry, cx: &mut Context<Self>) -> Div {
        let mut column = div().flex().flex_col().gap(tokens::SPACE_XS);
        let listing = self.listing.as_ref();
        let installed = listing.is_some_and(|listing| listing.installed.contains(&entry.id));
        let update = listing.is_some_and(|listing| listing.updates.contains(&entry.id));
        let asking = listing.is_some_and(|listing| listing.asking.contains(&entry.id));
        let running = catalog::running(cx);

        match &running {
            Some((id, progress)) if *id == entry.id => {
                let stopping = progress.stopping();
                column = column.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(tokens::SPACE_SM)
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(80.))
                                .child(bar(progress.fraction())),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(palette::text_muted())
                                .child(rox_i18n::t!(
                                    "catalog-downloading",
                                    done = fmt_bytes(progress.done()),
                                    total = fmt_bytes(progress.total())
                                )),
                        )
                        .child(
                            Button::new("catalog-cancel")
                                .icon(Icon::default().path(icons::STOP))
                                .label(rox_i18n::t!("catalog-cancel"))
                                .small()
                                .outline()
                                .disabled(stopping)
                                .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                        ),
                );
            }

            _ if installed && !update => {
                column = column.child(div().flex().child(installed_mark()));
            }

            _ if update && asking => {
                let busy = running.is_some();
                let button = |id: &'static str, icon: &'static str, label, choice| {
                    let package = entry.id.clone();

                    Button::new(id)
                        .icon(Icon::default().path(icon))
                        .label(label)
                        .small()
                        .outline()
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.install(package.clone(), choice, cx)
                        }))
                };

                // Objects already placed are the canvas's own items, which a
                // user would otherwise expect to follow the update.
                let explained = match entry.kind {
                    Kind::Object => rox_i18n::t!("catalog-update-choice-object"),
                    _ => rox_i18n::t!("catalog-update-choice-workspace"),
                };

                column = column
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(tokens::SPACE_XS)
                            .child(button(
                                "catalog-replace",
                                icons::REFRESH_CW,
                                rox_i18n::t!("catalog-replace"),
                                catalog::Choice::Replace,
                            ))
                            .child(button(
                                "catalog-keep-both",
                                icons::COPY,
                                rox_i18n::t!("catalog-keep-both"),
                                catalog::Choice::KeepBoth,
                            )),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette::text_muted())
                            .child(explained),
                    );
                column = self.busy_line(column, running.as_ref().map(|(id, _)| id.as_str()));
            }

            _ => {
                let label = if update {
                    rox_i18n::t!("catalog-update")
                } else {
                    rox_i18n::t!("catalog-install")
                };
                let id = entry.id.clone();

                column = column.child(
                    div().flex().child(
                        Button::new("catalog-install")
                            .icon(Icon::default().path(icons::DOWNLOAD))
                            .label(label)
                            .small()
                            .outline()
                            .disabled(running.is_some())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.install(id.clone(), catalog::Choice::KeepBoth, cx)
                            })),
                    ),
                );
                column = self.busy_line(column, running.as_ref().map(|(id, _)| id.as_str()));
            }
        }

        match &self.outcome {
            Some((id, Outcome::Failed(line))) if *id == entry.id => column.child(
                div()
                    .text_xs()
                    .text_color(palette::tone_bad())
                    .child(line.clone()),
            ),
            Some((id, Outcome::Quiet)) if *id == entry.id => column.child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!("catalog-installed-quiet")),
            ),
            Some((id, Outcome::ObjectUpdated)) if *id == entry.id => column.child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!("catalog-object-updated")),
            ),
            _ => column,
        }
    }

    /// One install at a time: say whose is holding this one up.
    fn busy_line(&self, column: Div, running: Option<&str>) -> Div {
        match running.and_then(|id| self.entry(id)) {
            Some(other) => column.child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!("catalog-busy", name = other.name.clone())),
            ),
            None => column,
        }
    }

    fn facts(&self, entry: &Entry, detail: Option<&Detail>) -> Div {
        let text = |value: String| SharedString::from(value).into_any_element();
        let mut facts: Vec<(SharedString, AnyElement)> = Vec::new();

        if !entry.version.is_empty() {
            facts.push((rox_i18n::t!("catalog-version"), text(entry.version.clone())));
        }
        if let Some(published) = detail.map(|d| &d.published).filter(|p| !p.is_empty()) {
            facts.push((rox_i18n::t!("catalog-published"), text(published.clone())));
        }
        if !entry.license.is_empty() {
            facts.push((rox_i18n::t!("catalog-license"), text(entry.license.clone())));
        }
        facts.push((rox_i18n::t!("catalog-size"), text(fmt_bytes(entry.bytes))));
        if !entry.tags.is_empty() {
            facts.push((rox_i18n::t!("catalog-tags"), text(entry.tags.join(", "))));
        }

        // Read per draw, so a plugin installed meanwhile stops showing as missing.
        if !entry.requires.is_empty() {
            let needs: Vec<String> = entry
                .requires
                .iter()
                .map(|id| {
                    if host::present(id) {
                        id.clone()
                    } else {
                        rox_i18n::t!("catalog-needs-missing", id = id.clone()).to_string()
                    }
                })
                .collect();
            facts.push((rox_i18n::t!("catalog-needs"), text(needs.join(", "))));
        }

        if let Some(homepage) = detail.and_then(|d| d.homepage.clone()) {
            facts.push((rox_i18n::t!("catalog-homepage"), link(homepage)));
        }

        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .pt(tokens::SPACE_SM)
            .text_xs()
            .children(facts.into_iter().map(|(label, value)| {
                div()
                    .flex()
                    .items_start()
                    .gap(tokens::SPACE_SM)
                    .child(
                        div()
                            .flex_none()
                            .w(px(96.))
                            .text_color(palette::text_muted())
                            .child(label),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(palette::text())
                            .child(value),
                    )
            }))
    }
}

/// The line a card keeps after an install lands. With no workspace window
/// up, nothing was offered anywhere, an object's update included.
fn settled(object_update: bool, quiet: bool) -> Option<Outcome> {
    if quiet {
        Some(Outcome::Quiet)
    } else if object_update {
        Some(Outcome::ObjectUpdated)
    } else {
        None
    }
}

fn row_mark(icon: &'static str, label: SharedString) -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(2.))
        .text_xs()
        .text_color(palette::accent())
        .child(Icon::default().path(icon).xsmall())
        .child(label)
}

fn installed_mark() -> Div {
    row_mark(icons::CHECK, rox_i18n::t!("catalog-installed"))
}

/// Filled like the menubar's update chips, so it reads as the same news.
fn update_mark() -> Div {
    row_mark(icons::ARROW_UP, rox_i18n::t!("catalog-update-available"))
        .px(tokens::SPACE_XS)
        .rounded_full()
        .bg(palette::accent())
        .text_color(palette::text_on_accent())
}

fn bar(fraction: f32) -> Div {
    div()
        .h(px(4.))
        .w_full()
        .rounded(px(2.))
        .bg(palette::bg_root())
        .child(
            div()
                .h_full()
                .rounded(px(2.))
                .w(relative(fraction.clamp(0.0, 1.0)))
                .bg(palette::accent()),
        )
}

/// Labelled with the host it goes to, so the place a click lands is never
/// text the catalog chose.
fn link(homepage: Homepage) -> AnyElement {
    let url = homepage.url;

    div()
        .flex()
        .items_center()
        .gap(px(2.))
        .text_color(palette::accent())
        .underline()
        .cursor_pointer()
        .hover(|d| d.text_color(palette::accent_hover()))
        .on_mouse_down(MouseButton::Left, move |_, _, cx: &mut App| {
            rox_core::desktop::open_url(cx, "catalog: open link", &url)
        })
        .child(SharedString::from(homepage.host))
        .child(Icon::default().path(icons::EXTERNAL_LINK).xsmall())
        .into_any_element()
}

impl Render for CatalogWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // No workspace player: this window's own id reads the base palette.
        let player = self.player.unwrap_or_else(|| cx.entity().entity_id());
        palette::note_focus(player, window.is_window_active(), cx);

        // Build inside the closure: anything built outside paints untinted.
        panel::window_body(player, || {
            let body: AnyElement = match &self.listing {
                Some(listing) => div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .child(self.list(listing, cx))
                    .child(self.detail_pane(cx))
                    .into_any_element(),
                None => self.nothing_yet(cx).into_any_element(),
            };

            div()
                .size_full()
                .flex()
                .flex_col()
                .bg(palette::bg_elevated())
                .text_color(palette::text())
                .text_sm()
                .track_focus(&self.focus)
                // The box passes an idle escape through, so this catches it anywhere.
                .on_key_down(cx.listener(|_, event: &KeyDownEvent, window, _| {
                    if event.keystroke.key == "escape" {
                        window.remove_window();
                    }
                }))
                .child(self.search_row())
                .when(self.listing.is_some(), |d| d.child(self.tab_row(cx)))
                .children(self.status_row(cx))
                .child(body)
                .into_any_element()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_object_update_says_it_was_offered_like_an_install() {
        assert!(matches!(settled(true, false), Some(Outcome::ObjectUpdated)));

        // A first install says nothing: its confirm or its place is the answer.
        assert!(settled(false, false).is_none());

        // With no window up nothing was offered, so neither line claims it was.
        assert!(matches!(settled(true, true), Some(Outcome::Quiet)));
        assert!(matches!(settled(false, true), Some(Outcome::Quiet)));
    }

    fn summary(id: &str, kind: &str, fields: serde_json::Value) -> Summary {
        let mut value = serde_json::json!({
            "id": id,
            "kind": kind,
            "name": id,
            "author": "someone",
            "summary": "One line.",
            "latest": {
                "version": "1.0",
                "file": format!("files/{id}.roxw"),
                "bytes": 10,
                "sha256": "0".repeat(64),
                "min_rox": "2.0.0",
            },
        });
        for (key, field) in fields.as_object().unwrap() {
            value[key] = field.clone();
        }

        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn search_matches_name_author_summary_and_tags() {
        let aurora = entry(&summary(
            "aurora",
            "workspace",
            serde_json::json!({
                "name": "Aurora Borealis",
                "author": "Night Owl",
                "summary": "Green ribbons over a dark sky.",
                "tags": ["minimal", "Dark"],
            }),
        ));

        for query in [
            "aurora", "BOREALIS", "owl", "ribbons", "minimal", "dark", "",
        ] {
            assert!(matches(&aurora, query), "{query:?}");
        }

        // Every word has to land somewhere, in any field.
        assert!(matches(&aurora, "owl minimal"));
        assert!(!matches(&aurora, "owl sunrise"));

        // The id and the file aren't searched.
        assert!(!matches(&aurora, "roxw"));
    }

    #[test]
    fn the_updates_tab_hides_when_there_are_none() {
        assert!(!tabs(0).contains(&Tab::Updates));
        assert_eq!(tabs(0), vec![Tab::Workspaces, Tab::Plugins, Tab::Objects]);
        assert_eq!(tabs(2).last(), Some(&Tab::Updates));

        let lamp = entry(&summary("lamp", "object", serde_json::json!({})));
        let updates: HashSet<String> = ["lamp".to_string()].into();
        assert!(in_tab(&lamp, Tab::Updates, &updates));
        assert!(in_tab(&lamp, Tab::Objects, &updates));
        assert!(!in_tab(&lamp, Tab::Updates, &HashSet::new()));
    }

    #[test]
    fn a_link_opens_on_its_id() {
        assert_eq!(install_id("rox://install/aurora"), Some("aurora"));
        assert_eq!(install_id("ROX://install/tones-2"), Some("tones-2"));
    }

    #[test]
    fn a_link_with_one_trailing_slash_opens_on_its_id() {
        assert_eq!(install_id("rox://install/aurora/"), Some("aurora"));
        assert_eq!(install_id("ROX://install/tones-2/"), Some("tones-2"));
    }

    #[test]
    fn a_link_with_more_after_the_slash_is_dropped() {
        for link in [
            "rox://install/aurora//",
            "rox://install/aurora/x",
            "rox://install/aurora/?",
            "rox://install/aurora/#now",
            "rox://install//",
            "rox://install//aurora",
        ] {
            assert_eq!(install_id(link), None, "{link}");
        }
    }

    #[test]
    fn a_link_with_a_query_is_dropped() {
        for link in [
            "rox://install/aurora?file=https://elsewhere.example/a.roxw",
            "rox://install/aurora?",
            "rox://install/aurora?/",
            "rox://install/aurora#now",
            "rox://install/aurora#/",
            "rox://install/aurora/files/x.roxw",
        ] {
            assert_eq!(install_id(link), None, "{link}");
        }
    }

    #[test]
    fn a_link_with_a_bad_id_is_dropped() {
        for link in [
            "rox://install/",
            "rox://install/Aurora",
            "rox://install/-flag",
            "rox://install/../index",
            "rox://install/aur%6Fra",
            "rox://install/a b",
            "rox://install/aurora\n",
            "rox://install/über",
            &format!("rox://install/{}", "a".repeat(65)),
        ] {
            assert_eq!(install_id(link), None, "{link:?}");
        }

        assert_eq!(
            install_id(&format!("rox://install/{}", "a".repeat(64))).map(str::len),
            Some(64)
        );
    }

    #[test]
    fn a_link_to_another_action_is_dropped() {
        for link in [
            "rox://open/aurora",
            "rox://INSTALL/aurora",
            "rox://install-now/aurora",
            "rox:install/aurora",
            "rox:///install/aurora",
            "https://install/aurora",
            "rox://",
            "rox",
            "",
        ] {
            assert_eq!(install_id(link), None, "{link}");
        }
    }

    #[test]
    fn catalog_text_is_plain_and_bounded() {
        // Control and bidi characters go, and breaks fold on one line.
        assert_eq!(
            line("A\u{202E}gnp.exe\u{202C}\tname\r\nhere\u{0007}", 80),
            "Agnp.exe name here"
        );
        assert_eq!(line("  \u{200B}  ", 80), "");

        // Markup is just text.
        assert_eq!(line("<b>bold</b> **x**", 80), "<b>bold</b> **x**");

        // Cut by characters, not bytes.
        assert_eq!(line(&"ü".repeat(100), 10), format!("{}...", "ü".repeat(10)));

        // A block keeps its breaks, one blank line at most, and its cap.
        assert_eq!(
            block("one\n\n\n\ntwo\r\nthree\n\n", 100, 10),
            "one\n\ntwo\nthree"
        );
        assert_eq!(block(&"x\n".repeat(50), 1000, 3), "x\nx\nx\n...");

        // An empty name falls back to the id.
        let bare = entry(&summary(
            "bare",
            "plugin",
            serde_json::json!({ "name": "\u{202E}" }),
        ));
        assert_eq!(bare.name, "bare");
    }

    #[test]
    fn a_homepage_opens_only_as_https_to_a_plain_host() {
        assert_eq!(
            homepage("https://example.org/aurora?x=1"),
            Some(Homepage {
                url: "https://example.org/aurora?x=1".into(),
                host: "example.org".into(),
            })
        );
        assert_eq!(
            homepage("https://Example.org:8443").map(|h| h.host),
            Some("example.org".into())
        );

        for refused in [
            "",
            "http://example.org",
            "javascript:alert(1)",
            "file:///etc/passwd",
            "HTTPS://example.org",
            "https://",
            "https://user:pw@example.org",
            "https://good.example@evil.example",
            "https://[::1]/",
            "https://exämple.org",
            "https://example.org/a b",
            "https://example.org:port",
            "https://.example.org",
            "https://example.org\u{202E}/",
            &format!("https://example.org/{}", "a".repeat(400)),
        ] {
            assert_eq!(homepage(refused), None, "{refused:?}");
        }
    }
}
