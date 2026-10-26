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
    /// Another filesystem is mounted here (a disk image, network share, container
    /// filesystem). Recursive scans don't cross into it.
    pub mount_point: bool,
    /// When the item arrived in this folder (created, downloaded, moved or copied in),
    /// seconds since the Unix epoch. 0 if unknown.
    pub added: i64,
}

impl Entry {
    /// Lowercased extension without the dot, or an empty string.
    pub fn extension(&self) -> String {
        extension_of(&self.name)
    }
}

/// Lowercased extension of a file name without the dot, or an empty string. Dotfiles
/// like `.zshrc` have no extension.
pub fn extension_of(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => name[i + 1..].to_lowercase(),
        _ => String::new(),
    }
}

/// Calls `f` with the lowercase extension of `name` (empty if none), like
/// [`extension_of`] but without allocating for the usual short ASCII extension. Used where
/// it runs for every item in the index.
pub fn with_extension<R>(name: &str, f: impl FnOnce(&str) -> R) -> R {
    let ext = match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => &name[i + 1..],
        _ => "",
    };
    let mut buf = [0u8; 16];
    if ext.len() <= buf.len() && ext.is_ascii() {
        let lower = &mut buf[..ext.len()];
        lower.copy_from_slice(ext.as_bytes());
        lower.make_ascii_lowercase();
        // SAFETY: lowercased ASCII is still ASCII.
        f(unsafe { std::str::from_utf8_unchecked(lower) })
    } else {
        f(&ext.to_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_extension_matches_extension_of() {
        for name in
            ["a.PDF", "archive.tar.GZ", ".zshrc", "noext", "x.", "photo.HÉIC", "long.abcdefghijklmnopqrstuvwxyz"]
        {
            assert_eq!(with_extension(name, str::to_string), extension_of(name), "{name}");
        }
    }

    fn entry(name: &str) -> Entry {
        Entry {
            name: name.into(),
            kind: EntryKind::File,
            size: 0,
            modified: 0,
            file_id: 0,
            hidden: false,
            child_count: None,
            mount_point: false,
            added: 0,
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
