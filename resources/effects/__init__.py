"""鼠标拖尾特效框架"""

import random
import math
from PySide6.QtCore import QTimer, QPointF
from PySide6.QtGui import QPainter, QColor, QPen, QBrush
from abc import ABC, abstractmethod


class TrailEffect(ABC):
    """拖尾特效基类"""

    def __init__(self):
        super().__init__()
        self._particles: list[dict] = []
        self._last_x = 0
        self._last_y = 0
        self._has_last = False
        self._elapsed = 0.0
        self._last_spawn = 0.0

    # ── 必须实现 ──

    @staticmethod
    @abstractmethod
    def effect_id() -> str:
        """唯一标识"""
        ...

    @staticmethod
    @abstractmethod
    def effect_name() -> str:
        """显示名称"""
        ...

    @abstractmethod
    def default_settings(self) -> dict:
        """返回默认参数"""
        ...

    @abstractmethod
    def apply_settings(self, settings: dict):
        """应用参数"""
        ...

    # ── 可选覆盖（粒子类特效） ──

    def create_particles(self, x: float, y: float, count: int):
        """生成粒子（粒子类特效覆盖）"""
        pass

    def update_particle(self, p: dict, dt: float):
        """更新单个粒子（粒子类特效覆盖）"""
        pass

    def draw_particle(self, painter: QPainter, p: dict):
        """绘制单个粒子（粒子类特效覆盖）"""
        pass

    # ── 通用逻辑（可覆盖） ──

    def start(self):
        self._particles.clear()
        self._has_last = False

    def stop(self):
        self._particles.clear()

    def on_mouse_move(self, x: int, y: int):
        """默认：基于时间间隔生成粒子。丝带类特效可覆盖全部逻辑"""
        import time
        now = time.perf_counter()
        if not self._has_last:
            self._last_x = x
            self._last_y = y
            self._last_spawn = now
            self._has_last = True
            return
        interval = getattr(self, 'spawn_interval_ms', 16) / 1000.0
        elapsed = now - self._last_spawn
        if elapsed >= interval:
            batches = min(int(elapsed / interval), 4)
            for _ in range(batches):
                self.create_particles(x, y, getattr(self, 'density', 10))
            self._last_spawn = now
        self._last_x = x
        self._last_y = y

    def update(self, dt_ms: float):
        """默认：更新粒子生命周期并逐一 update_particle。可覆盖"""
        dt = dt_ms / 1000.0
        self._elapsed += dt
        self._particles = [p for p in self._particles if p["life"] > 0]
        for p in self._particles:
            p["life"] -= dt
            self.update_particle(p, dt)

    def draw(self, painter: QPainter):
        """默认：逐一 draw_particle。可覆盖"""
        painter.setRenderHint(QPainter.RenderHint.Antialiasing)
        for p in self._particles:
            self.draw_particle(painter, p)

    # ── 粒子工具 ──

    @staticmethod
    def _life_alpha(p: dict) -> int:
        """基于剩余生命返回 alpha 值"""
        ratio = max(0.0, min(1.0, p["life"] / p["max_life"]))
        return int(255 * ratio * p.get("opacity", 1.0))

    @staticmethod
    def _hue_color(hue: float, alpha: int, sat: float = 1.0, val: float = 1.0) -> QColor:
        """HSV → QColor"""
        h = hue % 1.0
        c = QColor()
        c.setHsvF(h, sat, val)
        c.setAlpha(alpha)
        return c
