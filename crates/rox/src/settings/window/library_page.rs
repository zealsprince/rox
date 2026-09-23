//! The Library settings page: the sources table, the library switches, the
//! kanji dictionary, and the embed catch-up. The Subsonic rows in the sources
//! table are `subsonic`'s, and the acoustic and tempo passes further down the
//! page are `analysis`'s.

use super::*;

impl SettingsWindow {
    /// The watch-folders switch: flip the local copy and hand it to the shared
    /// library, which persists it and arms or drops the watcher on the spot.
    fn set_watch_library(&mut self, on: bool, cx: &mut Context<Self>) {
        self.watch_library = on;
        self.library
            .update(cx, |library, cx| library.set_watch(on, cx));
        cx.notify();
    }

    /// The case-fold switch: flip the live flag, persist, and reload the
    /// projection so every symbol table re-interns under the new rule.
    /// The database never changes; this is a read-model rebuild.
    fn set_fold_case(&mut self, on: bool, cx: &mut Context<Self>) {
        self.fold_case = on;
        settings::set_fold_case(on);
        Settings::update(move |s| s.fold_case = on);
        self.library
            .update(cx, |library, cx| library.reload_projection(cx));
        cx.notify();
    }

    /// The genre-separator switch, the case-fold's twin: flip the live
    /// flag in the genre module, persist, and reload the projection so
    /// every genre surface re-splits under the new rule. Files and the
    /// database never change; matching splits stored strings at read.
    fn set_split_genre_compounds(&mut self, on: bool, cx: &mut Context<Self>) {
        self.split_genre_compounds = on;
        rox_library::genre::set_split_compounds(on);
        Settings::update(move |s| s.split_genre_compounds = on);
        self.library
            .update(cx, |library, cx| library.reload_projection(cx));
        cx.notify();
    }

    /// The readings switch: through the live static, which every name cell
    /// reads as it draws, and into the file. Nothing is rebuilt, since the
    /// sort names are already in the projection; the windows just repaint.
    /// The toggle reads the static rather than a cached field, so it can't
    /// drift from what the panels are drawing.
    fn set_show_readings(&mut self, on: bool, cx: &mut Context<Self>) {
        settings::set_show_readings(on, cx);
        Settings::update(move |s| s.show_readings = on);
        cx.notify();
    }

    /// One row of the folder table: the path, its rollup numbers, and a
    /// remove control, inert while a scan runs.
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
            // Named after the folder, so the row's remove button is its
            // own rather than every other row's. See
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
        // Past the ceiling the watch stays off, so the toggle grays
        // out at off and the note says why, with the numbers. Folders summed
        // off the cached rollups, not a per-frame count; the roots never
        // nest, so nothing counts twice. Matched to the catalog's own limit,
        // which is None where the platform prices watching flat.
        let dirs = self.root_stats.iter().map(|(_, s)| s.dirs).sum::<u64>();
        let over_limit = rox_services::catalog::watch_limit_dirs().filter(|limit| dirs > *limit);
        let lead_in = div()
            .text_xs()
            .text_color(palette::text_muted())
            .child(rox_i18n::t!("settings-library-folders-intro"));
        // The rescan nudge, only while the separator rule has moved
        // this session: filtering and the genre wall follow the flip
        // right away, but genre lists earlier scans wrote into the
        // database keep their old shape until a rescan re-reads the
        // tags.
        let separators_moved = self.split_genre_compounds != self.split_genre_compounds_at_open;
        let nudge = div()
            .text_xs()
            .text_color(palette::text_muted())
            .child(rox_i18n::t!("settings-library-genre-separator-nudge"));
        // The sources table: a column header line, then a hairlined row per
        // folder and per server. One list for every kind, so a source that
        // arrives later as an extension takes a row here rather than a
        // section of its own.
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

            // A folder the sandbox reaches through the document portal:
            // every read goes through the portal and the watcher sees
            // nothing. The callout under the row carries the override that
            // grants the real folder. Only ever true inside a Flatpak.
            if rox_core::install::is_portal_path(root) {
                table = table.child(portal_banner(root));
            }
        }
        table = table.children(servers);
        // An add slot at the foot of the list, where the eye lands after
        // reading it. Same menu the header's Add opens.
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
        // The library's badge and the file under the scan cursor, or the
        // resting status, under the table.
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
                    // w_full: truncate with no definite width measures at
                    // min-content and the line collapses to a bare
                    // ellipsis.
                    div()
                        .w_full()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(note),
                )
            });

        // Add and rescan are in the section header like the colors
        // controls. Add drops the kinds of source; rescan walks the folders,
        // since a server has its own Sync.
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
            // The tag repair window: find and rewrite files with the
            // broken ID3v2.4 tag shape lofty reads mangled, where a user
            // ends up after seeing garbled tags.
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
            // The duplicates window: find tracks the library has more than
            // once and move the spare copies to the trash.
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
        // The lead-in describes the table, so both use the same terms
        // and a search never turns up one without the other. The portal
        // callouts live inside the table, so it answers to their terms too,
        // but only while one is showing.
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
                },
            ))
            .section(self.acoustic_section(q, cx))
            .section(self.tempo_section(q, cx))
            .section(self.dictionary_section(q, cx))
            .section(self.embed_section(q, cx))
    }

    /// The catch-up for the three save settings: what rox is already holding
    /// written into the files themselves.
    ///
    /// It's here, under the acoustic radio, because this page already has two
    /// of the three questions it answers: the save mode for descriptions is
    /// the row above it, and the folder tools at the top of the page are the
    /// other things that rewrite a library's files. The
    /// counts belong to the dialog rather than this row: working out how many
    /// files each source would touch means reading their tags, which is not
    /// something a settings page should do on the way past.
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

    /// Roll each scan folder up off the UI thread. Every row of that table
    /// is a COUNT and a SUM over the tracks under one path, which is a
    /// full table scan on a big library, and opening this window used to
    /// pay for all of them before it drew anything. The rows are already
    /// on screen by then; their numbers fill in when this lands.
    ///
    /// Its own connection rather than the catalog's, since the catalog's
    /// lives on the UI thread. WAL gives readers concurrency for free, so
    /// a scan running alongside this doesn't block it.
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
            // A database that wouldn't open is nothing to report: leave the
            // folders listed with whatever they last showed.
            let Some(measured) = measured else { return };
            this.update(cx, |this, cx| {
                this.root_stats = measured;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The Japanese dictionary behind kanji readings.
    ///
    /// On the Library page rather than beside the acoustic weights, which
    /// is where it looks like it belongs: it's a download with a size and
    /// a licence, the same shape those have. The ML Models page comes and
    /// goes with the AI switch, and with that switch off the dictionary
    /// would be unreachable while the romanization pass still ran and
    /// still pointed people at a page that wasn't in the sidebar. It also
    /// isn't a model. It's a lookup table of Japanese words and their
    /// readings, compiled in 2007, and nothing about it is learned.
    ///
    /// A second row rather than one shared with the model rows. The
    /// acoustic rows carry a Use button, an active mark and an extractor
    /// pick, because a library runs exactly one of several models and
    /// choosing between them is what that page half is for. There's one
    /// dictionary, nothing to choose, and no state for a Use button to
    /// move; factoring the two together would mean a row builder taking
    /// half its arguments as None from one caller. The download button,
    /// the progress readout and the licence line are the parts that
    /// repeat, and they're four lines each.
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

    /// The dictionary row's buttons: where it came from, then the one
    /// button that changes state.
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
                    // Deleting under a running pass would pull the
                    // dictionary out from under it mid-title.
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

    /// What the dictionary row says under itself: the download's progress,
    /// or why the last one didn't finish.
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

    /// Fetch and unpack the dictionary.
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

    /// Drop the dictionary. What it already romanized stays in the
    /// library's tables: those rows are still the best answer rox has, and
    /// making a delete cost a re-run would turn a reclaim-some-disk into a
    /// second pass.
    fn delete_dictionary(
        &mut self,
        dictionary: &'static rox_romanize::dictionary::Dictionary,
        cx: &mut Context<Self>,
    ) {
        if let Err(e) = dictionary.delete() {
            log::error!("deleting {}: {e}", dictionary.id);
        }
        // Whatever the shared accessor handed out stays mapped, but the
        // next caller has to find out the files are gone.
        rox_romanize::reload();
        cx.notify();
    }

    /// Keep the dictionary row moving while its download runs. Its own
    /// loop rather than a branch in [`Self::poll_analyzing`], since the two
    /// downloads are independent and either can run without the other.
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

/// The kinds of source the Add menu offers. A folder opens the file picker
/// straight away, the way the add slot always has; a server opens its setup
/// dialog. Scanning holds the folder back, since a new folder starts a
/// scan of its own.
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

/// The Library page's callout under a folder the sandbox reaches through
/// the document portal: what that costs, and the `flatpak override` that
/// grants the real folder. The portal never tells a sandboxed app where the
/// folder really lives (`Documents.info` is host-only), so the command
/// carries a placeholder parent for the user to fill in and the hint says
/// so, rather than the command pretending to be complete.
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
        // The command on its own line with the copy button beside it, the
        // socket path's shape: overrides run long, and a line that
        // truncates is a command that's wrong.
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

/// The scan folders with their rollups blank, what the table shows until
/// [`SettingsWindow::measure_root_stats`] comes back. Reading the roots
/// costs nothing; it's counting under them that's the scan.
pub(super) fn seed_root_stats(library: &Entity<Library>, cx: &App) -> Vec<(PathBuf, Stats)> {
    library
        .read(cx)
        .roots()
        .into_iter()
        .map(|root| (root, Stats::default()))
        .collect()
}
