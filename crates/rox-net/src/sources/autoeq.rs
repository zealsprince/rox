//! AutoEq client and profile parser: the master index, search over it, and
//! the EQ text formats (FixedBandEQ, ParametricEQ, GraphicEQ, CSV) folded onto
//! the ten ISO octave bands.

use std::cmp::Ordering;

use crate::providers::{agent, net_reason};

pub const BAND_HZ: [f32; 10] = [
    32.0, 64.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];

pub const BANDS: usize = BAND_HZ.len();

pub const GAIN_MAX_DB: f32 = 12.0;

pub const INDEX_URL: &str =
    "https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/results/INDEX.md";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutoEqEntry {
    pub name: String,
    /// Relative to `results/`, percent-encoding kept, e.g. "oratory1990/over-ear/Sennheiser%20HD%20600".
    pub path: String,
    /// The measurement rig, e.g. "crinacle on 711".
    pub source: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AutoEqProfile {
    pub name: String,
    pub preamp_db: Option<f32>,
    pub gains_db: [f32; BANDS],
}

/// One octave, matching `rox_playback::eq::Q_DEFAULT`. Mirrored rather than
/// imported: this module is the file format and knows nothing of the engine.
pub const Q_OCTAVE: f32 = std::f32::consts::SQRT_2;

/// A hand-shaped band needs all three numbers; [`AutoEqProfile`] is the
/// flattened gains-only case.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BandSetting {
    pub hz: f32,
    pub gain_db: f32,
    pub q: f32,
}

/// The run up to the delimiter that closes the one already open, counting
/// nested pairs; `text` starts one byte past the opener. Byte offsets are safe
/// because the delimiters are ASCII.
fn balanced_run(text: &str, open: u8, close: u8) -> Option<(&str, usize)> {
    let mut depth = 1usize;

    for (index, byte) in text.bytes().enumerate() {
        if byte == open {
            depth += 1;
        } else if byte == close {
            depth -= 1;

            if depth == 0 {
                return Some((&text[..index], index + 1));
            }
        }
    }

    None
}

/// Lines look like `- [Name (Variant)](./rig/Name%20(Variant)) by rig`. The
/// index leaves parentheses literal in paths, so both halves are scanned by
/// counting pairs: stopping at the first `)` fetches a path that 404s.
pub fn parse_index(text: &str) -> Vec<AutoEqEntry> {
    let mut entries = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("- [") else {
            continue;
        };

        let Some((name, after_name)) = balanced_run(rest, b'[', b']') else {
            continue;
        };
        let Some(link) = rest[after_name..].strip_prefix('(') else {
            continue;
        };
        let Some((raw_path, after_link)) = balanced_run(link, b'(', b')') else {
            continue;
        };

        // Kept exactly as written, encoding and all: it goes back out as a URL.
        let path = raw_path.strip_prefix("./").unwrap_or(raw_path);
        // A few upstream lines carry a stray extra ")"; keep it out of the
        // source.
        let trailing = link[after_link..].trim().trim_start_matches(')').trim();
        let source = trailing.strip_prefix("by ").unwrap_or(trailing);

        if !name.is_empty() && !path.is_empty() {
            entries.push(AutoEqEntry {
                name: name.to_string(),
                path: path.to_string(),
                source: source.to_string(),
            });
        }
    }

    entries
}

pub fn filter_entries<'a>(
    entries: &'a [AutoEqEntry],
    query: &str,
    limit: usize,
) -> Vec<&'a AutoEqEntry> {
    let query = query.trim();
    if query.is_empty() {
        return entries.iter().take(limit).collect();
    }

    let terms: Vec<String> = query.split_whitespace().map(|s| s.to_lowercase()).collect();

    let mut matches: Vec<(&'a AutoEqEntry, usize)> = entries
        .iter()
        .filter_map(|entry| {
            let name_lower = entry.name.to_lowercase();
            let source_lower = entry.source.to_lowercase();

            let all_match = terms
                .iter()
                .all(|t| name_lower.contains(t) || source_lower.contains(t));

            if !all_match {
                return None;
            }

            // Lower is better.
            let full_lower = query.to_lowercase();
            let score = if name_lower == full_lower {
                0
            } else if name_lower.starts_with(&full_lower) {
                1
            } else if name_lower.contains(&full_lower) {
                2
            } else {
                3 + entry.name.len()
            };

            Some((entry, score))
        })
        .collect();

    matches.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.name.cmp(&b.0.name)));
    matches.into_iter().take(limit).map(|(e, _)| e).collect()
}

pub fn fixed_band_url(path: &str) -> String {
    let clean = path.trim_start_matches("./").trim_matches('/');
    let folder = clean.rsplit('/').next().unwrap_or(clean);
    format!(
        "https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/results/{clean}/{folder}%20FixedBandEQ.txt"
    )
}

pub fn parametric_url(path: &str) -> String {
    let clean = path.trim_start_matches("./").trim_matches('/');
    let folder = clean.rsplit('/').next().unwrap_or(clean);
    format!(
        "https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/results/{clean}/{folder}%20ParametricEQ.txt"
    )
}

pub fn fetch_index() -> Result<String, String> {
    agent()
        .get(INDEX_URL)
        .call()
        .map_err(|e| net_reason(&e))?
        .into_string()
        .map_err(|e| e.to_string())
}

pub fn fetch_profile(path: &str, name: &str) -> Result<AutoEqProfile, String> {
    let url = fixed_band_url(path);
    let response = agent().get(&url).call();

    let body = match response {
        Ok(res) => res.into_string().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(404, _)) => {
            let fallback_url = parametric_url(path);
            agent()
                .get(&fallback_url)
                .call()
                .map_err(|e| net_reason(&e))?
                .into_string()
                .map_err(|e| e.to_string())?
        }
        Err(e) => return Err(net_reason(&e)),
    };

    parse_profile(name, &body)
}

fn closest_band(hz: f32) -> usize {
    if hz <= 0.0 {
        return 0;
    }
    let log_hz = hz.log10();
    BAND_HZ
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            let da = (log_hz - a.log10()).abs();
            let db = (log_hz - b.log10()).abs();
            da.partial_cmp(&db).unwrap_or(Ordering::Equal)
        })
        .map(|(idx, _)| idx)
        .unwrap_or(0)
}

/// Reads FixedBandEQ/ParametricEQ filter lines, `GraphicEQ:` lines, and
/// CSV response points.
pub fn parse_profile(name: &str, text: &str) -> Result<AutoEqProfile, String> {
    let mut gains_db = [0.0f32; BANDS];
    let mut preamp_db = None;
    let mut has_filters = false;
    let mut graphic_points: Vec<(f32, f32)> = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if line.to_lowercase().starts_with("preamp:") {
            if let Some(val_str) = line.split(':').nth(1) {
                let clean = val_str.to_lowercase().replace("db", "").trim().to_string();
                if let Ok(p) = clean.parse::<f32>() {
                    preamp_db = Some(p);
                }
            }
            continue;
        }

        if line.to_lowercase().starts_with("graphiceq:") {
            let data = line.split(':').nth(1).unwrap_or("");
            for pair in data.split(';') {
                let parts: Vec<&str> = pair.split_whitespace().collect();
                if parts.len() >= 2
                    && let (Ok(hz), Ok(db)) = (parts[0].parse::<f32>(), parts[1].parse::<f32>())
                {
                    graphic_points.push((hz, db));
                }
            }
            continue;
        }

        if line.to_lowercase().starts_with("filter") {
            if let Some(band) = filter_band(line) {
                gains_db[closest_band(band.hz)] = band.gain_db.clamp(-GAIN_MAX_DB, GAIN_MAX_DB);
                has_filters = true;
            }
            continue;
        }

        let parts: Vec<&str> = line
            .split(&[',', ' ', '\t'][..])
            .filter(|s| !s.is_empty())
            .collect();
        if parts.len() >= 2
            && let (Ok(hz), Ok(db)) = (parts[0].parse::<f32>(), parts[1].parse::<f32>())
        {
            graphic_points.push((hz, db));
        }
    }

    if !has_filters && !graphic_points.is_empty() {
        graphic_points.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal));
        for (i, &band_hz) in BAND_HZ.iter().enumerate() {
            let gain = interpolate_gain(&graphic_points, band_hz);
            gains_db[i] = gain.clamp(-GAIN_MAX_DB, GAIN_MAX_DB);
        }
        has_filters = true;
    }

    if !has_filters && preamp_db.is_none() {
        return Err("no valid filter settings found in profile".to_string());
    }

    Ok(AutoEqProfile {
        name: name.to_string(),
        preamp_db,
        gains_db,
    })
}

/// Equalizer APO parametric text, the format AutoEq publishes. Parametric
/// rather than GraphicEQ because bands here move and narrow; written at the
/// window's precision so a save and reload is a no-op.
pub fn format_bands(name: &str, bands: &[BandSetting], preamp_db: Option<f32>) -> String {
    let mut out = format!("# {name}\n");

    if let Some(db) = preamp_db {
        out.push_str(&format!("Preamp: {db:.1} dB\n"));
    }

    for (index, band) in bands.iter().enumerate() {
        out.push_str(&format!(
            "Filter {}: ON PK Fc {:.1} Hz Gain {:.2} dB Q {:.2}\n",
            index + 1,
            band.hz,
            band.gain_db,
            band.q
        ));
    }

    out
}

/// One band per enabled filter line. None when there are none, so the caller
/// falls back to [`parse_profile`].
pub fn parse_bands(text: &str) -> Option<Vec<BandSetting>> {
    let bands: Vec<BandSetting> = text
        .lines()
        .map(str::trim)
        .filter(|line| line.to_lowercase().starts_with("filter"))
        .filter_map(filter_band)
        .collect();

    (!bands.is_empty()).then_some(bands)
}

/// A line with no Q reads at one octave, which is what fixed-band files mean.
fn filter_band(line: &str) -> Option<BandSetting> {
    let lower = line.to_lowercase();
    if !lower.contains(" on ") && !lower.contains(": on ") {
        return None;
    }

    let mut hz: Option<f32> = None;
    let mut gain_db: Option<f32> = None;
    let mut q: Option<f32> = None;

    // Fields are labelled, and shelves carry fewer than peaks, so read by label.
    let tokens: Vec<&str> = line.split_whitespace().collect();
    for (index, token) in tokens.iter().enumerate() {
        let Some(value) = tokens.get(index + 1) else {
            continue;
        };
        // The unit is attached ("31Hz") in some files and separate in others.
        let number = |unit: &str| {
            let lower = value.to_lowercase();
            lower
                .strip_suffix(unit)
                .unwrap_or(lower.as_str())
                .parse::<f32>()
                .ok()
        };
        if token.eq_ignore_ascii_case("fc") {
            hz = number("hz");
        } else if token.eq_ignore_ascii_case("gain") {
            gain_db = number("db");
        } else if token.eq_ignore_ascii_case("q") {
            q = number("");
        }
    }

    Some(BandSetting {
        hz: hz?,
        gain_db: gain_db?,
        q: q.unwrap_or(Q_OCTAVE),
    })
}

fn interpolate_gain(points: &[(f32, f32)], target_hz: f32) -> f32 {
    if points.is_empty() {
        return 0.0;
    }
    if points.len() == 1 || target_hz <= points[0].0 {
        return points[0].1;
    }
    if target_hz >= points[points.len() - 1].0 {
        return points[points.len() - 1].1;
    }

    for window in points.windows(2) {
        let (f0, g0) = window[0];
        let (f1, g1) = window[1];
        if target_hz >= f0 && target_hz <= f1 {
            if (f1 - f0).abs() < 1e-6 {
                return g0;
            }
            let t = (target_hz.log10() - f0.log10()) / (f1.log10() - f0.log10());
            return g0 + t * (g1 - g0);
        }
    }

    points[points.len() - 1].1
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_INDEX: &str = r#"
# Index
This is a list of all equalization profiles.

- [1Custom SA02](./crinacle/711%20in-ear/1Custom%20SA02) by crinacle on 711
- [Sennheiser HD 600](./oratory1990/over-ear/Sennheiser%20HD%20600) by oratory1990
- [Sennheiser HD 600](./crinacle/GRAS%2043AG-7%20over-ear/Sennheiser%20HD%20600) by crinacle on GRAS 43AG-7
- [Apple AirPods Pro 2](./Rtings/in-ear/Apple%20AirPods%20Pro%202) by Rtings
"#;

    const SAMPLE_FIXED_BAND: &str = r#"
Preamp: -7.5 dB
Filter 1: ON PK Fc 31 Hz Gain 6.9 dB Q 1.41
Filter 2: ON PK Fc 62 Hz Gain 3.3 dB Q 1.41
Filter 3: ON PK Fc 125 Hz Gain -1.1 dB Q 1.41
Filter 4: ON PK Fc 250 Hz Gain -1.6 dB Q 1.41
Filter 5: ON PK Fc 500 Hz Gain 0.6 dB Q 1.41
Filter 6: ON PK Fc 1000 Hz Gain -0.8 dB Q 1.41
Filter 7: ON PK Fc 2000 Hz Gain 0.1 dB Q 1.41
Filter 8: ON PK Fc 4000 Hz Gain -1.0 dB Q 1.41
Filter 9: ON PK Fc 8000 Hz Gain 3.9 dB Q 1.41
Filter 10: ON PK Fc 16000 Hz Gain -6.5 dB Q 1.41
"#;

    const SAMPLE_GRAPHIC_EQ: &str = r#"
GraphicEQ: 20 -0.3; 32 6.9; 64 3.3; 125 -1.1; 250 -1.6; 500 0.6; 1000 -0.8; 2000 0.1; 4000 -1.0; 8000 3.9; 16000 -6.5; 20000 -8.0
"#;

    #[test]
    fn test_parse_index() {
        let entries = parse_index(SAMPLE_INDEX);
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].name, "1Custom SA02");
        assert_eq!(entries[0].path, "crinacle/711%20in-ear/1Custom%20SA02");
        assert_eq!(entries[0].source, "crinacle on 711");

        assert_eq!(entries[1].name, "Sennheiser HD 600");
        assert_eq!(
            entries[1].path,
            "oratory1990/over-ear/Sennheiser%20HD%20600"
        );
        assert_eq!(entries[1].source, "oratory1990");
    }

    /// Real index lines with parentheses in name and path, including two
    /// runs back to back.
    #[test]
    fn parentheses_in_a_name_and_a_path_survive() {
        const LINES: &str = r#"
- [1MORE Aero (ANC Off)](./HypetheSonics/GRAS%20RA0045%20in-ear/1MORE%20Aero%20(ANC%20Off)) by HypetheSonics on GRAS RA0045
- [1MORE Aero (ANC On)](./HypetheSonics/GRAS%20RA0045%20in-ear/1MORE%20Aero%20(ANC%20On)) by HypetheSonics on GRAS RA0045
- [1MORE Aero (transparency mode)](./HypetheSonics/GRAS%20RA0045%20in-ear/1MORE%20Aero%20(transparency%20mode)) by HypetheSonics on GRAS RA0045
- [Audeze LCD-X (pre-2021) (worn earpads)](./crinacle/GRAS%2043AG-7%20over-ear/Audeze%20LCD-X%20(pre-2021)%20(worn%20earpads)) by crinacle on GRAS 43AG-7
- [Sennheiser HD 600](./oratory1990/over-ear/Sennheiser%20HD%20600) by oratory1990
"#;

        let entries = parse_index(LINES);
        assert_eq!(entries.len(), 5);

        assert_eq!(entries[0].name, "1MORE Aero (ANC Off)");
        assert_eq!(
            entries[0].path,
            "HypetheSonics/GRAS%20RA0045%20in-ear/1MORE%20Aero%20(ANC%20Off)"
        );
        assert_eq!(entries[0].source, "HypetheSonics on GRAS RA0045");

        assert_eq!(entries[2].name, "1MORE Aero (transparency mode)");
        assert_eq!(
            entries[2].path,
            "HypetheSonics/GRAS%20RA0045%20in-ear/1MORE%20Aero%20(transparency%20mode)"
        );
        assert_eq!(entries[2].source, "HypetheSonics on GRAS RA0045");

        assert_eq!(entries[3].name, "Audeze LCD-X (pre-2021) (worn earpads)");
        assert_eq!(
            entries[3].path,
            "crinacle/GRAS%2043AG-7%20over-ear/Audeze%20LCD-X%20(pre-2021)%20(worn%20earpads)"
        );
        assert_eq!(entries[3].source, "crinacle on GRAS 43AG-7");

        assert_eq!(entries[4].name, "Sennheiser HD 600");
        assert_eq!(
            entries[4].path,
            "oratory1990/over-ear/Sennheiser%20HD%20600"
        );
        assert_eq!(entries[4].source, "oratory1990");

        assert!(
            fixed_band_url(&entries[0].path).ends_with(
                "/1MORE%20Aero%20(ANC%20Off)/1MORE%20Aero%20(ANC%20Off)%20FixedBandEQ.txt"
            )
        );
    }

    #[test]
    fn a_stray_closing_parenthesis_stays_out_of_the_source() {
        const LINES: &str = r#"
- [Alpha Omega Omega on-off-off)](./Super%20Review/in-ear/Alpha%20Omega%20Omega%20on-off-off)) by Super Review
- [Steven Slate Audio VSX (passive plugin inactive))](./oratory1990/over-ear/Steven%20Slate%20Audio%20VSX%20(passive%20plugin%20inactive))) by oratory1990
"#;

        let entries = parse_index(LINES);
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0].path,
            "Super%20Review/in-ear/Alpha%20Omega%20Omega%20on-off-off"
        );
        assert_eq!(entries[0].source, "Super Review");
        assert_eq!(
            entries[1].path,
            "oratory1990/over-ear/Steven%20Slate%20Audio%20VSX%20(passive%20plugin%20inactive)"
        );
        assert_eq!(entries[1].source, "oratory1990");
    }

    /// Percent-encoded parentheses carry no depth for the scan and go back out
    /// untouched.
    #[test]
    fn encoded_parentheses_in_a_path_are_left_alone() {
        const LINE: &str = "- [1MORE Aero (ANC Off)](./HypetheSonics/GRAS%20RA0045%20in-ear/1MORE%20Aero%20%28ANC%20Off%29) by HypetheSonics on GRAS RA0045";

        let entries = parse_index(LINE);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "1MORE Aero (ANC Off)");
        assert_eq!(
            entries[0].path,
            "HypetheSonics/GRAS%20RA0045%20in-ear/1MORE%20Aero%20%28ANC%20Off%29"
        );
        assert_eq!(entries[0].source, "HypetheSonics on GRAS RA0045");
    }

    #[test]
    fn an_unclosed_link_is_skipped() {
        assert!(parse_index("- [1MORE Aero (ANC Off)(./x/y) by someone").is_empty());
        assert!(parse_index("- [1MORE Aero](./x/1MORE%20Aero%20(ANC by someone").is_empty());
    }

    #[test]
    fn test_filter_entries() {
        let entries = parse_index(SAMPLE_INDEX);

        let hits = filter_entries(&entries, "hd 600", 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].name, "Sennheiser HD 600");

        let hits = filter_entries(&entries, "airpods", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "Apple AirPods Pro 2");

        let hits = filter_entries(&entries, "oratory", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source, "oratory1990");
    }

    #[test]
    fn test_fixed_band_url() {
        let url = fixed_band_url("oratory1990/over-ear/Sennheiser%20HD%20600");
        assert_eq!(
            url,
            "https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/results/oratory1990/over-ear/Sennheiser%20HD%20600/Sennheiser%20HD%20600%20FixedBandEQ.txt"
        );
    }

    #[test]
    fn test_parse_fixed_band_profile() {
        let profile = parse_profile("Sennheiser HD 600", SAMPLE_FIXED_BAND).unwrap();
        assert_eq!(profile.name, "Sennheiser HD 600");
        assert_eq!(profile.preamp_db, Some(-7.5));
        assert_eq!(profile.gains_db[0], 6.9);
        assert_eq!(profile.gains_db[1], 3.3);
        assert_eq!(profile.gains_db[2], -1.1);
        assert_eq!(profile.gains_db[3], -1.6);
        assert_eq!(profile.gains_db[4], 0.6);
        assert_eq!(profile.gains_db[5], -0.8);
        assert_eq!(profile.gains_db[6], 0.1);
        assert_eq!(profile.gains_db[7], -1.0);
        assert_eq!(profile.gains_db[8], 3.9);
        assert_eq!(profile.gains_db[9], -6.5);
    }

    #[test]
    fn test_parse_graphic_eq_profile() {
        let profile = parse_profile("Test GraphicEQ", SAMPLE_GRAPHIC_EQ).unwrap();
        assert_eq!(profile.name, "Test GraphicEQ");
        assert!((profile.gains_db[0] - 6.9).abs() < 0.05);
        assert!((profile.gains_db[1] - 3.3).abs() < 0.05);
        assert!((profile.gains_db[2] - -1.1).abs() < 0.05);
        assert!((profile.gains_db[9] - -6.5).abs() < 0.05);
    }

    /// Center, gain and width all survive a preset file, including bands
    /// dragged off their octave. The reason presets aren't GraphicEQ.
    #[test]
    fn bands_round_trip_through_a_preset_file() {
        let shaped = vec![
            BandSetting {
                hz: 32.0,
                gain_db: 4.25,
                q: Q_OCTAVE,
            },
            BandSetting {
                hz: 64.0,
                gain_db: -2.5,
                q: Q_OCTAVE,
            },
            BandSetting {
                hz: 125.0,
                gain_db: 0.0,
                q: Q_OCTAVE,
            },
            BandSetting {
                hz: 250.0,
                gain_db: 0.0,
                q: Q_OCTAVE,
            },
            BandSetting {
                hz: 500.0,
                gain_db: 0.0,
                q: Q_OCTAVE,
            },
            BandSetting {
                hz: 1350.0,
                gain_db: -6.75,
                q: 6.4,
            },
            BandSetting {
                hz: 2000.0,
                gain_db: 0.0,
                q: Q_OCTAVE,
            },
            BandSetting {
                hz: 3456.7,
                gain_db: 1.5,
                q: 0.35,
            },
            BandSetting {
                hz: 8000.0,
                gain_db: 0.0,
                q: Q_OCTAVE,
            },
            BandSetting {
                hz: 16000.0,
                gain_db: -3.0,
                q: Q_OCTAVE,
            },
        ];
        let text = format_bands("Night Shift", &shaped, None);
        let back = parse_bands(&text).expect("a preset file reads back as bands");

        assert_eq!(back.len(), shaped.len());
        for (read, wrote) in back.iter().zip(&shaped) {
            assert!((read.hz - wrote.hz).abs() < 0.05, "{read:?} vs {wrote:?}");
            assert!((read.gain_db - wrote.gain_db).abs() < 0.005);
            assert!((read.q - wrote.q).abs() < 0.005);
        }

        let profile = parse_profile("Night Shift", &text).expect("and as a profile");
        assert!((profile.gains_db[0] - 4.25).abs() < 0.01);
        assert!((profile.gains_db[9] - -3.0).abs() < 0.01);
    }

    #[test]
    fn a_preamp_is_written_and_read_back() {
        let text = format_bands(
            "HD 600",
            &[BandSetting {
                hz: 32.0,
                gain_db: 6.9,
                q: Q_OCTAVE,
            }],
            Some(-7.5),
        );
        assert!(text.contains("Preamp: -7.5 dB"));
        assert_eq!(
            parse_profile("HD 600", &text).unwrap().preamp_db,
            Some(-7.5)
        );
    }

    #[test]
    fn a_graphic_curve_holds_no_bands() {
        assert!(parse_bands(SAMPLE_GRAPHIC_EQ).is_none());
        let bands = parse_bands(SAMPLE_FIXED_BAND).expect("filter lines are bands");
        assert_eq!(bands.len(), 10);
        assert!((bands[0].hz - 31.0).abs() < 0.05);
        assert!((bands[0].gain_db - 6.9).abs() < 0.005);
        assert!((bands[0].q - 1.41).abs() < 0.005);
    }

    #[test]
    fn off_and_half_written_filters_are_skipped() {
        let text = "Filter 1: OFF PK Fc 31 Hz Gain 6.9 dB Q 1.41\n\
                    Filter 2: ON PK Fc 62 Hz\n\
                    Filter 3: ON PK Fc 125 Hz Gain -1.1 dB Q 1.41";
        let bands = parse_bands(text).expect("the one whole line");
        assert_eq!(bands.len(), 1);
        assert!((bands[0].hz - 125.0).abs() < 0.05);
    }
}
