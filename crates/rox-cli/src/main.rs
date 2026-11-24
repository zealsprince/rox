//! roxctl: the control socket's reference client (ADR 22), one call per
//! invocation. No rox listening exits 2; a refused method exits 1. `--json`
//! prints raw results for scripts.

use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::{Value, json};

use rox_ipc::client::Client;

/// `print!` through stdout, ending the run quietly once the reader has gone.
macro_rules! out {
    ($($arg:tt)*) => {
        write_out(format_args!($($arg)*))
    };
}

/// `println!` the same way.
macro_rules! outln {
    ($($arg:tt)*) => {
        write_out(format_args!("{}\n", format_args!($($arg)*)))
    };
}

/// std ignores SIGPIPE, so a closed pipe (`roxctl status | head -1`) comes
/// back as a BrokenPipe write error, which `println!` turns into a panic.
/// Here it exits 0, and `watch` ends at its next line.
fn write_out(args: std::fmt::Arguments) {
    use std::io::Write;

    let Err(err) = std::io::stdout().lock().write_fmt(args) else {
        return;
    };

    if err.kind() == std::io::ErrorKind::BrokenPipe {
        std::process::exit(0);
    }
    eprintln!("roxctl: writing output: {err}");
    std::process::exit(1);
}

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
  track [--include a,b] <key>
                             everything stored about one track, by path or
                             a key off `search --json`; sections: tags,
                             lyrics, acoustic, enrichment, stats
  history [--limit N] [--before C] [key]
                             what was played, newest first; --before takes
                             the cursor a page ends with
  rescan                     scan the library folders again
  tasks                      the long analysis passes and their progress
  task-start <pass>          start a pass: acoustic, replaygain, tempo,
                             sortnames, romanize
  task-stop <pass>           stop a running pass at the next file
  watch                      follow playback, track, and queue events
  art <path> <out-file>      save a track's cover art
  library folders            the library folders with their track counts
  library add <dir>          add a folder, scanned and watched as a picked
                             one is
  library remove <dir>       drop a folder and its tracks; files stay
  settings list              the settings these verbs reach, with values
  settings get <key>
  settings set <key> <value> through the setting's own switch; bools are
                             true or false, choices by their saved name
  workspace import <file>    store a .roxw in the workspace list and offer
                             it for applying in the front window
  workspace save <name>      save the live look under a new name
  workspace export <name> <file>
                             write a saved or shipped workspace as a .roxw
  asset import <file>        add an image or font to the live workspace
  plugin install <file>      list a .roxp on the Plugins page, switched off
  plugin pack <id> <file>    pack an installed plugin folder as a .roxp
  object install <file>      store a .roxo in the objects a canvas's Add
                             menu lists, placed nowhere
  object export [--name N] [--author A] [--description D] [--license L]
                <item> <file>
                             write a canvas item as a .roxo; exports never
                             write over a file
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
  frames [ms]                time a workspace window's frames over a span
                             (default 2000 ms): count, mean and worst cost
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
                out!("{USAGE}");
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
        "track" => ("library.track".into(), track_params(args)?),
        "history" => ("library.history".into(), history_params(args)?),
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
        "frames" => {
            let mut params = json!({});
            if let Some(ms) = args.first() {
                let ms: u64 = ms.parse().map_err(|_| format!("not a span in ms: {ms}"))?;
                params["ms"] = json!(ms);
            }
            ("debug.frames".into(), params)
        }
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
        "library" => match args {
            [verb] if verb == "folders" => ("library.folders".into(), json!({})),
            [verb, dir] if verb == "add" => (
                "library.add_folder".into(),
                json!({ "path": absolute(dir) }),
            ),
            [verb, dir] if verb == "remove" => (
                "library.remove_folder".into(),
                json!({ "path": absolute(dir) }),
            ),
            _ => return Err("library takes folders, add <dir>, or remove <dir>".into()),
        },
        "settings" => match args {
            [verb] if verb == "list" => ("settings.list".into(), json!({})),
            [verb, key] if verb == "get" => ("settings.get".into(), json!({ "key": key })),
            [verb, key, value] if verb == "set" => (
                "settings.set".into(),
                json!({ "key": key, "value": setting_value(value) }),
            ),
            _ => return Err("settings takes list, get <key>, or set <key> <value>".into()),
        },
        "workspace" => match args {
            [verb, file] if verb == "import" => {
                ("workspace.import".into(), json!({ "path": absolute(file) }))
            }
            [verb, name] if verb == "save" => ("workspace.save".into(), json!({ "name": name })),
            [verb, name, file] if verb == "export" => (
                "workspace.export".into(),
                json!({ "name": name, "path": out_path(file) }),
            ),
            _ => {
                return Err(
                    "workspace takes import <file>, save <name>, or export <name> <file>".into(),
                );
            }
        },
        "asset" => match args {
            [verb, file] if verb == "import" => {
                ("assets.import".into(), json!({ "path": absolute(file) }))
            }
            _ => return Err("asset takes import <file>".into()),
        },
        "plugin" => match args {
            [verb, file] if verb == "install" => {
                ("plugins.install".into(), json!({ "path": absolute(file) }))
            }
            [verb, id, file] if verb == "pack" => (
                "plugins.pack".into(),
                json!({ "id": id, "path": out_path(file) }),
            ),
            _ => return Err("plugin takes install <file> or pack <id> <file>".into()),
        },
        "object" => match args {
            [verb, file] if verb == "install" => {
                ("objects.install".into(), json!({ "path": absolute(file) }))
            }
            [verb, rest @ ..] if verb == "export" => {
                let mut params = object_export_params(rest)?;
                if let Some(id) = window {
                    params["window"] = json!(id);
                }
                ("objects.export".into(), params)
            }
            _ => return Err("object takes install <file> or export <item> <file>".into()),
        },
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
        outln!(
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
        "track" => print_track(&result),
        "history" => print_history(&result),
        "rescan" => outln!("scan started"),
        "tasks" => print_tasks(&result),
        "task-start" => print_task_started(&result),
        "task-stop" => outln!("stopping at the next file"),
        "windows" => print_windows(&result),
        "frames" => print_frames(&result),
        "actions" => print_actions(&result),
        "milkdrop" => print_milkdrop(&result),
        "workspace" => match args.first().map(String::as_str) {
            Some("save") => outln!("saved as {}", result["name"].as_str().unwrap_or_default()),
            Some("export") => outln!(
                "{}: {} bytes",
                result["path"].as_str().unwrap_or_default(),
                result["bytes"].as_u64().unwrap_or_default()
            ),
            _ => outln!(
                "imported as {}",
                result["name"].as_str().unwrap_or_default()
            ),
        },
        "plugin" if args.first().is_some_and(|verb| verb == "pack") => {
            outln!("packed to {}", result["path"].as_str().unwrap_or_default())
        }
        "plugin" => outln!(
            "installed {}, switched off",
            result["id"].as_str().unwrap_or_default()
        ),
        "object" if args.first().is_some_and(|verb| verb == "export") => outln!(
            "exported {} to {}",
            result["name"].as_str().unwrap_or_default(),
            result["path"].as_str().unwrap_or_default()
        ),
        "object" => outln!(
            "installed as {}",
            result["name"].as_str().unwrap_or_default()
        ),
        "library" => print_library(args, &result),
        "settings" => print_settings(&result),
        "asset" => outln!(
            "added {} as {}",
            result["file"].as_str().unwrap_or_default(),
            result["id"].as_str().unwrap_or_default()
        ),
        _ => outln!(
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
            outln!("{}", json!({ "method": method, "params": params }));
            continue;
        }
        match method.as_str() {
            "event.playback" => print_status(&params),
            "event.track" => print_track_change(&params),
            "event.queue" => outln!(
                "queue    rev {}",
                params["queue_rev"].as_u64().unwrap_or_default()
            ),
            other => outln!("{other}"),
        }
    }
}

fn print_track_change(track: &Value) {
    if !track.is_object() {
        outln!("track    (nothing)");
        return;
    }
    let artist = track["artist"].as_str().unwrap_or_default();
    let title = track["title"].as_str().unwrap_or_default();
    if artist.is_empty() {
        outln!("track    {title}");
    } else {
        outln!("track    {artist} - {title}");
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

/// A file that doesn't exist yet can't be canonicalized, so an export's
/// target joins onto the working directory instead.
fn out_path(path: &str) -> String {
    std::path::absolute(path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_owned())
}

/// `true`, `16` and `"de"` read as JSON; a bare word goes as a string, so
/// `set theme dark` needs no quotes.
fn setting_value(arg: &str) -> Value {
    serde_json::from_str(arg).unwrap_or_else(|_| json!(arg))
}

fn object_export_params(args: &[String]) -> Result<Value, String> {
    let mut params = json!({});
    let mut positional = Vec::new();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            flag @ ("--name" | "--author" | "--description" | "--license") => {
                let value = rest.next().ok_or(format!("{flag} takes a value"))?;
                params[&flag[2..]] = json!(value);
            }
            other => positional.push(other),
        }
    }

    match positional.as_slice() {
        [item, file] => {
            params["item"] = json!(item);
            params["path"] = json!(out_path(file));
            Ok(params)
        }
        _ => Err("object export takes a canvas item's id and an out file".into()),
    }
}

fn print_library(args: &[String], result: &Value) {
    match args.first().map(String::as_str) {
        Some("add") => {
            let path = result["path"].as_str().unwrap_or_default();
            match result["covered_by"].as_str() {
                Some(root) => outln!("{path} is inside {root}; rescanning it"),
                None => outln!("added {path}, scanning"),
            }
            for absorbed in result["absorbed"].as_array().into_iter().flatten() {
                outln!(
                    "  takes the place of {}",
                    absorbed.as_str().unwrap_or_default()
                );
            }
        }
        Some("remove") => outln!("removed {}", result["removed"].as_str().unwrap_or_default()),
        _ => {
            let Some(folders) = result["folders"].as_array().filter(|f| !f.is_empty()) else {
                outln!("no library folders");
                return;
            };
            for folder in folders {
                outln!(
                    "{:>7} tracks  {}{}",
                    folder["tracks"].as_u64().unwrap_or_default(),
                    folder["path"].as_str().unwrap_or_default(),
                    match folder["reachable"].as_bool() {
                        Some(false) => "  (unreachable)",
                        _ => "",
                    }
                );
            }
            if let Some(busy) = result["busy"].as_str() {
                outln!("busy: {busy}");
            }
        }
    }
}

/// `list` answers a row per key; `get` and `set` answer the one.
fn print_settings(result: &Value) {
    let rows = match result["settings"].as_array() {
        Some(rows) => rows.clone(),
        None => vec![result.clone()],
    };
    for row in rows {
        let value = match row["value"].as_str() {
            Some(text) => text.to_owned(),
            None => row["value"].to_string(),
        };
        let range = match row["type"].as_str() {
            Some("choice") => format!(
                "  ({})",
                row["values"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Some("number") => format!("  ({} to {})", row["min"], row["max"]),
            _ => String::new(),
        };
        outln!(
            "{:>16}  {value}{range}",
            row["key"].as_str().unwrap_or_default()
        );
    }
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
            outln!("{state}  {line}");
            outln!(
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
                    outln!("        A-B {} to {}", clock(Some(a)), clock(Some(b)))
                }
                (Some(a), None) => outln!("        A-B {} (waiting for B)", clock(Some(a))),
                _ => {}
            }
        }
        false => outln!("{state}"),
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
    outln!(
        "{indent}{edge}  tape {} of {}",
        clock(shift["window_secs"].as_f64()),
        clock(shift["cap_secs"].as_f64()),
    );
}

fn print_windows(result: &Value) {
    let Some(windows) = result["windows"].as_array().filter(|w| !w.is_empty()) else {
        outln!("no windows open");
        return;
    };
    for window in windows {
        outln!(
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

fn print_frames(result: &Value) {
    let frames = result["frames"].as_u64().unwrap_or_default();
    let span = result["span_ms"].as_f64().unwrap_or_default();
    if frames == 0 {
        outln!("no frames drawn in {span:.0} ms");
        return;
    }

    outln!(
        "{frames} frames in {span:.0} ms, mean {:.2} ms, worst {:.2} ms",
        result["mean_ms"].as_f64().unwrap_or_default(),
        result["worst_ms"].as_f64().unwrap_or_default(),
    );
}

fn print_actions(result: &Value) {
    let Some(actions) = result["actions"].as_array().filter(|a| !a.is_empty()) else {
        outln!("no matching actions");
        return;
    };
    for action in actions {
        if let Some(name) = action.as_str() {
            outln!("{name}");
        }
    }
}

fn print_queue(queue: &Value) {
    let Some(entries) = queue["entries"].as_array().filter(|e| !e.is_empty()) else {
        outln!("queue empty");
        return;
    };
    for entry in entries {
        outln!(
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
        outln!("no matches");
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
            outln!(
                "Radio - {}{key}",
                track["title"].as_str().unwrap_or_default(),
            );
            continue;
        }
        outln!(
            "{} - {} - {}  [{}]{key}",
            track["artist"].as_str().unwrap_or_default(),
            track["album"].as_str().unwrap_or_default(),
            track["title"].as_str().unwrap_or_default(),
            clock(track["duration_ms"].as_f64().map(|ms| ms / 1000.0)),
        );
    }
    let total = result["total"].as_u64().unwrap_or_default();
    if total as usize > tracks.len() {
        outln!("({} of {} matches shown)", tracks.len(), total);
    }
}

fn print_now(track: &Value) {
    if track.is_null() {
        outln!("nothing playing");
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
            outln!("{field:>12}  {text}");
        }
    }
    if let (Some(bpm), Some(source)) = (track["bpm"].as_f64(), track["bpm_source"].as_str()) {
        outln!("{:>12}  {bpm} ({source})", "bpm");
    }
    print_shift(&track["shift"], "       shift  ");
}

fn track_params(args: &[String]) -> Result<Value, String> {
    let mut include: Vec<&str> = Vec::new();
    let mut key = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        if arg == "--include" {
            let names = rest
                .next()
                .ok_or("--include takes section names, comma separated")?;
            include.extend(names.split(',').map(str::trim).filter(|n| !n.is_empty()));
        } else if key.is_none() {
            key = Some(add_arg(arg));
        } else {
            return Err(format!("track takes one key; {arg} is a second"));
        }
    }

    let key = key.ok_or("track takes a path or a key off `search --json`")?;
    let mut params = json!({ "key": key });
    if !include.is_empty() {
        params["include"] = json!(include);
    }
    Ok(params)
}

fn history_params(args: &[String]) -> Result<Value, String> {
    let mut params = json!({});
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--limit" => {
                let n = rest.next().ok_or("--limit takes a number")?;
                let n: u64 = n.parse().map_err(|_| format!("not a number: {n}"))?;
                params["limit"] = json!(n);
            }
            "--before" => {
                let cursor = rest
                    .next()
                    .ok_or("--before takes a cursor or a timestamp")?;
                params["before"] = json!(cursor);
            }
            key => params["key"] = json!(add_arg(key)),
        }
    }
    Ok(params)
}

/// The row's fields as `now` prints them, then each section asked for.
fn print_track(result: &Value) {
    let track = &result["track"];
    for field in [
        "title",
        "artist",
        "album",
        "album_artist",
        "genre",
        "year",
        "date",
        "disc_no",
        "track_no",
        "codec",
        "bitrate_kbps",
        "sample_rate_hz",
        "bit_depth",
        "rating",
        "plays",
        "comment",
        "key",
    ] {
        let value = &track[field];
        let text = match value.as_str() {
            Some(s) => s.to_owned(),
            None if value.is_null() => continue,
            None => value.to_string(),
        };
        outln!("{field:>14}  {text}");
    }
    if let Some(duration) = track["duration_ms"].as_f64() {
        outln!("{:>14}  {}", "duration", clock(Some(duration / 1000.0)));
    }
    if let (Some(bpm), Some(source)) = (track["bpm"].as_f64(), track["bpm_source"].as_str()) {
        outln!("{:>14}  {bpm} ({source})", "bpm");
    }
    if let Some(gain) = track["track_gain_db"].as_f64() {
        let source = track["replaygain_source"].as_str().unwrap_or("tag");
        outln!("{:>14}  {gain:+.2} dB ({source})", "track gain");
    }
    if let Some(start) = track["start_ms"].as_f64() {
        outln!(
            "{:>14}  {} to {}",
            "cue span",
            clock(Some(start / 1000.0)),
            clock(track["end_ms"].as_f64().map(|ms| ms / 1000.0)),
        );
    }

    if let Some(tags) = result["tags"].as_array() {
        outln!("\ntags");
        for tag in tags {
            let value = match tag["binary_bytes"].as_u64() {
                Some(bytes) => format!("({bytes} bytes, binary)"),
                None => tag["value"]
                    .as_str()
                    .unwrap_or_default()
                    .replace('\n', " / "),
            };
            outln!("  {:>20}  {value}", tag["key"].as_str().unwrap_or_default());
        }
    }
    let lyrics = &result["lyrics"];
    if !lyrics.is_null() {
        outln!(
            "\nlyrics ({}, {} timing)\n{}",
            lyrics["source"].as_str().unwrap_or_default(),
            lyrics["synced"].as_str().unwrap_or("no"),
            lyrics["text"].as_str().unwrap_or_default().trim_end(),
        );
    }
    let acoustic = &result["acoustic"];
    if !acoustic.is_null() {
        outln!(
            "\nacoustic ({})",
            acoustic["model"].as_str().unwrap_or_default()
        );
        match acoustic["neighbours"].as_array().filter(|n| !n.is_empty()) {
            Some(neighbours) => {
                for near in neighbours {
                    outln!(
                        "  {:.3}  {} - {}",
                        near["distance"].as_f64().unwrap_or_default(),
                        near["artist"].as_str().unwrap_or_default(),
                        near["title"].as_str().unwrap_or_default(),
                    );
                }
            }
            None => outln!("  not described yet"),
        }
    }
    let enrichment = &result["enrichment"];
    if !enrichment.is_null() {
        outln!("\nenrichment");
        for slot in ["artist", "album_artist", "album"] {
            let entry = &enrichment[slot];
            if entry.is_null() {
                continue;
            }
            let mut facts = Vec::new();
            for field in ["sort_name", "country", "formed"] {
                if let Some(value) = entry[field]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| entry[field].as_u64().map(|n| n.to_string()))
                {
                    facts.push(format!("{field} {value}"));
                }
            }
            let release = &entry["release"];
            for field in ["release_title", "first_release_date", "label"] {
                if let Some(value) = release[field].as_str().filter(|v| !v.is_empty()) {
                    facts.push(format!("{field} {value}"));
                }
            }
            if entry["bio"].is_string() {
                facts.push("biography stored".into());
            }
            outln!(
                "  {:>12}  {}  {}",
                slot,
                entry["name"].as_str().unwrap_or_default(),
                facts.join(", "),
            );
        }
    }
    let stats = &result["stats"];
    if !stats.is_null() {
        outln!(
            "\nplays {}, last played {}",
            stats["plays"].as_u64().unwrap_or_default(),
            stats["last_played"]
                .as_i64()
                .map(|at| at.to_string())
                .unwrap_or_else(|| "never".into()),
        );
    }
    if let Some(reasons) = result["reasons"].as_object() {
        for (section, reason) in reasons {
            outln!("\n{section}: {}", reason.as_str().unwrap_or_default());
        }
    }
}

/// One line per listen: when, what was heard, and the key that names it.
fn print_history(result: &Value) {
    let Some(entries) = result["entries"].as_array().filter(|e| !e.is_empty()) else {
        outln!("nothing played");
        return;
    };
    for entry in entries {
        let heard = &entry["heard"];
        outln!(
            "{:>10}  {} - {}  {}",
            entry["played_at"].as_i64().unwrap_or_default(),
            heard["artist"].as_str().unwrap_or("?"),
            heard["title"].as_str().unwrap_or("?"),
            entry["key"].as_str().unwrap_or_default(),
        );
    }
    if let Some(next) = result["next"].as_str() {
        outln!("(more: --before {next})");
    }
}

fn print_tasks(result: &Value) {
    for pass in ["acoustic", "replaygain", "tempo", "sortnames", "romanize"] {
        let task = &result[pass];
        let missing = task["missing"].as_u64().unwrap_or_default();
        // sortnames counts artists and romanize counts values.
        let unit = task["unit"].as_str().unwrap_or("tracks");
        if task["running"].as_bool().unwrap_or(false) {
            outln!(
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
            outln!("{pass:>10}  switched off, {missing} {unit} to do");
        } else {
            outln!("{pass:>10}  idle, {missing} {unit} to do");
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
    outln!("{line}");
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
    outln!(
        "{out}: {} bytes, {}",
        bytes.len(),
        result["mime"].as_str().unwrap_or("unknown type"),
    );
    Ok(())
}

fn print_milkdrop(snapshot: &Value) {
    outln!(
        "preset   {}",
        snapshot["preset"].as_str().unwrap_or("(none)")
    );
    outln!(
        "locked   {}",
        if snapshot["locked"].as_bool().unwrap_or(false) {
            "yes"
        } else {
            "no"
        }
    );
    outln!(
        "presets  {}",
        snapshot["presets"].as_u64().unwrap_or_default()
    );
    outln!(
        "rotation {}",
        snapshot["rotation"].as_str().unwrap_or_default()
    );
    let engine = snapshot["engine"].as_str().unwrap_or_default();
    match snapshot["renderer"].as_str() {
        Some(renderer) => outln!(
            "engine   {engine}, {renderer}, projectM {}",
            snapshot["projectm_version"].as_str().unwrap_or("?")
        ),
        None => outln!("engine   {engine}"),
    }
    outln!(
        "frames   {}",
        snapshot["frames"].as_u64().unwrap_or_default()
    );
    if let Some(failed) = snapshot["failed"].as_str() {
        outln!("failed   {failed}");
    }
    if let Some(error) = snapshot["error"].as_str() {
        outln!("error    {error}");
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
    outln!(
        "{out}: {}x{} png, seq {}",
        result["width"],
        result["height"],
        result["seq"]
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
    fn track_takes_one_key_and_a_comma_list_of_sections() {
        let args = |list: &[&str]| list.iter().map(|a| a.to_string()).collect::<Vec<_>>();

        assert_eq!(
            track_params(&args(&["--include", "tags,stats", "/m/a.flac"])).unwrap(),
            json!({ "key": "/m/a.flac", "include": ["tags", "stats"] })
        );
        assert_eq!(
            track_params(&args(&["radio|http://127.0.0.1:8767/stream"])).unwrap(),
            json!({ "key": "radio|http://127.0.0.1:8767/stream" })
        );
        assert!(track_params(&args(&[])).is_err());
        assert!(track_params(&args(&["/m/a.flac", "/m/b.flac"])).is_err());
    }

    #[test]
    fn history_reads_its_flags_and_an_optional_key() {
        let args = |list: &[&str]| list.iter().map(|a| a.to_string()).collect::<Vec<_>>();

        assert_eq!(history_params(&args(&[])).unwrap(), json!({}));
        assert_eq!(
            history_params(&args(&[
                "--limit",
                "5",
                "--before",
                "1700000000:42",
                "/m/a.flac"
            ]))
            .unwrap(),
            json!({ "limit": 5, "before": "1700000000:42", "key": "/m/a.flac" })
        );
        assert!(history_params(&args(&["--limit", "many"])).is_err());
    }

    #[test]
    fn a_setting_value_reads_as_json_or_a_bare_word() {
        assert_eq!(setting_value("true"), json!(true));
        assert_eq!(setting_value("16"), json!(16));
        assert_eq!(setting_value("dark"), json!("dark"));
        assert_eq!(setting_value("\"de\""), json!("de"));
    }

    #[test]
    fn object_export_takes_flags_an_item_and_a_file() {
        let args = |list: &[&str]| list.iter().map(|a| a.to_string()).collect::<Vec<_>>();

        let params =
            object_export_params(&args(&["--name", "Clock", "item-3", "/m/clock.roxo"])).unwrap();
        assert_eq!(params["name"], json!("Clock"));
        assert_eq!(params["item"], json!("item-3"));
        assert_eq!(params["path"], json!("/m/clock.roxo"));

        assert!(object_export_params(&args(&["item-3"])).is_err());
        assert!(object_export_params(&args(&["--author"])).is_err());
    }

    #[test]
    fn add_passes_an_unresolvable_path_through() {
        assert_eq!(add_arg("/m/not-here.flac"), "/m/not-here.flac");
    }
}
