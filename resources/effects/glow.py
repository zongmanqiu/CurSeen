"""辉光 — 柔软光芒拖尾（密度+间隔+大小+淡出）"""

import time
from collections import deque
from PySide6.QtCore import Qt, QPointF
from PySide6.QtGui import QPainter, QColor, QBrush, QRadialGradient
from resources.effects import TrailEffect


class GlowEffect(TrailEffect):
    """辉光 — 按间隔记录光斑，密度决定拖尾长度，粒子大小决定光斑半径"""

    @staticmethod
    def effect_id() -> str:
        return "glow"

    @staticmethod
    def effect_name() -> str:
        return "辉光圆影"

    def default_settings(self) -> dict:
        return {"fade_duration": 1.0, "opacity": 0.5,
                "color_mode": "rainbow", "fixed_color": "#FFD700"}

    def apply_settings(self, settings: dict):
        self.fade_duration = settings.get("fade_duration", 1.0)
        self.opacity = settings.get("opacity", 0.5)
        self.color_mode = settings.get("color_mode", "rainbow")
        self.fixed_color = QColor(settings.get("fixed_color", "#FFD700"))
        self.glow_radius = settings.get("particle_size", 8) * 2
        self.spawn_interval_ms = settings.get("spawn_interval_ms", 16)
        self.density = settings.get("density", 6)
        self._glows: deque = deque()

    def start(self):
        self._glows.clear()
        self._has_last = False

    def stop(self):
        self._glows.clear()

    def on_mouse_move(self, x: int, y: int):
        now = time.perf_counter()
        self._glows.append((x, y, now))
        while len(self._glows) > 120:
            self._glows.popleft()

    def update(self, dt_ms: float):
        now = time.perf_counter()
        cutoff = now - self.fade_duration
        while self._glows and self._glows[0][2] < cutoff:
            self._glows.popleft()
        self._elapsed += dt_ms / 1000.0

    def draw(self, painter: QPainter):
        if len(self._glows) < 1:
            return
        painter.setRenderHint(QPainter.RenderHint.Antialiasing)
        now = time.perf_counter()

        pts = list(self._glows)
        # 密度控制绘制间距：密度越大跳过越少点，光斑越密
        step = max(1, 12 - self.density)  # density=1→skip 11, density=10→skip 2

        for i in range(0, len(pts), step):
            px, py, t = pts[i]
            age = now - t
            ratio = max(0.0, 1.0 - age / self.fade_duration)
            if ratio <= 0.01:
                continue

            r = self.glow_radius * ratio
            alpha = int(255 * ratio * self.opacity)

            if self.color_mode == "rainbow":
                hue = (self._elapsed * 0.3 + i * 0.05) % 1.0
                color = self._hue_color(hue, alpha)
            else:
                color = QColor(self.fixed_color)
                color.setAlpha(alpha)

            g = QRadialGradient(QPointF(px, py), r)
            core = QColor(color)
            core.setAlpha(alpha)
            edge = QColor(0, 0, 0, 0)
            g.setColorAt(0, core)
            g.setColorAt(1, edge)

            painter.setPen(Qt.PenStyle.NoPen)
            painter.setBrush(QBrush(g))
            painter.drawEllipse(QPointF(px, py), r, r)
