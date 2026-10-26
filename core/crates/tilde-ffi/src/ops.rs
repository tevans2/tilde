//! File operations for the app: one `FileOps` shared by every window, so undo works
//! across them like the Finder's, and across restarts.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tilde_ops::{Clash, Listener, Operations, Outcome, OutcomeKind, Request, SystemTrash};

use crate::edit::EditSession;
use crate::index::home_dir;

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum OpKind {
    Move,
    Copy,
    Trash,
    Rename,
    NewFolder,
    Compress,
    Tag,
}

/// For Move and Copy, when an item's name is taken in the destination.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, uniffi::Enum)]
pub enum ClashPolicy {
    #[default]
    KeepBoth,
    /// The existing item goes to the Trash (undo puts it back).
    Replace,
    Skip,
}

/// `destination` is the folder for Move, Copy and NewFolder; `name` is the new name for
/// Rename and NewFolder; `tags_add` and `tags_remove` are for Tag.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct OpRequest {
    pub kind: OpKind,
    pub sources: Vec<String>,
    #[uniffi(default = None)]
    pub destination: Option<String>,
    #[uniffi(default = None)]
    pub name: Option<String>,
    pub clash: ClashPolicy,
    #[uniffi(default = [])]
    pub tags_add: Vec<String>,
    #[uniffi(default = [])]
    pub tags_remove: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum OpPhase {
    Done,
    Undone,
    Redone,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct OpOutcome {
    pub phase: OpPhase,
    pub label: String,
    pub completed: u32,
    pub total: u32,
    pub error: Option<String>,
    /// Items given a "keep both" name.
    pub renamed: u32,
    /// Existing items moved to the Trash to make way.
    pub replaced: u32,
    pub skipped: u32,
    /// Where things ended up, to select them afterwards.
    pub results: Vec<String>,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum OpError {
    #[error("{message}")]
    Invalid { message: String },
}

/// Implemented by the app. Called on the operations thread.
#[uniffi::export(with_foreign)]
pub trait OpsListener: Send + Sync {
    fn started(&self, id: u64, label: String, total: u32);
    fn progress(&self, id: u64, done: u32, total: u32, current: String);
    /// Bytes copied of the current item, for copies to another volume.
    fn bytes(&self, id: u64, copied: u64, total: u64);
    fn finished(&self, id: u64, outcome: OpOutcome);
}

struct Forward(Arc<dyn OpsListener>);

impl Listener for Forward {
    fn started(&self, id: u64, label: &str, total: u32) {
        self.0.started(id, label.to_string(), total);
    }
    fn progress(&self, id: u64, done: u32, total: u32, current: &str) {
        self.0.progress(id, done, total, current.to_string());
    }
    fn bytes(&self, id: u64, copied: u64, total: u64) {
        self.0.bytes(id, copied, total);
    }
    fn finished(&self, id: u64, o: Outcome) {
        self.0.finished(
            id,
            OpOutcome {
                phase: match o.kind {
                    OutcomeKind::Done => OpPhase::Done,
                    OutcomeKind::Undone => OpPhase::Undone,
                    OutcomeKind::Redone => OpPhase::Redone,
                },
                label: o.label,
                completed: o.completed,
                total: o.total,
                error: o.error,
                renamed: o.renamed,
                replaced: o.replaced,
                skipped: o.skipped,
                results: o.results.into_iter().map(|p| p.to_string_lossy().into_owned()).collect(),
            },
        );
    }
}

#[derive(uniffi::Object)]
pub struct FileOps {
    inner: Operations,
}

#[uniffi::export]
impl FileOps {
    /// The undo history is kept in `~/Library/Application Support/Tilde/undo.json`.
    #[uniffi::constructor]
    pub fn new(listener: Arc<dyn OpsListener>) -> Arc<Self> {
        let history = home_dir().join("Library/Application Support/Tilde/undo.json");
        Arc::new(FileOps {
            inner: Operations::with_history(Box::new(SystemTrash), Arc::new(Forward(listener)), history),
        })
    }

    /// Names of the items in a Move or Copy that are already taken in the destination,
    /// so the app can ask what to do (then submit with a `ClashPolicy`).
    pub fn clashes(&self, request: OpRequest) -> Result<Vec<String>, OpError> {
        Ok(tilde_ops::clashes(&to_request(request)?)
            .iter()
            .map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
            .collect())
    }

    /// Operations waiting or running, oldest first.
    pub fn running(&self) -> Vec<u64> {
        self.inner.running()
    }

    /// Starts an operation and returns its ID. Problems found while planning (a name
    /// that's taken, a folder moved into itself) arrive in `finished`.
    pub fn submit(&self, request: OpRequest) -> Result<u64, OpError> {
        Ok(self.inner.submit(to_request(request)?))
    }

    /// Applies an edited folder text as one undoable operation. Fails without changing
    /// anything if a line has a problem or there's nothing to change.
    pub fn submit_edit(&self, session: Arc<EditSession>, text: String) -> Result<u64, OpError> {
        let diff = session.inner.diff(&text);
        if let Some(e) = diff.errors.first() {
            return Err(OpError::Invalid { message: format!("Line {}: {}", e.line + 1, e.message) });
        }
        if diff.changes.is_empty() {
            return Err(OpError::Invalid { message: "Nothing to change".into() });
        }
        let folder = session.dir().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let n = diff.changes.len();
        let label = format!("Edit “{folder}” ({n} change{})", if n == 1 { "" } else { "s" });
        Ok(self.inner.submit(Request::Batch { label, items: diff.batch_items() }))
    }

    /// Undoes the latest operation; `None` if there's nothing to undo.
    pub fn undo(&self) -> Option<u64> {
        self.inner.undo()
    }

    pub fn redo(&self) -> Option<u64> {
        self.inner.redo()
    }

    pub fn cancel(&self, id: u64) {
        self.inner.cancel(id);
    }

    pub fn undo_label(&self) -> Option<String> {
        self.inner.undo_label()
    }

    pub fn redo_label(&self) -> Option<String> {
        self.inner.redo_label()
    }
}

/// Tags on all of `paths` (in the first item's order). Unreadable items count as having
/// none.
#[uniffi::export]
pub fn common_tags(paths: Vec<String>) -> Vec<String> {
    let mut lists = paths.iter().map(|p| tilde_ops::tags(Path::new(p)).unwrap_or_default());
    let Some(first) = lists.next() else { return Vec::new() };
    lists.fold(first, |common, tags| common.into_iter().filter(|t| tags.contains(t)).collect())
}

fn to_request(r: OpRequest) -> Result<Request, OpError> {
    let invalid = |m: &str| OpError::Invalid { message: m.to_string() };
    let sources: Vec<PathBuf> = r.sources.iter().map(PathBuf::from).collect();
    let destination = r.destination.map(PathBuf::from);
    let clash = match r.clash {
        ClashPolicy::KeepBoth => Clash::KeepBoth,
        ClashPolicy::Replace => Clash::Replace,
        ClashPolicy::Skip => Clash::Skip,
    };
    Ok(match r.kind {
        OpKind::Move => {
            Request::Move { sources, into: destination.ok_or_else(|| invalid("Move needs a destination"))?, clash }
        }
        OpKind::Copy => {
            Request::Copy { sources, into: destination.ok_or_else(|| invalid("Copy needs a destination"))?, clash }
        }
        OpKind::Tag => Request::Tag { sources, add: r.tags_add, remove: r.tags_remove },
        OpKind::Trash => Request::Trash { sources },
        OpKind::Compress => Request::Compress { sources },
        OpKind::Rename => Request::Rename {
            source: sources.into_iter().next().ok_or_else(|| invalid("Nothing to rename"))?,
            name: r.name.ok_or_else(|| invalid("Rename needs a name"))?,
        },
        OpKind::NewFolder => Request::NewFolder {
            parent: destination.ok_or_else(|| invalid("New folder needs a location"))?,
            name: r.name.ok_or_else(|| invalid("New folder needs a name"))?,
        },
    })
}
