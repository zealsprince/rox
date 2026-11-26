//! Keeps `examples/renderers/standin.py` honest. The stand-in is what an
//! outside renderer author tests against, so a layout it gets wrong is a
//! renderer that works there and fails in rox. Its constants are read out of
//! the file and held to this crate's, and a run against the bars example has
//! to come back with frames and nothing refused.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

use rox_frames::{cover, header, slot, spectrum};

fn examples() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/renderers")
}

/// Every `NAME = value` line at the top level of the stand-in, the value as
/// written with any comment dropped.
fn constants() -> HashMap<String, String> {
    let source = std::fs::read_to_string(examples().join("standin.py")).expect("standin.py");

    source
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(" = ")?;
            let plain = !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
            let value = value.split("  #").next()?.trim();
            plain.then(|| (name.to_string(), value.to_string()))
        })
        .collect()
}

#[test]
fn the_stand_ins_layout_is_the_crates() {
    let found = constants();
    let int = |name: &str| -> u64 {
        found
            .get(name)
            .unwrap_or_else(|| panic!("standin.py has no {name}"))
            .parse()
            .unwrap_or_else(|_| panic!("{name} isn't a whole number"))
    };
    let float = |name: &str| -> f32 {
        found
            .get(name)
            .unwrap_or_else(|| panic!("standin.py has no {name}"))
            .parse()
            .unwrap_or_else(|_| panic!("{name} isn't a number"))
    };
    let magic = |name: &str| -> Vec<u8> {
        let text = found
            .get(name)
            .unwrap_or_else(|| panic!("standin.py has no {name}"));
        text.strip_prefix("b\"")
            .and_then(|t| t.strip_suffix('"'))
            .unwrap_or_else(|| panic!("{name} isn't a bytes literal"))
            .as_bytes()
            .to_vec()
    };

    assert_eq!(magic("FRAME_MAGIC"), rox_frames::MAGIC);
    let whole = [
        ("FRAME_VERSION", u64::from(rox_frames::VERSION)),
        ("HEADER_BYTES", rox_frames::HEADER_BYTES as u64),
        ("SLOTS", u64::from(rox_frames::SLOTS)),
        ("PAGE", rox_frames::PAGE),
        ("PIXEL", u64::from(rox_frames::PIXEL)),
        ("WIDTH_MIN", u64::from(*rox_frames::WIDTH.start())),
        ("WIDTH_MAX", u64::from(*rox_frames::WIDTH.end())),
        ("HEIGHT_MIN", u64::from(*rox_frames::HEIGHT.start())),
        ("HEIGHT_MAX", u64::from(*rox_frames::HEIGHT.end())),
        ("H_MAGIC", header::MAGIC as u64),
        ("H_VERSION", header::VERSION as u64),
        ("H_WIDTH", header::WIDTH as u64),
        ("H_HEIGHT", header::HEIGHT as u64),
        ("H_STRIDE", header::STRIDE as u64),
        ("H_SLOTS", header::SLOTS as u64),
        ("H_SLOT_BYTES", header::SLOT_BYTES as u64),
        ("S_SEQ", slot::SEQ as u64),
        ("S_STAMP_NS", slot::STAMP_NS as u64),
        ("S_WIDTH", slot::WIDTH as u64),
        ("S_HEIGHT", slot::HEIGHT as u64),
        ("S_PIXELS", slot::PIXELS as u64),
        ("ANNOUNCE_MAX", rox_frames::ANNOUNCE_MAX as u64),
        ("COVER_VERSION", u64::from(cover::VERSION)),
        ("COVER_SIZE", u64::from(cover::SIZE)),
        ("COVER_BYTES", cover::BYTES as u64),
        ("COVER_WIDTH", cover::header::WIDTH as u64),
        ("COVER_HEIGHT", cover::header::HEIGHT as u64),
        ("COVER_SEQ", cover::header::SEQ as u64),
        ("COVER_PIXELS", cover::header::PIXELS as u64),
        ("SPECTRUM_VERSION", u64::from(spectrum::VERSION)),
        ("SPECTRUM_BYTES", spectrum::BYTES as u64),
        ("LOG_BINS", spectrum::LOG_BINS as u64),
        ("LINEAR_BINS", spectrum::LINEAR_BINS as u64),
        ("PLAYING", u64::from(spectrum::PLAYING)),
        ("SP_SEQ", spectrum::header::SEQ as u64),
        ("SP_STAMP_NS", spectrum::header::STAMP_NS as u64),
        ("SP_FLAGS", spectrum::header::FLAGS as u64),
        ("SP_SAMPLE_RATE", spectrum::header::SAMPLE_RATE as u64),
        ("SP_RATE", spectrum::header::RATE as u64),
        ("SP_LOG_BINS", spectrum::header::LOG_BINS as u64),
        ("SP_LOG_LOW_HZ", spectrum::header::LOG_LOW_HZ as u64),
        ("SP_LOG_HIGH_HZ", spectrum::header::LOG_HIGH_HZ as u64),
        ("SP_LINEAR_BINS", spectrum::header::LINEAR_BINS as u64),
        ("SP_LINEAR_BIN_HZ", spectrum::header::LINEAR_BIN_HZ as u64),
        ("SP_FLOOR_DB", spectrum::header::FLOOR_DB as u64),
        ("SP_MAX_DB", spectrum::header::MAX_DB as u64),
        ("SP_LOG_AT", spectrum::header::LOG_AT as u64),
        ("SP_LINEAR_AT", spectrum::header::LINEAR_AT as u64),
        ("SP_LOG_BYTES_AT", spectrum::header::LOG_BYTES_AT as u64),
        (
            "SP_LINEAR_BYTES_AT",
            spectrum::header::LINEAR_BYTES_AT as u64,
        ),
        ("ROW_LOG", spectrum::rows::LOG as u64),
        ("ROW_LINEAR", spectrum::rows::LINEAR as u64),
        ("ROW_LOG_BYTES", spectrum::rows::LOG_BYTES as u64),
        ("ROW_LINEAR_BYTES", spectrum::rows::LINEAR_BYTES as u64),
    ];
    for (name, value) in whole {
        assert_eq!(int(name), value, "{name}");
    }

    assert_eq!(magic("COVER_MAGIC"), cover::MAGIC);
    assert_eq!(magic("SPECTRUM_MAGIC"), spectrum::MAGIC);
    let numbers = [
        ("LOG_LOW_HZ", spectrum::LOG_LOW_HZ),
        ("LOG_HIGH_HZ", spectrum::LOG_HIGH_HZ),
        ("LINEAR_BIN_HZ", spectrum::LINEAR_BIN_HZ),
        ("FLOOR_DB", spectrum::FLOOR_DB),
        ("MAX_DB", spectrum::MAX_DB),
        ("SPECTRUM_RATE", spectrum::RATE),
    ];
    for (name, value) in numbers {
        assert_eq!(float(name), value, "{name}");
    }
}

fn python3() -> Option<&'static str> {
    Command::new("python3")
        .arg("--version")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|_| "python3")
}

#[cfg(unix)]
#[test]
fn the_bars_example_runs_against_the_stand_in() {
    let Some(python) = python3() else {
        eprintln!("skipped: no python3 on PATH to run the stand-in and the bars example");
        return;
    };

    let out = Command::new(python)
        .arg(examples().join("standin.py"))
        .args([
            "--seconds",
            "1",
            "--size",
            "160x90",
            "--spectrum",
            "--palette",
        ])
        .arg("--cover")
        .arg(examples().join("bars"))
        .output()
        .expect("the stand-in starts");

    // The report is one `name: value` line each.
    let stdout = String::from_utf8_lossy(&out.stdout);
    let report: HashMap<&str, &str> = stdout
        .lines()
        .filter_map(|line| line.split_once(": "))
        .collect();
    let said = |name: &str| {
        *report.get(name).unwrap_or_else(|| {
            panic!(
                "no {name} in the report: {stdout}\n{}",
                String::from_utf8_lossy(&out.stderr)
            )
        })
    };

    let frames: u64 = said("frames").parse().expect("a count");
    assert!(frames > 0, "{stdout}");
    assert_eq!(said("refused"), "0", "{stdout}");
    assert_eq!(said("torn"), "0", "{stdout}");
    assert_eq!(said("protocol"), "[]", "{stdout}");
    assert_eq!(said("flooded"), "False", "{stdout}");
    assert_eq!(said("exited within grace"), "True", "{stdout}");
    assert!(out.status.success(), "{stdout}");
}
