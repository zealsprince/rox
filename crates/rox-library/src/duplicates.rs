//! Tracks the library holds more than once: the same artist and title within
//! a small duration tolerance, matched over the projection. Which copy to
//! keep is the caller's call.
//!
//! The regroup is a sort, not a `HashMap<key, Vec<row>>`: at ten million
//! rows the map's per-identity vectors run out of memory, where a flat
//! hashed vector costs sixteen bytes a row. Rows sharing a hash are compared
//! on their real strings, so a collision never makes a wrong group.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use rayon::prelude::*;

use crate::projection::Projection;

/// Rips and transcodes drift by padding and encoder delay, not seconds.
const DUR_TOLERANCE_MS: u32 = 1500;

pub struct MemberSpec {
    pub id: i64,
    pub codec: String,
    pub bitrate_kbps: u16,
    pub added: i64,
}

pub struct GroupSpec {
    pub title: String,
    pub artist: String,
    pub duration_ms: u32,
    /// False for one song on several releases, which auto-selection should
    /// leave alone.
    pub same_album: bool,
    pub members: Vec<MemberSpec>,
}

/// Blocking; run it off the UI thread.
pub fn match_duplicates(projection: &Projection) -> Vec<GroupSpec> {
    let key = |i: usize| -> (&str, &str) {
        (
            projection.artists.lower[projection.artist[i] as usize].as_str(),
            projection.title_lower.get(i),
        )
    };
    let mut keyed: Vec<(u64, u32)> = (0..projection.db_id.len())
        .into_par_iter()
        .filter(|&i| {
            // Never match an unparsed file (duration zero, name from its stem): two
            // songs named alike would cluster and auto-select could trash one.
            if projection.duration_ms[i] == 0 {
                return false;
            }
            // Never match cue tracks: they share one file, and trashing a "copy" would
            // take the album.
            projection.sub[i] == 0
        })
        .map(|i| (hash_key(key(i)), i as u32))
        .collect();
    keyed.par_sort_unstable();

    let mut out: Vec<GroupSpec> = Vec::new();
    let mut run = 0;
    while run < keyed.len() {
        let mut run_end = run + 1;
        while run_end < keyed.len() && keyed[run_end].0 == keyed[run].0 {
            run_end += 1;
        }
        let hashed = &mut keyed[run..run_end];
        run = run_end;
        if hashed.len() < 2 {
            continue;
        }
        // A shared hash isn't a shared identity; split the run on the real strings.
        hashed.sort_unstable_by_key(|&(_, row)| key(row as usize));
        let mut bucket = 0;
        while bucket < hashed.len() {
            let mut bucket_end = bucket + 1;
            while bucket_end < hashed.len()
                && key(hashed[bucket_end].1 as usize) == key(hashed[bucket].1 as usize)
            {
                bucket_end += 1;
            }
            let mut rows: Vec<usize> = hashed[bucket..bucket_end]
                .iter()
                .map(|&(_, row)| row as usize)
                .collect();
            bucket = bucket_end;
            if rows.len() < 2 {
                continue;
            }
            // With the stable duration sort below, the earlier row leads a tie.
            rows.sort_unstable();
            cluster_bucket(projection, &mut rows, &mut out);
        }
    }
    // Hash order is arbitrary; sort for a stable list.
    out.sort_by(|a, b| {
        (a.artist.to_lowercase(), &a.title).cmp(&(b.artist.to_lowercase(), &b.title))
    });
    out
}

/// Never persisted, so the per-process hasher seed is fine.
fn hash_key(key: (&str, &str)) -> u64 {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}

fn cluster_bucket(projection: &Projection, rows: &mut [usize], out: &mut Vec<GroupSpec>) {
    // A row joins the open cluster while within tolerance of its start, so
    // drift never chains.
    rows.sort_by_key(|&i| projection.duration_ms[i]);
    let mut start = 0;
    for end in 1..=rows.len() {
        let split = end == rows.len()
            || projection.duration_ms[rows[end]] - projection.duration_ms[rows[start]]
                > DUR_TOLERANCE_MS;
        if !split {
            continue;
        }
        if end - start >= 2 {
            let cluster = &rows[start..end];
            let lead = cluster[0];
            let same_album = cluster
                .iter()
                .all(|&i| projection.album[i] == projection.album[lead]);
            out.push(GroupSpec {
                title: projection.title.get(lead).to_owned(),
                artist: projection.artists.strings[projection.artist[lead] as usize].clone(),
                duration_ms: projection.duration_ms[lead],
                same_album,
                members: cluster
                    .iter()
                    .map(|&i| MemberSpec {
                        id: projection.db_id[i],
                        codec: projection.codecs.strings[projection.codec[i] as usize].clone(),
                        bitrate_kbps: projection.bitrate_kbps[i],
                        added: projection.added[i],
                    })
                    .collect(),
            });
        }
        start = end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rusqlite::Connection;
    use crate::{TrackRow, store};
    use std::collections::HashMap;

    fn track(path: &str, title: &str, artist: &str, album: &str, duration_ms: u32) -> TrackRow {
        TrackRow {
            remote_url: String::new(),
            remote_live: false,
            title_sort: String::new(),
            artist_sort: String::new(),
            album_artist_sort: String::new(),
            album_sort: String::new(),
            sub: 0,
            cue: None,
            path: path.into(),
            title: title.into(),
            artist: artist.into(),
            album_artist: artist.into(),
            album: album.into(),
            genre: String::new(),
            year: 0,
            disc_no: 0,
            track_no: 0,
            duration_ms,
            codec: "mp3".into(),
            bitrate_kbps: 320,
            sample_rate_hz: 44100,
            bit_depth: 0,
            rating: 0,
            replay_gain: Default::default(),
            bpm: None,
            size: 0,
            mtime: 0,
        }
    }

    fn projection(rows: &[TrackRow]) -> Projection {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(&mut conn, rows).unwrap();
        Projection::load_serial(&conn, false).unwrap()
    }

    #[test]
    fn real_duplicates_cluster() {
        let p = projection(&[
            track("/a/song.mp3", "Song", "Artist", "Album", 200_000),
            track("/b/song.mp3", "Song", "Artist", "Album", 200_800),
        ]);
        let groups = match_duplicates(&p);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].members.len(), 2);
        assert!(groups[0].same_album);
    }

    #[test]
    fn zero_duration_rows_never_cluster() {
        let p = projection(&[
            track("/a/track.mp3", "track", "", "", 0),
            track("/b/track.mp3", "track", "", "", 0),
        ]);
        assert!(match_duplicates(&p).is_empty());
    }

    #[test]
    fn zero_duration_row_excluded_from_a_real_group() {
        let p = projection(&[
            track("/a/song.mp3", "Song", "Artist", "Album", 200_000),
            track("/b/song.mp3", "Song", "Artist", "Album", 200_500),
            track("/c/song.mp3", "Song", "Artist", "Album", 0),
        ]);
        let groups = match_duplicates(&p);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].members.len(), 2);
    }

    #[test]
    fn far_apart_durations_do_not_cluster() {
        let p = projection(&[
            track("/a/song.mp3", "Song", "Artist", "Album", 200_000),
            track("/b/song.mp3", "Song", "Artist", "Album", 260_000),
        ]);
        assert!(match_duplicates(&p).is_empty());
    }

    #[test]
    fn identity_folds_case() {
        let p = projection(&[
            track("/a/1.mp3", "Song", "ABBA", "Gold", 200_000),
            track("/b/2.mp3", "song", "Abba", "Gold", 200_400),
        ]);
        let groups = match_duplicates(&p);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].members.len(), 2);
    }

    #[test]
    fn cross_album_copies_flag_not_same_album() {
        let p = projection(&[
            track("/a/song.mp3", "Song", "Artist", "Singles", 200_000),
            track("/b/song.mp3", "Song", "Artist", "Greatest Hits", 200_300),
        ]);
        let groups = match_duplicates(&p);
        assert_eq!(groups.len(), 1);
        assert!(!groups[0].same_album);
    }

    #[test]
    fn cue_tracks_of_one_image_are_not_duplicates() {
        let image = "/m/Album/disc.flac";
        let cue = |sub: u16| TrackRow {
            sub,
            track_no: sub,
            cue: Some(crate::CueSlice {
                cue_path: "/m/Album/disc.cue".into(),
                span: crate::cue::Span {
                    start_ms: u32::from(sub) * 200_000,
                    end_ms: None,
                },
            }),
            ..track(image, "Reprise", "Artist", "Album", 200_000)
        };
        let p = projection(&[cue(1), cue(2), cue(3)]);
        assert!(match_duplicates(&p).is_empty());
    }

    #[test]
    fn single_copy_is_no_group() {
        let p = projection(&[track("/a/song.mp3", "Song", "Artist", "Album", 200_000)]);
        assert!(match_duplicates(&p).is_empty());
    }

    /// The reference the sort has to agree with.
    fn match_by_map(projection: &Projection) -> Vec<GroupSpec> {
        let mut by_key: HashMap<(&str, &str), Vec<usize>> = HashMap::new();
        for i in 0..projection.db_id.len() {
            if projection.duration_ms[i] == 0 || projection.sub[i] > 0 {
                continue;
            }
            let artist_lower = projection.artists.lower[projection.artist[i] as usize].as_str();
            by_key
                .entry((artist_lower, projection.title_lower.get(i)))
                .or_default()
                .push(i);
        }
        let mut out = Vec::new();
        for (_, mut rows) in by_key {
            if rows.len() < 2 {
                continue;
            }
            cluster_bucket(projection, &mut rows, &mut out);
        }
        out.sort_by(|a, b| {
            (a.artist.to_lowercase(), &a.title).cmp(&(b.artist.to_lowercase(), &b.title))
        });
        out
    }

    fn shape(groups: &[GroupSpec]) -> Vec<(String, String, u32, bool, Vec<i64>)> {
        groups
            .iter()
            .map(|g| {
                (
                    g.title.clone(),
                    g.artist.clone(),
                    g.duration_ms,
                    g.same_album,
                    g.members.iter().map(|m| m.id).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn the_sorted_regroup_matches_the_map() {
        let p = projection(&[
            track("/a/song.mp3", "Song", "Artist", "Album", 200_000),
            track("/b/song.mp3", "Song", "Artist", "Album", 200_400),
            track("/c/song.mp3", "Song", "Artist", "Album", 260_000),
            track("/d/gold.mp3", "Dancing", "ABBA", "Gold", 190_000),
            track("/e/gold.mp3", "dancing", "Abba", "Greatest", 190_200),
            track("/f/song.mp3", "Song", "Other", "Album", 200_000),
            track("/g/song.mp3", "Song", "Other", "Album", 200_100),
            track("/h/twin.mp3", "Twin", "Artist", "Album", 123_000),
            track("/i/twin.mp3", "Twin", "Artist", "Album", 123_000),
            track("/j/broken.mp3", "broken", "", "", 0),
            track("/k/alone.mp3", "Alone", "Artist", "Album", 150_000),
        ]);
        assert_eq!(shape(&match_duplicates(&p)), shape(&match_by_map(&p)));
        assert_eq!(match_duplicates(&p).len(), 4);
    }
}
