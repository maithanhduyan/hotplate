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
use crate::Config;

use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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
    /// Broadcast channel to trigger browser reloads.
    pub reload_tx: Option<broadcast::Sender<String>>,
    /// Current server config (set after `hotplate_start`).
    pub config: Option<Config>,
    /// Tokio runtime handle — used to spawn the HTTP server.
    pub rt_handle: tokio::runtime::Handle,
    /// Server task handle (so we can abort on `hotplate_stop`).
    pub server_handle: Option<tokio::task::JoinHandle<()>>,
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

        let reload_tx = broadcast::channel::<String>(16).0;
        st.reload_tx = Some(reload_tx.clone());

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
            if let Err(e) = crate::server::run(config).await {
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
    }));

    let mut server = McpServer::new();
    server.register_tool(Box::new(StatusTool { state: state.clone() }));
    server.register_tool(Box::new(StartTool  { state: state.clone() }));
    server.register_tool(Box::new(StopTool   { state: state.clone() }));
    server.register_tool(Box::new(ReloadTool { state: state.clone() }));

    eprintln!("[hotplate-mcp] ready — 4 tools registered, waiting for JSON-RPC on stdin…");

    server.run()
}
