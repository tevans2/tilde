use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use tilde_fs::{ListOptions, list_dir, sort_entries};
use tilde_query::{HereMatcher, Hit, Query};
use tilde_types::Entry;

use crate::views::RowView;

/// Implemented by the Swift app. Called on a background thread.
#[uniffi::export(with_foreign)]
pub trait EngineListener: Send + Sync {
    /// Results for `generation` are ready to fetch with `Engine::rows`.
    fn results_ready(&self, generation: u64, folder: String, count: u32);
    /// The folder requested in `generation` could not be opened. The previous folder
    /// stays current.
    fn folder_failed(&self, generation: u64, folder: String, message: String);
}

enum Command {
    Open { path: PathBuf, generation: u64 },
    Query { text: String, generation: u64 },
    ShowHidden { show: bool, generation: u64 },
}

/// What `rows` reads from. Replaced wholesale by the worker after each command batch.
struct Published {
    generation: u64,
    folder: PathBuf,
    entries: Arc<Vec<Entry>>,
    hits: Vec<Hit>,
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
    #[uniffi::constructor]
    pub fn new(listener: Arc<dyn EngineListener>) -> Arc<Self> {
        let (tx, rx) = mpsc::channel();
        let published = Arc::new(Mutex::new(Published {
            generation: 0,
            folder: PathBuf::new(),
            entries: Arc::new(Vec::new()),
            hits: Vec::new(),
        }));
        let latest_open = Arc::new(AtomicU64::new(0));
        let worker = Worker {
            rx,
            listener,
            published: published.clone(),
            latest_open: latest_open.clone(),
            folder: PathBuf::new(),
            entries: Arc::new(Vec::new()),
            query: Query::default(),
            show_hidden: false,
            matcher: HereMatcher::default(),
        };
        thread::Builder::new().name("tilde-engine".into()).spawn(move || worker.run()).expect("spawn engine worker");
        Arc::new(Engine { tx, published, latest_open })
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

    /// Rows `start..start+count` of the results for `generation`. Empty if a newer
    /// generation has already replaced them.
    pub fn rows(&self, generation: u64, start: u32, count: u32) -> Vec<RowView> {
        let p = self.published.lock().unwrap();
        if p.generation != generation {
            return Vec::new();
        }
        let start = start as usize;
        let end = (start + count as usize).min(p.hits.len());
        p.hits
            .get(start..end)
            .unwrap_or_default()
            .iter()
            .map(|hit| RowView::new(&p.folder, &p.entries[hit.index as usize], hit))
            .collect()
    }

    /// Position of the entry called `name` in the results for `generation`.
    pub fn index_of(&self, generation: u64, name: String) -> Option<u32> {
        let p = self.published.lock().unwrap();
        if p.generation != generation {
            return None;
        }
        p.hits.iter().position(|h| p.entries[h.index as usize].name == name).map(|i| i as u32)
    }

    pub fn home_dir(&self) -> String {
        std::env::var("HOME").unwrap_or_else(|_| "/".into())
    }
}

struct Worker {
    rx: Receiver<Command>,
    listener: Arc<dyn EngineListener>,
    published: Arc<Mutex<Published>>,
    latest_open: Arc<AtomicU64>,
    folder: PathBuf,
    entries: Arc<Vec<Entry>>,
    query: Query,
    show_hidden: bool,
    matcher: HereMatcher,
}

impl Worker {
    fn run(mut self) {
        while let Ok(first) = self.rx.recv() {
            // Coalesce everything queued so fast typing only ranks the latest text.
            let mut open = None;
            let mut generation = 0;
            let mut relist = false;
            for cmd in std::iter::once(first).chain(self.rx.try_iter()) {
                match cmd {
                    Command::Open { path, generation: g } => {
                        open = Some((path, g));
                        generation = generation.max(g);
                    }
                    Command::Query { text, generation: g } => {
                        self.query = Query::parse(&text);
                        generation = generation.max(g);
                    }
                    Command::ShowHidden { show, generation: g } => {
                        relist |= show != self.show_hidden;
                        self.show_hidden = show;
                        generation = generation.max(g);
                    }
                }
            }

            let target = match open {
                Some((path, g)) => Some((path, g)),
                None if relist => Some((self.folder.clone(), generation)),
                None => None,
            };
            if let Some((path, open_generation)) = target {
                let opts = ListOptions { include_hidden: self.show_hidden, ..Default::default() };
                let latest = self.latest_open.clone();
                match list_dir(&path, &opts, || latest.load(Ordering::SeqCst) > open_generation) {
                    Ok(mut entries) => {
                        sort_entries(&mut entries);
                        self.folder = path;
                        self.entries = Arc::new(entries);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => {
                        self.listener.folder_failed(open_generation, path.to_string_lossy().into(), e.to_string());
                    }
                }
            }

            let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
            let hits = self.matcher.rank(&self.entries, &self.query, now);
            let count = hits.len() as u32;
            {
                let mut p = self.published.lock().unwrap();
                *p = Published { generation, folder: self.folder.clone(), entries: self.entries.clone(), hits };
            }
            self.listener.results_ready(generation, self.folder.to_string_lossy().into(), count);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Condvar;
    use std::time::Duration;

    #[derive(Default)]
    struct Recorder {
        events: Mutex<Vec<(u64, String, u32)>>,
        failures: Mutex<Vec<(u64, String)>>,
        cv: Condvar,
    }

    impl EngineListener for Recorder {
        fn results_ready(&self, generation: u64, folder: String, count: u32) {
            self.events.lock().unwrap().push((generation, folder, count));
            self.cv.notify_all();
        }
        fn folder_failed(&self, generation: u64, _folder: String, message: String) {
            self.failures.lock().unwrap().push((generation, message));
            self.cv.notify_all();
        }
    }

    impl Recorder {
        fn wait_for(&self, generation: u64) -> (u64, String, u32) {
            let mut events = self.events.lock().unwrap();
            loop {
                if let Some(e) = events.iter().find(|e| e.0 >= generation) {
                    return e.clone();
                }
                let (guard, timeout) = self.cv.wait_timeout(events, Duration::from_secs(5)).unwrap();
                assert!(!timeout.timed_out(), "no results for generation {generation}");
                events = guard;
            }
        }
    }

    #[test]
    fn open_query_and_fetch_rows() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("Sources")).unwrap();
        std::fs::write(dir.path().join("README.md"), b"# hi").unwrap();
        std::fs::write(dir.path().join("Package.swift"), b"").unwrap();

        let rec = Arc::new(Recorder::default());
        let engine = Engine::new(rec.clone());
        let path = dir.path().to_string_lossy().into_owned();

        engine.open_folder(path.clone(), 1);
        let (_, folder, count) = rec.wait_for(1);
        assert_eq!(folder, path);
        assert_eq!(count, 3);
        let rows = engine.rows(1, 0, 10);
        assert_eq!(rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["Sources", "Package.swift", "README.md"]);
        assert_eq!(rows[0].kind, crate::RowKind::Folder);

        engine.set_query("rdm".into(), 2);
        let (_, _, count) = rec.wait_for(2);
        assert_eq!(count, 1);
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
        let engine = Engine::new(rec.clone());
        engine.open_folder(dir.path().to_string_lossy().into(), 1);
        rec.wait_for(1);
        engine.open_folder(dir.path().join("missing").to_string_lossy().into(), 2);
        let (_, folder, count) = rec.wait_for(2);
        assert_eq!(folder, dir.path().to_string_lossy());
        assert_eq!(count, 1);
        assert_eq!(rec.failures.lock().unwrap()[0].0, 2);
    }
}
