@echo off
REM ============================================================================
REM 2_download.bat - download + extract the third-party PATH dependencies
REM ----------------------------------------------------------------------------
REM Reads build/deps.txt: each active line is "name=url" or "name=url|marker".
REM
REM   * archives are cached under temp/download_cache/<name>.tar.gz
REM   * they are extracted into .3rdParty/<name>/
REM   * "marker" (a path relative to .3rdParty/<name>/) is the integrity probe -
REM     a library counts as present only if that file exists, so a half-extracted
REM     directory left by an interrupted run is re-fetched instead of accepted.
REM
REM Environment switches:
REM   CURSEEN_FORCE_DOWNLOAD=1  ignore cache + presence, re-download everything
REM   CURSEEN_CHAINED=1         suppress the final "press any key" (set by full.bat)
REM
REM Run from main\build\. Plain cmd is fine.
REM
REM IMPLEMENTATION NOTE: the per-dependency body runs in a subroutine (call :one)
REM because cmd cannot nest a "for /f" inside another "for /f" body without
REM losing variables set in the inner loop's parent scope - and splitting
REM "url|marker" needs exactly that. A subroutine also keeps error paths clean.
REM ============================================================================

setlocal enabledelayedexpansion

call "%~dp0_common.bat"
if %ERRORLEVEL% neq 0 (
    pause
    exit /b 1
)

echo ======================================
echo    CurSeen  [2/6] download third-party libs
echo ======================================
echo.

if not defined CURL_EXE (
    echo   [ERROR] curl.exe not found
    echo           Windows 10/11 ships it; install curl if missing.
    pause
    exit /b 1
)
if not exist "%DEPS_FILE%" (
    echo   [ERROR] deps.txt not found: %DEPS_FILE%
    pause
    exit /b 1
)

if not exist "%THIRDPARTY%" mkdir "%THIRDPARTY%"
if not exist "%DOWNLOAD_CACHE%" mkdir "%DOWNLOAD_CACHE%"

if "%CURSEEN_FORCE_DOWNLOAD%"=="1" echo   CURSEEN_FORCE_DOWNLOAD=1 - re-downloading everything
echo.

set "DL_COUNT=0"
set "SKIP_COUNT=0"
set "FIX_COUNT=0"

for /f "usebackq tokens=1,* delims==" %%a in ("%DEPS_FILE%") do (
    call :one "%%a" "%%b"
    if !ERRORLEVEL! neq 0 exit /b 1
)

echo.
echo   downloaded: !DL_COUNT!  skipped: !SKIP_COUNT!  repaired: !FIX_COUNT!
echo.

cd /d "%SCRIPT_DIR%"
if not "!CURSEEN_CHAINED!"=="1" pause
exit /b 0


REM ============================================================================
REM :one <name> <url-or-url|marker>
REM   Handles a single deps.txt entry. Returns 1 on any hard failure.
REM ============================================================================
:one
set "NAME=%~1"
set "REST=%~2"

REM Skip comments and blank lines.
if "!NAME:~0,1!"=="#" exit /b 0
if "!NAME!"=="" exit /b 0

REM Split "url|marker" (marker optional). Runs in a subroutine so the inner
REM for /f's parent scope is this label, not the outer loop.
set "URL=!REST!"
set "MARKER="
for /f "tokens=1,* delims=|" %%u in ("!REST!") do (
    set "URL=%%u"
    set "MARKER=%%v"
)
for /f "tokens=* delims= " %%u in ("!URL!") do set "URL=%%u"

REM Decide whether the extracted tree looks complete.
set "HAVE=0"
if exist "%THIRDPARTY%\!NAME!" (
    set "HAVE=1"
    if not "!MARKER!"=="" if not exist "%THIRDPARTY%\!NAME!\!MARKER!" set "HAVE=0"
)

echo   checking !NAME!...

if "!HAVE!"=="1" if not "!CURSEEN_FORCE_DOWNLOAD!"=="1" (
    echo     present, skipping
    set /a SKIP_COUNT+=1
    exit /b 0
)

if "!HAVE!"=="0" if exist "%THIRDPARTY%\!NAME!" (
    echo     incomplete directory, re-extracting
    set /a FIX_COUNT+=1
)

set "CACHED=0"
if not "!CURSEEN_FORCE_DOWNLOAD!"=="1" if exist "%DOWNLOAD_CACHE%\!NAME!.tar.gz" set "CACHED=1"

if "!CACHED!"=="0" (
    echo     downloading: !URL!
    "%CURL_EXE%" -L --fail --retry 3 --retry-all-errors --retry-delay 3 -o "%DOWNLOAD_CACHE%\!NAME!.tar.gz" "!URL!"
    if !ERRORLEVEL! neq 0 (
        echo     [ERROR] download failed: !NAME!
        echo             URL: !URL!
        pause
        exit /b 1
    )
) else (
    echo     cached, skipping download
)

REM Extract into a scratch dir first; only promote it once tar reported success,
REM so an interrupted run never leaves a directory that looks complete.
echo     extracting...
if exist "%THIRDPARTY%\!NAME!.extracting" rmdir /s /q "%THIRDPARTY%\!NAME!.extracting"
mkdir "%THIRDPARTY%\!NAME!.extracting"

"%TAR_EXE%" -xzf "%DOWNLOAD_CACHE%\!NAME!.tar.gz" -C "%THIRDPARTY%\!NAME!.extracting"
if !ERRORLEVEL! neq 0 (
    echo     [ERROR] extraction failed: !NAME!
    echo             archive: %DOWNLOAD_CACHE%\!NAME!.tar.gz
    echo             hint: delete that archive and retry.
    pause
    exit /b 1
)

set "EXTRACTED_DIR="
for /d %%d in ("%THIRDPARTY%\!NAME!.extracting\*") do (
    if not defined EXTRACTED_DIR set "EXTRACTED_DIR=%%d"
)

if not defined EXTRACTED_DIR (
    echo     [ERROR] no top-level directory after extraction
    pause
    exit /b 1
)

REM Verify the marker before promoting - catches a truncated archive.
if not "!MARKER!"=="" if not exist "!EXTRACTED_DIR!\!MARKER!" (
    echo     [ERROR] marker missing after extraction: !MARKER!
    echo             archive may be truncated: %DOWNLOAD_CACHE%\!NAME!.tar.gz
    pause
    exit /b 1
)

if exist "%THIRDPARTY%\!NAME!" rmdir /s /q "%THIRDPARTY%\!NAME!"
move "!EXTRACTED_DIR!" "%THIRDPARTY%\!NAME!" >nul
rmdir /s /q "%THIRDPARTY%\!NAME!.extracting" 2>nul
echo     extracted: !NAME!
set /a DL_COUNT+=1
exit /b 0
