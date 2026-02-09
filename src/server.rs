//! HTTP/HTTPS server with static files + WebSocket live reload + SPA fallback + proxy.

use crate::inject::inject_livereload;
use crate::events::{EventData, EventLogger};
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
use std::time::Instant;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::broadcast;
use tower_http::{
    cors::{Any, CorsLayer},
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
};

/// Built-in welcome page shown when root directory has no index.html.
const WELCOME_HTML: &str = include_str!("welcome.html");

/// Max number of console entries kept in memory for the MCP `hotplate_console` tool.
const MAX_CONSOLE_ENTRIES: usize = 500;

// ───────────────────── Console log buffer ─────────────────────

/// A single console/error entry captured from the browser.
#[derive(Clone, Debug, serde::Serialize)]
pub struct ConsoleEntry {
    /// "log" | "warn" | "error" | "js_error"
    pub level: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub col: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
    /// ISO-8601 timestamp
    pub timestamp: String,
}

/// Thread-safe console log buffer shared between server and MCP.
pub type ConsoleLogBuffer = Arc<std::sync::Mutex<Vec<ConsoleEntry>>>;

// ───────────────────── Network log buffer ─────────────────────

/// Max number of network entries kept in memory for the MCP `hotplate_network` tool.
const MAX_NETWORK_ENTRIES: usize = 500;

/// A single network request entry captured from the browser.
#[derive(Clone, Debug, serde::Serialize)]
pub struct NetworkEntry {
    pub url: String,
    pub method: String,
    pub status: u16,
    /// Round-trip duration in milliseconds.
    pub duration: u32,
    /// ISO-8601 timestamp
    pub timestamp: String,
}

/// Thread-safe network log buffer shared between server and MCP.
pub type NetworkLogBuffer = Arc<std::sync::Mutex<Vec<NetworkEntry>>>;

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
    pub event_logger: EventLogger,
    pub client_counter: Arc<AtomicU64>,
    /// Channel for browser → MCP screenshot responses (id, base64 data)
    pub screenshot_tx: tokio::sync::mpsc::UnboundedSender<(String, String)>,
    /// In-memory buffer of recent console logs from connected browsers.
    pub console_logs: ConsoleLogBuffer,
    /// In-memory buffer of recent network requests from connected browsers.
    pub network_logs: NetworkLogBuffer,
    /// Channel for browser → MCP DOM query responses (id, json_data).
    pub dom_tx: tokio::sync::mpsc::UnboundedSender<(String, String)>,
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
    let client_id = format!("c{}", state.client_counter.fetch_add(1, Ordering::Relaxed));

    loop {
        tokio::select! {
            // Server → Browser: reload / inject / screenshot commands
            result = rx.recv() => {
                let Ok(changed_path) = result else { break };

                // Inject, screenshot, and dom_query messages are forwarded as-is to the browser
                let msg = if changed_path.starts_with("inject:") || changed_path.starts_with("screenshot:") || changed_path.starts_with("dom_query:") {
                    changed_path
                } else {
                    // File-change reload logic
                    let reload_type = if !state.full_reload && is_css_file(&changed_path) {
                        "css"
                    } else {
                        "full"
                    };
                    let formatted = if reload_type == "css" {
                        format!("css:{}", changed_path)
                    } else {
                        "reload".to_string()
                    };

                    state.event_logger.log(EventData::ReloadTrigger {
                        path: changed_path,
                        reload_type: reload_type.to_string(),
                    });

                    formatted
                };

                if socket.send(Message::Text(msg)).await.is_err() {
                    break;
                }
            }
            // Browser → Server: client events (console, errors, etc.)
            result = socket.recv() => {
                match result {
                    Some(Ok(Message::Text(text))) => {
                        handle_browser_message(&text, &client_id, &state);
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {} // ignore binary, ping, pong
                }
            }
        }
    }

    state.event_logger.log(EventData::WsDisconnect {
        client_id,
    });
}

/// Parse and log a JSON message from the browser.
fn handle_browser_message(text: &str, client_id: &str, state: &Arc<AppState>) {
    #[derive(serde::Deserialize)]
    struct BrowserMsg {
        kind: String,
        #[serde(default)]
        url: String,
        #[serde(default)]
        ua: String,
        #[serde(default)]
        vw: u32,
        #[serde(default)]
        vh: u32,
        #[serde(default)]
        msg: String,
        #[serde(default)]
        src: String,
        #[serde(default)]
        line: u32,
        #[serde(default)]
        col: u32,
        #[serde(default)]
        stack: String,
        #[serde(default)]
        level: String,
        #[serde(default)]
        method: String,
        #[serde(default)]
        status: u16,
        #[serde(default)]
        error: String,
        #[serde(default)]
        duration: u32,
    }

    let Ok(m) = serde_json::from_str::<BrowserMsg>(text) else { return };

    match m.kind.as_str() {
        "connect" => {
            state.event_logger.log(EventData::WsConnect {
                client_id: client_id.to_string(),
                url: m.url,
                user_agent: m.ua,
                viewport: (m.vw, m.vh),
            });
        }
        "js_error" => {
            state.event_logger.log(EventData::JsError {
                message: m.msg.clone(),
                source: m.src.clone(),
                line: m.line,
                col: m.col,
                stack: m.stack.clone(),
            });
            push_console_entry(&state.console_logs, ConsoleEntry {
                level: "js_error".into(),
                message: m.msg,
                source: Some(m.src).filter(|s| !s.is_empty()),
                line: if m.line > 0 { Some(m.line) } else { None },
                col: if m.col > 0 { Some(m.col) } else { None },
                stack: Some(m.stack).filter(|s| !s.is_empty()),
                timestamp: now_iso(),
            });
        }
        "console" => {
            state.event_logger.log(EventData::ConsoleLog {
                level: m.level.clone(),
                message: m.msg.clone(),
            });
            push_console_entry(&state.console_logs, ConsoleEntry {
                level: m.level,
                message: m.msg,
                source: None,
                line: None,
                col: None,
                stack: None,
                timestamp: now_iso(),
            });
        }
        "net_request" => {
            push_network_entry(&state.network_logs, NetworkEntry {
                url: m.url,
                method: m.method,
                status: m.status,
                duration: m.duration,
                timestamp: now_iso(),
            });
        }
        "net_error" => {
            state.event_logger.log(EventData::NetworkError {
                url: m.url,
                method: m.method,
                status: m.status,
                error: m.error,
            });
        }
        "screenshot_response" => {
            // Browser sends: {kind:"screenshot_response", id:"...", data:"base64..."}
            // Route to MCP tool via mpsc channel
            let _ = state.screenshot_tx.send((m.url, m.msg));
        }
        "dom_response" => {
            // Browser sends: {kind:"dom_response", url: id, msg: json_data}
            // Route to MCP DomTool via mpsc channel
            let _ = state.dom_tx.send((m.url, m.msg));
        }
        _ => {} // unknown kind — silently skip
    }
}

/// Check if the file path is a CSS file.
fn is_css_file(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.ends_with(".css")
}

/// Push a console entry, capping at MAX_CONSOLE_ENTRIES.
fn push_console_entry(buf: &ConsoleLogBuffer, entry: ConsoleEntry) {
    if let Ok(mut logs) = buf.lock() {
        if logs.len() >= MAX_CONSOLE_ENTRIES {
            let half = logs.len() / 2;
            logs.drain(..half); // keep recent half
        }
        logs.push(entry);
    }
}

/// Push a network entry, capping at MAX_NETWORK_ENTRIES.
fn push_network_entry(buf: &NetworkLogBuffer, entry: NetworkEntry) {
    if let Ok(mut logs) = buf.lock() {
        if logs.len() >= MAX_NETWORK_ENTRIES {
            let half = logs.len() / 2;
            logs.drain(..half);
        }
        logs.push(entry);
    }
}

/// Current time as ISO-8601 string (UTC-ish, good enough for logs).
fn now_iso() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs();
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    let ms = d.subsec_millis();
    format!("{:02}:{:02}:{:02}.{:03}", h, m, s, ms)
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

    // Cache-Control: no-cache — browser must revalidate every request (304 still works).
    // Prevents stale JS/images after live-reload triggers location.reload().
    //
    // HTTP request logging middleware (outermost — captures all requests).
    let event_logger = state.event_logger.clone();
    app.layer(middleware::from_fn(move |req: Request<Body>, next: Next| {
        let logger = event_logger.clone();
        async move {
            let method = req.method().to_string();
            let path = req.uri().path().to_string();
            // Skip WebSocket upgrade and internal paths from logging
            let should_log = !path.starts_with("/__lr");
            let start = Instant::now();
            let resp = next.run(req).await;
            if should_log {
                logger.log(EventData::HttpRequest {
                    method,
                    path,
                    status: resp.status().as_u16(),
                    duration_ms: start.elapsed().as_millis() as u64,
                });
            }
            resp
        }
    }))
    .layer(SetResponseHeaderLayer::overriding(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-cache"),
    ))
    .layer(cors)
    .with_state(state)
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

/// Optional channels that can be pre-created by the MCP layer so it shares
/// the same broadcast/screenshot channels as the running server.
pub struct ExternalChannels {
    pub reload_tx: broadcast::Sender<String>,
    pub screenshot_tx: tokio::sync::mpsc::UnboundedSender<(String, String)>,
    pub dom_tx: tokio::sync::mpsc::UnboundedSender<(String, String)>,
    pub console_logs: ConsoleLogBuffer,
    pub network_logs: NetworkLogBuffer,
}

/// Start the HTTP/HTTPS server.
///
/// If `ext` is `Some`, uses the pre-created channels (MCP mode).
/// Otherwise creates fresh ones (standalone mode).
pub async fn run(mut config: Config, ext: Option<ExternalChannels>) -> Result<()> {
    let (reload_tx, screenshot_tx, dom_tx, console_logs, network_logs) = match ext {
        Some(e) => (e.reload_tx, e.screenshot_tx, e.dom_tx, e.console_logs, e.network_logs),
        None => {
            let (rtx, _) = broadcast::channel::<String>(16);
            let (stx, _) = tokio::sync::mpsc::unbounded_channel::<(String, String)>();
            let (dtx, _) = tokio::sync::mpsc::unbounded_channel::<(String, String)>();
            let clogs = Arc::new(std::sync::Mutex::new(Vec::new()));
            let nlogs = Arc::new(std::sync::Mutex::new(Vec::new()));
            (rtx, stx, dtx, clogs, nlogs)
        }
    };

    // Event logger (JSONL)
    let event_logger = if config.event_log {
        EventLogger::new(&config.workspace)
    } else {
        EventLogger::noop()
    };

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
        event_logger: event_logger.clone(),
        client_counter: Arc::new(AtomicU64::new(0)),
        screenshot_tx,
        dom_tx,
        console_logs,
        network_logs,
    });

    // Log server start event
    event_logger.log(EventData::ServerStart {
        port: config.port,
        host: config.host.clone(),
        root: config.root.display().to_string(),
        https: config.cert.is_some(),
        live_reload: config.live_reload,
    });

    // Start file watcher
    if config.live_reload {
        watcher::spawn(config.root.clone(), reload_tx, &config.ignore_patterns, &config.watch_extensions, event_logger)?;
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
