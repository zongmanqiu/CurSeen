"""鼠标特效控制器"""

import os
from PySide6.QtCore import QObject, Signal, QUrl, QTimer
from PySide6.QtGui import QColor
from PySide6.QtMultimedia import QSoundEffect

from curseen.trail.hover_dot import HoverDot
from curseen.trail.ripples import RippleManager
from resources.effects import TrailEffect
from resources.effects.pixie_dust import PixieDustEffect
from resources.effects.circle_particles import CircleParticles
from resources.effects.ribbon import RibbonTrail
from resources.effects.spider_web import SpiderWebEffect
from resources.effects.glow import GlowEffect


EFFECT_REGISTRY: dict[str, type] = {
    "pixie_dust": PixieDustEffect,
    "circle_particles": CircleParticles,
    "spider_web": SpiderWebEffect,
    "glow": GlowEffect,
    "ribbon": RibbonTrail,
}


class TrailController(QObject):
    """管理悬浮标、点击声音、点击波纹"""

    _AUDIO_DIR = os.path.normpath(os.path.join(os.path.dirname(os.path.dirname(__file__)), "..", "resources", "audio"))

    def __init__(self, parent=None):
        super().__init__(parent)
        self._hover_dot = HoverDot()
        self._ripples = RippleManager()
        self._enabled = False
        self._mode = 0
        self._last_x = 0
        self._last_y = 0
        self._config = None
        self._sound_cache: dict[str, QSoundEffect] = {}
        self._volume = 0.5
        self._ripple_size = 100
        self._ripple_duration = 300
        self._trail_effect: TrailEffect | None = None
        self._trail_enabled = False
        self._trail_timer = QTimer()
        self._trail_timer.setInterval(16)
        self._trail_timer.timeout.connect(self._on_trail_tick)
        self._trail_last_tick = 0
        self._on_update = None
        self._loading = False

    @property
    def hover_dot(self) -> HoverDot:
        return self._hover_dot

    @property
    def ripple_manager(self) -> RippleManager:
        return self._ripples

    def set_mode(self, mode: int):
        """0 = 关闭, 2 = 开启（空心圆）"""
        self._mode = mode
        self._hover_dot.set_mode(mode)
        self._hover_dot.hide()  # 始终隐藏独立窗口，由 overlay 绘制
        if self._on_update:
            self._on_update()
        self._save("cursor_mode", mode)

    def set_size(self, size: int):
        self._hover_dot.set_size(size)
        self._hover_dot.move_to(self._last_x, self._last_y)
        if self._on_update:
            self._on_update()
        self._save("cursor_outer", size)

    def set_arc_width(self, width: int):
        """圆弧宽度 → 内比例"""
        width = min(width, self._hover_dot._size // 2)
        self._hover_dot.set_inner_ratio(
            max(0.0, 1.0 - width * 2 / max(self._hover_dot._size, 1))
        )
        self._hover_dot.move_to(self._last_x, self._last_y)
        if self._on_update:
            self._on_update()
        self._save("cursor_arc", width)

    def set_color(self, color: QColor):
        self._hover_dot.set_color(color)
        if self._on_update:
            self._on_update()
        self._save("cursor_color", color.name(QColor.NameFormat.HexArgb))

    def set_volume(self, vol: int):
        """0–100 → 0.0–1.0，更新所有缓存的音效"""
        self._volume = vol / 100.0
        for e in self._sound_cache.values():
            e.setVolume(self._volume)
        self._save("sound_volume", vol)

    def set_ripple_size(self, size: int):
        self._ripple_size = size
        self._save("ripple_size", size)

    def set_ripple_duration(self, ms: int):
        self._ripple_duration = ms
        self._save("ripple_duration", ms)

    def set_trail_effect(self, effect_id: str, settings: dict | None = None):
        """切换拖尾特效（不影响 _trail_enabled，由全局开关控制）"""
        cls = EFFECT_REGISTRY.get(effect_id)
        if cls is None:
            if self._trail_effect:
                self._trail_effect.stop()
            self._trail_effect = None
            self._trail_timer.stop()
            if self._on_update:
                self._on_update()
            return
        if self._trail_effect:
            self._trail_effect.stop()
        self._trail_effect = cls()
        self._trail_effect.apply_settings(settings or {})
        if self._trail_enabled:
            self._trail_effect.start()
            import time
            self._trail_last_tick = time.perf_counter()
            self._trail_timer.start()
        self._save("trail_effect", effect_id)

    def set_trail_enabled(self, enabled: bool):
        self._trail_enabled = enabled
        if enabled and self._trail_effect:
            self._trail_effect.start()
            import time
            self._trail_last_tick = time.perf_counter()
            self._trail_timer.start()
        else:
            self._trail_timer.stop()
            if self._trail_effect:
                self._trail_effect.stop()
            if self._on_update:
                self._on_update()
        self._save("trail_enabled", enabled)

    def set_trail_settings(self, settings: dict):
        if self._trail_effect:
            self._trail_effect.apply_settings(settings)
        self._save("trail_settings", settings)

    @property
    def trail_effect(self) -> TrailEffect | None:
        return self._trail_effect

    def set_trail_update_callback(self, cb):
        self._on_update = cb

    def _on_trail_tick(self):
        if not self._trail_effect or not self._trail_enabled:
            return
        import time
        now = time.perf_counter()
        dt = (now - self._trail_last_tick) * 1000
        self._trail_last_tick = now
        self._trail_effect.update(dt)
        if self._on_update:
            self._on_update()

    @property
    def config(self):
        return self._config

    def set_config(self, config):
        self._config = config

    def _save(self, key: str, value):
        if self._config and not self._loading:
            self._config.data.setdefault("trail", {})[key] = value
            self._config.save()

    def load_settings(self, settings: dict):
        self._loading = True
        data = settings.get("trail", {})
        self._mode = data.get("cursor_mode", 0)
        self._hover_dot.set_mode(self._mode)
        outer = data.get("cursor_outer", 40)
        self._hover_dot.set_size(outer)
        arc = data.get("cursor_arc", 10)
        self.set_arc_width(arc)
        self._hover_dot.set_color(QColor(data.get("cursor_color", "#ffff0000")))
        self._volume = data.get("sound_volume", 50) / 100.0
        self._ripple_size = data.get("ripple_size", 100)
        self._ripple_duration = data.get("ripple_duration", 300)
        self._loading = False

    def on_mouse_moved(self, x: int, y: int):
        self._last_x = x
        self._last_y = y
        if not self._trail_enabled:
            return
        if self._mode != 0 and self._on_update:
            self._on_update()
        if self._trail_effect and self._trail_enabled:
            self._trail_effect.on_mouse_move(x, y)

    def on_mouse_click(self, btn: str, x: int, y: int):
        """按钮按下 → 播放音效 + 添加波纹"""
        if not self._trail_enabled:
            return
        data = {}
        if self._config:
            data = self._config.data.get("trail", {})

        # 1. 播放音效
        key_sound = f"sound_{btn}_enabled"
        if data.get(key_sound, False):
            file_key = f"sound_{btn}_file"
            file_name = data.get(file_key, "mouseclick1")
            self._play_sound(file_name)

        # 2. 添加波纹
        key_ripple = f"ripple_{btn}_enabled"
        if data.get(key_ripple, False):
            color_key = f"ripple_{btn}_color"
            color = QColor(data.get(color_key, "#ffff00"))
            self._ripples.add(x, y, color, outer=self._ripple_size, hollow=True, duration_ms=self._ripple_duration)

    def _play_sound(self, name: str):
        """播放音效文件（带缓存）"""
        if name not in self._sound_cache:
            path = os.path.join(self._AUDIO_DIR, f"{name}.wav")
            if not os.path.exists(path):
                return
            effect = QSoundEffect()
            effect.setSource(QUrl.fromLocalFile(path))
            effect.setVolume(self._volume)
            self._sound_cache[name] = effect
        effect = self._sound_cache[name]
        effect.stop()
        effect.play()

    def start(self):
        self._ripples.start()

    def stop(self):
        self._ripples.stop()
