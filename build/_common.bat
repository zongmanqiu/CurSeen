@echo off
REM ============================================================================
REM _common.bat - shared setup for the numbered build scripts (ASCII only in
REM               messages: cmd.exe cannot print UTF-8 reliably)
REM ----------------------------------------------------------------------------
REM Sourced via "call" by 1_check_env.bat / 2_download.bat / 3_patch.bat /
REM 4_build.bat / 5_test.bat / 6_deploy.bat / full.bat. Sets the path
REM variables every script needs.
REM
REM Layout reminder (we live in <project>\main\build\):
REM   %SCRIPT_DIR%   = <project>\main\build\
REM   %MAIN_DIR%     = <project>\main          <- crate root, Cargo.toml lives here
REM   %PROJECT_ROOT% = <project>               <- temp/ , build/ , .3rdParty/
REM
REM IMPORTANT: no setlocal here on purpose - the caller needs the variables.
REM
REM Build outputs:
REM   cargo intermediate -> %PROJECT_ROOT%\temp\target   (main\.cargo\config.toml)
REM   shipped exe        -> %PROJECT_ROOT%\build\curseen.exe  (step 6)
REM ============================================================================

set "SCRIPT_DIR=%~dp0"
set "MAIN_DIR=%SCRIPT_DIR%.."
set "PROJECT_ROOT=%MAIN_DIR%\.."

REM Normalize away the "..\" segments so every derived path and all log output
REM shows a clean absolute path.
for %%A in ("%PROJECT_ROOT%") do set "PROJECT_ROOT=%%~fA"
for %%A in ("%MAIN_DIR%") do set "MAIN_DIR=%%~fA"

set "TEMP_DIR=%PROJECT_ROOT%\temp"
set "TARGET_DIR=%TEMP_DIR%\target"
set "OUTPUT_DIR=%PROJECT_ROOT%\build"
set "EXE=%OUTPUT_DIR%\curseen.exe"

REM ---- third-party PATH dependencies (steps 2 / 3) --------------------------
set "THIRDPARTY=%PROJECT_ROOT%\.3rdParty"
set "PATCHES=%MAIN_DIR%\patches"
set "DEPS_FILE=%SCRIPT_DIR%deps.txt"
set "DOWNLOAD_CACHE=%TEMP_DIR%\download_cache"

REM ---- tar (bsdtar; must understand Windows drive-letter paths) -------------
REM Resolve via %SystemRoot% first, NOT PATH: when these scripts are launched
REM from Git Bash, PATH puts Git's GNU tar first, and GNU tar reads "E:\..." as
REM a remote host spec ("Cannot connect to E: resolve failed"). Windows' built-in
REM bsdtar handles drive letters natively.
set "TAR_EXE="
if defined SystemRoot if exist "%SystemRoot%\System32\tar.exe" set "TAR_EXE=%SystemRoot%\System32\tar.exe"
if not defined TAR_EXE for %%A in (tar.exe) do set "TAR_EXE=%%~$PATH:A"

REM ---- curl for downloading (step 2) ---------------------------------------
set "CURL_EXE="
if defined SystemRoot if exist "%SystemRoot%\System32\curl.exe" set "CURL_EXE=%SystemRoot%\System32\curl.exe"
if not defined CURL_EXE for %%A in (curl.exe) do set "CURL_EXE=%%~$PATH:A"

REM ---- python (step 6 needs it: deploy.py / sync_assets.py) ------------------
set "PY_EXE="
for %%A in (python.exe python3.exe) do if not defined PY_EXE for %%B in ("%%~$PATH:A") do set "PY_EXE=%%~B"
if not defined PY_EXE if exist "C:\Users\%USERNAME%\AppData\Local\Programs\Python" (
    REM last resort: a per-user install that never made it onto PATH
    for /f "delims=" %%P in ('dir /b /ad /o-n "C:\Users\%USERNAME%\AppData\Local\Programs\Python" 2^>nul') do (
        if not defined PY_EXE if exist "C:\Users\%USERNAME%\AppData\Local\Programs\Python\%%P\python.exe" (
            set "PY_EXE=C:\Users\%USERNAME%\AppData\Local\Programs\Python\%%P\python.exe"
        )
    )
)

exit /b 0
