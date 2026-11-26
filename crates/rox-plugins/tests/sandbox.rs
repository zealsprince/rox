//! The renderer sandbox against a real process (ADR 38): the probe fixture,
//! a Python renderer that tries every way out it can and says what stopped
//! it. Skipped with a note when the machine can't sandbox or has no Python,
//! so the gate still runs there.

#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::io::Read;
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use rox_plugins::manifest;
use rox_plugins::process::{self, Extra};
use rox_plugins::sandbox::{self, Profile, Support};
use serde_json::json;

/// A scratch tree standing in for the user's: a home with a secret, rox's
/// data dir, another package, a library folder and one beside it, and
/// listeners standing in for the session bus and the network.
struct Scene {
    root: PathBuf,
    package: PathBuf,
    cache: PathBuf,
    library: PathBuf,
    _tcp: std::net::TcpListener,
    _udp: std::net::UdpSocket,
    _bus: std::os::unix::net::UnixListener,
    _abstract: std::os::unix::net::UnixListener,
}

impl Scene {
    fn new(tag: &str) -> Scene {
        use std::os::linux::net::SocketAddrExt;

        let root = std::env::temp_dir().join(format!("rox-sandbox-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);

        let dir = |rel: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(&path).unwrap();
            path
        };
        let file = |rel: &str, text: &str| {
            let path = root.join(rel);
            std::fs::write(&path, text).unwrap();
            path
        };

        let package = dir("data/plugins/sandbox-probe");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sandbox-probe");
        for name in ["plugin.json", "probe.py"] {
            std::fs::copy(fixture.join(name), package.join(name)).unwrap();
        }

        dir("home/.ssh");
        let home_file = file("home/.ssh/id_ed25519", "a key\n");
        let data_file = file("data/accounts.json", "{\"token\": \"secret\"}\n");
        let other = dir("data/plugins/someone-else");
        file("data/plugins/someone-else/plugin.json", "{}\n");
        let cache = dir("data/renderer-cache/sandbox-probe");
        let library = dir("music");
        let library_file = file("music/track.flac", "fLaC\n");
        dir("beside");
        let beside_file = file("beside/notes.txt", "not music\n");

        let tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let bus_path = root.join("bus");
        let bus = std::os::unix::net::UnixListener::bind(&bus_path).unwrap();
        let name = format!("rox-sandbox-{tag}-{}", std::process::id());
        let addr = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes()).unwrap();
        let abstract_ = std::os::unix::net::UnixListener::bind_addr(&addr).unwrap();

        // The display: the real one when there is one, or the bus again.
        let wayland = std::env::var_os("XDG_RUNTIME_DIR")
            .zip(std::env::var_os("WAYLAND_DISPLAY"))
            .map(|(run, display)| Path::new(&run).join(display))
            .filter(|path| path.exists())
            .unwrap_or_else(|| bus_path.clone());

        let targets = json!({
            "home": root.join("home"),
            "home_file": home_file,
            "data_dir": root.join("data"),
            "data_file": data_file,
            "other_package": other,
            "tcp_port": tcp.local_addr().unwrap().port(),
            "udp_port": udp.local_addr().unwrap().port(),
            "bus": bus_path,
            "wayland": wayland,
            "abstract": name,
            "write_outside": root.join("home/planted"),
            "rox_pid": std::process::id(),
            "library_file": library_file,
            "beside_file": beside_file,
        });
        std::fs::write(package.join("targets.json"), targets.to_string()).unwrap();

        Scene {
            root,
            package,
            cache,
            library,
            _tcp: tcp,
            _udp: udp,
            _bus: bus,
            _abstract: abstract_,
        }
    }

    /// Runs the probe confined, answering each try's word by name.
    fn probe(&self, reads: &[PathBuf], network: bool) -> HashMap<String, String> {
        let manifest = manifest::load(&self.package).unwrap();
        let command = manifest::entry_for(&manifest, &self.package).unwrap();
        let interpreter = self.install(Path::new(command.get_program()));

        self.probe_with(command, &interpreter, reads, network)
    }

    /// What `program`'s install is granted, with this scene's home and data
    /// folder standing in for the user's.
    fn install(&self, program: &Path) -> Vec<PathBuf> {
        let home = self.root.join("home").canonicalize().unwrap();
        let data = self.root.join("data").canonicalize().unwrap();
        sandbox::interpreter_install(program, &self.package, Some(&home), Some(&data))
    }

    /// Runs `command` confined, with `interpreter` as its tier's grant.
    fn probe_with(
        &self,
        mut command: Command,
        interpreter: &[PathBuf],
        reads: &[PathBuf],
        network: bool,
    ) -> HashMap<String, String> {
        command.env("PROBE_OUT", "stdout").env("ROX_FRAMES_FD", "3");

        let frames_path = self.root.join("frames");
        std::fs::write(&frames_path, b"RXF1 frames").unwrap();
        let frames = std::fs::File::open(&frames_path).unwrap();

        let profile = Profile::renderer(&self.package, interpreter, &self.cache, reads, network);
        let abi = match sandbox::support() {
            Support::Sandbox { abi, .. } => abi,
            other => panic!("checked before: {other:?}"),
        };

        let (mut process, pipes) = process::spawn_confined(
            "renderer",
            "sandbox-probe",
            command,
            &self.package,
            Extra::Handles(vec![frames.as_fd()]),
            &profile,
            abi,
        )
        .unwrap();

        let mut out = String::new();
        let mut stdout = pipes.stdout;
        stdout.read_to_string(&mut out).unwrap();
        drop(pipes.stdin);
        process.wait_for(Duration::from_secs(10));

        // Shown with --nocapture, and on a failure.
        eprintln!("{out}");
        assert!(
            out.contains("probe done"),
            "the probe ran to its end:\n{out}"
        );

        out.lines()
            .filter_map(|line| {
                let mut words = line.splitn(4, ' ');
                (words.next()? == "probe").then_some(())?;
                Some((words.next()?.to_string(), words.next()?.to_string()))
            })
            .collect()
    }
}

impl Drop for Scene {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Whether this machine can run the probe, with a line saying why not.
fn runnable() -> bool {
    if !matches!(sandbox::support(), Support::Sandbox { .. }) {
        eprintln!("skipping the sandbox probe: {:?}", sandbox::support());
        return false;
    }
    if manifest::find_program("python3", None).is_none() {
        eprintln!("skipping the sandbox probe: no python3");
        return false;
    }

    true
}

fn assert_words(seen: &HashMap<String, String>, word: &str, names: &[&str]) {
    for name in names {
        assert_eq!(
            seen.get(*name).map(String::as_str),
            Some(word),
            "{name} should be {word}: {seen:?}"
        );
    }
}

const ALWAYS_BLOCKED: &[&str] = &[
    "home-file",
    "home-list",
    "data-dir-file",
    "data-dir-list",
    "other-package",
    "session-bus",
    "wayland",
    "abstract-unix",
    "write-package",
    "write-outside",
    "proc-environ",
    "proc-mem",
    "proc-cmdline",
    "ptrace-rox",
    "vm-read-rox",
    "proc-self",
    "dev-shm",
    "run-user",
    "etc-passwd",
    "tmp-list",
];

const ALWAYS_REACHED: &[&str] = &["own-folder", "own-list", "cache-write", "lent-fds"];

#[test]
fn a_renderer_reaches_its_own_things_and_nothing_else() {
    if !runnable() {
        return;
    }

    let scene = Scene::new("closed");
    let seen = scene.probe(&[], false);

    assert_words(&seen, "blocked", ALWAYS_BLOCKED);
    assert_words(&seen, "blocked", &["tcp-localhost", "udp-socket"]);
    assert_words(&seen, "reached", ALWAYS_REACHED);
    assert_words(&seen, "blocked", &["library-file", "beside-library"]);

    // Landlock from ABI 6 scopes signals; before it a renderer can signal.
    if !sandbox::Gaps::of_abi(abi()).signals {
        assert_words(&seen, "blocked", &["signal-rox-0", "signal-rox-cont"]);
    }

    // What the sandbox doesn't stop: learning that a path exists.
    assert_words(&seen, "reached", &["home-stat"]);
}

#[test]
fn the_network_ask_opens_tcp_and_udp_and_never_a_unix_socket() {
    if !runnable() {
        return;
    }

    let scene = Scene::new("network");
    let seen = scene.probe(&[], true);

    assert_words(&seen, "reached", &["tcp-localhost", "udp-socket"]);
    assert_words(&seen, "blocked", ALWAYS_BLOCKED);
    assert_words(&seen, "reached", ALWAYS_REACHED);
}

#[test]
fn a_read_ask_opens_that_folder_and_nothing_beside_it() {
    if !runnable() {
        return;
    }

    let scene = Scene::new("library");
    let seen = scene.probe(std::slice::from_ref(&scene.library), false);

    assert_words(&seen, "reached", &["library-file"]);
    assert_words(&seen, "blocked", &["beside-library", "tcp-localhost"]);
    assert_words(&seen, "blocked", ALWAYS_BLOCKED);
}

fn abi() -> u32 {
    match sandbox::support() {
        Support::Sandbox { abi, .. } => abi,
        _ => 0,
    }
}

/// A Python installed under home, pyenv's way, is granted its install and
/// runs confined. It still can't read a file beside the install, or the
/// home around it.
#[test]
fn an_interpreter_under_home_reaches_its_install_and_nothing_beside_it() {
    if !runnable() {
        return;
    }

    let scene = Scene::new("pyenv");
    let versions = scene.root.join("home/.pyenv/versions");
    let install = versions.join("3.12.1");
    for dir in ["bin", "lib/python3.12"] {
        std::fs::create_dir_all(install.join(dir)).unwrap();
    }
    let python = manifest::find_program("python3", None).unwrap();
    let copied = install.join("bin/python3");
    std::fs::copy(python.canonicalize().unwrap(), &copied).unwrap();
    let inside = install.join("lib/python3.12/marker.txt");
    std::fs::write(&inside, "in the install\n").unwrap();
    let beside = versions.join("notes.txt");
    std::fs::write(&beside, "beside the install\n").unwrap();

    let targets_at = scene.package.join("targets.json");
    let mut targets: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&targets_at).unwrap()).unwrap();
    targets["install_file"] = json!(inside);
    targets["beside_install"] = json!(beside);
    std::fs::write(&targets_at, targets.to_string()).unwrap();

    let install = install.canonicalize().unwrap();
    let interpreter = scene.install(&copied);
    assert_eq!(interpreter, [install.join("bin"), install.join("lib")]);

    let mut command = Command::new(&copied);
    command.arg(scene.package.join("probe.py"));
    let seen = scene.probe_with(command, &interpreter, &[], false);

    assert_words(&seen, "reached", &["install-file"]);
    assert_words(&seen, "blocked", &["beside-install"]);
    assert_words(&seen, "blocked", ALWAYS_BLOCKED);
    assert_words(&seen, "reached", ALWAYS_REACHED);

    // Without the grant the same program doesn't start at all.
    let mut command = Command::new(&copied);
    command.arg(scene.package.join("probe.py"));
    let profile = Profile::renderer(&scene.package, &[], &scene.cache, &[], false);
    let refused = process::spawn_confined(
        "renderer",
        "pyenv",
        command,
        &scene.package,
        Extra::None,
        &profile,
        abi(),
    );
    assert!(refused.is_err(), "it ran without its install granted");
}

/// The shell a renderer's script starts with, confined, still runs.
#[test]
fn a_shell_runs_confined() {
    if !matches!(sandbox::support(), Support::Sandbox { .. }) {
        return;
    }

    let scene = Scene::new("shell");
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg("echo ran > \"$ROX_CACHE_DIR/ran\"; cat \"$ROX_CACHE_DIR/ran\"");
    let profile = Profile::renderer(&scene.package, &[], &scene.cache, &[], false);

    let (mut process, pipes) = process::spawn_confined(
        "renderer",
        "shell",
        command,
        &scene.package,
        Extra::None,
        &profile,
        abi(),
    )
    .unwrap();

    let mut out = String::new();
    let mut stdout = pipes.stdout;
    stdout.read_to_string(&mut out).unwrap();
    process.wait_for(Duration::from_secs(5));

    assert_eq!(out, "ran\n");
}

/// What confining costs at start: spawn to exit of a program that does
/// nothing, confined and not, as a median of 30 runs each. Ignored in the
/// gate; `cargo test -p rox-plugins --test sandbox -- --ignored --nocapture`
/// prints the numbers.
#[test]
#[ignore]
fn what_starting_confined_costs() {
    if !runnable() {
        return;
    }

    let scene = Scene::new("cost");
    let python = manifest::find_program("python3", None).unwrap();
    let programs: [(&str, PathBuf, &[&str]); 2] = [
        ("sh -c true", PathBuf::from("/bin/sh"), &["-c", "true"]),
        ("python3 -c pass", python, &["-c", "pass"]),
    ];

    for (name, program, args) in programs {
        let time = |confined: bool| {
            let mut runs: Vec<Duration> = (0..30)
                .map(|_| {
                    let mut command = Command::new(&program);
                    command.args(args);
                    let began = std::time::Instant::now();

                    let (mut process, pipes) = match confined {
                        true => {
                            let profile = Profile::renderer(
                                &scene.package,
                                &scene.install(&program),
                                &scene.cache,
                                &[],
                                false,
                            );
                            process::spawn_confined(
                                "renderer",
                                "cost",
                                command,
                                &scene.package,
                                Extra::None,
                                &profile,
                                abi(),
                            )
                        }
                        false => process::spawn_with(
                            "renderer",
                            "cost",
                            command,
                            &scene.package,
                            Extra::None,
                        ),
                    }
                    .unwrap();

                    // stdout closes when it exits, with no polling in the way.
                    let mut out = Vec::new();
                    let mut stdout = pipes.stdout;
                    stdout.read_to_end(&mut out).unwrap();
                    let took = began.elapsed();
                    process.wait_for(Duration::from_secs(10));

                    took
                })
                .collect();
            runs.sort();
            runs[runs.len() / 2]
        };

        let (open, closed) = (time(false), time(true));
        eprintln!("{name}: unconfined {open:?}, confined {closed:?}");
    }
}
