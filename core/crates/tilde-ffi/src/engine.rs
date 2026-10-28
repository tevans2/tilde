use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tilde_activity::{FEED_LIMIT, Group, Item, Settings, local_midnight};
use tilde_config::SortConfig;
use tilde_fs::{ListOptions, SINCE_NOW, Watcher, list_dir, sort_entries_by, watch};
use tilde_index::GlobalHit;
use tilde_query::{HereMatcher, Hit, Query};
use tilde_types::{Entry, EntryKind, SortKey, SortOrder};

use crate::contents::{self, ContentHit};
use crate::index::{IndexService, IndexState, home_dir};
use crate::sizes;
use crate::sorts::SortMemory;
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

/// What a folder is sorted by (`[sort] by` in the config).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SortBy {
    Name,
    Modified,
    /// When the item arrived in the folder.
    Added,
    Size,
    /// By extension, then name.
    Kind,
}

impl From<SortBy> for SortKey {
    fn from(s: SortBy) -> Self {
        match s {
            SortBy::Name => SortKey::Name,
            SortBy::Modified => SortKey::Modified,
            SortBy::Added => SortKey::Added,
            SortBy::Size => SortKey::Size,
            SortBy::Kind => SortKey::Kind,
        }
    }
}

impl From<SortKey> for SortBy {
    fn from(k: SortKey) -> Self {
        match k {
            SortKey::Name => SortBy::Name,
            SortKey::Modified => SortBy::Modified,
            SortKey::Added => SortBy::Added,
            SortKey::Size => SortBy::Size,
            SortKey::Kind => SortBy::Kind,
        }
    }
}

/// The open folder's order. Text searches rank by match instead; filters keep it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct FolderSort {
    pub by: SortBy,
    /// Against the natural direction (Z→A, oldest first, smallest first).
    pub reverse: bool,
    /// Differs from the config's default (it was chosen for this folder).
    pub custom: bool,
}

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

/// A `text:` search: its results replace Here and Everywhere.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ContentsInfo {
    pub count: u32,
    /// Still looking.
    pub searching: bool,
    /// What was searched (`1 234 files here · Spotlight elsewhere`), or why nothing is
    /// (too short, turned off). Empty while searching.
    pub note: String,
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
    /// Set while the query has `text:`; its rows follow the (then empty) Here and
    /// Everywhere ones.
    pub contents: Option<ContentsInfo>,
    pub sort: FolderSort,
    /// The archive file when the open folder is inside one (read-only, its rows' paths
    /// aren't real files: `extract_from_archives` makes them so).
    pub archive: Option<String>,
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
    Sort {
        order: (SortKey, bool),
        generation: u64,
    },
    /// FSEvents reported a change directly inside this watched folder.
    FolderChanged(PathBuf),
    /// The index applied a batch of changes.
    IndexChanged,
    /// Folder sizes asked for were measured.
    SizesChanged,
    /// The window closed: stop.
    Quit,
    /// Something changed in these folders below the open one (sizes are out of date).
    SubfoldersChanged(Vec<PathBuf>),
    /// Files found by `text:` search `job`; `done` is its note once it's finished.
    ContentsFound {
        job: u64,
        hits: Vec<ContentHit>,
        done: Option<String>,
    },
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
    contents: Vec<ContentHit>,
    /// Rows show when items were added instead of modified (sorting by added).
    show_added: bool,
    /// Folders (by name) whose size is known.
    measured: Arc<HashSet<String>>,
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
        Engine::start(listener, index.home().to_path_buf(), Some(index), None, SortMemory::on_disk(), None)
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

    /// Sorts the open folder and remembers the order for it (if `[sort] remember`).
    pub fn set_sort(&self, by: SortBy, reverse: bool, generation: u64) {
        let _ = self.tx.send(Command::Sort { order: (by.into(), reverse), generation });
    }

    /// Rows `start..start+count` of the results for `generation`: Here rows first, then
    /// Everywhere rows. Empty if a newer generation has replaced them.
    pub fn rows(&self, generation: u64, start: u32, count: u32) -> Vec<RowView> {
        let p = self.published.lock().unwrap();
        if p.generation != generation {
            return Vec::new();
        }
        let total = p.here.len() + p.everywhere.len() + p.contents.len();
        let (start, end) = (start as usize, (start as usize + count as usize).min(total));
        let after_here = p.here.len() + p.everywhere.len();
        (start..end)
            .map(|i| match (p.here.get(i), &p.activity) {
                (Some(hit), Some(a)) => RowView::activity(&p.home, &a.items[a.shown[hit.index as usize]], hit),
                (Some(hit), None) => {
                    let entry = &p.entries[hit.index as usize];
                    RowView::here(&p.folder, entry, hit, p.show_added, p.measured.contains(&entry.name))
                }
                (None, _) if i < after_here => RowView::everywhere(&p.home, &p.everywhere[i - p.here.len()]),
                (None, _) => RowView::contents(&p.home, &p.contents[i - after_here]),
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

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Quit);
    }
}

impl Engine {
    /// `index: None` disables the Everywhere scope, and `activity` and `sort` replace the
    /// Activity and sort settings from the config (all for tests).
    fn start(
        listener: Arc<dyn EngineListener>,
        home: PathBuf,
        index: Option<Arc<IndexService>>,
        activity: Option<Settings>,
        sorts: SortMemory,
        sort: Option<SortConfig>,
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
            contents: Vec::new(),
            show_added: false,
            measured: Arc::new(HashSet::new()),
        }));
        let latest_open = Arc::new(AtomicU64::new(0));
        if let Some(index) = &index {
            index.subscribe(tx.clone());
        }
        static ENGINES: AtomicU64 = AtomicU64::new(0);
        let worker = Worker {
            id: ENGINES.fetch_add(1, Ordering::Relaxed),
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
            sort: SortOrder::default(),
            sort_default: SortOrder::default(),
            sorts,
            fixed_sort: sort,
            contents: ContentsState::default(),
            contents_current: Arc::new(AtomicU64::new(0)),
            archive: None,
            folder_sizes: false,
            measured: Arc::new(HashSet::new()),
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
    /// Tells this engine's folder size requests apart from other windows'.
    id: u64,
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
    /// The open folder's order, and the config's default when last read.
    sort: SortOrder,
    sort_default: SortOrder,
    sorts: SortMemory,
    fixed_sort: Option<SortConfig>,
    contents: ContentsState,
    /// The `text:` search job that may still report; others stop.
    contents_current: Arc<AtomicU64>,
    /// The archive file, when the open folder is inside one.
    archive: Option<PathBuf>,
    /// `[sort] folder_sizes`, when last read.
    folder_sizes: bool,
    /// Folders in `entries` (by name) whose size is known.
    measured: Arc<HashSet<String>>,
}

/// The `text:` search showing.
#[derive(Default)]
struct ContentsState {
    /// What it's for (text, folder, names, filters, hidden): a change starts a new job.
    key: Option<String>,
    job: u64,
    inside: Vec<ContentHit>,
    outside: Vec<ContentHit>,
    searching: bool,
    note: String,
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
                    Err(RecvTimeoutError::Disconnected) => {
                        sizes::want(self.id, Vec::new(), self.tx.clone());
                        return;
                    }
                }
            } else {
                match self.rx.recv() {
                    Ok(cmd) => Some(cmd),
                    Err(_) => {
                        sizes::want(self.id, Vec::new(), self.tx.clone());
                        return;
                    }
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
            let mut contents_changed = false;
            let mut sizes_changed = false;
            let mut subfolders_changed = false;
            let queued: Vec<Command> = std::iter::once(first).chain(self.rx.try_iter()).collect();
            for cmd in queued {
                user_change |= matches!(
                    cmd,
                    Command::Open { .. }
                        | Command::OpenActivity { .. }
                        | Command::Query { .. }
                        | Command::ShowHidden { .. }
                        | Command::Sort { .. }
                        | Command::Quit
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
                    Command::Sort { order: (key, reverse), generation } => {
                        let config = self.sort_config();
                        self.sort = SortOrder { key, reverse, folders_first: config.folders_first };
                        if config.same_everywhere {
                            self.save_default_sort(key, reverse);
                        } else if config.remember && !self.folder.as_os_str().is_empty() {
                            self.sorts.set(&self.folder, self.sort, config.order());
                        }
                        let mut entries = (*self.entries).clone();
                        sort_entries_by(&mut entries, self.sort);
                        self.entries = Arc::new(entries);
                        self.generation = self.generation.max(generation);
                    }
                    Command::FolderChanged(path) => {
                        let watched = self.watched.contains(&path);
                        regather |= watched && self.activity.is_some();
                        relist |= watched && path == self.folder;
                    }
                    Command::IndexChanged => index_changed = true,
                    Command::Quit => {
                        sizes::want(self.id, Vec::new(), self.tx.clone());
                        self.contents_current.fetch_add(1, Ordering::SeqCst);
                        return;
                    }
                    Command::SizesChanged => sizes_changed = true,
                    Command::SubfoldersChanged(folders) => {
                        if self.folder_sizes {
                            sizes::changed(&folders);
                            subfolders_changed = true;
                        }
                    }
                    Command::ContentsFound { job, hits, done } => {
                        if job == self.contents.job && self.contents.key.is_some() {
                            for hit in hits {
                                if hit.outside {
                                    self.contents.outside.push(hit)
                                } else {
                                    self.contents.inside.push(hit)
                                }
                            }
                            // The scan finds them in any order; the folder's read best by path.
                            self.contents.inside.sort_by(|a, b| a.path.cmp(&b.path));
                            if let Some(note) = done {
                                self.contents.searching = false;
                                self.contents.note = note;
                            }
                            contents_changed = true;
                        }
                    }
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

            if subfolders_changed {
                self.request_sizes();
            }
            if sizes_changed {
                self.apply_sizes();
            }

            // Index updates only matter to a search in progress.
            if !user_change
                && !relist
                && !contents_changed
                && !sizes_changed
                && !(index_changed && !self.query.is_empty())
            {
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
                // A refresh keeps the order; another folder has its own (or the default).
                let order = if path == self.folder { self.sort } else { self.order_for(&path) };
                let latest = self.latest_open.clone();
                match list_folder(&path, &opts, || latest.load(Ordering::SeqCst) > open_generation) {
                    Ok((mut entries, archive)) => {
                        sort_entries_by(&mut entries, order);
                        self.sort = order;
                        // Only folders the user navigated to: not the one opened at launch.
                        if explicit_open
                            && !self.folder.as_os_str().is_empty()
                            && path != self.folder
                            && archive.is_none()
                            && let Some(index) = &self.index
                        {
                            index.record_visit(&path);
                        }
                        self.activity = None;
                        // An archive isn't watched: it's read again when it's opened again.
                        let watched = if archive.is_some() { Vec::new() } else { vec![path.clone()] };
                        if self.watched != watched {
                            self.watch(watched);
                        }
                        self.archive = archive;
                        self.folder = path;
                        self.entries = Arc::new(entries);
                        self.request_sizes();
                        self.apply_sizes();
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) if !explicit_open && e.kind() == std::io::ErrorKind::NotFound => {
                        // The open folder was deleted: move to the nearest folder that exists.
                        let mut up = path.clone();
                        while !up.is_dir() && up.pop() {}
                        if let Ok(mut entries) = list_dir(&up, &opts, || false) {
                            self.sort = self.order_for(&up);
                            sort_entries_by(&mut entries, self.sort);
                            self.watch(vec![up.clone()]);
                            self.archive = None;
                            self.folder = up;
                            self.entries = Arc::new(entries);
                            self.request_sizes();
                            self.apply_sizes();
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
            // The stream is recursive: changes directly inside a folder relist it, deeper
            // ones only change folder sizes.
            for target in &targets {
                if batch.iter().any(|e| e.path == *target || (e.needs_rescan() && target.starts_with(&e.path))) {
                    let _ = tx.send(Command::FolderChanged(target.clone()));
                }
                let deeper: Vec<PathBuf> = batch
                    .iter()
                    .filter(|e| e.path != *target && e.path.starts_with(target))
                    .map(|e| e.path.clone())
                    .collect();
                if !deeper.is_empty() {
                    let _ = tx.send(Command::SubfoldersChanged(deeper));
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
        self.request_sizes();
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

    /// Reads `[sort]` (on opening a folder or sorting, not per keystroke).
    fn sort_config(&mut self) -> SortConfig {
        let config =
            self.fixed_sort.clone().unwrap_or_else(|| tilde_config::load(&tilde_config::config_path()).config.sort);
        self.sort_default = config.order();
        self.folder_sizes = config.folder_sizes;
        config
    }

    /// Asks for the sizes of the folders listed (or for none, in Activity, inside an
    /// archive or with `[sort] folder_sizes` off).
    fn request_sizes(&self) {
        let paths = if self.folder_sizes && self.archive.is_none() && self.activity.is_none() {
            self.entries
                .iter()
                .filter(|e| matches!(e.kind, EntryKind::Folder | EntryKind::Package) && !e.mount_point)
                .map(|e| self.folder.join(&e.name))
                .collect()
        } else {
            Vec::new()
        };
        sizes::want(self.id, paths, self.tx.clone());
    }

    /// Puts the folder sizes known into `entries` (sorting again if by size).
    fn apply_sizes(&mut self) {
        if self.archive.is_some() {
            // Listed with their sizes.
            let folders = self.entries.iter().filter(|e| e.kind == EntryKind::Folder).map(|e| e.name.clone());
            self.measured = Arc::new(folders.collect());
            return;
        }
        if !self.folder_sizes {
            self.measured = Arc::new(HashSet::new());
            return;
        }
        let mut entries = (*self.entries).clone();
        let mut measured = HashSet::new();
        let mut changed = false;
        for e in entries.iter_mut().filter(|e| matches!(e.kind, EntryKind::Folder | EntryKind::Package)) {
            if let Some(size) = sizes::get(&self.folder.join(&e.name)) {
                changed |= e.size != size;
                e.size = size;
                measured.insert(e.name.clone());
            }
        }
        if changed {
            if self.sort.key == SortKey::Size {
                sort_entries_by(&mut entries, self.sort);
            }
            self.entries = Arc::new(entries);
        }
        self.measured = Arc::new(measured);
    }

    /// Makes `key` (and `reverse`) the order for every folder: saved in the config.
    fn save_default_sort(&mut self, key: SortKey, reverse: bool) {
        if let Some(fixed) = &mut self.fixed_sort {
            (fixed.by, fixed.reverse) = (key.as_str().into(), reverse);
        } else {
            let path = tilde_config::config_path();
            let _ = tilde_config::set_value(&path, &["sort", "by"], tilde_config::Value::Text(key.as_str().into()));
            let _ = tilde_config::set_value(&path, &["sort", "reverse"], tilde_config::Value::Bool(reverse));
        }
        self.sort_config();
    }

    /// The order remembered for `folder`, else the default.
    fn order_for(&mut self, folder: &std::path::Path) -> SortOrder {
        let config = self.sort_config();
        let default = config.order();
        if config.same_everywhere || !config.remember {
            return default;
        }
        self.sorts.get(folder, default).unwrap_or(default)
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
        let contents_info = self.update_contents();
        let exclude = if self.activity.is_some() { None } else { Some(self.folder.as_path()) };
        let (here, everywhere) = match &self.index {
            _ if contents_info.is_some() => (Vec::new(), Vec::new()),
            Some(index) if !self.query.is_empty() => (here, index.search(&self.query, exclude, now, EVERYWHERE_LIMIT)),
            _ => (here, Vec::new()),
        };
        let mut contents = Vec::new();
        if contents_info.is_some() {
            contents.extend(self.contents.inside.iter().cloned());
            contents.extend(self.contents.outside.iter().cloned());
        }
        let (here_count, everywhere_count) = (here.len() as u32, everywhere.len() as u32);
        *self.published.lock().unwrap() = Published {
            generation: self.generation,
            folder: self.folder.clone(),
            home: self.home.clone(),
            entries: self.entries.clone(),
            activity: published_activity,
            here,
            everywhere,
            contents,
            show_added: self.sort.key == SortKey::Added,
            measured: self.measured.clone(),
        };
        let default = self.sort_default;
        self.listener.results_ready(ResultsInfo {
            generation: self.generation,
            folder: self.folder.to_string_lossy().into(),
            activity: activity_info,
            here_count,
            everywhere_count,
            contents: contents_info,
            sort: FolderSort {
                by: self.sort.key.into(),
                reverse: self.sort.reverse,
                custom: self.sort.key != default.key || self.sort.reverse != default.reverse,
            },
            archive: self.archive.as_ref().map(|a| a.to_string_lossy().into_owned()),
        });
    }
}

impl Worker {
    /// Starts, keeps or stops the `text:` search for the current query, and says what
    /// it shows. `None` when the query has no `text:` (or Activity is showing).
    fn update_contents(&mut self) -> Option<ContentsInfo> {
        let needle = match (&self.query.contents, &self.activity) {
            (Some(needle), None) => needle.clone(),
            _ => {
                if self.contents.key.take().is_some() {
                    self.contents_current.fetch_add(1, Ordering::SeqCst);
                    self.contents = ContentsState::default();
                }
                return None;
            }
        };
        let key = format!(
            "{needle}\0{}\0{}\0{:?}\0{}\0{}",
            self.folder.display(),
            self.query.text,
            self.query.filters,
            self.show_hidden,
            self.query.contents_all
        );
        if self.contents.key.as_deref() != Some(key.as_str()) {
            let job = self.contents_current.fetch_add(1, Ordering::SeqCst) + 1;
            self.contents = ContentsState { key: Some(key), job, ..Default::default() };
            let config = tilde_config::load(&tilde_config::config_path()).config;
            if !config.search.contents {
                self.contents.note = "Searching inside files is off: Settings › Search".into();
            } else if self.archive.is_some() {
                self.contents.note = "Searching inside files doesn't look in archives".into();
            } else if needle.chars().count() < contents::MIN_CHARS {
                self.contents.note = format!("Type at least {} characters to search inside files", contents::MIN_CHARS);
            } else {
                self.contents.searching = true;
                let job = contents::Job {
                    id: job,
                    needle,
                    folder: self.folder.clone(),
                    home: self.home.clone(),
                    query: self.query.clone(),
                    hidden: self.show_hidden,
                    all: self.query.contents_all,
                    spotlight: config.search.spotlight,
                    skip: config.cleanup.build_folders.clone(),
                };
                let (index, current, tx) = (self.index.clone(), self.contents_current.clone(), self.tx.clone());
                let _ = thread::Builder::new()
                    .name("tilde-contents".into())
                    .spawn(move || contents::run(job, index, current, tx));
            }
        }
        let c = &self.contents;
        Some(ContentsInfo {
            count: (c.inside.len() + c.outside.len()) as u32,
            searching: c.searching,
            note: c.note.clone(),
        })
    }
}

/// Lists a folder, or a folder inside an archive (returning the archive too).
fn list_folder(
    path: &Path,
    opts: &ListOptions,
    cancel: impl Fn() -> bool,
) -> io::Result<(Vec<Entry>, Option<PathBuf>)> {
    match list_dir(path, opts, &cancel) {
        Err(e) if matches!(e.raw_os_error(), Some(libc::ENOTDIR) | Some(libc::ENOENT)) => {
            let Some((archive, inner)) = tilde_archive::split(path) else { return Err(e) };
            let mut entries = tilde_archive::list(&archive, &inner, &cancel)?;
            if !opts.include_hidden {
                entries.retain(|e| !e.hidden);
            }
            Ok((entries, Some(archive)))
        }
        result => result.map(|entries| (entries, None)),
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
        Engine::start(rec, home_dir(), None, None, SortMemory::in_memory(), Some(SortConfig::default()))
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
    fn sorting_is_remembered_per_folder() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::create_dir_all(a.join("sub")).unwrap();
        std::fs::create_dir(&b).unwrap();
        let t0 = SystemTime::now() - Duration::from_secs(3600);
        for (name, size, age) in [("big.txt", 300, 30), ("old.pdf", 100, 50), ("new.txt", 200, 10)] {
            std::fs::write(a.join(name), vec![b'x'; size]).unwrap();
            let f = std::fs::File::options().write(true).open(a.join(name)).unwrap();
            f.set_modified(t0 + Duration::from_secs(60 - age)).unwrap();
        }
        let rec = Arc::new(Recorder::default());
        let per_folder = SortConfig { same_everywhere: false, ..SortConfig::default() };
        let engine = Engine::start(rec.clone(), home_dir(), None, None, SortMemory::in_memory(), Some(per_folder));
        let names = |g| engine.rows(g, 0, 10).iter().map(|r| r.name.clone()).collect::<Vec<_>>().join(" ");
        let sort_of = |g| rec.infos.lock().unwrap().iter().rev().find(|i| i.generation == g).unwrap().sort;

        engine.open_folder(a.to_string_lossy().into(), 1);
        rec.wait_for(1);
        assert_eq!(names(1), "sub big.txt new.txt old.pdf");
        assert_eq!(sort_of(1), FolderSort { by: SortBy::Name, reverse: false, custom: false });

        engine.set_sort(SortBy::Size, false, 2);
        rec.wait_for(2);
        assert_eq!(names(2), "sub big.txt new.txt old.pdf");
        engine.set_sort(SortBy::Modified, false, 3);
        rec.wait_for(3);
        assert_eq!(names(3), "sub new.txt big.txt old.pdf", "newest first");
        assert_eq!(sort_of(3), FolderSort { by: SortBy::Modified, reverse: false, custom: true });

        // A filter keeps the order; typed text ranks by match.
        engine.set_query("ext:txt".into(), 4);
        rec.wait_for(4);
        assert_eq!(names(4), "new.txt big.txt");
        engine.set_query("old".into(), 5);
        rec.wait_for(5);
        assert_eq!(names(5), "old.pdf");
        engine.set_query(String::new(), 6);
        rec.wait_for(6);

        // Another folder has the default; coming back restores this folder's order.
        engine.open_folder(b.to_string_lossy().into(), 7);
        rec.wait_for(7);
        assert_eq!(sort_of(7).by, SortBy::Name);
        engine.open_folder(a.to_string_lossy().into(), 8);
        rec.wait_for(8);
        assert_eq!(names(8), "sub new.txt big.txt old.pdf");

        // Choosing the default again forgets it.
        engine.set_sort(SortBy::Name, true, 9);
        rec.wait_for(9);
        assert_eq!(names(9), "sub old.pdf new.txt big.txt");
        engine.set_sort(SortBy::Name, false, 10);
        rec.wait_for(10);
        assert!(!sort_of(10).custom);
    }

    #[test]
    fn a_sort_applies_to_every_folder() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        for (folder, files) in [(&a, [("small.txt", 1), ("big.txt", 500)]), (&b, [("tiny.md", 2), ("huge.md", 900)])] {
            for (name, size) in files {
                std::fs::write(folder.join(name), vec![b'x'; size]).unwrap();
            }
        }
        let rec = Arc::new(Recorder::default());
        let engine = engine_without_index(rec.clone());
        let names = |g| engine.rows(g, 0, 10).iter().map(|r| r.name.clone()).collect::<Vec<_>>().join(" ");
        let sort_of = |g| rec.infos.lock().unwrap().iter().rev().find(|i| i.generation == g).unwrap().sort;

        engine.open_folder(a.to_string_lossy().into(), 1);
        rec.wait_for(1);
        assert_eq!(names(1), "big.txt small.txt");
        engine.set_sort(SortBy::Size, true, 2);
        rec.wait_for(2);
        assert_eq!(names(2), "small.txt big.txt");

        // Another folder: sorted the same way, which is now the default.
        engine.open_folder(b.to_string_lossy().into(), 3);
        rec.wait_for(3);
        assert_eq!(names(3), "tiny.md huge.md");
        assert_eq!(sort_of(3), FolderSort { by: SortBy::Size, reverse: true, custom: false });
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
    fn text_searches_inside_files_in_the_background() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/ship.rs"), "fn spawn() {}\n// Hold space to charge the warp drive\n").unwrap();
        std::fs::write(root.join("notes.md"), "warp, then warp again\n").unwrap();
        std::fs::write(root.join("other.txt"), "nothing here\n").unwrap();

        let rec = Arc::new(Recorder::default());
        let engine =
            Engine::start(rec.clone(), root.clone(), None, None, SortMemory::in_memory(), Some(SortConfig::default()));
        engine.open_folder(root.to_string_lossy().into(), 1);
        rec.wait_for(1);

        let done = |generation: u64| {
            let t = Instant::now();
            loop {
                let info = rec.infos.lock().unwrap().iter().rev().find(|i| i.generation == generation).cloned();
                if let Some(info) = info
                    && let Some(c) = &info.contents
                    && !c.searching
                {
                    return c.clone();
                }
                assert!(t.elapsed() < Duration::from_secs(5), "contents search didn't finish");
                std::thread::sleep(Duration::from_millis(10));
            }
        };

        engine.set_query("text:warp".into(), 2);
        let c = done(2);
        assert_eq!(c.count, 2, "{c:?}");
        let rows = engine.rows(2, 0, 10);
        assert_eq!(rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["notes.md", "ship.rs"]);
        assert!(rows.iter().all(|r| r.section == crate::Section::Contents));
        assert_eq!(rows[1].line, 2);
        assert_eq!(rows[1].location, "2  …e to charge the warp drive");
        // The folder is the (test) home.
        assert_eq!(rows[0].source, "~ · 2 matches");

        // Names narrow it; too short a text searches nothing.
        engine.set_query("ship text:warp".into(), 3);
        assert_eq!(done(3).count, 1);
        engine.set_query("text:wa".into(), 4);
        let c = done(4);
        assert_eq!(c.count, 0);
        assert!(c.note.contains("at least 3"), "{c:?}");
        // Without text: it's an ordinary search again.
        engine.set_query("notes".into(), 5);
        let (_, _, here, _) = rec.wait_for(5);
        assert_eq!(here, 1);
        assert!(rec.infos.lock().unwrap().last().unwrap().contents.is_none());
    }

    #[test]
    fn folders_show_their_size_and_sort_by_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("big/inner")).unwrap();
        std::fs::create_dir_all(root.join("small")).unwrap();
        std::fs::write(root.join("big/inner/video.mov"), vec![0u8; 50_000]).unwrap();
        std::fs::write(root.join("small/note.txt"), vec![0u8; 10]).unwrap();
        let rec = Arc::new(Recorder::default());
        let engine = engine_without_index(rec.clone());
        engine.open_folder(root.to_string_lossy().into(), 1);
        rec.wait_for(1);
        engine.set_sort(SortBy::Size, true, 2);
        let t = Instant::now();
        let rows = loop {
            let rows = engine.rows(2, 0, 10);
            if rows.len() == 2 && rows.iter().all(|r| r.measured) {
                break rows;
            }
            assert!(t.elapsed() < Duration::from_secs(5), "sizes didn't arrive: {rows:?}");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(
            rows.iter().map(|r| (r.name.as_str(), r.size)).collect::<Vec<_>>(),
            [("small", 10), ("big", 50_000)]
        );
    }

    #[test]
    fn archives_open_as_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("src/sprites")).unwrap();
        std::fs::write(root.join("src/sprites/ship.png"), vec![1u8; 300]).unwrap();
        std::fs::write(root.join("src/README.md"), "# hi").unwrap();
        let zip = root.join("assets.zip");
        let made = std::process::Command::new("/usr/bin/zip")
            .args(["-qr", zip.to_str().unwrap(), "."])
            .current_dir(root.join("src"))
            .status()
            .unwrap();
        assert!(made.success());

        let rec = Arc::new(Recorder::default());
        let engine = engine_without_index(rec.clone());
        engine.open_folder(zip.to_string_lossy().into(), 1);
        rec.wait_for(1);
        let info = rec.infos.lock().unwrap().last().unwrap().clone();
        assert_eq!(info.archive.as_deref(), Some(zip.to_str().unwrap()));
        let rows = engine.rows(1, 0, 10);
        assert_eq!(rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["sprites", "README.md"]);
        assert_eq!((rows[0].size, rows[0].measured), (300, true));
        assert_eq!(rows[0].path, zip.join("sprites").to_string_lossy());

        engine.open_folder(rows[0].path.clone(), 2);
        rec.wait_for(2);
        assert_eq!(engine.rows(2, 0, 10)[0].name, "ship.png");
        let real = crate::extract_from_archives(vec![engine.rows(2, 0, 1)[0].path.clone()]).unwrap();
        assert_eq!(std::fs::read(&real[0]).unwrap().len(), 300);

        // Out again: an ordinary folder.
        engine.open_folder(root.to_string_lossy().into(), 3);
        rec.wait_for(3);
        assert!(rec.infos.lock().unwrap().last().unwrap().archive.is_none());
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
            frecency: None,
            zoxide: false,
            max_items: 1_000_000,
            cache_path: None,
        };
        let index = IndexService::with_config(config, false);
        let engine = Engine::start(
            rec.clone(),
            root.clone(),
            Some(index.clone()),
            None,
            SortMemory::in_memory(),
            Some(SortConfig::default()),
        );
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
        let engine = Engine::start(
            rec.clone(),
            root.clone(),
            None,
            Some(settings),
            SortMemory::in_memory(),
            Some(SortConfig::default()),
        );
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
