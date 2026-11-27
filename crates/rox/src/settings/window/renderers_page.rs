//! The Renderers settings page (ADR 37 as amended 2026-11-26): every
//! renderer in the packages folder with its switch, version, standing and
//! failure, Developer mode, Pack and Remove. A renderer has a switch of its
//! own and none above it: Enable Plugins on the Plugins page doesn't reach
//! it.
//!
//! Underneath a renderer is the plugins' package, in the same folder, so the
//! rows, the install and the enable card are the Plugins page's own code.
//! Nothing on this page calls a renderer a plugin.

use super::plugins_page::{
    PROGRAM_FOLDERS_KEY, PROGRAM_FOLDERS_KEYWORDS, ProgramFolders, package_controls,
};
use super::*;

/// The renderer guide as of this build's release, as the plugin guide is.
const GUIDE_URL: &str = concat!(
    "https://github.com/zealsprince/rox/blob/v",
    env!("CARGO_PKG_VERSION"),
    "/docs/03-implementation/11-renderers.md"
);

impl SettingsWindow {
    pub(super) fn renderers_page(&self, q: &Query, cx: &mut Context<Self>) -> PageBody {
        let controls = package_controls(
            "renderers",
            rox_i18n::t!("settings-plugins-renderers-guide"),
            GUIDE_URL,
        );

        let (rows, missing) = self.package_rows(true, cx);
        let empty = rows.is_empty();

        let mut keywords = vec![
            "renderer",
            "render",
            "target",
            "visual",
            "folder",
            "approve",
            "developer",
        ];
        keywords.extend(
            self.plugin_page
                .folders
                .iter()
                .filter(|f| plugins_page::is_renderer(f))
                .map(|f| f.id.as_str()),
        );
        keywords.extend(missing.iter().copied());

        let intro = div()
            .text_xs()
            .text_color(palette::text_muted())
            .child(rox_i18n::t!("settings-plugins-renderers-intro"));

        // What every renderer runs as here, said once over the list. The
        // rows mark only what differs, and the card holds the full lines.
        let support = rox_plugins::sandbox::support();
        let lock = match support {
            rox_plugins::sandbox::Support::Sandbox { .. } => icons::LOCK,
            rox_plugins::sandbox::Support::Unavailable(_) => icons::LOCK_OPEN,
        };
        let sandbox = div()
            .flex()
            .flex_row()
            .items_start()
            .gap(tokens::SPACE_SM)
            .py(tokens::SPACE_XS)
            .text_xs()
            .text_color(palette::text_muted())
            .child(
                svg()
                    .path(lock)
                    .size(px(14.))
                    .flex_none()
                    .text_color(palette::text_muted()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(plugins_page::sandbox_line(&support)),
            );

        let table = div()
            .flex()
            .flex_col()
            .child(sandbox)
            .children(rows)
            .when(empty, |d| {
                d.child(
                    div()
                        .py(tokens::SPACE_XS)
                        .text_color(palette::text_muted())
                        .child(rox_i18n::t!("settings-plugins-renderers-empty")),
                )
            });

        PageBody::new().section(Section::new(
            q,
            icons::MONITOR_PLAY,
            rox_i18n::t!("settings-page-renderers"),
            Some(controls.into_any_element()),
            |rows| {
                // A renderer's interpreter is looked up in these folders
                // too, with plugins off as well as on.
                rows.keyed(
                    PROGRAM_FOLDERS_KEY,
                    PROGRAM_FOLDERS_KEYWORDS,
                    ProgramFolders::field(&self.program_folders.renderers),
                )
                .custom(&keywords, || intro.into_any_element())
                .custom(&keywords, || table.into_any_element())
            },
        ))
    }
}
