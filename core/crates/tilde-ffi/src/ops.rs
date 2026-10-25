//! File operations for the app: one `FileOps` shared by every window, so undo works
//! across them like the Finder's.

use std::path::PathBuf;
use std::sync::Arc;

use tilde_ops::{Listener, Operations, Outcome, OutcomeKind, Request, SystemTrash};

use crate::edit::EditSession;

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum OpKind {
    Move,
    Copy,
    Trash,
    Rename,
    NewFolder,
    Compress,
}

/// `destination` is the folder for Move, Copy and NewFolder; `name` is the new name for
/// Rename and NewFolder.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct OpRequest {
    pub kind: OpKind,
    pub sources: Vec<String>,
    pub destination: Option<String>,
    pub name: Option<String>,
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
    #[uniffi::constructor]
    pub fn new(listener: Arc<dyn OpsListener>) -> Arc<Self> {
        Arc::new(FileOps { inner: Operations::new(Box::new(SystemTrash), Arc::new(Forward(listener))) })
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

fn to_request(r: OpRequest) -> Result<Request, OpError> {
    let invalid = |m: &str| OpError::Invalid { message: m.to_string() };
    let sources: Vec<PathBuf> = r.sources.iter().map(PathBuf::from).collect();
    let destination = r.destination.map(PathBuf::from);
    Ok(match r.kind {
        OpKind::Move => {
            Request::Move { sources, into: destination.ok_or_else(|| invalid("Move needs a destination"))? }
        }
        OpKind::Copy => {
            Request::Copy { sources, into: destination.ok_or_else(|| invalid("Copy needs a destination"))? }
        }
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
