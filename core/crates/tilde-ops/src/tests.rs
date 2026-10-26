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

    let out = env.run(Request::Move {
        sources: vec![env.p("a/report.pdf"), env.p("b/report.pdf")],
        into: env.p("dest"),
        clash: Clash::KeepBoth,
    });
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
    let out = env.run(Request::Move { sources: vec![env.p("a/x.txt")], into: env.p("a"), clash: Clash::KeepBoth });
    assert_eq!((out.completed, out.skipped, out.error), (0, 1, None));
    assert!(env.ops.undo().is_none(), "nothing happened, so nothing to undo");
}

#[test]
fn folders_cant_go_inside_themselves() {
    let env = Env::new();
    env.file("a/inner/x.txt", "x");
    let out = env.run(Request::Move { sources: vec![env.p("a")], into: env.p("a/inner"), clash: Clash::KeepBoth });
    assert!(out.error.unwrap().contains("into itself"));
    let out = env.run(Request::Copy { sources: vec![env.p("a")], into: env.p("a"), clash: Clash::KeepBoth });
    assert!(out.error.unwrap().contains("into itself"));
}

#[test]
fn copy_folder_recursively_and_undo_to_trash() {
    let env = Env::new();
    env.file("src/one.txt", "1");
    env.file("src/sub/two.txt", "2");
    fs::create_dir(env.p("dest")).unwrap();
    let out = env.run(Request::Copy { sources: vec![env.p("src")], into: env.p("dest"), clash: Clash::KeepBoth });
    assert_eq!(out.error, None);
    assert_eq!(fs::read_to_string(env.p("dest/src/sub/two.txt")).unwrap(), "2");
    assert_eq!(fs::read_to_string(env.p("src/one.txt")).unwrap(), "1", "original kept");

    // Copying into the same folder keeps both.
    let out =
        env.run(Request::Copy { sources: vec![env.p("src/one.txt")], into: env.p("src"), clash: Clash::KeepBoth });
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
    let out =
        env.run(Request::Move { sources: vec![env.p("missing.txt")], into: env.root.clone(), clash: Clash::KeepBoth });
    assert!(out.error.unwrap().contains("no longer exists"));
    let out = env.run(Request::Move { sources: vec![env.p("a.txt")], into: env.p("nowhere"), clash: Clash::KeepBoth });
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

/// Random sequences of operations (with every clash policy), then undo everything: the
/// tree must be exactly as it started.
#[test]
fn undoing_everything_restores_the_original_tree() {
    for seed in 1..=40u64 {
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
            if all.is_empty() {
                break;
            }
            let dirs: Vec<PathBuf> = all.iter().filter(|p| p.is_dir()).cloned().chain([env.root.clone()]).collect();
            let pick = all[next(all.len())].clone();
            let clash = [Clash::KeepBoth, Clash::Replace, Clash::Skip][next(3)];
            let request = match next(6) {
                0 => Request::Move { sources: vec![pick], into: dirs[next(dirs.len())].clone(), clash },
                1 => Request::Copy { sources: vec![pick], into: dirs[next(dirs.len())].clone(), clash },
                2 => Request::Trash { sources: vec![pick] },
                3 => Request::Rename { source: pick, name: format!("r{seed}-{step}") },
                4 => Request::Tag { sources: vec![pick], add: vec![format!("t{step}")], remove: vec![] },
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

fn batch(items: Vec<BatchItem>) -> Request {
    Request::Batch { label: "Edit".into(), items }
}

#[test]
fn batch_swaps_names_and_undoes() {
    let env = Env::new();
    env.file("a.txt", "A");
    env.file("b.txt", "B");
    let before = env.tree();
    let out = env.run(batch(vec![
        BatchItem::Move { from: env.p("a.txt"), to: env.p("b.txt") },
        BatchItem::Move { from: env.p("b.txt"), to: env.p("a.txt") },
    ]));
    assert_eq!(out.error, None);
    assert_eq!(fs::read_to_string(env.p("a.txt")).unwrap(), "B");
    assert_eq!(fs::read_to_string(env.p("b.txt")).unwrap(), "A");
    assert!(!env.tree().keys().any(|k| k.starts_with(".tilde-moving")));
    env.undo();
    assert_eq!(env.tree(), before);
}

#[test]
fn batch_creates_folders_before_moving_into_them() {
    let env = Env::new();
    env.file("photo.jpg", "p");
    env.file("notes.txt", "n");
    let before = env.tree();
    let out = env.run(batch(vec![
        BatchItem::Move { from: env.p("photo.jpg"), to: env.p("photos/2026/photo.jpg") },
        BatchItem::CreateDir { path: env.p("photos/2026") },
        BatchItem::CreateDir { path: env.p("photos") },
        BatchItem::Copy { from: env.p("notes.txt"), to: env.p("photos/notes copy.txt") },
        BatchItem::Move { from: env.p("notes.txt"), to: env.p("todo.txt") },
        BatchItem::CreateFile { path: env.p("photos/README") },
    ]));
    assert_eq!(out.error, None);
    assert_eq!(fs::read_to_string(env.p("photos/2026/photo.jpg")).unwrap(), "p");
    assert_eq!(fs::read_to_string(env.p("photos/notes copy.txt")).unwrap(), "n", "copied from its new place");
    assert!(env.p("todo.txt").exists() && env.p("photos/README").exists());
    while env.ops.undo_label().is_some() {
        env.undo();
    }
    assert_eq!(env.tree(), before);
}

#[test]
fn batch_trash_frees_the_name_for_a_new_item() {
    let env = Env::new();
    env.file("draft.md", "old");
    let out = env.run(batch(vec![
        BatchItem::Trash { path: env.p("draft.md") },
        BatchItem::CreateFile { path: env.p("draft.md") },
    ]));
    assert_eq!(out.error, None);
    assert_eq!(fs::read_to_string(env.p("draft.md")).unwrap(), "");
    assert_eq!(fs::read_to_string(env.trash.join("draft.md")).unwrap(), "old");
}

#[test]
fn batch_problems_are_caught_before_anything_changes() {
    let env = Env::new();
    env.file("a.txt", "a");
    env.file("b.txt", "b");
    env.file("docs/x.txt", "x");
    let before = env.tree();
    let cases = [
        (vec![BatchItem::Move { from: env.p("a.txt"), to: env.p("b.txt") }], "already exists"),
        (
            vec![
                BatchItem::Move { from: env.p("a.txt"), to: env.p("c.txt") },
                BatchItem::CreateFile { path: env.p("C.txt") },
            ],
            "both be called",
        ),
        (vec![BatchItem::CreateFile { path: env.p("nope/new.txt") }], "doesn't exist"),
        (
            vec![
                BatchItem::Move { from: env.p("docs"), to: env.p("documents") },
                BatchItem::Move { from: env.p("a.txt"), to: env.p("docs/a.txt") },
            ],
            "same edit",
        ),
        (vec![BatchItem::Trash { path: env.p("gone.txt") }], "no longer exists"),
    ];
    for (items, expected) in cases {
        let out = env.run(batch(items));
        let error = out.error.unwrap_or_default();
        assert!(error.contains(expected), "expected “{expected}”, got “{error}”");
        assert_eq!(env.tree(), before);
    }
}

#[test]
fn clashes_can_replace_or_skip() {
    let env = Env::new();
    env.file("a/report.pdf", "new");
    env.file("a/notes.txt", "notes");
    env.file("dest/report.pdf", "old");
    let before = env.tree();
    let sources = vec![env.p("a/report.pdf"), env.p("a/notes.txt")];
    let request = |clash| Request::Move { sources: sources.clone(), into: env.p("dest"), clash };
    assert_eq!(clashes(&request(Clash::Replace)), [env.p("a/report.pdf")]);

    let out = env.run(request(Clash::Skip));
    assert_eq!((out.completed, out.skipped, out.error), (1, 1, None));
    assert_eq!(fs::read_to_string(env.p("dest/report.pdf")).unwrap(), "old");
    assert!(env.p("a/report.pdf").exists() && env.p("dest/notes.txt").exists());
    env.undo();
    assert_eq!(env.tree(), before);

    let out = env.run(request(Clash::Replace));
    assert_eq!((out.replaced, out.renamed, out.error), (1, 0, None));
    assert_eq!(fs::read_to_string(env.p("dest/report.pdf")).unwrap(), "new");
    assert_eq!(fs::read_to_string(env.trash.join("report.pdf")).unwrap(), "old", "replaced item is in the Trash");
    env.undo();
    assert_eq!(env.tree(), before, "undo puts the replaced item back");

    // Copies into their own folder are duplicates, never clashes.
    let dup = Request::Copy { sources: vec![env.p("a/notes.txt")], into: env.p("a"), clash: Clash::Replace };
    assert!(clashes(&dup).is_empty());
    assert_eq!(env.run(dup).renamed, 1);
    assert!(env.p("a/notes 2.txt").exists());

    // A folder can't be replaced by something inside it.
    env.file("x/x/inner.txt", "i");
    let out = env.run(Request::Move { sources: vec![env.p("x/x")], into: env.root.clone(), clash: Clash::Replace });
    assert!(out.error.unwrap().contains("inside it"));
}

#[test]
fn tags_add_and_remove_and_undo() {
    let env = Env::new();
    env.file("a.txt", "a");
    env.file("b.txt", "b");
    fsops::set_tags(&env.p("b.txt"), &["Old".into(), "Keep".into()]).unwrap();
    let sorted = |p: &str| {
        let mut t = tags(&env.p(p)).unwrap();
        t.sort();
        t
    };
    let out = env.run(Request::Tag {
        sources: vec![env.p("a.txt"), env.p("b.txt")],
        add: vec!["Work".into(), " Red ".into()],
        remove: vec!["old".into()],
    });
    assert_eq!((out.completed, out.error.clone()), (2, None));
    assert_eq!(out.label, "Tag 2 items");
    assert_eq!(sorted("a.txt"), ["Red", "Work"]);
    assert_eq!(sorted("b.txt"), ["Keep", "Red", "Work"]);

    // Nothing to change is skipped.
    let out = env.run(Request::Tag { sources: vec![env.p("a.txt")], add: vec!["work".into()], remove: vec![] });
    assert_eq!((out.completed, out.skipped), (0, 1));

    env.undo();
    assert!(sorted("a.txt").is_empty());
    assert_eq!(sorted("b.txt"), ["Keep", "Old"]);

    let bad = env.run(Request::Tag { sources: vec![env.p("a.txt")], add: vec!["a,b".into()], remove: vec![] });
    assert!(bad.error.unwrap().contains("commas"));
}

#[test]
fn history_survives_a_restart() {
    let env = Env::new();
    let history = env.root.parent().unwrap().join("state/undo.json");
    let start = || {
        let (tx, rx) = mpsc::channel();
        let ops = Operations::with_history(
            Box::new(FolderTrash(env.trash.clone())),
            Arc::new(Collect(Mutex::new(tx))),
            history.clone(),
        );
        (ops, rx)
    };
    env.file("a.txt", "a");
    env.file("b.txt", "b");
    let before = env.tree();
    {
        let (ops, rx) = start();
        ops.submit(Request::Rename { source: env.p("a.txt"), name: "c.txt".into() });
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        ops.submit(Request::Trash { sources: vec![env.p("b.txt")] });
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
    }
    let (ops, rx) = start();
    assert_eq!(ops.undo_label().as_deref(), Some("Trash “b.txt”"));
    ops.undo();
    assert_eq!(rx.recv_timeout(Duration::from_secs(10)).unwrap().error, None);
    ops.undo();
    assert_eq!(rx.recv_timeout(Duration::from_secs(10)).unwrap().error, None);
    assert_eq!(env.tree(), before);
    drop(ops);

    let (ops, _rx) = start();
    assert_eq!(ops.redo_label().as_deref(), Some("Rename “a.txt”"), "redo survives too");
    assert!(ops.undo_label().is_none());

    // A damaged file is an empty history, not a crash.
    fs::write(&history, "{not json").unwrap();
    let (ops, _rx) = start();
    assert!(ops.undo_label().is_none());
}

#[test]
fn any_operation_in_the_history_can_be_undone_or_redone() {
    let env = Env::new();
    env.file("a.txt", "a");
    env.file("b.txt", "b");
    env.file("c.txt", "c");
    env.run(Request::Rename { source: env.p("a.txt"), name: "a2.txt".into() });
    env.run(Request::Trash { sources: vec![env.p("b.txt")] });
    env.run(Request::Rename { source: env.p("c.txt"), name: "c2.txt".into() });

    let h = env.ops.history();
    assert_eq!(
        h.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(),
        ["Rename “c.txt”", "Trash “b.txt”", "Rename “a.txt”"]
    );
    assert!(h.iter().all(|i| i.state == RecordState::Applied && i.can_undo && !i.can_redo));

    // Undo the oldest one, leaving the later ones in place.
    let first = h[2].id;
    env.ops.undo_record(first).unwrap();
    assert_eq!(env.wait().error, None);
    assert!(env.p("a.txt").exists() && env.p("c2.txt").exists() && !env.p("b.txt").exists());
    assert_eq!(env.ops.undo_label().as_deref(), Some("Rename “c.txt”"), "⌘Z still undoes the newest");

    // A new operation clears quick redo, but the undone record can still be redone.
    env.run(Request::NewFolder { parent: env.root.clone(), name: "new".into() });
    assert!(env.ops.redo_label().is_none());
    let undone = env.ops.history().into_iter().find(|i| i.id == first).unwrap();
    assert_eq!((undone.state, undone.can_redo), (RecordState::Undone, true));
    assert!(matches!(&undone.changes[..], [Done::Moved { .. }]), "it still shows what it did");
    env.ops.redo_record(first).unwrap();
    assert_eq!(env.wait().error, None);
    assert!(env.p("a2.txt").exists());

    // Undoing what later operations depend on is refused step by step, not forced.
    env.run(Request::Rename { source: env.p("a2.txt"), name: "a3.txt".into() });
    env.ops.undo_record(first).unwrap();
    assert!(env.wait().error.unwrap().contains("no longer exists"));
    assert!(env.p("a3.txt").exists(), "nothing was changed");
    assert!(env.ops.redo_record(first).is_none(), "still applied, so nothing to redo");
}

#[test]
fn a_partly_undone_operation_can_finish_later() {
    let env = Env::new();
    env.file("x/1.txt", "1");
    env.file("x/2.txt", "2");
    env.file("y/.keep", "");
    env.run(Request::Move {
        sources: vec![env.p("x/1.txt"), env.p("x/2.txt")],
        into: env.p("y"),
        clash: Clash::KeepBoth,
    });
    // Something else takes 2.txt's old place, so it can't go back yet.
    env.file("x/2.txt", "other");
    let out = env.undo();
    assert!(out.error.is_some());
    let item = env.ops.history().remove(0);
    assert_eq!((item.state, item.can_undo, item.can_redo), (RecordState::PartlyUndone, true, false));
    fs::remove_file(env.p("x/2.txt")).unwrap();
    assert_eq!(env.undo().error, None);
    assert_eq!(env.ops.history()[0].state, RecordState::Undone);
    assert_eq!(fs::read_to_string(env.p("x/2.txt")).unwrap(), "2");
}

#[test]
fn the_first_history_format_is_upgraded() {
    let env = Env::new();
    env.file("b.txt", "b");
    let history = env.root.parent().unwrap().join("undo.json");
    let trashed = env.trash.join("b.txt");
    fs::rename(env.p("b.txt"), &trashed).unwrap();
    let old = serde_json::json!({
        "undo": [{"request": {"Trash": {"sources": [env.p("b.txt")]}},
                  "done": [{"Trashed": {"original": env.p("b.txt"), "in_trash": trashed}}], "at": history::now()}],
        "redo": [{"NewFolder": {"parent": env.root, "name": "gone"}}]
    });
    fs::write(&history, old.to_string()).unwrap();
    let (tx, rx) = mpsc::channel();
    let ops = Operations::with_history(
        Box::new(FolderTrash(env.trash.clone())),
        Arc::new(Collect(Mutex::new(tx))),
        history.clone(),
    );
    assert_eq!(ops.undo_label().as_deref(), Some("Trash “b.txt”"));
    assert_eq!(ops.redo_label().as_deref(), Some("New Folder “gone”"));
    ops.undo();
    assert_eq!(rx.recv_timeout(Duration::from_secs(10)).unwrap().error, None);
    assert!(env.p("b.txt").exists());
}
