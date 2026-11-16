//! The live look's file, `workspace.json`, shared between rox and whatever
//! editor a person opens it in (ADR 22). rox records the text of each write
//! so its watch can tell its own saves from a hand edit, and keeps the last
//! text that parsed so a load that lands on a half-saved edit keeps the look
//! instead of resetting it to defaults.
//!
//! Nothing here watches or applies. The watch lives up in rox, with the
//! windows a re-apply has to reach.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::settings::LookState;

/// How long an older write still counts as rox's own. A layout save and a
/// look edit can both land inside one watch debounce, and the watch reads
/// the file once, after both. The newest write counts however old it is.
const OWN_WRITE_WINDOW: Duration = Duration::from_secs(5);

struct Record {
    /// Hashes of the texts rox wrote, newest last.
    written: Vec<(u64, Instant)>,
    /// The newest text that parsed, read or written.
    good: Option<String>,
    /// Whether the last read of the file parsed as it stood.
    parsed: bool,
}

static RECORD: Mutex<Record> = Mutex::new(Record {
    written: Vec::new(),
    good: None,
    parsed: false,
});

fn hash(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

impl Record {
    fn note_written(&mut self, text: &str, now: Instant) {
        self.written
            .retain(|(_, at)| now.duration_since(*at) < OWN_WRITE_WINDOW);
        self.written.push((hash(text), now));
        self.note_good(text);
    }

    fn is_own(&self, text: &str, now: Instant) -> bool {
        let hash = hash(text);
        let newest = self.written.last().is_some_and(|(h, _)| *h == hash);

        newest
            || self
                .written
                .iter()
                .any(|(h, at)| *h == hash && now.duration_since(*at) < OWN_WRITE_WINDOW)
    }

    fn note_good(&mut self, text: &str) {
        if self.good.as_deref() != Some(text) {
            self.good = Some(text.to_string());
        }
    }

    fn read(&mut self, text: &str, path: &Path) -> LookState {
        let e = match serde_json::from_str(text) {
            Ok(look) => {
                self.parsed = true;
                self.note_good(text);
                return look;
            }
            Err(e) => e,
        };
        self.parsed = false;

        match self.good.as_deref().map(serde_json::from_str::<LookState>) {
            Some(Ok(look)) => {
                log::warn!(
                    "look: {} doesn't parse, keeping the last look that did: {e}",
                    path.display()
                );
                look
            }
            _ => {
                log::warn!("look: resetting {}: {e}", path.display());
                LookState::default()
            }
        }
    }
}

/// Called before the write lands, so the watch can never read the new text
/// ahead of the record.
pub(crate) fn note_written(text: &str) {
    RECORD.lock().unwrap().note_written(text, Instant::now());
}

/// Whether the look last loaded is the file's own: it was there and parsed.
/// A look from defaults, from the last good text or from a pre-split file
/// isn't, and its asset table doesn't speak for the folder.
pub fn loaded_from_file() -> bool {
    RECORD.lock().unwrap().parsed
}

/// Whether `text` is what rox itself last put in the file.
pub fn is_own_write(text: &str) -> bool {
    RECORD.lock().unwrap().is_own(text, Instant::now())
}

/// Once a hand edit is in, an older write rox made isn't the file's state
/// any more. Undoing the edit back to that text has to count as an edit.
pub fn forget_own_writes() {
    RECORD.lock().unwrap().written.clear();
}

/// The look shard's read. A file that doesn't parse falls back to the last
/// one that did, since a write built on a defaulted look would save the
/// defaults over everything the person had.
pub(crate) fn load(path: &Path, legacy: &serde_json::Value) -> LookState {
    match std::fs::read_to_string(path) {
        Ok(text) => RECORD.lock().unwrap().read(&text, path),
        Err(_) => {
            RECORD.lock().unwrap().parsed = false;
            LookState::from_legacy(legacy)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> Record {
        Record {
            written: Vec::new(),
            good: None,
            parsed: false,
        }
    }

    #[test]
    fn its_own_writes_are_recognized_and_an_edit_is_not() {
        let mut record = record();
        let now = Instant::now();
        record.note_written(r#"{"active_layout":"A"}"#, now);
        record.note_written(r#"{"active_layout":"B"}"#, now);

        assert!(record.is_own(r#"{"active_layout":"A"}"#, now));
        assert!(record.is_own(r#"{"active_layout":"B"}"#, now));
        assert!(!record.is_own(r#"{"active_layout":"C"}"#, now));
    }

    #[test]
    fn an_older_write_expires_and_the_newest_never_does() {
        let mut record = record();
        let then = Instant::now();
        record.note_written("old", then);
        record.note_written("new", then);

        let later = then + OWN_WRITE_WINDOW * 2;
        assert!(!record.is_own("old", later));
        assert!(record.is_own("new", later));
    }

    #[test]
    fn a_broken_file_keeps_the_last_look_that_parsed() {
        let mut record = record();
        let path = Path::new("workspace.json");
        let good = r#"{"bundle":{"name":"Critters"},"active_layout":"Main"}"#;

        let first = record.read(good, path);
        assert_eq!(first.bundle.name, "Critters");

        let broken = record.read(r#"{"bundle":{"name":"Crit"#, path);
        assert_eq!(broken.bundle.name, "Critters");
        assert_eq!(broken.active_layout.as_deref(), Some("Main"));
    }

    #[test]
    fn only_a_file_that_parsed_counts_as_loaded_from_it() {
        let mut record = record();
        let path = Path::new("workspace.json");

        record.read(r#"{"bundle":{"name":"Critters"}}"#, path);
        assert!(record.parsed);

        // The last good look stands in, but its table isn't the file's.
        record.read("{", path);
        assert!(!record.parsed);
    }

    #[test]
    fn a_broken_file_with_nothing_before_it_resets() {
        let mut record = record();
        let look = record.read("{", Path::new("workspace.json"));

        assert!(look.bundle.name.is_empty());
        assert!(record.good.is_none());
    }
}
