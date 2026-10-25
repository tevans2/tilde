//! The Activity view (see docs/SPEC.md §6.6): what arrived or changed recently, where it
//! came from, and what's still waiting in an inbox folder.
//!
//! Nothing is logged. The feed is worked out when it's opened, from the index (files
//! modified recently) and listings of the inbox folders (everything in them, with the
//! date each item arrived), with origins read from extended attributes. So it has
//! history from the first launch, and moves made outside Tilde count as filing too: a
//! download that's no longer in an inbox folder has been filed.

mod origin;

pub use origin::detect;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use tilde_fs::{ListOptions, list_dir};
use tilde_types::EntryKind;

/// Most items in the feed besides the inbox folders' contents.
pub const FEED_LIMIT: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Origin {
    Download,
    Screenshot,
    AirDrop,
    /// Made here (created recently and not changed since).
    Created,
    Edited,
}

/// The number-key filters in the Activity view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OriginFilter {
    #[default]
    All,
    Downloads,
    Screenshots,
    /// Edited or created.
    Edited,
    AirDrop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Filter {
    pub origin: OriginFilter,
    /// Only what's still in an inbox folder.
    pub inbox: bool,
}

impl Filter {
    pub fn accepts(&self, item: &Item) -> bool {
        let origin = match self.origin {
            OriginFilter::All => true,
            OriginFilter::Downloads => item.origin == Origin::Download,
            OriginFilter::Screenshots => item.origin == Origin::Screenshot,
            OriginFilter::Edited => matches!(item.origin, Origin::Edited | Origin::Created),
            OriginFilter::AirDrop => item.origin == Origin::AirDrop,
        };
        origin && (!self.inbox || item.in_inbox)
    }
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub home: PathBuf,
    /// Inbox folders, including the screenshot folder.
    pub inbox: Vec<PathBuf>,
    /// How far back the feed goes.
    pub days: u32,
}

impl Settings {
    /// `inbox` from the config plus the screenshot folder.
    pub fn new(home: PathBuf, mut inbox: Vec<PathBuf>, days: u32) -> Settings {
        let shots = screenshot_folder(&home);
        if !inbox.contains(&shots) {
            inbox.push(shots);
        }
        Settings { home, inbox, days }
    }

    fn is_inbox(&self, folder: &Path) -> bool {
        self.inbox.iter().any(|i| i == folder)
    }
}

/// A recently modified file from the index.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub path: PathBuf,
    pub kind: EntryKind,
    pub size: u64,
    pub modified: i64,
    pub child_count: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub path: PathBuf,
    pub name: String,
    pub kind: EntryKind,
    pub size: u64,
    pub modified: i64,
    pub child_count: Option<u32>,
    /// When it shows in the feed: the later of when it was modified and when it arrived
    /// in its folder.
    pub time: i64,
    pub origin: Origin,
    /// The site a download came from (`github.com`).
    pub source: Option<String>,
    /// Still in an inbox folder.
    pub in_inbox: bool,
    /// Downloaded, a screenshot or AirDropped, and since moved out of the inboxes.
    pub filed: bool,
}

/// Builds the feed, newest first: everything in the inbox folders, plus `recent` files
/// from the index changed within `settings.days` (at most `FEED_LIMIT` of those).
pub fn gather(settings: &Settings, recent: Vec<Candidate>, now: i64) -> Vec<Item> {
    let since = now - settings.days as i64 * 86_400;
    let mut items = Vec::new();
    let mut seen = HashSet::new();

    for folder in &settings.inbox {
        let Ok(entries) = list_dir(folder, &ListOptions::default(), || false) else { continue };
        for e in entries {
            let path = folder.join(&e.name);
            let created = birth_time(&path).unwrap_or(e.added);
            let (origin, source) = detect(&path, created, e.modified);
            seen.insert(path.clone());
            items.push(Item {
                name: e.name,
                path,
                kind: e.kind,
                size: e.size,
                modified: e.modified,
                child_count: e.child_count,
                time: e.modified.max(e.added),
                origin,
                source,
                in_inbox: true,
                filed: false,
            });
        }
    }

    let mut others = 0;
    for c in recent {
        if others >= FEED_LIMIT {
            break;
        }
        if c.modified < since || seen.contains(&c.path) || c.path.parent().is_some_and(|p| settings.is_inbox(p)) {
            continue;
        }
        let created = birth_time(&c.path).unwrap_or(c.modified);
        let (origin, source) = detect(&c.path, created, c.modified);
        let name = c.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        others += 1;
        items.push(Item {
            name,
            filed: matches!(origin, Origin::Download | Origin::Screenshot | Origin::AirDrop),
            path: c.path,
            kind: c.kind,
            size: c.size,
            modified: c.modified,
            child_count: c.child_count,
            time: c.modified,
            origin,
            source,
            in_inbox: false,
        });
    }
    items.sort_by_key(|i| std::cmp::Reverse(i.time));
    items
}

fn birth_time(path: &Path) -> Option<i64> {
    let created = std::fs::symlink_metadata(path).ok()?.created().ok()?;
    Some(created.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64)
}

/// Where screenshots are saved (`defaults write com.apple.screencapture location`),
/// else the Desktop.
pub fn screenshot_folder(home: &Path) -> PathBuf {
    let prefs = home.join("Library/Preferences/com.apple.screencapture.plist");
    let configured = plist::Value::from_file(prefs)
        .ok()
        .and_then(|v| v.as_dictionary()?.get("location")?.as_string().map(str::to_string))
        .map(|loc| match loc.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None => PathBuf::from(loc),
        })
        .filter(|p| p.is_dir());
    configured.unwrap_or_else(|| home.join("Desktop"))
}

/// Date headings in the feed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Today,
    Yesterday,
    ThisWeek,
    ThisMonth,
    Earlier,
}

impl Group {
    pub fn title(self) -> &'static str {
        match self {
            Group::Today => "Today",
            Group::Yesterday => "Yesterday",
            Group::ThisWeek => "This week",
            Group::ThisMonth => "This month",
            Group::Earlier => "Earlier",
        }
    }

    /// The heading for `time`, given the local midnight that started today.
    pub fn of(time: i64, today: i64) -> Group {
        const DAY: i64 = 86_400;
        match time {
            t if t >= today => Group::Today,
            t if t >= today - DAY => Group::Yesterday,
            t if t >= today - 6 * DAY => Group::ThisWeek,
            t if t >= today - 29 * DAY => Group::ThisMonth,
            _ => Group::Earlier,
        }
    }
}

/// Local midnight at the start of the day containing `now`.
pub fn local_midnight(now: i64) -> i64 {
    unsafe {
        let t = now as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return now - now.rem_euclid(86_400);
        }
        tm.tm_hour = 0;
        tm.tm_min = 0;
        tm.tm_sec = 0;
        tm.tm_isdst = -1;
        libc::mktime(&mut tm) as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{Duration, SystemTime};

    fn now() -> i64 {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
    }

    fn set_modified(p: &Path, secs_ago: u64) {
        let t = SystemTime::now() - Duration::from_secs(secs_ago);
        fs::File::options().write(true).open(p).unwrap().set_modified(t).unwrap();
    }

    #[test]
    fn feed_combines_inbox_and_recent_files() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap();
        for d in ["Downloads", "Desktop", "Documents"] {
            fs::create_dir(home.join(d)).unwrap();
        }
        let dl = home.join("Downloads/report.pdf");
        fs::write(&dl, b"x").unwrap();
        origin::set_xattr(&dl, "com.apple.quarantine", b"0281;6addd698;Safari;ABC");
        // Old, but still waiting in the inbox.
        let old = home.join("Downloads/old.zip");
        fs::write(&old, b"x").unwrap();
        set_modified(&old, 400 * 86_400);
        fs::write(home.join("Downloads/.DS_Store"), b"x").unwrap();
        // Filed: a download that now lives elsewhere.
        let filed = home.join("Documents/invoice.pdf");
        fs::write(&filed, b"x").unwrap();
        origin::set_xattr(&filed, "com.apple.quarantine", b"0281;6addd698;Safari;ABC");
        set_modified(&filed, 3600);
        let edited = home.join("Documents/notes.md");
        fs::write(&edited, b"x").unwrap();
        let stale = home.join("Documents/ancient.txt");
        fs::write(&stale, b"x").unwrap();
        set_modified(&stale, 90 * 86_400);

        let settings =
            Settings { home: home.clone(), inbox: vec![home.join("Downloads"), home.join("Desktop")], days: 30 };
        let candidate = |p: &Path| {
            let md = fs::metadata(p).unwrap();
            Candidate {
                path: p.to_path_buf(),
                kind: EntryKind::File,
                size: 1,
                modified: md.modified().unwrap().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64,
                child_count: None,
            }
        };
        // The index also reports the inbox download; it isn't listed twice.
        let recent = vec![candidate(&edited), candidate(&dl), candidate(&filed), candidate(&stale)];
        let items = gather(&settings, recent, now());
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names.len(), 4, "{names:?}");
        assert!(names.contains(&"report.pdf") && names.contains(&"notes.md"));
        // Newest first. (The old zip arrived in Downloads just now, so its time is now.)
        assert_eq!(*names.last().unwrap(), "invoice.pdf");
        assert!(names.contains(&"old.zip"));

        let get = |n: &str| items.iter().find(|i| i.name == n).unwrap();
        assert_eq!(get("report.pdf").origin, Origin::Download);
        assert!(get("report.pdf").in_inbox && !get("report.pdf").filed);
        assert!(get("invoice.pdf").filed && !get("invoice.pdf").in_inbox);
        assert!(matches!(get("notes.md").origin, Origin::Created | Origin::Edited));
        assert!(!get("notes.md").filed);

        let inbox = Filter { origin: OriginFilter::All, inbox: true };
        assert_eq!(items.iter().filter(|i| inbox.accepts(i)).count(), 2);
        let downloads = Filter { origin: OriginFilter::Downloads, inbox: false };
        assert_eq!(items.iter().filter(|i| downloads.accepts(i)).count(), 2);
    }

    #[test]
    fn groups_by_local_day() {
        let n = now();
        let today = local_midnight(n);
        assert!(today <= n && n - today < 86_400 + 3600);
        assert_eq!(Group::of(n, today), Group::Today);
        assert_eq!(Group::of(today - 10, today), Group::Yesterday);
        assert_eq!(Group::of(today - 3 * 86_400, today), Group::ThisWeek);
        assert_eq!(Group::of(today - 20 * 86_400, today), Group::ThisMonth);
        assert_eq!(Group::of(today - 40 * 86_400, today), Group::Earlier);
    }

    #[test]
    fn screenshot_folder_defaults_to_desktop() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(screenshot_folder(dir.path()), dir.path().join("Desktop"));
        let prefs = dir.path().join("Library/Preferences");
        fs::create_dir_all(&prefs).unwrap();
        fs::create_dir(dir.path().join("Shots")).unwrap();
        let mut d = plist::Dictionary::new();
        d.insert("location".into(), plist::Value::String("~/Shots".into()));
        plist::Value::Dictionary(d).to_file_binary(prefs.join("com.apple.screencapture.plist")).unwrap();
        assert_eq!(screenshot_folder(dir.path()), dir.path().join("Shots"));
    }
}
