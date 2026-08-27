//! The output seam and the shared-mode backend that implements it. ADR 9
//! kept the output layer swappable and deferred the exclusive path; ADR 19
//! spends that option, so this file is two things now: the contract a
//! backend implements, and cpal's implementation of it.
//!
//! A backend gets the shared atomics, hands back the producer end of the
//! sample ring and the consumer end of the PCM tap, and reports what the
//! device actually accepted. The engine never learns which one runs.
//!
//! The hard line from the components spec is in [`fill`], which every
//! backend calls: pop a pre-allocated ring, read atomics, write the device
//! buffer. No allocation, no lock, no logging, no I/O.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream, StreamConfig};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::shared::Shared;

#[cfg(target_os = "linux")]
mod alsa;
#[cfg(target_os = "macos")]
mod coreaudio;
// Not cfg-gated like its neighbours: the FFI half is Windows-only, but the
// format ladder and the period math above it are plain Rust and have their
// own tests, so the module compiles everywhere and those tests run in every
// build rather than only on the one platform nobody here develops on.
mod wasapi;

/// Half a second of buffered stereo between decode and the callback.
const RING_SECS: f64 = 0.5;
/// How long a shared open keeps retrying before its error stands. Right
/// after exclusive lets a device go, CoreAudio is still relocking the clock
/// and refuses queries (OSStatus 56 on the default config), so the first
/// attempt of an exclusive-to-shared switch falls inside that window and a
/// single try would kill the session over a transient. Twenty tries of 50 ms
/// is a second, blocking the caller like the exclusive rate settle already
/// does; a machine with genuinely no device pays it once and then the error
/// stands.
const OPEN_TRIES: u32 = 20;
const OPEN_STEP: Duration = Duration::from_millis(50);
/// Tap capacity in samples. Kept small so the tap consumer lags and loses
/// rather than backpressures.
const TAP_SAMPLES: usize = 16384;

/// What a platform without an exclusive backend reports. Linux, macOS, and
/// Windows all have one; anywhere else a stub that claimed to be one would be
/// worse than this string.
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
const NO_EXCLUSIVE: &str = "exclusive output isn't built for this platform yet";

/// How the samples get to the device.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Through the system's mixer, cpal on every platform. Other apps keep
    /// making sound and the mixer owns the rate, so a file whose rate
    /// differs gets resampled on the way out.
    #[default]
    Shared,
    /// The device claimed for rox alone, at the file's own rate where it
    /// takes one. Per-platform below cpal.
    Exclusive,
}

/// A device the picker can offer. The id is the one a [`Request`] names and
/// it's backend-scoped: a cpal device name in shared mode, an ALSA `hw:`
/// name in exclusive. They're never interchangeable, which is why the two
/// lists are asked for separately.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Device {
    pub id: String,
    pub name: String,
}

/// What the caller wants. Nothing here is a promise; [`Negotiated`] is what
/// came back.
// No `Eq`: the period is a float, and nothing compares requests for
// identity anyway.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct Request {
    pub mode: Mode,
    /// The device id to claim, or None for whatever the system calls
    /// default. An id that's gone (unplugged, renamed) also falls to the
    /// default rather than failing the open.
    pub device: Option<String>,
    /// The rate to ask for, the playing file's own where the caller knows
    /// it. Only exclusive follows it; shared runs at whatever rate the
    /// mixer already picked and ignores this.
    pub rate: Option<u32>,
    /// The sample format to ask for by its short name (`f32`, `s32`,
    /// `s16`), or None to take the widest the device offers. A name the
    /// device won't take falls to that same best-first search rather than
    /// failing the open, and [`Negotiated::format`] reports what was used.
    /// Exclusive only: the mixer owns the format in shared mode.
    pub format: Option<String>,
    /// How much audio the device holds per period, in milliseconds, or None
    /// for the backend's default. The knob behind the latency trade: short
    /// periods wake the writer more often and xrun sooner on a loaded
    /// machine. Exclusive only.
    pub period_ms: Option<f64>,
}

/// What the device actually accepted, for the UI to state instead of
/// echoing the request back. ADR 19 is blunt about this: a bit-perfect
/// claim nobody checked is decoration.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Negotiated {
    pub mode: Mode,
    /// The device that's running, by the name a human reads.
    pub device: String,
    pub sample_rate: u32,
    pub channels: u16,
    /// The sample format the device takes, in cpal's spelling for shared
    /// and ALSA's for exclusive.
    pub format: String,
    /// Why exclusive isn't running when exclusive was asked for. Some means
    /// this is shared output standing in, and the string is the reason the
    /// claim failed. None means the mode above is the mode requested.
    pub fallback: Option<String>,
}

/// A live output stream. Nothing to call on one: a backend hands it back so
/// the caller can hold it, and dropping it stops audio and gives the device
/// up. Boxed rather than an enum so a new backend is one new file instead of
/// another arm in every match.
pub trait OutputStream {}

impl OutputStream for Stream {}

pub struct OpenOutput {
    /// Held so the stream stays alive; dropping it stops audio.
    pub stream: Box<dyn OutputStream>,
    /// What the device accepted, the truth the UI shows.
    pub negotiated: Negotiated,
    pub sample_rate: u32,
    pub ring_frames: usize,
    /// Decode thread's side of the sample ring (interleaved stereo f32).
    pub producer: Producer<f32>,
    /// Visualizer side of the PCM tap.
    pub tap: Consumer<f32>,
}

/// The devices one mode can open. Two lists rather than one tagged list
/// because the ids don't cross: picking a cpal device name only means
/// something to the shared backend.
pub fn devices(mode: Mode) -> Vec<Device> {
    match mode {
        Mode::Shared => shared_devices(),
        Mode::Exclusive => exclusive_devices(),
    }
}

/// Open output per the request, allocate both rings, start the stream.
///
/// Exclusive that can't claim its device (busy, no such rate, no such
/// device) comes back as shared with the reason recorded, never as silence,
/// which is the failure shape ADR 19 asks for.
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

/// [`open_shared`] with the settling retry, so a device that refuses the
/// first try because it's mid-transition doesn't cost the session.
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

/// Allocate the sample ring and the PCM tap for a stream at `rate`. Both
/// backends call this before they touch the device, so the ring depth is
/// one rule (500 ms) rather than one per backend.
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

/// Whether this build has an exclusive backend at all, so the settings page
/// can say "not on this platform" instead of offering a toggle that always
/// falls back.
pub fn exclusive_supported() -> bool {
    cfg!(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows"
    ))
}

/// The device's name, or None where the query fails. cpal 0.18 dropped
/// `name()` for `description()`, and its Display wraps the same query with
/// the Err swallowed into `fmt::Error`, which `to_string` turns into a
/// panic. The query does fail in practice: on macOS a device mid-transition
/// (hog mode just released, clock relocking) won't return one, and the
/// exclusive toggle hits exactly that. So the name is asked through the fallible
/// path everywhere, and never through Display.
fn device_name(device: &cpal::Device) -> Option<String> {
    device
        .description()
        .ok()
        .map(|desc| desc.name().to_string())
}

/// The cpal devices, by name. cpal has no stable device id, so the name is
/// the id; two identical cards read as one entry and the first one wins,
/// which is the same ambiguity every cpal app has.
fn shared_devices() -> Vec<Device> {
    let host = cpal::default_host();
    let Ok(devices) = host.output_devices() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for device in devices {
        // A device whose name won't read is one the picker can't offer
        // anyway; skipping it beats aborting on the name read.
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

/// Shared mode: cpal's default host, the picked device or the default one,
/// and the config that device already runs at. The mixer owns the rate here,
/// so `request.rate` is not consulted; the resampler on the decode thread
/// covers the difference.
fn open_shared(request: &Request, shared: &Arc<Shared>) -> Result<OpenOutput, String> {
    let host = cpal::default_host();
    // A saved device that's gone falls back to the default rather than
    // failing: unplugging headphones shouldn't mean no audio until someone
    // visits the settings window.
    let picked = request.device.as_deref().and_then(|want| {
        host.output_devices()
            .ok()?
            .find(|d| device_name(d).as_deref() == Some(want))
    });
    let device = picked
        .or_else(|| host.default_output_device())
        .ok_or("no default output device")?;
    // A default device mid-transition may not return its name; the stream
    // still opens, so play through it rather than failing over a label.
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

    // The error callback runs on the backend's own thread when the stream
    // faults; it flags the loss so the app can reopen. Kept off the data
    // callback's clone so that one stays a straight move.
    let err_shared = shared.clone();

    let callback = move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
        fill(data, device_channels, &shared, &mut ring, &mut tap);
    };

    let err_fn = move |err: cpal::Error| {
        // The device dropped out or the backend faulted. The data callback
        // won't run again on this stream, so flag the loss and let the app
        // reopen; without this the ring fills, the engine parks, and the UI
        // stays frozen on "playing". Logging is fine here, this is the backend
        // error thread, not the RT data path.
        log::error!("stream error: {err}");
        err_shared.device_lost.store(true, Ordering::Release);
    };

    device
        .build_output_stream(*config, callback, err_fn, None)
        .map_err(|e| format!("build_output_stream: {e}"))
}

/// One device buffer's worth of work, and the whole of what a backend is
/// allowed to do to the samples. Both backends call this and neither may do
/// any of it differently: two backends drifting on the unity short-circuit
/// would make "bit-perfect" mean two things, and ADR 19 defines it once.
///
/// Runs on the real-time thread, so it obeys ADR 2 to the letter: no
/// allocation, no lock, no logging, no I/O.
///
/// `data` is stepped through a device frame at a time. cpal hands whole
/// frames, and so do the exclusive backends, but a buffer that ends
/// mid-frame anyway leaves its stub silent rather than panicking on the RT
/// thread.
pub(crate) fn fill<T>(
    data: &mut [T],
    device_channels: usize,
    shared: &Shared,
    ring: &mut Consumer<f32>,
    tap: &mut Producer<f32>,
) where
    T: SizedSample + FromSample<f32>,
{
    // A seek or a skip is in flight: throw away whatever the decode thread
    // queued before it, play silence, and advance nothing on the clock.
    //
    // Exactly once per epoch. The ack is this side's own bookkeeping (no
    // one else writes it), so it doubles as the record of which epoch was
    // handled, and no backend has to keep state across calls. What the
    // decode thread gets out of it is the end of the grace sleep: it knows
    // the ring is clear the moment this runs, instead of waiting long
    // enough that it must have been.
    let seq = shared.flush_seq.load(Ordering::Acquire);
    if seq != shared.flush_ack.load(Ordering::Relaxed) {
        while ring.pop().is_ok() {}
        shared.flush_ack.store(seq, Ordering::Release);
        data.fill(T::from_sample(0.0f32));
        return;
    }

    if !shared.playing.load(Ordering::Relaxed) {
        data.fill(T::from_sample(0.0f32));
        return;
    }

    let volume = f32::from_bits(shared.volume_bits.load(Ordering::Relaxed));
    let mut frames_out: u64 = 0;

    let mut frames = data.chunks_exact_mut(device_channels);
    for frame in frames.by_ref() {
        // The ring holds whole stereo frames; only pop when both
        // samples are there so interleaving can't slip. Dry ring means
        // underrun (or end of queue): emit silence, don't count it.
        if ring.slots() < 2 {
            frame.fill(T::from_sample(0.0f32));
            continue;
        }
        let (l, r) = (ring.pop().unwrap(), ring.pop().unwrap());

        // Lossy PCM tap: if the visualizer side is behind, drop, never
        // wait. Push L and R together or not at all, so a single free
        // slot can't drop R while L goes in and leave the tap stream
        // frame-misaligned for good. Tapped pre-volume, so the spectrum
        // and signals read the program material, not the listening level;
        // chain DSP (EQ, ReplayGain) still shows because it runs before
        // the ring.
        if tap.slots() >= 2 {
            let _ = tap.push(l);
            let _ = tap.push(r);
        }

        // Unity short-circuits (ADR 19's bypass rule): at volume 1.0 the
        // samples pass through untouched, so chain off + volume at 100% +
        // equal rates delivers the decoder's output bit-identically.
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
    }
    // A tail too short to be a frame gets silence, not a popped sample it
    // has no partner for.
    frames.into_remainder().fill(T::from_sample(0.0f32));

    if frames_out > 0 {
        shared
            .frames_consumed
            .fetch_add(frames_out, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// A ring primed with `frames` stereo frames counting up from 1.0, and a
    /// tap wide enough to take all of them.
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
    fn flush_discards_the_whole_ring_and_acks() {
        let (shared, mut ring, mut tap_tx, _tap) = primed(8);
        shared.flush_seq.store(1, Ordering::Release);
        let mut data = [9.0f32; 2];
        fill(&mut data, 2, &shared, &mut ring, &mut tap_tx);
        assert_eq!(data, [0.0, 0.0]);
        assert_eq!(ring.slots(), 0);
        assert_eq!(shared.frames_consumed.load(Ordering::Relaxed), 0);
        // The decode thread waits on the ack before it resyncs.
        assert_eq!(shared.flush_ack.load(Ordering::Acquire), 1);
    }

    #[test]
    fn an_acked_flush_never_discards_twice() {
        let (shared, mut ring, mut tap_tx, _tap) = primed(8);
        shared.flush_seq.store(1, Ordering::Release);
        let mut data = [9.0f32; 2];
        fill(&mut data, 2, &shared, &mut ring, &mut tap_tx);
        // Samples pushed after the discard belong to the new track; the
        // epoch is handled, so the next callback plays them instead of
        // eating them the way a flag being cleared late used to.
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
        // The device gets half scale, the visualizers get the program.
        assert_eq!(data, [0.5, -0.5, 1.0, -1.0]);
        assert_eq!(drain(&mut tap), vec![1.0, -1.0, 2.0, -2.0]);
    }

    #[test]
    fn a_full_tap_drops_frames_without_slipping_interleave() {
        let (shared, mut ring, _wide, _unused) = primed(4);
        // One frame of room, four frames of audio: the tap takes the first
        // pair whole and drops the rest rather than pushing a lone L.
        let (mut tap_tx, mut tap) = RingBuffer::<f32>::new(3);
        let mut data = [0.0f32; 8];
        fill(&mut data, 2, &shared, &mut ring, &mut tap_tx);
        assert_eq!(drain(&mut tap), vec![1.0, -1.0]);
    }

    /// Nothing here opens a device; only the platform seam is under test.
    /// Where no exclusive backend is built the error has to name that, not
    /// look like the device was busy, or the settings page would blame the
    /// hardware for a gap in rox.
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

    /// Both hardware tests below claim the same first exclusive device, and
    /// the test harness runs them on parallel threads, so without this one
    /// steals the card out from under the other and whichever loses the race
    /// reports a fallback it never asked for. Poison is ignored: a failed
    /// assertion in one shouldn't turn the other into a second, unrelated
    /// failure.
    static HARDWARE: Mutex<()> = Mutex::new(());

    /// The one test that touches hardware, so it only runs when asked:
    /// `cargo test -p rox-playback -- --ignored`. It claims the first
    /// exclusive device, feeds it silence, and checks the output clock
    /// moved, which is the whole path (claim, negotiate, writer thread,
    /// ring drain) short of anything audible. There's nothing to hear
    /// either way; failing it means the claim or the thread is broken.
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
            // Pin the widest format and a short period, so this covers the
            // two request fields the settings page can pin as well as the
            // defaults would.
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

    /// The other half of the hardware path, same opt-in: a device that's
    /// already claimed has to come back as shared output with the
    /// reason, because the alternative shape (an error, no stream) is
    /// silence with a toggle to blame for it.
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
