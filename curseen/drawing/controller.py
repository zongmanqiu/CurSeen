"""绘图控制器 — 工具切换 + 笔管理"""

from PySide6.QtCore import QObject, Signal
from PySide6.QtGui import QColor

from curseen.drawing.history import DrawingHistory
from curseen.core.drawing_overlay import DrawingOverlay


class DrawingController(QObject):
    """管理绘图状态、笔参数、工具"""

    pen_changed = Signal()

    def __init__(self, overlay: DrawingOverlay):
        super().__init__()
        self._overlay = overlay
        self._history = DrawingHistory()
        overlay.set_history(self._history)
        overlay.hide()

        self._pens = [
            {"color": QColor("#FF000000"), "width": 4, "style": "solid"},
            {"color": QColor("#40FF0000"), "width": 30, "style": "solid"},
            {"color": QColor("#FF00FF00"), "width": 4, "style": "dashed"},
        ]
        self._active_pen = 0
        self._drawing = False
        self._last_toggle = 0.0

    @property
    def active_pen(self) -> int:
        return self._active_pen

    @property
    def pen(self) -> dict:
        return self._pens[self._active_pen]

    @property
    def drawing(self) -> bool:
        return self._drawing

    def set_active_pen(self, index: int):
        self._active_pen = max(0, min(2, index))
        p = self._pens[self._active_pen]
        self._overlay.set_pen(p["color"], p["width"], p["style"])
        self._overlay.set_tool("pen")
        self.pen_changed.emit()

    def set_tool(self, tool: str):
        self._overlay.set_tool(tool)

    def set_pen_color(self, index: int, color: QColor):
        self._pens[index]["color"] = color
        if index == self._active_pen:
            self._overlay.set_pen(color, self._pens[index]["width"], self._pens[index]["style"])

    def set_pen_width(self, index: int, width: int):
        self._pens[index]["width"] = width
        if index == self._active_pen:
            self._overlay.set_pen(self._pens[index]["color"], width, self._pens[index]["style"])

    def set_pen_style(self, index: int, style: str):
        self._pens[index]["style"] = style
        if index == self._active_pen:
            self._overlay.set_pen(self._pens[index]["color"], self._pens[index]["width"], style)

    def start(self, mode: str = "screen"):
        self._overlay._on_stopped = self._on_overlay_esc
        self._overlay.start_drawing(mode)
        self._drawing = True
        p = self._pens[self._active_pen]
        self._overlay.set_pen(p["color"], p["width"], p["style"])
        self._overlay.set_tool("pen")

    def _on_overlay_esc(self):
        """覆盖层按 ESC 时通知控制器"""
        self._drawing = False

    def stop(self):
        self._overlay._on_stopped = None
        self._overlay.stop_drawing()
        self._drawing = False
        self._history.clear()

    def toggle(self, mode: str = "screen"):
        import time
        now = time.perf_counter()
        if now - self._last_toggle < 0.5:
            return
        self._last_toggle = now
        if self._drawing:
            self.stop()
        else:
            self.start(mode)

    def undo(self):
        if self._drawing:
            self._overlay.undo()

    def clear(self):
        if self._drawing:
            self._overlay.clear_all()

    @property
    def history(self):
        return self._history
