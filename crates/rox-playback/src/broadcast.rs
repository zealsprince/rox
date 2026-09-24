//! The icecast broadcast sink (ADR 22): rox connects out as a source client
//! and pushes the processed stream as MP3. rox owns no HTTP surface; icecast
//! owns the mount and the listeners.
//!
//! Fed on the decode thread after the chain (ADR 19), so the broadcast is what
//! the speakers get before volume. The feed never blocks: chunks cross a
//! bounded channel, and when the sink can't keep up they're dropped.
//!
//! Not done: silence while paused. A paused deck starves the stream.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use base64::Engine as _;

/// The playback-side copy of the settings shape. `None` in [`configure`] turns it off.
#[derive(Clone, PartialEq)]
pub struct Config {
    /// Host and port, no scheme: the source protocol is a plain socket.
    pub host: String,
    pub port: u16,
    /// Leading slash optional.
    pub mount: String,
    pub user: String,
    pub password: String,
    pub name: String,
    /// kbps, folded onto the nearest step LAME takes.
    pub bitrate: u32,
}

/// Chunks queued before the feed drops them: well over the decoder's ring of
/// lead, bounded memory against a dead server.
const FEED_BUFFER: usize = 64;

const RETRY: Duration = Duration::from_secs(5);

/// A stalled server fails the write instead of parking the sink.
const IO_TIMEOUT: Duration = Duration::from_secs(10);

/// Off is one relaxed load per chunk.
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// RwLock: the decode thread read-locks per chunk; only [`configure`] writes.
static FEED: RwLock<Option<SyncSender<Chunk>>> = RwLock::new(None);

/// Lets a reconfigure end the thread mid-retry.
static STOP: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);

/// "artist - title" plus a dirty flag, so a burst of track changes folds to one update.
static SONG: Mutex<Option<String>> = Mutex::new(None);
static SONG_DIRTY: AtomicBool = AtomicBool::new(false);

/// Carries its rate because a device rebuild changes it.
struct Chunk {
    rate: u32,
    samples: Vec<f32>,
}

/// From the decode thread. Never blocks: a full channel drops the chunk.
/// Playback never waits on the stream.
pub fn feed(samples: &[f32], rate: u32) {
    if !ACTIVE.load(Ordering::Relaxed) || samples.is_empty() {
        return;
    }
    let Ok(feed) = FEED.read() else { return };
    let Some(tx) = feed.as_ref() else { return };
    match tx.try_send(Chunk {
        rate,
        samples: samples.to_vec(),
    }) {
        Ok(()) | Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {}
    }
}

/// Pushed from the sink thread once connected, or announced on the next connect.
pub fn set_song(song: String) {
    *SONG.lock().unwrap() = Some(song);
    SONG_DIRTY.store(true, Ordering::Release);
}

/// Start broadcasting, or stop with `None`. A reconfigure tears the old sink
/// down first, which releases the mount.
pub fn configure(config: Option<Config>) {
    ACTIVE.store(false, Ordering::Relaxed);
    if let Some(stop) = STOP.lock().unwrap().take() {
        stop.store(true, Ordering::Relaxed);
    }
    *FEED.write().unwrap() = None;

    let Some(config) = config else { return };
    if config.host.trim().is_empty() {
        log::warn!("broadcast: configured without a host, staying off");
        return;
    }
    let (tx, rx) = std::sync::mpsc::sync_channel(FEED_BUFFER);
    let stop = Arc::new(AtomicBool::new(false));
    *STOP.lock().unwrap() = Some(stop.clone());
    *FEED.write().unwrap() = Some(tx);
    ACTIVE.store(true, Ordering::Relaxed);
    SONG_DIRTY.store(true, Ordering::Release);
    std::thread::spawn(move || sink(config, rx, stop));
}

/// Connect, encode, push; on failure drop the connection and backlog and retry
/// after a pause, until stopped.
fn sink(config: Config, rx: Receiver<Chunk>, stop: Arc<AtomicBool>) {
    let mount = normalized_mount(&config.mount);
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        // Encoder per connection at the first chunk's rate, so a device rate change
        // reconnects instead of mixing rates.
        match serve_connection(&config, &mount, &rx, &stop) {
            Served::Stopped => return,
            Served::Failed(err) => {
                log::warn!("broadcast: {err}; retrying in {}s", RETRY.as_secs());
            }
        }
        // Drop the stale backlog so the stream resumes at now.
        while rx.try_recv().is_ok() {}
        let waited = std::time::Instant::now();
        while waited.elapsed() < RETRY {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
}

enum Served {
    Stopped,
    Failed(String),
}

fn serve_connection(
    config: &Config,
    mount: &str,
    rx: &Receiver<Chunk>,
    stop: &Arc<AtomicBool>,
) -> Served {
    // Don't open a connection before there's audio to send.
    let first = loop {
        if stop.load(Ordering::Relaxed) {
            return Served::Stopped;
        }
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(chunk) => break chunk,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return Served::Stopped,
        }
    };

    let mut encoder = match Encoder::new(first.rate, config.bitrate) {
        Ok(encoder) => encoder,
        Err(err) => return Served::Failed(format!("encoder: {err}")),
    };
    let mut stream = match connect(config, mount) {
        Ok(stream) => stream,
        Err(err) => return Served::Failed(err),
    };
    log::info!(
        "broadcast: streaming to {}:{}{} at {} kbps",
        config.host,
        config.port,
        mount,
        encoder.bitrate
    );

    let mut chunk = Some(first);
    loop {
        if stop.load(Ordering::Relaxed) {
            return Served::Stopped;
        }
        if SONG_DIRTY.swap(false, Ordering::AcqRel)
            && let Some(song) = SONG.lock().unwrap().clone()
            && let Err(err) = push_metadata(config, mount, &song)
        {
            // Metadata failures never cost the stream.
            log::debug!("broadcast: metadata update failed: {err}");
        }
        let Some(current) = chunk.take() else {
            match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(next) => chunk = Some(next),
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return Served::Stopped,
            }
            continue;
        };
        if current.rate != encoder.rate {
            return Served::Failed(format!(
                "device rate moved {} -> {}, reopening the stream",
                encoder.rate, current.rate
            ));
        }
        let bytes = match encoder.encode(&current.samples) {
            Ok(bytes) => bytes,
            Err(err) => return Served::Failed(format!("encoder: {err}")),
        };
        if let Err(err) = stream.write_all(bytes) {
            return Served::Failed(format!("connection lost: {err}"));
        }
    }
}

fn normalized_mount(mount: &str) -> String {
    let trimmed = mount.trim().trim_start_matches('/');
    if trimmed.is_empty() {
        "/rox".to_string()
    } else {
        format!("/{trimmed}")
    }
}

/// PUT at the mount with Basic auth and ice headers; anything but a 200 is an error.
fn connect(config: &Config, mount: &str) -> Result<TcpStream, String> {
    let addr = (config.host.as_str(), config.port);
    let stream = TcpStream::connect(addr).map_err(|e| format!("{}: {e}", config.host))?;
    stream.set_write_timeout(Some(IO_TIMEOUT)).ok();
    stream.set_read_timeout(Some(IO_TIMEOUT)).ok();
    stream.set_nodelay(true).ok();

    let auth = base64::engine::general_purpose::STANDARD
        .encode(format!("{}:{}", config.user, config.password));
    let mut request = format!(
        "PUT {mount} HTTP/1.1\r\n\
         Host: {}:{}\r\n\
         Authorization: Basic {auth}\r\n\
         User-Agent: rox/{}\r\n\
         Accept: */*\r\n\
         Content-Type: audio/mpeg\r\n\
         Ice-Public: 0\r\n\
         Ice-Audio-Info: bitrate={}\r\n\
         Expect: 100-continue\r\n",
        config.host,
        config.port,
        env!("CARGO_PKG_VERSION"),
        config.bitrate,
    );
    if !config.name.trim().is_empty() {
        request.push_str(&format!("Ice-Name: {}\r\n", config.name.trim()));
    }
    request.push_str("\r\n");
    let mut stream = stream;
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("handshake: {e}"))?;

    // A 100 first when the Expect was honored, then the 200.
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| format!("handshake: {e}"))?);
    for _ in 0..2 {
        let status = read_response(&mut reader)?;
        if status.contains(" 100 ") {
            continue;
        }
        if status.contains(" 200 ") {
            return Ok(stream);
        }
        return Err(format!("server refused the source: {}", status.trim()));
    }
    Err("server never accepted the source".into())
}

fn read_response(reader: &mut BufReader<TcpStream>) -> Result<String, String> {
    let mut status = String::new();
    reader
        .read_line(&mut status)
        .map_err(|e| format!("handshake: {e}"))?;
    if status.is_empty() {
        return Err("server closed the connection mid-handshake".into());
    }
    loop {
        let mut line = String::new();
        let read = reader
            .read_line(&mut line)
            .map_err(|e| format!("handshake: {e}"))?;
        if read == 0 || line == "\r\n" || line == "\n" {
            return Ok(status);
        }
    }
}

/// The admin updinfo call, under the same source credentials.
fn push_metadata(config: &Config, mount: &str, song: &str) -> Result<(), String> {
    let addr = (config.host.as_str(), config.port);
    let mut stream = TcpStream::connect(addr).map_err(|e| e.to_string())?;
    stream.set_write_timeout(Some(IO_TIMEOUT)).ok();
    stream.set_read_timeout(Some(IO_TIMEOUT)).ok();
    let auth = base64::engine::general_purpose::STANDARD
        .encode(format!("{}:{}", config.user, config.password));
    let request = format!(
        "GET /admin/metadata?mode=updinfo&mount={}&song={} HTTP/1.0\r\n\
         Host: {}:{}\r\n\
         Authorization: Basic {auth}\r\n\
         User-Agent: rox/{}\r\n\r\n",
        percent_encode(mount),
        percent_encode(song),
        config.host,
        config.port,
        env!("CARGO_PKG_VERSION"),
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| e.to_string())?;
    // Drain so the server sees a clean close.
    let mut sink = Vec::new();
    let _ = stream.read_to_end(&mut sink);
    Ok(())
}

/// Keeps an ampersand in a title from splitting the value.
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

struct Encoder {
    encoder: mp3lame_encoder::Encoder,
    rate: u32,
    bitrate: u32,
    /// Reused across chunks so the steady state doesn't allocate.
    pcm: Vec<i16>,
    out: Vec<u8>,
}

impl Encoder {
    fn new(rate: u32, bitrate: u32) -> Result<Encoder, String> {
        let mut builder = mp3lame_encoder::Builder::new().ok_or("out of memory")?;
        builder.set_num_channels(2).map_err(|e| e.to_string())?;
        builder.set_sample_rate(rate).map_err(|e| e.to_string())?;
        let bitrate = nearest_bitrate(bitrate);
        builder.set_brate(bitrate.0).map_err(|e| e.to_string())?;
        builder
            .set_quality(mp3lame_encoder::Quality::Good)
            .map_err(|e| e.to_string())?;
        Ok(Encoder {
            encoder: builder.build().map_err(|e| e.to_string())?,
            rate,
            bitrate: bitrate.1,
            pcm: Vec::new(),
            out: Vec::new(),
        })
    }

    /// The returned slice lives until the next call.
    fn encode(&mut self, samples: &[f32]) -> Result<&[u8], String> {
        self.pcm.clear();
        self.pcm.extend(
            samples
                .iter()
                .map(|&s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16),
        );
        self.out.clear();
        let need = mp3lame_encoder::max_required_buffer_size(self.pcm.len() / 2);
        self.out.reserve(need);
        let input = mp3lame_encoder::InterleavedPcm(self.pcm.as_slice());
        let written = self
            .encoder
            .encode(input, self.out.spare_capacity_mut())
            .map_err(|e| e.to_string())?;
        // encode wrote `written` bytes into the reserved spare capacity.
        unsafe { self.out.set_len(written) };
        Ok(&self.out)
    }
}

/// With the resolved kbps for the log line. 0 means the 192 default.
fn nearest_bitrate(kbps: u32) -> (mp3lame_encoder::Bitrate, u32) {
    use mp3lame_encoder::Bitrate;
    let steps: [(Bitrate, u32); 8] = [
        (Bitrate::Kbps96, 96),
        (Bitrate::Kbps112, 112),
        (Bitrate::Kbps128, 128),
        (Bitrate::Kbps160, 160),
        (Bitrate::Kbps192, 192),
        (Bitrate::Kbps224, 224),
        (Bitrate::Kbps256, 256),
        (Bitrate::Kbps320, 320),
    ];
    let want = if kbps == 0 { 192 } else { kbps };
    steps
        .into_iter()
        .min_by_key(|&(_, step)| step.abs_diff(want))
        .expect("the step table is not empty")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feed_never_blocks_however_dead_the_sink() {
        configure(Some(Config {
            host: "127.0.0.1".into(),
            // Nothing listens here, so the sink retries forever while the feed overflows.
            port: 1,
            mount: "/test".into(),
            user: "source".into(),
            password: "hackme".into(),
            name: String::new(),
            bitrate: 192,
        }));
        let chunk = vec![0.0f32; 4096];
        // If a full buffer blocked the feed, this would hang.
        for _ in 0..FEED_BUFFER * 4 {
            feed(&chunk, 48_000);
        }
        configure(None);
        assert!(!ACTIVE.load(Ordering::Relaxed));
        feed(&chunk, 48_000);
    }

    #[test]
    fn mounts_normalize_and_queries_escape() {
        assert_eq!(normalized_mount("live"), "/live");
        assert_eq!(normalized_mount("/live"), "/live");
        assert_eq!(normalized_mount("  "), "/rox");
        assert_eq!(percent_encode("a b&c"), "a%20b%26c");
        assert_eq!(percent_encode("/live"), "/live");
    }

    #[test]
    fn bitrates_snap_to_lame_steps() {
        assert_eq!(nearest_bitrate(0).1, 192);
        assert_eq!(nearest_bitrate(190).1, 192);
        assert_eq!(nearest_bitrate(64).1, 96);
        assert_eq!(nearest_bitrate(999).1, 320);
    }
}
