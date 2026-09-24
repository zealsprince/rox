//! Han read as Mandarin through the `pinyin` crate's table: the fallback for
//! Han with no kana near it. Per character, no segmentation, which is enough
//! for a search key since most characters have one common reading.
//!
//! Toneless (`deng li jun`, not `dènglìjūn`), one space per character: without
//! a dictionary there's no knowing word boundaries, and search matches either
//! half. The table is a few hundred kilobytes compiled in.

use pinyin::ToPinyin;

/// The iteration marks count as Han because the Japanese path reads them
/// (佐々木); pinyin has no entry, so Chinese text carrying one is refused.
pub(crate) fn is_han(c: char) -> bool {
    matches!(c,
        '\u{3005}' | '\u{3007}' | '\u{303B}'
        | '\u{3400}'..='\u{4DBF}'
        | '\u{4E00}'..='\u{9FFF}'
        | '\u{F900}'..='\u{FAFF}'
        | '\u{20000}'..='\u{2FA1F}')
}

/// A space between syllables, none before the first. False when a character
/// has no reading, which throws the whole answer away.
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
        assert_eq!(read("鄧麗君").unwrap(), "deng li jun");
    }

    #[test]
    fn a_character_with_no_reading_refuses_the_whole_text() {
        assert!(read("佐々木").is_none());
        assert!(read("abc").is_none());
    }
}
