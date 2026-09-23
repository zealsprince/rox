//! The Application settings page: how the app itself behaves, from the AI
//! gate through launch, window residency, updates, and where the data is
//! kept. The portable switch's folder copy lives here with it.

use super::*;

impl SettingsWindow {
    /// The quit-to-tray switch, the Window menu toggle's twin: flips the
    /// live flag the close path reads, persists, and puts the tray icon up
    /// or takes it down on the spot. The toggle reads the static, not a
    /// cached field, so the two entry points never show different states.
    fn set_quit_to_tray(&mut self, on: bool, cx: &mut Context<Self>) {
        settings::set_quit_to_tray(on);
        Settings::update(move |s| s.quit_to_tray = on);
        tray::sync(cx);
        cx.notify();
    }

    /// The portable switch. On creates rox-data beside the executable,
    /// seeds it from the current data folder when it's new, and drops
    /// the marker file launch checks for; off removes the marker and
    /// leaves rox-data where it is. Going back doesn't migrate; that
    /// data is the user's to keep or delete. Either way the running app
    /// stays on the folder it started with.
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
            // A rox-data from an earlier portable stint: reuse it rather
            // than overwrite it with the current state.
            let _ = std::fs::write(&marker, b"");
            self.portable = marker.exists();
            cx.notify();
            return;
        }
        // Seed rox-data from the live data folder off the UI thread (the
        // caches can be big) and only drop the marker once the copy
        // finishes, so a restart mid-copy never boots on a half folder. The
        // copy is best-effort over live databases, the same risk copying
        // the folder by hand takes; the restart requirement keeps the
        // window small.
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

    /// The resize-lock switch, the design-mode setter's shape: the live
    /// flag repaints every window's handles, and the file keeps it.
    fn set_resize_lock(&mut self, on: bool, cx: &mut Context<Self>) {
        settings::set_resize_lock(on, cx);
        Settings::update(move |s| s.resize_lock = on);
        cx.notify();
    }

    /// The Application page: how the app itself behaves, from the AI gate
    /// through launch, layout, window residency, where the data is kept,
    /// and the control socket under it all. Everything about how the music
    /// plays is on the Playback page instead.
    pub(super) fn application_page(&self, q: &Query, cx: &mut Context<Self>) -> PageBody {
        // The portable row's control depends on the state: inert text
        // where the exe folder can't take writes or while the seed copy
        // runs, the live switch otherwise.
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
        // The restart note keys on the marker not matching the run, not
        // on a flip this session: it stays up across window reopens
        // until a launch actually applies the change.
        if self.portable != settings::portable() && !self.portable_busy {
            portable_row = portable_row.child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!("settings-application-portable-restart-note")),
            );
        }
        PageBody::new()
            // At the head of the page rather than sorted in: it's the gate
            // for two whole pages (MCP, ML Models), and a gate that hides
            // below the fold is a setting people ask where to find.
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
                    // Which releases to take and whether to fetch them only
                    // mean something while something's checking.
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
                        // Meaningless where the install can't replace itself (a
                        // distro package, a read-only folder), so the row only
                        // exists where the updater can act on it.
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
            // A resident process with no way back in is worse than quitting,
            // so the row only exists where something can bring a window back.
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
            // Here rather than on the MCP page: the socket is rox's one
            // machine interface, and rox-mcp is just one of its callers.
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
                                    // A named pipe isn't in the filesystem,
                                    // so Windows has nothing to reveal.
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
                            // The path on its own line rather than squeezed
                            // beside the buttons: runtime dirs run long, and a
                            // readout that truncates is a readout that lies.
                            .child(readout(text))
                            .into_any_element()
                    })
                },
            ))
            // Only ever on an AppImage: every other channel registers itself
            // with the desktop, and a switch over an entry the package
            // manager owns would be a lie.
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

    /// The AppImage's menu entry, written or removed on the spot; the row
    /// reads the entry back on the next render. Turning it off counts as
    /// declining, so the welcome window stops offering it.
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

    /// The launch-check toggle: into the file, so the next start reads the
    /// new setting. This run is already past its launch check either way.
    fn set_check_updates(&mut self, on: bool, cx: &mut Context<Self>) {
        self.check_updates = on;
        Settings::update(move |s| s.check_updates = on);
        cx.notify();
    }

    /// The candidates toggle: into the file, and the menubar chip
    /// recomputed against it, so a candidate the last check cached shows
    /// or hides at once rather than after the next daily check.
    fn set_prerelease_updates(&mut self, on: bool, cx: &mut Context<Self>) {
        self.prerelease_updates = on;
        Settings::update(move |s| s.prerelease_updates = on);
        updates::refresh_available(&Settings::load());
        cx.refresh_windows();
        cx.notify();
    }

    /// The auto-download toggle, same shape: the next launch's check reads
    /// it.
    fn set_download_updates(&mut self, on: bool, cx: &mut Context<Self>) {
        self.download_updates = on;
        Settings::update(move |s| s.download_updates = on);
        cx.notify();
    }

    fn set_ai_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        self.ai_enabled = on;
        Settings::update(move |s| s.ai_enabled = on);
        // Turning it off takes the MCP and ML Models pages out of the
        // sidebar; a window on one of them goes back to the page with the
        // toggle rather than staying on an orphaned page.
        if !on && matches!(self.page, Page::Mcp | Page::MlModels) {
            self.page = Page::Application;
        }
        cx.notify();
    }
}

/// Copy a folder tree whole, files and subfolders. The portable seed:
/// stops on the first error so a half copy reports as one instead of
/// passing for done.
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
