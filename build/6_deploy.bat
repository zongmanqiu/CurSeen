@echo off
REM ============================================================================
REM 6_deploy.bat - assemble <project>\build\ from the cargo output
REM ----------------------------------------------------------------------------
REM Three python helpers do the work (they are source, shipped in this directory):
REM   deploy.py       copy temp\target\release\curseen.exe -> build\curseen.exe
REM                   and verify by MD5 + size; it also CALLS gen_licenses.py
REM   sync_assets.py  one-way sync main\assets\ -> build\assets\
REM                   ("add/update only, never delete" - build\assets is a
REM                    run-time data dir, users drop their own scripts/sounds in)
REM   gen_licenses.py copy LICENSE + THIRD-PARTY-NOTICES.md + licenses\ into
REM                   build\ so the output dir is self-contained for shipping
REM                   (the CurSeen twin of Pecia's `pecia_copy_licenses`)
REM
REM Why python and not plain copy: build.rs does NOT copy assets, so a plain
REM copy leaves build\assets stale -> "I added a lua script but the panel does
REM not list it". The MD5 cross-check catches "deployed a stale exe".
REM
REM NOTE on the MD5 check: the exe is NOT reproducible (the PE header carries a
REM build timestamp), so MD5_MATCH only proves "the file in build\ is the one
REM that was just compiled" - it is not a code-identity check across builds.
REM
REM NOTE: deploy.py exits 1 when a licence source is missing, which fails this
REM step. That is deliberate - a build\ without licences must not be shipped.
REM
REM Environment switches:
REM   CURSEEN_CHAINED=1  suppress the final "press any key" (set by full.bat)
REM ============================================================================

setlocal enabledelayedexpansion

call "%~dp0_common.bat"
if %ERRORLEVEL% neq 0 (
    pause
    exit /b 1
)

echo ======================================
echo    CurSeen  [6/6] deploy to build\
echo ======================================
echo.

if not defined PY_EXE (
    echo   [ERROR] python not found - cannot run deploy.py / sync_assets.py
    echo           install python and put it on PATH, then re-run this step
    pause
    exit /b 1
)

cd /d "%MAIN_DIR%"

echo   [a/3] copy exe + assets + licences...
"%PY_EXE%" "%MAIN_DIR%\build\deploy.py"
if !ERRORLEVEL! neq 0 (
    echo   [ERROR] deploy.py failed - see %TEMP_DIR%\deploy_report.txt
    echo           a missing licence source also lands here - see
    echo           %TEMP_DIR%\licenses_report.txt
    pause
    exit /b 1
)

echo.
echo   [b/3] verify assets...
"%PY_EXE%" "%MAIN_DIR%\build\sync_assets.py"
if !ERRORLEVEL! neq 0 (
    echo   [WARNING] sync_assets.py reported differences - see %TEMP_DIR%\assets_sync.txt
)

REM Licences: deploy.py already copied them, this only re-states the result so a
REM truncated / hand-edited build\ cannot slip through unnoticed.
echo.
echo   [c/3] verify licences are in place...
set "LIC_FAIL=0"
REM NOTICES lives in build\licenses\ by design (gen_licenses.py: "NOTICES groups
REM with the other third-party licence files"); only LICENSE sits at the root.
for %%f in (LICENSE licenses\THIRD-PARTY-NOTICES.md) do (
    if exist "%OUTPUT_DIR%\%%f" (
        echo         [OK] build\%%f
    ) else (
        echo         [ERROR] missing build\%%f
        set "LIC_FAIL=1"
    )
)
set "LIC_COUNT=0"
for %%f in ("%OUTPUT_DIR%\licenses\*") do set /a LIC_COUNT+=1
if "!LIC_COUNT!"=="0" set "LIC_FAIL=1"
echo         build\licenses\ : !LIC_COUNT! file^(s^)
if "!LIC_COUNT!"=="0" echo         [ERROR] build\licenses\ is empty or missing
if not "!LIC_COUNT!"=="0" echo         [OK] third-party licence texts present

if "!LIC_FAIL!"=="1" (
    echo.
    echo   [ERROR] licence bundle incomplete - build\ is not shippable yet
    pause
    exit /b 1
)

echo.
echo   output: %EXE%
echo.

cd /d "%SCRIPT_DIR%"
if not "!CURSEEN_CHAINED!"=="1" pause
exit /b 0
