//! A small msgpack-rpc client for Neovim's `--listen` socket.
//!
//! Messages are arrays: requests `[0, id, method, params]`, responses `[1, id, error,
//! result]` and notifications `[2, method, params]`. Both sides send requests: Tilde calls
//! the API, and the plugin calls back with `rpcrequest` / `rpcnotify`. A reader thread
//! routes responses to whoever is waiting and hands incoming calls to a [`Handler`].
//!
//! Neovim waits for our reply while it handles a request from the plugin, so a handler
//! must not make requests of its own (it can send notifications).

use std::collections::HashMap;
use std::io::{BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rmpv::Value;

/// Calls from Neovim.
pub trait Handler: Send + Sync {
    /// Answers `rpcrequest(chan, method, ...)`: `Ok` becomes the return value, `Err` an
    /// error raised in Lua.
    fn request(&self, method: &str, args: Vec<Value>) -> Result<Value, String>;
    fn notify(&self, method: &str, args: Vec<Value>);
    /// The connection closed (Neovim exited).
    fn closed(&self);
}

type Reply = Result<Value, String>;

struct Inner {
    writer: Mutex<UnixStream>,
    pending: Mutex<HashMap<u64, Sender<Reply>>>,
    next_id: AtomicU64,
    open: AtomicBool,
}

#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

/// How long to wait for a reply before giving up (Neovim may be busy loading a config).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

impl Client {
    /// Connects to `socket`, retrying until it exists and accepts or `timeout` passes.
    pub fn connect(socket: &Path, timeout: Duration, handler: Arc<dyn Handler>) -> Result<Client, String> {
        let start = Instant::now();
        let stream = loop {
            match UnixStream::connect(socket) {
                Ok(s) => break s,
                Err(e) if start.elapsed() >= timeout => {
                    return Err(format!("couldn't connect to Neovim at {}: {e}", socket.display()));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        };
        let reader = stream.try_clone().map_err(|e| e.to_string())?;
        let client = Client {
            inner: Arc::new(Inner {
                writer: Mutex::new(stream),
                pending: Mutex::new(HashMap::new()),
                next_id: AtomicU64::new(1),
                open: AtomicBool::new(true),
            }),
        };
        let c = client.clone();
        std::thread::Builder::new()
            .name("tilde-nvim-rpc".into())
            .spawn(move || c.read_loop(reader, handler))
            .map_err(|e| e.to_string())?;
        Ok(client)
    }

    pub fn is_open(&self) -> bool {
        self.inner.open.load(Ordering::Acquire)
    }

    /// Calls an API method and waits for the result.
    pub fn request(&self, method: &str, args: Vec<Value>) -> Result<Value, String> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = channel();
        self.inner.pending.lock().unwrap().insert(id, tx);
        let msg = Value::Array(vec![0.into(), id.into(), method.into(), Value::Array(args)]);
        if let Err(e) = self.send(&msg) {
            self.inner.pending.lock().unwrap().remove(&id);
            return Err(e);
        }
        match rx.recv_timeout(REQUEST_TIMEOUT) {
            Ok(reply) => reply,
            Err(_) => {
                self.inner.pending.lock().unwrap().remove(&id);
                Err(if self.is_open() { format!("Neovim didn't answer {method}") } else { "Neovim quit".into() })
            }
        }
    }

    /// Calls an API method without waiting (errors show up in Neovim, if anywhere).
    pub fn notify(&self, method: &str, args: Vec<Value>) -> Result<(), String> {
        self.send(&Value::Array(vec![2.into(), method.into(), Value::Array(args)]))
    }

    /// Runs Lua (`...` holds `args`) and returns its result.
    pub fn exec_lua(&self, code: &str, args: Vec<Value>) -> Result<Value, String> {
        self.request("nvim_exec_lua", vec![code.into(), Value::Array(args)])
    }

    /// Like [`Client::exec_lua`] without waiting.
    pub fn exec_lua_async(&self, code: &str, args: Vec<Value>) -> Result<(), String> {
        self.notify("nvim_exec_lua", vec![code.into(), Value::Array(args)])
    }

    fn send(&self, msg: &Value) -> Result<(), String> {
        let mut buf = Vec::new();
        rmpv::encode::write_value(&mut buf, msg).map_err(|e| e.to_string())?;
        let mut w = self.inner.writer.lock().unwrap();
        w.write_all(&buf).map_err(|e| format!("Neovim connection: {e}"))
    }

    fn read_loop(&self, stream: UnixStream, handler: Arc<dyn Handler>) {
        let mut reader = BufReader::new(stream);
        while let Ok(msg) = rmpv::decode::read_value(&mut reader) {
            let Value::Array(parts) = msg else { continue };
            let kind = parts.first().and_then(Value::as_u64);
            match (kind, parts.len()) {
                (Some(0), 4) => {
                    let id = parts[1].clone();
                    let method = parts[2].as_str().unwrap_or_default().to_string();
                    let reply = match handler.request(&method, into_args(parts[3].clone())) {
                        Ok(v) => Value::Array(vec![1.into(), id, Value::Nil, v]),
                        Err(e) => Value::Array(vec![1.into(), id, e.into(), Value::Nil]),
                    };
                    let _ = self.send(&reply);
                }
                (Some(1), 4) => {
                    let Some(id) = parts[1].as_u64() else { continue };
                    let reply = if parts[2].is_nil() { Ok(parts[3].clone()) } else { Err(error_text(&parts[2])) };
                    if let Some(tx) = self.inner.pending.lock().unwrap().remove(&id) {
                        let _ = tx.send(reply);
                    }
                }
                (Some(2), 3) => {
                    let method = parts[1].as_str().unwrap_or_default().to_string();
                    handler.notify(&method, into_args(parts[2].clone()));
                }
                _ => {}
            }
        }
        self.inner.open.store(false, Ordering::Release);
        self.inner.pending.lock().unwrap().clear();
        handler.closed();
    }
}

fn into_args(v: Value) -> Vec<Value> {
    match v {
        Value::Array(a) => a,
        other => vec![other],
    }
}

/// API errors are `[type, message]`.
fn error_text(v: &Value) -> String {
    match v {
        Value::Array(a) if a.len() == 2 => a[1].as_str().unwrap_or_default().to_string(),
        Value::String(s) => s.as_str().unwrap_or_default().to_string(),
        other => other.to_string(),
    }
}
