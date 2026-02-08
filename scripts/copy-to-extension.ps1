<#
.SYNOPSIS
  Copy the built hotplate binary into vscode-extension/bin/ with the correct platform name.
#>
param(
    [Parameter(Mandatory = $true)]
    [string]$WorkspaceFolder
)

$ErrorActionPreference = "Stop"

# Detect platform
if ($IsWindows -or $env:OS -eq 'Windows_NT') {
    $platform = 'win32'
} elseif ($IsMacOS) {
    $platform = 'darwin'
} else {
    $platform = 'linux'
}

# Detect architecture
if ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -eq 'Arm64') {
    $arch = 'arm64'
} else {
    $arch = 'x64'
}

$ext = if ($platform -eq 'win32') { '.exe' } else { '' }

$src = Join-Path $WorkspaceFolder "target/release/hotplate$ext"
$dst = Join-Path $WorkspaceFolder "vscode-extension/bin/hotplate-$platform-$arch$ext"
$binDir = Join-Path $WorkspaceFolder "vscode-extension/bin"

# Ensure bin/ exists
New-Item -ItemType Directory -Force -Path $binDir | Out-Null

# Copy
Copy-Item $src $dst -Force
Write-Host "✅ Copied to $dst"
