//! Foreground Rexweb service entry point.

use std::{
    fs::{self, File, OpenOptions},
    io::ErrorKind,
    net::{SocketAddr, TcpListener as StdTcpListener},
    os::unix::{
        fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        io::AsRawFd,
    },
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use axum::{
    Router,
    body::Body,
    extract::Request,
    http::{HeaderValue, Method, StatusCode, Uri, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rexweb_backend::{linux, rex, service::Service, vt};
use rust_embed::RustEmbed;
use serde_json::json;
use tokio::{
    net::UnixListener,
    signal::unix::{SignalKind, signal},
};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(RustEmbed)]
#[folder = "../../web/dist/"]
struct Embedded;

#[derive(Clone, Debug)]
struct Options {
    mode: String,
    listen: String,
    rex_listen: String,
    socket: String,
    shell: String,
    workspace: String,
    isolation: String,
    web_dir: String,
    public_url: String,
    base_path: String,
    password_file: String,
    tls_cert: String,
    tls_key: String,
    uploads: bool,
    anonymous: bool,
    insecure: bool,
}

impl Default for Options {
    fn default() -> Self {
        let run = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        Self {
            mode: "web".into(),
            listen: "127.0.0.1:8080".into(),
            rex_listen: "127.0.0.1:7297".into(),
            socket: run
                .join("rexweb/session.sock")
                .to_string_lossy()
                .into_owned(),
            shell: "/bin/sh".into(),
            workspace: String::new(),
            isolation: "sandbox".into(),
            web_dir: String::new(),
            public_url: String::new(),
            base_path: "/".into(),
            password_file: String::new(),
            tls_cert: String::new(),
            tls_key: String::new(),
            uploads: false,
            anonymous: false,
            insecure: false,
        }
    }
}

fn usage() {
    eprintln!(
        "Usage: rexweb [options]\n\n  -mode web|rex|both        listeners (private Unix Rex socket is always enabled)\n  -listen ADDRESS           web IP:port (default 127.0.0.1:8080)\n  -rex-listen ADDRESS       native Rex IP:port\n  -session-socket PATH      private Unix socket\n  -workspace PATH           private workspace outside the account home\n  -shell PATH               interactive shell\n  -isolation sandbox|host    Linux namespace sandbox or explicit host access\n  -web-dir PATH             development frontend directory\n  -base-path PREFIX         URL prefix with leading and trailing slashes\n  -public-url ORIGIN        exact external HTTP(S) origin\n  -password-file PATH       private Basic-auth password\n  -tls-cert PATH            TLS certificate\n  -tls-key PATH             TLS private key\n  -allow-anonymous          permit remote native Rex access\n  -allow-insecure-http      permit authenticated remote HTTP behind trusted TLS proxy\n  -uploads                  enable uploads (supported only when backend integration is available)\n  -version                  print version and exit"
    );
}

fn parse_args() -> Result<Option<Options>> {
    let mut o = Options::default();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let raw = &args[i];
        if raw == "-h" || raw == "--help" || raw == "-help" {
            usage();
            return Ok(None);
        }
        if raw == "-version" || raw == "--version" || raw == "-version=true" {
            println!("rexweb {VERSION} (linux/{})", std::env::consts::ARCH);
            return Ok(None);
        }
        let (name, inline) = raw
            .strip_prefix("--")
            .or_else(|| raw.strip_prefix('-'))
            .ok_or_else(|| anyhow!("unexpected positional argument: {raw}"))?
            .split_once('=')
            .map_or((raw.trim_start_matches('-'), None), |(a, b)| (a, Some(b)));
        let set_bool = |value: &str, dst: &mut bool| -> Result<()> {
            *dst = match value {
                "true" | "1" => true,
                "false" | "0" => false,
                _ => bail!("invalid boolean value for -{name}: {value}"),
            };
            Ok(())
        };
        let is_bool = matches!(name, "allow-anonymous" | "allow-insecure-http" | "uploads");
        let value = if let Some(value) = inline {
            value.to_owned()
        } else if is_bool {
            if args
                .get(i + 1)
                .is_some_and(|v| matches!(v.as_str(), "true" | "false" | "1" | "0"))
            {
                i += 1;
                args[i].clone()
            } else {
                "true".into()
            }
        } else {
            i += 1;
            args.get(i)
                .cloned()
                .ok_or_else(|| anyhow!("missing value for -{name}"))?
        };
        match name {
            "mode" => o.mode = value,
            "listen" => o.listen = value,
            "rex-listen" => o.rex_listen = value,
            "session-socket" => o.socket = value,
            "shell" => o.shell = value,
            "workspace" => o.workspace = value,
            "isolation" => o.isolation = value,
            "web-dir" => o.web_dir = value,
            "public-url" => o.public_url = value,
            "base-path" => o.base_path = value,
            "password-file" => o.password_file = value,
            "tls-cert" => o.tls_cert = value,
            "tls-key" => o.tls_key = value,
            "allow-anonymous" => set_bool(&value, &mut o.anonymous)?,
            "allow-insecure-http" => set_bool(&value, &mut o.insecure)?,
            "uploads" => set_bool(&value, &mut o.uploads)?,
            _ => bail!("flag provided but not defined: -{name}"),
        }
        i += 1;
    }
    Ok(Some(o))
}

fn bind_address(address: &str) -> String {
    if address.starts_with(':') {
        format!("0.0.0.0{address}")
    } else {
        address.to_owned()
    }
}

fn loopback_address(address: &str) -> Result<bool> {
    if address.starts_with(':') {
        return Ok(false);
    }
    let address: SocketAddr = address
        .parse()
        .with_context(|| format!("invalid listener address {address}"))?;
    Ok(address.ip().is_loopback())
}

fn valid_origin(origin: &str) -> bool {
    let Ok(uri): std::result::Result<Uri, _> = origin.parse() else {
        return false;
    };
    matches!(uri.scheme_str(), Some("http" | "https"))
        && uri.authority().is_some_and(|a| !a.as_str().contains('@'))
        && uri.host().is_some()
        && uri.path() == "/"
        && uri.query().is_none()
}

fn validate(o: &Options) -> Result<()> {
    ensure!(cfg!(target_os = "linux"), "this service requires Linux");
    ensure!(
        matches!(o.mode.as_str(), "web" | "rex" | "both"),
        "-mode must be web, rex, or both"
    );
    ensure!(
        matches!(o.isolation.as_str(), "sandbox" | "host"),
        "-isolation must be sandbox or host"
    );
    ensure!(
        Path::new(&o.shell).is_absolute() && Path::new(&o.socket).is_absolute(),
        "absolute -shell and -session-socket paths required"
    );
    ensure!(
        o.base_path == "/"
            || (o.base_path.starts_with('/')
                && o.base_path.ends_with('/')
                && o.base_path[1..o.base_path.len() - 1]
                    .split('/')
                    .all(|p| !p.is_empty()
                        && p.bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))),
        "-base-path must be a URL prefix with leading and trailing slashes"
    );
    ensure!(
        (o.tls_cert.is_empty()) == (o.tls_key.is_empty()),
        "-tls-cert and -tls-key must be supplied together"
    );
    ensure!(
        !o.uploads || !o.password_file.is_empty(),
        "-uploads requires -password-file"
    );
    if o.mode != "rex" {
        ensure!(
            loopback_address(&o.listen)?
                || (!o.password_file.is_empty() && !o.public_url.is_empty()),
            "remote web requires -password-file and -public-url"
        );
        if !loopback_address(&o.listen)? {
            ensure!(
                !o.tls_cert.is_empty() || o.insecure,
                "remote web requires TLS or explicit -allow-insecure-http behind a trusted TLS tunnel"
            );
        }
        ensure!(
            o.public_url.is_empty() || valid_origin(&o.public_url),
            "-public-url must be an HTTP(S) origin without path, credentials or query"
        );
        ensure!(
            o.tls_cert.is_empty()
                || o.public_url.is_empty()
                || o.public_url.starts_with("https://"),
            "TLS listener requires HTTPS public URL"
        );
    }
    if o.mode != "web" {
        ensure!(
            loopback_address(&o.rex_listen)? || o.anonymous,
            "non-loopback Rex requires explicit -allow-anonymous; prefer SSH forwarding of the private Unix socket"
        );
    }
    Ok(())
}

fn read_password(path: &str) -> Result<Option<Vec<u8>>> {
    if path.is_empty() {
        return Ok(None);
    }
    let meta = fs::symlink_metadata(path)?;
    ensure!(
        meta.is_file() && meta.permissions().mode() & 0o077 == 0 && meta.len() <= 4096,
        "password file must be a private regular file (0600), at most 4096 bytes"
    );
    let mut bytes = fs::read(path)?;
    while matches!(bytes.last(), Some(b'\r' | b'\n')) {
        bytes.pop();
    }
    ensure!(
        bytes.len() >= 16,
        "web password must have at least 16 characters"
    );
    Ok(Some(bytes))
}

fn constant_equal(a: &[u8], b: &[u8]) -> bool {
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        diff |= a.get(i).copied().unwrap_or(0) as usize ^ b.get(i).copied().unwrap_or(0) as usize;
    }
    diff == 0
}

#[derive(Clone)]
struct WebPolicy {
    expected_host: Option<String>,
    public_url: Option<String>,
    password: Option<Vec<u8>>,
    tls: bool,
}

async fn web_policy(
    policy: axum::extract::State<WebPolicy>,
    request: Request,
    next: Next,
) -> Response {
    let p = policy.0;
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .or_else(|| request.uri().authority().map(|a| a.as_str()))
        .unwrap_or("");
    let valid_host = if let Some(expected) = &p.expected_host {
        host.eq_ignore_ascii_case(expected)
    } else {
        let name = if host.starts_with('[') {
            host.strip_prefix('[')
                .and_then(|v| v.split(']').next())
                .unwrap_or("")
        } else {
            host.rsplit_once(':').map_or(host, |(name, _)| name)
        };
        name.eq_ignore_ascii_case("localhost")
            || name
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    };
    if !valid_host {
        return (StatusCode::FORBIDDEN, "unexpected Host").into_response();
    }
    let expected_origin = p
        .public_url
        .clone()
        .unwrap_or_else(|| format!("{}://{host}", if p.tls { "https" } else { "http" }));
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok());
    let path = request.uri().path();
    let requires_origin = request.method() != Method::GET && request.method() != Method::HEAD
        || path.ends_with("/api/terminal");
    if origin.is_some_and(|v| v != expected_origin)
        || (requires_origin && origin != Some(expected_origin.as_str()))
    {
        return (StatusCode::FORBIDDEN, "same-origin request required").into_response();
    }
    if let Some(password) = &p.password {
        let supplied = request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Basic "))
            .and_then(|v| STANDARD.decode(v).ok());
        let (user, pass) = supplied
            .as_deref()
            .and_then(|v| {
                v.iter()
                    .position(|b| *b == b':')
                    .map(|n| (&v[..n], &v[n + 1..]))
            })
            .unwrap_or((&[], &[]));
        if user != b"rexweb" || !constant_equal(pass, password) {
            return (
                StatusCode::UNAUTHORIZED,
                [(
                    header::WWW_AUTHENTICATE,
                    "Basic realm=\"rexweb\", charset=\"UTF-8\"",
                )],
                "authentication required",
            )
                .into_response();
        }
    }
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert("cache-control", HeaderValue::from_static("no-store"));
    response
}

async fn native_policy(request: Request, next: Next) -> Response {
    if request.headers().contains_key(header::ORIGIN) {
        return (
            StatusCode::FORBIDDEN,
            "cross-origin Rex access is forbidden",
        )
            .into_response();
    }
    next.run(request).await
}

#[derive(Clone)]
struct Assets {
    root: Option<PathBuf>,
    base: String,
}
fn asset_response(name: &str, assets: &Assets) -> Result<Response> {
    let bytes = if let Some(root) = &assets.root {
        fs::read(root.join(name))?
    } else {
        Embedded::get(name)
            .map(|f| f.data.into_owned())
            .ok_or_else(|| anyhow!("not found"))?
    };
    let bytes = if name == "index.html" {
        String::from_utf8(bytes)?
            .replace("__REXWEB_BASE_PATH__", &assets.base)
            .into_bytes()
    } else {
        bytes
    };
    let mime = match Path::new(name).extension().and_then(|v| v.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    };
    Ok(([(header::CONTENT_TYPE, mime)], Body::from(bytes)).into_response())
}
async fn index(axum::extract::State(a): axum::extract::State<Assets>) -> Response {
    asset_response("index.html", &a).unwrap_or_else(|_| {
        (StatusCode::INTERNAL_SERVER_ERROR, "frontend unavailable").into_response()
    })
}
async fn asset(
    axum::extract::State(a): axum::extract::State<Assets>,
    request: Request,
) -> Response {
    let name = request.uri().path().trim_start_matches('/');
    if name.is_empty() || name.ends_with('/') || name.split('/').any(|p| p == ".." || p == ".") {
        return StatusCode::NOT_FOUND.into_response();
    }
    asset_response(name, &a).unwrap_or_else(|_| StatusCode::NOT_FOUND.into_response())
}

fn prepare_socket_dir(path: &str) -> Result<(PathBuf, File)> {
    let dir = Path::new(path)
        .parent()
        .ok_or_else(|| anyhow!("socket has no parent"))?;
    let created = !dir.exists();
    fs::create_dir_all(dir)?;
    if created {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    let metadata = fs::symlink_metadata(dir)?;
    ensure!(
        metadata.is_dir()
            && metadata.uid() == unsafe { libc::getuid() }
            && metadata.permissions().mode() & 0o077 == 0,
        "session socket directory must be owned by this account and private: {}",
        dir.display()
    );
    let lock_path = dir.join("service.lock");
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(lock_path)?;
    let rc = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error())
            .context("a service instance is already starting or running");
    }
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            ensure!(
                meta.file_type().is_socket(),
                "refusing to replace a non-socket file at {path}"
            );
            match std::os::unix::net::UnixStream::connect(path) {
                Ok(_) => bail!("a server is already listening on {path}"),
                Err(e) if e.raw_os_error() == Some(libc::ECONNREFUSED) => fs::remove_file(path)?,
                Err(e) => return Err(e).context("check existing session socket"),
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok((dir.to_path_buf(), lock))
}

async fn spawn_server(
    listener: StdTcpListener,
    router: Router,
    tls: Option<(String, String)>,
) -> Result<()> {
    listener.set_nonblocking(true)?;
    if let Some((cert, key)) = tls {
        use axum_server::tls_rustls::RustlsConfig;
        let config = RustlsConfig::from_pem_file(cert, key).await?;
        return axum_server::from_tcp_rustls(listener, config)?
            .serve(router.into_make_service())
            .await
            .map_err(Into::into);
    }
    axum::serve(tokio::net::TcpListener::from_std(listener)?, router).await?;
    Ok(())
}

async fn run(o: Options) -> Result<()> {
    validate(&o)?;
    let password = read_password(&o.password_file)?;
    let (socket_dir, _lock) = prepare_socket_dir(&o.socket)?;
    let sandbox = linux::Sandbox::prepare(
        if o.workspace.is_empty() {
            Path::new("")
        } else {
            Path::new(&o.workspace)
        },
        &socket_dir,
        o.isolation == "host",
    )?;
    if o.isolation == "host" {
        eprintln!(
            "WARNING: host isolation disabled; shells have full service-account file and network access"
        );
    }
    let wasm = Embedded::get("static/js/ghostty-vt.wasm")
        .ok_or_else(|| anyhow!("embedded VT WASM unavailable"))?;
    let engine = vt::Engine::new(&wasm.data)?;
    let workspace_root = sandbox.root().display().to_string();
    let service = Arc::new(Service::new(sandbox, o.shell.clone(), engine));
    if o.uploads {
        service.enable_uploads(Path::new(&workspace_root))?;
    }
    service
        .apply(&json!({"op":"ensure-default"}))
        .context("start sandboxed shell")?;
    let (web, native) = rex::routers(service.clone());
    let native = native.layer(middleware::from_fn(native_policy));
    let local = UnixListener::bind(&o.socket)?;
    fs::set_permissions(&o.socket, fs::Permissions::from_mode(0o600))?;
    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn(async move {
        axum::serve(local, native).await?;
        Ok::<(), anyhow::Error>(())
    });
    if o.mode != "rex" {
        let root = (!o.web_dir.is_empty()).then(|| PathBuf::from(&o.web_dir));
        if let Some(root) = &root {
            ensure!(
                root.join("index.html").is_file(),
                "frontend: index.html not found"
            );
        }
        let assets = Assets {
            root,
            base: o.base_path.clone(),
        };
        let frontend = Router::new()
            .route("/", get(index))
            .route("/index.html", get(index))
            .fallback(get(asset))
            .with_state(assets);
        let application = web.fallback_service(frontend);
        let application = if o.base_path == "/" {
            application
        } else {
            Router::new().nest(o.base_path.trim_end_matches('/'), application)
        };
        let normalized_public_url = o.public_url.trim_end_matches('/');
        let public_host = normalized_public_url
            .strip_prefix("https://")
            .or_else(|| normalized_public_url.strip_prefix("http://"))
            .filter(|_| !o.public_url.is_empty())
            .map(str::to_owned);
        let policy = WebPolicy {
            expected_host: public_host,
            password,
            public_url: (!o.public_url.is_empty())
                .then(|| o.public_url.trim_end_matches('/').to_owned()),
            tls: !o.tls_cert.is_empty(),
        };
        let router = application.layer(middleware::from_fn_with_state(policy, web_policy));
        let listener = StdTcpListener::bind(bind_address(&o.listen))?;
        let address = listener.local_addr()?;
        eprintln!("Web listener at {address}");
        let cert = o.tls_cert.clone();
        let key = o.tls_key.clone();
        let tls = (!cert.is_empty()).then_some((cert, key));
        tasks.spawn(async move { spawn_server(listener, router, tls).await });
    }
    if o.mode != "web" {
        let listener = StdTcpListener::bind(bind_address(&o.rex_listen))?;
        let address = listener.local_addr()?;
        eprintln!("Rex listener at {address}");
        let router = rex::routers(service.clone())
            .1
            .layer(middleware::from_fn(native_policy));
        let cert = o.tls_cert.clone();
        let key = o.tls_key.clone();
        let tls = (!cert.is_empty()).then_some((cert, key));
        tasks.spawn(async move { spawn_server(listener, router, tls).await });
    }
    eprintln!(
        "Ready: local Rex socket {}; workspace {}; isolation {}",
        o.socket, workspace_root, o.isolation
    );
    let mut terminate = signal(SignalKind::terminate())?;
    tokio::select! {
        _=tokio::signal::ctrl_c()=>{},
        _=terminate.recv()=>{},
        result=tasks.join_next()=>{ if let Some(result)=result { result??; } }
    }
    tasks.abort_all();
    service.close();
    let _ = fs::remove_file(&o.socket);
    Ok(())
}

#[tokio::main]
async fn main() {
    if std::env::args().nth(1).as_deref() == Some("sandbox-exec") {
        if let Err(error) = linux::run_helper(&std::env::args().skip(2).collect::<Vec<_>>()) {
            eprintln!("sandbox: {error}");
            std::process::exit(1);
        }
        return;
    }
    let options = match parse_args() {
        Ok(Some(o)) => o,
        Ok(None) => return,
        Err(error) => {
            eprintln!("rexweb: {error}");
            std::process::exit(1)
        }
    };
    if let Err(error) = run(options).await {
        eprintln!("rexweb: {error}");
        std::process::exit(1);
    }
}
