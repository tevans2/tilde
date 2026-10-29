//! The index's mutable side. Every item gets an ID. Nucleo holds just the path and the
//! ID; the item's metadata (kind, size, dates) lives in a table by ID, so an edited file
//! is updated in place. Nucleo only appends, so removed items are marked dead (search
//! skips them) and renamed ones are added again; once dead items are a fifth of the
//! list, it's rebuilt without them (`compact`). A compact per-folder table of what's
//! indexed lets a fresh listing of any changed folder be diffed and applied.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard};

use nucleo::{Injector, Nucleo, Utf32String};
use tilde_fs::{FsEvent, ListOptions, is_package_name, list_dir};
use tilde_types::{Entry, EntryKind};

use crate::walk::{ScanConfig, walk_sequential};
use crate::{ItemMeta, key_for, with_str};

/// Nucleo's list is rebuilt once dead items are this share of it…
const COMPACT_DEAD_SHARE: u64 = 5;
/// …and at least this many.
const COMPACT_MIN_DEAD: u64 = 1_000;

/// Makes an empty nucleo list (for rebuilding it).
pub(crate) type MakeNucleo = Box<dyn Fn() -> Nucleo<u32> + Send + Sync>;

/// What the index holds for one entry of a folder.
struct Child {
    name_hash: u64,
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
    /// Held while changing what's indexed (applying changes, compacting).
    writing: Mutex<()>,
    /// Item IDs, searched by path. An item's ID is also its position here. Replaced when
    /// compacting.
    injector: RwLock<Injector<u32>>,
    make_nucleo: MakeNucleo,
    /// Bumped by each compaction (under the `metas` lock); search checks its nucleo list
    /// is the current one.
    generation: AtomicU64,
    /// A rebuilt list waiting for search to take it, with its generation.
    compacted: Mutex<Option<(Nucleo<u32>, u64)>>,
    /// Metadata by item ID (`metas.len()` is the next ID).
    metas: RwLock<Vec<ItemMeta>>,
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

/// Item metadata and dead IDs, read-locked together for the duration of a search or
/// lookup (changes wait until it's done).
pub(crate) struct Items<'a> {
    metas: RwLockReadGuard<'a, Vec<ItemMeta>>,
    dead: RwLockReadGuard<'a, Vec<u64>>,
    /// Nucleo's list the IDs refer to.
    injector: Injector<u32>,
    /// Which compaction the IDs are from.
    pub generation: u64,
}

impl Items<'_> {
    fn is_dead(&self, id: u32) -> bool {
        self.dead.get(id as usize / 64).is_some_and(|w| w & (1 << (id % 64)) != 0)
    }

    /// The item's metadata, unless it's dead.
    pub fn live(&self, id: u32) -> Option<ItemMeta> {
        if self.is_dead(id) { None } else { self.metas.get(id as usize).copied() }
    }

    /// Every live item's metadata, in ID order.
    pub fn all_live(&self) -> impl Iterator<Item = &ItemMeta> + '_ {
        self.metas.iter().filter(|m| !self.is_dead(m.id))
    }
}

impl Shared {
    pub fn new(
        home: PathBuf,
        scan: ScanConfig,
        max_items: u64,
        cache: Option<(PathBuf, u64)>,
        injector: Injector<u32>,
        make_nucleo: MakeNucleo,
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
            writing: Mutex::new(()),
            injector: RwLock::new(injector),
            make_nucleo,
            generation: AtomicU64::new(0),
            compacted: Mutex::new(None),
            metas: RwLock::new(Vec::new()),
            next_id: AtomicU32::new(0),
            killed: AtomicU32::new(0),
            dead: RwLock::new(Vec::new()),
            children: Mutex::new(HashMap::new()),
        }
    }

    pub fn key(&self, path: &Path) -> String {
        key_for(&self.home, path)
    }

    /// Items ever added to nucleo, items marked dead among them, and folder tables.
    pub fn counts(&self) -> (u64, u64, u64) {
        let folders = self.children.lock().unwrap().len() as u64;
        (self.next_id() as u64, self.killed.load(Ordering::Relaxed) as u64, folders)
    }

    /// The ID the next item added will get.
    pub fn next_id(&self) -> u32 {
        self.next_id.load(Ordering::Relaxed)
    }

    /// For frecency `scores` by key: the IDs of the items with those keys, the IDs of the
    /// items directly inside those folders (each with its score), and the first ID not
    /// covered. Keys outside home and items not in the folder tables aren't resolved.
    pub fn ids_for(&self, scores: &HashMap<String, f64>) -> (HashMap<u32, f64>, HashMap<u32, f64>, u32) {
        let tables = self.children.lock().unwrap();
        let upto = self.next_id();
        let (mut own, mut inside) = (HashMap::new(), HashMap::new());
        for (key, &score) in scores {
            if key.starts_with('/') {
                continue;
            }
            let (parent, name) = key.rsplit_once('/').unwrap_or(("", key.as_str()));
            let name_hash = hash(&name);
            if let Some(child) = tables.get(parent).and_then(|t| t.iter().find(|c| c.name_hash == name_hash)) {
                own.insert(child.id, score);
            }
            for child in tables.get(key.as_str()).into_iter().flatten() {
                inside.insert(child.id, score);
            }
        }
        (own, inside, upto)
    }

    /// Items currently searchable.
    pub fn live_items(&self) -> u64 {
        // IDs are positions in nucleo's list, so the next one is its length. (Saturating:
        // compacting resets both, and they can be seen mid-way.)
        (self.next_id() as u64).saturating_sub(self.killed.load(Ordering::Relaxed) as u64)
    }

    pub fn items(&self) -> Items<'_> {
        // In this order everywhere: metas, then the injector, then dead.
        let metas = self.metas.read().unwrap();
        let injector = self.injector.read().unwrap().clone();
        let generation = self.generation.load(Ordering::SeqCst);
        Items { metas, dead: self.dead.read().unwrap(), injector, generation }
    }

    /// Live files and packages modified at or after `since` that `keep` accepts (given
    /// the key and metadata), newest first, at most `limit`. Skips anything hidden, inside
    /// a hidden folder, or inside a package (a Photos library's internals aren't
    /// activity). Library packages themselves (Photos, Music) change constantly and are
    /// skipped too.
    ///
    /// One pass over the metadata table that copies nothing: items are judged by their
    /// metadata, then their key where nucleo stores it, and only the newest `limit` so far
    /// are kept (by ID); just those keys are copied at the end.
    pub fn recent(&self, since: i64, limit: usize, keep: impl Fn(&str, &ItemMeta) -> bool) -> Vec<(String, ItemMeta)> {
        if limit == 0 {
            return Vec::new();
        }
        let items = self.items();
        // The newest so far, the worst on top: newer wins, then earlier position (as a
        // stable sort by date would order them).
        let mut best: BinaryHeap<Reverse<(i64, Reverse<u32>)>> = BinaryHeap::with_capacity(limit + 1);
        for &meta in items.all_live() {
            if meta.modified < since || !matches!(meta.kind, EntryKind::File | EntryKind::Package) {
                continue;
            }
            let rank = (meta.modified, Reverse(meta.id));
            if best.len() == limit && best.peek().is_some_and(|Reverse(worst)| rank <= *worst) {
                continue;
            }
            let Some(item) = items.injector.get(meta.id) else { continue };
            let wanted = with_str(&item.matcher_columns[0], |key| {
                let mut parts = key.split('/');
                let name = parts.next_back().unwrap_or("");
                let library = meta.kind == EntryKind::Package && key.ends_with("library");
                let hidden = name.starts_with('.') || parts.clone().any(|part| part.starts_with('.'));
                !library && !hidden && !parts.any(is_package_name) && keep(key, &meta)
            });
            if !wanted {
                continue;
            }
            best.push(Reverse(rank));
            if best.len() > limit {
                best.pop();
            }
        }
        let mut ranked: Vec<(i64, Reverse<u32>)> = best.into_iter().map(|Reverse(r)| r).collect();
        ranked.sort_unstable_by(|a, b| b.cmp(a));
        ranked
            .into_iter()
            .filter_map(|(_, Reverse(id))| {
                let item = items.injector.get(id)?;
                Some((item.matcher_columns[0].to_string(), items.live(id)?))
            })
            .collect()
    }

    /// Live items that `keep` accepts (given the key, where it's stored, and metadata), as
    /// (key, metadata). Only accepted keys are copied.
    pub fn select(&self, mut keep: impl FnMut(&str, &ItemMeta) -> bool) -> Vec<(String, ItemMeta)> {
        let items = self.items();
        let mut found = Vec::new();
        for &meta in items.all_live().filter(|m| !m.extra) {
            let Some(item) = items.injector.get(meta.id) else { continue };
            let column = &item.matcher_columns[0];
            if with_str(column, |key| keep(key, &meta)) {
                found.push((column.to_string(), meta));
            }
        }
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
        // The metadata goes in first, so anything nucleo hands out has some; nucleo is
        // pushed to under the same lock, so IDs and positions agree.
        let mut metas = self.metas.write().unwrap();
        meta.id = metas.len() as u32;
        metas.push(meta);
        self.next_id.store(meta.id + 1, Ordering::Relaxed);
        self.injector.read().unwrap().push(meta.id, |_, cols| cols[0] = Utf32String::from(key));
        Some(meta.id)
    }

    /// Brings item `id`'s metadata up to date with a fresh listing of it, in place. False
    /// if it's become a different kind of thing (a file replaced by a folder of the same
    /// name), which is added again instead.
    fn update(&self, id: u32, e: &Entry) -> bool {
        let current = self.metas.read().unwrap()[id as usize];
        if current.kind != e.kind {
            return false;
        }
        if (current.size, current.modified, current.child_count) != (e.size, e.modified, e.child_count) {
            let mut metas = self.metas.write().unwrap();
            let meta = &mut metas[id as usize];
            (meta.size, meta.modified, meta.child_count) = (e.size, e.modified, e.child_count);
        }
        true
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
            let path = dir.join(&e.name);
            let descend = is_folder && self.scan.descends_into(&path, &e.name);
            let id = match old.remove(&name_hash) {
                // Still there: an edit only changes its metadata.
                Some(c) if c.dir_name.is_some() == is_folder && self.update(c.id, e) => {
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
            table.push(Child { name_hash, id, dir_name: is_folder.then(|| e.name.as_str().into()) });
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
        let _writing = self.writing.lock().unwrap();
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

    /// Compacts if dead items have reached a fifth of nucleo's list. Returns whether it did.
    pub fn compact_if_due(&self) -> bool {
        let (injected, dead, _) = self.counts();
        let due = dead >= COMPACT_MIN_DEAD && dead * COMPACT_DEAD_SHARE >= injected;
        if due {
            self.compact();
        }
        due
    }

    /// Rebuilds nucleo's list from the live items, renumbering them in order (so IDs and
    /// positions still agree). Built under read locks, so searches carry on meanwhile
    /// (nothing else changes the index: changes wait for `writing`), then swapped in at
    /// once. Search takes the new list on its next call (`take_compacted`).
    pub fn compact(&self) {
        let _writing = self.writing.lock().unwrap();
        let nucleo = (self.make_nucleo)();
        let injector = nucleo.injector();
        let (metas, renumbered) = {
            let items = self.items();
            let mut metas = Vec::with_capacity(items.metas.len() - self.killed.load(Ordering::Relaxed) as usize);
            let mut renumbered = vec![u32::MAX; items.metas.len()];
            for old in 0..items.injector.injected_items() {
                let (Some(mut meta), Some(item)) = (items.live(old), items.injector.get(old)) else { continue };
                meta.id = metas.len() as u32;
                renumbered[old as usize] = meta.id;
                injector.push(meta.id, |_, cols| cols[0] = item.matcher_columns[0].clone());
                metas.push(meta);
            }
            (metas, renumbered)
        };

        let mut tables = self.children.lock().unwrap();
        for table in tables.values_mut() {
            // Tables only hold live items.
            table.retain_mut(|c| {
                c.id = renumbered[c.id as usize];
                c.id != u32::MAX
            });
        }
        let mut current = self.metas.write().unwrap();
        let mut current_injector = self.injector.write().unwrap();
        let mut dead = self.dead.write().unwrap();
        self.next_id.store(metas.len() as u32, Ordering::Relaxed);
        *current = metas;
        *current_injector = injector;
        dead.clear();
        self.killed.store(0, Ordering::Relaxed);
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *self.compacted.lock().unwrap() = Some((nucleo, generation));
    }

    /// The list `compact` rebuilt, if search hasn't taken it yet, with its generation.
    pub fn take_compacted(&self) -> Option<(Nucleo<u32>, u64)> {
        self.compacted.lock().unwrap().take()
    }

    /// Adds saved items, rebuilding the per-folder tables.
    pub fn load(&self, items: Vec<(String, ItemMeta)>) {
        let mut tables = self.children.lock().unwrap();
        for (key, meta) in items {
            let Some(id) = self.push(&key, meta) else { break };
            let (parent, name) = key.rsplit_once('/').unwrap_or(("", &key));
            let parent = if parent.is_empty() && key.starts_with('/') { "/" } else { parent };
            let dir_name = (meta.kind == EntryKind::Folder).then(|| name.into());
            tables.entry(parent.to_string()).or_default().push(Child { name_hash: hash(&name), id, dir_name });
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

    /// Every live item scanned from the roots (not frecency extras), for saving.
    pub fn snapshot(&self) -> Vec<(String, ItemMeta)> {
        let items = self.items();
        (0..items.injector.injected_items())
            .filter_map(|i| items.injector.get(i))
            .filter_map(|item| {
                let meta = items.live(*item.data).filter(|m| !m.extra)?;
                Some((item.matcher_columns[0].to_string(), meta))
            })
            .collect()
    }

    /// Rough memory use: path text plus per-item overhead in nucleo, the metadata table,
    /// the dead set and the folder tables.
    pub fn memory_estimate(&self) -> u64 {
        const PER_ITEM: u64 = 200;
        let items = self.items();
        let injected = items.injector.injected_items();
        let text: u64 = (0..injected)
            .filter_map(|i| items.injector.get(i))
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
        // Counted with keys borrowed from the tables (no copies), under their lock.
        let tables = self.children.lock().unwrap();
        let mut totals: HashMap<&str, u64> = HashMap::with_capacity(tables.len());
        for (key, table) in tables.iter() {
            let n = table.len() as u64;
            *totals.entry(key.as_str()).or_default() += n;
            let mut k = key.as_str();
            while let Some((parent, _)) = k.rsplit_once('/') {
                if parent.is_empty() {
                    break;
                }
                *totals.entry(parent).or_default() += n;
                k = parent;
            }
            if !key.is_empty() && !key.starts_with('/') && !key.contains('/') {
                // Top-level folder under home also counts towards home.
                *totals.entry("").or_default() += n;
            }
        }
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
            .map(|(key, items)| FolderSize { key: key.to_string(), items: *items })
            .collect();
        drop(totals);
        drop(tables);
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
