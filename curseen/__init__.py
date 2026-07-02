# CurSeen / 可寻 — 键鼠痕迹呈现工具：鼠标拖尾 / 按键显示 / 屏幕书写
# SPDX-License-Identifier: AGPL-3.0-only

import sys
from pathlib import Path

__version__ = "1.0.0"
__app_name__ = "CurSeen"
__app_name_cn__ = "可寻"
__author__ = "邱宗满"
__author_url__ = "https://space.bilibili.com/423767625"
__email__ = "qiuzongman@foxmail.com"
__url__ = "https://gitee.com/qiuzongman/curseen"
__tools__ = {
    "Reasonix": "https://reasonix.io/",
    "DeepSeek": "https://www.deepseek.com/",
}

# ── 引导：确保包路径和依赖可用 ──
_PKG_ROOT = Path(__file__).resolve().parent
_PROJ_ROOT = _PKG_ROOT.parent

if str(_PROJ_ROOT) not in sys.path:
    sys.path.insert(0, str(_PROJ_ROOT))

try:
    import PySide6  # noqa: F401
except ImportError:
    _VENV_SITE = (
        _PROJ_ROOT.parent.parent
        / "venvs" / "CurSeen" / "Lib" / "site-packages"
    )
    if not _VENV_SITE.exists():
        _VENV_SITE = Path("E:/tmp/Reasonix/venvs/CurSeen/Lib/site-packages")
    if _VENV_SITE.exists() and str(_VENV_SITE) not in sys.path:
        sys.path.insert(0, str(_VENV_SITE))
        import PySide6  # noqa: F401
