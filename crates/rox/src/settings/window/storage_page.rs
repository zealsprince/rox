//! The Storage settings page: what the data folder holds, measured off the
//! UI thread each time the page opens, and the clears that give space back.

use super::*;

/// The storage page's measurements, taken entering the page and after a
/// clear rather than per frame: the database walk and the store walks are
/// affordable once, nowhere near affordable every paint.
#[derive(Clone, Default)]
pub(crate) struct StorageInfo {
    /// The whole library's rollup: tracks, albums, bytes of music.
    music: Stats,
    /// Where library.db's pages went, bucket by bucket. The page reads out
    /// these buckets rather than the file's size, which says only that
    /// something in there is large.
    breakdown: Storage,
    /// Every acoustic model with vectors in the database, including ones
    /// this build doesn't recognize.
    models: Vec<rox_library::embeddings::ModelRows>,
    /// How many listens the library holds and how many of them an import
    /// wrote, the two numbers the history row and its confirm read out.
    pub(super) listens: rox_library::listens::Tally,
    /// thumbs.db with its WAL sidecars.
    thumbs: u64,
    /// Everything under waveforms/.
    waveforms: u64,
    /// Everything in the lyrics store (lyrics/).
    lyrics: u64,
    /// Everything in the artist store (artists/).
    artists: u64,
    /// The downloaded model weights (models/).
    weights: u64,
    /// The look the app is using plus everything set up around it: the
    /// saved workspaces, the ejected shaders.
    app_data: u64,
    /// The log file and its rolled back file (logs/).
    logs: u64,
}

impl StorageInfo {
    /// Walk everything the storage page shows. Runs on the background
    /// executor, on a connection of its own because the library's belongs to
    /// the UI thread: the page accounting reads every page in the file, and
    /// the stores are counted file by file on top of that.
    ///
    /// A database that isn't there is left alone rather than opened, since
    /// opening one creates it, and this page has no business making a
    /// library.db for someone who has never scanned a folder.
    fn measure(db: &Path) -> Self {
        let data = data_dir();
        let conn = db
            .exists()
            .then(|| rox_library::store::open(db).ok())
            .flatten();
        let (music, breakdown, models, listens) = match &conn {
            Some(conn) => (
                rox_library::store::stats(conn).unwrap_or_default(),
                rox_library::store::storage_breakdown(conn).unwrap_or_default(),
                rox_library::embeddings::models(conn).unwrap_or_default(),
                rox_library::listens::tally(conn).unwrap_or_default(),
            ),
            None => Default::default(),
        };
        Self {
            music,
            breakdown,
            models,
            listens,
            thumbs: db_size(&data.join("thumbs.db")),
            waveforms: dir_size(&rox_services::peaks::cache_dir()),
            lyrics: dir_size(&settings::lyrics_dir()),
            artists: dir_size(&settings::artists_dir()),
            weights: dir_size(&rox_acoustic::models::dir()),
            app_data: file_size(&settings::look_path())
                + dir_size(&settings::workspaces_dir())
                + dir_size(&settings::shaders_dir()),
            // The log file follows an override, so the size comes off
            // whichever folder the Reveal button opens rather than the
            // default one beside it.
            logs: rox_core::logging::log_path()
                .parent()
                .map(dir_size)
                .unwrap_or(0),
        }
    }
}

impl SettingsWindow {
    /// Measure everything the storage page shows, off the UI thread. It used
    /// to run whole on page entry, back when it was a handful of stat calls;
    /// the page accounting behind the database rows walks every page in the
    /// file, which is a tenth of a second on a described library, and the
    /// artist and model stores are counted file by file on top of that.
    ///
    /// The snapshot swaps in whole when it arrives, so a remeasure leaves the
    /// numbers already on screen up rather than blanking them, and the first
    /// one shows zeros for the moment it takes. One walk at a time: the
    /// library fires its update repeatedly through a scan, and every search
    /// keystroke asks for numbers until there are some.
    pub(super) fn refresh_storage(&mut self, cx: &mut Context<Self>) {
        if self.storage_measuring {
            // A walk that's already out was started before whatever just
            // changed, so what it brings back is stale on arrival. Queue one
            // behind it rather than dropping the ask or running a second
            // walk over the same files alongside the first.
            self.storage_remeasure = true;
            return;
        }
        self.storage_measuring = true;
        let db = self.library.read(cx).db_path();
        cx.spawn(async move |this, cx| {
            let info = cx
                .background_executor()
                .spawn(async move { StorageInfo::measure(&db) })
                .await;
            this.update(cx, |this, cx| {
                this.storage_measuring = false;
                this.storage = Some(info);
                cx.notify();
                if std::mem::take(&mut this.storage_remeasure) {
                    this.refresh_storage(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Empty the thumbnail store. The delete runs off the UI thread on
    /// the service's own connection, so it serializes against in-flight
    /// loads; the sizes refresh when it finishes.
    fn clear_thumbs(&mut self, cx: &mut Context<Self>) {
        let Some(conn) = self.thumbs.read(cx).store_conn() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move { rox_library::thumbs::clear(&conn) })
                .await;
            this.update(cx, |this, cx| this.refresh_storage(cx)).ok();
        })
        .detach();
    }

    /// Drop the waveform cache; strips re-decode on their next play.
    fn clear_waveforms(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move { rox_services::peaks::clear() })
                .await;
            this.update(cx, |this, cx| this.refresh_storage(cx)).ok();
        })
        .detach();
    }

    /// Drop the artist store: bios, portraits, banners and fanart. The walk
    /// deletes a folder tree, so it goes off the UI thread the way the peak
    /// cache's clear does; the panels fetch again as they open.
    fn clear_artists(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move { rox_services::artists::clear() })
                .await;
            this.update(cx, |this, cx| this.refresh_storage(cx)).ok();
        })
        .detach();
    }

    /// Drop one model's vectors, the confirm dialog's yes. The library holds
    /// the busy badge through the delete and the vacuum behind it and emits
    /// its update when they finish, which remeasures this page.
    pub(super) fn clear_embeddings(&mut self, model: &str, cx: &mut Context<Self>) {
        let model = model.to_owned();
        self.library
            .update(cx, |library, cx| library.clear_embeddings(&model, cx));
    }

    /// The measured-tempos clear, the confirm dialog's yes. The library
    /// emits Updated once the clear finishes, which refreshes the coverage
    /// split this window shows and the row's count with it.
    pub(super) fn clear_measured_bpm(&mut self, cx: &mut Context<Self>) {
        self.library
            .update(cx, |library, cx| library.clear_measured_bpm(cx));
    }

    /// The listening history's clear, either of the dialog's two yeses.
    /// The library emits Updated when the delete finishes, which walks
    /// the storage numbers again and brings the row's count back to what
    /// survived.
    pub(super) fn clear_listens(
        &mut self,
        what: rox_library::listens::Clear,
        cx: &mut Context<Self>,
    ) {
        self.library
            .update(cx, |library, cx| library.clear_listens(what, cx));
    }

    /// A row per acoustic model with vectors in the library: what it
    /// described, and the clear that drops it. Built here rather than inside
    /// the page's section closure because the model id is the row's label,
    /// and rows with built labels don't go through [`Rows::keyed`].
    fn embedding_rows(
        &self,
        models: &[rox_library::embeddings::ModelRows],
        cx: &mut Context<Self>,
    ) -> Vec<(String, AnyElement)> {
        // The library rejects a clear while it's busy, but nothing there can
        // stop the analysis pass, which opens the database by path on its
        // own and would write vectors straight back in behind the delete. The
        // page that offers the button is the one that knows a pass is running.
        let inert = self.acoustic_job.is_some() || self.library.read(cx).busy().is_some();
        models
            .iter()
            .map(|entry| {
                let id = entry.model.clone();
                let known = rox_acoustic::models::find(&id).is_some()
                    || id == rox_acoustic::MODEL
                    || self
                        .acoustic_local
                        .as_ref()
                        .is_some_and(|local| local.id == id);
                let description = if known {
                    format!(
                        "{}, {} values a track. Clearing gives the space back, and having the \
                         descriptions again means a whole pass over the library",
                        self.label_for(
                            &id,
                            rox_i18n::t_static("settings-storage-model-fallback-this")
                        ),
                        entry.dim
                    )
                } else {
                    format!(
                        "{} values a track. Nothing in this build writes this model, so these \
                         are left over from one that was renamed or dropped, and clearing them \
                         costs the library nothing it uses",
                        entry.dim
                    )
                };
                let control = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .child(readout(
                        rox_i18n::t!("settings-common-tracks-count", count = entry.rows)
                            .to_string(),
                    ))
                    .child(small_button(
                        rox_i18n::t!("settings-common-clear"),
                        icons::TRASH,
                        inert,
                        cx.listener({
                            let id = id.clone();
                            move |this, _, _, cx| {
                                this.pending = Some(Pending::ClearEmbeddings(id.clone()));
                                cx.notify();
                            }
                        }),
                    ));
                let row = rox_panel_kit::setting_row_dyn(
                    SharedString::from(id.clone()),
                    Some(description.into()),
                    control,
                )
                .into_any_element();
                (id, row)
            })
            .collect()
    }

    pub(super) fn storage_page(&self, q: &Query, cx: &mut Context<Self>) -> PageBody {
        let info = self.storage.clone().unwrap_or_default();
        let store = info.breakdown;
        let measured = self.storage.is_some();
        let models = self.embedding_rows(&info.models, cx);
        // The two counts come in already worded, so "1 track" never reads
        // as "1 tracks" and every locale gets its own plural rules.
        let music = rox_i18n::t!(
            "settings-storage-music-summary",
            tracks = rox_i18n::t!("status-count-tracks", count = info.music.tracks).to_string(),
            albums = rox_i18n::t!("status-count-albums", count = info.music.albums).to_string(),
            size = human_size(info.music.bytes)
        )
        .to_string();
        // Deletes leave pages behind and nothing vacuums on a schedule, so a
        // freelist is the ordinary state of the file rather than news. It's
        // worth a row once it's a real share of what library.db weighs.
        let reclaimable = store.free >= 1_000_000 && store.free * 10 >= store.total();
        // The tempo row's numbers come off the coverage split the window
        // already keeps live, not the storage walk: a tempo is a float a
        // row, so the interesting figure is how many, not how heavy. The
        // clear can't gate the tempo pass, which opens the database by path
        // on its own, so the button goes inert while one runs, the same
        // arrangement as the model rows above a running analysis.
        let listens = info.listens;
        let measured_tempos = self.bpm_coverage.measured;
        let tempos_inert = measured_tempos == 0
            || self.tempo_job.is_some()
            || self.library.read(cx).busy().is_some();
        PageBody::new()
            .section(Section::new(
                q,
                icons::DATABASE,
                rox_i18n::t!("settings-storage-section-library"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-storage-music-files",
                        &["size", "disk", "space"],
                        readout(music),
                    )
                    .keyed(
                        "settings-storage-catalog",
                        &["database", "catalog", "index", "size", "disk"],
                        readout(human_size(store.catalog)),
                    )
                    .keyed(
                        "settings-storage-playlists-history",
                        &["playlists", "history", "listens", "genres", "size"],
                        readout(human_size(store.playlists + store.history + store.genres)),
                    )
                    .keyed(
                        "settings-storage-listening-history",
                        &[
                            "listens",
                            "plays",
                            "history",
                            "scrobbles",
                            "import",
                            "clear",
                        ],
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            .child(readout(
                                rox_i18n::t!("listens-count", count = listens.total).to_string(),
                            ))
                            .child(small_button(
                                rox_i18n::t!("settings-common-clear"),
                                icons::TRASH,
                                listens.total == 0,
                                cx.listener(|this, _, _, cx| {
                                    this.pending = Some(Pending::ClearListens);
                                    cx.notify();
                                }),
                            )),
                    )
                    .when(reclaimable, |rows| {
                        rows.keyed(
                            "settings-storage-reclaimable",
                            &["free", "reclaim", "vacuum", "deleted", "size"],
                            readout(human_size(store.free)),
                        )
                    })
                    .keyed(
                        "settings-storage-lyrics",
                        &["size", "disk"],
                        readout(human_size(info.lyrics)),
                    )
                },
            ))
            .section(Section::new(
                q,
                icons::AUDIO_WAVEFORM,
                rox_i18n::t!("settings-storage-section-acoustic"),
                None,
                move |mut rows| {
                    rows = rows.keyed(
                        "settings-storage-vectors",
                        &["acoustic", "embeddings", "vectors", "similar", "size"],
                        readout(human_size(store.acoustic)),
                    );
                    if models.is_empty() {
                        // Only once a walk has actually come back. An empty list
                        // is also what the page holds for the beat before the
                        // first one arrives, and saying nothing has been
                        // described is a lie to tell a described library.
                        return rows.when(measured, |rows| {
                            rows.keyed(
                                "settings-storage-models-empty",
                                &["acoustic", "analysis", "model", "describe"],
                                readout(rox_i18n::t!("settings-storage-none").to_string()),
                            )
                        });
                    }
                    for (id, row) in models {
                        let terms = [
                            "acoustic",
                            "embeddings",
                            "vectors",
                            "clear",
                            "model",
                            id.as_str(),
                        ];
                        rows = rows.custom(&terms, || row);
                    }
                    rows
                },
            ))
            .section(Section::new(
                q,
                icons::CLOCK,
                rox_i18n::t!("settings-storage-section-tempo"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-storage-measured-tempos",
                        &["tempo", "bpm", "measured", "clear"],
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            .child(readout(
                                rox_i18n::t!(
                                    "settings-common-tracks-count",
                                    count = measured_tempos
                                )
                                .to_string(),
                            ))
                            .child(small_button(
                                rox_i18n::t!("settings-common-clear"),
                                icons::TRASH,
                                tempos_inert,
                                cx.listener(|this, _, _, cx| {
                                    this.pending = Some(Pending::ClearMeasuredBpm);
                                    cx.notify();
                                }),
                            )),
                    )
                },
            ))
            .section(Section::new(
                q,
                icons::LAYERS,
                rox_i18n::t!("settings-storage-section-caches"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-storage-cover-thumbnails",
                        &["cache", "clear", "artwork", "size"],
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            .child(readout(human_size(info.thumbs)))
                            .child(small_button(
                                rox_i18n::t!("settings-common-clear"),
                                icons::TRASH,
                                false,
                                cx.listener(|this, _, _, cx| this.clear_thumbs(cx)),
                            )),
                    )
                    .keyed(
                        "settings-storage-waveforms",
                        &["cache", "clear", "peaks", "size"],
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            .child(readout(human_size(info.waveforms)))
                            .child(small_button(
                                rox_i18n::t!("settings-common-clear"),
                                icons::TRASH,
                                false,
                                cx.listener(|this, _, _, cx| this.clear_waveforms(cx)),
                            )),
                    )
                    .keyed(
                        "settings-storage-artist-images",
                        &[
                            "cache",
                            "clear",
                            "artist",
                            "images",
                            "portrait",
                            "biography",
                            "size",
                        ],
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            .child(readout(human_size(info.artists)))
                            .child(small_button(
                                rox_i18n::t!("settings-common-clear"),
                                icons::TRASH,
                                false,
                                cx.listener(|this, _, _, cx| this.clear_artists(cx)),
                            )),
                    )
                },
            ))
            .section(Section::new(
                q,
                icons::FOLDER,
                rox_i18n::t!("settings-storage-section-app-data"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-storage-model-weights",
                        &["model", "weights", "download", "ml", "size"],
                        readout(human_size(info.weights)),
                    )
                    .keyed(
                        "settings-storage-looks-layouts",
                        &["workspace", "layout", "shader", "icons", "look", "size"],
                        readout(human_size(info.app_data)),
                    )
                },
            ))
            .section(Section::new(
                q,
                icons::FILE_TEXT,
                rox_i18n::t!("settings-storage-section-diagnostics"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-storage-logs",
                        &["debug", "reveal", "diagnostics"],
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            .child(readout(human_size(info.logs)))
                            .child(small_button(
                                rox_i18n::t!("settings-common-reveal"),
                                icons::FILE_TEXT,
                                false,
                                cx.listener(|_, _, _, cx| {
                                    cx.reveal_path(&rox_core::logging::log_path());
                                }),
                            )),
                    )
                },
            ))
    }
}

/// A SQLite database's weight on disk: the file plus its -wal and -shm
/// sidecars, which hold real data between checkpoints.
fn db_size(db: &Path) -> u64 {
    ["", "-wal", "-shm"]
        .iter()
        .map(|suffix| {
            let mut file = db.as_os_str().to_owned();
            file.push(suffix);
            std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0)
        })
        .sum()
}

/// Every file under one folder, subfolders and all. The caches are flat,
/// but the ejected shaders are nested a folder per workspace deep and the icon
/// packs a folder per pack, and a walk that stopped at the top would report
/// those as nothing. A symlink is measured as the link rather than followed,
/// so nothing here can walk in a circle.
fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => dir_size(&entry.path()),
            _ => entry.metadata().map(|meta| meta.len()).unwrap_or(0),
        })
        .sum()
}

/// One file's weight, zero when it isn't there.
fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}
