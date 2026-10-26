//! The history as a tree. Each operation records the one it was done on top of (the
//! `head` at the time), so undoing and then doing something new starts a branch instead of
//! throwing the undone work away, as in Vim's undo tree.
//!
//! Plans are worked out here, without touching the disk:
//! - going to any state: undo up to where the branches meet, then redo down to it;
//! - undoing one operation safely: later operations that touched the same files are undone
//!   first (newest first), so nothing is left half-reverted;
//! - redoing one safely: the undone operations it builds on are redone first.

use std::path::{Path, PathBuf};

use crate::{Done, Journal, Record, RecordState};

/// What a plan does to each record, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    Undo(u64),
    Redo(u64),
}

impl Journal {
    pub(crate) fn find(&self, id: u64) -> Option<&Record> {
        self.records.iter().find(|r| r.id == id)
    }

    /// `id`, its parent, and so on up to the first operation.
    fn lineage(&self, id: Option<u64>) -> Vec<u64> {
        let mut out = Vec::new();
        let mut at = id;
        while let Some(i) = at {
            if out.contains(&i) {
                break; // A damaged file shouldn't loop forever.
            }
            out.push(i);
            at = self.find(i).and_then(|r| r.parent);
        }
        out
    }

    /// The nearest record at or above `id` whose changes are in place.
    pub(crate) fn applied_at_or_above(&self, id: Option<u64>) -> Option<u64> {
        self.lineage(id).into_iter().find(|&i| self.find(i).is_some_and(|r| !r.done.is_empty()))
    }

    /// From the current state to the state right after `target` (`None`: before
    /// everything): exactly the operations from the first one down to `target` applied, and
    /// none of the others. Others are undone newest first (so later work comes off before
    /// what it built on), then the missing ones on the way down redone oldest first.
    pub(crate) fn goto_plan(&self, target: Option<u64>) -> Result<Vec<Move>, String> {
        let line = self.lineage(target);
        let mut off: Vec<&Record> =
            self.records.iter().filter(|r| !r.done.is_empty() && !line.contains(&r.id)).collect();
        off.sort_by_key(|r| std::cmp::Reverse((r.changed, r.id)));
        let mut plan: Vec<Move> = off.into_iter().map(|r| Move::Undo(r.id)).collect();
        for &id in line.iter().rev() {
            let Some(r) = self.find(id) else { continue };
            match r.state {
                RecordState::Undone => plan.push(Move::Redo(id)),
                RecordState::PartlyUndone => {
                    return Err(format!("“{}” is partly undone; undo the rest of it first.", r.request.label()));
                }
                RecordState::Applied => {}
            }
        }
        Ok(plan)
    }

    /// Applied records done after `id` that touch the same files (directly or through each
    /// other), newest first: what has to be undone before `id` can be.
    pub(crate) fn dependents(&self, id: u64) -> Vec<u64> {
        let Some(base) = self.find(id) else { return Vec::new() };
        let mut later: Vec<&Record> =
            self.records.iter().filter(|r| r.id != id && !r.done.is_empty() && after(r, base)).collect();
        later.sort_by_key(|r| (r.changed, r.id));
        let mut touched = paths(&base.done);
        let mut out = Vec::new();
        for r in later {
            let mine = paths(&r.done);
            if overlaps(&touched, &mine) {
                touched.extend(mine);
                out.push(r.id);
            }
        }
        out.reverse();
        out
    }

    /// Undone records above `id` in the tree that it builds on (they touch the same files),
    /// oldest first: what has to be redone before `id` can be.
    pub(crate) fn prerequisites(&self, id: u64) -> Vec<u64> {
        let Some(base) = self.find(id) else { return Vec::new() };
        let mut touched = paths(&base.log);
        let mut out = Vec::new();
        for ancestor in self.lineage(base.parent) {
            let Some(r) = self.find(ancestor) else { continue };
            if r.state == RecordState::Applied {
                continue;
            }
            let theirs = paths(&r.log);
            if overlaps(&touched, &theirs) {
                touched.extend(theirs);
                out.push(ancestor);
            }
        }
        out.reverse();
        out
    }

    /// Undoes `id` safely: its dependents first, newest first.
    pub(crate) fn undo_plan(&self, id: u64) -> Vec<Move> {
        let mut plan: Vec<Move> = self.dependents(id).into_iter().map(Move::Undo).collect();
        if self.find(id).is_some_and(|r| !r.done.is_empty()) {
            plan.push(Move::Undo(id));
        }
        plan
    }

    /// Redoes `id` safely: what it builds on first, oldest first.
    pub(crate) fn redo_plan(&self, id: u64) -> Vec<Move> {
        let mut plan: Vec<Move> = self.prerequisites(id).into_iter().map(Move::Redo).collect();
        if self.find(id).is_some_and(|r| r.state == RecordState::Undone) {
            plan.push(Move::Redo(id));
        }
        plan
    }

    /// What ⌘⇧Z redoes: the most recently undone operation done on top of the current one.
    pub(crate) fn redo_target(&self) -> Option<u64> {
        self.records
            .iter()
            .filter(|r| r.parent == self.head && r.state == RecordState::Undone)
            .max_by_key(|r| (r.changed, r.id))
            .map(|r| r.id)
    }

    /// Whether `a` is `b` or above it in the tree.
    pub(crate) fn is_ancestor(&self, a: Option<u64>, b: u64) -> bool {
        match a {
            None => true,
            Some(a) => self.lineage(Some(b)).contains(&a),
        }
    }
}

/// `r` was applied after `base` was (its current changes came later).
fn after(r: &Record, base: &Record) -> bool {
    (r.changed, r.id) > (base.changed, base.id)
}

/// Every path a change involves, where it came from and where it went.
fn paths(changes: &[Done]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for d in changes {
        match d {
            Done::Moved { from, to } => {
                out.push(from.clone());
                out.push(to.clone());
            }
            Done::Created { path } | Done::Tagged { path, .. } => out.push(path.clone()),
            Done::Trashed { original, in_trash } => {
                out.push(original.clone());
                out.push(in_trash.clone());
            }
        }
    }
    out
}

/// Two sets of paths share an item, or one holds the other (a folder and what's inside).
fn overlaps(a: &[PathBuf], b: &[PathBuf]) -> bool {
    let related = |x: &Path, y: &Path| x.starts_with(y) || y.starts_with(x);
    a.iter().any(|x| b.iter().any(|y| related(x, y)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Request;

    fn record(id: u64, parent: Option<u64>, state: RecordState, changes: Vec<Done>, changed: u64) -> Record {
        let applied = state == RecordState::Applied;
        Record {
            id,
            parent,
            request: Request::Trash { sources: vec![] },
            done: if applied { changes.clone() } else { Vec::new() },
            log: changes,
            at: changed,
            changed,
            state,
        }
    }

    fn moved(from: &str, to: &str) -> Vec<Done> {
        vec![Done::Moved { from: from.into(), to: to.into() }]
    }

    #[test]
    fn going_to_another_branch_undoes_up_then_redoes_down() {
        // 1 ─ 2 ─ 3 (head)
        //      └─ 4 (undone branch)
        let j = Journal {
            records: vec![
                record(1, None, RecordState::Applied, moved("/a", "/b"), 1),
                record(2, Some(1), RecordState::Applied, moved("/c", "/d"), 2),
                record(3, Some(2), RecordState::Applied, moved("/e", "/f"), 3),
                record(4, Some(2), RecordState::Undone, moved("/g", "/h"), 4),
            ],
            head: Some(3),
            next: 4,
            version: 2,
        };
        assert_eq!(j.goto_plan(Some(4)).unwrap(), [Move::Undo(3), Move::Redo(4)]);
        assert_eq!(j.goto_plan(Some(1)).unwrap(), [Move::Undo(3), Move::Undo(2)]);
        assert_eq!(j.goto_plan(None).unwrap(), [Move::Undo(3), Move::Undo(2), Move::Undo(1)]);
        assert_eq!(j.goto_plan(Some(3)).unwrap(), []);
        assert_eq!(j.redo_target(), None, "nothing undone on top of the head");
    }

    #[test]
    fn undoing_an_old_operation_takes_what_depends_on_it() {
        // 1 moves a→b, 2 is unrelated, 3 renames b→c (depends on 1), 4 moves c into d
        // (depends on 3, so on 1 too).
        let j = Journal {
            records: vec![
                record(1, None, RecordState::Applied, moved("/x/a", "/x/b"), 1),
                record(2, Some(1), RecordState::Applied, moved("/y/p", "/y/q"), 2),
                record(3, Some(2), RecordState::Applied, moved("/x/b", "/x/c"), 3),
                record(4, Some(3), RecordState::Applied, moved("/x/c", "/z/c"), 4),
            ],
            head: Some(4),
            next: 4,
            version: 2,
        };
        assert_eq!(j.dependents(1), [4, 3]);
        assert_eq!(j.undo_plan(1), [Move::Undo(4), Move::Undo(3), Move::Undo(1)]);
        assert_eq!(j.undo_plan(2), [Move::Undo(2)], "nothing later touches /y");
        // A folder and something inside it count as related.
        let j2 = Journal {
            records: vec![
                record(1, None, RecordState::Applied, vec![Done::Created { path: "/w/new".into() }], 1),
                record(2, Some(1), RecordState::Applied, moved("/w/f.txt", "/w/new/f.txt"), 2),
            ],
            head: Some(2),
            next: 2,
            version: 2,
        };
        assert_eq!(j2.dependents(1), [2]);
    }

    #[test]
    fn redoing_brings_back_what_it_builds_on_first() {
        // 1 creates /w/new (undone), 2 moved a file into it (undone), 3 unrelated (undone).
        let j = Journal {
            records: vec![
                record(1, None, RecordState::Undone, vec![Done::Created { path: "/w/new".into() }], 1),
                record(2, Some(1), RecordState::Undone, moved("/w/f.txt", "/w/new/f.txt"), 2),
                record(3, Some(2), RecordState::Undone, moved("/q/a", "/q/b"), 3),
            ],
            head: None,
            next: 3,
            version: 2,
        };
        assert_eq!(j.redo_plan(2), [Move::Redo(1), Move::Redo(2)]);
        assert_eq!(j.redo_plan(3), [Move::Redo(3)], "unrelated ancestors stay as they are");
        assert_eq!(j.redo_target(), Some(1));
    }
}
