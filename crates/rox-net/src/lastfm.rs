//! The signed audioscrobbler API: request signing, the one call that
//! sends them, and the connect flow's state. All of it blocks, so the
//! app runs it on the background executor. The api key and secret come
//! from the build's own identity ([`keys`]), with the settings file's
//! pair as the override for builds that ship none. The scrobbler built
//! on top of this, the part that tracks player state, is in rox.

use std::collections::BTreeMap;
use std::fmt;

pub mod keys;

/// Whether this build has its own api identity; without one the
/// settings page asks for the user's pair.
// The pair are consts baked in at compile time, so clippy can const-eval
// this and calls it a constant condition. That's exactly the question
// being asked: which build am I?
#[allow(clippy::const_is_empty)]
pub fn has_builtin_keys() -> bool {
    !keys::API_KEY.is_empty() && !keys::API_SECRET.is_empty()
}

const API_ROOT: &str = "https://ws.audioscrobbler.com/2.0/";

/// The api_sig the API requires on every signed call: the parameters
/// sorted by name, concatenated as name-value, the secret appended, md5
/// hex over the lot. `format` stays out of the signature per the docs.
fn sign(params: &BTreeMap<String, String>, secret: &str) -> String {
    let mut base = String::new();
    for (name, value) in params {
        base.push_str(name);
        base.push_str(value);
    }
    base.push_str(secret);
    format!("{:x}", md5::compute(base.as_bytes()))
}

/// A failed call: Last.fm's own error code where the service answered,
/// none where the request never got that far. The message is the part
/// worth showing; the code tells a retry from a waste of time.
pub struct ApiError {
    code: Option<i64>,
    message: String,
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl ApiError {
    /// Whether the same call could plausibly work later. A transport
    /// failure is the offline case, always worth another go. Of Last.fm's
    /// own codes only the service-side ones qualify: 8 operation failed,
    /// 11 and 16 service down or busy, 29 rate limit. A rejected session
    /// or a track it can't name comes back identical every time, so those
    /// stop where they are rather than burning the backoff.
    pub fn retryable(&self) -> bool {
        match self.code {
            None => true,
            Some(code) => matches!(code, 8 | 11 | 16 | 29),
        }
    }

    /// Whether Last.fm refused the session itself (code 9), the one
    /// failure that means the stored key is worth nothing to this build:
    /// revoked on the site, or minted under a different api key. Every
    /// call this build makes fails the same way until it reconnects, so
    /// the answer is worth acting on rather than logging.
    pub fn session_rejected(&self) -> bool {
        self.code == Some(9)
    }
}

/// One signed API call, blocking: POST the parameters, parse the JSON,
/// surface the API's own error message when it sends one. Runs on the
/// background executor only.
pub fn call(
    method: &str,
    secret: &str,
    mut params: BTreeMap<String, String>,
) -> Result<serde_json::Value, ApiError> {
    params.insert("method".into(), method.into());
    let sig = sign(&params, secret);
    params.insert("api_sig".into(), sig);
    params.insert("format".into(), "json".into());
    let pairs: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    // A request that never reached the service, or a body that won't read
    // or parse, gets no code: the next try may well go through, so these
    // count as retryable rather than as a rejection.
    let transport = |message: String| ApiError {
        code: None,
        message,
    };
    // An API error still has a JSON body worth reading, so a status
    // failure parses like a success. Use the shared provider agent for its
    // User-Agent and timeout; a bare ureq::post has neither, so a hung endpoint
    // parks the connect flow in Confirming forever.
    let text = match crate::providers::agent().post(API_ROOT).send_form(&pairs) {
        Ok(response) => response
            .into_string()
            .map_err(|e| transport(e.to_string()))?,
        Err(ureq::Error::Status(_, response)) => response
            .into_string()
            .map_err(|e| transport(e.to_string()))?,
        Err(e) => return Err(transport(crate::providers::net_reason(&e))),
    };
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| transport(e.to_string()))?;
    if let Some(code) = value.get("error") {
        let message = value
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown api error");
        return Err(ApiError {
            code: code.as_i64(),
            message: message.to_string(),
        });
    }
    Ok(value)
}

/// Where the connect flow stands, for the settings window's readout.
/// Connected is not a phase: a session filed under this build's api key
/// is.
#[derive(Clone, PartialEq)]
pub enum AuthPhase {
    Idle,
    /// auth.getToken is in flight.
    Requesting,
    /// The browser has the authorize page; the token waits for the user
    /// to come back and finish.
    Waiting(String),
    /// auth.getSession is in flight.
    Confirming,
    /// Last.fm refused the session this build was holding, so it was
    /// dropped. Its own phase rather than a `Failed`: nothing the user
    /// did failed, and the fix is a plain reconnect.
    Rejected,
    Failed(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_service_side_failures_are_worth_another_try() {
        let api = |code: i64| ApiError {
            code: Some(code),
            message: "api said no".to_string(),
        };
        // No code at all is the offline case: the request never reached
        // the service.
        assert!(ApiError {
            code: None,
            message: "no connection".to_string(),
        }
        .retryable());
        assert!(api(11).retryable(), "service offline");
        assert!(api(16).retryable(), "temporarily unavailable");
        assert!(api(29).retryable(), "rate limited");
        assert!(!api(9).retryable(), "invalid session, and it stays invalid");
        assert!(!api(6).retryable(), "a track Last.fm can't name");
    }

    #[test]
    fn only_code_nine_condemns_the_session() {
        let api = |code: Option<i64>| ApiError {
            code,
            message: "api said no".to_string(),
        };
        assert!(api(Some(9)).session_rejected());
        assert!(
            !api(Some(6)).session_rejected(),
            "the track, not the session"
        );
        assert!(
            !api(None).session_rejected(),
            "offline says nothing about it"
        );
    }
}
