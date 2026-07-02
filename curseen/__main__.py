"""应用入口：python -m curseen（内置管理员权限提升）"""
import sys
import os

def _ensure_admin():
    """如果不是管理员，用 ShellExecute runas 重开自己"""
    if sys.platform != "win32":
        return
    import ctypes
    # 检测是否已经是管理员
    try:
        is_admin = ctypes.windll.shell32.IsUserAnAdmin()
    except Exception:
        is_admin = False
    if is_admin:
        return
    # 重新以管理员身份启动
    exe = sys.executable
    params = " ".join(['-m', 'curseen'] + sys.argv[1:])
    ctypes.windll.shell32.ShellExecuteW(
        None, "runas", exe, params, os.getcwd(), 1
    )
    sys.exit(0)

_ensure_admin()

from curseen.core.app import main

if __name__ == "__main__":
    main()
