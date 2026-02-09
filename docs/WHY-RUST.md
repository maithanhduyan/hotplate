# Hotplate — Tại sao viết lại Live Server bằng Rust?

## Bối cảnh

Dự án Yakiniku sử dụng VS Code extension **Live Server** (ritwickdey.LiveServer) để serve các frontend app (table-order, kitchen, dashboard, checkin, POS) trong quá trình phát triển. Khi cần bật HTTPS để camera hoạt động trên LAN (`getUserMedia` yêu cầu Secure Context), chúng tôi phát hiện bug trong extension:

```
fs.readFileSync(httpsConfig.cert)
//              ↑ Không resolve relative path → crash khi dùng "./.cert/server.crt"
```

Extension viết bằng JavaScript (Node.js), source code nằm trong thư mục `node_modules` của VS Code extension, **không thể patch vĩnh viễn** — mỗi lần cập nhật extension sẽ mất fix.

Thay vì fork một extension JavaScript cũ (last commit 2021), chúng tôi quyết định viết lại từ đầu bằng Rust.

---

## Vấn đề với Live Server extension cũ

| Vấn đề | Chi tiết |
|--------|----------|
| **Không còn maintain** | Last commit: 2021. 900+ issues mở trên GitHub. Không ai review PR. |
| **HTTPS relative path bug** | `fs.readFileSync(cert)` không resolve đường dẫn tương đối so với workspace folder. Buộc phải dùng absolute path → không portable. |
| **Không đọc được JSONC** | VS Code settings.json cho phép comment (`//`) và trailing comma (`,}`). Extension dùng `JSON.parse` → crash với settings thực tế. |
| **Không thể tùy chỉnh sâu** | Chạy trong sandbox của VS Code extension. Không thể thêm middleware, custom header, proxy, hay WebSocket channel tùy ý. |
| **Phụ thuộc VS Code** | Chỉ chạy được trong VS Code. Không dùng được trong CI/CD, Docker, terminal thuần, hay editor khác (Neovim, Zed). |
| **Chậm với dự án lớn** | File watcher dùng `chokidar` (JS) — tốn RAM, không debounce tốt, reload cả khi thay đổi file `.pyc` hay `node_modules`. |

---

## Ưu điểm của Rust Live Server

### ⚡ Hiệu năng

| Metric | JS Live Server | Rust Live Server |
|--------|---------------|-----------------|
| Binary size | ~50MB (Node.js runtime) | ~7.5MB (static binary) |
| Startup time | ~800ms | ~10ms |
| Memory idle | ~40MB | ~3MB |
| File watcher | chokidar (JS polling) | notify (OS-native: ReadDirectoryChangesW / inotify / kqueue) |
| TLS engine | Node.js OpenSSL | rustls (memory-safe, no OpenSSL dependency) |

### 🔒 HTTPS Native

```bash
# Tự động đọc cert từ .vscode/settings.json (relative path OK)
liveserver --workspace .

# Hoặc chỉ định trực tiếp
liveserver --cert .hotplate/certs/server.crt --key .hotplate/certs/server.key
```

- Resolve đường dẫn tương đối so với workspace root
- Dùng `rustls` — không cần cài OpenSSL trên máy
- Self-signed cert hoạt động tốt cho dev

### 📄 JSONC Parser

Tự viết parser strip comment (`//`, `/* */`) và trailing comma trước khi parse JSON — tương thích 100% với VS Code settings.json:

```jsonc
{
    "[javascript]": {
        "files.encoding": "utf8",   // ← trailing comma OK
    },
    // ← line comment OK
    "liveServer.settings.https": {
        "enable": true,
        "cert": "./.hotplate/certs/server.crt",  /* ← relative path OK */
    }
}
```

### 🔄 Live Reload thông minh

- **WebSocket** tại `/__lr` — inject script tự động vào mọi HTML response
- **Debounce 150ms** — gom nhiều file save liên tiếp thành 1 reload
- **Filter thông minh** — bỏ qua `.git`, `node_modules`, `__pycache__`, `.pyc`, `.swp`
- **OS-native watcher** — dùng `ReadDirectoryChangesW` (Windows) / `inotify` (Linux) / `kqueue` (macOS)

### 🏗️ Kiến trúc module hóa

```
src/
├── main.rs      # CLI (clap) + JSONC config loader
├── server.rs    # Axum router + HTTPS/HTTP binding + WebSocket
├── watcher.rs   # File system watcher (notify) + debounce
└── inject.rs    # HTML middleware — inject livereload script
```

Mỗi module độc lập, dễ test, dễ mở rộng.

### 🚀 Zero dependency runtime

```bash
# Không cần Node.js, không cần VS Code, không cần npm
# Chỉ 1 file binary
./liveserver --root ./apps --port 5500
```

Chạy được ở mọi nơi: terminal, CI/CD, Docker, SSH remote, bất kỳ editor nào.

---

## So sánh trực quan

```
┌─────────────────────────────────────────────────────────────────┐
│                    VS Code Live Server (JS)                     │
│                                                                 │
│  VS Code ──→ Extension Host ──→ Node.js ──→ express/connect    │
│                                    │                            │
│                               chokidar (JS)                    │
│                               OpenSSL binding                  │
│                               ~50MB runtime                    │
│                               ❌ HTTPS relative path bug        │
│                               ❌ Không đọc JSONC                │
│                               ❌ Chỉ chạy trong VS Code        │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│                    Rust Live Server (ours)                       │
│                                                                 │
│  Terminal ──→ Single binary (7MB) ──→ axum (async Rust)        │
│                                         │                       │
│                                    notify (OS-native)          │
│                                    rustls (memory-safe TLS)    │
│                                    ~3MB RAM                    │
│                                    ✅ HTTPS relative path       │
│                                    ✅ JSONC parser              │
│                                    ✅ Chạy mọi nơi             │
└─────────────────────────────────────────────────────────────────┘
```
