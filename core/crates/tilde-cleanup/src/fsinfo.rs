//! What the cleanup rules need to know about a file beyond the index: the space it really
//! takes, how much of that would be freed (clones share blocks), when it was last opened,
//! and whether it's only a cloud placeholder.

use std::collections::HashSet;
use std::ffi::CString;
use std::os::darwin::fs::MetadataExt as _;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// `SF_DATALESS`: the contents are in the cloud (iCloud, other file providers).
const SF_DATALESS: u32 = 0x4000_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stat {
    /// Logical size.
    pub size: u64,
    /// Space allocated on disk.
    pub allocated: u64,
    pub modified: i64,
    /// Nanoseconds since the epoch, for telling a rewritten file apart.
    pub modified_ns: i128,
    pub created: i64,
    pub dev: u64,
    pub ino: u64,
    pub links: u64,
    pub is_dir: bool,
    pub is_file: bool,
    /// Only a placeholder; the contents aren't on this Mac.
    pub dataless: bool,
}

/// `lstat`, as a `Stat`.
pub fn stat(path: &Path) -> Option<Stat> {
    let m = path.symlink_metadata().ok()?;
    Some(Stat {
        size: m.len(),
        allocated: m.blocks() * 512,
        modified: m.mtime(),
        modified_ns: m.mtime() as i128 * 1_000_000_000 + m.mtime_nsec() as i128,
        created: m.st_birthtime(),
        dev: m.dev(),
        ino: m.ino(),
        links: m.nlink(),
        is_dir: m.is_dir(),
        is_file: m.is_file(),
        dataless: m.st_flags() & SF_DATALESS != 0,
    })
}

/// The space trashing this file would free: what it doesn't share with APFS clones
/// (`ATTR_CMNEXT_PRIVATESIZE`). Falls back to the allocated size where the volume can't
/// say.
pub fn freed_by(path: &Path, stat: &Stat) -> u64 {
    private_size(path).unwrap_or(stat.allocated)
}

fn private_size(path: &Path) -> Option<u64> {
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut list = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: 0,
        volattr: 0,
        dirattr: 0,
        fileattr: 0,
        forkattr: libc::ATTR_CMNEXT_PRIVATESIZE,
    };
    // u32 length, then the off_t.
    #[repr(C, packed(4))]
    struct Buf {
        length: u32,
        private: i64,
    }
    let mut buf = Buf { length: 0, private: 0 };
    let rc = unsafe {
        libc::getattrlist(
            c.as_ptr(),
            &mut list as *mut _ as *mut libc::c_void,
            &mut buf as *mut _ as *mut libc::c_void,
            std::mem::size_of::<Buf>(),
            libc::FSOPT_ATTR_CMN_EXTENDED | libc::FSOPT_NOFOLLOW,
        )
    };
    let (length, private) = (buf.length, buf.private);
    (rc == 0 && length as usize >= std::mem::size_of::<Buf>() && private >= 0).then_some(private as u64)
}

/// When the file was last opened, as macOS records it (`com.apple.lastuseddate#PS`, set
/// when apps open it through Launch Services, as the Finder does).
pub fn last_opened(path: &Path) -> Option<i64> {
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    let name = CString::new("com.apple.lastuseddate#PS").unwrap();
    let mut buf = [0u8; 16];
    let n = unsafe {
        libc::getxattr(
            c.as_ptr(),
            name.as_ptr(),
            buf.as_mut_ptr() as *mut libc::c_void,
            buf.len(),
            0,
            libc::XATTR_NOFOLLOW,
        )
    };
    (n >= 8).then(|| i64::from_le_bytes(buf[..8].try_into().unwrap()))
}

/// The space a folder (or file) takes: allocated sizes of everything inside, hard links
/// counted once, not crossing into other volumes, cloud placeholders counted as nothing.
/// Returns (bytes, files). Stops early (returning what it has) if `cancel` is set.
pub fn measure(path: &Path, cancel: &AtomicBool) -> (u64, u64) {
    let Some(root) = stat(path) else { return (0, 0) };
    if !root.is_dir {
        return (if root.dataless { 0 } else { root.allocated }, 1);
    }
    let mut seen_links = HashSet::new();
    let (mut bytes, mut files) = (root.allocated, 0u64);
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let p = entry.path();
            let Some(s) = stat(&p) else { continue };
            if s.dev != root.dev {
                continue;
            }
            if s.is_dir {
                bytes += s.allocated;
                stack.push(p);
                continue;
            }
            if s.links > 1 && !seen_links.insert((s.dev, s.ino)) {
                continue;
            }
            files += 1;
            if !s.dataless {
                bytes += s.allocated;
            }
        }
    }
    (bytes, files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_clones() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.bin");
        std::fs::write(&a, vec![7u8; 4 << 20]).unwrap();
        let sa = stat(&a).unwrap();
        assert_eq!(sa.size, 4 << 20);
        assert!(sa.allocated >= 4 << 20);
        assert!(freed_by(&a, &sa) >= 4 << 20, "a plain file frees all its space");

        // An APFS clone shares its blocks: trashing it frees (almost) nothing.
        let b = dir.path().join("b.bin");
        let (ca, cb) =
            (CString::new(a.as_os_str().as_bytes()).unwrap(), CString::new(b.as_os_str().as_bytes()).unwrap());
        let rc = unsafe { libc::clonefile(ca.as_ptr(), cb.as_ptr(), 0) };
        if rc == 0 {
            let sb = stat(&b).unwrap();
            assert!(freed_by(&b, &sb) < 1 << 20, "clone frees {}", freed_by(&b, &sb));
        }

        let cancel = AtomicBool::new(false);
        let (bytes, files) = measure(dir.path(), &cancel);
        assert_eq!(files, 2);
        assert!(bytes >= 8 << 20, "allocated sizes add up: {bytes}");
        assert_eq!(last_opened(&a), None);
    }
}
