//! The Application settings page: the AI gate, launch and updates, layout,
//! window residency, the data folder and the control socket.

use super::*;

impl SettingsWindow {
    /// Reads the static, so it matches the Window menu toggle.
    fn set_quit_to_tray(&mut self, on: bool, cx: &mut Context<Self>) {
        settings::set_quit_to_tray(on);
        Settings::update(move |s| s.quit_to_tray = on);
        tray::sync(cx);
        cx.notify();
    }

    /// On creates rox-data beside the executable, seeded from the current data
    /// folder when new, and drops the marker launch checks for. Off removes the
    /// marker and leaves rox-data alone. The running app stays on the folder it
    /// started with.
    fn set_portable(&mut self, on: bool, cx: &mut Context<Self>) {
        let (Some(marker), Some(portable_dir)) =
            (settings::portable_marker(), settings::portable_data_dir())
        else {
            return;
        };
        if !on {
            let _ = std::fs::remove_file(&marker);
            self.portable = marker.exists();
            cx.notify();
            return;
        }
        if portable_dir.exists() {
            // An earlier portable stint's rox-data is reused, not overwritten.
            let _ = std::fs::write(&marker, b"");
            self.portable = marker.exists();
            cx.notify();
            return;
        }
        // Drop the marker only after the copy finishes, so a restart mid-copy
        // never boots on a half folder. The copy is best-effort over live
        // databases.
        self.portable = true;
        self.portable_busy = true;
        let source = settings::data_dir();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move {
                    if copy_dir(&source, &portable_dir).is_ok() {
                        let _ = std::fs::write(&marker, b"");
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                this.portable_busy = false;
                this.portable = settings::portable_marker().is_some_and(|marker| marker.exists());
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn set_resize_lock(&mut self, on: bool, cx: &mut Context<Self>) {
        settings::set_resize_lock(on, cx);
        Settings::update(move |s| s.resize_lock = on);
        cx.notify();
    }

    pub(super) fn application_page(&self, q: &Query, cx: &mut Context<Self>) -> PageBody {
        let portable_control: AnyElement = if !self.portable_writable {
            readout(rox_i18n::t!("settings-application-portable-not-writable").to_string())
                .into_any_element()
        } else if self.portable_busy {
            readout(rox_i18n::t!("settings-application-portable-copying").to_string())
                .into_any_element()
        } else {
            panel::toggle(self.portable, Self::set_portable, cx).into_any_element()
        };
        let mut portable_row =
            div()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_XS)
                .child(panel::setting_row(
                    rox_i18n::t!("settings-application-portable-mode"),
                    Some(rox_i18n::t!(
                        "settings-application-portable-mode.description"
                    )),
                    portable_control,
                ));
        // Keys on the marker not matching this run, so the note survives window
        // reopens until a launch applies it.
        if self.portable != settings::portable() && !self.portable_busy {
            portable_row = portable_row.child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!("settings-application-portable-restart-note")),
            );
        }
        PageBody::new()
            // At the head of the page: it gates two whole pages.
            .section(Section::new(
                q,
                icons::LINK,
                rox_i18n::t!("settings-application-section-ai"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-application-enable-ai",
                        &["ai", "mcp", "agent", "assistant", "llm", "model"],
                        panel::toggle(self.ai_enabled, Self::set_ai_enabled, cx),
                    )
                },
            ))
            .section(Section::new(
                q,
                icons::PLAY,
                rox_i18n::t!("settings-application-section-startup"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-application-check-updates",
                        &["release", "version", "upgrade"],
                        panel::toggle(self.check_updates, Self::set_check_updates, cx),
                    )
                    .when(self.check_updates, |rows| {
                        rows.keyed(
                            "settings-application-prerelease-updates",
                            &[
                                "release",
                                "candidate",
                                "rc",
                                "prerelease",
                                "beta",
                                "preview",
                            ],
                            panel::toggle(
                                self.prerelease_updates,
                                Self::set_prerelease_updates,
                                cx,
                            ),
                        )
                        // Only where the install can replace itself.
                        .when(updater::can_update(), |rows| {
                            rows.keyed(
                                "settings-application-download-updates",
                                &["release", "download", "auto", "update"],
                                panel::toggle(
                                    self.download_updates,
                                    Self::set_download_updates,
                                    cx,
                                ),
                            )
                        })
                    })
                },
            ))
            .section(Section::new(
                q,
                icons::LAYOUT_DASHBOARD,
                rox_i18n::t!("settings-application-section-layout"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-application-lock-panel-resize",
                        &["resize", "lock", "design", "drag", "seam"],
                        panel::toggle(settings::resize_lock(), Self::set_resize_lock, cx),
                    )
                },
            ))
            // Only where something can bring a window back: a resident process
            // with no way in is worse than quitting.
            .when(tray::supported(), |page| {
                page.section(Section::new(
                    q,
                    icons::APP_WINDOW,
                    rox_i18n::t!("settings-application-section-window"),
                    None,
                    |rows| {
                        rows.keyed(
                            "settings-application-remain-in-tray",
                            &["quit", "minimize", "background"],
                            panel::toggle(settings::quit_to_tray(), Self::set_quit_to_tray, cx),
                        )
                    },
                ))
            })
            .section(Section::new(
                q,
                icons::DATABASE,
                rox_i18n::t!("settings-application-section-data"),
                None,
                |rows| {
                    rows.custom(&["portable mode", "usb", "folder", "executable"], || {
                        portable_row.into_any_element()
                    })
                },
            ))
            // Here rather than on the MCP page: rox-mcp is just one caller of
            // the socket.
            .section(Section::new(
                q,
                icons::LINK,
                rox_i18n::t!("settings-application-section-control-socket"),
                None,
                |rows| {
                    rows.custom(&["socket", "ipc", "control", "roxctl", "mcp"], || {
                        let path = rox_ipc::socket_path(&settings::data_dir());
                        let text = path.display().to_string();
                        let copy = text.clone();
                        div()
                            .flex()
                            .flex_col()
                            .gap(tokens::SPACE_XS)
                            .child(panel::setting_row(
                                rox_i18n::t!("settings-application-socket-path"),
                                Some(rox_i18n::t!("settings-application-socket-path.description")),
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(tokens::SPACE_SM)
                                    .child(small_button(
                                        rox_i18n::t!("settings-common-copy"),
                                        icons::COPY,
                                        false,
                                        move |_, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                copy.clone(),
                                            ));
                                        },
                                    ))
                                    .when(!cfg!(windows), |d| {
                                        d.child(small_button(
                                            rox_i18n::t!("settings-common-reveal"),
                                            icons::FOLDER,
                                            false,
                                            move |_, _, cx| {
                                                cx.reveal_path(&path);
                                            },
                                        ))
                                    })
                                    .into_any_element(),
                            ))
                            // On its own line: runtime dirs run long, and a truncated
                            // path is wrong.
                            .child(readout(text))
                            .into_any_element()
                    })
                },
            ))
            // AppImage only: every other channel registers its own desktop
            // entry.
            .when(rox_core::install::appimage().is_some(), |page| {
                page.section(Section::new(
                    q,
                    icons::APP_WINDOW,
                    rox_i18n::t!("settings-application-section-desktop"),
                    None,
                    |rows| {
                        rows.keyed(
                            "settings-application-menu-entry",
                            &[
                                "appimage",
                                "menu",
                                "launcher",
                                "desktop",
                                "integration",
                                "open with",
                            ],
                            panel::toggle(
                                matches!(
                                    crate::startup::desktop_integration::status(),
                                    crate::startup::desktop_integration::Status::Integrated { .. }
                                ),
                                Self::set_menu_entry,
                                cx,
                            ),
                        )
                    },
                ))
            })
    }

    /// Turning it off counts as declining, so the welcome window stops offering
    /// it.
    fn set_menu_entry(&mut self, on: bool, cx: &mut Context<Self>) {
        use crate::startup::desktop_integration;

        let outcome = if on {
            desktop_integration::install()
        } else {
            desktop_integration::remove()
        };
        if let Err(reason) = outcome {
            log::warn!("desktop entry: {reason}");
        }

        if !on {
            Settings::update(|s| s.session.appimage_menu_declined = true);
        }
        cx.notify();
    }

    fn set_check_updates(&mut self, on: bool, cx: &mut Context<Self>) {
        self.check_updates = on;
        Settings::update(move |s| s.check_updates = on);
        cx.notify();
    }

    /// Recompute the menubar chip now, so a cached candidate shows or hides at
    /// once.
    fn set_prerelease_updates(&mut self, on: bool, cx: &mut Context<Self>) {
        self.prerelease_updates = on;
        Settings::update(move |s| s.prerelease_updates = on);
        updates::refresh_available(&Settings::load());
        cx.refresh_windows();
        cx.notify();
    }

    fn set_download_updates(&mut self, on: bool, cx: &mut Context<Self>) {
        self.download_updates = on;
        Settings::update(move |s| s.download_updates = on);
        cx.notify();
    }

    fn set_ai_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        self.ai_enabled = on;
        Settings::update(move |s| s.ai_enabled = on);
        // Leave a page the toggle just hid.
        if !on && matches!(self.page, Page::Mcp | Page::MlModels) {
            self.page = Page::Application;
        }
        cx.notify();
    }
}

/// Stops on the first error, so a half copy reports as one.
fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}
