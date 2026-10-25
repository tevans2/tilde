//! Parallel filesystem scan built on the `getattrlistbulk` lister.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use tilde_fs::{ListOptions, list_dir};
use tilde_types::{Entry, EntryKind};

#[derive(Debug, Clone)]
pub struct ScanConfig {
    pub roots: Vec<PathBuf>,
    /// Folder names whose contents are skipped at any depth. The folder itself is still
    /// indexed, so you can find `node_modules` but not everything inside it.
    pub exclude_names: HashSet<String>,
    /// Absolute folders whose contents are skipped.
    pub exclude_paths: Vec<PathBuf>,
    pub include_hidden: bool,
}

impl ScanConfig {
    pub(crate) fn descends_into(&self, path: &Path, name: &str) -> bool {
        !self.exclude_names.contains(name) && !self.exclude_paths.iter().any(|p| p == path)
    }

    /// Whether the scan lists the contents of folder `dir`, so changes inside it matter.
    pub fn scans_inside(&self, dir: &Path) -> bool {
        if self.roots.iter().any(|r| r == dir) {
            return true;
        }
        let name = dir.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
        self.covers(dir) && self.descends_into(dir, &name)
    }

    /// Whether a scan reaches `path`. Used to add zoxide folders the scan would miss.
    pub fn covers(&self, path: &Path) -> bool {
        let Some(root) = self.roots.iter().find(|r| path.starts_with(r)) else { return false };
        let Ok(rel) = path.strip_prefix(root) else { return false };
        let parts: Vec<_> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        let mut current = root.clone();
        for (i, name) in parts.iter().enumerate() {
            if !self.include_hidden && name.starts_with('.') {
                return false;
            }
            current.push(name);
            let is_last = i + 1 == parts.len();
            if !is_last && !self.descends_into(&current, name) {
                return false;
            }
        }
        true
    }
}

/// Walks every root in parallel and calls `on_entry(folder, entry)` for each visible item.
/// Folders are entered unless excluded. Packages, symlinks and mount points (other disks,
/// network shares, container filesystems) are listed but never entered.
pub fn scan(config: &ScanConfig, stop: &AtomicBool, on_entry: &(dyn Fn(&Path, &Entry) + Sync)) {
    walk_parallel(config.roots.clone(), config.include_hidden, stop, &|dir, entries| {
        let mut next = Vec::new();
        for e in &entries {
            on_entry(dir, e);
            let child = dir.join(&e.name);
            if e.kind == EntryKind::Folder && !e.mount_point && config.descends_into(&child, &e.name) {
                next.push(child);
            }
        }
        next
    });
}

/// Called with each folder's listing; returns the subfolders to list next.
pub type OnListing<'a> = dyn Fn(&Path, Vec<Entry>) -> Vec<PathBuf> + Sync + 'a;

/// Lists `dirs` and, recursively, whatever `on_listing` returns, in parallel.
pub fn walk_parallel(dirs: Vec<PathBuf>, include_hidden: bool, stop: &AtomicBool, on_listing: &OnListing<'_>) {
    let opts = ListOptions { include_hidden, ..Default::default() };
    // Listing is I/O-bound; a few more threads than cores keeps the disk busy without
    // starving the matcher's own pool.
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 8);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|i| format!("tilde-scan-{i}"))
        .build()
        .expect("scan thread pool");
    pool.scope(|s| {
        for dir in dirs {
            let opts = &opts;
            s.spawn(move |s| walk(dir, s, opts, stop, on_listing));
        }
    });
}

fn walk<'s>(
    dir: PathBuf,
    s: &rayon::Scope<'s>,
    opts: &'s ListOptions,
    stop: &'s AtomicBool,
    on_listing: &'s OnListing<'s>,
) {
    if stop.load(Ordering::Relaxed) {
        return;
    }
    let Ok(entries) = list_dir(&dir, opts, || stop.load(Ordering::Relaxed)) else { return };
    for child in on_listing(&dir, entries) {
        s.spawn(move |s| walk(child, s, opts, stop, on_listing));
    }
}

/// Like `walk_parallel` but on the calling thread, for small incremental updates.
pub fn walk_sequential(dirs: Vec<PathBuf>, include_hidden: bool, on_listing: &OnListing<'_>) {
    let opts = ListOptions { include_hidden, ..Default::default() };
    let mut stack = dirs;
    while let Some(dir) = stack.pop() {
        if let Ok(entries) = list_dir(&dir, &opts, || false) {
            stack.extend(on_listing(&dir, entries));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Mutex;

    fn config(root: &Path) -> ScanConfig {
        ScanConfig {
            roots: vec![root.to_path_buf()],
            exclude_names: ["node_modules".to_string()].into(),
            exclude_paths: vec![root.join("Library")],
            include_hidden: false,
        }
    }

    #[test]
    fn scans_recursively_respecting_exclusions() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        for d in ["a/b", "node_modules/pkg", "Library/Caches", ".hidden", "App.app/Contents"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        for f in [
            "a/b/deep.txt",
            "a/top.txt",
            "node_modules/pkg/index.js",
            "Library/Caches/c",
            ".hidden/x",
            "App.app/Contents/y",
        ] {
            fs::write(root.join(f), b"").unwrap();
        }

        let seen = Mutex::new(Vec::new());
        scan(&config(&root), &AtomicBool::new(false), &|dir, e| {
            let rel = dir.join(&e.name).strip_prefix(&root).unwrap().to_string_lossy().into_owned();
            seen.lock().unwrap().push(rel);
        });
        let mut seen = seen.into_inner().unwrap();
        seen.sort();
        assert_eq!(seen, ["App.app", "Library", "a", "a/b", "a/b/deep.txt", "a/top.txt", "node_modules"]);
    }

    #[test]
    fn scans_inside_follows_exclusions() {
        let root = PathBuf::from("/Users/me");
        let c = config(&root);
        assert!(c.scans_inside(&root));
        assert!(c.scans_inside(Path::new("/Users/me/Developer")));
        assert!(!c.scans_inside(Path::new("/Users/me/node_modules")), "listed, but its contents aren't");
        assert!(!c.scans_inside(Path::new("/Users/me/Library")));
        assert!(!c.scans_inside(Path::new("/Users/me/.git")));
    }

    #[test]
    fn covers_matches_what_scan_reaches() {
        let root = PathBuf::from("/Users/me");
        let c = config(&root);
        assert!(c.covers(Path::new("/Users/me/Developer/tilde")));
        assert!(c.covers(Path::new("/Users/me/node_modules")), "the excluded folder itself is indexed");
        assert!(!c.covers(Path::new("/Users/me/node_modules/pkg")));
        assert!(!c.covers(Path::new("/Users/me/Library/Caches")));
        assert!(!c.covers(Path::new("/Users/me/.config/nvim")));
        assert!(!c.covers(Path::new("/opt/homebrew")));
    }
}
