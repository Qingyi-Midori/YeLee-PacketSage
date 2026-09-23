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
    [switch]$SkipStage,
    [switch]$Clean
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$desktop = Join-Path $root "desktop"

if ($Clean) {
    Write-Host "`n[0/4] cleaning build caches (-Clean)" -ForegroundColor Cyan
    & powershell -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "clean.ps1") -Desktop -Agent -Artifacts
    if ($LASTEXITCODE -ne 0) { throw "clean.ps1 failed ($LASTEXITCODE)" }
}

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

# ---------------------------------------------------------------------------
# Tidy up (2026-09-23, user feedback): every build used to leave its installer
# behind in two places, so the folders grew one package per version. Keep only
# the version we just built, mirror it into dist\windows\ (the local delivery
# folder) and refresh that folder's README so it never describes an old build.
# ---------------------------------------------------------------------------

Write-Host "`ntidy: pruning older installers + mirroring the current one" -ForegroundColor Cyan

$confPath = Join-Path $desktop "src-tauri\tauri.conf.json"
# UTF-8 explicit: PowerShell 5.1's Get-Content -Raw decodes BOM-less UTF-8 with
# the ANSI code page and mangles the product name (which the file also holds).
$version = ([IO.File]::ReadAllText($confPath, [Text.Encoding]::UTF8) | ConvertFrom-Json).version
$bundleDir = Join-Path $desktop "src-tauri\target\release\bundle\nsis"
$delivery = Join-Path $root "dist\windows"

foreach ($dir in @($bundleDir, $delivery)) {
    if (-not (Test-Path -LiteralPath $dir)) { continue }
    foreach ($file in Get-ChildItem -LiteralPath $dir -File -Filter "*_x64-setup.exe") {
        if ($file.Name -like "*_${version}_x64-setup.exe") { continue }
        Write-Host ("  prune  {0}" -f $file.FullName) -ForegroundColor DarkGray
        Remove-Item -LiteralPath $file.FullName -Force
    }
}

$installer = Get-ChildItem -LiteralPath $bundleDir -File -Filter "*_${version}_x64-setup.exe" -ErrorAction SilentlyContinue |
    Select-Object -First 1
if ($installer) {
    New-Item -ItemType Directory -Path $delivery -Force | Out-Null
    $target = Join-Path $delivery $installer.Name
    # A fresh 44 MB exe is often still being scanned (Defender / indexer) right
    # after it lands, so the mirror is retried instead of failing the build.
    $copied = $false
    for ($attempt = 1; $attempt -le 5 -and -not $copied; $attempt++) {
        try {
            Copy-Item -LiteralPath $installer.FullName -Destination $target -Force -ErrorAction Stop
            $copied = $true
        } catch {
            if ($attempt -eq 5) { throw }
            Write-Host ("  mirror busy, retry {0}/5: {1}" -f $attempt, $_.Exception.Message) -ForegroundColor Yellow
            Start-Sleep -Seconds 2
        }
    }
    $sha = (Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash
    $head = (& git -C $root rev-parse --short HEAD)
    $dirty = if ((& git -C $root status --porcelain)) { "-dirty" } else { "" }
    $note = @"
Windows build artifacts (this directory is gitignored - local only)

$($installer.Name)
    NSIS installer (currentUser, no UAC prompt), $("{0:N0}" -f $installer.Length) bytes, built $($installer.LastWriteTime)
    SHA256 $sha
    from worktree $head$dirty
    copy of $($installer.FullName)

    install    double-click it. An older install is overwritten;
               %LOCALAPPDATA%\<app> (db / reports / logs / provider.json) is preserved.
    rebuild    powershell -ExecutionPolicy Bypass -File scripts/build_desktop.ps1
               (engine first: powershell -ExecutionPolicy Bypass -File scripts/build.ps1 -CargoArgs --release)
               add -Clean to start from an empty cache; older packages are pruned automatically.
    cleanup    powershell -ExecutionPolicy Bypass -File scripts/clean.ps1 -Artifacts
"@
    Set-Content -LiteralPath (Join-Path $delivery "README.txt") -Value $note -Encoding UTF8
    Write-Host ("  mirror {0}" -f $target) -ForegroundColor Green
}
