use super::*;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::mpsc::Receiver;
use std::time::Duration;

struct Collect(Mutex<Sender<Outcome>>);

impl Listener for Collect {
    fn started(&self, _id: u64, _label: &str, _total: u32) {}
    fn progress(&self, _id: u64, _done: u32, _total: u32, _current: &str) {}
    fn finished(&self, _id: u64, outcome: Outcome) {
        let _ = self.0.lock().unwrap().send(outcome);
    }
}

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    trash: PathBuf,
    ops: Operations,
    rx: Receiver<Outcome>,
}

impl Env {
    fn new() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let (root, trash) = (base.join("root"), base.join("trash"));
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&trash).unwrap();
        let (tx, rx) = mpsc::channel();
        let ops = Operations::new(Box::new(FolderTrash(trash.clone())), Arc::new(Collect(Mutex::new(tx))));
        Env { _dir: dir, root, trash, ops, rx }
    }

    fn p(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn file(&self, rel: &str, contents: &str) {
        let p = self.p(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, contents).unwrap();
    }

    fn wait(&self) -> Outcome {
        self.rx.recv_timeout(Duration::from_secs(10)).expect("operation didn't finish")
    }

    fn run(&self, request: Request) -> Outcome {
        self.ops.submit(request);
        self.wait()
    }

    fn undo(&self) -> Outcome {
        self.ops.undo().expect("something to undo");
        self.wait()
    }

    fn redo(&self) -> Outcome {
        self.ops.redo().expect("something to redo");
        self.wait()
    }

    /// Every path under root with file contents ("/" for folders).
    fn tree(&self) -> BTreeMap<String, String> {
        fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<String, String>) {
            for e in fs::read_dir(dir).unwrap() {
                let p = e.unwrap().path();
                let rel = p.strip_prefix(root).unwrap().to_string_lossy().into_owned();
                if p.is_dir() {
                    out.insert(rel, "/".into());
                    walk(&p, root, out);
                } else {
                    out.insert(rel, fs::read_to_string(&p).unwrap_or_default());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(&self.root, &self.root, &mut out);
        out
    }
}

#[test]
fn move_with_conflicts_then_undo_and_redo() {
    let env = Env::new();
    env.file("a/report.pdf", "A");
    env.file("b/report.pdf", "B");
    env.file("dest/report.pdf", "D");
    let before = env.tree();

    let out =
        env.run(Request::Move { sources: vec![env.p("a/report.pdf"), env.p("b/report.pdf")], into: env.p("dest") });
    assert_eq!((out.completed, out.total, out.renamed, out.error.clone()), (2, 2, 2, None));
    assert_eq!(fs::read_to_string(env.p("dest/report 2.pdf")).unwrap(), "A");
    assert_eq!(fs::read_to_string(env.p("dest/report 3.pdf")).unwrap(), "B");
    assert_eq!(fs::read_to_string(env.p("dest/report.pdf")).unwrap(), "D", "existing file untouched");
    assert_eq!(out.results, vec![env.p("dest/report 2.pdf"), env.p("dest/report 3.pdf")]);
    assert_eq!(env.ops.undo_label().as_deref(), Some("Move 2 items"));

    let out = env.undo();
    assert_eq!((out.kind, out.error), (OutcomeKind::Undone, None));
    assert_eq!(env.tree(), before);
    assert_eq!(env.ops.redo_label().as_deref(), Some("Move 2 items"));

    let out = env.redo();
    assert_eq!((out.kind, out.completed), (OutcomeKind::Redone, 2));
    assert!(env.p("dest/report 3.pdf").exists());
}

#[test]
fn moving_into_the_same_folder_does_nothing() {
    let env = Env::new();
    env.file("a/x.txt", "x");
    let out = env.run(Request::Move { sources: vec![env.p("a/x.txt")], into: env.p("a") });
    assert_eq!((out.completed, out.skipped, out.error), (0, 1, None));
    assert!(env.ops.undo().is_none(), "nothing happened, so nothing to undo");
}

#[test]
fn folders_cant_go_inside_themselves() {
    let env = Env::new();
    env.file("a/inner/x.txt", "x");
    let out = env.run(Request::Move { sources: vec![env.p("a")], into: env.p("a/inner") });
    assert!(out.error.unwrap().contains("into itself"));
    let out = env.run(Request::Copy { sources: vec![env.p("a")], into: env.p("a") });
    assert!(out.error.unwrap().contains("into itself"));
}

#[test]
fn copy_folder_recursively_and_undo_to_trash() {
    let env = Env::new();
    env.file("src/one.txt", "1");
    env.file("src/sub/two.txt", "2");
    fs::create_dir(env.p("dest")).unwrap();
    let out = env.run(Request::Copy { sources: vec![env.p("src")], into: env.p("dest") });
    assert_eq!(out.error, None);
    assert_eq!(fs::read_to_string(env.p("dest/src/sub/two.txt")).unwrap(), "2");
    assert_eq!(fs::read_to_string(env.p("src/one.txt")).unwrap(), "1", "original kept");

    // Copying into the same folder keeps both.
    let out = env.run(Request::Copy { sources: vec![env.p("src/one.txt")], into: env.p("src") });
    assert_eq!(out.renamed, 1);
    assert!(env.p("src/one 2.txt").exists());

    env.undo();
    assert!(!env.p("src/one 2.txt").exists());
    env.undo();
    assert!(!env.p("dest/src").exists());
    assert!(env.trash.join("src").exists(), "undoing a copy trashes it rather than deleting");
}

#[test]
fn trash_and_put_back() {
    let env = Env::new();
    env.file("docs/notes.md", "n");
    env.file("docs/old/list.txt", "l");
    let before = env.tree();
    let out = env.run(Request::Trash { sources: vec![env.p("docs/notes.md"), env.p("docs/old")] });
    assert_eq!(out.completed, 2);
    assert!(!env.p("docs/notes.md").exists());
    assert!(env.trash.join("old/list.txt").exists());
    env.undo();
    assert_eq!(env.tree(), before);
}

#[test]
fn rename_rules() {
    let env = Env::new();
    env.file("a.txt", "a");
    env.file("b.txt", "b");

    let out = env.run(Request::Rename { source: env.p("a.txt"), name: "b.txt".into() });
    assert!(out.error.unwrap().contains("already exists"));
    let out = env.run(Request::Rename { source: env.p("a.txt"), name: "bad/name".into() });
    assert!(out.error.is_some());

    let out = env.run(Request::Rename { source: env.p("a.txt"), name: "c.txt".into() });
    assert_eq!(out.error, None);
    assert_eq!(fs::read_to_string(env.p("c.txt")).unwrap(), "a");

    // Case-only rename (the default macOS volume is case-insensitive).
    let out = env.run(Request::Rename { source: env.p("c.txt"), name: "C.txt".into() });
    assert_eq!(out.error, None);
    let listed: Vec<String> =
        fs::read_dir(&env.root).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into()).collect();
    assert!(listed.contains(&"C.txt".to_string()), "{listed:?}");

    env.undo();
    env.undo();
    assert_eq!(env.tree().keys().cloned().collect::<Vec<_>>(), ["a.txt", "b.txt"]);
}

#[test]
fn new_folder_and_compress() {
    let env = Env::new();
    env.file("x.txt", "x");
    env.file("y/z.txt", "z");
    let out = env.run(Request::NewFolder { parent: env.root.clone(), name: "Archive".into() });
    assert_eq!(out.error, None);
    assert!(env.p("Archive").is_dir());
    let out = env.run(Request::NewFolder { parent: env.root.clone(), name: "Archive".into() });
    assert!(out.error.unwrap().contains("already exists"));

    let out = env.run(Request::Compress { sources: vec![env.p("x.txt")] });
    assert_eq!(out.error, None);
    assert!(fs::metadata(env.p("x.txt.zip")).unwrap().len() > 0);

    let out = env.run(Request::Compress { sources: vec![env.p("x.txt"), env.p("y")] });
    assert_eq!(out.error, None);
    let listing = std::process::Command::new("/usr/bin/unzip").arg("-l").arg(env.p("Archive.zip")).output().unwrap();
    let listing = String::from_utf8_lossy(&listing.stdout);
    assert!(listing.contains("x.txt") && listing.contains("y/z.txt"), "{listing}");
    assert!(!env.tree().keys().any(|k| k.starts_with(".tilde-zip")), "staging folder removed");

    env.undo();
    assert!(!env.p("Archive.zip").exists());
}

#[test]
fn errors_are_reported_and_nothing_partial_is_lost() {
    let env = Env::new();
    env.file("a.txt", "a");
    let out = env.run(Request::Move { sources: vec![env.p("missing.txt")], into: env.root.clone() });
    assert!(out.error.unwrap().contains("no longer exists"));
    let out = env.run(Request::Move { sources: vec![env.p("a.txt")], into: env.p("nowhere") });
    assert!(out.error.unwrap().contains("isn't a folder"));
    assert_eq!(env.tree(), BTreeMap::from([("a.txt".to_string(), "a".to_string())]));
}

#[test]
fn new_operations_clear_redo() {
    let env = Env::new();
    env.file("a.txt", "a");
    env.run(Request::Rename { source: env.p("a.txt"), name: "b.txt".into() });
    env.undo();
    assert!(env.ops.redo_label().is_some());
    env.run(Request::Rename { source: env.p("a.txt"), name: "c.txt".into() });
    assert!(env.ops.redo_label().is_none());
}

/// Random sequences of operations, then undo everything: the tree must be exactly as it
/// started.
#[test]
fn undoing_everything_restores_the_original_tree() {
    for seed in 1..=12u64 {
        let env = Env::new();
        for f in ["a/1.txt", "a/2.txt", "b/3.txt", "b/c/4.txt", "d/5.txt", "6.txt"] {
            env.file(f, f);
        }
        let before = env.tree();
        let mut rng = seed;
        let mut next = |n: usize| {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (rng >> 33) as usize % n
        };
        let mut applied = 0;
        for step in 0..15 {
            let all: Vec<PathBuf> = env.tree().keys().map(|k| env.p(k)).collect();
            let dirs: Vec<PathBuf> = all.iter().filter(|p| p.is_dir()).cloned().chain([env.root.clone()]).collect();
            let pick = all[next(all.len())].clone();
            let request = match next(5) {
                0 => Request::Move { sources: vec![pick], into: dirs[next(dirs.len())].clone() },
                1 => Request::Copy { sources: vec![pick], into: dirs[next(dirs.len())].clone() },
                2 => Request::Trash { sources: vec![pick] },
                3 => Request::Rename { source: pick, name: format!("r{seed}-{step}") },
                _ => Request::NewFolder { parent: dirs[next(dirs.len())].clone(), name: format!("n{step}") },
            };
            if env.run(request).completed > 0 {
                applied += 1;
            }
        }
        for _ in 0..applied {
            let out = env.undo();
            assert_eq!(out.error, None, "seed {seed}");
        }
        assert_eq!(env.tree(), before, "seed {seed}");
    }
}

/// Uses the real Trash, so it only runs when asked: `cargo test -p tilde-ops -- --ignored`.
/// Cleans up after itself.
#[test]
#[ignore]
fn system_trash_reports_where_items_go() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(format!("tilde-trash-test-{}.txt", std::process::id()));
    fs::write(&file, "x").unwrap();
    let in_trash = SystemTrash.trash(&file).unwrap();
    assert!(!file.exists());
    assert!(in_trash.exists(), "{in_trash:?}");
    assert!(in_trash.to_string_lossy().contains(".Trash"), "{in_trash:?}");
    fs::remove_file(in_trash).unwrap();
}
