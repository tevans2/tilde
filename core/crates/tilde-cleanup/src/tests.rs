use super::*;
use std::fs;
use std::time::{Duration, SystemTime};

const DAY: i64 = 86_400;

fn now() -> i64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs() as i64
}

fn rules(home: &Path) -> Rules {
    Rules {
        big_file: 1_000_000,
        big_file_age: 180 * DAY,
        download_age: 60 * DAY,
        project_idle: 90 * DAY,
        duplicates_min: 100_000,
        build_folders: vec!["node_modules".into(), "target".into(), ".venv".into()],
        ignore: Ignore::new(&["~/keep".into(), "*.vmdk".into()], home),
    }
}

fn write(path: &Path, bytes: usize, days_old: i64) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![1u8; bytes]).unwrap();
    let t = SystemTime::now() - Duration::from_secs((days_old * DAY) as u64);
    fs::File::options().write(true).open(path).unwrap().set_modified(t).unwrap();
}

fn indexed(path: &Path) -> Indexed {
    let s = stat(path).unwrap();
    let kind = if s.is_dir { EntryKind::Folder } else { EntryKind::File };
    (path.to_path_buf(), IndexedMeta { kind, size: s.size, modified: s.modified })
}

#[test]
fn big_files_are_old_unopened_and_not_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    write(&home.join("Movies/old.mov"), 2_000_000, 400);
    write(&home.join("Movies/new.mov"), 2_000_000, 3);
    write(&home.join("Movies/small.mov"), 10_000, 400);
    write(&home.join("keep/backup.mov"), 2_000_000, 400);
    write(&home.join("vm/disk.vmdk"), 2_000_000, 400);
    write(&home.join(".hidden/old.bin"), 2_000_000, 400);
    write(&home.join("App.app/Contents/big.bin"), 2_000_000, 400);
    let index: Vec<Indexed> = [
        "Movies/old.mov",
        "Movies/new.mov",
        "Movies/small.mov",
        "keep/backup.mov",
        "vm/disk.vmdk",
        ".hidden/old.bin",
        "App.app/Contents/big.bin",
    ]
    .iter()
    .map(|p| indexed(&home.join(p)))
    .collect();
    let found = big_files(&index, &rules(&home), &home, now());
    let names: Vec<_> =
        found.iter().map(|i| i.path.strip_prefix(&home).unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["Movies/old.mov"]);
    assert!(found[0].bytes >= 2_000_000);
}

#[test]
fn downloads_say_why() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    let downloads = home.join("Downloads");
    write(&downloads.join("Setup.dmg"), 300_000, 0);
    write(&downloads.join("folder/inside.txt"), 200_000, 0);
    let mut r = rules(&home);
    r.download_age = -DAY; // Everything counts as old.
    let found = old_downloads(std::slice::from_ref(&downloads), &r, now(), &AtomicBool::new(false));
    assert_eq!(found.len(), 2);
    let dmg = found.iter().find(|i| i.path.ends_with("Setup.dmg")).unwrap();
    assert_eq!(dmg.detail, "installer");
    let folder = found.iter().find(|i| i.path.ends_with("folder")).unwrap();
    assert!(folder.is_dir && folder.bytes >= 200_000, "folders are measured");
    r.download_age = 60 * DAY;
    assert!(old_downloads(&[downloads], &r, now(), &AtomicBool::new(false)).is_empty(), "nothing old enough");
}

#[test]
fn idle_projects_offer_their_build_folders() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    write(&home.join("dev/old/Cargo.toml"), 100, 200);
    write(&home.join("dev/old/src/main.rs"), 100, 200);
    write(&home.join("dev/old/target/debug/app"), 500_000, 200);
    write(&home.join("dev/old/.venv/lib.py"), 1000, 200);
    write(&home.join("dev/busy/package.json"), 100, 200);
    write(&home.join("dev/busy/index.js"), 100, 2);
    write(&home.join("dev/busy/node_modules/x/index.js"), 1000, 200);
    write(&home.join("dev/nobuild/go.mod"), 100, 200);
    // The index has the projects' own files, not what's inside build folders.
    let index: Vec<Indexed> = [
        "dev/old/Cargo.toml",
        "dev/old/src/main.rs",
        "dev/busy/package.json",
        "dev/busy/index.js",
        "dev/nobuild/go.mod",
    ]
    .iter()
    .map(|p| indexed(&home.join(p)))
    .collect();
    let found = idle_projects(&index, &rules(&home), &home, now());
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].dir, home.join("dev/old"));
    let names: Vec<_> = found[0].build.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["target", ".venv"], "hidden build folders count too");
}

#[test]
fn trash_lists_and_empties() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    write(&home.join(".Trash/a.bin"), 300_000, 0);
    write(&home.join(".Trash/folder/b.bin"), 200_000, 0);
    let items = trash(&home, &AtomicBool::new(false));
    assert_eq!(items.len(), 2);
    assert!(items.iter().map(|i| i.bytes).sum::<u64>() >= 500_000);
    let freed = empty_trash(&home).unwrap();
    assert!(freed >= 500_000);
    assert_eq!(fs::read_dir(home.join(".Trash")).unwrap().count(), 0);
}

#[test]
fn ignore_matches_paths_names_and_patterns() {
    let home = Path::new("/Users/me");
    let ignore = Ignore::new(&["~/Movies/keep".into(), "*.vmdk".into(), "Steam".into()], home);
    assert!(ignore.matches(Path::new("/Users/me/Movies/keep/a.mov")));
    assert!(ignore.matches(Path::new("/Users/me/vm/disk.VMDK")));
    assert!(ignore.matches(Path::new("/Users/me/Library/Steam/game.pak")));
    assert!(!ignore.matches(Path::new("/Users/me/Movies/other.mov")));
}

#[test]
fn duplicates_in_stages() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().canonicalize().unwrap();
    let content: Vec<u8> = (0..400_000u32).map(|i| (i * 7 % 251) as u8).collect();
    let mut differs_at_end = content.clone();
    *differs_at_end.last_mut().unwrap() ^= 1;
    let mut differs_in_middle = content.clone();
    differs_in_middle[100_000] ^= 1; // Not in any sample: only the full check tells.
    fs::create_dir_all(home.join("Downloads")).unwrap();
    fs::create_dir_all(home.join("Documents")).unwrap();
    fs::write(home.join("Documents/report.pdf"), &content).unwrap();
    fs::write(home.join("Downloads/report (1).pdf"), &content).unwrap();
    fs::write(home.join("Documents/end.pdf"), &differs_at_end).unwrap();
    fs::write(home.join("Documents/middle.pdf"), &differs_in_middle).unwrap();
    fs::hard_link(home.join("Documents/report.pdf"), home.join("Documents/link.pdf")).unwrap();
    fs::write(home.join("Documents/small.txt"), b"tiny").unwrap();
    let paths: Vec<PathBuf> = fs::read_dir(home.join("Documents"))
        .unwrap()
        .chain(fs::read_dir(home.join("Downloads")).unwrap())
        .map(|e| e.unwrap().path())
        .collect();

    let likely_seen = Mutex::new(Vec::new());
    let mut cache = dupes::Cache::default();
    let groups = dupes::find(
        paths.clone(),
        100_000,
        &mut cache,
        &AtomicBool::new(false),
        &|g| likely_seen.lock().unwrap().push(g.to_vec()),
        &|_| {},
    );
    // Sampling can't see the change in the middle, so that one is "likely" at first.
    let first = &likely_seen.lock().unwrap()[0];
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].files.len(), 3, "{first:?}");
    assert!(!first[0].confirmed);
    // The full check drops it; the hard link is the same file, counted once.
    assert_eq!(groups.len(), 1);
    let g = &groups[0];
    assert!(g.confirmed);
    let names: Vec<_> = g.files.iter().map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(names.contains(&"report (1).pdf".to_string()));
    assert!(g.files[g.keep].path.starts_with(home.join("Documents")), "keeps the copy outside Downloads");
    assert!(g.reclaimable() >= 400_000);

    // A second scan answers from the cache: same result without reading contents.
    let cache_file = home.join("hashes.json");
    cache.save(&cache_file);
    let mut cache = dupes::Cache::load(&cache_file);
    let read = std::sync::atomic::AtomicU64::new(u64::MAX);
    let again = dupes::find(paths, 100_000, &mut cache, &AtomicBool::new(false), &|_| {}, &|p| {
        read.store(p.total, std::sync::atomic::Ordering::Relaxed)
    });
    assert_eq!(again.len(), 1);
    assert_eq!(read.load(std::sync::atomic::Ordering::Relaxed), 0, "nothing left to read in full");

    assert!(dupes::same_contents(&home.join("Documents/report.pdf"), &home.join("Downloads/report (1).pdf")).unwrap());
    assert!(!dupes::same_contents(&home.join("Documents/report.pdf"), &home.join("Documents/middle.pdf")).unwrap());
}

use std::sync::Mutex;
