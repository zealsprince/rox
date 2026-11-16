//! `debug.frames`, the debug scope's frame timer (ADR 22). Armed for one
//! workspace window over a span, it reports how many frames that window drew
//! and what each cost on the main thread.
//!
//! A frame is timed from the top of the workspace's render to the end of the
//! update that drew it, which is after gpui presents. That covers building,
//! layout, prepaint, paint, the renderer's swapchain acquire, encode and
//! submit, and its wait on the previous frame's GPU work, so a GPU that
//! falls behind shows up here a frame late. It can't see GPU time the CPU
//! never waits for, compositor time, or idle gaps between frames.
//!
//! Unarmed, a frame costs one thread-local read.

use std::cell::Cell;
use std::time::{Duration, Instant};

use gpui::{App, Window};
use serde_json::{Value, json};

use rox_ipc::{Request, RpcError};

const SPAN_DEFAULT_MS: u64 = 2000;
/// Under the socket's 30 s answer timeout.
const SPAN_MAX_MS: u64 = 20_000;
const SPAN_MIN_MS: u64 = 100;

#[derive(Clone, Copy)]
struct Timing {
    window: u64,
    armed: Instant,
    /// The frame being drawn, from its workspace render.
    frame: Option<Instant>,
    frames: u32,
    total: Duration,
    worst: Duration,
}

thread_local! {
    static TIMING: Cell<Option<Timing>> = const { Cell::new(None) };
}

/// Called first thing in the workspace's render.
pub(crate) fn frame_begins(window: &Window, cx: &mut App) {
    let Some(mut timing) = TIMING.get() else {
        return;
    };

    if timing.window != window.window_handle().window_id().as_u64() {
        return;
    }

    timing.frame = Some(Instant::now());
    TIMING.set(Some(timing));

    // A deferred effect runs when the outermost update flushes, and the
    // frame's update holds draw and present both. A fn item boxes nothing.
    cx.defer(frame_ends);
}

fn frame_ends(_: &mut App) {
    let Some(mut timing) = TIMING.get() else {
        return;
    };

    if let Some(began) = timing.frame.take() {
        let cost = began.elapsed();
        timing.frames += 1;
        timing.total += cost;
        timing.worst = timing.worst.max(cost);
    }

    TIMING.set(Some(timing));
}

/// `{"ms": span?, "window": id?}`. Answers once the span is over.
pub fn request(request: Request, cx: &mut App) {
    let (_, params, responder) = request.into_parts();

    let window = match workspace_window(params.get("window").and_then(Value::as_u64), cx) {
        Ok(window) => window,
        Err(e) => return responder.respond(Err(e)),
    };

    if TIMING.get().is_some() {
        return responder.respond(Err(RpcError::app("a frame timing is already running")));
    }

    let span = params
        .get("ms")
        .and_then(Value::as_u64)
        .unwrap_or(SPAN_DEFAULT_MS)
        .clamp(SPAN_MIN_MS, SPAN_MAX_MS);

    TIMING.set(Some(Timing {
        window,
        armed: Instant::now(),
        frame: None,
        frames: 0,
        total: Duration::ZERO,
        worst: Duration::ZERO,
    }));

    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(Duration::from_millis(span))
            .await;

        let report = TIMING
            .take()
            .map(|timing| report(&timing))
            .ok_or_else(|| RpcError::app("the frame timing was lost"));
        responder.respond(report);
    })
    .detach();
}

fn report(timing: &Timing) -> Value {
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let mean = match timing.frames {
        0 => 0.0,
        n => ms(timing.total) / f64::from(n),
    };

    json!({
        "window": timing.window,
        "frames": timing.frames,
        "span_ms": ms(timing.armed.elapsed()),
        "mean_ms": mean,
        "worst_ms": ms(timing.worst),
    })
}

/// The workspace window named by `id`, else the first one open. Only a
/// workspace's render carries the stamp.
fn workspace_window(id: Option<u64>, cx: &mut App) -> Result<u64, RpcError> {
    cx.default_global::<rox_panel_api::windows::WorkspaceWindows>()
        .open
        .iter()
        .map(|open| open.handle.window_id().as_u64())
        .find(|window| id.is_none_or(|id| *window == id))
        .ok_or_else(|| match id {
            Some(id) => RpcError::app(format!(
                "window {id} holds no workspace (see debug.windows)"
            )),
            None => RpcError::app("no workspace window open"),
        })
}
