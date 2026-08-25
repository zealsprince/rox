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
//! socket itself. The subset here is what tools need: initialize, ping,
//! tools/list, and tools/call; notifications are read and dropped.

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
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--socket" => socket = args.next().map(PathBuf::from),
            "--data-dir" => data_dir = args.next().map(PathBuf::from),
            other => {
                eprintln!("rox-mcp: unknown argument {other}; takes --socket or --data-dir");
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
            "tools/list" => Ok(json!({ "tools": tools() })),
            "tools/call" => Ok(call(&mut rox, &socket, &params)),
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

/// The tool surface: now-playing, transport, library search, and the
/// queue, each a straight proxy of one socket method.
fn tools() -> Value {
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
    ])
}

/// One tool call against the running rox. Tool-level failures (no rox, the
/// AI toggle off, a refused method) come back as isError results with the
/// reason in the text, which is where MCP wants them; only malformed
/// requests earn protocol errors.
fn call(rox: &mut Option<Client>, socket: &std::path::Path, params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
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

/// A tool-level failure the way MCP wants it: an isError result whose text
/// says why, so the model can read the reason instead of a bare code.
fn refusal(reason: &str) -> Value {
    json!({
        "isError": true,
        "content": [{ "type": "text", "text": reason }],
    })
}
