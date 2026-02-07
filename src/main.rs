//! ⚡ hotplate — Fast HTTPS live-reload dev server
//!
//! Usage:
//!   hotplate --root ./apps --port 5500
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
    #[serde(rename = "liveServer.settings.host")]
    host: Option<String>,
    #[serde(rename = "liveServer.settings.port")]
    port: Option<u16>,
    #[serde(rename = "liveServer.settings.root")]
    root: Option<String>,
    #[serde(rename = "liveServer.settings.https")]
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

    // HTTPS: CLI --cert/--key > vscode https
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
                } else {
                    (None, None)
                }
            } else {
                (None, None)
            }
        }
    };

    // Validate cert/key files exist
    if let Some(ref c) = cert {
        anyhow::ensure!(c.exists(), "Certificate not found: {}", c.display());
    }
    if let Some(ref k) = key {
        anyhow::ensure!(k.exists(), "Private key not found: {}", k.display());
    }
    anyhow::ensure!(root.exists(), "Root directory not found: {}", root.display());

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
