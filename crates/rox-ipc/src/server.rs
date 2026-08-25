//! The listening side: one accept thread, one thread per connection. The
//! connection thread owns the whole frame discipline - parse, handshake,
//! response - and the only thing that ever leaves it is a [`Request`] on the
//! channel, answered through its responder. A malformed frame costs its
//! caller an error response and nothing else; the connection and the app
//! both play on.
//!
//! Two transports carry it. Unix speaks std's own domain sockets and binds
//! with the single-instance guard's staging-and-rename discipline. Windows
//! speaks named pipes through interprocess, which needs none of that: a
//! pipe isn't a filesystem object, can't go stale, and dies with the
//! process. Windows pipes ride the default DACL, which scopes to the
//! session rather than strictly to the user; tightening that to an explicit
//! per-user descriptor is open follow-up work.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::path::PathBuf;
use std::sync::mpsc::{RecvTimeoutError, SyncSender};
use std::time::Duration;

use serde_json::{json, Value};

use crate::protocol::{RequestFrame, ResponseFrame, RpcError, PROTOCOL_VERSION};

/// The longest frame a client may send, one megabyte. A queue insert of a
/// few thousand paths fits many times over; what this stops is a peer
/// feeding an endless line into the reader's buffer.
const MAX_FRAME_BYTES: u64 = 1024 * 1024;

/// How long a connection waits for the app to answer one request before
/// telling its caller to retry. Generous because the answer rides the UI
/// thread, which a heavy frame can hold up; a wedged app surfaces as this
/// error rather than a silent hang.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);

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
/// proves it's still ours. Clone because gpui's quit hook wants to be
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

    /// Start accepting: one detached thread for the accept loop, one per
    /// connection. Requests land on the returned receiver; the app drains
    /// it on its own executor and answers each one.
    pub fn spawn(self) -> (async_channel::Receiver<Request>, Cleanup) {
        let (tx, requests) = async_channel::unbounded();
        let cleanup = Cleanup {
            path: self.path,
            inode: self.inode,
        };
        let listener = self.listener;
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let tx = tx.clone();
                std::thread::spawn(move || {
                    let Ok(write_half) = stream.try_clone() else {
                        return;
                    };
                    connection(stream, write_half, tx);
                });
            }
        });
        (requests, cleanup)
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
    pub fn spawn(self) -> (async_channel::Receiver<Request>, Cleanup) {
        use interprocess::local_socket::traits::{ListenerExt as _, Stream as _};

        let (tx, requests) = async_channel::unbounded();
        let cleanup = Cleanup {
            path: self.path,
            inode: self.inode,
        };
        let listener = self.listener;
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let tx = tx.clone();
                std::thread::spawn(move || {
                    let (read_half, write_half) = stream.split();
                    connection(read_half, write_half, tx);
                });
            }
        });
        (requests, cleanup)
    }
}

#[cfg(not(any(unix, windows)))]
impl Server {
    /// No transport on this platform, so the bind refuses and rox runs on
    /// without the socket.
    pub fn bind(_path: &std::path::Path) -> Result<Server, String> {
        Err("no control socket backend on this platform".into())
    }

    pub fn spawn(self) -> (async_channel::Receiver<Request>, Cleanup) {
        unreachable!("bind never succeeds on this platform");
    }
}

/// One connection's whole life: read frames, hold the handshake, forward
/// method calls, write responses. Runs on its own thread; blocking reads
/// are the pacing. Generic over the transport's two halves, which is the
/// whole seam between the Unix and Windows backends.
#[cfg(any(unix, windows))]
fn connection<R: std::io::Read, W: std::io::Write>(
    read_half: R,
    write_half: W,
    tx: async_channel::Sender<Request>,
) {
    let mut writer = std::io::BufWriter::new(write_half);
    let mut reader = BufReader::new(read_half);
    // A dead peer can't wedge the thread on a response it stopped reading.
    let mut greeted = false;

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
                let _ = write_frame(
                    &mut writer,
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
                if write_frame(
                    &mut writer,
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
        } else {
            match forward(&tx, frame) {
                Ok(result) => ResponseFrame::result(id, result),
                Err(err) => ResponseFrame::error(id, err),
            }
        };
        if write_frame(&mut writer, response).is_err() {
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

#[cfg(any(unix, windows))]
fn write_frame<W: std::io::Write>(
    writer: &mut std::io::BufWriter<W>,
    frame: ResponseFrame,
) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec(&frame)?;
    bytes.push(b'\n');
    writer.write_all(&bytes)?;
    writer.flush()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::os::unix::net::UnixStream;

    /// A server on a scratch socket with a dispatcher that answers
    /// `echo` with its params and refuses everything else.
    fn serve() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("rox-ipc-test-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let server = Server::bind(&path).expect("bind scratch socket");
        let (requests, _cleanup) = server.spawn();
        std::thread::spawn(move || {
            while let Ok(request) = requests.recv_blocking() {
                let answer = match request.method.as_str() {
                    "echo" => Ok(request.params.clone()),
                    other => Err(RpcError::method_not_found(other)),
                };
                request.respond(answer);
            }
        });
        path
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
        let path = serve();
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
