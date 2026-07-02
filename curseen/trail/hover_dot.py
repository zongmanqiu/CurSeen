"""悬浮标 — 透明跟随圆圈窗口"""

from PySide6.QtWidgets import QWidget
from PySide6.QtCore import Qt, QRect, QRectF
from PySide6.QtGui import QPainter, QColor, QBrush, QPen


class HoverDot(QWidget):
    """跟随鼠标的悬浮圆圈"""

    def __init__(self):
        super().__init__()
        self._size = 60
        self._color = QColor("#1E90FF")
        self._enabled = False
        self._mode = 0  # 0=关闭 1=实心圆 2=空心圆
        self._inner_ratio = 0.6  # 内直径 = 外直径 * 0.6

        self.setWindowFlags(
            Qt.WindowType.FramelessWindowHint
            | Qt.WindowType.WindowStaysOnTopHint
            | Qt.WindowType.Tool
            | Qt.WindowType.WindowTransparentForInput
        )
        self.setAttribute(Qt.WidgetAttribute.WA_TranslucentBackground)
        self.setAttribute(Qt.WidgetAttribute.WA_ShowWithoutActivating)
        self.resize(self._size, self._size)
        self.hide()

    def set_color(self, color: QColor):
        self._color = color
        self.update()

    def set_size(self, size: int):
        self._size = max(10, min(400, size))
        self.resize(self._size, self._size)

    def set_mode(self, mode: int):
        """0=关闭 1=实心圆 2=空心圆"""
        self._mode = mode
        if mode == 0:
            self.hide()
        else:
            self.show()
            self.raise_()
            self.repaint()

    def set_inner_ratio(self, ratio: float):
        self._inner_ratio = max(0.0, min(0.95, ratio))
        self.update()

    def move_to(self, x: int, y: int):
        if self._mode == 0:
            return
        self.move(x - self._size // 2, y - self._size // 2)

    def _draw_at(self, painter: QPainter, x: int, y: int):
        """在指定位置绘制（供 overlay 使用）"""
        if self._mode == 0:
            return
        painter.save()
        painter.setRenderHint(QPainter.RenderHint.Antialiasing)
        r = self._size / 2
        cx, cy = x, y
        pen_w = r * (1.0 - self._inner_ratio)
        if pen_w < 0.5 or pen_w >= r:
            painter.setBrush(QBrush(self._color))
            painter.setPen(Qt.PenStyle.NoPen)
            painter.drawEllipse(QRectF(cx - r, cy - r, self._size, self._size))
        else:
            draw_r = r - pen_w / 2
            painter.setPen(QPen(self._color, pen_w))
            painter.setBrush(Qt.BrushStyle.NoBrush)
            painter.drawEllipse(QRectF(cx - draw_r, cy - draw_r, draw_r * 2, draw_r * 2))
        painter.restore()

    def paintEvent(self, event):
        if self._mode == 0:
            return
        p = QPainter(self)
        p.setRenderHint(QPainter.RenderHint.Antialiasing)

        r = self._size / 2

        # 空心圆（轮廓绘制，宽度足够时自然成为实心圆）
        pen_w = r * (1.0 - self._inner_ratio)
        if pen_w < 0.5 or pen_w >= r:
            p.setBrush(QBrush(self._color))
            p.setPen(Qt.PenStyle.NoPen)
            p.drawEllipse(QRectF(0, 0, self._size, self._size))
        else:
            draw_r = r - pen_w / 2
            p.setPen(QPen(self._color, pen_w))
            p.setBrush(Qt.BrushStyle.NoBrush)
            p.drawEllipse(QRectF(r - draw_r, r - draw_r, draw_r * 2, draw_r * 2))
        p.end()
