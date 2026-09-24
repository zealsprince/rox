//! Synthesize an N-track library database, no audio files involved, for the
//! scale numbers in `docs/0R-research/02-library-scale.md`. Rows go through
//! [`store::insert_batch`], the scanner's path, so the benches load the shape
//! the app produces.
//!
//! Cardinality is what has to be realistic: the pools scale at the research
//! doc's 10M-track ratios (272k artists, 433k albums) with Zipf-ish picks.
//! Every column is deterministic from `--seed`, `added` included, so two runs
//! at the same N hold the same rows (not the same bytes: the `added` rewrite
//! leaves page slack).
//!
//! ```sh
//! cargo run --release -p rox-library --example genlib -- \
//!     --tracks 1000000 --out /tmp/rox-bench-1m.db
//! ```

use std::path::PathBuf;
use std::time::Instant;

use rox_library::replaygain::ReplayGain;
use rox_library::{TrackRow, store};

/// The research doc's 10M row: 272k artists, 433k albums.
const ARTIST_SHARE: f64 = 0.0272;
const ALBUM_SHARE: f64 = 0.0433;

/// SplitMix64, to keep `rand` out of the crate.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next_u64() % n }
    }

    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// `n^u` for uniform `u` has density proportional to 1/x, so low indices take
    /// most draws.
    fn zipf(&mut self, n: u64) -> u64 {
        if n <= 1 {
            return 0;
        }
        let u = self.unit();
        ((n as f64).powf(u) as u64).saturating_sub(1).min(n - 1)
    }
}

/// 128 words give 16k two-word and 2.1M three-word names, enough past 10M
/// tracks. "moon", "velvet" and "thunder" are the research doc's search
/// needles.
const WORDS: [&str; 128] = [
    "Moon", "Velvet", "Thunder", "Amber", "Hollow", "Silver", "Winter", "Ember", "Static", "Paper",
    "Glass", "Iron", "Neon", "Salt", "River", "Copper", "Marble", "Cinder", "Frost", "Harbor",
    "Lantern", "Meadow", "Nocturne", "Opal", "Prairie", "Quartz", "Ridge", "Sable", "Tundra",
    "Umber", "Vapor", "Willow", "Anchor", "Bison", "Cobalt", "Dune", "Echo", "Fable", "Garnet",
    "Hazel", "Indigo", "Juniper", "Kestrel", "Lichen", "Mantle", "Nimbus", "Orchard", "Pewter",
    "Quiver", "Rumor", "Signal", "Talon", "Ultra", "Vertigo", "Wander", "Xenon", "Yarrow",
    "Zephyr", "Alloy", "Beacon", "Cascade", "Drift", "Ellipse", "Furrow", "Gable", "Halo", "Inlet",
    "Jetty", "Knoll", "Ledger", "Mirage", "Nettle", "Oasis", "Plume", "Quarry", "Rift", "Solstice",
    "Thicket", "Undertow", "Vellum", "Wren", "Yonder", "Zenith", "Ashen", "Bramble", "Clover",
    "Dusk", "Errant", "Fathom", "Glimmer", "Hearth", "Ivory", "Jasper", "Kindling", "Lumen",
    "Murmur", "Noble", "Ochre", "Pilgrim", "Quiet", "Reverie", "Slate", "Tempest", "Upland",
    "Vessel", "Wither", "Amethyst", "Bellows", "Chalk", "Delta", "Estuary", "Fissure", "Granite",
    "Hush", "Isle", "Jubilee", "Kiln", "Lattice", "Monsoon", "Nadir", "Oxide", "Pallid", "Rally",
    "Saffron", "Tidal", "Vector", "Wharf", "Yield",
];

/// Picked Zipf-ish in this order, so the bottom half stays rare.
const GENRES: [&str; 40] = [
    "Rock",
    "Electronic",
    "Pop",
    "Hip-Hop",
    "Jazz",
    "Classical",
    "Metal",
    "Folk",
    "Ambient",
    "Punk",
    "Soul",
    "Funk",
    "Blues",
    "Country",
    "Reggae",
    "House",
    "Techno",
    "Drum & Bass",
    "Dubstep",
    "Trance",
    "Indie Rock",
    "Post-Rock",
    "Shoegaze",
    "Synthpop",
    "New Wave",
    "Disco",
    "Gospel",
    "Latin",
    "World",
    "Soundtrack",
    "Spoken Word",
    "Bluegrass",
    "Ska",
    "Grime",
    "Trip-Hop",
    "Industrial",
    "Noise",
    "Drone",
    "Bossa Nova",
    "Chiptune",
];

/// Extension, bitrate kbps, sample rate, bit depth, bytes a second.
const CODECS: [(&str, u16, u32, u8, u64); 4] = [
    ("flac", 900, 44100, 16, 112_000),
    ("mp3", 320, 44100, 0, 40_000),
    ("m4a", 256, 44100, 0, 32_000),
    ("ogg", 192, 48000, 0, 24_000),
];

/// Two words below 16k, three above, so every pool index names a distinct
/// value.
fn name(index: u64) -> String {
    let n = WORDS.len() as u64;
    let (a, b) = ((index % n) as usize, ((index / n) % n) as usize);
    if index < n * n {
        format!("{} {}", WORDS[a], WORDS[b])
    } else {
        let c = ((index / (n * n)) % n) as usize;
        format!("{} {} {}", WORDS[a], WORDS[b], WORDS[c])
    }
}

/// A non-repeating stride over `0..n`. Artists in index order would flatter the
/// sharded load's symbol merge.
fn stride(n: u64) -> u64 {
    fn gcd(a: u64, b: u64) -> u64 {
        if b == 0 { a } else { gcd(b, a % b) }
    }
    let mut k = (n / 3).max(1) | 1;
    while gcd(k, n) != 1 {
        k += 2;
    }
    k
}

/// Every artist gets an album before reuse starts, pinning the distinct count
/// to the pool size.
fn credits(
    rng: &mut Rng,
    entity: u64,
    artists: u64,
    albums: u64,
    (artist_stride, album_stride): (u64, u64),
) -> (String, String) {
    let artist_ix = if entity < artists {
        entity.wrapping_mul(artist_stride) % artists
    } else {
        rng.zipf(artists)
    };
    let album_ix = (entity % albums).wrapping_mul(album_stride) % albums;
    (name(artist_ix), name(album_ix))
}

struct Args {
    tracks: u64,
    out: PathBuf,
    seed: u64,
    batch: usize,
    force: bool,
}

/// The only thing that makes an existing `--out` safe to delete: a synthetic
/// library and a real one are the same shape.
const MARKER: &str = "genlib";

fn is_genlib_db(path: &std::path::Path) -> bool {
    use rox_library::rusqlite::{Connection, OpenFlags};
    let Ok(conn) = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
        return false;
    };
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [MARKER],
        |row| row.get::<_, i64>(0),
    )
    .unwrap_or(0)
        == 1
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        tracks: 100_000,
        out: PathBuf::from("/tmp/rox-bench.db"),
        seed: 0x5EED,
        // Big on purpose: insert_batch scans the table once per batch for measured
        // gains, with no index behind it.
        batch: 50_000,
        force: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || {
            it.next()
                .ok_or_else(|| format!("{flag} wants a value after it"))
        };
        match flag.as_str() {
            "--tracks" => {
                let v = value()?;
                args.tracks = v
                    .replace(['_', ','], "")
                    .parse()
                    .map_err(|_| format!("--tracks wants a number, got {v}"))?;
            }
            "--out" => args.out = PathBuf::from(value()?),
            "--seed" => {
                let v = value()?;
                args.seed = v
                    .parse()
                    .map_err(|_| format!("--seed wants a number, got {v}"))?;
            }
            "--batch" => {
                let v = value()?;
                args.batch = v
                    .parse()
                    .map_err(|_| format!("--batch wants a number, got {v}"))?;
            }
            "--force" => args.force = true,
            "--help" | "-h" => {
                println!(
                    "genlib --tracks N --out PATH [--seed N] [--batch N] [--force]\n\
                     Writes a synthetic library database with the research doc's\n\
                     cardinalities. Deterministic from the seed. An existing --out\n\
                     is only overwritten when genlib wrote it, or with --force."
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown flag {other}")),
        }
    }
    if args.tracks == 0 {
        return Err("--tracks has to be at least 1".into());
    }
    if args.batch == 0 {
        return Err("--batch has to be at least 1".into());
    }
    Ok(args)
}

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(err) => {
            eprintln!("genlib: {err}");
            std::process::exit(2);
        }
    };

    // Only delete genlib's own output. A mistyped --out is somebody's real
    // library.
    if args.out.exists() && !args.force && !is_genlib_db(&args.out) {
        eprintln!(
            "genlib: {} already exists and genlib did not write it; \
             pass --force to overwrite it anyway",
            args.out.display()
        );
        std::process::exit(2);
    }
    for suffix in ["", "-wal", "-shm"] {
        let mut path = args.out.clone().into_os_string();
        path.push(suffix);
        let _ = std::fs::remove_file(&path);
    }

    let mut conn = store::open(&args.out).expect("open the database");
    store::init_schema(&conn).expect("build the schema");
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {MARKER} (seed INTEGER NOT NULL, tracks INTEGER NOT NULL);"
    ))
    .expect("stamp the generator marker");
    conn.execute(
        &format!("INSERT INTO {MARKER} (seed, tracks) VALUES (?1, ?2)"),
        rox_library::rusqlite::params![args.seed as i64, args.tracks as i64],
    )
    .expect("stamp the generator marker");

    let artists = ((args.tracks as f64 * ARTIST_SHARE).round() as u64).max(1);
    let albums = ((args.tracks as f64 * ALBUM_SHARE).round() as u64).max(1);
    // Lands the distinct album count on the pool size exactly.
    let per_album = (args.tracks as f64 / albums as f64).max(1.0);
    let len_span = ((2.0 * per_album).round() as u64).max(2) - 1;
    let strides = (stride(artists), stride(albums));

    eprintln!(
        "genlib: {} tracks, {} artists, {} album names, ~{:.1} tracks an album, seed {}",
        args.tracks, artists, albums, per_album, args.seed
    );

    let started = Instant::now();
    let mut rng = Rng::new(args.seed);
    let mut rows: Vec<TrackRow> = Vec::with_capacity(args.batch);
    let mut entity: u64 = 0;
    let mut in_album: u64 = 0;
    let mut album_len: u64 = 1 + rng.below(len_span);
    let mut written: u64 = 0;
    let mut last_report = 0u64;
    // Drawn per entity, not per track, so a directory has one artist.
    let (mut artist, mut album) = credits(&mut rng, entity, artists, albums, strides);

    for _ in 0..args.tracks {
        if in_album == album_len {
            entity += 1;
            in_album = 0;
            album_len = 1 + rng.below(len_span);
            (artist, album) = credits(&mut rng, entity, artists, albums, strides);
        }
        let title = name(rng.next_u64() % (WORDS.len() as u64).pow(3));

        let (codec, bitrate, sample_rate, bit_depth, bytes_per_sec) = {
            // Weighted 40/35/15/10 across the table above.
            let roll = rng.below(100);
            CODECS[match roll {
                0..=39 => 0,
                40..=74 => 1,
                75..=89 => 2,
                _ => 3,
            }]
        };
        let duration_ms = 90_000 + rng.below(360_000) as u32;
        // A tenth carry a second genre, so the filter walks "; " lists.
        let genre = {
            let first = GENRES[rng.zipf(GENRES.len() as u64) as usize];
            if rng.below(10) == 0 {
                let second = GENRES[rng.zipf(GENRES.len() as u64) as usize];
                if second == first {
                    first.to_string()
                } else {
                    format!("{first}; {second}")
                }
            } else {
                first.to_string()
            }
        };
        let year = if rng.below(100) < 3 {
            0
        } else {
            1960 + rng.below(66) as u16
        };
        let disc_no = if rng.below(100) < 5 {
            1 + rng.below(3) as u16
        } else {
            1
        };
        let track_no = (in_album + 1) as u16;
        let rating = if rng.below(100) < 15 {
            (1 + rng.below(5) as u8) * 20
        } else {
            0
        };
        // Tagged gains only: a measured row sends insert_batch down its re-meter
        // branch, a different write path than a scan takes.
        let replay_gain = if rng.below(100) < 40 {
            ReplayGain {
                track_db: Some(-12.0 + rng.unit() as f32 * 10.0),
                track_peak: Some(0.7 + rng.unit() as f32 * 0.3),
                album_db: Some(-12.0 + rng.unit() as f32 * 10.0),
                album_peak: Some(0.7 + rng.unit() as f32 * 0.3),
            }
        } else {
            ReplayGain::default()
        };
        let bpm = if rng.below(4) == 0 {
            Some(70.0 + rng.unit() as f32 * 110.0)
        } else {
            None
        };
        let sorted = rng.below(20) == 0;
        let sort_of = |s: &str| {
            if sorted {
                format!("{s}, The")
            } else {
                String::new()
            }
        };

        let path = format!("/music/{artist}/{album} [{entity}]/{track_no:02} {title}.{codec}",);
        let size = duration_ms as u64 * bytes_per_sec / 1000;

        rows.push(TrackRow {
            remote_url: String::new(),
            remote_live: false,
            title_sort: sort_of(&title),
            artist_sort: sort_of(&artist),
            album_artist_sort: sort_of(&artist),
            album_sort: sort_of(&album),
            sub: 0,
            cue: None,
            path,
            title,
            album_artist: artist.clone(),
            artist: artist.clone(),
            album: album.clone(),
            genre,
            year,
            disc_no,
            track_no,
            duration_ms,
            codec: codec.to_string(),
            bitrate_kbps: bitrate,
            sample_rate_hz: sample_rate,
            bit_depth,
            rating,
            replay_gain,
            bpm,
            size,
            mtime: 1_700_000_000 + rng.below(50_000_000) as i64,
        });
        in_album += 1;

        if rows.len() == args.batch {
            written += rows.len() as u64;
            store::insert_batch(&mut conn, &rows).expect("insert a batch");
            rows.clear();
            if written - last_report >= args.tracks.div_ceil(20) {
                last_report = written;
                eprintln!(
                    "genlib: {written}/{} tracks, {:.1}s",
                    args.tracks,
                    started.elapsed().as_secs_f64()
                );
            }
        }
    }
    if !rows.is_empty() {
        written += rows.len() as u64;
        store::insert_batch(&mut conn, &rows).expect("insert the last batch");
    }

    // insert_batch stamps `added` with the clock. Rewrite it to a seed-derived
    // five-year spread so runs are reproducible.
    let step = (Rng::new(args.seed ^ 0xADDED).next_u64() % 100_003) as i64 | 1;
    conn.execute(
        "UPDATE tracks SET added = 1500000000 + (id * ?1) % 157680000",
        [step],
    )
    .expect("stamp the added column");

    // Checkpoint first, or most of the database is still in the -wal file.
    conn.pragma_update(None, "wal_checkpoint", "TRUNCATE")
        .expect("checkpoint the WAL");
    drop(conn);

    let bytes = std::fs::metadata(&args.out).map(|m| m.len()).unwrap_or(0);
    eprintln!(
        "genlib: wrote {written} tracks to {} in {:.1}s, {:.2} GB",
        args.out.display(),
        started.elapsed().as_secs_f64(),
        bytes as f64 / 1e9
    );
}
