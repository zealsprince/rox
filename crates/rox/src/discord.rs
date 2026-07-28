//! Discord rich presence: the "now playing" card rox publishes to the
//! local Discord client. Only the build identity lives here so far
//! ([`keys`]); the presence client itself is still to come, and until it
//! exists nothing reads these.

// The presence client is the consumer these are waiting on.
#![allow(dead_code)]

pub mod keys;

/// Whether this build carries a Discord application id. Without one
/// there's nothing to connect as, so presence never arms.
pub fn has_builtin_client_id() -> bool {
    !keys::CLIENT_ID.is_empty()
}
