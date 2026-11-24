//! The mapping on Windows: a page-file-backed section. Its size is fixed
//! when it's made, so it needs no seal. The handle isn't inheritable; the
//! renderer gets a duplicate with map rights only.
//!
//! Written from the Win32 documentation and the windows-sys signatures.
//! No compiler for a Windows target has seen it yet, and it has never run.

use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::ptr::NonNull;

use windows_sys::Win32::Foundation::{DuplicateHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP, FILE_MAP_READ, FILE_MAP_WRITE, MEMORY_MAPPED_VIEW_ADDRESS,
    MapViewOfFile, PAGE_READWRITE, UnmapViewOfFile,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

use crate::layout::{self, HEADER_BYTES};
use crate::{Error, Spec};

/// The section behind the mapping.
pub(crate) struct Section(OwnedHandle);

impl Section {
    pub(crate) fn borrow(&self) -> BorrowedHandle<'_> {
        self.0.as_handle()
    }

    /// Map read and map write, and nothing else: no right to change the
    /// section's security or to query it.
    pub(crate) fn share_with(&self, process: BorrowedHandle<'_>) -> Result<usize, Error> {
        let mut theirs: HANDLE = std::ptr::null_mut();

        // SAFETY: duplicates a handle we own into a process handle the
        // caller borrows for the call. `theirs` is only written.
        let ok = unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                self.0.as_raw_handle().cast(),
                process.as_raw_handle().cast(),
                &mut theirs,
                FILE_MAP_READ | FILE_MAP_WRITE,
                0,
                0,
            )
        };
        if ok == 0 {
            return Err(Error::os("DuplicateHandle"));
        }

        Ok(theirs as usize)
    }
}

/// One view of the section, unmapped on drop.
pub(crate) struct View {
    base: NonNull<u8>,
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
        let view = MEMORY_MAPPED_VIEW_ADDRESS {
            Value: self.base.as_ptr().cast(),
        };

        // SAFETY: the address MapViewOfFile returned, unmapped once, and
        // nothing borrows the view past its life.
        unsafe {
            UnmapViewOfFile(view);
        }
    }
}

fn map(section: BorrowedHandle<'_>, access: FILE_MAP, len: usize) -> Result<View, Error> {
    // SAFETY: a fresh view at an address the system picks, so it aliases
    // nothing of ours. The result is checked before it's used.
    let view = unsafe { MapViewOfFile(section.as_raw_handle().cast(), access, 0, 0, len) };

    let base = NonNull::new(view.Value.cast::<u8>()).ok_or_else(|| Error::os("MapViewOfFile"))?;
    Ok(View { base })
}

pub(crate) fn create(spec: &Spec) -> Result<(Section, View), Error> {
    let size = spec.bytes() as u64;

    // SAFETY: an unnamed section backed by the page file, default security
    // (not inheritable). The handle is checked and owned at once.
    let section = unsafe {
        let handle = CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            std::ptr::null(),
            PAGE_READWRITE,
            (size >> 32) as u32,
            size as u32,
            std::ptr::null(),
        );
        if handle.is_null() {
            return Err(Error::os("CreateFileMappingW"));
        }

        OwnedHandle::from_raw_handle(handle.cast())
    };

    // The writable view unmaps at the end of this block, before the handle
    // can reach anyone.
    {
        let header = map(section.as_handle(), FILE_MAP_WRITE, HEADER_BYTES)?;
        // SAFETY: the view is HEADER_BYTES long, aligned to the allocation
        // granularity and reachable by nobody else yet.
        unsafe { layout::write_header(header.base.as_ptr(), spec) };
    }

    let view = map(section.as_handle(), FILE_MAP_READ, spec.bytes())?;
    Ok((Section(section), view))
}

/// Maps a renderer's section read-write. A section smaller than the spec
/// fails the map, and the header check catches one larger.
///
/// # Safety
///
/// No other writer maps this section for the view's life.
#[cfg(any(test, feature = "writer"))]
pub(crate) unsafe fn map_writable(section: BorrowedHandle<'_>, spec: &Spec) -> Result<View, Error> {
    map(section, FILE_MAP_WRITE, spec.bytes())
}

/// Whether the region at `base` is one read-only view at least `len` long.
#[cfg(test)]
pub(crate) fn read_only(base: *const u8, len: usize) -> bool {
    use windows_sys::Win32::System::Memory::{
        MEMORY_BASIC_INFORMATION, PAGE_READONLY, VirtualQuery,
    };

    let mut info = MEMORY_BASIC_INFORMATION::default();

    // SAFETY: VirtualQuery fills one struct we own, of the size we pass.
    let got = unsafe {
        VirtualQuery(
            base.cast(),
            &mut info,
            std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    };

    got != 0 && info.Protect == PAGE_READONLY && info.RegionSize >= len
}
