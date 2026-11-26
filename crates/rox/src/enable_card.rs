//! The enable card (ADR 30, ADR 37): what a package is and what it's given,
//! read out before it runs for the first time or again after it changed.
//! Switching a package on is the approving act, and this card is the one
//! dialog in front of it. The Plugins and Renderers settings pages raise it
//! over the settings window, and a canvas or a Renderer panel raises the same
//! card over its own window when a renderer that isn't ready is picked there.
//!
//! Only [`EnableCard::accept`] approves, and only a Switch On press on a card
//! calls it. Building a card approves nothing.
//!
//! A renderer's card says what it runs as (ADR 38): in the sandbox, with
//! what it asks for past it; outside it, because it asks to; or with the
//! user's permissions, because this system can't sandbox it. The approval
//! records exactly that tier.

use gpui::{App, Div, SharedString, div, prelude::*};
use rox_core::settings::{PluginRecord, Settings};
use rox_design::{palette, tokens};
use rox_plugins::Loaded;
use rox_plugins::manifest::{Manifest, ProviderDomain};
use rox_plugins::sandbox::{Folder, Tier, Unavailable};
use rox_services::plugins::{self as host, Change, Grant, Since};

/// What the confirm reads out. The package and what changed are read once,
/// when the switch is pressed, and the words are made at each render so the
/// card follows a language switch while it's up.
pub(crate) struct EnableCard {
    pub(crate) name: String,
    /// The folder as scanned when the card opened: its hash is what gets
    /// approved, not whatever is on disk by the time Switch On is pressed.
    folder: Loaded,
    manifest: Manifest,
    /// What moved since the last approval. None for a first one.
    since: Option<Since>,
    /// What a renderer runs as here, read when the card opened: what
    /// Switch On approves. None for a plugin.
    tier: Option<Tier>,
    /// The tier its last approval recorded, when it recorded one.
    approved_tier: Option<String>,
}

impl EnableCard {
    pub(crate) fn of(folder: Loaded, record: Option<&PluginRecord>) -> Option<EnableCard> {
        let manifest = folder.manifest.clone()?;

        // Only a re-approval has something to diff against.
        let since = record
            .filter(|record| !record.approved_manifest.is_null())
            .map(|record| {
                host::since(
                    &record.approved_manifest,
                    &record.hash,
                    &folder.document,
                    &folder.hash,
                )
            });

        let tier = manifest
            .capabilities
            .renderer
            .as_ref()
            .map(|cap| rox_services::renderers::tier(&folder, cap));
        let approved_tier = record
            .map(|record| record.sandbox.clone())
            .filter(|sandbox| !sandbox.is_empty());

        Some(EnableCard {
            name: manifest.name.clone(),
            folder,
            manifest,
            since,
            tier,
            approved_tier,
        })
    }

    /// How it runs: as a plugin does, or a renderer's tier and what this
    /// system can't stop.
    pub(crate) fn runs(&self) -> Vec<SharedString> {
        match &self.tier {
            None => vec![rox_i18n::t!("settings-plugins-card-runs")],
            Some(tier) => tier_lines(tier),
        }
    }

    /// Whether a grant is worth a line: the asks past the sandbox mean
    /// nothing to a renderer that runs outside it.
    fn says(&self, grant: Grant) -> bool {
        let access = matches!(
            grant,
            Grant::Network | Grant::ReadLibrary | Grant::ReadFolders
        );
        !access || self.tier.as_ref().is_some_and(Tier::sandboxed)
    }

    /// Who made it and what it says it is, above the dialog's body.
    pub(crate) fn lead(&self) -> Vec<SharedString> {
        let manifest = &self.manifest;
        let mut lead: Vec<SharedString> = Vec::new();

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

        lead
    }

    /// What it declares, the programs it uses, and what changed, below the
    /// body.
    pub(crate) fn lines(&self) -> Vec<SharedString> {
        let manifest = &self.manifest;
        let mut lines: Vec<SharedString> = Vec::new();

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
            host::grants(manifest)
                .into_iter()
                .filter(|grant| self.says(*grant))
                .map(|grant| grant_line(grant, manifest)),
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

        for (program, found) in &self.folder.programs {
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

        let files = match manifest.is_renderer() {
            true => "settings-plugins-card-unchanged-renderer",
            false => "settings-plugins-card-unchanged",
        };
        match &self.since {
            Some(Since::Declared(changes)) => {
                lines.push(rox_i18n::t!("settings-plugins-card-changed"));
                lines.extend(
                    changes
                        .iter()
                        .filter(|change| match change {
                            Change::Grant(grant) => self.says(*grant),
                            _ => true,
                        })
                        .map(|change| change_line(change, manifest)),
                );
            }
            Some(Since::Manifest) => {
                lines.push(rox_i18n::t!("settings-plugins-card-manifest-changed"))
            }
            Some(Since::Files) => lines.push(rox_i18n::t!(files)),
            Some(Since::Nothing) | None => {}
        }

        // The same files, approved for another tier than this system gives
        // them now, or for an interpreter at another path.
        let moved = match (&self.approved_tier, &self.tier) {
            (Some(was), Some(now)) if *was != now.key() => Some(
                match without_interpreter(was) == without_interpreter(&now.key()) {
                    true => "settings-plugins-card-interpreter-moved",
                    false => "settings-plugins-card-sandbox-changed",
                },
            ),
            _ => None,
        };
        if let Some(moved) = moved
            && !matches!(self.since, Some(Since::Declared(_)))
        {
            lines.push(rox_i18n::t!(moved));
        }

        lines
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
            .children(self.lead().into_iter().map(line))
            .children(self.runs().into_iter().map(line))
            .children(self.lines().into_iter().map(line))
    }

    /// Switch On's press: approves exactly the files the card read out and
    /// switches the package on, as the settings switch does. Never called
    /// but from that press.
    pub(crate) fn accept(self, cx: &mut App) {
        let tier = self.tier.as_ref().map(Tier::key);
        host::approve_as(&self.folder, tier, cx);
    }
}

/// A tier key with its interpreter's paths left out, the `run:` words a
/// [`Tier::key`] puts between its reads and its gaps.
fn without_interpreter(key: &str) -> String {
    let mut parts = key.split(" run:");
    let mut kept = parts.next().unwrap_or_default().to_string();
    for part in parts {
        if let Some(at) = part.find(" gap:") {
            kept.push_str(&part[at..]);
        }
    }
    kept
}

/// A renderer's tier as the card reads it out. Only what this system
/// enforces is promised: an older kernel's gaps get a line each.
fn tier_lines(tier: &Tier) -> Vec<SharedString> {
    match tier {
        Tier::Sandboxed {
            network,
            folders,
            interpreter,
            gaps,
        } => {
            let asks = *network || !folders.is_empty() || !interpreter.is_empty();
            let mut lines = vec![match asks {
                true => rox_i18n::t!("settings-plugins-card-sandboxed-asks"),
                false => rox_i18n::t!("settings-plugins-card-sandboxed"),
            }];
            // The paths themselves, since only they say whose program it is.
            if !interpreter.is_empty() {
                let paths: Vec<String> = interpreter
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect();
                lines.push(rox_i18n::t!(
                    "settings-plugins-card-interpreter",
                    paths = paths.join(", ")
                ));
            }
            if gaps.signals {
                lines.push(rox_i18n::t!("settings-plugins-card-gap-signals"));
            }
            if gaps.truncate {
                lines.push(rox_i18n::t!("settings-plugins-card-gap-truncate"));
            }
            lines
        }

        Tier::Unconfined => vec![rox_i18n::t!("settings-plugins-card-unconfined")],

        Tier::Permissions(why) => vec![match why {
            Unavailable::Platform => rox_i18n::t!("settings-plugins-card-host"),
            Unavailable::NoLandlock => rox_i18n::t!("settings-plugins-card-no-sandbox-kernel"),
            Unavailable::LandlockOff => rox_i18n::t!("settings-plugins-card-no-sandbox-off"),
            Unavailable::NoSeccomp => rox_i18n::t!("settings-plugins-card-no-sandbox-seccomp"),
            Unavailable::Failed(error) => rox_i18n::t!(
                "settings-plugins-card-no-sandbox-failed",
                error = error.as_str()
            ),
        }],
    }
}

/// The user's folders a renderer asks to read, past the library, named as
/// the card names them.
fn folder_list(manifest: &Manifest) -> String {
    let folders = manifest
        .capabilities
        .renderer
        .as_ref()
        .map(|cap| cap.access.folders())
        .unwrap_or_default();

    folders
        .into_iter()
        .filter_map(|folder| match folder {
            Folder::Library => None,
            Folder::Music => Some(rox_i18n::t!("settings-plugins-folder-music")),
            Folder::Pictures => Some(rox_i18n::t!("settings-plugins-folder-pictures")),
            Folder::Videos => Some(rox_i18n::t!("settings-plugins-folder-videos")),
            Folder::Documents => Some(rox_i18n::t!("settings-plugins-folder-documents")),
            Folder::Downloads => Some(rox_i18n::t!("settings-plugins-folder-downloads")),
        })
        .collect::<Vec<_>>()
        .join(", ")
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
        Grant::Network => rox_i18n::t!("settings-plugins-card-access-network"),
        Grant::ReadLibrary => rox_i18n::t!("settings-plugins-card-access-library"),
        Grant::ReadFolders => rox_i18n::t!(
            "settings-plugins-card-access-folders",
            folders = folder_list(manifest)
        ),
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
        Change::Unconfined(true) => rox_i18n::t!("settings-plugins-change-unconfined-on"),
        Change::Unconfined(false) => rox_i18n::t!("settings-plugins-change-unconfined-off"),
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

        assert_eq!(card.lines().len(), 1, "{:?}", card.lines());
        assert!(
            card.lines()[0].contains("lyrics, artist info"),
            "{}",
            card.lines()[0]
        );
    }

    #[test]
    fn the_cards_words_follow_the_language_after_it_opens() {
        let mut renderer = folder(&[("python3", true)]);
        renderer.manifest = Some(
            rox_plugins::manifest::parse(
                r#"{ "id": "bars", "name": "Bars", "version": "2", "api": 1,
                     "meta": { "author": "rox" },
                     "entry": { "script": { "path": "draw.py", "interpreter": "python3" } },
                     "capabilities": { "renderer": { "label": "Bars", "palette": true } } }"#,
            )
            .unwrap(),
        );

        rox_i18n::pin_thread_locale(rox_i18n::SOURCE_LOCALE);
        let card = EnableCard::of(renderer, None).unwrap();
        let english = (card.lead(), card.lines());

        rox_i18n::pin_thread_locale("de");
        let german = (card.lead(), card.lines());
        rox_i18n::pin_thread_locale(rox_i18n::SOURCE_LOCALE);

        assert_eq!(english.0.len(), german.0.len());
        assert_eq!(english.1.len(), german.1.len());
        assert_ne!(english.0[0], german.0[0], "the by line");
        for (en, de) in english.1.iter().zip(&german.1) {
            assert_ne!(en, de, "every grant line");
        }
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
            card.lines().len(),
            1 + grants.len() + 1,
            "what it draws, each grant, the program: {:?}",
            card.lines()
        );
        assert!(card.lines()[0].contains("Bars"), "{}", card.lines()[0]);
        let manifest = renderer.manifest.clone().unwrap();
        for (line, grant) in card.lines()[1..].iter().zip(&grants) {
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
            again.lines().last(),
            Some(&rox_i18n::t!("settings-plugins-card-unchanged-renderer"))
        );

        // One whose description moved and nothing it declares.
        let mut described = renderer.clone();
        described.document = json!({ "id": "bars", "meta": { "description": "Louder." } });
        let redescribed = EnableCard::of(described, Some(&approved)).unwrap();
        assert_eq!(
            redescribed.lines().last(),
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
            asks.lines()[1..6],
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
        let lines = regained.lines();
        let tail = &lines[lines.len() - 3..];
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
            .lines()
            .into_iter()
            .chain(card.runs())
            .chain(again.lines())
            .chain(redescribed.lines())
            .chain(asks.lines())
            .chain(regained.lines())
            .chain(card.lead())
            .map(|line| line.to_lowercase())
            .collect();
        assert!(
            everything.iter().all(|line| !line.contains("plugin")),
            "{everything:?}"
        );
    }

    fn renderer_with(capabilities: Value) -> Loaded {
        let document = json!({
            "id": "bars", "name": "Bars", "version": "2", "api": 1,
            "entry": { "script": { "path": "draw.py", "interpreter": "python3" } },
            "capabilities": { "renderer": capabilities },
        });
        let mut renderer = folder(&[]);
        renderer.manifest = Some(rox_plugins::manifest::parse(&document.to_string()).unwrap());
        renderer.document = document;
        renderer
    }

    fn sandboxed(network: bool, folders: Vec<Folder>, abi: u32) -> Tier {
        Tier::Sandboxed {
            network,
            folders,
            interpreter: Vec::new(),
            gaps: rox_plugins::sandbox::Gaps::of_abi(abi),
        }
    }

    #[test]
    fn a_renderers_card_says_what_it_runs_as() {
        rox_i18n::pin_thread_locale(rox_i18n::SOURCE_LOCALE);

        assert_eq!(
            tier_lines(&sandboxed(false, Vec::new(), 7)),
            [SharedString::from(
                "Runs in a sandbox. It can use the graphics card and its own cache, \
                 and can't read your files or reach the network."
            )]
        );

        // An older kernel's card says what it can't stop, one line each.
        assert_eq!(
            tier_lines(&sandboxed(false, Vec::new(), 2)),
            [
                rox_i18n::t!("settings-plugins-card-sandboxed"),
                rox_i18n::t!("settings-plugins-card-gap-signals"),
                rox_i18n::t!("settings-plugins-card-gap-truncate"),
            ]
        );

        // An interpreter from home names its install, in one line.
        let pyenv = Tier::Sandboxed {
            network: false,
            folders: Vec::new(),
            interpreter: vec![
                "/home/u/.pyenv/versions/3.12.1/bin".into(),
                "/home/u/.pyenv/versions/3.12.1/lib".into(),
            ],
            gaps: rox_plugins::sandbox::Gaps::of_abi(7),
        };
        assert_eq!(
            tier_lines(&pyenv),
            [
                rox_i18n::t!("settings-plugins-card-sandboxed-asks"),
                SharedString::from(
                    "Can read and run its interpreter from outside the system's folders: \
                     /home/u/.pyenv/versions/3.12.1/bin, /home/u/.pyenv/versions/3.12.1/lib."
                ),
            ]
        );

        assert_eq!(
            tier_lines(&Tier::Unconfined),
            [rox_i18n::t!("settings-plugins-card-unconfined")]
        );
        assert_eq!(
            tier_lines(&Tier::Permissions(Unavailable::Platform)),
            [rox_i18n::t!("settings-plugins-card-host")]
        );
        assert_eq!(
            tier_lines(&Tier::Permissions(Unavailable::Failed("EPERM".into()))),
            [SharedString::from(
                "rox couldn't sandbox it: EPERM. If you switch it on, it runs as a \
                 program with your permissions until rox quits."
            )]
        );
    }

    #[test]
    fn each_ask_is_a_line_only_where_the_sandbox_holds() {
        rox_i18n::pin_thread_locale(rox_i18n::SOURCE_LOCALE);
        let renderer = renderer_with(json!({
            "label": "Bars",
            "access": { "network": true, "read": ["videos", "library", "pictures"] },
        }));

        let mut card = EnableCard::of(renderer, None).unwrap();
        card.tier = Some(sandboxed(true, vec![Folder::Library, Folder::Pictures], 7));
        assert_eq!(
            card.runs(),
            [rox_i18n::t!("settings-plugins-card-sandboxed-asks")]
        );
        let lines = card.lines();
        assert_eq!(
            lines[lines.len() - 3..],
            [
                rox_i18n::t!("settings-plugins-card-access-network"),
                rox_i18n::t!("settings-plugins-card-access-library"),
                SharedString::from(
                    "Can read these folders of yours, without changing them: Pictures, Videos."
                ),
            ]
        );

        // Outside the sandbox the asks mean nothing, so they aren't said.
        card.tier = Some(Tier::Permissions(Unavailable::NoLandlock));
        let unsandboxed = card.lines();
        assert_eq!(unsandboxed.len(), lines.len() - 3, "{unsandboxed:?}");
        assert_eq!(
            card.runs(),
            [rox_i18n::t!("settings-plugins-card-no-sandbox-kernel")]
        );
    }

    #[test]
    fn the_same_files_approved_as_another_tier_say_so() {
        rox_i18n::pin_thread_locale(rox_i18n::SOURCE_LOCALE);
        let renderer = renderer_with(json!({ "label": "Bars" }));
        let approved = PluginRecord {
            approved_manifest: renderer.document.clone(),
            sandbox: "sandboxed".into(),
            ..record(true, "beef")
        };

        let mut card = EnableCard::of(renderer.clone(), Some(&approved)).unwrap();
        card.tier = Some(Tier::Permissions(Unavailable::LandlockOff));
        assert_eq!(
            card.lines().last(),
            Some(&rox_i18n::t!("settings-plugins-card-sandbox-changed"))
        );

        card.tier = Some(sandboxed(false, Vec::new(), 7));
        assert!(
            !card
                .lines()
                .contains(&rox_i18n::t!("settings-plugins-card-sandbox-changed"))
        );

        // Approved with its interpreter at another path.
        let mut card = EnableCard::of(
            renderer.clone(),
            Some(&PluginRecord {
                sandbox: "sandboxed run:/home/u/.pyenv/versions/3.12.1/bin gap:signals".into(),
                ..approved.clone()
            }),
        )
        .unwrap();
        card.tier = Some(Tier::Sandboxed {
            network: false,
            folders: Vec::new(),
            interpreter: vec!["/home/u/.pyenv/versions/3.13.0/bin".into()],
            gaps: rox_plugins::sandbox::Gaps::of_abi(5),
        });
        assert_eq!(
            card.lines().last(),
            Some(&rox_i18n::t!("settings-plugins-card-interpreter-moved"))
        );
        assert_eq!(
            without_interpreter("sandboxed read:music run:/a b/bin run:/a b/lib gap:signals"),
            "sandboxed read:music gap:signals"
        );

        // A manifest that now asks to run unconfined says that instead.
        let loose = renderer_with(json!({ "label": "Bars", "unconfined": true }));
        let card = EnableCard::of(loose, Some(&approved)).unwrap();
        assert!(
            card.lines()
                .contains(&rox_i18n::t!("settings-plugins-change-unconfined-on")),
            "{:?}",
            card.lines()
        );
        assert_eq!(
            card.runs(),
            [rox_i18n::t!("settings-plugins-card-unconfined")]
        );
    }
}
