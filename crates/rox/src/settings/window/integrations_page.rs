//! The Integrations settings page: the scrobble destinations (Last.fm,
//! ListenBrainz, Libre.fm) in their shared shape, Discord Rich Presence, the
//! Icecast sink, and the ffmpeg check Convert runs. The Last.fm history
//! imports sit here with their destination.

use super::*;

/// The verbs a connection that authorizes in the browser answers to.
/// Last.fm and Libre.fm both connect this way, so one strip serves them
/// and only where the presses land differs.
struct BrowserAuth {
    phase: AuthPhase,
    connected: bool,
    username: String,
    /// A session under another build's api key, which only Last.fm files
    /// sessions by: the fix is a connect here, and saying so beats a bare
    /// "not connected" for someone who knows they already did this.
    elsewhere: bool,
    /// Whether Connect can go at all: a Last.fm build without its own api
    /// identity waits for the user's pair.
    ready: bool,
    begin: fn(&mut SettingsWindow, &mut Context<SettingsWindow>),
    finish: fn(&mut SettingsWindow, &mut Context<SettingsWindow>),
    disconnect: fn(&mut SettingsWindow, &mut Context<SettingsWindow>),
}

/// One scrobble destination as the Integrations page draws it. The three
/// services connect differently (Last.fm and Libre.fm through the
/// browser, ListenBrainz with a pasted token) and Last.fm alone carries
/// hearts, but the section around that is the same for all of them:
/// whatever the flow needs typed, then the connect row with the status,
/// the intro under it and the actions that move it along. This is what
/// each service hands [`SettingsWindow::destination_section`].
struct Destination {
    /// The service's name: the section label, and the `$service` every
    /// shared line takes.
    name: SharedString,
    icon: &'static str,
    /// Search terms beyond the name: what else someone might call it.
    keywords: &'static [&'static str],
    intro: SharedString,
    /// Rows above the connect strip: credentials the flow needs typed.
    fields: Vec<AnyElement>,
    status: SharedString,
    /// The buttons on the strip's right, disconnect first where two show.
    actions: Vec<AnyElement>,
    /// What the section header carries on its right, if anything.
    trailing: Option<AnyElement>,
}

/// The connect strip's status line for a browser flow that isn't
/// connected, one wording per phase. Only the refusal names the service.
fn connect_phase_line(phase: &AuthPhase, service: &SharedString) -> SharedString {
    match phase {
        AuthPhase::Idle => rox_i18n::t!("settings-integrations-scrobble-status-not-connected"),
        AuthPhase::Requesting => rox_i18n::t!("settings-integrations-scrobble-status-requesting"),
        AuthPhase::Waiting(_) => rox_i18n::t!("settings-integrations-scrobble-status-waiting"),
        AuthPhase::Confirming => rox_i18n::t!("settings-integrations-scrobble-status-confirming"),
        AuthPhase::Rejected => rox_i18n::t!(
            "settings-integrations-scrobble-status-rejected",
            service = service.to_string()
        ),
        AuthPhase::Failed(e) => rox_i18n::t!(
            "settings-integrations-scrobble-status-failed",
            error = e.clone()
        ),
    }
}

impl SettingsWindow {
    fn set_discord_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        self.discord_enabled = on;
        Settings::update(move |s| s.accounts.discord.enabled = on);
        self.discord.update(cx, |d, cx| d.reload_config(cx));
        cx.notify();
    }

    fn set_discord_show_lastfm_button(&mut self, on: bool, cx: &mut Context<Self>) {
        self.discord_show_lastfm_button = on;
        Settings::update(move |s| s.accounts.discord.show_lastfm_button = on);
        self.discord.update(cx, |d, cx| d.reload_config(cx));
        cx.notify();
    }

    fn set_discord_show_youtube_button(&mut self, on: bool, cx: &mut Context<Self>) {
        self.discord_show_youtube_button = on;
        Settings::update(move |s| s.accounts.discord.show_youtube_button = on);
        self.discord.update(cx, |d, cx| d.reload_config(cx));
        cx.notify();
    }

    fn set_discord_status_line(&mut self, line: DiscordStatusLine, cx: &mut Context<Self>) {
        self.discord_status_line = line;
        Settings::update(move |s| s.accounts.discord.status_line = line);
        self.discord.update(cx, |d, cx| d.reload_config(cx));
        cx.notify();
    }

    /// The Favourites section's header control: start the loved-tracks
    /// import, or stop the one that's running. The ReplayGain section's
    /// control in every respect but the work, since it's the same shape of
    /// thing: a job started from a page that doesn't have to stay open for
    /// it. Inert with its reason on the line below, so a disconnected
    /// account reads as a state rather than a dead button.
    fn import_control(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(job) = import::progress(cx) {
            let stopping = job.stopping();
            return small_button(
                if stopping {
                    rox_i18n::t!("settings-common-stopping")
                } else {
                    rox_i18n::t!("settings-common-stop")
                },
                icons::STOP,
                stopping,
                cx.listener(|_, _, _, cx| import::stop(cx)),
            )
            .into_any_element();
        }
        small_button(
            rox_i18n::t!("settings-integrations-lastfm-import-loved"),
            icons::DOWNLOAD,
            import::blocked_reason(cx).is_some(),
            cx.listener(|this, _, _, cx| {
                import::start(this.library.clone(), this.scrobbler.clone(), cx);
            }),
        )
        .into_any_element()
    }

    /// The Play Counts row's control: start the Last.fm play-count backfill,
    /// or stop the one that's running.
    fn plays_import_control(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(job) = plays_import::progress(cx) {
            let stopping = job.stopping();
            return small_button(
                if stopping {
                    rox_i18n::t!("settings-common-stopping")
                } else {
                    rox_i18n::t!("settings-common-stop")
                },
                icons::STOP,
                stopping,
                cx.listener(|_, _, _, cx| plays_import::stop(cx)),
            )
            .into_any_element();
        }
        small_button(
            rox_i18n::t!("settings-integrations-lastfm-import-plays-button"),
            icons::DOWNLOAD,
            plays_import::blocked_reason(cx).is_some(),
            cx.listener(|this, _, _, cx| {
                plays_import::start(this.library.clone(), cx);
            }),
        )
        .into_any_element()
    }

    /// The Integrations page: everything rox talks to that isn't the
    /// library or the audio device. The switch and threshold every
    /// scrobble destination shares, then the destinations one section
    /// each in the same shape, Discord Rich Presence, the icecast sink,
    /// and the ffmpeg binary Convert runs.
    pub(super) fn integrations_page(&self, q: &Query, cx: &mut Context<Self>) -> PageBody {
        let (scrobbling, threshold) = {
            let s = self.scrobbler.read(cx);
            (s.scrobbling(), s.threshold())
        };
        PageBody::new()
            .section(Section::new(
                q,
                icons::UPLOAD,
                rox_i18n::t!("settings-integrations-section-scrobbling"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-integrations-scrobble-tracks",
                        &["Last.fm", "Libre.fm", "ListenBrainz", "listens", "history"],
                        panel::toggle(
                            scrobbling,
                            |this: &mut Self, on, cx| {
                                this.scrobbler.update(cx, |s, cx| s.set_scrobbling(on, cx));
                                cx.notify();
                            },
                            cx,
                        ),
                    )
                    // The threshold is when a listen counts, and with
                    // nothing counting it's a number about nothing.
                    .when(scrobbling, |rows| {
                        rows.keyed(
                            "settings-integrations-scrobble-threshold",
                            &["Last.fm", "Libre.fm", "ListenBrainz", "percent"],
                            settings_ui::slider_edit(
                                &self.threshold_scrub,
                                &self.value_edit,
                                threshold,
                                |this: &mut Self, fraction, cx| {
                                    this.scrobbler
                                        .update(cx, |s, cx| s.set_threshold(fraction, cx));
                                    cx.notify();
                                },
                                cx,
                            ),
                        )
                    })
                },
            ))
            .section(self.lastfm_section(q, cx))
            .section(self.listenbrainz_section(q, cx))
            .section(self.librefm_section(q, cx))
            .section(Section::new(
                q,
                icons::GLOBE,
                rox_i18n::t!("settings-integrations-section-discord"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-integrations-discord-enable",
                        &["status", "now playing"],
                        panel::toggle(self.discord_enabled, Self::set_discord_enabled, cx),
                    )
                    // The card's own contents live on a presence that's
                    // being shown.
                    .when(self.discord_enabled, |rows| {
                        let first = self.discord_first_line.clone();
                        let second = self.discord_second_line.clone();
                        let hover = self.discord_hover_line.clone();
                        let presence = self.discord.read(cx);
                        let first_note = presence_line_note(
                            presence.preview_line(first.read(cx).value().trim()),
                        );
                        let second_note = presence_line_note(
                            presence.preview_line(second.read(cx).value().trim()),
                        );
                        let hover_note = presence_line_note(
                            presence.preview_line(hover.read(cx).value().trim()),
                        );

                        rows.custom(
                            &["discord", "line", "pattern", "title", "artist", "template"],
                            move || {
                                presence_line_block(
                                    "discord-first-line",
                                    rox_i18n::t!("settings-integrations-discord-first-line"),
                                    rox_i18n::t!(
                                        "settings-integrations-discord-first-line.description"
                                    ),
                                    &first,
                                    first_note,
                                )
                            },
                        )
                        .custom(
                            &["discord", "line", "pattern", "album", "template"],
                            move || {
                                presence_line_block(
                                    "discord-second-line",
                                    rox_i18n::t!("settings-integrations-discord-second-line"),
                                    rox_i18n::t!(
                                        "settings-integrations-discord-second-line.description"
                                    ),
                                    &second,
                                    second_note,
                                )
                            },
                        )
                        .custom(
                            &[
                                "discord", "hover", "tooltip", "artwork", "cover", "quality",
                                "template",
                            ],
                            move || {
                                presence_line_block(
                                    "discord-hover-line",
                                    rox_i18n::t!("settings-integrations-discord-hover-line"),
                                    rox_i18n::t!(
                                        "settings-integrations-discord-hover-line.description"
                                    ),
                                    &hover,
                                    hover_note,
                                )
                            },
                        )
                        .keyed(
                            "settings-integrations-discord-status-line",
                            &["member list", "name", "status", "beside"],
                            panel::picker(
                                "discord-status-line",
                                self.discord_status_line,
                                vec![
                                    (
                                        DiscordStatusLine::App,
                                        rox_i18n::t!(
                                            "settings-integrations-discord-status-line-app"
                                        ),
                                    ),
                                    (
                                        DiscordStatusLine::First,
                                        rox_i18n::t!(
                                            "settings-integrations-discord-status-line-first"
                                        ),
                                    ),
                                    (
                                        DiscordStatusLine::Second,
                                        rox_i18n::t!(
                                            "settings-integrations-discord-status-line-second"
                                        ),
                                    ),
                                ],
                                false,
                                Self::set_discord_status_line,
                                cx,
                            ),
                        )
                        .keyed(
                            "settings-integrations-discord-show-lastfm",
                            &["link", "profile"],
                            panel::toggle(
                                self.discord_show_lastfm_button,
                                Self::set_discord_show_lastfm_button,
                                cx,
                            ),
                        )
                        .keyed(
                            "settings-integrations-discord-show-youtube",
                            &["link", "video"],
                            panel::toggle(
                                self.discord_show_youtube_button,
                                Self::set_discord_show_youtube_button,
                                cx,
                            ),
                        )
                    })
                },
            ))
            .section(self.icecast_section(q, cx))
            // This row stays put when ffmpeg is missing, unlike every other
            // Convert surface: it's the one place that can fix the absence.
            .section(Section::new(
                q,
                icons::AUDIO_LINES,
                rox_i18n::t!("settings-integrations-section-conversion"),
                None,
                |rows| {
                    let flatpak = rox_core::install::kind() == rox_core::install::Kind::Flatpak;
                    rows.keyed(
                        "settings-integrations-ffmpeg-binary",
                        &["ffmpeg", "convert", "encoder", "binary", "test"],
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            .child(Input::new(&self.ffmpeg_path).w(px(240.)))
                            .child(small_button(
                                rox_i18n::t!("settings-integrations-ffmpeg-test"),
                                icons::FLASK,
                                false,
                                cx.listener(|this, _, _, cx| this.test_ffmpeg(cx)),
                            )),
                    )
                    // The callout the test produces, in the output status
                    // block's register: what the binary returned, and
                    // whether that's fine, readable before any of it is.
                    .when_some(self.ffmpeg_test.as_ref(), |rows, answer| {
                        rows.custom(&["ffmpeg", "convert", "test", "version"], || {
                            match answer {
                                Ok(version) => panel::banner(
                                    panel::Tone::Good,
                                    version.clone(),
                                    vec![rox_i18n::t!("settings-integrations-ffmpeg-ok-note")],
                                ),
                                Err(reason) => panel::banner(
                                    panel::Tone::Bad,
                                    rox_i18n::t!("settings-integrations-ffmpeg-fail-title"),
                                    vec![
                                        reason.clone().into(),
                                        rox_i18n::t!("settings-integrations-ffmpeg-fail-note"),
                                    ],
                                ),
                            }
                            .into_any_element()
                        })
                    })
                    // The passive note keeps covering the case where nothing
                    // was pressed, in the same banner dress as the test's
                    // result; once a test has run, its result says it better.
                    // Warn rather than Bad: nothing failed, a capability is
                    // just absent.
                    .when(
                        !convert::available() && self.ffmpeg_test.is_none(),
                        |rows| {
                            // Inside a Flatpak "install ffmpeg" is the wrong
                            // advice: the host's binary can't run in the
                            // sandbox. That note points at the data folder
                            // instead, which convert::binary() checks.
                            let note = if flatpak {
                                rox_i18n::t!("settings-integrations-ffmpeg-missing-note-flatpak")
                            } else {
                                rox_i18n::t!("settings-integrations-ffmpeg-missing-note")
                            };
                            rows.custom(&["ffmpeg", "convert", "missing"], || {
                                panel::banner(
                                    panel::Tone::Warn,
                                    rox_i18n::t!("settings-integrations-ffmpeg-missing-title"),
                                    vec![note],
                                )
                                .into_any_element()
                            })
                        },
                    )
                    // The folder a static build goes into, opened from here
                    // so nobody has to find ~/.var/app by hand. Only in a
                    // Flatpak: everywhere else PATH is the answer, and the
                    // folder is a detail. gpui's Linux reveal goes through
                    // the OpenURI portal, so it works from inside the sandbox.
                    .when(flatpak, |rows| {
                        rows.custom(
                            &["ffmpeg", "convert", "flatpak", "data", "folder", "reveal"],
                            || {
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .child(small_button(
                                        rox_i18n::t!("settings-integrations-ffmpeg-reveal"),
                                        icons::FOLDER,
                                        false,
                                        move |_, _, cx| {
                                            cx.reveal_path(&settings::data_dir());
                                        },
                                    ))
                                    .into_any_element()
                            },
                        )
                    })
                },
            ))
    }

    /// One scrobble destination's section, the shape all three share: the
    /// fields the flow needs, the connect row, and whatever rows the
    /// service adds below that.
    fn destination_section(
        &self,
        q: &Query,
        destination: Destination,
        extra: impl FnOnce(Rows) -> Rows,
    ) -> Section {
        let Destination {
            name,
            icon,
            keywords,
            intro,
            fields,
            status,
            actions,
            trailing,
        } = destination;
        // The connect row reads like the rows under it: the status stands
        // as its label, the intro sits beneath as its description, and the
        // actions take the control's place. Fields come first, so a token
        // or an api pair is above the line that says what pasting it does.
        let account = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .children(fields)
            .child(panel::setting_row(
                status,
                Some(intro),
                div()
                    .flex()
                    .flex_row()
                    .flex_none()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .children(actions),
            ));
        let mut terms: Vec<&str> = vec![name.as_ref(), "account", "connect", "login", "scrobble"];
        terms.extend_from_slice(keywords);
        Section::new(q, icon, name.clone(), trailing, |rows| {
            extra(rows.custom(&terms, || account.into_any_element()))
        })
    }

    /// The connect strip for a service that authorizes in the browser: the
    /// status follows the phase, and the one button offers the step that
    /// moves it along.
    fn browser_auth_strip(
        &self,
        service: &SharedString,
        auth: BrowserAuth,
        cx: &mut Context<Self>,
    ) -> (SharedString, Vec<AnyElement>) {
        let status: SharedString = if auth.connected {
            rox_i18n::t!(
                "settings-integrations-scrobble-status-connected",
                username = auth.username
            )
        } else if auth.elsewhere && matches!(auth.phase, AuthPhase::Idle) {
            rox_i18n::t!("settings-integrations-lastfm-status-elsewhere")
        } else {
            connect_phase_line(&auth.phase, service)
        };
        let action = if auth.connected {
            let disconnect = auth.disconnect;
            small_button(
                rox_i18n::t!("settings-integrations-scrobble-disconnect"),
                icons::CLOSE,
                false,
                cx.listener(move |this, _, _, cx| disconnect(this, cx)),
            )
        } else {
            match auth.phase {
                AuthPhase::Requesting | AuthPhase::Confirming => small_button(
                    rox_i18n::t!("settings-integrations-scrobble-working"),
                    icons::REFRESH_CW,
                    true,
                    |_, _, _| {},
                ),
                AuthPhase::Waiting(_) => {
                    let finish = auth.finish;
                    small_button(
                        rox_i18n::t!("settings-integrations-scrobble-finish-connecting"),
                        icons::REFRESH_CW,
                        false,
                        cx.listener(move |this, _, _, cx| finish(this, cx)),
                    )
                }
                // Reconnect where a session was lost rather than never
                // held: the button reads as picking something back up.
                phase => {
                    let begin = auth.begin;
                    small_button(
                        if matches!(phase, AuthPhase::Rejected) || auth.elsewhere {
                            rox_i18n::t!("settings-integrations-scrobble-reconnect")
                        } else {
                            rox_i18n::t!("settings-integrations-scrobble-connect")
                        },
                        icons::EXTERNAL_LINK,
                        !auth.ready,
                        cx.listener(move |this, _, _, cx| begin(this, cx)),
                    )
                }
            }
        };
        (status, vec![action.into_any_element()])
    }

    /// Last.fm: the browser flow, the api pair on a build that ships none,
    /// and the hearts mirror no other destination has.
    fn lastfm_section(&self, q: &Query, cx: &mut Context<Self>) -> Section {
        let name: SharedString = rox_i18n::t!("settings-integrations-section-lastfm");
        let (config, phase, connected, username, elsewhere, loves_pending, love_error) = {
            let s = self.scrobbler.read(cx);
            (
                s.config().clone(),
                s.phase().clone(),
                s.connected(),
                s.username().to_string(),
                s.connected_elsewhere(),
                s.loves_pending(),
                s.love_error(),
            )
        };
        // A build with its own api identity connects in one click; only
        // one without needs the user's pair.
        let builtin = has_builtin_keys();
        let (status, actions) = self.browser_auth_strip(
            &name,
            BrowserAuth {
                phase,
                connected,
                username,
                elsewhere,
                ready: builtin || (!config.api_key.is_empty() && !config.api_secret.is_empty()),
                begin: |this, cx| this.scrobbler.update(cx, |s, cx| s.begin_auth(cx)),
                finish: |this, cx| this.scrobbler.update(cx, |s, cx| s.finish_auth(cx)),
                disconnect: |this, cx| this.scrobbler.update(cx, |s, cx| s.disconnect(cx)),
            },
            cx,
        );
        let fields = if builtin {
            Vec::new()
        } else {
            vec![
                panel::setting_row(
                    rox_i18n::t!("settings-integrations-lastfm-api-key-row"),
                    None,
                    Input::new(&self.lastfm_key).w(px(240.)),
                )
                .into_any_element(),
                panel::setting_row(
                    rox_i18n::t!("settings-integrations-lastfm-secret-row"),
                    None,
                    Input::new(&self.lastfm_secret).w(px(240.)),
                )
                .into_any_element(),
            ]
        };

        // What the love sync has left to do, and why it stopped if it did.
        // A love that failed into a log file is two sides out of sync with
        // nothing on screen to say so, so this line is why the queue keeps
        // its reason.
        let hearts = |n: usize| {
            rox_i18n::t!("settings-integrations-lastfm-hearts", n = n as u64).to_string()
        };
        let love_status: Option<SharedString> = match (loves_pending, love_error) {
            (0, None) => None,
            (0, Some(error)) => Some(rox_i18n::t!(
                "settings-integrations-lastfm-love-failed",
                error = error.to_string()
            )),
            (pending, None) => Some(rox_i18n::t!(
                "settings-integrations-lastfm-love-pending",
                hearts = hearts(pending)
            )),
            (pending, Some(error)) => Some(rox_i18n::t!(
                "settings-integrations-lastfm-love-pending-failed",
                hearts = hearts(pending),
                error = error.to_string()
            )),
        };
        let love_toggle = panel::toggle(
            config.love_favourites,
            |this: &mut Self, on, cx| {
                this.scrobbler
                    .update(cx, |s, cx| s.set_love_favourites(on, cx));
                cx.notify();
            },
            cx,
        );
        let trailing = Some(self.import_control(cx));
        let plays_control = self.plays_import_control(cx);

        self.destination_section(
            q,
            Destination {
                name,
                icon: icons::RADIO,
                keywords: &[
                    "lastfm",
                    "api key",
                    "love",
                    "loved",
                    "heart",
                    "plays",
                    "playcount",
                ],
                intro: if builtin {
                    rox_i18n::t!("settings-integrations-lastfm-intro-builtin")
                } else {
                    rox_i18n::t!("settings-integrations-lastfm-intro-custom")
                },
                fields,
                status,
                actions,
                trailing,
            },
            |rows| {
                rows.keyed(
                    "settings-integrations-love-favourites",
                    &["Last.fm", "love", "loved", "heart", "mirror"],
                    love_toggle,
                )
                .when_some(love_status, |rows, status| {
                    rows.custom(&["love", "queue", "failed"], || {
                        div()
                            .text_xs()
                            .text_color(palette::text_muted())
                            .child(status)
                            .into_any_element()
                    })
                })
                .keyed(
                    "settings-integrations-lastfm-import-plays",
                    &[
                        "Last.fm",
                        "plays",
                        "playcount",
                        "scrobbles",
                        "import",
                        "history",
                    ],
                    plays_control,
                )
            },
        )
    }

    /// ListenBrainz: a token pasted from the site is the whole connection,
    /// so Connect checks the token rather than opening a browser, and the
    /// status line names the account it belongs to.
    fn listenbrainz_section(&self, q: &Query, cx: &mut Context<Self>) -> Section {
        let name: SharedString = rox_i18n::t!("settings-integrations-section-listenbrainz");
        let lb_status = self.listenbrainz.read(cx).status().clone();
        let connected = matches!(lb_status, ListenBrainzStatus::Connected(_));
        let status: SharedString = match lb_status {
            ListenBrainzStatus::Off => {
                rox_i18n::t!("settings-integrations-scrobble-status-not-connected")
            }
            ListenBrainzStatus::Unverified => {
                rox_i18n::t!("settings-integrations-scrobble-status-checking")
            }
            ListenBrainzStatus::Connected(user) => rox_i18n::t!(
                "settings-integrations-scrobble-status-connected",
                username = user
            ),
            ListenBrainzStatus::Invalid => rox_i18n::t!(
                "settings-integrations-scrobble-status-invalid",
                service = name.to_string()
            ),
            ListenBrainzStatus::Failed(reason) => rox_i18n::t!(
                "settings-integrations-scrobble-status-failed",
                error = reason
            ),
        };
        // Connect with an empty field would only ever say "not connected"
        // back, so it's inert until there's something to check.
        let token_empty = self.listenbrainz_token.read(cx).value().trim().is_empty();
        let mut actions = Vec::with_capacity(2);
        if connected {
            actions.push(
                small_button(
                    rox_i18n::t!("settings-integrations-scrobble-disconnect"),
                    icons::CLOSE,
                    false,
                    cx.listener(|this, _, window, cx| {
                        this.listenbrainz.update(cx, |lb, cx| lb.disconnect(cx));
                        this.listenbrainz_token
                            .update(cx, |input, cx| input.set_value("", window, cx));
                    }),
                )
                .into_any_element(),
            );
        }
        actions.push(
            small_button(
                rox_i18n::t!("settings-integrations-scrobble-connect"),
                icons::LINK,
                token_empty,
                cx.listener(|this, _, _, cx| {
                    let token = this.listenbrainz_token.read(cx).value().trim().to_string();
                    this.listenbrainz
                        .update(cx, |lb, cx| lb.set_token(token, cx));
                }),
            )
            .into_any_element(),
        );
        let token_field = panel::setting_row(
            rox_i18n::t!("settings-integrations-listenbrainz-token-row"),
            None,
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_MD)
                // The token only exists on the ListenBrainz settings page,
                // and fetching it comes before pasting it, so the link
                // sits ahead of the field in reading order.
                .child(
                    div()
                        .id("listenbrainz-get-token")
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(tokens::SPACE_XS)
                        .text_xs()
                        .text_color(palette::text_muted())
                        .hover(|d| d.text_color(palette::text_bright()))
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.open_url("https://listenbrainz.org/settings/")
                        })
                        .child(svg().path(icons::EXTERNAL_LINK).size(px(12.)))
                        .child(rox_i18n::t!("settings-integrations-listenbrainz-get-token")),
                )
                .child(Input::new(&self.listenbrainz_token).w(px(240.))),
        )
        .into_any_element();

        self.destination_section(
            q,
            Destination {
                name,
                icon: icons::AUDIO_WAVEFORM,
                keywords: &["listenbrainz", "musicbrainz", "token", "listens"],
                intro: rox_i18n::t!("settings-integrations-listenbrainz-intro"),
                fields: vec![token_field],
                status,
                actions,
                trailing: None,
            },
            |rows| rows,
        )
    }

    /// Libre.fm: Last.fm's browser dance against its own entity, with no
    /// api pair to ask for since the service takes the one rox ships.
    fn librefm_section(&self, q: &Query, cx: &mut Context<Self>) -> Section {
        let name: SharedString = rox_i18n::t!("settings-integrations-section-librefm");
        let (phase, connected, username) = {
            let lf = self.librefm.read(cx);
            (
                lf.phase().clone(),
                lf.connected(),
                lf.username().to_string(),
            )
        };
        let (status, actions) = self.browser_auth_strip(
            &name,
            BrowserAuth {
                phase,
                connected,
                username,
                elsewhere: false,
                ready: true,
                begin: |this, cx| this.librefm.update(cx, |lf, cx| lf.begin_auth(cx)),
                finish: |this, cx| this.librefm.update(cx, |lf, cx| lf.finish_auth(cx)),
                disconnect: |this, cx| this.librefm.update(cx, |lf, cx| lf.disconnect(cx)),
            },
            cx,
        );

        self.destination_section(
            q,
            Destination {
                name,
                icon: icons::DISC,
                keywords: &["librefm", "libre.fm", "gnu fm"],
                intro: rox_i18n::t!("settings-integrations-librefm-intro"),
                fields: Vec::new(),
                status,
                actions,
                trailing: None,
            },
            |rows| rows,
        )
    }

    /// The Icecast section (ADR 22): the source client, which is the audio
    /// half of the refused web server. The switch connects and
    /// disconnects; the fields write through as they're typed and the sink
    /// re-applies when one is left.
    ///
    /// Everything under the switch only appears once it's on. A mount, a
    /// source login and a bitrate are four rows of setup for something most
    /// people never turn on, and with the switch off they'd only be a
    /// question nobody asked.
    fn icecast_section(&self, q: &Query, cx: &mut Context<Self>) -> Section {
        Section::new(
            q,
            icons::RADIO,
            rox_i18n::t!("settings-audio-section-broadcast"),
            None,
            |rows| {
                rows.keyed(
                    "settings-audio-broadcast-enable",
                    &["icecast", "stream", "radio", "cast", "mount", "broadcast"],
                    panel::toggle(self.broadcast_enabled, Self::set_broadcast_enabled, cx),
                )
                .when(self.broadcast_enabled, |rows| {
                    rows.keyed(
                        "settings-audio-broadcast-server",
                        &["icecast", "server", "host", "port"],
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            .child(Input::new(&self.broadcast_host).w(px(180.)))
                            .child(Input::new(&self.broadcast_port).w(px(64.))),
                    )
                    .keyed(
                        "settings-audio-broadcast-mount",
                        &["icecast", "mount", "name", "advertise"],
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            .child(Input::new(&self.broadcast_mount).w(px(104.)))
                            .child(Input::new(&self.broadcast_name).w(px(140.))),
                    )
                    .keyed(
                        "settings-audio-broadcast-login",
                        &["icecast", "source", "login", "password", "credentials"],
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            .child(Input::new(&self.broadcast_user).w(px(104.)))
                            .child(Input::new(&self.broadcast_password).w(px(140.))),
                    )
                    .custom(&["bitrate", "kbps", "quality", "encoder", "mp3"], || {
                        self.broadcast_bitrate_row(cx).into_any_element()
                    })
                })
            },
        )
    }

    /// The Icecast half of the same idea: re-dial the sink on whatever the
    /// fields now say. Gated on the flag so a blur that passed through an
    /// untouched field doesn't drop a live connection and build it again.
    pub(super) fn broadcast_moved(&mut self) {
        if !self.broadcast_dirty {
            return;
        }

        self.broadcast_dirty = false;

        if self.broadcast_enabled {
            crate::integrations::broadcast::apply();
        }
    }

    /// The broadcast switch. Applying reads the file the field edits were
    /// written to, so the sink comes up on whatever the rows say; off tears
    /// the connection down, which releases the mount.
    fn set_broadcast_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        self.broadcast_enabled = on;
        Settings::update(move |s| s.broadcast.enabled = on);
        crate::integrations::broadcast::apply();
        cx.notify();
    }

    /// The encoder bitrate, the steps LAME takes. A change while streaming
    /// reconnects, since one stream can't change bitrate under a listener.
    fn broadcast_bitrate_row(&self, cx: &mut Context<Self>) -> Div {
        let options: Vec<(u32, SharedString)> = [96u32, 112, 128, 160, 192, 224, 256, 320]
            .into_iter()
            .map(|kbps| {
                (
                    kbps,
                    rox_i18n::format::format_unit(f64::from(kbps), 0, "kbps").into(),
                )
            })
            .collect();
        panel::setting_row(
            rox_i18n::t!("settings-audio-broadcast-bitrate"),
            Some(rox_i18n::t!("settings-audio-broadcast-bitrate.description")),
            panel::picker(
                "broadcast-bitrate",
                self.broadcast_bitrate,
                options,
                false,
                |this: &mut Self, kbps, cx| {
                    this.broadcast_bitrate = kbps;
                    Settings::update(move |s| s.broadcast.bitrate = kbps);
                    if this.broadcast_enabled {
                        crate::integrations::broadcast::apply();
                    }
                    cx.notify();
                },
                cx,
            ),
        )
    }

    /// Run the version probe against whatever the input holds, off the UI
    /// thread since it spawns a process, and keep the result for the
    /// callout. The probe cache records it too, so a pass flips the
    /// Convert surfaces on right here.
    fn test_ffmpeg(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let answer = cx
                .background_executor()
                .spawn(async { convert::test() })
                .await;
            this.update(cx, |this, cx| {
                this.ffmpeg_test = Some(answer);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// What the line under a Discord card line's input says: the line the
/// card will show, a note where the pattern renders nothing, or what's
/// wrong with the pattern.
fn presence_line_note(preview: Result<String, String>) -> PatternNote {
    match preview {
        // A line that renders to nothing isn't an error, but it does need
        // saying: the card goes out without it.
        Ok(line) if line.is_empty() => {
            PatternNote::Quiet(rox_i18n::t!("settings-integrations-discord-line-off"))
        }

        Ok(line) => PatternNote::Preview(rox_i18n::t!(
            "settings-integrations-discord-line-preview",
            line = line
        )),

        Err(e) => PatternNote::Wrong(e.into()),
    }
}

/// One Discord card line's block: the label, and the pattern box every
/// other pattern in the app is typed into.
fn presence_line_block(
    id: &'static str,
    title: SharedString,
    description: SharedString,
    input: &Entity<InputState>,
    note: PatternNote,
) -> AnyElement {
    panel::setting_block(
        title,
        Some(description),
        None,
        panel::pattern_input(
            id,
            input,
            rox_core::pattern::PLACEHOLDERS,
            Vec::new(),
            Some(note),
        )
        // Fill the block rather than shrinking to the input's own width,
        // the same as the capture pattern's column.
        .flex_1()
        .min_w_0(),
    )
    .into_any_element()
}
