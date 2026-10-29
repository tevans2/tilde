//! Folder sizes, measured in the background and shared by every window.
//!
//! A folder the index lists has its total kept by the index, current as files change
//! ([`tilde_index::IndexHandle::folder_total`]); only the folders inside it that the
//! index doesn't enter (`node_modules`, `.git`, packages…) are measured here, and their
//! sizes handed back to it. Anything else (`~/Library`, other disks) is measured whole.
//!
//! An engine asks for the folders it's showing ([`want`]); a few low-priority threads
//! measure what's needed, remember the totals (and those of folders a couple of levels
//! inside, so going in shows them at once), and tell the engines that asked. Sizes are
//! saved, so they show straight away after a restart.
//!
//! Measuring costs reading every folder inside (seconds for `~/Library` with a warm
//! disk cache, minutes with a cold one), so how often a size is measured again depends
//! on how long it took: after a change inside ([`changed`]) at most every
//! [`CHANGE_FACTOR`] times that, and otherwise once it's [`FRESH_FACTOR`] times that old.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tilde_index::IndexHandle;

use crate::engine::Command;
use crate::index::home_dir;

const THREADS: usize = 3;
/// Levels below a measured folder whose totals are remembered too.
const KEEP_LEVELS: usize = 2;
/// After a change inside, a folder is measured again once this many times as long as it
/// took has passed (at least [`MIN_CHANGE_WAIT`]).
const CHANGE_FACTOR: u32 = 20;
const MIN_CHANGE_WAIT: Duration = Duration::from_secs(30);
/// Without a change seen, a size is measured again (when it's next shown, the old one
/// showing meanwhile) once it's this many times as old as it took (at least
/// [`MIN_FRESH`]): changes while Tilde wasn't watching.
const FRESH_FACTOR: u32 = 200;
const MIN_FRESH: Duration = Duration::from_secs(10 * 60);
/// Remembered sizes before the cache is trimmed to what's being shown.
const MAX_CACHED: usize = 100_000;
/// Saved at most this often.
const SAVE_EVERY: Duration = Duration::from_secs(30);

struct Known {
    size: u64,
    measured: SystemTime,
    /// How long measuring took (for its folder; folders inside get the same).
    took: Duration,
    /// Something inside changed since.
    stale: bool,
}

impl Known {
    /// When to measure it again, or `None` if it's fresh.
    fn due(&self, now: SystemTime) -> Option<SystemTime> {
        if self.stale {
            return Some(self.after_change());
        }
        (self.measured + (self.took * FRESH_FACTOR).max(MIN_FRESH) <= now).then_some(now)
    }

    /// When to measure it again after something inside changed.
    fn after_change(&self) -> SystemTime {
        self.measured + (self.took * CHANGE_FACTOR).max(MIN_CHANGE_WAIT)
    }
}

struct Job {
    path: PathBuf,
    not_before: SystemTime,
    /// The index to hand the size to (the folder is one it doesn't enter).
    index: Option<IndexHandle>,
}

/// Whether anyone shows `path` or a folder it's inside.
fn is_wanted(owners: &HashMap<u64, Owner>, path: &Path) -> bool {
    owners.values().any(|o| path.ancestors().any(|a| o.wanted.contains(a)))
}

/// One engine's wishes.
struct Owner {
    wanted: HashSet<PathBuf>,
    tx: Sender<Command>,
}

#[derive(Default)]
struct State {
    cache: HashMap<PathBuf, Known>,
    queue: Vec<Job>,
    /// Folders being measured, each with a flag that stops it.
    running: HashMap<PathBuf, Arc<AtomicBool>>,
    owners: HashMap<u64, Owner>,
    started: bool,
    /// Where sizes are saved (`None` in tests).
    file: Option<PathBuf>,
    saved: Option<Instant>,
}

struct Service {
    state: Mutex<State>,
    wake: Condvar,
}

fn service() -> &'static Service {
    static SERVICE: OnceLock<Service> = OnceLock::new();
    SERVICE.get_or_init(|| {
        let file = (!cfg!(test)).then(|| home_dir().join("Library/Caches/Tilde/folder-sizes.json"));
        let cache = file.as_deref().map(load).unwrap_or_default();
        Service { state: Mutex::new(State { cache, file, ..Default::default() }), wake: Condvar::new() }
    })
}

/// The size of `path`: from the index once everything inside it is measured, otherwise
/// the last measured size, if any (possibly out of date).
pub(crate) fn get(path: &Path, index: Option<&IndexHandle>) -> Option<u64> {
    if let Some(total) = index.and_then(|i| i.folder_total(path))
        && total.unmeasured == 0
    {
        return Some(total.bytes);
    }
    service().state.lock().unwrap().cache.get(path).map(|k| k.size)
}

/// `owner` (an engine) shows `paths` now, instead of what it asked for before: measures
/// those not known or out of date (for folders `index` lists, just the folders inside
/// it doesn't enter), and sends `Command::SizesChanged` to `tx` as each is done. An empty
/// list stops measuring for it.
pub(crate) fn want(owner: u64, paths: Vec<PathBuf>, tx: Sender<Command>, index: Option<IndexHandle>) {
    let s = service();
    let (listed, whole): (Vec<PathBuf>, Vec<PathBuf>) =
        paths.iter().cloned().partition(|p| index.as_ref().is_some_and(|i| i.folder_total(p).is_some()));
    let skipped = match &index {
        Some(i) if !listed.is_empty() => i.skipped_inside(&listed),
        _ => Vec::new(),
    };
    let mut state = s.state.lock().unwrap();
    let now = SystemTime::now();
    // What's asked for now goes before what was asked for earlier.
    let mut jobs = Vec::new();
    let busy =
        |state: &State, path: &Path| state.running.contains_key(path) || state.queue.iter().any(|j| j.path == path);
    for path in whole {
        if busy(&state, &path) {
            continue;
        }
        let not_before = match state.cache.get(&path) {
            None => now,
            Some(k) => match k.due(now) {
                Some(t) => t,
                None => continue,
            },
        };
        jobs.push(Job { path, not_before, index: None });
    }
    let mut handed = false;
    for folder in skipped {
        if busy(&state, &folder.path) {
            continue;
        }
        let known = state.cache.get(&folder.path);
        let not_before = match (folder.size, known) {
            // Measured recently for something else: no need to walk it again.
            (None, Some(k)) if k.due(now).is_none() => {
                handed |= index.as_ref().is_some_and(|i| i.set_skipped_size(&folder.path, k.size));
                continue;
            }
            (None, _) => now,
            (Some(_), Some(k)) if folder.stale => k.after_change(),
            (Some(_), None) if folder.stale => now,
            // Changes in some places (`~/Library`) aren't seen: measured again with age.
            (Some(_), Some(k)) => match k.due(now) {
                Some(t) => t,
                None => continue,
            },
            (Some(_), None) => continue,
        };
        jobs.push(Job { path: folder.path, not_before, index: index.clone() });
    }
    if handed {
        let _ = tx.send(Command::SizesChanged);
    }
    state.queue.splice(0..0, jobs);
    if paths.is_empty() {
        state.owners.remove(&owner);
    } else {
        state.owners.insert(owner, Owner { wanted: paths.into_iter().collect(), tx });
    }
    // Nobody wants the rest any more: drop what's queued, stop what's running, and
    // let a stopped walk run on if something wants it again.
    let State { queue, owners, running, .. } = &mut *state;
    queue.retain(|j| is_wanted(owners, &j.path));
    for (path, stop) in running.iter() {
        stop.store(!is_wanted(owners, path), Ordering::Relaxed);
    }
    if !state.started && !state.queue.is_empty() {
        state.started = true;
        for i in 0..THREADS {
            let _ =
                thread::Builder::new().name(format!("tilde-sizes-{i}")).stack_size(8 << 20).spawn(|| worker(service()));
        }
    }
    s.wake.notify_all();
}

/// Something changed in each of `folders`: the sizes of the folders containing them are
/// out of date.
pub(crate) fn changed(folders: &[PathBuf]) {
    let mut state = service().state.lock().unwrap();
    for folder in folders {
        for dir in folder.ancestors() {
            if let Some(k) = state.cache.get_mut(dir) {
                k.stale = true;
            }
        }
    }
}

fn worker(s: &'static Service) {
    tilde_contents::lower_priority();
    let mut state = s.state.lock().unwrap();
    loop {
        let now = SystemTime::now();
        let ready = state.queue.iter().position(|j| j.not_before <= now);
        let Some(i) = ready else {
            let wait = state.queue.iter().filter_map(|j| j.not_before.duration_since(now).ok()).min();
            state = match wait {
                Some(wait) => s.wake.wait_timeout(state, wait).unwrap().0,
                None => s.wake.wait(state).unwrap(),
            };
            continue;
        };
        let Job { path, index, .. } = state.queue.remove(i);
        let stop = Arc::new(AtomicBool::new(false));
        state.running.insert(path.clone(), stop.clone());
        drop(state);

        // Stops once no window shows it any more (`want` sets the flag).
        let started = Instant::now();
        let found = Mutex::new(Vec::new());
        let result = tilde_fs::folder_size(&path, KEEP_LEVELS, &|| stop.load(Ordering::Relaxed), &|p, n| {
            found.lock().unwrap().push((p.to_path_buf(), n))
        });
        let took = started.elapsed();
        let found = found.into_inner().unwrap();

        state = s.state.lock().unwrap();
        state.running.remove(&path);
        if result.is_err() {
            // Wanted again since it was stopped: measure it afresh.
            if is_wanted(&state.owners, &path) {
                state.queue.insert(0, Job { path, not_before: SystemTime::now(), index });
            }
            continue;
        }
        if state.cache.len() + found.len() > MAX_CACHED {
            let State { cache, owners, .. } = &mut *state;
            cache.retain(|p, _| is_wanted(owners, p));
        }
        let measured = SystemTime::now();
        for (p, size) in found {
            if p == path
                && let Some(index) = &index
            {
                index.set_skipped_size(&p, size);
            }
            state.cache.insert(p, Known { size, measured, took, stale: false });
        }
        for owner in state.owners.values() {
            if path.ancestors().any(|a| owner.wanted.contains(a)) {
                let _ = owner.tx.send(Command::SizesChanged);
            }
        }
        if state.file.is_some() && state.saved.is_none_or(|t| t.elapsed() >= SAVE_EVERY) {
            state.saved = Some(Instant::now());
            save(&state);
        }
    }
}

/// Saved as `{path: [size, measured (Unix seconds), took (ms), stale (0/1)]}`.
type Saved = HashMap<String, (u64, u64, u64, u8)>;

fn load(file: &Path) -> HashMap<PathBuf, Known> {
    let saved: Saved = std::fs::read(file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    saved
        .into_iter()
        .map(|(path, (size, measured, took, stale))| {
            let known = Known {
                size,
                measured: UNIX_EPOCH + Duration::from_secs(measured),
                took: Duration::from_millis(took),
                stale: stale != 0,
            };
            (PathBuf::from(path), known)
        })
        .collect()
}

fn save(state: &State) {
    let Some(file) = &state.file else { return };
    let saved: Saved = state
        .cache
        .iter()
        .map(|(p, k)| {
            let measured = k.measured.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
            (p.to_string_lossy().into_owned(), (k.size, measured, k.took.as_millis() as u64, k.stale as u8))
        })
        .collect();
    let Ok(bytes) = serde_json::to_vec(&saved) else { return };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let partial = file.with_extension("json.partial");
    if std::fs::write(&partial, bytes).is_ok() {
        let _ = std::fs::rename(partial, file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn measures_wanted_folders_and_reports_back() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        std::fs::create_dir_all(a.join("inner")).unwrap();
        std::fs::write(a.join("inner/f"), vec![0u8; 4096]).unwrap();
        let (tx, rx) = mpsc::channel();
        want(9_001, vec![a.clone()], tx.clone(), None);
        assert!(matches!(rx.recv_timeout(Duration::from_secs(5)), Ok(Command::SizesChanged)));
        assert_eq!(get(&a, None), Some(4096));
        assert_eq!(get(&a.join("inner"), None), Some(4096), "folders inside are remembered too");

        // Fresh: asking again measures nothing.
        want(9_001, vec![a.clone()], tx.clone(), None);
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());

        // A change marks it out of date (measured again after a pause).
        changed(&[a.join("inner")]);
        assert!(service().state.lock().unwrap().cache[&a].stale);
        want(9_001, Vec::new(), tx, None);
    }

    #[test]
    fn slow_folders_are_measured_again_less_often() {
        let now = SystemTime::now();
        let known = |took_secs: u64, age_secs: u64, stale: bool| Known {
            size: 1,
            measured: now - Duration::from_secs(age_secs),
            took: Duration::from_secs(took_secs),
            stale,
        };
        assert!(known(0, 60, false).due(now).is_none(), "fresh");
        assert_eq!(known(0, 3_600, false).due(now), Some(now), "old");
        assert!(known(60, 3_600, false).due(now).is_none(), "took a minute: fresh for 200 minutes");
        // After a change: 30 s at least, or 20 times as long as it took.
        assert_eq!(known(0, 0, true).due(now), Some(now + MIN_CHANGE_WAIT));
        assert_eq!(known(12, 0, true).due(now), Some(now + Duration::from_secs(240)));
    }

    #[test]
    fn sizes_are_saved_and_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sizes.json");
        let measured = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut state = State { file: Some(file.clone()), ..Default::default() };
        state.cache.insert("/x/a".into(), Known { size: 42, measured, took: Duration::from_millis(1500), stale: true });
        save(&state);
        let loaded = load(&file);
        let k = &loaded[Path::new("/x/a")];
        assert_eq!((k.size, k.measured, k.took, k.stale), (42, measured, Duration::from_millis(1500), true));
    }
}
