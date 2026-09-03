//! Kana to romaji: the one script in this crate that needs no data at all.
//!
//! Hiragana and katakana are a syllabary, so the mapping is a table and a
//! few combining rules rather than a lookup into a dictionary. That's why
//! kana keeps working when the IPADIC download isn't installed, and it's
//! also why this module is the back end of the Japanese path rather than a
//! sibling of it: Lindera hands back a katakana `reading` for every token
//! it recognizes, and turning that reading into letters is this table's
//! job either way.
//!
//! Modified Hepburn, with two deliberate departures, both because what
//! comes out of here is a search key rather than a transcription:
//!
//! - Long vowels are spelled out rather than macronned, so 東京 reads
//!   `toukyou`. A macron is unreachable from Andrew's keyboard, and the
//!   stripped-down `tokyo` loses the distinction between 大阪 and お坂.
//!   Wapuro romaji is what a person actually types into a search box.
//!   `ー` repeats the vowel before it for the same reason.
//! - Syllabic ン is always `n`, never Hepburn's `n'` before a vowel. Nobody
//!   types the apostrophe, so `shin'ichi` would be unfindable by `shinichi`.
//!
//! Digraphs come out of rules rather than out of an eighty-row table.
//! Every youon is its base syllable with the final vowel traded for `y`
//! plus the small vowel (`ki` + ャ -> `kya`, `fu` + ュ -> `fyu`), the three
//! irregular bases (`shi`, `chi`, `ji`) drop their vowel instead, and every
//! small-vowel pair swaps the base's own vowel (`fu` + ァ -> `fa`). Table
//! rows would be shorter to read and far easier to get individually wrong.

/// Whether a character is kana this module can read: both syllabaries and
/// the long-vowel mark. Halfwidth katakana (U+FF66 and up) is deliberately
/// not here; it's vanishingly rare in tags and a text carrying it is
/// refused whole rather than half-read.
pub(crate) fn is_kana(c: char) -> bool {
    matches!(c, '\u{3041}'..='\u{3096}' | '\u{30A1}'..='\u{30FA}' | '\u{30FC}')
}

/// The katakana twin of a hiragana character; anything else unchanged. The
/// two syllabaries are laid out in the same order 0x60 apart, so the whole
/// module can work in one of them.
fn katakana(c: char) -> char {
    match c {
        '\u{3041}'..='\u{3096}' => char::from_u32(c as u32 + 0x60).unwrap_or(c),
        _ => c,
    }
}

/// Whether a character is one of the small kana that binds to the syllable
/// before it. The small tsu is not among them: it doubles a consonant
/// rather than joining a syllable, and it's handled on its own.
fn is_small(c: char) -> bool {
    matches!(
        c,
        'ァ' | 'ィ' | 'ゥ' | 'ェ' | 'ォ' | 'ャ' | 'ュ' | 'ョ' | 'ヮ'
    )
}

/// Whether a character is part of the syllable in front of it rather than
/// a sound of its own: a small kana, a small tsu or a long-vowel mark.
/// [`crate::japanese`] asks before it puts a space between two tokens,
/// because the segmenter will hand one of these over on its own and a
/// space there cuts a word in half (ラー | メン).
pub(crate) fn binds_left(c: char) -> bool {
    let c = katakana(c);
    c == 'ッ' || c == 'ー' || is_small(c)
}

fn is_vowel(c: char) -> bool {
    matches!(c, 'a' | 'i' | 'u' | 'e' | 'o')
}

/// One kana's romaji on its own, before anything after it gets a say.
/// None for a character this table doesn't cover, which refuses the whole
/// text rather than dropping a syllable out of the middle of a title.
fn syllable(c: char) -> Option<&'static str> {
    Some(match c {
        'ア' | 'ァ' => "a",
        'イ' | 'ィ' => "i",
        'ウ' | 'ゥ' => "u",
        'エ' | 'ェ' => "e",
        'オ' | 'ォ' => "o",
        'カ' | 'ヵ' => "ka",
        'キ' => "ki",
        'ク' => "ku",
        'ケ' | 'ヶ' => "ke",
        'コ' => "ko",
        'ガ' => "ga",
        'ギ' => "gi",
        'グ' => "gu",
        'ゲ' => "ge",
        'ゴ' => "go",
        'サ' => "sa",
        'シ' => "shi",
        'ス' => "su",
        'セ' => "se",
        'ソ' => "so",
        'ザ' => "za",
        'ジ' => "ji",
        'ズ' => "zu",
        'ゼ' => "ze",
        'ゾ' => "zo",
        'タ' => "ta",
        'チ' => "chi",
        'ツ' => "tsu",
        'テ' => "te",
        'ト' => "to",
        'ダ' => "da",
        // Both of these merged with ジ and ズ centuries ago and are
        // pronounced the same way today; Hepburn spells them that way too.
        'ヂ' => "ji",
        'ヅ' => "zu",
        'デ' => "de",
        'ド' => "do",
        'ナ' => "na",
        'ニ' => "ni",
        'ヌ' => "nu",
        'ネ' => "ne",
        'ノ' => "no",
        'ハ' => "ha",
        'ヒ' => "hi",
        'フ' => "fu",
        'ヘ' => "he",
        'ホ' => "ho",
        'バ' => "ba",
        'ビ' => "bi",
        'ブ' => "bu",
        'ベ' => "be",
        'ボ' => "bo",
        'パ' => "pa",
        'ピ' => "pi",
        'プ' => "pu",
        'ペ' => "pe",
        'ポ' => "po",
        'マ' => "ma",
        'ミ' => "mi",
        'ム' => "mu",
        'メ' => "me",
        'モ' => "mo",
        'ヤ' | 'ャ' => "ya",
        'ユ' | 'ュ' => "yu",
        'ヨ' | 'ョ' => "yo",
        'ラ' => "ra",
        'リ' => "ri",
        'ル' => "ru",
        'レ' => "re",
        'ロ' => "ro",
        'ワ' | 'ヮ' => "wa",
        // The two obsolete kana and the object-marker ヲ: all three are
        // read as bare vowels now, whatever they once were.
        'ヰ' => "i",
        'ヱ' => "e",
        'ヲ' => "o",
        'ン' => "n",
        'ヴ' => "vu",
        'ヷ' => "va",
        'ヸ' => "vi",
        'ヹ' => "ve",
        'ヺ' => "vo",
        _ => return None,
    })
}

/// A syllable and the small kana bound to it, as one sound. None when the
/// pair isn't a real combination (ン followed by a small vowel, say), which
/// leaves the small kana to be read as its own syllable.
fn combine(base: &str, small: char) -> Option<String> {
    let vowel = match small {
        'ャ' => 'a',
        'ュ' => 'u',
        'ョ' => 'o',
        'ァ' | 'ヮ' => 'a',
        'ィ' => 'i',
        'ゥ' => 'u',
        'ェ' => 'e',
        'ォ' => 'o',
        _ => return None,
    };
    // Youon: the palatalized series. The i-row is where it comes from
    // natively (ki + ャ is kya), and loanwords built the same shape on the
    // u and e rows for sounds Japanese had no kana for: フュージョン is
    // fyuujon, デュオ is dyuo, テューバ is tyuuba. All three trade the
    // base's vowel for a y, so the rule is one rule; only the three
    // irregular bases drop their vowel without leaving one behind.
    if matches!(small, 'ャ' | 'ュ' | 'ョ') {
        let stem = match base {
            "shi" => "sh".to_string(),
            "chi" => "ch".to_string(),
            "ji" => "j".to_string(),
            _ => format!("{}y", base.strip_suffix(['i', 'u', 'e'])?),
        };
        return Some(format!("{stem}{vowel}"));
    }
    // ヮ only ever follows ク in practice, and it's the base's vowel that
    // gives way rather than the w.
    if small == 'ヮ' {
        let stem = base.strip_suffix(is_vowel)?;
        return Some(format!("{stem}wa"));
    }
    // A small vowel swaps the base's own: フ + ァ is fa, テ + ィ is ti.
    // The two bare-vowel bases are the exceptions, because dropping their
    // only letter would leave nothing to carry the sound: ウ takes a w and
    // イ takes a y.
    let stem = match base {
        "u" => "w",
        "i" => "y",
        _ => base.strip_suffix(is_vowel)?,
    };
    Some(format!("{stem}{vowel}"))
}

/// The vowel a long mark should repeat: the last one written so far.
fn last_vowel(out: &str) -> Option<char> {
    out.chars().rev().find(|&c| is_vowel(c))
}

/// What a small tsu contributes in front of `next`: the doubled consonant.
/// Hepburn's one irregularity here is that a doubled `ch` is written `tch`
/// (マッチ is matchi), not `chch`.
fn geminate(next: &str) -> Option<char> {
    if next.starts_with("ch") {
        return Some('t');
    }
    match next.chars().next() {
        Some(c) if c.is_ascii_alphabetic() && !is_vowel(c) => Some(c),
        _ => None,
    }
}

/// Append `text`'s romaji to `out`. False when a character isn't kana this
/// table reads, in which case `out` is left holding whatever came before
/// it and the caller throws the whole answer away.
pub(crate) fn romaji(text: &str, out: &mut String) -> bool {
    let chars: Vec<char> = text.chars().map(katakana).collect();
    let mut i = 0;
    let mut doubled = false;
    while i < chars.len() {
        let c = chars[i];
        if c == 'ッ' {
            doubled = true;
            i += 1;
            continue;
        }
        if c == 'ー' {
            // A long mark with nothing in front of it has no vowel to
            // repeat, which happens in a title that opens with one; drop it
            // rather than refusing the title.
            if let Some(vowel) = last_vowel(out) {
                out.push(vowel);
            }
            i += 1;
            continue;
        }
        let Some(base) = syllable(c) else {
            return false;
        };
        let joined = chars
            .get(i + 1)
            .copied()
            .filter(|&next| is_small(next))
            .and_then(|next| combine(base, next));
        let (sound, width) = match &joined {
            Some(sound) => (sound.as_str(), 2),
            None => (base, 1),
        };
        if doubled {
            if let Some(consonant) = geminate(sound) {
                out.push(consonant);
            }
            doubled = false;
        }
        out.push_str(sound);
        i += width;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> Option<String> {
        let mut out = String::new();
        romaji(text, &mut out).then_some(out)
    }

    #[test]
    fn the_plain_syllabary_reads_the_same_in_both_scripts() {
        assert_eq!(read("さくら").unwrap(), "sakura");
        assert_eq!(read("サクラ").unwrap(), "sakura");
        assert_eq!(read("ひらがな").unwrap(), "hiragana");
        assert_eq!(read("ン").unwrap(), "n");
    }

    #[test]
    fn the_irregular_rows_take_hepburns_spelling() {
        assert_eq!(read("しちつふじ").unwrap(), "shichitsufuji");
        assert_eq!(read("ヂヅヲヰヱ").unwrap(), "jizuoie");
    }

    #[test]
    fn small_kana_bind_to_the_syllable_in_front_of_them() {
        assert_eq!(read("きゃきゅきょ").unwrap(), "kyakyukyo");
        assert_eq!(read("しゃちゅじょ").unwrap(), "shachujo");
        assert_eq!(read("ファイト").unwrap(), "faito");
        assert_eq!(read("ヴィーナス").unwrap(), "viinasu");
        assert_eq!(read("ウィスキー").unwrap(), "wisukii");
        assert_eq!(read("パーティー").unwrap(), "paatii");
        assert_eq!(read("チェック").unwrap(), "chekku");
        // The loanword youon: a u or e base palatalizes the same way the
        // i-row does, rather than leaving the small kana to be read on its
        // own as fuyuujon.
        assert_eq!(read("フュージョン").unwrap(), "fyuujon");
        assert_eq!(read("デュオ").unwrap(), "dyuo");
        assert_eq!(read("ヴュルツブルク").unwrap(), "vyurutsuburuku");
        // A base with no i, u or e to trade has nothing to palatalize, so
        // the small kana stays a syllable of its own.
        assert_eq!(read("アャ").unwrap(), "aya");
    }

    #[test]
    fn a_small_tsu_doubles_the_consonant_after_it() {
        assert_eq!(read("がっこう").unwrap(), "gakkou");
        assert_eq!(read("ざっし").unwrap(), "zasshi");
        // The one place Hepburn refuses to double the letters it sees.
        assert_eq!(read("マッチ").unwrap(), "matchi");
        // Nothing to double: a trailing small tsu is the glottal stop that
        // ends a shouted word, and it spells nothing.
        assert_eq!(read("あっ").unwrap(), "a");
    }

    #[test]
    fn a_long_mark_repeats_the_vowel_before_it() {
        assert_eq!(read("レモン").unwrap(), "remon");
        assert_eq!(read("ラーメン").unwrap(), "raamen");
        assert_eq!(read("コーヒー").unwrap(), "koohii");
        // Nothing in front of it to lengthen, so it spells nothing rather
        // than refusing the title.
        assert_eq!(read("ーア").unwrap(), "a");
    }

    #[test]
    fn a_character_this_table_cant_read_refuses_the_whole_text() {
        // Kanji, halfwidth katakana and the iteration mark: all three are
        // somebody else's problem, and half a reading is worse than none.
        assert!(read("東京").is_none());
        assert!(read("ｻｸﾗ").is_none());
        assert!(read("ヽ").is_none());
    }
}
