//! Neovim as the editor: the app hosts the process in a terminal view, this side talks to
//! it. Calls that wait on nvim run on their own threads so the main thread never blocks.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tilde_config::NvimConfigMode;
use tilde_nvim::{ConfigMode, Editor, Events, OpenMode};

use crate::edit::{EditPreview, EditSession};
use crate::ops::OpError;

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct NvimLaunch {
    pub program: String,
    /// Without the program name.
    pub args: Vec<String>,
    /// Added to the app's environment.
    pub env: HashMap<String, String>,
    /// nvim found, for messages: `0.11.2`.
    pub version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum NvimOpenMode {
    /// The whole folder, normal mode.
    Edit,
    /// Insert mode with the name's stem selected; `⏎` applies.
    Rename,
    /// Insert mode on a new line below; `⏎` applies.
    New,
}

impl From<NvimOpenMode> for OpenMode {
    fn from(m: NvimOpenMode) -> Self {
        match m {
            NvimOpenMode::Edit => OpenMode::Edit,
            NvimOpenMode::Rename => OpenMode::Rename,
            NvimOpenMode::New => OpenMode::New,
        }
    }
}

/// Called on background threads.
#[uniffi::export(with_foreign)]
pub trait NvimListener: Send + Sync {
    /// Connected and the session (if one was waiting) is open.
    fn nvim_ready(&self);
    /// Couldn't connect or open the session.
    fn nvim_failed(&self, message: String);
    fn nvim_preview(&self, preview: EditPreview);
    /// `:w` with no errors: apply `text`, then call `close` or `error`.
    fn nvim_apply(&self, text: String);
    /// `:q`.
    fn nvim_cancelled(&self);
    /// The connection dropped (nvim exited).
    fn nvim_disconnected(&self);
}

type Pending = (Arc<EditSession>, Option<u32>, NvimOpenMode);

#[derive(Default)]
struct Shared {
    session: Option<Arc<EditSession>>,
    /// Opened once connected.
    pending: Option<Pending>,
    socket: Option<PathBuf>,
}

struct Bridge {
    shared: Arc<Mutex<Shared>>,
    listener: Arc<dyn NvimListener>,
}

impl Events for Bridge {
    fn preview(&self, text: String) {
        let session = self.shared.lock().unwrap().session.clone();
        if let Some(s) = session {
            self.listener.nvim_preview(s.preview(text));
        }
    }
    fn apply(&self, text: String) {
        self.listener.nvim_apply(text);
    }
    fn cancel(&self) {
        self.shared.lock().unwrap().session = None;
        self.listener.nvim_cancelled();
    }
    fn disconnected(&self) {
        self.listener.nvim_disconnected();
    }
}

/// One window's nvim.
#[derive(uniffi::Object)]
pub struct NvimEditor {
    editor: Editor,
    shared: Arc<Mutex<Shared>>,
    listener: Arc<dyn NvimListener>,
}

static SOCKETS: AtomicU64 = AtomicU64::new(0);

#[uniffi::export]
impl NvimEditor {
    #[uniffi::constructor]
    pub fn new(listener: Arc<dyn NvimListener>) -> Arc<Self> {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let editor = Editor::new(Arc::new(Bridge { shared: shared.clone(), listener: listener.clone() }));
        Arc::new(NvimEditor { editor, shared, listener })
    }

    /// How to start nvim, per the config. Fails if nvim can't be found.
    pub fn launch_spec(&self) -> Result<NvimLaunch, OpError> {
        let config = tilde_config::load(&tilde_config::config_path()).config.edit;
        let found = tilde_nvim::find(&config.nvim_path).map_err(|message| OpError::Invalid { message })?;
        let mode = match config.nvim_config {
            NvimConfigMode::User => ConfigMode::User,
            NvimConfigMode::Clean => ConfigMode::Clean,
        };
        let socket = tilde_nvim::socket_path(SOCKETS.fetch_add(1, Ordering::Relaxed));
        let _ = std::fs::remove_file(&socket);
        let spec = tilde_nvim::launch_spec(&found, mode, &tilde_config::nvim_init_path(), &socket);
        self.shared.lock().unwrap().socket = Some(socket);
        Ok(NvimLaunch {
            program: spec.program.to_string_lossy().into_owned(),
            args: spec.args,
            env: spec.env.into_iter().collect(),
            version: found.version,
        })
    }

    /// The app started the process from `launch_spec`: connect, then open any waiting
    /// session.
    pub fn process_started(&self) {
        let Some(socket) = self.shared.lock().unwrap().socket.clone() else { return };
        let editor = self.editor.clone();
        let shared = self.shared.clone();
        let listener = self.listener.clone();
        std::thread::spawn(move || {
            if let Err(e) = editor.connect(&socket) {
                shared.lock().unwrap().pending = None;
                return listener.nvim_failed(e);
            }
            let pending = shared.lock().unwrap().pending.take();
            if let Some((session, line, mode)) = pending
                && let Err(e) = editor.open(session.inner.clone(), line.map(|l| l as usize), mode.into())
            {
                return listener.nvim_failed(e);
            }
            listener.nvim_ready();
        });
    }

    pub fn is_connected(&self) -> bool {
        self.editor.is_connected()
    }

    /// Opens `session` in nvim on line `line` (0-based), now or once connected.
    pub fn edit(&self, session: Arc<EditSession>, line: Option<u32>, mode: NvimOpenMode) {
        {
            let mut sh = self.shared.lock().unwrap();
            sh.session = Some(session.clone());
            if !self.editor.is_connected() {
                sh.pending = Some((session, line, mode));
                return;
            }
        }
        let editor = self.editor.clone();
        let listener = self.listener.clone();
        std::thread::spawn(move || match editor.open(session.inner.clone(), line.map(|l| l as usize), mode.into()) {
            Ok(()) => listener.nvim_ready(),
            Err(e) => listener.nvim_failed(e),
        });
    }

    /// Tilde's ⌘S: `:w`.
    pub fn write(&self) {
        self.editor.write();
    }

    /// Ends the session (applied, or cancelled from the app).
    pub fn close(&self) {
        self.shared.lock().unwrap().session = None;
        self.editor.close();
    }

    /// Applying failed: tell the user in nvim and keep editing.
    pub fn error(&self, message: String) {
        self.editor.error(&message);
    }

    /// The process exited.
    pub fn process_exited(&self) {
        self.editor.disconnect();
        let mut sh = self.shared.lock().unwrap();
        sh.session = None;
        sh.pending = None;
        if let Some(socket) = sh.socket.take() {
            let _ = std::fs::remove_file(socket);
        }
    }
}
