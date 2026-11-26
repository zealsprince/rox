//! The enable card (ADR 30, ADR 37): what a package is and what it's given,
//! read out before it runs for the first time or again after it changed.
//! Switching a package on is the approving act, and this card is the one
//! dialog in front of it. The Plugins and Renderers settings pages raise it
//! over the settings window, and a canvas or a Renderer panel raises the same
//! card over its own window when a renderer that isn't ready is picked there.
//!
//! Only [`EnableCard::accept`] approves, and only a Switch On press on a card
//! calls it. Building a card approves nothing.

use gpui::{App, Div, SharedString, div, prelude::*};
use rox_core::settings::{PluginRecord, Settings};
use rox_design::{palette, tokens};
use rox_plugins::Loaded;
use rox_plugins::manifest::{Manifest, ProviderDomain};
use rox_services::plugins::{self as host, Change, Grant, Since};

/// What the confirm reads out. Built once, when the switch is pressed.
pub(crate) struct EnableCard {
    pub(crate) name: String,
    /// The folder as scanned when the card opened: its hash is what gets
    /// approved, not whatever is on disk by the time Switch On is pressed.
    folder: Loaded,
    /// Who made it and what it says it is, above the dialog's body.
    pub(crate) lead: Vec<SharedString>,
    /// What it declares, the programs it uses, and what changed, below it.
    pub(crate) lines: Vec<SharedString>,
}

impl EnableCard {
    pub(crate) fn of(folder: Loaded, record: Option<&PluginRecord>) -> Option<EnableCard> {
        let manifest = folder.manifest.clone()?;
        let mut lead: Vec<SharedString> = Vec::new();
        let mut lines: Vec<SharedString> = Vec::new();

        let mut byline = Vec::new();
        if !manifest.meta.author.trim().is_empty() {
            byline.push(
                rox_i18n::t!(
                    "workspace-byline-author",
                    author = manifest.meta.author.trim()
                )
                .to_string(),
            );
        }
        byline.push(
            rox_i18n::t!(
                "workspace-byline-version",
                version = manifest.version.as_str()
            )
            .to_string(),
        );
        lead.push(byline.join(", ").into());

        if !manifest.meta.description.trim().is_empty() {
            lead.push(manifest.meta.description.trim().to_string().into());
        }

        if let Some(source) = &manifest.capabilities.source {
            lines.push(rox_i18n::t!(
                "settings-plugins-card-source",
                label = source.label.as_str()
            ));
            if source.scrobble {
                lines.push(rox_i18n::t!("settings-plugins-card-scrobbles"));
            }
            if source.lyrics {
                lines.push(rox_i18n::t!("settings-plugins-card-lyrics"));
            }
            if source.favourites.is_some() {
                lines.push(rox_i18n::t!("settings-plugins-card-favourites"));
            }

            if !source.actions.is_empty() {
                let labels: Vec<&str> = source.actions.iter().map(|a| a.label.as_str()).collect();
                lines.push(rox_i18n::t!(
                    "settings-plugins-card-actions",
                    actions = labels.join(", ")
                ));
            }
        }

        // What a renderer is sent, from the grants the host hands it, so the
        // card can't promise less than it gets.
        if let Some(renderer) = &manifest.capabilities.renderer {
            lines.push(rox_i18n::t!(
                "settings-plugins-card-renderer",
                label = renderer.label.as_str()
            ));
        }
        lines.extend(
            host::grants(&manifest)
                .into_iter()
                .map(|grant| grant_line(grant, &manifest)),
        );

        let providers = manifest.capabilities.providers;
        if providers.any() {
            let domains: Vec<SharedString> = ProviderDomain::ALL
                .into_iter()
                .filter(|domain| providers.has(*domain))
                .map(domain_name)
                .collect();
            lines.push(rox_i18n::t!(
                "settings-plugins-card-providers",
                domains = domains.join(", ")
            ));
        }

        for (program, found) in &folder.programs {
            lines.push(match found {
                true => rox_i18n::t!(
                    "settings-plugins-card-program-found",
                    program = program.as_str()
                ),
                false => rox_i18n::t!(
                    "settings-plugins-card-program-missing",
                    program = program.as_str()
                ),
            });
        }

        // Only a re-approval has something to diff against.
        if let Some(record) = record.filter(|record| !record.approved_manifest.is_null()) {
            let files = match manifest.is_renderer() {
                true => "settings-plugins-card-unchanged-renderer",
                false => "settings-plugins-card-unchanged",
            };

            let since = host::since(
                &record.approved_manifest,
                &record.hash,
                &folder.document,
                &folder.hash,
            );
            match since {
                Since::Declared(changes) => {
                    lines.push(rox_i18n::t!("settings-plugins-card-changed"));
                    lines.extend(changes.iter().map(|change| change_line(change, &manifest)));
                }
                Since::Manifest => {
                    lines.push(rox_i18n::t!("settings-plugins-card-manifest-changed"))
                }
                Since::Files => lines.push(rox_i18n::t!(files)),
                Since::Nothing => {}
            }
        }

        Some(EnableCard {
            name: manifest.name.clone(),
            folder,
            lead,
            lines,
        })
    }

    /// The card for renderer `id` as the plugins folder holds it now, with
    /// its record's last approval to diff against. None when no loadable
    /// renderer has the id.
    pub(crate) fn for_renderer(id: &str) -> Option<EnableCard> {
        let folder = host::loaded()
            .into_iter()
            .find(|folder| folder.id == id && folder.runs() && host::is_renderer(folder))?;
        let records = Settings::load().accounts.plugins;
        let record = records.iter().find(|record| record.id == id);

        EnableCard::of(folder, record)
    }

    pub(crate) fn folder(&self) -> &Loaded {
        &self.folder
    }

    pub(crate) fn title(&self) -> SharedString {
        rox_i18n::t!("settings-plugins-card-title", name = self.name.as_str())
    }

    /// The card's words, in the order every host reads them out: the title,
    /// who made it, that it runs as a program, then what it's given.
    pub(crate) fn body(&self) -> Div {
        let line = |text: SharedString| {
            div()
                .text_xs()
                .text_color(palette::text_muted())
                .child(text)
        };

        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(div().child(self.title()))
            .children(self.lead.iter().cloned().map(line))
            .child(line(rox_i18n::t!("settings-plugins-card-runs")))
            .children(self.lines.iter().cloned().map(line))
    }

    /// Switch On's press: approves exactly the files the card read out and
    /// switches the package on, as the settings switch does. Never called
    /// but from that press.
    pub(crate) fn accept(self, cx: &mut App) {
        host::approve(&self.folder, cx);
    }
}

fn grant_line(grant: Grant, manifest: &Manifest) -> SharedString {
    match grant {
        Grant::MusicSignals => rox_i18n::t!("settings-plugins-card-renderer-signals"),
        Grant::Pointer => rox_i18n::t!("settings-plugins-card-renderer-pointer"),
        Grant::WantedSignals => rox_i18n::t!(
            "settings-plugins-card-renderer-wanted",
            signals = wanted_list(manifest)
        ),
        Grant::Palette => rox_i18n::t!("settings-plugins-card-renderer-palette"),
        Grant::Cover => rox_i18n::t!("settings-plugins-card-renderer-cover"),
        Grant::Spectrum => rox_i18n::t!("settings-plugins-card-renderer-spectrum"),
    }
}

/// The signals a renderer asks for, worded as the pool would list them
/// once added. The names are the renderer's, capped and plain by the
/// manifest check.
fn wanted_list(manifest: &Manifest) -> String {
    let wanted = manifest
        .capabilities
        .renderer
        .as_ref()
        .map(|cap| cap.signals.as_slice())
        .unwrap_or_default();

    let adding = rox_services::renderers::signals_to_add(wanted, &[]);
    adding
        .iter()
        .map(|signal| crate::canvas::target::signal_words(signal, &adding))
        .collect::<Vec<_>>()
        .join(", ")
}

fn change_line(change: &Change, manifest: &Manifest) -> SharedString {
    match change {
        Change::CapabilityAdded(name) => rox_i18n::t!(
            "settings-plugins-change-capability-added",
            name = name.as_str()
        ),
        Change::CapabilityRemoved(name) => rox_i18n::t!(
            "settings-plugins-change-capability-removed",
            name = name.as_str()
        ),
        Change::ProgramAdded(program) => rox_i18n::t!(
            "settings-plugins-change-program",
            program = program.as_str()
        ),
        Change::Scrobble(true) => rox_i18n::t!("settings-plugins-change-scrobble-on"),
        Change::Scrobble(false) => rox_i18n::t!("settings-plugins-change-scrobble-off"),
        Change::Entry => rox_i18n::t!("settings-plugins-change-entry"),
        Change::ActionAdded(label) => rox_i18n::t!(
            "settings-plugins-change-action-added",
            label = label.as_str()
        ),
        Change::ActionChanged(label) => rox_i18n::t!(
            "settings-plugins-change-action-changed",
            label = label.as_str()
        ),
        Change::ProviderAdded(domain) => rox_i18n::t!(
            "settings-plugins-change-provider",
            domain = domain_name(*domain).to_string()
        ),
        // The card lists every grant above. Under the changes it says the
        // new one again, worded the same.
        Change::Grant(grant) => grant_line(*grant, manifest),
    }
}

/// A provider domain as the card and the change lines name it, mid-sentence.
fn domain_name(domain: ProviderDomain) -> SharedString {
    match domain {
        ProviderDomain::Lyrics => rox_i18n::t!("settings-plugins-domain-lyrics"),
        ProviderDomain::Metadata => rox_i18n::t!("settings-plugins-domain-metadata"),
        ProviderDomain::Artist => rox_i18n::t!("settings-plugins-domain-artist"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn folder(programs: &[(&str, bool)]) -> Loaded {
        let manifest = rox_plugins::manifest::parse(
            r#"{ "id": "tones", "name": "Tones", "version": "1", "api": 1,
                 "entry": { "native": { "linux-x86_64": "a" } } }"#,
        )
        .unwrap();

        Loaded {
            id: "tones".into(),
            dir: "/plugins/tones".into(),
            origin: rox_plugins::Origin::Folder,
            manifest: Some(manifest),
            document: Value::Null,
            hash: "beef".into(),
            programs: programs
                .iter()
                .map(|(program, found)| (program.to_string(), *found))
                .collect(),
            error: None,
            icon: None,
            action_icons: Vec::new(),
        }
    }

    fn record(enabled: bool, hash: &str) -> PluginRecord {
        PluginRecord {
            id: "tones".into(),
            enabled,
            hash: hash.into(),
            ..Default::default()
        }
    }

    #[test]
    fn the_card_names_every_declared_domain() {
        rox_i18n::pin_thread_locale(rox_i18n::SOURCE_LOCALE);
        let mut provider = folder(&[]);
        provider.manifest = Some(
            rox_plugins::manifest::parse(
                r#"{ "id": "tones", "name": "Tones", "version": "1", "api": 1,
                     "entry": { "native": { "linux-x86_64": "a" } },
                     "capabilities": { "providers": { "lyrics": true, "artist": true } } }"#,
            )
            .unwrap(),
        );

        let card = EnableCard::of(provider, None).unwrap();

        assert_eq!(card.lines.len(), 1, "{:?}", card.lines);
        assert!(
            card.lines[0].contains("lyrics, artist info"),
            "{}",
            card.lines[0]
        );
    }

    #[test]
    fn a_renderers_card_says_what_it_gets_and_never_plugin() {
        rox_i18n::pin_thread_locale(rox_i18n::SOURCE_LOCALE);
        let mut renderer = folder(&[("python3", true)]);
        renderer.manifest = Some(
            rox_plugins::manifest::parse(
                r#"{ "id": "bars", "name": "Bars", "version": "2", "api": 1,
                     "entry": { "script": { "path": "draw.py", "interpreter": "python3" } },
                     "capabilities": { "renderer": { "label": "Bars" } } }"#,
            )
            .unwrap(),
        );

        let card = EnableCard::of(renderer.clone(), None).unwrap();
        let grants = host::grants(renderer.manifest.as_ref().unwrap());
        assert_eq!(
            card.lines.len(),
            1 + grants.len() + 1,
            "what it draws, each grant, the program: {:?}",
            card.lines
        );
        assert!(card.lines[0].contains("Bars"), "{}", card.lines[0]);
        let manifest = renderer.manifest.clone().unwrap();
        for (line, grant) in card.lines[1..].iter().zip(&grants) {
            assert_eq!(*line, grant_line(*grant, &manifest));
        }

        // Re-approving a renderer whose manifest didn't move.
        let mut changed = renderer.clone();
        changed.document = json!({ "id": "bars" });
        let approved = PluginRecord {
            approved_manifest: json!({ "id": "bars" }),
            ..record(true, "old")
        };
        let again = EnableCard::of(changed, Some(&approved)).unwrap();
        assert_eq!(
            again.lines.last(),
            Some(&rox_i18n::t!("settings-plugins-card-unchanged-renderer"))
        );

        // One whose description moved and nothing it declares.
        let mut described = renderer.clone();
        described.document = json!({ "id": "bars", "meta": { "description": "Louder." } });
        let redescribed = EnableCard::of(described, Some(&approved)).unwrap();
        assert_eq!(
            redescribed.lines.last(),
            Some(&rox_i18n::t!("settings-plugins-card-manifest-changed"))
        );

        // One that asks for everything a renderer can, and one that gained
        // the palette and the cover since it was approved.
        let mut asking = renderer.clone();
        let document = json!({
            "id": "bars", "name": "Bars", "version": "2", "api": 1,
            "entry": { "script": { "path": "draw.py", "interpreter": "python3" } },
            "capabilities": { "renderer": {
                "label": "Bars", "palette": true, "cover": true,
                "signals": [
                    { "kind": "band", "low": 30, "high": 120, "name": "Kick" },
                    { "kind": "level" },
                ],
            } },
        });
        asking.manifest = Some(rox_plugins::manifest::parse(&document.to_string()).unwrap());
        asking.document = document;
        let asks = EnableCard::of(asking.clone(), None).unwrap();
        let wanted = "Follows these signals: Kick (Band 30-120 Hz), Level. \
                      When you place a render target for it, rox offers to add the ones you don't have yet.";
        assert_eq!(
            asks.lines[1..6],
            [
                rox_i18n::t!("settings-plugins-card-renderer-signals"),
                rox_i18n::t!("settings-plugins-card-renderer-pointer"),
                SharedString::from(wanted),
                SharedString::from(
                    "Gets the colours of the current theme, which follow the cover when song theming is on.",
                ),
                SharedString::from("Gets the cover of the playing track, as a picture."),
            ]
        );

        let mut approved_manifest = asking.document.clone();
        approved_manifest["capabilities"]["renderer"]["palette"] = json!(false);
        approved_manifest["capabilities"]["renderer"]
            .as_object_mut()
            .unwrap()
            .remove("cover");
        let before = PluginRecord {
            approved_manifest,
            ..record(true, "old")
        };
        let regained = EnableCard::of(asking, Some(&before)).unwrap();
        let tail = &regained.lines[regained.lines.len() - 3..];
        assert_eq!(
            tail,
            [
                rox_i18n::t!("settings-plugins-card-changed"),
                SharedString::from(
                    "Gets the colours of the current theme, which follow the cover when song theming is on.",
                ),
                SharedString::from("Gets the cover of the playing track, as a picture."),
            ]
        );

        let everything: Vec<String> = card
            .lines
            .iter()
            .chain(&again.lines)
            .chain(&redescribed.lines)
            .chain(&asks.lines)
            .chain(&regained.lines)
            .chain(&card.lead)
            .map(|line| line.to_lowercase())
            .collect();
        assert!(
            everything.iter().all(|line| !line.contains("plugin")),
            "{everything:?}"
        );
    }
}
