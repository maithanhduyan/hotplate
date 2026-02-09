# Copilot Instructions — Hotplate

> Guidelines for AI agents working in this codebase.

## Project Overview

Hotplate is a **Rust-powered HTTPS live-reload dev server** with a VS Code extension and a built-in MCP (Model Context Protocol) server. It is a single-binary replacement for Live Server.

- **Language:** Rust (edition 2021)
- **Framework:** Axum 0.7 + Tokio 1 (async runtime)
- **TLS:** rustls 0.23 (no OpenSSL)
- **CLI:** clap (derive API)
- **File watcher:** notify crate (OS-native)
- **VS Code extension:** Thin JavaScript wrapper (~600 LOC) spawning the Rust binary

## Architecture

```
src/
├── main.rs        # CLI (clap derive) + JSONC config loader + entry point
├── server.rs      # Axum router, HTTPS/HTTP binding, WebSocket /__lr, proxy, MCP channels
├── mcp.rs         # MCP stdio server — 11 JSON-RPC 2.0 tools for AI agents
├── events.rs      # JSONL event logger (EventData enum, mpsc writer, session rotation)
├── watcher.rs     # File watcher (notify crate) + 150ms debounce + ignore/whitelist filter
├── inject.rs      # HTML middleware — injects livereload.js before </body>
├── livereload.js  # Browser-side WebSocket agent (reload, inject, screenshot, dom, eval, console, network)
├── jsonrpc.rs     # JSON-RPC 2.0 request/response/error types
└── welcome.html   # Default welcome page when no index.html exists

vscode-extension/
├── extension.js   # VS Code extension — spawn & manage the Rust binary
├── package.json   # Extension manifest (contributes, activationEvents)
└── bin/           # Pre-built platform binaries
```

### Key Design Patterns

1. **Single binary** — all logic lives in the Rust binary; the VS Code extension is just a thin wrapper.
2. **Channel architecture** — the server uses tokio channels for inter-component communication:
   - `broadcast::Sender<String>` (`reload_tx`) — watcher → server → all browsers + MCP tool messages
   - `mpsc::Sender` — screenshot, DOM, eval responses (browser → MCP tool)
   - `Arc<Mutex<Vec<...>>>` — console logs, network logs (browser → shared buffer)
3. **`include_str!`** — `livereload.js` is embedded at compile time via `include_str!("livereload.js")` in `inject.rs`.
4. **MCP tools** follow a consistent pattern: each tool is a struct implementing request handling, registered in `run_mcp()`.

## Critical Build Caveats

### ⚠ `include_str!` Caching

When you modify `src/livereload.js`, Cargo may NOT recompile because it doesn't track `include_str!` dependencies. You **must** run:

```bash
cargo clean -p hotplate --release
cargo build --release
```

### ⚠ Binary File Lock (MCP Process)

The MCP process locks the binary file. To deploy a new build:

1. Copy to a versioned filename: `hotplate-win32-x64-v{N}.exe`
2. Update `.vscode/mcp.json` to point to the new versioned binary
3. User manually restarts MCP server in VS Code

### ⚠ Browser Caching

When testing with Playwright, the browser may cache HTML responses. Clear cache via CDP:

```javascript
const client = await page.context().newCDPSession(page);
await client.send('Network.clearBrowserCache');
```

## MCP Tool Pattern

All 11 MCP tools follow this pattern in `src/mcp.rs`:

1. **Struct** — e.g. `DomTool`, `EvalTool` with any needed channel receivers
2. **Registration** — tool name, description, and JSON Schema for parameters
3. **Execution** — handle params → send message via `reload_tx` → wait for response on dedicated channel with timeout
4. **Browser protocol** — messages sent as `{prefix}:{id}:{payload}` (e.g. `eval:abc123:return 1+1`)
5. **Browser response** — browser sends `{type}_response` WebSocket message → server routes to mpsc channel → MCP tool receives

### Browser ↔ Server Protocol (livereload.js)

Messages from server to browser via WebSocket:
- `reload` / `css:{path}` — reload triggers
- `inject:js:{code}` / `inject:css:{code}` — inject code
- `screenshot:{id}` — request screenshot
- `dom_query:{id}:{selector}` — query DOM
- `eval:{id}:{code}` — evaluate JavaScript

Browser responses (JSON via WebSocket):
- `{ type: "screenshot_response", ... }`
- `{ type: "dom_response", ... }`
- `{ type: "eval_response", ... }`
- `{ type: "console", ... }` / `{ type: "net_request", ... }` — passive collection

## Coding Conventions

- **Error handling:** Use `anyhow::Result` for application errors, `thiserror` is NOT used
- **Logging:** Use `eprintln!("[hotplate-mcp] ...")` for MCP stderr, `println!("  ...")` for normal server output (indented with 2 spaces)
- **Emoji prefixes:** `🔥` brand, `🔒` HTTPS, `📁` paths, `↻` file changes, `✓` success, `⚠` warnings
- **Module visibility:** `pub(crate)` for internal APIs, `pub` only when truly public
- **Config merging:** CLI flags > `.vscode/settings.json` > defaults
- **JSONC support:** The config loader strips `//`, `/* */` comments and trailing commas before parsing

## Testing

- MCP tools are tested interactively via `.vscode/mcp.json` configuration
- Use Playwright for browser automation testing (navigate, interact, verify)
- Verify binary contents with `Get-FileHash` and searching for expected strings in binary

## File Relationships

- `inject.rs` depends on `livereload.js` (compile-time embed)
- `server.rs` creates all channels and `AppState`, passes `ExternalChannels` to `mcp.rs`
- `mcp.rs` owns `HotplateState` which wraps `ExternalChannels` + server handle
- `events.rs` is used by both `server.rs` (HTTP, WS events) and `watcher.rs` (file changes)
- `watcher.rs` sends changed file paths via `reload_tx` which `server.rs` broadcasts to browsers

## VS Code Extension

- Entry: `vscode-extension/extension.js`
- Spawns the binary as a child process
- Reads stdout to detect server URL, pipes stderr to Output Channel
- 6 commands, status bar button, context menu, 16 settings
- Published as `maithanhduyan.hotplate` on VS Code Marketplace
