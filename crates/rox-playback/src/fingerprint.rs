//! AcoustID fingerprints (ADR 14): the first two minutes of a file turned
//! into the string acoustid.org's lookup takes verbatim.
//!
//! Chromaprint doesn't hash bytes. It folds the audio to 11025 Hz mono, takes
//! a short-time FFT, collapses each frame's spectrum into twelve chroma
//! bands, and quantizes the result into one 32-bit subfingerprint every
//! eighth of a second or so. Two encodes of the same recording at different
//! bitrates land close enough together in that space for AcoustID's index to
//! match them, which is what makes this worth having beside the text-matching
//! providers: it asks what the file sounds like, and that's the question left
//! over when the tags say nothing or say the wrong thing.
//!
//! Pure Rust, so ADR 2's no-C-in-the-decode-path line still holds:
//! `rusty-chromaprint` is a port of the reference library, not a binding to
//! it.
//!
//! ## Why it decodes for itself
//!
//! The same reason [`crate::analysis`] gives, about a different conversion.
//! [`crate::engine`]'s `Source` resamples to the device rate and folds to
//! stereo before anything downstream sees a sample. Chromaprint does its own
//! resample, from the file's rate straight down to 11025 Hz, and its own
//! downmix, a flat average over every channel. Feeding it playback's output
//! would put one lossy conversion in front of another and move the spectrum
//! the chroma bands are built from, on a number whose only job is to land in
//! the same neighbourhood as one fpcalc computed from the file directly. So
//! the probe and the decode loop below are `measure`'s: the file's own rate,
//! the file's own channel count, one packet at a time, nothing held but the
//! window. [`crate::engine::decode_window`] is the other thing not used here,
//! for the second half of that reason and because it buffers a whole span
//! before returning it.
//!
//! ## Algorithm and window
//!
//! [`WINDOW_SECS`] is 120, which is fpcalc's default `-length`
//! (`g_max_duration = 120` in chromaprint's `src/cmd/fpcalc.cpp`) and so the
//! span the fingerprints already in AcoustID's index were taken over. The
//! algorithm is TEST2, chromaprint's `CHROMAPRINT_ALGORITHM_DEFAULT`
//! (`src/chromaprint.h`), which `Configuration::preset_test2` matches down to
//! the id byte the compressed header carries.
//!
//! The wire form is the compressed fingerprint in base64 with the URL-safe
//! alphabet and no padding. Chromaprint's own `chromaprint_encode_fingerprint`
//! runs `CompressFingerprint` then `Base64Encode`, and that encoder's table in
//! `src/utils/base64.h` ends `...0123456789-_` and never emits a pad
//! character, so `URL_SAFE_NO_PAD` is the same encoding rather than a near
//! one.
//!
//! ## What it refuses
//!
//! A file with under about three seconds of decodable audio, where the
//! classifiers' filter window never fills and the fingerprint comes back
//! empty. A container that changes rate or channel count before
//! [`MIN_SECS`] have gone in. Anything the probe or the decoder can't open at
//! all. Each of those is an `Err`, never an empty or partial string, because
//! a lookup on a fingerprint that stands for nothing costs a round trip and
//! reads back as a clean no-match.

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

/// The first `WINDOW_SECS` of a file as an AcoustID fingerprint.
pub const WINDOW_SECS: u32 = 120;

/// How much audio has to be in the window before a mid-file format change is
/// something to stop at rather than something to fail on. AcoustID matches on
/// a span, not on the whole thing, and a quarter of a minute is well past the
/// three seconds the algorithm needs to produce anything at all, so a chained
/// stream that switches format after fifteen seconds still has a real
/// fingerprint in front of the seam.
pub const MIN_SECS: u32 = 15;

/// How much decoded audio goes by between cancel checks, matching
/// [`crate::analysis`]. Decode runs far faster than realtime, so a quarter
/// second of audio is a couple of milliseconds of wall clock.
const TICK_SECS: f64 = 0.25;

/// One file fingerprinted.
#[derive(Debug, Clone)]
pub struct Fingerprint {
    /// Chromaprint's wire encoding: the compressed fingerprint as URL-safe
    /// base64 without padding, the string AcoustID's lookup takes verbatim.
    pub encoded: String,
    /// Whole-file length in seconds as the container reports it. None when
    /// the container doesn't say; the caller substitutes the library's own
    /// duration, since AcoustID needs one.
    pub duration_secs: Option<u32>,
}

/// Fingerprint the head of `path`.
///
/// Blocking, offline, background executor only. `should_continue` is polled
/// every quarter second of decoded audio; false ends the decode and returns
/// `Err("cancelled")`, since a half-read window is not a fingerprint anyone
/// can look up.
///
/// `Err` is also a file that can't be opened, one with nothing decodable in
/// it, one too short for the algorithm, and one that changes format inside
/// the first [`MIN_SECS`]. A decode that falls over partway through keeps
/// what it already fed in, the same call playback makes on a corrupt packet.
pub fn compute(path: &Path, should_continue: impl Fn() -> bool) -> Result<Fingerprint, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    // Guarded for the reason [`crate::analysis::measure`] guards the same two
    // calls: the probe and the decoder build are third-party parsing of file
    // bytes, and a panic in either has to read as a file this can't
    // fingerprint rather than as the end of the worker.
    let mut format = guard_decode("probe", path, || {
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

    // The whole file's length rather than the window's, because that's the
    // number AcoustID matches a fingerprint against. Same ladder `measure`
    // climbs for its progress denominator: the frame count first, the
    // container's declared duration next, and a fragmented MP4's movie header
    // last, since that case reports zero for both of the others. A file under
    // half a second rounds to nothing usable, which reads the same here as a
    // container that never said.
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

    let mut decoder = guard_decode("decoder setup", path, || {
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
        // A panic in the reader or the decoder ends the fingerprint as an
        // error rather than as a dead worker, and it ends it for good: the
        // unwind came out of the middle of that state, so the loop never goes
        // back in for another packet.
        let packet = match guard_decode("packet read", path, || format.next_packet())? {
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

        // The copy into the scratch buffer is inside the guard because the
        // decoded audio is borrowed from the decoder: reading it runs the
        // codec's own code as much as producing it did.
        let decoded = guard_decode("decode", path, || {
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
            // Corrupt or truncated packet: skip it and keep going, the same
            // call playback makes.
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

        // A chained stream or a container switching format mid-file. The
        // meter in `measure` can reconfigure and keep its block history;
        // this can't, because restarting the fingerprinter is what resets
        // it, which would silently throw away everything decoded so far and
        // hand back a fingerprint of the tail. So the window ends at the seam
        // when there's already enough audio behind it to identify the
        // recording, and the file is refused outright when there isn't.
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

        // Chromaprint takes interleaved i16 at the file's own rate. The
        // scale is 32768 with the result clamped into range, which is the
        // conversion ffmpeg hands fpcalc, so the two see the same samples.
        // `pcm` keeps its capacity across packets, so nothing allocates past
        // the first one.
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
        // Under roughly three seconds the classifiers' filter window never
        // fills, so there is no subfingerprint to compress. Encoding the
        // empty case anyway would produce a four-byte header that every short
        // file shares, which looks like a fingerprint and matches nothing.
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

    /// A directory of fixture files that clears itself when the test ends,
    /// the same shape [`crate::analysis`]'s tests use. The path is unique per
    /// call so the suite's threads never share one.
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

        /// A 16-bit PCM wav of the interleaved samples handed in.
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

        /// Bytes with no container in them, for the probe-failure path.
        fn junk(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, vec![0x7fu8; 4096]).expect("writing the fixture");
            path
        }
    }

    /// The checked-in Opus fixtures, whose recipes live in
    /// `tests/fixtures/README.md`.
    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
    }

    /// A stack of partials on both channels, detuned per channel so the two
    /// don't collapse to mid, with the partial set chosen by the caller. A
    /// steady tone is enough for the fingerprinter, which is looking at
    /// chroma rather than at timbre, but a chord puts several bands in play
    /// so two fixtures differ in more than one of the twelve.
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

    /// The round trip in one test: a real decode, a real fingerprint, and the
    /// encoding that goes on the wire. Determinism is the assertion worth
    /// making here, since AcoustID's index is only useful if the same audio
    /// gives the same string every time.
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

    /// Different audio has to give a different string, or the encode is
    /// producing a constant and the determinism test above would still pass.
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

    /// The window is capped, so a file longer than [`WINDOW_SECS`] and the
    /// same file cut to the window fingerprint identically. This is the whole
    /// reason the cap exists: it makes the string comparable with one fpcalc
    /// took over its own default length.
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

    /// Cancelling has to come back as an error rather than as a fingerprint
    /// of whatever got read first, which would be a lookup that quietly
    /// matches the wrong thing.
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

    /// Nothing to decode is an error, not an empty string.
    #[test]
    fn a_file_that_is_not_audio_is_an_error() {
        let fx = Fixtures::new("not-audio");
        assert!(compute(&fx.missing("gone.wav"), || true).is_err());
        assert!(compute(&fx.junk("junk.wav"), || true).is_err());
        assert!(compute(&fx.wav("empty.wav", 48_000, 2, &[]), || true).is_err());
    }

    /// The checked-in Opus fixtures go through the same registry playback
    /// uses, so this covers the real container path end to end. Both are
    /// under three seconds, which is the algorithm's own floor: the
    /// classifiers' filter window never fills, the fingerprint comes back
    /// empty, and the refusal names the length rather than pretending the
    /// decode failed.
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
