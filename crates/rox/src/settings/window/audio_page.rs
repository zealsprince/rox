//! The Audio settings page: crossfade, ReplayGain, the equalizer, and the
//! output backend with exclusive mode's device, rate, format and period rows.

use super::*;

/// The two base clocks, doubled and quadrupled. A card missing one falls back
/// to its nearest and reports that.
const RATES: &[u32] = &[44100, 48000, 88200, 96000, 176400, 192000];

/// Either side of the backend's 10 ms default.
const PERIODS_MS: &[f64] = &[2.5, 5.0, 10.0, 20.0, 40.0];

impl SettingsWindow {
    /// In the order the audio meets it: the chain first (ADR 19), then the
    /// backend.
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

    /// Zero is gapless. Anything else fades only where the music isn't
    /// continuous, so an album still splices.
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

    /// Through the player, which holds the live copy of the whole struct.
    fn set_replay_gain_save(&mut self, save: ReplayGainSave, cx: &mut Context<Self>) {
        self.playback
            .update(cx, |player, cx| player.set_replay_gain_save(save, cx));
        cx.notify();
    }

    /// On the way on, the prompt prices the backlog, since the work list is
    /// everything with no gain; declining turns the switch back off through
    /// `pass_refused`.
    fn set_replay_gain_auto(&mut self, on: bool, cx: &mut Context<Self>) {
        self.playback
            .update(cx, |player, cx| player.set_replay_gain_auto(on, cx));
        if on && self.rg_coverage.missing > 0 && self.rg_job.is_none() {
            let library = self.library.clone();
            pass_prompt::raise_for_switch(self, pass_prompt::Pass::ReplayGain, library, cx);
        }
        cx.notify();
    }

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

    pub(super) fn poll_measuring(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(RG_POLL).await;
                let live = this.update(cx, |this, cx| {
                    let was = this.rg_job.is_some();
                    this.rg_job = replaygain_job::progress(cx);
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

    /// ALSA and CoreAudio have been run on real hardware. WASAPI is written
    /// from the platform contract and shipped for testers.
    fn exclusive_experimental() -> bool {
        cfg!(target_os = "windows")
    }

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

    /// In the Output header: the caveat covers the whole backend, not the
    /// switch.
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

    /// The readout is the point: the rows above are requests, and ADR 19 has
    /// the UI state what was negotiated.
    fn output_section(&self, q: &Query, cx: &mut Context<Self>) -> Section {
        // No exclusive backend built: a switch that never does anything reads
        // as a hardware bug.
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

    /// In shared mode the server owns the rate, format and buffer, so these
    /// draw inert.
    fn exclusive_only(&self) -> bool {
        !self.output_exclusive || !output::exclusive_supported()
    }

    /// Following each file's rate plays a mixed-rate library without a
    /// resampler.
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

    /// Widest-available is right almost always; the pick is for a driver that
    /// prefers one.
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

    /// Rescan because the list is taken when the window opens.
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
        // On Linux the two backends' lists don't overlap: a Bluetooth headset
        // only exists in the sound server. The note says so, or a device reads
        // as lost.
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

    /// Nothing here is derived from the settings: fallback lines come from the
    /// backend, and the rate compares against the file, not the request.
    fn output_status_block(&self, cx: &mut Context<Self>) -> Div {
        let Some(status) = self.playback.read(cx).output_status() else {
            // An error with no stream means the last open failed, which must
            // not read as idle.
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
        // A failed claim is an error: exclusive is on and you aren't hearing
        // it. Resampling is only a warning.
        let tone = if negotiated.fallback.is_some() {
            panel::Tone::Bad
        } else if resampling {
            panel::Tone::Warn
        } else {
            panel::Tone::Good
        };
        // The experimental note goes in the banner too, for someone reading
        // only this line.
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
        // Expanded: this block gets a sentence per reason where the output
        // panel folds them into one line.
        panel::banner(tone, headline, status.lines(true, true))
    }

    /// The player rebuilds its session onto the other backend on the spot, so
    /// no restart.
    fn set_output_exclusive(&mut self, on: bool, cx: &mut Context<Self>) {
        self.output_exclusive = on;
        self.playback
            .update(cx, |player, cx| player.set_exclusive_output(on, cx));
        self.output_devices = output::devices(output_mode(on));
        cx.notify();
    }

    fn set_output_device(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        self.playback
            .update(cx, |player, cx| player.set_output_device(id, cx));
        cx.notify();
    }

    fn rescan_output_devices(&mut self, cx: &mut Context<Self>) {
        self.output_devices = output::devices(output_mode(self.output_exclusive));
        cx.notify();
    }
}

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

/// Every byte outside the unreserved set goes out as `%XX`.
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

/// The two device lists don't share ids, so this follows the toggle, not what's
/// running.
pub(super) fn output_mode(exclusive: bool) -> output::Mode {
    if exclusive {
        output::Mode::Exclusive
    } else {
        output::Mode::Shared
    }
}
