//! The Audio settings page: crossfade, ReplayGain, and the output backend,
//! with exclusive mode's device, rate, format and period rows. ReplayGain's
//! measure pass polls from here, since this page is where its progress shows.

use super::*;

/// The rates the exclusive picker offers: the two base clocks and their
/// doubles and quadruples, which is every rate consumer hardware actually
/// runs. A card that hasn't got one falls back to its nearest and reports that.
const RATES: &[u32] = &[44100, 48000, 88200, 96000, 176400, 192000];

/// The periods the buffer picker offers, in milliseconds, either side of the
/// backend's 10 ms default.
const PERIODS_MS: &[f64] = &[2.5, 5.0, 10.0, 20.0, 40.0];

impl SettingsWindow {
    /// Everything that shapes the samples on their way to the device, in the
    /// order the audio meets it: the chain first (ADR 19), then the backend
    /// that hands it over.
    pub(super) fn audio_page(&self, q: &Query, cx: &mut Context<Self>) -> PageBody {
        PageBody::new()
            .section(Section::new(
                q,
                icons::AUDIO_LINES,
                rox_i18n::t!("settings-audio-section-playback"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-audio-transport",
                        &["play", "pause", "seek", "random", "preview"],
                        panel::transport_strip(&self.playback, &self.library, cx),
                    )
                    .custom(
                        &["crossfade", "fade", "gapless", "overlap", "transition"],
                        || self.crossfade_row(cx).into_any_element(),
                    )
                    .custom(&["crossfade", "fade", "gapless", "album", "splice"], || {
                        self.crossfade_albums_row(cx).into_any_element()
                    })
                },
            ))
            .section(self.replay_gain_section(q, cx))
            .section(Section::new(
                q,
                icons::SLIDERS,
                rox_i18n::t!("settings-audio-section-equalizer"),
                Some(
                    small_button(
                        rox_i18n::t!("settings-audio-open-equalizer"),
                        icons::AUDIO_LINES,
                        false,
                        cx.listener(|_, _, _, cx| crate::eq_window::open(cx)),
                    )
                    .into_any_element(),
                ),
                |rows| {
                    rows.custom(&["eq", "bands", "bass", "treble", "tone"], || {
                        div()
                            .text_xs()
                            .text_color(palette::text_muted())
                            .child(rox_i18n::t!("settings-audio-equalizer-note"))
                            .into_any_element()
                    })
                },
            ))
            .section(self.output_section(q, cx))
    }

    /// How long one track overlaps the next. Zero is off, which is the
    /// gapless boundary rox has always had; anything else fades only where
    /// the music isn't continuous, so an album still splices.
    fn crossfade_row(&self, cx: &mut Context<Self>) -> Div {
        panel::setting_row(
            rox_i18n::t!("settings-audio-crossfade"),
            Some(rox_i18n::t!("settings-audio-crossfade.description")),
            settings_ui::scalar(
                &self.crossfade_scrub,
                &self.value_edit,
                self.playback.read(cx).crossfade_secs(),
                settings_ui::span(0., engine::CROSSFADE_MAX_SECS, " s")
                    .decimals(1)
                    .hard(),
                |this: &mut Self, secs, cx| {
                    this.playback
                        .update(cx, |player, cx| player.set_crossfade_secs(secs, cx));
                    cx.notify();
                },
                cx,
            ),
        )
    }

    /// Whether the fade takes an album's own boundaries too. Inert while
    /// the fade is off, since there'd be nothing for it to change.
    fn crossfade_albums_row(&self, cx: &mut Context<Self>) -> Div {
        let player = self.playback.read(cx);
        let on = player.crossfade_albums();
        let control: AnyElement = if player.crossfade_secs() > 0.0 {
            panel::toggle(
                on,
                |this: &mut Self, on, cx| {
                    this.playback
                        .update(cx, |player, cx| player.set_crossfade_albums(on, cx));
                    cx.notify();
                },
                cx,
            )
            .into_any_element()
        } else {
            panel::toggle_locked(on).into_any_element()
        };
        panel::setting_row(
            rox_i18n::t!("settings-audio-fade-inside-albums"),
            Some(rox_i18n::t!(
                "settings-audio-fade-inside-albums.description"
            )),
            control,
        )
    }

    /// The ReplayGain section: which of a file's two gains to level by, the
    /// two offsets around it, and where the measurement pass puts what it
    /// measures. The offsets only show once a mode is picked, since with
    /// leveling off there's nothing for them to offset.
    fn replay_gain_section(&self, q: &Query, cx: &mut Context<Self>) -> Section {
        let modes: Vec<(SharedString, GainModeSetting)> = vec![
            (
                rox_i18n::t!("settings-audio-replaygain-mode-off"),
                GainModeSetting::Off,
            ),
            (
                rox_i18n::t!("settings-audio-replaygain-mode-track"),
                GainModeSetting::Track,
            ),
            (
                rox_i18n::t!("settings-audio-replaygain-mode-album"),
                GainModeSetting::Album,
            ),
        ];
        let rg = self.playback.read(cx).replay_gain();
        // A running pass takes over the line under the section: its count, the
        // file it's on, and whatever it had to skip. With nothing scanned
        // there's no coverage to state either.
        let split = self.rg_coverage;
        let total = split.total();
        let note: Option<String> = if let Some(job) = &self.rg_job {
            Some(Self::measure_progress_line(job))
        } else if total == 0 {
            None
        } else if split.covered() == 0 {
            Some(format!(
                "None of the {total} tracks scanned have a ReplayGain to level by. Measure \
                 Missing analyzes them and saves the numbers{}",
                self.rg_estimate_suffix(split.missing)
            ))
        } else if split.missing > 0 {
            Some(format!(
                "{} of {total} scanned tracks have a gain to level by, {} of them measured \
                 by rox. The other {} play at the untagged setting{}",
                split.covered(),
                split.measured,
                split.missing,
                self.rg_estimate_suffix(split.missing),
            ))
        } else if split.measured > 0 {
            Some(
                rox_i18n::t!(
                    "settings-audio-replaygain-status-measured",
                    total = total,
                    measured = split.measured
                )
                .to_string(),
            )
        } else {
            Some(rox_i18n::t!("settings-audio-replaygain-status-tagged", total = total).to_string())
        };
        Section::new(
            q,
            icons::GAUGE,
            rox_i18n::t!("settings-audio-section-replaygain"),
            Some(self.measure_control(cx)),
            |rows| {
                let rows = rows
                    .keyed(
                        "settings-audio-replaygain-level-by",
                        &["volume", "normalization", "loudness", "leveling"],
                        panel::choices_shared(
                            &modes,
                            rg.mode,
                            |this: &mut Self, mode, cx| {
                                this.playback
                                    .update(cx, |player, cx| player.set_replay_gain_mode(mode, cx));
                                cx.notify();
                            },
                            cx,
                        ),
                    )
                    .when(rg.mode != GainModeSetting::Off, |rows| {
                        rows.keyed(
                            "settings-audio-replaygain-preamp",
                            &["volume", "gain", "boost", "loudness"],
                            settings_ui::scalar(
                                &self.preamp_scrub,
                                &self.value_edit,
                                rg.preamp_db,
                                settings_ui::span(-15., 15., " dB").decimals(1).hard(),
                                |this: &mut Self, db, cx| {
                                    this.playback.update(cx, |player, cx| {
                                        player.set_replay_gain_preamp(db, cx)
                                    });
                                    cx.notify();
                                },
                                cx,
                            ),
                        )
                        .keyed(
                            "settings-audio-replaygain-untagged",
                            &["fallback", "default gain", "missing"],
                            settings_ui::scalar(
                                &self.fallback_scrub,
                                &self.value_edit,
                                rg.fallback_db,
                                settings_ui::span(-15., 15., " dB").decimals(1).hard(),
                                |this: &mut Self, db, cx| {
                                    this.playback.update(cx, |player, cx| {
                                        player.set_replay_gain_fallback(db, cx)
                                    });
                                    cx.notify();
                                },
                                cx,
                            ),
                        )
                    })
                    .keyed(
                        "settings-audio-replaygain-save",
                        &["write", "tags", "database", "analysis"],
                        panel::choices_shared(
                            &[
                                (
                                    rox_i18n::t!("settings-common-database"),
                                    ReplayGainSave::Database,
                                ),
                                (rox_i18n::t!("settings-common-tags"), ReplayGainSave::Tags),
                            ],
                            rg.save,
                            Self::set_replay_gain_save,
                            cx,
                        ),
                    )
                    .keyed(
                        "settings-audio-replaygain-measure-new",
                        &["automatic", "auto", "new files", "watch"],
                        panel::toggle(rg.auto, Self::set_replay_gain_auto, cx),
                    );
                match note {
                    Some(note) => rows
                        .custom(&["coverage", "measure", "missing", "progress"], || {
                            coverage_note(note).into_any_element()
                        }),
                    None => rows,
                }
            },
        )
    }

    /// Where a measured gain saves. Through the player like the other three
    /// leveling knobs, since it holds the live copy of the whole struct.
    fn set_replay_gain_save(&mut self, save: ReplayGainSave, cx: &mut Context<Self>) {
        self.playback
            .update(cx, |player, cx| player.set_replay_gain_save(save, cx));
        cx.notify();
    }

    /// The follow-the-watcher switch, through the player like the rest of the
    /// section. On the way on it asks about the backlog: the pass's work list
    /// is everything with no gain, so a switch flipped over a library nobody
    /// has measured would start hours of decoding at the next watch sync
    /// without anyone having seen a number first. The prompt prices that
    /// backlog and measures it now; declining is a no to the switch too, and
    /// comes back here through `pass_refused`.
    ///
    /// Nothing to ask about with nothing missing, or with a pass already
    /// working through it, so the switch just goes on.
    fn set_replay_gain_auto(&mut self, on: bool, cx: &mut Context<Self>) {
        self.playback
            .update(cx, |player, cx| player.set_replay_gain_auto(on, cx));
        if on && self.rg_coverage.missing > 0 && self.rg_job.is_none() {
            let library = self.library.clone();
            pass_prompt::raise_for_switch(self, pass_prompt::Pass::ReplayGain, library, cx);
        }
        cx.notify();
    }

    /// The section header's control: start the pass, or stop the one that's
    /// running. Inert with nothing missing, and while the library is busy
    /// scanning, since a scan is rewriting the very rows the pass reads.
    fn measure_control(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(job) = &self.rg_job {
            let stopping = job.stopping();
            return small_button(
                if stopping {
                    rox_i18n::t!("settings-common-stopping")
                } else {
                    rox_i18n::t!("settings-common-stop")
                },
                icons::STOP,
                stopping,
                cx.listener(|_, _, _, cx| replaygain_job::stop(cx)),
            )
            .into_any_element();
        }
        let idle = self.rg_coverage.missing == 0 || self.library.read(cx).busy().is_some();
        small_button(
            rox_i18n::t!("settings-audio-replaygain-measure-missing-button"),
            icons::GAUGE,
            idle,
            cx.listener(|this, _, _, cx| {
                let library = this.library.clone();
                pass_prompt::raise(this, pass_prompt::Pass::ReplayGain, library, cx);
            }),
        )
        .into_any_element()
    }

    /// Copy the running pass into the section, the scan badge's cadence.
    /// Stops itself once the pass clears the global.
    pub(super) fn poll_measuring(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(RG_POLL).await;
                let live = this.update(cx, |this, cx| {
                    let was = this.rg_job.is_some();
                    this.rg_job = replaygain_job::progress(cx);
                    // The pass that just ended wrote what it measured per file;
                    // pick it up so the next estimate prices off it.
                    if was && this.rg_job.is_none() {
                        this.rg_pace = Settings::load().session.replaygain_pace;
                    }
                    cx.notify();
                    this.rg_job.is_some()
                });
                if !matches!(live, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// A rough cost for measuring `missing` files at the current worker
    /// setting, ready to append to the coverage line, or nothing until a
    /// pass has measured this machine's pace. Off the last pass's own
    /// average, so it prices these files on this disk rather than an
    /// imagined library.
    fn rg_estimate_suffix(&self, missing: u64) -> String {
        match rox_core::pace::estimate(self.rg_pace, missing, self.rg_workers) {
            Some(estimate) => format!(
                " {}",
                rox_i18n::t!(
                    "tasks-estimate-at-workers",
                    estimate = estimate,
                    workers = rox_core::pace::workers_phrase(self.rg_workers)
                )
            ),
            None => String::new(),
        }
    }

    /// The running pass as one line: how far along, what it's on, and what
    /// it gave up on. The work list is built first, so a zero total means
    /// the pass hasn't finished building it.
    fn measure_progress_line(job: &replaygain_job::Progress) -> String {
        let total = job.total();
        if total == 0 {
            return rox_i18n::t!("settings-audio-replaygain-measuring-start").to_string();
        }
        let mut line = rox_i18n::t!(
            "settings-audio-replaygain-measuring-progress",
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
                rox_i18n::t!("tasks-failed-suffix", count = failed as u64)
            ));
        }
        line
    }

    /// Whether this platform's exclusive backend has ever been run by us on
    /// real hardware. ALSA and CoreAudio have; the WASAPI backend is written
    /// from the platform contract and shipped for testers, which is exactly
    /// what the badge and the issue link say.
    fn exclusive_experimental() -> bool {
        cfg!(target_os = "windows")
    }

    /// The prefilled new-issue page for exclusive-mode reports: the platform
    /// and version filled in, plus what the stream negotiated if one is up,
    /// so a report from a tester arrives with the part they'd forget.
    fn exclusive_issue_url(&self, cx: &Context<Self>) -> String {
        let negotiated = self
            .playback
            .read(cx)
            .output_status()
            .map(|status| {
                let negotiated = status.negotiated;
                format!(
                    "{:?} on {}, {} Hz, {} ch, {}{}",
                    negotiated.mode,
                    negotiated.device,
                    negotiated.sample_rate,
                    negotiated.channels,
                    negotiated.format,
                    negotiated
                        .fallback
                        .map(|why| format!("\nFallback reason: {why}"))
                        .unwrap_or_default(),
                )
            })
            .unwrap_or_else(|| "Nothing playing".into());
        let title = format!("Exclusive output on {}: ", std::env::consts::OS);
        let body = format!(
            "rox {} on {} ({})\n\nNegotiated: {}\n\nWhat happened:\n",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
            negotiated,
        );
        format!(
            "https://github.com/zealsprince/rox/issues/new?title={}&body={}",
            urlencode(&title),
            urlencode(&body)
        )
    }

    /// The badge and its report button are in the Output header rather than
    /// the Exclusive Mode row: they're about the whole backend, not the switch,
    /// and the header's right edge is where a section-wide caveat belongs.
    /// Returns None where nothing is being warned about.
    fn exclusive_notice(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if !output::exclusive_supported() || !Self::exclusive_experimental() {
            return None;
        }
        let url = self.exclusive_issue_url(cx);
        Some(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_XS)
                .child(
                    div()
                        .id("exclusive-experimental")
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(tokens::SPACE_XS)
                        .px(tokens::SPACE_SM)
                        .py(px(2.))
                        .rounded(tokens::RADIUS)
                        .bg(palette::alpha(palette::tone_warn(), 0x1c))
                        .text_xs()
                        .text_color(palette::tone_warn())
                        .child(
                            svg()
                                .path(icons::FLASK)
                                .size(px(12.))
                                .text_color(palette::tone_warn()),
                        )
                        .child(rox_i18n::t!("settings-audio-output-experimental-badge"))
                        .tooltip(|_, cx| {
                            cx.new(|_| {
                                ExperimentalTooltip(rox_i18n::t!(
                                    "settings-audio-output-experimental-tooltip"
                                ))
                            })
                            .into()
                        }),
                )
                .child(
                    div()
                        .id("exclusive-issue")
                        .child(settings_ui::icon_button(
                            icons::EXTERNAL_LINK,
                            false,
                            move |_, _, cx| cx.open_url(&url),
                        ))
                        .tooltip(|_, cx| {
                            cx.new(|_| {
                                ExperimentalTooltip(rox_i18n::t!(
                                    "settings-audio-output-issue-tooltip"
                                ))
                            })
                            .into()
                        }),
                )
                .into_any_element(),
        )
    }

    /// The Output section: the exclusive switch, the device list for
    /// whichever backend that picks, and what the running stream actually
    /// negotiated. The readout is the point of the section: the two rows
    /// above it are requests, and ADR 19 asks the UI to state the reality
    /// rather than repeat the ask.
    fn output_section(&self, q: &Query, cx: &mut Context<Self>) -> Section {
        // Where no exclusive backend is built there's nothing to toggle:
        // every claim would fall back, and a switch that never does
        // anything reads as a bug in the hardware rather than a gap in rox.
        let exclusive: AnyElement = if output::exclusive_supported() {
            panel::toggle(self.output_exclusive, Self::set_output_exclusive, cx).into_any_element()
        } else {
            readout(rox_i18n::t!("settings-audio-output-not-built").to_string()).into_any_element()
        };
        Section::new(
            q,
            icons::VOLUME_2,
            rox_i18n::t!("settings-audio-section-output"),
            self.exclusive_notice(cx),
            |rows| {
                rows.keyed(
                    "settings-audio-exclusive-mode",
                    &["bit perfect", "wasapi", "asio", "hog"],
                    exclusive,
                )
                .custom(
                    &["device", "soundcard", "headphones", "interface", "rescan"],
                    || self.output_devices_block(cx).into_any_element(),
                )
                .custom(&["sample rate", "hz", "khz", "resample"], || {
                    self.output_rate_row(cx).into_any_element()
                })
                .custom(&["format", "bit depth", "float", "integer"], || {
                    self.output_format_row(cx).into_any_element()
                })
                .custom(&["buffer", "latency", "period", "underrun"], || {
                    self.output_period_row(cx).into_any_element()
                })
                .custom(&["status", "negotiated", "stream", "fallback"], || {
                    self.output_status_block(cx).into_any_element()
                })
            },
        )
    }

    /// The three hardware knobs below only mean anything on a device rox
    /// holds alone. In shared mode the server owns the rate, the format and
    /// the buffer, so they draw inert rather than pretending.
    fn exclusive_only(&self) -> bool {
        !self.output_exclusive || !output::exclusive_supported()
    }

    /// The rate the device runs at: following each file's own lets a
    /// mixed-rate library play without a resampler anywhere, so it leads.
    fn output_rate_row(&self, cx: &mut Context<Self>) -> Div {
        let mut options: Vec<(Option<u32>, SharedString)> =
            vec![(None, rox_i18n::t!("settings-audio-output-rate-follow"))];
        options.extend(RATES.iter().map(|hz| {
            (
                Some(*hz),
                rox_i18n::format::format_unit(f64::from(*hz) / 1000.0, 1, "kHz").into(),
            )
        }));
        panel::setting_row(
            rox_i18n::t!("settings-audio-output-sample-rate"),
            Some(rox_i18n::t!(
                "settings-audio-output-sample-rate.description"
            )),
            panel::picker(
                "output-rate",
                self.playback.read(cx).output_rate(),
                options,
                self.exclusive_only(),
                |this: &mut Self, rate, cx| {
                    this.playback
                        .update(cx, |player, cx| player.set_output_rate(rate, cx));
                    cx.notify();
                },
                cx,
            ),
        )
    }

    /// The sample format asked for. Widest-available is right almost always;
    /// the pick exists for a card whose driver works better on one of them.
    fn output_format_row(&self, cx: &mut Context<Self>) -> Div {
        let options: Vec<(Option<String>, SharedString)> = vec![
            (None, rox_i18n::t!("settings-audio-output-format-widest")),
            (
                Some("f32".into()),
                rox_i18n::t!("settings-audio-output-format-f32"),
            ),
            (
                Some("s32".into()),
                rox_i18n::t!("settings-audio-output-format-s32"),
            ),
            (
                Some("s16".into()),
                rox_i18n::t!("settings-audio-output-format-s16"),
            ),
        ];
        panel::setting_row(
            rox_i18n::t!("settings-audio-output-format"),
            Some(rox_i18n::t!("settings-audio-output-format.description")),
            panel::picker(
                "output-format",
                self.playback.read(cx).output_format().map(str::to_string),
                options,
                self.exclusive_only(),
                |this: &mut Self, format, cx| {
                    this.playback
                        .update(cx, |player, cx| player.set_output_format(format, cx));
                    cx.notify();
                },
                cx,
            ),
        )
    }

    /// The period, the latency trade stated plainly.
    fn output_period_row(&self, cx: &mut Context<Self>) -> Div {
        let mut options: Vec<(Option<f64>, SharedString)> =
            vec![(None, rox_i18n::t!("settings-audio-output-buffer-default"))];
        options.extend(PERIODS_MS.iter().map(|ms| {
            (
                Some(*ms),
                rox_i18n::format::format_unit(*ms, 1, "ms").into(),
            )
        }));
        panel::setting_row(
            rox_i18n::t!("settings-audio-output-buffer"),
            Some(rox_i18n::t!("settings-audio-output-buffer.description")),
            panel::picker(
                "output-period",
                self.playback.read(cx).output_period(),
                options,
                self.exclusive_only(),
                |this: &mut Self, ms, cx| {
                    this.playback
                        .update(cx, |player, cx| player.set_output_period(ms, cx));
                    cx.notify();
                },
                cx,
            ),
        )
    }

    /// The device picker for the mode that's on, the system default at the
    /// head so switching back is one pick. Rescan is beside it because the
    /// list is taken when the window opens: plugging an interface in while
    /// it's up shouldn't mean closing and reopening.
    fn output_devices_block(&self, cx: &mut Context<Self>) -> Div {
        let mut options: Vec<(Option<String>, SharedString)> = vec![(
            None,
            rox_i18n::t!("settings-audio-output-device-system-default"),
        )];
        options.extend(
            self.output_devices
                .iter()
                .map(|device| (Some(device.id.clone()), device.name.clone().into())),
        );
        // The toggle swaps which backend's list this is, and on Linux the two
        // don't even overlap: exclusive enumerates kernel sound cards, and a
        // Bluetooth headset only exists inside the sound server. The note has
        // to say so, or a device that was just here reads as lost.
        let description = if self.exclusive_only() {
            rox_i18n::t!("settings-audio-output-device.description-default")
        } else if cfg!(target_os = "linux") {
            rox_i18n::t!("settings-audio-output-device.description-linux")
        } else {
            rox_i18n::t!("settings-audio-output-device.description-other")
        };
        panel::setting_row(
            rox_i18n::t!("settings-audio-output-device"),
            Some(description),
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_SM)
                .child(panel::picker(
                    "output-device",
                    self.playback.read(cx).output_device().map(str::to_string),
                    options,
                    false,
                    |this: &mut Self, id, cx| this.set_output_device(id, cx),
                    cx,
                ))
                .child(small_button(
                    rox_i18n::t!("settings-common-rescan"),
                    icons::REFRESH_CW,
                    false,
                    cx.listener(|this, _, _, cx| this.rescan_output_devices(cx)),
                )),
        )
    }

    /// What the stream negotiated, in plain words. Nothing here is derived
    /// from the settings above: a fallback line only appears because a
    /// backend reported one, and the rate line compares the device's rate
    /// against the file's rather than against what was asked for.
    fn output_status_block(&self, cx: &mut Context<Self>) -> Div {
        let Some(status) = self.playback.read(cx).output_status() else {
            // No stream and an error means the last open failed, which is a
            // different thing from an idle player and shouldn't read the
            // same: one is waiting, the other is broken.
            return match self.playback.read(cx).error() {
                Some(error) => panel::banner(
                    panel::Tone::Bad,
                    rox_i18n::t!("settings-audio-output-status-error-title"),
                    vec![
                        error,
                        rox_i18n::t!("settings-audio-output-status-error-hint"),
                    ],
                ),
                None => panel::banner(
                    panel::Tone::Info,
                    rox_i18n::t!("settings-audio-output-status-idle-title"),
                    vec![rox_i18n::t!("settings-audio-output-status-idle-hint")],
                ),
            };
        };
        let negotiated = &status.negotiated;
        let mode = match negotiated.mode {
            output::Mode::Exclusive => rox_i18n::t_static("settings-audio-output-mode-exclusive"),
            output::Mode::Shared => rox_i18n::t_static("settings-audio-output-mode-shared"),
        };
        let resampling = status
            .source_rate
            .is_some_and(|source| source != negotiated.sample_rate);
        // The tone is the whole point of the callout, and the two bad cases
        // aren't the same size. A claim that failed is a setting that didn't
        // take, which is an error: exclusive is switched on and you aren't
        // hearing it. Resampling is the mode working and still not being
        // bit-perfect, which is worth flagging without crying wolf.
        let tone = if negotiated.fallback.is_some() {
            panel::Tone::Bad
        } else if resampling {
            panel::Tone::Warn
        } else {
            panel::Tone::Good
        };
        // The experimental note goes in the banner too: someone reading only
        // the status line should know the mode they're hearing is the one
        // nobody has hardware-tested.
        let experimental =
            negotiated.mode == output::Mode::Exclusive && Self::exclusive_experimental();
        let headline = rox_i18n::t!(
            "settings-audio-output-headline",
            mode = mode.to_string(),
            note = if experimental {
                rox_i18n::t!("settings-audio-output-experimental").to_string()
            } else {
                String::new()
            },
            device = negotiated.device.clone(),
            rate = negotiated.sample_rate as u64,
            channels = negotiated.channels as u64,
            format = negotiated.format.to_string()
        )
        .to_string();
        // The expanded register: this block has a page to itself, so each
        // reason keeps a sentence of its own where the output panel folds
        // them into one line.
        panel::banner(tone, headline, status.lines(true, true))
    }

    /// Ask for exclusive output, or give the device back. The player
    /// rebuilds its running session onto the other backend right here, so
    /// the switch takes effect without a restart, and the device list is
    /// the other backend's from this point.
    fn set_output_exclusive(&mut self, on: bool, cx: &mut Context<Self>) {
        self.output_exclusive = on;
        self.playback
            .update(cx, |player, cx| player.set_exclusive_output(on, cx));
        self.output_devices = output::devices(output_mode(on));
        cx.notify();
    }

    /// Pick a device for the mode that's on, None for the system default.
    fn set_output_device(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        self.playback
            .update(cx, |player, cx| player.set_output_device(id, cx));
        cx.notify();
    }

    /// Re-enumerate, for an interface plugged in while this window is open.
    fn rescan_output_devices(&mut self, cx: &mut Context<Self>) {
        self.output_devices = output::devices(output_mode(self.output_exclusive));
        cx.notify();
    }
}

/// The hover note behind the Experimental badge and its issue button. Same
/// card the track info chip's tooltip uses, so the explanation reads the
/// same wherever it pops up.
struct ExperimentalTooltip(SharedString);

impl Render for ExperimentalTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .p(tokens::SPACE_SM)
            .max_w(px(320.))
            .rounded(tokens::RADIUS)
            .border_1()
            .border_color(palette::border())
            .bg(palette::bg_menu_opaque())
            .shadow_md()
            .text_xs()
            .text_color(palette::text())
            .child(self.0.clone())
    }
}

/// Percent-encode a string for a GitHub issue URL's query. Only the handful
/// of characters that break a query string; anything else passes through,
/// since the issue form is forgiving and over-encoding makes the URL
/// unreadable in logs.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// The exclusive toggle as the output layer's mode. The two device lists
/// don't share ids, so which one to ask for follows the toggle rather than
/// what happens to be running.
pub(super) fn output_mode(exclusive: bool) -> output::Mode {
    if exclusive {
        output::Mode::Exclusive
    } else {
        output::Mode::Shared
    }
}
