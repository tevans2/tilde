//! The undo history on disk, so operations can be undone after a restart.
//!
//! JSON, rewritten after every change (it's small). A missing or unreadable file is an
//! empty history; undoing an old entry plans like any other step, so items that have since
//! moved or gone are reported instead of guessed at.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::Journal;

/// Undoable operations kept.
const MAX_ENTRIES: usize = 100;
/// Entries older than this are dropped.
const MAX_AGE_SECS: u64 = 30 * 24 * 3600;

pub(crate) fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub(crate) fn load(path: &Path) -> Journal {
    let Ok(text) = std::fs::read_to_string(path) else { return Journal::default() };
    let mut journal: Journal = serde_json::from_str(&text).unwrap_or_default();
    let cutoff = now().saturating_sub(MAX_AGE_SECS);
    journal.undo.retain(|e| e.at >= cutoff);
    journal
}

pub(crate) fn save(path: &Path, journal: &Journal) {
    let keep = journal.undo.len().saturating_sub(MAX_ENTRIES);
    let trimmed = Journal {
        undo: journal.undo[keep..].to_vec(),
        redo: journal.redo.iter().rev().take(MAX_ENTRIES).rev().cloned().collect(),
    };
    let Ok(text) = serde_json::to_string(&trimmed) else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Written beside and renamed over, so a crash never leaves half a file.
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}
