//! The render thread: everything that touches OpenGL or libprojectM.
//!
//! One thread, spawned by [`crate::Engine::spawn`], that owns the GL context
//! for its whole life. That ownership is the point. A GL context is current
//! on exactly one thread, projectM keeps state in it, and the only way to
//! keep both facts true without locking is to never let anything else in.
//! So the panel talks to this thread through a channel and reads what it
//! publishes, and no rox code outside this file ever holds a projectM handle.
//!
//! The loop is: drain commands, feed projectM the audio that arrived since
//! last time, render into an FBO, start a readback into one pixel buffer
//! while mapping the other, flip the rows, publish. It paces itself against a
//! wall clock deadline rather than sleeping a fixed interval, so a slow frame
//! is absorbed instead of drifting the whole schedule.
//!
//! The one piece of reentrancy is projectM's preset callbacks, which fire
//! from inside `projectm_opengl_render_frame_fbo`. They record what happened
//! and return; the actual preset load happens back out in the loop. Loading a
//! preset from inside projectM's own render call is legal but it means a
//! `&mut` into state the render call is already walking, and one frame of
//! delay on a preset change nobody can see is a cheap way out of that.

use std::ffi::{c_char, c_void, CStr, CString};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError};
use rox_milkdrop_sys as pm;

use crate::context::{self, HeadlessGl};
use crate::gl::{self, Gl};
use crate::library::Shuffle;
use crate::{Command, EngineOptions, Event, Frame, PresetLibrary, Rotation, Shared, Status};

/// Render size is clamped to this on each side. The floor keeps a
/// mid-drag one-pixel panel from producing a degenerate FBO; the ceiling is
/// where a readback stops being affordable at any frame rate.
const MIN_SIZE: u32 = 128;
const MAX_SIZE: u32 = 4096;

/// Defaults matching `MilkdropConfig`. The panel sends its real values as
/// commands right after spawning, but the engine has to be sane before that
/// arrives.
const DEFAULT_PRESET_DURATION: f64 = 30.0;
const DEFAULT_BEAT_SENSITIVITY: f32 = 1.0;

/// Consecutive readback maps the driver can refuse before the worker gives
/// up. One refusal is a hiccup and the next frame covers it. Thirty in a
/// row, half a second at sixty frames, is a driver that is never going to
/// map the buffer, and until this the panel sat black over it with nothing
/// but a warning per frame in the log.
const MAP_MISS_LIMIT: u32 = 30;

/// What projectM's callbacks write down. Boxed and kept alive for as long as
/// the instance is, because projectM holds the pointer.
#[derive(Default)]
struct Callbacks {
    /// `Some(is_hard_cut)` when projectM asked for a different preset.
    switch_requested: std::cell::Cell<Option<bool>>,
    failures: std::cell::RefCell<Vec<(PathBuf, String)>>,
}

pub(crate) fn run(options: EngineOptions, commands: Receiver<Command>, shared: Arc<Shared>) {
    match Worker::start(options, shared.clone()) {
        Ok(mut worker) => worker.run(&commands),
        Err(message) => {
            log::warn!("milkdrop engine did not start: {message}");
            shared.set_status(Status::Failed(message));
            // Nothing to drain: the channel is unbounded, so senders never
            // block on a receiver that walked away.
        }
    }
}

struct Worker {
    shared: Arc<Shared>,
    feed: Arc<rox_viz::AudioFeed>,
    library: PresetLibrary,
    shuffle: Shuffle,
    /// Positions in `library.presets()` that Next, Previous and projectM's
    /// own timed switch move through. The whole library until the panel
    /// narrows it, and back to the whole library if what it asked for
    /// matched nothing.
    rotation: Vec<usize>,

    /// Held, not read: dropping it is what releases the GL context, at the
    /// end of the worker's life and on the worker's own thread.
    _headless: Box<HeadlessGl>,
    gl: Gl,
    instance: pm::projectm_handle,
    callbacks: Box<Callbacks>,
    version: String,
    /// `GL_RENDERER` and `GL_VERSION`, kept for the status and for the
    /// message a readback failure names.
    renderer: String,
    gl_version: String,
    /// Readback maps refused in a row. Reset by the next one that works.
    map_misses: u32,

    targets: Targets,
    /// Where the next frame's pixels are written. Buffers come back here
    /// once the consumer is done with them, so a steady state allocates
    /// nothing.
    scratch: Vec<u8>,
    /// A published buffer somebody was still reading when the frame after it
    /// landed, kept for one more publish so it can be asked again. Two
    /// buffers going round is what a consumer holding the newest frame while
    /// the next one is rendered needs; without the second ask that consumer
    /// would have the worker allocating a buffer a frame.
    waiting: Option<Arc<Vec<u8>>>,
    pcm: Vec<f32>,
    cursor: u64,

    current: Option<usize>,
    /// Where the user has been, so Previous and Next mean back and
    /// forward rather than a step through the sorted list.
    trail: Trail,
    preset: Option<PathBuf>,
    /// How long the last map-and-flip took. Published for the headless
    /// example, which is where the zero-copy follow-up gets its baseline.
    readback_micros: u64,
    locked: bool,
    paused: bool,
    fps: u32,
    max_pcm_frames: usize,
    seq: u64,
}

impl Worker {
    fn start(options: EngineOptions, shared: Arc<Shared>) -> Result<Worker, String> {
        forward_projectm_log();
        let headless = Box::new(context::create()?);
        if !headless.is_current() {
            return Err("the OpenGL context came up but is not current".to_string());
        }

        let gl = Gl::load(|name| headless.get_proc_address(name))?;
        let renderer = gl.string(gl::RENDERER);
        let gl_version = gl.string(gl::VERSION);
        log::info!("milkdrop engine on {renderer} ({gl_version})");

        let width = options.width.clamp(MIN_SIZE, MAX_SIZE);
        let height = options.height.clamp(MIN_SIZE, MAX_SIZE);

        let instance = unsafe {
            pm::projectm_create_with_opengl_load_proc(Some(load_proc), std::ptr::null_mut())
        };
        if instance.is_null() {
            // projectM's own reason went to the log through the callback
            // above; this is the line for the panel, with the fact that
            // decides most of these in it.
            return Err(format!(
                "libprojectM needs OpenGL 3.3 and this context is {gl_version} on {renderer}"
            ));
        }

        let version = version_string();
        let callbacks = Box::new(Callbacks::default());
        let user_data = callbacks.as_ref() as *const Callbacks as *mut c_void;

        let mut targets = Targets::default();
        let max_pcm_frames = unsafe { pm::projectm_pcm_get_max_samples() }.max(1) as usize;

        unsafe {
            pm::projectm_set_window_size(instance, width as usize, height as usize);
            pm::projectm_set_fps(instance, options.fps as i32);
            pm::projectm_set_preset_duration(instance, DEFAULT_PRESET_DURATION);
            pm::projectm_set_beat_sensitivity(instance, DEFAULT_BEAT_SENSITIVITY);
            pm::projectm_set_hard_cut_enabled(instance, true);
            pm::projectm_set_aspect_correction(instance, true);
            pm::projectm_set_preset_switch_requested_event_callback(
                instance,
                Some(on_switch_requested),
                user_data,
            );
            pm::projectm_set_preset_switch_failed_event_callback(
                instance,
                Some(on_switch_failed),
                user_data,
            );
        }
        set_texture_paths(instance, &options.library);

        if let Err(message) = targets.allocate(&gl, width, height) {
            unsafe { pm::projectm_destroy(instance) };
            return Err(message);
        }

        // Start from what the feed has already written rather than zero, so a
        // panel opened mid-track doesn't shove a second of stale audio at
        // projectM's beat detector on its first frame.
        let cursor = options.feed.written();

        let rotation = (0..options.library.presets().len()).collect();
        let mut worker = Worker {
            shared,
            feed: options.feed,
            library: options.library,
            shuffle: Shuffle::new(),
            rotation,
            trail: Trail::default(),
            _headless: headless,
            gl,
            instance,
            callbacks,
            version,
            renderer,
            gl_version,
            map_misses: 0,
            targets,
            scratch: Vec::new(),
            waiting: None,
            pcm: Vec::new(),
            cursor,
            current: None,
            preset: None,
            readback_micros: 0,
            locked: false,
            paused: false,
            fps: options.fps.max(1),
            max_pcm_frames,
            seq: 0,
        };

        // A restored preset goes straight up, with no shuffle before it.
        // Nothing to load is a normal state: projectM renders its built-in
        // idle preset, so the panel is never blank.
        if let Some(path) = options.preset {
            worker.load(path, false);
        } else if !worker.library.is_empty() {
            worker.advance(false);
        }
        worker.publish_status();
        Ok(worker)
    }

    fn run(&mut self, commands: &Receiver<Command>) {
        let mut deadline = Instant::now();
        loop {
            // Sleep out the rest of the frame in the channel, so a command
            // that lands mid-frame is acted on at once rather than after the
            // timer expires.
            loop {
                match commands.recv_deadline(deadline) {
                    Ok(command) => self.handle(command),
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }

            let interval = Duration::from_secs_f64(1.0 / self.fps as f64);
            deadline += interval;
            let now = Instant::now();
            if deadline < now {
                // Behind schedule. Restart the clock instead of trying to
                // catch up, which would just run a burst of frames nobody
                // sees and fall behind again.
                deadline = now + interval;
            }

            self.feed_audio();
            if !self.paused {
                if let Err(message) = self.render() {
                    // The loop ends here and the worker drops on its own
                    // thread, context and all. The status is what the
                    // panel shows over the last frame it got.
                    log::warn!("milkdrop engine stopped: {message}");
                    self.shared.set_status(Status::Failed(message));
                    return;
                }
            }
            self.drain_callbacks();
        }
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Resize { width, height } => {
                let width = width.clamp(MIN_SIZE, MAX_SIZE);
                let height = height.clamp(MIN_SIZE, MAX_SIZE);
                if (width, height) == (self.targets.width, self.targets.height) {
                    return;
                }
                if let Err(message) = self.targets.allocate(&self.gl, width, height) {
                    self.shared.set_status(Status::Failed(message));
                    return;
                }
                unsafe {
                    pm::projectm_set_window_size(self.instance, width as usize, height as usize)
                };
            }
            Command::LoadPreset { path, smooth } => self.load(path, smooth),
            // Next and Previous only ever come from somebody clicking, and
            // the lock is about the visual not changing on its own. Refusing
            // an explicit ask made the menu items look broken. The automatic
            // path checks the lock in `drain_callbacks`, and projectM's own
            // timed switch is off while `projectm_set_preset_locked` is set,
            // so the lock still does its job.
            // Forward retraces a step that was taken back before it picks
            // anything new, and back goes to what was actually on screen
            // before, random picks included: the browser's rule, which is
            // the one people bring with them.
            Command::NextPreset { smooth } => match self.trail.forward() {
                Some(path) => self.replay(path, smooth),
                None => self.advance(smooth),
            },
            Command::PreviousPreset { smooth } => {
                if let Some(path) = self.trail.back() {
                    self.replay(path, smooth);
                }
            }
            Command::SetRotation(rotation) => {
                self.rotation = resolve_rotation(&self.library, &rotation);
            }
            Command::SetLibrary { library, rotation } => self.set_library(library, &rotation),
            Command::SetLocked(locked) => {
                self.locked = locked;
                unsafe { pm::projectm_set_preset_locked(self.instance, locked) };
            }
            Command::SetPresetDuration(seconds) => unsafe {
                pm::projectm_set_preset_duration(self.instance, seconds)
            },
            Command::SetBeatSensitivity(sensitivity) => unsafe {
                pm::projectm_set_beat_sensitivity(self.instance, sensitivity)
            },
            Command::SetHardCut(enabled) => unsafe {
                pm::projectm_set_hard_cut_enabled(self.instance, enabled)
            },
            Command::SetFps(fps) => {
                self.fps = fps.max(1);
                unsafe { pm::projectm_set_fps(self.instance, self.fps as i32) };
            }
            Command::Pause => self.paused = true,
            Command::Resume => self.paused = false,
        }
    }

    /// Everything the feed has written since last time, handed to projectM as
    /// interleaved stereo. `count` in projectM's API is frames per channel,
    /// not floats, which the header says and the ring buffer confirms.
    fn feed_audio(&mut self) {
        self.cursor = self.feed.since(self.cursor, &mut self.pcm);
        let frames = self.pcm.len() / 2;
        if frames == 0 {
            return;
        }
        let mut offset = 0;
        while offset < frames {
            let chunk = (frames - offset).min(self.max_pcm_frames);
            unsafe {
                pm::projectm_pcm_add_float(
                    self.instance,
                    self.pcm[offset * 2..].as_ptr(),
                    chunk as u32,
                    pm::projectm_channels::PROJECTM_STEREO,
                );
            }
            offset += chunk;
        }
    }

    /// One frame: render, start this frame's readback, publish the last
    /// one. `Err` is a readback the driver refuses to map, which ends the
    /// worker; see [`MAP_MISS_LIMIT`].
    fn render(&mut self) -> Result<(), String> {
        let (width, height) = (self.targets.width, self.targets.height);
        let stride = width as usize * 4;
        let bytes = stride * height as usize;

        unsafe {
            (self.gl.Viewport)(0, 0, width as i32, height as i32);
            pm::projectm_opengl_render_frame_fbo(self.instance, self.targets.fbo);

            // Start this frame's readback into one buffer, then map the one
            // the previous frame filled. The GPU gets a whole frame to finish
            // the transfer, which is what keeps the map from being a stall.
            let write = self.targets.index;
            let read = 1 - write;
            (self.gl.BindFramebuffer)(gl::FRAMEBUFFER, self.targets.fbo);
            (self.gl.PixelStorei)(gl::PACK_ALIGNMENT, 1);
            (self.gl.BindBuffer)(gl::PIXEL_PACK_BUFFER, self.targets.pbos[write]);
            (self.gl.ReadPixels)(
                0,
                0,
                width as i32,
                height as i32,
                gl::RGBA,
                gl::UNSIGNED_BYTE,
                std::ptr::null_mut(),
            );
            self.targets.filled[write] = true;
            self.targets.index = read;

            if !self.targets.filled[read] {
                // First frame after a resize: there's nothing in the other
                // buffer yet, so there's nothing to publish.
                (self.gl.BindBuffer)(gl::PIXEL_PACK_BUFFER, 0);
                (self.gl.BindFramebuffer)(gl::FRAMEBUFFER, 0);
                return Ok(());
            }

            let started = Instant::now();
            (self.gl.BindBuffer)(gl::PIXEL_PACK_BUFFER, self.targets.pbos[read]);
            let mapped = (self.gl.MapBufferRange)(
                gl::PIXEL_PACK_BUFFER,
                0,
                bytes as isize,
                gl::MAP_READ_BIT,
            );
            if mapped.is_null() {
                (self.gl.BindBuffer)(gl::PIXEL_PACK_BUFFER, 0);
                (self.gl.BindFramebuffer)(gl::FRAMEBUFFER, 0);
                let error = match self.gl.take_error() {
                    Some(error) => format!(" (gl error {error:#x})"),
                    None => String::new(),
                };
                self.map_misses += 1;
                // Once when it starts, not sixty times a second.
                if self.map_misses == 1 {
                    log::warn!("milkdrop readback could not map the pixel buffer{error}");
                }
                if self.map_misses >= MAP_MISS_LIMIT {
                    return Err(format!(
                        "{} would not map the readback buffer {} frames in a row{error}",
                        self.renderer, self.map_misses
                    ));
                }
                return Ok(());
            }
            self.map_misses = 0;

            self.scratch.resize(bytes, 0);
            let source = std::slice::from_raw_parts(mapped as *const u8, bytes);
            // glReadPixels hands back the bottom row first, textures want the
            // top row first, so the copy out is also the flip.
            for row in 0..height as usize {
                let from = row * stride;
                let to = (height as usize - 1 - row) * stride;
                self.scratch[to..to + stride].copy_from_slice(&source[from..from + stride]);
            }
            (self.gl.UnmapBuffer)(gl::PIXEL_PACK_BUFFER);
            (self.gl.BindBuffer)(gl::PIXEL_PACK_BUFFER, 0);
            (self.gl.BindFramebuffer)(gl::FRAMEBUFFER, 0);

            self.readback_micros = started.elapsed().as_micros() as u64;
        }

        self.publish(width, height);
        Ok(())
    }

    /// Swap the finished pixels into the shared slot and take the previous
    /// buffer back to write the next frame into.
    fn publish(&mut self, width: u32, height: u32) {
        self.seq += 1;
        let frame = Frame {
            width,
            height,
            rgba8: Arc::new(std::mem::take(&mut self.scratch)),
            seq: self.seq,
        };
        let previous = self.shared.frame.lock().unwrap().replace(frame);
        // Release, paired with the panel's Acquire load, so a panel that sees
        // the new seq also sees the frame behind it.
        self.shared.seq.store(self.seq, Ordering::Release);
        self.shared
            .readback_micros
            .store(self.readback_micros, Ordering::Relaxed);
        // Oldest first, so the buffer that has had the longest to come free
        // is the one that gets reused.
        if let Some(waiting) = self.waiting.take() {
            self.reclaim(waiting);
        }
        if let Some(previous) = previous {
            self.reclaim(previous.rgba8);
        }
    }

    /// Take a published buffer back if nothing is reading it any more.
    ///
    /// One spare is the whole pool: the readback writes one buffer and a
    /// consumer holds at most one, so a third would only sit there holding
    /// three megabytes. Anything that doesn't fit is dropped, and the next
    /// `resize` allocates in its place.
    fn reclaim(&mut self, buffer: Arc<Vec<u8>>) {
        match Arc::try_unwrap(buffer) {
            Ok(rgba8) if self.scratch.is_empty() => self.scratch = rgba8,
            Ok(_) => {}
            Err(held) if self.waiting.is_none() => self.waiting = Some(held),
            Err(_) => {}
        }
    }

    fn drain_callbacks(&mut self) {
        for (path, message) in self.callbacks.failures.borrow_mut().drain(..) {
            log::warn!("milkdrop preset {} failed: {message}", path.display());
            self.shared
                .push_event(Event::PresetFailed { path, message });
        }
        if let Some(hard_cut) = self.callbacks.switch_requested.take() {
            if !self.locked {
                self.advance(!hard_cut);
            }
        }
    }

    /// The shuffle, over the rotation rather than the whole library. This is
    /// the one path Next and projectM's timed switch both take, so they can't
    /// drift apart on which set they walk.
    fn advance(&mut self, smooth: bool) {
        if let Some(slot) = self.shuffle.pick(self.rotation.len(), self.current_slot()) {
            self.load_index(self.rotation[slot], smooth);
        }
    }

    /// Where the preset on screen sits in the rotation. `None` when it came
    /// from an explicit pick outside it, which is a fine place to be: the
    /// next step just starts from the end of the rotation.
    fn current_slot(&self) -> Option<usize> {
        let current = self.current?;
        self.rotation.iter().position(|index| *index == current)
    }

    /// Swap in a rescanned library.
    ///
    /// The tricky part is what happens to the preset on screen. Its index is
    /// meaningless against the new list, so it gets looked up by path: still
    /// there and it keeps playing under its new index, gone and the worker
    /// forgets it and lets the next switch pick fresh. The one case worth
    /// handling loudly is a worker that had nothing at all, because that's
    /// the whole reason this command exists: presets landed on disk after the
    /// panel opened, and sitting on projectM's idle preset until something
    /// else happens to call for a switch would read as the rescan not
    /// working.
    fn set_library(&mut self, library: PresetLibrary, rotation: &Rotation) {
        let was_empty = self.library.is_empty();
        self.library = library;
        self.rotation = resolve_rotation(&self.library, rotation);
        // Texture search paths are per instance, not per preset, so a new
        // library's textures folders have to go in now.
        set_texture_paths(self.instance, &self.library);
        self.current = self
            .preset
            .as_deref()
            .and_then(|path| self.library.index_of(path));
        if was_empty && !self.library.is_empty() {
            self.advance(false);
        }
        self.publish_status();
    }

    /// Load a preset by path, through its rotation index when the library
    /// holds it. A preset outside the scanned roots is still loadable; the
    /// panel restores a remembered path this way.
    fn load(&mut self, path: PathBuf, smooth: bool) {
        match self.library.index_of(&path) {
            Some(index) => self.load_index(index, smooth),
            None => self.load_path(path, smooth),
        }
    }

    fn load_index(&mut self, index: usize, smooth: bool) {
        let Some(path) = self.library.presets().get(index).cloned() else {
            return;
        };
        self.current = Some(index);
        self.load_path(path, smooth);
    }

    /// Load a preset and remember it as the newest step, which is every
    /// load except the ones that walk the trail itself.
    fn load_path(&mut self, path: PathBuf, smooth: bool) {
        self.trail.record(path.clone());
        self.play(path, smooth);
    }

    /// Load a preset the trail handed back, without recording it again.
    /// The rotation index is looked up so a following timed switch still
    /// avoids what's on screen.
    fn replay(&mut self, path: PathBuf, smooth: bool) {
        self.current = self.library.index_of(&path);
        self.play(path, smooth);
    }

    fn play(&mut self, path: PathBuf, smooth: bool) {
        let Ok(c_path) = CString::new(path.as_os_str().as_encoded_bytes()) else {
            log::warn!(
                "milkdrop preset path has an interior nul: {}",
                path.display()
            );
            return;
        };
        unsafe { pm::projectm_load_preset_file(self.instance, c_path.as_ptr(), smooth) };
        self.shared.push_event(Event::PresetChanged(path.clone()));
        self.preset = Some(path);
        self.publish_status();
    }

    fn publish_status(&self) {
        self.shared.set_status(Status::Running {
            preset: self.preset.clone(),
            projectm_version: self.version.clone(),
            renderer: self.renderer.clone(),
            gl_version: self.gl_version.clone(),
        });
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Order matters: projectM's own GL objects go first, while the
        // context is still current, then ours, then the context itself when
        // `headless` drops after this.
        unsafe { pm::projectm_destroy(self.instance) };
        self.targets.release(&self.gl);
    }
}

/// The FBO projectM renders into, its colour and depth attachments, and the
/// two pixel buffers the frame is read back through.
#[derive(Default)]
struct Targets {
    fbo: gl::GLuint,
    texture: gl::GLuint,
    depth: gl::GLuint,
    pbos: [gl::GLuint; 2],
    filled: [bool; 2],
    index: usize,
    width: u32,
    height: u32,
}

impl Targets {
    fn allocate(&mut self, gl_fns: &Gl, width: u32, height: u32) -> Result<(), String> {
        self.release(gl_fns);
        let bytes = width as usize * height as usize * 4;

        unsafe {
            (gl_fns.GenTextures)(1, &mut self.texture);
            (gl_fns.BindTexture)(gl::TEXTURE_2D, self.texture);
            (gl_fns.TexImage2D)(
                gl::TEXTURE_2D,
                0,
                gl::RGBA8 as i32,
                width as i32,
                height as i32,
                0,
                gl::RGBA,
                gl::UNSIGNED_BYTE,
                std::ptr::null(),
            );
            (gl_fns.TexParameteri)(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::LINEAR);
            (gl_fns.TexParameteri)(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::LINEAR);
            (gl_fns.TexParameteri)(gl::TEXTURE_2D, gl::TEXTURE_WRAP_S, gl::CLAMP_TO_EDGE);
            (gl_fns.TexParameteri)(gl::TEXTURE_2D, gl::TEXTURE_WRAP_T, gl::CLAMP_TO_EDGE);
            (gl_fns.BindTexture)(gl::TEXTURE_2D, 0);

            // projectM's composite uses the stencil buffer for its border
            // shapes, so the FBO needs one; a depth-stencil renderbuffer is
            // the cheap way to have both.
            (gl_fns.GenRenderbuffers)(1, &mut self.depth);
            (gl_fns.BindRenderbuffer)(gl::RENDERBUFFER, self.depth);
            (gl_fns.RenderbufferStorage)(
                gl::RENDERBUFFER,
                gl::DEPTH24_STENCIL8,
                width as i32,
                height as i32,
            );
            (gl_fns.BindRenderbuffer)(gl::RENDERBUFFER, 0);

            (gl_fns.GenFramebuffers)(1, &mut self.fbo);
            (gl_fns.BindFramebuffer)(gl::FRAMEBUFFER, self.fbo);
            (gl_fns.FramebufferTexture2D)(
                gl::FRAMEBUFFER,
                gl::COLOR_ATTACHMENT0,
                gl::TEXTURE_2D,
                self.texture,
                0,
            );
            (gl_fns.FramebufferRenderbuffer)(
                gl::FRAMEBUFFER,
                gl::DEPTH_STENCIL_ATTACHMENT,
                gl::RENDERBUFFER,
                self.depth,
            );
            let status = (gl_fns.CheckFramebufferStatus)(gl::FRAMEBUFFER);

            // Start black rather than whatever the driver left in the
            // allocation, so a first frame that arrives before projectM has
            // drawn anything isn't noise.
            (gl_fns.ClearColor)(0.0, 0.0, 0.0, 1.0);
            (gl_fns.Clear)(gl::COLOR_BUFFER_BIT | gl::DEPTH_BUFFER_BIT);
            (gl_fns.BindFramebuffer)(gl::FRAMEBUFFER, 0);

            if status != gl::FRAMEBUFFER_COMPLETE {
                self.release(gl_fns);
                return Err(format!(
                    "the graphics driver would not give rox a {width}x{height} render target \
                     (framebuffer status {status:#x})"
                ));
            }

            (gl_fns.GenBuffers)(2, self.pbos.as_mut_ptr());
            for pbo in self.pbos {
                (gl_fns.BindBuffer)(gl::PIXEL_PACK_BUFFER, pbo);
                (gl_fns.BufferData)(
                    gl::PIXEL_PACK_BUFFER,
                    bytes as isize,
                    std::ptr::null(),
                    gl::STREAM_READ,
                );
            }
            (gl_fns.BindBuffer)(gl::PIXEL_PACK_BUFFER, 0);
        }

        self.filled = [false; 2];
        self.index = 0;
        self.width = width;
        self.height = height;
        Ok(())
    }

    fn release(&mut self, gl_fns: &Gl) {
        unsafe {
            if self.fbo != 0 {
                (gl_fns.DeleteFramebuffers)(1, &self.fbo);
                self.fbo = 0;
            }
            if self.texture != 0 {
                (gl_fns.DeleteTextures)(1, &self.texture);
                self.texture = 0;
            }
            if self.depth != 0 {
                (gl_fns.DeleteRenderbuffers)(1, &self.depth);
                self.depth = 0;
            }
            if self.pbos[0] != 0 || self.pbos[1] != 0 {
                (gl_fns.DeleteBuffers)(2, self.pbos.as_ptr());
                self.pbos = [0; 2];
            }
        }
        self.filled = [false; 2];
    }
}

fn version_string() -> String {
    unsafe {
        let raw = pm::projectm_get_version_string();
        if raw.is_null() {
            return "unknown".to_string();
        }
        let version = CStr::from_ptr(raw).to_string_lossy().into_owned();
        pm::projectm_free_string(raw);
        version
    }
}

/// The rotation the worker actually walks, given what the panel asked for.
///
/// A folder that matches nothing is the interesting case: the user renamed or
/// deleted it since the config was written, or the pack moved. Falling back to
/// the whole library keeps Next working, where an empty rotation would pin the
/// panel to whatever happened to be on screen. It warns once here, on the
/// command, rather than every time a frame wants a new preset.
fn resolve_rotation(library: &PresetLibrary, rotation: &Rotation) -> Vec<usize> {
    let indices = library.rotation_indices(rotation);
    if !indices.is_empty() || library.is_empty() {
        return indices;
    }
    match rotation {
        Rotation::Folder(folder) => log::warn!(
            "milkdrop rotation folder {} holds no presets, rotating the whole library instead",
            folder.display()
        ),
        Rotation::Set(paths) => log::warn!(
            "milkdrop favorites rotation matched none of its {} presets, rotating the whole library instead",
            paths.len()
        ),
        Rotation::All => {}
    }
    library.rotation_indices(&Rotation::All)
}

/// The presets loaded in order and where in that list the one on screen
/// sits. A new load from anywhere but the trail itself drops whatever was
/// ahead of the cursor, the way a browser forgets the forward pages once
/// you navigate somewhere new.
#[derive(Default)]
struct Trail {
    paths: Vec<PathBuf>,
    cursor: usize,
}

impl Trail {
    /// How far back it reaches. Past this the oldest step goes; nobody
    /// presses Previous two hundred times.
    const CAP: usize = 200;

    fn record(&mut self, path: PathBuf) {
        if self.paths.get(self.cursor) == Some(&path) {
            return;
        }
        if !self.paths.is_empty() {
            self.paths.truncate(self.cursor + 1);
        }
        self.paths.push(path);
        if self.paths.len() > Self::CAP {
            self.paths.remove(0);
        }
        self.cursor = self.paths.len() - 1;
    }

    fn back(&mut self) -> Option<PathBuf> {
        if self.cursor == 0 {
            return None;
        }
        self.cursor -= 1;
        self.paths.get(self.cursor).cloned()
    }

    fn forward(&mut self) -> Option<PathBuf> {
        if self.cursor + 1 >= self.paths.len() {
            return None;
        }
        self.cursor += 1;
        self.paths.get(self.cursor).cloned()
    }
}

fn set_texture_paths(instance: pm::projectm_handle, library: &PresetLibrary) {
    let owned: Vec<CString> = library
        .textures()
        .iter()
        .filter_map(|path| CString::new(path.as_os_str().as_encoded_bytes()).ok())
        .collect();
    if owned.is_empty() {
        return;
    }
    let paths: Vec<*const c_char> = owned.iter().map(|path| path.as_ptr()).collect();
    unsafe { pm::projectm_set_texture_search_paths(instance, paths.as_ptr(), paths.len()) };
}

/// Hand projectM's own log lines to `log`. Without a callback every `LOG_*`
/// inside libprojectM is a no-op, so the GL probe's summary line, every
/// shader compile error and every texture it couldn't load went nowhere,
/// and a Windows release build has no stderr to catch them either. The
/// callback is independent of any instance and covers every thread, so
/// the thumbnailer's engine gets it for free, and it's set once because a
/// second call would only replace it with itself.
fn forward_projectm_log() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        pm::projectm_set_log_callback(Some(on_log), false, std::ptr::null_mut());
    });
}

/// projectM's fatal and error both land as `error`: fatal is what precedes a
/// null instance, error is a shader that didn't compile, and both are what
/// someone reading a black panel's log is after. Trace and debug only exist
/// in projectM's debug builds and go to `debug` for the one that has them.
unsafe extern "C" fn on_log(
    message: *const c_char,
    level: pm::projectm_log_level,
    _user_data: *mut c_void,
) {
    if message.is_null() {
        return;
    }
    let message = CStr::from_ptr(message).to_string_lossy();
    let level = match level {
        pm::PROJECTM_LOG_LEVEL_FATAL | pm::PROJECTM_LOG_LEVEL_ERROR => log::Level::Error,
        pm::PROJECTM_LOG_LEVEL_WARN => log::Level::Warn,
        pm::PROJECTM_LOG_LEVEL_INFO => log::Level::Info,
        _ => log::Level::Debug,
    };
    log::log!(level, "projectm: {}", message.trim_end());
}

/// The trampoline projectM's glad loads through.
///
/// `user_data` is deliberately unused. projectM's resolver keeps whatever
/// pointer it's given for the whole process and hands it back on every later
/// instance's probe, so anything owned by one engine would dangle the moment
/// that engine goes away. [`context::resolve`] reads a display that outlives
/// them all instead.
unsafe extern "C" fn load_proc(name: *const c_char, _user_data: *mut c_void) -> *mut c_void {
    if name.is_null() {
        return std::ptr::null_mut();
    }
    context::resolve(CStr::from_ptr(name)) as *mut c_void
}

/// Fired from inside projectM's render call. Records and returns; the load
/// happens back in the loop.
unsafe extern "C" fn on_switch_requested(is_hard_cut: bool, user_data: *mut c_void) {
    if user_data.is_null() {
        return;
    }
    let callbacks = &*(user_data as *const Callbacks);
    callbacks.switch_requested.set(Some(is_hard_cut));
}

/// Both strings are only valid for the duration of the call, so they're
/// copied out before anything else happens.
unsafe extern "C" fn on_switch_failed(
    preset_filename: *const c_char,
    message: *const c_char,
    user_data: *mut c_void,
) {
    if user_data.is_null() {
        return;
    }
    let path = if preset_filename.is_null() {
        PathBuf::new()
    } else {
        PathBuf::from(
            CStr::from_ptr(preset_filename)
                .to_string_lossy()
                .into_owned(),
        )
    };
    let message = if message.is_null() {
        "libprojectM did not say why".to_string()
    } else {
        CStr::from_ptr(message).to_string_lossy().into_owned()
    };
    let callbacks = &*(user_data as *const Callbacks);
    if let Ok(mut failures) = callbacks.failures.try_borrow_mut() {
        failures.push((path, message));
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::{c_char, c_void, CStr};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use rox_milkdrop_sys as pm;

    use super::context;
    use super::{resolve_rotation, Trail};
    use crate::{Command, Engine, EngineOptions, Event, PresetLibrary, Rotation, Status};

    fn touch(path: &std::path::Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"").unwrap();
    }

    #[test]
    fn a_folder_rotation_resolves_to_that_folder() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("Dancer/a.milk"));
        touch(&root.join("Fractal/b.milk"));
        touch(&root.join("Fractal/c.milk"));

        let library = PresetLibrary::scan(&[root.to_path_buf()], None);
        let rotation = resolve_rotation(&library, &Rotation::Folder(root.join("Fractal")));
        assert_eq!(rotation, vec![1, 2]);
    }

    #[test]
    fn a_rotation_that_matches_nothing_falls_back_to_the_whole_library() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("Dancer/a.milk"));
        touch(&root.join("Fractal/b.milk"));

        let library = PresetLibrary::scan(&[root.to_path_buf()], None);
        let rotation = resolve_rotation(&library, &Rotation::Folder(root.join("Gone")));
        assert_eq!(rotation, vec![0, 1]);
    }

    #[test]
    fn a_rotation_over_an_empty_library_stays_empty() {
        let library = PresetLibrary::default();
        let rotation = resolve_rotation(
            &library,
            &Rotation::Folder(std::path::PathBuf::from("/nowhere")),
        );
        assert!(rotation.is_empty());
    }

    /// Back returns to what was on screen, forward retraces it, and a new
    /// load from anywhere else drops the forward steps.
    #[test]
    fn the_trail_walks_back_and_forward_like_a_browser() {
        let a = std::path::PathBuf::from("/p/a.milk");
        let b = std::path::PathBuf::from("/p/b.milk");
        let c = std::path::PathBuf::from("/p/c.milk");
        let mut trail = Trail::default();
        assert_eq!(trail.back(), None, "nowhere to go from nothing");
        assert_eq!(trail.forward(), None);

        trail.record(a.clone());
        trail.record(b.clone());
        trail.record(c.clone());
        assert_eq!(trail.back(), Some(b.clone()));
        assert_eq!(trail.back(), Some(a.clone()));
        assert_eq!(trail.back(), None, "the first step is the end of the line");
        assert_eq!(trail.forward(), Some(b.clone()));
        assert_eq!(trail.forward(), Some(c.clone()));
        assert_eq!(trail.forward(), None);

        // Back to a, then somewhere new: b and c are forgotten.
        trail.back();
        trail.back();
        let d = std::path::PathBuf::from("/p/d.milk");
        trail.record(d.clone());
        assert_eq!(trail.forward(), None);
        assert_eq!(trail.back(), Some(a.clone()));
        assert_eq!(trail.forward(), Some(d.clone()));

        // Loading what's already up records nothing, so Previous after a
        // restored preset doesn't land on the same preset again.
        trail.record(d.clone());
        assert_eq!(trail.back(), Some(a));
    }

    /// The trail is bounded, and the oldest step is what goes.
    #[test]
    fn the_trail_forgets_its_oldest_steps_past_the_cap() {
        let mut trail = Trail::default();
        for n in 0..(Trail::CAP + 5) {
            trail.record(std::path::PathBuf::from(format!("/p/{n}.milk")));
        }
        assert_eq!(trail.paths.len(), Trail::CAP);
        assert_eq!(
            trail.paths[0],
            std::path::PathBuf::from("/p/5.milk"),
            "the first five went"
        );
    }

    /// End to end without a UI: context, projectM, one rendered frame.
    ///
    /// Needs a working OpenGL driver. Mesa's llvmpipe is enough and is what a
    /// headless Linux box has, so this runs rather than being ignored. On a
    /// machine with no driver at all the engine reports `Status::Failed`,
    /// which is the documented behaviour, so the test accepts it and stops
    /// rather than failing a build over a missing GPU.
    #[test]
    #[cfg_attr(not(target_os = "linux"), ignore = "needs a GL driver")]
    fn the_engine_renders_a_frame() {
        let feed = Arc::new(rox_viz::AudioFeed::new());
        let engine = Engine::spawn(EngineOptions {
            feed: Arc::clone(&feed),
            library: PresetLibrary::default(),
            preset: None,
            fps: 60,
            width: 64,
            height: 64,
        });

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match engine.status() {
                Status::Running { .. } => break,
                Status::Failed(message) => {
                    eprintln!("no usable OpenGL here, skipping: {message}");
                    return;
                }
                Status::Starting => {
                    assert!(Instant::now() < deadline, "the engine never came up");
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }

        let deadline = Instant::now() + Duration::from_secs(2);
        let frame = loop {
            if let Some(frame) = engine.frame_after(0) {
                break frame;
            }
            assert!(
                Instant::now() < deadline,
                "no frame arrived within two seconds"
            );
            std::thread::sleep(Duration::from_millis(10));
        };

        // 64 was asked for, MIN_SIZE is what the engine will actually give.
        assert_eq!(frame.width, super::MIN_SIZE);
        assert_eq!(frame.height, super::MIN_SIZE);
        assert_eq!(frame.rgba8.len(), (frame.width * frame.height * 4) as usize);
        assert!(frame.seq > 0);

        // A consumer that lets go of each frame before the next one is
        // published gets the same buffer back, over and over. That's the
        // whole point of handing out a handle: at a panel's size a fresh
        // allocation per frame costs more in page faults than the render.
        // Addresses can repeat by luck, so this wants a run of frames, not
        // two.
        //
        // The frame is let go only once the one after it is in hand, which is
        // what the panel does: the texture upload keeps the handle until the
        // renderer has carried the bytes out. Three buffers is the floor for
        // that pattern, one being written, one published, one held, and the
        // point of the assert is that the set stops growing there.
        let mut seen = std::collections::HashSet::new();
        let mut held = frame;
        let deadline = Instant::now() + Duration::from_secs(8);
        for _ in 0..24 {
            let next = loop {
                if let Some(next) = engine.frame_after(held.seq) {
                    break next;
                }
                assert!(Instant::now() < deadline, "the frame stream stopped");
                std::thread::sleep(Duration::from_millis(5));
            };
            seen.insert(next.rgba8.as_ptr() as usize);
            held = next;
        }
        assert!(
            seen.len() <= 3,
            "the worker allocated a buffer per frame instead of recycling: \
             {} distinct buffers over twenty-four frames",
            seen.len()
        );
    }

    /// Close a Milkdrop panel, open another one.
    ///
    /// This is the sequence that used to take the app down, so it's worth
    /// having, but note what it does and doesn't prove. It exercises the
    /// second `spawn` end to end. It does *not* reliably catch the
    /// use-after-free that caused the original crash, because whether a read
    /// through a freed pointer faults depends on what the allocator did with
    /// the block. The deterministic proof of the underlying cause is
    /// `projectm_latches_the_first_load_proc_user_data` below.
    ///
    /// The test is only meaningful with a real driver. Without one both
    /// engines report `Status::Failed` and it stops early, the same as the
    /// smoke test above.
    #[test]
    #[cfg_attr(not(target_os = "linux"), ignore = "needs a GL driver")]
    fn a_second_engine_starts_after_the_first_is_dropped() {
        fn spawn_and_wait() -> Option<Engine> {
            let engine = Engine::spawn(EngineOptions {
                feed: Arc::new(rox_viz::AudioFeed::new()),
                library: PresetLibrary::default(),
                preset: None,
                fps: 60,
                width: 64,
                height: 64,
            });
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match engine.status() {
                    Status::Running { .. } => return Some(engine),
                    Status::Failed(message) => {
                        eprintln!("no usable OpenGL here, skipping: {message}");
                        return None;
                    }
                    Status::Starting => {
                        assert!(Instant::now() < deadline, "the engine never came up");
                        std::thread::sleep(Duration::from_millis(10));
                    }
                }
            }
        }

        let Some(first) = spawn_and_wait() else {
            return;
        };
        drop(first);

        // The worker joins on the last clone dropping, so by here the first
        // engine's context and its boxed state are gone. This is the call
        // that used to die.
        let second = spawn_and_wait().expect("the first engine ran, so the second has a driver");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if second.frame_after(0).is_some() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the second engine came up but never rendered"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// projectM's GL resolver latches the first load proc and user pointer it
    /// is ever given, and ignores both arguments on every later instance.
    ///
    /// This is the reason [`load_proc`] resolves through a process-lifetime
    /// display instead of a pointer to the engine that happens to be starting.
    /// It is asserted here rather than argued in a comment, because it is a
    /// property of the vendored libprojectM rather than of our code: if a
    /// future bump makes `Initialize` adopt the new callback, this test goes
    /// red and the workaround can be reconsidered.
    ///
    /// See `Renderer/Platform/GLResolver.cpp`, the `if (m_loaded) return
    /// true;` at the top of `GLResolver::Initialize`.
    ///
    /// Deliberately written not to care whether it is the first test in this
    /// binary to touch projectM. It latches the resolver itself, with the
    /// real trampoline, before offering a different one.
    #[test]
    #[cfg_attr(not(target_os = "linux"), ignore = "needs a GL driver")]
    fn projectm_latches_the_first_load_proc_user_data() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        /// What the second instance handed our replacement trampoline, or
        /// zero if it never called it at all. Both outcomes mean the same
        /// thing: the resolver is not listening to us any more.
        static SEEN: AtomicUsize = AtomicUsize::new(0);
        /// A sentinel, never dereferenced, only compared.
        const SECOND: usize = 0x2222;

        unsafe extern "C" fn recording_proc(
            name: *const c_char,
            user_data: *mut c_void,
        ) -> *mut c_void {
            SEEN.store(user_data as usize, Ordering::Relaxed);
            if name.is_null() {
                return std::ptr::null_mut();
            }
            context::resolve(CStr::from_ptr(name)) as *mut c_void
        }

        // A context has to be current on this thread before projectM will
        // resolve anything, and it has to outlive both instances.
        let Ok(headless) = context::create() else {
            eprintln!("no usable OpenGL here, skipping");
            return;
        };
        assert!(headless.is_current());

        // Make sure the resolver is latched, by us if nobody beat us to it.
        let first = unsafe {
            pm::projectm_create_with_opengl_load_proc(Some(super::load_proc), std::ptr::null_mut())
        };
        if first.is_null() {
            eprintln!("libprojectM would not start here, skipping");
            return;
        }
        unsafe { pm::projectm_destroy(first) };

        // Now offer a different callback and a different pointer. Neither
        // should reach the resolver.
        SEEN.store(0, Ordering::Relaxed);
        let second = unsafe {
            pm::projectm_create_with_opengl_load_proc(Some(recording_proc), SECOND as *mut c_void)
        };
        assert!(!second.is_null(), "the second instance should still start");
        let seen = SEEN.load(Ordering::Relaxed);
        unsafe { pm::projectm_destroy(second) };

        assert_ne!(
            seen, SECOND,
            "libprojectM adopted the second instance's load proc and user data. \
             If that is a deliberate upstream fix, load_proc no longer needs \
             the process-lifetime resolver in context.rs"
        );
    }

    /// Narrow the rotation, then step through it: every switch has to land
    /// inside the folder.
    ///
    /// The presets here are empty files, so libprojectM will refuse them and
    /// the panel would see `PresetFailed` for each. That's fine for what this
    /// asserts, which is which paths the worker chose, not whether they
    /// rendered. Same driver caveat as the tests above.
    #[test]
    #[cfg_attr(not(target_os = "linux"), ignore = "needs a GL driver")]
    fn a_rotation_keeps_every_switch_inside_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for name in ["a.milk", "b.milk", "c.milk"] {
            touch(&root.join("Dancer").join(name));
        }
        for name in ["d.milk", "e.milk", "f.milk"] {
            touch(&root.join("Fractal").join(name));
        }

        let library = PresetLibrary::scan(&[root.to_path_buf()], None);
        assert_eq!(library.presets().len(), 6);

        let engine = Engine::spawn(EngineOptions {
            feed: Arc::new(rox_viz::AudioFeed::new()),
            library,
            preset: None,
            fps: 60,
            width: 64,
            height: 64,
        });

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match engine.status() {
                Status::Running { .. } => break,
                Status::Failed(message) => {
                    eprintln!("no usable OpenGL here, skipping: {message}");
                    return;
                }
                Status::Starting => {
                    assert!(Instant::now() < deadline, "the engine never came up");
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }

        // Push the timed switch out of the way so the only preset changes
        // from here on are the ones this test asks for, then flush the
        // startup pick out of the event buffer.
        engine.send(Command::SetPresetDuration(3600.0));
        std::thread::sleep(Duration::from_millis(100));
        engine.take_events();

        // Every fresh pick lands in the folder. Previous isn't a pick: it
        // retraces the trail, and the trail starts at the preset the
        // engine came up on, before the rotation narrowed, so only Next
        // is asked here.
        engine.send(Command::SetRotation(Rotation::Folder(root.join("Fractal"))));
        for _ in 0..10 {
            engine.send(Command::NextPreset { smooth: false });
        }
        std::thread::sleep(Duration::from_millis(300));

        let mut picked = Vec::new();
        for event in engine.take_events() {
            if let Event::PresetChanged(path) = event {
                assert!(
                    path.starts_with(root.join("Fractal")),
                    "rotation leaked outside the folder: {}",
                    path.display()
                );
                picked.push(path);
            }
        }
        assert!(picked.len() >= 2, "the engine never switched preset");

        // Back goes to the pick before the last one, whatever the sorted
        // order says, and forward comes back to the last one.
        engine.send(Command::PreviousPreset { smooth: false });
        engine.send(Command::NextPreset { smooth: false });
        std::thread::sleep(Duration::from_millis(200));
        let retraced: Vec<_> = engine
            .take_events()
            .into_iter()
            .filter_map(|event| match event {
                Event::PresetChanged(path) => Some(path),
                _ => None,
            })
            .collect();
        assert_eq!(
            retraced,
            vec![
                picked[picked.len() - 2].clone(),
                picked[picked.len() - 1].clone()
            ],
            "previous then next should retrace the last two picks"
        );
    }

    /// Presets that land on disk after the panel opened still get played.
    ///
    /// This is the case Andrew hit: the panel scanned an empty directory at
    /// startup, he unpacked a pack into it, and the settings list caught up
    /// on rescan while the worker kept walking the empty snapshot it was
    /// spawned with. A rescan that only fixes the count is worse than no
    /// rescan, because it looks like it worked.
    #[test]
    #[cfg_attr(not(target_os = "linux"), ignore = "needs a GL driver")]
    fn a_rescanned_library_reaches_a_worker_that_started_empty() {
        let dir = tempfile::tempdir().unwrap();
        // projectM will reject these as presets, which is fine: the worker
        // announces the preset it asked for and reports the parse failure
        // separately, and it's the asking this test is about.
        for name in ["a.milk", "b.milk"] {
            std::fs::write(dir.path().join(name), "").unwrap();
        }

        let engine = Engine::spawn(EngineOptions {
            feed: Arc::new(rox_viz::AudioFeed::new()),
            library: PresetLibrary::default(),
            preset: None,
            fps: 60,
            width: 64,
            height: 64,
        });

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match engine.status() {
                Status::Running { .. } => break,
                Status::Failed(message) => {
                    eprintln!("no usable OpenGL here, skipping: {message}");
                    return;
                }
                Status::Starting => {
                    assert!(Instant::now() < deadline, "the engine never came up");
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }

        // An empty library has nothing to announce, so anything after this
        // point came from the rescan.
        engine.take_events();
        engine.send(Command::SetLibrary {
            library: PresetLibrary::scan(&[dir.path().to_path_buf()], None),
            rotation: Rotation::All,
        });

        let deadline = Instant::now() + Duration::from_secs(5);
        let changed = loop {
            let found = engine
                .take_events()
                .into_iter()
                .find_map(|event| match event {
                    Event::PresetChanged(path) => Some(path),
                    Event::PresetFailed { .. } => None,
                });
            if let Some(path) = found {
                break path;
            }
            assert!(
                Instant::now() < deadline,
                "the worker took a new library but never loaded anything from it"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(
            changed.parent(),
            Some(dir.path()),
            "the preset should have come from the rescanned directory"
        );
    }

    /// The lock stops the visual changing on its own. It doesn't stop you.
    ///
    /// Andrew locked a preset and then found Next, Previous and Random in
    /// the panel menu did nothing at all, which reads as three broken menu
    /// items rather than as the lock working. projectM's own timed switch is
    /// held off by `projectm_set_preset_locked` and the beat-driven one by
    /// the check in `drain_callbacks`, so an explicit command doesn't need
    /// to be refused as well.
    #[test]
    #[cfg_attr(not(target_os = "linux"), ignore = "needs a GL driver")]
    fn a_locked_engine_still_takes_an_explicit_preset_change() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["a.milk", "b.milk", "c.milk"] {
            std::fs::write(dir.path().join(name), "").unwrap();
        }

        let engine = Engine::spawn(EngineOptions {
            feed: Arc::new(rox_viz::AudioFeed::new()),
            library: PresetLibrary::scan(&[dir.path().to_path_buf()], None),
            preset: None,
            fps: 60,
            width: 64,
            height: 64,
        });

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match engine.status() {
                Status::Running { .. } => break,
                Status::Failed(message) => {
                    eprintln!("no usable OpenGL here, skipping: {message}");
                    return;
                }
                Status::Starting => {
                    assert!(Instant::now() < deadline, "the engine never came up");
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }

        // Nothing may switch on its own from here, so every preset change
        // after this point is one the test asked for.
        engine.send(Command::SetLocked(true));
        engine.send(Command::SetPresetDuration(3600.0));
        std::thread::sleep(Duration::from_millis(100));
        engine.take_events();

        for command in [
            Command::NextPreset { smooth: true },
            Command::PreviousPreset { smooth: true },
            Command::NextPreset { smooth: false },
        ] {
            engine.send(command);
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let changed = engine
                    .take_events()
                    .into_iter()
                    .any(|event| matches!(event, Event::PresetChanged(_)));
                if changed {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "a locked engine swallowed an explicit preset change"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    /// A restored preset is the first and only thing the worker loads. It
    /// used to shuffle one in the constructor and take the restore as a
    /// command after, so the owner saw two switches at start. The backdrop
    /// writes what it sees to settings while locked, kept the random one
    /// out of that pair, and came up somewhere else on every restart.
    #[test]
    #[cfg_attr(not(target_os = "linux"), ignore = "needs a GL driver")]
    fn a_worker_spawned_with_a_preset_comes_up_on_it_without_a_shuffle_first() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["a.milk", "b.milk", "c.milk"] {
            std::fs::write(dir.path().join(name), "").unwrap();
        }
        let restore = dir.path().join("b.milk");

        let engine = Engine::spawn(EngineOptions {
            feed: Arc::new(rox_viz::AudioFeed::new()),
            library: PresetLibrary::scan(&[dir.path().to_path_buf()], None),
            preset: Some(restore.clone()),
            fps: 60,
            width: 64,
            height: 64,
        });
        engine.send(Command::SetLocked(true));

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match engine.status() {
                Status::Running { .. } => break,
                Status::Failed(message) => {
                    eprintln!("no usable OpenGL here, skipping: {message}");
                    return;
                }
                Status::Starting => {
                    assert!(Instant::now() < deadline, "the engine never came up");
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
        // Long enough for a stray shuffle to have been announced too.
        std::thread::sleep(Duration::from_millis(100));

        let changed: Vec<_> = engine
            .take_events()
            .into_iter()
            .filter_map(|event| match event {
                Event::PresetChanged(path) => Some(path),
                Event::PresetFailed { .. } => None,
            })
            .collect();
        assert_eq!(
            changed,
            vec![restore],
            "the restored preset should be the one and only load at start"
        );
    }
}
