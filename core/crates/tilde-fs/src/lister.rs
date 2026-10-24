//! Folder listing with `getattrlistbulk(2)`, which returns names and attributes for
//! many entries per system call. Much faster than `readdir` plus a `stat` per entry.

use std::ffi::CString;
use std::io;
use std::mem::size_of;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use tilde_types::{Entry, EntryKind};

/// Not exported by the `libc` crate.
const ATTR_CMN_ERROR: u32 = 0x2000_0000;
/// `chflags` hidden flag (what `chflags hidden` and the Finder set).
const UF_HIDDEN: u32 = 0x0000_8000;

// `fsobj_type_t` values (`enum vtype` in <sys/vnode.h>).
const VREG: u32 = 1;
const VDIR: u32 = 2;
const VLNK: u32 = 5;

/// Folder extensions macOS treats as single documents. A stopgap until package
/// detection asks Launch Services from the Swift side.
const PACKAGE_EXTENSIONS: &[&str] = &[
    "app",
    "bundle",
    "framework",
    "plugin",
    "kext",
    "appex",
    "xpc",
    "prefpane",
    "qlgenerator",
    "mdimporter",
    "rtfd",
    "pages",
    "numbers",
    "key",
    "photoslibrary",
    "musiclibrary",
    "tvlibrary",
    "xcodeproj",
    "xcworkspace",
    "playground",
    "xcassets",
    "lproj_bundle",
    "pkg",
    "mpkg",
    "sparsebundle",
    "fcpbundle",
    "logicx",
    "band",
    "scriptd",
    "workflow",
];

#[derive(Debug, Clone)]
pub struct ListOptions {
    pub include_hidden: bool,
    /// Bytes per `getattrlistbulk` call. Larger means fewer system calls.
    pub buffer_size: usize,
}

impl Default for ListOptions {
    fn default() -> Self {
        ListOptions { include_hidden: false, buffer_size: 256 * 1024 }
    }
}

/// Lists the entries of `dir` (unsorted). Returns `ErrorKind::Interrupted` if
/// `should_cancel` returns true between batches.
pub fn list_dir(dir: &Path, opts: &ListOptions, should_cancel: impl Fn() -> bool) -> io::Result<Vec<Entry>> {
    let c_path = CString::new(dir.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))?;
    let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let _guard = FdGuard(fd);

    let mut attrs = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: libc::ATTR_CMN_RETURNED_ATTRS
            | ATTR_CMN_ERROR
            | libc::ATTR_CMN_NAME
            | libc::ATTR_CMN_OBJTYPE
            | libc::ATTR_CMN_MODTIME
            | libc::ATTR_CMN_FLAGS
            | libc::ATTR_CMN_FILEID,
        volattr: 0,
        dirattr: libc::ATTR_DIR_ENTRYCOUNT,
        fileattr: libc::ATTR_FILE_DATALENGTH,
        forkattr: 0,
    };

    // u64 storage keeps the buffer 8-byte aligned.
    let mut buf = vec![0u64; opts.buffer_size.max(4096) / 8];
    let buf_bytes = buf.len() * 8;
    let mut entries = Vec::new();

    loop {
        if should_cancel() {
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        let count = unsafe {
            libc::getattrlistbulk(
                fd,
                (&mut attrs as *mut libc::attrlist).cast(),
                buf.as_mut_ptr().cast(),
                buf_bytes,
                libc::FSOPT_PACK_INVAL_ATTRS as u64,
            )
        };
        if count < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        if count == 0 {
            break;
        }
        let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), buf_bytes) };
        let mut offset = 0usize;
        for _ in 0..count {
            let record_len = read_u32(bytes, offset) as usize;
            if record_len == 0 || offset + record_len > bytes.len() {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "malformed getattrlistbulk record"));
            }
            if let Some(entry) = parse_record(&bytes[offset..offset + record_len])
                && (opts.include_hidden || !entry.hidden)
            {
                entries.push(entry);
            }
            offset += record_len;
        }
    }
    Ok(entries)
}

/// Parses one record. Attributes are packed in a fixed order: the returned-attributes
/// set, the error code, then common, directory and file attributes in bit order.
/// `FSOPT_PACK_INVAL_ATTRS` guarantees every requested attribute has a slot.
fn parse_record(rec: &[u8]) -> Option<Entry> {
    let mut p = 4; // skip record length
    let returned = libc::attribute_set_t {
        commonattr: read_u32(rec, p),
        volattr: read_u32(rec, p + 4),
        dirattr: read_u32(rec, p + 8),
        fileattr: read_u32(rec, p + 12),
        forkattr: read_u32(rec, p + 16),
    };
    p += size_of::<libc::attribute_set_t>();

    let error = read_u32(rec, p);
    p += 4;
    if returned.commonattr & ATTR_CMN_ERROR != 0 && error != 0 {
        return None; // unreadable entry (e.g. permissions); skip it
    }

    // attrreference_t: offset (relative to this field) and length including NUL.
    let name_ref_at = p;
    let name_off = read_i32(rec, p);
    let name_len = read_u32(rec, p + 4) as usize;
    p += size_of::<libc::attrreference_t>();
    let start = (name_ref_at as i64 + name_off as i64) as usize;
    let end = start + name_len.saturating_sub(1);
    let name = String::from_utf8_lossy(rec.get(start..end)?).into_owned();

    let obj_type = read_u32(rec, p);
    p += 4;
    let modified = read_i64(rec, p); // timespec.tv_sec
    p += size_of::<libc::timespec>();
    let flags = read_u32(rec, p);
    p += 4;
    let file_id = read_u64(rec, p);
    p += 8;
    // Directory and file groups are only packed for objects of that type, even with
    // FSOPT_PACK_INVAL_ATTRS, so their presence comes from the returned set.
    let entry_count = (returned.dirattr & libc::ATTR_DIR_ENTRYCOUNT != 0).then(|| {
        let v = read_u32(rec, p);
        p += 4;
        v
    });
    let data_length = (returned.fileattr & libc::ATTR_FILE_DATALENGTH != 0).then(|| read_u64(rec, p));

    let mut entry = Entry {
        hidden: name.starts_with('.') || flags & UF_HIDDEN != 0,
        kind: EntryKind::Other,
        size: 0,
        modified,
        file_id,
        child_count: None,
        name,
    };
    match obj_type {
        VREG => {
            entry.kind = EntryKind::File;
            entry.size = data_length.unwrap_or(0);
        }
        VDIR => {
            let is_package = PACKAGE_EXTENSIONS.contains(&entry.extension().as_str());
            entry.kind = if is_package { EntryKind::Package } else { EntryKind::Folder };
            entry.child_count = entry_count;
        }
        VLNK => entry.kind = EntryKind::Symlink,
        _ => {}
    }
    Some(entry)
}

fn read_u32(b: &[u8], at: usize) -> u32 {
    u32::from_ne_bytes(b[at..at + 4].try_into().unwrap())
}
fn read_i32(b: &[u8], at: usize) -> i32 {
    i32::from_ne_bytes(b[at..at + 4].try_into().unwrap())
}
fn read_u64(b: &[u8], at: usize) -> u64 {
    u64::from_ne_bytes(b[at..at + 8].try_into().unwrap())
}
fn read_i64(b: &[u8], at: usize) -> i64 {
    i64::from_ne_bytes(b[at..at + 8].try_into().unwrap())
}

struct FdGuard(libc::c_int);
impl Drop for FdGuard {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    fn names(entries: &[Entry]) -> Vec<String> {
        let mut v: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
        v.sort();
        v
    }

    #[test]
    fn lists_files_folders_symlinks_and_packages() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("notes.txt"), b"hello world").unwrap();
        fs::write(root.join("café.md"), b"").unwrap();
        fs::create_dir(root.join("src")).unwrap();
        fs::write(root.join("src/a.rs"), b"").unwrap();
        fs::write(root.join("src/b.rs"), b"").unwrap();
        fs::create_dir(root.join("Thing.app")).unwrap();
        symlink(root.join("notes.txt"), root.join("link")).unwrap();
        fs::write(root.join(".hidden"), b"").unwrap();

        let entries = list_dir(root, &ListOptions::default(), || false).unwrap();
        assert_eq!(names(&entries), ["Thing.app", "café.md", "link", "notes.txt", "src"]);

        let get = |n: &str| entries.iter().find(|e| e.name == n).unwrap();
        assert_eq!(get("notes.txt").kind, EntryKind::File);
        assert_eq!(get("notes.txt").size, 11);
        assert!(get("notes.txt").modified > 1_600_000_000);
        assert!(get("notes.txt").file_id > 0);
        assert_eq!(get("src").kind, EntryKind::Folder);
        assert_eq!(get("src").child_count, Some(2));
        assert_eq!(get("Thing.app").kind, EntryKind::Package);
        assert_eq!(get("link").kind, EntryKind::Symlink);
    }

    #[test]
    fn hidden_files_are_optional() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".env"), b"").unwrap();
        fs::write(dir.path().join("visible"), b"").unwrap();
        let opts = ListOptions { include_hidden: true, ..Default::default() };
        let entries = list_dir(dir.path(), &opts, || false).unwrap();
        assert_eq!(names(&entries), [".env", "visible"]);
        assert!(entries.iter().find(|e| e.name == ".env").unwrap().hidden);
    }

    #[test]
    fn many_entries_across_small_buffers() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..1500 {
            fs::write(dir.path().join(format!("file-{i:04}-with-a-longish-name.dat")), b"x").unwrap();
        }
        let opts = ListOptions { buffer_size: 4096, ..Default::default() };
        let entries = list_dir(dir.path(), &opts, || false).unwrap();
        assert_eq!(entries.len(), 1500);
        assert!(entries.iter().all(|e| e.size == 1 && e.kind == EntryKind::File));
    }

    #[test]
    fn cancellation_and_errors() {
        let dir = tempfile::tempdir().unwrap();
        let err = list_dir(dir.path(), &ListOptions::default(), || true).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Interrupted);

        let err = list_dir(&dir.path().join("missing"), &ListOptions::default(), || false).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);

        fs::write(dir.path().join("file"), b"").unwrap();
        assert!(list_dir(&dir.path().join("file"), &ListOptions::default(), || false).is_err());
    }
}
