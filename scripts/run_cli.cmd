@echo off
rem ---------------------------------------------------------------------------
rem PacketSage CLI launcher for people who double-click things.
rem A console program with no arguments prints its help and exits immediately,
rem which is why double-clicking packetsage.exe only flashes a window. This
rem launcher prints the ASCII title, picks the *most capable* installed build,
rem runs something useful and keeps the window open with `pause`.
rem
rem   double-click                              -> title + version + analyze, pauses
rem   scripts\run_cli.cmd doctor --no-net       -> runs that exact command, pauses
rem
rem The launcher is deliberately ASCII-only: it must run under any code page.
rem ---------------------------------------------------------------------------
setlocal EnableExtensions
chcp 65001 >nul 2>&1
title PacketSage CLI

set "ROOT=%~dp0.."
rem Canonicalise ROOT so printed paths do not contain "..".
for %%I in ("%ROOT%") do set "ROOT=%%~fI"

rem --- the project title (same art the binary embeds) -----------------------
set "TITLE=%ROOT%\crates\packetsage-cli\src\banner.txt"
if not exist "%TITLE%" set "TITLE="
if defined TITLE (
    type "%TITLE%"
    echo.
)

rem --- pick the CLI: full capability first, distribution artifact last ------
set "CLI="
set "VIA="
if not defined CLI if exist "%ROOT%\install-test\source\bin\packetsage.exe" (set "CLI=%ROOT%\install-test\source\bin\packetsage.exe" & set "VIA=source install")
if not defined CLI if exist "%ROOT%\target\release\packetsage.exe" (set "CLI=%ROOT%\target\release\packetsage.exe" & set "VIA=cargo build --release")
if not defined CLI if exist "%ROOT%\target\debug\packetsage.exe" (set "CLI=%ROOT%\target\debug\packetsage.exe" & set "VIA=cargo build")
if not defined CLI for /d %%D in ("%ROOT%\install-test\tarball\packetsage-*") do if not defined CLI if exist "%%~fD\packetsage.exe" (set "CLI=%%~fD\packetsage.exe" & set "VIA=tarball (engine only)")

if not defined CLI (
    echo No packetsage.exe found. Build or install it first:
    echo.
    echo   powershell -ExecutionPolicy Bypass -File scripts\build.ps1 -CargoArgs --release
    echo   python scripts\install_smoke.py --run-cli
    echo.
    pause
    exit /b 2
)

echo Using: %CLI%
echo Via:   %VIA%
echo.

if "%~1"=="" goto demo

rem Explicit command passed in: hand the arguments straight to the CLI.
"%CLI%" %*
set "CODE=%ERRORLEVEL%"
echo.
echo [exit %CODE%]
pause
exit /b %CODE%

:demo
rem Sample capture: prefer the one the install test copied, else the repo samples.
set "SAMPLE=%ROOT%\install-test\work\sample.pcap"
if not exist "%SAMPLE%" set "SAMPLE=%ROOT%\samples\synth-mixed.pcap"

"%CLI%" version
echo.
if exist "%SAMPLE%" (
    "%CLI%" analyze "%SAMPLE%"
) else (
    echo No sample capture found; generate one with:
    echo   python scripts\gen_traffic.py --out samples\synth-mixed.pcap --packets 600 --profile mixed
)

rem --- is the Python agent reachable? chat / --full need it ----------------
set "AGENT="
for /f "delims=" %%P in ('where packetsage-agent 2^>nul') do if not defined AGENT set "AGENT=%%P"
if not defined AGENT python -m packetsage_agent --version >nul 2>&1 && set "AGENT=python -m packetsage_agent"

echo.
echo ---------------------------------------------------------------------------
echo Useful commands (copy, paste, add your own file):
echo   "%CLI%" analyze path\to\capture.pcap
echo   "%CLI%" analyze path\to\capture.pcap --jsonl ^> events.jsonl
echo   "%CLI%" doctor --no-net
echo   "%CLI%" rules list
if defined AGENT (
    echo   "%CLI%" chat path\to\capture.pcap
    echo   "%CLI%" analyze path\to\capture.pcap --full --report reports\demo.md --db sqlite://packetsage.db
    echo   "%CLI%" analyze path\to\capture.pcap --db sqlite://packetsage.db
    echo   %AGENT% run --task-id ^<task_...^> --db sqlite://packetsage.db
    echo.
    echo Agent: %AGENT%
) else (
    echo.
    echo chat / --full need the Python agent, which is not reachable here:
    echo   python -m pip install -e agent
)

echo.
for %%I in ("%CLI%") do set "CLIDIR=%%~dpI"
echo To use the bare command `packetsage` anywhere, add this directory to PATH:
echo   %CLIDIR%
echo.
echo More: INSTALL.md section 4 (how to run), section 5 (double-click vs terminal).
echo ---------------------------------------------------------------------------
pause
exit /b 0
