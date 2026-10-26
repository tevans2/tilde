//! One index shared by every window, plus the controls the Index window needs.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use tilde_config::IndexList;
use tilde_index::{GlobalHit, GlobalIndex, IndexConfig, IndexHandle, ScanConfig};
use tilde_query::Query;

use crate::config::ConfigError;
use crate::engine::Command;
use crate::views::{HighlightRange, utf16_ranges};

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum IndexState {
    /// `[index] enabled = false` in the config.
    Disabled,
    /// Cleared from the Index window; empty until rebuilt.
    Cleared,
    Scanning,
    Ready,
}

/// Cheap enough to poll for the status bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct IndexStatus {
    pub state: IndexState,
    pub items: u64,
    pub capped: bool,
}

/// Everything the Index window shows.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct IndexInfo {
    pub status: IndexStatus,
    pub max_items: u64,
    pub memory_bytes: u64,
    /// Size of the saved index, if there is one.
    pub disk_bytes: Option<u64>,
    pub cache_path: String,
    /// Indexed folders, for display (`~`).
    pub roots: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FolderInfo {
    pub path: String,
    /// `~/Developer/vendor`
    pub display: String,
    pub items: u64,
}

/// A folder to move or copy into.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Destination {
    pub path: String,
    /// `~/Documents/Finance`
    pub display: String,
    /// Matched characters in `display`.
    pub highlights: Vec<HighlightRange>,
}

struct Live {
    index: Option<GlobalIndex>,
    handle: Option<IndexHandle>,
    state: IndexState,
    record_visits: bool,
    roots: Vec<PathBuf>,
}

#[derive(uniffi::Object)]
pub struct IndexService {
    home: PathBuf,
    /// Held while searching; rebuild swaps the index under it.
    live: Mutex<Live>,
    /// Engines to tell when the index changes. Dead ones are dropped on the next send.
    subscribers: Arc<Mutex<Vec<Sender<Command>>>>,
    /// `None` in tests: settings come from the given config instead of the file.
    fixed_config: Option<(IndexConfig, bool)>,
}

#[uniffi::export]
impl IndexService {
    /// Starts the index from the user's config: loads the saved index or scans.
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        let service = Arc::new(IndexService {
            home: home_dir(),
            live: Mutex::new(Live {
                index: None,
                handle: None,
                state: IndexState::Disabled,
                record_visits: false,
                roots: Vec::new(),
            }),
            subscribers: Arc::new(Mutex::new(Vec::new())),
            fixed_config: None,
        });
        service.start(false);
        service
    }

    pub fn status(&self) -> IndexStatus {
        let live = self.live.lock().unwrap();
        status_of(&live)
    }

    pub fn info(&self) -> IndexInfo {
        let (status, handle, roots) = {
            let live = self.live.lock().unwrap();
            (status_of(&live), live.handle.clone(), live.roots.clone())
        };
        IndexInfo {
            status,
            max_items: handle.as_ref().map_or(0, |h| h.max_items()),
            memory_bytes: handle.as_ref().map_or(0, |h| h.memory_estimate()),
            disk_bytes: handle.as_ref().and_then(|h| h.disk_size()).or_else(|| file_size(&cache_path())),
            cache_path: cache_path().to_string_lossy().into_owned(),
            roots: roots.iter().map(|r| display_path(&self.home, r)).collect(),
        }
    }

    /// Folders holding the most items: good candidates to exclude.
    pub fn largest_folders(&self, limit: u32) -> Vec<FolderInfo> {
        let Some(handle) = self.live.lock().unwrap().handle.clone() else { return Vec::new() };
        handle
            .largest_folders(limit as usize)
            .into_iter()
            .map(|f| {
                let path = handle.absolute(&f.key);
                FolderInfo {
                    display: display_path(&self.home, &path),
                    path: path.to_string_lossy().into_owned(),
                    items: f.items,
                }
            })
            .collect()
    }

    /// Folders for the move/copy picker. A typed path (`~/…` or `/…`) to an existing folder
    /// comes first; an empty query lists zoxide's most-visited folders; anything else is a
    /// fuzzy search over indexed folders.
    pub fn destinations(&self, query: String, limit: u32) -> Vec<Destination> {
        self.lookup(&query, limit as usize, true)
    }

    /// Like `destinations`, with files too (the file-dialog helper).
    pub fn find(&self, query: String, limit: u32) -> Vec<Destination> {
        self.lookup(&query, limit as usize, false)
    }
}

impl IndexService {
    fn lookup(&self, query: &str, limit: usize, folders_only: bool) -> Vec<Destination> {
        let q = query.trim();
        let mut out = Vec::new();
        if q.starts_with('/') || q.starts_with('~') {
            out = self.complete_path(q, limit);
            if !out.is_empty() {
                return out;
            }
        }
        let mut live = self.live.lock().unwrap();
        let Some(index) = live.index.as_mut() else { return out };
        if q.is_empty() {
            for key in index.top_folders(limit) {
                let path = index.absolute(&key);
                out.push(Destination {
                    display: display_path(&self.home, &path),
                    path: path.to_string_lossy().into_owned(),
                    highlights: Vec::new(),
                });
            }
            return out;
        }
        let query = Query::parse(&if folders_only { format!("{q} kind:folder") } else { q.to_string() });
        for hit in index.search(&query, None, None, 0, limit) {
            let path = index.absolute(&hit.key);
            let display = display_path(&self.home, &path);
            // Index keys are relative to home; the display adds "~/".
            let shift = if hit.key.starts_with('/') { 0 } else { 2 };
            let indices: Vec<u32> = hit.indices.iter().map(|i| i + shift).collect();
            let path = path.to_string_lossy().into_owned();
            if out.iter().any(|d: &Destination| d.path == path) {
                continue;
            }
            out.push(Destination { highlights: utf16_ranges(&display, &indices), display, path });
        }
        out.truncate(limit.max(1));
        out
    }
}

#[uniffi::export]
impl IndexService {
    /// Re-reads the config and builds the index from scratch.
    pub fn rebuild(&self) {
        self.start(true);
    }

    /// Deletes the saved index and empties the in-memory one until `rebuild`.
    pub fn clear(&self) {
        let old = {
            let mut live = self.live.lock().unwrap();
            if let Some(h) = &live.handle {
                let _ = h.delete_saved();
            }
            live.handle = None;
            live.state = IndexState::Cleared;
            live.index.take()
        };
        let _ = std::fs::remove_file(cache_path());
        drop(old);
        self.notify();
    }

    /// Saves now (called when the app quits).
    pub fn save(&self) {
        let handle = self.live.lock().unwrap().handle.clone();
        if let Some(h) = handle {
            let _ = h.save();
        }
    }

    /// Adds `path` to the config and rebuilds. `hide`: leave the item out entirely;
    /// otherwise skip a folder's contents but keep the folder itself.
    pub fn add_exclusion(&self, path: String, hide: bool) -> Result<(), ConfigError> {
        let entry = display_path(&self.home, Path::new(&path));
        let list = if hide { IndexList::Hide } else { IndexList::Exclude };
        tilde_config::add_index_entry(&tilde_config::config_path(), list, &entry)
            .map_err(|e| ConfigError::Io { message: e.to_string() })?;
        self.rebuild();
        Ok(())
    }
}

impl IndexService {
    /// A service with fixed settings (tests).
    #[cfg(test)]
    pub(crate) fn with_config(config: IndexConfig, record_visits: bool) -> Arc<Self> {
        let service = Arc::new(IndexService {
            home: config.home.clone(),
            live: Mutex::new(Live {
                index: None,
                handle: None,
                state: IndexState::Disabled,
                record_visits: false,
                roots: Vec::new(),
            }),
            subscribers: Arc::new(Mutex::new(Vec::new())),
            fixed_config: Some((config, record_visits)),
        });
        service.start(false);
        service
    }

    /// Completes a typed path: the folder itself if it exists, then folders inside the
    /// parent whose names start with the last part (case-insensitive). A trailing `/` lists
    /// the folder's subfolders.
    fn complete_path(&self, typed: &str, limit: usize) -> Vec<Destination> {
        let path = match typed.strip_prefix('~') {
            Some(rest) => self.home.join(rest.trim_start_matches('/')),
            None => PathBuf::from(typed),
        };
        let to_dest = |p: &Path| Destination {
            display: display_path(&self.home, p),
            path: p.to_string_lossy().into_owned(),
            highlights: Vec::new(),
        };
        let mut out = Vec::new();
        let (dir, prefix) = if typed.ends_with('/') || typed == "~" {
            (path.clone(), String::new())
        } else {
            if path.is_dir() {
                out.push(to_dest(&path));
            }
            let prefix = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            (path.parent().map(Path::to_path_buf).unwrap_or_default(), prefix)
        };
        if dir.is_dir() {
            if prefix.is_empty() {
                out.push(to_dest(&dir));
            }
            if let Ok(mut entries) = tilde_fs::list_dir(&dir, &tilde_fs::ListOptions::default(), || false) {
                tilde_fs::sort_entries(&mut entries);
                for e in entries {
                    if out.len() >= limit {
                        break;
                    }
                    let child = dir.join(&e.name);
                    if e.kind == tilde_types::EntryKind::Folder
                        && e.name.to_lowercase().starts_with(&prefix)
                        && child != path
                    {
                        out.push(to_dest(&child));
                    }
                }
            }
        }
        out
    }

    pub(crate) fn subscribe(&self, tx: Sender<Command>) {
        self.subscribers.lock().unwrap().push(tx);
    }

    fn notify(&self) {
        notify(&self.subscribers);
    }

    /// (Re)starts the index. `fresh` discards the saved index first.
    fn start(&self, fresh: bool) {
        let (config, record_visits) = match &self.fixed_config {
            Some((c, r)) => (Some(c.clone()), *r),
            None => config_from_file(&self.home),
        };
        // Drop the old index outside the lock (its threads wind down on their own).
        let old = self.live.lock().unwrap().index.take();
        if let Some(old) = &old {
            let _ = old.handle().delete_saved();
        }
        drop(old);
        if fresh && let Some(path) = config.as_ref().and_then(|c| c.cache_path.clone()) {
            let _ = std::fs::remove_file(path);
        }

        let mut live = self.live.lock().unwrap();
        live.record_visits = record_visits;
        match config {
            Some(config) => {
                live.roots = config.scan.roots.clone();
                let subscribers = self.subscribers.clone();
                let index = GlobalIndex::start(config, move || notify(&subscribers));
                live.state = if index.status().scanning { IndexState::Scanning } else { IndexState::Ready };
                live.handle = Some(index.handle());
                live.index = Some(index);
            }
            None => {
                live.roots = Vec::new();
                live.handle = None;
                live.state = IndexState::Disabled;
            }
        }
        drop(live);
        self.notify();
    }

    /// Searches the index (empty when disabled or cleared).
    /// `folder` is left to the Here scope; `None` (Activity) searches everything.
    pub(crate) fn search(&self, query: &Query, folder: Option<&Path>, now: i64, limit: usize) -> Vec<GlobalHit> {
        let mut live = self.live.lock().unwrap();
        let Some(index) = live.index.as_mut() else { return Vec::new() };
        let cwd = folder.map(|f| index.key(f));
        index.search(query, cwd.as_deref(), cwd.as_deref(), now, limit)
    }

    /// Visible files modified since `since`, newest first, for the Activity feed.
    pub(crate) fn recent(&self, since: i64, limit: usize) -> Vec<tilde_activity::Candidate> {
        let Some(handle) = self.live.lock().unwrap().handle.clone() else { return Vec::new() };
        handle
            .recent(since, limit)
            .into_iter()
            .map(|(path, meta)| tilde_activity::Candidate {
                path,
                kind: meta.kind,
                size: meta.size,
                modified: meta.modified,
                child_count: meta.child_count,
            })
            .collect()
    }

    pub(crate) fn record_visit(&self, path: &Path) {
        let live = self.live.lock().unwrap();
        if live.record_visits
            && let Some(index) = &live.index
        {
            index.record_visit(path);
        }
    }

    pub(crate) fn home(&self) -> &Path {
        &self.home
    }
}

fn status_of(live: &Live) -> IndexStatus {
    match &live.handle {
        Some(h) => {
            let s = h.status();
            let state = if s.scanning { IndexState::Scanning } else { IndexState::Ready };
            IndexStatus { state, items: s.items, capped: s.capped }
        }
        None => IndexStatus { state: live.state, items: 0, capped: false },
    }
}

fn notify(subscribers: &Mutex<Vec<Sender<Command>>>) {
    subscribers.lock().unwrap().retain(|tx| tx.send(Command::IndexChanged).is_ok());
}

/// Index settings from the config file, or `None` if indexing is disabled. Also returns
/// whether visits should be recorded with zoxide.
fn config_from_file(home: &Path) -> (Option<IndexConfig>, bool) {
    let config = tilde_config::load(&tilde_config::config_path()).config;
    let record_visits = config.zoxide.enabled && config.zoxide.add_on_visit;
    if !config.index.enabled {
        return (None, record_visits);
    }
    let r = config.index.resolve(home);
    let index = IndexConfig {
        home: home.to_path_buf(),
        scan: ScanConfig {
            roots: r.roots,
            exclude_names: r.exclude_names.into_iter().collect(),
            exclude_paths: r.exclude_paths,
            hide_names: r.hide_names,
            hide_paths: r.hide_paths,
            include_hidden: config.index.include_hidden,
        },
        zoxide: config.zoxide.enabled,
        max_items: config.index.max_items,
        cache_path: Some(cache_path()),
    };
    (Some(index), record_visits)
}

/// `~/Library/Caches/Tilde/index.bin`. Inside `~/Library`, which isn't indexed, so saving
/// never shows up as a change.
fn cache_path() -> PathBuf {
    home_dir().join("Library/Caches/Tilde/index.bin")
}

pub(crate) fn home_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()))
}

fn display_path(home: &Path, path: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rel) if rel.as_os_str().is_empty() => "~".into(),
        Ok(rel) => format!("~/{}", rel.to_string_lossy()),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

fn file_size(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|m| m.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completes_typed_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        for d in ["Archive", "Artwork", "Beta"] {
            std::fs::create_dir(root.join(d)).unwrap();
        }
        std::fs::write(root.join("art.txt"), b"").unwrap();
        let config = IndexConfig {
            home: PathBuf::from("/nonexistent-home"),
            scan: ScanConfig::default(),
            zoxide: false,
            max_items: 10,
            cache_path: None,
        };
        let service = IndexService::with_config(config, false);
        let paths = |typed: String| -> Vec<String> {
            service
                .destinations(typed, 10)
                .into_iter()
                .map(|d| d.path.rsplit('/').next().unwrap().to_string())
                .collect()
        };
        let r = root.to_string_lossy();
        assert_eq!(paths(format!("{r}/ar")), ["Archive", "Artwork"], "folders only, by prefix");
        assert_eq!(paths(format!("{r}/Archive")), ["Archive"]);
        let all = paths(format!("{r}/"));
        assert_eq!(all[1..], ["Archive", "Artwork", "Beta"]);
        assert!(paths(format!("{r}/zzz")).is_empty());
    }
}
