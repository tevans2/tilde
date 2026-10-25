use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tilde_fs::{ListOptions, SINCE_NOW, Watcher, list_dir, sort_entries, watch};
use tilde_index::GlobalHit;
use tilde_query::{HereMatcher, Hit, Query};
use tilde_types::Entry;

use crate::index::{IndexService, IndexState, home_dir};
use crate::views::RowView;

/// Most Everywhere results returned for one query.
const EVERYWHERE_LIMIT: usize = 200;
/// How often results refresh while the index is still being built.
const SCAN_REFRESH: Duration = Duration::from_millis(400);
/// FSEvents coalescing window for the open folder.
const FOLDER_WATCH_LATENCY: Duration = Duration::from_millis(100);

/// Implemented by the Swift app. Called on a background thread.
#[uniffi::export(with_foreign)]
pub trait EngineListener: Send + Sync {
    /// Results for `generation` are ready to fetch with `Engine::rows`: `here_count` rows
    /// from the current folder followed by `everywhere_count` from the index. May be
    /// called more than once for a generation while the index is still being built.
    fn results_ready(&self, generation: u64, folder: String, here_count: u32, everywhere_count: u32);
    /// The folder requested in `generation` could not be opened. The previous folder
    /// stays current.
    fn folder_failed(&self, generation: u64, folder: String, message: String);
}

pub(crate) enum Command {
    Open {
        path: PathBuf,
        generation: u64,
    },
    Query {
        text: String,
        generation: u64,
    },
    ShowHidden {
        show: bool,
        generation: u64,
    },
    /// FSEvents reported a change directly inside this folder.
    FolderChanged(PathBuf),
    /// The index applied a batch of changes.
    IndexChanged,
}

/// What `rows` reads from. Replaced wholesale by the worker after each update.
struct Published {
    generation: u64,
    folder: PathBuf,
    home: PathBuf,
    entries: Arc<Vec<Entry>>,
    here: Vec<Hit>,
    everywhere: Vec<GlobalHit>,
}

#[derive(uniffi::Object)]
pub struct Engine {
    tx: Sender<Command>,
    published: Arc<Mutex<Published>>,
    /// Generation of the newest `open_folder` call, so an in-flight listing can stop early.
    latest_open: Arc<AtomicU64>,
}

#[uniffi::export]
impl Engine {
    /// Starts an engine (one per window) that searches the shared `index`.
    #[uniffi::constructor]
    pub fn new(listener: Arc<dyn EngineListener>, index: Arc<IndexService>) -> Arc<Self> {
        Engine::start(listener, index.home().to_path_buf(), Some(index))
    }

    /// Lists `path` and makes it the current folder. The current query is kept; the UI
    /// normally clears it with `set_query` at the same time.
    pub fn open_folder(&self, path: String, generation: u64) {
        self.latest_open.fetch_max(generation, Ordering::SeqCst);
        let _ = self.tx.send(Command::Open { path: PathBuf::from(path), generation });
    }

    pub fn set_query(&self, text: String, generation: u64) {
        let _ = self.tx.send(Command::Query { text, generation });
    }

    pub fn set_show_hidden(&self, show: bool, generation: u64) {
        let _ = self.tx.send(Command::ShowHidden { show, generation });
    }

    /// Rows `start..start+count` of the results for `generation`: Here rows first, then
    /// Everywhere rows. Empty if a newer generation has replaced them.
    pub fn rows(&self, generation: u64, start: u32, count: u32) -> Vec<RowView> {
        let p = self.published.lock().unwrap();
        if p.generation != generation {
            return Vec::new();
        }
        let total = p.here.len() + p.everywhere.len();
        let (start, end) = (start as usize, (start as usize + count as usize).min(total));
        (start..end)
            .map(|i| match p.here.get(i) {
                Some(hit) => RowView::here(&p.folder, &p.entries[hit.index as usize], hit),
                None => RowView::everywhere(&p.home, &p.everywhere[i - p.here.len()]),
            })
            .collect()
    }

    /// Position of the Here row called `name` in the results for `generation`.
    pub fn index_of(&self, generation: u64, name: String) -> Option<u32> {
        let p = self.published.lock().unwrap();
        if p.generation != generation {
            return None;
        }
        p.here.iter().position(|h| p.entries[h.index as usize].name == name).map(|i| i as u32)
    }

    pub fn home_dir(&self) -> String {
        home_dir().to_string_lossy().into_owned()
    }
}

impl Engine {
    /// `index: None` disables the Everywhere scope (used by tests).
    fn start(listener: Arc<dyn EngineListener>, home: PathBuf, index: Option<Arc<IndexService>>) -> Arc<Self> {
        let (tx, rx) = mpsc::channel();
        let published = Arc::new(Mutex::new(Published {
            generation: 0,
            folder: PathBuf::new(),
            home: home.clone(),
            entries: Arc::new(Vec::new()),
            here: Vec::new(),
            everywhere: Vec::new(),
        }));
        let latest_open = Arc::new(AtomicU64::new(0));
        if let Some(index) = &index {
            index.subscribe(tx.clone());
        }
        let worker = Worker {
            rx,
            tx: tx.clone(),
            folder_watcher: None,
            listener,
            published: published.clone(),
            latest_open: latest_open.clone(),
            home,
            index,
            generation: 0,
            folder: PathBuf::new(),
            entries: Arc::new(Vec::new()),
            query: Query::default(),
            show_hidden: false,
            matcher: HereMatcher::default(),
        };
        thread::Builder::new().name("tilde-engine".into()).spawn(move || worker.run()).expect("spawn engine worker");
        Arc::new(Engine { tx, published, latest_open })
    }
}

struct Worker {
    rx: Receiver<Command>,
    /// For the open folder's watcher to report back.
    tx: Sender<Command>,
    folder_watcher: Option<Watcher>,
    listener: Arc<dyn EngineListener>,
    published: Arc<Mutex<Published>>,
    latest_open: Arc<AtomicU64>,
    home: PathBuf,
    index: Option<Arc<IndexService>>,
    generation: u64,
    folder: PathBuf,
    entries: Arc<Vec<Entry>>,
    query: Query,
    show_hidden: bool,
    matcher: HereMatcher,
}

impl Worker {
    fn run(mut self) {
        loop {
            // While the index is being built, wake periodically to report progress and
            // refresh results so Everywhere fills in as the scan goes.
            let scanning = self.index_scanning();
            let first = if scanning {
                match self.rx.recv_timeout(SCAN_REFRESH) {
                    Ok(cmd) => Some(cmd),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            } else {
                match self.rx.recv() {
                    Ok(cmd) => Some(cmd),
                    Err(_) => return,
                }
            };
            let Some(first) = first else {
                let still_scanning = self.index_scanning();
                if !self.query.is_empty() || !still_scanning {
                    self.publish();
                }
                continue;
            };

            // Coalesce everything queued so fast typing only ranks the latest text.
            let mut open = None;
            let mut relist = false;
            let mut user_change = false;
            let mut index_changed = false;
            for cmd in std::iter::once(first).chain(self.rx.try_iter()) {
                user_change |= matches!(cmd, Command::Open { .. } | Command::Query { .. } | Command::ShowHidden { .. });
                match cmd {
                    Command::Open { path, generation } => {
                        open = Some((path, generation));
                        self.generation = self.generation.max(generation);
                    }
                    Command::Query { text, generation } => {
                        self.query = Query::parse(&text);
                        self.generation = self.generation.max(generation);
                    }
                    Command::ShowHidden { show, generation } => {
                        relist |= show != self.show_hidden;
                        self.show_hidden = show;
                        self.generation = self.generation.max(generation);
                    }
                    Command::FolderChanged(path) => relist |= path == self.folder,
                    Command::IndexChanged => index_changed = true,
                }
            }
            // Index updates only matter to a search in progress.
            if !user_change && !relist && !(index_changed && !self.query.is_empty()) {
                continue;
            }

            let explicit_open = open.is_some();
            let target = match open {
                Some(target) => Some(target),
                None if relist => Some((self.folder.clone(), self.generation)),
                None => None,
            };
            if let Some((path, open_generation)) = target {
                let opts = ListOptions { include_hidden: self.show_hidden, ..Default::default() };
                let latest = self.latest_open.clone();
                match list_dir(&path, &opts, || latest.load(Ordering::SeqCst) > open_generation) {
                    Ok(mut entries) => {
                        sort_entries(&mut entries);
                        // Only folders the user navigated to: not the one opened at launch.
                        if explicit_open
                            && !self.folder.as_os_str().is_empty()
                            && path != self.folder
                            && let Some(index) = &self.index
                        {
                            index.record_visit(&path);
                        }
                        if path != self.folder {
                            self.watch_folder(&path);
                        }
                        self.folder = path;
                        self.entries = Arc::new(entries);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) if !explicit_open && e.kind() == std::io::ErrorKind::NotFound => {
                        // The open folder was deleted: move to the nearest folder that exists.
                        let mut up = path.clone();
                        while !up.is_dir() && up.pop() {}
                        if let Ok(mut entries) = list_dir(&up, &opts, || false) {
                            sort_entries(&mut entries);
                            self.watch_folder(&up);
                            self.folder = up;
                            self.entries = Arc::new(entries);
                        }
                    }
                    Err(e) => {
                        self.listener.folder_failed(open_generation, path.to_string_lossy().into(), e.to_string());
                    }
                }
            }
            self.publish();
        }
    }

    /// Replaces the open folder's watcher with one for `folder`.
    fn watch_folder(&mut self, folder: &std::path::Path) {
        self.folder_watcher = None;
        let tx = self.tx.clone();
        let target = folder.to_path_buf();
        let roots = [target.clone()];
        self.folder_watcher = watch(&roots, &[], SINCE_NOW, FOLDER_WATCH_LATENCY, move |batch| {
            // The stream is recursive; only changes directly inside the folder matter.
            if batch.iter().any(|e| e.path == target || (e.needs_rescan() && target.starts_with(&e.path))) {
                let _ = tx.send(Command::FolderChanged(target.clone()));
            }
        })
        .ok();
    }

    fn index_scanning(&self) -> bool {
        self.index.as_ref().is_some_and(|i| i.status().state == IndexState::Scanning)
    }

    fn publish(&mut self) {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let here = self.matcher.rank(&self.entries, &self.query, now);
        let everywhere = match &self.index {
            Some(index) if !self.query.is_empty() => index.search(&self.query, &self.folder, now, EVERYWHERE_LIMIT),
            _ => Vec::new(),
        };
        let (here_count, everywhere_count) = (here.len() as u32, everywhere.len() as u32);
        *self.published.lock().unwrap() = Published {
            generation: self.generation,
            folder: self.folder.clone(),
            home: self.home.clone(),
            entries: self.entries.clone(),
            here,
            everywhere,
        };
        self.listener.results_ready(
            self.generation,
            self.folder.to_string_lossy().into(),
            here_count,
            everywhere_count,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Condvar;
    use std::time::Instant;

    /// (generation, folder, here_count, everywhere_count)
    type Event = (u64, String, u32, u32);

    #[derive(Default)]
    struct Recorder {
        events: Mutex<Vec<Event>>,
        failures: Mutex<Vec<(u64, String)>>,
        cv: Condvar,
    }

    impl EngineListener for Recorder {
        fn results_ready(&self, generation: u64, folder: String, here_count: u32, everywhere_count: u32) {
            self.events.lock().unwrap().push((generation, folder, here_count, everywhere_count));
            self.cv.notify_all();
        }
        fn folder_failed(&self, generation: u64, _folder: String, message: String) {
            self.failures.lock().unwrap().push((generation, message));
            self.cv.notify_all();
        }
    }

    impl Recorder {
        /// The latest event for at least `generation` that satisfies `pred`.
        fn wait_until(&self, generation: u64, pred: impl Fn(&Event) -> bool) -> Event {
            let mut events = self.events.lock().unwrap();
            loop {
                if let Some(e) = events.iter().rev().find(|e| e.0 >= generation && pred(e)) {
                    return e.clone();
                }
                let (guard, timeout) = self.cv.wait_timeout(events, Duration::from_secs(5)).unwrap();
                assert!(!timeout.timed_out(), "no matching results for generation {generation}");
                events = guard;
            }
        }

        fn wait_for(&self, generation: u64) -> Event {
            self.wait_until(generation, |_| true)
        }
    }

    fn engine_without_index(rec: Arc<Recorder>) -> Arc<Engine> {
        Engine::start(rec, home_dir(), None)
    }

    #[test]
    fn open_query_and_fetch_rows() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("Sources")).unwrap();
        std::fs::write(dir.path().join("README.md"), b"# hi").unwrap();
        std::fs::write(dir.path().join("Package.swift"), b"").unwrap();

        let rec = Arc::new(Recorder::default());
        let engine = engine_without_index(rec.clone());
        let path = dir.path().to_string_lossy().into_owned();

        engine.open_folder(path.clone(), 1);
        let (_, folder, here, everywhere) = rec.wait_for(1);
        assert_eq!((folder, here, everywhere), (path, 3, 0));
        let rows = engine.rows(1, 0, 10);
        assert_eq!(rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["Sources", "Package.swift", "README.md"]);
        assert_eq!(rows[0].kind, crate::RowKind::Folder);

        engine.set_query("rdm".into(), 2);
        let (_, _, here, _) = rec.wait_for(2);
        assert_eq!(here, 1);
        let rows = engine.rows(2, 0, 10);
        assert_eq!(rows[0].name, "README.md");
        assert!(!rows[0].highlights.is_empty());

        assert_eq!(engine.index_of(2, "README.md".into()), Some(0));
        assert_eq!(engine.index_of(2, "Sources".into()), None);

        // Stale generations return nothing.
        assert!(engine.rows(1, 0, 10).is_empty());
        assert_eq!(engine.index_of(1, "Sources".into()), None);
    }

    #[test]
    fn failed_open_keeps_previous_folder() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), b"").unwrap();
        let rec = Arc::new(Recorder::default());
        let engine = engine_without_index(rec.clone());
        engine.open_folder(dir.path().to_string_lossy().into(), 1);
        rec.wait_for(1);
        engine.open_folder(dir.path().join("missing").to_string_lossy().into(), 2);
        let (_, folder, here, _) = rec.wait_for(2);
        assert_eq!(folder, dir.path().to_string_lossy());
        assert_eq!(here, 1);
        assert_eq!(rec.failures.lock().unwrap()[0].0, 2);
    }

    #[test]
    fn open_folder_refreshes_when_its_contents_change() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("inner")).unwrap();
        std::fs::write(root.join("a.txt"), b"").unwrap();
        let rec = Arc::new(Recorder::default());
        let engine = engine_without_index(rec.clone());

        engine.open_folder(root.join("inner").to_string_lossy().into(), 1);
        engine.set_query(String::new(), 2);
        assert_eq!(rec.wait_for(2).2, 0);
        std::thread::sleep(Duration::from_millis(300));

        // A new file appears in the open folder without any command, same generation.
        std::fs::write(root.join("inner/new.txt"), b"").unwrap();
        let (generation, _, here, _) = rec.wait_until(2, |e| e.2 == 1);
        assert_eq!(generation, 2);
        assert_eq!(engine.rows(2, 0, 10)[0].name, "new.txt");

        // Deleting the open folder moves up to its parent.
        std::fs::remove_dir_all(root.join("inner")).unwrap();
        let (_, folder, here_after, _) = rec.wait_until(2, |e| e.1 == root.to_string_lossy());
        assert_eq!((here, here_after), (1, 1));
        assert_eq!(folder, root.to_string_lossy());
    }

    #[test]
    fn everywhere_results_follow_here_results() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("projects/tilde/src")).unwrap();
        std::fs::write(root.join("projects/tilde/src/main.rs"), b"").unwrap();
        std::fs::write(root.join("projects/main-notes.md"), b"").unwrap();

        let rec = Arc::new(Recorder::default());
        let config = tilde_index::IndexConfig {
            home: root.clone(),
            scan: tilde_index::ScanConfig { roots: vec![root.clone()], ..Default::default() },
            zoxide: false,
            max_items: 1_000_000,
            cache_path: None,
        };
        let index = IndexService::with_config(config, false);
        let engine = Engine::start(rec.clone(), root.clone(), Some(index.clone()));
        let t = Instant::now();
        while index.status().state == IndexState::Scanning || index.status().items < 5 {
            assert!(t.elapsed() < Duration::from_secs(5), "scan didn't finish");
            std::thread::sleep(Duration::from_millis(10));
        }

        engine.open_folder(root.join("projects").to_string_lossy().into(), 1);
        engine.set_query("main".into(), 2);
        let (_, _, here, everywhere) = rec.wait_until(2, |e| e.3 > 0);
        assert_eq!((here, everywhere), (1, 1));
        let rows = engine.rows(2, 0, 10);
        assert_eq!(rows[0].name, "main-notes.md");
        assert_eq!(rows[0].section, crate::Section::Here);
        assert_eq!(rows[1].name, "main.rs");
        assert_eq!(rows[1].section, crate::Section::Everywhere);
        assert_eq!(rows[1].location, "~/projects/tilde/src");
        assert_eq!(rows[1].path, root.join("projects/tilde/src/main.rs").to_string_lossy());

        // An empty query shows only the folder.
        engine.set_query(String::new(), 3);
        let (_, _, here, everywhere) = rec.wait_for(3);
        assert_eq!((here, everywhere), (2, 0));
    }
}
