//! Exclusive output on Linux: the card claimed as `hw:CARD=x,DEV=n`, the one
//! ALSA name with no dmix, plug, or sound server in the path, and the only
//! route that refuses to silently resample (ADR 19).
//!
//! We own the writer thread and block on `writei` per period, handing the
//! same [`fill`] the same buffer as cpal does, so the bypass rule holds in
//! both modes.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use alsa::card;
use alsa::ctl::{Ctl, DeviceIter};
use alsa::pcm::{Access, Format, Frames, HwParams, IoFormat, PCM};
use alsa::{Direction, ValueOr};
use cpal::{FromSample, SizedSample};
use rtrb::{Consumer, Producer};

use super::{Device, Mode, Negotiated, OpenOutput, OutputStream, Request, fill, rings};
use crate::shared::Shared;

/// Short enough that a pause or a device drop is noticed promptly.
const PERIOD_SECS: f64 = 0.01;
/// The usual floor against xruns; the 500 ms ring absorbs anything longer.
const PERIODS: Frames = 4;
/// Caps a hand-edited period: [`PERIODS`] of them has to stay under what the
/// 500 ms ring can keep stocked. Same ceiling as WASAPI.
const MAX_PERIOD_SECS: f64 = 0.1;
/// Before a file is decoded; the pump reopens at the file's rate.
const DEFAULT_RATE: u32 = 48000;

/// Best first. No packed 24-bit: cards that offer `S24_3LE` offer `S32_LE` too.
const FORMATS: &[(Sample, Format, &str)] = &[
    (Sample::F32, Format::float(), "f32"),
    (Sample::I32, Format::s32(), "s32"),
    (Sample::I16, Format::s16(), "s16"),
];

#[derive(Clone, Copy)]
enum Sample {
    F32,
    I32,
    I16,
}

/// Dropping it stops audio and hands the card back.
struct Claim {
    stop: Arc<AtomicBool>,
    writer: Option<JoinHandle<()>>,
}

impl OutputStream for Claim {}

impl Drop for Claim {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Waits a period at worst. Join, don't detach: the thread owns the PCM, and
        // a detached one would still hold the card while the next session claims it.
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

/// As `aplay -l` lists them, from card and pcm info. ALSA's name hints are
/// aliases, not claims on hardware.
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

/// Errors here become a fallback to shared, so they say what failed.
pub fn open(request: &Request, shared: &Arc<Shared>) -> Result<OpenOutput, String> {
    let list = devices();
    // A named card that's gone takes the first card, like the shared backend.
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

/// The rate goes through `set_rate_near`, and whatever comes back is what's
/// reported.
fn negotiate(
    pcm: &PCM,
    want_rate: u32,
    want_format: Option<&str>,
    period_secs: f64,
) -> Result<(Sample, u32, u16, &'static str), String> {
    let hwp = HwParams::any(pcm).map_err(|e| format!("hw params: {e}"))?;
    hwp.set_access(Access::RWInterleaved)
        .map_err(|e| format!("interleaved access: {e}"))?;
    // The line that makes this mode mean anything: with resampling off, an
    // unsupported rate fails here instead of being converted.
    hwp.set_rate_resample(false)
        .map_err(|e| format!("disabling alsa resampling: {e}"))?;

    // A named format the card takes wins; otherwise the widest it has.
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

    // Stereo where available; `fill` folds onto whatever comes back.
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
    // Start on a full buffer, not one period from an xrun.
    swp.set_start_threshold(buffer)
        .map_err(|e| format!("start threshold: {e}"))?;
    swp.set_avail_min(period)
        .map_err(|e| format!("avail min: {e}"))?;
    pcm.sw_params(&swp)
        .map_err(|e| format!("applying sw params: {e}"))?;

    Ok((sample, rate, channels as u16, name))
}

/// Read back, not remembered: it's the writer's buffer length.
fn period_frames(pcm: &PCM) -> Result<usize, String> {
    let hwp = pcm
        .hw_params_current()
        .map_err(|e| format!("reading period size: {e}"))?;
    let period = hwp
        .get_period_size()
        .map_err(|e| format!("reading period size: {e}"))?;
    Ok(period.max(1) as usize)
}

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

/// cpal's callback, inverted. The buffer is allocated once here, so the loop
/// allocates nothing, locks nothing, and does no I/O beyond the write.
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
                // Undocumented, but dropping the rest of the period would put the clock
                // ahead of the card by frames `fill` already counted, so retry.
                Ok(0) => {
                    recoveries += 1;
                    if recoveries > 4 {
                        break;
                    }
                }
                Ok(n) => written += n,
                Err(e) => {
                    // Xrun or suspend. Retry the same frames so the clock doesn't run ahead of
                    // the card; a device that won't recover is gone and the app reopens.
                    recoveries += 1;
                    if recoveries > 4 || pcm.try_recover(e, true).is_err() {
                        return lost(&shared, format!("alsa write: {e}"));
                    }
                }
            }
        }
    }
    // Dropping the PCM releases the card. No drain: every path here is a
    // teardown with the caller blocked in join.
}

/// Picked up by the app's reopen path. Logging is fine: the thread is ending.
fn lost(shared: &Shared, message: String) {
    log::error!("exclusive output: {message}");
    shared.device_lost.store(true, Ordering::Release);
}
