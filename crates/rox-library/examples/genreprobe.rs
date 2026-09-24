//! How often the genre vote would have guessed right.
//!
//! Hides the genre on tagged tracks and asks `genre_suggest` what it would
//! have said, so the weights between album, artist and acoustic neighbours
//! can be measured instead of argued (the external lookup is left out). The
//! seed's own row never votes, so hiding the tag is just not looking at it.
//! Each sample runs each source alone and all three together. Hit rates share
//! the coverage denominator, so a source that only speaks when sure shows as
//! low coverage.
//!
//! Fixed-seed sampling, so a weight change shows up as a difference. Run it
//! against a copy of a library, never the live one.
//!
//! ```sh
//! cp ~/.local/share/rox/library.db /tmp/genreprobe.db
//! cargo run --release -p rox-library --example genreprobe -- \
//!     --db /tmp/genreprobe.db
//! ```

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use rox_library::genre_suggest::{self, NEIGHBOURS, Weights};
use rox_library::projection::Projection;
use rox_library::{embeddings, genre, genre_meta, store};

/// Top-3 is the deepest number reported.
const CAP: usize = 3;

/// SplitMix64, so the sample is reproducible without a dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Partial Fisher-Yates: the first `n` of a shuffle.
    fn sample(&mut self, mut pool: Vec<u32>, n: usize) -> Vec<u32> {
        let n = n.min(pool.len());
        for i in 0..n {
            let j = i + (self.next() % (pool.len() - i) as u64) as usize;
            pool.swap(i, j);
        }
        pool.truncate(n);
        pool
    }
}

struct Way {
    name: &'static str,
    weights: Weights,
    covered: u64,
    top1: u64,
    top3: u64,
}

impl Way {
    fn new(name: &'static str, weights: Weights) -> Self {
        Way {
            name,
            weights,
            covered: 0,
            top1: 0,
            top3: 0,
        }
    }

    fn score(&mut self, out: &[genre_suggest::Suggestion], truth: &HashSet<String>) {
        if out.is_empty() {
            return;
        }
        self.covered += 1;
        let hit = |s: &genre_suggest::Suggestion| truth.contains(&s.genre.to_lowercase());
        if hit(&out[0]) {
            self.top1 += 1;
        }
        if out.iter().any(hit) {
            self.top3 += 1;
        }
    }
}

fn share(part: u64, whole: u64) -> String {
    if whole == 0 {
        return "     -".to_string();
    }
    format!("{:5.1}%", 100. * part as f64 / whole as f64)
}

fn main() {
    let mut db: Option<PathBuf> = None;
    let mut model: Option<String> = None;
    let mut sample = 2000usize;
    let mut seed = 0x5EED_5EED_5EED_5EEDu64;
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--db" => db = it.next().map(PathBuf::from),
            "--model" => model = it.next(),
            "--sample" => sample = it.next().and_then(|n| n.parse().ok()).unwrap_or(sample),
            "--seed" => seed = it.next().and_then(|n| n.parse().ok()).unwrap_or(seed),
            "--help" | "-h" => {
                println!("genreprobe --db PATH [--model NAME] [--sample N] [--seed N]");
                return;
            }
            other => {
                eprintln!("genreprobe: unknown flag {other}");
                std::process::exit(2);
            }
        }
    }
    let Some(db) = db else {
        eprintln!("genreprobe: pass --db PATH (a copy of a library, not the live one)");
        std::process::exit(2);
    };

    let started = Instant::now();
    let conn = store::open(&db).expect("open the database");
    // The app's open path creates the side tables, and an older copy may lack
    // them. Idempotent.
    store::init_schema(&conn).expect("bring the schema up");
    // Installed as the app does, so resolution folds the same values.
    genre::set_aliases(genre_meta::aliases(&conn).expect("read the alias map"));
    let projection = Projection::load_serial(&conn, false).expect("load the projection");
    println!(
        "library {} rows, {} live, {} genre values, loaded in {:.1}s",
        projection.len(),
        projection.live_len(),
        projection.genres.strings.len(),
        started.elapsed().as_secs_f64()
    );

    let models = embeddings::models(&conn).expect("list the models");
    let model = model.unwrap_or_else(|| {
        models
            .iter()
            .max_by_key(|m| m.rows)
            .map(|m| m.model.clone())
            .unwrap_or_default()
    });
    match models.iter().find(|m| m.model == model) {
        Some(m) => println!("model {} ({} vectors, dim {})", m.model, m.rows, m.dim),
        None => println!("model {model:?} has no vectors: the acoustic ways will be empty"),
    }

    let blank: HashSet<u32> = genre_suggest::untagged(&projection).into_iter().collect();
    let pool: Vec<u32> = (0..projection.len() as u32)
        .filter(|&row| !projection.is_dead(row) && !blank.contains(&row))
        .collect();
    let seeds = Rng(seed).sample(pool.clone(), sample);
    println!(
        "sampling {} of {} tagged live rows, seed {seed}\n",
        seeds.len(),
        pool.len()
    );

    let mut ways = [
        Way::new(
            "album only",
            Weights {
                artist: 0.,
                acoustic: 0.,
                lookup: 0.,
                ..Weights::default()
            },
        ),
        Way::new(
            "artist only",
            Weights {
                album: 0.,
                acoustic: 0.,
                lookup: 0.,
                ..Weights::default()
            },
        ),
        Way::new(
            "acoustic only",
            Weights {
                album: 0.,
                artist: 0.,
                lookup: 0.,
                ..Weights::default()
            },
        ),
        Way::new("all three", Weights::default()),
    ];

    let mut fetch = Duration::ZERO;
    let mut with_vectors = 0u64;
    let voting = Instant::now();
    for &row in &seeds {
        let mut truth: HashSet<String> = HashSet::new();
        for part in
            genre::split(&projection.genres.strings[projection.genre[row as usize] as usize])
        {
            truth.insert(genre::resolve(part).to_lowercase());
        }

        // Fetched once: the expensive half, and independent of the weights.
        let at = Instant::now();
        let neighbours = embeddings::ranked(&conn, projection.db_id[row as usize], &model)
            .map(|scored| genre_suggest::nearest(scored, NEIGHBOURS))
            .unwrap_or_default();
        fetch += at.elapsed();
        with_vectors += u64::from(!neighbours.is_empty());

        for way in &mut ways {
            let out =
                genre_suggest::vote_weighted(&projection, row, &neighbours, &[], CAP, way.weights);
            way.score(&out, &truth);
        }
    }

    let seeds = seeds.len() as u64;
    println!(
        "{:<14} {:>8} {:>8} {:>8}",
        "way", "coverage", "top-1", "top-3"
    );
    for way in &ways {
        println!(
            "{:<14} {:>8} {:>8} {:>8}",
            way.name,
            share(way.covered, seeds),
            share(way.top1, seeds),
            share(way.top3, seeds),
        );
    }
    println!(
        "\n{seeds} seeds in {:.1}s, {} of them had a vector",
        voting.elapsed().as_secs_f64(),
        with_vectors
    );
    println!(
        "neighbour fetch {:.1}s total, {:.1}ms a seed ({:.0}% of the run)",
        fetch.as_secs_f64(),
        fetch.as_secs_f64() * 1000. / seeds.max(1) as f64,
        100. * fetch.as_secs_f64() / voting.elapsed().as_secs_f64(),
    );
    println!("whole probe {:.1}s", started.elapsed().as_secs_f64());
}
