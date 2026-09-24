//! PANNs CNN10 in candle, the network behind the `panns-cnn10` model (Kong et
//! al., trained on AudioSet's 527 classes). rox keeps the 512 values before
//! the classifier. Chosen because it's plain convolutions candle-nn already
//! has, a 24 MB download, and CC BY 4.0 weights with MIT code.
//!
//! The architecture, from `pytorch/models.py`:
//!
//! ```text
//! log-mel (1, 1, T, 64)
//!   transpose -> BatchNorm2d(64) over the mel axis -> transpose back
//!   ConvBlock(1   -> 64)  avg pool 2x2
//!   ConvBlock(64  -> 128) avg pool 2x2
//!   ConvBlock(128 -> 256) avg pool 2x2
//!   ConvBlock(256 -> 512) avg pool 2x2
//!   mean over the mel axis            -> (1, 512, T/16)
//!   max over time + mean over time    -> (1, 512)
//!   Linear(512, 512) -> relu          -> the embedding
//! ```
//!
//! `ConvBlock` is conv 3x3 (no bias), batch norm, relu, twice, then the pool.
//! Dropout is a no-op in eval mode.
//!
//! The recipe is [`crate::models::PANNS_MEL`]. The weights file ships the
//! filterbank it was trained with, which the load uses and checks against
//! the recipe's, logging loudly on a mismatch.

use std::path::Path;

use candle_core::{D, DType, Device, Tensor};
use candle_nn::{BatchNorm, Conv2d, Conv2dConfig, Linear, Module, ModuleT, VarBuilder};

use crate::mel::Mel;
use crate::models::{Model, PANNS_MEL};
use crate::resample;

pub const DIM: usize = 512;

/// The two banks agree to ~1e-7 in practice; this still catches a wrong mel
/// scale, which moves weights by tens of percent.
const BANK_TOLERANCE: f32 = 1e-3;

struct ConvBlock {
    conv1: Conv2d,
    bn1: BatchNorm,
    conv2: Conv2d,
    bn2: BatchNorm,
}

impl ConvBlock {
    fn load(inputs: usize, outputs: usize, vb: VarBuilder) -> candle_core::Result<Self> {
        // Padding 1 keeps the axes' sizes; only the pools shrink them.
        let conv = Conv2dConfig {
            padding: 1,
            ..Default::default()
        };
        Ok(ConvBlock {
            conv1: candle_nn::conv2d_no_bias(inputs, outputs, 3, conv, vb.pp("conv1"))?,
            bn1: candle_nn::batch_norm(outputs, 1e-5, vb.pp("bn1"))?,
            conv2: candle_nn::conv2d_no_bias(outputs, outputs, 3, conv, vb.pp("conv2"))?,
            bn2: candle_nn::batch_norm(outputs, 1e-5, vb.pp("bn2"))?,
        })
    }

    fn forward(&self, xs: &Tensor) -> candle_core::Result<Tensor> {
        // Eval mode: batch norm uses the file's running statistics.
        let xs = self.conv1.forward(xs)?;
        let xs = self.bn1.forward_t(&xs, false)?.relu()?;
        let xs = self.conv2.forward(&xs)?;
        let xs = self.bn2.forward_t(&xs, false)?.relu()?;
        xs.avg_pool2d(2)
    }
}

pub struct Cnn10 {
    mel: Mel,
    bn0: BatchNorm,
    blocks: Vec<ConvBlock>,
    fc1: Linear,
    device: Device,
}

impl Cnn10 {
    /// Check the catalog checksum, then load. Tries Metal and falls back to the
    /// CPU if the device won't open or a probe forward pass fails, rather than
    /// failing once per track.
    pub fn load(model: &Model) -> Result<Self, String> {
        model.verify()?;
        let path = model.path().ok_or("this model has no weights to load")?;
        Self::load_from(&path)
    }

    /// Load any safetensors at `path`, no checksum: a user's own checkpoint.
    /// [`Self::build`] fails with the tensor name on a different network, and
    /// the stored filterbank check catches other spectrogram settings.
    pub fn load_from(path: &Path) -> Result<Self, String> {
        let mut fell_back = None;
        if candle_core::utils::metal_is_available() {
            match Device::new_metal(0) {
                Ok(device) => match Self::build(path, device) {
                    Ok(net) => match net.probe() {
                        Ok(()) => return Ok(net),
                        Err(e) => fell_back = Some(format!("a probe forward pass failed: {e}")),
                    },
                    Err(e) => fell_back = Some(format!("loading onto it failed: {e}")),
                },
                Err(e) => fell_back = Some(e.to_string()),
            }
        }
        if let Some(why) = fell_back {
            log::warn!("panns: Metal is there but unusable ({why}); running on the CPU");
        }
        Self::build(path, Device::Cpu)
    }

    fn build(path: &Path, device: Device) -> Result<Self, String> {
        // Unsafe because mmap can't promise the file isn't rewritten underneath.
        // Nothing writes it in place: a re-download renames a `.part` file over it.
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[path], DType::F32, &device)
                .map_err(|e| format!("{}: {e}", path.display()))?
        };
        let vb = vb.pp("backbone");

        // Stored transposed for torchlibrosa's matmul: rows are FFT bins.
        let stored = vb
            .get(
                (PANNS_MEL.bins(), PANNS_MEL.n_mels),
                "logmel_extractor.melW",
            )
            .map_err(|e| format!("this file carries no mel filterbank: {e}"))?
            .t()
            .and_then(|t| t.to_vec2::<f32>())
            .map_err(|e| e.to_string())?;
        let mel = Mel::with_bank(PANNS_MEL, stored)?;
        // If the recipe and the shipped bank disagree, every embedding is wrong
        // with no visible sign, so say so here.
        let deviation = mel.bank_deviation();
        if deviation > BANK_TOLERANCE {
            log::error!(
                "panns: the mel recipe in the catalog disagrees with the filterbank the weights \
                 ship by {deviation:.4} of full scale; the embeddings this produces are not \
                 comparable to anything"
            );
        } else {
            log::debug!("panns: the mel recipe matches the shipped filterbank to {deviation:.2e}");
        }

        let bn0 = candle_nn::batch_norm(PANNS_MEL.n_mels, 1e-5, vb.pp("bn0"))
            .map_err(|e| e.to_string())?;
        let widths = [(1usize, 64usize), (64, 128), (128, 256), (256, 512)];
        let blocks = widths
            .iter()
            .enumerate()
            .map(|(i, &(inputs, outputs))| {
                ConvBlock::load(inputs, outputs, vb.pp(format!("conv_block{}", i + 1)))
            })
            .collect::<candle_core::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?;
        let fc1 = candle_nn::linear(DIM, DIM, vb.pp("fc1")).map_err(|e| e.to_string())?;

        Ok(Cnn10 {
            mel,
            bn0,
            blocks,
            fc1,
            device,
        })
    }

    /// Proves the device can run the graph before a pass depends on it.
    fn probe(&self) -> Result<(), String> {
        let frames = vec![vec![0.0f32; PANNS_MEL.n_mels]; MIN_FRAMES];
        self.forward(&frames).map(|_| ())
    }

    /// For the pass's opening log line.
    pub fn device(&self) -> &'static str {
        if matches!(self.device, Device::Cpu) {
            "the CPU"
        } else {
            "the GPU"
        }
    }

    /// One track's vector over the built-in extractor's windows. Each window is
    /// scaled to unit length before averaging, so the loudest window doesn't
    /// dominate. The mean stays unscaled; the query standardizes per dimension.
    pub fn extract(&self, path: &Path, duration_ms: u32) -> Result<Vec<f32>, String> {
        let duration = duration_ms as f64 / 1000.0;
        // The built-in extractor's probe positions, so both models describe the
        // same parts of a record.
        let single = duration <= super::WINDOW_SECS;
        let span = (duration - super::WINDOW_SECS).max(0.0);

        let mut sum = vec![0f64; DIM];
        let mut taken = 0usize;
        let mut last_err = String::new();
        for probe in super::PROBES {
            let decoded =
                rox_playback::analysis::decode_mono(path, span * probe, super::WINDOW_SECS, || {
                    true
                });
            let (rate, mono) = match decoded {
                Ok(decoded) => decoded,
                Err(e) => {
                    last_err = e;
                    continue;
                }
            };
            // Band-limit on the way down; see the resample module.
            let clip = resample::convert(&mono, rate, PANNS_MEL.sample_rate);
            match self.embed(&clip)? {
                Some(vector) => {
                    let scale = vector
                        .iter()
                        .map(|v| (*v as f64).powi(2))
                        .sum::<f64>()
                        .sqrt();
                    if scale <= 0.0 {
                        last_err = "the network described this window as nothing".into();
                        continue;
                    }
                    for (acc, v) in sum.iter_mut().zip(&vector) {
                        *acc += *v as f64 / scale;
                    }
                    taken += 1;
                }
                None => last_err = "window too short to analyze".into(),
            }
            if single {
                break;
            }
        }
        if taken == 0 {
            return Err(if last_err.is_empty() {
                "nothing decodable".into()
            } else {
                last_err
            });
        }
        Ok(sum.iter().map(|v| (v / taken as f64) as f32).collect())
    }

    /// Audio at [`PANNS_MEL`]'s rate. Too short for four pools is None, never
    /// padded into seconds of silence.
    pub fn embed(&self, samples: &[f32]) -> Result<Option<Vec<f32>>, String> {
        let frames = self.mel.spectrogram(samples);
        if frames.len() < MIN_FRAMES {
            return Ok(None);
        }
        self.forward(&frames).map(Some)
    }

    fn forward(&self, frames: &[Vec<f32>]) -> Result<Vec<f32>, String> {
        self.forward_inner(frames).map_err(|e| e.to_string())
    }

    fn forward_inner(&self, frames: &[Vec<f32>]) -> candle_core::Result<Vec<f32>> {
        let time = frames.len();
        let mels = PANNS_MEL.n_mels;
        let flat: Vec<f32> = frames.iter().flatten().copied().collect();
        // (batch, channel, time, mel), as the original.
        let xs = Tensor::from_vec(flat, (1, 1, time, mels), &self.device)?;

        // bn0 normalizes per mel band, so the mel axis is the channel axis while it
        // runs; contiguous() because a convolution follows.
        let xs = xs.transpose(1, 3)?.contiguous()?;
        let xs = self.bn0.forward_t(&xs, false)?;
        let mut xs = xs.transpose(1, 3)?.contiguous()?;

        for block in &self.blocks {
            xs = block.forward(&xs)?;
        }

        // Fold the mels, then sum time's max and mean, as the original does.
        let xs = xs.mean(D::Minus1)?;
        let peak = xs.max(D::Minus1)?;
        let average = xs.mean(D::Minus1)?;
        let xs = (peak + average)?;
        let xs = self.fc1.forward(&xs)?.relu()?;
        xs.flatten_all()?.to_vec1::<f32>()
    }
}

/// Four 2x2 pools: under sixteen frames (160 ms) pools to nothing.
pub const MIN_FRAMES: usize = 16;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_pools_leave_a_quarter_of_the_mel_axis() {
        let mut mels = PANNS_MEL.n_mels;
        for _ in 0..4 {
            mels /= 2;
        }
        assert_eq!(mels, 4, "the mel axis must survive four halvings");
        assert_eq!(MIN_FRAMES, 16);
        let mut frames = MIN_FRAMES;
        for _ in 0..4 {
            frames /= 2;
        }
        assert_eq!(frames, 1, "the shortest allowed clip pools to one frame");
    }

    /// Too few frames is refused, checked at the mel front end.
    #[test]
    fn a_clip_shorter_than_the_pooling_stack_makes_too_few_frames() {
        let mel = Mel::new(PANNS_MEL).unwrap();
        // Centered framing: 1 + samples/hop frames.
        let short = vec![0.0f32; PANNS_MEL.hop_length * 10];
        assert!(mel.spectrogram(&short).len() < MIN_FRAMES);
        let long = vec![0.0f32; PANNS_MEL.hop_length * 40];
        assert!(mel.spectrogram(&long).len() >= MIN_FRAMES);
    }

    /// These need the 24 MB weights, so they skip when not installed, and say
    /// so: a silent skip is how a test stops being run.
    fn installed() -> Option<&'static Model> {
        let model = crate::models::find(crate::models::PANNS_CNN10)?;
        if model.installed() {
            Some(model)
        } else {
            eprintln!(
                "skipping: {} is not installed under {}",
                model.id,
                crate::models::dir().display()
            );
            None
        }
    }

    fn clip(shape: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..PANNS_MEL.sample_rate as usize * 10)
            .map(shape)
            .collect()
    }

    /// The catalog recipe against the file's filterbank. A wrong mel scale or a
    /// missing area norm is orders of magnitude past the tolerance.
    #[test]
    fn the_catalog_recipe_matches_the_filterbank_the_weights_ship() {
        let Some(model) = installed() else { return };
        let net = Cnn10::load(model).expect("the installed weights load");
        let deviation = net.mel.bank_deviation();
        assert!(
            deviation < BANK_TOLERANCE,
            "the derived filterbank is {deviation} of full scale away from the shipped one"
        );
    }

    /// The whole chain against AudioSet's own classifier head, which must name
    /// three sounds it was trained on. Any wrong step turns the predictions to
    /// noise. Class indices from `class_labels_indices.csv`.
    #[test]
    fn the_classifier_head_names_sounds_it_was_trained_to_name() {
        const SINE_WAVE: usize = 501;
        const WHITE_NOISE: usize = 520;
        const SILENCE: usize = 500;

        let Some(model) = installed() else { return };
        let net = Cnn10::load(model).expect("the installed weights load");
        let path = model.path().expect("an installed model has a path");
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[&path], DType::F32, &Device::Cpu)
                .expect("the weights map")
        };
        let head = candle_nn::linear(DIM, 527, vb.pp("backbone").pp("fc_audioset"))
            .expect("the classifier head loads");

        // Rank, not probability: the model was scored on ordering.
        let rank_of = |samples: &[f32], class: usize| -> usize {
            let embedding = net
                .embed(samples)
                .expect("the forward pass runs")
                .expect("ten seconds is long enough");
            let xs = Tensor::from_vec(embedding, (1, DIM), &Device::Cpu).unwrap();
            let logits = head.forward(&xs).unwrap().flatten_all().unwrap();
            let scores = logits.to_vec1::<f32>().unwrap();
            let mine = scores[class];
            scores.iter().filter(|&&score| score > mine).count()
        };

        let rate = PANNS_MEL.sample_rate as f32;
        let sine = clip(|i| (std::f32::consts::TAU * 440.0 * i as f32 / rate).sin() * 0.5);
        let noise = clip(|i| (((i as f32 * 12.9898).sin() * 43758.547).fract() - 0.5) * 0.6);
        let silence = clip(|_| 0.0);

        let sine_rank = rank_of(&sine, SINE_WAVE);
        let noise_rank = rank_of(&noise, WHITE_NOISE);
        let silence_rank = rank_of(&silence, SILENCE);
        assert!(
            sine_rank < 5,
            "a 440 Hz tone put \"Sine wave\" at rank {sine_rank} of 527"
        );
        assert!(
            noise_rank < 5,
            "broadband noise put \"White noise\" at rank {noise_rank} of 527"
        );
        assert!(
            silence_rank < 5,
            "digital silence put \"Silence\" at rank {silence_rank} of 527"
        );
    }

    /// Deterministic and discriminating.
    #[test]
    fn the_embedding_is_stable_and_discriminating() {
        let Some(model) = installed() else { return };
        let net = Cnn10::load(model).expect("the installed weights load");
        let rate = PANNS_MEL.sample_rate as f32;
        let low = clip(|i| (std::f32::consts::TAU * 110.0 * i as f32 / rate).sin() * 0.5);
        let high = clip(|i| (std::f32::consts::TAU * 5000.0 * i as f32 / rate).sin() * 0.5);

        let a = net.embed(&low).unwrap().unwrap();
        assert_eq!(a.len(), DIM);
        assert!(a.iter().all(|v| v.is_finite()));
        // All zeros would mean the forward pass collapsed.
        assert!(a.iter().any(|&v| v > 0.0), "the embedding is all zeros");
        assert_eq!(net.embed(&low).unwrap().unwrap(), a, "not deterministic");

        let b = net.embed(&high).unwrap().unwrap();
        assert_ne!(a, b, "two very different tones described identically");
    }
}
