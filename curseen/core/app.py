"""应用启动模块"""

import sys
import os
from PySide6.QtWidgets import QApplication
from PySide6.QtCore import Qt, QSharedMemory

from curseen.core.tray import TrayManager
from curseen.core.config import Config


class AppController:
    """主控制器"""

    def __init__(self):
        self.overlay = None
        self.main_window = None
        self.tray = None
        self.key_bridge = None
        self.mouse_bridge = None
        self.key_controller = None
        self.trail_controller = None
        self.config = None

    def start(self):
        from curseen.core.overlay import OverlayWindow
        from curseen.core.main_window import MainWindow
        from curseen.core.bridge import KeyEventBridge, MouseEventBridge
        from curseen.key_display.controller import KeyController
        from curseen.trail.controller import TrailController

        # 1. 加载配置
        self.config = Config()
        self.config.load()

        # 1b. 全局字号
        from curseen.core.style import set_font_size
        ui_size = self.config.data.get("ui", {}).get("font_size", 16)
        set_font_size(ui_size)

        # 2. 按键控制器
        self.key_controller = KeyController()
        key_settings = self.config.data.get("key_display", {})
        if key_settings:
            self.key_controller.load_settings(key_settings)

        # 2b. 全局功能开关
        gs = self.config.data.get("global", {})
        self.key_controller.enabled = gs.get("enabled_key", True)

        # 2c. 全局热键解析
        self._hotkey_mods: set[str] = set()
        self._parse_hotkey(gs.get("panel_hotkey", "Ctrl+Shift+D"))

        # 3. 覆盖层
        self.overlay = OverlayWindow()
        self.overlay.set_key_controller(self.key_controller)
        self.overlay.show_fullscreen()

        # 4. 鼠标特效控制器（需在主窗口之前初始化）
        self.trail_controller = TrailController()
        self.trail_controller.set_config(self.config)
        self.trail_controller.load_settings(self.config.data)
        self.trail_controller.ripple_manager.set_update_callback(self.overlay.update)
        self.trail_controller.set_trail_update_callback(self.overlay.update)
        self.overlay.set_trail_controller(self.trail_controller)

        # 4b. 绘图覆盖层 + 控制器
        from curseen.core.drawing_overlay import DrawingOverlay
        from curseen.drawing.controller import DrawingController
        self.drawing_overlay = DrawingOverlay()
        self.drawing_controller = DrawingController(self.drawing_overlay)


        # 5. 主窗口
        self.main_window = MainWindow(self, self.overlay)
        # 加载画笔设置到面板
        if hasattr(self.main_window, '_draw_panel'):
            self.main_window._draw_panel._app_controller = self
            self.main_window._draw_panel.load_settings(self.config.data)
            # 首次启动时写入全部默认热键
            hks = self.config.data.setdefault("drawing", {}).setdefault("hotkeys", {})
            defaults = {
                "draw_hk_screen": "Alt+G", "draw_hk_white": "Alt+W",
                "draw_hk_pen1": "1", "draw_hk_pen2": "2", "draw_hk_pen3": "3",
                "draw_hk_eraser": "4", "draw_hk_line": "5", "draw_hk_arrow": "6",
                "draw_hk_rect": "7", "draw_hk_ellipse": "8", "draw_hk_text": "9",
            }
            for k, v in defaults.items():
                hks.setdefault(k, v)
            self.config.save()

        # 5. 全局钩子
        self.key_bridge = KeyEventBridge()
        self.key_bridge.key_pressed.connect(self.key_controller.on_key_pressed)
        self.key_bridge.key_pressed.connect(self._on_key_for_hotkey)
        self.key_bridge.key_pressed.connect(self._on_key_for_capture)
        self.key_bridge.key_pressed.connect(self._on_key_for_drawing)
        self.key_bridge.key_released.connect(self.key_controller.on_key_released)
        self.key_bridge.key_released.connect(self._on_key_released_for_hotkey)
        self.key_bridge.key_released.connect(self._on_key_released_for_drawing)
        self.key_bridge.start()

        self.mouse_bridge = MouseEventBridge()
        self.mouse_bridge.start()

        # 5b. 鼠标特效 — 连接鼠标事件
        self.mouse_bridge.moved.connect(self.trail_controller.on_mouse_moved)
        self.mouse_bridge.pressed.connect(self._on_mouse_clicked)

        # 6. 系统托盘
        self.tray = TrayManager(self)

        self.main_window.show()
        # Windows 标题栏自定义颜色 (#FAFAFA)
        import sys as _sys
        if _sys.platform == "win32":
            try:
                import ctypes
                hwnd = int(self.main_window.winId())
                DWMWA_CAPTION_COLOR = 35
                col = 0x00FAFAFA  # COLORREF 0x00BBGGRR
                ctypes.windll.dwmapi.DwmSetWindowAttribute(
                    hwnd, DWMWA_CAPTION_COLOR,
                    ctypes.byref(ctypes.c_int(col)), 4)
            except Exception:
                pass
        self.tray.show()

        # ── 启动后同步功能开关状态到子页面 ──
        gs = self.config.data.get("global", {})
        trail_enabled = gs.get("enabled_trail", False)
        draw_enabled = gs.get("enabled_draw", False)
        if self.trail_controller:
            self.trail_controller._trail_enabled = trail_enabled
            if trail_enabled and self.trail_controller.trail_effect:
                import time
                self.trail_controller._trail_last_tick = time.perf_counter()
                self.trail_controller._trail_timer.start()
            else:
                self.trail_controller._trail_timer.stop()
                if self.trail_controller.trail_effect:
                    self.trail_controller.trail_effect.stop()
        if self.drawing_controller:
            self.drawing_controller._enabled = draw_enabled
        if hasattr(self, 'main_window') and self.main_window:
            if hasattr(self.main_window, '_trail_panel'):
                pass
            if hasattr(self.main_window, '_draw_panel'):
                pass

    def _parse_hotkey(self, hotkey_str: str):
        """解析快捷键字符串 → (_hotkey_mods, _hotkey_key)"""
        parts = [p.strip() for p in hotkey_str.split(" + ")]
        self._hotkey_mod_req: set[str] = set()
        self._hotkey_key_req: str | None = None
        for p in parts:
            if p in ("Ctrl", "Alt", "Shift", "Win"):
                self._hotkey_mod_req.add(p)
            else:
                self._hotkey_key_req = p

    def _on_mouse_clicked(self, btn: str, x: int, y: int):
        """鼠标按下 → 转发给 trail 控制器"""
        if self.trail_controller:
            self.trail_controller.on_mouse_click(btn, x, y)

    def _on_key_for_hotkey(self, key_str: str):
        """检测热键是否匹配（维护独立修饰键跟踪）"""
        if not self._hotkey_key_req or not self.key_controller:
            return
        from curseen.key_display.keymap import lookup
        r = lookup(key_str)
        if r is None:
            return
        display, ktype, _ = r
        if ktype == "mod":
            self._hotkey_mods.add(display)
            return

        # 非修饰键 → 检查热键
        key_name = display
        if key_str.startswith("'") and key_str.endswith("'"):
            raw = key_str[1:-1]
            if len(raw) == 1:
                key_name = raw.upper()
        else:
            r2 = lookup(key_str)
            if r2:
                key_name = r2[0]

        if not key_name:
            return

        if self._hotkey_mods == self._hotkey_mod_req and key_name == self._hotkey_key_req:
            self.toggle_window()

    def _on_key_released_for_hotkey(self, key_str: str):
        from curseen.key_display.keymap import lookup
        r = lookup(key_str)
        if r and r[1] == "mod":
            self._hotkey_mods.discard(r[0])

    def _on_key_released_for_drawing(self, key_str: str):
        """绘图修饰键释放"""
        if hasattr(self, '_draw_mods'):
            from curseen.key_display.keymap import lookup
            r = lookup(key_str)
            if r and r[1] == "mod":
                self._draw_mods.discard(r[0])

    def _on_key_for_capture(self, key_str: str):
        """捕获模式下转发 pynput 按键给全局设置面板和绘图面板"""
        # 全局设置面板
        if (self.main_window
                and hasattr(self.main_window, '_global_panel')
                and hasattr(self.main_window._global_panel, '_key_capture')):
            self.main_window._global_panel._key_capture.handle_pynput_key(key_str)
        # 绘图面板快捷键
        if (self.main_window
                and hasattr(self.main_window, '_draw_panel')
                and hasattr(self.main_window._draw_panel, '_hk_widgets')):
            for hk in self.main_window._draw_panel._hk_widgets.values():
                hk.handle_pynput_key(key_str)

    def _capture_in_progress(self) -> bool:
        """检查是否有 KeyCaptureLineEdit 正在捕获快捷键"""
        if not self.main_window:
            return False
        # 全局设置面板快捷键
        if (hasattr(self.main_window, '_global_panel')
                and hasattr(self.main_window._global_panel, '_key_capture')
                and self.main_window._global_panel._key_capture._capturing):
            return True
        # 绘图面板快捷键
        if (hasattr(self.main_window, '_draw_panel')
                and hasattr(self.main_window._draw_panel, '_hk_widgets')):
            for hk in self.main_window._draw_panel._hk_widgets.values():
                if hk._capturing:
                    return True
        return False

    def _on_key_for_drawing(self, key_str: str):
        """绘图快捷键检测（从 config 读取映射）"""
        if not self.drawing_controller:
            return
        if not self.drawing_controller._enabled:
            return

        # 如果有 KeyCaptureLineEdit 正在捕获，跳过快捷键判断
        if self._capture_in_progress():
            return

        from curseen.key_display.keymap import lookup
        r = lookup(key_str)
        if r is None:
            return
        display, ktype, _ = r

        # 本地追踪修饰键（不依赖 key_controller，避免跨信号状态丢失）
        if not hasattr(self, '_draw_mods'):
            self._draw_mods: set[str] = set()

        if ktype == "mod":
            self._draw_mods.add(display)
            return

        # 普通键 → 构建热键字符串
        mods = []
        if "Ctrl" in self._draw_mods: mods.append("Ctrl")
        if "Alt" in self._draw_mods: mods.append("Alt")
        if "Shift" in self._draw_mods: mods.append("Shift")
        if "Win" in self._draw_mods: mods.append("Win")
        mods.append(display.upper())
        pressed = " + ".join(mods)

        hks = self.config.data.get("drawing", {}).get("hotkeys", {})

        def match(target):
            if not target:
                return False
            norm_pressed = pressed.replace(" ", "").upper()
            norm_target = target.replace(" ", "").upper()
            return norm_pressed == norm_target

        if match(hks.get("draw_hk_screen", "Alt+G")):
            self.drawing_controller.toggle("screen")
            return
        if match(hks.get("draw_hk_white", "Alt+W")):
            self.drawing_controller.toggle("whiteboard")
            return

        if not self.drawing_controller.drawing:
            return

        # 固定键
        if display.upper() == "Z" and "Ctrl" in self._draw_mods:
            self.drawing_controller.undo()
            return
        if "backspace" in key_str.lower():
            self.drawing_controller.undo()
            return
        if display in ("Del", "Delete") or "delete" in key_str.lower():
            self.drawing_controller.clear()
            return

        # 工具切换（从 config 读取）
        tool_map = {
            "draw_hk_pen1": ("pen", 0), "draw_hk_pen2": ("pen", 1), "draw_hk_pen3": ("pen", 2),
            "draw_hk_eraser": ("eraser", None), "draw_hk_line": ("line", None),
            "draw_hk_arrow": ("arrow", None), "draw_hk_rect": ("rect", None),
            "draw_hk_ellipse": ("ellipse", None), "draw_hk_text": ("text", None),
        }
        for hk_key, (tool, pen_idx) in tool_map.items():
            if match(hks.get(hk_key, "")):
                if pen_idx is not None:
                    self.drawing_controller.set_active_pen(pen_idx)
                else:
                    self.drawing_controller.set_tool(tool)
                return

    def toggle_window(self):
        if self.main_window and self.main_window.isVisible():
            self.main_window.hide()
        else:
            self.main_window.show()
            self.main_window.raise_()
            self.main_window.activateWindow()

    def show_window(self):
        """托盘双击/显示窗口时调用"""
        if self.main_window:
            self.main_window.show()
            self.main_window.raise_()
            self.main_window.activateWindow()

    def hide_window(self):
        if self.main_window:
            self.main_window.hide()

    def quit(self):
        """退出应用"""
        # 立即强制退出，不等待监听器线程（daemon 线程由 OS 清理）
        if self.tray:
            self.tray.hide()
        if self.config:
            self.config.save()
        os._exit(0)


def main():
    # ── 单实例保护 ──
    shared_memory = QSharedMemory("CurSeen-SingleInstance-0972a1")
    # 先尝试清理遗留的共享内存段（上次崩溃可能没清理干净）
    if shared_memory.attach():
        shared_memory.detach()
    if not shared_memory.create(1):
        print("CurSeen 已在运行")
        sys.exit(0)

    # ── 写入 PID 文件（供 stop.py 关闭） ──
    _write_pid()

    QApplication.setHighDpiScaleFactorRoundingPolicy(
        Qt.HighDpiScaleFactorRoundingPolicy.PassThrough
    )
    app = QApplication(sys.argv)
    app.setApplicationName("CurSeen")
    app.setOrganizationName("CurSeen")
    app.setQuitOnLastWindowClosed(False)

    app.setStyle("Fusion")
    palette = app.palette()
    palette.setColor(palette.ColorRole.Window, Qt.GlobalColor.darkGray)
    app.setPalette(palette)

    presenter = AppController()
    presenter.start()

    sys.exit(app.exec())


def _write_pid():
    """将当前进程 PID 写入文件，供 stop.py 关闭用"""
    from pathlib import Path
    import os
    pid_dir = Path(os.environ.get("APPDATA", ".")) / "CurSeen"
    pid_dir.mkdir(parents=True, exist_ok=True)
    (pid_dir / "curseen.pid").write_text(str(os.getpid()))
