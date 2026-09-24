//! Discord rich presence's build identity. The presence client is in
//! rox-services.

pub mod keys;

/// Whether this build has a Discord application id; without one presence
/// never arms.
// Clippy const-evals the baked id and calls this a constant condition.
// That's the point: which build am I?
#[allow(clippy::const_is_empty)]
pub fn has_builtin_application_id() -> bool {
    !keys::APPLICATION_ID.is_empty()
}
