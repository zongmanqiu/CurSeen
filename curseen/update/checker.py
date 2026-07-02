"""检查最新 Gitee Release（urllib 后台线程，避免 Qt SSL 问题）"""

import json, threading
from PySide6.QtCore import QObject, Signal

from curseen.core.logger import get_logger

_log = get_logger("update")

GITEE_API = "https://gitee.com/api/v5/repos/qiuzongman/curseen/releases"
RELEASE_BASE = "https://gitee.com/qiuzongman/curseen/releases/download"


def _fetch(url: str, timeout: int = 8):
    import urllib.request, ssl
    ctx = ssl._create_unverified_context()
    req = urllib.request.Request(url, headers={
        "User-Agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) CurSeen/1.0",
    })
    try:
        with urllib.request.urlopen(req, timeout=timeout, context=ctx) as resp:
            return json.loads(resp.read().decode("utf-8"))
    except Exception as e:
        return {"_error": str(e)}


class VersionChecker(QObject):
    finished = Signal(dict)

    def __init__(self, current_version: str, parent=None):
        super().__init__(parent)
        self._current = self._parse(current_version)
        self._raw_version = current_version

    @staticmethod
    def _parse(v: str) -> tuple:
        v = v.lstrip("vV")
        parts = v.split(".")
        try:
            return tuple(int(p) for p in parts[:3])
        except ValueError:
            return (0, 0, 0)

    def check(self):
        _log.info("Version check started (current=%s)", self._raw_version)
        t = threading.Thread(target=self._run, daemon=True)
        t.start()

    def _run(self):
        data = _fetch(GITEE_API)
        if "_error" in data:
            _log.warning("Version check failed: %s", data["_error"])
            self.finished.emit({"has_new": False, "latest": self._raw_version,
                                "download_url": None, "error": data["_error"]})
            return

        best_tag, best_latest = "", (0, 0, 0)
        best_url = None
        for release in (data if isinstance(data, list) else [data]):
            t = release.get("tag_name", "")
            v = self._parse(t)
            if v > best_latest:
                best_latest = v
                best_tag = t
                best_url = None
                for asset in release.get("assets", []):
                    name = asset.get("name", "")
                    url = asset.get("browser_download_url", "")
                    if name.endswith(".zip") and "CurSeen" in name:
                        best_url = url
                        break
                if not best_url:
                    base = best_tag.lstrip("vV")
                    best_url = f"{RELEASE_BASE}/{best_tag}/CurSeen_{base}.zip"

        has_new = best_latest > self._current
        _log.info("Version check result: latest=%s has_new=%s url=%s",
                  best_tag, has_new, best_url)
        self.finished.emit({"has_new": has_new, "latest": best_tag,
                            "download_url": best_url, "error": None})
