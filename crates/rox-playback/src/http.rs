//! A [`MediaSource`] over HTTP, so the decoder doesn't care whether bytes come
//! off a disk or a socket. A seek is a fresh ranged GET; a window of recent
//! bytes makes the probe's small backward seeks free.
//!
//! A station isn't [`HttpSource`]: it has no length, ignores ranges, and drops
//! or throttles a client that stops reading. A live open puts a feed thread on
//! the socket and a tape behind it ([`crate::tape`]), and the decoder reads a
//! [`LiveSource`] at its own cursor. The feed drains whether or not anything
//! decodes, which makes a pause resumable and the last minutes seekable.
//!
//! The feed thread owns reconnects. A clean `Ok(0)` is a drop too, since a
//! broadcast never ends. Retries honour the session's interrupt flag, and a
//! reconnect records a gap in the tape, since two connections' bytes don't
//! decode as one stream.
//!
//! No `close`: dropping the reader drops the tape, and the feed thread holds
//! only a [`Weak`] to it.
//!
//! Opens on the decode thread, never the audio callback. The feed thread never
//! touches the ring or the callback's atomics.
//!
//! Layering: wire calls belong in `rox-net`, but this is a byte transport the
//! decode loop pulls synchronously. ADR 29 records the exception.

use std::io;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::Weak;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use rox_library::locator::Remote;
use symphonia::core::io::MediaSource;

use crate::icy::IcyReader;
use crate::icy::TitleSink;
use crate::shared::StreamSink;
use crate::shared::StreamState;
use crate::tape::Feed;
use crate::tape::Snap;
use crate::tape::Tape;
use crate::tape::TapeReader;

/// A megabyte, sized off symphonia's probe depth: a big ID3v2 tag pushes the
/// container header that far, and the probe walks back over it.
const WINDOW: usize = 1 << 20;

/// Waits before each reconnect. The first is free (servers cycle connections),
/// then doubling, so rate limiters aren't hammered. Seven seconds total rides
/// out a mount restart without holding the queue long.
const BACKOFF: [Duration; 4] = [
    Duration::from_secs(0),
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
];

/// How long the feed can keep retrying after a press.
const NAP_STEP: Duration = Duration::from_millis(250);

/// Wait out a backoff in steps, true if a command arrived. Relaxed: the flag
/// only hints at the channel, which synchronises itself. Tests record the
/// steps instead of sleeping.
fn nap(wait: Duration, interrupt: &AtomicBool) -> bool {
    let mut left = wait;
    while !left.is_zero() {
        if interrupt.load(Ordering::Relaxed) {
            return true;
        }

        let step = left.min(NAP_STEP);
        left -= step;

        #[cfg(test)]
        testing::record_nap(step);

        #[cfg(not(test))]
        std::thread::sleep(step);
    }

    interrupt.load(Ordering::Relaxed)
}

/// Icecast operators read their logs; give them something to name.
const USER_AGENT: &str = concat!(
    "rox/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/zealsprince/rox)"
);

/// What a station says about itself in its `icy-` headers: the only
/// description outside a directory. All-empty rather than absent when a
/// station sends none, since "connected, told us nothing" differs from "not
/// connected".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StationInfo {
    pub name: String,
    /// `icy-genre`: usually one word, sometimes a comma list.
    pub genre: String,
    /// `icy-br`, kbps. Zero when unstated.
    pub bitrate_kbps: u32,
    /// `icy-url`: the station's homepage.
    pub homepage: String,
    pub description: String,
    /// Also kept here: it's what says which codec the row records.
    pub content_type: String,
}

impl StationInfo {
    /// A plain file server and a bare Icecast mount answer none of these.
    pub fn is_empty(&self) -> bool {
        self.name.is_empty()
            && self.genre.is_empty()
            && self.bitrate_kbps == 0
            && self.homepage.is_empty()
            && self.description.is_empty()
    }
}

pub struct Resp {
    pub status: u16,
    /// Hints the probe when the locator has no container hint.
    pub content_type: Option<String>,
    /// From `Content-Range`: the server honours ranges, and this is the length.
    pub content_range_total: Option<u64>,
    pub content_length: Option<u64>,
    /// Audio bytes between metadata blocks, when the station agreed to send them.
    pub metaint: Option<usize>,
    /// All-empty for anything that isn't a station.
    pub station: StationInfo,
    pub body: Box<dyn Read + Send + Sync>,
}

/// One method, so tests can drive the seek arithmetic against a byte array.
pub trait Http: Send + Sync {
    /// `range` of None fetches from the top.
    fn get(
        &self,
        url: &str,
        headers: &[(String, String)],
        range: Option<u64>,
    ) -> Result<Resp, String>;
}

/// One pooled agent, the app User-Agent, and the ICY opt-in on every request.
pub struct Ureq;

/// Shared so tracks off one server reuse the connection and TLS session.
/// Connect and read timeouts only, never an overall `timeout`: it caps the
/// body too, and a station's body never ends.
fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .user_agent(USER_AGENT)
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(Duration::from_secs(20))
            .build()
    })
}

impl Http for Ureq {
    fn get(
        &self,
        url: &str,
        headers: &[(String, String)],
        range: Option<u64>,
    ) -> Result<Resp, String> {
        // A plain file server ignores it; a station starts interleaving.
        let mut req = agent().get(url).set("Icy-MetaData", "1");
        for (name, value) in headers {
            req = req.set(name, value);
        }
        if let Some(at) = range {
            req = req.set("Range", &format!("bytes={at}-"));
        }

        let resp = match req.call() {
            Ok(resp) => resp,

            // ureq's Display prints the URL, which can carry a token. Build the error
            // from safe parts.
            Err(ureq::Error::Status(code, _)) => return Err(format!("server returned {code}")),

            Err(ureq::Error::Transport(t)) => {
                return Err(match t.kind() {
                    ureq::ErrorKind::Dns
                    | ureq::ErrorKind::ConnectionFailed
                    | ureq::ErrorKind::Io => "no connection".to_string(),
                    kind => kind.to_string(),
                });
            }
        };

        let status = resp.status();
        let content_type = resp.header("Content-Type").map(str::to_string);
        let content_range_total = resp.header("Content-Range").and_then(range_total);
        let content_length = resp.header("Content-Length").and_then(|v| v.parse().ok());
        let metaint = resp
            .header("icy-metaint")
            .and_then(|v| v.trim().parse::<usize>().ok())
            .filter(|n| *n > 0);

        // The headers go past once, so read the description now.
        let station = StationInfo {
            name: header(&resp, "icy-name"),
            genre: header(&resp, "icy-genre"),
            bitrate_kbps: header(&resp, "icy-br").parse().unwrap_or(0),
            homepage: header(&resp, "icy-url"),
            description: header(&resp, "icy-description"),
            content_type: content_type.clone().unwrap_or_default(),
        };

        Ok(Resp {
            status,
            content_type,
            content_range_total,
            content_length,
            metaint,
            station,
            body: resp.into_reader(),
        })
    }
}

/// Trimmed: stations pad these with spaces.
fn header(resp: &ureq::Response, name: &str) -> String {
    resp.header(name).unwrap_or_default().trim().to_string()
}

/// `*` for an unknown length reads as None, not zero.
fn range_total(value: &str) -> Option<u64> {
    value.rsplit_once('/')?.1.trim().parse().ok()
}

/// Cap on the body read for a refusal reason, so a mislabelled stream isn't
/// pulled in whole.
const REFUSAL_PEEK: u64 = 4096;

/// The server's reason for answering something other than audio. Only JSON
/// or XML counts: vague types like `application/octet-stream` are left to
/// the probe.
fn refused(resp: &mut Resp) -> Option<String> {
    let mime = resp
        .content_type
        .as_deref()
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();

    let structured = mime.ends_with("/json")
        || mime.ends_with("+json")
        || mime.ends_with("/xml")
        || mime.ends_with("+xml");
    if !structured {
        return None;
    }

    let mut head = Vec::new();
    let _ = resp.body.by_ref().take(REFUSAL_PEEK).read_to_end(&mut head);
    let body = String::from_utf8_lossy(&head);

    Some(match api_message(&body) {
        Some(reason) => format!("the server refused the stream: {reason}"),

        // Naming the type beats the probe calling it a bad codec.
        None => format!("the server answered {mime} rather than audio"),
    })
}

/// A Subsonic error in JSON or XML. Without `f=json` the server picks, and
/// Navidrome and gonic pick XML.
fn api_message(body: &str) -> Option<String> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
        return value
            .pointer("/subsonic-response/error/message")
            .and_then(|m| m.as_str())
            .map(str::to_string);
    }

    let (_, rest) = body.split_once("message=\"")?;
    let (message, _) = rest.split_once('"')?;

    Some(message.to_string())
}

/// None lets symphonia sniff; a wrong hint is worse than none.
pub fn extension_for(content_type: &str) -> Option<&'static str> {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();

    match mime.as_str() {
        "audio/mpeg" | "audio/mp3" | "audio/x-mpeg" => Some("mp3"),

        "audio/aac" | "audio/aacp" | "audio/x-aac" => Some("aac"),

        "audio/ogg" | "application/ogg" | "audio/vorbis" | "audio/opus" => Some("ogg"),

        "audio/flac" | "audio/x-flac" => Some("flac"),

        "audio/wav" | "audio/wave" | "audio/x-wav" => Some("wav"),

        "audio/mp4" | "audio/m4a" | "audio/x-m4a" => Some("m4a"),

        _ => None,
    }
}

/// The decoder's source, and for a station its tape: once symphonia owns the
/// source nothing can reach down for the tape, and the engine needs it to
/// re-sync a decoder.
pub struct Opened {
    pub source: Box<dyn MediaSource>,
    pub tape: Option<Arc<Tape>>,
}

/// Open `remote`. Blocking: one request, whose headers say whether ranges
/// work, how long it is, and whether it's a broadcast. The station
/// description comes back in the return, since headers go past once.
///
/// For a station, `on_title` fires from the decode thread as the cursor
/// reaches each title's place in the tape, so a listener behind the broadcast
/// sees the song they're hearing; `on_stream` fires on the feed thread. Both
/// must be short. `window_secs` only matters for a station.
pub fn open(
    remote: &Remote,
    on_title: TitleSink,
    on_stream: StreamSink,
    interrupt: Arc<AtomicBool>,
    window_secs: u32,
) -> Result<(Opened, StationInfo), String> {
    // Tests put a byte array here instead of a server.
    #[cfg(test)]
    if let Some(http) = testing::current() {
        return open_with(http, remote, on_title, on_stream, interrupt, window_secs);
    }

    open_with(
        Arc::new(Ureq),
        remote,
        on_title,
        on_stream,
        interrupt,
        window_secs,
    )
}

fn open_with(
    http: Arc<dyn Http>,
    remote: &Remote,
    on_title: TitleSink,
    on_stream: StreamSink,
    interrupt: Arc<AtomicBool>,
    window_secs: u32,
) -> Result<(Opened, StationInfo), String> {
    let opened_at = Instant::now();

    // `bytes=0-` gets the top either way: a 206 with `Content-Range` is seekable,
    // a 200 isn't.
    let resp = http.get(&remote.url, &remote.headers, Some(0))?;

    // First of three open-latency timings: DNS, connect, TLS, server time. No
    // URL in these lines; it can carry a token.
    log::debug!("stream open: response headers in {:?}", opened_at.elapsed());

    if resp.status >= 300 {
        return Err(format!("server returned {}", resp.status));
    }

    // Subsonic answers 200 with an error document for a bad token, an expired
    // share, or a missing song. Catch it here, or the probe reports a missing
    // container.
    let mut resp = resp;
    if let Some(refusal) = refused(&mut resp) {
        return Err(refusal);
    }

    let station = resp.station.clone();

    if remote.live {
        let tape = spawn_feed(
            http,
            remote,
            resp,
            &station,
            on_title,
            on_stream,
            interrupt,
            window_secs,
        )?;
        let source = LiveSource {
            reader: tape.reader(0),
        };

        return Ok((
            Opened {
                source: Box::new(source),
                tape: Some(tape),
            },
            station,
        ));
    }

    let source = open_file(http, remote, resp, on_title, opened_at);

    Ok((
        Opened {
            source: Box::new(source),
            tape: None,
        },
        station,
    ))
}

/// A file or fixed-length stream, read on demand with a window for the
/// probe's rewinds.
fn open_file(
    http: Arc<dyn Http>,
    remote: &Remote,
    resp: Resp,
    on_title: TitleSink,
    opened_at: Instant,
) -> HttpSource {
    let ranged = resp.status == 206 && resp.content_range_total.is_some();

    // Interleaved metadata makes the server's byte count and ours differ, so a
    // `Range` would land wrong. Nothing with `icy-metaint` is seekable anyway.
    let banded = resp.metaint.is_some();

    let len = if banded {
        None
    } else {
        resp.content_range_total.or(resp.content_length)
    };

    HttpSource {
        http,
        url: remote.url.clone(),
        headers: remote.headers.clone(),
        seekable: ranged && !banded,
        len,
        body: wrap_body(resp, &on_title),
        on_title,
        buf: Vec::new(),
        buf_start: 0,
        pos: 0,
        attempts: 0,
        opened_at,
        read_any: false,
    }
}

/// Ogg has no in-stream sync word, so its seeks scan to a page boundary.
fn snap_for(remote: &Remote, station: &StationInfo) -> Snap {
    let ogg = |ext: Option<&str>| ext == Some("ogg");

    match ogg(extension_for(&station.content_type)) || ogg(Some(remote.hint.as_str())) {
        true => Snap::OggPage,
        false => Snap::Anywhere,
    }
}

/// A few appends a second rather than hundreds.
const FEED_CHUNK: usize = 16 * 1024;

/// Put a thread on the socket, taping, and return the tape. Nothing joins
/// it: it holds a [`Weak`] and returns once the last reader drops, closing
/// the socket within a read.
#[allow(clippy::too_many_arguments)]
fn spawn_feed(
    http: Arc<dyn Http>,
    remote: &Remote,
    resp: Resp,
    station: &StationInfo,
    on_title: TitleSink,
    on_stream: StreamSink,
    interrupt: Arc<AtomicBool>,
    window_secs: u32,
) -> Result<Arc<Tape>, String> {
    let tape = Arc::new(Tape::new(
        window_secs,
        station.bitrate_kbps,
        snap_for(remote, station),
        on_title,
        Arc::clone(&interrupt),
    ));

    let weak = Arc::downgrade(&tape);
    let body = wrap_live(resp, &weak);
    let url = remote.url.clone();
    let headers = remote.headers.clone();

    // Carry the test's retry bookkeeping onto the feed thread.
    #[cfg(test)]
    let probe = testing::probe();

    std::thread::Builder::new()
        .name("station".into())
        .spawn(move || {
            #[cfg(test)]
            testing::adopt(probe);

            feed(http, url, headers, body, weak, on_stream, interrupt);
        })
        .map_err(|e| format!("spawn station thread: {e}"))?;

    Ok(tape)
}

/// Pull the body into the tape until the tape goes away. `Ok(0)` is a drop,
/// never an end. Reconnects are bounded by [`BACKOFF`], then the tape is
/// marked done and the engine skips the entry; endless would be a stall the
/// UI can't escape. A waiting command abandons them too.
fn feed(
    http: Arc<dyn Http>,
    url: String,
    headers: Vec<(String, String)>,
    mut body: Box<dyn Read + Send + Sync>,
    tape: Weak<Tape>,
    on_stream: StreamSink,
    interrupt: Arc<AtomicBool>,
) {
    let mut buf = vec![0u8; FEED_CHUNK];
    let mut attempts = 0usize;
    let mut read_any = false;

    loop {
        let lost = match body.read(&mut buf) {
            Ok(0) => io::Error::other("the station closed the connection"),

            Ok(n) => {
                // Every reader is gone: skipped, hung up, or the session ended.
                let Some(tape) = tape.upgrade() else {
                    return;
                };

                tape.append(&buf[..n]);

                if !read_any {
                    read_any = true;
                    log::debug!("stream open: first byte off the station");
                }

                // Only worth saying if we'd said it dropped.
                if attempts > 0 {
                    log::info!("station recovered after {attempts} attempt(s)");
                    tape.set_feed(Feed::Live);
                    (on_stream)(StreamState::Live);
                    attempts = 0;
                }

                continue;
            }

            Err(e) => e,
        };

        let Some(live) = tape.upgrade() else {
            return;
        };

        let Some(wait) = BACKOFF.get(attempts).copied() else {
            log::warn!("station gone after {attempts} attempts: {lost}");
            live.set_feed(Feed::Done);
            (on_stream)(StreamState::Dropped);

            return;
        };

        attempts += 1;
        live.set_feed(Feed::Reconnecting);
        (on_stream)(StreamState::Reconnecting);
        log::info!(
            "station dropped ({lost}), reconnecting in {:?} (attempt {} of {})",
            wait,
            attempts,
            BACKOFF.len()
        );
        drop(live);

        // A command already waiting interrupts even a zero wait. Giving up publishes
        // the same state as running out of attempts.
        if nap(wait, &interrupt) {
            log::info!("station retry abandoned: a command is waiting");
            if let Some(live) = tape.upgrade() {
                live.set_feed(Feed::Done);
            }
            (on_stream)(StreamState::Dropped);

            return;
        }

        // A reconnect with no response goes round the loop like a dead read. No
        // range: a broadcast has no offset to name, so the tape marks the join.
        match http.get(&url, &headers, None) {
            Ok(resp) if resp.status < 300 => {
                let Some(live) = tape.upgrade() else {
                    return;
                };

                live.splice();
                body = wrap_live(resp, &tape);
            }

            Ok(resp) => log::warn!("station reconnect failed: server returned {}", resp.status),

            Err(e) => log::warn!("station reconnect failed: {e}"),
        }
    }
}

/// Titles are marked at their byte; the reader fires them as its cursor
/// reaches them.
fn wrap_live(resp: Resp, tape: &Weak<Tape>) -> Box<dyn Read + Send + Sync> {
    match resp.metaint {
        Some(metaint) => {
            let tape = tape.clone();

            Box::new(IcyReader::new(resp.body, metaint, move |title| {
                if let Some(tape) = tape.upgrade() {
                    tape.mark_title(title);
                }
            }))
        }

        None => resp.body,
    }
}

/// A station as the decoder sees it: a cursor into the tape. `is_seekable` is
/// false so symphonia never hunts for an index; timeshift seeks rebuild the
/// decoder instead. [`Seek`] still serves the probe's rewinds within the window.
pub struct LiveSource {
    reader: TapeReader,
}

impl Read for LiveSource {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        self.reader.read(out)
    }
}

impl Seek for LiveSource {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        self.reader.seek(from)
    }
}

impl MediaSource for LiveSource {
    fn is_seekable(&self) -> bool {
        false
    }

    fn byte_len(&self) -> Option<u64> {
        None
    }
}

/// A reader for a decoder re-synced mid-tape. The tape and connection carry on.
pub fn live_source(tape: &Arc<Tape>, at: u64) -> LiveSource {
    LiveSource {
        reader: tape.reader(at),
    }
}

/// Seekable when the server honours ranges. Files and fixed-length streams only.
pub struct HttpSource {
    http: Arc<dyn Http>,
    url: String,
    headers: Vec<(String, String)>,
    seekable: bool,
    len: Option<u64>,
    body: Box<dyn Read + Send + Sync>,
    /// Kept so a reconnect's fresh body gets the same sink.
    on_title: TitleSink,
    /// The only place a backward seek can be answered without a request.
    buf: Vec<u8>,
    /// Stream offset of `buf[0]`; the cursor never leaves the window.
    buf_start: u64,
    pos: u64,
    /// One reconnect, then the error goes up.
    attempts: usize,
    /// Where the open-latency timings measure from.
    opened_at: Instant,
    read_any: bool,
}

impl HttpSource {
    /// Where the body delivers its next byte.
    fn head(&self) -> u64 {
        self.buf_start + self.buf.len() as u64
    }

    fn reopen(&mut self, at: u64) -> Result<(), String> {
        let resp = self.http.get(&self.url, &self.headers, Some(at))?;
        if resp.status >= 300 {
            return Err(format!("server returned {}", resp.status));
        }

        // Asked for an offset and got a 200: the server ignored the range, and
        // reading on would feed the decoder the head of the file.
        if at > 0 && resp.status != 206 {
            return Err(format!("server ignored the range request at byte {at}"));
        }

        self.body = wrap_body(resp, &self.on_title);
        self.buf.clear();
        self.buf_start = at;
        self.pos = at;

        Ok(())
    }

    /// One reconnect between an error and giving up. A file's empty read is its end.
    fn pull(&mut self, out: &mut [u8]) -> io::Result<usize> {
        match self.body.read(out) {
            Ok(n) => {
                self.first_byte(n);
                self.attempts = 0;
                Ok(n)
            }

            Err(e) => {
                if self.attempts > 0 {
                    return Err(e);
                }

                self.attempts = 1;
                let at = self.pos;
                self.reopen(at).map_err(io::Error::other)?;
                self.body.read(out)
            }
        }
    }

    /// Once per source; a reconnect logs itself.
    fn first_byte(&mut self, n: usize) {
        if n == 0 || self.read_any {
            return;
        }

        self.read_any = true;
        log::debug!(
            "stream open: first byte {:?} after the open began",
            self.opened_at.elapsed()
        );
    }

    /// Drops the oldest half on overflow, so the move happens once per half window.
    fn trim(&mut self) {
        if self.buf.len() <= WINDOW {
            return;
        }

        let drop = self.buf.len() - WINDOW / 2;
        self.buf.drain(..drop);
        self.buf_start += drop as u64;
    }
}

/// Metadata stripped before anything reads a byte; titles go to the opener's sink.
fn wrap_body(resp: Resp, on_title: &TitleSink) -> Box<dyn Read + Send + Sync> {
    match resp.metaint {
        Some(metaint) => {
            let sink = Arc::clone(on_title);

            Box::new(IcyReader::new(resp.body, metaint, move |title| sink(title)))
        }

        None => resp.body,
    }
}

impl Read for HttpSource {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }

        // Behind the head after a backward seek: serve from memory.
        if self.pos < self.head() {
            let off = (self.pos - self.buf_start) as usize;
            let n = out.len().min(self.buf.len() - off);
            out[..n].copy_from_slice(&self.buf[off..off + n]);
            self.pos += n as u64;

            return Ok(n);
        }

        let n = self.pull(out)?;
        if n > 0 {
            self.buf.extend_from_slice(&out[..n]);
            self.pos += n as u64;
            self.trim();
        }

        Ok(n)
    }
}

impl Seek for HttpSource {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let target = match from {
            SeekFrom::Start(at) => Some(at),

            SeekFrom::Current(delta) => self.pos.checked_add_signed(delta),

            // Nothing to count back from on a stream. An error rather than a
            // panic: symphonia asks this of sources it hasn't checked
            // `is_seekable` on, and the answer is "no", not a crash.
            SeekFrom::End(delta) => match self.len {
                Some(len) => len.checked_add_signed(delta),
                None => return Err(io::Error::other("no length to seek from the end of")),
            },
        }
        .ok_or_else(|| io::Error::other("seek out of range"))?;

        // Inside the window, cursor move and nothing else. The head counts as
        // inside: it's exactly where the next read would go anyway.
        if target >= self.buf_start && target <= self.head() {
            self.pos = target;
            return Ok(target);
        }

        if !self.seekable {
            return Err(io::Error::other("stream is not seekable"));
        }

        self.reopen(target).map_err(io::Error::other)?;

        Ok(target)
    }
}

impl MediaSource for HttpSource {
    fn is_seekable(&self) -> bool {
        self.seekable
    }

    fn byte_len(&self) -> Option<u64> {
        self.len
    }
}

/// A byte-array transport and the thread-local slot that puts it in front of
/// ureq. Here rather than in `tests` because the engine's tests need it too.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::cell::RefCell;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    thread_local! {
        static TRANSPORT: RefCell<Option<Arc<dyn Http>>> = const { RefCell::new(None) };
        static PROBE: RefCell<Option<Arc<Probe>>> = const { RefCell::new(None) };
    }

    /// One test's view of the retry schedule. Shared, not thread-local, because
    /// the feed thread does the retrying; it adopts the spawning test's probe.
    #[derive(Default)]
    pub(crate) struct Probe {
        naps: Mutex<Vec<Duration>>,
        interrupt_at: Mutex<Option<(usize, Arc<AtomicBool>)>>,
    }

    pub(crate) fn probe() -> Arc<Probe> {
        PROBE.with(|slot| Arc::clone(slot.borrow_mut().get_or_insert_with(Default::default)))
    }

    /// A feed thread's first act.
    pub(crate) fn adopt(probe: Arc<Probe>) {
        PROBE.with(|slot| *slot.borrow_mut() = Some(probe));
    }

    /// Stands in for one backoff step. A registered interrupt flips here, between
    /// two steps, where a real one would land.
    pub(crate) fn record_nap(wait: Duration) {
        let probe = probe();
        probe.naps.lock().unwrap().push(wait);

        let mut slot = probe.interrupt_at.lock().unwrap();
        let Some((left, flag)) = slot.as_mut() else {
            return;
        };
        *left = left.saturating_sub(1);
        if *left > 0 {
            return;
        }

        flag.store(true, Ordering::Relaxed);
        *slot = None;
    }

    pub(crate) fn interrupt_after(steps: usize, flag: Arc<AtomicBool>) {
        *probe().interrupt_at.lock().unwrap() = Some((steps, flag));
    }

    pub(crate) fn naps() -> Vec<Duration> {
        probe().naps.lock().unwrap().clone()
    }

    pub(crate) fn napped() -> Duration {
        naps().iter().sum()
    }

    pub(crate) fn clear_naps() {
        let probe = probe();
        probe.naps.lock().unwrap().clear();
        *probe.interrupt_at.lock().unwrap() = None;
    }

    pub(crate) fn current() -> Option<Arc<dyn Http>> {
        TRANSPORT.with(|slot| slot.borrow().clone())
    }

    /// Thread local, so parallel tests never see each other's.
    pub(crate) fn with_transport<T>(http: Arc<dyn Http>, f: impl FnOnce() -> T) -> T {
        TRANSPORT.with(|slot| *slot.borrow_mut() = Some(http));
        let out = f();
        TRANSPORT.with(|slot| *slot.borrow_mut() = None);

        out
    }

    #[derive(Debug, PartialEq, Eq)]
    pub(crate) struct Ask {
        pub range: Option<u64>,
    }

    /// Honours ranges when `ranged`, answers like a station when `live`.
    pub(crate) struct Fake {
        pub bytes: Vec<u8>,
        pub ranged: bool,
        pub live: bool,
        pub content_type: String,
        /// Bytes per connection before it drops; `usize::MAX` holds.
        pub fail_after: usize,
        /// Upcoming connections that answer a clean `Ok(0)` on first read, counted down.
        pub empty_bodies: AtomicUsize,
        /// Delay and chunk per read. A paced body never ends: it wraps like a station.
        pub pace: Option<(Duration, usize)>,
        pub metaint: Option<usize>,
        pub station: StationInfo,
        pub asks: Mutex<Vec<Ask>>,
        pub reads: Arc<AtomicUsize>,
        /// Bodies dropped: the socket really went away.
        pub closed: Arc<AtomicUsize>,
    }

    impl Fake {
        pub(crate) fn new(bytes: Vec<u8>) -> Arc<Self> {
            Self::serving(bytes, "audio/flac")
        }

        pub(crate) fn serving(bytes: Vec<u8>, content_type: &str) -> Arc<Self> {
            Arc::new(Fake {
                bytes,
                ranged: true,
                live: false,
                content_type: content_type.to_string(),
                fail_after: usize::MAX,
                empty_bodies: AtomicUsize::new(0),
                pace: None,
                metaint: None,
                station: StationInfo::default(),
                asks: Mutex::new(Vec::new()),
                reads: Arc::new(AtomicUsize::new(0)),
                closed: Arc::new(AtomicUsize::new(0)),
            })
        }

        pub(crate) fn ask_count(&self) -> usize {
            self.asks.lock().unwrap().len()
        }

        pub(crate) fn closed_count(&self) -> usize {
            self.closed.load(Ordering::Relaxed)
        }

        /// Carries the served `Content-Type`, as a real response does.
        fn described(&self) -> StationInfo {
            StationInfo {
                content_type: self.content_type.clone(),
                ..self.station.clone()
            }
        }
    }

    /// `data` in `metaint` runs, each followed by a block carrying `titles[k]`;
    /// the last title repeats past the end of the list.
    pub(crate) fn interleave(data: &[u8], metaint: usize, titles: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        for (k, run) in data.chunks(metaint).enumerate() {
            out.extend_from_slice(run);

            // No block after a short final run.
            if run.len() < metaint {
                break;
            }

            let title = titles[k.min(titles.len().saturating_sub(1))];
            let mut text = format!("StreamTitle='{title}';").into_bytes();
            while !text.len().is_multiple_of(16) {
                text.push(0);
            }

            out.push((text.len() / 16) as u8);
            out.extend_from_slice(&text);
        }

        out
    }

    struct Body {
        data: Vec<u8>,
        at: usize,
        fail_after: usize,
        pace: Option<(Duration, usize)>,
        reads: Arc<AtomicUsize>,
        closed: Arc<AtomicUsize>,
    }

    /// Dropping a real body is the hang-up, so count it here.
    impl Drop for Body {
        fn drop(&mut self) {
            self.closed.fetch_add(1, Ordering::Relaxed);
        }
    }

    impl Read for Body {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            self.reads.fetch_add(1, Ordering::Relaxed);

            if self.at >= self.fail_after {
                return Err(io::Error::other("connection reset"));
            }

            if let Some((step, chunk)) = self.pace {
                std::thread::sleep(step);

                let from = self.at % self.data.len().max(1);
                let end = self.data.len().min(from + chunk);
                let n = out.len().min(end - from);
                out[..n].copy_from_slice(&self.data[from..from + n]);
                self.at += n;

                return Ok(n);
            }

            let end = self.data.len().min(self.fail_after);
            let n = out.len().min(end - self.at);
            out[..n].copy_from_slice(&self.data[self.at..self.at + n]);
            self.at += n;

            Ok(n)
        }
    }

    impl Http for Fake {
        fn get(
            &self,
            _url: &str,
            _headers: &[(String, String)],
            range: Option<u64>,
        ) -> Result<Resp, String> {
            self.asks.lock().unwrap().push(Ask { range });
            let at = range.unwrap_or(0) as usize;

            let spent = self
                .empty_bodies
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |left| {
                    left.checked_sub(1)
                })
                .is_ok();

            let body = match spent {
                true => Vec::new(),
                false => self.bytes[at.min(self.bytes.len())..].to_vec(),
            };
            let total = self.bytes.len() as u64;

            // No length: nothing to seek in.
            if self.live {
                return Ok(Resp {
                    status: 200,
                    content_type: Some(self.content_type.clone()),
                    content_range_total: None,
                    content_length: None,
                    metaint: self.metaint,
                    station: self.described(),
                    body: Box::new(Body {
                        data: body,
                        at: 0,
                        fail_after: self.fail_after,
                        pace: self.pace,
                        reads: Arc::clone(&self.reads),
                        closed: Arc::clone(&self.closed),
                    }),
                });
            }

            Ok(Resp {
                status: if self.ranged { 206 } else { 200 },
                content_type: Some(self.content_type.clone()),
                content_range_total: self.ranged.then_some(total),
                content_length: Some(total - at as u64),
                metaint: self.metaint,
                station: self.described(),
                body: Box::new(Body {
                    data: body,
                    at: 0,
                    fail_after: self.fail_after,
                    pace: self.pace,
                    reads: Arc::clone(&self.reads),
                    closed: Arc::clone(&self.closed),
                }),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{Ask, Fake};
    use super::*;
    use crate::icy::no_titles;
    use crate::shared::no_stream;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;

    fn uninterrupted() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    fn watched() -> (StreamSink, Arc<Mutex<Vec<StreamState>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);

        (
            Arc::new(move |state| sink.lock().unwrap().push(state)),
            seen,
        )
    }

    fn remote(live: bool) -> Remote {
        Remote {
            url: "http://example.invalid/track".into(),
            headers: vec![("Authorization".into(), "Bearer t".into())],
            hint: String::new(),
            live,
        }
    }

    fn bytes(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i % 251) as u8).collect()
    }

    fn file_open(fake: Arc<Fake>, remote: &Remote) -> (HttpSource, StationInfo) {
        let resp = fake.get(&remote.url, &remote.headers, Some(0)).unwrap();
        let station = resp.station.clone();

        (
            open_file(fake, remote, resp, no_titles(), Instant::now()),
            station,
        )
    }

    fn live_open(
        fake: Arc<Fake>,
        on_stream: StreamSink,
        interrupt: Arc<AtomicBool>,
    ) -> (Opened, StationInfo) {
        open_with(fake, &remote(true), no_titles(), on_stream, interrupt, 600).unwrap()
    }

    /// Waits on the feed thread's real work rather than a fixed sleep.
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

    fn read_n(src: &mut impl Read, n: usize) -> Vec<u8> {
        let mut out = vec![0u8; n];
        let mut got = 0;
        while got < n {
            let read = src.read(&mut out[got..]).expect("the fake never fails");
            if read == 0 {
                break;
            }
            got += read;
        }
        out.truncate(got);

        out
    }

    #[test]
    fn a_ranged_open_learns_its_length_and_type() {
        let fake = Fake::new(bytes(4096));
        let (src, info) = file_open(fake.clone(), &remote(false));

        assert!(src.is_seekable());
        assert_eq!(src.byte_len(), Some(4096));
        assert_eq!(info.content_type, "audio/flac");
        assert!(info.is_empty(), "a file server describes no station");
        assert_eq!(fake.asks.lock().unwrap().len(), 1);
    }

    /// Without the check, the probe reports Subsonic's error document as "no
    /// suitable format reader found".
    #[test]
    fn a_server_that_answers_an_error_document_says_so() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<subsonic-response status="failed" version="1.16.1">
  <error code="40" message="Wrong username or password"/>
</subsonic-response>"#;
        let fake = Fake::serving(xml.to_vec(), "application/xml");

        let err = open_with(
            fake,
            &remote(false),
            no_titles(),
            no_stream(),
            uninterrupted(),
            600,
        )
        .map(|_| ())
        .expect_err("a refused stream is not an open");

        assert_eq!(
            err,
            "the server refused the stream: Wrong username or password"
        );
    }

    /// JSON, and a structured body rox can't read.
    #[test]
    fn a_json_refusal_reads_the_same_and_an_unknown_one_names_the_type() {
        let json = br#"{"subsonic-response":{"status":"failed","version":"1.16.1",
            "error":{"code":70,"message":"The requested data was not found"}}}"#;
        let open = |bytes: Vec<u8>, content_type: &str| {
            open_with(
                Fake::serving(bytes, content_type),
                &remote(false),
                no_titles(),
                no_stream(),
                uninterrupted(),
                600,
            )
            .map(|_| ())
            .expect_err("a refused stream is not an open")
        };

        assert_eq!(
            open(json.to_vec(), "application/json; charset=utf-8"),
            "the server refused the stream: The requested data was not found"
        );
        assert_eq!(
            open(br#"{"detail":"nope"}"#.to_vec(), "application/json"),
            "the server answered application/json rather than audio"
        );
    }

    /// Stations serve `application/octet-stream` constantly; it must still open.
    #[test]
    fn an_unlabelled_stream_is_left_to_the_probe() {
        let fake = Fake::serving(bytes(4096), "application/octet-stream");

        let (opened, _) = open_with(
            fake,
            &remote(false),
            no_titles(),
            no_stream(),
            uninterrupted(),
            600,
        )
        .expect("a stream with a vague type still opens");

        assert_eq!(opened.source.byte_len(), Some(4096));
    }

    #[test]
    fn a_live_response_is_unseekable_with_no_length() {
        let mut fake = Fake::new(bytes(4096));
        Arc::get_mut(&mut fake).unwrap().live = true;
        let (opened, _) = live_open(fake, no_stream(), uninterrupted());

        assert!(!opened.source.is_seekable());
        assert_eq!(opened.source.byte_len(), None);
        assert!(opened.tape.is_some(), "and a tape came back with it");
    }

    #[test]
    fn a_station_describes_itself_at_the_open() {
        let mut fake = Fake::serving(bytes(4096), "audio/aacp");
        {
            let fake = Arc::get_mut(&mut fake).unwrap();
            fake.live = true;
            fake.station = StationInfo {
                name: "Jazz Forever".into(),
                genre: "Jazz".into(),
                bitrate_kbps: 128,
                homepage: "https://jazzforever.example".into(),
                description: "All the standards, all night".into(),
                content_type: String::new(),
            };
        }

        let (_opened, info) = live_open(fake, no_stream(), uninterrupted());

        assert_eq!(info.name, "Jazz Forever");
        assert_eq!(info.genre, "Jazz");
        assert_eq!(info.bitrate_kbps, 128);
        assert_eq!(info.homepage, "https://jazzforever.example");
        assert_eq!(info.description, "All the standards, all night");
        assert_eq!(info.content_type, "audio/aacp");
        assert!(!info.is_empty());
    }

    #[test]
    fn a_server_that_refuses_ranges_is_unseekable_but_still_plays() {
        let mut fake = Fake::new(bytes(4096));
        Arc::get_mut(&mut fake).unwrap().ranged = false;
        let (mut src, _) = file_open(fake, &remote(false));

        assert!(!src.is_seekable());
        assert_eq!(src.byte_len(), Some(4096));
        assert_eq!(read_n(&mut src, 16), &bytes(4096)[..16]);
    }

    #[test]
    fn reads_come_back_in_order() {
        let fake = Fake::new(bytes(4096));
        let (mut src, _) = file_open(fake.clone(), &remote(false));

        assert_eq!(read_n(&mut src, 4096), bytes(4096));
        assert_eq!(fake.asks.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_backward_seek_inside_the_window_issues_no_request() {
        let fake = Fake::new(bytes(4096));
        let (mut src, _) = file_open(fake.clone(), &remote(false));

        read_n(&mut src, 2048);
        assert_eq!(src.seek(SeekFrom::Start(64)).unwrap(), 64);
        assert_eq!(read_n(&mut src, 16), &bytes(4096)[64..80]);
        assert_eq!(fake.asks.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_forward_seek_inside_the_window_issues_no_request() {
        let fake = Fake::new(bytes(4096));
        let (mut src, _) = file_open(fake.clone(), &remote(false));

        read_n(&mut src, 2048);
        src.seek(SeekFrom::Start(100)).unwrap();
        assert_eq!(src.seek(SeekFrom::Current(1948)).unwrap(), 2048);
        assert_eq!(read_n(&mut src, 8), &bytes(4096)[2048..2056]);
        assert_eq!(fake.asks.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_seek_past_the_window_issues_one_ranged_request() {
        let fake = Fake::new(bytes(4096));
        let (mut src, _) = file_open(fake.clone(), &remote(false));

        read_n(&mut src, 64);
        assert_eq!(src.seek(SeekFrom::Start(3000)).unwrap(), 3000);
        assert_eq!(read_n(&mut src, 16), &bytes(4096)[3000..3016]);

        let asks = fake.asks.lock().unwrap();
        assert_eq!(asks.len(), 2);
        assert_eq!(asks[1], Ask { range: Some(3000) });
    }

    #[test]
    fn a_seek_from_the_end_lands_on_the_length() {
        let fake = Fake::new(bytes(4096));
        let (mut src, _) = file_open(fake, &remote(false));

        assert_eq!(src.seek(SeekFrom::End(-16)).unwrap(), 4080);
        assert_eq!(read_n(&mut src, 16), &bytes(4096)[4080..]);
    }

    /// The socket drains whether or not anything decodes.
    #[test]
    fn the_tape_fills_while_nothing_reads_it() {
        let mut fake = Fake::new(bytes(8192));
        {
            let f = Arc::get_mut(&mut fake).unwrap();
            f.live = true;
            f.pace = Some((Duration::from_millis(1), 512));
        }

        let (opened, _) = live_open(fake, no_stream(), uninterrupted());
        let tape = opened.tape.clone().unwrap();

        wait_for("the tape to fill", || tape.shift().window_secs > 0.0);
        assert_eq!(tape.cursor(), 0, "the decoder never asked for anything");
    }

    /// Keeping up with the station reads as a flat zero. The edge moves a chunk
    /// at a time, so the raw distance saws; the snap stops the playhead
    /// twitching.
    #[test]
    fn a_reader_keeping_up_stands_at_the_live_edge() {
        let mut fake = Fake::new(bytes(256 * 1024));
        {
            let f = Arc::get_mut(&mut fake).unwrap();
            f.live = true;
            // 16 kB every half second is 256 kbps in real time.
            f.station.bitrate_kbps = 256;
            f.pace = Some((Duration::from_millis(500), 16_000));
        }

        let (mut opened, _) = live_open(fake, no_stream(), uninterrupted());
        let tape = opened.tape.clone().unwrap();
        assert_eq!(tape.shift().cap_secs, 600.0, "the buffer's set length");

        // Either side of a chunk arriving, where the saw would show.
        for _ in 0..2 {
            assert_eq!(read_n(&mut opened.source, 16_000).len(), 16_000);
            assert_eq!(tape.shift().behind_secs, 0.0, "at the edge");

            std::thread::sleep(Duration::from_millis(250));
            assert_eq!(tape.shift().behind_secs, 0.0, "still at the edge");
        }
    }

    /// One reconnect is invisible from above, and asks for no range: an offset
    /// means nothing to a live server.
    #[test]
    fn a_live_stream_reconnects_through_a_dropped_connection() {
        let mut fake = Fake::new(bytes(4096));
        {
            let f = Arc::get_mut(&mut fake).unwrap();
            f.live = true;
            f.fail_after = 64;
        }
        let (mut opened, _) = live_open(fake.clone(), no_stream(), uninterrupted());

        assert_eq!(
            read_n(&mut opened.source, 128).len(),
            128,
            "the audio keeps coming"
        );

        let asks = fake.asks.lock().unwrap();
        assert!(asks.len() >= 2, "it reconnected");
        assert_eq!(asks[1], Ask { range: None }, "and asked for no range");
    }

    /// `Ok(0)` on a station is a drop, not an end, or the queue would move on
    /// while it's still broadcasting.
    #[test]
    fn a_live_stream_treats_a_clean_close_as_a_drop() {
        crate::http::testing::clear_naps();

        let mut fake = Fake::new(bytes(4096));
        {
            let f = Arc::get_mut(&mut fake).unwrap();
            f.live = true;
            // The open and first reconnect return nothing; the second works.
            f.empty_bodies = AtomicUsize::new(2);
            // And holds, so the schedule stops there.
            f.pace = Some((Duration::from_millis(1), 512));
        }
        let (watch, seen) = watched();
        let (mut opened, _) = live_open(fake.clone(), watch, uninterrupted());

        assert_eq!(
            read_n(&mut opened.source, 128).len(),
            128,
            "the audio keeps coming"
        );

        let asks = fake.asks.lock().unwrap();
        assert_eq!(asks.len(), 3, "the open and exactly two reconnects");
        assert_eq!(asks[1], Ask { range: None });
        assert_eq!(asks[2], Ask { range: None });
        drop(asks);

        // The first attempt is free, the second waits a second.
        assert_eq!(
            crate::http::testing::napped(),
            Duration::from_secs(1),
            "the backoff is paid once, before the second attempt"
        );

        // The recovery publishes just after its bytes, so wait for it.
        wait_for("the recovery to publish", || {
            seen.lock().unwrap().last() == Some(&StreamState::Live)
        });
        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                StreamState::Reconnecting,
                StreamState::Reconnecting,
                StreamState::Live,
            ],
            "two reconnects and the recovery, in order"
        );
    }

    /// Bounded attempts: the tape closes and the read errors, so the engine
    /// skips the entry instead of stalling.
    #[test]
    fn a_live_stream_gives_up_after_the_last_attempt() {
        crate::http::testing::clear_naps();

        let mut fake = Fake::new(bytes(4096));
        {
            let f = Arc::get_mut(&mut fake).unwrap();
            f.live = true;
            f.fail_after = 0;
        }
        let (watch, seen) = watched();
        let (mut opened, _) = live_open(fake.clone(), watch, uninterrupted());

        let mut out = [0u8; 64];
        assert!(
            opened.source.read(&mut out).is_err(),
            "the failure reaches the caller"
        );

        // The read errors a beat before the feed publishes why.
        wait_for("the feed to say it gave up", || {
            seen.lock().unwrap().last() == Some(&StreamState::Dropped)
        });

        assert_eq!(
            fake.ask_count(),
            1 + BACKOFF.len(),
            "the open, then every attempt the schedule allows"
        );
        assert_eq!(
            crate::http::testing::napped(),
            Duration::from_secs(1 + 2 + 4),
            "the whole schedule bar the free first attempt"
        );
        assert_eq!(
            seen.lock().unwrap().last(),
            Some(&StreamState::Dropped),
            "and it says so on the way out"
        );
    }

    /// A command mid-backoff ends the retries within one step.
    #[test]
    fn a_waiting_command_ends_the_retries() {
        crate::http::testing::clear_naps();

        let mut fake = Fake::new(bytes(4096));
        {
            let f = Arc::get_mut(&mut fake).unwrap();
            f.live = true;
            f.fail_after = 0;
        }

        // One step into the first real wait; the first attempt has none.
        let interrupt = Arc::new(AtomicBool::new(false));
        crate::http::testing::interrupt_after(1, Arc::clone(&interrupt));

        let (watch, seen) = watched();
        let (mut opened, _) = live_open(fake.clone(), watch, interrupt);

        let mut out = [0u8; 64];
        assert!(
            opened.source.read(&mut out).is_err(),
            "the failure reaches the caller"
        );

        wait_for("the feed to say it gave up", || {
            seen.lock().unwrap().last() == Some(&StreamState::Dropped)
        });
        assert_eq!(
            crate::http::testing::naps(),
            vec![NAP_STEP],
            "one step, not the second of backoff it was partway into"
        );
        assert_eq!(
            fake.ask_count(),
            2,
            "the open and the free first attempt, and nothing after the flag"
        );
        assert_eq!(
            seen.lock().unwrap().last(),
            Some(&StreamState::Dropped),
            "given up on, the same as running out of attempts"
        );
    }

    /// A file's zero stays an end.
    #[test]
    fn a_read_past_the_end_of_a_file_is_still_the_end() {
        crate::http::testing::clear_naps();

        let fake = Fake::new(bytes(64));
        let (mut src, _) = file_open(fake.clone(), &remote(false));

        assert_eq!(read_n(&mut src, 64), bytes(64));

        let mut out = [0u8; 16];
        assert_eq!(src.read(&mut out).unwrap(), 0, "the end is the end");
        assert_eq!(fake.ask_count(), 1, "and nothing reconnected over it");
        assert!(crate::http::testing::naps().is_empty());
    }

    /// No close call: the last reader dropping ends the feed thread and its body.
    #[test]
    fn dropping_the_last_reader_closes_the_connection() {
        let mut fake = Fake::new(bytes(8192));
        {
            let f = Arc::get_mut(&mut fake).unwrap();
            f.live = true;
            f.pace = Some((Duration::from_millis(1), 512));
        }
        let (opened, _) = live_open(fake.clone(), no_stream(), uninterrupted());

        wait_for("the first bytes", || {
            opened.tape.as_ref().unwrap().shift().window_secs > 0.0
        });
        assert_eq!(fake.closed_count(), 0, "still connected while it feeds");

        drop(opened);
        wait_for("the body to go", || fake.closed_count() == 1);
    }

    #[test]
    fn a_seek_from_the_end_of_a_stream_errors() {
        let mut fake = Fake::new(bytes(4096));
        Arc::get_mut(&mut fake).unwrap().live = true;
        let (mut opened, _) = live_open(fake, no_stream(), uninterrupted());

        assert!(opened.source.seek(SeekFrom::End(0)).is_err());
    }

    #[test]
    fn the_window_holds_only_its_most_recent_bytes() {
        let fake = Fake::new(bytes(WINDOW * 2));
        let (mut src, _) = file_open(fake.clone(), &remote(false));

        read_n(&mut src, WINDOW + WINDOW / 4);
        assert!(src.buf.len() <= WINDOW);
        src.seek(SeekFrom::Start(0)).unwrap();
        assert_eq!(fake.asks.lock().unwrap().len(), 2);
    }

    #[test]
    fn a_content_type_with_parameters_still_maps() {
        assert_eq!(extension_for("audio/mpeg; charset=UTF-8"), Some("mp3"));
        assert_eq!(extension_for("AUDIO/FLAC"), Some("flac"));
        assert_eq!(extension_for("application/octet-stream"), None);
    }

    #[test]
    fn a_content_range_gives_up_its_total() {
        assert_eq!(range_total("bytes 0-4095/8192"), Some(8192));
        assert_eq!(range_total("bytes 0-4095/*"), None);
    }

    /// Ogg can't be entered anywhere, so the content type picks the snap.
    #[test]
    fn an_ogg_station_seeks_to_page_boundaries() {
        let ogg = StationInfo {
            content_type: "application/ogg".into(),
            ..StationInfo::default()
        };
        let mp3 = StationInfo {
            content_type: "audio/mpeg".into(),
            ..StationInfo::default()
        };

        assert_eq!(snap_for(&remote(true), &ogg), Snap::OggPage);
        assert_eq!(snap_for(&remote(true), &mp3), Snap::Anywhere);
    }
}
