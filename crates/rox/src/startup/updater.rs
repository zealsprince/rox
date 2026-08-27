//! The self-updater: the download half of the update story, where
//! [`updates`](crate::startup::updates) is the check half. Given a newer
//! release it resolves this platform's artifact, downloads it with a
//! checksum verify against the release's SHA256SUMS.txt, stages the new
//! build next to the running one, and swaps it into place - rename-over on
//! Linux, the rename-aside dance on Windows where a running exe can't be
//! replaced, and a bundle swap on macOS. The swap lands on disk at once;
//! the running process keeps its old build until a restart, which is what
//! the About page's restart prompt is for.
//!
//! ## What can update
//!
//! Only an install that owns its own folder: the write probe in
//! [`can_update`] is the gate, so a distro package in /usr/bin, a nix store
//! path, or any other read-only home stays notify-only. A portable install
//! passes the probe by construction - portable requires a writable folder -
//! and updates in place beside its data. Platforms the release workflow
//! doesn't build for resolve no artifact and stay notify-only too.
//!
//! ## Why a failed download can't hurt
//!
//! Everything up to the swap happens in the OS temp dir, and the swap only
//! runs after the checksum matches, so a failed or interrupted download
//! leaves the install exactly as it was and a retry starts clean. Renames
//! within the install folder are the only writes it ever takes, and the
//! Windows dance rolls the first rename back if the second fails.
//! [`clean_leftovers`] sweeps the temp dir and any rename-aside remains at
//! the next launch.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::startup::updates::{self, Release};

/// This build's artifact suffix, matching release.yml's matrix. None on a
/// platform the workflow doesn't build, which leaves the check notify-only.
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

/// The checksum manifest the release workflow publishes beside the
/// artifacts, one `sha256sum` line per file.
const SUMS: &str = "SHA256SUMS.txt";

/// Where the archive downloads before anything touches the install.
fn work_dir() -> PathBuf {
    std::env::temp_dir().join("rox-update")
}

/// The update as it moves along, one global slot: at most one download per
/// run, and once a build is applied the only step left is a restart.
#[derive(Clone)]
pub enum Status {
    Idle,
    Downloading(Arc<Progress>),
    /// The new build is on disk where the old one was; a restart runs it.
    Applied {
        version: String,
    },
    Failed {
        error: String,
    },
}

static STATE: Mutex<Status> = Mutex::new(Status::Idle);

/// The updater's current state, for the About page's status line.
pub fn status() -> Status {
    STATE.lock().unwrap().clone()
}

/// Live progress of the download: the worker writes it, the UI polls it.
/// Shaped after `rox_acoustic::models::Progress` minus the cancel - a
/// download this size finishing unwanted costs nothing, the swap is only
/// ever wanted, and quitting kills the process anyway.
#[derive(Default)]
pub struct Progress {
    done: AtomicU64,
    total: AtomicU64,
}

impl Progress {
    /// How far along, 0 to 1. Zero until the artifact's size is known.
    pub fn fraction(&self) -> f32 {
        let total = self.total.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        (self.done.load(Ordering::Relaxed) as f32 / total as f32).clamp(0.0, 1.0)
    }
}

/// Whether this install can replace itself: the platform has an artifact
/// and the install's folder takes writes. Probed once per run - the answer
/// is about where the executable lives, which doesn't move mid-run.
pub fn can_update() -> bool {
    static CAN: OnceLock<bool> = OnceLock::new();
    *CAN.get_or_init(|| PLATFORM.is_some() && install_writable())
}

/// Claim the one download slot and hand back the blocking job, or None
/// when a download is already running or a build is already applied. The
/// claim happens on the caller's thread so the UI sees Downloading the
/// moment it asks, however long the executor takes to start the job.
pub fn begin(release: &Release) -> Option<impl FnOnce() + Send + 'static> {
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

/// Sweep what an update can leave behind: the temp workspace, and the
/// rename-aside remains in the install folder - the `.old` build Windows
/// can't delete while it runs, and a `.new` stage a crash stranded. Launch
/// calls this; every miss just waits for the next one.
pub fn clean_leftovers() {
    let _ = std::fs::remove_dir_all(work_dir());
    if let Ok(target) = install_target() {
        remove_any(&sibling(&target, "new"));
        remove_any(&sibling(&target, "old"));
        // The rox-mcp swap beside the app leaves the same remains.
        #[cfg(not(target_os = "macos"))]
        {
            let helper = target.with_file_name(helper_name());
            remove_any(&sibling(&helper, "new"));
            remove_any(&sibling(&helper, "old"));
        }
    }
}

/// The whole blocking journey: resolve, download, verify, stage, swap.
/// Returns the version now on disk.
fn download_and_apply(release: &Release, progress: &Progress) -> Result<String, String> {
    // A release rebuilt from the settings cache carries no asset list, so
    // ask GitHub again; whatever is latest now is what the user asked for.
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
    // The archive is spent either way; a failure's retry downloads fresh.
    let _ = std::fs::remove_file(&archive);
    applied.map(|()| release.version.clone())
}

/// Resolve this platform's artifact against the release's files, download
/// it into the work dir, and hand back the archive once its checksum
/// matches the release's manifest. The download half of the journey, with
/// no writes anywhere near the install.
fn fetch_verified(release: &Release, progress: &Progress) -> Result<PathBuf, String> {
    let platform = PLATFORM.ok_or_else(|| rox_i18n::t_static("updater-no-release-build"))?;
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

/// The agent the download rides. Not `rox_net::providers::agent`: that one
/// caps every request at ten seconds total, right for a metadata lookup and
/// fatal for tens of megabytes on a slow link. Bounding the connect and
/// each read instead means a stalled transfer still gives up while a
/// slow-but-alive one finishes. Same reasoning as the model downloader's.
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

/// Fetch the checksum manifest, a few hundred bytes of text.
fn fetch_sums(url: &str) -> Result<String, String> {
    agent()
        .get(url)
        .call()
        .map_err(|e| rox_net::providers::net_reason(&e))?
        .into_string()
        .map_err(|e| e.to_string())
}

/// The manifest's hash for one artifact. `sha256sum` writes
/// `<hex>  <name>`, with a `*` on the name in binary mode, so take the
/// first and last tokens and let the middle collapse.
fn expected_sum(manifest: &str, name: &str) -> Option<String> {
    manifest.lines().find_map(|line| {
        let mut tokens = line.split_whitespace();
        let hash = tokens.next()?;
        let file = tokens.next_back().unwrap_or(hash);
        (file.trim_start_matches('*') == name && hash.len() == 64)
            .then(|| hash.to_ascii_lowercase())
    })
}

/// Stream the artifact to a `.part` file, hashing as the bytes go by, and
/// rename to `path` only once the size and checksum both match. The part
/// file dies with any failure, so nothing half-written survives.
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
    // Guard the length before a byte is written: a redirect to an error
    // page shows up here as a wildly different size, and there's no point
    // streaming megabytes to find that out.
    if let Some(claimed) = response
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok())
    {
        if claimed != bytes {
            return Err(
                rox_i18n::t!("updater-size-mismatch", claimed = claimed, bytes = bytes).to_string(),
            );
        }
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

/// Copy the body into the part file, counting and hashing, then judge what
/// landed against the release's own numbers.
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
        // Refuse to write past the stated size, so a server streaming
        // forever can't fill the disk.
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

/// What the swap replaces: the executable itself, or on macOS the whole
/// app bundle, since a build is the bundle - Info.plist's version and all.
#[cfg(not(target_os = "macos"))]
fn install_target() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| format!("can't locate the running executable: {e}"))
}

#[cfg(target_os = "macos")]
fn install_target() -> Result<PathBuf, String> {
    bundle_root().ok_or_else(|| "not running from an app bundle".into())
}

/// The .app the running executable sits in, walking up from the binary.
/// None for a bare binary, which stays notify-only: the artifact is a
/// bundle, and a dev build has no install to keep current.
#[cfg(target_os = "macos")]
fn bundle_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .find(|dir| dir.extension().is_some_and(|ext| ext == "app"))
        .map(Path::to_path_buf)
}

/// The target's neighbor with a suffix tacked on: `rox.exe` to
/// `rox.exe.new`, `rox.app` to `rox.app.old`. Built off the target's own
/// name, so a renamed executable stages beside itself.
fn sibling(target: &Path, suffix: &str) -> PathBuf {
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    target.with_file_name(format!("{name}.{suffix}"))
}

/// Remove a leftover whatever it is, file or bundle folder.
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

/// Whether the install's folder takes writes, probed with a real file the
/// way the portable gate does: permission bits don't answer reliably
/// across platforms.
#[cfg(not(target_os = "macos"))]
fn install_writable() -> bool {
    rox_core::settings::portable_available()
}

/// On macOS the swap renames the bundle, so the probe belongs in the
/// folder holding it - /Applications, usually - not in MacOS/ inside.
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

/// Stage the verified archive's build beside the running one and swap it
/// into place. The stage is the last thing that can fail big; the swap is
/// renames within one folder. On macOS the bundle carries rox-mcp inside,
/// so the one swap covers both.
#[cfg(target_os = "macos")]
fn apply(archive: &Path) -> Result<(), String> {
    let target = install_target()?;
    let staged = sibling(&target, "new");
    remove_any(&staged);
    stage(archive, &staged)?;
    swap(&staged, &target)
}

/// The bare-binary flavor: the app and the rox-mcp proxy beside it, each
/// staged then swapped. Both stages land before anything moves, and the
/// helper swaps first: if it can't, the app hasn't moved and a retry
/// starts clean. The reverse partial - new helper under an old app - the
/// socket's generation check turns into a plain error rather than silence.
#[cfg(not(target_os = "macos"))]
fn apply(archive: &Path) -> Result<(), String> {
    let target = install_target()?;
    let binary = format!("rox{}", std::env::consts::EXE_SUFFIX);
    let helper_target = target.with_file_name(helper_name());
    let staged = sibling(&target, "new");
    let helper_staged = sibling(&helper_target, "new");
    remove_any(&staged);
    remove_any(&helper_staged);
    if !stage(archive, &binary, &staged)? {
        return Err(format!("the archive holds no {binary}"));
    }
    // Absent only in archives from before the proxy shipped; nothing to
    // deliver then, and the app still updates.
    if stage(archive, &helper_name(), &helper_staged)? {
        swap(&helper_staged, &helper_target)?;
    }
    swap(&staged, &target)
}

/// The proxy's file name beside the executable, the same shape the MCP
/// settings page hands out.
#[cfg(not(target_os = "macos"))]
fn helper_name() -> String {
    format!("rox-mcp{}", std::env::consts::EXE_SUFFIX)
}

/// Pull one file out of the Linux tarball by name, written out executable
/// and synced before the caller renames it live. False when the archive
/// doesn't carry it.
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

/// The same out of the Windows zip.
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

/// And the whole bundle out of the macOS disk image: mount read-only, copy
/// rox.app beside the installed one with ditto - which keeps the code
/// signature intact - then unmount whatever happened.
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

/// Run a staging tool, folding a failure's stderr into the error.
#[cfg(target_os = "macos")]
fn run_tool(command: &mut std::process::Command) -> Result<(), String> {
    let output = command.output().map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// Rename-over: one atomic rename, and the running process keeps its inode.
#[cfg(target_os = "linux")]
fn swap(staged: &Path, target: &Path) -> Result<(), String> {
    std::fs::rename(staged, target).map_err(|e| format!("{}: {e}", target.display()))
}

/// The rename-aside dance: a running exe can't be replaced but it can be
/// renamed, so the old build steps aside and the new one takes its name.
/// If the second rename fails the first rolls back, so a half-danced swap
/// never strands the install without a rox.exe.
#[cfg(windows)]
fn swap(staged: &Path, target: &Path) -> Result<(), String> {
    let old = sibling(target, "old");
    remove_any(&old);
    std::fs::rename(target, &old).map_err(|e| format!("{}: {e}", target.display()))?;
    if let Err(e) = std::fs::rename(staged, target) {
        let _ = std::fs::rename(&old, target);
        return Err(format!("{}: {e}", target.display()));
    }
    // The old exe is still running, so Windows won't delete it now;
    // clean_leftovers sweeps it on the next launch.
    Ok(())
}

/// The bundle flavor of the dance. A directory can't rename over another,
/// so the old bundle steps aside like Windows' exe; unlike Windows it can
/// be deleted at once, the running binary living on through its inode.
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
        // A plain text-mode line.
        assert_eq!(
            expected_sum(manifest, "rox-v1.2.0-linux-x86_64.tar.gz").as_deref(),
            Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        );
        // Binary-mode's `*` prefix comes off, and the hash lowercases.
        assert_eq!(
            expected_sum(manifest, "rox-v1.2.0-windows-x86_64.zip").as_deref(),
            Some("abcdef6789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        );
        // A short hash is not a checksum, and a missing file is a miss.
        assert_eq!(expected_sum(manifest, "something else entirely"), None);
        assert_eq!(expected_sum(manifest, "rox-v1.2.0-macos-aarch64.dmg"), None);
    }

    #[test]
    fn a_sibling_rides_the_targets_own_name() {
        assert_eq!(
            sibling(Path::new("/opt/rox/rox"), "new"),
            Path::new("/opt/rox/rox.new")
        );
        // The suffix lands after the extension, never instead of it:
        // rox.exe steps aside as rox.exe.old, not rox.old.
        assert_eq!(
            sibling(Path::new("/opt/rox/rox.exe"), "old"),
            Path::new("/opt/rox/rox.exe.old")
        );
        assert_eq!(
            sibling(Path::new("/Applications/rox.app"), "old"),
            Path::new("/Applications/rox.app.old")
        );
    }

    /// A short, flooding, or tampered body never becomes a staged archive;
    /// same failure family the model downloader guards, same reasons.
    #[test]
    fn a_wrong_body_never_lands() {
        let dir = std::env::temp_dir().join(format!("rox-updater-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let part = dir.join("artifact.part");
        // sha256("abc")
        let sum = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let progress = Progress::default();

        assert!(stream(&b"abc"[..], &part, 3, sum, &progress).is_ok());
        assert_eq!(progress.done.load(Ordering::Relaxed), 3);

        // Against the resolved message, not an English fragment of it: the
        // active locale comes from the OS, so a German machine would fail a
        // substring check for "stopped at" while the code was working fine.
        let short = stream(&b"ab"[..], &part, 3, sum, &progress).unwrap_err();
        assert_eq!(
            short,
            rox_i18n::t!("updater-short", done = 2u64, bytes = 3u64).to_string(),
            "{short}"
        );

        // A body of the right length but the wrong bytes is refused for the
        // checksum, not for a length: the digest is in the message so the
        // whole string can't be predicted, but it is neither of these two.
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

    /// The real resolve-download-verify, end to end against the latest
    /// published release. Ignored, so `cargo test` never reaches for the
    /// network or pulls a whole artifact; run it by hand
    /// (`cargo test -- --ignored downloads_and`) after the workflow
    /// change ships, since a release without SHA256SUMS.txt, a renamed
    /// artifact, or a manifest the parser misreads only shows up against
    /// the real thing. It never applies anything.
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
