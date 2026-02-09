//! ⚡ hotplate — Fast HTTPS live-reload dev server
//!
//! Usage:
//!   hotplate --root ./apps --port 5500
//!   hotplate --https                   # auto-generates self-signed cert in .hotplate/certs/
//!   hotplate --root ./apps --cert .hotplate/certs/server.crt --key .hotplate/certs/server.key
//!   hotplate                          # auto-reads .vscode/settings.json

mod events;
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

    /// File extensions to watch for live reload (can be repeated, e.g. --watch-ext html --watch-ext css)
    /// Defaults to UI-related extensions (html, css, js, ts, etc.). Use "*" to watch all files.
    #[arg(long = "watch-ext")]
    watch_extensions: Vec<String>,

    /// Disable event logging (no .hotplate/logs/events-*.jsonl files)
    #[arg(long, default_value_t = false)]
    no_event_log: bool,
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
fn generate_self_signed_cert(workspace: &Path) -> Result<(PathBuf, PathBuf)> {
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
