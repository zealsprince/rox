//! Latin spellings for text that has none.
//!
//! The problem this exists for is small and concrete. Andrew's library
//! holds Japanese, Korean and Chinese titles; almost none of the files
//! carry a sort tag; MusicBrainz has a sort name for artists and nothing
//! at all for titles or albums. So a track called レモン files in its own
//! bucket at the end of every letter rail and can't be found from a Latin
//! keyboard, and the only remaining source for its sort name is to read
//! the characters and write down what they say. That's what this does.
//!
//! It reopens a line the sort-names contract drew on purpose ("no
//! romanization library"), because the alternative turned out to be
//! leaving a fifth of a library unfindable.
//!
//! ## What it will and won't answer
//!
//! [`romanize`] returns None rather than a guess in three cases, and the
//! pass above it treats all three the same way: the row is left alone.
//!
//! - The text is already Latin, so there's nothing to add.
//! - It carries a script this crate doesn't read (Cyrillic, Greek, Thai,
//!   halfwidth katakana), or mixes two it can't route between.
//! - It's kanji-bearing and no dictionary is installed. A wrong answer
//!   here would specifically be a *Chinese* reading of Japanese text,
//!   which is not a near miss.
//!
//! ## Per script
//!
//! - **Hangul** is arithmetic: the syllable block factors into jamo, and
//!   [`hangul`] does the division. No data, never absent.
//! - **Han with no kana anywhere near it** is read as Mandarin from the
//!   `pinyin` crate's table, tones stripped ([`han`]).
//! - **Anything with kana in it**, plus Han a caller tells us is Japanese,
//!   goes through Lindera and IPADIC ([`japanese`]), falling back to the
//!   kana table alone when the dictionary isn't installed. Kana is a
//!   syllabary, so kana-only text romanizes on a fresh install.
//! - **Latin runs and punctuation are kept**, so "Lemon (レモン)" comes
//!   back "Lemon (remon)". Fullwidth forms and CJK punctuation are folded
//!   to their ASCII equivalents on the way through, since a sort name
//!   full of ！ and 　 is no more typeable than the kanji was.
//!
//! ## What it isn't
//!
//! A transcription. Every choice here favours what somebody would type
//! into a search box over what a style guide would print: wapuro romaji
//! rather than macrons, no apostrophe after a syllabic n, no Revised
//! Romanization sound changes, no tone marks. See each module for which
//! rule it broke and why.

use std::sync::Mutex;

pub mod dictionary;
mod han;
mod hangul;
mod japanese;
mod kana;

pub use japanese::Japanese;

/// What a caller knows about the text that this crate can't see for
/// itself.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Reading {
    /// Work it out from the characters. Han with no kana in sight reads as
    /// Mandarin, which is right far more often than not.
    #[default]
    Auto,
    /// Han in this text is Japanese. Kanji and hanzi are the same
    /// characters, so nothing in a bare 東京 says which language wrote it;
    /// the caller knows, because it has the rest of the row.
    Japanese,
}

/// Which of the three back ends a run of text goes to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Route {
    /// Copied through, punctuation folded to ASCII.
    Keep,
    Japanese,
    Hangul,
    Han,
}

/// What a character is, before the routing decision folds kana and Han
/// together.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Keep,
    Kana,
    Hangul,
    Han,
    Unreadable,
}

/// Characters that pass through untouched: ASCII, the Latin alphabet with
/// every accent it wears, and the punctuation, currency and symbol blocks
/// that sit between the scripts.
fn is_pass_through(c: char) -> bool {
    c.is_ascii()
        || matches!(c,
            '\u{00A0}'..='\u{02AF}'   // Latin-1 supplement through IPA extensions
            | '\u{0300}'..='\u{036F}' // combining marks, an accent's other half
            | '\u{1E00}'..='\u{1EFF}' // Latin extended additional
            | '\u{2000}'..='\u{2BFF}' // punctuation, currency, symbols, arrows
        )
}

/// The ASCII a CJK punctuation mark stands in for, or None when the
/// character isn't punctuation this folds.
///
/// Only the marks that have an unambiguous ASCII counterpart. A sort name
/// is typed, and 「」 in one is as unreachable as the kanji beside it.
fn fold_punctuation(c: char) -> Option<&'static str> {
    // Fullwidth ASCII is the same block shifted by a constant, so it folds
    // by arithmetic rather than by table. Handled by the caller, which has
    // a String to push into; this only covers the ones that need naming.
    Some(match c {
        '\u{3000}' | '・' => " ",
        '、' => ",",
        '。' => ".",
        '「' | '」' | '『' | '』' => "\"",
        '〈' | '《' => "<",
        '〉' | '》' => ">",
        '【' | '〔' => "[",
        '】' | '〕' => "]",
        '〜' => "~",
        _ => return None,
    })
}

/// The fullwidth twin of an ASCII character folds back to it by subtracting
/// a constant: the block at U+FF01 is U+0021 shifted up by 0xFEE0.
fn fold_fullwidth(c: char) -> Option<char> {
    matches!(c, '\u{FF01}'..='\u{FF5E}')
        .then(|| char::from_u32(c as u32 - 0xFEE0))
        .flatten()
}

fn class(c: char) -> Class {
    if kana::is_kana(c) {
        Class::Kana
    } else if hangul::is_hangul(c) {
        Class::Hangul
    } else if han::is_han(c) {
        Class::Han
    } else if is_pass_through(c) || fold_punctuation(c).is_some() || fold_fullwidth(c).is_some() {
        Class::Keep
    } else {
        Class::Unreadable
    }
}

/// Whether the text carries kana, which is the one unambiguous signal that
/// a row is Japanese rather than Chinese. The pass uses it on a row's
/// other fields to decide what a bare-kanji title is.
pub fn has_kana(text: &str) -> bool {
    text.chars().any(kana::is_kana)
}

/// Whether romanizing this text would need the downloaded dictionary: it
/// carries Han that routes to the Japanese reader. The pass asks this
/// before it starts, so it can refuse with a reason instead of grinding
/// through a backlog it can't answer.
pub fn needs_dictionary(text: &str, reading: Reading) -> bool {
    let japanese = reading == Reading::Japanese || has_kana(text);
    japanese && text.chars().any(han::is_han)
}

/// What [`japanese`] last answered: None until something asks, then the
/// load's verdict, kept so a library with no dictionary doesn't stat the
/// models directory once per row.
static LOADED: Mutex<Option<Option<&'static Japanese>>> = Mutex::new(None);

/// The process's one loaded dictionary, or None on an install that
/// hasn't downloaded it yet (or one whose download won't open).
///
/// IPADIC is forty megabytes of mapped tables, and by now two callers
/// want it: the library pass reading every title, and the metadata
/// panel filling one track's sort names on a click. Loading it twice
/// would map it twice, so it's loaded once here and handed out by
/// reference.
///
/// The first call pays for the load, which for the panel means the click
/// that runs Romanize can sit on a dictionary open. Every call after it
/// is a lock and a copy. [`reload`] is how an install or a delete
/// mid-session gets seen.
pub fn japanese() -> Option<&'static Japanese> {
    // A poisoned lock means a load panicked on another thread. Whatever
    // the slot holds is still the answer, and refusing to romanize for
    // the rest of the session helps nobody.
    let mut slot = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(loaded) = *slot {
        return loaded;
    }
    let loaded = open_installed();
    *slot = Some(loaded);
    loaded
}

/// Forget what [`japanese`] last answered, so the next call looks at the
/// models directory again. The settings page calls this when a download
/// finishes or a dictionary is deleted; without it an install mid-session
/// wouldn't take until a restart.
///
/// A dictionary already handed out stays alive: it's leaked, and callers
/// hold `&'static` references to it. That's a bounded cost, one mapping
/// per install in a session, against handing out a reference into a
/// dictionary that could be dropped under it.
pub fn reload() {
    *LOADED.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The load itself, leaked so the reference outlives every caller. Only
/// reached with the slot's lock held, so it runs once per [`reload`] at
/// most.
fn open_installed() -> Option<&'static Japanese> {
    if !dictionary::IPADIC.installed() {
        return None;
    }
    match Japanese::open() {
        Ok(ja) => Some(Box::leak(Box::new(ja))),
        Err(e) => {
            log::warn!("romanize: the dictionary would not load: {e}");
            None
        }
    }
}

/// A Latin spelling of `text`, or None when there isn't one worth having.
/// See the module header for the three cases that answer None.
///
/// `ja` is a loaded dictionary, or None on an install that hasn't
/// downloaded one. Without it, kana, hangul and Chinese still answer;
/// kanji doesn't.
pub fn romanize(text: &str, ja: Option<&Japanese>) -> Option<String> {
    romanize_as(text, ja, Reading::Auto)
}

/// [`romanize`], told what language the Han in the text is. Everything
/// else about it is the same.
pub fn romanize_as(text: &str, ja: Option<&Japanese>, reading: Reading) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut kana_seen = false;
    let mut hangul_seen = false;
    let mut han_seen = false;
    for &c in &chars {
        match class(c) {
            Class::Unreadable => return None,
            Class::Kana => kana_seen = true,
            Class::Hangul => hangul_seen = true,
            Class::Han => han_seen = true,
            Class::Keep => {}
        }
    }
    // Nothing but Latin and punctuation: the text already files where a
    // person would look for it, and a sort name identical to the name is
    // a row with no information in it.
    if !kana_seen && !hangul_seen && !han_seen {
        return None;
    }
    // Hanja beside hangul. Nothing here reads a Han character as Korean,
    // and handing back its Mandarin or Japanese reading inside a Korean
    // title isn't a near miss, it's a different word.
    if hangul_seen && (han_seen || kana_seen) {
        return None;
    }
    let japanese = kana_seen || reading == Reading::Japanese;

    let route = |c: char| match class(c) {
        Class::Keep => Route::Keep,
        Class::Kana => Route::Japanese,
        Class::Hangul => Route::Hangul,
        Class::Han if japanese => Route::Japanese,
        Class::Han => Route::Han,
        // Ruled out above, before any of this runs.
        Class::Unreadable => Route::Keep,
    };

    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let here = route(chars[i]);
        if here == Route::Keep {
            keep(chars[i], &mut out);
            i += 1;
            continue;
        }
        // One run of one script at a time. Kana and kanji share a run
        // deliberately: 君の名は only segments correctly as a whole.
        let start = i;
        while i < chars.len() && route(chars[i]) == here {
            i += 1;
        }
        let run: String = chars[start..i].iter().collect();
        let read = match here {
            Route::Hangul => hangul::romanize(&run, &mut out),
            Route::Han => han::romanize(&run, &mut out),
            Route::Japanese => match ja {
                Some(ja) => ja.read(&run, &mut out),
                // No dictionary: kana is still a table, kanji isn't.
                None => kana::romaji(&run, &mut out),
            },
            Route::Keep => unreachable!("a keep run is handled above"),
        };
        if !read {
            return None;
        }
    }

    let out = sentence_case(out.trim());
    (!out.is_empty()).then_some(out)
}

/// How this crate spells its answers, bumped whenever the shape of a
/// reading changes (spacing, particles, casing). The pass stores the number
/// beside each row it writes, so a build that reads differently knows which
/// of its own earlier answers to redo, and never touches a person's or a
/// service's.
pub const VERSION: u32 = 3;

/// Capitalise the first letter, the way a romanized title is written:
/// "Aki no kaze", "Seotaeji". Only the first, since particles stay
/// lowercase and a per-word rule would need that exception anyway. A
/// text that opens with a digit or a bracket keeps it and the first
/// letter after it is left alone, which matches how "(Live)" reads.
fn sentence_case(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) if first.is_lowercase() => {
            let mut out: String = first.to_uppercase().collect();
            out.push_str(chars.as_str());
            out
        }
        _ => text.to_string(),
    }
}

/// Copy a pass-through character, folding the fullwidth and CJK forms to
/// the ASCII they stand for.
fn keep(c: char, out: &mut String) {
    if let Some(ascii) = fold_fullwidth(c) {
        out.push(ascii);
    } else if let Some(ascii) = fold_punctuation(c) {
        out.push_str(ascii);
    } else {
        out.push(c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latin_text_has_nothing_to_add() {
        assert_eq!(romanize("Lemon", None), None);
        assert_eq!(romanize("Sigur Rós", None), None);
        assert_eq!(romanize("", None), None);
        assert_eq!(romanize("!?（）", None), None);
    }

    #[test]
    fn a_script_this_doesnt_read_answers_nothing() {
        assert_eq!(romanize("Мумий Тролль", None), None);
        assert_eq!(romanize("Ελλάδα", None), None);
        // Halfwidth katakana, which the kana table refuses on purpose.
        assert_eq!(romanize("ｻｸﾗ", None), None);
        // Hanja beside hangul: no table here reads Han as Korean.
        assert_eq!(romanize("서울 東大門", None), None);
    }

    #[test]
    fn hangul_and_chinese_need_no_download() {
        assert_eq!(romanize("서태지", None).unwrap(), "Seotaeji");
        assert_eq!(romanize("邓丽君", None).unwrap(), "Deng li jun");
        // Kana alone is a table too, so it answers on a fresh install.
        assert_eq!(romanize("レモン", None).unwrap(), "Remon");
        assert_eq!(romanize("ひとりごと", None).unwrap(), "Hitorigoto");
    }

    #[test]
    fn kanji_without_a_dictionary_answers_nothing() {
        // Not a wrong Chinese reading of Japanese text, which is the
        // failure this refusal exists to avoid.
        assert_eq!(romanize("君の名は", None), None);
        assert_eq!(romanize_as("東京", None, Reading::Japanese), None);
    }

    #[test]
    fn latin_runs_and_punctuation_survive_the_trip() {
        assert_eq!(romanize("Lemon (レモン)", None).unwrap(), "Lemon (remon)");
        assert_eq!(romanize("レモン・ツリー", None).unwrap(), "Remon tsurii");
        // Fullwidth forms fold to the ASCII they stand for.
        assert_eq!(romanize("レモン！", None).unwrap(), "Remon!");
        assert_eq!(romanize("ＡＢＣさん", None).unwrap(), "ABCsan");
    }

    #[test]
    fn a_kanji_title_is_the_one_case_that_needs_the_download() {
        assert!(needs_dictionary("君の名は", Reading::Auto));
        // Han with no kana anywhere reads as Chinese, which needs nothing.
        // 東京 is the same two characters in both languages, so nothing in
        // the text itself says which one wrote it.
        assert!(!needs_dictionary("東京", Reading::Auto));
        // Unless the caller says otherwise, having seen the rest of the row.
        assert!(needs_dictionary("東京", Reading::Japanese));
        assert!(!needs_dictionary("レモン", Reading::Auto));
        assert!(!needs_dictionary("서태지", Reading::Auto));
    }

    #[test]
    fn the_japanese_hint_only_moves_bare_han() {
        // No dictionary, so the hint's only visible effect here is to stop
        // Chinese being the answer.
        assert_eq!(romanize("北京", None).unwrap(), "Bei jing");
        assert_eq!(romanize_as("北京", None, Reading::Japanese), None);
        // Kana settles it without any hint at all.
        assert_eq!(romanize_as("レモン", None, Reading::Auto).unwrap(), "Remon");
    }

    #[test]
    fn kana_says_which_language_a_row_is_in() {
        assert!(has_kana("君の名は"));
        assert!(has_kana("レモン"));
        assert!(!has_kana("東京"));
        assert!(!has_kana("Lemon"));
    }

    /// The shared dictionary answers the same thing every time it's
    /// asked, and answers nothing at all without a download. The
    /// installed case is the ignored test below: this one has to pass on
    /// a machine that has never downloaded anything, which is every CI
    /// runner.
    #[test]
    fn the_shared_dictionary_is_stable_across_calls() {
        if dictionary::IPADIC.installed() {
            // The installed half is covered by the ignored test. Asserting
            // None here would fail on Andrew's own machine.
            assert!(japanese().is_some());
        } else {
            assert!(japanese().is_none());
        }
        let first = japanese().map(std::ptr::from_ref);
        let second = japanese().map(std::ptr::from_ref);
        assert_eq!(first, second);
        // A reload re-reads the models directory; with nothing installed
        // that lands on the same answer, and the pointer identity only
        // has to survive within a run.
        reload();
        assert_eq!(japanese().is_some(), first.is_some());
    }

    /// The dictionary-backed half, which is the only part of this crate
    /// that needs a download. Ignored unless IPADIC is installed at the
    /// models path: `cargo test` must never need the network, and there's
    /// no honest way to assert a kanji reading without the data that
    /// carries it. Install it from the Models settings page (or run
    /// `cargo test -p rox-romanize -- --ignored fetches`) and then
    /// `cargo test -p rox-romanize -- --ignored reads_kanji`.
    #[test]
    #[ignore = "needs the IPADIC download installed in the models directory"]
    fn reads_kanji_through_the_installed_dictionary() {
        assert!(
            dictionary::IPADIC.installed(),
            "install IPADIC from the Models page first"
        );
        let ja = Japanese::open().expect("the installed dictionary loads");
        let ja = Some(&ja);
        // Bare kanji needs the hint: the same two characters are a Chinese
        // city and a Japanese one, and without kana in the text nothing
        // but the caller knows which.
        assert_eq!(
            romanize_as("東京", ja, Reading::Japanese).unwrap(),
            "Toukyou"
        );
        assert_eq!(romanize("東京", ja).unwrap(), "Dong jing");
        // Kana in the text settles it without a hint, and the segmenter
        // gets the readings of the kanji around it right.
        assert_eq!(romanize("君の名は", ja).unwrap(), "Kimi no na wa");
        assert_eq!(romanize("夜に駆ける", ja).unwrap(), "Yoru ni kakeru");
        assert_eq!(romanize("Lemon (レモン)", ja).unwrap(), "Lemon (remon)");
        assert_eq!(
            romanize_as("打上花火", ja, Reading::Japanese).unwrap(),
            "Uchiagehanabi"
        );
        // The known failure, asserted rather than hidden: IPADIC has no
        // entry for this artist's name, so it segments into pieces and
        // reads them commonly. The right answer is "Yonezu Kenshi". This is
        // why artists keep MusicBrainz ahead of romanization and why the
        // tag editor overrides it.
        assert_eq!(
            romanize_as("米津玄師", ja, Reading::Japanese).unwrap(),
            "Yonetsu gen shi"
        );
        // The tables still answer with a dictionary loaded.
        assert_eq!(romanize("서태지", ja).unwrap(), "Seotaeji");
    }
}
