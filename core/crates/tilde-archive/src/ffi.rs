//! The few libarchive calls Tilde needs. macOS ships libarchive (`/usr/lib/libarchive.2.dylib`,
//! with a `.tbd` in the SDK) but not its headers, so these are declared by hand from
//! `archive.h` and `archive_entry.h` (libarchive 3.x).

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

#[repr(C)]
pub struct Archive {
    _private: [u8; 0],
}

#[repr(C)]
pub struct ArchiveEntry {
    _private: [u8; 0],
}

pub const ARCHIVE_EOF: c_int = 1;
pub const ARCHIVE_OK: c_int = 0;
pub const ARCHIVE_RETRY: c_int = -10;
pub const ARCHIVE_WARN: c_int = -20;

pub const AE_IFMT: u32 = 0o170000;
pub const AE_IFREG: u32 = 0o100000;
pub const AE_IFDIR: u32 = 0o040000;
pub const AE_IFLNK: u32 = 0o120000;

#[link(name = "archive")]
unsafe extern "C" {
    fn archive_read_new() -> *mut Archive;
    fn archive_read_support_filter_all(a: *mut Archive) -> c_int;
    fn archive_read_support_format_all(a: *mut Archive) -> c_int;
    fn archive_read_open_filename(a: *mut Archive, filename: *const c_char, block_size: usize) -> c_int;
    fn archive_read_next_header(a: *mut Archive, entry: *mut *mut ArchiveEntry) -> c_int;
    fn archive_read_data(a: *mut Archive, buf: *mut c_void, size: usize) -> isize;
    fn archive_read_data_skip(a: *mut Archive) -> c_int;
    fn archive_read_free(a: *mut Archive) -> c_int;
    fn archive_error_string(a: *mut Archive) -> *const c_char;

    fn archive_entry_pathname_utf8(e: *mut ArchiveEntry) -> *const c_char;
    fn archive_entry_pathname(e: *mut ArchiveEntry) -> *const c_char;
    fn archive_entry_size(e: *mut ArchiveEntry) -> i64;
    fn archive_entry_size_is_set(e: *mut ArchiveEntry) -> c_int;
    fn archive_entry_mtime(e: *mut ArchiveEntry) -> libc::time_t;
    fn archive_entry_filetype(e: *mut ArchiveEntry) -> libc::mode_t;
    fn archive_entry_perm(e: *mut ArchiveEntry) -> libc::mode_t;
}

/// One header read from an archive.
pub struct Header {
    pub path: String,
    /// `AE_IFREG`, `AE_IFDIR`, `AE_IFLNK` or another type.
    pub filetype: u32,
    pub size: u64,
    pub mtime: i64,
    pub perm: u32,
}

/// An archive open for reading, front to back.
pub struct Reader {
    a: *mut Archive,
}

impl Reader {
    pub fn open(path: &Path) -> io::Result<Reader> {
        let c_path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))?;
        // SAFETY: plain libarchive calls on a handle we own; freed in Drop.
        unsafe {
            let a = archive_read_new();
            if a.is_null() {
                return Err(io::Error::other("couldn't start reading the archive"));
            }
            let reader = Reader { a };
            archive_read_support_filter_all(a);
            archive_read_support_format_all(a);
            if archive_read_open_filename(a, c_path.as_ptr(), 64 * 1024) != ARCHIVE_OK {
                return Err(reader.error());
            }
            Ok(reader)
        }
    }

    /// The next header, or `None` at the end.
    pub fn next(&mut self) -> io::Result<Option<Header>> {
        let mut entry: *mut ArchiveEntry = std::ptr::null_mut();
        loop {
            // SAFETY: `self.a` is open; `entry` is valid until the next call.
            let r = unsafe { archive_read_next_header(self.a, &mut entry) };
            match r {
                ARCHIVE_OK | ARCHIVE_WARN => break,
                ARCHIVE_EOF => return Ok(None),
                ARCHIVE_RETRY => continue,
                _ => return Err(self.error()),
            }
        }
        // SAFETY: `entry` was just filled in by libarchive.
        unsafe {
            let mut name = archive_entry_pathname_utf8(entry);
            if name.is_null() {
                name = archive_entry_pathname(entry);
            }
            let path = if name.is_null() { String::new() } else { CStr::from_ptr(name).to_string_lossy().into_owned() };
            Ok(Some(Header {
                path,
                filetype: archive_entry_filetype(entry) as u32 & AE_IFMT,
                size: if archive_entry_size_is_set(entry) != 0 { archive_entry_size(entry).max(0) as u64 } else { 0 },
                mtime: archive_entry_mtime(entry) as i64,
                perm: archive_entry_perm(entry) as u32,
            }))
        }
    }

    /// Reads the current entry's data into `buf`; 0 at its end.
    pub fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // SAFETY: `buf` is valid for `buf.len()` bytes.
        let n = unsafe { archive_read_data(self.a, buf.as_mut_ptr().cast(), buf.len()) };
        if n < 0 { Err(self.error()) } else { Ok(n as usize) }
    }

    pub fn skip(&mut self) -> io::Result<()> {
        // SAFETY: `self.a` is open.
        match unsafe { archive_read_data_skip(self.a) } {
            ARCHIVE_OK | ARCHIVE_WARN => Ok(()),
            _ => Err(self.error()),
        }
    }

    fn error(&self) -> io::Error {
        // SAFETY: the string belongs to the handle and is copied straight away.
        let message = unsafe {
            let s = archive_error_string(self.a);
            if s.is_null() {
                "couldn't read the archive".to_string()
            } else {
                CStr::from_ptr(s).to_string_lossy().into_owned()
            }
        };
        io::Error::other(message)
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        // SAFETY: freed once, here.
        unsafe {
            archive_read_free(self.a);
        }
    }
}
