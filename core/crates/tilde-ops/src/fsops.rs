//! The primitive file operations everything else is built from.

use std::ffi::{CString, c_char, c_int, c_void};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

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

/// Byte progress for copies that move data (to another volume; same-volume copies clone
/// instantly), and a way to stop them partway.
pub struct Progress<'a> {
    /// Bytes copied so far and in total, a few times a second.
    pub report: &'a dyn Fn(u64, u64),
    pub cancel: &'a AtomicBool,
}

/// The error for a copy stopped by `Progress::cancel`.
pub const CANCELLED: &str = "Cancelled";

/// Moves (or renames) an item. Across volumes it's a copy, then the original is removed.
pub fn move_item(from: &Path, to: &Path, progress: Option<&Progress>) -> io::Result<()> {
    ensure_free(Some(from), to)?;
    match std::fs::rename(from, to) {
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            copy_item(from, to, progress)?;
            remove(from)
        }
        other => other,
    }
}

/// Copies an item (folders recursively). On APFS this clones, so it's instant and takes no
/// extra space until either copy changes. Keeps permissions, extended attributes and tags.
pub fn copy_item(from: &Path, to: &Path, progress: Option<&Progress>) -> io::Result<()> {
    copy_with(from, to, true, progress)
}

pub(crate) fn copy_with(from: &Path, to: &Path, clone: bool, progress: Option<&Progress>) -> io::Result<()> {
    ensure_free(None, to)?;
    let c = |p: &Path| CString::new(p.as_os_str().as_bytes()).map_err(|_| io::Error::other("path contains NUL"));
    let (src, dst) = (c(from)?, c(to)?);
    let mut flags = libc::COPYFILE_RECURSIVE
        | libc::COPYFILE_DATA
        | libc::COPYFILE_METADATA
        | libc::COPYFILE_NOFOLLOW
        | libc::COPYFILE_EXCL;
    if clone {
        flags |= libc::COPYFILE_CLONE;
    }
    // Only data copies report progress: a clone on the same volume is instant, so the
    // walk to total up sizes is skipped there.
    let progress = progress.filter(|_| !clone || !same_volume(from, to));
    let mut ctx = progress.map(|p| CopyContext {
        progress: p,
        total: total_bytes(from),
        finished: 0,
        current: 0,
        last: None,
        quit: false,
    });
    let rc = unsafe {
        let state = libc::copyfile_state_alloc();
        if let Some(ctx) = ctx.as_mut() {
            let cb: extern "C" fn(
                c_int,
                c_int,
                libc::copyfile_state_t,
                *const c_char,
                *const c_char,
                *mut c_void,
            ) -> c_int = copy_status;
            libc::copyfile_state_set(state, libc::COPYFILE_STATE_STATUS_CB as u32, cb as *const c_void);
            libc::copyfile_state_set(
                state,
                libc::COPYFILE_STATE_STATUS_CTX as u32,
                ctx as *mut CopyContext as *const c_void,
            );
        }
        let rc = libc::copyfile(src.as_ptr(), dst.as_ptr(), state, flags);
        libc::copyfile_state_free(state);
        rc
    };
    let cancelled = ctx.as_ref().is_some_and(|c| c.quit);
    if rc != 0 || cancelled {
        let err =
            if cancelled { io::Error::new(io::ErrorKind::Interrupted, CANCELLED) } else { io::Error::last_os_error() };
        // Don't leave half a copy behind.
        let _ = remove(to);
        return Err(err);
    }
    if let Some(ctx) = &ctx {
        (ctx.progress.report)(ctx.total, ctx.total);
    }
    Ok(())
}

struct CopyContext<'a> {
    progress: &'a Progress<'a>,
    total: u64,
    /// Bytes in files already finished (recursive copies).
    finished: u64,
    /// Bytes copied of the current file.
    current: u64,
    last: Option<Instant>,
    quit: bool,
}

const REPORT_EVERY: Duration = Duration::from_millis(100);

extern "C" fn copy_status(
    what: c_int,
    stage: c_int,
    state: libc::copyfile_state_t,
    _src: *const c_char,
    _dst: *const c_char,
    ctx: *mut c_void,
) -> c_int {
    // SAFETY: `ctx` is the `CopyContext` set in `copy_with`, alive for the whole copy.
    let ctx = unsafe { &mut *(ctx as *mut CopyContext) };
    if ctx.progress.cancel.load(Ordering::SeqCst) {
        ctx.quit = true;
        return libc::COPYFILE_QUIT;
    }
    if what == libc::COPYFILE_COPY_DATA && stage == libc::COPYFILE_PROGRESS {
        let mut copied: libc::off_t = 0;
        unsafe {
            libc::copyfile_state_get(state, libc::COPYFILE_STATE_COPIED as u32, &mut copied as *mut _ as *mut c_void)
        };
        ctx.current = copied.max(0) as u64;
        if ctx.last.is_none_or(|t| t.elapsed() >= REPORT_EVERY) {
            ctx.last = Some(Instant::now());
            (ctx.progress.report)((ctx.finished + ctx.current).min(ctx.total), ctx.total);
        }
    } else if what == libc::COPYFILE_RECURSE_FILE && stage == libc::COPYFILE_FINISH {
        ctx.finished += std::mem::take(&mut ctx.current);
    }
    libc::COPYFILE_CONTINUE
}

/// Whether `to` (not yet created) would be on the same volume as `from`.
fn same_volume(from: &Path, to: &Path) -> bool {
    match (from.symlink_metadata(), to.parent().map(|p| p.metadata())) {
        (Ok(a), Some(Ok(b))) => a.dev() == b.dev(),
        _ => false,
    }
}

/// Size of the files in `path`, recursively (symlinks aren't followed).
fn total_bytes(path: &Path) -> u64 {
    let Ok(meta) = path.symlink_metadata() else { return 0 };
    if !meta.is_dir() {
        return if meta.is_file() { meta.len() } else { 0 };
    }
    std::fs::read_dir(path)
        .map(|entries| entries.filter_map(Result::ok).map(|e| total_bytes(&e.path())).sum())
        .unwrap_or(0)
}

/// An item's Finder tags.
pub fn tags(path: &Path) -> io::Result<Vec<String>> {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSArray, NSString, NSURLTagNamesKey};
    if path.symlink_metadata().is_err() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "no longer exists"));
    }
    let url = file_url(path);
    let mut value: Option<Retained<AnyObject>> = None;
    unsafe { url.getResourceValue_forKey_error(&mut value, NSURLTagNamesKey) }
        .map_err(|e| io::Error::other(e.localizedDescription().to_string()))?;
    let Some(value) = value else { return Ok(Vec::new()) };
    let Ok(array) = value.downcast::<NSArray>() else { return Ok(Vec::new()) };
    // SAFETY: tag names are strings.
    let names: Retained<NSArray<NSString>> = unsafe { Retained::cast_unchecked(array) };
    Ok(names.iter().map(|n| n.to_string()).collect())
}

/// Replaces an item's Finder tags. Colour tags ("Red", "Blue") show in colour in the Finder.
pub fn set_tags(path: &Path, tags: &[String]) -> io::Result<()> {
    use objc2_foundation::{NSArray, NSString, NSURLTagNamesKey};
    if path.symlink_metadata().is_err() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "no longer exists"));
    }
    let url = file_url(path);
    let names: Vec<_> = tags.iter().map(|t| NSString::from_str(t)).collect();
    let array = NSArray::from_retained_slice(&names);
    unsafe { url.setResourceValue_forKey_error(Some(&array), NSURLTagNamesKey) }
        .map_err(|e| io::Error::other(e.localizedDescription().to_string()))
}

fn file_url(path: &Path) -> objc2::rc::Retained<objc2_foundation::NSURL> {
    use objc2_foundation::{NSString, NSURL};
    NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()))
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
            copy_item(s, &staging.join(name), None)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn data_copies_report_progress_and_can_stop() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("big.bin");
        std::fs::write(&from, vec![7u8; 24 << 20]).unwrap();
        let seen = Mutex::new(Vec::new());
        let report = |copied, total| seen.lock().unwrap().push((copied, total));
        let cancel = AtomicBool::new(false);
        let progress = Progress { report: &report, cancel: &cancel };

        let to = dir.path().join("copy.bin");
        copy_with(&from, &to, false, Some(&progress)).unwrap();
        assert_eq!(std::fs::metadata(&to).unwrap().len(), 24 << 20);
        let seen = std::mem::take(&mut *seen.lock().unwrap());
        assert!(seen.len() >= 2, "{seen:?}");
        assert!(seen.iter().all(|&(c, t)| t == 24 << 20 && c <= t));
        assert_eq!(seen.last(), Some(&(24 << 20, 24 << 20)));

        cancel.store(true, Ordering::SeqCst);
        let to = dir.path().join("stopped.bin");
        let err = copy_with(&from, &to, false, Some(&progress)).unwrap_err();
        assert_eq!(err.to_string(), CANCELLED);
        assert!(!to.exists(), "partial copy removed");
    }

    #[test]
    fn tags_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.txt");
        std::fs::write(&p, "x").unwrap();
        assert_eq!(tags(&p).unwrap(), Vec::<String>::new());
        set_tags(&p, &["Work".into(), "Red".into()]).unwrap();
        let mut got = tags(&p).unwrap();
        got.sort();
        assert_eq!(got, ["Red", "Work"]);
        set_tags(&p, &[]).unwrap();
        assert!(tags(&p).unwrap().is_empty());
        assert!(tags(&dir.path().join("missing")).is_err());
    }
}
