//! rox-mcp: the MCP face of the control socket (ADR 22). MCP clients spawn
//! stdio servers as child processes, which a long-running GUI can't be, so
//! this thin binary sits between: MCP over stdio on one side, the socket on
//! the other. Every tool proxies a socket method, so the tool surface can't
//! drift ahead of what the socket serves.
//!
//! Two toggles gate the surface: "Enable AI Features" on the Application
//! page, and "Enable MCP Server" on the MCP page it reveals. Each tool call
//! asks the running rox first and turns a switched-off toggle into a clear
//! refusal rather than a hang. The socket missing entirely (no rox running)
//! reads the same way, as a tool error with the reason in it.
//!
//! MCP is JSON-RPC 2.0, one object per line on stdio, same framing as the
//! socket itself. This covers the subset tools need: initialize, ping,
//! tools/list, and tools/call; notifications are read and dropped.
//!
//! `--dev` widens the surface with the ui_ drive tools, proxies of the
//! socket's debug scope, so an agent working on rox can list windows,
//! dispatch actions, and send synthetic input through its MCP client. The
//! flag goes on the config line spawning this binary, which keeps a user's
//! music-facing MCP config from carrying UI-driving tools by accident.

use std::io::{BufRead as _, Write as _};
use std::path::PathBuf;

use serde_json::{json, Value};

use rox_ipc::client::Client;

/// The newest MCP revision this proxy knows it satisfies, offered when the
/// client asks for one we don't recognize.
const MCP_VERSION: &str = "2025-06-18";

/// The revisions we answer verbatim: the tools surface is unchanged across
/// them, so agreeing to the client's own dialect beats forcing a downgrade
/// dance on it.
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
        // A notification carries no id and takes no response.
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

/// The tool surface: now-playing, transport, library search, the queue,
/// the rescan kick, and the long analysis passes, each a straight proxy
/// of one socket method. `--dev` adds the
/// drive tools over the socket's debug scope, for agents working on rox
/// itself; a user-facing MCP config leaves them out.
fn tools(dev: bool) -> Value {
    let mut tools = base_tools();
    if dev {
        if let (Value::Array(all), Value::Array(extra)) = (&mut tools, dev_tools()) {
            all.extend(extra);
        }
    }
    tools
}

fn base_tools() -> Value {
    json!([
        {
            "name": "now_playing",
            "description": "What rox is playing right now: the track's tags, where its \
                            clock sits, and whether audio is moving.",
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
            "name": "search_library",
            "description": "Search the music library. Free terms match title, artist, \
                            album, and genre; pins like artist:name, album:name, \
                            genre:name, year:1990 narrow to one field.",
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
            "name": "rescan_library",
            "description": "Rescan the library folders for new, changed, and removed \
                            files. The scan runs in the background; searches pick up \
                            its results as they land.",
            "inputSchema": { "type": "object", "properties": {} },
        },
        {
            "name": "get_tasks",
            "description": "The long analysis passes (acoustic, ReplayGain, tempo): \
                            whether each could start, how many tracks it would work \
                            through, and live progress while one runs.",
            "inputSchema": { "type": "object", "properties": {} },
        },
        {
            "name": "start_task",
            "description": "Start a long analysis pass over the library. These cost \
                            hours on a large library, and in tags save mode rewrite \
                            audio files; the answer says what the pass took on, with \
                            an estimate where this machine knows its pace.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pass": { "type": "string", "enum": ["acoustic", "replaygain", "tempo"] },
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
                    "pass": { "type": "string", "enum": ["acoustic", "replaygain", "tempo"] },
                },
                "required": ["pass"],
            },
        },
    ])
}

/// The drive tools `--dev` turns on: synthetic input and action dispatch
/// against a live rox, platform-free because everything lands in gpui's
/// own event pipeline. Coordinates are window-local logical pixels; every
/// tool takes an optional window id from ui_windows and defaults to the
/// active window.
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

/// One tool call against the running rox. Tool-level failures (no rox, the
/// AI toggle off, a refused method) come back as isError results with the
/// reason in the text, which is where MCP expects them; only malformed
/// requests earn protocol errors.
fn call(rox: &mut Option<Client>, socket: &std::path::Path, params: &Value, dev: bool) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    // The drive tools pass their arguments through whole: the socket method
    // validates, and its errors already read as sentences.
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
                )
            }
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
        "rescan_library" => ("library.rescan", json!({})),
        "get_tasks" => ("tasks.status", json!({})),
        // The pass argument goes through whole: the socket method validates
        // it and its error already reads as a sentence.
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

/// Ask the running rox, connecting or reconnecting as needed, with the AI
/// gate checked first on every call so a toggle flipped mid-session
/// applies to the next tool use.
fn proxy(
    rox: &mut Option<Client>,
    socket: &std::path::Path,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    // One reconnect attempt per call: a rox restarted since the last tool
    // use left a dead client behind, and the second try is against the
    // fresh socket.
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
                // The connection died under us; drop it and let the retry
                // reconnect.
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

/// A tool-level failure the way MCP expects it: an isError result whose text
/// says why, so the model can read the reason instead of a bare code.
fn refusal(reason: &str) -> Value {
    json!({
        "isError": true,
        "content": [{ "type": "text", "text": reason }],
    })
}
