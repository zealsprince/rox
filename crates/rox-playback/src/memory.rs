//! How much memory the machine has, and how much of it a live buffer is
//! allowed to take.
//!
//! The live buffer is the one setting in the app that can spend memory by
//! the gigabyte, and it's set in seconds. What a second costs is the
//! station's bitrate, which nobody knows when they move the slider: twelve
//! hours is 691 MB of a 128 kbps stream, 1.7 GB at 320, and several times
//! that on a station broadcasting lossless. So the length can't be the whole
//! rule. This is the other half of it, a ceiling in bytes taken off what the
//! machine actually has, under which the window comes up short of the length
//! it was asked for rather than taking the memory.

use std::sync::OnceLock;

/// The share of the machine's memory a live buffer may reach. A tenth is
/// enough that the setting's whole range is usable on an ordinary machine
/// (a tenth of 16 GB is 1.6 GB, which is twelve hours at 320 kbps) and
/// small enough that the app is never the reason something else had to be
/// swapped out.
///
/// The settings row names this as a tenth in so many words
/// (`settings-playback-live-buffer-cap`), so a change here is a change to
/// that copy in every locale.
pub const LIVE_BUFFER_SHARE: f64 = 0.10;

/// The floor under that share, so a small machine still gets a buffer worth
/// having. The shortest window the setting offers is thirty seconds and the
/// fattest rate the tape sizes against is a megabyte a second, so a ceiling
/// under this could fail to hold even what the slider's own minimum asks
/// for.
const MIN_CAP: u64 = 32 * 1024 * 1024;

/// What the ceiling falls back to when the machine won't say how much
/// memory it has. Every platform we ship on answers, so this is for the one
/// that somehow doesn't: a tenth of 8 GB, which is a modest laptop's worth
/// and errs towards spending too little rather than too much.
const CAP_UNKNOWN: u64 = 800 * 1024 * 1024;

/// How much physical memory the machine has, or None if it wouldn't say.
///
/// Read once and held: it's a constant for the life of the process, and the
/// tape asks for it on every trim. None is also what the settings row reads
/// to know it has nothing honest to say about this machine's memory.
pub fn total() -> Option<u64> {
    static TOTAL: OnceLock<Option<u64>> = OnceLock::new();

    *TOTAL.get_or_init(|| probe().filter(|bytes| *bytes > 0))
}

/// The most a live buffer may hold, in bytes, whatever length it's set to.
pub fn live_buffer_cap() -> usize {
    static CAP: OnceLock<u64> = OnceLock::new();

    let cap = *CAP.get_or_init(|| match total() {
        Some(total) => share_of(total),
        None => CAP_UNKNOWN,
    });

    usize::try_from(cap).unwrap_or(usize::MAX)
}

/// The share as bytes, held to [`MIN_CAP`] and up. Its own function so the
/// test can put machine sizes through it that this machine isn't.
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

/// What follows `MemTotal:` in /proc/meminfo: a count and its unit, which
/// the kernel writes as kB whatever the machine. Anything else is a format
/// we don't know, and a wrong guess at the unit is a thousandfold wrong
/// ceiling, so it reads as no answer instead.
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

    /// A machine small enough that a tenth of it wouldn't hold the shortest
    /// window the setting offers gets the floor instead.
    #[test]
    fn a_small_machine_still_gets_the_floor() {
        assert_eq!(share_of(128 * 1024 * 1024), MIN_CAP);
        assert_eq!(share_of(0), MIN_CAP);
    }

    /// Whatever this machine is, the ceiling is a real number and the
    /// shortest window the setting offers fits under it.
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

    /// The probe itself, on the platform the tests run on.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_machine_says_how_much_memory_it_has() {
        assert!(total().is_some_and(|bytes| bytes > 0), "/proc/meminfo read");
    }
}
