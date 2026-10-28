//! The saved index: saving, loading, memory held, searching and the other lookups (Activity,
//! Clean Up, the Index window), on synthetic indexes of each size and on the user's own.

use std::collections::HashMap;
use std::time::Duration;

use tilde_index::{GlobalIndex, IndexConfig, write_saved};
use tilde_query::Query;
use tilde_types::EntryKind;

use super::{Ctx, now, wait_ready};
use crate::runner::{Runner, info};
use crate::{alloc, data, probe};

const SUITE: &str = "index";
const LOAD_TIMEOUT: Duration = Duration::from_secs(600);

/// Queries for synthetic data: (label, query). The label names the case.
const QUERIES: &[(&str, &str)] = &[
    ("1 char", "r"),
    ("2 chars", "re"),
    ("word", "report"),
    ("word, common", "config"),
    ("word, rare", "sourdough"),
    ("two words", "invoice 2024"),
    ("path words", "developer src shader"),
    ("photo name", "IMG_12"),
    ("long", "developer starfield src engine render shader"),
    ("unicode", "café"),
    ("no match", "zzqxjv"),
    ("filter ext", "ext:pdf"),
    ("filter kind", "kind:image"),
    ("filter date", "mod:<7d"),
    ("filter size", "size:>100M"),
    ("two filters", "ext:rs mod:<30d"),
    ("word + filter", "report ext:pdf"),
    ("in: + word", "in:developer render"),
];

/// Words typed a key at a time.
const TYPED: &[&str] = &["invoice 2024", "render shader", "photos"];

/// A query that shares no prefix with the measured ones, so each measured search starts
/// matching from scratch (nucleo reuses a pass when the text only grows).
const RESET: &str = "§reset§";

pub fn run(r: &mut Runner, ctx: &Ctx) {
    for &n in &ctx.sizes {
        synthetic(r, ctx, n);
    }
    if ctx.real {
        real(r, ctx);
    }
}

fn synthetic(r: &mut Runner, ctx: &Ctx, n: usize) {
    let ds = format!("synthetic-{}", data::label(n));
    // The index file sits beside the (empty) home it describes, never inside it: the
    // index watches its home, and a change there would make it re-list the real folder.
    let dir = ctx.scratch.join(&ds);
    let home = dir.join("home");
    std::fs::create_dir_all(&home).expect("scratch folder");
    let nodes = data::home(n);
    let items = data::items(&nodes);
    drop(nodes);
    let config = data::index_config(&home, Some(dir.join("index.bin")));

    r.bench_slow(
        SUITE,
        &ds,
        "save index file",
        || (),
        |_| {
            let bytes = write_saved(&config, &items).map_err(|e| e.to_string())?;
            Ok(info([
                ("items", n as f64),
                ("file_bytes", bytes as f64),
                ("file_bytes_per_item", bytes as f64 / n as f64),
            ]))
        },
    );
    if !dir.join("index.bin").exists() {
        write_saved(&config, &items).expect("save index");
    }
    drop(items);

    r.bench_slow(
        SUITE,
        &ds,
        "load saved index",
        || (),
        |_| {
            let index = GlobalIndex::start(config.clone(), || {});
            wait_ready(&index, LOAD_TIMEOUT)?;
            Ok(info([("items", index.status().items as f64)]))
        },
    );

    let Some(mut index) = loaded(r, &ds, config.clone()) else { return };
    searches(r, &mut index, &ds, QUERIES.iter().map(|(l, q)| (l.to_string(), q.to_string())).collect(), data::NOW);
    lookups(r, &index, &ds, data::NOW);
    memory(r, &index, &ds, "memory after searches");
}

fn real(r: &mut Runner, ctx: &Ctx) {
    let ds = "real";
    let Some(config) = data::real_index(&ctx.scratch) else {
        r.fail(SUITE, ds, "load saved index", "no saved index (or indexing is off) for this user".into());
        return;
    };
    let index = GlobalIndex::start(config.clone(), || {});
    if index.status().scanning {
        // The saved index doesn't match the settings: it would scan the whole home.
        drop(index);
        r.fail(SUITE, ds, "load saved index", "the saved index was built with other settings; open Tilde once".into());
        return;
    }
    drop(index);
    r.bench_slow(
        SUITE,
        ds,
        "load saved index",
        || (),
        |_| {
            let index = GlobalIndex::start(config.clone(), || {});
            wait_ready(&index, LOAD_TIMEOUT)?;
            Ok(info([("items", index.status().items as f64)]))
        },
    );
    let Some(mut index) = loaded(r, ds, config) else { return };
    let mut queries: Vec<(String, String)> = [
        ("1 char", "r"),
        ("2 chars", "re"),
        ("no match", "zzqxjv"),
        ("filter ext", "ext:pdf"),
        ("filter kind", "kind:image"),
        ("filter date", "mod:<7d"),
        ("filter size", "size:>100M"),
    ]
    .iter()
    .map(|(l, q)| (l.to_string(), q.to_string()))
    .collect();
    // Words from the user's own file names, by how common they are. Labelled by rank
    // only, so reports don't carry the user's file names.
    for (rank, word) in name_words(&index) {
        queries.push((format!("name word #{rank}"), word.clone()));
        queries.push((format!("name word #{rank} + ext:pdf"), format!("{word} ext:pdf")));
    }
    searches(r, &mut index, ds, queries, now());
    lookups(r, &index, ds, now());
    memory(r, &index, ds, "memory after searches");
}

/// Loads `config` and records the memory it holds.
fn loaded(r: &mut Runner, ds: &str, config: IndexConfig) -> Option<GlobalIndex> {
    alloc::settle(Duration::from_secs(5));
    let (heap, footprint) = (alloc::heap().live, probe::footprint());
    let index = GlobalIndex::start(config, || {});
    if let Err(e) = wait_ready(&index, LOAD_TIMEOUT) {
        r.fail(SUITE, ds, "memory after load", e);
        return None;
    }
    alloc::settle(Duration::from_secs(5));
    let held = (alloc::heap().live - heap) as f64;
    let grew = probe::footprint() as f64 - footprint as f64;
    let items = index.status().items.max(1) as f64;
    let estimate = index.handle().memory_estimate() as f64;
    r.record(
        SUITE,
        ds,
        "memory after load",
        info([
            ("items", items),
            ("heap_bytes", held),
            ("heap_bytes_per_item", held / items),
            ("footprint_bytes", grew),
            ("footprint_bytes_per_item", grew / items),
            ("estimate_bytes", estimate),
            ("estimate_bytes_per_item", estimate / items),
        ]),
    );
    Some(index)
}

fn memory(r: &mut Runner, index: &GlobalIndex, ds: &str, name: &str) {
    alloc::settle(Duration::from_secs(5));
    let counts = index.handle().counts();
    r.record(
        SUITE,
        ds,
        name,
        info([
            ("injected", counts.injected as f64),
            ("dead", counts.dead as f64),
            ("folder_tables", counts.folder_tables as f64),
            ("process_heap_bytes", alloc::heap().live as f64),
            ("process_footprint_bytes", probe::footprint() as f64),
        ]),
    );
}

fn searches(r: &mut Runner, index: &mut GlobalIndex, ds: &str, queries: Vec<(String, String)>, now: i64) {
    let reset = Query::parse(RESET);
    for (label, text) in &queries {
        let q = Query::parse(text);
        let index_cell = std::cell::RefCell::new(&mut *index);
        r.bench_setup(
            SUITE,
            ds,
            &format!("search: {label}"),
            || {
                index_cell.borrow_mut().search(&reset, None, None, now, 200);
            },
            |_| index_cell.borrow_mut().search(&q, None, None, now, 200).len(),
        );
    }
    // The same text again: what each refresh costs while the index changes under a search.
    let q = Query::parse("report");
    index.search(&q, None, None, now, 200);
    r.bench(SUITE, ds, "search again, same text", || index.search(&q, None, None, now, 200).len());
    // Excluding the open folder's items, as the app does.
    let q = Query::parse("report");
    let cell = std::cell::RefCell::new(&mut *index);
    r.bench_setup(
        SUITE,
        ds,
        "search: word, from a folder",
        || {
            cell.borrow_mut().search(&reset, None, None, now, 200);
        },
        |_| cell.borrow_mut().search(&q, Some("Developer"), Some("Developer"), now, 200).len(),
    );
    for word in TYPED {
        let prefixes: Vec<Query> =
            (1..=word.chars().count()).map(|k| Query::parse(&word.chars().take(k).collect::<String>())).collect();
        let cell = std::cell::RefCell::new(&mut *index);
        r.bench_setup(
            SUITE,
            ds,
            &format!("typing \"{word}\", all keys"),
            || {
                cell.borrow_mut().search(&reset, None, None, now, 200);
            },
            |_| {
                let mut hits = 0;
                for q in &prefixes {
                    hits = cell.borrow_mut().search(q, None, None, now, 200).len();
                }
                hits
            },
        );
    }
}

/// The lookups other features make: Activity's recent files, Clean Up's selections, the
/// Index window's numbers.
fn lookups(r: &mut Runner, index: &GlobalIndex, ds: &str, now: i64) {
    let handle = index.handle();
    r.bench(SUITE, ds, "Activity: recent files, 30 days", || handle.recent(now - 30 * 86_400, 400).len());
    r.bench(SUITE, ds, "Clean Up: files over 100 MB", || {
        handle.select(|_, m| m.kind == EntryKind::File && m.size > 100_000_000).len()
    });
    r.bench(SUITE, ds, "text: without Spotlight: text files, 2 years", || {
        handle
            .recent_where(now - 2 * 365 * 86_400, 4_000, |key, m| {
                m.size <= 1 << 20
                    && matches!(
                        tilde_query::kind_for(m.kind, key.rsplit('/').next().unwrap_or(key)),
                        tilde_query::Kind::Code | tilde_query::Kind::Text
                    )
            })
            .len()
    });
    r.bench(SUITE, ds, "Index window: largest folders", || handle.largest_folders(20).len());
    r.bench(SUITE, ds, "Index window: memory estimate", || handle.memory_estimate() as usize);
    r.bench(SUITE, ds, "status", || handle.status().items as usize);
}

/// Words from file names (4+ letters), by frequency: ranks 1, 10, 100 and 1000.
fn name_words(index: &GlobalIndex) -> Vec<(usize, String)> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (path, _) in index.handle().select(|_, _| true) {
        let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        for w in name.split(|c: char| !c.is_alphabetic()).filter(|w| w.chars().count() >= 4) {
            *counts.entry(w.to_string()).or_default() += 1;
        }
    }
    let mut ranked: Vec<(String, usize)> = counts.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    [1usize, 10, 100, 1000].iter().filter_map(|&rank| ranked.get(rank - 1).map(|(w, _)| (rank, w.clone()))).collect()
}
