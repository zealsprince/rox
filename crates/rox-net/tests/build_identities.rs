//! Runs the build script's identity resolution against `cargo test`. The
//! module is the same source build.rs compiles; the tests live beside the
//! logic in it.

#[path = "../build/identities.rs"]
mod identities;
