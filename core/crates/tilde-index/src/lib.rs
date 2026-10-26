//! The global filename index behind the Everywhere scope.
//!
//! Built by a background scan, kept current with FSEvents, and saved between launches
//! (see docs/SPEC.md §6.2). On launch a saved index is loaded and FSEvents replays only
//! what changed since. Matching is nucleo, re-ranked with filename, depth, recency and
//! frecency boosts (Tilde's own, plus zoxide's when it's installed).
//!
//! Paths are stored as keys relative to the home folder (`Developer/tilde/README.md`), or
//! absolute for anything outside it. Relative keys keep memory down and stop every query
//! from matching the home folder's own name.

mod frecency;
mod live;
mod persist;
mod walk;
mod zoxide;

pub use frecency::{Frecency, HALF_LIFE, Use};
pub use live::FolderSize;
pub use walk::{ScanConfig, glob_match, scan, walk_parallel, walk_sequential};
pub use zoxide::{Zoxide, parse_scores};

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher, Nucleo, Utf32Str, Utf32String};
use tilde_fs::{FsEvent, SINCE_NOW, Watcher, current_event_id, watch};
use tilde_query::{Candidate, FilterContext, Query};
use tilde_types::EntryKind;

use crate::live::Shared;

/// How many top matches are re-ranked with boosts.
const RERANK_CANDIDATES: usize = 3000;
/// Scores decay slowly: derived ones are refreshed this often even without new uses.
const FRECENCY_REFRESH: Duration = Duration::from_secs(60);
/// zoxide's scores are re-read this often (a process launch).
const ZOXIDE_REFRESH: Duration = Duration::from_secs(60);
/// FSEvents coalescing window.
const WATCH_LATENCY: Duration = Duration::from_millis(200);
/// How often a changed index is saved while running.
const AUTOSAVE: Duration = Duration::from_secs(300);

#[derive(Debug, Clone)]
pub struct IndexConfig {
    pub home: PathBuf,
    pub scan: ScanConfig,
    /// Ranks by these uses, and indexes the folders in it the scan doesn't reach.
    pub frecency: Option<Arc<Mutex<Frecency>>>,
    /// Adds zoxide's scores (folders used in the terminal), if it's installed.
    pub zoxide: bool,
    pub max_items: u64,
    /// Where the index is saved between launches. `None` keeps it in memory only.
    pub cache_path: Option<PathBuf>,
}

impl IndexConfig {
    /// Identifies the settings a saved index was built with; a saved index built with
    /// different ones is discarded.
    fn fingerprint(&self) -> u64 {
        let mut h = DefaultHasher::new();
        "tilde-index-1".hash(&mut h);
        self.home.hash(&mut h);
        self.scan.roots.hash(&mut h);
        let mut names: Vec<&String> = self.scan.exclude_names.iter().collect();
        names.sort();
        names.hash(&mut h);
        self.scan.exclude_paths.hash(&mut h);
        self.scan.hide_names.hash(&mut h);
        self.scan.hide_paths.hash(&mut h);
        self.scan.include_hidden.hash(&mut h);
        h.finish()
    }
}

/// Saves `items` as `config`'s saved index, current to now, as if a scan had found them.
/// For benchmarks with synthetic trees too big to create on disk.
#[doc(hidden)]
pub fn write_saved(config: &IndexConfig, items: &[(String, ItemMeta)]) -> std::io::Result<u64> {
    let path = config.cache_path.as_deref().ok_or_else(|| std::io::Error::other("no cache_path"))?;
    persist::save(path, config.fingerprint(), current_event_id(), items)
}

/// What the index keeps per item besides its path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemMeta {
    /// Assigned when the item is added; used to mark it dead when it changes or goes.
    pub id: u32,
    pub kind: EntryKind,
    pub size: u64,
    pub modified: i64,
    pub child_count: Option<u32>,
    /// Added because you use it (frecency) rather than scanned; not saved.
    pub extra: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GlobalHit {
    /// Relative to home, or absolute when outside it.
    pub key: String,
    pub meta: ItemMeta,
    /// Matched char positions in `key`, sorted.
    pub indices: Vec<u32>,
    pub score: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexStatus {
    pub items: u64,
    pub scanning: bool,
    /// Stopped adding at `max_items`.
    pub capped: bool,
}

/// Thread-safe access to the index's size, stats and saving, without the search lock.
#[derive(Clone)]
pub struct IndexHandle {
    shared: Arc<Shared>,
    scanning: Arc<AtomicBool>,
}

impl IndexHandle {
    pub fn status(&self) -> IndexStatus {
        IndexStatus {
            items: self.shared.live_items(),
            scanning: self.scanning.load(Ordering::SeqCst),
            capped: self.shared.capped.load(Ordering::SeqCst),
        }
    }

    pub fn max_items(&self) -> u64 {
        self.shared.max_items
    }

    /// Rough bytes of memory the index uses.
    pub fn memory_estimate(&self) -> u64 {
        self.shared.memory_estimate()
    }

    pub fn largest_folders(&self, limit: usize) -> Vec<FolderSize> {
        self.shared.largest_folders(limit)
    }

    pub fn cache_path(&self) -> Option<&Path> {
        self.shared.cache.as_ref().map(|(p, _)| p.as_path())
    }

    /// Size of the saved index on disk, if there is one.
    pub fn disk_size(&self) -> Option<u64> {
        std::fs::metadata(self.cache_path()?).ok().map(|m| m.len())
    }

    /// Saves now. Returns the file size, or `None` if saving is off.
    pub fn save(&self) -> Option<std::io::Result<u64>> {
        self.shared.save()
    }

    /// Deletes the saved index and stops this index from saving again.
    pub fn delete_saved(&self) -> std::io::Result<()> {
        self.shared.saving_disabled.store(true, Ordering::SeqCst);
        match self.cache_path() {
            Some(p) => match std::fs::remove_file(p) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            },
            None => Ok(()),
        }
    }

    /// Absolute path for a key.
    pub fn absolute(&self, key: &str) -> PathBuf {
        if key.starts_with('/') { PathBuf::from(key) } else { self.shared.home.join(key) }
    }

    /// Every indexed item `keep` accepts (given its absolute path and metadata). Walks the
    /// whole index (a few ms per 100k items) without the search lock.
    pub fn select(&self, mut keep: impl FnMut(&Path, &ItemMeta) -> bool) -> Vec<(PathBuf, ItemMeta)> {
        let home = &self.shared.home;
        let absolute = |key: &str| if key.starts_with('/') { PathBuf::from(key) } else { home.join(key) };
        self.shared
            .select(|key, meta| keep(&absolute(key), meta))
            .into_iter()
            .map(|(key, meta)| (absolute(&key), meta))
            .collect()
    }

    /// Visible files modified at or after `since` (seconds since the epoch), newest
    /// first, as (absolute path, metadata). Doesn't wait for the search lock.
    pub fn recent(&self, since: i64, limit: usize) -> Vec<(PathBuf, ItemMeta)> {
        self.shared.recent(since, limit).into_iter().map(|(key, meta)| (self.absolute(&key), meta)).collect()
    }
}

pub struct GlobalIndex {
    nucleo: Nucleo<ItemMeta>,
    shared: Arc<Shared>,
    scanning: Arc<AtomicBool>,
    /// Dropping it ends the stream, which ends the update thread.
    _watcher: Option<Watcher>,
    frecency: Option<Arc<Mutex<Frecency>>>,
    zoxide: Option<Zoxide>,
    /// √ of zoxide's scores, by key.
    zoxide_scores: HashMap<String, f64>,
    zoxide_at: Instant,
    /// Frecency scores (Tilde's plus zoxide's) by key, as of `scores_at`.
    scores: HashMap<String, f64>,
    scores_version: u64,
    scores_at: Instant,
    pattern_text: String,
    matcher: Matcher,
    name_matcher: Matcher,
    buf: Vec<char>,
}

impl GlobalIndex {
    /// Loads the saved index or scans, then keeps the index current with FSEvents.
    /// `on_change` runs when the first load or scan finishes, after each batch of changes
    /// is applied, and when a matching pass that outlasted `search`'s wait completes (so
    /// slow searches still show up).
    pub fn start(config: IndexConfig, on_change: impl Fn() + Send + Sync + 'static) -> GlobalIndex {
        let on_change: Arc<dyn Fn() + Send + Sync> = Arc::new(on_change);
        let notify = on_change.clone();
        let nucleo = Nucleo::new(Config::DEFAULT.match_paths(), Arc::new(move || notify()), None, 1);
        let fingerprint = config.fingerprint();
        let cache = config.cache_path.clone().map(|p| (p, fingerprint));
        let shared =
            Arc::new(Shared::new(config.home.clone(), config.scan.clone(), config.max_items, cache, nucleo.injector()));

        // A saved index is usable if it was built with these settings and FSEvents can
        // replay everything since (IDs only go backwards if its database was reset).
        let saved = config
            .cache_path
            .as_deref()
            .and_then(|p| persist::load(p, fingerprint))
            .filter(|s| s.last_event_id <= current_event_id());
        let resume_from = saved.map(|s| {
            shared.load(s.items);
            s.last_event_id
        });
        shared.last_event_id.store(resume_from.unwrap_or_else(current_event_id), Ordering::SeqCst);

        // Start watching before scanning so nothing that changes mid-scan is missed;
        // events (or replayed history) queue up until the scan is done.
        let (events_tx, events_rx) = std::sync::mpsc::channel::<Vec<FsEvent>>();
        let watcher = watch(
            &config.scan.roots,
            &config.scan.exclude_paths,
            resume_from.unwrap_or(SINCE_NOW),
            WATCH_LATENCY,
            move |batch| {
                let _ = events_tx.send(batch);
            },
        )
        .ok();

        let mut index = GlobalIndex {
            nucleo,
            shared: shared.clone(),
            scanning: Arc::new(AtomicBool::new(resume_from.is_none())),
            _watcher: watcher,
            frecency: config.frecency.clone(),
            zoxide: config.zoxide.then(Zoxide::find).flatten(),
            zoxide_scores: HashMap::new(),
            zoxide_at: Instant::now(),
            scores: HashMap::new(),
            scores_version: u64::MAX,
            scores_at: Instant::now(),
            pattern_text: String::new(),
            matcher: Matcher::new(Config::DEFAULT.match_paths()),
            name_matcher: Matcher::new(Config::DEFAULT),
            buf: Vec::new(),
        };
        let mut known_dirs = index.reload_zoxide();
        known_dirs.extend(config.frecency.as_ref().map(|f| f.lock().unwrap().folders()).unwrap_or_default());

        let scanning = index.scanning.clone();
        std::thread::Builder::new()
            .name("tilde-index".into())
            .spawn(move || {
                if resume_from.is_none() {
                    let stop = shared.stop.clone();
                    walk_parallel(config.scan.roots.clone(), config.scan.include_hidden, &stop, &|dir, entries| {
                        shared.sync_dir(dir, &entries, false)
                    });
                }
                // Folders you use that the scan can't reach (hidden, excluded, outside home).
                for dir in known_dirs.iter().filter(|d| !config.scan.covers(d)) {
                    if let Ok(md) = std::fs::metadata(dir)
                        && md.is_dir()
                    {
                        let modified = md
                            .modified()
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map_or(0, |d| d.as_secs() as i64);
                        let key = shared.key(dir);
                        let meta = ItemMeta {
                            id: 0,
                            kind: EntryKind::Folder,
                            size: 0,
                            modified,
                            child_count: None,
                            extra: true,
                        };
                        if !key.is_empty() {
                            shared.push(&key, meta);
                        }
                    }
                }
                if resume_from.is_none() {
                    let _ = shared.save();
                }
                scanning.store(false, Ordering::SeqCst);
                on_change();

                // Then apply changes as they arrive, a batch at a time, saving now and then.
                loop {
                    match events_rx.recv_timeout(AUTOSAVE) {
                        Ok(first) => {
                            let mut batch = first;
                            batch.extend(events_rx.try_iter().flatten());
                            shared.apply(&batch);
                            on_change();
                        }
                        Err(RecvTimeoutError::Timeout) => {
                            if shared.dirty.load(Ordering::SeqCst) {
                                let _ = shared.save();
                            }
                        }
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .expect("spawn index thread");
        index
    }

    pub fn status(&self) -> IndexStatus {
        self.handle().status()
    }

    pub fn handle(&self) -> IndexHandle {
        IndexHandle { shared: self.shared.clone(), scanning: self.scanning.clone() }
    }

    /// The key form of a path (relative to home, or absolute). Empty for home itself.
    pub fn key(&self, path: &Path) -> String {
        self.shared.key(path)
    }

    /// Absolute path for a key.
    pub fn absolute(&self, key: &str) -> PathBuf {
        if key.starts_with('/') { PathBuf::from(key) } else { self.shared.home.join(key) }
    }

    /// The most-used folders that still exist, best first (keys).
    pub fn top_folders(&mut self, limit: usize) -> Vec<String> {
        self.refresh_scores();
        let mut best: Vec<(&String, f64)> =
            self.scores.iter().filter(|(k, _)| !k.is_empty()).map(|(k, s)| (k, *s)).collect();
        best.sort_by(|a, b| b.1.total_cmp(&a.1));
        best.into_iter().map(|(k, _)| k).filter(|k| self.absolute(k).is_dir()).take(limit).cloned().collect()
    }

    /// Searches the whole index. `cwd` (a key) supports `in:.`; items directly inside
    /// `exclude_parent` (a key) are skipped because the Here scope already shows them.
    pub fn search(
        &mut self,
        query: &Query,
        cwd: Option<&str>,
        exclude_parent: Option<&str>,
        now: i64,
        limit: usize,
    ) -> Vec<GlobalHit> {
        if query.is_empty() {
            return Vec::new();
        }
        self.refresh_scores();
        if query.text != self.pattern_text {
            let append = !self.pattern_text.is_empty()
                && query.text.starts_with(&self.pattern_text)
                && !self.pattern_text.ends_with('\\');
            self.nucleo.pattern.reparse(0, &query.text, CaseMatching::Ignore, Normalization::Smart, append);
            self.pattern_text = query.text.clone();
        }
        // Finish matching what's been injected so far (bounded, in case a scan is
        // flooding new items).
        let deadline = Instant::now() + Duration::from_millis(250);
        while self.nucleo.tick(10).running && Instant::now() < deadline {}

        let snap = self.nucleo.snapshot();
        let dead = self.shared.dead();
        let ctx = FilterContext { now, cwd };
        let has_text = !query.text.is_empty();
        let name_pattern = has_text.then(|| Pattern::parse(&query.text, CaseMatching::Ignore, Normalization::Smart));

        let mut ranked: Vec<(u32, i64)> =
            if has_text { Vec::new() } else { filtered_newest(snap, &dead, query, &ctx, exclude_parent) };
        for n in 0..if has_text { snap.matched_item_count() } else { 0 } {
            if ranked.len() >= RERANK_CANDIDATES {
                break;
            }
            let Some(item) = snap.get_matched_item(n) else { break };
            let meta = *item.data;
            if dead.contains(meta.id) {
                continue;
            }
            let column = &item.matcher_columns[0];
            let score = with_str(column, |key| {
                let (parent, name) = key.rsplit_once('/').unwrap_or(("", key));
                if exclude_parent == Some(parent) {
                    return None;
                }
                let candidate =
                    Candidate { name, kind: meta.kind, size: meta.size, modified: meta.modified, parent: Some(parent) };
                if !query.filters.iter().all(|f| f.accepts(&candidate, &ctx)) {
                    return None;
                }
                let path_score = snap.pattern().score(std::slice::from_ref(column), &mut self.matcher)? as i64;
                let name_score = name_pattern
                    .as_ref()
                    .and_then(|p| p.score(Utf32Str::new(name, &mut self.buf), &mut self.name_matcher))
                    .unwrap_or(0) as i64;
                let depth = key.matches('/').count() as i64;
                let length_penalty = key.len() as i64 / 10;
                let fresh = if now - meta.modified < 7 * 86_400 { 4 } else { 0 };
                let own = self.scores.get(key).map_or(0.0, |s| 15.0 * (1.0 + s).ln());
                let inside = self.scores.get(parent).map_or(0.0, |s| 5.0 * (1.0 + s).ln());
                Some(path_score + name_score + fresh + (own + inside) as i64 - 2 * depth - length_penalty)
            });
            if let Some(score) = score {
                ranked.push((n, score));
            }
        }
        // Only the best `limit` are kept: pick them out before sorting (filter-only queries
        // can rank most of the index).
        if ranked.len() > limit && limit > 0 {
            ranked.select_nth_unstable_by_key(limit - 1, |&(_, s)| std::cmp::Reverse(s));
            ranked.truncate(limit);
        }
        ranked.sort_by_key(|&(_, s)| std::cmp::Reverse(s));

        let pattern = snap.pattern().column_pattern(0);
        let mut indices = Vec::new();
        ranked
            .into_iter()
            .filter_map(|(n, score)| {
                let item = snap.get_matched_item(n)?;
                let column = &item.matcher_columns[0];
                indices.clear();
                if has_text {
                    pattern.indices(column.slice(..), &mut self.matcher, &mut indices);
                    indices.sort_unstable();
                    indices.dedup();
                }
                Some(GlobalHit { key: column.to_string(), meta: *item.data, indices: indices.clone(), score })
            })
            .collect()
    }

    /// Re-derives the scores by key after new uses, or once a minute as they decay (and
    /// zoxide's change).
    fn refresh_scores(&mut self) {
        let zoxide_changed = self.zoxide.is_some() && self.zoxide_at.elapsed() > ZOXIDE_REFRESH;
        if zoxide_changed {
            self.reload_zoxide();
        }
        let version = self.frecency.as_ref().map_or(0, |f| f.lock().unwrap().version());
        if !zoxide_changed && version == self.scores_version && self.scores_at.elapsed() < FRECENCY_REFRESH {
            return;
        }
        let mut scores = self.zoxide_scores.clone();
        if let Some(f) = &self.frecency {
            let now = now();
            for (path, s) in f.lock().unwrap().scores(now) {
                *scores.entry(self.shared.key(Path::new(path))).or_default() += s;
            }
        }
        self.scores = scores;
        self.scores_version = version;
        self.scores_at = Instant::now();
    }

    /// Re-reads zoxide's scores. Returns the folders it knows.
    fn reload_zoxide(&mut self) -> Vec<PathBuf> {
        self.zoxide_at = Instant::now();
        let Some(z) = &self.zoxide else { return Vec::new() };
        let scores = z.scores();
        // zoxide's scores run into the hundreds: compressed, keeping their order, to about
        // what a few weeks of use in Tilde gives.
        self.zoxide_scores = scores.iter().map(|(p, s)| (self.shared.key(p), s.max(0.0).sqrt())).collect();
        scores.into_iter().map(|(p, _)| p).collect()
    }
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

impl Drop for GlobalIndex {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
    }
}

pub(crate) fn key_for(home: &Path, path: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rel) => rel.to_string_lossy().into_owned(),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

/// Borrows a nucleo string as `&str` without allocating when it's ASCII.
/// Filters only (no text): every live item that passes, scored by modification time so the
/// newest come first. This visits the whole index, so it runs in parallel.
fn filtered_newest(
    snap: &nucleo::Snapshot<ItemMeta>,
    dead: &live::DeadSet<'_>,
    query: &Query,
    ctx: &FilterContext<'_>,
    exclude_parent: Option<&str>,
) -> Vec<(u32, i64)> {
    use rayon::prelude::*;
    (0..snap.matched_item_count())
        .into_par_iter()
        .filter_map(|n| {
            let item = snap.get_matched_item(n)?;
            let meta = *item.data;
            if dead.contains(meta.id) {
                return None;
            }
            with_str(&item.matcher_columns[0], |key| {
                let (parent, name) = key.rsplit_once('/').unwrap_or(("", key));
                let candidate =
                    Candidate { name, kind: meta.kind, size: meta.size, modified: meta.modified, parent: Some(parent) };
                let keep = exclude_parent != Some(parent) && query.filters.iter().all(|f| f.accepts(&candidate, ctx));
                keep.then_some((n, meta.modified))
            })
        })
        .collect()
}

fn with_str<R>(s: &Utf32String, f: impl FnOnce(&str) -> R) -> R {
    match s.slice(..) {
        // SAFETY: nucleo stores a string as ASCII bytes only when every byte is ASCII.
        Utf32Str::Ascii(bytes) => f(unsafe { std::str::from_utf8_unchecked(bytes) }),
        Utf32Str::Unicode(chars) => f(&chars.iter().collect::<String>()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn config(root: &Path) -> IndexConfig {
        IndexConfig {
            home: root.to_path_buf(),
            scan: ScanConfig {
                roots: vec![root.to_path_buf()],
                exclude_names: ["node_modules".to_string()].into(),
                ..Default::default()
            },
            frecency: None,
            zoxide: false,
            max_items: 1_000_000,
            cache_path: None,
        }
    }

    fn start_and_wait(config: IndexConfig) -> GlobalIndex {
        let index = GlobalIndex::start(config, || {});
        let t = Instant::now();
        while index.status().scanning {
            assert!(t.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(10));
        }
        index
    }

    fn build(root: &Path) -> GlobalIndex {
        start_and_wait(config(root))
    }

    fn tree() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        for d in ["Developer/projects/tilde/Sources", "Documents/Finance/2026", "node_modules/left-pad"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        for f in [
            "Developer/projects/tilde/README.md",
            "Developer/projects/tilde/Sources/main.swift",
            "Developer/projects/tilde/Sources/FileIndex.swift",
            "Documents/Finance/2026/invoice-oct.pdf",
            "Documents/notes.md",
            "node_modules/left-pad/index.js",
        ] {
            fs::write(root.join(f), b"x").unwrap();
        }
        (dir, root)
    }

    fn keys(hits: &[GlobalHit]) -> Vec<&str> {
        hits.iter().map(|h| h.key.as_str()).collect()
    }

    #[test]
    fn finds_files_anywhere_with_path_terms() {
        let (_dir, root) = tree();
        let mut index = build(&root);
        assert_eq!(index.status().items, 13);

        let hits = index.search(&Query::parse("tilde swift"), None, None, 0, 10);
        assert!(keys(&hits).contains(&"Developer/projects/tilde/Sources/main.swift"));
        assert!(keys(&hits).contains(&"Developer/projects/tilde/Sources/FileIndex.swift"));
        assert!(!hits[0].indices.is_empty());

        let hits = index.search(&Query::parse("invoice"), None, None, 0, 10);
        assert_eq!(keys(&hits), ["Documents/Finance/2026/invoice-oct.pdf"]);

        // Excluded folder contents aren't indexed, the folder is.
        assert!(index.search(&Query::parse("left-pad"), None, None, 0, 10).is_empty());
        assert_eq!(keys(&index.search(&Query::parse("node_modules"), None, None, 0, 10)), ["node_modules"]);
    }

    #[test]
    fn recent_lists_new_visible_files_newest_first() {
        let (_dir, root) = tree();
        fs::create_dir(root.join(".config")).unwrap();
        fs::write(root.join(".config/settings"), b"x").unwrap();
        fs::write(root.join(".zshrc"), b"x").unwrap();
        fs::create_dir_all(root.join("Pictures/Photos Library.photoslibrary/resources")).unwrap();
        fs::write(root.join("Pictures/Photos Library.photoslibrary/resources/sync.plist"), b"x").unwrap();
        let old = std::time::SystemTime::now() - Duration::from_secs(90 * 86_400);
        fs::File::options().write(true).open(root.join("Documents/notes.md")).unwrap().set_modified(old).unwrap();
        let newest = std::time::SystemTime::now() + Duration::from_secs(60);
        fs::File::options()
            .write(true)
            .open(root.join("Documents/Finance/2026/invoice-oct.pdf"))
            .unwrap()
            .set_modified(newest)
            .unwrap();
        let mut config = config(&root);
        config.scan.include_hidden = true;
        let index = start_and_wait(config);

        let since = (std::time::SystemTime::now() - Duration::from_secs(86_400))
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let recent = index.handle().recent(since, 10);
        let paths: Vec<String> =
            recent.iter().map(|(p, _)| p.strip_prefix(&root).unwrap().to_string_lossy().into_owned()).collect();
        // Folders, hidden files, files in hidden folders and old files are left out.
        assert_eq!(paths.len(), 4, "{paths:?}");
        assert_eq!(paths[0], "Documents/Finance/2026/invoice-oct.pdf");
        assert!(!paths.iter().any(|p| p.contains("notes.md") || p.contains('.') && p.starts_with('.')));
        assert_eq!(index.handle().recent(since, 2).len(), 2);
    }

    #[test]
    fn filters_scope_and_exclusion() {
        let (_dir, root) = tree();
        let mut index = build(&root);
        let hits = index.search(&Query::parse("ext:md"), None, None, i64::MAX / 2, 10);
        let mut k = keys(&hits);
        k.sort();
        assert_eq!(k, ["Developer/projects/tilde/README.md", "Documents/notes.md"]);

        let hits = index.search(&Query::parse("md in:dev"), None, None, 0, 10);
        assert_eq!(keys(&hits), ["Developer/projects/tilde/README.md"]);

        let hits = index.search(&Query::parse("ext:swift in:."), Some("Developer/projects/tilde"), None, 0, 10);
        assert_eq!(hits.len(), 2);

        // Items directly in the current folder are left to the Here scope.
        let hits = index.search(&Query::parse("notes"), None, Some("Documents"), 0, 10);
        assert!(hits.is_empty());
    }

    fn search_keys(index: &mut GlobalIndex, q: &str) -> Vec<String> {
        let mut k: Vec<String> = index.search(&Query::parse(q), None, None, 0, 50).into_iter().map(|h| h.key).collect();
        k.sort();
        k
    }

    fn changed(dir: PathBuf) -> FsEvent {
        FsEvent { path: dir, flags: 0, id: 0 }
    }

    #[test]
    fn applying_changes_adds_updates_and_removes() {
        let (_dir, root) = tree();
        let mut index = build(&root);
        let before = index.status().items;

        // New file and new folder with contents.
        fs::write(root.join("Documents/fresh-report.txt"), b"x").unwrap();
        fs::create_dir_all(root.join("Documents/Archive/2025")).unwrap();
        fs::write(root.join("Documents/Archive/2025/old-report.txt"), b"x").unwrap();
        index.shared.apply(&[changed(root.join("Documents"))]);
        assert_eq!(
            search_keys(&mut index, "report"),
            ["Documents/Archive/2025/old-report.txt", "Documents/fresh-report.txt"]
        );
        assert_eq!(index.status().items, before + 4);

        // A changed file is replaced, not duplicated.
        fs::write(root.join("Documents/fresh-report.txt"), b"longer contents").unwrap();
        index.shared.apply(&[changed(root.join("Documents"))]);
        let hits = index.search(&Query::parse("fresh-report"), None, None, 0, 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].meta.size, 15);

        // Removing a folder removes everything under it.
        fs::remove_dir_all(root.join("Documents/Archive")).unwrap();
        index.shared.apply(&[changed(root.join("Documents")), changed(root.join("Documents/Archive"))]);
        assert_eq!(search_keys(&mut index, "report"), ["Documents/fresh-report.txt"]);
        assert_eq!(index.status().items, before + 1);

        // Changes inside excluded folders are ignored.
        fs::write(root.join("node_modules/left-pad/extra.js"), b"x").unwrap();
        index.shared.apply(&[changed(root.join("node_modules/left-pad"))]);
        assert!(search_keys(&mut index, "extra").is_empty());
    }

    #[test]
    fn rescan_picks_up_changes_deep_inside() {
        let (_dir, root) = tree();
        let mut index = build(&root);
        fs::write(root.join("Developer/projects/tilde/Sources/Watcher.swift"), b"x").unwrap();
        fs::remove_file(root.join("Developer/projects/tilde/README.md")).unwrap();
        index.shared.apply(&[FsEvent { path: root.clone(), flags: 0x01, id: 0 }]);
        assert_eq!(search_keys(&mut index, "Watcher"), ["Developer/projects/tilde/Sources/Watcher.swift"]);
        assert!(search_keys(&mut index, "README").is_empty());
    }

    #[test]
    fn real_fsevents_keep_the_index_current() {
        let (_dir, root) = tree();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut index = GlobalIndex::start(config(&root), move || {
            let _ = tx.send(());
        });
        rx.recv_timeout(Duration::from_secs(10)).expect("initial scan");
        std::thread::sleep(Duration::from_millis(300));

        fs::write(root.join("Documents/created-live.md"), b"x").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if search_keys(&mut index, "created-live") == ["Documents/created-live.md"] {
                break;
            }
            assert!(Instant::now() < deadline, "new file never appeared in the index");
            let _ = rx.recv_timeout(Duration::from_millis(500));
        }
    }

    #[test]
    fn saves_and_resumes_with_changes_since() {
        let (_dir, root) = tree();
        // Outside the indexed folder, or saving would itself show up as a change.
        let cache_dir = tempfile::tempdir().unwrap();
        let cache = cache_dir.path().join("cache/index.bin");
        let with_cache = || IndexConfig { cache_path: Some(cache.clone()), ..config(&root) };

        let first = start_and_wait(with_cache());
        let items = first.status().items;
        let size = first.handle().disk_size().expect("saved after the first scan");
        assert!(size > 0);
        drop(first);

        // Changes while "closed" are replayed from FSEvents history on the next start.
        std::thread::sleep(Duration::from_millis(500));
        fs::write(root.join("Documents/while-closed.txt"), b"x").unwrap();
        std::thread::sleep(Duration::from_millis(500));

        let mut second = GlobalIndex::start(with_cache(), || {});
        assert!(!second.status().scanning, "loaded, not scanned");
        assert!(second.status().items >= items);
        let deadline = Instant::now() + Duration::from_secs(10);
        while search_keys(&mut second, "while-closed").is_empty() {
            assert!(Instant::now() < deadline, "change made while closed was never replayed");
            std::thread::sleep(Duration::from_millis(100));
        }

        // Different settings discard the saved index.
        let mut changed = with_cache();
        changed.scan.hide_names = vec!["*.md".into()];
        let third = start_and_wait(changed);
        assert!(third.status().items < items);
    }

    #[test]
    fn deleting_the_saved_index_stops_saving() {
        let (_dir, root) = tree();
        let cache_dir = tempfile::tempdir().unwrap();
        let cache = cache_dir.path().join("index.bin");
        let index = start_and_wait(IndexConfig { cache_path: Some(cache.clone()), ..config(&root) });
        assert!(cache.exists());
        index.handle().delete_saved().unwrap();
        assert!(!cache.exists());
        assert!(index.handle().save().is_none());
        drop(index);
        std::thread::sleep(Duration::from_millis(100));
        assert!(!cache.exists());
    }

    #[test]
    fn stops_at_max_items() {
        let (_dir, root) = tree();
        let index = start_and_wait(IndexConfig { max_items: 5, ..config(&root) });
        let status = index.status();
        assert_eq!(status.items, 5);
        assert!(status.capped);
    }

    #[test]
    fn hide_rules_leave_items_out() {
        let (_dir, root) = tree();
        let mut cfg = config(&root);
        cfg.scan.hide_names = vec!["*.swift".into()];
        cfg.scan.hide_paths = vec![root.join("Documents/Finance")];
        let mut index = start_and_wait(cfg);
        assert!(search_keys(&mut index, "swift").is_empty());
        assert!(search_keys(&mut index, "invoice").is_empty());
        assert!(search_keys(&mut index, "Finance").is_empty());
        assert_eq!(search_keys(&mut index, "notes"), ["Documents/notes.md"]);
    }

    #[test]
    fn largest_folders_point_at_the_real_culprit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        fs::create_dir_all(root.join("Developer/huge/vendor")).unwrap();
        fs::create_dir_all(root.join("Documents")).unwrap();
        for i in 0..900 {
            fs::write(root.join(format!("Developer/huge/vendor/f{i}")), b"").unwrap();
        }
        for i in 0..20 {
            fs::write(root.join(format!("Documents/d{i}")), b"").unwrap();
        }
        let index = build(&root);
        let largest = index.handle().largest_folders(5);
        // Developer and huge are almost entirely vendor, so vendor is what's listed.
        assert_eq!(largest[0], FolderSize { key: "Developer/huge/vendor".into(), items: 900 });
        assert!(largest.iter().all(|f| f.key != "Developer" && f.key != "Developer/huge"));
        assert!(index.handle().memory_estimate() > 900 * 100);
    }

    #[test]
    fn keys_and_absolute_paths() {
        let (_dir, root) = tree();
        let index = build(&root);
        assert_eq!(index.key(&root.join("Documents/notes.md")), "Documents/notes.md");
        assert_eq!(index.key(Path::new("/opt/homebrew")), "/opt/homebrew");
        assert_eq!(index.absolute("Documents"), root.join("Documents"));
        assert_eq!(index.absolute("/opt/homebrew"), PathBuf::from("/opt/homebrew"));
    }
}
