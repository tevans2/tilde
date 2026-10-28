//! Keeping the index current as files change, through real FSEvents on a real tree:
//! how long until a new, deleted or renamed item shows in search, and what editing many
//! files does to the index (dead entries, memory, search speed).

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tilde_index::GlobalIndex;
use tilde_query::Query;
use tilde_types::EntryKind;

use super::{Ctx, wait_ready};
use crate::probe::Cost;
use crate::runner::{Runner, info};
use crate::{alloc, data, probe};

const SUITE: &str = "live";
/// Longest wait for a change to show up.
const TIMEOUT: Duration = Duration::from_secs(10);
/// The index counts as settled after this long without applying a batch.
const QUIET: Duration = Duration::from_millis(1500);
/// Searches compared before and after edits.
const PROBES: &[&str] = &["report", "config", "ext:pdf"];

/// Counts batches the index applies, and when the last one was.
#[derive(Clone)]
struct Changes {
    count: Arc<AtomicU64>,
    last: Arc<Mutex<Instant>>,
}

impl Changes {
    fn new() -> Changes {
        Changes { count: Arc::new(AtomicU64::new(0)), last: Arc::new(Mutex::new(Instant::now())) }
    }

    /// Waits until the index has applied something after `since` and then been quiet;
    /// returns when the last batch was applied.
    fn settle(&self, since: u64, timeout: Duration) -> Result<Instant, String> {
        let t = Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(20));
            let last = *self.last.lock().unwrap();
            if self.count.load(Ordering::SeqCst) > since && last.elapsed() >= QUIET {
                return Ok(last);
            }
            if t.elapsed() > timeout {
                return Err(format!("the index didn't settle within {timeout:?}"));
            }
        }
    }
}

pub fn run(r: &mut Runner, ctx: &Ctx) {
    let ds = format!("tree-{}", data::label(ctx.live));
    let tmp = tempfile::tempdir_in(&ctx.scratch).expect("scratch tree");
    // FSEvents reports real paths (/private/var/…), so the root must be one.
    let root = tmp.path().canonicalize().expect("canonical root");
    let nodes = data::home(ctx.live);
    data::write_nodes(&root, &nodes);

    let changes = Changes::new();
    let on_change = changes.clone();
    let mut index = GlobalIndex::start(data::index_config(&root, None), move || {
        on_change.count.fetch_add(1, Ordering::SeqCst);
        *on_change.last.lock().unwrap() = Instant::now();
    });
    if let Err(e) = wait_ready(&index, Duration::from_secs(300)) {
        return r.fail(SUITE, &ds, "scan", e);
    }
    // Let the watcher start and anything from writing the tree drain.
    std::thread::sleep(Duration::from_millis(500));

    let folders: Vec<&str> = nodes
        .iter()
        .filter(|n| n.kind == EntryKind::Folder && !n.path.contains('.') && !n.path.contains("node_modules"))
        .map(|n| n.path.as_str())
        .collect();
    let samples = r.profile.min_samples.clamp(5, 20) as usize;

    // A new file showing up.
    let mut latencies = Vec::new();
    for i in 0..samples {
        let dir = folders[(i * 7919) % folders.len()];
        let name = format!("zqlive{i}x.txt");
        let path = root.join(dir).join(&name);
        let t = Instant::now();
        fs::write(&path, b"x").expect("write file");
        match wait_for(&mut index, &changes, &format!("zqlive{i}x"), &format!("{dir}/{name}"), true) {
            Ok(()) => latencies.push(t.elapsed()),
            Err(e) => return r.fail(SUITE, &ds, "new file shows up", e),
        }
    }
    r.record_samples(SUITE, &ds, "new file shows up", walls(&latencies), info([("note_watch_latency_ms", 200.0)]));

    // A deleted file going.
    let mut latencies = Vec::new();
    for i in 0..samples {
        let dir = folders[(i * 7919) % folders.len()];
        let name = format!("zqlive{i}x.txt");
        let path = root.join(dir).join(&name);
        let t = Instant::now();
        fs::remove_file(&path).expect("remove file");
        match wait_for(&mut index, &changes, &format!("zqlive{i}x"), &format!("{dir}/{name}"), false) {
            Ok(()) => latencies.push(t.elapsed()),
            Err(e) => return r.fail(SUITE, &ds, "deleted file goes", e),
        }
    }
    r.record_samples(SUITE, &ds, "deleted file goes", walls(&latencies), info([]));

    // A folder renamed: everything inside it re-added under the new name.
    rename(r, &mut index, &changes, &root, &ds, &nodes);

    // Editing files: each edit marks the old entry dead and adds a new one. Searches are
    // timed before and after.
    probe_searches(r, &mut index, &ds, "before edits");
    let files: Vec<&str> = nodes
        .iter()
        .filter(|n| n.kind == EntryKind::File && data_is_indexed(&n.path))
        .map(|n| n.path.as_str())
        .collect();
    for pct in [10usize, 50, 100] {
        let chosen: Vec<PathBuf> = files.iter().step_by((100 / pct).max(1)).map(|p| root.join(p)).collect();
        let since = changes.count.load(Ordering::SeqCst);
        let t = Instant::now();
        let before = probe::usage();
        for path in &chosen {
            let mut f = fs::OpenOptions::new().append(true).open(path).expect("open file");
            std::io::Write::write_all(&mut f, b"!").expect("edit file");
        }
        let written = t.elapsed();
        let settled = match changes.settle(since, Duration::from_secs(300)) {
            Ok(at) => at,
            Err(e) => return r.fail(SUITE, &ds, &format!("edit {pct}% of files"), e),
        };
        let mut cost: Cost = probe::usage().since(&before);
        cost.wall_ns = settled.duration_since(t).as_nanos() as f64;
        let counts = index.handle().counts();
        let applied = cost.wall_ns / 1e9;
        r.record_samples(
            SUITE,
            &ds,
            &format!("edit {pct}% of files: until applied"),
            vec![cost],
            info([
                ("files", chosen.len() as f64),
                ("write_ms", written.as_secs_f64() * 1e3),
                ("files_per_sec", chosen.len() as f64 / applied.max(1e-9)),
                ("injected", counts.injected as f64),
                ("dead", counts.dead as f64),
                ("live", (counts.injected - counts.dead) as f64),
                ("dead_share", counts.dead as f64 / counts.injected.max(1) as f64),
                ("estimate_bytes", index.handle().memory_estimate() as f64),
                ("process_heap_bytes", alloc::heap().live as f64),
                ("process_footprint_bytes", probe::footprint() as f64),
            ]),
        );
        probe_searches(r, &mut index, &ds, &format!("after editing {pct}%"));
    }
}

/// Files the synthetic index config keeps (not in excluded or hidden folders).
fn data_is_indexed(path: &str) -> bool {
    !path.split('/').any(|p| p.starts_with('.') || ["node_modules", "target", "build", "DerivedData"].contains(&p))
}

fn rename(r: &mut Runner, index: &mut GlobalIndex, changes: &Changes, root: &Path, ds: &str, nodes: &[data::Node]) {
    // The project with the most indexed items.
    let mut best: Option<(&str, usize)> = None;
    for n in nodes
        .iter()
        .filter(|n| n.kind == EntryKind::Folder && n.path.matches('/').count() == 1 && n.path.starts_with("Developer/"))
    {
        let prefix = format!("{}/", n.path);
        let count = nodes.iter().filter(|m| m.path.starts_with(&prefix) && data_is_indexed(&m.path)).count();
        if best.is_none_or(|(_, c)| count > c) {
            best = Some((&n.path, count));
        }
    }
    let Some((folder, count)) = best else { return };
    let to = format!("Developer/zqrenamed{}", count);
    let t = Instant::now();
    if let Err(e) = fs::rename(root.join(folder), root.join(&to)) {
        return r.fail(SUITE, ds, "renamed folder shows up", e.to_string());
    }
    let query = format!("zqrenamed{count} readme");
    let result = wait_for(index, changes, &query, &format!("{to}/README.md"), true);
    let took = t.elapsed();
    let _ = fs::rename(root.join(&to), root.join(folder));
    match result {
        Ok(()) => r.record_samples(
            SUITE,
            ds,
            "renamed folder shows up",
            walls(&[took]),
            info([("items_inside", count as f64)]),
        ),
        Err(e) => r.fail(SUITE, ds, "renamed folder shows up", e),
    }
    let _ = changes.settle(changes.count.load(Ordering::SeqCst).saturating_sub(1), TIMEOUT);
}

/// Waits until searching `text` finds the item `key` (or stops finding it, with `present`
/// false): checks each time the index applies a batch. Matched by key, as fuzzy matching
/// also finds similar names.
fn wait_for(index: &mut GlobalIndex, changes: &Changes, text: &str, key: &str, present: bool) -> Result<(), String> {
    let q = Query::parse(text);
    let t = Instant::now();
    let mut seen = u64::MAX;
    loop {
        let count = changes.count.load(Ordering::SeqCst);
        if count != seen {
            seen = count;
            let found = index.search(&q, None, None, data::NOW, 50).iter().any(|h| h.key == key);
            if found == present {
                return Ok(());
            }
        }
        if t.elapsed() > TIMEOUT {
            return Err(format!("“{text}” didn't change within {TIMEOUT:?}"));
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn probe_searches(r: &mut Runner, index: &mut GlobalIndex, ds: &str, when: &str) {
    let reset = Query::parse("§reset§");
    let cell = RefCell::new(index);
    for text in PROBES {
        let q = Query::parse(text);
        r.bench_setup(
            SUITE,
            ds,
            &format!("search \"{text}\", {when}"),
            || {
                cell.borrow_mut().search(&reset, None, None, data::NOW, 200);
            },
            |_| cell.borrow_mut().search(&q, None, None, data::NOW, 200).len(),
        );
    }
}

fn walls(durations: &[Duration]) -> Vec<Cost> {
    durations.iter().map(|d| Cost { wall_ns: d.as_nanos() as f64, ..Default::default() }).collect()
}
