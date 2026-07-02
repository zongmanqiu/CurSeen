"""下载完成后生成 update.bat → 启动 → 退出"""

import os, sys, subprocess
from PySide6.QtCore import QObject, Signal
from curseen.core.logger import get_logger

_log = get_logger("update")


class UpdateInstaller(QObject):
    finished = Signal(str)

    def __init__(self, zip_path: str, parent=None):
        super().__init__(parent)
        self._zip_path = zip_path
        self._app_dir = self._find_app_dir()

    @staticmethod
    def _find_app_dir() -> str:
        # 优先用主脚本路径（对 Nuitka 编译的 exe，sys.argv[0] 就是 exe 自身）
        return os.path.dirname(os.path.abspath(sys.argv[0]))

    def install(self):
        _log.info("Generating update.bat for zip=%s app_dir=%s", self._zip_path, self._app_dir)
        try:
            bat_path = os.path.join(self._app_dir, "curseen_update.bat")
            with open(bat_path, "w", encoding="utf-8") as f:
                f.write(self._bat_script())
            _log.info("Launching update.bat")
            subprocess.Popen(
                [bat_path],
                creationflags=subprocess.CREATE_NO_WINDOW,
                shell=True,
            )
            self.finished.emit(None)
            os._exit(0)
        except Exception as e:
            _log.error("Update failed: %s", e)
            self.finished.emit(str(e))

    def _bat_script(self) -> str:
        app = self._app_dir
        zipf = self._zip_path
        return f'''@echo off
chcp 65001 >nul
setlocal enabledelayedexpansion

echo =============================================
echo  CurSeen Updater
echo =============================================

REM ── c. 关闭 curseen 程序 ──
echo [1/5] Closing CurSeen...
:wait
tasklist /fi "ImageName eq curseen.exe" 2>nul | find "curseen.exe" >nul
if errorlevel 1 goto kill_done
taskkill /f /im curseen.exe >nul 2>nul
timeout /t 2 /nobreak >nul
goto wait
:kill_done

REM ── d. 解压 ZIP 到独立文件夹 ──
echo [2/5] Extracting...
set "EXTDIR={app}\\.update_tmp"
if exist "%EXTDIR%" rmdir /s /q "%EXTDIR%"
mkdir "%EXTDIR%"
powershell -Command "Expand-Archive -Path '{zipf}' -DestinationPath '%EXTDIR%' -Force"

REM ── e. 定位内容目录并复制到当前文件夹 ──
echo [3/5] Copying files...
set "SRCDIR=%EXTDIR%"
if not exist "%SRCDIR%\\curseen.exe" (
    for /d %%d in ("%SRCDIR%\\*") do (
        if exist "%%d\\curseen.exe" set "SRCDIR=%%d"
    )
)
if not exist "%SRCDIR%\\curseen.exe" (
    for /d %%d in ("%EXTDIR%\\*") do (
        for /d %%e in ("%%d\\*") do (
            if exist "%%e\\curseen.exe" set "SRCDIR=%%e"
        )
    )
)
echo Source: !SRCDIR!
robocopy "!SRCDIR!" "{app}" /e /njh /njs /ndl /nc /ns /np /is /it >nul 2>nul

REM ── f. 删除解压文件夹和 ZIP ──
echo [4/5] Cleaning up...
rmdir /s /q "%EXTDIR%" 2>nul
del /q "{zipf}" 2>nul

REM ── g. 重新启动 curseen ──
echo [5/5] Starting CurSeen...
start "" "{app}\\curseen.exe"
del "%~f0"
exit
'''