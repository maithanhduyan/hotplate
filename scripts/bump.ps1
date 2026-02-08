<#
.SYNOPSIS
  Bump Hotplate version in Cargo.toml + vscode-extension/package.json, then create a git tag.

.DESCRIPTION
  Usage:
    .\scripts\bump.ps1 patch    # 0.1.0 → 0.1.1
    .\scripts\bump.ps1 minor    # 0.1.0 → 0.2.0
    .\scripts\bump.ps1 major    # 0.1.0 → 1.0.0
    .\scripts\bump.ps1 1.2.3    # set exact version

  After running, just `git push --follow-tags` to trigger the release workflow.
#>

param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$BumpOrVersion
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# ── Read current version from Cargo.toml ──
$cargoContent = Get-Content "Cargo.toml" -Raw
if ($cargoContent -match 'version\s*=\s*"([^"]+)"') {
    $current = $Matches[1]
} else {
    Write-Error "Could not find version in Cargo.toml"
    exit 1
}
Write-Host "📦 Current version: $current" -ForegroundColor Cyan

# ── Calculate new version ──
$parts = $current.Split('-')[0].Split('.')
$major = [int]$parts[0]
$minor = [int]$parts[1]
$patch = [int]$parts[2]

switch ($BumpOrVersion) {
    "patch" { $newVersion = "$major.$minor.$($patch + 1)" }
    "minor" { $newVersion = "$major.$($minor + 1).0" }
    "major" { $newVersion = "$($major + 1).0.0" }
    default {
        # Treat as exact version string
        if ($BumpOrVersion -match '^\d+\.\d+\.\d+') {
            $newVersion = $BumpOrVersion
        } else {
            Write-Error "Invalid bump type or version: $BumpOrVersion (use patch/minor/major or a semver string)"
            exit 1
        }
    }
}

Write-Host "🚀 New version: $current → $newVersion" -ForegroundColor Green

# ── Check if tag exists ──
$tagExists = git tag -l "v$newVersion" 2>$null
if ($tagExists) {
    Write-Error "❌ Tag v$newVersion already exists!"
    exit 1
}

# ── Update Cargo.toml (only the [package] version, not dependency versions) ──
$cargoContent = [regex]::new('(version\s*=\s*")[^"]+(")').Replace($cargoContent, "`${1}$newVersion`${2}", 1)
Set-Content "Cargo.toml" -Value $cargoContent -NoNewline
Write-Host "✅ Cargo.toml → $newVersion"

# ── Update vscode-extension/package.json ──
Push-Location "vscode-extension"
npm version $newVersion --no-git-tag-version --allow-same-version | Out-Null
Pop-Location
Write-Host "✅ package.json → $newVersion"

# ── Git commit + tag ──
git add Cargo.toml vscode-extension/package.json
git commit -m "chore: bump version to v$newVersion"
git tag -a "v$newVersion" -m "Release v$newVersion"

Write-Host ""
Write-Host "═══════════════════════════════════════════" -ForegroundColor Yellow
Write-Host "  🔥 Version bumped to v$newVersion" -ForegroundColor Yellow
Write-Host ""
Write-Host "  Next step — push to trigger release:" -ForegroundColor White
Write-Host "    git push --follow-tags" -ForegroundColor Cyan
Write-Host "═══════════════════════════════════════════" -ForegroundColor Yellow
