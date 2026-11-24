//! The shared memory a render target's frames arrive through (ADR 37 and
//! its 2026-11-23 amendment). rox makes one mapping per renderer session,
//! seals it where the platform can, keeps only a read-only view of it and
//! hands the renderer a handle. The renderer writes frames into one of two
//! slots and announces each with a short line on its stdout. rox copies a
//! slot out and checks its counter didn't move under the copy.
//!
//! This crate is the trust boundary in that exchange. Under ADR 35 a frame
//! is a stranger's bytes: every length and offset here comes from the
//! [`Spec`] rox chose, nothing read from the mapping is used as one, and no
//! lock is shared with the renderer. There's no way to map a file the
//! renderer made, and nothing is mapped executable. Spawning the renderer,
//! its wire and its pacing live with the renderer host, not here.

mod layout;
mod map;
mod read;
#[cfg(any(test, feature = "writer"))]
mod write;

use std::fmt;
use std::io;

pub use layout::{HEADER_BYTES, HEIGHT, MAGIC, PAGE, PIXEL, SLOTS, Spec, VERSION, WIDTH};
pub use layout::{header, slot};
#[cfg(windows)]
pub use map::CHILD_ACCESS;
pub use map::{Handle, Mapping};
pub use read::{Read, Refusal};
#[cfg(any(test, feature = "writer"))]
pub use write::Writer;

/// The longest announce line, newline included.
pub const ANNOUNCE_MAX: usize = 64;

/// What can go wrong making or opening a mapping.
#[derive(Debug)]
pub enum Error {
    /// The spec's sizes don't fit the arithmetic, or the address space.
    Overflow,
    /// An OS call failed.
    Os {
        call: &'static str,
        error: io::Error,
    },
    /// The mapping's size or header isn't the one the spec describes.
    Mismatch,
}

impl Error {
    pub(crate) fn os(call: &'static str) -> Error {
        Error::Os {
            call,
            error: io::Error::last_os_error(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Overflow => f.write_str("frame mapping size overflows"),
            Error::Os { call, error } => write!(f, "{call}: {error}"),
            Error::Mismatch => f.write_str("frame mapping isn't the size it was said to be"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Os { error, .. } => Some(error),
            _ => None,
        }
    }
}

/// Reads a renderer's `f <frame> <slot>` line, with or without its newline.
/// None for anything longer than [`ANNOUNCE_MAX`], a slot past the last, or
/// any other shape.
pub fn parse_announce(line: &[u8]) -> Option<(u64, u32)> {
    if line.len() > ANNOUNCE_MAX {
        return None;
    }

    let line = line.strip_suffix(b"\n").unwrap_or(line);
    let rest = line.strip_prefix(b"f ")?;
    let space = rest.iter().position(|&b| b == b' ')?;
    let (frame, slot) = (&rest[..space], &rest[space + 1..]);

    // `u64::from_str` takes a leading `+`, which isn't part of the line.
    if frame.is_empty() || !frame.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let frame = std::str::from_utf8(frame).ok()?.parse::<u64>().ok()?;

    let slot = match slot {
        b"0" => 0,
        b"1" => 1,
        _ => return None,
    };

    Some((frame, slot))
}

/// The line a renderer writes to announce frame `n` in `slot`, newline
/// included.
pub fn announce(n: u64, slot: u32) -> String {
    format!("f {n} {slot}\n")
}

/// The clock a slot's `stamp_ns` is read against: `CLOCK_MONOTONIC` on
/// Unix, the system clock since the Unix epoch on Windows.
pub fn clock_ns() -> u64 {
    #[cfg(unix)]
    {
        let mut now = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: clock_gettime writes one timespec we own.
        if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) } != 0 {
            return 0;
        }

        let secs = u64::try_from(now.tv_sec).unwrap_or(0);
        let nanos = u64::try_from(now.tv_nsec).unwrap_or(0);
        secs.saturating_mul(1_000_000_000).saturating_add(nanos)
    }

    #[cfg(windows)]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_announce_line_parses_and_a_long_one_doesnt() {
        assert_eq!(parse_announce(b"f 12 0\n"), Some((12, 0)));
        assert_eq!(parse_announce(b"f 13 1"), Some((13, 1)));
        assert_eq!(
            parse_announce(announce(u64::MAX, 1).as_bytes()),
            Some((u64::MAX, 1))
        );

        let long = format!("f {}1 0\n", "0".repeat(ANNOUNCE_MAX));
        assert_eq!(parse_announce(long.as_bytes()), None);

        for bad in [
            &b"f 12 2\n"[..],
            b"f 12 01",
            b"f +12 0",
            b"f -1 0",
            b"f 12  0",
            b"f  12 0",
            b"f 18446744073709551616 0",
            b"f 12 0\r\n",
            b"g 12 0",
            b"f 12",
            b"",
        ] {
            assert_eq!(
                parse_announce(bad),
                None,
                "{:?}",
                String::from_utf8_lossy(bad)
            );
        }
    }
}
