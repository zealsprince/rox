//! Research prototype for ADR 8: can a curl-noise flow field driven by
//! spectrum bands hit the reference look inside a sane frame budget with
//! CPU-side rendering, and does it draw better as GPUI polylines or as a
//! per-frame image blit?
//!
//! Left click toggles the render mode, right click cycles the particle
//! count. The HUD reports sim and paint cost. Run with --release, the sim
//! and the rasterizer are an order of magnitude off in debug.

mod noise;
mod sim;

use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{
    canvas, div, point, prelude::*, px, rgb, rgba, size, App, Application, Bounds, Context,
    Corners, MouseButton, PathBuilder, Pixels, RenderImage, SharedString, TitlebarOptions, Window,
    WindowBounds, WindowOptions,
};

use sim::{Mode, Payload, Shared, SimFrame};

const BUCKET_COLORS: [u32; 5] = [0x1f8a4d40, 0x2bd97a66, 0x3dff9c99, 0x8fffc4cc, 0xd8ffe9ff];

#[derive(Default)]
struct Stats {
    sim_ms: f32,
    paint_ms: f32,
}

/// Last sim output, kept so the UI can repaint at full rate even when no new
/// sim frame has landed. Which field is live follows the last frame's mode.
#[derive(Default)]
struct FrameCache {
    trails: Option<Vec<sim::Trail>>,
    image: Option<Arc<RenderImage>>,
    showing: Option<Mode>,
}

struct VizProto {
    shared: Arc<Shared>,
    stats: Arc<Mutex<Stats>>,
    cache: Arc<Mutex<FrameCache>>,
    frames: u32,
    fps: f32,
    fps_since: Instant,
    auto: Option<AutoCycle>,
}

/// Enabled by ROX_VIZ_AUTOCYCLE=1: step through every mode and particle
/// count, print averaged stats per combo to stdout, then quit. This is how
/// the numbers in the research writeup get collected without a human
/// clicking through.
struct AutoCycle {
    combo: usize,
    since: Instant,
    frames: u32,
    sim_acc: f32,
    paint_acc: f32,
    samples: u32,
}

const AUTO_COMBOS: [(Mode, u32); 8] = [
    (Mode::Lines, 1_500),
    (Mode::Lines, 3_000),
    (Mode::Lines, 6_000),
    (Mode::Lines, 12_000),
    (Mode::Blit, 1_500),
    (Mode::Blit, 3_000),
    (Mode::Blit, 6_000),
    (Mode::Blit, 12_000),
];
const AUTO_WARMUP_SECS: f32 = 1.0;
const AUTO_COMBO_SECS: f32 = 4.0;

impl AutoCycle {
    fn apply(&mut self, combo: usize, shared: &Shared) {
        use std::sync::atomic::Ordering;
        let (mode, count) = AUTO_COMBOS[combo];
        shared
            .mode
            .store(if mode == Mode::Lines { 0 } else { 1 }, Ordering::Relaxed);
        shared.particle_count.store(count, Ordering::Relaxed);
        self.combo = combo;
        self.since = Instant::now();
        self.frames = 0;
        self.sim_acc = 0.0;
        self.paint_acc = 0.0;
        self.samples = 0;
    }

    /// Returns false when the cycle is finished.
    fn tick(&mut self, shared: &Shared, stats: &Mutex<Stats>) -> bool {
        let elapsed = self.since.elapsed().as_secs_f32();
        if elapsed > AUTO_WARMUP_SECS {
            self.frames += 1;
            let stats = stats.lock().unwrap();
            self.sim_acc += stats.sim_ms;
            self.paint_acc += stats.paint_ms;
            self.samples += 1;
        }
        if elapsed < AUTO_COMBO_SECS {
            return true;
        }
        let (mode, count) = AUTO_COMBOS[self.combo];
        let n = self.samples.max(1) as f32;
        println!(
            "autocycle mode={} particles={} sim={:.2}ms paint={:.2}ms fps={:.0}",
            if mode == Mode::Lines { "lines" } else { "blit" },
            count,
            self.sim_acc / n,
            self.paint_acc / n,
            self.frames as f32 / (elapsed - AUTO_WARMUP_SECS),
        );
        if self.combo + 1 >= AUTO_COMBOS.len() {
            println!("autocycle done");
            return false;
        }
        self.apply(self.combo + 1, shared);
        true
    }
}

impl VizProto {
    fn new(shared: Arc<Shared>) -> Self {
        let auto = std::env::var("ROX_VIZ_AUTOCYCLE").is_ok().then(|| {
            let mut auto = AutoCycle {
                combo: 0,
                since: Instant::now(),
                frames: 0,
                sim_acc: 0.0,
                paint_acc: 0.0,
                samples: 0,
            };
            auto.apply(0, &shared);
            auto
        });
        Self {
            shared,
            stats: Arc::default(),
            cache: Arc::default(),
            frames: 0,
            fps: 0.0,
            fps_since: Instant::now(),
            auto,
        }
    }
}

fn paint_trails(trails: &[sim::Trail], bounds: Bounds<Pixels>, window: &mut Window) {
    let mut builders: [(PathBuilder, bool); BUCKET_COLORS.len()] =
        std::array::from_fn(|_| (PathBuilder::stroke(px(1.25)), false));
    let map = |(x, y): (f32, f32)| {
        point(
            bounds.origin.x + bounds.size.width * x,
            bounds.origin.y + bounds.size.height * y,
        )
    };
    for trail in trails {
        let (builder, used) = &mut builders[trail.bucket as usize];
        builder.move_to(map(trail.pts[0]));
        for &pt in &trail.pts[1..] {
            builder.line_to(map(pt));
        }
        *used = true;
    }
    for (i, (builder, used)) in builders.into_iter().enumerate() {
        if !used {
            continue;
        }
        if let Ok(path) = builder.build() {
            window.paint_path(path, rgba(BUCKET_COLORS[i]));
        }
    }
}

fn absorb(cache: &mut FrameCache, frame: SimFrame, window: &mut Window) {
    match frame.payload {
        Payload::Trails(trails) => {
            cache.trails = Some(trails);
            cache.showing = Some(Mode::Lines);
        }
        Payload::Pixels { w, h, bgra } => {
            if let Some(old) = cache.image.take() {
                window.drop_image(old).ok();
            }
            let buffer = image::RgbaImage::from_raw(w as u32, h as u32, bgra)
                .expect("framebuffer size mismatch");
            cache.image = Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])));
            cache.showing = Some(Mode::Blit);
        }
    }
}

impl Render for VizProto {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.request_animation_frame();

        self.frames += 1;
        if self.fps_since.elapsed().as_secs_f32() >= 1.0 {
            self.fps = self.frames as f32 / self.fps_since.elapsed().as_secs_f32();
            self.frames = 0;
            self.fps_since = Instant::now();
        }

        if let Some(auto) = &mut self.auto {
            if !auto.tick(&self.shared, &self.stats) {
                cx.quit();
            }
        }

        let shared = self.shared.clone();
        let stats = self.stats.clone();
        let cache = self.cache.clone();

        let hud = {
            let stats = self.stats.lock().unwrap();
            let mode = match self.shared.mode() {
                Mode::Lines => "polylines",
                Mode::Blit => "image blit",
            };
            let count = self
                .shared
                .particle_count
                .load(std::sync::atomic::Ordering::Relaxed);
            [
                format!("mode: {mode} (left click to toggle)"),
                format!("particles: {count} (right click to cycle)"),
                format!("sim: {:.2} ms", stats.sim_ms),
                format!("paint: {:.2} ms", stats.paint_ms),
                format!("fps: {:.0}", self.fps),
            ]
        };

        div()
            .relative()
            .size_full()
            .bg(rgb(0x0a0f0c))
            .child(
                canvas(
                    move |_, _, _| {},
                    move |bounds, _, window, _| {
                        let started = Instant::now();
                        let fresh = shared.latest.lock().unwrap().take();
                        let mut cache = cache.lock().unwrap();
                        if let Some(frame) = fresh {
                            stats.lock().unwrap().sim_ms = frame.sim_ms;
                            absorb(&mut cache, frame, window);
                        }
                        match cache.showing {
                            Some(Mode::Lines) => {
                                if let Some(trails) = &cache.trails {
                                    paint_trails(trails, bounds, window);
                                }
                            }
                            Some(Mode::Blit) => {
                                if let Some(image) = &cache.image {
                                    window
                                        .paint_image(
                                            bounds,
                                            Corners::default(),
                                            image.clone(),
                                            0,
                                            false,
                                        )
                                        .ok();
                                }
                            }
                            None => {}
                        }
                        let mut stats = stats.lock().unwrap();
                        let ms = started.elapsed().as_secs_f32() * 1000.0;
                        stats.paint_ms = if stats.paint_ms == 0.0 {
                            ms
                        } else {
                            stats.paint_ms * 0.9 + ms * 0.1
                        };
                    },
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .top_2()
                    .left_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .text_sm()
                    .text_color(rgb(0x9adbb4))
                    .children(hud.map(SharedString::from)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.shared.toggle_mode()),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, _| {
                    this.shared.cycle_particles();
                }),
            )
    }
}

fn main() {
    let shared = Arc::new(Shared::new());
    sim::spawn(shared.clone());

    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.), px(720.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from("rox viz proto")),
                ..Default::default()
            }),
            ..Default::default()
        };
        cx.open_window(options, |_, cx| cx.new(|_| VizProto::new(shared)))
            .expect("failed to open the window");
        cx.activate(true);
    });
}
