//! Hangul to Latin, by arithmetic: every syllable in U+AC00..U+D7A3 factors
//! into initial, medial and final (19 x 21 x 28), so three short jamo lists
//! replace a table of eleven thousand. Nothing to download.
//!
//! Revised Romanization without its assimilation rules: 신라 reads `sinra`,
//! not Silla. Deterministic, and people type the letters they see as often as
//! the sounds.

const BASE: u32 = 0xAC00;

/// The twelfth, ㅇ, is silent as an initial.
const INITIAL: [&str; 19] = [
    "g", "kk", "n", "d", "tt", "r", "m", "b", "pp", "s", "ss", "", "j", "jj", "ch", "k", "t", "p",
    "h",
];

const MEDIAL: [&str; 21] = [
    "a", "ae", "ya", "yae", "eo", "e", "yeo", "ye", "o", "wa", "wae", "oe", "yo", "u", "wo", "we",
    "wi", "yu", "eu", "ui", "i",
];

/// Starting with the empty final. RR spells a final by its release, so
/// ㅅ, ㅆ, ㅈ, ㅊ, ㅌ and ㅎ all stop as `t`.
const FINAL: [&str; 28] = [
    "", "k", "k", "ks", "n", "nj", "nh", "t", "l", "lk", "lm", "lb", "ls", "lt", "lp", "lh", "m",
    "p", "ps", "t", "t", "ng", "t", "t", "k", "t", "p", "t",
];

/// Archaic conjoining jamo aren't included; a text carrying them is refused.
pub(crate) fn is_hangul(c: char) -> bool {
    matches!(c, '\u{AC00}'..='\u{D7A3}')
}

pub(crate) fn romanize(text: &str, out: &mut String) -> bool {
    for c in text.chars() {
        if !is_hangul(c) {
            return false;
        }
        let index = c as u32 - BASE;
        out.push_str(INITIAL[(index / (21 * 28)) as usize]);
        out.push_str(MEDIAL[(index / 28 % 21) as usize]);
        out.push_str(FINAL[(index % 28) as usize]);
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
    fn the_three_jamo_lists_line_up_with_the_block() {
        assert_eq!(
            INITIAL.len() * MEDIAL.len() * FINAL.len(),
            (0xD7A3 - 0xAC00 + 1)
        );
    }

    #[test]
    fn a_name_comes_apart_into_its_jamo() {
        assert_eq!(read("서태지").unwrap(), "seotaeji");
        assert_eq!(read("한국").unwrap(), "hanguk");
        assert_eq!(read("김치").unwrap(), "gimchi");
        assert_eq!(read("가").unwrap(), "ga");
        // The last syllable: a final ㅎ releases as t.
        assert_eq!(read("힣").unwrap(), "hit");
    }

    #[test]
    fn the_assimilation_rules_are_deliberately_not_applied() {
        assert_eq!(read("신라").unwrap(), "sinra");
    }

    #[test]
    fn anything_that_isnt_a_syllable_refuses_the_whole_text() {
        assert!(read("서 태지").is_none());
        assert!(read("ㄱ").is_none());
    }
}
