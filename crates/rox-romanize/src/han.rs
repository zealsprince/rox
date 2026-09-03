//! Han characters read as Mandarin, through the `pinyin` crate's table.
//!
//! This is the fallback for Han text with no kana anywhere near it and no
//! reason to think it's Japanese. It's a per-character table with no
//! segmentation and no context, which is exactly as much as pinyin needs
//! for a search key: a Chinese character has one common reading in the
//! overwhelming majority of cases, and the crate's table already carries
//! that one first.
//!
//! Tone marks are dropped. `plain` is the crate's own toneless spelling,
//! which is what someone typing 邓丽君 into a search box from memory would
//! write: `deng li jun`, not `dènglìjūn`.
//!
//! One space per character, which is the unsegmented form pinyin is
//! usually written in outside of proper names. Where the word boundaries
//! fall is a question this table can't answer (that would need the
//! dictionary the Japanese path has and this one doesn't), so it doesn't
//! pretend to: 秋风 is `qiu feng` rather than a guess at `qiufeng`, and
//! search matches either half.
//!
//! Note what this deliberately isn't: a dictionary. The table is a few
//! hundred kilobytes compiled into the binary, and that's the whole cost.
//! The thing Andrew ruled out shipping is the morphological dictionary the
//! Japanese path needs, which is a hundred times the size and lands as a
//! download.

use pinyin::ToPinyin;

/// Whether a character is a Han ideograph. The two iteration marks are
/// counted in because Japanese names use them (佐々木) and the Japanese
/// path reads them as part of a token's surface; the pinyin table has no
/// entry for either, so a Chinese text carrying one is refused whole.
pub(crate) fn is_han(c: char) -> bool {
    matches!(c,
        '\u{3005}' | '\u{3007}' | '\u{303B}'
        | '\u{3400}'..='\u{4DBF}'
        | '\u{4E00}'..='\u{9FFF}'
        | '\u{F900}'..='\u{FAFF}'
        | '\u{20000}'..='\u{2FA1F}')
}

/// Append `text`'s Mandarin reading to `out`, a space between syllables.
/// False when a character has no entry in the table, which throws the
/// whole answer away rather than leaving a hole in the middle of a title.
///
/// Nothing goes in front of the first syllable: whatever is already in
/// `out` is the Latin run this one follows, and its spacing is the text's.
pub(crate) fn romanize(text: &str, out: &mut String) -> bool {
    for (n, c) in text.chars().enumerate() {
        let Some(reading) = c.to_pinyin() else {
            return false;
        };
        if n > 0 {
            out.push(' ');
        }
        out.push_str(reading.plain());
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> Option<String> {
        let mut out = String::new();
        romanize(text, &mut out).then_some(out)
    }

    #[test]
    fn a_name_comes_back_toneless_one_syllable_a_word() {
        assert_eq!(read("北京").unwrap(), "bei jing");
        assert_eq!(read("邓丽君").unwrap(), "deng li jun");
        // Traditional and simplified both have entries.
        assert_eq!(read("鄧麗君").unwrap(), "deng li jun");
    }

    #[test]
    fn a_character_with_no_reading_refuses_the_whole_text() {
        // The iteration mark counts as Han for the Japanese path's sake
        // and has no Mandarin reading of its own.
        assert!(read("佐々木").is_none());
        assert!(read("abc").is_none());
    }
}
