//! The multi-genre convention, in one place. A track's genre column is a
//! single display string, but the string is a list: values joined with
//! "; ", the separator foobar2000 and Picard taught everyone's files.
//! Formats that carry real multiples (repeated GENRE comments on Vorbis,
//! null-separated TCON on ID3v2.4) fold into this form at scan and read,
//! and unfold from it at write. Matching splits; display and grouping
//! keep the joined string whole.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// The live alias map off the library's genre_meta table, folded name ->
/// canonical display. Module state rather than a parameter because every
/// consumer of genre values already routes through this module - the
/// projection's matching, the listens rollups, the panels - and each
/// would otherwise thread the map through call chains that never look
/// inside it. The app seeds it after opening the library and after every
/// alias edit, then reloads the projection, the case-fold toggle's move.
static ALIASES: RwLock<Option<Arc<HashMap<String, String>>>> = RwLock::new(None);

/// Install the alias map, [`crate::genre_meta::aliases`]'s output; an
/// empty map clears it.
pub fn set_aliases(map: HashMap<String, String>) {
    let map = if map.is_empty() {
        None
    } else {
        Some(Arc::new(map))
    };
    *ALIASES.write().expect("alias lock never poisons") = map;
}

/// A value through the alias map: the canonical display it folds into,
/// or itself untouched. The common no-alias library never allocates.
pub fn resolve(value: &str) -> String {
    let Some(map) = ALIASES.read().expect("alias lock never poisons").clone() else {
        return value.to_string();
    };
    match map.get(&value.to_lowercase()) {
        Some(target) => target.clone(),
        None => value.to_string(),
    }
}

/// The values inside one genre string: split on ';', trimmed, empties
/// dropped. A plain single genre comes back as itself. Raw values, no
/// alias applied: callers building display surfaces run each part
/// through [`resolve`]; [`has`] resolves internally.
pub fn split(s: &str) -> impl Iterator<Item = &str> {
    s.split(';').map(str::trim).filter(|part| !part.is_empty())
}

/// One display string from many values: joined with "; ", each value
/// trimmed, empties dropped. The inverse of [`split`].
pub fn join<'a>(values: impl Iterator<Item = &'a str>) -> String {
    let mut out = String::new();
    for value in values.flat_map(split) {
        if !out.is_empty() {
            out.push_str("; ");
        }
        out.push_str(value);
    }
    out
}

/// The string's canonical form: split and rejoined, so "Rock;;Pop " and
/// "Rock; Pop" read the same.
pub fn canonical(s: &str) -> String {
    join(std::iter::once(s))
}

/// Whether the genre string carries `value` as one of its values, exact
/// or case-folded per the library's `fold` rule, both sides read through
/// the alias map so a pick on the merged name takes the folded-away tags
/// too. An empty `value` is the untagged pick and matches only a string
/// with no values at all.
pub fn has(s: &str, value: &str, fold: bool) -> bool {
    if value.is_empty() {
        return split(s).next().is_none();
    }
    let value = resolve(value);
    split(s).any(|part| crate::value_eq(&resolve(part), &value, fold))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_trims_and_drops_empties() {
        let parts: Vec<&str> = split("Rock; Shoegaze").collect();
        assert_eq!(parts, ["Rock", "Shoegaze"]);
        let parts: Vec<&str> = split(" Rock ;; Pop ;").collect();
        assert_eq!(parts, ["Rock", "Pop"]);
        assert_eq!(split("").count(), 0);
        assert_eq!(split(" ; ").count(), 0);
    }

    #[test]
    fn join_canonicalizes_each_value() {
        assert_eq!(join(["Rock", "Shoegaze"].into_iter()), "Rock; Shoegaze");
        // A value that is itself a list folds flat, so joining tag items
        // that already carry the separator cannot nest.
        assert_eq!(join(["Rock;Pop", " Jazz "].into_iter()), "Rock; Pop; Jazz");
        assert_eq!(join(std::iter::empty()), "");
    }

    #[test]
    fn canonical_round_trips() {
        assert_eq!(canonical("Rock;;Pop "), "Rock; Pop");
        assert_eq!(canonical("Rock; Pop"), "Rock; Pop");
        assert_eq!(canonical(""), "");
    }

    /// Aliases route both sides of a match and resolve to the canonical
    /// display. The map is process-global, so the keys collide with no
    /// other test's values and the test clears it on its way out.
    #[test]
    fn aliases_route_matching_and_resolution() {
        set_aliases(HashMap::from([(
            "dnb-test".to_string(),
            "Drum & Bass Test".to_string(),
        )]));
        assert_eq!(resolve("DNB-Test"), "Drum & Bass Test");
        assert_eq!(resolve("House"), "House");
        assert!(has("Rock; dnb-test", "Drum & Bass Test", false));
        assert!(has("Rock; DNB-TEST", "drum & bass test", true));
        assert!(!has("Rock", "Drum & Bass Test", false));
        set_aliases(HashMap::new());
        assert_eq!(resolve("dnb-test"), "dnb-test");
    }

    #[test]
    fn has_matches_whole_values_only() {
        assert!(has("Rock; Shoegaze", "Rock", false));
        assert!(has("Rock; Shoegaze", "Shoegaze", false));
        assert!(!has("Rock; Shoegaze", "Rock; Shoegaze", false));
        assert!(!has("Progressive Rock", "Rock", false));
        // The empty pick is the untagged bucket.
        assert!(has("", "", false));
        assert!(has(" ; ", "", false));
        assert!(!has("Rock", "", false));
        // Folding matches across casings, still whole values only.
        assert!(!has("rock; shoegaze", "Rock", false));
        assert!(has("rock; shoegaze", "Rock", true));
        assert!(!has("progressive rock", "Rock", true));
    }
}
