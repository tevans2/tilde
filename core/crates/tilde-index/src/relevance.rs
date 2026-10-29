//! Ranking checks: a small home with the competition real ones have, and searches with
//! what should come first. Used by the tests and by `tilde-bench` (the `relevance`
//! suite), so ranking changes can't quietly make results worse.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tilde_query::Query;
use tilde_types::EntryKind;

use crate::{Frecency, GlobalIndex, IndexConfig, ItemMeta, ScanConfig, Use, write_saved};

/// "Now" for the fixture's dates and searches.
pub const NOW: i64 = 1_800_000_000;
const DAY: i64 = 86_400;

/// What a search should give.
#[derive(Debug, Clone, Copy)]
pub enum Expect {
    /// This key first.
    First(&'static str),
    /// The first result's file name is this (any folder).
    FirstName(&'static str),
    /// The first key ranks above the second (both found).
    Before(&'static str, &'static str),
}

#[derive(Debug, Clone, Copy)]
pub struct Case {
    pub query: &'static str,
    pub expect: Expect,
    /// What it checks, for reports.
    pub why: &'static str,
}

/// How a case went.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub case: Case,
    pub passed: bool,
    /// The first few keys found.
    pub top: Vec<String>,
}

pub fn cases() -> Vec<Case> {
    use Expect::*;
    let case = |query, expect, why| Case { query, expect, why };
    vec![
        case("cargo.toml", FirstName("Cargo.toml"), "an exact name beats fuzzy path matches"),
        case("readme", FirstName("README.md"), "a name equal to the query (without its extension) comes first"),
        case("Readme", FirstName("README.md"), "case doesn't matter"),
        case("main.rs", FirstName("main.rs"), "an exact name beats a path spelling it out"),
        case("rep", Before("Documents/report.pdf", "Documents/prep-list.txt"), "a name starting with the query first"),
        case(
            "project",
            Before("Documents/project-story.md", "Documents/projection-data.csv"),
            "a whole word beats part of one",
        ),
        case(
            "index",
            Before("Developer/app/src/searchIndex.ts", "Developer/app/src/windexer.ts"),
            "a word in camelCase",
        ),
        case("todo", Before("todo.txt", "Documents/Archive/2018/old/todo.txt"), "shallower first"),
        case("budget", Before("Documents/A/budget.xlsx", "Documents/B/budget.xlsx"), "changed recently first"),
        case("plan q", First("Documents/Projects/plan-q3.pdf"), "the one you open most"),
        case("notes", Before("Documents/notes.txt", "Documents/meeting-notes.md"), "a name match beats some use"),
        case("finance", First("Documents/Finance"), "a folder named exactly that"),
        case("resume", FirstName("Résumé.pages"), "accents don't matter"),
        case("tilde swift", FirstName("main.swift"), "folder words, then the name"),
        case(
            "invoice",
            Before("Developer/shop/web/src/invoice.js", "Developer/shop/dist/invoice.js"),
            "your own files before generated ones",
        ),
        case(
            "utils",
            Before("Developer/app/src/lib/utils.ts", "Developer/app/vendor/utils.ts"),
            "your own files before vendored ones",
        ),
    ]
}

/// The fixture home: (key, metadata) items and frecency uses (key, opens).
pub type Fixture = (Vec<(String, ItemMeta)>, Vec<(&'static str, usize)>);

pub fn fixture() -> Fixture {
    let mut items: Vec<(String, ItemMeta)> = Vec::new();
    let mut add = |key: &str, kind: EntryKind, modified: i64| {
        let meta = ItemMeta { id: 0, kind, size: 1_000, modified, child_count: None, extra: false };
        items.push((key.to_string(), meta));
    };
    let old = NOW - 400 * DAY;
    let file = EntryKind::File;
    let folder = EntryKind::Folder;
    // Filler: words that share letters with the queries, but not the queries themselves.
    let words = ["alpha", "bravo", "delta", "echo", "golf", "hotel", "kilo", "lima", "mike", "oscar", "papa", "quebec"];
    let exts = ["rs", "md", "pdf", "txt", "swift", "ts", "js", "png"];
    for p in 0..12 {
        let project = format!("Developer/{}-{}", words[p], words[(p * 5 + 3) % words.len()]);
        add(&project, folder, old);
        for d in ["src", "docs", "tests", "scripts"] {
            add(&format!("{project}/{d}"), folder, old);
            for f in 0..40 {
                let (a, b) = (words[(f * 7 + p) % words.len()], words[(f * 3 + 1) % words.len()]);
                add(&format!("{project}/{d}/{a}_{b}{f}.{}", exts[(f + p) % exts.len()]), file, old - f as i64);
            }
        }
    }
    for y in 2015..2027 {
        for f in 0..60 {
            let (a, b) = (words[(f + y as usize) % words.len()], words[f % words.len()]);
            add(&format!("Documents/{y}/{a} {b} {f}.{}", exts[f % exts.len()]), file, old);
        }
    }
    // The cases.
    for (key, kind, modified) in [
        ("Developer/tilde", folder, old),
        ("Developer/tilde/core", folder, old),
        ("Developer/tilde/core/Cargo.toml", file, old),
        ("Developer/cargo-tools/old/toml-notes.md", file, old),
        ("Developer/tilde/core/src/main.rs", file, old),
        ("Developer/domain/main/rs-notes.txt", file, old),
        ("Developer/tilde/Sources/main.swift", file, old),
        ("Developer/tilde/Sources/Panel.swift", file, old),
        ("Developer/tilde/README.md", file, old),
        ("Developer/readme-generator/src/main.rs", file, old),
        ("Documents/readmission-letter.pdf", file, old),
        ("Documents/report.pdf", file, old),
        ("Documents/prep-list.txt", file, old),
        ("Documents/project-story.md", file, old),
        ("Documents/projection-data.csv", file, old),
        ("Developer/app/src/searchIndex.ts", file, old),
        ("Developer/app/src/windexer.ts", file, old),
        // Generated copies first (lower IDs win ties), and shallower.
        ("Developer/app/vendor/utils.ts", file, old),
        ("Developer/app/src/lib/utils.ts", file, old),
        ("todo.txt", file, old),
        ("Documents/Archive/2018/old/todo.txt", file, old),
        ("Documents/A/budget.xlsx", file, NOW - DAY),
        ("Documents/B/budget.xlsx", file, old),
        ("Documents/Projects/plan-q1.pdf", file, old),
        ("Documents/Projects/plan-q2.pdf", file, old),
        ("Documents/Projects/plan-q3.pdf", file, old),
        ("Documents/notes.txt", file, old),
        ("Documents/meeting-notes.md", file, old),
        ("Documents/Finance", folder, old),
        ("Documents/Finance/finance-summary.pdf", file, old),
        ("Documents/Finance/2026/finances.xlsx", file, old),
        ("Documents/Résumé.pages", EntryKind::Package, old),
        ("Documents/resume-old/cover.txt", file, old),
        ("Developer/shop/dist/invoice.js", file, old),
        ("Developer/shop/web/src/invoice.js", file, old),
    ] {
        add(key, kind, modified);
    }
    let uses = vec![("Documents/Projects/plan-q3.pdf", 30), ("Documents/meeting-notes.md", 3)];
    (items, uses)
}

/// Starts an index of the fixture under `dir` (a fresh folder) and waits for it.
pub fn start(dir: &Path) -> GlobalIndex {
    let home = dir.join("home");
    std::fs::create_dir_all(&home).expect("fixture home");
    let (items, uses) = fixture();
    // Uses decay with the real clock, so they're recorded at real times.
    let mut frecency = Frecency::in_memory();
    for (key, opens) in uses {
        for i in 0..opens {
            frecency.record(&home.join(key), Use::Open, (crate::now() - i as i64 * 3_600) as f64, false);
        }
    }
    let config = IndexConfig {
        home: home.clone(),
        scan: ScanConfig { roots: vec![home], ..Default::default() },
        frecency: Some(Arc::new(Mutex::new(frecency))),
        zoxide: false,
        max_items: 1_000_000,
        cache_path: Some(dir.join("index.bin")),
    };
    write_saved(&config, &items).expect("save fixture");
    let index = GlobalIndex::start(config, || {});
    let t = Instant::now();
    while index.status().scanning && t.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(2));
    }
    index
}

/// Runs every case.
pub fn check(index: &mut GlobalIndex) -> Vec<Outcome> {
    cases()
        .into_iter()
        .map(|case| {
            let hits = index.search(&Query::parse(case.query), None, None, NOW, 50);
            let keys: Vec<&str> = hits.iter().map(|h| h.key.as_str()).collect();
            let rank = |k: &str| keys.iter().position(|x| *x == k);
            let passed = match case.expect {
                Expect::First(k) => rank(k) == Some(0),
                Expect::FirstName(name) => keys.first().is_some_and(|k| k.rsplit('/').next() == Some(name)),
                Expect::Before(a, b) => matches!((rank(a), rank(b)), (Some(x), Some(y)) if x < y),
            };
            Outcome { case, passed, top: keys.iter().take(5).map(|k| k.to_string()).collect() }
        })
        .collect()
}
