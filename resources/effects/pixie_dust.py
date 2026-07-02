"""Pixie Dust — 闪光粒子拖尾（菱形粒子）"""

import random
import math
from PySide6.QtCore import Qt, QPointF
from PySide6.QtGui import QPainter, QColor, QBrush, QPolygonF, QPolygonF
from resources.effects import TrailEffect


class PixieDustEffect(TrailEffect):
    """菱形闪光粒子 — 向上飘动 + 旋转"""

    @staticmethod
    def effect_id() -> str:
        return "pixie_dust"

    @staticmethod
    def effect_name() -> str:
        return "像素尘埃"

    def default_settings(self) -> dict:
        return {
            "fade_duration": 1.0,
            "density": 6,
            "opacity": 0.5,
            "color_mode": "rainbow",
            "fixed_color": "#FF0000",
            "spawn_interval_ms": 20,
            "drift_speed": 50.0,
            "particle_size": 5.0,
            "rainbow_speed": 0.5,
            "spread": 0.5,
        }

    def apply_settings(self, settings: dict):
        self.fade_duration = settings.get("fade_duration", 1.0)
        self.density = settings.get("density", 6)
        self.opacity = settings.get("opacity", 0.5)
        self.color_mode = settings.get("color_mode", "rainbow")
        self.fixed_color = QColor(settings.get("fixed_color", "#FF0000"))
        self.spawn_interval_ms = settings.get("spawn_interval_ms", 20)
        self.drift_speed = settings.get("drift_speed", 50.0)
        self.particle_size = settings.get("particle_size", 5.0)
        self.rainbow_speed = settings.get("rainbow_speed", 0.5)
        self.spread = settings.get("spread", 0.5)

    def create_particles(self, x: float, y: float, count: int):
        for _ in range(count):
            # 在鼠标周围以 particle_size 为半径的圆内随机生成
            angle = random.uniform(0, math.pi * 2)
            dist = random.uniform(0, self.particle_size * 3)
            px = x + math.cos(angle) * dist
            py = y + math.sin(angle) * dist
            self._particles.append({
                "x": px,
                "y": py,
                "vx": random.uniform(-self.spread * self.drift_speed, self.spread * self.drift_speed),
                "vy": -random.uniform(0.5, 1.0) * self.drift_speed,
                "life": self.fade_duration * random.uniform(0.5, 1.0),
                "max_life": self.fade_duration,
                "size": self.particle_size * random.uniform(0.6, 1.4),
                "hue": random.random(),
                "opacity": self.opacity,
                "angle": random.uniform(0, math.pi * 2),
                "spin": random.uniform(-2.0, 2.0),
            })

    def update_particle(self, p: dict, dt: float):
        p["x"] += p["vx"] * dt
        p["y"] += p["vy"] * dt
        p["vy"] += random.uniform(-10, 10) * dt
        p["angle"] += p["spin"] * dt

    def draw_particle(self, painter: QPainter, p: dict):
        alpha = self._life_alpha(p)
        if alpha <= 0:
            return

        # 随生命缩小
        life_ratio = p["life"] / p["max_life"]
        size = p["size"] * life_ratio
        if size < 0.3:
            return
        cx, cy = p["x"], p["y"]

        if self.color_mode == "rainbow":
            hue = (p["hue"] + self._elapsed * self.rainbow_speed * 0.1) % 1.0
            color = self._hue_color(hue, alpha)
        else:
            color = QColor(self.fixed_color)
            color.setAlpha(alpha)

        # 叠加混合 + 三层辉光 → 模拟参考案例的 GPU additive blending
        painter.save()
        painter.setCompositionMode(QPainter.CompositionMode.CompositionMode_Plus)
        painter.setPen(Qt.PenStyle.NoPen)

        # 外层辉光（大圆，极淡）
        glow = QColor(color)
        glow.setAlpha(int(alpha * 0.12))
        painter.setBrush(QBrush(glow))
        painter.drawEllipse(QPointF(cx, cy), size * 2.8, size * 2.8)

        # 内层辉光
        glow2 = QColor(color)
        glow2.setAlpha(int(alpha * 0.3))
        painter.setBrush(QBrush(glow2))
        painter.drawEllipse(QPointF(cx, cy), size * 1.4, size * 1.4)

        # 菱形核心
        painter.setBrush(QBrush(color))
        painter.translate(QPointF(cx, cy))
        painter.rotate(math.degrees(p["angle"]))
        half = size
        diamond = QPolygonF([
            QPointF(0, -half),
            QPointF(half, 0),
            QPointF(0, half),
            QPointF(-half, 0),
        ])
        painter.drawPolygon(diamond)
        painter.restore()
