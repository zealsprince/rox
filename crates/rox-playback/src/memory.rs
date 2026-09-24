//! How much memory the machine has, and how much a live buffer may take.
//!
//! The live buffer is set in seconds but paid for at the station's unknown
//! bitrate: twelve hours is 691 MB at 128 kbps, 1.7 GB at 320. So it also
//! gets a ceiling in bytes, under which the window comes up short instead.

use std::sync::OnceLock;

/// A tenth: 1.6 GB of a 16 GB machine, twelve hours at 320 kbps. The settings
/// row (`settings-playback-live-buffer-cap`) says "a tenth" in every locale,
/// so change that copy with it.
pub const LIVE_BUFFER_SHARE: f64 = 0.10;

/// Floor under the share: thirty seconds, the slider's minimum, at the tape's
/// 1 MB/s rate ceiling.
const MIN_CAP: u64 = 32 * 1024 * 1024;

/// Used when the machine won't report its memory: a tenth of 8 GB.
const CAP_UNKNOWN: u64 = 800 * 1024 * 1024;

/// Physical memory, or None if the machine won't say. Read once; the tape
/// asks on every trim.
pub fn total() -> Option<u64> {
    static TOTAL: OnceLock<Option<u64>> = OnceLock::new();

    *TOTAL.get_or_init(|| probe().filter(|bytes| *bytes > 0))
}

pub fn live_buffer_cap() -> usize {
    static CAP: OnceLock<u64> = OnceLock::new();

    let cap = *CAP.get_or_init(|| match total() {
        Some(total) => share_of(total),
        None => CAP_UNKNOWN,
    });

    usize::try_from(cap).unwrap_or(usize::MAX)
}

fn share_of(total: u64) -> u64 {
    ((total as f64 * LIVE_BUFFER_SHARE) as u64).max(MIN_CAP)
}

#[cfg(target_os = "linux")]
fn probe() -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;

    meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))
        .and_then(mem_total)
}

/// The `MemTotal:` value, which the kernel writes in kB. Any other unit reads
/// as None rather than a guess that could be a thousandfold off.
#[cfg(target_os = "linux")]
fn mem_total(rest: &str) -> Option<u64> {
    let mut parts = rest.split_whitespace();
    let value: u64 = parts.next()?.parse().ok()?;

    match parts.next() {
        Some("kB") => value.checked_mul(1024),
        _ => None,
    }
}

#[cfg(target_os = "macos")]
fn probe() -> Option<u64> {
    let mut bytes: u64 = 0;
    let mut len = std::mem::size_of::<u64>();

    // SAFETY: hw.memsize answers one u64, and the length in and out is the
    // size of the one being written to. A non-zero return means nothing was
    // written, which is the None below.
    let answered = unsafe {
        libc::sysctlbyname(
            c"hw.memsize".as_ptr(),
            (&raw mut bytes).cast(),
            &raw mut len,
            std::ptr::null_mut(),
            0,
        )
    };

    (answered == 0).then_some(bytes)
}

#[cfg(target_os = "windows")]
fn probe() -> Option<u64> {
    use windows::Win32::System::SystemInformation::GlobalMemoryStatusEx;
    use windows::Win32::System::SystemInformation::MEMORYSTATUSEX;

    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };

    // SAFETY: the one out parameter is this stack struct, and its dwLength
    // above is how the call knows how much of it there is to fill.
    unsafe { GlobalMemoryStatusEx(&mut status) }.ok()?;

    Some(status.ullTotalPhys)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn probe() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ceiling_is_a_tenth_of_the_machine() {
        assert_eq!(share_of(16 * 1024 * 1024 * 1024), 1_717_986_918);
        assert_eq!(share_of(8 * 1024 * 1024 * 1024), 858_993_459);
    }

    #[test]
    fn a_small_machine_still_gets_the_floor() {
        assert_eq!(share_of(128 * 1024 * 1024), MIN_CAP);
        assert_eq!(share_of(0), MIN_CAP);
    }

    #[test]
    fn this_machine_answers_with_a_usable_ceiling() {
        assert!(live_buffer_cap() as u64 >= MIN_CAP);
        if let Some(total) = total() {
            assert!(live_buffer_cap() as u64 <= total);
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn meminfo_reads_in_kilobytes() {
        assert_eq!(mem_total("       16337080 kB"), Some(16_729_169_920));
        assert_eq!(mem_total(" 16337080"), None, "no unit, no answer");
        assert_eq!(mem_total(" 16337080 MB"), None, "a unit we don't know");
        assert_eq!(mem_total(" plenty kB"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_machine_says_how_much_memory_it_has() {
        assert!(total().is_some_and(|bytes| bytes > 0), "/proc/meminfo read");
    }
}
