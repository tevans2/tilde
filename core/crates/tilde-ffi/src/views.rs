use std::path::Path;

use tilde_query::{Hit, Kind, kind_of};
use tilde_types::{Entry, EntryKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RowKind {
    File,
    Folder,
    Package,
    Symlink,
    Other,
}

/// Broad file category, for type badges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Category {
    Folder,
    App,
    Image,
    Video,
    Audio,
    Pdf,
    Document,
    Code,
    Text,
    Archive,
    Other,
}

/// A highlighted span of the name, in UTF-16 code units (what `NSString` uses).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct HighlightRange {
    pub start: u32,
    pub length: u32,
}

/// Everything the UI needs to draw one row.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct RowView {
    pub name: String,
    pub path: String,
    pub kind: RowKind,
    pub category: Category,
    /// Lowercased extension without the dot.
    pub ext: String,
    pub size: u64,
    /// Seconds since the Unix epoch.
    pub modified: i64,
    pub child_count: Option<u32>,
    pub highlights: Vec<HighlightRange>,
}

impl RowView {
    pub(crate) fn new(folder: &Path, entry: &Entry, hit: &Hit) -> RowView {
        RowView {
            name: entry.name.clone(),
            path: folder.join(&entry.name).to_string_lossy().into_owned(),
            kind: match entry.kind {
                EntryKind::File => RowKind::File,
                EntryKind::Folder => RowKind::Folder,
                EntryKind::Package => RowKind::Package,
                EntryKind::Symlink => RowKind::Symlink,
                EntryKind::Other => RowKind::Other,
            },
            category: match kind_of(entry) {
                Kind::Folder => Category::Folder,
                Kind::App => Category::App,
                Kind::Image => Category::Image,
                Kind::Video => Category::Video,
                Kind::Audio => Category::Audio,
                Kind::Pdf => Category::Pdf,
                Kind::Document => Category::Document,
                Kind::Code => Category::Code,
                Kind::Text => Category::Text,
                Kind::Archive => Category::Archive,
                Kind::Other => Category::Other,
            },
            ext: entry.extension(),
            size: entry.size,
            modified: entry.modified,
            child_count: entry.child_count,
            highlights: utf16_ranges(&entry.name, &hit.indices),
        }
    }
}

/// Converts sorted char indices into merged UTF-16 ranges.
pub(crate) fn utf16_ranges(name: &str, char_indices: &[u32]) -> Vec<HighlightRange> {
    let mut ranges: Vec<HighlightRange> = Vec::new();
    let mut wanted = char_indices.iter().copied().peekable();
    let mut utf16_pos = 0u32;
    for (i, c) in name.chars().enumerate() {
        let len = c.len_utf16() as u32;
        if wanted.peek() == Some(&(i as u32)) {
            wanted.next();
            match ranges.last_mut() {
                Some(r) if r.start + r.length == utf16_pos => r.length += len,
                _ => ranges.push(HighlightRange { start: utf16_pos, length: len }),
            }
        }
        utf16_pos += len;
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_adjacent_and_counts_surrogate_pairs() {
        assert_eq!(
            utf16_ranges("abcdef", &[0, 1, 2, 4]),
            [HighlightRange { start: 0, length: 3 }, HighlightRange { start: 4, length: 1 }]
        );
        // "📁" is one char but two UTF-16 units.
        assert_eq!(
            utf16_ranges("📁ab", &[0, 2]),
            [HighlightRange { start: 0, length: 2 }, HighlightRange { start: 3, length: 1 }]
        );
    }
}
