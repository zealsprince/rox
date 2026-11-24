//! One way in for a file joining the live workspace's assets (ADR 35): the
//! Workspace page's Add and the control socket's `assets.import` check,
//! stage and commit through here, so neither can take a file the other
//! refuses.

use std::path::Path;

use gpui::{App, SharedString};
use rox_core::settings::{Asset, AssetKind, assets};

pub(crate) enum Refusal {
    UnknownKind,
    TooManyFonts,
    FontTooLarge,
}

impl Refusal {
    /// For a toast.
    pub(crate) fn reason(&self) -> SharedString {
        match self {
            Refusal::UnknownKind => rox_i18n::t!("settings-workspace-assets-unknown-kind"),
            Refusal::TooManyFonts => rox_i18n::t!("settings-workspace-assets-too-many-fonts"),
            Refusal::FontTooLarge => rox_i18n::t!(
                "settings-workspace-assets-font-too-large",
                size = (crate::workspace_fonts::MAX_FONT_BYTES / 1_000_000).to_string()
            ),
        }
    }

    /// For a log line or a socket answer.
    pub(crate) fn english(&self) -> String {
        match self {
            Refusal::UnknownKind => "not an image or a font rox reads".to_string(),
            Refusal::TooManyFonts => format!(
                "the workspace already holds {} fonts",
                crate::workspace_fonts::MAX_FONTS
            ),
            Refusal::FontTooLarge => format!(
                "a font can be at most {} MB",
                crate::workspace_fonts::MAX_FONT_BYTES / 1_000_000
            ),
        }
    }
}

/// What the store would refuse anyway, caught before the copy starts so the
/// answer can say why. `file` is the name the asset is stored under.
pub(crate) fn refusal(path: &Path, file: &str) -> Option<Refusal> {
    let Some(kind) = assets::kind_for(file) else {
        return Some(Refusal::UnknownKind);
    };
    if kind != AssetKind::Font {
        return None;
    }

    let fonts = assets::all()
        .iter()
        .filter(|asset| asset.kind == AssetKind::Font)
        .count();
    if fonts >= crate::workspace_fonts::MAX_FONTS {
        return Some(Refusal::TooManyFonts);
    }

    let bytes = std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    if bytes > crate::workspace_fonts::MAX_FONT_BYTES {
        return Some(Refusal::FontTooLarge);
    }

    None
}

/// What [`assets::stage`] may copy for `file`. Checked on the bytes read, so
/// a font that grows after [`refusal`] looked at it still stops.
pub(crate) fn max_bytes(file: &str) -> u64 {
    match assets::kind_for(file) {
        Some(AssetKind::Font) => crate::workspace_fonts::MAX_FONT_BYTES,
        _ => u64::MAX,
    }
}

/// The quick half, on the UI thread. A font goes to the text system at once.
pub(crate) fn commit(staged: assets::Staged, cx: &mut App) -> Result<Asset, String> {
    let asset = assets::commit_staged(staged)?;
    if asset.kind == AssetKind::Font {
        crate::workspace_fonts::register(cx);
    }

    Ok(asset)
}
