//! The Library settings page: the sources table, the library switches, the
//! acoustic and tempo passes (`analysis`), the kanji dictionary and the embed
//! catch-up. Subsonic rows come from `subsonic`.

use super::*;

impl SettingsWindow {
    fn set_watch_library(&mut self, on: bool, cx: &mut Context<Self>) {
        self.watch_library = on;
        self.library
            .update(cx, |library, cx| library.set_watch(on, cx));
        cx.notify();
    }

    /// A read-model rebuild: the database never changes.
    fn set_fold_case(&mut self, on: bool, cx: &mut Context<Self>) {
        self.fold_case = on;
        settings::set_fold_case(on);
        Settings::update(move |s| s.fold_case = on);
        self.library
            .update(cx, |library, cx| library.reload_projection(cx));
        cx.notify();
    }

    /// Files and the database never change; matching splits stored strings at
    /// read.
    fn set_split_genre_compounds(&mut self, on: bool, cx: &mut Context<Self>) {
        self.split_genre_compounds = on;
        rox_library::genre::set_split_compounds(on);
        Settings::update(move |s| s.split_genre_compounds = on);
        self.library
            .update(cx, |library, cx| library.reload_projection(cx));
        cx.notify();
    }

    /// Nothing rebuilds: the sort names are already in the projection. Reads
    /// the static, so it can't drift from the panels.
    fn set_show_readings(&mut self, on: bool, cx: &mut Context<Self>) {
        settings::set_show_readings(on, cx);
        Settings::update(move |s| s.show_readings = on);
        cx.notify();
    }

    /// A pattern the glob parser can't read is refused here with its reason, so
    /// the scan never has to skip one.
    pub(super) fn add_exclusion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pattern = self.exclude_input.read(cx).value().trim().to_string();
        if pattern.is_empty() {
            return;
        }

        if let Err(reason) = rox_library::exclude::check(&pattern) {
            self.exclude_notice = Some(rox_i18n::t!(
                "settings-library-exclude-invalid",
                reason = reason
            ));
            cx.notify();
            return;
        }

        if !self.library_exclude.contains(&pattern) {
            self.library_exclude.push(pattern);
            self.write_exclusions(cx);
        }
        self.exclude_notice = None;
        self.exclude_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    fn remove_exclusion(&mut self, pattern: &str, cx: &mut Context<Self>) {
        self.library_exclude.retain(|p| p != pattern);
        self.write_exclusions(cx);
        cx.notify();
    }

    fn write_exclusions(&self, cx: &mut Context<Self>) {
        let patterns = self.library_exclude.clone();
        self.library
            .update(cx, |library, _| library.set_exclusions(patterns));
    }

    fn exclusion_row(&self, pattern: &str, cx: &mut Context<Self>) -> Stateful<Div> {
        let remove = icon_button(icons::CLOSE, false, {
            let pattern = pattern.to_string();
            cx.listener(move |this, _, _, cx| this.remove_exclusion(&pattern, cx))
        });

        div()
            // Prefixed so a pattern spelling a folder path can't share an id
            // with that folder's row.
            .id(ElementId::Name(format!("exclude:{pattern}").into()))
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_MD)
            .py(tokens::SPACE_XS)
            .border_b_1()
            .border_color(palette::border())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(pattern.to_string()),
            )
            .child(action_cell(remove))
    }

    fn exclusion_table(&self, cx: &mut Context<Self>) -> Div {
        let mut table = div().flex().flex_col().child(
            div()
                .pb(tokens::SPACE_XS)
                .border_b_1()
                .border_color(palette::border())
                .text_xs()
                .text_color(palette::text_muted())
                .child(rox_i18n::t!("settings-library-exclude-col")),
        );
        if self.library_exclude.is_empty() {
            table = table.child(
                div()
                    .py(tokens::SPACE_XS)
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!("settings-library-exclude-none")),
            );
        }
        for pattern in &self.library_exclude {
            table = table.child(self.exclusion_row(pattern, cx));
        }

        let add = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Input::new(&self.exclude_input)),
            )
            .child(icon_button(
                icons::PLUS,
                false,
                cx.listener(|this, _, window, cx| this.add_exclusion(window, cx)),
            ));

        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!("settings-library-exclude-intro")),
            )
            .child(table)
            .child(add)
            .when_some(self.exclude_notice.clone(), |d, notice| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(palette::tone_warn())
                        .child(notice),
                )
            })
            // The watcher follows a change at once, but rows an earlier scan
            // put in stay until the next one.
            .when(self.library_exclude != self.library_exclude_scanned, |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(rox_i18n::t!("settings-library-exclude-nudge")),
                )
            })
    }

    fn folder_row(
        &self,
        root: &Path,
        stats: Stats,
        scanning: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let path: SharedString = root.to_string_lossy().into_owned().into();
        let remove = icon_button(icons::CLOSE, scanning, {
            let root = root.to_path_buf();
            cx.listener(move |this, _, _, cx| {
                this.library
                    .update(cx, |library, cx| library.remove_root(&root, cx));
            })
        });
        div()
            // Named after the folder so its remove button is its own. See
            // `rox_panel_kit::ui::control_focus`.
            .id(ElementId::Name(path.clone()))
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_MD)
            .py(tokens::SPACE_XS)
            .border_b_1()
            .border_color(palette::border())
            .child(div().flex_1().min_w_0().truncate().child(path))
            .child(number_cell(TRACKS_COL_W, stats.tracks.to_string()))
            .child(number_cell(ALBUMS_COL_W, stats.albums.to_string()))
            .child(number_cell(SIZE_COL_W, human_size(stats.bytes)))
            .child(action_cell(remove))
    }

    pub(super) fn library_page(&self, q: &Query, cx: &mut Context<Self>) -> PageBody {
        let busy = self.library.read(cx).busy();
        let scanning = busy.is_some();
        // Past the platform's watch ceiling the toggle locks off and the note
        // says why. The roots never nest, so summing their rollups counts
        // nothing twice.
        let dirs = self.root_stats.iter().map(|(_, s)| s.dirs).sum::<u64>();
        let over_limit = rox_services::catalog::watch_limit_dirs().filter(|limit| dirs > *limit);
        let lead_in = div()
            .text_xs()
            .text_color(palette::text_muted())
            .child(rox_i18n::t!("settings-library-folders-intro"));
        // Stored genre lists keep their old shape until a rescan re-reads the
        // tags.
        let separators_moved = self.split_genre_compounds != self.split_genre_compounds_scanned;
        let nudge = div()
            .text_xs()
            .text_color(palette::text_muted())
            .child(rox_i18n::t!("settings-library-genre-separator-nudge"));
        // One list for every kind of source, so a later source kind takes a row
        // here, not a section.
        let mut table = div().flex().flex_col().child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_MD)
                .pb(tokens::SPACE_XS)
                .border_b_1()
                .border_color(palette::border())
                .text_xs()
                .text_color(palette::text_muted())
                .child(
                    div()
                        .flex_1()
                        .child(rox_i18n::t!("settings-library-col-source")),
                )
                .child(
                    div()
                        .w(TRACKS_COL_W)
                        .flex_none()
                        .text_right()
                        .child(rox_i18n::t!("settings-library-folder-col-tracks")),
                )
                .child(
                    div()
                        .w(ALBUMS_COL_W)
                        .flex_none()
                        .text_right()
                        .child(rox_i18n::t!("settings-library-folder-col-albums")),
                )
                .child(
                    div()
                        .w(SIZE_COL_W)
                        .flex_none()
                        .text_right()
                        .child(rox_i18n::t!("settings-library-folder-col-size")),
                )
                .child(div().w(ACTION_COL_W).flex_none()),
        );
        let servers = self.subsonic_rows(cx);
        if self.root_stats.is_empty() && servers.is_empty() {
            table = table.child(
                div()
                    .py(tokens::SPACE_XS)
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!("settings-library-no-sources")),
            );
        }
        for (root, stats) in &self.root_stats {
            table = table.child(self.folder_row(root, *stats, scanning, cx));

            // Only inside a Flatpak: reads go through the document portal and
            // the watcher sees nothing. The callout carries the override that
            // grants the real folder.
            if rox_core::install::is_portal_path(root) {
                table = table.child(portal_banner(root));
            }
        }
        table = table.children(servers);
        let this = cx.entity().downgrade();
        table = table.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .pt(tokens::SPACE_XS)
                .child(
                    settings_ui::menu_button("add-source-foot", "", icons::PLUS)
                        .dropdown_menu(move |menu, _, _| add_source_menu(menu, &this, scanning)),
                ),
        );
        let note: Option<SharedString> = busy.or_else(|| {
            let status = self.library.read(cx).status();
            (!status.is_empty()).then_some(status)
        });
        let table = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .child(table)
            .when_some(note, |d, note| {
                d.child(
                    // w_full: truncate with no definite width collapses to a
                    // bare ellipsis.
                    div()
                        .w_full()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(note),
                )
            });

        let this = cx.entity().downgrade();
        let controls = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .child(
                settings_ui::menu_button(
                    "add-source",
                    rox_i18n::t!("settings-common-add"),
                    icons::PLUS,
                )
                .dropdown_menu(move |menu, _, _| add_source_menu(menu, &this, scanning)),
            )
            .child(small_button(
                rox_i18n::t!("settings-common-rescan"),
                icons::REFRESH_CW,
                scanning || self.root_stats.is_empty(),
                cx.listener(|this, _, _, cx| {
                    this.library.update(cx, |library, cx| library.rescan(cx));
                }),
            ))
            // Rewrites files with the broken ID3v2.4 shape lofty reads mangled.
            .child(small_button(
                rox_i18n::t!("settings-library-repair-tags"),
                icons::FILE_TEXT,
                scanning,
                cx.listener(|this, _, _, cx| {
                    let library = this.library.clone();
                    let now_art = this.now_art.clone();
                    crate::tags::repair::open(library, now_art, cx);
                }),
            ))
            .child(small_button(
                rox_i18n::t!("settings-library-duplicates"),
                icons::COPY,
                scanning,
                cx.listener(|this, _, _, cx| {
                    let library = this.library.clone();
                    let thumbs = this.thumbs.clone();
                    let now_art = this.now_art.clone();
                    crate::duplicates::open(library, thumbs, now_art, cx);
                }),
            ));
        let excludes = self.exclusion_table(cx);

        // The lead-in and the table share terms, so a search never finds one
        // without the other. Portal terms only while a callout shows.
        let mut folders = vec![
            "scan", "rescan", "music", "add", "remove", "folder", "source",
        ];
        folders.extend_from_slice(subsonic::KEYWORDS);
        if self
            .root_stats
            .iter()
            .any(|(root, _)| rox_core::install::is_portal_path(root))
        {
            folders.extend(["flatpak", "portal", "permission", "override", "folder"]);
        }
        PageBody::new()
            .section(Section::new(
                q,
                icons::FOLDER,
                rox_i18n::t!("settings-library-section-sources"),
                Some(controls.into_any_element()),
                |rows| {
                    let rows = rows.custom(&folders, || lead_in.into_any_element());
                    let rows = match over_limit {
                        Some(limit) => rows.row_dyn(
                            &["monitor", "auto", "rescan", "folder"],
                            rox_i18n::t!("settings-library-watch-folders"),
                            Some(
                                format!(
                                    "Off: this library spans {dirs} folders and each needs \
                                 one Linux file watch, more than the {limit} the app \
                                 will take from the system's shared budget. Rescan by \
                                 hand to fold in changes"
                                )
                                .into(),
                            ),
                            panel::toggle_locked(false),
                        ),
                        None => rows.keyed(
                            "settings-library-watch-folders",
                            &["monitor", "auto", "live"],
                            panel::toggle(self.watch_library, Self::set_watch_library, cx),
                        ),
                    };
                    rows.keyed(
                        "settings-library-merge-case",
                        &["fold", "duplicates", "capitalization"],
                        panel::toggle(self.fold_case, Self::set_fold_case, cx),
                    )
                    .keyed(
                        "settings-show-readings",
                        &["romaji", "reading", "pronunciation", "sort name"],
                        panel::toggle(settings::show_readings(), Self::set_show_readings, cx),
                    )
                    .keyed(
                        "settings-library-split-genres",
                        &["separator", "multi-genre"],
                        panel::toggle(
                            self.split_genre_compounds,
                            Self::set_split_genre_compounds,
                            cx,
                        ),
                    )
                    .when(separators_moved, |rows| {
                        rows.custom(&["genre", "separator", "split", "rescan"], || {
                            nudge.into_any_element()
                        })
                    })
                    .custom(&folders, || table.into_any_element())
                    .custom(
                        &["exclude", "ignore", "skip", "pattern", "glob", "rescan"],
                        || excludes.into_any_element(),
                    )
                },
            ))
            .section(self.acoustic_section(q, cx))
            .section(self.tempo_section(q, cx))
            .section(self.dictionary_section(q, cx))
            .section(self.embed_section(q, cx))
    }

    /// The counts belong to the dialog: working out how many files each source
    /// touches means reading their tags.
    fn embed_section(&self, q: &Query, cx: &mut Context<Self>) -> Section {
        let button = small_button(
            rox_i18n::t!("settings-library-embed-button"),
            icons::UPLOAD,
            self.library.read(cx).busy().is_some(),
            cx.listener(|this, _, _, cx| {
                let library = this.library.clone();
                let now_art = this.now_art.clone();
                crate::bake_dialog::open(library, now_art, cx);
            }),
        )
        .into_any_element();
        Section::new(
            q,
            icons::TAG,
            rox_i18n::t!("settings-library-section-stored-metadata"),
            Some(button),
            |rows| {
                rows.keyed(
                    "settings-library-write-stored",
                    &[
                        "embed",
                        "bake",
                        "tags",
                        "lyrics",
                        "replaygain",
                        "acoustic",
                        "portable",
                    ],
                    div(),
                )
            },
        )
    }

    /// Every row is a COUNT and SUM over the tracks under one path, a full
    /// table scan on a big library. Its own connection, since the catalog's
    /// lives on the UI thread; WAL keeps a running scan from blocking it.
    pub(super) fn measure_root_stats(library: &Entity<Library>, cx: &mut Context<Self>) {
        let db = library.read(cx).db_path();
        let roots = library.read(cx).roots();
        cx.spawn(async move |this, cx| {
            let measured = cx
                .background_executor()
                .spawn(async move {
                    let conn = rox_library::store::open(&db).ok()?;
                    Some(
                        roots
                            .into_iter()
                            .map(|root| {
                                let stats = rox_library::store::stats_under(&conn, &root)
                                    .unwrap_or_default();
                                (root, stats)
                            })
                            .collect::<Vec<_>>(),
                    )
                })
                .await;
            // A database that wouldn't open leaves the rows as they were.
            let Some(measured) = measured else { return };
            this.update(cx, |this, cx| {
                this.root_stats = measured;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// On the Library page, not the ML Models page: that page hides with the AI
    /// switch, which would strand the romanization pass. It isn't a model
    /// either, just a 2007 lookup table of Japanese readings.
    ///
    /// Its own row rather than the model rows' builder: there's one dictionary
    /// and nothing to choose.
    fn dictionary_section(&self, q: &Query, cx: &mut Context<Self>) -> Section {
        let dictionary = &rox_romanize::dictionary::IPADIC;
        let note = self.dictionary_note(cx);
        Section::new(
            q,
            icons::GLOBE,
            rox_i18n::t!("settings-dictionary-heading"),
            None,
            move |rows| {
                let installed = dictionary.installed();
                let size = dictionary.size_on_disk();
                let summary =
                    rox_i18n::try_translate(&format!("dictionary-summary-{}", dictionary.id))
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| dictionary.summary.to_string());
                let mut description = rox_i18n::t!(
                    "settings-dictionary-description",
                    summary = summary,
                    licence = dictionary.licence
                )
                .to_string();
                if installed && size > 0 {
                    description.push_str(&rox_i18n::t!(
                        "settings-mlmodels-on-disk",
                        size = human_size(size)
                    ));
                } else {
                    description.push_str(&rox_i18n::t!(
                        "settings-mlmodels-to-download",
                        size = human_size(dictionary.bytes)
                    ));
                }
                let rows = rows.row_dyn(
                    &["dictionary", "japanese", "romanize", "kanji", "download"],
                    dictionary.label,
                    Some(description.into()),
                    self.dictionary_controls(dictionary, cx),
                );
                match note {
                    Some(note) => rows.custom(&["dictionary", "download", "progress"], || {
                        coverage_note(note).into_any_element()
                    }),
                    None => rows,
                }
            },
        )
    }

    fn dictionary_controls(
        &self,
        dictionary: &'static rox_romanize::dictionary::Dictionary,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let downloading = self
            .dictionary_job
            .as_ref()
            .is_some_and(|job| job.dictionary() == dictionary.id);
        let source = dictionary.source;
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .child(
                div()
                    .id(SharedString::from(format!(
                        "dictionary-source-{}",
                        dictionary.id
                    )))
                    .child(settings_ui::icon_button(
                        icons::EXTERNAL_LINK,
                        false,
                        move |_, _, cx| cx.open_url(source),
                    )),
            )
            .child(if downloading {
                let job = self
                    .dictionary_job
                    .clone()
                    .expect("downloading implies a job");
                small_button(
                    rox_i18n::format::format_percent(f64::from(job.fraction() * 100.0).round()),
                    icons::STOP,
                    false,
                    cx.listener(|_, _, _, cx| crate::romanize_job::dictionary::stop(cx)),
                )
            } else if dictionary.installed() {
                small_button(
                    rox_i18n::t!("settings-common-delete"),
                    icons::TRASH,
                    // Deleting under a running pass would pull the dictionary
                    // out mid-title.
                    crate::romanize_job::progress(cx).is_some(),
                    cx.listener(move |this, _, _, cx| this.delete_dictionary(dictionary, cx)),
                )
            } else {
                small_button(
                    rox_i18n::t!("settings-common-download"),
                    icons::DOWNLOAD,
                    self.dictionary_job.is_some(),
                    cx.listener(move |this, _, _, cx| this.download_dictionary(dictionary, cx)),
                )
            })
            .into_any_element()
    }

    fn dictionary_note(&self, cx: &Context<Self>) -> Option<String> {
        if let Some(job) = &self.dictionary_job {
            if job.stopping() {
                return Some(rox_i18n::t!("settings-dictionary-stopping").to_string());
            }
            return Some(
                rox_i18n::t!(
                    "settings-dictionary-downloading",
                    done = human_size(job.done()),
                    total = human_size(job.total())
                )
                .to_string(),
            );
        }
        let (_, reason) = crate::romanize_job::dictionary::last_failure(cx)?;
        Some(rox_i18n::t!("settings-dictionary-download-failed", reason = reason).to_string())
    }

    fn download_dictionary(
        &mut self,
        dictionary: &'static rox_romanize::dictionary::Dictionary,
        cx: &mut Context<Self>,
    ) {
        crate::romanize_job::dictionary::start(dictionary, cx);
        self.dictionary_job = crate::romanize_job::dictionary::progress(cx);
        Self::poll_dictionary(cx);
        cx.notify();
    }

    /// What it already romanized stays: a delete shouldn't cost a re-run.
    fn delete_dictionary(
        &mut self,
        dictionary: &'static rox_romanize::dictionary::Dictionary,
        cx: &mut Context<Self>,
    ) {
        if let Err(e) = dictionary.delete() {
            log::error!("deleting {}: {e}", dictionary.id);
        }
        // Already-mapped handles stay valid; the next caller finds the files
        // gone.
        rox_romanize::reload();
        cx.notify();
    }

    pub(super) fn poll_dictionary(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(RG_POLL).await;
                let live = this.update(cx, |this, cx| {
                    this.dictionary_job = crate::romanize_job::dictionary::progress(cx);
                    cx.notify();
                    this.dictionary_job.is_some()
                });
                if !matches!(live, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }
}

/// Scanning holds the folder back, since a new folder starts a scan of its own.
fn add_source_menu(
    menu: PopupMenu,
    this: &WeakEntity<SettingsWindow>,
    scanning: bool,
) -> PopupMenu {
    let folder = this.clone();
    let server = this.clone();

    menu.item(
        PopupMenuItem::new(rox_i18n::t!("settings-library-add-folder"))
            .icon(Icon::default().path(icons::FOLDER))
            .disabled(scanning)
            .on_click(move |_, _, cx| {
                if let Some(this) = folder.upgrade() {
                    let library = this.read(cx).library.clone();
                    rox_services::catalog::browse(&library, cx);
                }
            }),
    )
    .item(
        PopupMenuItem::new(rox_i18n::t!("settings-library-add-subsonic"))
            .icon(Icon::default().path(icons::DATABASE))
            .on_click(move |_, window, cx| {
                if let Some(this) = server.upgrade() {
                    this.update(cx, |this, cx| this.add_subsonic(window, cx));
                }
            }),
    )
}

/// The portal never tells a sandboxed app where the folder lives
/// (`Documents.info` is host-only), so the command carries a placeholder parent
/// and the hint says so.
fn portal_banner(root: &Path) -> Div {
    let name = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let command = format!(
        "flatpak override --user --filesystem=\"/path/to/{name}\" {}",
        rox_core::install::APP_ID_RDNS
    );
    let copy = command.clone();

    div()
        .flex()
        .flex_col()
        .gap(tokens::SPACE_XS)
        .py(tokens::SPACE_XS)
        .child(panel::banner_flow(
            panel::Tone::Warn,
            rox_i18n::t!("settings-library-portal-title"),
            vec![
                rox_i18n::t!("settings-library-portal-note"),
                rox_i18n::t!("settings-library-portal-hint"),
            ],
        ))
        // On its own line: overrides run long, and a truncated command is
        // wrong.
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_SM)
                .child(div().flex_1().min_w_0().child(readout(command)))
                .child(small_button(
                    rox_i18n::t!("settings-common-copy"),
                    icons::COPY,
                    false,
                    move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                    },
                )),
        )
}

pub(super) fn seed_root_stats(library: &Entity<Library>, cx: &App) -> Vec<(PathBuf, Stats)> {
    library
        .read(cx)
        .roots()
        .into_iter()
        .map(|root| (root, Stats::default()))
        .collect()
}
