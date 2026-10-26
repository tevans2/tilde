use std::path::Path;

use tilde_index::GlobalHit;
use tilde_query::{Hit, Kind, kind_for};
use tilde_types::{Entry, EntryKind, extension_of};

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

/// Which part of the results a row belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Section {
    /// Inside the current folder.
    Here,
    /// From the global index.
    Everywhere,
}

/// Where an Activity item came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ActivityOrigin {
    Download,
    Screenshot,
    AirDrop,
    Created,
    Edited,
}

impl From<tilde_activity::Origin> for ActivityOrigin {
    fn from(o: tilde_activity::Origin) -> Self {
        match o {
            tilde_activity::Origin::Download => ActivityOrigin::Download,
            tilde_activity::Origin::Screenshot => ActivityOrigin::Screenshot,
            tilde_activity::Origin::AirDrop => ActivityOrigin::AirDrop,
            tilde_activity::Origin::Created => ActivityOrigin::Created,
            tilde_activity::Origin::Edited => ActivityOrigin::Edited,
        }
    }
}

/// A highlighted span of text, in UTF-16 code units (what `NSString` uses).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct HighlightRange {
    pub start: u32,
    pub length: u32,
}

/// Everything the UI needs to draw one row.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct RowView {
    pub name: String,
    /// Absolute path.
    pub path: String,
    pub section: Section,
    /// The containing folder for display (`~/Developer/projects`). Empty for Here rows.
    pub location: String,
    pub kind: RowKind,
    pub category: Category,
    /// Lowercased extension without the dot.
    pub ext: String,
    pub size: u64,
    /// Seconds since the Unix epoch.
    pub modified: i64,
    pub child_count: Option<u32>,
    /// Matched characters in `name`.
    pub highlights: Vec<HighlightRange>,
    /// Matched characters in `location`.
    pub location_highlights: Vec<HighlightRange>,
    /// Activity rows only: where the item came from.
    pub origin: Option<ActivityOrigin>,
    /// Activity rows only: the site a download came from, or empty.
    pub source: String,
    /// Activity rows only: downloaded, a screenshot or AirDropped, and moved out of the
    /// inbox since.
    pub filed: bool,
}

impl RowView {
    /// A row from the current folder.
    /// `show_added`: the date shown is when the item arrived, not when it changed.
    pub(crate) fn here(folder: &Path, entry: &Entry, hit: &Hit, show_added: bool) -> RowView {
        RowView {
            name: entry.name.clone(),
            path: folder.join(&entry.name).to_string_lossy().into_owned(),
            section: Section::Here,
            location: String::new(),
            kind: row_kind(entry.kind),
            category: category(entry.kind, &entry.name),
            ext: entry.extension(),
            size: entry.size,
            modified: if show_added && entry.added > 0 { entry.added } else { entry.modified },
            child_count: entry.child_count,
            highlights: utf16_ranges(&entry.name, &hit.indices),
            location_highlights: Vec::new(),
            origin: None,
            source: String::new(),
            filed: false,
        }
    }

    /// A row in the Activity view. `modified` is the item's feed time.
    pub(crate) fn activity(home: &Path, item: &tilde_activity::Item, hit: &Hit) -> RowView {
        let parent = item.path.parent().unwrap_or(Path::new("/"));
        let location = match parent.strip_prefix(home) {
            Ok(rel) if rel.as_os_str().is_empty() => "~".to_string(),
            Ok(rel) => format!("~/{}", rel.to_string_lossy()),
            Err(_) => parent.to_string_lossy().into_owned(),
        };
        RowView {
            name: item.name.clone(),
            path: item.path.to_string_lossy().into_owned(),
            section: Section::Here,
            location,
            kind: row_kind(item.kind),
            category: category(item.kind, &item.name),
            ext: extension_of(&item.name),
            size: item.size,
            modified: item.time,
            child_count: item.child_count,
            highlights: utf16_ranges(&item.name, &hit.indices),
            location_highlights: Vec::new(),
            origin: Some(item.origin.into()),
            source: item.source.clone().unwrap_or_default(),
            filed: item.filed,
        }
    }

    /// A row from the global index. Keys are relative to `home` unless absolute.
    pub(crate) fn everywhere(home: &Path, hit: &GlobalHit) -> RowView {
        let key = hit.key.as_str();
        let (parent, name) = key.rsplit_once('/').unwrap_or(("", key));
        let absolute = key.starts_with('/');
        let (location, prefix_chars) = match (absolute, parent.is_empty()) {
            (true, _) => (if parent.is_empty() { "/".to_string() } else { parent.to_string() }, 0),
            (false, true) => ("~".to_string(), 0),
            (false, false) => (format!("~/{parent}"), 2),
        };
        // Split the match positions (char indices into the key) between the folder and
        // the name.
        let parent_chars = parent.chars().count() as u32;
        let name_start = if parent.is_empty() { 0 } else { parent_chars + 1 };
        let name_indices: Vec<u32> = hit.indices.iter().filter(|&&i| i >= name_start).map(|i| i - name_start).collect();
        let location_indices: Vec<u32> = if parent.is_empty() {
            Vec::new()
        } else {
            hit.indices.iter().filter(|&&i| i < parent_chars).map(|i| i + prefix_chars).collect()
        };
        let path = if absolute { key.to_string() } else { home.join(key).to_string_lossy().into_owned() };
        RowView {
            name: name.to_string(),
            path,
            section: Section::Everywhere,
            location_highlights: utf16_ranges(&location, &location_indices),
            location,
            kind: row_kind(hit.meta.kind),
            category: category(hit.meta.kind, name),
            ext: extension_of(name),
            size: hit.meta.size,
            modified: hit.meta.modified,
            child_count: hit.meta.child_count,
            highlights: utf16_ranges(name, &name_indices),
            origin: None,
            source: String::new(),
            filed: false,
        }
    }
}

fn row_kind(kind: EntryKind) -> RowKind {
    match kind {
        EntryKind::File => RowKind::File,
        EntryKind::Folder => RowKind::Folder,
        EntryKind::Package => RowKind::Package,
        EntryKind::Symlink => RowKind::Symlink,
        EntryKind::Other => RowKind::Other,
    }
}

fn category(kind: EntryKind, name: &str) -> Category {
    match kind_for(kind, name) {
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

    #[test]
    fn everywhere_rows_split_highlights_between_folder_and_name() {
        use tilde_index::ItemMeta;
        let meta = ItemMeta { id: 0, kind: EntryKind::File, size: 1, modified: 0, child_count: None, extra: false };
        // "Developer/tilde/main.swift": D=0 … t=10 (in "tilde"), m=16 (name start).
        let hit = GlobalHit { key: "Developer/tilde/main.swift".into(), meta, indices: vec![0, 10, 16, 17], score: 0 };
        let row = RowView::everywhere(Path::new("/Users/me"), &hit);
        assert_eq!(row.name, "main.swift");
        assert_eq!(row.path, "/Users/me/Developer/tilde/main.swift");
        assert_eq!(row.location, "~/Developer/tilde");
        assert_eq!(row.highlights, [HighlightRange { start: 0, length: 2 }]);
        // Shifted by the "~/" prefix.
        assert_eq!(
            row.location_highlights,
            [HighlightRange { start: 2, length: 1 }, HighlightRange { start: 12, length: 1 }]
        );

        let top = GlobalHit { key: "Documents".into(), meta, indices: vec![0], score: 0 };
        let row = RowView::everywhere(Path::new("/Users/me"), &top);
        assert_eq!((row.name.as_str(), row.location.as_str()), ("Documents", "~"));
        assert_eq!(row.highlights, [HighlightRange { start: 0, length: 1 }]);

        let outside = GlobalHit { key: "/opt/homebrew".into(), meta, indices: vec![], score: 0 };
        let row = RowView::everywhere(Path::new("/Users/me"), &outside);
        assert_eq!(
            (row.name.as_str(), row.location.as_str(), row.path.as_str()),
            ("homebrew", "/opt", "/opt/homebrew")
        );
    }
}
