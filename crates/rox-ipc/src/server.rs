//! The listening side: one accept thread, and per connection a reader
//! thread that owns the frame discipline (parse, handshake, dispatch) and
//! a writer thread draining one bounded outbound channel. Responses and
//! pushed events both leave through that channel, which lets an event land
//! between two responses without interleaving bytes mid-frame.
//! The only things that ever leave a connection are a [`Request`] on the
//! app's channel, answered through its responder, and a registration with
//! the [`Events`] registry when the client subscribes. A malformed frame
//! costs its caller an error response and nothing else; the connection and
//! the app both play on.
//!
//! Two transports carry it. Unix speaks std's own domain sockets and binds
//! with the single-instance guard's staging-and-rename discipline. Windows
//! speaks named pipes through interprocess, which needs none of that: a
//! pipe isn't a filesystem object, can't go stale, and dies with the
//! process. Windows pipes use the default DACL, which scopes to the
//! session rather than strictly to the user; tightening that to an explicit
//! per-user descriptor is open follow-up work.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::path::PathBuf;
use std::sync::mpsc::{RecvTimeoutError, SyncSender};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::events::Events;
use crate::protocol::{RequestFrame, ResponseFrame, RpcError, PROTOCOL_VERSION};

/// The longest frame a client may send, one megabyte. A queue insert of a
/// few thousand paths fits many times over; what this stops is a peer
/// feeding an endless line into the reader's buffer.
const MAX_FRAME_BYTES: u64 = 1024 * 1024;

/// How long a connection waits for the app to answer one request before
/// telling its caller to retry. Generous because the answer comes off the UI
/// thread, which a heavy frame can hold up; a wedged app surfaces as this
/// error rather than a silent hang.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);

/// How many outbound frames may queue for one connection before a
/// subscriber that stopped reading is cut off. A healthy consumer drains as
/// events arrive and never grows more than a few deep; this deep means the
/// peer is gone or wedged, and the emit side never waits to find out which.
const OUTBOUND_BUFFER: usize = 128;

/// One method call crossing from a connection thread to the app. The app
/// answers through [`respond`](Request::respond); dropping the request
/// unanswered reads as a timeout on the caller's side, never a hang.
pub struct Request {
    pub method: String,
    pub params: Value,
    responder: SyncSender<Result<Value, RpcError>>,
}

impl Request {
    pub fn respond(self, result: Result<Value, RpcError>) {
        // A send with nobody listening means the connection died while the
        // app worked; nothing left to tell.
        let _ = self.responder.send(result);
    }

    /// Split the request from its responder, for answers that finish off
    /// the calling thread (an artwork read on the background executor).
    pub fn into_parts(self) -> (String, Value, Responder) {
        (self.method, self.params, Responder(self.responder))
    }
}

/// The answering half of a split [`Request`].
pub struct Responder(SyncSender<Result<Value, RpcError>>);

impl Responder {
    pub fn respond(self, result: Result<Value, RpcError>) {
        let _ = self.0.send(result);
    }
}

/// The bound listener, carried from [`bind`](Server::bind) to
/// [`spawn`](Server::spawn).
pub struct Server {
    #[cfg(unix)]
    listener: std::os::unix::net::UnixListener,
    #[cfg(windows)]
    listener: interprocess::local_socket::Listener,
    path: PathBuf,
    /// The inode the socket path pointed at when we bound it, so quit can
    /// tell our socket from one a racing bind put there since. Unix only
    /// in practice; a pipe leaves nothing behind to clean.
    inode: Option<u64>,
}

/// What quit needs to clear the socket file: the path, and the inode that
/// proves it's still ours. Clone because gpui's quit hook needs to be
/// callable more than once.
#[derive(Clone)]
pub struct Cleanup {
    path: PathBuf,
    inode: Option<u64>,
}

impl Cleanup {
    /// Remove the socket file if it's still the one we bound.
    pub fn remove(&self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            let live = std::fs::metadata(&self.path).ok().map(|meta| meta.ino());
            if self.inode.is_some() && live == self.inode {
                let _ = std::fs::remove_file(&self.path);
            }
        }
        // A pipe leaves nothing behind; the fields only exist so quit's
        // hook has one shape everywhere.
        #[cfg(not(unix))]
        let _ = (&self.path, &self.inode);
    }
}

#[cfg(unix)]
impl Server {
    /// Bind the control socket at `path`. `Err` when another live rox
    /// already answers there or the directory won't take a socket; the app
    /// logs it and runs on without the surface.
    pub fn bind(path: &std::path::Path) -> Result<Server, String> {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        use std::os::unix::net::{UnixListener, UnixStream};

        // A live listener on the path means another rox owns this data
        // directory's control surface. A dead file doesn't answer, and is
        // safe to replace below.
        if UnixStream::connect(path).is_ok() {
            return Err("another instance already serves this socket".into());
        }
        // Bind under our own name and rename into place, the single-instance
        // guard's dance: two racing binds can't delete each other's socket,
        // the path just ends up at whichever renamed last.
        let staging = path.with_extension(format!("{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&staging);
        let listener = UnixListener::bind(&staging).map_err(|e| e.to_string())?;
        // User-only: the socket drives playback and reads the library, and
        // the data dir standing in for a missing runtime dir isn't
        // guaranteed private.
        let _ = std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o600));
        if let Err(err) = std::fs::rename(&staging, path) {
            let _ = std::fs::remove_file(&staging);
            return Err(err.to_string());
        }
        Ok(Server {
            listener,
            path: path.to_path_buf(),
            inode: std::fs::metadata(path).ok().map(|meta| meta.ino()),
        })
    }

    /// Start accepting: one detached thread for the accept loop, two per
    /// connection (reader and writer). Requests land on the returned
    /// receiver; the app drains it on its own executor and answers each
    /// one. Events emitted through the returned [`Events`] handle reach
    /// every connection that subscribed.
    pub fn spawn(self) -> (async_channel::Receiver<Request>, Events, Cleanup) {
        let (tx, requests) = async_channel::unbounded();
        let events = Events::new();
        let cleanup = Cleanup {
            path: self.path,
            inode: self.inode,
        };
        let listener = self.listener;
        let broadcast = events.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let tx = tx.clone();
                let events = broadcast.clone();
                std::thread::spawn(move || {
                    let Ok(write_half) = stream.try_clone() else {
                        return;
                    };
                    // The registry's cutoff for a subscriber that stopped
                    // draining: shut the socket down so both halves of the
                    // connection unwind instead of parking on a peer that
                    // will never read again.
                    let kill: Arc<dyn Fn() + Send + Sync> = match stream.try_clone() {
                        Ok(clone) => Arc::new(move || {
                            let _ = clone.shutdown(std::net::Shutdown::Both);
                        }),
                        Err(_) => Arc::new(|| {}),
                    };
                    connection(stream, write_half, tx, events, kill);
                });
            }
        });
        (requests, events, cleanup)
    }
}

#[cfg(windows)]
impl Server {
    /// Bind the control pipe named by `path`. `Err` when another rox
    /// already serves the name; nothing on disk to probe or replace, since
    /// a pipe lives and dies with its process.
    pub fn bind(path: &std::path::Path) -> Result<Server, String> {
        use interprocess::local_socket::ListenerOptions;

        let name = crate::pipe_name(path).map_err(|e| e.to_string())?;
        let listener = match ListenerOptions::new().name(name).create_sync() {
            Ok(listener) => listener,
            Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
                return Err("another instance already serves this pipe".into());
            }
            Err(err) => return Err(err.to_string()),
        };
        Ok(Server {
            listener,
            path: path.to_path_buf(),
            inode: None,
        })
    }

    /// The Unix spawn's twin: same accept loop, same per-connection
    /// threads, with the stream split into halves where std would clone.
    pub fn spawn(self) -> (async_channel::Receiver<Request>, Events, Cleanup) {
        use interprocess::local_socket::traits::{ListenerExt as _, Stream as _};

        let (tx, requests) = async_channel::unbounded();
        let events = Events::new();
        let cleanup = Cleanup {
            path: self.path,
            inode: self.inode,
        };
        let listener = self.listener;
        let broadcast = events.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let tx = tx.clone();
                let events = broadcast.clone();
                std::thread::spawn(move || {
                    let (read_half, write_half) = stream.split();
                    // A pipe's halves offer no shutdown to force, so a
                    // subscriber the registry cuts off just stops getting
                    // events; the bounded buffer still caps what it can
                    // cost, and the threads unwind when the peer goes.
                    let kill: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
                    connection(read_half, write_half, tx, events, kill);
                });
            }
        });
        (requests, events, cleanup)
    }
}

#[cfg(not(any(unix, windows)))]
impl Server {
    /// No transport on this platform, so the bind refuses and rox runs on
    /// without the socket.
    pub fn bind(_path: &std::path::Path) -> Result<Server, String> {
        Err("no control socket backend on this platform".into())
    }

    pub fn spawn(self) -> (async_channel::Receiver<Request>, Events, Cleanup) {
        unreachable!("bind never succeeds on this platform");
    }
}

/// One connection's whole life: read frames, hold the handshake, forward
/// method calls, queue responses for the writer. Runs on its own thread;
/// blocking reads are the pacing. Generic over the transport's two halves,
/// which is the whole seam between the Unix and Windows backends.
///
/// `subscribe` is answered here rather than by the app because what it
/// changes is a property of this connection's write path: from that frame
/// on, the outbound channel is enrolled with the registry and pushed events
/// share it with responses. A connection that never subscribes is never
/// enrolled and receives nothing unasked.
#[cfg(any(unix, windows))]
fn connection<R: std::io::Read, W: std::io::Write + Send + 'static>(
    read_half: R,
    write_half: W,
    tx: async_channel::Sender<Request>,
    events: Events,
    kill: Arc<dyn Fn() + Send + Sync>,
) {
    // Everything outbound crosses this bounded channel to one writer
    // thread, so responses and events interleave as whole frames. When the
    // peer stops reading, the writer blocks, the channel fills, and the
    // registry's try_send notices, never the app.
    let (out_tx, out_rx) = std::sync::mpsc::sync_channel::<Arc<[u8]>>(OUTBOUND_BUFFER);
    std::thread::spawn(move || {
        let mut writer = std::io::BufWriter::new(write_half);
        while let Ok(bytes) = out_rx.recv() {
            if writer
                .write_all(&bytes)
                .and_then(|_| writer.flush())
                .is_err()
            {
                return;
            }
        }
    });

    let mut reader = BufReader::new(read_half);
    let mut greeted = false;
    let mut subscribed = false;

    loop {
        let mut line = Vec::new();
        // Cap the frame instead of trusting read_until's appetite; one byte
        // past the cap means the peer isn't speaking our protocol.
        match reader
            .by_ref()
            .take(MAX_FRAME_BYTES + 1)
            .read_until(b'\n', &mut line)
        {
            Ok(0) => return,
            Ok(_) if line.len() as u64 > MAX_FRAME_BYTES => {
                let _ = send_frame(
                    &out_tx,
                    ResponseFrame::error(Value::Null, RpcError::invalid_request("frame too long")),
                );
                return;
            }
            Ok(_) => {}
            Err(_) => return,
        }
        if line.iter().all(|b| b.is_ascii_whitespace()) {
            continue;
        }

        let frame: RequestFrame = match serde_json::from_slice(&line) {
            Ok(frame) => frame,
            Err(err) => {
                // One bad frame is the caller's problem, not the
                // connection's: answer and read on.
                if send_frame(
                    &out_tx,
                    ResponseFrame::error(Value::Null, RpcError::parse_error(err)),
                )
                .is_err()
                {
                    return;
                }
                continue;
            }
        };

        let id = frame.id.clone();
        let response = if frame.method == "hello" {
            match hello(&frame.params) {
                Ok(result) => {
                    greeted = true;
                    ResponseFrame::result(id, result)
                }
                Err(err) => ResponseFrame::error(id, err),
            }
        } else if !greeted {
            ResponseFrame::error(id, RpcError::handshake_required())
        } else if frame.method == "subscribe" {
            // Idempotent: a second subscribe re-answers rather than
            // enrolling the channel twice and doubling every event.
            if !subscribed {
                events.register(out_tx.clone(), kill.clone());
                subscribed = true;
            }
            ResponseFrame::result(id, json!({ "subscribed": true }))
        } else {
            match forward(&tx, frame) {
                Ok(result) => ResponseFrame::result(id, result),
                Err(err) => ResponseFrame::error(id, err),
            }
        };
        if send_frame(&out_tx, response).is_err() {
            return;
        }
    }
}

/// The handshake: the client names the protocol generation it speaks, and
/// the answer is who we are. Anything but the one version this build
/// serves is refused, so a future client finds out here rather than on a
/// method that silently means something else.
#[cfg(any(unix, windows))]
fn hello(params: &Value) -> Result<Value, RpcError> {
    let asked = params.get("protocol");
    match asked.and_then(Value::as_u64) {
        Some(v) if v == PROTOCOL_VERSION as u64 => Ok(json!({
            "name": "rox",
            "version": env!("CARGO_PKG_VERSION"),
            "protocol": PROTOCOL_VERSION,
        })),
        Some(_) => Err(RpcError::unsupported_protocol(
            asked.cloned().unwrap_or(Value::Null),
        )),
        None => Err(RpcError::invalid_params(
            "hello takes {\"protocol\": <number>}",
        )),
    }
}

/// Hand one call to the app and wait for its answer. A full timeout means
/// the caller retries; the request's responder going out of scope on the
/// app side answers the same way.
#[cfg(any(unix, windows))]
fn forward(tx: &async_channel::Sender<Request>, frame: RequestFrame) -> Result<Value, RpcError> {
    let (responder, answer) = std::sync::mpsc::sync_channel(1);
    tx.send_blocking(Request {
        method: frame.method,
        params: frame.params,
        responder,
    })
    .map_err(|_| RpcError::app("rox is shutting down"))?;
    match answer.recv_timeout(RESPONSE_TIMEOUT) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout) => Err(RpcError::timeout()),
        // The app dropped the request without answering: a method it
        // recognized but couldn't route. Reads as a refusal, not a hang.
        Err(RecvTimeoutError::Disconnected) => Err(RpcError::app("request dropped unanswered")),
    }
}

/// Queue one response for the writer thread. Blocks while the outbound
/// buffer is full, which paces the reader to the peer's appetite; `Err`
/// means the writer is gone and the connection is over.
#[cfg(any(unix, windows))]
fn send_frame(out: &SyncSender<Arc<[u8]>>, frame: ResponseFrame) -> Result<(), ()> {
    let Ok(mut bytes) = serde_json::to_vec(&frame) else {
        return Err(());
    };
    bytes.push(b'\n');
    out.send(bytes.into()).map_err(|_| ())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::BufReader;
    use std::os::unix::net::UnixStream;

    /// A server on a scratch socket with a dispatcher that answers
    /// `echo` with its params and refuses everything else. Named per test
    /// so parallel tests don't race on one path.
    fn serve(name: &str) -> (std::path::PathBuf, Events) {
        let path =
            std::env::temp_dir().join(format!("rox-ipc-test-{name}-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let server = Server::bind(&path).expect("bind scratch socket");
        let (requests, events, _cleanup) = server.spawn();
        std::thread::spawn(move || {
            while let Ok(request) = requests.recv_blocking() {
                let answer = match request.method.as_str() {
                    "echo" => Ok(request.params.clone()),
                    other => Err(RpcError::method_not_found(other)),
                };
                request.respond(answer);
            }
        });
        (path, events)
    }

    fn call(stream: &mut UnixStream, line: &str) -> Value {
        stream.write_all(line.as_bytes()).unwrap();
        stream.write_all(b"\n").unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut response = String::new();
        reader.read_line(&mut response).unwrap();
        serde_json::from_str(&response).unwrap()
    }

    #[test]
    fn handshake_gates_methods_and_frames_round_trip() {
        let (path, _events) = serve("frames");
        let mut stream = UnixStream::connect(&path).unwrap();

        // Before hello, a method call is refused, and the connection lives on.
        let refused = call(&mut stream, r#"{"id":1,"method":"echo","params":{}}"#);
        assert_eq!(refused["error"]["code"], -32001);

        // A malformed frame gets an error response and wedges nothing.
        let garbled = call(&mut stream, "{not json");
        assert_eq!(garbled["error"]["code"], -32700);

        // The wrong protocol generation is refused by name.
        let wrong = call(
            &mut stream,
            r#"{"id":2,"method":"hello","params":{"protocol":99}}"#,
        );
        assert_eq!(wrong["error"]["code"], -32002);

        let hello = call(
            &mut stream,
            r#"{"id":3,"method":"hello","params":{"protocol":1}}"#,
        );
        assert_eq!(hello["result"]["name"], "rox");

        // Past the handshake, calls round-trip through the dispatcher.
        let echoed = call(
            &mut stream,
            r#"{"id":4,"method":"echo","params":{"track":"a.flac"}}"#,
        );
        assert_eq!(echoed["result"]["track"], "a.flac");
        assert_eq!(echoed["id"], 4);

        let unknown = call(&mut stream, r#"{"id":5,"method":"nope","params":{}}"#);
        assert_eq!(unknown["error"]["code"], -32601);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn events_reach_subscribers_only() {
        let (path, events) = serve("events");

        // One connection subscribes...
        let mut subscriber = UnixStream::connect(&path).unwrap();
        call(
            &mut subscriber,
            r#"{"id":1,"method":"hello","params":{"protocol":1}}"#,
        );
        let subscribed = call(
            &mut subscriber,
            r#"{"id":2,"method":"subscribe","params":{}}"#,
        );
        assert_eq!(subscribed["result"]["subscribed"], true);

        // ...one only shakes hands.
        let mut bystander = UnixStream::connect(&path).unwrap();
        call(
            &mut bystander,
            r#"{"id":1,"method":"hello","params":{"protocol":1}}"#,
        );

        // The registration is done once the subscribe response is read, so
        // this emit can't race it.
        events.emit("event.test", serde_json::json!({ "n": 1 }));

        let mut reader = BufReader::new(subscriber.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let frame: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(frame["method"], "event.test");
        assert_eq!(frame["params"]["n"], 1);
        // A missing id marks it a push, not a response.
        assert!(frame.get("id").is_none());

        // The bystander got nothing pushed: the very next frame on its
        // wire is the answer to its own call.
        let echoed = call(
            &mut bystander,
            r#"{"id":9,"method":"echo","params":{"x":1}}"#,
        );
        assert_eq!(echoed["id"], 9);
        assert_eq!(echoed["result"]["x"], 1);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn slow_subscriber_is_cut_not_waited_on() {
        let (path, events) = serve("slow");
        let mut subscriber = UnixStream::connect(&path).unwrap();
        call(
            &mut subscriber,
            r#"{"id":1,"method":"hello","params":{"protocol":1}}"#,
        );
        call(
            &mut subscriber,
            r#"{"id":2,"method":"subscribe","params":{}}"#,
        );

        // Flood without the subscriber reading a byte. The emit side must
        // sail through: once the socket buffers and the outbound channel
        // fill, the registry cuts the subscriber and later emits find an
        // empty registry. Blocking anywhere here fails the test by hanging.
        let pad = "x".repeat(2048);
        for n in 0..10_000 {
            events.emit("event.flood", serde_json::json!({ "n": n, "pad": pad }));
        }

        // The cutoff shut the socket down, so draining what was buffered
        // runs out well short of the flood instead of running forever.
        subscriber
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        let mut reader = BufReader::new(subscriber);
        let mut drained = 0usize;
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => drained += 1,
                Err(err) => panic!("subscriber socket neither drained nor closed: {err}"),
            }
            assert!(drained < 10_000, "the whole flood arrived; nobody was cut");
        }

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn socket_is_user_only() {
        use std::os::unix::fs::PermissionsExt as _;
        let path = std::env::temp_dir().join(format!("rox-ipc-perm-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let server = Server::bind(&path).expect("bind scratch socket");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        drop(server);
        let _ = std::fs::remove_file(&path);
    }
}
