"""按键显示核心逻辑"""

import time
from enum import Enum
from PySide6.QtCore import QObject, Signal, QTimer
from PySide6.QtGui import QColor

from curseen.key_display import keymap


class NotificationPlacement(Enum):
    BOTTOM_LEFT = "bottom_left"
    BOTTOM_RIGHT = "bottom_right"
    TOP_LEFT = "top_left"
    TOP_RIGHT = "top_right"


class KeyItem:
    """按键项"""

    MAX_LINES = 200

    def __init__(self, text: str, timestamp: float = None, is_char: bool = False):
        self.groups = [(text, 1)]
        self.timestamp = timestamp or time.time()
        self.is_char = is_char
        self.opacity = 1.0
        self.is_fading = False

    @property
    def display_text(self) -> str:
        parts = []
        for text, count in self.groups:
            parts.append(text)
            if count > 1:
                parts.append(f" x {count} ")
        return "".join(parts)

    def merge(self, other: "KeyItem") -> bool:
        other_text = other.groups[0][0]
        if self.groups:
            last = self.groups[-1][0]
            if last == other_text or last.lstrip() == other_text or last == " " + other_text:
                text, count = self.groups[-1]
                self.groups[-1] = (text, count + 1)
                self.timestamp = other.timestamp
                return True
        self.groups.append((" " + other_text, 1))
        self.timestamp = other.timestamp
        if len(self.groups) > self.MAX_LINES * 2:
            cut = max(10, len(self.groups) // 4)
            self.groups = self.groups[cut:]
        return True

    def _count_groups(self) -> int:
        return len(self.groups)


class KeyController(QObject):
    """按键显示控制器"""

    items_updated = Signal()
    settings_changed = Signal()

    FADE_DURATION = 0.5
    REMOVE_DELAY = 1.0

    def __init__(self, parent=None):
        super().__init__(parent)
        self._items: list[KeyItem] = []
        self._active_item: KeyItem | None = None
        self._modifiers: dict[str, bool] = {}
        self._modifier_times: dict[str, float] = {}

        self._enabled = True
        self._placement = NotificationPlacement.BOTTOM_RIGHT
        self._top_offset = 0
        self._bottom_offset = 10
        self._left_offset = 0
        self._right_offset = 0
        self._popup_width = 15
        self._opacity = 1.0
        self._fade_delay = 5
        self._font_size = 18
        self._font_color = QColor("#ffff00")
        self._symbol_color = QColor("#ffffff")
        self._fun_color = QColor("#ff0000")
        self._bg_color = QColor(0, 0, 0, 200)
        self._display_mode = "all"
        self._merge_interval = 2.0

        self._fade_timer = QTimer(self)
        self._fade_timer.setInterval(50)
        self._fade_timer.timeout.connect(self._tick)

    # ── 属性 ──

    @property
    def enabled(self): return self._enabled
    @enabled.setter
    def enabled(self, v):
        self._enabled = v
        if not v:
            self._active_item = None; self._items.clear(); self.items_updated.emit()

    @property
    def placement(self): return self._placement
    @placement.setter
    def placement(self, v): self._placement = v; self.settings_changed.emit()

    @property
    def top_offset(self): return self._top_offset
    @top_offset.setter
    def top_offset(self, v): self._top_offset = max(0, v)

    @property
    def bottom_offset(self): return self._bottom_offset
    @bottom_offset.setter
    def bottom_offset(self, v): self._bottom_offset = max(0, v)

    @property
    def left_offset(self): return self._left_offset
    @left_offset.setter
    def left_offset(self, v): self._left_offset = max(0, v)

    @property
    def right_offset(self): return self._right_offset
    @right_offset.setter
    def right_offset(self, v): self._right_offset = max(0, v)

    @property
    def popup_width(self): return self._popup_width
    @popup_width.setter
    def popup_width(self, v): self._popup_width = max(5, min(50, v))

    @property
    def opacity(self): return self._opacity
    @opacity.setter
    def opacity(self, v): self._opacity = max(0.05, min(1.0, v))

    @property
    def fade_delay(self): return self._fade_delay
    @fade_delay.setter
    def fade_delay(self, v): self._fade_delay = max(0.5, min(30.0, v))

    @property
    def font_size(self): return self._font_size
    @font_size.setter
    def font_size(self, v): self._font_size = max(8, min(72, v))

    @property
    def font_color(self): return self._font_color
    @font_color.setter
    def font_color(self, v): self._font_color = v; self.settings_changed.emit()

    @property
    def fun_color(self): return self._fun_color
    @fun_color.setter
    def fun_color(self, v): self._fun_color = v; self.settings_changed.emit()

    @property
    def symbol_color(self): return self._symbol_color
    @symbol_color.setter
    def symbol_color(self, v): self._symbol_color = v; self.settings_changed.emit()

    @property
    def bg_color(self): return self._bg_color
    @bg_color.setter
    def bg_color(self, v): self._bg_color = v; self.settings_changed.emit()

    @property
    def display_mode(self): return self._display_mode
    @display_mode.setter
    def display_mode(self, v): self._display_mode = v

    @property
    def show_space_unicode(self): return self._show_space_unicode
    @show_space_unicode.setter
    def show_space_unicode(self, v): self._show_space_unicode = v

    @property
    def show_app_icon(self): return self._show_app_icon
    @show_app_icon.setter
    def show_app_icon(self, v): self._show_app_icon = v

    @property
    def merge_interval(self): return self._merge_interval
    @merge_interval.setter
    def merge_interval(self, v): self._merge_interval = max(0.1, min(30.0, v))

    @property
    def items(self) -> list:
        result = list(self._items)
        if self._active_item:
            result.append(self._active_item)
        return result

    # ── 配置 ──

    def load_settings(self, settings: dict):
        mapping = {
            "top_offset": "top_offset", "bottom_offset": "bottom_offset",
            "left_offset": "left_offset", "right_offset": "right_offset",
            "popup_width": "popup_width", "opacity": "opacity",
            "fade_delay": "fade_delay", "font_size": "font_size",
            "shortcuts_only": "shortcuts_only", "only_modifiers": "only_modifiers",
            "merge_interval": "merge_interval",
            "display_mode": "display_mode",
        }
        for key, attr in mapping.items():
            if key in settings:
                setattr(self, attr, settings[key])
        if "placement" in settings:
            try:
                self._placement = NotificationPlacement(settings["placement"])
            except ValueError:
                pass
        for color_key, attr in [("font_color", "_font_color"), ("fun_color", "_fun_color"), ("symbol_color", "_symbol_color"), ("bg_color", "_bg_color")]:
            if color_key in settings:
                c = settings[color_key]
                if isinstance(c, (list, tuple)):
                    setattr(self, attr, QColor(*c))
                else:
                    setattr(self, attr, QColor(c))

    def to_dict(self) -> dict:
        return {
            "placement": self._placement.value,
            "top_offset": self._top_offset,
            "bottom_offset": self._bottom_offset,
            "left_offset": self._left_offset,
            "right_offset": self._right_offset,
            "popup_width": self._popup_width,
            "opacity": self._opacity,
            "fade_delay": self._fade_delay,
            "font_size": self._font_size,
            "font_color": (self._font_color.red(), self._font_color.green(), self._font_color.blue(), self._font_color.alpha()),
            "fun_color": (self._fun_color.red(), self._fun_color.green(), self._fun_color.blue(), self._fun_color.alpha()),
            "symbol_color": (self._symbol_color.red(), self._symbol_color.green(), self._symbol_color.blue(), self._symbol_color.alpha()),
            "bg_color": (self._bg_color.red(), self._bg_color.green(), self._bg_color.blue(), self._bg_color.alpha()),
            "display_mode": self._display_mode,
            "merge_interval": self._merge_interval,
        }

    # ── 按键处理 ──

    def on_key_pressed(self, key_str: str):
        if not self._enabled:
            return

        now = time.time()

        # 按键调试日志（取消注释可记录所有按键）
        # with open("E:/tmp/Reasonix/CurSeen/CurSeen/keys_debug.log", "a") as f:
        #    f.write(f"{time.time():.3f} PRESS: {key_str}\n")

        stuck_mods = [m for m, t in self._modifier_times.items() if now - t > 5.0]
        for m in stuck_mods:
            self._modifiers.pop(m, None)
            self._modifier_times.pop(m, None)

        lookup = keymap.lookup(key_str)
        if lookup:
            display, ktype, is_fun = lookup
            if ktype == keymap.MOD:
                self._modifiers[display] = True
                self._modifier_times[display] = now
                return

        parts = self._resolve_key_text(key_str)
        if not parts:
            return

        has_mod = bool(self._modifiers.get("Ctrl") or self._modifiers.get("Alt") or self._modifiers.get("Win") or self._modifiers.get("Shift") or self._modifiers.get("Menu"))

        # 获取 is_fun（用于显示模式过滤）
        is_fun = False
        if self._display_mode != "all":
            from curseen.key_display.keymap import lookup
            r = lookup(key_str)
            if r:
                _, _, is_fun = r

        # 显示模式过滤
        if self._display_mode == "shortcuts_fun" and not (has_mod or is_fun):
            return
        if self._display_mode == "shortcuts" and not has_mod:
            return

        display = "".join(parts) if has_mod else parts[0]
        is_char = not has_mod and len(display) == 1

        # 弹窗逻辑
        if self._active_item and self._active_item.opacity < 1.0:
            self._items.append(self._active_item)
            self._active_item = KeyItem(display, is_char=is_char)
        elif has_mod and self._active_item:
            last_raw = self._active_item.groups[0][0] if self._active_item.groups else ""
            time_since_last = now - self._active_item.timestamp
            if last_raw == display and time_since_last < self._merge_interval and not self._active_item.is_fading:
                self._active_item.merge(KeyItem(display, is_char=False))
                self._active_item.timestamp = now
                self._active_item.opacity = 1.0
            else:
                self._items.append(self._active_item)
                self._active_item = KeyItem(display, is_char=is_char)
        elif self._active_item and self._active_item.display_text.startswith(("Ctrl +", "Alt +", "Shift +", "Win +", "Menu +")):
            self._items.append(self._active_item)
            self._active_item = KeyItem(display, is_char=is_char)
        elif self._active_item:
            time_since_last = now - self._active_item.timestamp
            if time_since_last < self._merge_interval:
                self._active_item.merge(KeyItem(display, is_char=is_char))
                self._active_item.timestamp = now
                self._active_item.opacity = 1.0
                if self._fade_timer.isActive():
                    self._fade_timer.stop()
            else:
                self._items.append(self._active_item)
                self._active_item = KeyItem(display, is_char=is_char)
        else:
            self._active_item = KeyItem(display, is_char=is_char)

        if not self._fade_timer.isActive():
            self._fade_timer.start()
        self.items_updated.emit()

    def on_key_released(self, key_str: str):
        lookup = keymap.lookup(key_str)
        if lookup:
            display, ktype, is_fun = lookup
            if ktype == keymap.MOD:
                self._modifiers[display] = False
                self._modifier_times.pop(display, None)

    def _resolve_key_text(self, key_str: str) -> list | None:
        parts = []
        for mod in ("Ctrl", "Alt", "Shift", "Win", "Menu"):
            if self._modifiers.get(mod):
                parts.append(mod)
        is_shortcut = bool(parts)
        lookup = keymap.lookup(key_str)
        if lookup is None:
            return None
        display, ktype, is_fun = lookup
        if is_shortcut:
            display = display.upper() if len(display) == 1 else display
            result = []
            for i, mod in enumerate(parts):
                if i > 0:
                    result.append(" + ")
                result.append(mod)
            result.append(" + ")
            result.append(display)
            return result
        else:
            return [display]

    def _tick(self):
        changed = False
        now_t = time.time()
        stuck = [m for m, t in self._modifier_times.items() if now_t - t > 5.0]
        for m in stuck:
            self._modifiers.pop(m, None); self._modifier_times.pop(m, None)
        for item in list(self._items):
            age = now_t - item.timestamp
            if not hasattr(item, 'is_fading') or not item.is_fading:
                item.is_fading = False
            if not item.is_fading and age >= self._fade_delay:
                item.is_fading = True
            if item.is_fading:
                fa = age - self._fade_delay
                if fa < self.FADE_DURATION:
                    item.opacity = max(0.0, 1.0 - fa / self.FADE_DURATION)
                else:
                    ra = fa - self.FADE_DURATION
                    if ra >= self.REMOVE_DELAY:
                        self._items.remove(item); changed = True; continue
            changed = True
        if self._active_item:
            age = now_t - self._active_item.timestamp
            if age >= self._fade_delay:
                fa = age - self._fade_delay
                if fa < self.FADE_DURATION:
                    self._active_item.opacity = max(0.0, 1.0 - fa / self.FADE_DURATION)
                else:
                    ra = fa - self.FADE_DURATION
                    if ra >= self.REMOVE_DELAY:
                        self._active_item = None; changed = True
                changed = True
        if changed:
            self.items_updated.emit()
        if not self._items and not self._active_item:
            self._fade_timer.stop()
