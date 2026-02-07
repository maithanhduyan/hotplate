# 🍖 Hotplate — Live Server

**⚡ Fast HTTPS live-reload dev server powered by Rust.**

Zero config. HTTPS out of the box. LAN access for mobile testing.

---

## Features

| Feature | Description |
|---------|------------|
| ⚡ **Blazing Fast** | Rust binary — starts in <50ms, ~5MB |
| 🔒 **HTTPS** | Built-in rustls — no OpenSSL needed |
| 🔄 **Live Reload** | OS-native file watcher, WebSocket push |
| 🌐 **LAN Access** | Auto-detects local IP, works on mobile |
| 📂 **Zero Config** | Reads `.vscode/settings.json` automatically |
| 🎛️ **Context Menu** | Right-click folder → Start Server |

## Quick Start

1. Install from Marketplace
2. Open a project folder
3. Click **`$(flame) Go Live`** in the status bar
4. Done! Browser opens automatically.

Or use Command Palette: `Ctrl+Shift+P` → `Hotplate: Start Server`

## Commands

| Command | Keybinding | Description |
|---------|-----------|-------------|
| `Hotplate: Start Server` | `Alt+L Alt+O` | Start the dev server |
| `Hotplate: Stop Server` | `Alt+L Alt+C` | Stop the dev server |
| `Hotplate: Restart Server` | — | Restart the dev server |
| `Hotplate: Open in Browser` | — | Open server URL in browser |

## Settings

| Setting | Default | Description |
|---------|---------|-------------|
| `hotplate.port` | `5500` | Port number |
| `hotplate.root` | `""` | Root directory (relative to workspace) |
| `hotplate.host` | `0.0.0.0` | Bind host (`0.0.0.0` = LAN, `127.0.0.1` = local) |
| `hotplate.https.cert` | `""` | TLS certificate path (PEM) |
| `hotplate.https.key` | `""` | TLS private key path (PEM) |
| `hotplate.liveReload` | `true` | Enable live reload |
| `hotplate.openBrowser` | `true` | Auto-open browser on start |

### HTTPS Example

```jsonc
// .vscode/settings.json
{
  "hotplate.port": 5500,
  "hotplate.root": "apps",
  "hotplate.https.cert": ".cert/server.crt",
  "hotplate.https.key": ".cert/server.key"
}
```

## How It Works

```
┌─────────────┐   spawn    ┌──────────────────┐
│  extension.js│ ─────────→│  hotplate.exe     │
│  (~250 LOC)  │           │  (Rust binary)    │
│              │           │                   │
│  • statusBar │  stdout   │  • axum server    │
│  • commands  │◄──────────│  • file watcher   │
│  • config UI │           │  • WebSocket      │
└─────────────┘           │  • HTTPS/rustls   │
                           └──────────────────┘
```

The extension is a thin JavaScript wrapper. All heavy lifting runs in a native Rust binary:

- **Static file serving** — tower-http ServeDir
- **HTTPS** — rustls (no OpenSSL dependency)
- **Live reload** — OS-native file watcher (notify crate) + WebSocket
- **HTML injection** — auto-injects reload script before `</body>`

## Compatibility

Works with existing `liveServer.settings.*` in `.vscode/settings.json` — drop-in replacement for Live Server extension.

## Requirements

- VS Code ≥ 1.85.0
- No additional runtime needed (self-contained Rust binary)

## License

MIT
