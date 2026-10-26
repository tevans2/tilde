//! The Clean Up page: suggestions for freeing space, each with the space it would free.
//! Categories are worked out from the search index where it can (big files, project
//! folders) and by measuring folders the index skips (build output, caches). Nothing here
//! deletes anything: the app moves what's chosen to the Trash through the operations
//! queue, so it can be undone, except emptying the Trash itself.

pub mod dupes;
mod fsinfo;

pub use fsinfo::{Stat, freed_by, last_opened, measure, stat};

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use tilde_types::{EntryKind, with_extension};

/// What a suggestion is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    BuildOutput,
    BigFiles,
    OldDownloads,
    Duplicates,
    AppCaches,
    Trash,
}

/// One suggestion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub path: PathBuf,
    /// Space it would free.
    pub bytes: u64,
    /// Logical size and modification time when found, to check nothing changed before
    /// acting.
    pub size: u64,
    pub modified: i64,
    pub last_opened: Option<i64>,
    pub is_dir: bool,
    /// Items listed together: a project, an app's cache, a set of duplicates.
    pub group: Option<String>,
    /// Why it's suggested ("installer", "from example.com", "node_modules").
    pub detail: String,
    /// Duplicates: the copy to keep.
    pub keep: bool,
    /// Duplicates: the contents have been compared in full (not just sampled).
    pub confirmed: bool,
}

/// The rules, from the config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rules {
    pub big_file: u64,
    pub big_file_age: i64,
    pub download_age: i64,
    pub project_idle: i64,
    pub duplicates_min: u64,
    pub build_folders: Vec<String>,
    pub ignore: Ignore,
}

/// `[cleanup] ignore`: paths (and everything inside), names, and `*` / `?` patterns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ignore {
    paths: Vec<PathBuf>,
    names: Vec<String>,
}

impl Ignore {
    pub fn new(entries: &[String], home: &Path) -> Ignore {
        let mut ignore = Ignore::default();
        for e in entries {
            if let Some(rest) = e.strip_prefix("~/") {
                ignore.paths.push(home.join(rest));
            } else if e.starts_with('/') {
                ignore.paths.push(PathBuf::from(e));
            } else {
                ignore.names.push(e.clone());
            }
        }
        ignore
    }

    pub fn matches(&self, path: &Path) -> bool {
        if self.paths.iter().any(|p| path.starts_with(p)) {
            return true;
        }
        path.components().any(|c| {
            let name = c.as_os_str().to_string_lossy();
            self.names.iter().any(|n| glob(n, &name))
        })
    }
}

/// `*` (any run) and `?` (one character) matching, case-insensitively.
fn glob(pattern: &str, name: &str) -> bool {
    fn go(p: &[char], n: &[char]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (Some('*'), _) => go(&p[1..], n) || (!n.is_empty() && go(p, &n[1..])),
            (Some('?'), Some(_)) => go(&p[1..], &n[1..]),
            (Some(a), Some(b)) => a.eq_ignore_ascii_case(b) && go(&p[1..], &n[1..]),
            _ => false,
        }
    }
    go(&pattern.chars().collect::<Vec<_>>(), &name.chars().collect::<Vec<_>>())
}

/// Inside something that should be handled as a whole and never picked apart: app
/// bundles, Photos and Music libraries, packages in general, and hidden folders.
fn inside_package_or_hidden(path: &Path, home: &Path) -> bool {
    let rel = path.strip_prefix(home).unwrap_or(path);
    let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    let folders = &parts[..parts.len().saturating_sub(1)];
    parts.iter().any(|p| p.starts_with('.')) || folders.iter().any(|p| tilde_fs::is_package_name(p))
}

/// An indexed item, as the rules see it.
pub type Indexed = (PathBuf, IndexedMeta);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexedMeta {
    pub kind: EntryKind,
    pub size: u64,
    pub modified: i64,
}

/// Big files that haven't been opened or changed for a while.
pub fn big_files(index: &[Indexed], rules: &Rules, home: &Path, now: i64) -> Vec<Item> {
    let cutoff = now - rules.big_file_age;
    let mut out = Vec::new();
    for (path, meta) in index {
        if meta.kind != EntryKind::File || meta.size < rules.big_file || meta.modified > cutoff {
            continue;
        }
        if inside_package_or_hidden(path, home) || rules.ignore.matches(path) {
            continue;
        }
        let Some(s) = stat(path) else { continue };
        if s.dataless || !s.is_file {
            continue;
        }
        let opened = last_opened(path);
        if opened.is_some_and(|t| t > cutoff) {
            continue;
        }
        let bytes = freed_by(path, &s);
        if bytes < rules.big_file / 2 {
            continue; // Mostly shared with a clone, or sparse: little to gain.
        }
        out.push(Item {
            path: path.clone(),
            bytes,
            size: s.size,
            modified: s.modified,
            last_opened: opened,
            is_dir: false,
            group: None,
            detail: String::new(),
            keep: false,
            confirmed: true,
        });
    }
    sort_biggest_first(&mut out);
    out
}

const INSTALLERS: &[&str] = &["dmg", "pkg", "mpkg", "iso", "xip"];

/// What's been sitting in the inbox folders (Downloads, the Desktop) for a while.
pub fn old_downloads(inbox: &[PathBuf], rules: &Rules, now: i64, cancel: &AtomicBool) -> Vec<Item> {
    let cutoff = now - rules.download_age;
    let mut out = Vec::new();
    for folder in inbox {
        let Ok(entries) = tilde_fs::list_dir(folder, &tilde_fs::ListOptions::default(), || false) else { continue };
        for e in entries {
            let path = folder.join(&e.name);
            let arrived = if e.added > 0 { e.added } else { e.modified };
            if arrived > cutoff || rules.ignore.matches(&path) {
                continue;
            }
            let Some(s) = stat(&path) else { continue };
            if s.dataless {
                continue;
            }
            let bytes = if s.is_dir { measure(&path, cancel).0 } else { freed_by(&path, &s) };
            let (origin, site) = tilde_activity::detect(&path, s.created, s.modified);
            let installer = !s.is_dir && with_extension(&e.name, |x| INSTALLERS.contains(&x));
            let mut detail = Vec::new();
            if installer {
                detail.push("installer".to_string());
            }
            if let Some(site) = site {
                detail.push(format!("from {site}"));
            } else if origin == tilde_activity::Origin::AirDrop {
                detail.push("AirDrop".into());
            }
            out.push(Item {
                path,
                bytes,
                size: s.size,
                modified: s.modified,
                last_opened: last_opened(&folder.join(&e.name)),
                is_dir: s.is_dir,
                group: None,
                detail: detail.join(" · "),
                keep: false,
                confirmed: true,
            });
        }
    }
    sort_biggest_first(&mut out);
    out
}

/// What's in the Trash, item by item.
pub fn trash(home: &Path, cancel: &AtomicBool) -> Vec<Item> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(home.join(".Trash")) else { return out };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.file_name().is_some_and(|n| n == ".DS_Store") {
            continue;
        }
        let Some(s) = stat(&path) else { continue };
        out.push(Item {
            bytes: measure(&path, cancel).0,
            size: s.size,
            modified: s.modified,
            last_opened: None,
            is_dir: s.is_dir,
            group: None,
            detail: String::new(),
            keep: false,
            confirmed: true,
            path,
        });
    }
    sort_biggest_first(&mut out);
    out
}

/// Empties the Trash for good (files trashed from other volumes stay in those volumes'
/// own trash). Returns how much was freed; stops at the first item it can't remove.
pub fn empty_trash(home: &Path) -> std::io::Result<u64> {
    let cancel = AtomicBool::new(false);
    let mut freed = 0;
    for entry in std::fs::read_dir(home.join(".Trash"))?.flatten() {
        let path = entry.path();
        let (bytes, _) = measure(&path, &cancel);
        let meta = path.symlink_metadata()?;
        if meta.is_dir() {
            std::fs::remove_dir_all(&path)?
        } else {
            std::fs::remove_file(&path)?
        }
        freed += bytes;
    }
    Ok(freed)
}

/// Files that mark a project's top folder.
const PROJECT_MARKERS: &[&str] = &[
    "package.json",
    "Cargo.toml",
    "pyproject.toml",
    "requirements.txt",
    "setup.py",
    "Gemfile",
    "go.mod",
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "Podfile",
    "Package.swift",
    "pom.xml",
    "composer.json",
];

/// A project whose build output could go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdleProject {
    pub dir: PathBuf,
    /// The newest change to anything indexed in it.
    pub last_change: i64,
    /// Its build folders that exist.
    pub build: Vec<PathBuf>,
}

/// Projects (found from marker files in the index) where nothing has changed for
/// `project_idle`, with their build folders. Measuring those is left to the caller, since
/// it can take a while.
pub fn idle_projects(index: &[Indexed], rules: &Rules, home: &Path, now: i64) -> Vec<IdleProject> {
    let projects: HashSet<PathBuf> = index.iter().filter_map(|(path, meta)| project_of(path, meta.kind)).collect();
    let mut newest = HashMap::new();
    for (path, meta) in index {
        note_change(&projects, &mut newest, home, path, meta.modified);
    }
    idle_projects_from(&projects, &newest, rules, home, now)
}

/// The project folder `path` marks, if it's a marker file (`Cargo.toml`, `package.json`,
/// an `.xcodeproj`, …).
pub fn project_of(path: &Path, kind: EntryKind) -> Option<PathBuf> {
    let name = path.file_name()?.to_string_lossy();
    let marker = kind == EntryKind::File && PROJECT_MARKERS.contains(&name.as_ref());
    let xcode = name.ends_with(".xcodeproj") || name.ends_with(".xcworkspace");
    if marker || xcode { path.parent().map(Path::to_path_buf) } else { None }
}

/// Records a change at `path` (modified at `modified`) against the projects it's inside
/// (build folders' contents aren't indexed, so they don't count).
pub fn note_change(
    projects: &HashSet<PathBuf>,
    newest: &mut HashMap<PathBuf, i64>,
    home: &Path,
    path: &Path,
    modified: i64,
) {
    for ancestor in path.ancestors().skip(1) {
        if ancestor == home || ancestor.as_os_str().len() < home.as_os_str().len() {
            break;
        }
        if projects.contains(ancestor) {
            let e = newest.entry(ancestor.to_path_buf()).or_insert(i64::MIN);
            *e = (*e).max(modified);
        }
    }
}

/// `idle_projects` from projects and their newest changes, gathered by the caller.
pub fn idle_projects_from(
    projects: &HashSet<PathBuf>,
    newest: &HashMap<PathBuf, i64>,
    rules: &Rules,
    home: &Path,
    now: i64,
) -> Vec<IdleProject> {
    let cutoff = now - rules.project_idle;
    let mut out: Vec<IdleProject> = projects
        .iter()
        .filter(|dir| !rules.ignore.matches(dir) && !inside_package_or_hidden(dir, home))
        .filter_map(|dir| {
            let last_change = newest.get(dir.as_path()).copied().unwrap_or(i64::MIN);
            if last_change > cutoff {
                return None;
            }
            let build: Vec<PathBuf> = rules
                .build_folders
                .iter()
                .map(|name| dir.join(name))
                .filter(|p| p.symlink_metadata().is_ok_and(|m| m.is_dir()) && !rules.ignore.matches(p))
                .collect();
            (!build.is_empty()).then(|| IdleProject { dir: dir.clone(), last_change, build })
        })
        .collect();
    // A project inside another project's build folder (a dependency) belongs to it.
    let all: Vec<PathBuf> = out.iter().flat_map(|p| p.build.clone()).collect();
    out.retain(|p| !all.iter().any(|b| p.dir.starts_with(b)));
    out.sort_by(|a, b| a.dir.cmp(&b.dir));
    out
}

/// App caches in `~/Library/Caches`, one item per app (Apple's own and Tilde's are left
/// alone). Measured here, so call it off the main thread.
pub fn app_caches(home: &Path, rules: &Rules, cancel: &AtomicBool) -> Vec<Item> {
    let caches = home.join("Library/Caches");
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&caches) else { return out };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("com.apple.") || name == "Tilde" || name.starts_with('.') || rules.ignore.matches(&path) {
            continue;
        }
        let Some(s) = stat(&path) else { continue };
        let (bytes, _) = measure(&path, cancel);
        if bytes < 1_000_000 {
            continue;
        }
        out.push(Item {
            path,
            bytes,
            size: s.size,
            modified: s.modified,
            last_opened: None,
            is_dir: s.is_dir,
            group: Some(name),
            detail: String::new(),
            keep: false,
            confirmed: true,
        });
    }
    sort_biggest_first(&mut out);
    out
}

fn sort_biggest_first(items: &mut [Item]) {
    items.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.path.cmp(&b.path)));
}

/// Whether `path` still has the size and modification time it had when suggested
/// (checked just before acting, so nothing changed since gets trashed by surprise).
pub fn unchanged(path: &Path, size: u64, modified: i64) -> bool {
    stat(path).is_some_and(|s| s.is_dir || (s.size == size && s.modified == modified))
}

#[cfg(test)]
mod tests;
