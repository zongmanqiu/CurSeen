//! 键盘布局的几何数据 —— **根目录 `*.json` 即来源**。
//!
//! # 数据格式：行数组 + 相对 u 单位（JSON 化）
//!
//! 调研了多个同类开源实现（主播展示键盘 / 按键回显工具 / 屏幕键盘）后定下来的格式，
//! 取舍如下：
//!
//! | 方案 | 已见做法 | 问题 |
//! |---|---|---|
//! | 绝对像素多边形（社区通行 `keyboards/**/keyboard.json`，大方案 3000+ 行） | 主播键盘生态的标杆格式 | 体积大、不能缩放、无法按 DPI/窗口尺寸自适应 |
//! | 只有字符行数组，无几何 | 部分按键回显实现（数十种布局） | 画不出键帽 |
//! | CSS / XAML 绝对宽 | Web / XAML 系实现 | 与"纯 CPU 软渲染 + 可缩放"不合 |
//! | **行数组 + 相对 u 单位 + 每键 w** | **本模块** | 只存宽、x 由累加得出，紧凑又可任意缩放 |
//!
//! 关键约定：
//! - `1u` = 一个标准键的边长。非标准键全靠 `w` 表达（Tab 1.5 / Caps 1.75 / Enter 2.25 /
//!   LShift 2.25 / RShift 2.75 / Space 6.25 / 底排修饰键 1.25），**不需要多边形**；
//! - `h = 2` 表达"竖着占两行"的键（小键盘 `+` 与 `Enter`），渲染时就是一个高矩形；
//! - `pad` = 本键之前的额外留白，用于功能键行分组与导航区/小键盘的分栏。
//!
//! # 根目录 `*.json` 即来源（2.x）
//!
//! 每**一个文件** `assets/VirtualKeyboards/<方案名>.json` 就是一套完整方案（几何 +
//! 默认值 + 样式 + 逐键覆盖）。程序启动时扫描该目录，"虚拟键盘"下拉里就是扫到的东西：
//! **用户丢一个 JSON 进去、能解析，方案就直接可用**，不需要改配置也不需要重编译。
//!
//! ★ **不再建子目录**：文件名就是方案 id（也是下拉里的身份），多一层文件夹没有任何
//! 信息量，只会让"我这套方案叫什么"要去看目录名而不是文件名。
//!
//! 出厂四套（`ansi_104` / `ansi_104_dark` / `ansi_60` / `wasd_gaming`）同时以
//! `include_str!` 内嵌在二进制里——目录被清空、被误删时仍然有键盘可显示，不至于白屏；
//! 目录里同名方案优先于内嵌的。
//!
//! 几何是**编译期常量表 → 导出 JSON** 的一次性迁移（见 `temp/export_vk_layouts.py`）：
//! 导出时逐键比对过键数 / 行数 / 总宽高 / PrtSc 对齐，与原来的 Rust 常量表逐键等价。

use std::cell::RefCell;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::core::events::KeyCode;

// ─── 运行时类型（owned：方案来自文件，无法是 &'static） ───

/// 一个键的定义。
#[derive(Clone, Debug, PartialEq)]
pub struct KeyDef {
    /// 对应的物理键。修饰键刻意区分左右（左右各占一个键位）。
    pub code: KeyCode,
    /// 宽（单位 u，1.0 = 标准键）。
    pub w: f32,
    /// 高（单位 u）。> 1 表示竖着占多行（小键盘 `+` / `Enter`）。
    pub h: f32,
    /// 本键之前的额外留白（单位 u）。
    pub pad: f32,
    /// 常态图例。空串 = 交给 `defaults::default_label` 派生。
    pub base: String,
    /// Shift 态图例。缺省时等于 `base`（Shift 不改变键帽文字）。
    pub shift: String,
    /// 是否受 CapsLock 影响（字母 true，数字/符号 false）。
    pub caps: bool,
    /// 引用的命名样式名（指向方案的 `styles` 表）。`None` = 用面板上的颜色。
    pub style: Option<String>,
    /// 单键圆角覆盖（逻辑像素）。`None` = 跟随全局圆角。
    pub radius: Option<f32>,
}

impl KeyDef {
    /// 测试与内置构造用；方案里的键走 [`LayoutJson`] 解析。
    pub fn new(code: KeyCode, base: &str, shift: &str) -> Self {
        Self {
            code,
            w: 1.0,
            h: 1.0,
            pad: 0.0,
            base: base.to_string(),
            shift: shift.to_string(),
            caps: false,
            style: None,
            radius: None,
        }
    }

    pub fn w(mut self, w: f32) -> Self {
        self.w = w;
        self
    }

    pub fn h(mut self, h: f32) -> Self {
        self.h = h;
        self
    }

    pub fn pad(mut self, pad: f32) -> Self {
        self.pad = pad;
        self
    }

    pub fn caps(mut self) -> Self {
        self.caps = true;
        self
    }
}

/// 一行键。
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub keys: Vec<KeyDef>,
}

/// 一套布局。
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    /// 方案 id（= 文件名去掉 `.json` 扩展名，也是配置里落盘的值）。
    pub id: String,
    /// 下拉里显示的名字（方案 JSON 的 `name`，缺省用 id）。
    pub label: String,
    pub rows: Vec<Row>,
}

// ─── JSON 结构 ───

/// 方案 JSON 里的一颗键。
///
/// 除了 `code` 全是可选的：省略即用"标准键"的默认（1u 见方、无留白、文字按 `code` 派生）。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct KeyDefJson {
    /// 键码。支持三种写法：单字符（`"q"` / `";"`）、枚举名（`"Esc"` / `"LShift"`）、
    /// 带编号（`"F5"` / `"Num3"`）。
    pub code: String,
    /// 常态图例。省略 = 按 `code` 从 `defaults::default_label` 派生。
    pub text: Option<String>,
    /// Shift 态图例。省略 = 与 `text` 相同。**这就是"Shift 按下以后的名称"**。
    ///
    /// ★ 字段名统一走**下划线**（与 `state_styles` / `font_size` / `corner_radius` 一致）；
    ///   `shiftText` 作为别名保留，免得早期手写的方案忽然失效。
    #[serde(alias = "shiftText")]
    pub shift_text: Option<String>,
    pub w: Option<f32>,
    pub h: Option<f32>,
    pub pad: Option<f32>,
    /// 受 CapsLock 影响（字母键要写 true，否则 Caps 开时键帽不变）。
    pub caps: Option<bool>,
    /// 引用的命名样式名（指向 `styles` 表）。
    pub style: Option<String>,
    /// 单键圆角覆盖（逻辑像素）。
    pub radius: Option<f32>,
}

/// 方案 JSON 里的一行。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct RowJson {
    pub keys: Vec<KeyDefJson>,
}

/// 方案的几何部分（`vk_config::VkJson` 会把它嵌进来）。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct LayoutJson {
    /// 方案显示名。
    pub name: Option<String>,
    /// 引用的内置布局 id（仅当没写 `rows` 时使用，用来做"纯样式方案"）。
    ///
    /// 与 [`KeyDefJson::shift_text`] 同一条规矩：下划线为准，驼峰作别名兼容。
    #[serde(alias = "baseLayout")]
    pub base_layout: Option<String>,
    /// 完整几何。写了它就以它为准。
    pub rows: Vec<RowJson>,
}

// ─── 出厂内嵌方案 ───

/// 内嵌的出厂方案：`(方案 id, 出厂 JSON 全文)`。
///
/// 与 `assets/VirtualKeyboards/<id>.json` 是**同一份内容的两个副本**：文件那份是用户
/// 可改可删的现行版本，这份是"文件没了也要有键盘"的兜底。
///
/// 数组顺序**不决定下拉顺序**（下拉按 id 排序，见 `scan`），这里只按 id 排好方便阅读。
/// `ansi_104_dark` 是"纯样式方案"，靠 `baseLayout` 引用 104 的几何，所以自己不带 `rows`；
/// `wasd_gaming` 是直播/游戏用的迷你板（WASD + 空格 + 鼠标左右键），颜色走内置兜底
/// （与 104 原版同源），鼠标键只有显示语义（见 [`KeyCode::MouseLeft`]）。
pub const BUILTIN: &[(&str, &str)] = &[
    (
        "ansi_104",
        include_str!("../../../assets/VirtualKeyboards/ansi_104.json"),
    ),
    (
        "ansi_104_dark",
        include_str!("../../../assets/VirtualKeyboards/ansi_104_dark.json"),
    ),
    (
        "ansi_60",
        include_str!("../../../assets/VirtualKeyboards/ansi_60.json"),
    ),
    (
        "wasd_gaming",
        include_str!("../../../assets/VirtualKeyboards/wasd_gaming.json"),
    ),
];

/// 取内嵌方案 JSON 文本。
pub fn builtin_json(id: &str) -> Option<&'static str> {
    BUILTIN.iter().find(|(k, _)| *k == id).map(|(_, t)| *t)
}

/// 解析内嵌方案为布局。
pub fn builtin_layout(id: &str) -> Option<Layout> {
    let text = builtin_json(id)?;
    let json: LayoutJson = serde_json::from_str(text).ok()?;
    layout_from_json(id, json.name.as_deref().unwrap_or(id), &json).ok()
}

// ─── 方案发现 ───

/// 下拉里的一项。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemeEntry {
    /// 方案 id（= 文件名去掉 `.json`）。
    pub id: String,
    /// 显示名（方案 JSON 的 `name`）。
    pub label: String,
}

/// 扫描方案目录。结果**按 id（= 文件名去扩展名）排序**，出厂方案与目录里新增的混在一起排
/// —— 同名前缀的方案因此天然相邻（`ansi_104` → `ansi_104_dark` → `ansi_60`）。
/// 目录里有同名方案时，用目录那份的显示名（内容也优先用目录那份，见 `vk_config`）。
///
/// 带 500ms 的进程内缓存：面板重建（每次交互都会）不该每次都去敲磁盘目录。
pub fn discover(base: &Path) -> Vec<SchemeEntry> {
    const TTL: Duration = Duration::from_millis(500);
    let cached: Option<Vec<SchemeEntry>> = DISCOVER_CACHE.with(|c| {
        let b = c.borrow();
        match b.as_ref() {
            Some((p, at, v)) if p == base && at.elapsed() < TTL => Some(v.clone()),
            _ => None,
        }
    });
    if let Some(v) = cached {
        return v;
    }
    let v = scan(base);
    DISCOVER_CACHE.with(|c| {
        *c.borrow_mut() = Some((base.to_path_buf(), Instant::now(), v.clone()));
    });
    v
}

thread_local! {
    /// `(目录, 上次扫描时刻, 结果)`。只有一个方案目录，单条缓存够用。
    static DISCOVER_CACHE: RefCell<Option<(PathBuf, Instant, Vec<SchemeEntry>)>> =
        const { RefCell::new(None) };
}

fn scan(base: &Path) -> Vec<SchemeEntry> {
    let mut from_dir: Vec<SchemeEntry> = Vec::new();
    if let Ok(rd) = fs::read_dir(base) {
        for e in rd.flatten() {
            let file = e.path();
            // ★ 只认**顶层**的 `*.json`：方案就是文件本身，不再套一层目录。
            if !file.is_file() || file.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let Some(id) = file
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            // 显示名取方案自己的 `name`；读不出来（JSON 坏了）就退回文件名——
            // 坏方案也要出现在下拉里，否则用户"丢进去没反应"会以为机制没生效。
            let label = fs::read_to_string(&file)
                .ok()
                .and_then(|t| serde_json::from_str::<LayoutJson>(&t).ok())
                .and_then(|j| j.name)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| id.clone());
            from_dir.push(SchemeEntry { id, label });
        }
    }
    from_dir.sort_by(|a, b| a.id.cmp(&b.id));

    let mut out: Vec<SchemeEntry> = Vec::new();
    let mut used: HashSet<String> = HashSet::new();
    for (id, text) in BUILTIN {
        let label = from_dir
            .iter()
            .find(|e| e.id == *id)
            .map(|e| e.label.clone())
            .or_else(|| name_of(text))
            .unwrap_or_else(|| (*id).to_string());
        used.insert((*id).to_string());
        out.push(SchemeEntry {
            id: (*id).to_string(),
            label,
        });
    }
    for e in from_dir {
        if !used.contains(&e.id) {
            out.push(e);
        }
    }
    // ★ 下拉顺序 = **按 id 排序**（id 就是文件名去 `.json`），不做出厂优先。
    // 用户要的是"深色紧跟在它改的那套基础方案下面"：`ansi_104` / `ansi_104_dark` / `ansi_60`
    // 按文件名一排就是这个结果；以后往目录里丢 `ansi_104_light` 之类也自动归位。
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// 从 JSON 文本里取 `name`。
fn name_of(text: &str) -> Option<String> {
    serde_json::from_str::<LayoutJson>(text)
        .ok()
        .and_then(|j| j.name)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

// ─── 解析 ───

/// 把方案的 `rows` 解析成布局。`rows` 为空 → `Err`（调用方据此回落到内置几何）。
pub fn layout_from_json(id: &str, label: &str, json: &LayoutJson) -> Result<Layout, String> {
    if json.rows.is_empty() {
        return Err("方案没有 rows".to_string());
    }
    let mut rows = Vec::with_capacity(json.rows.len());
    for (r, row) in json.rows.iter().enumerate() {
        if row.keys.is_empty() {
            return Err(format!("第 {r} 行没有任何键"));
        }
        let mut keys = Vec::with_capacity(row.keys.len());
        for (i, k) in row.keys.iter().enumerate() {
            let code = parse_code(&k.code)
                .ok_or_else(|| format!("第 {r} 行第 {i} 个键的 code 无法识别：{:?}", k.code))?;
            let base = k.text.clone().unwrap_or_default();
            // `shiftText` 缺省 = 与 base 相同（Shift 不改文字）。
            let shift = k.shift_text.clone().unwrap_or_else(|| base.clone());
            keys.push(KeyDef {
                code,
                w: k.w.unwrap_or(1.0).max(0.05),
                h: k.h.unwrap_or(1.0).max(0.05),
                pad: k.pad.unwrap_or(0.0).max(0.0),
                base,
                shift,
                caps: k.caps.unwrap_or(false),
                style: k.style.clone(),
                radius: k.radius,
            });
        }
        rows.push(Row { keys });
    }
    Ok(Layout {
        id: id.to_string(),
        label: label.to_string(),
        rows,
    })
}

/// JSON 里的键码字符串 → [`KeyCode`]。
///
/// 三种写法都认：
/// - **单字符**：`"q"` → `Char('q')`、`";"` → `Char(';')`（方案里写键位最直观的写法）；
/// - **枚举名**：`"Esc"` / `"Backspace"` / `"LShift"`（与 `code_to_name` 同源）；
/// - **带编号**：`"F1"`..`"F12"`、`"Num0"`..`"Num9"`。
///
/// 另外兼容 `code_to_name` 的旧写法（`"CharA"` / `"SymSemicolon"`），旧方案照常加载。
pub fn parse_code(s: &str) -> Option<KeyCode> {
    // 单字符（空格例外：它走 "Space"，否则 "" 与 " " 会分不清）
    let mut it = s.chars();
    if let (Some(c), None) = (it.next(), it.next()) {
        if c != ' ' {
            return Some(KeyCode::Char(c.to_ascii_lowercase()));
        }
    }

    match s {
        "Backspace" => Some(KeyCode::Backspace),
        "Tab" => Some(KeyCode::Tab),
        "CapsLock" => Some(KeyCode::CapsLock),
        "Enter" => Some(KeyCode::Enter),
        "Space" => Some(KeyCode::Space),
        "Esc" => Some(KeyCode::Esc),
        "LShift" => Some(KeyCode::LShift),
        "RShift" => Some(KeyCode::RShift),
        "LCtrl" => Some(KeyCode::LCtrl),
        "RCtrl" => Some(KeyCode::RCtrl),
        "LAlt" => Some(KeyCode::LAlt),
        "RAlt" => Some(KeyCode::RAlt),
        "LWin" => Some(KeyCode::LWin),
        "RWin" => Some(KeyCode::RWin),
        "Menu" => Some(KeyCode::Menu),
        "PrtSc" => Some(KeyCode::PrtSc),
        "ScrollLock" => Some(KeyCode::ScrollLock),
        "Pause" => Some(KeyCode::Pause),
        "Insert" => Some(KeyCode::Insert),
        "Delete" => Some(KeyCode::Delete),
        "Home" => Some(KeyCode::Home),
        "End" => Some(KeyCode::End),
        "PageUp" => Some(KeyCode::PageUp),
        "PageDown" => Some(KeyCode::PageDown),
        "Up" => Some(KeyCode::Up),
        "Down" => Some(KeyCode::Down),
        "Left" => Some(KeyCode::Left),
        "Right" => Some(KeyCode::Right),
        "NumLock" => Some(KeyCode::NumLock),
        "NumDiv" => Some(KeyCode::NumDiv),
        "NumMul" => Some(KeyCode::NumMul),
        "NumSub" => Some(KeyCode::NumSub),
        "NumAdd" => Some(KeyCode::NumAdd),
        "NumDot" => Some(KeyCode::NumDot),
        "NumEnter" => Some(KeyCode::NumEnter),
        // 鼠标键：只有"显示"语义（直播游戏板）。别名 LMB/RMB 也认，写哪个都一样。
        "MouseLeft" | "LMB" => Some(KeyCode::MouseLeft),
        "MouseRight" | "RMB" => Some(KeyCode::MouseRight),
        "MouseMiddle" | "MMB" => Some(KeyCode::MouseMiddle),
        "MouseX1" | "X1" => Some(KeyCode::MouseX1),
        "MouseX2" | "X2" => Some(KeyCode::MouseX2),
        "WheelUp" => Some(KeyCode::WheelUp),
        "WheelDown" => Some(KeyCode::WheelDown),
        "VolumeUp" => Some(KeyCode::VolumeUp),
        "VolumeDown" => Some(KeyCode::VolumeDown),
        "VolumeMute" => Some(KeyCode::VolumeMute),
        "MediaNext" => Some(KeyCode::MediaNext),
        "MediaPrev" => Some(KeyCode::MediaPrev),
        "MediaStop" => Some(KeyCode::MediaStop),
        "MediaPlay" => Some(KeyCode::MediaPlay),
        "BrowserBack" => Some(KeyCode::BrowserBack),
        "BrowserForward" => Some(KeyCode::BrowserForward),
        "BrowserRefresh" => Some(KeyCode::BrowserRefresh),
        "BrowserHome" => Some(KeyCode::BrowserHome),
        _ => {
            if let Some(rest) = s.strip_prefix('F') {
                if let Ok(n) = rest.parse::<u8>() {
                    if (1..=12).contains(&n) {
                        return Some(KeyCode::F(n));
                    }
                }
            }
            if let Some(rest) = s.strip_prefix("Num") {
                if let Ok(n) = rest.parse::<u8>() {
                    if n <= 9 {
                        return Some(KeyCode::Num(n));
                    }
                }
            }
            // 旧写法的兼容：CharA / Char0
            if let Some(rest) = s.strip_prefix("Char") {
                if let Some(c) = single_char(rest) {
                    return Some(KeyCode::Char(c.to_ascii_lowercase()));
                }
            }
            // 旧写法的兼容：SymXxx
            symbol_by_name(s).map(KeyCode::Char)
        }
    }
}

fn single_char(s: &str) -> Option<char> {
    let mut it = s.chars();
    match (it.next(), it.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    }
}

/// `SymXxx` 旧写法 → 字符。
fn symbol_by_name(s: &str) -> Option<char> {
    Some(match s {
        "SymBacktick" => '`',
        "SymMinus" => '-',
        "SymEquals" => '=',
        "SymLBracket" => '[',
        "SymRBracket" => ']',
        "SymBackslash" => '\\',
        "SymSemicolon" => ';',
        "SymQuote" => '\'',
        "SymComma" => ',',
        "SymPeriod" => '.',
        "SymSlash" => '/',
        "SymTilde" => '~',
        "SymExcl" => '!',
        "SymAt" => '@',
        "SymHash" => '#',
        "SymDollar" => '$',
        "SymPercent" => '%',
        "SymCaret" => '^',
        "SymAmp" => '&',
        "SymStar" => '*',
        "SymLParen" => '(',
        "SymRParen" => ')',
        "SymUnderscore" => '_',
        "SymPlus" => '+',
        "SymLBrace" => '{',
        "SymRBrace" => '}',
        "SymPipe" => '|',
        "SymColon" => ':',
        "SymDQuote" => '"',
        "SymLAngle" => '<',
        "SymRAngle" => '>',
        "SymQuestion" => '?',
        _ => return None,
    })
}

/// [`KeyCode`] → JSON `keys` 表里的键名（与 Rust 枚举名一致）。
///
/// 逐键覆盖表（`keys`）用它做键；方案的几何里更推荐写单字符（见 [`parse_code`]）。
pub fn code_to_name(code: KeyCode) -> &'static str {
    match code {
        KeyCode::Backspace => "Backspace",
        KeyCode::Tab => "Tab",
        KeyCode::CapsLock => "CapsLock",
        KeyCode::Enter => "Enter",
        KeyCode::Space => "Space",
        KeyCode::Esc => "Esc",
        KeyCode::LShift => "LShift",
        KeyCode::RShift => "RShift",
        KeyCode::LCtrl => "LCtrl",
        KeyCode::RCtrl => "RCtrl",
        KeyCode::LAlt => "LAlt",
        KeyCode::RAlt => "RAlt",
        KeyCode::LWin => "LWin",
        KeyCode::RWin => "RWin",
        KeyCode::Menu => "Menu",
        KeyCode::PrtSc => "PrtSc",
        KeyCode::ScrollLock => "ScrollLock",
        KeyCode::Pause => "Pause",
        KeyCode::Insert => "Insert",
        KeyCode::Delete => "Delete",
        KeyCode::Home => "Home",
        KeyCode::End => "End",
        KeyCode::PageUp => "PageUp",
        KeyCode::PageDown => "PageDown",
        KeyCode::Up => "Up",
        KeyCode::Down => "Down",
        KeyCode::Left => "Left",
        KeyCode::Right => "Right",
        KeyCode::NumLock => "NumLock",
        KeyCode::NumDiv => "NumDiv",
        KeyCode::NumMul => "NumMul",
        KeyCode::NumSub => "NumSub",
        KeyCode::NumAdd => "NumAdd",
        KeyCode::NumDot => "NumDot",
        KeyCode::NumEnter => "NumEnter",
        KeyCode::MouseLeft => "MouseLeft",
        KeyCode::MouseRight => "MouseRight",
        KeyCode::MouseMiddle => "MouseMiddle",
        KeyCode::MouseX1 => "MouseX1",
        KeyCode::MouseX2 => "MouseX2",
        KeyCode::WheelUp => "WheelUp",
        KeyCode::WheelDown => "WheelDown",
        // 编译期无法把 u8 转 &'static str，但编号键数量极少，用 match 穷举即可。
        KeyCode::Num(n) => match n {
            0 => "Num0",
            1 => "Num1",
            2 => "Num2",
            3 => "Num3",
            4 => "Num4",
            5 => "Num5",
            6 => "Num6",
            7 => "Num7",
            8 => "Num8",
            _ => "Num9",
        },
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
        KeyCode::Char(c) => {
            if c == ' ' {
                return "Space";
            }
            if c.is_ascii_lowercase() {
                const CHAR_NAMES: [&str; 26] = [
                    "CharA", "CharB", "CharC", "CharD", "CharE", "CharF", "CharG", "CharH",
                    "CharI", "CharJ", "CharK", "CharL", "CharM", "CharN", "CharO", "CharP",
                    "CharQ", "CharR", "CharS", "CharT", "CharU", "CharV", "CharW", "CharX",
                    "CharY", "CharZ",
                ];
                return CHAR_NAMES[(c as u8 - b'a') as usize];
            }
            if c.is_ascii_digit() {
                const DIGIT_NAMES: [&str; 10] = [
                    "Char0", "Char1", "Char2", "Char3", "Char4", "Char5", "Char6", "Char7",
                    "Char8", "Char9",
                ];
                return DIGIT_NAMES[(c as u8 - b'0') as usize];
            }
            match c {
                '`' => "SymBacktick",
                '-' => "SymMinus",
                '=' => "SymEquals",
                '[' => "SymLBracket",
                ']' => "SymRBracket",
                '\\' => "SymBackslash",
                ';' => "SymSemicolon",
                '\'' => "SymQuote",
                ',' => "SymComma",
                '.' => "SymPeriod",
                '/' => "SymSlash",
                '~' => "SymTilde",
                '!' => "SymExcl",
                '@' => "SymAt",
                '#' => "SymHash",
                '$' => "SymDollar",
                '%' => "SymPercent",
                '^' => "SymCaret",
                '&' => "SymAmp",
                '*' => "SymStar",
                '(' => "SymLParen",
                ')' => "SymRParen",
                '_' => "SymUnderscore",
                '+' => "SymPlus",
                '{' => "SymLBrace",
                '}' => "SymRBrace",
                '|' => "SymPipe",
                ':' => "SymColon",
                '"' => "SymDQuote",
                '<' => "SymLAngle",
                '>' => "SymRAngle",
                '?' => "SymQuestion",
                _ => "SymOther",
            }
        }
        KeyCode::VolumeUp => "VolumeUp",
        KeyCode::VolumeDown => "VolumeDown",
        KeyCode::VolumeMute => "VolumeMute",
        KeyCode::MediaNext => "MediaNext",
        KeyCode::MediaPrev => "MediaPrev",
        KeyCode::MediaStop => "MediaStop",
        KeyCode::MediaPlay => "MediaPlay",
        KeyCode::BrowserBack => "BrowserBack",
        KeyCode::BrowserForward => "BrowserForward",
        KeyCode::BrowserRefresh => "BrowserRefresh",
        KeyCode::BrowserHome => "BrowserHome",
        KeyCode::Oem(_) => "Oem",
        KeyCode::Unknown(_) => "Unknown",
    }
}

// ─── 命中与图例 ───

/// 该键位是否对应这次按下的键。
///
/// 字符键要特殊处理：钩子给的 [`KeyCode::Char`] 是**已经按 Shift 修正过**的字符
/// （Shift+1 给 `'!'`，Shift+Q 给 `'Q'`），而布局里存的是物理键位（"1" / "q"）。
/// 所以除了大小写不敏感地比 `base`，还要比 `shift`。
pub fn hits(kd: &KeyDef, code: KeyCode) -> bool {
    match (kd.code, code) {
        (KeyCode::Char(a), KeyCode::Char(b)) => {
            a.eq_ignore_ascii_case(&b) || kd.base.starts_with(b) || kd.shift.starts_with(b)
        }
        (a, b) => a == b,
    }
}

/// 键帽上该显示的字。
///
/// ★ 这是**图例的唯一真相来源**（`vk_config::VkConfig::resolve_label` 直接调它）。
/// 曾经有第二条通路（`vk_config` 里的 `shift_char` 数字行映射表）与它并存，
/// 结果是布局表里精心写的 `shift` 层从来没被画出来过。
///
/// CapsLock 与 Shift 是**异或**关系（同类实现里一行公式就能表达）：
/// 字母受 Caps 影响（`caps=true`）时两者同状态反而回到下档，数字/符号只看 Shift。
///
/// 字母键的 base/shift 是**小写/大写**（`"q"` / `"Q"`）—— 真键盘键帽的印法。
pub fn legend(kd: &KeyDef, shift: bool, caps: bool) -> &str {
    let upper = if kd.caps { caps ^ shift } else { shift };
    if upper {
        &kd.shift
    } else {
        &kd.base
    }
}

/// 布局的几何尺寸（单位 u）。
pub struct Metrics {
    pub w: f32,
    pub h: f32,
}

/// 算一套布局的总宽与总高（单位 u）。
///
/// `h` 不是"行数"：小键盘的 `+` / `Enter` 是竖着占两行的（`h = 2`），它们把下面那行
/// 也吃掉了，所以高要按 `行号 + 该行最高键` 取最大值——简单累加会把 6 行算成 8u，
/// 整块键盘凭空高出两行。
pub fn metrics(l: &Layout) -> Metrics {
    let mut w = 0.0f32;
    let mut h = 0.0f32;
    for (r, row) in l.rows.iter().enumerate() {
        let mut x = 0.0f32;
        let mut row_h = 1.0f32;
        for k in &row.keys {
            x += k.pad + k.w;
            row_h = row_h.max(k.h);
        }
        w = w.max(x);
        h = h.max(r as f32 + row_h);
    }
    Metrics { w, h }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 出厂方案（内嵌 JSON 解析）。
    fn l104() -> Layout {
        builtin_layout("ansi_104").expect("内嵌的 104 方案必须能解析")
    }
    fn l60() -> Layout {
        builtin_layout("ansi_60").expect("内嵌的 60 方案必须能解析")
    }

    /// 一行的右边缘（单位 u）。`pad` 也是占位，故一并累加。
    fn row_end(row: &Row) -> f32 {
        row.keys.iter().fold(0.0f32, |x, k| x + k.pad + k.w)
    }

    /// 某个键在自己那行的起始 x（单位 u）。
    fn key_x(row: &Row, want: KeyCode) -> Option<f32> {
        let mut x = 0.0f32;
        for k in &row.keys {
            x += k.pad;
            if k.code == want {
                return Some(x);
            }
            x += k.w;
        }
        None
    }

    fn key_count(l: &Layout) -> usize {
        l.rows.iter().map(|r| r.keys.len()).sum()
    }

    #[test]
    fn builtin_schemes_parse_with_the_expected_shape() {
        let a = l104();
        assert_eq!(key_count(&a), 104, "104 方案应恰好 104 个键位");
        assert_eq!(a.rows.len(), 6);
        let m = metrics(&a);
        assert!((m.w - 23.0).abs() < 0.001, "104 总宽 {}", m.w);
        assert_eq!(m.h, 6.0);

        let b = l60();
        assert_eq!(key_count(&b), 61, "60% 方案应恰好 61 个键位");
        assert_eq!(b.rows.len(), 5);
        let m60 = metrics(&b);
        assert!((m60.w - 15.0).abs() < 0.001, "60% 总宽 {}", m60.w);
        assert_eq!(m60.h, 5.0);
    }

    #[test]
    fn no_row_is_wider_than_the_layout() {
        // 主区 15u + 0.5u 分栏 + 导航区 3u + 0.5u 分栏 + 小键盘 4u = 23.0u。
        //
        // 注意 F 行只有 18.5u——真实 104 键盘的功能键行也只到导航区上方，小键盘上方
        // 是空的（PrtSc/ScrLk/Pause 在 Ins/Home/PgUp 正上方）。所以这里锁的是
        // 「没有行超宽 + 最宽那行正好 23.0u」，而不是「每行都 23.0u」。
        //
        // ★ 小键盘整列从 19.5u 左移到 19.0u（NumLock 的 pad 由 1.0 收到 0.5），让
        // PgUp→Num 的间距与 Backspace→Ins 一致（都是 0.5u）；最宽那行因此由 23.5u 降为 23.0u。
        let a = l104();
        let widths: Vec<f32> = a.rows.iter().map(row_end).collect();
        let widest = widths.iter().cloned().fold(0.0f32, f32::max);
        assert!(
            (widest - 23.0).abs() < 0.001,
            "104 最宽的行应为 23.0u，实际 {widest}（各行 {widths:?}）"
        );
        for (i, w) in widths.iter().enumerate() {
            assert!(*w <= 23.0 + 0.001, "104 第 {i} 行 {w}u 超宽");
        }
    }

    #[test]
    fn numpad_column_aligns_across_rows() {
        // 小键盘五行的首键必须落在同一条竖线上，否则画出来就是错位的阶梯。
        let a = l104();
        let col = |r: usize, c: KeyCode| {
            key_x(&a.rows[r], c).unwrap_or_else(|| panic!("第 {r} 行应含 {c:?}"))
        };
        let x1 = col(1, KeyCode::NumLock);
        assert_eq!(col(2, KeyCode::Num(7)), x1);
        assert_eq!(col(3, KeyCode::Num(4)), x1);
        assert_eq!(col(4, KeyCode::Num(1)), x1);
        assert_eq!(col(5, KeyCode::Num(0)), x1);
        assert!((x1 - 19.0).abs() < 0.001, "小键盘列应在 19.0u，实际 {x1}");
    }

    #[test]
    fn every_row_of_60_is_exactly_15u_wide() {
        for (i, row) in l60().rows.iter().enumerate() {
            assert!(
                (row_end(row) - 15.0).abs() < 0.001,
                "60% 布局第 {i} 行宽 {}，应为 15u",
                row_end(row)
            );
        }
    }

    #[test]
    fn char_keys_match_both_shift_layers() {
        let a = l104();
        let one = a.rows[1]
            .keys
            .iter()
            .find(|k| k.base == "1")
            .expect("应有 1 键");
        assert!(hits(one, KeyCode::Char('1')));
        // Shift+1 → 钩子给的是 '!'
        assert!(hits(one, KeyCode::Char('!')));
        assert!(!hits(one, KeyCode::Char('2')));

        let q = a.rows[2]
            .keys
            .iter()
            .find(|k| k.base == "q")
            .expect("应有 Q 键");
        assert!(hits(q, KeyCode::Char('q')));
        assert!(hits(q, KeyCode::Char('Q')));
        assert!(!hits(q, KeyCode::Char('w')));
    }

    #[test]
    fn modifier_keys_are_left_right_distinct() {
        let a = l104();
        let l = a.rows[5]
            .keys
            .iter()
            .find(|k| k.code == KeyCode::LCtrl)
            .expect("应有左 Ctrl");
        assert!(hits(l, KeyCode::LCtrl));
        assert!(!hits(l, KeyCode::RCtrl));
    }

    #[test]
    fn caps_and_shift_are_exclusive_or_for_letters_only() {
        let a = l104();
        let ka = a.rows[3]
            .keys
            .iter()
            .find(|k| k.base == "a")
            .expect("应有 A 键");
        // 字母键帽按真键盘印法：base = 小写，shift = 大写。
        // 都不按 → 小写；只按一个 → 大写；都按 → 又回到小写。
        assert_eq!(legend(ka, false, false), "a");
        assert_eq!(legend(ka, true, false), "A", "Shift 按下该图上档（大写）");
        assert_eq!(legend(ka, false, true), "A", "Caps 开该图上档（大写）");
        assert_eq!(legend(ka, true, true), "a", "Caps + Shift 异或回小写");

        let one = a.rows[1]
            .keys
            .iter()
            .find(|k| k.base == "1")
            .expect("应有 1 键");
        assert_eq!(legend(one, false, false), "1");
        assert_eq!(legend(one, true, false), "!");
        assert_eq!(legend(one, false, true), "1", "符号键不受 Caps 影响");
    }

    /// ★ PrtSc / ScrLk / Pause 必须分别正对 Ins / Home / PgUp（真键盘的排布）。
    ///
    /// 回归锚点：`PrtSc.pad` 曾写成 1.0，F 行累计到 16.0u，而数字行的 Ins 在 15.5u，
    /// 三颗系统键整体右移 0.5u（默认 34px 键帽 + 3px 空隙 ≈ 18.5 逻辑 px，肉眼可见）。
    #[test]
    fn prtsc_aligns_with_insert() {
        let a = l104();
        let col = |r: usize, c: KeyCode| {
            key_x(&a.rows[r], c).unwrap_or_else(|| panic!("第 {r} 行应含 {c:?}"))
        };
        assert_eq!(
            col(0, KeyCode::PrtSc),
            col(1, KeyCode::Insert),
            "PrtSc 必须与 Ins 左对齐"
        );
        assert_eq!(
            col(0, KeyCode::ScrollLock),
            col(1, KeyCode::Home),
            "ScrLk 必须与 Home 左对齐"
        );
        assert_eq!(
            col(0, KeyCode::Pause),
            col(1, KeyCode::PageUp),
            "Pause 必须与 PgUp 左对齐"
        );
        assert_eq!(col(0, KeyCode::PrtSc), 15.5, "系统键列应在 15.5u");
    }

    /// ★ 游戏 WASD 板（2026-09-26 新增，同日改版）：7 颗键（WASD + 空格在左，
    ///   鼠标左/右键**并排**在右），2 行、最宽 5.5u；鼠标键码要在 JSON 写法
    ///   与键名往返之间闭环。
    ///
    /// ★★ 改版的判据（2026-09-26 用户报"左键和右键是鼠标的，应该平行，和 W 同一行"）：
    ///   旧版把鼠标两键排成**竖着的一列**（右键在上行、左键在下行），同一只鼠标的两个键
    ///   被拆到两行，看着像两件事；现在它们并排、且与 W 同一行。
    #[test]
    fn wasd_gaming_scheme_shape() {
        let g = builtin_layout("wasd_gaming").expect("游戏板必须能解析");
        assert_eq!(key_count(&g), 7);
        assert_eq!(g.rows.len(), 2);

        let codes: Vec<KeyCode> = g
            .rows
            .iter()
            .flat_map(|r| r.keys.iter())
            .map(|k| k.code)
            .collect();
        for want in [
            KeyCode::Char('w'),
            KeyCode::Char('a'),
            KeyCode::Char('s'),
            KeyCode::Char('d'),
            KeyCode::Space,
            KeyCode::MouseLeft,
            KeyCode::MouseRight,
        ] {
            assert!(codes.contains(&want), "游戏板缺 {want:?}");
        }

        let m = metrics(&g);
        assert!((m.w - 5.5).abs() < 0.001, "游戏板总宽 {}", m.w);
        assert_eq!(m.h, 2.0);

        // ★ 鼠标两键**并排在同一行**（左键在左、右键在右，与物理鼠标同向），且这一行
        //   就是 W 所在的行 —— 这是 2026-09-26 用户定的排法：一只鼠标的两个键要平行。
        let r1 = &g.rows[0];
        let r2 = &g.rows[1];
        let mouse_row = r1
            .keys
            .iter()
            .position(|k| k.code == KeyCode::MouseLeft)
            .expect("左键应落在第 1 行");
        assert_eq!(
            r1.keys[mouse_row + 1].code,
            KeyCode::MouseRight,
            "右键必须紧挨左键、与之同行"
        );
        assert!(
            r1.keys.iter().any(|k| k.code == KeyCode::Char('w')),
            "鼠标两键要与 W 同一行"
        );
        assert!(
            !r2.keys.iter().any(|k| {
                matches!(
                    k.code,
                    KeyCode::MouseLeft | KeyCode::MouseRight | KeyCode::MouseMiddle
                )
            }),
            "第 2 行不该再有鼠标键 —— 旧版正是把左键沉到了这一行"
        );

        // ★ 鼠标键帽上的文字必须与「击键回显」同一套缩写（LMB / RMB），
        //   来源是 `KeyCode::display_name` —— 2026-09-26 用户指出写中文"左键/右键"
        //   与击键回显的 LMB/RMB 不一致。写成中文这里会红。
        let text_of = |code: KeyCode| {
            g.rows
                .iter()
                .flat_map(|r| r.keys.iter())
                .find(|k| k.code == code)
                .map(|k| k.base.as_str())
                .unwrap_or("")
        };
        assert_eq!(text_of(KeyCode::MouseLeft), "LMB");
        assert_eq!(text_of(KeyCode::MouseRight), "RMB");

        // 两行右缘对齐（键盘区在左、鼠标区贴右缘）。
        let right_edge = |row: &Row| row.keys.iter().map(|k| k.pad + k.w).sum::<f32>();
        assert!(
            (right_edge(r1) - right_edge(r2)).abs() < 0.001,
            "两行右缘要对齐：{} vs {}",
            right_edge(r1),
            right_edge(r2)
        );
        // W 必须正落在 S 上方（真 WASD 配列的手感：W/S 同一竖列）。
        assert_eq!(
            key_x(r1, KeyCode::Char('w')),
            key_x(r2, KeyCode::Char('s')),
            "W 应在 S 正上方"
        );

        // 鼠标键在"键码 → 名字 → 键码"的往返里不丢（keys 表/逐键覆盖都靠这个名字）。
        for k in g.rows.iter().flat_map(|r| r.keys.iter()) {
            assert_eq!(
                parse_code(code_to_name(k.code)),
                Some(k.code),
                "{:?} 的键码名字无法往返",
                k.code
            );
        }
    }

    /// 出厂方案的 `name` 必须写出真实键数（规范命名，不出现"紧凑"这类模糊叫法）。
    #[test]
    fn builtin_labels_carry_the_real_key_count() {
        for (id, text) in BUILTIN {
            let json: LayoutJson = serde_json::from_str(text).expect("内嵌方案必须能解析");
            let label = name_of(text).expect("内嵌方案必须有 name");
            // 纯样式方案（如 ansi_104_dark）没有自己的几何，键数取 `baseLayout` 指向的方案。
            let n = if json.rows.is_empty() {
                let base = json.base_layout.as_deref().unwrap_or("ansi_104");
                key_count(&builtin_layout(base).expect("base 方案必须可解析"))
            } else {
                key_count(&builtin_layout(id).expect("内嵌方案必须可解析"))
            };
            if *id == "ansi_60" {
                // ★ 60% 方案按规范命名为 "ANSI 60"（指 60% 配列尺寸，不是字面键数）：
                // 它物理上是 61 个键位，但"60"是形态名，不写真实键数（见需求 4）。
                assert_eq!(label, "ANSI 60", "60% 方案按规范命名为 ANSI 60");
            } else if *id == "wasd_gaming" {
                // 迷你游戏板不是 ANSI 配列，走自己的命名（2026-09-26 用户定名 "WASD"）。
                assert_eq!(label, "WASD");
            } else {
                assert!(
                    label.contains(&n.to_string()),
                    "标签 {label:?} 里应写出实测键数 {n}"
                );
            }
            assert!(!label.contains("紧凑"), "标签 {label:?} 不该再用模糊叫法");
            // 游戏 WASD 板不是 ANSI 配列，不受"ANSI 前缀"约束。
            if *id != "wasd_gaming" {
                assert!(label.starts_with("ANSI"), "标签 {label:?} 应以标准名开头");
            }
        }
    }

    /// ★ 键码的三种写法都要能解析，且 `code_to_name` 的输出能原样转回来。
    ///
    /// 这是"方案 JSON 由人来写"的守门测试：写错了想当场报错，而不是静默丢一颗键。
    #[test]
    fn code_parsing_accepts_all_three_spellings_and_round_trips() {
        assert_eq!(parse_code("q"), Some(KeyCode::Char('q')));
        assert_eq!(parse_code("'"), Some(KeyCode::Char('\'')));
        assert_eq!(parse_code("\\"), Some(KeyCode::Char('\\')));
        assert_eq!(parse_code(";"), Some(KeyCode::Char(';')));
        assert_eq!(parse_code("Esc"), Some(KeyCode::Esc));
        assert_eq!(parse_code("LShift"), Some(KeyCode::LShift));
        assert_eq!(parse_code("F5"), Some(KeyCode::F(5)));
        assert_eq!(parse_code("Num3"), Some(KeyCode::Num(3)));
        assert_eq!(parse_code("Space"), Some(KeyCode::Space));
        // 旧写法兼容
        assert_eq!(parse_code("CharQ"), Some(KeyCode::Char('q')));
        assert_eq!(parse_code("SymSemicolon"), Some(KeyCode::Char(';')));
        // 不认识的必须报空，不能悄悄变成某个键
        assert_eq!(parse_code("Nope"), None);
        assert_eq!(parse_code("F99"), None);

        // 出厂方案里每一颗键都要能往返
        for id in ["ansi_104", "ansi_60"] {
            let lay = builtin_layout(id).expect("内嵌方案必须可解析");
            for row in &lay.rows {
                for k in &row.keys {
                    assert_eq!(
                        parse_code(code_to_name(k.code)),
                        Some(k.code),
                        "{id} 里 {:?} 的键码名字无法往返",
                        k.code
                    );
                }
            }
        }
    }

    /// 方案目录里出现什么，`discover` 就该报出什么（根目录 `*.json` 即来源）。
    #[test]
    fn discover_sorts_schemes_by_file_name() {
        let dir = std::env::temp_dir().join("curseen_vk_discover_test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("建目录失败");
        // ★ 方案就是顶层的一个 JSON 文件（文件名即 id），不再套子目录。
        // 与出厂同名 → 用目录那份的显示名。
        fs::write(dir.join("ansi_104.json"), r#"{"name":"我的 104"}"#).expect("写方案失败");
        fs::write(
            dir.join("my_board.json"),
            r#"{"name":"WASD 小板","rows":[{"keys":[{"code":"w"},{"code":"a"}]}]}"#,
        )
        .expect("写方案失败");

        let list = discover(&dir);
        let ids: Vec<&str> = list.iter().map(|e| e.id.as_str()).collect();
        // ★ 按文件名排序：`ansi_104_dark` 紧跟在它改的 `ansi_104` 后面（用户要求）；
        //   出厂的 `wasd_gaming` 与目录新增的 `my_board` 也按字母序混排 —— 不做出厂优先。
        assert_eq!(
            ids,
            vec![
                "ansi_104",
                "ansi_104_dark",
                "ansi_60",
                "my_board",
                "wasd_gaming"
            ],
            "{ids:?}"
        );
        assert_eq!(list[0].label, "我的 104", "同名时目录那份的显示名优先");
        assert_eq!(list[3].label, "WASD 小板");
        assert_eq!(list[4].label, "WASD");

        let _ = fs::remove_dir_all(&dir);
    }

    /// 用户方案里的几何要能完整落地（含 `shiftText` 缺省 = `text`）。
    #[test]
    fn user_scheme_geometry_round_trips() {
        let json: LayoutJson = serde_json::from_str(
            r#"{
                "name": "小板",
                "rows": [
                    { "keys": [
                        {"code": "q", "text": "Q!", "shiftText": "Q@", "caps": true},
                        {"code": "w", "w": 2.5, "pad": 0.5, "h": 2.0},
                        {"code": "F5", "text": "F5"},
                        {"code": "Num3"}
                    ]}
                ]
            }"#,
        )
        .expect("方案 JSON 应能解析");
        let lay = layout_from_json("小板", "小板", &json).expect("几何应能构造");
        let keys = &lay.rows[0].keys;
        assert_eq!(keys.len(), 4);
        assert_eq!(keys[0].base, "Q!");
        assert_eq!(keys[0].shift, "Q@");
        assert!(keys[0].caps);
        // 省略 shiftText → 与 text 相同（缺 text 时两者都是空串）
        assert_eq!(keys[1].base, "");
        assert_eq!(keys[1].shift, "");
        assert_eq!(keys[1].w, 2.5);
        assert_eq!(keys[1].pad, 0.5);
        assert_eq!(keys[1].h, 2.0);
        assert_eq!(keys[2].code, KeyCode::F(5));
        assert_eq!(keys[3].code, KeyCode::Num(3));
        assert_eq!(
            metrics(&lay).w,
            1.0 + 0.5 + 2.5 + 1.0 + 1.0,
            "总宽按 pad/w 累加"
        );
    }

    /// 坏方案要**报错**而不是给出半截布局（调用方据此回落内置）。
    #[test]
    fn broken_scheme_is_rejected() {
        let empty: LayoutJson = serde_json::from_str(r#"{"name":"空"}"#).expect("应能解析");
        assert!(
            layout_from_json("空", "空", &empty).is_err(),
            "没有 rows 应报错"
        );

        let bad: LayoutJson =
            serde_json::from_str(r#"{"rows":[{"keys":[{"code":"Nope"}]}]}"#).expect("应能解析");
        let err = layout_from_json("坏", "坏", &bad).expect_err("坏键码应报错");
        assert!(err.contains("Nope"), "错误信息该指出是哪个键码：{err}");
    }
}
