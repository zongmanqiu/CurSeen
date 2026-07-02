@echo off
title CurSeen
cd /d "%~dp0.."

REM Try venv pythonw first (no console window)
if exist "E:\tmp\Reasonix\venvs\CurSeen\Scripts\pythonw.exe" (
    start "" "E:\tmp\Reasonix\venvs\CurSeen\Scripts\pythonw.exe" -m curseen
    exit
)

REM Fallback to system pythonw
start "" pythonw -m curseen
exit
