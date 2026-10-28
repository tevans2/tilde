//! Folder sizes: walking a tree to total it (what the background measurer does per
//! folder). Warm disk cache after the first sample.

use std::path::Path;

use super::Ctx;
use crate::data;
use crate::runner::{Runner, info};

const SUITE: &str = "sizes";

pub fn run(r: &mut Runner, ctx: &Ctx) {
    for &n in &ctx.trees {
        let ds = format!("tree-{}", data::label(n));
        measure(r, &ds, "whole tree", &data::home_tree(n));
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

fn measure(r: &mut Runner, ds: &str, name: &str, dir: &Path) {
    r.bench_slow(
        SUITE,
        ds,
        name,
        || (),
        |_| {
            let mut folders = 0usize;
            let t = std::time::Instant::now();
            let total = tilde_fs::folder_size(dir, usize::MAX, &|| false, &mut |_, _| folders += 1)
                .map_err(|e| e.to_string())?;
            Ok(info([
                ("total_bytes", total as f64),
                ("folders", folders as f64),
                ("folders_per_sec", folders as f64 / t.elapsed().as_secs_f64()),
            ]))
        },
    );
}
