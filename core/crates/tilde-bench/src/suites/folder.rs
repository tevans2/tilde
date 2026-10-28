//! The open folder: listing it, sorting it and matching typed text in it (the Here
//! results), for folders of 100 to 50,000 items.

use tilde_fs::{ListOptions, list_dir, sort_entries_by};
use tilde_query::{HereMatcher, Query};
use tilde_types::{SortKey, SortOrder};

use super::Ctx;
use crate::data;
use crate::runner::Runner;

const SUITE: &str = "folder";

pub fn run(r: &mut Runner, ctx: &Ctx) {
    let largest = ctx.trees.iter().copied().max().unwrap_or(10_000);
    for n in [100usize, 1_000, 10_000, 50_000].into_iter().filter(|&n| n <= largest.max(10_000)) {
        let ds = format!("flat-{}", data::label(n));
        let dir = data::flat_folder(n);
        let opts = ListOptions::default();
        r.bench(SUITE, &ds, "list", || list_dir(&dir, &opts, || false).map(|e| e.len()).unwrap_or(0));
        let entries = list_dir(&dir, &opts, || false).expect("list folder");
        for key in [SortKey::Name, SortKey::Modified, SortKey::Size, SortKey::Kind] {
            let order = SortOrder { key, reverse: false, folders_first: true };
            r.bench_setup(
                SUITE,
                &ds,
                &format!("sort by {}", key.as_str()),
                || entries.clone(),
                |mut e| {
                    sort_entries_by(&mut e, order);
                    e.len()
                },
            );
        }
        let mut matcher = HereMatcher::default();
        for (label, text) in [
            ("1 char", "r"),
            ("word", "report"),
            ("two words", "report pdf"),
            ("filter", "ext:pdf"),
            ("no match", "zzqxj"),
        ] {
            let q = Query::parse(text);
            r.bench(SUITE, &ds, &format!("match: {label}"), || matcher.rank(&entries, &q, data::NOW).len());
        }
    }
}
