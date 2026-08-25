//! roxctl: the control socket's reference client (ADR 22). Transport verbs,
//! queue edits, library search, and the debug scope from a shell, one call
//! per invocation. Doubles as the scriptable test surface: state-level
//! checks against a live rox without eyes on the screen.
//!
//! No rox listening exits 2 with a sentence on stderr; a refused method
//! exits 1 with the server's error. `--json` prints the raw result for
//! scripts; the default output is lines for people.

use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::{json, Value};

use rox_ipc::client::Client;

const USAGE: &str = "\
roxctl - control a running rox

usage: roxctl [options] <command> [args]

options:
  --socket <path>    talk to this socket instead of deriving it
  --data-dir <path>  derive the socket for this data dir (a --portable rox)
  --json             print raw JSON results

commands:
  status                     what's playing and where its clock sits
  toggle | play | pause      the deck
  next | prev | stop
  seek <secs|+secs|-secs>    absolute, or relative when signed
  volume <0..2>
  queue                      the play order with entry ids
  add [--next|--now] <paths> queue files (default: end of the queue)
  remove <id...>             drop queued entries by id
  jump <id>                  play a queued entry now
  search [--limit N] <terms> search the library
  now                        the playing track's full tags
  art <path> <out-file>      save a track's cover art
  raw <method> [json]        any method, params as one JSON argument
";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    let mut socket: Option<PathBuf> = None;
    let mut data_dir: Option<PathBuf> = None;
    let mut as_json = false;

    // Global flags come off the front; what's left is the command.
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--socket" | "--data-dir" if i + 1 >= args.len() => {
                eprintln!("{} takes a path", args[i]);
                return ExitCode::from(1);
            }
            "--socket" => {
                socket = Some(PathBuf::from(args.remove(i + 1)));
                args.remove(i);
            }
            "--data-dir" => {
                data_dir = Some(PathBuf::from(args.remove(i + 1)));
                args.remove(i);
            }
            "--json" => {
                as_json = true;
                args.remove(i);
            }
            "--help" | "-h" | "help" => {
                print!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            _ => i += 1,
        }
    }
    let Some(command) = args.first().cloned() else {
        eprint!("{USAGE}");
        return ExitCode::from(1);
    };
    let args = &args[1..];

    let socket = socket.unwrap_or_else(|| {
        // The app's non-portable default: the OS data dir. A portable or
        // --fresh rox hashes a different folder; point --data-dir at it.
        let data_dir = data_dir.unwrap_or_else(|| {
            dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("rox")
        });
        rox_ipc::socket_path(&data_dir)
    });
    let mut client = match Client::connect(&socket) {
        Ok(client) => client,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(2);
        }
    };

    match run(&mut client, &command, args, as_json) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(1)
        }
    }
}

fn run(client: &mut Client, command: &str, args: &[String], as_json: bool) -> Result<(), String> {
    let (method, params) = match command {
        "status" => ("transport.status".into(), json!({})),
        "toggle" => ("transport.toggle".into(), json!({})),
        "play" => ("transport.play".into(), json!({})),
        "pause" => ("transport.pause".into(), json!({})),
        "next" => ("transport.next".into(), json!({})),
        "prev" => ("transport.prev".into(), json!({})),
        "stop" => ("transport.stop".into(), json!({})),
        "seek" => {
            let arg = args
                .first()
                .ok_or("seek takes seconds, signed for relative")?;
            let secs: f64 = arg.parse().map_err(|_| format!("not seconds: {arg}"))?;
            if arg.starts_with('+') || arg.starts_with('-') {
                ("transport.seek".into(), json!({ "by": secs }))
            } else {
                ("transport.seek".into(), json!({ "to": secs }))
            }
        }
        "volume" => {
            let arg = args.first().ok_or("volume takes a level, 0 to 2")?;
            let volume: f64 = arg.parse().map_err(|_| format!("not a level: {arg}"))?;
            ("transport.set_volume".into(), json!({ "volume": volume }))
        }
        "queue" => ("queue.list".into(), json!({})),
        "add" => {
            let mut mode = "end";
            let mut paths = Vec::new();
            for arg in args {
                match arg.as_str() {
                    "--next" => mode = "next",
                    "--now" => mode = "now",
                    path => paths.push(absolute(path)),
                }
            }
            if paths.is_empty() {
                return Err("add takes file or folder paths".into());
            }
            ("queue.add".into(), json!({ "paths": paths, "mode": mode }))
        }
        "remove" => {
            let ids = ids(args)?;
            ("queue.remove".into(), json!({ "ids": ids }))
        }
        "jump" => {
            let ids = ids(args)?;
            let id = ids.first().ok_or("jump takes one entry id")?;
            ("queue.jump".into(), json!({ "id": id }))
        }
        "search" => {
            let mut limit: Option<u64> = None;
            let mut terms = Vec::new();
            let mut rest = args.iter();
            while let Some(arg) = rest.next() {
                if arg == "--limit" {
                    let n = rest.next().ok_or("--limit takes a number")?;
                    limit = Some(n.parse().map_err(|_| format!("not a number: {n}"))?);
                } else {
                    terms.push(arg.as_str());
                }
            }
            if terms.is_empty() {
                return Err("search takes query terms".into());
            }
            let mut params = json!({ "query": terms.join(" ") });
            if let Some(limit) = limit {
                params["limit"] = json!(limit);
            }
            ("library.search".into(), params)
        }
        "now" => ("library.now_playing".into(), json!({})),
        "art" => {
            let path = args
                .first()
                .ok_or("art takes a track path and an out file")?;
            let out = args
                .get(1)
                .ok_or("art takes a track path and an out file")?;
            let result = client
                .call("library.artwork", json!({ "path": absolute(path) }))
                .map_err(|err| err.to_string())?;
            return save_art(&result, out);
        }
        "raw" => {
            let method = args.first().ok_or("raw takes a method name")?;
            let params = match args.get(1) {
                Some(raw) => {
                    serde_json::from_str(raw).map_err(|err| format!("bad params: {err}"))?
                }
                None => json!({}),
            };
            (method.clone(), params)
        }
        other => return Err(format!("unknown command: {other}\n{USAGE}")),
    };

    let result = client
        .call(&method, params)
        .map_err(|err| err.to_string())?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&result).unwrap_or_default()
        );
        return Ok(());
    }
    match command {
        "status" | "toggle" | "play" | "pause" | "next" | "prev" | "stop" | "seek" | "volume" => {
            print_status(&result)
        }
        "queue" => print_queue(&result),
        "search" => print_search(&result),
        "now" => print_now(&result),
        _ => println!(
            "{}",
            serde_json::to_string_pretty(&result).unwrap_or_default()
        ),
    }
    Ok(())
}

/// Entry ids off the argument list, whole and in order.
fn ids(args: &[String]) -> Result<Vec<u64>, String> {
    if args.is_empty() {
        return Err("takes entry ids (see `roxctl queue`)".into());
    }
    args.iter()
        .map(|arg| arg.parse().map_err(|_| format!("not an entry id: {arg}")))
        .collect()
}

/// The running rox has its own working directory, so relative paths leave
/// here absolute, the same treatment a single-instance handoff gets.
fn absolute(path: &str) -> String {
    std::fs::canonicalize(path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_owned())
}

fn print_status(status: &Value) {
    let state = match (
        status["active"].as_bool().unwrap_or(false),
        status["playing"].as_bool().unwrap_or(false),
    ) {
        (false, _) => "idle",
        (true, true) => "playing",
        (true, false) => "paused",
    };
    // Indexed as a Value, never as the Map underneath: Value answers a
    // missing key with null where the Map's Index panics on it.
    let track = &status["track"];
    match track.is_object() {
        true => {
            let artist = track["artist"].as_str().unwrap_or_default();
            let title = track["title"].as_str().unwrap_or_default();
            let line = if artist.is_empty() {
                title.to_owned()
            } else {
                format!("{artist} - {title}")
            };
            println!("{state}  {line}");
            println!(
                "        {} / {}  volume {:.2}",
                clock(status["position_secs"].as_f64()),
                clock(status["duration_secs"].as_f64()),
                status["volume"].as_f64().unwrap_or_default(),
            );
        }
        false => println!("{state}"),
    }
}

fn print_queue(queue: &Value) {
    let Some(entries) = queue["entries"].as_array().filter(|e| !e.is_empty()) else {
        println!("queue empty");
        return;
    };
    for entry in entries {
        println!(
            "{} {:>6}  {}{}",
            if entry["current"].as_bool().unwrap_or(false) {
                ">"
            } else {
                " "
            },
            entry["id"].as_u64().unwrap_or_default(),
            entry["path"].as_str().unwrap_or_default(),
            match entry["sub"].as_u64().unwrap_or(0) {
                0 => String::new(),
                sub => format!("#{sub}"),
            },
        );
    }
}

fn print_search(result: &Value) {
    let Some(tracks) = result["tracks"].as_array().filter(|t| !t.is_empty()) else {
        println!("no matches");
        return;
    };
    for track in tracks {
        println!(
            "{} - {} - {}  [{}]",
            track["artist"].as_str().unwrap_or_default(),
            track["album"].as_str().unwrap_or_default(),
            track["title"].as_str().unwrap_or_default(),
            clock(track["duration_ms"].as_f64().map(|ms| ms / 1000.0)),
        );
    }
    let total = result["total"].as_u64().unwrap_or_default();
    if total as usize > tracks.len() {
        println!("({} of {} matches shown)", tracks.len(), total);
    }
}

fn print_now(track: &Value) {
    if track.is_null() {
        println!("nothing playing");
        return;
    }
    for field in [
        "title",
        "artist",
        "album",
        "album_artist",
        "genre",
        "year",
        "track_no",
        "codec",
        "path",
    ] {
        let value = &track[field];
        if value.is_null() {
            continue;
        }
        let text = match value.as_str() {
            Some(s) => s.to_owned(),
            None => value.to_string(),
        };
        if !text.is_empty() {
            println!("{field:>12}  {text}");
        }
    }
}

/// Decode one artwork response to a file.
fn save_art(result: &Value, out: &str) -> Result<(), String> {
    let data = result["data_base64"]
        .as_str()
        .ok_or("no artwork in the answer")?;
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| "artwork arrived garbled")?;
    std::fs::write(out, &bytes).map_err(|err| format!("can't write {out}: {err}"))?;
    println!(
        "{out}: {} bytes, {}",
        bytes.len(),
        result["mime"].as_str().unwrap_or("unknown type"),
    );
    Ok(())
}

/// Seconds as m:ss, a dash while unknown.
fn clock(secs: Option<f64>) -> String {
    match secs {
        Some(secs) if secs.is_finite() && secs >= 0.0 => {
            let whole = secs as u64;
            format!("{}:{:02}", whole / 60, whole % 60)
        }
        _ => "-:--".into(),
    }
}
