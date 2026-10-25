//! Query language and ranking.
//!
//! A query is plain search terms mixed with filters such as `ext:pdf` or `mod:<7d`.
//! Terms are matched with nucleo (fzf-style scoring), so its pattern syntax works too:
//! `^start`, `end$`, `'exact`, `!exclude`.

mod kind;
mod parse;

pub use kind::{Kind, kind_for, kind_of};
pub use parse::{Candidate, Filter, FilterContext, Query, SizeBound};

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use tilde_types::Entry;

/// One matching entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// Index into the slice that was ranked.
    pub index: u32,
    pub score: u32,
    /// Matched character positions in the entry name (char indices, sorted).
    pub indices: Vec<u32>,
}

/// Ranks the entries of one folder (the "Here" scope).
pub struct HereMatcher {
    matcher: Matcher,
    buf: Vec<char>,
}

impl Default for HereMatcher {
    fn default() -> Self {
        HereMatcher { matcher: Matcher::new(Config::DEFAULT), buf: Vec::new() }
    }
}

impl HereMatcher {
    /// Returns matching entries, best first. Without search terms, filtered entries keep
    /// their original order. `now` is the current Unix time, for `mod:` filters.
    pub fn rank(&mut self, entries: &[Entry], query: &Query, now: i64) -> Vec<Hit> {
        let pattern =
            (!query.text.is_empty()).then(|| Pattern::parse(&query.text, CaseMatching::Ignore, Normalization::Smart));
        let mut hits = Vec::new();
        let mut indices = Vec::new();

        for (i, entry) in entries.iter().enumerate() {
            if !query.filters.iter().all(|f| f.matches(entry, now)) {
                continue;
            }
            let Some(pattern) = &pattern else {
                hits.push(Hit { index: i as u32, score: 0, indices: Vec::new() });
                continue;
            };
            indices.clear();
            let haystack = Utf32Str::new(&entry.name, &mut self.buf);
            if let Some(score) = pattern.indices(haystack, &mut self.matcher, &mut indices) {
                indices.sort_unstable();
                indices.dedup();
                hits.push(Hit { index: i as u32, score, indices: indices.clone() });
            }
        }
        if pattern.is_some() {
            // Stable, so equal scores keep folder order.
            hits.sort_by_key(|h| std::cmp::Reverse(h.score));
        }
        hits
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tilde_types::EntryKind;

    fn file(name: &str, size: u64, modified: i64) -> Entry {
        Entry {
            name: name.into(),
            kind: EntryKind::File,
            size,
            modified,
            file_id: 0,
            hidden: false,
            child_count: None,
            mount_point: false,
            added: 0,
        }
    }

    fn folder(name: &str) -> Entry {
        Entry { kind: EntryKind::Folder, child_count: Some(0), ..file(name, 0, 0) }
    }

    fn ranked(entries: &[Entry], q: &str) -> Vec<String> {
        let hits = HereMatcher::default().rank(entries, &Query::parse(q), 1_000_000);
        hits.iter().map(|h| entries[h.index as usize].name.clone()).collect()
    }

    #[test]
    fn empty_query_keeps_everything_in_order() {
        let e = [folder("src"), file("b.txt", 1, 0), file("a.txt", 1, 0)];
        assert_eq!(ranked(&e, ""), ["src", "b.txt", "a.txt"]);
    }

    #[test]
    fn fuzzy_ranking_prefers_word_starts_and_runs() {
        let e =
            [file("receipt_8841.pdf", 1, 0), file("invoice-oct-2026.pdf", 1, 0), file("slides-final-FINAL.key", 1, 0)];
        assert_eq!(ranked(&e, "inv"), ["invoice-oct-2026.pdf"]);
        assert_eq!(ranked(&e, "pdf")[..2].len(), 2);
        assert_eq!(ranked(&e, "zzz"), Vec::<String>::new());
    }

    #[test]
    fn multiple_terms_must_all_match() {
        let e = [file("invoice-oct-2026.pdf", 1, 0), file("invoice-sep-2026.pdf", 1, 0)];
        assert_eq!(ranked(&e, "inv oct"), ["invoice-oct-2026.pdf"]);
    }

    #[test]
    fn filters_combine_with_terms() {
        let now = 1_000_000;
        let e = [
            file("old.pdf", 10, now - 30 * 86_400),
            file("new.pdf", 10, now - 3_600),
            file("new.png", 5_000_000, now - 60),
            folder("newsletters"),
        ];
        let rank = |q: &str| -> Vec<String> {
            let hits = HereMatcher::default().rank(&e, &Query::parse(q), now);
            hits.iter().map(|h| e[h.index as usize].name.clone()).collect()
        };
        assert_eq!(rank("ext:pdf"), ["old.pdf", "new.pdf"]);
        assert_eq!(rank("ext:pdf mod:<7d"), ["new.pdf"]);
        assert_eq!(rank("size:>1M"), ["new.png"]);
        assert_eq!(rank("kind:folder"), ["newsletters"]);
        assert_eq!(rank("new kind:image"), ["new.png"]);
    }

    #[test]
    fn indices_are_char_positions() {
        // "ï" is two bytes in UTF-8, so byte offsets would give [4, 7].
        let e = [file("naïve-file.txt", 1, 0)];
        let hits = HereMatcher::default().rank(&e, &Query::parse("vf"), 0);
        assert_eq!(hits[0].indices, [3, 6]);
    }
}
