//! The renderer sandbox (ADR 38). A renderer runs confined unless its
//! manifest asks to run unconfined: it reads its own package, writes a cache
//! folder rox makes for it, uses the system's libraries and the GPU, and
//! reaches nothing else unless it asks and the user said yes on its card.
//! Plugins aren't confined here (ADR 30).
//!
//! On Linux the sandbox is Landlock for files, TCP and signals, a seccomp
//! filter on `socket(2)` for what Landlock leaves open, and no_new_privs,
//! applied in the child between fork and exec, so nothing the renderer runs
//! ever had more. Every path is opened, every rule added and the filter
//! built in the parent. The child makes three syscalls on what the parent
//! made, which is all `pre_exec` allows. macOS and Windows have no sandbox
//! here yet, and [`support`] says so.
//!
//! What the kernel can enforce is probed once. An older kernel gets the
//! rules it knows, and [`Gaps`] names what it can't stop, so the enable card
//! never promises more than the system holds to.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// A folder of the user's a renderer can ask to read (`access.read`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Folder {
    /// Every folder in rox's library when the renderer starts.
    Library,
    Music,
    Pictures,
    Videos,
    Documents,
    Downloads,
}

impl Folder {
    pub const ALL: [Folder; 6] = [
        Folder::Library,
        Folder::Music,
        Folder::Pictures,
        Folder::Videos,
        Folder::Documents,
        Folder::Downloads,
    ];

    /// The manifest's name for it, which the approval also records.
    pub fn key(self) -> &'static str {
        match self {
            Folder::Library => "library",
            Folder::Music => "music",
            Folder::Pictures => "pictures",
            Folder::Videos => "videos",
            Folder::Documents => "documents",
            Folder::Downloads => "downloads",
        }
    }

    pub fn from_key(key: &str) -> Option<Folder> {
        Folder::ALL.into_iter().find(|folder| folder.key() == key)
    }
}

/// `capabilities.renderer.access`: what a sandboxed renderer asks for past
/// its package, its cache and the GPU. Each ask is a grant the card names.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Access {
    /// TCP and UDP to any address. Never a unix socket, so never the
    /// session bus or the display server.
    pub network: bool,
    /// Folders to read, by [`Folder::key`].
    pub read: Vec<String>,
}

impl Access {
    /// The folders asked for that this rox knows, once each, in
    /// [`Folder::ALL`]'s order. A name it doesn't know is skipped, so a
    /// renderer written for a later rox loads with nothing this one can't
    /// name, and the approval records only what was granted.
    pub fn folders(&self) -> Vec<Folder> {
        Folder::ALL
            .into_iter()
            .filter(|folder| self.read.iter().any(|name| name == folder.key()))
            .collect()
    }
}

/// What this system's kernel can't stop a sandboxed renderer doing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Gaps {
    /// Landlock before ABI 6 doesn't scope signals: a renderer can signal
    /// rox and the user's other programs.
    pub signals: bool,
    /// Landlock before ABI 3 doesn't handle truncate(2): a renderer can
    /// empty any file the user can write, by path.
    pub truncate: bool,
}

impl Gaps {
    pub fn any(self) -> bool {
        self.signals || self.truncate
    }

    /// What Landlock at `abi` leaves open that the card would mention.
    /// Network and unix sockets aren't here: the seccomp filter holds those
    /// on every ABI.
    pub fn of_abi(abi: u32) -> Gaps {
        Gaps {
            signals: abi < 6,
            truncate: abi < 3,
        }
    }
}

/// Whether rox can sandbox a renderer here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Support {
    /// Landlock at `abi` and the seccomp filter.
    Sandbox {
        abi: u32,
        gaps: Gaps,
    },
    Unavailable(Unavailable),
}

/// Why rox can't sandbox here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unavailable {
    /// Not Linux: there's no sandbox for this platform yet.
    Platform,
    /// The kernel was built without Landlock.
    NoLandlock,
    /// Landlock is built in and switched off at boot.
    LandlockOff,
    /// The kernel can't install a seccomp filter.
    NoSeccomp,
    /// Applying it failed this session, with the reason.
    Failed(String),
}

/// What a renderer runs as, from its manifest and this system. The card
/// words it, and the approval records its [`key`](Tier::key), so a change in
/// either asks the user again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tier {
    Sandboxed {
        network: bool,
        folders: Vec<Folder>,
        /// What its interpreter needs from outside the system's folders,
        /// read and run ([`interpreter_install`]). Empty for one the system
        /// or the package holds.
        interpreter: Vec<PathBuf>,
        gaps: Gaps,
    },
    /// The manifest asks for `"unconfined": true`.
    Unconfined,
    /// It would be sandboxed, and this system can't.
    Permissions(Unavailable),
}

impl Tier {
    /// `interpreter` is what [`interpreter_install`] grants the program the
    /// entry resolved to, read only when the tier is sandboxed.
    pub fn of(
        unconfined: bool,
        access: &Access,
        support: &Support,
        interpreter: impl FnOnce() -> Vec<PathBuf>,
    ) -> Tier {
        if unconfined {
            return Tier::Unconfined;
        }

        match support {
            Support::Sandbox { gaps, .. } => Tier::Sandboxed {
                network: access.network,
                folders: access.folders(),
                interpreter: interpreter(),
                gaps: *gaps,
            },
            Support::Unavailable(why) => Tier::Permissions(why.clone()),
        }
    }

    /// What an approval records. Every grant and every gap is in it, so an
    /// approval stands for exactly what the card said, and an interpreter at
    /// another path asks again. Why a system can't sandbox isn't: the
    /// renderer runs the same with the user's permissions whatever the
    /// reason.
    pub fn key(&self) -> String {
        match self {
            Tier::Sandboxed {
                network,
                folders,
                interpreter,
                gaps,
            } => {
                let mut key = String::from("sandboxed");
                if *network {
                    key.push_str(" network");
                }
                if !folders.is_empty() {
                    let names: Vec<&str> = folders.iter().map(|folder| folder.key()).collect();
                    key.push_str(&format!(" read:{}", names.join(",")));
                }
                for path in interpreter {
                    key.push_str(&format!(" run:{}", path.display()));
                }
                if gaps.signals {
                    key.push_str(" gap:signals");
                }
                if gaps.truncate {
                    key.push_str(" gap:truncate");
                }
                key
            }
            Tier::Unconfined => "unconfined".into(),
            Tier::Permissions(_) => "permissions".into(),
        }
    }

    pub fn sandboxed(&self) -> bool {
        matches!(self, Tier::Sandboxed { .. })
    }
}

/// Whether rox can sandbox a renderer here, probed once per session. A
/// failure to apply it since ([`note_failure`]) answers that failure until
/// rox restarts.
pub fn support() -> Support {
    if let Some(why) = failure() {
        return Support::Unavailable(Unavailable::Failed(why));
    }

    #[cfg(target_os = "linux")]
    {
        static PROBED: std::sync::OnceLock<Support> = std::sync::OnceLock::new();
        PROBED.get_or_init(linux::probe).clone()
    }

    #[cfg(not(target_os = "linux"))]
    {
        Support::Unavailable(Unavailable::Platform)
    }
}

/// `path` as a grant, or None when granting it would hand over more than
/// the folder the card names: the user's home itself or anything above it,
/// which would be every file they have, and anything holding or inside
/// rox's data folder, which holds their accounts and approvals. A read ask
/// and an interpreter's install are both held to it. `home` and `data` are
/// canonical.
pub fn grantable(path: &Path, home: Option<&Path>, data: Option<&Path>) -> Option<PathBuf> {
    let path = path.canonicalize().ok()?;

    let holds_home = home.is_some_and(|home| home.starts_with(&path));
    let near_data = data.is_some_and(|data| data.starts_with(&path) || path.starts_with(data));
    if holds_home || near_data {
        return None;
    }

    Some(path)
}

static FAILED: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Records that applying the sandbox failed, so every renderer approved as
/// sandboxed stays off and asks again for the rest of the session. A
/// renderer is never started unconfined in its place.
pub fn note_failure(why: &str) {
    log::warn!("sandbox: couldn't be applied, renderers stay off until approved again: {why}");
    *FAILED.lock().unwrap_or_else(|e| e.into_inner()) = Some(why.to_string());
}

fn failure() -> Option<String> {
    FAILED.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

#[cfg(target_os = "linux")]
pub use linux::{Prepared, Profile, interpreter_install, prepare};

#[cfg(target_os = "linux")]
mod linux {
    use std::ffi::CString;
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::{Gaps, Support, Unavailable, grantable};

    // ── Landlock's uapi (linux/landlock.h) ──

    const CREATE_RULESET_VERSION: u32 = 1 << 0;
    const RULE_PATH_BENEATH: libc::c_int = 1;

    const EXECUTE: u64 = 1 << 0;
    const WRITE_FILE: u64 = 1 << 1;
    const READ_FILE: u64 = 1 << 2;
    const READ_DIR: u64 = 1 << 3;
    const REMOVE_DIR: u64 = 1 << 4;
    const REMOVE_FILE: u64 = 1 << 5;
    const MAKE_CHAR: u64 = 1 << 6;
    const MAKE_DIR: u64 = 1 << 7;
    const MAKE_REG: u64 = 1 << 8;
    const MAKE_SOCK: u64 = 1 << 9;
    const MAKE_FIFO: u64 = 1 << 10;
    const MAKE_BLOCK: u64 = 1 << 11;
    const MAKE_SYM: u64 = 1 << 12;
    /// ABI 2: linking or renaming a file into another directory.
    const REFER: u64 = 1 << 13;
    /// ABI 3: truncate(2) and O_TRUNC.
    const TRUNCATE: u64 = 1 << 14;
    /// ABI 5: ioctl(2) on a device file.
    const IOCTL_DEV: u64 = 1 << 15;

    /// ABI 4.
    const NET_BIND_TCP: u64 = 1 << 0;
    const NET_CONNECT_TCP: u64 = 1 << 1;

    /// ABI 6: no connecting to an abstract unix socket made outside the
    /// sandbox, and no signal to a process outside it.
    const SCOPE_ABSTRACT_UNIX: u64 = 1 << 0;
    const SCOPE_SIGNAL: u64 = 1 << 1;

    /// The rights a rule on a file, not a directory, may carry.
    const FILE_RIGHTS: u64 = EXECUTE | WRITE_FILE | READ_FILE | TRUNCATE | IOCTL_DEV;

    #[repr(C)]
    struct RulesetAttr {
        handled_access_fs: u64,
        handled_access_net: u64,
        scoped: u64,
    }

    #[repr(C, packed)]
    struct PathBeneath {
        allowed_access: u64,
        parent_fd: i32,
    }

    /// Every filesystem right Landlock at `abi` knows. A right it doesn't
    /// know can't be handled, so it stays allowed: that's what [`Gaps`]
    /// reports.
    fn handled_fs(abi: u32) -> u64 {
        let mut fs = (1 << 13) - 1;
        if abi >= 2 {
            fs |= REFER;
        }
        if abi >= 3 {
            fs |= TRUNCATE;
        }
        if abi >= 5 {
            fs |= IOCTL_DEV;
        }
        fs
    }

    /// The ABI the kernel answers, or why it has none. ENOSYS is a kernel
    /// built without Landlock, EOPNOTSUPP one with it switched off at boot
    /// (`lsm=` without `landlock`).
    fn landlock_abi() -> Result<u32, Unavailable> {
        // SAFETY: the version query takes no pointer and makes no fd.
        let abi = unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                std::ptr::null::<RulesetAttr>(),
                0usize,
                CREATE_RULESET_VERSION,
            )
        };
        if abi >= 1 {
            return Ok(abi as u32);
        }

        match io::Error::last_os_error().raw_os_error() {
            Some(libc::EOPNOTSUPP) => Err(Unavailable::LandlockOff),
            _ => Err(Unavailable::NoLandlock),
        }
    }

    /// Whether a filter returning an errno can be installed. Any kernel with
    /// Landlock has it unless it was built without seccomp.
    fn seccomp_works() -> bool {
        let action: u32 = libc::SECCOMP_RET_ERRNO;

        // SAFETY: the query reads the u32 it's pointed at and nothing else.
        let answer = unsafe {
            libc::syscall(
                libc::SYS_seccomp,
                libc::SECCOMP_GET_ACTION_AVAIL,
                0u32,
                &action as *const u32,
            )
        };
        answer == 0
    }

    pub(super) fn probe() -> Support {
        if ARCH.is_none() {
            return Support::Unavailable(Unavailable::Platform);
        }
        if let Some(simulated) = simulated() {
            return simulated;
        }

        let abi = match landlock_abi() {
            Ok(abi) => abi,
            Err(why) => return Support::Unavailable(why),
        };
        if !seccomp_works() {
            return Support::Unavailable(Unavailable::NoSeccomp);
        }

        log::info!("sandbox: Landlock ABI {abi}, seccomp filters available");
        Support::Sandbox {
            abi,
            gaps: Gaps::of_abi(abi),
        }
    }

    /// `ROX_SANDBOX_SIMULATE`, for showing what a lesser system gets
    /// without one: `no-landlock`, `landlock-off`, `no-seccomp`, `abi=N` (a
    /// real sandbox at that ABI's rules) or `apply-fails` (the apply fails
    /// for real in the child). Debug builds and tests only: a release build
    /// never reads it, so nothing outside the code can loosen the sandbox.
    #[cfg(debug_assertions)]
    fn simulated() -> Option<Support> {
        let value = std::env::var("ROX_SANDBOX_SIMULATE").ok()?;
        log::warn!("sandbox: ROX_SANDBOX_SIMULATE={value}");

        let real = landlock_abi().unwrap_or(0);
        match value.as_str() {
            "no-landlock" => Some(Support::Unavailable(Unavailable::NoLandlock)),
            "landlock-off" => Some(Support::Unavailable(Unavailable::LandlockOff)),
            "no-seccomp" => Some(Support::Unavailable(Unavailable::NoSeccomp)),
            // Never above what the kernel has: that would claim rules it
            // can't hold.
            other => {
                let abi = other.strip_prefix("abi=")?.parse::<u32>().ok()?;
                let abi = abi.clamp(1, real.max(1));
                Some(Support::Sandbox {
                    abi,
                    gaps: Gaps::of_abi(abi),
                })
            }
        }
    }

    #[cfg(not(debug_assertions))]
    fn simulated() -> Option<Support> {
        None
    }

    #[cfg(debug_assertions)]
    fn simulate_apply_failure() -> bool {
        std::env::var("ROX_SANDBOX_SIMULATE").as_deref() == Ok("apply-fails")
    }

    #[cfg(not(debug_assertions))]
    fn simulate_apply_failure() -> bool {
        false
    }

    /// What a confined process may reach, by path. A path that isn't there
    /// on this system is skipped. A rule follows symlinks, so it lands on
    /// what the path names: `/run/opengl-driver` on NixOS is a rule on its
    /// store path.
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct Profile {
        /// Read and run.
        pub run: Vec<PathBuf>,
        /// Read only.
        pub read: Vec<PathBuf>,
        /// Folders it may list, with nothing under them readable.
        pub list: Vec<PathBuf>,
        /// Read and write, never run: its cache folder.
        pub write: Vec<PathBuf>,
        /// Device nodes it may open for reading and writing and drive with
        /// ioctl(2): the GPU's, and the null devices.
        pub devices: Vec<PathBuf>,
        /// TCP and UDP sockets. Never unix ones.
        pub network: bool,
        /// The cache folder, which [`Profile::environment`] points the
        /// renderer's caches, temp files and home at.
        pub cache: PathBuf,
    }

    /// The system's libraries and programs, read and run. NixOS keeps them
    /// all in the store, and `/run/opengl-driver` names the GPU driver
    /// there. Other distros keep them under `/usr` and the `/lib` family,
    /// GPU drivers outside the distro's packages under `/opt`, and a
    /// Flatpak's extensions under `/app` and `/usr`.
    const SYSTEM_RUN: &[&str] = &[
        "/nix/store",
        "/run/opengl-driver",
        "/run/opengl-driver-32",
        "/usr",
        "/lib",
        "/lib64",
        "/lib32",
        "/libx32",
        "/bin",
        "/sbin",
        "/opt",
        "/app",
    ];

    /// What the loader, the Vulkan loader, Mesa and fontconfig read under
    /// `/etc`. Nothing that names the user or their machine: no passwd, no
    /// machine-id, no hostname.
    const SYSTEM_READ: &[&str] = &[
        "/etc/ld.so.cache",
        "/etc/ld.so.preload",
        "/etc/ld.so.conf",
        "/etc/ld.so.conf.d",
        "/etc/localtime",
        "/etc/vulkan",
        "/etc/glvnd",
        "/etc/egl",
        "/etc/drirc",
        "/etc/OpenCL",
        "/etc/fonts",
        // Core counts and topology, which thread pools and V8 read.
        "/sys/devices/system/cpu",
        "/dev/urandom",
        "/dev/random",
    ];

    /// Name lookups and TLS, for a renderer granted the network.
    const NETWORK_READ: &[&str] = &[
        "/etc/resolv.conf",
        "/etc/hosts",
        "/etc/nsswitch.conf",
        "/etc/host.conf",
        "/etc/gai.conf",
        "/etc/services",
        "/etc/protocols",
        "/etc/ssl",
        "/etc/pki",
        "/etc/ca-certificates",
        "/etc/crypto-policies",
    ];

    /// A shell redirect and many runtimes open these.
    const NULL_DEVICES: &[&str] = &["/dev/null", "/dev/zero", "/dev/full"];

    /// How many symlinks [`interpreter_install`] follows before it stops,
    /// so a loop ends.
    const LINKS_MAX: usize = 16;

    /// What the interpreter at `program` needs to start, when the system's
    /// folders and the package don't already hold it: the file it resolves
    /// to, and its install's `bin` and `lib` when it has one. A Program
    /// Folder as a whole is never granted, since one can be any folder the
    /// user has, home included.
    ///
    /// The path as found and every path its symlinks lead through are each
    /// looked at, since a venv's `bin/python` is a link to the Python it
    /// was made from and its packages are beside the link, not the target.
    /// An install is the folder above a `bin` holding the program, only when
    /// that folder's `lib` has something named for the program, as
    /// `lib/python3.12` is for `bin/python3`. A single binary like deno's
    /// gets its file alone. Every folder granted is held to [`grantable`],
    /// and so is the install as a whole, so this can never reach home, a
    /// folder above it or rox's data folder.
    pub fn interpreter_install(
        program: &Path,
        package: &Path,
        home: Option<&Path>,
        data: Option<&Path>,
    ) -> Vec<PathBuf> {
        let package = package
            .canonicalize()
            .unwrap_or_else(|_| package.to_path_buf());
        let system: Vec<PathBuf> = SYSTEM_RUN
            .iter()
            .filter_map(|root| Path::new(root).canonicalize().ok())
            .collect();
        // Already reachable: the profile grants these whole.
        let reached = |path: &Path| {
            path.starts_with(&package) || system.iter().any(|root| path.starts_with(root))
        };

        let mut grants: Vec<PathBuf> = Vec::new();
        for link in chain(program) {
            let Some(install) = install_of(&link).and_then(|dir| dir.canonicalize().ok()) else {
                continue;
            };
            if reached(&install) {
                continue;
            }
            if grantable(&install, home, data).is_none() {
                log::warn!(
                    "sandbox: not granting the install {} for {}: it holds the home, or holds or is inside rox's data folder",
                    install.display(),
                    program.display()
                );
                continue;
            }

            // A venv says where its packages are in pyvenv.cfg, beside its
            // bin, and Python reads it before anything else.
            for part in ["bin", "lib", "pyvenv.cfg"] {
                let Some(path) = grantable(&install.join(part), home, data) else {
                    continue;
                };
                // A part that's a link out of the install grants nothing.
                if path.starts_with(&install) && !reached(&path) && !grants.contains(&path) {
                    grants.push(path);
                }
            }
        }

        let Ok(file) = program.canonicalize() else {
            return grants;
        };
        if reached(&file) || grants.iter().any(|granted| file.starts_with(granted)) {
            return grants;
        }
        match grantable(&file, home, data) {
            Some(file) => grants.push(file),
            None => log::warn!(
                "sandbox: not granting {}: it's inside rox's data folder",
                file.display()
            ),
        }

        grants
    }

    /// `program` and each path its symlinks lead through, in order.
    fn chain(program: &Path) -> Vec<PathBuf> {
        let mut links = Vec::new();
        let Ok(mut at) = std::path::absolute(program) else {
            return links;
        };

        for _ in 0..LINKS_MAX {
            links.push(at.clone());
            let Ok(target) = std::fs::read_link(&at) else {
                break;
            };
            // A relative target is from the link's folder; an absolute one
            // replaces the path.
            at = match at.parent() {
                Some(dir) => dir.join(target),
                None => target,
            };
        }

        links
    }

    /// The install `link` belongs to: the folder above the `bin` it's in,
    /// when that folder's `lib` holds an entry named for it, its name with
    /// any version digits dropped (`python3.12` is `python`).
    fn install_of(link: &Path) -> Option<PathBuf> {
        let bin = link.parent()?;
        if bin.file_name()? != "bin" {
            return None;
        }

        let install = bin.parent()?;
        let name = link.file_name()?.to_str()?;
        let stem = name.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
        if stem.is_empty() {
            return None;
        }

        let named = std::fs::read_dir(install.join("lib"))
            .ok()?
            .flatten()
            .any(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|entry| entry.starts_with(stem))
            });
        named.then(|| install.to_path_buf())
    }

    impl Profile {
        /// A renderer's: its package folder, `interpreter` as its approved
        /// tier names it, the system's libraries, the GPU, `cache`
        /// writable, and `reads` read-only. `reads` are the folders its
        /// approved asks resolved to.
        pub fn renderer(
            package: &Path,
            interpreter: &[PathBuf],
            cache: &Path,
            reads: &[PathBuf],
            network: bool,
        ) -> Profile {
            let paths = |list: &[&str]| list.iter().map(PathBuf::from).collect::<Vec<_>>();

            let mut run = vec![package.to_path_buf()];
            run.extend(interpreter.iter().cloned());
            run.extend(paths(SYSTEM_RUN));

            let mut read = paths(SYSTEM_READ);
            if network {
                read.extend(paths(NETWORK_READ));
            }

            let gpu = Gpu::find();
            read.extend(gpu.sysfs);
            read.extend(reads.iter().cloned());

            let mut devices = paths(NULL_DEVICES);
            devices.extend(gpu.nodes);

            Profile {
                run,
                read,
                list: gpu.list,
                write: vec![cache.to_path_buf()],
                devices,
                network,
                cache: cache.to_path_buf(),
            }
        }

        /// Points what a renderer writes at its cache folder: home, the
        /// XDG folders, temp files and the shader caches. Drops the
        /// variables that name the display, the session bus and the runtime
        /// folder holding both, which the sandbox keeps it from reaching
        /// anyway. Creates the folders, emptying `tmp`.
        pub fn environment(&self, command: &mut Command) -> Result<(), String> {
            let cache = &self.cache;
            let tmp = cache.join("tmp");
            let _ = std::fs::remove_dir_all(&tmp);
            for dir in [cache.clone(), tmp.clone()] {
                std::fs::create_dir_all(&dir)
                    .map_err(|e| format!("cache folder {}: {e}", dir.display()))?;
            }

            command
                .env("ROX_CACHE_DIR", cache)
                .env("HOME", cache)
                .env("XDG_CACHE_HOME", cache.join("cache"))
                .env("XDG_CONFIG_HOME", cache.join("config"))
                .env("XDG_DATA_HOME", cache.join("data"))
                .env("XDG_STATE_HOME", cache.join("state"))
                .env("TMPDIR", &tmp)
                .env(
                    "MESA_SHADER_CACHE_DIR",
                    cache.join("cache/mesa_shader_cache"),
                )
                .env("__GL_SHADER_DISK_CACHE_PATH", cache.join("cache/nvidia"))
                .env("DENO_DIR", cache.join("cache/deno"));

            for name in [
                "WAYLAND_DISPLAY",
                "DISPLAY",
                "XAUTHORITY",
                "DBUS_SESSION_BUS_ADDRESS",
                "XDG_RUNTIME_DIR",
                "TEMP",
                "TMP",
            ] {
                command.env_remove(name);
            }

            Ok(())
        }
    }

    /// The GPU's device nodes, the sysfs folders its driver reads, and the
    /// folder listed to find it.
    struct Gpu {
        nodes: Vec<PathBuf>,
        sysfs: Vec<PathBuf>,
        list: Vec<PathBuf>,
    }

    impl Gpu {
        /// Every DRM render node, each with its PCI device's folder in
        /// sysfs, where Mesa and libdrm read the vendor and bus. Without the
        /// ioctl right on the node or the sysfs folder, a Vulkan driver falls
        /// back to software. NVIDIA's proprietary nodes and its proc folder
        /// when they exist, untested.
        fn find() -> Gpu {
            let mut gpu = Gpu {
                nodes: Vec::new(),
                sysfs: Vec::new(),
                list: vec![PathBuf::from("/dev/dri")],
            };

            let mut names: Vec<String> = std::fs::read_dir("/dev/dri")
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter(|name| name.starts_with("renderD"))
                .collect();
            names.sort();

            for name in names {
                gpu.nodes.push(Path::new("/dev/dri").join(&name));

                // /sys/class/drm/renderD128 is a link to
                // /sys/devices/<pci path>/drm/renderD128.
                let class = Path::new("/sys/class/drm").join(&name);
                let device = class
                    .canonicalize()
                    .ok()
                    .and_then(|node| Some(node.parent()?.parent()?.to_path_buf()));
                if let Some(device) = device {
                    gpu.sysfs.push(device);
                }
            }

            let nvidia: Vec<PathBuf> = std::fs::read_dir("/dev")
                .into_iter()
                .flatten()
                .flatten()
                .filter(|entry| entry.file_name().to_string_lossy().starts_with("nvidia"))
                .map(|entry| entry.path())
                .collect();
            if !nvidia.is_empty() {
                gpu.nodes.extend(nvidia);
                gpu.sysfs.push(PathBuf::from("/proc/driver/nvidia"));
            }

            gpu
        }
    }

    #[derive(Clone, Copy, Debug)]
    enum Rights {
        Run,
        Read,
        List,
        Write,
        Device,
    }

    impl Rights {
        const fn bits(self) -> u64 {
            match self {
                Rights::Run => READ_FILE | READ_DIR | EXECUTE,
                Rights::Read => READ_FILE | READ_DIR,
                Rights::List => READ_DIR,
                // Everything but running what it writes, making device
                // nodes, and ioctl on a device it makes.
                Rights::Write => {
                    READ_FILE
                        | READ_DIR
                        | WRITE_FILE
                        | REMOVE_DIR
                        | REMOVE_FILE
                        | MAKE_DIR
                        | MAKE_REG
                        | MAKE_SOCK
                        | MAKE_FIFO
                        | MAKE_SYM
                        | REFER
                        | TRUNCATE
                }
                Rights::Device => READ_FILE | WRITE_FILE | IOCTL_DEV,
            }
        }
    }

    const _: () = assert!(Rights::Write.bits() & (EXECUTE | MAKE_CHAR | MAKE_BLOCK) == 0);

    /// The ruleset, the filter and the report pipe, made before the fork.
    /// Held until the spawn returns.
    pub struct Prepared {
        ruleset: OwnedFd,
        filter: Vec<libc::sock_filter>,
        report: OwnedFd,
        report_write: OwnedFd,
        /// What it granted, for the log.
        pub granted: Vec<String>,
        abi: u32,
    }

    /// Builds the ruleset for `profile` at Landlock `abi` and the filter.
    /// Every fd it keeps open is numbered `above` or higher, past where a
    /// lent handle's dup2 in the child lands.
    pub fn prepare(profile: &Profile, abi: u32, above: RawFd) -> Result<Prepared, String> {
        let fs = handled_fs(abi);
        // Network is denied by default and allowed by no rule. A renderer
        // granted it gets TCP unhandled; the filter still keeps it off unix
        // sockets.
        let net = match (abi >= 4, profile.network) {
            (true, false) => NET_BIND_TCP | NET_CONNECT_TCP,
            _ => 0,
        };
        let scoped = match abi >= 6 {
            true => SCOPE_ABSTRACT_UNIX | SCOPE_SIGNAL,
            false => 0,
        };
        let attr = RulesetAttr {
            handled_access_fs: fs,
            handled_access_net: net,
            scoped,
        };

        // SAFETY: `attr` is a live struct of the size passed. A kernel that
        // knows a smaller struct accepts this one while the extra fields
        // are zero, which the masks above make sure of. The answer is a
        // fresh close-on-exec fd this function owns.
        let raw = unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                &attr as *const RulesetAttr,
                std::mem::size_of::<RulesetAttr>(),
                0u32,
            )
        };
        if raw < 0 {
            return Err(format!("landlock ruleset: {}", io::Error::last_os_error()));
        }
        // SAFETY: just made, owned nowhere else.
        let made = unsafe { OwnedFd::from_raw_fd(raw as RawFd) };
        let ruleset = above_fd(made, above)?;

        let groups = [
            (Rights::Run, &profile.run),
            (Rights::Read, &profile.read),
            (Rights::List, &profile.list),
            (Rights::Write, &profile.write),
            (Rights::Device, &profile.devices),
        ];

        let mut granted = Vec::new();
        for (rights, paths) in groups {
            for path in paths {
                if add_rule(&ruleset, rights, path, fs)? {
                    granted.push(format!("{rights:?} {}", path.display()));
                }
            }
        }

        let mut ends = [0 as RawFd; 2];
        // SAFETY: pipe2 writes two fresh fds into the array it's handed.
        if unsafe { libc::pipe2(ends.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
            return Err(format!("sandbox report: {}", io::Error::last_os_error()));
        }
        // SAFETY: both just made, owned nowhere else.
        let (read, write) =
            unsafe { (OwnedFd::from_raw_fd(ends[0]), OwnedFd::from_raw_fd(ends[1])) };
        let report_write = above_fd(write, above)?;

        Ok(Prepared {
            ruleset,
            filter: socket_filter(profile.network),
            report: read,
            report_write,
            granted,
            abi,
        })
    }

    /// `fd` moved to `above` or higher, close-on-exec.
    fn above_fd(fd: OwnedFd, above: RawFd) -> Result<OwnedFd, String> {
        // SAFETY: fcntl on an fd `fd` holds open; the answer is a fresh fd.
        let moved = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, above) };
        if moved < 0 {
            return Err(format!("sandbox fd: {}", io::Error::last_os_error()));
        }

        // SAFETY: just made, owned nowhere else.
        Ok(unsafe { OwnedFd::from_raw_fd(moved) })
    }

    /// One path's rule. False when the path isn't on this system.
    fn add_rule(ruleset: &OwnedFd, rights: Rights, path: &Path, fs: u64) -> Result<bool, String> {
        let Ok(name) = CString::new(path.as_os_str().as_bytes()) else {
            return Ok(false);
        };

        // O_PATH follows a symlink, so the rule lands on what it names.
        // SAFETY: `name` is NUL-terminated; the fd is owned below.
        let fd = unsafe { libc::open(name.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
        if fd < 0 {
            return Ok(false);
        }
        // SAFETY: just opened, owned nowhere else.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };

        // SAFETY: fstat fills the struct it's handed from an open fd.
        let dir = unsafe {
            let mut stat: libc::stat = std::mem::zeroed();
            libc::fstat(fd.as_raw_fd(), &mut stat) == 0
                && stat.st_mode & libc::S_IFMT == libc::S_IFDIR
        };

        let mut allowed = rights.bits() & fs;
        if !dir {
            allowed &= FILE_RIGHTS;
        }
        // A folder only listed has no right a file can hold.
        if allowed == 0 {
            return Ok(false);
        }

        let rule = PathBeneath {
            allowed_access: allowed,
            parent_fd: fd.as_raw_fd(),
        };

        // SAFETY: `rule` is a live packed struct; both fds are open. The
        // rule holds its own reference to the path, so `fd` closes after.
        let added = unsafe {
            libc::syscall(
                libc::SYS_landlock_add_rule,
                ruleset.as_raw_fd(),
                RULE_PATH_BENEATH,
                &rule as *const PathBeneath,
                0u32,
            )
        };
        if added < 0 {
            return Err(format!(
                "landlock rule {}: {}",
                path.display(),
                io::Error::last_os_error()
            ));
        }

        Ok(true)
    }

    /// Which step failed in the child, as the report carries it.
    const STEP_NO_NEW_PRIVS: u32 = 1;
    const STEP_SECCOMP: u32 = 2;
    const STEP_LANDLOCK: u32 = 3;

    impl Prepared {
        /// Sets `command` up to confine its child before exec. Register it
        /// after the lend closure, which has to dup the lent handles into
        /// place first.
        pub fn install(&self, command: &mut Command) {
            let filter = self.filter.clone();
            let report = self.report_write.as_raw_fd();
            // The simulated failure hands restrict_self a closed fd, so the
            // real failure path runs in the child.
            let ruleset = match simulate_apply_failure() {
                true => -1,
                false => self.ruleset.as_raw_fd(),
            };

            // SAFETY: the closure runs in the forked child before exec and
            // only makes async-signal-safe raw syscalls: prctl, seccomp,
            // landlock_restrict_self and write. It reads `filter`, `report`
            // and `ruleset`, all made before the fork, and allocates
            // nothing: an io::Error from an errno is a plain value.
            unsafe {
                command.pre_exec(move || {
                    let failed = |step: u32| {
                        let errno = io::Error::last_os_error().raw_os_error().unwrap_or(0);
                        let mut note = [0u8; 8];
                        note[..4].copy_from_slice(&step.to_ne_bytes());
                        note[4..].copy_from_slice(&errno.to_ne_bytes());
                        libc::write(report, note.as_ptr().cast(), note.len());
                        Err(io::Error::from_raw_os_error(errno))
                    };

                    // Unprivileged seccomp and Landlock both need it, and it
                    // keeps a setuid program from running with more.
                    if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                        return failed(STEP_NO_NEW_PRIVS);
                    }

                    let program = libc::sock_fprog {
                        len: filter.len() as libc::c_ushort,
                        filter: filter.as_ptr() as *mut libc::sock_filter,
                    };
                    let installed = libc::syscall(
                        libc::SYS_seccomp,
                        libc::SECCOMP_SET_MODE_FILTER,
                        0u32,
                        &program as *const libc::sock_fprog,
                    );
                    if installed != 0 {
                        return failed(STEP_SECCOMP);
                    }

                    if libc::syscall(libc::SYS_landlock_restrict_self, ruleset, 0u32) != 0 {
                        return failed(STEP_LANDLOCK);
                    }

                    Ok(())
                });
            }
        }

        /// After a failed spawn: why the sandbox didn't apply, if that's
        /// what failed. None when the child got past it.
        pub fn failure(&self) -> Option<String> {
            let mut note = [0u8; 8];

            // SAFETY: a non-blocking read into a local buffer of its size.
            let read = unsafe {
                libc::read(
                    self.report.as_raw_fd(),
                    note.as_mut_ptr().cast(),
                    note.len(),
                )
            };
            if read != note.len() as isize {
                return None;
            }

            let step = u32::from_ne_bytes(note[..4].try_into().ok()?);
            let errno = i32::from_ne_bytes(note[4..].try_into().ok()?);
            let step = match step {
                STEP_NO_NEW_PRIVS => "no_new_privs",
                STEP_SECCOMP => "the seccomp filter",
                STEP_LANDLOCK => "Landlock",
                _ => "the sandbox",
            };

            Some(format!(
                "{step} wasn't applied: {}",
                io::Error::from_raw_os_error(errno)
            ))
        }

        pub fn abi(&self) -> u32 {
            self.abi
        }
    }

    // ── The seccomp filter ──

    // Classic BPF opcodes, from linux/bpf_common.h.
    const LD_W_ABS: u16 = 0x20;
    const JEQ_K: u16 = 0x15;
    const JSET_K: u16 = 0x45;
    const RET_K: u16 = 0x06;

    /// seccomp_data's fields: the syscall number, the architecture, and the
    /// first argument's low word (both architectures are little-endian).
    const DATA_NR: u32 = 0;
    const DATA_ARCH: u32 = 4;
    const DATA_ARG0: u32 = 16;

    /// AUDIT_ARCH_X86_64 and AUDIT_ARCH_AARCH64 from linux/audit.h. The
    /// filter is written for those two, so another architecture has no
    /// sandbox.
    #[cfg(target_arch = "x86_64")]
    const ARCH: Option<u32> = Some(0xc000_003e);
    #[cfg(target_arch = "aarch64")]
    const ARCH: Option<u32> = Some(0xc000_00b7);
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    const ARCH: Option<u32> = None;

    /// The x32 ABI's syscalls carry this bit on an x86_64 kernel and reach
    /// the same handlers under other numbers.
    #[cfg(target_arch = "x86_64")]
    const X32_BIT: u32 = 0x4000_0000;

    fn errno(code: libc::c_int) -> u32 {
        libc::SECCOMP_RET_ERRNO | code as u32
    }

    /// The filter a confined renderer runs under. socket(2) answers EACCES,
    /// except for AF_INET and AF_INET6 when it's granted the network: no
    /// unix socket by path or abstract name, so no session bus or display
    /// server, and no netlink. io_uring answers EPERM, since it makes
    /// sockets past this filter. The keyring calls answer EPERM, since
    /// Landlock doesn't cover the user's session keyring. A syscall from
    /// another architecture's table, which an x86_64 process can reach
    /// with `int 0x80`, kills the process.
    pub(super) fn socket_filter(network: bool) -> Vec<libc::sock_filter> {
        let op = |code, jt, jf, k| libc::sock_filter { code, jt, jf, k };
        let deny_socket = errno(libc::EACCES);

        let mut filter = vec![
            op(LD_W_ABS, 0, 0, DATA_ARCH),
            op(JEQ_K, 1, 0, ARCH.unwrap_or(0)),
            op(RET_K, 0, 0, libc::SECCOMP_RET_KILL_PROCESS),
            op(LD_W_ABS, 0, 0, DATA_NR),
        ];

        #[cfg(target_arch = "x86_64")]
        filter.extend([
            op(JSET_K, 0, 1, X32_BIT),
            op(RET_K, 0, 0, libc::SECCOMP_RET_KILL_PROCESS),
        ]);
        #[cfg(not(target_arch = "x86_64"))]
        let _ = JSET_K;

        for nr in [
            libc::SYS_io_uring_setup,
            libc::SYS_keyctl,
            libc::SYS_add_key,
            libc::SYS_request_key,
        ] {
            filter.extend([
                op(JEQ_K, 0, 1, nr as u32),
                op(RET_K, 0, 0, errno(libc::EPERM)),
            ]);
        }

        match network {
            false => filter.extend([
                op(JEQ_K, 0, 1, libc::SYS_socket as u32),
                op(RET_K, 0, 0, deny_socket),
                op(RET_K, 0, 0, libc::SECCOMP_RET_ALLOW),
            ]),

            true => filter.extend([
                // Not socket(2): allow. Otherwise its family decides.
                op(JEQ_K, 1, 0, libc::SYS_socket as u32),
                op(RET_K, 0, 0, libc::SECCOMP_RET_ALLOW),
                op(LD_W_ABS, 0, 0, DATA_ARG0),
                op(JEQ_K, 2, 0, libc::AF_INET as u32),
                op(JEQ_K, 1, 0, libc::AF_INET6 as u32),
                op(RET_K, 0, 0, deny_socket),
                op(RET_K, 0, 0, libc::SECCOMP_RET_ALLOW),
            ]),
        }

        filter
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_come_known_once_and_in_order() {
        let access = Access {
            network: false,
            read: vec![
                "videos".into(),
                "library".into(),
                "sparkles".into(),
                "videos".into(),
            ],
        };

        assert_eq!(access.folders(), [Folder::Library, Folder::Videos]);
        for folder in Folder::ALL {
            assert_eq!(Folder::from_key(folder.key()), Some(folder));
        }
    }

    #[test]
    fn the_key_names_every_grant_and_gap() {
        let full = Support::Sandbox {
            abi: 7,
            gaps: Gaps::of_abi(7),
        };
        let old = Support::Sandbox {
            abi: 2,
            gaps: Gaps::of_abi(2),
        };
        let asks = Access {
            network: true,
            read: vec!["pictures".into(), "library".into()],
        };

        assert_eq!(
            Tier::of(false, &Access::default(), &full, Vec::new).key(),
            "sandboxed"
        );
        assert_eq!(
            Tier::of(false, &asks, &full, Vec::new).key(),
            "sandboxed network read:library,pictures"
        );
        assert_eq!(
            Tier::of(false, &Access::default(), &old, Vec::new).key(),
            "sandboxed gap:signals gap:truncate"
        );
        assert_eq!(
            Tier::of(
                false,
                &Access::default(),
                &Support::Sandbox {
                    abi: 4,
                    gaps: Gaps::of_abi(4)
                },
                Vec::new
            )
            .key(),
            "sandboxed gap:signals"
        );

        // Unconfined is the manifest's call, whatever the system can do.
        assert_eq!(Tier::of(true, &asks, &full, Vec::new), Tier::Unconfined);
        assert_eq!(Tier::of(true, &asks, &full, Vec::new).key(), "unconfined");

        // An interpreter from outside the system's folders is part of the
        // yes: another path asks again.
        let deno = || vec![PathBuf::from("/home/u/.deno/bin/deno")];
        assert_eq!(
            Tier::of(false, &Access::default(), &full, deno).key(),
            "sandboxed run:/home/u/.deno/bin/deno"
        );

        // Why a system can't sandbox doesn't change what it runs as.
        for why in [Unavailable::NoLandlock, Unavailable::Failed("x".into())] {
            let tier = Tier::of(false, &asks, &Support::Unavailable(why.clone()), || {
                panic!("no install is looked for outside the sandbox")
            });
            assert_eq!(tier, Tier::Permissions(why));
            assert_eq!(tier.key(), "permissions");
            assert!(!tier.sandboxed());
        }
    }

    #[test]
    fn a_failure_is_answered_until_restart() {
        // The probe's own answer, whatever this machine has, then the
        // failure on top of it. Other tests in this binary don't read it.
        let before = support();
        assert!(!matches!(
            before,
            Support::Unavailable(Unavailable::Failed(_))
        ));

        note_failure("the test said so");
        assert_eq!(
            support(),
            Support::Unavailable(Unavailable::Failed("the test said so".into()))
        );

        *FAILED.lock().unwrap() = None;
        assert_eq!(support(), before);
    }

    #[test]
    fn a_grant_never_holds_home_or_the_data_folder() {
        let root = std::env::temp_dir().join(format!("rox-sandbox-grant-{}", std::process::id()));
        let home = root.join("home");
        let data = home.join(".local/share/rox");
        let music = home.join("Music");
        for dir in [&data, &music] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let (home, data, music) = (
            home.canonicalize().unwrap(),
            data.canonicalize().unwrap(),
            music.canonicalize().unwrap(),
        );

        let grant = |path: &Path| grantable(path, Some(&home), Some(&data));
        assert_eq!(grant(&music), Some(music.clone()));
        assert_eq!(grant(&home), None, "home itself, as xdg-user-dirs can set");
        assert_eq!(grant(&root), None, "above home");
        assert_eq!(grant(Path::new("/")), None);
        assert_eq!(grant(&data.join("..")), None, "around the data folder");
        assert_eq!(grant(&data), None);
        assert_eq!(grant(&root.join("missing")), None, "not there");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A fake home with a deno, a pyenv Python, a venv over it, a Python
    /// under `~/.local` beside rox's data folder, and one in `~/bin`.
    #[cfg(target_os = "linux")]
    #[test]
    fn an_interpreter_gets_its_install_and_nothing_around_it() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!("rox-sandbox-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = |rel: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(&path).unwrap();
            path
        };
        let file = |rel: &str| {
            let path = root.join(rel);
            std::fs::write(&path, "").unwrap();
            path
        };

        let home = dir("home");
        let data = dir("home/.local/share/rox");
        let package = dir("home/.local/share/rox/plugins/bars");
        let deno = {
            dir("home/.deno/bin");
            file("home/.deno/bin/deno")
        };
        let pyenv = dir("home/.pyenv/versions/3.12.1");
        dir("home/.pyenv/versions/3.12.1/bin");
        dir("home/.pyenv/versions/3.12.1/lib/python3.12");
        let python = file("home/.pyenv/versions/3.12.1/bin/python3.12");
        symlink("python3.12", pyenv.join("bin/python3")).unwrap();
        let venv = dir("home/project/.venv");
        dir("home/project/.venv/bin");
        dir("home/project/.venv/lib/python3.12/site-packages");
        file("home/project/.venv/pyvenv.cfg");
        symlink(pyenv.join("bin/python3"), venv.join("bin/python")).unwrap();
        dir("home/.local/bin");
        dir("home/.local/lib/python3.12");
        symlink(&python, home.join(".local/bin/python3")).unwrap();
        dir("home/bin");
        dir("home/lib/python3.12");
        let loose = file("home/bin/python3");

        let canon = |path: &Path| path.canonicalize().unwrap();
        let (home, data) = (canon(&home), canon(&data));
        let install = |program: &Path| {
            linux::interpreter_install(program, &package, Some(&home), Some(&data))
        };

        assert_eq!(install(&deno), [canon(&deno)], "a single binary, alone");

        let pyenv = canon(&pyenv);
        let pyenv_grants = [pyenv.join("bin"), pyenv.join("lib")];
        assert_eq!(install(&pyenv.join("bin/python3")), pyenv_grants);

        let venv = canon(&venv);
        let mut venv_grants = vec![venv.join("bin"), venv.join("lib"), venv.join("pyvenv.cfg")];
        venv_grants.extend(pyenv_grants.clone());
        assert_eq!(
            install(&venv.join("bin/python")),
            venv_grants,
            "the venv and what it's over"
        );

        // `~/.local` holds rox's data folder, so it's never an install.
        assert_eq!(install(&home.join(".local/bin/python3")), pyenv_grants);

        // `~/bin` with a `~/lib` would make home the install.
        assert_eq!(install(&loose), [canon(&loose)]);

        // The system's and the package's own need nothing more.
        assert!(install(Path::new("/bin/sh")).is_empty());
        let own = package.join("run");
        std::fs::write(&own, "").unwrap();
        assert!(install(&own).is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_filter_keeps_its_jumps_inside_itself() {
        for network in [false, true] {
            let filter = linux::socket_filter(network);
            for (at, op) in filter.iter().enumerate().filter(|(_, op)| op.code != 0x06) {
                let past = at + 1 + usize::from(op.jt.max(op.jf));
                assert!(past < filter.len(), "{network}: op {at} jumps out");
            }
            assert_eq!(filter.last().unwrap().code, 0x06, "ends on a return");
        }
    }
}
