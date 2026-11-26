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

/// A renderer that draws nothing and watches the spectrum and the cover the
/// way the guide reads them, until stdin closes. What it sees goes to the
/// file named in its argument, a line for each change: `spectrum <seq>
/// <flags> <nonzero>`, `square <seq> <nonzero>` for the cover mapping, and
/// `cover <counter or null>` for each cover line.
#[cfg(unix)]
const WATCHER: &str = r#"
import json, mmap, os, select, struct, sys

spectrum = mmap.mmap(int(os.environ["ROX_SPECTRUM_FD"]), 0, prot=mmap.PROT_READ)
cover = mmap.mmap(int(os.environ["ROX_COVER_FD"]), 0, prot=mmap.PROT_READ)
seen, last = [], {}

def whole(view, at, start, end):
    while True:
        (seq,) = struct.unpack_from("<Q", view, at)
        if seq & 1:
            continue
        copy = bytes(view[start:end])
        if struct.unpack_from("<Q", view, at)[0] == seq:
            return seq, copy

def look():
    seq, page = whole(spectrum, 8, 16, 3328)
    (flags,) = struct.unpack_from("<I", page, 8)
    note("spectrum", f"spectrum {seq} {flags} {int(any(page[112:]))}")

    seq, pixels = whole(cover, 16, 64, 64 + 256 * 256 * 4)
    note("square", f"square {seq} {int(any(pixels))}")

def note(what, now):
    if last.get(what) != now:
        seen.append(now)
        last[what] = now

stdin = sys.stdin.buffer
while True:
    look()
    if select.select([stdin], [], [], 0.005)[0]:
        line = stdin.readline()
        if not line:
            break
        message = json.loads(line)
        if "cover" in message and "hello" not in message:
            seen.append(f"cover {json.dumps(message['cover'])}")

with open(sys.argv[1], "w") as out:
    out.write("\n".join(seen) + "\n")
"#;

/// The stand-in writes the spectrum as rox does: zeros once when it's made,
/// rows with the playing bit while its drive plays, zeros once when it
/// stops, and nothing between. Its cover mapping follows the track the same
/// way, the transparent square named `null`.
#[cfg(unix)]
#[test]
fn the_stand_in_writes_the_spectrum_and_the_cover_as_rox_does() {
    let Some(python) = python3() else {
        eprintln!("skipped: no python3 on PATH to run the stand-in");
        return;
    };

    let seen_at = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("standin-watcher.txt");
    let _ = std::fs::remove_file(&seen_at);

    // A rest to 0.48 s, music to 3.6 s, a pause from 2.4 s to 2.9 s.
    let out = Command::new(python)
        .arg(examples().join("standin.py"))
        .args([
            "--seconds",
            "4",
            "--size",
            "16x16",
            "--spectrum",
            "--cover",
            "--",
        ])
        .args([python, "-c", WATCHER])
        .arg(&seen_at)
        .output()
        .expect("the stand-in starts");
    let seen = std::fs::read_to_string(&seen_at).unwrap_or_else(|_| {
        panic!(
            "the watcher wrote nothing: {}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    let lines: Vec<Vec<&str>> = seen.lines().map(|line| line.split(' ').collect()).collect();

    // (seq, playing, nonzero) for each spectrum the watcher saw change.
    let spectrum: Vec<(u64, bool, bool)> = lines
        .iter()
        .filter(|words| words[0] == "spectrum")
        .map(|w| (w[1].parse().unwrap(), w[2] == "1", w[3] == "1"))
        .collect();

    // The rest: zeros written once, the counter stopped.
    assert_eq!(spectrum[0], (2, false, false), "{seen}");

    // The music: the counter moves with the playing bit set.
    let played: Vec<u64> = spectrum
        .iter()
        .filter(|&&(_, playing, _)| playing)
        .map(|&(seq, ..)| seq)
        .collect();
    assert!(played.len() > 50, "{seen}");
    assert!(played.windows(2).all(|w| w[1] > w[0]), "{seen}");
    assert!(
        spectrum
            .iter()
            .all(|&(_, playing, nonzero)| playing == nonzero),
        "rows only while it plays: {seen}"
    );

    // The stop: zeros once, the counter stopped again.
    let after = spectrum
        .iter()
        .rposition(|&(_, playing, _)| playing)
        .unwrap();
    let rest = &spectrum[after + 1..];
    assert_eq!(rest.len(), 1, "{seen}");
    assert_eq!(rest[0].0, played.last().unwrap() + 2, "{seen}");

    // The cover: transparent and null before the music, the picture while
    // it plays, then transparent and null again.
    let said = |what: &str| -> Vec<String> {
        lines
            .iter()
            .filter(|words| words[0] == what)
            .map(|words| words[1..].join(" "))
            .collect()
    };
    assert_eq!(said("cover"), ["null", "4", "null"], "{seen}");
    assert_eq!(said("square"), ["2 0", "4 1", "6 0"], "{seen}");
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

/// The stand-in's report, one `name: value` line each.
#[cfg(unix)]
fn report(out: &std::process::Output) -> HashMap<String, String> {
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| line.split_once(": "))
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

/// A renderer written before sizes: it draws at the mapping's size from the
/// environment and reads nothing rox sends. It must go on working whatever
/// a target does.
#[cfg(unix)]
const FIXED: &str = r#"
import mmap, os, select, struct, sys, time

w, h = int(os.environ["ROX_FRAMES_WIDTH"]), int(os.environ["ROX_FRAMES_HEIGHT"])
mem = mmap.mmap(int(os.environ["ROX_FRAMES_FD"]), 0)
(slot_bytes,) = struct.unpack_from("<Q", mem, 24)
n = 0
while True:
    if select.select([0], [], [], 1 / 30)[0] and not os.read(0, 65536):
        break
    base = 4096 + (n % 2) * slot_bytes
    struct.pack_into("<Q", mem, base, 2 * n + 1)
    mem[base + 64 : base + 64 + w * h * 4] = bytes((n % 256, 0, 0, 255)) * (w * h)
    struct.pack_into("<QII", mem, base + 8, time.clock_gettime_ns(time.CLOCK_MONOTONIC), w, h)
    struct.pack_into("<Q", mem, base, 2 * n + 2)
    sys.stdout.write(f"f {n} {n % 2}\n")
    sys.stdout.flush()
    n += 1
"#;

/// Bars reads sizes: told three across the run, it draws at each, and
/// nothing it draws is refused. A renderer that doesn't is sent no size
/// line and draws at its one size throughout.
#[cfg(unix)]
#[test]
fn a_renderer_that_resizes_follows_and_one_that_doesnt_is_left_alone() {
    let Some(python) = python3() else {
        eprintln!("skipped: no python3 on PATH to run the stand-in");
        return;
    };

    let out = Command::new(python)
        .arg(examples().join("standin.py"))
        .args([
            "--seconds",
            "2",
            "--size",
            "96x54",
            "--frames",
            "320x180",
            "--resize",
            "160x90,320x180,64x200",
        ])
        .arg(examples().join("bars"))
        .output()
        .expect("the stand-in starts");
    let said = report(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert_eq!(said["size lines"], "3", "{said:?} {stderr}");
    assert_eq!(said["refused"], "0", "{said:?} {stderr}");
    assert_eq!(said["torn"], "0", "{said:?}");
    assert_eq!(
        said["sizes"], "['160x90', '320x180', '57x180', '96x54']",
        "each size it was told, the last held to the mapping with its shape"
    );
    assert!(out.status.success(), "{said:?} {stderr}");

    let out = Command::new(python)
        .arg(examples().join("standin.py"))
        .args([
            "--seconds",
            "1",
            "--size",
            "48x32",
            "--resize",
            "96x64",
            "--",
        ])
        .args([python, "-c", FIXED])
        .output()
        .expect("the stand-in starts");
    let said = report(&out);

    assert_eq!(said["size lines"], "0", "{said:?}");
    assert_eq!(said["refused"], "0", "{said:?}");
    assert_eq!(said["sizes"], "['48x32']", "{said:?}");
    assert!(out.status.success(), "{said:?}");
}
