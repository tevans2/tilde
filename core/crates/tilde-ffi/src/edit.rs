//! Text editing a folder: the session the editor works on, and a live preview of changes.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tilde_fs::{ListOptions, list_dir, sort_entries};
use tilde_types::EntryKind;

use crate::index::home_dir;
use crate::ops::OpError;

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PendingChange {
    /// rename, move, copy, trash, create or folder.
    pub kind: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct EditLineError {
    /// 0-based line number.
    pub line: u32,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct EditPreview {
    pub changes: Vec<PendingChange>,
    pub errors: Vec<EditLineError>,
}

#[derive(uniffi::Object)]
pub struct EditSession {
    pub(crate) inner: tilde_edit::Session,
    home: PathBuf,
}

#[uniffi::export]
impl EditSession {
    /// Lists `dir` for editing (hidden items only if `show_hidden`).
    #[uniffi::constructor]
    pub fn new(dir: String, show_hidden: bool) -> Result<Arc<Self>, OpError> {
        let dir = PathBuf::from(dir);
        let opts = ListOptions { include_hidden: show_hidden, ..Default::default() };
        let mut entries = list_dir(&dir, &opts, || false).map_err(|e| OpError::Invalid { message: e.to_string() })?;
        sort_entries(&mut entries);
        let items = entries.into_iter().map(|e| (e.name, e.kind == EntryKind::Folder)).collect();
        let home = home_dir();
        Ok(Arc::new(EditSession { inner: tilde_edit::Session::new(dir, home.clone(), items), home }))
    }

    pub fn text(&self) -> String {
        self.inner.text()
    }

    /// 0-based line of the item called `name`.
    pub fn line_of(&self, name: String) -> Option<u32> {
        self.inner.line_of(&name).map(|l| l as u32)
    }

    /// What applying `text` would do, and any lines that need fixing first.
    pub fn preview(&self, text: String) -> EditPreview {
        let diff = self.inner.diff(&text);
        EditPreview {
            changes: diff
                .changes
                .iter()
                .map(|c| {
                    let (kind, description) = c.describe(&self.home);
                    PendingChange { kind: kind.to_string(), description }
                })
                .collect(),
            errors: diff
                .errors
                .into_iter()
                .map(|e| EditLineError { line: e.line as u32, message: e.message })
                .collect(),
        }
    }
}

impl EditSession {
    pub(crate) fn dir(&self) -> &Path {
        self.inner.dir()
    }
}
