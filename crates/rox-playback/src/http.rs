//! A [`MediaSource`] over HTTP, so the decoder stops caring whether its bytes
//! come off a disk or a socket.
//!
//! Symphonia asks a source to read and to seek, and a file answers both
//! without thinking about it. HTTP has no cursor: a seek here is a fresh GET
//! with a `Range` header, and everything between the last request and the one
//! after it has to come out of a buffer. That buffer is also what makes a
//! small backward seek free, which matters because the probe rewinds over the
//! container header more than once before it settles on a format.
//!
//! A station is the other case, and it isn't [`HttpSource`] at all. Internet
//! radio has no length and no end, the server ignores ranges because there's
//! nothing to range over, and it can't be read on demand: a client that stops
//! reading while a listener pauses gets dropped or throttled. So a live open
//! puts a thread on the socket and a tape behind it ([`crate::tape`]), and
//! hands the decoder a [`LiveSource`] reading that tape at its own cursor.
//! The connection is drained whether or not anything is decoding, which is
//! what makes a pause resumable and the last few minutes seekable.
//!
//! Everything about a dropping station lives on that thread now. A clean
//! `Ok(0)` is a drop wearing a tidy shutdown's clothes, since a broadcast has
//! no end, and it goes down the same bounded reconnect schedule an outright
//! error does. The schedule still watches the session's interrupt flag,
//! because a station that has gone quiet leaves the decode thread parked at
//! the live edge with a pause unanswered, and the flag is how that ends. What
//! changed is who waits: the retries no longer sit on the decode thread, so
//! the only thing a backoff holds up is the tape filling. A reconnect that
//! lands appends at the live edge and records a gap, since two connections'
//! bytes are not one decodable stream.
//!
//! There is no `close` here, on purpose. The body owns the socket, the reader
//! owns the body, and dropping the whole thing is how a connection ends: the
//! feed thread holds a [`Weak`] to the tape and stops the moment the last
//! reader lets go. A method would have to leave the source alive with no body
//! under it, which is one more state every read and reconnect would then have
//! to answer for.
//!
//! This opens on the decode thread and never on the audio callback. ADR 19's
//! no-allocation invariant is about the output side of the ring and this is
//! the other side of it, but the distinction is worth saying out loud: a
//! network round trip anywhere near the callback would be a dropout every
//! time. The decode thread is allowed to block, and it already does, on
//! `File::open`. The feed thread is a fourth thread beside those two and the
//! UI's, and it touches neither the ring nor the callback's atomics.
//!
//! Layering note: the repo's rule is that wire calls live in `rox-net`. This
//! one doesn't, because it's a byte transport the decode loop pulls from
//! synchronously rather than a request a background task makes, and `rox-net`
//! would have to learn a symphonia trait to host it. ADR 29 records that as a
//! stated exception.

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

/// How much of the recent stream stays in memory. Sized off the probe rather
/// than off playback: symphonia's probe depth is a megabyte, an MP3 carrying
/// embedded cover art in its ID3v2 tag can push the real container header out
/// that far, and the probe walks back over that header while it decides what
/// it's looking at. A smaller window turns each of those rewinds into a round
/// trip.
const WINDOW: usize = 1 << 20;

/// How long to wait before each reconnect attempt on a station.
///
/// The first is free, because the overwhelmingly common drop is a server
/// cycling a connection and answering the next request immediately. After
/// that the waits double, because a station that refused twice in a row is
/// either restarting or rate limiting, and hammering it is how a client earns
/// a block. Four entries is seven seconds of trying before the entry is
/// declared dead: long enough to ride out a mount point restart, and short
/// enough that the decode thread isn't somewhere else for the length of a
/// verse while someone holds down Next.
const BACKOFF: [Duration; 4] = [
    Duration::from_secs(0),
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
];

/// How much of a backoff is served before the interrupt flag is read again.
/// The wait is on the decode thread, so this is how long a press can go
/// unanswered while a station is being retried.
const NAP_STEP: Duration = Duration::from_millis(250);

/// Wait out a backoff, stopping early when a command arrives. True means it
/// stopped early and the caller should give up rather than reconnect.
///
/// Stepped rather than one long sleep because the decode thread is the thread
/// that answers the transport, and a listener pressing pause on a station
/// that has gone quiet should not be waiting on a retry schedule. Relaxed
/// ordering: the flag is a hint that something is in the channel, and the
/// channel does its own synchronising.
///
/// The steps are recorded rather than slept in tests, so the schedule stays
/// assertable and the suite doesn't sit out seven seconds of it.
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

/// The identity this sends. Icecast operators read their logs, and a station
/// that wants to block a client needs something to name.
const USER_AGENT: &str = concat!(
    "rox/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/zealsprince/rox)"
);

/// What a station says about itself in its response headers. Icecast and
/// Shoutcast both answer a stream request with these, and they're the only
/// description of a station that exists outside a directory: a URL typed
/// into the add box or imported out of a `.pls` arrives with a name at
/// best, and everything else about it is on the wire.
///
/// Every field is optional in practice, so every one of them can come back
/// empty. A station that sends none of them leaves this all-empty rather
/// than absent, because "connected and told us nothing" and "not connected"
/// are different states and only the first one should stop us asking.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StationInfo {
    /// `icy-name`: the station's own name for itself.
    pub name: String,
    /// `icy-genre`: one word most of the time, a comma list sometimes.
    pub genre: String,
    /// `icy-br`, in kilobits. Zero for a station that doesn't state one.
    pub bitrate_kbps: u32,
    /// `icy-url`: the station's homepage, which is the one link a listener
    /// might actually want to follow.
    pub homepage: String,
    /// `icy-description`: a sentence about the station, when it bothers.
    pub description: String,
    /// The `Content-Type`, kept here as well as on the response because
    /// this is what says which codec the row should record.
    pub content_type: String,
}

impl StationInfo {
    /// Whether anything came back at all. A plain file server answers none
    /// of these, and so does a bare-bones Icecast mount, and neither is
    /// worth publishing as a station description.
    pub fn is_empty(&self) -> bool {
        self.name.is_empty()
            && self.genre.is_empty()
            && self.bitrate_kbps == 0
            && self.homepage.is_empty()
            && self.description.is_empty()
    }
}

/// One response, reduced to what the transport actually reads off it.
pub struct Resp {
    pub status: u16,
    /// The `Content-Type`, so the caller can hint the probe when the locator
    /// carries no container hint of its own.
    pub content_type: Option<String>,
    /// The total size out of `Content-Range`, which is the server saying it
    /// honours ranges and how long the whole thing is.
    pub content_range_total: Option<u64>,
    pub content_length: Option<u64>,
    /// `icy-metaint`: audio bytes between two in-band metadata blocks. Present
    /// only when the station agreed to send metadata.
    pub metaint: Option<usize>,
    /// What the station said about itself on the way in, all-empty for
    /// everything that isn't one.
    pub station: StationInfo,
    pub body: Box<dyn Read + Send + Sync>,
}

/// Where the bytes are asked for. One method, so the seek arithmetic and the
/// buffer window can be driven by a test against an array of bytes instead of
/// a server. [`Ureq`] is the only implementor production ever sees.
pub trait Http: Send + Sync {
    /// GET `url` with `headers` set on it, starting at byte `range` when the
    /// caller wants a range and from the top when it doesn't.
    fn get(
        &self,
        url: &str,
        headers: &[(String, String)],
        range: Option<u64>,
    ) -> Result<Resp, String>;
}

/// The real transport: one pooled agent, the app User-Agent, and the ICY
/// metadata opt-in on every request.
pub struct Ureq;

/// The agent every remote source shares, so a queue of tracks off one server
/// reuses its connection and its TLS session.
///
/// Connect and read timeouts only, deliberately no overall `timeout`. That one
/// caps the whole request including the body, and the body here is the music:
/// on a live station it never ends, and on a long track it outlives any number
/// a timeout could sensibly be set to.
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
        // Asking for metadata costs nothing on a server that has none: a plain
        // file server ignores the header, and a station answers with
        // `icy-metaint` and starts interleaving.
        let mut req = agent().get(url).set("Icy-MetaData", "1");
        for (name, value) in headers {
            req = req.set(name, value);
        }
        if let Some(at) = range {
            req = req.set("Range", &format!("bytes={at}-"));
        }

        let resp = match req.call() {
            Ok(resp) => resp,

            // ureq's own Display prints the request URL, and a source's URL
            // can carry a token in its query string, so the error is built
            // out of the parts that are safe to log instead.
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

        // The station's own description of itself, read here because this is
        // the only moment it's offered: the headers go past once, on the way
        // into a body that then runs for hours.
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

/// One header as a trimmed string, empty for one the server didn't send.
/// Stations pad these with spaces often enough that an untrimmed read shows
/// up in the UI as a leading gap.
fn header(resp: &ureq::Response, name: &str) -> String {
    resp.header(name).unwrap_or_default().trim().to_string()
}

/// The total out of a `Content-Range: bytes 0-99/12345` header. A server that
/// knows the range but not the length sends `*` there, which reads as no
/// answer rather than as zero.
fn range_total(value: &str) -> Option<u64> {
    value.rsplit_once('/')?.1.trim().parse().ok()
}

/// How much of a refusing server's body is read to find its reason. An API
/// error is a few hundred bytes; anything past this isn't one, and the cap
/// is what keeps a mislabelled stream from being pulled into memory whole.
const REFUSAL_PEEK: u64 = 4096;

/// The server's own reason for answering something other than audio, or
/// None when what came back is a stream.
///
/// Only a JSON or XML content type counts. Stations serve plenty of vague
/// types, `application/octet-stream` most of all, and the probe sniffs
/// those perfectly well; a structured document is the one shape that is
/// never music.
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

        // Something structured that isn't a shape we read. Naming the type
        // is still worth more than letting the probe call it a bad codec.
        None => format!("the server answered {mime} rather than audio"),
    })
}

/// The message out of a Subsonic error body, in either form a server
/// answers in. A stream URL carries no `f=json`, so which one arrives is
/// the server's choice: Navidrome and gonic both pick XML.
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

/// The container extension a `Content-Type` implies, for hinting the probe
/// when the locator didn't say. None means let symphonia sniff the bytes,
/// which it's good at; a wrong hint is worse than no hint.
pub fn extension_for(content_type: &str) -> Option<&'static str> {
    // Parameters ride along on the same header ("audio/mpeg; charset=UTF-8").
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

/// What an open hands back: the thing the decoder reads through, and for a
/// station the tape behind it.
///
/// The tape rides back here because nothing above can reach down for it
/// later. Once symphonia owns the source it's buried under a
/// [`MediaSourceStream`](symphonia::core::io::MediaSourceStream) and a format
/// reader, and the engine needs the tape in hand to re-sync a decoder onto
/// another point in it.
pub struct Opened {
    pub source: Box<dyn MediaSource>,
    /// The station's tape, None for a file or a seekable stream.
    pub tape: Option<Arc<Tape>>,
}

/// Open `remote` and hand back what the decoder reads through, with what the
/// server said about itself on the way in: the `Content-Type` the probe hints
/// off, and the `icy-` headers a station describes itself with. Blocking: one
/// request, and the response headers answer whether ranges work, how long the
/// thing is, and whether this is a broadcast.
///
/// The description rides back rather than going out through a sink of its
/// own. Unlike a title, which keeps arriving for as long as the stream plays,
/// this is said once at the open and never again, so it belongs to the return
/// of the call that read it.
///
/// `on_title` takes the station's in-band titles. For a station it fires from
/// the decode thread as the cursor reaches the point in the tape each title
/// was found at, so a listener ten minutes behind the broadcast is told the
/// song they're hearing rather than the one on the air. `on_stream` takes the
/// drops and recoveries, and fires on the feed thread. Both have to be short;
/// the engine's own sinks write one slot and bump an atomic.
///
/// `window_secs` is how much of a station to keep behind the playhead. It
/// means nothing for anything that isn't live.
///
/// Called on the decode thread, never on the audio callback.
pub fn open(
    remote: &Remote,
    on_title: TitleSink,
    on_stream: StreamSink,
    interrupt: Arc<AtomicBool>,
    window_secs: u32,
) -> Result<(Opened, StationInfo), String> {
    // The engine's own tests open a `Remote` and there's no server to open it
    // against, so a test can put a byte array here instead. Compiled out of
    // every real build.
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

    // One request does the whole probe. `bytes=0-` gets the stream from the
    // top either way, and the status says which kind of thing is on the other
    // end: a 206 with a `Content-Range` is a file that can be seeked in, a 200
    // is one that can't.
    let resp = http.get(&remote.url, &remote.headers, Some(0))?;

    // First of the three timings the open-latency question needs. This one
    // covers DNS, the connection, TLS and the server's own time to answer, and
    // it's the one nothing in here can do anything about. No URL in any of
    // these lines: a source's URL can carry a token in its query string, and
    // the engine's own line names the stream.
    log::debug!("stream open: response headers in {:?}", opened_at.elapsed());

    if resp.status >= 300 {
        return Err(format!("server returned {}", resp.status));
    }

    // A server that won't serve this track can still answer 200, with its
    // own error document where the audio should be. Subsonic does exactly
    // that for a bad token, a share that expired and a song id it no longer
    // has. Caught here because the probe downstream can only say it found
    // no container, which sends the reader hunting a codec bug over what is
    // really a refused request.
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

/// The non-live half of an open: a file, or a stream with a length, read on
/// demand off one response with a window behind it for the probe's rewinds.
fn open_file(
    http: Arc<dyn Http>,
    remote: &Remote,
    resp: Resp,
    on_title: TitleSink,
    opened_at: Instant,
) -> HttpSource {
    let ranged = resp.status == 206 && resp.content_range_total.is_some();

    // Interleaved metadata makes the server's byte count and ours two
    // different numbers: the reader below eats the blocks, so a `Range` we ask
    // for lands somewhere other than where the cursor thinks it is. Anything
    // sending `icy-metaint` has nothing to seek in anyway, so this costs
    // nothing real.
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

/// What a seek in this station's tape has to land on, off what the server
/// said it was sending. Ogg is the one container with no in-stream sync word
/// to hunt for, so it gets scanned to a page boundary; everything else rides
/// its own frame header.
fn snap_for(remote: &Remote, station: &StationInfo) -> Snap {
    let ogg = |ext: Option<&str>| ext == Some("ogg");

    match ogg(extension_for(&station.content_type)) || ogg(Some(remote.hint.as_str())) {
        true => Snap::OggPage,
        false => Snap::Anywhere,
    }
}

/// How much comes off the socket per read. A few reads' worth of a station's
/// second, so the tape is appended to a handful of times a second rather than
/// hundreds.
const FEED_CHUNK: usize = 16 * 1024;

/// Put a thread on this station's socket, taping what comes off it, and hand
/// back the tape.
///
/// The thread outlives the open and nothing joins it. It holds a [`Weak`] to
/// the tape, so the last reader letting go is what ends it: dropping the
/// source drops the tape, the next upgrade fails, and the thread returns with
/// the body, and therefore the socket, going out of scope. That's the same
/// contract the old on-demand source had, where dropping the source closed
/// the connection; it just takes a read's worth of latency to take effect.
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

    // The bookkeeping a test watches the retry schedule through. It lives on
    // whichever thread set it up, so the feed carries it over rather than
    // recording into a slot nobody is looking at.
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

/// Pull the body into the tape until the tape goes away, reconnecting through
/// the drops a station takes as a matter of routine.
///
/// The clean zero is the case worth spelling out. A server that closes its
/// side tidily returns `Ok(0)`, and a broadcast has no end, so zero here is a
/// drop and goes down the same path an error does.
///
/// The reconnects are bounded rather than endless: [`BACKOFF`] attempts, then
/// the tape is marked done, the reader at the live edge gets an error out of
/// it, and the engine skips past the entry the way it does for a station that
/// was dead at the open. Endless would be a silent stall with no way out of
/// it from the UI.
///
/// They're also abandoned the moment a command arrives. Nothing is decoding
/// while a station is down, so whoever pressed something is waiting on a
/// stream that may never come back; the flag is how they stop waiting.
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
                // Every reader is gone: the entry was skipped, the pause hit
                // its idle cap, or the session ended. Nothing to tape for.
                let Some(tape) = tape.upgrade() else {
                    return;
                };

                tape.append(&buf[..n]);

                if !read_any {
                    read_any = true;
                    log::debug!("stream open: first byte off the station");
                }

                // Back on the air, and only worth saying when something was
                // told we had left it.
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

        // The wait answers for both halves of this: a command that was
        // already waiting when the connection died reads as an interrupt on a
        // wait of zero, and one that arrives mid-backoff stops it a step in.
        // Giving up is the same answer as running out of attempts, so the
        // state published is the same one too.
        if nap(wait, &interrupt) {
            log::info!("station retry abandoned: a command is waiting");
            if let Some(live) = tape.upgrade() {
                live.set_feed(Feed::Done);
            }
            (on_stream)(StreamState::Dropped);

            return;
        }

        // A reconnect that can't even get a response is the same failure as
        // one whose reads die, so it goes round the loop rather than giving
        // up on the first refused connection. The old body is still in place
        // and still failing, which is what carries the loop to the next
        // attempt.
        //
        // No range: there's no offset in a broadcast to name, and the server
        // would serve now whatever we asked for. The tape marks the join
        // instead, because the bytes either side of it don't decode as one
        // stream.
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

/// The body as the tape should see it: audio only, with the titles marked at
/// the byte they were announced at rather than fired straight out. The reader
/// on the other end fires them as its cursor reaches them.
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

/// A station as the decoder sees it: a cursor into the tape, with no length
/// and no seek.
///
/// `is_seekable` stays false the way it always did. Symphonia's own time seek
/// would go looking for an index that a broadcast has never had, and the seek
/// a listener actually wants, a step back through the last few minutes, is
/// the engine reopening a decoder over the same tape at another offset. The
/// [`Seek`] below is real all the same: it's what the probe's rewinds ride,
/// and it can't leave the window.
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

/// A source over a tape that's already running, for a decoder being re-synced
/// onto another point in a station. The tape, and the connection feeding it,
/// carry on untouched.
pub fn live_source(tape: &Arc<Tape>, at: u64) -> LiveSource {
    LiveSource {
        reader: tape.reader(at),
    }
}

/// A [`MediaSource`] reading off an HTTP response, seekable when the server
/// said it honours ranges.
///
/// Files and fixed-length streams only. A station is [`LiveSource`] over a
/// tape, since a broadcast can't be read on demand.
pub struct HttpSource {
    http: Arc<dyn Http>,
    url: String,
    headers: Vec<(String, String)>,
    seekable: bool,
    len: Option<u64>,
    body: Box<dyn Read + Send + Sync>,
    /// Where titles go, kept so a reconnect's fresh body gets the same sink.
    /// A plain file never fires it; a fixed-length stream that answered with
    /// `icy-metaint` can.
    on_title: TitleSink,
    /// The most recent bytes off the body, which is the only place a backward
    /// seek can be answered from without another request.
    buf: Vec<u8>,
    /// Stream offset `buf[0]` sits at. The window runs from here to
    /// `buf_start + buf.len()`, and the cursor never leaves it.
    buf_start: u64,
    pos: u64,
    /// How many reconnects have been spent since the last read that worked.
    /// A file over HTTP only ever reads this as the boolean it is: one
    /// reconnect, then the error goes up.
    attempts: usize,
    /// When the open started, for the timing lines the first read logs. The
    /// delay before a track starts is made of a request, a probe and a
    /// decode, and this is where the first two of those are measured from.
    opened_at: Instant,
    /// Whether a byte has come off the body yet, so the first-byte timing is
    /// logged once per source rather than per read.
    read_any: bool,
}

impl HttpSource {
    /// The stream offset just past the buffered window, which is where the
    /// body will deliver its next byte.
    fn head(&self) -> u64 {
        self.buf_start + self.buf.len() as u64
    }

    /// Start a new request at `at` and throw the window away, since nothing in
    /// it belongs to the new position.
    fn reopen(&mut self, at: u64) -> Result<(), String> {
        let resp = self.http.get(&self.url, &self.headers, Some(at))?;
        if resp.status >= 300 {
            return Err(format!("server returned {}", resp.status));
        }

        // Asked for an offset and got the whole thing back means the server
        // dropped the range on the floor. Reading on would hand the decoder
        // the head of the file while claiming to be somewhere else in it.
        if at > 0 && resp.status != 206 {
            return Err(format!("server ignored the range request at byte {at}"));
        }

        self.body = wrap_body(resp, &self.on_title);
        self.buf.clear();
        self.buf_start = at;
        self.pos = at;

        Ok(())
    }

    /// Pull from the body, with one reconnect between an error and giving up.
    /// A file over HTTP has a length and an end, so a read that comes back
    /// empty is that end and nothing else; only an error is worth answering,
    /// and a second one straight after the first is a dead server.
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

    /// Note the first bytes off this connection, for the open-latency lines.
    /// Once per source: a reconnect's first read is a different question and
    /// the reconnect logs itself.
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

    /// Keep the window from growing without bound, dropping the oldest half
    /// when it overflows. Half rather than the exact overflow so the move
    /// happens once per half window of audio instead of once per read.
    fn trim(&mut self) {
        if self.buf.len() <= WINDOW {
            return;
        }

        let drop = self.buf.len() - WINDOW / 2;
        self.buf.drain(..drop);
        self.buf_start += drop as u64;
    }
}

/// The body as the decoder should see it: audio only. A station that agreed to
/// send metadata interleaves it into the same stream, so the stripping goes on
/// before anything else reads a byte, and the titles it finds go out through
/// the sink the opener handed down.
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

        // Behind the head, which is where a backward seek inside the window
        // left the cursor. Serve it out of memory and touch no socket.
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

/// The seam a test reaches through: a transport made of a byte array, and the
/// thread-local slot that puts it in front of ureq. Lives here rather than in
/// the test module below because the engine's own tests open a `Remote`
/// through [`Source::open`](crate::engine) and need the same fake. Compiled
/// out of every real build.
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

    /// One test's view of the retry schedule.
    ///
    /// Shared rather than thread local because the retries moved off the
    /// thread that starts them: a station is fed by a thread of its own now,
    /// and a test asserting on the backoff has to see what that thread did.
    /// Each test still gets its own, since libtest gives each one its own
    /// thread and the feed adopts whichever it was spawned from.
    #[derive(Default)]
    pub(crate) struct Probe {
        naps: Mutex<Vec<Duration>>,
        interrupt_at: Mutex<Option<(usize, Arc<AtomicBool>)>>,
    }

    /// This thread's probe, made on first use.
    pub(crate) fn probe() -> Arc<Probe> {
        PROBE.with(|slot| Arc::clone(slot.borrow_mut().get_or_insert_with(Default::default)))
    }

    /// Take on the probe of the thread that spawned this one, which a feed
    /// thread does as its first act.
    pub(crate) fn adopt(probe: Arc<Probe>) {
        PROBE.with(|slot| *slot.borrow_mut() = Some(probe));
    }

    /// Stand in for one step of the backoff sleep. Recording them keeps the
    /// schedule assertable without the suite sitting out seven seconds of it,
    /// and per step rather than per wait so a test can say how far into a
    /// backoff an interrupt landed.
    ///
    /// The flag a registered test wants flipped mid-wait goes up here, which
    /// is the one moment a real session's would: between two steps, with the
    /// reconnect loop about to look at it.
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

    /// Have the `steps`-th recorded step be the one that sets `flag`, for the
    /// test about a command arriving while a station is being retried.
    pub(crate) fn interrupt_after(steps: usize, flag: Arc<AtomicBool>) {
        *probe().interrupt_at.lock().unwrap() = Some((steps, flag));
    }

    /// The steps waited for this test, wherever they were waited.
    pub(crate) fn naps() -> Vec<Duration> {
        probe().naps.lock().unwrap().clone()
    }

    /// The same as one number, for the tests that care about the schedule
    /// rather than about where in it something happened.
    pub(crate) fn napped() -> Duration {
        naps().iter().sum()
    }

    pub(crate) fn clear_naps() {
        let probe = probe();
        probe.naps.lock().unwrap().clear();
        *probe.interrupt_at.lock().unwrap() = None;
    }

    /// The transport standing in for ureq on this thread, if a test put one
    /// there.
    pub(crate) fn current() -> Option<Arc<dyn Http>> {
        TRANSPORT.with(|slot| slot.borrow().clone())
    }

    /// Run `f` with `http` answering every request. Thread local, so tests
    /// running in parallel never see each other's.
    pub(crate) fn with_transport<T>(http: Arc<dyn Http>, f: impl FnOnce() -> T) -> T {
        TRANSPORT.with(|slot| *slot.borrow_mut() = Some(http));
        let out = f();
        TRANSPORT.with(|slot| *slot.borrow_mut() = None);

        out
    }

    /// What a request asked for, so a test can assert that a seek issued one
    /// and with which `Range`.
    #[derive(Debug, PartialEq, Eq)]
    pub(crate) struct Ask {
        pub range: Option<u64>,
    }

    /// A server made of one byte array. Honours ranges when `ranged`, answers
    /// like a station when `live`, and records every request.
    pub(crate) struct Fake {
        pub bytes: Vec<u8>,
        pub ranged: bool,
        pub live: bool,
        pub content_type: String,
        /// Bytes each connection serves before the socket goes down under it,
        /// for the reconnect path. `usize::MAX` is a connection that holds.
        pub fail_after: usize,
        /// How many of the next connections hand back nothing at all: a clean
        /// `Ok(0)` on the first read, which is how a server closes its side
        /// tidily. Counted down as bodies go out, so a test can say "the
        /// first two connections are dead and the third one works".
        pub empty_bodies: AtomicUsize,
        /// How long each read takes and how much it hands over, for the
        /// tests about a tape filling while nothing reads it. A paced body
        /// is also an endless one: it wraps back to the top of its bytes
        /// rather than ending, which is what a station does and what a
        /// window has to overrun to be trimmed. None serves everything at
        /// once and then ends, which is every other test here.
        pub pace: Option<(Duration, usize)>,
        /// Answered as `icy-metaint`, for a body built by [`interleave`].
        pub metaint: Option<usize>,
        /// Answered as the `icy-` header set, for the tests about what a
        /// station says about itself at the open.
        pub station: StationInfo,
        pub asks: Mutex<Vec<Ask>>,
        /// Reads served across every body this handed out, for anything
        /// that wants to see the socket being worked rather than the bytes
        /// that came off it.
        pub reads: Arc<AtomicUsize>,
        /// Bodies dropped. A connection closes when its reader does, so this
        /// is what a test watches to know the socket actually went away
        /// rather than the source merely being told to stop using it.
        pub closed: Arc<AtomicUsize>,
    }

    impl Fake {
        pub(crate) fn new(bytes: Vec<u8>) -> Arc<Self> {
            Self::serving(bytes, "audio/flac")
        }

        /// The same with the `Content-Type` a test cares about, for the ones
        /// that go through the probe hint.
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

        /// How many requests this has been asked for, for the tests that only
        /// care about the count and not the ranges.
        pub(crate) fn ask_count(&self) -> usize {
            self.asks.lock().unwrap().len()
        }

        pub(crate) fn closed_count(&self) -> usize {
            self.closed.load(Ordering::Relaxed)
        }

        /// The description this answers with, which carries the served
        /// `Content-Type` the way a real response does: the transport
        /// reads both off the same headers.
        fn described(&self) -> StationInfo {
            StationInfo {
                content_type: self.content_type.clone(),
                ..self.station.clone()
            }
        }
    }

    /// Build what a station actually sends: `data` cut into `metaint` runs
    /// with a metadata block after each one. Block `k` carries `titles[k]`,
    /// and the last title stands for every block past the end of the list, so
    /// a test says "this, then this, then hold" and gets a stream that
    /// repeats the way a real one does between songs.
    pub(crate) fn interleave(data: &[u8], metaint: usize, titles: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        for (k, run) in data.chunks(metaint).enumerate() {
            out.extend_from_slice(run);

            // A short final run isn't followed by a block: the station would
            // still be mid-run when the bytes ran out.
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

    /// A body that hands back bytes until the connection drops out from under
    /// it, which is what a station does on a routine basis.
    struct Body {
        data: Vec<u8>,
        at: usize,
        fail_after: usize,
        /// Paced: sleep this long per read, hand over at most this much, and
        /// never end.
        pace: Option<(Duration, usize)>,
        reads: Arc<AtomicUsize>,
        closed: Arc<AtomicUsize>,
    }

    /// The close a test watches for. A real body is a socket and dropping it
    /// is the hang-up, so the counter moves in exactly the place the FIN
    /// would go out.
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

            // Paced, which is to say behaving like a broadcast: a read costs
            // real time, hands over a chunk, and the bytes never run out.
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

            // A connection that serves nothing: the body exists, the first
            // read off it is a clean zero.
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

            // A station answers 200 with no length at all, which is the whole
            // signal that there's nothing here to seek in.
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

    /// A session with no command waiting, which is every test here bar the
    /// one about what happens when there is.
    fn uninterrupted() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    /// A stream sink that keeps what it was told, so a test can read back the
    /// states a drop and its recovery published and in which order.
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

    /// The file path's source on its own, so the tests about the window and
    /// the range arithmetic can look at it rather than at the boxed
    /// `MediaSource` the open seam hands back.
    fn file_open(fake: Arc<Fake>, remote: &Remote) -> (HttpSource, StationInfo) {
        let resp = fake.get(&remote.url, &remote.headers, Some(0)).unwrap();
        let station = resp.station.clone();

        (
            open_file(fake, remote, resp, no_titles(), Instant::now()),
            station,
        )
    }

    /// A station open, with the feed thread already on the socket.
    fn live_open(
        fake: Arc<Fake>,
        on_stream: StreamSink,
        interrupt: Arc<AtomicBool>,
    ) -> (Opened, StationInfo) {
        open_with(fake, &remote(true), no_titles(), on_stream, interrupt, 600).unwrap()
    }

    /// Wait for the feed thread to get somewhere, or fail the test. Nothing
    /// here sleeps for a fixed length: the thread is doing real work on a
    /// real clock, so the tests watch for the state they're about rather
    /// than guessing at how long it takes.
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

    /// A refused stream reads as a refusal rather than as a broken file.
    /// Subsonic answers 200 with its error document where the audio should
    /// be, and before this the probe got a few hundred bytes of XML and
    /// reported "no suitable format reader found", which is a true sentence
    /// about entirely the wrong thing.
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

    /// The same for a server that answers JSON, which is what a `f=json`
    /// request gets, and for one whose document rox has no shape for.
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

    /// And a vague content type is still opened. Stations serve
    /// `application/octet-stream` constantly and the probe reads those; a
    /// refusal check that ate them would break every one of them.
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

    /// A station describes itself in headers and nowhere else, and they go
    /// past once. The open is the only place that sees them, so this is the
    /// read that everything the UI shows about a station hangs off.
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
        // Back into the window, then forward again to the head, which is a
        // cursor move both ways.
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

    /// The whole point of the feed thread: the socket is drained whether or
    /// not anything is decoding, so a paused station keeps piling up and the
    /// resume has somewhere to carry on from.
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

        // Nothing has read a byte, and the cursor proves it.
        wait_for("the tape to fill", || tape.shift().window_secs > 0.0);
        assert_eq!(tape.cursor(), 0, "the decoder never asked for anything");
    }

    /// A reader keeping up with the station stands at the live edge, and
    /// says so with a flat zero.
    ///
    /// The edge only moves when a chunk lands, which at radio bitrates is a
    /// step of half a second or more, so a cursor keeping pace is somewhere
    /// inside the last chunk at any given moment and the raw distance saws
    /// between nothing and a chunk. That saw is a playhead that twitches,
    /// which is the whole reason the number is snapped.
    #[test]
    fn a_reader_keeping_up_stands_at_the_live_edge() {
        let mut fake = Fake::new(bytes(256 * 1024));
        {
            let f = Arc::get_mut(&mut fake).unwrap();
            f.live = true;
            // 256 kbps is 32 kB/s, and 16 kB every half second is that
            // rate: a station sending in real time.
            f.station.bitrate_kbps = 256;
            f.pace = Some((Duration::from_millis(500), 16_000));
        }

        let (mut opened, _) = live_open(fake, no_stream(), uninterrupted());
        let tape = opened.tape.clone().unwrap();
        assert_eq!(tape.shift().cap_secs, 600.0, "the buffer's set length");

        // Sampled either side of a chunk arriving, which is where the saw
        // would show.
        for _ in 0..2 {
            assert_eq!(read_n(&mut opened.source, 16_000).len(), 16_000);
            assert_eq!(tape.shift().behind_secs, 0.0, "at the edge");

            std::thread::sleep(Duration::from_millis(250));
            assert_eq!(tape.shift().behind_secs, 0.0, "still at the edge");
        }
    }

    /// A station drops the connection mid-song as a matter of course, so one    /// A station drops the connection mid-song as a matter of course, so one
    /// reconnect has to be invisible from above. The reconnect asks for no
    /// range, which is the whole reason a live stream can recover at all: an
    /// offset into a stream means nothing to the server. What it does leave
    /// behind is a gap, since the bytes either side of it are two different
    /// runs of the encoder.
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

    /// The drop that reads as an ending. A server closing its side tidily
    /// answers `Ok(0)`, which would be end of stream on a file: the track
    /// ends, and a queue with nothing behind the station plays out while the
    /// station is still broadcasting. So zero is a drop here, answered the
    /// same way an error is, and the tape carries on across it.
    #[test]
    fn a_live_stream_treats_a_clean_close_as_a_drop() {
        crate::http::testing::clear_naps();

        let mut fake = Fake::new(bytes(4096));
        {
            let f = Arc::get_mut(&mut fake).unwrap();
            f.live = true;
            // The open's own connection and the first reconnect both hand
            // back nothing; the second reconnect is the one that works.
            f.empty_bodies = AtomicUsize::new(2);
            // And that one holds, the way a station does, so the schedule
            // this is about stops where it stopped.
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

        // The first attempt goes straight out and the second waits a second,
        // so one wait for two reconnects.
        assert_eq!(
            crate::http::testing::napped(),
            Duration::from_secs(1),
            "the backoff is paid once, before the second attempt"
        );

        // The recovery is published just after the bytes it recovered with,
        // so a read landing first is the ordinary case rather than a race
        // worth failing over.
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

    /// A station that never comes back. The attempts are bounded, so the
    /// tape is closed and the read at the live edge errors out, which is how
    /// the engine learns to skip past the entry rather than parking on a
    /// silent stall forever.
    #[test]
    fn a_live_stream_gives_up_after_the_last_attempt() {
        crate::http::testing::clear_naps();

        let mut fake = Fake::new(bytes(4096));
        {
            let f = Arc::get_mut(&mut fake).unwrap();
            f.live = true;
            // Every connection dies on its first read, reconnects included.
            f.fail_after = 0;
        }
        let (watch, seen) = watched();
        let (mut opened, _) = live_open(fake.clone(), watch, uninterrupted());

        let mut out = [0u8; 64];
        assert!(
            opened.source.read(&mut out).is_err(),
            "the failure reaches the caller"
        );

        // The read errors the moment the tape is closed, which is a beat
        // before the feed thread publishes what closed it.
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

    /// A command arriving mid-backoff ends the retries. Nothing is decoding
    /// while a station is down, so whoever pressed something is waiting on a
    /// stream that may never come back. The wait is served in steps so the
    /// answer comes inside a quarter second rather than at the end of
    /// whatever the schedule had left.
    #[test]
    fn a_waiting_command_ends_the_retries() {
        crate::http::testing::clear_naps();

        let mut fake = Fake::new(bytes(4096));
        {
            let f = Arc::get_mut(&mut fake).unwrap();
            f.live = true;
            // Nothing recovers, so only the interrupt can end this.
            f.fail_after = 0;
        }

        // Set one step into the first real wait, which is the second attempt:
        // the first is served with no wait at all.
        let interrupt = Arc::new(AtomicBool::new(false));
        crate::http::testing::interrupt_after(1, Arc::clone(&interrupt));

        let (watch, seen) = watched();
        let (mut opened, _) = live_open(fake.clone(), watch, interrupt);

        let mut out = [0u8; 64];
        assert!(
            opened.source.read(&mut out).is_err(),
            "the failure reaches the caller"
        );

        // The read errors the moment the tape is closed, a beat before the
        // thread publishes what closed it.
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

    /// A file over HTTP does end, so its zero stays a zero. Nothing about the
    /// station path reaches the shape that has a length.
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

    /// The hang-up, from this side. Nothing calls a close: the feed thread
    /// holds a weak handle to the tape, so the last reader letting go is
    /// what ends it, and the body goes out of scope with the thread.
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
        // The oldest bytes are gone, so going back to the top is a request.
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

    /// Ogg is the one container a timeshift seek can't drop into anywhere,
    /// so the content type decides what a seek has to land on.
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
