//! Finding the tracks the library carries more than once. A duplicate here
//! is a tag identity, the same title and artist within a small duration
//! tolerance, matched over the in-memory projection so a scan never walks
//! the disk. The match hands back bare specs: ids, not paths, and no
//! ordering policy, because which copy to keep is the caller's call.

use std::collections::HashMap;

use crate::projection::Projection;

/// How far apart two durations can sit and still read as the same
/// recording. Rips and transcodes of one track drift by padding and
/// encoder delay, not by seconds; anything past this is a different take.
const DUR_TOLERANCE_MS: u32 = 1500;

/// What the match hands back per member, before the caller resolves ids to
/// paths: everything else comes straight off the projection.
pub struct MemberSpec {
    pub id: i64,
    pub codec: String,
    pub bitrate_kbps: u16,
    pub added: i64,
}

/// One matched group, paths still unresolved.
pub struct GroupSpec {
    pub title: String,
    pub artist: String,
    pub duration_ms: u32,
    /// Whether every copy carries the same album tag. Copies spread over
    /// different albums are one song on several releases, which is the
    /// case a caller's auto-selection should leave alone.
    pub same_album: bool,
    pub members: Vec<MemberSpec>,
}

/// Group the projection's rows into duplicate identities: the same artist
/// and case-folded title, clustered within the duration tolerance. Each
/// cluster of two or more becomes a group; the caller orders members per
/// its keep policy. Blocking; run it off the UI thread.
pub fn match_duplicates(projection: &Projection) -> Vec<GroupSpec> {
    // Bucket by identity first; the map borrows the projection's strings,
    // nothing is owned until a bucket proves duplicated. Key on the folded
    // artist, not the case-sensitive symbol, so "ABBA" and "Abba" land in one
    // bucket like the folded title does; distinct symbols share a lower form.
    let mut by_key: HashMap<(&str, &str), Vec<usize>> = HashMap::new();
    for i in 0..projection.db_id.len() {
        // A file whose tags failed to parse scans as its filename stem, no
        // artist, no album, duration zero - no evidence of identity at all.
        // Two different songs named alike would cluster, pass the duration
        // and album checks on their empty fallbacks, and auto-select could
        // mark one for the trash. Keep such rows out of the matching.
        if projection.duration_ms[i] == 0 {
            continue;
        }
        // Cue tracks are spans of one image, so every row of a disc shares a
        // path. A pass that let them in would offer to trash "copies" that
        // are really the same file, and deleting one would take the album.
        // Their identity question is whether the disc is ripped twice, which
        // is not what this clustering answers.
        if projection.sub[i] > 0 {
            continue;
        }
        let artist_lower = projection.artists.lower[projection.artist[i] as usize].as_str();
        by_key
            .entry((artist_lower, projection.title_lower.get(i)))
            .or_default()
            .push(i);
    }
    let mut out: Vec<GroupSpec> = Vec::new();
    for ((_, _), mut rows) in by_key {
        if rows.len() < 2 {
            continue;
        }
        // Cluster by duration inside the bucket: sorted, a row joins the
        // open cluster while it sits within tolerance of the cluster's
        // start, so drift never chains far past it.
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
    // The map's order is arbitrary; artist then title keeps the list
    // stable across rescans.
    out.sort_by(|a, b| {
        (a.artist.to_lowercase(), &a.title).cmp(&(b.artist.to_lowercase(), &b.title))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rusqlite::Connection;
    use crate::{store, TrackRow};

    /// A track row with just the fields the clustering reads; the rest sit at
    /// their neutral defaults.
    fn track(path: &str, title: &str, artist: &str, album: &str, duration_ms: u32) -> TrackRow {
        TrackRow {
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
            size: 0,
            mtime: 0,
        }
    }

    /// Load a projection from an in-memory database seeded with the rows, the
    /// same path the app builds its read model over.
    fn projection(rows: &[TrackRow]) -> Projection {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(&mut conn, rows).unwrap();
        Projection::load_serial(&conn, false).unwrap()
    }

    /// Real duplicates - same artist and title, durations within tolerance -
    /// cluster into one group, so the tool can offer to trash a spare.
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

    /// The deletion-safety fix: two distinct zero-duration rows (files whose
    /// tags failed to parse, so they scan as their filename stem with duration
    /// zero) must never cluster. They carry no evidence of identity, and a
    /// false cluster could auto-mark one for the trash.
    #[test]
    fn zero_duration_rows_never_cluster() {
        // Same empty artist, same stem-derived title, both duration zero: the
        // shape a pair of unparseable files would take. Without the guard they
        // would bucket together and pass the duration check on their zeros.
        let p = projection(&[
            track("/a/track.mp3", "track", "", "", 0),
            track("/b/track.mp3", "track", "", "", 0),
        ]);
        assert!(match_duplicates(&p).is_empty());
    }

    /// A zero-duration row is dropped even when it shares a real track's
    /// identity, so a broken copy can't drag a good one into a group and get
    /// itself trashed.
    #[test]
    fn zero_duration_row_excluded_from_a_real_group() {
        let p = projection(&[
            track("/a/song.mp3", "Song", "Artist", "Album", 200_000),
            track("/b/song.mp3", "Song", "Artist", "Album", 200_500),
            track("/c/song.mp3", "Song", "Artist", "Album", 0),
        ]);
        let groups = match_duplicates(&p);
        assert_eq!(groups.len(), 1);
        // Only the two parsed copies; the zero-duration row stayed out.
        assert_eq!(groups[0].members.len(), 2);
    }

    /// Durations past the tolerance are different takes, not copies, so they
    /// split into separate clusters and neither becomes a two-copy group.
    #[test]
    fn far_apart_durations_do_not_cluster() {
        let p = projection(&[
            track("/a/song.mp3", "Song", "Artist", "Album", 200_000),
            // Well past DUR_TOLERANCE_MS from the first.
            track("/b/song.mp3", "Song", "Artist", "Album", 260_000),
        ]);
        assert!(match_duplicates(&p).is_empty());
    }

    /// Case-folded identity: "ABBA" and "Abba", "Song" and "song" land in one
    /// bucket, so a casing difference in the tags doesn't hide a duplicate.
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

    /// Copies spread across different albums cluster (same song, several
    /// releases) but carry same_album false, the flag auto-select reads to
    /// leave them untouched so no album loses a track by default.
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

    /// Cue tracks of one image never cluster with each other. They share a
    /// path, so a group would be offering to delete the album to get rid of a
    /// "copy"; two tracks of a disc that happen to be tagged alike (a hidden
    /// track, a reprise) are exactly the case that would trigger it.
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

    /// A lone copy is not a duplicate, so it never becomes a group.
    #[test]
    fn single_copy_is_no_group() {
        let p = projection(&[track("/a/song.mp3", "Song", "Artist", "Album", 200_000)]);
        assert!(match_duplicates(&p).is_empty());
    }
}
