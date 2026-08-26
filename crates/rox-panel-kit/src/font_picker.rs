//! The font-family picker: the field the app settings window and every
//! panel's Appearance page drop their typeface list from. The machinery
//! is [`search_picker`](crate::search_picker::search_picker); what stays
//! here is the family list, enumerated once, and the Default head that
//! clears the override so the text falls back to whatever the layer
//! above sets.

use std::sync::{Arc, OnceLock};

use gpui::{prelude::*, App, Context, SharedString};

use crate::search_picker::{search_picker, PickRow};

/// The head of the list, the row that clears the override so the text
/// falls back to whatever the layer above sets.
const DEFAULT_LABEL: &str = "Default";

/// A font-family picker: the shared field over the installed families,
/// with a Default at the head that clears the override back to the app
/// font. `current` is the panel's stored family, None meaning inherit.
pub fn font_picker<P: 'static>(
    id: &'static str,
    current: Option<String>,
    apply: impl Fn(&mut P, Option<String>, &mut Context<P>) + 'static,
    cx: &mut Context<P>,
) -> impl IntoElement {
    // The stored family shows even when it isn't installed here, since
    // clearing someone's override because this machine lacks the font
    // would be the picker's doing, not theirs.
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

/// The installed families, Default at the head, enumerated and sorted
/// once and shared from there. They don't change over a session, and this
/// used to run on every settings render, slider scrubs included, where
/// re-listing and re-sorting every font each frame was pure waste.
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
            }];
            rows.extend(names.into_iter().map(|name| {
                let name = SharedString::from(name);
                PickRow {
                    label: name.clone(),
                    value: Some(name),
                    terms: Vec::new(),
                }
            }));
            Arc::new(rows)
        })
        .clone()
}
