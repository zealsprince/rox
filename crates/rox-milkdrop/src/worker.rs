//! The render thread: everything that touches OpenGL or libprojectM.
//!
//! One thread owns the GL context for its whole life. A context is current on
//! exactly one thread and projectM keeps state in it, so nothing else ever
//! gets in: the panel talks through a channel and reads what's published, and
//! no code outside this file holds a projectM handle.
//!
//! projectM's preset callbacks fire from inside its render call. They only
//! record; the load happens back in the loop, a frame later, rather than
//! reentering state the render call is walking.

use std::ffi::{CStr, CString, c_char, c_void};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError};
use rox_milkdrop_sys as pm;

use crate::context::{self, HeadlessGl};
use crate::gl::{self, Gl};
use crate::library::Shuffle;
use crate::{Command, EngineOptions, Event, Frame, PresetLibrary, Rotation, Shared, Status};

/// The floor stops a mid-drag sliver producing a degenerate FBO; the ceiling
/// is where a readback stops being affordable.
const MIN_SIZE: u32 = 128;
const MAX_SIZE: u32 = 4096;

/// Matching `MilkdropConfig`, until the panel sends its real values.
const DEFAULT_PRESET_DURATION: f64 = 30.0;
const DEFAULT_BEAT_SENSITIVITY: f32 = 1.0;

/// Thirty refused maps in a row (half a second) is a driver that never will;
/// without a limit the panel sits black with a warning per frame.
const MAP_MISS_LIMIT: u32 = 30;

/// Boxed and kept alive as long as the instance: projectM holds the pointer.
#[derive(Default)]
struct Callbacks {
    switch_requested: std::cell::Cell<Option<bool>>,
    failures: std::cell::RefCell<Vec<(PathBuf, String)>>,
}

pub(crate) fn run(options: EngineOptions, commands: Receiver<Command>, shared: Arc<Shared>) {
    // Counted in before the context exists and out after it's gone, so every
    // GL call sits inside the count. See `exit_guard`.
    exit_guard::enter();
    match Worker::start(options, shared.clone()) {
        // The worker's drop is the GL teardown, so `leave` comes after the match.
        Ok(mut worker) => worker.run(&commands),
        Err(message) => {
            log::warn!("milkdrop engine did not start: {message}");
            shared.set_status(Status::Failed(message));
        }
    }
    exit_guard::leave();
}

struct Worker {
    shared: Arc<Shared>,
    feed: Arc<rox_viz::AudioFeed>,
    library: PresetLibrary,
    shuffle: Shuffle,
    /// Indices into `library.presets()`; the whole library when the panel's
    /// rotation matched nothing.
    rotation: Vec<usize>,

    /// Dropping it releases the GL context, on this thread.
    _headless: Box<HeadlessGl>,
    gl: Gl,
    instance: pm::projectm_handle,
    callbacks: Box<Callbacks>,
    version: String,
    renderer: String,
    gl_version: String,
    map_misses: u32,

    targets: Targets,
    /// Buffers come back here once consumers let go.
    scratch: Vec<u8>,
    /// A published buffer still being read when the next frame landed, kept
    /// one more publish so a consumer holding the newest frame doesn't force an
    /// allocation per frame.
    waiting: Option<Arc<Vec<u8>>>,
    pcm: Vec<f32>,
    cursor: u64,

    current: Option<usize>,
    /// So Previous and Next mean back and forward, like a browser.
    trail: Trail,
    preset: Option<PathBuf>,
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
            // projectM's own reason already went to the log.
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

        // Start at what the feed already holds, so a panel opened mid-track
        // doesn't feed a second of stale audio to the beat detector.
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

        // With nothing to load, projectM renders its built-in idle preset.
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
            // The process is exiting under us; the handler waits out the one
            // frame this check can lag. See `exit_guard`.
            if exit_guard::exiting() {
                exit_guard::park();
            }

            // Sleep out the frame in the channel, so commands act at once.
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
                // Behind schedule: restart the clock rather than burst to catch up.
                deadline = now + interval;
            }

            self.feed_audio();
            if !self.paused
                && let Err(message) = self.render()
            {
                // The status is what the panel shows over the last frame.
                log::warn!("milkdrop engine stopped: {message}");
                self.shared.set_status(Status::Failed(message));
                return;
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
            // Explicit Next and Previous ignore the lock, which is about the
            // visual not changing on its own; refusing them looks broken. The
            // automatic paths check it (`drain_callbacks`, and
            // `projectm_set_preset_locked`). Forward retraces a step taken
            // back before picking anything new.
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

    /// `count` in projectM's API is frames per channel, not floats.
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

    /// `Err` is a readback the driver refuses to map; see [`MAP_MISS_LIMIT`].
    fn render(&mut self) -> Result<(), String> {
        // A refused resize leaves no targets but the old size: going on would
        // read back into a null pointer. The failed resize already published
        // its status; idle under it.
        if self.targets.fbo == 0 {
            return Ok(());
        }

        let (width, height) = (self.targets.width, self.targets.height);
        let stride = width as usize * 4;
        let bytes = stride * height as usize;

        unsafe {
            (self.gl.Viewport)(0, 0, width as i32, height as i32);
            pm::projectm_opengl_render_frame_fbo(self.instance, self.targets.fbo);

            // Read this frame into one buffer and map the one the last frame
            // filled: the GPU gets a whole frame, so the map doesn't stall.
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
                // First frame after a resize: nothing to publish yet.
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
            // The map is where Mesa first names a pixel format, so its exit
            // handlers are registered by now and ours can go after them.
            exit_guard::arm();
            if mapped.is_null() {
                (self.gl.BindBuffer)(gl::PIXEL_PACK_BUFFER, 0);
                (self.gl.BindFramebuffer)(gl::FRAMEBUFFER, 0);
                let error = match self.gl.take_error() {
                    Some(error) => format!(" (gl error {error:#x})"),
                    None => String::new(),
                };
                self.map_misses += 1;
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
            // glReadPixels returns bottom-up, so the copy out is also the flip.
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

    fn publish(&mut self, width: u32, height: u32) {
        self.seq += 1;
        let frame = Frame {
            width,
            height,
            rgba8: Arc::new(std::mem::take(&mut self.scratch)),
            seq: self.seq,
        };
        let previous = self.shared.frame.lock().unwrap().replace(frame);
        // Release pairs with the panel's Acquire, so a new seq implies its frame.
        self.shared.seq.store(self.seq, Ordering::Release);
        self.shared
            .readback_micros
            .store(self.readback_micros, Ordering::Relaxed);
        // Oldest first: it's had the longest to come free.
        if let Some(waiting) = self.waiting.take() {
            self.reclaim(waiting);
        }
        if let Some(previous) = previous {
            self.reclaim(previous.rgba8);
        }
    }

    /// One spare is the whole pool: the readback writes one buffer and a
    /// consumer holds at most one.
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
        if let Some(hard_cut) = self.callbacks.switch_requested.take()
            && !self.locked
        {
            self.advance(!hard_cut);
        }
    }

    /// The one path Next and the timed switch share, so they walk the same set.
    fn advance(&mut self, smooth: bool) {
        if let Some(slot) = self.shuffle.pick(self.rotation.len(), self.current_slot()) {
            self.load_index(self.rotation[slot], smooth);
        }
    }

    /// `None` after an explicit pick outside the rotation.
    fn current_slot(&self) -> Option<usize> {
        let current = self.current?;
        self.rotation.iter().position(|index| *index == current)
    }

    /// The preset on screen is looked up by path in the new list. A worker
    /// that had nothing starts playing at once, or the rescan looks broken.
    fn set_library(&mut self, library: PresetLibrary, rotation: &Rotation) {
        let was_empty = self.library.is_empty();
        self.library = library;
        self.rotation = resolve_rotation(&self.library, rotation);
        // Texture search paths are per instance.
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

    /// A preset outside the scanned roots still loads (the panel restores one this way).
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

    fn load_path(&mut self, path: PathBuf, smooth: bool) {
        self.trail.record(path.clone());
        self.play(path, smooth);
    }

    /// Load without recording. The index lookup keeps the next timed switch
    /// off what's on screen.
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
        // Order matters: projectM's GL objects go first while the context is
        // current, then ours, then the context when `headless` drops.
        unsafe { pm::projectm_destroy(self.instance) };
        self.targets.release(&self.gl);
    }
}

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

            // projectM's composite needs a stencil buffer for its border shapes.
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

            // Start black, so an early frame isn't driver noise.
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

/// A rotation that matches nothing (a renamed or deleted folder) falls back
/// to the whole library, so Next keeps working. Warns once, here.
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

/// Loaded presets in order and the cursor into them. A new load drops what
/// was ahead of the cursor, like a browser's forward pages.
#[derive(Default)]
struct Trail {
    paths: Vec<PathBuf>,
    cursor: usize,
}

impl Trail {
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

/// Without a callback every `LOG_*` in libprojectM is a no-op, shader errors
/// included, and a Windows release build has no stderr. Process-wide, so set
/// once.
fn forward_projectm_log() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        pm::projectm_set_log_callback(Some(on_log), false, std::ptr::null_mut());
    });
}

/// Fatal and error both map to `error`: they're what a black panel's log
/// needs.
unsafe extern "C" fn on_log(
    message: *const c_char,
    level: pm::projectm_log_level,
    _user_data: *mut c_void,
) {
    if message.is_null() {
        return;
    }

    let message = unsafe { CStr::from_ptr(message) }.to_string_lossy();
    let level = match level {
        pm::PROJECTM_LOG_LEVEL_FATAL | pm::PROJECTM_LOG_LEVEL_ERROR => log::Level::Error,
        pm::PROJECTM_LOG_LEVEL_WARN => log::Level::Warn,
        pm::PROJECTM_LOG_LEVEL_INFO => log::Level::Info,
        _ => log::Level::Debug,
    };

    log::log!(level, "projectm: {}", message.trim_end());
}

/// Never use `user_data`: projectM's resolver keeps the first pointer for the
/// whole process, so anything one engine owns would dangle once it's gone.
/// [`context::resolve`] reads a display that outlives them all.
unsafe extern "C" fn load_proc(name: *const c_char, _user_data: *mut c_void) -> *mut c_void {
    if name.is_null() {
        return std::ptr::null_mut();
    }

    context::resolve(unsafe { CStr::from_ptr(name) }) as *mut c_void
}

unsafe extern "C" fn on_switch_requested(is_hard_cut: bool, user_data: *mut c_void) {
    if user_data.is_null() {
        return;
    }

    let callbacks = unsafe { &*(user_data as *const Callbacks) };
    callbacks.switch_requested.set(Some(is_hard_cut));
}

/// Both strings are only valid for the call, so they're copied out first.
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
            unsafe { CStr::from_ptr(preset_filename) }
                .to_string_lossy()
                .into_owned(),
        )
    };
    let message = if message.is_null() {
        "libprojectM did not say why".to_string()
    } else {
        unsafe { CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned()
    };

    let callbacks = unsafe { &*(user_data as *const Callbacks) };
    if let Ok(mut failures) = callbacks.failures.try_borrow_mut() {
        failures.push((path, message));
    }
}

/// Getting the render thread off GL before the process finishes exiting.
///
/// `exit()` runs C exit handlers on the calling thread while others keep
/// running. Mesa's handlers join its queue threads and free the table
/// `glReadPixels` looks pixel layouts up in, so a worker still rendering dies
/// in Mesa's hash table. Nothing drops the engines on the way out: the
/// backdrop holds one in a static, the panel in an entity, and gpui unwinds
/// neither. So the handler sets a latch and waits for every live worker to
/// stand down; each worker checks it once a frame.
///
/// Handlers run in reverse registration order, so ours must register after
/// Mesa's, which register lazily (the queue handler when the screen comes up,
/// the format table on the first readback). Hence arming from the first
/// readback, not `Engine::spawn`. A handler Mesa registers later still slips
/// underneath.
mod exit_guard {
    use std::sync::Once;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    /// Many frames of slack; past it the quit goes ahead rather than hang on a
    /// worker wedged in the driver.
    const STAND_DOWN: Duration = Duration::from_millis(500);

    static EXITING: AtomicBool = AtomicBool::new(false);
    static LIVE: AtomicUsize = AtomicUsize::new(0);
    static ARMED: Once = Once::new();

    // Declared rather than pulling in libc for one C89 function.
    unsafe extern "C" {
        fn atexit(handler: extern "C" fn()) -> i32;
    }

    extern "C" fn on_exit() {
        EXITING.store(true, Ordering::Release);

        let deadline = Instant::now() + STAND_DOWN;
        while LIVE.load(Ordering::Acquire) > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Once per process; the timing of the first call matters (see the module note).
    pub(super) fn arm() {
        ARMED.call_once(|| {
            // A refusal means we're already inside `exit()`; nothing left to do.
            unsafe { atexit(on_exit) };
        });
    }

    pub(super) fn enter() {
        LIVE.fetch_add(1, Ordering::AcqRel);
    }

    pub(super) fn leave() {
        LIVE.fetch_sub(1, Ordering::AcqRel);
    }

    pub(super) fn exiting() -> bool {
        EXITING.load(Ordering::Acquire)
    }

    /// Returning would drop the `Worker`, whose drop is more GL than an exiting
    /// process has left.
    pub(super) fn park() -> ! {
        leave();
        loop {
            std::thread::park();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::{CStr, c_char, c_void};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use rox_milkdrop_sys as pm;

    use super::context;
    use super::{Trail, resolve_rotation};
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

        trail.back();
        trail.back();
        let d = std::path::PathBuf::from("/p/d.milk");
        trail.record(d.clone());
        assert_eq!(trail.forward(), None);
        assert_eq!(trail.back(), Some(a.clone()));
        assert_eq!(trail.forward(), Some(d.clone()));

        // Loading what's already up records nothing.
        trail.record(d.clone());
        assert_eq!(trail.back(), Some(a));
    }

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

    /// End to end: context, projectM, one frame. llvmpipe is enough; with no
    /// driver at all the engine reports `Status::Failed` and the test stops.
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

        assert_eq!(frame.width, super::MIN_SIZE);
        assert_eq!(frame.height, super::MIN_SIZE);
        assert_eq!(frame.rgba8.len(), (frame.width * frame.height * 4) as usize);
        assert!(frame.seq > 0);

        // A consumer that lets go of each frame once it holds the next (what
        // the panel does) must cycle a fixed set of buffers. Three is the floor:
        // one written, one published, one held. A run of frames, since
        // addresses can repeat by luck.
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

    /// Close a Milkdrop panel, open another: the sequence that crashes if
    /// projectM resolves through the dropped engine (see `context::RESOLVER`).
    /// It won't reliably catch the use-after-free itself; the deterministic
    /// check is `projectm_latches_the_first_load_proc_user_data`.
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

    /// projectM's GL resolver latches the first load proc and user pointer
    /// (`GLResolver::Initialize`'s `if (m_loaded) return true;`) and ignores
    /// later ones. The reason [`load_proc`] never uses `user_data`; if a bump
    /// changes this, the test goes red. Latches the resolver itself first, so
    /// test order doesn't matter.
    #[test]
    #[cfg_attr(not(target_os = "linux"), ignore = "needs a GL driver")]
    fn projectm_latches_the_first_load_proc_user_data() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        /// What the second instance handed our trampoline, or zero if it never called.
        static SEEN: AtomicUsize = AtomicUsize::new(0);
        const SECOND: usize = 0x2222;

        unsafe extern "C" fn recording_proc(
            name: *const c_char,
            user_data: *mut c_void,
        ) -> *mut c_void {
            SEEN.store(user_data as usize, Ordering::Relaxed);
            if name.is_null() {
                return std::ptr::null_mut();
            }

            context::resolve(unsafe { CStr::from_ptr(name) }) as *mut c_void
        }

        // The context has to be current and outlive both instances.
        let Ok(headless) = context::create() else {
            eprintln!("no usable OpenGL here, skipping");
            return;
        };
        assert!(headless.is_current());

        let first = unsafe {
            pm::projectm_create_with_opengl_load_proc(Some(super::load_proc), std::ptr::null_mut())
        };
        if first.is_null() {
            eprintln!("libprojectM would not start here, skipping");
            return;
        }
        unsafe { pm::projectm_destroy(first) };

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

    /// Every switch lands inside the narrowed folder. The presets are empty
    /// files projectM refuses; the test is about which paths get chosen.
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

        // Push the timed switch out of the way, then flush the startup pick.
        engine.send(Command::SetPresetDuration(3600.0));
        std::thread::sleep(Duration::from_millis(100));
        engine.take_events();

        // Only Next: Previous retraces a trail that starts before the rotation
        // narrowed.
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

    /// Presets unpacked after the panel opened get played after a rescan, not
    /// just counted.
    #[test]
    #[cfg_attr(not(target_os = "linux"), ignore = "needs a GL driver")]
    fn a_rescanned_library_reaches_a_worker_that_started_empty() {
        let dir = tempfile::tempdir().unwrap();
        // projectM rejects these; the test is about what the worker asks for.
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

    /// The lock stops the visual changing on its own; explicit Next, Previous
    /// and Random still work.
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

    /// A restored preset is the first and only thing the worker loads, with
    /// no shuffle before it. `EngineOptions::preset` says why.
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
