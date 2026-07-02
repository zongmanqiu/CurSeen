"""屏幕预览控件 — 显示显示器布局和可点击位置选择"""

from PySide6.QtWidgets import QWidget
from PySide6.QtCore import Qt, QRect
from PySide6.QtGui import (
    QPainter, QColor, QPen, QFont, QMouseEvent,
)

from curseen.key_display.controller import NotificationPlacement


class MonitorPreview(QWidget):
    """显示器预览控件：绘制显示器矩形 + 分辨率 + 4个可点击位置"""

    MONITOR_BG = QColor("#2d2d2d")
    MONITOR_BORDER = QColor("#555555")
    POS_BG = QColor("#3c3c3c")
    POS_HOVER = QColor("#4a4a4a")
    POS_ACTIVE = QColor("#4a9eff")
    RESOLUTION_COLOR = QColor("#888888")

    PLACEMENT_LABELS = {
        NotificationPlacement.TOP_LEFT: "◤",
        NotificationPlacement.TOP_RIGHT: "◥",
        NotificationPlacement.BOTTOM_LEFT: "◣",
        NotificationPlacement.BOTTOM_RIGHT: "◢",
    }

    def __init__(self, parent=None):
        super().__init__(parent)
        self._placement = NotificationPlacement.BOTTOM_RIGHT  # 默认右下角
        self._hover_pos = None
        self._monitor_width = 1920
        self._monitor_height = 1080
        self._monitor_name = "Generic PnP Monitor"
        self._top_off = 0
        self._bottom_off = 10
        self._left_off = 0
        self._right_off = 0
        self._pos_rects = {}
        self.setFixedHeight(170)
        self.setMouseTracking(True)

    def showEvent(self, event):
        """首次显示时强制重绘确保高亮"""
        super().showEvent(event)
        self.repaint()

    def set_monitor_info(self, width: int, height: int, name: str = "Generic PnP Monitor"):
        self._monitor_width = width
        self._monitor_height = height
        self._monitor_name = name
        self.update()

    def set_placement(self, placement: NotificationPlacement):
        self._placement = placement
        self.update()

    def set_offsets(self, top: int, bottom: int, left: int, right: int):
        """从面板同步偏移值到预览"""
        self._top_off = top
        self._bottom_off = bottom
        self._left_off = left
        self._right_off = right
        self.update()

    def get_placement(self) -> NotificationPlacement:
        return self._placement

    # ── 鼠标交互 ──

    def mouseMoveEvent(self, event: QMouseEvent):
        pos = event.position().toPoint()
        self._hover_pos = None
        for placement, rect in self._pos_rects.items():
            if rect.contains(pos):
                self._hover_pos = placement
                break
        self.update()

    def mousePressEvent(self, event: QMouseEvent):
        pos = event.position().toPoint()
        for placement, rect in self._pos_rects.items():
            if rect.contains(pos):
                self._placement = placement
                self.update()
                cb = getattr(self, 'on_placement_changed', None)
                if cb:
                    cb(placement)
                break

    def leaveEvent(self, event):
        self._hover_pos = None
        self.update()

    # ── 绘制 ──

    def paintEvent(self, event):
        painter = QPainter(self)
        painter.setRenderHint(QPainter.RenderHint.Antialiasing)

        w = self.width()
        h = self.height()

        # 显示器矩形（居中，留底部文字空间）
        margin = 0
        avail_w = w - margin * 2
        avail_h = h - margin * 2

        aspect = self._monitor_width / max(self._monitor_height, 1)
        rect_w = avail_w
        rect_h = avail_w / aspect if aspect > 0 else avail_h

        if rect_h > avail_h:
            rect_h = avail_h
            rect_w = avail_h * aspect

        rect_x = margin
        rect_y = (h - rect_h) / 2

        monitor_rect = QRect(int(rect_x), int(rect_y), int(rect_w), int(rect_h))

        # 缩放比例：屏幕像素 → 预览像素
        scale_x = rect_w / max(self._monitor_width, 1)
        scale_y = rect_h / max(self._monitor_height, 1)

        # 绘制显示器
        painter.setPen(QPen(self.MONITOR_BORDER, 2))
        painter.setBrush(self.MONITOR_BG)
        painter.drawRoundedRect(monitor_rect, 6, 6)

        # 4个位置小方块（考虑偏移）
        pos_size = max(26, int(rect_w * 0.08))
        self._pos_rects = {}

        # 百分比偏移 → 预览像素
        off_t = int(rect_h * self._top_off / 100.0)
        off_b = int(rect_h * self._bottom_off / 100.0)
        off_l = int(rect_w * self._left_off / 100.0)
        off_r = int(rect_w * self._right_off / 100.0)

        positions = [
            (NotificationPlacement.TOP_LEFT,
             monitor_rect.left() + off_l,
             monitor_rect.top() + off_t),
            (NotificationPlacement.TOP_RIGHT,
             monitor_rect.right() - pos_size - off_r,
             monitor_rect.top() + off_t),
            (NotificationPlacement.BOTTOM_LEFT,
             monitor_rect.left() + off_l,
             monitor_rect.bottom() - pos_size - off_b),
            (NotificationPlacement.BOTTOM_RIGHT,
             monitor_rect.right() - pos_size - off_r,
             monitor_rect.bottom() - pos_size - off_b),
        ]

        for placement, px, py in positions:
            pr = QRect(int(px), int(py), pos_size, pos_size)
            self._pos_rects[placement] = pr

            if placement == self._placement:
                bg = self.POS_ACTIVE
                border_color = QColor("#7bb8ff")
            elif placement == self._hover_pos:
                bg = self.POS_HOVER
                border_color = QColor("#888888")
            else:
                bg = self.POS_BG
                border_color = QColor("#666666")

            painter.setPen(QPen(border_color, 1))
            painter.setBrush(bg)
            painter.drawRoundedRect(pr, 4, 4)

            symbol = self.PLACEMENT_LABELS.get(placement, "?")
            font = QFont()
            font.setPointSize(int(pos_size * 0.45))
            font.setBold(True)
            painter.setFont(font)
            painter.setPen(QColor("white") if placement == self._placement else QColor("#aaaaaa"))
            painter.drawText(pr, Qt.AlignmentFlag.AlignCenter, symbol)

    def sizeHint(self):
        from PySide6.QtCore import QSize
        return QSize(480, 280)
