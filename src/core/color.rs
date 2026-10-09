//! RGBA 颜色工具。与 Python 版 `QColor.name(HexArgb)` 互操作。

/// 带 alpha 的 RGBA 颜色（非预乘，便于配置与 UI 表达）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const TRANSPARENT: Rgba = Rgba {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };

    /// 解析 `"#RRGGBB"` 与 `"#RRGGBBAA"`（大小写不敏感，`#` 可省略）。
    ///
    /// ★★ 8 位的字节顺序是 **R,G,B,A**（不是 A,R,G,B）。全项目（`config.json`、
    /// Lua 脚本 `default = "#FF000040"`、windui `Color::to_hex_string`、配置里的
    /// `[R,G,B,A]` 数组）都是这个口径；2026-09-25 之前这里是 `#AARRGGBB`，
    /// 结果脚本里"红 + 25% 透明"被读成不透明深蓝、`#00FFCCBF` 被读成全透明。
    pub fn parse_hex(s: &str) -> Option<Rgba> {
        let s = s.trim_start_matches('#');
        match s.len() {
            6 => {
                let rgb = u32::from_str_radix(s, 16).ok()?;
                Some(Rgba {
                    r: (rgb >> 16) as u8,
                    g: (rgb >> 8) as u8,
                    b: rgb as u8,
                    a: 255,
                })
            }
            8 => {
                let rgba = u32::from_str_radix(s, 16).ok()?;
                Some(Rgba {
                    r: (rgba >> 24) as u8,
                    g: (rgba >> 16) as u8,
                    b: (rgba >> 8) as u8,
                    a: rgba as u8,
                })
            }
            _ => None,
        }
    }

    /// 输出 `"#RRGGBBAA"`（大写，带 `#`）。
    ///
    /// 与 [`Self::parse_hex`] 严格互逆；不带 alpha 的场合用 [`Self::to_hex_rgb`]。
    pub fn to_hex_rgba(self) -> String {
        format!("#{:02X}{:02X}{:02X}{:02X}", self.r, self.g, self.b, self.a)
    }

    /// 输出 `"#RRGGBB"`（忽略 alpha）。
    pub fn to_hex_rgb(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    /// 从 `[r, g, b, a]` 数组构造。
    pub fn from_array(v: [u8; 4]) -> Rgba {
        Rgba {
            r: v[0],
            g: v[1],
            b: v[2],
            a: v[3],
        }
    }

    /// 转为 `[r, g, b, a]`。
    pub fn to_array(self) -> [u8; 4] {
        [self.r, self.g, self.b, self.a]
    }

    /// 乘算 alpha（用于 `global_opacity × item.opacity`）。
    pub fn scale_alpha(self, f: f32) -> Rgba {
        let a = ((self.a as f32) * f.clamp(0.0, 1.0)) as u8;
        Rgba {
            r: self.r,
            g: self.g,
            b: self.b,
            a,
        }
    }

    /// HSV → RGBA。h: [0,360), s/v: [0,1]。
    #[allow(clippy::if_same_then_else)]
    pub fn from_hsv(h: f32, s: f32, v: f32, a: u8) -> Rgba {
        let c = v * s;
        let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
        let m = v - c;
        let (r, g, b) = if h < 60.0 {
            (c, x, 0.0)
        } else if h < 120.0 {
            (x, c, 0.0)
        } else if h < 180.0 {
            (0.0, c, x)
        } else if h < 240.0 {
            (0.0, x, c)
        } else if h < 300.0 {
            (x, 0.0, c)
        } else {
            (c, 0.0, x)
        };
        Rgba {
            // 四舍五入而非截断：half 落在半格（如 h=210 的绿通道 0.5）应进位 128，
            // 截断会得到 127，既偏离视觉预期也让 from_hsv_known_hues 这类精确断言失准。
            r: ((r + m) * 255.0).round() as u8,
            g: ((g + m) * 255.0).round() as u8,
            b: ((b + m) * 255.0).round() as u8,
            a,
        }
    }

    /// RGBA → HSV，返回 `(h, s, v)`。h: [0,360), s/v: [0,1]。
    #[allow(clippy::if_same_then_else)]
    pub fn to_hsv(self) -> (f32, f32, f32) {
        let r = self.r as f32 / 255.0;
        let g = self.g as f32 / 255.0;
        let b = self.b as f32 / 255.0;
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let d = max - min;
        let v = max;
        let s = if max == 0.0 { 0.0 } else { d / max };
        let h = if d == 0.0 {
            0.0
        } else if max == r {
            60.0 * ((g - b) / d % 6.0)
        } else if max == g {
            60.0 * ((b - r) / d + 2.0)
        } else if max == b {
            60.0 * ((r - g) / d + 4.0)
        } else {
            0.0
        };
        let h = if h < 0.0 { h + 360.0 } else { h };
        (h, s, v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hex_rgb() {
        let c = Rgba::parse_hex("#FF0000").unwrap();
        assert_eq!(
            c,
            Rgba {
                r: 255,
                g: 0,
                b: 0,
                a: 255
            }
        );
    }

    /// ★ 回归锁：8 位串是 `#RRGGBBAA`，alpha 在**末尾**。
    ///
    /// 2026-09-25 前这里是 `#AARRGGBB`，于是 Lua 里"红 + 25% 透明"（`#FF000040`）
    /// 被读成不透明深蓝、`#00FFCCBF` 被读成全透明——颜色悄悄错却不报错。
    #[test]
    fn parse_hex_rgba8() {
        // #FF000080 = 红 + alpha 0x80（不是"alpha FF 的 000080"）。
        let c = Rgba::parse_hex("#FF000080").unwrap();
        assert_eq!(
            c,
            Rgba {
                r: 255,
                g: 0,
                b: 0,
                a: 128
            }
        );
        // 反向钉死：`#FF000080` 若被当成老口径 AARRGGBB 读，会得到 alpha=FF 的
        // 深蓝（0x00,0x00,0x80）。这里断言的是"红 + 半透明"，两者绝不能混。
        assert_ne!(
            (c.r, c.g, c.b, c.a),
            (0x00, 0x00, 0x80, 0xFF),
            "alpha 必须在末尾，不能读成 AARRGGBB"
        );
    }

    #[test]
    fn parse_hex_no_hash() {
        let c = Rgba::parse_hex("00FF00").unwrap();
        assert_eq!(
            c,
            Rgba {
                r: 0,
                g: 255,
                b: 0,
                a: 255
            }
        );
    }

    #[test]
    fn hex_roundtrip() {
        let c = Rgba {
            r: 0x12,
            g: 0xAB,
            b: 0xCD,
            a: 0xEF,
        };
        let s = c.to_hex_rgba();
        assert_eq!(s, "#12ABCDEF");
        assert_eq!(Rgba::parse_hex(&s).unwrap(), c);
    }

    #[test]
    fn scale_alpha_basic() {
        let c = Rgba {
            r: 255,
            g: 0,
            b: 0,
            a: 200,
        };
        let s = c.scale_alpha(0.5);
        assert_eq!(s.a, 100);
    }

    #[test]
    fn hsv_roundtrip() {
        let c = Rgba {
            r: 255,
            g: 128,
            b: 0,
            a: 255,
        };
        let (h, s, v) = c.to_hsv();
        let c2 = Rgba::from_hsv(h, s, v, 255);
        // 允许 ±1 误差（整数量化）
        assert!((c.r as i16 - c2.r as i16).abs() <= 1);
        assert!((c.g as i16 - c2.g as i16).abs() <= 1);
        assert!((c.b as i16 - c2.b as i16).abs() <= 1);
    }

    #[test]
    fn from_hsv_known_hues() {
        // 已知色相必须落对通道（回归：180°–300° 段曾写反，导致青/蓝区间色相错乱）。
        // h=0 红、h=120 绿、h=240 蓝、h=300 品红。
        let red = Rgba::from_hsv(0.0, 1.0, 1.0, 255);
        assert_eq!((red.r, red.g, red.b), (255, 0, 0));
        let green = Rgba::from_hsv(120.0, 1.0, 1.0, 255);
        assert_eq!((green.r, green.g, green.b), (0, 255, 0));
        let blue = Rgba::from_hsv(240.0, 1.0, 1.0, 255);
        assert_eq!((blue.r, blue.g, blue.b), (0, 0, 255));
        let magenta = Rgba::from_hsv(300.0, 1.0, 1.0, 255);
        assert_eq!((magenta.r, magenta.g, magenta.b), (255, 0, 255));
        // 中段（写反的高发带）同样正确：h=180 青 (0,255,255)、h=210 蓝偏绿 (0,128,255)。
        let cyan = Rgba::from_hsv(180.0, 1.0, 1.0, 255);
        assert_eq!((cyan.r, cyan.g, cyan.b), (0, 255, 255));
        let mid = Rgba::from_hsv(210.0, 1.0, 1.0, 255);
        assert_eq!((mid.r, mid.g, mid.b), (0, 128, 255));
    }
}

// ─── Argb（u32，0xAARRGGBB）── 用于 DrawList，与 Python 版配置文件一致 ───

/// 32 位 ARGB 颜色（0xAARRGGBB）。用于绘制指令表，与 §6 设计令牌一致。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Argb(pub u32);

impl Argb {
    pub const TRANSPARENT: Argb = Argb(0);

    pub fn new(a: u8, r: u8, g: u8, b: u8) -> Self {
        Self(((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32))
    }

    pub fn a(self) -> u8 {
        (self.0 >> 24) as u8
    }
    pub fn r(self) -> u8 {
        (self.0 >> 16) as u8
    }
    pub fn g(self) -> u8 {
        (self.0 >> 8) as u8
    }
    pub fn b(self) -> u8 {
        self.0 as u8
    }

    /// 解析 "#RRGGBBAA" 或 "#RRGGBB"。
    ///
    /// ★★ 8 位串的字节顺序是 **R,G,B,A**：与 `config.json`、Lua 脚本、windui
    /// `Color::to_hex_string` 一致。这里早年写成 `#AARRGGBB`，与脚本端对不上
    /// （`#FF000040` 读成不透明深蓝而非"红 + 25% 透明"），2026-09-25 统一改回。
    pub fn parse_hex(s: &str) -> Option<Argb> {
        let s = s.trim_start_matches('#');
        match s.len() {
            6 => {
                let rgb = u32::from_str_radix(s, 16).ok()?;
                Some(Argb(0xFF000000 | rgb))
            }
            8 => {
                let rgba = u32::from_str_radix(s, 16).ok()?;
                // 落盘/脚本是 RRGGBBAA，内存 u32 是 0xAARRGGBB —— 在这里做一次轮转，
                // 让"字节顺序"这回事只在这一处存在。
                Some(Argb(rgba.rotate_right(8)))
            }
            _ => None,
        }
    }

    /// 从 Rgba 转换。
    pub fn from_rgba(c: Rgba) -> Argb {
        Argb::new(c.a, c.r, c.g, c.b)
    }

    /// 转为 Rgba。
    pub fn to_rgba(self) -> Rgba {
        Rgba {
            r: (self.0 >> 16) as u8,
            g: ((self.0 >> 8) & 0xFF) as u8,
            b: (self.0 & 0xFF) as u8,
            a: (self.0 >> 24) as u8,
        }
    }

    /// 乘算 alpha。
    pub fn with_alpha(self, a: u8) -> Self {
        Self((self.0 & 0x00FFFFFF) | ((a as u32) << 24))
    }

    /// ★ 输出 `"#RRGGBBAA"`：`config.json` / Lua 脚本 / windui 取色器的唯一口径。
    ///
    /// 面板与脚本里所有"颜色字符串"都必须走这里（或 [`Rgba::to_hex_rgba`]），
    /// 不要再用 `format!("#{:02X}..", a, r, g, b)` 手写 —— 手写过的地方正是
    /// `#AARRGGBB` 漏出来的地方。
    pub fn to_hex_rgba(self) -> String {
        let c = self.to_rgba();
        format!("#{:02X}{:02X}{:02X}{:02X}", c.r, c.g, c.b, c.a)
    }

    /// ★ 反色（RGB 逐通道取反，alpha 置满）。
    ///
    /// 序号贴纸的数字用：圆圈填充画笔色，数字用反色 —— 无论画笔是什么颜色，
    /// 数字与圆底都处在对比最大的两端。半透明画笔（如 α=0x40 的荧光笔）也返回
    /// **不透明**反色：数字叠在实心圆底上，不能跟着透。
    pub fn inverse_solid(self) -> Self {
        Argb::new(255, 255 - self.r(), 255 - self.g(), 255 - self.b())
    }
}

#[cfg(test)]
mod argb_tests {
    use super::*;

    #[test]
    fn argb_parse_hex() {
        // 6 位（#RRGGBB）→ alpha 补 FF
        assert_eq!(Argb::parse_hex("#FF0000"), Some(Argb(0xFFFF0000)));
        // 8 位（#RRGGBBAA）→ 内存仍是 0xAARRGGBB
        assert_eq!(Argb::parse_hex("#FF000080"), Some(Argb(0x80FF0000)));
        // 带不带 # 都行
        assert_eq!(Argb::parse_hex("FF0000"), Some(Argb(0xFFFF0000)));
        // 长度不合法 → None（不是 panic，配置里写错颜色要能被兜住）
        assert_eq!(Argb::parse_hex("#FFF"), None);
        assert_eq!(Argb::parse_hex("#FF00000"), None);
    }

    #[test]
    fn argb_with_alpha() {
        let c = Argb(0xFFFF0000);
        assert_eq!(c.with_alpha(0x40), Argb(0x40FF0000));
        // 边界：全透与全不透，且不能污染 RGB
        assert_eq!(c.with_alpha(0), Argb(0x00FF0000));
        assert_eq!(c.with_alpha(255), Argb(0xFFFF0000));
    }

    /// ★ 落盘/脚本口径是 `#RRGGBBAA`：解析 → 写回必须逐字节还原。
    #[test]
    fn argb_parse_roundtrip() {
        for s in ["#12ABCDEF", "#00000000", "#FFFFFFFF", "#FF000080"] {
            let c = Argb::parse_hex(s).unwrap();
            assert_eq!(c.to_hex_rgba(), s.to_uppercase());
        }
    }
}
