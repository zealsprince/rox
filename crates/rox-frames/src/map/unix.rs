//! The mapping on Linux and macOS: a memfd sealed against resizing on
//! Linux, an unlinked POSIX shm object on macOS. Either is an fd, mapped
//! `MAP_SHARED` and never `PROT_EXEC`.
//!
//! The macOS half has never been compiled or run.

use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::ptr::NonNull;

use crate::layout::{self, HEADER_BYTES};
use crate::{Error, Spec};

/// The file behind the mapping.
pub(crate) struct Section(OwnedFd);

impl Section {
    pub(crate) fn borrow(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

/// One `mmap` of the file, unmapped on drop.
pub(crate) struct View {
    base: NonNull<u8>,
    len: usize,
}

impl View {
    pub(crate) fn base(&self) -> *const u8 {
        self.base.as_ptr()
    }

    #[cfg(any(test, feature = "writer"))]
    pub(crate) fn base_mut(&mut self) -> *mut u8 {
        self.base.as_ptr()
    }
}

impl Drop for View {
    fn drop(&mut self) {
        // SAFETY: `base` and `len` are exactly what `map` got back from
        // mmap, and nothing borrows the region past the view's life.
        unsafe {
            libc::munmap(self.base.as_ptr().cast(), self.len);
        }
    }
}

fn map(fd: BorrowedFd<'_>, len: usize, prot: libc::c_int) -> Result<View, Error> {
    // SAFETY: a fresh mapping at an address the kernel picks, so it
    // aliases nothing of ours. The result is checked before it's used.
    let base = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            prot,
            libc::MAP_SHARED,
            fd.as_raw_fd(),
            0,
        )
    };
    if base == libc::MAP_FAILED {
        return Err(Error::os("mmap"));
    }

    let base = NonNull::new(base.cast::<u8>()).ok_or(Error::Mismatch)?;
    Ok(View { base, len })
}

pub(crate) fn create(spec: &Spec) -> Result<(Section, View), Error> {
    let size = libc::off_t::try_from(spec.bytes()).map_err(|_| Error::Overflow)?;
    let fd = open()?;

    // SAFETY: ftruncate on a file this function just made.
    if unsafe { libc::ftruncate(fd.as_raw_fd(), size) } != 0 {
        return Err(Error::os("ftruncate"));
    }

    // The writable view unmaps at the end of this block, before the seals
    // and before the fd can reach anyone.
    {
        let header = map(fd.as_fd(), HEADER_BYTES, libc::PROT_READ | libc::PROT_WRITE)?;
        // SAFETY: the view is HEADER_BYTES long, page-aligned by mmap and
        // reachable by nobody else yet.
        unsafe { layout::write_header(header.base.as_ptr(), spec) };
    }

    seal(fd.as_fd())?;

    let view = map(fd.as_fd(), spec.bytes(), libc::PROT_READ)?;
    Ok((Section(fd), view))
}

/// A fresh anonymous memfd, close-on-exec and open to sealing.
#[cfg(target_os = "linux")]
fn open() -> Result<OwnedFd, Error> {
    // SAFETY: memfd_create with a static name; the fd is checked and owned
    // at once.
    let fd = unsafe {
        libc::memfd_create(
            c"rox-frames".as_ptr(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        )
    };
    if fd < 0 {
        return Err(Error::os("memfd_create"));
    }

    // SAFETY: a valid fd nothing else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Without the shrink seal a renderer could truncate the file and turn
/// rox's next read into a SIGBUS. Grow and seal keep the size and the
/// seals where rox put them. The write seal can't go on: the renderer
/// writes.
#[cfg(target_os = "linux")]
fn seal(fd: BorrowedFd<'_>) -> Result<(), Error> {
    let seals = libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_SEAL;

    // SAFETY: fcntl on an fd we own.
    if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_ADD_SEALS, seals) } != 0 {
        return Err(Error::os("F_ADD_SEALS"));
    }

    Ok(())
}

/// A POSIX shm object under a random name, unlinked as soon as it's open,
/// so nothing else can open it by name.
#[cfg(target_os = "macos")]
fn open() -> Result<OwnedFd, Error> {
    for _ in 0..8 {
        let mut random = [0u8; 8];
        // SAFETY: fills the eight bytes we own.
        unsafe { libc::arc4random_buf(random.as_mut_ptr().cast(), random.len()) };

        // 28 bytes, inside macOS's 31 byte shm name limit.
        let name = format!("/rox-frames-{:016x}", u64::from_ne_bytes(random));
        let name = std::ffi::CString::new(name).map_err(|_| Error::Mismatch)?;

        // SAFETY: shm_open with a NUL-terminated name; the mode goes
        // through the variadic as a C int.
        let fd = unsafe {
            libc::shm_open(
                name.as_ptr(),
                libc::O_CREAT | libc::O_EXCL | libc::O_RDWR,
                0o600 as libc::c_uint,
            )
        };
        if fd < 0 {
            if std::io::Error::last_os_error().raw_os_error() == Some(libc::EEXIST) {
                continue;
            }
            return Err(Error::os("shm_open"));
        }

        // SAFETY: a valid fd nothing else owns, unlinked by the name we
        // just made. POSIX says shm_open sets FD_CLOEXEC; it's set again
        // here rather than trusted, since a leaked fd is a renderer's.
        let (fd, cloexec) = unsafe {
            let fd = OwnedFd::from_raw_fd(fd);
            libc::shm_unlink(name.as_ptr());
            let cloexec = libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC);
            (fd, cloexec)
        };
        if cloexec != 0 {
            return Err(Error::os("F_SETFD"));
        }

        return Ok(fd);
    }

    Err(Error::os("shm_open"))
}

/// macOS has no seals. As far as Apple's documentation goes, a POSIX shm
/// object takes one `ftruncate` and refuses the rest, which would close
/// the SIGBUS a shrink causes on Linux. That's untested: owed a check on a
/// Mac that a second `ftruncate` on the renderer's fd fails.
#[cfg(target_os = "macos")]
fn seal(_fd: BorrowedFd<'_>) -> Result<(), Error> {
    Ok(())
}

/// Maps a renderer's handle read-write, after checking the file is exactly
/// the size the spec says.
///
/// # Safety
///
/// No other writer maps this file for the view's life.
#[cfg(any(test, feature = "writer"))]
pub(crate) unsafe fn map_writable(fd: BorrowedFd<'_>, spec: &Spec) -> Result<View, Error> {
    // SAFETY: an all-zero stat is a valid value, and fstat fills the one we
    // own.
    let (stat, got) = unsafe {
        let mut stat = std::mem::zeroed::<libc::stat>();
        let got = libc::fstat(fd.as_raw_fd(), &mut stat);
        (stat, got)
    };
    if got != 0 {
        return Err(Error::os("fstat"));
    }
    if usize::try_from(stat.st_size).ok() != Some(spec.bytes()) {
        return Err(Error::Mismatch);
    }

    map(fd, spec.bytes(), libc::PROT_READ | libc::PROT_WRITE)
}

/// How many writable mappings of this fd's file `/proc/self/maps` lists.
#[cfg(all(test, target_os = "linux"))]
pub(crate) fn writable_views(fd: BorrowedFd<'_>) -> usize {
    use std::os::unix::fs::MetadataExt;

    let file = std::fs::File::from(fd.try_clone_to_owned().unwrap());
    let inode = file.metadata().unwrap().ino().to_string();
    let maps = std::fs::read_to_string("/proc/self/maps").unwrap();

    maps.lines()
        .filter(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            fields.len() > 4 && fields[4] == inode && fields[1].contains('w')
        })
        .count()
}

/// Whether `/proc/self/maps` lists `[base, base + len)` as one shared,
/// read-only, non-executable mapping.
#[cfg(all(test, target_os = "linux"))]
pub(crate) fn read_only(base: *const u8, len: usize) -> bool {
    let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
    let start = base as usize;

    maps.lines().any(|line| {
        let mut fields = line.split_whitespace();
        let (Some(range), Some(perms)) = (fields.next(), fields.next()) else {
            return false;
        };
        let Some((from, to)) = range.split_once('-') else {
            return false;
        };

        let from = usize::from_str_radix(from, 16).ok();
        let to = usize::from_str_radix(to, 16).ok();
        from == Some(start) && to.is_some_and(|to| to >= start + len) && perms == "r--s"
    })
}
