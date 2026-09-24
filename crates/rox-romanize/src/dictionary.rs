//! The Japanese dictionary: what it is, whether it's installed, and the
//! download that installs it.
//!
//! Kanji needs a morphological dictionary, and IPADIC is ten megabytes on the
//! wire and forty on disk. The feature exists on the condition that it never
//! ships in the binary, so it downloads like the PANNs weights
//! (`rox_acoustic::models`, which this module is shaped after), into
//! `models/lindera-ipadic/` inside [`rox_core::settings::data_dir`].
//!
//! Size and SHA-256 are checked as the bytes arrive, before anything unpacks:
//! Lindera would open a truncated dictionary and read wrong entries. The hash
//! also pins the release asset, which can be replaced under a tag. The archive
//! is checked once and deleted after it unpacks.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use sha2::{Digest, Sha256};

/// Static data: the checksum is the security boundary, and one fetched with
/// the thing it checks isn't one.
pub struct Dictionary {
    /// Stable forever once shipped: the settings row and logs name it.
    pub id: &'static str,
    pub label: &'static str,
    pub summary: &'static str,
    pub url: &'static str,
    /// Inside `models/`, and also the archive's top-level directory.
    pub folder: &'static str,
    /// The archive on the wire, not the unpacked directory.
    pub bytes: u64,
    pub sha256: &'static str,
    /// Stated because the user is the one fetching it.
    pub licence: &'static str,
    pub source: &'static str,
}

/// Pinned to Lindera's v5.3.0 release: the binary format is Lindera's own.
/// Bumping `lindera` means bumping this asset, size and hash together, and
/// nothing checks them against each other but care.
///
/// IPADIC over NEologd (140 MB) or UniDic (46 MB): both read modern titles
/// better, not by enough to justify the download by default.
pub static IPADIC: Dictionary = Dictionary {
    id: "lindera-ipadic",
    label: "IPADIC",
    summary: "The Japanese dictionary behind kanji readings. Without it, kana and hangul still \
              romanize and Chinese still reads as pinyin, but a kanji title is skipped",
    url: "https://github.com/lindera/lindera/releases/download/v5.3.0/lindera-ipadic-5.3.0.zip",
    folder: "lindera-ipadic",
    bytes: 10_519_545,
    sha256: "6c361500b091abc1143c1d5abdd66a69463ab911685daf6ba74d6aeee7e180fe",
    // mecab-ipadic-2.7.0-20070801 (NAIST), redistributed by Lindera; the
    // archive's NOTICE.txt stays beside the data.
    licence: "Dictionary mecab-ipadic-2.7.0-20070801 (NAIST, BSD-3-Clause), engine MIT (Lindera)",
    source: "https://github.com/lindera/lindera",
};

/// Shared with the acoustic weights: one folder to delete to get the disk back.
pub fn dir() -> PathBuf {
    rox_core::settings::data_dir().join("models")
}

/// Checked instead of the directory, so an interrupted unpack isn't installed.
const MARKERS: [&str; 2] = ["metadata.json", "dict.trie"];

impl Dictionary {
    pub fn path(&self) -> PathBuf {
        dir().join(self.folder)
    }

    pub fn installed(&self) -> bool {
        let path = self.path();
        MARKERS.iter().all(|file| path.join(file).is_file())
    }

    /// One directory read: Lindera's layout is flat.
    pub fn size_on_disk(&self) -> u64 {
        let Ok(entries) = std::fs::read_dir(self.path()) else {
            return 0;
        };
        entries
            .flatten()
            .filter_map(|entry| entry.metadata().ok())
            .filter(|meta| meta.is_file())
            .map(|meta| meta.len())
            .sum()
    }

    /// What it already romanized stays in the library: still the best answer,
    /// and a delete shouldn't cost a re-run.
    pub fn delete(&self) -> Result<(), String> {
        let path = self.path();
        match std::fs::remove_dir_all(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }
}

/// The same shape as `rox_acoustic::models::Progress`, so the settings row
/// can sample either.
#[derive(Default)]
pub struct Progress {
    dictionary: Mutex<String>,
    done: AtomicU64,
    total: AtomicU64,
    /// Raised by [`Progress::cancel`] and by app quit.
    cancel: AtomicBool,
}

impl Progress {
    pub fn new(dictionary: &Dictionary) -> Self {
        let progress = Progress::default();
        *progress.dictionary.lock().unwrap() = dictionary.id.to_string();
        progress.total.store(dictionary.bytes, Ordering::Relaxed);
        progress
    }

    /// The part file goes with it.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn dictionary(&self) -> String {
        self.dictionary.lock().unwrap().clone()
    }

    pub fn done(&self) -> u64 {
        self.done.load(Ordering::Relaxed)
    }

    /// From the descriptor, never the response: a lying Content-Length must
    /// not move the bar's denominator.
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

/// Its own agent: rox-net caps a whole request at ten seconds, a sure failure
/// on ten megabytes. Connect and each read are bounded instead.
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

/// Hashed as the bytes go by, so a mismatch is caught before anything
/// unpacks.
pub fn fetch(dictionary: &Dictionary, progress: &Progress) -> Result<(), String> {
    let dir = dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let part_path = dir.join(format!("{}.zip.part", dictionary.folder));

    let response = agent()
        .get(dictionary.url)
        .call()
        .map_err(|e| e.to_string())?;

    // A replaced asset or an error page shows as a different size; refuse
    // before streaming megabytes.
    if let Some(claimed) = response
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok())
        && claimed != dictionary.bytes
    {
        return Err(format!(
            "the server offered {claimed} bytes, the catalog expects {}",
            dictionary.bytes
        ));
    }

    let outcome = stream(response.into_reader(), &part_path, dictionary, progress)
        .and_then(|()| unpack(&part_path, &dir, dictionary));
    // The archive is scratch either way; never mistake it for a resumable download.
    let _ = std::fs::remove_file(&part_path);
    if outcome.is_err() {
        // A failed unpack can leave a half-written directory.
        let _ = dictionary.delete();
    }
    outcome
}

fn stream(
    mut body: impl Read,
    part_path: &Path,
    dictionary: &Dictionary,
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
        // A server streaming forever can't fill the disk.
        done += read as u64;
        if done > dictionary.bytes {
            return Err("the download ran past the size the catalog states".into());
        }
        hasher.update(&buffer[..read]);
        part.write_all(&buffer[..read])
            .map_err(|e| format!("{}: {e}", part_path.display()))?;
        progress.done.store(done, Ordering::Relaxed);
    }
    part.flush().map_err(|e| e.to_string())?;

    if done != dictionary.bytes {
        return Err(format!(
            "the download stopped at {done} of {} bytes",
            dictionary.bytes
        ));
    }
    let digest = hex(&hasher.finalize());
    if digest != dictionary.sha256 {
        return Err(format!(
            "the download's checksum is {digest}, not the {} the catalog states",
            dictionary.sha256
        ));
    }
    Ok(())
}

/// Every entry has to sit under the descriptor's folder. The checksum rules
/// out a hostile zip; this guards a future descriptor laid out differently.
fn unpack(archive_path: &Path, dir: &Path, dictionary: &Dictionary) -> Result<(), String> {
    let file = std::fs::File::open(archive_path).map_err(|e| e.to_string())?;
    let mut archive =
        zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|e| e.to_string())?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let Some(name) = entry.enclosed_name() else {
            return Err(format!("{} holds an unsafe path", dictionary.id));
        };
        if !name.starts_with(dictionary.folder) {
            return Err(format!(
                "{} holds {}, which is outside {}",
                dictionary.id,
                name.display(),
                dictionary.folder
            ));
        }
        let out = dir.join(&name);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let mut sink = std::io::BufWriter::new(
            std::fs::File::create(&out).map_err(|e| format!("{}: {e}", out.display()))?,
        );
        std::io::copy(&mut entry, &mut sink).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The half of the descriptor checkable offline: release URL, hash shape,
    /// and a folder that can't escape `models/`.
    #[test]
    fn the_descriptor_is_well_formed() {
        assert!(
            IPADIC
                .url
                .starts_with("https://github.com/lindera/lindera/releases/download/")
        );
        assert!(
            IPADIC.url.contains("v5.3.0"),
            "the asset and the pinned lindera release have to move together"
        );
        assert_eq!(IPADIC.sha256.len(), 64);
        assert!(
            IPADIC
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
        assert!(IPADIC.bytes > 0);
        assert!(!IPADIC.folder.contains('/') && !IPADIC.folder.contains('\\'));
        assert!(!IPADIC.licence.is_empty());
        assert!(IPADIC.source.starts_with("https://"));
    }

    #[test]
    fn hex_is_lowercase_and_padded() {
        assert_eq!(hex(&[0x00, 0x0f, 0xff, 0xa5]), "000fffa5");
    }

    #[test]
    fn a_short_or_wrong_body_never_gets_unpacked() {
        let dir = std::env::temp_dir().join(format!("rox-dictionary-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let part = dir.join("test.part");
        let descriptor = Dictionary {
            id: "test",
            label: "Test",
            summary: "",
            url: "https://example.invalid/x",
            folder: "test",
            bytes: 3,
            // sha256("abc")
            sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            licence: "",
            source: "https://example.invalid",
        };
        let progress = Progress::default();

        assert!(stream(&b"abc"[..], &part, &descriptor, &progress).is_ok());
        assert_eq!(progress.done(), 3);

        let short = stream(&b"ab"[..], &part, &descriptor, &progress).unwrap_err();
        assert!(short.contains("stopped at 2"), "{short}");

        let wrong = stream(&b"abd"[..], &part, &descriptor, &progress).unwrap_err();
        assert!(wrong.contains("checksum"), "{wrong}");

        let flood = stream(&b"abcdefgh"[..], &part, &descriptor, &progress).unwrap_err();
        assert!(flood.contains("ran past"), "{flood}");

        progress.cancel();
        assert_eq!(
            stream(&b"abc"[..], &part, &descriptor, &progress).unwrap_err(),
            "cancelled"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_fraction_is_bounded_even_when_the_counters_are_not() {
        let progress = Progress::new(&IPADIC);
        assert_eq!(progress.dictionary(), IPADIC.id);
        assert_eq!(progress.total(), IPADIC.bytes);
        progress.done.store(IPADIC.bytes / 4, Ordering::Relaxed);
        assert!((progress.fraction() - 0.25).abs() < 1e-3);
        progress.done.store(u64::MAX, Ordering::Relaxed);
        assert_eq!(progress.fraction(), 1.0);
    }

    /// Ignored: hits the network and writes forty megabytes. Run by hand when
    /// the descriptor changes.
    #[test]
    #[ignore = "hits the network and writes into the data folder"]
    fn fetches_the_dictionary_it_describes() {
        IPADIC.delete().expect("clearing whatever was there");
        assert!(!IPADIC.installed());
        let progress = Progress::new(&IPADIC);
        fetch(&IPADIC, &progress).expect("the download lands");
        assert!(IPADIC.installed());
        assert_eq!(progress.done(), IPADIC.bytes);
        assert!(IPADIC.size_on_disk() > IPADIC.bytes, "it unpacked");
    }
}
