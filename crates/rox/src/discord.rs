//! Discord rich presence: the "now playing" card rox publishes to the
//! local Discord client. The build identity lives here ([`keys`]); the
//! presence client that reads it is [`crate::integrations::discord`].

pub mod keys;

/// Whether this build carries a Discord application id. Without one
/// there's nothing to connect as, so presence never arms.
pub fn has_builtin_application_id() -> bool {
    !keys::APPLICATION_ID.is_empty()
}
