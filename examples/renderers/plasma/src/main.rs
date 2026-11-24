//! A rox renderer in Rust: a plasma drawn on the CPU, sped up by the music's
//! signals, tinted by the playing position, with a ring under the pointer.
//! The mapping goes through `rox_frames::Writer`, which keeps the frame
//! counter the way the contract wants it. The contract is
//! `docs/03-implementation/11-renderers.md`.
//!
//! rox's lines are read with a few string searches, so the example needs
//! nothing beyond std and rox-frames. It leans on rox writing compact JSON.
//! A real renderer wants a JSON parser.

use std::io::{BufRead, Write};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use rox_frames::{Spec, Writer, announce};

/// Frames a second, off this program's own clock. rox sends signals on the
/// frames it paints, so a renderer that stops drawing stops hearing them.
const RATE: u64 = 60;

fn main() {
    if let Err(why) = run() {
        // stderr goes to rox's log, a line at a time.
        eprintln!("plasma: {why}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let width = env_number("ROX_FRAMES_WIDTH")?;
    let height = env_number("ROX_FRAMES_HEIGHT")?;

    // stdin on a thread of its own, so a frame never waits on a line and a
    // line of any length arrives whole. The sender drops when stdin closes,
    // which is rox telling this program to exit.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut line = Vec::new();

        while matches!(stdin.read_until(b'\n', &mut line), Ok(n) if n > 0) {
            if tx.send(parse(&String::from_utf8_lossy(&line))).is_err() {
                return;
            }
            line.clear();
        }
    });

    // hello is always the first line. Its handle is the mapping on Windows.
    let Ok(Line::Hello { handle }) = rx.recv() else {
        return Err("no hello from rox".into());
    };

    let spec = Spec::new(width, height).map_err(|e| e.to_string())?;
    // SAFETY: this is the only writer of the mapping rox lent this process.
    let mut writer = unsafe { Writer::open(mapping(handle)?, spec) }.map_err(|e| e.to_string())?;

    let frame_time = Duration::from_nanos(1_000_000_000 / RATE);
    let mut state = State::default();
    let mut stdout = std::io::stdout().lock();
    let mut due = Instant::now();
    let mut phase = 0.0f32;
    let mut n = 0u64;

    loop {
        let now = Instant::now();
        if !state.paused && now >= due {
            // Louder signals turn the plasma faster.
            let energy = state.signals.iter().sum::<f32>() / state.signals.len().max(1) as f32;
            phase += frame_time.as_secs_f32() * (0.5 + 4.0 * energy);

            let (w, h) = (width as usize, height as usize);
            let slot = writer.frame(n, |pixels| draw(pixels, w, h, phase, &state));
            stdout
                .write_all(announce(n, slot).as_bytes())
                .and_then(|_| stdout.flush())
                .map_err(|e| format!("stdout: {e}"))?;

            n += 1;
            due = (due + frame_time).max(now);
        }

        // One line at most between frame checks, so a busy stdin can't hold
        // a frame back. Paused, only a line wakes it.
        let next = if state.paused {
            rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
        } else {
            rx.recv_timeout(due.saturating_duration_since(Instant::now()))
        };
        match next {
            Ok(line) => state.take(line),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

/// What this renderer knows of rox's lines.
#[derive(Default)]
struct State {
    paused: bool,
    signals: Vec<f32>,
    /// The playing position, in seconds.
    time: f32,
    pointer: Option<[f32; 2]>,
    pressed: bool,
}

impl State {
    fn take(&mut self, line: Line) {
        match line {
            Line::Pause(on) => self.paused = on,
            Line::Signals { values, time } => (self.signals, self.time) = (values, time),
            Line::Pointer { at, pressed } => {
                self.pointer = at;
                self.pressed = pressed.unwrap_or(self.pressed) && at.is_some();
            }
            Line::Hello { .. } | Line::Other => {}
        }
    }
}

/// The plasma: three waves summed through a palette, one byte table each so
/// a pixel costs three lookups.
fn draw(pixels: &mut [u8], width: usize, height: usize, phase: f32, state: &State) {
    let wave = |i: usize, scale: f32, speed: f32| {
        ((i as f32 * scale + phase * speed).sin() * 42.0 + 42.0) as u8
    };
    let across: Vec<u8> = (0..width)
        .map(|x| wave(x, 9.0 / width as f32, 1.0))
        .collect();
    let down: Vec<u8> = (0..height)
        .map(|y| wave(y, 7.0 / height as f32, -1.3))
        .collect();
    let diagonal: Vec<u8> = (0..width + height)
        .map(|i| wave(i, 5.0 / height as f32, 0.7))
        .collect();

    // The position moves the palette, so a seek shifts the colours.
    let shift = state.time * 0.1;
    let palette: Vec<[u8; 4]> = (0..256)
        .map(|i| {
            let a = i as f32 / 256.0 * std::f32::consts::TAU + shift;
            let channel = |offset: f32| ((a + offset).sin() * 110.0 + 120.0) as u8;
            [channel(0.0), channel(2.1), channel(4.2), 255]
        })
        .collect();

    // The ring under the pointer, in squared pixels.
    let (ring, (cx, cy)) = match state.pointer {
        Some([x, y]) => {
            let r = height as f32 / if state.pressed { 5.0 } else { 9.0 };
            ((r * r * 0.7, r * r), (x * width as f32, y * height as f32))
        }
        None => ((f32::MAX, f32::MAX), (0.0, 0.0)),
    };

    for (y, row) in pixels.chunks_exact_mut(width * 4).enumerate() {
        let dy = (y as f32 - cy).powi(2);

        for (x, pixel) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let at = across[x] as usize + down[y] as usize + diagonal[x + y] as usize;
            *pixel = palette[at];

            let d = (x as f32 - cx).powi(2) + dy;
            if d > ring.0 && d < ring.1 {
                pixel[..3].fill(255);
            }
        }
    }
}

/// A line from rox, as far as this example reads it.
enum Line {
    Hello {
        handle: u64,
    },
    Pause(bool),
    Signals {
        values: Vec<f32>,
        time: f32,
    },
    /// `at` is None once the pointer leaves. `pressed` is Some on a press
    /// or a release.
    Pointer {
        at: Option<[f32; 2]>,
        pressed: Option<bool>,
    },
    Other,
}

fn parse(line: &str) -> Line {
    // The pool line holds signal names the user typed, which could spell
    // any key. It's the only line with text from outside rox, and this
    // example doesn't need it, so it's set aside before any search.
    if line.starts_with("{\"pool\":") {
        return Line::Other;
    }

    if after(line, "hello").is_some() {
        let handle = number(line, "handle").unwrap_or(0.0) as u64;
        return Line::Hello { handle };
    }

    if let Some(on) = after(line, "pause") {
        return Line::Pause(on.starts_with("true"));
    }

    if let Some(list) = after(line, "signals") {
        let list = list.trim_start_matches('[');
        let list = &list[..list.find(']').unwrap_or(list.len())];
        let values = list
            .split(',')
            .filter_map(|v| v.trim().parse().ok())
            .collect();
        let time = number(line, "time").unwrap_or(0.0) as f32;
        return Line::Signals { values, time };
    }

    if after(line, "pointer").is_some() {
        let kind = after(line, "kind").and_then(|k| k.split('"').nth(1));
        let at = match (number(line, "x"), number(line, "y")) {
            (Some(x), Some(y)) if kind != Some("leave") => Some([x as f32, y as f32]),
            _ => None,
        };
        let pressed = match kind {
            Some("down") => Some(true),
            Some("up") | Some("leave") => Some(false),
            _ => None,
        };
        return Line::Pointer { at, pressed };
    }

    // A line this example doesn't know, from a newer rox. Skip it.
    Line::Other
}

/// The text right after `"key":`.
fn after<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let quoted = format!("\"{key}\":");
    line.find(&quoted).map(|at| &line[at + quoted.len()..])
}

/// The number right after `"key":`.
fn number(line: &str, key: &str) -> Option<f64> {
    let rest = after(line, key)?;
    let end = rest.find([',', '}', ']']).unwrap_or(rest.len());
    rest[..end].trim().parse().ok()
}

fn env_number(name: &str) -> Result<u32, String> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| format!("{name} isn't set to a number"))
}

/// The mapping rox lent: fd 3 on Unix, named by `ROX_FRAMES_FD`.
#[cfg(unix)]
fn mapping(_handle: u64) -> Result<rox_frames::Handle<'static>, String> {
    let fd = env_number("ROX_FRAMES_FD")? as i32;

    // SAFETY: rox opened this fd for this process, and nothing here closes it.
    Ok(unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) })
}

/// The mapping rox lent: the section handle `hello` names. Never compiled.
#[cfg(windows)]
fn mapping(handle: u64) -> Result<rox_frames::Handle<'static>, String> {
    use std::os::windows::io::{BorrowedHandle, RawHandle};

    // SAFETY: rox duplicated the section into this process as `handle`, and
    // nothing here closes it.
    Ok(unsafe { BorrowedHandle::borrow_raw(handle as usize as RawHandle) })
}
