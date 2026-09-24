//! The model manager: the catalog, what's installed, and the download.
//!
//! Nothing is bundled: weights are tens of megabytes under their own
//! licences (Essentia's effnet is CC BY-NC-SA, PANNs CC BY). Downloading on
//! the user's click keeps the app small and an NC model legal to offer.
//!
//! Files live in `models/` under [`rox_core::settings::data_dir`], one per
//! model. A download installs only when its size and SHA-256 match the
//! catalog: a truncated file loads as garbage weights nobody can spot, and
//! the hash pins the Hugging Face revision. It writes to `.part` and renames.

use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::mel;

/// The catalog is code, not fetched: the checksum is the security boundary,
/// and one downloaded beside its file isn't.
pub struct Model {
    /// Stable forever once shipped, or every stored vector orphans.
    pub id: &'static str,
    pub label: &'static str,
    pub summary: &'static str,
    pub dim: usize,
    /// None for the built-in extractor.
    pub weights: Option<Weights>,
    /// Some are non-commercial, and the user accepts that by downloading.
    pub licence: &'static str,
    pub source: &'static str,
}

pub struct Weights {
    pub url: &'static str,
    pub file: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

/// From rox-core, where the settings default names it.
pub use rox_core::acoustic::PANNS_CNN10;

/// Settings page order; the no-download model first.
pub const CATALOG: &[Model] = &[
    Model {
        id: crate::MODEL,
        label: "Timbre sketch",
        summary: "Built in, no download. A summary of each track's log-band energy, \
                  spectral shape, and onset rate. Coarse next to a trained network, but it \
                  needs nothing and it runs everywhere",
        dim: crate::DIM,
        weights: None,
        licence: "Part of rox (AGPL-3.0-only)",
        source: "https://github.com/zealsprince/rox",
    },
    Model {
        id: PANNS_CNN10,
        label: "PANNs CNN10",
        summary: "A convolutional network trained on AudioSet to recognize what a sound is. \
                  Its 512-value description of a track is far richer than the built-in \
                  sketch, at the cost of a 24 MB download and a slower analysis pass",
        dim: 512,
        weights: Some(Weights {
            // The safetensors mirror, because the Zenodo .pth files are pre-1.6
            // non-zip pickles candle can't read. Same weights; the checksum pins the file.
            url: "https://huggingface.co/nicofarr/panns_Cnn10/resolve/main/model.safetensors",
            file: "panns-cnn10.safetensors",
            bytes: 25_232_732,
            sha256: "0f1ccbde4f8c3cdf29d2fa4006cd3bcd5583c9afe4ebeb76eea334e75f0a08e3",
        }),
        licence: "Weights CC BY 4.0, code MIT (Kong et al., PANNs)",
        source: "https://github.com/qiuqiangkong/audioset_tagging_cnn",
    },
];

/// PANNs CNN10's spectrogram recipe from its training config: the
/// `pytorch/inference.py` defaults (32000 Hz, window 1024, hop 320, 64 mels,
/// 50-14000 Hz) through `Cnn10.__init__` (hann, center=True, reflect,
/// ref=1.0, amin=1e-10, top_db=None) into torchlibrosa with librosa's Slaney
/// scale and area norm.
///
/// Watch `top_db=None`: torchlibrosa defaults to 80, Cnn10 overrides it, so
/// the log is absolute. [`crate::panns`] checks the derived bank against the
/// file's `melW` tensor on every load.
pub const PANNS_MEL: mel::Config = mel::Config {
    sample_rate: 32_000,
    n_fft: 1024,
    win_length: 1024,
    hop_length: 320,
    n_mels: 64,
    fmin: 50.0,
    fmax: 14_000.0,
    window: mel::WindowKind::Hann,
    center: true,
    power: 2.0,
    scale: mel::Scale::Slaney,
    norm: mel::Norm::Area,
    log: mel::Log::Db {
        floor: 1e-10,
        top_db: None,
    },
};

/// None for a name from a newer build or a hand edit.
pub fn find(id: &str) -> Option<&'static Model> {
    CATALOG.iter().find(|model| model.id == id)
}

/// Always the built-in one, which needs nothing.
pub fn fallback() -> &'static Model {
    &CATALOG[0]
}

pub fn dir() -> PathBuf {
    rox_core::settings::data_dir().join("models")
}

impl Model {
    pub fn path(&self) -> Option<PathBuf> {
        self.weights.as_ref().map(|w| dir().join(w.file))
    }

    /// Length only: hashing 25 MB per settings render is absurd; [`Self::verify`]
    /// catches a wrong file at load.
    pub fn installed(&self) -> bool {
        let Some(weights) = &self.weights else {
            return true;
        };
        let Some(path) = self.path() else {
            return false;
        };
        std::fs::metadata(path).is_ok_and(|meta| meta.len() == weights.bytes)
    }

    /// Zero when not installed.
    pub fn size_on_disk(&self) -> u64 {
        self.path()
            .and_then(|path| std::fs::metadata(path).ok())
            .map(|meta| meta.len())
            .unwrap_or(0)
    }

    /// Run at load.
    pub fn verify(&self) -> Result<(), String> {
        let Some(weights) = &self.weights else {
            return Ok(());
        };
        let path = self.path().ok_or("no data directory")?;
        let file = std::fs::File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let digest = hash_reader(std::io::BufReader::new(file))?;
        if digest == weights.sha256 {
            Ok(())
        } else {
            Err(format!(
                "{} is not the file the catalog describes (sha256 {digest})",
                weights.file
            ))
        }
    }

    /// Stored vectors stay: they're still valid, and a re-download shouldn't
    /// cost a re-analysis.
    pub fn delete(&self) -> Result<(), String> {
        let Some(path) = self.path() else {
            return Ok(());
        };
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }
}

/// Written by the worker, polled by the UI, shaped like `replaygain_job::Progress`.
#[derive(Default)]
pub struct Progress {
    model: Mutex<String>,
    done: AtomicU64,
    total: AtomicU64,
    cancel: AtomicBool,
}

impl Progress {
    /// The total comes off the catalog; see [`Progress::total`].
    pub fn new(model: &Model) -> Self {
        let progress = Progress::default();
        *progress.model.lock().unwrap() = model.id.to_string();
        progress.total.store(
            model.weights.as_ref().map_or(0, |w| w.bytes),
            Ordering::Relaxed,
        );
        progress
    }

    /// The part file goes with it.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn model(&self) -> String {
        self.model.lock().unwrap().clone()
    }

    pub fn done(&self) -> u64 {
        self.done.load(Ordering::Relaxed)
    }

    /// From the catalog, so a lying or missing Content-Length can't move the bar.
    pub fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }

    pub fn fraction(&self) -> f32 {
        let total = self.total();
        if total == 0 {
            return 0.0;
        }
        (self.done() as f32 / total as f32).clamp(0.0, 1.0)
    }

    pub fn stopping(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn keep_going(&self) -> bool {
        !self.cancel.load(Ordering::Relaxed)
    }
}

/// Not [`rox_net::providers::agent`], whose ten-second cap would fail a 24 MB
/// download. Connect and read timeouts instead: stalls give up, slow links finish.
fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .user_agent(concat!(
                "rox/",
                env!("CARGO_PKG_VERSION"),
                " (https://github.com/zealsprince/rox)"
            ))
            .timeout_connect(Duration::from_secs(15))
            .timeout_read(Duration::from_secs(30))
            .build()
    })
}

/// Stream to `<file>.part`, check size and hash as the bytes go by, then
/// rename into place.
pub fn fetch(model: &Model, progress: &Progress) -> Result<(), String> {
    let weights = model.weights.as_ref().ok_or("this model has no weights")?;
    let dir = dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let final_path = dir.join(weights.file);
    let part_path = dir.join(format!("{}.part", weights.file));

    let response = agent()
        .get(weights.url)
        .call()
        .map_err(|e| rox_net::providers::net_reason(&e))?;

    // A wildly different length (an error page, a moved file) fails before
    // anything streams.
    if let Some(claimed) = response
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok())
        && claimed != weights.bytes
    {
        return Err(format!(
            "the server offered {claimed} bytes, the catalog expects {}",
            weights.bytes
        ));
    }

    let outcome = stream(response.into_reader(), &part_path, weights, progress);
    match outcome {
        Ok(()) => {
            // Rename last, so nothing earlier can pass for installed.
            std::fs::rename(&part_path, &final_path)
                .map_err(|e| format!("{}: {e}", final_path.display()))
        }
        Err(reason) => {
            let _ = std::fs::remove_file(&part_path);
            Err(reason)
        }
    }
}

fn stream(
    mut body: impl Read,
    part_path: &std::path::Path,
    weights: &Weights,
    progress: &Progress,
) -> Result<(), String> {
    use std::io::Write;
    let mut part = std::io::BufWriter::new(
        std::fs::File::create(part_path).map_err(|e| format!("{}: {e}", part_path.display()))?,
    );
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut done: u64 = 0;
    loop {
        if !progress.keep_going() {
            return Err("cancelled".into());
        }
        let read = body.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        // Stop at the catalog size, so an endless server can't fill the disk.
        done += read as u64;
        if done > weights.bytes {
            return Err("the download ran past the size the catalog states".into());
        }
        hasher.update(&buffer[..read]);
        part.write_all(&buffer[..read])
            .map_err(|e| format!("{}: {e}", part_path.display()))?;
        progress.done.store(done, Ordering::Relaxed);
    }
    part.flush().map_err(|e| e.to_string())?;

    if done != weights.bytes {
        return Err(format!(
            "the download stopped at {done} of {} bytes",
            weights.bytes
        ));
    }
    let digest = hex(&hasher.finalize());
    if digest != weights.sha256 {
        return Err(format!(
            "the download's checksum is {digest}, not the {} the catalog states",
            weights.sha256
        ));
    }
    Ok(())
}

/// Lowercase hex; [`crate::local_id`] names a local weights file from it.
pub fn hash_file(path: &std::path::Path) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    hash_reader(std::io::BufReader::new(file))
}

fn hash_reader(mut reader: impl Read) -> Result<String, String> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What can be checked offline: unique ids, well-formed hashes, and a
    /// built-in fallback with nothing to fetch.
    #[test]
    fn the_catalog_is_well_formed() {
        let mut seen = std::collections::HashSet::new();
        for model in CATALOG {
            assert!(seen.insert(model.id), "duplicate model id {}", model.id);
            // The pass trusts the stated width.
            assert_eq!(
                model.dim,
                match model.id {
                    crate::MODEL => crate::DIM,
                    PANNS_CNN10 => crate::panns::DIM,
                    other => panic!("{other} has no extractor"),
                }
            );
            assert!(model.dim > 0);
            assert!(!model.licence.is_empty(), "{} states no licence", model.id);
            assert!(model.source.starts_with("https://"));
            if let Some(weights) = &model.weights {
                assert!(weights.url.starts_with("https://"));
                assert_eq!(
                    weights.sha256.len(),
                    64,
                    "{} has a sha256 that isn't 32 bytes of hex",
                    model.id
                );
                assert!(
                    weights
                        .sha256
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                );
                assert!(weights.bytes > 0);
                // A weight file name must not be able to escape the models dir.
                assert!(!weights.file.contains('/') && !weights.file.contains('\\'));
            }
        }
        assert!(find(crate::MODEL).is_some());
        assert!(find(PANNS_CNN10).is_some());
        assert!(find("nothing-like-this").is_none());
        assert!(fallback().weights.is_none());
        assert!(fallback().installed());
    }

    /// The recipe must run, and match what the weights were fit against.
    #[test]
    fn the_panns_recipe_is_the_one_its_training_config_states() {
        assert!(PANNS_MEL.valid().is_ok());
        assert_eq!(PANNS_MEL.sample_rate, 32_000);
        assert_eq!(PANNS_MEL.n_fft, 1024);
        assert_eq!(PANNS_MEL.hop_length, 320);
        assert_eq!(PANNS_MEL.n_mels, 64);
        assert_eq!(PANNS_MEL.fmin, 50.0);
        assert_eq!(PANNS_MEL.fmax, 14_000.0);
        assert_eq!(PANNS_MEL.scale, mel::Scale::Slaney);
        assert_eq!(PANNS_MEL.norm, mel::Norm::Area);
        // Read off the config, so a change to the const fails here.
        let recipe = PANNS_MEL;
        assert!(recipe.center, "reflect-padded, librosa's framing");
        assert_eq!(PANNS_MEL.power, 2.0);
        // torchlibrosa defaults top_db to 80; Cnn10 turns it off.
        assert_eq!(
            PANNS_MEL.log,
            mel::Log::Db {
                floor: 1e-10,
                top_db: None
            }
        );
        assert_eq!(PANNS_MEL.bins(), 513);
    }

    #[test]
    fn hex_is_lowercase_and_padded() {
        assert_eq!(hex(&[0x00, 0x0f, 0xff, 0xa5]), "000fffa5");
    }

    /// Published SHA-256 vectors.
    #[test]
    fn the_hasher_agrees_with_the_published_vectors() {
        assert_eq!(
            hash_reader(&b""[..]).unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hash_reader(&b"abc"[..]).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    /// A truncated download is never renamed into place.
    #[test]
    fn a_short_or_wrong_body_never_becomes_an_installed_model() {
        let dir = std::env::temp_dir().join(format!("rox-models-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let part = dir.join("test.part");
        let weights = Weights {
            url: "https://example.invalid/x",
            file: "test",
            bytes: 3,
            // sha256("abc")
            sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        };
        let progress = Progress::default();

        assert!(stream(&b"abc"[..], &part, &weights, &progress).is_ok());
        assert_eq!(progress.done(), 3);

        let short = stream(&b"ab"[..], &part, &weights, &progress).unwrap_err();
        assert!(short.contains("stopped at 2"), "{short}");

        let wrong = stream(&b"abd"[..], &part, &weights, &progress).unwrap_err();
        assert!(wrong.contains("checksum"), "{wrong}");

        // An endless server is cut off at the stated size.
        let flood = stream(&b"abcdefgh"[..], &part, &weights, &progress).unwrap_err();
        assert!(flood.contains("ran past"), "{flood}");

        progress.cancel.store(true, Ordering::Relaxed);
        assert_eq!(
            stream(&b"abc"[..], &part, &weights, &progress).unwrap_err(),
            "cancelled"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_fraction_is_bounded_even_when_the_counters_are_not() {
        let progress = Progress::default();
        assert_eq!(progress.fraction(), 0.0, "no total, no bar");
        progress.total.store(100, Ordering::Relaxed);
        progress.done.store(25, Ordering::Relaxed);
        assert!((progress.fraction() - 0.25).abs() < 1e-6);
        progress.done.store(400, Ordering::Relaxed);
        assert_eq!(progress.fraction(), 1.0);
    }

    /// The real download. Ignored: it hits the network and writes 24 MB. Run it
    /// by hand (`cargo test -- --ignored fetches_the`) when a catalog entry changes.
    #[test]
    #[ignore = "hits the network and writes into the data folder"]
    fn fetches_the_catalog_entry_it_describes() {
        let model = find(PANNS_CNN10).expect("the entry is in the catalog");
        model.delete().expect("clearing whatever was there");
        assert!(!model.installed());
        let progress = Progress::default();
        fetch(model, &progress).expect("the download lands");
        assert!(model.installed(), "the size matches the catalog");
        model.verify().expect("and so does the checksum");
        assert_eq!(progress.done(), model.weights.as_ref().unwrap().bytes);
    }
}
