//! Putting what rox already knows about a track into the track's own file.
//!
//! Three settings decide where new metadata lands: [`crate::lyrics`] saves a
//! fetched sheet to the store, a sidecar or the tag; the ReplayGain pass
//! writes its numbers to the database or the tags; the acoustic pass does the
//! same with its vectors. All three only ever speak for the next write. Turn
//! one to Tags after a library is already described and nothing moves, and a
//! folder handed to another player carries none of it.
//!
//! This is the catch-up. Nothing here computes anything: every value it
//! writes is one the app is already holding, and a file it can't reach keeps
//! its database row exactly as it was.
//!
//! ## The two halves
//!
//! [`candidates`] is the database and a few stats: what rox holds, and which
//! of it has a file that could take a tag at all. [`examine`] is the
//! expensive half, one tag read per candidate, and it's separate precisely so
//! the caller can run it across a pool - see `rox/src/bake.rs`, which does.
//! Skipping it leaves every candidate looking writable, which is only wrong
//! in the direction of rewriting a file that already agreed with us.
//!
//! ## What gets refused
//!
//! [`crate::writer`] handles MP3 and FLAC, so every other format keeps its
//! row and nothing more. A cue subsong is refused for the reason
//! [`crate::writer::writes_to_file`] gives: twelve tracks share one image, and
//! writing one track's lyrics into it would caption the whole disc.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::embed_tag;
use crate::lyrics;
use crate::replaygain::ReplayGain;
use crate::writer::{self, Change, Field};

/// Where one pending write comes from, which is also how the dialog groups
/// its checkboxes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// A sheet in the app's own store or in a sidecar beside the track.
    Lyrics,
    /// Four numbers the measurement pass put in the database.
    Gain,
    /// One model's vector out of the embeddings table.
    Acoustic,
}

impl Source {
    /// Every source, in the order the dialog lists them.
    pub const ALL: [Source; 3] = [Source::Lyrics, Source::Gain, Source::Acoustic];

    pub fn label(self) -> &'static str {
        match self {
            Source::Lyrics => "Lyrics",
            Source::Gain => "ReplayGain",
            Source::Acoustic => "Acoustic descriptions",
        }
    }
}

/// Why a file rox has something for gets nothing written to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skip {
    /// A cue track: a span inside an image the whole disc shares, so there
    /// is no file that means this track alone.
    Subsong,
    /// Not one of the two formats the writer handles.
    Format,
    /// The file's tag already carries this, so writing it would rewrite the
    /// file to leave it as it was.
    Present,
}

/// One value rox is holding, ready to go into a file.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// The sheet as its home holds it, newlines and timestamps and all.
    Lyrics(String),
    /// Whatever of the four the database has. Missing numbers stay missing:
    /// see [`writer::replay_gain_additions`].
    Gain(ReplayGain),
    /// One model's vector, under that model's own key.
    Acoustic { model: String, vec: Vec<f32> },
}

impl Value {
    fn source(&self) -> Source {
        match self {
            Value::Lyrics(_) => Source::Lyrics,
            Value::Gain(_) => Source::Gain,
            Value::Acoustic { .. } => Source::Acoustic,
        }
    }
}

/// One file and one thing rox could put in it.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub path: PathBuf,
    pub value: Value,
    /// None while this still looks writable. [`candidates`] fills in the two
    /// cheap refusals; [`examine`] fills in the third.
    pub skip: Option<Skip>,
}

impl Candidate {
    pub fn source(&self) -> Source {
        self.value.source()
    }

    /// The tag writes this candidate is, empty for one that's been refused.
    pub fn changes(&self) -> Vec<Change> {
        if self.skip.is_some() {
            return Vec::new();
        }
        match &self.value {
            Value::Lyrics(text) => vec![Change {
                field: Field::Lyrics,
                value: Some(text.clone()),
            }],
            Value::Gain(gain) => writer::replay_gain_additions(*gain),
            Value::Acoustic { model, vec } => vec![Change {
                field: Field::Custom(embed_tag::key(model)),
                value: Some(embed_tag::encode(vec)),
            }],
        }
    }
}

/// One source's tally, the pair each checkbox states.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Files this source would actually write to.
    pub writes: usize,
    /// Files it holds something for and won't be writing to.
    pub skipped: usize,
}

/// What one source would do, over a surveyed list.
pub fn counts(candidates: &[Candidate], source: Source) -> Counts {
    candidates
        .iter()
        .filter(|c| c.source() == source)
        .fold(Counts::default(), |counts, c| match c.skip {
            Some(_) => Counts {
                skipped: counts.skipped + 1,
                ..counts
            },
            None => Counts {
                writes: counts.writes + 1,
                ..counts
            },
        })
}

/// One file's whole write: everything the picked sources have for it, in one
/// commit rather than one each. A file carrying lyrics, a gain and a vector
/// is rewritten once.
#[derive(Clone, Debug)]
pub struct Item {
    pub path: PathBuf,
    pub changes: Vec<Change>,
}

/// Fold a surveyed list down to one item per file, keeping only the sources
/// that were picked and only the candidates nothing refused.
pub fn merge(candidates: &[Candidate], sources: &[Source]) -> Vec<Item> {
    let mut items: Vec<Item> = Vec::new();
    let mut at: HashMap<&Path, usize> = HashMap::new();
    for candidate in candidates {
        if candidate.skip.is_some() || !sources.contains(&candidate.source()) {
            continue;
        }
        let index = *at.entry(candidate.path.as_path()).or_insert_with(|| {
            items.push(Item {
                path: candidate.path.clone(),
                changes: Vec::new(),
            });
            items.len() - 1
        });
        items[index].changes.extend(candidate.changes());
    }
    items
}

/// Write one file. Rides [`writer::commit`], so the atomic layer applies -
/// clone, verify, rename - and nothing but these fields moves.
pub fn apply(item: &Item) -> Result<(), String> {
    if item.changes.is_empty() {
        return Ok(());
    }
    writer::commit(&item.path, &item.changes)
}

/// Everything rox is holding that a file could carry, with the refusals it
/// can work out without opening anything.
///
/// `model` is the acoustic model whose vectors to offer, `lyrics_dir` the
/// app's own lyrics store. Every candidate that comes back with `skip` unset
/// still owes a [`examine`] before its count means anything.
pub fn candidates(
    conn: &Connection,
    model: &str,
    lyrics_dir: Option<&Path>,
) -> rusqlite::Result<Vec<Candidate>> {
    let mut out = Vec::new();
    for (path, sub) in crate::store::local_paths(conn)? {
        let file = PathBuf::from(&path);
        // The tag read this would otherwise cost on every untouched file in
        // the library is the whole reason for the stat first: load's own
        // order ends at the embedded tag, and a sheet that's already there
        // is not one this tool moves.
        if !has_stored_sheet(&file, lyrics_dir) {
            continue;
        }
        let Some(sheet) = lyrics::load(&file, lyrics_dir) else {
            continue;
        };
        if sheet.source == lyrics::Source::Tag {
            continue;
        }
        out.push(Candidate {
            skip: refusal(&file, sub),
            path: file,
            value: Value::Lyrics(sheet.text),
        });
    }
    for (path, sub, gain) in crate::store::measured_replaygain(conn)? {
        let file = PathBuf::from(&path);
        out.push(Candidate {
            skip: refusal(&file, sub),
            path: file,
            value: Value::Gain(gain),
        });
    }
    for (path, sub, vec) in crate::embeddings::embedded(conn, model)? {
        let file = PathBuf::from(&path);
        out.push(Candidate {
            skip: refusal(&file, sub),
            path: file,
            value: Value::Acoustic {
                model: model.to_owned(),
                vec,
            },
        });
    }
    Ok(out)
}

/// Look in the file and refuse a candidate whose tag already says this.
///
/// One file open, which is why this is its own call rather than part of
/// [`candidates`]: over a described library it's the whole cost of a survey,
/// and it parallelizes cleanly because every candidate is independent.
/// A candidate already refused is left alone.
pub fn examine(candidate: &mut Candidate) {
    if candidate.skip.is_some() {
        return;
    }
    let present = match &candidate.value {
        // Same words already in the frame. Compared trimmed, since a
        // trailing newline is the difference between two homes for the same
        // sheet rather than between two sheets.
        Value::Lyrics(text) => {
            lyrics::tag_lyrics(&candidate.path).is_some_and(|tag| tag.trim() == text.trim())
        }
        // Nothing to look at: a measured row exists because the file's tags
        // carried no gain when it was scanned, so the database is the only
        // copy by definition.
        Value::Gain(_) => false,
        // Any readable vector under this model's key is the one this would
        // write: the value carries the model and the width and is refused
        // unless both match, so a hit can't be another model's.
        Value::Acoustic { model, vec } => {
            embed_tag::read(&candidate.path, model, vec.len()).is_some()
        }
    };
    if present {
        candidate.skip = Some(Skip::Present);
    }
}

/// The two refusals a path and a subsong number are enough to make.
fn refusal(path: &Path, sub: u16) -> Option<Skip> {
    if !writer::writes_to_file(sub) {
        return Some(Skip::Subsong);
    }
    (!embed_tag::writable(path)).then_some(Skip::Format)
}

/// Whether a sheet lives anywhere but the file's own tag. Stats only: this
/// is the gate that keeps a survey off the tags of a library nobody has
/// fetched lyrics for.
fn has_stored_sheet(path: &Path, lyrics_dir: Option<&Path>) -> bool {
    lyrics::sidecar_candidates(path)
        .iter()
        .any(|side| side.is_file())
        || lyrics_dir.is_some_and(|dir| lyrics::store_file(dir, path).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::writer::{flac_file, mp3_file, scratch};

    /// A library with one row per path, everything local and everything a
    /// plain file unless a sub is given.
    fn library(rows: &[(&Path, u16)]) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::store::init_schema(&conn).unwrap();
        for (path, sub) in rows {
            conn.execute(
                "INSERT INTO tracks (path, sub, title, artist, album, genre, year, track_no,
                    duration_ms, size, mtime)
                 VALUES (?1, ?2, 'T', 'A', 'Al', 'g', 0, 1, 200000, 0, 0)",
                rusqlite::params![path.display().to_string(), *sub as i64],
            )
            .unwrap();
        }
        conn
    }

    /// The whole tool over one file: a gain the pass measured into the
    /// database and a sheet the app keeps in its own store, both of them
    /// somewhere no other player can see, come out of one commit as tags any
    /// player reads.
    #[test]
    fn a_measured_gain_and_a_stored_sheet_land_in_the_file() {
        let dir = scratch("bake-round-trip");
        let store = dir.join("lyrics");
        let path = mp3_file(&dir, "track.mp3");
        let sheet = "[00:12.00] the first line\nthe second";
        std::fs::create_dir_all(&store).unwrap();
        std::fs::write(lyrics::store_file(&store, &path), sheet).unwrap();

        let mut conn = library(&[(&path, 0)]);
        let gain = ReplayGain {
            track_db: Some(-7.35),
            track_peak: Some(0.987654),
            album_db: Some(-8.10),
            album_peak: None,
        };
        crate::store::set_measured_replaygain(
            &mut conn,
            &[(path.display().to_string().as_str(), gain)],
        )
        .unwrap();

        let mut found = candidates(&conn, "builtin-v1", Some(&store)).unwrap();
        assert_eq!(found.len(), 2, "a sheet and a gain, {found:?}");
        for candidate in &mut found {
            examine(candidate);
            assert_eq!(candidate.skip, None, "{candidate:?}");
        }
        assert_eq!(counts(&found, Source::Lyrics).writes, 1);
        assert_eq!(counts(&found, Source::Gain).writes, 1);

        // Both sources, one file, one rewrite.
        let items = merge(&found, &Source::ALL);
        assert_eq!(items.len(), 1);
        apply(&items[0]).unwrap();

        let read = crate::scanner::read_one(&path).unwrap().replay_gain;
        assert_eq!(read.track_db, Some(-7.35));
        assert_eq!(read.track_peak, Some(0.987654));
        assert_eq!(read.album_db, Some(-8.10));
        // Never written, so never invented: the file carries the three
        // numbers the database had and nothing in the fourth slot.
        assert_eq!(read.album_peak, None);
        let tagged = writer::read(&path)
            .unwrap()
            .into_iter()
            .find(|(field, _)| *field == Field::Lyrics)
            .map(|(_, text)| text);
        assert_eq!(tagged.as_deref(), Some(sheet));

        // And a second run has nothing left to do: the sheet is now in the
        // tag as well, and the gain will read back off it after a rescan.
        let mut again = candidates(&conn, "builtin-v1", Some(&store)).unwrap();
        for candidate in &mut again {
            examine(candidate);
        }
        assert_eq!(counts(&again, Source::Lyrics).writes, 0);
        assert_eq!(counts(&again, Source::Lyrics).skipped, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The two things this tool refuses to touch. A gain that came off the
    /// file's own tags is already where it belongs, so it never becomes a
    /// candidate at all; a format the writer can't open is one it says so
    /// about, since a count that hid it would leave someone wondering which
    /// files went missing.
    #[test]
    fn a_tag_sourced_gain_and_an_unwritable_file_are_left_alone() {
        let dir = scratch("bake-refusals");
        let mp3 = mp3_file(&dir, "measured.mp3");
        let ogg = dir.join("measured.ogg");
        std::fs::write(&ogg, b"not a file the writer opens").unwrap();
        let tagged = mp3_file(&dir, "tagged.mp3");

        let mut conn = library(&[(&mp3, 0), (&ogg, 0), (&tagged, 0)]);
        let gain = ReplayGain {
            track_db: Some(-6.5),
            track_peak: Some(0.9),
            ..Default::default()
        };
        crate::store::set_measured_replaygain(
            &mut conn,
            &[
                (mp3.display().to_string().as_str(), gain),
                (ogg.display().to_string().as_str(), gain),
            ],
        )
        .unwrap();
        // The third row's numbers came off its own tags, which is the
        // default source and what every row written before the measurement
        // pass existed reads as.
        conn.execute(
            "UPDATE tracks SET rg_track_gain = -4.0, rg_source = 0 WHERE path = ?1",
            rusqlite::params![tagged.display().to_string()],
        )
        .unwrap();

        let found = candidates(&conn, "builtin-v1", None).unwrap();
        assert_eq!(
            counts(&found, Source::Gain),
            Counts {
                writes: 1,
                skipped: 1
            },
            "{found:?}"
        );
        assert!(
            !found.iter().any(|c| c.path == tagged),
            "a tag-sourced gain is already in the file it came from"
        );
        let refused = found.iter().find(|c| c.path == ogg).unwrap();
        assert_eq!(refused.skip, Some(Skip::Format));
        assert!(refused.changes().is_empty(), "a refusal writes nothing");
        // And the picked set is the one writable file.
        assert_eq!(merge(&found, &Source::ALL).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A vector the file is already carrying, and a cue track that has no
    /// file to carry one. Both keep their database row and cost nothing.
    #[test]
    fn a_vector_already_in_the_file_and_a_cue_track_are_skipped() {
        let dir = scratch("bake-vectors");
        let fresh = flac_file(&dir, "fresh.flac");
        let already = flac_file(&dir, "already.flac");
        let image = flac_file(&dir, "disc.flac");
        let vec: Vec<f32> = (0..16).map(|i| i as f32 * 0.25 - 2.0).collect();
        writer::commit_embedding(&already, "builtin-v1", &vec).unwrap();

        let conn = library(&[(&fresh, 0), (&already, 0), (&image, 1)]);
        for path in [&fresh, &already, &image] {
            let id: i64 = conn
                .query_row(
                    "SELECT id FROM tracks WHERE path = ?1",
                    rusqlite::params![path.display().to_string()],
                    |r| r.get(0),
                )
                .unwrap();
            crate::embeddings::upsert(&conn, id, "builtin-v1", &vec).unwrap();
        }

        let mut found = candidates(&conn, "builtin-v1", None).unwrap();
        assert_eq!(found.len(), 3);
        // The cue track is refused before anything is opened; the other two
        // need the file read to tell apart.
        assert_eq!(
            found.iter().find(|c| c.path == image).unwrap().skip,
            Some(Skip::Subsong)
        );
        for candidate in &mut found {
            examine(candidate);
        }
        assert_eq!(
            found.iter().find(|c| c.path == already).unwrap().skip,
            Some(Skip::Present)
        );
        assert_eq!(
            counts(&found, Source::Acoustic),
            Counts {
                writes: 1,
                skipped: 2
            }
        );

        let items = merge(&found, &[Source::Acoustic]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].path, fresh);
        apply(&items[0]).unwrap();
        assert!(embed_tag::read(&fresh, "builtin-v1", vec.len()).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Picking one source writes that source alone, which is what the
    /// dialog's checkboxes are.
    #[test]
    fn an_unpicked_source_writes_nothing() {
        let lyrics = Candidate {
            path: PathBuf::from("/m/a.mp3"),
            value: Value::Lyrics("words".into()),
            skip: None,
        };
        let gain = Candidate {
            path: PathBuf::from("/m/a.mp3"),
            value: Value::Gain(ReplayGain {
                track_db: Some(-6.0),
                ..Default::default()
            }),
            skip: None,
        };
        let both = merge(&[lyrics.clone(), gain.clone()], &Source::ALL);
        assert_eq!(both.len(), 1, "one file, one commit");
        assert_eq!(both[0].changes.len(), 2);
        let only_gain = merge(&[lyrics, gain], &[Source::Gain]);
        assert_eq!(only_gain[0].changes.len(), 1);
    }
}
