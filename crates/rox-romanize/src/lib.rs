//! Latin spellings for text that has none.
//!
//! CJK titles rarely carry sort tags, and MusicBrainz only has sort names for
//! artists, so a track called レモン can't be found from a Latin keyboard.
//! This reads the characters and writes down what they say. It reopens the
//! sort-names contract's "no romanization library" line, because the
//! alternative left a fifth of a library unfindable.
//!
//! [`romanize`] returns None rather than guess when the text is already Latin,
//! carries a script it doesn't read (or mixes two it can't route), or needs
//! kanji readings with no dictionary installed: a Chinese reading of Japanese
//! is not a near miss.
//!
//! - **Hangul** is arithmetic ([`hangul`]). No data, never absent.
//! - **Han with no kana** reads as Mandarin from the `pinyin` table ([`han`]).
//! - **Anything with kana**, or Han the caller says is Japanese, goes through
//!   Lindera and IPADIC ([`japanese`]); kana alone works without it.
//! - **Latin and punctuation are kept**, fullwidth and CJK forms folded to
//!   ASCII.
//!
//! Every choice favours what someone would type into a search box over a
//! style guide: wapuro romaji, no apostrophe after n, no Revised Romanization
//! sound changes, no tone marks.

use std::sync::Mutex;

pub mod dictionary;
mod han;
mod hangul;
mod japanese;
mod kana;

pub use japanese::Japanese;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Reading {
    /// Han with no kana reads as Mandarin, right far more often than not.
    #[default]
    Auto,
    /// Kanji and hanzi are the same characters; the caller knows from the rest
    /// of the row.
    Japanese,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Route {
    Keep,
    Japanese,
    Hangul,
    Han,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Keep,
    Kana,
    Hangul,
    Han,
    Unreadable,
}

fn is_pass_through(c: char) -> bool {
    c.is_ascii()
        || matches!(c,
            '\u{00A0}'..='\u{02AF}'   // Latin-1 supplement through IPA extensions
            | '\u{0300}'..='\u{036F}' // combining marks, an accent's other half
            | '\u{1E00}'..='\u{1EFF}' // Latin extended additional
            | '\u{2000}'..='\u{2BFF}' // punctuation, currency, symbols, arrows
        )
}

/// Only marks with an unambiguous ASCII counterpart.
fn fold_punctuation(c: char) -> Option<&'static str> {
    // Fullwidth ASCII folds by arithmetic, in the caller.
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

/// The block at U+FF01 is U+0021 shifted up by 0xFEE0.
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

/// Kana is the one unambiguous sign a row is Japanese; the pass checks a
/// row's other fields with it to place a bare-kanji title.
pub fn has_kana(text: &str) -> bool {
    text.chars().any(kana::is_kana)
}

/// For the fonts check behind the Appearance page: each script falls back to
/// its own font family.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct CjkScripts {
    pub kana: bool,
    pub hangul: bool,
    pub han: bool,
}

impl CjkScripts {
    pub fn of(text: &str) -> Self {
        let mut scripts = Self::default();
        scripts.add(text);
        scripts
    }

    /// Plain ASCII, most of any library, skips the char walk.
    pub fn add(&mut self, text: &str) {
        if text.is_ascii() {
            return;
        }

        for c in text.chars() {
            match class(c) {
                Class::Kana => self.kana = true,
                Class::Hangul => self.hangul = true,
                Class::Han => self.han = true,
                Class::Keep | Class::Unreadable => {}
            }
        }
    }

    pub fn any(&self) -> bool {
        self.kana || self.hangul || self.han
    }

    pub fn all(&self) -> bool {
        self.kana && self.hangul && self.han
    }
}

/// Whether the text has Han routed to the Japanese reader. The pass asks up
/// front so it can refuse with a reason.
pub fn needs_dictionary(text: &str, reading: Reading) -> bool {
    let japanese = reading == Reading::Japanese || has_kana(text);
    japanese && text.chars().any(han::is_han)
}

/// None until asked, then the load's verdict, so a library with no dictionary
/// doesn't stat the models directory per row.
static LOADED: Mutex<Option<Option<&'static Japanese>>> = Mutex::new(None);

/// The process's one loaded dictionary. IPADIC is forty megabytes of mapped
/// tables and both the library pass and the metadata panel want it, so it's
/// loaded once and handed out by reference. [`reload`] picks up an install
/// or delete mid-session.
pub fn japanese() -> Option<&'static Japanese> {
    // A poisoned lock still holds a valid answer.
    let mut slot = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(loaded) = *slot {
        return loaded;
    }
    let loaded = open_installed();
    *slot = Some(loaded);
    loaded
}

/// Called by the settings page when a download finishes or a dictionary is
/// deleted. Dictionaries already handed out are leaked, not dropped: callers
/// hold `&'static` references, and one mapping per install is bounded.
pub fn reload() {
    *LOADED.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Leaked so the reference outlives every caller.
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
/// Without `ja`, kana, hangul and Chinese still answer; kanji doesn't.
pub fn romanize(text: &str, ja: Option<&Japanese>) -> Option<String> {
    romanize_as(text, ja, Reading::Auto)
}

/// [`romanize`], told what language the Han in the text is.
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
    // Nothing but Latin: a sort name identical to the name carries nothing.
    if !kana_seen && !hangul_seen && !han_seen {
        return None;
    }
    // Hanja beside hangul: nothing here reads Han as Korean.
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
        // Kana and kanji share a run: 君の名は only segments correctly whole.
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

/// Bumped whenever the shape of a reading changes. The pass stores it per
/// row, so a new build redoes its own old answers and never a person's or a
/// service's.
pub const VERSION: u32 = 3;

/// Capitalise the first letter only ("Aki no kaze"): particles stay
/// lowercase. A leading digit or bracket leaves the next letter alone.
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
        assert_eq!(romanize("ｻｸﾗ", None), None);
        assert_eq!(romanize("서울 東大門", None), None);
    }

    #[test]
    fn hangul_and_chinese_need_no_download() {
        assert_eq!(romanize("서태지", None).unwrap(), "Seotaeji");
        assert_eq!(romanize("邓丽君", None).unwrap(), "Deng li jun");
        assert_eq!(romanize("レモン", None).unwrap(), "Remon");
        assert_eq!(romanize("ひとりごと", None).unwrap(), "Hitorigoto");
    }

    #[test]
    fn kanji_without_a_dictionary_answers_nothing() {
        assert_eq!(romanize("君の名は", None), None);
        assert_eq!(romanize_as("東京", None, Reading::Japanese), None);
    }

    #[test]
    fn latin_runs_and_punctuation_survive_the_trip() {
        assert_eq!(romanize("Lemon (レモン)", None).unwrap(), "Lemon (remon)");
        assert_eq!(romanize("レモン・ツリー", None).unwrap(), "Remon tsurii");
        assert_eq!(romanize("レモン！", None).unwrap(), "Remon!");
        assert_eq!(romanize("ＡＢＣさん", None).unwrap(), "ABCsan");
    }

    #[test]
    fn a_kanji_title_is_the_one_case_that_needs_the_download() {
        assert!(needs_dictionary("君の名は", Reading::Auto));
        assert!(!needs_dictionary("東京", Reading::Auto));
        assert!(needs_dictionary("東京", Reading::Japanese));
        assert!(!needs_dictionary("レモン", Reading::Auto));
        assert!(!needs_dictionary("서태지", Reading::Auto));
    }

    #[test]
    fn the_japanese_hint_only_moves_bare_han() {
        assert_eq!(romanize("北京", None).unwrap(), "Bei jing");
        assert_eq!(romanize_as("北京", None, Reading::Japanese), None);
        assert_eq!(romanize_as("レモン", None, Reading::Auto).unwrap(), "Remon");
    }

    #[test]
    fn kana_says_which_language_a_row_is_in() {
        assert!(has_kana("君の名は"));
        assert!(has_kana("レモン"));
        assert!(!has_kana("東京"));
        assert!(!has_kana("Lemon"));
    }

    #[test]
    fn scripts_name_each_family_a_value_falls_back_to() {
        let mixed = CjkScripts::of("ドリームCastle 東京");
        assert!(mixed.kana && mixed.han && !mixed.hangul);

        let mut library = CjkScripts::of("Beyoncé");
        assert!(!library.any());

        library.add("서울");
        library.add("黄昏ホリック");
        assert!(library.all());
    }

    /// This half has to pass on a machine with no download, which is every CI
    /// runner.
    #[test]
    fn the_shared_dictionary_is_stable_across_calls() {
        if dictionary::IPADIC.installed() {
            assert!(japanese().is_some());
        } else {
            assert!(japanese().is_none());
        }
        let first = japanese().map(std::ptr::from_ref);
        let second = japanese().map(std::ptr::from_ref);
        assert_eq!(first, second);
        reload();
        assert_eq!(japanese().is_some(), first.is_some());
    }

    /// Ignored unless IPADIC is installed: `cargo test` never needs the
    /// network. Install from the Library settings page (or `--ignored fetches`),
    /// then run `cargo test -p rox-romanize -- --ignored reads_kanji`.
    #[test]
    #[ignore = "needs the IPADIC download installed in the models directory"]
    fn reads_kanji_through_the_installed_dictionary() {
        assert!(
            dictionary::IPADIC.installed(),
            "install IPADIC from the Models page first"
        );
        let ja = Japanese::open().expect("the installed dictionary loads");
        let ja = Some(&ja);
        assert_eq!(
            romanize_as("東京", ja, Reading::Japanese).unwrap(),
            "Toukyou"
        );
        assert_eq!(romanize("東京", ja).unwrap(), "Dong jing");
        assert_eq!(romanize("君の名は", ja).unwrap(), "Kimi no na wa");
        assert_eq!(romanize("夜に駆ける", ja).unwrap(), "Yoru ni kakeru");
        assert_eq!(romanize("Lemon (レモン)", ja).unwrap(), "Lemon (remon)");
        assert_eq!(
            romanize_as("打上花火", ja, Reading::Japanese).unwrap(),
            "Uchiagehanabi"
        );
        // Known failure: IPADIC lacks this name, so it reads the pieces
        // commonly (right is "Yonezu Kenshi"). Why MusicBrainz leads for artists.
        assert_eq!(
            romanize_as("米津玄師", ja, Reading::Japanese).unwrap(),
            "Yonetsu gen shi"
        );
        assert_eq!(romanize("서태지", ja).unwrap(), "Seotaeji");
    }
}
