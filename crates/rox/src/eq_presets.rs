//! Saved equalizer curves: one Equalizer APO text file per preset under
//! [`settings::eq_presets_dir`], so a saved curve is already an exported one
//! and a file dropped in the folder joins the picker. Applying one is
//! [`rox_services::player::apply_eq_bands`]; this module is the folder.

use std::path::Path;

use gpui::{App, Context, Global, Subscription};

use rox_core::settings;
use rox_net::sources::autoeq::{self, BandSetting};
use rox_playback::eq::BANDS;
use rox_services::player;

fn file_name(name: &str) -> String {
    format!("{}.txt", settings::safe_file_stem(name, "preset"))
}

/// Alphabetical. A directory read only: the picker lists these on every open.
pub fn list() -> Vec<String> {
    list_in(&settings::eq_presets_dir())
}

fn list_in(dir: &Path) -> Vec<String> {
    let Ok(dir) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = dir
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "txt"))
        .filter_map(|path| {
            let stem = path.file_stem()?.to_string_lossy().into_owned();
            (!stem.trim().is_empty()).then_some(stem)
        })
        .collect();
    names.sort_by_key(|name| name.to_lowercase());
    names
}

/// Write a curve under `name`, replacing whatever it held. `preamp_db` rides
/// along for other players; rox has no preamp of its own.
///
/// Returns the name folded to a filename stem, which is what the picker lists.
pub fn save(
    name: &str,
    bands: &[BandSetting],
    preamp_db: Option<f32>,
    cx: &mut App,
) -> Option<String> {
    let saved = save_in(&settings::eq_presets_dir(), name, bands, preamp_db);
    if saved.is_some() {
        changed(cx);
    }
    saved
}

fn save_in(
    dir: &Path,
    name: &str,
    bands: &[BandSetting],
    preamp_db: Option<f32>,
) -> Option<String> {
    if let Err(e) = std::fs::create_dir_all(dir) {
        log::warn!("eq presets: creating {}: {e}", dir.display());
        return None;
    }

    let stem = settings::safe_file_stem(name, "preset");
    let path = dir.join(format!("{stem}.txt"));
    match std::fs::write(&path, autoeq::format_bands(&stem, bands, preamp_db)) {
        Ok(()) => Some(stem),
        Err(e) => {
            log::warn!("eq presets: writing {}: {e}", path.display());
            None
        }
    }
}

pub fn load(name: &str) -> Option<Vec<BandSetting>> {
    load_in(&settings::eq_presets_dir(), name)
}

fn load_in(dir: &Path, name: &str) -> Option<Vec<BandSetting>> {
    read_curve(&std::fs::read_to_string(dir.join(file_name(name))).ok()?)
}

/// The file's own bands when it has a full set, otherwise the profile parser's
/// gains projected onto the ISO octaves at one octave wide, so a GraphicEQ
/// line or a CSV dropped in the folder still applies.
pub fn read_curve(text: &str) -> Option<Vec<BandSetting>> {
    if let Some(bands) = autoeq::parse_bands(text)
        && bands.len() == BANDS
    {
        return Some(bands);
    }

    let profile = autoeq::parse_profile("preset", text).ok()?;
    Some(
        autoeq::BAND_HZ
            .iter()
            .zip(profile.gains_db)
            .map(|(&hz, gain_db)| BandSetting {
                hz,
                gain_db,
                q: autoeq::Q_OCTAVE,
            })
            .collect(),
    )
}

pub fn remove(name: &str, cx: &mut App) {
    remove_in(&settings::eq_presets_dir(), name);
    changed(cx);
}

fn remove_in(dir: &Path, name: &str) {
    let path = dir.join(file_name(name));
    if let Err(e) = std::fs::remove_file(&path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        log::warn!("eq presets: deleting {}: {e}", path.display());
    }
}

/// Taken mutably whenever a preset is written or dropped, so listing windows
/// re-read the folder on change instead of polling.
#[derive(Default)]
struct PresetsChanged;

impl Global for PresetsChanged {}

fn changed(cx: &mut App) {
    let _ = cx.default_global::<PresetsChanged>();
}

pub fn observe<V: 'static>(
    cx: &mut Context<V>,
    on_change: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> Subscription {
    cx.observe_global::<PresetsChanged>(move |view, cx| on_change(view, cx))
}

pub fn live_bands() -> Vec<BandSetting> {
    (0..BANDS)
        .map(|band| BandSetting {
            hz: player::eq_freq(band),
            gain_db: player::eq_gain(band),
            q: player::eq_q(band),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rox-eq-presets-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn shaped() -> Vec<BandSetting> {
        (0..BANDS)
            .map(|band| BandSetting {
                hz: autoeq::BAND_HZ[band],
                gain_db: band as f32 - 4.0,
                q: autoeq::Q_OCTAVE,
            })
            .collect()
    }

    /// The folder round trip; the file format has its own test in rox-net.
    #[test]
    fn a_saved_curve_comes_back_by_name() {
        let dir = scratch("round-trip");
        let mut bands = shaped();
        bands[5] = BandSetting {
            hz: 1350.0,
            gain_db: -6.75,
            q: 6.4,
        };
        assert_eq!(
            save_in(&dir, "Night Shift", &bands, None).as_deref(),
            Some("Night Shift")
        );
        assert_eq!(list_in(&dir), vec!["Night Shift".to_string()]);

        let back = load_in(&dir, "Night Shift").expect("the preset loads");
        assert_eq!(back.len(), BANDS);
        assert!((back[5].hz - 1350.0).abs() < 0.05);
        assert!((back[5].gain_db - -6.75).abs() < 0.005);
        assert!((back[5].q - 6.4).abs() < 0.005);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_second_save_replaces_and_delete_removes() {
        let dir = scratch("replace");
        let mut bands = shaped();
        assert!(save_in(&dir, "Warm", &bands, None).is_some());
        bands[0].gain_db = 9.5;
        assert!(save_in(&dir, "Warm", &bands, None).is_some());

        assert_eq!(list_in(&dir).len(), 1);
        let back = load_in(&dir, "Warm").expect("the preset loads");
        assert!((back[0].gain_db - 9.5).abs() < 0.005);

        remove_in(&dir, "Warm");
        assert!(list_in(&dir).is_empty());
        assert!(load_in(&dir, "Warm").is_none());
        remove_in(&dir, "Warm");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_foreign_file_falls_back_to_the_octaves() {
        let text = "GraphicEQ: 20 -0.3; 32 6.9; 64 3.3; 125 -1.1; 250 -1.6; 500 0.6; \
                    1000 -0.8; 2000 0.1; 4000 -1.0; 8000 3.9; 16000 -6.5; 20000 -8.0";
        let bands = read_curve(text).expect("a graphic curve is still a curve");
        assert_eq!(bands.len(), BANDS);
        assert_eq!(bands[0].hz, autoeq::BAND_HZ[0]);
        assert!((bands[0].gain_db - 6.9).abs() < 0.05);
        assert!(bands.iter().all(|band| band.q == autoeq::Q_OCTAVE));
        assert!(read_curve("just some text\n").is_none());
    }

    #[test]
    fn a_name_that_cant_be_a_filename_is_folded() {
        assert_eq!(file_name("Night Shift"), "Night Shift.txt");
        assert_eq!(file_name("Drum & Bass / Neuro"), "Drum & Bass   Neuro.txt");
        assert_eq!(file_name("..."), "preset.txt");

        let dir = scratch("folded");
        assert_eq!(
            save_in(&dir, "Bass / Boost", &shaped(), None).as_deref(),
            Some("Bass   Boost")
        );
        assert_eq!(list_in(&dir), vec!["Bass   Boost".to_string()]);
        assert!(load_in(&dir, "Bass / Boost").is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
