//! Loads a synthetic index of N paths (default 2,000,000) through the saved-index path and
//! times searches, including typing a query one character at a time (nucleo reuses the
//! previous pass when the pattern grows).
//!
//!   cargo run --release -p tilde-index --example bench_synthetic -- [items] [query ...]
//!
//! The index is written by this process and loaded by a fresh child (`--load <dir>`), so
//! the reported memory is what a launch with that index uses.
use std::time::{Duration, Instant};

use tilde_index::{GlobalIndex, IndexConfig, ItemMeta, ScanConfig, write_saved};
use tilde_query::Query;
use tilde_types::EntryKind;

const WORDS: &[&str] = &[
    "src", "docs", "assets", "images", "projects", "archive", "notes", "client", "server", "build", "tests",
    "invoices", "photos", "music", "reports", "design", "tilde", "config", "scripts", "vendor", "lib", "app", "core",
    "ui", "data", "models", "drafts", "2023", "2024", "2025", "taxes", "receipts", "travel", "work",
];
const EXTS: &[&str] = &["rs", "swift", "md", "pdf", "png", "jpg", "txt", "json", "toml", "js", "ts", "zip", "mov"];

/// xorshift64*: deterministic and dependency-free.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[(self.next() % xs.len() as u64) as usize]
    }
}

/// Folders of 2–7 levels, each holding a few dozen files.
fn synthetic(n: usize) -> Vec<(String, ItemMeta)> {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let now = 1_760_000_000;
    let mut items = Vec::with_capacity(n);
    let mut id = 0u32;
    while items.len() < n {
        let depth = 2 + (rng.next() % 6) as usize;
        let mut dir = String::new();
        for d in 0..depth {
            if d > 0 {
                dir.push('/');
            }
            dir.push_str(rng.pick(WORDS));
            if rng.next().is_multiple_of(3) {
                dir.push_str(&format!("-{}", rng.next() % 100));
            }
        }
        let meta = |kind, id| ItemMeta {
            id,
            kind,
            size: 0,
            modified: now - (id as i64 % 5_000_000),
            child_count: None,
            extra: false,
        };
        items.push((dir.clone(), meta(EntryKind::Folder, id)));
        id += 1;
        for _ in 0..(10 + rng.next() % 50) {
            let name = format!("{}_{}{}.{}", rng.pick(WORDS), rng.pick(WORDS), rng.next() % 1000, rng.pick(EXTS));
            items.push((format!("{dir}/{name}"), meta(EntryKind::File, id)));
            id += 1;
        }
    }
    items.truncate(n);
    items
}

fn config(dir: &std::path::Path) -> IndexConfig {
    IndexConfig {
        home: dir.to_path_buf(),
        scan: ScanConfig { roots: vec![dir.to_path_buf()], ..Default::default() },
        frecency: None,
        zoxide: false,
        max_items: 10_000_000,
        cache_path: Some(dir.join("index.bin")),
    }
}

fn main() {
    let mut args = std::env::args().skip(1).peekable();
    if args.peek().map(String::as_str) == Some("--load") {
        args.next();
        let dir = std::path::PathBuf::from(args.next().unwrap());
        return load_and_search(&dir, args.collect());
    }
    let n: usize = args.peek().and_then(|a| a.parse().ok()).inspect(|_| drop(args.next())).unwrap_or(2_000_000);
    let queries: Vec<String> = args.collect();

    let root = tempfile::tempdir().unwrap();
    let t = Instant::now();
    let items = synthetic(n);
    println!("generate: {} paths in {:.2?}", items.len(), t.elapsed());
    let t = Instant::now();
    let bytes = write_saved(&config(root.path()), &items).unwrap();
    println!("write saved index: {} MB in {:.2?}", bytes / 1_048_576, t.elapsed());
    drop(items);
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--load")
        .arg(root.path())
        .args(queries)
        .status()
        .unwrap();
    assert!(status.success());
}

fn peak_mb() -> i64 {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    usage.ru_maxrss / 1_048_576
}

fn load_and_search(dir: &std::path::Path, mut queries: Vec<String>) {
    if queries.is_empty() {
        queries = ["invoice", "ext:pdf", "tilde swift", "receipts 2024 pdf", "zzqx"].map(String::from).to_vec();
    }
    let t = Instant::now();
    let mut index = GlobalIndex::start(config(dir), || {});
    while index.status().scanning {
        std::thread::sleep(Duration::from_millis(5));
    }
    println!("load: {} items in {:.2?}", index.status().items, t.elapsed());
    println!("memory after load: {} MB peak (estimate {} MB)", peak_mb(), index.handle().memory_estimate() / 1_048_576);
    let now = 1_760_000_000;
    let mut run = |q: &str| {
        let t = Instant::now();
        let hits = index.search(&Query::parse(q), None, None, now, 200);
        (hits, t.elapsed())
    };
    println!("full searches (fresh pattern each time):");
    for q in &queries {
        // A non-prefix query first, so each measured search starts from scratch.
        run("qqqq");
        let (hits, took) = run(q);
        let top: Vec<&str> = hits.iter().take(2).map(|h| h.key.as_str()).collect();
        println!("  {q:>18}: {:>4} hits in {:>9.2?}  {top:?}", hits.len(), took);
    }
    println!("typing \"receipts\" one key at a time:");
    run("qqqq");
    let mut typed = String::new();
    for c in "receipts".chars() {
        typed.push(c);
        let (hits, took) = run(&typed);
        println!("  {typed:>18}: {:>4} hits in {:>9.2?}", hits.len(), took);
    }

    println!("peak memory: {} MB", peak_mb());
}
