@echo off
REM ============================================================================
REM 3_patch.bat - apply our patches from main/patches/ into .3rdParty/
REM ----------------------------------------------------------------------------
REM Copies ONE WAY (patches -> .3rdParty only, never the reverse).
REM
REM Rule: for every sub-directory of main/patches/ whose name matches a
REM directory under .3rdParty/, copy its whole tree over (recursive, overwrite),
REM except the registry file PATCHES.md itself. Example:
REM
REM   main\patches\wind-ui-rust-0.20.0\**  ->  .3rdParty\wind-ui-rust-0.20.0\**
REM
REM WHY one-way: .3rdParty/ usually holds the PRISTINE upstream sources right
REM after 2_download.bat, so copying back would overwrite the authoritative
REM patch archive with unpatched code (see AGENTS.md / patches/README.md).
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
echo    CurSeen  [3/6] apply third-party patches
echo ======================================
echo.

if not exist "%PATCHES%" (
    echo   [ERROR] patches directory not found: %PATCHES%
    pause
    exit /b 1
)

set "APPLIED=0"
set "SKIPPED=0"

for /d %%d in ("%PATCHES%\*") do (
    set "NAME=%%~nxd"
    if exist "%THIRDPARTY%\!NAME!" (
        echo   !NAME! ...
        REM robocopy exit codes: 0-7 = success (files copied / nothing to do),
        REM 8+ = at least one file failed. /XF drops the registry file.
        robocopy "%%d" "%THIRDPARTY%\!NAME!" /E /XF PATCHES.md /NFL /NDL /NJH /NJS /NP >nul
        if !ERRORLEVEL! GEQ 8 (
            echo     [ERROR] robocopy failed with exit code !ERRORLEVEL!
            pause
            exit /b 1
        )
        set /a APPLIED+=1
    ) else (
        echo   !NAME! ... skipped ^(no matching directory under .3rdParty^)
        set /a SKIPPED+=1
    )
)

echo.
echo   applied: !APPLIED!  skipped: !SKIPPED!
echo.

cd /d "%SCRIPT_DIR%"
if not "!CURSEEN_CHAINED!"=="1" pause
exit /b 0
