//! The ListenBrainz submission API: the two calls rox makes and their
//! payloads. The user token is the whole credential, sent as an
//! `Authorization: Token ...` header. What counts as a listen, and retries,
//! live in the service on top.

use std::fmt;

use serde::Serialize;

const API_ROOT: &str = "https://api.listenbrainz.org/1/";

const CLIENT: &str = "rox";

pub struct ApiError {
    pub status: Option<u16>,
    pub message: String,
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl ApiError {
    /// No status (offline), 429, and 5xx are worth retrying.
    pub fn retryable(&self) -> bool {
        match self.status {
            None => true,
            Some(429) => true,
            Some(code) => (500..600).contains(&code),
        }
    }

    /// A refused token fails every call until it's replaced, so it goes on
    /// screen rather than in the log.
    pub fn token_rejected(&self) -> bool {
        self.status == Some(401)
    }
}

#[derive(Serialize, Clone)]
pub struct Listen {
    /// Omitted for `playing_now`, which rejects a timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub listened_at: Option<u64>,
    pub track_metadata: TrackMetadata,
}

#[derive(Serialize, Clone)]
pub struct TrackMetadata {
    pub artist_name: String,
    pub track_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_name: Option<String>,
    pub additional_info: AdditionalInfo,
}

/// `duration_ms` lets ListenBrainz tell a full play of a short track from a
/// skip through a long one.
#[derive(Serialize, Clone)]
pub struct AdditionalInfo {
    pub media_player: &'static str,
    pub submission_client: &'static str,
    pub submission_client_version: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

impl Default for AdditionalInfo {
    fn default() -> Self {
        AdditionalInfo {
            media_player: CLIENT,
            submission_client: CLIENT,
            submission_client_version: env!("CARGO_PKG_VERSION"),
            duration_ms: None,
        }
    }
}

impl Listen {
    /// An empty album is omitted: a blank release name is worse than none.
    pub fn new(
        artist: String,
        title: String,
        album: String,
        duration_secs: Option<f64>,
        listened_at: Option<u64>,
    ) -> Self {
        Listen {
            listened_at,
            track_metadata: TrackMetadata {
                artist_name: artist,
                track_name: title,
                release_name: (!album.is_empty()).then_some(album),
                additional_info: AdditionalInfo {
                    duration_ms: duration_secs
                        .filter(|d| *d > 0.0)
                        .map(|d| (d * 1000.0).round() as u64),
                    ..AdditionalInfo::default()
                },
            },
        }
    }
}

/// Some(name) for a valid token, None for an invalid one. A 401 is the
/// service's other way of saying invalid, so it folds into None.
pub fn validate_token(token: &str) -> Result<Option<String>, ApiError> {
    let value = match request(agent_get("validate-token"), token, None) {
        Ok(value) => value,
        Err(e) if e.token_rejected() => return Ok(None),
        Err(e) => return Err(e),
    };
    if value.get("valid").and_then(|v| v.as_bool()) != Some(true) {
        return Ok(None);
    }
    Ok(Some(
        value
            .get("user_name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
    ))
}

/// `listen_type` is `"single"`, `"playing_now"`, or `"import"`.
pub fn submit(token: &str, listen_type: &str, payload: &[Listen]) -> Result<(), ApiError> {
    let body = serde_json::json!({ "listen_type": listen_type, "payload": payload });
    request(agent_post("submit-listens"), token, Some(body)).map(|_| ())
}

fn agent_get(path: &str) -> ureq::Request {
    crate::providers::agent().get(&format!("{API_ROOT}{path}"))
}

fn agent_post(path: &str) -> ureq::Request {
    crate::providers::agent().post(&format!("{API_ROOT}{path}"))
}

/// A status error still carries a JSON body with the service's `error` string.
fn request(
    request: ureq::Request,
    token: &str,
    body: Option<serde_json::Value>,
) -> Result<serde_json::Value, ApiError> {
    let request = request.set("Authorization", &format!("Token {token}"));
    // Sent as a string: send_json needs ureq's json feature, which this crate doesn't take.
    let sent = match body {
        Some(body) => request
            .set("Content-Type", "application/json")
            .send_string(&body.to_string()),
        None => request.call(),
    };
    // Never stringify a ureq error directly: its Display prints the full URL,
    // which is the leak net_reason stops.
    let transport = |message: String| ApiError {
        status: None,
        message,
    };
    let (status, text) = match sent {
        Ok(response) => (
            None,
            response
                .into_string()
                .map_err(|e| transport(e.to_string()))?,
        ),
        Err(ureq::Error::Status(code, response)) => (
            Some(code),
            response
                .into_string()
                .map_err(|e| transport(e.to_string()))?,
        ),
        Err(e) => return Err(transport(crate::providers::net_reason(&e))),
    };
    let value: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
    if let Some(status) = status {
        let message = value
            .get("error")
            .and_then(|e| e.as_str())
            .map(|e| e.to_string())
            .unwrap_or_else(|| format!("service returned {status}"));
        return Err(ApiError {
            status: Some(status),
            message,
        });
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_playing_now_listen_carries_no_timestamp() {
        let listen = Listen::new(
            "Boards of Canada".into(),
            "Roygbiv".into(),
            "Music Has the Right to Children".into(),
            Some(151.0),
            None,
        );
        let json = serde_json::to_value(&listen).unwrap();
        assert!(
            json.get("listened_at").is_none(),
            "the API rejects a timestamp on playing_now: {json}"
        );
        assert_eq!(
            json["track_metadata"]["release_name"],
            serde_json::json!("Music Has the Right to Children")
        );
    }

    #[test]
    fn a_played_listen_names_rox_and_its_length() {
        let listen = Listen::new(
            "Boards of Canada".into(),
            "Roygbiv".into(),
            String::new(),
            Some(151.4),
            Some(1_700_000_000),
        );
        let json = serde_json::to_value(&listen).unwrap();
        assert_eq!(json["listened_at"], serde_json::json!(1_700_000_000u64));
        let info = &json["track_metadata"]["additional_info"];
        assert_eq!(info["media_player"], serde_json::json!("rox"));
        assert_eq!(info["submission_client"], serde_json::json!("rox"));
        assert_eq!(info["duration_ms"], serde_json::json!(151_400u64));
        assert!(
            json["track_metadata"].get("release_name").is_none(),
            "{json}"
        );
    }

    #[test]
    fn only_service_side_failures_are_worth_another_try() {
        let api = |status: Option<u16>| ApiError {
            status,
            message: "service said no".to_string(),
        };
        assert!(api(None).retryable(), "the request never landed");
        assert!(api(Some(429)).retryable(), "rate limited");
        assert!(api(Some(503)).retryable(), "service unavailable");
        assert!(!api(Some(400)).retryable(), "a payload it will never take");
        assert!(!api(Some(401)).retryable(), "a token that stays refused");
    }

    #[test]
    fn only_a_401_condemns_the_token() {
        let api = |status: Option<u16>| ApiError {
            status,
            message: "service said no".to_string(),
        };
        assert!(api(Some(401)).token_rejected());
        assert!(
            !api(Some(400)).token_rejected(),
            "the payload, not the token"
        );
        assert!(
            !api(None).token_rejected(),
            "offline says nothing about the token"
        );
    }
}
