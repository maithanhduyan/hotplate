# Changelog

## 0.1.3 — MCP Server (2026-02-09)

### Added
- 🤖 **MCP Server** — Built-in Model Context Protocol server (`--mcp` flag)
- 10 MCP tools for AI-driven development:
  - `hotplate_start` / `hotplate_stop` / `hotplate_status` — server lifecycle
  - `hotplate_reload` — force-reload connected browsers
  - `hotplate_inject` — inject JS/CSS into connected pages
  - `hotplate_screenshot` — capture page screenshot (base64 PNG)
  - `hotplate_console` — get browser console logs
  - `hotplate_network` — get browser network requests with timing
  - `hotplate_server_logs` — read server-side JSONL event logs
  - `hotplate_dom` — query DOM using CSS selector
- 📊 **Event sourcing** — JSONL event log (`.hotplate/logs/`)
- 🎨 **Watch extensions** — configurable file types to watch

## 0.1.1 — DX + Extension (2026-02-07)

### Added
- 🎨 CSS hot reload without full page refresh
- 📱 SPA fallback (`--file index.html`)
- 🔀 Proxy pass (`--proxy-base /api --proxy-target http://...`)
- 📂 Mount multiple directories (`--mount "/url:./path"`)
- 🔒 Auto-generate HTTPS cert (`--https`)
- ⌨️ Custom headers (`--header "Key: Value"`)
- 📦 Full reload flag (`--full-reload`)

## 0.1.0 — Initial Release (2026-02-06)

### Added
- 🔥 Start/Stop/Restart server via command palette
- 🔒 HTTPS support (rustls — no OpenSSL needed)
- 🔄 Live reload with WebSocket
- 🌐 LAN access with auto-detected IP
- 📂 Context menu: right-click folder → Start Server
- ⌨️ Keybindings: `Alt+L Alt+O` (start), `Alt+L Alt+C` (stop)
- 📊 Status bar with port display
