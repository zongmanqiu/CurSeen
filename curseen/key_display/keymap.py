"""按键映射表 — 标准 104 键 + Menu

返回 (显示名, 类型, 是否fun)
  类型: "char" 或 "mod"
  是否fun: True=红色, False=黄色
"""

CHAR = "char"
MOD = "mod"

# ── Key.name → (显示名, 类型, is_fun) ──
KEY_NAMES = {
    # 字母 A-Z
    **{f"Key.{chr(0x61 + i)}": (chr(0x41 + i), CHAR, False) for i in range(26)},

    # 主键盘数字 0-9
    **{f"Key.{i}": (str(i), CHAR, False) for i in range(10)},

    # 修饰键 (mod + fun)
    "Key.ctrl_l": ("Ctrl", MOD, True),
    "Key.ctrl_r": ("Ctrl", MOD, True),
    "Key.shift_l": ("Shift", MOD, True),
    "Key.shift_r": ("Shift", MOD, True),
    "Key.alt_l": ("Alt", MOD, True),
    "Key.alt_r": ("Alt", MOD, True),
    "Key.cmd_l": ("Win", MOD, True),
    "Key.cmd_r": ("Win", MOD, True),
    "Key.ctrl": ("Ctrl", MOD, True),
    "Key.shift": ("Shift", MOD, True),
    "Key.alt": ("Alt", MOD, True),
    "Key.cmd": ("Win", MOD, True),
    "Key.menu": ("Menu", MOD, True),

    # 功能键 F1-F12 (fun)
    **{f"Key.f{i}": (f"F{i}", CHAR, True) for i in range(1, 13)},

    # 特殊功能键 (fun)
    "Key.enter": ("\u21b5", CHAR, True),       # ↵
    "Key.space": ("\u2423", CHAR, False),      # ␣
    "Key.backspace": ("\u2190", CHAR, True),   # ←
    "Key.tab": ("Tab", CHAR, True),
    "Key.esc": ("Esc", CHAR, True),
    "Key.delete": ("Del", CHAR, True),
    "Key.home": ("Home", CHAR, True),
    "Key.end": ("End", CHAR, True),
    "Key.page_up": ("PgUp", CHAR, True),
    "Key.page_down": ("PgDn", CHAR, True),
    "Key.insert": ("Ins", CHAR, True),
    "Key.pause": ("Pause", CHAR, True),
    "Key.print_screen": ("PrtSc", CHAR, True),
    "Key.caps_lock": ("Caps", CHAR, True),
    "Key.num_lock": ("Num", CHAR, True),
    "Key.scroll_lock": ("Scrlk", CHAR, True),

    # 方向键 (fun)
    "Key.left": ("\u2190", CHAR, True),
    "Key.up": ("\u2191", CHAR, True),
    "Key.right": ("\u2192", CHAR, True),
    "Key.down": ("\u2193", CHAR, True),
}

# ── VK 码 → (显示名, 类型, is_fun) ──
VK_MAP = {
    # 主键盘
    0x08: ("\u2190", CHAR, True),   # Backspace → ←
    0x09: ("Tab", CHAR, True),
    0x0D: ("\u21b5", CHAR, True),   # Enter → ↵

    0x10: ("Shift", MOD, True),
    0x11: ("Ctrl", MOD, True),
    0x12: ("Alt", MOD, True),
    0x13: ("Pause", CHAR, True),
    0x14: ("Caps", CHAR, True),

    0x1B: ("Esc", CHAR, True),

    0x20: ("Space", CHAR, False),
    0x21: ("PgUp", CHAR, True),
    0x22: ("PgDn", CHAR, True),
    0x23: ("End", CHAR, True),
    0x24: ("Home", CHAR, True),
    0x25: ("\u2190", CHAR, True),
    0x26: ("\u2191", CHAR, True),
    0x27: ("\u2192", CHAR, True),
    0x28: ("\u2193", CHAR, True),
    0x2C: ("PrtSc", CHAR, True),
    0x2D: ("Ins", CHAR, True),
    0x2E: ("Del", CHAR, True),

    # 主键盘数字 0-9
    0x30: ("0", CHAR, False), 0x31: ("1", CHAR, False),
    0x32: ("2", CHAR, False), 0x33: ("3", CHAR, False),
    0x34: ("4", CHAR, False), 0x35: ("5", CHAR, False),
    0x36: ("6", CHAR, False), 0x37: ("7", CHAR, False),
    0x38: ("8", CHAR, False), 0x39: ("9", CHAR, False),

    # 字母 A-Z
    **{i: (chr(i), CHAR, False) for i in range(0x41, 0x5B)},

    # 符号键
    0xBA: (";", CHAR, False), 0xBB: ("=", CHAR, False),
    0xBC: (",", CHAR, False), 0xBD: ("-", CHAR, False),
    0xBE: (".", CHAR, False), 0xBF: ("/", CHAR, False),
    0xC0: ("`", CHAR, False),
    0xDB: ("[", CHAR, False), 0xDC: ("\\", CHAR, False),
    0xDD: ("]", CHAR, False), 0xDE: ("'", CHAR, False),

    # 功能键 F1-F12
    **{i: (f"F{i - 0x6F}", CHAR, True) for i in range(0x70, 0x7C)},

    # 小键盘
    0x60: ("0", CHAR, False), 0x61: ("1", CHAR, False),
    0x62: ("2", CHAR, False), 0x63: ("3", CHAR, False),
    0x64: ("4", CHAR, False), 0x65: ("5", CHAR, False),
    0x66: ("6", CHAR, False), 0x67: ("7", CHAR, False),
    0x68: ("8", CHAR, False), 0x69: ("9", CHAR, False),
    0x6A: ("*", CHAR, False), 0x6B: ("+", CHAR, False),
    0x6D: ("-", CHAR, False), 0x6E: (".", CHAR, False),
    0x6F: ("/", CHAR, False),

    # NpEnter (VK=0x0D, 与主键盘 Enter 共用)
    0x6C: ("NpEnter", CHAR, True),

    # Num/Scrlk
    0x90: ("Num", CHAR, True),
    0x91: ("Scrlk", CHAR, True),

    # Win/Menu
    0x5B: ("Win", MOD, True), 0x5C: ("Win", MOD, True),
    0x5D: ("Menu", MOD, True),
}


def lookup(key_str: str) -> tuple | None:
    """返回 (显示名, 类型, is_fun) 或 None"""
    if key_str in KEY_NAMES:
        return KEY_NAMES[key_str]

    if key_str.startswith("'") and key_str.endswith("'"):
        raw = key_str[1:-1]
        if raw == " ":
            return ("\u2423", CHAR, False)
        if raw.startswith("\\x") and len(raw) == 4:
            try:
                code = int(raw[2:], 16)
                if code < 32:
                    return (chr(ord('A') + code - 1), CHAR, False)
            except ValueError:
                pass
            return (raw, CHAR, False)
        if len(raw) == 1 and ord(raw) < 32:
            return (chr(ord('A') + ord(raw) - 1), CHAR, False)
        if len(raw) == 1:
            return (raw, CHAR, False)
        return (raw, CHAR, False)

    if (key_str.startswith("KeyCode(") and key_str.endswith(")")) or \
       (key_str.startswith("<") and key_str.endswith(">")):
        try:
            vk_str = key_str[8:-1] if key_str.startswith("KeyCode(") else key_str[1:-1]
            vk = int(vk_str)
            return VK_MAP.get(vk)
        except ValueError:
            pass

    return None


def display_info(display_name: str) -> tuple | None:
    """根据显示名查 (类型, is_fun)"""
    for _key, (_display, _type, _fun) in KEY_NAMES.items():
        if _display == display_name:
            return (_type, _fun)
    for _vk, (_display, _type, _fun) in VK_MAP.items():
        if _display == display_name:
            return (_type, _fun)
    return None
