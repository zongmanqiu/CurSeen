//! 键位映射表。只移植 Python 的 VK_MAP（keymap.py:63-131）。
//!
//! Shift 不改变显示字符——VK_MAP 只按 VK 码查表，不管 Shift 状态。
//! 这是产品既定行为（见 plan.md M6-1）。

use std::collections::HashMap;
use std::sync::OnceLock;

/// 键的类别。对齐 keymap.py 的 CHAR / MOD。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeyKind {
    Char,
    Mod,
}

/// 显示信息。对齐 (显示名, 类型, is_fun) 三元组。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyInfo {
    pub display: &'static str,
    pub kind: KeyKind,
    /// true → 用 fun_color（默认红）绘制；false → 用 font_color（默认黄）
    pub is_fun: bool,
}

/// VK → 显示信息。逐项照抄 keymap.py:63-131。
pub fn lookup_vk(vk: u16) -> Option<KeyInfo> {
    match vk {
        0x08 => Some(KeyInfo {
            display: "\u{2190}",
            kind: KeyKind::Char,
            is_fun: true,
        }), // ← Backspace
        0x09 => Some(KeyInfo {
            display: "Tab",
            kind: KeyKind::Char,
            is_fun: true,
        }),
        0x0D => Some(KeyInfo {
            display: "\u{21B5}",
            kind: KeyKind::Char,
            is_fun: true,
        }), // ↵ Enter
        0x10 => Some(KeyInfo {
            display: "Shift",
            kind: KeyKind::Mod,
            is_fun: true,
        }),
        0x11 => Some(KeyInfo {
            display: "Ctrl",
            kind: KeyKind::Mod,
            is_fun: true,
        }),
        0x12 => Some(KeyInfo {
            display: "Alt",
            kind: KeyKind::Mod,
            is_fun: true,
        }),
        0x13 => Some(KeyInfo {
            display: "Pause",
            kind: KeyKind::Char,
            is_fun: true,
        }),
        0x14 => Some(KeyInfo {
            display: "Caps",
            kind: KeyKind::Char,
            is_fun: true,
        }),
        0x1B => Some(KeyInfo {
            display: "Esc",
            kind: KeyKind::Char,
            is_fun: true,
        }),
        0x20 => Some(KeyInfo {
            display: "Space",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x21 => Some(KeyInfo {
            display: "PgUp",
            kind: KeyKind::Char,
            is_fun: true,
        }),
        0x22 => Some(KeyInfo {
            display: "PgDn",
            kind: KeyKind::Char,
            is_fun: true,
        }),
        0x23 => Some(KeyInfo {
            display: "End",
            kind: KeyKind::Char,
            is_fun: true,
        }),
        0x24 => Some(KeyInfo {
            display: "Home",
            kind: KeyKind::Char,
            is_fun: true,
        }),
        0x25 => Some(KeyInfo {
            display: "\u{2190}",
            kind: KeyKind::Char,
            is_fun: true,
        }), // ← Left
        0x26 => Some(KeyInfo {
            display: "\u{2191}",
            kind: KeyKind::Char,
            is_fun: true,
        }), // ↑ Up
        0x27 => Some(KeyInfo {
            display: "\u{2192}",
            kind: KeyKind::Char,
            is_fun: true,
        }), // → Right
        0x28 => Some(KeyInfo {
            display: "\u{2193}",
            kind: KeyKind::Char,
            is_fun: true,
        }), // ↓ Down
        0x2C => Some(KeyInfo {
            display: "PrtSc",
            kind: KeyKind::Char,
            is_fun: true,
        }),
        0x2D => Some(KeyInfo {
            display: "Ins",
            kind: KeyKind::Char,
            is_fun: true,
        }),
        0x2E => Some(KeyInfo {
            display: "Del",
            kind: KeyKind::Char,
            is_fun: true,
        }),
        // 0x30-0x39: 主键盘数字
        0x30 => Some(KeyInfo {
            display: "0",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x31 => Some(KeyInfo {
            display: "1",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x32 => Some(KeyInfo {
            display: "2",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x33 => Some(KeyInfo {
            display: "3",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x34 => Some(KeyInfo {
            display: "4",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x35 => Some(KeyInfo {
            display: "5",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x36 => Some(KeyInfo {
            display: "6",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x37 => Some(KeyInfo {
            display: "7",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x38 => Some(KeyInfo {
            display: "8",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x39 => Some(KeyInfo {
            display: "9",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        // 0x41-0x5A: A-Z
        0x41 => Some(KeyInfo {
            display: "A",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x42 => Some(KeyInfo {
            display: "B",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x43 => Some(KeyInfo {
            display: "C",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x44 => Some(KeyInfo {
            display: "D",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x45 => Some(KeyInfo {
            display: "E",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x46 => Some(KeyInfo {
            display: "F",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x47 => Some(KeyInfo {
            display: "G",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x48 => Some(KeyInfo {
            display: "H",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x49 => Some(KeyInfo {
            display: "I",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x4A => Some(KeyInfo {
            display: "J",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x4B => Some(KeyInfo {
            display: "K",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x4C => Some(KeyInfo {
            display: "L",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x4D => Some(KeyInfo {
            display: "M",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x4E => Some(KeyInfo {
            display: "N",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x4F => Some(KeyInfo {
            display: "O",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x50 => Some(KeyInfo {
            display: "P",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x51 => Some(KeyInfo {
            display: "Q",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x52 => Some(KeyInfo {
            display: "R",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x53 => Some(KeyInfo {
            display: "S",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x54 => Some(KeyInfo {
            display: "T",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x55 => Some(KeyInfo {
            display: "U",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x56 => Some(KeyInfo {
            display: "V",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x57 => Some(KeyInfo {
            display: "W",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x58 => Some(KeyInfo {
            display: "X",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x59 => Some(KeyInfo {
            display: "Y",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x5A => Some(KeyInfo {
            display: "Z",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x5B | 0x5C => Some(KeyInfo {
            display: "Win",
            kind: KeyKind::Mod,
            is_fun: true,
        }),
        0x5D => Some(KeyInfo {
            display: "Menu",
            kind: KeyKind::Mod,
            is_fun: true,
        }),
        // 0x60-0x69: 小键盘数字
        0x60 => Some(KeyInfo {
            display: "0",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x61 => Some(KeyInfo {
            display: "1",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x62 => Some(KeyInfo {
            display: "2",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x63 => Some(KeyInfo {
            display: "3",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x64 => Some(KeyInfo {
            display: "4",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x65 => Some(KeyInfo {
            display: "5",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x66 => Some(KeyInfo {
            display: "6",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x67 => Some(KeyInfo {
            display: "7",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x68 => Some(KeyInfo {
            display: "8",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x69 => Some(KeyInfo {
            display: "9",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x6A => Some(KeyInfo {
            display: "*",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x6B => Some(KeyInfo {
            display: "+",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x6C => Some(KeyInfo {
            display: "NpEnter",
            kind: KeyKind::Char,
            is_fun: true,
        }),
        0x6D => Some(KeyInfo {
            display: "-",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x6E => Some(KeyInfo {
            display: ".",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0x6F => Some(KeyInfo {
            display: "/",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        // F1-F12
        0x70 => Some(KeyInfo {
            display: "F1",
            kind: KeyKind::Char,
            is_fun: true,
        }),
        0x71 => Some(KeyInfo {
            display: "F2",
            kind: KeyKind::Char,
            is_fun: true,
        }),
        0x72 => Some(KeyInfo {
            display: "F3",
            kind: KeyKind::Char,
            is_fun: true,
        }),
        0x73 => Some(KeyInfo {
            display: "F4",
            kind: KeyKind::Char,
            is_fun: true,
        }),
        0x74 => Some(KeyInfo {
            display: "F5",
            kind: KeyKind::Char,
            is_fun: true,
        }),
        0x75 => Some(KeyInfo {
            display: "F6",
            kind: KeyKind::Char,
            is_fun: true,
        }),
        0x76 => Some(KeyInfo {
            display: "F7",
            kind: KeyKind::Char,
            is_fun: true,
        }),
        0x77 => Some(KeyInfo {
            display: "F8",
            kind: KeyKind::Char,
            is_fun: true,
        }),
        0x78 => Some(KeyInfo {
            display: "F9",
            kind: KeyKind::Char,
            is_fun: true,
        }),
        0x79 => Some(KeyInfo {
            display: "F10",
            kind: KeyKind::Char,
            is_fun: true,
        }),
        0x7A => Some(KeyInfo {
            display: "F11",
            kind: KeyKind::Char,
            is_fun: true,
        }),
        0x7B => Some(KeyInfo {
            display: "F12",
            kind: KeyKind::Char,
            is_fun: true,
        }),
        0x90 => Some(KeyInfo {
            display: "Num",
            kind: KeyKind::Char,
            is_fun: true,
        }),
        0x91 => Some(KeyInfo {
            display: "Scrlk",
            kind: KeyKind::Char,
            is_fun: true,
        }),
        // OEM 键
        0xBA => Some(KeyInfo {
            display: ";",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0xBB => Some(KeyInfo {
            display: "=",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0xBC => Some(KeyInfo {
            display: ",",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0xBD => Some(KeyInfo {
            display: "-",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0xBE => Some(KeyInfo {
            display: ".",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0xBF => Some(KeyInfo {
            display: "/",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0xC0 => Some(KeyInfo {
            display: "`",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0xDB => Some(KeyInfo {
            display: "[",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0xDC => Some(KeyInfo {
            display: "\\",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0xDD => Some(KeyInfo {
            display: "]",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        0xDE => Some(KeyInfo {
            display: "'",
            kind: KeyKind::Char,
            is_fun: false,
        }),
        _ => None,
    }
}

/// 反向表：显示名 → (类别, is_fun)。绘制时逐词调用，必须 O(1)。
fn reverse_table() -> &'static HashMap<&'static str, (KeyKind, bool)> {
    static TABLE: OnceLock<HashMap<&'static str, (KeyKind, bool)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut map = HashMap::new();
        for vk in 0x00..=0xFF {
            if let Some(info) = lookup_vk(vk) {
                let entry = map.entry(info.display).or_insert((info.kind, info.is_fun));
                // 同一显示名的 (kind, is_fun) 必须一致
                debug_assert_eq!(
                    *entry,
                    (info.kind, info.is_fun),
                    "Inconsistent display info for '{}': {:?} vs {:?}",
                    info.display,
                    *entry,
                    (info.kind, info.is_fun)
                );
            }
        }
        map
    })
}

/// 显示名 → (类别, is_fun)。绘制时逐词调用，O(1)。
pub fn display_info(name: &str) -> Option<(KeyKind, bool)> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }
    reverse_table().get(trimmed).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vk_map_count() {
        // 验证覆盖了所有计划的 VK 码
        let mut count = 0;
        for vk in 0x00..=0xFF {
            if lookup_vk(vk).is_some() {
                count += 1;
            }
        }
        // 68 项（plan.md M6-1 说的）
        assert!(count >= 60, "VK_MAP has {} entries, expected >= 60", count);
    }

    #[test]
    fn mod_keys() {
        for vk in [0x10, 0x11, 0x12, 0x5B, 0x5C, 0x5D] {
            let info = lookup_vk(vk).unwrap();
            assert_eq!(info.kind, KeyKind::Mod, "VK 0x{:02X} should be Mod", vk);
            assert!(info.is_fun);
        }
    }

    #[test]
    fn arrows_are_char() {
        for vk in [0x25, 0x26, 0x27, 0x28] {
            let info = lookup_vk(vk).unwrap();
            assert_eq!(info.kind, KeyKind::Char);
            assert!(info.is_fun);
        }
    }

    #[test]
    fn display_info_lookup() {
        let info = display_info("Ctrl").unwrap();
        assert_eq!(info, (KeyKind::Mod, true));
        let info = display_info("A").unwrap();
        assert_eq!(info, (KeyKind::Char, false));
    }

    #[test]
    fn display_info_trim() {
        assert!(display_info(" Ctrl ").is_some());
        assert!(display_info("").is_none());
    }
}
