"""配置管理：JSON 持久化用户偏好"""

import json
import os
from pathlib import Path


def _config_dir() -> Path:
    """返回配置文件目录 %APPDATA%/CurSeen"""
    return Path(os.environ.get("APPDATA", ".")) / "CurSeen"


def _config_path() -> Path:
    return _config_dir() / "config.json"


DEFAULT_CONFIG = {
    "trail": {
        "cursor_mode": 0, "cursor_outer": 60, "cursor_arc": 5,
        "cursor_color": "#80ff0000",
        "sound_volume": 50,
        "ripple_size": 100,
        "ripple_duration": 300,
        "trail_effect": "pixie_dust",
        "trail_enabled": False,
        "trail_settings": {
            "fade_duration": 1.0,
            "density": 5,
            "opacity": 0.5,
            "spawn_interval_ms": 100,
            "particle_size": 5,
            "color_mode": "rainbow",
            "fixed_color": "#FFFF00",
        },
        "ripple_left_enabled": False, "ripple_left_color": "#FFFFFF00",
        "ripple_right_enabled": False, "ripple_right_color": "#FF0000FF",
        "sound_left_enabled": False,
        "sound_right_enabled": False,
    },
    "key_display": {
        "placement": "bottom_right",
        "top_offset": 0, "bottom_offset": 10,
        "left_offset": 0, "right_offset": 0,
        "popup_width": 15, "opacity": 1.0,
        "fade_delay": 10, "font_size": 16,
        "font_color": [255, 255, 0, 255],
        "fun_color": [255, 0, 0, 255],
        "symbol_color": [255, 255, 255, 255],
        "bg_color": [0, 0, 0, 255],
        "merge_interval": 1.5,
        "display_mode": "all",
    },
    "drawing": {"pen_color": "#FF0000", "pen_width": 4},
    "general": {"auto_start": False, "overlay_opacity": 1.0},
    "ui": {"font_size": 16},
    "global": {
        "panel_hotkey": "",
        "enabled_trail": False,
        "enabled_key": False,
        "enabled_draw": False,
    },
    "hotkeys": {"toggle_drawing": "Ctrl+Shift+D"},
    "drawing": {
        "hotkeys": {
            "draw_hk_screen": "Alt+G",
            "draw_hk_white": "Alt+W",
            "draw_hk_pen1": "1",
            "draw_hk_pen2": "2",
            "draw_hk_pen3": "3",
            "draw_hk_eraser": "4",
            "draw_hk_line": "5",
            "draw_hk_arrow": "6",
            "draw_hk_rect": "7",
            "draw_hk_ellipse": "8",
            "draw_hk_text": "9",
        },
        "pens": [
            {"color": "#FF000000", "width": 4, "style": "solid"},
            {"color": "#40FF0000", "width": 30, "style": "solid"},
            {"color": "#FF00FF00", "width": 4, "style": "dashed"},
        ],
    },
}


class Config:
    """JSON 配置文件管理器"""

    def __init__(self):
        self.data = json.loads(json.dumps(DEFAULT_CONFIG))

    def load(self):
        path = _config_path()
        if path.exists():
            with open(path, "r", encoding="utf-8") as f:
                stored = json.load(f)
                self._merge(self.data, stored)
        else:
            self.save()

    def save(self):
        path = _config_dir()
        path.mkdir(parents=True, exist_ok=True)
        with open(_config_path(), "w", encoding="utf-8") as f:
            json.dump(self.data, f, ensure_ascii=False, indent=2)

    def reset_to_defaults(self):
        """删除 config.json + 内存重置为 DEFAULT_CONFIG"""
        path = _config_path()
        if path.exists():
            path.unlink()
        self.data = json.loads(json.dumps(DEFAULT_CONFIG))

    def set_and_save(self, keys: list, value):
        """深路径覆值 + 即时写盘。例: set_and_save([\"trail\",\"cursor_outer\"], 40)"""
        d = self.data
        for k in keys[:-1]:
            d = d.setdefault(k, {})
        d[keys[-1]] = value
        self.save()

    @staticmethod
    def _merge(base, override):
        for k, v in override.items():
            if isinstance(v, dict) and isinstance(base.get(k), dict):
                Config._merge(base[k], v)
            else:
                base[k] = v
