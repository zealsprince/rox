//! AcoustID fingerprints (ADR 14): the first two minutes of a file as the
//! string acoustid.org's lookup takes. Chromaprint folds to 11025 Hz mono and
//! quantizes chroma bands, so encodes of one recording land close together:
//! it identifies a file when its tags say nothing. `rusty-chromaprint` is a
//! pure Rust port, so no C (ADR 2).
//!
//! It decodes for itself like [`crate::analysis`]: chromaprint does its own
//! resample and downmix, and feeding it playback's converted output would
//! drift from what fpcalc computes from the file.
//!
//! [`WINDOW_SECS`] is fpcalc's default `-length` (120, `src/cmd/fpcalc.cpp`),
//! the span AcoustID's index was built over. The algorithm is TEST2,
//! `CHROMAPRINT_ALGORITHM_DEFAULT`. Chromaprint's base64 table ends `-_` with
//! no padding, so `URL_SAFE_NO_PAD` is the exact wire form.
//!
//! Refused as `Err`, never a partial string: under ~3 s of audio (the
//! fingerprint comes back empty), a format change before [`MIN_SECS`], and
//! anything that won't open.

use std::path::Path;

use base64::Engine as _;
use rusty_chromaprint::{Configuration, FingerprintCompressor, Fingerprinter};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Timestamp;

use crate::engine::guard_decode;

pub const WINDOW_SECS: u32 = 120;

/// A mid-file format change after this much audio ends the window at the seam
/// instead of failing; well past the ~3 s the algorithm needs.
pub const MIN_SECS: u32 = 15;

/// Between cancel checks, matching [`crate::analysis`].
const TICK_SECS: f64 = 0.25;

#[derive(Debug, Clone)]
pub struct Fingerprint {
    /// URL-safe base64, no padding: what AcoustID takes verbatim.
    pub encoded: String,
    /// Whole-file length. None when the container doesn't say; the caller then
    /// substitutes the library's duration, since AcoustID needs one.
    pub duration_secs: Option<u32>,
}

/// Fingerprint the head of `path`. Blocking. A cancel returns
/// `Err("cancelled")`: a half-read window isn't a fingerprint anyone can look
/// up. A decode that fails partway keeps what it already fed in.
pub fn compute(path: &Path, should_continue: impl Fn() -> bool) -> Result<Fingerprint, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let origin = path.display().to_string();

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    // Guarded like [`crate::analysis::measure`]: a panic here is a file this
    // can't fingerprint, not a dead worker.
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
    let rate = params.sample_rate.ok_or("unknown sample rate")?;
    let channels = params.channels.as_ref().map(|c| c.count()).unwrap_or(2) as u32;

    // The whole file's length, which AcoustID matches against: frame count,
    // then declared duration, then a fragmented MP4's movie header.
    let duration_secs = track
        .num_frames
        .filter(|n| *n > 0)
        .map(|n| n as f64 / rate as f64)
        .or_else(|| {
            track
                .duration
                .filter(|dur| dur.get() > 0)
                .zip(time_base)
                .and_then(|(dur, tb)| tb.calc_time(Timestamp::from(dur.get() as i64)))
                .map(|t| t.as_secs_f64())
        })
        .or_else(|| rox_library::mp4::fragment_duration_secs(path))
        .map(|secs| secs.round() as u32)
        .filter(|secs| *secs > 0);

    let mut decoder = guard_decode("decoder setup", &origin, || {
        crate::codecs::registry().make_audio_decoder(params, &AudioDecoderOptions::default())
    })?
    .map_err(|e| format!("decoder: {e}"))?;

    let config = Configuration::preset_test2();
    let mut printer = Fingerprinter::new(&config);
    printer.start(rate, channels).map_err(|e| {
        format!(
            "fingerprinter for {channels} ch at {rate} Hz: {}",
            e.to_string().trim()
        )
    })?;

    let want = WINDOW_SECS as u64 * rate as u64;
    let least = MIN_SECS as u64 * rate as u64;
    let tick = ((rate as f64 * TICK_SECS) as u64).max(1);

    let mut scratch: Vec<f32> = Vec::new();
    let mut pcm: Vec<i16> = Vec::new();
    let mut frames: u64 = 0;
    let mut since_tick: u64 = 0;

    while frames < want {
        // A panic ends the fingerprint for good; the decoder is never re-entered.
        let packet = match guard_decode("packet read", &origin, || format.next_packet())? {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(e) => {
                log::warn!(
                    "packet error, ending the fingerprint of {}: {e}",
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
                log::error!("fatal decode error, ending the fingerprint: {e}");
                break;
            }
        };

        // Restarting the fingerprinter would discard everything so far, so a format
        // change ends the window if there's enough behind it and refuses if not.
        if (packet_rate, packet_channels) != (rate, channels) {
            if frames < least {
                return Err(format!(
                    "format changed to {packet_channels} ch at {packet_rate} Hz after {:.1}s, too early to fingerprint",
                    frames as f64 / rate as f64
                ));
            }
            log::warn!(
                "{} changes to {packet_channels} ch at {packet_rate} Hz mid-file, ending the fingerprint window at the seam",
                path.display()
            );
            break;
        }

        // Scale by 32768 and clamp, the conversion ffmpeg hands fpcalc.
        let take = ((want - frames) as usize).min(packet_frames) * channels as usize;
        pcm.clear();
        pcm.extend(
            scratch[..take]
                .iter()
                .map(|s| (s * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16),
        );
        printer.consume(&pcm);

        let taken = (take / channels as usize) as u64;
        frames += taken;
        since_tick += taken;

        if since_tick >= tick {
            since_tick = 0;
            if !should_continue() {
                return Err("cancelled".into());
            }
        }
    }

    if frames == 0 {
        return Err("no decodable audio".into());
    }

    printer.finish();
    let raw = printer.fingerprint();
    if raw.is_empty() {
        // Under ~3 s there's nothing to compress, and encoding it anyway gives a
        // header every short file shares.
        return Err(format!(
            "{:.1}s of audio is too short to fingerprint",
            frames as f64 / rate as f64
        ));
    }

    let compressed = FingerprintCompressor::from(&config).compress(raw);
    Ok(Fingerprint {
        encoded: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(compressed),
        duration_secs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::f64::consts::TAU;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

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
                .join(format!("rox-fingerprint-{name}-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("fixture directory");
            Fixtures(dir)
        }

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

    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
    }

    /// Detuned per channel so they don't collapse to mid; a chord puts several
    /// chroma bands in play.
    fn chord(rate: u32, secs: f64, partials: &[f64]) -> Vec<f32> {
        let frames = (secs * rate as f64) as usize;
        let amp = 0.5 / partials.len() as f64;
        let mut out = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let t = i as f64 / rate as f64;
            for ch in 0..2 {
                let detune = 1.0 + ch as f64 * 0.001;
                let s: f64 = partials
                    .iter()
                    .map(|f| (t * f * detune * TAU).sin() * amp)
                    .sum();
                out.push(s as f32);
            }
        }
        out
    }

    /// Determinism is the point: AcoustID's index needs the same string every time.
    #[test]
    fn a_tone_fingerprints_to_a_stable_string() {
        let fx = Fixtures::new("stable");
        let path = fx.wav(
            "chord.wav",
            48_000,
            2,
            &chord(48_000, 10.0, &[220.0, 277.0, 330.0]),
        );

        let first = compute(&path, || true).expect("the fixture fingerprints");
        let second = compute(&path, || true).expect("and it fingerprints again");

        assert!(
            !first.encoded.is_empty(),
            "a fingerprint encodes to something"
        );
        assert_eq!(
            first.encoded, second.encoded,
            "two passes over the same file agree"
        );
        assert!(
            first
                .encoded
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "URL-safe base64 without padding, got {}",
            first.encoded
        );
        assert_eq!(
            first.duration_secs,
            Some(10),
            "and the container's own length comes back whole"
        );
    }

    /// Guards against an encode that returns a constant.
    #[test]
    fn different_audio_fingerprints_differently() {
        let fx = Fixtures::new("distinct");
        let a = fx.wav(
            "a.wav",
            48_000,
            2,
            &chord(48_000, 10.0, &[220.0, 277.0, 330.0]),
        );
        let b = fx.wav(
            "b.wav",
            48_000,
            2,
            &chord(48_000, 10.0, &[233.0, 311.0, 370.0]),
        );

        let a = compute(&a, || true).expect("the first fixture fingerprints");
        let b = compute(&b, || true).expect("the second fixture fingerprints");
        assert_ne!(a.encoded, b.encoded, "two chords are two fingerprints");
    }

    /// Audio past the window never reaches the fingerprinter.
    #[test]
    fn the_window_caps_what_goes_in() {
        let fx = Fixtures::new("window");
        let partials = [220.0, 277.0, 330.0];
        let long = fx.wav(
            "long.wav",
            8_000,
            2,
            &chord(8_000, WINDOW_SECS as f64 + 20.0, &partials),
        );
        let cut = fx.wav(
            "cut.wav",
            8_000,
            2,
            &chord(8_000, WINDOW_SECS as f64, &partials),
        );

        let long = compute(&long, || true).expect("the long fixture fingerprints");
        let cut = compute(&cut, || true).expect("the cut fixture fingerprints");
        assert_eq!(
            long.encoded, cut.encoded,
            "the audio past the window never reached the fingerprinter"
        );
        assert_eq!(
            long.duration_secs,
            Some(WINDOW_SECS + 20),
            "while the duration reported is still the whole file's"
        );
    }

    /// A fingerprint of whatever got read first would quietly match the wrong thing.
    #[test]
    fn a_cancelled_decode_is_an_error() {
        let fx = Fixtures::new("cancel");
        let path = fx.wav(
            "chord.wav",
            48_000,
            2,
            &chord(48_000, 10.0, &[220.0, 277.0, 330.0]),
        );

        let polls = Cell::new(0u32);
        let err = compute(&path, || {
            polls.set(polls.get() + 1);
            false
        })
        .expect_err("a cancel stops the decode");
        assert_eq!(err, "cancelled");
        assert_eq!(polls.get(), 1, "and it stops on the first poll");
    }

    #[test]
    fn a_file_that_is_not_audio_is_an_error() {
        let fx = Fixtures::new("not-audio");
        assert!(compute(&fx.missing("gone.wav"), || true).is_err());
        assert!(compute(&fx.junk("junk.wav"), || true).is_err());
        assert!(compute(&fx.wav("empty.wav", 48_000, 2, &[]), || true).is_err());
    }

    /// Both fixtures are under ~3 s, the algorithm's floor: refused for length,
    /// not as a decode failure.
    #[test]
    fn a_file_shorter_than_the_algorithm_needs_is_refused() {
        for (name, secs) in [("tone-440.opus", 1.0), ("dense-transients.opus", 2.0)] {
            let err = compute(&fixtures().join(name), || true)
                .expect_err("under three seconds there is no fingerprint to take");
            assert!(
                err.ends_with("is too short to fingerprint"),
                "{name} is refused for its length, got {err:?}"
            );
            assert!(
                err.starts_with(&format!("{secs:.1}s")),
                "and the decode reached the end of it, got {err:?}"
            );
        }
    }
}
