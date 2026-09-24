//! The language picker over rox-i18n's registry, shared by the settings
//! window and the welcome tour. Persisting the pick is each host's apply.

use std::sync::{Arc, Mutex};

use gpui::{Context, SharedString, prelude::*};

use crate::search_picker::{PickRow, search_picker};

/// `current` is a registry id, None following the OS. An id the registry no
/// longer has reads as System.
// `use<..>` and the named `A` for the same reason as the crate root's
// `picker`.
pub fn language_picker<P, A>(
    id: &'static str,
    current: Option<String>,
    apply: A,
    cx: &mut Context<P>,
) -> impl IntoElement + use<P, A>
where
    P: 'static,
    A: Fn(&mut P, Option<String>, &mut Context<P>) + 'static,
{
    let rows = rows();
    let current: Option<SharedString> = current.and_then(|stored| {
        rox_i18n::LOCALES
            .iter()
            .find(|loc| loc.id == stored)
            .map(|loc| loc.id.into())
    });
    let label = rows
        .iter()
        .find(|row| row.value == current)
        .map(|row| row.label.clone())
        .unwrap_or_else(|| rox_i18n::t!("settings-language-system"));
    search_picker(
        id,
        rows,
        label,
        current,
        rox_i18n::t!("settings-language-search"),
        rox_i18n::t!("picker-no-matches"),
        apply,
        cx,
    )
}

/// Native names stay untranslated so a reader lost in the wrong language
/// can find their own. Only the System head is localized, so the rows are
/// cached per locale; the stable Arc is the picker's changed-or-not check.
fn rows() -> Arc<Vec<PickRow>> {
    type Cached = Option<(&'static str, Arc<Vec<PickRow>>)>;
    static ROWS: Mutex<Cached> = Mutex::new(None);
    let locale = rox_i18n::locale();
    let mut cached = ROWS.lock().unwrap();
    if let Some((at, rows)) = cached.as_ref()
        && *at == locale
    {
        return rows.clone();
    }
    let mut built = vec![PickRow {
        label: rox_i18n::t!("settings-language-system"),
        value: None,
        terms: Vec::new(),
        icon: None,
    }];
    built.extend(rox_i18n::LOCALES.iter().map(|loc| PickRow {
        label: format!("{} {}", loc.flag, loc.native).into(),
        value: Some(loc.id.into()),
        terms: loc.aliases.iter().map(|alias| (*alias).into()).collect(),
        icon: None,
    }));
    let rows = Arc::new(built);
    *cached = Some((locale, rows.clone()));
    rows
}
