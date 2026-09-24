//! roxctl: the control socket's reference client (ADR 22), one call per
//! invocation. No rox listening exits 2; a refused method exits 1. `--json`
//! prints raw results for scripts.

use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::{Value, json};

use rox_ipc::client::Client;

const USAGE: &str = "\
roxctl - control a running rox

usage: roxctl [options] <command> [args]

options:
  --socket <path>    talk to this socket instead of deriving it
  --data-dir <path>  derive the socket for this data dir (a --portable rox)
  --window <id>      aim drive commands at this window (see `windows`)
  --json             print raw JSON results

commands:
  status                     what's playing and where its clock sits
  toggle | play | pause      the deck
  next | prev | stop
  seek <secs|+secs|-secs>    absolute, or relative when signed. On a
                             station the number is seconds behind the live
                             edge, since a broadcast has no position
  seek live [secs]           a station only: jump to the live edge, or that
                             many seconds behind it
  volume <0..2>
  ab [mark|clear|<a> <b>]    A-B repeat: step the cycle (default), clear
                             it, or set a section in seconds outright
  queue                      the play order with entry ids
  add [--next|--now] <what>  queue tracks (default: end of the queue): file
                             or folder paths, a stream URL of a station in
                             the library, or a source|path key off `search
                             --json`
  remove <id...>             drop queued entries by id
  jump <id>                  play a queued entry now
  search [--limit N] <terms> search the library
  now                        the playing track's full tags
  rescan                     scan the library folders again
  tasks                      the long analysis passes and their progress
  task-start <pass>          start a pass: acoustic, replaygain, tempo,
                             sortnames, romanize
  task-stop <pass>           stop a running pass at the next file
  watch                      follow playback, track, and queue events
  art <path> <out-file>      save a track's cover art
  raw <method> [json]        any method, params as one JSON argument

drive commands (the debug scope: work the UI without OS input tools):
  windows                    open windows with the ids drive commands take
  actions [filter]           dispatchable action names
  action <name> [json]       dispatch an action by name, data as JSON
  key <keystrokes...>        send keystrokes, e.g. ctrl-comma escape
  type <text...>             type into the focused element
  click <x> <y>              click at window-local logical pixels
                             (--right, --middle, --double)
  hover <x> <y>              move the mouse to a point
  scroll <x> <y> <dy> [dx]   scroll at a point, wheel lines, signed
  panels                     the frontmost workspace's dock tree
  milkdrop [op] [arg]        the Milkdrop panel by verb: status (default),
                             load <file>, lock on|off, rescan, next, prev,
                             frame <out.png> (the engine's own frame)
";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    let mut socket: Option<PathBuf> = None;
    let mut data_dir: Option<PathBuf> = None;
    let mut window: Option<u64> = None;
    let mut as_json = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--socket" | "--data-dir" | "--window" if i + 1 >= args.len() => {
                eprintln!("{} takes a value", args[i]);
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
            "--window" => {
                let id = args.remove(i + 1);
                let Ok(id) = id.parse() else {
                    eprintln!("not a window id: {id}");
                    return ExitCode::from(1);
                };
                window = Some(id);
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
        // A portable or --fresh rox hashes a different folder; use --data-dir.
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

    match run(&mut client, &command, args, window, as_json) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(1)
        }
    }
}

fn run(
    client: &mut Client,
    command: &str,
    args: &[String],
    window: Option<u64>,
    as_json: bool,
) -> Result<(), String> {
    let (method, mut params) = match command {
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
                .ok_or("seek takes seconds, signed for relative, or live")?;
            // A station's seek counts back from the live edge, so `live` makes
            // a script say which it meant.
            if arg == "live" {
                let behind: f64 = match args.get(1) {
                    Some(secs) => secs.parse().map_err(|_| format!("not seconds: {secs}"))?,
                    None => 0.0,
                };
                ("transport.seek".into(), json!({ "behind": behind }))
            } else {
                let secs: f64 = arg.parse().map_err(|_| format!("not seconds: {arg}"))?;
                match arg.starts_with('+') || arg.starts_with('-') {
                    true => ("transport.seek".into(), json!({ "by": secs })),
                    false => ("transport.seek".into(), json!({ "to": secs })),
                }
            }
        }
        "volume" => {
            let arg = args.first().ok_or("volume takes a level, 0 to 2")?;
            let volume: f64 = arg.parse().map_err(|_| format!("not a level: {arg}"))?;
            ("transport.set_volume".into(), json!({ "volume": volume }))
        }
        "ab" => match args {
            [] => ("transport.ab".into(), json!({ "action": "mark" })),
            [word] if word == "mark" || word == "clear" => {
                ("transport.ab".into(), json!({ "action": word }))
            }
            [a, b] => {
                let secs = |arg: &String| -> Result<f64, String> {
                    arg.parse().map_err(|_| format!("not seconds: {arg}"))
                };
                (
                    "transport.ab".into(),
                    json!({ "a": secs(a)?, "b": secs(b)? }),
                )
            }
            _ => return Err("ab takes mark, clear, or two positions in seconds".into()),
        },
        "queue" => ("queue.list".into(), json!({})),
        "add" => {
            let mut mode = "end";
            let mut paths = Vec::new();
            for arg in args {
                match arg.as_str() {
                    "--next" => mode = "next",
                    "--now" => mode = "now",
                    what => paths.push(add_arg(what)),
                }
            }
            if paths.is_empty() {
                return Err("add takes paths, a stream URL, or a source|path key".into());
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
        "rescan" => ("library.rescan".into(), json!({})),
        "tasks" => ("tasks.status".into(), json!({})),
        "task-start" => {
            let pass = args
                .first()
                .ok_or("task-start takes acoustic, replaygain, tempo, sortnames, or romanize")?;
            ("tasks.start".into(), json!({ "pass": pass }))
        }
        "task-stop" => {
            let pass = args
                .first()
                .ok_or("task-stop takes acoustic, replaygain, tempo, sortnames, or romanize")?;
            ("tasks.stop".into(), json!({ "pass": pass }))
        }
        "windows" => ("debug.windows".into(), json!({})),
        "panels" => ("debug.panels".into(), json!({})),
        "actions" => {
            let mut params = json!({});
            if let Some(filter) = args.first() {
                params["filter"] = json!(filter);
            }
            ("debug.actions".into(), params)
        }
        "action" => {
            let name = args
                .first()
                .ok_or("action takes a name (see `roxctl actions`)")?;
            let mut params = json!({ "name": name });
            if let Some(raw) = args.get(1) {
                params["data"] =
                    serde_json::from_str(raw).map_err(|err| format!("bad data: {err}"))?;
            }
            ("debug.action".into(), params)
        }
        "key" => {
            if args.is_empty() {
                return Err("key takes keystrokes, e.g. ctrl-comma escape".into());
            }
            ("debug.key".into(), json!({ "keys": args.join(" ") }))
        }
        "type" => {
            if args.is_empty() {
                return Err("type takes the text to type".into());
            }
            ("debug.type".into(), json!({ "text": args.join(" ") }))
        }
        "click" => {
            let mut params = json!({});
            let mut coords = Vec::new();
            for arg in args {
                match arg.as_str() {
                    "--right" => params["button"] = json!("right"),
                    "--middle" => params["button"] = json!("middle"),
                    "--double" => params["count"] = json!(2),
                    other => coords.push(other),
                }
            }
            let (x, y) = point_args(&coords, "click")?;
            params["x"] = json!(x);
            params["y"] = json!(y);
            ("debug.click".into(), params)
        }
        "hover" => {
            let coords: Vec<&str> = args.iter().map(String::as_str).collect();
            let (x, y) = point_args(&coords, "hover")?;
            ("debug.hover".into(), json!({ "x": x, "y": y }))
        }
        "scroll" => {
            let coords: Vec<&str> = args.iter().map(String::as_str).collect();
            let (x, y) = point_args(&coords, "scroll")?;
            let dy: f64 = coords
                .get(2)
                .ok_or("scroll takes x, y, and a wheel-line delta")?
                .parse()
                .map_err(|_| format!("not a delta: {}", coords[2]))?;
            let mut params = json!({ "x": x, "y": y, "dy": dy });
            if let Some(dx) = coords.get(3) {
                let dx: f64 = dx.parse().map_err(|_| format!("not a delta: {dx}"))?;
                params["dx"] = json!(dx);
            }
            ("debug.scroll".into(), params)
        }
        "milkdrop" => {
            let op = args.first().map(String::as_str).unwrap_or("status");
            let mut params = json!({ "op": op });
            match op {
                "load" => {
                    let file = args.get(1).ok_or("load takes a preset file")?;
                    params["path"] = json!(absolute(file));
                }
                "lock" => {
                    params["on"] = json!(match args.get(1).map(String::as_str) {
                        Some("on") => true,
                        Some("off") => false,
                        _ => return Err("lock takes on or off".into()),
                    });
                }
                "frame" => {
                    let out = args.get(1).ok_or("frame takes an out file")?;
                    if let Some(id) = window {
                        params["window"] = json!(id);
                    }
                    let result = client
                        .call("debug.milkdrop", params)
                        .map_err(|err| err.to_string())?;
                    return save_frame(&result, out);
                }
                _ => {}
            }
            ("debug.milkdrop".into(), params)
        }
        "watch" => return watch(client, as_json),
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

    if let Some(id) = window
        && method.starts_with("debug.")
    {
        params["window"] = json!(id);
    }

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
        "rescan" => println!("scan started"),
        "tasks" => print_tasks(&result),
        "task-start" => print_task_started(&result),
        "task-stop" => println!("stopping at the next file"),
        "windows" => print_windows(&result),
        "actions" => print_actions(&result),
        "milkdrop" => print_milkdrop(&result),
        _ => println!(
            "{}",
            serde_json::to_string_pretty(&result).unwrap_or_default()
        ),
    }
    Ok(())
}

/// The human view leads with the current status; `--json` prints frames only.
fn watch(client: &mut Client, as_json: bool) -> Result<(), String> {
    client
        .call("subscribe", json!({}))
        .map_err(|err| err.to_string())?;
    if !as_json {
        let status = client
            .call("transport.status", json!({}))
            .map_err(|err| err.to_string())?;
        print_status(&status);
    }
    loop {
        let (method, params) = client.next_event().map_err(|err| err.to_string())?;
        if as_json {
            println!("{}", json!({ "method": method, "params": params }));
            continue;
        }
        match method.as_str() {
            "event.playback" => print_status(&params),
            "event.track" => print_track_change(&params),
            "event.queue" => println!(
                "queue    rev {}",
                params["queue_rev"].as_u64().unwrap_or_default()
            ),
            other => println!("{other}"),
        }
    }
}

fn print_track_change(track: &Value) {
    if !track.is_object() {
        println!("track    (nothing)");
        return;
    }
    let artist = track["artist"].as_str().unwrap_or_default();
    let title = track["title"].as_str().unwrap_or_default();
    if artist.is_empty() {
        println!("track    {title}");
    } else {
        println!("track    {artist} - {title}");
    }
}

fn point_args(args: &[&str], command: &str) -> Result<(f64, f64), String> {
    let parse = |i: usize| -> Result<f64, String> {
        let arg = args
            .get(i)
            .ok_or(format!("{command} takes x and y in window-local pixels"))?;
        arg.parse().map_err(|_| format!("not a coordinate: {arg}"))
    };
    Ok((parse(0)?, parse(1)?))
}

fn ids(args: &[String]) -> Result<Vec<u64>, String> {
    if args.is_empty() {
        return Err("takes entry ids (see `roxctl queue`)".into());
    }
    args.iter()
        .map(|arg| arg.parse().map_err(|_| format!("not an entry id: {arg}")))
        .collect()
}

/// The running rox has its own working directory, so paths go absolute.
fn absolute(path: &str) -> String {
    resolve(std::path::Path::new(path))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_owned())
}

#[cfg(unix)]
fn resolve(path: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    std::fs::canonicalize(path)
}

/// Windows canonicalize answers in the `\\?\C:\...` verbatim form, and the
/// library stores paths as the scanner walked them, without that prefix, so
/// a canonical path would match no row. `absolute` joins onto the working
/// directory without touching the disk, the same as the single-instance
/// handoff does.
#[cfg(not(unix))]
fn resolve(path: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    std::path::absolute(path)
}

/// Stream URLs and `source|path` keys pass through: they name rows, not
/// files on this machine.
fn add_arg(what: &str) -> String {
    if what.starts_with("http://") || what.starts_with("https://") {
        return what.to_owned();
    }

    absolute(what)
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
    // Index the Value, never the Map: Map's Index panics on a missing key.
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
            // A station's `position_secs` is a listen clock, not a position;
            // the tape line says where it really is.
            print_shift(&status["shift"], "        ");
            let ab = &status["ab"];
            match (ab["a"].as_f64(), ab["b"].as_f64()) {
                (Some(a), Some(b)) => {
                    println!("        A-B {} to {}", clock(Some(a)), clock(Some(b)))
                }
                (Some(a), None) => println!("        A-B {} (waiting for B)", clock(Some(a))),
                _ => {}
            }
        }
        false => println!("{state}"),
    }
}

fn print_shift(shift: &Value, indent: &str) {
    let Some(shift) = shift.as_object() else {
        return;
    };

    let edge = match shift["timeshifted"].as_bool().unwrap_or(false) {
        true => format!("-{} behind live", clock(shift["behind_secs"].as_f64())),
        false => "live".to_owned(),
    };
    println!(
        "{indent}{edge}  tape {} of {}",
        clock(shift["window_secs"].as_f64()),
        clock(shift["cap_secs"].as_f64()),
    );
}

fn print_windows(result: &Value) {
    let Some(windows) = result["windows"].as_array().filter(|w| !w.is_empty()) else {
        println!("no windows open");
        return;
    };
    for window in windows {
        println!(
            "{} {:>4}  {:>4}x{:<4} @{}  {}",
            if window["active"].as_bool().unwrap_or(false) {
                ">"
            } else {
                " "
            },
            window["id"].as_u64().unwrap_or_default(),
            window["width"].as_f64().unwrap_or_default() as u64,
            window["height"].as_f64().unwrap_or_default() as u64,
            window["scale"].as_f64().unwrap_or(1.0),
            window["title"].as_str().unwrap_or("(untitled)"),
        );
    }
}

fn print_actions(result: &Value) {
    let Some(actions) = result["actions"].as_array().filter(|a| !a.is_empty()) else {
        println!("no matching actions");
        return;
    };
    for action in actions {
        if let Some(name) = action.as_str() {
            println!("{name}");
        }
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
        let source = track["source"].as_str();
        // Stations and Subsonic songs have no typeable path, so print the key
        // that queues them.
        let key = match source {
            Some("local") | None => String::new(),
            Some(_) => match track["key"].as_str() {
                Some(key) => format!("  {key}"),
                None => String::new(),
            },
        };
        if source == Some("radio") {
            println!(
                "Radio - {}{key}",
                track["title"].as_str().unwrap_or_default(),
            );
            continue;
        }
        println!(
            "{} - {} - {}  [{}]{key}",
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
    print_shift(&track["shift"], "       shift  ");
}

fn print_tasks(result: &Value) {
    for pass in ["acoustic", "replaygain", "tempo", "sortnames", "romanize"] {
        let task = &result[pass];
        let missing = task["missing"].as_u64().unwrap_or_default();
        // sortnames counts artists and romanize counts values.
        let unit = task["unit"].as_str().unwrap_or("tracks");
        if task["running"].as_bool().unwrap_or(false) {
            println!(
                "{pass:>10}  {}/{}  eta {}{}",
                task["done"].as_u64().unwrap_or_default(),
                task["total"].as_u64().unwrap_or_default(),
                clock(task["eta_secs"].as_f64()),
                if task["stopping"].as_bool().unwrap_or(false) {
                    "  (stopping)"
                } else {
                    ""
                },
            );
        } else if !task["enabled"].as_bool().unwrap_or(true) {
            println!("{pass:>10}  switched off, {missing} {unit} to do");
        } else {
            println!("{pass:>10}  idle, {missing} {unit} to do");
        }
    }
}

fn print_task_started(result: &Value) {
    let unit = result["unit"].as_str().unwrap_or("tracks");
    let mut line = format!(
        "started  {} {unit} on {} workers",
        result["missing"].as_u64().unwrap_or_default(),
        result["workers"].as_u64().unwrap_or_default(),
    );
    if let Some(estimate) = result["estimate"].as_str() {
        line.push_str(&format!(", {estimate}"));
    }
    if let Some(save) = result["save"].as_str() {
        line.push_str(&format!(", saving to {save}"));
    }
    println!("{line}");
}

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

fn print_milkdrop(snapshot: &Value) {
    println!(
        "preset   {}",
        snapshot["preset"].as_str().unwrap_or("(none)")
    );
    println!(
        "locked   {}",
        if snapshot["locked"].as_bool().unwrap_or(false) {
            "yes"
        } else {
            "no"
        }
    );
    println!(
        "presets  {}",
        snapshot["presets"].as_u64().unwrap_or_default()
    );
    println!(
        "rotation {}",
        snapshot["rotation"].as_str().unwrap_or_default()
    );
    let engine = snapshot["engine"].as_str().unwrap_or_default();
    match snapshot["renderer"].as_str() {
        Some(renderer) => println!(
            "engine   {engine}, {renderer}, projectM {}",
            snapshot["projectm_version"].as_str().unwrap_or("?")
        ),
        None => println!("engine   {engine}"),
    }
    println!(
        "frames   {}",
        snapshot["frames"].as_u64().unwrap_or_default()
    );
    if let Some(failed) = snapshot["failed"].as_str() {
        println!("failed   {failed}");
    }
    if let Some(error) = snapshot["error"].as_str() {
        println!("error    {error}");
    }
}

fn save_frame(result: &Value, out: &str) -> Result<(), String> {
    let data = result["data_base64"]
        .as_str()
        .ok_or("no frame in the answer")?;
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| "frame arrived garbled")?;
    std::fs::write(out, &bytes).map_err(|err| format!("can't write {out}: {err}"))?;
    println!(
        "{out}: {}x{} png, seq {}",
        result["width"], result["height"], result["seq"]
    );
    Ok(())
}

fn clock(secs: Option<f64>) -> String {
    match secs {
        Some(secs) if secs.is_finite() && secs >= 0.0 => {
            let whole = secs as u64;
            format!("{}:{:02}", whole / 60, whole % 60)
        }
        _ => "-:--".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_leaves_a_url_and_a_source_key_alone() {
        for what in [
            "http://127.0.0.1:8767/stream",
            "https://stream.example/live.mp3",
            "radio|http://127.0.0.1:8767/stream",
            "subsonic:9f2c|tr-1801",
        ] {
            assert_eq!(add_arg(what), what);
        }

        let here = std::env::current_dir().expect("a working directory");
        assert_eq!(add_arg("."), here.to_string_lossy());
    }

    #[test]
    fn add_passes_an_unresolvable_path_through() {
        assert_eq!(add_arg("/m/not-here.flac"), "/m/not-here.flac");
    }
}
