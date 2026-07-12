//! Decode diagnostics from the command line. Two modes:
//!
//!   peaks_timing <file>...            time decode_peaks, then a full decode
//!   peaks_timing --probe-scan < paths probe every path on stdin, log failures
//!
//! The first answers "why is the waveform slow" with numbers, the second
//! answers "which files in a library can we not read". Build it the way the
//! app is built; the dev profile is what `cargo run` uses.

use std::io::BufRead;
use std::path::PathBuf;
use std::time::Instant;

/// Probe-only sweep: paths on stdin, one failure line per unprobeable file,
/// then a total. Mirrors the hint construction in engine::Source::open.
fn probe_scan() {
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;

    let stdin = std::io::stdin();
    let mut total = 0u64;
    let mut failed = 0u64;
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let path = PathBuf::from(&line);
        total += 1;
        let file = match std::fs::File::open(&path) {
            Ok(f) => f,
            Err(e) => {
                failed += 1;
                println!("FAIL {}: open: {e}", path.display());
                continue;
            }
        };
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }
        let probe = symphonia::default::get_probe().probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        );
        if let Err(e) = probe {
            failed += 1;
            println!("FAIL {}: probe: {e}", path.display());
        }
        if total % 1000 == 0 {
            eprintln!("... {total} probed, {failed} failed");
        }
    }
    println!("probed {total} files, {failed} failed");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--probe-scan") {
        probe_scan();
        return;
    }
    if args.is_empty() {
        eprintln!("usage: peaks_timing <file>... | peaks_timing --probe-scan < paths");
        std::process::exit(1);
    }

    for arg in &args {
        let path = PathBuf::from(arg);
        print!("{}: ", path.display());

        let start = Instant::now();
        match rox_playback::engine::decode_peaks(&path, 2048) {
            Ok(peaks) => {
                let elapsed = start.elapsed();
                println!("decode_peaks {} bins in {:.2?}", peaks.len(), elapsed);
            }
            Err(e) => {
                println!("decode_peaks failed: {e}");
                continue;
            }
        }

        // Prove the file actually decodes through the playback path too, by
        // counting the frames of a full decode.
        let start = Instant::now();
        match rox_playback::engine::count_frames(&path) {
            Ok((decoded, claimed)) => println!(
                "  full decode: {decoded} frames (container claims {claimed:?}) in {:.2?}",
                start.elapsed()
            ),
            Err(e) => println!("  full decode failed: {e}"),
        }
    }
}
