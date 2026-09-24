//! The Providers settings page (ADR 14): a toggle per online enrichment
//! service. Nothing fetches on its own; the toggles gate the actions the panels
//! offer.

use super::*;

impl SettingsWindow {
    fn set_lrclib(&mut self, on: bool, cx: &mut Context<Self>) {
        self.providers.lrclib = on;
        providers::set_lyrics_online(on);
        let config = self.providers.clone();
        Settings::update(move |s| s.accounts.providers = config);
        cx.notify();
    }

    fn set_lyrics_save(&mut self, save: LyricsSave, cx: &mut Context<Self>) {
        self.providers.lyrics_save = save;
        let config = self.providers.clone();
        Settings::update(move |s| s.accounts.providers = config);
        cx.notify();
    }

    fn set_musicbrainz(&mut self, on: bool, cx: &mut Context<Self>) {
        self.providers.musicbrainz = on;
        providers::set_metadata_online(on);
        let config = self.providers.clone();
        Settings::update(move |s| s.accounts.providers = config);
        cx.notify();
    }

    fn set_acoustid(&mut self, on: bool, cx: &mut Context<Self>) {
        self.providers.acoustid = on;
        providers::set_acoustid_online(on);
        let config = self.providers.clone();
        Settings::update(move |s| s.accounts.providers = config);
        cx.notify();
    }

    fn set_itunes(&mut self, on: bool, cx: &mut Context<Self>) {
        self.providers.itunes = on;
        providers::set_itunes_online(on);
        let config = self.providers.clone();
        Settings::update(move |s| s.accounts.providers = config);
        cx.notify();
    }

    fn set_deezer(&mut self, on: bool, cx: &mut Context<Self>) {
        self.providers.deezer = on;
        providers::set_deezer_online(on);
        let config = self.providers.clone();
        Settings::update(move |s| s.accounts.providers = config);
        cx.notify();
    }

    fn set_lastfm_art(&mut self, on: bool, cx: &mut Context<Self>) {
        self.providers.lastfm_art = on;
        providers::set_lastfm_art_online(on);
        let config = self.providers.clone();
        Settings::update(move |s| s.accounts.providers = config);
        cx.notify();
    }

    fn set_artist(&mut self, on: bool, cx: &mut Context<Self>) {
        self.providers.artist = on;
        providers::set_artist_online(on);
        let config = self.providers.clone();
        Settings::update(move |s| s.accounts.providers = config);
        cx.notify();
    }

    pub(super) fn providers_page(&self, q: &Query, cx: &mut Context<Self>) -> PageBody {
        PageBody::new()
            .section(Section::new(
                q,
                icons::MIC,
                rox_i18n::t!("settings-providers-section-lyrics"),
                None,
                |rows| {
                    rows.custom(&["online", "network", "offline", "privacy"], || {
                        div()
                            .text_xs()
                            .text_color(palette::text_muted())
                            .child(rox_i18n::t!("settings-providers-lyrics-intro"))
                            .into_any_element()
                    })
                    .keyed(
                        "settings-providers-lrclib",
                        &["online", "fetch"],
                        panel::toggle(self.providers.lrclib, Self::set_lrclib, cx),
                    )
                    .keyed(
                        "settings-providers-save-lyrics",
                        &["sidecar", "store"],
                        panel::choices_shared(
                            &[
                                (
                                    rox_i18n::t!("settings-providers-save-lyrics-data-folder"),
                                    LyricsSave::Store,
                                ),
                                (
                                    rox_i18n::t!("settings-providers-save-lyrics-sidecar"),
                                    LyricsSave::Sidecar,
                                ),
                                (
                                    rox_i18n::t!("settings-providers-save-lyrics-tag"),
                                    LyricsSave::Tag,
                                ),
                            ],
                            self.providers.lyrics_save,
                            Self::set_lyrics_save,
                            cx,
                        ),
                    )
                },
            ))
            .section(Section::new(
                q,
                icons::TAG,
                rox_i18n::t!("settings-providers-section-metadata"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-providers-musicbrainz",
                        &["lookup", "online"],
                        panel::toggle(self.providers.musicbrainz, Self::set_musicbrainz, cx),
                    )
                    .keyed(
                        "settings-providers-acoustid",
                        &["lookup", "online", "fingerprint", "identify"],
                        panel::toggle(self.providers.acoustid, Self::set_acoustid, cx),
                    )
                    .row_dyn(
                        &["lookup", "online", "fingerprint", "identify", "key"],
                        rox_i18n::t!("settings-providers-acoustid-key"),
                        // The hint only shows on a build that ships its own key,
                        // where the row is optional.
                        (!providers::acoustid::CLIENT_KEY.is_empty())
                            .then(|| rox_i18n::t!("settings-providers-acoustid-key-hint")),
                        Input::new(&self.acoustid_key).w(px(240.)),
                    )
                },
            ))
            .section(Section::new(
                q,
                icons::DISC,
                rox_i18n::t!("settings-providers-section-cover-art"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-providers-itunes",
                        &["artwork", "covers", "album art"],
                        panel::toggle(self.providers.itunes, Self::set_itunes, cx),
                    )
                    .keyed(
                        "settings-providers-deezer",
                        &["artwork", "covers", "album art"],
                        panel::toggle(self.providers.deezer, Self::set_deezer, cx),
                    )
                    .keyed(
                        "settings-providers-lastfm-art",
                        &["artwork", "covers", "album art"],
                        panel::toggle(self.providers.lastfm_art, Self::set_lastfm_art, cx),
                    )
                },
            ))
            .section(Section::new(
                q,
                icons::USER,
                rox_i18n::t!("settings-providers-section-artist"),
                None,
                |rows| {
                    rows.row(
                        rox_i18n::t!("settings-providers-artist"),
                        Some(rox_i18n::t!("settings-providers-artist.description")),
                        panel::toggle(self.providers.artist, Self::set_artist, cx),
                    )
                },
            ))
    }
}
