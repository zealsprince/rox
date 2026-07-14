//! Standalone stdin-driven runner. The same engine plays behind a GPUI
//! window in the rox app; this binary keeps the CLI spike working.
//!
//! Usage: rox-prototype-playback <file> [file...]
//! Commands on stdin: p pause/resume, s <secs> seek, n next, b prev,
//! v <0..2> volume, q quit. With stdin closed it plays the queue and exits.

use std::io::BufRead;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use rox_playback::engine::{self, Cmd};
use rox_playback::output;
use rox_playback::shared::Shared;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();

    // --count: no playback, just decode and compare frame counts against the
    // container's playable count. Exact match = sample-accurate gapless trim.
    if args.first().map(String::as_str) == Some("--count") {
        for path in &args[1..] {
            let path = std::path::PathBuf::from(path);
            match engine::count_frames(&path) {
                Ok((decoded, expected)) => {
                    let verdict = match expected {
                        Some(e) if e == decoded => "exact".to_string(),
                        Some(e) => format!("off by {}", decoded as i64 - e as i64),
                        None => "cannot verify".to_string(),
                    };
                    println!(
                        "{}: decoded {decoded} frames, container says {} -> {verdict}",
                        path.display(),
                        expected.map_or("unknown".into(), |e| e.to_string()),
                    );
                }
                Err(e) => eprintln!("{}: {e}", path.display()),
            }
        }
        return;
    }

    let queue: Vec<std::path::PathBuf> = std::mem::take(&mut args)
        .into_iter()
        .map(Into::into)
        .collect();
    if queue.is_empty() {
        eprintln!("usage: rox-prototype-playback [--count] <file> [file...]");
        std::process::exit(2);
    }

    let shared = Arc::new(Shared::new(queue.len()));

    let out = match output::open(shared.clone()) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("audio output: {e}");
            std::process::exit(1);
        }
    };
    let device_rate = out.sample_rate;
    println!(
        "device: {} Hz, ring {} frames, {} track(s) queued",
        device_rate,
        out.ring_frames,
        queue.len()
    );
    println!("commands: p pause, s <secs> seek, n next, b prev, v <0..2> volume, q quit");

    let (tx, rx) = mpsc::channel::<Cmd>();
    let engine = engine::Engine::new(queue, shared.clone(), out.producer, device_rate, rx);
    let decode = std::thread::Builder::new()
        .name("decode".into())
        .spawn(move || engine.run())
        .expect("spawn decode thread");

    // Status line + tap meter, 5 Hz.
    let status_shared = shared.clone();
    let mut tap = out.tap;
    std::thread::Builder::new()
        .name("status".into())
        .spawn(move || {
            let mut meter = 0.0f32;
            loop {
                // Drain the tap like a visualizer would: take what's there,
                // never wait for more.
                let mut peak = 0.0f32;
                let mut drained = false;
                while let Ok(s) = tap.pop() {
                    peak = peak.max(s.abs());
                    drained = true;
                }
                meter = if drained { peak } else { meter * 0.7 };

                print_status(&status_shared, device_rate, meter);
                std::thread::sleep(Duration::from_millis(200));
            }
        })
        .expect("spawn status thread");

    // Command loop on the main thread. EOF on stdin means run unattended:
    // play the queue to the end, then exit.
    let mut quit_now = false;
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let mut parts = line.split_whitespace();
        match parts.next() {
            Some("p") => drop(tx.send(Cmd::TogglePause)),
            Some("n") => drop(tx.send(Cmd::Next)),
            Some("b") => drop(tx.send(Cmd::Prev)),
            Some("s") => match parts.next().and_then(|a| a.parse::<f64>().ok()) {
                Some(secs) => drop(tx.send(Cmd::Seek(secs))),
                None => println!("\nusage: s <seconds>"),
            },
            Some("v") => match parts.next().and_then(|a| a.parse::<f32>().ok()) {
                Some(v) => drop(tx.send(Cmd::Volume(v))),
                None => println!("\nusage: v <0..2>"),
            },
            Some("q") => {
                quit_now = true;
                break;
            }
            Some(other) => println!("\nunknown command: {other}"),
            None => {}
        }
    }

    while !quit_now && !shared.ended.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(100));
    }

    let _ = tx.send(Cmd::Quit);
    let _ = decode.join();
    drop(out.stream);
    println!();
}

fn print_status(shared: &Shared, device_rate: u32, meter: f32) {
    let Some((track, secs)) = shared.position(device_rate) else {
        return;
    };
    let tracks = shared.tracks.lock().unwrap();
    let info = tracks.get(track).and_then(|t| t.as_ref());
    let name = info.map(|i| i.name.as_str()).unwrap_or("?");
    let dur = info
        .and_then(|i| i.duration_secs)
        .map(fmt_time)
        .unwrap_or_else(|| "?".into());
    let src = info
        .map(|i| format!("{} Hz/{}ch", i.sample_rate, i.channels))
        .unwrap_or_default();

    let state = if shared.playing.load(Ordering::Relaxed) {
        ">"
    } else {
        "="
    };
    let bars = (meter.min(1.0) * 12.0).round() as usize;
    let vol = (shared.volume() * 100.0).round() as u32;

    print!(
        "\r{state} [{}] {} {} / {} ({src}) vol {vol}% |{:<12}|   ",
        track + 1,
        name,
        fmt_time(secs),
        dur,
        "#".repeat(bars),
    );
    use std::io::Write;
    let _ = std::io::stdout().flush();
}

fn fmt_time(secs: f64) -> String {
    let m = (secs / 60.0).floor() as u64;
    format!("{m}:{:04.1}", secs - (m * 60) as f64)
}
