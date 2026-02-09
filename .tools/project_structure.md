# Cấu trúc Dự án như sau:

```
./
├── Cargo.toml
├── apps
│   └── index.html
├── scripts
├── src
│   ├── config.rs
│   ├── events.rs
│   ├── inject.rs
│   ├── jsonrpc.rs
│   ├── livereload.js
│   ├── main.rs
│   ├── mcp.rs
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

## File ./src\config.rs:
```rust
//! Watcher configuration — file extensions that trigger live reload.
//!
//! By default, Hotplate only watches UI-related file extensions:
//!   html, htm, css, scss, sass, less,
//!   js, jsx, ts, tsx, mjs, cjs,
//!   json, svg, png, jpg, jpeg, gif, webp, ico,
//!   woff, woff2, ttf, eot,
//!   xml, md, tx
//!
//! Users can override this via:
//!   - CLI: `--watch-ext html --watch-ext css --watch-ext js`
//!   - VS Code settings: `"hotplate.watchExtensions": ["html", "css", "js"]`
//!   - Use `"*"` to watch ALL file extensions (disable filtering)

```

## File ./src\events.rs:
```rust
//! Event sourcing — structured JSONL event log for all server activities.
//!
//! Every server event (file change, reload, WS connect, JS error, etc.) is logged
//! as a single JSON line to `.hotplate/events-{session}.jsonl`.
//!
//! This provides AI agents with rich context to diagnose UI issues:
//!   - What files changed and when
//!   - What errors occurred in the browser (console, JS, network)
//!   - HTTP request timeline
//!   - WebSocket connection lifecycle

use serde::Serialize;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;

// ───────────────────── Event types ─────────────────────

/// All event kinds that Hotplate can produce.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", content = "data")]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum EventData {
    /// Server started with given configuration.
    ServerStart {
        port: u16,
        host: String,
        root: String,
        https: bool,
        live_reload: bool,
    },

    /// Server stopped.
    ServerStop {
        uptime_secs: u64,
    },

    /// A file was changed on disk (create/modify/remove).
    FileChange {
        path: String,
        ext: String,
        change: String, // "create" | "modify" | "remove"
    },

    /// A reload event was broadcast to connected browsers.
    ReloadTrigger {
        path: String,
        reload_type: String, // "full" | "css"
    },

    /// A browser connected via WebSocket.
    WsConnect {
        client_id: String,
        url: String,
        user_agent: String,
        viewport: (u32, u32),
    },

    /// A browser disconnected.
    WsDisconnect {
        client_id: String,
    },

    /// An HTTP request was handled.
    HttpRequest {
        method: String,
        path: String,
        status: u16,
        duration_ms: u64,
    },

    /// A JavaScript error occurred in the browser.
    JsError {
        message: String,
        source: String,
        line: u32,
        col: u32,
        stack: String,
    },

    /// A console message from the browser.
    ConsoleLog {
        level: String, // "log" | "warn" | "error" | "info"
        message: String,
    },

    /// A network error from the browser (failed fetch, non-ok status).
    NetworkError {
        url: String,
        method: String,
        status: u16,
        error: String,
    },
}

/// A single event with timestamp and session ID.
#[derive(Debug, Clone, Serialize)]
pub struct HotplateEvent {
    /// ISO 8601 timestamp (e.g. "2026-02-09T14:30:01.123Z")
    pub ts: String,
    /// Session ID (shared across one server run)
    pub session: String,
    /// Event payload
    #[serde(flatten)]
    pub data: EventData,
}

// ───────────────────── Session ID ─────────────────────

/// Generate a short session ID from current timestamp (e.g. "20260209-143001").
pub fn generate_session_id() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Convert to readable format: YYYYMMDD-HHMMSS
    let secs_per_day = 86400u64;
    let days_since_epoch = now / secs_per_day;
    let time_of_day = now % secs_per_day;

    // Simple date calculation (good enough for session IDs)
    let (year, month, day) = days_to_ymd(days_since_epoch);
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        year, month, day, hours, minutes, seconds
    )
}

/// Convert days since epoch to (year, month, day).
fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Get current ISO 8601 timestamp string.
fn now_iso() -> String {
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();
    let millis = dur.subsec_millis();

    let secs_per_day = 86400u64;
    let days = secs / secs_per_day;
    let time_of_day = secs % secs_per_day;

    let (year, month, day) = days_to_ymd(days);
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        year, month, day, hours, minutes, seconds, millis
    )
}

// ───────────────────── EventLogger ─────────────────────

/// Async event logger — receives events via mpsc channel, writes JSONL to disk.
#[derive(Clone)]
pub struct EventLogger {
    tx: mpsc::UnboundedSender<HotplateEvent>,
    session: String,
}

impl EventLogger {
    /// Create a new EventLogger that writes to `.hotplate/logs/events-{session}.jsonl`
    /// in the given workspace directory.
    ///
    /// Spawns a background tokio task for non-blocking writes.
    pub fn new(workspace: &Path) -> Self {
        let session = generate_session_id();
        let (tx, rx) = mpsc::unbounded_channel();

        let log_dir = workspace.join(".hotplate").join("logs");
        let log_file = log_dir.join(format!("events-{}.jsonl", session));

        // Spawn writer task
        let session_clone = session.clone();
        tokio::spawn(async move {
            Self::writer_loop(rx, log_dir, log_file, &session_clone).await;
        });

        EventLogger { tx, session }
    }

    /// Create a no-op logger that discards all events (when --no-event-log).
    pub fn noop() -> Self {
        let (tx, _rx) = mpsc::unbounded_channel();
        EventLogger {
            tx,
            session: "noop".into(),
        }
    }

    /// Background writer loop — creates dir, opens file, writes events.
    async fn writer_loop(
        mut rx: mpsc::UnboundedReceiver<HotplateEvent>,
        log_dir: PathBuf,
        log_file: PathBuf,
        session: &str,
    ) {
        use std::io::Write;

        // Create .hotplate directory
        if let Err(e) = std::fs::create_dir_all(&log_dir) {
            eprintln!("  ⚠ Failed to create event log dir: {}", e);
            return;
        }

        // Clean up old session files (keep last 10)
        Self::cleanup_old_sessions(&log_dir, 10);

        // Open log file (append mode)
        let mut file = match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file)
        {
            Ok(f) => f,
            Err(e) => {
                eprintln!("  ⚠ Failed to open event log: {}", e);
                return;
            }
        };

        println!("  📋 Events:  .hotplate/logs/events-{}.jsonl", session);

        // Write events as they arrive
        while let Some(event) = rx.recv().await {
            if let Ok(json) = serde_json::to_string(&event) {
                let _ = writeln!(file, "{}", json);
                let _ = file.flush();
            }
        }
    }

    /// Remove old session files, keeping only the most recent `keep` files.
    fn cleanup_old_sessions(log_dir: &Path, keep: usize) {
        let mut files: Vec<_> = std::fs::read_dir(log_dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("events-")
                    && e.file_name().to_string_lossy().ends_with(".jsonl")
            })
            .collect();

        if files.len() <= keep {
            return;
        }

        // Sort by name (which includes timestamp, so chronological)
        files.sort_by_key(|e| e.file_name());

        // Remove oldest files
        let to_remove = files.len() - keep;
        for entry in files.into_iter().take(to_remove) {
            let _ = std::fs::remove_file(entry.path());
        }
    }

    /// Log an event (non-blocking — sends to writer task via channel).
    pub fn log(&self, data: EventData) {
        let event = HotplateEvent {
            ts: now_iso(),
            session: self.session.clone(),
            data,
        };
        let _ = self.tx.send(event);
    }

    /// Get the session ID.
    #[allow(dead_code)]
    pub fn session(&self) -> &str {
        &self.session
    }
}

```

## File ./src\inject.rs:
```rust
//! HTML injection middleware — inserts live-reload WebSocket script before </body>.

use axum::{
    body::Body,
    http::{header, Request, Response},
    middleware::Next,
};
use http_body_util::BodyExt;

/// Live-reload + browser agent script, loaded from `src/livereload.js`.
/// Using `include_str!` embeds the JS at compile time — zero runtime cost,
/// and the JS file gets proper syntax highlighting & lint in the IDE.
const RELOAD_JS: &str = include_str!("livereload.js");

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

    // Build <script>...</script> from the external JS file
    let reload_script = format!("<script>\n{}\n</script>", RELOAD_JS);

    // Inject before </body>, or </html>, or at the end
    let injected = if let Some(pos) = html.rfind("</body>") {
        format!("{}{}\n{}", &html[..pos], reload_script, &html[pos..])
    } else if let Some(pos) = html.rfind("</html>") {
        format!("{}{}\n{}", &html[..pos], reload_script, &html[pos..])
    } else {
        format!("{}\n{}", html, reload_script)
    };

    // Remove old Content-Length (body size changed)
    parts.headers.remove(header::CONTENT_LENGTH);

    Response::from_parts(parts, Body::from(injected))
}

```

## File ./src\jsonrpc.rs:
```rust
//! JSON-RPC 2.0 protocol types

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC 2.0 Request
#[derive(Deserialize, Debug, Clone)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    /// Check if this is a valid JSON-RPC 2.0 request
    pub fn is_valid(&self) -> bool {
        self.jsonrpc == "2.0"
    }

    /// Check if this is a notification (no id)
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

/// JSON-RPC 2.0 Success Response
#[derive(Serialize, Debug)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Value,
    pub result: Value,
}

impl JsonRpcResponse {
    /// Create a new success response
    pub fn new(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result,
        }
    }
}

/// JSON-RPC 2.0 Error Response
#[derive(Serialize, Debug)]
pub struct JsonRpcError {
    pub jsonrpc: String,
    pub id: Value,
    pub error: ErrorObject,
}

impl JsonRpcError {
    /// Create a new error response
    pub fn new(id: Value, code: i32, message: String, data: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            error: ErrorObject {
                code,
                message,
                data,
            },
        }
    }

    /// Create a parse error response
    pub fn parse_error(id: Value, details: String) -> Self {
        Self::new(
            id,
            -32700,
            "Parse error".to_string(),
            Some(serde_json::json!({"details": details})),
        )
    }

    /// Create an invalid request error response
    pub fn invalid_request(id: Value, details: String) -> Self {
        Self::new(
            id,
            -32600,
            "Invalid Request".to_string(),
            Some(serde_json::json!({"details": details})),
        )
    }

    /// Create a method not found error response
    pub fn method_not_found(id: Value, method: String) -> Self {
        Self::new(
            id,
            -32601,
            "Method not found".to_string(),
            Some(serde_json::json!({"method": method})),
        )
    }

    /// Create an invalid params error response
    pub fn invalid_params(id: Value, details: String) -> Self {
        Self::new(
            id,
            -32602,
            "Invalid params".to_string(),
            Some(serde_json::json!({"details": details})),
        )
    }

    /// Create an internal error response
    pub fn internal_error(id: Value, details: String) -> Self {
        Self::new(
            id,
            -32603,
            "Internal error".to_string(),
            Some(serde_json::json!({"details": details})),
        )
    }
}

/// JSON-RPC 2.0 Error Object
#[derive(Serialize, Debug)]
pub struct ErrorObject {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl ErrorObject {
    /// Create a new error object
    pub fn new(code: i32, message: String, data: Option<Value>) -> Self {
        Self {
            code,
            message,
            data,
        }
    }
}

```

## File ./src\main.rs:
```rust
//! ⚡ hotplate — Fast HTTPS live-reload dev server
//!
//! Usage:
//!   hotplate --root ./apps --port 5500
//!   hotplate --https                   # auto-generates self-signed cert in .hotplate/certs/
//!   hotplate --root ./apps --cert .hotplate/certs/server.crt --key .hotplate/certs/server.key
//!   hotplate                          # auto-reads .vscode/settings.json

mod events;
mod inject;
#[allow(dead_code)]
mod jsonrpc;
mod mcp;
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

    /// File extensions to watch for live reload (can be repeated, e.g. --watch-ext html --watch-ext css)
    /// Defaults to UI-related extensions (html, css, js, ts, etc.). Use "*" to watch all files.
    #[arg(long = "watch-ext")]
    watch_extensions: Vec<String>,

    /// Disable event logging (no .hotplate/logs/events-*.jsonl files)
    #[arg(long, default_value_t = false)]
    no_event_log: bool,

    /// Run as MCP server (stdio JSON-RPC) instead of HTTP server
    #[arg(long, default_value_t = false)]
    mcp: bool,
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
    pub watch_extensions: Vec<String>,
    pub spa_file: Option<String>,
    pub proxy_base: Option<String>,
    pub proxy_target: Option<String>,
    pub headers: Vec<(String, String)>,
    pub mounts: Vec<(String, PathBuf)>,
    pub event_log: bool,
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
    #[serde(rename = "hotplate.watchExtensions", default)]
    watch_extensions: Option<Vec<String>>,
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

/// Generate a self-signed TLS certificate and key in `<workspace>/.hotplate/certs/`.
/// Returns the paths to the generated cert and key files.
/// If the files already exist, they are reused without regeneration.
/// Also migrates old `.cert/` directory layout if found.
pub(crate) fn generate_self_signed_cert(workspace: &Path) -> Result<(PathBuf, PathBuf)> {
    let cert_dir = workspace.join(".hotplate").join("certs");
    let cert_path = cert_dir.join("hotplate.crt");
    let key_path = cert_dir.join("hotplate.key");

    // Migrate from old .cert/ layout → .hotplate/certs/
    let old_cert_dir = workspace.join(".cert");
    let old_cert = old_cert_dir.join("hotplate.crt");
    let old_key = old_cert_dir.join("hotplate.key");
    if old_cert.exists() && old_key.exists() && !cert_path.exists() {
        std::fs::create_dir_all(&cert_dir)
            .with_context(|| format!("Failed to create directory: {}", cert_dir.display()))?;
        std::fs::rename(&old_cert, &cert_path).ok();
        std::fs::rename(&old_key, &key_path).ok();
        // Remove old directory if empty
        let _ = std::fs::remove_dir(&old_cert_dir);
        println!("  🔄 Migrated certs from .cert/ → .hotplate/certs/");
    }

    // Reuse existing certs if they exist
    if cert_path.exists() && key_path.exists() {
        println!("  🔒 Reusing existing self-signed cert at .hotplate/certs/");
        return Ok((cert_path, key_path));
    }

    // Create .hotplate/certs directory
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

    println!("  🔒 Generated self-signed certificate in .hotplate/certs/:");
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

    // Watch extensions: CLI --watch-ext > vscode watchExtensions > [] (use defaults in watcher)
    let watch_extensions = if !cli.watch_extensions.is_empty() {
        cli.watch_extensions
    } else {
        vs.as_ref()
            .and_then(|s| s.watch_extensions.clone())
            .unwrap_or_default()
    };

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
        watch_extensions,
        spa_file: cli.file,
        proxy_base: cli.proxy_base,
        proxy_target: cli.proxy_target,
        headers: parse_headers(&cli.headers),
        mounts,
        event_log: !cli.no_event_log,
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

fn main() -> Result<()> {
    // Install rustls crypto provider (ring) before any TLS usage
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    let cli = Cli::parse();

    // ── MCP mode: stdio JSON-RPC server for AI agents ──
    if cli.mcp {
        return mcp::run_mcp().map_err(|e| anyhow::anyhow!("{}", e));
    }

    // ── Normal mode: HTTP/HTTPS dev server ──
    let config = build_config(cli).context("Failed to load configuration")?;
    let rt = tokio::runtime::Runtime::new().context("Failed to create tokio runtime")?;
    rt.block_on(server::run(config, None))
}

```

## File ./src\mcp.rs:
```rust
//! MCP (Model Context Protocol) server — stdio JSON-RPC 2.0
//!
//! Implements the MCP protocol for AI agents to control Hotplate:
//!   - `hotplate_status`  — get current server status
//!   - `hotplate_start`   — start the live server (background)
//!   - `hotplate_stop`    — stop the live server
//!   - `hotplate_reload`  — force reload all connected browsers
//!
//! Usage:
//!   hotplate --mcp   # runs MCP stdio server instead of HTTP server
//!
//! Architecture (following memory-graph pattern):
//!   AI Agent ← JSON-RPC 2.0 (stdin/stdout) → McpServer → tools → HotplateState
//!
//! Future: SSE transport can be added alongside stdio.

use crate::jsonrpc::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};
use crate::server::ExternalChannels;
use crate::Config;

use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;

/// Result type for MCP operations.
pub type McpResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

// ───────────────────── Tool trait ─────────────────────

/// MCP Tool definition (serialised in `tools/list` response).
#[derive(Serialize, Debug, Clone)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

/// Every tool implements this trait.
pub trait Tool: Send + Sync {
    fn definition(&self) -> McpTool;
    fn execute(&self, params: Value) -> McpResult<Value>;
}

// ───────────────────── Shared state ─────────────────────

/// Shared state between MCP tools and the background HTTP server.
pub struct HotplateState {
    /// Whether the HTTP server is currently running.
    pub running: Arc<AtomicBool>,
    /// Broadcast channel to trigger browser reloads / inject / screenshot commands.
    pub reload_tx: Option<broadcast::Sender<String>>,
    /// Current server config (set after `hotplate_start`).
    pub config: Option<Config>,
    /// Tokio runtime handle — used to spawn the HTTP server.
    pub rt_handle: tokio::runtime::Handle,
    /// Server task handle (so we can abort on `hotplate_stop`).
    pub server_handle: Option<tokio::task::JoinHandle<()>>,
    /// Receiver for screenshot responses from the browser (id, base64).
    pub screenshot_rx: Option<Arc<tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<(String, String)>>>>,
    /// Shared in-memory buffer of browser console logs.
    pub console_logs: Option<crate::server::ConsoleLogBuffer>,
    /// Shared in-memory buffer of browser network requests.
    pub network_logs: Option<crate::server::NetworkLogBuffer>,
    /// Receiver for DOM query responses from the browser (id, json_data).
    pub dom_rx: Option<Arc<tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<(String, String)>>>>,
    /// Receiver for eval responses from the browser (id, result_json).
    pub eval_rx: Option<Arc<tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<(String, String)>>>>,
}

// ───────────────────── McpServer ─────────────────────

/// MCP Server — reads JSON-RPC from stdin, writes responses to stdout.
///
/// Logs go to **stderr** so they are visible to users but invisible
/// to the AI agent reading stdout.
pub struct McpServer {
    tools: HashMap<String, Box<dyn Tool>>,
    reader: BufReader<io::Stdin>,
    stdout: BufWriter<io::Stdout>,
}

impl McpServer {
    /// Create a new MCP server.
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            reader: BufReader::new(io::stdin()),
            stdout: BufWriter::new(io::stdout()),
        }
    }

    /// Register a tool.
    pub fn register_tool(&mut self, tool: Box<dyn Tool>) -> &mut Self {
        let name = tool.definition().name.clone();
        self.tools.insert(name, tool);
        self
    }

    /// Blocking stdio loop — same pattern as memory-graph.
    pub fn run(&mut self) -> McpResult<()> {
        let mut line = String::new();
        while self.reader.read_line(&mut line)? > 0 {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                self.handle_request(trimmed)?;
            }
            line.clear();
        }
        Ok(())
    }

    // ── request routing ──

    fn handle_request(&mut self, raw: &str) -> McpResult<()> {
        let request: JsonRpcRequest = match serde_json::from_str(raw) {
            Ok(r) => r,
            Err(e) => {
                self.send_error(Value::Null, -32700, "Parse error",
                    Some(json!({"details": e.to_string()})))?;
                return Ok(());
            }
        };

        if request.jsonrpc != "2.0" {
            let id = request.id.unwrap_or(Value::Null);
            self.send_error(id, -32600, "Invalid Request",
                Some(json!({"details": "jsonrpc must be '2.0'"})))?;
            return Ok(());
        }

        let id = request.id.clone().unwrap_or(Value::Null);

        match request.method.as_str() {
            "initialize"               => self.handle_initialize(id),
            "notifications/initialized" => Ok(()), // notification — no response
            "tools/list"               => self.handle_tools_list(id),
            "tools/call"               => self.handle_tool_call(id, request.params),
            "ping"                     => self.send_success(id, json!({})),
            _ => self.send_error(id, -32601, "Method not found",
                    Some(json!({"method": request.method}))),
        }
    }

    fn handle_initialize(&mut self, id: Value) -> McpResult<()> {
        let result = json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": {
                "name": "hotplate",
                "version": env!("CARGO_PKG_VERSION")
            }
        });
        self.send_success(id, result)
    }

    fn handle_tools_list(&mut self, id: Value) -> McpResult<()> {
        let tools: Vec<McpTool> = self.tools.values().map(|t| t.definition()).collect();
        self.send_success(id, json!({ "tools": tools }))
    }

    fn handle_tool_call(&mut self, id: Value, params: Option<Value>) -> McpResult<()> {
        let params = match params {
            Some(p) => p,
            None => return self.send_error(id, -32602, "Invalid params",
                        Some(json!({"details": "Missing parameters"}))),
        };
        let tool_name = match params.get("name").and_then(|v| v.as_str()) {
            Some(n) => n.to_string(),
            None => return self.send_error(id, -32602, "Invalid params",
                        Some(json!({"details": "Missing tool name"}))),
        };
        let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

        let tool = match self.tools.get(&tool_name) {
            Some(t) => t,
            None => return self.send_error(id, -32602, "Unknown tool",
                        Some(json!({"tool": tool_name}))),
        };

        match tool.execute(arguments) {
            Ok(result) => self.send_success(id, result),
            Err(e) => self.send_error(id, -32603, "Tool execution error",
                        Some(json!({"details": e.to_string()}))),
        }
    }

    // ── response helpers ──

    fn send_success(&mut self, id: Value, result: Value) -> McpResult<()> {
        let resp = JsonRpcResponse::new(id, result);
        writeln!(self.stdout, "{}", serde_json::to_string(&resp)?)?;
        self.stdout.flush()?;
        Ok(())
    }

    fn send_error(&mut self, id: Value, code: i32, msg: &str, data: Option<Value>) -> McpResult<()> {
        let resp = JsonRpcError::new(id, code, msg.to_string(), data);
        writeln!(self.stdout, "{}", serde_json::to_string(&resp)?)?;
        self.stdout.flush()?;
        Ok(())
    }
}

// ───────────────────── helpers ─────────────────────

/// Build a standard MCP text-content response.
fn text_response(text: String) -> Value {
    json!({
        "content": [{ "type": "text", "text": text }]
    })
}

// ───────────────────── hotplate_status ─────────────────────

struct StatusTool {
    state: Arc<std::sync::Mutex<HotplateState>>,
}

impl Tool for StatusTool {
    fn definition(&self) -> McpTool {
        McpTool {
            name: "hotplate_status".into(),
            description: "Get current Hotplate dev-server status.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        }
    }

    fn execute(&self, _params: Value) -> McpResult<Value> {
        let st = self.state.lock().map_err(|e| format!("Lock: {e}"))?;
        let running = st.running.load(Ordering::Relaxed);

        let info = if let Some(ref cfg) = st.config {
            json!({
                "running": running,
                "port":    cfg.port,
                "host":    cfg.host,
                "root":    cfg.root.display().to_string(),
                "https":   cfg.cert.is_some(),
                "live_reload": cfg.live_reload,
            })
        } else {
            json!({ "running": false })
        };

        Ok(text_response(serde_json::to_string_pretty(&info)?))
    }
}

// ───────────────────── hotplate_start ─────────────────────

struct StartTool {
    state: Arc<std::sync::Mutex<HotplateState>>,
}

impl Tool for StartTool {
    fn definition(&self) -> McpTool {
        McpTool {
            name: "hotplate_start".into(),
            description: "Start the Hotplate live-reload dev server (background).".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "root":  { "type": "string",  "description": "Root directory (default: .)" },
                    "port":  { "type": "number",  "description": "Port (default: 5500)" },
                    "https": { "type": "boolean", "description": "Enable HTTPS (default: false)" }
                },
                "required": []
            }),
        }
    }

    fn execute(&self, params: Value) -> McpResult<Value> {
        let mut st = self.state.lock().map_err(|e| format!("Lock: {e}"))?;

        if st.running.load(Ordering::Relaxed) {
            return Ok(text_response("Server is already running. Stop it first.".into()));
        }

        let root_str = params.get("root").and_then(|v| v.as_str()).unwrap_or(".");
        let port  = params.get("port").and_then(|v| v.as_u64()).unwrap_or(5500) as u16;
        let https = params.get("https").and_then(|v| v.as_bool()).unwrap_or(false);

        let workspace = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let root = if std::path::Path::new(root_str).is_absolute() {
            std::path::PathBuf::from(root_str)
        } else {
            workspace.join(root_str)
        };
        if !root.exists() {
            return Ok(text_response(format!("Root directory not found: {}", root.display())));
        }

        let (cert, key) = if https {
            match crate::generate_self_signed_cert(&workspace) {
                Ok(pair) => (Some(pair.0), Some(pair.1)),
                Err(e) => return Ok(text_response(format!("Cert error: {e}"))),
            }
        } else {
            (None, None)
        };

        let (reload_tx, _) = broadcast::channel::<String>(16);
        let (screenshot_tx, screenshot_rx) = tokio::sync::mpsc::unbounded_channel::<(String, String)>();
        let (dom_tx, dom_rx) = tokio::sync::mpsc::unbounded_channel::<(String, String)>();
        let (eval_tx, eval_rx) = tokio::sync::mpsc::unbounded_channel::<(String, String)>();
        let console_logs: crate::server::ConsoleLogBuffer = Arc::new(std::sync::Mutex::new(Vec::new()));
        let network_logs: crate::server::NetworkLogBuffer = Arc::new(std::sync::Mutex::new(Vec::new()));
        st.reload_tx = Some(reload_tx.clone());
        st.screenshot_rx = Some(Arc::new(tokio::sync::Mutex::new(screenshot_rx)));
        st.dom_rx = Some(Arc::new(tokio::sync::Mutex::new(dom_rx)));
        st.eval_rx = Some(Arc::new(tokio::sync::Mutex::new(eval_rx)));
        st.console_logs = Some(console_logs.clone());
        st.network_logs = Some(network_logs.clone());

        let ext = ExternalChannels {
            reload_tx: reload_tx.clone(),
            screenshot_tx,
            dom_tx,
            eval_tx,
            console_logs,
            network_logs,
        };

        let config = Config {
            host: "0.0.0.0".into(),
            port,
            root: root.clone(),
            cert,
            key,
            live_reload: true,
            full_reload: false,
            workspace,
            ignore_patterns: vec![],
            watch_extensions: vec![],
            spa_file: None,
            proxy_base: None,
            proxy_target: None,
            headers: vec![],
            mounts: vec![],
            event_log: true,
        };

        let scheme = if config.cert.is_some() { "https" } else { "http" };
        let msg = format!("Server starting on {}://localhost:{} serving {}",
                          scheme, config.port, config.root.display());

        st.config = Some(config.clone());
        let running = st.running.clone();
        running.store(true, Ordering::Relaxed);

        let handle = st.rt_handle.spawn(async move {
            if let Err(e) = crate::server::run(config, Some(ext)).await {
                eprintln!("[hotplate-mcp] Server error: {e}");
            }
            running.store(false, Ordering::Relaxed);
        });
        st.server_handle = Some(handle);

        Ok(text_response(msg))
    }
}

// ───────────────────── hotplate_stop ─────────────────────

struct StopTool {
    state: Arc<std::sync::Mutex<HotplateState>>,
}

impl Tool for StopTool {
    fn definition(&self) -> McpTool {
        McpTool {
            name: "hotplate_stop".into(),
            description: "Stop the running Hotplate server.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        }
    }

    fn execute(&self, _params: Value) -> McpResult<Value> {
        let mut st = self.state.lock().map_err(|e| format!("Lock: {e}"))?;

        if !st.running.load(Ordering::Relaxed) {
            return Ok(text_response("Server is not running.".into()));
        }

        if let Some(h) = st.server_handle.take() {
            h.abort();
        }
        st.running.store(false, Ordering::Relaxed);
        st.config = None;
        st.reload_tx = None;
        st.screenshot_rx = None;
        st.dom_rx = None;
        st.eval_rx = None;
        st.console_logs = None;
        st.network_logs = None;

        Ok(text_response("Server stopped.".into()))
    }
}

// ───────────────────── hotplate_reload ─────────────────────

struct ReloadTool {
    state: Arc<std::sync::Mutex<HotplateState>>,
}

impl Tool for ReloadTool {
    fn definition(&self) -> McpTool {
        McpTool {
            name: "hotplate_reload".into(),
            description: "Force-reload all connected browsers.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path for CSS hot-swap detection. Omit for full reload."
                    }
                },
                "required": []
            }),
        }
    }

    fn execute(&self, params: Value) -> McpResult<Value> {
        let st = self.state.lock().map_err(|e| format!("Lock: {e}"))?;

        if !st.running.load(Ordering::Relaxed) {
            return Ok(text_response("Server is not running.".into()));
        }

        let path = params.get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("manual-reload")
            .to_string();

        match st.reload_tx {
            Some(ref tx) => match tx.send(path.clone()) {
                Ok(n) => Ok(text_response(
                    format!("Reload triggered ('{path}'). {n} browser(s) notified."))),
                Err(_) => Ok(text_response("Reload sent but no browsers connected.".into())),
            },
            None => Ok(text_response("No reload channel (live reload may be off).".into())),
        }
    }
}

// ───────────────────── hotplate_console ─────────────────────

struct ConsoleTool {
    state: Arc<std::sync::Mutex<HotplateState>>,
}

impl Tool for ConsoleTool {
    fn definition(&self) -> McpTool {
        McpTool {
            name: "hotplate_console".into(),
            description: "Get browser console logs from connected clients.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "level": {
                        "type": "string",
                        "enum": ["all", "warn", "error", "js_error"],
                        "description": "Filter by log level. Default: 'all'."
                    },
                    "clear": {
                        "type": "boolean",
                        "description": "Clear the log buffer after reading. Default: false."
                    }
                },
                "required": []
            }),
        }
    }

    fn execute(&self, params: Value) -> McpResult<Value> {
        let st = self.state.lock().map_err(|e| format!("Lock: {e}"))?;

        if !st.running.load(Ordering::Relaxed) {
            return Ok(text_response("Server is not running.".into()));
        }

        let console_logs = match st.console_logs {
            Some(ref buf) => buf.clone(),
            None => return Ok(text_response("Console log buffer not available.".into())),
        };

        // Drop HotplateState lock before locking console buffer
        drop(st);

        let level_filter = params.get("level")
            .and_then(|v| v.as_str())
            .unwrap_or("all");
        let clear = params.get("clear")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let mut buf = console_logs.lock().map_err(|e| format!("Lock: {e}"))?;

        let entries: Vec<&crate::server::ConsoleEntry> = if level_filter == "all" {
            buf.iter().collect()
        } else {
            buf.iter().filter(|e| e.level == level_filter).collect()
        };

        let result = json!({
            "total": entries.len(),
            "logs": entries
        });

        let text = serde_json::to_string_pretty(&result)?;

        if clear {
            buf.clear();
        }

        Ok(text_response(text))
    }
}

// ───────────────────── hotplate_network ─────────────────────

struct NetworkTool {
    state: Arc<std::sync::Mutex<HotplateState>>,
}

impl Tool for NetworkTool {
    fn definition(&self) -> McpTool {
        McpTool {
            name: "hotplate_network".into(),
            description: "Get network requests from connected browsers.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "method": {
                        "type": "string",
                        "description": "Filter by HTTP method (e.g. 'GET', 'POST'). Default: all."
                    },
                    "status": {
                        "type": "integer",
                        "description": "Filter by exact status code. Default: all."
                    },
                    "clear": {
                        "type": "boolean",
                        "description": "Clear the buffer after reading. Default: false."
                    }
                },
                "required": []
            }),
        }
    }

    fn execute(&self, params: Value) -> McpResult<Value> {
        let st = self.state.lock().map_err(|e| format!("Lock: {e}"))?;

        if !st.running.load(Ordering::Relaxed) {
            return Ok(text_response("Server is not running.".into()));
        }

        let network_logs = match st.network_logs {
            Some(ref buf) => buf.clone(),
            None => return Ok(text_response("Network log buffer not available.".into())),
        };

        // Drop HotplateState lock before locking network buffer
        drop(st);

        let method_filter = params.get("method").and_then(|v| v.as_str());
        let status_filter = params.get("status").and_then(|v| v.as_u64()).map(|s| s as u16);
        let clear = params.get("clear").and_then(|v| v.as_bool()).unwrap_or(false);

        let mut buf = network_logs.lock().map_err(|e| format!("Lock: {e}"))?;

        let entries: Vec<&crate::server::NetworkEntry> = buf.iter().filter(|e| {
            if let Some(m) = method_filter {
                if !e.method.eq_ignore_ascii_case(m) { return false; }
            }
            if let Some(s) = status_filter {
                if e.status != s { return false; }
            }
            true
        }).collect();

        let result = json!({
            "total": entries.len(),
            "requests": entries
        });

        let text = serde_json::to_string_pretty(&result)?;

        if clear {
            buf.clear();
        }

        Ok(text_response(text))
    }
}

// ───────────────────── hotplate_server_logs ─────────────────────

struct ServerLogsTool {
    state: Arc<std::sync::Mutex<HotplateState>>,
}

impl Tool for ServerLogsTool {
    fn definition(&self) -> McpTool {
        McpTool {
            name: "hotplate_server_logs".into(),
            description: "Get server-side event logs (file changes, reloads, errors, HTTP requests, WS connections).".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "kind": {
                        "type": "string",
                        "enum": ["all", "server_start", "server_stop", "file_change", "reload_trigger",
                                 "ws_connect", "ws_disconnect", "http_request", "js_error", "console_log", "network_error"],
                        "description": "Filter by event kind. Default: 'all'."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max number of entries to return (from the end, most recent). Default: 100."
                    },
                    "session": {
                        "type": "string",
                        "enum": ["current", "latest", "all"],
                        "description": "Which session file to read. 'current' = running server session, 'latest' = most recent log file, 'all' = list available sessions. Default: 'current'."
                    }
                },
                "required": []
            }),
        }
    }

    fn execute(&self, params: Value) -> McpResult<Value> {
        let st = self.state.lock().map_err(|e| format!("Lock: {e}"))?;

        // Determine workspace path from config or cwd
        let workspace = if let Some(ref cfg) = st.config {
            cfg.workspace.clone()
        } else {
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
        };
        drop(st);

        let log_dir = workspace.join(".hotplate").join("logs");
        if !log_dir.exists() {
            return Ok(text_response("No event logs found. Server may not have been started with event logging enabled.".into()));
        }

        let session_mode = params.get("session")
            .and_then(|v| v.as_str())
            .unwrap_or("current");

        // List available session files
        let mut session_files: Vec<std::path::PathBuf> = std::fs::read_dir(&log_dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.starts_with("events-") && name.ends_with(".jsonl")
            })
            .map(|e| e.path())
            .collect();
        session_files.sort();

        if session_files.is_empty() {
            return Ok(text_response("No event log files found.".into()));
        }

        // Handle "all" mode — list available sessions
        if session_mode == "all" {
            let sessions: Vec<String> = session_files.iter()
                .map(|p| {
                    let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
                    let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
                    format!("{} ({}B)", name, size)
                })
                .collect();
            let result = json!({
                "total": sessions.len(),
                "sessions": sessions
            });
            return Ok(text_response(serde_json::to_string_pretty(&result)?));
        }

        // Pick which file to read
        let log_file = if session_mode == "current" {
            // Find the current session's file (from running config)
            let st = self.state.lock().map_err(|e| format!("Lock: {e}"))?;
            if st.running.load(Ordering::Relaxed) {
                // Most recent file is likely the current session
                session_files.last().cloned()
            } else {
                // Server not running — show latest
                session_files.last().cloned()
            }
        } else {
            // "latest"
            session_files.last().cloned()
        };

        let log_file = match log_file {
            Some(f) => f,
            None => return Ok(text_response("No event log files found.".into())),
        };

        // Read and parse the JSONL file
        let content = match std::fs::read_to_string(&log_file) {
            Ok(c) => c,
            Err(e) => return Ok(text_response(format!("Failed to read log file: {e}"))),
        };

        let kind_filter = params.get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("all");
        let limit = params.get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(100) as usize;

        // Parse lines as raw JSON values, filter by kind
        let mut entries: Vec<Value> = content.lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|entry| {
                if kind_filter == "all" { return true; }
                entry.get("kind")
                    .and_then(|k| k.as_str())
                    .map(|k| k == kind_filter)
                    .unwrap_or(false)
            })
            .collect();

        // Take last N entries (most recent)
        let total = entries.len();
        if entries.len() > limit {
            entries = entries.split_off(entries.len() - limit);
        }

        let session_name = log_file.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let result = json!({
            "session": session_name,
            "total": total,
            "showing": entries.len(),
            "logs": entries
        });

        Ok(text_response(serde_json::to_string_pretty(&result)?))
    }
}

// ───────────────────── hotplate_inject ─────────────────────

struct InjectTool {
    state: Arc<std::sync::Mutex<HotplateState>>,
}

impl Tool for InjectTool {
    fn definition(&self) -> McpTool {
        McpTool {
            name: "hotplate_inject".into(),
            description: "Inject custom script/CSS into all connected pages.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "code": {
                        "type": "string",
                        "description": "The JavaScript or CSS code to inject."
                    },
                    "type": {
                        "type": "string",
                        "enum": ["js", "css"],
                        "description": "Type of code to inject: 'js' or 'css'."
                    }
                },
                "required": ["code", "type"]
            }),
        }
    }

    fn execute(&self, params: Value) -> McpResult<Value> {
        let st = self.state.lock().map_err(|e| format!("Lock: {e}"))?;

        if !st.running.load(Ordering::Relaxed) {
            return Ok(text_response("Server is not running.".into()));
        }

        let code = params.get("code")
            .and_then(|v| v.as_str())
            .ok_or("Missing 'code' parameter")?;
        let inject_type = params.get("type")
            .and_then(|v| v.as_str())
            .ok_or("Missing 'type' parameter")?;

        let msg = match inject_type {
            "js"  => format!("inject:js:{code}"),
            "css" => format!("inject:css:{code}"),
            other => return Ok(text_response(
                format!("Unknown inject type '{other}'. Use 'js' or 'css'."))),
        };

        match st.reload_tx {
            Some(ref tx) => match tx.send(msg) {
                Ok(n) => Ok(text_response(
                    format!("Injected {inject_type} into {n} browser(s)."))),
                Err(_) => Ok(text_response("Inject sent but no browsers connected.".into())),
            },
            None => Ok(text_response("No reload channel available.".into())),
        }
    }
}

// ───────────────────── hotplate_dom ─────────────────────

struct DomTool {
    state: Arc<std::sync::Mutex<HotplateState>>,
}

impl Tool for DomTool {
    fn definition(&self) -> McpTool {
        McpTool {
            name: "hotplate_dom".into(),
            description: "Query DOM from connected browser using a CSS selector.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "selector": {
                        "type": "string",
                        "description": "CSS selector to query (e.g. 'h1', '.card', '#main')"
                    }
                },
                "required": ["selector"]
            }),
        }
    }

    fn execute(&self, params: Value) -> McpResult<Value> {
        let st = self.state.lock().map_err(|e| format!("Lock: {e}"))?;

        if !st.running.load(Ordering::Relaxed) {
            return Ok(text_response("Server is not running.".into()));
        }

        let selector = params.get("selector")
            .and_then(|v| v.as_str())
            .ok_or("Missing 'selector' parameter")?;

        let request_id = format!("dom_{}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis());

        // Send DOM query to browser: "dom_query:{id}:{selector}"
        let msg = format!("dom_query:{}:{}", request_id, selector);
        let tx = match st.reload_tx {
            Some(ref tx) => tx.clone(),
            None => return Ok(text_response("No reload channel available.".into())),
        };
        match tx.send(msg) {
            Ok(0) | Err(_) => {
                return Ok(text_response("No browsers connected to query DOM.".into()));
            }
            _ => {}
        }

        // Get dom_rx and rt_handle to wait for response
        let dom_rx = match st.dom_rx {
            Some(ref rx) => rx.clone(),
            None => return Ok(text_response("DOM channel not available.".into())),
        };
        let rt_handle = st.rt_handle.clone();
        let rid = request_id.clone();

        // Drop the lock before blocking wait
        drop(st);

        // Wait for the browser response with a timeout
        let result = rt_handle.block_on(async {
            let mut rx = dom_rx.lock().await;
            tokio::time::timeout(Duration::from_secs(10), async {
                while let Some((id, data)) = rx.recv().await {
                    if id == rid {
                        return Some(data);
                    }
                }
                None
            }).await
        });

        match result {
            Ok(Some(json_data)) => {
                // Parse the JSON string from browser to get structured data
                let parsed: Value = serde_json::from_str(&json_data)
                    .unwrap_or_else(|_| json!({"error": "Failed to parse DOM response"}));

                // Check if it's an error response
                if let Some(err) = parsed.get("error").and_then(|v| v.as_str()) {
                    return Ok(text_response(format!("DOM query error: {}", err)));
                }

                let elements = if let Some(arr) = parsed.as_array() { arr.len() } else { 0 };
                let result = json!({
                    "selector": selector,
                    "total": elements,
                    "elements": parsed
                });
                Ok(text_response(serde_json::to_string_pretty(&result)?))
            }
            Ok(None) => Ok(text_response("DOM query returned no data.".into())),
            Err(_) => Ok(text_response("DOM query timed out after 10s. Is a browser page open?".into())),
        }
    }
}

// ───────────────────── hotplate_eval ─────────────────────

struct EvalTool {
    state: Arc<std::sync::Mutex<HotplateState>>,
}

impl Tool for EvalTool {
    fn definition(&self) -> McpTool {
        McpTool {
            name: "hotplate_eval".into(),
            description: "Evaluate JavaScript expression on page or element".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "function": {
                        "type": "string",
                        "description": "() => { /* code */ } or (element) => { /* code */ } when element is provided"
                    },
                    "ref": {
                        "type": "string",
                        "description": "Exact target element reference from the page snapshot"
                    },
                    "element": {
                        "type": "string",
                        "description": "Human-readable element description used to obtain permission to interact with the element"
                    }
                },
                "required": ["function"]
            }),
        }
    }

    fn execute(&self, params: Value) -> McpResult<Value> {
        let st = self.state.lock().map_err(|e| format!("Lock: {e}"))?;

        if !st.running.load(Ordering::Relaxed) {
            return Ok(text_response("Server is not running.".into()));
        }

        let code = params.get("function")
            .and_then(|v| v.as_str())
            .ok_or("Missing 'function' parameter")?;

        let request_id = format!("eval_{}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis());

        // Send eval request to browser: "eval:{id}:{code}"
        let msg = format!("eval:{}:{}", request_id, code);
        let tx = match st.reload_tx {
            Some(ref tx) => tx.clone(),
            None => return Ok(text_response("No reload channel available.".into())),
        };
        match tx.send(msg) {
            Ok(0) | Err(_) => {
                return Ok(text_response("No browsers connected to evaluate code.".into()));
            }
            _ => {}
        }

        // Get eval_rx and rt_handle to wait for response
        let eval_rx = match st.eval_rx {
            Some(ref rx) => rx.clone(),
            None => return Ok(text_response("Eval channel not available.".into())),
        };
        let rt_handle = st.rt_handle.clone();
        let rid = request_id.clone();

        // Drop the lock before blocking wait
        drop(st);

        // Wait for the browser response with a timeout
        let result = rt_handle.block_on(async {
            let mut rx = eval_rx.lock().await;
            tokio::time::timeout(Duration::from_secs(10), async {
                while let Some((id, data)) = rx.recv().await {
                    if id == rid {
                        return Some(data);
                    }
                }
                None
            }).await
        });

        match result {
            Ok(Some(json_data)) => {
                // Try to parse as JSON first
                let parsed: Value = serde_json::from_str(&json_data)
                    .unwrap_or_else(|_| json!(json_data));

                // Check if it's an error response
                if let Some(err) = parsed.get("error").and_then(|v| v.as_str()) {
                    let stack = parsed.get("stack").and_then(|v| v.as_str()).unwrap_or("");
                    let result = json!({
                        "error": err,
                        "stack": stack
                    });
                    return Ok(text_response(serde_json::to_string_pretty(&result)?));
                }

                let result = json!({
                    "result": parsed
                });
                Ok(text_response(serde_json::to_string_pretty(&result)?))
            }
            Ok(None) => Ok(text_response("Eval returned no data.".into())),
            Err(_) => Ok(text_response("Eval timed out after 10s. Is a browser page open?".into())),
        }
    }
}

// ───────────────────── hotplate_screenshot ─────────────────────

struct ScreenshotTool {
    state: Arc<std::sync::Mutex<HotplateState>>,
}

impl Tool for ScreenshotTool {
    fn definition(&self) -> McpTool {
        McpTool {
            name: "hotplate_screenshot".into(),
            description: "Take a screenshot of the page from a connected browser.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "viewport": {
                        "type": "object",
                        "description": "Viewport dimensions for the screenshot.",
                        "properties": {
                            "width":  { "type": "number", "description": "Width in pixels (default: browser width)" },
                            "height": { "type": "number", "description": "Height in pixels (default: browser height)" }
                        }
                    }
                },
                "required": []
            }),
        }
    }

    fn execute(&self, params: Value) -> McpResult<Value> {
        let st = self.state.lock().map_err(|e| format!("Lock: {e}"))?;

        if !st.running.load(Ordering::Relaxed) {
            return Ok(text_response("Server is not running.".into()));
        }

        let width = params.get("viewport")
            .and_then(|v| v.get("width"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let height = params.get("viewport")
            .and_then(|v| v.get("height"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let request_id = format!("ss_{}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis());

        // Send screenshot request to browser: "screenshot:{id}:{w}x{h}"
        let msg = format!("screenshot:{}:{}x{}", request_id, width, height);
        let tx = match st.reload_tx {
            Some(ref tx) => tx.clone(),
            None => return Ok(text_response("No reload channel available.".into())),
        };
        match tx.send(msg) {
            Ok(0) | Err(_) => {
                return Ok(text_response("No browsers connected to take a screenshot.".into()));
            }
            _ => {}
        }

        // Get screenshot_rx and rt_handle to wait for response
        let screenshot_rx = match st.screenshot_rx {
            Some(ref rx) => rx.clone(),
            None => return Ok(text_response("Screenshot channel not available.".into())),
        };
        let rt_handle = st.rt_handle.clone();
        let rid = request_id.clone();

        // Drop the lock before blocking wait
        drop(st);

        // Wait for the browser response with a timeout
        let result = rt_handle.block_on(async {
            let mut rx = screenshot_rx.lock().await;
            tokio::time::timeout(Duration::from_secs(10), async {
                // Drain messages looking for our request_id
                while let Some((id, data)) = rx.recv().await {
                    if id == rid {
                        return Some(data);
                    }
                    // Not our response — skip (could be from a different request)
                }
                None
            }).await
        });

        match result {
            Ok(Some(base64_data)) if !base64_data.is_empty() => {
                Ok(json!({
                    "content": [{
                        "type": "image",
                        "data": base64_data,
                        "mimeType": "image/png"
                    }]
                }))
            }
            Ok(_) => Ok(text_response("Screenshot capture failed (empty response from browser).".into())),
            Err(_) => Ok(text_response("Screenshot timed out after 10s. Is a browser page open?".into())),
        }
    }
}

// ───────────────────── Entry point ─────────────────────

/// Run Hotplate in MCP stdio mode.
///
/// Blocks on stdin reading JSON-RPC messages.
/// The HTTP server runs in a background tokio task when started via `hotplate_start`.
pub fn run_mcp() -> McpResult<()> {
    let rt = tokio::runtime::Runtime::new()?;

    let state = Arc::new(std::sync::Mutex::new(HotplateState {
        running: Arc::new(AtomicBool::new(false)),
        reload_tx: None,
        config: None,
        rt_handle: rt.handle().clone(),
        server_handle: None,
        screenshot_rx: None,
        console_logs: None,
        network_logs: None,
        dom_rx: None,
        eval_rx: None,
    }));

    let mut server = McpServer::new();
    server.register_tool(Box::new(StatusTool     { state: state.clone() }));
    server.register_tool(Box::new(StartTool      { state: state.clone() }));
    server.register_tool(Box::new(StopTool       { state: state.clone() }));
    server.register_tool(Box::new(ReloadTool     { state: state.clone() }));
    server.register_tool(Box::new(InjectTool     { state: state.clone() }));
    server.register_tool(Box::new(ScreenshotTool { state: state.clone() }));
    server.register_tool(Box::new(ConsoleTool    { state: state.clone() }));
    server.register_tool(Box::new(NetworkTool    { state: state.clone() }));
    server.register_tool(Box::new(ServerLogsTool { state: state.clone() }));
    server.register_tool(Box::new(DomTool        { state: state.clone() }));
    server.register_tool(Box::new(EvalTool       { state: state.clone() }));

    eprintln!("[hotplate-mcp] ready — 11 tools registered, waiting for JSON-RPC on stdin…");

    server.run()
}

```

## File ./src\server.rs:
```rust
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
    /// Channel for browser → MCP eval responses (id, result_json).
    pub eval_tx: tokio::sync::mpsc::UnboundedSender<(String, String)>,
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

                // Inject, screenshot, dom_query, and eval messages are forwarded as-is to the browser
                let msg = if changed_path.starts_with("inject:") || changed_path.starts_with("screenshot:") || changed_path.starts_with("dom_query:") || changed_path.starts_with("eval:") {
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
        "eval_response" => {
            // Browser sends: {kind:"eval_response", url: id, msg: result_json}
            // Route to MCP EvalTool via mpsc channel
            let _ = state.eval_tx.send((m.url, m.msg));
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
    pub eval_tx: tokio::sync::mpsc::UnboundedSender<(String, String)>,
    pub console_logs: ConsoleLogBuffer,
    pub network_logs: NetworkLogBuffer,
}

/// Start the HTTP/HTTPS server.
///
/// If `ext` is `Some`, uses the pre-created channels (MCP mode).
/// Otherwise creates fresh ones (standalone mode).
pub async fn run(mut config: Config, ext: Option<ExternalChannels>) -> Result<()> {
    let (reload_tx, screenshot_tx, dom_tx, eval_tx, console_logs, network_logs) = match ext {
        Some(e) => (e.reload_tx, e.screenshot_tx, e.dom_tx, e.eval_tx, e.console_logs, e.network_logs),
        None => {
            let (rtx, _) = broadcast::channel::<String>(16);
            let (stx, _) = tokio::sync::mpsc::unbounded_channel::<(String, String)>();
            let (dtx, _) = tokio::sync::mpsc::unbounded_channel::<(String, String)>();
            let (etx, _) = tokio::sync::mpsc::unbounded_channel::<(String, String)>();
            let clogs = Arc::new(std::sync::Mutex::new(Vec::new()));
            let nlogs = Arc::new(std::sync::Mutex::new(Vec::new()));
            (rtx, stx, dtx, etx, clogs, nlogs)
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
        eval_tx,
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

```

## File ./src\watcher.rs:
```rust
//! File system watcher — debounced, filtered, broadcasts reload events.

use anyhow::Result;
use globset::{Glob, GlobSet, GlobSetBuilder};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

use crate::events::{EventData, EventLogger};

/// Directories/files to always ignore.
const IGNORE_DIRS: &[&str] = &[".git", "node_modules", "target", "__pycache__", ".venv"];
const IGNORE_EXTS: &[&str] = &["pyc", "pyo", "swp", "swo", "tmp"];

/// Default file extensions to watch (UI-related files).
/// Only files with these extensions trigger a reload.
/// Users can override this with `--watch-ext` or `hotplate.watchExtensions`.
pub const DEFAULT_WATCH_EXTS: &[&str] = &[
    "html", "htm", "css", "scss", "sass", "less",
    "js", "jsx", "ts", "tsx", "mjs", "cjs",
    "json", "svg", "png", "jpg", "jpeg", "gif", "webp", "ico",
    "woff", "woff2", "ttf", "eot",
    "xml", "md", "txt",
];

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

fn should_ignore(
    paths: &[PathBuf],
    root: &PathBuf,
    user_globs: &Option<GlobSet>,
    watch_exts: &Option<HashSet<String>>,
) -> bool {
    paths.iter().all(|p| {
        let s = p.to_string_lossy();
        // Ignored directories
        if IGNORE_DIRS.iter().any(|d| s.contains(d)) {
            return true;
        }
        // Ignored extensions (always blocked)
        if let Some(ext) = p.extension() {
            let ext_lower = ext.to_string_lossy().to_lowercase();
            if IGNORE_EXTS.contains(&ext_lower.as_str()) {
                return true;
            }
        }
        // Watch extension whitelist — only trigger for these extensions
        if let Some(ref exts) = watch_exts {
            match p.extension() {
                Some(ext) => {
                    let ext_lower = ext.to_string_lossy().to_lowercase();
                    if !exts.contains(ext_lower.as_str()) {
                        return true; // not in whitelist → ignore
                    }
                }
                None => return true, // no extension → ignore
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
/// `watch_extensions` limits which file extensions trigger reloads (e.g. ["html", "css", "js"]).
/// If empty, the default UI-related extensions are used. Pass `["*"]` to watch all files.
pub fn spawn(
    root: PathBuf,
    reload_tx: broadcast::Sender<String>,
    ignore_patterns: &[String],
    watch_extensions: &[String],
    event_logger: EventLogger,
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

    // Build watch extensions whitelist
    let watch_exts: Option<HashSet<String>> = {
        let exts: Vec<String> = if watch_extensions.is_empty() {
            // Default: UI-related extensions
            DEFAULT_WATCH_EXTS.iter().map(|s| s.to_string()).collect()
        } else {
            watch_extensions.to_vec()
        };
        // "*" means watch all files (no filter)
        if exts.iter().any(|e| e == "*") {
            None
        } else {
            Some(exts.into_iter().map(|e| e.to_lowercase().trim_start_matches('.').to_string()).collect())
        }
    };

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
                if should_ignore(&event.paths, &watch_root, &user_globs, &watch_exts) {
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
                // Determine change type from event kind
                let change = match &event.kind {
                    EventKind::Create(_) => "create",
                    EventKind::Remove(_) => "remove",
                    _ => "modify",
                };

                // Extract file extension
                let ext = event.paths.first()
                    .and_then(|p| p.extension())
                    .map(|e| e.to_string_lossy().to_lowercase())
                    .unwrap_or_default();

                event_logger.log(EventData::FileChange {
                    path: rel_path.clone(),
                    ext,
                    change: change.to_string(),
                });
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
  "version": "0.1.3",
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
          "description": "Enable HTTPS. If cert/key are not provided, a self-signed certificate will be auto-generated in .hotplate/certs/ directory."
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

