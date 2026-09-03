//! The matching key search compares against, per ADR 6: case folded, and
//! since this module exists, accent folded too. Search was lowercase-only
//! for as long as the interesting question was "Beatles" against "beatles",
//! and it left "beyonce" unable to find Beyoncé, "strasse" unable to find
//! Straße, and every French, Spanish, and Vietnamese tag in a library
//! reachable only from a keyboard that can type it. People drop accents
//! constantly, out of habit and because the keys are awkward.
//!
//! `rox_i18n::fold` does exactly this for the command palette, over UI copy.
//! This is a second copy of it on purpose: rox-library is a leaf crate with
//! no sibling deps (it's the storage floor, and rox-i18n is a locale bundle
//! sitting above it), so reaching up for ten lines would buy an edge that
//! layering says shouldn't exist. The two are expected to agree; if one
//! changes, change the other.
//!
//! Nothing here is a normalization for storage or identity. Tag values keep
//! their accents everywhere they're displayed, interned, or compared for
//! equality; folding only ever produces the throwaway key a substring scan
//! runs over.

use std::sync::OnceLock;

/// Case and accent folded, the key both sides of a search comparison are
/// put through.
///
/// Decomposes to NFD and drops the combining marks, so an accented letter
/// falls back to its base. The German sharp s is spelled out first, since
/// it has no mark to strip and a keyboard without it produces "ss".
pub fn fold(text: &str) -> String {
    // Most of a Latin library is ASCII, which has no marks to strip and no
    // sharp s, so its fold is its lowercase. Worth the branch: this runs
    // over every interned value and every title in the projection, and the
    // slow path allocates three times to the fast path's one.
    if text.is_ascii() {
        return text.to_ascii_lowercase();
    }
    static NFD: OnceLock<icu_normalizer::DecomposingNormalizerBorrowed<'static>> = OnceLock::new();
    let nfd = NFD.get_or_init(icu_normalizer::DecomposingNormalizerBorrowed::new_nfd);
    // str::to_lowercase, not the per-char one: only the string form knows
    // that a trailing sigma lowercases to the final form, and query needles
    // fold through this same function, so both sides agree either way.
    let lowered = text.to_lowercase().replace('ß', "ss");
    nfd.normalize(&lowered)
        .chars()
        // Mn is the nonspacing-mark class every stripped accent falls into.
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

    /// The sharp s has no combining mark to strip, so it needs spelling
    /// out or "strasse" never finds "Straße".
    #[test]
    fn sharp_s_spells_itself_out() {
        assert_eq!(fold("Straße"), "strasse");
    }

    /// The ASCII shortcut has to land on the same answer as the long way
    /// around, or the fast path is a second folding rule.
    #[test]
    fn plain_ascii_is_only_lowercased() {
        assert_eq!(fold("Daft Punk"), "daft punk");
        assert_eq!(fold("R.E.M."), "r.e.m.");
    }

    /// A needle folded twice is the needle folded once: the projection
    /// folds its tables at build time and the query at parse time, and
    /// nothing tracks which strings have been through already.
    #[test]
    fn folding_is_idempotent() {
        for s in ["Beyoncé", "Straße", "米津玄師", "ΟΔΥΣΣΕΥΣ"] {
            assert_eq!(fold(&fold(s)), fold(s));
        }
    }

    /// Scripts with no case and no combining marks come out untouched, so
    /// a CJK or Cyrillic library matches exactly as it did before folding.
    #[test]
    fn a_script_without_marks_is_left_alone() {
        assert_eq!(fold("米津玄師"), "米津玄師");
    }
}
