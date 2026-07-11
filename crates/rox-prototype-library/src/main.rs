//! Harness: populate a synthetic catalog, load the projection, and time every
//! operation the browse UI depends on. Reuses an existing database when the
//! row count already matches, so repeat runs only measure the read path.
//!
//! Usage: rox-prototype-library [--tracks N] [--db PATH] [--skip-serial] [--skip-like]
//! Defaults to 10 million tracks in target/rox-prototype-library/.

use std::path::PathBuf;
use std::time::Instant;

use rox_library::projection::Projection;
use rox_library::store;
use rox_prototype_library::gen::{self, Rng};

const SEED: u64 = 0x0520;

fn main() {
    let mut tracks: u64 = 10_000_000;
    let mut db: Option<PathBuf> = None;
    let mut skip_serial = false;
    let mut skip_like = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--tracks" => {
                let v = args.next().expect("--tracks needs a value");
                tracks = v.replace('_', "").parse().expect("--tracks needs a number");
            }
            "--db" => db = Some(PathBuf::from(args.next().expect("--db needs a path"))),
            "--skip-serial" => skip_serial = true,
            "--skip-like" => skip_like = true,
            other => panic!("unknown arg: {other}"),
        }
    }

    let db = db.unwrap_or_else(|| {
        let dir = PathBuf::from("target/rox-prototype-library");
        std::fs::create_dir_all(&dir).expect("create db dir");
        dir.join(format!("library-{tracks}.db"))
    });

    println!("tracks: {tracks}, db: {}", db.display());

    // Populate, unless a matching database is already there.
    let mut conn = store::open(&db).expect("open db");
    store::init_schema(&conn).expect("init schema");
    let existing = store::count(&conn).expect("count");
    if existing != tracks {
        if existing != 0 {
            drop(conn);
            std::fs::remove_file(&db).expect("remove stale db");
            conn = store::open(&db).expect("reopen db");
            store::init_schema(&conn).expect("init schema");
        }
        let t = Instant::now();
        populate(&mut conn, SEED, tracks).expect("populate");
        report("populate (generate + insert)", t.elapsed(), None);
    } else {
        println!("reusing existing database");
    }
    let db_size = std::fs::metadata(&db).map(|m| m.len()).unwrap_or(0);
    println!("db size: {:.2} GB", db_size as f64 / 1e9);

    // Cold open: serial (the ADR 5 shape as written) vs sharded readers.
    if !skip_serial {
        let t = Instant::now();
        let p = Projection::load_serial(&conn).expect("serial load");
        report("cold open, serial (1 reader)", t.elapsed(), None);
        assert_eq!(p.len() as u64, tracks);
        drop(p);
    }

    let shards = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8);
    let t = Instant::now();
    let p = Projection::load_parallel(&db, shards).expect("parallel load");
    report(&format!("cold open, parallel ({shards} readers)"), t.elapsed(), None);
    assert_eq!(p.len() as u64, tracks);

    println!(
        "projection heap: {:.2} GB ({} artists, {} albums, {} genres)",
        p.heap_bytes() as f64 / 1e9,
        p.artists.strings.len(),
        p.albums.strings.len(),
        p.genres.strings.len(),
    );
    if let Some(rss) = rss_gb() {
        println!("process RSS: {rss:.2} GB");
    }

    // Sort index builds: the cost of clicking a column header.
    let t = Instant::now();
    let by_artist = p.sort_artist_album_track();
    report("sort: artist / album / track", t.elapsed(), None);
    let t = Instant::now();
    let by_title = p.sort_title();
    report("sort: title", t.elapsed(), None);
    let t = Instant::now();
    p.sort_year();
    report("sort: year", t.elapsed(), None);
    drop(by_title);

    // Search: warm run reported (first run touches cold pages).
    for query in ["velvet thunder", "moon", "the", "a", "qqqzzz"] {
        p.search(query);
        let t = Instant::now();
        let hits = p.search(query);
        report(&format!("search \"{query}\""), t.elapsed(), Some(hits.len()));
    }

    // Filters.
    let t = Instant::now();
    let hits = p.filter_genre("Jazz");
    report("filter: genre = Jazz", t.elapsed(), Some(hits.len()));
    let t = Instant::now();
    let hits = p.filter_year(1990, 1999);
    report("filter: year 1990-1999", t.elapsed(), Some(hits.len()));

    // Scroll: resolve 1000 random 50-row windows out of the canonical order,
    // the per-frame work of a fast scrollbar drag.
    let mut rng = Rng::new(SEED);
    let mut sink = 0usize;
    let t = Instant::now();
    for _ in 0..1000 {
        let start = rng.below((p.len() - 50) as u64) as usize;
        for &row in &by_artist[start..start + 50] {
            let v = p.resolve(row);
            sink += v.title.len() + v.artist.len() + v.album.len() + v.duration_ms as usize % 2;
        }
    }
    let per = t.elapsed() / 1000;
    println!("{:<44} {:>10.3?} per 50-row window", "scroll: resolve windows", per);
    std::hint::black_box(sink);

    // Contrast: the same substring search pushed down to SQLite.
    if !skip_like {
        let t = Instant::now();
        let n = like_search(&conn, "moon").expect("like search");
        report("sqlite LIKE \"%moon%\" (contrast)", t.elapsed(), Some(n as usize));
    }
}

/// Generate and insert `total` synthetic tracks in 50k-row transactions.
fn populate(conn: &mut rusqlite::Connection, seed: u64, total: u64) -> rusqlite::Result<()> {
    const BATCH: usize = 50_000;
    let mut batch = Vec::with_capacity(BATCH);
    let mut pending = Ok(());
    gen::generate(seed, total, |row| {
        batch.push(row);
        if batch.len() == BATCH && pending.is_ok() {
            pending = store::insert_batch(conn, &batch);
            batch.clear();
        }
    });
    pending?;
    store::insert_batch(conn, &batch)
}

/// The naive alternative the projection replaces: substring search pushed down
/// to SQLite. Timed for contrast.
fn like_search(conn: &rusqlite::Connection, needle: &str) -> rusqlite::Result<u64> {
    let pattern = format!("%{needle}%");
    conn.query_row(
        "SELECT COUNT(*) FROM tracks
         WHERE title LIKE ?1 OR artist LIKE ?1 OR album LIKE ?1",
        [&pattern],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n as u64)
}

fn report(label: &str, elapsed: std::time::Duration, hits: Option<usize>) {
    match hits {
        Some(n) => println!("{label:<44} {elapsed:>10.3?}   {n} hits"),
        None => println!("{label:<44} {elapsed:>10.3?}"),
    }
}

fn rss_gb() -> Option<f64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    let kb: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1e6)
}
