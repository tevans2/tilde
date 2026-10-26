//! Tilde's own frecency: how often and how recently each folder and file is used, for
//! ranking search results and listing favourite folders.
//!
//! Each path keeps one score that halves every `HALF_LIFE` without use; a use adds to it
//! (an open, a visit, a pick in the folder picker). Exponential decay needs no aging
//! passes, and a recent burst outranks a habit you've dropped.
//!
//! Folder visits are weighed by how long you stayed: walking through `~/Developer` on the
//! way to `~/Developer/tilde` barely counts for `~/Developer`. A visit is held until the
//! next one (or a flush) settles its weight.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The score halves after this many seconds without use.
pub const HALF_LIFE: f64 = 10.0 * 86_400.0;
/// A folder left sooner than this was passed through.
const PASS_THROUGH_SECS: f64 = 2.0;
const PASS_THROUGH_WEIGHT: f64 = 0.15;
/// Scores below this are forgotten when saving.
const FORGET_BELOW: f64 = 0.02;
/// Most entries kept (the weakest go first).
const MAX_ENTRIES: usize = 5000;

/// How a path was used, and how much that counts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Use {
    /// Opened a folder in the browser (weighed by how long you stayed).
    Visit,
    /// Opened a file (in its app, or picked in a dialog).
    Open,
    /// Picked as a move or copy destination.
    Destination,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct Entry {
    /// The score as of `at`.
    score: f64,
    /// Seconds since the epoch.
    at: i64,
    dir: bool,
}

impl Entry {
    fn score_at(&self, now: i64) -> f64 {
        decay(self.score, now - self.at)
    }
}

fn decay(score: f64, elapsed: i64) -> f64 {
    score * (-(elapsed.max(0) as f64) * std::f64::consts::LN_2 / HALF_LIFE).exp()
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Saved {
    version: u32,
    entries: HashMap<String, Entry>,
}

#[derive(Debug, Default)]
pub struct Frecency {
    entries: HashMap<String, Entry>,
    /// The last folder visited, until the next visit says how long it was open.
    pending: Option<(PathBuf, f64)>,
    /// Bumped on every change, so readers know to refresh what they derived.
    version: u64,
    path: Option<PathBuf>,
    dirty: bool,
}

impl Frecency {
    /// Kept in memory only (tests).
    pub fn in_memory() -> Frecency {
        Frecency::default()
    }

    /// Loads `path` (empty if it doesn't exist yet).
    pub fn load(path: &Path) -> Frecency {
        let mut f = Frecency { path: Some(path.to_path_buf()), ..Frecency::default() };
        if let Ok(saved) = std::fs::read(path).map(|b| serde_json::from_slice::<Saved>(&b)) {
            f.entries = saved.map(|s| s.entries).unwrap_or_default();
        }
        f
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    /// Records a use. `now` is in seconds, with a fraction (for how long a visit lasted).
    pub fn record(&mut self, path: &Path, how: Use, now: f64, dir: bool) {
        match how {
            Use::Visit => {
                self.settle(now);
                self.pending = Some((path.to_path_buf(), now));
            }
            Use::Open => self.add(path, 1.0, now as i64, dir),
            Use::Destination => self.add(path, 0.5, now as i64, dir),
        }
    }

    /// Settles the folder visit in progress (its weight by how long it's been open).
    pub fn settle(&mut self, now: f64) {
        if let Some((path, since)) = self.pending.take() {
            let weight = if now - since < PASS_THROUGH_SECS { PASS_THROUGH_WEIGHT } else { 1.0 };
            self.add(&path, weight, now as i64, true);
        }
    }

    fn add(&mut self, path: &Path, weight: f64, now: i64, dir: bool) {
        let key = path.to_string_lossy().into_owned();
        let e = self.entries.entry(key).or_insert(Entry { score: 0.0, at: now, dir });
        e.score = e.score_at(now) + weight;
        e.at = now;
        e.dir = dir;
        self.version += 1;
        self.dirty = true;
    }

    /// The score of every path, as of `now` (absolute paths).
    pub fn scores(&self, now: i64) -> impl Iterator<Item = (&str, f64)> {
        self.entries.iter().map(move |(k, e)| (k.as_str(), e.score_at(now)))
    }

    /// Folders, best first, that still exist.
    pub fn top_folders(&self, now: i64, limit: usize) -> Vec<PathBuf> {
        let mut dirs: Vec<(&String, f64)> =
            self.entries.iter().filter(|(_, e)| e.dir).map(|(k, e)| (k, e.score_at(now))).collect();
        dirs.sort_by(|a, b| b.1.total_cmp(&a.1));
        dirs.into_iter().map(|(k, _)| PathBuf::from(k)).filter(|p| p.is_dir()).take(limit).collect()
    }

    /// Every folder known, for indexing folders the scan doesn't reach.
    pub fn folders(&self) -> Vec<PathBuf> {
        self.entries.iter().filter(|(_, e)| e.dir).map(|(k, _)| PathBuf::from(k)).collect()
    }

    /// Writes the file if anything changed, dropping what has faded away.
    pub fn save(&mut self, now: i64) -> std::io::Result<()> {
        let Some(path) = self.path.clone() else { return Ok(()) };
        if !self.dirty {
            return Ok(());
        }
        self.entries.retain(|_, e| e.score_at(now) >= FORGET_BELOW);
        if self.entries.len() > MAX_ENTRIES {
            let mut scores: Vec<f64> = self.entries.values().map(|e| e.score_at(now)).collect();
            scores.sort_by(|a, b| b.total_cmp(a));
            let cut = scores[MAX_ENTRIES - 1];
            self.entries.retain(|_, e| e.score_at(now) >= cut);
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let saved = Saved { version: 1, entries: self.entries.clone() };
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec(&saved)?)?;
        std::fs::rename(&tmp, &path)?;
        self.dirty = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: f64 = 86_400.0;

    fn score(f: &Frecency, path: &str, now: f64) -> f64 {
        f.scores(now as i64).find(|(k, _)| *k == path).map_or(0.0, |(_, s)| s)
    }

    #[test]
    fn visits_count_by_time_spent() {
        let mut f = Frecency::in_memory();
        let t = 1_000_000.0;
        // Through ~/Developer to ~/Developer/tilde, then stay.
        f.record(Path::new("/h/Developer"), Use::Visit, t, true);
        f.record(Path::new("/h/Developer/tilde"), Use::Visit, t + 0.5, true);
        f.settle(t + 60.0);
        assert!((score(&f, "/h/Developer", t + 60.0) - PASS_THROUGH_WEIGHT).abs() < 1e-3);
        assert!((score(&f, "/h/Developer/tilde", t + 60.0) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn scores_halve_and_recent_use_wins() {
        let mut f = Frecency::in_memory();
        let t = 1_000_000.0;
        for i in 0..8 {
            f.record(Path::new("/old"), Use::Open, t + i as f64, false);
        }
        let later = t + 40.0 * DAY;
        for i in 0..2 {
            f.record(Path::new("/new"), Use::Open, later + i as f64, false);
        }
        // 8 uses 40 days (4 half-lives) ago are worth 0.5 now; 2 today are worth 2.
        assert!((score(&f, "/old", later) - 0.5).abs() < 0.01);
        assert!(score(&f, "/new", later) > score(&f, "/old", later));
    }

    #[test]
    fn saves_loads_and_forgets() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("frecency.json");
        let now = 2_000_000;
        let kept = dir.path().join("kept");
        std::fs::create_dir(&kept).unwrap();
        let mut f = Frecency::load(&file);
        for i in 0..6 {
            f.record(&kept, Use::Open, (now + i) as f64, true);
        }
        f.record(Path::new("/gone"), Use::Open, now as f64, true);
        assert_eq!(f.top_folders(now, 10), std::slice::from_ref(&kept), "missing folders aren't listed");
        f.record(Path::new("/faded"), Use::Destination, now as f64, true);
        f.save(now).unwrap();

        // Faded entries are dropped on save.
        let mut g = Frecency::load(&file);
        assert!(score(&g, "/faded", now as f64) > 0.4);
        let much_later = now + (HALF_LIFE * 5.0) as i64;
        g.dirty = true;
        g.save(much_later).unwrap();
        let h = Frecency::load(&file);
        assert!(h.scores(much_later).all(|(k, _)| k != "/faded"));
        assert!(h.scores(much_later).any(|(k, _)| k == kept.to_str().unwrap()), "6 → 0.19, still above the floor");
    }
}
