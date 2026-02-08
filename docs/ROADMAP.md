# Roadmap — Hotplate

> Mục tiêu: Từ internal tool → VS Code extension → cộng đồng open-source

---

## Phase 1 — Core (✅ Done)

Những gì đã hoàn thành trong v0.1.0:

- [x] Static file serving (axum + tower-http ServeDir)
- [x] HTTPS với rustls (TLS certificate PEM)
- [x] Relative path cert/key (resolve từ workspace root)
- [x] JSONC parser (strip comments + trailing commas)
- [x] Auto-read `.vscode/settings.json` (liveServer.settings.*)
- [x] Live reload qua WebSocket (`/__lr` endpoint)
- [x] HTML injection middleware (inject script trước `</body>`)
- [x] File watcher OS-native (notify crate, debounce 150ms)
- [x] Ignore filter (.git, node_modules, __pycache__, .pyc)
- [x] CLI interface (clap): --host, --port, --root, --cert, --key, --workspace
- [x] LAN IP auto-detect (hiển thị Network URL trong banner)
- [x] Single binary ~7.5MB, zero runtime dependency

---

## Phase 2 — Developer Experience

Cải thiện trải nghiệm lập trình viên hàng ngày:

- [ ] **CSS Hot Reload** — inject CSS thay đổi mà không reload toàn trang
- [ ] **SPA fallback** — `--spa` flag: serve `index.html` cho mọi route 404 (React/Vue/Angular)
- [ ] **Custom headers** — `--header "X-Custom: value"` hoặc từ settings.json
- [ ] **Proxy pass** — `--proxy /api=http://localhost:8000/api` (thay CORS)
- [ ] **Open browser** — tự mở trình duyệt khi start (`--open`)
- [ ] **QR Code** — hiển thị QR code trong terminal cho mobile truy cập nhanh
- [ ] **Gzip/Brotli** — nén response tự động
- [ ] **Cache control** — `Cache-Control: no-cache` cho dev, configurable cho prod
- [ ] **Error overlay** — hiển thị lỗi build đẹp trên browser (như Vite)
- [ ] **Multi-root workspace** — serve nhiều thư mục trên cùng một port

---

## Phase 3 — VS Code Extension

Đóng gói thành VS Code extension chính thức, thay thế Live Server cũ:

- [ ] **Extension wrapper** — VS Code extension gọi Rust binary bên trong
- [ ] **Status bar** — nút Go Live / Go Offline trên thanh trạng thái
- [ ] **Settings UI** — contribute configuration schema (`contributes.configuration`)
- [ ] **Output channel** — log server output vào VS Code Output panel
- [ ] **Auto-detect workspace** — tự lấy `workspaceFolders[0]` làm workspace
- [ ] **Multi-workspace** — mỗi workspace folder có server riêng
- [ ] **Command palette** — `Live Server: Start`, `Stop`, `Restart`, `Open Browser`
- [ ] **Context menu** — Right-click HTML file → "Open with Live Server"
- [ ] **Marketplace publish** — `yakiniku.liveserver` trên VS Code Marketplace

### Kiến trúc extension

```
vscode-liveserver/
├── extension.js          # VS Code extension entry — spawn/manage Rust binary
├── package.json          # Extension manifest (contributes, activationEvents)
├── bin/
│   ├── liveserver-win.exe
│   ├── liveserver-linux
│   └── liveserver-darwin
└── media/
    └── icon.png
```

Extension chỉ là thin wrapper — toàn bộ logic nằm trong Rust binary. Khác biệt cốt lõi so với Live Server cũ (toàn bộ logic trong JS):

| | Live Server cũ | Live Server mới |
|--|----------------|-----------------|
| Logic | 100% JavaScript | 100% Rust binary |
| Extension | JS + express + chokidar | Thin wrapper — spawn binary |
| Cập nhật logic | Phải cập nhật extension | Chỉ cần thay binary |
| Chạy ngoài VS Code | ❌ | ✅ `./liveserver` |

---

## Phase 4 — MCP Server (AI-driven development)

> 🎯 **Mục tiêu lớn**: Biến live server thành AI-controllable thông qua Model Context Protocol

Tích hợp MCP (Model Context Protocol) để AI agent (Copilot, Claude, Cursor) có thể điều khiển live server:

### MCP Tools

```yaml
tools:
  - liveserver_start:
      description: Start the live server
      params: { root: string, port: number, https: boolean }

  - liveserver_stop:
      description: Stop the live server

  - liveserver_status:
      description: Get current server status
      returns: { running, port, root, connections, https }

  - liveserver_reload:
      description: Force reload all connected browsers

  - liveserver_inject:
      description: Inject custom script/CSS into all pages
      params: { code: string, type: "js" | "css" }

  - liveserver_screenshot:
      description: Take screenshot of a specific page
      params: { path: string, viewport: { width, height } }
      returns: { image: base64 }

  - liveserver_console:
      description: Get browser console logs from connected clients
      returns: { logs: [{ level, message, source, line }] }

  - liveserver_network:
      description: Get network requests from connected browsers
      returns: { requests: [{ url, method, status, duration }] }

  - liveserver_dom:
      description: Query DOM from connected browser
      params: { selector: string, page: string }
      returns: { elements: [{ tag, text, attributes }] }

  - liveserver_eval:
      description: Evaluate JavaScript in connected browser
      params: { code: string, page: string }
      returns: { result: any }
```

### Kịch bản sử dụng

```
User: "Sửa màu nền header thành đỏ và kiểm tra trên mobile"

AI Agent:
  1. liveserver_status → đang chạy port 5500
  2. Sửa file CSS
  3. liveserver_reload → browser tự reload
  4. liveserver_screenshot { viewport: {375, 812} } → xem kết quả mobile
  5. liveserver_console → kiểm tra không có lỗi JS
  6. Trả lời user kèm screenshot
```

### Kiến trúc MCP

```
┌──────────┐     stdio/SSE      ┌──────────────────┐
│ AI Agent │ ◄──────────────── │  MCP Server Layer │
│ (Claude) │                    │  (built into      │
└──────────┘                    │   liveserver)     │
                                └────────┬─────────┘
                                         │
                              ┌──────────┴──────────┐
                              │   Axum HTTP/WS      │
                              │   Server Core       │
                              ├─────────────────────┤
                              │ Static Files        │
                              │ WebSocket /__lr     │
                              │ File Watcher        │
                              │ HTML Injector       │
                              ├─────────────────────┤
                              │ Browser Connections │◄──── Browser tabs
                              │ (bidirectional WS)  │      (collect console,
                              │                     │       DOM, network)
                              └─────────────────────┘
```

Khác biệt với Playwright MCP:
- **Playwright MCP**: Điều khiển browser bên ngoài (launch Chrome, navigate)
- **Live Server MCP**: Điều khiển từ bên trong (inject code, collect data qua WebSocket đã có sẵn)
- **Kết hợp**: Playwright navigate → Live Server inject + collect → AI phân tích

---

## Phase 5 — Ecosystem & Community

Mở rộng thành hệ sinh thái cho cộng đồng:

- [ ] **Plugin system** — Rust trait-based plugins (WASM hoặc dynamic loading)
- [ ] **Neovim integration** — `:LiveServer start` command
- [ ] **Zed extension** — Native integration với Zed editor
- [ ] **GitHub Action** — `uses: maithanhduyan/hotplate@v1` cho CI preview deploy
- [ ] **Docker image** — `FROM ghcr.io/maithanhduyan/hotplate:latest`
- [ ] **Cross-platform binaries** — pre-built cho Windows/Linux/macOS (x64 + ARM64)
- [ ] **Config file** — `liveserver.toml` ngoài `.vscode/settings.json`
- [ ] **Middleware API** — cho phép viết custom middleware bằng Lua/WASM

### So sánh với alternatives

| Feature | Live Server (cũ) | Vite | Our Rust Server |
|---------|------------------|------|-----------------|
| Language | JavaScript | JavaScript | Rust |
| Binary size | ~50MB (Node) | ~80MB (Node) | ~7.5MB |
| Startup | ~800ms | ~300ms | ~10ms |
| HTTPS | ✅ (buggy) | ✅ | ✅ (rustls) |
| HMR | ❌ | ✅ (JS only) | Phase 2 (CSS) |
| SPA fallback | ❌ | ✅ | Phase 2 |
| MCP support | ❌ | ❌ | Phase 4 |
| No runtime needed | ❌ | ❌ | ✅ |
| VS Code extension | ✅ | ❌ | Phase 3 |
| Framework agnostic | ✅ | ❌ (Vite only) | ✅ |

---

## Timeline dự kiến

```
2026 Q1  ████████████ Phase 1 — Core (DONE)
2026 Q2  ████████████ Phase 2 — DX (CSS hot reload, proxy, SPA)
2026 Q3  ████████████ Phase 3 — VS Code Extension
2026 Q4  ████████████ Phase 4 — MCP Server
2027 Q1  ████████████ Phase 5 — Ecosystem
```

---

## Đóng góp

Dự án open-source. Mọi đóng góp đều được hoan nghênh:

- 🐛 Bug reports → GitHub Issues
- 💡 Feature requests → GitHub Discussions
- 🔧 Pull requests → Fork → Branch → PR
- 📖 Documentation → `docs/` folder
- 🌍 Translations → i18n support

License: MIT
