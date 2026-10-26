//! Times the duplicate finder on a folder (default: your home, without ~/Library and
//! hidden folders), the way Clean Up runs it, then again from the cache.
//!
//!     cargo run --release -p tilde-cleanup --example bench_dupes -- [folder] [cache.json]
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use tilde_cleanup::dupes;

fn walk(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let p = e.path();
        let Ok(m) = p.symlink_metadata() else { continue };
        if m.is_dir() {
            if p.ends_with("Library") && p.parent() == std::env::var_os("HOME").map(PathBuf::from).as_deref() {
                continue;
            }
            walk(&p, out);
        } else if m.is_file() && m.len() >= 1 << 20 {
            out.push(p);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let root = args.get(1).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap()));
    let cache_path =
        args.get(2).map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("tilde-bench-hashes.json"));
    let t = Instant::now();
    let mut paths = Vec::new();
    walk(&root, &mut paths);
    println!(
        "files >= 1 MB: {} (walk {:.2}s; Tilde gets these from the index instantly)",
        paths.len(),
        t.elapsed().as_secs_f64()
    );
    for run in ["cold", "cached"] {
        let mut cache = if run == "cold" { dupes::Cache::default() } else { dupes::Cache::load(&cache_path) };
        let start = Instant::now();
        let first = Mutex::new(None::<(f64, usize, u64)>);
        let read = Mutex::new(0u64);
        let groups = dupes::find(
            paths.clone(),
            1 << 20,
            &mut cache,
            &AtomicBool::new(false),
            &|g| {
                let mut f = first.lock().unwrap();
                if f.is_none() {
                    *f = Some((start.elapsed().as_secs_f64(), g.len(), g.iter().map(|g| g.reclaimable()).sum()));
                }
            },
            &|p| *read.lock().unwrap() = p.total,
        );
        let total = start.elapsed().as_secs_f64();
        let (t1, n1, b1) = first.lock().unwrap().unwrap_or((total, 0, 0));
        let reclaim: u64 = groups.iter().map(|g| g.reclaimable()).sum();
        println!(
            "{run:6}: likely after {t1:.2}s ({n1} groups, {:.2} GB) · confirmed after {total:.2}s ({} groups, {:.2} GB reclaimable) · read in full {:.2} GB",
            b1 as f64 / 1e9,
            groups.len(),
            reclaim as f64 / 1e9,
            *read.lock().unwrap() as f64 / 1e9
        );
        cache.save(&cache_path);
    }
}
