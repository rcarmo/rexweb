//! Rex HTTP/WebSocket wire protocol.
//!
//! The PTY/session implementation is deliberately supplied by [`Backend`].  This
//! module owns connection lifetime, admission, ticket use and wire framing; it
//! does not bypass the service's sandbox or outer authentication policy.

use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fmt,
    io::Read,
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};

use anyhow::Result;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{
        DefaultBodyLimit, Multipart, Query, State,
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};

pub const CONTROL_PROTOCOL: &str = "rex.control.v1";
pub const DATA_PROTOCOL: &str = "rex.data.terminal.v1";
pub const REVISION: u32 = 12;
pub const MAX_MESSAGE_BYTES: usize = 1 << 20;
const MAX_CONNECTIONS: usize = 96;
const MAX_CONTROLS: usize = 32;
const MAX_TICKETS: usize = 256;
const TICKET_TTL: Duration = Duration::from_secs(60);
const HEARTBEAT: Duration = Duration::from_secs(15);
const IDLE: Duration = Duration::from_secs(45);
// Blank 80×24 Ghostty snapshot after resetting an idle /bin/cat block in
// Rex 0.1.0-preview.14. It contains no program output or filesystem paths.
const BLANK_SNAPSHOT: &[u8] = include_bytes!("../assets/fixtures/rex_blank.snapshot");

/// A synchronous backend boundary.  Implementations remain responsible for
/// serialization, sandboxing, PTY I/O and authoritative VT/capture state.
/// `args` and return values are the JSON shapes specified by `SPEC.md`.
pub trait Backend: Send + Sync + 'static {
    fn summaries(&self) -> Result<Vec<Value>>;
    fn view(&self, session_id: &str) -> Result<Value>;
    /// `method` is a Rex `session.*`/layout/ownership operation.  It must reject
    /// unsupported layouts rather than coercing them into an executable shell.
    fn mutate(&self, client_id: Option<&str>, method: &str, args: &Value) -> Result<Value>;
    fn resolve_targets(&self, args: &Value) -> Result<Value>;
    fn pane_exists(&self, session_id: &str, block_id: &str) -> Result<bool>;
    fn client_disconnected(&self, client_id: &str) -> Result<()>;
    /// Release explicit pane ownership for one session without dropping the
    /// client's attachments or ownership of other sessions.
    fn client_detached(&self, client_id: &str, session_id: &str) -> Result<()> {
        let _ = (client_id, session_id);
        Ok(())
    }

    /// Attach a fresh Rex data stream, or a browser stream when `offset` is
    /// supplied.  The receiver is bounded by the backend.  `initial` is sent as
    /// binary frames in order, sent after the mandatory Rex GHOSTSNP frame.
    fn pane_attach(
        &self,
        client_id: &str,
        session_id: &str,
        block_id: &str,
        offset: Option<u64>,
    ) -> Result<PaneAttachment>;
    fn pane_input(
        &self,
        client_id: &str,
        session_id: &str,
        block_id: &str,
        data: &[u8],
    ) -> Result<()>;
    /// `event` is the unmodified Rex event object (`key`, `paste`, or `focus`).
    fn pane_event(
        &self,
        client_id: &str,
        session_id: &str,
        block_id: &str,
        event: &Value,
    ) -> Result<Value>;
    fn pane_method(
        &self,
        client_id: &str,
        session_id: &str,
        block_id: &str,
        method: &str,
        args: &Value,
    ) -> Result<Value>;
    fn uploads_enabled(&self) -> bool {
        false
    }
    fn upload(
        &self,
        session_id: &str,
        block_id: &str,
        filename: &str,
        data: &mut dyn Read,
    ) -> Result<Value> {
        let _ = (session_id, block_id, filename, data);
        Err(unsupported("uploads are not enabled"))
    }
    fn pane_capture(
        &self,
        session_id: &str,
        block_id: &str,
        format: &str,
        trim: bool,
        unwrap: bool,
    ) -> Result<Vec<u8>>;
    fn host_id(&self) -> String {
        "local".into()
    }
}

pub struct PaneAttachment {
    pub initial: Vec<Vec<u8>>,
    pub replay: Vec<u8>,
    pub offset: u64,
    pub reset: bool,
    pub columns: u16,
    pub rows: u16,
    pub cell_width_px: u16,
    pub cell_height_px: u16,
    pub updates: mpsc::Receiver<PaneUpdate>,
    pub exited: Option<i32>,
}

#[derive(Debug)]
pub enum PaneUpdate {
    Output(Vec<u8>),
    Resize {
        columns: u16,
        rows: u16,
        cell_width_px: u16,
        cell_height_px: u16,
    },
    Exit(i32),
}

#[derive(Debug)]
/// Typed backend error mapped verbatim to the Rex/HTTP error envelope.
/// Backends may downcast-match with ordinary `anyhow` errors, but protocol
/// errors should return this value to preserve 4xx semantics.
pub struct WireError {
    pub code: &'static str,
    pub message: String,
    pub status: u16,
}
impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl Error for WireError {}

fn invalid(message: impl Into<String>) -> anyhow::Error {
    WireError {
        code: "bad_request",
        message: message.into(),
        status: 400,
    }
    .into()
}
fn unsupported(message: impl Into<String>) -> anyhow::Error {
    WireError {
        code: "not_supported",
        message: message.into(),
        status: 400,
    }
    .into()
}
fn conflict(message: impl Into<String>) -> anyhow::Error {
    WireError {
        code: "conflict",
        message: message.into(),
        status: 409,
    }
    .into()
}
fn missing(resource: &str) -> anyhow::Error {
    WireError {
        code: if resource == "session" {
            "session_not_found"
        } else {
            "not_found"
        },
        message: format!("{resource} not found"),
        status: 404,
    }
    .into()
}

fn wire_error(error: &anyhow::Error) -> Value {
    if let Some(e) = error.downcast_ref::<WireError>() {
        json!({"code":e.code,"message":e.message,"status":e.status})
    } else {
        json!({"code":"internal","message":"workspace operation failed","status":500})
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct RexRequest {
    #[serde(rename = "type")]
    kind: String,
    id: String,
    method: String,
    #[serde(default)]
    payload: Value,
}
#[derive(Clone, Deserialize)]
struct Hello {
    #[serde(rename = "type")]
    kind: String,
    api_revision: u32,
    #[serde(default)]
    api_min_revision: u32,
}

#[derive(Clone)]
struct Event {
    session: Option<String>,
    body: Value,
}
struct Client {
    id: String,
    all: Mutex<bool>,
    attached: Mutex<HashSet<String>>,
    live: std::sync::atomic::AtomicBool,
}
impl Client {
    fn is_live(&self) -> bool {
        self.live.load(std::sync::atomic::Ordering::Acquire)
    }
    fn can_use(&self, session: &str) -> bool {
        !session.is_empty()
            && (*self.all.lock().unwrap() || self.attached.lock().unwrap().contains(session))
    }
}

struct Ticket {
    session: String,
    block: String,
    client: Weak<Client>,
    expires: Instant,
}
struct Limits {
    controls: usize,
    data: usize,
    total: usize,
}
impl Limits {
    fn acquire(&mut self, control: bool) -> bool {
        let limit = if control {
            MAX_CONTROLS
        } else {
            MAX_CONNECTIONS
        };
        let current = if control {
            &mut self.controls
        } else {
            &mut self.data
        };
        if *current >= limit || self.total >= MAX_CONNECTIONS {
            return false;
        }
        *current += 1;
        self.total += 1;
        true
    }
    fn release(&mut self, control: bool) {
        if control {
            self.controls -= 1;
        } else {
            self.data -= 1;
        }
        self.total -= 1;
    }
}

struct Server<B> {
    backend: Arc<B>,
    instance: String,
    started: String,
    tickets: Mutex<HashMap<[u8; 16], Ticket>>,
    events: broadcast::Sender<Event>,
    limits: Mutex<Limits>,
}

/// Build the browser and native listeners from one shared backend state.
/// Authentication, browser Host/Origin policy and TLS termination remain the
/// owning service's responsibility.
pub fn routers<B: Backend>(backend: Arc<B>) -> (Router, Router) {
    let (events, _) = broadcast::channel(128);
    let state = Arc::new(Server {
        backend,
        instance: random_id(),
        started: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        tickets: Mutex::new(HashMap::new()),
        events,
        limits: Mutex::new(Limits {
            controls: 0,
            data: 0,
            total: 0,
        }),
    });
    let web = Router::new()
        .route("/api/help", get(help))
        .route("/llms.txt", get(llms))
        .route("/api/sessions", get(sessions::<B>))
        .route("/api/session", get(session_view::<B>))
        .route("/api/capabilities", get(capabilities::<B>))
        .route("/api/action", post(action::<B>))
        .route(
            "/api/upload",
            post(upload::<B>).layer(DefaultBodyLimit::max(17 << 20)),
        )
        .route("/api/capture", get(capture::<B>))
        .route("/api/terminal", get(browser_terminal::<B>))
        .with_state(state.clone());
    let native = Router::new()
        .route("/api/control", get(control::<B>))
        .route("/api/data", get(data::<B>))
        .with_state(state);
    (web, native)
}

/// Compatibility router for callers that deliberately serve both surfaces.
/// Production listeners should use [`routers`] to retain route isolation.
pub fn router<B: Backend>(backend: Arc<B>) -> Router {
    let (web, native) = routers(backend);
    web.merge(native)
}

fn random_id() -> String {
    let mut bytes = [0; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

async fn help() -> impl IntoResponse {
    Json(json!({
        "api_revision":REVISION, "api_min_revision":REVISION,
        "control":CONTROL_PROTOCOL,"data":DATA_PROTOCOL,
        "routes":["/api/control","/api/data","/api/sessions","/api/session","/api/action","/api/upload","/api/capture","/api/terminal"],
        "limits":{"max_message_bytes":MAX_MESSAGE_BYTES,"ticket_ttl_ms":60_000,"max_tickets":MAX_TICKETS}
    }))
}
async fn llms() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        format!(
            "Rexweb implements Rex API revision {REVISION}.\nControl: {CONTROL_PROTOCOL} at /api/control.\nData: {DATA_PROTOCOL} at /api/data.\nTickets are single-use and expire after 60 seconds.\nSee /api/help.\n"
        ),
    )
}

async fn reject_origin(headers: &HeaderMap) -> Option<Response> {
    if headers.get(header::ORIGIN).is_some() {
        Some(
            (
                StatusCode::FORBIDDEN,
                "cross-origin Rex access is forbidden",
            )
                .into_response(),
        )
    } else {
        None
    }
}
fn request_protocol(headers: &HeaderMap, expected: &str) -> bool {
    headers
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|p| p.trim() == expected))
}

async fn control<B: Backend>(
    State(server): State<Arc<Server<B>>>,
    ws: WebSocketUpgrade,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = reject_origin(&headers).await {
        return response;
    }
    if !request_protocol(&headers, CONTROL_PROTOCOL) {
        return (StatusCode::BAD_REQUEST, "required Rex subprotocol missing").into_response();
    }
    if !server.limits.lock().unwrap().acquire(true) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "too many control connections",
        )
            .into_response();
    }
    ws.protocols([CONTROL_PROTOCOL])
        .max_message_size(MAX_MESSAGE_BYTES)
        .max_frame_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| async move {
            control_socket(server.clone(), socket).await;
            server.limits.lock().unwrap().release(true);
        })
}

async fn control_socket<B: Backend>(server: Arc<Server<B>>, mut socket: WebSocket) {
    let Some(hello) = recv_timeout(&mut socket, 5).await else {
        return;
    };
    let Ok(Message::Text(text)) = hello else {
        return;
    };
    let Ok(hello): std::result::Result<Hello, _> = serde_json::from_str(&text) else {
        return;
    };
    if hello.kind != "hello" || hello.api_revision < REVISION || hello.api_min_revision > REVISION {
        let _ = socket
            .send(Message::Close(Some(CloseFrame {
                code: 1008,
                reason: "API revision 12 required".into(),
            })))
            .await;
        return;
    }
    let client = Arc::new(Client {
        id: format!("client:{}", uuid::Uuid::new_v4()),
        all: Mutex::new(false),
        attached: Mutex::new(HashSet::new()),
        live: std::sync::atomic::AtomicBool::new(true),
    });
    let welcome = json!({"type":"welcome","protocol":CONTROL_PROTOCOL,"limits":{"max_message_bytes":MAX_MESSAGE_BYTES},"api_revision":REVISION,"api_server_revision":REVISION,"api_min_revision":REVISION,"client":{"id":client.id,"reconnect_token":random_id(),"resumed":false}});
    if socket
        .send(Message::Text(welcome.to_string().into()))
        .await
        .is_err()
    {
        return;
    }
    let mut events = server.events.subscribe();
    let mut heartbeat = tokio::time::interval(HEARTBEAT);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut timeout = Box::pin(tokio::time::sleep(IDLE));
    let mut seq = 0_u64;
    loop {
        tokio::select! {
            _ = &mut timeout => { break; }
            _ = heartbeat.tick() => {
                if socket.send(Message::Ping(Vec::new().into())).await.is_err() { break; }
            }
            event = events.recv() => match event {
                Ok(event) if *client.all.lock().unwrap() || event.session.as_ref().is_some_and(|id| client.attached.lock().unwrap().contains(id)) => {
                    seq += 1;
                    if socket.send(Message::Text(json!({"type":"event","seq":seq,"payload":event.body}).to_string().into())).await.is_err() { break; }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
                _ => {}
            },
            message = socket.recv() => match message {
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(Message::Pong(_))) => { timeout.as_mut().reset(tokio::time::Instant::now()+IDLE); }
                Some(Ok(Message::Text(text))) => {
                    timeout.as_mut().reset(tokio::time::Instant::now()+IDLE);
                    let Ok(request): std::result::Result<RexRequest,_> = serde_json::from_str(&text) else { break; };
                    if request.kind != "request" || request.id.is_empty() { break; }
                    let reply = match dispatch(&server, &client, &request.method, request.payload).await {
                        Ok(payload) => json!({"type":"response","id":request.id,"seq":seq,"payload":payload}),
                        Err(error) => json!({"type":"response","id":request.id,"seq":seq,"error":wire_error(&error)}),
                    };
                    if socket.send(Message::Text(reply.to_string().into())).await.is_err() { break; }
                }
                Some(Ok(Message::Binary(_))) => break,
                Some(Ok(_)) => {},
                Some(Err(_)) => break,
            }
        }
    }
    client
        .live
        .store(false, std::sync::atomic::Ordering::Release);
    let _ = server.backend.client_disconnected(&client.id);
}

async fn recv_timeout(
    socket: &mut WebSocket,
    seconds: u64,
) -> Option<Result<Message, axum::Error>> {
    tokio::time::timeout(Duration::from_secs(seconds), socket.recv())
        .await
        .ok()
        .flatten()
}

async fn dispatch<B: Backend>(
    server: &Arc<Server<B>>,
    client: &Arc<Client>,
    method: &str,
    args: Value,
) -> Result<Value> {
    let s = || required_string(&args, "session_id");
    match method {
        "target.resolve" => return server.backend.resolve_targets(&args),
        "server.version" => {
            return Ok(
                json!({"rex_version":"0.1.0-rexweb.rust","platform":std::env::consts::OS,"api_revision":REVISION,"api_min_revision":REVISION,"dirty":false}),
            );
        }
        "server.status" => {
            return Ok(
                json!({"pid":std::process::id(),"instance_id":server.instance,"host_id":server.backend.host_id(),"started_at":server.started,"exit_empty":false,"session_count":server.backend.summaries()?.len(),"api_revision":REVISION,"api_min_revision":REVISION}),
            );
        }
        "server.whoami" => return Ok(json!({"principal":{"kind":"anonymous","transport":"tcp"}})),
        "hosts.list" => return Ok(json!({"hosts":[]})),
        "client.keymap" => return keymap(&args, server),
        "client.set_label" => return Ok(json!({})),
        "block_creator.list" => return Ok(json!({"creators":[creator(server.backend.host_id())]})),
        "session.list" => return Ok(json!({"sessions":server.backend.summaries()?})),
        "session.attach" => {
            let id = s()?;
            let view = server.backend.view(&id)?;
            client.attached.lock().unwrap().insert(id);
            return Ok(json!({"client_id":client.id,"view":view}));
        }
        "session.detach" => {
            let id = s()?;
            client.attached.lock().unwrap().remove(&id);
            server.backend.client_detached(&client.id, &id)?;
            return Ok(json!({}));
        }
        "session.attach_all" | "session.bootstrap" => {
            let scope = args
                .get("event_scope")
                .and_then(Value::as_str)
                .unwrap_or("");
            if !matches!(scope, "session" | "all") {
                return Err(invalid("event scope"));
            }
            *client.all.lock().unwrap() = true;
            let sessions = server.backend.summaries()?;
            if method == "session.attach_all" {
                return Ok(json!({"sessions":sessions}));
            }
            let status = json!({"pid":std::process::id(),"instance_id":server.instance,"host_id":server.backend.host_id(),"started_at":server.started,"exit_empty":false,"session_count":sessions.len(),"api_revision":REVISION,"api_min_revision":REVISION});
            let mut attached = Vec::new();
            let ids = args
                .get("attach_session_ids")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for id in ids {
                if let Some(id) = id.as_str()
                    && let Ok(view) = server.backend.view(id)
                {
                    client.attached.lock().unwrap().insert(id.into());
                    let summary = sessions
                        .iter()
                        .find(|x| x.get("session_id").and_then(Value::as_str) == Some(id))
                        .cloned()
                        .unwrap_or(json!({"session_id":id}));
                    attached.push(json!({"session":summary,"view":view}));
                }
            }
            return Ok(
                json!({"client_id":client.id,"server_status":status,"attach_all":{"sessions":sessions},"selected_session":null,"attached_sessions":attached,"data_tickets":[]}),
            );
        }
        "session.view" => {
            let id = s()?;
            if !client.can_use(&id) {
                return Err(conflict("attach to the session first"));
            }
            return server.backend.view(&id);
        }
        "session.create" => {
            validate_layouts(&args)?;
            return mutate_and_notify(server, Some(&client.id), method, args);
        }
        _ => {}
    }
    let session = args.get("session_id").and_then(Value::as_str).unwrap_or("");
    if !session.is_empty() && !client.can_use(session) {
        return Err(conflict("attach to the session first"));
    }
    match method {
        "session.destroy"
        | "session.set_label"
        | "session.set_window_label"
        | "session.new_window"
        | "session.close_window"
        | "session.focus_window"
        | "session.focus_block"
        | "session.new_split"
        | "session.resize_pane"
        | "block.close"
        | "session.own_block"
        | "session.disown_block" => {
            if matches!(method, "session.new_window" | "session.new_split") {
                validate_layouts(&args)?;
            }
            mutate_and_notify(server, Some(&client.id), method, args)
        }
        "session.describe_block" | "session.list_blocks" => {
            let mut response = server.backend.mutate(Some(&client.id), method, &args)?;
            if method == "session.describe_block" {
                response
                    .as_object_mut()
                    .ok_or_else(|| invalid("block description"))?
                    .insert("api".into(), terminal_api());
            }
            Ok(response)
        }
        "data.connect" => {
            let id = s()?;
            if !client.attached.lock().unwrap().contains(&id) {
                return Err(conflict("attach to the session first"));
            }
            let block = required_string(&args, "block_id")?;
            let reference = args
                .get("data_ref_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            let protocol = args
                .get("subprotocol")
                .and_then(Value::as_str)
                .unwrap_or("");
            if reference != "terminal" || (!protocol.is_empty() && protocol != DATA_PROTOCOL) {
                return Err(invalid("terminal data source or subprotocol"));
            }
            issue_ticket(server, client, id, block)
        }
        "client.key" => pane_call(server, client, &args, "key", None),
        "block.event" => pane_call(server, client, &args, "event", None),
        "block.method" => {
            let terminal = args
                .get("method")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("terminal method"))?;
            pane_call(server, client, &args, terminal, args.get("args"))
        }
        _ if method.starts_with("com.superlogical.terminal.") => pane_call(
            server,
            client,
            &args,
            method.strip_prefix("com.superlogical.terminal.").unwrap(),
            args.get("args"),
        ),
        _ => Err(WireError {
            code: "method_not_found",
            message: format!("unknown Rex method: {method}"),
            status: 404,
        }
        .into()),
    }
}

fn pane_call<B: Backend>(
    server: &Arc<Server<B>>,
    client: &Arc<Client>,
    args: &Value,
    operation: &str,
    method_args: Option<&Value>,
) -> Result<Value> {
    let session = required_string(args, "session_id")?;
    let block = required_string(args, "block_id")?;
    if !client.can_use(&session) {
        return Err(conflict("attach to the session first"));
    }
    match operation {
        "key" => {
            let key = args.get("key").ok_or_else(|| invalid("key"))?;
            server.backend.pane_event(
                &client.id,
                &session,
                &block,
                &json!({"type":"key","payload":key}),
            )?;
            Ok(json!({"handled":false}))
        }
        "event" => server.backend.pane_event(
            &client.id,
            &session,
            &block,
            args.get("event").ok_or_else(|| invalid("event"))?,
        ),
        other => {
            let args = method_args.unwrap_or(&Value::Null);
            if other == "resize" {
                let mut normalized = args.as_object().cloned().unwrap_or_default();
                for (native, backend) in [
                    ("columns", "width"),
                    ("rows", "height"),
                    ("cell_width_px", "cell_width"),
                    ("cell_height_px", "cell_height"),
                ] {
                    if let Some(value) = normalized.get(native).cloned() {
                        normalized.entry(backend.to_owned()).or_insert(value);
                    }
                }
                server.backend.pane_method(
                    &client.id,
                    &session,
                    &block,
                    other,
                    &Value::Object(normalized),
                )
            } else {
                server
                    .backend
                    .pane_method(&client.id, &session, &block, other, args)
            }
        }
    }
}

fn mutate_and_notify<B: Backend>(
    server: &Arc<Server<B>>,
    client_id: Option<&str>,
    method: &str,
    args: Value,
) -> Result<Value> {
    let result = server.backend.mutate(client_id, method, &args)?;
    let session = args
        .get("session_id")
        .or_else(|| args.get("session"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let body = match method {
        "session.create" => {
            json!({"type":"session_created","payload":{"session":result.get("session").cloned().unwrap_or_else(||json!({"session_id":result["session_id"].clone(),"label":result["label"].clone()}))}})
        }
        "session.destroy" => json!({"type":"session_destroyed","payload":{"session_id":session}}),
        "session.set_label" => {
            json!({"type":"session_event","payload":{"session_id":session,"event":{"type":"session_label_changed","payload":{"label":args.get("label")}}}})
        }
        _ => {
            json!({"type":"session_event","payload":{"session_id":session,"event":{"type":"session_view_changed","payload":{"revision":result.get("revision")}}}})
        }
    };
    let _ = server.events.send(Event { session, body });
    Ok(result)
}

fn required_string(value: &Value, key: &str) -> Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| invalid(key))
}
fn validate_layouts(args: &Value) -> Result<()> {
    let layouts = match args.get("initial_windows").and_then(Value::as_array) {
        Some(v) if v.len() <= 1 => v.iter().filter_map(|w| w.get("layout")).collect::<Vec<_>>(),
        Some(_) => return Err(unsupported("only one initial shell tab is supported")),
        None => args.get("layout").into_iter().collect(),
    };
    for layout in layouts {
        if layout.get("split").is_some() {
            return Err(unsupported("split layouts are not supported on creation"));
        }
        if let Some(block) = layout.get("block")
            && (block
                .get("flavor")
                .and_then(Value::as_str)
                .is_some_and(|v| v != "com.superlogical.terminal.shell")
                || block
                    .get("options")
                    .and_then(|o| o.get("command"))
                    .and_then(Value::as_array)
                    .is_some_and(|v| !v.is_empty())
                || block
                    .get("options")
                    .and_then(|o| o.get("initial_input"))
                    .and_then(Value::as_str)
                    .is_some_and(|v| !v.is_empty()))
        {
            return Err(unsupported("only a single interactive shell can be opened"));
        }
    }
    Ok(())
}

fn issue_ticket<B: Backend>(
    server: &Arc<Server<B>>,
    client: &Arc<Client>,
    session: String,
    block: String,
) -> Result<Value> {
    // Validate placement before publishing a credential-bearing ticket.
    if !server.backend.pane_exists(&session, &block)? {
        return Err(missing("pane"));
    }
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    let mut tickets = server.tickets.lock().unwrap();
    let now = Instant::now();
    tickets.retain(|_, t| now < t.expires && t.client.upgrade().is_some_and(|c| c.is_live()));
    if tickets.len() >= MAX_TICKETS {
        return Err(conflict("too many outstanding data tickets"));
    }
    tickets.insert(
        bytes,
        Ticket {
            session,
            block,
            client: Arc::downgrade(client),
            expires: now + TICKET_TTL,
        },
    );
    Ok(json!({"ticket":URL_SAFE_NO_PAD.encode(bytes),"expires_in_ms":60_000}))
}

async fn data<B: Backend>(
    State(server): State<Arc<Server<B>>>,
    ws: WebSocketUpgrade,
    headers: HeaderMap,
) -> Response {
    if let Some(r) = reject_origin(&headers).await {
        return r;
    }
    if !request_protocol(&headers, DATA_PROTOCOL) {
        return (StatusCode::BAD_REQUEST, "required Rex subprotocol missing").into_response();
    }
    if !server.limits.lock().unwrap().acquire(false) {
        return (StatusCode::SERVICE_UNAVAILABLE, "too many data connections").into_response();
    }
    ws.protocols([DATA_PROTOCOL])
        .max_message_size(4096)
        .max_frame_size(4096)
        .on_upgrade(move |socket| async move {
            data_socket(server.clone(), socket).await;
            server.limits.lock().unwrap().release(false);
        })
}
async fn reject_data(socket: &mut WebSocket, reason: u8) {
    let _ = socket
        .send(Message::Binary(vec![0x82, reason].into()))
        .await;
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code: 1008,
            reason: "data handshake rejected".into(),
        })))
        .await;
}
async fn data_socket<B: Backend>(server: Arc<Server<B>>, mut socket: WebSocket) {
    let Some(Ok(Message::Binary(hello))) = recv_timeout(&mut socket, 5).await else {
        return;
    };
    if hello.len() < 17 {
        reject_data(&mut socket, 5).await;
        return;
    }
    if hello[0] == 2 {
        reject_data(&mut socket, 1).await;
        return;
    }
    if hello[0] != 1 {
        reject_data(&mut socket, 5).await;
        return;
    }
    let id: [u8; 16] = hello[1..17].try_into().unwrap();
    let ticket = server.tickets.lock().unwrap().remove(&id);
    let Some(ticket) = ticket else {
        reject_data(&mut socket, 1).await;
        return;
    };
    let Some(client) = ticket.client.upgrade() else {
        reject_data(&mut socket, 1).await;
        return;
    };
    if ticket.expires <= Instant::now() || !client.is_live() {
        reject_data(&mut socket, 1).await;
        return;
    }
    let Ok(mut pane) = server
        .backend
        .pane_attach(&client.id, &ticket.session, &ticket.block, None)
    else {
        reject_data(&mut socket, 2).await;
        return;
    };
    let mut welcome = [0u8; 32];
    welcome[0] = 0x81;
    rand::thread_rng().fill_bytes(&mut welcome[1..17]);
    let mut nonce_bytes = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    if nonce_bytes == [0; 8] {
        nonce_bytes[0] = 1;
    }
    welcome[24..].copy_from_slice(&nonce_bytes);
    let nonce = u64::from_le_bytes(nonce_bytes);
    if socket
        .send(Message::Binary(welcome.to_vec().into()))
        .await
        .is_err()
    {
        return;
    }
    if socket
        .send(Message::Binary(BLANK_SNAPSHOT.to_vec().into()))
        .await
        .is_err()
    {
        return;
    }
    for frame in pane.initial {
        if socket.send(Message::Binary(frame.into())).await.is_err() {
            return;
        }
    }
    if send_apc(&mut socket,nonce,json!({"type":"resize","cols":pane.columns,"rows":pane.rows,"cell_width_px":pane.cell_width_px,"cell_height_px":pane.cell_height_px,"owner":client.id})).await.is_err(){return;}
    for chunk in pane.replay.chunks(64 << 10) {
        if socket
            .send(Message::Binary(chunk.to_vec().into()))
            .await
            .is_err()
        {
            return;
        }
    }
    let mut heartbeat = tokio::time::interval(HEARTBEAT);
    let mut idle = Box::pin(tokio::time::sleep(IDLE));
    if let Some(code) = pane.exited {
        let _ = send_apc(
            &mut socket,
            nonce,
            json!({"type":"child_exited","exit_code":code}),
        )
        .await;
        return;
    }
    loop {
        tokio::select! {_= &mut idle=>break,_=heartbeat.tick()=>{if socket.send(Message::Ping(Vec::new().into())).await.is_err(){break;}},msg=socket.recv()=>match msg{Some(Ok(Message::Pong(_)))=>idle.as_mut().reset(tokio::time::Instant::now()+IDLE),Some(Ok(Message::Close(_)))|None=>break,Some(Err(_))=>break,_=>{}},update=pane.updates.recv()=>match update{Some(PaneUpdate::Output(v))=>if socket.send(Message::Binary(v.into())).await.is_err(){break},Some(PaneUpdate::Resize{columns,rows,cell_width_px,cell_height_px})=>if send_apc(&mut socket,nonce,json!({"type":"resize","cols":columns,"rows":rows,"cell_width_px":cell_width_px,"cell_height_px":cell_height_px,"owner":client.id})).await.is_err(){break},Some(PaneUpdate::Exit(code))=>{let _=send_apc(&mut socket,nonce,json!({"type":"child_exited","exit_code":code})).await;break},None=>break}}
    }
}
async fn send_apc(
    socket: &mut WebSocket,
    nonce: u64,
    value: Value,
) -> std::result::Result<(), axum::Error> {
    socket
        .send(Message::Binary(
            format!("\x1b_rex1;{nonce};{}\x1b\\", value)
                .into_bytes()
                .into(),
        ))
        .await
}

#[derive(Deserialize)]
struct SessionQuery {
    session: String,
}
async fn sessions<B: Backend>(State(s): State<Arc<Server<B>>>) -> Response {
    http_result(s.backend.summaries().map(|x| json!({"sessions":x})))
}
async fn session_view<B: Backend>(
    State(s): State<Arc<Server<B>>>,
    Query(q): Query<SessionQuery>,
) -> Response {
    http_result(s.backend.view(&q.session))
}
async fn capabilities<B: Backend>(State(s): State<Arc<Server<B>>>) -> Json<Value> {
    Json(json!({"capture":true,"native_rex":true,"uploads":s.backend.uploads_enabled()}))
}
async fn upload<B: Backend>(
    State(s): State<Arc<Server<B>>>,
    Query(q): Query<HashMap<String, String>>,
    mut form: Multipart,
) -> Response {
    let Some(session) = q.get("session") else {
        return http_error(invalid("session"));
    };
    let Some(block) = q.get("block") else {
        return http_error(invalid("block"));
    };
    let field = match form.next_field().await {
        Ok(Some(field)) => field,
        Ok(None) => return http_error(invalid("upload file")),
        Err(_) => return http_error(invalid("multipart upload")),
    };
    let filename = field.file_name().unwrap_or("upload").to_owned();
    let data: Bytes = match field.bytes().await {
        Ok(data) => data,
        Err(_) => return http_error(invalid("multipart upload")),
    };
    if data.len() > 16 << 20 {
        return http_error(
            WireError {
                code: "payload_too_large",
                message: "upload exceeds 16 MiB".into(),
                status: 413,
            }
            .into(),
        );
    }
    let mut input = &data[..];
    http_result(s.backend.upload(session, block, &filename, &mut input))
}

async fn action<B: Backend>(State(s): State<Arc<Server<B>>>, Json(args): Json<Value>) -> Response {
    if args.get("op").and_then(Value::as_str).is_none() {
        return http_error(invalid("action"));
    }
    http_result(mutate_and_notify(&s, None, "browser.action", args))
}
async fn capture<B: Backend>(
    State(s): State<Arc<Server<B>>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(session) = q.get("session") else {
        return http_error(invalid("session"));
    };
    let Some(block) = q.get("block") else {
        return http_error(invalid("block"));
    };
    let format = q.get("format").map(String::as_str).unwrap_or("text");
    if !matches!(format, "text" | "vt" | "html") {
        return http_error(invalid("format"));
    }
    match s.backend.pane_capture(
        session,
        block,
        format,
        q.get("trim").map(String::as_str).unwrap_or("true") != "false",
        q.get("unwrap")
            .map(String::as_str)
            .is_some_and(|v| v == "true"),
    ) {
        Ok(bytes) => {
            let mime = if format == "html" {
                "text/html; charset=utf-8"
            } else {
                "text/plain; charset=utf-8"
            };
            let mut r = bytes.into_response();
            r.headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
            r.headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            if format == "html" {
                r.headers_mut().insert(
                    header::CONTENT_SECURITY_POLICY,
                    HeaderValue::from_static(
                        "default-src 'none'; style-src 'unsafe-inline'; sandbox",
                    ),
                );
            }
            r
        }
        Err(e) => http_error(e),
    }
}
fn http_result(value: Result<Value>) -> Response {
    match value {
        Ok(v) => Json(v).into_response(),
        Err(e) => http_error(e),
    }
}
fn http_error(e: anyhow::Error) -> Response {
    let (status, body) = if let Some(w) = e.downcast_ref::<WireError>() {
        (
            StatusCode::from_u16(w.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            json!({"message":w.message}),
        )
    } else {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"message":"workspace operation failed"}),
        )
    };
    (status, Json(body)).into_response()
}

async fn browser_terminal<B: Backend>(
    State(s): State<Arc<Server<B>>>,
    Query(q): Query<HashMap<String, String>>,
    ws: WebSocketUpgrade,
) -> Response {
    let Some(session) = q.get("session").cloned() else {
        return http_error(invalid("session"));
    };
    let Some(block) = q.get("block").cloned() else {
        return http_error(invalid("block"));
    };
    let offset = q.get("offset").and_then(|v| v.parse::<u64>().ok());
    if q.contains_key("offset") && offset.is_none() {
        return http_error(invalid("stream offset"));
    }
    ws.on_upgrade(move |socket| browser_socket(s, socket, session, block, offset))
}
async fn browser_socket<B: Backend>(
    s: Arc<Server<B>>,
    ws: WebSocket,
    session: String,
    block: String,
    offset: Option<u64>,
) {
    let client = random_id();
    let backend = s.backend.clone();
    browser_stream(&s, ws, &client, &session, &block, offset).await;
    let _ = backend.client_disconnected(&client);
}

async fn browser_stream<B: Backend>(
    s: &Arc<Server<B>>,
    mut ws: WebSocket,
    client: &str,
    session: &str,
    block: &str,
    offset: Option<u64>,
) {
    let Ok(mut pane) = s.backend.pane_attach(client, session, block, offset) else {
        return;
    };
    if ws
        .send(Message::Text(
            json!(["stream",{"offset":pane.offset,"reset":pane.reset}])
                .to_string()
                .into(),
        ))
        .await
        .is_err()
    {
        return;
    }
    if !pane.replay.is_empty() && ws.send(Message::Binary(pane.replay.into())).await.is_err() {
        return;
    }
    if ws
        .send(Message::Text(json!(["ready", null]).to_string().into()))
        .await
        .is_err()
    {
        return;
    }
    loop {
        tokio::select! {msg=ws.recv()=>match msg{Some(Ok(Message::Text(v)))=>{let Ok(frames):std::result::Result<Vec<Value>,_>=serde_json::from_str(&v)else{break;};if frames.len()!=2{break;}match frames[0].as_str(){Some("stdin")=>{let Some(text)=frames[1].as_str()else{break;};if text.len()>64<<10||s.backend.pane_input(client,session,block,text.as_bytes()).is_err(){break;}},Some("resize")=>{if s.backend.pane_method(client,session,block,"resize",&frames[1]).is_err(){break;}},Some("ping")=>{if ws.send(Message::Text(json!(["pong",frames[1]]).to_string().into())).await.is_err(){break;}},_=>break}},Some(Ok(Message::Close(_)))|None=>break,Some(Err(_))=>break,_=>{}},update=pane.updates.recv()=>match update{Some(PaneUpdate::Output(v))=>if ws.send(Message::Binary(v.into())).await.is_err(){break},Some(PaneUpdate::Exit(_))=>{let _=ws.send(Message::Text(json!(["exit",null]).to_string().into())).await;break},Some(PaneUpdate::Resize{..})=>{},None=>break}}
    }
}

fn creator(host: String) -> Value {
    json!({"name":"com.superlogical.terminal","title":"Terminal","description":"An interactive shell in the workspace","host_id":host,"flavors":[{"name":"shell","flavor":"com.superlogical.terminal.shell","title":"Shell","description":"An interactive shell","options_schema":{"type":"object"}}],"api":terminal_api()})
}
fn terminal_api() -> Value {
    let methods: [&str; 11] = [
        "write",
        "resize",
        "size",
        "input_mode",
        "title",
        "pwd",
        "process",
        "program_status",
        "set_theme",
        "format",
        "blob_read",
    ];
    let mut map = serde_json::Map::new();
    for m in methods {
        map.insert(m.into(), json!({"description":format!("Terminal {m}")}));
    }
    json!({"methods":map,"events":{"paste":{"type":"object","properties":{"text":{"type":"string"}}},"key":{"type":"object"},"focus":{"type":"object"}}})
}
fn keymap<B: Backend>(args: &Value, server: &Arc<Server<B>>) -> Result<Value> {
    #[derive(Deserialize, Default)]
    struct Layer {
        #[serde(default)]
        name: String,
        #[serde(default)]
        bindings: Vec<Value>,
        #[serde(default)]
        removals: Vec<Value>,
    }
    #[derive(Deserialize, Default)]
    struct Request {
        #[serde(default)]
        defaults: Vec<Layer>,
        #[serde(default)]
        overrides: Vec<Layer>,
    }
    let request: Request = if args.is_null() {
        Request::default()
    } else {
        serde_json::from_value(args.clone()).map_err(|_| invalid("keymap"))?
    };
    let mut bindings = Vec::new();
    let mut names = HashSet::new();
    for layer in request.defaults.into_iter().chain(request.overrides) {
        if layer.name.is_empty() || layer.name.contains(':') || !names.insert(layer.name.clone()) {
            return Err(invalid("keymap layer name"));
        }
        for removal in layer.removals {
            bindings.retain(|binding: &Value| {
                binding.get("mode") != removal.get("mode")
                    || (removal.get("action").is_some_and(|v| !v.is_null())
                        && binding.get("action") != removal.get("action"))
                    || (removal
                        .get("keys")
                        .is_some_and(|v| binding.get("keys") != Some(v)))
            });
        }
        for mut binding in layer.bindings {
            if let Some(o) = binding.as_object_mut() {
                o.insert("source".into(), json!(layer.name));
            }
            bindings.push(binding);
        }
    }
    bindings.reverse();
    Ok(
        json!({"bindings":bindings,"diagnostics":[],"config":{"path":".home/init.lua","exists":false,"generation":1,"instance_id":server.instance,"hash":""}}),
    )
}
