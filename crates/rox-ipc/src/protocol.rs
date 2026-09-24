//! The wire shape: one JSON-RPC 2.0 object per line. Every request gets
//! exactly one response echoing its id; pushed events are id-less frames, so
//! the missing `id` is the whole discriminator.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Bumped only for breaking changes; additions don't move it.
pub const PROTOCOL_VERSION: u32 = 1;

/// `jsonrpc` is ignored: the handshake carries the version that matters.
#[derive(Deserialize)]
pub(crate) struct RequestFrame {
    #[serde(default)]
    pub id: Value,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

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

#[derive(Serialize)]
pub(crate) struct EventFrame<'a> {
    pub jsonrpc: &'static str,
    pub method: &'a str,
    pub params: &'a Value,
}

/// Reserved JSON-RPC codes keep their meanings; the -32000 range is ours.
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

    pub fn app(detail: impl std::fmt::Display) -> Self {
        RpcError {
            code: -32000,
            message: detail.to_string(),
        }
    }

    pub fn handshake_required() -> Self {
        RpcError {
            code: -32001,
            message: "handshake required: call hello first".into(),
        }
    }

    pub fn unsupported_protocol(asked: Value) -> Self {
        RpcError {
            code: -32002,
            message: format!("unsupported protocol {asked}: this rox speaks {PROTOCOL_VERSION}"),
        }
    }

    /// The player is fine; the caller should retry rather than assume it landed.
    pub fn timeout() -> Self {
        RpcError {
            code: -32003,
            message: "no answer from the app in time".into(),
        }
    }

    /// Client-side only: the connection failed under a call, worth
    /// reconnecting.
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
