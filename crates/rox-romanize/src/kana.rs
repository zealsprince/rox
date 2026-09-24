//! Kana to romaji: a table and a few combining rules, no data. It's also the
//! back end of the Japanese path, which hands over katakana readings.
//!
//! Modified Hepburn, bent toward what people type into a search box:
//!
//! - Long vowels are spelled out (`toukyou`), not macronned; `ー` repeats the
//!   vowel before it.
//! - Syllabic ン is always `n`, never `n'`.
//!
//! Digraphs come from rules rather than an eighty-row table: youon trade the
//! base's vowel for `y` (`ki` + ャ -> `kya`), and small vowels swap the base's
//! own (`fu` + ァ -> `fa`).

/// Halfwidth katakana (U+FF66 and up) is left out; text carrying it is refused.
pub(crate) fn is_kana(c: char) -> bool {
    matches!(c, '\u{3041}'..='\u{3096}' | '\u{30A1}'..='\u{30FA}' | '\u{30FC}')
}

/// The two syllabaries sit 0x60 apart.
fn katakana(c: char) -> char {
    match c {
        '\u{3041}'..='\u{3096}' => char::from_u32(c as u32 + 0x60).unwrap_or(c),
        _ => c,
    }
}

/// The small tsu isn't one: it doubles a consonant, handled on its own.
fn is_small(c: char) -> bool {
    matches!(
        c,
        'ァ' | 'ィ' | 'ゥ' | 'ェ' | 'ォ' | 'ャ' | 'ュ' | 'ョ' | 'ヮ'
    )
}

/// A small kana, small tsu or long mark. The segmenter can split one off, and
/// a space there cuts a word in half (ラー | メン).
pub(crate) fn binds_left(c: char) -> bool {
    let c = katakana(c);
    c == 'ッ' || c == 'ー' || is_small(c)
}

fn is_vowel(c: char) -> bool {
    matches!(c, 'a' | 'i' | 'u' | 'e' | 'o')
}

/// None refuses the whole text rather than dropping a syllable.
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

/// None when the pair isn't a real combination; the small kana then stands alone.
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
    // Youon, which loanwords extend to the u and e rows (フュ fyu, デュ dyu).
    // The irregular bases drop their vowel instead.
    if matches!(small, 'ャ' | 'ュ' | 'ョ') {
        let stem = match base {
            "shi" => "sh".to_string(),
            "chi" => "ch".to_string(),
            "ji" => "j".to_string(),
            _ => format!("{}y", base.strip_suffix(['i', 'u', 'e'])?),
        };
        return Some(format!("{stem}{vowel}"));
    }
    // ヮ only follows ク in practice.
    if small == 'ヮ' {
        let stem = base.strip_suffix(is_vowel)?;
        return Some(format!("{stem}wa"));
    }
    // ウ and イ have no consonant to keep, so they take w and y.
    let stem = match base {
        "u" => "w",
        "i" => "y",
        _ => base.strip_suffix(is_vowel)?,
    };
    Some(format!("{stem}{vowel}"))
}

fn last_vowel(out: &str) -> Option<char> {
    out.chars().rev().find(|&c| is_vowel(c))
}

/// A doubled `ch` is written `tch` (マッチ is matchi).
fn geminate(next: &str) -> Option<char> {
    if next.starts_with("ch") {
        return Some('t');
    }
    match next.chars().next() {
        Some(c) if c.is_ascii_alphabetic() && !is_vowel(c) => Some(c),
        _ => None,
    }
}

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
            // A leading long mark has no vowel to repeat; drop it.
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
        assert_eq!(read("フュージョン").unwrap(), "fyuujon");
        assert_eq!(read("デュオ").unwrap(), "dyuo");
        assert_eq!(read("ヴュルツブルク").unwrap(), "vyurutsuburuku");
        assert_eq!(read("アャ").unwrap(), "aya");
    }

    #[test]
    fn a_small_tsu_doubles_the_consonant_after_it() {
        assert_eq!(read("がっこう").unwrap(), "gakkou");
        assert_eq!(read("ざっし").unwrap(), "zasshi");
        assert_eq!(read("マッチ").unwrap(), "matchi");
        // A trailing small tsu spells nothing.
        assert_eq!(read("あっ").unwrap(), "a");
    }

    #[test]
    fn a_long_mark_repeats_the_vowel_before_it() {
        assert_eq!(read("レモン").unwrap(), "remon");
        assert_eq!(read("ラーメン").unwrap(), "raamen");
        assert_eq!(read("コーヒー").unwrap(), "koohii");
        assert_eq!(read("ーア").unwrap(), "a");
    }

    #[test]
    fn a_character_this_table_cant_read_refuses_the_whole_text() {
        assert!(read("東京").is_none());
        assert!(read("ｻｸﾗ").is_none());
        assert!(read("ヽ").is_none());
    }
}
