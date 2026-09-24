//! The font-family picker for the app settings window and every panel's
//! Appearance page, built on [`search_picker`](crate::search_picker).

use std::sync::{Arc, OnceLock};

use gpui::{App, Context, SharedString, prelude::*};

use crate::search_picker::{PickRow, search_picker};

const DEFAULT_LABEL: &str = "Default";

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
        .unwrap_or_else(|| DEFAULT_LABEL.into());
    search_picker(
        id,
        families(cx),
        label,
        current.map(SharedString::from),
        "Search fonts".into(),
        "No matches".into(),
        apply,
        cx,
    )
}

/// Enumerated once: the settings pages render on every slider scrub.
fn families(cx: &mut App) -> Arc<Vec<PickRow>> {
    static FONTS: OnceLock<Arc<Vec<PickRow>>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut names = cx.text_system().all_font_names();
            names.sort();
            names.dedup();
            let mut rows = vec![PickRow {
                label: DEFAULT_LABEL.into(),
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
            Arc::new(rows)
        })
        .clone()
}
