//! 鼠标特效模块设置。对应 `config.trail` 的 Rust 侧语义层。
//!
//! Python 用一个共享 dict（`trail_settings`）同时喂所有特效，
//! 各特效 `get(key, 自己默认)` 取值。保留这个"宽 dict + 各自默认值兜底"的语义。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::color::Argb;
use crate::core::events::MouseButton;

/// 特效标识。支持内置效果和动态 Lua 脚本效果。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EffectId {
    None,
    PixieDust,
    CircleParticles,
    SpiderWeb,
    Glow,
    Ribbon,
    /// 动态 Lua 脚本效果（`id` 为脚本文件名，不含 `.lua` 后缀）。
    ///
    /// 配置与下拉里写成 [`LUA_PREFIX`] + `id`。加前缀不是洁癖：
    /// `controller::sync_settings` 解析 id 时**先查内置注册表**，一个叫 `glow.lua`
    /// 的用户脚本会被内置 `glow` 顶掉、永远加载不到，而且下拉里会出现两个同名项。
    Lua(String),
}

/// Lua 效果在配置 / UI 目录里的 id 前缀（见 [`EffectId::Lua`]）。
pub const LUA_PREFIX: &str = "lua:";

impl EffectId {
    /// 效果 id，不含 [`LUA_PREFIX`]。
    ///
    /// 运行期拿它查内置注册表 / 拼 `<id>.lua` 脚本路径，所以 Lua 变体返回裸文件名。
    pub fn as_str(&self) -> &str {
        match self {
            Self::None => "none",
            Self::PixieDust => "pixie_dust",
            Self::CircleParticles => "circle_particles",
            Self::SpiderWeb => "spider_web",
            Self::Glow => "glow",
            Self::Ribbon => "ribbon",
            Self::Lua(id) => id,
        }
    }

    /// 配置里写的 id（Lua 效果带前缀）。
    pub fn config_id(&self) -> String {
        match self {
            Self::Lua(id) => format!("{LUA_PREFIX}{id}"),
            other => other.as_str().to_string(),
        }
    }

    pub fn parse_str(s: &str) -> Self {
        if let Some(stem) = s.strip_prefix(LUA_PREFIX) {
            return Self::Lua(stem.to_string());
        }
        match s {
            "pixie_dust" => Self::PixieDust,
            "circle_particles" => Self::CircleParticles,
            "spider_web" => Self::SpiderWeb,
            "glow" => Self::Glow,
            "ribbon" => Self::Ribbon,
            // 兼容 2.0.0 之前写裸脚本名的旧配置。
            _ => Self::Lua(s.to_string()),
        }
    }
}

/// 颜色模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ColorMode {
    #[default]
    Rainbow,
    Fixed,
}

/// 点击效果样式。映射 `config.ripple_bindings[key].style` 字符串
/// （"ring"/"thin"/"disc"/"double"）。
///
/// ★ 现在**只有兜底作用**：波纹的选中效果由 `script` 决定（`assets/ClickFx/<id>.lua`，
/// 四个经典样式也已是同名脚本）。只有在脚本文件缺失、`controller` 退回 Rust `Ripple`
/// 时，本枚举才决定外观（见 `modules::trail::controller::on_click`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RippleStyle {
    /// 经典圆环（较粗描边）。
    #[default]
    Ring,
    /// 纤细圆环（细描边）。
    Thin,
    /// 实心圆斑（填充）。
    Disc,
    /// 双环（同心两圈）。
    Double,
}

impl RippleStyle {
    /// 配置字符串 → 样式（未知 / 空 → 经典圆环）。
    pub fn parse(s: &str) -> Self {
        match s {
            "thin" => RippleStyle::Thin,
            "disc" => RippleStyle::Disc,
            "double" => RippleStyle::Double,
            _ => RippleStyle::Ring,
        }
    }
    /// 样式 → 配置字符串（与 [`crate::ui::model::RIPPLE_STYLE_IDS`] 对应）。
    pub fn as_str(self) -> &'static str {
        match self {
            RippleStyle::Ring => "ring",
            RippleStyle::Thin => "thin",
            RippleStyle::Disc => "disc",
            RippleStyle::Double => "double",
        }
    }
}

/// 每键波纹参数（模块态：颜色已解析，尺寸/时长已换算）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ButtonRipple {
    pub enabled: bool,
    pub style: RippleStyle,
    pub size: f32,
    pub duration_ms: f32,
    pub color: Argb,
    /// 自定义脚本 id（空 = 内置样式；非空 = `assets/ClickFx/<id>.lua`）。
    pub script: String,
    /// 自定义脚本参数表（键 = 脚本 schema 的 key）。
    pub script_params: std::collections::HashMap<String, serde_json::Value>,
}

impl Default for ButtonRipple {
    fn default() -> Self {
        Self {
            enabled: false,
            style: RippleStyle::Ring,
            size: 100.0,
            duration_ms: 300.0,
            color: Argb(0xFFFFFFFF),
            script: String::new(),
            script_params: std::collections::HashMap::new(),
        }
    }
}

/// 每键点击音效（模块态：文件已规整成 stem，音量已钳制）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ButtonSound {
    pub enabled: bool,
    /// 声音名 = `assets/Sounds/<file>.wav` 的文件名 stem（空 = 不发声）。
    pub file: String,
    /// 0..=100。
    pub volume: u8,
}

impl Default for ButtonSound {
    fn default() -> Self {
        Self {
            enabled: false,
            file: String::new(),
            volume: 50,
        }
    }
}

/// TrailSettings 是"配置的 Rust 原样表示"，保留原始 Map 作为权威存储。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TrailSettings {
    /// ★ 每键波纹绑定（左/右/中/侧1/侧2 各自独立；键名 left/right/middle/x1/x2）。
    pub ripple_bindings: HashMap<String, ButtonRipple>,
    /// ★ 每键点击音效绑定（同上一组键名）。
    pub sound_bindings: HashMap<String, ButtonSound>,
    /// ★ 悬浮标卡片开关（唯一开关；每类型的样式参数在 `hover_params[type]` 里）。
    pub hover_enabled: bool,
    /// ★ 悬浮标类型 id（**legacy 单选**，仅作 `hover_active` 的播种源，新代码不写它）。
    pub hover_type: String,
    /// ★ 启用的悬浮标脚本 id 列表（实例 id 相对 `assets/lua`，内置脚本带 `HoverDot/`
    /// 前缀；多元素 = 多实例同显，由 `from_config` 的 `hover_active_list()` 解析）。
    pub hover_active: Vec<String>,
    /// ★ 每类型的参数表：`hover_params[type][key] = value`（缺键用脚本 schema 默认值）。
    pub hover_params: HashMap<String, HashMap<String, serde_json::Value>>,
    /// 悬浮标「PNG 图片」样式选用的图片：存**文件名**（导入到 `assets/HoverDot/` 的情况），
    /// 或由 `paths::hover_image_path` 还原成绝对路径。空串 = 未选图。
    pub hover_image: String,
    pub trail_effect: EffectId,
    pub trail_enabled: bool,
    /// 参数表（§38 起只装命名空间键 `{特效id}.{参数key}`；脚本没被改过的参数没有键，
    /// 由 schema 默认接管——"只存当前方案"）。`Default` 里那批未加前缀的历史键
    /// 已无人读取，仅保留以免连带改动 Default 的其它消费方。
    pub trail_settings: HashMap<String, serde_json::Value>,
}

impl Default for TrailSettings {
    fn default() -> Self {
        let mut ts = HashMap::new();
        ts.insert("fade_duration".into(), serde_json::json!(1.0));
        ts.insert("density".into(), serde_json::json!(5));
        ts.insert("opacity".into(), serde_json::json!(0.5));
        ts.insert("spawn_interval_ms".into(), serde_json::json!(100));
        ts.insert("particle_size".into(), serde_json::json!(5.0));
        ts.insert("color_mode".into(), serde_json::json!("rainbow"));
        ts.insert("fixed_color".into(), serde_json::json!("#FF0000"));

        Self {
            ripple_bindings: HashMap::from([
                (
                    "left".to_string(),
                    ButtonRipple {
                        color: Argb(0xFFFFFF00),
                        ..Default::default()
                    },
                ),
                (
                    "right".to_string(),
                    ButtonRipple {
                        color: Argb(0xFF0000FF),
                        ..Default::default()
                    },
                ),
                ("middle".to_string(), ButtonRipple::default()),
                ("x1".to_string(), ButtonRipple::default()),
                ("x2".to_string(), ButtonRipple::default()),
            ]),
            sound_bindings: ["left", "right", "middle", "x1", "x2"]
                .into_iter()
                .map(|k| (k.to_string(), ButtonSound::default()))
                .collect(),
            hover_enabled: false,
            hover_type: "solid".into(),
            hover_active: Vec::new(),
            hover_params: HashMap::new(),
            hover_image: String::new(),
            trail_effect: EffectId::PixieDust,
            trail_enabled: false,
            trail_settings: ts,
        }
    }
}

/// 特效默认值表。每个特效定义自己需要的键和默认值。
#[derive(Debug, Clone)]
pub struct EffectDefaults {
    pub map: HashMap<String, serde_json::Value>,
}

impl EffectDefaults {
    pub fn get_f32(&self, key: &str) -> f32 {
        self.map
            .get(key)
            .and_then(|v| v.as_f64())
            .map(|v| v as f32)
            .unwrap_or(0.0)
    }

    pub fn get_u32(&self, key: &str) -> u32 {
        self.map
            .get(key)
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .unwrap_or(0)
    }

    pub fn get_string(&self, key: &str) -> &str {
        self.map.get(key).and_then(|v| v.as_str()).unwrap_or("")
    }
}

/// 按特效解析出的强类型视图。只读，改参数走 TrailSettings::set_param。
#[derive(Debug, Clone)]
pub struct EffectParams {
    pub fade_duration: f32,
    pub density: u32,
    pub opacity: f32,
    pub spawn_interval_ms: u32,
    pub particle_size: f32,
    pub max_width: f32,
    pub color_mode: ColorMode,
    pub fixed_color: Argb,
    pub extra: HashMap<String, serde_json::Value>,
}

impl From<EffectDefaults> for EffectParams {
    fn from(d: EffectDefaults) -> Self {
        Self {
            fade_duration: d.get_f32("fade_duration"),
            density: d.get_u32("density"),
            opacity: d.get_f32("opacity"),
            spawn_interval_ms: d.get_u32("spawn_interval_ms"),
            particle_size: d.get_f32("particle_size"),
            max_width: d.get_f32("max_width"),
            color_mode: match d.get_string("color_mode") {
                "fixed" => ColorMode::Fixed,
                _ => ColorMode::Rainbow,
            },
            fixed_color: Argb::parse_hex(d.get_string("fixed_color")).unwrap_or(Argb(0xFFFF0000)),
            extra: d.map,
        }
    }
}

impl TrailSettings {
    /// 启用的悬浮标脚本 id 列表（消费端统一入口；id = 裸 stem）。
    pub fn hover_active_list(&self) -> Vec<String> {
        self.hover_active.clone()
    }

    /// 某悬浮标脚本的参数表（`hover_params[id]`，缺键 = 空 = schema 默认接管）。
    ///
    /// ★ 读取侧兼容（2026-09-28 修「悬浮标实心圆调透明度没反应」）：实例 id 带目录
    /// 前缀（`HoverDot/solid`），而 UI/配置层存的是**裸 stem**（`solid`）——先按
    /// 原样精确查，查不到退回按 `/` 切出的裸 stem。与配置层
    /// `TrailDetail::hover_params_for` 同一口径；缺这层回退时面板改任何参数
    /// （颜色 / 直径 / 透明度）都到不了脚本，实例永远吃 schema 默认值。
    pub fn hover_params_for(&self, id: &str) -> HashMap<String, serde_json::Value> {
        if let Some(m) = self.hover_params.get(id) {
            return m.clone();
        }
        let bare = id.rsplit('/').next().unwrap_or(id);
        self.hover_params.get(bare).cloned().unwrap_or_default()
    }

    /// 取原始值（不存在 → None）。
    pub fn raw_param(&self, key: &str) -> Option<&serde_json::Value> {
        self.trail_settings.get(key)
    }

    /// 写原始值。
    pub fn set_param(&mut self, key: &str, v: serde_json::Value) {
        self.trail_settings.insert(key.into(), v);
    }

    /// 按特效的默认值表解析出视图。defaults 来自 TrailEffect::default_settings()。
    /// 注意：只有当 trail_settings 中有该键时才使用它，否则用 defaults。
    pub fn view(&self, defaults: &EffectDefaults) -> EffectParams {
        let get = |key: &str| -> serde_json::Value {
            // 先检查 trail_settings 是否有这个键（不是 null 就算有）
            if let Some(v) = self.trail_settings.get(key) {
                if !v.is_null() {
                    return v.clone();
                }
            }
            // 否则用默认值
            defaults
                .map
                .get(key)
                .cloned()
                .unwrap_or(serde_json::Value::Null)
        };

        let color_mode = match get("color_mode").as_str().unwrap_or("rainbow") {
            "fixed" => ColorMode::Fixed,
            _ => ColorMode::Rainbow,
        };

        let fixed_color = get("fixed_color")
            .as_str()
            .and_then(Argb::parse_hex)
            .unwrap_or(Argb(0xFFFF0000));

        EffectParams {
            fade_duration: get("fade_duration").as_f64().unwrap_or(1.0) as f32,
            density: get("density").as_u64().unwrap_or(5) as u32,
            opacity: get("opacity").as_f64().unwrap_or(0.5) as f32,
            spawn_interval_ms: get("spawn_interval_ms").as_u64().unwrap_or(100) as u32,
            particle_size: get("particle_size").as_f64().unwrap_or(5.0) as f32,
            max_width: get("max_width").as_f64().unwrap_or(12.0) as f32,
            color_mode,
            fixed_color,
            extra: self
                .trail_settings
                .iter()
                .filter(|(k, _)| {
                    ![
                        "fade_duration",
                        "density",
                        "opacity",
                        "spawn_interval_ms",
                        "particle_size",
                        "max_width",
                        "color_mode",
                        "fixed_color",
                    ]
                    .contains(&k.as_str())
                })
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        }
    }

    /// 应用一组参数（合并而非整体替换）。
    pub fn apply_params(&mut self, kv: &[(&str, serde_json::Value)]) {
        for (k, v) in kv {
            self.trail_settings.insert((*k).into(), v.clone());
        }
    }

    /// 取某特效"自己的"参数表，键去掉命名空间前缀 `{id}.`。
    ///
    /// 例：配置里存 `pixie_dust.opacity = 0.8`，这里返回 `{"opacity": 0.8, ...}` 直接喂 Lua。
    ///
    /// ★ §38 起只认**命名空间键**：旧的"未加前缀键兜底"废除——那会把 legacy 共享字段
    /// `TrailDetail.fixed_color`（Python 时代默认 `#FFFF00`）盖到脚本 schema 默认
    /// `#FF0000` 上（用户实测 UI 与脚本颜色不一致的根因）。落盘语义同步收敛为
    /// "只存当前方案"：脚本没被用户改过的参数没有键，由 schema 默认接管。
    pub fn effect_param_map(&self, id: &str) -> HashMap<String, Value> {
        let prefix = format!("{id}.");
        let mut map = HashMap::new();
        for (k, v) in &self.trail_settings {
            if let Some(stripped) = k.strip_prefix(&prefix) {
                map.insert(stripped.to_string(), v.clone());
            }
        }
        map
    }

    /// 取某特效某参数的当前值：只认 `{id}.{key}`（§38 起无未加前缀兜底，理由见
    /// [`effect_param_map`]）；调用方自带与脚本 schema 一致的 default。
    pub fn param_value(&self, id: &str, key: &str) -> Option<&Value> {
        self.trail_settings.get(&format!("{id}.{key}"))
    }

    pub fn param_f64(&self, id: &str, key: &str, default: f64) -> f64 {
        self.param_value(id, key)
            .and_then(|v| v.as_f64())
            .unwrap_or(default)
    }

    pub fn param_u32(&self, id: &str, key: &str, default: u32) -> u32 {
        self.param_value(id, key)
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .unwrap_or(default)
    }

    /// 按键 → 每键波纹参数（键名映射 left/right/middle/x1/x2）。
    pub fn ripple_for_button(&self, button: &MouseButton) -> Option<&ButtonRipple> {
        self.ripple_bindings.get(button_key(button))
    }

    /// 按键 → 每键音效（未配置 / 未启用时返回 `None`）。
    pub fn sound_for_button(&self, button: &MouseButton) -> Option<&ButtonSound> {
        self.sound_bindings
            .get(button_key(button))
            .filter(|s| s.enabled && !s.file.is_empty())
    }
}

/// 鼠标按键 → 绑定表键名（`config.trail.ripple_bindings` / `sound_bindings` 共用）。
pub fn button_key(button: &MouseButton) -> &'static str {
    match button {
        MouseButton::Left => "left",
        MouseButton::Right => "right",
        MouseButton::Middle => "middle",
        MouseButton::X1 => "x1",
        MouseButton::X2 => "x2",
    }
}

/// 推荐设置预设。下标 = 特效选择下拉的 index（0=关闭，1..5=注册表顺序）。
/// 数据照抄 panel.py:434-439。
pub struct Preset {
    pub fade_ms: u32,
    pub density: u32,
    pub interval_ms: u32,
    pub size: u32,
    pub opacity: u32,
    pub fixed: u32,
}

pub const RECOMMEND_PRESETS: [Preset; 6] = [
    Preset {
        fade_ms: 1000,
        density: 5,
        interval_ms: 100,
        size: 5,
        opacity: 50,
        fixed: 0xFFFF00,
    },
    Preset {
        fade_ms: 2000,
        density: 5,
        interval_ms: 20,
        size: 7,
        opacity: 50,
        fixed: 0xFFFF00,
    },
    Preset {
        fade_ms: 2000,
        density: 5,
        interval_ms: 20,
        size: 7,
        opacity: 50,
        fixed: 0xFFFF00,
    },
    Preset {
        fade_ms: 10000,
        density: 30,
        interval_ms: 100,
        size: 10,
        opacity: 100,
        fixed: 0xFFFF00,
    },
    Preset {
        fade_ms: 1000,
        density: 10,
        interval_ms: 100,
        size: 10,
        opacity: 75,
        fixed: 0xFFFF00,
    },
    Preset {
        fade_ms: 2000,
        density: 10,
        interval_ms: 100,
        size: 30,
        opacity: 50,
        fixed: 0xFFFF00,
    },
];

/// 将 config 里的简单 TrailSettings 转换到模块层 TrailSettings。
pub fn from_config(config: &crate::core::config::TrailSettings) -> TrailSettings {
    // 每键波纹：config 的 bindings → 模块态（颜色解析、数值换算）。
    // ★ 总开关在这里做 AND 闸（2026-09-25 单卡化）：`ripple_enabled=false` 时五键
    //   一律不可用——控制器无需知道总开关的存在，"总开关关"翻译成"每键 enabled=false"。
    let ripple_bindings = config
        .ripple_bindings
        .iter()
        .map(|(k, b)| {
            (
                k.clone(),
                ButtonRipple {
                    enabled: b.enabled && config.ripple_enabled,
                    style: RippleStyle::parse(&b.style),
                    size: b.size as f32,
                    duration_ms: b.duration as f32,
                    color: Argb::parse_hex(&b.color).unwrap_or(Argb(0xFFFFFFFF)),
                    script: b.script.clone(),
                    script_params: b.script_params.clone(),
                },
            )
        })
        .collect();
    let mut ts = TrailSettings {
        hover_enabled: config.hover_enabled,
        hover_type: config.hover_type.clone(),
        // 多开启用列表：`hover_active` 三态在配置层解析（None = 按 hover_type 播种），
        // 模块层只见最终列表，消费端（controller 的 sync_instances）零判断。
        hover_active: config.hover_active_list(),
        hover_params: config.hover_params.clone(),
        hover_image: config.hover_image.clone(),
        ripple_bindings,
        sound_bindings: config
            .sound_bindings
            .iter()
            .map(|(k, s)| {
                (
                    k.clone(),
                    ButtonSound {
                        // ★ 总开关 AND 闸（同波纹）：`sound_enabled=false` = 五键全静音。
                        enabled: s.enabled && config.sound_enabled,
                        file: s.file.clone(),
                        volume: s.volume.min(100) as u8,
                    },
                )
            })
            .collect(),
        trail_effect: EffectId::parse_str(&config.trail_effect),
        trail_enabled: config.trail_enabled,
        ..Default::default()
    };

    // ★ §38 起不再从 TrailDetail 的共享字段播种（fade_duration/density/…/fixed_color）：
    //   那批 Python 时代字段（fixed_color 默认 #FFFF00）会盖住脚本 schema 默认
    //   #FF0000，且与"只存当前方案"矛盾。字段保留在 config 结构里仅为 serde 兼容。
    //
    // 命名空间参数（脚本特效）：`{id}.{key}` 原样并入模块 map，供控制器按前缀过滤。
    for (k, v) in &config.trail_settings.extra {
        ts.trail_settings.insert(k.clone(), v.clone());
    }

    ts
}

/// 生成推荐设置的参数。
pub fn apply_preset(idx: usize) -> Option<EffectParams> {
    if idx >= RECOMMEND_PRESETS.len() {
        return None;
    }
    let p = &RECOMMEND_PRESETS[idx];
    let _effect_id = match idx {
        1 => EffectId::PixieDust,
        2 => EffectId::CircleParticles,
        3 => EffectId::SpiderWeb,
        4 => EffectId::Glow,
        5 => EffectId::Ribbon,
        _ => EffectId::None,
    };
    Some(EffectParams {
        fade_duration: p.fade_ms as f32 / 1000.0,
        density: p.density,
        opacity: p.opacity as f32 / 100.0,
        spawn_interval_ms: p.interval_ms,
        particle_size: p.size as f32,
        max_width: p.size as f32,
        color_mode: ColorMode::Rainbow,
        fixed_color: Argb::new(
            0xFF,
            (p.fixed >> 16) as u8,
            (p.fixed >> 8) as u8,
            p.fixed as u8,
        ),
        extra: HashMap::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_id_roundtrip() {
        for id in [
            EffectId::PixieDust,
            EffectId::CircleParticles,
            EffectId::SpiderWeb,
            EffectId::Glow,
            EffectId::Ribbon,
        ] {
            let s = id.as_str().to_string();
            assert_eq!(EffectId::parse_str(&s), id);
        }
    }

    #[test]
    fn effect_id_lua() {
        let id = EffectId::Lua("my_custom_effect".to_string());
        // as_str 给运行期用来拼脚本路径，必须是裸文件名。
        assert_eq!(id.as_str(), "my_custom_effect");
        // 写进配置用带前缀的形式，避免与内置 id 撞车。
        assert_eq!(id.config_id(), "lua:my_custom_effect");
        assert_eq!(EffectId::parse_str("lua:my_custom_effect"), id);
        assert!(matches!(id, EffectId::Lua(_)));
    }

    /// ★ 回归锁（2026-09-28 用户报告「悬浮标实心圆调透明度没反应」）：模块层
    /// `hover_params_for` 必须兼容裸 stem 键——UI/配置层存 `solid`，运行实例 id 是
    /// `HoverDot/solid`，缺回退时面板改任何参数都到不了脚本。
    #[test]
    fn hover_params_lookup_falls_back_to_bare_stem() {
        let mut ts = TrailSettings::default();
        ts.hover_params.insert(
            "solid".to_string(),
            HashMap::from([("color".to_string(), serde_json::json!("#FF000054"))]),
        );
        // 带前缀查（运行实例 id 形态）→ 必须命中裸 stem 键。
        let got = ts.hover_params_for("HoverDot/solid");
        assert_eq!(got.get("color"), Some(&serde_json::json!("#FF000054")));
        // 带前缀键精确命中优先，不受回退影响。
        ts.hover_params.insert(
            "HoverDot/ruler".to_string(),
            HashMap::from([("show_fine".to_string(), serde_json::json!(true))]),
        );
        let got = ts.hover_params_for("HoverDot/ruler");
        assert_eq!(got.get("show_fine"), Some(&serde_json::json!(true)));
        // 完全没配 → 空（schema 默认接管）。
        assert!(ts.hover_params_for("HoverDot/absent").is_empty());
    }

    /// 内置同名脚本（如用户自己写 `glow.lua`）不能再被内置 id 顶掉。
    #[test]
    fn lua_script_named_like_builtin_is_reachable() {
        let id = EffectId::parse_str("lua:glow");
        assert_eq!(id, EffectId::Lua("glow".to_string()));
        assert_eq!(id.as_str(), "glow");
        // 裸 `glow` 仍按内置解析，旧配置不受影响。
        assert_eq!(EffectId::parse_str("glow"), EffectId::Glow);
    }

    /// 2.0.0 之前配置里直接写裸脚本名，仍需能解析成 Lua 效果。
    #[test]
    fn legacy_bare_lua_id_still_parses() {
        assert_eq!(
            EffectId::parse_str("some_user_script"),
            EffectId::Lua("some_user_script".to_string())
        );
    }

    #[test]
    fn settings_default_values() {
        let s = TrailSettings::default();
        assert_eq!(s.hover_type, "solid");
        assert!(!s.hover_enabled);
        assert!(s.hover_params.is_empty());
        // 五键波纹绑定齐全且默认关闭
        for key in ["left", "right", "middle", "x1", "x2"] {
            let b = s
                .ripple_bindings
                .get(key)
                .unwrap_or_else(|| panic!("缺 {key}"));
            assert!(!b.enabled);
            assert_eq!(b.size, 100.0);
        }
        // 五键音效绑定齐全且默认关闭
        for key in ["left", "right", "middle", "x1", "x2"] {
            let b = s
                .sound_bindings
                .get(key)
                .unwrap_or_else(|| panic!("缺 {key}"));
            assert!(!b.enabled);
            assert_eq!(b.volume, 50);
        }
        assert!(!s.trail_enabled);
    }

    #[test]
    fn view_uses_defaults() {
        let mut ts = TrailSettings::default();
        // 清空共享参数表，让 view() 走 defaults 路径
        ts.trail_settings.clear();
        let defaults = EffectDefaults {
            map: vec![
                ("fade_duration".into(), serde_json::json!(2.0)),
                ("density".into(), serde_json::json!(10)),
                ("particle_size".into(), serde_json::json!(8.0)),
            ]
            .into_iter()
            .collect(),
        };
        let v = ts.view(&defaults);
        assert_eq!(v.fade_duration, 2.0);
        assert_eq!(v.density, 10);
        assert_eq!(v.particle_size, 8.0);
    }

    #[test]
    fn recommend_presets_count() {
        assert_eq!(RECOMMEND_PRESETS.len(), 6);
        // Preset 1 and 2 are identical (per Python)
        assert_eq!(RECOMMEND_PRESETS[1].fade_ms, RECOMMEND_PRESETS[2].fade_ms);
        assert_eq!(RECOMMEND_PRESETS[1].density, RECOMMEND_PRESETS[2].density);
    }
}
