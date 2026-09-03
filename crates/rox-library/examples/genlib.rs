//! Synthesize an N-track library database, no audio files involved.
//!
//! The scale numbers in `docs/0R-research/02-library-scale.md` came out of a
//! prototype crate that no longer exists, so nothing in the tree could
//! reproduce them and every claim about the projection at a million tracks
//! was an assertion. This generator is the missing half: it writes rows
//! straight through [`store::insert_batch`], the same path the scanner uses,
//! so the database the benches load is the shape the app actually produces.
//!
//! Realistic shape matters here, realism doesn't. Nobody needs the titles to
//! read well; what has to hold is the cardinality, because the projection's
//! whole design rests on interned columns being a hundredth of the row count.
//! The research doc measured 272k artists and 433k distinct album names at 10
//! million tracks, so the pools scale at those ratios (2.72% and 4.33% of N)
//! and the picks are Zipf-ish, giving a few artists a long tail of tracks the
//! way a real collection does. Every column is deterministic from `--seed`,
//! `added` included (insert_batch stamps that one with the clock, so the run
//! rewrites it), which is what makes a before/after measurement compare like
//! with like: two runs at the same N hold the same rows. Not the same bytes,
//! though. The pages carry slack from the `added` rewrite, so the files
//! differ where their contents don't.
//!
//! ```sh
//! cargo run --release -p rox-library --example genlib -- \
//!     --tracks 1000000 --out /tmp/rox-bench-1m.db
//! ```

use std::path::PathBuf;
use std::time::Instant;

use rox_library::replaygain::ReplayGain;
use rox_library::{store, TrackRow};

/// Distinct artists as a share of tracks, and distinct album names the same
/// way. Straight off the research doc's 10M row: 272k artists, 433k albums.
const ARTIST_SHARE: f64 = 0.0272;
const ALBUM_SHARE: f64 = 0.0433;

/// SplitMix64. Small enough to read, good enough for synthetic tags, and it
/// keeps `rand` out of the crate for a generator nobody ships.
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
        if n == 0 {
            0
        } else {
            self.next_u64() % n
        }
    }

    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// A Zipf-ish index into `0..n`: `n^u` for uniform `u` has density
    /// proportional to 1/x, so the low indices take most of the draws and
    /// the tail still gets hit. Cheap, and the exact exponent doesn't
    /// matter as long as the reuse is skewed rather than flat.
    fn zipf(&mut self, n: u64) -> u64 {
        if n <= 1 {
            return 0;
        }
        let u = self.unit();
        ((n as f64).powf(u) as u64).saturating_sub(1).min(n - 1)
    }
}

/// One shared vocabulary for artists, albums, and titles. 128 words gives
/// 16k two-word names and 2.1M three-word ones, which covers the artist and
/// album pools past 10 million tracks without a numeric suffix anywhere.
/// "moon", "velvet" and "thunder" are in here on purpose: the research doc's
/// search timings used those needles, so the benches can reuse them.
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

/// A genre list with the usual long tail. The picks are Zipf-ish over this
/// order, so rock and electronic carry most of the library and the bottom
/// half stays rare, which is what the filter panel's value lists look like.
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

/// The four containers with the numbers a scanner would read off them:
/// extension, bitrate kbps, sample rate, bit depth, and bytes a second for
/// the file size. Weights are the first field of the pick below.
const CODECS: [(&str, u16, u32, u8, u64); 4] = [
    ("flac", 900, 44100, 16, 112_000),
    ("mp3", 320, 44100, 0, 40_000),
    ("m4a", 256, 44100, 0, 32_000),
    ("ogg", 192, 48000, 0, 24_000),
];

/// A name from the shared vocabulary: two words below 16k, three above, so
/// the map from index to name is injective across the whole pool and every
/// generated pool entry is genuinely distinct.
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

/// A stride that walks `0..n` without repeating, so a sequential pass over
/// album entities touches every artist exactly once while the names it picks
/// stay scattered. Without the scatter the first two thirds of the database
/// would hold artists in index order, which flatters the sharded load's
/// symbol merge.
fn stride(n: u64) -> u64 {
    fn gcd(a: u64, b: u64) -> u64 {
        if b == 0 {
            a
        } else {
            gcd(b, a % b)
        }
    }
    let mut k = (n / 3).max(1) | 1;
    while gcd(k, n) != 1 {
        k += 2;
    }
    k
}

/// The artist and album name one album entity is filed under. Every artist
/// gets an album of its own before any reuse starts, which pins the distinct
/// count to the pool size instead of leaving it to how the Zipf tail happened
/// to fall; past that the draws are skewed, so the artists that came up early
/// keep coming up.
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

/// The table genlib stamps its own output with, and the only thing that makes
/// an existing `--out` safe to delete. A synthetic library and a real one are
/// the same shape, so without a marker the difference between regenerating a
/// bench database and wiping somebody's library is one mistyped path.
const MARKER: &str = "genlib";

/// Whether a file already at `--out` is a database genlib wrote.
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
        // Big on purpose. insert_batch asks the whole table once a batch
        // whether any row carries a measured gain, and there's no index
        // behind that question, so a small batch turns the populate into a
        // table scan per few thousand rows.
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

    // A stale database would upsert onto its rows instead of writing fresh
    // ones, which is a different measurement than the one this makes. Only
    // genlib's own output gets deleted for that, though: --out points at a
    // library-shaped file, and the one it points at by accident is somebody's
    // real library.
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
    // Album length averages tracks-per-album so the entity count lands on
    // the album pool size, which is what makes the distinct-name count come
    // out at the target instead of near it.
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
    // Whose album this is and what it's called, drawn once when the entity
    // advances. Per-track would give one directory a different artist every
    // file, which is not a library, it's a folder of loose tracks.
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
            // Weighted 40/35/15/10 across the table above, which is roughly
            // what a collection that has been re-ripped a few times looks
            // like.
            let roll = rng.below(100);
            CODECS[match roll {
                0..=39 => 0,
                40..=74 => 1,
                75..=89 => 2,
                _ => 3,
            }]
        };
        let duration_ms = 90_000 + rng.below(360_000) as u32;
        // A tenth of the library carries a second genre, because the column
        // holds "; " lists and the filter has to walk them.
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
        // Untagged years are real and the projection has a symbol for them,
        // so a few percent of the library has none.
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
        // Two fifths of files carry ReplayGain tags, all of them read off
        // the file rather than measured by rox: a measured row makes
        // insert_batch take its per-row re-meter branch, which is a
        // different write path than the one a scan of tagged files walks.
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
        // A twentieth of the library has sort names, which is about what a
        // collection with some Japanese and some classical in it carries.
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

    // insert_batch stamps `added` with the wall clock, the one column that
    // would otherwise differ between two runs at the same seed. Rewrite it to
    // a seed-derived spread over five years, so what a run produces depends
    // on its arguments and nothing else, and a sort by date added still has
    // something scattered to sort.
    let step = (Rng::new(args.seed ^ 0xADDED).next_u64() % 100_003) as i64 | 1;
    conn.execute(
        "UPDATE tracks SET added = 1500000000 + (id * ?1) % 157680000",
        [step],
    )
    .expect("stamp the added column");

    // WAL checkpoint before the size report, or most of the database is
    // still sitting in the -wal file and the number is a fiction.
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
