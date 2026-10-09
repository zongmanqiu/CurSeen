//! 画笔定义、三支默认笔、PenStyle。

use serde::{Deserialize, Serialize};

use crate::core::color::Argb;

/// 画笔样式。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PenStyle {
    #[default]
    Solid,
    Dashed,
    Dotted,
    DashDot,
}

/// 画笔定义。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pen {
    pub color: Argb,
    pub width: u16,
    pub style: PenStyle,
}

impl Default for Pen {
    fn default() -> Self {
        Self {
            color: Argb::new(0xFF, 0x00, 0x00, 0x00), // 黑色
            width: 4,
            style: PenStyle::Solid,
        }
    }
}

/// 三支默认笔。
pub fn default_pens() -> Vec<Pen> {
    vec![
        Pen {
            color: Argb::new(0xFF, 0x00, 0x00, 0x00),
            width: 4,
            style: PenStyle::Solid,
        }, // 黑细笔
        Pen {
            color: Argb::new(0x40, 0xFF, 0x00, 0x00),
            width: 30,
            style: PenStyle::Solid,
        }, // 红高亮（alpha=0x40≈25%）
        Pen {
            color: Argb::new(0xFF, 0x00, 0xFF, 0x00),
            width: 4,
            style: PenStyle::Dashed,
        }, // 绿虚线
    ]
}

impl Pen {
    /// 从 config::Pen 转换。
    pub fn from_config(p: &crate::core::config::Pen) -> Self {
        let color = Argb::parse_hex(&p.color).unwrap_or_default();
        let width = (p.width as u16).clamp(1, 100);
        let style = match p.style.as_str() {
            "dashed" => PenStyle::Dashed,
            "dotted" => PenStyle::Dotted,
            "dashdot" => PenStyle::DashDot,
            _ => PenStyle::Solid,
        };
        Self {
            color,
            width,
            style,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_pen_values() {
        let p = Pen::default();
        assert_eq!(p.width, 4);
        assert_eq!(p.style, PenStyle::Solid);
    }

    #[test]
    fn three_default_pens() {
        let pens = default_pens();
        assert_eq!(pens.len(), 3);
        assert_eq!(pens[0].width, 4);
        assert_eq!(pens[1].width, 30);
        assert_eq!(pens[2].style, PenStyle::Dashed);
    }

    #[test]
    fn pen_from_config() {
        let cp = crate::core::config::Pen {
            color: "#FF0000".into(),
            width: 10,
            style: "dashed".into(),
        };
        let p = Pen::from_config(&cp);
        assert_eq!(p.width, 10);
        assert_eq!(p.style, PenStyle::Dashed);
    }
}
