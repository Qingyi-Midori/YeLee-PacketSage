<#
.SYNOPSIS
    Clean local build caches and stale packages (nothing here is source).

.DESCRIPTION
    Every path in this script is a *build output*; deleting them never loses
    work, only build time. Nothing outside the repository is touched except
    PyInstaller's own cache (%LOCALAPPDATA%\pyinstaller) and leftover
    %TEMP%\packetsage-* scratch dirs, both of which the tools recreate.

    Keep this file ASCII-only: Windows PowerShell 5.1 reads BOM-less scripts
    with the ANSI code page, and non-ASCII text can swallow a closing quote.

    Switches (no switch = -Desktop, the cheapest cache to lose):

      -Engine       target\                          engine (cargo) build cache
      -Desktop      desktop\src-tauri\target\, desktop\dist\
      -Agent        dist\agent-sidecar\, build\, %LOCALAPPDATA%\pyinstaller
      -Artifacts    old installers in bundle\nsis + dist\windows, %TEMP%\packetsage-*
      -NodeModules  desktop\node_modules\            (next build needs npm install)
      -All          all of the above

    -WhatIf is supported (prints what would go, deletes nothing).

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts/clean.ps1 -All
    powershell -ExecutionPolicy Bypass -File scripts/clean.ps1 -Artifacts -WhatIf
#>
[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [switch]$Engine,
    [switch]$Desktop,
    [switch]$Agent,
    [switch]$Artifacts,
    [switch]$NodeModules,
    [switch]$All
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot

if ($All) {
    $Engine = $true; $Desktop = $true; $Agent = $true; $Artifacts = $true
}
if (-not ($Engine -or $Desktop -or $Agent -or $Artifacts -or $NodeModules)) {
    Write-Host "nothing selected - defaulting to -Desktop (use -All for everything)" -ForegroundColor DarkGray
    $Desktop = $true
}

function Get-TreeSizeMb {
    param([string]$Path)
    $sum = (Get-ChildItem -LiteralPath $Path -Recurse -File -ErrorAction SilentlyContinue |
        Measure-Object -Property Length -Sum).Sum
    if (-not $sum) { return 0 }
    return $sum / 1MB
}

# Deleting outside the repo is only allowed for paths named here on purpose.
$script:outsideAllow = @(
    (Join-Path $env:LOCALAPPDATA "pyinstaller")
)

function Remove-Target {
    param([string]$Path)
    $full = [IO.Path]::GetFullPath($Path)
    $inRepo = $full.StartsWith($root, [StringComparison]::OrdinalIgnoreCase)
    $allowed = $script:outsideAllow | Where-Object {
        $_ -and $full.StartsWith([IO.Path]::GetFullPath($_), [StringComparison]::OrdinalIgnoreCase)
    }
    if (-not $inRepo -and -not $allowed) {
        Write-Host "  skip (not a known build path): $full" -ForegroundColor Yellow
        return
    }
    if (-not (Test-Path -LiteralPath $full)) { return }
    $mb = Get-TreeSizeMb -Path $full
    if ($PSCmdlet.ShouldProcess($full, "remove")) {
        Remove-Item -LiteralPath $full -Recurse -Force -ErrorAction SilentlyContinue
        Write-Host ("  removed {0}  ({1:N1} MB)" -f $full, $mb)
    }
}

function Remove-StaleInstallers {
    param([string]$Dir, [string]$Version)
    if (-not (Test-Path -LiteralPath $Dir)) { return }
    $keep = "*_${Version}_x64-setup.exe"
    foreach ($file in Get-ChildItem -LiteralPath $Dir -File -Filter "*_x64-setup.exe") {
        if ($file.Name -like $keep) { continue }
        if ($PSCmdlet.ShouldProcess($file.FullName, "remove stale installer")) {
            Write-Host ("  removed {0}" -f $file.FullName)
            Remove-Item -LiteralPath $file.FullName -Force
        }
    }
}

# %TEMP%\packetsage-*: only that prefix, and only directly inside the temp dir.
function Remove-Scratch {
    param([string]$Path)
    $full = [IO.Path]::GetFullPath($Path)
    $temp = [IO.Path]::GetFullPath($env:TEMP)
    $leaf = Split-Path -Leaf $full
    if ((Split-Path -Parent $full) -ne $temp -or $leaf -notlike "packetsage-*") {
        Write-Host "  skip (not a packetsage scratch dir): $full" -ForegroundColor Yellow
        return
    }
    $mb = Get-TreeSizeMb -Path $full
    if ($PSCmdlet.ShouldProcess($full, "remove scratch dir")) {
        Remove-Item -LiteralPath $full -Recurse -Force -ErrorAction SilentlyContinue
        Write-Host ("  removed {0}  ({1:N1} MB)" -f $full, $mb)
    }
}

function Get-CurrentVersion {
    $conf = Join-Path $root "desktop\src-tauri\tauri.conf.json"
    if (-not (Test-Path -LiteralPath $conf)) { return "" }
    # Read as UTF-8 explicitly: PowerShell 5.1's Get-Content -Raw would decode a
    # BOM-less UTF-8 file with the ANSI code page and mangle the product name.
    return ([IO.File]::ReadAllText($conf, [Text.Encoding]::UTF8) | ConvertFrom-Json).version
}

Write-Host "clean: $root" -ForegroundColor Cyan

if ($Desktop) {
    Write-Host "[desktop] shell build cache + frontend bundle"
    Remove-Target (Join-Path $root "desktop\src-tauri\target")
    Remove-Target (Join-Path $root "desktop\dist")
}

if ($Engine) {
    Write-Host "[engine] cargo target dir"
    Remove-Target (Join-Path $root "target")
}

if ($Agent) {
    Write-Host "[agent] PyInstaller output + caches"
    Remove-Target (Join-Path $root "dist\agent-sidecar")
    Remove-Target (Join-Path $root "build")
    Remove-Target (Join-Path $env:LOCALAPPDATA "pyinstaller")
}

if ($Artifacts) {
    $version = Get-CurrentVersion
    Write-Host "[artifacts] stale installers (keeping $version) + scratch dirs"
    Remove-StaleInstallers -Dir (Join-Path $root "desktop\src-tauri\target\release\bundle\nsis") -Version $version
    Remove-StaleInstallers -Dir (Join-Path $root "dist\windows") -Version $version
    foreach ($scratch in Get-ChildItem -LiteralPath $env:TEMP -Directory -Filter "packetsage-*" -ErrorAction SilentlyContinue) {
        Remove-Scratch $scratch.FullName
    }
}

if ($NodeModules) {
    Write-Host "[node] desktop dependent packages"
    Remove-Target (Join-Path $root "desktop\node_modules")
}

Write-Host "done." -ForegroundColor Green
