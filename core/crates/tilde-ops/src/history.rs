//! The undo history on disk, so operations can be undone after a restart.
//!
//! JSON, rewritten after every change (it's small). A missing or unreadable file is an
//! empty history; undoing an old entry plans like any other step, so items that have since
//! moved or gone are reported instead of guessed at. The first format (an undo stack and
//! a redo stack) is upgraded on load.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::{Done, Journal, Record, RecordState, Request};

/// Operations kept.
const MAX_ENTRIES: usize = 200;
/// Entries older than this are dropped.
const MAX_AGE_SECS: u64 = 30 * 24 * 3600;

pub(crate) fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub(crate) fn load(path: &Path) -> Journal {
    let Ok(text) = std::fs::read_to_string(path) else { return Journal::default() };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else { return Journal::default() };
    let mut journal = if value.get("records").is_some() {
        serde_json::from_value(value).unwrap_or_default()
    } else {
        serde_json::from_value::<Legacy>(value).map(Legacy::upgrade).unwrap_or_default()
    };
    let cutoff = now().saturating_sub(MAX_AGE_SECS);
    journal.records.retain(|r| r.changed >= cutoff);
    let ids: Vec<u64> = journal.records.iter().map(|r| r.id).collect();
    journal.redo.retain(|id| ids.contains(id));
    journal.next = journal.next.max(ids.iter().copied().max().unwrap_or(0));
    journal
}

pub(crate) fn save(path: &Path, journal: &Journal) {
    let keep = journal.records.len().saturating_sub(MAX_ENTRIES);
    let records = journal.records[keep..].to_vec();
    let ids: Vec<u64> = records.iter().map(|r| r.id).collect();
    let trimmed = Journal {
        redo: journal.redo.iter().copied().filter(|id| ids.contains(id)).collect(),
        records,
        next: journal.next,
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

/// The first format: an undo stack and a redo stack of requests.
#[derive(Deserialize)]
struct Legacy {
    #[serde(default)]
    undo: Vec<LegacyEntry>,
    #[serde(default)]
    redo: Vec<Request>,
}

#[derive(Deserialize)]
struct LegacyEntry {
    request: Request,
    done: Vec<Done>,
    #[serde(default)]
    at: u64,
}

impl Legacy {
    fn upgrade(self) -> Journal {
        let mut journal = Journal::default();
        for e in self.undo {
            journal.next += 1;
            journal.records.push(Record {
                id: journal.next,
                request: e.request,
                log: e.done.clone(),
                done: e.done,
                at: e.at,
                changed: e.at,
                state: RecordState::Applied,
            });
        }
        // Redo requests were undone most recently last-pushed-first.
        let undone_at = now();
        for request in self.redo.into_iter().rev() {
            journal.next += 1;
            journal.records.push(Record {
                id: journal.next,
                request,
                done: Vec::new(),
                log: Vec::new(),
                at: undone_at,
                changed: undone_at,
                state: RecordState::Undone,
            });
            journal.redo.insert(0, journal.next);
        }
        journal
    }
}
