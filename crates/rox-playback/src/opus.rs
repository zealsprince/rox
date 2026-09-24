//! Opus decode, which symphonia 0.6 lacks: its Ogg reader maps an Opus stream
//! to a track but nothing claims `CODEC_ID_OPUS`. rox's own converter writes
//! .opus, so this is needed to play its own output. Backed by `opus-pure`, no
//! C (ADR 2), picked by measurement: -89 to -126 dB of error against libopus
//! on real tracks (`tests/fixtures/README.md`).
//!
//! Multistream (mapping family 1 past stereo) is refused, not decoded;
//! `OpusMSDecoder` exists if someone wires it.
//!
//! Gapless (ADR 3): end padding arrives as `trim_end` from the Ogg reader, but
//! its `trim_start` is always zero for Opus, so the pre-skip is read from
//! `OpusHead` and dropped here, keyed off `packet.pts` so a seek can't redo it.
//!
//! Known gap: RFC 7845's 80 ms pre-roll after a seek isn't done, so the first
//! frames after a seek can be slightly off.

use symphonia::core::audio::{
    AsGenericAudioBufferRef, AudioBuffer, AudioMut, AudioSpec, GenericAudioBufferRef,
};
use symphonia::core::codecs::CodecInfo;
use symphonia::core::codecs::audio::well_known::CODEC_ID_OPUS;
use symphonia::core::codecs::audio::{
    AudioCodecParameters, AudioDecoder, AudioDecoderOptions, FinalizeResult,
};
use symphonia::core::codecs::registry::{RegisterableAudioDecoder, SupportedAudioCodec};
use symphonia::core::errors::{Result, decode_error, unsupported_error};
use symphonia::core::packet::PacketRef;
use symphonia::core::support_audio_codec;

/// 120 ms at 48 kHz, the longest packet. Buffers are sized to it at open and never grow.
const MAX_FRAMES_PER_PACKET: usize = 5760;

/// Opus always decodes at 48 kHz; the `OpusHead` rate is only what the encoder was fed.
const OPUS_RATE: u32 = 48_000;

pub struct OpusDecoder {
    params: AudioCodecParameters,
    inner: opus_pure::OpusDecoder,
    /// `None` at 0 dB, so the common case is an exact passthrough.
    gain: Option<f32>,
    pre_skip: u64,
    /// Part of the trait contract; rox never turns gapless off.
    gapless: bool,
    channels: usize,
    buf: AudioBuffer<f32>,
    scratch: Vec<f32>,
}

impl OpusDecoder {
    pub fn try_new(params: &AudioCodecParameters, opts: &AudioDecoderOptions) -> Result<Self> {
        let Some(channels) = params.channels.clone() else {
            return unsupported_error("opus: no channel layout");
        };
        let count = channels.count();
        if count == 0 || count > 2 {
            return unsupported_error("opus: multistream channel layouts");
        }

        let inner = match opus_pure::OpusDecoder::new(OPUS_RATE as i32, count) {
            Ok(inner) => inner,
            Err(_) => return decode_error("opus: decoder rejected the stream parameters"),
        };

        let (pre_skip, gain) = parse_head(params.extra_data.as_deref().unwrap_or(&[]));

        Ok(OpusDecoder {
            params: params.clone(),
            inner,
            gain,
            pre_skip,
            gapless: opts.gapless,
            channels: count,
            buf: AudioBuffer::new(AudioSpec::new(OPUS_RATE, channels), MAX_FRAMES_PER_PACKET),
            scratch: vec![0.0; MAX_FRAMES_PER_PACKET * count],
        })
    }

    fn decode_inner(&mut self, packet: &PacketRef<'_>) -> Result<()> {
        let frames = match self
            .inner
            .decode(packet.data, MAX_FRAMES_PER_PACKET, &mut self.scratch)
        {
            Ok(frames) => frames.min(MAX_FRAMES_PER_PACKET),
            Err(e) => {
                log::debug!("opus: packet decode failed ({e})");
                return decode_error("opus: packet decode failed");
            }
        };

        let Self {
            buf,
            scratch,
            gain,
            channels,
            ..
        } = self;
        buf.clear();
        buf.render_uninit(Some(frames));
        for ch in 0..*channels {
            let Some(plane) = buf.plane_mut(ch) else {
                continue;
            };
            let src = scratch[ch..].iter().step_by(*channels);
            match gain {
                Some(g) => {
                    for (dst, s) in plane.iter_mut().zip(src) {
                        *dst = *s * *g;
                    }
                }
                None => {
                    for (dst, s) in plane.iter_mut().zip(src) {
                        *dst = *s;
                    }
                }
            }
        }

        if self.gapless {
            // Pts counts from zero including the pre-skip, so after a seek this is zero.
            let pts = packet.pts.get().max(0) as u64;
            let skip = self.pre_skip.saturating_sub(pts).min(frames as u64) as usize;
            self.buf.trim(skip, packet.trim_end.get() as usize);
        }

        Ok(())
    }
}

/// Pre-skip (u16 at byte 10) and output gain (Q7.8 dB i16 at byte 16) from
/// `OpusHead`, RFC 7845 5.1. A short header reads as neither.
fn parse_head(head: &[u8]) -> (u64, Option<f32>) {
    let pre_skip = head
        .get(10..12)
        .map(|b| u64::from(u16::from_le_bytes([b[0], b[1]])))
        .unwrap_or(0);
    let q78 = head
        .get(16..18)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .unwrap_or(0);
    let gain = (q78 != 0).then(|| 10f32.powf(f32::from(q78) / 256.0 / 20.0));
    (pre_skip, gain)
}

impl AudioDecoder for OpusDecoder {
    /// Opus packets decode independently, so there's no overlap to reconstruct.
    fn reset(&mut self) {
        // Can't fail with the parameters that built it, but the trait can't report
        // it either, so log rather than swallow.
        if let Err(e) = self.inner.reset_state() {
            log::warn!("opus: decoder reset failed ({e})");
        }
        self.buf.clear();
    }

    fn codec_info(&self) -> &CodecInfo {
        &Self::supported_codecs().first().unwrap().info
    }

    fn codec_params(&self) -> &AudioCodecParameters {
        &self.params
    }

    fn decode_ref(&mut self, packet: &PacketRef<'_>) -> Result<GenericAudioBufferRef<'_>> {
        match self.decode_inner(packet) {
            Ok(()) => Ok(self.buf.as_generic_audio_buffer_ref()),
            Err(e) => {
                self.buf.clear();
                Err(e)
            }
        }
    }

    fn finalize(&mut self) -> FinalizeResult {
        Default::default()
    }

    fn last_decoded(&self) -> GenericAudioBufferRef<'_> {
        self.buf.as_generic_audio_buffer_ref()
    }
}

impl RegisterableAudioDecoder for OpusDecoder {
    fn try_registry_new(
        params: &AudioCodecParameters,
        opts: &AudioDecoderOptions,
    ) -> Result<Box<dyn AudioDecoder>>
    where
        Self: Sized,
    {
        Ok(Box::new(OpusDecoder::try_new(params, opts)?))
    }

    fn supported_codecs() -> &'static [SupportedAudioCodec] {
        // The macro names `symphonia_core`; rox only depends on the facade.
        use symphonia::core as symphonia_core;
        &[support_audio_codec!(CODEC_ID_OPUS, "opus", "Opus")]
    }
}

#[cfg(test)]
mod opus_tests {
    use super::*;
    use symphonia::core::audio::{Channels, Position};

    fn head(channels: u8, pre_skip: u16, gain: i16, family: u8) -> Vec<u8> {
        let mut h = b"OpusHead".to_vec();
        h.push(1);
        h.push(channels);
        h.extend_from_slice(&pre_skip.to_le_bytes());
        h.extend_from_slice(&48_000u32.to_le_bytes());
        h.extend_from_slice(&gain.to_le_bytes());
        h.push(family);
        h
    }

    fn params(channels: Channels, head: &[u8]) -> AudioCodecParameters {
        let mut p = AudioCodecParameters::new();
        p.for_codec(CODEC_ID_OPUS)
            .with_sample_rate(OPUS_RATE)
            .with_channels(channels)
            .with_extra_data(Box::from(head));
        p
    }

    #[test]
    fn the_header_gain_reads_as_a_linear_factor() {
        let (pre_skip, gain) = parse_head(&head(2, 312, 256, 0));
        assert_eq!(pre_skip, 312, "pre-skip off bytes 10..12");
        let gain = gain.expect("+1 dB is a real gain");
        assert!(
            (gain - 1.122_018_5).abs() < 1e-5,
            "+1 dB in Q7.8 is a factor of about 1.122, got {gain}"
        );
    }

    /// 0 dB must not become a multiply by a rounded 1.0.
    #[test]
    fn a_zero_header_gain_is_no_gain_at_all() {
        let (_, gain) = parse_head(&head(2, 312, 0, 0));
        assert_eq!(gain, None, "0 dB is a passthrough, not a factor");
    }

    #[test]
    fn a_truncated_header_reads_as_no_skip_and_no_gain() {
        assert_eq!(parse_head(b"OpusHead"), (0, None));
        assert_eq!(parse_head(&[]), (0, None));
    }

    #[test]
    fn a_multistream_layout_refuses_to_open() {
        let six = Channels::Positioned(
            Position::FRONT_LEFT
                | Position::FRONT_CENTER
                | Position::FRONT_RIGHT
                | Position::REAR_LEFT
                | Position::REAR_RIGHT
                | Position::LFE1,
        );
        let params = params(six, &head(6, 312, 0, 1));
        let err = OpusDecoder::try_new(&params, &AudioDecoderOptions::default())
            .err()
            .expect("six channels is multistream");
        assert!(
            err.to_string().contains("multistream"),
            "the refusal names the reason, got {err}"
        );
    }

    #[test]
    fn a_stereo_stream_opens() {
        let stereo = Channels::Positioned(Position::FRONT_LEFT | Position::FRONT_RIGHT);
        let params = params(stereo, &head(2, 312, 0, 0));
        let dec = OpusDecoder::try_new(&params, &AudioDecoderOptions::default())
            .expect("stereo family 0 is the ordinary case");
        assert_eq!(dec.pre_skip, 312);
        assert_eq!(dec.channels, 2);
    }

    /// One second of 440 Hz; the ffmpeg command is in the fixtures README.
    fn fixture() -> rox_library::locator::Locator {
        named("tone-440.opus")
    }

    fn named(name: &str) -> rox_library::locator::Locator {
        rox_library::locator::Locator::Local(fixtures().join(name))
    }

    fn fixtures() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
    }

    /// 48960 decoded frames: 312 pre-skip, 48000 tone, 648 padding. A clean
    /// second means both trims landed.
    #[test]
    fn the_fixture_decodes_to_exactly_one_second() {
        let audio = crate::engine::decode_window(&fixture(), 0.0, OPUS_RATE, 1_000_000)
            .expect("the fixture decodes");
        let frames = audio.len() / 2;
        assert!(
            frames.abs_diff(48_000) <= 1,
            "pre-skip and end padding both trimmed leaves one second, got {frames} frames"
        );
    }

    /// The anti-collapse regression: a band at sixteen blocks, which overflowed
    /// opus-decoder 0.1.1's `u8` mask (it panics here at packet 75 of 101). A
    /// sine never reaches that path.
    #[test]
    fn the_transient_fixture_decodes_without_panicking() {
        let audio = crate::engine::decode_window(
            &named("dense-transients.opus"),
            0.0,
            OPUS_RATE,
            1_000_000,
        )
        .expect("the transient fixture decodes");
        let frames = audio.len() / 2;
        assert!(
            frames.abs_diff(96_000) <= 1,
            "two seconds at 48 kHz once both trims land, got {frames} frames"
        );
    }

    /// Right length isn't enough for a wrong mask. ffmpeg reads RMS 0.125/0.126,
    /// peak 0.774.
    #[test]
    fn the_transient_fixture_decodes_to_the_level_libopus_reads() {
        let audio = crate::engine::decode_window(
            &named("dense-transients.opus"),
            0.0,
            OPUS_RATE,
            1_000_000,
        )
        .expect("the transient fixture decodes");
        let rms = (audio.iter().map(|s| s * s).sum::<f32>() / audio.len() as f32).sqrt();
        assert!(
            (0.115..0.136).contains(&rms),
            "the level matches what ffmpeg reads out of the same file, RMS {rms}"
        );
        let peak = audio.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(
            (0.70..0.85).contains(&peak),
            "the transients survive the decode, peak {peak}"
        );
    }

    /// ffmpeg's volumedetect reads RMS 0.0624 (lavfi's sine is near -18 dBFS);
    /// this decode reads 0.0626.
    #[test]
    fn the_fixture_decodes_to_a_440_hz_tone() {
        let audio = crate::engine::decode_window(&fixture(), 0.0, OPUS_RATE, 1_000_000)
            .expect("the fixture decodes");
        let left: Vec<f32> = audio.iter().step_by(2).copied().collect();
        let rms = (left.iter().map(|s| s * s).sum::<f32>() / left.len() as f32).sqrt();
        assert!(
            (0.055..0.070).contains(&rms),
            "the level matches what ffmpeg reads out of the same file, RMS {rms}"
        );

        // Skip 10 ms at each end so attack and release don't skew the count.
        let skip = 480;
        let body = &left[skip..left.len() - skip];
        let crossings = body
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        let hz = crossings as f32 / 2.0 / (body.len() as f32 / OPUS_RATE as f32);
        assert!(
            (hz - 440.0).abs() / 440.0 < 0.01,
            "the tone is 440 Hz within 1%, measured {hz}"
        );
    }
}
