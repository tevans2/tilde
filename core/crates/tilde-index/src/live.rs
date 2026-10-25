//! The index's mutable side. Nucleo only appends, so every item gets an ID: removed
//! and changed items are marked dead (search skips them) and changed items are pushed
//! again. A compact per-folder table of what's indexed lets a fresh listing of any
//! changed folder be diffed and applied.

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard};

use nucleo::{Injector, Utf32String};
use tilde_fs::{FsEvent, ListOptions, is_package_name, list_dir};
use tilde_types::{Entry, EntryKind};

use crate::walk::{ScanConfig, walk_sequential};
use crate::{ItemMeta, key_for};

/// What the index holds for one entry of a folder.
struct Child {
    name_hash: u64,
    /// Changes when the entry's size or modification time does (always 0 for folders,
    /// whose times change with every edit inside them).
    meta_hash: u64,
    id: u32,
    /// Kept for folders only, to find their own table when they disappear.
    dir_name: Option<Box<str>>,
}

pub(crate) struct Shared {
    pub home: PathBuf,
    pub scan: ScanConfig,
    pub max_items: u64,
    /// Set when `max_items` is reached; adding stops.
    pub capped: AtomicBool,
    /// Stops a scan in progress (on drop, or when capped).
    pub stop: Arc<AtomicBool>,
    /// The FSEvents ID the index is current to, for resuming after a restart.
    pub last_event_id: AtomicU64,
    /// Changed since the last save.
    pub dirty: AtomicBool,
    /// Where to save and the settings fingerprint to save with.
    pub cache: Option<(PathBuf, u64)>,
    /// Set after the saved index is deleted, so it isn't written back.
    pub saving_disabled: AtomicBool,
    save_lock: Mutex<()>,
    injector: Injector<ItemMeta>,
    next_id: AtomicU32,
    killed: AtomicU32,
    dead: RwLock<Vec<u64>>,
    /// Folder key → its indexed entries.
    children: Mutex<HashMap<String, Vec<Child>>>,
}

/// One folder and how many items the index holds under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderSize {
    /// Index key (relative to home, or absolute).
    pub key: String,
    pub items: u64,
}

/// Dead item IDs, read-locked for the duration of a search.
pub(crate) struct DeadSet<'a>(RwLockReadGuard<'a, Vec<u64>>);

impl DeadSet<'_> {
    pub fn contains(&self, id: u32) -> bool {
        self.0.get(id as usize / 64).is_some_and(|w| w & (1 << (id % 64)) != 0)
    }
}

impl Shared {
    pub fn new(
        home: PathBuf,
        scan: ScanConfig,
        max_items: u64,
        cache: Option<(PathBuf, u64)>,
        injector: Injector<ItemMeta>,
    ) -> Shared {
        Shared {
            home,
            scan,
            max_items,
            capped: AtomicBool::new(false),
            stop: Arc::new(AtomicBool::new(false)),
            last_event_id: AtomicU64::new(0),
            dirty: AtomicBool::new(false),
            cache,
            saving_disabled: AtomicBool::new(false),
            save_lock: Mutex::new(()),
            injector,
            next_id: AtomicU32::new(0),
            killed: AtomicU32::new(0),
            dead: RwLock::new(Vec::new()),
            children: Mutex::new(HashMap::new()),
        }
    }

    pub fn key(&self, path: &Path) -> String {
        key_for(&self.home, path)
    }

    /// Items currently searchable.
    pub fn live_items(&self) -> u64 {
        self.injector.injected_items() as u64 - self.killed.load(Ordering::Relaxed) as u64
    }

    pub fn dead(&self) -> DeadSet<'_> {
        DeadSet(self.dead.read().unwrap())
    }

    /// Live files and packages modified at or after `since`, newest first, at most
    /// `limit`. Skips anything hidden, inside a hidden folder, or inside a package (a
    /// Photos library's internals aren't activity). Library packages themselves (Photos,
    /// Music) change constantly and are skipped too.
    pub fn recent(&self, since: i64, limit: usize) -> Vec<(String, ItemMeta)> {
        let dead = self.dead();
        let mut found: Vec<(String, ItemMeta)> = Vec::new();
        for i in 0..self.injector.injected_items() {
            let Some(item) = self.injector.get(i) else { continue };
            let meta = *item.data;
            if meta.modified < since
                || !matches!(meta.kind, EntryKind::File | EntryKind::Package)
                || dead.contains(meta.id)
            {
                continue;
            }
            let key = item.matcher_columns[0].to_string();
            let mut folders = key.split('/');
            folders.next_back();
            let library = meta.kind == EntryKind::Package && key.ends_with("library");
            if library || key.split('/').any(|part| part.starts_with('.')) || folders.any(is_package_name) {
                continue;
            }
            found.push((key, meta));
        }
        found.sort_by_key(|(_, m)| std::cmp::Reverse(m.modified));
        found.truncate(limit);
        found
    }

    /// Adds an item and returns its ID, or `None` once the index is full.
    pub fn push(&self, key: &str, mut meta: ItemMeta) -> Option<u32> {
        if self.live_items() >= self.max_items {
            if !self.capped.swap(true, Ordering::SeqCst) {
                self.stop.store(true, Ordering::SeqCst);
            }
            return None;
        }
        meta.id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.injector.push(meta, |_, cols| cols[0] = Utf32String::from(key));
        Some(meta.id)
    }

    fn kill(&self, id: u32) {
        let mut dead = self.dead.write().unwrap();
        let word = id as usize / 64;
        if dead.len() <= word {
            dead.resize(word + 1, 0);
        }
        dead[word] |= 1 << (id % 64);
        self.killed.fetch_add(1, Ordering::Relaxed);
    }

    /// Diffs a fresh listing of `dir` against the index and applies it. Returns the
    /// subfolders to list next: new ones, or all of them when `rescan`.
    pub fn sync_dir(&self, dir: &Path, entries: &[Entry], rescan: bool) -> Vec<PathBuf> {
        let key = self.key(dir);
        let mut tables = self.children.lock().unwrap();
        let mut old: HashMap<u64, Child> =
            tables.remove(&key).unwrap_or_default().into_iter().map(|c| (c.name_hash, c)).collect();
        let mut table = Vec::with_capacity(entries.len());
        let mut next = Vec::new();

        for e in entries {
            if self.scan.hides(&dir.join(&e.name), &e.name) {
                continue;
            }
            let name_hash = hash(&e.name);
            let is_folder = e.kind == EntryKind::Folder && !e.mount_point;
            let meta_hash = if e.kind == EntryKind::Folder { 0 } else { hash(&(e.kind as u8, e.size, e.modified)) };
            let path = dir.join(&e.name);
            let descend = is_folder && self.scan.descends_into(&path, &e.name);
            let id = match old.remove(&name_hash) {
                Some(c) if c.meta_hash == meta_hash => {
                    if rescan && descend {
                        next.push(path);
                    }
                    c.id
                }
                existing => {
                    let is_new = existing.is_none();
                    if let Some(c) = existing {
                        self.kill(c.id);
                    }
                    let item_key = if key.is_empty() { e.name.clone() } else { format!("{key}/{}", e.name) };
                    let meta = ItemMeta {
                        id: 0,
                        kind: e.kind,
                        size: e.size,
                        modified: e.modified,
                        child_count: e.child_count,
                        extra: false,
                    };
                    let Some(id) = self.push(&item_key, meta) else { continue };
                    if descend && (is_new || rescan) {
                        next.push(path);
                    }
                    id
                }
            };
            table.push(Child { name_hash, meta_hash, id, dir_name: is_folder.then(|| e.name.as_str().into()) });
        }

        // Whatever wasn't in the listing is gone.
        for (_, c) in old {
            self.kill(c.id);
            if let Some(name) = c.dir_name {
                let child_key = if key.is_empty() { name.to_string() } else { format!("{key}/{name}") };
                self.remove_subtree(&mut tables, &child_key);
            }
        }
        tables.insert(key, table);
        next
    }

    /// Marks everything under folder `key` dead (not the folder's own item).
    fn remove_subtree(&self, tables: &mut HashMap<String, Vec<Child>>, key: &str) {
        let Some(table) = tables.remove(key) else { return };
        for c in table {
            self.kill(c.id);
            if let Some(name) = c.dir_name {
                self.remove_subtree(tables, &format!("{key}/{name}"));
            }
        }
    }

    /// Applies a batch of FSEvents: re-lists each changed folder the index covers.
    pub fn apply(&self, events: &[FsEvent]) {
        if let Some(max) = events.iter().map(|e| e.id).max() {
            self.last_event_id.fetch_max(max, Ordering::SeqCst);
        }
        self.dirty.store(true, Ordering::SeqCst);
        let mut done = HashSet::new();
        for e in events {
            if e.is_history_done() || !self.scan.scans_inside(&e.path) {
                continue;
            }
            let rescan = e.needs_rescan();
            if done.insert((e.path.clone(), rescan)) {
                self.refresh_dir(&e.path, rescan);
            }
        }
    }

    fn refresh_dir(&self, dir: &Path, rescan: bool) {
        let opts = ListOptions { include_hidden: self.scan.include_hidden, ..Default::default() };
        match list_dir(dir, &opts, || false) {
            Ok(entries) => {
                let next = self.sync_dir(dir, &entries, rescan);
                walk_sequential(next, self.scan.include_hidden, &|d, es| self.sync_dir(d, &es, rescan));
            }
            // The folder itself is gone; its parent's event removes its own item.
            Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory) => {
                let key = self.key(dir);
                let mut tables = self.children.lock().unwrap();
                self.remove_subtree(&mut tables, &key);
            }
            Err(_) => {}
        }
    }

    /// Adds saved items, rebuilding the per-folder tables.
    pub fn load(&self, items: Vec<(String, ItemMeta)>) {
        let mut tables = self.children.lock().unwrap();
        for (key, meta) in items {
            let Some(id) = self.push(&key, meta) else { break };
            let (parent, name) = key.rsplit_once('/').unwrap_or(("", &key));
            let parent = if parent.is_empty() && key.starts_with('/') { "/" } else { parent };
            let meta_hash =
                if meta.kind == EntryKind::Folder { 0 } else { hash(&(meta.kind as u8, meta.size, meta.modified)) };
            let dir_name = (meta.kind == EntryKind::Folder).then(|| name.into());
            tables.entry(parent.to_string()).or_default().push(Child {
                name_hash: hash(&name),
                meta_hash,
                id,
                dir_name,
            });
        }
    }

    /// Writes the index to its cache file. `None` if saving is off.
    pub fn save(&self) -> Option<std::io::Result<u64>> {
        let (path, fingerprint) = self.cache.as_ref()?;
        if self.saving_disabled.load(Ordering::SeqCst) {
            return None;
        }
        let _guard = self.save_lock.lock().unwrap();
        self.dirty.store(false, Ordering::SeqCst);
        let last_event_id = self.last_event_id.load(Ordering::SeqCst);
        Some(crate::persist::save(path, *fingerprint, last_event_id, &self.snapshot()))
    }

    /// Every live item scanned from the roots (not zoxide extras), for saving.
    pub fn snapshot(&self) -> Vec<(String, ItemMeta)> {
        let dead = self.dead();
        (0..self.injector.injected_items())
            .filter_map(|i| self.injector.get(i))
            .filter(|item| !item.data.extra && !dead.contains(item.data.id))
            .map(|item| (item.matcher_columns[0].to_string(), *item.data))
            .collect()
    }

    /// Rough memory use: path text plus per-item overhead in nucleo, the dead set and the
    /// folder tables.
    pub fn memory_estimate(&self) -> u64 {
        const PER_ITEM: u64 = 200;
        let injected = self.injector.injected_items();
        let text: u64 = (0..injected)
            .filter_map(|i| self.injector.get(i))
            .map(|item| {
                let s = &item.matcher_columns[0];
                if s.slice(..).is_ascii() { s.len() as u64 } else { s.len() as u64 * 4 }
            })
            .sum();
        text + injected as u64 * PER_ITEM
    }

    /// The folders holding the most items, skipping any whose items are mostly in one
    /// subfolder (that subfolder is listed instead). Folders at the top of a root aren't
    /// candidates only when they're a root themselves.
    pub fn largest_folders(&self, limit: usize) -> Vec<FolderSize> {
        let tables = self.children.lock().unwrap();
        let mut totals: HashMap<String, u64> = HashMap::new();
        for (key, table) in tables.iter() {
            let n = table.len() as u64;
            *totals.entry(key.clone()).or_default() += n;
            let mut k = key.as_str();
            while let Some((parent, _)) = k.rsplit_once('/') {
                if parent.is_empty() {
                    break;
                }
                *totals.entry(parent.to_string()).or_default() += n;
                k = parent;
            }
            if !key.is_empty() && !key.starts_with('/') && !key.contains('/') {
                // Top-level folder under home also counts towards home.
                *totals.entry(String::new()).or_default() += n;
            }
        }
        drop(tables);
        let total_items = self.live_items().max(1);
        let threshold = (total_items / 50).max(500);
        let mut candidates: Vec<FolderSize> = totals
            .iter()
            .filter(|(key, items)| !key.is_empty() && **items >= threshold)
            .filter(|(key, items)| {
                // Skip if one direct subfolder holds 80% or more of this folder's items.
                let prefix = format!("{key}/");
                !totals
                    .iter()
                    .any(|(k, n)| k.starts_with(&prefix) && !k[prefix.len()..].contains('/') && *n * 5 >= **items * 4)
            })
            .map(|(key, items)| FolderSize { key: key.clone(), items: *items })
            .collect();
        candidates.sort_by(|a, b| b.items.cmp(&a.items).then_with(|| a.key.cmp(&b.key)));
        candidates.truncate(limit);
        candidates
    }
}

fn hash(value: &impl Hash) -> u64 {
    let mut h = DefaultHasher::new();
    value.hash(&mut h);
    h.finish()
}
