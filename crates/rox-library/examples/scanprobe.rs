//! What a scan of an already-indexed library costs in memory before it reads
//! a single tag.
//!
//! `scanner::scan` opens by pulling every local row's path into a HashMap and
//! walking the tree into a `Vec<PathBuf>`, then works off that one list: the
//! unclaimed subset is borrowed out of it, and the prune searches it rather
//! than copying it into a set. It used to build both of those as full owned
//! copies of every path, which nobody notices at a hundred thousand tracks
//! and is two gigabytes at ten million. This probe rebuilds the sequence
//! against a real database, with no filesystem walk, and prints the resident
//! high water mark after each step, so a change to the scanner's opening has
//! a before and an after instead of an argument.
//!
//! Linux only, deliberately: it reads VmHWM out of `/proc/self/status`, and
//! the machine this is measured on is the machine CI runs on.
//!
//! ```sh
//! cargo run --release -p rox-library --example scanprobe -- \
//!     --db /tmp/rox-bench-1m.db
//! ```

use std::path::{Path, PathBuf};

use rox_library::store;

/// One field out of `/proc/self/status`, in bytes. None off Linux, or when
/// the kernel doesn't publish it.
fn status_bytes(field: &str) -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with(field))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

fn report(step: &str) {
    let fmt = |bytes: Option<u64>| match bytes {
        Some(b) => format!("{:.3} GB", b as f64 / 1e9),
        None => "unknown".to_string(),
    };
    println!(
        "{step:<28} rss {:>10}  peak {:>10}",
        fmt(status_bytes("VmRSS:")),
        fmt(status_bytes("VmHWM:"))
    );
}

fn main() {
    let mut db: Option<PathBuf> = None;
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--db" => db = it.next().map(PathBuf::from),
            "--help" | "-h" => {
                println!("scanprobe --db PATH");
                return;
            }
            other => {
                eprintln!("scanprobe: unknown flag {other}");
                std::process::exit(2);
            }
        }
    }
    let Some(db) = db.or_else(|| std::env::var_os("ROX_BENCH_DB").map(PathBuf::from)) else {
        eprintln!("scanprobe: pass --db PATH or set ROX_BENCH_DB");
        std::process::exit(2);
    };

    report("start");
    let conn = store::open(&db).expect("open the database");

    // Step one of scan(): every indexed local file's (mtime, size), keyed by
    // path. One String key and one heap allocation per track.
    let known = store::local_files(&conn).expect("read the local files");
    report(&format!("local_files ({})", known.len()));

    // Step two, standing in for the filesystem walk: the same paths as owned
    // PathBufs, sorted the way the scanner sorts them (by the string form,
    // which is what the prune's search compares on). Synthesized from the
    // store rather than walked, so the probe measures the scanner's
    // structures and not the disk.
    let mut audio: Vec<PathBuf> = known.keys().map(PathBuf::from).collect();
    audio.sort_unstable_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
    report("walk vec");

    // Step three: the unclaimed subset the batch loop runs over, borrowed
    // out of the walk vector. One pointer per file, not a path.
    let claimed: std::collections::HashMap<PathBuf, ()> = std::collections::HashMap::new();
    let files: Vec<&PathBuf> = audio
        .iter()
        .filter(|path| !claimed.contains_key(Path::new(*path)))
        .collect();
    report(&format!("files vec ({})", files.len()));

    // Nothing above may be dropped early, or the peak this prints isn't the
    // peak the scanner reaches.
    println!(
        "held: {} known, {} walked, {} to scan",
        known.len(),
        audio.len(),
        files.len()
    );
}
