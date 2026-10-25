use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tilde_activity::{FEED_LIMIT, Group, Item, Settings, local_midnight};
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
/// Changes elsewhere in the index refresh the Activity feed at most this often (the
/// inbox folders are watched directly and refresh it straight away).
const ACTIVITY_REFRESH: Duration = Duration::from_secs(2);

/// The Activity view's number-key filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum OriginFilter {
    All,
    Downloads,
    Screenshots,
    /// Edited or created.
    Edited,
    AirDrop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct ActivityFilter {
    pub origin: OriginFilter,
    /// Only what's still in an inbox folder (Downloads, Desktop, screenshots).
    pub inbox: bool,
}

impl From<ActivityFilter> for tilde_activity::Filter {
    fn from(f: ActivityFilter) -> Self {
        use tilde_activity::OriginFilter as O;
        let origin = match f.origin {
            OriginFilter::All => O::All,
            OriginFilter::Downloads => O::Downloads,
            OriginFilter::Screenshots => O::Screenshots,
            OriginFilter::Edited => O::Edited,
            OriginFilter::AirDrop => O::AirDrop,
        };
        tilde_activity::Filter { origin, inbox: f.inbox }
    }
}

/// A date heading in the Activity feed, before the Here row at `start`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RowGroup {
    pub start: u32,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ActivityInfo {
    pub filter: ActivityFilter,
    /// Items still in an inbox folder, whatever the filter.
    pub inbox_count: u32,
    /// Date headings; empty while searching (results are ranked, not dated).
    pub groups: Vec<RowGroup>,
}

/// What `results_ready` reports.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ResultsInfo {
    pub generation: u64,
    /// The open folder. In Activity, the folder it was opened from.
    pub folder: String,
    /// Set while the Activity view is showing; its items are the Here rows.
    pub activity: Option<ActivityInfo>,
    pub here_count: u32,
    pub everywhere_count: u32,
}

/// Implemented by the Swift app. Called on a background thread.
#[uniffi::export(with_foreign)]
pub trait EngineListener: Send + Sync {
    /// Results are ready to fetch with `Engine::rows`: `here_count` rows from the current
    /// folder (or Activity) followed by `everywhere_count` from the index. May be called
    /// more than once for a generation while the index is still being built or when the
    /// folder changes.
    fn results_ready(&self, info: ResultsInfo);
    /// The folder requested in `generation` could not be opened. The previous folder
    /// stays current.
    fn folder_failed(&self, generation: u64, folder: String, message: String);
}

pub(crate) enum Command {
    Open {
        path: PathBuf,
        generation: u64,
    },
    OpenActivity {
        filter: ActivityFilter,
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
    /// FSEvents reported a change directly inside this watched folder.
    FolderChanged(PathBuf),
    /// The index applied a batch of changes.
    IndexChanged,
}

/// The Activity items shown, as indices into `items` in Here order.
struct PublishedActivity {
    items: Arc<Vec<Item>>,
    shown: Vec<usize>,
}

/// What `rows` reads from. Replaced wholesale by the worker after each update.
struct Published {
    generation: u64,
    folder: PathBuf,
    home: PathBuf,
    entries: Arc<Vec<Entry>>,
    activity: Option<PublishedActivity>,
    here: Vec<Hit>,
    everywhere: Vec<GlobalHit>,
}

impl Published {
    fn here_name(&self, hit: &Hit) -> &str {
        match &self.activity {
            Some(a) => &a.items[a.shown[hit.index as usize]].name,
            None => &self.entries[hit.index as usize].name,
        }
    }
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
        Engine::start(listener, index.home().to_path_buf(), Some(index), None)
    }

    /// Lists `path` and makes it the current folder. The current query is kept; the UI
    /// normally clears it with `set_query` at the same time.
    pub fn open_folder(&self, path: String, generation: u64) {
        self.latest_open.fetch_max(generation, Ordering::SeqCst);
        let _ = self.tx.send(Command::Open { path: PathBuf::from(path), generation });
    }

    /// Shows the Activity view, or changes its filter if it's already showing. The
    /// current folder is remembered for coming back with `open_folder`.
    pub fn open_activity(&self, filter: ActivityFilter, generation: u64) {
        let _ = self.tx.send(Command::OpenActivity { filter, generation });
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
            .map(|i| match (p.here.get(i), &p.activity) {
                (Some(hit), Some(a)) => RowView::activity(&p.home, &a.items[a.shown[hit.index as usize]], hit),
                (Some(hit), None) => RowView::here(&p.folder, &p.entries[hit.index as usize], hit),
                (None, _) => RowView::everywhere(&p.home, &p.everywhere[i - p.here.len()]),
            })
            .collect()
    }

    /// Position of the Here row called `name` in the results for `generation`.
    pub fn index_of(&self, generation: u64, name: String) -> Option<u32> {
        let p = self.published.lock().unwrap();
        if p.generation != generation {
            return None;
        }
        p.here.iter().position(|h| p.here_name(h) == name).map(|i| i as u32)
    }

    pub fn home_dir(&self) -> String {
        home_dir().to_string_lossy().into_owned()
    }
}

impl Engine {
    /// `index: None` disables the Everywhere scope, and `activity` replaces the Activity
    /// settings from the config (both for tests).
    fn start(
        listener: Arc<dyn EngineListener>,
        home: PathBuf,
        index: Option<Arc<IndexService>>,
        activity: Option<Settings>,
    ) -> Arc<Self> {
        let (tx, rx) = mpsc::channel();
        let published = Arc::new(Mutex::new(Published {
            generation: 0,
            folder: PathBuf::new(),
            home: home.clone(),
            entries: Arc::new(Vec::new()),
            activity: None,
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
            watcher: None,
            watched: Vec::new(),
            listener,
            published: published.clone(),
            latest_open: latest_open.clone(),
            home,
            index,
            generation: 0,
            folder: PathBuf::new(),
            entries: Arc::new(Vec::new()),
            activity: None,
            fixed_activity: activity,
            query: Query::default(),
            show_hidden: false,
            matcher: HereMatcher::default(),
        };
        thread::Builder::new().name("tilde-engine".into()).spawn(move || worker.run()).expect("spawn engine worker");
        Arc::new(Engine { tx, published, latest_open })
    }
}

/// The Activity view while it's showing.
struct ActivityState {
    filter: ActivityFilter,
    items: Arc<Vec<Item>>,
    gathered: Instant,
}

struct Worker {
    rx: Receiver<Command>,
    /// For watchers to report back.
    tx: Sender<Command>,
    watcher: Option<Watcher>,
    /// Folders the watcher reports changes in: the open folder, or the inbox folders.
    watched: Vec<PathBuf>,
    listener: Arc<dyn EngineListener>,
    published: Arc<Mutex<Published>>,
    latest_open: Arc<AtomicU64>,
    home: PathBuf,
    index: Option<Arc<IndexService>>,
    generation: u64,
    folder: PathBuf,
    entries: Arc<Vec<Entry>>,
    activity: Option<ActivityState>,
    fixed_activity: Option<Settings>,
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
            let mut open_activity = None;
            let mut relist = false;
            let mut regather = false;
            let mut user_change = false;
            let mut index_changed = false;
            for cmd in std::iter::once(first).chain(self.rx.try_iter()) {
                user_change |= matches!(
                    cmd,
                    Command::Open { .. }
                        | Command::OpenActivity { .. }
                        | Command::Query { .. }
                        | Command::ShowHidden { .. }
                );
                match cmd {
                    Command::Open { path, generation } => {
                        open = Some((path, generation));
                        open_activity = None;
                        self.generation = self.generation.max(generation);
                    }
                    Command::OpenActivity { filter, generation } => {
                        open_activity = Some(filter);
                        open = None;
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
                    Command::FolderChanged(path) => {
                        let watched = self.watched.contains(&path);
                        regather |= watched && self.activity.is_some();
                        relist |= watched && path == self.folder;
                    }
                    Command::IndexChanged => index_changed = true,
                }
            }

            if let Some(filter) = open_activity {
                match &mut self.activity {
                    // Already showing: only the filter changed.
                    Some(state) => state.filter = filter,
                    None => self.enter_activity(filter),
                }
                self.publish();
                continue;
            }
            if open.is_none() && self.activity.is_some() {
                let stale = self.activity.as_ref().is_some_and(|a| a.gathered.elapsed() >= ACTIVITY_REFRESH);
                if regather || (index_changed && stale) {
                    self.refresh_activity();
                }
                if user_change || regather || (index_changed && (stale || !self.query.is_empty())) {
                    self.publish();
                }
                continue;
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
                        self.activity = None;
                        if self.watched != [path.clone()] {
                            self.watch(vec![path.clone()]);
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
                            self.watch(vec![up.clone()]);
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

    /// Replaces the watcher with one reporting changes directly inside `folders`.
    fn watch(&mut self, folders: Vec<PathBuf>) {
        self.watcher = None;
        let tx = self.tx.clone();
        let targets = folders.clone();
        self.watched = folders;
        self.watcher = watch(&self.watched, &[], SINCE_NOW, FOLDER_WATCH_LATENCY, move |batch| {
            // The stream is recursive; only changes directly inside the folders matter.
            for target in &targets {
                if batch.iter().any(|e| e.path == *target || (e.needs_rescan() && target.starts_with(&e.path))) {
                    let _ = tx.send(Command::FolderChanged(target.clone()));
                }
            }
        })
        .ok();
    }

    fn enter_activity(&mut self, filter: ActivityFilter) {
        let (settings, items) = self.gather_activity();
        let inbox: Vec<PathBuf> = settings.inbox.into_iter().filter(|p| p.is_dir()).collect();
        self.watch(inbox);
        self.activity = Some(ActivityState { filter, items: Arc::new(items), gathered: Instant::now() });
    }

    fn refresh_activity(&mut self) {
        let (_, items) = self.gather_activity();
        if let Some(state) = &mut self.activity {
            state.items = Arc::new(items);
            state.gathered = Instant::now();
        }
    }

    fn gather_activity(&self) -> (Settings, Vec<Item>) {
        let settings = self.fixed_activity.clone().unwrap_or_else(|| {
            let config = tilde_config::load(&tilde_config::config_path()).config.activity;
            Settings::new(self.home.clone(), config.inbox_paths(&self.home), config.days)
        });
        let now = now();
        let since = now - settings.days as i64 * 86_400;
        // Extra candidates: some will turn out to be in an inbox folder (listed anyway).
        let recent = self.index.as_ref().map(|i| i.recent(since, FEED_LIMIT * 2)).unwrap_or_default();
        let items = tilde_activity::gather(&settings, recent, now);
        (settings, items)
    }

    fn index_scanning(&self) -> bool {
        self.index.as_ref().is_some_and(|i| i.status().state == IndexState::Scanning)
    }

    fn publish(&mut self) {
        let now = now();
        let mut activity_info = None;
        let mut published_activity = None;
        let here = match &self.activity {
            Some(state) => {
                let filter: tilde_activity::Filter = state.filter.into();
                let shown: Vec<usize> = (0..state.items.len()).filter(|&i| filter.accepts(&state.items[i])).collect();
                let entries: Vec<Entry> = shown.iter().map(|&i| entry_for(&state.items[i])).collect();
                let here = self.matcher.rank(&entries, &self.query, now);
                // Ranked results aren't in date order, so only unranked ones get headings.
                let mut groups = Vec::new();
                if self.query.text.is_empty() {
                    let today = local_midnight(now);
                    let mut last = None;
                    for (row, hit) in here.iter().enumerate() {
                        let group = Group::of(state.items[shown[hit.index as usize]].time, today);
                        if last != Some(group) {
                            groups.push(RowGroup { start: row as u32, title: group.title().into() });
                            last = Some(group);
                        }
                    }
                }
                activity_info = Some(ActivityInfo {
                    filter: state.filter,
                    inbox_count: state.items.iter().filter(|i| i.in_inbox).count() as u32,
                    groups,
                });
                published_activity = Some(PublishedActivity { items: state.items.clone(), shown });
                here
            }
            None => self.matcher.rank(&self.entries, &self.query, now),
        };
        let exclude = if self.activity.is_some() { None } else { Some(self.folder.as_path()) };
        let everywhere = match &self.index {
            Some(index) if !self.query.is_empty() => index.search(&self.query, exclude, now, EVERYWHERE_LIMIT),
            _ => Vec::new(),
        };
        let (here_count, everywhere_count) = (here.len() as u32, everywhere.len() as u32);
        *self.published.lock().unwrap() = Published {
            generation: self.generation,
            folder: self.folder.clone(),
            home: self.home.clone(),
            entries: self.entries.clone(),
            activity: published_activity,
            here,
            everywhere,
        };
        self.listener.results_ready(ResultsInfo {
            generation: self.generation,
            folder: self.folder.to_string_lossy().into(),
            activity: activity_info,
            here_count,
            everywhere_count,
        });
    }
}

/// An Activity item as a folder entry, for matching and query filters.
fn entry_for(item: &Item) -> Entry {
    Entry {
        name: item.name.clone(),
        kind: item.kind,
        size: item.size,
        modified: item.time,
        file_id: 0,
        hidden: false,
        child_count: item.child_count,
        mount_point: false,
        added: 0,
    }
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
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
        infos: Mutex<Vec<ResultsInfo>>,
        cv: Condvar,
    }

    impl EngineListener for Recorder {
        fn results_ready(&self, info: ResultsInfo) {
            self.events.lock().unwrap().push((
                info.generation,
                info.folder.clone(),
                info.here_count,
                info.everywhere_count,
            ));
            self.infos.lock().unwrap().push(info);
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
        Engine::start(rec, home_dir(), None, None)
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
        let engine = Engine::start(rec.clone(), root.clone(), Some(index.clone()), None);
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

    #[test]
    fn activity_lists_inbox_items_with_filters_and_comes_back() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("Downloads")).unwrap();
        std::fs::create_dir_all(root.join("work")).unwrap();
        std::fs::write(root.join("Downloads/report.pdf"), b"x").unwrap();
        std::fs::write(root.join("Downloads/photo.png"), b"x").unwrap();
        std::fs::write(root.join("work/a.txt"), b"x").unwrap();

        let rec = Arc::new(Recorder::default());
        let settings = Settings { home: root.clone(), inbox: vec![root.join("Downloads")], days: 30 };
        let engine = Engine::start(rec.clone(), root.clone(), None, Some(settings));
        engine.open_folder(root.join("work").to_string_lossy().into(), 1);
        rec.wait_for(1);

        let all = ActivityFilter { origin: OriginFilter::All, inbox: false };
        engine.open_activity(all, 2);
        let (_, folder, here, _) = rec.wait_for(2);
        // The folder Activity was opened from is kept for coming back.
        assert_eq!((folder, here), (root.join("work").to_string_lossy().into_owned(), 2));
        let info = rec.infos.lock().unwrap().last().unwrap().clone();
        let activity = info.activity.unwrap();
        assert_eq!(activity.inbox_count, 2);
        assert_eq!(activity.groups, [RowGroup { start: 0, title: "Today".into() }]);
        let rows = engine.rows(2, 0, 10);
        assert_eq!(rows[0].location, "~/Downloads");
        assert_eq!(rows[0].origin, Some(crate::ActivityOrigin::Created));
        assert!(engine.index_of(2, "photo.png".into()).is_some());

        // Searching ranks without date headings.
        engine.set_query("rep".into(), 3);
        assert_eq!(rec.wait_for(3).2, 1);
        assert!(rec.infos.lock().unwrap().last().unwrap().activity.as_ref().unwrap().groups.is_empty());
        assert_eq!(engine.rows(3, 0, 10)[0].name, "report.pdf");

        engine.set_query(String::new(), 4);
        engine.open_activity(ActivityFilter { origin: OriginFilter::Downloads, inbox: false }, 5);
        assert_eq!(rec.wait_for(5).2, 0);

        // A new download shows up without a command.
        engine.open_activity(all, 6);
        rec.wait_for(6);
        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(root.join("Downloads/new.zip"), b"x").unwrap();
        let (generation, _, here, _) = rec.wait_until(6, |e| e.2 == 3);
        assert_eq!((generation, here), (6, 3));

        engine.open_folder(root.join("work").to_string_lossy().into(), 7);
        let (_, _, here, _) = rec.wait_for(7);
        assert_eq!(here, 1);
        assert!(rec.infos.lock().unwrap().last().unwrap().activity.is_none());
        assert_eq!(engine.rows(7, 0, 1)[0].name, "a.txt");
    }
}
