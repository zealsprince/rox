//! The Radio settings page: the stations the library holds, added one at a
//! time or imported from a `.pls` or `.m3u`.

use super::*;

impl SettingsWindow {
    /// The Radio page: the stations the library holds. A station is a
    /// stream with no catalog behind it, so it isn't one of the Library
    /// page's sources, the folders and servers a library is browsed out of.
    /// It's configured here and played from its own panel.
    pub(super) fn radio_page(&self, q: &Query, cx: &mut Context<Self>) -> PageBody {
        PageBody::new().section(self.stations_section(q, cx))
    }

    /// The stations section: what the library holds, a row to add one by
    /// URL, and the two ways in that aren't typing. The stations panel
    /// lists and plays; everything that changes the list is here.
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

        // The list, then the add row at its foot, where the eye lands
        // after reading it. The folder table above takes the same shape.
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

    /// One station: its name over its stream, and the remove that drops
    /// it. Unnamed stations show the URL on both lines, which is all they
    /// have and still better than a blank row.
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
            // Named after the stream, so the row's remove button is its own
            // rather than every other row's. See
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

    /// The add row under the list: the stream, an optional name, and the
    /// button. The URL is the identity, so it's the only field that has to
    /// be filled in.
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

    /// Write stations, then have the library rebuild its projection so the
    /// new rows show up everywhere else too, not only in this list.
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

    /// Add whatever is in the two fields, in two passes. The URL string
    /// is judged first, because most of what gets pasted in here is
    /// wrong in a way the string already shows, and that answer is
    /// instant. What survives that goes to the stream itself, because
    /// the other common mistake is a station's web page, and a web page
    /// URL is indistinguishable from a mount until something asks.
    fn add_station(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = self.station_url.read(cx).value().trim().to_string();
        if url.is_empty() || self.station_probing {
            return;
        }

        // The string rule is the import reader's, so typing a URL in and
        // importing a file holding it agree on which lines rox will take.
        // Each refusal gets its own line: someone holding a perfectly good
        // `.pls` needs to hear "Import", not "that isn't a stream".
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

    /// The second half of an add, once the stream has answered.
    ///
    /// Only one answer stops the add, and that's the one where the URL
    /// positively said it serves a document. Everything else lands as a
    /// row, including a station that didn't answer at all: a stream URL
    /// is something somebody found on a forum years ago, an Icecast
    /// mount whose source is asleep answers 404, and refusing a station
    /// for being down today would be a worse bug than the one this
    /// check exists to catch. The line says the check came back empty so
    /// the row isn't mistaken for a verified one.
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

    /// Import a `.pls` or `.m3u` of stream URLs, which is how most people
    /// already have their stations.
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
                // A playlist of local files is a real thing to pick by
                // mistake, and it imports as nothing. Say so.
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

    /// Drop one station, then rebuild the projection the way a write does:
    /// the row has to leave the library everywhere, not just this list.
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

/// Every radio station the library holds. Its own connection, opened per
/// read rather than held, for the reason the row count above gives: this
/// runs at the pace someone edits a list, and a held connection would sit
/// through every scan in between. A database that isn't there yet reads as
/// no stations.
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
