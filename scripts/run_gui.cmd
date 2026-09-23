@echo off
rem ---------------------------------------------------------------------------
rem Streamlit 原型启动器（双击可用；原型已冻结为参考实现，不是交付物）：
rem print the title, pick the engine, start Streamlit, keep the window open.
rem
rem   double-click                          -> title + Streamlit on :8501
rem   scripts\run_gui.cmd --server.port 8600 -> extra args go to `streamlit run`
rem
rem ASCII-only on purpose: it must run under any code page.
rem ---------------------------------------------------------------------------
setlocal EnableExtensions
chcp 65001 >nul 2>&1
title PacketSage GUI

set "ROOT=%~dp0.."
for %%I in ("%ROOT%") do set "ROOT=%%~fI"
cd /d "%ROOT%"

rem --- the project title (same art the binary embeds) -----------------------
set "TITLE=%ROOT%\crates\packetsage-cli\src\banner.txt"
if not exist "%TITLE%" set "TITLE="
if defined TITLE (
    type "%TITLE%"
    echo.
)

rem --- python: prefer the interpreter the agent was installed into ---------
set "PY="
for /f "delims=" %%P in ('where python 2^>nul') do if not defined PY set "PY=%%P"
rem Quote it: the interpreter path contains spaces ("C:\Users\...\Python\").
if defined PY set "PY_CMD="%PY%""
if not defined PY (
    py -3 --version >nul 2>&1
    if not errorlevel 1 set "PY_CMD=py -3"
)
if not defined PY_CMD (
    echo No Python found on PATH. Install it, then: python -m pip install -e agent
    echo.
    pause
    exit /b 3
)

rem --- the GUI needs Streamlit (agent extra `gui`) --------------------------
%PY_CMD% -c "import streamlit, packetsage_agent" >nul 2>&1
if errorlevel 1 (
    echo This interpreter lacks Streamlit or the agent package:
    echo   %PY_CMD%
    echo.
    echo   %PY_CMD% -m pip install -e "agent[gui]"
    echo.
    pause
    exit /b 3
)

rem --- the engine: same search order the CLI launcher uses ------------------
set "CLI="
set "VIA="
if not defined CLI if exist "%ROOT%\install-test\source\bin\packetsage.exe" (set "CLI=%ROOT%\install-test\source\bin\packetsage.exe" & set "VIA=source install")
if not defined CLI if exist "%ROOT%\target\release\packetsage.exe" (set "CLI=%ROOT%\target\release\packetsage.exe" & set "VIA=cargo build --release")
if not defined CLI if exist "%ROOT%\target\debug\packetsage.exe" (set "CLI=%ROOT%\target\debug\packetsage.exe" & set "VIA=cargo build")
if not defined CLI for %%P in (packetsage.exe) do if exist "%%~$PATH:P" (set "CLI=%%~$PATH:P" & set "VIA=PATH")

echo Working directory: %ROOT%
if defined CLI (
    echo Engine:            %CLI%
    echo Via:               %VIA%
    rem The GUI spawns `packetsage serve`; point it at the build we just found.
    set "PACKETSAGE_ENGINE=%CLI%"
) else (
    echo Engine:            not found - the GUI will offer the build command instead.
    echo                    powershell -ExecutionPolicy Bypass -File scripts\build.ps1 -CargoArgs --release
)
echo GUI:               http://localhost:8501  (override with extra args, e.g. --server.port 8600)
echo.
echo Stop the server with Ctrl-C in this window.
echo ---------------------------------------------------------------------------

%PY_CMD% -m streamlit run "%ROOT%\gui\app.py" --server.port 8501 --server.headless false %*
set "CODE=%ERRORLEVEL%"
echo.
echo [exit %CODE%]
pause
exit /b %CODE%
