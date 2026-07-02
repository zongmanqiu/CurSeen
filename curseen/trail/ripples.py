"""点击波纹效果 — 实心圆 / 空心圆"""

from PySide6.QtCore import Qt, QRectF, QTimer
from PySide6.QtGui import QPainter, QPen, QBrush, QColor


class RippleEffect:
    """单个波纹动画"""

    def __init__(self, x: int, y: int, color: QColor, outer: float, hollow: bool, duration_ms: int = 1000):
        self.x = x
        self.y = y
        self.color = QColor(color)
        self.outer_size = outer
        self.hollow = hollow
        self.progress = 0.0
        self.duration_ms = duration_ms
        self.elapsed = 0.0
        self.alive = True

    def tick(self, dt_ms: float) -> bool:
        if not self.alive:
            return False
        self.elapsed += dt_ms
        self.progress = min(self.elapsed / self.duration_ms, 1.0)
        if self.progress >= 1.0:
            self.alive = False
        return self.alive

    def draw(self, painter: QPainter):
        if not self.alive:
            return
        alpha = int(255 * (1.0 - self.progress))
        size = int(self.outer_size * (0.2 + self.progress * 0.8))
        r = size / 2
        c = QColor(self.color)
        c.setAlpha(alpha)

        cx, cy = self.x - r, self.y - r
        rect = QRectF(cx, cy, size, size)

        painter.setRenderHint(QPainter.RenderHint.Antialiasing)
        if self.hollow:
            pen_w = max(1.0, self.outer_size * 0.1)
            painter.setPen(QPen(c, pen_w))
            painter.setBrush(Qt.BrushStyle.NoBrush)
        else:
            painter.setPen(Qt.PenStyle.NoPen)
            painter.setBrush(QBrush(c))
        painter.drawEllipse(rect)


class RippleManager:
    """管理所有活动波纹"""

    def __init__(self):
        self._ripples: list[RippleEffect] = []
        self._timer = QTimer()
        self._timer.setInterval(16)  # ~60fps
        self._timer.timeout.connect(self._tick)
        self._last_tick = 0
        self._on_update = None

    def set_update_callback(self, cb):
        self._on_update = cb

    def start(self):
        import time
        self._last_tick = time.perf_counter()
        self._timer.start()

    def stop(self):
        self._timer.stop()
        self._ripples.clear()

    def add(self, x: int, y: int, color: QColor, outer: float, hollow: bool, duration_ms: int = 1000):
        import time
        if not self._ripples and not self._timer.isActive():
            self._last_tick = time.perf_counter()
        self._ripples.append(RippleEffect(x, y, color, outer, hollow, duration_ms))
        if not self._timer.isActive():
            self._timer.start()

    def _tick(self):
        import time
        now = time.perf_counter()
        dt = (now - self._last_tick) * 1000
        self._last_tick = now

        self._ripples = [r for r in self._ripples if r.tick(dt)]
        if not self._ripples:
            self._timer.stop()
        if self._on_update:
            self._on_update()

    def draw(self, painter: QPainter):
        for ripple in self._ripples:
            ripple.draw(painter)
