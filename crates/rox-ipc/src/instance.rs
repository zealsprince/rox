//! The Windows half of the single-instance guard: one rox per data directory,
//! later launches hand their files to the running one. The app side lives in
//! `rox`'s `startup::single_instance`; this only moves opaque bytes.
//!
//! No staging-and-rename like Unix: a pipe can't go stale, and interprocess
//! creates a listener's first instance with `FILE_FLAG_FIRST_PIPE_INSTANCE`,
//! so ownership is one atomic create. [`claim`] creates before it connects.
//!
//! Named off the same data-dir hash as the control pipe, so `--portable` and
//! `--fresh` runs are their own instance. Pipe names are machine-wide; the
//! default DACL gives write access only to the creator, so a second account
//! sharing a portable folder is refused and runs unguarded.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use interprocess::local_socket::traits::{ListenerExt as _, Stream as _};
use interprocess::local_socket::{ListenerOptions, Name, Stream};

/// A Windows command line tops out around 32K characters; this only stops a
/// peer feeding the reader forever.
const MAX_HANDOFF_BYTES: u64 = 1024 * 1024;

/// More than one covers the owner quitting between our create and connect.
const ATTEMPTS: usize = 3;

const RETRY_PAUSE: Duration = Duration::from_millis(100);

/// Kept apart from the control pipe's `rox-ipc-` name so the two never collide.
pub fn pipe_path(data_dir: &Path) -> PathBuf {
    let hash = crate::data_dir_hash(data_dir);
    PathBuf::from(format!(r"\\.\pipe\rox-{hash:016x}"))
}

pub enum Claim {
    Owner(Listener),
    HandedOff,
    /// The launch runs without a guard; the reason is for the log.
    Unguarded(String),
}

pub struct Listener(interprocess::local_socket::Listener);

pub fn claim(data_dir: &Path, payload: &[u8]) -> Claim {
    let name = match crate::pipe_name(&pipe_path(data_dir)) {
        Ok(name) => name,
        Err(err) => return Claim::Unguarded(err.to_string()),
    };

    let mut refused = String::new();
    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(RETRY_PAUSE);
        }

        // Create first: winning the create is the whole ownership decision.
        match ListenerOptions::new().name(name.borrow()).create_sync() {
            Ok(listener) => return Claim::Owner(Listener(listener)),
            Err(err) if crate::pipe_taken(&err) => {}
            Err(err) => return Claim::Unguarded(err.to_string()),
        }

        // A refused handoff usually means the owner is quitting; go round and
        // see whether the name came free.
        match hand_off(name.borrow(), payload) {
            Ok(()) => return Claim::HandedOff,
            Err(err) => refused = err.to_string(),
        }
    }

    Claim::Unguarded(format!(
        "the running instance didn't take the handoff: {refused}"
    ))
}

/// On a pipe the flush waits until the owner has read everything.
fn hand_off(name: Name<'_>, payload: &[u8]) -> std::io::Result<()> {
    let mut stream = Stream::connect(name)?;
    stream.write_all(payload)?;
    stream.flush()
}

impl Listener {
    /// Each connection gets its own reader thread: pipes take no read
    /// timeout, so a silent peer would otherwise hold the accept loop.
    pub fn spawn(self) -> async_channel::Receiver<Vec<u8>> {
        let (tx, handoffs) = async_channel::unbounded();
        let listener = self.0;

        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let tx = tx.clone();

                std::thread::spawn(move || {
                    let mut payload = Vec::new();
                    if stream
                        .take(MAX_HANDOFF_BYTES)
                        .read_to_end(&mut payload)
                        .is_ok()
                    {
                        let _ = tx.send_blocking(payload);
                    }
                });
            }
        });

        handoffs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Arc, Barrier};

    fn scratch_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rox-instance-{}-{tag}", std::process::id()))
    }

    #[test]
    fn a_second_claim_hands_its_payload_to_the_first() {
        let dir = scratch_dir("pair");
        let Claim::Owner(listener) = claim(&dir, b"first") else {
            panic!("the first claim should own the pipe");
        };
        let handoffs = listener.spawn();

        assert!(matches!(claim(&dir, b"second"), Claim::HandedOff));
        assert_eq!(handoffs.recv_blocking().unwrap(), b"second");
    }

    #[test]
    fn simultaneous_claims_elect_exactly_one_owner() {
        const LAUNCHES: usize = 8;

        let dir = scratch_dir("race");
        let start = Arc::new(Barrier::new(LAUNCHES));
        let (owner_tx, owner_rx) = std::sync::mpsc::channel();

        let launches: Vec<_> = (0..LAUNCHES)
            .map(|i| {
                let dir = dir.clone();
                let start = start.clone();
                let owner_tx = owner_tx.clone();

                std::thread::spawn(move || {
                    start.wait();
                    match claim(&dir, i.to_string().as_bytes()) {
                        // Served at once so the losers can finish their flush.
                        Claim::Owner(listener) => {
                            owner_tx.send(listener.spawn()).unwrap();
                            true
                        }
                        Claim::HandedOff => false,
                        Claim::Unguarded(reason) => panic!("launch {i} ran unguarded: {reason}"),
                    }
                })
            })
            .collect();

        let owners = launches
            .into_iter()
            .map(|launch| launch.join().unwrap())
            .filter(|owned| *owned)
            .count();
        assert_eq!(owners, 1);

        let handoffs = owner_rx.recv().unwrap();
        for _ in 1..LAUNCHES {
            assert!(handoffs.recv_blocking().is_ok());
        }
    }
}
