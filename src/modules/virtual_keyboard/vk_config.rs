//! 虚拟键盘方案（根目录 `*.json`）的加载与样式解析。
//!
//! # 方案是什么
//!
//! `assets/VirtualKeyboards/<方案名>.json` 一个文件即一套方案，内容五块：
//!
//! | 块 | 作用 | 省略时 |
//! |---|---|---|
//! | `rows` | **几何**：键位排布（行数组 + u 单位 + 每键 w/h/pad） | 用 `baseLayout` 引用的出厂几何 |
//! | `defaults` | 字号 / 圆角 / 缩放 / 总不透明度 | 用 Rust 内置默认 |
//! | `styles` + `stateStyles` | 命名样式（CSS 思路） | 用面板上的四个颜色 |
//! | `keys` | 逐键覆盖（文字 / 上档文字 / 样式 / 圆角） | 用几何里写的那份 |
//! | `controls` | **面板控件清单**：暴露哪些选项、叫什么、默认多少、范围多大 | 由代码自动补齐一套预设 |
//!
//! 「虚拟键盘」下拉里的每一项就是扫描 `VirtualKeyboards/` 扫出来的一份方案 ——
//! **用户丢一个 JSON 进去就能用**（见 `layout::discover`）。
//!
//! # 控件（面板上的选项 = 方案说了算）
//!
//! 面板子卡里的**每一行都是方案声明出来的**，与 Lua 脚本
//! 暴露参数输入是同一套思路：方案作者决定"哪些可调、叫什么、默认多少、范围多大"。
//! 没写 `controls` 时代码自动补齐 —— 预设项（键帽大小 / 空隙 / 圆角 / 不透明度 /
//! 高亮保持 / 字号 + 四个兜底色）**加上**方案里每个命名样式的颜色，于是"方案给某些
//! 键上了别的颜色"这种情况在面板里也一定有对应的色块可改（不会出现"看得见改不了"）。
//!
//! 有三个选项是**全局行为**、方案不能干预：鼠标穿透 / 总在最上 / 全屏避让 —— 它们
//! 改的是窗口与输入的行为，让方案预设一个开关值只会让用户猜不到当前状态。
//! 「虚拟键盘」卡上的位置与四向偏移同理（几何位置是全局配置）。
//!
//! # 分层优先级（低→高）
//!
//! ```text
//! Rust 内置默认值
//!   ↑ 方案 defaults 块（仅当面板值仍等于出厂默认 —— 见 VkConfig::load）
//!     ↑ 方案 styles[name]
//!       ↑ 几何里的逐键 style/radius/text
//!         ↑ 方案 keys[code] 覆盖表
//!           ↑ UI 面板实时调整（最高）
//! ```
//!
//! ★ 面板最高这条**曾经只写在注释里、实现是反的**：`font_size` 是"面板值作初值 →
//! 被 JSON defaults 覆盖"，`corner_radius` / `scale` 干脆只读 JSON。现已在
//! [`VkConfig::load`] 里按上表落实。
//!
//! ★ **Shift 图例不再有开关**（2.x）：真键盘的键帽本来就是"下档在上、上档在下"，
//! 按 Shift 显示上档天经地义——输入法都这么做，做成开关只会让人以为"没用"。
//! 上档文字的来源有两处：几何里的 `shiftText`（省略 = 与 `text` 相同），
//! 以及 `keys[code].shiftText`（不改几何只改文字）。

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;

use serde::Deserialize;

use super::layout::{self, KeyDef, Layout, LayoutJson};
use crate::core::color::Argb;
use crate::core::config::{ParamValue, VirtualKeyboardSettings};

// ─── JSON 数据结构 ───

/// 完整的 keyboard.json 结构（反序列化用的壳，落到 [`Scheme`]）。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct VkJson {
    /// 几何块（`name` / `baseLayout` / `rows` 都在这里）。
    #[serde(flatten)]
    pub geom: LayoutJson,
    /// 文件级默认值（覆盖 Rust 内置默认值）。
    pub defaults: VkDefaults,
    /// 命名样式表：key = 样式名，value = 样式定义。
    pub styles: HashMap<String, VkStyleDef>,
    /// 状态样式：`pressed` / `disabled` 等状态的视觉覆盖。
    pub state_styles: HashMap<String, VkStateStyle>,
    /// 按键级覆盖表：key = [`layout::code_to_name`] 的名字（如 `"Backspace"` / `"CharQ"`）。
    pub keys: HashMap<String, VkKeyOverride>,
    /// 面板控件清单（可选）。写了就按它暴露；没覆盖到的预设项仍然保留。
    pub controls: Option<Vec<ControlJson>>,
}

/// `controls` 里的一项（反序列化壳，落到 [`ControlDef`]）。
///
/// ★ 只有 `id` 与 `bind` 是必需的：其余都由代码推断/兜底，作者想改哪个写哪个。
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct ControlJson {
    /// 控件 id（方案内唯一，也是配置里落盘的键）。缺省用 `bind`。
    pub id: String,
    /// 面板上显示的名字。缺省按 bind 猜一个中文名。
    pub label: String,
    /// 绑到哪个量（见 [`Bind::parse`] 的路径语法）。空 = 该控件无效，跳过。
    pub bind: String,
    /// 控件类型：`color` / `range` / `bool`。缺省按 bind 猜（颜色类 → `color`）。
    #[serde(rename = "type")]
    pub kind: String,
    /// 归到哪一组（面板用它决定行序）：`look`（数值项）/ `color`（颜色项）。缺省按类型猜。
    pub group: String,
    pub min: Option<f32>,
    pub max: Option<f32>,
    pub step: Option<f32>,
    /// 滑块显示的小数位（0 = 整数）。缺省按 step 猜（step<1 → 1 位）。
    pub decimals: Option<u8>,
    /// 默认值（不给 = 用方案里已经写好的那个值 / 内置默认）。
    pub default: Option<crate::core::config::ParamValue>,
}

/// 文件级默认值。
///
/// ★ 只收**外观类**参数：行为类开关（鼠标穿透 / 总在最上 / 全屏避让）留在面板上，
///   因为它们改的是窗口与输入的行为，方案里预设一个"默认值"会让人猜不到当前状态。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct VkDefaults {
    pub font_size: Option<u32>,
    pub opacity: Option<f32>,
    pub corner_radius: Option<f32>,
    pub scale: Option<f32>,
    /// 一个标准键的边长（逻辑像素）。
    pub key_size: Option<u32>,
    /// 键间空隙（逻辑像素）。
    pub gap: Option<u32>,
    /// 按下后高亮保持（秒）。
    pub hold: Option<f32>,
}

/// 命名样式定义（CSS 思路）。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct VkStyleDef {
    /// 面板上的显示名（自动生成的控件用它做标签前缀）。缺省用样式的键名。
    pub label: Option<String>,
    /// 填充色（`#RRGGBB` 或 `#RRGGBBAA`）。
    pub fill: Option<String>,
    /// 文字色。
    pub text: Option<String>,
    /// 描边色。
    pub border: Option<String>,
    /// **本样式的按下色**（`#RRGGBB` / `#RRGGBBAA`）。
    ///
    /// ★ 双色方案要两种按下色（输入键蓝、功能键紫），而 `stateStyles.pressed` 是**全局
    ///   一个**。样式级按下色优先级更高；没写才回落到全局按下态、再回落到兜底按下色。
    pub press: Option<String>,
    /// 圆角半径（逻辑像素）。
    pub radius: Option<f32>,
    /// **侧壁色**（`#RRGGBBAA`，可选）。
    ///
    /// ★ 键帽"立体感"的通用画法（2026-09-28，随四套外来样式方案一起加）：先在键帽
    ///   矩形**正下方偏移 `depth` px** 处画一颗侧壁色键帽，再画键帽本体 —— 侧壁从
    ///   底边露出一圈，就是"键帽有厚度"的视觉。"硬阴影"键帽（深色侧壁）与
    ///   "双层"键帽（裙边色侧壁 + 顶面色本体）都是这一个特性的参数化。
    ///
    /// 按下时本体**向侧壁方向压入**（见 `mod::draw`）：侧壁从底边露出的那圈被
    /// 本体"坐进去"，正是真键帽按下去的物理感 —— 不需要额外的"按下位移"参数。
    pub side: Option<String>,
    /// 侧壁厚度（逻辑像素，0 = 无侧壁）。缺省 3。
    pub depth: Option<f32>,
}

/// 状态样式（pressed / disabled 等）。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct VkStateStyle {
    pub fill: Option<String>,
    pub text: Option<String>,
}

/// 单个按键的覆盖定义。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct VkKeyOverride {
    /// 自定义显示文字（覆盖几何里的 `text`）。
    pub text: Option<String>,
    /// Shift 态的自定义显示文字（覆盖几何里的 `shiftText`）。
    #[serde(rename = "shiftText", alias = "shift_text")]
    pub shift_text: Option<String>,
    /// 引用的样式名（指向 `styles` 表）。
    pub style: Option<String>,
    /// 单键圆角覆盖。
    pub radius: Option<f32>,
}

// ─── 控件：面板上每一行选项都是方案声明出来的 ───

/// 一个控件改的是**哪个量**。
///
/// ★ 这是"方案 JSON 里的 `bind` 字符串"与"代码里的取值点"之间唯一的翻译层：
/// 新增一个可调量 = 加一个变体 + 在 [`Bind::parse`] / [`VkConfig::fallback_num`] /
/// [`VkConfig::fallback_color`] 三处各补一行。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Bind {
    // ── 文件级默认值 ──
    /// 一个标准键的边长（`defaults.keySize`）。
    KeySize,
    /// 键间空隙（`defaults.gap`）。
    Gap,
    /// 总不透明度（`defaults.opacity`）。
    Opacity,
    /// 高亮保持秒数（`defaults.hold`）。
    Hold,
    /// 字号（`defaults.fontSize`）。
    FontSize,
    /// 全局圆角（`defaults.cornerRadius`）。
    CornerRadius,
    /// 缩放（`defaults.scale`）。
    Scale,
    // ── 面板上的四个兜底色 ──
    /// 没引用命名样式的键用的底色（`base.fill`）。
    BaseFill,
    /// 按下色兜底（`base.press`）。
    BasePress,
    /// 描边色兜底（`base.border`）。
    BaseBorder,
    /// 文字色兜底（`base.text`）。
    BaseText,
    // ── 按下态 ──
    PressFill,
    PressText,
    // ── 命名样式 ──
    StyleFill(String),
    StyleText(String),
    StyleBorder(String),
    /// 某个命名样式的**按下色**（`styles.<样式名>.press`）。
    StylePress(String),
    StyleRadius(String),
    /// 某个命名样式的**侧壁色**（`styles.<样式名>.side`）。
    StyleSide(String),
    /// 某个命名样式的**侧壁厚度**（`styles.<样式名>.depth`，逻辑像素）。
    StyleDepth(String),
    // ── 逐键 ──
    KeyRadius(String),
}

impl Bind {
    /// 路径 → [`Bind`]。认不出来（拼错 / 该目标不提供控件）返回 `None`。
    ///
    /// 语法（大小写与下划线都不敏感，样式名 / 键名除外）：
    ///
    /// ```text
    /// defaults.keySize | gap | opacity | hold | fontSize | cornerRadius | scale
    /// base.fill | press | border | text
    /// pressed.fill | text
    /// styles.<样式名>.fill | press | text | border | radius
    /// keys.<键名>.radius
    /// ```
    ///
    /// ★ `styles.<样式名>.press` = **该样式的按下色**（双色方案要两种按下色，全局那个不够用）。
    ///   优先级：它 > 全局 `pressed.fill` > 兜底 `base.press`。
    pub fn parse(s: &str) -> Option<Self> {
        let norm = |x: &str| x.trim().to_ascii_lowercase().replace('_', "");
        let mut it = s.split('.');
        let head = norm(it.next().unwrap_or(""));
        let sec = it.next().unwrap_or("").trim();
        match head.as_str() {
            "defaults" | "default" => match norm(sec).as_str() {
                "keysize" => Some(Bind::KeySize),
                "gap" => Some(Bind::Gap),
                "opacity" => Some(Bind::Opacity),
                "hold" => Some(Bind::Hold),
                "fontsize" => Some(Bind::FontSize),
                "cornerradius" | "radius" => Some(Bind::CornerRadius),
                "scale" => Some(Bind::Scale),
                _ => None,
            },
            "base" => match norm(sec).as_str() {
                "fill" | "key" | "keycolor" => Some(Bind::BaseFill),
                "press" | "presscolor" => Some(Bind::BasePress),
                "border" => Some(Bind::BaseBorder),
                "text" => Some(Bind::BaseText),
                _ => None,
            },
            "pressed" | "statestyles" | "state" => {
                // `stateStyles.pressed.fill` 与 `pressed.fill` 都认。
                let third = if norm(sec).as_str() == "pressed" {
                    it.next().unwrap_or("").trim()
                } else {
                    sec
                };
                match norm(third).as_str() {
                    "fill" => Some(Bind::PressFill),
                    "text" => Some(Bind::PressText),
                    _ => None,
                }
            }
            "styles" | "style" => {
                let field = norm(it.next().unwrap_or(""));
                if sec.is_empty() {
                    return None;
                }
                match field.as_str() {
                    "fill" => Some(Bind::StyleFill(sec.to_string())),
                    "text" => Some(Bind::StyleText(sec.to_string())),
                    "border" => Some(Bind::StyleBorder(sec.to_string())),
                    "press" | "pressed" | "pressfill" => Some(Bind::StylePress(sec.to_string())),
                    "radius" => Some(Bind::StyleRadius(sec.to_string())),
                    "side" | "shadow" => Some(Bind::StyleSide(sec.to_string())),
                    "depth" | "sidedepth" => Some(Bind::StyleDepth(sec.to_string())),
                    _ => None,
                }
            }
            "keys" | "key" => {
                let field = norm(it.next().unwrap_or(""));
                if sec.is_empty() {
                    return None;
                }
                match field.as_str() {
                    "radius" => Some(Bind::KeyRadius(sec.to_string())),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// [`Bind::parse`] 的逆：给配置落盘与日志用的稳定路径。
    pub fn path(&self) -> String {
        match self {
            Bind::KeySize => "defaults.keySize".into(),
            Bind::Gap => "defaults.gap".into(),
            Bind::Opacity => "defaults.opacity".into(),
            Bind::Hold => "defaults.hold".into(),
            Bind::FontSize => "defaults.fontSize".into(),
            Bind::CornerRadius => "defaults.cornerRadius".into(),
            Bind::Scale => "defaults.scale".into(),
            Bind::BaseFill => "base.fill".into(),
            Bind::BasePress => "base.press".into(),
            Bind::BaseBorder => "base.border".into(),
            Bind::BaseText => "base.text".into(),
            Bind::PressFill => "pressed.fill".into(),
            Bind::PressText => "pressed.text".into(),
            Bind::StyleFill(n) => format!("styles.{n}.fill"),
            Bind::StyleText(n) => format!("styles.{n}.text"),
            Bind::StyleBorder(n) => format!("styles.{n}.border"),
            Bind::StylePress(n) => format!("styles.{n}.press"),
            Bind::StyleRadius(n) => format!("styles.{n}.radius"),
            Bind::StyleSide(n) => format!("styles.{n}.side"),
            Bind::StyleDepth(n) => format!("styles.{n}.depth"),
            Bind::KeyRadius(n) => format!("keys.{n}.radius"),
        }
    }

    /// 颜色类 → 用取色器；其余 → 滑块。
    pub fn is_color(&self) -> bool {
        matches!(
            self,
            Bind::BaseFill
                | Bind::BasePress
                | Bind::BaseBorder
                | Bind::BaseText
                | Bind::PressFill
                | Bind::PressText
                | Bind::StyleFill(_)
                | Bind::StyleText(_)
                | Bind::StyleBorder(_)
                | Bind::StylePress(_)
                | Bind::StyleSide(_)
        )
    }

    /// 预设显示名（方案没给 `label` 时用它）。
    pub fn default_label(&self) -> String {
        match self {
            Bind::KeySize => "键帽大小".into(),
            Bind::Gap => "键间空隙".into(),
            Bind::Opacity => "不透明度".into(),
            Bind::Hold => "高亮保持".into(),
            Bind::FontSize => "字号".into(),
            Bind::CornerRadius => "圆角".into(),
            Bind::Scale => "缩放".into(),
            Bind::BaseFill => "键帽色".into(),
            Bind::BasePress => "按下色".into(),
            Bind::BaseBorder => "描边色".into(),
            Bind::BaseText => "文字色".into(),
            Bind::PressFill => "按下色".into(),
            Bind::PressText => "按下文字色".into(),
            // ★ 样式色统一 `<样式名>_<颜色类型>`（下划线分隔，用户 2026-09-30 要求）：
            //   `输入键_键帽色` / `功能键_按下色` …… 比旧的中点点隔 `输入键·底色` 更直观。
            Bind::StyleFill(n) => format!("{n}_键帽色"),
            Bind::StyleText(n) => format!("{n}_文字色"),
            Bind::StyleBorder(n) => format!("{n}_描边色"),
            Bind::StylePress(n) => format!("{n}_按下色"),
            Bind::StyleSide(n) => format!("{n}_侧壁"),
            // ★ 圆角/厚度项一律 `圆角_<作用域>` / `侧壁厚度_<作用域>`：功能上它们与全局
            //   「圆角」是同一个量，只是作用范围不同 —— 公共前缀才排得拢、扫一眼就知道
            //   是一组（2026-09-18 用户要求）。
            Bind::StyleDepth(n) => format!("侧壁厚度_{n}"),
            Bind::StyleRadius(n) => format!("圆角_{n}"),
            Bind::KeyRadius(n) => format!("圆角_{n}"),
        }
    }

    /// 预设取值范围 `(min, max, step, 小数位)`。
    fn preset_range(&self) -> (f32, f32, f32, u8) {
        match self {
            // ★ 1u = 一个标准键的边长（逻辑像素）。整块 104 键约 15u 宽，所以
            //   10 → 约 150px、80 → 约 1200px（出厂 64 → 约 960px）。
            Bind::KeySize => (10.0, 80.0, 1.0, 0),
            Bind::Gap => (0.0, 20.0, 1.0, 0),
            Bind::CornerRadius | Bind::StyleRadius(_) | Bind::KeyRadius(_) => (0.0, 20.0, 1.0, 0),
            Bind::StyleDepth(_) => (0.0, 12.0, 0.5, 1),
            Bind::Opacity => (0.05, 1.0, 0.01, 2),
            // ★ 下限 0 = **松开即灭**（淡出也跟着归零，见 `mod::fade_sec`）。2026-09-18
            //   用户实测：下限卡在 0.1 时"选了最小值还是灭不掉"，因为淡出恒为 0.25s，
            //   松开后总有约 0.35s 的余光。步长 0.05 让 0 附近有几档可挑。
            Bind::Hold => (0.0, 2.0, 0.05, 2),
            Bind::FontSize => (6.0, 28.0, 1.0, 0),
            Bind::Scale => (0.5, 2.0, 0.05, 2),
            _ => (0.0, 100.0, 1.0, 0),
        }
    }
}

/// 控件类型。
// ★ 不 derive `Eq`：变体里带着 `f32`（范围），浮点没有全序相等。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ControlKind {
    /// 滑块。
    Range {
        min: f32,
        max: f32,
        step: f32,
        decimals: u8,
    },
    /// 取色器。
    Color,
    /// 勾选框。
    Bool,
}

impl ControlKind {
    /// ★ **滑块信号恒为 `0..=1`**，控件的值却是真实量纲（12~96 像素、0.1~2.0 秒……）。
    ///
    /// windui 的 `slider_value_range(min, max, decimals)` 只改**旋钮右侧标签的显示**
    /// （`min + v*(max-min)`，CurSeen 补丁 014），`Signal<f32>` 里装的始终是归一化后的
    /// 比例。所以面板读写两端各要换算一次 —— 少了哪一端，症状都是"拖动滑块 → 存进
    /// 配置的是 0.xx → 键帽被压到最小值"（2026-09-18 实测：改一下键帽大小键盘就缩成
    /// 一小块，正是这里漏了换算）。
    ///
    /// 换算规则收在 [`ControlKind`] 自己身上，而不是散在 UI 层：`min/max/step/decimals`
    /// 本来就在这儿，UI 只需要 `kind.to_slider(v)` / `kind.from_slider(s)`。
    pub fn to_slider(&self, v: f32) -> f32 {
        match *self {
            ControlKind::Range { min, max, .. } => {
                if (max - min).abs() < f32::EPSILON {
                    0.0
                } else {
                    ((v - min) / (max - min)).clamp(0.0, 1.0)
                }
            }
            // 非滑块没有"位置"这一说。
            _ => v,
        }
    }

    /// 滑块位置 → 真实值：反向插值后按 `step` 吸附，再按 `decimals` 修约。
    ///
    /// 吸附是必需的：不吸附的话 0.37 回去是 47.999996，与方案默认的 48 差一点点，
    /// 于是"拖回原处"仍被判成"用户改过"（配置里凭空多出一项）。
    pub fn from_slider(&self, s: f32) -> f32 {
        match *self {
            ControlKind::Range {
                min,
                max,
                step,
                decimals,
            } => {
                let v = min + s.clamp(0.0, 1.0) * (max - min);
                let v = if step > 0.0 {
                    (v / step).round() * step
                } else {
                    v
                };
                let p = 10f32.powi(decimals as i32);
                ((v.clamp(min, max) * p).round() / p).clamp(min, max)
            }
            _ => s,
        }
    }

    /// 这个值落在本控件的合法域里吗（用于过滤配置里手改坏的值）？
    pub fn accepts(&self, v: &ParamValue) -> bool {
        match (*self, v) {
            (ControlKind::Range { min, max, .. }, ParamValue::Float(x)) => {
                x.is_finite() && *x >= min - 1e-3 && *x <= max + 1e-3
            }
            (ControlKind::Color, ParamValue::Color(c)) => Argb::parse_hex(c).is_some(),
            (ControlKind::Bool, ParamValue::Bool(_)) => true,
            // 类型对不上（用户手改 JSON 写了个字符串给滑块）→ 不认。
            _ => false,
        }
    }
}

/// 一个方案控件是**数值项**还是**颜色项**。
///
/// 面板侧的消费方式：**每组各成一张子卡**（`Look` / `Color` 各自一趟渲染，见
/// `ui::pages::key_display::control_rows`）。所以这个分类决定的是**控件归哪一组**。
///
/// 卡叫什么、排第几、并成一张还是拆成两张，全是页面的事 —— 这一节曾写死过卡名，结果页面
/// 每调整一次分组，注释就跟着错一次。现在这里只声明组的语义，UI 呈现由页面决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlGroup {
    /// 数值项：键帽大小 / 键间空隙 / 圆角（含命名样式圆角）/ 不透明度 / 高亮保持 / 字号。
    Look,
    /// 颜色项：兜底色（含按下态）+ 命名样式色（底色 / 文字 / 描边）。
    Color,
}

/// 一个面板控件（方案声明或代码补齐后的最终形态）。
#[derive(Debug, Clone)]
pub struct ControlDef {
    /// 方案内唯一 id，也是配置里落盘的键（同一方案改标签/范围不会丢值）。
    pub id: String,
    pub label: String,
    pub bind: Bind,
    pub kind: ControlKind,
    pub group: ControlGroup,
    /// ★ **方案给的默认值**（不含用户改动）：面板拿它当滑块初值，也拿它判
    /// "这一项要不要写进配置"（当前值 == 它 → 不写）。
    pub default: ParamValue,
}

impl ControlDef {
    fn new(bind: Bind, label: String, default: ParamValue) -> Self {
        let kind = if bind.is_color() {
            ControlKind::Color
        } else {
            let (min, max, step, decimals) = bind.preset_range();
            ControlKind::Range {
                min,
                max,
                step,
                decimals,
            }
        };
        let group = if bind.is_color() {
            ControlGroup::Color
        } else {
            ControlGroup::Look
        };
        Self {
            id: bind.path(),
            label,
            bind,
            kind,
            group,
            default,
        }
    }
}

/// 生成一份方案的控件清单 = **预设项 + 方案里出现的命名样式 + 方案自己声明的**。
///
/// # 为什么预设项不因方案声明了 `controls` 就消失
///
/// 声明只是"改名 / 改范围 / 改默认"，不是"删掉"。真按"声明即全集"做，作者少写一项
/// 用户就少一个能调的东西 —— 上一次的教训正是"方案里有的颜色面板里改不了"。
/// 所以：**同一 `bind` 被声明覆盖，其余预设照旧保留**。
/// 兜底色（`base.fill` / `base.press` / `base.border` / `base.text`）是否**真的会被某颗键用到**。
///
/// 用不上就该藏起来 —— 面板上放一个"改了没反应"的色块比不给更糟。兜底色的取值顺序是
/// **样式级 > 兜底**，所以只要存在一颗键会一路落到兜底，它就还有用：
///
/// - 某颗键没有样式、或它的样式没写这个字段 → 那颗键用兜底 → 可达；
/// - 「按下色」多一层：方案写了全局 `stateStyles.pressed.fill` → 按下态恒被它压过 →
///   不可达（此时面板会出现绑到 `pressed.fill` 的「按下色」控件顶替，见 [`controls_of`]
///   第 3 步 —— 双色方案正是这条判据存在的理由：两套样式都写了 `press`、104 颗键全部
///   归到其中一套，于是 `base.press` 是死的）。
///
/// ★ 2026-09-26 扩展（用户报告：双色方案面板上还挂着旧的「键帽色/描边色/文字色」）：
/// 同一条判据推广到全部四个兜底色 —— 104 颗键全归了类、两套样式把 fill/text/border
/// 也都写齐了，这三个兜底色同样谁也用不到。
#[derive(Clone, Copy, PartialEq)]
enum BaseColor {
    Fill,
    Press,
    Border,
    Text,
}

fn base_color_is_reachable(scheme: &Scheme, want: BaseColor) -> bool {
    if want == BaseColor::Press {
        // 全局按下态能兜住所有按键的按下色 → 兜底「按下色」不可达。
        let global = scheme
            .state_styles
            .get("pressed")
            .and_then(|s| s.fill.as_deref());
        if let Some(hex) = global {
            if Argb::parse_hex(hex).is_some() {
                return false;
            }
        }
    }
    scheme.layout.rows.iter().any(|row| {
        row.keys.iter().any(|k| {
            scheme
                .keys
                .get(layout::code_to_name(k.code))
                .and_then(|o| o.style.as_deref())
                .or(k.style.as_deref())
                .and_then(|n| scheme.styles.get(n))
                .and_then(|st| base_style_field(st, want))
                .and_then(Argb::parse_hex)
                .is_none()
        })
    })
}

/// 命名样式在某个兜底色位上的值（独立成函数：闭包版本在这里会撞两个生命周期）。
fn base_style_field(st: &VkStyleDef, want: BaseColor) -> Option<&str> {
    match want {
        BaseColor::Fill => st.fill.as_deref(),
        BaseColor::Press => st.press.as_deref(),
        BaseColor::Border => st.border.as_deref(),
        BaseColor::Text => st.text.as_deref(),
    }
}

pub fn controls_of(scheme: &Scheme) -> Vec<ControlDef> {
    let base = VirtualKeyboardSettings::default();
    let mut out: Vec<ControlDef> = Vec::new();

    // 1. 预设外观项（顺序 = 面板上的顺序）。
    for b in [
        Bind::KeySize,
        Bind::Gap,
        Bind::CornerRadius,
        Bind::Opacity,
        Bind::Hold,
        Bind::FontSize,
    ] {
        out.push(preset(scheme, &base, b));
    }
    // 2. 四个兜底色 —— 只暴露**真会被某颗键用到**的那些（判据见
    //    [`base_color_is_reachable`]：全键归了类、样式色写齐的方案一个也不该给，
    //    否则面板上挂着一排"改了没反应"的旧色块）。
    for (bind, field) in [
        (Bind::BaseFill, BaseColor::Fill),
        (Bind::BasePress, BaseColor::Press),
        (Bind::BaseBorder, BaseColor::Border),
        (Bind::BaseText, BaseColor::Text),
    ] {
        if base_color_is_reachable(scheme, field) {
            out.push(preset(scheme, &base, bind));
        }
    }
    // 3. 按下态（方案给了才出现）。
    if scheme
        .state_styles
        .get("pressed")
        .and_then(|s| s.fill.as_deref())
        .and_then(Argb::parse_hex)
        .is_some()
    {
        out.push(preset(scheme, &base, Bind::PressFill));
    }
    if scheme
        .state_styles
        .get("pressed")
        .and_then(|s| s.text.as_deref())
        .and_then(Argb::parse_hex)
        .is_some()
    {
        out.push(preset(scheme, &base, Bind::PressText));
    }

    // 4. 命名样式：方案里每一个写出来的颜色/圆角都给一个控件 —— 这就是
    //    "方案给某些键上了别的颜色"在面板上的落点。
    let mut names: Vec<&String> = scheme.styles.keys().collect();
    names.sort();
    for n in names {
        let st = &scheme.styles[n];
        let tag = st.label.clone().unwrap_or_else(|| n.clone());
        if st.fill.is_some() {
            out.push(
                preset(scheme, &base, Bind::StyleFill(n.clone()))
                    .with_label(format!("{tag}_键帽色")),
            );
        }
        // ★ 样式级按下色：双色方案要"输入键蓝 / 功能键紫"两种按下色，全局那一个不够用。
        if st.press.is_some() {
            out.push(
                preset(scheme, &base, Bind::StylePress(n.clone()))
                    .with_label(format!("{tag}_按下色")),
            );
        }
        if st.text.is_some() {
            out.push(
                preset(scheme, &base, Bind::StyleText(n.clone()))
                    .with_label(format!("{tag}_文字色")),
            );
        }
        if st.border.is_some() {
            out.push(
                preset(scheme, &base, Bind::StyleBorder(n.clone()))
                    .with_label(format!("{tag}_描边色")),
            );
        }
        if st.radius.is_some() {
            // 命名与全局「圆角」同前缀，好让 `group_radii` 把它们排到一起。
            out.push(
                preset(scheme, &base, Bind::StyleRadius(n.clone()))
                    .with_label(format!("圆角_{tag}")),
            );
        }
        // 侧壁色/厚度：方案写了才出现（同"改了没反应的色块不给"的口径）。
        if st.side.is_some() {
            out.push(
                preset(scheme, &base, Bind::StyleSide(n.clone())).with_label(format!("{tag}_侧壁")),
            );
        }
        if st.depth.is_some() {
            out.push(
                preset(scheme, &base, Bind::StyleDepth(n.clone()))
                    .with_label(format!("侧壁厚度_{tag}")),
            );
        }
    }

    // 5. 方案自己声明的：同 bind 覆盖，其余追加。
    if let Some(list) = &scheme.declared {
        for c in list {
            let Some(bind) = Bind::parse(&c.bind) else {
                log::warn!(
                    "方案 {:?} 的控件 {:?} 绑了个认不出来的目标 {:?}，已跳过",
                    scheme.id,
                    c.id,
                    c.bind
                );
                continue;
            };
            let mut def = ControlDef::new(
                bind.clone(),
                if c.label.is_empty() {
                    bind.default_label()
                } else {
                    c.label.clone()
                },
                // 默认：作者写了 default 就用，否则照旧取"方案值 ?? 内置默认"。
                c.default
                    .clone()
                    .or_else(|| scheme_value(scheme, &bind))
                    .unwrap_or_else(|| preset_default(&base, &bind)),
            );
            if !c.id.is_empty() {
                def.id = c.id.clone();
            }
            // 类型/范围/归属：作者指定的优先。
            let is_color = match c.kind.as_str() {
                "color" => true,
                "bool" => {
                    def.kind = ControlKind::Bool;
                    false
                }
                "range" | "" => false,
                other => {
                    log::warn!(
                        "方案 {:?} 的控件 {:?} 类型 {other:?} 不认识，按滑块处理",
                        scheme.id,
                        c.id
                    );
                    false
                }
            };
            if is_color {
                def.kind = ControlKind::Color;
            } else if !matches!(def.kind, ControlKind::Bool) {
                let (min, max, step, dec) = bind.preset_range();
                let min = c.min.unwrap_or(min);
                let max = c.max.unwrap_or(max);
                let step = c.step.unwrap_or(step);
                let decimals = c.decimals.unwrap_or(dec);
                def.kind = ControlKind::Range {
                    min,
                    max,
                    step,
                    decimals,
                };
            }
            def.group = match c.group.as_str() {
                "color" => ControlGroup::Color,
                "look" => ControlGroup::Look,
                _ => {
                    if def.bind.is_color() {
                        ControlGroup::Color
                    } else {
                        ControlGroup::Look
                    }
                }
            };
            match out.iter_mut().find(|e| e.bind == bind) {
                Some(e) => *e = def,
                None => out.push(def),
            }
        }
    }
    group_radii(out)
}

/// ★ 把全部圆角类控件排到一起，**紧跟全局「圆角」之后**（2026-09-18 用户要求）。
///
/// # 为什么
///
/// 全局 `圆角`、`styles.<n>.radius`（`圆角_修饰键` 之类）、`keys.<n>.radius` 在功能上是
/// 同一个量、只是作用范围不同（整块 / 一类键 / 单颗键）。按原顺序它们散在各命名样式的
/// 颜色项后面，读起来像是"修饰键专有的一个属性"，用户得在两种量纲之间来回跳。
/// 归拢后排成一段：`圆角` → `圆角_修饰键` → `圆角_主键` → `圆角_功能键`（组内保持原相对
/// 顺序，即命名样式按 id 排序的结果）。
///
/// 锚点用全局 `圆角`（预设里恒有这一项），因此圆角组仍留在**数值组**开头、且紧跟着
/// `键帽大小 / 键间空隙` 这两项几何参数。
fn group_radii(out: Vec<ControlDef>) -> Vec<ControlDef> {
    let is_radius = |b: &Bind| {
        matches!(
            b,
            Bind::CornerRadius | Bind::StyleRadius(_) | Bind::KeyRadius(_)
        )
    };
    let mut kept: Vec<ControlDef> = Vec::with_capacity(out.len());
    let mut radii: Vec<ControlDef> = Vec::new();
    let mut at = None;
    for d in out {
        if is_radius(&d.bind) {
            if matches!(&d.bind, Bind::CornerRadius) {
                kept.push(d);
                at = Some(kept.len());
            } else {
                radii.push(d);
            }
        } else {
            kept.push(d);
        }
    }
    // 倒着插同一个下标 = 组内保持原顺序，且不用跟着挪下标。
    let at = at.unwrap_or(kept.len());
    for d in radii.into_iter().rev() {
        kept.insert(at, d);
    }
    kept
}

impl ControlDef {
    fn with_label(mut self, label: String) -> Self {
        self.label = label;
        self
    }
}

/// 预设项：默认值 = 方案里写的那个，没写就用内置默认。
fn preset(scheme: &Scheme, base: &VirtualKeyboardSettings, bind: Bind) -> ControlDef {
    let default = scheme_value(scheme, &bind).unwrap_or_else(|| preset_default(base, &bind));
    ControlDef::new(bind.clone(), bind.default_label(), default)
}

/// 内置默认值（面板滑块的出厂位置）。
fn preset_default(base: &VirtualKeyboardSettings, bind: &Bind) -> ParamValue {
    match bind {
        Bind::KeySize => ParamValue::Float(base.key_size as f32),
        Bind::Gap => ParamValue::Float(base.gap as f32),
        Bind::Opacity => ParamValue::Float(base.opacity),
        Bind::Hold => ParamValue::Float(base.hold),
        Bind::FontSize => ParamValue::Float(base.font_size as f32),
        Bind::CornerRadius | Bind::StyleRadius(_) | Bind::KeyRadius(_) => {
            ParamValue::Float(base.corner_radius as f32)
        }
        Bind::Scale => ParamValue::Float(base.scale),
        Bind::BaseFill => ParamValue::Color(hex_of(base.key_color)),
        Bind::BasePress => ParamValue::Color(hex_of(base.press_color)),
        Bind::BaseBorder => ParamValue::Color(hex_of(base.border_color)),
        Bind::BaseText => ParamValue::Color(hex_of(base.text_color)),
        // 兜底：没在方案里出现过的样式字段，跟兜底色走。
        Bind::PressFill => ParamValue::Color(hex_of(base.press_color)),
        Bind::PressText | Bind::StyleText(_) => ParamValue::Color(hex_of(base.text_color)),
        Bind::StyleFill(_) => ParamValue::Color(hex_of(base.key_color)),
        Bind::StylePress(_) => ParamValue::Color(hex_of(base.press_color)),
        Bind::StyleBorder(_) => ParamValue::Color(hex_of(base.border_color)),
        // 侧壁：默认透明（无侧壁），厚度默认 0 —— 不写就保持出厂观感。
        Bind::StyleSide(_) => ParamValue::Color("#00000000".into()),
        Bind::StyleDepth(_) => ParamValue::Float(0.0),
    }
}

/// 方案里为这个 bind 写的值。
fn scheme_value(scheme: &Scheme, bind: &Bind) -> Option<ParamValue> {
    let d = &scheme.defaults;
    match bind {
        Bind::KeySize => d.key_size.map(|v| ParamValue::Float(v as f32)),
        Bind::Gap => d.gap.map(|v| ParamValue::Float(v as f32)),
        Bind::Opacity => d.opacity.map(ParamValue::Float),
        Bind::Hold => d.hold.map(ParamValue::Float),
        Bind::FontSize => d.font_size.map(|v| ParamValue::Float(v as f32)),
        Bind::CornerRadius => d.corner_radius.map(ParamValue::Float),
        Bind::Scale => d.scale.map(ParamValue::Float),
        Bind::PressFill => scheme
            .state_styles
            .get("pressed")
            .and_then(|s| s.fill.as_deref())
            .map(|s| ParamValue::Color(s.to_string())),
        Bind::PressText => scheme
            .state_styles
            .get("pressed")
            .and_then(|s| s.text.as_deref())
            .map(|s| ParamValue::Color(s.to_string())),
        Bind::StyleFill(n) => scheme
            .styles
            .get(n)
            .and_then(|s| s.fill.as_deref())
            .map(|s| ParamValue::Color(s.to_string())),
        Bind::StyleText(n) => scheme
            .styles
            .get(n)
            .and_then(|s| s.text.as_deref())
            .map(|s| ParamValue::Color(s.to_string())),
        Bind::StyleBorder(n) => scheme
            .styles
            .get(n)
            .and_then(|s| s.border.as_deref())
            .map(|s| ParamValue::Color(s.to_string())),
        Bind::StylePress(n) => scheme
            .styles
            .get(n)
            .and_then(|s| s.press.as_deref())
            .map(|s| ParamValue::Color(s.to_string())),
        Bind::StyleRadius(n) => scheme
            .styles
            .get(n)
            .and_then(|s| s.radius)
            .map(ParamValue::Float),
        Bind::StyleSide(n) => scheme
            .styles
            .get(n)
            .and_then(|s| s.side.as_deref())
            .map(|s| ParamValue::Color(s.to_string())),
        Bind::StyleDepth(n) => scheme
            .styles
            .get(n)
            .and_then(|s| s.depth)
            .map(ParamValue::Float),
        Bind::KeyRadius(n) => scheme
            .keys
            .get(n)
            .and_then(|k| k.radius)
            .map(ParamValue::Float),
        // 兜底四色是**面板全局配置**，方案里没有对应的"默认值"这一说。
        Bind::BaseFill | Bind::BasePress | Bind::BaseBorder | Bind::BaseText => None,
    }
}

/// `[r,g,b,a]`（配置里的顺序）→ `"#RRGGBBAA"`。
///
/// ★★ alpha 在**末尾**，与 `config.json`、Lua 脚本、windui 取色器同口径。
fn hex_of(c: [u8; 4]) -> String {
    format!("#{:02X}{:02X}{:02X}{:02X}", c[0], c[1], c[2], c[3])
}

/// [`Argb`] → `"#RRGGBBAA"`（与 [`hex_of`] 同口径，只是入参是 u32）。
fn argb_hex(a: Argb) -> String {
    a.to_hex_rgba()
}

// ─── 解析后的方案 ───

/// 一份可用方案：几何 + 外观参数 + 控件清单。
#[derive(Debug, Clone)]
pub struct Scheme {
    /// 方案 id（= 文件名去掉 `.json` 扩展名，也是配置里落盘的值）。
    pub id: String,
    /// 显示名。
    pub name: String,
    /// 已构造好的几何。
    pub layout: Layout,
    pub defaults: VkDefaults,
    pub styles: HashMap<String, VkStyleDef>,
    pub state_styles: HashMap<String, VkStateStyle>,
    pub keys: HashMap<String, VkKeyOverride>,
    /// 面板控件清单（预设 + 命名样式 + 方案声明合并后的结果）。
    pub controls: Vec<ControlDef>,
    /// 方案 `controls` 块的原文（[`controls_of`] 的输入）。
    pub declared: Option<Vec<ControlJson>>,
}

/// 一个键在当前帧的视觉参数（合并了方案 + config + UI 面板的结果）。
#[derive(Debug, Clone, Copy)]
pub struct ResolvedStyle {
    pub fill: Argb,
    pub fill_pressed: Argb,
    pub text: Argb,
    pub border: Argb,
    pub radius: f32,
    /// **侧壁色**（alpha = 0 表示无侧壁）。
    pub side: Argb,
    /// 侧壁厚度（逻辑像素；0 = 无侧壁）。
    pub depth: f32,
}

// ─── 合并后的完整配置 ───

/// 虚拟键盘的运行时配置（方案 + config 合并后的产物）。
///
/// 生命周期：每帧从 `VirtualKeyboardSettings` 构建一次。方案本身按文件戳缓存
/// （见 [`FILE_CACHE`]），所以每帧的成本只是克隆一个 `Rc` + 建一张十来项的值表。
#[derive(Debug, Clone)]
pub struct VkConfig {
    scheme: Rc<Scheme>,
    /// 用户在面板上**改过**的值（`bind → 值`）。查不到就走兜底 —— 所以"没动过的项"
    /// 不进这张表，方案改了默认值用户也能立刻看到新观感。
    values: HashMap<Bind, ParamValue>,
    /// 面板全局配置的一份快照（兜底 + 老配置的迁移都靠它）。
    base: VkBase,
}

/// 面板全局配置里参与合并的那几个数（扁平 + `Copy`：每帧克隆一次也不肉疼）。
#[derive(Debug, Clone, Copy)]
struct VkBase {
    key_size: u32,
    gap: u32,
    opacity: f32,
    hold: f32,
    font_size: u32,
    corner_radius: u32,
    scale: f32,
    key_color: [u8; 4],
    press_color: [u8; 4],
    border_color: [u8; 4],
    text_color: [u8; 4],
}

impl VkConfig {
    /// 从 `VirtualKeyboardSettings` 构建。
    ///
    /// # 取值优先级（高 → 低）
    ///
    /// ```text
    /// 用户在面板控件上改过的值（config.params[方案 id][控件 id]）
    ///   ↑ 面板全局配置里那个字段（≠ 出厂默认时：老配置自动迁移过来）
    ///     ↑ 方案 JSON 里写的值（defaults / styles / stateStyles）
    ///       ↑ Rust 内置默认
    /// ```
    pub fn load(settings: &VirtualKeyboardSettings) -> Self {
        let scheme = resolve_scheme(&settings.layout);
        let base = VkBase {
            key_size: settings.key_size,
            gap: settings.gap,
            opacity: settings.opacity,
            hold: settings.hold,
            font_size: settings.font_size,
            corner_radius: settings.corner_radius,
            scale: settings.scale,
            key_color: settings.key_color,
            press_color: settings.press_color,
            border_color: settings.border_color,
            text_color: settings.text_color,
        };
        // 用户值按"控件 id → bind"落到表里：控件改名/改范围，老配置里的值照样认。
        //
        // ★ 超出控件量程的值**直接丢弃**（而不是钳到边界）：它多半是量纲搞错写进去的
        //   （2026-09-18 那次 0.55 的键帽大小），或者是手改 JSON 改坏了。丢掉让它退回
        //   方案默认，比显示一个"看起来生效其实不对"的值好 —— 也顺手自愈了存量配置。
        let mut values = HashMap::new();
        if let Some(saved) = settings.params.get(&scheme.id) {
            for c in &scheme.controls {
                if let Some(v) = saved.get(&c.id) {
                    if c.kind.accepts(v) {
                        values.insert(c.bind.clone(), v.clone());
                    } else {
                        log::warn!(
                            "虚拟键盘方案 {} 的控件 {} 存了量程外的值 {:?}，已忽略（回退方案默认）",
                            scheme.id,
                            c.id,
                            v
                        );
                    }
                }
            }
        }
        Self {
            scheme,
            values,
            base,
        }
    }

    /// 当前方案的几何。
    pub fn layout(&self) -> &Layout {
        &self.scheme.layout
    }

    /// 当前方案的显示名（日志用）。
    pub fn scheme_name(&self) -> &str {
        &self.scheme.name
    }

    /// 当前方案的 id（配置里 `params` 的键）。
    pub fn scheme_id(&self) -> &str {
        &self.scheme.id
    }

    /// 当前方案的控件清单（面板据此生成子卡：数值组 + 颜色组，见 [`ControlGroup`]）。
    pub fn controls(&self) -> &[ControlDef] {
        &self.scheme.controls
    }

    /// 某个控件**当前该显示的值**（面板建信号时的初值）。
    pub fn value_of(&self, c: &ControlDef) -> ParamValue {
        if let Some(v) = self.values.get(&c.bind) {
            return v.clone();
        }
        match c.kind {
            ControlKind::Color => ParamValue::Color(argb_hex(self.color(&c.bind))),
            ControlKind::Bool => ParamValue::Bool(
                self.values
                    .get(&c.bind)
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            ),
            ControlKind::Range { .. } => ParamValue::Float(self.num(&c.bind)),
        }
    }

    // ── 取值（用户值 → 兜底）──

    /// 只查用户改过的值。
    fn opt_f32(&self, b: &Bind) -> Option<f32> {
        self.values.get(b).and_then(|v| v.as_f32())
    }

    /// 只查用户改过的颜色。
    fn opt_color(&self, b: &Bind) -> Option<Argb> {
        self.values.get(b).and_then(|v| v.as_argb())
    }

    /// 数值类 bind 的最终值。
    pub fn num(&self, b: &Bind) -> f32 {
        if let Some(v) = self.opt_f32(b) {
            return v;
        }
        self.fallback_num(b)
    }

    /// 颜色类 bind 的最终值。
    pub fn color(&self, b: &Bind) -> Argb {
        if let Some(c) = self.opt_color(b) {
            return c;
        }
        self.fallback_color(b)
    }

    /// "面板全局配置 → 方案 → 内置默认"。
    fn fallback_num(&self, b: &Bind) -> f32 {
        let def = VirtualKeyboardSettings::default();
        // ★ 量程校验：落在量程外的**一律视同"没动过"**。
        //
        // 两个来源都可能越界：
        //   · 老配置 —— 出厂量程改过（键帽大小 12~96 → 10~80 后，老值 96 就出界了），
        //     沿用它等于让"用户没动过的项"卡在一个新版本根本给不出的值上；
        //   · 方案 JSON 写错 —— 退回内置默认，比照着一个错值渲染好。
        let (lo, hi, ..) = b.preset_range();
        let ok = |v: f32| v.is_finite() && v >= lo - 1e-3 && v <= hi + 1e-3;
        // 老配置迁移的判据与从前一致：面板字段 ≠ 出厂默认 ⇒ 用户动过（且要在量程内）。
        let u32_of = |cur: u32, dflt: u32, from_scheme: Option<u32>| -> f32 {
            if cur != dflt && ok(cur as f32) {
                cur as f32
            } else {
                from_scheme.filter(|v| ok(*v as f32)).unwrap_or(dflt) as f32
            }
        };
        let f32_of = |cur: f32, dflt: f32, from_scheme: Option<f32>| -> f32 {
            if (cur - dflt).abs() > f32::EPSILON && ok(cur) {
                cur
            } else {
                from_scheme.filter(|v| ok(*v)).unwrap_or(dflt)
            }
        };
        let d = &self.scheme.defaults;
        match b {
            Bind::KeySize => u32_of(self.base.key_size, def.key_size, d.key_size),
            Bind::Gap => u32_of(self.base.gap, def.gap, d.gap),
            Bind::Opacity => f32_of(self.base.opacity, def.opacity, d.opacity),
            Bind::Hold => f32_of(self.base.hold, def.hold, d.hold),
            Bind::FontSize => u32_of(self.base.font_size, def.font_size, d.font_size),
            Bind::CornerRadius => f32_of(
                self.base.corner_radius as f32,
                def.corner_radius as f32,
                d.corner_radius,
            ),
            Bind::Scale => f32_of(self.base.scale, def.scale, d.scale),
            // 样式/逐键的圆角：方案没给就跟全局圆角走。
            Bind::StyleRadius(n) => self
                .scheme
                .styles
                .get(n)
                .and_then(|s| s.radius)
                .unwrap_or(self.num(&Bind::CornerRadius)),
            Bind::KeyRadius(n) => self
                .scheme
                .keys
                .get(n)
                .and_then(|k| k.radius)
                .unwrap_or(self.num(&Bind::CornerRadius)),
            // 侧壁厚度：方案没给就 0（无侧壁）。
            Bind::StyleDepth(n) => self
                .scheme
                .styles
                .get(n)
                .and_then(|s| s.depth)
                .unwrap_or(0.0),
            // 颜色类走了这条路说明类型不对（用户塞了个数字）→ 给个中性值。
            _ => 0.0,
        }
    }

    fn fallback_color(&self, b: &Bind) -> Argb {
        let from = |c: [u8; 4]| Argb::new(c[3], c[0], c[1], c[2]);
        let base = VirtualKeyboardSettings::default();
        match b {
            Bind::BaseFill => from(self.base.key_color),
            Bind::BasePress => from(self.base.press_color),
            Bind::BaseBorder => from(self.base.border_color),
            Bind::BaseText => from(self.base.text_color),
            Bind::PressFill => self
                .scheme
                .state_styles
                .get("pressed")
                .and_then(|s| s.fill.as_deref())
                .and_then(Argb::parse_hex)
                .unwrap_or_else(|| from(self.base.press_color)),
            Bind::StyleFill(n) => self
                .scheme
                .styles
                .get(n)
                .and_then(|s| s.fill.as_deref())
                .and_then(Argb::parse_hex)
                .unwrap_or_else(|| from(base.key_color)),
            Bind::StyleText(n) => self
                .scheme
                .styles
                .get(n)
                .and_then(|s| s.text.as_deref())
                .and_then(Argb::parse_hex)
                .unwrap_or_else(|| from(base.text_color)),
            Bind::StyleBorder(n) => self
                .scheme
                .styles
                .get(n)
                .and_then(|s| s.border.as_deref())
                .and_then(Argb::parse_hex)
                .unwrap_or_else(|| from(base.border_color)),
            Bind::StylePress(n) => self
                .scheme
                .styles
                .get(n)
                .and_then(|s| s.press.as_deref())
                .and_then(Argb::parse_hex)
                .unwrap_or_else(|| from(base.press_color)),
            Bind::StyleSide(n) => self
                .scheme
                .styles
                .get(n)
                .and_then(|s| s.side.as_deref())
                .and_then(Argb::parse_hex)
                .unwrap_or_else(|| Argb::new(0, 0, 0, 0)),
            Bind::PressText => self
                .scheme
                .state_styles
                .get("pressed")
                .and_then(|s| s.text.as_deref())
                .and_then(Argb::parse_hex)
                .unwrap_or_else(|| from(base.text_color)),
            _ => from(base.key_color),
        }
    }

    // ── 渲染用的几个量 ──

    /// 一个标准键的边长（逻辑像素，已含缩放）。
    pub fn unit(&self) -> f32 {
        (self.num(&Bind::KeySize).max(8.0)) * self.num(&Bind::Scale).max(0.1)
    }
    /// 键间空隙（逻辑像素）。
    pub fn gap(&self) -> f32 {
        self.num(&Bind::Gap).max(0.0)
    }
    /// 总不透明度（容器级乘子）。
    pub fn opacity(&self) -> f32 {
        self.num(&Bind::Opacity).clamp(0.05, 1.0)
    }
    /// 高亮保持（秒）。`0` = 松开即灭。
    pub fn hold(&self) -> f64 {
        self.num(&Bind::Hold).max(0.0) as f64
    }
    /// 当前方案里有没有滚轮键帽（`WheelUp`/`WheelDown`）。
    ///
    /// 这是**滚轮门的武装判据**：`Runtime::sync_input_gate` 只有在它为真（或击键
    /// 回显要滚轮）时才让鼠标钩子转发普通滚轮 —— 否则"每滚一格唤醒一帧"的开销
    /// 白付，方案里又没有键帽等着亮。
    pub fn wants_wheel(&self) -> bool {
        self.scheme.layout.rows.iter().any(|row| {
            row.keys
                .iter()
                .any(|k| matches!(k.code, super::KeyCode::WheelUp | super::KeyCode::WheelDown))
        })
    }
    /// 当前方案里有没有**鼠标按键**键帽（左/右/中/侧键，不含滚轮）。
    ///
    /// ★★ 这是「**鼠标穿透**」与「**键帽高亮**」的解耦点（2026-09-26 修）。
    /// 旧实现把两件事焊在同一个开关上：`mouse_input_needed()` 只在
    /// `enabled && !click_through` 时为虚拟键盘开鼠标门，于是**勾着鼠标穿透的方案**
    /// （WASD 游戏板出厂就勾着）其鼠标事件被桥接线程整条丢弃 —— 键帽永远收不到
    /// `MouseDown`，表现就是"鼠标键点了没反应"，而键盘键一切正常。
    ///
    /// 可穿透管的是「点键帽要不要**注入**真实按键」（穿透 = 不注入、点击照常给下层
    /// 应用），**与"要不要知道鼠标被按了"完全无关** —— 后者是点亮键帽的唯一输入。
    /// 所以这里只看方案里有没有鼠标键帽：有，就要收按下/抬起。
    ///
    /// 与 [`Self::wants_wheel`] 同款：都是"按需武装"，没有鼠标键帽的方案（如 ANSI 104）
    /// 一个鼠标事件都不用付代价。
    pub fn wants_mouse(&self) -> bool {
        self.scheme.layout.rows.iter().any(|row| {
            row.keys.iter().any(|k| {
                matches!(
                    k.code,
                    super::KeyCode::MouseLeft
                        | super::KeyCode::MouseRight
                        | super::KeyCode::MouseMiddle
                        | super::KeyCode::MouseX1
                        | super::KeyCode::MouseX2
                )
            })
        })
    }
    /// 图例字号（逻辑像素）。
    pub fn font_size(&self) -> f32 {
        self.num(&Bind::FontSize).max(6.0)
    }
    /// 全局圆角。
    pub fn corner_radius(&self) -> f32 {
        self.num(&Bind::CornerRadius).max(0.0)
    }

    /// 解析一个键的视觉样式。
    ///
    /// `kd` = 几何里的键定义（自带可选的 `style` / `radius` 覆盖），`pressed` = 是否正在按下。
    ///
    /// # 每一层的取值顺序（高 → 低）
    ///
    /// ```text
    /// 用户在面板控件上改过的值（config.params → `values`）
    ///   ↑ 方案里写的：keys[code] > 几何逐键 > 命名样式
    ///     ↑ 面板全局配置的四个兜底色（键帽/按下/描边/文字）
    /// ```
    ///
    /// **按下色**多一层：样式级 `press` 压过全局 `stateStyles.pressed.fill`
    /// （双色方案要"输入键蓝 / 功能键紫"两种按下色，全局那一个不够用）。
    /// ★ 兜底色**从 `self` 取**，不再由调用方传进来：四个兜底色本身也是控件
    /// （`base.fill` 等），传参等于把"可被控件改的东西"绕开控件。
    pub fn resolve_style(&self, kd: &KeyDef, pressed: bool) -> ResolvedStyle {
        let s = &*self.scheme;
        let key_name = layout::code_to_name(kd.code);
        let ov = s.keys.get(key_name);

        // 1. 样式名：keys 表的覆盖 > 几何里写的
        let style_name = ov
            .and_then(|o| o.style.as_deref())
            .or(kd.style.as_deref())
            .map(str::to_string);
        let named = style_name.as_deref().and_then(|n| s.styles.get(n));

        // 2. 四个兜底色（面板全局配置 + 可被控件改）
        let base_fill = self.color(&Bind::BaseFill);
        let base_press = self.color(&Bind::BasePress);
        let base_border = self.color(&Bind::BaseBorder);
        let base_text = self.color(&Bind::BaseText);

        // 3. 填充色 / 文字色 / 描边色：用户值 > 命名样式 > 兜底
        let fill = style_name
            .as_deref()
            .and_then(|n| self.opt_color(&Bind::StyleFill(n.to_string())))
            .or_else(|| {
                named
                    .and_then(|st| st.fill.as_deref())
                    .and_then(Argb::parse_hex)
            })
            .unwrap_or(base_fill);

        // 按下色：★ 双色方案要两种，所以**样式级 press 优先于全局按下态**
        // （样式级 → 全局 stateStyles.pressed → 兜底 `base.press`）。
        let fill_pressed = style_name
            .as_deref()
            .and_then(|n| self.opt_color(&Bind::StylePress(n.to_string())))
            .or_else(|| {
                named
                    .and_then(|st| st.press.as_deref())
                    .and_then(Argb::parse_hex)
            })
            .or_else(|| {
                self.opt_color(&Bind::PressFill).or_else(|| {
                    s.state_styles
                        .get("pressed")
                        .and_then(|st| st.fill.as_deref())
                        .and_then(Argb::parse_hex)
                })
            })
            .unwrap_or(base_press);

        let text = style_name
            .as_deref()
            .and_then(|n| self.opt_color(&Bind::StyleText(n.to_string())))
            .or_else(|| {
                named
                    .and_then(|st| st.text.as_deref())
                    .and_then(Argb::parse_hex)
            })
            .unwrap_or(base_text);

        // 按下态可以单独指定文字色（深色底配浅字、浅色底配深字）。
        let text_pressed = self.opt_color(&Bind::PressText).or_else(|| {
            s.state_styles
                .get("pressed")
                .and_then(|st| st.text.as_deref())
                .and_then(Argb::parse_hex)
        });

        let border = style_name
            .as_deref()
            .and_then(|n| self.opt_color(&Bind::StyleBorder(n.to_string())))
            .or_else(|| {
                named
                    .and_then(|st| st.border.as_deref())
                    .and_then(Argb::parse_hex)
            })
            .unwrap_or(base_border);

        // 4. 圆角：逐键(用户) > 逐键(方案) > 几何逐键 > 样式(用户) > 样式(方案) > 全局
        let radius = self
            .opt_f32(&Bind::KeyRadius(key_name.to_string()))
            .or_else(|| ov.and_then(|o| o.radius))
            .or(kd.radius)
            .or_else(|| {
                style_name
                    .as_deref()
                    .and_then(|n| self.opt_f32(&Bind::StyleRadius(n.to_string())))
            })
            .or_else(|| named.and_then(|st| st.radius))
            .unwrap_or_else(|| self.corner_radius());

        // 5. 侧壁：颜色与厚度都只认样式级（用户改过的值优先）。
        //    厚度 0 或侧壁 alpha = 0 都等于"无侧壁"——渲染层以此短路。
        let side = style_name
            .as_deref()
            .and_then(|n| self.opt_color(&Bind::StyleSide(n.to_string())))
            .or_else(|| {
                named
                    .and_then(|st| st.side.as_deref())
                    .and_then(Argb::parse_hex)
            })
            .unwrap_or_else(|| Argb::new(0, 0, 0, 0));
        let depth = style_name
            .as_deref()
            .and_then(|n| self.opt_f32(&Bind::StyleDepth(n.to_string())))
            .or(named.and_then(|st| st.depth))
            .unwrap_or(0.0)
            .clamp(0.0, 12.0);

        ResolvedStyle {
            fill: if pressed { fill_pressed } else { fill },
            fill_pressed,
            text: if pressed {
                text_pressed.unwrap_or(text)
            } else {
                text
            },
            border,
            radius,
            side,
            depth,
        }
    }

    /// 解析一个键的显示文字。
    ///
    /// ★ **图例的唯一真相来源是几何表**（[`KeyDef`] 的 `base` / `shift`）：
    /// 本函数只负责"选哪一层"和"方案 `keys` 表有没有覆盖它"。这里曾经另有一张
    /// `shift_char` 数字行映射表，于是几何里写好的 `shift` 层从来没被画出来过 ——
    /// 同一个问题有两个答案，改一处不动另一处。
    ///
    /// ★ 也没有「随 Shift 切换」开关了：键帽显示上档是键盘的固有语义（输入法一致），
    /// 开关只会让"没效果"看起来像缺陷。上档文字来自几何的 `shiftText` /
    /// `keys[code].shiftText`。
    pub fn resolve_label(&self, kd: &KeyDef, shift: bool, caps: bool) -> String {
        // 1. 方案 keys 表里的自定义文字（最高优先级）
        if let Some(ov) = self.scheme.keys.get(layout::code_to_name(kd.code)) {
            if shift {
                if let Some(st) = &ov.shift_text {
                    return st.clone();
                }
            }
            if let Some(t) = &ov.text {
                return t.clone();
            }
        }

        // 2. 几何表的两层图例
        let label = layout::legend(kd, shift, caps);
        if !label.is_empty() {
            return label.to_string();
        }

        // 3. 几何里刻意留白的键（只有 Space）→ 走默认表，结果仍是空串。
        super::defaults::default_label(kd.code).to_string()
    }
}

/// 当前目录下所有可用方案（下拉数据源）。
pub fn scheme_list() -> Vec<layout::SchemeEntry> {
    layout::discover(&crate::core::paths::asset_dir("VirtualKeyboards"))
}

// ─── 方案加载（带进程内缓存）───
//
// ★ 为什么需要缓存：`VkConfig::load` 是**每帧**调用的（`vk/mod.rs::draw`），而键盘
//   高亮期间覆盖层会持续出帧 —— 每帧一次文件读 + 一次 `serde_json` 解析 + 一次几何
//   构造（104 个键的 Vec + String）。原注释写"构建成本很低，可以每帧重建"，那是把
//   "合并几个字段"当成全部成本，漏掉了 I/O 与分配。
//
// 失效判据 = 路径变了 **或** 文件的 (mtime, 长度) 变了：用户改完 `keyboard.json`
// 下一帧就能看到，不必重启。
//
// 只在 UI/渲染线程上访问 → thread_local 即可，不需要锁。

/// 文件方案缓存条目：`(mtime, size, 解析结果)`——mtime/size 都没变就直接复用。
type SchemeCacheEntry = (u64, u64, Option<Rc<Scheme>>);

thread_local! {
    static FILE_CACHE: RefCell<HashMap<PathBuf, SchemeCacheEntry>> =
        RefCell::new(HashMap::new());
    static BUILTIN_CACHE: RefCell<HashMap<&'static str, Rc<Scheme>>> =
        RefCell::new(HashMap::new());
}

/// 按方案 id 取方案：**目录里那份优先**，没有（或坏了）才用内嵌的出厂方案。
///
/// 任何情况下都返回一个可画的方案（最差是一个空键盘），不 panic：键盘整个消失
/// 比"少一套方案"严重得多。
/// 取 `base_layout` 指向的那套几何。
///
/// ★ 目录里的那份**优先于**内嵌那份：用户改了 `ansi_104.json` 的几何，
///   引用它的"纯样式方案"应当跟着变 —— 否则"根目录即来源"在方案之间不成立。
///
/// 只看一层（直接解析对方自己的 `rows`，不跟着它的 `base_layout` 再往下走）：
/// 方案之间互相引用就会成环，而"纯样式套纯样式"没有实际用途，不值得为它引入
/// 深度计数与环检测。对方没写 `rows` 时自然落到内嵌那份。
fn base_layout_of(base_id: &str) -> Option<Layout> {
    if base_id.is_empty() {
        return None;
    }
    let file = crate::core::paths::asset_dir("VirtualKeyboards").join(format!("{base_id}.json"));
    if file.is_file() {
        if let Ok(text) = fs::read_to_string(&file) {
            if let Ok(j) = serde_json::from_str::<LayoutJson>(&text) {
                let label = j.name.as_deref().unwrap_or(base_id);
                if let Ok(l) = layout::layout_from_json(base_id, label, &j) {
                    return Some(l);
                }
            }
        }
    }
    layout::builtin_layout(base_id)
}

fn resolve_scheme(id: &str) -> Rc<Scheme> {
    let base = crate::core::paths::asset_dir("VirtualKeyboards");
    // 测试用：id 直接是目录路径时，从 `<id>/keyboard.json` 取（生产传的是裸名，不会命中）。
    let dir_file = base.join(id).join("keyboard.json");
    if dir_file.is_file() {
        if let Some(s) = load_file_scheme(id, &dir_file) {
            return s;
        }
    }
    // ★ 方案就是**顶层的一个 JSON 文件**（文件名即 id），不再套子目录。
    let file = base.join(format!("{id}.json"));
    if file.is_file() {
        if let Some(s) = load_file_scheme(id, &file) {
            return s;
        }
    }
    if let Some(s) = builtin_scheme(id) {
        return s;
    }
    // 用户选的方案文件中途被删 / 改坏：退回出厂第一套，并在日志里留痕。
    if let Some(s) = builtin_scheme("ansi_104") {
        log::warn!("方案 {id:?} 读取失败，回落到出厂方案");
        return s;
    }
    log::error!("出厂方案也无法解析（assets 与内嵌都坏了）");
    let mut sc = Scheme {
        id: id.to_string(),
        name: id.to_string(),
        layout: Layout {
            id: id.to_string(),
            label: id.to_string(),
            rows: Vec::new(),
        },
        defaults: VkDefaults::default(),
        styles: HashMap::new(),
        state_styles: HashMap::new(),
        keys: HashMap::new(),
        controls: Vec::new(),
        declared: None,
    };
    sc.controls = controls_of(&sc);
    Rc::new(sc)
}

/// 从文件加载一份方案（带文件戳缓存）。
fn load_file_scheme(id: &str, path: &PathBuf) -> Option<Rc<Scheme>> {
    let stamp = file_stamp(path);
    let hit: Option<Option<Rc<Scheme>>> = {
        let c = FILE_CACHE.with(|c| {
            let b = c.borrow();
            match b.get(path) {
                Some((m, l, s)) if Some((*m, *l)) == stamp => Some(s.clone()),
                _ => None,
            }
        });
        c
    };
    if let Some(s) = hit {
        return s;
    }

    let scheme = build_scheme(id, path);
    let (m, l) = stamp.unwrap_or((0, 0));
    FILE_CACHE.with(|c| {
        c.borrow_mut().insert(path.clone(), (m, l, scheme.clone()));
    });
    scheme
}

/// 读盘 + 解析 + 定几何。失败返回 `None`（调用方回落内嵌）。
fn build_scheme(id: &str, path: &PathBuf) -> Option<Rc<Scheme>> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            log::warn!("方案 {} 读取失败：{e}", path.display());
            return None;
        }
    };
    let json: VkJson = match serde_json::from_str(&text) {
        Ok(j) => j,
        Err(e) => {
            log::warn!("方案 {} 解析失败：{e}", path.display());
            return None;
        }
    };
    let name = json
        .geom
        .name
        .clone()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| id.to_string());

    // 几何来源三级：自己的 rows → baseLayout 引用的出厂几何 → 出厂 104。
    let layout = match layout::layout_from_json(id, &name, &json.geom) {
        Ok(l) => l,
        Err(why) => {
            let base_id = json.geom.base_layout.as_deref().unwrap_or("");
            match base_layout_of(base_id) {
                Some(mut l) => {
                    l.id = id.to_string();
                    l.label = name.clone();
                    l
                }
                None => {
                    // 明确说了用哪套出厂几何却找不到，或键码写错 —— 两种都要说话。
                    if json.geom.rows.is_empty() {
                        log::warn!("方案 {name:?} 没写 rows，也没给出可用的 baseLayout：{why}");
                    } else {
                        log::warn!("方案 {name:?} 的 rows 解析失败：{why}");
                    }
                    return None;
                }
            }
        }
    };

    let mut sc = Scheme {
        id: id.to_string(),
        name,
        layout,
        defaults: json.defaults,
        styles: json.styles,
        state_styles: json.state_styles,
        keys: json.keys,
        controls: Vec::new(),
        declared: json.controls,
    };
    sc.controls = controls_of(&sc);
    Some(Rc::new(sc))
}

/// 内嵌的出厂方案（解析一次后缓存）。
fn builtin_scheme(id: &str) -> Option<Rc<Scheme>> {
    let cached = BUILTIN_CACHE.with(|c| c.borrow().get(id).cloned());
    if let Some(s) = cached {
        return Some(s);
    }
    let text = layout::builtin_json(id)?;
    // 内嵌 JSON 是我们自己生成的，解析失败属于打包事故——但仍以 Err 处理，
    // 不 panic（运行期宁可少一套方案）。
    let json: VkJson = match serde_json::from_str(text) {
        Ok(j) => j,
        Err(e) => {
            log::error!("内嵌方案 {id} 解析失败（打包损坏）：{e}");
            return None;
        }
    };
    let name = json.geom.name.clone().unwrap_or_else(|| id.to_string());
    let layout = match layout::layout_from_json(id, &name, &json.geom) {
        Ok(l) => l,
        Err(e) => {
            log::error!("内嵌方案 {id} 的几何构造失败（打包损坏）：{e}");
            return None;
        }
    };
    let mut sc = Scheme {
        id: id.to_string(),
        name,
        layout,
        defaults: json.defaults,
        styles: json.styles,
        state_styles: json.state_styles,
        keys: json.keys,
        controls: Vec::new(),
        declared: json.controls,
    };
    sc.controls = controls_of(&sc);
    let s = Rc::new(sc);
    BUILTIN_CACHE.with(|c| {
        c.borrow_mut().insert(
            layout::BUILTIN
                .iter()
                .find(|(k, _)| *k == id)
                .map(|(k, _)| *k)
                .unwrap_or("ansi_104"),
            s.clone(),
        );
    });
    Some(s)
}

/// 文件的 `(mtime 纳秒, 字节数)`。取不到时间戳时 mtime 记 0（长度仍参与比较）。
fn file_stamp(path: &PathBuf) -> Option<(u64, u64)> {
    let md = fs::metadata(path).ok()?;
    let m = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    Some((m, md.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::KeyCode;

    fn settings_for(id: &str) -> VirtualKeyboardSettings {
        VirtualKeyboardSettings {
            layout: id.to_string(),
            ..Default::default()
        }
    }

    /// 写一份临时方案 JSON，返回其**目录名**（相对 `assets/VirtualKeyboards` 的绝对路径）。
    fn write_tmp_dir(name: &str, body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("建临时方案目录失败");
        fs::write(dir.join("keyboard.json"), body).expect("写临时方案失败");
        dir
    }

    #[test]
    fn builtin_scheme_loads_with_geometry() {
        let cfg = VkConfig::load(&settings_for("ansi_104"));
        assert_eq!(cfg.layout().rows.len(), 6);
        let keys: usize = cfg.layout().rows.iter().map(|r| r.keys.len()).sum();
        assert_eq!(keys, 104);
        assert_eq!(cfg.scheme_name(), "ANSI 104");
    }

    /// ★★ 鼠标门的武装判据：**方案里有鼠标键帽才收鼠标事件，与「鼠标穿透」无关**。
    ///
    /// 回归锚点（2026-09-26 用户报"左键和右键点了没反应"）：`mouse_input_needed()`
    /// 只在 `enabled && !click_through` 时为虚拟键盘开鼠标门，而 WASD 游戏板**出厂就
    /// 勾着穿透** —— 桥接线程于是把它的 `MouseDown` 整条丢掉，鼠标键帽永远收不到事件。
    /// 穿透管的是"点键帽注不注入按键"，跟"键帽要不要亮"是两件事，这里钉死这个区分。
    #[test]
    fn wants_mouse_follows_the_scheme_not_the_passthrough_flag() {
        // 游戏板有鼠标键帽 ⇒ 要收鼠标事件（**不管**穿透勾没勾）。
        let game = VkConfig::load(&settings_for("wasd_gaming"));
        assert!(game.wants_mouse(), "WASD 板含左/右键，必须收鼠标事件");

        // 穿透只是另一个开关，不影响"要不要收"：这里直接翻它验判据不读它。
        let mut s = settings_for("wasd_gaming");
        s.click_through = true;
        assert!(VkConfig::load(&s).wants_mouse(), "穿透不该影响 wants_mouse");
        s.click_through = false;
        assert!(VkConfig::load(&s).wants_mouse());

        // 纯键盘方案没有鼠标键帽 ⇒ 一个鼠标事件都不用付代价。
        for id in ["ansi_104", "ansi_60", "ansi_104_dark"] {
            assert!(
                !VkConfig::load(&settings_for(id)).wants_mouse(),
                "{id} 是纯键盘方案，不该为它开鼠标门"
            );
        }

        // 滚轮不算"鼠标按键"：它走 `wants_wheel` 那道门，两条门互不混淆。
        let wheel = VkConfig::load(&settings_for("wasd_gaming"));
        assert!(!wheel.wants_wheel(), "WASD 板没有滚轮键帽");
    }

    /// 未知方案 id 要回落到出厂方案，而不是画不出键盘。
    #[test]
    fn unknown_scheme_falls_back_to_builtin() {
        let cfg = VkConfig::load(&settings_for("这个方案不存在"));
        assert_eq!(cfg.layout().rows.len(), 6, "应回落到出厂 104");
    }

    /// ★ 纯样式方案：不写 `rows`，几何从 `base_layout` 指向的方案借。
    ///
    /// 出厂就带一份活样例（`assets/VirtualKeyboards/default/keyboard.json`）。这条通路
    /// 坏掉的症状很隐蔽 —— `rows` 空、`base_layout` 又没解析出来时，`resolve_scheme`
    /// 会**静默回落**到出厂 104，键盘照常画出来，只是用户写的配色与逐键文字全丢了。
    /// 所以这里不只看键数，还要看方案的 `keys` 覆盖有没有真的落到键帽上。
    #[test]
    fn style_only_scheme_borrows_geometry_from_base_layout() {
        let cfg = VkConfig::load(&settings_for("ansi_104_dark"));
        assert_eq!(cfg.layout().rows.len(), 6, "应借到 ANSI 104 的 6 行");
        let keys: usize = cfg.layout().rows.iter().map(|r| r.keys.len()).sum();
        assert_eq!(keys, 104, "应借到全部 104 颗键");

        let find = |code: KeyCode| {
            cfg.layout()
                .rows
                .iter()
                .flat_map(|r| r.keys.iter())
                .find(|k| k.code == code)
                .expect("该键应存在")
        };
        // 方案里的 `keys` 覆盖真的生效了 —— 这是"没静默回落"的判据。
        // （原来用"Backspace 被改名成 Back"来验；2026-09-26 双色方案改成只带样式、不带
        //  文字覆盖，所以判据换成"Backspace 归到 func 样式、底色是功能键色"。）
        assert_eq!(
            cfg.resolve_style(find(KeyCode::Backspace), false)
                .fill
                .to_hex_rgba(),
            "#004040BF",
            "Backspace 应走功能键样式，说明这份 JSON 确实被读到了"
        );
        // 没被覆盖的键仍走几何自带的图例。
        assert_eq!(
            cfg.resolve_label(find(KeyCode::Char('q')), false, false),
            "q"
        );
        assert_eq!(
            cfg.resolve_label(find(KeyCode::Char('q')), true, false),
            "Q"
        );
    }

    /// ★ 双色方案：104 颗键**每颗都要归类**，且归类必须合用户定的那条规则。
    ///
    /// 规则（2026-09-26 用户定，同日二次修订）：**输入键** = 数字 0-9（含小键盘）、
    /// 26 个字母、所有标点、小键盘运算符 `/ * - + .`（用户："这些本质上都是输入键"）；
    /// **功能键** = 其余全部（Esc/F 行、编辑键、修饰键、空格、导航区、NumLock、
    /// 小键盘 Enter）。
    ///
    /// ★ 判据里有"没有一颗键漏归类"：漏一颗它就掉回兜底色（黑），肉眼看只是"某个键颜色
    /// 不对"，很难发现 —— 而且正是 104 键逐行抄表最容易出的错。
    #[test]
    fn duo_scheme_classifies_all_104_keys_by_the_input_rule() {
        let cfg = VkConfig::load(&settings_for("ansi_104_dark"));
        let keys: Vec<&KeyDef> = cfg
            .layout()
            .rows
            .iter()
            .flat_map(|r| r.keys.iter())
            .collect();
        assert_eq!(keys.len(), 104);

        let input_hex = "#000000BF";
        let func_hex = "#004040BF";
        let mut n_input = 0usize;
        let mut n_func = 0usize;
        for k in &keys {
            let hex = cfg.resolve_style(k, false).fill.to_hex_rgba();
            match hex.as_str() {
                _ if hex == input_hex => n_input += 1,
                _ if hex == func_hex => n_func += 1,
                other => panic!(
                    "{:?}（图例 {:?}）没归到任何一类，底色是 {other} —— keys 表漏了这一颗",
                    k.code, k.base
                ),
            }
        }
        // 输入键：主区 47（13 个数字/符号 + 26 字母 + 8 个标点）+ 小键盘 0-9 共 10
        // + 小键盘运算符 5（`/` `*` `-` `+` `.`，2026-09-26 改归输入键）。
        assert_eq!(n_input, 62, "输入键应有 62 颗，实际 {n_input}");
        assert_eq!(n_func, 42, "功能键应有 42 颗，实际 {n_func}");

        let style_of = |code: KeyCode| {
            let k = keys.iter().find(|k| k.code == code).unwrap_or_else(|| {
                panic!("布局里应有 {code:?}");
            });
            cfg.resolve_style(k, false).fill.to_hex_rgba()
        };
        // 输入键侧
        for c in ['q', 'a', 'z', 'm'] {
            assert_eq!(style_of(KeyCode::Char(c)), input_hex, "{c} 应是输入键");
        }
        for c in ['1', '0', '5'] {
            assert_eq!(style_of(KeyCode::Char(c)), input_hex, "{c} 应是输入键");
        }
        for c in ['`', '-', '=', '[', ']', '\\', ';', '\'', ',', '.', '/'] {
            assert_eq!(
                style_of(KeyCode::Char(c)),
                input_hex,
                "{c} 是标点，应算输入键"
            );
        }
        for n in 0..=9u8 {
            assert_eq!(style_of(KeyCode::Num(n)), input_hex, "小键盘 {n} 算输入键");
        }
        // 小键盘运算符也是输入键（2026-09-26 用户二次定）。
        for c in [
            KeyCode::NumDiv,
            KeyCode::NumMul,
            KeyCode::NumSub,
            KeyCode::NumAdd,
            KeyCode::NumDot,
        ] {
            assert_eq!(style_of(c), input_hex, "{c:?} 是输入键");
        }
        // 功能键侧
        for c in [
            KeyCode::Esc,
            KeyCode::F(1),
            KeyCode::F(12),
            KeyCode::PrtSc,
            KeyCode::Backspace,
            KeyCode::Tab,
            KeyCode::CapsLock,
            KeyCode::Enter,
            KeyCode::LShift,
            KeyCode::RShift,
            KeyCode::LCtrl,
            KeyCode::RCtrl,
            KeyCode::LAlt,
            KeyCode::RAlt,
            KeyCode::LWin,
            KeyCode::RWin,
            KeyCode::Menu,
            KeyCode::Space,
            KeyCode::Insert,
            KeyCode::Delete,
            KeyCode::Home,
            KeyCode::End,
            KeyCode::PageUp,
            KeyCode::PageDown,
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::NumLock,
            // 小键盘运算符（NumDiv/NumMul/NumSub/NumAdd/NumDot）2026-09-26 起归输入键。
            KeyCode::NumEnter,
        ] {
            assert_eq!(style_of(c), func_hex, "{c:?} 应是功能键");
        }
    }

    /// ★ 按下色是**逐样式**的：双色方案要"输入键蓝 / 功能键紫"两种。
    ///
    /// 在此之前 `stateStyles.pressed` 是全局唯一一个，两套键只能共用一种按下色 ——
    /// 这就是这条能力存在的理由。
    #[test]
    fn press_color_is_per_style_not_global() {
        let cfg = VkConfig::load(&settings_for("ansi_104_dark"));
        let find = |code: KeyCode| {
            cfg.layout()
                .rows
                .iter()
                .flat_map(|r| r.keys.iter())
                .find(|k| k.code == code)
                .expect("该键应存在")
                .clone()
        };
        assert_eq!(
            cfg.resolve_style(&find(KeyCode::Char('q')), true)
                .fill
                .to_hex_rgba(),
            "#0000FFBF",
            "输入键按下色"
        );
        assert_eq!(
            cfg.resolve_style(&find(KeyCode::Space), true)
                .fill
                .to_hex_rgba(),
            "#700070BF",
            "功能键按下色"
        );
        // 没按下的两种底色也要对（按下色不能把常态顶掉）。
        assert_eq!(
            cfg.resolve_style(&find(KeyCode::Char('q')), false)
                .fill
                .to_hex_rgba(),
            "#000000BF"
        );
        assert_eq!(
            cfg.resolve_style(&find(KeyCode::Space), false)
                .fill
                .to_hex_rgba(),
            "#004040BF"
        );
    }

    /// ★ 面板不给"改了没反应"的色块：双色方案两套样式把 fill/press/text/border 写齐、
    ///   104 颗键全归了类，于是四个兜底色（键帽/按下/描边/文字）谁也用不到 —— 该藏起来
    ///   （2026-09-26 用户报告：面板上还挂着旧的「键帽色/描边色/文字色」）；两颗
    ///   `样式_按下` 则必须出现。
    #[test]
    fn dead_base_color_controls_are_hidden_when_every_key_has_a_style() {
        let cfg = VkConfig::load(&settings_for("ansi_104_dark"));
        let ctls = controls_of(&cfg.scheme);
        let has = |b: &Bind| ctls.iter().any(|c| &c.bind == b);
        for (b, name) in [
            (&Bind::BasePress, "按下色"),
            (&Bind::BaseFill, "键帽色"),
            (&Bind::BaseBorder, "描边色"),
            (&Bind::BaseText, "文字色"),
        ] {
            assert!(!has(b), "没有键会落到兜底{name}，这个控件不该出现");
        }
        assert!(has(&Bind::StylePress("input".into())), "输入键_按下 应出现");
        assert!(has(&Bind::StylePress("func".into())), "功能键_按下 应出现");

        // 对照：原版 104 一颗键都没有样式，四个兜底色必须全在。
        let plain = VkConfig::load(&settings_for("ansi_104"));
        let ctls = controls_of(&plain.scheme);
        for b in [
            &Bind::BasePress,
            &Bind::BaseFill,
            &Bind::BaseBorder,
            &Bind::BaseText,
        ] {
            assert!(
                ctls.iter().any(|c| &c.bind == b),
                "没有样式的方案仍要能改兜底色 {b:?}"
            );
        }
    }

    /// ★ 圆角出厂值随方案 JSON 走（2026-09-26 用户定）：双色 20、ANSI 60 是 8。
    #[test]
    fn scheme_corner_radius_defaults_follow_the_json() {
        for (id, want) in [
            ("ansi_104_dark", 20.0),
            ("ansi_60", 8.0),
            ("wasd_gaming", 8.0),
        ] {
            let cfg = VkConfig::load(&settings_for(id));
            let k = cfg
                .layout()
                .rows
                .iter()
                .flat_map(|r| r.keys.iter())
                .next()
                .expect("应有键");
            assert_eq!(
                cfg.resolve_style(k, false).radius,
                want,
                "{id} 的出厂圆角应为 {want}"
            );
        }
    }

    /// ★ 游戏 WASD 板：没写任何样式/配色 → 四个兜底色全可达（与 104 原版同源），
    ///   面板上不该出现任何"样式·xx"色块；鼠标键码能解析。
    #[test]
    fn gaming_scheme_uses_fallback_colors_and_no_named_styles() {
        assert_eq!(
            layout::parse_code("MouseLeft"),
            Some(KeyCode::MouseLeft),
            "鼠标键码要能从 JSON 解析"
        );
        assert_eq!(layout::parse_code("RMB"), Some(KeyCode::MouseRight));
        // 滚轮门：游戏板没有滚轮键帽 → 不武装滚轮转发；含滚轮键帽的方案要武装。
        assert!(
            !VkConfig::load(&settings_for("wasd_gaming")).wants_wheel(),
            "游戏板没有滚轮键帽，不该武装滚轮门"
        );

        let cfg = VkConfig::load(&settings_for("wasd_gaming"));
        let ctls = controls_of(&cfg.scheme);
        for b in [
            &Bind::BaseFill,
            &Bind::BasePress,
            &Bind::BaseBorder,
            &Bind::BaseText,
        ] {
            assert!(
                ctls.iter().any(|c| &c.bind == b),
                "没写样式的方案必须能改兜底色 {b:?}"
            );
        }
        assert!(
            !ctls.iter().any(|c| matches!(c.bind, Bind::StyleFill(_))),
            "游戏板没有命名样式，不该长出样式色块"
        );
    }

    /// ★ 图例不再有开关：按 Shift 恒切上档；方案可用 `shift_text` 改名。
    #[test]
    fn legend_follows_shift_and_scheme_can_rename_the_upper_layer() {
        let cfg = VkConfig::load(&settings_for("ansi_104"));
        let q = cfg
            .layout()
            .rows
            .iter()
            .flat_map(|r| r.keys.iter())
            .find(|k| k.base == "q")
            .expect("应有 q 键");
        assert_eq!(cfg.resolve_label(q, false, false), "q", "不按 Shift 印下档");
        assert_eq!(cfg.resolve_label(q, true, false), "Q", "按 Shift 印上档");
        // Caps 与 Shift 异或：都开反而回到下档。
        assert_eq!(cfg.resolve_label(q, true, true), "q");

        let one = cfg
            .layout()
            .rows
            .iter()
            .flat_map(|r| r.keys.iter())
            .find(|k| k.base == "1")
            .expect("应有 1 键");
        assert_eq!(cfg.resolve_label(one, false, false), "1");
        assert_eq!(cfg.resolve_label(one, true, false), "!");
    }

    /// 方案里改上档文字（两条路：几何 `shiftText` / `keys` 表）。
    #[test]
    fn scheme_shift_text_overrides_both_ways() {
        let dir = write_tmp_dir(
            "curseen_vk_shiftname",
            r#"{
                "name":"T",
                "baseLayout":"ansi_104",
                "keys": { "CharQ": { "shiftText": "SHIFT-Q" } }
            }"#,
        );
        let cfg = VkConfig::load(&settings_for(&dir.to_string_lossy()));
        let q = cfg
            .layout()
            .rows
            .iter()
            .flat_map(|r| r.keys.iter())
            .find(|k| k.base == "q")
            .expect("应有 q 键");
        assert_eq!(cfg.resolve_label(q, true, false), "SHIFT-Q");
        assert_eq!(cfg.resolve_label(q, false, false), "q", "下档不受影响");
        let _ = fs::remove_dir_all(&dir);
    }

    /// ★ 优先级：用户在面板上改过的 > 老配置里动过的字段 > 方案 defaults > 内置默认。
    #[test]
    fn panel_value_wins_over_scheme_defaults() {
        let dir = write_tmp_dir(
            "curseen_vk_priority",
            r#"{"name":"T","baseLayout":"ansi_104",
                "defaults":{"font_size":20,"corner_radius":12,"opacity":0.5}}"#,
        );
        let mut settings = settings_for(&dir.to_string_lossy());

        // 用户没动过（params 空、字段仍是出厂值）→ 让方案兜底。
        let cfg = VkConfig::load(&settings);
        assert_eq!(cfg.font_size(), 20.0, "没动过时该用方案里的字号");
        assert_eq!(cfg.corner_radius(), 12.0, "没动过时该用方案里的圆角");
        assert!((cfg.opacity() - 0.5).abs() < 1e-6, "不透明度同理");

        // 老配置迁移：字段 ≠ 出厂默认 ⇒ 视为用户动过，压过方案。
        //
        // ★ 这里**不能**拿 0 当"用户动过"的样本（2026-09-26 起圆角出厂就是 0）：
        //   `f32_of` 的约定是"字段 == 出厂值 ⇒ 当没动过，让方案兜底"，
        //   "直角 0" 与 "没动过" 在字段层因此不可区分。用户真正选的 0 走
        //   **params**（下一段），那条路径带"改过"这个事实，能赢。
        settings.font_size = 26;
        settings.corner_radius = 8;
        let cfg = VkConfig::load(&settings);
        assert_eq!(cfg.font_size(), 26.0, "面板值必须优先于方案");
        assert_eq!(cfg.corner_radius(), 8.0, "字段动过就压过方案");

        // 新机制：用户改过的值在 params 里（优先级最高，含"改回出厂值"这一档）。
        settings.font_size = VirtualKeyboardSettings::default().font_size;
        settings.corner_radius = VirtualKeyboardSettings::default().corner_radius;
        let mut m = std::collections::BTreeMap::new();
        m.insert("defaults.fontSize".to_string(), ParamValue::Float(14.0));
        m.insert("defaults.cornerRadius".to_string(), ParamValue::Float(0.0));
        settings.params.insert(dir.to_string_lossy().to_string(), m);
        let cfg = VkConfig::load(&settings);
        assert_eq!(cfg.font_size(), 14.0, "params 里的值赢过方案与出厂字段");
        assert_eq!(cfg.corner_radius(), 0.0);
        let _ = fs::remove_dir_all(&dir);
    }

    /// ★ 每帧的方案读盘必须被缓存，但**文件改了要立刻生效**。
    #[test]
    fn scheme_load_is_cached_but_follows_the_file() {
        let dir = write_tmp_dir(
            "curseen_vk_cache",
            r#"{"name":"A","baseLayout":"ansi_104","defaults":{"font_size":18}}"#,
        );
        let settings = settings_for(&dir.to_string_lossy());
        assert_eq!(VkConfig::load(&settings).font_size(), 18.0);

        // 两次内容等长，只有 mtime 会变 —— 缓存必须靠文件戳而不是靠内容比对。
        std::thread::sleep(std::time::Duration::from_millis(30));
        fs::write(
            dir.join("keyboard.json"),
            r#"{"name":"A","baseLayout":"ansi_104","defaults":{"font_size":22}}"#,
        )
        .expect("改写临时方案失败");
        assert_eq!(
            VkConfig::load(&settings).font_size(),
            22.0,
            "文件改了缓存必须失效（下一帧就该看到新值）"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// 方案里自己的几何（rows）优先于 baseLayout 引用。
    #[test]
    fn own_rows_win_over_base_layout() {
        let dir = write_tmp_dir(
            "curseen_vk_rows",
            r#"{"name":"小板","baseLayout":"ansi_104",
                "rows":[{"keys":[{"code":"q","text":"Q"}]}]}"#,
        );
        let cfg = VkConfig::load(&settings_for(&dir.to_string_lossy()));
        assert_eq!(
            cfg.layout().rows.len(),
            1,
            "有自己的 rows 就不该用引用来的几何"
        );
        assert_eq!(cfg.layout().rows[0].keys.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    /// 坏方案（键码写错）要回落到出厂，而不是画出半块键盘。
    #[test]
    fn broken_rows_fall_back_to_builtin() {
        let dir = write_tmp_dir(
            "curseen_vk_broken",
            r#"{"name":"坏的","rows":[{"keys":[{"code":"NotAKey"}]}]}"#,
        );
        let cfg = VkConfig::load(&settings_for(&dir.to_string_lossy()));
        assert_eq!(cfg.layout().rows.len(), 6, "坏 rows 应回落出厂 104");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 方案里的命名样式与逐键覆盖要按优先级合起来。
    #[test]
    fn style_merging_respects_precedence() {
        let dir = write_tmp_dir(
            "curseen_vk_style",
            r##"{
                "name":"S", "baseLayout":"ansi_104",
                "defaults": {"corner_radius": 9},
                "styles": { "hot": {"fill":"#FF0000","radius":3} },
                "state_styles": { "pressed": {"fill":"#00FF00"} },
                "keys": { "CharQ": { "style": "hot" } }
            }"##,
        );
        let cfg = VkConfig::load(&settings_for(&dir.to_string_lossy()));
        let q = cfg
            .layout()
            .rows
            .iter()
            .flat_map(|r| r.keys.iter())
            .find(|k| k.base == "q")
            .expect("应有 q 键");
        let st = cfg.resolve_style(q, false);
        assert_eq!(st.fill, Argb::parse_hex("#FF0000").expect("色值应可解析"));
        assert_eq!(st.radius, 3.0, "命名样式的圆角应压过方案默认");

        // 方案 defaults 的圆角对其他键生效
        let w = cfg
            .layout()
            .rows
            .iter()
            .flat_map(|r| r.keys.iter())
            .find(|k| k.base == "w")
            .expect("应有 w 键");
        let st = cfg.resolve_style(w, false);
        assert_eq!(st.radius, 9.0);
        let _ = fs::remove_dir_all(&dir);
    }

    // ── 控件（面板上的选项 = 方案说了算）───

    /// ★ 用户提的核心问题："方案里有的按键颜色，面板上没有地方能改"。
    ///
    /// 这条锁住：**方案里每个写出来的颜色都必须长出一个控件**。
    #[test]
    fn every_scheme_color_grows_a_panel_control() {
        let cfg = VkConfig::load(&settings_for("ansi_104_dark"));
        let binds: Vec<String> = cfg.controls().iter().map(|c| c.bind.path()).collect();
        for want in [
            "styles.input.fill",
            "styles.input.press",
            "styles.input.text",
            "styles.input.border",
            "styles.func.fill",
            "styles.func.press",
            "styles.func.text",
            "styles.func.border",
        ] {
            assert!(
                binds.iter().any(|b| b == want),
                "方案里写了 {want}，面板上就该有对应的控件（控件清单：{binds:?}）"
            );
        }
        // ★ 两套样式都写了 `press`、104 颗键全归了类 → 兜底「按下色」谁也用不到，
        //   给了就是"改了没反应"（判据见 `base_press_is_reachable`）。
        assert!(
            !binds.iter().any(|b| b == "base.press"),
            "没有键会落到兜底按下色，不该再给一个改了没反应的色块"
        );
    }

    /// 方案可以给控件改名、改范围 —— 这是"方案作者决定暴露什么"的落点。
    #[test]
    fn scheme_can_rename_and_rescope_a_control() {
        let cfg = VkConfig::load(&settings_for("ansi_104_dark"));
        let c = cfg
            .controls()
            .iter()
            .find(|c| c.bind == Bind::KeySize)
            .expect("出厂方案声明了键帽大小");
        assert_eq!(c.label, "键帽大小");
        // 出厂样例声明的键帽大小量程（= 10~80，与出厂约定一致）。
        assert_eq!(
            c.kind,
            ControlKind::Range {
                min: 10.0,
                max: 80.0,
                step: 1.0,
                decimals: 0
            },
            "方案写的 min/max 要盖掉预设量程"
        );
        // 样式色按方案给的 label 显示，格式为 `<样式名>_<颜色类型>`（下划线分隔）。
        let fill = cfg
            .controls()
            .iter()
            .find(|c| c.bind == Bind::StyleFill("input".into()))
            .expect("输入键_键帽色 应有控件");
        assert_eq!(fill.label, "输入键_键帽色");
        let press = cfg
            .controls()
            .iter()
            .find(|c| c.bind == Bind::StylePress("func".into()))
            .expect("功能键_按下色 应有控件");
        assert_eq!(press.label, "功能键_按下色");
    }

    /// 用户在面板上改过的值（params）压过方案里写死的颜色。
    #[test]
    fn user_param_overrides_scheme_color() {
        let mut settings = settings_for("ansi_104_dark");
        let mut m = std::collections::BTreeMap::new();
        m.insert(
            "inputFill".to_string(),
            ParamValue::Color("#00FF00".to_string()),
        );
        settings.params.insert("ansi_104_dark".to_string(), m);

        let cfg = VkConfig::load(&settings);
        let q = cfg
            .layout()
            .rows
            .iter()
            .flat_map(|r| r.keys.iter())
            .find(|k| k.code == KeyCode::Char('q'))
            .expect("应有 Q 键");
        let st = cfg.resolve_style(q, false);
        assert_eq!(
            st.fill,
            Argb::parse_hex("#00FF00").expect("色值应可解析"),
            "面板上改过的颜色必须压过方案里的 #000000BF"
        );
        // 另一类键不受影响（改成输入键的底色不会把功能键一起改了）。
        let space = cfg
            .layout()
            .rows
            .iter()
            .flat_map(|r| r.keys.iter())
            .find(|k| k.code == KeyCode::Space)
            .expect("应有空格");
        assert_eq!(
            cfg.resolve_style(space, false).fill.to_hex_rgba(),
            "#004040BF"
        );
    }

    /// 没写 controls 的方案：预设项 + 命名样式照样齐全（不因"没声明"就少东西）。
    #[test]
    fn preset_controls_stay_when_scheme_declares_nothing() {
        let cfg = VkConfig::load(&settings_for("ansi_104"));
        let binds: Vec<String> = cfg.controls().iter().map(|c| c.bind.path()).collect();
        for want in [
            "defaults.keySize",
            "defaults.gap",
            "defaults.cornerRadius",
            "defaults.opacity",
            "defaults.hold",
            "defaults.fontSize",
            "base.fill",
            "base.press",
            "base.border",
            "base.text",
        ] {
            assert!(binds.iter().any(|b| b == want), "预设项 {want} 应保留");
        }
        // 滑块范围用出厂的那套（方案没改）—— 量程只在 `preset_range` 里定义一次，
        // 这里按它算，免得改了量程还要回来改数。
        let (min, max, step, decimals) = Bind::KeySize.preset_range();
        let ks = cfg
            .controls()
            .iter()
            .find(|c| c.bind == Bind::KeySize)
            .expect("应有键帽大小");
        assert_eq!(
            ks.kind,
            ControlKind::Range {
                min,
                max,
                step,
                decimals
            }
        );
    }

    /// ★ 控件清单快照：既是门禁，也是"面板上到底有什么"的可复现证据
    /// （`cargo test --release --lib -- --nocapture controls_snapshot` 可打印全表）。
    #[test]
    fn controls_snapshot_prints_what_the_panel_shows() {
        // 下拉里的方案顺序（= 文件名排序），一并印出来当证据。
        let list = scheme_list();
        println!(
            "── 方案下拉（{} 项）──\n{}",
            list.len(),
            list.iter()
                .map(|e| format!("  {:<16} {}", e.id, e.label))
                .collect::<Vec<_>>()
                .join("\n")
        );
        for id in ["ansi_104", "ansi_60", "ansi_104_dark"] {
            let cfg = VkConfig::load(&settings_for(id));
            let mut lines = Vec::new();
            for c in cfg.controls() {
                let kind = match c.kind {
                    ControlKind::Color => "色".to_string(),
                    ControlKind::Bool => "勾".to_string(),
                    ControlKind::Range { min, max, .. } => format!("{min}~{max}"),
                };
                lines.push(format!(
                    "{:<6} {:<8} {:>10}  bind={}",
                    c.label,
                    if c.group == ControlGroup::Color {
                        "颜色"
                    } else {
                        "数值"
                    },
                    kind,
                    c.bind.path()
                ));
            }
            println!(
                "── 方案 {id}（{}）：{} 个控件 ──\n{}",
                cfg.scheme_name(),
                lines.len(),
                lines.join("\n")
            );
            assert!(!lines.is_empty(), "{id} 至少要有一套控件");
        }
    }

    /// ★ 每个方案控件必落在「数值 / 颜色」二者之一，且**圆角一律算数值、颜色一律算颜色**。
    ///
    /// 这条钉住的是一类事故：面板曾把控件另按"主观刀口"拆成「外观 / 配色」四张卡，结果
    /// 「圆角」被挪进颜色卡、「字号 / 不透明度」被拆到第三张卡，用户看着就像"JSON 里的数值
    /// 选项丢了一堆"。
    ///
    /// 分组判据必须**客观**且落在本模块：它是 `ui::pages::key_display::control_rows` 分卡的
    /// **唯一依据**，判错就会让某项落进错误的组（例如"圆角"被排进颜色堆里）。页面怎么呈现
    /// （几张卡、叫什么）不在这里决定。
    #[test]
    fn controls_split_into_exactly_look_and_color() {
        for id in ["ansi_104", "ansi_60", "ansi_104_dark"] {
            let cfg = VkConfig::load(&settings_for(id));
            let all = cfg.controls();
            let look = all.iter().filter(|c| c.group == ControlGroup::Look).count();
            let color = all
                .iter()
                .filter(|c| c.group == ControlGroup::Color)
                .count();
            assert_eq!(
                look + color,
                all.len(),
                "{id}: 控件必须恰好分进「数值/颜色」两组"
            );
            for c in all {
                match c.bind {
                    Bind::CornerRadius | Bind::KeyRadius(_) | Bind::StyleRadius(_) => assert_eq!(
                        c.group,
                        ControlGroup::Look,
                        "{id}: 圆角 {} 必须算数值项",
                        c.bind.path()
                    ),
                    Bind::BaseFill
                    | Bind::BasePress
                    | Bind::BaseBorder
                    | Bind::BaseText
                    | Bind::PressFill
                    | Bind::PressText
                    | Bind::StyleFill(_)
                    | Bind::StyleText(_)
                    | Bind::StyleBorder(_) => assert_eq!(
                        c.group,
                        ControlGroup::Color,
                        "{id}: 颜色 {} 必须算颜色项",
                        c.bind.path()
                    ),
                    _ => {}
                }
            }
        }
    }

    /// bind 路径的解析与回写要对得上（配置里存的是路径/控件 id）。
    #[test]
    fn bind_path_roundtrips() {
        for b in [
            Bind::KeySize,
            Bind::Gap,
            Bind::Opacity,
            Bind::Hold,
            Bind::FontSize,
            Bind::CornerRadius,
            Bind::Scale,
            Bind::BaseFill,
            Bind::BasePress,
            Bind::BaseBorder,
            Bind::BaseText,
            Bind::PressFill,
            Bind::PressText,
            Bind::StyleFill("primary".into()),
            Bind::StyleText("mod".into()),
            Bind::StyleBorder("mod".into()),
            Bind::StyleRadius("mod".into()),
            Bind::KeyRadius("Space".into()),
        ] {
            assert_eq!(
                Bind::parse(&b.path()),
                Some(b.clone()),
                "{:?} 的路径 {} 必须能解析回自己",
                b,
                b.path()
            );
        }
        // 认不出来的路径：None（控件被跳过，不至于崩）。
        assert_eq!(Bind::parse("defaults.nonsense"), None);
        assert_eq!(Bind::parse("窗口行为.鼠标穿透"), None, "行为类选项不开放");
    }

    // ─── 2026-09-18：滑块量纲回归 ───
    //
    // 起因：面板报告"改一下键帽大小，键盘就变得非常小"。根因是 windui 的 slider 信号
    // 恒为 0..=1（`slider_value_range` 只改右侧标签的显示量纲），而控件值是真实像素 /
    // 秒 —— 面板建信号与落盘这两端各要换算一次，漏了哪一端，滑块一动就把 0.xx 当成
    // 48 像素存进配置。下面三条把整条链路钉住。

    /// 该 bind 对应的控件 id（不硬编码路径字符串，改名也不会让测试假红）。
    fn control_id(scheme: &str, bind: &Bind) -> String {
        VkConfig::load(&settings_for(scheme))
            .controls()
            .iter()
            .find(|c| &c.bind == bind)
            .map(|c| c.id.clone())
            .unwrap_or_else(|| panic!("方案 {scheme} 应有 {:?} 这个控件", bind))
    }

    /// 造一份"用户在面板上把这个 bind 改成了 v"的配置。
    fn with_bind(scheme: &str, bind: &Bind, v: ParamValue) -> VkConfig {
        let mut s = settings_for(scheme);
        let mut m = std::collections::BTreeMap::new();
        m.insert(control_id(scheme, bind), v);
        s.params.insert(scheme.to_string(), m);
        VkConfig::load(&s)
    }

    /// ★ 真实值 → 滑块 → 真实值，必须还原（且滑块位置恒在 0..=1）。
    #[test]
    fn slider_roundtrip_keeps_the_real_value() {
        // 量程从 `preset_range` 取（不写死），这样出厂量程一改，这条就跟着验新的。
        for bind in [
            Bind::KeySize,
            Bind::Gap,
            Bind::CornerRadius,
            Bind::Opacity,
            Bind::Hold,
            Bind::FontSize,
            Bind::Scale,
        ] {
            let name = bind.default_label();
            let (min, max, step, decimals) = bind.preset_range();
            let kind = ControlKind::Range {
                min,
                max,
                step,
                decimals,
            };
            // 取量程两端与一个 step 上的点：都能原样回来（不在 step 网格上的点会
            // 被吸附，那是预期行为，见 `from_slider` 的注释）。
            for v in [min, min + step, max] {
                let s = kind.to_slider(v);
                assert!(
                    (0.0..=1.0).contains(&s),
                    "{name}: 滑块位置 {s} 必须落在 0..=1（slider 只认比例）"
                );
                let back = kind.from_slider(s);
                assert!(
                    (back - v).abs() < 1e-4,
                    "{name}: {v} → 滑块 {s} → {back}，应还原"
                );
            }
            // 滑到最左 / 最右就是量程两端。
            assert_eq!(kind.from_slider(0.0), min, "{name}: 最左 = min");
            assert_eq!(kind.from_slider(1.0), max, "{name}: 最右 = max");
        }
    }

    /// ★ 量程外的值直接忽略，回退方案默认（顺手自愈 0.xx 那批存量配置）。
    #[test]
    fn out_of_range_param_is_dropped() {
        let fresh = VkConfig::load(&settings_for("ansi_104"));
        // 2026-09-18 的实际事故值：滑块位置 0.55 被当成键帽大小存了进去。
        let broken = with_bind("ansi_104", &Bind::KeySize, ParamValue::Float(0.55));
        assert_eq!(
            broken.unit(),
            fresh.unit(),
            "0.55 不在 12~96 里，必须被忽略而不是被当成 0.55 像素"
        );

        // 量程内的值照常生效。
        let good = with_bind("ansi_104", &Bind::KeySize, ParamValue::Float(64.0));
        assert_eq!(good.unit(), 64.0, "键帽大小控件必须真的改到键帽边长");
    }

    /// ★ 逐个数值控件：改它，渲染用的那个量必须跟着变。
    ///
    /// 这是"有的 JSON 预设属性好像没有真正控制对应属性"的正面回答 —— 七个量全覆盖，
    /// 少一个就等于承认面板上有个拖了没反应的滑块。
    #[test]
    fn every_numeric_control_drives_its_render_value() {
        let scheme = "ansi_104";
        // ★ 这六个就是面板**数值组**（`ControlGroup::Look`）的全部滑块（`controls_of` 的预设清单）。
        //   `scale` 不在其中（面板不暴露缩放，它只由方案 JSON 声明，见下一条测试）。
        //
        //   取值点按**各自的量程**算（25% / 75% 两档），不写死数字 —— 量程改了不用回来改。
        type BindRead = (Bind, fn(&VkConfig) -> f32);
        let reads: [BindRead; 6] = [
            (Bind::KeySize, |c| c.unit()),
            (Bind::Gap, |c| c.gap()),
            (Bind::Opacity, |c| c.opacity()),
            (Bind::Hold, |c| c.hold() as f32),
            (Bind::FontSize, |c| c.font_size()),
            (Bind::CornerRadius, |c| c.corner_radius()),
        ];
        for (bind, read) in reads {
            let (lo, hi, ..) = bind.preset_range();
            for t in [0.25f32, 0.75f32] {
                let v = lo + (hi - lo) * t;
                let c = with_bind(scheme, &bind, ParamValue::Float(v));
                assert!(
                    (read(&c) - v).abs() < 1e-3,
                    "{:?} 设成 {v} 后读回 {}，控件没接到渲染上",
                    bind,
                    read(&c)
                );
            }
        }
    }

    /// ★ 颜色与圆角：改控件必须落到**键帽上**（不只是配色卡上有个色块）。
    #[test]
    fn color_and_radius_controls_reach_the_key_cap() {
        let cfg = VkConfig::load(&settings_for("ansi_104"));
        let q = cfg
            .layout()
            .rows
            .iter()
            .flat_map(|r| r.keys.iter())
            .find(|k| k.base == "q")
            .expect("应有 q 键");

        let painted = with_bind(
            "ansi_104",
            &Bind::BaseFill,
            ParamValue::Color("#123456FF".into()),
        );
        assert_eq!(
            painted.resolve_style(q, false).fill,
            Argb::parse_hex("#123456FF").expect("色值应可解析"),
            "键帽色控件必须真的改到键帽填充"
        );

        let rounded = with_bind("ansi_104", &Bind::CornerRadius, ParamValue::Float(11.0));
        assert_eq!(
            rounded.resolve_style(q, false).radius,
            11.0,
            "圆角控件必须真的改到键帽圆角"
        );

        // 命名样式色：从控件清单里挑一个样式底色，再找一颗**真的用了它**的键。
        let styled = VkConfig::load(&settings_for("ansi_104_dark"));
        let ctl = styled
            .controls()
            .iter()
            .find(|c| matches!(c.bind, Bind::StyleFill(_)))
            .expect("ansi_104_dark 方案里 styles 表应长出样式底色控件");
        let Bind::StyleFill(name) = &ctl.bind else {
            unreachable!("上面已筛选")
        };
        let style_fill = styled
            .scheme
            .styles
            .get(name)
            .and_then(|s| s.fill.as_deref())
            .and_then(Argb::parse_hex)
            .expect("该样式应有底色");
        let kd = styled
            .layout()
            .rows
            .iter()
            .flat_map(|r| r.keys.iter())
            .find(|k| styled.resolve_style(k, false).fill == style_fill)
            .unwrap_or_else(|| panic!("应有键用了 '{name}' 样式"));
        let recolored = with_bind(
            "ansi_104_dark",
            &Bind::StyleFill(name.clone()),
            ParamValue::Color("#00FF00FF".into()),
        );
        assert_eq!(
            recolored.resolve_style(kd, false).fill,
            Argb::parse_hex("#00FF00FF").expect("色值应可解析"),
            "方案里 '{name}' 样式的底色必须能在面板上改到"
        );
    }

    /// 方案 JSON 的 `defaults.scale` 也要落到键帽边长上 —— 面板上**没有**缩放滑块
    /// （它与键帽大小是同一个乘子），方案声明是它唯一的入口，别让它也是个死参数。
    #[test]
    fn scheme_scale_is_consumed_without_a_panel_control() {
        let dir = write_tmp_dir(
            "curseen_vk_scale_20260918",
            "{\"name\":\"T\",\"baseLayout\":\"ansi_104\",\
             \"defaults\":{\"key_size\":60,\"scale\":1.5}}",
        );
        let cfg = VkConfig::load(&settings_for(&dir.to_string_lossy()));
        assert_eq!(cfg.unit(), 90.0, "60 × 1.5 = 90（缩放必须真的乘上去）");
        let _ = fs::remove_dir_all(&dir);
    }

    /// ★ 出厂默认值必须落在**自己的量程**里。
    ///
    /// 越界的后果不显眼但很烦：滑块初值被 clamp 到边界（标签显示 min），而渲染用的是
    /// 那个越界的原值 —— 面板显示与实际不一致，用户怎么拖都对不上。
    /// 历史事故：出厂键帽大小曾落在自己的量程之外（34 / 50~100），滑块被 clamp 到边界，
    /// 而渲染用的是那个越界原值 —— 面板显示与实际不一致，用户怎么拖都对不上。
    #[test]
    fn factory_defaults_sit_inside_their_own_ranges() {
        let d = VirtualKeyboardSettings::default();
        let checks: [(Bind, f32); 7] = [
            (Bind::KeySize, d.key_size as f32),
            (Bind::Gap, d.gap as f32),
            (Bind::Opacity, d.opacity),
            (Bind::Hold, d.hold),
            (Bind::FontSize, d.font_size as f32),
            (Bind::CornerRadius, d.corner_radius as f32),
            (Bind::Scale, d.scale),
        ];
        for (b, v) in checks {
            let (lo, hi, ..) = b.preset_range();
            assert!(
                v >= lo && v <= hi,
                "{:?} 的出厂默认 {v} 落在量程 {lo}~{hi} 之外",
                b
            );
        }
    }

    /// ★ 三个几何量程是**约定值**（2026-09-18 用户定）：键帽大小 10~80、键间空隙 0~20、
    /// 圆角 0~20。面板滑块能拖多远直接由 `preset_range` 决定，改它 = 改用户的可调范围。
    #[test]
    fn preset_ranges_are_the_agreed_numbers() {
        assert_eq!(
            Bind::KeySize.preset_range(),
            (10.0, 80.0, 1.0, 0),
            "键帽大小应为 10~80"
        );
        assert_eq!(
            Bind::Gap.preset_range(),
            (0.0, 20.0, 1.0, 0),
            "键间空隙应为 0~20"
        );
        for b in [
            Bind::CornerRadius,
            Bind::StyleRadius("s".into()),
            Bind::KeyRadius("k".into()),
        ] {
            assert_eq!(b.preset_range(), (0.0, 20.0, 1.0, 0), "{b:?} 应为 0~20");
        }

        // 方案 JSON 也能声明量程（`ansi_104_dark` 就声明了 keySize）：声明值必须与约定一致，
        // 否则同一项在深色方案里能拖到一边、在浅色方案里只能拖到另一边。
        let cfg = VkConfig::load(&settings_for("ansi_104_dark"));
        let ks = cfg
            .controls()
            .iter()
            .find(|c| matches!(&c.bind, Bind::KeySize))
            .expect("深色方案应有键帽大小");
        assert_eq!(
            ks.kind,
            ControlKind::Range {
                min: 10.0,
                max: 80.0,
                step: 1.0,
                decimals: 0
            },
            "方案声明的键帽大小量程应与出厂约定一致"
        );
    }

    /// ★ 圆角项连成一段（紧跟全局「圆角」之后），标签统一 `圆角_<作用域>`。
    ///
    /// 钉住的是一类观感事故：命名样式的圆角散在各家颜色项后面，被读成"修饰键专有的属性"，
    /// 而它和全局「圆角」其实是同一个量、只是作用范围不同（整块 / 一类键 / 单颗键）。
    #[test]
    fn radius_controls_are_grouped_together() {
        let is_radius = |b: &Bind| {
            matches!(
                b,
                Bind::CornerRadius | Bind::StyleRadius(_) | Bind::KeyRadius(_)
            )
        };

        // 浅色方案没有命名样式 → 只有全局圆角，且紧跟在「键间空隙」后面。
        let cfg = VkConfig::load(&settings_for("ansi_104"));
        let labels: Vec<&str> = cfg.controls().iter().map(|c| c.label.as_str()).collect();
        let pos = |l: &str| {
            labels
                .iter()
                .position(|x| *x == l)
                .unwrap_or_else(|| panic!("应有 {l} 控件：{labels:?}"))
        };
        assert_eq!(pos("圆角"), pos("键间空隙") + 1, "全局圆角应紧跟键间空隙");

        // 带命名样式圆角的方案 → 三个圆角项必须连续且同前缀。
        //
        // ★ 双色方案自己不带 `radius`（圆角统一走全局），所以这里就地构造一份带样式圆角的
        //   方案来验分组 —— 分组规则本身与"出厂哪套方案带圆角"无关。
        let mut sc = Scheme {
            id: "radius_demo".into(),
            name: "圆角样例".into(),
            layout: layout::builtin_layout("ansi_104").expect("104 方案应可解析"),
            defaults: VkDefaults::default(),
            styles: HashMap::from([
                (
                    "input".to_string(),
                    VkStyleDef {
                        label: Some("输入键".into()),
                        radius: Some(4.0),
                        ..Default::default()
                    },
                ),
                (
                    "func".to_string(),
                    VkStyleDef {
                        label: Some("功能键".into()),
                        radius: Some(8.0),
                        ..Default::default()
                    },
                ),
            ]),
            state_styles: HashMap::new(),
            keys: HashMap::new(),
            controls: Vec::new(),
            declared: None,
        };
        sc.controls = controls_of(&sc);
        let all = &sc.controls;
        let radii: Vec<&ControlDef> = all.iter().filter(|c| is_radius(&c.bind)).collect();
        assert_eq!(
            radii.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(),
            vec!["圆角", "圆角_功能键", "圆角_输入键"],
            "圆角项应连成一段、且都带「圆角」前缀（样式按名字排序）"
        );
        let first = all
            .iter()
            .position(|c| is_radius(&c.bind))
            .expect("应有圆角");
        let last = all
            .iter()
            .rposition(|c| is_radius(&c.bind))
            .expect("应有圆角");
        assert_eq!(last - first + 1, radii.len(), "圆角项之间不该夹别的控件");
        for c in radii {
            assert_eq!(
                c.kind,
                ControlKind::Range {
                    min: 0.0,
                    max: 20.0,
                    step: 1.0,
                    decimals: 0
                },
                "{} 的量程应为 0~20",
                c.label
            );
        }
    }

    /// 图例简写：功能键区那三颗键名太长（1u 键放 5 个字母），几何里直接写短的。
    ///
    /// 简写改的是**方案 JSON**（根目录 `*.json` 即来源），不是代码里的常量表 —— 用户想改回去
    /// 改 JSON 就行。
    #[test]
    fn long_legends_are_shortened_in_the_scheme() {
        let cfg = VkConfig::load(&settings_for("ansi_104"));
        let legend = |code: KeyCode| {
            cfg.layout()
                .rows
                .iter()
                .flat_map(|r| r.keys.iter())
                .find(|k| k.code == code)
                .map(|k| cfg.resolve_label(k, false, false))
                .unwrap_or_else(|| panic!("应有 {code:?} 键"))
        };
        assert_eq!(legend(KeyCode::PrtSc), "Prt");
        assert_eq!(legend(KeyCode::ScrollLock), "Scr");
        assert_eq!(legend(KeyCode::Pause), "Pau");
        // 没被简写的照旧。
        assert_eq!(legend(KeyCode::Home), "Home");
    }

    /// ★ 侧壁特性（2026-09-28，随四套外来样式方案加入）：
    /// 样式级 `side`/`depth` 逐样式解析；出厂方案没写侧壁时必须保持"无侧壁"，
    /// 渲染层据此短路 —— 老方案一个字节都不能变。
    #[test]
    fn side_wall_resolves_per_style_and_stays_absent_by_default() {
        let find = |cfg: &VkConfig, code: KeyCode| {
            cfg.layout()
                .rows
                .iter()
                .flat_map(|r| r.keys.iter())
                .find(|k| k.code == code)
                .expect("该键应存在")
                .clone()
        };

        // 硬阴影·暗夜：输入键/功能键各自带侧壁，颜色与厚度都按样式分开。
        let kiosk = VkConfig::load(&settings_for("ansi_104_kiosk_dark"));
        let st_in = kiosk.resolve_style(&find(&kiosk, KeyCode::Char('q')), false);
        assert_eq!(st_in.side.to_hex_rgba(), "#FFFFFF33", "输入键侧壁色");
        assert_eq!(st_in.depth, 4.0, "输入键侧壁厚度");
        let st_fn = kiosk.resolve_style(&find(&kiosk, KeyCode::Backspace), false);
        assert_eq!(st_fn.side.to_hex_rgba(), "#FFFFFF2E", "功能键侧壁色");
        // 按下位移不在 resolve 里（渲染层按 depth 算），但按下色要能解析：
        assert_eq!(st_in.fill_pressed.to_hex_rgba(), "#3D3D3DFF");

        // 双层键帽：侧壁=裙边色 #44475A，按下色=裙边色（顶面沉入露裙边）。
        let dbl = VkConfig::load(&settings_for("ansi_104_kb_double"));
        let st = dbl.resolve_style(&find(&dbl, KeyCode::Char('q')), false);
        assert_eq!(st.side.to_hex_rgba(), "#44475AFF");
        assert_eq!(st.depth, 5.0);
        assert_eq!(st.fill_pressed.to_hex_rgba(), "#44475AFF");

        // 出厂双色方案没写侧壁 → side 透明、depth 0（渲染层与旧版逐字节一致）。
        let legacy = VkConfig::load(&settings_for("ansi_104_dark"));
        let st = legacy.resolve_style(&find(&legacy, KeyCode::Char('q')), false);
        assert_eq!(st.side.a(), 0, "出厂方案不得带出侧壁");
        assert_eq!(st.depth, 0.0);

        // 侧壁色/厚度在面板上有控件（"看得见改不了"的教训）。
        let ctls = controls_of(&kiosk.scheme);
        assert!(
            ctls.iter()
                .any(|c| matches!(&c.bind, Bind::StyleSide(n) if n == "input")),
            "输入键_侧壁 控件应出现"
        );
        assert!(
            ctls.iter()
                .any(|c| matches!(&c.bind, Bind::StyleDepth(n) if n == "input")),
            "侧壁厚度_输入键 控件应出现"
        );
    }
}
