# Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.1.3] — 2026-02-09

### Added — Phase 4: MCP Server (11/11 tools ✅)

Built-in MCP (Model Context Protocol) server for AI-driven development via `hotplate --mcp`.

**MCP Tools:**
- `hotplate_start` — Start the live server as a background task
- `hotplate_stop` — Stop the running server
- `hotplate_status` — Get server status (running, port, host, root, HTTPS, live_reload)
- `hotplate_reload` — Force-reload all connected browsers (with optional CSS hot-swap path)
- `hotplate_inject` — Inject JavaScript or CSS into all connected pages in real-time
- `hotplate_screenshot` — Take a screenshot from a connected browser (base64 PNG)
- `hotplate_console` — Get browser console logs (warn, error, js_error) with level/clear filters
- `hotplate_network` — Get browser network requests (url, method, status, duration) with method/status/clear filters
- `hotplate_server_logs` — Read server-side JSONL event logs (session listing, kind filter, limit)
- `hotplate_dom` — Query DOM from connected browser using CSS selector (returns tag, text, attributes, innerHTML)
- `hotplate_eval` — Evaluate JavaScript expression in connected browser (async/await, error+stack capture)

**Browser Agent (livereload.js):**
- Bidirectional WebSocket protocol for browser ↔ MCP communication
- Console interception (console.warn, console.error)
- Unhandled JS error capture (window.onerror, unhandledrejection)
- Fetch wrapper — tracks all network requests with timing (performance.now)
- DOM query handler — querySelectorAll with structured response (cap: 200 elements)
- Screenshot handler — canvas-based page capture
- Eval handler — `new Function` + async/await, structured result/error response

**Architecture:**
- JSON-RPC 2.0 over stdio (`--mcp` flag)
- Protocol version: `2024-11-05`
- Shared state via `HotplateState` (Arc<Mutex>)
- Channels: broadcast (reload_tx), mpsc (screenshot_rx, dom_rx, eval_rx), Arc<Mutex<Vec>> (console, network)
- Event sourcing: JSONL logs at `.hotplate/logs/events-{session}.jsonl` (keeps last 10 sessions)

---

## [0.1.2] — 2026-02-08

### Added — Phase 2 continued: Event Sourcing

- **Event sourcing** — JSONL event log recording all server activity:
  - File changes, reload triggers, HTTP requests, WebSocket connections
  - Browser JS errors, console logs, network errors
  - Stored in `.hotplate/logs/events-{session}.jsonl`
  - Automatic session rotation (keeps last 10 files)
  - `--no-event-log` flag to disable
- **Watch extensions** — configurable file types to watch:
  - Default: html, css, js, ts, jsx, tsx, json, md, svg, png, jpg, gif, ico, woff, woff2
  - `--watch-ext` CLI flag or `hotplate.watchExtensions` setting
  - `"*"` to watch all file types
- **Cache control** — `Cache-Control: no-cache` default for development

---

## [0.1.1] — 2026-02-07

### Added — Phase 2: Developer Experience

- **CSS hot reload** — inject CSS changes without full page reload (`css:<path>` via WebSocket)
- **SPA fallback** — `--file index.html` serves index.html for all 404 routes (React/Vue/Angular)
- **Custom headers** — `--header "Key: Value"` repeatable flag, or via settings.json
- **Proxy pass** — `--proxy-base /api --proxy-target http://localhost:8000` (replaces CORS setup)
- **Open browser** — auto-open browser on server start (VS Code extension detects stdout)
- **Mount directories** — `--mount "/url:./path"` serve multiple directories on one server
- **Auto-generate HTTPS cert** — `--https` flag auto-generates self-signed cert with rcgen
- **Full reload flag** — `--full-reload` disables CSS hot swap, always full page reload

### Added — Phase 3: VS Code Extension

- **Extension wrapper** — VS Code extension (~600 LOC) spawning Rust binary
- **Status bar** — `$(flame) Go Live` / `Port: XXXX` status bar button
- **Settings UI** — 16 settings in `contributes.configuration`
- **Output channel** — `Hotplate` output channel, pipes stdout/stderr from binary
- **Auto-detect workspace** — reads `workspaceFolders[0]`, multi-root QuickPick support
- **Command palette** — 6 commands: Start, Stop, Restart, Open Browser, Open File, Change Workspace
- **Context menu** — Right-click HTML → "Open with Hotplate", Right-click folder → "Start Server"
- **Keybindings** — `Alt+L Alt+O` (start), `Alt+L Alt+C` (stop) with `when` clause
- **Marketplace publish** — `maithanhduyan.hotplate` on VS Code Marketplace

---

## [0.1.0] — 2026-02-06

### Added — Phase 1: Core

- **Static file serving** — Axum + tower-http ServeDir
- **HTTPS** — Built-in TLS with rustls (PEM cert/key)
- **Relative path cert/key** — resolve from workspace root
- **JSONC parser** — strip comments + trailing commas from settings.json
- **Auto-read settings** — `.vscode/settings.json` (hotplate.* namespace)
- **Live reload** — WebSocket-based at `/__lr` endpoint, auto-injected into HTML
- **HTML injection middleware** — inject livereload script before `</body>`
- **File watcher** — OS-native (notify crate), debounced at 150ms
- **Ignore filter** — `.git`, `node_modules`, `__pycache__`, `.pyc` automatically excluded
- **CLI interface** — clap: `--host`, `--port`, `--root`, `--cert`, `--key`, `--workspace`
- **LAN IP auto-detect** — displays Network URL in startup banner
- **Single binary** — ~7.5MB, zero runtime dependencies

---

[0.1.3]: https://github.com/maithanhduyan/hotplate/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/maithanhduyan/hotplate/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/maithanhduyan/hotplate/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/maithanhduyan/hotplate/releases/tag/v0.1.0
