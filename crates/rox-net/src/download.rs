//! One file to disk with its size and checksum known ahead: streamed to a
//! `.part` file beside the destination, stopped the moment it runs past its
//! size, hashed as it goes and renamed into place only when the sha256
//! matches. Nothing else in rox should ever see a file this hasn't passed.
//!
//! The size and the hash are whatever the caller was told. Where they were
//! fetched from the same server as the file, they prove the file is the one
//! that server meant and nothing about the server (ADR 35).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::providers::{USER_AGENT, net_reason};

const CHUNK: usize = 64 * 1024;

/// Written by the download, read by whatever shows it. Cancelling takes the
/// part file with it.
#[derive(Default)]
pub struct Progress {
    done: AtomicU64,
    total: AtomicU64,
    cancel: AtomicBool,
}

impl Progress {
    pub fn new() -> Self {
        Progress::default()
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn stopping(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub fn done(&self) -> u64 {
        self.done.load(Ordering::Relaxed)
    }

    /// The size the caller stated, so a lying Content-Length can't move it.
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
}

/// A release build takes https only, at the agent as well as wherever the
/// URL was made. Test and debug builds reach a plain http server on
/// loopback.
pub(crate) const HTTPS_ONLY: bool = cfg!(not(any(test, debug_assertions)));

/// Not [`crate::providers::agent`], whose ten-second total would fail a
/// large file: the connect and each read are bounded instead. It follows no
/// redirect at all, so a file can only come from the host it was asked of.
fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .user_agent(USER_AGENT)
            .timeout_connect(Duration::from_secs(15))
            .timeout_read(Duration::from_secs(30))
            .redirects(0)
            .https_only(HTTPS_ONLY)
            .build()
    })
}

/// Streams to `<dest>.part`, stops past `bytes`, checks the sha256, then
/// renames. Refuses a redirect. On any failure, cancel included, neither
/// the part file nor `dest` is left behind.
pub fn to_file(
    url: &str,
    dest: &Path,
    bytes: u64,
    sha256: &str,
    progress: &Progress,
) -> Result<(), String> {
    if !is_sha256(sha256) {
        return Err("the expected checksum isn't 64 lowercase hex characters".into());
    }

    let response = agent().get(url).call().map_err(|e| net_reason(&e))?;
    save(response, dest, bytes, sha256, progress)
}

/// [`to_file`] once the server has answered.
fn save(
    response: ureq::Response,
    dest: &Path,
    bytes: u64,
    sha256: &str,
    progress: &Progress,
) -> Result<(), String> {
    progress.total.store(bytes, Ordering::Relaxed);
    progress.done.store(0, Ordering::Relaxed);

    refuse_redirect(&response)?;

    // An error page or a moved file fails here, before anything streams.
    if let Some(claimed) = stated_length(&response)
        && claimed != bytes
    {
        return Err(format!(
            "the server offered {claimed} bytes where {bytes} were expected"
        ));
    }

    let part = part_path(dest);
    let outcome = stream(response.into_reader(), &part, bytes, sha256, progress).and_then(|()| {
        std::fs::rename(&part, dest).map_err(|e| format!("{}: {e}", dest.display()))
    });

    if outcome.is_err() {
        let _ = std::fs::remove_file(&part);
    }

    outcome
}

/// With redirects off, ureq hands a 3xx back as an answer rather than an
/// error, so this is the line that refuses one.
pub(crate) fn refuse_redirect(response: &ureq::Response) -> Result<(), String> {
    let status = response.status();
    if (300..400).contains(&status) {
        return Err(format!(
            "the server redirected ({status}), and rox follows no redirect here"
        ));
    }

    if !(200..300).contains(&status) {
        return Err(format!("service returned {status}"));
    }

    Ok(())
}

pub(crate) fn stated_length(response: &ureq::Response) -> Option<u64> {
    response
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok())
}

/// Beside `dest` with `.part` added to its whole name, so `a.roxw` and
/// `a.roxp` never share one.
fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

fn stream(
    mut body: impl Read,
    part: &Path,
    bytes: u64,
    sha256: &str,
    progress: &Progress,
) -> Result<(), String> {
    let mut out = std::io::BufWriter::new(
        std::fs::File::create(part).map_err(|e| format!("{}: {e}", part.display()))?,
    );
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; CHUNK];
    let mut done: u64 = 0;

    loop {
        if progress.stopping() {
            return Err("cancelled".into());
        }

        let read = body.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }

        // Counted before it's written, so a server that never stops can't
        // put one byte past the stated size on disk.
        done += read as u64;
        if done > bytes {
            return Err(format!(
                "the download ran past the {bytes} bytes it was meant to be"
            ));
        }

        hasher.update(&buffer[..read]);
        out.write_all(&buffer[..read])
            .map_err(|e| format!("{}: {e}", part.display()))?;
        progress.done.store(done, Ordering::Relaxed);
    }

    out.flush()
        .map_err(|e| format!("{}: {e}", part.display()))?;

    if done != bytes {
        return Err(format!("the download stopped at {done} of {bytes} bytes"));
    }

    let digest = hex(&hasher.finalize());
    if digest != sha256 {
        return Err(format!(
            "the download's checksum is {digest}, not the {sha256} it was meant to be"
        ));
    }

    Ok(())
}

pub fn is_sha256(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Canned answers for the tests here and in the catalog source, parsed
/// without a socket.
#[cfg(test)]
pub(crate) mod canned {
    pub fn ok(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    /// No Content-Length: the body runs until the connection closes.
    pub fn open_ended(body: &str) -> String {
        format!("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{body}")
    }

    pub fn typed(kind: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    pub fn redirect(to: &str) -> String {
        format!(
            "HTTP/1.1 302 Found\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
    }

    pub fn answer(text: &str) -> ureq::Response {
        text.parse().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canned::answer;

    fn sha(bytes: &[u8]) -> String {
        hex(&Sha256::digest(bytes))
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rox-download-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn listed(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_good_file_lands() {
        let body = "a package's bytes";
        let dir = scratch("good");
        let dest = dir.join("package.roxw");
        let progress = Progress::new();

        save(
            answer(&canned::ok(body)),
            &dest,
            body.len() as u64,
            &sha(body.as_bytes()),
            &progress,
        )
        .unwrap();

        assert_eq!(std::fs::read_to_string(&dest).unwrap(), body);
        assert_eq!(listed(&dir), vec!["package.roxw"]);
        assert_eq!(progress.fraction(), 1.0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_wrong_hash_leaves_nothing() {
        let body = "not the reviewed bytes";
        let dir = scratch("hash");

        let e = save(
            answer(&canned::ok(body)),
            &dir.join("package.roxw"),
            body.len() as u64,
            &sha(b"the reviewed bytes"),
            &Progress::new(),
        )
        .unwrap_err();

        assert!(e.contains("checksum"), "{e}");
        assert!(listed(&dir).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_body_past_its_size_stops_early() {
        // With no Content-Length to refuse up front, the count is the guard.
        let body = "7".repeat(4 * CHUNK);
        let dir = scratch("long");
        let dest = dir.join("package.roxw");
        let progress = Progress::new();

        let e = save(
            answer(&canned::open_ended(&body)),
            &dest,
            100,
            &sha(&body.as_bytes()[..100]),
            &progress,
        )
        .unwrap_err();

        assert!(e.contains("ran past"), "{e}");
        assert!(listed(&dir).is_empty());
        assert!(progress.done() <= 100);

        // A stated length that isn't the expected one fails before streaming.
        let e = save(
            answer(&canned::ok(&body)),
            &dest,
            100,
            &sha(&body.as_bytes()[..100]),
            &progress,
        )
        .unwrap_err();

        assert!(e.contains("offered"), "{e}");
        assert!(listed(&dir).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_short_body_fails() {
        let dir = scratch("short");

        let e = save(
            answer(&canned::open_ended("half")),
            &dir.join("package.roxw"),
            64,
            &sha(b"half"),
            &Progress::new(),
        )
        .unwrap_err();

        assert!(e.contains("stopped at 4 of 64"), "{e}");
        assert!(listed(&dir).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_redirect_to_another_host_is_refused() {
        let dir = scratch("redirect");

        let e = save(
            answer(&canned::redirect("http://127.0.0.1:9/f")),
            &dir.join("package.roxw"),
            4,
            &sha(b"good"),
            &Progress::new(),
        )
        .unwrap_err();

        assert!(e.contains("redirect"), "{e}");
        assert!(listed(&dir).is_empty());

        // The refusal above is only reached because the agent hands a 3xx
        // back instead of following it.
        assert!(format!("{:?}", agent()).contains("redirects: 0"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cancelled_download_leaves_nothing() {
        let body = "1".repeat(4 * CHUNK);
        let dir = scratch("cancel");
        let progress = Progress::new();
        progress.cancel();

        let e = save(
            answer(&canned::ok(&body)),
            &dir.join("package.roxw"),
            body.len() as u64,
            &sha(body.as_bytes()),
            &progress,
        )
        .unwrap_err();

        assert_eq!(e, "cancelled");
        assert!(listed(&dir).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_checksum_is_lowercase_hex() {
        assert!(is_sha256(&"a".repeat(64)));
        assert!(!is_sha256(&"A".repeat(64)));
        assert!(!is_sha256(&"a".repeat(63)));
        assert!(!is_sha256(&"g".repeat(64)));

        let e = to_file(
            "http://127.0.0.1:9/f",
            Path::new("x"),
            1,
            "nope",
            &Progress::new(),
        );
        assert!(e.unwrap_err().contains("checksum"));
    }
}
