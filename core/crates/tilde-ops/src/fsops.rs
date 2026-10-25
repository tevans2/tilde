//! The primitive file operations everything else is built from.

use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where trashed items go. The system one asks the Finder's Trash; tests use a folder.
pub trait Trash: Send + Sync {
    /// Moves `path` to the trash and returns where it ended up (needed for undo).
    fn trash(&self, path: &Path) -> io::Result<PathBuf>;
}

/// The user's Trash, via `NSFileManager`, so items can be put back from the Finder too.
pub struct SystemTrash;

impl Trash for SystemTrash {
    fn trash(&self, path: &Path) -> io::Result<PathBuf> {
        use objc2::rc::Retained;
        use objc2_foundation::{NSFileManager, NSString, NSURL};
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        let mut resulting: Option<Retained<NSURL>> = None;
        let manager = NSFileManager::defaultManager();
        manager
            .trashItemAtURL_resultingItemURL_error(&url, Some(&mut resulting))
            .map_err(|e| io::Error::other(e.localizedDescription().to_string()))?;
        resulting
            .and_then(|u| u.path())
            .map(|p| PathBuf::from(p.to_string()))
            .ok_or_else(|| io::Error::other("the Trash didn't say where the item went"))
    }
}

/// A plain folder standing in for the Trash (tests).
pub struct FolderTrash(pub PathBuf);

impl Trash for FolderTrash {
    fn trash(&self, path: &Path) -> io::Result<PathBuf> {
        let name = path.file_name().ok_or_else(|| io::Error::other("no file name"))?.to_string_lossy();
        let is_folder = path.is_dir();
        let to = crate::names::keep_both(&self.0, &name, is_folder, |p| p.symlink_metadata().is_ok());
        std::fs::rename(path, &to)?;
        Ok(to)
    }
}

/// Whether two paths are the same file (true for a case-only rename on a case-insensitive
/// volume).
pub fn same_file(a: &Path, b: &Path) -> bool {
    match (a.symlink_metadata(), b.symlink_metadata()) {
        (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
        _ => false,
    }
}

/// Fails if `to` is taken by anything other than `from` itself. `rename(2)` would
/// silently replace a file, so every step checks first.
pub fn ensure_free(from: Option<&Path>, to: &Path) -> io::Result<()> {
    if to.symlink_metadata().is_ok() && !from.is_some_and(|f| same_file(f, to)) {
        let name = to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("“{name}” already exists")));
    }
    Ok(())
}

/// Moves (or renames) an item. Across volumes it's a copy, then the original is removed.
pub fn move_item(from: &Path, to: &Path) -> io::Result<()> {
    ensure_free(Some(from), to)?;
    match std::fs::rename(from, to) {
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            copy_item(from, to)?;
            remove(from)
        }
        other => other,
    }
}

/// Copies an item (folders recursively). On APFS this clones, so it's instant and takes no
/// extra space until either copy changes. Keeps permissions, extended attributes and tags.
pub fn copy_item(from: &Path, to: &Path) -> io::Result<()> {
    ensure_free(None, to)?;
    let c = |p: &Path| CString::new(p.as_os_str().as_bytes()).map_err(|_| io::Error::other("path contains NUL"));
    let (src, dst) = (c(from)?, c(to)?);
    let flags = libc::COPYFILE_CLONE
        | libc::COPYFILE_RECURSIVE
        | libc::COPYFILE_DATA
        | libc::COPYFILE_METADATA
        | libc::COPYFILE_NOFOLLOW
        | libc::COPYFILE_EXCL;
    let rc = unsafe { libc::copyfile(src.as_ptr(), dst.as_ptr(), std::ptr::null_mut(), flags) };
    if rc != 0 {
        let err = io::Error::last_os_error();
        // Don't leave half a copy behind.
        let _ = remove(to);
        return Err(err);
    }
    Ok(())
}

/// Creates an empty file (failing if anything is already there).
pub fn create_file(path: &Path) -> io::Result<()> {
    ensure_free(None, path)?;
    std::fs::OpenOptions::new().write(true).create_new(true).open(path).map(|_| ())
}

pub fn make_dir(path: &Path) -> io::Result<()> {
    ensure_free(None, path)?;
    std::fs::create_dir(path)
}

/// Zips `sources` into `to`. One item is zipped with its name as the top folder, like the
/// Finder; several are cloned into a hidden staging folder next to `to` and zipped from
/// there, so they sit at the top of the archive.
pub fn compress(sources: &[PathBuf], to: &Path) -> io::Result<()> {
    ensure_free(None, to)?;
    let ditto = |args: &[&std::ffi::OsStr]| -> io::Result<()> {
        let out = Command::new("/usr/bin/ditto").args(args).output()?;
        if out.status.success() {
            Ok(())
        } else {
            Err(io::Error::other(String::from_utf8_lossy(&out.stderr).trim().to_string()))
        }
    };
    if let [single] = sources {
        return ditto(&[
            "-c".as_ref(),
            "-k".as_ref(),
            "--sequesterRsrc".as_ref(),
            "--keepParent".as_ref(),
            single.as_os_str(),
            to.as_os_str(),
        ]);
    }
    let parent = to.parent().ok_or_else(|| io::Error::other("no destination folder"))?;
    let staging = parent.join(format!(".tilde-zip-{}", std::process::id()));
    std::fs::create_dir(&staging)?;
    let result = (|| {
        for s in sources {
            let name = s.file_name().ok_or_else(|| io::Error::other("no file name"))?;
            copy_item(s, &staging.join(name))?;
        }
        ditto(&["-c".as_ref(), "-k".as_ref(), "--sequesterRsrc".as_ref(), staging.as_os_str(), to.as_os_str()])
    })();
    let _ = std::fs::remove_dir_all(&staging);
    result
}

fn remove(path: &Path) -> io::Result<()> {
    match path.symlink_metadata() {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}
