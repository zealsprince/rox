//! Builds the vendored libprojectM with cmake and links it statically. GL
//! isn't linked on Linux or macOS: projectM resolves every entry point
//! through its vendored glad with the load proc we hand it. Windows still
//! needs opengl32 for the loader itself.

use std::env;
use std::path::{Path, PathBuf};

fn main() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vendor/projectm");
    if !source.join("CMakeLists.txt").exists() {
        panic!(
            "vendored libprojectM is missing at {}. Run ./scripts/vendor-projectm.sh from the \
             repo root first; the nix dev shell runs it for you on entry.",
            source.display()
        );
    }

    // The stamp covers the pinned commits and patches; watching the tree
    // would walk thousands of files on every cargo invocation.
    println!(
        "cargo:rerun-if-changed={}",
        source.join(".rox-stamp").display()
    );

    let dst = cmake::Config::new(&source)
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("ENABLE_PLAYLIST", "OFF")
        .define("ENABLE_SDL_UI", "OFF")
        .define("BUILD_TESTING", "OFF")
        // No system copy on any of our platforms; vendoring keeps vcpkg out.
        .define("ENABLE_SYSTEM_GLM", "OFF")
        .define("ENABLE_SYSTEM_PROJECTM_EVAL", "OFF")
        .define("ENABLE_GLES", "OFF")
        // Release regardless of profile: a debug projectM misses frames at
        // 60fps, and it keeps ENABLE_DEBUG_POSTFIX from renaming libs to *d.
        .profile("Release")
        .build();

    // GNUInstallDirs picks lib64 on some distributions and lib elsewhere.
    println!("cargo:rustc-link-search=native={}/lib", dst.display());
    println!("cargo:rustc-link-search=native={}/lib64", dst.display());

    // One archive: the static build folds projectm-eval in through
    // TARGET_OBJECTS. It's libprojectM-4.a, or libprojectM-4.lib on Windows
    // (CMake forces the "lib" prefix), and rustc's `static=projectM-4` only
    // finds projectM-4.lib on MSVC, so the file is named verbatim.
    let mut archive = None;
    for dir in ["lib", "lib64"] {
        for name in ["libprojectM-4.a", "libprojectM-4.lib", "projectM-4.lib"] {
            if dst.join(dir).join(name).exists() {
                archive.get_or_insert(name);
            }
        }
    }
    let archive = archive.unwrap_or_else(|| {
        panic!(
            "libprojectM built but no archive was found under {}/lib or lib64",
            dst.display()
        )
    });
    println!("cargo:rustc-link-lib=static:+verbatim={archive}");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    match target_os.as_str() {
        // projectM is C++, and rustc links neither standard library for us.
        "macos" | "ios" => println!("cargo:rustc-link-lib=dylib=c++"),
        "windows" if target_env == "msvc" => {
            // MSVC's runtime comes in through the linker defaults.
            println!("cargo:rustc-link-lib=dylib=opengl32");
        }
        _ => println!("cargo:rustc-link-lib=dylib=stdc++"),
    }

    println!(
        "cargo:include={}",
        Path::new(&dst).join("include").display()
    );
    println!("cargo:root={}", dst.display());
}
