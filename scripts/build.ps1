<#
.SYNOPSIS
  One-shot build helper for Windows/GNU hosts (dlltool + spaced-path workarounds).

.DESCRIPTION
  rustup's x86_64-pc-windows-gnu toolchain ships dlltool/ld but no assembler, and a
  system MinGW installed under a path with spaces makes gcc hand an unquoted
  sysroot to ld ("cannot find C:/Users/..."). This script:

    1. locates cargo (USERPROFILE\.cargo\bin and friends);
    2. locates a MinGW bin directory that has both as.exe and dlltool.exe;
    3. uses rustup's own self-contained gcc driver as the linker;
    4. runs cargo build, passing extra arguments through.

  Keep this file ASCII-only: Windows PowerShell 5.1 reads BOM-less scripts with the
  ANSI code page, and non-ASCII text can swallow the closing quote of a string.

.PARAMETER Command
  Cargo sub-command to run: build (default), test, clippy, check, run, install ...

.PARAMETER CargoArgs
  Extra arguments for cargo. Multiple flags go in as a comma list
  (`-CargoArgs --release,-p,packetsage-cli`) or as repeated elements.
  Note: the parameter is *not* named `Args` on purpose - `$Args` is a PowerShell
  automatic variable and shadowing it silently swallows the values.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts/build.ps1
  powershell -ExecutionPolicy Bypass -File scripts/build.ps1 -CargoArgs --release
  powershell -ExecutionPolicy Bypass -File scripts/build.ps1 -CargoArgs --release,-p,packetsage-cli
  powershell -ExecutionPolicy Bypass -File scripts/build.ps1 -Command test
  powershell -ExecutionPolicy Bypass -File scripts/build.ps1 -Command clippy -CargoArgs -p,packetsage-cli
#>
param(
    [string]$Command = "build",
    [string[]]$CargoArgs = @()
)

$ErrorActionPreference = "Stop"

# A quoted `-CargoArgs '--release,-p,…'` arrives as one element: split
# comma-joined *flag* strings so both spellings do what the user means.
$CargoArgs = @(
    $CargoArgs | ForEach-Object {
        if ($_ -like "-*" -and $_ -match ",") { $_ -split ',' } else { $_ }
    } | Where-Object { $_ }
)

function Find-Cargo {
    # Built defensively: $env:CARGO_HOME is often unset, and an unset variable
    # inside @( ... ) would make Join-Path bind a null Path and abort the script.
    $candidates = @()
    if ($env:USERPROFILE) { $candidates += Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe" }
    if ($env:CARGO_HOME) { $candidates += Join-Path $env:CARGO_HOME "bin\cargo.exe" }
    foreach ($candidate in $candidates) {
        if ($candidate -and (Test-Path $candidate)) { return $candidate }
    }
    $onPath = Get-Command cargo -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }
    throw "cargo not found; install Rust first (https://rustup.rs)"
}

function Find-MingwBin {
    $roots = @("C:\mingw64", "C:\msys64\mingw64", "C:\MinGW", "C:\tools\mingw64")
    if ($env:LOCALAPPDATA) { $roots += Join-Path $env:LOCALAPPDATA "Programs\mingw64" }
    if ($env:ProgramFiles) { $roots += Join-Path $env:ProgramFiles "mingw64" }
    foreach ($root in $roots) {
        $bin = Join-Path $root "bin"
        if ((Test-Path (Join-Path $bin "as.exe")) -and (Test-Path (Join-Path $bin "dlltool.exe"))) {
            return $bin
        }
    }
    return $null
}

function Find-SelfContainedGcc {
    $rustup = Join-Path $env:USERPROFILE ".rustup\toolchains"
    if (-not (Test-Path $rustup)) { return $null }
    $found = Get-ChildItem $rustup -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -like "*windows-gnu*" } |
        Sort-Object Name -Descending |
        ForEach-Object {
            Join-Path $_.FullName "lib\rustlib\x86_64-pc-windows-gnu\bin\self-contained\x86_64-w64-mingw32-gcc.exe"
        } |
        Where-Object { Test-Path $_ } |
        Select-Object -First 1
    return $found
}

$cargo = Find-Cargo
$mingwBin = Find-MingwBin
$selfGcc = Find-SelfContainedGcc

if ($mingwBin) {
    $env:Path = "$mingwBin;" + $env:Path
    Write-Host "build.ps1: MinGW bin = $mingwBin"
} else {
    Write-Host "build.ps1: no MinGW with as.exe found; install MinGW-w64 if dlltool fails"
}

if ($selfGcc) {
    $env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = $selfGcc
    Write-Host "build.ps1: linker = $selfGcc"
}

$effective = @($Command) + $CargoArgs
Write-Host ("build.ps1: " + $cargo + " " + ($effective -join " "))
& $cargo @effective
exit $LASTEXITCODE
