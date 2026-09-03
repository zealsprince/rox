//! Hangul to Latin, by arithmetic.
//!
//! Korean is the easy one. Every precomposed syllable in U+AC00..U+D7A3 is
//! a formula rather than an entry: the codepoint's distance from AC00
//! factors exactly into an initial consonant, a medial vowel and an
//! optional final consonant, nineteen by twenty-one by twenty-eight. So
//! there's no table of eleven thousand syllables here, just three short
//! lists of jamo and a division. Nothing is downloaded and nothing can be
//! missing: 서태지 romanizes on a fresh install.
//!
//! Revised Romanization, the South Korean standard since 2000, without its
//! assimilation rules. RR proper respells consonants across a syllable
//! boundary the way they're actually pronounced (신라 is Silla, not
//! Sinla), which needs the sound-change table and a lookahead. What comes
//! out of here is the syllable-by-syllable transliteration instead, so
//! that name reads `sinla`. For a search key that's the better trade in
//! both directions: it's deterministic, and somebody typing what they see
//! written is as likely to type the letters as the sounds.

/// The first syllable in the precomposed block, the base every syllable's
/// index is measured from.
const BASE: u32 = 0xAC00;

/// The nineteen initial consonants, in codepoint order. The eleventh is
/// ㅇ, which is silent in this position and so spells nothing.
const INITIAL: [&str; 19] = [
    "g", "kk", "n", "d", "tt", "r", "m", "b", "pp", "s", "ss", "", "j", "jj", "ch", "k", "t", "p",
    "h",
];

/// The twenty-one medial vowels, in codepoint order.
const MEDIAL: [&str; 21] = [
    "a", "ae", "ya", "yae", "eo", "e", "yeo", "ye", "o", "wa", "wae", "oe", "yo", "u", "wo", "we",
    "wi", "yu", "eu", "ui", "i",
];

/// The twenty-eight finals, in codepoint order, starting with the empty
/// one that means the syllable has no final consonant at all. RR spells a
/// final by how it's released, which is why several distinct jamo share a
/// letter here: ㅅ, ㅆ, ㅈ, ㅊ, ㅌ and ㅎ all stop as `t`.
const FINAL: [&str; 28] = [
    "", "k", "k", "ks", "n", "nj", "nh", "t", "l", "lk", "lm", "lb", "ls", "lt", "lp", "lh", "m",
    "p", "ps", "t", "t", "ng", "t", "t", "k", "t", "p", "t",
];

/// Whether a character is a precomposed hangul syllable. The archaic
/// conjoining jamo blocks are not included: nothing writes a song title in
/// them, and a text carrying one is refused rather than half-read.
pub(crate) fn is_hangul(c: char) -> bool {
    matches!(c, '\u{AC00}'..='\u{D7A3}')
}

/// Append `text`'s romanization to `out`. False when a character isn't a
/// precomposed syllable, which throws the whole answer away.
pub(crate) fn romanize(text: &str, out: &mut String) -> bool {
    for c in text.chars() {
        if !is_hangul(c) {
            return false;
        }
        let index = c as u32 - BASE;
        // The three factors, outermost first: 21 vowels times 28 finals to
        // a consonant, 28 finals to a vowel.
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
        // 19 * 21 * 28 is exactly the syllables between AC00 and D7A3.
        assert_eq!(
            INITIAL.len() * MEDIAL.len() * FINAL.len(),
            (0xD7A3 - 0xAC00 + 1)
        );
    }

    #[test]
    fn a_name_comes_apart_into_its_jamo() {
        // 서태지: silent initial, then two syllables with finals.
        assert_eq!(read("서태지").unwrap(), "seotaeji");
        assert_eq!(read("한국").unwrap(), "hanguk");
        assert_eq!(read("김치").unwrap(), "gimchi");
        // The corners of the block, which is where an off-by-one in the
        // division would show up.
        assert_eq!(read("가").unwrap(), "ga");
        // The last syllable in the block: a final ㅎ is released as t,
        // the same stop the other five finals in that row take.
        assert_eq!(read("힣").unwrap(), "hit");
    }

    #[test]
    fn the_assimilation_rules_are_deliberately_not_applied() {
        // RR proper spells this Silla, because ㄴ followed by ㄹ is
        // pronounced ll. Syllable by syllable it's the final n and the
        // initial r, which is what a person reading the letters would
        // type, and it's the one that never needs a lookahead.
        assert_eq!(read("신라").unwrap(), "sinra");
    }

    #[test]
    fn anything_that_isnt_a_syllable_refuses_the_whole_text() {
        assert!(read("서 태지").is_none());
        assert!(read("ㄱ").is_none());
    }
}
