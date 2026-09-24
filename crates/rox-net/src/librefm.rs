//! Libre.fm: Last.fm's protocol at a different host. The service accepts
//! any key pair, so rox signs with a fixed one of its own and there's
//! nothing to file sessions under the way ADR 26 does for Last.fm. The
//! signing and error handling are [`crate::lastfm`]'s.

use std::collections::BTreeMap;

use crate::lastfm::{self, ApiError};

const API_ROOT: &str = "https://libre.fm/2.0/";

/// The authorize page, with `api_key` and `token` as its query.
pub const AUTH_URL: &str = "https://libre.fm/api/auth/";

/// Arbitrary: the service registers nothing, so a secret in the source is
/// fine.
pub const API_KEY: &str = "5am5ijte60yp01u33a0xnvxx3tsjb4m3";
const API_SECRET: &str = "92vhtr86swzvnhonisiikwgbb403nutl";

pub fn call(method: &str, params: BTreeMap<String, String>) -> Result<serde_json::Value, ApiError> {
    lastfm::call_at(API_ROOT, method, API_SECRET, params)
}
