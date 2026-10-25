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
    fn descends_into(&self, path: &Path, name: &str) -> bool {
        !self.exclude_names.contains(name) && !self.exclude_paths.iter().any(|p| p == path)
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
    let opts = ListOptions { include_hidden: config.include_hidden, ..Default::default() };
    // Listing is I/O-bound; a few more threads than cores keeps the disk busy without
    // starving the matcher's own pool.
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 8);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|i| format!("tilde-scan-{i}"))
        .build()
        .expect("scan thread pool");
    pool.scope(|s| {
        for root in &config.roots {
            let root = root.clone();
            let opts = &opts;
            s.spawn(move |s| walk(root, s, config, opts, stop, on_entry));
        }
    });
}

fn walk<'s>(
    dir: PathBuf,
    s: &rayon::Scope<'s>,
    config: &'s ScanConfig,
    opts: &'s ListOptions,
    stop: &'s AtomicBool,
    on_entry: &'s (dyn Fn(&Path, &Entry) + Sync),
) {
    if stop.load(Ordering::Relaxed) {
        return;
    }
    let Ok(entries) = list_dir(&dir, opts, || stop.load(Ordering::Relaxed)) else { return };
    for e in &entries {
        on_entry(&dir, e);
        if e.kind == EntryKind::Folder && !e.mount_point {
            let child = dir.join(&e.name);
            if config.descends_into(&child, &e.name) {
                s.spawn(move |s| walk(child, s, config, opts, stop, on_entry));
            }
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
