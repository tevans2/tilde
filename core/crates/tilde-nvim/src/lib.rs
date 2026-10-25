//! Neovim as the EDIT editor (see docs/SPEC.md §6.9).
//!
//! The app runs `nvim --listen <socket>` in a terminal view; [`Editor`] connects to the
//! socket, loads the bundled plugin (`lua/tilde.lua`) and opens folder sessions in it. The
//! plugin reports edits (`tilde_preview`), `:w` (`tilde_apply`) and `:q` (`tilde_cancel`),
//! which become [`Events`]. One nvim serves a window's sessions, one at a time.

mod discover;
mod rpc;

pub use discover::{Found, MIN_VERSION, find, login_path};
pub use rpc::{Client, Handler};

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmpv::Value;
use tilde_edit::Session;

pub const PLUGIN: &str = include_str!("../lua/tilde.lua");
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Whether nvim loads the user's config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigMode {
    User,
    /// `--clean`, plus `init` (Tilde's own nvim config) if it exists.
    Clean,
}

/// How the app should start nvim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    pub program: PathBuf,
    /// Without the program name.
    pub args: Vec<String>,
    /// Set on top of the app's environment.
    pub env: Vec<(String, String)>,
    pub socket: PathBuf,
}

/// The command line for `found`, listening on `socket`.
pub fn launch_spec(found: &Found, mode: ConfigMode, init: &Path, socket: &Path) -> LaunchSpec {
    let mut args = vec!["--listen".to_string(), socket.to_string_lossy().into_owned()];
    if mode == ConfigMode::Clean {
        args.push("--clean".into());
        if init.is_file() {
            args.extend(["-u".into(), init.to_string_lossy().into_owned()]);
        }
    }
    let mut env = vec![
        ("TERM".into(), "xterm-256color".into()),
        ("COLORTERM".into(), "truecolor".into()),
        ("TERM_PROGRAM".into(), "Tilde".into()),
    ];
    if let Some(path) = login_path() {
        env.push(("PATH".into(), path.into()));
    }
    if std::env::var_os("LANG").is_none() {
        env.push(("LANG".into(), "en_US.UTF-8".into()));
    }
    LaunchSpec { program: found.path.clone(), args, env, socket: socket.to_path_buf() }
}

/// A fresh socket path for one nvim: `$TMPDIR/tilde-<pid>-<n>.sock`.
pub fn socket_path(n: u64) -> PathBuf {
    std::env::temp_dir().join(format!("tilde-{}-{n}.sock", std::process::id()))
}

/// How a session starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenMode {
    /// Normal mode on the line.
    Edit,
    /// Insert mode with the name's stem selected; `⏎` applies.
    Rename,
    /// Insert mode on a new line below; `⏎` applies.
    New,
}

/// What the plugin reports.
pub trait Events: Send + Sync {
    /// The buffer changed (debounced).
    fn preview(&self, text: String);
    /// `:w` with no errors in the text. The app applies it, then calls [`Editor::close`]
    /// or [`Editor::error`].
    fn apply(&self, text: String);
    /// `:q` (or the buffer was closed).
    fn cancel(&self);
    /// nvim went away.
    fn disconnected(&self);
}

#[derive(Default)]
struct State {
    client: Option<Client>,
    chan: i64,
    session: Option<Session>,
}

/// The connection to one nvim and the session open in it.
#[derive(Clone)]
pub struct Editor {
    state: Arc<Mutex<State>>,
    events: Arc<dyn Events>,
}

impl Editor {
    pub fn new(events: Arc<dyn Events>) -> Editor {
        Editor { state: Arc::default(), events }
    }

    pub fn is_connected(&self) -> bool {
        self.state.lock().unwrap().client.as_ref().is_some_and(Client::is_open)
    }

    /// Connects to a starting nvim and loads the plugin. Blocks until it's ready (up to a
    /// couple of seconds for the socket, longer if a heavy config is still loading).
    pub fn connect(&self, socket: &Path) -> Result<(), String> {
        let handler = Arc::new(PluginCalls { editor: self.clone() });
        let client = Client::connect(socket, CONNECT_TIMEOUT, handler)?;
        let info = client.request("nvim_get_api_info", vec![])?;
        let chan = info.as_array().and_then(|a| a.first()).and_then(Value::as_i64).ok_or("unexpected api info")?;
        client.exec_lua(&format!("package.loaded.tilde = (function()\n{PLUGIN}\nend)()"), vec![])?;
        let mut st = self.state.lock().unwrap();
        st.client = Some(client);
        st.chan = chan;
        Ok(())
    }

    /// Forgets the connection (nvim exited).
    pub fn disconnect(&self) {
        let mut st = self.state.lock().unwrap();
        st.client = None;
        st.session = None;
    }

    /// Opens `session` in nvim, on line `line` (0-based).
    pub fn open(&self, session: Session, line: Option<usize>, mode: OpenMode) -> Result<(), String> {
        let text = session.text();
        let (client, chan) = {
            let mut st = self.state.lock().unwrap();
            st.session = Some(session.clone());
            (st.client.clone().ok_or("Neovim isn't running")?, st.chan)
        };
        let lines: Vec<Value> = if text.is_empty() { vec![] } else { text.split('\n').map(Value::from).collect() };
        let mode = match mode {
            OpenMode::Edit => "edit",
            OpenMode::Rename => "rename",
            OpenMode::New => "new",
        };
        let opts = Value::Map(vec![
            ("chan".into(), chan.into()),
            ("dir".into(), session.dir().to_string_lossy().as_ref().into()),
            ("lines".into(), Value::Array(lines)),
            ("line".into(), (line.unwrap_or(0) as u64 + 1).into()),
            ("mode".into(), mode.into()),
        ]);
        client.exec_lua("require('tilde').open(...)", vec![opts]).map(|_| ())
    }

    /// Writes the buffer, as `:w` does (Tilde's ⌘S).
    pub fn write(&self) {
        self.call_async("require('tilde').write()", vec![]);
    }

    /// Ends the session without a cancel event: after applying, or when the app cancels.
    pub fn close(&self) {
        self.state.lock().unwrap().session = None;
        self.call_async("require('tilde').close()", vec![]);
    }

    /// Applying failed: shows `message` in nvim and keeps the session open.
    pub fn error(&self, message: &str) {
        self.call_async("require('tilde').error(...)", vec![message.into()]);
    }

    fn call_async(&self, code: &str, args: Vec<Value>) {
        if let Some(client) = self.state.lock().unwrap().client.clone() {
            let _ = client.exec_lua_async(code, args);
        }
    }

    fn session(&self) -> Option<Session> {
        self.state.lock().unwrap().session.clone()
    }
}

struct PluginCalls {
    editor: Editor,
}

/// The buffer's lines, from the plugin's argument list.
fn text_of(args: &[Value]) -> String {
    let lines = args.first().and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    lines.iter().map(|l| l.as_str().unwrap_or_default()).collect::<Vec<_>>().join("\n")
}

impl Handler for PluginCalls {
    fn request(&self, method: &str, args: Vec<Value>) -> Result<Value, String> {
        match method {
            "tilde_apply" => {
                let Some(session) = self.editor.session() else {
                    return Ok("Tilde: this edit has ended; nothing was applied.".into());
                };
                let text = text_of(&args);
                let diff = session.diff(&text);
                if let Some(e) = diff.errors.first() {
                    let more = match diff.errors.len() {
                        1 => String::new(),
                        n => format!(" (and {} more)", n - 1),
                    };
                    return Ok(format!("Tilde: fix line {} first: {}{more}", e.line + 1, e.message).into());
                }
                self.editor.events.apply(text);
                Ok(Value::Nil)
            }
            other => Err(format!("unknown method {other}")),
        }
    }

    fn notify(&self, method: &str, args: Vec<Value>) {
        match method {
            "tilde_preview" => {
                let text = text_of(&args);
                if let Some(session) = self.editor.session() {
                    let errors: Vec<Value> = session
                        .diff(&text)
                        .errors
                        .into_iter()
                        .map(|e| Value::Array(vec![(e.line as u64).into(), e.message.into()]))
                        .collect();
                    self.editor.call_async("require('tilde').diagnostics(...)", vec![Value::Array(errors)]);
                }
                self.editor.events.preview(text);
            }
            "tilde_cancel" => {
                self.editor.state.lock().unwrap().session = None;
                self.editor.events.cancel();
            }
            _ => {}
        }
    }

    fn closed(&self) {
        self.editor.disconnect();
        self.editor.events.disconnected();
    }
}
