//! The self-updater, the download half of the update story
//! ([`updates`](crate::startup::updates) is the check half). It resolves this
//! platform's artifact, verifies it against the release's SHA256SUMS.txt,
//! stages it beside the running build, and swaps it in: rename-over on Linux,
//! rename-aside on Windows where a running exe can't be replaced, a bundle
//! swap on macOS. The running process keeps its old build until a restart.
//!
//! ## What can update
//!
//! Only an install that owns its folder: the write probe in [`can_update`]
//! is the gate, so a distro package, a nix store path, or any other
//! read-only home stays notify-only, as does a platform the release workflow
//! doesn't build for.
//!
//! ## The AppImage
//!
//! `current_exe()` is the read-only squashfs mount, so the target is the
//! file `$APPIMAGE` names, never the mount. The restart can't go through
//! gpui's, which would re-run the replaced mount; [`relaunch`] waits for
//! this pid and execs the .AppImage instead.
//!
//! ## Why a failed download can't hurt
//!
//! Everything before the swap happens in the OS temp dir, and the swap only
//! runs after the checksum matches. The only writes in the install folder
//! are renames, and the Windows dance rolls back if its second rename fails.
//! [`clean_leftovers`] sweeps the remains at the next launch.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::startup::updates::{self, Release};

/// Matches release.yml's matrix. None leaves the check notify-only.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const PLATFORM: Option<&str> = Some("linux-x86_64.tar.gz");
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const PLATFORM: Option<&str> = Some("macos-aarch64.dmg");
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const PLATFORM: Option<&str> = Some("windows-x86_64.zip");
#[cfg(not(any(
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "windows", target_arch = "x86_64"),
)))]
const PLATFORM: Option<&str> = None;

fn platform() -> Option<&'static str> {
    platform_for(rox_core::install::appimage())
}

/// Takes the AppImage answer as an argument so tests can hand one in.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn platform_for(appimage: Option<&Path>) -> Option<&'static str> {
    if appimage.is_some() {
        return Some("linux-x86_64.AppImage");
    }

    PLATFORM
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
fn platform_for(_appimage: Option<&Path>) -> Option<&'static str> {
    PLATFORM
}

const SUMS: &str = "SHA256SUMS.txt";

fn work_dir() -> PathBuf {
    std::env::temp_dir().join("rox-update")
}

/// One global slot: at most one download per run.
#[derive(Clone)]
pub enum Status {
    Idle,
    Downloading(Arc<Progress>),
    /// The new build is on disk; a restart runs it.
    Applied {
        version: String,
    },
    Failed {
        error: String,
    },
}

static STATE: Mutex<Status> = Mutex::new(Status::Idle);

pub fn status() -> Status {
    STATE.lock().unwrap().clone()
}

/// No cancel, unlike the model downloader's: an unwanted download costs
/// nothing and quitting kills it anyway.
#[derive(Default)]
pub struct Progress {
    done: AtomicU64,
    total: AtomicU64,
}

impl Progress {
    pub fn fraction(&self) -> f32 {
        let total = self.total.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        (self.done.load(Ordering::Relaxed) as f32 / total as f32).clamp(0.0, 1.0)
    }
}

/// Probed once per run: the executable's location doesn't move mid-run.
pub fn can_update() -> bool {
    static CAN: OnceLock<bool> = OnceLock::new();
    *CAN.get_or_init(|| platform().is_some() && install_writable())
}

/// Under an AppImage, gpui's restart would run the replaced mount and paste
/// the path into bash unquoted, which breaks on a space. So spawn our own
/// wait-then-exec and quit.
pub fn relaunch(cx: &mut gpui::App) {
    #[cfg(target_os = "linux")]
    {
        if let Some(appimage) = rox_core::install::appimage() {
            match spawn_relauncher(std::process::id(), appimage) {
                Ok(()) => cx.quit(),
                Err(e) => log::error!("update: can't spawn the relauncher: {e}"),
            }
            return;
        }
    }

    cx.restart();
}

/// Both values arrive as positional arguments, so the shell never parses
/// the path.
#[cfg(target_os = "linux")]
fn spawn_relauncher(pid: u32, appimage: &Path) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt as _;
    use std::process::{Command, Stdio};

    Command::new("/bin/sh")
        .arg("-c")
        .arg("while kill -0 \"$1\" 2>/dev/null; do sleep 0.1; done; exec \"$2\"")
        .arg("sh")
        .arg(pid.to_string())
        .arg(appimage)
        // Its own group and no inherited stdio, so it outlives this process.
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(drop)
}

/// The claim happens on the caller's thread, so the UI sees Downloading
/// at once.
pub fn begin(release: &Release) -> Option<impl FnOnce() + Send + 'static + use<>> {
    let progress = Arc::new(Progress::default());
    {
        let mut state = STATE.lock().unwrap();
        match *state {
            Status::Downloading(_) | Status::Applied { .. } => return None,
            _ => *state = Status::Downloading(progress.clone()),
        }
    }
    let release = release.clone();
    Some(move || {
        let outcome = download_and_apply(&release, &progress);
        let mut state = STATE.lock().unwrap();
        *state = match outcome {
            Ok(version) => {
                log::info!("update: {version} applied, a restart runs it");
                Status::Applied { version }
            }
            Err(error) => {
                log::warn!("update: {error}");
                Status::Failed { error }
            }
        };
    })
}

/// Sweep the temp dir and the `.old`/`.new` rename-aside remains. Called at
/// launch.
pub fn clean_leftovers() {
    let _ = std::fs::remove_dir_all(work_dir());
    if let Ok(target) = install_target() {
        remove_any(&sibling(&target, "new"));
        remove_any(&sibling(&target, "old"));
        #[cfg(not(target_os = "macos"))]
        {
            let helper = target.with_file_name(helper_name());
            remove_any(&sibling(&helper, "new"));
            remove_any(&sibling(&helper, "old"));
        }
    }
}

fn download_and_apply(release: &Release, progress: &Progress) -> Result<String, String> {
    // A release rebuilt from the settings cache has no asset list.
    let release = if release.assets.is_empty() {
        updates::fetch_latest()?
    } else {
        release.clone()
    };
    if !release.is_new() {
        return Err(rox_i18n::t!("updater-already-latest").to_string());
    }
    let archive = fetch_verified(&release, progress)?;
    let applied = apply(&archive);
    let _ = std::fs::remove_file(&archive);
    applied.map(|()| release.version.clone())
}

fn fetch_verified(release: &Release, progress: &Progress) -> Result<PathBuf, String> {
    let platform = platform().ok_or_else(|| rox_i18n::t_static("updater-no-release-build"))?;
    let name = format!("rox-v{}-{platform}", release.version);
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == name)
        .ok_or_else(|| rox_i18n::t!("updater-no-asset", name = name.clone()).to_string())?;
    let sums = release
        .assets
        .iter()
        .find(|a| a.name == SUMS)
        .ok_or_else(|| rox_i18n::t!("updater-no-checksums", sums = SUMS.to_string()).to_string())?;

    let manifest = fetch_sums(&sums.url)?;
    let expected = expected_sum(&manifest, &name).ok_or_else(|| {
        rox_i18n::t!(
            "updater-checksum-missing-entry",
            sums = SUMS.to_string(),
            name = name.clone()
        )
        .to_string()
    })?;

    let dir = work_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let archive = dir.join(&name);
    download(&asset.url, asset.bytes, &expected, &archive, progress)?;
    Ok(archive)
}

/// Not `rox_net::providers::agent`: its ten-second total cap is fatal for
/// a large download. Bound the connect and each read instead.
fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .user_agent(concat!(
                "rox/",
                env!("CARGO_PKG_VERSION"),
                " (https://github.com/zealsprince/rox)"
            ))
            .timeout_connect(Duration::from_secs(15))
            .timeout_read(Duration::from_secs(30))
            .build()
    })
}

fn fetch_sums(url: &str) -> Result<String, String> {
    agent()
        .get(url)
        .call()
        .map_err(|e| rox_net::providers::net_reason(&e))?
        .into_string()
        .map_err(|e| e.to_string())
}

/// `sha256sum` writes `<hex>  <name>`, with `*` on the name in binary mode.
fn expected_sum(manifest: &str, name: &str) -> Option<String> {
    manifest.lines().find_map(|line| {
        let mut tokens = line.split_whitespace();
        let hash = tokens.next()?;
        let file = tokens.next_back().unwrap_or(hash);
        (file.trim_start_matches('*') == name && hash.len() == 64)
            .then(|| hash.to_ascii_lowercase())
    })
}

/// Rename to `path` only once size and checksum match; the part file is
/// removed on any failure.
fn download(
    url: &str,
    bytes: u64,
    expected: &str,
    path: &Path,
    progress: &Progress,
) -> Result<(), String> {
    progress.total.store(bytes, Ordering::Relaxed);
    let response = agent()
        .get(url)
        .call()
        .map_err(|e| rox_net::providers::net_reason(&e))?;
    // A redirect to an error page shows up here as a wildly different size.
    if let Some(claimed) = response
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok())
        && claimed != bytes
    {
        return Err(
            rox_i18n::t!("updater-size-mismatch", claimed = claimed, bytes = bytes).to_string(),
        );
    }
    let part = path.with_extension("part");
    let outcome = stream(response.into_reader(), &part, bytes, expected, progress);
    match outcome {
        Ok(()) => std::fs::rename(&part, path).map_err(|e| format!("{}: {e}", path.display())),
        Err(reason) => {
            let _ = std::fs::remove_file(&part);
            Err(reason)
        }
    }
}

fn stream(
    mut body: impl std::io::Read,
    part: &Path,
    bytes: u64,
    expected: &str,
    progress: &Progress,
) -> Result<(), String> {
    use std::io::Write;
    let mut out = std::io::BufWriter::new(
        std::fs::File::create(part).map_err(|e| format!("{}: {e}", part.display()))?,
    );
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut done: u64 = 0;
    loop {
        let read = body.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        // Never write past the stated size, so an endless stream can't fill the disk.
        done += read as u64;
        if done > bytes {
            return Err(rox_i18n::t!("updater-overran").to_string());
        }
        hasher.update(&buffer[..read]);
        out.write_all(&buffer[..read])
            .map_err(|e| format!("{}: {e}", part.display()))?;
        progress.done.store(done, Ordering::Relaxed);
    }
    out.flush().map_err(|e| e.to_string())?;
    if done != bytes {
        return Err(rox_i18n::t!("updater-short", done = done, bytes = bytes).to_string());
    }
    let digest = hex(&hasher.finalize());
    if digest != expected {
        return Err(rox_i18n::t!(
            "updater-checksum-mismatch",
            digest = digest,
            expected = expected.to_string()
        )
        .to_string());
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// On macOS the whole bundle, since a build is the whole bundle.
#[cfg(not(target_os = "macos"))]
fn install_target() -> Result<PathBuf, String> {
    // The .AppImage file, never its read-only mount.
    if let Some(appimage) = rox_core::install::appimage() {
        return Ok(appimage.to_path_buf());
    }

    std::env::current_exe().map_err(|e| format!("can't locate the running executable: {e}"))
}

#[cfg(target_os = "macos")]
fn install_target() -> Result<PathBuf, String> {
    bundle_root().ok_or_else(|| "not running from an app bundle".into())
}

/// None for a bare binary, which stays notify-only.
#[cfg(target_os = "macos")]
fn bundle_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .find(|dir| dir.extension().is_some_and(|ext| ext == "app"))
        .map(Path::to_path_buf)
}

/// `rox.exe` to `rox.exe.new`: the suffix goes after the extension.
fn sibling(target: &Path, suffix: &str) -> PathBuf {
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    target.with_file_name(format!("{name}.{suffix}"))
}

fn remove_any(path: &Path) {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return;
    };
    let _ = if meta.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
}

/// Probed with a real file: permission bits aren't reliable across platforms.
#[cfg(not(target_os = "macos"))]
fn install_writable() -> bool {
    rox_core::settings::portable_available()
}

/// The swap renames the bundle, so probe the folder holding it.
#[cfg(target_os = "macos")]
fn install_writable() -> bool {
    let Some(dir) = bundle_root().and_then(|app| app.parent().map(Path::to_path_buf)) else {
        return false;
    };
    let probe = dir.join(".rox-write-probe");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// The bundle contains rox-mcp, so one swap covers both.
#[cfg(target_os = "macos")]
fn apply(archive: &Path) -> Result<(), String> {
    let target = install_target()?;
    let staged = sibling(&target, "new");
    remove_any(&staged);
    stage(archive, &staged)?;
    swap(&staged, &target)
}

/// The app and the rox-mcp proxy beside it. Both stage before anything
/// moves, and the helper swaps first so a failure leaves the app untouched.
#[cfg(not(target_os = "macos"))]
fn apply(archive: &Path) -> Result<(), String> {
    let target = install_target()?;

    #[cfg(target_os = "linux")]
    {
        if rox_core::install::appimage().is_some() {
            return apply_appimage(archive, &target);
        }
    }

    let binary = format!("rox{}", std::env::consts::EXE_SUFFIX);
    let helper_target = target.with_file_name(helper_name());
    let staged = sibling(&target, "new");
    let helper_staged = sibling(&helper_target, "new");
    remove_any(&staged);
    remove_any(&helper_staged);
    if !stage(archive, &binary, &staged)? {
        return Err(format!("the archive holds no {binary}"));
    }
    // Absent only in archives from before the proxy shipped.
    if stage(archive, &helper_name(), &helper_staged)? {
        swap(&helper_staged, &helper_target)?;
    }
    swap(&staged, &target)
}

#[cfg(target_os = "linux")]
fn apply_appimage(archive: &Path, target: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let staged = sibling(target, "new");
    remove_any(&staged);
    std::fs::copy(archive, &staged).map_err(|e| format!("{}: {e}", staged.display()))?;
    std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("{}: {e}", staged.display()))?;

    // Synced before the rename, so a power cut can't leave the name on a file
    // whose bytes never reached the disk.
    std::fs::File::open(&staged)
        .and_then(|file| file.sync_all())
        .map_err(|e| format!("{}: {e}", staged.display()))?;

    swap(&staged, target)
}

/// Same shape the MCP settings page hands out.
#[cfg(not(target_os = "macos"))]
fn helper_name() -> String {
    format!("rox-mcp{}", std::env::consts::EXE_SUFFIX)
}

/// False when the archive doesn't contain it.
#[cfg(target_os = "linux")]
fn stage(archive: &Path, name: &str, staged: &Path) -> Result<bool, String> {
    use std::os::unix::fs::PermissionsExt;
    let file = std::fs::File::open(archive).map_err(|e| format!("{}: {e}", archive.display()))?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
    for entry in tar.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let is_binary = entry.header().entry_type().is_file()
            && entry
                .path()
                .is_ok_and(|p| p.file_name().is_some_and(|n| n == name));
        if !is_binary {
            continue;
        }
        let mut out =
            std::fs::File::create(staged).map_err(|e| format!("{}: {e}", staged.display()))?;
        std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
        out.sync_all().map_err(|e| e.to_string())?;
        std::fs::set_permissions(staged, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
        return Ok(true);
    }
    Ok(false)
}

#[cfg(windows)]
fn stage(archive: &Path, name: &str, staged: &Path) -> Result<bool, String> {
    let file = std::fs::File::open(archive).map_err(|e| format!("{}: {e}", archive.display()))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let mut entry = match zip.by_name(name) {
        Ok(entry) => entry,
        Err(_) => return Ok(false),
    };
    let mut out =
        std::fs::File::create(staged).map_err(|e| format!("{}: {e}", staged.display()))?;
    std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
    out.sync_all().map_err(|e| e.to_string())?;
    Ok(true)
}

/// ditto keeps the code signature intact. Unmount whatever happened.
#[cfg(target_os = "macos")]
fn stage(archive: &Path, staged: &Path) -> Result<(), String> {
    use std::process::Command;
    let mount = std::env::temp_dir().join("rox-update-mount");
    run_tool(
        Command::new("hdiutil")
            .args(["attach", "-nobrowse", "-readonly", "-mountpoint"])
            .arg(&mount)
            .arg(archive),
    )?;
    let copied = (|| {
        let app = mount.join("rox.app");
        if !app.exists() {
            return Err("the disk image holds no rox.app".to_string());
        }
        run_tool(Command::new("ditto").arg(&app).arg(staged))
    })();
    let _ = Command::new("hdiutil")
        .args(["detach", "-force"])
        .arg(&mount)
        .output();
    copied
}

#[cfg(target_os = "macos")]
fn run_tool(command: &mut std::process::Command) -> Result<(), String> {
    let output = command.output().map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// The running process keeps its inode.
#[cfg(target_os = "linux")]
fn swap(staged: &Path, target: &Path) -> Result<(), String> {
    std::fs::rename(staged, target).map_err(|e| format!("{}: {e}", target.display()))
}

/// A running exe can't be replaced but can be renamed. If the second rename
/// fails the first rolls back, so the install never loses its rox.exe.
#[cfg(windows)]
fn swap(staged: &Path, target: &Path) -> Result<(), String> {
    let old = sibling(target, "old");
    remove_any(&old);
    std::fs::rename(target, &old).map_err(|e| format!("{}: {e}", target.display()))?;
    if let Err(e) = std::fs::rename(staged, target) {
        let _ = std::fs::rename(&old, target);
        return Err(format!("{}: {e}", target.display()));
    }
    // Windows won't delete the running exe; clean_leftovers gets it next launch.
    Ok(())
}

/// A directory can't rename over another, so the old bundle steps aside,
/// and unlike Windows it can be deleted at once.
#[cfg(target_os = "macos")]
fn swap(staged: &Path, target: &Path) -> Result<(), String> {
    let old = sibling(target, "old");
    remove_any(&old);
    std::fs::rename(target, &old).map_err(|e| format!("{}: {e}", target.display()))?;
    if let Err(e) = std::fs::rename(staged, target) {
        let _ = std::fs::rename(&old, target);
        return Err(format!("{}: {e}", target.display()));
    }
    remove_any(&old);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_parser_reads_sha256sum_lines() {
        let manifest = concat!(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  rox-v1.2.0-linux-x86_64.tar.gz\n",
            "ABCDEF6789abcdef0123456789abcdef0123456789abcdef0123456789abcdef *rox-v1.2.0-windows-x86_64.zip\n",
            "deadbeef  something else entirely\n",
        );
        assert_eq!(
            expected_sum(manifest, "rox-v1.2.0-linux-x86_64.tar.gz").as_deref(),
            Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        );
        assert_eq!(
            expected_sum(manifest, "rox-v1.2.0-windows-x86_64.zip").as_deref(),
            Some("abcdef6789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        );
        assert_eq!(expected_sum(manifest, "something else entirely"), None);
        assert_eq!(expected_sum(manifest, "rox-v1.2.0-macos-aarch64.dmg"), None);
    }

    #[test]
    fn a_sibling_rides_the_targets_own_name() {
        assert_eq!(
            sibling(Path::new("/opt/rox/rox"), "new"),
            Path::new("/opt/rox/rox.new")
        );
        assert_eq!(
            sibling(Path::new("/opt/rox/rox.exe"), "old"),
            Path::new("/opt/rox/rox.exe.old")
        );
        assert_eq!(
            sibling(Path::new("/Applications/rox.app"), "old"),
            Path::new("/Applications/rox.app.old")
        );
    }

    #[test]
    fn a_wrong_body_never_lands() {
        let dir = std::env::temp_dir().join(format!("rox-updater-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let part = dir.join("artifact.part");
        let sum = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let progress = Progress::default();

        assert!(stream(&b"abc"[..], &part, 3, sum, &progress).is_ok());
        assert_eq!(progress.done.load(Ordering::Relaxed), 3);

        // Compare against the resolved message: the locale comes from the OS.
        let short = stream(&b"ab"[..], &part, 3, sum, &progress).unwrap_err();
        assert_eq!(
            short,
            rox_i18n::t!("updater-short", done = 2u64, bytes = 3u64).to_string(),
            "{short}"
        );

        // The digest is in the message, so check it's neither of the other two.
        let wrong = stream(&b"abd"[..], &part, 3, sum, &progress).unwrap_err();
        assert_ne!(wrong, short, "{wrong}");
        assert_ne!(
            wrong,
            rox_i18n::t!("updater-overran").to_string(),
            "{wrong}"
        );

        let flood = stream(&b"abcdefgh"[..], &part, 3, sum, &progress).unwrap_err();
        assert_eq!(
            flood,
            rox_i18n::t!("updater-overran").to_string(),
            "{flood}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// End to end against the latest published release. Ignored, since it hits
    /// the network; run it by hand after a release workflow change. It never
    /// applies anything.
    #[test]
    #[ignore = "hits the network and downloads a whole release artifact"]
    fn downloads_and_verifies_the_latest_release() {
        let release = updates::fetch_latest().expect("the latest release answers");
        let progress = Progress::default();
        let archive = fetch_verified(&release, &progress).expect("the artifact lands and verifies");
        assert!(archive.exists());
        assert_eq!(progress.fraction(), 1.0);
        let _ = std::fs::remove_file(&archive);
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn an_appimage_run_resolves_its_own_artifact() {
        assert_eq!(
            platform_for(Some(Path::new("/home/me/Apps/rox.AppImage"))),
            Some("linux-x86_64.AppImage")
        );
        assert_eq!(platform_for(None), Some("linux-x86_64.tar.gz"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_appimage_swaps_in_as_one_executable_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("rox-appimage-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("rox.AppImage");
        let archive = dir
            .join("download")
            .join("rox-v9.9.9-linux-x86_64.AppImage");
        std::fs::create_dir_all(archive.parent().unwrap()).unwrap();
        std::fs::write(&target, b"old build").unwrap();
        std::fs::write(&archive, b"new build").unwrap();
        std::fs::set_permissions(&archive, std::fs::Permissions::from_mode(0o644)).unwrap();

        apply_appimage(&archive, &target).unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"new build");
        let mode = std::fs::metadata(&target).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755, "mode {mode:o}");
        assert!(!sibling(&target, "new").exists(), "no stage left behind");
        assert!(archive.exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_fraction_is_bounded_even_when_the_counters_are_not() {
        let progress = Progress::default();
        assert_eq!(progress.fraction(), 0.0, "no total, no bar");
        progress.total.store(100, Ordering::Relaxed);
        progress.done.store(25, Ordering::Relaxed);
        assert!((progress.fraction() - 0.25).abs() < 1e-6);
        progress.done.store(400, Ordering::Relaxed);
        assert_eq!(progress.fraction(), 1.0);
    }
}
