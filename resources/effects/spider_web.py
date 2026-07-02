"""蛛网 — 光标与随机锚点的连线（带淡出和随机粗细）"""

import random
import math
import time
from PySide6.QtCore import Qt, QPointF
from PySide6.QtGui import QPainter, QColor, QPen
from resources.effects import TrailEffect


class SpiderWebEffect(TrailEffect):
    """蛛网 — 每条线有生命周期，过期后替换，随机粗细"""

    @staticmethod
    def effect_id() -> str:
        return "spider_web"

    @staticmethod
    def effect_name() -> str:
        return "蛛丝缠网"

    def default_settings(self) -> dict:
        return {"fade_duration": 1.5, "opacity": 0.5,
                "color_mode": "rainbow", "fixed_color": "#AAAAAA"}

    def apply_settings(self, settings: dict):
        self.fade_duration = settings.get("fade_duration", 1.5)
        self.opacity = settings.get("opacity", 0.5)
        self.color_mode = settings.get("color_mode", "rainbow")
        self.fixed_color = QColor(settings.get("fixed_color", "#AAAAAA"))
        self.max_width = settings.get("particle_size", 8)
        self.connect_distance = 250
        self.anchor_count = max(10, settings.get("density", 6) * 5)
        self._anchors: list[dict] = []
        self._need_refresh = True

    def start(self):
        self._anchors.clear()
        self._need_refresh = True
        self._has_last = False

    def stop(self):
        self._anchors.clear()

    def _create_anchor(self):
        from PySide6.QtWidgets import QApplication
        screen = QApplication.primaryScreen().geometry()
        return {
            "ax": random.randint(0, screen.width()),
            "ay": random.randint(0, screen.height()),
            "width": random.uniform(1.0, max(1.0, self.max_width)),
            "born": time.perf_counter(),
            "life": self.fade_duration * random.uniform(0.8, 1.2),
        }

    def _fill_anchors(self):
        while len(self._anchors) < self.anchor_count:
            self._anchors.append(self._create_anchor())

    def on_mouse_move(self, x: int, y: int):
        if self._need_refresh:
            self._fill_anchors()
            self._need_refresh = False
        self._last_x, self._last_y = x, y
        self._has_last = True

    def update(self, dt_ms: float):
        now = time.perf_counter()
        self._anchors = [a for a in self._anchors if now - a["born"] < a["life"]]
        self._fill_anchors()
        self._elapsed += dt_ms / 1000.0

    def draw(self, painter: QPainter):
        if not self._has_last or not self._anchors:
            return
        painter.setRenderHint(QPainter.RenderHint.Antialiasing)

        cx, cy = self._last_x, self._last_y
        now = time.perf_counter()

        if self.color_mode == "rainbow":
            hue = (self._elapsed * 0.2) % 1.0
            alpha = int(255 * self.opacity)
            base_color = self._hue_color(hue, alpha)
        else:
            base_color = QColor(self.fixed_color)
            base_color.setAlpha(int(255 * self.opacity))

        for a in self._anchors:
            dist = math.hypot(a["ax"] - cx, a["ay"] - cy)
            if dist >= self.connect_distance:
                continue
            ratio = 1.0 - dist / self.connect_distance
            life_ratio = max(0.0, 1.0 - (now - a["born"]) / a["life"])
            alpha = int(255 * self.opacity * ratio * life_ratio)
            if alpha <= 0:
                continue
            c = QColor(base_color)
            c.setAlpha(alpha)
            painter.setPen(QPen(c, a["width"]))
            painter.drawLine(QPointF(cx, cy), QPointF(a["ax"], a["ay"]))
