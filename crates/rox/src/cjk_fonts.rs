//! Whether this machine has the fonts to draw the library's CJK text
//! quickly, for the warning under the Appearance page's font row.
//!
//! On Linux, cosmic-text falls back to the Noto Sans CJK family for the
//! script but only takes a face at exactly the requested weight. The
//! variable Noto Sans CJK that NixOS and Fedora ship lists itself at weight
//! 100, so it never matches, and cosmic-text reshapes against every installed
//! face until one covers the text. Measured on 3,800 faces: about 2.5ms per
//! library cell against 85µs for Latin, enough to blow a scroll frame.
//!
//! rox can't fix it: gpui draws a variable font at its default instance, so
//! forcing the match would draw Thin. The static fonts are the fix; this
//! only reports when they're missing, mirroring cosmic-text 0.14's rule.
//! macOS and Windows fall back through the OS and never hit this.

use rox_library::projection::Projection;
use rox_romanize::CjkScripts;

/// Stops once all three scripts turn up.
pub fn library_scripts(projection: &Projection) -> CjkScripts {
    let mut scripts = CjkScripts::default();

    // The interned tables answer for most libraries before any title is read.
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

/// cosmic-text 0.14's Han family, matched on the whole locale string: "ja"
/// gets Japanese forms, "ja-JP" gets Simplified Chinese.
fn han_family(locale: &str) -> &'static str {
    match locale {
        "ja" => "Noto Sans CJK JP",
        "ko" => "Noto Sans CJK KR",
        "zh-HK" => "Noto Sans CJK HK",
        "zh-TW" => "Noto Sans CJK TC",
        _ => "Noto Sans CJK SC",
    }
}

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

#[cfg(target_os = "linux")]
pub fn fallback_missing(scripts: CjkScripts) -> bool {
    use fontdb::{Database, Stretch, Style, Weight};

    if !scripts.any() {
        return false;
    }

    // cosmic-text's own locale read and default.
    let locale = sys_locale::get_locale().unwrap_or_else(|| "en-US".to_owned());
    let wanted = families(scripts, &locale);

    let mut db = Database::new();
    db.load_system_fonts();

    // What cosmic-text accepts for a regular request: upright, normal width, weight 400 exactly.
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
