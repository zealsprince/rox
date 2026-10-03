//! The convolution and spatial audio processor node (ADR 19).
//!
//! Provides zero-latency FIR convolution for Head-Related Impulse Responses (HRIR)
//! including 14-channel HeSuVi WAV files, 4-channel True Stereo WAV files, standard
//! stereo and mono impulse responses, alongside a built-in acoustic crossfeed
//! and stereo width spatializer.
//!
//! Runs on the decode thread in the processing chain immediately following the
//! parametric equalizer. Shared parameters are held in [`ConvolverParams`], which
//! uses atomics and an ArcSwap-style update for live, click-free parameter and
//! impulse response swapping.
//!
//! Follows the ADR 19 bypass rule: when disabled or with no impulse response and
//! neutral spatial settings, samples pass bit-exact and internal filter state stays clear.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use serde::{Deserialize, Serialize};

use crate::chain::Node;
use crate::resample::Resampler;
use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

/// The maximum supported impulse response length in samples (approx 170 ms at 48 kHz).
/// Typical HRIRs and HeSuVi profiles are 512 to 2048 samples (~10 to 45 ms).
pub const MAX_IR_SAMPLES: usize = 8192;

/// Operating mode for 14-channel HeSuVi impulse responses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum ConvolverMode {
    /// Simulates two virtual front stereo speakers placed in front of the listener
    /// using the Front Left (FL) and Front Right (FR) binaural filters.
    #[default]
    VirtualStereo = 0,
    /// Upmixes stereo audio to 7.1 surround sound (FL, FR, FC, SL, SR, BL, BR)
    /// and convolves all 7 surround positions using the full 14-channel HeSuVi HRIR set.
    Surround7_1 = 1,
}

impl ConvolverMode {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => ConvolverMode::Surround7_1,
            _ => ConvolverMode::VirtualStereo,
        }
    }
}

/// Built-in HeSuVi HRIR profiles bundled into the binary for instant out-of-the-box spatialization.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuiltinHesuviProfile {
    #[default]
    None,
    Atmos,
    Gsx,
    Dtshx,
    CmssGame,
    Sbx67,
    Razer,
    Sonic,
    DolbyHeadphone,
}

impl BuiltinHesuviProfile {
    pub fn all() -> &'static [BuiltinHesuviProfile] {
        &[
            BuiltinHesuviProfile::None,
            BuiltinHesuviProfile::Atmos,
            BuiltinHesuviProfile::Gsx,
            BuiltinHesuviProfile::Dtshx,
            BuiltinHesuviProfile::CmssGame,
            BuiltinHesuviProfile::Sbx67,
            BuiltinHesuviProfile::Razer,
            BuiltinHesuviProfile::Sonic,
            BuiltinHesuviProfile::DolbyHeadphone,
        ]
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            BuiltinHesuviProfile::None => "Built-in Crossfeed",
            BuiltinHesuviProfile::Atmos => "Dolby Atmos",
            BuiltinHesuviProfile::Gsx => "Sennheiser GSX",
            BuiltinHesuviProfile::Dtshx => "DTS Headphone:X",
            BuiltinHesuviProfile::CmssGame => "Creative CMSS-3D",
            BuiltinHesuviProfile::Sbx67 => "Sound BlasterX",
            BuiltinHesuviProfile::Razer => "Razer Surround",
            BuiltinHesuviProfile::Sonic => "Windows Sonic",
            BuiltinHesuviProfile::DolbyHeadphone => "Dolby Headphone",
        }
    }

    pub fn load_ir(&self) -> Option<WavIr> {
        let (name, bytes): (&str, &[u8]) = match self {
            BuiltinHesuviProfile::None => return None,
            BuiltinHesuviProfile::Atmos => ("Dolby Atmos", include_bytes!("../hrir/atmos.wav")),
            BuiltinHesuviProfile::Gsx => ("Sennheiser GSX", include_bytes!("../hrir/gsx.wav")),
            BuiltinHesuviProfile::Dtshx => ("DTS Headphone:X", include_bytes!("../hrir/dtshx.wav")),
            BuiltinHesuviProfile::CmssGame => {
                ("Creative CMSS-3D", include_bytes!("../hrir/cmss_game.wav"))
            }
            BuiltinHesuviProfile::Sbx67 => ("Sound BlasterX", include_bytes!("../hrir/sbx67-.wav")),
            BuiltinHesuviProfile::Razer => ("Razer Surround", include_bytes!("../hrir/razer.wav")),
            BuiltinHesuviProfile::Sonic => ("Windows Sonic", include_bytes!("../hrir/sonic-.wav")),
            BuiltinHesuviProfile::DolbyHeadphone => {
                ("Dolby Headphone", include_bytes!("../hrir/dh+.wav"))
            }
        };
        let ir = parse_wav(name, bytes).ok()?;
        Some(WavIr {
            profile: *self,
            ..ir
        })
    }
}

/// Channel layout detected from an impulse response file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IrLayout {
    /// 14-channel HeSuVi HRIR:
    /// - Ch 0: Front Left -> Left ear (FL->L)
    /// - Ch 1: Front Left -> Right ear (FL->R)
    /// - Ch 2: Side Left -> Left ear (SL->L)
    /// - Ch 3: Side Left -> Right ear (SL->R)
    /// - Ch 4: Back Left -> Left ear (BL->L)
    /// - Ch 5: Back Left -> Right ear (BL->R)
    /// - Ch 6: Center -> Left ear (FC->L)
    /// - Ch 7: Front Right -> Right ear (FR->R) [note: right ear first]
    /// - Ch 8: Front Right -> Left ear (FR->L)
    /// - Ch 9: Side Right -> Right ear (SR->R)
    /// - Ch 10: Side Right -> Left ear (SR->L)
    /// - Ch 11: Back Right -> Right ear (BR->R)
    /// - Ch 12: Back Right -> Left ear (BR->L)
    /// - Ch 13: Center -> Right ear (FC->R)
    Hesuvi14,
    /// 4-channel True Stereo:
    /// - Ch 0: Left in -> Left ear (LL)
    /// - Ch 1: Left in -> Right ear (LR)
    /// - Ch 2: Right in -> Left ear (RL)
    /// - Ch 3: Right in -> Right ear (RR)
    TrueStereo4,
    /// Standard 2-channel Stereo:
    /// - Ch 0: Left in -> Left ear
    /// - Ch 1: Right in -> Right ear
    Stereo2,
    /// 1-channel Mono:
    /// - Applied to both channels
    Mono1,
    /// Generic channel count
    Generic(usize),
}

impl IrLayout {
    pub fn from_channels(channels: usize) -> Self {
        match channels {
            14 => IrLayout::Hesuvi14,
            4 => IrLayout::TrueStereo4,
            2 => IrLayout::Stereo2,
            1 => IrLayout::Mono1,
            n => IrLayout::Generic(n),
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            IrLayout::Hesuvi14 => "HeSuVi 14-ch HRIR",
            IrLayout::TrueStereo4 => "True Stereo (4-ch)",
            IrLayout::Stereo2 => "Stereo IR (2-ch)",
            IrLayout::Mono1 => "Mono IR (1-ch)",
            IrLayout::Generic(_) => "Multi-channel IR",
        }
    }
}

/// A parsed impulse response with metadata.
#[derive(Clone, Debug)]
pub struct WavIr {
    pub name: String,
    pub sample_rate: u32,
    pub layout: IrLayout,
    pub channels: Vec<Vec<f32>>,
    /// The bundled profile this was loaded from, `None` for a file the user picked.
    pub profile: BuiltinHesuviProfile,
}

impl WavIr {
    /// Resample this impulse response's channels to the target sample rate if needed.
    ///
    /// Uses the playback [`Resampler`], which is interleaved stereo, so the
    /// channels go through in pairs (an odd last channel is paired with
    /// silence). Each channel comes out `round(len * target / source)` long.
    pub fn resampled_to(&self, target_rate: u32) -> Self {
        if self.sample_rate == target_rate || self.channels.is_empty() {
            return self.clone();
        }
        let mut resampler = Resampler::new(self.sample_rate, target_rate);
        let mut resampled = Vec::with_capacity(self.channels.len());
        let mut pair_in = Vec::new();
        let mut pair_out = Vec::new();
        for pair in self.channels.chunks(2) {
            let (left, right) = (&pair[0], pair.get(1));
            pair_in.clear();
            pair_out.clear();
            for (i, &l) in left.iter().enumerate() {
                pair_in.push(l);
                pair_in.push(right.and_then(|r| r.get(i)).copied().unwrap_or(0.0));
            }
            resampler.process(&pair_in, &mut pair_out);
            // Flush lands on the exact frame count and re-arms for the next pair.
            resampler.flush(&mut pair_out);
            resampled.push(pair_out.iter().step_by(2).copied().collect());
            if right.is_some() {
                resampled.push(pair_out.iter().skip(1).step_by(2).copied().collect());
            }
        }
        WavIr {
            name: self.name.clone(),
            sample_rate: target_rate,
            layout: self.layout,
            channels: resampled,
            profile: self.profile,
        }
    }
}

/// Parse a RIFF/WAVE source into a [`WavIr`] using any seekable reader.
/// Only reads the header and up to [`MAX_IR_SAMPLES`] frames from the data chunk,
/// avoiding reading entire multi-gigabyte audio files into memory.
pub fn parse_wav_reader<R: std::io::Read + std::io::Seek>(
    name: &str,
    mut reader: R,
) -> Result<WavIr, String> {
    let mut header = [0u8; 12];
    reader
        .read_exact(&mut header)
        .map_err(|_| "WAV data too short")?;
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Err("Not a valid RIFF/WAVE file".into());
    }

    let mut channels: Option<u16> = None;
    let mut sample_rate: Option<u32> = None;
    let mut bits_per_sample: Option<u16> = None;
    let mut format_tag: Option<u16> = None;
    let mut sub_format: Option<[u8; 16]> = None;
    let mut data_chunk: Option<(u64, usize)> = None;

    let mut chunk_header = [0u8; 8];
    while reader.read_exact(&mut chunk_header).is_ok() {
        let chunk_id = &chunk_header[0..4];
        let chunk_size = u32::from_le_bytes(chunk_header[4..8].try_into().unwrap()) as usize;
        let pad = (chunk_size % 2) as i64;

        if chunk_id == b"fmt " {
            if chunk_size < 16 {
                return Err("fmt chunk too small".into());
            }
            let mut fmt_buf = vec![0u8; chunk_size];
            reader
                .read_exact(&mut fmt_buf)
                .map_err(|_| "Failed to read fmt chunk")?;
            let fmt = u16::from_le_bytes(fmt_buf[0..2].try_into().unwrap());
            let ch = u16::from_le_bytes(fmt_buf[2..4].try_into().unwrap());
            let sr = u32::from_le_bytes(fmt_buf[4..8].try_into().unwrap());
            let bps = u16::from_le_bytes(fmt_buf[14..16].try_into().unwrap());

            format_tag = Some(fmt);
            channels = Some(ch);
            sample_rate = Some(sr);
            bits_per_sample = Some(bps);

            if fmt == 0xFFFE && chunk_size >= 40 {
                let mut guid = [0u8; 16];
                guid.copy_from_slice(&fmt_buf[24..40]);
                sub_format = Some(guid);
            }
            if pad > 0 {
                reader
                    .seek(std::io::SeekFrom::Current(pad))
                    .map_err(|e| e.to_string())?;
            }
        } else if chunk_id == b"data" {
            let pos = reader.stream_position().map_err(|e| e.to_string())?;
            data_chunk = Some((pos, chunk_size));
            reader
                .seek(std::io::SeekFrom::Current(chunk_size as i64 + pad))
                .map_err(|e| e.to_string())?;
        } else {
            reader
                .seek(std::io::SeekFrom::Current(chunk_size as i64 + pad))
                .map_err(|e| e.to_string())?;
        }
    }

    let (ch_count, rate, bps, fmt) = match (channels, sample_rate, bits_per_sample, format_tag) {
        (Some(c), Some(r), Some(b), Some(f)) if c > 0 && r > 0 => (c as usize, r, b, f),
        _ => return Err("Missing or invalid fmt chunk in WAV".into()),
    };

    let (data_offset, chunk_size) =
        data_chunk.ok_or_else(|| "Missing data chunk in WAV".to_string())?;

    let is_float = match fmt {
        3 => true,
        0xFFFE => sub_format.is_some_and(|g| g[0] == 3 && g[1] == 0),
        _ => false,
    };

    let bytes_per_sample = (bps as usize) / 8;
    if bytes_per_sample == 0 {
        return Err("Invalid zero bits per sample".into());
    }
    let block_align = ch_count * bytes_per_sample;
    let num_frames = chunk_size / block_align;
    if num_frames == 0 {
        return Err("No audio frames found in WAV data".into());
    }

    let frames_to_read = num_frames.min(MAX_IR_SAMPLES);
    let bytes_to_read = frames_to_read * block_align;

    reader
        .seek(std::io::SeekFrom::Start(data_offset))
        .map_err(|e| e.to_string())?;
    let mut pcm_data = vec![0u8; bytes_to_read];
    reader
        .read_exact(&mut pcm_data)
        .map_err(|_| "Failed to read audio frames".to_string())?;

    let mut channel_data = vec![Vec::with_capacity(frames_to_read); ch_count];

    for frame in 0..frames_to_read {
        let frame_offset = frame * block_align;
        for (c, out) in channel_data.iter_mut().enumerate() {
            let sample_offset = frame_offset + c * bytes_per_sample;
            let sample = if is_float {
                match bps {
                    32 => {
                        let bytes: [u8; 4] = pcm_data[sample_offset..sample_offset + 4]
                            .try_into()
                            .unwrap();
                        f32::from_le_bytes(bytes)
                    }
                    64 => {
                        let bytes: [u8; 8] = pcm_data[sample_offset..sample_offset + 8]
                            .try_into()
                            .unwrap();
                        f64::from_le_bytes(bytes) as f32
                    }
                    _ => return Err(format!("Unsupported float bit depth: {bps}")),
                }
            } else {
                match bps {
                    16 => {
                        let bytes: [u8; 2] = pcm_data[sample_offset..sample_offset + 2]
                            .try_into()
                            .unwrap();
                        let v = i16::from_le_bytes(bytes);
                        v as f32 / 32768.0
                    }
                    24 => {
                        let b0 = pcm_data[sample_offset] as i32;
                        let b1 = pcm_data[sample_offset + 1] as i32;
                        let b2 = pcm_data[sample_offset + 2] as i8 as i32; // sign-extended
                        let v = b0 | (b1 << 8) | (b2 << 16);
                        v as f32 / 8388608.0
                    }
                    32 => {
                        let bytes: [u8; 4] = pcm_data[sample_offset..sample_offset + 4]
                            .try_into()
                            .unwrap();
                        let v = i32::from_le_bytes(bytes);
                        v as f32 / 2147483648.0
                    }
                    _ => return Err(format!("Unsupported PCM bit depth: {bps}")),
                }
            };
            out.push(sample);
        }
    }

    // Apply a smooth taper (cosine window) at the end of the loaded IR if it was truncated,
    // to prevent any harsh discontinuities.
    if frames_to_read < num_frames {
        let taper_len = 64.min(frames_to_read);
        let start = frames_to_read - taper_len;
        for ch in &mut channel_data {
            for (i, sample) in ch[start..frames_to_read].iter_mut().enumerate() {
                let phase = (i as f32 / taper_len as f32) * std::f32::consts::FRAC_PI_2;
                *sample *= phase.cos();
            }
        }
    }

    // The IR is kept at its recorded level. Output headroom is worked out when
    // an engine is built, from the routing the current mode actually uses
    // (see `lf_headroom_scale`), because it depends on how channels sum.

    // Expand the 7-channel HeSuVi compact format to the 14-channel layout.
    // A 7-channel file is the first half of the 14-channel order, the left
    // speakers and the centre:
    //   FL→L, FL→R, SL→L, SL→R, BL→L, BL→R, FC→L
    // The right speakers are their mirror images (swap the ears), so the second
    // half of the 14-channel order repeats the first seven in the same order:
    //   FR→R=FL→L, FR→L=FL→R, SR→R=SL→L, SR→L=SL→R, BR→R=BL→L, BR→L=BL→R, FC→R=FC→L
    let (final_channels, final_layout) = if ch_count == 7 {
        let full = channel_data
            .iter()
            .chain(channel_data.iter())
            .cloned()
            .collect();
        (full, IrLayout::Hesuvi14)
    } else {
        (channel_data, IrLayout::from_channels(ch_count))
    };

    Ok(WavIr {
        name: name.to_string(),
        sample_rate: rate,
        layout: final_layout,
        channels: final_channels,
        profile: BuiltinHesuviProfile::None,
    })
}

/// Parse a RIFF/WAVE in-memory byte slice into a [`WavIr`].
pub fn parse_wav(name: &str, data: &[u8]) -> Result<WavIr, String> {
    parse_wav_reader(name, std::io::Cursor::new(data))
}

/// Number of log-spaced frequencies in the headroom probe.
const HEADROOM_PROBES: usize = 16;
/// The band the headroom probe covers, in Hz. HRIRs from the same side sum
/// closest to coherently down here, which is where a loud master peaks.
const HEADROOM_BAND_HZ: (f64, f64) = (20.0, 300.0);

/// The gain (at most 1) that keeps a full-scale low-frequency signal from
/// leaving the engine above 0 dBFS, for the routing `routings` describes.
///
/// For each probe frequency it sums every routing's filter response into each
/// ear, for a hard-left, hard-right and centred (in-phase) full-scale input, and
/// takes the largest result. Measuring the real sum matters: a profile's
/// per-filter peak says little about how much its filters add up to.
///
/// `upmix` says the inputs are the 7 upmixed speaker feeds rather than the two
/// stereo channels. Only the default channel gains are assumed.
fn lf_headroom_scale(
    ir_channels: &[Vec<f32>],
    rate: u32,
    routings: &[ChannelRouting],
    upmix: bool,
) -> f32 {
    let sources = [[1.0f64, 0.0], [0.0, 1.0], [1.0, 1.0]];
    let (lo, hi) = HEADROOM_BAND_HZ;
    let mut peak = 0.0f64;
    for k in 0..HEADROOM_PROBES {
        let t = k as f64 / (HEADROOM_PROBES - 1) as f64;
        let w = std::f64::consts::TAU * lo * (hi / lo).powf(t) / f64::from(rate.max(1));
        let responses: Vec<(f64, f64)> = ir_channels.iter().map(|ir| dft_at(ir, w)).collect();
        for src in sources {
            for ear_is_left in [true, false] {
                let (mut re, mut im) = (0.0f64, 0.0f64);
                for r in routings.iter().filter(|r| r.to_left == ear_is_left) {
                    let feed = if upmix {
                        let row = STEREO_UPMIX[r.in_ch];
                        f64::from(row[0]) * src[0] + f64::from(row[1]) * src[1]
                    } else {
                        src[r.in_ch.min(1)]
                    };
                    let (h_re, h_im) = responses[r.filter_idx];
                    let g = feed * f64::from(r.scale);
                    re += g * h_re;
                    im += g * h_im;
                }
                peak = peak.max(re.hypot(im));
            }
        }
    }
    if peak.is_finite() && peak > 1.0 {
        (1.0 / peak) as f32
    } else {
        1.0
    }
}

/// The impulse response's complex frequency response at angular frequency
/// `w` (radians per sample), by direct summation with a rotating phasor.
fn dft_at(ir: &[f32], w: f64) -> (f64, f64) {
    let (sin, cos) = w.sin_cos();
    let (mut pr, mut pi) = (1.0f64, 0.0f64);
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for &x in ir {
        let x = f64::from(x);
        re += x * pr;
        im -= x * pi;
        (pr, pi) = (pr * cos - pi * sin, pr * sin + pi * cos);
    }
    (re, im)
}

/// Partition block size B for zero-latency hybrid convolution.
pub const PARTITION_LEN: usize = 128;
/// FFT size 2B for Overlap-Save tail convolution.
pub const FFT_LEN: usize = 256;
/// Number of complex frequency bins: FFT_LEN / 2 + 1.
pub const NUM_BINS: usize = 129;

/// Precomputed partitioned impulse response for one filter channel.
#[derive(Clone)]
pub struct PartitionedFilter {
    /// Head impulse response coefficients reversed for forward inner product: length PARTITION_LEN.
    pub head_rev: Vec<f32>,
    /// Precomputed FFT spectra for each tail partition: each has length NUM_BINS.
    pub tail_spectra: Vec<Vec<Complex32>>,
}

impl PartitionedFilter {
    pub fn new(ir: &[f32], r2c: &dyn RealToComplex<f32>) -> Self {
        let b = PARTITION_LEN;
        let head_len = ir.len().min(b);
        let mut head_rev = vec![0.0f32; b];
        for i in 0..head_len {
            head_rev[b - 1 - i] = ir[i];
        }

        let mut tail_spectra = Vec::new();
        if ir.len() > b {
            let tail = &ir[b..];
            let p_tail = tail.len().div_ceil(b);
            let mut time_buf = vec![0.0f32; FFT_LEN];
            let mut complex_buf = vec![Complex32::default(); NUM_BINS];

            for p in 0..p_tail {
                let start = p * b;
                let end = (start + b).min(tail.len());
                time_buf.fill(0.0);
                time_buf[..end - start].copy_from_slice(&tail[start..end]);
                r2c.process(&mut time_buf, &mut complex_buf).unwrap();
                tail_spectra.push(complex_buf.clone());
            }
        }

        PartitionedFilter {
            head_rev,
            tail_spectra,
        }
    }

    /// Scale the whole filter, head and tail, by `gain`.
    fn scale(&mut self, gain: f32) {
        for h in &mut self.head_rev {
            *h *= gain;
        }
        for spectrum in &mut self.tail_spectra {
            for bin in spectrum {
                *bin *= gain;
            }
        }
    }
}

/// An input-to-output channel routing tap:
/// specifies which input channel feeds which filter, routing to Left or Right ear with an optional gain scale.
#[derive(Clone, Copy)]
pub struct ChannelRouting {
    pub in_ch: usize,
    pub filter_idx: usize,
    pub to_left: bool,
    pub scale: f32,
}

/// State for one input channel in the partitioned convolver.
#[derive(Clone)]
pub struct InputChannelState {
    /// The last `PARTITION_LEN` input samples as a double-written ring of
    /// `2 * PARTITION_LEN`: every sample is stored at `pos` and `pos +
    /// PARTITION_LEN`, so the history in order is always one contiguous slice
    /// and a push never shifts anything. `pos` is the block position, which
    /// wraps at the same length.
    pub head_hist: Vec<f32>,
    pub cur_block: Vec<f32>,
    pub prev_block: Vec<f32>,
    pub spectra_history: Vec<Vec<Complex32>>,
}

impl InputChannelState {
    pub fn new(p_tail: usize) -> Self {
        InputChannelState {
            head_hist: vec![0.0; 2 * PARTITION_LEN],
            cur_block: vec![0.0; PARTITION_LEN],
            prev_block: vec![0.0; PARTITION_LEN],
            spectra_history: vec![vec![Complex32::default(); NUM_BINS]; p_tail.max(1)],
        }
    }

    pub fn clear(&mut self) {
        self.head_hist.fill(0.0);
        self.cur_block.fill(0.0);
        self.prev_block.fill(0.0);
        for s in &mut self.spectra_history {
            s.fill(Complex32::default());
        }
    }

    /// Record the sample for block position `pos`.
    #[inline(always)]
    pub fn push_sample(&mut self, x: f32, pos: usize) {
        self.head_hist[pos] = x;
        self.head_hist[pos + PARTITION_LEN] = x;
        self.cur_block[pos] = x;
    }

    /// The last `PARTITION_LEN` samples, oldest first, as of the push at `pos`.
    #[inline(always)]
    pub fn head_history(&self, pos: usize) -> &[f32] {
        &self.head_hist[pos + 1..pos + 1 + PARTITION_LEN]
    }
}

/// Dot product with independent partial sums, so the adds don't form one
/// serial dependency chain and the compiler can vectorize the loop.
#[inline(always)]
fn dot_product(a: &[f32], b: &[f32]) -> f32 {
    const LANES: usize = 8;
    let (a_lanes, a_rest) = a.as_chunks::<LANES>();
    let (b_lanes, b_rest) = b.as_chunks::<LANES>();
    let mut acc = [0.0f32; LANES];
    for (x, y) in a_lanes.iter().zip(b_lanes) {
        for ((s, p), q) in acc.iter_mut().zip(x).zip(y) {
            *s += p * q;
        }
    }
    let rest: f32 = a_rest.iter().zip(b_rest).map(|(p, q)| p * q).sum();
    acc.iter().sum::<f32>() + rest
}

/// A zero-latency partitioned convolution engine (ADR 19).
///
/// Implements Gardner's zero-latency hybrid algorithm (1995) combined with Uniform
/// Partitioned Overlap-Save (UPOLS):
/// - The head of each impulse response (first 128 samples) runs in the time domain
///   using direct SIMD dot products for exact zero algorithmic latency.
/// - The tail of each impulse response runs in the frequency domain using real FFTs of size 256.
/// - All multi-channel contributions accumulate in the frequency domain before the inverse FFT,
///   requiring only TWO inverse FFTs per block total.
pub struct PartitionedConvolver {
    layout: IrLayout,
    mode: ConvolverMode,
    p_tail: usize,
    block_pos: usize,
    r2c: Arc<dyn RealToComplex<f32>>,
    c2r: Arc<dyn ComplexToReal<f32>>,
    filters: Vec<PartitionedFilter>,
    routings: Vec<ChannelRouting>,
    input_channels: Vec<InputChannelState>,
    tail_block_l: Vec<f32>,
    tail_block_r: Vec<f32>,
    fft_scratch_time: Vec<f32>,
    fft_scratch_freq: Vec<Complex32>,
    accum_freq_l: Vec<Complex32>,
    accum_freq_r: Vec<Complex32>,
    ifft_scratch_time: Vec<f32>,
}

impl PartitionedConvolver {
    pub fn new(
        ir: &WavIr,
        mode: ConvolverMode,
        r2c: Arc<dyn RealToComplex<f32>>,
        c2r: Arc<dyn ComplexToReal<f32>>,
    ) -> Self {
        let b = PARTITION_LEN;
        let mut filters: Vec<PartitionedFilter> = ir
            .channels
            .iter()
            .map(|ch| PartitionedFilter::new(ch, r2c.as_ref()))
            .collect();

        let p_tail = filters
            .iter()
            .map(|f| f.tail_spectra.len())
            .max()
            .unwrap_or(0);

        let layout = ir.layout;
        let (num_inputs, routings) = match layout {
            IrLayout::Hesuvi14 if filters.len() >= 14 => match mode {
                ConvolverMode::VirtualStereo => (
                    2,
                    vec![
                        ChannelRouting {
                            in_ch: 0,
                            filter_idx: 0,
                            to_left: true,
                            scale: 1.0,
                        },
                        ChannelRouting {
                            in_ch: 0,
                            filter_idx: 1,
                            to_left: false,
                            scale: 1.0,
                        },
                        ChannelRouting {
                            in_ch: 1,
                            filter_idx: 8,
                            to_left: true,
                            scale: 1.0,
                        },
                        ChannelRouting {
                            in_ch: 1,
                            filter_idx: 7,
                            to_left: false,
                            scale: 1.0,
                        },
                    ],
                ),
                ConvolverMode::Surround7_1 => (
                    7,
                    vec![
                        ChannelRouting {
                            in_ch: 0,
                            filter_idx: 0,
                            to_left: true,
                            scale: 1.0,
                        },
                        ChannelRouting {
                            in_ch: 0,
                            filter_idx: 1,
                            to_left: false,
                            scale: 1.0,
                        },
                        ChannelRouting {
                            in_ch: 1,
                            filter_idx: 8,
                            to_left: true,
                            scale: 1.0,
                        },
                        ChannelRouting {
                            in_ch: 1,
                            filter_idx: 7,
                            to_left: false,
                            scale: 1.0,
                        },
                        ChannelRouting {
                            in_ch: 2,
                            filter_idx: 6,
                            to_left: true,
                            scale: 1.0,
                        },
                        ChannelRouting {
                            in_ch: 2,
                            filter_idx: 13,
                            to_left: false,
                            scale: 1.0,
                        },
                        ChannelRouting {
                            in_ch: 3,
                            filter_idx: 2,
                            to_left: true,
                            scale: 1.0,
                        },
                        ChannelRouting {
                            in_ch: 3,
                            filter_idx: 3,
                            to_left: false,
                            scale: 1.0,
                        },
                        ChannelRouting {
                            in_ch: 4,
                            filter_idx: 10,
                            to_left: true,
                            scale: 1.0,
                        },
                        ChannelRouting {
                            in_ch: 4,
                            filter_idx: 9,
                            to_left: false,
                            scale: 1.0,
                        },
                        ChannelRouting {
                            in_ch: 5,
                            filter_idx: 4,
                            to_left: true,
                            scale: 1.0,
                        },
                        ChannelRouting {
                            in_ch: 5,
                            filter_idx: 5,
                            to_left: false,
                            scale: 1.0,
                        },
                        ChannelRouting {
                            in_ch: 6,
                            filter_idx: 12,
                            to_left: true,
                            scale: 1.0,
                        },
                        ChannelRouting {
                            in_ch: 6,
                            filter_idx: 11,
                            to_left: false,
                            scale: 1.0,
                        },
                    ],
                ),
            },
            IrLayout::TrueStereo4 if filters.len() >= 4 => (
                2,
                vec![
                    ChannelRouting {
                        in_ch: 0,
                        filter_idx: 0,
                        to_left: true,
                        scale: 1.0,
                    },
                    ChannelRouting {
                        in_ch: 0,
                        filter_idx: 1,
                        to_left: false,
                        scale: 1.0,
                    },
                    ChannelRouting {
                        in_ch: 1,
                        filter_idx: 2,
                        to_left: true,
                        scale: 1.0,
                    },
                    ChannelRouting {
                        in_ch: 1,
                        filter_idx: 3,
                        to_left: false,
                        scale: 1.0,
                    },
                ],
            ),
            IrLayout::Stereo2 if filters.len() >= 2 => (
                2,
                vec![
                    ChannelRouting {
                        in_ch: 0,
                        filter_idx: 0,
                        to_left: true,
                        scale: 1.0,
                    },
                    ChannelRouting {
                        in_ch: 1,
                        filter_idx: 1,
                        to_left: false,
                        scale: 1.0,
                    },
                ],
            ),
            IrLayout::Mono1 if !filters.is_empty() => (
                2,
                vec![
                    ChannelRouting {
                        in_ch: 0,
                        filter_idx: 0,
                        to_left: true,
                        scale: 1.0,
                    },
                    ChannelRouting {
                        in_ch: 1,
                        filter_idx: 0,
                        to_left: false,
                        scale: 1.0,
                    },
                ],
            ),
            _ => (
                2,
                vec![
                    ChannelRouting {
                        in_ch: 0,
                        filter_idx: 0,
                        to_left: true,
                        scale: 1.0,
                    },
                    ChannelRouting {
                        in_ch: 1,
                        filter_idx: if filters.len() > 1 { 1 } else { 0 },
                        to_left: false,
                        scale: 1.0,
                    },
                ],
            ),
        };

        // Default settings must not push a loud master past full scale: fold
        // the headroom the routing needs into the filters themselves.
        let upmix = layout == IrLayout::Hesuvi14
            && filters.len() >= 14
            && mode == ConvolverMode::Surround7_1;
        let headroom = lf_headroom_scale(&ir.channels, ir.sample_rate, &routings, upmix);
        if headroom < 1.0 {
            for filter in &mut filters {
                filter.scale(headroom);
            }
        }

        let input_channels = vec![InputChannelState::new(p_tail); num_inputs];

        PartitionedConvolver {
            layout,
            mode,
            p_tail,
            block_pos: 0,
            r2c,
            c2r,
            filters,
            routings,
            input_channels,
            tail_block_l: vec![0.0; b],
            tail_block_r: vec![0.0; b],
            fft_scratch_time: vec![0.0; FFT_LEN],
            fft_scratch_freq: vec![Complex32::default(); NUM_BINS],
            accum_freq_l: vec![Complex32::default(); NUM_BINS],
            accum_freq_r: vec![Complex32::default(); NUM_BINS],
            ifft_scratch_time: vec![0.0; FFT_LEN],
        }
    }

    pub fn clear(&mut self) {
        self.block_pos = 0;
        for ch in &mut self.input_channels {
            ch.clear();
        }
        self.tail_block_l.fill(0.0);
        self.tail_block_r.fill(0.0);
    }

    /// Process one stereo frame (in_l, in_r) through the impulse response convolver with zero latency.
    #[inline(always)]
    pub fn process_frame(&mut self, in_l: f32, in_r: f32, channel_gains: &[f32; 7]) -> (f32, f32) {
        let pos = self.block_pos;

        // Push samples to input channels according to mode, scaled by per-channel gains
        match (self.layout, self.mode) {
            (IrLayout::Hesuvi14, ConvolverMode::Surround7_1) => {
                for (i, (ch, row)) in self
                    .input_channels
                    .iter_mut()
                    .zip(&STEREO_UPMIX)
                    .enumerate()
                {
                    ch.push_sample((row[0] * in_l + row[1] * in_r) * channel_gains[i], pos);
                }
            }
            _ => {
                self.input_channels[0].push_sample(in_l * channel_gains[0], pos);
                self.input_channels[1].push_sample(in_r * channel_gains[1], pos);
            }
        }
        let mut out_l = self.tail_block_l[pos];
        let mut out_r = self.tail_block_r[pos];

        for r in &self.routings {
            let hist = self.input_channels[r.in_ch].head_history(pos);
            let head = &self.filters[r.filter_idx].head_rev;
            let head_val = dot_product(head, hist) * r.scale;
            if r.to_left {
                out_l += head_val;
            } else {
                out_r += head_val;
            }
        }

        // Advance block position; run tail UPOLS block when full
        self.block_pos += 1;
        if self.block_pos == PARTITION_LEN {
            self.block_pos = 0;
            self.process_block_tail();
        }

        (out_l, out_r)
    }

    fn process_block_tail(&mut self) {
        let b = PARTITION_LEN;
        let fft_size = FFT_LEN;
        let p_tail = self.p_tail;
        if p_tail == 0 {
            return;
        }

        // 1. Forward FFT for each input channel
        for in_ch in &mut self.input_channels {
            self.fft_scratch_time[..b].copy_from_slice(&in_ch.prev_block);
            self.fft_scratch_time[b..].copy_from_slice(&in_ch.cur_block);
            in_ch.prev_block.copy_from_slice(&in_ch.cur_block);

            self.r2c
                .process(&mut self.fft_scratch_time, &mut self.fft_scratch_freq)
                .unwrap();

            in_ch.spectra_history.rotate_right(1);
            in_ch.spectra_history[0].copy_from_slice(&self.fft_scratch_freq);
        }

        // 2. Frequency-domain accumulation
        self.accum_freq_l.fill(Complex32::default());
        self.accum_freq_r.fill(Complex32::default());

        for r in &self.routings {
            let filter = &self.filters[r.filter_idx];
            let in_spectra = &self.input_channels[r.in_ch].spectra_history;
            let scale = r.scale;
            let target = if r.to_left {
                &mut self.accum_freq_l
            } else {
                &mut self.accum_freq_r
            };

            // Zip stops at the shorter list: a filter with a short tail only
            // reaches back as far as it has partitions.
            let partitions = in_spectra.iter().zip(&filter.tail_spectra);
            if (scale - 1.0).abs() < 0.001 {
                for (x, h) in partitions {
                    for ((t, x), h) in target.iter_mut().zip(x).zip(h) {
                        *t += x * h;
                    }
                }
            } else {
                for (x, h) in partitions {
                    for ((t, x), h) in target.iter_mut().zip(x).zip(h) {
                        *t += (x * h) * scale;
                    }
                }
            }
        }

        // 3. Exactly TWO inverse FFTs: one for Left, one for Right
        let norm_scale = 1.0 / (fft_size as f32);

        self.c2r
            .process(&mut self.accum_freq_l, &mut self.ifft_scratch_time)
            .unwrap();
        for i in 0..b {
            self.tail_block_l[i] = self.ifft_scratch_time[b + i] * norm_scale;
        }

        self.c2r
            .process(&mut self.accum_freq_r, &mut self.ifft_scratch_time)
            .unwrap();
        for i in 0..b {
            self.tail_block_r[i] = self.ifft_scratch_time[b + i] * norm_scale;
        }
    }
}

/// Natural acoustic crossfeed and stereo width processor.
///
/// Implements frequency-dependent acoustic head shadowing with an interaural time delay (~260 µs)
/// and a gentle low-pass shelf (~700 Hz cutoff) along with Mid/Side stereo width adjustment.
#[derive(Clone)]
pub struct SpatialProcessor {
    delay_l: [f32; 64],
    delay_r: [f32; 64],
    delay_ptr: usize,
    delay_len: usize,
    lpf_l: f32,
    lpf_r: f32,
    lpf_alpha: f32,
}

impl SpatialProcessor {
    pub fn new(rate: u32) -> Self {
        let mut p = SpatialProcessor {
            delay_l: [0.0; 64],
            delay_r: [0.0; 64],
            delay_ptr: 0,
            delay_len: 13,
            lpf_l: 0.0,
            lpf_r: 0.0,
            lpf_alpha: 0.1,
        };
        p.reset(rate);
        p
    }

    pub fn reset(&mut self, rate: u32) {
        self.delay_l.fill(0.0);
        self.delay_r.fill(0.0);
        self.lpf_l = 0.0;
        self.lpf_r = 0.0;
        let rate_f = rate.max(8000) as f32;
        // ~260 microseconds interaural head delay
        self.delay_len = ((0.00026 * rate_f).round() as usize).clamp(1, 63);
        // ~700 Hz low-pass cutoff for head shadow simulation
        let fc = 700.0f32;
        let dt = 1.0 / rate_f;
        let rc = 1.0 / (2.0 * std::f32::consts::PI * fc);
        self.lpf_alpha = (dt / (rc + dt)).clamp(0.01, 0.99);
    }

    /// Process a stereo frame through crossfeed and stereo width.
    #[inline(always)]
    pub fn process_frame(&mut self, l: f32, r: f32, crossfeed: f32, width: f32) -> (f32, f32) {
        let mut out_l = l;
        let mut out_r = r;

        if crossfeed > 0.001 {
            let read_idx = (self.delay_ptr + 64 - self.delay_len) % 64;
            let del_l = self.delay_l[read_idx];
            let del_r = self.delay_r[read_idx];

            self.delay_l[self.delay_ptr] = l;
            self.delay_r[self.delay_ptr] = r;
            self.delay_ptr = (self.delay_ptr + 1) % 64;

            self.lpf_l += self.lpf_alpha * (del_l - self.lpf_l);
            self.lpf_r += self.lpf_alpha * (del_r - self.lpf_r);

            // Crossfeed level up to -4.5 dB
            let cross_gain = crossfeed * 0.45;
            let cross_l = l + cross_gain * self.lpf_r;
            let cross_r = r + cross_gain * self.lpf_l;
            let norm = 1.0 / (1.0 + cross_gain * 0.7);
            out_l = cross_l * norm;
            out_r = cross_r * norm;
        }

        if (width - 1.0).abs() > 0.001 {
            let mid = 0.5 * (out_l + out_r);
            let side = 0.5 * (out_l - out_r) * width;
            out_l = mid + side;
            out_r = mid - side;
        }

        (out_l, out_r)
    }
}

/// The 7 discrete surround speaker points (HeSuVi / 7.1 layout):
/// FL, FR, FC, SL, SR, BL, BR.
pub const SURROUND_POINTS: usize = 7;
pub const POINT_FL: usize = 0;
pub const POINT_FR: usize = 1;
pub const POINT_FC: usize = 2;
pub const POINT_SL: usize = 3;
pub const POINT_SR: usize = 4;
pub const POINT_BL: usize = 5;
pub const POINT_BR: usize = 6;

/// Stereo to 7.1 upmix for [`ConvolverMode::Surround7_1`]: one
/// `[from_left, from_right]` row per speaker, in point order (FL, FR, FC, SL,
/// SR, BL, BR). HeSuVi's own stereo upmix, from its Equalizer APO setup:
/// <https://sourceforge.net/projects/hesuvi/>.
const STEREO_UPMIX: [[f32; 2]; SURROUND_POINTS] = [
    [0.5, 0.0],    // FL
    [0.0, 0.5],    // FR
    [0.2, 0.2],    // FC
    [0.45, -0.25], // SL
    [-0.25, 0.45], // SR
    [0.3, -0.2],   // BL
    [-0.2, 0.3],   // BR
];

/// Shared parameters between the UI and the [`Convolver`] node running on the decode thread.
pub struct ConvolverParams {
    enabled: AtomicBool,
    /// Wet mix factor (0.0 to 1.0), stored as f32 bits.
    wet: AtomicU32,
    /// Output gain in dB (-12.0 to +12.0), stored as f32 bits.
    gain_db: AtomicU32,
    /// Operating mode for HeSuVi profiles (VirtualStereo or Surround7_1).
    mode: AtomicU8,
    /// Stereo width (0.0 = mono, 1.0 = normal, 2.0 = ultra-wide), stored as f32 bits.
    stereo_width: AtomicU32,
    /// Crossfeed intensity (0.0 = off, 1.0 = full), stored as f32 bits.
    crossfeed: AtomicU32,
    /// Volume adjustments in dB for the 7 surround speaker positions:
    /// [FL, FR, FC, SL, SR, BL, BR].
    channel_gains_db: [AtomicU32; 7],
    /// Currently loaded impulse response, if any.
    ir: RwLock<Option<Arc<WavIr>>>,
    /// Bumped by set_ir so the decode thread can cheaply detect changes
    /// without taking the lock on every buffer.
    ir_gen: AtomicU64,
    /// Device rate of the live node, 0 until it has been reset. Off-thread
    /// builds target it.
    rate: AtomicU32,
    r2c: Arc<dyn RealToComplex<f32>>,
    c2r: Arc<dyn ComplexToReal<f32>>,
    /// An engine built off the decode thread, waiting for the node to swap it in.
    prepared: Mutex<Option<PreparedEngine>>,
    /// Set whenever the IR or mode changed since the worker last looked.
    rebuild_wanted: AtomicBool,
    /// True while a build worker thread is alive, so changes coalesce into one.
    worker_running: AtomicBool,
}

/// An engine built for one (IR generation, mode, device rate) combination.
/// The node only takes it if all three still match what it wants.
struct PreparedEngine {
    ir_gen: u64,
    mode: ConvolverMode,
    rate: u32,
    /// `None` when the IR was cleared.
    engine: Option<PartitionedConvolver>,
}

impl ConvolverParams {
    // One argument per persisted setting, mirroring `settings::ConvolverSettings`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        enabled: bool,
        wet: f32,
        gain_db: f32,
        mode: ConvolverMode,
        stereo_width: f32,
        crossfeed: f32,
        channel_gains_db: Option<&[f32]>,
        ir: Option<WavIr>,
    ) -> ConvolverParams {
        let default_gains = [0.0f32; 7];
        let gains = channel_gains_db.unwrap_or(&default_gains);
        let mut planner = RealFftPlanner::<f32>::new();
        ConvolverParams {
            enabled: AtomicBool::new(enabled),
            wet: AtomicU32::new(wet.clamp(0.0, 1.0).to_bits()),
            gain_db: AtomicU32::new(gain_db.clamp(-12.0, 12.0).to_bits()),
            mode: AtomicU8::new(mode as u8),
            stereo_width: AtomicU32::new(stereo_width.clamp(0.0, 2.0).to_bits()),
            crossfeed: AtomicU32::new(crossfeed.clamp(0.0, 1.0).to_bits()),
            channel_gains_db: std::array::from_fn(|i| {
                let g = gains.get(i).copied().unwrap_or(0.0);
                AtomicU32::new(g.clamp(-24.0, 12.0).to_bits())
            }),
            ir: RwLock::new(ir.map(Arc::new)),
            ir_gen: AtomicU64::new(0),
            rate: AtomicU32::new(0),
            r2c: planner.plan_fft_forward(FFT_LEN),
            c2r: planner.plan_fft_inverse(FFT_LEN),
            prepared: Mutex::new(None),
            rebuild_wanted: AtomicBool::new(false),
            worker_running: AtomicBool::new(false),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }
    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }

    pub fn wet(&self) -> f32 {
        f32::from_bits(self.wet.load(Ordering::Relaxed))
    }

    pub fn set_wet(&self, wet: f32) {
        self.wet
            .store(wet.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn gain_db(&self) -> f32 {
        f32::from_bits(self.gain_db.load(Ordering::Relaxed))
    }

    pub fn set_gain_db(&self, db: f32) {
        self.gain_db
            .store(db.clamp(-12.0, 12.0).to_bits(), Ordering::Relaxed);
    }

    pub fn gain_linear(&self) -> f32 {
        let db = self.gain_db();
        if db.abs() < 0.01 {
            1.0
        } else {
            10.0f32.powf(db / 20.0)
        }
    }

    pub fn mode(&self) -> ConvolverMode {
        ConvolverMode::from_u8(self.mode.load(Ordering::Relaxed))
    }

    pub fn set_mode(self: &Arc<Self>, mode: ConvolverMode) {
        self.mode.store(mode as u8, Ordering::SeqCst);
        self.request_build();
    }

    pub fn stereo_width(&self) -> f32 {
        f32::from_bits(self.stereo_width.load(Ordering::Relaxed))
    }

    pub fn set_stereo_width(&self, width: f32) {
        self.stereo_width
            .store(width.clamp(0.0, 2.0).to_bits(), Ordering::Relaxed);
    }

    pub fn crossfeed(&self) -> f32 {
        f32::from_bits(self.crossfeed.load(Ordering::Relaxed))
    }

    pub fn set_crossfeed(&self, crossfeed: f32) {
        self.crossfeed
            .store(crossfeed.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn channel_gain_db(&self, ch: usize) -> f32 {
        if ch < SURROUND_POINTS {
            f32::from_bits(self.channel_gains_db[ch].load(Ordering::Relaxed))
        } else {
            0.0
        }
    }

    pub fn set_channel_gain_db(&self, ch: usize, db: f32) {
        if ch < SURROUND_POINTS {
            self.channel_gains_db[ch].store(db.clamp(-24.0, 12.0).to_bits(), Ordering::Relaxed);
        }
    }

    pub fn channel_gain_linear(&self, ch: usize) -> f32 {
        let db = self.channel_gain_db(ch);
        if db.abs() < 0.01 {
            1.0
        } else {
            10.0f32.powf(db / 20.0)
        }
    }

    pub fn all_channel_gains_linear(&self) -> [f32; SURROUND_POINTS] {
        std::array::from_fn(|i| self.channel_gain_linear(i))
    }

    /// Read the current IR generation counter (cheap atomic load).
    pub fn ir_gen(&self) -> u64 {
        self.ir_gen.load(Ordering::Acquire)
    }

    /// Clone the current IR Arc. Only called when the generation changed.
    pub fn current_ir(&self) -> Option<Arc<WavIr>> {
        self.ir.read().ok()?.clone()
    }

    pub fn set_ir(self: &Arc<Self>, ir: Option<WavIr>) {
        if let Ok(mut lock) = self.ir.write() {
            *lock = ir.map(Arc::new);
        }
        self.ir_gen.fetch_add(1, Ordering::SeqCst);
        self.request_build();
    }

    /// Ask for an engine matching the current IR, mode and device rate to be
    /// built off the decode thread. Requests made while a worker is running
    /// coalesce into one more pass.
    ///
    /// Does nothing until a node has been reset, which records the device
    /// rate: that reset builds synchronously, and it stores the rate before it
    /// reads the generation, so a change racing it is never lost.
    fn request_build(self: &Arc<Self>) {
        if self.rate.load(Ordering::SeqCst) == 0 {
            return;
        }
        self.rebuild_wanted.store(true, Ordering::SeqCst);
        if self.worker_running.swap(true, Ordering::SeqCst) {
            return;
        }
        let params = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("rox-convolver-build".into())
            .spawn(move || params.build_worker());
        if let Err(e) = spawned {
            log::error!("convolver build thread could not start: {e}");
            self.worker_running.store(false, Ordering::SeqCst);
        }
    }

    fn build_worker(self: Arc<Self>) {
        loop {
            while self.rebuild_wanted.swap(false, Ordering::SeqCst) {
                self.build_once();
            }
            self.worker_running.store(false, Ordering::SeqCst);
            // A request between the last check and the store above saw the
            // worker still running and returned; pick it up unless another
            // worker already has.
            if !self.rebuild_wanted.load(Ordering::SeqCst)
                || self.worker_running.swap(true, Ordering::SeqCst)
            {
                return;
            }
        }
    }

    fn build_once(&self) {
        let rate = self.rate.load(Ordering::SeqCst);
        if rate == 0 {
            return;
        }
        let mode = self.mode();
        let ir_gen = self.ir_gen();
        let engine = self
            .current_ir()
            .map(|ir| build_engine(&ir, mode, rate, &self.r2c, &self.c2r));
        let Ok(mut slot) = self.prepared.lock() else {
            return;
        };
        // Something changed while this was building: a newer pass is queued.
        if self.ir_gen() != ir_gen
            || self.mode() != mode
            || self.rate.load(Ordering::SeqCst) != rate
        {
            return;
        }
        *slot = Some(PreparedEngine {
            ir_gen,
            mode,
            rate,
            engine,
        });
    }
}

/// Build an engine for `ir` at the device `rate`. Resamples the IR if its rate
/// differs, and allocates and transforms everything the engine needs, so it
/// belongs on a worker thread or in `reset`, never in `process`.
fn build_engine(
    ir: &WavIr,
    mode: ConvolverMode,
    rate: u32,
    r2c: &Arc<dyn RealToComplex<f32>>,
    c2r: &Arc<dyn ComplexToReal<f32>>,
) -> PartitionedConvolver {
    if ir.sample_rate == rate {
        PartitionedConvolver::new(ir, mode, r2c.clone(), c2r.clone())
    } else {
        PartitionedConvolver::new(&ir.resampled_to(rate), mode, r2c.clone(), c2r.clone())
    }
}

/// The Convolver DSP node, implementing [`Node`] for inclusion in [`crate::chain::Chain`].
pub struct Convolver {
    params: Arc<ConvolverParams>,
    rate: u32,
    engine: Option<PartitionedConvolver>,
    spatial: SpatialProcessor,
    seen_ir_gen: u64,
    seen_mode: ConvolverMode,
}

impl Convolver {
    pub fn new(params: Arc<ConvolverParams>) -> Convolver {
        Convolver {
            params,
            rate: 0,
            engine: None,
            spatial: SpatialProcessor::new(48000),
            seen_ir_gen: u64::MAX,
            seen_mode: ConvolverMode::VirtualStereo,
        }
    }

    /// Swap in the engine a worker built for the generation and mode the node
    /// wants, if one is ready. Never blocks and never builds: until the new
    /// engine lands the old one keeps playing.
    ///
    /// The retired engine is freed here. Freeing is a few hundred small
    /// deallocations, nothing next to the work the swap replaces.
    fn take_prepared(&mut self, ir_gen: u64, mode: ConvolverMode) {
        let Ok(mut slot) = self.params.prepared.try_lock() else {
            return;
        };
        let rate = self.rate;
        let ready = slot.take_if(|p| p.ir_gen == ir_gen && p.mode == mode && p.rate == rate);
        drop(slot);
        if let Some(ready) = ready {
            self.engine = ready.engine;
            self.seen_ir_gen = ir_gen;
            self.seen_mode = mode;
        }
    }
}

impl Node for Convolver {
    /// Allocates: builds the engine for the current IR and mode on the calling
    /// thread. Live changes after this go through the worker instead.
    fn reset(&mut self, rate: u32) {
        self.rate = rate;
        self.params.rate.store(rate, Ordering::SeqCst);
        self.spatial.reset(rate);
        self.seen_ir_gen = self.params.ir_gen();
        self.seen_mode = self.params.mode();
        let (mode, r2c, c2r) = (self.seen_mode, &self.params.r2c, &self.params.c2r);
        self.engine = match self.params.current_ir() {
            Some(ir) if rate != 0 => Some(build_engine(&ir, mode, rate, r2c, c2r)),
            _ => None,
        };
        // Drop a prepared engine that the build above already covers. One for
        // a newer change has a different tag and must survive.
        let (seen_gen, seen_mode) = (self.seen_ir_gen, self.seen_mode);
        if let Ok(mut slot) = self.params.prepared.lock() {
            slot.take_if(|p| p.ir_gen == seen_gen && p.mode == seen_mode);
        }
    }

    fn process(&mut self, buf: &mut [f32]) {
        if self.rate == 0 || !self.params.enabled() {
            if let Some(engine) = &mut self.engine {
                engine.clear();
            }
            return;
        }

        // A changed IR or mode is built on a worker thread. Swap the result in
        // once it is ready, and keep playing the old engine until then.
        let ir_gen_now = self.params.ir_gen();
        let mode = self.params.mode();
        if ir_gen_now != self.seen_ir_gen || mode != self.seen_mode {
            self.take_prepared(ir_gen_now, mode);
        }

        let wet = self.params.wet();
        let gain = self.params.gain_linear();
        let width = self.params.stereo_width();
        let crossfeed = self.params.crossfeed();
        let has_ir = self.engine.is_some();
        let channel_gains = self.params.all_channel_gains_linear();

        let (frames, _) = buf.as_chunks_mut::<2>();
        for chunk in frames {
            let in_l = chunk[0];
            let in_r = chunk[1];

            let (wet_l, wet_r) = match &mut self.engine {
                Some(conv) => conv.process_frame(in_l, in_r, &channel_gains),
                None => self.spatial.process_frame(in_l, in_r, crossfeed, width),
            };

            // Post-convolution stereo width if an IR was used
            let (final_wet_l, final_wet_r) = if has_ir && (width - 1.0).abs() > 0.001 {
                let mid = 0.5 * (wet_l + wet_r);
                let side = 0.5 * (wet_l - wet_r) * width;
                (mid + side, mid - side)
            } else {
                (wet_l, wet_r)
            };

            // Wet/dry mix and output gain
            let out_l = ((1.0 - wet) * in_l + wet * final_wet_l) * gain;
            let out_r = ((1.0 - wet) * in_r + wet * final_wet_r) * gain;

            chunk[0] = out_l;
            chunk[1] = out_r;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::Chain;

    const RATE: u32 = 48000;

    fn make_test_wav(channels: u16, rate: u32, samples_per_channel: usize) -> Vec<u8> {
        let mut buf = Vec::new();
        // RIFF header
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&0u32.to_le_bytes()); // placeholder
        buf.extend_from_slice(b"WAVE");

        // fmt chunk (IEEE Float)
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&18u32.to_le_bytes()); // chunk size
        buf.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
        buf.extend_from_slice(&channels.to_le_bytes());
        buf.extend_from_slice(&rate.to_le_bytes());
        let byte_rate = rate * (channels as u32) * 4;
        buf.extend_from_slice(&byte_rate.to_le_bytes());
        let block_align = channels * 4;
        buf.extend_from_slice(&block_align.to_le_bytes());
        buf.extend_from_slice(&32u16.to_le_bytes()); // bits per sample
        buf.extend_from_slice(&0u16.to_le_bytes()); // cbSize

        // data chunk
        buf.extend_from_slice(b"data");
        let data_size = (samples_per_channel * block_align as usize) as u32;
        buf.extend_from_slice(&data_size.to_le_bytes());

        for frame in 0..samples_per_channel {
            for ch in 0..channels {
                // An impulse at frame 0, then silence
                let val: f32 = if frame == 0 {
                    1.0 / (ch as f32 + 1.0)
                } else {
                    0.0
                };
                buf.extend_from_slice(&val.to_le_bytes());
            }
        }

        // Fill in total RIFF size
        let total_size = (buf.len() - 8) as u32;
        buf[4..8].copy_from_slice(&total_size.to_le_bytes());
        buf
    }

    #[test]
    fn parse_wav_14_channel_hesuvi() {
        let wav_bytes = make_test_wav(14, 48000, 64);
        let ir = parse_wav("test_hesuvi.wav", &wav_bytes).expect("parse failed");
        assert_eq!(ir.layout, IrLayout::Hesuvi14);
        assert_eq!(ir.channels.len(), 14);
        assert_eq!(ir.sample_rate, 48000);
        assert_eq!(ir.channels[0].len(), 64);
    }

    #[test]
    fn parse_wav_true_stereo_4ch() {
        let wav_bytes = make_test_wav(4, 44100, 32);
        let ir = parse_wav("true_stereo.wav", &wav_bytes).expect("parse failed");
        assert_eq!(ir.layout, IrLayout::TrueStereo4);
        assert_eq!(ir.channels.len(), 4);
        assert_eq!(ir.sample_rate, 44100);
    }

    #[test]
    fn disabled_convolver_is_bit_exact_passthrough() {
        let params = Arc::new(ConvolverParams::new(
            false,
            1.0,
            0.0,
            ConvolverMode::VirtualStereo,
            1.0,
            0.0,
            None,
            None,
        ));
        let mut chain = Chain::new();
        chain.push(Box::new(Convolver::new(params)));
        chain.reset(RATE);

        let original = vec![0.123f32, -0.456, 0.789, -0.987, 0.0, 1.0];
        let mut buf = original.clone();
        chain.process(&mut buf);
        assert_eq!(buf, original);
    }

    #[test]
    fn impulse_response_convolution_yields_exact_coefficients() {
        let wav_bytes = make_test_wav(2, RATE, 16);
        let ir = parse_wav("stereo.wav", &wav_bytes).unwrap();
        let params = Arc::new(ConvolverParams::new(
            true,
            1.0,
            0.0,
            ConvolverMode::VirtualStereo,
            1.0,
            0.0,
            None,
            Some(ir.clone()),
        ));

        let mut node = Convolver::new(params);
        node.reset(RATE);

        // Input an impulse in L and R: [1.0, 1.0, 0.0, 0.0, 0.0, 0.0, ...]
        let mut buf = vec![0.0f32; 32];
        buf[0] = 1.0;
        buf[1] = 1.0;
        node.process(&mut buf);

        // The first sample output should match the first coefficient of the IR
        assert!((buf[0] - ir.channels[0][0]).abs() < 1e-5);
        assert!((buf[1] - ir.channels[1][0]).abs() < 1e-5);
    }

    #[test]
    fn spatial_crossfeed_and_width_alters_soundstage() {
        let params = Arc::new(ConvolverParams::new(
            true,
            1.0,
            0.0,
            ConvolverMode::VirtualStereo,
            1.5, // 150% stereo width
            0.8, // 80% crossfeed
            None,
            None,
        ));
        let mut node = Convolver::new(params);
        node.reset(RATE);

        // Hard-panned left signal
        let mut buf = vec![0.0f32; 64];
        for i in (0..buf.len()).step_by(2) {
            buf[i] = 0.5;
            buf[i + 1] = 0.0;
        }
        node.process(&mut buf);

        // Crossfeed should have introduced signal into the right channel
        let right_energy: f32 = buf.iter().skip(1).step_by(2).map(|s| s.abs()).sum();
        assert!(
            right_energy > 0.01,
            "Crossfeed must leak signal to right ear"
        );
    }

    #[test]
    fn hesuvi_surround_upmix_convolves_all_channels() {
        let wav_bytes = make_test_wav(14, RATE, 32);
        let ir = parse_wav("hesuvi.wav", &wav_bytes).unwrap();
        let params = Arc::new(ConvolverParams::new(
            true,
            1.0,
            0.0,
            ConvolverMode::Surround7_1,
            1.0,
            0.0,
            None,
            Some(ir),
        ));
        let mut node = Convolver::new(params);
        node.reset(RATE);

        let mut buf = vec![0.0f32; 64];
        buf[0] = 0.8;
        buf[1] = 0.6;
        node.process(&mut buf);

        assert!(buf[0].abs() > 0.001);
        assert!(buf[1].abs() > 0.001);
    }

    fn make_test_pcm_wav(channels: u16, rate: u32, bits: u16, samples: usize) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(b"WAVE");

        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
        buf.extend_from_slice(&channels.to_le_bytes());
        buf.extend_from_slice(&rate.to_le_bytes());
        let bytes_per_sample = (bits / 8) as u32;
        let block_align = channels * (bits / 8);
        let byte_rate = rate * (channels as u32) * bytes_per_sample;
        buf.extend_from_slice(&byte_rate.to_le_bytes());
        buf.extend_from_slice(&block_align.to_le_bytes());
        buf.extend_from_slice(&bits.to_le_bytes());

        buf.extend_from_slice(b"data");
        let data_size = (samples * block_align as usize) as u32;
        buf.extend_from_slice(&data_size.to_le_bytes());

        for frame in 0..samples {
            for ch in 0..channels {
                let sample_val = if frame == 0 && ch == 0 {
                    0.5f32
                } else {
                    0.0f32
                };
                match bits {
                    16 => {
                        let v = (sample_val * 32767.0) as i16;
                        buf.extend_from_slice(&v.to_le_bytes());
                    }
                    24 => {
                        let v = (sample_val * 8388607.0) as i32;
                        buf.push((v & 0xFF) as u8);
                        buf.push(((v >> 8) & 0xFF) as u8);
                        buf.push(((v >> 16) & 0xFF) as u8);
                    }
                    32 => {
                        let v = (sample_val * 2147483647.0) as i32;
                        buf.extend_from_slice(&v.to_le_bytes());
                    }
                    _ => panic!("unsupported bits"),
                }
            }
        }
        let total_size = (buf.len() - 8) as u32;
        buf[4..8].copy_from_slice(&total_size.to_le_bytes());
        buf
    }

    #[test]
    fn parse_wav_16bit_pcm() {
        let wav = make_test_pcm_wav(2, 44100, 16, 10);
        let ir = parse_wav("pcm16.wav", &wav).expect("16-bit PCM WAV failed to parse");
        assert_eq!(ir.layout, IrLayout::Stereo2);
        assert_eq!(ir.sample_rate, 44100);
        assert_eq!(ir.channels.len(), 2);
        assert!((ir.channels[0][0] - 0.5).abs() < 0.05);
    }

    #[test]
    fn parse_wav_24bit_pcm() {
        let wav = make_test_pcm_wav(2, 48000, 24, 10);
        let ir = parse_wav("pcm24.wav", &wav).expect("24-bit PCM WAV failed to parse");
        assert_eq!(ir.layout, IrLayout::Stereo2);
        assert_eq!(ir.sample_rate, 48000);
        assert!((ir.channels[0][0] - 0.5).abs() < 0.05);
    }

    #[test]
    fn ir_resampling_converts_rates() {
        let wav = make_test_pcm_wav(2, 44100, 16, 44);
        let ir = parse_wav("resample.wav", &wav).unwrap();
        let resampled = ir.resampled_to(48000);
        assert_eq!(resampled.sample_rate, 48000);
        assert_eq!(resampled.channels.len(), 2);
        let expected_len = (44.0 * 48000.0 / 44100.0f64).round() as usize;
        assert_eq!(resampled.channels[0].len(), expected_len);
    }

    #[test]
    fn wet_dry_mix_and_gain_scaling() {
        let params = Arc::new(ConvolverParams::new(
            true,
            0.5, // 50% wet
            6.0, // +6 dB gain (~2.0 linear)
            ConvolverMode::VirtualStereo,
            1.0,
            0.0,
            None,
            None,
        ));
        let mut node = Convolver::new(params);
        node.reset(RATE);

        let mut buf = vec![0.5f32; 8];
        node.process(&mut buf);
        // Gain ~ 1.995 * 0.5 ~ 1.0
        assert!((buf[0] - 1.0).abs() < 0.05);
    }

    #[test]
    fn test_all_builtin_profiles_load_and_convolve() {
        for &profile in BuiltinHesuviProfile::all() {
            if profile == BuiltinHesuviProfile::None {
                assert!(profile.load_ir().is_none());
                continue;
            }
            let ir = profile
                .load_ir()
                .unwrap_or_else(|| panic!("Failed to load profile {:?}", profile));
            assert_eq!(ir.layout, IrLayout::Hesuvi14);
            assert_eq!(ir.channels.len(), 14);
            assert_eq!(ir.sample_rate, 48000);
            assert_eq!(ir.profile, profile);
            assert_eq!(ir.resampled_to(44100).profile, profile);

            let params = Arc::new(ConvolverParams::new(
                true,
                1.0,
                0.0,
                ConvolverMode::VirtualStereo,
                1.0,
                0.0,
                None,
                Some(ir),
            ));
            let mut node = Convolver::new(params);
            node.reset(48000);
            let mut buf = vec![0.5f32; 32];
            node.process(&mut buf);
            assert!(buf[0].is_finite());
        }
    }

    #[test]
    fn test_surround_individual_channel_volume_scaling() {
        let wav_bytes = make_test_wav(14, RATE, 64);
        let ir = parse_wav("hesuvi.wav", &wav_bytes).unwrap();
        // Center channel boosted +6 dB, side channels muted (-24 dB)
        let mut gains = [0.0f32; 7];
        gains[POINT_FC] = 6.0;
        gains[POINT_SL] = -24.0;
        gains[POINT_SR] = -24.0;

        let params = Arc::new(ConvolverParams::new(
            true,
            1.0,
            0.0,
            ConvolverMode::Surround7_1,
            1.0,
            0.0,
            Some(&gains),
            Some(ir),
        ));

        assert_eq!(params.channel_gain_db(POINT_FC), 6.0);
        assert_eq!(params.channel_gain_db(POINT_SL), -24.0);
        assert!((params.channel_gain_linear(POINT_FC) - 1.995).abs() < 0.05);

        let mut node = Convolver::new(params.clone());
        node.reset(RATE);

        let mut buf = vec![0.5f32; 64];
        node.process(&mut buf);
        assert!(buf[0].is_finite());

        // Changing live gain is atomic
        params.set_channel_gain_db(POINT_FC, -12.0);
        assert_eq!(params.channel_gain_db(POINT_FC), -12.0);
    }
}
