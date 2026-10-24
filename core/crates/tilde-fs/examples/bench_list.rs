//! Compares `list_dir` (getattrlistbulk) with `read_dir` + `metadata` on a generated folder.
//!
//!   cargo run --release -p tilde-fs --example bench_list -- [file_count]
use std::time::Instant;

use tilde_fs::{ListOptions, list_dir, sort_entries};

fn main() {
    let count: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(10_000);
    let dir = tempfile::tempdir().unwrap();
    for i in 0..count {
        std::fs::write(dir.path().join(format!("file-{i:06}.txt")), b"x").unwrap();
    }

    let runs = 20;
    let mut best_bulk = f64::MAX;
    let mut best_sort = f64::MAX;
    let mut best_std = f64::MAX;
    for _ in 0..runs {
        let t = Instant::now();
        let mut entries = list_dir(dir.path(), &ListOptions::default(), || false).unwrap();
        best_bulk = best_bulk.min(t.elapsed().as_secs_f64() * 1e3);
        let t = Instant::now();
        sort_entries(&mut entries);
        best_sort = best_sort.min(t.elapsed().as_secs_f64() * 1e3);
        assert_eq!(entries.len(), count);

        let t = Instant::now();
        let n = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                let m = e.metadata().unwrap();
                (e.file_name(), m.len(), m.modified().unwrap())
            })
            .count();
        best_std = best_std.min(t.elapsed().as_secs_f64() * 1e3);
        assert_eq!(n, count);
    }
    println!("{count} files, best of {runs}:");
    println!("  getattrlistbulk        : {best_bulk:7.2} ms");
    println!("  natural sort           : {best_sort:7.2} ms");
    println!("  read_dir + metadata    : {best_std:7.2} ms (unsorted)");
}
