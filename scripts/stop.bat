@echo off
title Stop CurSeen
cd /d "%~dp0"
if exist "E:\tmp\Reasonix\venvs\CurSeen\Scripts\python.exe" (
    "E:\tmp\Reasonix\venvs\CurSeen\Scripts\python.exe" stop.py
) else (
    python stop.py
)
