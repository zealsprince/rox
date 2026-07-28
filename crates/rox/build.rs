#[path = "build/identities.rs"]
mod identities;

use std::path::PathBuf;

fn main() {
    load_identities();

    // Windows resolves the taskbar and Explorer icon from a resource compiled
    // into the exe; every other platform gets it from the packaging instead.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_windows_icon();
    }
}

/// Hands the identities to the crate's compilation, from the environment when
/// it carries them and from the workspace `.env` otherwise. Cargo doesn't watch
/// env vars or stray files on its own, so both sides need declaring or a
/// rotated key comes back cached.
fn load_identities() {
    for key in identities::IDENTITY_KEYS {
        println!("cargo:rerun-if-env-changed={key}");
    }

    // Declared whether or not it exists right now: that's what makes cargo
    // rebuild when a .env first appears, not just when one changes.
    let env_file = workspace_root().join(".env");
    println!("cargo:rerun-if-changed={}", env_file.display());

    for (key, value) in identities::resolve(&env_file, |key| std::env::var(key).ok()) {
        println!("cargo:rustc-env={key}={value}");
    }
}

/// Two levels up from `crates/rox`, where `.env` and `.env.template` live.
fn workspace_root() -> PathBuf {
    let manifest =
        PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/rox sits two levels under the workspace root")
        .to_path_buf()
}

#[cfg(windows)]
fn embed_windows_icon() {
    winresource::WindowsResource::new()
        .set_icon("assets/app/rox.ico")
        .compile()
        .expect("failed to embed assets/app/rox.ico");
}

#[cfg(not(windows))]
fn embed_windows_icon() {}
