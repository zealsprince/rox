//! The library panel: the projection-backed track list with substring search,
//! and the scan entry point. Owns the app's library database; the list only
//! ever reads the in-memory projection, per the library service boundary.
//! Clicking a track emits a play request the workspace routes to the player.

use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    div, prelude::*, px, relative, rgb, uniform_list, Context, EventEmitter, FocusHandle,
    KeyDownEvent, MouseButton, PathPromptOptions, SharedString, Window,
};

use rox_library::projection::Projection;
use rox_library::rusqlite::Connection;
use rox_library::scanner::{self, ScanSummary};
use rox_library::store;

/// Play from a clicked row: at most this many tracks are queued behind it.
const QUEUE_CAP: usize = 1000;

pub enum LibraryEvent {
    Play(Vec<PathBuf>),
}

pub struct LibraryPanel {
    db_path: PathBuf,
    /// UI-side connection for id -> path lookups; scans and projection loads
    /// open their own connections on the background executor.
    conn: Option<Connection>,
    projection: Option<Arc<Projection>>,
    /// The canonical browse order: artist, album, track number.
    order: Arc<Vec<u32>>,
    /// Rows currently displayed: the canonical order, or search hits.
    view: Arc<Vec<u32>>,
    query: String,
    search_focus: FocusHandle,
    /// Set while a scan or projection load runs in the background.
    busy: Option<SharedString>,
    status: SharedString,
}

impl EventEmitter<LibraryEvent> for LibraryPanel {}

impl LibraryPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("rox");
        let _ = std::fs::create_dir_all(&dir);
        let db_path = dir.join("library.db");
        let (conn, status) = match store::open(&db_path)
            .and_then(|conn| store::init_schema(&conn).map(|_| conn))
        {
            Ok(conn) => (Some(conn), SharedString::default()),
            Err(e) => (None, SharedString::from(format!("library db: {e}"))),
        };

        let mut this = LibraryPanel {
            db_path,
            conn,
            projection: None,
            order: Arc::new(Vec::new()),
            view: Arc::new(Vec::new()),
            query: String::new(),
            search_focus: cx.focus_handle(),
            busy: None,
            status,
        };
        if this.conn.is_some() {
            this.reload(None, cx);
        }
        this
    }

    /// Load the projection off the UI thread, optionally scanning `root`
    /// first. The finished projection and its canonical sort swap in whole.
    fn reload(&mut self, scan_root: Option<PathBuf>, cx: &mut Context<Self>) {
        self.busy = Some(if scan_root.is_some() {
            "scanning...".into()
        } else {
            "loading library...".into()
        });
        let db_path = self.db_path.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { load(&db_path, scan_root) })
                .await;
            this.update(cx, |this, cx| {
                this.busy = None;
                match result {
                    Ok((projection, order, summary)) => {
                        this.status = status_line(projection.len(), summary.as_ref()).into();
                        this.projection = Some(Arc::new(projection));
                        this.order = Arc::new(order);
                        this.refresh_view();
                    }
                    Err(e) => this.status = format!("library: {e}").into(),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn refresh_view(&mut self) {
        let Some(projection) = &self.projection else {
            self.view = Arc::new(Vec::new());
            return;
        };
        self.view = if self.query.is_empty() {
            self.order.clone()
        } else {
            Arc::new(projection.search(&self.query))
        };
    }

    pub fn browse(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = rx.await {
                if let Some(root) = paths.pop() {
                    this.update(cx, |this, cx| this.reload(Some(root), cx)).ok();
                }
            }
        })
        .detach();
    }

    /// Queue the clicked row and what follows it in the current view order.
    fn play_from(&mut self, ix: usize, cx: &mut Context<Self>) {
        let (Some(projection), Some(conn)) = (&self.projection, &self.conn) else {
            return;
        };
        let end = (ix + QUEUE_CAP).min(self.view.len());
        let ids: Vec<i64> = self.view[ix..end]
            .iter()
            .map(|&row| projection.db_id[row as usize])
            .collect();
        match store::paths_for(conn, &ids) {
            Ok(paths) => cx.emit(LibraryEvent::Play(
                paths.into_iter().map(Into::into).collect(),
            )),
            Err(e) => {
                self.status = format!("library: {e}").into();
                cx.notify();
            }
        }
    }

    fn on_search_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        match keystroke.key.as_str() {
            "backspace" => {
                self.query.pop();
            }
            "escape" => self.query.clear(),
            _ => {
                if keystroke.modifiers.control
                    || keystroke.modifiers.platform
                    || keystroke.modifiers.alt
                {
                    return;
                }
                let Some(text) = &keystroke.key_char else { return };
                self.query.push_str(text);
            }
        }
        self.refresh_view();
        cx.notify();
    }

    fn toolbar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.search_focus.is_focused(window);
        let search_text: SharedString = if self.query.is_empty() {
            "search".into()
        } else {
            self.query.clone().into()
        };
        div()
            .flex_none()
            .h(px(36.))
            .px_2()
            .gap_2()
            .flex()
            .flex_row()
            .items_center()
            .bg(rgb(0x1f1f1f))
            .border_b_1()
            .border_color(rgb(0x333333))
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(rgb(0x2a2a2a))
                    .hover(|d| d.bg(rgb(0x3a3a3a)))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.browse(cx)),
                    )
                    .child("open folder"),
            )
            .child(
                div()
                    .flex_1()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(rgb(0x141414))
                    .border_1()
                    .border_color(if focused { rgb(0x4a6a55) } else { rgb(0x333333) })
                    .when(self.query.is_empty(), |d| d.text_color(rgb(0x808080)))
                    .track_focus(&self.search_focus)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            window.focus(&this.search_focus);
                            cx.notify();
                        }),
                    )
                    .on_key_down(cx.listener(|this, event, _, cx| {
                        this.on_search_key(event, cx);
                    }))
                    .child(search_text),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(rgb(0x808080))
                    .child(self.busy.clone().unwrap_or_else(|| self.status.clone())),
            )
    }

    fn track_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        uniform_list(
            "tracks",
            self.view.len(),
            cx.processor(|this, range: Range<usize>, _, cx| {
                let Some(projection) = this.projection.clone() else {
                    return Vec::new();
                };
                let view = this.view.clone();
                range
                    .map(|ix| {
                        let v = projection.resolve(view[ix]);
                        div()
                            .id(ix)
                            .flex_none()
                            .h(px(28.))
                            .px_2()
                            .gap_2()
                            .flex()
                            .flex_row()
                            .items_center()
                            .when(ix % 2 == 1, |d| d.bg(rgb(0x202020)))
                            .hover(|d| d.bg(rgb(0x2e2e2e)))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.play_from(ix, cx)
                            }))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(SharedString::from(v.title.to_string())),
                            )
                            .child(
                                div()
                                    .w(relative(0.22))
                                    .min_w_0()
                                    .truncate()
                                    .text_color(rgb(0xa0a0a0))
                                    .child(SharedString::from(v.artist.to_string())),
                            )
                            .child(
                                div()
                                    .w(relative(0.22))
                                    .min_w_0()
                                    .truncate()
                                    .text_color(rgb(0xa0a0a0))
                                    .child(SharedString::from(v.album.to_string())),
                            )
                            .child(
                                div()
                                    .w(px(44.))
                                    .flex_none()
                                    .text_color(rgb(0x808080))
                                    .child(fmt_ms(v.duration_ms)),
                            )
                    })
                    .collect()
            }),
        )
        .h_full()
    }

    fn empty_state(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.browse(cx)),
            )
            .child(div().text_lg().child("open a music folder"))
            .child(
                div()
                    .text_color(rgb(0x808080))
                    .child("it gets scanned into the library (flac, mp3, wav)"),
            )
    }
}

impl Render for LibraryPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let empty = self.view.is_empty();
        let body = if empty && self.busy.is_none() && self.query.is_empty() {
            self.empty_state(cx).into_any_element()
        } else {
            self.track_list(cx).into_any_element()
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(rgb(0x181818))
            .child(self.toolbar(window, cx))
            .child(div().flex_1().min_h_0().child(body))
    }
}

fn load(
    db_path: &std::path::Path,
    scan_root: Option<PathBuf>,
) -> Result<(Projection, Vec<u32>, Option<ScanSummary>), rox_library::rusqlite::Error> {
    let summary = match scan_root {
        Some(root) => {
            let mut conn = store::open(db_path)?;
            store::init_schema(&conn)?;
            Some(scanner::scan(&mut conn, &root)?)
        }
        None => None,
    };
    let shards = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let projection = Projection::load_parallel(db_path, shards)?;
    let order = projection.sort_artist_album_track();
    Ok((projection, order, summary))
}

fn status_line(total: usize, summary: Option<&ScanSummary>) -> String {
    match summary {
        Some(s) => format!(
            "{} tracks ({} indexed, {} unchanged, {} untagged)",
            total, s.indexed, s.unchanged, s.untagged
        ),
        None => format!("{total} tracks"),
    }
}

fn fmt_ms(ms: u32) -> String {
    let secs = ms / 1000;
    format!("{}:{:02}", secs / 60, secs % 60)
}
