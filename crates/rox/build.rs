fn main() {
    // Windows reads its icon from the exe; packaging supplies it elsewhere.
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
