//! Kanji readings, through Lindera and an external IPADIC.
//!
//! A kanji's reading depends on its word (東京 `toukyou`, 東 alone
//! `higashi`), so Lindera segments the text and hands back each token's
//! katakana reading, which [`crate::kana`] turns into letters. The engine is
//! compiled in; the dictionary is [`crate::dictionary`]'s download. Loading is
//! expensive, so a pass loads once and reuses it.
//!
//! IPADIC is weak at personal names, which is why artists keep MusicBrainz's
//! sort name ahead of this and the tag editor can override it.
//!
//! Readings are written as words (`aki no kaze`), since word boundaries are
//! what the analyzer is paid for. Particles read as spoken (は `wa`), keyed
//! off the part-of-speech tag, and marks that end a word stay welded to it.

use std::borrow::Cow;
use std::path::Path;

use lindera::dictionary::load_dictionary;
use lindera::mode::Mode;
use lindera::segmenter::Segmenter;

use crate::kana;

/// Resolved by name against the dictionary's metadata; a dictionary without
/// the field falls back to the surface.
const READING: &str = "reading";

const PART_OF_SPEECH: &str = "part_of_speech";

const PARTICLE: &str = "助詞";

/// Only for tokens tagged as particles: the は in 母 is not this は.
fn spoken(surface: &str) -> Option<&'static str> {
    Some(match surface {
        "は" | "ハ" => "wa",
        "へ" | "ヘ" => "e",
        "を" | "ヲ" => "o",
        _ => return None,
    })
}

/// No space before it: kana that bind leftwards (which the segmenter splits
/// off unknown katakana), iteration marks, and punctuation.
fn joins_left(surface: &str) -> bool {
    let Some(c) = surface.chars().next() else {
        return true;
    };
    kana::binds_left(c)
        || matches!(c, '々' | 'ヽ' | 'ヾ' | 'ゝ' | 'ゞ')
        || !(kana::is_kana(c) || crate::han::is_han(c))
}

pub struct Japanese {
    segmenter: Segmenter,
}

impl Japanese {
    /// The error means the download is missing or damaged.
    pub fn open() -> Result<Self, String> {
        Self::load(&crate::dictionary::IPADIC.path())
    }

    /// Separate from [`Japanese::open`] so a test can point elsewhere.
    pub fn load(path: &Path) -> Result<Self, String> {
        let uri = path
            .to_str()
            .ok_or_else(|| format!("{} is not valid UTF-8", path.display()))?;
        // A bare path: no embed feature is compiled in.
        let dictionary = load_dictionary(uri).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Japanese {
            segmenter: Segmenter::new(Mode::Normal, dictionary, None),
        })
    }

    /// False when a token has no reading and isn't kana, which throws the
    /// whole answer away: half romaji, half kanji is worse than untouched.
    pub(crate) fn read(&self, text: &str, out: &mut String) -> bool {
        let Ok(tokens) = self.segmenter.segment(Cow::Borrowed(text)) else {
            return false;
        };
        let mut first = true;
        for mut token in tokens {
            // Before `get`, which borrows the token mutably.
            let surface = token.surface.to_string();
            let particle = token
                .get(PART_OF_SPEECH)
                .is_some_and(|class| class == PARTICLE);
            // IPADIC writes `*` for a missing field, as on an unknown word.
            let reading = match token.get(READING) {
                Some(reading) if reading != "*" && !reading.is_empty() => reading.to_string(),
                _ => surface.clone(),
            };
            if !first && !joins_left(&surface) {
                out.push(' ');
            }
            first = false;
            match particle.then(|| spoken(&surface)).flatten() {
                Some(said) => out.push_str(said),
                None => {
                    if !kana::romaji(&reading, out) {
                        return false;
                    }
                }
            }
        }
        true
    }
}
