//! Whether this machine has the fonts to draw the library's Japanese,
//! Chinese and Korean text quickly, for the warning under the Appearance
//! page's font row.
//!
//! On Linux gpui shapes text with cosmic-text, and cosmic-text finds a
//! glyph the UI font doesn't have by trying the Noto Sans CJK family named
//! for that script. It only takes a face at exactly the weight asked for,
//! though. The variable Noto Sans CJK that NixOS and Fedora install by
//! default lists itself at weight 100, its Thin default, so it never
//! matches a regular-weight request. cosmic-text then tries every installed
//! face in turn, shaping the text again with each one, until one happens to
//! cover it. Measured on a desktop with 3,800 font faces, that's about
//! 2.5ms per library cell against 85µs for a Latin one, and a scroll step
//! that brings in a few Japanese rows blows the frame. The face it lands on
//! there is GNU Unifont.
//!
//! rox can't fix this from its side. gpui draws a variable font at its
//! default instance, so forcing the variable face to match would draw the
//! text Thin. The static fonts carry a real Regular, which is the fix, and
//! all this module does is say when it's missing.
//!
//! The check mirrors cosmic-text 0.14's rule rather than asking gpui, which
//! has no way to report which fallback it used: the same family per
//! script, the same exact-weight match. It reads its own copy of the font
//! database, so it runs off the UI thread. macOS and Windows fall back
//! through the OS and never hit this.

use rox_library::projection::Projection;
use rox_romanize::CjkScripts;

/// Every script the library's titles and names are written in. Stops
/// once all three turn up, so a big CJK library doesn't walk every row.
pub fn library_scripts(projection: &Projection) -> CjkScripts {
    let mut scripts = CjkScripts::default();

    // The interned tables first: one entry per distinct name, so they
    // answer for most libraries before the titles are touched.
    for table in [
        &projection.artists,
        &projection.album_artists,
        &projection.albums,
    ] {
        for name in &table.strings {
            scripts.add(name);
            if scripts.all() {
                return scripts;
            }
        }
    }

    for row in 0..projection.len() {
        if projection.is_dead(row as u32) {
            continue;
        }

        scripts.add(projection.title.get(row));
        if scripts.all() {
            break;
        }
    }

    scripts
}

/// Which Noto Sans CJK family cosmic-text 0.14 falls back to for Han, by
/// the system locale string exactly as it matches it. So "ja" gets the
/// Japanese forms, but "ja-JP" and every non-CJK locale get Simplified
/// Chinese.
fn han_family(locale: &str) -> &'static str {
    match locale {
        "ja" => "Noto Sans CJK JP",
        "ko" => "Noto Sans CJK KR",
        "zh-HK" => "Noto Sans CJK HK",
        "zh-TW" => "Noto Sans CJK TC",
        _ => "Noto Sans CJK SC",
    }
}

/// The families these scripts fall back to under this locale.
fn families(scripts: CjkScripts, locale: &str) -> Vec<&'static str> {
    let mut families = Vec::new();

    if scripts.kana {
        families.push("Noto Sans CJK JP");
    }
    if scripts.hangul {
        families.push("Noto Sans CJK KR");
    }
    if scripts.han {
        families.push(han_family(locale));
    }

    families.sort_unstable();
    families.dedup();
    families
}

/// Whether some of the library's text will fall back slowly on this
/// machine. False when the library has no CJK text at all, since then
/// there's nothing to warn about.
#[cfg(target_os = "linux")]
pub fn fallback_missing(scripts: CjkScripts) -> bool {
    use fontdb::{Database, Stretch, Style, Weight};

    if !scripts.any() {
        return false;
    }

    // cosmic-text reads the locale the same way and makes the same
    // default when there isn't one.
    let locale = sys_locale::get_locale().unwrap_or_else(|| "en-US".to_owned());
    let wanted = families(scripts, &locale);

    let mut db = Database::new();
    db.load_system_fonts();

    // A face cosmic-text will accept for a regular-weight request: the
    // family, upright, normal width, weight 400 on the nose.
    let usable = |family: &str| {
        db.faces().any(|face| {
            face.weight == Weight::NORMAL
                && face.style == Style::Normal
                && face.stretch == Stretch::Normal
                && face.families.iter().any(|(name, _)| name == family)
        })
    };

    !wanted.iter().all(|family| usable(family))
}

#[cfg(not(target_os = "linux"))]
pub fn fallback_missing(_: CjkScripts) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_script_asks_for_its_own_family() {
        let scripts = CjkScripts::of("ドリーム 서울 東京");
        assert_eq!(
            families(scripts, "en-US"),
            ["Noto Sans CJK JP", "Noto Sans CJK KR", "Noto Sans CJK SC"]
        );
    }

    /// cosmic-text matches the locale string whole, so a regional
    /// Japanese locale still reads bare kanji as Simplified Chinese.
    #[test]
    fn han_follows_the_locale_string_exactly() {
        let han = CjkScripts::of("東京");
        assert_eq!(families(han, "ja"), ["Noto Sans CJK JP"]);
        assert_eq!(families(han, "ja-JP"), ["Noto Sans CJK SC"]);
        assert_eq!(families(han, "zh-TW"), ["Noto Sans CJK TC"]);
    }

    #[test]
    fn a_latin_library_needs_nothing() {
        assert!(families(CjkScripts::of("Daft Punk"), "en-US").is_empty());
        assert!(!fallback_missing(CjkScripts::default()));
    }
}
