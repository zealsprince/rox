fn main() {
    // Windows resolves the taskbar and Explorer icon from a resource compiled
    // into the exe; every other platform gets it from the packaging instead.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_windows_icon();
    }
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
