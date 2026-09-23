//! The Playback settings page: how the queue arranges and extends itself,
//! what a launch brings back, the step keys, ratings, the live buffer, and
//! stream capture.

use super::*;

/// The orders shuffle can put the upcoming queue in, and what each one
/// means. Read on the Playback page.
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

/// The strategies that refill a queue which has run dry (ADR 17).
///
/// Note how these differ from the orders above: every one of them is about
/// which tracks join the queue, and not one of them touches the order the
/// queue already has.
///
/// There's no Radio here. The Similar order does the radio draw when it runs
/// out, so it's part of that pick instead of a fourth strategy that only ever
/// made sense alongside it.
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
    /// The restore switch: straight into the file. Launch reads it there,
    /// so the flip is live for the next start without touching playback.
    fn set_restore_last_track(&mut self, on: bool, cx: &mut Context<Self>) {
        self.restore_last_track = on;
        Settings::update(move |s| s.restore_last_track = on);
        cx.notify();
    }

    /// The rating scale: through the live static, so every open rating
    /// column redraws, and into the file.
    fn set_rating_style(&mut self, style: RatingStyle, cx: &mut Context<Self>) {
        self.rating_style = style;
        settings::set_rating_style(style, cx);
        Settings::update(move |s| s.look.bundle.appearance.rating_style = style);
        cx.notify();
    }

    /// The unrated dots, the scale's sibling: same live-static route.
    fn set_rating_dots(&mut self, on: bool, cx: &mut Context<Self>) {
        self.rating_dots = on;
        settings::set_rating_dots(on, cx);
        Settings::update(move |s| s.look.bundle.appearance.rating_dots = on);
        cx.notify();
    }

    /// The live buffer row's description: what the buffer is, then what
    /// the length it's set to actually weighs.
    ///
    /// The weight is the only reason the top of the range is where it is,
    /// and a number of seconds says nothing about it. Two reference rates
    /// bracket what stations broadcast at, so the line holds whatever the
    /// listener is about to tune into. A station already playing gets a
    /// third clause at its own measured rate, which is the one figure on
    /// the row that's about this listener's own memory rather than radio
    /// in general. The ceiling comes last, because every weight above it
    /// is what the length asks for rather than what it gets.
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

        // The ceiling under all of it, which the weights above can be well
        // past: a lossless station at twelve hours asks for more memory
        // than most machines have. Named only when the machine said how
        // much it has, since the sentence is about this machine's memory
        // and there's nothing honest to say about a figure we guessed.
        if rox_playback::memory::total().is_some() {
            text.push(' ');
            text.push_str(&rox_i18n::t!(
                "settings-playback-live-buffer-cap",
                cap = human_size(rox_playback::memory::live_buffer_cap() as u64),
            ));
        }

        text.into()
    }

    /// The Playback page: how the queue arranges and extends itself, what a
    /// launch brings back, and how tracks get rated along the way. Split off
    /// the Application page so the music behavior reads together instead of
    /// between window and data rows.
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

    /// How far one step key moves the playhead.
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

    /// How long a step taken while paused plays for.
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

    /// What the transport's shuffle and continue buttons are doing when
    /// they're on.
    ///
    /// Here rather than behind the buttons themselves, where these two
    /// lists used to be as press-and-hold menus. Both are a pick
    /// between strategies that differ in kind, and the difference is the
    /// whole question: a menu of four bare words next to a menu of two bare
    /// words made the two buttons read as the same button twice. A settings
    /// row has room to say what each one does, and the button goes back to
    /// being a plain on/off.
    fn playback_behavior_section(&self, q: &Query, cx: &mut Context<Self>) -> Section {
        // Similar needs vectors to sort by, and the switch that builds them
        // being on isn't enough: it permits the pass, it doesn't run it. The
        // mode stays listed either way, so it's discoverable and its own row
        // can say what's missing.
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

    /// Saving songs off a stream. Two controls and one warning, because the
    /// warning is the part someone has to read before turning it on: a
    /// station flips its title a few seconds either side of the audio
    /// switching, so what lands on disk carries a little of the song
    /// before it or the one after. It lives on the Playback page beside
    /// the live buffer because the two are one knob from the listener's
    /// side: a song longer than the buffer is never saved.
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
                // A saved song is only ever as long as the buffer it comes
                // out of, so a buffer shorter than what a station plays is
                // capture quietly doing nothing, with the setting that
                // decides it sitting in another section. Warn rather than
                // Bad, the same reading the ffmpeg note takes: nothing
                // failed, a capability is just out of reach where it stands.
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

                    // The vocabulary is the renamer's, so a pattern
                    // learned there reads the same here. What the tip
                    // adds is the three names a broadcast reads its own
                    // way.
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
                            // Reveal creates the folder if the first capture
                            // hasn't yet, so there's always something to open.
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

    /// The capture switch. The tee in the transport reads the file, so
    /// this writes it and then tells the service to look again.
    fn set_capture_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        self.capture_enabled = on;
        Settings::update(move |s| s.capture.enabled = on);
        rox_services::capture::apply();
        cx.notify();
    }

    /// Browse for the folder captures land in.
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

/// What `secs` of a stream running at `bytes_per_sec` weighs in memory,
/// which is what the live buffer setting is really spending. A bitrate in
/// kbps is kilobits over the wire, so the eight is the only arithmetic in
/// it and the rest is the app's own size formatter.
fn live_buffer_weight(bytes_per_sec: f64, secs: f64) -> String {
    human_size((bytes_per_sec * secs).max(0.0) as u64)
}

#[cfg(test)]
mod tests {
    use super::live_buffer_weight;

    /// What the live buffer row prints under its slider. The setting is a
    /// length of time and the cost is a weight of memory, so the line has
    /// to do that conversion out loud: ten minutes of a 128 kbps stream is
    /// 9.6 MB, and of a 320 kbps one it's two and a half times that.
    #[test]
    fn the_buffer_weighs_its_seconds_at_the_streams_rate() {
        let at = |kbps: f64| kbps * 1000.0 / 8.0;

        assert_eq!(live_buffer_weight(at(128.0), 600.0), "9.6 MB");
        assert_eq!(live_buffer_weight(at(320.0), 600.0), "24.0 MB");
        assert_eq!(live_buffer_weight(at(128.0), 30.0), "480 KB");
        assert_eq!(live_buffer_weight(at(320.0), 43200.0), "1.7 GB");
    }
}
