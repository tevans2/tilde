//! Drives a real `nvim --headless` through the plugin: edits made with normal-mode
//! commands and typed keys, then `:w` and `:q`. Skipped when nvim isn't installed.

use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmpv::Value;
use tilde_edit::{Change, Session};
use tilde_nvim::{Client, Editor, Events, Handler, OpenMode};

#[derive(Debug, PartialEq)]
enum Event {
    Preview(String),
    Apply(String),
    Cancel,
    Disconnected,
}

struct Recorder(Mutex<Sender<Event>>);

impl Events for Recorder {
    fn preview(&self, text: String) {
        let _ = self.0.lock().unwrap().send(Event::Preview(text));
    }
    fn apply(&self, text: String) {
        let _ = self.0.lock().unwrap().send(Event::Apply(text));
    }
    fn cancel(&self) {
        let _ = self.0.lock().unwrap().send(Event::Cancel);
    }
    fn disconnected(&self) {
        let _ = self.0.lock().unwrap().send(Event::Disconnected);
    }
}

struct Ignore;

impl Handler for Ignore {
    fn request(&self, _: &str, _: Vec<Value>) -> Result<Value, String> {
        Ok(Value::Nil)
    }
    fn notify(&self, _: &str, _: Vec<Value>) {}
    fn closed(&self) {}
}

struct Nvim {
    child: Child,
    _dir: tempfile::TempDir,
    editor: Editor,
    events: Receiver<Event>,
    /// A second connection for driving nvim like a user would.
    driver: Client,
}

impl Drop for Nvim {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start() -> Option<Nvim> {
    let found = tilde_nvim::find("").ok()?;
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("nvim.sock");
    let child = Command::new(&found.path).args(["--headless", "--clean", "--listen"]).arg(&socket).spawn().unwrap();
    let (tx, rx) = channel();
    let editor = Editor::new(Arc::new(Recorder(Mutex::new(tx))));
    editor.connect(&socket).unwrap();
    let driver = Client::connect(&socket, Duration::from_secs(2), Arc::new(Ignore)).unwrap();
    Some(Nvim { child, _dir: dir, editor, events: rx, driver })
}

fn session() -> Session {
    Session::new(
        PathBuf::from("/Users/me/dl"),
        PathBuf::from("/Users/me"),
        vec![
            ("photos".into(), true),
            ("contract.pdf".into(), false),
            ("dataset.zip".into(), false),
            ("notes.md".into(), false),
        ],
    )
}

impl Nvim {
    fn cmd(&self, c: &str) {
        self.driver.request("nvim_command", vec![c.into()]).unwrap();
    }

    fn input(&self, keys: &str) {
        self.driver.request("nvim_input", vec![keys.into()]).unwrap();
    }

    fn eval(&self, expr: &str) -> Value {
        self.driver.request("nvim_eval", vec![expr.into()]).unwrap()
    }

    /// The next event other than a preview.
    fn next(&self) -> Event {
        loop {
            match self.events.recv_timeout(Duration::from_secs(5)).expect("no event from nvim") {
                Event::Preview(_) => continue,
                e => return e,
            }
        }
    }

    fn next_preview(&self) -> String {
        loop {
            if let Event::Preview(t) = self.events.recv_timeout(Duration::from_secs(5)).expect("no preview") {
                return t;
            }
        }
    }

    /// Waits for typed keys to be handled (nvim_input returns before they are).
    fn settle(&self) {
        for _ in 0..100 {
            if self.eval("mode(1)").as_str() == Some("n") && self.eval("getchar(1)").as_i64() == Some(0) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[test]
fn edits_with_normal_commands_then_write() {
    let Some(nv) = start() else { return eprintln!("nvim not found; skipped") };
    let s = session();
    nv.editor.open(s.clone(), Some(1), OpenMode::Edit).unwrap();
    assert_eq!(nv.eval("&filetype").as_str(), Some("tilde"));
    assert_eq!(nv.eval("line('.')").as_i64(), Some(2));
    // The ID prefix is concealed and the cursor starts after it.
    assert_eq!(nv.eval("col('.')").as_i64(), Some(6));
    assert_eq!(nv.eval("&conceallevel").as_i64(), Some(3));

    nv.cmd("2normal! A-signed");
    nv.cmd("3normal! yyp");
    nv.cmd("4normal! A.bak");
    nv.cmd("5delete");
    nv.cmd("$put ='inbox/'");
    let preview = nv.next_preview();
    assert!(preview.contains("contract.pdf-signed"), "{preview}");

    nv.cmd("write");
    let Event::Apply(text) = nv.next() else { panic!("expected apply") };
    let diff = s.diff(&text);
    assert!(diff.errors.is_empty(), "{:?}", diff.errors);
    let dl = |n: &str| PathBuf::from("/Users/me/dl").join(n);
    assert_eq!(
        diff.changes,
        vec![
            Change::Rename { from: dl("contract.pdf"), to: dl("contract.pdf-signed") },
            Change::Copy { from: dl("dataset.zip"), to: dl("dataset.zip.bak") },
            Change::CreateDir { path: dl("inbox") },
            Change::Trash { path: dl("notes.md") },
        ]
    );
    assert_eq!(nv.eval("&modified").as_i64(), Some(0));

    // The app applied it and closes the session: no cancel event follows.
    nv.editor.close();
    std::thread::sleep(Duration::from_millis(100));
    assert!(nv.eval("bufname()").as_str().is_some_and(|b| !b.starts_with("tilde://")));
    assert!(nv.events.try_iter().all(|e| matches!(e, Event::Preview(_))));
}

#[test]
fn errors_block_the_write_and_keep_the_buffer() {
    let Some(nv) = start() else { return eprintln!("nvim not found; skipped") };
    nv.editor.open(session(), None, OpenMode::Edit).unwrap();
    // Two lines with the same name.
    nv.cmd("3normal! yyp");
    // The error is shown in nvim (and fails the API call that wrote).
    let e = nv.driver.request("nvim_command", vec!["write".into()]).unwrap_err();
    assert!(e.contains("fix line 4 first"), "{e}");
    std::thread::sleep(Duration::from_millis(200));
    assert!(nv.events.try_iter().all(|e| matches!(e, Event::Preview(_))), "nothing should be applied");
    assert_eq!(nv.eval("&modified").as_i64(), Some(1));
    // The bad line is marked.
    let diags = nv.eval("luaeval('#vim.diagnostic.get(0)')");
    assert_eq!(diags.as_i64(), Some(1));

    // An apply that fails in the app reports back the same way.
    nv.cmd("4delete");
    nv.cmd("write");
    assert!(matches!(nv.next(), Event::Apply(_)));
    nv.editor.error("couldn't move: permission denied");
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(nv.eval("&modified").as_i64(), Some(1));
    assert!(nv.eval("bufname()").as_str().is_some_and(|b| b.starts_with("tilde://")));
}

#[test]
fn typed_quit_cancels_without_quitting_nvim() {
    let Some(nv) = start() else { return eprintln!("nvim not found; skipped") };
    nv.editor.open(session(), None, OpenMode::Edit).unwrap();
    nv.cmd("2delete");
    nv.input(":q!<CR>");
    assert_eq!(nv.next(), Event::Cancel);
    nv.settle();
    assert!(nv.eval("bufname()").as_str().is_some_and(|b| !b.starts_with("tilde://")));
    assert!(nv.editor.is_connected());

    // The next session works in the same nvim.
    nv.editor.open(session(), None, OpenMode::Edit).unwrap();
    nv.input("ZQ");
    assert_eq!(nv.next(), Event::Cancel);
}

#[test]
fn rename_mode_replaces_the_stem_and_enter_applies() {
    let Some(nv) = start() else { return eprintln!("nvim not found; skipped") };
    let s = session();
    nv.editor.open(s.clone(), Some(1), OpenMode::Rename).unwrap();
    nv.input("signed<CR>");
    let Event::Apply(text) = nv.next() else { panic!("expected apply") };
    let dl = |n: &str| PathBuf::from("/Users/me/dl").join(n);
    assert_eq!(s.diff(&text).changes, vec![Change::Rename { from: dl("contract.pdf"), to: dl("signed.pdf") }]);
}

#[test]
fn new_mode_opens_a_line_below() {
    let Some(nv) = start() else { return eprintln!("nvim not found; skipped") };
    let s = session();
    nv.editor.open(s.clone(), Some(0), OpenMode::New).unwrap();
    nv.input("todo.txt<CR>");
    let Event::Apply(text) = nv.next() else { panic!("expected apply") };
    assert_eq!(s.diff(&text).changes, vec![Change::CreateFile { path: PathBuf::from("/Users/me/dl/todo.txt") }]);
    assert_eq!(text.lines().nth(1), Some("todo.txt"));
}

#[test]
fn cursor_stays_out_of_the_prefix() {
    let Some(nv) = start() else { return eprintln!("nvim not found; skipped") };
    nv.editor.open(session(), Some(2), OpenMode::Edit).unwrap();
    nv.input("0");
    nv.settle();
    assert_eq!(nv.eval("col('.')").as_i64(), Some(6));
}

#[test]
fn nvim_exiting_disconnects() {
    let Some(mut nv) = start() else { return eprintln!("nvim not found; skipped") };
    nv.editor.open(session(), None, OpenMode::Edit).unwrap();
    nv.child.kill().unwrap();
    assert_eq!(nv.next(), Event::Disconnected);
    assert!(!nv.editor.is_connected());
    assert!(nv.editor.open(session(), None, OpenMode::Edit).is_err());
}
