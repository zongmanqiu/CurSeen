@echo off
REM ============================================================================
REM 1_check_env.bat - verify the build toolchain is installed (read-only)
REM ----------------------------------------------------------------------------
REM Checks, in order:
REM   cargo / rustc  - must be the MSVC host (x86_64-pc-windows-msvc), not GNU
REM   python         - step 6 (deploy.py / sync_assets.py) is a python script
REM   git            - optional: only used to stamp the git hash into the About page
REM                    (missing => recorded as "unknown", build still works)
REM   .3rdParty/     - windui + nnnoiseless are PATH dependencies and MUST be
REM                    present BEFORE cargo is invoked, or it fails with a terse
REM                    "failed to load manifest for workspace member".
REM                    They are NOT an env error here: on a clean checkout they
REM                    are simply absent, and step 2 (2_download.bat) fetches
REM                    them. A missing dir is therefore reported as a WARNING
REM                    (informational), never as a hard failure.
REM
REM This script changes NOTHING on disk. It only reports. Safe to run first on a
REM fresh machine to find out what has to be installed.
REM
REM Environment switch:
REM   CURSEEN_CHAINED=1  suppress the final "press any key" (set by full.bat)
REM ============================================================================

setlocal enabledelayedexpansion

call "%~dp0_common.bat"
if %ERRORLEVEL% neq 0 (
    pause
    exit /b 1
)

echo ======================================
echo    CurSeen  [1/6] check toolchain
echo ======================================
echo.

set "ENV_FAIL=0"

REM ---- cargo / rustc --------------------------------------------------------
where cargo >nul 2>&1
if !ERRORLEVEL! neq 0 (
    echo   [ERROR] cargo not found - install Rust from https://rustup.rs
    set "ENV_FAIL=1"
) else (
    for /f "delims=" %%v in ('cargo --version 2^>nul') do if not defined CARGO_VER set "CARGO_VER=%%v"
    echo   [OK] !CARGO_VER!

    REM The MSVC toolchain is required (windui / windows crates).
    REM NOTE: this block is deliberately at top level - a pipe inside a
    REM parenthesised block that also uses delayed expansion is a classic source
    REM of parse errors, so the value is stashed and tested below.
    set "RUST_HOST="
    for /f "delims=" %%h in ('rustc -vV 2^>nul ^| findstr /B /C:"host:"') do set "RUST_HOST=%%h"
    echo   [OK] !RUST_HOST!
)

set "RUST_HOST_OK=0"
if defined RUST_HOST if not "!RUST_HOST:pc-windows-msvc=!"=="!RUST_HOST!" set "RUST_HOST_OK=1"
if "%RUST_HOST_OK%"=="0" (
    echo   [ERROR] rust host is not MSVC - need stable-x86_64-pc-windows-msvc
    echo           run: rustup default stable-x86_64-pc-windows-msvc
    set "ENV_FAIL=1"
)

REM ---- python (only needed by step 6) --------------------------------------
if defined PY_EXE (
    echo   [OK] python   : %PY_EXE%
) else (
    echo   [WARNING] python not found on PATH - step 6 ^(deploy^) will be skipped
)

REM ---- git (optional) -------------------------------------------------------
where git >nul 2>&1
if !ERRORLEVEL! neq 0 (
    echo   [WARNING] git not found - the About page will show git hash "unknown"
) else (
    echo   [OK] git
)

REM ---- third-party PATH dependencies (informational) ------------------------
REM NOT a hard error: on a clean checkout .3rdParty/ is empty by design and
REM 2_download.bat populates it. Only warn so full.bat can run from scratch.
if exist "%PROJECT_ROOT%\.3rdParty\wind-ui-rust-0.30.0\Cargo.toml" (
    echo   [OK] .3rdParty\wind-ui-rust-0.30.0
) else (
    echo   [WARNING] missing .3rdParty\wind-ui-rust-0.30.0
    echo             step 2 ^(2_download.bat^) will fetch it
)
if exist "%PROJECT_ROOT%\.3rdParty\nnnoiseless-0.5.2\Cargo.toml" (
    echo   [OK] .3rdParty\nnnoiseless-0.5.2
) else (
    echo   [WARNING] missing .3rdParty\nnnoiseless-0.5.2
    echo             step 2 ^(2_download.bat^) will fetch it
)

echo.
echo   crate  : %MAIN_DIR%
echo   target : %TARGET_DIR%
echo   output : %EXE%
echo.

if "!ENV_FAIL!"=="1" (
    echo ======================================
    echo    toolchain incomplete - fix the [ERROR] lines above
    echo ======================================
    echo.
    cd /d "%SCRIPT_DIR%"
    if not "!CURSEEN_CHAINED!"=="1" pause
    exit /b 1
)

echo   toolchain check passed
echo.

cd /d "%SCRIPT_DIR%"
if not "!CURSEEN_CHAINED!"=="1" pause
exit /b 0
