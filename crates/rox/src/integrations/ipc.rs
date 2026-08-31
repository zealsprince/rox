//! The control socket's app side (ADR 22): rox-ipc owns the wire, this
//! module owns the answers. One bind per process at launch, an accept
//! machinery contained entirely in the crate, and a drain here on the
//! foreground executor, the same marshalling the tray, the media keys, and
//! the single-instance guard use to get onto the UI thread. Every method
//! reads or drives the same player and library entities the panels do, so
//! the socket can never say something the UI wouldn't.
//!
//! Method surface, version 1: `transport.*` for the deck, `queue.*` for
//! edits by stable entry id, `library.*` for search, now-playing tags,
//! artwork, and kicking off a rescan, `tasks.*` for the long analysis
//! passes the tasks window runs. `subscribe` turns on the push half: `event.*` frames for track
//! turnover, play-state edges, and queue revision bumps, published off the
//! player observer below so a front end never has to poll. The `debug.*`
//! scope is the runtime test surface: the settings and panel dumps here,
//! and the drive half (windows, actions, synthetic input) in the sibling
//! `drive` module.

use std::path::PathBuf;

use gpui::App;
use serde_json::{json, Value};

use rox_ipc::{Request, RpcError};
use rox_library::cue::TrackKey;
use rox_library::projection::Projection;
use rox_panel_api::panel::AppState;
use rox_services::catalog::Library;

/// How many search rows come back when the caller doesn't say, and the most
/// it can ask for. Each row costs a path lookup on the UI-side connection,
/// so the cap keeps one greedy query from holding the thread.
const SEARCH_LIMIT_DEFAULT: usize = 50;
const SEARCH_LIMIT_MAX: usize = 500;

/// Bind the control socket and start answering. Failure to bind (another
/// instance, an unwritable runtime dir, a platform with no backend yet) logs
/// and returns: rox runs on without the surface.
pub fn serve(state: &AppState, cx: &mut App) {
    let path = rox_ipc::socket_path(&rox_core::settings::data_dir());
    let server = match rox_ipc::Server::bind(&path) {
        Ok(server) => server,
        Err(err) => {
            log::warn!("control socket: not serving: {err}");
            return;
        }
    };
    log::info!("control socket: listening at {}", path.display());
    let (requests, events, cleanup) = server.spawn();
    publish_events(state, events, cx);
    let state = state.clone();
    cx.spawn(async move |cx| {
        while let Ok(request) = requests.recv().await {
            if cx.update(|cx| dispatch(&state, request, cx)).is_err() {
                break;
            }
        }
    })
    .detach();
    cx.on_app_quit(move |_| {
        let cleanup = cleanup.clone();
        async move { cleanup.remove() }
    })
    .detach();
}

/// The slice of player state whose edges become events.
struct Snapshot {
    playing: bool,
    active: bool,
    volume: f32,
    muted: bool,
    queue_rev: Option<u64>,
    track: Option<TrackKey>,
}

impl Snapshot {
    fn take(state: &AppState, cx: &App) -> Snapshot {
        let player = state.player.read(cx);
        Snapshot {
            playing: player.is_playing(),
            active: player.is_active(),
            volume: player.volume(),
            muted: player.muted(),
            queue_rev: player.queue_rev(),
            track: player.now_playing().map(|now| now.key),
        }
    }
}

/// Publish `event.*` frames to subscribed connections off the player
/// observer, the same wake the media widget publishes on: the player pump
/// already notifies on exactly the edges the contract names (play-state
/// flips, track turnover, queue revision bumps), so this diffs a small
/// snapshot and emits only when something moved. While audio plays the
/// pump notifies every tick for the clock; the diff makes those free, and
/// the emit itself never blocks (a consumer that can't keep up is cut off
/// in the crate, not waited on here).
fn publish_events(state: &AppState, events: rox_ipc::Events, cx: &mut App) {
    let state = state.clone();
    let player = state.player.clone();
    let mut seen = Snapshot::take(&state, cx);
    cx.observe(&player, move |_, cx| {
        let now = Snapshot::take(&state, cx);
        if now.track != seen.track {
            events.emit("event.track", now_playing(&state, cx));
        }
        if (now.playing, now.active, now.muted) != (seen.playing, seen.active, seen.muted)
            || now.volume != seen.volume
        {
            events.emit("event.playback", status(&state, cx));
        }
        if now.queue_rev != seen.queue_rev {
            events.emit("event.queue", json!({ "queue_rev": now.queue_rev }));
        }
        seen = now;
    })
    .detach();
}

/// Route one request. The blocking answers (art reads, the search scan)
/// leave for the background executor with their responder; everything else
/// responds right here.
fn dispatch(state: &AppState, request: Request, cx: &mut App) {
    match request.method.as_str() {
        "library.search" => return search(state, request, cx),
        "library.artwork" => return artwork(request, cx),
        _ => {}
    }
    let result = route(state, &request.method, &request.params, cx);
    request.respond(result);
}

fn route(state: &AppState, method: &str, params: &Value, cx: &mut App) -> Result<Value, RpcError> {
    match method {
        "transport.status" => Ok(status(state, cx)),
        "transport.toggle" => {
            state.player.read(cx).toggle_pause();
            Ok(status(state, cx))
        }
        "transport.play" => {
            let player = state.player.read(cx);
            if !player.is_playing() {
                player.toggle_pause();
            }
            Ok(status(state, cx))
        }
        "transport.pause" => {
            let player = state.player.read(cx);
            if player.is_playing() {
                player.toggle_pause();
            }
            Ok(status(state, cx))
        }
        "transport.next" => {
            state.player.update(cx, |player, cx| player.next(cx));
            Ok(status(state, cx))
        }
        "transport.prev" => {
            state.player.read(cx).prev();
            Ok(status(state, cx))
        }
        "transport.stop" => {
            state.player.update(cx, |player, cx| player.stop(cx));
            Ok(status(state, cx))
        }
        "transport.seek" => {
            let player = state.player.read(cx);
            if let Some(to) = params.get("to").and_then(Value::as_f64) {
                player.seek_to(to);
            } else if let Some(by) = params.get("by").and_then(Value::as_f64) {
                player.seek_by(by);
            } else {
                return Err(RpcError::invalid_params(
                    "seek takes {\"to\": seconds} or {\"by\": seconds}",
                ));
            }
            Ok(status(state, cx))
        }
        "transport.set_volume" => {
            let volume = params
                .get("volume")
                .and_then(Value::as_f64)
                .ok_or_else(|| RpcError::invalid_params("set_volume takes {\"volume\": 0..2}"))?;
            state
                .player
                .update(cx, |player, cx| player.set_volume(volume as f32, cx));
            Ok(status(state, cx))
        }
        "queue.list" => Ok(queue_list(state, cx)),
        "queue.add" => queue_add(state, params, cx),
        "queue.remove" => {
            let ids = id_list(params)?;
            state.player.read(cx).remove_many_from_queue(ids);
            Ok(Value::Null)
        }
        "queue.move" => {
            let id = params
                .get("id")
                .and_then(Value::as_u64)
                .ok_or_else(|| RpcError::invalid_params("move takes {\"id\", \"after\"?}"))?;
            let after = params.get("after").and_then(Value::as_u64);
            state.player.read(cx).move_in_queue(id, after);
            Ok(Value::Null)
        }
        "queue.jump" => {
            let id = params
                .get("id")
                .and_then(Value::as_u64)
                .ok_or_else(|| RpcError::invalid_params("jump takes {\"id\"}"))?;
            state.player.read(cx).jump_to(id);
            Ok(Value::Null)
        }
        "library.now_playing" => Ok(now_playing(state, cx)),
        // Kick off the same rescan as the menubar's refresh button. The scan
        // runs in the background; the reply only says it started. Refused in
        // a sentence while other background work holds the library, since
        // rescan() would silently no-op and the caller would wait forever.
        "library.rescan" => {
            let library = state.library.read(cx);
            if let Some(busy) = library.busy() {
                return Err(RpcError::app(format!("the library is busy: {busy}")));
            }
            if !library.can_rescan() {
                return Err(RpcError::app(
                    "no library folders to rescan; open one in rox first",
                ));
            }
            state.library.update(cx, |library, cx| library.rescan(cx));
            Ok(json!({ "started": true }))
        }
        "tasks.status" => Ok(tasks_status(state, cx)),
        "tasks.start" => tasks_start(state, params, cx),
        "tasks.stop" => tasks_stop(params, cx),
        // What rox-mcp asks before serving a tool call: with the AI gate or
        // the MCP page's own switch off it turns clients away in a sentence
        // instead of hanging or pretending (ADR 22). The socket itself stays
        // up either way; the toggles gate what talks to AI tooling, not the
        // control surface.
        "ai.status" => {
            let settings = rox_core::settings::Settings::load();
            Ok(json!({
                "enabled": settings.ai_enabled,
                "mcp": settings.mcp_enabled,
            }))
        }
        "debug.settings" => {
            serde_json::to_value(rox_core::settings::Settings::load()).map_err(RpcError::app)
        }
        // The artwork service's cumulative counters: requests served,
        // loads started/landed/refused, current pool depth and cache
        // size. Never reset, so two samples a few seconds apart and their
        // difference is the rate; a scroll that stalls shows here as
        // requests still climbing while lands stall, or as requests
        // themselves going flat.
        "debug.thumbs" => {
            serde_json::to_value(state.thumbs.read(cx).stats()).map_err(RpcError::app)
        }
        "debug.panels" => panel_tree(cx),
        // Apply a workspace to the front window by name, the welcome tiles'
        // path. Debug scope: lets a script step through looks without
        // driving the settings window's dialog (ADR 22).
        "debug.workspace" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| RpcError::invalid_params("workspace takes {\"name\"}"))?;
            if crate::workspaces::resolve(name).is_none() {
                return Err(RpcError::app(format!("no workspace named {name}")));
            }
            crate::workspace::apply_workspace_to_front(name, cx);
            Ok(Value::Null)
        }
        // Live gpui entity counts by type, largest first. Diagnostic surface
        // for leak hunting; rides the vendored entity-map accessor.
        "debug.entities" => {
            let counts = cx.entity_diagnostic_counts();
            let total: usize = counts.iter().map(|(_, n)| n).sum();
            Ok(json!({
                "total": total,
                "by_type": counts
                    .into_iter()
                    .map(|(name, n)| json!([name, n]))
                    .collect::<Vec<_>>(),
            }))
        }
        other => super::drive::route(other, params, cx)
            .unwrap_or_else(|| Err(RpcError::method_not_found(other))),
    }
}

/// The pass a tasks method names, or a sentence listing what it takes.
fn pass_param(params: &Value) -> Result<&str, RpcError> {
    match params.get("pass").and_then(Value::as_str) {
        Some(pass @ ("acoustic" | "replaygain" | "tempo")) => Ok(pass),
        _ => Err(RpcError::invalid_params(
            "takes {\"pass\": \"acoustic\" | \"replaygain\" | \"tempo\"}",
        )),
    }
}

/// One pass's row: whether it could start, what it would work through, and
/// live progress while it runs. `progress` arrives pre-serialized because
/// the three passes each have their own Progress type.
fn pass_json(enabled: bool, missing: u64, progress: Option<Value>) -> Value {
    match progress {
        Some(Value::Object(mut fields)) => {
            fields.insert("running".into(), json!(true));
            fields.insert("enabled".into(), json!(enabled));
            fields.insert("missing".into(), json!(missing));
            Value::Object(fields)
        }
        _ => json!({ "running": false, "enabled": enabled, "missing": missing }),
    }
}

/// The three long passes the tasks window lists, one row each. The library
/// scan isn't in here: it has its own surface in `library.rescan` and the
/// menubar, the same split the window's badge makes.
fn tasks_status(state: &AppState, cx: &App) -> Value {
    let settings = rox_core::settings::Settings::load();
    let source = rox_services::acoustic::acoustic_source();
    let library = state.library.read(cx);
    json!({
        "acoustic": pass_json(
            settings.acoustic_analysis,
            library.acoustic_coverage(source.id()).missing() as u64,
            crate::embeddings::progress(cx).map(|p| json!({
                "done": p.done(), "total": p.total(), "failed": p.failed(),
                "current": p.current(), "stopping": p.stopping(), "eta_secs": p.eta_secs(),
            })),
        ),
        "replaygain": pass_json(
            true,
            library.replaygain_breakdown().missing,
            crate::replaygain_job::progress(cx).map(|p| json!({
                "done": p.done(), "total": p.total(), "failed": p.failed(),
                "current": p.current(), "stopping": p.stopping(), "eta_secs": p.eta_secs(),
            })),
        ),
        "tempo": pass_json(
            settings.tempo_analysis,
            library.bpm_breakdown().missing,
            crate::tempo_job::progress(cx).map(|p| json!({
                "done": p.done(), "total": p.total(), "failed": p.failed(),
                "current": p.current(), "stopping": p.stopping(), "eta_secs": p.eta_secs(),
            })),
        ),
    })
}

/// Start one of the long passes, the tasks window's start button without
/// its prompt. The UI never starts these on a bare press, so the reply
/// carries what the prompt would have shown: the track count, the worker
/// count, the estimate where this machine has a pace, and the save mode,
/// which matters because tags mode rewrites audio files. The silent no-op
/// guards inside the passes become sentences here, same as the rescan.
fn tasks_start(state: &AppState, params: &Value, cx: &mut App) -> Result<Value, RpcError> {
    let pass = pass_param(params)?;
    let settings = rox_core::settings::Settings::load();
    let library = state.library.clone();
    let (missing, workers, pace, detail) = match pass {
        "acoustic" => {
            if crate::embeddings::progress(cx).is_some() {
                return Err(RpcError::app("the acoustic pass is already running"));
            }
            if !settings.acoustic_analysis {
                return Err(RpcError::app(
                    "acoustic analysis is switched off; turn on \"Describe How Tracks \
                     Sound\" on Settings > Library first",
                ));
            }
            let source = rox_services::acoustic::acoustic_source();
            let missing = library.read(cx).acoustic_coverage(source.id()).missing() as u64;
            let pace = settings
                .session
                .acoustic_pace
                .get(source.id())
                .copied()
                .unwrap_or_default();
            let detail = json!({ "model": source.label(), "save": settings.acoustic_save });
            crate::embeddings::start(library, cx);
            (missing, settings.acoustic_workers.max(1), pace, detail)
        }
        "replaygain" => {
            if crate::replaygain_job::progress(cx).is_some() {
                return Err(RpcError::app("the ReplayGain pass is already running"));
            }
            let missing = library.read(cx).replaygain_breakdown().missing;
            let detail = json!({ "save": settings.replay_gain.save });
            crate::replaygain_job::start(library, cx);
            (
                missing,
                settings.replaygain_workers.max(1),
                settings.session.replaygain_pace,
                detail,
            )
        }
        _ => {
            if crate::tempo_job::progress(cx).is_some() {
                return Err(RpcError::app("the tempo pass is already running"));
            }
            if !settings.tempo_analysis {
                return Err(RpcError::app(
                    "tempo analysis is switched off; turn on \"Work Out How Fast Tracks \
                     Run\" on Settings > Library first",
                ));
            }
            let missing = library.read(cx).bpm_breakdown().missing;
            crate::tempo_job::start(library, cx);
            (
                missing,
                settings.tempo_workers.max(1),
                settings.session.tempo_pace,
                json!({}),
            )
        }
    };
    let mut reply = json!({
        "started": true,
        "missing": missing,
        "workers": workers,
        "estimate": rox_core::pace::estimate(pace, missing, workers),
    });
    if let (Value::Object(reply), Value::Object(detail)) = (&mut reply, detail) {
        reply.extend(detail);
    }
    Ok(reply)
}

/// Ask a running pass to stop. Graceful the way the stop button is: the
/// workers drop out at the next file, so "stopping" rather than "stopped".
fn tasks_stop(params: &Value, cx: &mut App) -> Result<Value, RpcError> {
    let pass = pass_param(params)?;
    let running = match pass {
        "acoustic" => crate::embeddings::progress(cx).is_some(),
        "replaygain" => crate::replaygain_job::progress(cx).is_some(),
        _ => crate::tempo_job::progress(cx).is_some(),
    };
    if !running {
        return Err(RpcError::app(format!("the {pass} pass isn't running")));
    }
    match pass {
        "acoustic" => crate::embeddings::stop(cx),
        "replaygain" => crate::replaygain_job::stop(cx),
        _ => crate::tempo_job::stop(cx),
    }
    Ok(json!({ "stopping": true }))
}

/// The frontmost workspace's dock tree, the same dump the layout persist
/// writes. Debug scope: no external consumer needs it, but it lets a script
/// or an agent verify a layout against a live instance without eyes on the
/// screen (ADR 22).
fn panel_tree(cx: &mut App) -> Result<Value, RpcError> {
    let workspace = cx
        .default_global::<rox_panel_api::windows::WorkspaceWindows>()
        .open
        .iter()
        .find_map(|open| open.workspace.upgrade())
        .and_then(|any| any.downcast::<crate::workspace::Workspace>().ok())
        .ok_or_else(|| RpcError::app("no workspace window open"))?;
    let dump = workspace.read(cx).dock().read(cx).dump(cx);
    serde_json::to_value(dump).map_err(RpcError::app)
}

/// The deck at a glance: what's playing, where its clock is, and the
/// queue revision an event consumer will later diff against. Every
/// transport verb replies with this, so a caller sees what its command did
/// without a second round trip.
fn status(state: &AppState, cx: &App) -> Value {
    let player = state.player.read(cx);
    let now = player.now_playing();
    let track = now.as_ref().map(|now| {
        let tags = state.library.read(cx).meta_for_key(&now.key);
        track_json(&now.key, tags.as_ref())
    });
    json!({
        "playing": player.is_playing(),
        "active": player.is_active(),
        "position_secs": now.as_ref().map(|n| n.position_secs),
        "duration_secs": now.as_ref().and_then(|n| n.duration_secs),
        "volume": player.volume(),
        "muted": player.muted(),
        "queue_rev": player.queue_rev(),
        "track": track,
    })
}

/// The playing track's full tags, or null while nothing plays.
fn now_playing(state: &AppState, cx: &App) -> Value {
    let Some(now) = state.player.read(cx).now_playing() else {
        return Value::Null;
    };
    let tags = state.library.read(cx).meta_for_key(&now.key);
    let mut track = track_json(&now.key, tags.as_ref());
    track["position_secs"] = json!(now.position_secs);
    track["duration_secs"] = json!(now.duration_secs);
    track
}

/// One track as the wire shows it: the key that names it, and the library's
/// tags where it has a row. The filename stands in for a missing title the
/// same way the media widget's card does.
fn track_json(key: &TrackKey, tags: Option<&rox_library::store::TrackMeta>) -> Value {
    let fallback_title = || {
        key.path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    json!({
        "path": key.path,
        "sub": key.sub,
        "title": tags
            .map(|t| t.title.clone())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(fallback_title),
        "artist": tags.map(|t| t.artist.clone()).unwrap_or_default(),
        "album": tags.map(|t| t.album.clone()).unwrap_or_default(),
        "album_artist": tags.map(|t| t.album_artist.clone()).unwrap_or_default(),
        "genre": tags.map(|t| t.genre.clone()).unwrap_or_default(),
        "year": tags.map(|t| t.year),
        "track_no": tags.map(|t| t.track_no),
        "duration_ms": tags.map(|t| t.duration_ms),
        "codec": tags.map(|t| t.codec.clone()).unwrap_or_default(),
        "rating": tags.map(|t| t.rating),
    })
}

/// The whole play order with the handles an edit needs: stable ids for
/// remove, move, and jump, the explicit flag the queue widgets split on,
/// and which entry is audible.
fn queue_list(state: &AppState, cx: &App) -> Value {
    let player = state.player.read(cx);
    let Some((entries, cursor)) = player.play_order() else {
        return json!({ "entries": [], "cursor": null, "queue_rev": player.queue_rev() });
    };
    let entries: Vec<Value> = entries
        .iter()
        .enumerate()
        .map(|(i, (id, key, explicit))| {
            json!({
                "id": id,
                "path": key.path,
                "sub": key.sub,
                "explicit": explicit,
                "current": i == cursor,
            })
        })
        .collect();
    json!({
        "entries": entries,
        "cursor": cursor,
        "queue_rev": player.queue_rev(),
    })
}

/// Queue files by path. `mode` places them: "end" (the default) behind
/// what's queued, "next" right after the playing track, "now" splices and
/// jumps. Paths are filtered to decodable audio the same way an OS file
/// open is; a `path#N` string names a cue track the way the m3u export
/// does. Returns how many made the cut.
fn queue_add(state: &AppState, params: &Value, cx: &mut App) -> Result<Value, RpcError> {
    let paths = params
        .get("paths")
        .and_then(Value::as_array)
        .ok_or_else(|| RpcError::invalid_params("add takes {\"paths\": [..], \"mode\"?}"))?;
    let mut keys = Vec::new();
    for path in paths {
        let Some(s) = path.as_str() else {
            return Err(RpcError::invalid_params("paths are strings"));
        };
        let key = TrackKey::from_fragment(s, |p| std::path::Path::new(p).is_file());
        if key.sub > 0 {
            // A cue track names a slice, not a file to sniff; the engine
            // resolves its span off the library at insert.
            keys.push(key);
            continue;
        }
        keys.extend(
            rox_library::open_files::resolve_audio_paths([PathBuf::from(s)])
                .into_iter()
                .map(TrackKey::from),
        );
    }
    if keys.is_empty() {
        return Err(RpcError::app("no playable files in the batch"));
    }
    let queued = keys.len();
    let mode = params.get("mode").and_then(Value::as_str).unwrap_or("end");
    state.player.update(cx, |player, cx| {
        match mode {
            "end" => player.enqueue(keys, cx),
            "next" => player.play_next(keys, cx),
            "now" => player.play_now(keys, cx),
            other => {
                return Err(RpcError::invalid_params(format!(
                    "unknown mode {other:?}: end, next, or now"
                )))
            }
        }
        Ok(())
    })?;
    Ok(json!({ "queued": queued }))
}

/// The ids a batch edit names, from `{"ids": [..]}` or a single `{"id": ..}`.
fn id_list(params: &Value) -> Result<Vec<u64>, RpcError> {
    if let Some(id) = params.get("id").and_then(Value::as_u64) {
        return Ok(vec![id]);
    }
    params
        .get("ids")
        .and_then(Value::as_array)
        .map(|ids| ids.iter().filter_map(Value::as_u64).collect())
        .filter(|ids: &Vec<u64>| !ids.is_empty())
        .ok_or_else(|| RpcError::invalid_params("remove takes {\"ids\": [..]}"))
}

/// Search the library off the projection, the same scan the panels run. The
/// scan itself is rayon-parallel and proportional to the library, so it
/// leaves for the background executor with the responder; only the id-to-
/// path resolve comes back to the UI side, bounded by the row cap.
fn search(state: &AppState, request: Request, cx: &mut App) {
    let (_, params, responder) = request.into_parts();
    let Some(query) = params.get("query").and_then(Value::as_str) else {
        responder.respond(Err(RpcError::invalid_params(
            "search takes {\"query\": \"..\", \"limit\"?}",
        )));
        return;
    };
    let Some(projection) = state.library.read(cx).projection().cloned() else {
        responder.respond(Err(RpcError::app("no library loaded")));
        return;
    };
    let query = query.to_owned();
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .map(|l| l as usize)
        .unwrap_or(SEARCH_LIMIT_DEFAULT)
        .min(SEARCH_LIMIT_MAX);
    let library = state.library.clone();
    cx.spawn(async move |cx| {
        let scan = projection.clone();
        let scanned = cx
            .background_executor()
            .spawn(async move {
                let query = query;
                scan.search(&query)
            })
            .await;
        let total = scanned.len();
        let rows: Vec<u32> = scanned.into_iter().take(limit).collect();
        let result = cx.update(|cx| {
            let library = library.read(cx);
            json!({
                "total": total,
                "tracks": rows
                    .iter()
                    .map(|&row| search_row(&projection, row, library))
                    .collect::<Vec<Value>>(),
            })
        });
        match result {
            Ok(result) => responder.respond(Ok(result)),
            Err(_) => responder.respond(Err(RpcError::app("rox is shutting down"))),
        }
    })
    .detach();
}

/// One search hit: the projection row's tags plus the key that plays it,
/// resolved through the same id the rating writes use.
fn search_row(projection: &Projection, row: u32, library: &Library) -> Value {
    let view = projection.resolve(row);
    let id = projection.db_id[row as usize];
    let key = library.keys_for(&[id]).ok().and_then(|mut keys| keys.pop());
    json!({
        "id": id,
        "path": key.as_ref().map(|k| k.path.clone()),
        "sub": key.as_ref().map(|k| k.sub),
        "title": view.title,
        "artist": view.artist,
        "album_artist": view.album_artist,
        "album": view.album,
        "genre": view.genre,
        "year": view.year,
        "disc_no": view.disc_no,
        "track_no": view.track_no,
        "duration_ms": view.duration_ms,
        "codec": view.codec,
        "rating": view.rating,
        "plays": view.plays,
    })
}

/// Cover art by path, read off the background executor: the same embedded-
/// tag-then-folder resolve the media widget uses, handed back as base64
/// with its mime beside it.
fn artwork(request: Request, cx: &mut App) {
    use base64::Engine as _;

    let (_, params, responder) = request.into_parts();
    let Some(path) = params.get("path").and_then(Value::as_str) else {
        responder.respond(Err(RpcError::invalid_params(
            "artwork takes {\"path\": \"..\"}",
        )));
        return;
    };
    let path = PathBuf::from(path);
    cx.background_executor()
        .spawn(async move {
            match rox_library::art::cover_art(&path) {
                Some((bytes, mime)) => responder.respond(Ok(json!({
                    "mime": mime,
                    "data_base64": base64::engine::general_purpose::STANDARD.encode(bytes),
                }))),
                None => responder.respond(Err(RpcError::app("no artwork for that path"))),
            }
        })
        .detach();
}
