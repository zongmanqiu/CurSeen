@echo off
REM ============================================================================
REM 4_build.bat - compile the release binary
REM ----------------------------------------------------------------------------
REM Runs "cargo build --release" from %MAIN_DIR% (so that main\.cargo\config.toml
REM is picked up and the intermediate output lands in <project>\temp\target,
REM never inside the source tree).
REM
REM Environment switches:
REM   CURSEEN_CHAINED=1   suppress the final "press any key" (set by full.bat)
REM   CURSEEN_CARGO_ARGS  extra args appended to the cargo invocation
REM                       (e.g. "-v")
REM
REM This step does NOT copy anything to <project>\build - that is step 6.
REM ============================================================================

setlocal enabledelayedexpansion

call "%~dp0_common.bat"
if %ERRORLEVEL% neq 0 (
    pause
    exit /b 1
)

echo ======================================
echo    CurSeen  [4/6] cargo build --release
echo ======================================
echo.

cd /d "%MAIN_DIR%"
if !ERRORLEVEL! neq 0 (
    echo   [ERROR] cannot cd to %MAIN_DIR%
    pause
    exit /b 1
)

REM CARGO_INCREMENTAL=0: incremental dirs are a frequent source of "os error 5
REM access denied" on this machine (antivirus / indexer holding a lock).
set "CARGO_INCREMENTAL=0"

echo   compiling... ^(first build of a clean checkout takes several minutes^)
cargo build --release %CURSEEN_CARGO_ARGS%
if !ERRORLEVEL! neq 0 (
    echo.
    echo   [ERROR] build failed
    pause
    exit /b 1
)

echo.
echo   built: %TARGET_DIR%\release\curseen.exe
echo.

cd /d "%SCRIPT_DIR%"
if not "!CURSEEN_CHAINED!"=="1" pause
exit /b 0
