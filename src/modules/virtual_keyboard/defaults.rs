//! KeyCode → 默认显示名映射。
//!
//! 用户在 `keyboard.json` 的 `keys` 块里可以覆盖任何一个键的显示文字。
//! 未覆盖的键走此表。字母键返回大写单字符，功能键/修饰键返回缩写。

use crate::core::events::KeyCode;

/// 按键的默认显示名（未被 JSON 覆盖时使用）。
///
/// 字母键：返回大写单字符（`"A"` .. `"Z"`）。
/// 数字键：返回 `"0"` .. `"9"`。
/// 功能键/修饰键/编辑键：返回人类可读缩写。
pub fn default_label(code: KeyCode) -> &'static str {
    match code {
        // ── 字母 ──
        KeyCode::Char(c) => {
            if c == ' ' {
                return "";
            }
            if c.is_ascii_lowercase() {
                // 'a'..='z' → "A".."Z"（编译期能确定长度，用静态表）
                return CHAR_LABELS[(c as u8 - b'a') as usize];
            }
            if c.is_ascii_digit() {
                return CHAR_LABELS[26 + (c as u8 - b'0') as usize];
            }
            // 符号键：直接用符号本身。
            //
            // ★ 这里原本是 `Box::leak` + thread_local 缓冲：每按到一个新的符号键就
            //   泄漏一段字符串（连原注释都承认了）。ANSI 布局里出现的符号一共就这么
            //   几个，穷举成编译期字面量即可 —— 零分配、零泄漏。
            match c {
                '`' => "`",
                '-' => "-",
                '=' => "=",
                '[' => "[",
                ']' => "]",
                '\\' => "\\",
                ';' => ";",
                '\'' => "'",
                ',' => ",",
                '.' => ".",
                '/' => "/",
                // 布局表之外的字符（OEM/IME 等）没有既定画法。
                _ => "?",
            }
        }
        // ── 功能键 ──
        KeyCode::F(n) => match n {
            1 => "F1",
            2 => "F2",
            3 => "F3",
            4 => "F4",
            5 => "F5",
            6 => "F6",
            7 => "F7",
            8 => "F8",
            9 => "F9",
            10 => "F10",
            11 => "F11",
            12 => "F12",
            _ => "F?",
        },
        // ── 修饰键 ──
        KeyCode::LShift | KeyCode::RShift => "Shift",
        KeyCode::LCtrl | KeyCode::RCtrl => "Ctrl",
        KeyCode::LAlt | KeyCode::RAlt => "Alt",
        KeyCode::LWin | KeyCode::RWin => "Win",
        KeyCode::Menu => "Menu",
        // ── 编辑/导航 ──
        KeyCode::Backspace => "Backspace",
        KeyCode::Tab => "Tab",
        KeyCode::CapsLock => "Caps",
        KeyCode::Enter => "Enter",
        KeyCode::Space => "",
        KeyCode::Esc => "Esc",
        KeyCode::Insert => "Ins",
        KeyCode::Delete => "Del",
        KeyCode::Home => "Home",
        KeyCode::End => "End",
        KeyCode::PageUp => "PgUp",
        KeyCode::PageDown => "PgDn",
        KeyCode::Up => "↑",
        KeyCode::Down => "↓",
        KeyCode::Left => "←",
        KeyCode::Right => "→",
        // ── 系统键 ──
        KeyCode::PrtSc => "PrtSc",
        KeyCode::ScrollLock => "ScrLk",
        KeyCode::Pause => "Pause",
        // ── 小键盘 ──
        KeyCode::NumLock => "NumLk",
        KeyCode::NumDiv => "/",
        KeyCode::NumMul => "*",
        KeyCode::NumSub => "-",
        KeyCode::NumAdd => "+",
        KeyCode::NumDot => ".",
        KeyCode::NumEnter => "Enter",
        // ── 鼠标（游戏 WASD 板；方案 JSON 里通常自带 text，这里只是兜底）──
        KeyCode::MouseLeft => "LMB",
        KeyCode::MouseRight => "RMB",
        // 与击键回显的 keynames 同款中文命名（显示一致性）。
        KeyCode::MouseMiddle => "中键",
        KeyCode::MouseX1 => "侧键1",
        KeyCode::MouseX2 => "侧键2",
        KeyCode::WheelUp => "滚轮上",
        KeyCode::WheelDown => "滚轮下",
        KeyCode::Num(n) => {
            // '0'..'9' 的 static 版本
            NUM_LABELS[n as usize]
        }
        // ── 媒体/浏览器 ──
        KeyCode::VolumeUp => "Vol+",
        KeyCode::VolumeDown => "Vol-",
        KeyCode::VolumeMute => "Mute",
        KeyCode::MediaNext => "Next",
        KeyCode::MediaPrev => "Prev",
        KeyCode::MediaStop => "Stop",
        KeyCode::MediaPlay => "Play",
        KeyCode::BrowserBack => "Back",
        KeyCode::BrowserForward => "Fwd",
        KeyCode::BrowserRefresh => "Refresh",
        KeyCode::BrowserHome => "Home",
        KeyCode::Oem(_) => "Oem",
        KeyCode::Unknown(_) => "?",
    }
}

/// 字母+数字的静态标签表：`'a'→"A"`, `'b'→"B"`, ..., `'0'→"0"`, ..., `'9'→"9"`。
const CHAR_LABELS: [&str; 36] = [
    "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S",
    "T", "U", "V", "W", "X", "Y", "Z", "0", "1", "2", "3", "4", "5", "6", "7", "8", "9",
];

/// 小键盘数字的静态标签表。
const NUM_LABELS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letter_labels_are_uppercase() {
        assert_eq!(default_label(KeyCode::Char('a')), "A");
        assert_eq!(default_label(KeyCode::Char('z')), "Z");
    }

    #[test]
    fn digit_labels() {
        assert_eq!(default_label(KeyCode::Char('0')), "0");
        assert_eq!(default_label(KeyCode::Char('9')), "9");
    }

    #[test]
    fn modifier_short_names() {
        assert_eq!(default_label(KeyCode::LShift), "Shift");
        assert_eq!(default_label(KeyCode::RCtrl), "Ctrl");
        assert_eq!(default_label(KeyCode::LAlt), "Alt");
    }

    #[test]
    fn special_keys() {
        assert_eq!(default_label(KeyCode::Backspace), "Backspace");
        assert_eq!(default_label(KeyCode::Enter), "Enter");
        assert_eq!(default_label(KeyCode::Space), "");
        assert_eq!(default_label(KeyCode::Delete), "Del");
        assert_eq!(default_label(KeyCode::Tab), "Tab");
    }

    #[test]
    fn numpad() {
        assert_eq!(default_label(KeyCode::Num(0)), "0");
        assert_eq!(default_label(KeyCode::Num(5)), "5");
        assert_eq!(default_label(KeyCode::NumAdd), "+");
    }

    #[test]
    fn fn_keys() {
        assert_eq!(default_label(KeyCode::F(1)), "F1");
        assert_eq!(default_label(KeyCode::F(12)), "F12");
    }

    /// 符号键直接显示自身（回归锚点：这里曾用 `Box::leak` 造 `&'static str`）。
    ///
    /// 这条测试同时锁住"零泄漏"：若有人改回 `Box::leak`，行为仍然对，
    /// 所以真正的防线是代码评审 —— 但至少行为差异会被这条测试钉住。
    #[test]
    fn symbol_labels_are_their_own_glyph() {
        for c in ['`', '-', '=', '[', ']', '\\', ';', '\'', ',', '.', '/'] {
            assert_eq!(
                default_label(KeyCode::Char(c)),
                c.to_string(),
                "符号键 {c:?} 应显示自身"
            );
        }
        // 布局表之外的字符没有既定画法。
        assert_eq!(default_label(KeyCode::Char('中')), "?");
    }
}
