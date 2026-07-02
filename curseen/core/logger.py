"""简易日志记录（追加到 %APPDATA%/CurSeen/curseen.log）"""

import os
from pathlib import Path

_LOG_DIR = Path(os.environ.get("APPDATA", ".")) / "CurSeen"
_LOG_FILE = _LOG_DIR / "curseen.log"


def _ensure():
    _LOG_DIR.mkdir(parents=True, exist_ok=True)


def log(level: str, msg: str, *args):
    """写一行日志"""
    from datetime import datetime
    try:
        _ensure()
        text = msg % args if args else msg
        ts = datetime.now().strftime("%H:%M:%S.%f")[:12]
        line = f"{ts} [{level}] {text}\n"
        with open(_LOG_FILE, "a", encoding="utf-8") as f:
            f.write(line)
            if f.tell() > 1_048_576:
                f.truncate(0)
    except OSError:
        pass


def get_logger(name: str = "curseen"):
    """返回简易日志记录器（非标准 logging）"""
    class SimpleLogger:
        def info(self, msg, *args):
            log("INFO", msg, *args)

        def warning(self, msg, *args):
            log("WARN", msg, *args)

        def error(self, msg, *args):
            log("ERR", msg, *args)
    return SimpleLogger()
