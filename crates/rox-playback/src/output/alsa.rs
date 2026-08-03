//! Exclusive output on Linux: the card claimed directly as `hw:CARD=x,DEV=n`,
//! which is the one ALSA name with no dmix, no plug, and no sound server in
//! the path. ADR 19 left the PipeWire pro-audio route open as an
//! alternative; `hw:` wins because it's the only one that refuses to
//! silently resample, and a mode whose whole point is "the file's rate
//! reaches the converter" can't be built on a device node that would paper
//! over a mismatch.
//!
//! The shape mirrors the cpal backend deliberately. There, cpal owns a
//! real-time thread and calls us per buffer; here we own the thread and
//! block on `writei` per period. Both hand the same [`fill`] the same
//! buffer, so the bypass rule holds identically in either mode, which is
//! the part of the contract ADR 19 says both backends must keep.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use alsa::card;
use alsa::ctl::{Ctl, DeviceIter};
use alsa::pcm::{Access, Format, Frames, HwParams, IoFormat, PCM};
use alsa::{Direction, ValueOr};
use cpal::{FromSample, SizedSample};
use rtrb::{Consumer, Producer};

use super::{fill, rings, Device, Mode, Negotiated, OpenOutput, OutputStream, Request};
use crate::shared::Shared;

/// One period, in seconds. Ten milliseconds is short enough that a pause or
/// a device drop is noticed promptly and long enough that the writer thread
/// isn't waking a hundred times per buffer.
const PERIOD_SECS: f64 = 0.01;
/// Periods in the device buffer. Four is the usual floor for a card that
/// won't xrun on a scheduler hiccup, and the 500 ms sample ring in front of
/// it absorbs everything longer.
const PERIODS: Frames = 4;
/// The longest period a settings file can ask for. The buffer is [`PERIODS`]
/// of them and the ring in front holds 500 ms, so a hand-edited 1000 asks for
/// a device buffer the decode thread can't keep stocked. Same ceiling the
/// WASAPI backend puts on its own period.
const MAX_PERIOD_SECS: f64 = 0.1;
/// The rate to ask for when the caller doesn't name one, which is every
/// session that opens before a file has been decoded. The pump reopens at
/// the file's own rate once it knows it.
const DEFAULT_RATE: u32 = 48000;

/// Formats we'll take, best first: float straight through, then the integer
/// widths every card has. Packed 24-bit (`S24_3LE`) is deliberately absent.
/// rox has no three-byte sample type, and the cards that offer it offer
/// `S32_LE` too, which carries the same 24 bits in a type Rust already has.
const FORMATS: &[(Sample, Format, &str)] = &[
    (Sample::F32, Format::float(), "f32"),
    (Sample::I32, Format::s32(), "s32"),
    (Sample::I16, Format::s16(), "s16"),
];

/// Which Rust sample type a negotiated ALSA format writes as. The writer
/// thread is generic over it, so this picks the instantiation.
#[derive(Clone, Copy)]
enum Sample {
    F32,
    I32,
    I16,
}

/// The claim on the device: the writer thread plus its stop flag. Dropping
/// it stops audio and hands the card back, which is what makes toggling
/// exclusive off actually release it.
struct Claim {
    stop: Arc<AtomicBool>,
    writer: Option<JoinHandle<()>>,
}

impl OutputStream for Claim {}

impl Drop for Claim {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // The thread checks the flag once per period and blocks in `writei`
        // in between, so this waits a period at worst. Joining rather than
        // detaching matters: the PCM handle lives in the thread, and a
        // detached thread would still hold the card while the next session
        // tried to claim it.
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

/// Every playback device the cards expose, as `aplay -l` lists them. Built
/// off the card and pcm info rather than ALSA's name hints: hints are full
/// of `default`, `sysdefault`, and plugin aliases, none of which is a claim
/// on hardware.
pub fn devices() -> Vec<Device> {
    let mut out = Vec::new();
    for card in card::Iter::new().flatten() {
        let Ok(ctl) = Ctl::from_card(&card, false) else {
            continue;
        };
        let Ok(info) = ctl.card_info() else { continue };
        let (Ok(id), Ok(card_name)) = (info.get_id(), info.get_name()) else {
            continue;
        };
        let (id, card_name) = (id.to_string(), card_name.to_string());
        for device in DeviceIter::new(&ctl) {
            let Ok(pcm) = ctl.pcm_info(device as u32, 0, Direction::Playback) else {
                continue;
            };
            let name = pcm.get_name().unwrap_or("").to_string();
            out.push(Device {
                id: format!("hw:CARD={id},DEV={device}"),
                name: if name.is_empty() {
                    card_name.clone()
                } else {
                    format!("{card_name}: {name}")
                },
            });
        }
    }
    out
}

/// Claim a device and start the writer thread. Every error here is one the
/// seam turns into a fallback to shared output, so they say what failed
/// rather than just that something did.
pub fn open(request: &Request, shared: &Arc<Shared>) -> Result<OpenOutput, String> {
    let list = devices();
    // A named device that's gone (card unplugged, renamed by a kernel
    // update) takes the first card rather than failing the open, matching
    // what the shared backend does with a stale cpal name.
    let picked = request
        .device
        .as_deref()
        .and_then(|want| list.iter().find(|d| d.id == want))
        .or_else(|| list.first())
        .ok_or("no ALSA playback device")?;

    let pcm = PCM::new(&picked.id, Direction::Playback, false)
        .map_err(|e| format!("claiming {}: {e}", picked.id))?;

    let want_rate = request.rate.unwrap_or(DEFAULT_RATE);
    let period_secs = request
        .period_ms
        .map(|ms| (ms / 1000.0).min(MAX_PERIOD_SECS))
        .unwrap_or(PERIOD_SECS);
    let (sample, rate, channels, format) =
        negotiate(&pcm, want_rate, request.format.as_deref(), period_secs)?;

    let (ring_frames, producer, ring, tap_tx, tap) = rings(rate);
    let period = period_frames(&pcm)?;
    let stop = Arc::new(AtomicBool::new(false));
    let writer = spawn(
        sample,
        pcm,
        period,
        channels as usize,
        shared.clone(),
        ring,
        tap_tx,
        stop.clone(),
    )?;

    Ok(OpenOutput {
        stream: Box::new(Claim {
            stop,
            writer: Some(writer),
        }),
        negotiated: Negotiated {
            mode: Mode::Exclusive,
            device: picked.name.clone(),
            sample_rate: rate,
            channels,
            format: format.into(),
            fallback: None,
        },
        sample_rate: rate,
        ring_frames,
        producer,
        tap,
    })
}

/// Settle the hardware parameters and report what the card took. The rate
/// is asked for exactly through `set_rate_near`, which lands on the closest
/// the hardware has; whatever comes back is what gets reported, so a card
/// that won't do 96 kHz reads as the rate it does instead of a rate rox
/// wished for.
fn negotiate(
    pcm: &PCM,
    want_rate: u32,
    want_format: Option<&str>,
    period_secs: f64,
) -> Result<(Sample, u32, u16, &'static str), String> {
    let hwp = HwParams::any(pcm).map_err(|e| format!("hw params: {e}"))?;
    hwp.set_access(Access::RWInterleaved)
        .map_err(|e| format!("interleaved access: {e}"))?;
    // The one line that makes this mode mean anything: with resampling off,
    // a rate the card can't do fails here instead of getting quietly
    // converted in the library.
    hwp.set_rate_resample(false)
        .map_err(|e| format!("disabling alsa resampling: {e}"))?;

    // A named format the card takes wins; anything else, including a name
    // for a format this card doesn't have, falls to the widest it does.
    // Reporting the result is what keeps that honest, so a pick the hardware
    // refused reads as the format actually running.
    let (sample, format, name) = want_format
        .and_then(|want| FORMATS.iter().find(|(_, _, name)| *name == want))
        .filter(|(_, format, _)| hwp.test_format(*format).is_ok())
        .or_else(|| {
            FORMATS
                .iter()
                .find(|(_, format, _)| hwp.test_format(*format).is_ok())
        })
        .copied()
        .ok_or("device takes no format rox can write")?;
    hwp.set_format(format)
        .map_err(|e| format!("format {name}: {e}"))?;

    // Stereo where the card has it, nearest where it doesn't; fill folds
    // onto whatever comes back the same way it does for cpal.
    let channels = hwp
        .set_channels_near(2)
        .map_err(|e| format!("channels: {e}"))?;
    let rate = hwp
        .set_rate_near(want_rate, ValueOr::Nearest)
        .map_err(|e| format!("rate {want_rate}: {e}"))?;

    let period = hwp
        .set_period_size_near((rate as f64 * period_secs) as Frames, ValueOr::Nearest)
        .map_err(|e| format!("period size: {e}"))?;
    let buffer = hwp
        .set_buffer_size_near(period * PERIODS)
        .map_err(|e| format!("buffer size: {e}"))?;
    pcm.hw_params(&hwp)
        .map_err(|e| format!("applying hw params: {e}"))?;

    let swp = pcm
        .sw_params_current()
        .map_err(|e| format!("sw params: {e}"))?;
    // Start once the buffer is full rather than on the first period, so the
    // stream comes up with its whole cushion instead of starting one period
    // from an xrun.
    swp.set_start_threshold(buffer)
        .map_err(|e| format!("start threshold: {e}"))?;
    swp.set_avail_min(period)
        .map_err(|e| format!("avail min: {e}"))?;
    pcm.sw_params(&swp)
        .map_err(|e| format!("applying sw params: {e}"))?;

    Ok((sample, rate, channels as u16, name))
}

/// The period the card settled on, read back rather than remembered: it's
/// the writer thread's buffer length, and guessing it wrong means writing
/// in chunks the device never asked for.
fn period_frames(pcm: &PCM) -> Result<usize, String> {
    let hwp = pcm
        .hw_params_current()
        .map_err(|e| format!("reading period size: {e}"))?;
    let period = hwp
        .get_period_size()
        .map_err(|e| format!("reading period size: {e}"))?;
    Ok(period.max(1) as usize)
}

/// Start the writer thread on the sample type the card took.
#[allow(clippy::too_many_arguments)]
fn spawn(
    sample: Sample,
    pcm: PCM,
    period: usize,
    channels: usize,
    shared: Arc<Shared>,
    ring: Consumer<f32>,
    tap: Producer<f32>,
    stop: Arc<AtomicBool>,
) -> Result<JoinHandle<()>, String> {
    let body = move || match sample {
        Sample::F32 => run::<f32>(pcm, period, channels, shared, ring, tap, stop),
        Sample::I32 => run::<i32>(pcm, period, channels, shared, ring, tap, stop),
        Sample::I16 => run::<i16>(pcm, period, channels, shared, ring, tap, stop),
    };
    std::thread::Builder::new()
        .name("alsa-out".into())
        .spawn(body)
        .map_err(|e| format!("spawn alsa writer: {e}"))
}

/// The writer thread: cpal's real-time callback, inverted. The buffer is
/// allocated once here and refilled in place, so the loop itself allocates
/// nothing, takes no lock, and does no I/O beyond the write the whole thread
/// exists for.
#[allow(clippy::too_many_arguments)]
fn run<T>(
    pcm: PCM,
    period: usize,
    channels: usize,
    shared: Arc<Shared>,
    mut ring: Consumer<f32>,
    mut tap: Producer<f32>,
    stop: Arc<AtomicBool>,
) where
    T: IoFormat + SizedSample + FromSample<f32>,
{
    let mut buf = vec![T::from_sample(0.0f32); period * channels];
    let io = match pcm.io_checked::<T>() {
        Ok(io) => io,
        Err(e) => return lost(&shared, format!("alsa io: {e}")),
    };
    if let Err(e) = pcm.prepare() {
        return lost(&shared, format!("alsa prepare: {e}"));
    }

    while !stop.load(Ordering::Acquire) {
        fill(&mut buf, channels, &shared, &mut ring, &mut tap);
        let mut written = 0;
        let mut recoveries = 0;
        while written < period {
            match io.writei(&buf[written * channels..]) {
                // A blocking write that took nothing. Nothing documented
                // gets here, but dropping the rest of the period would put
                // the position clock ahead of the card by frames `fill`
                // already counted, so try again and only give up if the
                // device keeps saying zero.
                Ok(0) => {
                    recoveries += 1;
                    if recoveries > 4 {
                        break;
                    }
                }
                Ok(n) => written += n,
                Err(e) => {
                    // An xrun or a suspend, both of which ALSA can put back
                    // together. Retry the same frames rather than dropping
                    // them, so the position clock doesn't quietly run ahead
                    // of what the card played. A device that won't come
                    // back after a few tries is gone, and the app's reopen
                    // path takes it from there.
                    recoveries += 1;
                    if recoveries > 4 || pcm.try_recover(e, true).is_err() {
                        return lost(&shared, format!("alsa write: {e}"));
                    }
                }
            }
        }
    }
    // Falling out of here closes the PCM, which stops the stream and hands
    // the card back. Deliberately not drained first: every path to this
    // point is a teardown, and the caller is blocked in join waiting for
    // it, so playing out the last 40 ms would only be a hitch in the UI.
}

/// Flag the device as gone the same way cpal's error callback does, so the
/// player's existing reopen path picks it up. Logging is fine here: this is
/// the last thing the writer thread does before it stops.
fn lost(shared: &Shared, message: String) {
    log::error!("exclusive output: {message}");
    shared.device_lost.store(true, Ordering::Release);
}
