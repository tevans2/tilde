//! `text:` searches: a job per query, on its own threads at low priority, streaming hits
//! back to the engine worker. A newer query (or leaving `text:`) cancels it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use rayon::prelude::*;
use tilde_contents::{Match, Needle, ScanOptions};
use tilde_query::{Candidate, FilterContext, Kind, Query, kind_for};
use tilde_types::EntryKind;

use crate::engine::Command;
use crate::index::IndexService;

/// Shortest text searched for.
pub(crate) const MIN_CHARS: usize = 3;
/// Typing pauses this long before a search starts.
const SETTLE: Duration = Duration::from_millis(250);
/// Most results for one search.
const MAX_HITS: usize = 300;
/// Files outside the open folder read when Spotlight isn't used.
const RECENT_FILES: usize = 4_000;
/// …each at most this big.
const RECENT_MAX_SIZE: u64 = 1 << 20;
/// Most Spotlight results looked at.
const SPOTLIGHT_LIMIT: usize = 200;
/// Hits are sent in batches, at least this often while there are some.
const BATCH_EVERY: Duration = Duration::from_millis(150);

/// A file whose contents match.
#[derive(Debug, Clone)]
pub(crate) struct ContentHit {
    pub path: PathBuf,
    pub kind: EntryKind,
    pub size: u64,
    pub modified: i64,
    /// The first matching line; `None` for Spotlight results it can't read (PDFs…).
    pub found: Option<Match>,
    /// Outside the open folder.
    pub outside: bool,
}

/// What to search for, and where.
pub(crate) struct Job {
    pub id: u64,
    pub needle: String,
    pub folder: PathBuf,
    pub home: PathBuf,
    /// Names and filters results must also match.
    pub query: Query,
    pub hidden: bool,
    /// `text!:`: ignored files and build folders too.
    pub all: bool,
    pub spotlight: bool,
    /// Folder names not entered (`[cleanup] build_folders`).
    pub skip: Vec<String>,
}

/// Runs `job` on the calling thread, sending `Command::ContentsFound` batches and a last
/// one with `done` set. Returns early (sending nothing more) once `current` moves on.
pub(crate) fn run(job: Job, index: Option<Arc<IndexService>>, current: Arc<AtomicU64>, tx: Sender<Command>) {
    let cancelled = || current.load(Ordering::SeqCst) != job.id;
    std::thread::sleep(SETTLE);
    if cancelled() {
        return;
    }
    tilde_contents::lower_priority();
    let Some(needle) = Needle::new(&job.needle) else { return };
    let now = now();
    let full = AtomicBool::new(false);
    let mut out = Batcher { id: job.id, tx: &tx, pending: Vec::new(), sent: Instant::now(), total: 0, full: &full };

    // The open folder and below.
    let mut skip = if job.all { Vec::new() } else { job.skip.clone() };
    // From home, ~/Library is app data and caches, not your files (and most of the files).
    if job.folder == job.home {
        skip.push("Library".into());
    }
    let options = ScanOptions { hidden: job.hidden || job.all, ignored: job.all, skip, ..Default::default() };
    let end = tilde_contents::scan(
        &job.folder,
        &needle,
        &options,
        &|| cancelled() || full.load(Ordering::Relaxed),
        &mut |path, m| {
            if let Some(hit) = accept(&job, &path, Some(m), false, now) {
                out.push(hit);
            }
        },
    );
    if cancelled() {
        return;
    }
    let mut notes = vec![if end.truncated {
        format!("stopped after {} files here", group(end.files))
    } else {
        format!("{} files here", group(end.files))
    }];

    // Everywhere else.
    let elsewhere = job.folder != job.home && !out.full();
    if elsewhere && job.spotlight && tilde_contents::spotlight_available(&job.home) {
        for path in tilde_contents::spotlight(&job.home, needle.text(), SPOTLIGHT_LIMIT, &cancelled) {
            if cancelled() || out.full() {
                break;
            }
            if path.starts_with(&job.folder) {
                continue;
            }
            let found = tilde_contents::search_file(&path, &needle, options.max_size);
            if let Some(hit) = accept(&job, &path, found, true, now) {
                out.push(hit);
            }
        }
        notes.push("Spotlight elsewhere".into());
    } else if elsewhere && let Some(index) = &index {
        let candidates: Vec<PathBuf> = index
            .recent(now - 2 * 365 * 86_400, RECENT_FILES * 8)
            .into_iter()
            .filter(|c| {
                c.size <= RECENT_MAX_SIZE
                    && matches!(kind_for(c.kind, &name_of(&c.path)), Kind::Code | Kind::Text)
                    && !c.path.starts_with(&job.folder)
            })
            .take(RECENT_FILES)
            .map(|c| c.path)
            .collect();
        let read = candidates.len();
        for chunk in candidates.chunks(64) {
            if cancelled() || out.full() {
                break;
            }
            let found: Vec<(PathBuf, Match)> = pool().install(|| {
                chunk
                    .par_iter()
                    .filter_map(|p| tilde_contents::search_file(p, &needle, RECENT_MAX_SIZE).map(|m| (p.clone(), m)))
                    .collect()
            });
            for (path, m) in found {
                if let Some(hit) = accept(&job, &path, Some(m), true, now) {
                    out.push(hit);
                }
            }
        }
        notes.push(format!(
            "{} recent text files elsewhere{}",
            group(read),
            if job.spotlight { " (Spotlight is off)" } else { "" }
        ));
    }
    if cancelled() {
        return;
    }
    if out.full() {
        notes.push(format!("first {MAX_HITS} shown"));
    }
    out.finish(notes.join(" · "));
}

/// Collects hits and sends them in batches.
struct Batcher<'a> {
    id: u64,
    tx: &'a Sender<Command>,
    pending: Vec<ContentHit>,
    sent: Instant,
    total: usize,
    /// Set at `MAX_HITS`, which stops the scan.
    full: &'a AtomicBool,
}

impl Batcher<'_> {
    fn push(&mut self, hit: ContentHit) {
        if self.full() {
            return;
        }
        self.pending.push(hit);
        self.total += 1;
        if self.full() {
            self.full.store(true, Ordering::Relaxed);
        }
        if self.sent.elapsed() >= BATCH_EVERY || self.pending.len() >= 40 {
            self.flush(None);
        }
    }

    fn full(&self) -> bool {
        self.total >= MAX_HITS
    }

    fn flush(&mut self, done: Option<String>) {
        let hits = std::mem::take(&mut self.pending);
        let _ = self.tx.send(Command::ContentsFound { job: self.id, hits, done });
        self.sent = Instant::now();
    }

    fn finish(mut self, note: String) {
        self.flush(Some(note));
    }
}

/// The hit, if the file also matches the query's names and filters.
fn accept(job: &Job, path: &Path, found: Option<Match>, outside: bool, now: i64) -> Option<ContentHit> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let name = name_of(path);
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64);
    let parent = path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let folder = job.folder.to_string_lossy();
    let candidate = Candidate { name: &name, kind: EntryKind::File, size: meta.len(), modified, parent: Some(&parent) };
    let ctx = FilterContext { now, cwd: Some(&folder) };
    if !job.query.filters.iter().all(|f| f.accepts(&candidate, &ctx)) || !name_matches(&job.query.text, &name) {
        return None;
    }
    Some(ContentHit { path: path.to_path_buf(), kind: EntryKind::File, size: meta.len(), modified, found, outside })
}

/// Every term's characters appear in the name, in order (case-insensitive); `!term`
/// excludes names containing it.
fn name_matches(terms: &str, name: &str) -> bool {
    let name = name.to_lowercase();
    terms.split_whitespace().all(|term| {
        let term = term.to_lowercase();
        if let Some(excluded) = term.strip_prefix('!') {
            return excluded.is_empty() || !name.contains(excluded);
        }
        let term = term.trim_start_matches(['^', '\'']).trim_end_matches('$');
        let mut chars = name.chars();
        term.chars().all(|t| chars.any(|c| c == t))
    })
}

fn name_of(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Threads for reading files outside the open folder, at low priority.
fn pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .thread_name(|i| format!("tilde-contents-{i}"))
            .start_handler(|_| tilde_contents::lower_priority())
            .build()
            .expect("contents thread pool")
    })
}

/// `12 345`, as the app shows counts.
fn group(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_filter_like_the_bar() {
        assert!(name_matches("", "anything.md"));
        assert!(name_matches("orb md", "orbital-mechanics.md"));
        assert!(!name_matches("zzz", "orbital-mechanics.md"));
        assert!(name_matches("!draft", "final.md"));
        assert!(!name_matches("!draft", "draft-2.md"));
        assert!(name_matches("^orb", "orbital.md"));
    }

    #[test]
    fn groups_thousands() {
        assert_eq!(group(12), "12");
        assert_eq!(group(1234), "1 234");
        assert_eq!(group(20000), "20 000");
    }
}
