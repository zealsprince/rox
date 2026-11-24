//! The font-family picker for the app settings window and every panel's
//! Appearance page, built on [`search_picker`](crate::search_picker).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use gpui::{App, Context, SharedString, prelude::*};

use crate::search_picker::{PickRow, search_picker};

/// A picker over the installed families, headed by a Default row that clears
/// the override. `current` None means inherit.
// `use<..>` and the named `A` for the same reason as the crate root's
// `picker`.
pub fn font_picker<P, A>(
    id: &'static str,
    current: Option<String>,
    apply: A,
    cx: &mut Context<P>,
) -> impl IntoElement + use<P, A>
where
    P: 'static,
    A: Fn(&mut P, Option<String>, &mut Context<P>) + 'static,
{
    // Show the stored family even when this machine lacks it; never clear
    // the override on the user's behalf.
    let label: SharedString = current
        .clone()
        .map(SharedString::from)
        .unwrap_or_else(|| rox_i18n::t!("picker-font-default"));
    search_picker(
        id,
        families(cx),
        label,
        current.map(SharedString::from),
        rox_i18n::t!("picker-font-search"),
        rox_i18n::t!("picker-no-matches"),
        apply,
        cx,
    )
}

static FONTS_REV: AtomicU64 = AtomicU64::new(0);

/// Fonts were added to the text system, so the next picker lists them.
pub fn fonts_changed() {
    FONTS_REV.fetch_add(1, Ordering::Relaxed);
}

/// The family rows as built for one font revision in one language.
struct Families {
    rev: u64,
    locale: &'static str,
    rows: Arc<Vec<PickRow>>,
}

/// Enumerated once per [`fonts_changed`] and language: the settings pages
/// render on every slider scrub, and the Default row is translated.
fn families(cx: &mut App) -> Arc<Vec<PickRow>> {
    static FONTS: Mutex<Option<Families>> = Mutex::new(None);

    let rev = FONTS_REV.load(Ordering::Relaxed);
    let locale = rox_i18n::locale();
    let mut cache = FONTS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(held) = cache.as_ref()
        && held.rev == rev
        && held.locale == locale
    {
        return held.rows.clone();
    }

    let mut names = cx.text_system().all_font_names();
    names.sort();
    names.dedup();

    let mut rows = vec![PickRow {
        label: rox_i18n::t!("picker-font-default"),
        value: None,
        terms: Vec::new(),
        icon: None,
    }];
    rows.extend(names.into_iter().map(|name| {
        let name = SharedString::from(name);
        PickRow {
            label: name.clone(),
            value: Some(name),
            terms: Vec::new(),
            icon: None,
        }
    }));

    let rows = Arc::new(rows);
    *cache = Some(Families {
        rev,
        locale,
        rows: rows.clone(),
    });

    rows
}
