//! The Playback settings page: queue order and continuation, step keys, the
//! live buffer, stream capture, startup and ratings.

use super::*;

fn shuffle_modes() -> Vec<panel::ModeSpec<ShuffleMode>> {
    vec![
        panel::ModeSpec {
            label: rox_i18n::t!("settings-playback-shuffle-random"),
            description: rox_i18n::t!("settings-playback-shuffle-random.description"),
            value: ShuffleMode::Random,
        },
        panel::ModeSpec {
            label: rox_i18n::t!("settings-playback-shuffle-similar"),
            description: rox_i18n::t!("settings-playback-shuffle-similar.description"),
            value: ShuffleMode::Similar,
        },
    ]
}

/// The strategies that refill a queue which has run dry (ADR 17). They pick
/// which tracks join, never the order. No Radio: the Similar order does the
/// radio draw itself.
fn continuation_modes() -> Vec<panel::ModeSpec<continuation::Mode>> {
    vec![
        panel::ModeSpec {
            label: rox_i18n::t!("settings-playback-continuation-off"),
            description: rox_i18n::t!("settings-playback-continuation-off.description"),
            value: continuation::Mode::Off,
        },
        panel::ModeSpec {
            label: rox_i18n::t!("settings-playback-continuation-continue"),
            description: rox_i18n::t!("settings-playback-continuation-continue.description"),
            value: continuation::Mode::Continue,
        },
        panel::ModeSpec {
            label: rox_i18n::t!("settings-playback-continuation-weighted"),
            description: rox_i18n::t!("settings-playback-continuation-weighted.description"),
            value: continuation::Mode::Weighted,
        },
    ]
}

impl SettingsWindow {
    fn set_restore_last_track(&mut self, on: bool, cx: &mut Context<Self>) {
        self.restore_last_track = on;
        Settings::update(move |s| s.restore_last_track = on);
        cx.notify();
    }

    fn set_rating_style(&mut self, style: RatingStyle, cx: &mut Context<Self>) {
        self.rating_style = style;
        settings::set_rating_style(style, cx);
        Settings::update(move |s| s.look.bundle.appearance.rating_style = style);
        cx.notify();
    }

    fn set_rating_dots(&mut self, on: bool, cx: &mut Context<Self>) {
        self.rating_dots = on;
        settings::set_rating_dots(on, cx);
        Settings::update(move |s| s.look.bundle.appearance.rating_dots = on);
        cx.notify();
    }

    /// Seconds say nothing about cost, so the row names the weight at two
    /// reference bitrates, the playing station's own rate, and the memory
    /// ceiling.
    fn live_buffer_description(&self, cx: &mut Context<Self>) -> SharedString {
        let player = self.playback.read(cx);
        let secs = player.live_buffer_secs() as f64;
        let weight = |bytes_per_sec: f64| live_buffer_weight(bytes_per_sec, secs);
        let mut text = format!(
            "{} {}",
            rox_i18n::t!("settings-playback-live-buffer.description"),
            rox_i18n::t!(
                "settings-playback-live-buffer-memory",
                low = weight(128_000.0 / 8.0),
                high = weight(320_000.0 / 8.0),
            )
        );

        if let Some(rate) = player.live_bytes_per_sec().filter(|rate| *rate > 0.0) {
            text.push(' ');
            text.push_str(&rox_i18n::t!(
                "settings-playback-live-buffer-playing",
                size = weight(rate),
            ));
        }

        // Only when the machine reported its memory: nothing honest to say
        // about a guessed figure.
        if rox_playback::memory::total().is_some() {
            text.push(' ');
            text.push_str(&rox_i18n::t!(
                "settings-playback-live-buffer-cap",
                cap = human_size(rox_playback::memory::live_buffer_cap() as u64),
            ));
        }

        text.into()
    }

    pub(super) fn playback_page(&self, q: &Query, cx: &mut Context<Self>) -> PageBody {
        PageBody::new()
            .section(self.playback_behavior_section(q, cx))
            .section(Section::new(
                q,
                icons::MOVE_HORIZONTAL,
                rox_i18n::t!("settings-playback-section-stepping"),
                None,
                |rows| {
                    rows.custom(
                        &["step", "frame", "nudge", "comma", "dot", "fine", "seek"],
                        || self.step_row(cx).into_any_element(),
                    )
                    .custom(
                        &["step", "preview", "audition", "blip", "hear", "paused"],
                        || self.step_preview_row(cx).into_any_element(),
                    )
                },
            ))
            .section(Section::new(
                q,
                icons::RADIO,
                rox_i18n::t!("settings-playback-section-streaming"),
                None,
                |rows| {
                    rows.row_dyn(
                        &["live", "buffer", "radio", "rewind", "timeshift"],
                        rox_i18n::t!("settings-playback-live-buffer"),
                        Some(self.live_buffer_description(cx)),
                        settings_ui::scalar(
                            &self.live_buffer_scrub,
                            &self.value_edit,
                            self.playback.read(cx).live_buffer_secs() as f32,
                            settings_ui::span_secs(
                                settings::LIVE_BUFFER_SECS_MIN as f32,
                                settings::LIVE_BUFFER_SECS_MAX as f32,
                            )
                            .log(),
                            |this: &mut Self, secs, cx| {
                                this.playback.update(cx, |player, cx| {
                                    player.set_live_buffer_secs(secs.round() as u32, cx)
                                });
                                cx.notify();
                            },
                            cx,
                        ),
                    )
                },
            ))
            .section(self.capture_section(q, cx))
            .section(Section::new(
                q,
                icons::PLAY,
                rox_i18n::t!("settings-playback-section-startup"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-playback-restore-last-session",
                        &["resume", "reopen", "track", "queue"],
                        panel::toggle(self.restore_last_track, Self::set_restore_last_track, cx),
                    )
                },
            ))
            .section(Section::new(
                q,
                icons::STAR,
                rox_i18n::t!("settings-playback-section-ratings"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-playback-rating-scale",
                        &["stars", "numeric"],
                        panel::choices_shared(
                            &[
                                (
                                    rox_i18n::t!("settings-playback-rating-scale-stars"),
                                    RatingStyle::Stars,
                                ),
                                (
                                    rox_i18n::t!("settings-playback-rating-scale-numeric"),
                                    RatingStyle::Numeric,
                                ),
                            ],
                            self.rating_style,
                            Self::set_rating_style,
                            cx,
                        ),
                    )
                    .keyed(
                        "settings-playback-unrated-dots",
                        &["stars", "empty"],
                        panel::toggle(self.rating_dots, Self::set_rating_dots, cx),
                    )
                },
            ))
    }

    fn step_row(&self, cx: &mut Context<Self>) -> Div {
        panel::setting_row(
            rox_i18n::t!("settings-playback-step"),
            Some(rox_i18n::t!("settings-playback-step.description")),
            settings_ui::scalar(
                &self.step_scrub,
                &self.value_edit,
                self.playback.read(cx).step_ms(),
                settings_ui::span(
                    rox_core::settings::STEP_MS_MIN,
                    rox_core::settings::STEP_MS_MAX,
                    " ms",
                )
                .decimals(0)
                .hard(),
                |this: &mut Self, ms, cx| {
                    this.playback
                        .update(cx, |player, cx| player.set_step_ms(ms, cx));
                    cx.notify();
                },
                cx,
            ),
        )
    }

    fn step_preview_row(&self, cx: &mut Context<Self>) -> Div {
        panel::setting_row(
            rox_i18n::t!("settings-playback-step-preview"),
            Some(rox_i18n::t!("settings-playback-step-preview.description")),
            settings_ui::scalar(
                &self.step_preview_scrub,
                &self.value_edit,
                self.playback.read(cx).step_preview_ms(),
                settings_ui::span(
                    rox_core::settings::STEP_PREVIEW_MS_MIN,
                    rox_core::settings::STEP_PREVIEW_MS_MAX,
                    " ms",
                )
                .decimals(0)
                .hard(),
                |this: &mut Self, ms, cx| {
                    this.playback
                        .update(cx, |player, cx| player.set_step_preview_ms(ms, cx));
                    cx.notify();
                },
                cx,
            ),
        )
    }

    /// Here rather than as press-and-hold menus on the transport buttons: the
    /// strategies differ in kind, and a settings row has room to say how.
    fn playback_behavior_section(&self, q: &Query, cx: &mut Context<Self>) -> Section {
        // The switch permits the pass but doesn't run it. The mode stays listed
        // so its row can say what's missing.
        let analyzed = settings::similarity_ready();
        let shuffle_mode = self.playback.read(cx).shuffle_mode();
        let continuation = self.playback.read(cx).continuation_mode();
        Section::new(
            q,
            icons::LIST_MUSIC,
            rox_i18n::t!("settings-playback-section-queue"),
            None,
            move |rows| {
                rows.custom(
                    &[
                        "shuffle",
                        "order",
                        "random",
                        "similar",
                        "sound",
                        "play order",
                    ],
                    || {
                        panel::setting_block(
                            rox_i18n::t!("settings-playback-play-order"),
                            Some(rox_i18n::t!("settings-playback-play-order.description")),
                            None,
                            panel::mode_list(
                                &shuffle_modes(),
                                shuffle_mode,
                                move |mode| mode != ShuffleMode::Similar || analyzed,
                                |this: &mut Self, mode, cx| {
                                    this.playback
                                        .update(cx, |player, cx| player.set_shuffle_mode(mode, cx));
                                    cx.notify();
                                },
                                cx,
                            ),
                        )
                        .into_any_element()
                    },
                )
                .custom(
                    &[
                        "continue",
                        "continuation",
                        "endless",
                        "queue",
                        "radio",
                        "weighted",
                        "keep playing",
                    ],
                    || {
                        panel::setting_block(
                            rox_i18n::t!("settings-playback-keep-playing"),
                            Some(rox_i18n::t!("settings-playback-keep-playing.description")),
                            None,
                            panel::mode_list(
                                &continuation_modes(),
                                continuation,
                                |_| true,
                                |this: &mut Self, mode, cx| {
                                    this.playback.update(cx, |player, cx| {
                                        player.set_continuation_mode(mode, cx)
                                    });
                                    cx.notify();
                                },
                                cx,
                            ),
                        )
                        .into_any_element()
                    },
                )
            },
        )
    }

    /// The warning matters: a station flips its title a few seconds off the
    /// audio switch, so saved songs carry a little of their neighbours. Beside
    /// the live buffer because a song longer than the buffer is never saved.
    fn capture_section(&self, q: &Query, cx: &mut Context<Self>) -> Section {
        Section::new(
            q,
            icons::DOWNLOAD,
            rox_i18n::t!("settings-playback-section-capture"),
            None,
            |rows| {
                let buffer = self.playback.read(cx).live_buffer_secs();

                rows.keyed(
                    "settings-playback-capture-enable",
                    &[
                        "capture", "record", "save", "rip", "radio", "station", "stream",
                    ],
                    panel::toggle(self.capture_enabled, Self::set_capture_enabled, cx),
                )
                // Capture can't save a song longer than the buffer. Warn, not
                // Bad: nothing failed.
                .when(
                    self.capture_enabled && buffer < settings::DEFAULT_LIVE_BUFFER_SECS,
                    |rows| {
                        rows.custom(&["capture", "buffer", "short", "length"], || {
                            panel::banner(
                                panel::Tone::Warn,
                                rox_i18n::t!("settings-playback-capture-buffer-title"),
                                vec![rox_i18n::t!(
                                    "settings-playback-capture-buffer-note",
                                    buffer = settings_ui::fmt_duration_secs(buffer as f32),
                                    default = settings_ui::fmt_duration_secs(
                                        settings::DEFAULT_LIVE_BUFFER_SECS as f32
                                    ),
                                )],
                            )
                            .into_any_element()
                        })
                    },
                )
                .when(self.capture_enabled, |rows| {
                    let folder = self.capture_folder.clone();

                    // The renamer's vocabulary; the tip adds the three names a
                    // broadcast reads its own way.
                    let notes = vec![
                        rox_i18n::t!("settings-playback-capture-pattern-station"),
                        rox_i18n::t!("settings-playback-capture-pattern-source"),
                        rox_i18n::t!("settings-playback-capture-pattern-date"),
                    ];
                    let sample =
                        capture::Sample::playing(self.playback.read(cx)).unwrap_or_default();
                    let preview =
                        capture::preview(self.capture_pattern.read(cx).value().trim(), &sample);
                    let pattern_input = self.capture_pattern.clone();
                    let album_input = self.capture_album.clone();

                    rows.row_dyn(
                        &[
                            "capture",
                            "folder",
                            "where",
                            "destination",
                            "save",
                            "reveal",
                        ],
                        rox_i18n::t!("settings-playback-capture-folder"),
                        Some(self.capture_folder.display().to_string().into()),
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            .child(small_button(
                                rox_i18n::t!("settings-playback-capture-choose"),
                                icons::FOLDER,
                                false,
                                cx.listener(|this, _, window, cx| {
                                    this.pick_capture_folder(window, cx)
                                }),
                            ))
                            .child(small_button(
                                rox_i18n::t!("settings-common-reveal"),
                                icons::FOLDER,
                                false,
                                move |_, _, cx| {
                                    if let Err(e) = std::fs::create_dir_all(&folder) {
                                        log::warn!("capture: creating the folder failed: {e}");
                                        return;
                                    }
                                    cx.reveal_path(&folder);
                                },
                            )),
                    )
                    .custom(
                        &[
                            "capture",
                            "pattern",
                            "name",
                            "naming",
                            "folder",
                            "structure",
                        ],
                        move || {
                            let note = match preview {
                                Ok(name) => PatternNote::Preview(rox_i18n::t!(
                                    "settings-playback-capture-pattern-preview",
                                    name = name
                                )),

                                Err(e) => PatternNote::Wrong(e.into()),
                            };

                            panel::setting_block(
                                rox_i18n::t!("settings-playback-capture-pattern"),
                                Some(rox_i18n::t!(
                                    "settings-playback-capture-pattern.description"
                                )),
                                None,
                                panel::pattern_input(
                                    "capture-pattern",
                                    &pattern_input,
                                    rox_core::pattern::PLACEHOLDERS,
                                    notes,
                                    Some(note),
                                )
                                .flex_1()
                                .min_w_0(),
                            )
                            .into_any_element()
                        },
                    )
                    .custom(
                        &["capture", "album", "tag", "station", "singles", "radio"],
                        move || {
                            panel::setting_block(
                                rox_i18n::t!("settings-playback-capture-album"),
                                Some(rox_i18n::t!("settings-playback-capture-album.description")),
                                None,
                                Input::new(&album_input).small(),
                            )
                            .into_any_element()
                        },
                    )
                })
            },
        )
    }

    /// The transport's tee reads the file, so write it and tell the service to
    /// look again.
    fn set_capture_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        self.capture_enabled = on;
        Settings::update(move |s| s.capture.enabled = on);
        rox_services::capture::apply();
        cx.notify();
    }

    fn pick_capture_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });

        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = rx.await else {
                return;
            };
            let Some(folder) = paths.pop() else {
                return;
            };

            this.update(cx, |this, cx| {
                this.capture_folder = folder.clone();
                Settings::update(move |s| s.capture.folder = folder.clone());
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

fn live_buffer_weight(bytes_per_sec: f64, secs: f64) -> String {
    human_size((bytes_per_sec * secs).max(0.0) as u64)
}

#[cfg(test)]
mod tests {
    use super::live_buffer_weight;

    /// Ten minutes of a 128 kbps stream is 9.6 MB.
    #[test]
    fn the_buffer_weighs_its_seconds_at_the_streams_rate() {
        let at = |kbps: f64| kbps * 1000.0 / 8.0;

        assert_eq!(live_buffer_weight(at(128.0), 600.0), "9.6 MB");
        assert_eq!(live_buffer_weight(at(320.0), 600.0), "24.0 MB");
        assert_eq!(live_buffer_weight(at(128.0), 30.0), "480 KB");
        assert_eq!(live_buffer_weight(at(320.0), 43200.0), "1.7 GB");
    }
}
