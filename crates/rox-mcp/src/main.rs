//! rox-mcp: the MCP face of the control socket (ADR 22). MCP clients spawn
//! stdio servers, which a GUI can't be, so this thin binary proxies MCP over
//! stdio to the socket. Every tool is one socket method.
//!
//! Gated by "Enable AI Features" and "Enable MCP Server", checked on every
//! call. `--dev` adds the ui_ drive tools over the socket's debug scope; it's a
//! flag on the spawning config so a music-facing setup never carries them.

use std::io::{BufRead as _, Write as _};
use std::path::PathBuf;

use serde_json::{Value, json};

use rox_ipc::client::Client;

/// Offered when the client asks for a revision we don't recognize.
const MCP_VERSION: &str = "2025-06-18";

/// Answered verbatim: the tools surface is unchanged across them.
const MCP_KNOWN: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18"];

fn main() {
    let mut socket: Option<PathBuf> = None;
    let mut data_dir: Option<PathBuf> = None;
    let mut dev = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--socket" => socket = args.next().map(PathBuf::from),
            "--data-dir" => data_dir = args.next().map(PathBuf::from),
            "--dev" => dev = true,
            other => {
                eprintln!(
                    "rox-mcp: unknown argument {other}; takes --socket, --data-dir, or --dev"
                );
                std::process::exit(2);
            }
        }
    }
    let socket = socket.unwrap_or_else(|| {
        let data_dir = data_dir.unwrap_or_else(|| {
            dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("rox")
        });
        rox_ipc::socket_path(&data_dir)
    });

    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let mut rox: Option<Client> = None;
    while let Some(Ok(line)) = lines.next() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(frame) = serde_json::from_str::<Value>(&line) else {
            respond(json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": { "code": -32700, "message": "parse error" },
            }));
            continue;
        };
        let Some(id) = frame.get("id").filter(|id| !id.is_null()).cloned() else {
            continue;
        };
        let method = frame.get("method").and_then(Value::as_str).unwrap_or("");
        let params = frame.get("params").cloned().unwrap_or(Value::Null);
        let body = match method {
            "initialize" => Ok(initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools(dev) })),
            "tools/call" => Ok(call(&mut rox, &socket, &params, dev)),
            other => Err(json!({
                "code": -32601,
                "message": format!("method not found: {other}"),
            })),
        };
        respond(match body {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
        });
    }
}

fn respond(frame: Value) {
    let mut stdout = std::io::stdout().lock();
    let _ = serde_json::to_writer(&mut stdout, &frame);
    let _ = stdout.write_all(b"\n");
    let _ = stdout.flush();
}

fn initialize(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = match asked {
        Some(v) if MCP_KNOWN.contains(&v) => v,
        _ => MCP_VERSION,
    };
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": "rox",
            "version": env!("CARGO_PKG_VERSION"),
        },
    })
}

fn tools(dev: bool) -> Value {
    let mut tools = base_tools();
    if dev && let (Value::Array(all), Value::Array(extra)) = (&mut tools, dev_tools()) {
        all.extend(extra);
    }
    tools
}

fn base_tools() -> Value {
    json!([
        {
            "name": "now_playing",
            "description": "What rox is playing right now: the track's tags, where its \
                            clock sits, and whether audio is moving. On a radio station \
                            live is true and there is no position to speak of: the clock \
                            counts the listen, and shift says where the playhead sits \
                            against the broadcast (behind_secs from the live edge, \
                            window_secs of buffer taped so far, cap_secs the buffer's \
                            size). shift is null for a file.",
            "inputSchema": { "type": "object", "properties": {} },
        },
        {
            "name": "transport",
            "description": "Drive playback: toggle, play, pause, next, prev, or stop. \
                            Answers with the resulting player state.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["toggle", "play", "pause", "next", "prev", "stop"],
                    },
                },
                "required": ["action"],
            },
        },
        {
            "name": "ab_repeat",
            "description": "Repeat a section of the playing track. Actions: mark steps \
                            the three-press cycle (mark A at the current position, then \
                            B and start repeating, then clear); clear drops the section; \
                            set takes a and b in track seconds, at least a quarter second \
                            apart. Answers with the player state, whose ab field holds the \
                            section.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["mark", "clear", "set"] },
                    "a": { "type": "number", "minimum": 0 },
                    "b": { "type": "number", "minimum": 0 },
                },
                "required": ["action"],
            },
        },
        {
            "name": "search_library",
            "description": "Search the music library. Free terms match title, artist, \
                            album artist, album, and genre; a field: prefix narrows to \
                            one, as in artist:name or year:1990. Fields: title, artist, \
                            albumartist, album, genre, year, folder, codec, rating, \
                            plays, added. Each hit carries a key field, the string \
                            add_to_queue takes to queue that exact track. Radio \
                            stations match by name and come last, with source set to \
                            radio, no artist or album, and duration_ms 0, since a \
                            stream has no length.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 500 },
                },
                "required": ["query"],
            },
        },
        {
            "name": "get_queue",
            "description": "The play order: every queued entry with its stable id, \
                            path, and whether it is the one playing.",
            "inputSchema": { "type": "object", "properties": {} },
        },
        {
            "name": "add_to_queue",
            "description": "Queue tracks. Each item is a file or folder path, a radio \
                            station's stream URL, or a source|path key as search_library \
                            prints one in its key field, which is the only thing that \
                            names a track on a Subsonic server or a station uniquely. \
                            mode places them: end behind what is queued, next right \
                            after the playing track, now splices and starts playing. \
                            A stream URL or a source key with no row in the library is \
                            refused; nothing here can fetch one.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "items": {
                        "type": "array",
                        "items": { "type": "string" },
                        "minItems": 1,
                    },
                    "mode": { "type": "string", "enum": ["end", "next", "now"] },
                },
                "required": ["items"],
            },
        },
        {
            "name": "rescan_library",
            "description": "Rescan the library folders for new, changed, and removed \
                            files. The scan runs in the background; searches pick up \
                            its results as they land.",
            "inputSchema": { "type": "object", "properties": {} },
        },
        {
            "name": "get_tasks",
            "description": "The long library passes (acoustic, ReplayGain, tempo, \
                            sort names, romanize): whether each could start, how \
                            much it would work through, and live progress while one \
                            runs.",
            "inputSchema": { "type": "object", "properties": {} },
        },
        {
            "name": "start_task",
            "description": "Start a long analysis pass over the library. These cost \
                            hours on a large library, and in tags save mode rewrite \
                            audio files; the answer says what the pass took on, with \
                            an estimate where this machine knows its pace. The \
                            sortnames pass writes no files at all: it asks \
                            MusicBrainz what each artist files under and stores the \
                            answer in the library, over the non-Latin names only. \
                            The romanize pass writes no files either and runs at \
                            disk speed: it reads every non-Latin title, album and \
                            artist that still has no sort name and stores a Latin \
                            spelling. Korean, Chinese and kana need nothing; kanji \
                            values are skipped unless the optional Japanese \
                            dictionary is installed, and get_tasks reports how \
                            many that is.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pass": { "type": "string", "enum": ["acoustic", "replaygain", "tempo", "sortnames", "romanize"] },
                },
                "required": ["pass"],
            },
        },
        {
            "name": "stop_task",
            "description": "Ask a running analysis pass to stop. Graceful: the \
                            workers drop out at the next file, keeping what's done.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pass": { "type": "string", "enum": ["acoustic", "replaygain", "tempo", "sortnames", "romanize"] },
                },
                "required": ["pass"],
            },
        },
    ])
}

/// Coordinates are window-local logical pixels; every tool takes an optional
/// window id and defaults to the active window.
fn dev_tools() -> Value {
    let window = json!({ "type": "integer", "description": "Window id from ui_windows; defaults to the active window." });
    let coord = json!({ "type": "number", "description": "Window-local logical pixels." });
    json!([
        {
            "name": "ui_windows",
            "description": "Open windows: id, title, size in logical pixels, scale, \
                            and which is active. Ids feed the other ui_ tools.",
            "inputSchema": { "type": "object", "properties": {} },
        },
        {
            "name": "ui_panels",
            "description": "The frontmost workspace's dock tree: which panels are \
                            open, where, and how they split.",
            "inputSchema": { "type": "object", "properties": {} },
        },
        {
            "name": "ui_actions",
            "description": "Dispatchable action names, optionally narrowed by a \
                            substring filter.",
            "inputSchema": {
                "type": "object",
                "properties": { "filter": { "type": "string" } },
            },
        },
        {
            "name": "ui_action",
            "description": "Dispatch an action by name down a window's focus chain, \
                            exactly as its keybinding would.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "data": { "description": "Payload for actions that carry one, as a keymap entry would." },
                    "window": window,
                },
                "required": ["name"],
            },
        },
        {
            "name": "ui_key",
            "description": "Send keystrokes in gpui keymap syntax, space separated: \
                            \"ctrl-comma\", \"escape\", \"cmd-shift-p enter\". \
                            Answers per stroke with whether anything handled it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "keys": { "type": "string" },
                    "window": window,
                },
                "required": ["keys"],
            },
        },
        {
            "name": "ui_type",
            "description": "Type text into whatever holds focus. Newlines land as \
                            enter.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": { "type": "string" },
                    "window": window,
                },
                "required": ["text"],
            },
        },
        {
            "name": "ui_click",
            "description": "Click at window-local logical coordinates. count 2 or 3 \
                            makes it a double or triple click; modifiers like \
                            {\"ctrl\": true} ride along for modified clicks.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "x": coord, "y": coord,
                    "button": { "type": "string", "enum": ["left", "right", "middle"] },
                    "count": { "type": "integer", "minimum": 1, "maximum": 3 },
                    "modifiers": { "type": "object" },
                    "window": window,
                },
                "required": ["x", "y"],
            },
        },
        {
            "name": "ui_hover",
            "description": "Move the mouse to a point without pressing anything, for \
                            hover styles and tooltips.",
            "inputSchema": {
                "type": "object",
                "properties": { "x": coord, "y": coord, "window": window },
                "required": ["x", "y"],
            },
        },
        {
            "name": "ui_scroll",
            "description": "Scroll at a point. dx/dy are wheel lines, positive y \
                            scrolling content up as a wheel-up does.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "x": coord, "y": coord,
                    "dx": { "type": "number" }, "dy": { "type": "number" },
                    "window": window,
                },
                "required": ["x", "y"],
            },
        },
    ])
}

/// Tool-level failures come back as isError results, where MCP expects them;
/// only malformed requests earn protocol errors.
fn call(rox: &mut Option<Client>, socket: &std::path::Path, params: &Value, dev: bool) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    // The socket method validates the drive tools' arguments.
    if let Some(rest) = name.strip_prefix("ui_") {
        if !dev {
            return refusal(&format!(
                "no such tool: {name} (the ui_ tools need rox-mcp started with --dev)"
            ));
        }
        let method = match rest {
            "windows" => "debug.windows",
            "panels" => "debug.panels",
            "actions" => "debug.actions",
            "action" => "debug.action",
            "key" => "debug.key",
            "type" => "debug.type",
            "click" => "debug.click",
            "hover" => "debug.hover",
            "scroll" => "debug.scroll",
            _ => return refusal(&format!("no such tool: {name}")),
        };
        return match proxy(rox, socket, method, args) {
            Ok(result) => json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string_pretty(&result).unwrap_or_default(),
                }],
            }),
            Err(reason) => refusal(&reason),
        };
    }
    let (method, params) = match name {
        "now_playing" => ("transport.status", json!({})),
        "transport" => match args.get("action").and_then(Value::as_str) {
            Some("toggle") => ("transport.toggle", json!({})),
            Some("play") => ("transport.play", json!({})),
            Some("pause") => ("transport.pause", json!({})),
            Some("next") => ("transport.next", json!({})),
            Some("prev") => ("transport.prev", json!({})),
            Some("stop") => ("transport.stop", json!({})),
            _ => {
                return refusal(
                    "transport takes an action: toggle, play, pause, next, prev, or stop",
                );
            }
        },
        "ab_repeat" => match args.get("action").and_then(Value::as_str) {
            Some("mark") => ("transport.ab", json!({ "action": "mark" })),
            Some("clear") => ("transport.ab", json!({ "action": "clear" })),
            Some("set") => match (
                args.get("a").and_then(Value::as_f64),
                args.get("b").and_then(Value::as_f64),
            ) {
                (Some(a), Some(b)) => ("transport.ab", json!({ "a": a, "b": b })),
                _ => return refusal("ab_repeat set takes a and b in track seconds"),
            },
            _ => return refusal("ab_repeat takes an action: mark, clear, or set"),
        },
        "search_library" => {
            let Some(query) = args.get("query").and_then(Value::as_str) else {
                return refusal("search_library takes a query");
            };
            let mut params = json!({ "query": query });
            if let Some(limit) = args.get("limit").and_then(Value::as_u64) {
                params["limit"] = json!(limit);
            }
            ("library.search", params)
        }
        "get_queue" => ("queue.list", json!({})),
        // Items go through as typed: an MCP client shares no working directory
        // with rox, and the socket's refusal explains a relative path.
        "add_to_queue" => {
            let Some(items) = args.get("items").and_then(Value::as_array) else {
                return refusal(
                    "add_to_queue takes items: paths, station stream URLs, or \
                     source|path keys from search_library",
                );
            };
            if items.is_empty() {
                return refusal("add_to_queue takes at least one item");
            }

            let mut params = json!({ "paths": items });
            if let Some(mode) = args.get("mode").and_then(Value::as_str) {
                params["mode"] = json!(mode);
            }
            ("queue.add", params)
        }
        "rescan_library" => ("library.rescan", json!({})),
        "get_tasks" => ("tasks.status", json!({})),
        "start_task" => ("tasks.start", args),
        "stop_task" => ("tasks.stop", args),
        other => return refusal(&format!("no such tool: {other}")),
    };

    match proxy(rox, socket, method, params) {
        Ok(result) => json!({
            "content": [{
                "type": "text",
                "text": serde_json::to_string_pretty(&result).unwrap_or_default(),
            }],
        }),
        Err(reason) => refusal(&reason),
    }
}

/// The gates are checked on every call, so a toggle flipped mid-session applies at once.
fn proxy(
    rox: &mut Option<Client>,
    socket: &std::path::Path,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    // One reconnect per call, for a rox restarted since the last tool use.
    for _ in 0..2 {
        if rox.is_none() {
            *rox =
                Some(Client::connect(socket).map_err(|err| {
                    format!("{err}. Is rox running, and on this data directory?")
                })?);
        }
        let client = rox.as_mut().expect("connected above");
        let (ai, mcp) = match client.call("ai.status", json!({})) {
            Ok(status) => (
                status
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                status.get("mcp").and_then(Value::as_bool).unwrap_or(false),
            ),
            Err(err) if err.is_transport() => {
                *rox = None;
                continue;
            }
            Err(err) => return Err(err.to_string()),
        };
        if !ai {
            return Err(
                "AI features are switched off in rox. Turn on \"Enable AI Features\" at the \
                 top of Settings > Application to let MCP clients in."
                    .into(),
            );
        }
        if !mcp {
            return Err(
                "The MCP server is switched off in rox. Turn on \"Enable MCP Server\" on the \
                 Settings > MCP page to let clients in."
                    .into(),
            );
        }
        match client.call(method, params.clone()) {
            Ok(result) => return Ok(result),
            Err(err) if err.is_transport() => {
                *rox = None;
                continue;
            }
            Err(err) => return Err(err.to_string()),
        }
    }
    Err("rox stopped answering; is it still running?".into())
}

fn refusal(reason: &str) -> Value {
    json!({
        "isError": true,
        "content": [{ "type": "text", "text": reason }],
    })
}
