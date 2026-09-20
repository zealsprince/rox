//! AutoEq database client and profile parser.
//!
//! AutoEq (https://github.com/jaakkopasanen/AutoEq) provides thousands of
//! headphone and in-ear monitor frequency response corrections measured by
//! oratory1990, Crinacle, Rtings, Super Review, and squig.link reviewers.
//!
//! Each profile in the repository includes a precomputed `FixedBandEQ.txt`
//! specifically optimized for 10-band graphic equalizers on the standard
//! ISO octave bands (31/32, 62/64, 125, 250, 500, 1000, 2000, 4000, 8000, 16000 Hz).
//!
//! This module parses the master index (`INDEX.md`), parses EQ formats
//! (FixedBandEQ, ParametricEQ, GraphicEQ, and CSV), and provides live
//! search and fetching routines.

use std::cmp::Ordering;

use crate::providers::{agent, net_reason};

/// The 10 standard ISO octave center frequencies in Hz used by graphic equalizers.
pub const BAND_HZ: [f32; 10] = [
    32.0, 64.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];

/// How many bands there are in the graphic equalizer.
pub const BANDS: usize = BAND_HZ.len();

/// The maximum cut or boost in dB supported by the equalizer.
pub const GAIN_MAX_DB: f32 = 12.0;

/// The raw GitHub URL for the AutoEq master results index.
pub const INDEX_URL: &str =
    "https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/results/INDEX.md";

/// An entry in the AutoEq index representing a headphone model measurement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutoEqEntry {
    /// The headphone or IEM model name, e.g. "Sennheiser HD 600".
    pub name: String,
    /// The relative path in the results repository, e.g. "oratory1990/over-ear/Sennheiser HD 600".
    pub path: String,
    /// The measurement source / rig, e.g. "oratory1990" or "crinacle on 711".
    pub source: String,
}

/// A parsed equalizer profile with 10 band gains and optional preamp.
#[derive(Clone, Debug, PartialEq)]
pub struct AutoEqProfile {
    pub name: String,
    pub preamp_db: Option<f32>,
    pub gains_db: [f32; BANDS],
}

/// The width a band takes when the text it came from doesn't say: one
/// octave, where `rox_playback::eq::Q_DEFAULT` sits and what the ISO octave
/// layout means. Mirrored here rather than imported, the way [`BAND_HZ`] is;
/// this module is the file format and knows nothing of the engine.
pub const Q_OCTAVE: f32 = std::f32::consts::SQRT_2;

/// One band of a curve as the player holds it: where it's centered, how hard
/// it pushes, and how wide it is. An [`AutoEqProfile`] is the flattened case
/// of this, ten gains welded to the ISO octaves, which is all a headphone
/// correction ever needs; a curve the user shaped by hand needs all three
/// numbers or a save throws away most of what they did.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BandSetting {
    pub hz: f32,
    pub gain_db: f32,
    pub q: f32,
}

/// Read the run that ends at the delimiter matching the one already open,
/// counting nested pairs on the way. `text` begins one byte past the opener.
/// Answers the run and the byte offset just past its closer, or nothing if
/// the line never closes it.
///
/// The delimiters are ASCII, so the byte offsets a byte scan produces are
/// char boundaries and the slices are safe on the accented model names in
/// the index.
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

/// Parse the AutoEq `INDEX.md` content into a list of [`AutoEqEntry`].
///
/// A line is a markdown link plus the rig the measurement came off:
/// `- [1MORE Aero (ANC Off)](./HypetheSonics/GRAS%20RA0045%20in-ear/1MORE%20Aero%20(ANC%20Off)) by HypetheSonics on GRAS RA0045`.
/// Both halves carry parentheses of their own: AutoEq marks the variant of a
/// measurement in the model name, and the directory is named after the
/// model. The index percent-encodes spaces but leaves parentheses literal,
/// so roughly a third of the file has them in the link target. Stopping at
/// the first `)` cuts that path at "1MORE Aero (ANC" and hands the rest to
/// the source column, which is how a row ends up reading
/// ") by HypetheSonics on GRAS RA0045" and Apply fetches a URL that 404s.
/// Both halves are scanned by counting pairs instead.
pub fn parse_index(text: &str) -> Vec<AutoEqEntry> {
    let mut entries = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("- [") else {
            continue;
        };

        // The name, then the link target, each read to the delimiter that
        // closes it rather than to the first one that turns up.
        let Some((name, after_name)) = balanced_run(rest, b'[', b']') else {
            continue;
        };
        let Some(link) = rest[after_name..].strip_prefix('(') else {
            continue;
        };
        let Some((raw_path, after_link)) = balanced_run(link, b'(', b')') else {
            continue;
        };

        // The path stays exactly as the index wrote it, encoding and all,
        // since it goes straight back out as a URL.
        let path = raw_path.strip_prefix("./").unwrap_or(raw_path);
        // Two lines in today's index close one parenthesis more than they
        // open, a stray ")" upstream typed into the model name. The path
        // still reads right; this keeps the leftover out of the source.
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

/// Filter and rank entries matching a multi-word search query.
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

            // All terms must appear in either name or source.
            let all_match = terms
                .iter()
                .all(|t| name_lower.contains(t) || source_lower.contains(t));

            if !all_match {
                return None;
            }

            // Score: lower is better.
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

/// The raw GitHub URL for a profile's `FixedBandEQ.txt`.
pub fn fixed_band_url(path: &str) -> String {
    let clean = path.trim_start_matches("./").trim_matches('/');
    let folder = clean.rsplit('/').next().unwrap_or(clean);
    format!(
        "https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/results/{clean}/{folder}%20FixedBandEQ.txt"
    )
}

/// The raw GitHub URL for a profile's `ParametricEQ.txt` as fallback.
pub fn parametric_url(path: &str) -> String {
    let clean = path.trim_start_matches("./").trim_matches('/');
    let folder = clean.rsplit('/').next().unwrap_or(clean);
    format!(
        "https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/results/{clean}/{folder}%20ParametricEQ.txt"
    )
}

/// Fetch the index content from GitHub. Blocking; run on background executor.
pub fn fetch_index() -> Result<String, String> {
    agent()
        .get(INDEX_URL)
        .call()
        .map_err(|e| net_reason(&e))?
        .into_string()
        .map_err(|e| e.to_string())
}

/// Fetch and parse a profile from GitHub. Blocking; run on background executor.
pub fn fetch_profile(path: &str, name: &str) -> Result<AutoEqProfile, String> {
    let url = fixed_band_url(path);
    let response = agent().get(&url).call();

    let body = match response {
        Ok(res) => res.into_string().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(404, _)) => {
            // Fallback to ParametricEQ.txt if FixedBandEQ.txt is not found
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

/// Find which band index in [`BAND_HZ`] a frequency is closest to (log scale).
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

/// Parse an equalizer profile from text.
///
/// Supports:
/// - AutoEq / Equalizer APO `FixedBandEQ.txt` & `ParametricEQ.txt`
/// - AutoEq / Wavelet / squig.link `GraphicEQ: ...` format
/// - Comma/space-separated CSV frequency response points
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

        // 1. Preamp line: e.g. "Preamp: -7.5 dB"
        if line.to_lowercase().starts_with("preamp:") {
            if let Some(val_str) = line.split(':').nth(1) {
                let clean = val_str.to_lowercase().replace("db", "").trim().to_string();
                if let Ok(p) = clean.parse::<f32>() {
                    preamp_db = Some(p);
                }
            }
            continue;
        }

        // 2. GraphicEQ format: e.g. "GraphicEQ: 20 -0.3; 25 -0.4; 32 -1.1; ..."
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

        // 3. Filter line: e.g. "Filter 1: ON PK Fc 31 Hz Gain 6.9 dB Q 1.41"
        if line.to_lowercase().starts_with("filter") {
            if let Some(band) = filter_band(line) {
                gains_db[closest_band(band.hz)] = band.gain_db.clamp(-GAIN_MAX_DB, GAIN_MAX_DB);
                has_filters = true;
            }
            continue;
        }

        // 4. Fallback line: CSV / space-separated "freq gain"
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

    // If GraphicEQ or CSV points were found and no discrete filters parsed:
    if !has_filters && !graphic_points.is_empty() {
        graphic_points.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal));
        for (i, &band_hz) in BAND_HZ.iter().enumerate() {
            // Interpolate gain at band_hz
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

/// Write a curve as Equalizer APO parametric text: a preamp line when there
/// is one, then a peaking filter per band. The format AutoEq itself
/// publishes, so a preset rox writes drops into Equalizer APO or anything
/// else that reads one, and the round trip keeps what a graphic curve would
/// drop on the floor: a `GraphicEQ:` line carries gains against fixed
/// frequencies, and the bands in this window move and narrow.
///
/// Written at the precision the window can be set to rather than AutoEq's
/// one decimal, so saving a preset and loading it back is a no-op instead of
/// a nudge.
pub fn format_bands(name: &str, bands: &[BandSetting], preamp_db: Option<f32>) -> String {
    // The name is the file's stem, so this line is for whoever opens the
    // file in an editor rather than for the reader below.
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

/// Read a curve back out of parametric text, one band per enabled filter in
/// file order. None when the text holds no filter lines at all, which is a
/// graphic curve or a response table and belongs to [`parse_profile`]:
/// those only answer in gains, and the caller wants every number it can get
/// before it falls back to that.
pub fn parse_bands(text: &str) -> Option<Vec<BandSetting>> {
    let bands: Vec<BandSetting> = text
        .lines()
        .map(str::trim)
        .filter(|line| line.to_lowercase().starts_with("filter"))
        .filter_map(filter_band)
        .collect();

    (!bands.is_empty()).then_some(bands)
}

/// One `Filter 1: ON PK Fc 31 Hz Gain 6.9 dB Q 1.41` line as a band. None
/// for a filter that's switched off, or one missing its center or its gain.
/// A line with no Q reads at one octave, which is what the fixed-band files
/// mean when they leave it out.
fn filter_band(line: &str) -> Option<BandSetting> {
    let lower = line.to_lowercase();
    // Only enabled filters count; a disabled one is a band the profile's
    // author took out.
    if !lower.contains(" on ") && !lower.contains(": on ") {
        return None;
    }

    let mut hz: Option<f32> = None;
    let mut gain_db: Option<f32> = None;
    let mut q: Option<f32> = None;

    // The fields are named in the line rather than positional, and a shelf
    // filter carries fewer of them than a peak, so each one is read off its
    // own label instead of by counting tokens.
    let tokens: Vec<&str> = line.split_whitespace().collect();
    for (index, token) in tokens.iter().enumerate() {
        let Some(value) = tokens.get(index + 1) else {
            continue;
        };
        // The unit rides the number in some files ("Fc 31Hz") and stands as
        // its own token in others, so the field's own unit comes off first.
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

/// Interpolate a gain value at `target_hz` from a sorted list of `(freq, gain)` points.
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
            // Linear interpolation in log10 frequency space
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

    /// Real lines off `results/INDEX.md`: the three 1MORE Aero variants,
    /// which carry parentheses in both the name and the path, a model whose
    /// path holds two parenthesised runs back to back, and a plain entry for
    /// company. Before the balanced scan the first four came back cut at
    /// "1MORE Aero (ANC", with the rest of the path sitting in the source
    /// column, and Apply on them fetched a path GitHub answers 404 to.
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

        // Two runs in a row, neither of them nested in the other.
        assert_eq!(entries[3].name, "Audeze LCD-X (pre-2021) (worn earpads)");
        assert_eq!(
            entries[3].path,
            "crinacle/GRAS%2043AG-7%20over-ear/Audeze%20LCD-X%20(pre-2021)%20(worn%20earpads)"
        );
        assert_eq!(entries[3].source, "crinacle on GRAS 43AG-7");

        // The plain case reads the same as it always did.
        assert_eq!(entries[4].name, "Sennheiser HD 600");
        assert_eq!(
            entries[4].path,
            "oratory1990/over-ear/Sennheiser%20HD%20600"
        );
        assert_eq!(entries[4].source, "oratory1990");

        // Every path joins to a URL that keeps the whole folder name, which
        // is the part that decides whether the fetch finds anything.
        assert!(
            fixed_band_url(&entries[0].path).ends_with(
                "/1MORE%20Aero%20(ANC%20Off)/1MORE%20Aero%20(ANC%20Off)%20FixedBandEQ.txt"
            )
        );
    }

    /// Two lines upstream carry a stray ")" after the link, so the scan ends
    /// with a leftover parenthesis where the source column starts. The path
    /// is whole either way; the source shouldn't wear the typo.
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

    /// Today's index leaves parentheses literal in the link target, but
    /// percent-encoded ones are just as valid a way to write the same path.
    /// They carry no depth for the scan, so the run ends where it should
    /// either way and the encoding goes back out to GitHub untouched.
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

    /// A line that opens a bracket or a parenthesis and never closes it is
    /// skipped rather than swallowing the rest of the line.
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

    /// A hand-shaped curve makes the whole trip through a preset file: every
    /// band's center, gain and width come back where they were, including the
    /// two that were dragged off their octave and narrowed. This is the
    /// reason presets aren't written as a graphic curve, so it's the test
    /// that would catch a change back to one.
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
            // Off its octave and narrow, the shape a graphic curve loses.
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

        // The same file is still a profile to anything that only speaks in
        // gains, which is what keeps a saved preset usable elsewhere.
        let profile = parse_profile("Night Shift", &text).expect("and as a profile");
        assert!((profile.gains_db[0] - 4.25).abs() < 0.01);
        assert!((profile.gains_db[9] - -3.0).abs() < 0.01);
    }

    /// A preamp survives the write, since a saved AutoEq profile carries one
    /// and the file is what a user hands to another player.
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

    /// A graphic curve holds no bands to read, so it falls through to the
    /// profile parser rather than coming back as an empty list.
    #[test]
    fn a_graphic_curve_holds_no_bands() {
        assert!(parse_bands(SAMPLE_GRAPHIC_EQ).is_none());
        // A fixed-band file does, at one octave each, since those lines carry
        // no Q of their own.
        let bands = parse_bands(SAMPLE_FIXED_BAND).expect("filter lines are bands");
        assert_eq!(bands.len(), 10);
        assert!((bands[0].hz - 31.0).abs() < 0.05);
        assert!((bands[0].gain_db - 6.9).abs() < 0.005);
        assert!((bands[0].q - 1.41).abs() < 0.005);
    }

    /// A filter someone switched off isn't a band, and a line missing its
    /// gain isn't either.
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
