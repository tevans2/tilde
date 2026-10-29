//! End to end through the engine the app uses: from a command (open a folder, type a key,
//! sort) to `results_ready` for it, then fetching rows. Runs on a synthetic home on disk
//! with an index built from it. Folder sizes are off, so their background work doesn't
//! blur the numbers (the `sizes` suite measures that).

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use tilde_ffi::{Engine, EngineListener, IndexService, IndexState, ResultsInfo, SortBy};
use tilde_types::EntryKind;

use super::Ctx;
use crate::data;
use crate::runner::{Runner, info};

const SUITE: &str = "engine";
const TIMEOUT: Duration = Duration::from_secs(30);

/// The first `results_ready` for each generation.
#[derive(Default)]
struct Listener {
    seen: Mutex<HashMap<u64, (Instant, u32, u32)>>,
    cv: Condvar,
}

impl EngineListener for Listener {
    fn results_ready(&self, info: ResultsInfo) {
        self.seen.lock().unwrap().entry(info.generation).or_insert((
            Instant::now(),
            info.here_count,
            info.everywhere_count,
        ));
        self.cv.notify_all();
    }

    fn folder_failed(&self, generation: u64, _folder: String, _message: String) {
        self.seen.lock().unwrap().entry(generation).or_insert((Instant::now(), 0, 0));
        self.cv.notify_all();
    }
}

impl Listener {
    /// Waits for results for `generation`: (here, everywhere) counts.
    fn wait(&self, generation: u64) -> (u32, u32) {
        let mut seen = self.seen.lock().unwrap();
        let t = Instant::now();
        loop {
            if let Some(&(_, here, everywhere)) = seen.get(&generation) {
                return (here, everywhere);
            }
            assert!(t.elapsed() < TIMEOUT, "no results for generation {generation}");
            seen = self.cv.wait_timeout(seen, Duration::from_millis(50)).unwrap().0;
        }
    }
}

pub fn run(r: &mut Runner, ctx: &Ctx) {
    let n = ctx.trees.iter().copied().min().unwrap_or(20_000);
    let ds = format!("tree-{}", data::label(n));
    let root = data::home_tree(n).canonicalize().expect("tree");
    let index = IndexService::with_config(data::index_config(&root, None), false);
    let t = Instant::now();
    while index.status().state != IndexState::Ready {
        if t.elapsed() > Duration::from_secs(600) {
            return r.fail(SUITE, &ds, "index", "the index wasn't ready in 10 minutes".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let listener = Arc::new(Listener::default());
    let sort = tilde_config::SortConfig { folder_sizes: false, same_everywhere: false, ..Default::default() };
    let engine = Engine::with_settings(listener.clone(), root.clone(), Some(index), sort);
    let mut generation = 0u64;
    let mut next = || {
        generation += 1;
        generation
    };

    // Folders to open: home, a typical one, the biggest.
    let nodes = data::home(n);
    let mut children: HashMap<&str, usize> = HashMap::new();
    for node in &nodes {
        let parent = node.path.rsplit_once('/').map_or("", |(p, _)| p);
        *children.entry(parent).or_default() += 1;
    }
    let biggest =
        children.iter().filter(|(p, _)| !p.is_empty()).max_by_key(|(_, c)| **c).map(|(p, c)| (p.to_string(), *c));
    let typical =
        nodes.iter().find(|n| n.kind == EntryKind::Folder && n.path.ends_with("/src")).map(|n| n.path.clone());
    let mut folders = vec![("home".to_string(), root.clone())];
    if let Some(p) = typical {
        folders.push((format!("a project's src ({} items)", children.get(p.as_str()).unwrap_or(&0)), root.join(&p)));
    }
    if let Some((p, c)) = biggest {
        folders.push((format!("the biggest folder ({c} items)"), root.join(p)));
    }
    // A huge Downloads-like folder.
    folders.push(("a folder of 50k items".to_string(), data::flat_folder(50_000)));
    let gen_cell = std::cell::RefCell::new(&mut next);
    for (label, path) in &folders {
        let path = path.to_string_lossy().into_owned();
        r.bench_setup(
            SUITE,
            &ds,
            &format!("open {label}"),
            || {
                // Somewhere else first, so the open is a real change of folder.
                let g = (gen_cell.borrow_mut())();
                engine.open_folder(root.join("Documents").to_string_lossy().into_owned(), g);
                listener.wait(g);
            },
            |_| {
                let g = (gen_cell.borrow_mut())();
                engine.open_folder(path.clone(), g);
                listener.wait(g).0 as usize
            },
        );
    }

    // Typing in home: Here and Everywhere results for each key.
    let g = (gen_cell.borrow_mut())();
    engine.open_folder(root.to_string_lossy().into_owned(), g);
    listener.wait(g);
    for word in ["report", "invoice 2024", "ext:pdf"] {
        let prefixes: Vec<String> = (1..=word.chars().count()).map(|k| word.chars().take(k).collect()).collect();
        r.bench_setup(
            SUITE,
            &ds,
            &format!("type \"{word}\", all keys"),
            || {
                // An unrelated query first, so the index matches afresh (an empty one
                // doesn't reach the index).
                let g = (gen_cell.borrow_mut())();
                engine.set_query("§reset§".into(), g);
                listener.wait(g);
            },
            |_| {
                let mut total = 0;
                for p in &prefixes {
                    let g = (gen_cell.borrow_mut())();
                    engine.set_query(p.clone(), g);
                    let (here, everywhere) = listener.wait(g);
                    total = (here + everywhere) as usize;
                }
                total
            },
        );
        r.bench_setup(
            SUITE,
            &ds,
            &format!("query \"{word}\" at once"),
            || {
                // An unrelated query first, so the index matches afresh (an empty one
                // doesn't reach the index).
                let g = (gen_cell.borrow_mut())();
                engine.set_query("§reset§".into(), g);
                listener.wait(g);
            },
            |_| {
                let g = (gen_cell.borrow_mut())();
                engine.set_query(word.to_string(), g);
                let (here, everywhere) = listener.wait(g);
                (here + everywhere) as usize
            },
        );
    }

    // Rows for the screen (what the table asks for as it scrolls).
    let g = (gen_cell.borrow_mut())();
    engine.set_query("report".into(), g);
    listener.wait(g);
    r.bench(SUITE, &ds, "fetch 60 rows", || engine.rows(g, 0, 60).len());

    // Sorting the biggest folder.
    if let Some((_, path)) = folders.iter().rev().nth(1) {
        let g = (gen_cell.borrow_mut())();
        engine.set_query(String::new(), g);
        engine.open_folder(path.to_string_lossy().into_owned(), g);
        listener.wait(g);
        let flip = std::cell::Cell::new(false);
        r.bench(SUITE, &ds, "sort the biggest folder", || {
            flip.set(!flip.get());
            let g = (gen_cell.borrow_mut())();
            engine.set_sort(if flip.get() { SortBy::Size } else { SortBy::Name }, false, g);
            listener.wait(g).0 as usize
        });
    }
    r.record(SUITE, &ds, "setup", info([("items", nodes.len() as f64)]));
}
