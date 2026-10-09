@echo off
REM ============================================================================
REM 5_test.bat - run the test gate
REM ----------------------------------------------------------------------------
REM This is a GATE, not a report: a failing test makes this script return 1,
REM which stops full.bat before it deploys anything.
REM
REM Two layers are run in order:
REM   [a] cargo test --release          -> integration tests (tests/*.rs, including
REM                                        tests/docs_consistency.rs and the
REM                                        registered tests/probes/*.rs)
REM   [b] cargo test --release --lib    -> unit tests inside src/
REM
REM NOTE: never pass "--lib" alone as the whole gate - it silently skips
REM tests/*.rs, which is where docs_consistency (the doc-rot gate) lives.
REM
REM NOTE: "-- --test-threads=1" is NOT optional on this machine. The default
REM multi-threaded runner has each test binary link/execute in parallel and the
REM antivirus holds a lock on the freshly-linked binaries, which stalls the run
REM (it looks like a hang). Single-threaded finishes the whole suite in seconds.
REM See docs/踩坑记录.md ("杀软扫描新链接二进制的已知坑").
REM
REM Environment switches:
REM   CURSEEN_SKIP_TESTS=1  skip everything (quick local loop)
REM   CURSEEN_CHAINED=1     suppress the final "press any key" (set by full.bat)
REM ============================================================================

setlocal enabledelayedexpansion

call "%~dp0_common.bat"
if %ERRORLEVEL% neq 0 (
    pause
    exit /b 1
)

echo ======================================
echo    CurSeen  [5/6] test gate
echo ======================================
echo.

if "%CURSEEN_SKIP_TESTS%"=="1" (
    echo   skipped - CURSEEN_SKIP_TESTS=1
    echo.
    cd /d "%SCRIPT_DIR%"
    if not "!CURSEEN_CHAINED!"=="1" pause
    exit /b 0
)

cd /d "%MAIN_DIR%"
set "CARGO_INCREMENTAL=0"

echo [a/2] integration tests ^(tests/ + tests/probes/^)...
cargo test --release -- --test-threads=1
if !ERRORLEVEL! neq 0 (
    echo.
    echo   [ERROR] integration tests failed
    pause
    exit /b 1
)

echo.
echo [b/2] unit tests ^(--lib^)...
cargo test --release --lib -- --test-threads=1
if !ERRORLEVEL! neq 0 (
    echo.
    echo   [ERROR] unit tests failed
    pause
    exit /b 1
)

echo.
echo   all tests passed
echo.

cd /d "%SCRIPT_DIR%"
if not "!CURSEEN_CHAINED!"=="1" pause
exit /b 0
