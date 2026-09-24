//! The multi-genre convention: the genre column is one string holding a
//! "; " list. Native multiples (repeated Vorbis GENRE, null-separated TCON)
//! fold into it on read and unfold on write. Matching splits; display and
//! grouping keep the string whole.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

/// Whether `,` and `/` split alongside `;`. Seeded from the library setting;
/// off for taggers whose slashes name single genres.
static SPLIT_COMPOUNDS: AtomicBool = AtomicBool::new(true);

pub fn set_split_compounds(on: bool) {
    SPLIT_COMPOUNDS.store(on, Ordering::Relaxed);
}

/// The live alias map, folded name -> canonical display. Module state
/// because every genre consumer already routes through here. The app
/// reseeds it after each alias edit, then reloads the projection.
static ALIASES: RwLock<Option<Arc<HashMap<String, String>>>> = RwLock::new(None);

/// An empty map clears it.
pub fn set_aliases(map: HashMap<String, String>) {
    let map = if map.is_empty() {
        None
    } else {
        Some(Arc::new(map))
    };
    *ALIASES.write().expect("alias lock never poisons") = map;
}

pub fn resolve(value: &str) -> String {
    let Some(map) = ALIASES.read().expect("alias lock never poisons").clone() else {
        return value.to_string();
    };
    match map.get(&value.to_lowercase()) {
        Some(target) => target.clone(),
        None => value.to_string(),
    }
}

/// The raw values in one genre string, no alias applied. `&` and `+` never
/// split.
pub fn split(s: &str) -> impl Iterator<Item = &str> {
    let compounds = SPLIT_COMPOUNDS.load(Ordering::Relaxed);
    s.split(move |c: char| c == ';' || (compounds && (c == ',' || c == '/')))
        .map(str::trim)
        .filter(|part| !part.is_empty())
}

/// The inverse of [`split`].
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

pub fn canonical(s: &str) -> String {
    join(std::iter::once(s))
}

/// Raise the first letter of all-lowercase words; a word with any capital
/// ("IDM", "nu-Disco") stays as typed.
pub fn capitalize(s: &str) -> String {
    join_owned(split(s).map(capitalize_value))
}

fn capitalize_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for word in value.split_inclusive([' ', '-']) {
        let (body, sep) = match word.strip_suffix([' ', '-']) {
            Some(body) => (body, &word[body.len()..]),
            None => (word, ""),
        };
        let mut chars = body.chars();
        match chars.next() {
            Some(first) if body.chars().all(|c| !c.is_uppercase()) => {
                out.extend(first.to_uppercase());
                out.push_str(chars.as_str());
            }
            _ => out.push_str(body),
        }
        out.push_str(sep);
    }
    out
}

fn join_owned(values: impl Iterator<Item = String>) -> String {
    let values: Vec<String> = values.collect();
    join(values.iter().map(String::as_str))
}

/// Whether `value` is one of the string's values, both sides through the
/// alias map. An empty `value` matches only an empty list.
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
    fn capitalize_raises_lowercase_words_and_keeps_the_rest() {
        assert_eq!(capitalize("ambient; hip-hop"), "Ambient; Hip-Hop");
        assert_eq!(capitalize("drum & bass"), "Drum & Bass");
        assert_eq!(capitalize("IDM; nu-Disco"), "IDM; Nu-Disco");
        assert_eq!(capitalize("Progressive Trance"), "Progressive Trance");
        assert_eq!(capitalize(" rock ;; pop "), "Rock; Pop");
        assert_eq!(capitalize("rock;pop"), "Rock; Pop");
    }

    #[test]
    fn split_trims_and_drops_empties() {
        let parts: Vec<&str> = split("Rock; Shoegaze").collect();
        assert_eq!(parts, ["Rock", "Shoegaze"]);
        let parts: Vec<&str> = split(" Rock ;; Pop ;").collect();
        assert_eq!(parts, ["Rock", "Pop"]);
        assert_eq!(split("").count(), 0);
        assert_eq!(split(" ; ").count(), 0);
    }

    /// The flag is process-global, so the test restores the default.
    #[test]
    fn compound_separators_follow_the_setting() {
        let parts: Vec<&str> = split("Dubstep, Trap, Grime").collect();
        assert_eq!(parts, ["Dubstep", "Trap", "Grime"]);
        let parts: Vec<&str> = split("Drum & Bass / Neurofunk").collect();
        assert_eq!(parts, ["Drum & Bass", "Neurofunk"]);
        assert_eq!(canonical("Dubstep/Grime"), "Dubstep; Grime");

        set_split_compounds(false);
        let parts: Vec<&str> = split("Dubstep, Trap; Grime").collect();
        assert_eq!(parts, ["Dubstep, Trap", "Grime"], "only ';' splits now");
        assert!(has("Rock/Pop; Jazz", "Rock/Pop", false));
        set_split_compounds(true);

        assert!(has("Rock/Pop; Jazz", "Pop", false));
        assert!(!has("Rock/Pop; Jazz", "Rock/Pop", false));
    }

    #[test]
    fn join_canonicalizes_each_value() {
        assert_eq!(join(["Rock", "Shoegaze"].into_iter()), "Rock; Shoegaze");
        assert_eq!(join(["Rock;Pop", " Jazz "].into_iter()), "Rock; Pop; Jazz");
        assert_eq!(join(std::iter::empty()), "");
    }

    #[test]
    fn canonical_round_trips() {
        assert_eq!(canonical("Rock;;Pop "), "Rock; Pop");
        assert_eq!(canonical("Rock; Pop"), "Rock; Pop");
        assert_eq!(canonical(""), "");
    }

    /// The map is process-global, so the test clears it on its way out.
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
        assert!(has("", "", false));
        assert!(has(" ; ", "", false));
        assert!(!has("Rock", "", false));
        assert!(!has("rock; shoegaze", "Rock", false));
        assert!(has("rock; shoegaze", "Rock", true));
        assert!(!has("progressive rock", "Rock", true));
    }
}
