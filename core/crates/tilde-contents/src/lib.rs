//! Searching inside files, on request (`text:` in a query), without an index.
//!
//! - [`scan`] reads the files under a folder in parallel, at low priority: `.gitignore`d,
//!   build and hidden folders are skipped (unless asked for), binary and large files too,
//!   and it stops after a number of files.
//! - [`spotlight`] asks Spotlight (`mdfind`) for files anywhere else, if it's indexing.
//! - [`search_file`] finds the first matching line in one file, for a result's snippet.
//!
//! Matching is a literal: case-insensitive (ASCII) unless the text has a capital, as
//! ripgrep's smart case.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use aho_corasick::AhoCorasick;
use ignore::{WalkBuilder, WalkState};

/// What to look for.
pub struct Needle {
    text: String,
    matcher: AhoCorasick,
}

impl Needle {
    /// `None` for empty text.
    pub fn new(text: &str) -> Option<Needle> {
        if text.is_empty() {
            return None;
        }
        let case_sensitive = text.chars().any(char::is_uppercase);
        let matcher = AhoCorasick::builder().ascii_case_insensitive(!case_sensitive).build([text]).ok()?;
        Some(Needle { text: text.to_string(), matcher })
    }

    pub fn text(&self) -> &str {
        &self.text
    }
}

/// The first match in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// 1-based.
    pub line: u32,
    /// The line, trimmed and shortened around the match.
    pub snippet: String,
    /// Matched characters in `snippet` (char indices).
    pub highlights: Vec<u32>,
    /// Matches in the whole file, up to 999.
    pub count: u32,
}

/// Longest snippet, in characters.
const SNIPPET: usize = 160;
/// Bytes looked at for a NUL, to tell binary files.
const SNIFF: usize = 8192;

/// Searches one file. `None` if it's binary, bigger than `max_size`, unreadable or has no
/// match.
pub fn search_file(path: &Path, needle: &Needle, max_size: u64) -> Option<Match> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len > max_size || len == 0 {
        return None;
    }
    let mut data = Vec::with_capacity(len as usize);
    file.read_to_end(&mut data).ok()?;
    search_bytes(&data, needle)
}

fn search_bytes(data: &[u8], needle: &Needle) -> Option<Match> {
    if memchr::memchr(0, &data[..data.len().min(SNIFF)]).is_some() {
        return None;
    }
    let mut matches = needle.matcher.find_iter(data);
    let first = matches.next()?;
    let count = 1 + matches.take(998).count() as u32;
    let line_start = memchr::memrchr(b'\n', &data[..first.start()]).map_or(0, |i| i + 1);
    let line_end = memchr::memchr(b'\n', &data[first.end()..]).map_or(data.len(), |i| first.end() + i);
    let line = 1 + memchr::memchr_iter(b'\n', &data[..line_start]).count() as u32;

    // The match as char positions in the (lossily decoded) line.
    let before = String::from_utf8_lossy(&data[line_start..first.start()]);
    let matched = String::from_utf8_lossy(&data[first.start()..first.end()]);
    let after = String::from_utf8_lossy(&data[first.end()..line_end]);
    let before = before.trim_start();
    let after = after.trim_end();
    let (before_chars, matched_chars) = (before.chars().count(), matched.chars().count());
    // Long lines keep a little context before the match (rows are narrow), and as much
    // after as fits.
    let skip = before_chars.saturating_sub(16);
    let mut snippet: String = before.chars().skip(skip).collect();
    snippet.push_str(&matched);
    snippet.extend(after.chars().take(SNIPPET.saturating_sub(before_chars - skip + matched_chars)));
    let mut start = (before_chars - skip) as u32;
    if skip > 0 {
        snippet.insert(0, '…');
        start += 1;
    }
    Some(Match { line, snippet, highlights: (start..start + matched_chars as u32).collect(), count })
}

/// What [`scan`] skips and how far it goes.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Look in hidden files and folders.
    pub hidden: bool,
    /// Also read what `.gitignore`, `.ignore` and `.git/info/exclude` leave out.
    pub ignored: bool,
    /// Folder names never entered (`node_modules`, `target`…). `.git` always is.
    pub skip: Vec<String>,
    /// Stop after reading this many files.
    pub max_files: usize,
    /// Files bigger than this are left out.
    pub max_size: u64,
    pub threads: usize,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions {
            hidden: false,
            ignored: false,
            skip: Vec::new(),
            max_files: 20_000,
            max_size: 2 << 20,
            threads: 4,
        }
    }
}

/// How a scan ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanEnd {
    pub files: usize,
    /// Stopped at `max_files`.
    pub truncated: bool,
    pub cancelled: bool,
}

/// Searches the files under `root`, calling `on_hit` (on the calling thread) as matches
/// are found, in no particular order. `cancelled` is checked between files.
pub fn scan(
    root: &Path,
    needle: &Needle,
    options: &ScanOptions,
    cancelled: &(dyn Fn() -> bool + Sync),
    on_hit: &mut dyn FnMut(PathBuf, Match),
) -> ScanEnd {
    let files = AtomicUsize::new(0);
    let truncated = AtomicBool::new(false);
    let stop = AtomicBool::new(false);
    let skip = options.skip.clone();
    let walker = WalkBuilder::new(root)
        .hidden(!options.hidden)
        .git_ignore(!options.ignored)
        .git_global(false)
        .git_exclude(!options.ignored)
        .ignore(!options.ignored)
        .parents(true)
        .require_git(false)
        .follow_links(false)
        .threads(options.threads.max(1))
        .filter_entry(move |e| {
            let name = e.file_name().to_string_lossy();
            !(e.file_type().is_some_and(|t| t.is_dir()) && (name == ".git" || skip.iter().any(|s| *s == name)))
        })
        .build_parallel();
    let (tx, rx) = mpsc::channel::<(PathBuf, Match)>();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            walker.run(|| {
                let tx = tx.clone();
                let (files, truncated, stop) = (&files, &truncated, &stop);
                Box::new(move |entry| {
                    background_priority();
                    if stop.load(Ordering::Relaxed) || cancelled() {
                        stop.store(true, Ordering::Relaxed);
                        return WalkState::Quit;
                    }
                    let Ok(entry) = entry else { return WalkState::Continue };
                    if !entry.file_type().is_some_and(|t| t.is_file()) {
                        return WalkState::Continue;
                    }
                    if files.fetch_add(1, Ordering::Relaxed) >= options.max_files {
                        truncated.store(true, Ordering::Relaxed);
                        stop.store(true, Ordering::Relaxed);
                        return WalkState::Quit;
                    }
                    if let Some(m) = search_file(entry.path(), needle, options.max_size) {
                        let _ = tx.send((entry.into_path(), m));
                    }
                    WalkState::Continue
                })
            });
            drop(tx);
        });
        for (path, m) in rx {
            on_hit(path, m);
        }
    });
    let truncated = truncated.load(Ordering::Relaxed);
    ScanEnd {
        files: files.load(Ordering::Relaxed).min(options.max_files),
        truncated,
        cancelled: stop.load(Ordering::Relaxed) && !truncated,
    }
}

/// Lowers this thread to utility QoS (macOS), once.
fn background_priority() {
    thread_local!(static SET: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
    SET.with(|set| {
        if !set.replace(true) {
            #[cfg(target_os = "macos")]
            {
                unsafe extern "C" {
                    fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
                }
                const QOS_CLASS_UTILITY: u32 = 0x11;
                // SAFETY: only changes the calling thread's scheduling class.
                unsafe { pthread_set_qos_class_self_np(QOS_CLASS_UTILITY, 0) };
            }
        }
    });
}

/// Lowers the calling thread's priority, for work done on behalf of a search.
pub fn lower_priority() {
    background_priority();
}

/// Whether Spotlight is indexing `path`'s volume (`mdutil -s`). Asked once.
pub fn spotlight_available(path: &Path) -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        Command::new("/usr/bin/mdutil")
            .arg("-s")
            .arg(path)
            .stderr(Stdio::null())
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("Indexing enabled"))
            .unwrap_or(false)
    })
}

/// Files under `root` whose text Spotlight has and contains `text` (case and diacritic
/// insensitive), up to `limit`. Empty if Spotlight fails, or `cancelled` turns true
/// first (the query is stopped).
pub fn spotlight(root: &Path, text: &str, limit: usize, cancelled: &dyn Fn() -> bool) -> Vec<PathBuf> {
    let escaped: String = text
        .chars()
        .flat_map(|c| match c {
            '"' | '\\' | '*' => vec!['\\', c],
            c => vec![c],
        })
        .collect();
    let query = format!("kMDItemTextContent == \"*{escaped}*\"cd");
    let Ok(mut child) = Command::new("/usr/bin/mdfind")
        .arg("-onlyin")
        .arg(root)
        .arg(query)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
    else {
        return Vec::new();
    };
    let mut stdout = child.stdout.take().expect("piped");
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        out
    });
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if cancelled() => {
                let _ = child.kill();
                let _ = child.wait();
                return Vec::new();
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => return Vec::new(),
        }
    }
    let out = reader.join().unwrap_or_default();
    out.lines().filter(|l| !l.is_empty()).take(limit).map(PathBuf::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn needle(s: &str) -> Needle {
        Needle::new(s).unwrap()
    }

    #[test]
    fn finds_the_first_line_and_counts() {
        let m = search_bytes(b"alpha\n  the Launch window opens\nlaunch again\n", &needle("launch")).unwrap();
        assert_eq!(m.line, 2);
        assert_eq!(m.snippet, "the Launch window opens");
        assert_eq!(m.highlights, [4, 5, 6, 7, 8, 9]);
        assert_eq!(m.count, 2);
    }

    #[test]
    fn smart_case() {
        assert!(search_bytes(b"Launch", &needle("launch")).is_some());
        assert!(search_bytes(b"launch", &needle("Launch")).is_none());
        assert!(search_bytes(b"Launch", &needle("Launch")).is_some());
    }

    #[test]
    fn skips_binary() {
        assert!(search_bytes(b"launch\0\x01", &needle("launch")).is_none());
    }

    #[test]
    fn long_lines_are_cut_around_the_match() {
        let line = format!("{}needle{}", "a".repeat(300), "b".repeat(300));
        let m = search_bytes(line.as_bytes(), &needle("needle")).unwrap();
        assert!(m.snippet.starts_with('…'));
        assert!(m.snippet.chars().count() <= SNIPPET + 1);
        let hl: String = m.highlights.iter().map(|&i| m.snippet.chars().nth(i as usize).unwrap()).collect();
        assert_eq!(hl, "needle");
    }

    #[test]
    fn unicode_positions_are_chars() {
        let m = search_bytes("naïve café rocks".as_bytes(), &needle("café")).unwrap();
        let hl: String = m.highlights.iter().map(|&i| m.snippet.chars().nth(i as usize).unwrap()).collect();
        assert_eq!(hl, "café");
    }

    #[test]
    fn scans_respecting_ignores_and_skips() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/x")).unwrap();
        std::fs::create_dir_all(root.join("build")).unwrap();
        std::fs::write(root.join("src/a.rs"), "fn warp() {}\n").unwrap();
        std::fs::write(root.join("notes.md"), "# Warp drive\n").unwrap();
        std::fs::write(root.join("node_modules/x/index.js"), "warp\n").unwrap();
        std::fs::write(root.join("build/out.txt"), "warp\n").unwrap();
        std::fs::write(root.join(".gitignore"), "build/\n").unwrap();
        std::fs::write(root.join(".hidden.txt"), "warp\n").unwrap();
        let options = ScanOptions { skip: vec!["node_modules".into()], ..Default::default() };
        let mut found = Vec::new();
        let end = scan(root, &needle("warp"), &options, &|| false, &mut |p, _| {
            found.push(p.strip_prefix(root).unwrap().to_string_lossy().into_owned())
        });
        found.sort();
        assert_eq!(found, ["notes.md", "src/a.rs"]);
        assert!(!end.truncated && !end.cancelled);

        let everything = ScanOptions { hidden: true, ignored: true, ..Default::default() };
        let mut found = Vec::new();
        scan(root, &needle("warp"), &everything, &|| false, &mut |p, _| {
            found.push(p.strip_prefix(root).unwrap().to_string_lossy().into_owned())
        });
        found.sort();
        assert_eq!(found, [".hidden.txt", "build/out.txt", "node_modules/x/index.js", "notes.md", "src/a.rs"]);
    }

    #[test]
    fn stops_at_the_file_limit_and_when_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..50 {
            std::fs::write(dir.path().join(format!("{i}.txt")), "x\n").unwrap();
        }
        let options = ScanOptions { max_files: 10, threads: 1, ..Default::default() };
        let end = scan(dir.path(), &needle("x"), &options, &|| false, &mut |_, _| {});
        assert!(end.truncated);
        assert_eq!(end.files, 10);
        let end = scan(dir.path(), &needle("x"), &ScanOptions::default(), &|| true, &mut |_, _| {});
        assert!(end.cancelled);
    }
}
