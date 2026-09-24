//! Run the engine for ten seconds with no UI and print frames per second,
//! the average readback cost, and the libprojectM version: the baseline the
//! zero-copy follow-up gets judged against.
//!
//! ```text
//! cargo run -p rox-milkdrop --release --example headless -- /path/to/presets
//! ```
//!
//! Without a preset directory it runs projectM's light built-in idle preset.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rox_milkdrop::{Engine, EngineOptions, PresetLibrary, Status};

const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1080;
const SECONDS: u64 = 10;

fn main() {
    let roots: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    let textures = roots.first().map(|root| root.join("textures"));
    let library = PresetLibrary::scan(&roots, textures);
    println!("presets found: {}", library.presets().len());

    let feed = Arc::new(rox_viz::AudioFeed::new());
    let engine = Engine::spawn(EngineOptions {
        feed: Arc::clone(&feed),
        library,
        preset: None,
        fps: 60,
        width: WIDTH,
        height: HEIGHT,
    });

    // Several presets draw nothing on silence, so feed a sine.
    let mut phase = 0.0f32;
    let mut tone = |samples: usize| {
        let mut buffer = Vec::with_capacity(samples * 2);
        for _ in 0..samples {
            phase = (phase + 0.02) % std::f32::consts::TAU;
            let value = phase.sin() * 0.5;
            buffer.push(value);
            buffer.push(value);
        }
        buffer
    };

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match engine.status() {
            Status::Running {
                projectm_version,
                renderer,
                gl_version,
                ..
            } => {
                println!("projectM version: {projectm_version}");
                println!("renderer: {renderer} ({gl_version})");
                break;
            }
            Status::Failed(message) => {
                eprintln!("engine failed: {message}");
                std::process::exit(1);
            }
            Status::Starting => {
                if Instant::now() > deadline {
                    eprintln!("engine never came up");
                    std::process::exit(1);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    let mut seq = 0;
    let mut frames = 0u64;
    let mut readback_total = 0u64;
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(SECONDS) {
        feed.push(&tone(800));
        if let Some(frame) = engine.frame_after(seq) {
            seq = frame.seq;
            frames += 1;
            readback_total += engine.last_readback_micros();
            assert_eq!(frame.rgba8.len(), (frame.width * frame.height * 4) as usize);
        }
        std::thread::sleep(Duration::from_millis(4));
    }
    let elapsed = started.elapsed().as_secs_f64();

    for event in engine.take_events() {
        println!("event: {event:?}");
    }
    println!("size: {WIDTH}x{HEIGHT}");
    println!("frames: {frames} in {elapsed:.2}s");
    println!("frames per second: {:.1}", frames as f64 / elapsed);
    if let Some(average) = readback_total.checked_div(frames) {
        println!("average readback: {average} us");
    }
}
