Optimizing tool selection...## Gợi ý tên

| Tên | Ý nghĩa | Availability |
|-----|----------|-------------|
| **Servelite** | Serve + Lite — nhẹ, nhanh | ✅ Chưa có trên Marketplace |
| **Hotplate** | Hot reload + plate (đĩa nướng yakiniku 🔥) | ✅ Catchy, gốc yakiniku |
| **Blaze** | Lửa, tốc độ — ⚡🔥 | ⚠️ Có vài package trùng |
| **Embrs** | Embers (than hồng) — viết tắt kiểu Rust | ✅ Unique |
| **Ignite** | Bật lửa — start server nhanh | ⚠️ Có framework trùng |
| **Sear** | Nướng áp chảo — nhanh, nóng | ✅ Ngắn gọn |
| **Rekindle** | Re + kindle (nhóm lại lửa = live reload) | ✅ Ý nghĩa đẹp |
| **Devflame** | Dev + flame — lửa phát triển | ✅ Rõ mục đích |
| **Tong** | Cái kẹp nướng — serve files như kẹp thịt | ✅ Yakiniku DNA |
| **Livewire** | Live + wire — kết nối trực tiếp | ⚠️ Laravel dùng rồi |

### Top 3 đề xuất

1. **`hotplate`** — Yakiniku DNA, dễ nhớ, chưa ai dùng, liên tưởng "hot reload" tự nhiên
2. **`servelite`** — Chuyên nghiệp, mô tả đúng chức năng, dễ tìm trên Marketplace
3. **`embrs`** — Ngắn, unique, phong cách Rust (giống `tokio`, `axum`, `hyper`)

---

## Cách cài đặt vào VS Code Extension Marketplace

Kiến trúc: **Rust binary + thin JS wrapper**

```
vscode-hotplate/
├── package.json            ← Extension manifest
├── extension.js            ← ~100 dòng JS — spawn binary
├── bin/                    ← Pre-built Rust binaries
│   ├── hotplate-win-x64.exe
│   ├── hotplate-linux-x64
│   ├── hotplate-linux-arm64
│   └── hotplate-darwin-arm64
├── media/
│   └── icon.png
└── README.md
```

### Bước 1 — `package.json` (Extension Manifest)

```jsonc
{
  "name": "hotplate",
  "displayName": "Hotplate — Live Server",
  "description": "⚡ Fast HTTPS live-reload dev server powered by Rust",
  "version": "0.1.0",
  "publisher": "yakiniku",
  "engines": { "vscode": "^1.85.0" },
  "categories": ["Other"],
  "activationEvents": ["onStartupFinished"],
  "main": "./extension.js",

  "contributes": {
    "commands": [
      { "command": "hotplate.start", "title": "Hotplate: Start Server" },
      { "command": "hotplate.stop",  "title": "Hotplate: Stop Server" }
    ],
    "configuration": {
      "title": "Hotplate",
      "properties": {
        "hotplate.port":  { "type": "number",  "default": 5500 },
        "hotplate.root":  { "type": "string",  "default": "" },
        "hotplate.https.enable": { "type": "boolean", "default": false },
        "hotplate.https.cert":   { "type": "string",  "default": "" },
        "hotplate.https.key":    { "type": "string",  "default": "" }
      }
    }
  }
}
```

### Bước 2 — `extension.js` (~100 dòng)

```javascript
const vscode = require('vscode');
const { spawn } = require('child_process');
const path = require('path');
const os = require('os');

let serverProcess = null;
let statusBar = null;

function getBinaryPath() {
    const platform = os.platform();  // win32, linux, darwin
    const arch = os.arch();          // x64, arm64
    const ext = platform === 'win32' ? '.exe' : '';
    return path.join(__dirname, 'bin', `hotplate-${platform}-${arch}${ext}`);
}

function activate(context) {
    // Status bar
    statusBar = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Right, 100);
    statusBar.text = '$(flame) Go Live';
    statusBar.command = 'hotplate.start';
    statusBar.show();

    // Start command
    context.subscriptions.push(
        vscode.commands.registerCommand('hotplate.start', () => {
            if (serverProcess) { vscode.window.showWarningMessage('Already running'); return; }

            const workspace = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
            if (!workspace) return;

            const config = vscode.workspace.getConfiguration('hotplate');
            const args = ['--workspace', workspace, '--port', String(config.get('port', 5500))];

            if (config.get('root'))         args.push('--root', config.get('root'));
            if (config.get('https.cert'))   args.push('--cert', config.get('https.cert'));
            if (config.get('https.key'))    args.push('--key', config.get('https.key'));

            serverProcess = spawn(getBinaryPath(), args);

            const output = vscode.window.createOutputChannel('Hotplate');
            serverProcess.stdout.on('data', d => output.append(d.toString()));
            serverProcess.stderr.on('data', d => output.append(d.toString()));
            serverProcess.on('close', () => { serverProcess = null; statusBar.text = '$(flame) Go Live'; });

            statusBar.text = '$(flame) Port: ' + config.get('port', 5500);
            vscode.window.showInformationMessage(`Hotplate started on port ${config.get('port', 5500)}`);
        }),

        vscode.commands.registerCommand('hotplate.stop', () => {
            serverProcess?.kill();
            serverProcess = null;
            statusBar.text = '$(flame) Go Live';
        })
    );
}

function deactivate() { serverProcess?.kill(); }
module.exports = { activate, deactivate };
```

### Bước 3 — Build binaries (CI)

```yaml
# .github/workflows/release.yml
jobs:
  build:
    strategy:
      matrix:
        include:
          - os: windows-latest
            target: x86_64-pc-windows-msvc
            binary: hotplate-win32-x64.exe
          - os: ubuntu-latest
            target: x86_64-unknown-linux-gnu
            binary: hotplate-linux-x64
          - os: ubuntu-latest
            target: aarch64-unknown-linux-gnu
            binary: hotplate-linux-arm64
          - os: macos-latest
            target: aarch64-apple-darwin
            binary: hotplate-darwin-arm64
    steps:
      - uses: actions/checkout@v4
      - run: cargo build --release --target ${{ matrix.target }}
      - run: cp target/${{ matrix.target }}/release/hotplate* bin/${{ matrix.binary }}
```

### Bước 4 — Publish

```bash
# Cài vsce (VS Code Extension packaging tool)
npm install -g @vscode/vsce

# Đóng gói thành .vsix
vsce package

# Publish lên Marketplace (cần Personal Access Token)
vsce publish
```

### Tóm tắt flow

```
┌─────────────┐   spawn    ┌──────────────────┐
│  extension.js│ ─────────→│  hotplate.exe     │
│  (~100 LOC)  │           │  (Rust binary)    │
│              │           │                   │
│  • statusBar │  stdout   │  • axum server    │
│  • commands  │◄──────────│  • file watcher   │
│  • config UI │           │  • WebSocket      │
└─────────────┘           │  • HTTPS/rustls   │
                           └──────────────────┘
```

Extension JS chỉ làm 3 việc: **spawn**, **kill**, **pipe output**. Toàn bộ logic phức tạp nằm trong Rust binary — dễ test, dễ cập nhật, chạy độc lập ngoài VS Code.
