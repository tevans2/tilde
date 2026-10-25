use std::cmp::Ordering;

use tilde_types::{Entry, EntryKind};

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
    entries.sort_by_cached_key(|e| (e.kind != EntryKind::Folder, natural_key(&e.name)));
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

    #[test]
    fn folders_sort_first() {
        let mk = |name: &str, kind| Entry {
            name: name.into(),
            kind,
            size: 0,
            modified: 0,
            file_id: 0,
            hidden: false,
            child_count: None,
            mount_point: false,
            added: 0,
        };
        let mut v = vec![mk("b.txt", EntryKind::File), mk("z", EntryKind::Folder), mk("a.txt", EntryKind::File)];
        sort_entries(&mut v);
        assert_eq!(v.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["z", "a.txt", "b.txt"]);
    }
}
