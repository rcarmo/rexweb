//! Runtime ownership of workspace state and Linux PTYs.
use crate::{
    linux::{self, Sandbox},
    paste,
    rex::{Backend, PaneAttachment, PaneUpdate, WireError},
    state::State,
    uploads::UploadRoot,
    vt::{Engine, Terminal},
};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    fs::File,
    io::{Cursor, Read, Write},
    os::fd::AsRawFd,
    path::Path,
    process::Child,
    sync::{
        Arc, Mutex,
        atomic::{AtomicI32, Ordering},
    },
    time::Instant,
};
use tokio::sync::mpsc;
const HISTORY: usize = 512 << 10;
const NATIVE_JSON_BUDGET: usize = 960 << 10;
const NATIVE_BLOB_BUDGET: usize = 4 << 20;
const NATIVE_BLOB_TTL: std::time::Duration = std::time::Duration::from_secs(5 * 60);

#[derive(Default)]
struct BlobStore {
    entries: HashMap<u32, CaptureBlob>,
    next: u32,
}
struct CaptureBlob {
    data: Vec<u8>,
    expires: Instant,
}
impl BlobStore {
    fn insert(&mut self, data: Vec<u8>) -> u32 {
        self.insert_at(data, Instant::now())
    }
    fn insert_at(&mut self, data: Vec<u8>, now: Instant) -> u32 {
        self.entries.retain(|_, blob| now < blob.expires);
        let mut retained: usize = self.entries.values().map(|blob| blob.data.len()).sum();
        while retained + data.len() > NATIVE_BLOB_BUDGET {
            let Some(id) = self.entries.keys().next().copied() else {
                break;
            };
            if let Some(blob) = self.entries.remove(&id) {
                retained -= blob.data.len();
            }
        }
        self.next = self.next.wrapping_add(1);
        if self.next == 0 {
            self.next = 1;
        }
        let mut id = self.next;
        while self.entries.contains_key(&id) {
            id = id.wrapping_add(1);
            if id == 0 {
                id = 1;
            }
        }
        self.next = id;
        self.entries.insert(
            id,
            CaptureBlob {
                data,
                expires: now + NATIVE_BLOB_TTL,
            },
        );
        id
    }
    fn read(&mut self, id: u32, offset: usize, max_bytes: Option<usize>) -> Result<Value> {
        self.read_at(id, offset, max_bytes, Instant::now())
    }
    fn read_at(
        &mut self,
        id: u32,
        offset: usize,
        max_bytes: Option<usize>,
        now: Instant,
    ) -> Result<Value> {
        let expired = self
            .entries
            .get(&id)
            .is_some_and(|blob| now >= blob.expires);
        if expired {
            self.entries.remove(&id);
        }
        let blob = self
            .entries
            .get(&id)
            .ok_or_else(|| missing("capture blob"))?;
        if offset > blob.data.len()
            || (offset < blob.data.len() && !is_utf8_start(blob.data[offset]))
        {
            return Err(wire("bad_request", "invalid blob offset", 400));
        }
        let limit = max_bytes.map_or(NATIVE_JSON_BUDGET, |value| value.min(NATIVE_JSON_BUDGET));
        let end = offset + json_prefix(&blob.data[offset..], limit);
        if end == offset && offset < blob.data.len() {
            return Err(wire("bad_request", "max_bytes (too small)", 400));
        }
        let content = std::str::from_utf8(&blob.data[offset..end])?;
        Ok(json!({"content":content,"total":blob.data.len(),"eof":end==blob.data.len()}))
    }
}

pub struct Service {
    inner: Mutex<Workspace>,
    sandbox: Sandbox,
    shell: String,
    engine: Engine,
    uploads: Mutex<Option<UploadRoot>>,
}
#[derive(Default)]
struct Workspace {
    state: State,
    panes: HashMap<String, Arc<Pane>>,
}
struct Pane {
    input: Mutex<File>,
    child: Mutex<Child>,
    child_pid: AtomicI32,
    state: Mutex<Stream>,
}
struct Stream {
    vt: Terminal,
    history: VecDeque<u8>,
    end: u64,
    consumers: HashMap<String, mpsc::Sender<PaneUpdate>>,
    size: [u16; 4],
    exit: Option<i32>,
    owner: Option<String>,
    blobs: BlobStore,
}
fn wire(code: &'static str, message: &str, status: u16) -> anyhow::Error {
    WireError {
        code,
        message: message.into(),
        status,
    }
    .into()
}
fn missing(kind: &str) -> anyhow::Error {
    wire("not_found", &format!("{kind} not found"), 404)
}
fn field<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or_default()
}
fn input_mode(file: &File) -> Result<Value> {
    // SAFETY: `file` stays alive for the call, and `attrs` is writable storage
    // of exactly the termios type expected by `tcgetattr`.
    let mut attrs: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(file.as_raw_fd(), &raw mut attrs) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(json!({
        "echo": attrs.c_lflag & libc::ECHO != 0,
        "canonical": attrs.c_lflag & libc::ICANON != 0,
        "signals": attrs.c_lflag & libc::ISIG != 0,
    }))
}
impl Service {
    pub fn new(sandbox: Sandbox, shell: String, engine: Engine) -> Self {
        Self {
            inner: Mutex::new(Workspace::default()),
            sandbox,
            shell,
            engine,
            uploads: Mutex::new(None),
        }
    }
    pub fn enable_uploads(&self, path: &Path) -> Result<()> {
        let root = UploadRoot::open(path)?;
        let mut uploads = self.uploads.lock().unwrap();
        ensure!(uploads.is_none(), "uploads are already enabled");
        *uploads = Some(root);
        Ok(())
    }
    pub fn apply(&self, action: &Value) -> Result<Value> {
        let mut inner = self.inner.lock().unwrap();
        let Workspace { state, panes } = &mut *inner;
        let response = state.apply(action, || {
            let id = format!("block:{}", uuid::Uuid::new_v4());
            let vt = self.engine.terminal(80, 24)?;
            panes.insert(id.clone(), Pane::start(&self.sandbox, &self.shell, vt)?);
            Ok(id)
        });
        let live: std::collections::HashSet<_> = state
            .summaries()
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| state.pane_ids(field(s, "session_id")))
            .flatten()
            .collect();
        panes.retain(|id, pane| {
            if live.contains(id) {
                true
            } else {
                pane.stop();
                false
            }
        });
        response
    }
    fn pane(&self, session: &str, block: &str) -> Result<Arc<Pane>> {
        let inner = self.inner.lock().unwrap();
        if !inner.state.contains(session, block) {
            return Err(missing("block"));
        }
        inner
            .panes
            .get(block)
            .cloned()
            .ok_or_else(|| missing("block"))
    }
    pub fn metadata(&self, session: &str, block: &str) -> Result<Value> {
        let pane = self.pane(session, block)?;
        let state = pane.state.lock().unwrap();
        Ok(state.vt.info())
    }
    fn describe_blocks(&self, session: &str, block: &str, single: bool) -> Result<Value> {
        let inner = self.inner.lock().unwrap();
        let view = inner
            .state
            .view(session)
            .ok_or_else(|| missing("session"))?;
        let mut blocks = Vec::new();
        for window in view["windows"].as_array().into_iter().flatten() {
            for item in window["layers"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|layer| layer["blocks"].as_array().into_iter().flatten())
            {
                let id = item["block_id"].as_str().unwrap_or_default();
                if single && id != block {
                    continue;
                }
                let mut value = item.clone();
                value["placement"] = json!("placed");
                value["window_id"] = window["window_id"].clone();
                let owner = inner
                    .panes
                    .get(id)
                    .and_then(|p| p.state.lock().unwrap().owner.clone());
                if let Some(owner) = owner {
                    value["owner"] = json!(owner);
                }
                blocks.push(value);
            }
        }
        if single {
            let block = blocks.into_iter().next().ok_or_else(|| missing("pane"))?;
            return Ok(json!({"block":block}));
        }
        Ok(json!({"blocks":blocks}))
    }
    fn ownership(
        &self,
        client: Option<&str>,
        session: &str,
        args: &Value,
        own: bool,
    ) -> Result<Value> {
        let client = client.ok_or_else(|| wire("conflict", "client identity required", 409))?;
        let inner = self.inner.lock().unwrap();
        let ids: Vec<String> = if let Some(values) = args.get("blocks").and_then(Value::as_array) {
            values
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        } else {
            let id = field(args, "block_id");
            if id.is_empty() {
                inner
                    .state
                    .pane_ids(session)
                    .ok_or_else(|| missing("session"))?
                    .into_iter()
                    .collect()
            } else {
                vec![id.into()]
            }
        };
        let mut panes = Vec::with_capacity(ids.len());
        for id in &ids {
            let pane = inner.panes.get(id).ok_or_else(|| missing("pane"))?.clone();
            if !inner.state.contains(session, id) {
                return Err(missing("pane"));
            }
            panes.push((id.clone(), pane));
        }
        let request = args.get("mode").and_then(Value::as_str) == Some("request");
        let mut blocks = Vec::new();
        for (id, pane) in panes {
            let mut state = pane.state.lock().unwrap();
            if own && (!request || state.owner.is_none()) {
                state.owner = Some(client.into());
            } else if !own && state.owner.as_deref() == Some(client) {
                state.owner = None;
            }
            blocks.push(json!({"block_id":id,"owner":state.owner}));
        }
        Ok(json!({"blocks":blocks}))
    }
    fn release_ownership(&self, id: &str, session: Option<&str>) {
        let inner = self.inner.lock().unwrap();
        for (block, pane) in &inner.panes {
            if session.is_none()
                || session.is_some_and(|session| inner.state.contains(session, block))
            {
                let mut s = pane.state.lock().unwrap();
                if s.owner.as_deref() == Some(id) {
                    s.owner = None;
                }
            }
        }
    }
    fn resolve_targets_impl(&self, args: &Value) -> Result<Value> {
        let kinds = args
            .get("kinds")
            .and_then(Value::as_array)
            .ok_or_else(|| wire("bad_request", "target kinds or query count", 400))?
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let queries = args
            .get("queries")
            .and_then(Value::as_array)
            .ok_or_else(|| wire("bad_request", "target query", 400))?;
        if kinds.is_empty()
            || queries.len() > 128
            || kinds
                .iter()
                .any(|v| !matches!(v.as_str(), "session" | "window" | "block"))
        {
            return Err(wire("bad_request", "target kinds or query count", 400));
        }
        let inner = self.inner.lock().unwrap();
        let summaries = inner.state.summaries();
        let sessions = summaries
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|v| json!({"kind":"session","session_id":v["session_id"],"label":v["label"]}))
            .collect::<Vec<_>>();
        let matches = |candidates: &[Value], query: &str| -> Vec<Value> {
            for pass in 0..4 {
                let out = candidates
                    .iter()
                    .filter(|candidate| {
                        let kind = candidate["kind"].as_str().unwrap_or_default();
                        let id = candidate
                            .get(format!("{kind}_id").as_str())
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        match pass {
                            0 => id == query,
                            1 => {
                                !query.is_empty()
                                    && candidate["label"]
                                        .as_str()
                                        .is_some_and(|v| v.eq_ignore_ascii_case(query))
                            }
                            2 => {
                                kind == "window"
                                    && candidate["ordinal"].as_u64().map(|v| v.to_string())
                                        == Some(query.into())
                            }
                            3 => query.len() >= 4 && id.ends_with(query),
                            _ => false,
                        }
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                if !out.is_empty() {
                    return out;
                }
            }
            Vec::new()
        };
        let scope_session = args["scope"]["session"].as_str().unwrap_or_default();
        let context_session = args["context"]["session_id"].as_str().unwrap_or_default();
        let selected = if !scope_session.is_empty() {
            let found = matches(&sessions, scope_session);
            if found.len() != 1 {
                return Err(wire(
                    "bad_request",
                    "session scope (not found or ambiguous)",
                    400,
                ));
            }
            found[0]["session_id"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        } else if !context_session.is_empty() {
            context_session.into()
        } else {
            sessions
                .first()
                .and_then(|v| v["session_id"].as_str())
                .unwrap_or_default()
                .into()
        };
        let mut candidates = sessions.clone();
        let mut active_window = String::new();
        let mut focused = String::new();
        if let Some(view) = inner.state.view(&selected) {
            for (index, window) in view["windows"].as_array().into_iter().flatten().enumerate() {
                if window["window_id"] == view["active_window_id"] {
                    active_window = field(window, "window_id").into();
                    focused = field(window, "focused_block_id").into();
                }
                let candidate = json!({"kind":"window","session_id":view["session_id"],"window_id":window["window_id"],"label":window["label"],"ordinal":index+1});
                if kinds.iter().any(|k| k == "window") {
                    candidates.push(candidate);
                }
                if kinds.iter().any(|k| k == "block") {
                    for layer in window["layers"].as_array().into_iter().flatten() {
                        for block in layer["blocks"].as_array().into_iter().flatten() {
                            candidates.push(json!({"kind":"block","session_id":view["session_id"],"window_id":window["window_id"],"block_id":block["block_id"],"label":"shell"}));
                        }
                    }
                }
            }
        }
        candidates.retain(|v| kinds.iter().any(|k| v["kind"] == *k));
        let context_block = args["context"]["block_id"].as_str().unwrap_or_default();
        let mut results = Vec::new();
        for raw in queries {
            let original = raw
                .as_str()
                .ok_or_else(|| wire("bad_request", "target query", 400))?;
            let query = if !original.is_empty() {
                original.into()
            } else if kinds == ["session"] {
                selected.clone()
            } else if kinds == ["window"] {
                active_window.clone()
            } else if !context_block.is_empty() {
                context_block.into()
            } else {
                focused.clone()
            };
            let found = matches(&candidates, &query);
            let mut result = json!({"query":original,"status":"not_found"});
            if found.len() == 1 {
                result["status"] = json!("resolved");
                result["target"] = found[0].clone();
            } else if found.len() > 1 {
                result["status"] = json!("ambiguous");
                result["candidates"] = json!(found);
            }
            results.push(result);
        }
        Ok(json!({"results":results}))
    }
    pub fn close(&self) {
        let mut inner = self.inner.lock().unwrap();
        for (_, pane) in inner.panes.drain() {
            pane.stop();
        }
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.close();
    }
}
impl Pane {
    fn can_control_locked(&self, state: &Stream, id: &str) -> bool {
        let active = state
            .consumers
            .iter()
            .find(|(_, sender)| !sender.is_closed());
        active.is_none() || active.is_some_and(|(consumer, _)| consumer == id)
    }
    fn can_control(&self, id: &str) -> bool {
        let state = self.state.lock().unwrap();
        self.can_control_locked(&state, id)
    }
    fn start(sandbox: &Sandbox, shell: &str, vt: Terminal) -> Result<Arc<Self>> {
        let (master, mut child) = sandbox.spawn(shell)?;
        let mut read = match master.try_clone() {
            Ok(file) => file,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.into());
            }
        };
        let child_pid = i32::try_from(child.id()).expect("spawned PID fits i32");
        let pane = Arc::new(Self {
            input: Mutex::new(master),
            child: Mutex::new(child),
            child_pid: AtomicI32::new(child_pid),
            state: Mutex::new(Stream {
                vt,
                history: VecDeque::new(),
                end: 0,
                consumers: HashMap::new(),
                size: [80, 24, 0, 0],
                exit: None,
                owner: None,
                blobs: BlobStore::default(),
            }),
        });
        let active = pane.clone();
        if let Err(err) = std::thread::Builder::new()
            .name("rexweb-pty".into())
            .spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    let data = match read.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => &buf[..n],
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    };
                    let replies = {
                        let mut s = active.state.lock().unwrap();
                        let replies = s.vt.feed(data).unwrap_or_default();
                        s.end += data.len() as u64;
                        s.history.extend(data);
                        let overflow = s.history.len().saturating_sub(HISTORY);
                        s.history.drain(..overflow);
                        s.consumers
                            .retain(|_, tx| tx.try_send(PaneUpdate::Output(data.to_vec())).is_ok());
                        replies
                    };
                    if !replies.is_empty() {
                        let _ = active.input.lock().unwrap().write_all(&replies);
                    }
                }
                let status = active
                    .child
                    .lock()
                    .unwrap()
                    .wait()
                    .ok()
                    .and_then(|s| s.code())
                    .unwrap_or(1);
                active.child_pid.store(0, Ordering::Release);
                let mut s = active.state.lock().unwrap();
                s.exit = Some(status);
                for (_, tx) in s.consumers.drain() {
                    let _ = tx.try_send(PaneUpdate::Exit(status));
                }
            })
        {
            pane.stop();
            let _ = pane.child.lock().unwrap().wait();
            return Err(err.into());
        }
        Ok(pane)
    }
    fn stop(&self) {
        let pid = self.child_pid.swap(0, Ordering::AcqRel);
        if pid > 0 {
            // SAFETY: spawn established this child as the leader of its process
            // group. The PID is cleared exactly once before signalling, so this
            // cannot race the reader thread reaping the Child and later PID reuse.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
}
impl Backend for Service {
    fn summaries(&self) -> Result<Vec<Value>> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .state
            .summaries()
            .as_array()
            .cloned()
            .unwrap_or_default())
    }
    fn view(&self, id: &str) -> Result<Value> {
        let inner = self.inner.lock().unwrap();
        let mut view = inner.state.view(id).ok_or_else(|| missing("session"))?;
        let mut metadata = serde_json::Map::new();
        for block in inner.state.pane_ids(id).ok_or_else(|| missing("session"))? {
            let Some(pane) = inner.panes.get(&block) else {
                continue;
            };
            let state = pane.state.lock().unwrap();
            let info = state.vt.info();
            let mut summary = json!({"title":info["title"],"pwd":info["pwd"],"revision":info["revision"],"state":""});
            let priorities = [
                ("idle", 1),
                ("done", 2),
                ("working", 3),
                ("blocked", 4),
                ("error", 5),
            ];
            if let Some(selected) = info["records"]
                .as_array()
                .unwrap_or(&Vec::new())
                .iter()
                .filter_map(|record| {
                    let state = record["state"].as_str().unwrap_or("");
                    priorities
                        .iter()
                        .find(|(name, _)| *name == state)
                        .map(|(_, rank)| (*rank, record))
                })
                .max_by_key(|(rank, _)| *rank)
            {
                summary["state"] = selected.1["state"].clone();
                summary["app"] = selected.1["app"].clone();
                summary["message"] = selected.1["message"].clone();
                summary["status_title"] = selected.1["title"].clone();
                if let Some(progress) = selected.1["progress"].as_i64()
                    && progress >= 0
                {
                    summary["progress"] = json!(progress);
                }
            }
            if let Some(code) = state.exit {
                summary["state"] = json!(if code == 0 { "done" } else { "error" });
                summary["message"] = json!(format!("Exited with status {code}"));
            }
            metadata.insert(block, summary);
        }
        view["terminal_metadata"] = Value::Object(metadata);
        Ok(view)
    }
    fn mutate(&self, client: Option<&str>, method: &str, args: &Value) -> Result<Value> {
        if method == "browser.action" || method == "web.action" {
            return self.apply(args);
        }
        let id = field(args, "session_id");
        if method == "session.create" {
            let mut action = json!({"op":"create","label":field(args,"label")});
            if let Some(label) = args
                .get("initial_windows")
                .and_then(Value::as_array)
                .and_then(|v| v.first())
                .and_then(|w| w.get("window_label"))
            {
                action["window_label"] = label.clone();
            }
            return self.apply(&action);
        }
        if matches!(method, "session.describe_block" | "session.list_blocks") {
            return self.describe_blocks(
                id,
                field(args, "block_id"),
                method == "session.describe_block",
            );
        }
        if matches!(method, "session.own_block" | "session.disown_block") {
            return self.ownership(client, id, args, method == "session.own_block");
        }
        let view = self.view(id)?;
        let mut window = field(args, "window_id");
        if window.is_empty() {
            window = field(&view, "active_window_id");
        }
        let mut block = field(
            args,
            if method == "session.new_split" {
                "anchor_id"
            } else {
                "block_id"
            },
        );
        if block.is_empty() {
            block = view["windows"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|w| field(w, "window_id") == window)
                .map(|w| field(w, "focused_block_id"))
                .unwrap_or_default();
        }
        let op = match method {
            "session.destroy" => "destroy",
            "session.set_label" => "rename",
            "session.set_window_label" => "rename-window",
            "session.new_window" => "new-window",
            "session.close_window" => "close-window",
            "session.focus_window" => "focus-window",
            "session.focus_block" => "focus-block",
            "session.new_split" => "split",
            "session.resize_pane" => "resize-split",
            "block.close" => "close-block",
            _ => {
                return Err(wire(
                    "not_supported",
                    "unsupported workspace operation",
                    400,
                ));
            }
        };
        self.apply(&json!({"op":op,"session":id,"window":window,"block":block,"direction":field(args,"direction"),"label":field(args,"label"),"window_label":field(args,"window_label"),"before":field(args,"before"),"after":field(args,"after"),"ratio":args["ratio"].as_f64().unwrap_or(0.0),"side":field(args,"side"),"focus":args["focus"]}))
    }
    fn resolve_targets(&self, args: &Value) -> Result<Value> {
        self.resolve_targets_impl(args)
    }
    fn pane_exists(&self, session: &str, block: &str) -> Result<bool> {
        Ok(self.inner.lock().unwrap().state.contains(session, block))
    }
    fn client_disconnected(&self, id: &str) -> Result<()> {
        for pane in self.inner.lock().unwrap().panes.values() {
            let mut s = pane.state.lock().unwrap();
            s.consumers.remove(id);
            if s.owner.as_deref() == Some(id) {
                s.owner = None;
            }
        }
        Ok(())
    }
    fn client_detached(&self, id: &str, session: &str) -> Result<()> {
        self.release_ownership(id, Some(session));
        Ok(())
    }
    fn pane_attach(
        &self,
        id: &str,
        session: &str,
        block: &str,
        offset: Option<u64>,
    ) -> Result<PaneAttachment> {
        let p = self.pane(session, block)?;
        let mut s = p.state.lock().unwrap();
        s.consumers.retain(|_, tx| !tx.is_closed());
        if !s.consumers.is_empty() {
            return Err(wire(
                "conflict",
                "pane already has an active terminal connection",
                409,
            ));
        }
        let start = s.end - s.history.len() as u64;
        let requested = offset.unwrap_or(start);
        let reset = requested < start || requested > s.end;
        let offset = if reset { start } else { requested };
        let replay = s
            .history
            .iter()
            .skip((offset - start) as usize)
            .copied()
            .collect();
        let (tx, updates) = mpsc::channel(32);
        if s.exit.is_none() {
            s.consumers.insert(id.into(), tx);
        }
        let [columns, rows, cell_width_px, cell_height_px] = s.size;
        Ok(PaneAttachment {
            initial: Vec::new(),
            replay,
            offset,
            reset,
            columns,
            rows,
            cell_width_px,
            cell_height_px,
            updates,
            exited: s.exit,
        })
    }
    fn pane_input(&self, id: &str, session: &str, block: &str, data: &[u8]) -> Result<()> {
        ensure!(data.len() <= 65536, "terminal input too large");
        let p = self.pane(session, block)?;
        if !p.can_control(id) {
            return Err(wire(
                "conflict",
                "pane is controlled by another terminal connection",
                409,
            ));
        }
        p.input.lock().unwrap().write_all(data)?;
        Ok(())
    }
    fn pane_event(&self, id: &str, session: &str, block: &str, event: &Value) -> Result<Value> {
        let payload = &event["payload"];
        let p = self.pane(session, block)?;
        match field(event, "type") {
            "key" => {
                let modes = stream_modes(&p.state.lock().unwrap().history);
                self.pane_input(
                    id,
                    session,
                    block,
                    encode_key(payload, modes.cursor).as_bytes(),
                )?;
            }
            "paste" => {
                let text = payload["text"]
                    .as_str()
                    .ok_or_else(|| wire("bad_request", "invalid paste event", 400))?;
                ensure!(text.len() <= 64 << 10, "invalid paste event");
                let modes = stream_modes(&p.state.lock().unwrap().history);
                self.pane_input(
                    id,
                    session,
                    block,
                    paste::encode(text, modes.paste).as_bytes(),
                )?;
            }
            "focus" => {
                let action = field(payload, "action");
                if !matches!(action, "in" | "out") {
                    return Err(wire("bad_request", "invalid focus event", 400));
                }
                if stream_modes(&p.state.lock().unwrap().history).focus {
                    self.pane_input(
                        id,
                        session,
                        block,
                        if action == "in" { b"\x1b[I" } else { b"\x1b[O" },
                    )?;
                }
            }
            _ => return Err(wire("not_supported", "unsupported terminal event", 400)),
        }
        Ok(json!({}))
    }
    fn pane_method(
        &self,
        id: &str,
        session: &str,
        block: &str,
        method: &str,
        args: &Value,
    ) -> Result<Value> {
        let pane = self.pane(session, block)?;
        match method {
            "write" => {
                let data = args
                    .get("data")
                    .and_then(Value::as_str)
                    .ok_or_else(|| wire("bad_request", "terminal method arguments", 400))?;
                self.pane_input(id, session, block, data.as_bytes())?;
                Ok(json!({}))
            }
            "resize" => {
                let dimension = |key: &str| {
                    args[key]
                        .as_u64()
                        .and_then(|v| u16::try_from(v).ok())
                        .unwrap_or(0)
                };
                let size = [
                    dimension("width"),
                    dimension("height"),
                    dimension("cell_width"),
                    dimension("cell_height"),
                ];
                ensure!(
                    size[0] >= 2
                        && size[0] <= 500
                        && size[1] >= 1
                        && size[1] <= 500
                        && size[2] <= 1024
                        && size[3] <= 1024,
                    "invalid terminal size"
                );
                let mut state = pane.state.lock().unwrap();
                if !pane.can_control_locked(&state, id) {
                    return Err(wire(
                        "conflict",
                        "pane is controlled by another terminal connection",
                        409,
                    ));
                }
                linux::resize(
                    &pane.input.lock().unwrap(),
                    size[0],
                    size[1],
                    size[2],
                    size[3],
                )?;
                state.vt.resize(
                    size[0].into(),
                    size[1].into(),
                    size[2].into(),
                    size[3].into(),
                )?;
                state.size = size;
                state.consumers.retain(|_, tx| {
                    tx.try_send(PaneUpdate::Resize {
                        columns: size[0],
                        rows: size[1],
                        cell_width_px: size[2],
                        cell_height_px: size[3],
                    })
                    .is_ok()
                });
                Ok(json!({}))
            }
            "format" => {
                let f = field(args, "format");
                let format = if f.is_empty() { "text" } else { f };
                if !matches!(format, "text" | "vt" | "html") {
                    return Err(wire("bad_request", "invalid format", 400));
                }
                let max_bytes = optional_nonnegative_usize(args, "max_bytes", "max_bytes")?;
                let mut state = pane.state.lock().unwrap();
                let data = match state.vt.capture(
                    format,
                    args["trim"].as_bool().unwrap_or(false),
                    args["unwrap"].as_bool().unwrap_or(false),
                ) {
                    Ok(data) => data,
                    Err(error) if error.to_string().contains("capture exceeds 4 MiB") => {
                        return Err(wire("too_large", "capture exceeds 4 MiB", 413));
                    }
                    Err(error) => return Err(error),
                };
                drop(state);
                native_capture(&pane, data, max_bytes)
            }
            "blob_read" => {
                let id = match args.get("id") {
                    None => 0,
                    Some(value) => value
                        .as_u64()
                        .and_then(|value| u32::try_from(value).ok())
                        .ok_or_else(|| {
                            wire("bad_request", "invalid terminal method arguments", 400)
                        })?,
                };
                let offset =
                    optional_nonnegative_usize(args, "offset", "blob offset")?.unwrap_or(0);
                let max_bytes = optional_nonnegative_usize(args, "max_bytes", "max_bytes")?;
                pane.state.lock().unwrap().blobs.read(id, offset, max_bytes)
            }
            "input_mode" => input_mode(&pane.input.lock().unwrap()),
            "size" => {
                let s = pane.state.lock().unwrap();
                Ok(
                    json!({"columns":s.size[0],"rows":s.size[1],"cell_width_px":s.size[2],"cell_height_px":s.size[3],"desired":[]}),
                )
            }
            "title" | "pwd" | "program_status" => {
                let m = self.metadata(session, block)?;
                Ok(json!({method:m[method]}))
            }
            "set_theme" => Ok(json!({})),
            "process" => {
                let pid = pane.child_pid.load(Ordering::Acquire);
                Ok(
                    json!({"child":{"pid":pid,"name":"sh","path":self.shell,"cwd":"."},"foreground":{"pid":pid},"ancestors":[],"home":".home"}),
                )
            }
            _ => Err(wire("method_not_found", "unknown terminal method", 404)),
        }
    }
    fn uploads_enabled(&self) -> bool {
        self.uploads.lock().unwrap().is_some()
    }
    fn upload(
        &self,
        session: &str,
        block: &str,
        filename: &str,
        data: &mut dyn Read,
    ) -> Result<Value> {
        self.pane(session, block)?;
        let mut bytes = Vec::new();
        data.read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 16 << 20, "upload exceeds 16 MiB");
        let uploads = self.uploads.lock().unwrap();
        let root = uploads
            .as_ref()
            .ok_or_else(|| wire("not_supported", "uploads are not enabled", 503))?;
        root.store(filename, &mut Cursor::new(bytes))
    }
    fn pane_capture(
        &self,
        session: &str,
        block: &str,
        format: &str,
        trim: bool,
        unwrap: bool,
    ) -> Result<Vec<u8>> {
        self.pane(session, block)?
            .state
            .lock()
            .unwrap()
            .vt
            .capture(format, trim, unwrap)
    }
}

fn optional_nonnegative_usize(args: &Value, key: &str, field: &str) -> Result<Option<usize>> {
    let Some(value) = args.get(key) else {
        return Ok(None);
    };
    let Some(number) = value.as_u64() else {
        return Err(wire("bad_request", &format!("invalid {field}"), 400));
    };
    usize::try_from(number)
        .map(Some)
        .map_err(|_| wire("bad_request", &format!("invalid {field}"), 400))
}

fn native_capture(pane: &Pane, data: Vec<u8>, max_bytes: Option<usize>) -> Result<Value> {
    let content = std::str::from_utf8(&data)?;
    if max_bytes.is_none() || max_bytes == Some(0) {
        if json_prefix(&data, NATIVE_JSON_BUDGET) != data.len() {
            return Err(wire(
                "too_large",
                "set max_bytes to read this capture in chunks",
                413,
            ));
        }
        return Ok(json!({"content":content}));
    }
    let limit = max_bytes.unwrap_or(0).min(NATIVE_JSON_BUDGET);
    let end = json_prefix(&data, limit);
    if end == 0 && !data.is_empty() {
        return Err(wire("bad_request", "invalid max_bytes (too small)", 400));
    }
    let mut reply = json!({
        "content":std::str::from_utf8(&data[..end])?,
        "total":data.len(),
        "eof":end==data.len(),
    });
    if end < data.len() {
        let id = pane.state.lock().unwrap().blobs.insert(data);
        reply["id"] = json!(id);
    }
    Ok(reply)
}

fn is_utf8_start(byte: u8) -> bool {
    byte & 0xc0 != 0x80
}

/// Length of a UTF-8 prefix whose JSON string representation fits in `budget`,
/// including the two surrounding quotes and Go's encoding/json escapes.
fn json_prefix(data: &[u8], budget: usize) -> usize {
    if budget < 8 {
        return 0;
    }
    let (mut low, mut high, mut best) = (0, data.len().min(budget), 0);
    while low <= high {
        let middle = low + (high - low) / 2;
        let mut end = middle;
        while end > 0 && data.get(end).is_some_and(|byte| byte & 0xc0 == 0x80) {
            end -= 1;
        }
        if go_json_fits(&data[..end], budget - 2) {
            best = end;
            low = middle + 1;
        } else {
            high = middle - 1;
        }
    }
    best
}

fn go_json_fits(data: &[u8], mut remaining: usize) -> bool {
    for chunk in data.utf8_chunks() {
        for character in chunk.valid().chars() {
            let cost = match character {
                '\u{08}' | '\t' | '\n' | '\u{0c}' | '\r' | '"' | '\\' => 2,
                '\0'..='\u{1f}' | '<' | '>' | '&' | '\u{2028}' | '\u{2029}' => 6,
                _ => character.len_utf8(),
            };
            let Some(rest) = remaining.checked_sub(cost) else {
                return false;
            };
            remaining = rest;
        }
        let Some(rest) = remaining.checked_sub(6 * chunk.invalid().len()) else {
            return false;
        };
        remaining = rest;
    }
    true
}

#[derive(Default)]
struct StreamModes {
    cursor: bool,
    paste: bool,
    focus: bool,
}
fn stream_modes(history: &VecDeque<u8>) -> StreamModes {
    let bytes = history.iter().copied().collect::<Vec<_>>();
    let enabled = |mode: &str| {
        let on = format!("\x1b[?{mode}h").into_bytes();
        let off = format!("\x1b[?{mode}l").into_bytes();
        let last_on = bytes
            .windows(on.len())
            .rposition(|v| v == on)
            .map(|v| v as i64)
            .unwrap_or(-1);
        let last_off = bytes
            .windows(off.len())
            .rposition(|v| v == off)
            .map(|v| v as i64)
            .unwrap_or(-1);
        last_on > last_off
    };
    StreamModes {
        cursor: enabled("1"),
        paste: enabled("2004"),
        focus: enabled("1004"),
    }
}
fn modifier(value: &str, name: &str) -> bool {
    value.split(['+', ',', ' ', '|']).any(|part| part == name)
}
fn encode_key(value: &Value, cursor: bool) -> String {
    let text = value["text"].as_str().unwrap_or_default();
    let action = value["action"].as_str().unwrap_or_default();
    let mods = value["mods"].as_str().unwrap_or_default();
    let consumed = value["consumed_mods"].as_str().unwrap_or_default();
    if action == "release"
        || value["composing"].as_bool().unwrap_or(false)
        || modifier(mods, "super")
    {
        return String::new();
    }
    let mut encoded: String = match value["key"].as_str().unwrap_or_default() {
        "enter" | "numpad_enter" => "\r".into(),
        "tab" if modifier(mods, "shift") => "\x1b[Z".into(),
        "tab" => "\t".into(),
        "backspace" => "\x7f".into(),
        "escape" => "\x1b".into(),
        "arrow_up" | "up" => if cursor { "\x1bOA" } else { "\x1b[A" }.into(),
        "arrow_down" | "down" => if cursor { "\x1bOB" } else { "\x1b[B" }.into(),
        "arrow_right" | "right" => if cursor { "\x1bOC" } else { "\x1b[C" }.into(),
        "arrow_left" | "left" => if cursor { "\x1bOD" } else { "\x1b[D" }.into(),
        "home" => "\x1b[H".into(),
        "end" => "\x1b[F".into(),
        "insert" => "\x1b[2~".into(),
        "delete" => "\x1b[3~".into(),
        "page_up" => "\x1b[5~".into(),
        "page_down" => "\x1b[6~".into(),
        _ => text.into(),
    };
    if modifier(mods, "ctrl") && !modifier(consumed, "ctrl") {
        let mut code = value["unshifted_codepoint"].as_u64().unwrap_or_default();
        let key = value["key"].as_str().unwrap_or_default();
        if code == 0 && key.len() == 5 && key.starts_with("key_") {
            code = key.as_bytes()[4] as u64;
        }
        if (b'a' as u64..=b'z' as u64).contains(&code) {
            code -= b'a' as u64 - b'A' as u64;
        }
        if (b'@' as u64..=b'_' as u64).contains(&code) {
            encoded = char::from_u32((code & 31) as u32).unwrap_or('\0').into();
        } else if code == b' ' as u64 {
            encoded = "\0".into();
        }
    }
    if !encoded.is_empty() && modifier(mods, "alt") && !modifier(consumed, "alt") {
        encoded.insert(0, '\x1b');
    }
    encoded
}

#[cfg(test)]
mod capture_tests {
    use super::{BlobStore, NATIVE_BLOB_BUDGET, NATIVE_BLOB_TTL, json_prefix};
    use std::time::{Duration, Instant};

    #[test]
    fn native_json_prefix_matches_go_escape_costs() {
        assert_eq!(json_prefix(b"a<&", 8), 1);
        assert_eq!(json_prefix(b"\n\t\x08\x0c\r", 10), 4);
        assert_eq!(json_prefix("a\u{2028}b".as_bytes(), 9), 4);
        assert_eq!(json_prefix("é🙂中".as_bytes(), 8), 6);
        assert_eq!(json_prefix(b"abcd", 7), 0);
    }

    #[test]
    fn native_json_prefix_matches_recorded_cases() {
        #[derive(serde::Deserialize)]
        struct Case {
            #[serde(rename = "fn")]
            operation: String,
            args: (String, i64),
            ok: usize,
        }

        let data = include_str!("../tests/fixtures/capture-chunks.jsonl");
        assert_eq!(data.lines().count(), 225);
        for (index, line) in data.lines().enumerate() {
            let case: Case = serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("capture fixture line {}: {error}", index + 1));
            assert_eq!(case.operation, "JSONPrefix");
            let (text, budget) = case.args;
            assert_eq!(
                json_prefix(text.as_bytes(), usize::try_from(budget).unwrap_or(0)),
                case.ok,
                "capture fixture line {} with text {text:?} and budget {budget}",
                index + 1,
            );
        }
    }

    #[test]
    fn native_json_prefix_accounts_for_malformed_utf8() {
        assert_eq!(json_prefix(b"\xf0\x9f", 8), 0);
        assert_eq!(json_prefix(b"\xf0\x9f", 14), 2);
        assert_eq!(json_prefix(b"x\xff", 9), 2);
    }

    #[test]
    fn blob_read_paginates_on_utf8_boundaries() {
        let mut blobs = BlobStore::default();
        let id = blobs.insert("aé🙂z".as_bytes().to_vec());
        let first = blobs.read(id, 0, Some(8)).unwrap();
        assert_eq!(first["content"], "aé");
        assert_eq!(first["eof"], false);
        let offset = first["content"].as_str().unwrap().len();
        let last = blobs.read(id, offset, None).unwrap();
        assert_eq!(last["content"], "🙂z");
        assert_eq!(last["eof"], true);
        assert!(blobs.read(id, 2, None).is_err());
    }

    #[test]
    fn blobs_expire_and_remain_within_pane_budget() {
        let now = Instant::now();
        let mut blobs = BlobStore::default();
        let first = blobs.insert_at(vec![1; 3 << 20], now);
        let second = blobs.insert_at(vec![2; 3 << 20], now);
        assert_eq!(blobs.entries.len(), 1);
        assert!(!blobs.entries.contains_key(&first));
        assert!(blobs.entries.contains_key(&second));
        assert!(
            blobs
                .entries
                .values()
                .map(|blob| blob.data.len())
                .sum::<usize>()
                <= NATIVE_BLOB_BUDGET
        );
        assert!(
            blobs
                .read_at(
                    second,
                    0,
                    None,
                    now + NATIVE_BLOB_TTL + Duration::from_secs(1)
                )
                .is_err()
        );
        assert!(blobs.entries.is_empty());
    }
}
