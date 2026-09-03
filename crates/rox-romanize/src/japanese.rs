//! Kanji readings, through Lindera and an external IPADIC.
//!
//! Japanese is the one script here that can't be done with a table.
//! 東京 is `toukyou` and 東海 is `toukai` but 東 alone is `higashi`, and
//! which reading a character takes depends on the word it's in, so
//! something has to find the word boundaries first. Lindera is a
//! morphological analyzer: it segments the text against a dictionary and
//! hands back, per token, the reading IPADIC records for it in katakana.
//! [`crate::kana`] turns that katakana into letters, which is the same
//! table that handles kana in the source text, so there's one Hepburn
//! implementation rather than two.
//!
//! The engine is compiled in and the dictionary isn't. Lindera's
//! `embed-*` features bake a dictionary into the binary and none of them
//! are enabled; what's linked is the Viterbi lattice and the loader, and
//! the data arrives as [`crate::dictionary`]'s download. That split is the
//! whole reason this crate exists as a separate thing.
//!
//! Loading is not cheap and the result is immutable, so a caller loads one
//! of these and hands it to every call. The pass does exactly that: one
//! load, then tens of thousands of titles through the same segmenter.
//!
//! ## What it gets wrong
//!
//! Personal names, mostly, which IPADIC is famously weak at: 人名 readings
//! are the classic failure and a name it doesn't hold gets segmented into
//! pieces and read by their common readings instead. That's why artists
//! keep MusicBrainz's sort name ahead of anything romanized, and why the
//! tag editor overrides any of this by hand. For a title there's no better
//! source, and a wrong romaji is still findable by the wrong romaji, which
//! beats a title nobody can type at all.
//!
//! ## Words, not one long string
//!
//! The segmenter's tokens are words, so the reading is written as words:
//! 秋ノ風 comes back `aki no kaze` rather than `akinokaze`, which is what
//! Google Translate prints and what somebody would type. The whole point
//! of paying for a morphological analyzer is that it knows where the
//! boundaries are, and throwing that away at the last step was leaving the
//! useful half of the answer on the floor.
//!
//! Two things get read the way they're said rather than the way they're
//! written. Particles: は is `wa`, へ is `e`, を is `o`, keyed off IPADIC's
//! part-of-speech tag rather than off the character, because は inside a
//! word is still `ha`. And the marks a word can be split at (a long-vowel
//! mark, a small tsu, a small vowel) stay welded to the token in front of
//! them, since a space there would cut a word in half.

use std::borrow::Cow;
use std::path::Path;

use lindera::dictionary::load_dictionary;
use lindera::mode::Mode;
use lindera::segmenter::Segmenter;

use crate::kana;

/// The index of `reading` in IPADIC's schema is not hardcoded here;
/// [`lindera::token::Token::get`] resolves the name against the metadata
/// the dictionary shipped with. IPADIC calls the field this, and a
/// dictionary that calls it something else answers None and falls back to
/// the surface.
const READING: &str = "reading";

/// The part-of-speech field, resolved by name the same way [`READING`] is.
/// Only one question is asked of it: whether the token is a particle.
const PART_OF_SPEECH: &str = "part_of_speech";

/// IPADIC's tag for a particle, the one word class whose spelling and its
/// reading come apart.
const PARTICLE: &str = "助詞";

/// What a particle is said as, when that isn't what it's written as. Three
/// of them, all inherited from a spelling reform that left the grammar
/// alone: は marking a topic is said `wa`, へ marking a direction is `e`,
/// and を is `o`.
///
/// Only reached for a token IPADIC tagged as a particle, which is the
/// whole reason the tag is read at all: the は in 母 is not this は, and a
/// rule keyed off the character alone would break every word carrying one.
fn spoken(surface: &str) -> Option<&'static str> {
    Some(match surface {
        "は" | "ハ" => "wa",
        "へ" | "ヘ" => "e",
        "を" | "ヲ" => "o",
        _ => return None,
    })
}

/// Whether a token hangs off the word in front of it rather than starting
/// one of its own, so no space goes between them.
///
/// Two kinds. The kana that bind leftwards by definition (a long-vowel
/// mark, a small tsu, a small vowel) are a word's tail even when the
/// segmenter hands them over separately, which it does for a katakana
/// title it doesn't hold. The iteration marks repeat what came before
/// them, so they're the same case written differently. Anything else that
/// isn't kana or Han is punctuation, and a space in front of a comma is
/// nobody's idea of a word boundary.
fn joins_left(surface: &str) -> bool {
    let Some(c) = surface.chars().next() else {
        return true;
    };
    kana::binds_left(c)
        || matches!(c, '々' | 'ヽ' | 'ヾ' | 'ゝ' | 'ゞ')
        || !(kana::is_kana(c) || crate::han::is_han(c))
}

/// A loaded dictionary and the segmenter over it. Immutable once built,
/// and shared by reference across every call in a pass.
pub struct Japanese {
    segmenter: Segmenter,
}

impl Japanese {
    /// Load the dictionary from the models directory. The error is worth
    /// showing a person: it means the download is missing or damaged.
    pub fn open() -> Result<Self, String> {
        Self::load(&crate::dictionary::IPADIC.path())
    }

    /// Load a dictionary directory by path, which is what
    /// [`Japanese::open`] does with the installed one. Separate so a test
    /// can point at a dictionary somewhere else.
    pub fn load(path: &Path) -> Result<Self, String> {
        let uri = path
            .to_str()
            .ok_or_else(|| format!("{} is not valid UTF-8", path.display()))?;
        // A bare filesystem path, not an `embedded://` URI: no embed
        // feature is enabled, so the embedded loader isn't even compiled
        // in and this is the only branch that can succeed.
        let dictionary = load_dictionary(uri).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Japanese {
            segmenter: Segmenter::new(Mode::Normal, dictionary, None),
        })
    }

    /// Append `text`'s reading to `out`. False when a token has no reading
    /// and isn't kana to fall back on, which throws the whole answer away:
    /// a title half in romaji and half in kanji is worse than an untouched
    /// one.
    ///
    /// One space between tokens, none in front of the first: what's
    /// already in `out` is the Latin run this one follows, and its own
    /// spacing is the text's business, not the segmenter's.
    pub(crate) fn read(&self, text: &str, out: &mut String) -> bool {
        let Ok(tokens) = self.segmenter.segment(Cow::Borrowed(text)) else {
            return false;
        };
        let mut first = true;
        for mut token in tokens {
            // Taken before the details are touched: `get` borrows the
            // token mutably to fault its details in on first use.
            let surface = token.surface.to_string();
            let particle = token
                .get(PART_OF_SPEECH)
                .is_some_and(|class| class == PARTICLE);
            // IPADIC writes `*` for a field it has no value for, and an
            // unknown word gets the whole unknown-entry row, so both the
            // missing and the placeholder case land here.
            let reading = match token.get(READING) {
                Some(reading) if reading != "*" && !reading.is_empty() => reading.to_string(),
                // A word the dictionary doesn't hold is still readable
                // when it's written in kana, which is most of what's left.
                _ => surface.clone(),
            };
            if !first && !joins_left(&surface) {
                out.push(' ');
            }
            first = false;
            match particle.then(|| spoken(&surface)).flatten() {
                Some(said) => out.push_str(said),
                // Nothing else departs from the reading IPADIC recorded.
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
