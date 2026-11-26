//! A rox renderer in Rust: a plasma drawn on the CPU, sped up by the music's
//! signals, tinted by the playing position, with a ring under the pointer.
//! The mapping goes through `rox_frames::Writer`, which keeps the frame
//! counter the way the contract wants it. The contract is
//! `docs/03-implementation/11-renderers.md`.
//!
//! Its manifest says `resizes`, so rox makes the mapping at the target's
//! largest size and tells it the size to draw at: in `hello`, then in a
//! `size` line whenever the box it fills settles at another. It draws each
//! frame at the newest size it holds, packed into the slot's start, and the
//! slot says the size. It declares two settings, which come in `hello` and
//! then in a `settings` line when the user moves one: a palette, applied on
//! the line, and its frame rate, which it reads once at its start, so rox
//! starts it again when that one moves.
//!
//! rox's lines are read with a few string searches, so the example needs
//! nothing beyond std and rox-frames. It leans on rox writing compact JSON.
//! A real renderer wants a JSON parser.

use std::io::{BufRead, Write};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use rox_frames::{Spec, Writer, announce};

/// Frames a second, off this program's own clock, when the `rate` setting
/// doesn't say otherwise. rox sends signals on the frames it paints, so a
/// renderer that stops drawing stops hearing them.
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
    // The environment names the mapping's size, and hello the size to draw
    // at, which for a renderer that resizes can be smaller.
    let Ok(Line::Hello {
        handle,
        size,
        settings,
    }) = rx.recv()
    else {
        return Err("no hello from rox".into());
    };

    let spec = Spec::new(width, height).map_err(|e| e.to_string())?;
    // SAFETY: this is the only writer of the mapping rox lent this process.
    let mut writer = unsafe { Writer::open(mapping(handle)?, spec) }.map_err(|e| e.to_string())?;

    let mut state = State {
        size,
        ..State::default()
    };
    state.take(Line::Settings(settings));

    // Read once: rox starts this program again when `rate` moves.
    let rate = state.rate.unwrap_or(RATE).clamp(1, 240);
    let frame_time = Duration::from_nanos(1_000_000_000 / rate);
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

            // The newest size it was told, inside the mapping.
            let [w, h] = [state.size[0].min(width), state.size[1].min(height)];
            let slot = writer.frame_at(n, w, h, |pixels| {
                draw(pixels, w as usize, h as usize, phase, &state)
            });
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
    /// The size to draw at, as rox last said.
    size: [u32; 2],
    signals: Vec<f32>,
    /// The playing position, in seconds.
    time: f32,
    pointer: Option<[f32; 2]>,
    pressed: bool,
    /// The `look` setting: which palette the plasma runs through.
    look: Look,
    /// The `rate` setting, frames a second.
    rate: Option<u64>,
}

/// The palettes the `look` setting picks between, by the values its
/// manifest declares.
#[derive(Clone, Copy, Default, PartialEq)]
enum Look {
    #[default]
    Rainbow,
    Ember,
    Ice,
}

impl State {
    fn take(&mut self, line: Line) {
        match line {
            Line::Pause(on) => self.paused = on,
            Line::Size(size) => self.size = size,
            Line::Signals { values, time } => (self.signals, self.time) = (values, time),
            Line::Pointer { at, pressed } => {
                self.pointer = at;
                self.pressed = pressed.unwrap_or(self.pressed) && at.is_some();
            }
            Line::Settings(settings) => {
                // rox sends the whole set, held to the manifest, every time.
                self.look = match settings.look.as_deref() {
                    Some("ember") => Look::Ember,
                    Some("ice") => Look::Ice,
                    _ => Look::Rainbow,
                };
                self.rate = settings.rate;
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
            let wave = |offset: f32| (a + offset).sin() * 0.5 + 0.5;
            let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0) as u8;
            match state.look {
                Look::Rainbow => [
                    byte(wave(0.0) * 0.86 + 0.04),
                    byte(wave(2.1) * 0.86 + 0.04),
                    byte(wave(4.2) * 0.86 + 0.04),
                    255,
                ],
                Look::Ember => {
                    let heat = wave(0.0);
                    [
                        byte(0.3 + heat * 0.7),
                        byte(heat * heat * 0.75),
                        byte(heat.powi(4) * 0.3),
                        255,
                    ]
                }
                Look::Ice => {
                    let cold = wave(1.0);
                    [
                        byte(cold.powi(3) * 0.7),
                        byte(0.25 + cold * 0.6),
                        byte(0.45 + cold * 0.55),
                        255,
                    ]
                }
            }
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
        /// The size to draw at first.
        size: [u32; 2],
        settings: Settings,
    },
    Pause(bool),
    /// Draw at this size from the next frame on.
    Size([u32; 2]),
    Settings(Settings),
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

/// The settings `plugin.json` declares, as rox sent them.
#[derive(Default)]
struct Settings {
    look: Option<String>,
    rate: Option<u64>,
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
        // The first width and height are hello's own: the size to draw at.
        // The mapping's come after, under `frames`.
        let size = [
            number(line, "width").unwrap_or(0.0) as u32,
            number(line, "height").unwrap_or(0.0) as u32,
        ];
        return Line::Hello {
            handle,
            size,
            settings: settings(line),
        };
    }

    if let Some(on) = after(line, "pause") {
        return Line::Pause(on.starts_with("true"));
    }

    if let Some(size) = after(line, "size") {
        let width = number(size, "width").unwrap_or(0.0) as u32;
        let height = number(size, "height").unwrap_or(0.0) as u32;
        return Line::Size([width, height]);
    }

    if line.starts_with("{\"settings\":") {
        return Line::Settings(settings(line));
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

/// The two settings out of a line that carries them.
fn settings(line: &str) -> Settings {
    let Some(held) = after(line, "settings") else {
        return Settings::default();
    };

    Settings {
        look: after(held, "look")
            .and_then(|text| text.split('"').nth(1))
            .map(str::to_string),
        rate: number(held, "rate").map(|rate| rate as u64),
    }
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
