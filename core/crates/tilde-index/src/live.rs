//! The index's mutable side. Nucleo only appends, so every item gets an ID: removed
//! and changed items are marked dead (search skips them) and changed items are pushed
//! again. A compact per-folder table of what's indexed lets a fresh listing of any
//! changed folder be diffed and applied.

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, RwLock, RwLockReadGuard};

use nucleo::{Injector, Utf32String};
use tilde_fs::{FsEvent, ListOptions, list_dir};
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
    injector: Injector<ItemMeta>,
    next_id: AtomicU32,
    killed: AtomicU32,
    dead: RwLock<Vec<u64>>,
    /// Folder key → its indexed entries.
    children: Mutex<HashMap<String, Vec<Child>>>,
}

/// Dead item IDs, read-locked for the duration of a search.
pub(crate) struct DeadSet<'a>(RwLockReadGuard<'a, Vec<u64>>);

impl DeadSet<'_> {
    pub fn contains(&self, id: u32) -> bool {
        self.0.get(id as usize / 64).is_some_and(|w| w & (1 << (id % 64)) != 0)
    }
}

impl Shared {
    pub fn new(home: PathBuf, scan: ScanConfig, injector: Injector<ItemMeta>) -> Shared {
        Shared {
            home,
            scan,
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

    /// Adds an item. Returns its ID.
    pub fn push(&self, key: &str, mut meta: ItemMeta) -> u32 {
        meta.id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.injector.push(meta, |_, cols| cols[0] = Utf32String::from(key));
        meta.id
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
                    if let Some(c) = existing {
                        self.kill(c.id);
                    } else if descend {
                        next.push(path);
                    }
                    let item_key = if key.is_empty() { e.name.clone() } else { format!("{key}/{}", e.name) };
                    let meta = ItemMeta {
                        id: 0,
                        kind: e.kind,
                        size: e.size,
                        modified: e.modified,
                        child_count: e.child_count,
                    };
                    self.push(&item_key, meta)
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
}

fn hash(value: &impl Hash) -> u64 {
    let mut h = DefaultHasher::new();
    value.hash(&mut h);
    h.finish()
}
