//! The icecast broadcast sink (ADR 22): rox connects out to an icecast
//! server as a source client and pushes the processed stream, encoded to
//! MP3, at the mount the config names. Everything downstream (the mount,
//! the listeners, the network face) belongs to icecast; rox owns no HTTP
//! surface, it only speaks the source protocol out of one thread here.
//!
//! The engine feeds this module on the decode thread, right after the
//! chain, so the broadcast carries exactly what the speakers get (ADR 19).
//! The feed never blocks and never waits on the network: chunks cross a
//! bounded channel to the sink thread, and when the sink can't keep up
//! (server unreachable, socket stalled) chunks drop on the floor and local
//! playback never notices. The sink reconnects on its own clock for as
//! long as the config stands, and tearing the config down closes the
//! connection, which releases the mount.
//!
//! What this doesn't do yet: synthesize silence while rox is
//! paused. A paused deck starves the stream and listeners stall on their
//! buffer; icecast keeps the mount either way, and audio resumes with
//! playback.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use base64::Engine as _;

/// Where and how to broadcast, the playback-side copy of the settings
/// shape. `None` in [`configure`] is the off switch.
#[derive(Clone, PartialEq)]
pub struct Config {
    /// The icecast server, host and port; no scheme, since the source
    /// protocol runs over a plain socket.
    pub host: String,
    pub port: u16,
    /// The mount listeners tune to, with or without its leading slash.
    pub mount: String,
    /// Source credentials, icecast.xml's source user and password.
    pub user: String,
    pub password: String,
    /// The stream name the mount advertises. Empty stays nameless.
    pub name: String,
    /// Encoder bitrate in kbps, folded onto the nearest step LAME takes.
    pub bitrate: u32,
}

/// How many chunks may queue for the sink before the feed drops them. A
/// chunk is one decode batch, a few hundred milliseconds at most, so this
/// holds well over the ring's worth of lead the decoder runs at while an
/// unreachable server costs bounded memory and zero waiting.
const FEED_BUFFER: usize = 64;

/// How long the sink waits between connection attempts.
const RETRY: Duration = Duration::from_secs(5);

/// Socket timeouts. A server that stops draining fails the write instead
/// of parking the sink forever; the read timeout paces the shutdown check.
const IO_TIMEOUT: Duration = Duration::from_secs(10);

/// The engine's cheap gate: false is one relaxed load per chunk and
/// nothing else, so a build with broadcast unconfigured pays nothing.
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// The feed half of the running sink's channel. RwLock because the decode
/// thread read-locks per chunk and only [`configure`] ever writes.
static FEED: RwLock<Option<SyncSender<Chunk>>> = RwLock::new(None);

/// The running sink's stop flag, so a reconfigure or teardown can end the
/// thread mid-retry rather than waiting a whole backoff out.
static STOP: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);

/// The stream metadata as "artist - title", plus the flag that tells the
/// sink it changed. Shared state rather than a channel message so a burst
/// of track changes folds to one update, the latest.
static SONG: Mutex<Option<String>> = Mutex::new(None);
static SONG_DIRTY: AtomicBool = AtomicBool::new(false);

/// One decode batch as the engine hands it over: interleaved stereo f32 at
/// the device rate it was processed at. The rate is included because a
/// device rebuild changes it, and the encoder has to follow.
struct Chunk {
    rate: u32,
    samples: Vec<f32>,
}

/// Hand one processed batch to the sink, from the decode thread. Never
/// blocks: with broadcast off this is one atomic load, and with the sink
/// behind it drops the chunk, because the stream skipping is the acceptable
/// cost and playback waiting is not.
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

/// Update the stream metadata on the next track. The sink pushes it to
/// icecast's admin endpoint from its own thread once connected; with the
/// sink down it just becomes the state the next connection announces.
pub fn set_song(song: String) {
    *SONG.lock().unwrap() = Some(song);
    SONG_DIRTY.store(true, Ordering::Release);
}

/// Start broadcasting with `config`, or stop with `None`. A reconfigure
/// tears the old sink down first, which closes its connection and releases
/// the mount; the new one connects on its own thread and keeps retrying
/// for as long as the config stands.
pub fn configure(config: Option<Config>) {
    // End the running sink: gate the feed off, wake the thread out of
    // whatever retry sleep it's in, and drop its channel.
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

/// The sink thread's whole life: connect, announce, encode and push chunks,
/// and on any failure drop the connection, flush the backlog, and try again
/// after a pause. Ends when the stop flag is set.
fn sink(config: Config, rx: Receiver<Chunk>, stop: Arc<AtomicBool>) {
    let mount = normalized_mount(&config.mount);
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        // The encoder is created per connection, at the rate of the first
        // chunk through, so a device rebuild mid-broadcast reconnects
        // rather than feeding one stream two rates.
        match serve_connection(&config, &mount, &rx, &stop) {
            Served::Stopped => return,
            Served::Failed(err) => {
                log::warn!("broadcast: {err}; retrying in {}s", RETRY.as_secs());
            }
        }
        // The backlog encoded for a dead connection is stale the moment a
        // new one opens; drop it so the stream resumes at now.
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

/// Why one connection's serve loop ended.
enum Served {
    /// The stop flag was set; the thread is done.
    Stopped,
    /// The connection or the encoder failed; the caller retries.
    Failed(String),
}

/// One connection: wait for audio, open the encoder at its rate, shake
/// hands with icecast, then pump until something gives.
fn serve_connection(
    config: &Config,
    mount: &str,
    rx: &Receiver<Chunk>,
    stop: &Arc<AtomicBool>,
) -> Served {
    // Nothing to broadcast until the deck moves; don't hold a silent
    // connection open before the first chunk ever arrives.
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
        if SONG_DIRTY.swap(false, Ordering::AcqRel) {
            if let Some(song) = SONG.lock().unwrap().clone() {
                if let Err(err) = push_metadata(config, mount, &song) {
                    // Metadata is decoration; a failed update never costs
                    // the stream itself.
                    log::debug!("broadcast: metadata update failed: {err}");
                }
            }
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

/// A mount with exactly one leading slash, whatever the config held.
fn normalized_mount(mount: &str) -> String {
    let trimmed = mount.trim().trim_start_matches('/');
    if trimmed.is_empty() {
        "/rox".to_string()
    } else {
        format!("/{trimmed}")
    }
}

/// Open the source connection: a PUT at the mount with Basic auth and the
/// ice headers, answered by icecast before any audio flows. Anything but
/// acceptance is an error sentence for the retry log.
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

    // icecast answers the headers before the body flows: a 100 first when
    // the Expect was honored, then the 200 that accepts the source.
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

/// One HTTP response off the wire: the status line kept, the headers read
/// through to the blank line and dropped.
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

/// Tell icecast what's playing: the admin updinfo call, one short-lived
/// connection under the same source credentials, which icecast honors for
/// the source's own mount.
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
    // Drain the response so the server sees a clean close; its contents
    // don't change anything on our side.
    let mut sink = Vec::new();
    let _ = stream.read_to_end(&mut sink);
    Ok(())
}

/// Query-string percent encoding, the unreserved set kept and everything
/// else escaped, so a song title with an ampersand in it stays one value.
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

/// LAME behind the two calls the sink makes: created at a rate and bitrate,
/// fed interleaved stereo f32, handing back MP3 bytes from its own buffer.
struct Encoder {
    encoder: mp3lame_encoder::Encoder,
    rate: u32,
    bitrate: u32,
    /// Scratch for the f32 -> i16 conversion and the encoded output,
    /// reused across chunks so the steady state doesn't allocate.
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

    /// One chunk of interleaved stereo f32 in, the encoded bytes out. The
    /// returned slice points into this encoder's scratch until the next call.
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

/// The nearest bitrate step LAME takes, with what it resolved to for the
/// log line. 0 (an unconfigured field) resolves to the 192 default.
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
            // A port nothing listens on: the sink retries forever while
            // the feed keeps pushing into (and overflowing) the buffer.
            port: 1,
            mount: "/test".into(),
            user: "source".into(),
            password: "hackme".into(),
            name: String::new(),
            bitrate: 192,
        }));
        let chunk = vec![0.0f32; 4096];
        // Far past FEED_BUFFER: if a full buffer blocked the feed, this
        // test would hang instead of finishing.
        for _ in 0..FEED_BUFFER * 4 {
            feed(&chunk, 48_000);
        }
        configure(None);
        // Torn down, the gate is closed and feeding is a no-op.
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
