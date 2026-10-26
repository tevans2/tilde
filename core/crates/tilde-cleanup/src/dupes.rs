//! Duplicate files, found in stages so results come quickly and cheaply:
//!
//! 1. sizes (from the index): files with a size nobody else has can't be duplicates;
//! 2. samples: 16 KB from the start, middle and end of each remaining file, read in
//!    parallel. Groups that still match are shown straight away as likely duplicates;
//! 3. full contents, hashed in the background biggest groups first, so most of the space
//!    is confirmed early.
//!
//! Hashes are cached by file identity, size and modification time, so later scans only
//! read new or changed files. Hard links (the same file twice) and APFS clones (which
//! share their blocks, so trashing one frees nothing) aren't counted as space to free.
//! Reads bypass the file cache so other apps keep theirs. Before anything is trashed, the
//! app compares the two files byte for byte (`same_contents`).

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::os::unix::fs::FileExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use xxhash_rust::xxh3::{Xxh3, xxh3_128};

use crate::fsinfo::{Stat, freed_by, stat};

const SAMPLE: usize = 16 * 1024;
const CHUNK: usize = 1 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DupFile {
    pub path: PathBuf,
    /// Space trashing this copy would free.
    pub freed: u64,
    pub modified: i64,
    pub created: i64,
}

/// Files with the same contents (likely, or confirmed in full).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub size: u64,
    /// `files[keep]` is the copy suggested to keep.
    pub files: Vec<DupFile>,
    pub keep: usize,
    pub confirmed: bool,
}

impl Group {
    /// Space freed by trashing every copy but the kept one.
    pub fn reclaimable(&self) -> u64 {
        self.files.iter().enumerate().filter(|(i, _)| *i != self.keep).map(|(_, f)| f.freed).sum()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// Bytes read so far and in all (full-content stage).
    pub done: u64,
    pub total: u64,
}

/// Hashes from earlier scans.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Cache {
    entries: HashMap<String, CacheEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    size: u64,
    mtime_s: i64,
    mtime_ns: u32,
    sample: String,
    full: Option<String>,
}

impl Cache {
    pub fn load(path: &Path) -> Cache {
        std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    /// Keeps only the files seen in this scan, so it doesn't grow forever.
    pub fn save(&self, path: &Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string(self) {
            let tmp = path.with_extension("tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
        }
    }

    fn key(s: &Stat) -> String {
        format!("{}:{}", s.dev, s.ino)
    }

    fn get(&self, s: &Stat) -> Option<&CacheEntry> {
        let (secs, nanos) = split_ns(s.modified_ns);
        self.entries.get(&Self::key(s)).filter(|e| e.size == s.size && e.mtime_s == secs && e.mtime_ns == nanos)
    }
}

fn split_ns(ns: i128) -> (i64, u32) {
    (ns.div_euclid(1_000_000_000) as i64, ns.rem_euclid(1_000_000_000) as u32)
}

struct Candidate {
    path: PathBuf,
    stat: Stat,
}

/// Candidates with their full hash, once known.
type Hashed = Vec<(Candidate, Option<u128>)>;

/// Finds duplicates among `paths` (files at least `min` bytes; the index has their
/// sizes, but each is checked on disk). `on_groups` gets the groups as they stand: once
/// with likely groups after sampling, then repeatedly as full checks confirm them
/// (biggest first). Returns the confirmed groups. `cache` is used and updated.
pub fn find(
    paths: Vec<PathBuf>,
    min: u64,
    cache: &mut Cache,
    cancel: &AtomicBool,
    on_groups: &(dyn Fn(&[Group]) + Sync),
    on_progress: &(dyn Fn(Progress) + Sync),
) -> Vec<Group> {
    // Stage 1: same size, real files, the same file once.
    let mut seen = std::collections::HashSet::new();
    let mut by_size: HashMap<u64, Vec<Candidate>> = HashMap::new();
    let stats: Vec<Candidate> = paths
        .into_par_iter()
        .filter_map(|path| {
            let s = stat(&path)?;
            (s.is_file && !s.dataless && s.size >= min).then_some(Candidate { path, stat: s })
        })
        .collect();
    for c in stats {
        if seen.insert((c.stat.dev, c.stat.ino)) {
            by_size.entry(c.stat.size).or_default().push(c);
        }
    }
    let candidates: Vec<Candidate> = by_size.into_values().filter(|v| v.len() > 1).flatten().collect();

    // Stage 2: samples.
    let cache_view: &Cache = cache;
    let sampled: Vec<(Candidate, u128, Option<u128>)> = candidates
        .into_par_iter()
        .filter_map(|c| {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            if let Some(e) = cache_view.get(&c.stat) {
                let sample = u128::from_str_radix(&e.sample, 16).ok()?;
                let full = e.full.as_deref().and_then(|f| u128::from_str_radix(f, 16).ok());
                return Some((c, sample, full));
            }
            let sample = sample_hash(&c.path, c.stat.size).ok()?;
            Some((c, sample, None))
        })
        .collect();
    let mut by_sample: HashMap<(u64, u128), Hashed> = HashMap::new();
    for (c, sample, full) in sampled {
        by_sample.entry((c.stat.size, sample)).or_default().push((c, full));
    }
    let mut new_cache: HashMap<String, CacheEntry> = HashMap::new();
    let mut pending: Vec<(u64, Hashed)> = Vec::new();
    for ((size, sample), members) in by_sample {
        for (c, full) in &members {
            let (secs, nanos) = split_ns(c.stat.modified_ns);
            new_cache.insert(
                Cache::key(&c.stat),
                CacheEntry {
                    size,
                    mtime_s: secs,
                    mtime_ns: nanos,
                    sample: format!("{sample:x}"),
                    full: full.map(|f| format!("{f:x}")),
                },
            );
        }
        if members.len() > 1 {
            pending.push((size, members));
        }
    }
    // Working out what each copy would free asks the file system about shared blocks,
    // which is slow per file: in parallel.
    let mut likely: Vec<(Group, Hashed)> = pending
        .into_par_iter()
        .map(|(size, members)| (group_of(size, members.iter().map(|(c, _)| c), false), members))
        .filter(|(group, _)| group.reclaimable() >= min / 2)
        .collect();
    likely.sort_by_key(|(g, _)| std::cmp::Reverse(g.reclaimable()));
    let mut current: Vec<Group> = likely.iter().map(|(g, _)| g.clone()).collect();
    on_groups(&current);

    // Stage 3: full contents, biggest first.
    let total: u64 = likely
        .iter()
        .map(|(g, m)| if m.iter().all(|(_, f)| f.is_some()) { 0 } else { g.size * g.files.len() as u64 })
        .sum();
    let done = AtomicU64::new(0);
    on_progress(Progress { done: 0, total });
    let last_report = Mutex::new(Instant::now());
    let mut confirmed: Vec<Group> = Vec::new();
    // A few groups at a time (most have two files), still biggest first, so the disk has
    // several reads in flight.
    let mut batches = Vec::new();
    let mut batch = Vec::new();
    for item in likely {
        batch.push(item);
        if batch.len() == 8 {
            batches.push(std::mem::take(&mut batch));
        }
    }
    if !batch.is_empty() {
        batches.push(batch);
    }
    for batch in batches {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let hashed: Vec<(Group, Hashed)> = batch
            .into_par_iter()
            .map(|(group, members)| {
                let members = members
                    .into_par_iter()
                    .map(|(c, full)| {
                        if full.is_some() || cancel.load(Ordering::Relaxed) {
                            return (c, full);
                        }
                        let h = full_hash(&c.path, cancel, &|n| {
                            let d = done.fetch_add(n, Ordering::Relaxed) + n;
                            let mut last = last_report.lock().unwrap();
                            if last.elapsed() > Duration::from_millis(150) {
                                *last = Instant::now();
                                on_progress(Progress { done: d, total });
                            }
                        })
                        .ok();
                        (c, h)
                    })
                    .collect();
                (group, members)
            })
            .collect();
        for (group, members) in hashed {
            let mut by_full: HashMap<u128, Vec<&Candidate>> = HashMap::new();
            for (c, full) in &members {
                if let Some(f) = full {
                    by_full.entry(*f).or_default().push(c);
                    if let Some(e) = new_cache.get_mut(&Cache::key(&c.stat)) {
                        e.full = Some(format!("{f:x}"));
                    }
                }
            }
            let mut found: Vec<Group> = by_full
                .into_values()
                .filter(|v| v.len() > 1)
                .map(|v| group_of(group.size, v.into_iter(), true))
                .filter(|g| g.reclaimable() >= min / 2)
                .collect();
            confirmed.extend(found.iter().cloned());
            // Replace the likely group with what it turned out to be.
            current.retain(|g| g != &group);
            current.append(&mut found);
        }
        current.sort_by_key(|g| std::cmp::Reverse(g.reclaimable()));
        on_groups(&current);
    }
    on_progress(Progress { done: total, total });
    on_groups(&current);
    cache.entries = new_cache;
    confirmed.sort_by_key(|g| std::cmp::Reverse(g.reclaimable()));
    confirmed
}

fn group_of<'a>(size: u64, members: impl Iterator<Item = &'a Candidate>, confirmed: bool) -> Group {
    let files: Vec<DupFile> = members
        .map(|c| DupFile {
            path: c.path.clone(),
            freed: freed_by(&c.path, &c.stat),
            modified: c.stat.modified,
            created: c.stat.created,
        })
        .collect();
    let keep = choose_keep(&files);
    Group { size, files, keep, confirmed }
}

/// The copy worth keeping: not one in Downloads, on the Desktop or in a cache; then the
/// oldest; then the one with the shortest path.
fn choose_keep(files: &[DupFile]) -> usize {
    let transient = |p: &Path| {
        let s = p.to_string_lossy();
        ["/Downloads/", "/Desktop/", "/.Trash/", "/Caches/", "/tmp/"].iter().any(|t| s.contains(t))
    };
    files
        .iter()
        .enumerate()
        .min_by_key(|(_, f)| (transient(&f.path), f.created, f.path.as_os_str().len()))
        .map(|(i, _)| i)
        .unwrap_or(0)
}

/// Opens for reading without filling the file cache.
fn open_uncached(path: &Path) -> std::io::Result<File> {
    let f = File::open(path)?;
    unsafe { libc::fcntl(f.as_raw_fd(), libc::F_NOCACHE, 1) };
    Ok(f)
}

/// The size and 16 KB from the start, middle and end.
fn sample_hash(path: &Path, size: u64) -> std::io::Result<u128> {
    let f = open_uncached(path)?;
    let mut buf = Vec::with_capacity(SAMPLE * 3 + 8);
    buf.extend_from_slice(&size.to_le_bytes());
    let mut chunk = vec![0u8; SAMPLE];
    let offsets = [0, size.saturating_sub(SAMPLE as u64) / 2, size.saturating_sub(SAMPLE as u64)];
    for off in offsets {
        let n = f.read_at(&mut chunk, off)?;
        buf.extend_from_slice(&chunk[..n]);
    }
    Ok(xxh3_128(&buf))
}

fn full_hash(path: &Path, cancel: &AtomicBool, progress: &dyn Fn(u64)) -> std::io::Result<u128> {
    let mut f = open_uncached(path)?;
    let mut hasher = Xxh3::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(std::io::Error::other("cancelled"));
        }
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        progress(n as u64);
    }
    Ok(hasher.digest128())
}

/// Whether two files have exactly the same bytes (read fresh, stopping at the first
/// difference). Checked before a duplicate is trashed.
pub fn same_contents(a: &Path, b: &Path) -> std::io::Result<bool> {
    let (ma, mb) = (a.symlink_metadata()?, b.symlink_metadata()?);
    if ma.len() != mb.len() || !ma.is_file() || !mb.is_file() {
        return Ok(false);
    }
    let (mut fa, mut fb) = (open_uncached(a)?, open_uncached(b)?);
    let (mut ba, mut bb) = (vec![0u8; CHUNK], vec![0u8; CHUNK]);
    loop {
        let n = read_full(&mut fa, &mut ba)?;
        let m = read_full(&mut fb, &mut bb)?;
        if n != m || ba[..n] != bb[..m] {
            return Ok(false);
        }
        if n == 0 {
            return Ok(true);
        }
    }
}

fn read_full(f: &mut File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match f.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}
