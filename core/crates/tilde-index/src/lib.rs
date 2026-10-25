//! The global filename index behind the Everywhere scope.
//!
//! First cut (see docs/SPEC.md §6.2): an in-memory index built by a background scan on
//! startup, matched with nucleo, re-ranked with filename, depth, recency and zoxide
//! boosts. Persistence and FSEvents catch-up come later.
//!
//! Paths are stored as keys relative to the home folder (`Developer/tilde/README.md`), or
//! absolute for anything outside it. Relative keys keep memory down and stop every query
//! from matching the home folder's own name.

mod walk;
mod zoxide;

pub use walk::{ScanConfig, scan};
pub use zoxide::{Zoxide, parse_scores};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Injector, Matcher, Nucleo, Utf32Str, Utf32String};
use tilde_query::{Candidate, FilterContext, Query};
use tilde_types::EntryKind;

/// How many top matches are re-ranked with boosts.
const RERANK_CANDIDATES: usize = 3000;
const ZOXIDE_REFRESH: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct IndexConfig {
    pub home: PathBuf,
    pub scan: ScanConfig,
    pub zoxide: bool,
}

/// What the index keeps per item besides its path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemMeta {
    pub kind: EntryKind,
    pub size: u64,
    pub modified: i64,
    pub child_count: Option<u32>,
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
}

pub struct GlobalIndex {
    nucleo: Nucleo<ItemMeta>,
    /// Kept for its live count of pushed items (the snapshot only updates on `tick`).
    counter: Injector<ItemMeta>,
    home: PathBuf,
    scanning: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    zoxide: Option<Zoxide>,
    /// zoxide scores by key.
    zoxide_scores: HashMap<String, f64>,
    zoxide_loaded: Instant,
    pattern_text: String,
    matcher: Matcher,
    name_matcher: Matcher,
    buf: Vec<char>,
}

impl GlobalIndex {
    /// Creates the index and starts scanning in the background.
    pub fn start(config: IndexConfig) -> GlobalIndex {
        let nucleo = Nucleo::new(Config::DEFAULT.match_paths(), Arc::new(|| {}), None, 1);
        let zoxide = config.zoxide.then(Zoxide::find).flatten();
        let mut index = GlobalIndex {
            counter: nucleo.injector(),
            nucleo,
            home: config.home.clone(),
            scanning: Arc::new(AtomicBool::new(true)),
            stop: Arc::new(AtomicBool::new(false)),
            zoxide,
            zoxide_scores: HashMap::new(),
            zoxide_loaded: Instant::now(),
            pattern_text: String::new(),
            matcher: Matcher::new(Config::DEFAULT.match_paths()),
            name_matcher: Matcher::new(Config::DEFAULT),
            buf: Vec::new(),
        };
        let zoxide_dirs = index.reload_zoxide();

        let injector = index.nucleo.injector();
        let (scanning, stop, home) = (index.scanning.clone(), index.stop.clone(), config.home.clone());
        std::thread::Builder::new()
            .name("tilde-index".into())
            .spawn(move || {
                let push = |path: &Path, meta: ItemMeta| {
                    let key = key_for(&home, path);
                    if !key.is_empty() {
                        injector.push(meta, |_, cols| cols[0] = Utf32String::from(key.as_str()));
                    }
                };
                scan(&config.scan, &stop, &|dir, e| {
                    push(
                        &dir.join(&e.name),
                        ItemMeta { kind: e.kind, size: e.size, modified: e.modified, child_count: e.child_count },
                    );
                });
                // zoxide folders the scan can't reach (hidden, excluded, outside home).
                for dir in zoxide_dirs.iter().filter(|d| !config.scan.covers(d)) {
                    if let Ok(md) = std::fs::metadata(dir)
                        && md.is_dir()
                    {
                        let modified = md
                            .modified()
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map_or(0, |d| d.as_secs() as i64);
                        push(dir, ItemMeta { kind: EntryKind::Folder, size: 0, modified, child_count: None });
                    }
                }
                scanning.store(false, Ordering::SeqCst);
            })
            .expect("spawn index thread");
        index
    }

    pub fn status(&self) -> IndexStatus {
        IndexStatus { items: self.counter.injected_items() as u64, scanning: self.scanning.load(Ordering::SeqCst) }
    }

    /// The key form of a path (relative to home, or absolute). Empty for home itself.
    pub fn key(&self, path: &Path) -> String {
        key_for(&self.home, path)
    }

    /// Absolute path for a key.
    pub fn absolute(&self, key: &str) -> PathBuf {
        if key.starts_with('/') { PathBuf::from(key) } else { self.home.join(key) }
    }

    /// Records a folder visit with zoxide, if enabled.
    pub fn record_visit(&self, path: &Path) {
        if let Some(z) = &self.zoxide {
            z.add(path);
        }
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
        if self.zoxide.is_some() && self.zoxide_loaded.elapsed() > ZOXIDE_REFRESH {
            self.reload_zoxide();
        }
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
        let ctx = FilterContext { now, cwd };
        let has_text = !query.text.is_empty();
        let name_pattern = has_text.then(|| Pattern::parse(&query.text, CaseMatching::Ignore, Normalization::Smart));

        let mut ranked: Vec<(u32, i64)> = Vec::new();
        for n in 0..snap.matched_item_count() {
            if has_text && ranked.len() >= RERANK_CANDIDATES {
                break;
            }
            let Some(item) = snap.get_matched_item(n) else { break };
            let meta = *item.data;
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
                if !has_text {
                    // Filters only: newest first.
                    return Some(meta.modified);
                }
                let path_score = snap.pattern().score(std::slice::from_ref(column), &mut self.matcher)? as i64;
                let name_score = name_pattern
                    .as_ref()
                    .and_then(|p| p.score(Utf32Str::new(name, &mut self.buf), &mut self.name_matcher))
                    .unwrap_or(0) as i64;
                let depth = key.matches('/').count() as i64;
                let length_penalty = key.len() as i64 / 10;
                let fresh = if now - meta.modified < 7 * 86_400 { 4 } else { 0 };
                let own = self.zoxide_scores.get(key).map_or(0.0, |z| 15.0 * (1.0 + z).ln());
                let inside = self.zoxide_scores.get(parent).map_or(0.0, |z| 5.0 * (1.0 + z).ln());
                Some(path_score + name_score + fresh + (own + inside) as i64 - 2 * depth - length_penalty)
            });
            if let Some(score) = score {
                ranked.push((n, score));
            }
        }
        ranked.sort_by_key(|&(_, s)| std::cmp::Reverse(s));
        ranked.truncate(limit);

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

    /// Loads zoxide scores. Returns the folders it knows about.
    fn reload_zoxide(&mut self) -> Vec<PathBuf> {
        self.zoxide_loaded = Instant::now();
        let Some(z) = &self.zoxide else { return Vec::new() };
        let scores = z.scores();
        self.zoxide_scores = scores.iter().map(|(p, s)| (key_for(&self.home, p), *s)).collect();
        scores.into_iter().map(|(p, _)| p).collect()
    }
}

impl Drop for GlobalIndex {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn key_for(home: &Path, path: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rel) => rel.to_string_lossy().into_owned(),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

/// Borrows a nucleo string as `&str` without allocating when it's ASCII.
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

    fn build(root: &Path) -> GlobalIndex {
        let config = IndexConfig {
            home: root.to_path_buf(),
            scan: ScanConfig {
                roots: vec![root.to_path_buf()],
                exclude_names: ["node_modules".to_string()].into(),
                exclude_paths: vec![],
                include_hidden: false,
            },
            zoxide: false,
        };
        let index = GlobalIndex::start(config);
        let t = Instant::now();
        while index.status().scanning {
            assert!(t.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(10));
        }
        index
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
