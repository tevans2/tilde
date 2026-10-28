//! The Clean Up page's data: a background scan that fills in each category as it can
//! (the index-backed ones at once, then measured folders, then duplicates), reported to
//! the app as it goes. Acting on suggestions goes through `FileOps` (so it's undoable);
//! only emptying the Trash happens here, and the app asks first.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use tilde_cleanup::{Ignore, IndexedMeta, Item, Kind, Rules, dupes};
use tilde_types::EntryKind;

use crate::config::ConfigError;
use crate::index::{IndexService, home_dir};
use crate::ops::OpError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum CleanupKind {
    BuildOutput,
    BigFiles,
    OldDownloads,
    Duplicates,
    AppCaches,
    Trash,
}

impl From<CleanupKind> for Kind {
    fn from(k: CleanupKind) -> Self {
        match k {
            CleanupKind::BuildOutput => Kind::BuildOutput,
            CleanupKind::BigFiles => Kind::BigFiles,
            CleanupKind::OldDownloads => Kind::OldDownloads,
            CleanupKind::Duplicates => Kind::Duplicates,
            CleanupKind::AppCaches => Kind::AppCaches,
            CleanupKind::Trash => Kind::Trash,
        }
    }
}

const ALL: [CleanupKind; 6] = [
    CleanupKind::BuildOutput,
    CleanupKind::BigFiles,
    CleanupKind::OldDownloads,
    CleanupKind::Duplicates,
    CleanupKind::AppCaches,
    CleanupKind::Trash,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum CleanupState {
    /// Not looked at yet.
    Waiting,
    /// Measuring folders, or checking duplicates' contents: the numbers will grow.
    Working,
    Ready,
    /// Not checked this time (duplicates in Low Power Mode).
    Skipped,
}

/// A category's line on the page.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CleanupCategory {
    pub kind: CleanupKind,
    pub state: CleanupState,
    /// Space it would free (all of it, or everything but the kept copies).
    pub bytes: u64,
    pub count: u32,
    /// Duplicates' full check: bytes read so far and in all.
    pub done: u64,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CleanupItem {
    pub path: String,
    /// `~/…`.
    pub display: String,
    pub bytes: u64,
    /// Size and modification time when found (see `unchanged`).
    pub size: u64,
    pub modified: i64,
    pub last_opened: Option<i64>,
    pub is_dir: bool,
    /// Listed together: a project, an app's cache folder, a set of duplicates.
    pub group: Option<String>,
    pub detail: String,
    /// Duplicates: the copy to keep.
    pub keep: bool,
    /// Duplicates: contents compared in full.
    pub confirmed: bool,
}

/// Called on a background thread.
#[uniffi::export(with_foreign)]
pub trait CleanupListener: Send + Sync {
    fn cleanup_changed(&self, category: CleanupCategory);
}

/// A category's state, its items, and (done, total) for the content check.
type Entry = (CleanupState, Vec<Item>, (u64, u64));

#[derive(Default)]
struct State {
    categories: HashMap<CleanupKind, Entry>,
}

#[derive(uniffi::Object)]
pub struct CleanupService {
    index: Arc<IndexService>,
    listener: Arc<dyn CleanupListener>,
    state: Arc<Mutex<State>>,
    cancel: Mutex<Arc<AtomicBool>>,
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn display(home: &Path, path: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rel) if rel.as_os_str().is_empty() => "~".into(),
        Ok(rel) => format!("~/{}", rel.to_string_lossy()),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

/// Lowers the current thread's priority, so the scan doesn't compete with what you're
/// doing (QOS_CLASS_UTILITY).
fn background_priority() {
    unsafe extern "C" {
        fn pthread_set_qos_class_self_np(qos: u32, relative: i32) -> i32;
    }
    unsafe { pthread_set_qos_class_self_np(0x11, 0) };
}

struct Scan {
    state: Arc<Mutex<State>>,
    listener: Arc<dyn CleanupListener>,
    cancel: Arc<AtomicBool>,
}

impl Scan {
    fn set(&self, kind: CleanupKind, state: CleanupState, items: Option<Vec<Item>>, progress: Option<(u64, u64)>) {
        let summary = {
            let mut s = self.state.lock().unwrap();
            let entry = s.categories.entry(kind).or_insert((CleanupState::Waiting, Vec::new(), (0, 0)));
            entry.0 = state;
            if let Some(items) = items {
                entry.1 = items;
            }
            if let Some(p) = progress {
                entry.2 = p;
            }
            summarize(kind, entry)
        };
        if !self.cancel.load(Ordering::Relaxed) {
            self.listener.cleanup_changed(summary);
        }
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

fn summarize(kind: CleanupKind, (state, items, (done, total)): &Entry) -> CleanupCategory {
    let removable = items.iter().filter(|i| !i.keep);
    CleanupCategory {
        kind,
        state: *state,
        bytes: removable.clone().map(|i| i.bytes).sum(),
        count: removable.count() as u32,
        done: *done,
        total: *total,
    }
}

#[uniffi::export]
impl CleanupService {
    #[uniffi::constructor]
    pub fn new(index: Arc<IndexService>, listener: Arc<dyn CleanupListener>) -> Arc<Self> {
        Arc::new(CleanupService {
            index,
            listener,
            state: Arc::default(),
            cancel: Mutex::new(Arc::new(AtomicBool::new(false))),
        })
    }

    /// Looks again at everything, in the background (replacing a scan in progress).
    /// `duplicates: false` skips the slow content check (Low Power Mode).
    pub fn scan(&self, duplicates: bool) {
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut current = self.cancel.lock().unwrap();
            current.store(true, Ordering::SeqCst);
            *current = cancel.clone();
        }
        {
            let mut s = self.state.lock().unwrap();
            s.categories.clear();
            for kind in ALL {
                s.categories.insert(kind, (CleanupState::Waiting, Vec::new(), (0, 0)));
            }
        }
        let scan = Scan { state: self.state.clone(), listener: self.listener.clone(), cancel };
        let handle = self.index.handle();
        std::thread::Builder::new()
            .name("tilde-cleanup".into())
            .spawn(move || {
                background_priority();
                run(scan, handle, duplicates);
            })
            .expect("spawn cleanup thread");
    }

    pub fn cancel(&self) {
        self.cancel.lock().unwrap().store(true, Ordering::SeqCst);
    }

    pub fn categories(&self) -> Vec<CleanupCategory> {
        let s = self.state.lock().unwrap();
        ALL.iter().filter_map(|k| s.categories.get(k).map(|e| summarize(*k, e))).collect()
    }

    /// A category's items, biggest first (duplicates and build output grouped).
    pub fn items(&self, kind: CleanupKind) -> Vec<CleanupItem> {
        let home = home_dir();
        let s = self.state.lock().unwrap();
        let Some((_, items, _)) = s.categories.get(&kind) else { return Vec::new() };
        items
            .iter()
            .map(|i| CleanupItem {
                path: i.path.to_string_lossy().into_owned(),
                display: display(&home, &i.path),
                bytes: i.bytes,
                size: i.size,
                modified: i.modified,
                last_opened: i.last_opened,
                is_dir: i.is_dir,
                group: i.group.clone(),
                detail: i.detail.clone(),
                keep: i.keep,
                confirmed: i.confirmed,
            })
            .collect()
    }

    /// Drops suggestions whose files are gone (trashed from the page, or elsewhere), and
    /// updates the totals.
    pub fn forget_missing(&self) {
        let kinds: Vec<(CleanupKind, CleanupCategory)> = {
            let mut s = self.state.lock().unwrap();
            for (_, items, _) in s.categories.values_mut() {
                items.retain(|i| i.path.symlink_metadata().is_ok());
                // A duplicate set down to one copy isn't a set any more.
                let mut counts: HashMap<String, usize> = HashMap::new();
                for i in items.iter() {
                    if let Some(g) = &i.group {
                        *counts.entry(g.clone()).or_default() += 1;
                    }
                }
                items.retain(|i| !(i.keep && i.group.as_ref().is_some_and(|g| counts[g] < 2)));
            }
            ALL.iter().filter_map(|k| s.categories.get(k).map(|e| (*k, summarize(*k, e)))).collect()
        };
        for (_, summary) in kinds {
            self.listener.cleanup_changed(summary);
        }
    }

    /// Empties the Trash for good. Returns the bytes freed.
    pub fn empty_trash(&self) -> Result<u64, OpError> {
        let freed = tilde_cleanup::empty_trash(&home_dir()).map_err(|e| OpError::Invalid { message: e.to_string() })?;
        self.forget_missing();
        Ok(freed)
    }

    /// Whether `path` still has the size and date it was suggested with.
    pub fn unchanged(&self, path: String, size: u64, modified: i64) -> bool {
        tilde_cleanup::unchanged(Path::new(&path), size, modified)
    }

    /// Whether two files are byte for byte the same (before trashing a duplicate).
    pub fn same_contents(&self, a: String, b: String) -> bool {
        dupes::same_contents(Path::new(&a), Path::new(&b)).unwrap_or(false)
    }

    /// Never suggests `path` again (`[cleanup] ignore`), and drops it now.
    pub fn ignore(&self, path: String) -> Result<(), ConfigError> {
        let home = home_dir();
        let entry = display(&home, Path::new(&path));
        tilde_config::add_to_list(&tilde_config::config_path(), "cleanup", "ignore", &entry)
            .map_err(|e| ConfigError::Io { message: e.to_string() })?;
        let p = PathBuf::from(&path);
        {
            let mut s = self.state.lock().unwrap();
            for (_, items, _) in s.categories.values_mut() {
                items.retain(|i| !i.path.starts_with(&p));
            }
        }
        self.forget_missing();
        Ok(())
    }
}

fn rules(home: &Path) -> Rules {
    let config = tilde_config::load(&tilde_config::config_path()).config.cleanup;
    let (r, _) = config.rules();
    Rules {
        big_file: r.big_file,
        big_file_age: r.big_file_age,
        download_age: r.download_age,
        project_idle: r.project_idle,
        duplicates_min: r.duplicates_min,
        build_folders: r.build_folders,
        ignore: Ignore::new(&r.ignore, home),
    }
}

fn meta(m: &tilde_index::ItemMeta) -> IndexedMeta {
    IndexedMeta { kind: m.kind, size: m.size, modified: m.modified }
}

fn run(scan: Scan, handle: Option<tilde_index::IndexHandle>, duplicates: bool) {
    use CleanupKind::*;
    use CleanupState::*;
    let home = home_dir();
    let rules = rules(&home);
    let now = now();
    let cancel = scan.cancel.clone();

    // From the index: one pass for big files and duplicate candidates, project markers.
    let mut big = Vec::new();
    let mut dup_paths = Vec::new();
    let mut projects = HashSet::new();
    if let Some(h) = &handle {
        for (path, m) in h.select(|path, m| {
            m.kind == EntryKind::File && (m.size >= rules.duplicates_min.min(rules.big_file) || is_marker(path))
                || is_marker(path)
        }) {
            if let Some(dir) = tilde_cleanup::project_of(&path, m.kind) {
                projects.insert(dir);
            }
            if m.kind == EntryKind::File && m.size >= rules.big_file {
                big.push((path.clone(), meta(&m)));
            }
            if m.kind == EntryKind::File
                && m.size >= rules.duplicates_min
                && !hidden_or_packaged(&path, &home)
                && !rules.ignore.matches(&path)
            {
                dup_paths.push(path);
            }
        }
    }
    scan.set(BigFiles, Ready, Some(tilde_cleanup::big_files(&big, &rules, &home, now)), None);
    if scan.cancelled() {
        return;
    }
    let inbox = tilde_config::load(&tilde_config::config_path()).config.activity.inbox_paths(&home);
    let inbox: Vec<PathBuf> = inbox.into_iter().filter(|p| p.is_dir()).collect();
    scan.set(OldDownloads, Working, None, None);
    scan.set(OldDownloads, Ready, Some(tilde_cleanup::old_downloads(&inbox, &rules, now, &cancel)), None);
    scan.set(Trash, Working, None, None);
    scan.set(Trash, Ready, Some(tilde_cleanup::trash(&home, &cancel)), None);
    if scan.cancelled() {
        return;
    }

    // Build output: projects' newest changes from a second pass, then measure.
    scan.set(BuildOutput, Working, None, None);
    let mut newest = HashMap::new();
    if let Some(h) = &handle {
        h.select(|path, m| {
            tilde_cleanup::note_change(&projects, &mut newest, &home, path, m.modified);
            false
        });
    }
    let idle = tilde_cleanup::idle_projects_from(&projects, &newest, &rules, &home, now);
    let mut build = Vec::new();
    for project in idle {
        for folder in &project.build {
            if scan.cancelled() {
                return;
            }
            let (bytes, _) = tilde_cleanup::measure(folder, &cancel);
            let Some(s) = tilde_cleanup::stat(folder) else { continue };
            build.push(Item {
                path: folder.clone(),
                bytes,
                size: s.size,
                modified: s.modified,
                // For build output: when the project last changed.
                last_opened: Some(project.last_change),
                is_dir: true,
                group: Some(display(&home, &project.dir)),
                detail: String::new(),
                keep: false,
                confirmed: true,
            });
        }
        build.sort_by_key(|i| std::cmp::Reverse(i.bytes));
        scan.set(BuildOutput, Working, Some(build.clone()), None);
    }
    scan.set(BuildOutput, Ready, Some(build), None);

    scan.set(AppCaches, Working, None, None);
    scan.set(AppCaches, Ready, Some(tilde_cleanup::app_caches(&home, &rules, &cancel)), None);
    if scan.cancelled() {
        return;
    }

    // Duplicates, in stages: likely groups first, then confirmed ones.
    if !duplicates {
        scan.set(Duplicates, Skipped, Some(Vec::new()), None);
        return;
    }
    scan.set(Duplicates, Working, None, None);
    let cache_path = home.join("Library/Caches/Tilde/hashes.json");
    let mut cache = dupes::Cache::load(&cache_path);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(6)
        .start_handler(|_| background_priority())
        .build()
        .expect("thread pool");
    let progress = Mutex::new((0u64, 0u64));
    let to_items = |groups: &[dupes::Group]| -> Vec<Item> {
        groups
            .iter()
            .enumerate()
            .flat_map(|(n, g)| {
                g.files.iter().enumerate().map(move |(i, f)| Item {
                    path: f.path.clone(),
                    bytes: f.freed,
                    size: g.size,
                    modified: f.modified,
                    last_opened: None,
                    is_dir: false,
                    group: Some(format!("dup-{n}")),
                    detail: String::new(),
                    keep: i == g.keep,
                    confirmed: g.confirmed,
                })
            })
            .collect()
    };
    pool.install(|| {
        dupes::find(
            dup_paths,
            rules.duplicates_min,
            &mut cache,
            &cancel,
            &|groups| {
                let p = *progress.lock().unwrap();
                let state = if groups.iter().all(|g| g.confirmed) && p.0 >= p.1 { Ready } else { Working };
                scan.set(Duplicates, state, Some(to_items(groups)), Some(p));
            },
            &|p| {
                *progress.lock().unwrap() = (p.done, p.total);
                scan.set(Duplicates, Working, None, Some((p.done, p.total)));
            },
        )
    });
    if !scan.cancelled() {
        cache.save(&cache_path);
        let p = *progress.lock().unwrap();
        scan.set(Duplicates, Ready, None, Some(p));
    }
}

fn is_marker(path: &Path) -> bool {
    tilde_cleanup::project_of(path, EntryKind::File).is_some()
        || tilde_cleanup::project_of(path, EntryKind::Folder).is_some()
}

fn hidden_or_packaged(path: &Path, home: &Path) -> bool {
    let rel = path.strip_prefix(home).unwrap_or(path);
    let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    let folders = &parts[..parts.len().saturating_sub(1)];
    parts.iter().any(|p| p.starts_with('.')) || folders.iter().any(|p| tilde_fs::is_package_name(p))
}

/// A list row for any path (the Clean Up page's preview), without listing its folder.
#[uniffi::export]
pub fn row_for_path(path: String) -> Option<crate::views::RowView> {
    use std::os::unix::fs::MetadataExt;
    let p = PathBuf::from(&path);
    let m = p.symlink_metadata().ok()?;
    let name = p.file_name()?.to_string_lossy().into_owned();
    let kind = if m.file_type().is_symlink() {
        EntryKind::Symlink
    } else if m.is_dir() && tilde_fs::is_package_name(&name) {
        EntryKind::Package
    } else if m.is_dir() {
        EntryKind::Folder
    } else {
        EntryKind::File
    };
    let entry = tilde_types::Entry {
        hidden: name.starts_with('.'),
        name,
        kind,
        size: if m.is_dir() { 0 } else { m.len() },
        modified: m.mtime(),
        file_id: m.ino(),
        child_count: None,
        mount_point: false,
        added: 0,
    };
    let hit = tilde_query::Hit { index: 0, score: 0, indices: Vec::new() };
    Some(crate::views::RowView::here(p.parent()?, &entry, &hit, false, false))
}
