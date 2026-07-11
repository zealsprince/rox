//! The cpal output stream and the real-time callback. This is the hard line
//! from the components spec: the callback pops a pre-allocated ring and reads
//! atomics. No allocation, no lock, no logging, no I/O.
//!
//! Kept in its own module because ADR 9 wants the output layer swappable; a
//! bit-perfect backend would replace this file, not the engine.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, FromSample, SampleFormat, SizedSample, Stream, StreamConfig};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::shared::Shared;

/// Half a second of buffered stereo between decode and the callback.
const RING_SECS: f64 = 0.5;
/// Tap capacity in samples. Small on purpose: the tap consumer may lag and
/// lose, never backpressure.
const TAP_SAMPLES: usize = 16384;

pub struct OpenOutput {
    /// Held so the stream stays alive; dropping it stops audio.
    pub stream: Stream,
    pub sample_rate: u32,
    pub ring_frames: usize,
    /// Decode thread's side of the sample ring (interleaved stereo f32).
    pub producer: Producer<f32>,
    /// Visualizer side of the PCM tap.
    pub tap: Consumer<f32>,
}

/// Open the default output device, allocate both rings, start the stream.
pub fn open(shared: Arc<Shared>) -> Result<OpenOutput, String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("no default output device")?;
    let supported = device
        .default_output_config()
        .map_err(|e| format!("no default output config: {e}"))?;
    let sample_format = supported.sample_format();
    let config: StreamConfig = supported.into();
    let sample_rate = config.sample_rate;

    let ring_frames = (sample_rate as f64 * RING_SECS) as usize;
    let (producer, ring) = RingBuffer::<f32>::new(ring_frames * 2);
    let (tap_tx, tap) = RingBuffer::<f32>::new(TAP_SAMPLES);

    let stream = match sample_format {
        SampleFormat::F32 => build::<f32>(&device, &config, shared, ring, tap_tx),
        SampleFormat::I16 => build::<i16>(&device, &config, shared, ring, tap_tx),
        SampleFormat::U16 => build::<u16>(&device, &config, shared, ring, tap_tx),
        SampleFormat::I32 => build::<i32>(&device, &config, shared, ring, tap_tx),
        other => return Err(format!("unsupported device sample format {other}")),
    }?;
    stream.play().map_err(|e| format!("stream play: {e}"))?;

    Ok(OpenOutput { stream, sample_rate, ring_frames, producer, tap })
}

fn build<T>(
    device: &Device,
    config: &StreamConfig,
    shared: Arc<Shared>,
    mut ring: Consumer<f32>,
    mut tap: Producer<f32>,
) -> Result<Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let device_channels = config.channels as usize;

    let callback = move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
        // A seek is in flight: throw away whatever the decode thread queued
        // before it, play silence, and advance nothing on the clock.
        if shared.flush.load(Ordering::Acquire) {
            while ring.pop().is_ok() {}
            data.fill(T::from_sample(0.0f32));
            return;
        }

        if !shared.playing.load(Ordering::Relaxed) {
            data.fill(T::from_sample(0.0f32));
            return;
        }

        let volume = f32::from_bits(shared.volume_bits.load(Ordering::Relaxed));
        let mut frames_out: u64 = 0;

        for frame in data.chunks_mut(device_channels) {
            // The ring carries whole stereo frames; only pop when both
            // samples are there so interleaving can't slip. Dry ring means
            // underrun (or end of queue): emit silence, don't count it.
            if ring.slots() < 2 {
                frame.fill(T::from_sample(0.0f32));
                continue;
            }
            let l = ring.pop().unwrap() * volume;
            let r = ring.pop().unwrap() * volume;

            // Lossy PCM tap: if the visualizer side is behind, drop, never
            // wait. Tapped post-volume, i.e. what the device gets.
            let _ = tap.push(l);
            let _ = tap.push(r);

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

        if frames_out > 0 {
            shared
                .frames_consumed
                .fetch_add(frames_out, Ordering::Relaxed);
        }
    };

    let err_fn = |err: cpal::Error| eprintln!("\nstream error: {err}");

    device
        .build_output_stream(*config, callback, err_fn, None)
        .map_err(|e| format!("build_output_stream: {e}"))
}
