"""飘带拖尾 — 鼠标轨迹收束为尖尾丝带（头尾尖版）"""

import math
import time
from collections import deque
from PySide6.QtCore import Qt, QPointF
from PySide6.QtGui import QPainter, QColor, QBrush, QPen, QPainterPath
from resources.effects import TrailEffect


class RibbonTrail(TrailEffect):
    """飘带 — 头尾始终尖点，中间宽度按淡出时间缩放"""

    @staticmethod
    def effect_id() -> str:
        return "ribbon"

    @staticmethod
    def effect_name() -> str:
        return "流光飘带"

    def default_settings(self) -> dict:
        return {"fade_duration": 1.0, "max_width": 12.0, "opacity": 0.6,
                "color_mode": "rainbow", "fixed_color": "#FF0000"}

    def apply_settings(self, settings: dict):
        self.fade_duration = settings.get("fade_duration", 1.0)
        self.max_width = settings.get("max_width", 12.0)
        self.opacity = settings.get("opacity", 0.6)
        self.color_mode = settings.get("color_mode", "rainbow")
        self.fixed_color = QColor(settings.get("fixed_color", "#FF0000"))
        self._points: deque = deque()

    def start(self):
        self._points.clear(); self._has_last = False

    def stop(self):
        self._points.clear()

    def on_mouse_move(self, x: int, y: int):
        now = time.perf_counter()
        if not self._has_last:
            self._has_last = True
        # 跳过过近的点（减少抖动）
        if self._points:
            lx, ly, _ = self._points[-1]
            if math.hypot(x - lx, y - ly) < 2.0:
                return
        self._points.append((x, y, now))
        while len(self._points) > 120:
            self._points.popleft()

    def update(self, dt_ms: float):
        now = time.perf_counter()
        cutoff = now - self.fade_duration
        while self._points and self._points[0][2] < cutoff:
            self._points.popleft()
        self._elapsed += dt_ms / 1000.0

    def draw(self, painter: QPainter):
        if len(self._points) < 3:
            return
        painter.setRenderHint(QPainter.RenderHint.Antialiasing)

        pts = list(self._points)
        half_w = self.max_width / 2
        now = time.perf_counter()

        # 构建左右偏移点（头尾不加偏移，永远尖）
        left_raw = []
        right_raw = []
        alive_count = 0

        for i, (px, py, t) in enumerate(pts):
            age = now - t
            # 宽度按位置线性：尾部=0，头部=max_width
            ratio = i / (len(pts) - 1) if len(pts) > 1 else 0.0
            if ratio <= 0.001:
                continue
            alive_count += 1

            # 方向：直接用前后紧邻点
            if i == 0:
                dx = pts[i + 1][0] - px
                dy = pts[i + 1][1] - py
            elif i == len(pts) - 1:
                dx = px - pts[i - 1][0]
                dy = py - pts[i - 1][1]
            else:
                dx = pts[i + 1][0] - pts[i - 1][0]
                dy = pts[i + 1][1] - pts[i - 1][1]
            length = math.hypot(dx, dy)
            if length < 0.5:
                continue
            nx, ny = -dy / length, dx / length

            w = half_w * (ratio ** 3)
            left_raw.append((px + nx * w, py + ny * w, ratio, i))
            right_raw.append((px - nx * w, py - ny * w, ratio, i))

        if alive_count < 2:
            return

        # 对边界点做 3 点滑动平均去锯齿
        def smooth_boundary(raw):
            if len(raw) < 3:
                return raw
            result = [raw[0]]
            for i in range(1, len(raw) - 1):
                ax = (raw[i - 1][0] + raw[i][0] + raw[i + 1][0]) / 3
                ay = (raw[i - 1][1] + raw[i][1] + raw[i + 1][1]) / 3
                result.append((ax, ay, raw[i][2], raw[i][3]))
            result.append(raw[-1])
            return result

        left_raw = smooth_boundary(left_raw)
        right_raw = smooth_boundary(right_raw)

        # 头尾坐标
        head_x, head_y = pts[-1][0], pts[-1][1]
        tail_x, tail_y = pts[0][0], pts[0][1]

        path = QPainterPath()
        path.setFillRule(Qt.FillRule.WindingFill)

        # 尾尖（单点）
        path.moveTo(QPointF(tail_x, tail_y))

        # 左侧 → 头尖
        if left_raw:
            for i, (lx, ly, ratio, orig_i) in enumerate(left_raw):
                if i == 0:
                    path.lineTo(QPointF(lx, ly))
                elif i < len(left_raw) - 1:
                    cx = (lx + left_raw[i + 1][0]) / 2
                    cy = (ly + left_raw[i + 1][1]) / 2
                    path.quadTo(QPointF(lx, ly), QPointF(cx, cy))
                else:
                    path.lineTo(QPointF(lx, ly))

        # 左→右（直接连接，不用头尖单点；头部改用圆帽）
        if left_raw and right_raw:
            path.lineTo(QPointF(*right_raw[-1][:2]))

        # 右侧 → 尾尖
        if right_raw:
            for i in range(len(right_raw) - 1, -1, -1):
                rx, ry = right_raw[i][0], right_raw[i][1]
                if i == len(right_raw) - 1:
                    path.lineTo(QPointF(rx, ry))
                elif i > 0:
                    cx = (rx + right_raw[i - 1][0]) / 2
                    cy = (ry + right_raw[i - 1][1]) / 2
                    path.quadTo(QPointF(rx, ry), QPointF(cx, cy))
                else:
                    path.lineTo(QPointF(rx, ry))

        path.lineTo(QPointF(tail_x, tail_y))
        path.closeSubpath()
        # 头部圆帽：作为独立子路径合并（用 united 避免透明重叠）
        cap = QPainterPath()
        cap.addEllipse(QPointF(head_x, head_y), half_w, half_w)
        united = path.united(cap)

        if self.color_mode == "rainbow":
            alpha = int(255 * self.opacity)
            color = self._hue_color((self._elapsed * 0.3) % 1.0, alpha)
        else:
            color = QColor(self.fixed_color)
            color.setAlpha(int(255 * self.opacity))

        painter.setPen(Qt.PenStyle.NoPen)
        painter.setBrush(QBrush(color))
        painter.drawPath(united)
