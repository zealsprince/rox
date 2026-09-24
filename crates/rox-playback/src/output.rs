//! The output seam (ADR 9, ADR 19) and cpal's shared-mode backend. A backend
//! gets the shared atomics, hands back the ring's producer end and the PCM
//! tap's consumer end, and reports what the device accepted.
//!
//! The hard line is in [`fill`], which every backend calls: pop a
//! pre-allocated ring, read atomics, write the device buffer. No allocation,
//! no lock, no logging, no I/O.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream, StreamConfig};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::shared::Shared;

#[cfg(target_os = "linux")]
mod alsa;
#[cfg(target_os = "macos")]
mod coreaudio;
// Not cfg-gated: the format ladder and period math are plain Rust with tests
// that should run on every platform.
mod wasapi;

const RING_SECS: f64 = 0.5;
/// Right after exclusive releases a device, CoreAudio refuses queries while
/// it relocks the clock (OSStatus 56), so a shared open retries for a second
/// instead of failing the session on a transient.
const OPEN_TRIES: u32 = 20;
const OPEN_STEP: Duration = Duration::from_millis(50);
/// Small, so a slow tap consumer loses samples rather than backpressuring.
const TAP_SAMPLES: usize = 16384;

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
const NO_EXCLUSIVE: &str = "exclusive output isn't built for this platform yet";

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Through the system mixer, which owns the rate.
    #[default]
    Shared,
    /// The device claimed for rox alone, at the file's rate where it takes one.
    Exclusive,
}

/// Ids are backend-scoped (a cpal name in shared, a platform id in
/// exclusive) and never interchangeable.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Device {
    pub id: String,
    pub name: String,
}

/// Nothing here is a promise; [`Negotiated`] is what came back.
// No `Eq`: the period is a float.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct Request {
    pub mode: Mode,
    /// None, or an id that's gone, means the system default.
    pub device: Option<String>,
    /// Exclusive only; shared runs at the mixer's rate.
    pub rate: Option<u32>,
    /// `f32`, `s32`, `s16`, or None for the widest. A name the device won't take
    /// falls back to the widest. Exclusive only.
    pub format: Option<String>,
    /// Milliseconds, or None for the backend default. Exclusive only.
    pub period_ms: Option<f64>,
}

/// What the device actually accepted, for the UI to state (ADR 19: an
/// unchecked bit-perfect claim is decoration).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Negotiated {
    pub mode: Mode,
    pub device: String,
    pub sample_rate: u32,
    pub channels: u16,
    /// cpal's spelling for shared, the [`Request::format`] names for exclusive.
    pub format: String,
    /// Some: shared is standing in, and this is why exclusive failed.
    pub fallback: Option<String>,
}

/// Held to keep audio running; dropping it releases the device.
pub trait OutputStream {}

impl OutputStream for Stream {}

pub struct OpenOutput {
    /// Dropping it stops audio.
    pub stream: Box<dyn OutputStream>,
    pub negotiated: Negotiated,
    pub sample_rate: u32,
    pub ring_frames: usize,
    pub producer: Producer<f32>,
    pub tap: Consumer<f32>,
}

/// Separate lists per mode because the ids don't cross.
pub fn devices(mode: Mode) -> Vec<Device> {
    match mode {
        Mode::Shared => shared_devices(),
        Mode::Exclusive => exclusive_devices(),
    }
}

/// Open output per the request. Exclusive that can't claim its device comes
/// back as shared with the reason recorded, never as silence (ADR 19).
pub fn open(request: &Request, shared: &Arc<Shared>) -> Result<OpenOutput, String> {
    if request.mode == Mode::Exclusive {
        match open_exclusive(request, shared) {
            Ok(out) => return Ok(out),
            Err(e) => {
                log::warn!("exclusive output: {e}; falling back to shared");
                let mut out = open_shared_settled(request, shared)?;
                out.negotiated.fallback = Some(e);
                return Ok(out);
            }
        }
    }
    open_shared_settled(request, shared)
}

fn open_shared_settled(request: &Request, shared: &Arc<Shared>) -> Result<OpenOutput, String> {
    let mut tries = 0;
    loop {
        match open_shared(request, shared) {
            Ok(out) => return Ok(out),
            Err(e) => {
                tries += 1;
                if tries >= OPEN_TRIES {
                    return Err(e);
                }
                log::warn!("audio output: {e}; retrying");
                std::thread::sleep(OPEN_STEP);
            }
        }
    }
}

/// Shared by every backend so the ring depth is one rule.
fn rings(
    rate: u32,
) -> (
    usize,
    Producer<f32>,
    Consumer<f32>,
    Producer<f32>,
    Consumer<f32>,
) {
    let ring_frames = (rate as f64 * RING_SECS) as usize;
    let (producer, ring) = RingBuffer::<f32>::new(ring_frames * 2);
    let (tap_tx, tap) = RingBuffer::<f32>::new(TAP_SAMPLES);
    (ring_frames, producer, ring, tap_tx, tap)
}

#[cfg(target_os = "linux")]
fn open_exclusive(request: &Request, shared: &Arc<Shared>) -> Result<OpenOutput, String> {
    alsa::open(request, shared)
}

#[cfg(target_os = "macos")]
fn open_exclusive(request: &Request, shared: &Arc<Shared>) -> Result<OpenOutput, String> {
    coreaudio::open(request, shared)
}

#[cfg(target_os = "windows")]
fn open_exclusive(request: &Request, shared: &Arc<Shared>) -> Result<OpenOutput, String> {
    wasapi::open(request, shared)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn open_exclusive(_request: &Request, _shared: &Arc<Shared>) -> Result<OpenOutput, String> {
    Err(NO_EXCLUSIVE.into())
}

#[cfg(target_os = "linux")]
fn exclusive_devices() -> Vec<Device> {
    alsa::devices()
}

#[cfg(target_os = "macos")]
fn exclusive_devices() -> Vec<Device> {
    coreaudio::devices()
}

#[cfg(target_os = "windows")]
fn exclusive_devices() -> Vec<Device> {
    wasapi::devices()
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn exclusive_devices() -> Vec<Device> {
    Vec::new()
}

pub fn exclusive_supported() -> bool {
    cfg!(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows"
    ))
}

/// Never through Display: cpal 0.18's Display turns a failed query into a
/// `to_string` panic, and on macOS a device mid-transition fails it.
fn device_name(device: &cpal::Device) -> Option<String> {
    device
        .description()
        .ok()
        .map(|desc| desc.name().to_string())
}

/// cpal has no stable id, so the name is the id; identical cards collapse.
fn shared_devices() -> Vec<Device> {
    let host = cpal::default_host();
    let Ok(devices) = host.output_devices() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for device in devices {
        let Some(name) = device_name(&device) else {
            continue;
        };
        if out.iter().any(|d: &Device| d.id == name) {
            continue;
        }
        out.push(Device {
            id: name.clone(),
            name,
        });
    }
    out
}

/// The mixer owns the rate, so `request.rate` is ignored; the decode thread resamples.
fn open_shared(request: &Request, shared: &Arc<Shared>) -> Result<OpenOutput, String> {
    let host = cpal::default_host();
    // A saved device that's gone falls back to the default: unplugged
    // headphones shouldn't mean no audio.
    let picked = request.device.as_deref().and_then(|want| {
        host.output_devices()
            .ok()?
            .find(|d| device_name(d).as_deref() == Some(want))
    });
    let device = picked
        .or_else(|| host.default_output_device())
        .ok_or("no default output device")?;
    let name = device_name(&device).unwrap_or_else(|| "unnamed device".into());
    let supported = device
        .default_output_config()
        .map_err(|e| format!("no default output config: {e}"))?;
    let sample_format = supported.sample_format();
    let config: StreamConfig = supported.into();
    let sample_rate = config.sample_rate;

    let (ring_frames, producer, ring, tap_tx, tap) = rings(sample_rate);

    let stream = match sample_format {
        SampleFormat::F32 => build::<f32>(&device, &config, shared.clone(), ring, tap_tx),
        SampleFormat::I16 => build::<i16>(&device, &config, shared.clone(), ring, tap_tx),
        SampleFormat::U16 => build::<u16>(&device, &config, shared.clone(), ring, tap_tx),
        SampleFormat::I32 => build::<i32>(&device, &config, shared.clone(), ring, tap_tx),
        other => return Err(format!("unsupported device sample format {other}")),
    }?;
    stream.play().map_err(|e| format!("stream play: {e}"))?;

    Ok(OpenOutput {
        stream: Box::new(stream),
        negotiated: Negotiated {
            mode: Mode::Shared,
            device: name,
            sample_rate,
            channels: config.channels,
            format: sample_format.to_string(),
            fallback: None,
        },
        sample_rate,
        ring_frames,
        producer,
        tap,
    })
}

fn build<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    shared: Arc<Shared>,
    mut ring: Consumer<f32>,
    mut tap: Producer<f32>,
) -> Result<Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let device_channels = config.channels as usize;

    let err_shared = shared.clone();

    let callback = move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
        fill(data, device_channels, &shared, &mut ring, &mut tap);
    };

    let err_fn = move |err: cpal::Error| {
        // The stream is dead. Flag it so the app reopens; otherwise the ring fills,
        // the engine parks, and the UI freezes on "playing". Logging is fine: this
        // is the error thread, not the data path.
        log::error!("stream error: {err}");
        err_shared.device_lost.store(true, Ordering::Release);
    };

    device
        .build_output_stream(*config, callback, err_fn, None)
        .map_err(|e| format!("build_output_stream: {e}"))
}

/// One device buffer's worth of work, and all a backend may do to samples.
/// Every backend calls this, so "bit-perfect" means one thing (ADR 19).
///
/// Runs on the real-time thread: no allocation, no lock, no logging, no I/O
/// (ADR 2). A trailing partial frame is filled with silence, never a panic.
pub(crate) fn fill<T>(
    data: &mut [T],
    device_channels: usize,
    shared: &Shared,
    ring: &mut Consumer<f32>,
    tap: &mut Producer<f32>,
) where
    T: SizedSample + FromSample<f32>,
{
    // A seek or skip is in flight: discard what was queued before it, play
    // silence, and don't advance the clock. Exactly once per epoch; the ack is
    // this side's own record, and tells the decode thread the ring is clear.
    let seq = shared.flush_seq.load(Ordering::Acquire);
    if seq != shared.flush_ack.load(Ordering::Relaxed) {
        while ring.pop().is_ok() {}
        shared.flush_ack.store(seq, Ordering::Release);
        data.fill(T::from_sample(0.0f32));
        return;
    }

    // An audition blip plays this many frames through a pause without touching
    // `playing`, so nothing watching the pause reacts. Only frames actually
    // played count, so an underrun during the seek's refill doesn't spend it.
    let mut blip = shared.audition_left.load(Ordering::Relaxed);
    let auditioning = blip > 0;

    if !shared.playing.load(Ordering::Relaxed) && !auditioning {
        data.fill(T::from_sample(0.0f32));
        return;
    }

    let volume = f32::from_bits(shared.volume_bits.load(Ordering::Relaxed));
    let mut frames_out: u64 = 0;

    let mut frames = data.chunks_exact_mut(device_channels);
    for frame in frames.by_ref() {
        if auditioning && blip == 0 {
            frame.fill(T::from_sample(0.0f32));
            continue;
        }
        // Pop whole frames only so interleaving can't slip. Dry ring is an
        // underrun: silence, not counted.
        if ring.slots() < 2 {
            frame.fill(T::from_sample(0.0f32));
            continue;
        }
        let (l, r) = (ring.pop().unwrap(), ring.pop().unwrap());

        // Lossy tap: drop, never wait. L and R go in together or not at all, or a
        // single free slot would misalign the tap for good. Pre-volume, so the
        // visualizers read the program, chain DSP included.
        if tap.slots() >= 2 {
            let _ = tap.push(l);
            let _ = tap.push(r);
        }

        // Unity short-circuits (ADR 19's bypass rule): chain off, volume 100%, and
        // equal rates deliver the decoder's output bit-identically.
        let (l, r) = if volume == 1.0 {
            (l, r)
        } else {
            (l * volume, r * volume)
        };

        match device_channels {
            1 => frame[0] = T::from_sample((l + r) * 0.5),
            _ => {
                frame[0] = T::from_sample(l);
                frame[1] = T::from_sample(r);
                for s in frame.iter_mut().skip(2) {
                    *s = T::from_sample(0.0f32);
                }
            }
        }
        frames_out += 1;
        blip = blip.saturating_sub(1);
    }
    frames.into_remainder().fill(T::from_sample(0.0f32));

    if frames_out > 0 {
        shared
            .frames_consumed
            .fetch_add(frames_out, Ordering::Relaxed);
    }
    if auditioning {
        shared.audition_left.store(blip, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    fn primed(frames: usize) -> (Arc<Shared>, Consumer<f32>, Producer<f32>, Consumer<f32>) {
        let shared = Arc::new(Shared::new(1));
        let (mut producer, ring) = RingBuffer::<f32>::new(frames * 2);
        let (tap_tx, tap) = RingBuffer::<f32>::new(frames * 2);
        for i in 0..frames {
            producer.push(i as f32 + 1.0).unwrap();
            producer.push(-(i as f32) - 1.0).unwrap();
        }
        (shared, ring, tap_tx, tap)
    }

    fn drain(tap: &mut Consumer<f32>) -> Vec<f32> {
        let mut out = Vec::new();
        while let Ok(s) = tap.pop() {
            out.push(s);
        }
        out
    }

    #[test]
    fn unity_volume_passes_samples_through() {
        let (shared, mut ring, mut tap_tx, _tap) = primed(2);
        let mut data = [0.0f32; 4];
        fill(&mut data, 2, &shared, &mut ring, &mut tap_tx);
        assert_eq!(data, [1.0, -1.0, 2.0, -2.0]);
        assert_eq!(shared.frames_consumed.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn volume_scales_below_unity() {
        let (shared, mut ring, mut tap_tx, _tap) = primed(1);
        shared
            .volume_bits
            .store(0.5f32.to_bits(), Ordering::Relaxed);
        let mut data = [0.0f32; 2];
        fill(&mut data, 2, &shared, &mut ring, &mut tap_tx);
        assert_eq!(data, [0.5, -0.5]);
    }

    #[test]
    fn paused_emits_silence_and_keeps_the_ring() {
        let (shared, mut ring, mut tap_tx, _tap) = primed(1);
        shared.playing.store(false, Ordering::Relaxed);
        let mut data = [9.0f32; 2];
        fill(&mut data, 2, &shared, &mut ring, &mut tap_tx);
        assert_eq!(data, [0.0, 0.0]);
        assert_eq!(ring.slots(), 2);
        assert_eq!(shared.frames_consumed.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn an_audition_blip_plays_its_length_through_the_pause() {
        let (shared, mut ring, mut tap_tx, _tap) = primed(4);
        shared.playing.store(false, Ordering::Relaxed);
        shared.audition_left.store(2, Ordering::Relaxed);
        let mut data = [9.0f32; 8];
        fill(&mut data, 2, &shared, &mut ring, &mut tap_tx);
        assert_eq!(data, [1.0, -1.0, 2.0, -2.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(shared.frames_consumed.load(Ordering::Relaxed), 2);
        assert_eq!(shared.audition_left.load(Ordering::Relaxed), 0);
        assert!(!shared.playing.load(Ordering::Relaxed), "still paused");
        assert_eq!(ring.slots(), 4);
        let mut data = [9.0f32; 2];
        fill(&mut data, 2, &shared, &mut ring, &mut tap_tx);
        assert_eq!(data, [0.0, 0.0]);
        assert_eq!(ring.slots(), 4);
    }

    #[test]
    fn a_long_audition_carries_across_callbacks() {
        let (shared, mut ring, mut tap_tx, _tap) = primed(4);
        shared.playing.store(false, Ordering::Relaxed);
        shared.audition_left.store(3, Ordering::Relaxed);
        let mut data = [9.0f32; 4];
        fill(&mut data, 2, &shared, &mut ring, &mut tap_tx);
        assert_eq!(shared.audition_left.load(Ordering::Relaxed), 1);
        assert_eq!(shared.frames_consumed.load(Ordering::Relaxed), 2);
        fill(&mut data, 2, &shared, &mut ring, &mut tap_tx);
        assert_eq!(data, [3.0, -3.0, 0.0, 0.0]);
        assert_eq!(shared.audition_left.load(Ordering::Relaxed), 0);
    }

    /// A dry ring is the seek's refill in flight; it mustn't spend the blip.
    #[test]
    fn an_underrun_doesnt_spend_the_blip() {
        let (shared, mut ring, mut tap_tx, _tap) = primed(0);
        shared.playing.store(false, Ordering::Relaxed);
        shared.audition_left.store(2, Ordering::Relaxed);
        let mut data = [9.0f32; 4];
        fill(&mut data, 2, &shared, &mut ring, &mut tap_tx);
        assert_eq!(data, [0.0, 0.0, 0.0, 0.0]);
        assert_eq!(shared.audition_left.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn flush_discards_the_whole_ring_and_acks() {
        let (shared, mut ring, mut tap_tx, _tap) = primed(8);
        shared.flush_seq.store(1, Ordering::Release);
        let mut data = [9.0f32; 2];
        fill(&mut data, 2, &shared, &mut ring, &mut tap_tx);
        assert_eq!(data, [0.0, 0.0]);
        assert_eq!(ring.slots(), 0);
        assert_eq!(shared.frames_consumed.load(Ordering::Relaxed), 0);
        assert_eq!(shared.flush_ack.load(Ordering::Acquire), 1);
    }

    #[test]
    fn an_acked_flush_never_discards_twice() {
        let (shared, mut ring, mut tap_tx, _tap) = primed(8);
        shared.flush_seq.store(1, Ordering::Release);
        let mut data = [9.0f32; 2];
        fill(&mut data, 2, &shared, &mut ring, &mut tap_tx);
        // Samples after the discard belong to the new track and must play.
        let (mut producer, mut ring) = RingBuffer::<f32>::new(4);
        producer.push(0.25).unwrap();
        producer.push(-0.25).unwrap();
        fill(&mut data, 2, &shared, &mut ring, &mut tap_tx);
        assert_eq!(data, [0.25, -0.25]);
        assert_eq!(shared.frames_consumed.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn dry_ring_emits_silence_and_counts_nothing() {
        let (shared, mut ring, mut tap_tx, _tap) = primed(1);
        let mut data = [9.0f32; 6];
        fill(&mut data, 2, &shared, &mut ring, &mut tap_tx);
        assert_eq!(data, [1.0, -1.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(shared.frames_consumed.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn mono_device_folds_and_wider_devices_get_silence() {
        let (shared, mut ring, mut tap_tx, _tap) = primed(1);
        let mut mono = [9.0f32; 1];
        fill(&mut mono, 1, &shared, &mut ring, &mut tap_tx);
        assert_eq!(mono, [0.0]);

        let (shared, mut ring, mut tap_tx, _tap) = primed(1);
        let mut wide = [9.0f32; 4];
        fill(&mut wide, 4, &shared, &mut ring, &mut tap_tx);
        assert_eq!(wide, [1.0, -1.0, 0.0, 0.0]);
    }

    #[test]
    fn tap_takes_a_pre_volume_copy_of_every_frame() {
        let (shared, mut ring, mut tap_tx, mut tap) = primed(2);
        shared
            .volume_bits
            .store(0.5f32.to_bits(), Ordering::Relaxed);
        let mut data = [0.0f32; 4];
        fill(&mut data, 2, &shared, &mut ring, &mut tap_tx);
        assert_eq!(data, [0.5, -0.5, 1.0, -1.0]);
        assert_eq!(drain(&mut tap), vec![1.0, -1.0, 2.0, -2.0]);
    }

    #[test]
    fn a_full_tap_drops_frames_without_slipping_interleave() {
        let (shared, mut ring, _wide, _unused) = primed(4);
        // One frame of room: the first pair goes in whole, never a lone L.
        let (mut tap_tx, mut tap) = RingBuffer::<f32>::new(3);
        let mut data = [0.0f32; 8];
        fill(&mut data, 2, &shared, &mut ring, &mut tap_tx);
        assert_eq!(drain(&mut tap), vec![1.0, -1.0]);
    }

    /// Without an exclusive backend the error must say so, not blame the device.
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    #[test]
    fn exclusive_where_it_is_not_built_says_so() {
        let shared = Arc::new(Shared::new(1));
        let request = Request {
            mode: Mode::Exclusive,
            ..Request::default()
        };
        assert!(!exclusive_supported());
        assert!(devices(Mode::Exclusive).is_empty());
        assert_eq!(
            open_exclusive(&request, &shared).map(|_| ()).unwrap_err(),
            NO_EXCLUSIVE
        );
    }

    /// Both hardware tests claim the same device, so they take turns. Poison
    /// is ignored.
    static HARDWARE: Mutex<()> = Mutex::new(());

    /// Claims real hardware, so opt-in: `cargo test -p rox-playback -- --ignored`.
    /// Covers claim, negotiate, writer thread, and ring drain.
    #[test]
    #[ignore = "claims a real audio device"]
    fn exclusive_claims_a_device_and_runs_its_clock() {
        let _hardware = HARDWARE.lock().unwrap_or_else(|e| e.into_inner());
        let list = devices(Mode::Exclusive);
        assert!(!list.is_empty(), "no exclusive device on this machine");
        let shared = Arc::new(Shared::new(1));
        let request = Request {
            mode: Mode::Exclusive,
            device: Some(list[0].id.clone()),
            rate: Some(44100),
            format: Some("f32".into()),
            period_ms: Some(5.0),
        };
        let mut out = open(&request, &shared).expect("claiming the device");
        println!("negotiated {:?}", out.negotiated);
        assert_eq!(out.negotiated.mode, Mode::Exclusive);
        assert!(out.negotiated.fallback.is_none());
        assert_eq!(out.sample_rate, out.negotiated.sample_rate);

        while out.producer.push(0.0).is_ok() {}
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!shared.device_lost(), "the writer thread faulted");
        assert!(
            shared.frames_consumed.load(Ordering::Relaxed) > 0,
            "the device never took a frame"
        );
        drop(out.stream);
    }

    /// A busy device must fall back to shared with a reason, not fail silent.
    #[test]
    #[ignore = "claims a real audio device"]
    fn a_device_that_is_busy_falls_back_to_shared() {
        let _hardware = HARDWARE.lock().unwrap_or_else(|e| e.into_inner());
        let list = devices(Mode::Exclusive);
        assert!(!list.is_empty(), "no exclusive device on this machine");
        let request = Request {
            mode: Mode::Exclusive,
            device: Some(list[0].id.clone()),
            rate: None,
            format: None,
            period_ms: None,
        };
        let first = Arc::new(Shared::new(1));
        let held = open(&request, &first).expect("claiming the device");
        assert_eq!(held.negotiated.mode, Mode::Exclusive);

        let second = Arc::new(Shared::new(1));
        let out = open(&request, &second).expect("falling back rather than failing");
        println!("fell back: {:?}", out.negotiated);
        assert_eq!(out.negotiated.mode, Mode::Shared);
        assert!(out.negotiated.fallback.is_some());
    }
}
