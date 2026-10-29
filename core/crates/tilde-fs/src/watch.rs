//! Folder-level change notifications from FSEvents.
//!
//! Events name the folders whose contents changed (not individual files), which is what
//! both the open folder and the index need: re-list that folder and diff.

use std::ffi::{CStr, CString, OsStr, c_char, c_void};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use core_foundation::array::CFArray;
use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use core_foundation_sys::array::CFArrayRef;
use core_foundation_sys::base::CFAllocatorRef;

/// FSEvents' "start from now" event ID.
pub const SINCE_NOW: u64 = u64::MAX;

// FSEventStreamCreateFlags
const CREATE_FLAG_NO_DEFER: u32 = 0x02;
// FSEventStreamEventFlags
const EVENT_MUST_SCAN_SUBDIRS: u32 = 0x01;
const EVENT_USER_DROPPED: u32 = 0x02;
const EVENT_KERNEL_DROPPED: u32 = 0x04;
const EVENT_IDS_WRAPPED: u32 = 0x08;
const EVENT_HISTORY_DONE: u32 = 0x10;
const EVENT_ROOT_CHANGED: u32 = 0x20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsEvent {
    /// The folder whose contents changed.
    pub path: PathBuf,
    pub flags: u32,
    /// Persistent, increasing event ID (for resuming after a restart).
    pub id: u64,
}

impl FsEvent {
    /// Events were coalesced or dropped: everything under `path` must be re-scanned.
    pub fn needs_rescan(&self) -> bool {
        self.flags & (EVENT_MUST_SCAN_SUBDIRS | EVENT_USER_DROPPED | EVENT_KERNEL_DROPPED | EVENT_ROOT_CHANGED) != 0
    }

    /// Event IDs ran out and started again: anything saved against an ID is meaningless,
    /// so everything must be re-scanned.
    pub fn ids_wrapped(&self) -> bool {
        self.flags & EVENT_IDS_WRAPPED != 0
    }

    /// Marker sent once replaying history (`since` an old ID) has finished.
    pub fn is_history_done(&self) -> bool {
        self.flags & EVENT_HISTORY_DONE != 0
    }
}

type Handler = Box<dyn Fn(Vec<FsEvent>) + Send + Sync>;
type FSEventStreamRef = *mut c_void;

#[repr(C)]
struct FSEventStreamContext {
    version: isize,
    info: *mut c_void,
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
}

type FSEventStreamCallback = extern "C" fn(
    stream: *const c_void,
    info: *mut c_void,
    count: usize,
    paths: *mut c_void,
    flags: *const u32,
    ids: *const u64,
);

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn FSEventStreamCreate(
        allocator: CFAllocatorRef,
        callback: FSEventStreamCallback,
        context: *const FSEventStreamContext,
        paths_to_watch: CFArrayRef,
        since_when: u64,
        latency: f64,
        flags: u32,
    ) -> FSEventStreamRef;
    fn FSEventStreamSetDispatchQueue(stream: FSEventStreamRef, queue: *mut c_void);
    fn FSEventStreamSetExclusionPaths(stream: FSEventStreamRef, paths: CFArrayRef) -> u8;
    fn FSEventStreamStart(stream: FSEventStreamRef) -> u8;
    fn FSEventStreamStop(stream: FSEventStreamRef);
    fn FSEventStreamInvalidate(stream: FSEventStreamRef);
    fn FSEventStreamRelease(stream: FSEventStreamRef);
    fn FSEventsGetCurrentEventId() -> u64;
    fn FSEventsCopyUUIDForDevice(dev: libc::dev_t) -> core_foundation_sys::uuid::CFUUIDRef;
}

unsafe extern "C" {
    fn dispatch_queue_create(label: *const c_char, attr: *const c_void) -> *mut c_void;
    fn dispatch_release(object: *mut c_void);
}

/// The most recent FSEvents event ID system-wide.
pub fn current_event_id() -> u64 {
    unsafe { FSEventsGetCurrentEventId() }
}

/// The identity of the FSEvents store on the volume holding `path`. Event IDs only mean
/// something with it: if it changes (the store was reset, or it's another disk), changes
/// since a saved ID can't be replayed. `None` if the volume keeps no store.
pub fn event_store_id(path: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let dev = std::fs::metadata(path).ok()?.dev() as libc::dev_t;
    // SAFETY: returns a new CFUUID (owned, released by the wrapper) or null.
    let uuid = unsafe { FSEventsCopyUUIDForDevice(dev) };
    if uuid.is_null() {
        return None;
    }
    let uuid = unsafe { core_foundation::uuid::CFUUID::wrap_under_create_rule(uuid) };
    let bytes = unsafe { core_foundation_sys::uuid::CFUUIDGetUUIDBytes(uuid.as_concrete_TypeRef()) };
    let b = [
        bytes.byte0,
        bytes.byte1,
        bytes.byte2,
        bytes.byte3,
        bytes.byte4,
        bytes.byte5,
        bytes.byte6,
        bytes.byte7,
        bytes.byte8,
        bytes.byte9,
        bytes.byte10,
        bytes.byte11,
        bytes.byte12,
        bytes.byte13,
        bytes.byte14,
        bytes.byte15,
    ];
    Some(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// A running FSEvents stream. Stops when dropped.
pub struct Watcher {
    stream: FSEventStreamRef,
    queue: *mut c_void,
    handler: *mut Handler,
}

// The stream and queue are only touched in `drop`; the handler is Send + Sync.
unsafe impl Send for Watcher {}

/// Watches `roots` recursively and calls `handler` with batches of changed folders on a
/// background queue. `exclude` (at most 8 paths) are ignored by FSEvents itself.
pub fn watch(
    roots: &[PathBuf],
    exclude: &[PathBuf],
    since: u64,
    latency: Duration,
    handler: impl Fn(Vec<FsEvent>) + Send + Sync + 'static,
) -> io::Result<Watcher> {
    let to_cf = |paths: &[PathBuf]| -> CFArray<CFString> {
        let strings: Vec<CFString> = paths.iter().map(|p| CFString::new(&p.to_string_lossy())).collect();
        CFArray::from_CFTypes(&strings)
    };
    let roots_cf = to_cf(roots);
    let handler: *mut Handler = Box::into_raw(Box::new(Box::new(handler)));
    let context = FSEventStreamContext {
        version: 0,
        info: handler.cast(),
        retain: std::ptr::null(),
        release: std::ptr::null(),
        copy_description: std::ptr::null(),
    };
    unsafe {
        let stream = FSEventStreamCreate(
            std::ptr::null(),
            callback,
            &context,
            roots_cf.as_concrete_TypeRef(),
            since,
            latency.as_secs_f64(),
            CREATE_FLAG_NO_DEFER,
        );
        if stream.is_null() {
            drop(Box::from_raw(handler));
            return Err(io::Error::other("FSEventStreamCreate failed"));
        }
        if !exclude.is_empty() {
            let exclude: Vec<PathBuf> = exclude.iter().take(8).cloned().collect();
            FSEventStreamSetExclusionPaths(stream, to_cf(&exclude).as_concrete_TypeRef());
        }
        let label = CString::new("tilde.fsevents").unwrap();
        let queue = dispatch_queue_create(label.as_ptr(), std::ptr::null());
        FSEventStreamSetDispatchQueue(stream, queue);
        let watcher = Watcher { stream, queue, handler };
        if FSEventStreamStart(stream) == 0 {
            return Err(io::Error::other("FSEventStreamStart failed"));
        }
        Ok(watcher)
    }
}

extern "C" fn callback(
    _stream: *const c_void,
    info: *mut c_void,
    count: usize,
    paths: *mut c_void,
    flags: *const u32,
    ids: *const u64,
) {
    // A panic must not unwind into CoreServices.
    let _ = std::panic::catch_unwind(|| {
        let handler = unsafe { &*(info as *const Handler) };
        let paths = paths as *const *const c_char;
        let events = (0..count)
            .map(|i| unsafe {
                let bytes = CStr::from_ptr(*paths.add(i)).to_bytes();
                FsEvent {
                    path: Path::new(OsStr::from_bytes(bytes)).to_path_buf(),
                    flags: *flags.add(i),
                    id: *ids.add(i),
                }
            })
            .collect();
        handler(events);
    });
}

impl Drop for Watcher {
    fn drop(&mut self) {
        unsafe {
            FSEventStreamStop(self.stream);
            // After invalidation no further callbacks run, so the handler can be freed.
            FSEventStreamInvalidate(self.stream);
            FSEventStreamRelease(self.stream);
            dispatch_release(self.queue);
            drop(Box::from_raw(self.handler));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_event_store_has_an_id() {
        let dir = tempfile::tempdir().unwrap();
        let id = event_store_id(dir.path()).expect("the boot volume keeps FSEvents");
        assert_eq!(id.len(), 32);
        assert_eq!(event_store_id(&std::env::temp_dir()), Some(id), "same volume, same store");
        assert_eq!(event_store_id(Path::new("/does/not/exist")), None);
    }
    use std::sync::mpsc;

    fn wait_for(rx: &mpsc::Receiver<Vec<FsEvent>>, pred: impl Fn(&FsEvent) -> bool) -> FsEvent {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let batch = rx.recv_timeout(left).expect("no matching FSEvent within 10s");
            if let Some(e) = batch.into_iter().find(|e| pred(e)) {
                return e;
            }
        }
    }

    #[test]
    fn reports_the_folder_that_changed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        let (tx, rx) = mpsc::channel();
        let _w = watch(std::slice::from_ref(&root), &[], SINCE_NOW, Duration::from_millis(50), move |batch| {
            let _ = tx.send(batch);
        })
        .unwrap();
        // Give the stream a moment to start before changing anything.
        std::thread::sleep(Duration::from_millis(300));

        std::fs::write(root.join("sub/new.txt"), b"x").unwrap();
        let e = wait_for(&rx, |e| e.path == root.join("sub"));
        assert!(e.id > 0);

        std::fs::remove_file(root.join("sub/new.txt")).unwrap();
        wait_for(&rx, |e| e.path == root.join("sub"));
    }

    #[test]
    fn current_event_id_increases() {
        let a = current_event_id();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("x"), b"x").unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(current_event_id() >= a);
    }
}
