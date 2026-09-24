#[path = "build/identities.rs"]
mod identities;

use std::path::PathBuf;

fn main() {
    load_identities();
}

/// Cargo watches neither env vars nor stray files on its own, so both need
/// declaring or a rotated key comes back cached.
fn load_identities() {
    for key in identities::IDENTITY_KEYS {
        println!("cargo:rerun-if-env-changed={key}");
    }

    // Declared even when absent, so cargo rebuilds when a .env first
    // appears.
    let env_file = workspace_root().join(".env");
    println!("cargo:rerun-if-changed={}", env_file.display());

    for (key, value) in identities::resolve(&env_file, |key| std::env::var(key).ok()) {
        println!("cargo:rustc-env={key}={value}");
    }
}

fn workspace_root() -> PathBuf {
    let manifest =
        PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/rox-net sits two levels under the workspace root")
        .to_path_buf()
}
