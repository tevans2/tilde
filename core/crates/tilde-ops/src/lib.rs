//! File operations: move, copy, trash, rename, new folder, compress. Every request is
//! planned before anything is touched, run on a background thread with progress, and
//! recorded so it can be undone and redone (see docs/SPEC.md §6.4).

mod fsops;
mod names;
mod plan;

pub use fsops::{FolderTrash, SystemTrash, Trash};
pub use names::{keep_both, validate_name};
pub use plan::{Plan, plan};

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use crate::plan::name;

/// What the user asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Move { sources: Vec<PathBuf>, into: PathBuf },
    Copy { sources: Vec<PathBuf>, into: PathBuf },
    Trash { sources: Vec<PathBuf> },
    Rename { source: PathBuf, name: String },
    NewFolder { parent: PathBuf, name: String },
    Compress { sources: Vec<PathBuf> },
}

impl Request {
    /// "Move 3 items", "Rename report.pdf", for status messages and the Undo menu.
    pub fn label(&self) -> String {
        fn items(v: &[PathBuf]) -> String {
            match v {
                [one] => format!("“{}”", name(one)),
                _ => format!("{} items", v.len()),
            }
        }
        match self {
            Request::Move { sources, .. } => format!("Move {}", items(sources)),
            Request::Copy { sources, .. } => format!("Copy {}", items(sources)),
            Request::Trash { sources } => format!("Trash {}", items(sources)),
            Request::Rename { source, .. } => format!("Rename “{}”", name(source)),
            Request::NewFolder { name, .. } => format!("New Folder “{name}”"),
            Request::Compress { sources } => format!("Compress {}", items(sources)),
        }
    }
}

/// One concrete change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Move { from: PathBuf, to: PathBuf },
    Copy { from: PathBuf, to: PathBuf },
    Trash { path: PathBuf },
    MakeDir { path: PathBuf },
    Compress { sources: Vec<PathBuf>, to: PathBuf },
}

/// A completed step, with what's needed to reverse it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Done {
    Moved { from: PathBuf, to: PathBuf },
    Created { path: PathBuf },
    Trashed { original: PathBuf, in_trash: PathBuf },
}

impl Done {
    /// The step that reverses this one. Created things go to the Trash rather than being
    /// deleted outright, so undoing a copy can itself be recovered.
    fn inverse(&self) -> Step {
        match self {
            Done::Moved { from, to } => Step::Move { from: to.clone(), to: from.clone() },
            Done::Created { path } => Step::Trash { path: path.clone() },
            Done::Trashed { original, in_trash } => Step::Move { from: in_trash.clone(), to: original.clone() },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeKind {
    Done,
    Undone,
    Redone,
}

/// How an operation ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub kind: OutcomeKind,
    pub label: String,
    pub completed: u32,
    pub total: u32,
    /// Why it stopped early or didn't start, for the user.
    pub error: Option<String>,
    pub renamed: u32,
    pub skipped: u32,
    /// The paths the operation created or moved things to (to select them afterwards).
    pub results: Vec<PathBuf>,
}

/// Progress and completion, called on the operations thread.
pub trait Listener: Send + Sync {
    fn started(&self, id: u64, label: &str, total: u32);
    fn progress(&self, id: u64, done: u32, total: u32, current: &str);
    fn finished(&self, id: u64, outcome: Outcome);
}

struct Entry {
    request: Request,
    done: Vec<Done>,
}

#[derive(Default)]
struct Journal {
    undo: Vec<Entry>,
    redo: Vec<Request>,
}

enum Job {
    Run { id: u64, request: Request },
    Undo { id: u64 },
    Redo { id: u64 },
}

/// Runs operations one at a time on a background thread and keeps the undo history.
pub struct Operations {
    tx: Sender<Job>,
    journal: Arc<Mutex<Journal>>,
    cancels: Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>>,
    next_id: AtomicU64,
}

impl Operations {
    pub fn new(trash: Box<dyn Trash>, listener: Arc<dyn Listener>) -> Operations {
        let (tx, rx) = mpsc::channel::<Job>();
        let journal = Arc::new(Mutex::new(Journal::default()));
        let cancels: Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>> = Arc::default();
        let runner = Runner { trash, listener, journal: journal.clone(), cancels: cancels.clone() };
        std::thread::Builder::new()
            .name("tilde-ops".into())
            .spawn(move || {
                for job in rx {
                    runner.run(job);
                }
            })
            .expect("spawn operations thread");
        Operations { tx, journal, cancels, next_id: AtomicU64::new(1) }
    }

    fn enqueue(&self, make: impl FnOnce(u64) -> Job) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.cancels.lock().unwrap().insert(id, Arc::new(AtomicBool::new(false)));
        let _ = self.tx.send(make(id));
        id
    }

    pub fn submit(&self, request: Request) -> u64 {
        self.enqueue(|id| Job::Run { id, request })
    }

    /// Undoes the most recent operation. `None` if there's nothing to undo.
    pub fn undo(&self) -> Option<u64> {
        if self.journal.lock().unwrap().undo.is_empty() {
            return None;
        }
        Some(self.enqueue(|id| Job::Undo { id }))
    }

    pub fn redo(&self) -> Option<u64> {
        if self.journal.lock().unwrap().redo.is_empty() {
            return None;
        }
        Some(self.enqueue(|id| Job::Redo { id }))
    }

    /// Stops an operation between items. Finished items stay done (and undoable).
    pub fn cancel(&self, id: u64) {
        if let Some(flag) = self.cancels.lock().unwrap().get(&id) {
            flag.store(true, Ordering::SeqCst);
        }
    }

    pub fn undo_label(&self) -> Option<String> {
        self.journal.lock().unwrap().undo.last().map(|e| e.request.label())
    }

    pub fn redo_label(&self) -> Option<String> {
        self.journal.lock().unwrap().redo.last().map(Request::label)
    }
}

struct Runner {
    trash: Box<dyn Trash>,
    listener: Arc<dyn Listener>,
    journal: Arc<Mutex<Journal>>,
    cancels: Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>>,
}

impl Runner {
    fn run(&self, job: Job) {
        let id = match &job {
            Job::Run { id, .. } | Job::Undo { id } | Job::Redo { id } => *id,
        };
        let cancel = self.cancels.lock().unwrap().get(&id).cloned().unwrap_or_default();
        let outcome = match job {
            Job::Run { request, .. } => self.perform(id, request, OutcomeKind::Done, &cancel),
            Job::Redo { .. } => {
                // Pop in its own statement so the journal lock is released before running.
                let request = self.journal.lock().unwrap().redo.pop();
                match request {
                    Some(request) => self.perform(id, request, OutcomeKind::Redone, &cancel),
                    None => nothing(OutcomeKind::Redone, "Nothing to redo"),
                }
            }
            Job::Undo { .. } => self.undo(id, &cancel),
        };
        self.cancels.lock().unwrap().remove(&id);
        self.listener.finished(id, outcome);
    }

    fn perform(&self, id: u64, request: Request, kind: OutcomeKind, cancel: &AtomicBool) -> Outcome {
        let label = request.label();
        let plan = match plan(&request) {
            Ok(p) => p,
            Err(e) => return Outcome { error: Some(e), ..nothing(kind, &label) },
        };
        let total = plan.steps.len() as u32;
        self.listener.started(id, &label, total);
        let (done, error) = self.execute(id, &plan.steps, cancel);
        let results = done.iter().map(result_path).collect();
        let completed = done.len() as u32;
        if !done.is_empty() {
            let mut journal = self.journal.lock().unwrap();
            journal.undo.push(Entry { request, done });
            if kind == OutcomeKind::Done {
                journal.redo.clear();
            }
        }
        Outcome { kind, label, completed, total, error, renamed: plan.renamed, skipped: plan.skipped, results }
    }

    fn undo(&self, id: u64, cancel: &AtomicBool) -> Outcome {
        let Some(entry) = self.journal.lock().unwrap().undo.pop() else {
            return nothing(OutcomeKind::Undone, "Nothing to undo");
        };
        let label = entry.request.label();
        let steps: Vec<Step> = entry.done.iter().rev().map(Done::inverse).collect();
        let total = steps.len() as u32;
        self.listener.started(id, &label, total);
        let (reversed, error) = self.execute(id, &steps, cancel);
        let results = reversed.iter().map(result_path).collect();
        let completed = reversed.len() as u32;
        let mut journal = self.journal.lock().unwrap();
        if error.is_none() {
            journal.redo.push(entry.request);
        } else {
            // Keep what couldn't be reversed undoable; drop the steps that were.
            let remaining = entry.done.len() - reversed.len();
            journal.undo.push(Entry { request: entry.request, done: entry.done[..remaining].to_vec() });
        }
        Outcome { kind: OutcomeKind::Undone, label, completed, total, error, renamed: 0, skipped: 0, results }
    }

    /// Runs steps in order, stopping at the first error or a cancel.
    fn execute(&self, id: u64, steps: &[Step], cancel: &AtomicBool) -> (Vec<Done>, Option<String>) {
        let total = steps.len() as u32;
        let mut done = Vec::new();
        for (i, step) in steps.iter().enumerate() {
            if cancel.load(Ordering::SeqCst) {
                return (done, Some("Cancelled".into()));
            }
            self.listener.progress(id, i as u32, total, &step_name(step));
            let result = match step {
                Step::Move { from, to } => {
                    fsops::move_item(from, to).map(|_| Done::Moved { from: from.clone(), to: to.clone() })
                }
                Step::Copy { from, to } => fsops::copy_item(from, to).map(|_| Done::Created { path: to.clone() }),
                Step::MakeDir { path } => fsops::make_dir(path).map(|_| Done::Created { path: path.clone() }),
                Step::Compress { sources, to } => {
                    fsops::compress(sources, to).map(|_| Done::Created { path: to.clone() })
                }
                Step::Trash { path } => {
                    self.trash.trash(path).map(|in_trash| Done::Trashed { original: path.clone(), in_trash })
                }
            };
            match result {
                Ok(d) => done.push(d),
                Err(e) => return (done, Some(format!("{}: {e}", step_name(step)))),
            }
        }
        self.listener.progress(id, total, total, "");
        (done, None)
    }
}

fn nothing(kind: OutcomeKind, label: &str) -> Outcome {
    Outcome {
        kind,
        label: label.to_string(),
        completed: 0,
        total: 0,
        error: None,
        renamed: 0,
        skipped: 0,
        results: Vec::new(),
    }
}

fn step_name(step: &Step) -> String {
    match step {
        Step::Move { from, .. } | Step::Copy { from, .. } => name(from),
        Step::Trash { path } | Step::MakeDir { path } => name(path),
        Step::Compress { to, .. } => name(to),
    }
}

fn result_path(d: &Done) -> PathBuf {
    match d {
        Done::Moved { to, .. } => to.clone(),
        Done::Created { path } => path.clone(),
        Done::Trashed { in_trash, .. } => in_trash.clone(),
    }
}

#[cfg(test)]
mod tests;
