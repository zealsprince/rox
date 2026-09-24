//! The read path measured against a generated library, backing the scale
//! claims in `docs/0R-research/02-library-scale.md`.
//!
//! Runs over a database `examples/genlib.rs` wrote, named by `ROX_BENCH_DB`.
//! Unset, the file registers no benchmarks, so `cargo bench` stays green.
//!
//! ```sh
//! ROX_BENCH_DB=/tmp/rox-bench-1m.db cargo bench -p rox-library --bench scale
//! ```

use std::hint::black_box;
use std::path::PathBuf;
use std::time::Instant;

use std::collections::HashMap;

use criterion::{Criterion, criterion_group, criterion_main};
use rox_library::projection::{self, FilterField, FilterSet, Patch, Projection, SortKey};
use rox_library::store;

/// An absent database is the ordinary case on CI, not a failure.
fn bench_db() -> Option<PathBuf> {
    let Some(path) = std::env::var_os("ROX_BENCH_DB") else {
        eprintln!(
            "scale: ROX_BENCH_DB is unset, skipping. Generate a database first:\n  \
             cargo run --release -p rox-library --example genlib -- \
             --tracks 1000000 --out /tmp/rox-bench-1m.db"
        );
        return None;
    };
    let path = PathBuf::from(path);
    if !path.exists() {
        eprintln!(
            "scale: ROX_BENCH_DB points at {}, which isn't there, skipping.",
            path.display()
        );
        return None;
    }
    Some(path)
}

fn shards() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

/// Read off the data so the filter benches survive vocabulary changes.
fn hot_values(p: &Projection) -> (String, Vec<String>) {
    let mut genre_rows = vec![0usize; p.genres.strings.len()];
    for &sym in &p.genre {
        genre_rows[sym as usize] += 1;
    }
    let genre = genre_rows
        .iter()
        .enumerate()
        .max_by_key(|(_, n)| **n)
        .map(|(sym, _)| {
            p.genres.strings[sym]
                .split("; ")
                .next()
                .unwrap_or("")
                .to_string()
        })
        .unwrap_or_default();

    let mut artist_rows = vec![0usize; p.artists.strings.len()];
    for &sym in &p.artist {
        artist_rows[sym as usize] += 1;
    }
    let mut ranked: Vec<usize> = (0..artist_rows.len()).collect();
    ranked.sort_unstable_by_key(|&sym| std::cmp::Reverse(artist_rows[sym]));
    let artists = ranked
        .iter()
        .take(2)
        .map(|&sym| p.artists.strings[sym].clone())
        .collect();

    (genre, artists)
}

/// One changed file's upsert and remove against a full rebuild. Printed, not
/// benched: a patch mutates its input, so criterion's repeated samples would
/// measure a growing projection.
fn patch_timings(db: &std::path::Path, shards: usize) {
    let started = Instant::now();
    let mut projection = Projection::load_parallel(db, shards, false).expect("load the projection");
    let full_load = started.elapsed();
    // Capped so a small database doesn't index past the end.
    let rounds = (projection.len() / 2).min(50);
    if rounds == 0 {
        eprintln!(
            "scale: skipping the patch timings, {} rows is too few to sample",
            projection.len()
        );
        return;
    }
    let mut order = projection.sort_canonical();
    let mut index: HashMap<i64, u32> = projection
        .db_id
        .iter()
        .enumerate()
        .map(|(row, &id)| (id, row as u32))
        .collect();
    let step = (projection.len() / (rounds * 2)).max(1);
    let ids: Vec<i64> = (0..rounds * 2)
        .map(|n| projection.db_id[n * step])
        .collect();
    let conn = store::open(db).expect("open the bench database");

    let apply = |projection: &mut Projection,
                 order: &mut Vec<u32>,
                 index: &mut HashMap<i64, u32>,
                 id: i64,
                 upsert: bool| {
        let started = Instant::now();
        let mut patch = if upsert {
            let shard =
                projection::shard_for_ids(&conn, &[id], false).expect("read the changed row");
            let plays = store::plays_for_ids(&conn, shard.ids()).expect("read the play counts");
            let spans = store::cue_spans_for_ids(&conn, shard.ids()).expect("read the spans");
            projection
                .apply_upserts(shard, index, &plays, &spans)
                .expect("the shard fits")
        } else {
            Patch::default()
        };
        if !upsert {
            patch = projection.remove_ids(&[id], index);
        }
        *order = projection.patch_order(order, &patch);
        for &row in &patch.added {
            index.insert(projection.db_id[row as usize], row);
        }
        for id in &patch.gone {
            index.remove(id);
        }
        started.elapsed()
    };

    let mut upserts: Vec<f64> = Vec::new();
    let mut removes: Vec<f64> = Vec::new();
    for &id in ids.iter().take(rounds) {
        upserts.push(apply(&mut projection, &mut order, &mut index, id, true).as_secs_f64());
    }
    for &id in ids.iter().skip(rounds) {
        removes.push(apply(&mut projection, &mut order, &mut index, id, false).as_secs_f64());
    }
    let median = |mut xs: Vec<f64>| {
        xs.sort_by(f64::total_cmp);
        xs[xs.len() / 2] * 1e3
    };
    eprintln!(
        "scale: one-row patches over {} rows, {rounds} rounds each: \n\
         scale:   upsert {:.3} ms, remove {:.3} ms, full load {:.1} ms\n\
         scale:   {:.2}% of the projection is dead weight after {} patches",
        projection.len(),
        median(upserts),
        median(removes),
        full_load.as_secs_f64() * 1e3,
        projection.dead_fraction() * 100.,
        rounds * 2,
    );
}

fn benches(c: &mut Criterion) {
    let Some(db) = bench_db() else {
        return;
    };
    let shards = shards();

    // Ten samples: at a million tracks a load is most of a second.
    let mut group = c.benchmark_group("load");
    group.sample_size(10);
    group.bench_function("load_parallel", |b| {
        b.iter(|| black_box(Projection::load_parallel(&db, shards, false).expect("load")))
    });
    group.finish();

    let started = Instant::now();
    let projection = Projection::load_parallel(&db, shards, false).expect("load the projection");
    let order = projection.sort_canonical();
    let (genre, artists) = hot_values(&projection);

    // Printed: criterion can't report a size.
    eprintln!(
        "scale: {} tracks from {} in {:.2}s\n\
         scale: {} artists, {} album artists, {} albums, {} genres, {} folders\n\
         scale: projection heap {:.3} GB, filtering on genre {genre:?} and artists {artists:?}",
        projection.len(),
        db.display(),
        started.elapsed().as_secs_f64(),
        projection.artists.strings.len(),
        projection.album_artists.strings.len(),
        projection.albums.strings.len(),
        projection.genres.strings.len(),
        projection.folders.strings.len(),
        projection.heap_bytes() as f64 / 1e9,
    );

    patch_timings(&db, shards);

    let mut group = c.benchmark_group("search");
    for needle in ["a", "velvet thunder"] {
        eprintln!(
            "scale: {:?} hits {} rows",
            needle,
            projection.search(needle).len()
        );
        group.bench_function(needle, |b| {
            b.iter(|| black_box(projection.search(black_box(needle))))
        });
    }
    group.finish();

    let genre_only = FilterSet {
        fields: vec![(FilterField::Genre, vec![genre.clone()])],
        ids: None,
    };
    let genre_and_artist = FilterSet {
        fields: vec![
            (FilterField::Genre, vec![genre]),
            (FilterField::Artist, artists),
        ],
        ids: None,
    };
    // The view builder's mask intersection (`view.rs`), timed apart from the mask
    // build.
    let mask = projection
        .filter_mask(&genre_only)
        .expect("a non-empty filter has a mask");

    let mut group = c.benchmark_group("filter");
    group.bench_function("mask_genre", |b| {
        b.iter(|| black_box(projection.filter_mask(black_box(&genre_only))))
    });
    group.bench_function("mask_genre_and_artist", |b| {
        b.iter(|| black_box(projection.filter_mask(black_box(&genre_and_artist))))
    });
    group.bench_function("intersect", |b| {
        b.iter(|| {
            let rows: Vec<u32> = order
                .iter()
                .copied()
                .filter(|&row| mask[row as usize])
                .collect();
            black_box(rows)
        })
    });
    group.finish();

    // The one sort comparing strings, the research doc's slow click at 10M.
    let mut group = c.benchmark_group("sort");
    group.sample_size(10);
    group.bench_function("sort_view_title", |b| {
        b.iter(|| black_box(projection.sort_view(black_box(&order), SortKey::Title, false)))
    });
    group.finish();
}

criterion_group!(scale, benches);
criterion_main!(scale);
