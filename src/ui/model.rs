//! 控制面板的绑定模型：每个控件一个 windui `Signal`（Copy 句柄）。
//!
//! 面板打开时由 [`PanelModel::from_config`] 用当前 `Config` 初始化全部信号，并把模型交给
//! `Runtime`；此后覆盖层每帧调 [`PanelModel::apply_to`] 把信号 diff 回 `Config`（改了才写、
//! 才 bump `config_version` 让各模块重新同步），并按防抖落盘。
//!
//! 之所以走"每帧 diff"而不是逐个控件挂回调：`Element::slider / dropdown / color_picker`
//! 只更新绑定的 `Signal`，**不暴露 `on_change`**。diff 是唯一能覆盖全部控件形态的做法，
//! 也顺带让"改哪个写哪个"变得天然（没变的字段原样保留）。

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use serde_json::json;
use serde_json::Value;
use windui::geometry::Color;
use windui::prelude::*;

use crate::core::color::Argb;
use crate::core::config::{
    Config, ParamValue, FULLSCREEN_ZOOM_MAX, FULLSCREEN_ZOOM_MIN, NOTE_ALPHA_MAX, NOTE_ALPHA_MIN,
    NOTE_FONT_MAX, NOTE_FONT_MIN, PIP_PANEL_MAX, PIP_PANEL_MIN, PIP_ZOOM_MAX, PIP_ZOOM_MIN,
    TIMER_DURATION_MAX, TIMER_DURATION_MIN, TIMER_FONT_MAX, TIMER_FONT_MIN, TIMER_OFFSET_MAX,
    TIMER_OFFSET_MIN,
};
use crate::modules::virtual_keyboard::layout::SchemeEntry;
use crate::modules::virtual_keyboard::vk_config::{ControlDef, ControlKind};
use crate::platform::audio_capture::DefaultDevice;
use crate::scripting::ParamDef;
use crate::ui::effects::EffectCatalog;
use crate::ui::sfx::SoundCatalog;

/// 击键音效脚本目录（`assets/lua` 里 `role="keysound"` 的脚本）。
///
/// 迁移到统一 Lua 框架后不再扫 `assets/KeySounds/`，而是复用统一注册表里
/// `role="keysound"` 的脚本：下拉显示名 = 脚本 `name`，id = 相对 `assets/lua/`
/// 的相对路径（含子目录），schema 仍由 `LuaExtRegistry` 解析。
#[derive(Clone)]
pub struct KeysoundCatalog {
    /// 与 `names` 同序的配置 id（脚本相对路径）。
    ids: Rc<RefCell<Vec<String>>>,
    /// 显示名（脚本 `name`），绑到下拉控件。
    pub names: Signal<Vec<String>>,
    /// 每个 id 的参数声明（供动态 UI）。
    schemas: Rc<RefCell<HashMap<String, Vec<ParamDef>>>>,
}

impl KeysoundCatalog {
    /// 扫 `assets/lua` 中 `role="keysound"` 的脚本。
    pub fn from_lua() -> Self {
        let mut reg =
            crate::scripting::registry_ext::LuaExtRegistry::new(&crate::core::paths::lua_ext_dir());
        reg.scan();
        let mut ids = Vec::new();
        let mut names = Vec::new();
        let mut schemas = HashMap::new();
        for s in reg.role_of("keysound") {
            ids.push(s.id.clone());
            names.push(s.name.clone());
            schemas.insert(s.id.clone(), s.params.clone());
        }
        Self {
            ids: Rc::new(RefCell::new(ids)),
            names: signal(names),
            schemas: Rc::new(RefCell::new(schemas)),
        }
    }

    /// 重扫 `assets/lua` 里 `role="keysound"` 的脚本，并把新名字推给信号。
    pub fn refresh(&self) {
        let mut reg =
            crate::scripting::registry_ext::LuaExtRegistry::new(&crate::core::paths::lua_ext_dir());
        reg.scan();
        let mut ids = Vec::new();
        let mut names = Vec::new();
        let mut schemas = HashMap::new();
        for s in reg.role_of("keysound") {
            ids.push(s.id.clone());
            names.push(s.name.clone());
            schemas.insert(s.id.clone(), s.params.clone());
        }
        *self.ids.borrow_mut() = ids;
        *self.schemas.borrow_mut() = schemas;
        self.names.set(names);
    }

    /// 显示名下标 → 配置 id。越界时回退到第一个（保证永远写得出合法 id）。
    pub fn id_at(&self, idx: usize) -> String {
        self.ids.borrow().get(idx).cloned().unwrap_or_default()
    }

    /// 配置 id → 显示名下标。未知 id 回退 0。
    pub fn index_of(&self, id: &str) -> usize {
        self.ids.borrow().iter().position(|x| x == id).unwrap_or(0)
    }

    /// 取某脚本的参数声明（按 id）。不存在返回空。
    pub fn schema_for(&self, id: &str) -> Vec<ParamDef> {
        self.schemas.borrow().get(id).cloned().unwrap_or_default()
    }
}

// ─── 下拉选项表（显示名 ↔ 配置字符串）───

/// 显示内容模式。
pub const DISPLAY_MODES: &[&str] = &["all", "no_mouse", "shortcuts_fun", "shortcuts"];
pub const DISPLAY_MODES_LABELS: &[&str] = &["全部", "排除鼠标", "组合键 + 功能键", "组合键"];

/// 弹窗位置。
pub const PLACEMENTS: &[&str] = &["top_left", "top_right", "bottom_left", "bottom_right"];
pub const PLACEMENTS_LABELS: &[&str] = &["左上", "右上", "左下", "右下"];

/// 笔迹线型。
pub const PEN_STYLES: &[&str] = &["solid", "dashed", "dotted", "dash_dot"];
pub const PEN_STYLES_LABELS: &[&str] = &["实线", "虚线", "点线", "点划线"];

// ─── 数值映射助手（slider 恒为 0..=1，配置里是真实量纲）───

#[inline]
fn s2v(s: f32, min: f32, max: f32) -> f32 {
    min + s * (max - min)
}

#[inline]
pub(crate) fn v2s(v: f32, min: f32, max: f32) -> f32 {
    if (max - min).abs() < f32::EPSILON {
        0.0
    } else {
        ((v - min) / (max - min)).clamp(0.0, 1.0)
    }
}

/// 显示小数位：由脚本 schema 的 `step` 推出。
///
/// 步长 0.2（秒类参数）→ 1 位：读数显示「1.2」「3.0」，不显示无意义的第二位。
/// 步长 ≥1 → 0 位；步长 0.01（不透明度）→ 2 位，与既有观感一致。
pub(crate) fn decimals_for_step(step: f32) -> u8 {
    if !step.is_finite() || step <= 0.0 {
        return 2;
    }
    if step >= 1.0 {
        0
    } else if step >= 0.1 {
        1
    } else if step >= 0.01 {
        2
    } else {
        3
    }
}

/// 把真实值吸附到「min + n×step」的网格上，并按显示位数抹掉浮点尾数。
///
/// ★ 为什么必须有这一步：windui 的滑块内部恒存 0..=1 的**连续**比例，脚本 schema 里
///   的 `step` 此前只被解析、**没有任何消费者**——拖出来的值可以是 1.37 秒这种
///   「既读不准也存不整」的数。秒类参数要求 0.2 秒一档，就得在这里收口。
///
/// 吸附后 `apply_to` 会把比例回写滑块（`sig.set`），让**读数与落盘值同源**——
/// 否则会出现"面板显示 1.37、实际按 1.4 生效"的错位。
pub(crate) fn quantize(v: f32, min: f32, max: f32, step: f32, decimals: u8) -> f32 {
    let span = max - min;
    let mut out = if step.is_finite() && step > 0.0 && span.abs() > f32::EPSILON {
        let n = ((v - min) / step).round();
        (min + n * step).clamp(min, max)
    } else {
        v.clamp(min, max)
    };
    let p = 10f32.powi(decimals as i32);
    out = (out * p).round() / p;
    out.clamp(min, max)
}

/// 把一个参数单元的当前值 diff 进它自己的参数表，返回"有没有改动"。
///
/// ★★ 语义：**配置与面板对账**——配置现值 ≠ 面板现值就写成面板值。唯一的例外是
///   「配置里还没有这个键 && 用户没动过控件」：这时不落盘（脚本侧有同值默认），
///   保住"打开面板什么都不动 ≠ 写配置"的既有承诺（`model_no_spurious_change` 钉着）。
///
/// ★★ **绝不能"回到 `initial()` 就把键摘掉"**（2026-09-25 第二版修复踩的坑）：
///   `initial()` 可能来自**配置现值**而不是脚本 schema 默认。两者不等时摘键 =
///   让脚本悄悄跳回默认值。实录：用户先取消细分刻度（config=false），再勾选（config
///   变 true），再取消 → new==initial(false) → 摘键 → 脚本默认 **true** 接管
///   → 细分刻度继续画 → 用户看到的就是"取消勾选没反应"。写回初值本身才是对的。
/// `key` 是这条参数在**该张表**里的键：拖尾特效是 `{特效id}.{key}`（与其它特效共用一张
/// 表，必须带前缀），悬浮标 / Lua 扩展是各类型一张独立表，直接用 `cell.key()`。
fn apply_param_value(
    stored: &mut HashMap<String, Value>,
    cell: &EffectParamCell,
    new_val: Value,
    key: &str,
) -> bool {
    if stored.get(key) == Some(&new_val) {
        return false; // 配置已经是这个值，一个字节都不用动
    }
    if new_val == *cell.initial() && stored.get(key).is_none() {
        return false; // 配置无键且没动过（初值即脚本默认）：保持不落盘
    }
    stored.insert(key.to_string(), new_val);
    true
}

/// f32 → 按显示位数取整后的 `Value`（落盘用）。
///
/// ★ 不做这一步，1.4 会落成 `1.399999976158142`（f32 尾数放大到 f64）：配置难读不说，
///   下次"变没变"的比对也拿它跟 1.4 比，平白多一次伪落盘。
fn json_decimal(v: f32, decimals: u8) -> Value {
    let p = 10f64.powi(decimals as i32);
    json!(((v as f64) * p).round() / p)
}

/// Float / Int 参数单元的落盘值：**滑条 + 输入框双入口三方对账**（2026-09-25，
/// 自 `float_cell_value` 升级——输入框是脚本参数的第二入口，见
/// [`card::slider_row`] 与 `build_param_cell`）。
///
/// 基准 = 配置现值 `stored[key]`，没有键时回退 `cell.initial()`（两者都正是信号
/// 初始化的来源）。谁与基准偏离谁赢：
///   · 滑条刚拖（滑条值 ≠ 基准）：值跟滑条（吸附 `step` 网格），文本回写成新值；
///   · 文本编辑成合法数字且 ≠ 滑条刻度（此时滑条值必 = 基准 ⇒ 滑条没动）：
///     值跟文本（同样吸附），滑条信号跳过去，文本**不回写**——"03" 这类前导零
///     中间态原样保留，不打断输入；
///   · 文本空串 / 非数字 / 溢出：打字中间态，忽略——值跟滑条，也**不**把文本
///     回写成旧值（清空重输不被同步进程打架）。
///
/// 返回吸附后的 f32，调用方按类型落盘（Int 取整 `json!`、Float 走
/// [`json_decimal`]）。吸附幂等：下一帧值已在网格上，不再回写，不与拖拽打架。
/// `key` 用**落盘键**（拖尾是 `{特效id}.{key}` 前缀键，其余直接 `cell.key()`），
/// 与调用方随后 `apply_param_value` 用的是同一个键。
// 9 参豁免：参数两两成对（信号对 sig+text / 量程对 min+max / 网格对 step+decimals）
// 且量程对必须与调用方初始化信号的那一对常量**字面同源**——拆成结构体反而把
// "这两对必须一起抄"的约束藏起来，代价大于函数签名长度。
#[allow(clippy::too_many_arguments)]
fn float_param_sync(
    stored: &HashMap<String, Value>,
    key: &str,
    cell: &EffectParamCell,
    sig: &Signal<f32>,
    text: &Signal<String>,
    min: f32,
    max: f32,
    step: f32,
    decimals: u8,
) -> f32 {
    let referee = stored
        .get(key)
        .cloned()
        .unwrap_or_else(|| cell.initial().clone());
    let ref_f = referee.as_f64().map(|f| f as f32);
    let raw = s2v(sig.get(), min, max);
    let sv = quantize(raw, min, max, step, decimals);
    let tv = text
        .get()
        .trim()
        .parse::<f64>()
        .ok()
        .map(|t| quantize(t as f32, min, max, step, decimals));
    let close = |a: f32, b: f32| (a - b).abs() <= 1e-4;
    let text_caused = match (tv, ref_f) {
        (Some(t), Some(r)) => !close(t, sv) && close(sv, r),
        _ => false,
    };
    let slider_caused = matches!(ref_f, Some(r) if !close(sv, r));
    // text_caused ⟹ tv 必为 Some（见上方判据），match 写法守住生产禁 unwrap 红线。
    let q = match tv {
        Some(t) if text_caused => t,
        _ => sv,
    };
    // 滑条读数与生效值**同源**（旧 `float_cell_value` 的既有承诺）：滑条原始位置
    // 不在 q 上——拖到两档之间要吸附，文本赢要跳过去——就回写滑条。幂等：回写后
    // 下一帧 raw == q，不再写。
    if !close(raw, q) {
        sig.set(v2s(q, min, max));
    }
    // 文本回写**仅在滑条真动了**（sv ≠ 基准）时发生：空串/前导零等打字中间态
    // 不被回写打架（清空重输是合法操作路径）。
    if slider_caused {
        let want = format!("{:.*}", decimals as usize, q);
        if text.get().trim() != want {
            text.set(want);
        }
    }
    q
}

/// 面板**固定参数**滑条的三方对账（2026-09-25 §35：全软件滑条统一三件套时引入，
/// 与 [`float_param_sync`] 同一套判据——唯一差别是基准直接给配置现值 `cur`，
/// 不走 `stored[key]` 参数表）。
///
/// 消费者是 [`PanelModel::apply_to`] 里每根「滑条 + 数值输入框」：滑条信号 `sig`
/// 装比例、镜像文本 `text` 装输入框原样字符串。谁与基准（配置现值）偏离谁赢：
///   · 滑条刚拖（滑条读数 ≠ 基准）：值跟滑条（吸附 `step`），文本回写成新值；
///   · 文本编辑成合法数字且 ≠ 滑条刻度（此时滑条值必 = 基准 ⇒ 滑条没动）：
///     值跟文本（同样吸附），滑条信号跳过去，文本**不回写**（前导零等打字
///     中间态原样保留，不打断输入）；
///   · 文本空串 / 非数字 / 溢出：打字中间态，忽略——值跟滑条，也**不**回写文本。
///
/// 返回吸附后的 f32，调用方按字段类型落盘（整数取整、浮点按各自精度 diff）。
/// 幂等：对账后滑条与文本都与返回值一致，下一帧不再产生任何写入——
/// `model_no_spurious_change` 钉着这条。
fn slider_pair_sync(
    sig: &Signal<f32>,
    text: &Signal<String>,
    cur: f32,
    min: f32,
    max: f32,
    step: f32,
    decimals: u8,
) -> f32 {
    let close = |a: f32, b: f32| (a - b).abs() <= 1e-4;
    let raw = s2v(sig.get(), min, max);
    let sv = quantize(raw, min, max, step, decimals);
    let tv = text
        .get()
        .trim()
        .parse::<f64>()
        .ok()
        .map(|t| quantize(t as f32, min, max, step, decimals));
    let text_caused = match tv {
        Some(t) => !close(t, sv) && close(sv, cur),
        None => false,
    };
    let slider_caused = !close(sv, cur);
    // text_caused ⟹ tv 必为 Some（见上方判据），match 写法守住生产禁 unwrap 红线。
    let q = match tv {
        Some(t) if text_caused => t,
        _ => sv,
    };
    // 滑条原始位置不在 q 上（两档之间 / 文本赢）就回写，让读数与生效值同源。
    if !close(raw, q) {
        sig.set(v2s(q, min, max));
    }
    // 文本回写**仅在滑条真动了**时发生：打字中间态不被回写打架（清空重输合法）。
    if slider_caused {
        let want = format!("{:.*}", decimals as usize, q);
        if text.get().trim() != want {
            text.set(want);
        }
    }
    q
}

// ─── 颜色助手 ───

fn hex_to_color(s: &str) -> Color {
    Argb::parse_hex(s)
        .map(|c| Color::rgba(c.r(), c.g(), c.b(), c.a()))
        .unwrap_or_else(|| Color::rgba(0, 0, 0, 255))
}

/// 两个颜色字符串是否表示同一颜色（忽略大小写与 `#RRGGBB`/`#RRGGBBAA` 写法差异）。
fn same_color(a: &str, b: &str) -> bool {
    match (Argb::parse_hex(a), Argb::parse_hex(b)) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

/// 把颜色格式化成与 `existing` 相同的位数：6 位（`#RRGGBB`）或 8 位（`#RRGGBBAA`）。
///
/// ★★ 8 位是 **R,G,B,A**（alpha 在末尾），与 windui `Color::to_hex_string`、
/// `config.json`、Lua 脚本同口径。早年这里是 `#AARRGGBB`，与取色器显示对不上，
/// 于是"打开面板什么都不动"也会被判成改过（2026-09-25 统一）。
///
/// 沿用原串位数可避免把配置里的 `#FFFF00` 无谓改写成 `#FFFF00FF`（颜色等价但
/// 字符串不同 → 每帧误报"改了"，反复落盘）。
fn fmt_like(existing: &str, c: Color) -> String {
    if existing.trim_start_matches('#').len() == 6 {
        format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b)
    } else {
        format!("#{:02X}{:02X}{:02X}{:02X}", c.r, c.g, c.b, c.a)
    }
}

/// 特效参数里的颜色**恒为 6 位 `#RRGGBB`**：不带透明度。
///
/// 透明度由外面那条「不透明度」参数统一管，彩虹与单色共用同一份——彩虹模式根本拿不到
/// 「固定色」的 alpha，两套透明度并存只会互相打架。相应地取色器也关掉了透明度条
/// （见 `pages::trail::build_param_cell`）。老配置里若存着 8 位 `#RRGGBBAA`，一律截断成
/// 6 位：颜色照旧，alpha 由参数接管。
fn fmt_rgb6(c: Color) -> String {
    format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b)
}

/// 颜色 → 字符串：位数由脚本声明的 `alpha` 决定（见 [`crate::scripting::ParamDef::alpha`]）。
///
/// ★ 与 `hex_to_color` 必须严格互逆，否则"打开面板什么都不动"也会被判成改过：
///   8 位分支写的是 `#RRGGBBAA`（与 `Argb::parse_hex`、windui 取色器同口径）。
fn fmt_color(c: Color, alpha: bool) -> String {
    if alpha {
        color_hex(c)
    } else {
        fmt_rgb6(c)
    }
}

fn array_to_color(a: [u8; 4]) -> Color {
    Color::rgba(a[0], a[1], a[2], a[3])
}

fn color_to_array(c: Color) -> [u8; 4] {
    [c.r, c.g, c.b, c.a]
}

/// `Color` → `"#RRGGBB"`（不透明）/ `"#RRGGBBAA"`（半透明）。
///
/// ★★ 与 windui `Color::to_hex_string` **逐字节同款**：alpha 在末尾，且 alpha 满
/// 值时写成 6 位。取色器显示什么、配置里就存什么——这是"打开面板不算改动"的前提。
/// 与 [`hex_to_color`] 严格互逆（都走 `Argb::parse_hex`）。
fn color_hex(c: Color) -> String {
    if c.a == 255 {
        format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b)
    } else {
        format!("#{:02X}{:02X}{:02X}{:02X}", c.r, c.g, c.b, c.a)
    }
}

/// 虚拟键盘控件信号的初值（用户改过的 → 面板全局配置 → 方案 → 出厂）。
fn control_signal(
    vc: &crate::modules::virtual_keyboard::vk_config::VkConfig,
    d: &ControlDef,
) -> VkControlSignal {
    let v = vc.value_of(d);
    match d.kind {
        ControlKind::Color => VkControlSignal::Color(signal(param_to_color(&v, &d.default))),
        ControlKind::Bool => VkControlSignal::Bool(signal(v.as_bool().unwrap_or(false))),
        // ★ 滑块里装的是 0..=1 的比例（`ControlKind::to_slider`），不是像素/秒。
        //   这里漏算一次的症状是：初值看起来对（滑到最右），一拖就变成 0.xx 存进配置。
        ControlKind::Range { min, decimals, .. } => {
            let real = v.as_f32().unwrap_or(min);
            // 镜像文本与滑块同一初值（同一格式，apply_to 的回写判据才认得
            // "这就是没动过"的样子）。
            VkControlSignal::Range(
                signal(d.kind.to_slider(real)),
                signal(format!("{:.*}", decimals as usize, real)),
            )
        }
    }
}

fn param_to_color(v: &ParamValue, fallback: &ParamValue) -> Color {
    let a = v
        .as_argb()
        .or_else(|| fallback.as_argb())
        .unwrap_or(Argb::new(255, 0, 0, 0));
    Color::rgba(a.r(), a.g(), a.b(), a.a())
}

/// "这个值算不算改过" —— 与方案默认值比较。
///
/// ★ 两种值都不能直接 `==`：
///   · 浮点：滑块回读带末位噪声，不该被当成"用户动过"；
///   · 颜色：面板回写的是取色器原样串（半透明 8 位 `#RRGGBBAA`），方案里常写
///     `#RRGGBB`（6 位）——比字符串会把"没动过"误判成"改过"，于是首帧就把默认值
///     写进配置（`model_no_spurious_change` 这类测试会立刻红）。
fn param_eq(a: &ParamValue, b: &ParamValue) -> bool {
    match (a, b) {
        (ParamValue::Float(x), ParamValue::Float(y)) => (x - y).abs() < 1e-3,
        (ParamValue::Color(x), ParamValue::Color(y)) => {
            match (Argb::parse_hex(x), Argb::parse_hex(y)) {
                (Some(x), Some(y)) => x == y,
                _ => x == y,
            }
        }
        _ => a == b,
    }
}

fn idx_of(list: &[&str], s: &str) -> usize {
    list.iter().position(|x| *x == s).unwrap_or(0)
}

fn pick(list: &[&str], idx: usize) -> String {
    list.get(idx.min(list.len().saturating_sub(1)))
        .copied()
        .unwrap_or_default()
        .to_string()
}

/// 方案 id → 下拉下标（找不到 = 0，即出厂第一套）。
///
/// 用户把方案目录删了、或换了一台机器时，配置里存的 id 可能已经不存在了——
/// 这时的行为是"回落第一套方案"，而不是留一个空的下拉。
fn idx_of_schemes(list: &[SchemeEntry], id: &str) -> usize {
    list.iter().position(|e| e.id == id).unwrap_or(0)
}

/// 下拉下标 → 方案 id。列表为空（连出厂内嵌都没有，理论上不可能）时回落出厂 104。
fn pick_scheme(list: &[SchemeEntry], idx: usize) -> String {
    list.get(idx)
        .map(|e| e.id.clone())
        .unwrap_or_else(|| "ansi_104".to_string())
}

/// 一个特效参数的动态 UI 单元。页面用 `Element::host_signal` 把这些单元渲染成对应控件，
/// `apply_to` 再把控件值写回配置。每个单元都记得自己的参数 key 与取值所需的范围/候选项，
/// 这样选中特效变化时旧单元被整批回收、新单元按新 schema 重建，互不串味。
#[derive(Clone)]
pub enum EffectParamCell {
    Float {
        key: String,
        label: String,
        sig: Signal<f32>,
        /// 输入框镜像文本（2026-09-25）：滑条之外的第二入口，补"滑条点不精确"的
        /// 精度短板。与 `sig` 绑同一份参数值，写回侧在 [`float_param_sync`] 对账。
        text: Signal<String>,
        min: f32,
        max: f32,
        /// 吸附步长（脚本 schema 的 `step`）。秒类参数用 0.2 这种小数网格。
        step: f32,
        /// 显示小数位（由 `step` 推出，见 [`decimals_for_step`]）。
        decimals: u8,
        init: Value,
    },
    Int {
        key: String,
        label: String,
        sig: Signal<f32>,
        /// 输入框镜像文本（2026-09-25）：同 [`EffectParamCell::Float`] 的 `text`。
        text: Signal<String>,
        min: f32,
        max: f32,
        init: Value,
    },
    Bool {
        key: String,
        label: String,
        sig: Signal<bool>,
        init: Value,
    },
    /// 下拉。`choices` 是**落盘与喂给脚本**的值，`labels` 是界面显示名（空则用 `choices`）。
    Choice {
        key: String,
        label: String,
        sig: Signal<usize>,
        choices: Vec<String>,
        labels: Vec<String>,
        init: Value,
    },
    /// ★ 音效下拉（击键音效「分类音效」用）：与 `Choice` 同控件，但 `choices`
    /// 不是脚本写死的，而由宿主按「音效目录」扫描得出（见 `sfx::sound_choices`）。
    Sound {
        key: String,
        label: String,
        sig: Signal<usize>,
        choices: Vec<String>,
        labels: Vec<String>,
        init: Value,
    },
    /// ★ 文件夹（击键音效「音效目录」行）：值 = 目录绝对路径，空串 = 出厂目录。
    /// 改了它，同一脚本里的 `Sound` 单元整批重建（下拉内容换一批）。
    Folder {
        key: String,
        label: String,
        sig: Signal<String>,
        init: Value,
    },
    /// ★ 颜色。`alpha` = 脚本有没有声明 `alpha = true`（取色器带透明度条）。
    /// 决定落盘位数：true → 8 位 `#RRGGBBAA`，false → 6 位 `#RRGGBB`。
    Color {
        key: String,
        label: String,
        sig: Signal<Color>,
        alpha: bool,
        init: Value,
    },
    /// 图片文件接口（悬浮标复用同一套脚本执行机制，支持用户导入自定义图片）。
    /// 本期仅契约 + 路径框，实际加载/绘制留待接入悬浮标时实现。
    Image {
        key: String,
        label: String,
        sig: Signal<String>,
        init: Value,
    },
    /// ★ 自由文本（Lua 扩展脚本的参数：api_key / 模型名 / 目录等）。值 = 输入原样字符串。
    Str {
        key: String,
        label: String,
        sig: Signal<String>,
        init: Value,
    },
    /// ★ 音频文件浏览（2026-09-25 二次改版）：值 = 路径（程序目录内记便携相对
    /// 路径，外部文件记绝对路径——浏览即引用，不复制、无中转目录）。
    File {
        key: String,
        label: String,
        sig: Signal<String>,
        init: Value,
    },
    /// ★ 快捷键捕获框（2026-09-29，`type = "hotkey"`）：与屏幕标注快捷键同一套
    /// 视觉与交互（`card::hotkey_row` / `hotkey_row_script`）。捕获态走
    /// [`PanelModel::script_hotkey_cap`]（不占用固定字段的 `hotkey_capture`——
    /// 那套的回写目标信号是编译期固定的，脚本参数的 sig 是运行期动态的）。
    Hotkey {
        key: String,
        label: String,
        sig: Signal<String>,
        init: Value,
    },
}

impl EffectParamCell {
    /// 参数键（写入配置时的 `{特效id}.{key}` 里的 key）。
    pub fn key(&self) -> &str {
        match self {
            EffectParamCell::Float { key, .. }
            | EffectParamCell::Int { key, .. }
            | EffectParamCell::Bool { key, .. }
            | EffectParamCell::Choice { key, .. }
            | EffectParamCell::Sound { key, .. }
            | EffectParamCell::Folder { key, .. }
            | EffectParamCell::Color { key, .. }
            | EffectParamCell::Image { key, .. }
            | EffectParamCell::Str { key, .. }
            | EffectParamCell::File { key, .. }
            | EffectParamCell::Hotkey { key, .. } => key,
        }
    }

    /// 初始有效值（配置现值优先、schema 默认兜底）。
    ///
    /// ★ `apply_to` 用它判"控件没动过就别写配置"——不能用纯 schema 默认值比：
    /// 旧配置字段（如 spawn_interval_ms=100）与新 schema 默认（20）不同时，
    /// 按 schema 默认比会每帧误报改动、反复落盘。
    pub fn initial(&self) -> &Value {
        match self {
            EffectParamCell::Float { init, .. }
            | EffectParamCell::Int { init, .. }
            | EffectParamCell::Bool { init, .. }
            | EffectParamCell::Choice { init, .. }
            | EffectParamCell::Sound { init, .. }
            | EffectParamCell::Folder { init, .. }
            | EffectParamCell::Color { init, .. }
            | EffectParamCell::Image { init, .. }
            | EffectParamCell::Str { init, .. }
            | EffectParamCell::File { init, .. }
            | EffectParamCell::Hotkey { init, .. } => init,
        }
    }
}

/// 把脚本 `schema()` 里的单条参数声明，连同它在当前配置里的取值，打包成一个 UI 单元。
///
/// `pm` 是已按特效 id 过滤好的「该特效自己的参数表」（命名空间前缀已剥掉），单元据此初始化
/// 初值：优先用配置里的值，否则回退脚本声明的默认值。
/// 见 [`cell_from_param_in`]（`sounds` 为空：拖尾/悬浮标脚本不发音效参数）。
fn cell_from_param(p: &ParamDef, pm: &HashMap<String, Value>) -> EffectParamCell {
    cell_from_param_in(p, pm, &[])
}

/// `sounds` = 音效下拉的候选项（宿主按「音效目录」扫描；见 `sfx::sound_choices`）。
fn cell_from_param_in(
    p: &ParamDef,
    pm: &HashMap<String, Value>,
    sounds: &[(String, String)],
) -> EffectParamCell {
    let key = p.key.clone();
    let label = p.label.clone();
    let raw = pm.get(&key);
    match p.ptype {
        crate::scripting::ParamType::Bool => {
            let b = raw
                .and_then(|v| v.as_bool())
                .unwrap_or_else(|| p.default.as_bool().unwrap_or(false));
            EffectParamCell::Bool {
                key,
                label,
                sig: signal(b),
                init: json!(b),
            }
        }
        crate::scripting::ParamType::Choice => {
            let s = raw
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| p.default.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| p.choices.first().cloned().unwrap_or_default());
            let idx = p.choices.iter().position(|c| c == &s).unwrap_or(0);
            EffectParamCell::Choice {
                key,
                label,
                sig: signal(idx),
                choices: p.choices.clone(),
                labels: p.choice_labels.clone(),
                init: json!(s),
            }
        }
        // ★ 音效下拉：候选项由宿主按「音效目录」扫出来（脚本写不出目录列表）。
        //   值可能是短名（素材目录里的音）或绝对路径（外部音效包），两者都是字符串。
        crate::scripting::ParamType::Sound => {
            let s = raw
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| p.default.as_str().map(|s| s.to_string()))
                .unwrap_or_default();
            let choices: Vec<String> = sounds.iter().map(|(v, _)| v.clone()).collect();
            let labels: Vec<String> = sounds.iter().map(|(_, l)| l.clone()).collect();
            let idx = choices.iter().position(|c| c == &s).unwrap_or(0);
            // ★ 音源候选来自外部目录（宿主扫出来的），可能不含用户存的值（目录换了）
            //   或用户压根没配过（s 为空）。此时 sig 落回 0，渲染值会是 `choices[0]`
            //   ——`init` 必须取**同一个渲染值**，否则"开面板未改动 → 落盘时误判成
            //   changed"，从而把第一个候选当用户选择写进配置。
            let effective = choices.get(idx).cloned().unwrap_or_else(|| s.clone());
            EffectParamCell::Sound {
                key,
                label,
                sig: signal(idx),
                choices,
                labels,
                init: json!(effective),
            }
        }
        // ★ 音效目录：值就是目录路径字符串（空串 = 出厂目录）。
        crate::scripting::ParamType::Folder => {
            let s = raw
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| p.default.as_str().map(|s| s.to_string()))
                .unwrap_or_default();
            EffectParamCell::Folder {
                key,
                label,
                sig: signal(s.clone()),
                init: json!(s),
            }
        }
        crate::scripting::ParamType::Color
        | crate::scripting::ParamType::Image
        | crate::scripting::ParamType::Str
        | crate::scripting::ParamType::File => {
            let s = raw
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| p.default.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| {
                    if matches!(p.ptype, crate::scripting::ParamType::Color) {
                        "#FF0000".to_string()
                    } else {
                        String::new()
                    }
                });
            if matches!(p.ptype, crate::scripting::ParamType::Color) {
                let c = hex_to_color(&s);
                // ★ 初值归一到脚本声明的位数（不带 alpha → 6 位，带 → 8 位 `#RRGGBBAA`）。
                //   老配置里位数不一样时若原样留着，会与控件写回的串不等 → 首帧就误判
                //   "改了"，把默认值写进配置（见 `fmt_color`）。
                let s = fmt_color(c, p.alpha);
                EffectParamCell::Color {
                    key,
                    label,
                    sig: signal(c),
                    alpha: p.alpha,
                    init: json!(s),
                }
            } else if matches!(p.ptype, crate::scripting::ParamType::Str) {
                EffectParamCell::Str {
                    key,
                    label,
                    sig: signal(s.clone()),
                    init: json!(s),
                }
            } else if matches!(p.ptype, crate::scripting::ParamType::File) {
                EffectParamCell::File {
                    key,
                    label,
                    sig: signal(s.clone()),
                    init: json!(s),
                }
            } else {
                EffectParamCell::Image {
                    key,
                    label,
                    sig: signal(s.clone()),
                    init: json!(s),
                }
            }
        }
        // ★ 快捷键捕获框（2026-09-29）：值 = 规范化按键串（`Ctrl+Alt+H`）或空串（停用）。
        //   与 Str 同是字符串管道，只是 UI 形态不同（捕获框 vs 文本框）；捕获写回由
        //   `Runtime::try_capture_hotkey` 走 [`PanelModel::script_hotkey_cap`] 完成。
        crate::scripting::ParamType::Hotkey => {
            let s = raw
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| p.default.as_str().map(|s| s.to_string()))
                .unwrap_or_default();
            EffectParamCell::Hotkey {
                key,
                label,
                sig: signal(s.clone()),
                init: json!(s),
            }
        }
        crate::scripting::ParamType::Int => {
            let v = raw
                .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
                .or_else(|| p.default.as_i64())
                .unwrap_or(p.min as i64) as f32;
            let whole = v.round().clamp(p.min, p.max) as i64;
            EffectParamCell::Int {
                key,
                label,
                sig: signal(v2s(v, p.min, p.max)),
                text: signal(whole.to_string()),
                min: p.min,
                max: p.max,
                init: json!(whole),
            }
        }
        _ => {
            // Float
            let v = raw
                .and_then(|v| v.as_f64())
                .or_else(|| p.default.as_f64())
                .unwrap_or(p.min as f64) as f32;
            // ★ step 从此有了消费者（此前只解析不使用）：初值就落在网格上，
            //   `init` 与 `apply_to` 的写回值因此严格相等 → 开面板不改东西不落盘。
            let step = if p.step.is_finite() && p.step > 0.0 {
                p.step
            } else {
                0.01
            };
            let decimals = decimals_for_step(step);
            let q = quantize(v, p.min, p.max, step, decimals);
            EffectParamCell::Float {
                key,
                label,
                sig: signal(v2s(q, p.min, p.max)),
                // 镜像文本与滑块同一初值（同一格式，落盘侧 `float_param_sync` 的
                // 回写判据才认得"这就是没动过"的样子）。
                text: signal(format!("{:.*}", decimals as usize, q)),
                min: p.min,
                max: p.max,
                step,
                decimals,
                init: json_decimal(q, decimals),
            }
        }
    }
}

/// 五个鼠标按键的键名。点击效果与点击声音**共用同一组键、同一顺序**
/// （`config.trail.ripple_bindings` / `sound_bindings` 的键名就是它）。
///
/// 定义在 `core::config`（配置表的键在那里），这里只是转出去给 UI 用——
/// **别再写第二份字面量**，否则卡片 id 与绑定键会悄悄错位。
pub const BUTTON_KEYS: [&str; 5] = crate::core::config::BUTTON_KEYS;
/// 点击效果的键序（[`BUTTON_KEYS`] 的别名：波纹相关代码量已很大，改名收益低）。
pub const RIPPLE_KEYS: [&str; 5] = BUTTON_KEYS;

/// 每键波纹的信号组（键序见 RIPPLE_KEYS，参数各自独立）。
///
/// 效果列表**只有一个来源**：`assets/ClickFx/*.lua`（见
/// [`PanelModel::clickfx_catalog`]）。每个效果就是一个脚本，所以这里只有一个
/// `effect_idx`，不再分"内置样式"和"自定义脚本"两个下拉——对面板来说只有"一个效果"，
/// 与拖尾 / 悬浮标完全同构。往目录里丢一个新 `.lua`，重开面板就会出现在下拉里。
///
/// 尺寸 / 时长 / 颜色不再是面板上的固定行：它们是**脚本自己**的参数（各脚本在自己的
/// `schema()` 里声明），由 `clickfx_params` 那套动态 UI 承载，因此不同效果可以有完全
/// 不同的参数集。
/// 一个虚拟键盘方案的**控件组**（面板按当前选中的方案切着显示）。
#[derive(Clone)]
pub struct VkSchemeControls {
    /// 方案 id（= `vk_schemes[i].id`，也是配置里 `params` 的键）。
    pub scheme_id: String,
    /// 它在 `vk_schemes` 里的下标（控件行的 `visible_when` 用它门控）。
    pub index: usize,
    /// 该方案的控件行（顺序 = 面板上的顺序）。
    pub items: Vec<VkControlItem>,
}

/// 一个控件行：方案给的定义 + 面板上的信号。
#[derive(Clone)]
pub struct VkControlItem {
    /// 方案（或预设）给的定义：标签 / 类型 / 范围 / 默认值。
    pub def: ControlDef,
    /// 面板上的值。类型与 `def.kind` 一致。
    pub sig: VkControlSignal,
}

/// 控件信号的三种形态（与 [`ControlKind`] 一一对应）。
#[derive(Clone, Copy)]
pub enum VkControlSignal {
    /// 滑块（比例信号 + 镜像文本信号）。文本与滑块的写回对账在 `apply_to`
    /// 的 Range 分支："谁与方案默认偏离谁赢"——与脚本参数 `float_param_sync`
    /// 同一套判据（2026-09-25 键盘页滑条升级三件套时引入）。
    Range(Signal<f32>, Signal<String>),
    /// 取色器。
    Color(Signal<Color>),
    /// 勾选框。
    Bool(Signal<bool>),
}

#[derive(Clone)]
pub struct RippleKeySignals {
    /// 「关闭 + 效果」组合下拉的下标（2026-09-25 单卡化）：**0 = 关闭**（该键停用，
    /// 对应 `enabled=false`），1.. = `ripple_choices[i-1]` 对应的脚本。每键的启停
    /// 就住在下拉首项里（先例：录屏音频来源下拉 0=关闭），不再有独立开关。
    pub combo: Signal<usize>,
    /// 当前选中脚本的动态参数单元（schema 驱动；换脚本整批重建且**旧参数即弃**——
    /// 每键只记其当前效果的参数，见 `apply_to` 内的清空逻辑）。
    pub clickfx_params: Signal<Vec<EffectParamCell>>,
    /// 上一次选中的脚本 id（检测"换了脚本" → 重建参数单元）。
    last_clickfx: Rc<RefCell<String>>,
}

/// 把每键波纹信号 diff 回对应绑定项；返回是否有改动。
///
/// ★ 下拉 0 = 关闭：只关 `enabled`，**不丢** `script`（配置里保留"上次的效果"，
/// 用户重新打开下拉选回效果即可）。换效果时参数由 `apply_to` 清空重建（"只记录
/// 当前效果"的参数，旧调参即弃）。配置里的 `style` / `size` / `duration` / `color`
/// 是 1.0 时代的内置样式字段，现在**只读不写**——仅在脚本文件缺失时供 Rust `Ripple`
/// 兜底（见 `modules::trail::controller::on_click`）。
fn apply_ripple_key(
    ks: &RippleKeySignals,
    entry: &mut crate::core::config::ButtonRippleCfg,
    script_id: &str,
) -> bool {
    let mut ch = false;
    if entry.script != script_id {
        entry.script = script_id.to_string();
        ch = true;
    }
    let en = ks.combo.get() > 0;
    if entry.enabled != en {
        entry.enabled = en;
        ch = true;
    }
    ch
}

/// 每键声音的信号组（键序同 [`BUTTON_KEYS`]）。
///
/// ★ 2026-09-25 单卡化后每键只剩**一个**信号：「关闭 + 声音文件」组合下拉的下标
/// （0 = 关闭 = 该键停用，1.. = `sound_choices[i-1]`）。音量五键共享（见
/// [`PanelModel::sound_volume`]），不再每键一份。
#[derive(Clone)]
pub struct SoundKeySignals {
    pub combo: Signal<usize>,
}

/// 老配置（只有内置样式字段、没有脚本 id）→ 新模型（脚本 id + 脚本参数）的一次性迁移。
///
/// 四个内置样式已改写成 `assets/ClickFx/` 里的同名脚本（`ring` / `thin` / `disc` /
/// `double`），所以老配置里的 `style` 可以直接当脚本名用。老配置里的颜色 / 尺寸 / 时长
/// 没有丢的理由——只要该脚本声明了同名参数就播种进去：
///
/// - `color`    → `color`（同为 `#RRGGBB`）
/// - `duration` → `duration`（同为毫秒）
/// - `size`     → `max_size`（老 `size` 是**直径**，新参数是**半径**，故除以 2）
///
/// 只在"脚本确实声明了这个参数"且"配置里还没存过"时才播种：脚本自己的默认值、以及
/// 用户已经调过的值都优先。迁移结果会在下一次 `apply_to` 时落盘。
fn migrate_clickfx(
    entry: Option<&crate::core::config::ButtonRippleCfg>,
    catalog: &EffectCatalog,
) -> (String, HashMap<String, Value>) {
    let Some(e) = entry else {
        return (catalog.id_at(0), HashMap::new());
    };
    let raw_id = if !e.script.is_empty() {
        e.script.clone()
    } else if !e.style.is_empty() {
        e.style.clone()
    } else {
        String::new()
    };
    // §39 脚本目录收敛为 disc / multi_ring：旧 id（ring / thin / double）重映射到
    // 外观最接近的幸存脚本，避免老配置落空；仍不在目录的按首项兜底。
    let script_id = match raw_id.as_str() {
        "ring" | "thin" | "" => "disc".to_string(),
        "double" => "multi_ring".to_string(),
        other => other.to_string(),
    };
    let script_id = if catalog.index_of(&script_id) == 0 && catalog.id_at(0) != script_id {
        catalog.id_at(0)
    } else {
        script_id
    };
    // ★ §40 根治「颜色选择器影子」：迁移**只重映射脚本 id**，历史 `script_params`
    //   一次性重置。此前这里把 legacy `e.color`（1.0 默认 #FFFF00）/ duration / size
    //   播种进参数表——正是"UI 显示 #FFFF00 而脚本默认 #FF0000"反复复发的根子；
    //   且播种值会被 apply_to 落进 config.json，清了播种也清不掉已落盘的毒值。
    //   §39 目录整体重写（disc/multi_ring，参数键与默认值全新），旧参数键/值语义
    //   已失效，重置是唯一干净的迁移。参数一律由脚本 schema 默认接管。
    let _ = e;
    (script_id, HashMap::new())
}

/// 把每键声音信号 diff 回对应绑定项；返回是否有改动。
///
/// ★ 下拉 0 = 关闭：只关 `enabled`，不丢 `file`（同波纹的 `script` 保留策略）。
/// 音量不在每键（共享，见 [`PanelModel::sound_volume`] 的写回块）。
fn apply_sound_key(
    ks: &SoundKeySignals,
    entry: &mut crate::core::config::ButtonSoundCfg,
    file: &str,
) -> bool {
    let mut ch = false;
    if entry.file != file {
        entry.file = file.to_string();
        ch = true;
    }
    let en = ks.combo.get() > 0;
    if entry.enabled != en {
        entry.enabled = en;
        ch = true;
    }
    ch
}

// ─── 模型 ───

/// 快捷键捕获框对应的目标字段。
///
/// 点击某个快捷键框进入捕获态时，`Runtime` 据它把按下的按键写回正确的信号。必须是 `Copy`：
/// 捕获态存在 `Signal<Option<HotkeyField>>` 里，而 `Signal::get` 要求 `T: Clone`。字段与
/// `hk_*` 信号（外加面板快捷键的 `panel_hotkey`）一一对应（见 [`PanelModel::hotkey_signal`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyField {
    Screen,
    White,
    Pen1,
    Pen2,
    Pen3,
    // ── 以下工具项的顺序 = [`crate::core::hotkeys::DRAW_TOOL_ORDER`]（工具栏按钮序）──
    Line,
    Arrow,
    Rect,
    Ellipse,
    Text,
    /// ★ 序号贴纸（2026-10-01 新增，出厂**留空**）：切到「序号」工具，点一下盖一个
    ///   带编号的圈。留空 = 停用（与其余各键的"空串即停用"同口径）。
    Stamp,
    /// 马赛克（2026-09-28 新增，出厂 `0`）。
    Mosaic,
    Eraser,
    /// 截图与贴图。
    Shot,
    /// ★ 截图并贴图。
    ShotPin,
    /// ★ 贴图（剪贴板内容钉到桌面）。
    PinClip,
    /// ★ 长截图快捷键（开始 / 结束一键开关；空串 = 停用）。受「截图与贴图」卡总开关管辖。
    ShotScroll,
    /// ★ 宽截图（向右滚，2026-10-01）。
    ShotScrollWide,
    /// 视频录制（开始 / 停止一键开关）。
    Record,
    /// GIF 录制快捷键（开始 / 停止一键开关）。
    Gif,
    /// ★ GIF 裁剪快捷键（独立打开裁剪窗）。
    Trim,
    /// ★ 选区录屏快捷键（复用 GIF 选框，框好后直接倒计时录这块区域；空串 = 停用）。
    RecordRegion,
    /// ★ MP4 时间裁剪快捷键（打开最近一次录像的裁剪窗；空串 = 停用）。
    RecordTrim,
    /// 计时器开始 / 停止一键开关。
    Timer,
    /// 全屏放大开始 / 停止一键开关。
    FullscreenZoom,
    /// 画中画放大开始 / 停止一键开关（2026-09-26 新增，出厂留空）。
    PipToggle,
    /// 呼出便签（空串 = 停用）。
    Sticky,
    /// 呼出剪贴板历史面板（空串 = 停用）。
    Clipboard,
    /// 显示/隐藏控制面板（空串 = 停用）。2026-09-25 起并入快捷键捕获体系：设置页的
    /// 「快捷键」集合要求所有快捷键同一形状（点击捕获框），面板快捷键不再用文本框特例——
    /// 捕获期间信号先被清空、经 `apply_to` 落到配置后，旧面板快捷键当帧起就不再触发，
    /// 不会出现"捕获时按旧键把面板藏了"的竞态。
    Panel,
}

impl HotkeyField {
    /// 本字段的快捷键是否允许是**裸键**（无修饰键）。
    ///
    /// 只有标注态内的**工具切换键**允许：它们在 `DrawingModule::handle_key` 里匹配、
    /// 且 `is_drawing()` 门控（标注没开根本不生效），打字时按不到，出厂默认就是
    /// `1`..`9`/`0`。其余字段全部是**全局语义**（标注 toggle、截图、录制、贴图、
    /// 便签、剪贴板、计时器、放大镜、面板），裸键会在用户打字时每一次按下都触发，
    /// 一律不允许——捕获时由 `Runtime::try_capture_hotkey` 依此拒绝（判据本体是
    /// `core::hotkeys::global_safe`：带修饰键放行，裸键须落在 F 键区等白名单）。
    pub fn allows_bare_key(self) -> bool {
        matches!(
            self,
            HotkeyField::Pen1
                | HotkeyField::Pen2
                | HotkeyField::Pen3
                | HotkeyField::Line
                | HotkeyField::Arrow
                | HotkeyField::Rect
                | HotkeyField::Ellipse
                | HotkeyField::Text
                | HotkeyField::Stamp
                | HotkeyField::Mosaic
                | HotkeyField::Eraser
        )
    }
}

/// 脚本热键的捕获目标（2026-09-29，`type = "hotkey"` 参数）。
///
/// 与固定字段那套（[`HotkeyField`] + `hotkey_capture`）并行：固定字段的回写信号是
/// 编译期一一对应的 `hk_*`，而脚本参数的 sig 是运行期随单元重建的——捕获态里
/// 直接带上目标信号，`Runtime::try_capture_hotkey` 据此写回。
///
/// ★ 判等只带 sig 一个字段：windui `Signal` 自带 `PartialEq`（槽位 + 代次），同一
///   参数单元的 sig 两份句柄必等——互斥清理据此跳过"正在写新值的自己"。
///   参数单元整批重建（换脚本/重开面板）后旧 sig 槽位回收、代次递增，旧捕获态
///   的句柄不会再与任何新单元相等，往里 set 是无害空操作；捕获态本身随面板模型
///   生命周期走（面板关闭即整模型销毁），不会跨面板残留。
#[derive(Clone, PartialEq)]
pub struct ScriptHotkeyCap {
    /// 回写目标信号（捕获完成后写入规范化按键串）。
    pub sig: Signal<String>,
}

/// 音频「来源」下拉的三态下标契约：**0 = 关闭，1 = 跟随系统默认，2.. = 设备列表[i-2]**。
///
/// ★ 这三个常量 + 下面两个换算函数是初始化 / 保存两处**唯一**的解释处。偏移量一旦散在
/// 各处手算（旧实现是 `i - 1`），就会出现"面板显示 3 号设备、存进去的却是 2 号"——
/// 这种错位不报任何错，只会在录完之后才发现声音来自错误的设备。
/// 纯下标算术，不碰 UI 也不碰配置，故可直接单测（见本文件 `source_index_*` 用例）。
pub const AUDIO_SRC_OFF: usize = 0;
pub const AUDIO_SRC_DEFAULT: usize = 1;
pub const AUDIO_SRC_DEVICE_BASE: usize = 2;

/// 把配置里指定的设备补进下拉列表（枚举不到就追加到末尾）。
///
/// 不做这一步的话，"拔掉的麦克风"会在下拉里变成「关闭」，用户顺手保存就把设置弄丢了。
fn keep_configured_device(devices: &mut Vec<String>, configured: &str) {
    if !configured.is_empty() && !devices.iter().any(|d| d == configured) {
        devices.push(configured.to_string());
    }
}

/// 把"当前默认设备"从下拉列表里剔掉，列表只留**可显式钉死**的其它设备。
///
/// ★ 必须排在 [`keep_configured_device`] **之后**：配置里钉的若正好是当前默认，
///   先剔再补会把刚补进去的那台又留下，等于白做。
///
/// 剔掉之后，[`source_index`] 对"配置里就是那台默认设备"会自然落回
/// [`AUDIO_SRC_DEFAULT`]（找不到 → `unwrap_or`）—— 这正是想要的：界面显示「默认 (…)」，
/// 也顺带把配置归一成"跟随默认"，将来换耳机时不会再把声音钉死在旧设备上。
fn drop_default_device(devices: &mut Vec<String>, default: Option<&DefaultDevice>) {
    if let Some(d) = default {
        devices.retain(|x| !x.eq_ignore_ascii_case(&d.full));
    }
}

/// 三态来源 → 下拉下标（`None`→关闭；`Some("")`→默认；`Some(名)`→该设备，找不到回落默认）。
fn source_index(src: Option<&str>, devices: &[String]) -> usize {
    match src {
        None => AUDIO_SRC_OFF,
        Some("") => AUDIO_SRC_DEFAULT,
        Some(name) => devices
            .iter()
            .position(|d| d == name)
            .map(|i| i + AUDIO_SRC_DEVICE_BASE)
            .unwrap_or(AUDIO_SRC_DEFAULT),
    }
}

/// 下拉下标 → 三态来源（写配置用）。越界按「默认」处理，绝不写出非法状态。
fn source_from_index(idx: usize, devices: &[String]) -> Option<&str> {
    match idx {
        AUDIO_SRC_OFF => None,
        AUDIO_SRC_DEFAULT => Some(""),
        _ => devices
            .get(idx.saturating_sub(AUDIO_SRC_DEVICE_BASE))
            .map(|s| s.as_str())
            .or(Some("")),
    }
}

/// 包卡里的一个可选子脚本。
///
/// 包卡（`scripts/<Pack>/`）把目录里的直接 `.lua` 聚合成一张卡；下拉项只带
/// 展示/参数/宿主所需的最小信息，真正的脚本仍由 `CompanionHost` 按 `id` 加载。
#[derive(Clone)]
pub struct LuaPackEffect {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub params: Vec<crate::scripting::ParamDef>,
}

/// 包卡标题栏下拉的状态。
///
/// `current` 是**上一次已重建参数**的子脚本 id；`apply_to` 每帧对比 `selected`，
/// 发现切换时重填 `card.params`。这样 UI 不必给下拉挂额外回调，沿用现有
/// `host_signal` 的参数重建机制。
#[derive(Clone)]
pub struct LuaExtSelector {
    pub names: Signal<Vec<String>>,
    pub selected: Signal<usize>,
    pub effects: Rc<RefCell<Vec<LuaPackEffect>>>,
    pub current: Rc<RefCell<String>>,
    /// 出厂默认子脚本 id；下拉停在它上面时 `pack_selected` 不落盘（保持零冗余）。
    pub default_id: String,
}

/// 脚本页（原 Lua 页）一张脚本卡片：标题 = `name`，正文 = description + schema 参数，独立折叠态。
///
/// 每脚本一张卡（不再有下拉/运行按钮）：`expanded` 持久化到 `config.ui.expanded["lua.<id>"]`，
/// 参数写回 `config.lua_ext.params[id]`。
///
/// ★ 标题开关 2026-09-27 起**所有 role 都有**（用户口径：`scripts/` 下每个脚本都要能
/// 开关）。语义随 role 走，但控件形状一致：
/// - `composite`：开关 = **运行**（常驻实例，写 `composite_active`）；
/// - `image`：开关 = **贴图右键「脚本」里这一条是否出现**（写 `image_active`）。
///
/// 两种都是"勾了就有可见/可运行的效果"，不是装饰——这是 2026-09-26 那句"没有副作用
/// 就不许画控件"的同一把尺子，只是结论翻了个面：给手动动作配一个只管入口的开关，
/// 正是用户要的开关。
///
/// `role` 供页面决定开关语义（`role_is_active` 的唯一入口）。挂载点由目录决定，
/// 不在卡片上带任何归属字段。
#[derive(Clone)]
pub struct LuaExtCard {
    pub id: String,
    pub name: String,
    pub description: String,
    pub role: String,
    /// 脚本页分组（`mouse`/`picture`/`text`/`ai`/…）；空 = 老脚本不分类。
    pub category: String,
    pub expanded: Signal<bool>,
    /// 标题左侧的开关信号。初值 = [`crate::core::config::LuaExtSettings::role_is_active`]，
    /// 写回 `set_role_active`——**按 `role` 落到对应的列表**，调用方不必自己 match。
    pub toggle: Signal<bool>,
    pub params: Signal<Vec<EffectParamCell>>,
    /// `Some` = 包卡（一张卡聚合 `scripts/<Pack>/` 下多个子脚本）；`None` = 普通单脚本卡。
    pub selector: Option<LuaExtSelector>,
}

impl LuaExtCard {
    /// 该卡片在 `config.ui.expanded` 里的持久化键（id 是相对路径，不与 `CARD_IDS` 冲突）。
    pub fn expanded_key(&self) -> String {
        format!("lua.{}", self.id)
    }

    /// 这张卡是否属于「脚本开关」集合卡与脚本页的**共同收录范围**（2026-09-28）：
    /// 只收 `scripts/` 目录，且排除 `keysound`（键盘页「击键音效」卡专属）。
    /// MouseTrail/HoverDot/ClickFx 下的特效脚本由鼠标页三个专属下拉承接，同样不在内。
    ///
    /// ★ 唯一判定入口：脚本页（`ui::pages::lua`）出卡与设置页「功能开关」卡里的
    /// 「脚本开关」段 **必须**走它——两处各自手写谓词就会慢慢走偏（一边收录一边漏）。
    pub fn is_script_card(&self) -> bool {
        self.id.starts_with("scripts/") && self.role.as_str() != "keysound"
    }
}

/// `scripts/<Pack>/<effect>` → `Some("scripts/<Pack>")`；其余（顶层单脚本）→ `None`。
///
/// ★ 包内脚本按“前两段路径”聚合，不要求额外的 `pack.json`；目录本身就是卡。
fn pack_id_of(id: &str) -> Option<String> {
    let mut parts = id.split('/');
    let root = parts.next()?;
    let pack = parts.next()?;
    if root != "scripts" || pack.is_empty() {
        return None;
    }
    parts.next()?;
    Some(format!("scripts/{pack}"))
}

/// 包下拉里的类别中文名（只给重名项做消歧后缀）。
fn pack_kind_label(kind: &str) -> &'static str {
    match kind {
        "cursor" => "光标",
        "trail" => "拖尾",
        "click" => "点击",
        "hover" => "悬浮",
        "scroll" => "滚轮",
        "hold" => "长按",
        _ => "其他",
    }
}

/// 读包根的 `pack.json`（可选）。只做元数据/顺序/默认项，不参与权限或加载路径。
fn read_pack_meta(dir: &std::path::Path, pack_id: &str) -> Option<serde_json::Value> {
    let name = pack_id.strip_prefix("scripts/")?;
    if name.is_empty() || name.contains('/') {
        return None;
    }
    let path = dir.join("scripts").join(name).join("pack.json");
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// 普通单脚本卡（非包）。保持原有行为：一脚本一卡、无下拉。
fn single_lua_card(
    cfg: &crate::core::config::Config,
    s: &crate::scripting::ExtSchema,
) -> LuaExtCard {
    let stored = cfg.lua_ext.params.get(&s.id).cloned().unwrap_or_default();
    let cells = s
        .params
        .iter()
        .map(|p| cell_from_param(p, &stored))
        .collect();
    let expanded = cfg
        .ui
        .expanded
        .get(&format!("lua.{}", s.id))
        .copied()
        .unwrap_or(false);
    LuaExtCard {
        id: s.id.clone(),
        name: s.name.clone(),
        description: s.description.clone(),
        role: s.role.clone(),
        category: s.category.clone(),
        expanded: signal(expanded),
        toggle: signal(cfg.lua_ext.role_is_active(&s.role, &s.id)),
        params: signal(cells),
        selector: None,
    }
}

/// 工作文件夹「自动清理」的阈值表（**天**；0 = 永不）——单一来源，
/// 下拉显示名（[`TEMP_CLEANUP_LABELS`]，同序）、`from_config`（天数→下标）、
/// `apply_to`（下标→天数写回 `config.general.temp_cleanup_days`）三处共用。
pub const TEMP_CLEANUP_DAYS: [u32; 4] = [0, 7, 30, 180];
/// 「自动清理」下拉的显示名（与 [`TEMP_CLEANUP_DAYS`] 同序）。
pub const TEMP_CLEANUP_LABELS: [&str; 4] = ["永不", "7天之前", "30天之前", "180天之前"];

/// 面板全部控件的信号集合。
#[derive(Clone)]
pub struct PanelModel {
    // 效果目录（内置 + Lua）：下拉选项与 id 映射都从这里来。
    pub catalog: EffectCatalog,

    // ── 设置页 · 工作文件夹 ──
    /// 自动清理下拉下标（指向 [`TEMP_CLEANUP_DAYS`]；0 = 永不）。
    pub temp_cleanup: Signal<usize>,

    // ── 设置页 · 系统 ──
    /// 管理员权限启动勾选（写 `config.general.admin_launch`；**下次启动**生效，
    /// 由 `main.rs` 启动分支消费）。
    pub admin_launch: Signal<bool>,

    // ── 鼠标特效 · 拖尾 ──
    /// 拖尾总开关（同时写 `trail.trail_enabled` 与 `global.enabled_trail`）。
    pub trail_enabled: Signal<bool>,
    /// 效果下标（指向 `catalog`）。
    pub trail_effect: Signal<usize>,
    /// 当前选中特效的参数 UI 单元。选中特效变化时由 `apply_to` 整批重建（经 `host_signal`）。
    pub effect_params: Signal<Vec<EffectParamCell>>,
    /// 上一帧生效的特效 id，用于检测"选中的特效变了"以触发参数单元重建。
    last_effect: Rc<RefCell<String>>,

    // ── 鼠标特效 · 悬浮标 ──
    pub hover_enabled: Signal<bool>,
    /// 悬浮标类型下拉下标（指向 `hover_catalog`）。
    pub hover_type: Signal<usize>,
    /// 当前选中悬浮标脚本的参数 UI 单元（换类型时由 `apply_to` 整批重建）。
    pub hover_params: Signal<Vec<EffectParamCell>>,
    /// 上一帧生效的悬浮标脚本 id（检测"选中变了"以触发参数单元重建）。
    pub last_hover_type: Rc<RefCell<String>>,
    pub hover_image: Signal<String>,
    /// 悬浮标脚本目录（`assets/HoverDot/`，id = 裸 stem）。
    pub hover_catalog: EffectCatalog,

    // ── 鼠标特效 · 点击效果（单卡：总开关 + 五键平铺「关闭/效果」+ 每键参数段）──
    pub ripple: Vec<RippleKeySignals>,
    /// 点击效果效果目录（`assets/ClickFx`）扫描结果：**下拉 + schema 都只从这里来**。
    /// 每个效果就是一个 `.lua`，所以这就是"全部样式"（内置的四个也不例外）。
    pub clickfx_catalog: EffectCatalog,
    /// ★ 效果下拉的数据源：**首项「关闭」+** `clickfx_catalog.display_names()`。
    /// 下标契约见 [`RippleKeySignals::combo`]（0=关闭）。用信号：`assets/` 变化时
    /// 目录监听会重扫并换掉整张表，面板不重开也能选到新效果。
    pub ripple_choices: Signal<Vec<String>>,

    // ── 鼠标特效 · 点击声音（单卡：总开关 + 共享音量 + 每键「关闭/文件」）──
    pub sound: Vec<SoundKeySignals>,
    /// 声音目录（`assets/Sounds`）扫描结果：下拉只从这里来，用户丢个 `.wav` 就能选。
    pub sound_catalog: SoundCatalog,
    /// ★ 声音下拉的数据源：**首项「关闭」+** 目录文件名（带扩展名显示）。下标契约
    /// 同 [`SoundKeySignals::combo`]。同样是信号：`assets/Sounds` 变了就换表。
    pub sound_choices: Signal<Vec<String>>,
    /// ★ 五键**共享音量**（0..=100，滑条装比例、配置装真实量纲）。
    ///
    /// 2026-09-25 单卡化：每键音量是过度细分（1.0 本就是全局音量），收成一行；
    /// `apply_to` 把它写进五键绑定 + `trail.sound_volume`（保持 legacy 字段同步，
    /// 不再 stale）。初值取「第一个启用键的音量」，全关时取 `trail.sound_volume`。
    pub sound_volume: Signal<f32>,
    pub sound_volume_text: Signal<String>,
    /// ★ 点击效果卡总开关（→ `trail.ripple_enabled`，AND 闸）。
    pub ripple_master: Signal<bool>,
    /// ★ 点击声音卡总开关（→ `trail.sound_enabled`，AND 闸）。
    pub sound_master: Signal<bool>,

    // ── Lua 扩展页签 ──
    /// Lua 每脚本一张卡（`assets/lua` 全部脚本，递归扫），顺序 = 注册表字典序。
    /// 脚本页的卡片表（单脚本卡 + 包卡）。**信号**而不是普通字段（m33）：脚本页的
    /// 「刷新」按钮要在面板不重建的前提下换掉整张表，`Element::host_signal` 订阅的就是它。
    /// 其它读者（`apply_to` 的参数 diff、设置页的镜像）一律 `.get()` 拿当前值。
    pub lua_ext_cards: Signal<Vec<LuaExtCard>>,

    // ── 面板卡片展开态（纯界面状态，但同样落盘）──
    /// 键 = [`crate::core::config::CARD_IDS`]，值 = 该卡的展开信号。
    ///
    /// ★ **每张卡一个独立信号**。这里绝不能写成 `[signal(v); N]`——数组重复语法
    /// 只求值一次，N 张卡会共用同一个句柄，表现就是"点一个全部展开"（真实事故）。
    /// 出厂默认见 `core::config::default_card_expanded`（每页第一张展开）。
    pub card_expanded: BTreeMap<&'static str, Signal<bool>>,
    /// `card_expanded` 查不到时的兜底信号。正常路径不会走到（`CARD_IDS` 全量播种），
    /// 存一个真信号只是为了让 [`PanelModel::expand`] 不必返回 `Option`（本项目禁 panic）。
    no_expand: Signal<bool>,

    // ── 击键回显 ──
    pub placement: Signal<usize>,
    /// ☆ 2026-09-21 收敛：四根偏移→两根（水平/垂直，与画中画放大同型）。
    /// 字段保留全部四个（落盘结构不动，老配置自然兼容）：
    /// 水平偏移驱动 left/right 中被方位选中的那根，垂直驱动 top/bottom 中的那根；
    /// 另一侧恒为 0。面板上只露两根。
    pub offset_top: Signal<f32>,
    pub offset_bottom: Signal<f32>,
    pub offset_left: Signal<f32>,
    pub offset_right: Signal<f32>,
    /// 镜像文本（2026-09-25 §35 全滑条三件套）：与左边的滑条信号绑同一份配置，
    /// 在 `apply_to` 经 [`slider_pair_sync`] 对账。下方各滑条字段同款，不再重复注释。
    pub offset_left_text: Signal<String>,
    pub offset_bottom_text: Signal<String>,
    pub popup_width: Signal<f32>,
    pub popup_width_text: Signal<String>,
    pub key_opacity: Signal<f32>,
    pub key_opacity_text: Signal<String>,
    pub fade_delay: Signal<f32>,
    pub fade_delay_text: Signal<String>,
    pub merge_interval: Signal<f32>,
    pub merge_interval_text: Signal<String>,
    pub font_size: Signal<f32>,
    pub font_size_text: Signal<String>,
    pub color_font: Signal<Color>,
    pub color_fun: Signal<Color>,
    pub color_symbol: Signal<Color>,
    pub color_bg: Signal<Color>,
    pub display_mode: Signal<usize>,

    // ── 屏幕标注 ──
    pub pen1_color: Signal<Color>,
    pub pen1_width: Signal<f32>,
    pub pen1_width_text: Signal<String>,
    pub pen1_style: Signal<usize>,
    pub pen2_color: Signal<Color>,
    pub pen2_width: Signal<f32>,
    pub pen2_width_text: Signal<String>,
    pub pen2_style: Signal<usize>,
    pub pen3_color: Signal<Color>,
    pub pen3_width: Signal<f32>,
    pub pen3_width_text: Signal<String>,
    pub pen3_style: Signal<usize>,
    pub hk_screen: Signal<String>,
    pub hk_white: Signal<String>,
    pub hk_pen1: Signal<String>,
    pub hk_pen2: Signal<String>,
    pub hk_pen3: Signal<String>,
    pub hk_eraser: Signal<String>,
    pub hk_line: Signal<String>,
    pub hk_arrow: Signal<String>,
    pub hk_rect: Signal<String>,
    pub hk_ellipse: Signal<String>,
    pub hk_text: Signal<String>,
    pub hk_mosaic: Signal<String>,
    /// 序号贴纸键（= 屏幕页「快捷键」子卡那一行标签「序号」，空串 = 停用）。
    ///
    /// ★ 2026-10-01 新增：序号工具此前只有工具栏图标一个入口。
    pub hk_stamp: Signal<String>,
    /// 截图键（= 屏幕页那一行标签「截图」，空串 = 停用）。
    pub hk_shot: Signal<String>,
    /// ★ 截图后贴图键（= 屏幕页那一行标签「截图后贴图」，空串 = 停用）。
    pub hk_shot_pin: Signal<String>,
    /// ★ 贴图键（= 屏幕页那一行标签「贴图」，空串 = 停用）。
    pub hk_pin_clipboard: Signal<String>,
    /// ★ 长截图键（= 屏幕页那一行标签「长截图」；**开始 / 结束**一键开关；空串 = 停用）。
    pub hk_shot_scroll: Signal<String>,
    /// ★ 宽截图键（= 屏幕页那一行标签「宽截图」，空串 = 停用）。
    pub hk_shot_scroll_wide: Signal<String>,
    /// 录屏快捷键（**开始 / 停止**一键开关；空串 = 停用）。
    pub hk_record: Signal<String>,
    /// GIF 录制快捷键（**开始 / 停止**一键开关；空串 = 停用）。
    pub hk_gif: Signal<String>,
    /// ★ GIF 裁剪快捷键（**独立打开裁剪窗**；空串 = 停用）。
    ///
    /// 与 `hk_gif` 分开：录制键是"框选并录"，裁剪键是"打开编辑器"。合成一把的话，
    /// 想裁磁盘上一个老 GIF 的用户就得先录点什么。
    pub hk_gif_trim: Signal<String>,
    /// 选区录屏快捷键（复用 GIF 选框；空串 = 停用）。与 `hk_record` 并存，两把独立。
    pub hk_record_region: Signal<String>,
    /// MP4 时间裁剪快捷键（打开最近一次录像的裁剪窗；空串 = 停用）。
    pub hk_record_trim: Signal<String>,
    /// 计时器**快捷键**（**开始 / 停止**一键开关；空串 = 停用）。2026-09-27 起出厂留空。
    ///
    /// 与录屏快捷键同款语义：没有独立的总开关，**快捷键为空串即停用**。理由见
    /// `ui::pages::record` 的模块文档——"勾了开关但快捷键是空的"是一种只能骗用户点的死态。
    pub hk_timer: Signal<String>,
    /// 全屏放大的启停快捷键（2026-09-26 起出厂留空，用户自配；Esc 脱身不依赖它）。
    pub hk_fullscreen_zoom: Signal<String>,
    /// 画中画放大的启停快捷键（2026-09-26 新增，出厂留空）。
    pub hk_pip_toggle: Signal<String>,
    // （放大镜倍率的 Ctrl+Win+滚轮 是内置快捷键，无信号、无配置——见
    //  `platform::input::mouse` 的 WM_MOUSEWHEEL 注释。）

    // ── 屏幕页 · 截图与贴图 ──
    /// 贴图历史环容量（下标 → `PIN_CAP_VALUES`）。
    pub pin_cap: Signal<usize>,

    // ── 屏幕页 · 视频录制 ──
    /// 帧率档下标（指向 `RECORD_FPS_LABELS`）。
    pub rec_fps: Signal<usize>,
    /// 分辨率档下标（指向 `RECORD_RESOLUTION_LABELS`）。
    pub rec_resolution: Signal<usize>,
    /// 硬件编码策略下标（指向 `RECORD_HARDWARE_LABELS`）。
    pub rec_hardware: Signal<usize>,
    /// 码率档下标（指向 `RECORD_BITRATE_LABELS` / `RECORD_BITRATE_VALUES`；
    /// 下标 0 = 自动，由录屏器按最终输出尺寸 × 帧率算）。
    pub rec_bitrate: Signal<usize>,
    /// 是否把鼠标光标画进画面。
    pub rec_cursor: Signal<bool>,
    /// 麦克风来源**三态**下标：0=关闭，1=跟随系统默认，2..= `rec_mic_devices[i - 2]`。
    ///
    /// 与系统声音的来源合成一个下拉（用户口径 2026-09-19）：原来"开关 + 来源"两行说的是
    /// 同一件事，且"选着设备却把开关关了"是一种能存进配置的矛盾态。
    pub rec_mic_source: Signal<usize>,
    /// 系统声音来源三态下标，偏移同 [`Self::rec_mic_source`]。
    pub rec_speaker_source: Signal<usize>,
    /// 麦克风降噪档位下标（指向 `MIC_DENOISE_LABELS`：`off`/`weak`/`medium`/`strong`）。
    pub rec_denoise: Signal<usize>,
    /// ★ 麦克风试听开关（**纯界面状态，不落盘**）。
    ///
    /// 它不是一个配置项：试听只在面板开着、用户按住这个意图时才有意义，关面板/重启
    /// 都不该恢复"上次还在听"。所以它不进 `Config`，由 `Runtime` 每帧对账成真实的
    /// `MicMonitor` 生命周期（见 `runtime::Runtime::sync_mic_monitor`）。
    pub rec_monitor: Signal<bool>,
    /// 当前发现的麦克风设备友好名列表（不含"默认"项），供下拉解析。
    pub rec_mic_devices: Vec<String>,
    /// 当前发现的系统声音设备友好名列表（不含"默认"项），供下拉解析。
    pub rec_speaker_devices: Vec<String>,
    /// 当前系统默认麦克风设备。`None` = 没有默认端点 / 枚举失败。
    ///
    /// 两个用途：① 让面板把「默认」写成「默认 (Realtek…）」，用户不必去系统设置里翻
    /// 才知道跟随的是哪台；② [`drop_default_device`] 靠它把默认设备从列表里**剔除** ——
    /// 否则下拉里会同时出现「默认 (Realtek…）」与「扬声器 (Realtek…)」，两条指向同一端点
    /// 却语义不同（跟随 vs 钉死），用户没法选。
    pub rec_mic_default: Option<DefaultDevice>,
    /// 当前系统默认系统声音设备。语义见 [`Self::rec_mic_default`]。
    pub rec_speaker_default: Option<DefaultDevice>,

    // ── 录制 · GIF ──
    /// GIF 帧率档下标（指向 `GIF_FPS_LABELS`）。
    pub gif_fps: Signal<usize>,
    /// GIF 输出尺寸档下标（指向 `GIF_SIZE_LABELS`）。
    pub gif_size: Signal<usize>,
    /// 是否把鼠标光标画进 GIF。
    pub gif_cursor: Signal<bool>,
    /// 是否无限循环。
    pub gif_loop: Signal<bool>,

    // ── 虚拟键盘 ──
    pub vk_enabled: Signal<bool>,
    /// 当前可用方案快照（`assets/VirtualKeyboards/` 的扫描结果 + 出厂内嵌兜底）。
    ///
    /// `assets/` 目录监听重扫时整表换掉（见 [`PanelModel::reload_asset_lists`]），
    /// 下拉选项走 [`PanelModel::vk_scheme_names`] 信号、控件行走 `vk_controls` 信号。
    pub vk_schemes: Vec<SchemeEntry>,
    /// 方案下拉的显示名（与 [`PanelModel::vk_schemes`] 同序）。信号驱动：
    /// 往 `assets/VirtualKeyboards/` 丢一个新方案，面板不重开也能在表里看到。
    pub vk_scheme_names: Signal<Vec<String>>,
    /// 方案下标（指向 [`PanelModel::vk_schemes`]）。
    pub vk_layout: Signal<usize>,
    pub vk_placement: Signal<usize>,
    pub vk_offset_top: Signal<f32>,
    pub vk_offset_bottom: Signal<f32>,
    pub vk_offset_left: Signal<f32>,
    pub vk_offset_right: Signal<f32>,
    pub vk_offset_left_text: Signal<String>,
    pub vk_offset_bottom_text: Signal<String>,
    /// ★ 虚拟键盘「大小」「颜色」两卡的控件：**每个方案一份**，与
    /// [`PanelModel::vk_schemes`] 同序。
    ///
    /// 控件是方案 JSON 声明的（没声明就由 `vk_config::controls_of` 补齐一套预设），
    /// 按方案逐个建；切方案时页面按 `vk_layout` 的下标切到对应那一组
    /// （见 `ui::pages::key_display`）。目录监听重扫出新方案时整表换掉，页面经
    /// `host_signal` 重建控件行，不等重开面板。
    /// 位置与四向偏移不在这里：它们是全局配置，方案不能干预。
    /// 用信号：目录监听重扫出新方案后整批换掉，页面经 `host_signal` 重建控件行。
    pub vk_controls: Signal<Vec<VkSchemeControls>>,
    /// 鼠标穿透（勾选 = 纯展示；不勾 = 可点击，命中键帽即注入按键）。
    pub vk_click_through: Signal<bool>,
    /// 总在最上（全局 `Config::overlay`，对所有功能生效）。
    pub vk_always_on_top: Signal<bool>,
    /// 全屏避让（全局 `Config::overlay`；前台全屏应用时覆盖层让位，取消勾选则压上去）。
    pub vk_fullscreen_avoid: Signal<bool>,

    // ── 快捷键捕获态（纯 UI 状态，不落盘）──
    /// 当前正在捕获的快捷键字段；`None` = 没有框在捕获。点击某个快捷键框把它设成 `Some(field)`
    /// （同时清空该框对应信号），`Runtime::frame()` 拦截到下一个按键就写回信号并清零。
    pub hotkey_capture: Signal<Option<HotkeyField>>,
    /// 脚本热键捕获态（2026-09-29）：`type = "hotkey"` 参数的捕获框点下后置 Some，
    /// `Runtime::try_capture_hotkey` 拦到按键写回 [`ScriptHotkeyCap::sig`] 并清零。
    /// 与 `hotkey_capture` 互斥使用（同一时刻至多一个捕获框在等键）。
    pub script_hotkey_cap: Signal<Option<ScriptHotkeyCap>>,

    // ── 全局 ──
    /// ★ 面板当前所在导航页（主窗口状态记忆 → `ui.panel_page`）。面板每次打开都是一套
    /// 新信号，页码必须活在配置里才能跨开关/跨进程存活；由 `panel::build` 用它做导航
    /// 与内容切换，`apply_to` 每帧 diff 回配置，随防抖落盘。
    pub page: Signal<usize>,
    /// 击键回显卡片开关（唯一开关 → `key_display.enabled`）。
    pub enabled_key: Signal<bool>,
    /// 屏幕标注卡片开关（唯一开关 → `drawing.enabled`）。
    pub enabled_draw: Signal<bool>,
    // ── 便签（工具页；无总开关，快捷键空串即停用）──
    /// 呼出便签的快捷键（空串 = 停用，→ `note.hotkey`）。
    pub hk_sticky: Signal<String>,
    /// 背景颜色（→ `note.bg_color`）。
    pub note_bg_color: Signal<Color>,
    /// 文本颜色（→ `note.text_color`）。
    pub note_text_color: Signal<Color>,
    /// 编辑区颜色（→ `note.edit_color`）。
    pub note_edit_color: Signal<Color>,
    /// 窗口透明度（百分比，10–100，滑块装比例、配置装真实量纲）。
    pub note_alpha: Signal<f32>,
    pub note_alpha_text: Signal<String>,
    /// 字号（9–72，滑块装比例、配置装真实量纲）。
    pub note_font_sz: Signal<f32>,
    pub note_font_sz_text: Signal<String>,
    /// 打开控制面板的快捷键（空串 = 停用）。
    pub panel_hotkey: Signal<String>,
    // ── 计时器（docs/DESIGN.md §1 #15）──
    /// 计时器模式下标：0 正计时 / 1 倒计时（→ `global.timer.mode`）。
    /// 2026-09-25 起绑在卡片**标题栏**下拉（功能级选择器，与击键回显的模式同款先例）；
    /// 番茄钟同日整体删除（用户口径），老配置残留 `"pomodoro"` 在这里落回 0 正计时。
    pub timer_mode: Signal<usize>,
    /// 倒计时时长（秒，→ `global.timer.duration_sec`；只对倒计时生效，正计时没有终点）。
    pub timer_duration: Signal<f32>,
    /// 时长输入框的镜像文本（2026-09-25）：滑条之外的第二入口，补"1–3600 秒量程下
    /// 1px ≈ 18s 点不到精确秒"的精度短板。与 `timer_duration` 绑同一份配置，两边在
    /// [`Self::apply_to`] 对账（谁与配置偏离谁赢）——这里只存字符串原样，前导零等
    /// 打字中间态不被回写打架。
    pub timer_duration_text: Signal<String>,
    /// 横幅锚点下标：0 中央 / 1 顶部居中 / 2 底部居中 / 3 左上 / 4 右上 / 5 左下 / 6 右下
    /// （→ `global.timer.anchor`；语义见 `runtime::timer::timer_center`）。
    pub timer_anchor: Signal<usize>,
    /// 水平偏移（屏幕宽度百分比 0–50，→ `global.timer.offset_x`；中央锚点不生效）。
    pub timer_offset_x: Signal<f32>,
    pub timer_offset_x_text: Signal<String>,
    /// 垂直偏移（屏幕高度百分比 0–50，→ `global.timer.offset_y`；中央锚点不生效）。
    pub timer_offset_y: Signal<f32>,
    pub timer_offset_y_text: Signal<String>,
    /// 读秒字号（pt 24–160，→ `global.timer.font_pt`；横幅底框随文字量得，字号即整体大小）。
    pub timer_font: Signal<f32>,
    pub timer_font_text: Signal<String>,
    // ── 画中画放大（屏幕页）──
    /// 总开关。
    pub pip_enabled: Signal<bool>,
    /// 放大面板宽（物理像素）。
    pub pip_panel_w: Signal<f32>,
    pub pip_panel_w_text: Signal<String>,
    /// 放大面板高（物理像素）。
    pub pip_panel_h: Signal<f32>,
    pub pip_panel_h_text: Signal<String>,
    /// 放大倍率（百分比，100–400）。
    pub pip_zoom: Signal<f32>,
    pub pip_zoom_text: Signal<String>,
    /// 面板位置：0 鼠标跟随 / 1 左上 / 2 右上 / 3 左下 / 4 右下。
    pub pip_anchor: Signal<usize>,
    /// 水平偏移（屏幕宽度百分比 0–50）：贴边时把面板从边上往屏内挪。
    pub pip_offset_x: Signal<f32>,
    pub pip_offset_x_text: Signal<String>,
    /// 垂直偏移（屏幕高度百分比 0–50）：贴下边抬高可让开任务栏。
    pub pip_offset_y: Signal<f32>,
    pub pip_offset_y_text: Signal<String>,
    /// 面板边框颜色。
    pub pip_border_color: Signal<Color>,
    // ── 全屏放大（屏幕页）──
    /// 总开关。
    pub fs_enabled: Signal<bool>,
    /// 放大倍率（1–5，滑块装比例、配置装真实量纲）。
    pub fs_zoom: Signal<f32>,
    pub fs_zoom_text: Signal<String>,
    // ── 剪贴板历史（工具页）──
    /// 功能总开关（→ clipboard.enabled）。
    pub clip_enabled: Signal<bool>,
    /// 呼出面板的快捷键（空串 = 停用，→ clipboard.hotkey）。
    pub hk_clipboard: Signal<String>,
    /// 历史容量档下标（指向 `CLIP_ENTRIES_VALUES`：500/1000/2000/不限）。
    pub clip_max_entries: Signal<usize>,

    // ── 五卡启用开关（截图与贴图 / GIF录制 / 视频录制 / 便签 / 计时器）──
    /// 截图与贴图总开关（→ screenshot.enabled）。关 = 三把快捷键全部不响应。
    pub shot_enabled: Signal<bool>,
    /// GIF录制总开关（→ gif.enabled）。关 = GIF / 裁剪快捷键不响应。
    pub gif_enabled: Signal<bool>,
    /// 视频录制总开关（→ record.enabled）。关 = 录屏快捷键不响应。
    pub rec_enabled: Signal<bool>,
    /// 便签总开关（→ sticky_note.enabled）。关 = 呼出快捷键不响应。
    pub sticky_enabled: Signal<bool>,
    /// 计时器总开关（→ global.timer.enabled）。关 = 计时器快捷键不响应。
    pub timer_enabled: Signal<bool>,

    // ── 击键音效（键盘页）──
    /// 总开关。
    pub ksound_enabled: Signal<bool>,
    /// 脚本下标（指向 `ksound_names`；与拖尾效果的下拉同款——下拉恒存下标）。
    pub ksound_script: Signal<usize>,
    /// 音量（0–100，滑块装比例、配置装真实量纲）。
    pub ksound_volume: Signal<f32>,
    pub ksound_volume_text: Signal<String>,
    /// 击键音效脚本目录扫描结果（`assets/lua` 里 `role="keysound"`；下拉 + schema）。
    pub ksound_catalog: KeysoundCatalog,
    /// 当前脚本的参数单元（脚本 `schema()` 生成，换脚本整批重建；驱动 host_signal 段）。
    pub ksound_params: Signal<Vec<EffectParamCell>>,
    /// 上一次生效的脚本 id（检测"换了脚本"→ 重建参数单元）。
    last_ksound_script: Rc<RefCell<String>>,
    // （「音效文件夹 / 音效文件」两行与其信号已删——2026-09-25 起统一为脚本参数里的
    //   `file` 浏览按钮，浏览即引用：程序目录内记便携相对路径，外部记绝对路径。）
}

/// 击键音效的参数单元 = 脚本 schema × 该脚本已存的参数 × 素材目录扫描结果。
///
/// ★ 音效下拉（`Sound` 参数，老脚本兼容）的候选项**不在脚本里**（脚本没有 io），由宿主
/// 扫出厂素材目录 `assets/Sounds/` 得出。值 = 文件名字符串（`ctx.sounds` 里的元素）。
fn ks_build_cells(
    catalog: &KeysoundCatalog,
    id: &str,
    stored: &HashMap<String, Value>,
) -> Vec<EffectParamCell> {
    let sounds = ks_sound_choices();
    catalog
        .schema_for(id)
        .iter()
        .map(|p| cell_from_param_in(p, stored, &sounds))
        .collect()
}

/// 击键音效的参数候选：出厂素材目录（`assets/Sounds`）里的文件名列表（值 = 显示名 = 文件名）。
///
/// ★ 2026-09-25 二次改版：浏览即引用、无中转目录，`ctx.sounds` 恒指出厂素材——
/// 所以这里不再吃参数，直接扫出厂目录。
fn ks_sound_choices() -> Vec<(String, String)> {
    crate::modules::key_sound::wav_filenames(&crate::core::paths::sounds_dir().to_string_lossy())
        .into_iter()
        .map(|f| (f.clone(), f))
        .collect()
}

/// 扫 `assets/lua` 构建脚本页的全部卡片（单脚本卡 + 包卡），按 id 排序。
///
/// ★ 从 [`PanelModel::from_config`] 里抽出来（m33）：**面板打开时**调一次，脚本页的
///   「刷新」按钮再调一次（见 [`PanelModel::reload_lua_cards`]）——两处必须是同一份构建，
///   否则"刷新出来的东西"和"重启后看到的"会悄悄不一样。
pub(crate) fn build_lua_cards(cfg: &Config, dir: &std::path::Path) -> Vec<LuaExtCard> {
    // ── Lua 扩展页签：扫描 `assets/lua`。
    //   `scripts/<Pack>/` 下的直接 `.lua` 聚合成**一张包卡**；其余脚本仍一脚本一卡。
    let mut lua_ext_catalog = crate::scripting::registry_ext::LuaExtRegistry::new(dir);
    lua_ext_catalog.scan();
    let mut lua_cards: Vec<LuaExtCard> = Vec::new();
    let mut packs: std::collections::BTreeMap<String, Vec<crate::scripting::ExtSchema>> =
        std::collections::BTreeMap::new();
    for s in lua_ext_catalog.list() {
        match pack_id_of(&s.id) {
            Some(pack_id) => packs.entry(pack_id).or_default().push(s.clone()),
            None => lua_cards.push(single_lua_card(cfg, s)),
        }
    }
    for (pack_id, mut effects) in packs {
        effects.sort_by(|a, b| a.id.cmp(&b.id));
        let meta = read_pack_meta(dir, &pack_id);
        // `single: true` 的官方工具包（OCR/条码/图片体检）只有一个脚本：
        // 不摆成"一项的下拉"，直接出普通脚本卡。
        if meta
            .as_ref()
            .and_then(|m| m.get("single"))
            .and_then(|v| v.as_bool())
            == Some(true)
        {
            for s in effects {
                lua_cards.push(single_lua_card(cfg, &s));
            }
            continue;
        }
        // `pack.json.effects` 的顺序/白名单优先；缺清单时按 id 排序。
        if let Some(list) = meta
            .as_ref()
            .and_then(|m| m.get("effects"))
            .and_then(|v| v.as_array())
        {
            let mut ordered = Vec::new();
            for item in list {
                let Some(id) = item.get("id").and_then(|v| v.as_str()) else {
                    continue;
                };
                if let Some(pos) = effects
                    .iter()
                    .position(|e| e.id.rsplit('/').next() == Some(id))
                {
                    ordered.push(effects.remove(pos));
                }
            }
            ordered.extend(effects);
            effects = ordered;
        }
        let Some(first) = effects.first() else {
            continue;
        };
        let default_id = meta
            .as_ref()
            .and_then(|m| m.get("default"))
            .and_then(|v| v.as_str())
            .and_then(|id| {
                effects
                    .iter()
                    .find(|e| e.id.rsplit('/').next() == Some(id))
                    .map(|e| e.id.clone())
            })
            .unwrap_or_else(|| first.id.clone());
        let selected_id = cfg
            .lua_ext
            .pack_selected
            .get(&pack_id)
            .cloned()
            .unwrap_or_else(|| default_id.clone());
        let selected_idx = effects
            .iter()
            .position(|e| e.id == selected_id)
            .unwrap_or(0);
        let current_id = effects
            .get(selected_idx)
            .map(|e| e.id.clone())
            .unwrap_or_default();
        let stored = cfg
            .lua_ext
            .params
            .get(&current_id)
            .cloned()
            .unwrap_or_default();
        let cells = effects
            .get(selected_idx)
            .map(|e| {
                e.params
                    .iter()
                    .map(|p| cell_from_param(p, &stored))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        // 同一包里的 `bubbles` / `stars` 原始名确实重复（拖尾 + 点击各一份）。
        // 只对**重名项**追加类别后缀，唯一名保持原样——标题仍是原名。
        let names = effects
            .iter()
            .map(|e| {
                let dup = effects.iter().filter(|o| o.name == e.name).count() > 1;
                if dup {
                    format!("{} ({})", e.name, pack_kind_label(&e.kind))
                } else {
                    e.name.clone()
                }
            })
            .collect::<Vec<_>>();
        let metas = effects
            .iter()
            .map(|e| LuaPackEffect {
                id: e.id.clone(),
                name: e.name.clone(),
                kind: e.kind.clone(),
                params: e.params.clone(),
            })
            .collect::<Vec<_>>();
        let active = effects
            .iter()
            .any(|e| cfg.lua_ext.composite_active.iter().any(|x| x == &e.id));
        let expanded = cfg
            .ui
            .expanded
            .get(&format!("lua.{pack_id}"))
            .copied()
            .unwrap_or(false);
        let pack_name = meta
            .as_ref()
            .and_then(|m| m.get("name"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| pack_id.rsplit('/').next().unwrap_or(&pack_id).to_string());
        let description = meta
            .as_ref()
            .and_then(|m| m.get("description"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let category = meta
            .as_ref()
            .and_then(|m| m.get("category"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        lua_cards.push(LuaExtCard {
            id: pack_id.clone(),
            name: pack_name,
            description,
            role: "pack".to_string(),
            category,
            expanded: signal(expanded),
            toggle: signal(active),
            params: signal(cells),
            selector: Some(LuaExtSelector {
                names: signal(names),
                selected: signal(selected_idx),
                effects: Rc::new(RefCell::new(metas)),
                current: Rc::new(RefCell::new(current_id)),
                default_id,
            }),
        });
    }
    // 分类卡片先排（mouse → picture → text → ai），未知分类按分类名排，
    // **没有 category 的老脚本最后**、组内仍按原来的 id 顺序。
    fn rank(category: &str) -> u8 {
        match category {
            "mouse" => 0,
            "picture" => 1,
            "text" => 2,
            "ai" => 3,
            "" => 5,
            _ => 4,
        }
    }
    fn card_key(c: &LuaExtCard) -> (u8, &str, &str) {
        let r = rank(&c.category);
        (
            r,
            if r == 4 { c.category.as_str() } else { "" },
            c.id.as_str(),
        )
    }
    lua_cards.sort_by(|a, b| card_key(a).cmp(&card_key(b)));

    lua_cards
}

/// 两张卡能不能"沿用旧实例"（见 [`PanelModel::reload_lua_cards`]）：id 相同、且**外形**
/// 一致——名字/角色/说明、参数格的 key 序列、包卡的下拉候选。外形变了就得重建。
fn same_card_shape(old: &LuaExtCard, new: &LuaExtCard) -> bool {
    if old.id != new.id
        || old.name != new.name
        || old.role != new.role
        || old.description != new.description
        || old.category != new.category
    {
        return false;
    }
    let (old_cells, new_cells) = (old.params.get(), new.params.get());
    let old_keys: Vec<&str> = old_cells.iter().map(|c| c.key()).collect();
    let new_keys: Vec<&str> = new_cells.iter().map(|c| c.key()).collect();
    if old_keys != new_keys {
        return false;
    }
    match (&old.selector, &new.selector) {
        (None, None) => true,
        (Some(a), Some(b)) => a.names.get() == b.names.get(),
        _ => false,
    }
}

impl PanelModel {
    /// 重扫脚本目录并把卡片表换掉（m33：脚本页「刷新」按钮的后半段）。
    ///
    /// 与 [`build_lua_cards`] 是同一份构建 ⇒ "刷新出来的"与"重开面板看到的"必然一致。
    /// **只换列表**：配置与正在运行的实例一个都不碰；新脚本按 `cfg` 现值初始化卡片。
    /// 返回条目数（包卡也算）。
    pub fn reload_lua_cards(&self, cfg: &Config, dir: &std::path::Path) -> usize {
        let fresh = build_lua_cards(cfg, dir);
        let n = fresh.len();
        // ★ 同 id 且"形状没变"的卡片**复用旧实例**（m33 审查修）：设置页那列脚本开关
        //   镜像行是**面板打开时**绑的旧卡片 `toggle` 信号。整表替换会让那些行绑在
        //   **孤儿信号**上——拨了开关看着有反应，`apply_to` 却读不到它，配置没人写。
        //   复用之后旧句柄仍指向当前卡片，镜像行照常生效。
        //   形状变了（schema 加参数 / 改名 / 换角色 / 包的子项变了）就不复用：那类改动
        //   本来就要重建参数格，宁可视作"这张卡需要重开面板"。
        let old = self.lua_ext_cards.get();
        let merged: Vec<LuaExtCard> = fresh
            .into_iter()
            .map(|new| match old.iter().find(|o| same_card_shape(o, &new)) {
                Some(o) => o.clone(),
                None => new,
            })
            .collect();
        self.lua_ext_cards.set(merged);
        n
    }

    /// 目录监听用：重扫所有**从 `assets/` 扫出来**的列表，并尽量保住当前选中项。
    ///
    /// 覆盖：拖尾 / 悬浮标 / 点击效果 / 点击声音 / 键音脚本 / 虚拟键盘方案。
    /// 脚本卡由 [`Self::reload_lua_cards`] 单独换（它有自己的"形状没变就复用"规则）。
    ///
    /// ★ 选中项按 **id** 重新定位，不按下标：新增文件可能插到旧项前面，按下标会
    ///   悄悄把用户选中的效果换掉。被删掉的项：拖尾/悬浮标沿用 `index_of_or_keep`
    ///   （下拉里标「脚本缺失」），点击效果/声音回「关闭」，键音回第一项。
    pub fn reload_asset_lists(&mut self, cfg: &Config) {
        let trail_id = self.catalog.id_at(self.trail_effect.get());
        self.catalog.refresh();
        self.trail_effect
            .set(self.catalog.index_of_or_keep(&trail_id));

        let hover_id = self.hover_catalog.id_at(self.hover_type.get());
        self.hover_catalog.refresh();
        self.hover_type
            .set(self.hover_catalog.index_of_or_keep(&hover_id));

        let ripple_ids: Vec<String> = self
            .ripple
            .iter()
            .map(|k| {
                let combo = k.combo.get();
                if combo == 0 {
                    String::new()
                } else {
                    self.clickfx_catalog.id_at(combo - 1)
                }
            })
            .collect();
        self.clickfx_catalog.refresh();
        let mut ripple_choices = vec!["关闭".to_string()];
        ripple_choices.extend(self.clickfx_catalog.display_names());
        self.ripple_choices.set(ripple_choices);
        for (k, id) in self.ripple.iter().zip(ripple_ids) {
            if id.is_empty() {
                k.combo.set(0);
                continue;
            }
            let idx = self.clickfx_catalog.index_of(&id);
            let kept = self.clickfx_catalog.id_at(idx) == id;
            k.combo.set(if kept { idx + 1 } else { 0 });
        }

        let sound_ids: Vec<String> = self
            .sound
            .iter()
            .map(|k| {
                let combo = k.combo.get();
                if combo == 0 {
                    String::new()
                } else {
                    self.sound_catalog.id_at(combo - 1)
                }
            })
            .collect();
        self.sound_catalog.refresh();
        let mut sound_choices = vec!["关闭".to_string()];
        sound_choices.extend(self.sound_catalog.display_names());
        self.sound_choices.set(sound_choices);
        for (k, id) in self.sound.iter().zip(sound_ids) {
            if id.is_empty() {
                k.combo.set(0);
                continue;
            }
            let idx = self.sound_catalog.index_of(&id);
            let kept = self.sound_catalog.id_at(idx) == id;
            k.combo.set(if kept { idx + 1 } else { 0 });
        }

        let ks_id = self.ksound_catalog.id_at(self.ksound_script.get());
        self.ksound_catalog.refresh();
        let ks_idx = self.ksound_catalog.index_of(&ks_id);
        self.ksound_script
            .set(if self.ksound_catalog.id_at(ks_idx) == ks_id {
                ks_idx
            } else {
                0
            });

        let old_vk = self
            .vk_schemes
            .get(self.vk_layout.get())
            .map(|e| e.id.clone());
        let schemes = crate::modules::virtual_keyboard::vk_config::scheme_list();
        let vk_idx = old_vk
            .and_then(|id| schemes.iter().position(|e| e.id == id))
            .unwrap_or(0);
        self.vk_schemes = schemes;
        self.vk_scheme_names
            .set(self.vk_schemes.iter().map(|e| e.label.clone()).collect());
        self.vk_layout.set(vk_idx);
        self.vk_controls
            .set(Self::vk_controls_for(&self.vk_schemes, cfg));
    }

    /// 用当前配置初始化全部信号。
    pub fn from_config(cfg: &Config) -> Self {
        let t = &cfg.trail;
        let k = &cfg.key_display;
        let d = &cfg.drawing;
        let g = &cfg.global;
        let p = &cfg.magnifier_pip;
        let f = &cfg.magnifier_fullscreen;

        let catalog = EffectCatalog::new();
        // 悬浮标脚本目录扫描（与拖尾特效同一套机制；png 也是真脚本，绘制走 ctx:image）。
        let hover_catalog = EffectCatalog::with_dir(crate::core::paths::hoverdot_dir());
        // 击键音效脚本目录扫描（`assets/lua` 里 `role="keysound"`；面板每次打开重建
        // PanelModel → 自动重扫，往 assets/lua 丢新 keysound 脚本下次开面板即可选到）。
        let ksound_catalog = KeysoundCatalog::from_lua();
        // 当前脚本的参数单元：schema 声明 + 配置里该脚本已存的值（缺键用 schema 默认）。
        let ks_cells: Vec<EffectParamCell> = {
            let ks = &cfg.key_sound;
            let stored = ks.params.get(&ks.script).cloned().unwrap_or_default();
            ks_build_cells(&ksound_catalog, &ks.script, &stored)
        };
        // 点击效果效果目录扫描（与悬浮标同套机制，独立目录 assets/ClickFx）。
        let clickfx_catalog = EffectCatalog::with_dir(crate::core::paths::clickfx_dir());
        // 点击声音目录扫描（assets/Sounds 下的 .wav；用户丢新文件即可扩展）。
        let sound_catalog = SoundCatalog::new();
        // 自动清理：天数 → 下标（不在表里的历史值一律回「永不」，不猜）。
        let temp_cleanup_idx = TEMP_CLEANUP_DAYS
            .iter()
            .position(|d| *d == cfg.general.temp_cleanup_days)
            .unwrap_or(0);
        // 卡片展开态：`CARD_IDS` 全量播种，值 = 配置里记的（没有就取出厂默认）。
        // ★ 逐项调 `signal()` 造独立句柄——写成 `[signal(v); N]` 就成了同一个信号。
        let card_expanded: BTreeMap<&'static str, Signal<bool>> = crate::core::config::CARD_IDS
            .iter()
            .map(|id| {
                let v = cfg
                    .ui
                    .expanded
                    .get(*id)
                    .copied()
                    .unwrap_or_else(|| crate::core::config::default_card_expanded(id));
                (*id, signal(v))
            })
            .collect();
        let hover_type_idx = hover_catalog.index_of_or_keep(&t.hover_type);
        let hover_type_id = hover_catalog.id_at(hover_type_idx);
        let hover_stored = t
            .hover_params
            .get(&hover_type_id)
            .cloned()
            .unwrap_or_default();
        let hover_cells: Vec<EffectParamCell> = hover_catalog
            .schema_for(&hover_type_id)
            .iter()
            .map(|p| cell_from_param(p, &hover_stored))
            .collect();
        let pens = &d.pens;

        // 选中特效的 id 与参数初始化：从目录 schema + 当前配置取值构造动态 UI 单元。
        let effect_idx = catalog.index_of_or_keep(&t.trail_effect);
        let effect_id = catalog.id_at(effect_idx);
        let module_ts = crate::modules::trail::settings::from_config(&cfg.trail);
        let pm = module_ts.effect_param_map(&effect_id);
        let effect_params: Vec<EffectParamCell> = catalog
            .schema_for(&effect_id)
            .iter()
            .map(|p| cell_from_param(p, &pm))
            .collect();

        // 录制音频设备列表（运行期枚举；用于"来源"三态下拉）。
        //
        // ★ 配置里指定的那台设备若枚举不到（拔了 / 改了名），要**补进列表末尾**：
        //   否则下拉会显示成「关闭」，用户随手一保存就把设置弄丢了。
        let mut mic_devices = crate::platform::audio_capture::list_mic_devices();
        let mut speaker_devices = crate::platform::audio_capture::list_speaker_devices();
        keep_configured_device(&mut mic_devices, &cfg.record.mic_device);
        keep_configured_device(&mut speaker_devices, &cfg.record.speaker_device);
        // 默认设备：单独记住（给「默认 (…)」当文案）+ 从列表里剔除（见 `drop_default_device`）。
        let mic_default = crate::platform::audio_capture::default_mic_device();
        let speaker_default = crate::platform::audio_capture::default_speaker_device();
        drop_default_device(&mut mic_devices, mic_default.as_ref());
        drop_default_device(&mut speaker_devices, speaker_default.as_ref());
        // 三态语义只有 `source_index` / `source_from_index` 两处解释，见文件末尾。
        let mic_source_idx = source_index(cfg.record.mic_source(), &mic_devices);
        let speaker_source_idx = source_index(cfg.record.speaker_source(), &speaker_devices);

        // 虚拟键盘方案：扫 `assets/VirtualKeyboards/`（目录即来源）+ 出厂内嵌兜底。
        //
        // ★ 顺手把每个方案的控件组也建出来（外观 / 配色两卡的内容）：控件是方案声明的，
        //   只能在"知道有哪些方案"之后按方案逐个建。
        let vk_schemes = crate::modules::virtual_keyboard::vk_config::scheme_list();
        let vk_scheme_names = vk_schemes.iter().map(|e| e.label.clone()).collect();
        let vk_layout_idx = idx_of_schemes(&vk_schemes, &cfg.virtual_keyboard.layout);
        let vk_controls = Self::vk_controls_for(&vk_schemes, cfg);
        // 降噪单一下拉（`off`/`weak`/`medium`/`strong`）。`Config::load()` 里的
        // `migrate_denoise()` 已经把旧值（`gate`/`spectral`/`rnnoise`）归一到 `medium`，
        // `rnnoise` 是历史遗留取值，现已直接并入中等强度（它就是现在用的 RNNoise
        // 神经网络降噪，当时只是预留名称），这里直接用迁移后的取值即可。
        let denoise_init = cfg.record.mic_denoise.as_str();

        // ── Lua 扩展页签：扫描 `assets/lua`（构建逻辑在 `build_lua_cards`，刷新时复用同一份）。
        let lua_ext_cards = build_lua_cards(cfg, &crate::core::paths::lua_ext_dir());

        let m = Self {
            trail_enabled: signal(t.trail_enabled),
            trail_effect: signal(effect_idx),
            effect_params: signal(effect_params),
            last_effect: Rc::new(RefCell::new(effect_id)),

            hover_enabled: signal(t.hover_enabled),
            hover_type: signal(hover_type_idx),
            // 设置页 · 工作文件夹：自动清理下拉下标。
            temp_cleanup: signal(temp_cleanup_idx),
            admin_launch: signal(cfg.general.admin_launch),
            hover_image: signal(t.hover_image.clone()),
            hover_params: signal(hover_cells),
            last_hover_type: Rc::new(RefCell::new(hover_type_id)),
            hover_catalog,
            clickfx_catalog: clickfx_catalog.clone(),
            // 下拉首项「关闭」+ 目录效果名（下标契约：combo 0=关闭，1..=choices[i-1]）。
            ripple_choices: signal(
                std::iter::once("关闭".to_string())
                    .chain(clickfx_catalog.display_names())
                    .collect(),
            ),

            ripple: RIPPLE_KEYS
                .iter()
                .map(|key| {
                    let b = t.ripple_bindings.get(*key);
                    let (script_id, params) = migrate_clickfx(b, &clickfx_catalog);
                    let effect_idx = clickfx_catalog.index_of(&script_id);
                    let cells: Vec<EffectParamCell> = clickfx_catalog
                        .schema_for(&script_id)
                        .iter()
                        .map(|p| cell_from_param(p, &params))
                        .collect();
                    // 组合下标 = 启用 ? 效果下标+1 : 0（「关闭」）。
                    let combo = if b.is_some_and(|r| r.enabled) {
                        effect_idx + 1
                    } else {
                        0
                    };
                    RippleKeySignals {
                        combo: signal(combo),
                        clickfx_params: signal(cells),
                        last_clickfx: Rc::new(RefCell::new(script_id)),
                    }
                })
                .collect(),
            ripple_master: signal(t.ripple_enabled),

            sound: BUTTON_KEYS
                .iter()
                .map(|key| {
                    let b = t.sound_bindings.get(*key);
                    let file = b.map(|s| s.file.clone()).unwrap_or_default();
                    // 组合下标 = 启用 ? 声音下标+1 : 0（「关闭」）。文件已从目录消失时
                    // `index_of` 回退 0（第一个 wav）—— 与旧版"下拉解析"行为一致，
                    // 下一次 apply 会把 file 修正成真实存在的名字。
                    let combo = if b.is_some_and(|s| s.enabled) {
                        sound_catalog.index_of(&file) + 1
                    } else {
                        0
                    };
                    SoundKeySignals {
                        combo: signal(combo),
                    }
                })
                .collect(),
            // 共享音量初值：第一个启用键的音量（老配置各键可能调得不一样，取在用的那格）；
            // 全关时退回 legacy 全局音量字段。
            sound_volume: signal(v2s(
                t.sound_bindings
                    .values()
                    .find(|s| s.enabled)
                    .map(|s| s.volume)
                    .unwrap_or(t.sound_volume) as f32,
                0.0,
                100.0,
            )),
            sound_volume_text: signal(format!(
                "{:.0}",
                t.sound_bindings
                    .values()
                    .find(|s| s.enabled)
                    .map(|s| s.volume)
                    .unwrap_or(t.sound_volume)
            )),
            sound_master: signal(t.sound_enabled),
            // 下拉首项「关闭」+ 目录文件名（带扩展名显示）。
            sound_choices: signal(
                std::iter::once("关闭".to_string())
                    .chain(sound_catalog.display_names())
                    .collect(),
            ),
            sound_catalog,

            // ── Lua 扩展页签 ──
            lua_ext_cards: signal(lua_ext_cards),
            card_expanded,
            no_expand: signal(false),

            placement: signal(idx_of(PLACEMENTS, &k.placement)),
            // ☆ 收敛读端：水平/垂直两根信号分别取方位对应侧的字段。
            // 老配置升级不丢值：方位没变时显示的就是旧值；方位变了
            // 会读到另一侧（老配置里那侧本就是无效项，语义等价于 0）。
            offset_top: signal(v2s(0.0, 0.0, 50.0)),
            offset_bottom: signal(v2s(
                if k.placement.starts_with("top") {
                    k.top_offset
                } else {
                    k.bottom_offset
                } as f32,
                0.0,
                50.0,
            )),
            offset_left: signal(v2s(
                if k.placement.ends_with("left") {
                    k.left_offset
                } else {
                    k.right_offset
                } as f32,
                0.0,
                50.0,
            )),
            offset_right: signal(v2s(0.0, 0.0, 50.0)),
            // 镜像文本初值 = 滑条信号来源的那份配置值（§35：from_config 是唯一初始化
            // 点，保证"开面板即所见即配置"；整数按 0 位、浮点按各自显示位数格式化）。
            offset_left_text: signal(format!(
                "{:.0}",
                if k.placement.ends_with("left") {
                    k.left_offset
                } else {
                    k.right_offset
                }
            )),
            offset_bottom_text: signal(format!(
                "{:.0}",
                if k.placement.starts_with("top") {
                    k.top_offset
                } else {
                    k.bottom_offset
                }
            )),
            popup_width: signal(v2s(k.popup_width as f32, 5.0, 50.0)),
            popup_width_text: signal(format!("{:.0}", k.popup_width)),
            key_opacity: signal(v2s(k.opacity, 0.05, 1.0)),
            key_opacity_text: signal(format!("{:.2}", k.opacity)),
            fade_delay: signal(v2s(k.fade_delay as f32, 1.0, 30.0)),
            fade_delay_text: signal(format!("{:.0}", k.fade_delay)),
            merge_interval: signal(v2s(k.merge_interval, 0.5, 10.0)),
            merge_interval_text: signal(format!("{:.1}", k.merge_interval)),
            font_size: signal(v2s(k.font_size as f32, 8.0, 72.0)),
            font_size_text: signal(format!("{:.0}", k.font_size)),
            color_font: signal(array_to_color(k.font_color)),
            color_fun: signal(array_to_color(k.fun_color)),
            color_symbol: signal(array_to_color(k.symbol_color)),
            color_bg: signal(array_to_color(k.bg_color)),
            display_mode: signal(idx_of(DISPLAY_MODES, &k.display_mode)),

            pen1_color: signal(hex_to_color(&pens[0].color)),
            pen1_width: signal(v2s(pens[0].width as f32, 1.0, 500.0)),
            pen1_width_text: signal(format!("{:.0}", pens[0].width)),
            pen1_style: signal(idx_of(PEN_STYLES, &pens[0].style)),
            pen2_color: signal(hex_to_color(&pens[1].color)),
            pen2_width: signal(v2s(pens[1].width as f32, 1.0, 500.0)),
            pen2_width_text: signal(format!("{:.0}", pens[1].width)),
            pen2_style: signal(idx_of(PEN_STYLES, &pens[1].style)),
            pen3_color: signal(hex_to_color(&pens[2].color)),
            pen3_width: signal(v2s(pens[2].width as f32, 1.0, 500.0)),
            pen3_width_text: signal(format!("{:.0}", pens[2].width)),
            pen3_style: signal(idx_of(PEN_STYLES, &pens[2].style)),
            hk_screen: signal(d.hotkeys.draw_hk_screen.clone()),
            hk_white: signal(d.hotkeys.draw_hk_white.clone()),
            hk_pen1: signal(d.hotkeys.draw_hk_pen1.clone()),
            hk_pen2: signal(d.hotkeys.draw_hk_pen2.clone()),
            hk_pen3: signal(d.hotkeys.draw_hk_pen3.clone()),
            hk_line: signal(d.hotkeys.draw_hk_line.clone()),
            hk_arrow: signal(d.hotkeys.draw_hk_arrow.clone()),
            hk_rect: signal(d.hotkeys.draw_hk_rect.clone()),
            hk_ellipse: signal(d.hotkeys.draw_hk_ellipse.clone()),
            hk_text: signal(d.hotkeys.draw_hk_text.clone()),
            hk_stamp: signal(d.hotkeys.draw_hk_stamp.clone()),
            hk_mosaic: signal(d.hotkeys.draw_hk_mosaic.clone()),
            hk_eraser: signal(d.hotkeys.draw_hk_eraser.clone()),

            hk_shot: signal(cfg.screenshot.hotkey.clone()),
            hk_shot_pin: signal(cfg.screenshot.hotkey_pin.clone()),
            hk_pin_clipboard: signal(cfg.screenshot.hotkey_pin_clipboard.clone()),
            hk_shot_scroll: signal(cfg.screenshot.hotkey_scroll.clone()),
            hk_shot_scroll_wide: signal(cfg.screenshot.hotkey_scroll_wide.clone()),
            hk_record: signal(cfg.record.hotkey.clone()),
            hk_gif: signal(cfg.gif.hotkey.clone()),
            hk_gif_trim: signal(cfg.gif.trim_hotkey.clone()),
            hk_record_region: signal(cfg.record.region_hotkey.clone()),
            hk_record_trim: signal(cfg.record.trim_hotkey.clone()),
            hk_timer: signal(cfg.global.timer.hotkey.clone()),
            hk_fullscreen_zoom: signal(cfg.magnifier_fullscreen.hotkey.clone()),
            hk_pip_toggle: signal(cfg.magnifier_pip.hotkey.clone()),
            ksound_enabled: signal(cfg.key_sound.enabled),
            ksound_volume: signal(v2s(cfg.key_sound.volume as f32, 0.0, 100.0)),
            ksound_volume_text: signal(format!("{:.0}", cfg.key_sound.volume)),
            // 脚本目录扫描（面板每次打开重建 PanelModel 时重扫，用户丢新脚本即可选到）。
            // 下拉存下标：配置里的 stem 不在列表里（被删了）时落 0，不 panic。
            ksound_script: signal(ksound_catalog.index_of(&cfg.key_sound.script)),
            ksound_params: signal(ks_cells),
            last_ksound_script: Rc::new(RefCell::new(cfg.key_sound.script.clone())),
            ksound_catalog,
            // 容量存**下标**（与帧率/画质同款）：档位表以后调整不会让存量配置错位。
            pin_cap: signal(idx_of(
                &crate::core::config::PIN_CAP_LABELS,
                &cfg.screenshot.pin_ring_cap.to_string(),
            )),

            rec_fps: signal(idx_of(
                &crate::core::config::RECORD_FPS_LABELS,
                &cfg.record.fps.to_string(),
            )),
            rec_resolution: signal(
                (cfg.record.resolution as usize)
                    .min(crate::core::config::RECORD_RESOLUTION_LABELS.len() - 1),
            ),
            rec_hardware: signal(
                (cfg.record.hardware as usize)
                    .min(crate::core::config::RECORD_HARDWARE_LABELS.len() - 1),
            ),
            // 配置存的是自描述取值（"auto"/"8000"），面板用下标 —— 键表就是
            // `RECORD_BITRATE_VALUES`，两处不会各写一份偏移量。
            rec_bitrate: signal(idx_of(
                &crate::core::config::RECORD_BITRATE_VALUES,
                &cfg.record.bitrate,
            )),
            rec_cursor: signal(cfg.record.show_cursor),
            rec_mic_source: signal(mic_source_idx),
            rec_speaker_source: signal(speaker_source_idx),
            rec_denoise: signal(idx_of(
                &crate::core::config::MIC_DENOISE_VALUES,
                denoise_init,
            )),
            // 试听永远是关的初值：它不落盘，开面板时"上次还在听"不是用户此刻的意图。
            rec_monitor: signal(false),
            rec_mic_devices: mic_devices,
            rec_speaker_devices: speaker_devices,
            rec_mic_default: mic_default,
            rec_speaker_default: speaker_default,

            gif_fps: signal(idx_of(
                &crate::core::config::GIF_FPS_LABELS,
                &cfg.gif.fps.to_string(),
            )),
            gif_size: signal(
                (cfg.gif.size as usize).min(crate::core::config::GIF_SIZE_LABELS.len() - 1),
            ),
            gif_cursor: signal(cfg.gif.show_cursor),
            gif_loop: signal(cfg.gif.loop_forever),

            vk_enabled: signal(cfg.virtual_keyboard.enabled),
            // 方案目录扫描（目录即来源）：下拉里就是这里扫到的东西。
            vk_schemes,
            vk_scheme_names: signal(vk_scheme_names),
            vk_layout: signal(vk_layout_idx),
            vk_placement: signal(idx_of(PLACEMENTS, &cfg.virtual_keyboard.placement)),
            // ☆ 收敛读端（与击键回显同款）：取方位对应侧。
            vk_offset_top: signal(v2s(0.0, 0.0, 50.0)),
            vk_offset_bottom: signal(v2s(
                if cfg.virtual_keyboard.placement.starts_with("top") {
                    cfg.virtual_keyboard.top_offset
                } else {
                    cfg.virtual_keyboard.bottom_offset
                },
                0.0,
                50.0,
            )),
            vk_offset_left: signal(v2s(
                if cfg.virtual_keyboard.placement.ends_with("left") {
                    cfg.virtual_keyboard.left_offset
                } else {
                    cfg.virtual_keyboard.right_offset
                },
                0.0,
                50.0,
            )),
            vk_offset_right: signal(v2s(0.0, 0.0, 50.0)),
            // ★ 文本位数必须与 `apply_to` 里 `slider_pair_sync(.., decimals)` 一致：
            //   位数对不上时首帧不会回写（滑条没动），但用户一拖就会跳字（"5" → "5.0"）。
            vk_offset_left_text: signal(format!(
                "{:.1}",
                if cfg.virtual_keyboard.placement.ends_with("left") {
                    cfg.virtual_keyboard.left_offset
                } else {
                    cfg.virtual_keyboard.right_offset
                }
            )),
            vk_offset_bottom_text: signal(format!(
                "{:.1}",
                if cfg.virtual_keyboard.placement.starts_with("top") {
                    cfg.virtual_keyboard.top_offset
                } else {
                    cfg.virtual_keyboard.bottom_offset
                }
            )),
            vk_controls: signal(vk_controls),
            vk_click_through: signal(cfg.virtual_keyboard.click_through),
            vk_always_on_top: signal(cfg.overlay.always_on_top),
            vk_fullscreen_avoid: signal(cfg.overlay.avoid_fullscreen),

            hotkey_capture: signal(None),
            script_hotkey_cap: signal(None),

            // 面板页码还原（主窗口状态记忆）。★ 页 id 优先：下标会随页序重排漂移
            // （2026-09-25 从 8 页重排成 7 页），按名字读才稳；老配置没有 id，
            // 走 `legacy_page_index` 把旧下标翻成新下标。越界兜底到第一页，导航不失明。
            page: signal(
                cfg.ui
                    .panel_page_id
                    .as_deref()
                    .and_then(crate::ui::nav::page_index)
                    .unwrap_or_else(|| crate::ui::nav::legacy_page_index(cfg.ui.panel_page)),
            ),
            enabled_key: signal(k.enabled),
            enabled_draw: signal(d.enabled),
            hk_sticky: signal(cfg.sticky_note.hotkey.clone()),
            note_bg_color: signal(array_to_color(cfg.sticky_note.bg_color)),
            note_text_color: signal(array_to_color(cfg.sticky_note.text_color)),
            note_edit_color: signal(array_to_color(cfg.sticky_note.edit_color)),
            note_alpha: signal(v2s(
                cfg.sticky_note.alpha as f32,
                NOTE_ALPHA_MIN as f32,
                NOTE_ALPHA_MAX as f32,
            )),
            note_alpha_text: signal(format!("{:.0}", cfg.sticky_note.alpha)),
            note_font_sz: signal(v2s(cfg.sticky_note.font_size, NOTE_FONT_MIN, NOTE_FONT_MAX)),
            note_font_sz_text: signal(format!("{:.0}", cfg.sticky_note.font_size)),
            panel_hotkey: signal(g.panel_hotkey.clone()),
            timer_mode: signal(match g.timer.mode.as_str() {
                "countdown" => 1,
                _ => 0,
            }),
            // ★★ 这五个滑块**必须**走 `v2s`：滑块内部恒存 `0..=1`，配置里是真实量纲
            //   （秒 / 比例 / 倍数）。直接把真实值塞进去的话，用户一拖，滑块按自己的
            //   `0..=1` 刻度回写，`apply_to` 再把它当真实量纲读 —— 两端量程对不上：
            //      · 倍率：`round()` 后只有 0/1，`clamp(1,4)` 恒为 **1** ⇒ 拖了等于没拖；
            //      · 时长：`round().max(1)` 恒为 **1 秒**；
            //      · 面板宽高：恰好落在 0.1–0.5 区间内，看着"能用"但映射是错的。
            //   这正是 2026-09-18 虚拟键盘那次事故的同一类错误第三次出现，往返用例见
            //   `tests::slider_ranges_roundtrip_through_the_panel`。
            timer_duration: signal(v2s(
                g.timer.duration_sec as f32,
                TIMER_DURATION_MIN as f32,
                TIMER_DURATION_MAX as f32,
            )),
            // 输入框文本 = 配置值的十进制字面量：from_config 是唯一初始化点，保证
            // "开面板即所见即配置"（对账规则见 `apply_to` 的时长块）。
            timer_duration_text: signal(g.timer.duration_sec.to_string()),
            // 计时器位置/大小（2026-09-25）：偏移与字号同样**必须**走 `v2s`（滑块恒存
            // 0..=1，见上方画中画偏移的同一类注释）；锚点是纯下标，直接 `as usize`。
            timer_anchor: signal(g.timer.anchor as usize),
            timer_offset_x: signal(v2s(
                g.timer.offset_x as f32,
                TIMER_OFFSET_MIN as f32,
                TIMER_OFFSET_MAX as f32,
            )),
            timer_offset_x_text: signal(format!("{:.0}", g.timer.offset_x)),
            timer_offset_y: signal(v2s(
                g.timer.offset_y as f32,
                TIMER_OFFSET_MIN as f32,
                TIMER_OFFSET_MAX as f32,
            )),
            timer_offset_y_text: signal(format!("{:.0}", g.timer.offset_y)),
            timer_font: signal(v2s(
                g.timer.font_pt as f32,
                TIMER_FONT_MIN as f32,
                TIMER_FONT_MAX as f32,
            )),
            timer_font_text: signal(format!("{:.0}", g.timer.font_pt)),
            pip_enabled: signal(p.enabled),
            pip_panel_w: signal(v2s(p.panel_width, PIP_PANEL_MIN, PIP_PANEL_MAX)),
            pip_panel_w_text: signal(format!("{:.2}", p.panel_width)),
            pip_panel_h: signal(v2s(p.panel_height, PIP_PANEL_MIN, PIP_PANEL_MAX)),
            pip_panel_h_text: signal(format!("{:.2}", p.panel_height)),
            pip_zoom: signal(v2s(p.zoom as f32, PIP_ZOOM_MIN as f32, PIP_ZOOM_MAX as f32)),
            pip_zoom_text: signal(format!("{:.0}", p.zoom)),
            pip_anchor: signal(p.anchor as usize),
            pip_offset_x: signal(v2s(p.offset_x as f32, 0.0, 50.0)),
            pip_offset_x_text: signal(format!("{:.0}", p.offset_x)),
            pip_offset_y: signal(v2s(p.offset_y as f32, 0.0, 50.0)),
            pip_offset_y_text: signal(format!("{:.0}", p.offset_y)),
            pip_border_color: signal(hex_to_color(&p.border_color)),

            // ── 全屏放大 ── 倍率同样走 `v2s`（滑块恒存 0..=1，见本文件顶部的量纲约定）。
            fs_enabled: signal(f.enabled),
            fs_zoom: signal(v2s(
                f.zoom as f32,
                FULLSCREEN_ZOOM_MIN as f32,
                FULLSCREEN_ZOOM_MAX as f32,
            )),
            fs_zoom_text: signal(format!("{:.0}", f.zoom)),

            // ── 剪贴板历史 ──
            clip_enabled: signal(cfg.clipboard.enabled),
            hk_clipboard: signal(cfg.clipboard.hotkey.clone()),
            clip_max_entries: signal({
                // 配置里的真实值 → 档位下标（找不到回落出厂档 500）。
                let v = cfg.clipboard.max_entries;
                crate::core::config::CLIP_ENTRIES_VALUES
                    .iter()
                    .position(|&x| x == v)
                    .unwrap_or(0)
            }),

            // ── 五卡启用开关（勾上 = 功能可用；都是快捷键触发型，运行时按键时读配置）──
            shot_enabled: signal(cfg.screenshot.enabled),
            gif_enabled: signal(cfg.gif.enabled),
            rec_enabled: signal(cfg.record.enabled),
            sticky_enabled: signal(cfg.sticky_note.enabled),
            timer_enabled: signal(cfg.global.timer.enabled),

            catalog,
        };
        // ★ 开关架构：每个功能卡片一个信号，直接映射各自配置段的开关字段；
        // 没有功能总开关，也没有第二处控件与它共享（不存在互相写回打架的问题）。
        m
    }

    /// 为每个方案各建一组控件信号。
    ///
    /// ★ 逐个方案问一次 `VkConfig`：控件清单（`controls_of`）与每个控件的当前值
    /// （用户改过的 → 面板全局配置 → 方案 → 出厂）都在这一份里，面板不需要自己拼。
    fn vk_controls_for(schemes: &[SchemeEntry], cfg: &Config) -> Vec<VkSchemeControls> {
        schemes
            .iter()
            .enumerate()
            .map(|(index, e)| {
                let mut s = cfg.virtual_keyboard.clone();
                s.layout = e.id.clone();
                let vc = crate::modules::virtual_keyboard::vk_config::VkConfig::load(&s);
                let items = vc
                    .controls()
                    .iter()
                    .map(|d| VkControlItem {
                        def: d.clone(),
                        sig: control_signal(&vc, d),
                    })
                    .collect();
                VkSchemeControls {
                    scheme_id: e.id.clone(),
                    index,
                    items,
                }
            })
            .collect()
    }

    /// 取某张卡片的**展开信号**（id 见 [`crate::core::config::CARD_IDS`]）。
    ///
    /// 页面侧一律用它，别再自己 `signal(...)`：自己建的信号不落盘，切页/重开面板就复位
    /// （`ui::content` 是把五页同时挂上去的，就是因为重建子树会把这些状态抹掉）。
    #[inline]
    pub fn expand(&self, id: &str) -> Signal<bool> {
        self.card_expanded
            .get(id)
            .copied()
            .unwrap_or(self.no_expand)
    }

    /// 快捷键捕获落到的目标信号（见 [`HotkeyField`]）。
    ///
    /// 捕获态下 `Runtime::frame()` 拦截到按键，据 `HotkeyField` 找到这里对应的 `hk_*` 信号
    /// 写回按键串。`hk_*` 信号本就由 `apply_to` 实时 diff 回配置，所以捕获结果会自动落盘。
    #[inline]
    pub fn hotkey_signal(&self, field: HotkeyField) -> &Signal<String> {
        match field {
            HotkeyField::Screen => &self.hk_screen,
            HotkeyField::White => &self.hk_white,
            HotkeyField::Pen1 => &self.hk_pen1,
            HotkeyField::Pen2 => &self.hk_pen2,
            HotkeyField::Pen3 => &self.hk_pen3,
            HotkeyField::Eraser => &self.hk_eraser,
            HotkeyField::Line => &self.hk_line,
            HotkeyField::Arrow => &self.hk_arrow,
            HotkeyField::Rect => &self.hk_rect,
            HotkeyField::Ellipse => &self.hk_ellipse,
            HotkeyField::Text => &self.hk_text,
            HotkeyField::Mosaic => &self.hk_mosaic,
            HotkeyField::Stamp => &self.hk_stamp,
            HotkeyField::Shot => &self.hk_shot,
            HotkeyField::ShotPin => &self.hk_shot_pin,
            HotkeyField::PinClip => &self.hk_pin_clipboard,
            HotkeyField::ShotScroll => &self.hk_shot_scroll,
            HotkeyField::ShotScrollWide => &self.hk_shot_scroll_wide,
            HotkeyField::Record => &self.hk_record,
            HotkeyField::Gif => &self.hk_gif,
            HotkeyField::Trim => &self.hk_gif_trim,
            HotkeyField::RecordRegion => &self.hk_record_region,
            HotkeyField::RecordTrim => &self.hk_record_trim,
            HotkeyField::Timer => &self.hk_timer,
            HotkeyField::FullscreenZoom => &self.hk_fullscreen_zoom,
            HotkeyField::PipToggle => &self.hk_pip_toggle,
            HotkeyField::Sticky => &self.hk_sticky,
            HotkeyField::Clipboard => &self.hk_clipboard,
            HotkeyField::Panel => &self.panel_hotkey,
        }
    }

    /// 写入一个快捷键，**并把所有冲突的旧占用者清空**（后配的赢）。
    ///
    /// # 为什么必须互斥
    ///
    /// 一把键同时挂在两个功能上时行为是**未定义**的：谁生效取决于 `match_drawing` 的
    /// 表顺序与 `handle_action_key` 的分支顺序——用户看到的是"配了却没反应"，而且排查
    /// 时无从下手（两个配置都是对的）。互斥把歧义消掉：**新配的生效、旧的自动留空**
    /// （留空即停用，框里显示「（空）」）。
    ///
    /// ★ 比较走 [`crate::core::hotkeys::normalize`]（忽略顺序 / 空白 / 大小写），与匹配
    /// 时同一套口径——`Ctrl+Shift+D` 与 `Shift+Ctrl+D` 必须判为冲突，否则用户换个写法
    /// 就能绕过互斥。
    ///
    /// 字段全集取自 [`crate::ui::pages::global::HOTKEY_ENTRIES`]（设置页的显示顺序表）：
    /// 新增一把快捷键必须先在那里登记，于是"设置页看得到的"与"参与互斥的"天然是
    /// 同一个集合，不需要再维护第二张表。
    ///
    /// 返回被清空的字段与其显示名（供日志与提示）。
    pub fn set_hotkey_exclusive(
        &self,
        field: HotkeyField,
        value: String,
    ) -> Vec<(HotkeyField, &'static str)> {
        let norm = crate::core::hotkeys::normalize(&value);
        let mut cleared = Vec::new();
        if !norm.is_empty() {
            for (label, other) in crate::ui::pages::global::HOTKEY_ENTRIES {
                if *other == field {
                    continue;
                }
                if crate::core::hotkeys::normalize(&self.hotkey_signal(*other).get()) == norm {
                    self.hotkey_signal(*other).set(String::new());
                    cleared.push((*other, *label));
                }
            }
            // 脚本热键也参与互斥（2026-09-29）：这把键若挂在某脚本的 `hotkey` 参数上，
            // 一并清空（后配的赢）——否则脚本热键与功能热键同抢一把键，谁生效看派发
            // 顺序，用户看到的是"配了没反应"。
            self.clear_conflicting_script_hotkeys(&norm, None);
        }
        self.hotkey_signal(field).set(value);
        cleared
    }

    /// 脚本热键捕获落盘（2026-09-29）：写回目标 sig，**并清空所有冲突占用者**。
    ///
    /// 互斥范围与 [`Self::set_hotkey_exclusive`] 同一套口径（比较走
    /// `normalize`，忽略顺序/空白/大小写）：固定字段 + 其它脚本卡的 hotkey 参数
    /// （目标自己除外——sig 判等）。返回被清空的固定字段显示名（供日志）。
    pub fn set_script_hotkey_exclusive(
        &self,
        target: &ScriptHotkeyCap,
        value: String,
    ) -> Vec<&'static str> {
        let norm = crate::core::hotkeys::normalize(&value);
        let mut cleared = Vec::new();
        if !norm.is_empty() {
            for (label, other) in crate::ui::pages::global::HOTKEY_ENTRIES {
                if crate::core::hotkeys::normalize(&self.hotkey_signal(*other).get()) == norm {
                    self.hotkey_signal(*other).set(String::new());
                    cleared.push(*label);
                }
            }
            // 其它脚本卡的同值热键也清（sig 相等 = 目标自己，跳过）。
            self.clear_conflicting_script_hotkeys(&norm, Some(&target.sig));
        }
        target.sig.set(value);
        cleared
    }

    /// 清空与 `norm` 冲突的**脚本热键参数**（`EffectParamCell::Hotkey`）。
    ///
    /// `keep = Some(&目标sig)` 时跳过该信号（正在给它写新值的场景）；
    /// `keep = None` 时全清（固定字段捕获的场景）。
    fn clear_conflicting_script_hotkeys(&self, norm: &str, keep: Option<&Signal<String>>) {
        for card in self.lua_ext_cards.get() {
            for cell in card.params.get() {
                if let EffectParamCell::Hotkey { sig, .. } = &cell {
                    if keep.is_some_and(|k| *k == *sig) {
                        continue;
                    }
                    if crate::core::hotkeys::normalize(&sig.get()) == norm {
                        sig.set(String::new());
                    }
                }
            }
        }
    }

    /// 麦克风试听是否开启（**只读**：这不是配置项，只用来驱动 `Runtime` 的监听线程）。
    #[inline]
    pub fn mic_monitor_on(&self) -> bool {
        self.rec_monitor.get()
    }

    /// 把信号 diff 回 `cfg`；返回是否有改动（有改动由调用方 bump 版本 + 落盘）。
    pub fn apply_to(&self, cfg: &mut Config) -> bool {
        let mut changed = false;

        // ── 拖尾效果卡片开关 ──
        let on = self.trail_enabled.get();
        if cfg.trail.trail_enabled != on {
            cfg.trail.trail_enabled = on;
            changed = true;
        }
        let effect = self.catalog.id_at(self.trail_effect.get());
        if !effect.is_empty() && cfg.trail.trail_effect != effect {
            cfg.trail.trail_effect = effect;
            changed = true;
        }

        // ── 拖尾参数（动态，按选中特效 schema）──
        let effect_id = self.catalog.id_at(self.trail_effect.get());
        // 选中的特效变了 → 整批重建参数单元（host_signal 会随之重建页面段落）。
        // ★ §38「只存当前方案」：切方案 = 旧方案的落盘参数**全部丢弃**，新方案从
        //   脚本 schema 默认起步（清空 extra 里所有命名空间键后重建，单元 initial
        //   即 schema 默认；随后写回循环发现 new==initial 不落键，配置保持干净）。
        //   同一方案的调参照常保留，与旧语义的分界只在"切换"这一下。
        if effect_id != *self.last_effect.borrow() {
            *self.last_effect.borrow_mut() = effect_id.clone();
            let before = cfg.trail.trail_settings.extra.len();
            cfg.trail
                .trail_settings
                .extra
                .retain(|k, _| !k.contains('.'));
            if cfg.trail.trail_settings.extra.len() != before {
                changed = true;
            }
            let module_ts = crate::modules::trail::settings::from_config(&cfg.trail);
            let pm = module_ts.effect_param_map(&effect_id);
            let cells = self
                .catalog
                .schema_for(&effect_id)
                .iter()
                .map(|p| cell_from_param(p, &pm))
                .collect::<Vec<_>>();
            self.effect_params.set(cells);
        }
        // ★ 同口径对账（2026-09-28）：extra 的命名空间键只留**当前特效**的——切换事件
        //   错过时（老配置残留、脚本被删）也兜得住。非命名空间键不属于任何脚本，保留。
        if !effect_id.is_empty() {
            let before = cfg.trail.trail_settings.extra.len();
            let prefix = format!("{effect_id}.");
            cfg.trail
                .trail_settings
                .extra
                .retain(|k, _| !k.contains('.') || k.starts_with(&prefix));
            if cfg.trail.trail_settings.extra.len() != before {
                changed = true;
            }
        }
        // 把当前参数单元的值写回配置（命名空间键 `{特效id}.{参数key}`，落在 TrailDetail.extra）。
        let extra = &mut cfg.trail.trail_settings.extra;
        let mut params_changed = false;
        for cell in self.effect_params.get() {
            let key = format!("{}.{}", effect_id, cell.key());
            let new_val: Value = match &cell {
                EffectParamCell::Float {
                    sig,
                    text,
                    min,
                    max,
                    step,
                    decimals,
                    ..
                } => {
                    let q = float_param_sync(
                        extra, &key, &cell, sig, text, *min, *max, *step, *decimals,
                    );
                    json_decimal(q, *decimals)
                }
                EffectParamCell::Int {
                    sig,
                    text,
                    min,
                    max,
                    ..
                } => {
                    let q = float_param_sync(extra, &key, &cell, sig, text, *min, *max, 0.0, 0);
                    json!(q.round() as i64)
                }
                EffectParamCell::Bool { sig, .. } => json!(sig.get()),
                EffectParamCell::Choice { sig, choices, .. } => {
                    json!(choices.get(sig.get()).cloned().unwrap_or_default())
                }
                // 颜色：位数由脚本声明的 `alpha` 决定（见 `fmt_color`）。
                EffectParamCell::Color { sig, alpha, .. } => json!(fmt_color(sig.get(), *alpha)),
                // 音效/目录是击键音效专用的两类参数（拖尾脚本不会声明），这里一并按
                // "当前候选项 / 当前串"落盘，保持 match 穷尽。
                EffectParamCell::Sound { sig, choices, .. } => {
                    json!(choices.get(sig.get()).cloned().unwrap_or_default())
                }
                EffectParamCell::Folder { sig, .. } => json!(sig.get()),
                EffectParamCell::Image { sig, .. }
                | EffectParamCell::Str { sig, .. }
                | EffectParamCell::File { sig, .. }
                | EffectParamCell::Hotkey { sig, .. } => json!(sig.get()),
            };
            if apply_param_value(extra, &cell, new_val, &key) {
                params_changed = true;
            }
        }
        if params_changed {
            changed = true;
        }

        // ── Lua 扩展脚本卡片：每帧 diff 每张卡的参数写回 `config.lua_ext.params[id]`，
        //   折叠态写回 `config.ui.expanded["lua.<id>"]`。包卡额外处理下拉选择与
        //   `composite_active`：实际运行的是“当前子脚本 id”，不是包 id。
        for card in self.lua_ext_cards.get() {
            if card.id.is_empty() {
                continue;
            }
            let (param_id, on) = if let Some(sel) = &card.selector {
                let effect = {
                    let effects = sel.effects.borrow();
                    let idx = sel.selected.get().min(effects.len().saturating_sub(1));
                    let Some(effect) = effects.get(idx).cloned() else {
                        continue;
                    };
                    effect
                };
                let child_id = effect.id.clone();
                if *sel.current.borrow() != child_id {
                    *sel.current.borrow_mut() = child_id.clone();
                    // ★ 2026-10-26 用户口径：包卡换子脚本 = 恢复该脚本的**出厂默认**，
                    //   只保留“当前脚本”的设定。与内置鼠标特效卡片「切换即刷新」一致：
                    //   先清掉本包所有子脚本的参数，再用空表重建 schema 默认。
                    let prefix = format!("{}/", card.id);
                    cfg.lua_ext.params.retain(|id, _| !id.starts_with(&prefix));
                    let cells = effect
                        .params
                        .iter()
                        .map(|p| cell_from_param(p, &HashMap::new()))
                        .collect::<Vec<_>>();
                    card.params.set(cells);
                    changed = true;
                }
                // ★ 出厂固定点：下拉停在第一项时**不落盘**（缺键 = 默认第一项）。
                //   否则任何 `Config::default()` 的 `from_config → apply_to` 都会因
                //   包卡首次出现而写出一条 `pack_selected`，把"没动过"判成改动。
                let stored_sel = cfg.lua_ext.pack_selected.get(&card.id).cloned();
                let is_default = child_id == sel.default_id;
                if stored_sel.as_deref() != Some(child_id.as_str()) {
                    if is_default && stored_sel.is_none() {
                        // 缺键就是出厂默认项，保持零冗余。
                    } else if is_default {
                        cfg.lua_ext.pack_selected.remove(&card.id);
                        changed = true;
                    } else {
                        cfg.lua_ext
                            .pack_selected
                            .insert(card.id.clone(), child_id.clone());
                        changed = true;
                    }
                }
                let on = card.toggle.get();
                let before = cfg.lua_ext.composite_active.clone();
                let effect_ids = sel
                    .effects
                    .borrow()
                    .iter()
                    .map(|e| e.id.clone())
                    .collect::<Vec<_>>();
                cfg.lua_ext
                    .composite_active
                    .retain(|x| !effect_ids.iter().any(|id| id == x));
                if on && !cfg.lua_ext.composite_active.iter().any(|x| x == &child_id) {
                    cfg.lua_ext.composite_active.push(child_id.clone());
                }
                if cfg.lua_ext.composite_active != before {
                    changed = true;
                }
                (child_id, on)
            } else {
                (card.id.clone(), card.toggle.get())
            };

            let mut stored = cfg
                .lua_ext
                .params
                .get(&param_id)
                .cloned()
                .unwrap_or_default();
            let mut card_changed = false;
            for cell in card.params.get() {
                let key = cell.key().to_string();
                let new_val: Value = match &cell {
                    EffectParamCell::Float {
                        sig,
                        text,
                        min,
                        max,
                        step,
                        decimals,
                        ..
                    } => {
                        let q = float_param_sync(
                            &stored, &key, &cell, sig, text, *min, *max, *step, *decimals,
                        );
                        json_decimal(q, *decimals)
                    }
                    EffectParamCell::Int {
                        sig,
                        text,
                        min,
                        max,
                        ..
                    } => {
                        let q =
                            float_param_sync(&stored, &key, &cell, sig, text, *min, *max, 0.0, 0);
                        json!(q.round() as i64)
                    }
                    EffectParamCell::Bool { sig, .. } => json!(sig.get()),
                    EffectParamCell::Choice { sig, choices, .. } => {
                        json!(choices.get(sig.get()).cloned().unwrap_or_default())
                    }
                    EffectParamCell::Color { sig, alpha, .. } => {
                        json!(fmt_color(sig.get(), *alpha))
                    }
                    EffectParamCell::Sound { sig, choices, .. } => {
                        json!(choices.get(sig.get()).cloned().unwrap_or_default())
                    }
                    EffectParamCell::Folder { sig, .. } => json!(sig.get()),
                    EffectParamCell::Image { sig, .. }
                    | EffectParamCell::Str { sig, .. }
                    | EffectParamCell::File { sig, .. }
                    | EffectParamCell::Hotkey { sig, .. } => json!(sig.get()),
                };
                if apply_param_value(&mut stored, &cell, new_val, &key) {
                    card_changed = true;
                }
            }
            if card_changed {
                cfg.lua_ext.params.insert(param_id.clone(), stored);
                changed = true;
            }
            // 折叠态：与出厂默认（折叠）不同才落盘；回到默认则移除键。
            let key_name = card.expanded_key();
            let v = card.expanded.get();
            let stored = cfg.ui.expanded.get(&key_name).copied();
            if stored.unwrap_or(false) != v {
                if v {
                    cfg.ui.expanded.insert(key_name, v);
                } else {
                    cfg.ui.expanded.remove(&key_name);
                }
                changed = true;
            }
            // 普通脚本：标题开关按 role 落到各自列表。包卡在上面的分支里已经
            // 按“当前子脚本 id”维护 `composite_active`，这里不能再按包 id 写一次。
            if card.selector.is_none() && cfg.lua_ext.role_is_active(&card.role, &card.id) != on {
                cfg.lua_ext.set_role_active(&card.role, &card.id, on);
                changed = true;
            }
        }

        // ── 悬浮标 ──
        // 卡片开关 → hover_enabled；类型下拉 → hover_type（换类型时整批重建参数单元）；
        // 参数单元只把**相对初值有变化**的键写回 hover_params[type]（未动的键留给脚本默认值，
        // 也保证“开面板不改任何东西”不会产生伪落盘）。
        if cfg.trail.hover_enabled != self.hover_enabled.get() {
            cfg.trail.hover_enabled = self.hover_enabled.get();
            changed = true;
        }
        let hover_id = self.hover_catalog.id_at(self.hover_type.get());
        // ★ 换脚本即刷新（2026-09-28 用户口径）：配置里只保留**当前选中脚本**的参数。
        //   其余条目——换掉的、老配置残留的带前缀键、脚本已被删除的——一律整条删除。
        //   于是"切回来"也拿不到旧调参：每次选中都从脚本 schema 默认起步。
        if !hover_id.is_empty() {
            let before = cfg.trail.hover_params.len();
            cfg.trail.hover_params.retain(|k, _| {
                k == &hover_id || k.rsplit('/').next().is_some_and(|b| b == hover_id)
            });
            if cfg.trail.hover_params.len() != before {
                changed = true;
            }
        }
        if hover_id != *self.last_hover_type.borrow() {
            *self.last_hover_type.borrow_mut() = hover_id.clone();
            // 上面刚 prune 过，新选中脚本在配置里必然无键；这里显式传空表重建，
            // 单元 initial 即 schema 默认——语义就是"切换 = 刷新"。
            let cells = self
                .hover_catalog
                .schema_for(&hover_id)
                .iter()
                .map(|p| cell_from_param(p, &HashMap::new()))
                .collect::<Vec<_>>();
            self.hover_params.set(cells);
        }
        if cfg.trail.hover_type != hover_id {
            cfg.trail.hover_type = hover_id.clone();
            changed = true;
        }
        let mut hover_params_changed = false;
        for cell in self.hover_params.get() {
            let new_val: Value = match &cell {
                EffectParamCell::Float {
                    sig,
                    text,
                    min,
                    max,
                    step,
                    decimals,
                    ..
                } => {
                    // referee 现场读（`hover_extra` 的 entry 在 match 之后才建，这里
                    // 不能提前持有可变借用）；没有键就回退 initial（见 helper 注释）。
                    let empty = HashMap::new();
                    let stored = cfg.trail.hover_params.get(&hover_id).unwrap_or(&empty);
                    let q = float_param_sync(
                        stored,
                        cell.key(),
                        &cell,
                        sig,
                        text,
                        *min,
                        *max,
                        *step,
                        *decimals,
                    );
                    json_decimal(q, *decimals)
                }
                EffectParamCell::Int {
                    sig,
                    text,
                    min,
                    max,
                    ..
                } => {
                    let empty = HashMap::new();
                    let stored = cfg.trail.hover_params.get(&hover_id).unwrap_or(&empty);
                    let q =
                        float_param_sync(stored, cell.key(), &cell, sig, text, *min, *max, 0.0, 0);
                    json!(q.round() as i64)
                }
                EffectParamCell::Bool { sig, .. } => json!(sig.get()),
                EffectParamCell::Choice { sig, choices, .. } => {
                    json!(choices.get(sig.get()).cloned().unwrap_or_default())
                }
                EffectParamCell::Color { sig, alpha, .. } => json!(fmt_color(sig.get(), *alpha)),
                // 同上：音效/目录是击键音效专用，保持 match 穷尽。
                EffectParamCell::Sound { sig, choices, .. } => {
                    json!(choices.get(sig.get()).cloned().unwrap_or_default())
                }
                EffectParamCell::Folder { sig, .. } => json!(sig.get()),
                EffectParamCell::Image { sig, .. }
                | EffectParamCell::Str { sig, .. }
                | EffectParamCell::File { sig, .. }
                | EffectParamCell::Hotkey { sig, .. } => json!(sig.get()),
            };
            // ★ 判据统一走 `apply_param_value`：与配置现值对账（见那里的注释）。
            let hover_extra = cfg.trail.hover_params.entry(hover_id.clone()).or_default();
            if apply_param_value(hover_extra, &cell, new_val, cell.key()) {
                hover_params_changed = true;
            }
        }
        // 一张参数都没剩下就把整张空表摘掉，别在 config.json 里留 `"ruler": {}`。
        if cfg
            .trail
            .hover_params
            .get(&hover_id)
            .is_some_and(|m| m.is_empty())
        {
            cfg.trail.hover_params.remove(&hover_id);
        }
        if hover_params_changed {
            changed = true;
        }
        // PNG 图片样式：写回图片标识（文件名或绝对路径）。
        let hi = self.hover_image.get().trim().to_string();
        if cfg.trail.hover_image != hi {
            cfg.trail.hover_image = hi;
            changed = true;
        }

        // ── 点击效果（单卡：总开关 + 五键「关闭/效果」下拉，参数各自独立）──
        // 总开关先落（AND 闸：运行期把 master 乘进每键 enabled，见
        // `modules::trail::settings::from_config`）。
        let master = self.ripple_master.get();
        if cfg.trail.ripple_enabled != master {
            cfg.trail.ripple_enabled = master;
            changed = true;
        }
        for (i, key) in RIPPLE_KEYS.iter().enumerate() {
            let ks = &self.ripple[i];
            let entry = cfg
                .trail
                .ripple_bindings
                .entry((*key).to_string())
                .or_default();
            // 组合下拉：0 = 关闭（只关 enabled）；1.. = 目录效果。
            // ★ 关闭态沿用 `last_clickfx`（面板侧解析出的"当前脚本"，老配置 style 已在
            //   建模时解析）而不是 entry.script——这样关闭不回写空串、参数单元不被
            //   `schema_for("")` 清空，"上次的效果"也留在配置里，重选即回。
            let script_id = if ks.combo.get() == 0 {
                ks.last_clickfx.borrow().clone()
            } else {
                let n = self.clickfx_catalog.display_names().len();
                self.clickfx_catalog
                    .id_at((ks.combo.get() - 1).min(n.saturating_sub(1)))
            };
            if apply_ripple_key(ks, entry, &script_id) {
                changed = true;
            }
            // 选中脚本变化 → 整批重建参数单元。
            // ★ 只记录当前方案（2026-09-25 用户口径）：换效果先**清空** script_params
            //   再从 schema 默认值重建。裸参数名共键的 map 若不清，ring 调过的 color
            //   会种进 thin 的初值（跨效果泄漏），切回 ring 还会复活旧调参——等于
            //   偷偷给每个效果记一套状态。换效果即弃；未动过的新参数也不落盘（写回
            //   只落相对初值有变化的键）。关闭（combo=0）不算换效果：script 与参数
            //   都保留（"关闭不丢上次的效果"，见上）。
            if script_id != *self.ripple[i].last_clickfx.borrow() {
                *self.ripple[i].last_clickfx.borrow_mut() = script_id.clone();
                entry.script_params.clear();
                let cells = self
                    .clickfx_catalog
                    .schema_for(&script_id)
                    .iter()
                    .map(|p| cell_from_param(p, &entry.script_params))
                    .collect::<Vec<_>>();
                self.ripple[i].clickfx_params.set(cells);
            }
            // 写回脚本参数（只写相对初值有变化的键，未动的键留给脚本默认值）。
            let mut cf_changed = false;
            for cell in self.ripple[i].clickfx_params.get() {
                let new_val: Value = match &cell {
                    EffectParamCell::Float {
                        sig,
                        text,
                        min,
                        max,
                        step,
                        decimals,
                        ..
                    } => {
                        let q = float_param_sync(
                            &entry.script_params,
                            cell.key(),
                            &cell,
                            sig,
                            text,
                            *min,
                            *max,
                            *step,
                            *decimals,
                        );
                        json_decimal(q, *decimals)
                    }
                    EffectParamCell::Int {
                        sig,
                        text,
                        min,
                        max,
                        ..
                    } => {
                        let q = float_param_sync(
                            &entry.script_params,
                            cell.key(),
                            &cell,
                            sig,
                            text,
                            *min,
                            *max,
                            0.0,
                            0,
                        );
                        json!(q.round() as i64)
                    }
                    EffectParamCell::Bool { sig, .. } => json!(sig.get()),
                    EffectParamCell::Choice { sig, choices, .. } => {
                        json!(choices.get(sig.get()).cloned().unwrap_or_default())
                    }
                    EffectParamCell::Color { sig, alpha, .. } => {
                        json!(fmt_color(sig.get(), *alpha))
                    }
                    // 同上：音效/目录是击键音效专用，保持 match 穷尽。
                    EffectParamCell::Sound { sig, choices, .. } => {
                        json!(choices.get(sig.get()).cloned().unwrap_or_default())
                    }
                    EffectParamCell::Folder { sig, .. } => json!(sig.get()),
                    EffectParamCell::Image { sig, .. }
                    | EffectParamCell::Str { sig, .. }
                    | EffectParamCell::File { sig, .. }
                    | EffectParamCell::Hotkey { sig, .. } => json!(sig.get()),
                };
                if cell.initial() == &new_val {
                    continue; // 用户没动过的参数不落盘
                }
                if entry.script_params.get(cell.key()) != Some(&new_val) {
                    entry.script_params.insert(cell.key().to_string(), new_val);
                    cf_changed = true;
                }
            }
            if cf_changed {
                changed = true;
            }
        }

        // ── 点击声音（单卡：总开关 + 共享音量 + 五键「关闭/文件」下拉）──
        let sound_len = self.sound_catalog.display_names().len();
        // 共享音量（滑条装比例、配置装真实量纲；§35 三件套：文本赢时配置跟文本）：
        // 写五键 + legacy 字段保持同步。
        let vol = slider_pair_sync(
            &self.sound_volume,
            &self.sound_volume_text,
            cfg.trail.sound_volume as f32,
            0.0,
            100.0,
            1.0,
            0,
        )
        .round()
        .clamp(0.0, 100.0) as u32;
        for (i, key) in BUTTON_KEYS.iter().enumerate() {
            let ks = &self.sound[i];
            let entry = cfg
                .trail
                .sound_bindings
                .entry((*key).to_string())
                .or_default();
            // 组合下拉：0 = 关闭（只关 enabled，保留 entry.file——"上次的声音"）；
            // 1.. = 目录文件（目录空时写空串，该键静音，而不是硬写不存在的名字）。
            let file = if ks.combo.get() == 0 {
                entry.file.clone()
            } else if sound_len == 0 {
                String::new()
            } else {
                self.sound_catalog
                    .id_at((ks.combo.get() - 1).min(sound_len - 1))
            };
            if apply_sound_key(ks, entry, &file) {
                changed = true;
            }
            if entry.volume != vol {
                entry.volume = vol;
                changed = true;
            }
        }
        if cfg.trail.sound_volume != vol {
            cfg.trail.sound_volume = vol;
            changed = true;
        }
        let master = self.sound_master.get();
        if cfg.trail.sound_enabled != master {
            cfg.trail.sound_enabled = master;
            changed = true;
        }

        // ── 击键回显 ──
        if cfg.key_display.enabled != self.enabled_key.get() {
            cfg.key_display.enabled = self.enabled_key.get();
            changed = true;
        }
        let k = &mut cfg.key_display;
        let pl = pick(PLACEMENTS, self.placement.get());
        if k.placement != pl {
            k.placement = pl;
            changed = true;
        }
        macro_rules! seti {
            ($field:ident, $expr:expr) => {{
                let v = $expr;
                if k.$field != v {
                    k.$field = v;
                    changed = true;
                }
            }};
        }
        macro_rules! setkf {
            ($field:ident, $expr:expr, $eps:expr) => {{
                let v = $expr;
                if (k.$field - v).abs() > $eps {
                    k.$field = v;
                    changed = true;
                }
            }};
        }
        // ☆ 2026-09-21 偏移收敛：面板只露「水平偏移 / 垂直偏移」两根（与画中画
        //   放大同型），在这里按方位映射到旧的四字段：水平写到方位选中的
        //   那根（left 或 right），垂直写到 top/bottom 中的那根；对侧恒 0。
        //   落盘结构不动（老配置升级不丢值，四根各自的历史值继续存在）。
        //   §35 三件套：基准取方位选中那根的配置现值（与信号初始化同源），
        //   对账走 `slider_pair_sync`（文本赢时配置跟文本）。
        let kd_pl = pick(PLACEMENTS, self.placement.get());
        let kd_left = kd_pl.ends_with("left");
        let kd_top = kd_pl.starts_with("top");
        let kd_h = slider_pair_sync(
            &self.offset_left,
            &self.offset_left_text,
            (if kd_left {
                k.left_offset
            } else {
                k.right_offset
            }) as f32,
            0.0,
            50.0,
            1.0,
            0,
        )
        .round() as i32;
        let kd_v = slider_pair_sync(
            &self.offset_bottom,
            &self.offset_bottom_text,
            (if kd_top {
                k.top_offset
            } else {
                k.bottom_offset
            }) as f32,
            0.0,
            50.0,
            1.0,
            0,
        )
        .round() as i32;
        seti!(top_offset, if kd_top { kd_v } else { 0 });
        seti!(bottom_offset, if kd_top { 0 } else { kd_v });
        seti!(left_offset, if kd_left { kd_h } else { 0 });
        seti!(right_offset, if kd_left { 0 } else { kd_h });
        seti!(
            popup_width,
            slider_pair_sync(
                &self.popup_width,
                &self.popup_width_text,
                k.popup_width as f32,
                5.0,
                50.0,
                1.0,
                0,
            )
            .round() as u32
        );
        setkf!(
            opacity,
            slider_pair_sync(
                &self.key_opacity,
                &self.key_opacity_text,
                k.opacity,
                0.05,
                1.0,
                0.01,
                2,
            ),
            1e-3
        );
        seti!(
            fade_delay,
            slider_pair_sync(
                &self.fade_delay,
                &self.fade_delay_text,
                k.fade_delay as f32,
                1.0,
                30.0,
                1.0,
                0,
            )
            .round() as u32
        );
        setkf!(
            merge_interval,
            slider_pair_sync(
                &self.merge_interval,
                &self.merge_interval_text,
                k.merge_interval,
                0.5,
                10.0,
                0.1,
                1,
            ),
            1e-3
        );
        seti!(
            font_size,
            slider_pair_sync(
                &self.font_size,
                &self.font_size_text,
                k.font_size as f32,
                8.0,
                72.0,
                1.0,
                0,
            )
            .round() as u32
        );
        for (field, sig) in [
            (&mut k.font_color, self.color_font),
            (&mut k.fun_color, self.color_fun),
            (&mut k.symbol_color, self.color_symbol),
            (&mut k.bg_color, self.color_bg),
        ] {
            let v = color_to_array(sig.get());
            if *field != v {
                *field = v;
                changed = true;
            }
        }
        let dm = pick(DISPLAY_MODES, self.display_mode.get());
        if k.display_mode != dm {
            k.display_mode = dm;
            changed = true;
        }
        // （旧「鼠标按键」开关已删：运行时由 display_mode 派生 show_mouse，
        //   见 key_display::settings::from_config——面板不再有这个信号。）
        // ── 屏幕标注 ──
        if cfg.drawing.enabled != self.enabled_draw.get() {
            cfg.drawing.enabled = self.enabled_draw.get();
            changed = true;
        }
        let d = &mut cfg.drawing;
        for (i, (color, width, width_text, style)) in [
            (
                self.pen1_color,
                self.pen1_width,
                self.pen1_width_text,
                self.pen1_style,
            ),
            (
                self.pen2_color,
                self.pen2_width,
                self.pen2_width_text,
                self.pen2_style,
            ),
            (
                self.pen3_color,
                self.pen3_width,
                self.pen3_width_text,
                self.pen3_style,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let Some(pen) = d.pens.get_mut(i) else {
                continue;
            };
            let c = fmt_like(&pen.color, color.get());
            if !same_color(&pen.color, &c) {
                pen.color = c;
                changed = true;
            }
            let w = slider_pair_sync(&width, &width_text, pen.width as f32, 1.0, 500.0, 1.0, 0)
                .round() as u32;
            if pen.width != w {
                pen.width = w;
                changed = true;
            }
            let s = pick(PEN_STYLES, style.get());
            if pen.style != s {
                pen.style = s;
                changed = true;
            }
        }
        let hk = &mut d.hotkeys;
        for (field, sig) in [
            (&mut hk.draw_hk_screen, self.hk_screen),
            (&mut hk.draw_hk_white, self.hk_white),
            (&mut hk.draw_hk_pen1, self.hk_pen1),
            (&mut hk.draw_hk_pen2, self.hk_pen2),
            (&mut hk.draw_hk_pen3, self.hk_pen3),
            (&mut hk.draw_hk_line, self.hk_line),
            (&mut hk.draw_hk_arrow, self.hk_arrow),
            (&mut hk.draw_hk_rect, self.hk_rect),
            (&mut hk.draw_hk_ellipse, self.hk_ellipse),
            (&mut hk.draw_hk_text, self.hk_text),
            (&mut hk.draw_hk_stamp, self.hk_stamp),
            (&mut hk.draw_hk_mosaic, self.hk_mosaic),
            (&mut hk.draw_hk_eraser, self.hk_eraser),
        ] {
            let v = sig.get().trim().to_string();
            if *field != v {
                *field = v;
                changed = true;
            }
        }

        // ── 便签（工具页）：快捷键 + 两项颜色 + 透明度 + 字号 ──
        let n = &mut cfg.sticky_note;
        // 功能开关（面板卡标题栏选框）。
        if n.enabled != self.sticky_enabled.get() {
            n.enabled = self.sticky_enabled.get();
            changed = true;
        }
        // 快捷键空串 = 停用（与计时器同款；开关之外的第二道闸）。
        let n_hk = self.hk_sticky.get().trim().to_string();
        if n.hotkey != n_hk {
            n.hotkey = n_hk;
            changed = true;
        }
        if n.bg_color != color_to_array(self.note_bg_color.get()) {
            n.bg_color = color_to_array(self.note_bg_color.get());
            changed = true;
        }
        if n.text_color != color_to_array(self.note_text_color.get()) {
            n.text_color = color_to_array(self.note_text_color.get());
            changed = true;
        }
        if n.edit_color != color_to_array(self.note_edit_color.get()) {
            n.edit_color = color_to_array(self.note_edit_color.get());
            changed = true;
        }
        let n_alpha = slider_pair_sync(
            &self.note_alpha,
            &self.note_alpha_text,
            n.alpha as f32,
            NOTE_ALPHA_MIN as f32,
            NOTE_ALPHA_MAX as f32,
            1.0,
            0,
        )
        .round()
        .clamp(NOTE_ALPHA_MIN as f32, NOTE_ALPHA_MAX as f32) as u32;
        if n.alpha != n_alpha {
            n.alpha = n_alpha;
            changed = true;
        }
        let n_font = slider_pair_sync(
            &self.note_font_sz,
            &self.note_font_sz_text,
            n.font_size,
            NOTE_FONT_MIN,
            NOTE_FONT_MAX,
            1.0,
            0,
        )
        .clamp(NOTE_FONT_MIN, NOTE_FONT_MAX);
        if (n.font_size - n_font).abs() > f32::EPSILON {
            n.font_size = n_font;
            changed = true;
        }

        // ── 剪贴板历史（工具页）：开关 + 快捷键 + 容量档 ──
        let cb = &mut cfg.clipboard;
        let cb_hk = self.hk_clipboard.get().trim().to_string();
        if cb.hotkey != cb_hk {
            cb.hotkey = cb_hk;
            changed = true;
        }
        if cb.enabled != self.clip_enabled.get() {
            cb.enabled = self.clip_enabled.get();
            changed = true;
        }

        // 容量：档位下标 → 真实值（找不到回落出厂档 500）。
        let cb_max = crate::core::config::CLIP_ENTRIES_VALUES
            .get(self.clip_max_entries.get())
            .copied()
            .unwrap_or(crate::core::config::CLIP_ENTRIES_VALUES[0]);
        if cb.max_entries != cb_max {
            cb.max_entries = cb_max;
            changed = true;
        }

        // ── 屏幕页：截图与贴图 ──
        // 通用「有变化才写」助手（上面那段用的是绑死 `k` 的 `seti!`/`setkf!`，这里另起
        // 一组按表达式取字段的，供截图/录屏/虚拟键盘三段共用）。
        macro_rules! seti2 {
            ($field:expr, $v:expr) => {{
                let v = $v;
                if $field != v {
                    $field = v;
                    changed = true;
                }
            }};
        }
        let shot = &mut cfg.screenshot;
        // 功能开关（`setbool!` 宏定义在后面 rec 段里，这里手写同款 diff）。
        if shot.enabled != self.shot_enabled.get() {
            shot.enabled = self.shot_enabled.get();
            changed = true;
        }
        macro_rules! setstr {
            ($field:expr, $sig:expr) => {{
                let v = $sig.get().trim().to_string();
                if $field != v {
                    $field = v;
                    changed = true;
                }
            }};
        }
        setstr!(shot.hotkey, self.hk_shot);
        // ★ 截图并贴图 / 贴图快捷键（同一套捕获流，见 HotkeyField）。
        setstr!(shot.hotkey_pin, self.hk_shot_pin);
        setstr!(shot.hotkey_pin_clipboard, self.hk_pin_clipboard);
        setstr!(shot.hotkey_scroll, self.hk_shot_scroll);
        setstr!(shot.hotkey_scroll_wide, self.hk_shot_scroll_wide);
        // 容量：下标 → 实际值（与帧率/画质同款，越界回落到出厂值）。
        let pin_cap = crate::core::config::PIN_CAP_VALUES
            .get(self.pin_cap.get())
            .copied()
            .unwrap_or(crate::core::config::DEFAULT_PIN_RING_CAP);
        seti2!(shot.pin_ring_cap, pin_cap);

        // ── 屏幕页：视频录制 ──
        macro_rules! setbool {
            ($field:expr, $sig:expr) => {{
                let v = $sig.get();
                if $field != v {
                    $field = v;
                    changed = true;
                }
            }};
        }
        let rec = &mut cfg.record;
        setbool!(rec.enabled, self.rec_enabled);
        setstr!(rec.hotkey, self.hk_record);
        setstr!(rec.region_hotkey, self.hk_record_region);
        setstr!(rec.trim_hotkey, self.hk_record_trim);
        // 帧率存下标、取值走换算表（见 `RECORD_FPS_VALUES`）：以后想改档位只动表。
        let fps = crate::core::config::RECORD_FPS_VALUES
            .get(self.rec_fps.get())
            .copied()
            .unwrap_or(30);
        if rec.fps != fps {
            rec.fps = fps;
            changed = true;
        }
        seti2!(
            rec.bitrate,
            pick(
                &crate::core::config::RECORD_BITRATE_VALUES,
                self.rec_bitrate.get()
            )
        );
        seti2!(
            rec.resolution,
            (self.rec_resolution.get()).min(crate::core::config::RECORD_RESOLUTION_LABELS.len() - 1)
                as u32
        );
        seti2!(
            rec.hardware,
            (self.rec_hardware.get()).min(crate::core::config::RECORD_HARDWARE_LABELS.len() - 1)
                as u32
        );
        setbool!(rec.show_cursor, self.rec_cursor);
        // 音频来源：下拉下标 → 三态。**不要在保存路径上手写"开关 + 设备名"两行** ——
        // 那正是这次要收掉的重复口径（旧实现里"来源选着设备、开关却是关的"能存进配置）。
        let mic_src = source_from_index(self.rec_mic_source.get(), &self.rec_mic_devices)
            .map(|v| v.to_string());
        if rec.mic_source() != mic_src.as_deref() {
            rec.set_mic_source(mic_src.as_deref());
            changed = true;
        }
        let spk_src = source_from_index(self.rec_speaker_source.get(), &self.rec_speaker_devices)
            .map(|v| v.to_string());
        if rec.speaker_source() != spk_src.as_deref() {
            rec.set_speaker_source(spk_src.as_deref());
            changed = true;
        }
        // 麦克风降噪档位：下标 → 内部值（off/weak/medium/strong）。
        let denoise = crate::core::config::MIC_DENOISE_VALUES
            .get(self.rec_denoise.get())
            .copied()
            .unwrap_or("off");
        if rec.mic_denoise != denoise {
            rec.mic_denoise = denoise.to_string();
            changed = true;
        }

        // ── 屏幕页：GIF录制 ──
        // 录制与剪辑各一把键（见 `HotkeyField::Gif` / `HotkeyField::Trim`）。
        let gif = &mut cfg.gif;
        setbool!(gif.enabled, self.gif_enabled);
        setstr!(gif.hotkey, self.hk_gif);
        setstr!(gif.trim_hotkey, self.hk_gif_trim);
        let gfps = crate::core::config::GIF_FPS_VALUES
            .get(self.gif_fps.get())
            .copied()
            .unwrap_or(15);
        if gif.fps != gfps {
            gif.fps = gfps;
            changed = true;
        }
        seti2!(
            gif.size,
            (self.gif_size.get()).min(crate::core::config::GIF_SIZE_LABELS.len() - 1) as u32
        );
        setbool!(gif.show_cursor, self.gif_cursor);
        setbool!(gif.loop_forever, self.gif_loop);

        // ── 虚拟键盘 ──
        let vk = &mut cfg.virtual_keyboard;
        setbool!(vk.enabled, self.vk_enabled);
        // 方案 id 从面板构建时那份快照里取（目录即来源，见 `PanelModel::vk_schemes`）。
        let vkl = pick_scheme(&self.vk_schemes, self.vk_layout.get());
        if vk.layout != vkl {
            vk.layout = vkl.clone();
            changed = true;
        }
        let vkp = pick(PLACEMENTS, self.vk_placement.get());
        if vk.placement != vkp {
            vk.placement = vkp;
            changed = true;
        }
        // ☆ 与击键回显同款收敛：两根信号按方位映射到四字段，对侧恒 0。
        //   §35 三件套：基准取方位选中那根的配置现值，对账走 `slider_pair_sync`。
        let vk_left = vk.placement.ends_with("left");
        let vk_top = vk.placement.starts_with("top");
        let vk_h = slider_pair_sync(
            &self.vk_offset_left,
            &self.vk_offset_left_text,
            if vk_left {
                vk.left_offset
            } else {
                vk.right_offset
            },
            0.0,
            50.0,
            0.5,
            1,
        );
        let vk_v = slider_pair_sync(
            &self.vk_offset_bottom,
            &self.vk_offset_bottom_text,
            if vk_top {
                vk.top_offset
            } else {
                vk.bottom_offset
            },
            0.0,
            50.0,
            0.5,
            1,
        );
        seti2!(vk.top_offset, if vk_top { vk_v } else { 0.0 });
        seti2!(vk.bottom_offset, if vk_top { 0.0 } else { vk_v });
        seti2!(vk.left_offset, if vk_left { vk_h } else { 0.0 });
        seti2!(vk.right_offset, if vk_left { 0.0 } else { vk_h });
        // ★ 只保留**当前所选方案**的一整套控件值（请求 2）：配置不该同时记录多套。
        //
        // 切换方案即载入该方案默认、清掉旧设定 —— 这里落盘时只写当前方案那一个键，
        // 其余方案的历史取值一概不写（读盘时 `Config::migrate_vk_layout` 也会把残留的
        // 非当前键清掉）。写全量会把"当时的默认值"钉死在配置里，也不符合"只记一套"。
        let mut params: BTreeMap<String, BTreeMap<String, ParamValue>> = BTreeMap::new();
        if let Some(sc) = self.vk_controls.get().iter().find(|sc| sc.scheme_id == vkl) {
            let mut m: BTreeMap<String, ParamValue> = BTreeMap::new();
            for it in &sc.items {
                let cur = match it.sig {
                    // ★ 滑块里是比例，落盘要还原成真实量纲（并按 step 吸附）；
                    //   文本框键入时"文本赢"（与脚本参数 float_param_sync 同判据：
                    //   文本可解析、与当前滑条值不同、且滑条值仍贴着基准 → 文本引起）。
                    VkControlSignal::Range(s, text) => {
                        let (min, max, step, decimals) = match it.def.kind {
                            crate::modules::virtual_keyboard::vk_config::ControlKind::Range {
                                min,
                                max,
                                step,
                                decimals,
                                ..
                            } => (min, max, step, decimals),
                            _ => (0.0, 1.0, 0.0, 0),
                        };
                        // ★ 基准 = **配置现值**（上次落盘的值；没写过则方案默认）。
                        //   用方案默认当基准会留下第二个 bug：拖过一次滑条后 `sv ≠ 默认`，
                        //   判据里的"滑条没动"恒为假 → 之后往输入框打字永远不生效。
                        let base = vk
                            .params
                            .get(&vkl)
                            .and_then(|m| m.get(&it.def.id))
                            .and_then(|v| v.as_f32())
                            .or_else(|| it.def.default.as_f32())
                            .unwrap_or(min);
                        // ★ 与鼠标页逐字同款：走 `slider_pair_sync` 正本——它负责
                        //   ① 滑条动了把镜像文本回写成新值（本卡此前缺的就是这一句，
                        //      症状"拖动有效果、输入框数字不变"）；② 按 step 吸附；
                        //   ③ 文本赢时把滑条跳过去。此前这里手抄了一份判据，抄漏了 ①②。
                        ParamValue::Float(slider_pair_sync(
                            &s, &text, base, min, max, step, decimals,
                        ))
                    }
                    VkControlSignal::Bool(s) => ParamValue::Bool(s.get()),
                    VkControlSignal::Color(s) => ParamValue::Color(color_hex(s.get())),
                };
                // 只写**与方案默认不同**的项：没动过的项不落盘，方案作者改了默认值
                // 用户下一帧就看到新观感（"方案说了算"）。
                if !param_eq(&cur, &it.def.default) {
                    m.insert(it.def.id.clone(), cur);
                }
            }
            if !m.is_empty() {
                params.insert(vkl, m);
            }
        }
        if vk.params != params {
            vk.params = params;
            changed = true;
        }
        setbool!(vk.click_through, self.vk_click_through);
        setbool!(cfg.overlay.always_on_top, self.vk_always_on_top);
        setbool!(cfg.overlay.avoid_fullscreen, self.vk_fullscreen_avoid);

        // ── 全局 ──
        let ph = self.panel_hotkey.get().trim().to_string();
        if cfg.global.panel_hotkey != ph {
            cfg.global.panel_hotkey = ph;
            changed = true;
        }

        // ── 计时器（§1 #15）：状态机在 runtime，配置存"模式 + 时长 + 位置/大小"。──
        setbool!(cfg.global.timer.enabled, self.timer_enabled);
        let th = self.hk_timer.get().trim().to_string();
        if cfg.global.timer.hotkey != th {
            cfg.global.timer.hotkey = th;
            changed = true;
        }
        let mode = match self.timer_mode.get() {
            1 => "countdown",
            _ => "countup",
        };
        if cfg.global.timer.mode != mode {
            cfg.global.timer.mode = mode.to_string();
            changed = true;
        }
        // ── 计时器时长：滑条 + 数值输入框双入口（2026-09-25）──
        // 输入框补滑条的精度短板（1–3600 秒量程下 1px ≈ 18s，点不到精确秒）。
        // 两个控件绑两个信号（f32 滑条 / String 文本），这里对账：**谁与配置偏离
        // 谁赢**（一次同步只有一个入口在变），另一侧回写：
        //   · 滑条刚拖动（滑条值 ≠ 配置）：配置跟滑条，文本回写成新值；
        //   · 文本刚编辑成合法数字且 ≠ 滑条刻度（此时滑条值必 = 配置 ⇒ 滑条没动）：
        //     配置跟文本，滑条信号跳过去；文本**不回写**——"0300" 这类中间态原样
        //     保留，不打断输入；
        //   · 文本是空串 / 非数字 / 超 u64（打字中间态）：忽略，配置与滑条不动，
        //     也**不**把文本回写成旧值（清空重输不被同步进程打架）。
        // ★ 对账走 `slider_pair_sync` 正本（与其余 39 处三件套同一套判据）——
        //   此前这里手抄了一份 u64 版，抄一份就多一份漂移风险（虚拟键盘那次漏抄
        //   "滑条动 → 文本回写"就是这么来的）。u64 只在出口取整 + 钳制。
        let td = slider_pair_sync(
            &self.timer_duration,
            &self.timer_duration_text,
            cfg.global.timer.duration_sec as f32,
            TIMER_DURATION_MIN as f32,
            TIMER_DURATION_MAX as f32,
            1.0,
            0,
        )
        .round()
        .clamp(TIMER_DURATION_MIN as f32, TIMER_DURATION_MAX as f32) as u64;
        if cfg.global.timer.duration_sec != td {
            cfg.global.timer.duration_sec = td;
            changed = true;
        }
        // 锚点 / 偏移 / 字号（2026-09-25）：量纲与画中画同款（见 `timer_center` 的
        // 几何注释）。锚点是下标，`.min(6)` 兜底未知取值（画中画 `.min(4)` 同款）。
        let anchor = self.timer_anchor.get().min(6) as u32;
        if cfg.global.timer.anchor != anchor {
            cfg.global.timer.anchor = anchor;
            changed = true;
        }
        let tox = slider_pair_sync(
            &self.timer_offset_x,
            &self.timer_offset_x_text,
            cfg.global.timer.offset_x as f32,
            TIMER_OFFSET_MIN as f32,
            TIMER_OFFSET_MAX as f32,
            1.0,
            0,
        )
        .round()
        .clamp(TIMER_OFFSET_MIN as f32, TIMER_OFFSET_MAX as f32) as u8;
        if cfg.global.timer.offset_x != tox {
            cfg.global.timer.offset_x = tox;
            changed = true;
        }
        let toy = slider_pair_sync(
            &self.timer_offset_y,
            &self.timer_offset_y_text,
            cfg.global.timer.offset_y as f32,
            TIMER_OFFSET_MIN as f32,
            TIMER_OFFSET_MAX as f32,
            1.0,
            0,
        )
        .round()
        .clamp(TIMER_OFFSET_MIN as f32, TIMER_OFFSET_MAX as f32) as u8;
        if cfg.global.timer.offset_y != toy {
            cfg.global.timer.offset_y = toy;
            changed = true;
        }
        let font = slider_pair_sync(
            &self.timer_font,
            &self.timer_font_text,
            cfg.global.timer.font_pt as f32,
            TIMER_FONT_MIN as f32,
            TIMER_FONT_MAX as f32,
            1.0,
            0,
        )
        .round()
        .clamp(TIMER_FONT_MIN as f32, TIMER_FONT_MAX as f32) as u32;
        if cfg.global.timer.font_pt != font {
            cfg.global.timer.font_pt = font;
            changed = true;
        }

        // ── 画中画放大（屏幕页）──
        let p = &mut cfg.magnifier_pip;
        setbool!(p.enabled, self.pip_enabled);
        // §35 三件套：面板宽高是连续量（0.1 步长无意义），step=0 只做 2 位小数收尾；
        // diff 用 1e-3 容差，浮点往返尾数不产生伪落盘。
        let pw = slider_pair_sync(
            &self.pip_panel_w,
            &self.pip_panel_w_text,
            p.panel_width,
            PIP_PANEL_MIN,
            PIP_PANEL_MAX,
            0.0,
            2,
        );
        if (p.panel_width - pw).abs() > 1e-3 {
            p.panel_width = pw;
            changed = true;
        }
        let ph = slider_pair_sync(
            &self.pip_panel_h,
            &self.pip_panel_h_text,
            p.panel_height,
            PIP_PANEL_MIN,
            PIP_PANEL_MAX,
            0.0,
            2,
        );
        if (p.panel_height - ph).abs() > 1e-3 {
            p.panel_height = ph;
            changed = true;
        }
        let zoom = slider_pair_sync(
            &self.pip_zoom,
            &self.pip_zoom_text,
            p.zoom as f32,
            PIP_ZOOM_MIN as f32,
            PIP_ZOOM_MAX as f32,
            1.0,
            0,
        )
        .round()
        .clamp(PIP_ZOOM_MIN as f32, PIP_ZOOM_MAX as f32) as u32;
        if p.zoom != zoom {
            p.zoom = zoom;
            changed = true;
        }
        // 位置：0 鼠标跟随 / 1 左上 / 2 右上 / 3 左下 / 4 右下。
        let anchor = self.pip_anchor.get().min(4);
        if p.anchor != anchor as u32 {
            p.anchor = anchor as u32;
            changed = true;
        }
        // 偏移（屏幕百分比 0–50）：贴边时往屏内挪，贴下边抬高让开任务栏。
        let ox = slider_pair_sync(
            &self.pip_offset_x,
            &self.pip_offset_x_text,
            p.offset_x as f32,
            0.0,
            50.0,
            1.0,
            0,
        )
        .round()
        .clamp(0.0, 50.0) as u8;
        if p.offset_x != ox {
            p.offset_x = ox;
            changed = true;
        }
        let oy = slider_pair_sync(
            &self.pip_offset_y,
            &self.pip_offset_y_text,
            p.offset_y as f32,
            0.0,
            50.0,
            1.0,
            0,
        )
        .round()
        .clamp(0.0, 50.0) as u8;
        if p.offset_y != oy {
            p.offset_y = oy;
            changed = true;
        }
        // （指针准星选项已删除：2026-09-21 用户拍板"完全不需要这个"。老配置里的
        // `show_crosshair` 字段由 serde 兼容语义忽略，无需迁移。）
        // 边框色走「保持原串位数」回写（6 位 `#RRGGBB` / 8 位 `#RRGGBBAA`），
        // 避免把 config 里已有的写法无谓改写成另一种（对照 `fmt_like` 用途）。
        if !same_color(
            &p.border_color,
            &fmt_like(&p.border_color, self.pip_border_color.get()),
        ) {
            p.border_color = fmt_like(&p.border_color, self.pip_border_color.get());
            changed = true;
        }
        // （缩放快捷键 Ctrl+Win+滚轮 为内置，无配置项可同步——见
        //  `platform::input::mouse` 的 WM_MOUSEWHEEL 注释。）

        // ── 全屏放大（屏幕页）──
        let f = &mut cfg.magnifier_fullscreen;
        // 快捷键留空 = 停用（与计时器同款：判空即停用，没有第二处总开关）。
        let fh = self.hk_fullscreen_zoom.get().trim().to_string();
        if f.hotkey != fh {
            f.hotkey = fh;
            changed = true;
        }
        setbool!(f.enabled, self.fs_enabled);
        // ── 画中画放大：启停快捷键（2026-09-26 新增，与全屏同款写法）──
        let ph = self.hk_pip_toggle.get().trim().to_string();
        if p.hotkey != ph {
            p.hotkey = ph;
            changed = true;
        }
        let zoom = slider_pair_sync(
            &self.fs_zoom,
            &self.fs_zoom_text,
            f.zoom as f32,
            FULLSCREEN_ZOOM_MIN as f32,
            FULLSCREEN_ZOOM_MAX as f32,
            1.0,
            0,
        )
        .round()
        .clamp(FULLSCREEN_ZOOM_MIN as f32, FULLSCREEN_ZOOM_MAX as f32) as u32;
        if f.zoom != zoom {
            f.zoom = zoom;
            changed = true;
        }

        // ── 击键音效（键盘页）：开关 + 脚本 + 音量。脚本目录每次 diff 前重扫太贵，
        // 目录扫描只在面板构建时做一次（面板重开即重扫，用户丢新脚本即可选到）。
        let ksnd = &mut cfg.key_sound;
        setbool!(ksnd.enabled, self.ksound_enabled);
        // 下标 → 脚本 id（目录被删空时保持原值不动，等目录恢复）。
        let ks_id = self.ksound_catalog.id_at(self.ksound_script.get());
        let ks_id = if ks_id.trim().is_empty() {
            ksnd.script.clone()
        } else {
            ks_id
        };
        // ★ 换脚本 → 整批重建参数单元（新脚本的 schema + 它自己已存的参数）。
        //   这就是"选不同脚本看到不同选项"的落点：单元由脚本 schema 生成，互不串味。
        if ks_id != *self.last_ksound_script.borrow() {
            *self.last_ksound_script.borrow_mut() = ks_id.clone();
            let stored = ksnd.params.get(&ks_id).cloned().unwrap_or_default();
            self.ksound_params
                .set(ks_build_cells(&self.ksound_catalog, &ks_id, &stored));
        }
        if ksnd.script != ks_id {
            ksnd.script = ks_id.clone();
            changed = true;
        }
        // 参数回写：只把**相对初值有变化**的键写进 params[脚本id]（未动的键留给脚本
        // 默认值；机制与悬浮标 hover_params 一致，保证开面板不改东西不产生伪落盘）。
        let mut ks_params_changed = false;
        for cell in self.ksound_params.get() {
            let new_val: Value = match &cell {
                EffectParamCell::Float {
                    sig,
                    text,
                    min,
                    max,
                    step,
                    decimals,
                    ..
                } => {
                    // referee 现场读（`ksnd.params.entry` 在 match 之后才建）；该脚本
                    // 还没有任何落盘参数时用空表兜底 → helper 回退 initial。
                    let empty = HashMap::new();
                    let stored = ksnd.params.get(&ks_id).unwrap_or(&empty);
                    let q = float_param_sync(
                        stored,
                        cell.key(),
                        &cell,
                        sig,
                        text,
                        *min,
                        *max,
                        *step,
                        *decimals,
                    );
                    json_decimal(q, *decimals)
                }
                EffectParamCell::Int {
                    sig,
                    text,
                    min,
                    max,
                    ..
                } => {
                    let empty = HashMap::new();
                    let stored = ksnd.params.get(&ks_id).unwrap_or(&empty);
                    let q =
                        float_param_sync(stored, cell.key(), &cell, sig, text, *min, *max, 0.0, 0);
                    json!(q.round() as i64)
                }
                EffectParamCell::Bool { sig, .. } => json!(sig.get()),
                EffectParamCell::Choice { sig, choices, .. } => {
                    json!(choices.get(sig.get()).cloned().unwrap_or_default())
                }
                // 音效下拉与 Choice 同形：值取当前候选项里的那一项（短名或绝对路径）。
                EffectParamCell::Sound { sig, choices, .. } => {
                    json!(choices.get(sig.get()).cloned().unwrap_or_default())
                }
                EffectParamCell::Folder { sig, .. } => json!(sig.get()),
                EffectParamCell::Color { sig, alpha, .. } => json!(fmt_color(sig.get(), *alpha)),
                EffectParamCell::Image { sig, .. }
                | EffectParamCell::Str { sig, .. }
                | EffectParamCell::File { sig, .. }
                | EffectParamCell::Hotkey { sig, .. } => json!(sig.get()),
            };
            if cell.initial() == &new_val {
                continue; // 用户没动过的参数不落盘（脚本侧有同值默认）
            }
            let ks_extra = ksnd.params.entry(ks_id.clone()).or_default();
            if ks_extra.get(cell.key()) != Some(&new_val) {
                ks_extra.insert(cell.key().to_string(), new_val);
                ks_params_changed = true;
            }
        }
        if ks_params_changed {
            changed = true;
        }
        let kv = slider_pair_sync(
            &self.ksound_volume,
            &self.ksound_volume_text,
            ksnd.volume as f32,
            0.0,
            100.0,
            1.0,
            0,
        )
        .round()
        .clamp(0.0, 100.0) as u8;
        if ksnd.volume != kv {
            ksnd.volume = kv;
            changed = true;
        }
        // （「音效文件 / 音效文件夹」两行已删：file 字段仅作出厂兜底不再由面板写；
        //   音效选择迁入脚本参数的 `file` 浏览按钮，走上面的 params 回写。）

        // ★★ 2026-09-26 两段语义：勾选 = **载入模块**，两者可同时载入（互斥移到
        //   运行态：`runtime::zoom` 里激活一边退另一边，`Runtime::render` 兜底收窗）。
        //   面板在这里只做一件事：**取消勾选 = 顺带退出运行态**——模块都载出了，
        //   快捷键/窗口不许残留。
        if !cfg.magnifier_pip.enabled {
            cfg.magnifier_pip.active = false;
        }
        if !cfg.magnifier_fullscreen.enabled {
            cfg.magnifier_fullscreen.active = false;
        }

        // ── 面板页码（主窗口状态记忆，纯界面状态同样落盘）──
        // 点导航即变信号 → 这里每帧 diff → 防抖落盘：重开面板/重启进程都回到最后所在页。
        // ★ 页 id 与下标**一起**落，但只在下标真的变了时写：id 缺失（老配置 None）时若
        //   按 "id != Some(..)" 单独补写，零操作打开面板就会产生伪改动
        //   （`model_no_spurious_change` 的承诺就破了）。没切页就继续靠读取侧的
        //   `legacy_page_index` 迁移，每次打开面板都正确，效果等价。
        let page = self.page.get().min(crate::ui::nav::NAV_ABOUT);
        if cfg.ui.panel_page != page {
            cfg.ui.panel_page = page;
            cfg.ui.panel_page_id = Some(crate::ui::nav::page_id(page).to_string());
            changed = true;
        }

        // ── 面板卡片展开态（纯界面状态，同样落盘）──
        // 只记"与出厂默认不同"的项：相等就跳过，回到默认就把键删掉。这样
        //   ① 没动过任何卡片时 `expanded` 保持为空 → 首帧不产生伪改动
        //      （对照 `model_no_spurious_change`：这条一旦写错就会每帧误报"有改动"）；
        //   ② 配置文件只留用户真正改过的卡，不会糊上一堆默认值。
        for (id, sig) in &self.card_expanded {
            let v = sig.get();
            let default = crate::core::config::default_card_expanded(id);
            let stored = cfg.ui.expanded.get(*id).copied();
            if stored.unwrap_or(default) == v {
                continue;
            }
            if v == default {
                cfg.ui.expanded.remove(*id);
            } else {
                cfg.ui.expanded.insert((*id).to_string(), v);
            }
            changed = true;
        }

        // ── 设置页 · 工作文件夹「自动清理」──
        // 下标 → 天数写回 `config.general.temp_cleanup_days`（0 = 永不）。
        let days = TEMP_CLEANUP_DAYS[self.temp_cleanup.get().min(TEMP_CLEANUP_DAYS.len() - 1)];
        if cfg.general.temp_cleanup_days != days {
            cfg.general.temp_cleanup_days = days;
            changed = true;
        }

        // ── 设置页 · 系统「管理员权限启动」──
        // 勾选态写回 `config.general.admin_launch`；**下次启动**才生效（main.rs 消费），
        // 当前进程的权限不变。
        if cfg.general.admin_launch != self.admin_launch.get() {
            cfg.general.admin_launch = self.admin_launch.get();
            changed = true;
        }

        changed
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    /// ★ m33：刷新 = **重扫目录 + 换掉卡片表**，而且
    ///   ① 刚放进目录的脚本能出现（这是按钮存在的理由）；
    ///   ② 配置一字不动（"正在跑"的实例表还在）；
    ///   ③ 新卡片按配置现值初始化（已经打开的开关刷新后仍然是开的）。
    ///
    /// 用临时目录而不是 `assets/lua`：判据要能证明"扫描的是**传进来那个目录**"——
    /// 拿真资产测的话，即使实现忽略参数、只会扫出厂目录，这一格也会绿。
    #[test]
    fn reload_lua_cards_picks_up_new_scripts_without_touching_config() {
        let root = std::env::temp_dir().join(format!("curseen_refresh_{}", std::process::id()));
        let scripts = root.join("scripts");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::write(
            scripts.join("demo.lua"),
            "name = \"演示\"
role = \"image\"
menu = \"演示\"
function run(ctx) return \"ok\" end
",
        )
        .unwrap();

        let mut cfg = crate::core::config::Config::default();
        // 一个"正在跑"的复合实例：刷新不许把它从配置里抹掉。
        cfg.lua_ext.composite_active = vec!["scripts/running_now".to_string()];
        // 已经勾开的 image 脚本：刷新后新卡片的开关必须还是开的（按现值初始化）。
        cfg.lua_ext.image_active = vec!["scripts/demo".to_string()];

        let m = PanelModel::from_config(&cfg);
        let before_running = cfg.lua_ext.composite_active.clone();

        let n = m.reload_lua_cards(&cfg, &root);
        assert_eq!(n, 1, "临时目录里只有一个脚本");
        let cards = m.lua_ext_cards.get();
        let demo = cards
            .iter()
            .find(|c| c.id == "scripts/demo")
            .expect("刷新后应出现刚放进目录的脚本");
        assert!(demo.is_script_card(), "scripts/ 下的脚本应出卡");
        assert!(demo.toggle.get(), "开关初值必须按配置现值来（不是恒关）");

        std::fs::write(
            scripts.join("second.lua"),
            "name = \"第二个\"
function run(ctx) return \"ok\" end
",
        )
        .unwrap();
        assert_eq!(m.reload_lua_cards(&cfg, &root), 2);

        assert_eq!(
            cfg.lua_ext.composite_active, before_running,
            "刷新不许改运行列表"
        );
        assert_eq!(cfg.lua_ext.image_active, vec!["scripts/demo".to_string()]);

        // ★★ 审查修（m33）：设置页那列镜像行绑的是**面板打开时**的旧卡片信号。
        //   刷新必须**沿用旧实例**（外形没变时），否则那些行绑到孤儿信号上——
        //   拨开关看着有反应、`apply_to` 却读不到。
        let demo_old = m
            .lua_ext_cards
            .get()
            .iter()
            .find(|c| c.id == "scripts/demo")
            .cloned()
            .expect("刷新后应有 demo 卡");
        m.reload_lua_cards(&cfg, &root);
        demo_old.toggle.set(false); // 模拟"另一页的镜像行拨了一下"
        let demo_now = m
            .lua_ext_cards
            .get()
            .into_iter()
            .find(|c| c.id == "scripts/demo")
            .expect("demo 卡还在");
        assert!(
            !demo_now.toggle.get(),
            "外形没变时必须复用旧卡片实例，否则旧句柄（镜像行）写不进当前卡片"
        );

        // 形状变了（schema 多一格）⇒ 不复用：卡片要重建，新参数才出得来。
        std::fs::write(
            scripts.join("demo.lua"),
            "name = \"演示\"
role = \"image\"
menu = \"演示\"
             function schema() return { { key = \"extra\", label = \"多一格\", type = \"bool\", default = true } } end
             function run(ctx) return \"ok\" end
",
        )
        .unwrap();
        m.reload_lua_cards(&cfg, &root);
        let demo_new = m
            .lua_ext_cards
            .get()
            .into_iter()
            .find(|c| c.id == "scripts/demo")
            .expect("demo 卡还在");
        assert_eq!(
            demo_new.params.get().len(),
            1,
            "schema 变了必须重建卡片（新参数要出现）"
        );
        let before = demo_new.toggle.get();
        demo_old.toggle.set(!before); // 旧句柄拨到反相
        assert_eq!(
            demo_new.toggle.get(),
            before,
            "形状变过之后旧句柄就该失效（这是刻意接受的代价：镜像行要重开面板才恢复）"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// ★ 快捷键**互斥**（2026-09-28 用户口径）：一把键只能挂一个功能，后配的赢。
    ///
    /// 不互斥时同一组合挂两个功能，谁生效取决于各处的匹配顺序（书写表 / 动作分支），
    /// 用户看到的是"配了却没反应"，而两份配置看起来都对。
    #[test]
    fn a_newly_captured_hotkey_vacates_its_previous_owner() {
        let m = PanelModel::from_config(&Config::default());
        m.set_hotkey_exclusive(HotkeyField::Record, "F3".to_string());
        let cleared = m.set_hotkey_exclusive(HotkeyField::Timer, "F3".to_string());

        assert_eq!(
            m.hotkey_signal(HotkeyField::Record).get(),
            "",
            "旧占用者必须被清空（留空即停用，框里显示「（空）」）"
        );
        assert_eq!(m.hotkey_signal(HotkeyField::Timer).get(), "F3");
        assert_eq!(cleared.len(), 1, "应报告被回收的那一个");
    }

    /// ★ 裸键分类（2026-10-01）：只有标注态工具键允许裸键，其余全部是全局语义。
    ///
    /// 分辨力在两端：`Pen1`/`Mosaic`（出厂裸 `1`/`0`）必须放行——出厂键位若被拒，
    /// 捕获校验会把用户重配的工具键也挡掉；`Screen`/`Panel` 必须拒——这两把从 park
    /// 态就能按（toggle / 呼出面板），裸 `S` 会在打字时疯狂触发。两把 toggle（Screen /
    /// White）虽在标注热键表里，但它们标注没开也要生效，属于全局语义，与工具键分家。
    #[test]
    fn bare_keys_are_only_allowed_for_annotation_tool_fields() {
        // 工具键：裸键合法（出厂键位就是这么发的）。
        for f in [
            HotkeyField::Pen1,
            HotkeyField::Pen2,
            HotkeyField::Pen3,
            HotkeyField::Line,
            HotkeyField::Arrow,
            HotkeyField::Rect,
            HotkeyField::Ellipse,
            HotkeyField::Text,
            HotkeyField::Stamp,
            HotkeyField::Mosaic,
            HotkeyField::Eraser,
        ] {
            assert!(
                f.allows_bare_key(),
                "{f:?} 是标注态工具键，必须允许裸键（出厂默认 1..9/0）"
            );
        }
        // 两把 toggle + 全部全局动作键：裸键一律拒绝。
        for f in [
            HotkeyField::Screen,
            HotkeyField::White,
            HotkeyField::Shot,
            HotkeyField::ShotPin,
            HotkeyField::PinClip,
            HotkeyField::ShotScroll,
            HotkeyField::ShotScrollWide,
            HotkeyField::Record,
            HotkeyField::Gif,
            HotkeyField::Trim,
            HotkeyField::RecordRegion,
            HotkeyField::RecordTrim,
            HotkeyField::Timer,
            HotkeyField::FullscreenZoom,
            HotkeyField::PipToggle,
            HotkeyField::Sticky,
            HotkeyField::Clipboard,
            HotkeyField::Panel,
        ] {
            assert!(
                !f.allows_bare_key(),
                "{f:?} 是全局语义，裸键会在打字时误触发，必须拒绝"
            );
        }
    }

    /// 冲突判定走**归一化**后的串：换个写法（顺序 / 大小写 / 空白）也躲不过互斥。
    #[test]
    fn hotkey_exclusion_compares_normalized_forms() {
        let m = PanelModel::from_config(&Config::default());
        m.set_hotkey_exclusive(HotkeyField::Gif, "Ctrl+Shift+D".to_string());
        let cleared =
            m.set_hotkey_exclusive(HotkeyField::Clipboard, "shift + ctrl + d".to_string());

        assert_eq!(
            cleared.len(),
            1,
            "`Ctrl+Shift+D` 与 `shift + ctrl + d` 是同一把键"
        );
        assert_eq!(m.hotkey_signal(HotkeyField::Gif).get(), "");
    }

    /// 置空**不参与**互斥：否则"空 == 空"会把别人的快捷键一起清掉
    /// （点框清空自己 = 捕获流程的第一步，天天发生）。
    #[test]
    fn clearing_a_hotkey_never_vacates_others() {
        let m = PanelModel::from_config(&Config::default());
        m.set_hotkey_exclusive(HotkeyField::Sticky, "Alt+S".to_string());
        m.set_hotkey_exclusive(HotkeyField::Panel, String::new());

        assert_eq!(
            m.hotkey_signal(HotkeyField::Sticky).get(),
            "Alt+S",
            "清空自己不该牵连别人"
        );
    }

    /// 秒类参数（step 0.2）：滑块拖出来的连续值必须吸附到 0.2 的网格上，
    /// 并且**回写滑块**——读数与落盘值必须同源，不能"显示 1.37、按 1.4 生效"。
    #[test]
    fn float_params_snap_to_the_schema_step() {
        // 量纲 0.2–5.0 秒、步长 0.2：min=0.2 起算的网格是 0.2/0.4/0.6…
        assert_eq!(decimals_for_step(0.2), 1, "0.2 秒档显示 1 位小数");
        assert_eq!(decimals_for_step(0.01), 2);
        assert_eq!(decimals_for_step(5.0), 0);

        let q = |v: f32| quantize(v, 0.2, 5.0, 0.2, decimals_for_step(0.2));
        assert!(
            (q(1.37) - 1.4).abs() < 1e-4,
            "1.37 应吸附到 1.4，实际 {}",
            q(1.37)
        );
        assert!(
            (q(1.29) - 1.2).abs() < 1e-4,
            "1.29 应吸附到 1.2，实际 {}",
            q(1.29)
        );
        assert!(
            (q(3.0) - 3.0).abs() < 1e-4,
            "已在网格上的值不动：{}",
            q(3.0)
        );
        // 越界钳回量程，且仍落在网格上。
        assert!(
            (q(9.0) - 5.0).abs() < 1e-4,
            "超上限钳到 5.0，实际 {}",
            q(9.0)
        );
        assert!(
            (q(-3.0) - 0.2).abs() < 1e-4,
            "低于下限钳到 0.2，实际 {}",
            q(-3.0)
        );

        // 落盘值 + 回写：把滑块摆到"两档之间"，apply_to 之后滑块自己也该归到那一档。
        let sig = signal(0.0_f32);
        sig.set(v2s(1.37, 0.2, 5.0));
        let text = signal("1.4".to_string());
        let stored = HashMap::new();
        let cell = EffectParamCell::Float {
            key: "k".into(),
            label: "k".into(),
            sig,
            text,
            min: 0.2,
            max: 5.0,
            step: 0.2,
            decimals: 1,
            init: json!(1.4),
        };
        let q = float_param_sync(&stored, "k", &cell, &sig, &text, 0.2, 5.0, 0.2, 1);
        assert!((q - 1.4).abs() < 1e-4, "落盘值必须是吸附后的 1.4，实际 {q}");
        let back = s2v(sig.get(), 0.2, 5.0);
        assert!(
            (back - 1.4).abs() < 1e-3,
            "滑块读数必须同步到 1.4，实际 {back}"
        );
    }

    /// 脚本参数输入框（2026-09-25）：Float/Int 单元的文本第二入口四条路 + 收敛。
    /// 对账规则与面板固定参数同款（谁与基准偏离谁赢），见 `float_param_sync`。
    #[test]
    fn script_param_text_box_and_slider_stay_in_sync() {
        let sig = signal(v2s(2.0, 0.2, 5.0));
        let text = signal("2.0".to_string());
        let mut stored = HashMap::new();
        let make_cell = |init: Value| EffectParamCell::Float {
            key: "k".into(),
            label: "k".into(),
            sig,
            text,
            min: 0.2,
            max: 5.0,
            step: 0.2,
            decimals: 1,
            init,
        };
        let cell = make_cell(json!(2.0));

        // ① referee 必须取**配置现值**（stored 里有键就压过 schema 默认 initial=2.0）：
        //   滑条/文本都与 stored=3.0 一致 → 值 3.0，且不产生任何回写。
        stored.insert("k".to_string(), json!(3.0));
        sig.set(v2s(3.0, 0.2, 5.0));
        text.set("3.0".to_string());
        let q = float_param_sync(&stored, "k", &cell, &sig, &text, 0.2, 5.0, 0.2, 1);
        assert!(
            (q - 3.0).abs() < 1e-4,
            "referee 必须取配置现值 3.0，实际 {q}"
        );
        assert!(
            (s2v(sig.get(), 0.2, 5.0) - 3.0).abs() < 1e-3,
            "一致态不得动滑条"
        );

        // ② 文本赢：滑条在基准上、文本改成合法新值 → 值跟文本，滑条跳过去。
        text.set("4.6".to_string());
        let q = float_param_sync(&stored, "k", &cell, &sig, &text, 0.2, 5.0, 0.2, 1);
        assert!((q - 4.6).abs() < 1e-4, "文本赢：值应为 4.6，实际 {q}");
        assert!(
            (s2v(sig.get(), 0.2, 5.0) - 4.6).abs() < 1e-3,
            "滑条必须跟上输入框"
        );
        assert_eq!(text.get(), "4.6", "文本赢时不得回写文本");
        // 真实流程里 `apply_param_value` 会随后把 4.6 落进 stored——模拟之。
        stored.insert("k".to_string(), json!(4.6));

        // ③ 滑条赢：滑条拖离基准 → 值跟滑条，文本回写成新值。
        sig.set(v2s(1.0, 0.2, 5.0));
        let q = float_param_sync(&stored, "k", &cell, &sig, &text, 0.2, 5.0, 0.2, 1);
        assert!((q - 1.0).abs() < 1e-4, "滑条赢：值应为 1.0，实际 {q}");
        assert_eq!(text.get(), "1.0", "滑条赢时文本必须回写");
        // 同上：模拟 apply_param_value 落盘（新值 ≠ initial 才会写，这里会写）。
        stored.insert("k".to_string(), json!(1.0));

        // ④ 中间态（基准已同步后）：空串不动值、不回写；前导零原样保留。
        text.set(String::new());
        let q = float_param_sync(&stored, "k", &cell, &sig, &text, 0.2, 5.0, 0.2, 1);
        assert!((q - 1.0).abs() < 1e-4, "空串是打字中间态：值跟滑条");
        assert_eq!(text.get(), "", "空串不得被回写打架");
        text.set("03.0".to_string());
        let q = float_param_sync(&stored, "k", &cell, &sig, &text, 0.2, 5.0, 0.2, 1);
        assert!((q - 3.0).abs() < 1e-4, "前导零解析为 3.0（越界钳制内）");
        assert_eq!(text.get(), "03.0", "前导零原样保留，不打断输入");
    }

    /// 「自动清理」下拉：下标 ↔ 天数双向对账 + 同值不落盘。
    #[test]
    fn temp_cleanup_dropdown_writes_back_days() {
        let mut cfg = crate::core::config::Config::default();
        let m = PanelModel::from_config(&cfg);
        assert_eq!(m.temp_cleanup.get(), 0, "出厂默认 = 永不（0 天）");

        // 选「7天之前」→ 写回 7。
        m.temp_cleanup.set(1);
        assert!(m.apply_to(&mut cfg), "换了档位应报改动");
        assert_eq!(cfg.general.temp_cleanup_days, 7);

        // 回读：7 天 → 下标 1；再 apply 同值不该报改动（无伪落盘）。
        let m2 = PanelModel::from_config(&cfg);
        assert_eq!(m2.temp_cleanup.get(), 1);
        assert!(!m2.apply_to(&mut cfg), "同值不应产生改动");
        assert_eq!(cfg.general.temp_cleanup_days, 7);

        // 回「永不」→ 0。
        m2.temp_cleanup.set(0);
        assert!(m2.apply_to(&mut cfg));
        assert_eq!(cfg.general.temp_cleanup_days, 0);
    }

    /// 「系统 > 管理员权限启动」勾选 → 落 `general.admin_launch`；回读再 apply 同值
    /// 不报改动（无伪落盘）——与自动清理同一套对账纪律。
    #[test]
    fn admin_launch_toggle_writes_back_and_is_stable() {
        // 出厂默认不勾。
        let mut cfg = Config::default();
        assert!(!cfg.general.admin_launch, "出厂默认必须是普通权限启动");
        let m = PanelModel::from_config(&cfg);
        assert!(!m.admin_launch.get());
        assert!(!m.apply_to(&mut cfg), "出厂值未动不应产生改动");

        // 勾选 → 落盘 true。
        m.admin_launch.set(true);
        assert!(m.apply_to(&mut cfg), "勾选应报改动");
        assert!(cfg.general.admin_launch);

        // 回读 + 同值不伪落盘。
        let m2 = PanelModel::from_config(&cfg);
        assert!(m2.admin_launch.get());
        assert!(!m2.apply_to(&mut cfg), "同值不应产生改动");

        // 取消勾选 → 落回 false（下次启动恢复普通权限）。
        m2.admin_launch.set(false);
        assert!(m2.apply_to(&mut cfg));
        assert!(!cfg.general.admin_launch);
    }

    /// 「取消勾选 → 再勾选」必须落得回去（2026-09-25 真 bug）。
    ///
    /// 旧判据 `new_val == initial → 跳过` 会让第二步永久失效：配置里留下 `show_fine=false`
    /// 就再也回不来（关掉悬浮标重开也一样，因为读的就是这份配置）。用户报的
    /// "细分刻度取消再勾选不出现、关了重开也没有"，根因就这一行。
    #[test]
    fn hover_bool_toggled_off_then_on_writes_back() {
        // 出厂：`show_fine` 默认 true，配置里还没有这个键。
        let mut cell = EffectParamCell::Bool {
            key: "show_fine".to_string(),
            label: "细分刻度".to_string(),
            sig: signal(true),
            init: json!(true),
        };
        let mut stored: HashMap<String, Value> = HashMap::new();

        // ① 什么都不动：不落盘（脚本侧有同值默认）。
        assert!(
            !apply_param_value(&mut stored, &cell, json!(true), "show_fine"),
            "没动过不该产生落盘"
        );
        assert!(stored.is_empty());

        // ② 取消勾选 → 落盘 false。
        cell = EffectParamCell::Bool {
            key: "show_fine".to_string(),
            label: "细分刻度".to_string(),
            sig: signal(false),
            init: json!(true),
        };
        assert!(apply_param_value(
            &mut stored,
            &cell,
            json!(false),
            "show_fine"
        ));
        assert_eq!(stored.get("show_fine"), Some(&json!(false)));

        // ③ ★ 再勾回来：值回到了 `initial`，但配置里现在是 false —— 必须把 true
        //    **写回配置**。绝不能摘键交回脚本默认：initial 可能来自配置现值而非默认，
        //    摘键会让脚本悄悄跳回默认值（"取消勾选没反应"的真凶，2026-09-25 二版）。
        cell = EffectParamCell::Bool {
            key: "show_fine".to_string(),
            label: "细分刻度".to_string(),
            sig: signal(true),
            init: json!(true),
        };
        assert!(
            apply_param_value(&mut stored, &cell, json!(true), "show_fine"),
            "勾回来必须产生一次配置改动（写回 true），否则配置永久停在 false"
        );
        assert_eq!(
            stored.get("show_fine"),
            Some(&json!(true)),
            "必须显式写回 true，不能摘键"
        );

        // ④ 再取消一次：仍然要能落盘 false。
        cell = EffectParamCell::Bool {
            key: "show_fine".to_string(),
            label: "细分刻度".to_string(),
            sig: signal(false),
            init: json!(true),
        };
        assert!(apply_param_value(
            &mut stored,
            &cell,
            json!(false),
            "show_fine"
        ));
        assert_eq!(stored.get("show_fine"), Some(&json!(false)));

        // ⑤ "没动过不落盘"的承诺只对"配置无键"成立：配置已有键且与面板一致时
        //    对账结果就是"已一致"，同样零改动（不能为它摘键、也不能反复写）。
        cell = EffectParamCell::Bool {
            key: "show_fine".to_string(),
            label: "细分刻度".to_string(),
            sig: signal(false),
            init: json!(false), // 初值来自配置现值（面板重建后的形态）
        };
        assert!(
            !apply_param_value(&mut stored, &cell, json!(false), "show_fine"),
            "配置已是 false 且没动过 → 零改动"
        );
    }

    /// 颜色参数的位数由脚本声明的 `alpha` 决定，且与 `hex_to_color` 严格互逆
    /// （互逆不成立 = 打开面板什么都不动也被判成"改过"，每帧反复落盘）。
    #[test]
    fn color_cell_hex_width_follows_alpha_flag() {
        let c = Color::rgba(0x00, 0xE5, 0xCC, 0x80);
        assert_eq!(fmt_color(c, false), "#00E5CC", "不带 alpha → 6 位");
        assert_eq!(fmt_color(c, true), "#00E5CC80", "带 alpha → 8 位 #RRGGBBAA");

        // 互逆：8 位串解析回来必须还是同一个颜色（含 alpha）。
        let back = hex_to_color(&fmt_color(c, true));
        assert_eq!((back.r, back.g, back.b, back.a), (0x00, 0xE5, 0xCC, 0x80));
        // 6 位串解析回来 alpha 恒为不透明。
        let back6 = hex_to_color(&fmt_color(c, false));
        assert_eq!(back6.a, 255);
    }

    /// ★★ 真实链路回归：配置 `show_fine=false` → 打开面板 → 勾选 → `apply_to` →
    /// 配置必须变 true；反向同理。用**真 `PanelModel`** 而不是裸 helper——
    /// 这条链上任何一环断了（初值读取、信号共享、落盘判据）它都会红。
    /// 用户报"细分刻度取消再勾选不出现"，就要靠它保证面板层永不再断。
    #[test]
    fn hover_show_fine_toggles_roundtrip_through_the_real_panel() {
        // 用户取消过细分刻度后的真实配置状态。
        let mut cfg = Config::default();
        cfg.trail.hover_enabled = true;
        cfg.trail.hover_type = "ruler".to_string();
        cfg.trail
            .hover_params
            .entry("ruler".to_string())
            .or_default()
            .insert("show_fine".to_string(), json!(false));

        // 打开面板：真实构建路径。
        let m = PanelModel::from_config(&cfg);
        let cell = m
            .hover_params
            .get()
            .into_iter()
            .find(|c| c.key() == "show_fine")
            .expect("悬浮标标尺面板必须有细分刻度单元格");
        let EffectParamCell::Bool { sig, init, .. } = cell else {
            panic!("show_fine 应是 Bool 单元格");
        };
        assert_eq!(init, json!(false), "初值必须是配置现值 false");
        assert!(!sig.get(), "复选框应显示未勾选");

        // 勾上 → sync_panel 每 250ms 的真实落盘路径。
        sig.set(true);
        let mut back = cfg.clone();
        assert!(m.apply_to(&mut back), "勾选细分刻度必须产生一次配置改动");
        assert_eq!(
            back.trail.hover_params["ruler"]["show_fine"],
            json!(true),
            "勾选必须写进配置"
        );

        // 再取消 → 必须写回 false（不能停在 true）。
        sig.set(false);
        let mut back = back.clone();
        assert!(m.apply_to(&mut back), "取消勾选必须产生一次配置改动");
        assert_eq!(
            back.trail.hover_params["ruler"]["show_fine"],
            json!(false),
            "取消必须写回 false"
        );

        // 再勾回来（用户报的第三步）：到这一步配置已经是 false，最怕"判成没动过"。
        sig.set(true);
        let mut back = back.clone();
        assert!(
            m.apply_to(&mut back),
            "再勾选必须产生一次配置改动（2026-09-25 修的正是这一步）"
        );
        assert_eq!(back.trail.hover_params["ruler"]["show_fine"], json!(true),);
    }

    /// ★ 换脚本即刷新（2026-09-28 用户口径）：配置只记录**当前选中脚本**及其参数。
    ///
    /// 三个断言面：
    /// 1. 切走 → 旧脚本的参数条目**整条删除**（含老版本残留的带前缀键）；
    /// 2. 新脚本没动过参数 → 不落盘（schema 默认接管）；
    /// 3. 切回来 → **不复活**旧调参，单元初值即 schema 默认。
    #[test]
    fn hover_switch_drops_previous_script_params_and_starts_fresh() {
        let mut cfg = Config::default();
        cfg.trail.hover_type = "solid".to_string();
        cfg.trail
            .hover_params
            .entry("solid".to_string())
            .or_default()
            .insert("color".to_string(), json!("#00FF0080"));
        // 老版本残留：带前缀键 + "已删除脚本"的条目，都必须被对账清掉。
        cfg.trail
            .hover_params
            .entry("HoverDot/solid".to_string())
            .or_default()
            .insert("color".to_string(), json!("#12345678"));
        cfg.trail
            .hover_params
            .entry("vanished".to_string())
            .or_default()
            .insert("x".to_string(), json!(1));

        let m = PanelModel::from_config(&cfg);
        // 切到 ruler。
        let ruler = m.hover_catalog.index_of("ruler");
        assert_ne!(ruler, usize::MAX, "测试依赖内置 ruler 脚本在目录里");
        m.hover_type.set(ruler);
        let mut back = cfg.clone();
        assert!(m.apply_to(&mut back), "切换 + 清理必须产生配置改动");
        assert!(
            !back.trail.hover_params.contains_key("solid"),
            "旧脚本的参数必须整条删除"
        );
        assert!(
            !back.trail.hover_params.contains_key("HoverDot/solid")
                && !back.trail.hover_params.contains_key("vanished"),
            "带前缀残留 / 已删除脚本的条目必须清掉"
        );
        assert!(
            !back.trail.hover_params.contains_key("ruler"),
            "新脚本没动过参数不落盘（schema 默认接管）"
        );
        // 新单元必须整批换成 ruler 的 schema（键集合与 schema 一一对应）。
        let mut cell_keys: Vec<String> = m
            .hover_params
            .get()
            .iter()
            .map(|c| c.key().to_string())
            .collect();
        cell_keys.sort();
        let mut schema_keys: Vec<String> = m
            .hover_catalog
            .schema_for("ruler")
            .iter()
            .map(|p| p.key.clone())
            .collect();
        schema_keys.sort();
        assert_eq!(
            cell_keys, schema_keys,
            "切走后参数单元必须整批换成新脚本的 schema"
        );

        // 切回 solid → 不复活旧调参。
        let solid = m.hover_catalog.index_of("solid");
        m.hover_type.set(solid);
        let mut back2 = back.clone();
        assert!(m.apply_to(&mut back2), "切回必须产生改动（清上一任）");
        assert_eq!(
            back2.trail.hover_params.get("solid"),
            None,
            "切回旧脚本不得复活旧调参——每次切换都是刷新"
        );
        let color = m
            .hover_params
            .get()
            .into_iter()
            .find(|c| c.key() == "color")
            .expect("solid 必须有 color 单元");
        assert_eq!(
            *color.initial(),
            json!("#FF000040"),
            "单元初值必须是 schema 默认色"
        );
    }

    /// ★ 拖尾 extra 同口径：命名空间键只留当前特效的，老配置残留的其它特效键清掉；
    /// 非命名空间键不属于任何脚本，保留。
    #[test]
    fn trail_extra_prunes_keys_of_other_effects() {
        let mut cfg = Config::default();
        cfg.trail
            .trail_settings
            .extra
            .insert("glow.color".to_string(), json!("#123456"));
        cfg.trail
            .trail_settings
            .extra
            .insert("not_namespaced".to_string(), json!(true));

        let m = PanelModel::from_config(&cfg);
        let mut back = cfg.clone();
        assert!(m.apply_to(&mut back), "清理残留必须产生配置改动");
        assert!(
            !back.trail.trail_settings.extra.contains_key("glow.color"),
            "其它特效的命名空间键必须清掉"
        );
        assert_eq!(
            back.trail.trail_settings.extra.get("not_namespaced"),
            Some(&json!(true)),
            "非命名空间键不属于任何脚本，保留"
        );
    }

    /// ★ 2026-09-26 晚出厂口径的**面板侧**锚点：信号初值必须等于新出厂值。
    ///
    /// `core::config` 里的用例锁的是"配置层"，这条锁的是"用户打开面板第一眼看到的
    /// 东西"——两者中间还隔着 `from_config` 那一层翻译（来源下标 / 档位下标 /
    /// 每脚本开关），翻译错了配置层再对也没用。
    #[test]
    fn panel_signals_start_at_the_2026_09_26_factory_defaults() {
        let cfg = Config::default();
        let m = PanelModel::from_config(&cfg);

        // 设个本地别名，避免下面几行被完整路径淹掉（`DEFAULT_MIC_DENOISE` 是唯一方向源的那一处）。
        use crate::core::config::{DEFAULT_MIC_DENOISE, MIC_DENOISE_LABELS};

        // ① 三把录制快捷键：面板上是空的（= 未配键，停用）。
        assert!(m.hk_record.get().is_empty(), "录屏快捷键框出厂应为空");
        assert!(m.hk_gif.get().is_empty(), "GIF 录制快捷键框出厂应为空");
        assert!(m.hk_gif_trim.get().is_empty(), "裁剪快捷键框出厂应为空");
        assert!(
            m.hk_record_region.get().is_empty(),
            "选区录屏快捷键框出厂应为空"
        );
        assert!(
            m.hk_record_trim.get().is_empty(),
            "MP4 裁剪快捷键框出厂应为空"
        );

        // ② 音频两路：**下拉停在第 2 项「默认」**（0=关闭 / 1=默认）。
        assert_eq!(m.rec_mic_source.get(), AUDIO_SRC_DEFAULT);
        assert_eq!(m.rec_speaker_source.get(), AUDIO_SRC_DEFAULT);

        // ③ 降噪档位下标必须指向「中」，且这个下标的标签真是「中」——下标与标签表
        //   错位会让用户以为选了"中"、实际录出来是另一档，且不报错。
        assert_eq!(m.rec_denoise.get(), DEFAULT_MIC_DENOISE);
        assert_eq!(MIC_DENOISE_LABELS[DEFAULT_MIC_DENOISE], "中");

        // ④ 脚本出厂全关：**每一种 role 的开关初值都必须是未勾选**（2026-09-27 起
        //   所有 role 都画开关，composite 的运行开关与 image 的菜单开关都算）。
        //   （扫不到脚本 = 环境不对，`assets/lua` 没解析到——这时直接红，
        //   而不是悄悄退化成"零张卡、全通过"的假绿。）
        assert!(
            !m.lua_ext_cards.get().is_empty(),
            "脚本卡列表空了——`assets/lua` 没扫到，这条用例会退化成假绿"
        );
        assert!(
            m.lua_ext_cards
                .get()
                .iter()
                .any(|c| c.id == "KeySound/class_sound"),
            "内置音效脚本没扫到，这条用例遍历范围不对"
        );
        for card in m.lua_ext_cards.get() {
            assert!(
                !card.toggle.get(),
                "脚本「{}」（role={}）出厂不应是开着",
                card.id,
                card.role
            );
        }
    }

    /// 脚本卡的开关按 role 落进**对应**的列表：image → `image_active`、
    /// composite → `composite_active`，互不得串（串了就变成"勾了没反应"或"关了还在跑"）。
    #[test]
    fn script_card_toggles_write_to_the_list_of_their_own_role() {
        // 图片体检 / OCR 迁到 CurSeen-Scripts 后，内置已经没有 image 脚本；这条判据
        // 只关心"开关按 role 落到各自的表"，所以用两枚临时脚本造出 image + composite。
        let dir = std::env::temp_dir().join(format!("curseen_cards_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::write(
            dir.join("scripts/demo_image.lua"),
            "role = \"image\"\nname = \"图片演示\"\nmenu = \"图片演示\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("scripts/demo_composite.lua"),
            "role = \"composite\"\nname = \"复合演示\"\n",
        )
        .unwrap();
        let cfg = Config::default();
        let m = PanelModel::from_config(&cfg);
        m.lua_ext_cards.set(build_lua_cards(&cfg, &dir));
        let cards = m.lua_ext_cards.get();
        let image_card = cards
            .iter()
            .find(|c| c.role == "image")
            .expect("临时 image 脚本没扫到，这条用例退化成假绿");
        let composite_card = cards
            .iter()
            .find(|c| c.role == "composite")
            .expect("临时 composite 脚本没扫到");

        let mut back = cfg.clone();
        assert!(
            !m.apply_to(&mut back),
            "打开面板原样 diff 回去不应产生任何改动"
        );
        // 勾上 image 卡：只许动 image_active，composite_active 必须原封不动。
        image_card.toggle.set(true);
        assert!(m.apply_to(&mut back));
        assert_eq!(
            back.lua_ext.image_active,
            vec![image_card.id.clone()],
            "image 卡开关只该进 image_active"
        );
        assert!(
            back.lua_ext.composite_active.is_empty(),
            "image 卡开关污染了 composite_active"
        );
        // 再勾上 composite 卡：两列同时有值，各自独立。
        composite_card.toggle.set(true);
        let mut back2 = back.clone();
        assert!(m.apply_to(&mut back2));
        assert_eq!(
            back2.lua_ext.composite_active,
            vec![composite_card.id.clone()]
        );
        assert_eq!(
            back2.lua_ext.image_active, back.lua_ext.image_active,
            "写 composite 不该回头改 image 那一列"
        );
        // 关掉 image 卡：只从 image_active 里摘，composite_active 不受牵连。
        image_card.toggle.set(false);
        let mut back3 = back2.clone();
        assert!(m.apply_to(&mut back3));
        assert!(
            back3.lua_ext.image_active.is_empty(),
            "关掉就该从列表里移除"
        );
        assert_eq!(
            back3.lua_ext.composite_active,
            back2.lua_ext.composite_active
        );
    }

    /// 打开面板再原样 diff 回去，不应产生任何改动（否则会每帧误报改动、反复落盘）。
    /// ★ 击键音效参数：按脚本分段落盘 + 切换重建 + 重启恢复（对齐 hover_params）。
    /// ★ 2026-09-25 改版：音效选择是脚本参数里的 `file` 浏览按钮（无外部文件夹配置）。
    #[test]
    fn ksound_params_are_per_script_and_survive_restart() {
        let mut cfg = Config::default();
        cfg.key_sound.enabled = true;
        cfg.key_sound.script = "KeySound/unified_sound".into();
        let m = PanelModel::from_config(&cfg);
        // 统一音效 = 「音效」file 浏览 + 「长按连发出声」bool，共 2 个参数。
        let keys: Vec<String> = m
            .ksound_params
            .get()
            .iter()
            .map(|c| c.key().to_string())
            .collect();
        assert_eq!(
            keys,
            vec!["sound".to_string(), "repeat_sound".to_string()],
            "统一音效应是 file + bool 两个参数：{keys:?}"
        );
        // 关掉「长按连发出声」→ apply_to 只写进 统一音效 自己的段。
        for c in &m.ksound_params.get() {
            if let EffectParamCell::Bool { sig, .. } = c {
                sig.set(false);
            }
        }
        let mut back = cfg.clone();
        assert!(m.apply_to(&mut back), "改参数应产生配置变更");
        assert_eq!(
            back.key_sound
                .params
                .get("KeySound/unified_sound")
                .and_then(|p| p.get("repeat_sound")),
            Some(&json!(false))
        );
        // 切到分类音效：单元重建为它自己的 schema（5 个音效 + 连发开关，共 6 个，键名不同）。
        let idx = m.ksound_catalog.index_of("KeySound/class_sound");
        m.ksound_script.set(idx);
        let mut back2 = back.clone();
        assert!(m.apply_to(&mut back2), "换脚本应产生配置变更");
        let keys2: Vec<String> = m
            .ksound_params
            .get()
            .iter()
            .map(|c| c.key().to_string())
            .collect();
        assert_eq!(
            keys2.len(),
            6,
            "分类音效应重建出 6 个参数单元（5 类音效 + 连发开关）：{keys2:?}"
        );
        assert!(
            keys2.contains(&"letter_sound".to_string()),
            "切换后应是分类音效自己的参数：{keys2:?}"
        );
        // ★ 五类音效都是 `file` 单元（浏览按钮），初值为空（没挑过 = 落回默认音）。
        let cells2 = m.ksound_params.get();
        let letter = cells2
            .iter()
            .find(|c| c.key() == "letter_sound")
            .expect("分类音效要有字母键音效");
        if let EffectParamCell::File { sig, init, .. } = letter {
            assert_eq!(sig.get(), "", "没挑过时音效应为空");
            assert_eq!(init, &json!(""), "初值也应为空（脚本默认）");
        } else {
            panic!("letter_sound 应是 File 单元（浏览按钮）");
        }
        // 模拟用户挑了一个音（信号直设，等价于浏览按钮回写）→ 落进分类音效自己的段。
        for c in &m.ksound_params.get() {
            if let EffectParamCell::File { sig, .. } = c {
                if c.key() == "letter_sound" {
                    sig.set("thock.wav".to_string());
                }
            }
        }
        let mut back3 = back2.clone();
        assert!(m.apply_to(&mut back3), "挑音效应产生配置变更");
        assert_eq!(
            back3.key_sound.params["KeySound/class_sound"]["letter_sound"],
            json!("thock.wav")
        );
        // 重启模拟：用落盘后的配置重建面板——当前脚本已是「分类音效」，参数单元
        // 仍是它自己的 6 项；统一音效段里关掉的连发也随配置保留。
        let m2 = PanelModel::from_config(&back3);
        let keys3: Vec<String> = m2
            .ksound_params
            .get()
            .iter()
            .map(|c| c.key().to_string())
            .collect();
        assert_eq!(
            keys3,
            vec![
                "fn_sound".to_string(),
                "mod_sound".to_string(),
                "symbol_sound".to_string(),
                "digit_sound".to_string(),
                "letter_sound".to_string(),
                "repeat_sound".to_string(),
            ],
            "重启后仍应停在分类音效自己的参数上：{keys3:?}"
        );
        assert_eq!(
            back3.key_sound.params["KeySound/unified_sound"]["repeat_sound"],
            json!(false)
        );
    }

    #[test]
    fn model_no_spurious_change() {
        let cfg = Config::default();
        let m = PanelModel::from_config(&cfg);
        let mut back = cfg.clone();
        assert!(!m.apply_to(&mut back), "未操作控件不该改配置");
        assert_eq!(back, cfg);
    }

    /// 包卡：`scripts/CursorFX/` 下的平铺 `.lua` 必须聚合成**一张**卡，
    /// 下拉项按 id 排序，且出厂不动不写 `pack_selected`。
    #[test]
    fn cursorfx_pack_is_one_card_with_flat_effects() {
        let cfg = Config::default();
        let m = PanelModel::from_config(&cfg);
        let cards = m.lua_ext_cards.get();
        let card = cards
            .iter()
            .find(|c| c.id == "scripts/CursorFX")
            .expect("应扫描到 CursorFX 包卡");
        let sel = card.selector.as_ref().expect("包卡应有下拉选择器");
        let names = sel.names.get();
        assert_eq!(names.len(), 42, "CursorFX 应有 42 个子脚本");
        let effect_names = sel
            .effects
            .borrow()
            .iter()
            .map(|e| e.name.clone())
            .collect::<Vec<_>>();
        for expected in ["fire", "ring", "ripple", "bubbles", "stars"] {
            assert!(
                effect_names.iter().any(|n| n == expected),
                "缺少 {expected}"
            );
        }
        assert!(names.iter().any(|n| n == "bubbles (拖尾)"));
        assert!(names.iter().any(|n| n == "bubbles (点击)"));
        assert_eq!(
            sel.effects
                .borrow()
                .get(sel.selected.get())
                .map(|e| e.name.as_str()),
            Some("fire"),
            "pack.json.default 应选中 fire"
        );
        assert!(sel.default_id.ends_with("/fire"));
        assert!(!card.toggle.get(), "包卡应关闭");
        assert!(
            card.params.get().len() >= 3,
            "默认子脚本 fire 的四项参数应已生成"
        );
        assert!(
            !m.lua_ext_cards
                .get()
                .iter()
                .any(|c| c.id.starts_with("scripts/CursorFX/")),
            "包内子脚本不得各自成卡"
        );
        let mut back = cfg.clone();
        assert!(!m.apply_to(&mut back), "包卡首次出现不应写 pack_selected");
        assert_eq!(back, cfg);
    }

    /// MouseEffects 包卡：33 个子脚本聚合为一张卡，默认项是 pack.json 的 aurora，
    /// 子脚本不得各自成卡。
    #[test]
    fn mouseeffects_pack_is_one_card_with_flat_effects() {
        let cfg = Config::default();
        let m = PanelModel::from_config(&cfg);
        let cards = m.lua_ext_cards.get();
        let card = cards
            .iter()
            .find(|c| c.id == "scripts/MouseEffects")
            .expect("应扫描到 MouseEffects 包卡");
        let sel = card.selector.as_ref().expect("包卡应有下拉选择器");
        let names = sel.names.get();
        assert_eq!(names.len(), 33, "MouseEffects 应有 33 个子脚本");
        let effect_names = sel
            .effects
            .borrow()
            .iter()
            .map(|e| e.name.clone())
            .collect::<Vec<_>>();
        for expected in ["Aurora", "Firework", "Zoom", "Tesla", "Nebula"] {
            assert!(
                effect_names.iter().any(|n| n == expected),
                "缺少 {expected}"
            );
        }
        assert_eq!(
            sel.effects
                .borrow()
                .get(sel.selected.get())
                .map(|e| e.name.as_str()),
            Some("Aurora"),
            "pack.json.default 应选中 aurora"
        );
        assert!(sel.default_id.ends_with("/aurora"));
        assert!(!card.toggle.get(), "包卡应关闭");
        assert!(
            !m.lua_ext_cards
                .get()
                .iter()
                .any(|c| c.id.starts_with("scripts/MouseEffects/")),
            "包内子脚本不得各自成卡"
        );
    }

    /// `single: true` 的工具包（OCR/条码/图片体检）只出一个普通脚本卡，不摆一项的下拉；
    /// 普通多脚本包仍聚合成包卡。
    #[test]
    fn single_pack_renders_a_plain_card_and_multi_pack_still_aggregates() {
        let root = std::env::temp_dir().join(format!("curseen_single_{}", std::process::id()));
        let scripts = root.join("scripts");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(scripts.join("Tool")).unwrap();
        std::fs::create_dir_all(scripts.join("Pack")).unwrap();
        std::fs::write(
            scripts.join("Tool/pack.json"),
            r#"{"id":"Tool","name":"工具","version":"1.0.0","category":"text","single":true}"#,
        )
        .unwrap();
        std::fs::write(
            scripts.join("Tool/tool.lua"),
            "name = \"工具\"
category = \"text\"
role = \"image\"
function run(ctx) return \"ok\" end
",
        )
        .unwrap();
        std::fs::write(
            scripts.join("Pack/pack.json"),
            r#"{"id":"Pack","name":"包","version":"1.0.0","default":"one"}"#,
        )
        .unwrap();
        for name in ["one", "two"] {
            std::fs::write(
                scripts.join(format!("Pack/{name}.lua")),
                format!(
                    "name = \"{name}\"
kind = \"trail\"
function run(ctx) return \"ok\" end
"
                ),
            )
            .unwrap();
        }

        let cards = build_lua_cards(&crate::core::config::Config::default(), &root);
        let tool = cards
            .iter()
            .find(|c| c.id == "scripts/Tool/tool")
            .expect("single 包应出普通脚本卡");
        assert!(tool.selector.is_none(), "普通卡不许带包下拉");
        assert_eq!(tool.category, "text");
        assert!(
            !cards.iter().any(|c| c.id == "scripts/Tool"),
            "single 包不许再出包卡"
        );
        let pack = cards
            .iter()
            .find(|c| c.id == "scripts/Pack")
            .expect("多脚本包仍应聚合");
        assert_eq!(
            pack.selector
                .as_ref()
                .expect("多脚本包应有下拉")
                .names
                .get()
                .len(),
            2
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 包卡下拉切换 / 开关必须落到“当前子脚本 id”的 `composite_active`，
    /// 而不是包 id；回到出厂默认项时 `pack_selected` 要清空。
    #[test]
    fn cursorfx_pack_switch_and_toggle_drive_composite_active() {
        let mut cfg = Config::default();
        cfg.lua_ext.params.insert(
            "scripts/CursorFX/ring".to_string(),
            HashMap::from([("size".to_string(), json!(60))]),
        );
        cfg.lua_ext.params.insert(
            "scripts/CursorFX/fire".to_string(),
            HashMap::from([("rate".to_string(), json!(3.0))]),
        );
        let m = PanelModel::from_config(&cfg);
        let cards = m.lua_ext_cards.get();
        let card = cards
            .iter()
            .find(|c| c.id == "scripts/CursorFX")
            .expect("应扫描到 CursorFX 包卡");
        let sel = card.selector.as_ref().expect("包卡应有下拉选择器");
        let ring_idx = sel
            .effects
            .borrow()
            .iter()
            .position(|e| e.name == "ring")
            .expect("缺少 ring");
        let fire_idx = sel
            .effects
            .borrow()
            .iter()
            .position(|e| e.name == "fire")
            .expect("缺少 fire");

        sel.selected.set(ring_idx);
        let mut back = cfg.clone();
        assert!(m.apply_to(&mut back), "切换子脚本应写配置");
        assert_eq!(
            back.lua_ext.pack_selected.get("scripts/CursorFX"),
            Some(&"scripts/CursorFX/ring".to_string())
        );
        assert!(
            back.lua_ext.composite_active.is_empty(),
            "包卡开关仍关着，不应启动"
        );
        assert!(
            !back.lua_ext.params.contains_key("scripts/CursorFX/ring")
                && !back.lua_ext.params.contains_key("scripts/CursorFX/fire"),
            "切换子脚本应丢弃本包旧参数，新脚本从 schema 默认起步"
        );

        card.toggle.set(true);
        let mut back = cfg.clone();
        assert!(m.apply_to(&mut back), "打开包卡应启动当前子脚本");
        assert!(
            back.lua_ext
                .composite_active
                .iter()
                .any(|id| id == "scripts/CursorFX/ring"),
            "composite_active 里应是子脚本 id，不是包 id"
        );

        sel.selected.set(fire_idx);
        assert!(m.apply_to(&mut back), "切回默认项应更新运行列表");
        assert!(
            back.lua_ext.pack_selected.is_empty(),
            "停在 default 时应清掉 pack_selected"
        );
        assert!(
            back.lua_ext
                .composite_active
                .iter()
                .any(|id| id == "scripts/CursorFX/fire"),
            "新子脚本应接替运行"
        );
    }

    /// ★★ 每一根数值滑块都必须"存进去什么、原样读回来什么"。
    ///
    /// 滑块内部恒存 `0..=1`，配置里是真实量纲（秒 / 比例 / 倍数），两端靠 `v2s`/`s2v`
    /// 抵消。**漏做一端**是这个项目反复犯的同一类错（2026-09-18 虚拟键盘键帽缩成一小块；
    /// 2026-09-21 画中画倍率拖了恒为 1）：初值看着正常、一拖滑块就把 `0.xx` 当成真实值
    /// 写进配置。
    ///
    /// 只测 `Config::default()` 的固定点抓不到它 —— 默认值换算后仍是默认值，**看起来是
    /// 自洽的**。必须把滑块拨到别处再读回来，才逼得出量程错位。所以这里逐个字段：
    /// 取一个非默认的真实值 → `from_config` → `apply_to` → 必须回到同一个值。
    /// 一根数值滑块的往返检查用例：(字段说明, 改配置, 读滑块, 从滑块读回配置)。
    type SliderCase = (
        &'static str,
        fn(&mut Config),
        fn(&PanelModel) -> f32,
        fn(&Config) -> f32,
    );

    #[test]
    fn every_real_valued_slider_roundtrips_through_the_panel() {
        let base = Config::default();
        let cases: Vec<SliderCase> = vec![
            (
                "倒计时时长(秒)",
                |c| c.global.timer.duration_sec = 1500,
                |m| m.timer_duration.get(),
                |c| c.global.timer.duration_sec as f32,
            ),
            (
                "计时器水平偏移",
                |c| c.global.timer.offset_x = 30,
                |m| m.timer_offset_x.get(),
                |c| c.global.timer.offset_x as f32,
            ),
            (
                "计时器垂直偏移",
                |c| c.global.timer.offset_y = 15,
                |m| m.timer_offset_y.get(),
                |c| c.global.timer.offset_y as f32,
            ),
            (
                "计时器字号",
                |c| c.global.timer.font_pt = 96,
                |m| m.timer_font.get(),
                |c| c.global.timer.font_pt as f32,
            ),
            (
                "画中画面板宽",
                |c| c.magnifier_pip.panel_width = 0.3,
                |m| m.pip_panel_w.get(),
                |c| c.magnifier_pip.panel_width,
            ),
            (
                "画中画面板高",
                |c| c.magnifier_pip.panel_height = 0.45,
                |m| m.pip_panel_h.get(),
                |c| c.magnifier_pip.panel_height,
            ),
            (
                "画中画放大倍率",
                |c| c.magnifier_pip.zoom = 4,
                |m| m.pip_zoom.get(),
                |c| c.magnifier_pip.zoom as f32,
            ),
            (
                "画中画水平偏移",
                |c| c.magnifier_pip.offset_x = 37,
                |m| m.pip_offset_x.get(),
                |c| c.magnifier_pip.offset_x as f32,
            ),
            (
                "画中画垂直偏移",
                |c| c.magnifier_pip.offset_y = 12,
                |m| m.pip_offset_y.get(),
                |c| c.magnifier_pip.offset_y as f32,
            ),
            (
                // ★ 量程 2026-09-21 起统一收敛到 1–5，取 5（到顶、非默认 2）：
                //   若哪天有人把上限改回去或把全屏倍率接回 `PIP_ZOOM_*`，这条会立刻红。
                "全屏放大倍率",
                |c| c.magnifier_fullscreen.zoom = 5,
                |m| m.fs_zoom.get(),
                |c| c.magnifier_fullscreen.zoom as f32,
            ),
        ];
        for (name, set, read_slider, read_cfg) in cases {
            let mut cfg = base.clone();
            set(&mut cfg);
            let want = read_cfg(&cfg);
            let m = PanelModel::from_config(&cfg);
            // 滑条位置必须在 0..=1 内（真实值直接塞进来时会 >1，滑条自己会 clamp 掉）。
            let at = read_slider(&m);
            assert!(
                (0.0..=1.0).contains(&at),
                "{name}：滑块装的是 {at}，不在 0..=1 内 —— 真实值没走 `v2s` 归一化"
            );
            let mut back = cfg.clone();
            m.apply_to(&mut back);
            let got = read_cfg(&back);
            assert!(
                (got - want).abs() <= 1.0,
                "{name}：拖都没拖就被改写成 {got}（原值 {want}）—— 两端换算没有抵消"
            );
        }
    }

    /// §35 全滑条三件套的门禁（2026-09-25）：**固定参数**滑条（`slider_pair_sync`
    /// 消费者）的输入框同样是第二入口——以击键音效音量为样本钉四条：
    /// ① 文本赢 → 配置跟文本、滑条跳过去；② 拖滑条 → 文本回写；③ 空串中间态不动
    /// 配置也不回写打架；④ 对账收敛，不得反复改写。
    #[test]
    fn fixed_slider_text_entry_wins_and_converges() {
        let cfg = Config::default();
        let m = PanelModel::from_config(&cfg);
        let initial = cfg.key_sound.volume;

        // ① 文本赢。
        m.ksound_volume_text.set("77".to_string());
        let mut back = cfg.clone();
        assert!(m.apply_to(&mut back), "输入框编辑必须产生一次配置改动");
        assert_eq!(back.key_sound.volume, 77);
        assert!(
            (m.ksound_volume.get() - v2s(77.0, 0.0, 100.0)).abs() < 1e-6,
            "滑条没跟上输入框"
        );
        assert_eq!(m.ksound_volume_text.get(), "77", "文本赢时不得回写文本");

        // ② 滑条赢：拖到 30 → 配置 30，文本回写成 "30"。
        m.ksound_volume.set(v2s(30.0, 0.0, 100.0));
        assert!(m.apply_to(&mut back), "拖滑条必须产生一次配置改动");
        assert_eq!(back.key_sound.volume, 30);
        assert_eq!(m.ksound_volume_text.get(), "30", "滑条动后文本必须回写");

        // ③ 空串是打字中间态：配置不动、文本不被回写成旧值。
        m.ksound_volume_text.set(String::new());
        let mut back2 = back.clone();
        assert!(!m.apply_to(&mut back2), "空串不该动配置");
        assert_eq!(back2.key_sound.volume, 30);
        assert_eq!(m.ksound_volume_text.get(), "");

        // ④ 收敛：对账完再同步一次不得产生第二笔改动。
        m.ksound_volume_text.set("30".to_string());
        assert!(!m.apply_to(&mut back2), "对账必须收敛，不得反复改写");
        let _ = initial;
    }

    /// 时长输入框（2026-09-25）：文本是滑条之外的第二入口，四条路都要对——
    /// ① 文本编辑写进配置并带动滑条；② 拖滑条回写文本；③ 空串/非数字等打字中间态
    /// 不动配置、也不回写打架；④ 越界钳制且前导零原样保留。最后钉一条收敛：
    /// 对账完不得反复改写（`model_no_spurious_change` 的时长专项版）。
    #[test]
    fn timer_duration_text_box_and_slider_stay_in_sync() {
        let cfg = Config::default();
        let m = PanelModel::from_config(&cfg);
        assert_eq!(m.timer_duration_text.get(), "300");

        // ① 文本赢：输入 125 → 配置 125，滑条刻度跟着跳，文本保持原样（不重排）。
        m.timer_duration_text.set("125".to_string());
        let mut back = cfg.clone();
        assert!(m.apply_to(&mut back), "输入框编辑必须产生一次配置改动");
        assert_eq!(back.global.timer.duration_sec, 125);
        let at = v2s(125.0, TIMER_DURATION_MIN as f32, TIMER_DURATION_MAX as f32);
        assert!(
            (m.timer_duration.get() - at).abs() < 1e-6,
            "滑条没跟上输入框"
        );
        assert_eq!(m.timer_duration_text.get(), "125");

        // ② 滑条赢：拖到 600（过 `v2s` 刻度）→ 配置 600，文本回写成 "600"。
        m.timer_duration.set(v2s(
            600.0,
            TIMER_DURATION_MIN as f32,
            TIMER_DURATION_MAX as f32,
        ));
        assert!(m.apply_to(&mut back), "拖滑条必须产生一次配置改动");
        assert_eq!(back.global.timer.duration_sec, 600);
        assert_eq!(m.timer_duration_text.get(), "600", "滑条动后文本必须回写");

        // ③ 中间态：空串不产生改动、配置不动、文本也不被回写成旧值（清空重输）。
        m.timer_duration_text.set(String::new());
        let mut back2 = back.clone();
        assert!(!m.apply_to(&mut back2), "空串是打字中间态，不该动配置");
        assert_eq!(back2.global.timer.duration_sec, 600);
        assert_eq!(m.timer_duration_text.get(), "", "中间态不得被回写打架");

        // ④ 越界 + 前导零："09999" → 钳到 3600，文本原样保留（不打断输入）。
        m.timer_duration_text.set("09999".to_string());
        let mut back3 = back2;
        assert!(m.apply_to(&mut back3), "越界值钳制后仍是一次改动");
        assert_eq!(back3.global.timer.duration_sec, 3600);
        assert_eq!(m.timer_duration_text.get(), "09999");

        // 收敛：同一状态下再同步一次，不得产生第二笔改动。
        let mut back4 = back3.clone();
        assert!(!m.apply_to(&mut back4), "对账必须收敛，不得反复改写");
        assert_eq!(back4, back3);
    }

    /// 拖动倍率滑条**必须真的改到倍率**：这是用户报的那个症状的直接判据。
    ///
    /// 修复前：滑块被拨到 0.75 → `round()` → 1 → `clamp(1,5)` → 仍是 1，拖到头都不动。
    /// 这条把"拨到最右就该是 5 倍"钉住，不依赖任何具体默认值。
    #[test]
    fn dragging_the_zoom_slider_actually_changes_the_zoom() {
        let cfg = Config::default();
        let m = PanelModel::from_config(&cfg);
        // ★ 先钉**读端**：滑块初值必须是"默认倍率"在量程里的刻度，而不是真实值本身。
        //   这条单独就能抓住"真实值直塞滑块"——只测拨动是不够的（拨动会覆盖掉错误的
        //   初值，于是写端对了就绿，读端的错被漏掉）。
        let at = v2s(
            cfg.magnifier_pip.zoom as f32,
            PIP_ZOOM_MIN as f32,
            PIP_ZOOM_MAX as f32,
        );
        assert!(
            (m.pip_zoom.get() - at).abs() < 1e-6,
            "倍率滑块初值应是默认 {} 倍对应的刻度 {at}，实际 {}",
            cfg.magnifier_pip.zoom,
            m.pip_zoom.get()
        );
        let mut seen = Vec::new();
        // ★ 配置必须**跨轮累积**（2026-09-25 §35 起倍率滑条带镜像文本）：对账判据
        //   "谁与基准偏离谁赢"的前提是基准（配置现值）与信号状态同一条时间线——
        //   上一轮拖动后文本被回写成 "1"，若每轮仍拿原始默认配置当基准，
        //   "sv==基准 && 文本≠sv" 会让**上一轮的旧文本**反过来吃掉这一轮的拖动。
        //   真实应用里配置是连续演进的（面板开着持续 apply），这里与之同构。
        let mut back = cfg.clone();
        for pos in [0.0f32, 0.34, 0.67, 1.0] {
            m.pip_zoom.set(pos);
            m.apply_to(&mut back);
            seen.push(back.magnifier_pip.zoom);
        }
        assert!(
            seen.iter().zip(seen.iter().skip(1)).all(|(a, b)| b > a),
            "倍率必须随滑块单调上升，实际 {seen:?}"
        );
        assert_eq!(seen.first(), Some(&PIP_ZOOM_MIN), "滑块拉到最左 = 最小倍率");
        assert_eq!(seen.last(), Some(&PIP_ZOOM_MAX), "滑块拉到最右 = 最大倍率");
    }

    /// ★ 老配置文件（带已删除的截图键、缺本轮新增的环容量键）补齐后，面板首帧
    /// **也不该**报改动。
    ///
    /// 这是"出厂默认必须是面板解析后的固定点"在**存量配置**上的回归：新增键的默认值若
    /// 与它对应的控件（下拉档位 / 快捷键框）解析不回同一处，用户只是打开面板就会让
    /// 配置被无端重写一遍——而且每开一次都可能再写一次。
    ///
    /// ★ 2026-09-17：老配置里还带着 `hotkey_shot_last` / `save_dir` / `copy_to_clipboard`
    /// / `notify` / `clipboard_poll` / `pin_hover_outline` 六个已删除的键，它们必须被
    /// **静默忽略**（不能因为"读到一个不认识的键"就回退默认或报错）。
    #[test]
    fn legacy_screenshot_keys_backfill_without_spurious_change() {
        // 逐字取自 2.0.0 之前真实落盘的那份 screenshot 段（含已删除键）。
        let old = r#"{"screenshot":{"hotkey":"Ctrl+F1","hotkey_pin":"Ctrl+Alt+Q",
            "hotkey_pin_clipboard":"Ctrl+Alt+P","save_dir":"","copy_to_clipboard":true,
            "notify":true,"hotkey_shot_last":"Alt+Shift+S","clipboard_poll":true,
            "pin_hover_outline":false}}"#;
        let cfg: Config = serde_json::from_str(old).expect("老配置应能反序列化");
        assert_eq!(cfg.screenshot.pin_ring_cap, 10, "缺失的环容量应补出厂默认");
        assert_eq!(cfg.screenshot.hotkey, "Ctrl+F1", "老键的值必须原样保留");

        let m = PanelModel::from_config(&cfg);
        let mut back = cfg.clone();
        assert!(!m.apply_to(&mut back), "老配置补齐后不该被面板改写");
        assert_eq!(back, cfg);
    }

    #[test]
    fn trail_switch_writes_its_own_card() {
        // ★ 开关架构：拖尾效果卡片的开关只写 `trail.trail_enabled`，功能总开关已废除。
        let cfg = Config::default();
        let m = PanelModel::from_config(&cfg);
        m.trail_enabled.set(true);
        m.trail_effect.set(m.catalog.index_of("ribbon"));
        let mut back = cfg.clone();
        assert!(m.apply_to(&mut back));
        assert!(back.trail.trail_enabled);
        assert_eq!(back.trail.trail_effect, "ribbon");
    }

    /// §38「只存当前方案」：切到新方案时，落盘里**所有**命名空间参数键（含新旧两个
    /// 方案自己的）一并丢弃，新方案从脚本 schema 默认起步；同一方案内不切换时调参照常保留。
    #[test]
    fn switching_effect_drops_all_stored_params() {
        let mut cfg = Config::default();
        cfg.trail.trail_effect = "pixie_dust".into();
        cfg.trail
            .trail_settings
            .extra
            .insert("pixie_dust.particle_size".into(), serde_json::json!(20.0));
        cfg.trail
            .trail_settings
            .extra
            .insert("ribbon.max_width".into(), serde_json::json!(20.0));
        let m = PanelModel::from_config(&cfg);
        m.trail_effect.set(m.catalog.index_of("ribbon"));
        let mut back = cfg.clone();
        assert!(m.apply_to(&mut back));
        assert!(
            !back
                .trail
                .trail_settings
                .extra
                .keys()
                .any(|k| k.contains('.')),
            "切方案后不应残留任何命名空间参数键，实际 {:?}",
            back.trail.trail_settings.extra
        );
        // 再 apply 一帧：单元初值 = schema 默认，new == initial 不落键，配置保持干净。
        let mut back2 = back.clone();
        assert!(!m.apply_to(&mut back2), "切完方案后零操作不该产生伪落盘");
    }

    /// §38 颜色影子修复：legacy 共享字段 `trail_settings.fixed_color`（Python 时代
    /// 默认 `#FFFF00`）不得再盖住脚本 schema 默认 `#FF0000`——用户实测 UI 与脚本
    /// 颜色不一致的根因。
    #[test]
    fn legacy_shared_fixed_color_no_longer_shadows_schema_default() {
        let mut cfg = Config::default();
        cfg.trail.trail_settings.fixed_color = "#FFFF00".into();
        let m = PanelModel::from_config(&cfg);
        let cell = m
            .effect_params
            .get()
            .into_iter()
            .find(|c| c.key() == "fixed_color")
            .unwrap_or_else(|| panic!("pixie_dust schema 应声明 fixed_color"));
        match cell {
            EffectParamCell::Color { sig, alpha, .. } => {
                assert_eq!(
                    fmt_color(sig.get(), alpha),
                    fmt_color(hex_to_color("#FF0000"), false),
                    "UI 固定色应来自脚本 schema 默认，而非共享字段"
                );
            }
            _ => panic!("fixed_color 应是 Color 单元"),
        }
    }

    #[test]
    fn feature_card_switches_write_their_own_sections() {
        // ★ 开关架构：击键回显/屏幕书写卡片的开关分别写 `key_display.enabled` /
        // `drawing.enabled`（原 `global.enabled_key/enabled_draw` 已废除）。
        let cfg = Config::default();
        let m = PanelModel::from_config(&cfg);
        m.enabled_key.set(true);
        m.enabled_draw.set(true);
        let mut back = cfg.clone();
        assert!(m.apply_to(&mut back));
        assert!(back.key_display.enabled);
        assert!(back.drawing.enabled);
        // 运行时帧门按各功能段派生：开任一卡片即激活。
        assert!(back.trail_module_active() || back.key_display.enabled || back.drawing.enabled);
    }

    /// ★★ 两段语义（2026-09-26 用户口径）：勾选 = 载入，两个模块**可同时载入**
    /// （互斥移到运行态：`runtime::zoom` 激活一边退另一边）。面板这里只保证
    /// **取消勾选 = 顺带退出运行态**——载出模块时不许残留快捷键激活的窗。
    /// 另外勾选位本身不得产生伪 diff（打开面板不碰开关 = 零改动）。
    #[test]
    fn unchecking_a_magnifier_also_exits_its_running_state() {
        let mut cfg = Config::default();
        cfg.magnifier_pip.enabled = true;
        cfg.magnifier_pip.active = true;
        cfg.magnifier_fullscreen.enabled = true;
        cfg.magnifier_fullscreen.active = true;
        let m = PanelModel::from_config(&cfg);
        // 两个都载入：互不排挤。
        assert!(m.pip_enabled.get() && m.fs_enabled.get(), "载入态不互斥");
        // 取消画中画勾选：它的运行态必须一并退出；全屏不受牵连。
        m.pip_enabled.set(false);
        let mut back = cfg.clone();
        assert!(m.apply_to(&mut back));
        assert!(!back.magnifier_pip.enabled);
        assert!(!back.magnifier_pip.active, "载出要连带退出运行态");
        assert!(back.magnifier_fullscreen.enabled);
    }

    /// 出厂默认仍然自洽（新增的 `magnifier_fullscreen` 段不该让面板首帧误报改动）。
    #[test]
    fn fullscreen_zoom_defaults_are_a_fixed_point() {
        let cfg = Config::default();
        let m = PanelModel::from_config(&cfg);
        let mut back = cfg.clone();
        assert!(!m.apply_to(&mut back), "未操作控件不该改配置");
        assert_eq!(back, cfg);
    }

    #[test]
    fn panel_page_roundtrips_through_config() {
        // ★ 主窗口状态记忆：切页 → 配置（下标 + 页 id 双写）；重开面板（from_config）
        //   按 id 还原。页 id 是稳定键，页序重排后老配置照样落回正确的页。
        let cfg = Config::default();
        let m = PanelModel::from_config(&cfg);
        assert_eq!(cfg.ui.panel_page, 0);
        m.page.set(crate::ui::nav::NAV_SCREEN);
        let mut back = cfg.clone();
        assert!(m.apply_to(&mut back), "切页应报改动");
        assert_eq!(back.ui.panel_page, crate::ui::nav::NAV_SCREEN);
        assert_eq!(back.ui.panel_page_id.as_deref(), Some("screen"));

        // 老配置：没有页 id，只有 8 页时代的下标 —— 按映射表翻新页。
        // 旧 3 =「录制」并进了屏幕页，旧 4 =「其它」改名工具页，旧 7 = Lua → 脚本页。
        let mut legacy = cfg.clone();
        legacy.ui.panel_page = 3;
        legacy.ui.panel_page_id = None;
        let m2 = PanelModel::from_config(&legacy);
        assert_eq!(m2.page.get(), crate::ui::nav::NAV_SCREEN);
        let mut legacy4 = cfg.clone();
        legacy4.ui.panel_page = 4;
        legacy4.ui.panel_page_id = None;
        assert_eq!(
            PanelModel::from_config(&legacy4).page.get(),
            crate::ui::nav::NAV_TOOL
        );
        let mut legacy7 = cfg.clone();
        legacy7.ui.panel_page = 7;
        legacy7.ui.panel_page_id = None;
        assert_eq!(
            PanelModel::from_config(&legacy7).page.get(),
            crate::ui::nav::NAV_LUA
        );

        // 手改配置越界（99）时兜底到第一页，导航不至于失明。
        let mut bad = cfg.clone();
        bad.ui.panel_page = 99;
        bad.ui.panel_page_id = None;
        let m3 = PanelModel::from_config(&bad);
        assert_eq!(m3.page.get(), 0);
    }

    #[test]
    fn hover_switch_writes_hover_enabled() {
        // ★ 开关架构：悬浮标卡片开关直写 trail.hover_enabled（无 cursor_mode 中转）。
        let cfg = Config::default();
        let m = PanelModel::from_config(&cfg);
        let mut back = cfg.clone();
        m.hover_enabled.set(true);
        assert!(m.apply_to(&mut back));
        assert!(back.trail.hover_enabled);
        m.hover_enabled.set(false);
        assert!(m.apply_to(&mut back));
        assert!(!back.trail.hover_enabled);
    }

    #[test]
    fn kd_display_mode_switch_writes_config() {
        // ★ 显示模式下拉（原「鼠标按键」开关测试改写——该开关已删，改为模式下拉直写）。
        let cfg = Config::default();
        let m = PanelModel::from_config(&cfg);
        let mut back = cfg.clone();
        m.display_mode.set(1); // "排除鼠标"
        assert!(m.apply_to(&mut back));
        assert_eq!(back.key_display.display_mode, "no_mouse");
        m.display_mode.set(0); // "全部"
        assert!(m.apply_to(&mut back));
        assert_eq!(back.key_display.display_mode, "all");
    }

    #[test]
    fn hover_type_switch_and_image_writeback() {
        let cfg = Config::default();
        let m = PanelModel::from_config(&cfg);
        m.hover_image.set("cursor.png".into());
        let png_idx = m.hover_catalog.index_of("png");
        m.hover_type.set(png_idx);
        let mut back = cfg.clone();
        assert!(m.apply_to(&mut back));
        assert_eq!(back.trail.hover_image, "cursor.png");
        assert_eq!(back.trail.hover_type, "png");
    }

    #[test]
    fn hover_param_edit_writes_only_changed_keys() {
        // 打开面板不动任何控件 → 不产生参数落盘（无伪变更）。
        let cfg = Config::default();
        let m = PanelModel::from_config(&cfg);
        let mut back = cfg.clone();
        assert!(!m.apply_to(&mut back));
        assert!(back.trail.hover_params.is_empty());
    }

    #[test]
    fn legacy_cursor_color_survives_untouched() {
        // ★ 迁移语义：cursor_color 是老字段，新架构不再从 UI 写它；但已有值必须原样保留
        // （迁移读它 → hover_params 种子化）。
        let mut cfg = Config::default();
        cfg.trail.trail_settings.fixed_color = "#FFFF00".into(); // 6 位
        cfg.trail.cursor_color = "#FF000080".into(); // 8 位
        let m = PanelModel::from_config(&cfg);
        let mut back = cfg.clone();
        assert!(!m.apply_to(&mut back));
        assert_eq!(back.trail.trail_settings.fixed_color, "#FFFF00");
        assert_eq!(back.trail.cursor_color, "#FF000080");
    }

    #[test]
    fn blank_hotkey_is_dropped_to_empty_string() {
        let cfg = Config::default();
        let m = PanelModel::from_config(&cfg);
        m.hk_screen.set("   ".into());
        let mut back = cfg.clone();
        m.apply_to(&mut back);
        assert_eq!(back.drawing.hotkeys.draw_hk_screen, "");
    }

    /// ★ 回归锁：每张卡的展开信号必须**各自独立**。
    ///
    /// 曾经的写法 `let key_expanded = [signal(false); 5]` 只求值一次（数组重复语法 +
    /// `Signal: Copy`），五个子卡共用同一个句柄 → 点一个，五个全展开。这里用"改一个、
    /// 断言其余不动"来钉死它——比比较句柄地址更贴近用户看到的现象。
    #[test]
    fn card_expand_signals_are_independent() {
        let cfg = Config::default();
        let m = PanelModel::from_config(&cfg);

        // ★ 2026-09-25 单卡化后波纹/声音各只剩一张卡；独立性改用「击键回显」的
        //   三张子卡验证（这是同一套 `m.expand` 机制）。
        m.expand("key_display.pos").set(true);
        assert!(m.expand("key_display.pos").get());
        for k in ["display", "color"] {
            assert!(
                !m.expand(&format!("key_display.{k}")).get(),
                "key_display.{k} 跟着 key_display.pos 一起展开了：信号被共用"
            );
        }
        // 声音父卡：改它，波纹父卡（以及别的卡）都不能动（2026-09-25 晚出厂全折叠，
        // 改"展开声音"来验证波纹不受牵连）。
        m.expand("sound").set(true);
        assert!(m.expand("sound").get());
        assert!(
            !m.expand("ripple").get(),
            "声音父卡动了，波纹父卡也跟着动：信号被共用"
        );
        // 顶层卡同样各自独立
        m.expand("hover").set(true);
        assert!(m.expand("hover").get());
        assert!(!m.expand("key_display.display").get());
    }

    /// ★ 两张鼠标父卡（「点击效果」「点击声音」）的展开信号必须是**具名**的
    /// （2026-09-22 起它们可折叠；2026-09-25 单卡化后子卡消失，只剩父卡）。
    ///
    /// 判据取"初值 = 出厂展开态 + 两张父卡互不牵连 + 确实落盘"：卡若漏了
    /// `CARD_IDS` 里的裸名 id，`m.expand("ripple")` 会落到兜底信号（`signal(false)`
    /// 的共享句柄），于是第一条初值断言就会挂 —— 这正是不报错的静默失效。
    #[test]
    fn mouse_parent_cards_have_their_own_expand_signals() {
        let cfg = Config::default();
        let m = PanelModel::from_config(&cfg);
        for parent in ["ripple", "sound"] {
            assert!(!m.expand(parent).get(), "{parent} 卡出厂应折叠");
        }

        // 展开一张不动另一张。
        m.expand("ripple").set(true);
        assert!(m.expand("ripple").get());
        assert!(!m.expand("sound").get(), "声音卡没动过，仍是出厂折叠");

        // 展开是用户的真实选择 → 必须落盘，并在重开面板时还原。
        let mut back = cfg.clone();
        assert!(m.apply_to(&mut back), "动过展开态就该报改动");
        assert_eq!(back.ui.expanded.get("ripple"), Some(&true));
        let m2 = PanelModel::from_config(&back);
        assert!(m2.expand("ripple").get(), "波纹卡应保持展开");
        assert!(!m2.expand("sound").get(), "没动过的声音卡仍是默认折叠");
    }

    /// ★ 2026-09-25 单卡化：声音的「关闭/文件」组合下拉、共享音量与总开关的写回。
    ///
    /// 锁三件事：① 下拉选文件 = 启用该键（首项「关闭」= 停用）；② 共享音量写五键
    /// + legacy `sound_volume` 同步；③ 重开面板读回后是稳定态（无伪落盘）。
    #[test]
    fn click_sound_single_card_writeback() {
        let cfg = Config::default();
        let m = PanelModel::from_config(&cfg);

        // 出厂：总开关关（2026-09-25 晚所有功能默认全关）、五键全关（combo=0）。
        assert!(!m.sound_master.get());
        assert!(m.sound.iter().all(|s| s.combo.get() == 0));

        // 用户：左键选 mouseclick1（choices[0] = 关闭，故下标 = 目录位置 + 1）、
        // 音量拉到 80、总开关不动。
        let pos = m
            .sound_choices
            .get()
            .iter()
            .position(|n| n.starts_with("mouseclick1"))
            .expect("出厂声音应出现在下拉里");
        m.sound[0].combo.set(pos);
        m.sound_volume.set(v2s(80.0, 0.0, 100.0));

        let mut back = cfg.clone();
        assert!(m.apply_to(&mut back));
        let left = &back.trail.sound_bindings["left"];
        assert!(left.enabled, "下拉选了文件 = 启用该键");
        assert_eq!(left.file, "mouseclick1");
        assert_eq!(left.volume, 80, "共享音量应写进绑定");
        for k in ["right", "middle", "x1", "x2"] {
            assert!(!back.trail.sound_bindings[k].enabled, "{k} 仍是关闭");
            assert_eq!(
                back.trail.sound_bindings[k].volume, 80,
                "{k} 音量应与共享值一致"
            );
        }
        assert_eq!(back.trail.sound_volume, 80, "legacy 字段保持同步");
        assert!(!back.trail.sound_enabled, "总开关没动过（出厂即关）");

        // 重开面板 → 原样读回 → 再 diff 无改动（稳定态）。
        let m2 = PanelModel::from_config(&back);
        assert_eq!(m2.sound[0].combo.get(), pos, "左键下拉应停在同一位");
        let mut again = back.clone();
        assert!(!m2.apply_to(&mut again));
        assert_eq!(again, back);

        // 总开关开 → 配置 true（出厂已改关，反向验证总开关写回路径）。
        m2.sound_master.set(true);
        let mut back2 = back.clone();
        assert!(m2.apply_to(&mut back2));
        assert!(back2.trail.sound_enabled);
    }

    /// ★ 2026-09-25 单卡化：波纹组合下拉（0 = 关闭）与总开关的写回；
    /// 关闭**不丢**"上次的效果"（script 保留、参数单元不清空）。
    #[test]
    fn click_ripple_single_card_writeback() {
        let cfg = Config::default();
        let m = PanelModel::from_config(&cfg);
        assert!(!m.ripple_master.get(), "总开关出厂关（2026-09-25 晚）");
        assert!(m.ripple.iter().all(|k| k.combo.get() == 0), "五键出厂全关");

        // 右键选 choices[2]（= 目录效果下标 1）→ 启用 + script 解析 + 参数单元重建。
        m.ripple[1].combo.set(2);
        let mut back = cfg.clone();
        assert!(m.apply_to(&mut back));
        let want = m.clickfx_catalog.id_at(1);
        let right = &back.trail.ripple_bindings["right"];
        assert!(right.enabled);
        assert_eq!(right.script, want);
        assert_eq!(
            m.ripple[1].clickfx_params.get().len(),
            m.clickfx_catalog.schema_for(&want).len(),
            "换效果后参数单元应整批重建"
        );
        // 左键没动：仍关闭，script 保持出厂解析值（§39 起出厂 = disc）。
        assert!(!back.trail.ripple_bindings["left"].enabled);
        assert_eq!(back.trail.ripple_bindings["left"].script, "disc");

        // 关闭回去：enabled=false，script 保留（"上次的效果"不丢）。
        m.ripple[1].combo.set(0);
        let mut back2 = back.clone();
        assert!(m.apply_to(&mut back2));
        let right2 = &back2.trail.ripple_bindings["right"];
        assert!(!right2.enabled);
        assert_eq!(right2.script, want, "关闭不丢上次的效果");

        // 总开关开 → 配置 true（出厂已改关，反向验证总开关写回路径）。
        m.ripple_master.set(true);
        let mut back3 = back2.clone();
        assert!(m.apply_to(&mut back3));
        assert!(back3.trail.ripple_enabled);
    }

    /// ★ 2026-09-25「只记录当前方案」：换效果即**丢弃**旧效果的参数。
    ///
    /// 旧行为是泄漏：script_params 用裸参数名共键，ring 调过的 color 会被当成
    /// thin 的初值，切回 ring 又复活旧调参——等于给每个效果偷偷记一套状态。
    /// 新口径：每键只记其**当前**效果的参数；换效果清空重建（取 schema 默认值，
    /// 未动过的参数不落盘）；关闭不算换效果（script 与参数都保留）。
    #[test]
    fn ripple_script_switch_discards_stale_params() {
        let mut cfg = Config::default();
        let left = cfg
            .trail
            .ripple_bindings
            .entry("left".to_string())
            .or_default();
        left.enabled = true;
        left.script = "disc".to_string();
        left.script_params
            .insert("color".to_string(), json!("#FF0000"));

        let m = PanelModel::from_config(&cfg);
        let disc_pos = 1 + m.clickfx_catalog.index_of("disc");
        assert_eq!(
            m.ripple[0].combo.get(),
            disc_pos,
            "启用 + disc → 组合下标应就位"
        );

        // 关闭（combo=0）不算换效果：script 与参数都保留。
        m.ripple[0].combo.set(0);
        let mut back = cfg.clone();
        assert!(m.apply_to(&mut back));
        let l = &back.trail.ripple_bindings["left"];
        assert!(!l.enabled);
        assert_eq!(l.script, "disc", "关闭不丢上次的效果");
        assert_eq!(
            l.script_params.get("color").and_then(|v| v.as_str()),
            Some("#FF0000"),
            "关闭不动参数"
        );

        // 换效果（disc → multi_ring）：参数即弃；multi_ring 从默认开始，未动过的不落盘。
        let mr_pos = 1 + m.clickfx_catalog.index_of("multi_ring");
        m.ripple[0].combo.set(mr_pos);
        let mut back2 = back.clone();
        assert!(m.apply_to(&mut back2));
        let l2 = &back2.trail.ripple_bindings["left"];
        assert_eq!(l2.script, "multi_ring");
        assert!(l2.script_params.is_empty(), "旧调参应被丢弃");

        // 切回 disc：从默认开始，旧调参不复活。
        m.ripple[0].combo.set(disc_pos);
        let mut back3 = back2.clone();
        assert!(m.apply_to(&mut back3));
        let l3 = &back3.trail.ripple_bindings["left"];
        assert_eq!(l3.script, "disc");
        assert!(l3.script_params.is_empty(), "切回旧效果不复活参数");

        // 重开面板稳定态：无伪落盘。
        let m2 = PanelModel::from_config(&back3);
        assert_eq!(m2.ripple[0].combo.get(), disc_pos);
        let mut again = back3.clone();
        assert!(!m2.apply_to(&mut again));
        assert_eq!(again, back3);
    }

    /// ★ §40 颜色影子根治：legacy `e.color`（1.0 默认 #FFFF00）与历史播种的
    /// `script_params` 都不得再进入 UI —— 波纹颜色单元一律从脚本 schema 默认
    /// （#FF0000）起步。此前 §38 修了拖尾那条路，波纹这条（`migrate_clickfx`
    /// 播种 `e.color`）漏网，导致同一 bug 复发。
    #[test]
    fn ripple_legacy_color_never_shadows_schema_default() {
        let mut cfg = Config::default();
        let left = cfg
            .trail
            .ripple_bindings
            .entry("left".to_string())
            .or_default();
        left.enabled = true;
        left.color = "#FFFF00".to_string();
        left.script_params
            .insert("color".to_string(), json!("#F1F10AFF"));

        let m = PanelModel::from_config(&cfg);
        let cells = m.ripple[0].clickfx_params.get();
        let color = cells
            .iter()
            .find(|c| c.key() == "color")
            .expect("disc.lua schema 应声明 color");
        match color {
            EffectParamCell::Color { sig, alpha, .. } => {
                assert!(*alpha, "§39 起波纹颜色带透明度条");
                assert_eq!(
                    fmt_color(sig.get(), false),
                    "#FF0000",
                    "UI 颜色必须来自脚本 schema 默认，legacy #FFFF00 / 播种值不得影子"
                );
            }
            _ => panic!("color 应是 Color 单元"),
        }
    }

    /// ★ 「击键回显」的**模式**下拉 2026-09-22 从「显示」子卡搬到了**父卡标题栏**
    /// （与「虚拟键盘」标题栏的"方案"下拉同层级）。
    ///
    /// UI 树本身没有可断言的纯数据，这条锁住搬家后最容易坏的那一半：下拉的两张表 ——
    /// `*_LABELS`（显示名）与对应的值表（写进配置的字符串）—— 必须**等长同序**。
    /// 少一个显示名 = 下拉少一项（选项消失）；顺序错位 = 选「仅快捷键」写进去的却是
    /// "快捷键 + 功能键"（写错值，且不报错、不崩）。
    #[test]
    fn dropdown_value_and_label_tables_line_up() {
        for (values, labels, what) in [
            (DISPLAY_MODES, DISPLAY_MODES_LABELS, "显示内容模式"),
            (PLACEMENTS, PLACEMENTS_LABELS, "弹窗位置"),
            (PEN_STYLES, PEN_STYLES_LABELS, "笔迹样式"),
        ] {
            assert_eq!(
                values.len(),
                labels.len(),
                "{what}：显示名与配置值个数不一致"
            );
            for (i, (v, label)) in values.iter().zip(labels).enumerate() {
                assert_eq!(pick(values, i), *v, "{what}：第 {i} 项应映射到 {v}");
                assert!(!label.is_empty(), "{what}：第 {i} 项的显示名不能为空");
                assert_eq!(
                    idx_of(values, v),
                    i,
                    "{what}：{v} 的下标应为 {i}（往返抵消）"
                );
            }
        }

        // 模式下拉的初值落在配置值对应的下标上（父卡标题栏读的就是这个信号）。
        let cfg = Config::default();
        let m = PanelModel::from_config(&cfg);
        assert_eq!(
            m.display_mode.get(),
            idx_of(DISPLAY_MODES, &cfg.key_display.display_mode),
            "模式下拉初值应由 `key_display.display_mode` 反查得到"
        );
    }

    /// 展开态要能落盘、并在"下次打开面板"时还原（用户诉求：读文件配置）。
    #[test]
    fn card_expand_state_is_persisted_and_restored() {
        let cfg = Config::default();
        let m = PanelModel::from_config(&cfg);
        // 出厂（2026-09-25 晚）：全部卡片折叠
        assert!(!m.expand("trail").get());
        assert!(!m.expand("hover").get());
        assert!(!m.expand("drawing.pen").get());

        // 用户操作：展开键盘页「显示」子卡、展开拖尾、展开全局页「快捷键」集合卡
        m.expand("key_display.display").set(true);
        m.expand("trail").set(true);
        m.expand("global.hotkey").set(true);

        let mut back = cfg.clone();
        assert!(m.apply_to(&mut back), "动过展开态就该报改动");
        assert_eq!(back.ui.expanded.get("key_display.display"), Some(&true));
        assert_eq!(back.ui.expanded.get("trail"), Some(&true));
        assert_eq!(back.ui.expanded.get("global.hotkey"), Some(&true));

        // 重开面板：一律从配置读回来
        let m2 = PanelModel::from_config(&back);
        assert!(
            m2.expand("key_display.display").get(),
            "「显示」子卡应保持展开"
        );
        assert!(m2.expand("trail").get(), "拖尾应保持展开");
        assert!(m2.expand("global.hotkey").get(), "「快捷键」卡应保持展开");
        assert!(
            !m2.expand("key_display.color").get(),
            "没动过的子卡仍是默认收起"
        );
        assert!(
            !m2.expand("key_display").get() && !m2.expand("key_display.pos").get(),
            "没动过的键盘页：父卡与「位置」子卡都仍是默认收起"
        );

        // 读回来后再原样 diff，不该再产生改动（配置是稳定态）
        let mut again = back.clone();
        assert!(!m2.apply_to(&mut again));
        assert_eq!(again, back);
    }

    /// 回到出厂默认时把键删掉：配置文件只留用户真改过的项。
    #[test]
    fn returning_to_default_drops_the_stored_key() {
        let cfg = Config::default();
        let m = PanelModel::from_config(&cfg);
        m.expand("hover").set(true);
        let mut back = cfg.clone();
        assert!(m.apply_to(&mut back));
        assert_eq!(back.ui.expanded.get("hover"), Some(&true));

        // 再收回去 → 与出厂默认一致 → 键应被删掉
        m.expand("hover").set(false);
        assert!(m.apply_to(&mut back), "从非默认回到默认也算改动");
        assert!(
            !back.ui.expanded.contains_key("hover"),
            "回到默认应删键，而不是存一个等于默认的值"
        );
    }

    /// ★ 虚拟键盘控件的面板 ↔ 配置往返：滑块装的是 0..=1，两端换算必须**抵消**。
    ///
    /// 2026-09-18 的事故：只做了"读"端没做"写"端（或反之），于是面板初值看着正常，
    /// 拖一下滑块就把 0.xx 当成像素写进配置 —— 键帽大小一改，键盘缩成一小块。
    /// 这条把"存进去的是真实量纲、读回来的是滑块比例"同时钉住。
    /// ★ 回归锁（2026-09-25 夜）：击键回显背景色出厂 `#00000040`，取色器必须**显示同一个串**。
    ///
    /// 三套口径**必须完全一致**，混了就是"颜色悄悄错却不报错"：
    ///
    /// - 配置数组是 **`[R, G, B, A]`**（`array_to_color` / `color_to_array` 与模块层
    ///   `rgba_to_argb` 三处一致）；
    /// - **取色器显示**走 windui `Color::to_hex_string` = **`#RRGGBBAA`**（不透明时 6 位）——
    ///   用户眼睛看到的就是这个，所以"出厂色"要按它校验；
    /// - 写入**脚本参数 / 配置**的颜色串走 [`color_hex`]，同样是 **`#RRGGBBAA`**
    ///   （与 `Argb::parse_hex`、Lua `hex_to_rgb` 同一口径）。
    ///
    /// ★ 2026-09-25 之前第三条是 `#AARRGGBB`，于是 Lua 里"红 + 25% 透明"
    ///   （`default = "#FF000040"`）被读成不透明深蓝、标尺的 `#00FFCCBF` 被读成
    ///   全透明——同一个颜色在面板上和屏幕上不是一个颜色。现在三处逐字节一致。
    ///
    /// 写默认值时把 alpha 放首位（`[0x40, 0, 0, 0]`）会被读成"透明红"，面板显示
    /// `#40000000` —— 正是那次实测被抓住的错误。
    #[test]
    fn key_display_bg_color_default_reads_as_the_picker_shows_it() {
        let cfg = Config::default();
        assert_eq!(cfg.key_display.bg_color, [0, 0, 0, 0x40], "[R,G,B,A]");
        let m = PanelModel::from_config(&cfg);
        let c = m.color_bg.get();
        assert_eq!(
            c.to_hex_string(),
            "#00000040",
            "取色器按 #RRGGBBAA 显示，出厂色应显示成 #00000040（不是 #40000000）"
        );
        // 落盘串也必须是同一个串（三处口径已统一：不再是 #AARRGGBB）。
        assert_eq!(color_hex(c), "#00000040", "#RRGGBBAA 口径");
        assert_eq!(
            crate::core::color::Argb::parse_hex("#00000040").map(|a| (a.r(), a.g(), a.b(), a.a())),
            Some((0, 0, 0, 0x40)),
            "解析回来还是黑 + alpha 0x40"
        );
        // 反向：面板原样读回 → 落盘不变（无伪差异）。
        let mut back = cfg.clone();
        assert!(!m.apply_to(&mut back), "没动过颜色就不该报改动");
        assert_eq!(back.key_display.bg_color, [0, 0, 0, 0x40]);
    }

    /// ★ 音频来源三态下标：初始化与保存是**互逆**的，且偏移量只有一处定义。
    ///
    /// 旧实现里"开关"和"来源"是两个信号、偏移各写一遍（`i - 1`），改一处漏一处
    /// 不会报错，只会把声音录成别的设备。这条用例把偏移钉在 `AUDIO_SRC_*` 上。
    ///
    /// ★ 2026-09-25：这条此前**根本没被执行** —— `#[test]` 被误放在了前面那条用例的
    /// 文档块里（两个 `#[test]` 叠在同一个 fn 上），本体退化成死代码（编译器只报
    /// "never used"，测试数里它一直是缺席的）。
    #[test]
    fn audio_source_index_roundtrips() {
        let devs: Vec<String> = vec!["麦克风 A".into(), "麦克风 B".into()];
        // 三态 → 下标
        assert_eq!(source_index(None, &devs), 0, "关闭 = 0");
        assert_eq!(source_index(Some(""), &devs), 1, "跟随默认 = 1");
        assert_eq!(
            source_index(Some("麦克风 B"), &devs),
            3,
            "第 2 台设备 = 下标 3"
        );
        // 下标 → 三态（必须回得来）
        assert_eq!(source_from_index(0, &devs), None);
        assert_eq!(source_from_index(1, &devs), Some(""));
        assert_eq!(source_from_index(2, &devs), Some("麦克风 A"));
        assert_eq!(source_from_index(3, &devs), Some("麦克风 B"));
        // 越界不写非法状态：按「跟随默认」兜底。
        assert_eq!(source_from_index(9, &devs), Some(""));
        // 配置里那台设备枚举不到时，要补进列表（否则下拉显示「关闭」→ 一保存就丢设置）。
        let mut gone: Vec<String> = vec!["麦克风 A".into()];
        keep_configured_device(&mut gone, "已拔掉的麦");
        assert_eq!(gone.len(), 2);
        assert_eq!(gone[1], "已拔掉的麦", "应追加到末尾，不能插在前面");
        assert_eq!(
            source_index(Some("已拔掉的麦"), &gone),
            3,
            "补进去之后仍应能定位到它（列表第 2 项 → 下标 2 + 1 = 3）"
        );
    }

    /// 「默认」这一项要标出当前跟的是哪台，并且**默认设备必须从可钉死的列表里消失**。
    ///
    /// 背景（2026-09-19 用户报）：配置里钉的设备恰好就是系统默认，于是下拉显示成
    /// 「扬声器 (Realtek…)」，用户看不出这和「默认」是同一台，也看不出「默认」会跟随谁。
    #[test]
    fn default_device_is_named_and_excluded_from_the_list() {
        let def = DefaultDevice {
            full: "麦克风 (UGREEN CM379 USB Audio)".into(),
            short: "UGREEN CM379 USB Audio".into(),
        };
        let mut devs: Vec<String> = vec!["麦克风 A".into(), def.full.clone()];
        // 先补配置里钉的（若恰好是默认，补了也要被剔掉）
        keep_configured_device(&mut devs, &def.full);
        drop_default_device(&mut devs, Some(&def));
        assert_eq!(
            devs,
            vec!["麦克风 A".to_string()],
            "默认设备必须从列表里剔掉，否则和「默认 (…)」重复"
        );
        // 配置里钉的就是当前默认 → 显示成「默认」，而不是一台叫得出名字的设备。
        assert_eq!(
            source_index(Some(&def.full), &devs),
            AUDIO_SRC_DEFAULT,
            "钉的正好是当前默认时，应显示「默认」"
        );
        // 钉的是**别的**设备时照旧定位到它。
        assert_eq!(source_index(Some("麦克风 A"), &devs), AUDIO_SRC_DEVICE_BASE);
        // 没有默认端点时不能动列表（否则会把仅有的设备剔没）。
        let mut only: Vec<String> = vec!["麦克风 A".into()];
        drop_default_device(&mut only, None);
        assert_eq!(only.len(), 1);
        // 大小写不同也算同一台（Windows 设备名会随驱动换大小写）。
        let mut cased: Vec<String> = vec!["microphone (usb audio)".into()];
        drop_default_device(
            &mut cased,
            Some(&DefaultDevice {
                full: "Microphone (USB Audio)".into(),
                short: "USB Audio".into(),
            }),
        );
        assert!(cased.is_empty(), "比较应忽略大小写");
    }

    /// ★ 资产重扫后，所有"从目录扫出来"的下拉都走信号；否则面板不重开就看不到
    /// 新拖尾 / 新点击效果 / 新声音 / 新虚拟键盘方案。
    #[test]
    fn asset_scanned_dropdowns_are_signal_driven() {
        let vk = include_str!("pages/key_display.rs");
        assert!(
            vk.contains(".title_dropdown_signal(m.vk_scheme_names, m.vk_layout)"),
            "虚拟键盘方案下拉必须绑信号"
        );
        assert!(
            vk.contains("Element::host_signal(m.vk_controls"),
            "虚拟键盘控件行必须随方案表信号重建"
        );
        let trail = include_str!("pages/trail.rs");
        assert!(
            trail.contains(".title_dropdown_signal(m.ripple_choices, ks.combo)"),
            "点击效果下拉必须绑信号"
        );
        assert!(
            trail.contains("card::choice_row_signal("),
            "点击声音下拉必须绑信号"
        );
    }

    /// 重扫不能悄悄换掉用户选中的资产：按 id 重新定位，不按下标。
    /// 分类排序：有 category 的按 mouse→picture→text→ai→未知分类 先排；
    /// **没有 category 的老脚本最后**，组内仍按原来的 id 顺序。
    #[test]
    fn cards_with_category_group_first_and_uncategorized_keep_id_order() {
        let dir = std::env::temp_dir().join(format!("curseen_card_order_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        let write = |name: &str, body: &str| {
            std::fs::write(dir.join("scripts").join(name), body).unwrap();
        };
        write(
            "z_unclassified.lua",
            "role = \"image\"
name = \"Z\"
",
        );
        write(
            "a_unclassified.lua",
            "role = \"image\"
name = \"A\"
",
        );
        write(
            "m_mouse.lua",
            "role = \"image\"
category = \"mouse\"
name = \"M\"
",
        );
        write(
            "p_picture.lua",
            "role = \"image\"
category = \"picture\"
name = \"P\"
",
        );
        write(
            "t_text.lua",
            "role = \"image\"
category = \"text\"
name = \"T\"
",
        );
        write(
            "i_ai.lua",
            "role = \"image\"
category = \"ai\"
name = \"I\"
",
        );
        write(
            "x_custom.lua",
            "role = \"image\"
category = \"zzz\"
name = \"X\"
",
        );

        let cards = build_lua_cards(&Config::default(), &dir);
        let ids: Vec<&str> = cards.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "scripts/m_mouse",
                "scripts/p_picture",
                "scripts/t_text",
                "scripts/i_ai",
                "scripts/x_custom",
                "scripts/a_unclassified",
                "scripts/z_unclassified",
            ],
            "分类卡片先排，无分类老脚本按 id 排最后"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reload_asset_lists_keeps_selected_ids() {
        let cfg = Config::default();
        let mut m = PanelModel::from_config(&cfg);
        if m.catalog.display_names().len() > 2 {
            m.trail_effect.set(2);
        }
        if m.hover_catalog.display_names().len() > 1 {
            m.hover_type.set(1);
        }
        let trail = m.catalog.id_at(m.trail_effect.get());
        let hover = m.hover_catalog.id_at(m.hover_type.get());
        let vk = m.vk_schemes[m.vk_layout.get()].id.clone();
        let ks = m.ksound_catalog.id_at(m.ksound_script.get());
        let ripples: Vec<usize> = m.ripple.iter().map(|k| k.combo.get()).collect();
        let sounds: Vec<usize> = m.sound.iter().map(|k| k.combo.get()).collect();

        m.reload_asset_lists(&cfg);

        assert_eq!(m.catalog.id_at(m.trail_effect.get()), trail);
        assert_eq!(m.hover_catalog.id_at(m.hover_type.get()), hover);
        assert_eq!(m.vk_schemes[m.vk_layout.get()].id, vk);
        assert_eq!(m.ksound_catalog.id_at(m.ksound_script.get()), ks);
        assert_eq!(
            m.ripple.iter().map(|k| k.combo.get()).collect::<Vec<_>>(),
            ripples
        );
        assert_eq!(
            m.sound.iter().map(|k| k.combo.get()).collect::<Vec<_>>(),
            sounds
        );
    }

    #[test]
    fn vk_slider_roundtrip_preserves_the_value() {
        use crate::modules::virtual_keyboard::vk_config::Bind;

        let cfg = Config::default();
        let m = PanelModel::from_config(&cfg);
        let controls = m.vk_controls.get();
        let group = controls
            .iter()
            .find(|sc| sc.scheme_id == "ansi_104")
            .expect("出厂 104 方案应在控件列表里");
        let ks = group
            .items
            .iter()
            .find(|it| it.def.bind == Bind::KeySize)
            .expect("应有键帽大小控件");
        let VkControlSignal::Range(sig, _) = ks.sig else {
            panic!("键帽大小应是滑块");
        };

        // 初值 = 默认值归一化后的位置（漏算时这里会是 1.0 顶格）。
        let dflt = ks.def.default.as_f32().expect("默认值应是数值");
        let at = ks.def.kind.to_slider(dflt);
        assert!(
            (sig.get() - at).abs() < 1e-6,
            "初值应是默认值的归一化位置 {at}，实际 {}",
            sig.get()
        );

        // 滑到 80%：期望值按量程算（不写死，量程改了这条跟着走），吸附 step 1。
        let expect = ks.def.kind.from_slider(0.8);
        sig.set(0.8);
        let mut back = cfg.clone();
        assert!(m.apply_to(&mut back), "改了控件应报改动");
        let saved = back
            .virtual_keyboard
            .params
            .get("ansi_104")
            .and_then(|p| p.get(&ks.def.id));
        assert_eq!(
            saved,
            Some(&ParamValue::Float(expect)),
            "存进配置的必须是真实像素 {expect}，不是滑块位置 0.8"
        );

        // 再读回来：滑块位置该在 0.8 附近（漏算读端会变成 79 → 顶格）。
        let m2 = PanelModel::from_config(&back);
        let controls2 = m2.vk_controls.get();
        let g2 = controls2
            .iter()
            .find(|sc| sc.scheme_id == "ansi_104")
            .expect("同上");
        let ks2 = g2
            .items
            .iter()
            .find(|it| it.def.bind == Bind::KeySize)
            .expect("同上");
        let VkControlSignal::Range(sig2, _) = ks2.sig else {
            panic!("键帽大小应是滑块");
        };
        assert!(
            (sig2.get() - 0.8).abs() < 0.01,
            "读回来滑块应在 0.8 附近，实际 {}",
            sig2.get()
        );
    }

    /// ★ 虚拟键盘「显示」卡的滑条与输入框**必须同进同退**（2026-09-26 用户实测报的：
    ///   拖滑条有效果，右边输入框的数字纹丝不动）。
    ///
    /// 根因：那一支 Range 手写了一份"谁赢"的判据，抄漏了两点 ——
    ///   ① 滑条动了**不回写**镜像文本（就是用户看到的症状）；
    ///   ② 基准取的是**方案默认**而不是配置现值，于是"拖过一次之后再往输入框打字"
    ///     永远被判成滑条赢（第二个 bug，用户还没撞上）。
    /// 现在走 `slider_pair_sync` 正本，这两条由它统一保证。
    #[test]
    fn vk_slider_drags_write_back_the_input_box_and_typing_still_wins() {
        use crate::modules::virtual_keyboard::vk_config::{Bind, ControlKind};

        let cfg = Config::default();
        let m = PanelModel::from_config(&cfg);
        let controls = m.vk_controls.get();
        let ks = controls
            .iter()
            .find(|sc| sc.scheme_id == "ansi_104")
            .and_then(|sc| sc.items.iter().find(|it| it.def.bind == Bind::KeySize))
            .expect("出厂 104 方案应有键帽大小控件");
        let VkControlSignal::Range(sig, text) = ks.sig else {
            panic!("键帽大小应是滑块三件套");
        };
        let (min, max, step, decimals) = match ks.def.kind {
            ControlKind::Range {
                min,
                max,
                step,
                decimals,
                ..
            } => (min, max, step, decimals),
            _ => panic!("键帽大小应是 Range"),
        };
        let saved = |c: &Config| {
            c.virtual_keyboard
                .params
                .get("ansi_104")
                .and_then(|p| p.get(&ks.def.id))
                .and_then(|v| v.as_f32())
        };

        // ① 拖滑条 → 输入框必须跟着变（本条就是用户报的那个症状）。
        let typed_before = text.get();
        sig.set(0.8);
        let mut back = cfg.clone();
        assert!(m.apply_to(&mut back), "拖滑条应产生一次改动");
        let dragged = ks.def.kind.from_slider(0.8);
        assert_eq!(
            text.get(),
            format!("{:.*}", decimals as usize, dragged),
            "滑条动后输入框必须回写成新值（原样 {}）",
            typed_before
        );

        // ② 拖过之后再往输入框打字，仍然要生效（基准若是方案默认，这一步会失效）。
        let typed = (min + (max - min) * 0.4).round();
        assert!(
            (typed - dragged).abs() > 1.0,
            "用例前提：键入值 {} 要与滑条值 {} 不同",
            typed,
            dragged
        );
        text.set(format!("{:.0}", typed));
        let mut back2 = back.clone();
        assert!(m.apply_to(&mut back2), "输入框键入应产生一次改动");
        assert_eq!(
            saved(&back2),
            Some(quantize(typed, min, max, step, decimals)),
            "拖过滑条之后键入仍必须生效（旧实现会停在滑条值 {dragged}）"
        );
        // 滑条也要跟着输入框跳过去（三件套的两侧永远一致）。
        let at = v2s(quantize(typed, min, max, step, decimals), min, max);
        assert!(
            (sig.get() - at).abs() < 1e-4,
            "文本赢后滑条应跳到 {at}，实际 {}",
            sig.get()
        );

        // ③ 收敛：同一状态再对账一次不得改写（`model_no_spurious_change` 的专项版）。
        let mut back3 = back2.clone();
        assert!(!m.apply_to(&mut back3), "对账必须收敛");
        assert_eq!(back3, back2);
    }

    /// ★ **默认值只准有一个来源**（m17）。界面控件过去各自写一遍"缺键怎么兜"
    /// （`raw.and_then(as_bool).unwrap_or_else(|| p.default…)`），脚本侧则什么都拿不到，
    /// 于是同一个 `schema()` 有两种答案。现在两边都走 `ParamDef::value_or_default`。
    ///
    /// 这格读的是**产品真入口**（`cell_from_param` = 脚本页/特效页那一片控件的构造函数），
    /// 比的是"界面显示的初值 == 交棒给脚本的那一份默认值"，逐档类型各一格。
    /// 两边将来任何一处改动都会在这里翻红——这条判据买的就是"飘"本身。
    #[test]
    fn the_ui_initial_value_is_the_default_the_script_gets() {
        use crate::scripting::ParamType;
        let empty: HashMap<String, serde_json::Value> = HashMap::new();
        // 每档各配一个**类型对得上**的 `default`（脚本写了 `default` 的那一档走声明值）。
        // ★ 刻意不测"default 与声明类型不符"：那种时候 UI 交 `as_xxx().unwrap_or(…)`、
        //   宿主交那个不符的值——两边都没承诺过，测它等于**当场发明第三条规则**，
        //   而不是核对现有的一条。
        let typed: Vec<(ParamType, serde_json::Value)> = vec![
            (ParamType::Bool, serde_json::json!(true)),
            (ParamType::Int, serde_json::json!(4)),
            (ParamType::Float, serde_json::json!(4.5)),
            (ParamType::Choice, serde_json::json!("zzz")),
            (ParamType::Color, serde_json::json!("#00FF00")),
            (ParamType::Sound, serde_json::json!("ding")),
            (ParamType::Folder, serde_json::json!("D:/x")),
            (ParamType::Str, serde_json::json!("s")),
            (ParamType::File, serde_json::json!("D:/x.png")),
            (ParamType::Hotkey, serde_json::json!("Ctrl+F9")),
            (ParamType::Image, serde_json::json!("D:/x.png")),
        ];
        for (t, declared) in typed {
            // 两遍：脚本"没写 default"（走类型兜底）与"写了 default"（走声明值）——
            // 只测后者会漏掉 `default_value()` 里那半条兜底规则。
            for d in [serde_json::Value::Null, declared] {
                let p = ParamDef {
                    key: "k".to_string(),
                    label: "K".to_string(),
                    ptype: t,
                    min: 1.0,
                    max: 9.0,
                    step: 0.5,
                    default: d.clone(),
                    choices: vec!["7".to_string()],
                    choice_labels: Vec::new(),
                    group: String::new(),
                    alpha: true,
                };
                let want = p.value_or_default(None);
                let cell = cell_from_param(&p, &empty);
                assert_eq!(
                    cell.initial(),
                    &want,
                    "{t:?} 档（default＝{d:?}）：界面初值与脚本默认值不是同一个来源"
                );
            }
        }
    }
}
