//! The decode thread: Symphonia decode, gapless track boundary, seek, and the
//! producer side of the sample ring. Everything here is allowed to allocate,
//! lock, and block; the RT line is the ring in output.rs.
//!
//! Gapless (ADR 3): one long-lived stream, the decoder swaps at EOF and the
//! next track's first frame lands in the ring right behind the last. Encoder
//! delay/padding comes from the LAME/iTunes headers: symphonia 0.6 exposes it
//! as packet trim metadata and the mp3 decoder applies it, so the samples we
//! see are already the playable range. The spike verifies that claim against
//! real files; if it falls short we trim from Track::delay/padding ourselves,
//! which the ADR anticipated.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration as StdDuration;

use rtrb::Producer;
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{
    FormatOptions, FormatReader, SeekMode, SeekTo, TrackType,
};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Time, TimeBase, Timestamp};

use crate::resample::Resampler;
use crate::shared::{Segment, Shared, TrackInfo};

pub enum Cmd {
    TogglePause,
    Seek(f64),
    Next,
    Prev,
    Volume(f32),
    Quit,
}

/// One open file: reader, decoder, and the per-track conversion state.
struct Source {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    time_base: Option<TimeBase>,
    device_rate: u32,
    resampler: Resampler,
    /// Scratch for one decoded packet, interleaved in the file's channel
    /// count, reused across packets.
    scratch: Vec<f32>,
}

pub struct Engine {
    queue: Vec<PathBuf>,
    idx: usize,
    shared: Arc<Shared>,
    producer: Producer<f32>,
    device_rate: u32,
    rx: Receiver<Cmd>,
    /// Frames pushed on the frames_consumed clock; resynced after each flush.
    pushed_playable: u64,
    /// Decoded, converted samples waiting for ring space.
    pending: Vec<f32>,
    pending_pos: usize,
}

impl Engine {
    pub fn new(
        queue: Vec<PathBuf>,
        shared: Arc<Shared>,
        producer: Producer<f32>,
        device_rate: u32,
        rx: Receiver<Cmd>,
    ) -> Self {
        Engine {
            queue,
            idx: 0,
            shared,
            producer,
            device_rate,
            rx,
            pushed_playable: 0,
            pending: Vec::new(),
            pending_pos: 0,
        }
    }

    pub fn run(mut self) {
        let mut source = self.open_current(0);

        loop {
            // Commands first so pause/seek stay responsive even when the
            // ring is full and decode is idle.
            let mut flush_to: Option<FlushAction> = None;
            while let Ok(cmd) = self.rx.try_recv() {
                match cmd {
                    Cmd::TogglePause => {
                        let now = self.shared.playing.load(Ordering::Relaxed);
                        self.shared.playing.store(!now, Ordering::Relaxed);
                    }
                    Cmd::Volume(v) => {
                        let v = v.clamp(0.0, 2.0);
                        self.shared.volume_bits.store(v.to_bits(), Ordering::Relaxed);
                    }
                    Cmd::Seek(secs) => flush_to = Some(FlushAction::Seek(secs.max(0.0))),
                    Cmd::Next => {
                        if self.idx + 1 < self.queue.len() {
                            flush_to = Some(FlushAction::Track(self.idx + 1));
                        }
                    }
                    Cmd::Prev => {
                        flush_to = Some(FlushAction::Track(self.idx.saturating_sub(1)));
                    }
                    Cmd::Quit => return,
                }
            }

            if let Some(action) = flush_to {
                self.flush_ring();
                match action {
                    FlushAction::Track(i) => {
                        self.shared.ended.store(false, Ordering::Relaxed);
                        source = self.open_current(i);
                    }
                    FlushAction::Seek(secs) => {
                        if let Some(src) = source.as_mut() {
                            let landed = src.seek(secs);
                            self.register_segment(landed);
                        }
                    }
                }
                continue;
            }

            // Move pending samples into the ring. Ring full means we're
            // comfortably ahead; nap and go back to command handling.
            while self.pending_pos < self.pending.len() {
                match self.producer.push(self.pending[self.pending_pos]) {
                    Ok(()) => self.pending_pos += 1,
                    Err(_) => break,
                }
            }
            if self.pending_pos < self.pending.len() {
                std::thread::sleep(StdDuration::from_millis(3));
                continue;
            }
            self.pushed_playable += (self.pending.len() / 2) as u64;
            self.pending.clear();
            self.pending_pos = 0;

            // Refill from the decoder.
            match source.as_mut() {
                Some(src) => {
                    let device_rate = self.device_rate;
                    if !src.next_chunk(device_rate, &mut self.pending) {
                        // EOF: swap the decoder under the live stream. No
                        // flush, no stream teardown; this IS the gapless
                        // boundary.
                        source = if self.idx + 1 < self.queue.len() {
                            self.open_current(self.idx + 1)
                        } else {
                            None
                        };
                    }
                }
                None => {
                    // Queue exhausted: report ended once the ring drains.
                    let cap = self.producer.buffer().capacity();
                    if self.producer.slots() == cap {
                        self.shared.ended.store(true, Ordering::Relaxed);
                    }
                    std::thread::sleep(StdDuration::from_millis(20));
                }
            }
        }
    }

    /// Open queue[i], falling forward through unreadable files. Registers the
    /// position segment for the new track.
    fn open_current(&mut self, mut i: usize) -> Option<Source> {
        while i < self.queue.len() {
            match Source::open(&self.queue[i], self.device_rate) {
                Ok((src, info)) => {
                    self.idx = i;
                    self.shared.tracks.lock().unwrap()[i] = Some(info);
                    let at_frame = self.pushed_playable;
                    self.shared.segments.lock().unwrap().push(Segment {
                        at_frame,
                        track: i,
                        track_frame: 0,
                    });
                    return Some(src);
                }
                Err(e) => {
                    eprintln!("\nskipping {}: {e}", self.queue[i].display());
                    i += 1;
                }
            }
        }
        None
    }

    /// Have the callback discard everything queued, wait for the ring to
    /// drain, then resync our clock to what actually played.
    fn flush_ring(&mut self) {
        self.pending.clear();
        self.pending_pos = 0;
        self.shared.flush.store(true, Ordering::Release);
        let cap = self.producer.buffer().capacity();
        while self.producer.slots() < cap {
            std::thread::sleep(StdDuration::from_millis(2));
        }
        // One callback period of grace so an in-flight callback that read
        // flush=true finishes before new samples land. A late callback could
        // still eat the first few ms after a seek; acceptable for the spike.
        std::thread::sleep(StdDuration::from_millis(25));
        self.shared.flush.store(false, Ordering::Release);
        self.pushed_playable = self.shared.frames_consumed.load(Ordering::Relaxed);
    }

    fn register_segment(&self, track_secs: f64) {
        self.shared.segments.lock().unwrap().push(Segment {
            at_frame: self.pushed_playable,
            track: self.idx,
            track_frame: (track_secs * self.device_rate as f64).round() as u64,
        });
    }
}

enum FlushAction {
    Seek(f64),
    Track(usize),
}

/// Decode a whole file through the same path playback uses and report
/// (decoded frames, frames the container claims are playable). Equal numbers
/// mean the encoder delay/padding trim is exact, i.e. the gapless boundary
/// is sample-accurate by construction. No audio device involved.
pub fn count_frames(path: &PathBuf) -> Result<(u64, Option<u64>), String> {
    // Probe once for the source rate, then open for real with the device
    // rate equal to it, so the resampler is a passthrough and the count is
    // in source frames.
    let (probe, info) = Source::open(path, 48000)?;
    drop(probe);
    let (mut src, info) = Source::open(path, info.sample_rate)?;

    let mut decoded: u64 = 0;
    let mut chunk = Vec::new();
    loop {
        chunk.clear();
        if !src.next_chunk(info.sample_rate, &mut chunk) {
            break;
        }
        decoded += (chunk.len() / 2) as u64;
    }
    Ok((decoded, info.num_frames))
}

impl Source {
    fn open(path: &PathBuf, device_rate: u32) -> Result<(Source, TrackInfo), String> {
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());

        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }

        let format = symphonia::default::get_probe()
            .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
            .map_err(|e| format!("probe: {e}"))?;

        let track = format
            .default_track(TrackType::Audio)
            .ok_or("no audio track")?;
        let track_id = track.id;
        let time_base = track.time_base;

        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or("no audio codec parameters")?;
        let sample_rate = params.sample_rate.ok_or("unknown sample rate")?;
        let channels = params.channels.as_ref().map(|c| c.count()).unwrap_or(2);

        // num_frames already excludes encoder delay and padding in 0.6.
        let duration_secs = track
            .duration
            .zip(time_base)
            .and_then(|(dur, tb)| tb.calc_time(Timestamp::from(dur.get() as i64)))
            .map(|t| t.as_secs_f64())
            .or_else(|| track.num_frames.map(|n| n as f64 / sample_rate as f64));

        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(params, &AudioDecoderOptions::default())
            .map_err(|e| format!("decoder: {e}"))?;

        let info = TrackInfo {
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            duration_secs,
            num_frames: track.num_frames,
            sample_rate,
            channels,
        };

        Ok((
            Source {
                format,
                decoder,
                track_id,
                time_base,
                device_rate,
                resampler: Resampler::new(sample_rate, device_rate),
                scratch: Vec::new(),
            },
            info,
        ))
    }

    /// Decode packets until one yields samples, appending device-rate stereo
    /// to `out`. Returns false at end of stream.
    fn next_chunk(&mut self, device_rate: u32, out: &mut Vec<f32>) -> bool {
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(p)) => p,
                Ok(None) => return false,
                Err(e) => {
                    eprintln!("\npacket error, ending track: {e}");
                    return false;
                }
            };
            if packet.track_id != self.track_id {
                continue;
            }

            let (frames, rate, ch) = match self.decoder.decode(&packet) {
                Ok(decoded) => {
                    let frames = decoded.frames();
                    if frames == 0 {
                        continue;
                    }
                    let spec = decoded.spec();
                    let rate = spec.rate();
                    let ch = spec.channels().count();
                    self.scratch.resize(decoded.samples_interleaved(), 0.0);
                    decoded.copy_to_slice_interleaved(&mut self.scratch);
                    (frames, rate, ch)
                }
                // Corrupt or truncated packet: skip it, keep the track going.
                Err(Error::DecodeError(e)) => {
                    eprintln!("\ndecode error, skipping packet: {e}");
                    continue;
                }
                Err(Error::IoError(e)) => {
                    eprintln!("\nio error, skipping packet: {e}");
                    continue;
                }
                Err(e) => {
                    eprintln!("\nfatal decode error, ending track: {e}");
                    return false;
                }
            };

            if rate != self.resampler.src_rate() {
                self.resampler = Resampler::new(rate, device_rate);
            }

            // Fold to stereo: mono duplicates, extra channels drop. Real
            // downmix is engine work, not spike work.
            let stereo: Vec<f32> = match ch {
                2 => std::mem::take(&mut self.scratch),
                1 => {
                    let mut v = Vec::with_capacity(frames * 2);
                    for &s in &self.scratch {
                        v.push(s);
                        v.push(s);
                    }
                    v
                }
                n => {
                    let mut v = Vec::with_capacity(frames * 2);
                    for f in self.scratch.chunks_exact(n) {
                        v.push(f[0]);
                        v.push(f[1]);
                    }
                    v
                }
            };

            self.resampler.process(&stereo, out);
            if ch == 2 {
                self.scratch = stereo;
            }
            return true;
        }
    }

    /// Accurate seek. Returns the track position actually landed on, in
    /// seconds, which can differ from the request.
    fn seek(&mut self, secs: f64) -> f64 {
        let time = Time::try_from_secs_f64(secs).unwrap_or(Time::ZERO);
        match self.format.seek(
            SeekMode::Accurate,
            SeekTo::Time { time, track_id: Some(self.track_id) },
        ) {
            Ok(seeked) => {
                self.decoder.reset();
                self.resampler = Resampler::new(self.resampler.src_rate(), self.device_rate);
                self.time_base
                    .and_then(|tb| tb.calc_time(seeked.actual_ts))
                    .map(|t| t.as_secs_f64().max(0.0))
                    .unwrap_or(secs)
            }
            Err(e) => {
                eprintln!("\nseek failed: {e}");
                // Position is unchanged; report where we were by falling back
                // to the request so the display does not lie wildly.
                secs
            }
        }
    }
}
