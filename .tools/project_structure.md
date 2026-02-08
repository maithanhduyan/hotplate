# Cấu trúc Dự án như sau:

```
./
├── Cargo.toml
├── apps
│   └── index.html
├── scripts
├── src
│   ├── inject.rs
│   ├── main.rs
│   ├── server.rs
│   ├── watcher.rs
│   └── welcome.html
└── vscode-extension
    ├── apps
    │   └── index.html
    ├── extension.js
    └── package.json
```

# Danh sách chi tiết các file:

## File ./src\inject.rs:
```rust
//! HTML injection middleware — inserts live-reload WebSocket script before </body>.

use axum::{
    body::Body,
    http::{header, Request, Response},
    middleware::Next,
};
use http_body_util::BodyExt;

/// Live-reload client script with CSS hot-swap support.
/// Connects to ws(s)://<host>/__lr.
///   - On "reload" message → full page reload.
///   - On "css:<path>" message → hot-swap only that stylesheet (no page reload).
/// Auto-reconnects after 1s on disconnect.
const RELOAD_SCRIPT: &str = r#"<script>
(()=>{
  const p=location.protocol==='https:'?'wss:':'ws:';
  let t;
  function reloadCSS(path){
    const links=document.querySelectorAll('link[rel="stylesheet"]');
    let found=false;
    links.forEach(link=>{
      const href=link.getAttribute('href');
      if(!href)return;
      const clean=href.split('?')[0];
      if(clean===path||clean==='/'+path||clean.endsWith('/'+path)){
        link.href=clean+'?_lr='+Date.now();
        found=true;
      }
    });
    if(!found)location.reload();
  }
  function connect(){
    const ws=new WebSocket(`${p}//${location.host}/__lr`);
    ws.onmessage=e=>{
      const d=e.data;
      if(d==='reload')location.reload();
      else if(d.startsWith('css:'))reloadCSS(d.slice(4));
    };
    ws.onclose=()=>{clearTimeout(t);t=setTimeout(connect,1000)};
    ws.onerror=()=>ws.close();
  }
  connect();
})();
</script>"#;

/// Axum middleware: if the response is HTML, inject the reload script.
pub async fn inject_livereload(req: Request<Body>, next: Next) -> Response<Body> {
    let resp = next.run(req).await;

    // Only process text/html responses
    let is_html = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("text/html"))
        .unwrap_or(false);

    if !is_html {
        return resp;
    }

    // Buffer the body
    let (mut parts, body) = resp.into_parts();
    let collected = match body.collect().await {
        Ok(c) => c.to_bytes(),
        Err(_) => return Response::from_parts(parts, Body::empty()),
    };

    let html = String::from_utf8_lossy(&collected);

    // Inject before </body>, or </html>, or at the end
    let injected = if let Some(pos) = html.rfind("</body>") {
        format!("{}{}\n{}", &html[..pos], RELOAD_SCRIPT, &html[pos..])
    } else if let Some(pos) = html.rfind("</html>") {
        format!("{}{}\n{}", &html[..pos], RELOAD_SCRIPT, &html[pos..])
    } else {
        format!("{}\n{}", html, RELOAD_SCRIPT)
    };

    // Remove old Content-Length (body size changed)
    parts.headers.remove(header::CONTENT_LENGTH);

    Response::from_parts(parts, Body::from(injected))
}

```

## File ./src\main.rs:
```rust
//! ⚡ hotplate — Fast HTTPS live-reload dev server
//!
//! Usage:
//!   hotplate --root ./apps --port 5500
//!   hotplate --https                   # auto-generates self-signed cert in .cert/
//!   hotplate --root ./apps --cert .cert/server.crt --key .cert/server.key
//!   hotplate                          # auto-reads .vscode/settings.json

mod inject;
mod server;
mod watcher;

use anyhow::{Context, Result};
use clap::Parser;
use serde::Deserialize;
use std::path::{Path, PathBuf};

// ───────────────────── CLI ─────────────────────

#[derive(Parser)]
#[command(name = "hotplate", about = "⚡ Fast HTTPS live-reload dev server")]
struct Cli {
    /// Bind host
    #[arg(long, default_value = "0.0.0.0")]
    host: String,

    /// Bind port
    #[arg(short, long, default_value_t = 5500)]
    port: u16,

    /// Root directory to serve
    #[arg(short, long)]
    root: Option<String>,

    /// TLS certificate path (PEM)
    #[arg(long)]
    cert: Option<String>,

    /// TLS private key path (PEM)
    #[arg(long)]
    key: Option<String>,

    /// Disable live reload
    #[arg(long, default_value_t = false)]
    no_reload: bool,

    /// Force full page reload on every change (disable CSS-only hot swap)
    #[arg(long, default_value_t = false)]
    full_reload: bool,

    /// Workspace directory (for loading .vscode/settings.json)
    #[arg(short, long)]
    workspace: Option<String>,

    /// Glob patterns of files to ignore for live reload (can be repeated)
    #[arg(long)]
    ignore: Vec<String>,

    /// Serve this file for every 404 (SPA fallback, e.g. "index.html")
    #[arg(long)]
    file: Option<String>,

    /// Proxy base URI (e.g. "/api")
    #[arg(long)]
    proxy_base: Option<String>,

    /// Proxy target URL (e.g. "http://127.0.0.1:8000")
    #[arg(long)]
    proxy_target: Option<String>,

    /// Custom response header (can be repeated, format: "Key: Value")
    #[arg(long = "header")]
    headers: Vec<String>,

    /// Mount an extra directory at a URL path (can be repeated, format: "/url_path:./fs_path")
    #[arg(long = "mount")]
    mounts: Vec<String>,

    /// Enable HTTPS (auto-generates self-signed cert if --cert/--key not provided)
    #[arg(long, default_value_t = false)]
    https: bool,
}

// ───────────────────── Config ─────────────────────

#[derive(Debug, Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub root: PathBuf,
    pub cert: Option<PathBuf>,
    pub key: Option<PathBuf>,
    pub live_reload: bool,
    pub full_reload: bool,
    pub workspace: PathBuf,
    pub ignore_patterns: Vec<String>,
    pub spa_file: Option<String>,
    pub proxy_base: Option<String>,
    pub proxy_target: Option<String>,
    pub headers: Vec<(String, String)>,
    pub mounts: Vec<(String, PathBuf)>,
}

// ───────────────────── VS Code settings.json ─────────────────────

#[derive(Debug, Deserialize, Default)]
#[allow(dead_code)]
struct VsCodeHttps {
    enable: Option<bool>,
    cert: Option<String>,
    key: Option<String>,
    passphrase: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[allow(dead_code)]
struct VsCodeSettings {
    #[serde(rename = "hotplate.host")]
    host: Option<String>,
    #[serde(rename = "hotplate.port")]
    port: Option<u16>,
    #[serde(rename = "hotplate.root")]
    root: Option<String>,
    #[serde(rename = "hotplate.https")]
    https: Option<VsCodeHttps>,
}

/// Strip // and /* */ comments and trailing commas from JSONC
fn strip_jsonc(input: &str) -> String {
    // Pass 1: strip comments
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escape = false;

    while let Some(c) = chars.next() {
        if escape {
            out.push(c);
            escape = false;
            continue;
        }
        if in_string {
            if c == '\\' {
                escape = true;
            }
            if c == '"' {
                in_string = false;
            }
            out.push(c);
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            continue;
        }
        if c == '/' {
            match chars.peek() {
                Some(&'/') => {
                    for nc in chars.by_ref() {
                        if nc == '\n' {
                            out.push('\n');
                            break;
                        }
                    }
                    continue;
                }
                Some(&'*') => {
                    chars.next();
                    while let Some(nc) = chars.next() {
                        if nc == '*' && chars.peek() == Some(&'/') {
                            chars.next();
                            break;
                        }
                    }
                    continue;
                }
                _ => {}
            }
        }
        out.push(c);
    }

    // Pass 2: strip trailing commas (,  followed by } or ])
    let bytes = out.as_bytes();
    let mut result = String::with_capacity(out.len());
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        if bytes[i] == b',' {
            // Look ahead: skip whitespace, check for } or ]
            let mut j = i + 1;
            while j < len && (bytes[j] == b' ' || bytes[j] == b'\t' || bytes[j] == b'\n' || bytes[j] == b'\r') {
                j += 1;
            }
            if j < len && (bytes[j] == b'}' || bytes[j] == b']') {
                // Skip this comma, push the whitespace
                i += 1;
                continue;
            }
        }
        result.push(bytes[i] as char);
        i += 1;
    }

    result
}

fn load_vscode_settings(workspace: &Path) -> Option<VsCodeSettings> {
    let path = workspace.join(".vscode/settings.json");
    let content = std::fs::read_to_string(&path).ok()?;
    let cleaned = strip_jsonc(&content);
    match serde_json::from_str(&cleaned) {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("  ⚠ Failed to parse {}: {}", path.display(), e);
            None
        }
    }
}

/// Resolve a possibly-relative path against the workspace root.
fn resolve_path(workspace: &Path, p: &str) -> PathBuf {
    let path = PathBuf::from(p);
    if path.is_absolute() {
        path
    } else {
        workspace.join(path)
    }
}

/// Generate a self-signed TLS certificate and key in `<workspace>/.cert/`.
/// Returns the paths to the generated cert and key files.
/// If the files already exist, they are reused without regeneration.
fn generate_self_signed_cert(workspace: &Path) -> Result<(PathBuf, PathBuf)> {
    let cert_dir = workspace.join(".cert");
    let cert_path = cert_dir.join("hotplate.crt");
    let key_path = cert_dir.join("hotplate.key");

    // Reuse existing certs if they exist
    if cert_path.exists() && key_path.exists() {
        println!("  🔒 Reusing existing self-signed cert at .cert/");
        return Ok((cert_path, key_path));
    }

    // Create .cert directory
    std::fs::create_dir_all(&cert_dir)
        .with_context(|| format!("Failed to create directory: {}", cert_dir.display()))?;

    // Generate self-signed certificate with rcgen
    let mut params = rcgen::CertificateParams::new(vec![
        "localhost".to_string(),
    ])?;
    params.distinguished_name.push(
        rcgen::DnType::CommonName,
        rcgen::DnValue::Utf8String("Hotplate Dev Server".to_string()),
    );
    params.distinguished_name.push(
        rcgen::DnType::OrganizationName,
        rcgen::DnValue::Utf8String("Hotplate".to_string()),
    );

    // Add Subject Alternative Names for common dev scenarios
    params.subject_alt_names = vec![
        rcgen::SanType::DnsName("localhost".try_into()?),
        rcgen::SanType::IpAddress(std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1))),
        rcgen::SanType::IpAddress(std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)),
    ];

    // Add LAN IPs to SAN so mobile devices can connect without warnings
    if let Some(lan_ip) = get_lan_ip() {
        params.subject_alt_names.push(rcgen::SanType::IpAddress(lan_ip));
    }

    let key_pair = rcgen::KeyPair::generate()?;
    let cert = params.self_signed(&key_pair)?;

    // Write PEM files
    std::fs::write(&cert_path, cert.pem())
        .with_context(|| format!("Failed to write cert: {}", cert_path.display()))?;
    std::fs::write(&key_path, key_pair.serialize_pem())
        .with_context(|| format!("Failed to write key: {}", key_path.display()))?;

    println!("  🔒 Generated self-signed certificate:");
    println!("     📄 {}", cert_path.display());
    println!("     🔑 {}", key_path.display());

    Ok((cert_path, key_path))
}

/// Detect LAN IPv4 address (same logic used in server.rs banner)
fn get_lan_ip() -> Option<std::net::IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    socket.local_addr().ok().map(|a| a.ip())
}

fn build_config(cli: Cli) -> Result<Config> {
    let workspace = cli
        .workspace
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let vs = load_vscode_settings(&workspace);

    // Merge: CLI > .vscode/settings.json > defaults
    let host = cli.host;
    let port = cli.port;
    let live_reload = !cli.no_reload;
    let full_reload = cli.full_reload;

    // Root: CLI --root > vscode root > "."
    let root_str = cli.root.or_else(|| {
        vs.as_ref()
            .and_then(|s| s.root.as_ref())
            .map(|r| r.trim_start_matches('/').to_string())
    });
    let root = match root_str {
        Some(r) => resolve_path(&workspace, &r),
        None => workspace.clone(),
    };

    // HTTPS: CLI --cert/--key > vscode https > --https (auto-generate)
    let https_flag = cli.https;
    let (cert, key) = match (cli.cert, cli.key) {
        (Some(c), Some(k)) => (
            Some(resolve_path(&workspace, &c)),
            Some(resolve_path(&workspace, &k)),
        ),
        _ => {
            if let Some(https) = vs.as_ref().and_then(|s| s.https.as_ref()) {
                if https.enable.unwrap_or(false) {
                    let c = https
                        .cert
                        .as_ref()
                        .map(|p| resolve_path(&workspace, p));
                    let k = https
                        .key
                        .as_ref()
                        .map(|p| resolve_path(&workspace, p));
                    (c, k)
                } else if https_flag {
                    // --https flag without cert paths in settings → auto-generate
                    (None, None)
                } else {
                    (None, None)
                }
            } else {
                (None, None)
            }
        }
    };

    // Auto-generate self-signed cert when --https is used but no cert/key provided
    let (cert, key) = if cert.is_none() && key.is_none() && https_flag {
        let (c, k) = generate_self_signed_cert(&workspace)?;
        (Some(c), Some(k))
    } else {
        (cert, key)
    };

    // Validate cert/key files exist
    if let Some(ref c) = cert {
        anyhow::ensure!(c.exists(), "Certificate not found: {}", c.display());
    }
    if let Some(ref k) = key {
        anyhow::ensure!(k.exists(), "Private key not found: {}", k.display());
    }
    anyhow::ensure!(root.exists(), "Root directory not found: {}", root.display());

    let mounts = parse_mounts(&cli.mounts, &workspace);

    Ok(Config {
        host,
        port,
        root,
        cert,
        key,
        live_reload,
        full_reload,
        workspace,
        ignore_patterns: cli.ignore,
        spa_file: cli.file,
        proxy_base: cli.proxy_base,
        proxy_target: cli.proxy_target,
        headers: parse_headers(&cli.headers),
        mounts,
    })
}

/// Parse "Key: Value" strings into (key, value) tuples.
fn parse_headers(raw: &[String]) -> Vec<(String, String)> {
    raw.iter()
        .filter_map(|h| {
            let mut parts = h.splitn(2, ':');
            let key = parts.next()?.trim().to_string();
            let value = parts.next()?.trim().to_string();
            if key.is_empty() {
                None
            } else {
                Some((key, value))
            }
        })
        .collect()
}

/// Parse "/url_path:./fs_path" strings into (url_path, resolved_fs_path) tuples.
fn parse_mounts(raw: &[String], workspace: &Path) -> Vec<(String, PathBuf)> {
    raw.iter()
        .filter_map(|m| {
            let mut parts = m.splitn(2, ':');
            let url_path = parts.next()?.trim().to_string();
            let fs_path_str = parts.next()?.trim().to_string();
            if url_path.is_empty() || fs_path_str.is_empty() {
                eprintln!("  ⚠ Invalid mount: {}", m);
                return None;
            }
            // Ensure url_path starts with /
            let url_path = if url_path.starts_with('/') {
                url_path
            } else {
                format!("/{}", url_path)
            };
            let fs_path = resolve_path(workspace, &fs_path_str);
            if !fs_path.exists() {
                eprintln!("  ⚠ Mount path does not exist: {} → {}", url_path, fs_path.display());
                // Still allow it — directory might be created later
            }
            Some((url_path, fs_path))
        })
        .collect()
}

// ───────────────────── Main ─────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    // Install rustls crypto provider (ring) before any TLS usage
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    let cli = Cli::parse();
    let config = build_config(cli).context("Failed to load configuration")?;
    server::run(config).await
}

```

## File ./src\server.rs:
```rust
//! HTTP/HTTPS server with static files + WebSocket live reload + SPA fallback + proxy.

use crate::inject::inject_livereload;
use crate::watcher;
use crate::Config;

use anyhow::Result;
use axum::{
    body::Body,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Request, State,
    },
    http::{header, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::broadcast;
use tower_http::{
    cors::{Any, CorsLayer},
    services::{ServeDir, ServeFile},
};

/// Built-in welcome page shown when root directory has no index.html.
const WELCOME_HTML: &str = include_str!("welcome.html");

// ───────────────────── Shared state ─────────────────────

#[derive(Clone)]
#[allow(dead_code)]
pub struct AppState {
    pub reload_tx: broadcast::Sender<String>,
    pub live_reload: bool,
    pub full_reload: bool,
    pub proxy_base: Option<String>,
    pub proxy_target: Option<String>,
    pub http_client: reqwest::Client,
}

// ───────────────────── WebSocket handler ─────────────────────

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: Arc<AppState>) {
    let mut rx = state.reload_tx.subscribe();
    while let Ok(changed_path) = rx.recv().await {
        // Determine reload message:
        //   - If full_reload is true → always send "reload" (full page reload)
        //   - If the changed file is CSS → send "css:<path>" for CSS-only hot swap
        //   - Otherwise → send "reload"
        let msg = if !state.full_reload && is_css_file(&changed_path) {
            format!("css:{}", changed_path)
        } else {
            "reload".to_string()
        };

        if socket
            .send(Message::Text(msg))
            .await
            .is_err()
        {
            break;
        }
    }
}

/// Check if the file path is a CSS file.
fn is_css_file(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.ends_with(".css")
}

// ───────────────────── Build router ─────────────────────

fn build_router(state: Arc<AppState>, config: &Config) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let mut app = Router::new();

    // WebSocket endpoint for live reload
    if config.live_reload {
        app = app.route("/__lr", get(ws_handler));
    }

    // Proxy: forward /base/* to target server
    if let (Some(ref base), Some(_)) = (&config.proxy_base, &config.proxy_target) {
        let proxy_path = format!("{}/*rest", base.trim_end_matches('/'));
        // Also handle exact base path (no trailing subpath)
        let proxy_path_exact = base.trim_end_matches('/').to_string();
        app = app
            .route(&proxy_path, axum::routing::any(proxy_handler))
            .route(&proxy_path_exact, axum::routing::any(proxy_handler));
    }

    // Mount extra directories at specific URL paths
    // e.g. --mount "/node_modules:./node_modules" serves ./node_modules at /node_modules
    for (url_path, fs_path) in &config.mounts {
        let mount_service = ServeDir::new(fs_path).append_index_html_on_directories(true);
        // nest_service strips the prefix before passing to ServeDir
        app = app.nest_service(url_path, mount_service);
    }

    // Check if root directory has an index.html
    let has_index = config.root.join("index.html").exists();

    // Static file serving with optional SPA fallback
    if has_index || config.spa_file.is_some() {
        let serve_dir = if let Some(ref spa_file) = config.spa_file {
            ServeDir::new(&config.root)
                .append_index_html_on_directories(true)
                .fallback(ServeFile::new(config.root.join(spa_file)))
        } else {
            ServeDir::new(&config.root)
                .append_index_html_on_directories(true)
                .fallback(ServeFile::new(config.root.join("404.html")))
        };
        app = app.fallback_service(serve_dir);
    } else {
        // No index.html in root — serve static files normally but show welcome page on "/"
        let serve_dir = ServeDir::new(&config.root)
            .append_index_html_on_directories(true);
        app = app
            .route("/", get(welcome_handler))
            .fallback_service(serve_dir);
    }

    // Middleware stack (applied bottom-up)
    if config.live_reload {
        app = app.layer(middleware::from_fn(inject_livereload));
    }

    // Custom response headers
    if !config.headers.is_empty() {
        let headers_vec: Vec<(axum::http::HeaderName, axum::http::HeaderValue)> = config
            .headers
            .iter()
            .filter_map(|(key, value)| {
                match (
                    axum::http::HeaderName::from_bytes(key.as_bytes()),
                    axum::http::HeaderValue::from_str(value),
                ) {
                    (Ok(name), Ok(val)) => Some((name, val)),
                    _ => {
                        eprintln!("  ⚠ Invalid header: {}: {}", key, value);
                        None
                    }
                }
            })
            .collect();

        if !headers_vec.is_empty() {
            app = app.layer(middleware::from_fn(move |req: Request<Body>, next: Next| {
                let hdrs = headers_vec.clone();
                async move {
                    let mut resp = next.run(req).await;
                    for (name, value) in hdrs {
                        resp.headers_mut().insert(name, value);
                    }
                    resp
                }
            }));
        }
    }

    app.layer(cors).with_state(state)
}

// ───────────────────── Welcome page handler ─────────────────────

/// Serve the built-in welcome page (when root has no index.html).
async fn welcome_handler() -> impl IntoResponse {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(WELCOME_HTML))
        .unwrap()
}

// ───────────────────── Proxy handler ─────────────────────

/// Forward requests to the configured proxy target.
/// Preserves method, headers, query string, and body.
async fn proxy_handler(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> Response<Body> {
    let (Some(ref base), Some(ref target)) = (&state.proxy_base, &state.proxy_target) else {
        return (StatusCode::BAD_GATEWAY, "Proxy not configured").into_response();
    };

    // Build target URL: strip proxy base, keep remaining path + query
    let uri = req.uri().clone();
    let path = uri.path();
    let remaining = path
        .strip_prefix(base.trim_end_matches('/'))
        .unwrap_or(path);
    let remaining = if remaining.is_empty() { "/" } else { remaining };

    let target_url = if let Some(query) = uri.query() {
        format!("{}{}?{}", target.trim_end_matches('/'), remaining, query)
    } else {
        format!("{}{}", target.trim_end_matches('/'), remaining)
    };

    // Forward the request
    let method = req.method().clone();
    let mut builder = state.http_client.request(
        reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET),
        &target_url,
    );

    // Copy headers (skip host — reqwest sets it automatically)
    for (key, value) in req.headers() {
        if key != header::HOST {
            if let Ok(v) = value.to_str() {
                builder = builder.header(key.as_str(), v);
            }
        }
    }

    // Copy body for non-GET/HEAD methods
    if method != Method::GET && method != Method::HEAD {
        let body_bytes = match axum::body::to_bytes(req.into_body(), 10 * 1024 * 1024).await {
            Ok(b) => b,
            Err(_) => return (StatusCode::BAD_REQUEST, "Failed to read request body").into_response(),
        };
        builder = builder.body(body_bytes);
    }

    // Execute proxied request
    match builder.send().await {
        Ok(proxy_resp) => {
            let status = StatusCode::from_u16(proxy_resp.status().as_u16())
                .unwrap_or(StatusCode::BAD_GATEWAY);
            let mut response = Response::builder().status(status);

            // Copy response headers
            for (key, value) in proxy_resp.headers() {
                response = response.header(key.as_str(), value.as_bytes());
            }

            match proxy_resp.bytes().await {
                Ok(body) => response.body(Body::from(body)).unwrap_or_else(|_| {
                    (StatusCode::BAD_GATEWAY, "Failed to build response").into_response()
                }),
                Err(_) => (StatusCode::BAD_GATEWAY, "Failed to read proxy response").into_response(),
            }
        }
        Err(e) => {
            eprintln!("  ⚠ Proxy error: {}", e);
            (StatusCode::BAD_GATEWAY, format!("Proxy error: {}", e)).into_response()
        }
    }
}

// ───────────────────── Startup banner ─────────────────────

fn print_banner(config: &Config) {
    let scheme = if config.cert.is_some() {
        "https"
    } else {
        "http"
    };

    println!();
    println!("  🔥 hotplate v{}", env!("CARGO_PKG_VERSION"));
    println!("  ─────────────────────────────────────");
    println!("  📂 Root:    {}", config.root.display());
    println!("  🔗 Local:   {}://localhost:{}", scheme, config.port);
    if config.host == "0.0.0.0" {
        // Show LAN addresses
        if let Ok(addrs) = local_ip_addresses() {
            for addr in addrs {
                println!("  🌐 Network: {}://{}:{}", scheme, addr, config.port);
            }
        }
    }
    if config.cert.is_some() {
        println!("  🔒 HTTPS:   enabled");
    }
    let reload_mode = if !config.live_reload {
        "OFF"
    } else if config.full_reload {
        "ON (full page)"
    } else {
        "ON (CSS hot swap)"
    };
    println!("  🔄 Reload:  {}", reload_mode);
    if let (Some(ref base), Some(ref target)) = (&config.proxy_base, &config.proxy_target) {
        println!("  🔀 Proxy:   {} → {}", base, target);
    }
    if !config.mounts.is_empty() {
        for (url_path, fs_path) in &config.mounts {
            println!("  📁 Mount:   {} → {}", url_path, fs_path.display());
        }
    }
    if let Some(ref file) = config.spa_file {
        println!("  📄 SPA:     {} (fallback)", file);
    }
    println!("  ─────────────────────────────────────");
    println!();
}

fn local_ip_addresses() -> Result<Vec<String>> {
    let mut addrs = Vec::new();
    let socket = std::net::UdpSocket::bind("0.0.0.0:0")?;
    // Connect to a public address to determine local IP
    socket.connect("8.8.8.8:80")?;
    if let Ok(local) = socket.local_addr() {
        addrs.push(local.ip().to_string());
    }
    Ok(addrs)
}

// ───────────────────── Run server ─────────────────────

/// Maximum number of port increments to try when the port is already in use.
const MAX_PORT_RETRIES: u16 = 20;

pub async fn run(mut config: Config) -> Result<()> {
    let (reload_tx, _) = broadcast::channel::<String>(16);

    // HTTP client for proxy (reusable connection pool)
    let http_client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true) // dev mode — proxy targets may use self-signed
        .build()
        .unwrap_or_default();

    let state = Arc::new(AppState {
        reload_tx: reload_tx.clone(),
        live_reload: config.live_reload,
        full_reload: config.full_reload,
        proxy_base: config.proxy_base.clone(),
        proxy_target: config.proxy_target.clone(),
        http_client,
    });

    // Start file watcher
    if config.live_reload {
        watcher::spawn(config.root.clone(), reload_tx, &config.ignore_patterns)?;
    }

    let app = build_router(state, &config);

    // Bind HTTP or HTTPS — with auto port increment on AddrInUse
    match (&config.cert, &config.key) {
        (Some(cert), Some(key)) => {
            let tls_config =
                axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key).await?;

            let original_port = config.port;
            let mut bound = None;

            for attempt in 0..=MAX_PORT_RETRIES {
                let try_port = config.port + attempt;
                if try_port < config.port {
                    break; // overflow guard
                }
                let addr: SocketAddr = format!("{}:{}", config.host, try_port).parse()?;

                // Quick check: can we bind this port?
                match std::net::TcpListener::bind(addr) {
                    Ok(probe) => {
                        drop(probe); // release immediately so axum_server can bind
                        config.port = try_port;
                        bound = Some(addr);
                        break;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                        if attempt == 0 {
                            eprintln!(
                                "  ⚠ Port {} is in use, searching for an available port...",
                                try_port
                            );
                        }
                        continue;
                    }
                    Err(e) => return Err(e.into()),
                }
            }

            let addr = bound.ok_or_else(|| {
                anyhow::anyhow!(
                    "Ports {}-{} are all in use. Please free a port or choose a different one.",
                    original_port,
                    original_port + MAX_PORT_RETRIES
                )
            })?;

            if config.port != original_port {
                println!(
                    "  ℹ Port {} was in use, switched to port {}.",
                    original_port, config.port
                );
            }

            print_banner(&config);
            println!(
                "  🚀 Listening on https://{}:{} ...",
                config.host, config.port
            );
            axum_server::bind_rustls(addr, tls_config)
                .serve(app.into_make_service())
                .await?;
        }
        _ => {
            let original_port = config.port;
            let mut listener_result = None;

            for attempt in 0..=MAX_PORT_RETRIES {
                let try_port = config.port + attempt;
                if try_port < config.port {
                    break; // overflow guard
                }
                let addr: SocketAddr = format!("{}:{}", config.host, try_port).parse()?;

                match tokio::net::TcpListener::bind(addr).await {
                    Ok(l) => {
                        config.port = try_port;
                        listener_result = Some(l);
                        break;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                        if attempt == 0 {
                            eprintln!(
                                "  ⚠ Port {} is in use, searching for an available port...",
                                try_port
                            );
                        }
                        continue;
                    }
                    Err(e) => return Err(e.into()),
                }
            }

            let listener = listener_result.ok_or_else(|| {
                anyhow::anyhow!(
                    "Ports {}-{} are all in use. Please free a port or choose a different one.",
                    original_port,
                    original_port + MAX_PORT_RETRIES
                )
            })?;

            if config.port != original_port {
                println!(
                    "  ℹ Port {} was in use, switched to port {}.",
                    original_port, config.port
                );
            }

            print_banner(&config);
            println!(
                "  🚀 Listening on http://{}:{} ...",
                config.host, config.port
            );
            axum::serve(listener, app).await?;
        }
    }

    Ok(())
}

```

## File ./src\watcher.rs:
```rust
//! File system watcher — debounced, filtered, broadcasts reload events.

use anyhow::Result;
use globset::{Glob, GlobSet, GlobSetBuilder};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// Directories/files to always ignore.
const IGNORE_DIRS: &[&str] = &[".git", "node_modules", "target", "__pycache__", ".venv"];
const IGNORE_EXTS: &[&str] = &["pyc", "pyo", "swp", "swo", "tmp"];

/// Build a GlobSet from user-provided patterns (e.g. ["**/*.scss", ".vscode/**"]).
fn build_ignore_globs(patterns: &[String]) -> Option<GlobSet> {
    if patterns.is_empty() {
        return None;
    }
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        match Glob::new(pattern) {
            Ok(g) => { builder.add(g); }
            Err(e) => eprintln!("  ⚠ Invalid ignore pattern '{}': {}", pattern, e),
        }
    }
    builder.build().ok()
}

fn should_ignore(paths: &[PathBuf], root: &PathBuf, user_globs: &Option<GlobSet>) -> bool {
    paths.iter().all(|p| {
        let s = p.to_string_lossy();
        // Ignored directories
        if IGNORE_DIRS.iter().any(|d| s.contains(d)) {
            return true;
        }
        // Ignored extensions
        if let Some(ext) = p.extension() {
            if IGNORE_EXTS.contains(&ext.to_string_lossy().as_ref()) {
                return true;
            }
        }
        // User-provided glob patterns (matched against relative path)
        if let Some(ref globs) = user_globs {
            let rel = p.strip_prefix(root).unwrap_or(p);
            // Normalize to forward slashes for glob matching (Windows uses backslash)
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            if globs.is_match(&rel_str) {
                return true;
            }
        }
        false
    })
}

fn is_relevant_event(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
    )
}

/// Spawn a file watcher on a background thread.
/// Sends the relative path of changed files to `reload_tx` (debounced 150ms).
/// `ignore_patterns` are user-provided glob patterns to skip (e.g. "**/*.scss").
pub fn spawn(
    root: PathBuf,
    reload_tx: broadcast::Sender<String>,
    ignore_patterns: &[String],
) -> Result<()> {
    let (tx, rx) = std::sync::mpsc::channel::<Result<Event, notify::Error>>();

    let mut watcher = RecommendedWatcher::new(
        move |res| {
            let _ = tx.send(res);
        },
        notify::Config::default(),
    )?;

    watcher.watch(&root, RecursiveMode::Recursive)?;

    let user_globs = build_ignore_globs(ignore_patterns);
    let watch_root = root.clone();

    // Dedicated OS thread — never blocks tokio
    std::thread::Builder::new()
        .name("fs-watcher".into())
        .spawn(move || {
            let _watcher = watcher; // prevent drop
            let mut last_reload = Instant::now();
            let debounce = Duration::from_millis(150);

            for event in rx {
                let Ok(event) = event else { continue };

                if !is_relevant_event(&event.kind) {
                    continue;
                }
                if should_ignore(&event.paths, &watch_root, &user_globs) {
                    continue;
                }
                if last_reload.elapsed() < debounce {
                    continue;
                }

                last_reload = Instant::now();

                // Log changed file and send its relative path
                let rel_path = event.paths.first()
                    .map(|p| {
                        let rel = p.strip_prefix(&watch_root).unwrap_or(p);
                        let display = rel.display();
                        println!("  ↻ {}", display);
                        // Normalize to forward slashes
                        rel.to_string_lossy().replace('\\', "/")
                    })
                    .unwrap_or_default();

                let _ = reload_tx.send(rel_path);
            }
        })?;

    Ok(())
}

```

## File ./vscode-extension\extension.js:
```javascript
// ⚡ Hotplate — VS Code Extension
// Thin JS wrapper that spawns the Rust binary and manages its lifecycle.
//
// Architecture:
//   extension.js (this file) → spawn → hotplate.exe (Rust binary)
//   All heavy lifting (HTTP, HTTPS, WebSocket, file watching) is in Rust.
//   This file only does: spawn, kill, pipe output, status bar, config.

const vscode = require('vscode');
const { spawn } = require('child_process');
const path = require('path');
const os = require('os');

/** @type {import('child_process').ChildProcess | null} */
let serverProcess = null;

/** @type {vscode.StatusBarItem} */
let statusBar;

/** @type {vscode.OutputChannel} */
let outputChannel;

/** @type {string | null} */
let serverUrl = null;

/** @type {string | null} File path to open in browser after server starts */
let pendingOpenFile = null;

/** @type {string | null} Track which workspace the running server belongs to */
let previousWorkspacePath = null;

// ────────────────────── Multi-root Workspace Resolution ──────────────────────

/**
 * Show a quick picker to let the user choose a workspace folder.
 * Saves the choice to `hotplate.multiRootWorkspaceName`.
 * @returns {Promise<string | undefined>} Chosen workspace name
 */
async function pickWorkspaceFolder() {
    const folders = vscode.workspace.workspaceFolders;
    if (!folders || !folders.length) return undefined;

    const names = folders.map(f => f.name);
    const chosen = await vscode.window.showQuickPick(names, {
        placeHolder: 'Choose workspace folder for Hotplate',
        ignoreFocusOut: true,
    });

    if (chosen) {
        await vscode.workspace.getConfiguration('hotplate').update(
            'multiRootWorkspaceName', chosen, false
        );
    }
    return chosen;
}

/**
 * Resolve the workspace folder path for starting the server.
 *
 * Resolution order:
 *   1. If only one workspace folder → use it directly
 *   2. If a file URI is provided (right-click) → detect its workspace folder
 *   3. If `hotplate.multiRootWorkspaceName` is set → use that folder
 *   4. Otherwise → show quick picker
 *
 * @param {string} [fileUri] - Absolute file path from context menu
 * @returns {Promise<string | null>} Workspace folder path, or null if cancelled
 */
async function resolveWorkspaceFolder(fileUri) {
    const folders = vscode.workspace.workspaceFolders;
    if (!folders || !folders.length) {
        vscode.window.showErrorMessage('Open a folder or workspace first. (File → Open Folder)');
        return null;
    }

    // 1. Single workspace — no ambiguity
    if (folders.length === 1) {
        return folders[0].uri.fsPath;
    }

    // 2. If a file/folder URI is given, detect its workspace
    if (fileUri) {
        const matched = folders.find(f => fileUri.startsWith(f.uri.fsPath));
        if (matched) {
            await vscode.workspace.getConfiguration('hotplate').update(
                'multiRootWorkspaceName', matched.name, false
            );
            return matched.uri.fsPath;
        }
    }

    // 3. Check saved preference
    const config = vscode.workspace.getConfiguration('hotplate');
    const savedName = config.get('multiRootWorkspaceName', null);
    if (savedName) {
        const target = folders.find(f => f.name === savedName);
        if (target) return target.uri.fsPath;
        // Saved name is stale — clear it
        await config.update('multiRootWorkspaceName', null, false);
    }

    // 4. Show picker
    const chosen = await pickWorkspaceFolder();
    if (!chosen) return null; // user cancelled
    const folder = folders.find(f => f.name === chosen);
    return folder ? folder.uri.fsPath : null;
}

// ────────────────────── Binary Resolution ──────────────────────

/**
 * Resolve the path to the hotplate binary.
 * Search order:
 *   1. bin/ folder in extension directory (packaged extension)
 *   2. Workspace hotplate/target/release/ (dev mode)
 *   3. System PATH
 */
function getBinaryPath(context) {
    const platform = os.platform();   // win32, linux, darwin
    const arch = os.arch();           // x64, arm64
    const ext = platform === 'win32' ? '.exe' : '';

    // 1. Bundled binary (published extension)
    const bundled = path.join(context.extensionPath, 'bin', `hotplate-${platform}-${arch}${ext}`);
    if (require('fs').existsSync(bundled)) {
        return bundled;
    }

    // 2. Platform-generic bundled binary (single-platform dev build)
    const bundledSimple = path.join(context.extensionPath, 'bin', `hotplate${ext}`);
    if (require('fs').existsSync(bundledSimple)) {
        return bundledSimple;
    }

    // 3. Workspace dev build (hotplate/target/release/)
    const workspaceFolder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    if (workspaceFolder) {
        const devBuild = path.join(workspaceFolder, 'hotplate', 'target', 'release', `hotplate${ext}`);
        if (require('fs').existsSync(devBuild)) {
            return devBuild;
        }
    }

    // 4. Fall back to PATH
    return `hotplate${ext}`;
}

// ────────────────────── Server Lifecycle ──────────────────────

/**
 * Build CLI arguments from VS Code configuration.
 * @param {string} workspacePath
 * @param {string} [folderRoot] - Explicit root from context menu
 * @returns {string[]}
 */
function buildArgs(workspacePath, folderRoot) {
    const config = vscode.workspace.getConfiguration('hotplate');

    const args = [
        '--workspace', workspacePath,
        '--host', config.get('host', '0.0.0.0'),
        '--port', String(config.get('port', 5500)),
    ];

    // Root directory
    const root = folderRoot || config.get('root', '');
    if (root) {
        args.push('--root', root);
    }

    // HTTPS
    const httpsEnable = config.get('https.enable', false);
    const cert = config.get('https.cert', '');
    const key = config.get('https.key', '');
    if (cert && key) {
        args.push('--cert', cert, '--key', key);
    } else if (httpsEnable) {
        // Auto-generate self-signed cert via --https flag
        args.push('--https');
    }

    // Live reload
    if (!config.get('liveReload', true)) {
        args.push('--no-reload');
    }

    // Full reload (disable CSS-only hot swap)
    if (config.get('fullReload', false)) {
        args.push('--full-reload');
    }

    // Ignore patterns
    const ignoreFiles = config.get('ignoreFiles', []);
    for (const pattern of ignoreFiles) {
        args.push('--ignore', pattern);
    }

    // SPA fallback file
    const spaFile = config.get('file', '');
    if (spaFile) {
        args.push('--file', spaFile);
    }

    // Proxy
    const proxy = config.get('proxy', {});
    if (proxy && proxy.enable && proxy.baseUri && proxy.proxyUri) {
        args.push('--proxy-base', proxy.baseUri, '--proxy-target', proxy.proxyUri);
    }

    // Custom headers
    const headers = config.get('headers', {});
    if (headers && typeof headers === 'object') {
        for (const [key, value] of Object.entries(headers)) {
            args.push('--header', `${key}: ${value}`);
        }
    }

    // Mount directories
    const mounts = config.get('mount', []);
    if (Array.isArray(mounts)) {
        for (const entry of mounts) {
            if (Array.isArray(entry) && entry.length === 2) {
                const [urlPath, fsPath] = entry;
                args.push('--mount', `${urlPath}:${fsPath}`);
            }
        }
    }

    return args;
}

/**
 * Start the hotplate server.
 * @param {vscode.ExtensionContext} context
 * @param {string} [folderRoot] - Root folder override (from context menu)
 * @param {string} [openFilePath] - File path to open in browser after server starts
 * @param {string} [fileUri] - Absolute file path (used for workspace resolution in multi-root)
 */
async function startServer(context, folderRoot, openFilePath, fileUri) {
    if (serverProcess) {
        vscode.window.showWarningMessage('Hotplate is already running. Stop it first.');
        return;
    }

    // Save all open files before starting (like Live Server does)
    await vscode.workspace.saveAll();

    const workspaceFolder = await resolveWorkspaceFolder(fileUri);
    if (!workspaceFolder) {
        return; // user cancelled or no workspace
    }

    // Guard: server was already started from a different workspace
    if (previousWorkspacePath && previousWorkspacePath !== workspaceFolder) {
        vscode.window.showErrorMessage(
            'Hotplate is already configured for a different workspace. Stop the server first.'
        );
        return;
    }
    previousWorkspacePath = workspaceFolder;

    const binaryPath = getBinaryPath(context);
    const args = buildArgs(workspaceFolder, folderRoot);
    const config = vscode.workspace.getConfiguration('hotplate');
    const port = config.get('port', 5500);
    const httpsEnabled = config.get('https.enable', false) || config.get('https.cert', '');
    const scheme = httpsEnabled ? 'https' : 'http';

    outputChannel.clear();
    outputChannel.show(true);
    outputChannel.appendLine(`[Hotplate] Starting server...`);
    outputChannel.appendLine(`[Hotplate] Binary: ${binaryPath}`);
    outputChannel.appendLine(`[Hotplate] Args: ${args.join(' ')}`);
    outputChannel.appendLine('');

    try {
        serverProcess = spawn(binaryPath, args, {
            cwd: workspaceFolder,
            env: { ...process.env },
        });
    } catch (err) {
        vscode.window.showErrorMessage(`Failed to start Hotplate: ${err.message}`);
        outputChannel.appendLine(`[Hotplate] ERROR: ${err.message}`);
        return;
    }

    serverUrl = `${scheme}://localhost:${port}`;
    pendingOpenFile = openFilePath || null;

    /** @type {number} Track the actual port the server bound to */
    let actualPort = port;

    // Pipe stdout
    serverProcess.stdout?.on('data', (data) => {
        const text = data.toString();
        outputChannel.append(text);

        // Detect port change (auto port increment)
        // Rust binary prints: "  ℹ Port 5500 was in use, switched to port 5501."
        const portSwitchMatch = text.match(/switched to port (\d+)/);
        if (portSwitchMatch) {
            const newPort = parseInt(portSwitchMatch[1], 10);
            if (newPort) {
                actualPort = newPort;
                serverUrl = `${scheme}://localhost:${newPort}`;
                updateStatusBar(true, newPort);
                vscode.window.showWarningMessage(
                    `Port ${port} was in use. Hotplate switched to port ${newPort}.`
                );
            }
        }

        // Detect "Listening on" line to auto-open browser
        if (text.includes('Listening on')) {
            // Parse the real port from the "Listening on" line as a final source of truth
            // Format: "  🚀 Listening on http://0.0.0.0:5501 ..."
            const listenMatch = text.match(/Listening on \w+:\/\/[^:]+:(\d+)/);
            if (listenMatch) {
                const listenPort = parseInt(listenMatch[1], 10);
                if (listenPort && listenPort !== actualPort) {
                    actualPort = listenPort;
                    serverUrl = `${scheme}://localhost:${listenPort}`;
                    updateStatusBar(true, listenPort);
                }
            }

            // Build the URL — if a specific file was requested, append its path
            let url = serverUrl;
            if (pendingOpenFile) {
                url = `${serverUrl}/${pendingOpenFile}`;
                pendingOpenFile = null;
            }

            if (config.get('openBrowser', true) || openFilePath) {
                const open = require('child_process');
                if (os.platform() === 'win32') {
                    open.exec(`start "" "${url}"`);
                } else if (os.platform() === 'darwin') {
                    open.exec(`open "${url}"`);
                } else {
                    open.exec(`xdg-open "${url}"`);
                }
            }
        }
    });

    // Pipe stderr
    serverProcess.stderr?.on('data', (data) => {
        outputChannel.append(data.toString());
    });

    // Handle process exit
    serverProcess.on('close', (code) => {
        outputChannel.appendLine(`\n[Hotplate] Server stopped (exit code: ${code})`);
        serverProcess = null;
        serverUrl = null;
        updateStatusBar(false);
        vscode.commands.executeCommand('setContext', 'hotplate:running', false);
    });

    serverProcess.on('error', (err) => {
        vscode.window.showErrorMessage(`Hotplate error: ${err.message}`);
        outputChannel.appendLine(`[Hotplate] ERROR: ${err.message}`);
        serverProcess = null;
        serverUrl = null;
        updateStatusBar(false);
        vscode.commands.executeCommand('setContext', 'hotplate:running', false);
    });

    // Update UI
    updateStatusBar(true, port);
    vscode.commands.executeCommand('setContext', 'hotplate:running', true);
    vscode.window.showInformationMessage(`🔥 Hotplate started on port ${port}`);
}

/**
 * Stop the hotplate server.
 */
function stopServer() {
    if (!serverProcess) {
        vscode.window.showInformationMessage('Hotplate is not running.');
        return;
    }

    outputChannel.appendLine('\n[Hotplate] Stopping server...');

    // On Windows, use taskkill for clean shutdown of child processes
    if (os.platform() === 'win32') {
        spawn('taskkill', ['/pid', String(serverProcess.pid), '/f', '/t']);
    } else {
        serverProcess.kill('SIGTERM');
    }

    serverProcess = null;
    serverUrl = null;
    previousWorkspacePath = null;
    updateStatusBar(false);
    vscode.commands.executeCommand('setContext', 'hotplate:running', false);
    vscode.window.showInformationMessage('Hotplate stopped.');
}

/**
 * Restart the hotplate server.
 * @param {vscode.ExtensionContext} context
 */
function restartServer(context) {
    if (serverProcess) {
        // Stop first, then start after a small delay
        if (os.platform() === 'win32') {
            spawn('taskkill', ['/pid', String(serverProcess.pid), '/f', '/t']);
        } else {
            serverProcess.kill('SIGTERM');
        }
        serverProcess = null;
        serverUrl = null;
        previousWorkspacePath = null;
    }

    // Small delay to ensure port is released
    setTimeout(() => startServer(context), 500);
}

// ────────────────────── Status Bar ──────────────────────

/**
 * Update the status bar item.
 * @param {boolean} running
 * @param {number} [port]
 */
function updateStatusBar(running, port) {
    if (running) {
        statusBar.text = `$(flame) Port: ${port}`;
        statusBar.tooltip = `Hotplate running on port ${port} — click to stop`;
        statusBar.command = 'hotplate.stop';
        statusBar.backgroundColor = new vscode.ThemeColor('statusBarItem.warningBackground');
    } else {
        statusBar.text = '$(flame) Go Live';
        statusBar.tooltip = 'Click to start Hotplate dev server';
        statusBar.command = 'hotplate.start';
        statusBar.backgroundColor = undefined;
    }
}

// ────────────────────── Activation ──────────────────────

/**
 * @param {vscode.ExtensionContext} context
 */
function activate(context) {
    // Create output channel
    outputChannel = vscode.window.createOutputChannel('Hotplate');

    // Create status bar
    statusBar = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Right, 100);
    updateStatusBar(false);

    // Respect showOnStatusbar setting
    const showOnStatusbar = vscode.workspace.getConfiguration('hotplate').get('showOnStatusbar', true);
    if (showOnStatusbar) {
        statusBar.show();
    }

    // Watch for config changes to show/hide status bar
    context.subscriptions.push(
        vscode.workspace.onDidChangeConfiguration((e) => {
            if (e.affectsConfiguration('hotplate.showOnStatusbar')) {
                const show = vscode.workspace.getConfiguration('hotplate').get('showOnStatusbar', true);
                if (show) statusBar.show();
                else statusBar.hide();
            }
        })
    );

    // Register commands
    context.subscriptions.push(
        // Start
        vscode.commands.registerCommand('hotplate.start', async (uri) => {
            // If called from explorer context menu, use the folder path
            let folderRoot;
            const fileUri = uri?.fsPath || undefined;
            if (uri && uri.fsPath) {
                const workspaceFolder = await resolveWorkspaceFolder(uri.fsPath);
                if (workspaceFolder) {
                    // Make relative to workspace
                    folderRoot = path.relative(workspaceFolder, uri.fsPath);
                }
            }
            await startServer(context, folderRoot, undefined, fileUri);
        }),

        // Stop
        vscode.commands.registerCommand('hotplate.stop', () => {
            stopServer();
        }),

        // Restart
        vscode.commands.registerCommand('hotplate.restart', () => {
            restartServer(context);
        }),

        // Open in browser
        vscode.commands.registerCommand('hotplate.openBrowser', () => {
            if (serverUrl) {
                vscode.env.openExternal(vscode.Uri.parse(serverUrl));
            } else {
                vscode.window.showWarningMessage('Hotplate is not running.');
            }
        }),

        // Open with Hotplate (right-click on HTML/XML file)
        vscode.commands.registerCommand('hotplate.openFile', async (uri) => {
            await vscode.workspace.saveAll();

            // Get the file path — from context menu URI or active editor
            let filePath;
            if (uri && uri.fsPath) {
                filePath = uri.fsPath;
            } else if (vscode.window.activeTextEditor) {
                filePath = vscode.window.activeTextEditor.document.uri.fsPath;
            }

            if (!filePath) {
                vscode.window.showWarningMessage('No file selected.');
                return;
            }

            const workspaceFolder = await resolveWorkspaceFolder(filePath);
            if (!workspaceFolder) {
                return;
            }

            // Calculate relative path from workspace root (considering configured root)
            const config = vscode.workspace.getConfiguration('hotplate');
            const configRoot = config.get('root', '');
            const serveRoot = configRoot
                ? path.join(workspaceFolder, configRoot)
                : workspaceFolder;
            const relativePath = path.relative(serveRoot, filePath).replace(/\\/g, '/');

            if (serverProcess) {
                // Server already running — just open the file URL
                const url = `${serverUrl}/${relativePath}`;
                const open = require('child_process');
                if (os.platform() === 'win32') {
                    open.exec(`start "" "${url}"`);
                } else if (os.platform() === 'darwin') {
                    open.exec(`open "${url}"`);
                } else {
                    open.exec(`xdg-open "${url}"`);
                }
            } else {
                // Start server then open the file
                startServer(context, undefined, relativePath, filePath);
            }
        }),

        // Change Workspace (multi-root)
        vscode.commands.registerCommand('hotplate.changeWorkspace', async () => {
            const chosen = await pickWorkspaceFolder();
            if (chosen) {
                vscode.window.showInformationMessage(
                    `Hotplate workspace set to '${chosen}'.`
                );
                // If server is running, stop it so the user can restart with the new workspace
                if (serverProcess) {
                    stopServer();
                    vscode.window.showInformationMessage(
                        'Server stopped. Start again to use the new workspace.'
                    );
                }
            }
        }),

        // Cleanup
        statusBar,
        outputChannel,
    );

    // Set initial context
    vscode.commands.executeCommand('setContext', 'hotplate:running', false);
}

/**
 * Cleanup on deactivation.
 */
function deactivate() {
    if (serverProcess) {
        if (os.platform() === 'win32') {
            spawn('taskkill', ['/pid', String(serverProcess.pid), '/f', '/t']);
        } else {
            serverProcess.kill('SIGTERM');
        }
        serverProcess = null;
    }
}

module.exports = { activate, deactivate };

```

## File ./vscode-extension\package.json:
```json
{
  "name": "hotplate",
  "displayName": "Hotplate — Live Server",
  "description": "⚡ Fast HTTPS live-reload dev server powered by Rust. Hot reload, zero config, LAN access with QR code.",
  "version": "0.1.1",
  "publisher": "maithanhduyan",
  "author": {
    "name": "Mai Thành Duy An",
    "email": "tiachop0102@gmail.com",
    "url": "https://x.com/maithanhduyan"
  },
  "license": "MIT",
  "repository": {
    "type": "git",
    "url": "https://github.com/maithanhduyan/hotplate"
  },
  "engines": {
    "vscode": "^1.85.0"
  },
  "icon": "images/icon.png",
  "categories": [
    "Other",
    "Testing",
    "Themes"
  ],
  "keywords": [
    "live server",
    "live reload",
    "https",
    "hot reload",
    "static server",
    "dev server",
    "rust"
  ],
  "activationEvents": [
    "onStartupFinished"
  ],
  "main": "./extension.js",
  "contributes": {
    "commands": [
      {
        "command": "hotplate.start",
        "title": "Start Server",
        "category": "Hotplate",
        "icon": "$(flame)"
      },
      {
        "command": "hotplate.stop",
        "title": "Stop Server",
        "category": "Hotplate",
        "icon": "$(debug-stop)"
      },
      {
        "command": "hotplate.restart",
        "title": "Restart Server",
        "category": "Hotplate",
        "icon": "$(debug-restart)"
      },
      {
        "command": "hotplate.openBrowser",
        "title": "Open in Browser",
        "category": "Hotplate",
        "icon": "$(globe)"
      },
      {
        "command": "hotplate.openFile",
        "title": "Open with Hotplate",
        "category": "Hotplate",
        "icon": "$(flame)"
      },
      {
        "command": "hotplate.changeWorkspace",
        "title": "Change Workspace",
        "category": "Hotplate",
        "icon": "$(folder-opened)"
      }
    ],
    "menus": {
      "editor/context": [
        {
          "command": "hotplate.openFile",
          "group": "hotplate@1",
          "when": "resourceLangId == html"
        },
        {
          "command": "hotplate.openFile",
          "group": "hotplate@1",
          "when": "resourceLangId == xml"
        },
        {
          "command": "hotplate.stop",
          "group": "hotplate@2",
          "when": "hotplate:running"
        }
      ],
      "explorer/context": [
        {
          "command": "hotplate.openFile",
          "group": "navigation@-Hotplate",
          "when": "resourceLangId == html"
        },
        {
          "command": "hotplate.openFile",
          "group": "navigation@-Hotplate",
          "when": "resourceLangId == xml"
        },
        {
          "command": "hotplate.start",
          "when": "explorerResourceIsFolder",
          "group": "hotplate@1"
        }
      ],
      "commandPalette": [
        {
          "command": "hotplate.changeWorkspace",
          "when": "workspaceFolderCount > 1"
        }
      ],
      "editor/title": [
        {
          "command": "hotplate.openBrowser",
          "when": "hotplate:running",
          "group": "navigation"
        }
      ]
    },
    "configuration": {
      "title": "Hotplate",
      "properties": {
        "hotplate.port": {
          "type": "number",
          "default": 5500,
          "description": "Port number for the dev server."
        },
        "hotplate.root": {
          "type": "string",
          "default": "",
          "description": "Root directory to serve (relative to workspace). Empty = workspace root."
        },
        "hotplate.host": {
          "type": "string",
          "default": "0.0.0.0",
          "description": "Bind host. Use 0.0.0.0 for LAN access, 127.0.0.1 for localhost only."
        },
        "hotplate.https.enable": {
          "type": "boolean",
          "default": false,
          "description": "Enable HTTPS. If cert/key are not provided, a self-signed certificate will be auto-generated in .cert/ directory."
        },
        "hotplate.https.cert": {
          "type": "string",
          "default": "",
          "description": "Path to TLS certificate file (PEM). Relative to workspace. Leave empty for auto-generated self-signed cert."
        },
        "hotplate.https.key": {
          "type": "string",
          "default": "",
          "description": "Path to TLS private key file (PEM). Relative to workspace. Leave empty for auto-generated self-signed cert."
        },
        "hotplate.liveReload": {
          "type": "boolean",
          "default": true,
          "description": "Enable live reload on file changes."
        },
        "hotplate.fullReload": {
          "type": "boolean",
          "default": false,
          "description": "When false (default), CSS changes are injected without a full page reload. Set to true to always do a full page reload on any file change."
        },
        "hotplate.openBrowser": {
          "type": "boolean",
          "default": true,
          "description": "Automatically open browser when server starts."
        },
        "hotplate.showOnStatusbar": {
          "type": "boolean",
          "default": true,
          "description": "Show the Go Live button in the status bar."
        },
        "hotplate.ignoreFiles": {
          "type": "array",
          "default": [
            ".vscode/**",
            "**/*.scss",
            "**/*.sass",
            "**/*.ts"
          ],
          "description": "Glob patterns for files to ignore during live reload."
        },
        "hotplate.file": {
          "type": "string",
          "default": "",
          "description": "Serve this file for every 404 (useful for single-page applications)."
        },
        "hotplate.proxy": {
          "type": "object",
          "default": {
            "enable": false,
            "baseUri": "/api",
            "proxyUri": "http://127.0.0.1:8000"
          },
          "properties": {
            "enable": {
              "type": "boolean",
              "default": false,
              "description": "Enable proxy forwarding."
            },
            "baseUri": {
              "type": "string",
              "default": "/api",
              "description": "Base URI path to intercept (e.g. /api)."
            },
            "proxyUri": {
              "type": "string",
              "default": "http://127.0.0.1:8000",
              "description": "Target URL to forward requests to."
            }
          },
          "additionalProperties": false,
          "description": "Proxy setup — forward matching requests to another server."
        },
        "hotplate.wait": {
          "type": "number",
          "default": 150,
          "description": "Debounce delay before live reloading (milliseconds)."
        },
        "hotplate.headers": {
          "type": "object",
          "default": {},
          "description": "Custom HTTP response headers. Example: { \"X-Custom-Header\": \"value\", \"Cache-Control\": \"no-cache\" }"
        },
        "hotplate.mount": {
          "type": "array",
          "default": [],
          "items": {
            "type": "array",
            "minItems": 2,
            "maxItems": 2,
            "items": {
              "type": "string"
            }
          },
          "markdownDescription": "Mount additional directories at specific URL paths. Each entry is `[urlPath, fsPath]`.\n\nExample:\n```json\n[\n  [\"/node_modules\", \"./node_modules\"],\n  [\"/assets\", \"../shared/assets\"]\n]\n```"
        },
        "hotplate.multiRootWorkspaceName": {
          "type": [
            "string",
            "null"
          ],
          "default": null,
          "description": "In a multi-root workspace, set the workspace folder name to use as the Live Server root. If not set, a picker will appear."
        }
      }
    },
    "keybindings": [
      {
        "command": "hotplate.start",
        "key": "alt+l alt+o",
        "mac": "cmd+l cmd+o",
        "when": "editorTextFocus && !hotplate:running"
      },
      {
        "command": "hotplate.stop",
        "key": "alt+l alt+c",
        "mac": "cmd+l cmd+c",
        "when": "editorTextFocus && hotplate:running"
      }
    ]
  },
  "scripts": {
    "package": "vsce package",
    "publish": "vsce publish"
  },
  "devDependencies": {
    "@vscode/vsce": "^2.32.0"
  }
}

```

