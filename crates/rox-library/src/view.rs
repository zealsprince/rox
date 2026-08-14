//! What a track list actually shows: the projection narrowed by a search
//! and a filter, put through a sort, and walked into display rows with a
//! header block opening every group run.
//!
//! All of it is arithmetic over the projection's interned columns, so it
//! sits here next to the projection instead of inside a panel. The panel
//! keeps the parts that need a window: resolving its column keys to
//! [`SortKey`]s, and drawing the rows this hands back.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::projection::{FilterSet, Projection, SortKey};

/// One display row of a track list: a track from the projection, or a line
/// of the group header opening the artist/album run that follows it.
/// Headers open whatever runs the current order holds: the canonical
/// order's groups, or the runs a column sort leaves adjacent. Search hits
/// render flat. Headers live in the same index space as tracks, so a
/// virtualized table scrolls them like any row. A table draws
/// every row one fixed height, so a header block is one row per composed
/// line, each drawing its own piece list.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Row {
    Track(u32),
    /// One line of a group's header block: the group (indexing the groups
    /// vector this comes back with) and which composed line this row draws.
    Head(u32, u8),
    /// The divider opening one disc's run inside a multi-disc group.
    Disc(u16),
}

/// One group of the current view: what its header rows draw. The name,
/// year, and genre resolve through the first track.
#[derive(Debug)]
pub struct Group {
    pub first: u32,
    pub tracks: u32,
    pub total_ms: u64,
    /// The group's codec symbol while every track agrees; None once two
    /// differ, and the meta line drops it.
    pub codec: Option<u32>,
    /// The bitrate spread over tracks that carry one, in kbps; both 0 when
    /// none does.
    pub min_kbps: u16,
    pub max_kbps: u16,
    /// The run's bit depth and sample rate while every track agrees, None
    /// once two differ, the same all-or-nothing rule the codec follows:
    /// a mixed album has no one shape to name. Option rather than a 0
    /// sentinel because 0 is also what a track with an unread depth or
    /// rate carries, and a group where every track agrees on "unread" is
    /// not a group that disagrees.
    pub bit_depth: Option<u8>,
    pub sample_rate_hz: Option<u32>,
    /// The cover paths the header tile loads by, resolved through the
    /// store once on the group's first paint: the run's first cover, or up
    /// to four distinct albums' covers when the grouping mosaics (genre).
    /// The inner vec empty is a group with nothing to show.
    pub art: Option<Vec<PathBuf>>,
}

impl Group {
    /// The group's codec name, resolved off the interned symbol, while
    /// every track in the run agrees on one.
    pub fn codec_name<'a>(&self, projection: &'a Projection) -> Option<&'a str> {
        self.codec
            .map(|sym| projection.codecs.strings[sym as usize].as_str())
    }
}

/// How a view breaks its rows into groups. The caller owns the mapping
/// from its own config, since the key is the only thing that differs
/// between grouping by album, artist, genre, or year.
pub struct Grouping<'a> {
    /// How many header rows open each run, one per composed line.
    pub head_rows: u8,
    /// The re-sort a grouping needs before its runs are contiguous; None
    /// keeps the canonical order.
    pub pre_sort: Option<SortKey>,
    /// The group key a row belongs to. Rows sharing a key and sitting next
    /// to each other are one run.
    pub key: &'a dyn Fn(&Projection, u32) -> u64,
    /// Whether a run spanning several discs gets a divider row opening each
    /// numbered disc. Only album grouping does; the others mix discs by
    /// definition.
    pub discs: bool,
}

/// Everything a view needs beyond the projection and the canonical order.
pub struct ViewSpec<'a> {
    pub query: &'a str,
    pub filter: &'a FilterSet,
    /// Similarity scores by db id and the sort direction, when the similar
    /// column owns the sort. Takes precedence over `sort`.
    pub similar: Option<(&'a HashMap<i64, f32>, bool)>,
    /// The column sort, when one is set.
    pub sort: Option<(SortKey, bool)>,
    /// The grouping to lay headers out under. A sorted view groups the runs
    /// the sort leaves adjacent; a search renders flat however this is set.
    pub grouping: Option<Grouping<'a>>,
}

/// The rows a view shows: the canonical order or search hits, narrowed by
/// the structured filter, put through the active sort when one is set.
/// Grouping headers open the runs of whichever order shows: the canonical
/// groups unsorted, or the runs a column sort leaves adjacent (an album
/// scanned in one go stays together under the added sort, and keeps its
/// header). Search hits render flat.
pub fn view_for(
    projection: &Projection,
    order: Arc<Vec<u32>>,
    spec: &ViewSpec,
) -> (Arc<Vec<Row>>, Vec<Group>) {
    let base = if spec.query.is_empty() {
        order
    } else {
        Arc::new(projection.search(spec.query))
    };
    let base = match projection.filter_mask(spec.filter) {
        Some(mask) => Arc::new(
            base.iter()
                .copied()
                .filter(|&row| mask[row as usize])
                .collect(),
        ),
        None => base,
    };
    // Similarity sorts on the caller's score map rather than a projection
    // field, so it takes its own branch. Anything unscored sinks to the
    // bottom either way: a track with no vector isn't the least similar
    // thing in the library, it's an unknown, and floating those to the top
    // of an ascending sort would bury the real answer.
    if let Some((scores, desc)) = spec.similar {
        // Stable, so tracks scoring the same keep the canonical order under
        // each other rather than shuffling between paints.
        let mut rows: Vec<u32> = base.iter().copied().collect();
        rows.sort_by(|a, b| {
            let (a, b) = (
                scores.get(&projection.db_id[*a as usize]),
                scores.get(&projection.db_id[*b as usize]),
            );
            match (a, b) {
                (Some(a), Some(b)) => {
                    if desc {
                        b.total_cmp(a)
                    } else {
                        a.total_cmp(b)
                    }
                }
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        });
        return (
            Arc::new(rows.into_iter().map(Row::Track).collect()),
            Vec::new(),
        );
    }
    match spec.sort {
        Some((key, desc)) => {
            let sorted = projection.sort_view(&base, key, desc);
            match &spec.grouping {
                // The sort itself is the order, so the grouping's pre-sort
                // goes unused; `group_rows` breaks on adjacency and a key
                // recurring later just opens a fresh group. Loners go
                // bare: a run of one is no series, and a sort that
                // scatters every group reads as the flat list it is.
                Some(grouping) if spec.query.is_empty() => {
                    let (rows, groups) = group_rows(&sorted, projection, grouping, false);
                    (Arc::new(rows), groups)
                }
                _ => (
                    Arc::new(sorted.into_iter().map(Row::Track).collect()),
                    Vec::new(),
                ),
            }
        }
        None => match &spec.grouping {
            // A query breaks the runs the headers name, so hits render flat
            // whatever grouping the caller asked for.
            Some(grouping) if spec.query.is_empty() => {
                // Genre and year runs aren't contiguous in the canonical
                // order; re-sort by the group field, canonical inside.
                let base = match grouping.pre_sort {
                    Some(key) => Arc::new(projection.sort_view(&base, key, false)),
                    None => base,
                };
                let (rows, groups) = group_rows(&base, projection, grouping, true);
                (Arc::new(rows), groups)
            }
            _ => (
                Arc::new(base.iter().copied().map(Row::Track).collect()),
                Vec::new(),
            ),
        },
    }
}

/// The given order with a header block opening every group run:
/// `head_rows` rows per block, one per composed line. Runs are adjacency
/// in the given order, so a key recurring later opens a fresh group: under
/// a column sort an album can split, and each piece heads itself.
/// Album groups break on the album artist, not the track artist, so a
/// compilation stays one run with its per-track artists inside, and a
/// group spanning discs gets a divider row opening each numbered disc's
/// run, as long as the run lists its discs in order (the canonical order
/// always does; a column sort can interleave them, and an out-of-order
/// run reads better undivided). Untagged tracks (disc 0) sit under the
/// header undivided. Breaks compare interned symbols (years their raw
/// value) and the stats are two integer sums, so the walk stays cheap and
/// runs once per view swap, never while scrolling.
///
/// `solo_heads` says whether a run of one track still opens a block. The
/// canonical order heads everything (a single is its own album); a sorted
/// view leaves loners bare, headers only over the runs that held together.
pub fn group_rows(
    order: &[u32],
    projection: &Projection,
    grouping: &Grouping,
    solo_heads: bool,
) -> (Vec<Row>, Vec<Group>) {
    let mut rows = Vec::with_capacity(order.len() + order.len() / 8);
    let mut groups: Vec<Group> = Vec::new();
    let key = |row: u32| -> u64 { (grouping.key)(projection, row) };
    let mut i = 0;
    while i < order.len() {
        // One album run: the order keeps a group contiguous, so its extent
        // is known before any of its rows are pushed, which is what lets
        // the first disc get its divider too.
        let mut j = i + 1;
        while j < order.len() && key(order[j]) == key(order[i]) {
            j += 1;
        }
        let run = &order[i..j];
        i = j;

        if run.len() == 1 && !solo_heads {
            rows.push(Row::Track(run[0]));
            continue;
        }

        let g = groups.len() as u32;
        groups.push(Group {
            first: run[0],
            tracks: 0,
            total_ms: 0,
            codec: Some(projection.codec[run[0] as usize]),
            min_kbps: 0,
            max_kbps: 0,
            bit_depth: Some(projection.bit_depth[run[0] as usize]),
            sample_rate_hz: Some(projection.sample_rate_hz[run[0] as usize]),
            art: None,
        });
        for line in 0..grouping.head_rows {
            rows.push(Row::Head(g, line));
        }
        let disc = |row: u32| projection.disc_no[row as usize];
        let multi_disc = grouping.discs
            && run.iter().any(|&row| disc(row) != disc(run[0]))
            && run.windows(2).all(|pair| disc(pair[0]) <= disc(pair[1]));
        let mut last_disc = None;
        for &row in run {
            if multi_disc && disc(row) > 0 && last_disc != Some(disc(row)) {
                rows.push(Row::Disc(disc(row)));
                last_disc = Some(disc(row));
            }
            let group = groups.last_mut().unwrap();
            group.tracks += 1;
            group.total_ms += projection.duration_ms[row as usize] as u64;
            if group.codec != Some(projection.codec[row as usize]) {
                group.codec = None;
            }
            if group.bit_depth != Some(projection.bit_depth[row as usize]) {
                group.bit_depth = None;
            }
            if group.sample_rate_hz != Some(projection.sample_rate_hz[row as usize]) {
                group.sample_rate_hz = None;
            }
            let kbps = projection.bitrate_kbps[row as usize];
            if kbps > 0 {
                group.min_kbps = if group.min_kbps == 0 {
                    kbps
                } else {
                    group.min_kbps.min(kbps)
                };
                group.max_kbps = group.max_kbps.max(kbps);
            }
            rows.push(Row::Track(row));
        }
    }
    (rows, groups)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{store, TrackRow};

    #[allow(clippy::too_many_arguments)]
    fn track(
        path: &str,
        album_artist: &str,
        album: &str,
        disc_no: u16,
        track_no: u16,
        duration_ms: u32,
        codec: &str,
        bitrate_kbps: u16,
        sample_rate_hz: u32,
        bit_depth: u8,
    ) -> TrackRow {
        TrackRow {
            sub: 0,
            cue: None,
            path: path.into(),
            title: path.into(),
            artist: album_artist.into(),
            album_artist: album_artist.into(),
            album: album.into(),
            genre: String::new(),
            year: 0,
            disc_no,
            track_no,
            duration_ms,
            codec: codec.into(),
            bitrate_kbps,
            sample_rate_hz,
            bit_depth,
            rating: 0,
            replay_gain: Default::default(),
            bpm: None,
            size: 0,
            mtime: 0,
        }
    }

    fn projection(rows: &[TrackRow]) -> Projection {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(&mut conn, rows).unwrap();
        Projection::load_serial(&conn, false).unwrap()
    }

    fn by_album(projection: &Projection, row: u32) -> u64 {
        let i = row as usize;
        (projection.album_artist[i] as u64) << 32 | projection.album[i] as u64
    }

    fn grouping(head_rows: u8) -> Grouping<'static> {
        Grouping {
            head_rows,
            pre_sort: None,
            key: &by_album,
            discs: true,
        }
    }

    #[test]
    fn a_header_block_opens_every_run_and_the_stats_collapse() {
        let p = projection(&[
            track("/m/a1.flac", "A", "One", 1, 1, 1000, "flac", 900, 44100, 16),
            track("/m/a2.flac", "A", "One", 1, 2, 2000, "flac", 700, 44100, 16),
            track("/m/b1.mp3", "B", "Two", 0, 1, 3000, "mp3", 320, 48000, 0),
        ]);
        let order = p.sort_canonical();
        let (rows, groups) = group_rows(&order, &p, &grouping(2), true);

        assert_eq!(groups.len(), 2);
        // Two header lines per block, then the run's tracks.
        assert_eq!(
            rows.iter().filter(|r| matches!(r, Row::Head(0, _))).count(),
            2
        );
        assert_eq!(
            rows.iter().filter(|r| matches!(r, Row::Track(_))).count(),
            3
        );

        let a = groups
            .iter()
            .find(|g| p.resolve(g.first).album == "One")
            .expect("the A group");
        assert_eq!(a.tracks, 2);
        assert_eq!(a.total_ms, 3000);
        assert_eq!(a.codec_name(&p), Some("flac"));
        assert_eq!((a.min_kbps, a.max_kbps), (700, 900));
        assert_eq!(a.bit_depth, Some(16));
        assert_eq!(a.sample_rate_hz, Some(44100));
    }

    #[test]
    fn a_mixed_run_drops_the_shape_it_cannot_name() {
        let p = projection(&[
            track("/m/1.flac", "A", "One", 1, 1, 0, "flac", 900, 44100, 16),
            track("/m/2.mp3", "A", "One", 1, 2, 0, "mp3", 320, 48000, 24),
        ]);
        let order = p.sort_canonical();
        let (_, groups) = group_rows(&order, &p, &grouping(1), true);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].codec_name(&p), None);
        assert_eq!(groups[0].bit_depth, None);
        assert_eq!(groups[0].sample_rate_hz, None);
    }

    /// An unread depth or rate is 0 on every track, which agrees, so the
    /// group keeps the Some it started with rather than reading as a run
    /// that disagrees.
    #[test]
    fn an_unread_shape_agrees_with_itself() {
        let p = projection(&[
            track("/m/1.mp3", "A", "One", 0, 1, 0, "mp3", 320, 0, 0),
            track("/m/2.mp3", "A", "One", 0, 2, 0, "mp3", 320, 0, 0),
        ]);
        let order = p.sort_canonical();
        let (_, groups) = group_rows(&order, &p, &grouping(1), true);
        assert_eq!(groups[0].bit_depth, Some(0));
        assert_eq!(groups[0].sample_rate_hz, Some(0));
    }

    #[test]
    fn a_multi_disc_run_gets_a_divider_over_each_numbered_disc() {
        let p = projection(&[
            track("/m/1.flac", "A", "One", 1, 1, 0, "flac", 900, 44100, 16),
            track("/m/2.flac", "A", "One", 2, 1, 0, "flac", 900, 44100, 16),
        ]);
        let order = p.sort_canonical();
        let (rows, _) = group_rows(&order, &p, &grouping(1), true);
        let discs: Vec<u16> = rows
            .iter()
            .filter_map(|r| match r {
                Row::Disc(d) => Some(*d),
                _ => None,
            })
            .collect();
        assert_eq!(discs, vec![1, 2]);

        // One disc, or untagged tracks, get no dividers at all.
        let flat = projection(&[
            track("/m/1.flac", "A", "One", 0, 1, 0, "flac", 900, 44100, 16),
            track("/m/2.flac", "A", "One", 0, 2, 0, "flac", 900, 44100, 16),
        ]);
        let order = flat.sort_canonical();
        let (rows, _) = group_rows(&order, &flat, &grouping(1), true);
        assert!(!rows.iter().any(|r| matches!(r, Row::Disc(_))));

        // A run whose discs come through out of order (a column sort can
        // interleave them) stays undivided too.
        let p = projection(&[
            track("/m/1.flac", "A", "One", 1, 1, 0, "flac", 900, 44100, 16),
            track("/m/2.flac", "A", "One", 2, 1, 0, "flac", 900, 44100, 16),
        ]);
        let order: Vec<u32> = p.sort_canonical().into_iter().rev().collect();
        let (rows, _) = group_rows(&order, &p, &grouping(1), true);
        assert!(!rows.iter().any(|r| matches!(r, Row::Disc(_))));
    }

    /// A column sort keeps the headers over whatever runs held together,
    /// while a run of one goes bare: a loner is no series, and a sort
    /// that scatters everything reads as the flat list it is.
    #[test]
    fn a_sorted_view_heads_its_runs_and_leaves_loners_bare() {
        let p = projection(&[
            track("/m/a.flac", "A", "One", 0, 1, 0, "flac", 900, 44100, 16),
            track("/m/b.flac", "A", "One", 0, 2, 0, "flac", 900, 44100, 16),
            track("/m/c.flac", "B", "Two", 0, 1, 0, "flac", 900, 44100, 16),
            track("/m/d.flac", "B", "Two", 0, 2, 0, "flac", 900, 44100, 16),
            track("/m/e.flac", "C", "Three", 0, 1, 0, "flac", 900, 44100, 16),
        ]);
        let order = Arc::new(p.sort_canonical());
        let filter = FilterSet::default();
        // Titles read a through e, so ascending keeps One and Two whole
        // and leaves Three's lone track at the end.
        let sorted = ViewSpec {
            query: "",
            filter: &filter,
            similar: None,
            sort: Some((SortKey::Title, false)),
            grouping: Some(grouping(1)),
        };
        let (rows, groups) = view_for(&p, order, &sorted);
        assert_eq!(groups.len(), 2);
        assert_eq!(
            groups.iter().map(|g| g.tracks).collect::<Vec<_>>(),
            vec![2, 2]
        );
        // Two header lines, five tracks, and no header over the loner.
        assert_eq!(rows.len(), 7);
        match rows.last() {
            Some(&Row::Track(r)) => assert_eq!(p.resolve(r).title, "/m/e.flac"),
            other => panic!("expected the bare loner last, got {other:?}"),
        }
    }

    /// Grouping off or a search renders flat: hits are no run the headers
    /// could name.
    #[test]
    fn a_searched_view_renders_flat() {
        let p = projection(&[
            track("/m/one.flac", "A", "One", 0, 1, 0, "flac", 900, 44100, 16),
            track("/m/two.flac", "B", "Two", 0, 1, 0, "flac", 900, 44100, 16),
        ]);
        let order = Arc::new(p.sort_canonical());
        let filter = FilterSet::default();
        let grouped = ViewSpec {
            query: "",
            filter: &filter,
            similar: None,
            sort: None,
            grouping: Some(grouping(1)),
        };
        let (rows, groups) = view_for(&p, order.clone(), &grouped);
        assert_eq!(groups.len(), 2);
        assert_eq!(rows.len(), 4);

        let searched = ViewSpec {
            query: "one",
            filter: &filter,
            similar: None,
            sort: None,
            grouping: Some(grouping(1)),
        };
        let (rows, groups) = view_for(&p, order, &searched);
        assert!(groups.is_empty());
        assert_eq!(rows.len(), 1);
    }

    /// The similarity sort beats the column sort, and anything unscored
    /// sinks to the bottom whichever way the sort runs.
    #[test]
    fn unscored_tracks_sink_under_the_similarity_sort() {
        let p = projection(&[
            track("/m/1.flac", "A", "One", 0, 1, 0, "flac", 900, 44100, 16),
            track("/m/2.flac", "B", "Two", 0, 1, 0, "flac", 900, 44100, 16),
            track("/m/3.flac", "C", "Three", 0, 1, 0, "flac", 900, 44100, 16),
        ]);
        let order = Arc::new(p.sort_canonical());
        let filter = FilterSet::default();
        let scores: HashMap<i64, f32> =
            [(p.db_id[0], 0.2), (p.db_id[2], 0.9)].into_iter().collect();
        let spec = ViewSpec {
            query: "",
            filter: &filter,
            similar: Some((&scores, true)),
            sort: Some((SortKey::Title, false)),
            grouping: Some(grouping(1)),
        };
        let (rows, groups) = view_for(&p, order, &spec);
        assert!(groups.is_empty());
        let ids: Vec<i64> = rows
            .iter()
            .map(|r| match r {
                Row::Track(row) => p.db_id[*row as usize],
                other => panic!("expected a track row, got {other:?}"),
            })
            .collect();
        assert_eq!(ids, vec![p.db_id[2], p.db_id[0], p.db_id[1]]);
    }
}
