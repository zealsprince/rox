//! ReplayGain measurement for files that have none (ADR 19): EBU R128
//! integrated loudness and true peak, in the [`crate::gain::ReplayGain`]
//! shape the tag side reads. Blocking; streams a packet at a time.
//!
//! It decodes for itself rather than through the engine's `Source`, which
//! resamples and folds to stereo: BS.1770 weights channels by position, so a
//! mono file duplicated into L and R measures 3 dB loud and a 5.1 mix loses
//! four channels.
//!
//! Album gain is the gated loudness of the whole record as one program, not
//! the mean of track gains. [`AlbumAnalysis`] keeps each track's meter and
//! merges block histories with `loudness_global_multiple`, so no second decode.

use std::path::Path;

use ebur128::{EbuR128, Mode};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Time, Timestamp};

use crate::engine::guard_decode;
use crate::gain::ReplayGain;

/// ReplayGain 2's reference, which every current tagger writes against.
pub const REFERENCE_LUFS: f64 = -18.0;

/// Between cancel checks and progress ticks: instant cancel, closures not
/// called per packet.
const TICK_SECS: f64 = 0.25;

/// Keeps its meter so the measurement can join an album without a second decode.
#[derive(Debug)]
pub struct TrackAnalysis {
    /// Gated per BS.1770. None when nothing clears the absolute gate: ebur128
    /// returns -inf, which would be an infinite boost.
    pub loudness_lufs: Option<f64>,
    /// Loudest true peak across channels, 1.0 full scale. None for all-zero samples.
    pub peak: Option<f32>,
    /// Short of the container's claim means the decode gave up partway.
    pub frames: u64,
    /// As the meter finished, in case the file changed them mid-stream.
    pub sample_rate: u32,
    pub channels: u32,
    meter: EbuR128,
}

impl TrackAnalysis {
    /// Negative for a loud master.
    pub fn gain_db(&self) -> Option<f32> {
        self.loudness_lufs.and_then(gain_db)
    }

    /// Album fields stay None: writing a track's own gain there would level a
    /// compilation track by itself.
    pub fn replay_gain(&self) -> ReplayGain {
        ReplayGain {
            track_db: self.gain_db(),
            track_peak: self.peak,
            ..ReplayGain::default()
        }
    }
}

#[derive(Debug, Default)]
pub struct AlbumAnalysis {
    tracks: Vec<TrackAnalysis>,
}

impl AlbumAnalysis {
    pub fn new() -> AlbumAnalysis {
        AlbumAnalysis::default()
    }

    /// Push order is the order [`AlbumAnalysis::replay_gains`] returns.
    pub fn push(&mut self, track: TrackAnalysis) {
        self.tracks.push(track);
    }

    pub fn tracks(&self) -> &[TrackAnalysis] {
        &self.tracks
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    /// Gated loudness over every track's blocks at once. Not an average: the
    /// relative gate runs against the record's mean. None when empty or all gated.
    pub fn loudness_lufs(&self) -> Option<f64> {
        if self.tracks.is_empty() {
            return None;
        }
        let lufs = EbuR128::loudness_global_multiple(self.tracks.iter().map(|t| &t.meter)).ok()?;
        lufs.is_finite().then_some(lufs)
    }

    pub fn gain_db(&self) -> Option<f32> {
        self.loudness_lufs().and_then(gain_db)
    }

    /// The loudest track peak: a boost has to clear the loudest moment on the record.
    pub fn peak(&self) -> Option<f32> {
        self.tracks
            .iter()
            .filter_map(|t| t.peak)
            .fold(None, |max: Option<f32>, p| {
                Some(max.map_or(p, |m| m.max(p)))
            })
    }

    pub fn replay_gains(&self) -> Vec<ReplayGain> {
        let album_db = self.gain_db();
        let album_peak = self.peak();
        self.tracks
            .iter()
            .map(|t| ReplayGain {
                track_db: t.gain_db(),
                track_peak: t.peak,
                album_db,
                album_peak,
            })
            .collect()
    }
}

/// None for a non-finite measurement, so silence stays untagged.
pub fn gain_db(lufs: f64) -> Option<f32> {
    lufs.is_finite().then_some((REFERENCE_LUFS - lufs) as f32)
}

/// Decode `path` end to end and measure it. Blocking.
///
/// A cancel is `Ok(None)`. `progress` gets (frames fed, frames claimed).
/// `Err` means the file couldn't be read at all; a decode that fails partway
/// measures what it got.
pub fn measure(
    path: &Path,
    should_continue: impl Fn() -> bool,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<Option<TrackAnalysis>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let origin = path.display().to_string();

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    // Probe and decoder setup parse file bytes, so a panic there is a file this
    // can't measure, not a dead worker.
    let mut format = guard_decode("probe", &origin, || {
        symphonia::default::get_probe().probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
    })?
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
    let mut rate = params.sample_rate.ok_or("unknown sample rate")?;
    let mut channels = params.channels.as_ref().map(|c| c.count()).unwrap_or(2) as u32;

    // The progress denominator. A zero from either field means unknown; a
    // fragmented MP4 states its length only in the movie header.
    let total_frames = track
        .num_frames
        .filter(|n| *n > 0)
        .or_else(|| {
            track
                .duration
                .filter(|dur| dur.get() > 0)
                .zip(time_base)
                .and_then(|(dur, tb)| tb.calc_time(Timestamp::from(dur.get() as i64)))
                .map(|t| (t.as_secs_f64() * rate as f64).round() as u64)
        })
        .or_else(|| {
            rox_library::mp4::fragment_duration_secs(path)
                .map(|secs| (secs * rate as f64).round() as u64)
        });

    let mut decoder = guard_decode("decoder setup", &origin, || {
        crate::codecs::registry().make_audio_decoder(params, &AudioDecoderOptions::default())
    })?
    .map_err(|e| format!("decoder: {e}"))?;

    let mut meter = new_meter(channels, rate)?;
    // Tracked here because reconfiguring the meter clears its peaks.
    let mut peak = 0.0f64;

    let mut scratch: Vec<f32> = Vec::new();
    let mut frames: u64 = 0;
    let mut since_tick: u64 = 0;
    let mut tick = tick_frames(rate);

    loop {
        // A panic ends the measurement for good; the decoder is never re-entered.
        let packet = match guard_decode("packet read", &origin, || format.next_packet())? {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(e) => {
                log::warn!(
                    "packet error, ending the measurement of {}: {e}",
                    path.display()
                );
                break;
            }
        };
        if packet.track_id != track_id {
            continue;
        }

        // The copy is inside the guard: the decoded audio is borrowed from the codec.
        let decoded = guard_decode("decode", &origin, || {
            decoder.decode(&packet).map(|decoded| {
                let frames = decoded.frames();
                if frames == 0 {
                    return None;
                }
                let spec = decoded.spec();
                let got = (frames, spec.rate(), spec.channels().count() as u32);
                scratch.resize(decoded.samples_interleaved(), 0.0);
                decoded.copy_to_slice_interleaved(&mut scratch);
                Some(got)
            })
        })?;

        let (packet_frames, packet_rate, packet_channels) = match decoded {
            Ok(Some(got)) => got,
            Ok(None) => continue,
            // Skip a corrupt packet, as playback does.
            Err(Error::DecodeError(e)) => {
                log::warn!("decode error, skipping packet: {e}");
                continue;
            }
            Err(Error::IoError(e)) => {
                log::warn!("io error, skipping packet: {e}");
                continue;
            }
            Err(e) => {
                log::error!("fatal decode error, ending the measurement: {e}");
                break;
            }
        };

        if (packet_rate, packet_channels) != (rate, channels) {
            // Format changed mid-file: reconfigure and keep the block history. Only the
            // unfinished 100 ms block at the seam is lost.
            peak = peak.max(meter_peak(&meter, channels));
            meter
                .change_parameters(packet_channels, packet_rate)
                .map_err(|e| {
                    format!(
                        "format changed mid-file to {packet_channels} ch at {packet_rate} Hz: {e}"
                    )
                })?;
            rate = packet_rate;
            channels = packet_channels;
            tick = tick_frames(rate);
        }

        meter
            .add_frames_f32(&scratch)
            .map_err(|e| format!("meter: {e}"))?;
        frames += packet_frames as u64;
        since_tick += packet_frames as u64;

        if since_tick >= tick {
            since_tick = 0;
            progress(frames, total_frames);
            if !should_continue() {
                return Ok(None);
            }
        }
    }

    if frames == 0 {
        return Err("no decodable audio".into());
    }
    peak = peak.max(meter_peak(&meter, channels));
    progress(frames, total_frames);

    Ok(Some(TrackAnalysis {
        loudness_lufs: meter.loudness_global().ok().filter(|l| l.is_finite()),
        peak: (peak > 0.0).then_some(peak as f32),
        frames,
        sample_rate: rate,
        channels,
        meter,
    }))
}

/// Decode one span of a file to mono at the file's own rate, for
/// `rox_acoustic::panns`. Not through `Source`, which resamples and folds to
/// stereo: the caller band-limits down to the model's rate itself, or content
/// above the new Nyquist folds into the band the model reads.
///
/// Mono is the plain channel mean, the fold these models were trained on.
/// Never BS.1770's weighted fold, which would attenuate 5.1 surrounds.
///
/// The seek is coarse: an embedding spans seconds, so a frame or two doesn't
/// matter. `max_secs` bounds one call. A cancel returns what was decoded.
pub fn decode_mono(
    path: &Path,
    from_secs: f64,
    max_secs: f64,
    should_continue: impl Fn() -> bool,
) -> Result<(u32, Vec<f32>), String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let origin = path.display().to_string();

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let mut format = guard_decode("probe", &origin, || {
        symphonia::default::get_probe().probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
    })?
    .map_err(|e| format!("probe: {e}"))?;

    let track = format
        .default_track(TrackType::Audio)
        .ok_or("no audio track")?;
    let track_id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or("no audio codec parameters")?;
    let rate = params.sample_rate.ok_or("unknown sample rate")?;

    let mut decoder = guard_decode("decoder setup", &origin, || {
        crate::codecs::registry().make_audio_decoder(params, &AudioDecoderOptions::default())
    })?
    .map_err(|e| format!("decoder: {e}"))?;

    if from_secs > 0.0 {
        let time = Time::try_from_secs_f64(from_secs).unwrap_or(Time::ZERO);
        // A failed seek decodes the head of the track instead: a worse excerpt, not
        // a broken one.
        let seeked = guard_decode("seek", &origin, || {
            format.seek(
                SeekMode::Coarse,
                SeekTo::Time {
                    time,
                    track_id: Some(track_id),
                },
            )
        })?;
        if let Err(e) = seeked {
            log::warn!("seek to {from_secs:.1}s in {} failed: {e}", path.display());
        }
        guard_decode("decoder reset", &origin, || decoder.reset())?;
    }

    let want = ((max_secs * rate as f64) as usize).max(1);
    let tick = tick_frames(rate) as usize;
    let mut mono: Vec<f32> = Vec::with_capacity(want.min(rate as usize * 60));
    let mut scratch: Vec<f32> = Vec::new();
    let mut since_tick = 0usize;

    while mono.len() < want {
        // A panic is this excerpt failing, not the worker. The decoder is never re-entered.
        let packet = match guard_decode("packet read", &origin, || format.next_packet())? {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(e) => {
                log::warn!("packet error, ending the decode of {}: {e}", path.display());
                break;
            }
        };
        if packet.track_id != track_id {
            continue;
        }
        let decoded = guard_decode("decode", &origin, || {
            decoder.decode(&packet).map(|decoded| {
                let spec = decoded.spec();
                let got = (
                    decoded.frames(),
                    spec.rate(),
                    spec.channels().count().max(1),
                );
                scratch.resize(decoded.samples_interleaved(), 0.0);
                decoded.copy_to_slice_interleaved(&mut scratch);
                got
            })
        })?;
        let (packet_frames, packet_rate, channels) = match decoded {
            Ok(got) => got,
            Err(Error::DecodeError(e)) => {
                log::warn!("decode error, skipping packet: {e}");
                continue;
            }
            Err(Error::IoError(e)) => {
                log::warn!("io error, skipping packet: {e}");
                continue;
            }
            Err(e) => {
                log::error!("fatal decode error, ending the decode: {e}");
                break;
            }
        };
        // Stop at a rate change: the caller resamples the whole buffer as one rate.
        if packet_rate != rate {
            log::warn!(
                "{} changes sample rate mid-file, ending the excerpt at the seam",
                path.display()
            );
            break;
        }
        for frame in scratch.chunks_exact(channels) {
            mono.push(frame.iter().sum::<f32>() / channels as f32);
        }

        since_tick += packet_frames;
        if since_tick >= tick {
            since_tick = 0;
            if !should_continue() {
                break;
            }
        }
    }

    if mono.is_empty() {
        return Err("no decodable audio".into());
    }
    mono.truncate(want);
    Ok((rate, mono))
}

/// TRUE_PEAK includes SAMPLE_PEAK; `true_peak` returns the higher.
fn new_meter(channels: u32, rate: u32) -> Result<EbuR128, String> {
    EbuR128::new(channels, rate, Mode::I | Mode::TRUE_PEAK)
        .map_err(|e| format!("meter for {channels} ch at {rate} Hz: {e}"))
}

fn meter_peak(meter: &EbuR128, channels: u32) -> f64 {
    (0..channels)
        .filter_map(|ch| meter.true_peak(ch).ok())
        .fold(0.0, f64::max)
}

fn tick_frames(rate: u32) -> u64 {
    ((rate as f64 * TICK_SECS) as u64).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::f64::consts::{PI, TAU};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn measured(path: &Path) -> TrackAnalysis {
        measure(path, || true, |_, _| {})
            .expect("the fixture measures")
            .expect("nothing cancelled it")
    }

    /// Unique per call, so parallel tests never share one.
    struct Fixtures(PathBuf);

    impl Drop for Fixtures {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    impl Fixtures {
        fn new(name: &str) -> Fixtures {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir()
                .join(format!("rox-analysis-{name}-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("fixture directory");
            Fixtures(dir)
        }

        /// 16-bit quantization sits around -96 dBFS, far below anything asserted.
        fn wav(&self, name: &str, rate: u32, channels: u16, samples: &[f32]) -> PathBuf {
            let block_align = channels * 2;
            let data_len = (samples.len() * 2) as u32;
            let mut out: Vec<u8> = Vec::with_capacity(44 + data_len as usize);
            out.extend_from_slice(b"RIFF");
            out.extend_from_slice(&(36 + data_len).to_le_bytes());
            out.extend_from_slice(b"WAVEfmt ");
            out.extend_from_slice(&16u32.to_le_bytes());
            out.extend_from_slice(&1u16.to_le_bytes()); // PCM
            out.extend_from_slice(&channels.to_le_bytes());
            out.extend_from_slice(&rate.to_le_bytes());
            out.extend_from_slice(&(rate * block_align as u32).to_le_bytes());
            out.extend_from_slice(&block_align.to_le_bytes());
            out.extend_from_slice(&16u16.to_le_bytes());
            out.extend_from_slice(b"data");
            out.extend_from_slice(&data_len.to_le_bytes());
            for &s in samples {
                let q = (s.clamp(-1.0, 1.0) as f64 * 32767.0).round() as i16;
                out.extend_from_slice(&q.to_le_bytes());
            }
            let path = self.0.join(name);
            std::fs::write(&path, out).expect("writing the fixture");
            path
        }

        fn missing(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }

        fn junk(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, vec![0x7fu8; 4096]).expect("writing the fixture");
            path
        }
    }

    fn sine(rate: u32, channels: usize, secs: f64, freq: f64, amp: f64, phase: f64) -> Vec<f32> {
        let frames = (secs * rate as f64) as usize;
        let mut out = Vec::with_capacity(frames * channels);
        for i in 0..frames {
            let s = (phase + i as f64 * freq * TAU / rate as f64).sin() * amp;
            for _ in 0..channels {
                out.push(s as f32);
            }
        }
        out
    }

    fn biquad_magnitude(b: [f64; 3], a: [f64; 3], f_norm: f64) -> f64 {
        let w = TAU * f_norm;
        let (c1, s1) = ((-w).cos(), (-w).sin());
        let (c2, s2) = ((-2.0 * w).cos(), (-2.0 * w).sin());
        let num = (b[0] + b[1] * c1 + b[2] * c2, b[1] * s1 + b[2] * s2);
        let den = (a[0] + a[1] * c1 + a[2] * c2, a[1] * s1 + a[2] * s2);
        num.0.hypot(num.1) / den.0.hypot(den.1)
    }

    /// K-weighting magnitude from BS.1770-4's own constants. Independent
    /// arithmetic from there, so this checks the meter rather than restating it.
    fn k_weight_magnitude(rate: f64, freq: f64) -> f64 {
        let shelf = {
            let f0 = 1681.974450955533;
            let g = 3.999843853973347;
            let q = 0.7071752369554196;
            let k = (PI * f0 / rate).tan();
            let vh = 10f64.powf(g / 20.0);
            let vb = vh.powf(0.4996667741545416);
            let a0 = 1.0 + k / q + k * k;
            biquad_magnitude(
                [
                    (vh + vb * k / q + k * k) / a0,
                    2.0 * (k * k - vh) / a0,
                    (vh - vb * k / q + k * k) / a0,
                ],
                [1.0, 2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
                freq / rate,
            )
        };
        let highpass = {
            let f0 = 38.13547087602444;
            let q = 0.5003270373238773;
            let k = (PI * f0 / rate).tan();
            let a0 = 1.0 + k / q + k * k;
            biquad_magnitude(
                [1.0, -2.0, 1.0],
                [1.0, 2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
                freq / rate,
            )
        };
        shelf * highpass
    }

    /// BS.1770's value for a steady sine on every channel. 1 or 2 channels only,
    /// where every weight is 1.0.
    fn analytic_lufs(rate: u32, channels: u32, freq: f64, amp: f64) -> f64 {
        let h = k_weight_magnitude(rate as f64, freq);
        let mean_square = amp * amp / 2.0 * h * h;
        -0.691 + 10.0 * (mean_square * channels as f64).log10()
    }

    #[test]
    fn a_sine_measures_the_loudness_the_standard_says_it_has() {
        let fx = Fixtures::new("sine-loudness");
        let path = fx.wav(
            "tone.wav",
            48_000,
            2,
            &sine(48_000, 2, 5.0, 1000.0, 0.5, 0.0),
        );
        let measured = measured(&path).loudness_lufs.expect("a tone measures");
        let expected = analytic_lufs(48_000, 2, 1000.0, 0.5);
        assert!(
            (measured - expected).abs() < 0.2,
            "measured {measured:.3} LUFS against an analytic {expected:.3}"
        );
    }

    #[test]
    fn halving_the_amplitude_costs_exactly_six_db() {
        let fx = Fixtures::new("sine-halved");
        let loud = fx.wav(
            "loud.wav",
            48_000,
            2,
            &sine(48_000, 2, 4.0, 1000.0, 0.5, 0.0),
        );
        let quiet = fx.wav(
            "quiet.wav",
            48_000,
            2,
            &sine(48_000, 2, 4.0, 1000.0, 0.25, 0.0),
        );
        let delta =
            measured(&loud).loudness_lufs.unwrap() - measured(&quiet).loudness_lufs.unwrap();
        assert!(
            (delta - 20.0 * 2f64.log10()).abs() < 0.05,
            "half the amplitude came out {delta:.3} LU apart"
        );
    }

    #[test]
    fn a_mono_file_is_not_folded_up_into_stereo() {
        // Playback duplicates mono into both channels, which would read 3 dB out.
        let fx = Fixtures::new("mono");
        let mono = fx.wav(
            "mono.wav",
            48_000,
            1,
            &sine(48_000, 1, 4.0, 1000.0, 0.5, 0.0),
        );
        let stereo = fx.wav(
            "stereo.wav",
            48_000,
            2,
            &sine(48_000, 2, 4.0, 1000.0, 0.5, 0.0),
        );
        let mono = measured(&mono);
        assert_eq!(mono.channels, 1);
        let delta = measured(&stereo).loudness_lufs.unwrap() - mono.loudness_lufs.unwrap();
        assert!(
            (delta - 10.0 * 2f64.log10()).abs() < 0.1,
            "the same tone in mono and stereo came out {delta:.3} LU apart"
        );
    }

    #[test]
    fn the_peak_is_the_loudest_sample_at_least() {
        let fx = Fixtures::new("sample-peak");
        let path = fx.wav(
            "tone.wav",
            48_000,
            2,
            &sine(48_000, 2, 1.0, 1000.0, 0.8, 0.0),
        );
        let peak = measured(&path).peak.expect("a tone has a peak");
        assert!(
            (0.79..0.83).contains(&peak),
            "a 0.8 amplitude tone peaked at {peak}"
        );
    }

    #[test]
    fn true_peak_catches_what_lands_between_two_samples() {
        // A quarter-rate sine an eighth of a cycle off: every sample sits at
        // 1/sqrt(2) of the crest. Sample peak 0.636, real peak 0.9.
        let fx = Fixtures::new("true-peak");
        let samples = sine(48_000, 2, 1.0, 12_000.0, 0.9, PI / 4.0);
        let sample_peak = samples.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        assert!(
            (sample_peak - 0.636).abs() < 0.01,
            "the fixture is the one described"
        );
        let path = fx.wav("intersample.wav", 48_000, 2, &samples);
        let peak = measured(&path).peak.expect("a tone has a peak");
        assert!(
            peak > sample_peak * 1.2,
            "true peak {peak} never got past the sample peak {sample_peak}"
        );
        assert!(
            (0.8..1.0).contains(&peak),
            "true peak {peak} missed the 0.9 the waveform actually reaches"
        );
    }

    #[test]
    fn silence_measures_as_nothing_to_level_by() {
        let fx = Fixtures::new("silence");
        let path = fx.wav("quiet.wav", 48_000, 2, &vec![0.0; 48_000 * 2 * 3]);
        let analysis = measured(&path);
        assert!(analysis.frames > 0, "the file decoded");
        assert_eq!(analysis.loudness_lufs, None, "everything gated out");
        assert_eq!(analysis.peak, None);
        assert_eq!(analysis.replay_gain(), ReplayGain::default());
    }

    #[test]
    fn an_album_gates_its_tracks_as_one_program() {
        let fx = Fixtures::new("album-between");
        let loud = fx.wav(
            "loud.wav",
            48_000,
            2,
            &sine(48_000, 2, 4.0, 1000.0, 0.5, 0.0),
        );
        let quiet = fx.wav(
            "quiet.wav",
            48_000,
            2,
            &sine(48_000, 2, 4.0, 1000.0, 0.125, 0.0),
        );

        let mut album = AlbumAnalysis::new();
        album.push(measured(&loud));
        album.push(measured(&quiet));

        let loud_lufs = album.tracks()[0].loudness_lufs.unwrap();
        let quiet_lufs = album.tracks()[1].loudness_lufs.unwrap();
        let album_lufs = album.loudness_lufs().expect("the album measures");
        assert!(
            quiet_lufs < album_lufs && album_lufs < loud_lufs,
            "album {album_lufs:.3} isn't between {quiet_lufs:.3} and {loud_lufs:.3}"
        );
        // Not the mean either: the relative gate drops the quiet track's blocks.
        assert!(album_lufs > (loud_lufs + quiet_lufs) / 2.0);
    }

    #[test]
    fn an_album_measures_what_its_tracks_end_to_end_would() {
        let fx = Fixtures::new("album-concat");
        let first = sine(48_000, 2, 3.0, 1000.0, 0.5, 0.0);
        let second = sine(48_000, 2, 3.0, 1000.0, 0.2, 0.0);
        let a = fx.wav("a.wav", 48_000, 2, &first);
        let b = fx.wav("b.wav", 48_000, 2, &second);
        let joined: Vec<f32> = first.iter().chain(second.iter()).copied().collect();
        let whole = fx.wav("joined.wav", 48_000, 2, &joined);

        let mut album = AlbumAnalysis::new();
        album.push(measured(&a));
        album.push(measured(&b));

        let album_lufs = album.loudness_lufs().unwrap();
        let whole_lufs = measured(&whole).loudness_lufs.unwrap();
        // Not bit-identical: each file restarts the K-weighting filter and the joined
        // file has gating blocks straddling the seam.
        assert!(
            (album_lufs - whole_lufs).abs() < 0.1,
            "album {album_lufs:.3} against the same audio as one file {whole_lufs:.3}"
        );
    }

    #[test]
    fn the_album_peak_is_the_loudest_track_peak() {
        let fx = Fixtures::new("album-peak");
        let soft = fx.wav(
            "soft.wav",
            48_000,
            2,
            &sine(48_000, 2, 1.0, 1000.0, 0.3, 0.0),
        );
        let hot = fx.wav(
            "hot.wav",
            48_000,
            2,
            &sine(48_000, 2, 1.0, 1000.0, 0.9, 0.0),
        );

        let mut album = AlbumAnalysis::new();
        album.push(measured(&soft));
        album.push(measured(&hot));

        let peak = album.peak().expect("the album has a peak");
        let hottest = album.tracks()[1].peak.unwrap();
        assert!((peak - hottest).abs() < f32::EPSILON);

        let gains = album.replay_gains();
        assert_eq!(gains.len(), 2);
        assert_eq!(gains[0].album_peak, Some(peak));
        assert_eq!(gains[1].album_peak, Some(peak));
        assert_eq!(gains[0].album_db, gains[1].album_db);
        assert_ne!(
            gains[0].track_db, gains[1].track_db,
            "two levels, two track gains"
        );
    }

    #[test]
    fn an_empty_album_has_no_numbers() {
        let album = AlbumAnalysis::new();
        assert!(album.is_empty());
        assert_eq!(album.loudness_lufs(), None);
        assert_eq!(album.gain_db(), None);
        assert_eq!(album.peak(), None);
        assert!(album.replay_gains().is_empty());
    }

    #[test]
    fn cancelling_stops_the_pass_partway() {
        let fx = Fixtures::new("cancel");
        let path = fx.wav(
            "long.wav",
            48_000,
            2,
            &sine(48_000, 2, 20.0, 1000.0, 0.5, 0.0),
        );

        let ticks = Cell::new(0u32);
        let furthest = Cell::new(0u64);
        let out = measure(
            &path,
            || {
                ticks.set(ticks.get() + 1);
                ticks.get() < 2
            },
            |done, _| furthest.set(done),
        )
        .expect("the file itself is fine");

        assert!(out.is_none(), "a cancelled pass measures nothing");
        assert_eq!(ticks.get(), 2, "it stopped on the tick that said stop");
        assert!(
            furthest.get() < 48_000 * 20,
            "it read {} frames of a 20 second file",
            furthest.get()
        );
    }

    #[test]
    fn progress_counts_up_to_the_length_the_container_claims() {
        let fx = Fixtures::new("progress");
        let path = fx.wav(
            "tone.wav",
            48_000,
            2,
            &sine(48_000, 2, 2.0, 1000.0, 0.5, 0.0),
        );

        let mut calls: Vec<(u64, Option<u64>)> = Vec::new();
        let analysis = measure(&path, || true, |done, total| calls.push((done, total)))
            .expect("the fixture measures")
            .expect("nothing cancelled it");

        assert!(
            calls.len() > 4,
            "a two second file ticked {} times",
            calls.len()
        );
        assert!(
            calls.windows(2).all(|w| w[0].0 <= w[1].0),
            "progress went backwards"
        );
        let (done, total) = *calls.last().unwrap();
        assert_eq!(done, analysis.frames);
        assert_eq!(total, Some(96_000), "the wav states its own length");
        assert_eq!(analysis.frames, 96_000);
    }

    #[test]
    fn a_file_that_will_not_decode_comes_back_as_an_error() {
        let fx = Fixtures::new("undecodable");
        assert!(measure(&fx.missing("gone.wav"), || true, |_, _| {}).is_err());
        assert!(measure(&fx.junk("junk.wav"), || true, |_, _| {}).is_err());
        assert!(measure(&fx.wav("empty.wav", 48_000, 2, &[]), || true, |_, _| {}).is_err());
    }

    #[test]
    fn a_gain_is_the_reference_minus_the_measurement() {
        assert_eq!(gain_db(REFERENCE_LUFS), Some(0.0));
        assert_eq!(gain_db(-8.0), Some(-10.0));
        assert_eq!(gain_db(-23.0), Some(5.0));
        assert_eq!(gain_db(f64::NEG_INFINITY), None);
        assert_eq!(gain_db(f64::NAN), None);
    }

    #[test]
    fn a_mono_excerpt_keeps_the_files_own_rate_and_stops_at_the_cap() {
        let fx = Fixtures::new("decode-mono");
        let path = fx.wav(
            "tone.wav",
            44_100,
            2,
            &sine(44_100, 2, 4.0, 1000.0, 0.5, 0.0),
        );
        let (rate, samples) = decode_mono(&path, 0.0, 1.5, || true).expect("the fixture decodes");
        assert_eq!(rate, 44_100, "no resampling on the way out");
        assert_eq!(samples.len(), 66_150, "1.5 s at the file's own rate");
        let peak = samples.iter().cloned().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((peak - 0.5).abs() < 0.01, "peak came back {peak}");
    }

    #[test]
    fn a_short_file_and_a_cancel_both_return_what_they_got() {
        let fx = Fixtures::new("decode-mono-short");
        let path = fx.wav(
            "short.wav",
            48_000,
            1,
            &sine(48_000, 1, 0.5, 440.0, 0.4, 0.0),
        );
        let (_, all) = decode_mono(&path, 0.0, 60.0, || true).expect("the fixture decodes");
        assert_eq!(all.len(), 24_000);

        let calls = Cell::new(0);
        let (_, stopped) = decode_mono(&path, 0.0, 60.0, || {
            calls.set(calls.get() + 1);
            false
        })
        .expect("a cancel is not a failure");
        assert!(calls.get() > 0, "the cancel hook should have been polled");
        assert!(!stopped.is_empty());
        assert!(stopped.len() <= all.len());
    }

    /// Opposed channels cancel: the plain mean, not BS.1770's weighted fold.
    #[test]
    fn the_mono_fold_is_the_plain_channel_mean() {
        let fx = Fixtures::new("decode-mono-fold");
        let left = sine(44_100, 1, 0.5, 440.0, 0.5, 0.0);
        let right = sine(44_100, 1, 0.5, 440.0, 0.5, PI);
        let interleaved: Vec<f32> = left
            .iter()
            .zip(&right)
            .flat_map(|(&l, &r)| [l, r])
            .collect();
        let path = fx.wav("opposed.wav", 44_100, 2, &interleaved);
        let (_, samples) = decode_mono(&path, 0.0, 1.0, || true).expect("the fixture decodes");
        let peak = samples.iter().cloned().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(
            peak < 0.01,
            "opposed channels should cancel, peak was {peak}"
        );
    }

    #[test]
    fn an_undecodable_file_is_an_error_rather_than_an_empty_excerpt() {
        let fx = Fixtures::new("decode-mono-bad");
        assert!(decode_mono(&fx.missing("gone.wav"), 0.0, 1.0, || true).is_err());
        assert!(decode_mono(&fx.junk("junk.wav"), 0.0, 1.0, || true).is_err());
        assert!(decode_mono(&fx.wav("empty.wav", 48_000, 2, &[]), 0.0, 1.0, || true).is_err());
    }

    /// The Opus fixture through the shared registry: analysis and playback agree
    /// about what decodes.
    #[test]
    fn an_opus_file_measures_a_finite_gain() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tone-440.opus");
        let out = measure(&path, || true, |_, _| {})
            .expect("the fixture decodes")
            .expect("nothing asked it to stop");
        let gain = out.gain_db();
        assert!(
            gain.is_some_and(f32::is_finite),
            "a real tone measures a real gain, got {gain:?}"
        );
        assert!(
            out.frames.abs_diff(48_000) <= 1,
            "and it measured the trimmed second, got {} frames",
            out.frames
        );
    }
}
