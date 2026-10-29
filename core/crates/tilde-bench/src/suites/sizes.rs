//! Folder sizes: walking a tree to total it (what the background measurer does for
//! folders the index doesn't list), and the index's own totals, which only need the
//! folders it doesn't enter (`.git`, `node_modules`…) walked. Warm disk cache after the
//! first sample.

use std::path::Path;

use super::{Ctx, wait_ready};
use crate::data;
use crate::runner::{Runner, info};

const SUITE: &str = "sizes";

pub fn run(r: &mut Runner, ctx: &Ctx) {
    for &n in &ctx.trees {
        let ds = format!("tree-{}", data::label(n));
        let root = data::home_tree(n);
        measure(r, &ds, "whole tree", &root);
        from_index(r, &ds, &root);
    }
    if ctx.real_heavy {
        let home = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default());
        for name in ["Developer", "Documents", "Downloads", "Pictures"] {
            let dir = home.join(name);
            if dir.is_dir() {
                measure(r, "real", &format!("~/{name}"), &dir);
            }
        }
    }
}

/// The index's totals: walking only the folders it doesn't enter (once, then only when
/// something inside them changes), and reading the totals.
fn from_index(r: &mut Runner, ds: &str, root: &Path) {
    let index = tilde_index::GlobalIndex::start(data::index_config(root, None), || {});
    if let Err(e) = wait_ready(&index, std::time::Duration::from_secs(300)) {
        return r.fail(SUITE, ds, "index: walk the folders it skips", e);
    }
    let handle = index.handle();
    let roots = [root.to_path_buf()];
    r.bench_slow(
        SUITE,
        ds,
        "index: walk the folders it skips",
        || (),
        |_| {
            let skipped = handle.skipped_inside(&roots);
            let folders = std::sync::atomic::AtomicUsize::new(0);
            for s in &skipped {
                let size = tilde_fs::folder_size(&s.path, 0, &|| false, &|_, _| {
                    folders.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                })
                .map_err(|e| e.to_string())?;
                handle.set_skipped_size(&s.path, size);
            }
            Ok(info([("skipped_folders", skipped.len() as f64), ("folders_walked", folders.into_inner() as f64)]))
        },
    );
    // Same as a whole walk, or the case fails.
    let walked = tilde_fs::folder_size(root, 0, &|| false, &|_, _| {}).unwrap_or(0);
    match handle.folder_total(root) {
        Some(t) if t.bytes == walked && t.unmeasured == 0 => {}
        other => return r.fail(SUITE, ds, "index: totals", format!("index says {other:?}, a walk {walked}")),
    }
    let tops: Vec<std::path::PathBuf> = std::fs::read_dir(root)
        .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default();
    r.bench(SUITE, ds, "index: totals of the top-level folders", || {
        tops.iter().filter_map(|t| handle.folder_total(t)).count()
    });
}

fn measure(r: &mut Runner, ds: &str, name: &str, dir: &Path) {
    r.bench_slow(
        SUITE,
        ds,
        name,
        || (),
        |_| {
            let folders = std::sync::atomic::AtomicUsize::new(0);
            let t = std::time::Instant::now();
            let total = tilde_fs::folder_size(dir, usize::MAX, &|| false, &|_, _| {
                folders.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            })
            .map_err(|e| e.to_string())?;
            let folders = folders.into_inner();
            Ok(info([
                ("total_bytes", total as f64),
                ("folders", folders as f64),
                ("folders_per_sec", folders as f64 / t.elapsed().as_secs_f64()),
            ]))
        },
    );
}
