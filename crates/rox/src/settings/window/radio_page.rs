//! The Radio settings page: the stations the library holds, added by URL or
//! imported from a `.pls` or `.m3u`. Not a Library source, since a station has
//! no catalog behind it.

use super::*;

impl SettingsWindow {
    pub(super) fn radio_page(&self, q: &Query, cx: &mut Context<Self>) -> PageBody {
        PageBody::new().section(self.stations_section(q, cx))
    }

    fn stations_section(&self, q: &Query, cx: &mut Context<Self>) -> Section {
        let controls = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .child(small_button(
                rox_i18n::t!("settings-sources-stations-find"),
                icons::SEARCH,
                false,
                cx.listener(|this, _, _, cx| {
                    rox_panel_api::openers::station_directory(this.state.clone(), cx);
                }),
            ))
            .child(small_button(
                rox_i18n::t!("settings-sources-stations-import"),
                icons::DOWNLOAD,
                false,
                cx.listener(|this, _, window, cx| this.import_stations(window, cx)),
            ))
            .into_any_element();

        let mut table = div().flex().flex_col();
        if self.stations.is_empty() {
            table = table.child(
                div()
                    .py(tokens::SPACE_XS)
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!("settings-sources-stations-none")),
            );
        }
        for station in &self.stations {
            table = table.child(self.station_row(station, cx));
        }

        let table = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .child(table)
            .child(self.station_add_row(cx))
            .when_some(self.station_notice.clone(), |d, notice| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(palette::tone_warn())
                        .child(notice),
                )
            });

        Section::new(
            q,
            icons::RADIO,
            rox_i18n::t!("settings-sources-section-stations"),
            Some(controls),
            |rows| {
                rows.custom(
                    &[
                        "radio",
                        "station",
                        "stream",
                        "url",
                        "pls",
                        "m3u",
                        "shoutcast",
                    ],
                    || table.into_any_element(),
                )
            },
        )
    }

    /// Unnamed stations show the URL on both lines.
    fn station_row(&self, station: &Station, cx: &mut Context<Self>) -> Stateful<Div> {
        let url: SharedString = station.url.clone().into();
        let name = station.name.trim();
        let title: SharedString = if name.is_empty() {
            url.clone()
        } else {
            name.to_string().into()
        };
        let remove = icon_button(icons::CLOSE, false, {
            let url = station.url.clone();
            cx.listener(move |this, _, _, cx| this.remove_station(&url, cx))
        });

        div()
            // Named after the stream so its remove button is its own. See
            // `rox_panel_kit::ui::control_focus`.
            .id(ElementId::Name(url.clone()))
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_MD)
            .py(tokens::SPACE_XS)
            .border_b_1()
            .border_color(palette::border())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().truncate().child(title))
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(palette::text_muted())
                            .child(url),
                    ),
            )
            .child(remove)
    }

    fn station_add_row(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Input::new(&self.station_url)),
            )
            .child(div().w(px(160.)).child(Input::new(&self.station_name)))
            .child(icon_button(
                icons::PLUS,
                self.station_probing,
                cx.listener(|this, _, window, cx| this.add_station(window, cx)),
            ))
    }

    fn write_stations(&mut self, stations: &[Station], cx: &mut Context<Self>) -> bool {
        let path = self.library.read(cx).db_path();
        let Ok(mut conn) = rox_library::store::open(&path) else {
            return false;
        };

        if let Err(e) = stations::put(&mut conn, stations) {
            log::warn!("stations: writing {} rows failed: {e}", stations.len());
            return false;
        }

        self.library
            .update(cx, |library, cx| library.reload_projection(cx));
        self.stations = read_stations(&self.library, cx);
        cx.notify();
        true
    }

    /// Two passes: the URL string first, which catches most mistakes instantly,
    /// then the stream itself, since a station's web page looks like a mount
    /// until something asks.
    fn add_station(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = self.station_url.read(cx).value().trim().to_string();
        if url.is_empty() || self.station_probing {
            return;
        }

        // The import reader's rule, so typing and importing agree. Each refusal
        // says what to do instead.
        if let Some(refusal) = stations::refusal(&url) {
            self.station_notice = Some(match refusal {
                Refusal::Scheme => rox_i18n::t!("stations-not-a-stream"),
                Refusal::Hls => rox_i18n::t!("stations-is-hls"),
                Refusal::Playlist => rox_i18n::t!("stations-is-a-playlist"),
            });
            cx.notify();
            return;
        }

        self.station_probing = true;
        self.station_notice = Some(rox_i18n::t!("stations-checking"));

        let name = self.station_name.read(cx).value().trim().to_string();
        let ask = cx.background_executor().spawn({
            let url = url.clone();
            async move { rox_net::sources::stream_probe::probe(&url) }
        });

        cx.spawn_in(window, async move |this, cx| {
            let answer = ask.await;

            this.update_in(cx, |this, window, cx| {
                this.station_probing = false;
                this.finish_add_station(url, name, answer, window, cx);
            })
            .ok();
        })
        .detach();

        cx.notify();
    }

    /// Only a URL that positively serves a document stops the add. A station
    /// that didn't answer still lands, since a sleeping Icecast mount answers
    /// 404; the line says the check came back empty.
    fn finish_add_station(
        &mut self,
        url: String,
        name: String,
        answer: Probe,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Probe::Document { content_type } = answer {
            self.station_notice = Some(rox_i18n::t!("stations-not-audio", kind = content_type));
            cx.notify();
            return;
        }

        let station = Station {
            url,
            name,
            genre: String::new(),
        };

        if !self.write_stations(&[station], cx) {
            return;
        }

        self.station_notice = match answer {
            Probe::Unknown { reason } => {
                Some(rox_i18n::t!("stations-added-unchecked", reason = reason))
            }

            _ => None,
        };

        self.station_url
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.station_name
            .update(cx, |input, cx| input.set_value("", window, cx));
    }

    fn import_stations(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });

        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.pop() else {
                return;
            };
            let Ok(text) = std::fs::read_to_string(&path) else {
                return;
            };

            let found = stations::import(&text);
            this.update(cx, |this, cx| {
                // A playlist of local files imports as nothing, so say so.
                if found.is_empty() {
                    this.station_notice = Some(rox_i18n::t!("stations-import-empty"));
                    cx.notify();
                    return;
                }

                if this.write_stations(&found, cx) {
                    this.station_notice = None;
                }
            })
            .ok();
        })
        .detach();
    }

    fn remove_station(&mut self, url: &str, cx: &mut Context<Self>) {
        let path = self.library.read(cx).db_path();
        let Ok(mut conn) = rox_library::store::open(&path) else {
            return;
        };

        if let Err(e) = stations::remove(&mut conn, url) {
            log::warn!("stations: removing a station failed: {e}");
            return;
        }

        self.library
            .update(cx, |library, cx| library.reload_projection(cx));
        self.stations = read_stations(&self.library, cx);
        cx.notify();
    }
}

/// Its own connection per read: a held one would sit through every scan. A
/// missing database reads as no stations.
pub(super) fn read_stations(library: &Entity<Library>, cx: &App) -> Vec<Station> {
    let db = library.read(cx).db_path();
    if !db.exists() {
        return Vec::new();
    }

    rox_library::store::open(&db)
        .ok()
        .and_then(|conn| stations::all(&conn).ok())
        .unwrap_or_default()
}
