//! A small blocking client over the socket, for the CLI and the MCP proxy.
//! The transport sits behind boxed halves so `call` is one body on Unix
//! sockets and Windows pipes.

use std::collections::VecDeque;
use std::io::{BufRead as _, BufReader, Read, Write};
use std::path::Path;

use serde_json::{Value, json};

use crate::protocol::{PROTOCOL_VERSION, RpcError};

/// Calls are strictly serial. Pushed events that arrive under a call are
/// queued for [`next_event`](Client::next_event) in arrival order.
pub struct Client {
    reader: BufReader<Box<dyn Read + Send>>,
    writer: Box<dyn Write + Send>,
    next_id: u64,
    pending: VecDeque<(String, Value)>,
    pub server: Value,
}

impl Client {
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

    /// Connection failures come back as the transport code, so a holder can
    /// tell a dead socket from a refusal.
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

    pub fn next_event(&mut self) -> Result<(String, Value), RpcError> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Ok(event);
            }
            let frame = self.read_frame()?;
            if frame.get("id").is_none() {
                self.stash(frame);
            }
            // A stray response can't happen with serial calls; drop it.
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
    use interprocess::local_socket::Stream;
    use interprocess::local_socket::traits::Stream as _;

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
