//! The signed audioscrobbler API: request signing, the one call that sends
//! it, and the connect flow's state. Libre.fm serves the same protocol, so
//! the call takes its root as an argument. The unsigned public-profile
//! reads the imports use live in [`user`].

use std::collections::BTreeMap;
use std::fmt;

pub mod keys;
pub mod user;

/// Whether this build has its own api identity.
// Clippy const-evals the baked pair and calls this a constant condition.
// That's the point: which build am I?
#[allow(clippy::const_is_empty)]
pub fn has_builtin_keys() -> bool {
    !keys::API_KEY.is_empty() && !keys::API_SECRET.is_empty()
}

const API_ROOT: &str = "https://ws.audioscrobbler.com/2.0/";

/// `ROX_LASTFM_API_ROOT` overrides the root in debug builds only, for
/// testing imports against a stand-in server. Release builds never read it.
pub fn api_root() -> String {
    override_root().unwrap_or_else(|| API_ROOT.to_string())
}

#[cfg(debug_assertions)]
fn override_root() -> Option<String> {
    std::env::var("ROX_LASTFM_API_ROOT")
        .ok()
        .filter(|root| !root.is_empty())
}

#[cfg(not(debug_assertions))]
fn override_root() -> Option<String> {
    None
}

/// md5 over the params sorted by name, name-value concatenated, secret appended.
fn sign(params: &BTreeMap<String, String>, secret: &str) -> String {
    let mut base = String::new();
    for (name, value) in params {
        base.push_str(name);
        base.push_str(value);
    }
    base.push_str(secret);
    format!("{:x}", md5::compute(base.as_bytes()))
}

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
    /// Transport failures and Last.fm's service-side codes (8 operation
    /// failed, 11/16 down or busy, 29 rate limit) are worth retrying.
    pub fn retryable(&self) -> bool {
        match self.code {
            None => true,
            Some(code) => matches!(code, 8 | 11 | 16 | 29),
        }
    }

    /// Code 9: the stored session is dead for this build (revoked, or minted
    /// under another api key).
    pub fn session_rejected(&self) -> bool {
        self.code == Some(9)
    }
}

pub fn call(
    method: &str,
    secret: &str,
    params: BTreeMap<String, String>,
) -> Result<serde_json::Value, ApiError> {
    call_at(&api_root(), method, secret, params)
}

/// [`call`] against any host that speaks the protocol.
pub fn call_at(
    root: &str,
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
    // No code means the request never landed, which is retryable.
    let transport = |message: String| ApiError {
        code: None,
        message,
    };
    // A status failure still carries a JSON error body. The shared agent's
    // timeout matters: a bare ureq::post can hang the connect flow forever.
    let text = match crate::providers::agent().post(root).send_form(&pairs) {
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

/// Where the connect flow stands. Connected isn't a phase: a session filed
/// under this build's api key is.
#[derive(Clone, PartialEq)]
pub enum AuthPhase {
    Idle,
    Requesting,
    Waiting(String),
    Confirming,
    /// Last.fm refused the held session. Not `Failed`: the fix is a reconnect.
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
        assert!(
            ApiError {
                code: None,
                message: "no connection".to_string(),
            }
            .retryable()
        );
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
