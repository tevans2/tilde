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
pub use live::{FolderSize, FolderTotal};
pub use walk::{ScanConfig, glob_match, scan, walk_parallel, walk_sequential};
pub use zoxide::{Zoxide, parse_scores};

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher, Nucleo, Utf32Str, Utf32String};
use tilde_fs::{FsEvent, SINCE_NOW, Watcher, current_event_id, watch};
use tilde_query::{Candidate, FilterContext, Query};
use tilde_types::EntryKind;

use crate::live::Shared;

/// Points for an item changed in the last week.
const FRESH_BONUS: i64 = 4;
/// How much more a file name can score than its own path, per word of the query and
/// per character: the name is the end of the path, and on its own its characters can earn
/// at most 2 bonus points more (the first doubled).
const NAME_SLACK_FIRST_CHAR: i64 = 4;
const NAME_SLACK_CHAR: i64 = 2;
/// Matches scored per task when searching in parallel.
const CHUNK: u32 = 2048;
/// Items added since the frecency table was made before it's made again.
const USAGE_REBUILD_AFTER: u32 = 20_000;
/// Scores decay slowly: derived ones are refreshed this often even without new uses.
const FRECENCY_REFRESH: Duration = Duration::from_secs(60);
/// zoxide's scores are re-read this often (a process launch).
const ZOXIDE_REFRESH: Duration = Duration::from_secs(60);
/// FSEvents coalescing window. The first change after a quiet moment is delivered at once
/// (`NoDefer`); changes right after it wait this long, gathered into one batch. Shorter
/// costs no more CPU applying changes (tilde-bench `live`), but each batch can re-run an
/// open search, so not much shorter.
const WATCH_LATENCY: Duration = Duration::from_millis(100);
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
    persist::save(path, config.fingerprint(), current_event_id(), items, &[])
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

/// What the index holds internally: everything ever added to the matcher (nucleo only
/// appends), the dead entries among them, and the per-folder tables.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexCounts {
    pub injected: u64,
    pub dead: u64,
    pub folder_tables: u64,
}

/// A folder inside an indexed one that the index doesn't enter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedFolder {
    pub path: PathBuf,
    /// As last measured.
    pub size: Option<u64>,
    /// Something inside changed since.
    pub stale: bool,
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

    /// Internals for benchmarks (`tilde-bench`).
    #[doc(hidden)]
    pub fn counts(&self) -> IndexCounts {
        let (injected, dead, folders) = self.shared.counts();
        IndexCounts { injected, dead, folder_tables: folders }
    }

    /// Rebuilds the matcher's list without dead items now (normally done once they're a
    /// fifth of it). For benchmarks (`tilde-bench`).
    #[doc(hidden)]
    pub fn compact(&self) {
        self.shared.compact();
    }

    /// Rough bytes of memory the index uses.
    pub fn memory_estimate(&self) -> u64 {
        self.shared.memory_estimate()
    }

    pub fn largest_folders(&self, limit: usize) -> Vec<FolderSize> {
        self.shared.largest_folders(limit)
    }

    /// The total size of everything inside `folder` (file lengths, hidden files
    /// included), if the index lists it: kept current as files change. Folders inside
    /// that the index doesn't enter (excluded, hidden, packages) count once measured
    /// (see [`IndexHandle::skipped_inside`]).
    pub fn folder_total(&self, folder: &Path) -> Option<FolderTotal> {
        self.shared.folder_total(&self.shared.key(folder))
    }

    /// The folders inside `folders`, at any depth, that the index doesn't enter, with
    /// their measured sizes: what [`IndexHandle::folder_total`] needs measured by walking
    /// them (those without a size, or out of date).
    pub fn skipped_inside(&self, folders: &[PathBuf]) -> Vec<SkippedFolder> {
        let keys: Vec<String> = folders.iter().map(|f| self.shared.key(f)).collect();
        self.shared
            .skipped_inside(&keys)
            .into_iter()
            .map(|(key, size, stale)| SkippedFolder { path: self.absolute(&key), size, stale })
            .collect()
    }

    /// Records the measured size of a folder the index doesn't enter, adding it to the
    /// totals above it. False if `folder` isn't one.
    pub fn set_skipped_size(&self, folder: &Path, size: u64) -> bool {
        self.shared.set_skipped_size(&self.shared.key(folder), size)
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

    /// The key form of a path (relative to home, or absolute). Empty for home itself.
    pub fn key(&self, path: &Path) -> String {
        self.shared.key(path)
    }

    /// Every indexed item `keep` accepts, as (absolute path, metadata). `keep` sees the
    /// key (relative to home, or absolute; [`IndexHandle::absolute`] makes it a path) and
    /// the metadata without anything being copied, so test the metadata first. Walks the
    /// whole index without the search lock.
    pub fn select(&self, keep: impl FnMut(&str, &ItemMeta) -> bool) -> Vec<(PathBuf, ItemMeta)> {
        self.shared.select(keep).into_iter().map(|(key, meta)| (self.absolute(&key), meta)).collect()
    }

    /// Visible files modified at or after `since` (seconds since the epoch), newest
    /// first, as (absolute path, metadata). Doesn't wait for the search lock.
    pub fn recent(&self, since: i64, limit: usize) -> Vec<(PathBuf, ItemMeta)> {
        self.recent_where(since, limit, |_, _| true)
    }

    /// Like [`IndexHandle::recent`], counting only items `keep` accepts (given the key and
    /// metadata, as for [`IndexHandle::select`]).
    pub fn recent_where(
        &self,
        since: i64,
        limit: usize,
        keep: impl Fn(&str, &ItemMeta) -> bool,
    ) -> Vec<(PathBuf, ItemMeta)> {
        self.shared.recent(since, limit, keep).into_iter().map(|(key, meta)| (self.absolute(&key), meta)).collect()
    }
}

pub struct GlobalIndex {
    /// Item IDs, matched by path (metadata is in `shared`, by ID).
    nucleo: Nucleo<u32>,
    /// Which compaction `nucleo` is from.
    generation: u64,
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
    /// The most frecency can add to any item's score (its own use and its folder's).
    usage_ceiling: i64,
    /// Frecency points by item ID, so searching needn't look keys up.
    usage: UsageById,
    scores_version: u64,
    scores_at: Instant,
    pattern_text: String,
    matcher: Matcher,
}

impl GlobalIndex {
    /// Loads the saved index or scans, then keeps the index current with FSEvents.
    /// `on_change` runs when the first load or scan finishes, after each batch of changes
    /// is applied, and when a matching pass that outlasted `search`'s wait completes (so
    /// slow searches still show up).
    pub fn start(config: IndexConfig, on_change: impl Fn() + Send + Sync + 'static) -> GlobalIndex {
        let on_change: Arc<dyn Fn() + Send + Sync> = Arc::new(on_change);
        let notify = on_change.clone();
        let make_nucleo: live::MakeNucleo = Box::new(move || {
            let notify = notify.clone();
            Nucleo::new(Config::DEFAULT.match_paths(), Arc::new(move || notify()), None, 1)
        });
        let nucleo = make_nucleo();
        let fingerprint = config.fingerprint();
        let cache = config.cache_path.clone().map(|p| (p, fingerprint));
        let shared = Arc::new(Shared::new(
            config.home.clone(),
            config.scan.clone(),
            config.max_items,
            cache,
            nucleo.injector(),
            make_nucleo,
        ));

        // A saved index is usable if it was built with these settings and FSEvents can
        // replay everything since (IDs only go backwards if its database was reset).
        let saved = config
            .cache_path
            .as_deref()
            .and_then(|p| persist::load(p, fingerprint))
            .filter(|s| s.last_event_id <= current_event_id());
        let resume_from = saved.map(|s| {
            shared.load(s.items, s.folders);
            shared.complete.store(true, Ordering::SeqCst);
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
            generation: 0,
            shared: shared.clone(),
            scanning: Arc::new(AtomicBool::new(resume_from.is_none())),
            _watcher: watcher,
            frecency: config.frecency.clone(),
            zoxide: config.zoxide.then(Zoxide::find).flatten(),
            zoxide_scores: HashMap::new(),
            zoxide_at: Instant::now(),
            scores: HashMap::new(),
            usage_ceiling: 0,
            usage: UsageById::default(),
            scores_version: u64::MAX,
            scores_at: Instant::now(),
            pattern_text: String::new(),
            matcher: Matcher::new(Config::DEFAULT.match_paths()),
        };
        let mut known_dirs = index.reload_zoxide();
        known_dirs.extend(config.frecency.as_ref().map(|f| f.lock().unwrap().folders()).unwrap_or_default());

        let scanning = index.scanning.clone();
        std::thread::Builder::new()
            .name("tilde-index".into())
            .spawn(move || {
                if resume_from.is_none() {
                    let stop = shared.stop.clone();
                    // Hidden entries too, for folder totals (`sync_dir` leaves them out of the index).
                    walk_parallel(config.scan.roots.clone(), true, &stop, &|dir, entries| {
                        shared.sync_dir(dir, &entries, false)
                    });
                    // Stopped because the index was dropped: not worth keeping.
                    let finished = !stop.load(Ordering::SeqCst) || shared.capped.load(Ordering::SeqCst);
                    shared.complete.store(finished, Ordering::SeqCst);
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
                            shared.compact_if_due();
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
    ///
    /// With text, nucleo matches whole paths and hands them over best path score first;
    /// each is then scored by Tilde (file name, recency, use, depth). Everything Tilde adds
    /// has a ceiling, so once an item's path score plus that ceiling can't beat the
    /// `limit`th best so far, no later item can either and the walk stops. The result is
    /// exactly what scoring every match would give.
    pub fn search(
        &mut self,
        query: &Query,
        cwd: Option<&str>,
        exclude_parent: Option<&str>,
        now: i64,
        limit: usize,
    ) -> Vec<GlobalHit> {
        self.search_with(query, cwd, exclude_parent, now, limit, Stop::WhenNothingCanWin)
    }

    /// `search` scoring every match: slower, the same results. For checking `search`
    /// against (`tilde-bench`).
    #[doc(hidden)]
    pub fn search_scoring_everything(
        &mut self,
        query: &Query,
        cwd: Option<&str>,
        exclude_parent: Option<&str>,
        now: i64,
        limit: usize,
    ) -> Vec<GlobalHit> {
        self.search_with(query, cwd, exclude_parent, now, limit, Stop::Never)
    }

    /// `search`, stopping as `stop` says (tests compare the ways).
    fn search_with(
        &mut self,
        query: &Query,
        cwd: Option<&str>,
        exclude_parent: Option<&str>,
        now: i64,
        limit: usize,
        stop: Stop,
    ) -> Vec<GlobalHit> {
        if query.is_empty() || limit == 0 {
            return Vec::new();
        }
        let shared = self.shared.clone();
        // Item IDs are only meaningful with the list they came from: if the index was
        // compacted meanwhile, start again with the new list (rare).
        let items = loop {
            self.take_compacted();
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
            let items = shared.items();
            if items.generation == self.generation {
                break items;
            }
        };
        let snap = self.nucleo.snapshot();
        let ctx = FilterContext { now, cwd };
        let has_text = !query.text.is_empty();
        let name_pattern = has_text.then(|| Pattern::parse(&query.text, CaseMatching::Ignore, Normalization::Smart));

        let best = if has_text {
            let ranking = Ranking {
                snap,
                items: &items,
                query,
                ctx,
                exclude_parent,
                name_pattern: name_pattern.as_ref().expect("text"),
                name_slack: name_slack(&query.text),
                scores: &self.scores,
                usage: &self.usage,
                usage_ceiling: self.usage_ceiling,
                now,
                limit,
                stop,
                bar: AtomicI64::new(i64::MIN),
            };
            ranking.run()
        } else {
            Vec::new()
        };
        let ranked: Vec<(u32, i64)> = if has_text {
            best
        } else {
            // Filters only: newest first. Only the best `limit` are kept, picked out before
            // sorting (this can rank most of the index).
            let mut ranked = filtered_newest(snap, &items, query, &ctx, exclude_parent);
            if ranked.len() > limit {
                ranked.select_nth_unstable_by_key(limit - 1, |&(_, s)| Reverse(s));
                ranked.truncate(limit);
            }
            ranked.sort_by_key(|&(_, s)| Reverse(s));
            ranked
        };

        let pattern = snap.pattern().column_pattern(0);
        let mut indices = Vec::new();
        ranked
            .into_iter()
            .filter_map(|(n, score)| {
                let item = snap.get_matched_item(n)?;
                let meta = items.live(*item.data)?;
                let column = &item.matcher_columns[0];
                indices.clear();
                if has_text {
                    pattern.indices(column.slice(..), &mut self.matcher, &mut indices);
                    indices.sort_unstable();
                    indices.dedup();
                }
                Some(GlobalHit { key: column.to_string(), meta, indices: indices.clone(), score })
            })
            .collect()
    }

    /// Switches to the list `Shared::compact` rebuilt, if there's one.
    fn take_compacted(&mut self) {
        if let Some((nucleo, generation)) = self.shared.take_compacted() {
            self.nucleo = nucleo;
            self.generation = generation;
            // The new list has no pattern yet, and items were renumbered.
            self.pattern_text.clear();
            self.scores_version = u64::MAX;
        }
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
            // Items added since the table was made are looked up by key; remake it when
            // there are many.
            if self.shared.next_id() > self.usage.upto + USAGE_REBUILD_AFTER {
                self.usage = UsageById::build(&self.shared, &self.scores);
            }
            return;
        }
        let mut scores = self.zoxide_scores.clone();
        if let Some(f) = &self.frecency {
            let now = now();
            for (path, s) in f.lock().unwrap().scores(now) {
                *scores.entry(self.shared.key(Path::new(path))).or_default() += s;
            }
        }
        let most = scores.values().copied().fold(0.0f64, f64::max);
        self.usage_ceiling = usage_points(most, most).ceil() as i64 + 1;
        self.usage = UsageById::build(&self.shared, &scores);
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

/// When a search stops walking nucleo's matches.
#[derive(Debug, Clone, Copy)]
enum Stop {
    /// Once no remaining match could make the list (exact).
    WhenNothingCanWin,
    /// Never: every match is scored.
    Never,
    /// After this many matches pass the filters, in one pass (how search used to stop,
    /// at 3,000).
    #[allow(dead_code)]
    AfterScoring(usize),
}

/// What frecency adds: for the item's own uses and its folder's.
fn usage_points(own: f64, folder: f64) -> f64 {
    15.0 * (1.0 + own.max(0.0)).ln() + 5.0 * (1.0 + folder.max(0.0)).ln()
}

/// How much more a file name can score than its path against `text` (nucleo sums its
/// words' scores; negated words score nothing).
fn name_slack(text: &str) -> i64 {
    text.split_whitespace()
        .filter(|w| !w.starts_with('!'))
        .map(|w| NAME_SLACK_FIRST_CHAR + NAME_SLACK_CHAR * (w.chars().count() as i64 - 1))
        .sum()
}

/// Frecency points (`usage_points`, as whole points) by item ID for IDs below `upto`:
/// items with uses of their own, and items directly inside folders with uses.
#[derive(Default)]
struct UsageById {
    points: Vec<i32>,
    upto: u32,
}

impl UsageById {
    fn build(shared: &Shared, scores: &HashMap<String, f64>) -> UsageById {
        let (own, inside, upto) = shared.ids_for(scores);
        let mut points = vec![0i32; upto as usize];
        for &id in own.keys().chain(inside.keys()) {
            if let Some(p) = points.get_mut(id as usize) {
                let s = |m: &HashMap<u32, f64>| m.get(&id).copied().unwrap_or(0.0);
                *p = usage_points(s(&own), s(&inside)) as i32;
            }
        }
        UsageById { points, upto }
    }
}

/// One search's scoring of nucleo's matches, shared by the threads doing it.
struct Ranking<'a> {
    snap: &'a nucleo::Snapshot<u32>,
    items: &'a live::Items<'a>,
    query: &'a Query,
    ctx: FilterContext<'a>,
    exclude_parent: Option<&'a str>,
    name_pattern: &'a Pattern,
    name_slack: i64,
    scores: &'a HashMap<String, f64>,
    usage: &'a UsageById,
    usage_ceiling: i64,
    now: i64,
    limit: usize,
    stop: Stop,
    /// A score the `limit`th best is known to reach (from any thread's full list).
    bar: AtomicI64,
}

/// The best results so far, the worst on top; ties go to the earlier match.
type Best = BinaryHeap<Reverse<(i64, Reverse<u32>)>>;

impl Ranking<'_> {
    /// The best `limit` matches, best first, as (match number, score).
    fn run(&self) -> Vec<(u32, i64)> {
        use rayon::prelude::*;
        let count = self.snap.matched_item_count();
        // Counting what's been scored needs one pass in order.
        let chunk = if matches!(self.stop, Stop::AfterScoring(_)) { count.max(1) } else { CHUNK };
        let ranges: Vec<std::ops::Range<u32>> =
            (0..count).step_by(chunk as usize).map(|s| s..(s + chunk).min(count)).collect();
        // The first chunk (the best path scores) on its own first: it sets the bar, so the
        // rest mostly stop at their first match instead of all scoring at once.
        let Some((first, rest)) = ranges.split_first() else { return Vec::new() };
        let mut found = vec![with_matchers(|paths, names, buf| self.rank(first.clone(), paths, names, buf))];
        found.par_extend(
            rest.par_iter().map(|range| with_matchers(|paths, names, buf| self.rank(range.clone(), paths, names, buf))),
        );
        let mut all: Vec<(i64, Reverse<u32>)> = found.into_iter().flatten().map(|Reverse(r)| r).collect();
        all.sort_unstable_by(|a, b| b.cmp(a));
        all.truncate(self.limit);
        all.into_iter().map(|(score, Reverse(n))| (n, score)).collect()
    }

    /// Scores matches `range` (in nucleo's order, best path score first).
    fn rank(&self, range: std::ops::Range<u32>, paths: &mut Matcher, names: &mut Matcher, buf: &mut Vec<char>) -> Best {
        let mut best = Best::with_capacity(self.limit + 1);
        let mut scored = 0usize;
        let exact = matches!(self.stop, Stop::WhenNothingCanWin);
        for n in range {
            let Some(item) = self.snap.get_matched_item(n) else { break };
            let column = &item.matcher_columns[0];
            let Some(path_score) = self.snap.pattern().score(std::slice::from_ref(column), paths) else { continue };
            let path_score = path_score as i64;
            // What the `limit`th best is known to reach: what it takes to make the list.
            let bar = self.bar.load(Ordering::Relaxed);
            // Nothing from here on can make the list: its name can't add more than its path
            // plus the slack, and the rest have ceilings.
            match self.stop {
                Stop::WhenNothingCanWin
                    if 2 * path_score + self.name_slack + FRESH_BONUS + self.usage_ceiling < bar =>
                {
                    break;
                }
                Stop::AfterScoring(most) if scored >= most => break,
                _ => {}
            }
            let Some(meta) = self.items.live(*item.data) else { continue };
            let score = with_str(column, |key| {
                let (parent, name) = key.rsplit_once('/').unwrap_or(("", key));
                if self.exclude_parent == Some(parent) {
                    return None;
                }
                let candidate =
                    Candidate { name, kind: meta.kind, size: meta.size, modified: meta.modified, parent: Some(parent) };
                if !self.query.filters.iter().all(|f| f.accepts(&candidate, &self.ctx)) {
                    return None;
                }
                let depth = key.matches('/').count() as i64;
                let length_penalty = key.len() as i64 / 10;
                let fresh = if self.now - meta.modified < 7 * 86_400 { FRESH_BONUS } else { 0 };
                let usage = match self.usage.points.get(meta.id as usize) {
                    Some(&p) if !meta.extra && !key.starts_with('/') => p as i64,
                    _ => {
                        let s = |k: &str| self.scores.get(k).copied().unwrap_or(0.0);
                        usage_points(s(key), s(parent)) as i64
                    }
                };
                let known = path_score + fresh + usage - 2 * depth - length_penalty;
                // Even the best name it could have wouldn't make the list: skip matching it.
                let local = if best.len() == self.limit {
                    best.peek().map_or(i64::MIN, |Reverse((w, _))| *w)
                } else {
                    i64::MIN
                };
                if exact && known + path_score + self.name_slack < bar.max(local) {
                    return None;
                }
                let name_score = self.name_pattern.score(Utf32Str::new(name, buf), names).unwrap_or(0) as i64;
                Some(known + name_score)
            });
            if let Some(score) = score {
                scored += 1;
                best.push(Reverse((score, Reverse(n))));
                if best.len() > self.limit {
                    best.pop();
                }
                if best.len() == self.limit
                    && let Some(Reverse((worst, _))) = best.peek()
                {
                    self.bar.fetch_max(*worst, Ordering::Relaxed);
                }
            }
        }
        best
    }
}

/// Runs `f` with this thread's matchers (paths, names) and scratch buffer. Matchers are
/// big to make, so each thread keeps its own across searches.
fn with_matchers<R>(f: impl FnOnce(&mut Matcher, &mut Matcher, &mut Vec<char>) -> R) -> R {
    thread_local! {
        static MATCHERS: std::cell::RefCell<(Matcher, Matcher, Vec<char>)> = std::cell::RefCell::new((
            Matcher::new(Config::DEFAULT.match_paths()),
            Matcher::new(Config::DEFAULT),
            Vec::new(),
        ));
    }
    MATCHERS.with_borrow_mut(|(paths, names, buf)| f(paths, names, buf))
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

/// Filters only (no text): every live item that passes, scored by modification time so the
/// newest come first. This visits the whole index, so it runs in parallel.
fn filtered_newest(
    snap: &nucleo::Snapshot<u32>,
    items: &live::Items<'_>,
    query: &Query,
    ctx: &FilterContext<'_>,
    exclude_parent: Option<&str>,
) -> Vec<(u32, i64)> {
    use rayon::prelude::*;
    (0..snap.matched_item_count())
        .into_par_iter()
        .filter_map(|n| {
            let item = snap.get_matched_item(n)?;
            let meta = items.live(*item.data)?;
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

/// Borrows a nucleo string as `&str` without allocating when it's ASCII.
pub(crate) fn with_str<R>(s: &Utf32String, f: impl FnOnce(&str) -> R) -> R {
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

    /// The lookups that walk the whole index give exactly what copying every key, filtering
    /// and sorting gave: same items, same order (ties by position), at any limit.
    #[test]
    fn whole_index_lookups_match_a_plain_filter_and_sort() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        fs::create_dir(&home).unwrap();
        let names = ["report.pdf", ".hidden", "café notes.md", "IMG_0001.HEIC", "Résumé.pages", "x.rs"];
        let folders = ["Documents", "Developer/.git", "Pictures/Photos Library.photoslibrary", "Music", "日本"];
        let mut items = Vec::new();
        for i in 0..3_000u32 {
            let kind = match i % 11 {
                0 => EntryKind::Folder,
                1 => EntryKind::Package,
                _ => EntryKind::File,
            };
            let name = if i % 97 == 0 { "Photos Library.photoslibrary" } else { names[i as usize % names.len()] };
            let key = format!("{}/{i}/{name}", folders[(i as usize / 7) % folders.len()]);
            // Few distinct dates, so many ties.
            let meta = ItemMeta {
                id: i,
                kind,
                size: (i as u64 * 7919) % 5_000_000,
                modified: 1_000 + (i as i64 % 40),
                child_count: None,
                extra: false,
            };
            items.push((key, meta));
        }
        let mut config = config(&home);
        config.cache_path = Some(dir.path().join("index.bin"));
        write_saved(&config, &items).unwrap();
        let index = start_and_wait(config);
        let shared = &index.shared;

        let plain_recent = |since: i64, limit: usize| {
            let mut found: Vec<(String, ItemMeta)> = shared
                .snapshot()
                .into_iter()
                .filter(|(key, m)| {
                    let mut folders = key.split('/');
                    folders.next_back();
                    m.modified >= since
                        && matches!(m.kind, EntryKind::File | EntryKind::Package)
                        && !(m.kind == EntryKind::Package && key.ends_with("library"))
                        && !key.split('/').any(|p| p.starts_with('.'))
                        && !folders.any(tilde_fs::is_package_name)
                })
                .collect();
            found.sort_by_key(|(_, m)| std::cmp::Reverse(m.modified));
            found.truncate(limit);
            found
        };
        for (since, limit) in [(0, 10_000), (0, 1), (1_020, 25), (1_039, 3), (1_000, 0), (5_000, 10)] {
            assert_eq!(
                shared.recent(since, limit, |_, _| true),
                plain_recent(since, limit),
                "since {since}, limit {limit}"
            );
        }
        let pdfs = shared.recent(0, 50, |key, _| key.ends_with(".pdf"));
        assert_eq!(
            pdfs,
            plain_recent(0, 10_000).into_iter().filter(|(k, _)| k.ends_with(".pdf")).take(50).collect::<Vec<_>>()
        );

        let big: Vec<_> = shared.snapshot().into_iter().filter(|(_, m)| m.size > 4_000_000).collect();
        assert_eq!(shared.select(|_, m| m.size > 4_000_000), big);
        let accented: Vec<_> = shared.snapshot().into_iter().filter(|(k, _)| k.contains('é')).collect();
        assert!(!accented.is_empty());
        assert_eq!(shared.select(|k, _| k.contains('é')), accented);
    }

    /// A saved index of `items` under a fresh home, with frecency `uses` (key, opens).
    fn loaded(items: &[(String, ItemMeta)], uses: &[(&str, usize)]) -> (tempfile::TempDir, GlobalIndex) {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        fs::create_dir(&home).unwrap();
        let now = super::now() as f64;
        let mut frecency = Frecency::in_memory();
        for (key, opens) in uses {
            for _ in 0..*opens {
                frecency.record(&home.join(key), Use::Open, now, false);
            }
        }
        let mut config = config(&home);
        config.frecency = Some(Arc::new(Mutex::new(frecency)));
        config.cache_path = Some(dir.path().join("index.bin"));
        write_saved(&config, items).unwrap();
        (dir, start_and_wait(config))
    }

    /// Stopping early returns exactly what scoring every match returns: same items,
    /// same scores, same order, whatever the query and limit.
    #[test]
    fn stopping_early_finds_the_same_results_as_scoring_everything() {
        let words = ["report", "repo", "render", "invoice", "notes", "photo", "receipt", "readme", "config", "café"];
        let mut items = Vec::new();
        for i in 0..20_000u32 {
            let w = |k: u32| words[(k as usize * 7 + i as usize) % words.len()];
            let key = format!(
                "{}/{}-{}/{}{}/{}_{i}.{}",
                w(1),
                w(2),
                i % 13,
                w(3),
                i % 7,
                w(4),
                ["pdf", "rs", "md"][i as usize % 3]
            );
            let meta = ItemMeta {
                id: i,
                kind: EntryKind::File,
                size: 1,
                modified: (i as i64) * 100,
                child_count: None,
                extra: false,
            };
            items.push((key, meta));
        }
        // Heavy use of a few items deep in the list, and of some folders.
        let uses: Vec<(&str, usize)> = [items[19_000].0.as_str(), items[12_345].0.as_str(), items[7].0.as_str()]
            .into_iter()
            .zip([40, 9, 3])
            .collect();
        let (_dir, mut index) = loaded(&items, &uses);
        let mut old_cutoff_wrong = 0;
        for text in ["r", "re", "rep", "report", "repo notes", "café", "invoice 2024", "zz", "readme md", "ren"] {
            for limit in [1, 10, 200] {
                let q = Query::parse(text);
                let summary = |h: Vec<GlobalHit>| h.into_iter().map(|h| (h.key, h.score)).collect::<Vec<_>>();
                let early = summary(index.search_with(&q, None, None, 1_000_000, limit, Stop::WhenNothingCanWin));
                let all = summary(index.search_with(&q, None, None, 1_000_000, limit, Stop::Never));
                assert_eq!(early, all, "“{text}”, limit {limit}");
                let capped = summary(index.search_with(&q, None, None, 1_000_000, limit, Stop::AfterScoring(3_000)));
                if capped != all {
                    old_cutoff_wrong += 1;
                }
            }
        }
        assert!(old_cutoff_wrong > 0, "the data should show the old cutoff's mistakes");
    }

    /// The case the old cutoff got wrong: a file you use, whose path matches less well
    /// than thousands of others, is still found and ranked by its use.
    #[test]
    fn a_file_you_use_is_found_behind_thousands_of_better_path_matches() {
        let meta = |id| ItemMeta { id, kind: EntryKind::File, size: 1, modified: 0, child_count: None, extra: false };
        let target = "Archive/2019/scans/plan-final.pdf";
        let mut items = vec![(target.to_string(), meta(0))];
        // 5,000 paths that match "plan" better (in a short folder name) but don't name it.
        for i in 1..5_000u32 {
            items.push((format!("plan/x{i}.txt"), meta(i)));
        }
        let (_dir, mut index) = loaded(&items, &[(target, 50)]);
        let q = Query::parse("plan");
        let rank = |hits: Vec<GlobalHit>| hits.iter().position(|h| h.key == target);
        let now = rank(index.search(&q, None, None, 1_000_000, 10));
        let before = rank(index.search_with(&q, None, None, 1_000_000, 10, Stop::AfterScoring(3_000)));
        assert_eq!(before, None, "the old cutoff never scored it");
        assert_eq!(now, Some(0), "used fifty times, with the word in its name: first");
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
        // Each folder that changed, as FSEvents reports them (the watcher may have listed
        // a new folder before what's in it arrived).
        index.shared.apply(&[
            changed(root.join("Documents")),
            changed(root.join("Documents/Archive")),
            changed(root.join("Documents/Archive/2025")),
        ]);
        assert_eq!(
            search_keys(&mut index, "report"),
            ["Documents/Archive/2025/old-report.txt", "Documents/fresh-report.txt"]
        );
        assert_eq!(index.status().items, before + 4);

        // A changed file is updated in place: same item, nothing added or marked dead.
        let id = index.search(&Query::parse("fresh-report"), None, None, 0, 10)[0].meta.id;
        let counts = index.handle().counts();
        fs::write(root.join("Documents/fresh-report.txt"), b"longer contents").unwrap();
        index.shared.apply(&[changed(root.join("Documents"))]);
        let hits = index.search(&Query::parse("fresh-report"), None, None, 0, 10);
        assert_eq!(hits.len(), 1);
        assert_eq!((hits[0].meta.id, hits[0].meta.size), (id, 15));
        assert_eq!(index.handle().counts(), counts);
        // Filters and lookups see the new size at once.
        assert_eq!(search_keys(&mut index, "size:>10"), ["Documents/fresh-report.txt"]);
        assert_eq!(index.handle().select(|_, m| m.size == 15).len(), 1);

        // A file replaced by a folder of the same name is replaced in the index too.
        fs::remove_file(root.join("Documents/notes.md")).unwrap();
        fs::create_dir(root.join("Documents/notes.md")).unwrap();
        index.shared.apply(&[changed(root.join("Documents"))]);
        let hits = index.search(&Query::parse("notes.md"), None, None, 0, 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].meta.kind, EntryKind::Folder);
        assert_eq!(index.handle().counts().dead, counts.dead + 1);

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
    fn compacting_drops_dead_items_and_keeps_everything_else() {
        let (_dir, root) = tree();
        let mut index = build(&root);
        // Dead items: files that come and go (fewer than compacting on its own needs, as the
        // watcher applies these too).
        let churn = root.join("Documents/churn");
        fs::create_dir(&churn).unwrap();
        for round in 0..2 {
            for i in 0..400 {
                fs::write(churn.join(format!("tmp-{round}-{i}.txt")), b"x").unwrap();
            }
            index.shared.apply(&[changed(churn.clone())]);
            for i in 0..400 {
                fs::remove_file(churn.join(format!("tmp-{round}-{i}.txt"))).unwrap();
            }
            index.shared.apply(&[changed(churn.clone())]);
        }
        let before = index.handle().counts();
        assert_eq!(before.dead, 800);
        assert!(!index.shared.compact_if_due(), "too few to bother");
        let live = index.status().items;
        let snapshot = index.shared.snapshot();
        let found = search_keys(&mut index, "swift");

        index.shared.compact();
        let after = index.handle().counts();
        assert_eq!((after.injected, after.dead), (live, 0));
        assert_eq!(index.status().items, live);
        // The same items, metadata and search results, renumbered in order.
        let renumbered = index.shared.snapshot();
        let strip = |items: Vec<(String, ItemMeta)>| -> Vec<(String, ItemMeta)> {
            items.into_iter().map(|(k, m)| (k, ItemMeta { id: 0, ..m })).collect()
        };
        assert_eq!(strip(renumbered.clone()), strip(snapshot));
        assert!(renumbered.iter().enumerate().all(|(i, (_, m))| m.id == i as u32));
        assert_eq!(search_keys(&mut index, "swift"), found);

        // The folder tables still line up: edits, removals and additions apply as before.
        fs::write(root.join("Documents/notes.md"), b"edited!").unwrap();
        fs::remove_file(root.join("Documents/Finance/2026/invoice-oct.pdf")).unwrap();
        fs::write(root.join("Documents/Finance/2026/invoice-nov.pdf"), b"x").unwrap();
        index.shared.apply(&[changed(root.join("Documents")), changed(root.join("Documents/Finance/2026"))]);
        let notes = index.search(&Query::parse("notes.md"), None, None, 0, 10);
        assert_eq!((notes.len(), notes[0].meta.size), (1, 7));
        assert_eq!(search_keys(&mut index, "invoice"), ["Documents/Finance/2026/invoice-nov.pdf"]);
        assert_eq!(index.handle().counts().dead, 1);
    }

    #[test]
    fn searching_while_compacting_never_mixes_up_items() {
        // Each item's size is its key's length, so a hit with another item's metadata shows.
        let items: Vec<(String, ItemMeta)> = (0..20_000)
            .map(|i| {
                let key = format!("folder{}/report-{i}.txt", i % 50);
                let meta = ItemMeta {
                    id: 0,
                    kind: EntryKind::File,
                    size: key.len() as u64,
                    modified: 0,
                    child_count: None,
                    extra: false,
                };
                (key, meta)
            })
            .collect();
        let (dir, mut index) = loaded(&items, &[]);
        let shared = index.shared.clone();
        let home = dir.path().join("home");
        let compacting = std::thread::spawn(move || {
            // Folders that don't exist on disk: each change kills 400 items, then compacts.
            for f in 0..40 {
                shared.apply(&[changed(home.join(format!("folder{f}")))]);
                shared.compact();
            }
        });
        let mut searches = 0;
        while !compacting.is_finished() || searches == 0 {
            for text in ["report", "rep 1", "folder7 txt"] {
                for hit in index.search(&Query::parse(text), None, None, 0, 50) {
                    assert_eq!(hit.meta.size, hit.key.len() as u64, "{hit:?}");
                }
                searches += 1;
            }
        }
        compacting.join().unwrap();
        assert_eq!(index.handle().counts().injected, 4_000);
        assert_eq!(index.handle().select(|k, _| k.starts_with("folder45/")).len(), 400);
    }

    #[test]
    fn frecency_still_counts_after_compacting() {
        let file = |key: String| {
            (key, ItemMeta { id: 0, kind: EntryKind::File, size: 1, modified: 0, child_count: None, extra: false })
        };
        // Decoys in two folders, then the file you use, which compacting renumbers.
        let items: Vec<(String, ItemMeta)> = (0..3_000)
            .map(|i| file(format!("{}/x{i}.txt", if i < 1_500 { "plan" } else { "planb" })))
            .chain([file("Archive/2019/scans/plan-final.pdf".into())])
            .collect();
        let (dir, mut index) = loaded(&items, &[("Archive/2019/scans/plan-final.pdf", 50)]);
        let first = |index: &mut GlobalIndex| index.search(&Query::parse("plan"), None, None, 0, 10)[0].clone();
        assert_eq!(first(&mut index).meta.id, 3_000);

        // The first folder is gone (it never existed on disk): its 1,500 items die.
        index.shared.apply(&[changed(dir.path().join("home/plan"))]);
        assert!(index.shared.compact_if_due());
        assert_eq!(index.handle().counts().injected, 1_501);
        let hit = first(&mut index);
        assert_eq!((hit.key.as_str(), hit.meta.id), ("Archive/2019/scans/plan-final.pdf", 1_500));
    }

    #[test]
    fn folder_totals_match_a_disk_walk_and_follow_changes() {
        let (_dir, root) = tree();
        // Things the index doesn't hold: hidden files and folders, an excluded folder
        // (node_modules, from `tree`), a package.
        fs::create_dir_all(root.join("Developer/projects/tilde/.git/objects")).unwrap();
        fs::write(root.join("Developer/projects/tilde/.git/objects/blob"), vec![0u8; 5_000]).unwrap();
        fs::write(root.join("Documents/.DS_Store"), vec![0u8; 6_148]).unwrap();
        fs::create_dir_all(root.join("Documents/Old.app/Contents")).unwrap();
        fs::write(root.join("Documents/Old.app/Contents/binary"), vec![0u8; 700]).unwrap();
        fs::write(root.join("Documents/notes.md"), vec![0u8; 300]).unwrap();
        let cache = tempfile::tempdir().unwrap();
        let mut config = config(&root);
        config.cache_path = Some(cache.path().join("index.bin"));
        let index = start_and_wait(config.clone());
        let handle = index.handle();

        let walk = |p: &Path| tilde_fs::folder_size(p, 0, &|| false, &|_, _| {}).unwrap();
        let measure = |only_stale: bool| {
            for s in handle.skipped_inside(std::slice::from_ref(&root)) {
                if !only_stale || s.stale || s.size.is_none() {
                    assert!(handle.set_skipped_size(&s.path, walk(&s.path)));
                }
            }
        };
        let folders = ["", "Documents", "Developer", "Developer/projects/tilde", "Documents/Finance/2026"];
        let check = |handle: &IndexHandle| {
            for f in folders.iter().map(|f| root.join(f)).filter(|f| f.exists()) {
                assert_eq!(handle.folder_total(&f), Some(FolderTotal { bytes: walk(&f), unmeasured: 0 }), "{f:?}");
            }
        };

        // Not measured yet: the totals say what they're missing.
        let skipped: Vec<PathBuf> =
            handle.skipped_inside(std::slice::from_ref(&root)).into_iter().map(|s| s.path).collect();
        assert_eq!(skipped.len(), 3, "{skipped:?}");
        assert_eq!(handle.folder_total(&root).unwrap().unmeasured, 3);
        assert_eq!(handle.folder_total(&root.join("Documents")).unwrap().unmeasured, 1);
        assert_eq!(handle.folder_total(&root.join("node_modules")), None, "not listed by the index");
        measure(false);
        check(&handle);

        // Changes: an edit, a new hidden file, a removed folder, a change inside a skipped
        // folder.
        fs::write(root.join("Documents/notes.md"), vec![0u8; 10_000]).unwrap();
        fs::write(root.join("Documents/.hidden-new"), b"abc").unwrap();
        fs::remove_dir_all(root.join("Documents/Finance")).unwrap();
        index.shared.apply(&[changed(root.join("Documents")), changed(root.join("Documents/Finance/2026"))]);
        fs::write(root.join("node_modules/left-pad/more.js"), vec![0u8; 999]).unwrap();
        index.shared.apply(&[changed(root.join("node_modules/left-pad"))]);
        let stale: Vec<PathBuf> = handle
            .skipped_inside(std::slice::from_ref(&root))
            .into_iter()
            .filter(|s| s.stale)
            .map(|s| s.path)
            .collect();
        assert_eq!(stale, [root.join("node_modules")]);
        measure(true);
        check(&handle);

        // Saved and loaded: the same totals, without measuring again.
        handle.save().unwrap().unwrap();
        drop(index);
        let index = start_and_wait(config);
        check(&index.handle());
    }

    #[test]
    fn an_event_id_is_recorded_once_its_batch_is_applied() {
        let (_dir, root) = tree();
        let mut index = build(&root);
        let before = index.shared.last_event_id.load(Ordering::SeqCst);
        fs::write(root.join("Documents/late.txt"), b"x").unwrap();
        let id = before + 1_000_000;
        index.shared.apply(&[FsEvent { path: root.join("Documents"), flags: 0, id }]);
        assert_eq!(index.shared.last_event_id.load(Ordering::SeqCst), id);
        assert_eq!(search_keys(&mut index, "late"), ["Documents/late.txt"]);
    }

    #[test]
    fn a_scan_stopped_part_way_is_never_saved() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        for i in 0..300 {
            let deep = root.join(format!("d{i}/a/b"));
            fs::create_dir_all(&deep).unwrap();
            for f in 0..10 {
                fs::write(deep.join(format!("f{f}")), b"x").unwrap();
            }
        }
        let cache = tempfile::tempdir().unwrap();
        let mut config = config(&root);
        config.cache_path = Some(cache.path().join("index.bin"));
        // Dropped straight away: the scan stops wherever it is.
        drop(GlobalIndex::start(config.clone(), || {}));
        std::thread::sleep(Duration::from_millis(300));
        // Either nothing was saved, or everything was.
        if let Some(saved) = persist::load(&cache.path().join("index.bin"), config.fingerprint()) {
            assert_eq!(saved.items.len(), 300 * 13);
        }
    }

    #[test]
    fn renaming_a_folder_of_ten_thousand_files() {
        let (_dir, root) = tree();
        let big = root.join("Documents/Scans");
        fs::create_dir(&big).unwrap();
        for i in 0..10_000 {
            fs::write(big.join(format!("scan-{i:05}.pdf")), vec![0u8; i % 7]).unwrap();
        }
        let mut index = build(&root);
        let before = index.status().items;
        let total = index.handle().folder_total(&root).unwrap();

        fs::rename(&big, root.join("Documents/Scans 2019")).unwrap();
        // What FSEvents reports: the folder's parent, and the folder at both names.
        index.shared.apply(&[
            changed(root.join("Documents")),
            changed(big.clone()),
            changed(root.join("Documents/Scans 2019")),
        ]);
        assert_eq!(index.status().items, before);
        assert_eq!(index.handle().folder_total(&root), Some(total));
        let hits = index.search(&Query::parse("scans 2019 scan-04321"), None, None, 0, 5);
        assert_eq!(hits[0].key, "Documents/Scans 2019/scan-04321.pdf");
        assert!(index.handle().select(|k, _| k.starts_with("Documents/Scans/")).is_empty());
    }

    #[test]
    fn a_folder_that_cant_be_read_keeps_what_was_indexed() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, root) = tree();
        let mut index = build(&root);
        let locked = root.join("Documents/Finance/2026");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        index.shared.apply(&[changed(locked.clone()), changed(root.join("Documents/Finance"))]);
        let found = search_keys(&mut index, "invoice");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(found, ["Documents/Finance/2026/invoice-oct.pdf"]);
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
