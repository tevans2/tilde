//! Building the index from disk (no saved index), and the folder walk under it, so
//! listing cost and indexing cost can be told apart. Disk caches are warm after the
//! first sample; a cold scan (after a restart) is slower.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use tilde_index::{GlobalIndex, walk_parallel};
use tilde_types::EntryKind;

use super::{Ctx, wait_ready};
use crate::data;
use crate::runner::{Runner, info};

const SUITE: &str = "scan";

pub fn run(r: &mut Runner, ctx: &Ctx) {
    for &n in &ctx.trees {
        let ds = format!("tree-{}", data::label(n));
        let root = data::home_tree(n).canonicalize().expect("tree");
        scan(r, &ds, data::index_config(&root, None));
        walk(r, &ds, &root);
    }
    if ctx.real_heavy {
        match tilde_ffi::user_index_config() {
            Some(mut config) => {
                config.cache_path = None;
                config.frecency = None;
                scan(r, "real", config.clone());
                walk(r, "real", &config.scan.roots[0]);
            }
            None => r.fail(SUITE, "real", "full scan", "indexing is off for this user".into()),
        }
    }
}

fn scan(r: &mut Runner, ds: &str, config: tilde_index::IndexConfig) {
    r.bench_slow(
        SUITE,
        ds,
        "full scan",
        || (),
        |_| {
            let t = std::time::Instant::now();
            let index = GlobalIndex::start(config.clone(), || {});
            wait_ready(&index, Duration::from_secs(1800))?;
            let items = index.status().items as f64;
            let counts = index.handle().counts();
            Ok(info([
                ("items", items),
                ("items_per_sec", items / t.elapsed().as_secs_f64()),
                ("folders", counts.folder_tables as f64),
            ]))
        },
    );
}

/// Listing every folder (hidden ones skipped, nothing excluded), without indexing.
fn walk(r: &mut Runner, ds: &str, root: &Path) {
    r.bench_slow(
        SUITE,
        ds,
        "walk only (list every folder)",
        || (),
        |_| {
            let t = std::time::Instant::now();
            let (stop, entries, folders) = (AtomicBool::new(false), AtomicUsize::new(0), AtomicUsize::new(0));
            walk_parallel(vec![root.to_path_buf()], false, &stop, &|dir, list| {
                folders.fetch_add(1, Ordering::Relaxed);
                entries.fetch_add(list.len(), Ordering::Relaxed);
                list.iter()
                    .filter(|e| e.kind == EntryKind::Folder && !e.mount_point)
                    .map(|e| dir.join(&e.name))
                    .collect::<Vec<PathBuf>>()
            });
            let n = entries.load(Ordering::Relaxed) as f64;
            Ok(info([
                ("entries", n),
                ("folders", folders.load(Ordering::Relaxed) as f64),
                ("entries_per_sec", n / t.elapsed().as_secs_f64()),
            ]))
        },
    );
}
