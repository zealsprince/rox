//! Saved equalizer curves. A preset is one text file under
//! [`settings::eq_presets_dir`] named after the preset, so a saved curve is
//! already an exported one: drop a file someone sent you in the folder and it
//! joins the picker, delete it and it's gone. The workspace store's rule,
//! applied to something much smaller.
//!
//! The file is Equalizer APO parametric text, written and read by
//! [`rox_net::sources::autoeq`], which is also the format the AutoEq database
//! publishes. That makes the three ways a preset arrives one thing: the
//! browser saving a headphone profile, the window saving what you shaped, and
//! a file from somewhere else all land as the same file, and what rox writes
//! opens in every other equalizer that reads one.
//!
//! Nothing here touches the live curve. Applying a preset is
//! [`rox_services::player::apply_eq_bands`]; this module is the folder.

use std::path::Path;

use gpui::{App, Context, Global, Subscription};

use rox_core::settings;
use rox_net::sources::autoeq::{self, BandSetting};
use rox_playback::eq::BANDS;
use rox_services::player;

/// What a preset file is called on disk. The stem is the preset's name, so
/// it goes through the same fold every other name-as-filename does.
fn file_name(name: &str) -> String {
    format!("{}.txt", settings::safe_file_stem(name, "preset"))
}

/// Every saved preset's name, in alphabetical order. A directory read and
/// nothing more: the picker lists these on every open, and parsing ten
/// files to draw a menu would be ten files too many.
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

/// Write a curve under `name`, replacing whatever that name held. Saving
/// over a preset is how you update one, the panel presets' rule: the picker
/// shows the name, and a second save of it can't mean anything else.
///
/// `preamp_db` is the headphone profile's own trim where a save came from
/// one. rox has no preamp of its own to write, but the number belongs to the
/// profile and the file is something you hand to another player.
///
/// Answers with the name the preset landed under, which is the one passed in
/// folded to something a filename can hold. The picker lists file stems, so
/// a caller that wants to point at what it just saved needs that name rather
/// than the one it typed.
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

/// The curve a preset holds, or None when the file is gone or holds nothing
/// an equalizer could read.
pub fn load(name: &str) -> Option<Vec<BandSetting>> {
    load_in(&settings::eq_presets_dir(), name)
}

fn load_in(dir: &Path, name: &str) -> Option<Vec<BandSetting>> {
    read_curve(&std::fs::read_to_string(dir.join(file_name(name))).ok()?)
}

/// A file's text as a curve this window can take: its own bands when the
/// text carries a full set of them, and otherwise whatever the profile
/// parser can make of it, on the ISO octaves at one octave wide.
///
/// The fallback is what lets a `GraphicEQ:` line, a CSV of measurement
/// points or a parametric file with some other number of filters in it be
/// dropped in the folder and still apply. It's a projection, not a read:
/// those formats don't say where the bands are, so the octaves are the only
/// honest place to put them.
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

/// Drop the preset named `name`. A name nothing is saved under is a no-op.
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

/// Touched whenever a preset is written or dropped, so a window listing them
/// re-reads the folder on the change instead of polling a directory every
/// frame. The player's [`rox_services::player::observe_eq`] shape: it holds
/// nothing, because whoever wakes reads the folder back.
#[derive(Default)]
struct PresetsChanged;

impl Global for PresetsChanged {}

/// Tell the lists something was written. Taking the global mutably is the
/// whole notification.
fn changed(cx: &mut App) {
    let _ = cx.default_global::<PresetsChanged>();
}

/// Run `on_change` on `view` whenever the folder gains or loses a preset,
/// wherever it happened. The AutoEq browser saving a profile is the case
/// that needs it: the equalizer window's picker is the list it lands in, and
/// re-reading a directory every frame is no way to keep one honest.
pub fn observe<V: 'static>(
    cx: &mut Context<V>,
    on_change: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> Subscription {
    cx.observe_global::<PresetsChanged>(move |view, cx| on_change(view, cx))
}

/// The curve as it stands right now, read off the live parameters. What a
/// save from the equalizer window writes and what an export hands to the
/// file dialog.
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

    /// A folder of this test's own, since the store's own folder is the one
    /// the running app saves into.
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

    /// A curve saved under a name comes back by that name, band for band,
    /// and the name is what the list shows. The round trip the window's
    /// Save and its picker make between them, which nothing else covers:
    /// the format has its own test over in rox-net, and this is the folder.
    #[test]
    fn a_saved_curve_comes_back_by_name() {
        let dir = scratch("round-trip");
        let mut bands = shaped();
        // One band moved off its octave and narrowed, the case a file of
        // gains alone would lose.
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

    /// Saving under a name already taken replaces that preset rather than
    /// leaving two files the picker can't tell apart, and deleting takes the
    /// file with it.
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
        // A name nothing is saved under deletes without complaint.
        remove_in(&dir, "Warm");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file that came from somewhere else still applies: a graphic curve
    /// has no bands of its own, so it lands on the octaves at one octave
    /// wide rather than being refused.
    #[test]
    fn a_foreign_file_falls_back_to_the_octaves() {
        let text = "GraphicEQ: 20 -0.3; 32 6.9; 64 3.3; 125 -1.1; 250 -1.6; 500 0.6; \
                    1000 -0.8; 2000 0.1; 4000 -1.0; 8000 3.9; 16000 -6.5; 20000 -8.0";
        let bands = read_curve(text).expect("a graphic curve is still a curve");
        assert_eq!(bands.len(), BANDS);
        assert_eq!(bands[0].hz, autoeq::BAND_HZ[0]);
        assert!((bands[0].gain_db - 6.9).abs() < 0.05);
        assert!(bands.iter().all(|band| band.q == autoeq::Q_OCTAVE));
        // Nothing an equalizer can read at all comes back as nothing.
        assert!(read_curve("just some text\n").is_none());
    }

    /// Names double as filenames, so a name with a separator in it is folded
    /// the way every other name-as-filename is, and still lists and loads.
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
