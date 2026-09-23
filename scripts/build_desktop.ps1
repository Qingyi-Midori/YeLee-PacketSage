<#
.SYNOPSIS
    Build the PacketSage desktop app (Tauri shell + installer), GUI 规格 v0.2 §7.

.DESCRIPTION
    Four steps, in the order the docs require:

      1. put cargo on PATH (this host keeps it at %USERPROFILE%\.cargo\bin);
      2. build the Agent sidecar (PyInstaller --onedir) unless -SkipAgent;
      3. stage both sidecars where Tauri expects them;
      4. `npm run tauri build` → packetsage-desktop.exe + NSIS installer.

    The engine keeps its own build (GNU, `scripts/build.ps1`); this script only
    copies the already-built `target/release/packetsage.exe` into the bundle.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts/build_desktop.ps1
    powershell -ExecutionPolicy Bypass -File scripts/build_desktop.ps1 -SkipAgent -SkipInstall
#>
[CmdletBinding()]
param(
    [switch]$SkipAgent,
    [switch]$SkipInstall,
    [switch]$SkipStage
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$desktop = Join-Path $root "desktop"

function Resolve-CargoBin {
    $candidates = @(
        (Join-Path $env:USERPROFILE ".cargo\bin"),
        "C:\Program Files\Rust\bin"
    )
    foreach ($dir in $candidates) {
        if (Test-Path (Join-Path $dir "cargo.exe")) { return $dir }
    }
    return $null
}

$cargoBin = Resolve-CargoBin
if ($cargoBin) {
    $env:PATH = "$cargoBin;$env:PATH"
    Write-Host "cargo: $cargoBin"
} else {
    Write-Host "WARN: cargo not found; the Tauri CLI needs it on PATH" -ForegroundColor Yellow
}

if (-not $SkipAgent) {
    Write-Host "`n[1/4] building the Agent sidecar (PyInstaller --onedir)" -ForegroundColor Cyan
    & python (Join-Path $root "scripts\build_agent_sidecar.py")
    if ($LASTEXITCODE -ne 0) { throw "build_agent_sidecar.py failed ($LASTEXITCODE)" }
}

if (-not $SkipStage) {
    Write-Host "`n[2/4] staging sidecars for the bundle" -ForegroundColor Cyan
    & python (Join-Path $root "scripts\stage_desktop_sidecars.py")
    if ($LASTEXITCODE -ne 0) { throw "stage_desktop_sidecars.py failed ($LASTEXITCODE)" }
}

Push-Location $desktop
try {
    if (-not $SkipInstall -and -not (Test-Path (Join-Path $desktop "node_modules"))) {
        Write-Host "`n[3/4] npm install" -ForegroundColor Cyan
        npm install
        if ($LASTEXITCODE -ne 0) { throw "npm install failed ($LASTEXITCODE)" }
    } else {
        Write-Host "`n[3/4] node_modules present (skip npm install)" -ForegroundColor DarkGray
    }

    Write-Host "`n[4/4] npm run tauri build" -ForegroundColor Cyan
    npm run tauri build
    if ($LASTEXITCODE -ne 0) { throw "tauri build failed ($LASTEXITCODE)" }
} finally {
    Pop-Location
}

Write-Host "`nartifacts:" -ForegroundColor Green
Get-ChildItem -Path (Join-Path $desktop "src-tauri\target\release") -Filter "packetsage-desktop.exe" -ErrorAction SilentlyContinue |
    ForEach-Object { Write-Host ("  shell      {0} ({1:N0} MB)" -f $_.FullName, ($_.Length / 1MB)) }
Get-ChildItem -Path (Join-Path $desktop "src-tauri\target\release\bundle") -Recurse -Filter "*.exe" -ErrorAction SilentlyContinue |
    ForEach-Object { Write-Host ("  installer  {0} ({1:N0} MB)" -f $_.FullName, ($_.Length / 1MB)) }
