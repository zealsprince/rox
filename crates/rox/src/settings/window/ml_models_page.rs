//! The ML Models settings page: the models a job can run, recommended ones rox
//! fetches or a file the user supplies, and which one the acoustic pass uses.

use super::*;

/// Every model category has these two halves.
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
    /// Its own page rather than part of Library because a model has a
    /// lifecycle: downloaded, sized, replaceable by a user file. The Library
    /// page picks the extractor off this shelf.
    ///
    /// One section per job; the Recommended/Custom split is a control on the
    /// section so a custom file stays tied to its job.
    pub(super) fn ml_models_page(&self, q: &Query, cx: &mut Context<Self>) -> PageBody {
        PageBody::new().section(self.acoustic_models_section(q, cx))
    }

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
        // Download progress and pass failures show under either half; a refused
        // file only under Custom.
        let note = match kind {
            ModelKind::Custom => self
                .acoustic_local_error
                .clone()
                .or_else(|| self.model_note(cx)),
            ModelKind::Recommended => self.model_note(cx),
        };
        // A search shows both halves, so no row hides behind the picker.
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

    /// The licence is on the row because the user is the one accepting it:
    /// nothing here is bundled.
    fn recommended_model_rows<'a>(&self, mut rows: Rows<'a>, cx: &mut Context<Self>) -> Rows<'a> {
        // Only models with weights. The built-in extractor lives on the Library
        // page's switch.
        for model in rox_acoustic::models::CATALOG
            .iter()
            .filter(|model| model.weights.is_some())
        {
            let size = self.model_size(model.id);
            // The catalog crate stays English; the summary maps to a message
            // key here, falling back to its own prose.
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

    /// No download, size or licence to state: the file is the user's own.
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

    /// Whether the library runs this model now. Being the page's pick isn't
    /// shown: with Library on Built-in nothing here runs.
    fn model_running(&self, id: &str) -> bool {
        self.acoustic_source.id() == id
    }

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

    /// Hashed and loaded off the UI thread. Loading is the validation: candle
    /// fails with the missing tensor's name when the file isn't this network.
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
                        // Stamped before the read, so a file rewritten mid-hash
                        // reads as changed rather than vouching for bytes nobody
                        // hashed.
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

    /// Browsing to a file makes it the page's offer.
    fn adopt_local_model(&mut self, local: settings::LocalModel, cx: &mut Context<Self>) {
        self.acoustic_local = Some(local.clone());
        let stored = local.clone();
        // Before the pick below, which reads this back out of the file.
        Settings::update(move |s| s.acoustic_local_model = Some(stored.clone()));
        self.set_acoustic_model(
            rox_acoustic::Source::Local(Arc::new(rox_acoustic::Local {
                path: local.path,
                id: local.id,
            })),
            cx,
        );
    }

    /// A file whose bytes changed goes back through the check: the id is the
    /// hash, so a retrained checkpoint is a different model.
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

    /// What it described stays in the database, so the same file picks up its
    /// old work.
    fn clear_local_model(&mut self, cx: &mut Context<Self>) {
        let was = self.acoustic_local.take();
        self.acoustic_local_error = None;
        Settings::update(|s| s.acoustic_local_model = None);
        // A library running the file moves back to the built-in extractor
        // rather than failing at the next pass.
        if was.is_some_and(|local| self.acoustic_source.id() == local.id) {
            self.use_extractor(rox_acoustic::MODEL, cx);
        }
        self.acoustic_ml_source = rox_services::acoustic::acoustic_ml_source();
        cx.notify();
    }

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

    /// An uninstalled model can't be offered: the pass would fail with nothing
    /// on the row to say why.
    fn model_controls(
        &self,
        model: &'static rox_acoustic::models::Model,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // The running model holds its weights until something else is picked.
        let running = self.model_running(model.id);
        let installed = model.installed();
        let downloading = self
            .model_job
            .as_ref()
            .is_some_and(|job| job.model() == model.id);
        let busy = self.model_job.is_some() || self.acoustic_job.is_some();

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
        // Only the running extractor gets a mark; the Extractor row names the
        // page's pick.
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
        // A pass that failed to start is nearly always the model's fault.
        embeddings::last_failure(cx).map(|reason| {
            rox_i18n::t!("settings-mlmodels-pass-stopped", reason = reason).to_string()
        })
    }

    /// Following straight away only when the library already runs a model; on
    /// Built-in this only changes what Model means.
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

    /// Set from the id and read back, so this window never shows a model the
    /// rest of the app refused to resolve.
    pub(super) fn use_extractor(&mut self, id: &str, cx: &mut Context<Self>) {
        let owned = id.to_string();
        Settings::update(move |s| s.acoustic_model = owned.clone());
        rox_services::acoustic::set_acoustic_model(id, cx);
        self.acoustic_source = rox_services::acoustic::acoustic_source();
        self.acoustic_coverage = self
            .library
            .read(cx)
            .acoustic_coverage(self.acoustic_source.id());
        // Switching models can turn ordering by sound on or off.
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
