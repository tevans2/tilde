//! Scans the home folder with the default exclusions and times some searches.
//!
//!   cargo run --release -p tilde-index --example bench_index -- [query ...]
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tilde_index::{GlobalIndex, IndexConfig, ScanConfig};
use tilde_query::Query;

fn main() {
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    let config = IndexConfig {
        scan: ScanConfig {
            roots: vec![home.clone()],
            exclude_names: [
                "node_modules",
                ".git",
                "target",
                "build",
                "DerivedData",
                "Pods",
                "__pycache__",
                ".venv",
                "venv",
            ]
            .map(String::from)
            .into(),
            exclude_paths: vec![home.join("Library"), home.join(".Trash")],
            ..Default::default()
        },
        home,
        zoxide: true,
        max_items: 5_000_000,
        cache_path: Some(std::env::temp_dir().join(format!("tilde-bench-{}.bin", std::process::id()))),
    };
    let cache = config.cache_path.clone().unwrap();

    let t = Instant::now();
    let first = GlobalIndex::start(config.clone(), || {});
    while first.status().scanning {
        std::thread::sleep(Duration::from_millis(20));
    }
    let handle = first.handle();
    println!("scan: {} items in {:.2?}", first.status().items, t.elapsed());
    println!("memory (estimate): {} MB", handle.memory_estimate() / 1_048_576);
    let t = Instant::now();
    let saved = handle.save().unwrap().unwrap();
    println!("save: {} MB in {:.2?}", saved / 1_048_576, t.elapsed());
    for f in handle.largest_folders(5) {
        println!("  largest: {:>7} items  ~/{}", f.items, f.key);
    }
    drop(first);

    let t = Instant::now();
    let mut index = GlobalIndex::start(config, || {});
    println!("load: {} items in {:.2?}", index.status().items, t.elapsed());
    let _ = std::fs::remove_file(&cache);

    let mut queries: Vec<String> = std::env::args().skip(1).collect();
    if queries.is_empty() {
        queries =
            ["t", "ti", "til", "tild", "tilde", "tilde swift", "readme", "ext:pdf", "invoice ext:pdf", "projects"]
                .map(String::from)
                .to_vec();
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    for q in &queries {
        let t = Instant::now();
        let hits = index.search(&Query::parse(q), None, None, now, 200);
        let took = t.elapsed();
        let top: Vec<&str> = hits.iter().take(3).map(|h| h.key.as_str()).collect();
        println!("{q:>18}: {:>4} hits in {:>8.2?}  {top:?}", hits.len(), took);
    }

    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    println!("peak memory: {} MB", usage.ru_maxrss / 1_048_576);
}
