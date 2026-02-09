# 🔥 Hotplate

[![Deploy to GitHub Pages](https://github.com/maithanhduyan/hotplate/actions/workflows/static.yml/badge.svg)](https://github.com/maithanhduyan/hotplate/actions/workflows/static.yml)
[![Release](https://github.com/maithanhduyan/hotplate/actions/workflows/release.yml/badge.svg)](https://github.com/maithanhduyan/hotplate/actions/workflows/release.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Version](https://img.shields.io/badge/version-0.1.0-orange.svg)](https://github.com/maithanhduyan/hotplate/releases)

**⚡ Lightning-fast HTTPS live-reload dev server powered by Rust.**

A blazingly fast, single-binary replacement for [Live Server](https://marketplace.visualstudio.com/items?itemName=ritwickdey.LiveServer) — built from scratch in Rust with zero runtime dependencies.

🌐 **[Landing Page](https://maithanhduyan.github.io/hotplate/)**

---

## Why Hotplate?

Live Server (ritwickdey) hasn't been maintained since 2021 (900+ open issues). It has critical bugs — HTTPS relative path crashes, can't read JSONC comments in `settings.json`, and requires the entire Node.js runtime.

Hotplate fixes all of this with a single ~7.5MB binary:

| Metric | Live Server (JS) | Vite | 🔥 Hotplate |
|--------|------------------|------|-------------|
| Binary size | ~50MB (Node.js) | ~80MB (Node.js) | **~7.5MB** |
| Startup time | ~800ms | ~300ms | **~10ms** |
| Memory (idle) | ~40MB | ~50MB | **~3MB** |
| HTTPS | ⚠️ Buggy | ✅ | **✅ rustls** |
| JSONC support | ❌ | ✅ | **✅** |
| No runtime needed | ❌ | ❌ | **✅** |
| Framework agnostic | ✅ | ❌ | **✅** |

---

## Features

- ⚡ **Blazingly fast** — Axum + Tokio async runtime, starts in ~10ms
- 🔒 **HTTPS native** — Built-in TLS with rustls, relative cert paths just work
- 🔄 **Live reload** — WebSocket-based, auto-injected into HTML, 150ms debounce
- 🎨 **CSS hot reload** — Inject CSS changes without full page reload
- 👁️ **OS-native file watcher** — `ReadDirectoryChangesW` / `inotify` / `kqueue`
- 📄 **JSONC parser** — Reads `settings.json` with comments and trailing commas
- 🌐 **LAN auto-detect** — Shows Network URL for mobile testing
- 📦 **Single binary** — No Node.js, no npm, zero runtime dependencies
- 🎯 **Smart filtering** — Ignores `.git`, `node_modules`, `__pycache__` automatically
- 🔀 **Proxy pass** — Forward `/api` requests to backend server
- 📱 **SPA fallback** — Serve `index.html` for all 404 routes (React/Vue/Angular)
- 📂 **Mount directories** — Serve multiple directories on one server
- 🧩 **VS Code extension** — Go Live button, context menu, output channel

---

## Quick Start

### CLI Usage

```bash
# Basic — serve current directory
hotplate

# Specify root and port
hotplate --root ./apps --port 5500

# With HTTPS
hotplate --root ./apps --cert .hotplate/certs/server.crt --key .hotplate/certs/server.key

# SPA mode (React/Vue/Angular)
hotplate --root ./dist --file index.html

# With proxy (forward /api to backend)
hotplate --root ./frontend --proxy-base /api --proxy-target http://127.0.0.1:8000

# Mount extra directories
hotplate --root ./src --mount "/node_modules:./node_modules" --mount "/assets:../shared/assets"

# Custom headers
hotplate --header "X-Custom: value" --header "Cache-Control: no-cache"

# CSS-only hot swap disabled (always full reload)
hotplate --full-reload
```

### VS Code Extension

1. Install **Hotplate — Live Server** from the VS Code Marketplace
2. Click **🔥 Go Live** in the status bar — or use `Alt+L Alt+O`
3. Right-click any HTML file → **Open with Hotplate**
4. Stop with `Alt+L Alt+C`

---

## CLI Options

```
hotplate [OPTIONS]

Options:
  -p, --port <PORT>              Bind port [default: 5500]
      --host <HOST>              Bind host [default: 0.0.0.0]
  -r, --root <ROOT>              Root directory to serve
      --cert <CERT>              TLS certificate path (PEM)
      --key <KEY>                TLS private key path (PEM)
      --no-reload                Disable live reload
      --full-reload              Force full page reload (disable CSS hot swap)
  -w, --workspace <WORKSPACE>    Workspace dir (for .vscode/settings.json)
      --ignore <PATTERN>         Glob patterns to ignore (repeatable)
      --file <FILE>              SPA fallback file (e.g. "index.html")
      --proxy-base <PATH>        Proxy base URI (e.g. "/api")
      --proxy-target <URL>       Proxy target URL (e.g. "http://127.0.0.1:8000")
      --header <HEADER>          Custom header "Key: Value" (repeatable)
      --mount <MOUNT>            Mount dir "/url:./path" (repeatable)
  -h, --help                     Print help
```

---

## VS Code Settings

Hotplate reads settings from `.vscode/settings.json` (JSONC supported):

```jsonc
{
    // Server
    "hotplate.port": 5500,
    "hotplate.host": "0.0.0.0",
    "hotplate.root": "",

    // HTTPS
    "hotplate.https.cert": ".hotplate/certs/server.crt",
    "hotplate.https.key": ".hotplate/certs/server.key",

    // Live Reload
    "hotplate.liveReload": true,
    "hotplate.fullReload": false,
    "hotplate.wait": 150,
    "hotplate.ignoreFiles": [".vscode/**", "**/*.scss", "**/*.sass", "**/*.ts"],

    // SPA
    "hotplate.file": "index.html",

    // Proxy
    "hotplate.proxy": {
        "enable": true,
        "baseUri": "/api",
        "proxyUri": "http://127.0.0.1:8000",
    },

    // Custom headers
    "hotplate.headers": {
        "X-Custom-Header": "value",
        "Cache-Control": "no-cache",
    },

    // Mount extra directories
    "hotplate.mount": [
        ["/node_modules", "./node_modules"],
        ["/assets", "../shared/assets"],
    ],

    // UI
    "hotplate.openBrowser": true,
    "hotplate.showOnStatusbar": true,
}
```

---

## Architecture

```
src/
├── main.rs        # CLI (clap) + JSONC config loader
├── server.rs      # Axum router + HTTPS/HTTP + WebSocket
├── watcher.rs     # File system watcher (notify) + debounce
└── inject.rs      # HTML middleware — inject livereload script

vscode-extension/
├── extension.js   # VS Code extension — spawn & manage binary
├── package.json   # Extension manifest
└── bin/           # Pre-built Rust binaries
```

The VS Code extension is a thin wrapper — all logic lives in the Rust binary. This means:
- ✅ Can run outside VS Code (terminal, CI/CD, Docker, SSH)
- ✅ Update logic without updating the extension
- ✅ Framework and editor agnostic

**Tech Stack:** Axum · Tokio · Rustls · Notify · Tower · Clap

---

## Build from Source

```bash
# Prerequisites: Rust toolchain (rustup.rs)
git clone https://github.com/maithanhduyan/hotplate.git
cd hotplate

# Build release binary
cargo build --release

# Binary at: target/release/hotplate (or hotplate.exe on Windows)
./target/release/hotplate --root ./apps --port 5500
```

---

## Keyboard Shortcuts

| Action | Windows/Linux | macOS |
|--------|--------------|-------|
| Start Server | `Alt+L Alt+O` | `Cmd+L Cmd+O` |
| Stop Server | `Alt+L Alt+C` | `Cmd+L Cmd+C` |

---

## Roadmap

| Phase | Timeline | Status |
|-------|----------|--------|
| **Core** — Static serving, HTTPS, live reload, file watcher, CLI | 2026 Q1 | ✅ Done |
| **DX** — CSS hot reload, SPA fallback, proxy, custom headers, mount, auto-cert | 2026 Q2 | ✅ Done |
| **VS Code Extension** — Status bar, 6 commands, context menu, keybindings, settings UI | 2026 Q3 | ✅ Done |
| **MCP Server** — AI-controllable via Model Context Protocol | 2026 Q4 | 📋 Planned |
| **Ecosystem** — Plugin system, Neovim/Zed, GitHub Action, Docker | 2027 Q1 | 📋 Planned |

See [docs/ROADMAP.md](docs/ROADMAP.md) for details.

---

## Contributing

Contributions are welcome! 🎉

- 🐛 Bug reports → [GitHub Issues](https://github.com/maithanhduyan/hotplate/issues)
- 💡 Feature requests → [GitHub Discussions](https://github.com/maithanhduyan/hotplate/discussions)
- 🔧 Pull requests → Fork → Branch → PR

---

## License

[MIT](LICENSE) © [Mai Thành Duy An](https://github.com/maithanhduyan)
