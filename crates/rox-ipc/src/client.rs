//! A small blocking client over the socket: connect, shake hands, call
//! methods. The reference consumers (the CLI, the MCP proxy) use this so
//! the frame discipline stays in one place on their side too. The transport
//! is behind boxed halves, which lets `call` stay one body over the Unix
//! socket and the Windows pipe.

use std::collections::VecDeque;
use std::io::{BufRead as _, BufReader, Read, Write};
use std::path::Path;

use serde_json::{json, Value};

use crate::protocol::{RpcError, PROTOCOL_VERSION};

/// One connection past its handshake. Calls are strictly serial: one frame
/// out, one frame back. Pushed events (id-less frames, flowing once
/// `subscribe` has been called) can land anywhere in that rhythm; a call
/// steps over them into the pending queue, and [`next_event`](Client::next_event)
/// drains them in arrival order.
pub struct Client {
    reader: BufReader<Box<dyn Read + Send>>,
    writer: Box<dyn Write + Send>,
    next_id: u64,
    /// Events read off the wire while waiting on a response, kept in order
    /// for `next_event`.
    pending: VecDeque<(String, Value)>,
    /// What the server said it was in the handshake.
    pub server: Value,
}

impl Client {
    /// Connect and complete the version handshake. The error is a sentence
    /// for the caller's stderr: no rox listening, or a rox that speaks a
    /// different protocol generation.
    pub fn connect(path: &Path) -> Result<Client, String> {
        let (read_half, write_half) = open(path)?;
        let mut client = Client {
            reader: BufReader::new(read_half),
            writer: write_half,
            next_id: 0,
            pending: VecDeque::new(),
            server: Value::Null,
        };
        client.server = client
            .call("hello", json!({ "protocol": PROTOCOL_VERSION }))
            .map_err(|err| format!("handshake refused: {err}"))?;
        Ok(client)
    }

    /// One method call, one answer. `Err` carries the server's error
    /// object; failures of the connection itself come back as the
    /// transport code, so a holder can tell a dead socket from a refusal.
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        self.next_id += 1;
        let frame = json!({
            "jsonrpc": "2.0",
            "id": self.next_id,
            "method": method,
            "params": params,
        });
        let mut bytes = serde_json::to_vec(&frame).map_err(RpcError::app)?;
        bytes.push(b'\n');
        self.writer.write_all(&bytes).map_err(RpcError::transport)?;
        self.writer.flush().map_err(RpcError::transport)?;

        loop {
            let response = self.read_frame()?;
            // An event arriving under the call keeps its place in line for
            // next_event; the response is the frame carrying an id.
            if response.get("id").is_none() {
                self.stash(response);
                continue;
            }
            if let Some(error) = response.get("error") {
                return Err(serde_json::from_value(error.clone())
                    .unwrap_or_else(|_| RpcError::app(error.clone())));
            }
            return Ok(response.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// Block until the next pushed event and hand back its name and
    /// payload. Only useful after a `subscribe` call; events that arrived
    /// while a call waited on its response come out first, in order.
    pub fn next_event(&mut self) -> Result<(String, Value), RpcError> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Ok(event);
            }
            let frame = self.read_frame()?;
            if frame.get("id").is_none() {
                self.stash(frame);
            }
            // A frame with an id here is a response nobody is waiting on;
            // with calls strictly serial it can't happen, and dropping it
            // beats wedging the event loop over it.
        }
    }

    fn read_frame(&mut self) -> Result<Value, RpcError> {
        let mut line = String::new();
        let read = self
            .reader
            .read_line(&mut line)
            .map_err(RpcError::transport)?;
        if read == 0 {
            return Err(RpcError::transport("rox closed the connection"));
        }
        serde_json::from_str(&line).map_err(RpcError::transport)
    }

    fn stash(&mut self, frame: Value) {
        let Some(method) = frame.get("method").and_then(Value::as_str) else {
            return;
        };
        self.pending.push_back((
            method.to_owned(),
            frame.get("params").cloned().unwrap_or(Value::Null),
        ));
    }
}

/// The two halves of a fresh connection, boxed behind the traits the
/// client reads and writes through.
type Halves = (Box<dyn Read + Send>, Box<dyn Write + Send>);

#[cfg(unix)]
fn open(path: &Path) -> Result<Halves, String> {
    use std::os::unix::net::UnixStream;

    let stream = UnixStream::connect(path)
        .map_err(|err| format!("no rox listening at {}: {err}", path.display()))?;
    let write_half = stream.try_clone().map_err(|e| e.to_string())?;
    Ok((Box::new(stream), Box::new(write_half)))
}

#[cfg(windows)]
fn open(path: &Path) -> Result<Halves, String> {
    use interprocess::local_socket::traits::Stream as _;
    use interprocess::local_socket::Stream;

    let name = crate::pipe_name(path).map_err(|e| e.to_string())?;
    let stream = Stream::connect(name)
        .map_err(|err| format!("no rox listening at {}: {err}", path.display()))?;
    let (read_half, write_half) = stream.split();
    Ok((Box::new(read_half), Box::new(write_half)))
}

#[cfg(not(any(unix, windows)))]
fn open(_path: &Path) -> Result<Halves, String> {
    Err("no control socket backend on this platform".into())
}
