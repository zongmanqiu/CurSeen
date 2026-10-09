//! 应用级快捷键接线。
//!
//! 匹配逻辑（归一化、按键 → token、书写快捷键表）在 [`crate::core::hotkeys`]——那是纯数据
//! 转换，`modules` 也要用，而 `modules` 不能依赖 `app`。本模块只负责"从配置里读出生效的
//! 面板快捷键"这一件应用层的事，并把 core 的匹配函数转出来给 `ui` 用。

pub use crate::core::hotkeys::{
    is_modifier_key, match_drawing, matches, normalize, pressed, DrawingAction,
};

use crate::core::config::Config;

/// 取生效的面板快捷键。空串表示**未启用**（对齐 Python：`panel_hotkey` 为空即无效）。
///
/// 出厂默认见 `GlobalSettings::default()`（`Ctrl+Shift+D`）。Python 的出厂默认是空串，
/// 于是它的"面板快捷键"功能实际上从不生效；plan3 §1.3 决定把这个死字段落成真功能。
pub fn panel_hotkey(config: &Config) -> &str {
    config.global.panel_hotkey.trim()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::{KeyCode, Modifiers};

    #[test]
    fn panel_hotkey_reads_config() {
        let mut cfg = Config::default();
        assert_eq!(panel_hotkey(&cfg), "Ctrl+Shift+D", "出厂默认应启用");
        cfg.global.panel_hotkey = "  Alt+P  ".into();
        assert_eq!(panel_hotkey(&cfg), "Alt+P");
        cfg.global.panel_hotkey = String::new();
        assert_eq!(panel_hotkey(&cfg), "", "清空即停用");
    }

    #[test]
    fn default_panel_hotkey_is_reachable() {
        let cfg = Config::default();
        let none = Modifiers {
            ctrl: false,
            alt: false,
            shift: false,
            win: false,
        };
        assert!(matches(
            panel_hotkey(&cfg),
            KeyCode::Char('d'),
            Modifiers {
                ctrl: true,
                shift: true,
                ..none
            }
        ));
    }
}
