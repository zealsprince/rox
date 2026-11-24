//! `debug.renderer`, the debug scope's way to start a renderer before a
//! render target can (ADR 37). Compiled into Linux debug builds only. It starts a
//! renderer through the same gate a render target will, so it approves and
//! switches on nothing, and refuses whatever the gate refuses.
//!
//! It stands in for the render target's supervisor as far as a harness run
//! needs: it holds the session, drains the renderer's stdout, and counts an
//! exit rox didn't ask for.

use std::io::{BufRead, BufReader};
use std::process::ChildStdout;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use gpui::App;
use serde_json::{Value, json};

use rox_ipc::{Request, RpcError};
use rox_services::renderers::{self, Session};

/// The stand-in mapping's size. It is never read.
const STAND_IN_BYTES: i64 = 1 << 20;

/// Sessions this verb started, by serial.
static SESSIONS: Mutex<Vec<(u64, Session)>> = Mutex::new(Vec::new());
static SERIAL: AtomicU64 = AtomicU64::new(0);

/// One of `{"list": true}`, `{"start": "<id>", "width"?, "height"?}`,
/// `{"stop": "<id>"}` or `{"status": "<id>"}`.
pub fn request(request: Request, cx: &mut App) {
    let (_, params, responder) = request.into_parts();

    // Starting may probe an interpreter, and stopping waits out a grace.
    cx.spawn(async move |cx| {
        let answer = cx
            .background_executor()
            .spawn(async move { answer(&params) })
            .await;
        responder.respond(answer);
    })
    .detach();
}

fn answer(params: &Value) -> Result<Value, RpcError> {
    if params.get("list").and_then(Value::as_bool) == Some(true) {
        let listed: Vec<Value> = renderers::all()
            .into_iter()
            .map(|r| {
                json!({
                    "id": r.id,
                    "name": r.name,
                    "label": r.label,
                    "approved": r.approved,
                    "enabled": r.enabled,
                })
            })
            .collect();
        return Ok(json!({ "renderers": listed }));
    }

    if let Some(id) = params.get("start").and_then(Value::as_str) {
        let size = |key: &str, fallback: u32| {
            params
                .get(key)
                .and_then(Value::as_u64)
                .map_or(fallback, |n| n.min(u32::MAX as u64) as u32)
        };
        return start(id, size("width", 640), size("height", 360));
    }

    if let Some(id) = params.get("stop").and_then(Value::as_str) {
        let mine: Vec<Session> = match SESSIONS.lock() {
            Ok(mut sessions) => {
                let (gone, kept) = std::mem::take(&mut *sessions)
                    .into_iter()
                    .partition(|(_, session)| session.id() == id);
                *sessions = kept;
                gone.into_iter().map(|(_, session)| session).collect()
            }
            Err(_) => Vec::new(),
        };

        let stopped = mine.len();
        for mut session in mine {
            session.stop();
        }
        return Ok(json!({ "stopped": stopped }));
    }

    if let Some(id) = params.get("status").and_then(Value::as_str) {
        let pids: Vec<u32> = SESSIONS
            .lock()
            .map(|sessions| {
                sessions
                    .iter()
                    .filter(|(_, session)| session.id() == id && !session.exited())
                    .filter_map(|(_, session)| session.pid())
                    .collect()
            })
            .unwrap_or_default();
        let due = renderers::due(id).map(|wait| wait.as_millis() as u64);

        return Ok(json!({ "running": pids, "due_ms": due }));
    }

    Err(RpcError::invalid_params(
        "renderer takes {\"list\"}, {\"start\"}, {\"stop\"} or {\"status\"}",
    ))
}

fn start(id: &str, width: u32, height: u32) -> Result<Value, RpcError> {
    // A stand-in for the frame mapping a render target makes, so there is a
    // handle to lend. Nothing reads it; the real one comes with the render
    // target, and this goes then.
    let mapping = stand_in()?;

    let mut session = {
        use std::os::fd::AsFd;
        renderers::start(id, mapping.as_fd(), width, height)
            .map_err(|refused| RpcError::app(refused.to_string()))?
    };
    drop(mapping);

    let pid = session.pid();
    let stdout = session.stdout.take();
    let serial = SERIAL.fetch_add(1, Ordering::Relaxed);

    if let Ok(mut sessions) = SESSIONS.lock() {
        sessions.push((serial, session));
    }

    if let Some(stdout) = stdout {
        let tag = id.to_string();
        let watched = std::thread::Builder::new()
            .name(format!("renderer-{id}-debug"))
            .spawn(move || watch(&tag, serial, stdout));
        if let Err(e) = watched {
            log::warn!("debug.renderer {id}: nothing watches it: {e}");
        }
    }

    Ok(json!({ "id": id, "pid": pid, "width": width, "height": height }))
}

/// Reads the renderer's announce lines until its stdout closes, then files
/// the exit and drops the session, which reaps it.
fn watch(id: &str, serial: u64, stdout: ChildStdout) {
    let mut lines = 0u64;
    for line in BufReader::new(stdout).split(b'\n') {
        if line.is_err() {
            break;
        }
        lines += 1;
    }

    let session = SESSIONS.lock().ok().and_then(|mut sessions| {
        let at = sessions.iter().position(|(held, _)| *held == serial)?;
        Some(sessions.remove(at).1)
    });

    // A stop through this verb took it out of the list already.
    let Some(session) = session else {
        return;
    };

    let deadline = Instant::now() + Duration::from_millis(500);
    while !session.exited() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }

    if !session.stopped() {
        renderers::note_exit(id);
        log::info!(
            "debug.renderer {id}: exited after {lines} lines, due {:?}",
            renderers::due(id)
        );
    }
}

fn stand_in() -> Result<std::os::fd::OwnedFd, RpcError> {
    use std::os::fd::FromRawFd;

    // SAFETY: memfd_create and ftruncate on an fd this function owns.
    unsafe {
        let fd = libc::memfd_create(c"rox-debug-renderer".as_ptr(), libc::MFD_CLOEXEC);
        if fd < 0 {
            return Err(RpcError::app(std::io::Error::last_os_error().to_string()));
        }
        let fd = std::os::fd::OwnedFd::from_raw_fd(fd);

        if libc::ftruncate(std::os::fd::AsRawFd::as_raw_fd(&fd), STAND_IN_BYTES) != 0 {
            return Err(RpcError::app(std::io::Error::last_os_error().to_string()));
        }

        Ok(fd)
    }
}
