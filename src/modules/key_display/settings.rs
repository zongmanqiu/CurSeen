//! 击键回显模块设置。对应 config.key_display 的 Rust 侧语义层。

use serde::{Deserialize, Serialize};

use crate::core::color::Argb;

/// 弹窗位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    TopLeft,
    TopRight,
    BottomLeft,
    #[default]
    BottomRight,
}

impl Placement {
    pub fn is_bottom(self) -> bool {
        matches!(self, Self::BottomLeft | Self::BottomRight)
    }
    pub fn is_left(self) -> bool {
        matches!(self, Self::TopLeft | Self::BottomLeft)
    }
}

/// 显示模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DisplayMode {
    #[default]
    All,
    /// 「排除鼠标」：鼠标条目（含滚轮/侧键）全部不显示，键盘不受影响。
    NoMouse,
    ShortcutsFun,
    Shortcuts,
}

/// 击键回显设置。默认值来自 core/config.py:40-52。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyDisplaySettings {
    pub placement: Placement,
    pub top_offset: u16,
    pub bottom_offset: u16,
    pub left_offset: u16,
    pub right_offset: u16,
    /// 屏幕宽度的百分比。
    pub popup_width: u8,
    pub opacity: f32,
    /// 淡出延迟（秒）。
    pub fade_delay: f32,
    /// 字号（pt），layout.rs 里换算成 px。
    pub font_size: f32,
    pub font_color: Argb,
    pub fun_color: Argb,
    pub symbol_color: Argb,
    pub bg_color: Argb,
    /// 合并间隔（秒）。
    pub merge_interval: f32,
    pub display_mode: DisplayMode,
    /// 鼠标按键（左键 / 右键 / 中键 / 滚轮 / 侧键）要不要进弹窗。
    ///
    /// ★ **由 `display_mode` 派生**（2026-09-25）：只有「全部」模式回显鼠标；
    /// 「排除鼠标」显式关掉，「组合键 / 组合键 + 功能键」本就只显示键盘组合。
    /// 独立配置字段已删（用户口径：与模式下拉重复，不再单独暴露）。
    pub show_mouse: bool,
}

impl Default for KeyDisplaySettings {
    fn default() -> Self {
        Self {
            placement: Placement::BottomRight,
            top_offset: 0,
            bottom_offset: 5, // 2026-09-25 晚：垂直偏移出厂 5（原 10）
            left_offset: 0,
            right_offset: 0,
            popup_width: 15,
            opacity: 1.0,
            fade_delay: 10.0,
            font_size: 16.0,
            font_color: Argb::new(255, 255, 255, 0), // #FFFF00
            fun_color: Argb::new(255, 255, 0, 0),    // #FF0000
            symbol_color: Argb::new(255, 255, 255, 255), // #FFFFFF
            bg_color: Argb::new(0x40, 0, 0, 0),      // #00000040（2026-09-25 晚）
            merge_interval: 1.0,                     // 2026-09-25 晚（原 1.5）
            display_mode: DisplayMode::All,
            show_mouse: true,
        }
    }
}

/// 从 config 的 KeyDisplaySettings 转换。
pub fn from_config(config: &crate::core::config::KeyDisplaySettings) -> KeyDisplaySettings {
    let placement = match config.placement.as_str() {
        "top_left" => Placement::TopLeft,
        "top_right" => Placement::TopRight,
        "bottom_left" => Placement::BottomLeft,
        _ => Placement::BottomRight,
    };
    let display_mode = match config.display_mode.as_str() {
        "shortcuts" => DisplayMode::Shortcuts,
        "shortcuts_fun" => DisplayMode::ShortcutsFun,
        "no_mouse" => DisplayMode::NoMouse,
        _ => DisplayMode::All,
    };

    let rgba_to_argb = |rgba: [u8; 4]| Argb::new(rgba[3], rgba[0], rgba[1], rgba[2]);

    KeyDisplaySettings {
        placement,
        top_offset: config.top_offset.max(0) as u16,
        bottom_offset: config.bottom_offset.max(0) as u16,
        left_offset: config.left_offset.max(0) as u16,
        right_offset: config.right_offset.max(0) as u16,
        popup_width: config.popup_width.clamp(5, 50) as u8,
        opacity: config.opacity.clamp(0.05, 1.0),
        fade_delay: (config.fade_delay as f32).clamp(0.5, 30.0),
        font_size: (config.font_size as f32).clamp(8.0, 72.0),
        font_color: rgba_to_argb(config.font_color),
        fun_color: rgba_to_argb(config.fun_color),
        symbol_color: rgba_to_argb(config.symbol_color),
        bg_color: rgba_to_argb(config.bg_color),
        merge_interval: config.merge_interval.clamp(0.1, 30.0),
        show_mouse: display_mode == DisplayMode::All,
        display_mode,
    }
}

/// 常量（plan.md M6-2）。
pub const FADE_DURATION: f32 = 0.5;
pub const REMOVE_DELAY: f32 = 1.0;
pub const MOD_STUCK_TIMEOUT: f32 = 5.0;
pub const MAX_ITEMS: usize = 64;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values() {
        let s = KeyDisplaySettings::default();
        assert_eq!(s.placement, Placement::BottomRight);
        assert_eq!(s.popup_width, 15);
        assert_eq!(s.opacity, 1.0);
        assert_eq!(s.fade_delay, 10.0);
        assert_eq!(s.font_size, 16.0);
        assert_eq!(s.merge_interval, 1.0); // 2026-09-25 晚（原 1.5）
        assert_eq!(s.bottom_offset, 5); // 2026-09-25 晚（原 10）
        assert_eq!(s.display_mode, DisplayMode::All);
    }

    #[test]
    fn placement_semantics() {
        assert!(Placement::BottomRight.is_bottom());
        assert!(Placement::BottomLeft.is_bottom());
        assert!(!Placement::TopLeft.is_bottom());
        assert!(Placement::TopLeft.is_left());
        assert!(Placement::BottomLeft.is_left());
        assert!(!Placement::TopRight.is_left());
    }

    /// ★ 开关必须**从配置透传**（判据取非默认值 false：只测默认 true 的话，
    /// `from_config` 里写成常量 `true` 也是绿的）。
    /// ★ 「排除鼠标」模式：配置串 no_mouse 必须解析到 NoMouse（未知值仍回退 All）。
    #[test]
    fn display_mode_parses_no_mouse() {
        let mk = |m: &str| crate::core::config::KeyDisplaySettings {
            display_mode: m.into(),
            ..Default::default()
        };
        assert!(matches!(
            from_config(&mk("no_mouse")).display_mode,
            DisplayMode::NoMouse
        ));
        assert!(matches!(
            from_config(&mk("shortcuts")).display_mode,
            DisplayMode::Shortcuts
        ));
        // 不认识的值回退「全部」。
        assert!(matches!(
            from_config(&mk("不认识的值")).display_mode,
            DisplayMode::All
        ));
    }

    /// ★ `show_mouse` 由 `display_mode` **派生**（2026-09-25 删独立开关）：
    /// 只有「全部」回显鼠标；「排除鼠标 / 组合键*」都不回显。
    #[test]
    fn show_mouse_is_derived_from_display_mode() {
        let mk = |m: &str| crate::core::config::KeyDisplaySettings {
            display_mode: m.into(),
            ..Default::default()
        };
        assert!(from_config(&mk("all")).show_mouse, "「全部」应当回显鼠标");
        assert!(
            !from_config(&mk("no_mouse")).show_mouse,
            "「排除鼠标」必须关掉鼠标回显"
        );
        assert!(
            !from_config(&mk("shortcuts")).show_mouse,
            "「组合键」只显示键盘组合，鼠标不回显"
        );
        assert!(
            !from_config(&mk("shortcuts_fun")).show_mouse,
            "「组合键 + 功能键」同上"
        );
    }
}
