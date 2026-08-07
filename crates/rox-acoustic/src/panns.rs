//! PANNs CNN10 in candle: the network behind the `panns-cnn10` model.
//!
//! CNN10 is one of the pretrained audio neural networks from Kong et al.,
//! trained on AudioSet to answer "what is this a recording of" across 527
//! classes. The 512 values before that final classifier are what rox keeps:
//! a description of what a piece of audio sounds like, learned from two
//! million clips, which is a different and much better thing than the
//! hand-rolled sketch the built-in [`crate::MODEL`] produces.
//!
//! ## Why this one
//!
//! It's a plain stack of 3x3 convolutions, batch norms, and average pools,
//! which means every operation it needs already exists in candle-nn and
//! there's no ONNX graph to fight. It's 24 MB, which is a download people
//! will actually accept. And the weights are CC BY 4.0 with MIT code, so
//! nothing about offering it is legally awkward, which is not true of the
//! Essentia music models that would otherwise be the obvious pick.
//!
//! ## The architecture, from `pytorch/models.py`
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
//! `ConvBlock` is conv 3x3 (no bias) -> batch norm -> relu, twice, then the
//! pool. Every dropout in the original is a no-op in eval mode, so the
//! forward pass here is the whole of it.
//!
//! ## The front end
//!
//! The spectrogram recipe lives in [`crate::models::PANNS_MEL`],
//! copied from the model's training config. The weights file also ships the
//! filterbank it was trained with, so [`Cnn10::load`] uses that matrix
//! directly and compares it against the one the config derives. A
//! disagreement means the config is wrong, and it gets logged loudly rather
//! than quietly producing embeddings that look fine.

use std::path::Path;

use candle_core::{DType, Device, Tensor, D};
use candle_nn::{BatchNorm, Conv2d, Conv2dConfig, Linear, Module, ModuleT, VarBuilder};

use crate::mel::Mel;
use crate::models::{Model, PANNS_MEL};
use crate::resample;

/// The width of the vector this produces.
pub const DIM: usize = 512;

/// How far the shipped filterbank may sit from the one the config derives
/// before the two are calling each other liars. The two are computed in
/// different languages at different precisions over the same formula, so
/// they agree to about a part in ten million in practice; this leaves four
/// orders of magnitude of headroom and still catches a wrong mel scale,
/// which moves weights by tens of percent.
const BANK_TOLERANCE: f32 = 1e-3;

/// One `ConvBlock`: two 3x3 convolutions each followed by a batch norm and
/// a relu, then a 2x2 average pool.
struct ConvBlock {
    conv1: Conv2d,
    bn1: BatchNorm,
    conv2: Conv2d,
    bn2: BatchNorm,
}

impl ConvBlock {
    fn load(inputs: usize, outputs: usize, vb: VarBuilder) -> candle_core::Result<Self> {
        // Padding 1 on a 3x3 kernel keeps the time and mel axes the size
        // they came in at, so only the pools change the shape.
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
        // forward_t with false is the eval path: the batch norm uses the
        // running statistics the file carries rather than measuring this
        // batch, which is the whole point of using a pretrained model.
        let xs = self.conv1.forward(xs)?;
        let xs = self.bn1.forward_t(&xs, false)?.relu()?;
        let xs = self.conv2.forward(&xs)?;
        let xs = self.bn2.forward_t(&xs, false)?.relu()?;
        xs.avg_pool2d(2)
    }
}

/// The loaded network, its front end, and the device it runs on.
pub struct Cnn10 {
    mel: Mel,
    bn0: BatchNorm,
    blocks: Vec<ConvBlock>,
    fc1: Linear,
    device: Device,
}

impl Cnn10 {
    /// Load the weights for `model`, checking the file against the
    /// catalog's checksum first.
    ///
    /// Tries Metal where candle was built with it and falls back to CPU,
    /// both when the device won't open and when a probe forward pass over
    /// it fails. The probe is the useful half: a Metal device that opens
    /// and then can't run a convolution would otherwise fail once per track
    /// for a whole library pass.
    pub fn load(model: &Model) -> Result<Self, String> {
        model.verify()?;
        let path = model.path().ok_or("this model has no weights to load")?;
        Self::load_from(&path)
    }

    /// Load whatever safetensors sit at `path`, with no catalog entry and no
    /// checksum behind them. This is the user-supplied route: a bigger CNN10
    /// of their own, or a checkpoint they trained.
    ///
    /// Nothing validates the architecture up front, and nothing needs to.
    /// [`Self::build`] reads named tensors at fixed shapes, so a file that
    /// isn't this network fails there with the name of the tensor it wanted;
    /// and the mel filterbank the file carries is checked against the one the
    /// front end computes, which catches a CNN10 trained at other spectrogram
    /// settings even though every tensor loads.
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
        // Unsafe because mmap can't promise the file won't be rewritten
        // underneath us. Nothing else writes here: a re-download lands on a
        // .part file and renames, which swaps the directory entry rather
        // than the pages this mapping holds.
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[path], DType::F32, &device)
                .map_err(|e| format!("{}: {e}", path.display()))?
        };
        let vb = vb.pp("backbone");

        // The filterbank the model was trained with, straight out of the
        // file. torchlibrosa stores it transposed for its matmul, so the
        // rows here are FFT bins and the columns are mel bands.
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
        // The config is the claim; the shipped bank is the evidence. If they
        // part company, everything downstream is fed a spectrogram the
        // weights were never fit against, and nothing about the output would
        // show it, so say so here where there's still a name to blame.
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

    /// One forward pass over silence, to find out whether this device can
    /// actually run the graph before a library pass depends on it.
    fn probe(&self) -> Result<(), String> {
        let frames = vec![vec![0.0f32; PANNS_MEL.n_mels]; MIN_FRAMES];
        self.forward(&frames).map(|_| ())
    }

    /// Where the forward pass runs, for the log line a pass opens with: a
    /// library that takes an hour on the CPU and ten minutes on the GPU
    /// should say which one it picked.
    pub fn device(&self) -> &'static str {
        if matches!(self.device, Device::Cpu) {
            "the CPU"
        } else {
            "the GPU"
        }
    }

    /// One track's vector: the same windows the built-in extractor samples,
    /// described by the network and averaged.
    ///
    /// Windows are scaled to unit length before the average. The relu at the
    /// end of the network means a loud passage produces larger activations
    /// than a quiet one on the same material, and averaging raw would let
    /// whichever window happened to be loudest write most of the track's
    /// vector. What's being averaged is a direction in the model's space.
    ///
    /// The mean is not rescaled on the way out. The storage layer
    /// standardizes every dimension against the corpus at query time
    /// (`rox_library::embeddings::Stats`), so a track-level magnitude has no
    /// vote in the ranking, and leaving it raw keeps this consistent with
    /// what the built-in extractor writes.
    pub fn extract(&self, path: &Path, duration_ms: u32) -> Result<Vec<f32>, String> {
        let duration = duration_ms as f64 / 1000.0;
        // A track no longer than one window has one window in it, read from
        // the top. Anything longer spreads the probes across the range a
        // window can still start in, the same arithmetic the built-in
        // extractor uses, and the same probe positions, so switching models
        // describes the same parts of the record.
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
            // Band-limited on the way down to the model's rate. The engine's
            // linear resampler would fold everything above 16 kHz back into
            // the band the network reads; see the resample module's header.
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

    /// Embed one clip of audio already at [`PANNS_MEL`]'s sample rate.
    /// A clip too short to survive the four pooling stages comes back as
    /// None rather than being padded into a shape the network would read as
    /// several seconds of silence.
    pub fn embed(&self, samples: &[f32]) -> Result<Option<Vec<f32>>, String> {
        let frames = self.mel.spectrogram(samples);
        if frames.len() < MIN_FRAMES {
            return Ok(None);
        }
        self.forward(&frames).map(Some)
    }

    /// The network proper, over a log-mel spectrogram.
    fn forward(&self, frames: &[Vec<f32>]) -> Result<Vec<f32>, String> {
        self.forward_inner(frames).map_err(|e| e.to_string())
    }

    fn forward_inner(&self, frames: &[Vec<f32>]) -> candle_core::Result<Vec<f32>> {
        let time = frames.len();
        let mels = PANNS_MEL.n_mels;
        let flat: Vec<f32> = frames.iter().flatten().copied().collect();
        // (batch, channel, time, mel), the layout the original takes.
        let xs = Tensor::from_vec(flat, (1, 1, time, mels), &self.device)?;

        // bn0 normalizes per mel band, so the mel axis has to be the channel
        // axis while it runs. The original does exactly this pair of
        // transposes; contiguous() because what follows is a convolution and
        // a transposed view isn't laid out for one.
        let xs = xs.transpose(1, 3)?.contiguous()?;
        let xs = self.bn0.forward_t(&xs, false)?;
        let mut xs = xs.transpose(1, 3)?.contiguous()?;

        for block in &self.blocks {
            xs = block.forward(&xs)?;
        }

        // Fold the mel axis away, then reduce time two ways at once: the
        // loudest moment and the average one. Summing them is what the
        // original does, and it's why a clip with one distinctive event and
        // a clip that sounds like that throughout land near each other.
        let xs = xs.mean(D::Minus1)?;
        let peak = xs.max(D::Minus1)?;
        let average = xs.mean(D::Minus1)?;
        let xs = (peak + average)?;
        let xs = self.fc1.forward(&xs)?.relu()?;
        xs.flatten_all()?.to_vec1::<f32>()
    }
}

/// The fewest spectrogram frames the network can take: four 2x2 pools halve
/// the time axis four times, so anything under sixteen frames pools down to
/// nothing. Sixteen frames is 160 ms at the model's hop, well under any
/// excerpt the pass actually feeds it.
pub const MIN_FRAMES: usize = 16;

#[cfg(test)]
mod tests {
    use super::*;

    /// The pooling arithmetic the whole forward pass depends on: four 2x2
    /// pools take 64 mel bands down to 4 and the time axis down to a
    /// sixteenth, and a clip under [`MIN_FRAMES`] has nothing left.
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

    /// A clip that produces too few frames is refused rather than padded,
    /// so nothing feeds the network a shape it would read as silence.
    /// Checked through the mel front end, since that's what decides.
    #[test]
    fn a_clip_shorter_than_the_pooling_stack_makes_too_few_frames() {
        let mel = Mel::new(PANNS_MEL).unwrap();
        // Centered framing gives 1 + samples/hop frames, so under fifteen
        // hops of audio is under the floor.
        let short = vec![0.0f32; PANNS_MEL.hop_length * 10];
        assert!(mel.spectrogram(&short).len() < MIN_FRAMES);
        let long = vec![0.0f32; PANNS_MEL.hop_length * 40];
        assert!(mel.spectrogram(&long).len() >= MIN_FRAMES);
    }

    /// The tests below need the weights, which are a 24 MB download and so
    /// are not something a `cargo test` can assume. They skip when the model
    /// isn't installed rather than failing, and say so, since a silent skip
    /// is how a test stops being run at all.
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

    /// Ten seconds at the model's rate, from a function of the sample index.
    fn clip(shape: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..PANNS_MEL.sample_rate as usize * 10)
            .map(shape)
            .collect()
    }

    /// The config in the catalog against the filterbank the weights were
    /// actually trained with, which the file carries.
    ///
    /// This is the check that the mel recipe is right rather than merely
    /// plausible. A wrong mel scale (HTK where Slaney was meant) moves
    /// weights by tens of percent, and a missing area normalization moves
    /// them by a factor of ten across the top bands, so either one is orders
    /// of magnitude outside the tolerance below.
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

    /// The whole chain against the network's own semantics: run the AudioSet
    /// classifier the embedding sits behind and check that it recognizes
    /// three sounds it was explicitly trained to name.
    ///
    /// This is the strongest verification available without a PyTorch to
    /// diff against. The mel recipe, the weight layout, the batch norms in
    /// eval mode, the pooling, and the transposes all have to be right at
    /// once, because getting any of them wrong turns the input into
    /// something the classifier has never seen and the predictions into
    /// noise. Class indices are AudioSet's own, from the ontology's
    /// `class_labels_indices.csv`.
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

        // Where a sound ranks among the 527 classes, 0 being the model's
        // first pick. A rank rather than a probability: the absolute numbers
        // depend on the clip, the ordering is what the model was scored on.
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
        // A deterministic hash-noise, so the test is the same run to run.
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

    /// The same audio describes the same way twice, and different audio
    /// differently, which is the floor for the vectors being comparable at
    /// all.
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
        // relu means half of it should be zero and the rest positive; an
        // all-zero vector would mean the forward pass collapsed.
        assert!(a.iter().any(|&v| v > 0.0), "the embedding is all zeros");
        assert_eq!(net.embed(&low).unwrap().unwrap(), a, "not deterministic");

        let b = net.embed(&high).unwrap().unwrap();
        assert_ne!(a, b, "two very different tones described identically");
    }
}
