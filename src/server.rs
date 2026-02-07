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

    // Static file serving with optional SPA fallback
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
    println!("  🍖 hotplate v{}", env!("CARGO_PKG_VERSION"));
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
