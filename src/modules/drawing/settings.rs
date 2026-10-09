//! 书写模块设置。对应 config.drawing 的 Rust 侧语义层。

use serde::{Deserialize, Serialize};

use crate::core::config;

use super::pens::Pen;

/// 书写快捷键。**直接复用 `core::config` 的定义**而不是再抄一份：
/// 早先这里有一份 11 字段的副本，其默认值与配置层的默认值靠人肉同步，改一处漏一处。
/// 快捷键表只有"一堆字符串"这一种形状，不存在模块层需要不同表示的理由。
pub use crate::core::config::DrawingHotkeys;

/// 书写设置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DrawingSettings {
    pub hotkeys: DrawingHotkeys,
    pub pens: Vec<Pen>,
}

impl Default for DrawingSettings {
    fn default() -> Self {
        Self {
            hotkeys: DrawingHotkeys::default(),
            pens: super::pens::default_pens(),
        }
    }
}

/// 从 config 的 DrawingSettings 转换。
pub fn from_config(config: &config::DrawingSettings) -> DrawingSettings {
    let pens: Vec<Pen> = config.pens.iter().map(Pen::from_config).collect();
    let pens = if pens.is_empty() {
        DrawingSettings::default().pens
    } else {
        pens
    };

    DrawingSettings {
        hotkeys: config.hotkeys.clone(),
        pens,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values() {
        let s = DrawingSettings::default();
        assert_eq!(s.pens.len(), 3);
        assert_eq!(s.pens[0].width, 4);
        assert_eq!(s.pens[1].width, 30);
    }

    #[test]
    fn hotkey_defaults() {
        let h = DrawingHotkeys::default();
        // 2026-09-27 用户口径：进标注 / 进白板出厂留空；画笔等内部切换键仍是单键。
        assert_eq!(h.draw_hk_screen, "");
        assert_eq!(h.draw_hk_pen1, "1");
    }

    #[test]
    fn from_config_carries_hotkeys() {
        let mut cfg = config::Config::default();
        cfg.drawing.hotkeys.draw_hk_line = "L".into();
        let s = from_config(&cfg.drawing);
        assert_eq!(s.hotkeys.draw_hk_line, "L");
        assert_eq!(s.hotkeys.draw_hk_text, "9");
    }
}
