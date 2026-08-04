//! The model manager: what acoustic models exist, which are installed, and
//! the download that installs one.
//!
//! ## Why nothing is bundled
//!
//! Network weights are tens of megabytes and they carry their own licences,
//! which are frequently not licences you can ship under. The two best music
//! embedding models available are a case in point: Essentia's discogs-effnet
//! family is CC BY-NC-SA, so it can't ride along in a release at all, while
//! PANNs is CC BY, which can, at the cost of most of the installer. Putting
//! the download behind a button the user presses sidesteps both: the app
//! stays small, an NC-licensed model stays legal to offer because the user
//! is the one fetching it for their own use, and someone who never wants
//! acoustic similarity never pays for it.
//!
//! ## Where files land
//!
//! `models/` inside [`crate::settings::data_dir`], beside `library.db`, so a
//! portable install carries its models with it and a wiped data folder takes
//! them with everything else. One file per model, named by the catalog.
//!
//! ## Verification
//!
//! Every catalog entry states its size and SHA-256, and a download is not
//! installed until both match. This is not about a hostile network so much
//! as a truncated one: a download cut off at 90% is a perfectly well formed
//! file that loads as garbage weights and produces embeddings nobody can
//! tell are wrong. The hash is also what pins the catalog to a specific
//! revision of a Hugging Face repo, which can be force-pushed underneath us.
//!
//! The download writes to a `.part` file and renames on success, so an
//! interrupted download can never be mistaken for an installed model.

use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use gpui::{App, Global};
use sha2::{Digest, Sha256};

use crate::embeddings::mel;

/// One model rox can analyze with. Static data: the catalog is code, not a
/// file the app fetches, because the checksum is the security boundary and
/// a checksum the app downloads alongside the thing it's checking isn't one.
pub struct Model {
    /// The name written into the embeddings table. Stable forever once
    /// shipped: change it and every stored vector orphans.
    pub id: &'static str,
    pub label: &'static str,
    /// One line for the settings row, in plain language.
    pub summary: &'static str,
    /// The vector width this model produces.
    pub dim: usize,
    /// What has to be fetched before it can run. None for the built-in
    /// extractor, which is code rather than weights.
    pub weights: Option<Weights>,
    /// The licence the weights are under, stated because some of these are
    /// non-commercial and the user is the one accepting that by downloading.
    pub licence: &'static str,
    /// Where the model came from, for the settings row's link.
    pub source: &'static str,
}

/// The file behind a model, and what makes it that file rather than
/// something else that arrived at the same URL.
pub struct Weights {
    pub url: &'static str,
    /// The name it takes inside `models/`.
    pub file: &'static str,
    pub bytes: u64,
    /// Lowercase hex SHA-256 of the file at `bytes` length.
    pub sha256: &'static str,
}

/// PANNs CNN10's name. The built-in extractor's is [`super::MODEL`], which
/// stays where it is: it names the vectors already in people's libraries.
pub const PANNS_CNN10: &str = "panns-cnn10";

/// Every model the app knows about. Order is the order the settings page
/// lists them, so the one that works without a download comes first.
pub const CATALOG: &[Model] = &[
    Model {
        id: super::MODEL,
        label: "Timbre sketch",
        summary: "Built in, no download. A summary of each track's log-band energy, \
                  spectral shape, and onset rate. Coarse next to a trained network, but it \
                  needs nothing and it runs everywhere",
        dim: super::DIM,
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
            // The safetensors mirror rather than the original Zenodo
            // checkpoint, and not for convenience: the Zenodo .pth files are
            // pre-1.6 PyTorch pickles, the flat non-zip format, and candle's
            // pickle reader only opens the zip flavour. This mirror is the
            // same weights re-saved, and the checksum below pins the exact
            // file rather than trusting the repo to stay put.
            url: "https://huggingface.co/nicofarr/panns_Cnn10/resolve/main/model.safetensors",
            file: "panns-cnn10.safetensors",
            bytes: 25_232_732,
            sha256: "0f1ccbde4f8c3cdf29d2fa4006cd3bcd5583c9afe4ebeb76eea334e75f0a08e3",
        }),
        licence: "Weights CC BY 4.0, code MIT (Kong et al., PANNs)",
        source: "https://github.com/qiuqiangkong/audioset_tagging_cnn",
    },
];

/// PANNs CNN10's spectrogram recipe, taken from the model's own training
/// config rather than guessed.
///
/// The chain is `pytorch/inference.py`'s argparse defaults (sample_rate
/// 32000, window_size 1024, hop_size 320, mel_bins 64, fmin 50, fmax 14000)
/// feeding `Cnn10.__init__`, which pins window='hann', center=True,
/// pad_mode='reflect', ref=1.0, amin=1e-10, top_db=None, into torchlibrosa's
/// `Spectrogram` (power 2.0) and `LogmelFilterBank` (a plain
/// `librosa.filters.mel` with no htk or norm arguments, so Slaney scale and
/// Slaney area normalization, librosa's defaults).
///
/// `top_db=None` is the one that would be easiest to get wrong, because
/// torchlibrosa's own default is 80 and Cnn10 overrides it. With no ceiling
/// and ref=1.0 the whole log step is `10 * log10(max(x, 1e-10))`, which is
/// absolute rather than relative to the clip's own peak.
///
/// This isn't taken on trust. The weights file ships the filterbank it was
/// trained with as a [513, 64] `melW` tensor, and [`crate::embeddings::panns`]
/// checks the bank built from this config against that tensor every time the
/// model loads. If the numbers here were wrong, that check would say so.
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

/// The model with this id, or None for a name from a newer build or a
/// hand-edited settings file.
pub fn find(id: &str) -> Option<&'static Model> {
    CATALOG.iter().find(|model| model.id == id)
}

/// The model the app falls back to when the selected one is unknown or its
/// weights are missing. Always the built-in one, which needs nothing.
pub fn fallback() -> &'static Model {
    &CATALOG[0]
}

/// Where the weight files live.
pub fn dir() -> PathBuf {
    crate::settings::data_dir().join("models")
}

impl Model {
    /// This model's file, or None when it has no weights to install.
    pub fn path(&self) -> Option<PathBuf> {
        self.weights.as_ref().map(|w| dir().join(w.file))
    }

    /// Whether the weights are there and the right length. Length only:
    /// hashing 25 MB on every settings render would be absurd, and a file of
    /// exactly the right size that is nonetheless wrong gets caught by
    /// [`Self::verify`] when the model loads.
    pub fn installed(&self) -> bool {
        let Some(weights) = &self.weights else {
            // Nothing to install means always installed.
            return true;
        };
        let Some(path) = self.path() else {
            return false;
        };
        std::fs::metadata(path).is_ok_and(|meta| meta.len() == weights.bytes)
    }

    /// What the installed file weighs, for the settings readout. Zero when
    /// nothing is installed.
    pub fn size_on_disk(&self) -> u64 {
        self.path()
            .and_then(|path| std::fs::metadata(path).ok())
            .map(|meta| meta.len())
            .unwrap_or(0)
    }

    /// Hash the installed file and check it against the catalog. Run when
    /// the model loads, not per frame.
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

    /// Remove the installed weights. Leaves whatever the model already
    /// embedded in the database alone: the vectors are still valid, and
    /// deleting a file the user can re-download shouldn't cost them a full
    /// re-analysis of their library.
    pub fn delete(&self) -> Result<(), String> {
        let Some(path) = self.path() else {
            return Ok(());
        };
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            // Already gone is the state the caller asked for.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }
}

/// Live progress of a download: the worker writes it, the UI polls it.
/// Shaped after `replaygain_job::Progress` for the same reason, which is
/// that the settings window already knows how to sample one of these.
#[derive(Default)]
pub struct Progress {
    /// Which model is coming down, so a UI can tell whose row to light up.
    model: Mutex<String>,
    done: AtomicU64,
    total: AtomicU64,
    /// Raised by [`stop`] and by app quit.
    cancel: AtomicBool,
}

impl Progress {
    pub fn model(&self) -> String {
        self.model.lock().unwrap().clone()
    }

    pub fn done(&self) -> u64 {
        self.done.load(Ordering::Relaxed)
    }

    /// Bytes expected, from the catalog rather than from the response: a
    /// server that lies about Content-Length, or omits it, must not be able
    /// to move the bar's denominator.
    pub fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }

    /// How far along, 0 to 1.
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

/// The running download, or nothing. App-global so it outlives the settings
/// window that started it.
#[derive(Default)]
struct Running(Option<Arc<Progress>>);

impl Global for Running {}

/// The last download's failure, kept after the download itself is gone so
/// the settings page can still say what went wrong.
#[derive(Default)]
struct LastFailure(Option<(String, String)>);

impl Global for LastFailure {}

/// The running download's progress, for a UI that wants to show it.
pub fn progress(cx: &App) -> Option<Arc<Progress>> {
    cx.try_global::<Running>().and_then(|r| r.0.clone())
}

/// What the last download failed with, as (model id, reason). Cleared when
/// a new download starts.
pub fn last_failure(cx: &App) -> Option<(String, String)> {
    cx.try_global::<LastFailure>().and_then(|f| f.0.clone())
}

/// Ask the running download to stop. The part file goes with it, so a stop
/// leaves nothing half-written behind.
pub fn stop(cx: &mut App) {
    if let Some(progress) = progress(cx) {
        progress.cancel.store(true, Ordering::Relaxed);
    }
}

/// Fetch a model's weights. A no-op while a download is already running, and
/// for a model that has nothing to fetch.
pub fn start(model: &'static Model, cx: &mut App) {
    if progress(cx).is_some() || model.weights.is_none() {
        return;
    }
    let progress = Arc::new(Progress::default());
    *progress.model.lock().unwrap() = model.id.to_string();
    progress.total.store(
        model.weights.as_ref().map_or(0, |w| w.bytes),
        Ordering::Relaxed,
    );
    cx.set_global(Running(Some(progress.clone())));
    cx.set_global(LastFailure(None));
    // Quitting mid-download shouldn't leave a part file behind, and the
    // worker deletes one on the way out of a cancelled fetch.
    cx.on_app_quit({
        let progress = progress.clone();
        move |_| {
            progress.cancel.store(true, Ordering::Relaxed);
            async {}
        }
    })
    .detach();
    cx.spawn(async move |cx| {
        let outcome = cx
            .background_executor()
            .spawn({
                let progress = progress.clone();
                async move { fetch(model, &progress) }
            })
            .await;
        cx.update(|cx| {
            if let Err(reason) = outcome {
                log::error!("model download: {}: {reason}", model.id);
                cx.set_global(LastFailure(Some((model.id.to_string(), reason))));
            } else {
                log::info!("model download: {} installed", model.id);
            }
            cx.set_global(Running(None));
        })
        .ok();
    })
    .detach();
}

/// The agent downloads ride. Not [`crate::providers::agent`]: that one caps
/// every request at ten seconds, which is the right call for a metadata
/// lookup and would guarantee failure on a 24 MB file over anything but a
/// fast link. This one bounds the connect and each read instead, so a
/// stalled connection still gives up while a slow-but-alive one is allowed
/// to finish.
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

/// The blocking half: stream the file to `<file>.part`, check the size and
/// hash, then rename into place.
///
/// The hash is computed as the bytes go by rather than by re-reading the
/// finished file, which halves the IO and means a mismatch is caught before
/// anything is renamed.
fn fetch(model: &Model, progress: &Progress) -> Result<(), String> {
    let weights = model.weights.as_ref().ok_or("this model has no weights")?;
    let dir = dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let final_path = dir.join(weights.file);
    let part_path = dir.join(format!("{}.part", weights.file));

    let response = agent()
        .get(weights.url)
        .call()
        .map_err(|e| crate::providers::net_reason(&e))?;

    // Guard the length before a byte is written: a redirect to an error page
    // or a repo whose file moved shows up here as a wildly different size,
    // and there's no point streaming megabytes to find that out.
    if let Some(claimed) = response
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok())
    {
        if claimed != weights.bytes {
            return Err(format!(
                "the server offered {claimed} bytes, the catalog expects {}",
                weights.bytes
            ));
        }
    }

    let outcome = stream(response.into_reader(), &part_path, weights, progress);
    match outcome {
        Ok(()) => {
            // Rename last, so nothing between here and the start of this
            // function could have been mistaken for an installed model.
            std::fs::rename(&part_path, &final_path)
                .map_err(|e| format!("{}: {e}", final_path.display()))
        }
        Err(reason) => {
            let _ = std::fs::remove_file(&part_path);
            Err(reason)
        }
    }
}

/// Copy the body into the part file, hashing and counting as it goes, then
/// check what landed against the catalog.
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
        // Refuse to keep writing past what the catalog says the file is, so
        // a server streaming forever can't fill the disk.
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

/// SHA-256 of everything a reader hands back, as lowercase hex.
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

    /// A catalog entry is a promise about a file on the internet, so the
    /// parts that can be checked without the network get checked here: the
    /// ids are unique and stable, the hashes are the right shape, and the
    /// built-in model is the one with nothing to fetch.
    #[test]
    fn the_catalog_is_well_formed() {
        let mut seen = std::collections::HashSet::new();
        for model in CATALOG {
            assert!(seen.insert(model.id), "duplicate model id {}", model.id);
            // The catalog states each model's width, and the pass trusts it to
            // size the vectors it writes.
            assert_eq!(
                model.dim,
                match model.id {
                    crate::embeddings::MODEL => crate::embeddings::DIM,
                    PANNS_CNN10 => crate::embeddings::panns::DIM,
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
                assert!(weights
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
                assert!(weights.bytes > 0);
                // A weight file must not be able to escape the models dir.
                assert!(!weights.file.contains('/') && !weights.file.contains('\\'));
            }
        }
        assert!(find(crate::embeddings::MODEL).is_some());
        assert!(find(PANNS_CNN10).is_some());
        assert!(find("nothing-like-this").is_none());
        // The fallback is the one that never needs a download.
        assert!(fallback().weights.is_none());
        assert!(fallback().installed());
    }

    /// PANNs' recipe has to describe a transform that can actually run, and
    /// its numbers are the ones the weights were fit against. A typo here is
    /// the failure mode this whole module is written around.
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
        // Centered framing, which is what torchlibrosa asks librosa for and
        // what every PyTorch pipeline inherits. Read off the config rather
        // than asserted flat, so this fails if the const above changes.
        let recipe = PANNS_MEL;
        assert!(recipe.center, "reflect-padded, librosa's framing");
        assert_eq!(PANNS_MEL.power, 2.0);
        // The override that catches people out: torchlibrosa defaults to a
        // top_db of 80 and Cnn10 turns it off, which makes the log absolute
        // rather than relative to each clip's own peak.
        assert_eq!(
            PANNS_MEL.log,
            mel::Log::Db {
                floor: 1e-10,
                top_db: None
            }
        );
        // 513 bins is what the shipped melW tensor is sized for.
        assert_eq!(PANNS_MEL.bins(), 513);
    }

    #[test]
    fn hex_is_lowercase_and_padded() {
        assert_eq!(hex(&[0x00, 0x0f, 0xff, 0xa5]), "000fffa5");
    }

    /// The empty input's SHA-256 is a published constant, which is enough to
    /// pin that the hash being computed is the hash the catalog names.
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

    /// A truncated download is refused rather than renamed into place, which
    /// is the failure this whole verification path exists for: a short file
    /// loads as weights and produces embeddings nobody can tell are wrong.
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

        // The right bytes go through.
        assert!(stream(&b"abc"[..], &part, &weights, &progress).is_ok());
        assert_eq!(progress.done(), 3);

        // One byte short: right prefix, wrong file.
        let short = stream(&b"ab"[..], &part, &weights, &progress).unwrap_err();
        assert!(short.contains("stopped at 2"), "{short}");

        // Right length, wrong contents.
        let wrong = stream(&b"abd"[..], &part, &weights, &progress).unwrap_err();
        assert!(wrong.contains("checksum"), "{wrong}");

        // A server that never stops sending is cut off at the stated size
        // rather than filling the disk.
        let flood = stream(&b"abcdefgh"[..], &part, &weights, &progress).unwrap_err();
        assert!(flood.contains("ran past"), "{flood}");

        // And a cancel stops it without writing a whole file.
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

    /// The real download, end to end against the catalog's URL. Ignored, so
    /// `cargo test` never reaches for the network or writes 24 MB into the
    /// data folder; run it by hand (`cargo test -- --ignored fetches_the`)
    /// when a catalog entry changes, since a wrong URL, size, or checksum is
    /// exactly the kind of mistake that only shows up against the server.
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
