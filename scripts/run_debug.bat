@echo off
title CurSeen Debug
cd /d "%~dp0.."

echo ========================================
echo  CurSeen Debug Mode
echo ========================================
echo.

python -m curseen

echo.
if errorlevel 1 (
    echo Program exited with code: %errorlevel%
) else (
    echo Program exited normally.
)
echo.
pause
