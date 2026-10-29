//! The suites. Each measures one part of Tilde, on synthetic data and optionally on the
//! user's own (read-only).

pub mod archive;
pub mod contents;
pub mod engine;
pub mod folder;
pub mod index;
pub mod live;
pub mod relevance;
pub mod scan;
pub mod sizes;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use tilde_index::GlobalIndex;

/// What every suite is given.
pub struct Ctx {
    /// Synthetic index sizes (items), loaded from a saved index.
    pub sizes: Vec<usize>,
    /// Synthetic homes written to disk (items), for scans, folder sizes and the engine.
    pub trees: Vec<usize>,
    /// Items in the tree the live-update suite changes.
    pub live: usize,
    /// Text files for contents search; members for archives.
    pub text_files: usize,
    pub archive_members: usize,
    /// Also the user's own index (a copy).
    pub real: bool,
    /// Also scan the user's home and measure folders there (read-only, slow).
    pub real_heavy: bool,
    pub scratch: PathBuf,
}

pub const ALL: &[(&str, &str)] = &[
    (
        "index",
        "the saved index: save, load, memory held, searches (fuzzy, filters, typing), Activity and Clean Up lookups",
    ),
    (
        "live",
        "keeping the index current: a new, deleted or renamed item showing up, and what editing many files does to memory and search speed",
    ),
    ("scan", "building the index from disk, and raw folder listing speed"),
    ("folder", "listing one folder, sorting it and matching typed text in it"),
    ("relevance", "ranking quality: searches with what should come first, passed or not"),
    ("sizes", "measuring folder sizes"),
    ("contents", "searching inside files (text:)"),
    ("archive", "listing and unpacking zip and tar.gz"),
    ("engine", "end to end through the engine the app uses: opening folders, typing, fetching rows, sorting"),
];

/// Waits until a just-started index has loaded or scanned.
pub fn wait_ready(index: &GlobalIndex, timeout: Duration) -> Result<(), String> {
    let t = Instant::now();
    while index.status().scanning {
        if t.elapsed() > timeout {
            return Err(format!("index not ready after {timeout:?}"));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}
