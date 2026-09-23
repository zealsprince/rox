//! The analysis passes on the Library page: acoustic similarity and tempo,
//! each a switch, a save mode where there is one, and a progress line. The
//! pass prompt's host side is here too, since every long pass that prompt
//! starts reports back through it, ReplayGain's on the Audio page included.

use super::*;

impl SettingsWindow {
    fn set_acoustic_analysis(&mut self, on: bool, cx: &mut Context<Self>) {
        self.acoustic_analysis = on;
        Settings::update(move |s| s.acoustic_analysis = on);
        settings::set_acoustic_analysis(on, cx);
        // Switching it off mid-pass stops the pass: it's the only thing that
        // sanctioned the decoding in the first place.
        if !on {
            embeddings::stop(cx);
        }
        cx.notify();
    }

    /// The follow-the-watcher switch for the analysis pass,
    /// [`Self::set_replay_gain_auto`]'s shape: on the way on it prices the
    /// backlog through the prompt, and declining is a no to the switch too,
    /// coming back through `pass_refused`.
    fn set_acoustic_auto(&mut self, on: bool, cx: &mut Context<Self>) {
        self.acoustic_auto = on;
        Settings::update(move |s| s.acoustic_auto = on);
        if on && self.acoustic_coverage.missing() > 0 && self.acoustic_job.is_none() {
            let library = self.library.clone();
            pass_prompt::raise_for_switch(self, pass_prompt::Pass::Acoustic, library, cx);
        }
        cx.notify();
    }

    /// Where an analyzed vector saves. Straight to the file: nothing holds a
    /// live copy of it, and the pass reads it once when it starts, so a pass
    /// already running keeps the destination it began with.
    fn set_acoustic_save(&mut self, save: AcousticSave, cx: &mut Context<Self>) {
        self.acoustic_save = save;
        Settings::update(move |s| s.acoustic_save = save);
        cx.notify();
    }

    /// The Library page's switch: run the built-in sketch, or the model the
    /// ML Models page is offering.
    fn set_acoustic_uses_model(&mut self, on: bool, cx: &mut Context<Self>) {
        let id = if on {
            self.acoustic_ml_source.id().to_string()
        } else {
            rox_acoustic::MODEL.to_string()
        };
        self.use_extractor(&id, cx);
        cx.notify();
    }

    /// Acoustic analysis, on the Library page because that's what it
    /// describes: the switch, which extractor runs, and how far it has got.
    ///
    /// The extractor choice is two options rather than a list of every model,
    /// because the shelf is on the ML Models page. This is a job picking a
    /// tool off it, so the question here is only built-in or the model, and
    /// which model is the other page's business.
    pub(super) fn acoustic_section(&self, q: &Query, cx: &mut Context<Self>) -> Section {
        let on = self.acoustic_analysis;
        let auto = self.acoustic_auto;
        let note = on.then(|| self.acoustic_note());
        let ml_label = self.acoustic_ml_source.label();
        let installed = self.acoustic_ml_source.installed();
        Section::new(
            q,
            icons::AUDIO_WAVEFORM,
            rox_i18n::t!("settings-acoustic-analysis-heading"),
            on.then(|| self.acoustic_control(cx)),
            move |mut rows| {
                rows = rows.keyed(
                    "settings-library-acoustic-enable",
                    &["acoustic", "embeddings", "similar", "analysis"],
                    panel::toggle(on, Self::set_acoustic_analysis, cx),
                );
                if !on {
                    return rows;
                }
                rows = rows.row_dyn(
                    &["extractor", "model", "built-in", "quality"],
                    rox_i18n::t!("settings-library-acoustic-extractor"),
                    Some(
                        if installed {
                            format!(
                                "Built-in needs no download and describes timbre and rhythm. \
                                 {ml_label} hears more, and is the model the ML Models page is \
                                 offering. Switching re-describes the library under the new one"
                            )
                        } else {
                            format!(
                                "Built-in needs no download and describes timbre and rhythm. \
                                 No model is installed, so there's nothing to switch to yet: \
                                 download {ml_label}, or pick a weights file of your own, on \
                                 the ML Models page"
                            )
                        }
                        .into(),
                    ),
                    // Model is dimmed with nothing installed rather than
                    // simply refused. Pressing it used to fall straight back
                    // to Built-in, since the pick can't resolve, which looks
                    // like a broken button rather than a missing download.
                    panel::choices_gated(
                        &[
                            (rox_i18n::t!("settings-common-built-in"), false),
                            (
                                rox_i18n::t!("settings-library-acoustic-extractor-model"),
                                true,
                            ),
                        ],
                        !self.acoustic_source.is_builtin(),
                        move |model| !model || installed,
                        Self::set_acoustic_uses_model,
                        cx,
                    ),
                );
                rows = rows.keyed(
                    "settings-library-acoustic-save",
                    &["save", "write", "tags", "database", "vectors"],
                    panel::choices_shared(
                        &[
                            (
                                rox_i18n::t!("settings-common-database"),
                                AcousticSave::Database,
                            ),
                            (rox_i18n::t!("settings-common-tags"), AcousticSave::Tags),
                        ],
                        self.acoustic_save,
                        Self::set_acoustic_save,
                        cx,
                    ),
                );
                rows = rows.keyed(
                    "settings-library-acoustic-auto",
                    &["automatic", "auto", "new files", "watch"],
                    panel::toggle(auto, Self::set_acoustic_auto, cx),
                );
                match note {
                    Some(note) => rows
                        .custom(&["coverage", "analyze", "missing", "progress"], || {
                            coverage_note(note).into_any_element()
                        }),
                    None => rows,
                }
            },
        )
    }

    fn acoustic_note(&self) -> String {
        if let Some(job) = &self.acoustic_job {
            // The running pass's own model, not the current pick: switching
            // models mid-pass is possible, and the line should say what's
            // actually being written.
            let running = self.label_for(
                &job.model(),
                rox_i18n::t_static("settings-library-acoustic-fallback"),
            );
            let total = job.total();
            if total == 0 {
                return rox_i18n::t!(
                    "settings-library-acoustic-progress-start",
                    running = running
                )
                .to_string();
            }
            let mut line = rox_i18n::t!(
                "settings-library-acoustic-progress",
                running = running,
                done = job.done().min(total) as u64,
                total = total as u64
            )
            .to_string();
            // The pass's own measured rate, which prices whatever worker
            // count it's actually running with.
            if let Some(eta) = job.eta_secs() {
                line.push_str(&rox_i18n::t!(
                    "tasks-time-left",
                    left = rox_core::pace::human(eta)
                ));
            }
            let current = job.current();
            if let Some(name) = Path::new(&current).file_name() {
                line.push_str(&format!(
                    " {}",
                    rox_i18n::t!(
                        "tasks-file-suffix",
                        file = name.to_string_lossy().to_string()
                    )
                ));
            }
            let failed = job.failed();
            if failed > 0 {
                line.push_str(&format!(
                    " {}",
                    rox_i18n::t!("tasks-failed-suffix", count = failed as u64)
                ));
            }
            return line;
        }
        let coverage = self.acoustic_coverage;
        if coverage.total == 0 {
            return rox_i18n::t!("settings-analyze-nothing-scanned").to_string();
        }
        // Named, because the count is per model: every model describes the
        // library separately, and a line that said "142 of 208" without
        // saying whose would read as the library's own progress.
        let label = self.acoustic_source.label();
        if coverage.missing() == 0 {
            return rox_i18n::t!(
                "settings-library-acoustic-all-described",
                total = coverage.total as u64,
                label = label
            )
            .to_string();
        }
        let mut line = rox_i18n::t!(
            "settings-library-acoustic-partial",
            label = label,
            done = coverage.embedded as u64,
            total = coverage.total as u64
        )
        .to_string();
        // Priced off what the last pass measured on this machine for this
        // model, scaled to the worker setting, so dragging the slider shows
        // what it buys. Quiet until a pass has measured anything: a number
        // invented from constants would be wrong on every machine but one.
        if let Some(estimate) = self.acoustic_estimate(coverage.missing()) {
            line.push_str(&format!(
                " {}",
                rox_i18n::t!(
                    "tasks-estimate-at-workers",
                    estimate = estimate,
                    workers = rox_core::pace::workers_phrase(self.acoustic_workers)
                )
            ));
        }
        line
    }

    /// A rough cost for analyzing `missing` tracks at the current worker
    /// setting, off the pace the last pass over this model measured here.
    /// None until one has.
    fn acoustic_estimate(&self, missing: usize) -> Option<String> {
        let pace = *self.acoustic_pace.get(self.acoustic_source.id())?;
        rox_core::pace::estimate(pace, missing as u64, self.acoustic_workers)
    }

    /// Start the pass, or stop the one running. Inert with nothing missing,
    /// and while the library is scanning, since a scan rewrites the very
    /// rows the pass reads.
    fn acoustic_control(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(job) = &self.acoustic_job {
            let stopping = job.stopping();
            return small_button(
                if stopping {
                    rox_i18n::t!("settings-common-stopping")
                } else {
                    rox_i18n::t!("settings-common-stop")
                },
                icons::STOP,
                stopping,
                cx.listener(|_, _, _, cx| embeddings::stop(cx)),
            )
            .into_any_element();
        }
        // Also inert while a model is coming down: the pass would load the
        // half-written file, and the download is the thing that has to
        // finish first anyway.
        let idle = self.acoustic_coverage.missing() == 0
            || self.library.read(cx).busy().is_some()
            || self.model_job.is_some();
        small_button(
            rox_i18n::t!("settings-common-analyze-missing"),
            icons::FLASK,
            idle,
            cx.listener(|this, _, _, cx| {
                let library = this.library.clone();
                pass_prompt::raise(this, pass_prompt::Pass::Acoustic, library, cx);
            }),
        )
        .into_any_element()
    }

    /// Copy the running pass into the section, `poll_measuring`'s twin.
    /// Stops itself once the pass clears the global, and refreshes the
    /// coverage once on the way out so the line ends on the final count.
    ///
    /// Covers the model download on the same timer rather than on one of its
    /// own: the two never run together (the analyze button is inert while a
    /// download runs), and one loop means one place that decides when the
    /// section has stopped moving.
    pub(super) fn poll_analyzing(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(RG_POLL).await;
                let live = this.update(cx, |this, cx| {
                    let was_analyzing = this.acoustic_job.is_some();
                    this.acoustic_job = embeddings::progress(cx);
                    let was_downloading = this.model_job.is_some();
                    this.model_job = embeddings::models::progress(cx);
                    // Only the pass moves the count, so it's re-read on the tick
                    // the pass ends rather than on every tick: this loop also runs
                    // for the whole length of a model download, and the count is a
                    // walk of the tracks table on the UI thread.
                    if was_analyzing && this.acoustic_job.is_none() {
                        this.acoustic_coverage = this
                            .library
                            .read(cx)
                            .acoustic_coverage(this.acoustic_source.id());
                        // The pass that just ended wrote what it measured per
                        // track; pick it up so the next estimate prices off it.
                        this.acoustic_pace = Settings::load().session.acoustic_pace.clone();
                    }
                    // A finished download changed what's on disk, so the sizes
                    // and the install marks have to be re-walked once.
                    if was_downloading && this.model_job.is_none() {
                        this.model_sizes = Self::measure_models();
                    }
                    cx.notify();
                    this.acoustic_job.is_some() || this.model_job.is_some()
                });
                if !matches!(live, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// Tempo analysis, under the acoustic section because it's the same
    /// kind of thing: a pass over the audio that fills a column in. One
    /// switch and one line, since there's nothing to pick: no model, and
    /// nowhere but the database for the numbers to go.
    pub(super) fn tempo_section(&self, q: &Query, cx: &mut Context<Self>) -> Section {
        let on = self.tempo_analysis;
        let auto = self.tempo_auto;
        let note = on.then(|| self.tempo_note());
        Section::new(
            q,
            icons::CLOCK,
            rox_i18n::t!("settings-library-section-tempo"),
            on.then(|| self.tempo_control(cx)),
            move |mut rows| {
                rows = rows.keyed(
                    "settings-library-tempo-enable",
                    &["tempo", "bpm", "analysis"],
                    panel::toggle(on, Self::set_tempo_analysis, cx),
                );
                if on {
                    rows = rows.keyed(
                        "settings-library-tempo-auto",
                        &["automatic", "auto", "new files", "watch"],
                        panel::toggle(auto, Self::set_tempo_auto, cx),
                    );
                }
                match note {
                    Some(note) => rows.custom(
                        &["coverage", "analyze", "missing", "refused", "progress"],
                        || coverage_note(note).into_any_element(),
                    ),
                    None => rows,
                }
            },
        )
    }

    /// The tempo switch. It's the feature as well as the permission: with
    /// it off nothing measures, the BPM column isn't offered, and the pass
    /// no-ops even if something asks it to run.
    fn set_tempo_analysis(&mut self, on: bool, cx: &mut Context<Self>) {
        self.tempo_analysis = on;
        Settings::update(move |s| s.tempo_analysis = on);
        settings::set_tempo_analysis(on, cx);
        cx.notify();
    }

    /// The follow-the-watcher switch for the tempo pass, the acoustic
    /// setter's twin: the backlog gets priced on the way on, and a decline
    /// puts the switch back through `pass_refused`.
    fn set_tempo_auto(&mut self, on: bool, cx: &mut Context<Self>) {
        self.tempo_auto = on;
        Settings::update(move |s| s.tempo_auto = on);
        if on && self.bpm_coverage.missing > 0 && self.tempo_job.is_none() {
            let library = self.library.clone();
            pass_prompt::raise_for_switch(
                self,
                pass_prompt::Pass::Tempo {
                    retry_refused: false,
                },
                library,
                cx,
            );
        }
        cx.notify();
    }

    /// The line under the tempo switch: what a running pass is doing, or
    /// where the library stands. The three-way split is worth spelling out
    /// for the ReplayGain section's reason: a number rox worked out and a
    /// number the file arrived with are not the same claim.
    fn tempo_note(&self) -> String {
        if let Some(job) = &self.tempo_job {
            let total = job.total();
            if total == 0 {
                return rox_i18n::t!("settings-library-tempo-progress-start").to_string();
            }
            let mut line = rox_i18n::t!(
                "settings-library-tempo-progress",
                done = job.done().min(total) as u64,
                total = total as u64
            )
            .to_string();
            if let Some(eta) = job.eta_secs() {
                line.push_str(&rox_i18n::t!(
                    "tasks-time-left",
                    left = rox_core::pace::human(eta)
                ));
            }
            let current = job.current();
            if let Some(name) = Path::new(&current).file_name() {
                line.push_str(&format!(
                    " {}",
                    rox_i18n::t!(
                        "tasks-file-suffix",
                        file = name.to_string_lossy().to_string()
                    )
                ));
            }
            let failed = job.failed();
            if failed > 0 {
                line.push_str(&format!(
                    " {}",
                    rox_i18n::t!("tasks-no-beat-suffix", count = failed as u64)
                ));
            }
            return line;
        }
        let split = self.bpm_coverage;
        let total = split.total();
        if total == 0 {
            return rox_i18n::t!("settings-analyze-nothing-scanned").to_string();
        }
        // Refused gets its own sentence rather than a share of either count.
        // They're not missing, since nothing will pick them up again on its
        // own, and they're not covered either, so folding them into one of
        // the two would misreport it.
        let refused = match split.refused {
            0 => String::new(),
            // Carries its own sentence break, the way the other appended
            // messages carry their leading comma: where one sentence ends
            // and the next starts is the translator's call, not something
            // to hard-code as ". " here and get wrong in Japanese.
            count => rox_i18n::t!("settings-library-tempo-refused", count = count).to_string(),
        };
        // The missing check rides along because a library where every track
        // was refused has no tempos and no work either, and this line offers
        // to do some.
        if split.covered() == 0 && split.missing > 0 {
            return format!(
                "{}{}{refused}",
                rox_i18n::t!("settings-library-tempo-status-none", total = total),
                self.tempo_estimate_suffix(split.missing)
            );
        }
        if split.missing > 0 {
            return format!(
                "{}{}{refused}",
                rox_i18n::t!(
                    "settings-library-tempo-status-partial",
                    covered = split.covered(),
                    total = total,
                    measured = split.measured,
                    missing = split.missing
                ),
                self.tempo_estimate_suffix(split.missing)
            );
        }
        // Nothing left to reach, but a refused pile still means the library
        // isn't fully timed, so the "all of them" wording is kept for the
        // case where it's true of every scanned track.
        if split.refused > 0 {
            let line = if split.measured > 0 {
                rox_i18n::t!(
                    "settings-library-tempo-status-measured-some",
                    covered = split.covered(),
                    total = total,
                    measured = split.measured
                )
            } else {
                rox_i18n::t!(
                    "settings-library-tempo-status-tagged-some",
                    covered = split.covered(),
                    total = total
                )
            };
            return format!("{line}{refused}");
        }
        if split.measured > 0 {
            return rox_i18n::t!(
                "settings-library-tempo-status-measured",
                total = total,
                measured = split.measured
            )
            .to_string();
        }
        rox_i18n::t!("settings-library-tempo-status-tagged", total = total).to_string()
    }

    /// A rough cost for working out `missing` tempos at the current worker
    /// setting, ready to append to the line above, or nothing until a pass
    /// has measured this machine's pace.
    fn tempo_estimate_suffix(&self, missing: u64) -> String {
        match rox_core::pace::estimate(self.tempo_pace, missing, self.tempo_workers) {
            Some(estimate) => format!(
                " {}",
                rox_i18n::t!(
                    "tasks-estimate-at-workers",
                    estimate = estimate,
                    workers = rox_core::pace::workers_phrase(self.tempo_workers)
                )
            ),
            None => String::new(),
        }
    }

    /// Start the pass, or stop the one running. Inert with nothing missing,
    /// and while the library is busy, since a scan rewrites the very rows
    /// the pass reads.
    fn tempo_control(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(job) = &self.tempo_job {
            let stopping = job.stopping();
            return small_button(
                if stopping {
                    rox_i18n::t!("settings-common-stopping")
                } else {
                    rox_i18n::t!("settings-common-stop")
                },
                icons::STOP,
                stopping,
                cx.listener(|_, _, _, cx| tempo_job::stop(cx)),
            )
            .into_any_element();
        }
        let busy = self.library.read(cx).busy().is_some();
        // Retry Refused stands beside Analyze Missing rather than replacing
        // it: the two work through different piles, and the refused one is
        // the only work left once missing hits zero. It goes inert with an
        // empty pile the way its neighbour does with nothing missing, so the
        // pair reads as two standing offers rather than a button that comes
        // and goes.
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .child(small_button(
                rox_i18n::t!("settings-common-analyze-missing"),
                icons::CLOCK,
                self.bpm_coverage.missing == 0 || busy,
                cx.listener(|this, _, _, cx| {
                    let library = this.library.clone();
                    pass_prompt::raise(
                        this,
                        pass_prompt::Pass::Tempo {
                            retry_refused: false,
                        },
                        library,
                        cx,
                    );
                }),
            ))
            .child(small_button(
                rox_i18n::t!("settings-library-tempo-retry"),
                icons::REFRESH_CW,
                self.bpm_coverage.refused == 0 || busy,
                cx.listener(|this, _, _, cx| {
                    let library = this.library.clone();
                    pass_prompt::raise(
                        this,
                        pass_prompt::Pass::Tempo {
                            retry_refused: true,
                        },
                        library,
                        cx,
                    );
                }),
            ))
            .into_any_element()
    }

    /// Copy the running tempo pass into the section, `poll_measuring`'s
    /// twin. Stops itself once the pass clears the global, and re-reads the
    /// split on the way out so the line ends on the final count.
    pub(super) fn poll_timing(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(RG_POLL).await;
                let live = this.update(cx, |this, cx| {
                    let was = this.tempo_job.is_some();
                    this.tempo_job = tempo_job::progress(cx);
                    if was && this.tempo_job.is_none() {
                        this.bpm_coverage = this.library.read(cx).bpm_breakdown();
                        // The pass that just ended wrote what it measured per
                        // track; pick it up so the next estimate prices off it.
                        this.tempo_pace = Settings::load().session.tempo_pace;
                    }
                    cx.notify();
                    this.tempo_job.is_some()
                });
                if !matches!(live, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }
}

/// The pass prompt's host side: where the dialog's state is kept on this
/// window, and what the window re-reads once the dialog has done something.
impl pass_prompt::Host for SettingsWindow {
    fn prompt(&self) -> Option<&pass_prompt::Prompt> {
        self.prompt.as_ref()
    }

    fn prompt_mut(&mut self) -> &mut Option<pass_prompt::Prompt> {
        &mut self.prompt
    }

    fn value_edit(&self) -> &panel::ValueEdit {
        &self.value_edit
    }

    fn dialog_focus(&self) -> &FocusHandle {
        &self.dialog_focus
    }

    /// Everything the pages state about the passes, re-read at once: the
    /// counts a start just changed, the pace a probe just measured, and the
    /// worker counts the dialog's slider wrote.
    fn pass_changed(&mut self, cx: &mut Context<Self>) {
        let settings = Settings::load();
        self.acoustic_workers = settings.acoustic_workers.max(1);
        self.rg_workers = settings.replaygain_workers.max(1);
        self.tempo_workers = settings.tempo_workers.max(1);
        self.acoustic_pace = settings.session.acoustic_pace.clone();
        self.rg_pace = settings.session.replaygain_pace;
        self.tempo_pace = settings.session.tempo_pace;
        self.acoustic_coverage = self
            .library
            .read(cx)
            .acoustic_coverage(self.acoustic_source.id());
        self.rg_coverage = self.library.read(cx).replaygain_breakdown();
        self.bpm_coverage = self.library.read(cx).bpm_breakdown();
        let (was_analyzing, was_measuring) = (self.acoustic_job.is_some(), self.rg_job.is_some());
        let was_timing = self.tempo_job.is_some();
        self.acoustic_job = embeddings::progress(cx);
        self.rg_job = replaygain_job::progress(cx);
        self.tempo_job = tempo_job::progress(cx);
        // A pass that just started needs its poll; one that was already
        // running has a loop and doesn't need a second.
        if !was_analyzing && self.acoustic_job.is_some() {
            Self::poll_analyzing(cx);
        }
        if !was_measuring && self.rg_job.is_some() {
            Self::poll_measuring(cx);
        }
        if !was_timing && self.tempo_job.is_some() {
            Self::poll_timing(cx);
        }
        cx.notify();
    }

    /// The backlog behind a follow-the-watcher switch was declined, so the
    /// switch was a no as well: put it back, rather than leave it on to start
    /// the pass it just refused at the next watch sync.
    fn pass_refused(&mut self, pass: pass_prompt::Pass, cx: &mut Context<Self>) {
        match pass {
            pass_prompt::Pass::Acoustic => {
                self.acoustic_auto = false;
                Settings::update(|s| s.acoustic_auto = false);
            }
            pass_prompt::Pass::ReplayGain => {
                self.playback
                    .update(cx, |player, cx| player.set_replay_gain_auto(false, cx));
            }
            pass_prompt::Pass::Tempo { .. } => {
                self.tempo_auto = false;
                Settings::update(|s| s.tempo_auto = false);
            }
            // No switch stands behind either of the last two, so nothing
            // here ever raises them through `raise_for_switch` and there's
            // nothing to put back.
            pass_prompt::Pass::SortNames { .. } | pass_prompt::Pass::Romanize => {}
        }
    }
}
