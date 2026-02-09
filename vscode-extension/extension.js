// ⚡ Hotplate — VS Code Extension
// Thin JS wrapper that spawns the Rust binary and manages its lifecycle.
//
// Architecture:
//   extension.js (this file) → spawn → hotplate.exe (Rust binary)
//   All heavy lifting (HTTP, HTTPS, WebSocket, file watching) is in Rust.
//   This file only does: spawn, kill, pipe output, status bar, config.

const vscode = require('vscode');
const { spawn } = require('child_process');
const path = require('path');
const os = require('os');

/** @type {import('child_process').ChildProcess | null} */
let serverProcess = null;

/** @type {vscode.StatusBarItem} */
let statusBar;

/** @type {vscode.OutputChannel} */
let outputChannel;

/** @type {string | null} */
let serverUrl = null;

/** @type {string | null} File path to open in browser after server starts */
let pendingOpenFile = null;

/** @type {string | null} Track which workspace the running server belongs to */
let previousWorkspacePath = null;

// ────────────────────── Multi-root Workspace Resolution ──────────────────────

/**
 * Show a quick picker to let the user choose a workspace folder.
 * Saves the choice to `hotplate.multiRootWorkspaceName`.
 * @returns {Promise<string | undefined>} Chosen workspace name
 */
async function pickWorkspaceFolder() {
    const folders = vscode.workspace.workspaceFolders;
    if (!folders || !folders.length) return undefined;

    const names = folders.map(f => f.name);
    const chosen = await vscode.window.showQuickPick(names, {
        placeHolder: 'Choose workspace folder for Hotplate',
        ignoreFocusOut: true,
    });

    if (chosen) {
        await vscode.workspace.getConfiguration('hotplate').update(
            'multiRootWorkspaceName', chosen, false
        );
    }
    return chosen;
}

/**
 * Resolve the workspace folder path for starting the server.
 *
 * Resolution order:
 *   1. If only one workspace folder → use it directly
 *   2. If a file URI is provided (right-click) → detect its workspace folder
 *   3. If `hotplate.multiRootWorkspaceName` is set → use that folder
 *   4. Otherwise → show quick picker
 *
 * @param {string} [fileUri] - Absolute file path from context menu
 * @returns {Promise<string | null>} Workspace folder path, or null if cancelled
 */
async function resolveWorkspaceFolder(fileUri) {
    const folders = vscode.workspace.workspaceFolders;
    if (!folders || !folders.length) {
        vscode.window.showErrorMessage('Open a folder or workspace first. (File → Open Folder)');
        return null;
    }

    // 1. Single workspace — no ambiguity
    if (folders.length === 1) {
        return folders[0].uri.fsPath;
    }

    // 2. If a file/folder URI is given, detect its workspace
    if (fileUri) {
        const matched = folders.find(f => fileUri.startsWith(f.uri.fsPath));
        if (matched) {
            await vscode.workspace.getConfiguration('hotplate').update(
                'multiRootWorkspaceName', matched.name, false
            );
            return matched.uri.fsPath;
        }
    }

    // 3. Check saved preference
    const config = vscode.workspace.getConfiguration('hotplate');
    const savedName = config.get('multiRootWorkspaceName', null);
    if (savedName) {
        const target = folders.find(f => f.name === savedName);
        if (target) return target.uri.fsPath;
        // Saved name is stale — clear it
        await config.update('multiRootWorkspaceName', null, false);
    }

    // 4. Show picker
    const chosen = await pickWorkspaceFolder();
    if (!chosen) return null; // user cancelled
    const folder = folders.find(f => f.name === chosen);
    return folder ? folder.uri.fsPath : null;
}

// ────────────────────── Binary Resolution ──────────────────────

/**
 * Resolve the path to the hotplate binary.
 * Search order:
 *   1. bin/ folder in extension directory (packaged extension)
 *   2. Workspace hotplate/target/release/ (dev mode)
 *   3. System PATH
 */
function getBinaryPath(context) {
    const platform = os.platform();   // win32, linux, darwin
    const arch = os.arch();           // x64, arm64
    const ext = platform === 'win32' ? '.exe' : '';

    // 1. Bundled binary (published extension)
    const bundled = path.join(context.extensionPath, 'bin', `hotplate-${platform}-${arch}${ext}`);
    if (require('fs').existsSync(bundled)) {
        return bundled;
    }

    // 2. Platform-generic bundled binary (single-platform dev build)
    const bundledSimple = path.join(context.extensionPath, 'bin', `hotplate${ext}`);
    if (require('fs').existsSync(bundledSimple)) {
        return bundledSimple;
    }

    // 3. Workspace dev build (hotplate/target/release/)
    const workspaceFolder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    if (workspaceFolder) {
        const devBuild = path.join(workspaceFolder, 'hotplate', 'target', 'release', `hotplate${ext}`);
        if (require('fs').existsSync(devBuild)) {
            return devBuild;
        }
    }

    // 4. Fall back to PATH
    return `hotplate${ext}`;
}

// ────────────────────── Server Lifecycle ──────────────────────

/**
 * Build CLI arguments from VS Code configuration.
 * @param {string} workspacePath
 * @param {string} [folderRoot] - Explicit root from context menu
 * @returns {string[]}
 */
function buildArgs(workspacePath, folderRoot) {
    const config = vscode.workspace.getConfiguration('hotplate');

    const args = [
        '--workspace', workspacePath,
        '--host', config.get('host', '0.0.0.0'),
        '--port', String(config.get('port', 5500)),
    ];

    // Root directory
    const root = folderRoot || config.get('root', '');
    if (root) {
        args.push('--root', root);
    }

    // HTTPS
    const httpsEnable = config.get('https.enable', false);
    const cert = config.get('https.cert', '');
    const key = config.get('https.key', '');
    if (cert && key) {
        args.push('--cert', cert, '--key', key);
    } else if (httpsEnable) {
        // Auto-generate self-signed cert via --https flag
        args.push('--https');
    }

    // Live reload
    if (!config.get('liveReload', true)) {
        args.push('--no-reload');
    }

    // Full reload (disable CSS-only hot swap)
    if (config.get('fullReload', false)) {
        args.push('--full-reload');
    }

    // Ignore patterns
    const ignoreFiles = config.get('ignoreFiles', []);
    for (const pattern of ignoreFiles) {
        args.push('--ignore', pattern);
    }

    // SPA fallback file
    const spaFile = config.get('file', '');
    if (spaFile) {
        args.push('--file', spaFile);
    }

    // Proxy
    const proxy = config.get('proxy', {});
    if (proxy && proxy.enable && proxy.baseUri && proxy.proxyUri) {
        args.push('--proxy-base', proxy.baseUri, '--proxy-target', proxy.proxyUri);
    }

    // Custom headers
    const headers = config.get('headers', {});
    if (headers && typeof headers === 'object') {
        for (const [key, value] of Object.entries(headers)) {
            args.push('--header', `${key}: ${value}`);
        }
    }

    // Mount directories
    const mounts = config.get('mount', []);
    if (Array.isArray(mounts)) {
        for (const entry of mounts) {
            if (Array.isArray(entry) && entry.length === 2) {
                const [urlPath, fsPath] = entry;
                args.push('--mount', `${urlPath}:${fsPath}`);
            }
        }
    }

    return args;
}

/**
 * Start the hotplate server.
 * @param {vscode.ExtensionContext} context
 * @param {string} [folderRoot] - Root folder override (from context menu)
 * @param {string} [openFilePath] - File path to open in browser after server starts
 * @param {string} [fileUri] - Absolute file path (used for workspace resolution in multi-root)
 */
async function startServer(context, folderRoot, openFilePath, fileUri) {
    if (serverProcess) {
        vscode.window.showWarningMessage('Hotplate is already running. Stop it first.');
        return;
    }

    // Save all open files before starting (like Live Server does)
    await vscode.workspace.saveAll();

    const workspaceFolder = await resolveWorkspaceFolder(fileUri);
    if (!workspaceFolder) {
        return; // user cancelled or no workspace
    }

    // Guard: server was already started from a different workspace
    if (previousWorkspacePath && previousWorkspacePath !== workspaceFolder) {
        vscode.window.showErrorMessage(
            'Hotplate is already configured for a different workspace. Stop the server first.'
        );
        return;
    }
    previousWorkspacePath = workspaceFolder;

    const binaryPath = getBinaryPath(context);
    const args = buildArgs(workspaceFolder, folderRoot);
    const config = vscode.workspace.getConfiguration('hotplate');
    const port = config.get('port', 5500);
    const httpsEnabled = config.get('https.enable', false) || config.get('https.cert', '');
    const scheme = httpsEnabled ? 'https' : 'http';

    outputChannel.clear();
    outputChannel.show(true);
    outputChannel.appendLine(`[Hotplate] Starting server...`);
    outputChannel.appendLine(`[Hotplate] Binary: ${binaryPath}`);
    outputChannel.appendLine(`[Hotplate] Args: ${args.join(' ')}`);
    outputChannel.appendLine('');

    try {
        serverProcess = spawn(binaryPath, args, {
            cwd: workspaceFolder,
            env: { ...process.env },
        });
    } catch (err) {
        vscode.window.showErrorMessage(`Failed to start Hotplate: ${err.message}`);
        outputChannel.appendLine(`[Hotplate] ERROR: ${err.message}`);
        return;
    }

    serverUrl = `${scheme}://localhost:${port}`;
    pendingOpenFile = openFilePath || null;

    /** @type {number} Track the actual port the server bound to */
    let actualPort = port;

    // Pipe stdout
    serverProcess.stdout?.on('data', (data) => {
        const text = data.toString();
        outputChannel.append(text);

        // Detect port change (auto port increment)
        // Rust binary prints: "  ℹ Port 5500 was in use, switched to port 5501."
        const portSwitchMatch = text.match(/switched to port (\d+)/);
        if (portSwitchMatch) {
            const newPort = parseInt(portSwitchMatch[1], 10);
            if (newPort) {
                actualPort = newPort;
                serverUrl = `${scheme}://localhost:${newPort}`;
                updateStatusBar(true, newPort);
                vscode.window.showWarningMessage(
                    `Port ${port} was in use. Hotplate switched to port ${newPort}.`
                );
            }
        }

        // Detect "Listening on" line to auto-open browser
        if (text.includes('Listening on')) {
            // Parse the real port from the "Listening on" line as a final source of truth
            // Format: "  🚀 Listening on http://0.0.0.0:5501 ..."
            const listenMatch = text.match(/Listening on \w+:\/\/[^:]+:(\d+)/);
            if (listenMatch) {
                const listenPort = parseInt(listenMatch[1], 10);
                if (listenPort && listenPort !== actualPort) {
                    actualPort = listenPort;
                    serverUrl = `${scheme}://localhost:${listenPort}`;
                    updateStatusBar(true, listenPort);
                }
            }

            // Build the URL — if a specific file was requested, append its path
            let url = serverUrl;
            if (pendingOpenFile) {
                url = `${serverUrl}/${pendingOpenFile}`;
                pendingOpenFile = null;
            }

            if (config.get('openBrowser', true) || openFilePath) {
                const open = require('child_process');
                if (os.platform() === 'win32') {
                    open.exec(`start "" "${url}"`);
                } else if (os.platform() === 'darwin') {
                    open.exec(`open "${url}"`);
                } else {
                    open.exec(`xdg-open "${url}"`);
                }
            }
        }
    });

    // Pipe stderr
    serverProcess.stderr?.on('data', (data) => {
        outputChannel.append(data.toString());
    });

    // Handle process exit
    serverProcess.on('close', (code) => {
        outputChannel.appendLine(`\n[Hotplate] Server stopped (exit code: ${code})`);
        serverProcess = null;
        serverUrl = null;
        updateStatusBar(false);
        vscode.commands.executeCommand('setContext', 'hotplate:running', false);
    });

    serverProcess.on('error', (err) => {
        vscode.window.showErrorMessage(`Hotplate error: ${err.message}`);
        outputChannel.appendLine(`[Hotplate] ERROR: ${err.message}`);
        serverProcess = null;
        serverUrl = null;
        updateStatusBar(false);
        vscode.commands.executeCommand('setContext', 'hotplate:running', false);
    });

    // Update UI
    updateStatusBar(true, port);
    vscode.commands.executeCommand('setContext', 'hotplate:running', true);
    vscode.window.showInformationMessage(`🔥 Hotplate started on port ${port}`);
}

/**
 * Stop the hotplate server.
 */
function stopServer() {
    if (!serverProcess) {
        vscode.window.showInformationMessage('Hotplate is not running.');
        return;
    }

    outputChannel.appendLine('\n[Hotplate] Stopping server...');

    // On Windows, use taskkill for clean shutdown of child processes
    if (os.platform() === 'win32') {
        spawn('taskkill', ['/pid', String(serverProcess.pid), '/f', '/t']);
    } else {
        serverProcess.kill('SIGTERM');
    }

    serverProcess = null;
    serverUrl = null;
    previousWorkspacePath = null;
    updateStatusBar(false);
    vscode.commands.executeCommand('setContext', 'hotplate:running', false);
    vscode.window.showInformationMessage('Hotplate stopped.');
}

/**
 * Restart the hotplate server.
 * @param {vscode.ExtensionContext} context
 */
function restartServer(context) {
    if (serverProcess) {
        // Stop first, then start after a small delay
        if (os.platform() === 'win32') {
            spawn('taskkill', ['/pid', String(serverProcess.pid), '/f', '/t']);
        } else {
            serverProcess.kill('SIGTERM');
        }
        serverProcess = null;
        serverUrl = null;
        previousWorkspacePath = null;
    }

    // Small delay to ensure port is released
    setTimeout(() => startServer(context), 500);
}

// ────────────────────── Status Bar ──────────────────────

/**
 * Update the status bar item.
 * @param {boolean} running
 * @param {number} [port]
 */
function updateStatusBar(running, port) {
    if (running) {
        statusBar.text = `$(flame) Port: ${port}`;
        statusBar.tooltip = `Hotplate running on port ${port} — click to stop`;
        statusBar.command = 'hotplate.stop';
        statusBar.backgroundColor = new vscode.ThemeColor('statusBarItem.warningBackground');
    } else {
        statusBar.text = '$(flame) Go Live';
        statusBar.tooltip = 'Click to start Hotplate dev server';
        statusBar.command = 'hotplate.start';
        statusBar.backgroundColor = undefined;
    }
}

// ────────────────────── MCP Server Registration ──────────────────────

/**
 * Register Hotplate as an MCP server so AI agents (Copilot, Claude, etc.)
 * can discover it automatically — no manual mcp.json configuration needed.
 *
 * Uses the bundled binary path (same as getBinaryPath) with `--mcp` flag.
 * Works on all platforms (Windows/macOS/Linux) and all architectures.
 *
 * @param {vscode.ExtensionContext} context
 */
function registerMcpServer(context) {
    // Guard: vscode.lm.registerMcpServerDefinitionProvider may not exist
    // on older VS Code versions that don't support the MCP API.
    if (!vscode.lm || typeof vscode.lm.registerMcpServerDefinitionProvider !== 'function') {
        return;
    }

    const binaryPath = getBinaryPath(context);

    const provider = {
        provideMcpServerDefinitions(_token) {
            return [
                new vscode.McpStdioServerDefinition(
                    'Hotplate',       // label
                    binaryPath,       // command
                    ['--mcp'],        // args
                    undefined,        // env
                    context.extension.packageJSON.version // version
                ),
            ];
        },
    };

    context.subscriptions.push(
        vscode.lm.registerMcpServerDefinitionProvider('hotplate-mcp', provider)
    );
}

// ────────────────────── Activation ──────────────────────

/**
 * @param {vscode.ExtensionContext} context
 */
function activate(context) {
    // Create output channel
    outputChannel = vscode.window.createOutputChannel('Hotplate');

    // Create status bar
    statusBar = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Right, 100);
    updateStatusBar(false);

    // Respect showOnStatusbar setting
    const showOnStatusbar = vscode.workspace.getConfiguration('hotplate').get('showOnStatusbar', true);
    if (showOnStatusbar) {
        statusBar.show();
    }

    // Watch for config changes to show/hide status bar
    context.subscriptions.push(
        vscode.workspace.onDidChangeConfiguration((e) => {
            if (e.affectsConfiguration('hotplate.showOnStatusbar')) {
                const show = vscode.workspace.getConfiguration('hotplate').get('showOnStatusbar', true);
                if (show) statusBar.show();
                else statusBar.hide();
            }
        })
    );

    // Register commands
    context.subscriptions.push(
        // Start
        vscode.commands.registerCommand('hotplate.start', async (uri) => {
            // If called from explorer context menu, use the folder path
            let folderRoot;
            const fileUri = uri?.fsPath || undefined;
            if (uri && uri.fsPath) {
                const workspaceFolder = await resolveWorkspaceFolder(uri.fsPath);
                if (workspaceFolder) {
                    // Make relative to workspace
                    folderRoot = path.relative(workspaceFolder, uri.fsPath);
                }
            }
            await startServer(context, folderRoot, undefined, fileUri);
        }),

        // Stop
        vscode.commands.registerCommand('hotplate.stop', () => {
            stopServer();
        }),

        // Restart
        vscode.commands.registerCommand('hotplate.restart', () => {
            restartServer(context);
        }),

        // Open in browser
        vscode.commands.registerCommand('hotplate.openBrowser', () => {
            if (serverUrl) {
                vscode.env.openExternal(vscode.Uri.parse(serverUrl));
            } else {
                vscode.window.showWarningMessage('Hotplate is not running.');
            }
        }),

        // Open with Hotplate (right-click on HTML/XML file)
        vscode.commands.registerCommand('hotplate.openFile', async (uri) => {
            await vscode.workspace.saveAll();

            // Get the file path — from context menu URI or active editor
            let filePath;
            if (uri && uri.fsPath) {
                filePath = uri.fsPath;
            } else if (vscode.window.activeTextEditor) {
                filePath = vscode.window.activeTextEditor.document.uri.fsPath;
            }

            if (!filePath) {
                vscode.window.showWarningMessage('No file selected.');
                return;
            }

            const workspaceFolder = await resolveWorkspaceFolder(filePath);
            if (!workspaceFolder) {
                return;
            }

            // Calculate relative path from workspace root (considering configured root)
            const config = vscode.workspace.getConfiguration('hotplate');
            const configRoot = config.get('root', '');
            const serveRoot = configRoot
                ? path.join(workspaceFolder, configRoot)
                : workspaceFolder;
            const relativePath = path.relative(serveRoot, filePath).replace(/\\/g, '/');

            if (serverProcess) {
                // Server already running — just open the file URL
                const url = `${serverUrl}/${relativePath}`;
                const open = require('child_process');
                if (os.platform() === 'win32') {
                    open.exec(`start "" "${url}"`);
                } else if (os.platform() === 'darwin') {
                    open.exec(`open "${url}"`);
                } else {
                    open.exec(`xdg-open "${url}"`);
                }
            } else {
                // Start server then open the file
                startServer(context, undefined, relativePath, filePath);
            }
        }),

        // Change Workspace (multi-root)
        vscode.commands.registerCommand('hotplate.changeWorkspace', async () => {
            const chosen = await pickWorkspaceFolder();
            if (chosen) {
                vscode.window.showInformationMessage(
                    `Hotplate workspace set to '${chosen}'.`
                );
                // If server is running, stop it so the user can restart with the new workspace
                if (serverProcess) {
                    stopServer();
                    vscode.window.showInformationMessage(
                        'Server stopped. Start again to use the new workspace.'
                    );
                }
            }
        }),

        // Cleanup
        statusBar,
        outputChannel,
    );

    // Set initial context
    vscode.commands.executeCommand('setContext', 'hotplate:running', false);

    // Register MCP server for AI agent auto-discovery
    registerMcpServer(context);
}

/**
 * Cleanup on deactivation.
 */
function deactivate() {
    if (serverProcess) {
        if (os.platform() === 'win32') {
            spawn('taskkill', ['/pid', String(serverProcess.pid), '/f', '/t']);
        } else {
            serverProcess.kill('SIGTERM');
        }
        serverProcess = null;
    }
}

module.exports = { activate, deactivate };
