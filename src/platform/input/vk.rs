//! 虚拟键码到 KeyCode 的映射。
//!
//! 三条规则：
//!   ① 修饰键按 scanCode + LLKHF_EXTENDED 区分左右
//!   ② 小键盘与编辑键区靠 LLKHF_EXTENDED 区分
//!   ③ 字符键的字符值考虑当前 CapsLock 与 Shift 状态

use crate::core::events::KeyCode;

/// VK 码常量（与 windows crate 的 VIRTUAL_KEY 对齐，但用 u16 避免引入平台依赖）
#[allow(dead_code)]
pub mod vk_const {
    pub const LBUTTON: u16 = 0x01;
    pub const RBUTTON: u16 = 0x02;
    pub const CANCEL: u16 = 0x03;
    pub const MBUTTON: u16 = 0x04;
    pub const XBUTTON1: u16 = 0x05;
    pub const XBUTTON2: u16 = 0x06;
    pub const BACK: u16 = 0x08;
    pub const TAB: u16 = 0x09;
    pub const CLEAR: u16 = 0x0C;
    pub const RETURN: u16 = 0x0D;
    pub const SHIFT: u16 = 0x10;
    pub const CONTROL: u16 = 0x11;
    pub const MENU: u16 = 0x12;
    pub const PAUSE: u16 = 0x13;
    pub const CAPITAL: u16 = 0x14;
    pub const KANA: u16 = 0x15;
    pub const IME_ON: u16 = 0x16;
    pub const JUNJA: u16 = 0x17;
    pub const FINAL: u16 = 0x18;
    pub const IME_OFF: u16 = 0x1A;
    pub const ESCAPE: u16 = 0x1B;
    pub const CONVERT: u16 = 0x1C;
    pub const NONCONVERT: u16 = 0x1D;
    pub const ACCEPT: u16 = 0x1E;
    pub const MODECHANGE: u16 = 0x1F;
    pub const SPACE: u16 = 0x20;
    pub const PRIOR: u16 = 0x21;
    pub const NEXT: u16 = 0x22;
    pub const END: u16 = 0x23;
    pub const HOME: u16 = 0x24;
    pub const LEFT: u16 = 0x25;
    pub const UP: u16 = 0x26;
    pub const RIGHT: u16 = 0x27;
    pub const DOWN: u16 = 0x28;
    pub const SELECT: u16 = 0x29;
    pub const PRINT: u16 = 0x2A;
    pub const EXECUTE: u16 = 0x2B;
    pub const SNAPSHOT: u16 = 0x2C;
    pub const INSERT: u16 = 0x2D;
    pub const DELETE: u16 = 0x2E;
    pub const HELP: u16 = 0x2F;
    pub const KEY_0: u16 = 0x30;
    pub const KEY_9: u16 = 0x39;
    pub const KEY_A: u16 = 0x41;
    pub const KEY_Z: u16 = 0x5A;
    pub const LWIN: u16 = 0x5B;
    pub const RWIN: u16 = 0x5C;
    pub const APPS: u16 = 0x5D;
    pub const SLEEP: u16 = 0x5F;
    pub const NUMPAD0: u16 = 0x60;
    pub const NUMPAD9: u16 = 0x69;
    pub const MULTIPLY: u16 = 0x6A;
    pub const ADD: u16 = 0x6B;
    pub const SEPARATOR: u16 = 0x6C;
    pub const SUBTRACT: u16 = 0x6D;
    pub const DECIMAL: u16 = 0x6E;
    pub const DIVIDE: u16 = 0x6F;
    pub const F1: u16 = 0x70;
    pub const F24: u16 = 0x87;
    pub const NUMLOCK: u16 = 0x90;
    pub const SCROLL: u16 = 0x91;
    pub const LSHIFT: u16 = 0xA0;
    pub const RSHIFT: u16 = 0xA1;
    pub const LCONTROL: u16 = 0xA2;
    pub const RCONTROL: u16 = 0xA3;
    pub const LMENU: u16 = 0xA4;
    pub const RMENU: u16 = 0xA5;
    pub const BROWSER_BACK: u16 = 0xA6;
    pub const BROWSER_FORWARD: u16 = 0xA7;
    pub const BROWSER_REFRESH: u16 = 0xA8;
    pub const BROWSER_STOP: u16 = 0xA9;
    pub const BROWSER_SEARCH: u16 = 0xAA;
    pub const BROWSER_FAVORITES: u16 = 0xAB;
    pub const BROWSER_HOME: u16 = 0xAC;
    pub const VOLUME_MUTE: u16 = 0xAD;
    pub const VOLUME_DOWN: u16 = 0xAE;
    pub const VOLUME_UP: u16 = 0xAF;
    pub const MEDIA_NEXT_TRACK: u16 = 0xB0;
    pub const MEDIA_PREV_TRACK: u16 = 0xB1;
    pub const MEDIA_STOP: u16 = 0xB2;
    pub const MEDIA_PLAY_PAUSE: u16 = 0xB3;
    pub const OEM_1: u16 = 0xBA;
    pub const OEM_PLUS: u16 = 0xBB;
    pub const OEM_COMMA: u16 = 0xBC;
    pub const OEM_MINUS: u16 = 0xBD;
    pub const OEM_PERIOD: u16 = 0xBE;
    pub const OEM_2: u16 = 0xBF;
    pub const OEM_3: u16 = 0xC0;
    pub const OEM_4: u16 = 0xDB;
    pub const OEM_5: u16 = 0xDC;
    pub const OEM_6: u16 = 0xDD;
    pub const OEM_7: u16 = 0xDE;
    pub const OEM_102: u16 = 0xE2;
}

use vk_const as v;

/// 从 VK 码 + 扫描码 + 扩展标志 + Shift/CapsLock 状态映射到 KeyCode。
pub fn from_vk(vk: u16, scan: u16, extended: bool, shift: bool, caps: bool) -> KeyCode {
    match vk {
        v::LSHIFT => KeyCode::LShift,
        v::RSHIFT => KeyCode::RShift,
        v::LCONTROL => KeyCode::LCtrl,
        v::RCONTROL => KeyCode::RCtrl,
        v::LMENU => KeyCode::LAlt,
        v::RMENU => KeyCode::RAlt,
        v::SHIFT => {
            if scan == 0x36 {
                KeyCode::RShift
            } else {
                KeyCode::LShift
            }
        }
        v::CONTROL => {
            if extended {
                KeyCode::RCtrl
            } else {
                KeyCode::LCtrl
            }
        }
        v::MENU => {
            if extended {
                KeyCode::RAlt
            } else {
                KeyCode::LAlt
            }
        }
        v::LWIN => KeyCode::LWin,
        v::RWIN => KeyCode::RWin,
        v::APPS => KeyCode::Menu,
        v::ESCAPE => KeyCode::Esc,
        v::TAB => KeyCode::Tab,
        v::CAPITAL => KeyCode::CapsLock,
        v::BACK => KeyCode::Backspace,
        // ★ 小键盘 Enter 与主键盘 Enter 在 Windows 上是**同一个 VK（0x0D）**，区别只有
        //   扩展标志：主区 Enter 不带、小键盘 Enter 带（`LLKHF_EXTENDED`）。
        //   不看这个位，按小键盘 Enter 就会点亮主键盘那颗（用户实测："小键盘的 enter 键，
        //   点击以后居然显示的是右 shift 键上面的 enter"）。
        //
        //   与 [`to_vk_ext`] 互为逆：`to_vk_ext(NumEnter) == (RETURN, true)` 早就这么写了，
        //   只是正向没接上。
        //
        //   ★ 副作用（有意为之）：快捷键串里 `ENTER` 与 `NUMENTER` 从此是两个键。此前按小键盘
        //     Enter 会误触发注册为 `Enter` 的快捷键 —— 那不是"方便"，是"按了不该触发的键"。
        v::RETURN => {
            if extended {
                KeyCode::NumEnter
            } else {
                KeyCode::Enter
            }
        }
        v::SPACE => KeyCode::Space,
        v::SNAPSHOT => KeyCode::PrtSc,
        v::SCROLL => KeyCode::ScrollLock,
        v::PAUSE => KeyCode::Pause,
        v::INSERT => KeyCode::Insert,
        v::DELETE => KeyCode::Delete,
        v::HOME => KeyCode::Home,
        v::END => KeyCode::End,
        v::PRIOR => KeyCode::PageUp,
        v::NEXT => KeyCode::PageDown,
        v::UP => KeyCode::Up,
        v::DOWN => KeyCode::Down,
        v::LEFT => KeyCode::Left,
        v::RIGHT => KeyCode::Right,
        v::NUMLOCK => KeyCode::NumLock,
        v::F1..=v::F24 => KeyCode::F((vk - v::F1 + 1) as u8),
        v::NUMPAD0..=v::NUMPAD9 => {
            if extended {
                match vk {
                    0x60 => KeyCode::Insert,
                    0x61 => KeyCode::End,
                    0x62 => KeyCode::Down,
                    0x63 => KeyCode::PageDown,
                    0x64 => KeyCode::Left,
                    0x65 => KeyCode::Num(5),
                    0x66 => KeyCode::Right,
                    0x67 => KeyCode::Home,
                    0x68 => KeyCode::Up,
                    0x69 => KeyCode::PageUp,
                    _ => KeyCode::Num((vk - v::NUMPAD0) as u8),
                }
            } else {
                KeyCode::Num((vk - v::NUMPAD0) as u8)
            }
        }
        v::MULTIPLY => KeyCode::NumMul,
        v::ADD => KeyCode::NumAdd,
        v::SEPARATOR => KeyCode::NumEnter,
        v::SUBTRACT => KeyCode::NumSub,
        v::DECIMAL => KeyCode::NumDot,
        v::DIVIDE => KeyCode::NumDiv,
        v::KEY_0..=v::KEY_9 => {
            let digit = (vk - v::KEY_0) as u8;
            if shift {
                KeyCode::Char(match digit {
                    0 => ')',
                    1 => '!',
                    2 => '@',
                    3 => '#',
                    4 => '$',
                    5 => '%',
                    6 => '^',
                    7 => '&',
                    8 => '*',
                    _ => '(',
                })
            } else {
                KeyCode::Char(char::from(b'0' + digit))
            }
        }
        v::KEY_A..=v::KEY_Z => {
            let base = (vk - v::KEY_A) as u8;
            let upper = (caps && !shift) || (!caps && shift);
            KeyCode::Char(if upper {
                (b'A' + base) as char
            } else {
                (b'a' + base) as char
            })
        }
        v::OEM_1 => KeyCode::Char(if shift { ':' } else { ';' }),
        v::OEM_PLUS => KeyCode::Char(if shift { '+' } else { '=' }),
        v::OEM_COMMA => KeyCode::Char(if shift { '<' } else { ',' }),
        v::OEM_MINUS => KeyCode::Char(if shift { '_' } else { '-' }),
        v::OEM_PERIOD => KeyCode::Char(if shift { '>' } else { '.' }),
        v::OEM_2 => KeyCode::Char(if shift { '?' } else { '/' }),
        v::OEM_3 => KeyCode::Char(if shift { '~' } else { '`' }),
        v::OEM_4 => KeyCode::Char(if shift { '{' } else { '[' }),
        v::OEM_5 => KeyCode::Char(if shift { '|' } else { '\\' }),
        v::OEM_6 => KeyCode::Char(if shift { '}' } else { ']' }),
        v::OEM_7 => KeyCode::Char(if shift { '"' } else { '\'' }),
        v::OEM_102 => KeyCode::Char(if shift { '>' } else { '<' }),
        v::VOLUME_MUTE => KeyCode::VolumeMute,
        v::VOLUME_DOWN => KeyCode::VolumeDown,
        v::VOLUME_UP => KeyCode::VolumeUp,
        v::MEDIA_NEXT_TRACK => KeyCode::MediaNext,
        v::MEDIA_PREV_TRACK => KeyCode::MediaPrev,
        v::MEDIA_STOP => KeyCode::MediaStop,
        v::MEDIA_PLAY_PAUSE => KeyCode::MediaPlay,
        v::BROWSER_BACK => KeyCode::BrowserBack,
        v::BROWSER_FORWARD => KeyCode::BrowserForward,
        v::BROWSER_REFRESH => KeyCode::BrowserRefresh,
        v::BROWSER_HOME => KeyCode::BrowserHome,
        _ => KeyCode::Unknown(vk),
    }
}

/// 是否为修饰键。
pub fn is_modifier(k: KeyCode) -> bool {
    matches!(
        k,
        KeyCode::LShift
            | KeyCode::RShift
            | KeyCode::LCtrl
            | KeyCode::RCtrl
            | KeyCode::LAlt
            | KeyCode::RAlt
            | KeyCode::LWin
            | KeyCode::RWin
    )
}

/// KeyCode → 原始 VK 码（快捷键注册需要）。
pub fn to_vk(k: KeyCode) -> Option<u16> {
    match k {
        KeyCode::Char(c) => {
            if c.is_ascii_lowercase() {
                Some(v::KEY_A + (c as u16 - 'a' as u16))
            } else if c.is_ascii_uppercase() {
                Some(v::KEY_A + (c as u16 - 'A' as u16))
            } else if c.is_ascii_digit() {
                Some(v::KEY_0 + (c as u16 - '0' as u16))
            } else {
                match c {
                    ';' => Some(v::OEM_1),
                    '=' => Some(v::OEM_PLUS),
                    ',' => Some(v::OEM_COMMA),
                    '-' => Some(v::OEM_MINUS),
                    '.' => Some(v::OEM_PERIOD),
                    '/' => Some(v::OEM_2),
                    '`' => Some(v::OEM_3),
                    '[' => Some(v::OEM_4),
                    '\\' => Some(v::OEM_5),
                    ']' => Some(v::OEM_6),
                    '\'' => Some(v::OEM_7),
                    ' ' => Some(v::SPACE),
                    _ => None,
                }
            }
        }
        KeyCode::F(n) => Some(v::F1 + (n as u16 - 1)),
        KeyCode::Esc => Some(v::ESCAPE),
        KeyCode::Tab => Some(v::TAB),
        KeyCode::CapsLock => Some(v::CAPITAL),
        KeyCode::Backspace => Some(v::BACK),
        KeyCode::Enter => Some(v::RETURN),
        KeyCode::Space => Some(v::SPACE),
        KeyCode::LShift => Some(v::LSHIFT),
        KeyCode::RShift => Some(v::RSHIFT),
        KeyCode::LCtrl => Some(v::LCONTROL),
        KeyCode::RCtrl => Some(v::RCONTROL),
        KeyCode::LAlt => Some(v::LMENU),
        KeyCode::RAlt => Some(v::RMENU),
        KeyCode::LWin => Some(v::LWIN),
        KeyCode::RWin => Some(v::RWIN),
        KeyCode::Menu => Some(v::APPS),
        KeyCode::PrtSc => Some(v::SNAPSHOT),
        KeyCode::ScrollLock => Some(v::SCROLL),
        KeyCode::Pause => Some(v::PAUSE),
        KeyCode::Insert => Some(v::INSERT),
        KeyCode::Delete => Some(v::DELETE),
        KeyCode::Home => Some(v::HOME),
        KeyCode::End => Some(v::END),
        KeyCode::PageUp => Some(v::PRIOR),
        KeyCode::PageDown => Some(v::NEXT),
        KeyCode::Up => Some(v::UP),
        KeyCode::Down => Some(v::DOWN),
        KeyCode::Left => Some(v::LEFT),
        KeyCode::Right => Some(v::RIGHT),
        KeyCode::NumLock => Some(v::NUMLOCK),
        KeyCode::NumDiv => Some(v::DIVIDE),
        KeyCode::NumMul => Some(v::MULTIPLY),
        KeyCode::NumSub => Some(v::SUBTRACT),
        KeyCode::NumAdd => Some(v::ADD),
        KeyCode::NumDot => Some(v::DECIMAL),
        KeyCode::NumEnter => Some(v::RETURN),
        KeyCode::Num(n) => Some(v::NUMPAD0 + n as u16),
        _ => None,
    }
}

/// 注入用的 VK：`(vk 码, 是否需要 KEYEVENTF_EXTENDEDKEY)`。
///
/// 与 [`to_vk`]（快捷键注册用）的差别有两个：
///
/// 1. **上档字符先归一到同一物理键**（`'!'` → `'1'`）。方案的 `code` 本该写基字符，
///    但用户写了 `"code":"!"` 也得能按 —— 键帽印的是 `1/!`，点它就是按那颗物理键；
/// 2. **额外报出"扩展键"**：小键盘 Enter、右 Ctrl/Alt、方向键与导航键这些必须带
///    `KEYEVENTF_EXTENDEDKEY`，否则部分应用认不出来（典型症状：小键盘 Enter 被当成
///    主键盘 Enter、右 Ctrl 与左 Ctrl 混为一谈）。
pub fn to_vk_ext(k: KeyCode) -> Option<(u16, bool)> {
    let base = match k {
        KeyCode::Char(c) if !c.is_ascii_alphanumeric() => KeyCode::Char(base_of(c)),
        other => other,
    };
    let extended = matches!(
        base,
        KeyCode::RCtrl
            | KeyCode::RAlt
            | KeyCode::Insert
            | KeyCode::Delete
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::PageUp
            | KeyCode::PageDown
            | KeyCode::Up
            | KeyCode::Down
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::NumLock
            | KeyCode::NumDiv
            | KeyCode::PrtSc
            | KeyCode::Menu
            | KeyCode::LWin
            | KeyCode::RWin
            | KeyCode::NumEnter
    );
    to_vk(base).map(|vk| (vk, extended))
}

/// 上档字符 → 同一物理键上的下档字符。
fn base_of(c: char) -> char {
    match c {
        '~' => '`',
        '!' => '1',
        '@' => '2',
        '#' => '3',
        '$' => '4',
        '%' => '5',
        '^' => '6',
        '&' => '7',
        '*' => '8',
        '(' => '9',
        ')' => '0',
        '_' => '-',
        '+' => '=',
        '{' => '[',
        '}' => ']',
        '|' => '\\',
        ':' => ';',
        '"' => '\'',
        '<' => ',',
        '>' => '.',
        '?' => '/',
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ★ 小键盘回归锁（2026-09-22）：NumLock 进 KeyCode::NumLock；小键盘 Enter
    /// 是 NumEnter 而不是 Num(0)；Num(5) 注入回 VK 0x65。
    #[test]
    fn numpad_enter_and_numlock_roundtrip() {
        assert!(matches!(
            from_vk(v::NUMLOCK, 0, false, false, false),
            KeyCode::NumLock
        ));
        assert!(matches!(
            from_vk(v::SEPARATOR, 0, false, false, false),
            KeyCode::NumEnter
        ));
        assert!(matches!(
            from_vk(0x65, 0, false, false, false),
            KeyCode::Num(5)
        ));
        assert_eq!(to_vk(KeyCode::NumEnter), Some(v::RETURN)); // 注入语义：小键盘 Enter → 主键盘 Enter VK（to_vk_ext 再补 extended）
        assert_eq!(to_vk(KeyCode::Num(5)), Some(0x65));
    }

    /// ★ 小键盘 Enter 与主区 Enter 是**同一个 VK（0x0D）**，只能靠扩展标志区分。
    ///
    /// 正向漏了这个位 → 按小键盘 Enter 点亮的是主键盘那颗 Enter（用户实测报的 bug）；
    /// 而且**原版 ANSI 104 也一样**，因为根因在键码映射、不在布局。
    ///
    /// 对旧实现是真红：`from_vk(RETURN, extended=true)` 旧返回 `Enter`。
    #[test]
    fn numpad_enter_is_told_apart_by_the_extended_flag() {
        assert_eq!(from_vk(v::RETURN, 0, false, false, false), KeyCode::Enter);
        assert_eq!(
            from_vk(v::RETURN, 0, true, false, false),
            KeyCode::NumEnter,
            "带扩展标志的 VK_RETURN 必须是小键盘 Enter"
        );
        // 与注入方向互为逆（to_vk_ext 早就这么写了，只是正向没接上）。
        assert_eq!(to_vk_ext(KeyCode::Enter), Some((v::RETURN, false)));
        assert_eq!(to_vk_ext(KeyCode::NumEnter), Some((v::RETURN, true)));
    }

    #[test]
    fn shifted_chars_map_to_the_same_physical_key() {
        assert_eq!(to_vk_ext(KeyCode::Char('!')), to_vk_ext(KeyCode::Char('1')));
        assert_eq!(to_vk_ext(KeyCode::Char('~')), to_vk_ext(KeyCode::Char('`')));
        assert_eq!(to_vk_ext(KeyCode::Char('?')), to_vk_ext(KeyCode::Char('/')));
        assert_eq!(to_vk_ext(KeyCode::Char(':')), to_vk_ext(KeyCode::Char(';')));
    }

    #[test]
    fn extended_keys_are_flagged() {
        assert_eq!(to_vk_ext(KeyCode::NumEnter), Some((v::RETURN, true)));
        assert_eq!(to_vk_ext(KeyCode::RCtrl), Some((v::RCONTROL, true)));
        assert_eq!(to_vk_ext(KeyCode::LCtrl), Some((v::LCONTROL, false)));
        assert_eq!(to_vk_ext(KeyCode::Left), Some((v::LEFT, true)));
        assert_eq!(to_vk_ext(KeyCode::Char('a')), Some((v::KEY_A, false)));
        assert_eq!(to_vk_ext(KeyCode::Space), Some((v::SPACE, false)));
    }

    #[test]
    fn unknown_keys_have_no_mapping() {
        assert_eq!(to_vk_ext(KeyCode::Oem(0xFF)), None);
        assert_eq!(to_vk_ext(KeyCode::Unknown(0x00)), None);
    }
}
