//! Types shared by every Tilde crate.

/// What kind of filesystem object an entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryKind {
    File,
    Folder,
    /// A folder macOS presents as a single document (`.app`, `.rtfd`, `.key`, …).
    Package,
    Symlink,
    Other,
}

impl EntryKind {
    /// Folders you can navigate into. Packages are deliberately excluded.
    pub fn is_navigable(self) -> bool {
        matches!(self, EntryKind::Folder)
    }
}

/// One item in a folder listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub kind: EntryKind,
    /// Logical size in bytes. Zero for folders.
    pub size: u64,
    /// Modification time, seconds since the Unix epoch.
    pub modified: i64,
    /// Filesystem object ID (inode number on APFS).
    pub file_id: u64,
    /// Hidden by a leading dot or the `UF_HIDDEN` flag.
    pub hidden: bool,
    /// Number of entries inside, for folders and packages.
    pub child_count: Option<u32>,
}

impl Entry {
    /// Lowercased extension without the dot, or an empty string.
    pub fn extension(&self) -> String {
        match self.name.rfind('.') {
            Some(i) if i > 0 && i + 1 < self.name.len() => self.name[i + 1..].to_lowercase(),
            _ => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> Entry {
        Entry {
            name: name.into(),
            kind: EntryKind::File,
            size: 0,
            modified: 0,
            file_id: 0,
            hidden: false,
            child_count: None,
        }
    }

    #[test]
    fn extension_handles_dotfiles_and_trailing_dots() {
        assert_eq!(entry("report.PDF").extension(), "pdf");
        assert_eq!(entry("archive.tar.gz").extension(), "gz");
        assert_eq!(entry(".zshrc").extension(), "");
        assert_eq!(entry("README").extension(), "");
        assert_eq!(entry("odd.").extension(), "");
    }
}
