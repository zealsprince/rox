//! The search matching key per ADR 6: case and accent folded, so "beyonce"
//! finds Beyoncé and "strasse" finds Straße.
//!
//! A deliberate copy of `rox_i18n::fold`, since this leaf crate can't depend
//! upward. Keep the two in agreement.
//!
//! Only ever a throwaway search key: tag values keep their accents
//! everywhere else.

use std::sync::OnceLock;

/// NFD with the combining marks dropped, and the sharp s spelled out as "ss".
pub fn fold(text: &str) -> String {
    // ASCII fast path: no marks, no sharp s, and this runs over every interned
    // value.
    if text.is_ascii() {
        return text.to_ascii_lowercase();
    }
    static NFD: OnceLock<icu_normalizer::DecomposingNormalizerBorrowed<'static>> = OnceLock::new();
    let nfd = NFD.get_or_init(icu_normalizer::DecomposingNormalizerBorrowed::new_nfd);
    // The string form, not per-char: only it lowercases a trailing sigma to its
    // final form.
    let lowered = text.to_lowercase().replace('ß', "ss");
    nfd.normalize(&lowered)
        .chars()
        .filter(|c| {
            !icu_properties::CodePointMapData::<icu_properties::props::GeneralCategory>::new()
                .get(*c)
                .eq(&icu_properties::props::GeneralCategory::NonspacingMark)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::fold;

    #[test]
    fn accents_fold_to_their_base_letter() {
        assert_eq!(fold("Beyoncé"), "beyonce");
        assert_eq!(fold("Émilie Simon"), "emilie simon");
        assert_eq!(fold("Sigur Rós"), "sigur ros");
    }

    #[test]
    fn sharp_s_spells_itself_out() {
        assert_eq!(fold("Straße"), "strasse");
    }

    #[test]
    fn plain_ascii_is_only_lowercased() {
        assert_eq!(fold("Daft Punk"), "daft punk");
        assert_eq!(fold("R.E.M."), "r.e.m.");
    }

    #[test]
    fn folding_is_idempotent() {
        for s in ["Beyoncé", "Straße", "米津玄師", "ΟΔΥΣΣΕΥΣ"] {
            assert_eq!(fold(&fold(s)), fold(s));
        }
    }

    #[test]
    fn a_script_without_marks_is_left_alone() {
        assert_eq!(fold("米津玄師"), "米津玄師");
    }
}
