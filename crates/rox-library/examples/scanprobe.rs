//! What a scan of an already-indexed library costs in memory before it reads
//! a single tag.
//!
//! Rebuilds `scanner::scan`'s opening sequence against a real database, with
//! no filesystem walk, and prints the resident high water mark after each
//! step. Owned copies of every path go unnoticed at a hundred thousand tracks
//! and cost two gigabytes at ten million.
//!
//! Linux only: it reads VmHWM out of `/proc/self/status`.
//!
//! ```sh
//! cargo run --release -p rox-library --example scanprobe -- \
//!     --db /tmp/rox-bench-1m.db
//! ```

use std::path::{Path, PathBuf};

use rox_library::store;

/// In bytes. None off Linux.
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

    // Step one of scan(): every indexed local file's (mtime, size), keyed by path.
    let known = store::local_files(&conn).expect("read the local files");
    report(&format!("local_files ({})", known.len()));

    // Step two, standing in for the walk: the same paths as PathBufs, sorted by
    // string form like the scanner's.
    let mut audio: Vec<PathBuf> = known.keys().map(PathBuf::from).collect();
    audio.sort_unstable_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
    report("walk vec");

    // Step three: the unclaimed subset, borrowed out of the walk vector.
    let claimed: std::collections::HashMap<PathBuf, ()> = std::collections::HashMap::new();
    let files: Vec<&PathBuf> = audio
        .iter()
        .filter(|path| !claimed.contains_key(Path::new(*path)))
        .collect();
    report(&format!("files vec ({})", files.len()));

    // Nothing above may drop early, or this isn't the scanner's peak.
    println!(
        "held: {} known, {} walked, {} to scan",
        known.len(),
        audio.len(),
        files.len()
    );
}
