use std::cmp::{Ordering, Reverse};

use tilde_types::{Entry, EntryKind, SortKey, SortOrder, with_extension};

/// Finder-style ordering: case-insensitive, with runs of digits compared by value,
/// so `file2` sorts before `file10`.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    natural_key(a).cmp(&natural_key(b))
}

/// A token sequence whose lexicographic order is natural order, so a sort can build
/// keys once instead of re-scanning names on every comparison.
///
/// Letters become their lowercase code points. A digit run becomes: a marker equal to
/// `'0'` (so it orders like a digit against other characters), the number of significant
/// digits, the significant digits, then the run's full length (so `1` < `01` < `001`).
/// After a marker matches, both sides are digit runs, so the counts line up.
pub fn natural_key(s: &str) -> Vec<u32> {
    let mut key = Vec::with_capacity(s.len() + 4);
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() {
            let mut digits = vec![c as u32];
            while let Some(d) = chars.next_if(char::is_ascii_digit) {
                digits.push(d as u32);
            }
            let first_significant = digits.iter().position(|&d| d != '0' as u32).unwrap_or(digits.len());
            let significant = &digits[first_significant..];
            key.push('0' as u32);
            key.push(significant.len() as u32);
            key.extend_from_slice(significant);
            key.push(digits.len() as u32);
        } else {
            key.extend(c.to_lowercase().map(|l| l as u32));
        }
    }
    key
}

/// Folders first, then everything else, each group in natural order.
pub fn sort_entries(entries: &mut [Entry]) {
    sort_entries_by(entries, SortOrder::default());
}

/// Sorts by `order.key` in its natural direction (A→Z, newest first, largest first), or
/// the other way with `order.reverse`. Ties go by name, A→Z. For `Size`, folders are
/// compared by item count among themselves.
pub fn sort_entries_by(entries: &mut [Entry], order: SortOrder) {
    // A number that sorts the natural direction ascending, flipped for reverse.
    let primary = |e: &Entry| -> i128 {
        let v: i128 = match order.key {
            SortKey::Name | SortKey::Kind => 0,
            SortKey::Modified => -(e.modified as i128),
            SortKey::Added => -(e.added as i128),
            SortKey::Size => match e.kind {
                EntryKind::Folder => -(e.child_count.unwrap_or(0) as i128),
                _ => -(e.size as i128),
            },
        };
        if order.reverse { -v } else { v }
    };
    entries.sort_by_cached_key(|e| {
        let is_folder = e.kind == EntryKind::Folder;
        // Item counts and byte sizes don't compare, so a size sort keeps folders apart
        // (after the files, unless they come first anyway).
        let folder_rank = if order.folders_first { !is_folder } else { order.key == SortKey::Size && is_folder };
        let ext = match order.key {
            SortKey::Kind if e.kind != EntryKind::Folder => with_extension(&e.name, |x| x.to_string()),
            _ => String::new(),
        };
        let name = natural_key(&e.name);
        // Name and kind reverse by flipping the text keys; ties stay A→Z otherwise.
        let flip = order.reverse && matches!(order.key, SortKey::Name | SortKey::Kind);
        let text = if flip { (Some(Reverse((ext, name.clone()))), None) } else { (None, Some((ext, name.clone()))) };
        (folder_rank, primary(e), text, name)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(names: &[&str]) -> Vec<String> {
        let mut v: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        v.sort_by(|a, b| natural_cmp(a, b));
        v
    }

    #[test]
    fn numbers_compare_by_value() {
        assert_eq!(sorted(&["file10", "file2", "file1", "File3"]), ["file1", "file2", "File3", "file10"]);
    }

    #[test]
    fn leading_zeros_and_huge_numbers() {
        assert_eq!(sorted(&["a01", "a1", "a001"]), ["a1", "a01", "a001"]);
        assert_eq!(sorted(&["x99999999999999999999999", "x2"]), ["x2", "x99999999999999999999999"]);
    }

    #[test]
    fn case_insensitive_and_prefixes() {
        assert_eq!(sorted(&["b", "A", "a", "ab"]), ["A", "a", "ab", "b"]);
    }

    #[test]
    fn digits_order_like_characters_against_punctuation_and_letters() {
        // '-' < digits < letters, as in a character-by-character comparison.
        assert_eq!(sorted(&["ab", "a1", "a-", "a"]), ["a", "a-", "a1", "ab"]);
        assert_eq!(sorted(&["v1.10", "v1.9", "v1.9.1"]), ["v1.9", "v1.9.1", "v1.10"]);
    }

    fn mk(name: &str, kind: EntryKind) -> Entry {
        Entry {
            name: name.into(),
            kind,
            size: 0,
            modified: 0,
            file_id: 0,
            hidden: false,
            child_count: None,
            mount_point: false,
            added: 0,
        }
    }

    fn names(v: &[Entry]) -> Vec<&str> {
        v.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn sorts_by_each_key_both_ways() {
        let file = |name: &str, size, modified, added| Entry { size, modified, added, ..mk(name, EntryKind::File) };
        let mut dir = mk("photos", EntryKind::Folder);
        dir.child_count = Some(3);
        let mut v = vec![
            file("b.txt", 300, 10, 50),
            file("a.pdf", 100, 30, 40),
            file("c.txt", 200, 20, 60),
            file("d.pdf", 200, 20, 60),
            dir,
        ];
        let order = |key, reverse| SortOrder { key, reverse, folders_first: true };
        let mut by = |key, reverse| {
            sort_entries_by(&mut v, order(key, reverse));
            names(&v).join(" ")
        };
        assert_eq!(by(SortKey::Name, false), "photos a.pdf b.txt c.txt d.pdf");
        assert_eq!(by(SortKey::Name, true), "photos d.pdf c.txt b.txt a.pdf");
        assert_eq!(by(SortKey::Modified, false), "photos a.pdf c.txt d.pdf b.txt", "newest first, ties by name");
        assert_eq!(by(SortKey::Modified, true), "photos b.txt c.txt d.pdf a.pdf", "ties stay A→Z");
        assert_eq!(by(SortKey::Added, false), "photos c.txt d.pdf b.txt a.pdf");
        assert_eq!(by(SortKey::Size, false), "photos b.txt c.txt d.pdf a.pdf", "largest first");
        assert_eq!(by(SortKey::Size, true), "photos a.pdf c.txt d.pdf b.txt");
        assert_eq!(by(SortKey::Kind, false), "photos a.pdf d.pdf b.txt c.txt", "pdf before txt");

        sort_entries_by(&mut v, SortOrder { key: SortKey::Size, reverse: false, folders_first: false });
        assert_eq!(names(&v), ["b.txt", "c.txt", "d.pdf", "a.pdf", "photos"], "folders after files");
        sort_entries_by(&mut v, SortOrder { key: SortKey::Name, reverse: false, folders_first: false });
        assert_eq!(names(&v), ["a.pdf", "b.txt", "c.txt", "d.pdf", "photos"], "mixed in by name");
    }

    #[test]
    fn folders_sort_first() {
        let mut v = vec![mk("b.txt", EntryKind::File), mk("z", EntryKind::Folder), mk("a.txt", EntryKind::File)];
        sort_entries(&mut v);
        assert_eq!(v.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["z", "a.txt", "b.txt"]);
    }
}
