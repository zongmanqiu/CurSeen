"""圆形粒子 — 彩虹爆发粒子（仿 MFCMouseEffect chromatic 模式）"""

import random
import math
from PySide6.QtCore import Qt, QPointF
from PySide6.QtGui import QPainter, QColor, QBrush
from resources.effects import TrailEffect


class CircleParticles(TrailEffect):
    """彩色圆粒 — 向四周爆发 + 重力下落 + 缩小淡出"""

    @staticmethod
    def effect_id() -> str:
        return "circle_particles"

    @staticmethod
    def effect_name() -> str:
        return "律动粒子"

    def default_settings(self) -> dict:
        return {
            "fade_duration": 1.2,
            "density": 4,
            "opacity": 0.7,
            "particle_size": 6.0,
            "spawn_interval_ms": 16,
            "color_mode": "rainbow",
            "fixed_color": "#FF0000",
            "spread_speed": 120.0,    # 粒子飞出速度
            "gravity": 30.0,           # 重力加速度 px/s²
        }

    def apply_settings(self, settings: dict):
        self.fade_duration = settings.get("fade_duration", 1.2)
        self.density = settings.get("density", 4)
        self.opacity = settings.get("opacity", 0.7)
        self.particle_size = settings.get("particle_size", 6.0)
        self.spawn_interval_ms = settings.get("spawn_interval_ms", 16)
        self.color_mode = settings.get("color_mode", "rainbow")
        self.fixed_color = QColor(settings.get("fixed_color", "#FF0000"))
        self.spread_speed = settings.get("spread_speed", 120.0)
        self.gravity = settings.get("gravity", 30.0)

    def create_particles(self, x: float, y: float, count: int):
        for _ in range(count):
            angle = random.uniform(0, math.pi * 2)
            speed = random.uniform(0.3, 1.0) * self.spread_speed
            vx = math.cos(angle) * speed
            vy = math.sin(angle) * speed
            self._particles.append({
                "x": x + random.uniform(-3, 3),
                "y": y + random.uniform(-3, 3),
                "vx": vx,
                "vy": vy,
                "life": self.fade_duration * random.uniform(0.4, 1.0),
                "max_life": self.fade_duration,
                "size": self.particle_size * random.uniform(0.5, 1.0),
                "hue": random.random(),
                "opacity": self.opacity,
            })

    def update_particle(self, p: dict, dt: float):
        p["x"] += p["vx"] * dt
        p["y"] += p["vy"] * dt
        p["vy"] += self.gravity * dt  # 重力

    def draw_particle(self, painter: QPainter, p: dict):
        alpha = self._life_alpha(p)
        if alpha <= 0:
            return

        r = p["size"] * (p["life"] / p["max_life"])  # 随生命缩小
        if r < 0.3:
            return

        cx, cy = p["x"], p["y"]

        if self.color_mode == "rainbow":
            hue = p["hue"] % 1.0
            color = self._hue_color(hue, alpha)
        else:
            color = QColor(self.fixed_color)
            color.setAlpha(alpha)

        painter.setPen(Qt.PenStyle.NoPen)
        painter.setBrush(QBrush(color))
        painter.drawEllipse(QPointF(cx, cy), r, r)
