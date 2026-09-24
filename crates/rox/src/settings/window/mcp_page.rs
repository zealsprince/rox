//! The MCP settings page (ADR 22): the switch that lets rox-mcp serve requests,
//! and the config snippet a client pastes, built for however this copy was
//! installed.

use super::*;

impl SettingsWindow {
    fn set_mcp_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        self.mcp_enabled = on;
        Settings::update(move |s| s.mcp_enabled = on);
        cx.notify();
    }

    /// Only in the sidebar while AI features are on, and off at its own toggle
    /// even then: revealing the page isn't opening the door.
    pub(super) fn mcp_page(
        &self,
        q: &Query,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PageBody {
        let snippet = mcp_config_snippet();
        // A TextView so the snippet can be selected in place.
        let block =
            TextView::markdown("mcp-config", format!("```json\n{snippet}\n```"), window, cx)
                .selectable(true)
                .text_xs();
        let toggle = panel::toggle(self.mcp_enabled, Self::set_mcp_enabled, cx);
        PageBody::new().section(Section::new(
            q,
            icons::LINK,
            rox_i18n::t!("settings-page-mcp"),
            // Copy only while the server is on.
            self.mcp_enabled.then(|| {
                small_button(
                    rox_i18n::t!("settings-common-copy"),
                    icons::COPY,
                    false,
                    move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(snippet.clone()));
                    },
                )
                .into_any_element()
            }),
            move |rows| {
                rows.keyed(
                    "settings-mcp-enable",
                    &["mcp", "enable", "server", "tools"],
                    toggle,
                )
                .custom(
                    &["mcp", "client", "config", "claude", "agent"],
                    move || {
                        div()
                            .flex()
                            .flex_col()
                            .gap(tokens::SPACE_XS)
                            .child(panel::setting_row(
                                rox_i18n::t!("settings-mcp-client-config"),
                                Some(rox_i18n::t!("settings-mcp-client-config.description")),
                                div().into_any_element(),
                            ))
                            .child(block)
                            .into_any_element()
                    },
                )
            },
        ))
    }
}

/// The client config in the mcpServers shape.
///
/// Flatpak runs `flatpak run --command=rox-mcp`: /app/bin is out of the host's
/// reach and the socket's runtime dir is only shared within the app id.
/// AppImage runs the .AppImage with `--mcp`, since its mount path changes every
/// launch. A portable run adds its data folder, which the socket is keyed to; a
/// Flatpak never runs portable.
fn mcp_config_snippet() -> String {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("rox"));
    let portable = settings::portable().then(settings::data_dir);
    mcp_config_for(
        rox_core::install::kind(),
        &exe,
        rox_core::install::appimage(),
        portable.as_deref(),
    )
}

/// Split out from the environment so the shapes can be tested.
fn mcp_config_for(
    kind: rox_core::install::Kind,
    exe: &Path,
    appimage: Option<&Path>,
    portable_data: Option<&Path>,
) -> String {
    use rox_core::install::{APP_ID_RDNS, Kind};

    let mut args: Vec<String> = Vec::new();
    let command = match (kind, appimage) {
        (Kind::Flatpak, _) => {
            args.extend(["run".into(), "--command=rox-mcp".into(), APP_ID_RDNS.into()]);
            "flatpak".to_string()
        }

        (Kind::AppImage, Some(image)) => {
            args.push("--mcp".into());
            image.display().to_string()
        }

        // Bare, or an AppImage that lost its own path.
        _ => {
            let binary = format!("rox-mcp{}", std::env::consts::EXE_SUFFIX);
            exe.parent()
                .map(|dir| dir.join(&binary).display().to_string())
                .unwrap_or(binary)
        }
    };

    // The portable pair goes after whatever the command needs first.
    if kind != Kind::Flatpak
        && let Some(data) = portable_data
    {
        args.extend(["--data-dir".into(), data.display().to_string()]);
    }

    let mut server = serde_json::json!({ "command": command });
    if !args.is_empty() {
        server["args"] = serde_json::json!(args);
    }
    let config = serde_json::json!({ "mcpServers": { "rox": server } });
    serde_json::to_string_pretty(&config).unwrap_or_default()
}

#[cfg(test)]
mod mcp_config_tests {
    use super::mcp_config_for;
    use rox_core::install::Kind;
    use std::path::Path;

    fn parsed(snippet: &str) -> serde_json::Value {
        serde_json::from_str(snippet).unwrap()
    }

    fn beside(dir: &str) -> String {
        format!("{dir}/rox-mcp{}", std::env::consts::EXE_SUFFIX)
    }

    #[test]
    fn bare_names_the_binary_beside_the_executable() {
        let snippet = mcp_config_for(Kind::Bare, Path::new("/opt/rox/rox"), None, None);
        assert_eq!(
            parsed(&snippet),
            serde_json::json!({ "mcpServers": { "rox": { "command": beside("/opt/rox") } } })
        );
    }

    #[test]
    fn bare_portable_adds_the_data_dir() {
        let snippet = mcp_config_for(
            Kind::Bare,
            Path::new("/media/usb/rox/rox"),
            None,
            Some(Path::new("/media/usb/rox/rox-data")),
        );
        assert_eq!(
            parsed(&snippet),
            serde_json::json!({ "mcpServers": { "rox": {
                "command": beside("/media/usb/rox"),
                "args": ["--data-dir", "/media/usb/rox/rox-data"],
            } } })
        );
    }

    /// A portable data dir can't happen in a Flatpak and is ignored.
    #[test]
    fn flatpak_runs_the_proxy_inside_the_sandbox() {
        let snippet = mcp_config_for(
            Kind::Flatpak,
            Path::new("/app/bin/rox"),
            None,
            Some(Path::new("/app/bin/rox-data")),
        );
        assert_eq!(
            parsed(&snippet),
            serde_json::json!({ "mcpServers": { "rox": {
                "command": "flatpak",
                "args": ["run", "--command=rox-mcp", "com.zealsprince.rox"],
            } } })
        );
    }

    #[test]
    fn appimage_goes_through_apprun() {
        let snippet = mcp_config_for(
            Kind::AppImage,
            Path::new("/tmp/.mount_rox1234/usr/bin/rox"),
            Some(Path::new("/home/me/Apps/rox.AppImage")),
            None,
        );
        assert_eq!(
            parsed(&snippet),
            serde_json::json!({ "mcpServers": { "rox": {
                "command": "/home/me/Apps/rox.AppImage",
                "args": ["--mcp"],
            } } })
        );
    }

    /// AppRun eats `--mcp` first, so the portable pair follows it.
    #[test]
    fn appimage_portable_keeps_mcp_first() {
        let snippet = mcp_config_for(
            Kind::AppImage,
            Path::new("/tmp/.mount_rox1234/usr/bin/rox"),
            Some(Path::new("/home/me/Apps/rox.AppImage")),
            Some(Path::new("/home/me/Apps/rox-data")),
        );
        assert_eq!(
            parsed(&snippet),
            serde_json::json!({ "mcpServers": { "rox": {
                "command": "/home/me/Apps/rox.AppImage",
                "args": ["--mcp", "--data-dir", "/home/me/Apps/rox-data"],
            } } })
        );
    }

    #[test]
    fn appimage_without_its_path_falls_back_to_bare() {
        let snippet = mcp_config_for(
            Kind::AppImage,
            Path::new("/tmp/.mount_rox1234/usr/bin/rox"),
            None,
            None,
        );
        assert_eq!(
            parsed(&snippet),
            serde_json::json!({ "mcpServers": { "rox": {
                "command": beside("/tmp/.mount_rox1234/usr/bin"),
            } } })
        );
    }
}
