//! The Japanese dictionary: what it is, whether it's installed, and the
//! download that installs it.
//!
//! ## Why nothing is bundled
//!
//! Kana, hangul and pinyin are tables, small enough to compile in and
//! never absent. Kanji isn't: reading 東京 as `toukyou` needs a
//! morphological dictionary that segments the text and hands back a
//! reading per word, and IPADIC is ten megabytes on the wire and forty on
//! disk. Andrew's condition on this whole feature was that the dictionary
//! doesn't ship in the binary, so it lands the same way the PANNs weights
//! do ([`rox_acoustic::models`], which this module is shaped after): a
//! button on the Models page, a checksum, and an app that stays the size
//! it was for everyone who doesn't press it.
//!
//! ## Where it goes
//!
//! `models/lindera-ipadic/` inside [`rox_core::settings::data_dir`],
//! beside the acoustic weights, so a portable install carries it and a
//! wiped data folder takes it along.
//!
//! ## Verification
//!
//! The archive's size and SHA-256 are stated here and both are checked as
//! the bytes arrive, before anything is unpacked. A truncated download is
//! the failure this guards: Lindera's loader would open a short dictionary
//! and either fail somewhere deep or, worse, read wrong entries out of it.
//! The hash also pins the exact GitHub release asset, which can be
//! replaced under a tag.
//!
//! Unlike a weights file there's nothing to re-hash afterwards, because
//! what's installed is an unpacked directory rather than the archive. The
//! archive is checked once, on the way in, and deleted after it unpacks.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use sha2::{Digest, Sha256};

/// One dictionary the romanizer can read kanji with. Static data for the
/// same reason the acoustic catalog is: the checksum is the security
/// boundary, and a checksum fetched alongside the thing it checks isn't
/// one.
pub struct Dictionary {
    /// Stable forever once shipped: the settings row and the log lines
    /// name it.
    pub id: &'static str,
    pub label: &'static str,
    /// One line for the settings row, in plain language.
    pub summary: &'static str,
    pub url: &'static str,
    /// The directory it unpacks into inside `models/`, which is also the
    /// top-level directory inside the archive.
    pub folder: &'static str,
    /// Bytes of the archive on the wire, not of the unpacked directory.
    pub bytes: u64,
    /// Lowercase hex SHA-256 of the archive at `bytes` length.
    pub sha256: &'static str,
    /// The licence the data is under, stated because the user is the one
    /// fetching it.
    pub licence: &'static str,
    pub source: &'static str,
}

/// The one dictionary rox offers, pinned to Lindera's v5.3.0 release.
///
/// Pinned to a release rather than tracking the latest, because the binary
/// dictionary format is Lindera's own and a dictionary built by a newer
/// release is not guaranteed to load in the version rox links. Bumping the
/// `lindera` dependency means bumping this asset, its size and its hash
/// together, and the three are checked against each other by nothing but
/// care.
///
/// IPADIC rather than NEologd or UniDic, both of which read modern titles
/// better: NEologd is 140 MB against this 10 MB and UniDic 46 MB, and
/// neither difference buys enough on a search key to spend a user's
/// download on by default. This descriptor is what makes a second one an
/// entry rather than a redesign.
pub static IPADIC: Dictionary = Dictionary {
    id: "lindera-ipadic",
    label: "IPADIC",
    summary: "The Japanese dictionary behind kanji readings. Without it, kana and hangul still \
              romanize and Chinese still reads as pinyin, but a kanji title is skipped",
    url: "https://github.com/lindera/lindera/releases/download/v5.3.0/lindera-ipadic-5.3.0.zip",
    folder: "lindera-ipadic",
    bytes: 10_519_545,
    sha256: "6c361500b091abc1143c1d5abdd66a69463ab911685daf6ba74d6aeee7e180fe",
    // The data is mecab-ipadic-2.7.0-20070801, NAIST's, redistributed by
    // Lindera under its own three-clause notice; the archive carries that
    // notice as NOTICE.txt and unpacking keeps it beside the data.
    licence: "Dictionary mecab-ipadic-2.7.0-20070801 (NAIST, BSD-3-Clause), engine MIT (Lindera)",
    source: "https://github.com/lindera/lindera",
};

/// Where downloaded data lives. The same `models/` the acoustic weights
/// use, deliberately: one folder a user can delete to get their disk back.
pub fn dir() -> PathBuf {
    rox_core::settings::data_dir().join("models")
}

/// The two files every Lindera dictionary directory has, and the ones a
/// half-unpacked archive would be missing. Checked rather than the
/// directory's mere existence, so an interrupted unpack doesn't read as
/// installed.
const MARKERS: [&str; 2] = ["metadata.json", "dict.trie"];

impl Dictionary {
    /// The directory this dictionary unpacks into.
    pub fn path(&self) -> PathBuf {
        dir().join(self.folder)
    }

    /// Whether the dictionary is there to be loaded.
    pub fn installed(&self) -> bool {
        let path = self.path();
        MARKERS.iter().all(|file| path.join(file).is_file())
    }

    /// What the unpacked dictionary weighs, for the settings readout. Zero
    /// when nothing is installed. One directory read rather than a walk:
    /// Lindera's layout is flat.
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

    /// Remove the unpacked dictionary. Whatever it already romanized stays
    /// in the library's tables: those rows are still the best answer rox
    /// has, and deleting a directory shouldn't cost a re-run of the pass.
    pub fn delete(&self) -> Result<(), String> {
        let path = self.path();
        match std::fs::remove_dir_all(&path) {
            Ok(()) => Ok(()),
            // Already gone is the state the caller asked for.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }
}

/// Live progress of a download: the worker writes it, the UI polls it.
/// The same shape [`rox_acoustic::models::Progress`] has, so the settings
/// window's model row can sample either one.
#[derive(Default)]
pub struct Progress {
    /// Which dictionary is coming down, so a UI can tell whose row to
    /// light up.
    dictionary: Mutex<String>,
    done: AtomicU64,
    total: AtomicU64,
    /// Raised by [`Progress::cancel`] and by app quit.
    cancel: AtomicBool,
}

impl Progress {
    /// A fresh readout for a download of `dictionary`. The total comes off
    /// the descriptor rather than off the response, for the reason
    /// [`Progress::total`] gives.
    pub fn new(dictionary: &Dictionary) -> Self {
        let progress = Progress::default();
        *progress.dictionary.lock().unwrap() = dictionary.id.to_string();
        progress.total.store(dictionary.bytes, Ordering::Relaxed);
        progress
    }

    /// Ask the running download to stop. The part file goes with it, so a
    /// stop leaves nothing half-written behind.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn dictionary(&self) -> String {
        self.dictionary.lock().unwrap().clone()
    }

    pub fn done(&self) -> u64 {
        self.done.load(Ordering::Relaxed)
    }

    /// Bytes expected, from the descriptor rather than from the response:
    /// a server that lies about Content-Length, or omits it, must not be
    /// able to move the bar's denominator.
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

/// The agent this download uses. Its own rather than rox-net's, for the
/// reason the acoustic one gives: a ten-second cap on the whole request is
/// right for a metadata lookup and guarantees failure on a ten-megabyte
/// file. The connect and each read are bounded instead, so a stalled
/// connection gives up and a slow one is allowed to finish.
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

/// The blocking half: stream the archive to a `.part` file, check its size
/// and hash, unpack it, then delete the archive.
///
/// The hash is computed as the bytes go by rather than by re-reading the
/// file, which halves the IO and means a mismatch is caught before
/// anything is unpacked.
pub fn fetch(dictionary: &Dictionary, progress: &Progress) -> Result<(), String> {
    let dir = dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let part_path = dir.join(format!("{}.zip.part", dictionary.folder));

    let response = agent()
        .get(dictionary.url)
        .call()
        .map_err(|e| e.to_string())?;

    // Guard the length before a byte is written: a redirect to an error
    // page, or a release whose asset was replaced, shows up here as a
    // wildly different size, and there's no point streaming megabytes to
    // find that out.
    if let Some(claimed) = response
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok())
    {
        if claimed != dictionary.bytes {
            return Err(format!(
                "the server offered {claimed} bytes, the catalog expects {}",
                dictionary.bytes
            ));
        }
    }

    let outcome = stream(response.into_reader(), &part_path, dictionary, progress)
        .and_then(|()| unpack(&part_path, &dir, dictionary));
    // The archive is scratch either way: it unpacked, or it didn't and
    // nothing should mistake it for a download to resume.
    let _ = std::fs::remove_file(&part_path);
    if outcome.is_err() {
        // A failed unpack can leave a half-written directory, which
        // `installed` would already refuse; remove it so a retry starts
        // clean.
        let _ = dictionary.delete();
    }
    outcome
}

/// Copy the body into the part file, hashing and counting as it goes, then
/// check what arrived against the descriptor.
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
        // Refuse to keep writing past what the descriptor states, so a
        // server streaming forever can't fill the disk.
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

/// Unpack the checked archive into `dir`.
///
/// Every entry has to sit under the descriptor's own folder. The checksum
/// already pins the archive's contents byte for byte, so this can't be
/// reached by a hostile zip; it's here because a future descriptor could
/// name an asset laid out differently, and an archive that quietly writes
/// outside `models/` is not a mistake worth being able to make.
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

    /// The descriptor is a promise about a file on the internet, so the
    /// half of it that can be checked without the network gets checked
    /// here: the URL is the release the comment names, the hash is the
    /// right shape, and the folder can't escape `models/`.
    #[test]
    fn the_descriptor_is_well_formed() {
        assert!(IPADIC
            .url
            .starts_with("https://github.com/lindera/lindera/releases/download/"));
        assert!(
            IPADIC.url.contains("v5.3.0"),
            "the asset and the pinned lindera release have to move together"
        );
        assert_eq!(IPADIC.sha256.len(), 64);
        assert!(IPADIC
            .sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        assert!(IPADIC.bytes > 0);
        assert!(!IPADIC.folder.contains('/') && !IPADIC.folder.contains('\\'));
        assert!(!IPADIC.licence.is_empty());
        assert!(IPADIC.source.starts_with("https://"));
    }

    #[test]
    fn hex_is_lowercase_and_padded() {
        assert_eq!(hex(&[0x00, 0x0f, 0xff, 0xa5]), "000fffa5");
    }

    /// A truncated or substituted archive is refused before anything
    /// unpacks, which is the failure the whole descriptor exists for.
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

        // A server that never stops sending is cut off at the stated size
        // rather than filling the disk.
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

    /// The real download, end to end. Ignored, so `cargo test` never
    /// touches the network or writes forty megabytes into the data folder;
    /// run it by hand (`cargo test -p rox-romanize -- --ignored fetches`)
    /// when the descriptor changes, since a wrong URL, size or checksum is
    /// exactly the mistake that only shows up against the server.
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
