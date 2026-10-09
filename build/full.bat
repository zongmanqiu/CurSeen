@echo off
REM ============================================================================
REM full.bat - the one script to run (double-click this on a fresh clone)
REM ----------------------------------------------------------------------------
REM Runs the whole pipeline in order, stopping at the first failure:
REM
REM   [1/6] 1_check_env.bat   verify the toolchain (read-only)
REM   [2/6] 2_download.bat    fetch .3rdParty/ deps listed in deps.txt
REM   [3/6] 3_patch.bat       copy main/patches/ over .3rdParty/ (one way)
REM   [4/6] 4_build.bat       cargo build --release
REM   [5/6] 5_test.bat        test gate - FAILING TESTS STOP THE PIPELINE
REM   [6/6] 6_deploy.bat      copy exe + sync assets into <project>\build\
REM
REM Steps 2 and 3 make this runnable from a BARE checkout (no .3rdParty/ at
REM all): the upstream sources are downloaded, then the CurSeen patch series is
REM replayed onto them from main/patches/.
REM
REM Each step is also runnable on its own - handy when only one thing changed:
REM   * new machine / no toolchain  -> 1_check_env.bat
REM   * clean .3rdParty/            -> 2_download.bat  then  3_patch.bat
REM   * iterating on code           -> 4_build.bat
REM   * before committing           -> 5_test.bat
REM   * need a runnable build\      -> 6_deploy.bat
REM
REM Environment switches:
REM   CURSEEN_SKIP_TESTS=1      skip step 5 (quick local loop)
REM   CURSEEN_FORCE_DOWNLOAD=1  re-fetch every dependency in step 2
REM   CURSEEN_CARGO_ARGS        extra cargo args for step 4
REM
REM NOTE: main\build\ is SOURCE (it goes into git). The shipped binary goes to
REM <project>\build\ - one level up, outside the repo.
REM ============================================================================

setlocal enabledelayedexpansion
set "CURSEEN_CHAINED=1"

call "%~dp0_common.bat"
if %ERRORLEVEL% neq 0 (
    pause
    exit /b 1
)

echo ============================================================================
echo   CurSeen full build
echo   crate  : %MAIN_DIR%
echo   target : %TARGET_DIR%
echo   output : %EXE%
echo ============================================================================
echo.

call "%SCRIPT_DIR%1_check_env.bat"
if !ERRORLEVEL! neq 0 (
    echo.
    echo   pipeline stopped at step 1 ^(toolchain^)
    pause
    exit /b 1
)

call "%SCRIPT_DIR%2_download.bat"
if !ERRORLEVEL! neq 0 (
    echo.
    echo   pipeline stopped at step 2 ^(download^)
    pause
    exit /b 1
)

call "%SCRIPT_DIR%3_patch.bat"
if !ERRORLEVEL! neq 0 (
    echo.
    echo   pipeline stopped at step 3 ^(patch^)
    pause
    exit /b 1
)

call "%SCRIPT_DIR%4_build.bat"
if !ERRORLEVEL! neq 0 (
    echo.
    echo   pipeline stopped at step 4 ^(build^)
    pause
    exit /b 1
)

call "%SCRIPT_DIR%5_test.bat"
if !ERRORLEVEL! neq 0 (
    echo.
    echo   pipeline stopped at step 5 ^(tests^)
    pause
    exit /b 1
)

call "%SCRIPT_DIR%6_deploy.bat"
if !ERRORLEVEL! neq 0 (
    echo.
    echo   pipeline stopped at step 6 ^(deploy^)
    pause
    exit /b 1
)

echo.
echo ======================================
echo    build complete
echo ======================================
echo.
echo   %EXE%
echo.
pause
exit /b 0
