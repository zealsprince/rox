//! The ML Models settings page: the shelf of models a job can run, the
//! recommended ones rox fetches and a custom file the user supplies, and
//! which one the acoustic pass uses.

use super::*;

/// Where a model for a given job comes from: the shelf rox keeps, or a file
/// the user supplies. Every model category on the ML Models page reads this
/// way, so a second category (whatever job it ends up doing) inherits the
/// same two halves rather than inventing its own arrangement.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum ModelKind {
    Recommended,
    Custom,
}

fn model_kinds() -> Vec<(SharedString, ModelKind)> {
    vec![
        (
            rox_i18n::t!("settings-mlmodels-kind-recommended"),
            ModelKind::Recommended,
        ),
        (
            rox_i18n::t!("settings-mlmodels-kind-custom"),
            ModelKind::Custom,
        ),
    ]
}

impl SettingsWindow {
    /// The models page: what can run a job that needs a network, what it
    /// costs to fetch, and which one the library uses.
    ///
    /// Its own page rather than a section under Library because a model is an
    /// asset with a lifecycle of its own. It's downloaded, it takes up disk
    /// space, it can be replaced by a file the user supplies, and it will one
    /// day serve more than one job. The Library page picks a job's extractor;
    /// this page is the shelf that pick reads from.
    ///
    /// One section per job the models do, which today is acoustic
    /// analysis and one day won't be. Each section is the same shape: a
    /// Recommended half rox keeps a catalog for, and a Custom half that is
    /// whatever file the user points at. That's the whole reason the split
    /// is a control on the section rather than a second section, since a
    /// standalone "Custom Model" would have nothing to say about which job
    /// it was custom for.
    pub(super) fn ml_models_page(&self, q: &Query, cx: &mut Context<Self>) -> PageBody {
        PageBody::new().section(self.acoustic_models_section(q, cx))
    }

    /// Where the acoustic vectors come from: the catalog's downloads, or a
    /// checkpoint of the user's own.
    fn acoustic_models_section(&self, q: &Query, cx: &mut Context<Self>) -> Section {
        let kind = self.models_kind;
        let picker = panel::choices_shared(
            &model_kinds(),
            kind,
            |this: &mut Self, kind, cx| {
                this.models_kind = kind;
                cx.notify();
            },
            cx,
        )
        .into_any_element();
        // The download's progress and the last pass's failure belong to the
        // whole category, so they show under either half; a refused file is
        // the Custom half's own business and only shows there.
        let note = match kind {
            ModelKind::Custom => self
                .acoustic_local_error
                .clone()
                .or_else(|| self.model_note(cx)),
            ModelKind::Recommended => self.model_note(cx),
        };
        // A search shows both halves. The picker is a way of putting one of
        // them away, and a row nobody can find because it's behind a control
        // the searcher can't see is worse than a longer section.
        let searching = q.active();
        Section::new(
            q,
            icons::AUDIO_WAVEFORM,
            rox_i18n::t!("settings-acoustic-analysis-heading"),
            Some(picker),
            move |mut rows| {
                if searching || kind == ModelKind::Recommended {
                    rows = self.recommended_model_rows(rows, cx);
                }
                if searching || kind == ModelKind::Custom {
                    rows = self.custom_model_row(rows, cx);
                }
                match note {
                    Some(note) => rows.custom(&["download", "progress", "model"], || {
                        coverage_note(note).into_any_element()
                    }),
                    None => rows,
                }
            },
        )
    }

    /// The catalog half: one row per model rox knows how to fetch, each
    /// saying what it is, what it costs, and what licence it arrives under.
    ///
    /// The licence is on the row rather than in a footnote because the user
    /// is the one accepting it. Nothing here is bundled, so a download is
    /// them fetching a model for their own use, and that only works as an
    /// arrangement if the terms are in front of them when they press the
    /// button.
    fn recommended_model_rows<'a>(&self, mut rows: Rows<'a>, cx: &mut Context<Self>) -> Rows<'a> {
        // Only the ones with weights to fetch. The built-in extractor is
        // code rather than a model, and it belongs on the Library page as
        // the other side of the extractor switch, not on a shelf of
        // downloads.
        for model in rox_acoustic::models::CATALOG
            .iter()
            .filter(|model| model.weights.is_some())
        {
            let size = self.model_size(model.id);
            // The catalog is a domain crate and stays English; the summary
            // maps to a key here, at the UI layer, the way `Source::label`
            // does. A model with no message falls back to its own prose.
            let summary = rox_i18n::try_translate(&format!("model-summary-{}", model.id))
                .map(|s| s.to_string())
                .unwrap_or_else(|| model.summary.to_string());
            let mut description = rox_i18n::t!(
                "settings-mlmodels-description",
                summary = summary,
                dim = model.dim as u64,
                licence = model.licence
            )
            .to_string();
            if size > 0 {
                description.push_str(&rox_i18n::t!(
                    "settings-mlmodels-on-disk",
                    size = human_size(size)
                ));
            } else if let Some(weights) = &model.weights {
                description.push_str(&rox_i18n::t!(
                    "settings-mlmodels-to-download",
                    size = human_size(weights.bytes)
                ));
            }
            rows = rows.row_dyn(
                &["model", "acoustic", "download", "embeddings", "similar"],
                model.label,
                Some(description.into()),
                self.model_controls(model, cx),
            );
        }
        rows
    }

    /// The custom half: a weights file on disk, for a CNN10 someone trained
    /// or fine-tuned themselves. No download, no size to state before
    /// fetching, and no licence rox can name, since the file is the user's
    /// own.
    fn custom_model_row<'a>(&self, rows: Rows<'a>, cx: &mut Context<Self>) -> Rows<'a> {
        let busy = self.model_job.is_some() || self.acoustic_job.is_some();
        let checking = self.acoustic_local_checking;
        let local = self.acoustic_local.clone();
        let description = match &local {
            Some(local) => format!(
                "{}. {} values per track, stored under {}, so its vectors never mix with \
                 the catalog's",
                local.path.display(),
                rox_acoustic::panns::DIM,
                local.id
            ),
            None => rox_i18n::t!("settings-mlmodels-custom-description-empty").to_string(),
        };
        let mut controls = div().flex().flex_row().items_center().gap(tokens::SPACE_SM);
        if local.is_some() {
            controls = controls.child(small_button(
                rox_i18n::t!("settings-common-clear"),
                icons::TRASH,
                busy || checking,
                cx.listener(|this, _, _, cx| this.clear_local_model(cx)),
            ));
        }
        controls = controls.child(small_button(
            if checking {
                rox_i18n::t!("settings-mlmodels-checking")
            } else {
                rox_i18n::t!("settings-mlmodels-choose-file")
            },
            icons::FOLDER,
            busy || checking,
            cx.listener(|this, _, window, cx| this.pick_local_model(window, cx)),
        ));
        let running = local
            .as_ref()
            .is_some_and(|local| self.model_running(&local.id));
        controls = controls.child(if running {
            readout(rox_i18n::t!("settings-common-active").to_string()).into_any_element()
        } else {
            small_button(
                rox_i18n::t!("settings-common-use"),
                icons::CHECK,
                local.is_none() || busy || checking,
                cx.listener(|this, _, _, cx| this.use_local_model(cx)),
            )
            .into_any_element()
        });
        rows.row_dyn(
            &["custom", "model", "local", "weights", "checkpoint", "file"],
            rox_i18n::t!("settings-mlmodels-weights-file"),
            Some(description.into()),
            controls.into_any_element(),
        )
    }

    /// Whether a model row is the extractor the library is actually running.
    /// Kept apart from the buttons because the shelf and the custom row draw
    /// differently and have to match exactly on this. Being the page's pick
    /// isn't a state a row shows: with the Library page on
    /// Built-in nothing here is running, and a row claiming otherwise reads
    /// as though it were describing the library.
    fn model_running(&self, id: &str) -> bool {
        self.acoustic_source.id() == id
    }

    /// Browse for a weights file and check what comes back.
    fn pick_local_model(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        self.acoustic_local_error = None;
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.pop() else {
                return;
            };
            this.update(cx, |this, cx| this.check_local_model(path, cx))
                .ok();
        })
        .detach();
    }

    /// Name a weights file by its hash, then check it by loading it, and take
    /// it as the custom model. The hash and the load both happen off the UI
    /// thread: one reads 25 MB, the other builds the network and runs a probe
    /// pass over it.
    ///
    /// Loading it is the validation. There's no checksum to compare against,
    /// so the only way to find out whether a file is this network is to have
    /// candle build it, which fails with the name of the missing tensor when
    /// it isn't.
    fn check_local_model(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.acoustic_local_checking = true;
        self.acoustic_local_error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let checked = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move {
                        // Stamped before the read rather than after it: a file
                        // rewritten while this hashes then leaves a stamp that
                        // matches nothing, which reads as changed and hashes
                        // again, instead of one that vouches for bytes nobody
                        // ever hashed.
                        let stamp = settings::file_stamp(&path).unwrap_or_default();
                        let digest = rox_acoustic::models::hash_file(&path)?;
                        rox_acoustic::panns::Cnn10::load_from(&path)?;
                        Ok::<_, String>((rox_acoustic::local_id(&digest), stamp))
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                this.acoustic_local_checking = false;
                match checked {
                    Ok((id, (bytes, mtime))) => this.adopt_local_model(
                        settings::LocalModel {
                            path,
                            id,
                            bytes,
                            mtime,
                        },
                        cx,
                    ),
                    Err(reason) => this.acoustic_local_error = Some(reason),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Take a checked file as the custom model and make it the page's offer,
    /// since browsing to a file is a clearer "use this" than any button on
    /// the row would be.
    fn adopt_local_model(&mut self, local: settings::LocalModel, cx: &mut Context<Self>) {
        self.acoustic_local = Some(local.clone());
        let stored = local.clone();
        // Before the pick below: resolving a local id reads this back out of
        // the settings file.
        Settings::update(move |s| s.acoustic_local_model = Some(stored.clone()));
        self.set_acoustic_model(
            rox_acoustic::Source::Local(Arc::new(rox_acoustic::Local {
                path: local.path,
                id: local.id,
            })),
            cx,
        );
    }

    /// Offer the custom model again after something else was picked. A file
    /// whose bytes have moved since it was named goes back through the check
    /// instead: the id is the hash, so a checkpoint retrained in place is a
    /// different model and has to be adopted under its own name rather than
    /// filling the old one's coordinates. Nothing else can adopt it for the
    /// user, since resolving that id rejects the file until it's re-hashed.
    fn use_local_model(&mut self, cx: &mut Context<Self>) {
        let Some(local) = self.acoustic_local.clone() else {
            return;
        };
        if settings::file_stamp(&local.path) != Some((local.bytes, local.mtime)) {
            self.check_local_model(local.path, cx);
            return;
        }
        self.set_acoustic_model(
            rox_acoustic::Source::Local(Arc::new(rox_acoustic::Local {
                path: local.path,
                id: local.id,
            })),
            cx,
        );
    }

    /// Forget the custom model. What it described stays in the database under
    /// its own name, the way a deleted download's vectors do: point rox at
    /// the same file again and it comes back to the work it already did.
    fn clear_local_model(&mut self, cx: &mut Context<Self>) {
        let was = self.acoustic_local.take();
        self.acoustic_local_error = None;
        Settings::update(|s| s.acoustic_local_model = None);
        // Nothing to fall back to but the catalog, so a library running the
        // file that just went away moves off it rather than failing at the
        // next pass.
        if was.is_some_and(|local| self.acoustic_source.id() == local.id) {
            self.use_extractor(rox_acoustic::MODEL, cx);
        }
        self.acoustic_ml_source = rox_services::acoustic::acoustic_ml_source();
        cx.notify();
    }

    /// What each catalog model weighs on disk right now. Walked entering the
    /// page and after anything that changes it, never in a paint.
    pub(super) fn measure_models() -> Vec<(&'static str, u64)> {
        rox_acoustic::models::CATALOG
            .iter()
            .map(|model| (model.id, model.size_on_disk()))
            .collect()
    }

    fn model_size(&self, id: &str) -> u64 {
        self.model_sizes
            .iter()
            .find(|(name, _)| *name == id)
            .map(|(_, size)| *size)
            .unwrap_or(0)
    }

    /// One model row's buttons: make it the page's offer, and fetch or drop
    /// its weights. A model whose download is running shows the stop instead,
    /// and one that isn't installed can't be offered, since selecting a model
    /// rox can't load would leave the pass failing with no explanation on the
    /// row that caused it.
    fn model_controls(
        &self,
        model: &'static rox_acoustic::models::Model,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Deleting the running model would leave the pass with nothing to
        // load, so it holds its weights until something else is picked.
        let running = self.model_running(model.id);
        let installed = model.installed();
        let downloading = self
            .model_job
            .as_ref()
            .is_some_and(|job| job.model() == model.id);
        let busy = self.model_job.is_some() || self.acoustic_job.is_some();

        // Where the model came from, so someone deciding whether to fetch
        // 24 MB and accept its licence can go and read about it first.
        let source = model.source;
        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .child(
                div()
                    .id(SharedString::from(format!("model-source-{}", model.id)))
                    .child(settings_ui::icon_button(
                        icons::EXTERNAL_LINK,
                        false,
                        move |_, _, cx| cx.open_url(source),
                    )),
            );
        if model.weights.is_some() {
            row = row.child(if downloading {
                let job = self.model_job.clone().expect("downloading implies a job");
                small_button(
                    rox_i18n::format::format_percent(f64::from(job.fraction() * 100.0).round()),
                    icons::STOP,
                    false,
                    cx.listener(|_, _, _, cx| embeddings::models::stop(cx)),
                )
            } else if installed {
                small_button(
                    rox_i18n::t!("settings-common-delete"),
                    icons::TRASH,
                    busy || running,
                    cx.listener(move |this, _, _, cx| this.delete_model(model, cx)),
                )
            } else {
                small_button(
                    rox_i18n::t!("settings-common-download"),
                    icons::DOWNLOAD,
                    busy,
                    cx.listener(move |this, _, _, cx| this.download_model(model, cx)),
                )
            });
        }
        // Only the running extractor gets a word. Pressing Use on a row while
        // the Library page is on Built-in points the Model switch here, which
        // the Extractor row spells out by name, so the shelf doesn't need a
        // second mark for it.
        row = row.child(if running {
            readout(rox_i18n::t!("settings-common-active").to_string()).into_any_element()
        } else {
            small_button(
                rox_i18n::t!("settings-common-use"),
                icons::CHECK,
                !installed || busy,
                cx.listener(move |this, _, _, cx| {
                    this.set_acoustic_model(rox_acoustic::Source::Catalog(model), cx)
                }),
            )
            .into_any_element()
        });
        row.into_any_element()
    }

    /// The line under the model list: what a running download is doing, or
    /// why the last one or the last pass gave up.
    fn model_note(&self, cx: &Context<Self>) -> Option<String> {
        if let Some(job) = &self.model_job {
            let label = self.label_for(
                &job.model(),
                rox_i18n::t_static("settings-mlmodels-fallback-model"),
            );
            if job.stopping() {
                return Some(rox_i18n::t!("settings-mlmodels-stopping", label = label).to_string());
            }
            return Some(
                rox_i18n::t!(
                    "settings-mlmodels-downloading",
                    label = label,
                    done = human_size(job.done()),
                    total = human_size(job.total())
                )
                .to_string(),
            );
        }
        if let Some((id, reason)) = embeddings::models::last_failure(cx) {
            let label = self.label_for(
                &id,
                rox_i18n::t_static("settings-mlmodels-fallback-the-model"),
            );
            return Some(
                rox_i18n::t!(
                    "settings-mlmodels-download-failed",
                    label = label,
                    reason = reason
                )
                .to_string(),
            );
        }
        // A pass that failed to start is nearly always the model rather than
        // the library, so its reason belongs on this section.
        embeddings::last_failure(cx).map(|reason| {
            rox_i18n::t!("settings-mlmodels-pass-stopped", reason = reason).to_string()
        })
    }

    /// Make a model the active one: what the pass fills and what the
    /// similarity queries read. The coverage line re-counts against it, so
    /// switching immediately says how much of the library this model has
    /// actually described rather than keeping the last one's number. When
    /// the library is already running a model rather than the built-in
    /// extractor, the switch follows the new pick straight away; when it's
    /// on Built-in this only changes what Model would mean.
    fn set_acoustic_model(&mut self, source: rox_acoustic::Source, cx: &mut Context<Self>) {
        let id = source.id().to_string();
        self.acoustic_ml_source = source;
        let stored = id.clone();
        Settings::update(move |s| s.acoustic_ml_model = stored.clone());
        if !self.acoustic_source.is_builtin() {
            self.use_extractor(&id, cx);
        }
        cx.notify();
    }

    /// Point the library at an extractor and re-read its coverage. Every
    /// model describes the library separately, so the count has to follow the
    /// pick rather than the pick alone.
    ///
    /// The live pick is set from the id and then read back rather than
    /// assigned here, so this window can't end up showing a model the rest of
    /// the app refused to resolve.
    pub(super) fn use_extractor(&mut self, id: &str, cx: &mut Context<Self>) {
        let owned = id.to_string();
        Settings::update(move |s| s.acoustic_model = owned.clone());
        rox_services::acoustic::set_acoustic_model(id, cx);
        self.acoustic_source = rox_services::acoustic::acoustic_source();
        self.acoustic_coverage = self
            .library
            .read(cx)
            .acoustic_coverage(self.acoustic_source.id());
        // Every model describes the library separately, so switching can turn
        // ordering by sound on or off for the surfaces that offer it.
        let described = self.library.read(cx).analyzed(self.acoustic_source.id());
        settings::set_acoustic_described(described, cx);
    }

    fn download_model(
        &mut self,
        model: &'static rox_acoustic::models::Model,
        cx: &mut Context<Self>,
    ) {
        embeddings::models::start(model, cx);
        self.model_job = embeddings::models::progress(cx);
        Self::poll_analyzing(cx);
        cx.notify();
    }

    /// Drop a model's weights. The vectors it already wrote stay: they're
    /// still valid, and making a delete cost a full re-analysis would turn a
    /// reclaim-some-disk into an afternoon.
    fn delete_model(
        &mut self,
        model: &'static rox_acoustic::models::Model,
        cx: &mut Context<Self>,
    ) {
        if let Err(e) = model.delete() {
            log::error!("deleting {}: {e}", model.id);
        }
        self.model_sizes = Self::measure_models();
        cx.notify();
    }
}
