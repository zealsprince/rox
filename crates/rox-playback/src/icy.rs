//! Shoutcast/Icecast in-band metadata, stripped out before the decoder sees
//! it. With `icy-metaint: N` a station interleaves N bytes of audio, a length
//! byte, and that many sixteen-byte units of text; left in, the blocks decode
//! as garbage frames. A wrapper `Read`, present only when the header was.
//!
//! For a live station this runs on the feed thread, between socket and tape.
//! Titles are only marked there; the decode side publishes each as its cursor
//! reaches it, so a listener minutes behind sees the song they're hearing.
//!
//! The title blocks also mark where songs start and stop in the station's own
//! encoded bytes, so a second tee hands bytes and boundaries to the capture
//! service. It only copies; buffering and writing happen on the other end.

use std::io::Read;
use std::io::Result as IoResult;
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// One `StreamTitle=`, split on " - ". `artist` is empty for a single field,
/// which is common.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IcyTitle {
    pub artist: String,
    pub title: String,
}

/// Where titles go, bound to a queue entry at the open because the reader sits
/// far below anything that knows which one. `Sync` because the reader ends up
/// inside a `MediaSource`.
pub type TitleSink = Arc<dyn Fn(IcyTitle) + Send + Sync>;

/// For analysis passes and local files.
pub fn no_titles() -> TitleSink {
    Arc::new(|_| {})
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureEvent {
    /// Bytes before this belong to the song that ended, bytes after to the next.
    Boundary(IcyTitle),
    /// Batched: a channel send per read would cost more than the read.
    Bytes(Vec<u8>),
    /// The body is gone. Whatever was mid-capture can't be saved.
    End,
}

/// Fires inside a read, so whatever is behind it must be a channel send.
pub type CaptureSink = Arc<dyn Fn(CaptureEvent) + Send + Sync>;

pub fn no_capture() -> CaptureSink {
    Arc::new(|_| {})
}

/// A handful of reads' worth, so the sink fires a few times a second.
const BATCH: usize = 16 * 1024;

/// Written once by the capture service, read once per connection.
static TEE: RwLock<Option<CaptureSink>> = RwLock::new(None);

/// Separate from [`TEE`] and read per read, so flipping it takes effect at the
/// next song without rebuilding the channel.
static ARMED: AtomicBool = AtomicBool::new(false);

/// Nothing reaches it until [`set_capturing`] turns it on.
pub fn tee_to(sink: CaptureSink) {
    if let Ok(mut tee) = TEE.write() {
        *tee = Some(sink);
    }
}

pub fn set_capturing(on: bool) {
    ARMED.store(on, Ordering::Relaxed);
}

fn capturing() -> bool {
    ARMED.load(Ordering::Relaxed)
}

fn installed_tee() -> Option<CaptureSink> {
    TEE.read().ok()?.clone()
}

/// Strips in-band metadata out of `inner`; titles go to the callback as they change.
pub struct IcyReader<R: Read> {
    inner: R,
    metaint: usize,
    /// Zero means the next inner byte is a block length.
    until_meta: usize,
    /// Stations repeat the title every block; this keeps the keepalive from firing.
    last: String,
    on_title: Box<dyn Fn(IcyTitle) + Send + Sync>,
    /// Grabbed at construction: a connection has somewhere to copy to or it doesn't.
    capture: Option<CaptureSink>,
    /// Held until [`BATCH`] or the next boundary.
    batch: Vec<u8>,
}

impl<R: Read> IcyReader<R> {
    pub fn new(
        inner: R,
        metaint: usize,
        on_title: impl Fn(IcyTitle) + Send + Sync + 'static,
    ) -> Self {
        Self::with_capture(inner, metaint, on_title, installed_tee())
    }

    /// With the tee named outright, for tests: `new` reads a process global.
    pub fn with_capture(
        inner: R,
        metaint: usize,
        on_title: impl Fn(IcyTitle) + Send + Sync + 'static,
        capture: Option<CaptureSink>,
    ) -> Self {
        Self {
            inner,
            metaint,
            until_meta: metaint,
            last: String::new(),
            on_title: Box::new(on_title),
            capture,
            batch: Vec::new(),
        }
    }

    /// Also called on every boundary, so a batch never straddles two songs.
    fn flush_batch(&mut self) {
        if self.batch.is_empty() {
            return;
        }

        let Some(capture) = self.capture.clone() else {
            self.batch.clear();
            return;
        };

        capture(CaptureEvent::Bytes(std::mem::take(&mut self.batch)));
    }

    /// Read exactly `buf.len()`, false at end of stream. A block can split across
    /// any number of inner reads.
    fn fill(&mut self, buf: &mut [u8]) -> IoResult<bool> {
        let mut got = 0;
        while got < buf.len() {
            let n = self.inner.read(&mut buf[got..])?;
            if n == 0 {
                return Ok(false);
            }
            got += n;
        }
        Ok(true)
    }

    /// False means the stream ended inside the block.
    fn consume_meta(&mut self) -> IoResult<bool> {
        let mut len = [0u8; 1];
        if !self.fill(&mut len)? {
            return Ok(false);
        }

        // The keepalive between title changes.
        let bytes = len[0] as usize * 16;
        if bytes == 0 {
            self.until_meta = self.metaint;
            return Ok(true);
        }

        let mut block = vec![0u8; bytes];
        if !self.fill(&mut block)? {
            return Ok(false);
        }

        // A block that won't parse never stops playback.
        if let Some(title) = parse_title(&block)
            && title != self.last
        {
            self.last = title.clone();
            let split = split_title(&title);
            (self.on_title)(split.clone());

            // The bytes so far are the last song's; they go out before its boundary.
            if self.capture.is_some() && capturing() {
                self.flush_batch();
                if let Some(capture) = self.capture.clone() {
                    capture(CaptureEvent::Boundary(split));
                }
            }
        }

        self.until_meta = self.metaint;
        Ok(true)
    }
}

impl<R: Read> Read for IcyReader<R> {
    fn read(&mut self, out: &mut [u8]) -> IoResult<usize> {
        if out.is_empty() {
            return Ok(0);
        }

        // Returning zero here would read as end of stream upstream.
        if self.until_meta == 0 && !self.consume_meta()? {
            return Ok(0);
        }

        // Never read past the next boundary; short reads are legal.
        let want = out.len().min(self.until_meta);
        let n = self.inner.read(&mut out[..want])?;
        self.until_meta -= n;

        if n > 0 && self.capture.is_some() && capturing() {
            self.batch.extend_from_slice(&out[..n]);
            if self.batch.len() >= BATCH {
                self.flush_batch();
            }
        }

        Ok(n)
    }
}

/// Dropping the reader is how a connection ends, so the song being read was
/// cut short: the held bytes go nowhere and the tee hears `End`.
impl<R: Read> Drop for IcyReader<R> {
    fn drop(&mut self) {
        let Some(capture) = self.capture.clone() else {
            return;
        };

        capture(CaptureEvent::End);
    }
}

/// `StreamTitle` from one NUL-padded block of `key='value';` pairs.
fn parse_title(block: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(block);
    let rest = text.split_once("StreamTitle='")?.1;

    // Ends at `';`, not the first quote: "Rock 'n' Roll" is an ordinary title.
    let value = match rest.find("';") {
        Some(end) => &rest[..end],
        None => rest.trim_end_matches('\0').trim_end_matches('\''),
    };

    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// Artist first by convention; no separator means title only.
fn split_title(value: &str) -> IcyTitle {
    match value.split_once(" - ") {
        Some((artist, title)) => IcyTitle {
            artist: artist.trim().to_string(),
            title: title.trim().to_string(),
        },

        None => IcyTitle {
            artist: String::new(),
            title: value.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::Mutex;

    fn meta(text: &str) -> Vec<u8> {
        let mut bytes = text.as_bytes().to_vec();
        while !bytes.len().is_multiple_of(16) {
            bytes.push(0);
        }
        let mut out = vec![(bytes.len() / 16) as u8];
        out.extend_from_slice(&bytes);
        out
    }

    struct Choppy {
        data: Vec<u8>,
        at: usize,
        chunk: usize,
    }

    impl Read for Choppy {
        fn read(&mut self, out: &mut [u8]) -> IoResult<usize> {
            let n = out.len().min(self.chunk).min(self.data.len() - self.at);
            out[..n].copy_from_slice(&self.data[self.at..self.at + n]);
            self.at += n;
            Ok(n)
        }
    }

    /// The arming switch is process-global, so the tee's tests take turns.
    static ARM: Mutex<()> = Mutex::new(());

    /// Every capture event over `data`, tee armed; `End` always comes last.
    fn tee(data: Vec<u8>, metaint: usize) -> Vec<CaptureEvent> {
        let _held = ARM.lock().unwrap_or_else(|e| e.into_inner());
        set_capturing(true);

        let seen: Arc<Mutex<Vec<CaptureEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let capture: CaptureSink = Arc::new(move |e| sink.lock().unwrap().push(e));

        {
            let src = Choppy {
                data,
                at: 0,
                chunk: 1024,
            };
            let mut reader = IcyReader::with_capture(src, metaint, |_| {}, Some(capture));
            let mut buf = [0u8; 7];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
            }
        }

        set_capturing(false);

        let events = seen.lock().unwrap();
        events.clone()
    }

    fn drain(data: Vec<u8>, metaint: usize, chunk: usize) -> (Vec<u8>, Vec<IcyTitle>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let src = Choppy { data, at: 0, chunk };
        let mut reader = IcyReader::new(src, metaint, move |t| sink.lock().unwrap().push(t));

        let mut out = Vec::new();
        let mut buf = [0u8; 7];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => out.extend_from_slice(&buf[..n]),
                Err(e) => panic!("read failed: {e}"),
            }
        }
        let titles = seen.lock().unwrap().clone();

        (out, titles)
    }

    #[test]
    fn metadata_never_reaches_the_caller() {
        let audio: Vec<u8> = (0..64u8).collect();
        let mut stream = Vec::new();
        stream.extend_from_slice(&audio[..32]);
        stream.extend_from_slice(&meta("StreamTitle='Boards of Canada - Roygbiv';"));
        stream.extend_from_slice(&audio[32..]);
        stream.extend_from_slice(&meta(""));

        let (out, _) = drain(stream, 32, 1024);
        assert_eq!(out, audio);
    }

    #[test]
    fn a_title_change_fires_once() {
        let mut stream = vec![0u8; 16];
        stream.extend_from_slice(&meta("StreamTitle='Aphex Twin - Xtal';"));
        stream.extend_from_slice(&[0u8; 16]);
        // The repeat is a keepalive, not a new song.
        stream.extend_from_slice(&meta("StreamTitle='Aphex Twin - Xtal';"));
        stream.extend_from_slice(&[0u8; 16]);
        stream.extend_from_slice(&meta("StreamTitle='Autechre - Rae';"));
        stream.extend_from_slice(&[0u8; 16]);

        let (out, titles) = drain(stream, 16, 1024);
        assert_eq!(out.len(), 64);
        assert_eq!(
            titles,
            vec![
                IcyTitle {
                    artist: "Aphex Twin".into(),
                    title: "Xtal".into(),
                },
                IcyTitle {
                    artist: "Autechre".into(),
                    title: "Rae".into(),
                },
            ]
        );
    }

    #[test]
    fn an_empty_block_is_skipped() {
        let mut stream = vec![1u8; 8];
        stream.push(0);
        stream.extend_from_slice(&[2u8; 8]);
        stream.push(0);

        let (out, titles) = drain(stream, 8, 1024);
        assert_eq!(out, [vec![1u8; 8], vec![2u8; 8]].concat());
        assert!(titles.is_empty());
    }

    #[test]
    fn a_title_with_no_separator_is_all_title() {
        let mut stream = vec![0u8; 8];
        stream.extend_from_slice(&meta("StreamTitle='NTS Radio 1';StreamUrl='';"));
        stream.extend_from_slice(&[0u8; 8]);

        let (_, titles) = drain(stream, 8, 1024);
        assert_eq!(
            titles,
            vec![IcyTitle {
                artist: String::new(),
                title: "NTS Radio 1".into(),
            }]
        );
    }

    #[test]
    fn a_block_split_across_reads_still_parses() {
        let mut stream = vec![9u8; 8];
        stream.extend_from_slice(&meta("StreamTitle='Burial - Archangel';"));
        stream.extend_from_slice(&[9u8; 8]);

        let (out, titles) = drain(stream, 8, 3);
        assert_eq!(out, vec![9u8; 16]);
        assert_eq!(
            titles,
            vec![IcyTitle {
                artist: "Burial".into(),
                title: "Archangel".into(),
            }]
        );
    }

    #[test]
    fn an_apostrophe_in_a_title_survives() {
        let mut stream = vec![0u8; 8];
        stream.extend_from_slice(&meta("StreamTitle='Guns N' Roses - Sweet Child O' Mine';"));
        stream.extend_from_slice(&[0u8; 8]);

        let (_, titles) = drain(stream, 8, 1024);
        assert_eq!(
            titles,
            vec![IcyTitle {
                artist: "Guns N' Roses".into(),
                title: "Sweet Child O' Mine".into(),
            }]
        );
    }

    #[test]
    fn the_tee_splits_the_bytes_at_the_title_changes() {
        let mut stream = vec![1u8; 8];
        stream.extend_from_slice(&meta("StreamTitle='Aphex Twin - Xtal';"));
        stream.extend_from_slice(&[2u8; 8]);
        stream.extend_from_slice(&meta("StreamTitle='Autechre - Rae';"));
        stream.extend_from_slice(&[3u8; 8]);
        stream.push(0);

        assert_eq!(
            tee(stream, 8),
            vec![
                // Bytes before the first block have no boundary in front of them: the tail
                // of whatever was on at connect.
                CaptureEvent::Bytes(vec![1u8; 8]),
                CaptureEvent::Boundary(IcyTitle {
                    artist: "Aphex Twin".into(),
                    title: "Xtal".into(),
                }),
                CaptureEvent::Bytes(vec![2u8; 8]),
                CaptureEvent::Boundary(IcyTitle {
                    artist: "Autechre".into(),
                    title: "Rae".into(),
                }),
                // The last song never ended, so there's nothing to save.
                CaptureEvent::End,
            ]
        );
    }

    #[test]
    fn a_repeated_title_is_no_boundary() {
        let mut stream = vec![1u8; 8];
        stream.extend_from_slice(&meta("StreamTitle='Burial - Archangel';"));
        stream.extend_from_slice(&[2u8; 8]);
        stream.extend_from_slice(&meta("StreamTitle='Burial - Archangel';"));
        stream.extend_from_slice(&[3u8; 8]);
        stream.push(0);

        let boundaries = tee(stream, 8)
            .into_iter()
            .filter(|e| matches!(e, CaptureEvent::Boundary(_)))
            .count();

        assert_eq!(boundaries, 1, "the keepalive is not a song change");
    }

    #[test]
    fn the_tee_stays_quiet_while_capture_is_off() {
        let _held = ARM.lock().unwrap_or_else(|e| e.into_inner());
        set_capturing(false);

        let seen: Arc<Mutex<Vec<CaptureEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let capture: CaptureSink = Arc::new(move |e| sink.lock().unwrap().push(e));

        let mut stream = vec![1u8; 8];
        stream.extend_from_slice(&meta("StreamTitle='Aphex Twin - Xtal';"));
        stream.extend_from_slice(&[2u8; 8]);
        stream.push(0);

        {
            let src = Choppy {
                data: stream,
                at: 0,
                chunk: 1024,
            };
            let mut reader = IcyReader::with_capture(src, 8, |_| {}, Some(capture));
            let mut buf = [0u8; 7];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
            }
        }

        // Only the drop is heard.
        assert_eq!(*seen.lock().unwrap(), vec![CaptureEvent::End]);
    }
}
