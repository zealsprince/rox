//! The wire shape: one JSON-RPC 2.0 object per line, LF-terminated, UTF-8.
//! Requests carry `id`, `method`, and optional `params`; every request gets
//! exactly one response frame, `result` or `error`, echoing the id. Pushed
//! events are sent as id-less frames between responses on a connection that
//! called `subscribe`, and the missing `id` is the whole discriminator, which
//! is why responses always carry one even when the request forgot theirs.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The protocol generation the handshake agrees on. Bumped when a change
/// breaks an existing consumer; additions (new methods, new response fields)
/// don't move it.
pub const PROTOCOL_VERSION: u32 = 1;

/// One request frame as read off the wire. `jsonrpc` is accepted and
/// ignored: the version that matters is the one the handshake carries.
#[derive(Deserialize)]
pub(crate) struct RequestFrame {
    #[serde(default)]
    pub id: Value,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// One response frame as written to the wire.
#[derive(Serialize)]
pub(crate) struct ResponseFrame {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl ResponseFrame {
    pub fn result(id: Value, result: Value) -> Self {
        ResponseFrame {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Value, error: RpcError) -> Self {
        ResponseFrame {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// One pushed event as written to the wire: a JSON-RPC notification. No id,
/// no answer expected; `method` names the event and `params` carries its
/// payload.
#[derive(Serialize)]
pub(crate) struct EventFrame<'a> {
    pub jsonrpc: &'static str,
    pub method: &'a str,
    pub params: &'a Value,
}

/// A method's failure, on the wire as JSON-RPC's error object. The reserved
/// codes keep their standard meanings; the -32000 range is ours.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl RpcError {
    pub fn parse_error(detail: impl std::fmt::Display) -> Self {
        RpcError {
            code: -32700,
            message: format!("parse error: {detail}"),
        }
    }

    pub fn invalid_request(detail: impl std::fmt::Display) -> Self {
        RpcError {
            code: -32600,
            message: format!("invalid request: {detail}"),
        }
    }

    pub fn method_not_found(method: &str) -> Self {
        RpcError {
            code: -32601,
            message: format!("method not found: {method}"),
        }
    }

    pub fn invalid_params(detail: impl std::fmt::Display) -> Self {
        RpcError {
            code: -32602,
            message: format!("invalid params: {detail}"),
        }
    }

    /// The app looked and couldn't answer: a file that isn't there, a
    /// library without a database, a track with no art.
    pub fn app(detail: impl std::fmt::Display) -> Self {
        RpcError {
            code: -32000,
            message: detail.to_string(),
        }
    }

    /// A method call arrived before `hello` settled the protocol version.
    pub fn handshake_required() -> Self {
        RpcError {
            code: -32001,
            message: "handshake required: call hello first".into(),
        }
    }

    /// The client asked for a protocol generation this build doesn't speak.
    pub fn unsupported_protocol(asked: Value) -> Self {
        RpcError {
            code: -32002,
            message: format!("unsupported protocol {asked}: this rox speaks {PROTOCOL_VERSION}"),
        }
    }

    /// The app didn't answer in time. The player is fine; the caller should
    /// retry rather than assume the command landed.
    pub fn timeout() -> Self {
        RpcError {
            code: -32003,
            message: "no answer from the app in time".into(),
        }
    }

    /// Client-side only, never sent by the server: the connection itself
    /// failed under a call. Its own code so a consumer holding a client
    /// (the MCP proxy) can tell a dead socket worth reconnecting from a
    /// method the app refused.
    pub fn transport(detail: impl std::fmt::Display) -> Self {
        RpcError {
            code: -32004,
            message: detail.to_string(),
        }
    }

    pub fn is_transport(&self) -> bool {
        self.code == -32004
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl std::error::Error for RpcError {}
