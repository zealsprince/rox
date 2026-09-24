//! The listening side: one accept thread, and per connection a reader thread
//! (parse, handshake, dispatch) and a writer draining one bounded outbound
//! channel, so pushed events land between responses as whole frames. A
//! malformed frame costs its caller an error response and nothing else.
//!
//! Unix binds with the single-instance guard's staging-and-rename. Windows
//! uses named pipes, which can't go stale and need none of that. Pipes keep
//! the default DACL; an explicit per-user descriptor is open follow-up work.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{RecvTimeoutError, SyncSender};
use std::time::Duration;

use serde_json::{Value, json};

use crate::events::Events;
use crate::protocol::{PROTOCOL_VERSION, RequestFrame, ResponseFrame, RpcError};

/// Stops a peer feeding an endless line into the reader.
const MAX_FRAME_BYTES: u64 = 1024 * 1024;

/// Generous because the answer comes off the UI thread; a wedged app
/// surfaces as this error rather than a silent hang.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);

/// A healthy consumer never queues more than a few; this deep means the peer
/// is gone or wedged.
const OUTBOUND_BUFFER: usize = 128;

/// Dropping it unanswered reads as a refusal on the caller's side, never a hang.
pub struct Request {
    pub method: String,
    pub params: Value,
    responder: SyncSender<Result<Value, RpcError>>,
}

impl Request {
    pub fn respond(self, result: Result<Value, RpcError>) {
        let _ = self.responder.send(result);
    }

    /// For answers that finish off the calling thread.
    pub fn into_parts(self) -> (String, Value, Responder) {
        (self.method, self.params, Responder(self.responder))
    }
}

pub struct Responder(SyncSender<Result<Value, RpcError>>);

impl Responder {
    pub fn respond(self, result: Result<Value, RpcError>) {
        let _ = self.0.send(result);
    }
}

pub struct Server {
    #[cfg(unix)]
    listener: std::os::unix::net::UnixListener,
    #[cfg(windows)]
    listener: interprocess::local_socket::Listener,
    path: PathBuf,
    /// So quit can tell our socket from one a racing bind put there since.
    inode: Option<u64>,
}

/// Clone because gpui's quit hook can run more than once.
#[derive(Clone)]
pub struct Cleanup {
    path: PathBuf,
    inode: Option<u64>,
}

impl Cleanup {
    pub fn remove(&self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            let live = std::fs::metadata(&self.path).ok().map(|meta| meta.ino());
            if self.inode.is_some() && live == self.inode {
                let _ = std::fs::remove_file(&self.path);
            }
        }
        #[cfg(not(unix))]
        let _ = (&self.path, &self.inode);
    }
}

#[cfg(unix)]
impl Server {
    /// `Err` when another live rox already answers there; the app runs on without it.
    pub fn bind(path: &std::path::Path) -> Result<Server, String> {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        use std::os::unix::net::{UnixListener, UnixStream};

        // A dead file doesn't answer and is safe to replace below.
        if UnixStream::connect(path).is_ok() {
            return Err("another instance already serves this socket".into());
        }
        // Bind under our own name and rename into place: two racing binds can't
        // delete each other's socket.
        let staging = path.with_extension(format!("{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&staging);
        let listener = UnixListener::bind(&staging).map_err(|e| e.to_string())?;
        // User-only: the socket drives playback, and the data dir standing in
        // for a missing runtime dir isn't guaranteed private.
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
                    // The registry's cutoff: shut both halves down.
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
    pub fn bind(path: &std::path::Path) -> Result<Server, String> {
        use interprocess::local_socket::ListenerOptions;

        let name = crate::pipe_name(path).map_err(|e| e.to_string())?;
        let listener = match ListenerOptions::new().name(name).create_sync() {
            Ok(listener) => listener,
            Err(err) if crate::pipe_taken(&err) => {
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

    /// Like the Unix spawn, with the stream split into halves.
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
                    // Pipe halves can't be shut down, so a cut subscriber just
                    // stops getting events; the bounded buffer caps the cost.
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
    pub fn bind(_path: &std::path::Path) -> Result<Server, String> {
        Err("no control socket backend on this platform".into())
    }

    pub fn spawn(self) -> (async_channel::Receiver<Request>, Events, Cleanup) {
        unreachable!("bind never succeeds on this platform");
    }
}

/// One connection's whole life, generic over the transport halves.
/// `subscribe` is answered here rather than by the app: it enrolls this
/// connection's outbound channel with the registry.
#[cfg(any(unix, windows))]
fn connection<R: std::io::Read, W: std::io::Write + Send + 'static>(
    read_half: R,
    write_half: W,
    tx: async_channel::Sender<Request>,
    events: Events,
    kill: Arc<dyn Fn() + Send + Sync>,
) {
    // One bounded channel to one writer thread. When the peer stops reading,
    // the registry's try_send notices, never the app.
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
        // Cap the frame instead of trusting read_until's appetite.
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
            // Idempotent: never enroll the channel twice.
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

/// Anything but this build's exact protocol version is refused, so a future
/// client finds out here rather than on a method that means something else.
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
        Err(RecvTimeoutError::Disconnected) => Err(RpcError::app("request dropped unanswered")),
    }
}

/// Blocks while the buffer is full, pacing the reader to the peer.
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

    /// Answers `echo` with its params and refuses everything else.
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

        let refused = call(&mut stream, r#"{"id":1,"method":"echo","params":{}}"#);
        assert_eq!(refused["error"]["code"], -32001);

        let garbled = call(&mut stream, "{not json");
        assert_eq!(garbled["error"]["code"], -32700);

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

        let mut bystander = UnixStream::connect(&path).unwrap();
        call(
            &mut bystander,
            r#"{"id":1,"method":"hello","params":{"protocol":1}}"#,
        );

        // Registered once the subscribe response is read, so no race.
        events.emit("event.test", serde_json::json!({ "n": 1 }));

        let mut reader = BufReader::new(subscriber.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let frame: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(frame["method"], "event.test");
        assert_eq!(frame["params"]["n"], 1);
        assert!(frame.get("id").is_none());

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

        // Flood without reading. Blocking anywhere fails the test by hanging.
        let pad = "x".repeat(2048);
        for n in 0..10_000 {
            events.emit("event.flood", serde_json::json!({ "n": n, "pad": pad }));
        }

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
