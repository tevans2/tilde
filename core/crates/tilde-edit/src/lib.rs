//! Editing a folder as text (see docs/SPEC.md §6.5).
//!
//! Each item is one line, `/<id> <name>`, with folders ending in `/`. The ID is hidden by
//! the editor but is real text, so a line keeps its identity when it's cut and pasted,
//! reordered or duplicated. Comparing the edited text with the session gives the changes:
//!
//! | Line | Change |
//! |---|---|
//! | known ID, same name | none |
//! | known ID, new name | rename |
//! | known ID, a path (`sub/x`, `~/x`, `/x`) | move |
//! | known ID again | copy of the original |
//! | known ID missing | trash |
//! | no ID, ends with `/` | new folder |
//! | no ID | new empty file |

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use tilde_ops::{BatchItem, validate_name};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Item {
    id: u32,
    name: String,
    folder: bool,
}

/// One folder being edited.
#[derive(Debug, Clone)]
pub struct Session {
    dir: PathBuf,
    home: PathBuf,
    items: Vec<Item>,
    width: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Rename { from: PathBuf, to: PathBuf },
    Move { from: PathBuf, to: PathBuf },
    Copy { from: PathBuf, to: PathBuf },
    Trash { path: PathBuf },
    CreateFile { path: PathBuf },
    CreateDir { path: PathBuf },
}

/// A problem on one line (0-based) that blocks applying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineError {
    pub line: usize,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diff {
    pub changes: Vec<Change>,
    pub errors: Vec<LineError>,
}

impl Session {
    /// `entries` are the folder's items (name, is folder) in display order.
    pub fn new(dir: PathBuf, home: PathBuf, entries: Vec<(String, bool)>) -> Session {
        let width = entries.len().to_string().len().max(3);
        let items = entries
            .into_iter()
            .enumerate()
            .map(|(i, (name, folder))| Item { id: i as u32 + 1, name, folder })
            .collect();
        Session { dir, home, items, width }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The text to edit: one line per item.
    pub fn text(&self) -> String {
        self.items
            .iter()
            .map(|i| format!("/{:0w$} {}{}", i.id, i.name, if i.folder { "/" } else { "" }, w = self.width))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The line showing `name`.
    pub fn line_of(&self, name: &str) -> Option<usize> {
        self.items.iter().position(|i| i.name == name)
    }

    /// Compares edited text with the session.
    pub fn diff(&self, text: &str) -> Diff {
        let mut diff = Diff::default();
        let mut uses: HashMap<u32, usize> = HashMap::new();
        // Final paths (lowercased: macOS volumes are usually case-insensitive) → line.
        let mut targets: HashMap<String, usize> = HashMap::new();

        for (line_no, raw) in text.split('\n').enumerate() {
            let line = raw.trim_end_matches('\r');
            if line.trim().is_empty() {
                continue;
            }
            let (id, rest) = split_prefix(line);
            let rest = rest.trim();
            let mut error = |message: String| diff.errors.push(LineError { line: line_no, message });
            if rest.is_empty() {
                error("This line has no name.".into());
                continue;
            }
            let wants_folder = rest.len() > 1 && rest.ends_with('/');
            let target = self.resolve(rest.trim_end_matches('/'));
            let Some(file_name) = target.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                error(format!("“{rest}” isn't a valid name."));
                continue;
            };
            if let Err(e) = validate_name(&file_name) {
                error(e);
                continue;
            }
            if let Some(&first) = targets.get(&target.to_string_lossy().to_lowercase()) {
                error(format!("Line {} is already called “{file_name}”.", first + 1));
                continue;
            }
            targets.insert(target.to_string_lossy().to_lowercase(), line_no);

            match id {
                None if wants_folder => diff.changes.push(Change::CreateDir { path: target }),
                None => diff.changes.push(Change::CreateFile { path: target }),
                Some(id) => {
                    let Some(item) = self.items.iter().find(|i| i.id == id) else {
                        error("This line came from another folder; Tilde can't tell which item it is.".into());
                        continue;
                    };
                    let from = self.dir.join(&item.name);
                    let count = uses.entry(id).or_default();
                    *count += 1;
                    if *count > 1 {
                        diff.changes.push(Change::Copy { from, to: target });
                    } else if target != from {
                        if target.parent() == Some(self.dir.as_path()) {
                            diff.changes.push(Change::Rename { from, to: target });
                        } else {
                            diff.changes.push(Change::Move { from, to: target });
                        }
                    }
                }
            }
        }
        for item in &self.items {
            if !uses.contains_key(&item.id) {
                diff.changes.push(Change::Trash { path: self.dir.join(&item.name) });
            }
        }
        diff
    }

    /// `name` → a path: relative to the folder, or from `~/` or `/`. `.` and `..` are
    /// resolved.
    fn resolve(&self, name: &str) -> PathBuf {
        let raw = if let Some(rest) = name.strip_prefix("~/") {
            self.home.join(rest)
        } else if name.starts_with('/') {
            PathBuf::from(name)
        } else {
            self.dir.join(name)
        };
        let mut out = PathBuf::new();
        for c in raw.components() {
            match c {
                Component::CurDir => {}
                Component::ParentDir => {
                    out.pop();
                }
                other => out.push(other),
            }
        }
        out
    }
}

/// Length in bytes (also chars and UTF-16 units: the prefix is ASCII) of a line's hidden
/// `/<id> ` prefix, or 0.
pub fn prefix_len(line: &str) -> usize {
    let b = line.as_bytes();
    if b.first() != Some(&b'/') {
        return 0;
    }
    let digits = b[1..].iter().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 || b.get(1 + digits) != Some(&b' ') {
        return 0;
    }
    digits + 2
}

fn split_prefix(line: &str) -> (Option<u32>, &str) {
    match prefix_len(line) {
        0 => (None, line),
        n => (line[1..n - 1].parse().ok(), &line[n..]),
    }
}

impl Change {
    /// For the pending-changes list: (kind, description).
    pub fn describe(&self, home: &Path) -> (&'static str, String) {
        let show = |p: &Path| match p.strip_prefix(home) {
            Ok(rel) => format!("~/{}", rel.to_string_lossy()),
            Err(_) => p.to_string_lossy().into_owned(),
        };
        let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match self {
            Change::Rename { from, to } => ("rename", format!("{} → {}", name(from), name(to))),
            Change::Move { from, to } => ("move", format!("{} → {}", name(from), show(to))),
            Change::Copy { from, to } => ("copy", format!("{} → {}", name(from), show(to))),
            Change::Trash { path } => ("trash", name(path)),
            Change::CreateFile { path } => ("create", show(path)),
            Change::CreateDir { path } => ("folder", format!("{}/", show(path))),
        }
    }
}

impl Diff {
    pub fn batch_items(&self) -> Vec<BatchItem> {
        self.changes
            .iter()
            .map(|c| match c.clone() {
                Change::Rename { from, to } | Change::Move { from, to } => BatchItem::Move { from, to },
                Change::Copy { from, to } => BatchItem::Copy { from, to },
                Change::Trash { path } => BatchItem::Trash { path },
                Change::CreateFile { path } => BatchItem::CreateFile { path },
                Change::CreateDir { path } => BatchItem::CreateDir { path },
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        Session::new(
            PathBuf::from("/Users/me/dl"),
            PathBuf::from("/Users/me"),
            vec![
                ("photos".into(), true),
                ("contract.pdf".into(), false),
                ("dataset.zip".into(), false),
                ("notes.md".into(), false),
            ],
        )
    }

    fn p(rel: &str) -> PathBuf {
        PathBuf::from("/Users/me/dl").join(rel)
    }

    #[test]
    fn renders_lines_with_ids() {
        let s = session();
        assert_eq!(s.text(), "/001 photos/\n/002 contract.pdf\n/003 dataset.zip\n/004 notes.md");
        assert_eq!(s.line_of("dataset.zip"), Some(2));
        assert_eq!(prefix_len("/002 contract.pdf"), 5);
        assert_eq!(prefix_len("contract.pdf"), 0);
        assert_eq!(prefix_len("/tmp/x"), 0);
    }

    #[test]
    fn unchanged_text_has_no_changes() {
        let s = session();
        assert_eq!(s.diff(&s.text()), Diff::default());
        // Reordering lines changes nothing either.
        assert_eq!(s.diff("/004 notes.md\n/001 photos/\n\n/003 dataset.zip\n/002 contract.pdf\n"), Diff::default());
    }

    #[test]
    fn every_kind_of_change() {
        let s = session();
        let text = "/001 photos/\n\
                    /002 signed contract.pdf\n\
                    /003 ../archive/dataset.zip\n\
                    /002 photos/contract copy.pdf\n\
                    receipts/\n\
                    todo.txt\n";
        let d = s.diff(text);
        assert!(d.errors.is_empty(), "{:?}", d.errors);
        assert_eq!(
            d.changes,
            vec![
                Change::Rename { from: p("contract.pdf"), to: p("signed contract.pdf") },
                Change::Move { from: p("dataset.zip"), to: PathBuf::from("/Users/me/archive/dataset.zip") },
                Change::Copy { from: p("contract.pdf"), to: p("photos/contract copy.pdf") },
                Change::CreateDir { path: p("receipts") },
                Change::CreateFile { path: p("todo.txt") },
                Change::Trash { path: p("notes.md") },
            ]
        );
        let (kind, text) = d.changes[1].describe(Path::new("/Users/me"));
        assert_eq!((kind, text.as_str()), ("move", "dataset.zip → ~/archive/dataset.zip"));
    }

    #[test]
    fn home_and_absolute_paths() {
        let s = session();
        let d = s.diff("/001 photos/\n/002 ~/Documents/contract.pdf\n/003 /tmp/dataset.zip\n/004 notes.md");
        assert_eq!(
            d.changes[0],
            Change::Move { from: p("contract.pdf"), to: PathBuf::from("/Users/me/Documents/contract.pdf") }
        );
        assert_eq!(d.changes[1], Change::Move { from: p("dataset.zip"), to: PathBuf::from("/tmp/dataset.zip") });
    }

    #[test]
    fn errors_point_at_lines() {
        let s = session();
        let d = s.diff("/001 photos/\n/002 notes.MD\n/004 notes.md\n/999 stray\n/003 bad:name\n/003    \n");
        let lines: Vec<usize> = d.errors.iter().map(|e| e.line).collect();
        assert_eq!(lines, [2, 3, 4, 5], "{:?}", d.errors);
        assert!(d.errors[0].message.contains("Line 2"));
        assert!(d.errors[1].message.contains("another folder"));
    }

    #[test]
    fn case_only_rename_is_a_rename() {
        let s = session();
        let d = s.diff("/001 photos/\n/002 Contract.pdf\n/003 dataset.zip\n/004 notes.md");
        assert_eq!(d.changes, vec![Change::Rename { from: p("contract.pdf"), to: p("Contract.pdf") }]);
    }

    #[test]
    fn batch_items_map_one_to_one() {
        let s = session();
        let d = s.diff("/001 photos/\n/002 c.pdf\n/003 dataset.zip\nnew/\n");
        assert_eq!(
            d.batch_items(),
            vec![
                BatchItem::Move { from: p("contract.pdf"), to: p("c.pdf") },
                BatchItem::CreateDir { path: p("new") },
                BatchItem::Trash { path: p("notes.md") },
            ]
        );
    }
}
