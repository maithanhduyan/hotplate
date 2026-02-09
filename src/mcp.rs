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
        let console_logs: crate::server::ConsoleLogBuffer = Arc::new(std::sync::Mutex::new(Vec::new()));
        let network_logs: crate::server::NetworkLogBuffer = Arc::new(std::sync::Mutex::new(Vec::new()));
        st.reload_tx = Some(reload_tx.clone());
        st.screenshot_rx = Some(Arc::new(tokio::sync::Mutex::new(screenshot_rx)));
        st.dom_rx = Some(Arc::new(tokio::sync::Mutex::new(dom_rx)));
        st.console_logs = Some(console_logs.clone());
        st.network_logs = Some(network_logs.clone());

        let ext = ExternalChannels {
            reload_tx: reload_tx.clone(),
            screenshot_tx,
            dom_tx,
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

    eprintln!("[hotplate-mcp] ready — 10 tools registered, waiting for JSON-RPC on stdin…");

    server.run()
}
