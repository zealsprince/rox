//! A plugin's stream as the decoder sees it (ADR 29's first amendment, ADR
//! 30). The plugin serves container bytes by offset, and [`PluginSource`]
//! reads them like a file, with a window of recent bytes so the probe's small
//! backward seeks never go back to the plugin. A seek outside the window is a
//! read at the new offset, and on a stream that can't seek it's an error.
//!
//! The engine never knows what's behind an [`Opener`]. It's passed in with
//! the queue, so this crate never depends on the plugin host.
//!
//! A failed read isn't the end of the track. A seekable stream of known
//! length reopens through the opener with radio's backoff and carries on at
//! the byte it failed on, provided the reopened stream is the same length:
//! a different length is a different encode, and two encodes never splice.
//! A live stream gets radio's feed thread and tape instead, and rejoins at
//! the live edge with a gap. Only when that runs out does the track end with
//! a refusal naming the plugin.
//!
//! [`ReadAt::read_at`] blocks the decode thread (or a live stream's feed
//! thread) for as long as the plugin takes, and it's never called from the
//! output callback. Read-ahead and timeouts belong to the host side of the
//! opener.

use std::io;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use rox_library::locator::PluginStream;
use symphonia::core::io::MediaSource;

use crate::download::{Buffered, Download};
use crate::http::{BACKOFF, LiveSource, Upstream};
use crate::shared::{BufferSink, RefusalSink, StreamSink, StreamState};
use crate::tape::{Snap, Tape};

pub trait ReadAt: Send + Sync {
    /// At most `len` bytes from `offset`; empty means end of stream.
    fn read_at(&self, offset: u64, len: usize) -> Result<Vec<u8>, String>;
}

pub struct Opened {
    /// Dropping it closes the stream.
    pub reader: Box<dyn ReadAt>,
    /// Container extension for the probe; empty lets it sniff.
    pub hint: String,
    pub length: Option<u64>,
    pub seekable: bool,
    /// False when the plugin asked for read-ahead only. The engine downloads
    /// the rest whole when they qualify ([`crate::download`]).
    pub buffer_whole: bool,
    /// The plugin's word on the length, for a container that doesn't state
    /// one.
    pub duration_ms: Option<u64>,
}

/// The reader the decoder reads through: a download for a stream that
/// qualifies, the plugin's own reader otherwise.
pub(crate) fn reader_for(opened: Opened) -> (Box<dyn ReadAt>, Option<Arc<dyn Buffered>>) {
    let whole = opened
        .buffer_whole
        .then(|| crate::download::whole(opened.length, opened.seekable))
        .flatten();

    let Some(length) = whole else {
        return (opened.reader, None);
    };

    match Download::start(opened.reader, length, &opened.hint) {
        Ok((download, buffered)) => (Box::new(download), Some(buffered)),
        Err(e) => {
            log::warn!("plugin stream: {e}; reading it as it plays instead");
            (Box::new(Unbuffered), None)
        }
    }
}

/// Stands in when the download thread couldn't start and the stream went
/// with it: every read fails, which starts the recovery.
struct Unbuffered;

impl ReadAt for Unbuffered {
    fn read_at(&self, _: u64, _: usize) -> Result<Vec<u8>, String> {
        Err("the download couldn't start".into())
    }
}

pub type Opener = Arc<dyn Fn(&PluginStream) -> Result<Opened, String> + Send + Sync>;

/// Asked of the plugin per read.
const CHUNK: usize = 256 * 1024;

/// Sized like `HttpSource`'s, for the same reason: a big ID3v2 tag pushes the
/// container header this far, and the probe walks back over it.
const WINDOW: usize = 1 << 20;

/// What a stream needs to come back after a failed read: the way it was
/// opened, and where to say how it's going.
pub struct Recovery {
    pub stream: PluginStream,
    pub opener: Opener,
    pub on_stream: StreamSink,
    pub on_refusal: RefusalSink,
    /// A reopen that downloads whole says so, replacing the old download.
    pub on_buffer: BufferSink,
    pub interrupt: Arc<AtomicBool>,
}

pub struct PluginSource {
    /// None between a failed read and its reopen, or once recovery gave up.
    reader: Option<Box<dyn ReadAt>>,
    seekable: bool,
    len: Option<u64>,
    /// The only place a backward seek on an unseekable stream can be answered.
    buf: Vec<u8>,
    /// Stream offset of `buf[0]`; the cursor never leaves the window.
    buf_start: u64,
    pos: u64,
    /// None reads without recovery: a failed read is the end.
    recovery: Option<Recovery>,
}

impl PluginSource {
    pub fn new(reader: Box<dyn ReadAt>, length: Option<u64>, seekable: bool) -> PluginSource {
        PluginSource {
            reader: Some(reader),
            seekable,
            len: length,
            buf: Vec::new(),
            buf_start: 0,
            pos: 0,
            recovery: None,
        }
    }

    pub fn with_recovery(mut self, recovery: Recovery) -> PluginSource {
        self.recovery = Some(recovery);
        self
    }

    /// Where the next read from the plugin starts.
    fn head(&self) -> u64 {
        self.buf_start + self.buf.len() as u64
    }

    /// One read at the head. False at the end of the stream.
    fn fill(&mut self) -> io::Result<bool> {
        let at = self.head();
        let read = match &self.reader {
            Some(reader) => reader.read_at(at, CHUNK),
            None => Err("the stream is closed".to_string()),
        };

        let bytes = match read {
            Ok(bytes) => bytes,
            Err(lost) => self.recover(at, lost)?,
        };
        if bytes.is_empty() {
            return Ok(false);
        }

        self.buf.extend_from_slice(&bytes);
        self.trim();

        Ok(true)
    }

    /// Reopen and read on from `at`: radio's backoff, publishing each
    /// attempt, and a refusal once they run out. A host restart after a crash
    /// is just a slow reopen here.
    fn recover(&mut self, at: u64, first: String) -> io::Result<Vec<u8>> {
        // Taken for the duration, and never put back after a give-up, so a
        // stream that gave up stays given up.
        let Some(rec) = self.recovery.take() else {
            return Err(io::Error::other(first));
        };
        let name = rec.stream.source.clone();

        // Only a stream readable from any byte, with a length to check a
        // reopen against, can pick up where it left off.
        let Some(length) = self.len.filter(|_| self.seekable) else {
            let reason = format!("{name}: {first}, and the stream can't resume where it stopped");
            return Err(give_up(&rec, reason));
        };

        let mut lost = first;
        for (attempt, wait) in BACKOFF.iter().enumerate() {
            // Closes the stream the plugin failed on.
            self.reader = None;

            (rec.on_stream)(StreamState::Reconnecting);
            log::info!(
                "plugin stream dropped ({lost}), reopening in {wait:?} (attempt {} of {})",
                attempt + 1,
                BACKOFF.len()
            );

            if crate::http::nap(*wait, &rec.interrupt) {
                log::info!("plugin stream retry abandoned: a command is waiting");
                (rec.on_stream)(StreamState::Dropped);

                return Err(io::Error::other(format!("{name}: {lost}")));
            }

            let opened = match (rec.opener)(&rec.stream) {
                Ok(opened) => opened,
                Err(e) => {
                    lost = e;
                    continue;
                }
            };

            if opened.length != Some(length) {
                let reason = format!(
                    "{name}: the stream came back as a different encode ({} bytes where it was {length}), and two encodes don't splice",
                    opened.length.map_or("unknown".into(), |l| l.to_string())
                );
                return Err(give_up(&rec, reason));
            }

            let (reader, buffered) = reader_for(opened);
            match reader.read_at(at, CHUNK) {
                Ok(bytes) => {
                    log::info!(
                        "plugin stream recovered at byte {at} after {} attempt(s)",
                        attempt + 1
                    );
                    self.reader = Some(reader);
                    if let Some(buffered) = &buffered {
                        (rec.on_buffer)(Arc::downgrade(buffered));
                    }
                    (rec.on_stream)(StreamState::Live);
                    self.recovery = Some(rec);

                    return Ok(bytes);
                }

                Err(e) => lost = e,
            }
        }

        let reason = format!("{name}: {lost}");
        Err(give_up(&rec, reason))
    }

    /// Drops the oldest half on overflow, never past the cursor, so a plugin
    /// that answers more than it was asked for still gets read in full.
    fn trim(&mut self) {
        if self.buf.len() <= WINDOW {
            return;
        }

        let behind = (self.pos - self.buf_start) as usize;
        let drop = (self.buf.len() - WINDOW / 2).min(behind);
        self.buf.drain(..drop);
        self.buf_start += drop as u64;
    }
}

fn give_up(rec: &Recovery, reason: String) -> io::Error {
    log::warn!("plugin stream gone: {reason}");
    (rec.on_stream)(StreamState::Dropped);
    (rec.on_refusal)(reason.clone());

    io::Error::other(reason)
}

impl Read for PluginSource {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }

        if self.pos == self.head() && !self.fill()? {
            return Ok(0);
        }

        let off = (self.pos - self.buf_start) as usize;
        let n = out.len().min(self.buf.len() - off);
        out[..n].copy_from_slice(&self.buf[off..off + n]);
        self.pos += n as u64;

        Ok(n)
    }
}

impl Seek for PluginSource {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let target = match from {
            SeekFrom::Start(at) => Some(at),

            SeekFrom::Current(delta) => self.pos.checked_add_signed(delta),

            // Symphonia asks this of sources it hasn't checked `is_seekable` on.
            SeekFrom::End(delta) => match self.len {
                Some(len) => len.checked_add_signed(delta),
                None => return Err(io::Error::other("no length to seek from the end of")),
            },
        }
        .ok_or_else(|| io::Error::other("seek out of range"))?;

        // The head counts as inside: it's where the next read goes anyway.
        if target >= self.buf_start && target <= self.head() {
            self.pos = target;
            return Ok(target);
        }

        if !self.seekable {
            return Err(io::Error::other("stream is not seekable"));
        }

        self.buf.clear();
        self.buf_start = target;
        self.pos = target;

        Ok(target)
    }
}

impl MediaSource for PluginSource {
    fn is_seekable(&self) -> bool {
        self.seekable
    }

    fn byte_len(&self) -> Option<u64> {
        self.len
    }
}

/// A live plugin stream reads in order from wherever it was opened, and a
/// reopen joins at the live edge, as a station's reconnect does.
struct LiveUpstream {
    stream: PluginStream,
    opener: Opener,
    reader: Option<Box<dyn ReadAt>>,
    pos: u64,
}

impl Upstream for LiveUpstream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let reader = self
            .reader
            .as_ref()
            .ok_or_else(|| io::Error::other("the stream is closed"))?;

        let bytes = reader
            .read_at(self.pos, buf.len())
            .map_err(io::Error::other)?;
        let n = bytes.len().min(buf.len());
        buf[..n].copy_from_slice(&bytes[..n]);
        self.pos += n as u64;

        Ok(n)
    }

    fn reconnect(&mut self) -> Result<(), String> {
        // The old stream closes before the new one opens.
        self.reader = None;

        let opened = (self.opener)(&self.stream)?;
        self.reader = Some(opened.reader);
        self.pos = 0;

        Ok(())
    }
}

/// Ogg only decodes from a page, so a seek on its tape scans to one.
fn snap_for(hint: &str) -> Snap {
    match hint.eq_ignore_ascii_case("ogg") || hint.eq_ignore_ascii_case("opus") {
        true => Snap::OggPage,
        false => Snap::Anywhere,
    }
}

/// A live plugin stream on radio's tape: a feed thread pulls `source.read`
/// whether or not anything decodes, so a pause resumes where it stopped.
///
/// The tape starts with no stated bitrate and measures the rate off the
/// decoder, as it does for a station that sends no `icy-br`: the locator
/// carries no bitrate, and a plugin's row may not know one.
pub fn open_live(
    stream: &PluginStream,
    opener: &Opener,
    opened: Opened,
    on_stream: StreamSink,
    interrupt: Arc<AtomicBool>,
    window_secs: u32,
) -> Result<(LiveSource, Arc<Tape>), String> {
    let tape = Arc::new(Tape::new(
        window_secs,
        0,
        snap_for(&opened.hint),
        crate::icy::no_titles(),
        Arc::clone(&interrupt),
    ));

    let upstream = LiveUpstream {
        stream: stream.clone(),
        opener: Arc::clone(opener),
        reader: Some(opened.reader),
        pos: 0,
    };
    let tape = crate::http::feed_tape(
        tape,
        Box::new(upstream),
        "plugin stream",
        on_stream,
        interrupt,
    )?;

    Ok((crate::http::live_source(&tape, 0), tape))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serves `bytes`, at most `most` per read, and records every offset asked for.
    struct Memory {
        bytes: Vec<u8>,
        most: usize,
        asked: Mutex<Vec<u64>>,
    }

    impl Memory {
        fn new(bytes: Vec<u8>, most: usize) -> Arc<Memory> {
            Arc::new(Memory {
                bytes,
                most,
                asked: Mutex::new(Vec::new()),
            })
        }

        fn asked(&self) -> Vec<u64> {
            self.asked.lock().unwrap().clone()
        }
    }

    impl ReadAt for Arc<Memory> {
        fn read_at(&self, offset: u64, len: usize) -> Result<Vec<u8>, String> {
            self.asked.lock().unwrap().push(offset);

            let start = (offset as usize).min(self.bytes.len());
            let end = (start + len.min(self.most)).min(self.bytes.len());

            Ok(self.bytes[start..end].to_vec())
        }
    }

    struct Failing;

    impl ReadAt for Failing {
        fn read_at(&self, _: u64, _: usize) -> Result<Vec<u8>, String> {
            Err("upstream went away".into())
        }
    }

    fn pattern(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    fn source(memory: &Arc<Memory>, seekable: bool) -> PluginSource {
        let len = memory.bytes.len() as u64;

        PluginSource::new(Box::new(Arc::clone(memory)), Some(len), seekable)
    }

    #[test]
    fn a_sequential_read_returns_every_byte_then_ends() {
        let bytes = pattern(CHUNK * 3 + 17);
        let memory = Memory::new(bytes.clone(), usize::MAX);
        let mut src = source(&memory, true);

        let mut out = Vec::new();
        src.read_to_end(&mut out).expect("the stream reads");

        assert_eq!(out, bytes);
        assert_eq!(src.read(&mut [0; 16]).unwrap(), 0, "and stays ended");
        assert_eq!(src.byte_len(), Some(bytes.len() as u64));
    }

    #[test]
    fn short_reads_still_add_up_to_the_whole_stream() {
        let bytes = pattern(10_000);
        let memory = Memory::new(bytes.clone(), 333);
        let mut src = source(&memory, false);

        let mut out = Vec::new();
        src.read_to_end(&mut out).expect("the stream reads");

        assert_eq!(out, bytes);

        // Each read picks up exactly where the last one stopped, which is all an
        // unseekable plugin can answer.
        let asked = memory.asked();
        for pair in asked.windows(2) {
            assert!(pair[1] - pair[0] <= 333);
        }
    }

    #[test]
    fn a_seek_inside_the_window_asks_the_plugin_nothing() {
        let bytes = pattern(CHUNK * 2);
        let memory = Memory::new(bytes.clone(), usize::MAX);
        let mut src = source(&memory, false);

        let mut head = [0; 4096];
        src.read_exact(&mut head).unwrap();
        let asked = memory.asked().len();

        assert_eq!(src.seek(SeekFrom::Start(10)).unwrap(), 10);
        let mut again = [0; 32];
        src.read_exact(&mut again).unwrap();

        assert_eq!(&again[..], &bytes[10..42]);
        assert_eq!(memory.asked().len(), asked, "served from memory");
    }

    #[test]
    fn a_seek_outside_the_window_reads_at_the_new_offset() {
        let bytes = pattern(WINDOW * 3);
        let memory = Memory::new(bytes.clone(), usize::MAX);
        let mut src = source(&memory, true);

        let target = (WINDOW * 2 + 5) as u64;
        assert_eq!(src.seek(SeekFrom::Start(target)).unwrap(), target);

        let mut out = [0; 64];
        src.read_exact(&mut out).unwrap();

        assert_eq!(&out[..], &bytes[target as usize..target as usize + 64]);
        assert_eq!(memory.asked().last(), Some(&target));

        // Back to the top is outside the window again.
        src.seek(SeekFrom::Start(0)).unwrap();
        src.read_exact(&mut out).unwrap();
        assert_eq!(&out[..], &bytes[..64]);
    }

    #[test]
    fn an_unseekable_stream_refuses_a_seek_it_cant_serve() {
        let bytes = pattern(WINDOW * 3);
        let memory = Memory::new(bytes, usize::MAX);
        let mut src = source(&memory, false);

        // Read far enough that the start has left the window.
        let mut out = vec![0; WINDOW * 2];
        src.read_exact(&mut out).unwrap();

        let back = src.seek(SeekFrom::Start(0));
        assert!(
            back.is_err(),
            "the start is gone and the plugin can't go back"
        );

        let ahead = src.seek(SeekFrom::Start((WINDOW * 3 - 1) as u64));
        assert!(ahead.is_err(), "nor skip ahead");

        assert!(!src.is_seekable());
    }

    #[test]
    fn a_plugin_error_surfaces_as_an_io_error() {
        let mut src = PluginSource::new(Box::new(Failing), None, false);

        let err = src.read(&mut [0; 16]).expect_err("the read fails");
        assert_eq!(err.to_string(), "upstream went away");

        assert!(
            src.seek(SeekFrom::End(0)).is_err(),
            "no length to count back from"
        );
    }

    mod recovery {
        use super::super::*;
        use super::{Failing, pattern};
        use std::sync::Mutex;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::{Duration, Instant};

        use crate::http::testing;
        use crate::shared::StreamState;

        /// Serves `bytes`, failing every read from `fail_at` on while `fails`
        /// (shared across every stream the opener hands out) counts down.
        struct Flaky {
            bytes: Arc<Vec<u8>>,
            fail_at: u64,
            fails: Arc<AtomicUsize>,
        }

        impl ReadAt for Flaky {
            fn read_at(&self, offset: u64, len: usize) -> Result<Vec<u8>, String> {
                if offset + len as u64 > self.fail_at
                    && self
                        .fails
                        .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
                        .is_ok()
                {
                    return Err("upstream went away".into());
                }

                let start = (offset as usize).min(self.bytes.len());
                let end = (start + len).min(self.bytes.len());

                Ok(self.bytes[start..end].to_vec())
            }
        }

        struct Harness {
            opens: Arc<AtomicUsize>,
            states: Arc<Mutex<Vec<StreamState>>>,
            refusals: Arc<Mutex<Vec<String>>>,
            interrupt: Arc<AtomicBool>,
        }

        fn stream() -> PluginStream {
            PluginStream {
                source: "plugin:test".into(),
                key: "k".into(),
                live: false,
                duration_ms: None,
            }
        }

        /// A source that fails `fails` reads past `fail_at`, and an opener
        /// that reopens it at `reopen_len` bytes.
        fn flaky(
            bytes: Vec<u8>,
            fail_at: u64,
            fails: usize,
            reopen_len: u64,
        ) -> (PluginSource, Harness) {
            testing::clear_naps();

            let bytes = Arc::new(bytes);
            let len = bytes.len() as u64;
            let fails = Arc::new(AtomicUsize::new(fails));
            let opens = Arc::new(AtomicUsize::new(0));

            let reader = Flaky {
                bytes: Arc::clone(&bytes),
                fail_at,
                fails: Arc::clone(&fails),
            };

            let counted = Arc::clone(&opens);
            let opener: Opener = Arc::new(move |_| {
                counted.fetch_add(1, Ordering::Relaxed);

                Ok(Opened {
                    reader: Box::new(Flaky {
                        bytes: Arc::clone(&bytes),
                        fail_at,
                        fails: Arc::clone(&fails),
                    }),
                    hint: String::new(),
                    length: Some(reopen_len),
                    seekable: true,
                    buffer_whole: false,
                    duration_ms: None,
                })
            });

            let states = Arc::new(Mutex::new(Vec::new()));
            let refusals = Arc::new(Mutex::new(Vec::new()));
            let interrupt = Arc::new(AtomicBool::new(false));

            let (seen, said) = (Arc::clone(&states), Arc::clone(&refusals));
            let src =
                PluginSource::new(Box::new(reader), Some(len), true).with_recovery(Recovery {
                    stream: stream(),
                    opener,
                    on_stream: Arc::new(move |state| seen.lock().unwrap().push(state)),
                    on_refusal: Arc::new(move |reason| said.lock().unwrap().push(reason)),
                    on_buffer: crate::shared::no_buffer(),
                    interrupt: Arc::clone(&interrupt),
                });

            let harness = Harness {
                opens,
                states,
                refusals,
                interrupt,
            };

            (src, harness)
        }

        #[test]
        fn a_read_that_fails_once_resumes_at_the_same_byte() {
            let bytes = pattern(CHUNK * 3);
            let (mut src, h) = flaky(bytes.clone(), CHUNK as u64 + 10, 1, bytes.len() as u64);

            let mut out = Vec::new();
            src.read_to_end(&mut out).expect("it recovers");

            assert_eq!(out, bytes, "every byte, none twice");
            assert_eq!(h.opens.load(Ordering::Relaxed), 1, "one reopen");
            assert_eq!(
                *h.states.lock().unwrap(),
                vec![StreamState::Reconnecting, StreamState::Live]
            );
            assert!(h.refusals.lock().unwrap().is_empty());
            assert_eq!(
                testing::napped(),
                Duration::ZERO,
                "the first attempt is free"
            );
        }

        #[test]
        fn a_reopen_with_a_different_length_drops_with_a_refusal() {
            let bytes = pattern(CHUNK * 3);
            let (mut src, h) = flaky(bytes.clone(), CHUNK as u64 + 10, 1, bytes.len() as u64 + 1);

            let mut out = Vec::new();
            assert!(src.read_to_end(&mut out).is_err(), "the track ends");

            assert_eq!(h.states.lock().unwrap().last(), Some(&StreamState::Dropped));
            let refusals = h.refusals.lock().unwrap();
            assert_eq!(refusals.len(), 1);
            assert!(refusals[0].starts_with("plugin:test: "), "{}", refusals[0]);
            assert!(refusals[0].contains("different encode"), "{}", refusals[0]);
        }

        #[test]
        fn four_failed_attempts_drop_the_stream() {
            let bytes = pattern(CHUNK * 3);
            let (mut src, h) = flaky(
                bytes.clone(),
                CHUNK as u64 + 10,
                usize::MAX,
                bytes.len() as u64,
            );

            let mut out = Vec::new();
            let err = src.read_to_end(&mut out).expect_err("the track ends");

            assert_eq!(h.opens.load(Ordering::Relaxed), BACKOFF.len());
            assert_eq!(testing::napped(), Duration::from_secs(1 + 2 + 4));
            assert_eq!(h.states.lock().unwrap().last(), Some(&StreamState::Dropped));
            assert_eq!(*h.refusals.lock().unwrap(), vec![err.to_string()]);

            // Given up stays given up: no second round of attempts.
            assert!(src.read(&mut [0; 16]).is_err());
            assert_eq!(h.opens.load(Ordering::Relaxed), BACKOFF.len());
        }

        #[test]
        fn an_interrupt_during_the_wait_abandons_the_recovery() {
            let bytes = pattern(CHUNK * 3);
            let (mut src, h) = flaky(
                bytes.clone(),
                CHUNK as u64 + 10,
                usize::MAX,
                bytes.len() as u64,
            );
            testing::interrupt_after(1, Arc::clone(&h.interrupt));

            let mut out = Vec::new();
            assert!(src.read_to_end(&mut out).is_err());

            assert_eq!(
                h.opens.load(Ordering::Relaxed),
                1,
                "the free attempt, then the flag"
            );
            assert_eq!(testing::naps(), vec![crate::http::NAP_STEP]);
            assert_eq!(h.states.lock().unwrap().last(), Some(&StreamState::Dropped));
            assert!(
                h.refusals.lock().unwrap().is_empty(),
                "the listener asked for something else"
            );
        }

        #[test]
        fn a_stream_that_cant_seek_gives_up_at_once() {
            testing::clear_naps();
            let said = Arc::new(Mutex::new(Vec::new()));
            let refusals = Arc::clone(&said);
            let opener: Opener = Arc::new(|_| Err("unused".into()));

            let mut src =
                PluginSource::new(Box::new(Failing), None, false).with_recovery(Recovery {
                    stream: stream(),
                    opener,
                    on_stream: crate::shared::no_stream(),
                    on_refusal: Arc::new(move |reason| said.lock().unwrap().push(reason)),
                    on_buffer: crate::shared::no_buffer(),
                    interrupt: Arc::new(AtomicBool::new(false)),
                });

            assert!(src.read(&mut [0; 16]).is_err());
            assert!(testing::naps().is_empty());
            assert_eq!(refusals.lock().unwrap().len(), 1);
        }

        /// Endless generated bytes at a fixed pace; `dies_after` reads per
        /// stream, then every read fails.
        struct Endless {
            reads: Arc<AtomicUsize>,
            dies_after: usize,
        }

        impl ReadAt for Endless {
            fn read_at(&self, offset: u64, len: usize) -> Result<Vec<u8>, String> {
                if self.reads.fetch_add(1, Ordering::Relaxed) >= self.dies_after {
                    return Err("the broadcast dropped".into());
                }

                std::thread::sleep(Duration::from_millis(1));
                let n = len.min(512);

                Ok((0..n as u64).map(|i| ((offset + i) % 251) as u8).collect())
            }
        }

        fn live_stream() -> PluginStream {
            PluginStream {
                live: true,
                ..stream()
            }
        }

        fn endless(dies_after: usize) -> Opened {
            endless_counted(dies_after, Arc::new(AtomicUsize::new(0)))
        }

        fn endless_counted(dies_after: usize, reads: Arc<AtomicUsize>) -> Opened {
            Opened {
                reader: Box::new(Endless { reads, dies_after }),
                hint: "mp3".into(),
                length: None,
                seekable: false,
                buffer_whole: false,
                duration_ms: None,
            }
        }

        fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if ready() {
                    return;
                }

                std::thread::sleep(Duration::from_millis(2));
            }

            panic!("timed out waiting for {what}");
        }

        fn watched() -> (StreamSink, Arc<Mutex<Vec<StreamState>>>) {
            let seen = Arc::new(Mutex::new(Vec::new()));
            let sink = Arc::clone(&seen);

            (
                Arc::new(move |state| sink.lock().unwrap().push(state)),
                seen,
            )
        }

        #[test]
        fn a_paused_live_stream_resumes_from_the_tape() {
            testing::clear_naps();
            let opener: Opener = Arc::new(|_| Ok(endless(usize::MAX)));
            let reads = Arc::new(AtomicUsize::new(0));
            let (mut src, _tape) = open_live(
                &live_stream(),
                &opener,
                endless_counted(usize::MAX, Arc::clone(&reads)),
                crate::shared::no_stream(),
                Arc::new(AtomicBool::new(false)),
                60,
            )
            .expect("it opens");

            let mut first = vec![0u8; 1000];
            src.read_exact(&mut first).unwrap();

            // Paused: nothing decodes, and the feed keeps pulling.
            let before = reads.load(Ordering::Relaxed);
            wait_for("the feed to read on through the pause", || {
                reads.load(Ordering::Relaxed) > before + 10
            });

            let mut after = vec![0u8; 1000];
            src.read_exact(&mut after).unwrap();

            let mut expected = first.clone();
            expected.extend_from_slice(&after);
            assert_eq!(
                expected,
                (0..2000u64).map(|i| (i % 251) as u8).collect::<Vec<_>>(),
                "it picked up exactly where it paused"
            );
        }

        #[test]
        fn a_dropped_live_feed_reconnects_and_marks_a_gap() {
            testing::clear_naps();
            let opens = Arc::new(AtomicUsize::new(0));
            let counted = Arc::clone(&opens);
            let opener: Opener = Arc::new(move |_| {
                counted.fetch_add(1, Ordering::Relaxed);
                Ok(endless(usize::MAX))
            });

            let (watch, seen) = watched();
            let (mut src, tape) = open_live(
                &live_stream(),
                &opener,
                endless(4),
                watch,
                Arc::new(AtomicBool::new(false)),
                60,
            )
            .expect("it opens");

            let mut out = vec![0u8; 8192];
            src.read_exact(&mut out).expect("the audio keeps coming");

            wait_for("the recovery to publish", || {
                seen.lock().unwrap().last() == Some(&StreamState::Live)
            });
            assert_eq!(
                *seen.lock().unwrap(),
                vec![StreamState::Reconnecting, StreamState::Live]
            );
            assert_eq!(
                opens.load(Ordering::Relaxed),
                1,
                "one reopen, at the live edge"
            );
            assert!(tape.marks_rev() > 0, "the join is marked on the tape");
        }

        #[test]
        fn a_live_feed_out_of_attempts_drops() {
            testing::clear_naps();
            let opener: Opener = Arc::new(|_| Err("the plugin is gone".into()));

            let (watch, seen) = watched();
            let (mut src, _tape) = open_live(
                &live_stream(),
                &opener,
                endless(0),
                watch,
                Arc::new(AtomicBool::new(false)),
                60,
            )
            .expect("it opens");

            assert!(
                src.read(&mut [0u8; 64]).is_err(),
                "the failure reaches the decoder"
            );
            wait_for("the feed to give up", || {
                seen.lock().unwrap().last() == Some(&StreamState::Dropped)
            });
            assert_eq!(testing::napped(), Duration::from_secs(1 + 2 + 4));
        }
    }
}
