//! File operations: move, copy, trash, rename, new folder, compress, tags. Every request
//! is planned before anything is touched, run on a background thread with progress, and
//! recorded so it can be undone and redone, also after a restart (see docs/SPEC.md §6.4).

mod fsops;
mod history;
mod names;
mod plan;
mod tree;

pub use fsops::{FolderTrash, SystemTrash, Trash, tags};
pub use names::{keep_both, validate_name};
pub use plan::{Plan, clashes, plan};

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::fsops::Progress;
use crate::plan::name;

/// What to do when an item being moved or copied has the same name as something already
/// in the destination.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Clash {
    /// Give the arriving item a free name ("report 2.pdf").
    #[default]
    KeepBoth,
    /// Move the existing item to the Trash first (undo puts it back).
    Replace,
    /// Leave that item where it is.
    Skip,
}

/// What the user asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Request {
    Move {
        sources: Vec<PathBuf>,
        into: PathBuf,
        #[serde(default)]
        clash: Clash,
    },
    Copy {
        sources: Vec<PathBuf>,
        into: PathBuf,
        #[serde(default)]
        clash: Clash,
    },
    Trash {
        sources: Vec<PathBuf>,
    },
    Rename {
        source: PathBuf,
        name: String,
    },
    NewFolder {
        parent: PathBuf,
        name: String,
    },
    Compress {
        sources: Vec<PathBuf>,
    },
    /// Adds and removes Finder tags, keeping each item's other tags.
    Tag {
        sources: Vec<PathBuf>,
        add: Vec<String>,
        remove: Vec<String>,
    },
    /// A mix of changes applied together and undone together (text editing a folder).
    Batch {
        label: String,
        items: Vec<BatchItem>,
    },
}

/// One change in a `Request::Batch`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BatchItem {
    /// Rename or move.
    Move {
        from: PathBuf,
        to: PathBuf,
    },
    Copy {
        from: PathBuf,
        to: PathBuf,
    },
    Trash {
        path: PathBuf,
    },
    CreateFile {
        path: PathBuf,
    },
    CreateDir {
        path: PathBuf,
    },
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
            Request::Tag { sources, .. } => format!("Tag {}", items(sources)),
            Request::Batch { label, .. } => label.clone(),
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
    CreateFile { path: PathBuf },
    Compress { sources: Vec<PathBuf>, to: PathBuf },
    SetTags { path: PathBuf, tags: Vec<String> },
}

/// A completed step, with what's needed to reverse it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Done {
    Moved { from: PathBuf, to: PathBuf },
    Created { path: PathBuf },
    Trashed { original: PathBuf, in_trash: PathBuf },
    Tagged { path: PathBuf, before: Vec<String> },
}

impl Done {
    /// The step that reverses this one. Created things go to the Trash rather than being
    /// deleted outright, so undoing a copy can itself be recovered.
    fn inverse(&self) -> Step {
        match self {
            Done::Moved { from, to } => Step::Move { from: to.clone(), to: from.clone() },
            Done::Created { path } => Step::Trash { path: path.clone() },
            Done::Trashed { original, in_trash } => Step::Move { from: in_trash.clone(), to: original.clone() },
            Done::Tagged { path, before } => Step::SetTags { path: path.clone(), tags: before.clone() },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeKind {
    Done,
    Undone,
    Redone,
    /// Went to an earlier or other state (undos and redos).
    Restored,
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
    pub replaced: u32,
    pub skipped: u32,
    /// The paths the operation created or moved things to (to select them afterwards).
    pub results: Vec<PathBuf>,
}

/// Progress and completion, called on the operations thread.
pub trait Listener: Send + Sync {
    fn started(&self, id: u64, label: &str, total: u32);
    fn progress(&self, id: u64, done: u32, total: u32, current: &str);
    /// Bytes copied of the current item, for copies that move data (to another volume).
    fn bytes(&self, _id: u64, _copied: u64, _total: u64) {}
    fn finished(&self, id: u64, outcome: Outcome);
}

/// Whether a recorded operation's changes are in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecordState {
    Applied,
    Undone,
    /// Undo stopped partway (something had changed since); the rest can still be undone.
    PartlyUndone,
}

/// One operation in the history. Records are never dropped by undo or by new operations
/// (only by age and count), so every branch stays reachable: anything undone can be
/// redone later, and anything applied can be undone, not just the latest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Record {
    id: u64,
    /// The operation this one was done on top of (the current state then); `None` for
    /// the first one, or one done after undoing everything.
    #[serde(default)]
    parent: Option<u64>,
    request: Request,
    /// The steps in place now: what undoing reverses.
    done: Vec<Done>,
    /// What it did when last applied, kept for the history list after undoing.
    log: Vec<Done>,
    /// When it first ran, and when it last changed state (seconds since 1970).
    at: u64,
    changed: u64,
    state: RecordState,
}

/// An entry of `Operations::history`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryItem {
    pub id: u64,
    pub label: String,
    pub at: u64,
    pub changed: u64,
    pub state: RecordState,
    /// The operation it was done on top of.
    pub parent: Option<u64>,
    /// The current state is right after this one.
    pub is_head: bool,
    /// What it did when last applied.
    pub changes: Vec<Done>,
    pub can_undo: bool,
    pub can_redo: bool,
}

/// What an undo, redo or go-to would do, for showing before it runs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Preview {
    /// Records to undo, in order (newest first), and to redo (oldest first).
    pub undo: Vec<u64>,
    pub redo: Vec<u64>,
    /// Why it can't be done, if it can't.
    pub problem: Option<String>,
}

#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Journal {
    /// Oldest first.
    #[serde(default)]
    records: Vec<Record>,
    /// The current state: right after this record (`None`: before all of them). New
    /// operations are done on top of it; undo and redo move it along its branch.
    #[serde(default)]
    head: Option<u64>,
    #[serde(default)]
    next: u64,
    /// 2: records have parents (earlier files are linear and get them on load).
    #[serde(default)]
    version: u32,
}

impl Journal {
    fn get(&mut self, id: u64) -> Option<&mut Record> {
        self.records.iter_mut().find(|r| r.id == id)
    }

    fn add(&mut self, request: Request, done: Vec<Done>) {
        let now = history::now();
        self.next += 1;
        self.records.push(Record {
            id: self.next,
            parent: self.head,
            request,
            log: done.clone(),
            done,
            at: now,
            changed: now,
            state: RecordState::Applied,
        });
        self.head = Some(self.next);
    }

    /// What ⌘Z undoes: the current operation (or, if it was undone on its own, the
    /// nearest one above it that's still in place).
    fn undo_target(&self) -> Option<u64> {
        self.applied_at_or_above(self.head)
    }
}

/// How a run of undos and redos is described: "Undo “x” and 2 later operations".
#[derive(Clone)]
enum Intent {
    Undo(u64),
    Redo(u64),
    Goto(Option<u64>),
}

enum Job {
    Run { id: u64, request: Request },
    Moves { id: u64, plan: Intent, moves: Vec<tree::Move>, label: String },
}

/// Runs operations one at a time on a background thread and keeps the undo history.
pub struct Operations {
    tx: Sender<Job>,
    journal: Arc<Mutex<Journal>>,
    cancels: Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>>,
    next_id: AtomicU64,
}

impl Operations {
    /// History lasts as long as this value.
    pub fn new(trash: Box<dyn Trash>, listener: Arc<dyn Listener>) -> Operations {
        Self::start(trash, listener, None)
    }

    /// History is loaded from `history` and saved there after every change, so undo works
    /// across restarts.
    pub fn with_history(trash: Box<dyn Trash>, listener: Arc<dyn Listener>, history: PathBuf) -> Operations {
        Self::start(trash, listener, Some(history))
    }

    fn start(trash: Box<dyn Trash>, listener: Arc<dyn Listener>, history: Option<PathBuf>) -> Operations {
        let (tx, rx) = mpsc::channel::<Job>();
        let loaded = history.as_deref().map(history::load).unwrap_or_default();
        let journal = Arc::new(Mutex::new(loaded));
        let cancels: Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>> = Arc::default();
        let runner = Runner { trash, listener, journal: journal.clone(), cancels: cancels.clone(), history };
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

    /// Undoes the current operation (moving back along its branch). `None` if there's
    /// nothing to undo.
    pub fn undo(&self) -> Option<u64> {
        let target = self.journal.lock().unwrap().undo_target()?;
        self.undo_record(target)
    }

    /// Redoes the operation undone most recently on top of the current one.
    pub fn redo(&self) -> Option<u64> {
        let target = self.journal.lock().unwrap().redo_target()?;
        self.redo_record(target)
    }

    /// What undoing `record` involves: later operations on the same files come off first.
    pub fn preview_undo(&self, record: u64) -> Preview {
        let journal = self.journal.lock().unwrap();
        split(journal.undo_plan(record), None)
    }

    /// What redoing `record` involves: undone operations it builds on come back first.
    pub fn preview_redo(&self, record: u64) -> Preview {
        let journal = self.journal.lock().unwrap();
        split(journal.redo_plan(record), None)
    }

    /// What going to the state right after `record` (`None`: before everything) involves.
    pub fn preview_goto(&self, record: Option<u64>) -> Preview {
        let journal = self.journal.lock().unwrap();
        match journal.goto_plan(record) {
            Ok(moves) => split(moves, None),
            Err(e) => Preview { problem: Some(e), ..Preview::default() },
        }
    }

    /// Undoes `record` from `history`, with the later operations that touched the same
    /// files (newest first), so nothing is left half-reverted. Steps that something
    /// outside Tilde made impossible are reported, not forced.
    pub fn undo_record(&self, record: u64) -> Option<u64> {
        let (moves, label) = {
            let journal = self.journal.lock().unwrap();
            let moves = journal.undo_plan(record);
            let label = label(&journal, &Intent::Undo(record), &moves);
            (moves, label)
        };
        (!moves.is_empty()).then(|| self.enqueue(|id| Job::Moves { id, plan: Intent::Undo(record), moves, label }))
    }

    /// Redoes an undone record (planned afresh, as when it first ran), after the undone
    /// operations it builds on.
    pub fn redo_record(&self, record: u64) -> Option<u64> {
        let (moves, label) = {
            let journal = self.journal.lock().unwrap();
            let moves = journal.redo_plan(record);
            let label = label(&journal, &Intent::Redo(record), &moves);
            (moves, label)
        };
        (!moves.is_empty()).then(|| self.enqueue(|id| Job::Moves { id, plan: Intent::Redo(record), moves, label }))
    }

    /// Puts files back the way they were right after `record` (`None`: before everything),
    /// across branches. Fails (without changing anything) if a partly undone operation is
    /// in the way; `None` when there's nothing to do.
    pub fn goto(&self, record: Option<u64>) -> Result<Option<u64>, String> {
        let (moves, label) = {
            let journal = self.journal.lock().unwrap();
            let moves = journal.goto_plan(record)?;
            let label = label(&journal, &Intent::Goto(record), &moves);
            (moves, label)
        };
        Ok((!moves.is_empty()).then(|| self.enqueue(|id| Job::Moves { id, plan: Intent::Goto(record), moves, label })))
    }

    /// The current state: right after this record (`None`: before all of them).
    pub fn head(&self) -> Option<u64> {
        self.journal.lock().unwrap().head
    }

    /// Stops an operation between items, or partway through copying one (the partial copy
    /// is removed). Finished items stay done (and undoable).
    pub fn cancel(&self, id: u64) {
        if let Some(flag) = self.cancels.lock().unwrap().get(&id) {
            flag.store(true, Ordering::SeqCst);
        }
    }

    pub fn undo_label(&self) -> Option<String> {
        let journal = self.journal.lock().unwrap();
        journal.undo_target().and_then(|id| journal.find(id)).map(|r| r.request.label())
    }

    pub fn redo_label(&self) -> Option<String> {
        let journal = self.journal.lock().unwrap();
        journal.redo_target().and_then(|id| journal.find(id)).map(|r| r.request.label())
    }

    /// Every recorded operation, newest first.
    pub fn history(&self) -> Vec<HistoryItem> {
        let journal = self.journal.lock().unwrap();
        journal
            .records
            .iter()
            .rev()
            .map(|r| HistoryItem {
                id: r.id,
                label: r.request.label(),
                at: r.at,
                changed: r.changed,
                state: r.state,
                parent: r.parent,
                is_head: journal.head == Some(r.id),
                changes: r.log.clone(),
                can_undo: !r.done.is_empty(),
                can_redo: r.state == RecordState::Undone,
            })
            .collect()
    }

    /// Operations waiting or running.
    pub fn running(&self) -> Vec<u64> {
        let mut ids: Vec<u64> = self.cancels.lock().unwrap().keys().copied().collect();
        ids.sort_unstable();
        ids
    }
}

struct Runner {
    trash: Box<dyn Trash>,
    listener: Arc<dyn Listener>,
    journal: Arc<Mutex<Journal>>,
    cancels: Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>>,
    history: Option<PathBuf>,
}

impl Runner {
    fn run(&self, job: Job) {
        let id = match &job {
            Job::Run { id, .. } | Job::Moves { id, .. } => *id,
        };
        let cancel = self.cancels.lock().unwrap().get(&id).cloned().unwrap_or_default();
        let outcome = match job {
            Job::Run { request, .. } => self.perform(id, request, &cancel),
            Job::Moves { plan, moves, label, .. } => self.moves(id, &plan, &moves, label, &cancel),
        };
        if let Some(path) = &self.history {
            history::save(path, &self.journal.lock().unwrap());
        }
        self.cancels.lock().unwrap().remove(&id);
        self.listener.finished(id, outcome);
    }

    /// Plans and runs `request`, returning the outcome and the steps that completed.
    fn apply(&self, id: u64, request: &Request, kind: OutcomeKind, cancel: &AtomicBool) -> (Outcome, Vec<Done>) {
        let label = request.label();
        let plan = match plan(request) {
            Ok(p) => p,
            Err(e) => return (Outcome { error: Some(e), ..nothing(kind, &label) }, Vec::new()),
        };
        let total = plan.steps.len() as u32;
        self.listener.started(id, &label, total);
        let (done, error) = self.execute(id, &plan.steps, cancel);
        let outcome = Outcome {
            kind,
            label,
            completed: done.len() as u32,
            total,
            error,
            renamed: plan.renamed,
            replaced: plan.replaced,
            skipped: plan.skipped,
            results: done.iter().map(result_path).collect(),
        };
        (outcome, done)
    }

    fn perform(&self, id: u64, request: Request, cancel: &AtomicBool) -> Outcome {
        let (outcome, done) = self.apply(id, &request, OutcomeKind::Done, cancel);
        if !done.is_empty() {
            self.journal.lock().unwrap().add(request, done);
        }
        outcome
    }

    /// Runs a plan's undos and redos in order, stopping at the first that fails (the ones
    /// before it stay done, and are recorded as such).
    fn moves(&self, id: u64, plan: &Intent, moves: &[tree::Move], label: String, cancel: &AtomicBool) -> Outcome {
        let kind = match plan {
            Intent::Undo(_) => OutcomeKind::Undone,
            Intent::Redo(_) => OutcomeKind::Redone,
            Intent::Goto(_) => OutcomeKind::Restored,
        };
        // One operation reports its own items ("2 of 3 moved").
        if let [single] = moves {
            let step = match *single {
                tree::Move::Undo(record) => self.undo(id, record, cancel),
                tree::Move::Redo(record) => self.redo(id, record, cancel),
            };
            return Outcome { kind, label, ..step };
        }
        let mut total = Outcome { kind, label, ..nothing(kind, "") };
        total.total = moves.len() as u32;
        for m in moves {
            let step = match *m {
                tree::Move::Undo(record) => self.undo(id, record, cancel),
                tree::Move::Redo(record) => self.redo(id, record, cancel),
            };
            total.results.extend(step.results);
            if let Some(e) = step.error {
                total.error = Some(if moves.len() > 1 { format!("{}: {e}", step.label) } else { e });
                break;
            }
            total.completed += 1;
        }
        total
    }

    fn redo(&self, id: u64, record: u64, cancel: &AtomicBool) -> Outcome {
        // Look up in its own statement so the journal lock is released before running.
        let found = {
            let mut journal = self.journal.lock().unwrap();
            journal.get(record).filter(|r| r.state == RecordState::Undone).map(|r| r.request.clone())
        };
        // Already in place (an earlier step may have brought it back): nothing to do.
        let Some(request) = found else { return nothing(OutcomeKind::Redone, "") };
        let (outcome, done) = self.apply(id, &request, OutcomeKind::Redone, cancel);
        if !done.is_empty() {
            let mut journal = self.journal.lock().unwrap();
            if let Some(r) = journal.get(record) {
                r.log = done.clone();
                r.done = done;
                r.state = RecordState::Applied;
                r.changed = history::now();
            }
            // Forward along the current branch moves the current state with it.
            if journal.is_ancestor(journal.head, record) {
                journal.head = Some(record);
            }
        }
        outcome
    }

    fn undo(&self, id: u64, record: u64, cancel: &AtomicBool) -> Outcome {
        let found = {
            let mut journal = self.journal.lock().unwrap();
            journal.get(record).filter(|r| !r.done.is_empty()).map(|r| (r.request.label(), r.done.clone()))
        };
        let Some((label, done)) = found else { return nothing(OutcomeKind::Undone, "") };
        let steps: Vec<Step> = done.iter().rev().map(Done::inverse).collect();
        let total = steps.len() as u32;
        self.listener.started(id, &label, total);
        let (reversed, error) = self.execute(id, &steps, cancel);
        let results = reversed.iter().map(result_path).collect();
        let completed = reversed.len() as u32;
        let mut journal = self.journal.lock().unwrap();
        // Steps not reversed stay undoable.
        let remaining = done.len() - reversed.len();
        if let Some(r) = journal.get(record) {
            r.done.truncate(remaining);
            r.changed = history::now();
            r.state = if remaining == 0 { RecordState::Undone } else { RecordState::PartlyUndone };
        }
        // The current state moves up past anything no longer in place.
        if remaining == 0 && journal.head.is_some() {
            let head = journal.head;
            journal.head = journal.applied_at_or_above(head);
        }
        Outcome {
            kind: OutcomeKind::Undone,
            label,
            completed,
            total,
            error,
            renamed: 0,
            replaced: 0,
            skipped: 0,
            results,
        }
    }

    /// Runs steps in order, stopping at the first error or a cancel.
    fn execute(&self, id: u64, steps: &[Step], cancel: &AtomicBool) -> (Vec<Done>, Option<String>) {
        let total = steps.len() as u32;
        let mut done = Vec::new();
        let report = |copied, total| self.listener.bytes(id, copied, total);
        let progress = Progress { report: &report, cancel };
        for (i, step) in steps.iter().enumerate() {
            if cancel.load(Ordering::SeqCst) {
                return (done, Some(fsops::CANCELLED.into()));
            }
            self.listener.progress(id, i as u32, total, &step_name(step));
            // An item moved or deleted since (outside Tilde, or by a later operation when
            // undoing an older one) is reported plainly.
            let source = match step {
                Step::Move { from, .. } | Step::Copy { from, .. } => Some(from),
                Step::Trash { path } | Step::SetTags { path, .. } => Some(path),
                _ => None,
            };
            if let Some(source) = source
                && source.symlink_metadata().is_err()
            {
                return (done, Some(format!("“{}” no longer exists where Tilde left it", name(source))));
            }
            let result = match step {
                Step::Move { from, to } => fsops::move_item(from, to, Some(&progress))
                    .map(|_| Done::Moved { from: from.clone(), to: to.clone() }),
                Step::Copy { from, to } => {
                    fsops::copy_item(from, to, Some(&progress)).map(|_| Done::Created { path: to.clone() })
                }
                Step::MakeDir { path } => fsops::make_dir(path).map(|_| Done::Created { path: path.clone() }),
                Step::CreateFile { path } => fsops::create_file(path).map(|_| Done::Created { path: path.clone() }),
                Step::Compress { sources, to } => {
                    fsops::compress(sources, to).map(|_| Done::Created { path: to.clone() })
                }
                Step::Trash { path } => {
                    self.trash.trash(path).map(|in_trash| Done::Trashed { original: path.clone(), in_trash })
                }
                Step::SetTags { path, tags } => fsops::tags(path).and_then(|before| {
                    fsops::set_tags(path, tags).map(|_| Done::Tagged { path: path.clone(), before })
                }),
            };
            match result {
                Ok(d) => done.push(d),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => return (done, Some(e.to_string())),
                Err(e) => return (done, Some(format!("{}: {e}", step_name(step)))),
            }
        }
        self.listener.progress(id, total, total, "");
        (done, None)
    }
}

/// Splits a plan into what's undone and what's redone.
fn split(moves: Vec<tree::Move>, problem: Option<String>) -> Preview {
    let mut p = Preview { problem, ..Preview::default() };
    for m in moves {
        match m {
            tree::Move::Undo(id) => p.undo.push(id),
            tree::Move::Redo(id) => p.redo.push(id),
        }
    }
    p
}

/// "Undo “Rename a.txt”", "Undo “Rename a.txt” and 2 later operations", "Go back to after
/// “Trash b.txt”".
fn label(journal: &Journal, plan: &Intent, moves: &[tree::Move]) -> String {
    let name = |id: u64| journal.find(id).map(|r| r.request.label()).unwrap_or_default();
    let others = moves.len().saturating_sub(1);
    let more = |what: &str| match others {
        0 => String::new(),
        1 => format!(" and 1 {what} operation"),
        n => format!(" and {n} {what} operations"),
    };
    match plan {
        Intent::Undo(id) => format!("{}{}", name(*id), more("later")),
        Intent::Redo(id) => format!("{}{}", name(*id), more("earlier")),
        Intent::Goto(Some(id)) => format!("the state after {}", name(*id)),
        Intent::Goto(None) => "the start of the history".into(),
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
        replaced: 0,
        skipped: 0,
        results: Vec::new(),
    }
}

fn step_name(step: &Step) -> String {
    match step {
        Step::Move { from, .. } | Step::Copy { from, .. } => name(from),
        Step::Trash { path } | Step::MakeDir { path } | Step::CreateFile { path } | Step::SetTags { path, .. } => {
            name(path)
        }
        Step::Compress { to, .. } => name(to),
    }
}

fn result_path(d: &Done) -> PathBuf {
    match d {
        Done::Moved { to, .. } => to.clone(),
        Done::Created { path } => path.clone(),
        Done::Trashed { in_trash, .. } => in_trash.clone(),
        Done::Tagged { path, .. } => path.clone(),
    }
}

#[cfg(test)]
mod tests;
