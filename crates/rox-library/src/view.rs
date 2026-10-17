//! What a track list shows: the projection narrowed by search and filter,
//! sorted, and walked into display rows with header blocks opening each
//! group run. The panel only resolves its column keys and draws.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::projection::{FilterSet, Projection, SortKey, SymTable};

/// Big enough that the split doesn't dominate a per-row index-and-push: a
/// library under a hundred thousand tracks stays on one thread.
const PAR_CHUNK: usize = 64 * 1024;

/// A track, or one line of a group header. Headers share the tracks' index
/// space, so a virtualized fixed-height table scrolls them like any row.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Row {
    Track(u32),
    /// (group index, composed line).
    Head(u32, u8),
    Disc(u16),
}

/// What a group's header rows draw. Name, year and genre resolve through
/// the first track.
#[derive(Debug)]
pub struct Group {
    pub first: u32,
    pub tracks: u32,
    /// The grouping's key. Symbol ids renumber when the projection rebuilds, so
    /// it only means anything within one build.
    pub key: u64,
    /// Only the header rows made it into the view.
    pub collapsed: bool,
    /// The run's span of [`View::order`], whether or not its rows are shown.
    pub members: Range<usize>,
    pub total_ms: u64,
    /// Some while every track agrees.
    pub codec: Option<u32>,
    /// Over tracks that have one; both 0 when none does.
    pub min_kbps: u16,
    pub max_kbps: u16,
    /// Some while every track agrees. Option, not a 0 sentinel, since 0 is also
    /// "unread" and a run that agrees on unread still agrees.
    pub bit_depth: Option<u8>,
    pub sample_rate_hz: Option<u32>,
    /// Resolved on first paint: the first cover, or up to four albums' covers
    /// for a mosaic grouping. Empty means nothing to show.
    pub art: Option<Vec<PathBuf>>,
}

impl Group {
    pub fn codec_name<'a>(&self, projection: &'a Projection) -> Option<&'a str> {
        self.codec
            .map(|sym| projection.codecs.strings[sym as usize].as_str())
    }
}

/// The caller maps its config onto this; only the key differs between
/// album, artist, genre and year grouping.
#[derive(Clone, Copy)]
pub struct Grouping<'a> {
    pub head_rows: u8,
    /// The re-sort that makes runs contiguous. Callers grouping a searched
    /// subset must name it.
    pub pre_sort: Option<SortKey>,
    /// Adjacent rows sharing a key are one run.
    pub key: &'a dyn Fn(&Projection, u32) -> u64,
    /// A divider row per numbered disc. Album grouping only.
    pub discs: bool,
    /// Which runs show their header alone. Only the canonical order honours
    /// it: a search or a column sort opens every group.
    pub collapse: Option<&'a CollapseKeys>,
}

/// What a group keys on. The packed key and the saved name both come from
/// here, so the two can't drift apart.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GroupKind {
    /// The album artist and album together.
    Album,
    /// The album artist.
    Artist,
    Genre,
    Year,
}

impl GroupKind {
    pub fn key(self, projection: &Projection, row: u32) -> u64 {
        let i = row as usize;
        match self {
            GroupKind::Album => {
                (projection.album_artist[i] as u64) << 32 | projection.album[i] as u64
            }
            GroupKind::Artist => projection.album_artist[i] as u64,
            GroupKind::Genre => projection.genre[i] as u64,
            GroupKind::Year => projection.year[i] as u64,
        }
    }

    /// The key spelled out, which survives a rebuild where the symbol ids
    /// don't. The raw tags, not the header's artist fallback, so it maps
    /// back onto the key exactly.
    pub fn name(self, projection: &Projection, row: u32) -> GroupName {
        let i = row as usize;
        let sym = |table: &SymTable, sym: u32| table.strings[sym as usize].clone();
        match self {
            GroupKind::Album => GroupName::Album(
                sym(&projection.album_artists, projection.album_artist[i]),
                sym(&projection.albums, projection.album[i]),
            ),
            GroupKind::Artist => {
                GroupName::Name(sym(&projection.album_artists, projection.album_artist[i]))
            }
            GroupKind::Genre => GroupName::Name(sym(&projection.genres, projection.genre[i])),
            GroupKind::Year => GroupName::Name(projection.year[i].to_string()),
        }
    }
}

/// A group as saved: album artist and album for an album group, the one
/// name otherwise. Serializes as `["artist", "album"]` or `"name"`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GroupName {
    Album(String, String),
    Name(String),
}

/// One grouping's saved collapse state. `all` sets what a group does by
/// default, so collapsing everything stays one flag however big the library
/// is, and groups added later follow it.
#[derive(Clone, Default, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CollapseState {
    pub all: bool,
    /// Groups toggled away from `all`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub except: Vec<GroupName>,
}

impl CollapseState {
    pub fn is_default(&self) -> bool {
        !self.all && self.except.is_empty()
    }

    pub fn flip(&mut self, name: GroupName) {
        match self.except.iter().position(|n| *n == name) {
            Some(ix) => {
                self.except.remove(ix);
            }
            None => self.except.push(name),
        }
    }

    /// The saved names mapped onto this build's keys. A name the library no
    /// longer holds maps to nothing and stays saved.
    pub fn keys(&self, kind: GroupKind, projection: &Projection) -> CollapseKeys {
        let fold = projection.fold;
        let norm = |s: &str| {
            if fold {
                crate::fold::fold(s)
            } else {
                s.to_string()
            }
        };
        let mut except = HashSet::new();
        match kind {
            GroupKind::Album => {
                let pairs: Vec<(String, String)> = self
                    .except
                    .iter()
                    .filter_map(|name| match name {
                        GroupName::Album(artist, album) => Some((norm(artist), norm(album))),
                        GroupName::Name(_) => None,
                    })
                    .collect();
                let artists = symbols(
                    &projection.album_artists,
                    fold,
                    pairs.iter().map(|(artist, _)| artist.as_str()),
                );
                let albums = symbols(
                    &projection.albums,
                    fold,
                    pairs.iter().map(|(_, album)| album.as_str()),
                );
                for (artist, album) in &pairs {
                    let (Some(artist_syms), Some(album_syms)) =
                        (artists.get(artist.as_str()), albums.get(album.as_str()))
                    else {
                        continue;
                    };
                    for &a in artist_syms {
                        except.extend(album_syms.iter().map(|&b| (a as u64) << 32 | b as u64));
                    }
                }
            }
            GroupKind::Artist | GroupKind::Genre => {
                let names: Vec<String> = self
                    .except
                    .iter()
                    .filter_map(|name| match name {
                        GroupName::Name(name) => Some(norm(name)),
                        GroupName::Album(..) => None,
                    })
                    .collect();
                let table = match kind {
                    GroupKind::Artist => &projection.album_artists,
                    _ => &projection.genres,
                };
                let found = symbols(table, fold, names.iter().map(String::as_str));
                except.extend(found.values().flatten().map(|&sym| sym as u64));
            }
            GroupKind::Year => {
                except.extend(self.except.iter().filter_map(|name| match name {
                    GroupName::Name(year) => year.parse::<u16>().ok().map(u64::from),
                    GroupName::Album(..) => None,
                }));
            }
        }
        CollapseKeys {
            all: self.all,
            except,
        }
    }
}

/// Every symbol spelled like one of `names`, by name. Compared folded when
/// the projection folds: the table's display string is then a weighted pick
/// among the case variants, which the next rebuild can change.
fn symbols<'n>(
    table: &SymTable,
    fold: bool,
    names: impl Iterator<Item = &'n str>,
) -> HashMap<&'n str, Vec<u32>> {
    let names: HashSet<&str> = names.collect();
    let mut found: HashMap<&str, Vec<u32>> = HashMap::new();
    if names.is_empty() {
        return found;
    }

    let column = if fold { &table.lower } else { &table.strings };
    for (sym, s) in column.iter().enumerate() {
        if let Some(&name) = names.get(s.as_str()) {
            found.entry(name).or_default().push(sym as u32);
        }
    }
    found
}

/// A [`CollapseState`] resolved against one projection build.
#[derive(Debug, Default)]
pub struct CollapseKeys {
    pub all: bool,
    pub except: HashSet<u64>,
}

impl CollapseKeys {
    pub fn collapsed(&self, key: u64) -> bool {
        self.all != self.except.contains(&key)
    }
}

pub struct ViewSpec<'a> {
    pub query: &'a str,
    pub filter: &'a FilterSet,
    /// Scores by db id and direction. Takes precedence over `sort`.
    pub similar: Option<(&'a HashMap<i64, f32>, bool)>,
    pub sort: Option<(SortKey, bool)>,
    /// None for a flat result.
    pub grouping: Option<Grouping<'a>>,
}

/// One finished view pass.
pub struct View {
    pub rows: Arc<Vec<Row>>,
    pub groups: Vec<Group>,
    /// Every track the rows stand for, in row order, collapsed groups'
    /// members included. Playback runs through this, not the rows.
    pub order: Arc<Vec<u32>>,
    /// Whether the grouping honoured its collapse set, which only the
    /// canonical order does.
    pub collapsible: bool,
}

/// [`view_with_order`] for a caller that only draws the rows.
pub fn view_for(
    projection: &Projection,
    order: Arc<Vec<u32>>,
    spec: &ViewSpec,
) -> (Arc<Vec<Row>>, Vec<Group>) {
    let view = view_with_order(projection, order, spec);
    (view.rows, view.groups)
}

/// The canonical order or search hits, filtered, then sorted. Without a
/// column sort the grouping may pre-sort so its runs are contiguous; with
/// one, headers follow whatever runs the sort leaves adjacent.
pub fn view_with_order(projection: &Projection, order: Arc<Vec<u32>>, spec: &ViewSpec) -> View {
    let base = if spec.query.is_empty() {
        order
    } else {
        Arc::new(projection.search(spec.query))
    };
    // Both passes below walk the whole base, so they split across cores.
    // Rayon keeps collect order; `with_min_len` keeps small views serial.
    let base = match projection.filter_mask(spec.filter) {
        Some(mask) => Arc::new(
            base.par_iter()
                .with_min_len(PAR_CHUNK)
                .copied()
                .filter(|&row| mask[row as usize])
                .collect(),
        ),
        None => base,
    };
    // Unscored rows sink either way: no vector is an unknown, not the least
    // similar.
    if let Some((scores, desc)) = spec.similar {
        // Stable, so equal scores keep canonical order between paints.
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
        return flat(Arc::new(rows));
    }
    match spec.sort {
        Some((key, desc)) => {
            let sorted = Arc::new(projection.sort_view(&base, key, desc));
            match &spec.grouping {
                // The sort is the order, so the pre-sort goes unused. A key
                // can recur in several runs here, so nothing collapses.
                Some(grouping) => {
                    // Loners go bare outside search; a search still names a lone hit's group.
                    let solo_heads = !spec.query.is_empty();
                    let grouping = Grouping {
                        collapse: None,
                        ..*grouping
                    };
                    let (rows, groups) = group_rows(&sorted, projection, &grouping, solo_heads);
                    View {
                        rows: Arc::new(rows),
                        groups,
                        order: sorted,
                        collapsible: false,
                    }
                }
                _ => flat(sorted),
            }
        }
        None => match &spec.grouping {
            Some(grouping) => {
                // Genre and year runs aren't contiguous in the canonical order.
                let base = match grouping.pre_sort {
                    Some(key) => Arc::new(projection.sort_view(&base, key, false)),
                    None => base,
                };
                // A search lists hits, so a folded group would hide what was
                // searched for.
                let collapsible = spec.query.is_empty();
                let grouping = Grouping {
                    collapse: grouping.collapse.filter(|_| collapsible),
                    ..*grouping
                };
                let (rows, groups) = group_rows(&base, projection, &grouping, true);
                View {
                    rows: Arc::new(rows),
                    groups,
                    order: base,
                    collapsible,
                }
            }
            _ => flat(base),
        },
    }
}

fn flat(order: Arc<Vec<u32>>) -> View {
    View {
        rows: Arc::new(
            order
                .par_iter()
                .with_min_len(PAR_CHUNK)
                .copied()
                .map(Row::Track)
                .collect(),
        ),
        groups: Vec::new(),
        order,
        collapsible: false,
    }
}

/// The order with a header block opening each run of equal keys. A key
/// recurring later opens a fresh group. With `discs`, a multi-disc run gets
/// a divider per disc, but only if its discs are in order; disc 0 stays
/// undivided. Cheap enough to run per view swap, never per scroll.
///
/// `solo_heads` says whether a run of one track still opens a block. A
/// collapsed run keeps its header block and stats and drops the rest.
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
        // Find the run's extent first so the first disc gets its divider too.
        let mut j = i + 1;
        while j < order.len() && key(order[j]) == key(order[i]) {
            j += 1;
        }
        let members = i..j;
        let run = &order[i..j];
        i = j;

        if run.len() == 1 && !solo_heads {
            rows.push(Row::Track(run[0]));
            continue;
        }

        let g = groups.len() as u32;
        let run_key = key(run[0]);
        let collapsed = grouping
            .collapse
            .is_some_and(|collapse| collapse.collapsed(run_key));
        groups.push(Group {
            first: run[0],
            tracks: 0,
            key: run_key,
            collapsed,
            members,
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
        let multi_disc = !collapsed
            && grouping.discs
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
            if !collapsed {
                rows.push(Row::Track(row));
            }
        }
    }
    (rows, groups)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TrackRow, store};

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
            remote_url: String::new(),
            remote_live: false,
            title_sort: String::new(),
            artist_sort: String::new(),
            album_artist_sort: String::new(),
            album_sort: String::new(),
            sub: 0,
            cue: None,
            path: path.into(),
            title: path.into(),
            artist: album_artist.into(),
            album_artist: album_artist.into(),
            album: album.into(),
            genre: String::new(),
            comment: String::new(),
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

    /// A radio station added the way the sources page does it.
    fn projection_with_a_station(rows: &[TrackRow]) -> Projection {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(&mut conn, rows).unwrap();
        crate::stations::put(
            &mut conn,
            &[crate::stations::Station {
                url: "http://127.0.0.1:8768/stream".into(),
                name: "Noise FM - EDM Radio".into(),
                genre: "EDM".into(),
            }],
        )
        .unwrap();
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
            collapse: None,
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

        let flat = projection(&[
            track("/m/1.flac", "A", "One", 0, 1, 0, "flac", 900, 44100, 16),
            track("/m/2.flac", "A", "One", 0, 2, 0, "flac", 900, 44100, 16),
        ]);
        let order = flat.sort_canonical();
        let (rows, _) = group_rows(&order, &flat, &grouping(1), true);
        assert!(!rows.iter().any(|r| matches!(r, Row::Disc(_))));

        let p = projection(&[
            track("/m/1.flac", "A", "One", 1, 1, 0, "flac", 900, 44100, 16),
            track("/m/2.flac", "A", "One", 2, 1, 0, "flac", 900, 44100, 16),
        ]);
        let order: Vec<u32> = p.sort_canonical().into_iter().rev().collect();
        let (rows, _) = group_rows(&order, &p, &grouping(1), true);
        assert!(!rows.iter().any(|r| matches!(r, Row::Disc(_))));
    }

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
        assert_eq!(rows.len(), 7);
        match rows.last() {
            Some(&Row::Track(r)) => assert_eq!(p.resolve(r).title, "/m/e.flac"),
            other => panic!("expected the bare loner last, got {other:?}"),
        }
    }

    #[test]
    fn a_searched_view_groups_only_when_requested() {
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
        let (rows, groups) = view_for(&p, order.clone(), &searched);
        assert_eq!(groups.len(), 1);
        assert_eq!(rows.len(), 2);
        assert!(matches!(rows[0], Row::Head(0, 0)));
        assert!(matches!(rows[1], Row::Track(_)));

        let flat = ViewSpec {
            grouping: None,
            ..searched
        };
        let (rows, groups) = view_for(&p, order, &flat);
        assert!(groups.is_empty());
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn a_sorted_search_keeps_a_singleton_header() {
        let p = projection(&[
            track("/m/a.flac", "A", "One", 0, 1, 0, "flac", 900, 44100, 16),
            track("/m/b.flac", "B", "Two", 0, 1, 0, "flac", 900, 44100, 16),
        ]);
        let order = Arc::new(p.sort_canonical());
        let filter = FilterSet::default();
        let spec = ViewSpec {
            query: "a.flac",
            filter: &filter,
            similar: None,
            sort: Some((SortKey::Title, false)),
            grouping: Some(grouping(1)),
        };
        let (rows, groups) = view_for(&p, order, &spec);
        assert_eq!(groups.len(), 1);
        assert_eq!(rows.len(), 2);
        assert!(matches!(rows[0], Row::Head(0, 0)));
        assert!(matches!(rows[1], Row::Track(_)));
    }

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

    #[test]
    fn a_collapsed_run_keeps_its_header_and_drops_its_rows() {
        let p = projection(&[
            track("/m/a1.flac", "A", "One", 1, 1, 1000, "flac", 900, 44100, 16),
            track("/m/a2.flac", "A", "One", 2, 1, 2000, "flac", 700, 44100, 16),
            track("/m/b1.flac", "B", "Two", 0, 1, 3000, "flac", 900, 44100, 16),
        ]);
        let order = p.sort_canonical();
        let mut state = CollapseState::default();
        state.flip(GroupName::Album("A".into(), "One".into()));
        let keys = state.keys(GroupKind::Album, &p);
        let spec = Grouping {
            collapse: Some(&keys),
            ..grouping(2)
        };
        let (rows, groups) = group_rows(&order, &p, &spec, true);

        let b = order[2];
        assert_eq!(
            rows,
            vec![
                Row::Head(0, 0),
                Row::Head(0, 1),
                Row::Head(1, 0),
                Row::Head(1, 1),
                Row::Track(b),
            ],
            "the folded album keeps its block and loses its discs and tracks"
        );

        assert!(groups[0].collapsed);
        assert_eq!(groups[0].members, 0..2);
        assert_eq!(groups[0].tracks, 2);
        assert_eq!(groups[0].total_ms, 3000);
        assert_eq!((groups[0].min_kbps, groups[0].max_kbps), (700, 900));

        assert!(!groups[1].collapsed);
        assert_eq!(groups[1].members, 2..3);
        assert_eq!(&order[groups[1].members.clone()], &[b]);
    }

    #[test]
    fn collapse_all_folds_everything_but_the_exceptions() {
        let p = projection(&[
            track("/m/a.flac", "A", "One", 0, 1, 0, "flac", 900, 44100, 16),
            track("/m/b.flac", "B", "Two", 0, 1, 0, "flac", 900, 44100, 16),
        ]);
        let order = p.sort_canonical();
        let mut state = CollapseState {
            all: true,
            except: Vec::new(),
        };
        state.flip(GroupName::Album("B".into(), "Two".into()));
        let keys = state.keys(GroupKind::Album, &p);
        let spec = Grouping {
            collapse: Some(&keys),
            ..grouping(1)
        };
        let (rows, groups) = group_rows(&order, &p, &spec, true);

        assert_eq!(
            rows,
            vec![Row::Head(0, 0), Row::Head(1, 0), Row::Track(order[1])]
        );
        assert!(groups[0].collapsed);
        assert!(!groups[1].collapsed);

        // Flipping the exception back folds it with the rest.
        state.flip(GroupName::Album("B".into(), "Two".into()));
        assert!(state.except.is_empty());
        assert!(!state.is_default());
    }

    #[test]
    fn saved_names_map_back_onto_this_builds_keys() {
        let mut a = track("/m/a.flac", "A", "One", 0, 1, 0, "flac", 900, 44100, 16);
        a.genre = "Jazz".into();
        a.year = 1999;
        let mut b = track("/m/b.flac", "B", "Two", 0, 1, 0, "flac", 900, 44100, 16);
        b.genre = "Rock".into();
        b.year = 2001;
        let p = projection(&[a, b]);

        for kind in [
            GroupKind::Album,
            GroupKind::Artist,
            GroupKind::Genre,
            GroupKind::Year,
        ] {
            for row in 0..p.len() as u32 {
                let mut state = CollapseState::default();
                state.flip(kind.name(&p, row));
                let keys = state.keys(kind, &p);
                assert_eq!(
                    keys.except,
                    HashSet::from([kind.key(&p, row)]),
                    "{kind:?} row {row}"
                );
            }
        }

        // A name the library lost maps to nothing and doesn't trip the rest.
        let state = CollapseState {
            all: false,
            except: vec![
                GroupName::Album("A".into(), "Gone".into()),
                GroupName::Album("A".into(), "One".into()),
                GroupName::Name("Gone".into()),
            ],
        };
        let one = (0..p.len() as u32)
            .find(|&r| p.resolve(r).album == "One")
            .unwrap();
        assert_eq!(
            state.keys(GroupKind::Album, &p).except,
            HashSet::from([GroupKind::Album.key(&p, one)])
        );
        assert!(state.keys(GroupKind::Artist, &p).except.is_empty());
        assert!(state.keys(GroupKind::Year, &p).except.is_empty());
    }

    #[test]
    fn a_folding_projection_matches_names_case_blind() {
        let air = track(
            "/m/a.flac",
            "Air",
            "Moon Safari",
            0,
            1,
            0,
            "flac",
            900,
            44100,
            16,
        );
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(&mut conn, &[air]).unwrap();
        let p = Projection::load_serial(&conn, true).unwrap();

        let state = CollapseState {
            all: false,
            except: vec![GroupName::Album("AIR".into(), "moon safari".into())],
        };
        assert_eq!(
            state.keys(GroupKind::Album, &p).except,
            HashSet::from([GroupKind::Album.key(&p, 0)])
        );
    }

    #[test]
    fn group_names_save_as_plain_json() {
        let state = CollapseState {
            all: false,
            except: vec![
                GroupName::Album("A".into(), "One".into()),
                GroupName::Name("Jazz".into()),
            ],
        };
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "all": false, "except": [["A", "One"], "Jazz"] })
        );
        let back: CollapseState = serde_json::from_value(json).unwrap();
        assert_eq!(back, state);
        let empty: CollapseState = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(empty.is_default());
    }

    #[test]
    fn only_the_canonical_order_collapses() {
        let p = projection(&[
            track("/m/a1.flac", "A", "One", 0, 1, 0, "flac", 900, 44100, 16),
            track("/m/a2.flac", "A", "One", 0, 2, 0, "flac", 900, 44100, 16),
            track("/m/b1.flac", "B", "Two", 0, 1, 0, "flac", 900, 44100, 16),
        ]);
        let order = Arc::new(p.sort_canonical());
        let filter = FilterSet::default();
        let keys = CollapseKeys {
            all: true,
            except: HashSet::new(),
        };
        let spec = |query: &'static str, sort: Option<(SortKey, bool)>| ViewSpec {
            query,
            filter: &filter,
            similar: None,
            sort,
            grouping: Some(Grouping {
                collapse: Some(&keys),
                ..grouping(1)
            }),
        };

        let view = view_with_order(&p, order.clone(), &spec("", None));
        assert!(view.collapsible);
        assert_eq!(view.rows.len(), 2, "two folded headers");
        assert_eq!(view.order.len(), 3, "the order keeps every track");
        assert!(view.groups.iter().all(|g| g.collapsed));

        for (query, sort) in [("a", None), ("", Some((SortKey::Title, false)))] {
            let view = view_with_order(&p, order.clone(), &spec(query, sort));
            assert!(!view.collapsible, "{query:?} {sort:?}");
            assert!(view.groups.iter().all(|g| !g.collapsed));
            assert_eq!(
                view.rows
                    .iter()
                    .filter(|r| matches!(r, Row::Track(_)))
                    .count(),
                view.order.len()
            );
        }
    }

    /// The station stays out through the browse mask; nothing here names
    /// radio.
    #[test]
    fn a_station_never_reaches_the_track_list() {
        let p = projection_with_a_station(&[
            track("/m/a1.flac", "A", "One", 1, 1, 1000, "flac", 900, 44100, 16),
            track("/m/a2.flac", "A", "One", 1, 2, 2000, "flac", 700, 44100, 16),
        ]);
        assert_eq!(p.len(), 3, "the station is in the projection");

        let empty = FilterSet::default();
        let spec = |query: &'static str| ViewSpec {
            query,
            filter: &empty,
            similar: None,
            sort: None,
            grouping: None,
        };
        let order = Arc::new(p.sort_canonical());
        let (rows, _) = view_for(&p, order.clone(), &spec(""));
        assert_eq!(rows.len(), 2);

        let (rows, groups) = view_for(
            &p,
            order,
            &ViewSpec {
                grouping: Some(grouping(1)),
                ..spec("")
            },
        );
        assert_eq!(groups.len(), 1, "one album, no Unknown beside it");
        assert_eq!(
            rows.iter().filter(|r| matches!(r, Row::Track(_))).count(),
            2
        );

        let (rows, _) = view_for(&p, Arc::new(p.sort_canonical()), &spec("noise"));
        assert!(rows.is_empty());
    }
}
