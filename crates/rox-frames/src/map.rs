//! Making the mapping. rox creates it, writes the header through a view
//! that's gone before [`Mapping::create`] returns, and keeps a read-only
//! view for the life of the session. There is deliberately no constructor
//! from an existing handle: a renderer that made the file could shrink it
//! under rox's reads.

#[cfg(unix)]
pub(crate) mod unix;
#[cfg(windows)]
pub(crate) mod windows;

#[cfg(unix)]
pub(crate) use unix as os;
#[cfg(windows)]
pub(crate) use windows as os;

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
compile_error!("rox-frames makes its mapping on Linux, macOS and Windows only");

use crate::{Error, Spec};

/// What the renderer is handed: the file descriptor on Unix, the section
/// handle on Windows.
#[cfg(unix)]
pub type Handle<'a> = std::os::fd::BorrowedFd<'a>;
/// What the renderer is handed: the file descriptor on Unix, the section
/// handle on Windows.
#[cfg(windows)]
pub type Handle<'a> = std::os::windows::io::BorrowedHandle<'a>;

/// The rights the renderer's copy of the section gets on Windows: map read
/// and map write, so no right to change the section's security or query
/// it.
#[cfg(windows)]
pub const CHILD_ACCESS: u32 = os::CHILD_ACCESS;

/// One session's frame mapping, as rox holds it: the handle the renderer
/// gets, and a read-only view of the whole block.
pub struct Mapping {
    // Field order is drop order: the view unmaps before the handle closes.
    view: os::View,
    section: os::Section,
    spec: Spec,
}

// SAFETY: the view is only read, through raw pointers, and the handle is a
// kernel object reference usable from any thread.
unsafe impl Send for Mapping {}

impl Mapping {
    /// Makes a mapping sized for `spec`, header written and, on Linux,
    /// sealed against resizing.
    pub fn create(spec: Spec) -> Result<Mapping, Error> {
        let (section, view) = os::create(&spec)?;

        Ok(Mapping {
            view,
            section,
            spec,
        })
    }

    /// The handle to pass to the renderer. On Unix it's close-on-exec, so
    /// the spawn has to put it in place for the child on purpose. On
    /// Windows it's not inheritable, and the spawn duplicates it into the
    /// child with [`CHILD_ACCESS`] and nothing more.
    pub fn child_handle(&self) -> Handle<'_> {
        self.section.borrow()
    }

    /// The size this mapping was made for.
    pub fn spec(&self) -> &Spec {
        &self.spec
    }

    /// The start of rox's read-only view, `spec().bytes()` long.
    pub(crate) fn base(&self) -> *const u8 {
        self.view.base()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "linux")]
    fn the_mapping_cant_be_shrunk() {
        use std::os::fd::AsRawFd;

        let spec = Spec::new(16, 16).unwrap();
        let mapping = Mapping::create(spec).unwrap();
        let fd = mapping.child_handle().as_raw_fd();

        // SAFETY: ftruncate and fcntl on an fd the mapping holds open.
        let (shrink, grow, more, seals) = unsafe {
            (
                libc::ftruncate(fd, 0),
                libc::ftruncate(fd, 2 * spec.bytes() as libc::off_t),
                libc::fcntl(fd, libc::F_ADD_SEALS, libc::F_SEAL_WRITE),
                libc::fcntl(fd, libc::F_GET_SEALS),
            )
        };

        assert_eq!(shrink, -1);
        assert_eq!(grow, -1);
        assert_eq!(more, -1, "F_SEAL_SEAL lets no seal be added or taken off");

        let want = libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_SEAL;
        assert_eq!(seals & want, want);
    }

    #[test]
    #[cfg(any(target_os = "linux", windows))]
    fn the_mapping_cant_be_written_through_rox_s_view() {
        let spec = Spec::new(64, 32).unwrap();
        let mapping = Mapping::create(spec).unwrap();

        assert!(os::read_only(mapping.base(), spec.bytes()));

        // The header went in through a writable view. Nothing of it is left.
        #[cfg(target_os = "linux")]
        {
            assert_eq!(os::writable_views(mapping.child_handle()), 0);

            // And the count does see one when there is one.
            // SAFETY: the only writer for this mapping.
            let writer = unsafe { crate::Writer::open(mapping.child_handle(), spec) }.unwrap();
            assert_eq!(os::writable_views(mapping.child_handle()), 1);
            drop(writer);
        }
    }
}
