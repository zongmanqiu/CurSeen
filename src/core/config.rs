//! 配置读写。文件位置：`<程序目录>/config.json`（便携化，见 `core::paths`）。
//!
//! 三个必须保持不变的性质（否则用户升级时会丢配置）：
//!   1. 字段名与 Python 版**完全一致**（snake_case）
//!   2. 缺失字段用 `#[serde(default)]` 补默认值 → 老配置文件能直接读
//!   3. 未知字段**忽略**（不报错）→ 不加 `#[serde(deny_unknown_fields)]`

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::error::{CurSeenError, Result};

/// 父目录 fsync 的实现，由 `platform` 在启动时注册（见 [`Config::save`] 第 ④ 步）。
///
/// ★ 用注入而不是直接调 `platform::win_util::sync_dir`：依赖方向是
///   `core ← platform`，`core` 反向引用 `platform` 就是分层违规（AGENTS.md 第三节）。
///   抓屏那条路（`core::backdrop::Backdrop::capture` 收一个抓屏函数）是同一个手法。
/// 未注册时（单测 / 平台未提供）跳过该步——见 `save` 里的说明。
static DIR_SYNC: OnceLock<fn(&Path) -> bool> = OnceLock::new();

/// 注册父目录 fsync 实现。幂等：重复注册保留**先注册**的那个（钩子只应有一个实现）。
pub fn set_dir_sync(f: fn(&Path) -> bool) {
    let _ = DIR_SYNC.set(f);
}

/// 测试用：钩子是否已注册（避免测试之间互相污染时无法判因）。
#[cfg(test)]
pub(crate) fn dir_sync_registered() -> bool {
    DIR_SYNC.get().is_some()
}

/// 五个鼠标按键的键名。`ripple_bindings` / `sound_bindings` 的键就是它，
/// 面板子卡的展开态 id（`ripple.<key>` / `sound.<key>`）也由它派生。
///
/// **单一定义点**：`ui::model::BUTTON_KEYS` 是从这里转出去的别名，别再写第二份字面量。
pub const BUTTON_KEYS: [&str; 5] = ["left", "right", "middle", "x1", "x2"];

// ─── 子配置节 ───

/// 每键波纹参数（左/右/中/侧1/侧2 各自独立）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ButtonRippleCfg {
    // ── 以下四个是 1.0 时代的内置样式字段，**现在只读不写** ──
    /// 旧内置样式 id。已改写成 `assets/ClickFx/` 里的同名脚本（ring / thin / disc /
    /// double），故 `from_config` 会把空 `script` 的旧配置按它迁移过去；此后不再更新。
    /// 仅当脚本文件缺失、退回 Rust `Ripple` 兜底时才会被读。
    pub style: String,
    /// 旧波纹尺寸（**直径**，10..500）。迁移时按 `max_size = size / 2` 播种给脚本。
    pub size: u32,
    /// 旧扩散时长（毫秒，100..3000）。迁移时按同名的 `duration` 参数播种给脚本。
    pub duration: u32,
    /// 旧波纹颜色（`#RRGGBB` 或 `#RRGGBBAA`）。迁移时按同名的 `color` 参数播种给脚本。
    pub color: String,

    // ── 当前模型 ──
    pub enabled: bool,
    /// 效果脚本 id = `assets/ClickFx/<id>.lua` 的文件名 stem。**这是效果的唯一来源**：
    /// 四个内置样式也已改写成脚本，所以这里没有"内置样式"这个分支了。
    pub script: String,
    /// 脚本参数表（键 = 脚本 `schema()` 里的参数 key）。
    pub script_params: HashMap<String, Value>,
}

/// 每键点击音效（左/右/中/侧1/侧2 各自独立）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ButtonSoundCfg {
    pub enabled: bool,
    /// 声音名 = `assets/Sounds/<file>.wav` 的文件名 stem。
    pub file: String,
    /// 该键的音量 0..=100。
    pub volume: u32,
}

impl Default for ButtonSoundCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            file: String::new(),
            // 与老版全局音量默认值一致，升级后手感不变。
            volume: default_sound_volume(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrailSettings {
    #[serde(default)]
    pub cursor_mode: u32,
    #[serde(default = "default_cursor_outer")]
    pub cursor_outer: u32,
    #[serde(default)]
    pub cursor_arc: u32,
    #[serde(default = "default_cursor_color")]
    pub cursor_color: String,
    /// ★ 悬浮标卡片开关（唯一开关；旧 cursor_mode 字段仅作迁移源）。
    #[serde(default)]
    pub hover_enabled: bool,
    /// ★ 启用的悬浮标脚本 id 列表（2026-09-26 多开改造）。
    ///
    /// 实例 id = 相对 `assets/lua` 的路径 stem（`HoverDot/solid`，见
    /// [`Self::hover_active_list`] 的播种与 [`prefixed_hover_id`]）；
    /// **多元素 = 多个悬浮标同时显示**（都跟随鼠标）。
    ///
    /// 语义用 [`Option`] 表达三态，避免"从默认值补齐"产生伪落盘（见 DESIGN 门禁）：
    /// - `None`（不落盘）：用户从没管过列表 —— 消费端按 [`Self::hover_type`] 播种
    ///   （老配置/新装机 = 单选语义，行为与旧版完全一致）；
    /// - `Some(vec)`：用户明确管过（勾了至少一个，或全清了）—— 播种永不复活。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hover_active: Option<Vec<String>>,
    /// ★ 悬浮标类型 id（**legacy 单选字段**，2026-09-26 起只作 `hover_active` 的播种源，
    /// 新代码不再写它）。值是裸 stem（`solid`）——播种时由 [`Self::hover_active_list`]
    /// 补 `HoverDot/` 前缀成实例 id。
    #[serde(default = "default_hover_type")]
    pub hover_type: String,
    /// ★ 每类型的参数表：`hover_params[type][key] = value`（由脚本 schema 定义键与默认值）。
    #[serde(default)]
    pub hover_params: HashMap<String, HashMap<String, Value>>,
    #[serde(default = "default_hover_image")]
    pub hover_image: String,
    /// ★ 点击声音**共享音量**（0..=100，2026-09-25 官复原职）：五键共用一格音量
    /// （面板一行滑条，`apply_to` 写五键 + 本字段保持同步），每键差异只剩
    /// 「响哪个文件」。1.0 本就是全局音量，每键音量是 2.0 中间版的过度细分。
    #[serde(default = "default_sound_volume")]
    pub sound_volume: u32,
    /// ★ 点击效果**总开关**（2026-09-25 单卡化）：「点击效果」卡标题栏选框，AND 闸——
    /// 总开关关 = 五键全不响；每键的启停走各自下拉的首项「关闭」。
    /// 缺字段 = true = 老配置行为不变（每键 enabled 照旧生效），零迁移。
    #[serde(default = "default_true")]
    pub ripple_enabled: bool,
    /// ★ 点击声音**总开关**（同 [`Self::ripple_enabled`]）。
    #[serde(default = "default_true")]
    pub sound_enabled: bool,
    #[serde(default = "default_ripple_size")]
    pub ripple_size: u32,
    #[serde(default = "default_ripple_duration")]
    pub ripple_duration: u32,
    /// ★ 每键波纹绑定（左/右/中/侧1/侧2 各自独立；旧 ripple_* 字段为迁移源，仅读不写）。
    #[serde(default = "default_ripple_bindings")]
    pub ripple_bindings: HashMap<String, ButtonRippleCfg>,
    /// ★ §40 一次性重置标记（2026-09-25）：波纹 `script_params` 曾被
    /// `migrate_clickfx` 用 legacy 颜色（#FFFF00）播种并落盘，毒值无法与真实调参
    /// 区分。首次加载新版本时清空全部波纹脚本参数（schema 默认接管），随后置位
    /// 不再重置。`false` 不落盘（skip_serializing_if），避免默认配置产生伪差异。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ripple_params_reset: bool,
    #[serde(default = "default_trail_effect")]
    pub trail_effect: String,
    #[serde(default)]
    pub trail_enabled: bool,
    #[serde(default)]
    pub trail_settings: TrailDetail,
    #[serde(default)]
    pub ripple_left_enabled: bool,
    #[serde(default)]
    pub ripple_right_enabled: bool,
    /// ★ 每键点击音效绑定（左/右/中/侧1/侧2 各自独立）。
    #[serde(default = "default_sound_bindings")]
    pub sound_bindings: HashMap<String, ButtonSoundCfg>,
    /// 旧版「按键声音」只有左右两键，且共用一个音量。**只读**：以下是 1.0 的迁移源，
    /// 新模型一律读写 `sound_bindings`（见 `ui::model::from_config`）。
    #[serde(default)]
    pub sound_left_enabled: bool,
    #[serde(default)]
    pub sound_right_enabled: bool,
    #[serde(default = "default_sound_file")]
    pub sound_left_file: String,
    #[serde(default = "default_sound_file")]
    pub sound_right_file: String,
}

impl TrailSettings {
    /// 启用的悬浮标脚本 id 列表（消费端统一入口）。
    ///
    /// `hover_active` 是三态（见字段注释）：`None` = 用户没管过 → 按旧单选字段
    /// [`Self::hover_type`] 即时播种（裸 stem 补 `HoverDot/` 前缀，**只算不落盘**，
    /// 与"读取侧做兼容迁移"同一纪律）；`Some` = 用户明确管过，原样返回。
    pub fn hover_active_list(&self) -> Vec<String> {
        match &self.hover_active {
            Some(v) => v.clone(),
            None => {
                if self.hover_type.is_empty() {
                    Vec::new()
                } else {
                    vec![prefixed_hover_id(&self.hover_type)]
                }
            }
        }
    }

    /// 某悬浮标脚本的参数表（`hover_params[id][key]`）。
    ///
    /// ★ 读取侧兼容：UI 下拉 id 是**裸 stem**（如 `solid`），实例 id 带目录前缀
    /// （`HoverDot/solid`）——先按带前缀键精确查，查不到退回裸 stem（切分按 `/`
    /// 字节，无 UTF-8 边界问题）。
    pub fn hover_params_for(&self, id: &str) -> HashMap<String, Value> {
        if let Some(m) = self.hover_params.get(id) {
            return m.clone();
        }
        let bare = id.rsplit('/').next().unwrap_or(id);
        self.hover_params.get(bare).cloned().unwrap_or_default()
    }
}

/// 悬浮标脚本 id 的目录前缀补齐：裸 stem（UI 下拉口径）补 `HoverDot/` 前缀；
/// 已带前缀（含 `/`）的 id 原样返回。
///
/// ★ 实例引擎根 = `assets/lua`（`hover_dot::HoverDotInstance::load`），id 自带目录
/// 前缀才能拼出 `assets/lua/HoverDot/<stem>.lua`；老配置只有裸 stem 且全部内置脚本
/// 都在 `HoverDot/`，所以补前缀是安全的。
fn prefixed_hover_id(stem: &str) -> String {
    if stem.contains('/') {
        stem.to_string()
    } else {
        format!("HoverDot/{stem}")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrailDetail {
    #[serde(default = "default_fade_duration")]
    pub fade_duration: f32,
    #[serde(default = "default_density")]
    pub density: u32,
    #[serde(default = "default_trail_opacity")]
    pub opacity: f32,
    #[serde(default = "default_spawn_interval")]
    pub spawn_interval_ms: u32,
    #[serde(default = "default_particle_size")]
    pub particle_size: u32,
    #[serde(default = "default_color_mode")]
    pub color_mode: String,
    #[serde(default = "default_fixed_color")]
    pub fixed_color: String,
    /// 命名空间参数表：每个 Lua 脚本特效的参数以 `{脚本id}.{参数key}` 为键存于此，
    /// 与上面 7 个内置共享字段互不干扰。脚本声明的参数经主窗口动态 UI 写入这里，
    /// 控制器再按 `{id}.` 前缀过滤还原成「该特效自己的参数表」喂给 Lua。
    #[serde(default)]
    pub extra: HashMap<String, Value>,
}

impl Default for TrailDetail {
    /// ★ 必须与 Python `DEFAULT_CONFIG["trail"]["trail_settings"]` 逐字段一致。
    /// 早先这里用 `#[derive(Default)]`，得到全 0 / 空串——与 Python 不符，且会让
    /// 控制面板的滑条（值域非 0 起）在首帧把配置"修正"成越界的下限值。
    fn default() -> Self {
        Self {
            fade_duration: 1.0,
            density: 5,
            opacity: 0.5,
            spawn_interval_ms: 100,
            particle_size: 5,
            color_mode: "rainbow".into(),
            fixed_color: "#FFFF00".into(),
            extra: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KeyDisplaySettings {
    /// ★ 本功能卡片的开关（原「全局设置 → 功能总开关 → 击键回显」，已下沉到功能页）。
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_placement")]
    pub placement: String,
    #[serde(default)]
    pub top_offset: i32,
    #[serde(default = "default_bottom_offset")]
    pub bottom_offset: i32,
    #[serde(default)]
    pub left_offset: i32,
    #[serde(default)]
    pub right_offset: i32,
    #[serde(default = "default_popup_width")]
    pub popup_width: u32,
    #[serde(default = "default_opacity")]
    pub opacity: f32,
    #[serde(default = "default_fade_delay")]
    pub fade_delay: u32,
    #[serde(default = "default_font_size")]
    pub font_size: u32,
    #[serde(default = "default_font_color")]
    pub font_color: [u8; 4],
    #[serde(default = "default_fun_color")]
    pub fun_color: [u8; 4],
    #[serde(default = "default_symbol_color")]
    pub symbol_color: [u8; 4],
    #[serde(default = "default_bg_color")]
    pub bg_color: [u8; 4],
    #[serde(default = "default_merge_interval")]
    pub merge_interval: f32,
    #[serde(default = "default_display_mode")]
    pub display_mode: String,
    // 「显示鼠标按键」的独立开关已删（2026-09-25 用户口径）：与模式
    // 下拉的「排除鼠标」重复。运行时由 `display_mode` 派生（只有「全部」回显鼠标），
    // 见 `key_display::settings::from_config`。旧配置里的 `show_mouse` 键被静默忽略
    // （本结构体未开 `deny_unknown_fields`）。
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DrawingSettings {
    /// ★ 本功能卡片的开关（原「全局设置 → 功能总开关 → 屏幕书写」，已下沉到功能页）。
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub hotkeys: DrawingHotkeys,
    #[serde(default = "default_pens")]
    pub pens: Vec<Pen>,
}

fn default_pens() -> Vec<Pen> {
    vec![
        Pen {
            color: "#000000FF".into(),
            width: 4,
            style: "solid".into(),
        },
        Pen {
            color: "#FF000040".into(),
            width: 30,
            style: "solid".into(),
        },
        Pen {
            color: "#00FF00FF".into(),
            width: 4,
            style: "dashed".into(),
        },
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DrawingHotkeys {
    #[serde(default = "default_hk_screen")]
    pub draw_hk_screen: String,
    #[serde(default = "default_hk_white")]
    pub draw_hk_white: String,
    #[serde(default = "default_hk_pen1")]
    pub draw_hk_pen1: String,
    #[serde(default = "default_hk_pen2")]
    pub draw_hk_pen2: String,
    #[serde(default = "default_hk_pen3")]
    pub draw_hk_pen3: String,
    // ── 以下各键的行序 = [`crate::core::hotkeys::DRAW_TOOL_ORDER`]（工具栏按钮序）──
    #[serde(default = "default_hk_line")]
    pub draw_hk_line: String,
    #[serde(default = "default_hk_arrow")]
    pub draw_hk_arrow: String,
    #[serde(default = "default_hk_rect")]
    pub draw_hk_rect: String,
    #[serde(default = "default_hk_ellipse")]
    pub draw_hk_ellipse: String,
    #[serde(default = "default_hk_text")]
    pub draw_hk_text: String,
    /// 序号贴纸（2026-10-01 新增）：切到「序号」工具，点一下盖一个带编号的圈。
    ///
    /// 出厂**留空**（空串 = 停用）。工具栏里这个动作本来只有图标按钮，而它是教学标注里
    /// 用得最多的一把键（写步骤编号），配了键位才划得来；出厂给键位反而会和 `0`~`9`
    /// 那排数字键抢位置，也让"出厂就带键位"的标注页多一条用户没要求的绑定。
    #[serde(default = "default_hk_stamp")]
    pub draw_hk_stamp: String,
    /// 马赛克：拖一个矩形把那块画面打码。出厂 `0`。
    #[serde(default = "default_hk_mosaic")]
    pub draw_hk_mosaic: String,
    /// 橡皮擦。出厂 `4`。
    ///
    /// ★ 字段顺序与工具栏按钮序、界面行序同源（`core::hotkeys::DRAW_TOOL_ORDER`）：
    ///   橡皮擦排在马赛克**之后**——按 4 键切橡皮，与工具栏上第四格是同一个位置。
    #[serde(default = "default_hk_eraser")]
    pub draw_hk_eraser: String,
}

impl Default for DrawingHotkeys {
    /// ★ 必须与 Python `DEFAULT_CONFIG["drawing"]["hotkeys"]` 逐字段一致。
    /// serde 的 `default = "fn"` 只作用于**反序列化**，`derive(Default)` 会让
    /// `Config::default()` 拿到 11 个空串——那样出厂状态下 Alt+G / 1..9 全是死键。
    /// （Python 甚至专门在启动时 `hks.setdefault(...)` 把默认值写进配置文件，就是为了
    /// 绕开同类问题；我们一次写对即可。）
    fn default() -> Self {
        Self {
            draw_hk_screen: default_hk_screen(),
            draw_hk_white: default_hk_white(),
            draw_hk_pen1: default_hk_pen1(),
            draw_hk_pen2: default_hk_pen2(),
            draw_hk_pen3: default_hk_pen3(),
            draw_hk_line: default_hk_line(),
            draw_hk_arrow: default_hk_arrow(),
            draw_hk_rect: default_hk_rect(),
            draw_hk_ellipse: default_hk_ellipse(),
            draw_hk_text: default_hk_text(),
            draw_hk_stamp: default_hk_stamp(),
            draw_hk_mosaic: default_hk_mosaic(),
            draw_hk_eraser: default_hk_eraser(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pen {
    #[serde(default = "default_pen_color")]
    pub color: String,
    #[serde(default = "default_pen_width")]
    pub width: u32,
    #[serde(default = "default_pen_style")]
    pub style: String,
}

/// 屏幕截图设置。
///
/// ★ 开关架构（2026-09-25 起两道闸）：卡标题栏的 `enabled` + 快捷键为空串——任一关上
/// 即不可触发。★ 2026-09-26 起**三把快捷键出厂全留空**（用户口径，与放大镜快捷键同款）：
/// 功能开关本来就出厂关闭，出厂键位只是摆设；想用哪个自己在面板里配。
///
/// ★ 2026-09-17 精简（用户口径）：截图**一定**进剪贴板、**一定**收进贴图环、贴图窗
/// **一定**描边、右键**一定**能另存为——这些都不再是开关。"上次选区截图""保存目录"
/// "保存后提示"三个功能整体删除：留档改由「贴图右键 → 另存为」按需触发，程序自身的
/// 临时产物统一落到 `temp/screenshot`（见 `core::paths::module_temp_dir`）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScreenshotSettings {
    /// 功能开关（面板「截图与贴图」卡标题栏的选框）。关 = 三把快捷键全部不响应。
    /// 2026-09-25 晚：出厂改未勾选（所有功能默认全关）。
    #[serde(default)]
    pub enabled: bool,
    /// 截图键（**UI 行名**「截图」，2026-09-27 去掉「快捷键」三字；区域选择 → 剪贴板 +
    /// 临时留档）。空串 = 停用。
    #[serde(default = "default_hk_screenshot")]
    pub hotkey: String,
    /// ★ 截图后贴图键（**UI 行名**「截图后贴图」；区域选择 → 剪贴板 + 钉到桌面）。空串 = 停用。
    #[serde(default = "default_hk_screenshot_pin")]
    pub hotkey_pin: String,
    /// ★ 贴图键（**UI 行名**「贴图」；剪贴板内容钉到桌面；文本以位图形式；多次按逆序回溯
    /// 贴图历史）。空串 = 停用。
    #[serde(default = "default_hk_pin_clipboard")]
    pub hotkey_pin_clipboard: String,
    /// ★ 长截图快捷键（**开始 / 结束**一键开关，同录屏）：框一块区域 → 滚动页面 →
    /// 再按一次结束（或页面停止滚动后自动收工）。产物进剪贴板 + `temp/screenshot`
    /// 留档。空串 = 停用。受 `enabled` 管辖（关掉「截图与贴图」卡，这把键一并失效）。
    #[serde(default = "default_hk_scroll")]
    pub hotkey_scroll: String,
    /// ★ 宽截图快捷键（2026-10-01）：与长截图同一套交互/拼接，方向**向右**——用户横向
    /// 滚动页面（宽表格、横向时间线），每帧转置后喂同一个拼接器。空串 = 停用。
    /// 受 `enabled` 管辖。
    #[serde(default = "default_hk_scroll_wide")]
    pub hotkey_scroll_wide: String,
    /// ★ 贴图历史环容量（"最多支持 pin 最近的 N 个内容"）。默认 10。
    ///
    /// 这个值同时约束剪贴板监听的内存上界：轮询只是**按需收新内容**，环本身裁剪到
    /// 本容量，所以常驻内存最坏就是 N 张位图（见 `Runtime::poll_clipboard`）。
    #[serde(default = "default_pin_ring_cap")]
    pub pin_ring_cap: usize,
}

/// 屏幕录制设置。
///
/// ★ 2026-09-19（用户口径）：录制快捷键改**一键开关** —— 按一下开始（3 秒倒计时后录），
/// 再按一下停止并落盘。原「停止快捷键」（`stop_hotkey`）字段**直接删除**：两把键要多占一个
/// 全局快捷键，"按 A 却停了 B"本身就是歧义源。本结构体没开 `deny_unknown_fields`，
/// 旧配置里那行会被静默忽略，所以不需要迁移代码（GIF 同理，见 [`GifSettings`]）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecordSettings {
    /// 功能开关（面板「视频录制」卡标题栏的选框）。关 = 录屏快捷键不响应。
    /// 2026-09-25 晚：出厂改未勾选（所有功能默认全关）。
    #[serde(default)]
    pub enabled: bool,
    /// 录屏快捷键（**开始 / 停止**一键开关）。空串 = 停用。
    #[serde(default = "default_hk_record")]
    pub hotkey: String,
    /// 选区录屏快捷键：复用 GIF 选框，框好后直接进 3 秒倒计时录这块区域（MP4）。
    /// 空串 = 停用。与 `hotkey`（整屏录）并存，两把独立、互不影响。
    #[serde(default = "default_hk_record_region")]
    pub region_hotkey: String,
    /// MP4 时间裁剪快捷键：打开最近一次录像的裁剪窗（区间导出）。空串 = 停用。
    #[serde(default = "default_hk_record_trim")]
    pub trim_hotkey: String,
    /// 帧率。档位见 [`RECORD_FPS_LABELS`]（15 / 30 / 60），出厂 30。
    #[serde(default = "default_fps")]
    pub fps: u32,
    /// 分辨率档下标，指向 [`RECORD_RESOLUTION_LONG_EDGE`]：
    /// 0=原始 1=720p 2=1080p 3=1440p 4=2160p（清晰度升序，第一个仍是「原始」）。
    /// 老配置（旧顺序 `原始/1080p/720p/...`）的下标由 [`Config::migrate_resolution_order`]
    /// 一次性重映射，避免静默变义。
    #[serde(default)]
    pub resolution: u32,
    /// 硬件编码策略，指向 [`RECORD_HARDWARE_LABELS`]：0=自动 1=关闭。
    #[serde(default)]
    pub hardware: u32,
    /// **码率**档，取值见 [`RECORD_BITRATE_VALUES`]：`"auto"` 或 kbps 数字串
    /// （`"4000"` / `"8000"` / …）。用哪个由 [`Self::bitrate_kbps`] 收敛。
    ///
    /// 字段名原本是 `quality`（旧显示名「画质」，容易被读成"也管清晰度"）；`alias`
    /// 让存量配置原样读出，不需要迁移脚本。
    ///
    /// ★★ 类型从 `u32`（档位下标）改成 `String`（自描述取值）是有意的：档位表要加
    ///   高档位，而"下标"的语义一旦变化，老配置的 `bitrate: 1` 会从 8 Mbps **静默**
    ///   变成 4 Mbps —— 用户只会觉得"画质怎么突然糊了"，几乎不可能自己查出来。
    ///   取值写进字符串后，加档位不影响老配置；老的**数字下标**仍能读入，见 [`de_bitrate`]。
    #[serde(
        default = "default_bitrate",
        deserialize_with = "de_bitrate",
        alias = "quality"
    )]
    pub bitrate: String,
    /// 是否把鼠标光标画进画面（DXGI 桌面复制本身不含光标）。
    #[serde(default = "default_true")]
    pub show_cursor: bool,
    /// 录完弹桌面提示（右下角，含「打开文件夹」入口；用户手动关闭）。
    // ★ 此字段已废弃（用户口径：录制完总是需要提示），保留 serde 别名兼容旧配置。
    #[serde(default = "default_true", skip_serializing)]
    #[allow(dead_code)]
    pub notify: bool,
    /// 是否采集麦克风声音。★ 与 [`Self::mic_device`] 合起来表示**三态来源**
    /// （关闭 / 跟随默认 / 指定设备），读写请走 [`Self::mic_source`] / [`Self::set_mic_source`]。
    ///
    /// ★ 2026-09-26 晚用户口径：出厂 **开**（配合空设备名 = 「跟随系统默认」），
    ///   不再是 `false`（关闭）。理由是"录屏带麦克风"是绝大多数录屏的默认预期，
    ///   出厂关闭会让第一次录出来的视频没有声音，而这类失败不可逆（录完了才发现）。
    #[serde(default = "default_true")]
    pub record_mic: bool,
    /// 是否采集系统声音（扬声器 / 桌面音频）。同一套三态，见 [`Self::speaker_source`]。
    ///
    /// ★ 与 [`Self::record_mic`] 同一晚改为出厂 **开**（= 「跟随系统默认」）。
    #[serde(default = "default_true")]
    pub record_speaker: bool,
    /// 麦克风降噪档位（`off` / `weak` / `medium` / `strong`）。
    ///
    /// 单一下拉：关闭 / 弱 / 中 / 强。底层是 **RNNoise 神经网络降噪**
    /// （Xiph RNNoise 算法的纯 Rust 移植，见 `third_party/nnnoiseless`，零 C 依赖）；
    /// `weak`/`medium`/`strong` 对应不同的「干湿混合比」（50% / 80% / 100%，
    /// 见 `audio_capture::DenoiseStrength`）—— 同一份模型，只改降噪后信号混入多少。
    /// ★ 2026-09-26 晚：出厂改**中**（历史值 `off`），见 [`default_mic_denoise`]。
    ///
    /// ★ 遗留取值 `gate` / `spectral` / `rnnoise` 由 [`Self::migrate_denoise`] 规整为
    /// `medium` —— 不再有独立的「噪声门」档，RNNoise 已覆盖其场景。
    #[serde(default = "default_mic_denoise")]
    pub mic_denoise: String,
    /// 指定麦克风设备（友好名；空串 = 跟随系统默认）。来源列表由
    /// `audio_capture::list_mic_devices` 提供。**只在 `record_mic` 为真时有意义**。
    #[serde(default)]
    pub mic_device: String,
    /// 指定系统声音设备（友好名；空串 = 跟随系统默认）。loopback 采集的来源；
    /// 列表由 `audio_capture::list_speaker_devices` 提供。
    #[serde(default)]
    pub speaker_device: String,
}

/// 音频来源的**三态**：`None` = 关闭；`Some("")` = 跟随系统默认；`Some(名)` = 指定设备。
///
/// ★ 为什么存储层仍是「开关 + 设备名」两个字段、而不是改成 `Option<String>`：
/// 两字段方案对**存量配置零迁移**（老 JSON 的 `record_mic: true` + `mic_device: "耳机"`
/// 天然就是"指定设备"），换成 `Option<String>` 得写自定义反序列化才能兼容旧键，
/// 而旧键一旦读错就是"录音突然没声"这种最难查的故障。
/// 三态只在这四个方法里收敛成一个概念 —— 面板与运行时都只跟 `Option<&str>` 打交道，
/// 不会再出现"开关说关、来源却指着一个设备"这种两处口径打架。
impl RecordSettings {
    /// 麦克风来源三态。
    pub fn mic_source(&self) -> Option<&str> {
        if self.record_mic {
            Some(self.mic_device.as_str())
        } else {
            None
        }
    }
    /// 写麦克风来源三态。`Some("")` = 跟随系统默认。
    pub fn set_mic_source(&mut self, src: Option<&str>) {
        match src {
            None => self.record_mic = false,
            Some(name) => {
                self.record_mic = true;
                self.mic_device = name.to_string();
            }
        }
    }
    /// 系统声音来源三态。
    pub fn speaker_source(&self) -> Option<&str> {
        if self.record_speaker {
            Some(self.speaker_device.as_str())
        } else {
            None
        }
    }
    /// 写系统声音来源三态。`Some("")` = 跟随系统默认。
    pub fn set_speaker_source(&mut self, src: Option<&str>) {
        match src {
            None => self.record_speaker = false,
            Some(name) => {
                self.record_speaker = true;
                self.speaker_device = name.to_string();
            }
        }
    }

    /// 本次录制的目标码率（kbps）；选「自动」返回 `None`。
    ///
    /// ★ 为什么"自动"不能在这里算成数字、要交给录屏器：自动档要按**最终输出尺寸**
    ///   定（原始分辨率在不同屏幕上差好几倍），而输出尺寸要等桌面复制会话建好、
    ///   拿到 `DXGI_OUTDUPL_DESC.ModeDesc` 之后才知道（见 `recorder::Session::open`）。
    ///   在这里算只能拿到"原始分辨率"这个未知量，等于瞎猜。
    ///
    /// 固定档的取值被手改成无法解析的内容时也返回 `None`（回落自动）—— 宁可给出一个
    /// 按分辨率算出来的合理数字，也不要回落成 0（那会让编码器按自己的默认值编，
    /// 用户看到的码率与面板显示的完全无关）。
    pub fn bitrate_kbps(&self) -> Option<u32> {
        match self.bitrate.as_str() {
            "auto" => None,
            other => other.parse().ok(),
        }
    }
}

/// 虚拟键盘设置。
///
/// 与击键回显并列：击键回显是"打出什么字"，虚拟键盘是"按的是哪个键位"。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VirtualKeyboardSettings {
    /// 本功能卡片的开关（唯一开关，在「虚拟键盘」父卡标题行）。
    #[serde(default)]
    pub enabled: bool,
    /// 方案 id = `assets/VirtualKeyboards/<id>.json` 的文件名（去掉扩展名）。
    ///
    /// 出厂三套（`ansi_104` / `ansi_60` / `ansi_104_dark`）内嵌在二进制里兜底；用户往
    /// 方案目录里丢一个 JSON 文件就能在下拉里看到并使用，不需要改配置也不需要重编译。
    #[serde(default = "default_vk_layout")]
    pub layout: String,
    #[serde(default = "default_placement")]
    pub placement: String,
    /// 四向偏移（屏幕宽/高的**百分比** 0–50）。
    ///
    /// ★ 2026-09-26 起是 `f32`（原 `i32`）：用户把水平偏移定在 **0.5%**，整数存不下，
    ///   面板滑条步长同步改成 0.5、显示一位小数。老配置里写的是整数（`5`），
    ///   serde 从 JSON 整数读 `f32` 是合法的，无需迁移。
    #[serde(default)]
    pub top_offset: f32,
    #[serde(default = "default_vk_bottom_offset")]
    pub bottom_offset: f32,
    #[serde(default)]
    pub left_offset: f32,
    #[serde(default = "default_vk_right_offset")]
    pub right_offset: f32,
    /// 一个标准键（1u）的边长（逻辑像素）。整块键盘由它等比放大缩小。
    #[serde(default = "default_vk_key_size")]
    pub key_size: u32,
    /// 键间空隙（逻辑像素）。
    #[serde(default = "default_vk_gap")]
    pub gap: u32,
    /// 整块键盘的总不透明度（容器级乘子）。
    ///
    /// 与「键帽色/按下色/描边色/文字色」各自的 alpha 是**相乘**关系：
    /// `最终 alpha = 总不透明度 × 元素 alpha`（与击键回显同构）。见
    /// `temp/notes-archive/虚拟键盘问题分析与改进方案-20260917.md` §2（历史归档）。
    #[serde(default = "default_opacity")]
    pub opacity: f32,
    /// 按下后高亮保持的时长（秒）。范围 0.1~2.0，出厂 0.1。
    #[serde(default = "default_vk_hold")]
    pub hold: f32,
    /// 键帽图例字号（逻辑像素）。范围 6~28，出厂 14。
    #[serde(default = "default_vk_font_size")]
    pub font_size: u32,
    /// 键帽底色。
    #[serde(default = "default_vk_key_color")]
    pub key_color: [u8; 4],
    /// 按下时的键帽底色。
    #[serde(default = "default_vk_press_color")]
    pub press_color: [u8; 4],
    /// 键帽描边色。
    #[serde(default = "default_vk_border_color")]
    pub border_color: [u8; 4],
    /// 图例文字色。
    #[serde(default = "default_font_color")]
    pub text_color: [u8; 4],
    // ── 2.x 新增字段 ──
    /// 键帽圆角半径（逻辑像素）。`0` = **直角**。
    ///
    /// 优先级：面板值偏离出厂默认(4)时以面板为准；恰好等于出厂默认时让方案 JSON 的
    /// `defaults.corner_radius` 兜底（"看起来没被用户动过" → 允许方案设定观感）。
    #[serde(default = "default_vk_corner_radius")]
    pub corner_radius: u32,
    /// 鼠标穿透（`WS_EX_TRANSPARENT`）。勾选 = 纯展示：点击直接落到下面的窗口。
    ///
    /// 不勾 = 可点击：鼠标落在键帽范围内时**由我们吃掉**（下层窗口收不到、前台焦点不变），
    /// 命中哪颗键就注入哪颗键；落在键盘以外的地方照常穿透。
    ///
    /// ★ 出厂 **true**（2026-09-18 用户要求三个窗口行为都默认开启）：104 键铺开要占
    ///   近千像素宽，默认把这么大一块的鼠标事件吃掉太具侵入性。要"点键盘输入"就把这一项
    ///   取消勾选 —— 那个能力本身一直在，只是默认不接管鼠标。
    #[serde(default = "default_true")]
    pub click_through: bool,
    /// 方案 JSON 自己的缺省缩放倍率（面板不暴露，只由 `defaults.scale` 提供）。
    ///
    /// 字段保留是为了让"方案自带倍率"这条通路还在；面板上的「缩放」项已删除
    /// —— 它与「键帽大小」是同一个乘子（都只缩放 `unit`），功能完全重叠。
    #[serde(default = "default_vk_scale")]
    pub scale: f32,
    /// ★ **方案控件的用户取值**：`方案 id → 控件 id → 值`。
    ///
    /// 虚拟键盘「窗口」「方案配置」两张父卡下的方案控件是**方案 JSON 声明出来的**
    /// （没声明就由代码补齐一套预设，见 `virtual_keyboard::vk_config::controls_of`），
    /// 所以用户调过的值按"当前方案 + 控件 id"记 —— 详见 `virtual_keyboard` 模块说明，
    /// 配置只保留**当前所选方案**的一整套控件值（切换方案即载入该方案默认、清掉旧设定）。
    /// 这表只存"用户改过的那部分"：没动的项不写（值与方案默认相同就删键），
    /// 于是配置文件里不会出现一整屏的默认值。
    #[serde(default)]
    pub params: BTreeMap<String, BTreeMap<String, ParamValue>>,
}

/// 一个方案控件的值。
///
/// 只有三种形态，与控件类型一一对应（`range` → `Float`，`color` → `Color`，
/// `bool` → `Bool`）。`untagged` 是为了让配置文件可读：`14.0` / `"#1F6FEB"` / `true`
/// 而不是包一层 `{"t":..}`。顺序按"最不易误判"排：字符串色值不会落进 Float。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ParamValue {
    Bool(bool),
    Float(f32),
    /// `#RRGGBB` 或 `#RRGGBBAA`。解析失败时该值被忽略（退回方案默认）。
    Color(String),
}

impl ParamValue {
    /// 取浮点值；类型不符时 `None`（调用方退回默认）。
    pub fn as_f32(&self) -> Option<f32> {
        match self {
            ParamValue::Float(v) => Some(*v),
            _ => None,
        }
    }

    /// 取颜色；类型不符或色值写坏时 `None`。
    pub fn as_argb(&self) -> Option<crate::core::color::Argb> {
        match self {
            ParamValue::Color(s) => crate::core::color::Argb::parse_hex(s),
            _ => None,
        }
    }

    /// 取布尔值；类型不符时 `None`。
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            ParamValue::Bool(v) => Some(*v),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct GeneralSettings {
    /// 工作文件夹自动清理阈值（**天**；0 = 永不，出厂默认）。
    ///
    /// 每次程序启动执行一次：递归检查 `<exe>/temp`，**修改时间早于 N 天前**的文件
    /// 直接删除，随后自底向上清掉因此变空的子目录（根目录不删）。0/7/30/180 四档
    /// 由设置页「工作文件夹 → 自动清理」下拉选择（`ui::model::TEMP_CLEANUP_DAYS`）。
    #[serde(default)]
    pub temp_cleanup_days: u32,
    /// ★ 管理员权限启动（用户拍板 2026-10-07）：勾选后**下一次启动**走 UAC 提权，
    /// 由设置页「系统」卡的勾选项控制（`main.rs` 启动分支读它）。
    ///
    /// 出厂**不勾** = 普通权限：启动链路单进程干净、无 UAC 弹窗；代价 = 提权窗口
    /// （任务管理器等）前台时低级钩子收不到输入，拖尾/击键回显等特效在其上冻结
    /// （1.0.0 启动即提权的原始动机，实测复现过）。勾选与否由用户自己权衡。
    /// 取消勾选后**无需任何中转**：用户手动退出再双击启动，经过 explorer 权限
    /// 自然回到普通（普通重启反而会继承管理员权限——这是绕开它的原因）。
    #[serde(default)]
    pub admin_launch: bool,
}

/// 面板上**可折叠卡片**的 id 全集——展开态要落盘的就是这些（见 [`UiSettings::expanded`]）。
///
/// 命名规则：顶层卡用裸名（`trail`），子卡用 `<父卡>.<键>`（`key_display.pos`）。
/// ★ 2026-09-25 晚：点击效果改回**父卡 + 五键子卡**（子卡标题栏 = 键名 +「关闭/效果」
/// 下拉，参数在子卡正文——平铺行的参数段没有视觉分隔，用户拍板回子卡），5 个
/// `ripple.<键>` id 回归；声音卡没有每键参数，保持平铺单卡，`sound.<键>` 仍退役。
/// 老配置里残留的展开键（含 `sound.<键>`）被 serde 原样保留，`ripple.<键>` 直接复活。
pub const CARD_IDS: [&str; 43] = [
    // 鼠标页（拖尾 / 悬浮标 / 点击效果 / 点击声音；分类判据 = 操作对象是鼠标，
    // 见 `ui::nav` 头注释。★ 卡片搬家不换 id：id 只管展开态落盘，与所属页无关）
    "trail",
    "hover",
    // 点击效果：父卡 + 五键子卡（子卡 id 就是 BUTTON_KEYS 拼出来的 `ripple.<键>`，
    // 由 `card_ids_follow_the_naming_rule` 锁住）。父卡自己占一个 id 的理由（2026-09-22）
    // 不变：缺 id 时 `m.expand("ripple")` 只能拿到兜底信号，展开态读不到也写不进。
    "ripple",
    "ripple.left",
    "ripple.right",
    "ripple.middle",
    "ripple.x1",
    "ripple.x2",
    // 点击声音：平铺单卡（每键只有一行下拉，没有子卡）。
    "sound",
    // 键盘页 · 击键回显（父卡 + 三张子卡）
    "key_display",
    "key_display.pos",
    "key_display.display",
    "key_display.color",
    // 屏幕页 · 屏幕标注（「画笔 / 快捷键」两张子卡挂在同一父卡下）
    "drawing.pen",
    "drawing.pensub",
    "drawing.hotkey",
    // 工具页 · 便签（原屏幕页；2026-09-25 重排，id 不变）
    "sticky",
    "sticky.hotkey",
    // 工具页 · 剪贴板（独立原生模块）
    "clipboard",
    // 屏幕页 · 抓取录制三卡：截图与贴图 / GIF录制 / 视频录制（原「录制」页；`record.` 前缀保留，id 与页无关）
    "record.shot",
    "record.gif",
    "record.video",
    // 视频录制（父卡）下的三张子卡：快捷键 / 画面 / 声音
    "record.video.hotkey",
    "record.video.image",
    "record.video.audio",
    // 键盘页 · 虚拟键盘（父卡 + 四张子卡）
    "virtual_keyboard",
    "virtual_keyboard.pos",
    "virtual_keyboard.window",
    "virtual_keyboard.look",
    "virtual_keyboard.color",
    // 设置页（id 前缀 global. 为历史遗留；global.timer 计时器卡现挂工具页，id 保留）
    // ★ 2026-10-07：「功能开关」卡（原「模块开关」+「脚本开关」两张卡合并，正文分两段）
    //   与「快捷键」卡（原「快捷键」+「脚本快捷键」合并）**各只留一个展开态**——
    //   `global.script_switches` / `global.script_hotkeys` 随之作废（老配置里残留的键
    //   由 serde 原样保留，不影响读取；见 `global_page_cards_are_merged`）。
    "global.hotkey",
    "global.switches",
    // 设置页 · 「高级」卡（m69，2026-10-31）：下载源 + 图层覆盖 + 工作文件夹 + 系统
    // 合并成一张杂项卡。原 `global.overlay` / `global.temp` / `global.system` 三个
    // 展开态 id 随合并作废（老配置残留键由 serde 原样保留）。
    "global.advanced",
    // 工具页 · 计时器卡（`global.` 前缀为历史遗留，卡片现挂工具页，id 保留）
    "global.timer",
    // 脚本页 · 官方资源下载卡（m37 建卡，m69 从设置页搬到脚本页**第一张**；卡片搬家
    // 不换 id，展开态键沿用 `global.store`）
    "global.store",
    // 设置页 · 帮助卡（2026-09-29）：随包文档（快捷键手册 / Lua 指南 / Lua API）的打开入口
    "global.help",
    // 屏幕页 · 放大镜（原「其它」页；`other.` 前缀保留，id 与页无关）
    "other.magnifier.pip",
    "other.magnifier.pip.hotkey",
    "other.magnifier.pip.pos",
    "other.magnifier.pip.display",
    "other.magnifier.full",
    // 键盘页（击键音效）
    "key_sound",
    // 脚本页（原 Lua 页）
    "lua_ext",
];

/// 卡片的**出厂展开态**：全部折叠（2026-09-25 晚用户定案——无论父卡还是子卡，
/// 出厂一律收起，打开面板只看到一列标题，想要什么点开什么）。
///
/// 这张表是"展开态不落盘也能正常用"的依据——`UiSettings::expanded` 里没有的键
/// 一律取这里的值，所以用户没动过的卡不会被面板首帧当成"改动"写盘
/// （对照 `ui::model::model_no_spurious_change`）。
///
/// ★ 历史口径（每页第一张展开 / 键盘页两层展开等）整体作废；函数保留签名是因为
///   `CARD_IDS` 全集遍历与落盘判据都挂在它上面，折叠只是把"默认值"统一成 `false`。
#[inline]
pub fn default_card_expanded(_id: &str) -> bool {
    false
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UiSettings {
    #[serde(default = "default_font_size")]
    pub font_size: u32,
    /// ★ 面板卡片的展开/折叠态（纯界面状态，但一样要落盘：下次开面板要还原现场）。
    ///
    /// 键 = [`CARD_IDS`]；**只记录与出厂默认不同的项**——回到默认就把键删掉。
    /// 于是"没动过任何卡片"时这个 map 是空的，配置文件不会被塞进一堆默认值，
    /// 也不会因为补默认值而在首帧误报改动（这正是早先 `script: ""` 踩过的坑）。
    ///
    /// 用 `BTreeMap` 而非 `HashMap`：序列化顺序稳定，配置文件可读、diff 不抖。
    #[serde(default)]
    pub expanded: BTreeMap<String, bool>,
    /// ★ 面板最后所在的导航页（下标，见 `ui::nav` 的索引常量）。
    ///
    /// 「打开主窗口要还原到最后一次的界面」的记忆项之一：面板每次打开都是一套全新
    /// 信号，页码必须落在配置里才能跨开关/跨进程存活（修复：重开面板总是回到「鼠标」页）。
    ///
    /// ★★ 2026-09-25 起这个字段**降级为兼容层**：下标会随页序重排漂移（8 页 → 7 页时，
    /// 旧 3「录制」该落屏幕页、旧 7「Lua」该落脚本页），所以真正的稳定键是
    /// [`UiSettings::panel_page_id`]。读：id 优先、没有才按映射表翻旧下标；
    /// 写：两份都写，老逻辑/外部工具读到下标也不会越界。
    #[serde(default)]
    pub panel_page: usize,
    /// ★ 面板最后所在的导航页**id**（`ui::nav::NAV_IDS`，如 `"screen"`）。稳定键：
    /// 页加减、页序调整都不影响它的含义。`None` = 8 页时代的老配置（或手改缺失），
    /// 此时按 [`ui::nav::legacy_page_index`] 把 [`UiSettings::panel_page`] 翻成新下标。
    #[serde(default)]
    pub panel_page_id: Option<String>,
    /// ★ 面板窗口最后的位置（左上角，物理屏幕像素，含副屏负坐标）。`None` = 从未记录
    /// （首次打开走居中）。面板固定 1000×660 逻辑像素（2.0 设计尺寸、不可缩放），
    /// 几何里只有位置是自由变量，因此只记位置不记尺寸。
    ///
    /// 在面板关闭时经 `platform::win_util::window_pos_by_title` 采集（`EventCtx` 没有
    /// 「读窗口屏幕坐标」的原语，Win32 `GetWindowRect` 是唯一通路）。
    #[serde(default)]
    pub panel_pos: Option<(i32, i32)>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GlobalSettings {
    /// 打开控制面板的全局快捷键。空串 = 停用（Python 语义）。
    #[serde(default = "default_panel_hotkey")]
    pub panel_hotkey: String,
    /// 计时器（正计时 / 倒计时）。标题栏开关 + 快捷键为空串两道闸。
    #[serde(default)]
    pub timer: TimerSettings,
}

impl Default for GlobalSettings {
    /// ★ `panel_hotkey` 出厂默认 `Ctrl+Shift+D`——**与 Python 不同**（Python 的
    /// `DEFAULT_CONFIG` 是空串，于是它的"面板快捷键"功能实际上从不生效）。
    /// plan3 §1.3 决定把这个死字段落成真功能，默认就给一个可用的组合。
    fn default() -> Self {
        Self {
            panel_hotkey: default_panel_hotkey(),
            timer: TimerSettings::default(),
        }
    }
}

/// 覆盖层全局置顶策略（对**所有**屏幕工具生效，勾选一次全体作用）。
///
/// ★ 这是「统一层级模型」的**轴 A·窗口置顶策略**：管的是"我们的画面窗口在系统 z 序里
/// 落在哪"，与另两条轴正交——「轴 B·同窗绘制层」（`ctx:layer` base/top，单窗内部）
/// 和「轴 C·窗间 rank」（我们自己窗口之间的先后）都不受这里影响。
///
/// 只有 **2 个选项**，彼此独立。用户口径 2026-09-20：「总在最上」默认**开**、「全屏避让」
///   默认**关**（这是屏幕行为工具，默认盖住全屏以显示特效；介意全屏被盖就勾上避让）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OverlaySettings {
    /// 总在最上：被**其它置顶应用**压住时，夺回置顶带顶部（普通窗口之上仍由
    /// `WS_EX_TOPMOST` 保证）。关掉 = "普通置顶"（不与别的置顶窗争抢）。
    ///
    /// 出厂 true：有它才看得见特效——常驻置顶 helper / Quicker 面板 / 任务管理器置顶
    /// 刷新都会把覆盖层压住。
    #[serde(default = "default_true")]
    pub always_on_top: bool,
    /// 全屏避让：前台是**全屏应用/游戏**时覆盖层让位（不压全屏）。
    ///
    /// ★ 出厂**取消勾选**（用户口径 2026-09-20）：本工具是屏幕行为，使命就是"在你看的任何
    ///   画面里呈现键鼠效果"，默认盖住全屏以显示特效；全屏游戏/视频介意时可勾上让位。
    /// ⚠️ 覆盖层承载鼠标拖尾 / 击键回显 / 虚拟键盘，压在全屏上时它们**一起**留上面。
    #[serde(default)]
    pub avoid_fullscreen: bool,
}

impl Default for OverlaySettings {
    fn default() -> Self {
        Self {
            // 与 `TopmostKeeper::new()` 的出厂值同源（两处都是"首帧之前的行为"）。
            always_on_top: true,
            // 出厂取消全屏避让（屏幕工具默认盖住全屏以显示特效）。
            avoid_fullscreen: false,
        }
    }
}

/// 计时器设置（`docs/DESIGN.md` §1 #15）。核心只是一个状态机：快捷键按下即开始，再按一下即停。
///
/// ★ 开关架构（2026-09-25 起）：卡标题栏 `enabled` + 快捷键为空串，两道闸任一关上即停用。
/// （2026-09-20 旧口径"没有总开关"已被推翻——用户要求五张无开关卡统一补上选框。）
/// 计时器「时长」滑条的量程（秒）。
pub const TIMER_DURATION_MIN: u64 = 1;
pub const TIMER_DURATION_MAX: u64 = 3600;
/// 计时器「字号」滑条的量程（pt）。横幅底框随文字量得，字号即整体大小。
pub const TIMER_FONT_MIN: u32 = 24;
pub const TIMER_FONT_MAX: u32 = 160;
/// 计时器「偏移」滑条的量程（屏幕宽/高的百分比）。与画中画 `offset_x/y` 同量纲同量程
/// （见 `MagnifierPipSettings`）：贴边锚点把横幅从边上往屏内挪，0–50 + 横幅自身宽度
/// 保证不越出屏幕。
pub const TIMER_OFFSET_MIN: u32 = 0;
pub const TIMER_OFFSET_MAX: u32 = 50;
//
// ★ 上面这两对常量是**面板滑条、钳制、换算三处的唯一来源**：滑块内部恒存 `0..=1`
//   （见 `ui::model` 顶部的数值映射助手），真实秒数要经 `v2s/s2v` 换算。量程各写一份
//   时，改一处漏一处不会报错，只会让"拖滑条"与"实际生效值"悄悄错位。

/// 一层"勾了开关但快捷键是空的"死态 —— 用户勾上却按不动，正是这个功能最初的病。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TimerSettings {
    /// 功能开关（面板「计时器」卡标题栏的选框）。关 = 计时器快捷键不响应。
    /// 2026-09-25 晚：出厂改未勾选（所有功能默认全关）。
    #[serde(default)]
    pub enabled: bool,
    /// **快捷键**（开始 / 停止一键开关；空串 = 停用）。
    ///
    /// ★ 2026-09-27 用户口径：行名从「启停快捷键」简化为「快捷键」，且**出厂留空**（沿用
    ///   §55「所有快捷键出厂留空」）—— 不占用户全局键位，空框就是"停用且看得见"。
    #[serde(default = "default_hk_timer")]
    pub hotkey: String,
    /// 模式：`countup` 正计时 / `countdown` 倒计时。
    ///
    /// ★ 番茄钟（`pomodoro` + `break_sec`）已于 2026-09-25 应用户要求整体删除——
    ///   老配置里的这两个残留键由 serde 静默忽略（本结构体未开 `deny_unknown_fields`，
    ///   同 `MagnifierPipSettings` 删缩放快捷键的先例）；`mode` 残留 `"pomodoro"` 的在
    ///   面板侧落回正计时档、运行时侧按未知模式不动作，打开面板选一次即写回。
    #[serde(default = "default_timer_mode")]
    pub mode: String,
    /// 倒计时时长（秒）。正计时没有终点，不消费此字段。
    #[serde(default = "default_timer_duration")]
    pub duration_sec: u64,
    /// 横幅锚点：0=中央 / 1=**顶部居中（默认）** / 2=底部居中 / 3=左上 / 4=右上 /
    /// 5=左下 / 6=右下。
    ///
    /// 与画中画 `MagnifierPipSettings::anchor` 同思路的**贴边枚举**（u32 下标，面板下拉
    /// 按同序出选项）。★ 2026-09-27 用户口径：出厂默认 **1（顶部居中）**——读秒挂在
    /// 屏幕顶部既不挡视线也扫一眼就看见；`offset_x/y` 出厂都是 0，顶部居中时它们不
    /// 生效（贴边锚点才有"往上/下挪"的语义）。
    #[serde(default = "default_timer_anchor")]
    pub anchor: u32,
    /// 水平偏移（屏幕宽度的百分比 0–50）：贴左/右边的锚点把横幅从边上往屏内挪。
    /// 中央没有"贴边"可言，此偏移不生效（与画中画"鼠标跟随时偏移不生效"同一语义）。
    #[serde(default)]
    pub offset_x: u8,
    /// 垂直偏移（屏幕高度的百分比 0–50）：贴上/下边的锚点把横幅从边上往屏内挪——
    /// 贴下边抬高即可让开任务栏。中央不生效。
    #[serde(default)]
    pub offset_y: u8,
    /// 读秒字号（pt，24–160）。底框宽高由文字量得 + 内边距随字号走，**字号即整体大小**。
    #[serde(default = "default_timer_font")]
    pub font_pt: u32,
}

impl Default for TimerSettings {
    /// ★ 必须与各字段的 `#[serde(default = ...)]` **逐字一致**。
    ///
    /// 面板的"未操作不改配置"回归门禁（`ui::model::tests::*`）要求 `Config::default()`
    /// 是 `from_config → apply_to` 的**固定点**：派生 `Default` 会给出全 0 / 空串
    /// （`mode=""`、`duration_sec=0`），而控件把它们解析成档位 0 / 1 秒，回写时与默认
    /// 对不上 → "只是打开一次面板就把配置重写一遍"，且每开一次再写一次。这是 `TrailDetail`
    /// 踩过的同一个坑（见那里的注释）。
    fn default() -> Self {
        Self {
            enabled: false,
            hotkey: default_hk_timer(),
            mode: default_timer_mode(),
            duration_sec: default_timer_duration(),
            anchor: default_timer_anchor(),
            offset_x: 0,
            offset_y: 0,
            font_pt: default_timer_font(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct HotkeySettings {
    /// Python 遗留的顶层冗余键。Python 1.0.0 从不读它，我们已改用
    /// `global.panel_hotkey`；保留字段只为老配置 round-trip 不丢值。
    #[serde(default = "default_toggle_drawing")]
    pub toggle_drawing: String,
}

/// 便签字号量程（逻辑像素）：Ctrl+滚轮在此范围内缩放，也给面板滑条/钳制/换算单一来源。
pub const NOTE_FONT_MIN: f32 = 9.0;
pub const NOTE_FONT_MAX: f32 = 72.0;
/// 便签窗口透明度量程（百分比）：标题栏 Ctrl+滚轮在 10–100 之间调整。
pub const NOTE_ALPHA_MIN: u32 = 10;
pub const NOTE_ALPHA_MAX: u32 = 100;

/// 便签（`DOCS/DESIGN` 屏幕工具族；UI 卡片在「工具」页，2026-09-25 重排）。
///
/// ★ 总开关 = [`NoteSettings::enabled`]（面板「便签」卡标题栏选框）：关 = 快捷键不响应；
/// 快捷键为空串同样停用（用户口径 2026-09-27：卡片上这行标签就叫「快捷键」，不带"呼出"）。
/// 一条便签即一个**原生 Win32 顶层窗**（参考 WeNote 的"无边框窗 + 原生文本控件"），
/// 多便签并存；标题栏左侧 `+` 新增、右侧 `置顶/锁定/收起/关闭`。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NoteSettings {
    /// 功能开关（面板「便签」卡标题栏的选框）。关 = 快捷键不响应。
    /// 2026-09-25 晚：出厂改未勾选（所有功能默认全关）。
    #[serde(default)]
    pub enabled: bool,
    /// 呼出便签的快捷键（空串 = 停用；面板这行标签就叫「快捷键」）。出厂**留空**
    /// （2026-09-27 用户口径），`Alt+N`（`Note` = 便签）留给用户在面板里自配。
    #[serde(default = "default_hk_note")]
    pub hotkey: String,
    /// 背景颜色 `[R,G,B,A]`：**标题栏窗**那一层用它（含 1px 边环）。
    ///
    /// ★ `A` 是**这一层自己的**不透明度，与下面 [`Self::alpha`] **相乘**才是最终
    ///   不透明度（2026-09-27 用户口径）。它以前形同虚设（`COLORREF` 只有 3 个字节，
    ///   第 4 个直接被丢），现在靠"标题栏与正文各是一张 layered 窗"真正生效。
    #[serde(default = "default_note_bg")]
    pub bg_color: [u8; 4],
    /// 文本颜色（正文 + 标题栏按钮/文字）：`A` = 文字自身的不透明度，按它**预混**到
    /// 下面的底色上（层内不透光，混完再乘整层 alpha，与真 alpha 合成等价）。
    #[serde(default = "default_note_text")]
    pub text_color: [u8; 4],
    /// 编辑区颜色（正文文本区背景，可与整体背景区分）：`A` = **正文窗**那一层的不透明度。
    #[serde(default = "default_note_edit")]
    pub edit_color: [u8; 4],
    /// 整张便签的**总**不透明度（百分比，10–100）。
    ///
    /// ★ 它是**新便签的出厂值**，也是配置变化时广播给现存便签的值；单张便签在标题栏
    ///   ◐ 拖动条上调出来的值只活在那张便签里，**不会写回这里**（用户口径）。
    #[serde(default = "default_note_alpha")]
    pub alpha: u32,
    /// 字号（逻辑像素，9–72）。
    #[serde(default = "default_note_font")]
    pub font_size: f32,
}

impl Default for NoteSettings {
    /// ★ 与各字段 `#[serde(default = ...)]` 逐字一致（见 `TimerSettings` 的同款注释：
    /// 否则 `Config::default()` 不是 `from_config → apply_to` 的固定点，面板首帧误报改动）。
    fn default() -> Self {
        Self {
            enabled: false,
            hotkey: default_hk_note(),
            bg_color: default_note_bg(),
            text_color: default_note_text(),
            edit_color: default_note_edit(),
            alpha: default_note_alpha(),
            font_size: default_note_font(),
        }
    }
}

/// 画中画面板宽 / 高的量程：屏幕宽高的**比例**，10%–50%。
pub const PIP_PANEL_MIN: f32 = 0.1;
pub const PIP_PANEL_MAX: f32 = 0.5;
/// 画中画放大倍率的量程：1–5 倍整数（2026-09-21 与全屏统一收敛到 5）。
pub const PIP_ZOOM_MIN: u32 = 1;
pub const PIP_ZOOM_MAX: u32 = 5;

/// 剪贴板面板窗口记忆（上次位置 + 置顶态）。
///
/// ★ 2026-09-29 用户口径：窗口位置/大小这类记忆**并入总配置**，不再单独养一个
/// `panel.json`。写回链路：剪贴板线程变更 → 全局槽位 → 帧循环取走 → `Config::save()`
/// ——config.json 保持**单写者**（运行时线程），这是不直接让剪贴板线程写它的原因。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct ClipPanelGeom {
    pub x: i32,
    pub y: i32,
    pub pinned: bool,
}

/// 剪贴板历史（`ClipboardSettings`：监控 + 面板 + 粘贴）。
///
/// 与便签同构：**没有第二处总开关以外的开关**——`enabled` 即功能总开关，`hotkey`
/// 为空串即停用呼出面板。一条独立原生 Win32 顶层窗承载「列表 + 搜索 + 分类页签 + 收藏
/// + 悬停预览」，复制即记录、点击条目即写回剪贴板并模拟 Ctrl+V（见 `platform::clipboard`
///   与 `modules::clipboard`）。
///
/// 明确不做（超出范围）：多设备同步 / 富文本保留 / 密码箱 / 分片 / 标签 / 云存储 / SQLite。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClipSettings {
    /// 功能总开关。
    #[serde(default)]
    pub enabled: bool,
    /// 呼出面板的快捷键（空串 = 停用；面板这行标签叫「快捷键」）。出厂**留空**
    /// （2026-09-27 用户口径），`Alt+V` 留给用户在面板里自配。
    #[serde(default = "default_hk_clipboard")]
    pub hotkey: String,
    /// 历史容量上限（条目数；超出淘汰**非收藏**最旧条目）。
    #[serde(default = "default_clip_max_entries")]
    pub max_entries: usize,
    /// 面板窗口记忆（上次位置 + 置顶态）；`None` = 无记忆（面板居中，2026-09-27 口径）。
    #[serde(default)]
    pub panel: Option<ClipPanelGeom>,
}

impl Default for ClipSettings {
    /// ★ 与 `#[serde(default = ...]` 逐字一致（`Config::default()` 是 `from_config → apply_to`
    /// 的固定点，见 `TimerSettings::default` 的注释：否则面板首帧误报改动、反复落盘）。
    fn default() -> Self {
        Self {
            enabled: false,
            hotkey: default_hk_clipboard(),
            max_entries: default_clip_max_entries(),
            panel: None,
        }
    }
}

/// 历史容量档（下拉）：500 / 1000 / 2000 / 不限。
///
/// ★ 2026-09-24 用户定案：去掉最小的 200，末端改「不限」。
pub const CLIP_ENTRIES_LABELS: [&str; 4] = ["500", "1000", "2000", "不限"];
/// 与 [`CLIP_ENTRIES_LABELS`] 同序的真实容量值。出厂「不限」（下标 3，2026-10-08）。
///
/// 「不限」= `usize::MAX`：淘汰判据是 `len() > max`，恒不成立，等于不淘汰。
/// 序列化到 config.json 是 `18446744073709551615`，回读时按值反查下标仍命中，
/// 不需要额外的哨兵字段。
pub const CLIP_ENTRIES_VALUES: [usize; 4] = [500, 1000, 2000, usize::MAX];

/// 画中画放大（`magnifier::pip`）设置。
///
/// 与 [触发计划《magnifier-and-hover-locate.md》] 一致：独立原生模块，复用覆盖层
/// `image` 指令，抓鼠标附近方域放大画到屏幕一角。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MagnifierPipSettings {
    /// 总开关（面板勾选 = **载入模块**，2026-09-26 用户口径）。
    ///
    /// ★ 与 `active` 的两段语义：勾上只是"允许开启"（武装），真正在屏显示由
    /// **启停快捷键**切换（`active`）。与录屏快捷键同款：没有快捷键/不激活，勾了也不上屏。
    #[serde(default)]
    pub enabled: bool,
    /// 运行中（快捷键激活的瞬时态）。
    ///
    /// ★ **刻意不落盘**（`skip`）：这是"当前在不在屏上"的会话态，重启后理应回到
    /// "载入但未激活"——把上次退出时的放大画面复活出来反而是惊吓。
    #[serde(skip, default)]
    pub active: bool,
    /// 放大面板宽：屏幕宽的**比例**（0.1–0.5）。
    #[serde(default = "default_pip_panel_w")]
    pub panel_width: f32,
    /// 放大面板高：屏幕高的**比例**（0.1–0.5）。
    #[serde(default = "default_pip_panel_h")]
    pub panel_height: f32,
    /// 放大倍率（1–5 倍整数）。
    #[serde(default = "default_pip_zoom")]
    pub zoom: u32,
    /// 面板位置：0=鼠标跟随 1=左上 2=右上 3=左下 4=右下。
    #[serde(default = "default_pip_anchor")]
    pub anchor: u32,
    /// 水平偏移（屏幕宽度的百分比 0–50）：贴左/右边时把面板从边上往屏内挪。
    /// 鼠标跟随没有"贴边"可言，此偏移不生效。
    #[serde(default)]
    pub offset_x: u8,
    /// 垂直偏移（屏幕高度的百分比 0–50）：贴下边抬高即可让开任务栏（2026-09-21 用户报告）。
    #[serde(default)]
    pub offset_y: u8,
    // ★ 缩放快捷键（Ctrl+Win+滚轮）自 2026-09-21 夜起为**内置**，不设配置项、面板不
    //   暴露——完整说明见 `platform::input::mouse` 的 WM_MOUSEWHEEL 注释。历史上这里
    //   曾有 zoom_in_hotkey / zoom_out_hotkey / pip_wheel_zoom 三个字段（同日中间版），
    //   已删除；老配置里的残留键被 serde 静默忽略（本结构体未开 deny_unknown_fields），
    //   门禁 `legacy_zoom_hotkey_keys_are_ignored` 钉住这一行为。
    /// 面板边框颜色（`#RRGGBBAA` 或 `#RRGGBB`）。
    #[serde(default = "default_pip_border")]
    pub border_color: String,
    /// 启停快捷键（开关一键切换；留空则只能从面板开关）。
    ///
    /// ★ 2026-09-26 用户口径：出厂**留空**——快捷键是"要用的人自己去配"的东西，
    ///   不预占任何组合键。配置后按一次激活、再按退出；与全屏放大互斥的规则
    ///   同 `runtime::zoom::set_fullscreen_zoom`（开这个关那个）。
    #[serde(default)]
    pub hotkey: String,
}

impl Default for MagnifierPipSettings {
    /// ★ 必须与各字段的 `#[serde(default = ...)]` **逐字一致**（`Config::default()`
    /// 是 `from_config → apply_to` 的固定点，见 `TimerSettings::default` 的注释）。
    /// 面板 = 屏幕宽高 × 比例；取样区 = 面板 ÷ 倍率。默认 20% 宽 × 20% 高 @ 2 倍。
    fn default() -> Self {
        Self {
            enabled: false,
            active: false,
            panel_width: default_pip_panel_w(),
            panel_height: default_pip_panel_h(),
            zoom: default_pip_zoom(),
            anchor: default_pip_anchor(), // ★ 1=左上（此前硬抄 0=鼠标跟随，与 serde 默认双源不一致）
            offset_x: 0,
            offset_y: 0,
            border_color: default_pip_border(),
            hotkey: String::new(),
        }
    }
}

/// 全屏放大倍率的量程：1–5 倍整数。
///
/// ★ 2026-09-21 与画中画统一收敛到 1–5（此前是 1–8）：8 倍下取样区只剩屏幕的 1/8，
///   GDI 最近邻放大到整屏的颗粒感已经不可用，5 是"还能看清"的上限。两对常量仍分开
///   保留——量纲虽同，量程各自独立演进，不共用一份。
pub const FULLSCREEN_ZOOM_MIN: u32 = 1;
pub const FULLSCREEN_ZOOM_MAX: u32 = 5;

/// 全屏放大（`magnifier::fullscreen`）设置。
///
/// # 与画中画的关系：同一个底层，两种形态
///
/// 全屏放大**不是**另一套实现——它复用的就是画中画那套链路（`platform::pip_window`
/// 那个自带 `WDA_EXCLUDEFROMCAPTURE` 的原生分层窗 + `screenshot::grab`），区别只有
/// 一个几何参数：**面板尺寸从"屏幕的一小块"改成"整个虚拟屏"**。
///
/// 面板一旦等于整屏，"面板位置"就没有意义了（`clamp` 到屏内后恒等于屏幕左上角），
/// 于是"跟随鼠标"自然地落到**取样区**上：取样区在屏幕内滑动、中心对准鼠标、到边
/// 缘被钳住。这正是 Windows 放大镜全屏模式的行为——**不需要把面板撑成 2 倍屏幕**。
///
/// 两个功能互斥（全屏会盖住画中画），归一规则见 `ui::model::apply_to`。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MagnifierFullscreenSettings {
    /// 总开关（面板勾选 = **载入模块**，语义同 `MagnifierPipSettings::enabled`）。
    #[serde(default)]
    pub enabled: bool,
    /// 运行中（快捷键激活的瞬时态；刻意不落盘，理由同画中画）。
    #[serde(skip, default)]
    pub active: bool,
    /// 放大倍率（1–5 倍整数）。
    #[serde(default = "default_fullscreen_zoom")]
    pub zoom: u32,
    /// 启停快捷键（开关一键切换；留空则只能从面板开关）。
    ///
    /// ★ 2026-09-26 用户口径：出厂**留空**（此前是 `Alt+Z`）。全屏窗置顶盖住一切时
    ///   脱身的路仍是 **Esc**（`runtime::input` 的 Esc 分支不依赖这个字段）；快捷键是
    ///   "要用的人自己去配"的东西，不预占任何组合键。
    #[serde(default)]
    pub hotkey: String,
}

impl Default for MagnifierFullscreenSettings {
    /// ★ 必须与各字段的 `#[serde(default = ...)]` **逐字一致**（`Config::default()`
    /// 是 `from_config → apply_to` 的固定点，见 `TimerSettings::default` 的注释）。
    fn default() -> Self {
        Self {
            enabled: false,
            active: false,
            zoom: default_fullscreen_zoom(),
            hotkey: default_hk_fullscreen_zoom(),
        }
    }
}

// ─── 默认值函数 ───

fn default_cursor_outer() -> u32 {
    60
}
fn default_fullscreen_zoom() -> u32 {
    2
}
fn default_pip_panel_w() -> f32 {
    0.2
}
fn default_pip_panel_h() -> f32 {
    0.2
}
fn default_pip_zoom() -> u32 {
    2
}
fn default_pip_anchor() -> u32 {
    1 // 默认左上角贴边
}
fn default_pip_border() -> String {
    "#00FF00C8".into()
}
fn default_cursor_color() -> String {
    "#FF000080".into()
}
fn default_hover_image() -> String {
    String::new()
}
fn default_hover_type() -> String {
    "solid".to_string()
}
fn default_sound_volume() -> u32 {
    100
}
fn default_ripple_size() -> u32 {
    100
}
fn default_ripple_duration() -> u32 {
    300
}
fn default_trail_effect() -> String {
    "pixie_dust".into()
}
fn default_timer_mode() -> String {
    "countup".into()
}
fn default_timer_duration() -> u64 {
    300
}
/// 计时器出厂默认（2026-09-27 用户口径）：**顶部居中 + 双偏移 0 + 48pt 字号**。
///
/// 位置这块之所以给"顶部居中的整数下标"而不是"0 中央"：中央默认读秒会压在屏幕正中，
/// 挡视线也容易被其它浮窗盖住；顶部居中是"看得见又不碍事"的那一档。偏移量纲是屏幕
/// 宽/高的百分比，0 即不动。
fn default_timer_anchor() -> u32 {
    1
}
/// 计时器字号出厂 48pt（2026-09-27 用户口径）。旧版硬编码的是 72pt，"位置/字号可调"
/// 落地时把它搬成了默认值；这次按用户定档下调到 48。
fn default_timer_font() -> u32 {
    48
}
fn default_ripple_bindings() -> HashMap<String, ButtonRippleCfg> {
    HashMap::new()
}

/// 反序列化缺字段时的默认值：**空表**（与 `default_ripple_bindings` 一致）。
///
/// 不用"五键齐全"的版本：空表才能让 [`Config::migrate_sound_bindings`] 分辨出
/// "这是 1.0 的老配置，一条声音绑定都没有"，从而把老字段搬进来。
fn default_sound_bindings() -> HashMap<String, ButtonSoundCfg> {
    HashMap::new()
}

/// 出厂默认的五键声音绑定。
///
/// ★ 关键不变量：五键的 `file` 都必须**指向一个真实存在的声音**（这里统一用
/// `default_sound_file()`）。留空串会被下拉解析成"目录里第一个 wav"（`index_of("")` 回退 0），
/// 于是面板第一次 `apply_to` 就把 `""` 改写成 `"click1"` 之类的名字——同样属于"没动控件却
/// 落盘"。中键 / 侧键默认关着，预置一个声音并不会响；真要换，下拉就在标题栏上。
fn seeded_sound_bindings() -> HashMap<String, ButtonSoundCfg> {
    let preset = default_sound_file();
    BUTTON_KEYS
        .into_iter()
        .map(|k| {
            (
                k.to_string(),
                ButtonSoundCfg {
                    file: preset.clone(),
                    ..Default::default()
                },
            )
        })
        .collect()
}

/// 出厂默认的五键波纹绑定。
///
/// ★ 关键不变量：`script` 必须**已经**写着解析后的效果 id（"ring"）。
/// `ui::model::apply_to` 把选中效果写回 `script`，若出厂值留空串（靠 `style` 兜底），
/// 那么面板第一次 `apply_to` 就会把 `""` 改成 `"ring"` —— 表现为"没动任何控件却落盘"，
/// 会让 `model_no_spurious_change` 这类 roundtrip 断言失败，也会让配置文件每次开面板
/// 都被重写。
///
/// `style` / `size` / `duration` / `color` 是 1.0 的内置样式字段，现在**只读**：
/// 脚本缺失时 Rust `Ripple` 兜底用它决定外观（见 `modules::trail::controller::on_click`），
/// 同时也是老配置 → 脚本参数的迁移源（见 `ui::model::migrate_clickfx`）。
fn seeded_ripple_bindings() -> HashMap<String, ButtonRippleCfg> {
    let mk = |color: &str| ButtonRippleCfg {
        enabled: false,
        style: "ring".into(),
        size: 100,
        duration: 300,
        color: color.into(),
        script: "disc".into(),
        script_params: HashMap::new(),
    };
    // §39：颜色统一 #FF0000（脚本默认），不再沿用 1.0 的左黄/右蓝/中白。
    let colors = ["#FF0000", "#FF0000", "#FF0000", "#FF0000", "#FF0000"];
    BUTTON_KEYS
        .into_iter()
        .zip(colors)
        .map(|(k, c)| (k.to_string(), mk(c)))
        .collect()
}

fn default_sound_file() -> String {
    "mouseclick1".into()
}
fn default_fade_duration() -> f32 {
    1.0
}
fn default_density() -> u32 {
    5
}
fn default_trail_opacity() -> f32 {
    0.5
}
fn default_spawn_interval() -> u32 {
    100
}
fn default_particle_size() -> u32 {
    5
}
fn default_color_mode() -> String {
    "rainbow".into()
}
fn default_fixed_color() -> String {
    "#FFFF00".into()
}
fn default_placement() -> String {
    "bottom_right".into()
}
fn default_bottom_offset() -> i32 {
    // 2026-09-25 晚：垂直偏移出厂 5（原 10）。
    5
}
/// 虚拟键盘的「下偏移」出厂值：**5（=%）**。
///
/// ★ 为什么另立一个而不共用 [`default_bottom_offset`]（5）：两个功能各自演进，
/// "改一处动两家"是隐患——与 `default_vk_font_size` / `default_font_size` 同一条纪律。
/// （2026-09-25 晚两者恰好同为 5，但这是巧合不是合并理由。）
fn default_vk_bottom_offset() -> f32 {
    5.0
}
/// 虚拟键盘的「右偏移」出厂值：**0.5（=%）**（2026-09-26 用户定档）。
///
/// 出厂方位是 `bottom_right`，所以「水平偏移」这一根收敛到 `right_offset` 上
/// （见 `ui::model::apply_to` 的收敛段）。0.5% 让整块键盘离右边只留一条窄缝。
fn default_vk_right_offset() -> f32 {
    0.5
}
fn default_popup_width() -> u32 {
    15
}
fn default_opacity() -> f32 {
    1.0
}
fn default_fade_delay() -> u32 {
    10
}
fn default_font_size() -> u32 {
    16
}
fn default_font_color() -> [u8; 4] {
    [255, 255, 0, 255]
}
fn default_fun_color() -> [u8; 4] {
    [255, 0, 0, 255]
}
fn default_symbol_color() -> [u8; 4] {
    [255, 255, 255, 255]
}
fn default_bg_color() -> [u8; 4] {
    // 2026-09-25 晚：背景色出厂 #00000040（黑 + 25% alpha）。
    // ★ 顺序是 **[R, G, B, A]**（见 `modules::key_display::settings::rgba_to_argb`：
    //   `Argb::new(rgba[3], rgba[0], …)`）——写成 [0x40,0,0,0] 会被读成 R=0x40 全透明红，
    //   面板显示 "#40000000"（2026-09-25 夜实测）。
    [0, 0, 0, 0x40]
}
fn default_merge_interval() -> f32 {
    // 2026-09-25 晚：合并间隔出厂 1.0（原 1.5），量程同步改 0.5–10（见面板滑条）。
    1.0
}
fn default_display_mode() -> String {
    "all".into()
}
fn default_pen_color() -> String {
    "#000000FF".into()
}
fn default_pen_width() -> u32 {
    4
}
fn default_pen_style() -> String {
    "solid".into()
}
fn default_toggle_drawing() -> String {
    "Ctrl+Shift+D".into()
}
fn default_panel_hotkey() -> String {
    "Ctrl+Shift+D".into()
}
/// ★ 2026-09-27 用户口径：这两把**出厂留空**（空 = 停用）。
///
/// 配套的还有两件事：`#[serde(default = "…")]` 与 `impl Default` 都走这里（同值，不会
/// "手删一行配置就换一套默认"）；`build/config.json` 里的旧值键（`"Alt+G"` / `"Alt+W"`）
/// 必须同步删掉，否则新默认被旧值挡住。
///
/// 这把键以前是「进标注 / 进白板」的唯一入口，`Alt+G` / `Alt+W` 出厂就占着用户的顺手
/// 键位。现在这两项归于「屏幕标注」卡的总开关 + 页内按钮，快捷键归用户自己挑；
/// **画笔 1..9 / 橡皮 / 直线…那 9 把仍是出厂单键**（`1`..`9`），它们是功能内部的切换
/// 键、不与系统冲突，用户也没要求动。
fn default_hk_screen() -> String {
    String::new()
}
fn default_hk_white() -> String {
    String::new()
}
fn default_hk_pen1() -> String {
    "1".into()
}
fn default_hk_pen2() -> String {
    "2".into()
}
fn default_hk_pen3() -> String {
    "3".into()
}
fn default_hk_eraser() -> String {
    "4".into()
}
fn default_hk_line() -> String {
    "5".into()
}
fn default_hk_arrow() -> String {
    "6".into()
}
fn default_hk_rect() -> String {
    "7".into()
}
fn default_hk_ellipse() -> String {
    "8".into()
}
fn default_hk_text() -> String {
    "9".into()
}
fn default_hk_mosaic() -> String {
    "0".into()
}
/// 「序号」贴纸的出厂键位：留空（停用）。
///
/// ★ 空串是**唯一**合法的出厂值（其它书写键出厂就占着 `1`~`9`/`0`），留空才有得让用户在
///   面板里自己挑一把不打架的键；这也是"出厂默认只许一处字面量"的落点（仅此一处）。
fn default_hk_stamp() -> String {
    String::new()
}
fn default_true() -> bool {
    true
}
fn default_hk_screenshot() -> String {
    // ★ 2026-09-26 用户口径：截图与贴图的三把快捷键出厂**全留空**（空 = 停用）。
    //   历史出厂值 `Alt+S`（2026-09-26 前的功能默认）随之作废；存量配置不迁移。
    String::new()
}
/// ★★ 出厂值**必须不含 Ctrl**（2026-09-23 用户报错后改，当时出厂 `Alt+Q`）。
///
/// "按住 Ctrl 进截图编辑态"是靠 [`crate::core::hotkeys::with_ctrl`] 从本体拼出一个
/// Ctrl 变体实现的，而该函数对**本身已含 Ctrl** 的规格返回空串 —— 那意味着"再按住 Ctrl"
/// 物理上不存在另一种按法，这把键**永远进不了编辑态**。
///
/// 历史教训：旧默认值 `Ctrl+Alt+Q` 正好踩中——截图（当时的 `Alt+S`）按住 Ctrl 能进编辑态，
/// 截图并贴图按住 Ctrl 毫无反应。改 `Alt+Q` 后本体 = 纯截图并贴图（一键直达，不打折），
/// Ctrl 变体 `Ctrl+Alt+Q` = 编辑态 + 确认后钉到桌面。
///
/// ★ 2026-09-26 起出厂**留空**（用户口径：截图与贴图三把快捷键全不预置）。这条纪律仍然
/// 生效：用户自己配的值若含 Ctrl，[`Config::snap_keys_missing_ctrl_variant`] 会在启动
/// 日志里点名，[`Config::migrate_snap_hotkey_shadowing`] 负责拆撞弦。
///
/// 存量配置**不迁移**：他们配过的键保持原语义。
fn default_hk_screenshot_pin() -> String {
    String::new()
}
/// 贴图键没有编辑语义（`pin_next` 只是贴剪贴板内容），含不含 Ctrl 都无所谓。
/// ★ 2026-09-26 起出厂留空（历史值 `Ctrl+Alt+P`）。
fn default_hk_pin_clipboard() -> String {
    String::new()
}
/// 长截图快捷键同一条出厂留空规则（2026-10-01 新增功能）：一键开关、全局语义，
/// 想用的用户在面板里自己设一把，不替用户占全局键位。
fn default_hk_scroll() -> String {
    String::new()
}
/// 宽截图快捷键：与长截图同一条出厂留空规则（2026-10-01 新增功能）。
fn default_hk_scroll_wide() -> String {
    String::new()
}
/// 贴图历史环容量默认值。与 `runtime::snap::PIN_RING_CAP` 同源（用户要求 10）。
/// config 是 core 层、不依赖 runtime，故此处只能再写一个字面量——由
/// `pin_ring_cap_matches_runtime_default` 这条测试锁住两边一致。
fn default_pin_ring_cap() -> usize {
    DEFAULT_PIN_RING_CAP
}
fn default_hk_record() -> String {
    // ★ 2026-09-26 晚用户口径：录屏快捷键出厂**留空**（历史值 `Alt+R`）。
    //   与截图三键、GIF 录制/裁剪同一条规则——**快捷键出厂全留空，键位交给用户**；
    //   留一把出厂预置的键等于替用户占了一个全局快捷键位（常与其它软件撞），
    //   而"没配键 = 功能未启用"是可见的（面板里空框），不会变成"按了没反应"。
    String::new()
}
fn default_hk_record_region() -> String {
    // ★ 选区录屏快捷键与录屏快捷键同一条出厂留空规则（2026-09-30）：复用 GIF 选框
    //   录区域是低频功能，想用的用户在面板里自己设一把，不替用户占全局键位。
    String::new()
}
fn default_hk_record_trim() -> String {
    // ★ MP4 时间裁剪快捷键同一条出厂留空规则（2026-09-30）。
    String::new()
}
fn default_fps() -> u32 {
    30
}
/// 码率出厂档：**自动**（见 [`DEFAULT_BITRATE`] 的理由）。
fn default_bitrate() -> String {
    DEFAULT_BITRATE.to_string()
}

/// 读码率：同时吃**旧数字下标**与**新字符串取值**。
///
/// 老配置里 `bitrate`（或更老的 `quality`）是个下标，`0/1/2` 分别代表 4/8/14 Mbps；
/// 新配置里是自描述取值（`"auto"` / `"8000"` / …）。两者靠 JSON 的类型天然分开
/// （数字 vs 字符串），所以用 `untagged` 一次分流，不需要额外标记字段。
///
/// ★ 数字一律按**旧语义**翻译：新表里同一个数字的含义已经变了，若按新语义解释，
///   老用户的档位会静默平移一格。
///
/// ★★ 但数字不只有"旧下标"一种来源 —— 手改 `config.json` 的人很自然会写
///   `"bitrate": 40000`（他想要 40 Mbps）。若一律按旧下标处理，这种写法会**静默**
///   回落到「自动」，用户只会觉得"我明明写了码率怎么没用"。所以按数量级分开：
///   `0..=2` 是旧下标（历史上只有这三个值），`>= 500` 视为 kbps 本身，
///   其余（3~499 这种既不是合法下标、也不像码率的数）回落自动。
fn de_bitrate<'de, D>(d: D) -> std::result::Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Number(u64),
        Name(String),
    }
    /// 数字 >= 这个值就当成 kbps 本身（< 500 kbps 的档位不存在，也没有这种手改理由）。
    const KBPS_THRESHOLD: u64 = 500;
    Ok(match Repr::deserialize(d)? {
        Repr::Number(n) => {
            if (n as usize) < RECORD_BITRATE_LEGACY_INDEX.len() {
                RECORD_BITRATE_LEGACY_INDEX[n as usize].to_string()
            } else if n >= KBPS_THRESHOLD {
                n.to_string()
            } else {
                DEFAULT_BITRATE.to_string()
            }
        }
        Repr::Name(s) => s,
    })
}
fn default_mic_denoise() -> String {
    // ★ 2026-09-26 晚用户口径：出厂**中**（`MIC_DENOISE_VALUES[2]`，历史值 `off`）。
    //   出厂默认麦克风已是"跟随系统默认"，不降噪的话笔记本内置麦的底噪（风扇/电流）
    //   会原样录进去，而"录完发现有底噪"是不可逆的。
    MIC_DENOISE_VALUES[DEFAULT_MIC_DENOISE].to_string()
}
fn default_hk_gif() -> String {
    // ★ 2026-09-26 晚用户口径：GIF 录制快捷键**默认空**（停用）。GIF 是低频功能，
    //   想用的用户在面板里自己设一把；不再默认占 Alt+Shift+G。
    String::new()
}
fn default_hk_gif_trim() -> String {
    // ★ 2026-09-26 晚用户口径：与录制快捷键一起**出厂留空**（历史值 `Alt+Shift+T`）。
    //   裁剪是独立编辑入口（不录也能裁磁盘 GIF），但它同样是"一把全局快捷键"，
    //   按同一规则交给用户自配。
    String::new()
}
fn default_hk_timer() -> String {
    // ★ 2026-09-27 用户口径：计时器**快捷键出厂留空**（沿用 §55「所有快捷键出厂留空」）——
    //   不占用户全局键位，空框就是"停用且看得见"。想用再自己配 `Alt+T`
    //   （`Timer` = 计时，与录屏 `Alt+R`、截图 `Alt+S`、标注 `Alt+G` 同为单字母族，
    //   互不相撞）。★ 与计时器一样**不设总开关**：快捷键为空串即等于停用。
    String::new()
}
fn default_hk_note() -> String {
    // ★★ 2026-09-27 用户口径：便签**快捷键出厂留空**（沿用 §55「所有快捷键出厂留空」定稿，
    //   当时便签/剪贴板漏网了，这次补上）——不占用户全局键位，空框就是"停用且看得见"。
    //   想用再自己配：`Alt+N`（`Note` = 便签），与 R/S/G/T/Z 不相撞（同族单字母）。
    //   ★ 与计时器一样「快捷键即最顺手的开关」，不设总开关：快捷键为空串即停用。
    String::new()
}
fn default_hk_clipboard() -> String {
    // ★★ 2026-09-27 用户口径：剪贴板**快捷键出厂留空**（同上）。`Alt+V`
    //   （`View` = 查看历史）这把留给用户在面板里自己配，与 R/S/G/T/Z/N 不相撞
    //   （同族单字母）。「快捷键即最顺手的开关」，不设总开关：快捷键为空串即停用。
    String::new()
}
fn default_clip_max_entries() -> usize {
    // ★ 出厂 = 「不限」（下标 3，2026-10-08 用户定案；早期出厂为 500）。
    //   淘汰判据 `len() > max` 对 usize::MAX 恒不成立 → 只进不出；收藏 / 清空 /
    //   手动删除仍可主动清理。
    CLIP_ENTRIES_VALUES[3]
}
fn default_note_bg() -> [u8; 4] {
    // 出厂便签底（[R,G,B,A]）= `#8AE6FF`：淡青底 + 黑字，一条便签贴上去像纸片。
    [0x8A, 0xE6, 0xFF, 255]
}
fn default_note_text() -> [u8; 4] {
    // 出厂文字色（[R,G,B,A]）= `#000000`：配 `#8AE6FF` 底的黑字，对比度拉满。
    [0x00, 0x00, 0x00, 255]
}
fn default_note_edit() -> [u8; 4] {
    // 出厂编辑区色（[R,G,B,A]）= `#FFFFFF`：编辑区留白比标题栏亮一档，正文区与标题栏可区分。
    [0xFF, 0xFF, 0xFF, 255]
}
fn default_note_alpha() -> u32 {
    // 总不透明度（%）：80 = 半透但看得清（用户定档 2026-09-27，原 100）。
    80
}
fn default_note_font() -> f32 {
    // 便签字号（逻辑像素）：24（用户定档 2026-09-27，原 16）。
    24.0
}
fn default_hk_fullscreen_zoom() -> String {
    // 2026-09-26 用户口径：出厂**留空**（此前 Alt+Z）——快捷键留给用户自己配。
    // 脱身不依赖本字段：全屏窗盖住一切时 Esc 分支仍在（runtime::input）。
    String::new()
}
fn default_vk_layout() -> String {
    "ansi_104".into()
}
/// 一个标准键（1u）的出厂边长（逻辑像素）。
///
/// ★ 单位：1u = 键盘里最普通那颗字母键的边长，其余键按 `w`/`h` 的 u 数等比放大
/// （Backspace 2u、空格 6.25u……）。整块 104 键约 15u 宽 → 64 ≈ 960px。
///
/// 出厂值 32（2026-09-26 用户定档，原 64）。
///
/// 64 那版是因为 34 时键帽只有 34px、图例字号 14px，`Home` / `PgUp` 这类 4 字母图例
/// 横向顶到描边。用户实测后定在 32 —— 与「键间空隙 0 + 圆角 0」同批，整块键盘收成
/// 紧凑的实心块。量程见 `vk_config::Bind::preset_range`（10~80）。
fn default_vk_key_size() -> u32 {
    32
}
/// 键间空隙出厂 **0**（2026-09-26 用户定档，原 3）：键帽连成一片，靠描边分界。
fn default_vk_gap() -> u32 {
    0
}
fn default_vk_hold() -> f32 {
    // 出厂 0.1s。面板量程 0.0~2.0（见 `vk_config::preset_range`）——下限 0 = 松开即灭，
    // 出厂仍留一点点保持：一次快速敲击的按下-抬起常常短于一帧，完全不高亮会让用户
    // 觉得"没反应"。想要"一松开就消失"的，把滑块拖到底即可。
    0.1
}
/// 虚拟键盘自己的字号默认值。
///
/// ★ 不共用 [`default_font_size`]（那是**击键回显**的 16）：两个功能各自演进，
/// "改一处动两家"是隐患。
fn default_vk_font_size() -> u32 {
    14
}
/// 虚拟键盘键帽圆角出厂值（逻辑像素）：**0 = 直角**（2026-09-26 用户定档，原 4）。
///
/// ★ 一个参数只回答一个问题，不留隐式分支（更不再是"用键高的 14% 自动算"）。
/// ★ 改这个值必须**同步** `assets/VirtualKeyboards/ansi_104.json` 的
///   `defaults.cornerRadius`：面板值与出厂值相等时走方案兜底（见
///   `vk_config::f32_of`），只改一处会让"出厂 0"被方案的旧值顶掉。
fn default_vk_corner_radius() -> u32 {
    0
}
fn default_vk_scale() -> f32 {
    1.0
}
/// 键帽底色出厂 `#000000BF`（2026-09-26 用户定档，原 `#282C34D2`）。
///
/// 数组是 `[R, G, B, A]`；对应的十六进制串由 `vk_config::hex_of` 生成
/// —— 全项目颜色串只有 `#RRGGBB` / `#RRGGBBAA` 两种（alpha 在**末尾**）。
fn default_vk_key_color() -> [u8; 4] {
    [0, 0, 0, 0xBF]
}
/// 按下色出厂 `#0000FFBF`（原 `#4C8BF5EB`）。
fn default_vk_press_color() -> [u8; 4] {
    [0, 0, 255, 0xBF]
}
/// 描边色出厂 `#00FFFFBF`（原 `#636E72A0`）。
fn default_vk_border_color() -> [u8; 4] {
    [0, 255, 255, 0xBF]
}

/// 录屏**码率**档：显示名 → 目标码率（kbps，直接进编码器的 `MF_MT_AVG_BITRATE`）。
///
/// ★ 2026-09-19 改名（用户口径）：原「画质」→「码率」。这一档**唯一**作用就是写目标码率，
/// 与像素数无关；叫「画质」会被读成"它也在管分辨率"，于是"选了 720p 还觉得糊"时不知道该
/// 调哪一个。显示名直接用实际码率，用户看到的就是写进文件的值，不经过一次心算。
///
/// ★ 旧注释写"实际按像素数等比缩放"是**失实的**：缩放从来没实现过——`runtime` 把表值
/// 原样送进 `bitrate_kbps`。这正是"感觉分辨率和画质都在影响清晰度"的真实来源：原始分辨率
/// 下 4 Mbps 要喂 368 万像素，每像素比特只有 720p 档的四分之一。要让两档真正正交，得按
/// 像素数算码率；那是独立改动，本轮先把名字与注释改成与实现一致。
pub const RECORD_BITRATE_LABELS: [&str; 7] = [
    "自动", "4 Mbps", "8 Mbps", "14 Mbps", "24 Mbps", "40 Mbps", "60 Mbps",
];
/// 与 [`RECORD_BITRATE_LABELS`] 同序的**落盘取值**（自描述，见 [`RecordSettings::bitrate`]）。
pub const RECORD_BITRATE_VALUES: [&str; 7] =
    ["auto", "4000", "8000", "14000", "24000", "40000", "60000"];
/// 与 [`RECORD_BITRATE_LABELS`] 同序的目标码率（kbps）。`0` = 自动，见 [`auto_bitrate_kbps`]。
pub const RECORD_BITRATE_KBPS: [u32; 7] = [0, 4_000, 8_000, 14_000, 24_000, 40_000, 60_000];

/// 码率出厂档：**自动**。
///
/// ★ 用户口径 2026-09-19：固定档位表无法覆盖所有屏幕 —— 「原始」档在 1440p/4K 屏上
///   就是 14 Mbps 喂 370 万/830 万像素，会饿死；而在 720p 上 14 Mbps 又纯属浪费。
///   自动档把"分辨率 × 帧率"的账算对，是唯一不会在别人的屏幕上出错的选择。
pub const DEFAULT_BITRATE: &str = "auto";

/// 旧版**数字下标** → 新取值（0/1/2 = 4/8/14 Mbps）。只给 [`de_bitrate`] 读老配置用。
pub const RECORD_BITRATE_LEGACY_INDEX: [&str; 3] = ["4000", "8000", "14000"];

/// [`auto_bitrate_kbps`] 的除数，见该函数里的标定点。
const AUTO_BITRATE_DIVISOR: u64 = 7_680;
/// 自动档下限（kbps）：再低连 720p30 的文字都糊。
const AUTO_BITRATE_MIN_KBPS: u32 = 2_000;
/// 自动档上限（kbps）：60 Mbps 已覆盖 4K60，再高只是给磁盘找麻烦。
const AUTO_BITRATE_MAX_KBPS: u32 = 60_000;

/// 自动码率：`像素数 × 帧率 ÷ 7680`，取整到 500 kbps，再夹进上下限。
///
/// 标定点是 **1920×1080 @30fps → 8 Mbps**（`2073600 × 30 ÷ 7680 = 8100`）。选它的理由：
/// 1080p30 是绝大多数人的默认场景，而 8 Mbps 是"录代码/文字也够清晰"的经验下限。
/// 其余档位由同一公式外推：720p30→3.5、1440p30→14.5、4K30→32.5、4K60→65 Mbps。
///
/// ★ 用**一个公式**而不是查表：查表只能覆盖写死的几种分辨率，「原始」档在 5K /
///   带鱼屏 / 竖屏上会直接掉出表外 —— 而那正是最需要"别把码率配错"的场景。
pub fn auto_bitrate_kbps(out_w: u32, out_h: u32, fps: u32) -> u32 {
    let px = u64::from(out_w.max(1)) * u64::from(out_h.max(1));
    // ★ 用 `saturating_mul`：`px × fps` 在极端输入（如 u32::MAX² × u32::MAX）会溢出 u64。
    //   dev 构建（开溢出检查）会 panic，release 构建（关检查）则静默回绕成垃圾值后落到
    //   clamp 的怪异结果。饱和到 u64::MAX 后 `÷ 7680` 仍远超上限，会被 clamp 正确夹回，
    //   两种构建都稳定落在 `AUTO_BITRATE_MAX_KBPS`，而不是崩溃或回绕。
    let raw = px.saturating_mul(u64::from(fps.max(1))) / AUTO_BITRATE_DIVISOR;
    let clamped = raw.clamp(
        u64::from(AUTO_BITRATE_MIN_KBPS),
        u64::from(AUTO_BITRATE_MAX_KBPS),
    );
    (((clamped + 250) / 500) * 500) as u32
}

/// 录屏帧率档。
///
/// ★ 用户口径 2026-09-18：给出 15 / 30 / 60 三档。
/// - **15**：体积优先（长时间录制、或只讲操作不追流畅度）。早期曾判定低帧率不可用，
///   那是因为当时"静态期跳过写帧"——每个 tick 都写帧之后（见 `recorder.rs` 主循环），
///   成片帧率 = 设定值、时长分毫不差，15 档因此可用。
/// - **60**：游戏/滚动内容的流畅度，体积约为 30 档的两倍。
pub const RECORD_FPS_LABELS: [&str; 3] = ["15", "30", "60"];
pub const RECORD_FPS_VALUES: [u32; 3] = [15, 30, 60];

/// 录屏分辨率档的显示名（与 [`RECORD_RESOLUTION_LONG_EDGE`] 同序）。
///
/// 记的是**长边上限**而不是"宽×高"：竖屏显示器、超宽屏都能用同一档位表达，
/// 且缩放交给 GPU（VideoProcessorMFT 的输出类型），CPU 零成本。
///
/// ★ 档位顺序 = **清晰度升序**（第一个仍是「原始」）：`原始 / 720p / 1080p / 1440p / 2160p`。
/// `resolution` 存的是下标，老配置（旧顺序 `原始/1080p/720p/...`）里的 `1=1080p`、`2=720p`
/// 由 [`Config::migrate_resolution_order`] 重映射成新下标 `2` / `1`，见该函数注释。
pub const RECORD_RESOLUTION_LABELS: [&str; 5] = ["原始", "720p", "1080p", "1440p", "2160p"];
/// 与 [`RECORD_RESOLUTION_LABELS`] 对应的长边上限（像素）。`0` = 原始分辨率。
pub const RECORD_RESOLUTION_LONG_EDGE: [u32; 5] = [0, 1280, 1920, 2560, 3840];

/// 硬件加速策略显示名。
pub const RECORD_HARDWARE_LABELS: [&str; 2] = ["自动", "关闭"];

/// 麦克风降噪档位显示名（与 [`MIC_DENOISE_VALUES`] 同序）。单一下拉：关闭 / 弱 / 中 / 强。
pub const MIC_DENOISE_LABELS: [&str; 4] = ["关闭", "弱", "中", "强"];
/// 麦克风降噪档位内部值（传给 `audio_capture::parse_mic_denoise`）。
pub const MIC_DENOISE_VALUES: [&str; 4] = ["off", "weak", "medium", "strong"];
/// 降噪出厂档下标：中（`MIC_DENOISE_VALUES[2]`）。
pub const DEFAULT_MIC_DENOISE: usize = 2;

/// GIF 帧率档。
///
/// GIF 的帧延时单位是 10ms，且**浏览器把 <2cs 当 100ms 播**（见 `core::gif_enc`），
/// 所以实用上限就是 50fps；再往上只是涨体积。屏幕动图 10/15/20 三档够用。
pub const GIF_FPS_LABELS: [&str; 3] = ["10", "15", "20"];
pub const GIF_FPS_VALUES: [u32; 3] = [10, 15, 20];

/// GIF 输出尺寸档。
///
/// 标签带「长边」二字是刻意的：这一档管的是**长边上限**（缩放后的最长那条边），
/// 竖屏/超宽屏用同一档表达。只写「960」会让人以为是宽度，选完发现竖屏没变窄会以为坏了。
pub const GIF_SIZE_LABELS: [&str; 3] = ["长边 960", "长边 1280", "长边 1600"];
pub const GIF_SIZE_LONG_EDGE: [u32; 3] = [960, 1280, 1600];
/// GIF 出厂尺寸档：1280（长边）。动图大多贴在聊天/文档里，1280 已很清楚且体积可控。
pub const DEFAULT_GIF_LONG_EDGE: u32 = 1280;

/// GIF 输出长边的**安全上限**（像素）。
///
/// ★ 这一档**不再是用户选项**（用户口径 2026-09-20）：选框上线之后，"输出尺寸"变成了
///   双重控制——用户框多大就多大，再叠一个档位只会自相矛盾（框了 400px 宽的小区域，
///   三档一个都不生效，纯属摆设）。所以面板上那一行已删，这里只留一个**不暴露的上限**，
///   用途是挡住"框了整屏 → 体积爆掉"：超过才缩，不超过一律 1:1 输出。
///
/// 上面那两个常量仍保留：旧 `config.json` 里的 `gif.size` 要能被读进来（只是不再生效），
/// 而 `GIF_SIZE_LABELS.len()` 的对账测试也还指着它们。
pub const GIF_MAX_LONG_EDGE: u32 = 1600;

/// 贴图历史环容量档（「最多支持 pin 最近的 N 个内容」）。默认档是 [`DEFAULT_PIN_RING_CAP`]。
pub const PIN_CAP_LABELS: [&str; 4] = ["5", "10", "15", "20"];
pub const PIN_CAP_VALUES: [usize; 4] = [5, 10, 15, 20];
/// 出厂档：用户明确要求 10。
pub const DEFAULT_PIN_RING_CAP: usize = 10;

// ─── 主配置结构体 ───

/// 官方脚本清单的默认地址（Gitee 镜像）。
pub const DEFAULT_STORE_GITEE_URL: &str =
    "https://gitee.com/qiuzongman/CurSeen-Scripts/raw/main/scripts.json";
/// 官方脚本清单的默认地址（GitHub 镜像）。
pub const DEFAULT_STORE_GITHUB_URL: &str =
    "https://raw.githubusercontent.com/zongmanqiu/CurSeen-Scripts/main/scripts.json";

/// 本体更新清单的默认地址（Gitee raw CDN）。
pub const DEFAULT_UPDATE_GITEE_URL: &str =
    "https://raw.giteeusercontent.com/qiuzongman/curseen/raw/main/update.json";
/// 本体更新清单的默认地址（GitHub raw）。
pub const DEFAULT_UPDATE_GITHUB_URL: &str =
    "https://raw.githubusercontent.com/zongmanqiu/CurSeen/refs/heads/main/update.json";

/// 在线脚本库（设置页「官方资源下载」）。
///
/// ★ 两条清单地址**进配置**（m40 用户口径）：换镜像/自建镜像都只改 `config.json`，
///   不必重新编译；面板上的来源下拉选中的是哪一条也一并持久化（下次开面板还在）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct StoreSettings {
    /// Gitee 镜像的清单地址；空串 = 该来源不可用（界面按钮置灰、刷新跳过）。
    pub gitee_url: String,
    /// GitHub 镜像的清单地址；空串 = 该来源不可用。
    pub github_url: String,
    /// 选中的来源下标：0 = Gitee / 1 = GitHub。
    pub source: usize,
}

impl Default for StoreSettings {
    fn default() -> Self {
        Self {
            gitee_url: DEFAULT_STORE_GITEE_URL.to_string(),
            github_url: DEFAULT_STORE_GITHUB_URL.to_string(),
            source: 0,
        }
    }
}

impl StoreSettings {
    /// 两条地址按 [`StoreSettings::source`] 的下标顺序交出（`[Gitee, GitHub]`）。
    pub fn urls(&self) -> [String; 2] {
        [self.gitee_url.clone(), self.github_url.clone()]
    }

    /// 夹到合法范围（配置里手写成 9 也不许把界面弄崩）。
    pub fn source_index(&self) -> usize {
        self.source.min(1)
    }

    /// 写入选中来源（夹到 0..=1）。**返回是否真的改了**——面板每帧对账靠它判断脏，
    /// 恒返回 true 会让"开面板零操作也写一次盘"。
    pub fn set_source(&mut self, source: usize) -> bool {
        let source = source.min(1);
        if self.source == source {
            return false;
        }
        self.source = source;
        true
    }
}

/// 本体更新清单（About 页「版本更新」）。
///
/// 选中源**跟随** [`StoreSettings::source`]：同一台机器只需要在设置页「高级」卡里
/// 选一次 Gitee/GitHub，脚本包与本体更新走同一个镜像。两条清单地址仍可分别配置，
/// 换自建镜像不用重新编译。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UpdateSettings {
    /// Gitee 镜像的 `update.json` 地址；空串 = 该来源不可用。
    pub gitee_url: String,
    /// GitHub 镜像的 `update.json` 地址；空串 = 该来源不可用。
    pub github_url: String,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            gitee_url: DEFAULT_UPDATE_GITEE_URL.to_string(),
            github_url: DEFAULT_UPDATE_GITHUB_URL.to_string(),
        }
    }
}

impl UpdateSettings {
    /// 两条地址按 `[Gitee, GitHub]` 交出。
    pub fn urls(&self) -> [String; 2] {
        [self.gitee_url.clone(), self.github_url.clone()]
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Config {
    #[serde(default)]
    pub trail: TrailSettings,
    #[serde(default)]
    pub key_display: KeyDisplaySettings,
    #[serde(default)]
    pub drawing: DrawingSettings,
    /// 截图与贴图（屏幕页顶层卡之一；原「录制」页，2026-09-25 重排）。
    #[serde(default)]
    pub screenshot: ScreenshotSettings,
    /// 视频录制（屏幕页顶层卡之三；原「屏幕录制」）。
    #[serde(default)]
    pub record: RecordSettings,
    /// GIF录制（屏幕页顶层卡之二）。与录屏共用采集管线。
    #[serde(default)]
    pub gif: GifSettings,
    /// 虚拟键盘（「键盘」页，与击键回显并列）。
    #[serde(default)]
    pub virtual_keyboard: VirtualKeyboardSettings,
    /// 击键音效（「键盘」页）：按键时按 Lua 脚本选定的音效发声。
    #[serde(default)]
    pub key_sound: KeySoundSettings,
    #[serde(default)]
    pub general: GeneralSettings,
    /// 覆盖层全局置顶策略（轴 A·窗口置顶，对一切屏幕工具生效）。
    #[serde(default)]
    pub overlay: OverlaySettings,
    #[serde(default)]
    pub ui: UiSettings,
    #[serde(default)]
    pub global: GlobalSettings,
    /// 放大镜·画中画（独立原生模块，放屏幕页；原「其它」页，2026-09-25 重排）。
    #[serde(default)]
    pub magnifier_pip: MagnifierPipSettings,
    /// 放大镜·全屏放大（同一底层的另一种形态，与画中画互斥；放屏幕页）。
    #[serde(default)]
    pub magnifier_fullscreen: MagnifierFullscreenSettings,
    #[serde(default)]
    pub hotkeys: HotkeySettings,
    /// Lua 扩展脚本框架（`assets/lua/` 下的扩展脚本 + 每脚本参数记忆）。见 `LuaExtSettings`。
    #[serde(default)]
    pub lua_ext: LuaExtSettings,
    /// 在线脚本库：两条清单地址 + 当前来源（设置页「官方资源下载」）。
    #[serde(default)]
    pub store: StoreSettings,
    /// 本体更新清单：两条 `update.json` 地址；选中源跟随 [`StoreSettings::source`]。
    #[serde(default)]
    pub update: UpdateSettings,
    /// 便签（「工具」页顶层卡，2026-09-25 重排）。
    #[serde(default)]
    pub sticky_note: NoteSettings,
    /// 剪贴板历史（「工具」页顶层卡）。独立原生 Win32 面板窗 + 后台监听。
    #[serde(default)]
    pub clipboard: ClipSettings,
    /// ★ 一次性迁移标记（2026-09-25）：8 位颜色串已统一为 `#RRGGBBAA`。
    ///
    /// 此前写盘的是 `#AARRGGBB`，与取色器 / Lua 脚本 / `[R,G,B,A]` 数组三处对不上
    /// （`#FF000040` 被读成不透明深蓝、`#00FFCCBF` 被读成全透明）。升级时 [`Config::load`]
    /// 会在 **JSON 层**把老串轮转一次并立刻落盘；这个标记保证只做一次。
    /// 恒 false 时不写盘（避免"打开面板零操作也改配置"）。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub color_hex_v2: bool,
    /// 录屏分辨率档从旧顺序「原始 / 1080p / 720p / 1440p / 2160p」调整为新顺序
    /// 「原始 / 720p / 1080p / 1440p / 2160p」（清晰度升序）后，`record.resolution` 存的是
    /// 下标，老配置里的 `1`=1080p、`2`=720p 必须重映射成新下标 `2` / `1`。这个标记锁住
    /// "只映射一次"——否则已迁移的 `2`（1080p）下次启动又会被映射回 `1`（720p），越改越错。
    /// 恒 false 时不写盘（避免"打开面板零操作也改配置"）。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub resolution_order_v2: bool,
}

impl Config {
    /// 画中画放大是否**在屏运行**：勾选（载入）× 快捷键激活，两者同时为真。
    ///
    /// ★ 2026-09-26 用户口径：面板勾选只是"载入模块"，真正上屏只能靠启停快捷键。
    ///   所有帧门/渲染/滚轮武装的判据一律用本函数，**不要**直读 `enabled`——
    ///   那会把"勾了但还没按键"误当成"在屏"。
    pub fn pip_running(&self) -> bool {
        self.magnifier_pip.enabled && self.magnifier_pip.active
    }

    /// 全屏放大是否**在屏运行**（见 [`Self::pip_running`]）。
    pub fn fullscreen_running(&self) -> bool {
        self.magnifier_fullscreen.enabled && self.magnifier_fullscreen.active
    }
}

/// Lua 扩展脚本框架设置（`config.lua_ext`）。
///
/// 通用扩展脚本统一放在 `assets/lua/scripts/`（源码 `main/assets/lua/scripts`，部署同步到
/// `build/assets/lua/scripts`）；`MouseTrail/` 等模块特效脚本仍归各模块文件夹。
/// 每脚本一套参数（`params[id][key]`），值与键由脚本 `schema()` 声明，机制与
/// `key_sound.params` 完全一致：**没动过的参数不落盘**，缺键时脚本侧 `params.x or 默认` 兜底。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LuaExtSettings {
    /// 每脚本参数表：`params[脚本id][参数key] = 值`。
    #[serde(default)]
    pub params: HashMap<String, HashMap<String, Value>>,
    /// ★ 脚本页**常驻运行列表**（2026-09-27 起：不止 `composite`——`scripts/` 下任何
    /// 可运行脚本（含 `role="effect"`、未声明 role 的）开关打开都落这里）。
    ///
    /// 用户在 `scripts/` 放脚本（拖尾 + 点击声音 + …同时实现，或直接复制一份特效脚本），
    /// 宿主给脚本全开放 API（事件 + 绘制 + 声音）；列表里的每个 id 一个常驻实例**同时运行**。
    /// 空表不落盘（skip_serializing_if），出厂数据零冗余；参数仍存 `params[id]`。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub composite_active: Vec<String>,
    /// ★ image 脚本**启用列表**（2026-09-27 用户口径：`scripts/` 下每个脚本都要有开关）。
    ///
    /// 这一族是贴图右键「脚本」里的**手动动作**：开关打开 = 菜单里出现这一条，
    /// 关掉 = 收起（不留空条目）。出厂为空（与"所有脚本出厂全关"同口径），所以首次
    /// 打开时「脚本」是空壳 + 灰色占位项——用户去脚本页勾开要用的脚本即可。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub image_active: Vec<String>,
    /// ★ 包卡下拉选择：`pack_id -> child_id`。
    ///
    /// 包卡在脚本页是**一张卡**，标题栏下拉决定当前运行哪个子脚本；`composite_active`
    /// 仍只记"实际在跑的脚本 id"，因此开关/运行列表的判据不需要第二套。
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub pack_selected: HashMap<String, String>,
}

impl LuaExtSettings {
    /// 有没有常驻实例在跑（帧门判据用）。★ 只看 `composite_active`——image 脚本点了才
    /// 跑一次、不常驻，不该让覆盖层因为"菜单开着"就一直不 park。
    pub fn composite_running(&self) -> bool {
        !self.composite_active.is_empty()
    }

    /// 这个脚本的开关是否**打开**（不看 role）。★ 脚本热键派发的**唯一**判据入口
    /// （2026-09-30：卡片开关关着时热键还能跑脚本，是因为派发侧只看 `params[id].hotkey`、
    /// 从不查启用列表）。
    ///
    /// 为什么不走 [`Self::role_is_active`]：那要先知道 role，而 role 得扫 Lua 目录
    /// （热键在**每次按键**的热路径上，不能每次扫盘）。两张启用列表本身就是 role 唯一
    /// 的落点（见 [`Self::role_list`] 的落点映射），所以"任一列表里有它"与
    /// `∃role. role_is_active(role, id)` 同义——等价性由单测钉死。
    ///
    /// `keysound` 脚本无落点（由键盘页 `key_sound.script` 管理），因此这里恒为关：
    /// 它本来也不该注册全局热键。
    pub fn script_active(&self, id: &str) -> bool {
        self.composite_active.iter().any(|x| x == id) || self.image_active.iter().any(|x| x == id)
    }

    /// 某 role 下这个脚本的开关是否**打开**。★ 唯一判定入口：UI 初值、菜单过滤、
    /// 帧门判据一律走这里，不许再备第二套口径（双源必错——曾因"菜单缺键=开"与
    /// "脚本卡缺键=关"分叉，出厂态自己打自己）。
    ///
    /// ★ 未知 role 一律当关（新 role 忘了登记列表 = 出厂缺省，而不是"默认全开"）。
    pub fn role_is_active(&self, role: &str, id: &str) -> bool {
        self.role_list(role).iter().any(|x| x == id)
    }

    /// 该 role 的启用列表。★ 落点映射（2026-09-27 起放宽）：
    /// - `image` → `image_active`（菜单可见性，语义独立）；
    /// - `keysound` → **无落点**（击键音效由键盘页 `key_sound.script` 专属管理，不走开关列表）；
    /// - **其它一切 role（`composite`/`effect`/未来新 role）→ `composite_active`**
    ///   （常驻运行列表）。这样"往 `scripts/` 复制任何特效脚本"都有开关可开——
    ///   `scripts/fireflies`（`role="effect"`）与 `scripts/composite_demo` 同一落点。
    ///
    /// 列表本身就带语义，取长度/查成员都用它，避免调用方 `match role` 再抄一遍判据。
    pub fn role_list(&self, role: &str) -> &[String] {
        match role {
            "image" => &self.image_active,
            "keysound" => &[],
            _ => &self.composite_active,
        }
    }

    /// 打开 / 关掉某 role 下这个脚本的开关（幂等；列表空 ⟺ 全关，不落盘的保证
    /// 由字段级 `skip_serializing_if` 承担）。落点映射与 [`Self::role_list`] 一致；
    /// `keysound` 无落点：宁可什么都不写，也不要悄悄落到一个没人读的列表里。
    pub fn set_role_active(&mut self, role: &str, id: &str, active: bool) {
        let list = match role {
            "image" => &mut self.image_active,
            "keysound" => return,
            _ => &mut self.composite_active,
        };
        let has = list.iter().any(|x| x == id);
        if active && !has {
            list.push(id.to_string());
        } else if !active && has {
            list.retain(|x| x != id);
        }
    }
}

/// 击键音效设置。对应 `config.key_sound` 的 Rust 侧语义层。
///
/// # 脚本契约（assets/lua/ 下的预设脚本头注释是权威版本；统一 `run(ctx,params)`）
///
/// 每次按键（含长按自动重复）宿主经引擎缓存调脚本 `run(ctx, params)`，
/// `ctx` 按 `role="keysound"` 打包：
/// - `ctx.key`：与击键回显一致的键名（`KeyCode::display_name`）；
/// - `ctx.mods`：`{ ctrl=bool, alt=bool, shift=bool, win=bool }`；
/// - `ctx.repeated`：是否长按自动重复；`ctx.dt`：距上次按键时长；
/// - `ctx.sounds`：宿主扫「音效文件夹」得到的音频文件名列表（`{"a.wav",…}`）。
///
/// 返回值语义：`ctx.sounds` 里的**文件名**（或它的**索引**）→ 播放它；
/// `false` → **强制静音**（连默认音也不响）；`nil` → 落回「音效文件」行的默认音。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KeySoundSettings {
    /// 总开关。
    #[serde(default)]
    pub enabled: bool,
    /// 脚本 id = `assets/lua/KeySound/` 下的文件名 stem（击键音效专属目录，
    /// 如 `KeySound/unified_sound`，解析用 `lua_ext_dir().join(id+".lua")`）。
    #[serde(default = "default_key_sound_script")]
    pub script: String,
    /// 音量百分比（0–100）。
    #[serde(default = "default_key_sound_volume")]
    pub volume: u8,
    /// 默认音（兜底）：脚本 `run` 返回 `nil` 时播这个。存**文件名**，解析顺序
    /// 便携相对 → `assets/Sounds/`（出厂 `click3` 就在鼠标点击音效目录里）。
    /// ★ 2026-09-25 起面板不再有这一行（脚本参数里的 `file` 浏览按钮取代了它），
    /// 字段仅作出厂兜底保留。
    #[serde(default = "default_key_sound_file")]
    pub file: String,
    // 「音效文件夹」（sound_dir）已删（2026-09-25 用户口径）：脚本参数的 `file`
    // 浏览按钮直接选文件并**记路径**（程序目录内记便携相对路径）。旧配置里的
    // `sound_dir` 键被静默忽略（本结构体未开 `deny_unknown_fields`）。
    /// ★ 每脚本参数表：`params[脚本id][参数key] = 值`（键与类型由脚本 `schema()`
    ///   声明，机制与拖尾 `hover_params` 完全一致：面板按 schema 动态生成选项，
    ///   换脚本换一套参数，各自分别记忆）。未出现的键用脚本默认值——所以面板
    ///   "没动过的参数"不落盘。
    #[serde(default)]
    pub params: HashMap<String, HashMap<String, Value>>,
}

fn default_key_sound_script() -> String {
    "KeySound/unified_sound".into()
}
fn default_key_sound_volume() -> u8 {
    // ★ 与点击音效 `default_sound_volume`（100）同值（2026-09-26 统一）：两处音量出厂
    //   手感一致，不再一个 80 一个 100。模块侧 `KeySoundModule::new` 直接取
    //   `KeySoundSettings::default().volume`，不另抄一份字面量。
    100
}
fn default_key_sound_file() -> String {
    // 出厂即鼠标点击音效目录里的 click3（用户 2026-09-21 指定）。
    "click3".into()
}

impl Default for KeySoundSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            script: default_key_sound_script(),
            volume: default_key_sound_volume(),
            file: default_key_sound_file(),
            params: HashMap::new(),
        }
    }
}

impl Default for TrailSettings {
    fn default() -> Self {
        Self {
            hover_active: None,
            cursor_mode: 0,
            cursor_outer: 60,
            cursor_arc: 5,
            cursor_color: "#FF000080".into(),
            hover_enabled: false,
            hover_type: "solid".into(),
            hover_params: HashMap::new(),
            hover_image: String::new(),
            // §40 重置标记：默认 false（serde skip，不落盘）。
            ripple_params_reset: false,
            // §41：点击声音音量出厂 100（原 50）。
            sound_volume: 100,
            // 2026-09-25 晚：所有功能出厂**全关**（一级父卡选框一律未勾选）。
            ripple_enabled: false,
            sound_enabled: false,
            ripple_size: 100,
            ripple_duration: 300,
            ripple_bindings: seeded_ripple_bindings(),
            trail_effect: "pixie_dust".into(),
            trail_enabled: false,
            trail_settings: TrailDetail::default(),
            ripple_left_enabled: false,
            ripple_right_enabled: false,
            sound_bindings: seeded_sound_bindings(),
            sound_left_enabled: false,
            sound_right_enabled: false,
            sound_left_file: "mouseclick1".into(),
            sound_right_file: "mouseclick1".into(),
        }
    }
}

impl Default for KeyDisplaySettings {
    fn default() -> Self {
        Self {
            enabled: false,
            placement: "bottom_right".into(),
            top_offset: 0,
            bottom_offset: 5,
            left_offset: 0,
            right_offset: 0,
            popup_width: 15,
            opacity: 1.0,
            fade_delay: 10,
            font_size: 16,
            font_color: [255, 255, 0, 255],
            fun_color: [255, 0, 0, 255],
            symbol_color: [255, 255, 255, 255],
            bg_color: [0, 0, 0, 0x40], // #00000040（[R,G,B,A]）
            merge_interval: 1.0,
            display_mode: "all".into(),
        }
    }
}

impl Default for DrawingSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            hotkeys: DrawingHotkeys::default(),
            pens: vec![
                Pen {
                    color: "#000000FF".into(),
                    width: 4,
                    style: "solid".into(),
                },
                Pen {
                    color: "#FF000040".into(),
                    width: 30,
                    style: "solid".into(),
                },
                Pen {
                    color: "#00FF00FF".into(),
                    width: 4,
                    style: "dashed".into(),
                },
            ],
        }
    }
}

impl Default for ScreenshotSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            hotkey: default_hk_screenshot(),
            hotkey_pin: default_hk_screenshot_pin(),
            hotkey_pin_clipboard: default_hk_pin_clipboard(),
            hotkey_scroll: default_hk_scroll(),
            hotkey_scroll_wide: default_hk_scroll_wide(),
            pin_ring_cap: default_pin_ring_cap(),
        }
    }
}

impl Default for RecordSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            hotkey: default_hk_record(),
            region_hotkey: default_hk_record_region(),
            trim_hotkey: default_hk_record_trim(),
            fps: default_fps(),
            resolution: 0,
            hardware: 0,
            // 出厂码率档 = 8 Mbps（默认档下标 1，与 RECORD_BITRATE_LABELS 对齐）。
            bitrate: default_bitrate(),
            show_cursor: true,
            notify: true,
            // ★ 2026-09-26 晚：两路音频出厂各走「跟随系统默认」（开关开 + 设备名空），
            //   出厂不再是「关闭」——详见 `RecordSettings::record_mic` 的理由。
            record_mic: true,
            record_speaker: true,
            mic_denoise: default_mic_denoise(),
            mic_device: String::new(),
            speaker_device: String::new(),
        }
    }
}

/// GIF 动图录制（屏幕页顶层卡之二；原「录制」页，2026-09-25 重排）。
///
/// ★ 与录屏**共用一套采集**（`platform::recorder::Session`），只是换个 sink：录屏进
/// H264 编码器，GIF 进调色板量化 + LZW。所以"帧率 / 分辨率 / 录制鼠标"三项在两张卡里
/// 语义完全一致，只是量程不同（GIF 帧率上限低、尺寸档更小）。
///
/// ★ **GIF 快捷键本身也是一键开关**（2026-09-19 与录屏一并统一）：按一下开始、再按一下停止。
/// 原来那把"两卡共用的停止快捷键"已删除，理由见 [`RecordSettings`] 的文档注释。
///
/// ★ **选框（2026-09-20）**：录制区域改由快捷键后的选框决定，**不是配置项**——所以本结构体
/// 里没有 `region` 字段（`size` 因此退役，见下）。选框矩形在交互期存于 `Runtime::gif_sel`，
/// 录制时作为 `RecOptions::crop` 传给采集层（见 `platform::recorder`）。
///
/// ★ **裁剪（2026-09-20）**：裁剪窗既能裁"刚录的那一段"，也能作为**独立编辑器**打开
/// （`trim_hotkey`）去裁磁盘上任意一个 GIF；保存走「另存为」，原片只读。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GifSettings {
    /// 功能开关（面板「GIF录制」卡标题栏的选框）。关 = GIF / 裁剪两把快捷键都不响应。
    /// 2026-09-25 晚：出厂改未勾选（所有功能默认全关）。
    #[serde(default)]
    pub enabled: bool,
    /// GIF 录制快捷键（**开始 / 停止**一键开关）。空串 = 停用。
    #[serde(default = "default_hk_gif")]
    pub hotkey: String,
    /// 裁剪窗快捷键：**独立打开裁剪器**（不依赖刚录过什么）。空串 = 停用。
    ///
    /// ★ 与录制快捷键分开是刻意的：录制快捷键是"按下就开始框选录制"，而裁剪是编辑动作 ——
    ///   把编辑入口挂在录制键上，用户在不录的时候就没法裁磁盘上已有的 GIF 了。
    ///   与 `hotkey` 同时按下也只是各自触发各自的动作（两者语义不重叠）。
    #[serde(default = "default_hk_gif_trim")]
    pub trim_hotkey: String,
    /// 帧率。档位见 [`GIF_FPS_LABELS`]，出厂 15。
    #[serde(default = "default_gif_fps")]
    pub fps: u32,
    /// 输出尺寸档下标，指向 [`GIF_SIZE_LONG_EDGE`]，出厂 1（1280）。
    ///
    /// ★ **不再生效**（2026-09-20）：选框上线后输出尺寸由选框决定，面板上那一行已删。
    ///   保留字段只为让旧 `config.json` 读得进来（写了也不报错），实际生效的上限是
    ///   [`GIF_MAX_LONG_EDGE`]。
    #[serde(default = "default_gif_size")]
    pub size: u32,
    /// 是否把鼠标光标画进画面（与录屏同一个开关语义、各存各的值）。
    #[serde(default = "default_true")]
    pub show_cursor: bool,
    /// 是否无限循环播放。
    ///
    /// ★ **不再生效**（2026-09-20）：循环恒定开启（写 GIF 时 `loop_forever` 直接传
    ///   `true`，见 `platform::recorder`）。保留字段只为旧配置兼容。
    #[serde(default = "default_true")]
    pub loop_forever: bool,
}

fn default_gif_fps() -> u32 {
    15
}
fn default_gif_size() -> u32 {
    // 指向 DEFAULT_GIF_LONG_EDGE = 1280 那一档的下标。
    GIF_SIZE_LONG_EDGE
        .iter()
        .position(|&v| v == DEFAULT_GIF_LONG_EDGE)
        .unwrap_or(1) as u32
}

impl Default for GifSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            hotkey: default_hk_gif(),
            trim_hotkey: default_hk_gif_trim(),
            fps: default_gif_fps(),
            size: default_gif_size(),
            show_cursor: true,
            loop_forever: true,
        }
    }
}

impl Default for VirtualKeyboardSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            layout: default_vk_layout(),
            placement: default_placement(),
            top_offset: 0.0,
            bottom_offset: default_vk_bottom_offset(),
            left_offset: 0.0,
            right_offset: default_vk_right_offset(),
            key_size: default_vk_key_size(),
            gap: default_vk_gap(),
            opacity: default_opacity(),
            hold: default_vk_hold(),
            font_size: default_vk_font_size(),
            key_color: default_vk_key_color(),
            press_color: default_vk_press_color(),
            border_color: default_vk_border_color(),
            text_color: default_font_color(),
            corner_radius: default_vk_corner_radius(),
            // 与字段上的 `#[serde(default = "default_true")]` 必须一致：两处都是"出厂值"。
            click_through: true,
            scale: 1.0,
            params: BTreeMap::new(),
        }
    }
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            font_size: 16,
            // 空 = 全部卡片按 `default_card_expanded`（每页第一张展开）。
            // 不在这里播种 20 个默认值：那样只会让配置文件变脏。
            expanded: BTreeMap::new(),
            // 面板页码从「鼠标」起、位置未记录（首次打开居中）。
            panel_page: 0,
            panel_page_id: None,
            panel_pos: None,
        }
    }
}

// ─── 读写逻辑 ───

impl Config {
    /// 配置目录 = **程序目录**（便携化，见 `core::paths` 的模块说明）。
    ///
    /// 配置只认这一处；`%APPDATA%` 等任何外部位置一概不读不写。
    pub fn config_dir() -> PathBuf {
        crate::core::paths::data_dir()
    }

    pub fn config_path() -> PathBuf {
        crate::core::paths::config_path()
    }

    /// `main.rs` 启动分支专用的**轻量**读取：只关心 `general.admin_launch` 一个键。
    ///
    /// 刻意不走 `Config::load()`——那套迁移/落盘有副作用，而"要不要提权"的判定
    /// 必须发生在一切副作用之前（提权路径里本进程随即退出，落了盘也是白动）。
    /// 文件不存在 / 解析失败 / 键缺失一律 `false`（= 出厂默认：普通权限启动）。
    pub fn read_admin_launch_flag() -> bool {
        let path = Self::config_path();
        let Ok(s) = fs::read_to_string(&path) else {
            return false;
        };
        serde_json::from_str::<Value>(&s)
            .ok()
            .and_then(|v| v.get("general")?.get("admin_launch")?.as_bool())
            .unwrap_or(false)
    }

    /// 悬浮标是否有实际在显示的内容（帧门判据，2026-09-26 多开改造）。
    ///
    /// 总闸开着但**一个启用脚本都没有** = 空转，不该让覆盖层保持唤醒。
    /// （旧版单选语义下"总闸开 = 必有 hover_type"，不存在这个中间态。）
    pub fn hover_running(&self) -> bool {
        self.trail.hover_enabled && !self.trail.hover_active_list().is_empty()
    }

    /// 鼠标特效模块是否有任一功能在用。
    ///
    /// ★ 开关架构（2.0 起）：鼠标特效页四张卡片（拖尾效果/悬浮标/点击效果/点击声音）
    /// 各自标题栏的开关就是唯一开关，运行时的帧门/输入门/休眠判定一律以本判据为准。
    /// 历史上这里只看 `global.enabled_trail`，导致悬浮标等三张卡片的开关是「死开关」
    /// （开了没反应，覆盖层始终休眠）。
    /// ★★ 2026-09-25：波纹/声音加了总开关（AND 闸），这里的贡献也要跟着乘上——
    /// 总开关关着时，即使某键 enabled=true 也不该让覆盖层保持唤醒。
    pub fn trail_module_active(&self) -> bool {
        self.trail_visual_active()
            // ★ 复合脚本（2026-09-26）：宿主住在 TrailModule 里，运行列表非空 = 模块
            //   有活干。漏了这条 = 其它功能全关时只开复合脚本，覆盖层 park、帧门与
            //   鼠标输入门全关（桥接线程把 MouseMove 整条丢弃）→「勾了没反应」。
            //   ★★ 只按"列表非空"给的是**配置侧**的粗判据：运行期还得问一句这些脚本
            //   到底要不要覆盖层的帧（见 `runtime::frame` 的 `composite_needs_frame`）。
            || self.lua_ext.composite_running()
    }

    /// 只算**四张视觉卡片**（拖尾 / 悬浮标 / 点击效果 / 点击声音），不含复合脚本。
    ///
    /// ★ 拆分的原因（2026-09-30 内存归因）：复合脚本里有一类是**一次性 / 开窗脚本**
    /// （如 `scripts/AI/ai_chat`，只声明 `schema()` + `run()`，不碰覆盖层）。它们进了
    /// `composite_active` 就会把 [`Self::trail_module_active`] 顶成 true —— 覆盖层
    /// 于是**整屏缓冲常驻 + 60fps 整屏重绘**（1920×1080 实测私有工作集 +8.8 MB），
    /// 而那条链路上一个像素都没画。
    ///
    /// 所以帧门 / 输入门改用「本判据 ‖ 运行期确认过"真要用帧"的复合脚本」；本函数
    /// 保留给只拿得到配置的场合（状态快照等），语义不变。
    pub fn trail_visual_active(&self) -> bool {
        self.trail.trail_enabled
            || self.hover_running()
            || (self.trail.ripple_enabled
                && self.trail.ripple_bindings.values().any(|b| b.enabled))
            || (self.trail.sound_enabled
                && self.trail.sound_bindings.values().any(|s| s.enabled))
            // 迁移期的老字段也要看：老配置在第一次落盘前只有 sound_left/right_enabled。
            || self.trail.sound_left_enabled
            || self.trail.sound_right_enabled
    }

    /// 老配置 → 每键波纹绑定的一次性迁移：ripple_* 旧字段 → ripple_bindings。
    /// 只在 `ripple_bindings` 为空时执行（幂等，不覆盖用户已调好的参数）。
    /// 颜色归一化为 6 位 `#RRGGBB`（与面板颜色选择器写回格式一致，避免首开面板伪落盘）。
    ///
    /// 缺失的键用 [`seeded_ripple_bindings`] 里的条目补（**不是** `Default::default()`）：
    /// 出厂条目已带 `script: "ring"`，这样迁移完的配置就是面板 `apply_to` 的固定点，
    /// 第一次开面板不会因 `script: "" → "ring"` 而伪落盘。
    pub fn migrate_ripple_bindings(&mut self) {
        if !self.trail.ripple_bindings.is_empty() {
            return;
        }
        // §39/§40：波纹颜色出厂统一 #FF0000，不再继承 1.0 的 ripple_*_color（黄/蓝）——
        // 脚本颜色参数由各 lua 的 schema 默认接管，此字段仅剩 Rust Ripple 兜底外观用。
        let size = self.trail.ripple_size;
        let duration = self.trail.ripple_duration;
        let mut seeded = seeded_ripple_bindings();
        let mut left = seeded.remove("left").unwrap_or_default();
        left.enabled = self.trail.ripple_left_enabled;
        left.size = size;
        left.duration = duration;
        left.color = "#FF0000".into();
        self.trail.ripple_bindings.insert("left".to_string(), left);
        let mut right = seeded.remove("right").unwrap_or_default();
        right.enabled = self.trail.ripple_right_enabled;
        right.size = size;
        right.duration = duration;
        right.color = "#FF0000".into();
        self.trail
            .ripple_bindings
            .insert("right".to_string(), right);
        // 中键/侧键：老配置无对应字段，出厂默认关闭（防误触）。
        for key in ["middle", "x1", "x2"] {
            self.trail
                .ripple_bindings
                .entry(key.to_string())
                .or_insert_with(|| seeded.remove(key).unwrap_or_default());
        }
    }

    /// ★ §40 一次性清洗：清空全部波纹绑定的 `script_params`（见字段注释）。
    /// 幂等靠 `ripple_params_reset` 标记：只执行一次，之后用户调参照常持久化。
    pub fn reset_ripple_script_params(&mut self) {
        if self.trail.ripple_params_reset {
            return;
        }
        for b in self.trail.ripple_bindings.values_mut() {
            b.script_params.clear();
        }
        self.trail.ripple_params_reset = true;
    }

    /// 老配置 → 每键声音绑定的一次性迁移：`sound_left_*` / `sound_right_*` / 全局
    /// `sound_volume` → `sound_bindings`。
    ///
    /// 只在 `sound_bindings` 为空时执行（幂等，不覆盖用户已调好的参数）。老配置里只有
    /// 左右两键，中键 / 侧键按出厂默认补上（关闭 + 预置声音）。
    pub fn migrate_sound_bindings(&mut self) {
        if !self.trail.sound_bindings.is_empty() {
            return;
        }
        let volume = self.trail.sound_volume;
        let mut seeded = seeded_sound_bindings();
        for (key, enabled, file) in [
            (
                "left",
                self.trail.sound_left_enabled,
                &self.trail.sound_left_file,
            ),
            (
                "right",
                self.trail.sound_right_enabled,
                &self.trail.sound_right_file,
            ),
        ] {
            let entry = seeded.entry(key.to_string()).or_default();
            entry.enabled = enabled;
            entry.volume = volume;
            if !file.is_empty() {
                entry.file = file.clone();
            }
        }
        // 中键 / 侧键：老配置没有对应字段，只继承音量。
        for key in ["middle", "x1", "x2"] {
            seeded.entry(key.to_string()).or_default().volume = volume;
        }
        self.trail.sound_bindings = seeded;
    }

    /// 老配置 → 新悬浮标架构的一次性迁移：cursor_* 旧字段 → hover_enabled/hover_type/
    /// hover_params。只在 `hover_params` 为空时执行（幂等，不覆盖用户已调好的参数）。
    ///
    /// 迁移规则（对齐用户要求的三套参数）：
    /// - cursor_mode：0 → 关；1 → solid；2 → ring；3 → png（hover_enabled 同步）
    /// - solid/ring：颜色=旧颜色**含 alpha 折成 8 位 #RRGGBBAA**（2026-10-09 修正：
    ///   现行 schema 的透明度在颜色里，独立的 opacity 参数已删除——旧迁移播种
    ///   6 位色 + opacity 键，脚本两者都不读，`hex_to_rgb` 对 6 位色取 a=255，
    ///   于是每次启动都"复活"一个不透明的 #FF0000 实心圆）
    /// - png：缩放=1.0（旧迁移写的 size 键 schema 里根本没有）、透明度沿用
    pub fn migrate_hover_settings(&mut self) {
        if !self.trail.hover_params.is_empty() {
            return;
        }
        let (opacity, color8) = match crate::core::color::Argb::parse_hex(&self.trail.cursor_color)
        {
            Some(c) => {
                let op = (c.a() as f32 / 255.0 * 100.0).round() / 100.0;
                // 8 位 #RRGGBBAA：透明度随颜色一起走，这是现行脚本的唯一透明度来源。
                (
                    op,
                    format!("#{:02X}{:02X}{:02X}{:02X}", c.r(), c.g(), c.b(), c.a()),
                )
            }
            None => (0.5, "#FF000080".to_string()),
        };
        let outer = self.trail.cursor_outer.clamp(4, 1000);
        let arc = self.trail.cursor_arc.clamp(1, 999);
        let solid = HashMap::from([
            ("diameter".to_string(), serde_json::json!(outer)),
            ("color".to_string(), serde_json::json!(color8)),
        ]);
        let ring = HashMap::from([
            ("outer".to_string(), serde_json::json!(outer)),
            ("arc".to_string(), serde_json::json!(arc)),
            ("color".to_string(), serde_json::json!(color8)),
        ]);
        let png = HashMap::from([
            ("scale".to_string(), serde_json::json!(1.0)),
            ("opacity".to_string(), serde_json::json!(opacity)),
        ]);
        self.trail.hover_params = HashMap::from([
            ("solid".to_string(), solid),
            ("ring".to_string(), ring),
            ("png".to_string(), png),
        ]);
        // ★ cursor_mode 只在**非零**（老配置真的开了悬浮标）时才接管开关与类型。
        //   新架构落盘的配置里 cursor_mode 是不再更新的遗留快照（UI 从不写它，
        //   见 model.rs 的迁移注释），拿它覆盖 hover_enabled 会把用户开着的悬浮标
        //   在每次启动时静默关掉（磁盘 true → 内存 false，日志里"enabled=false"）。
        match self.trail.cursor_mode {
            3 => {
                self.trail.hover_type = "png".to_string();
                self.trail.hover_enabled = true;
            }
            2 => {
                self.trail.hover_type = "ring".to_string();
                self.trail.hover_enabled = true;
            }
            1 => {
                self.trail.hover_type = "solid".to_string();
                self.trail.hover_enabled = true;
            }
            _ => {}
        }
    }

    /// 虚拟键盘方案迁移。
    ///
    /// 1. 旧配置里方案名 `"default"`（改名前的默认方案）一律 → `"ansi_104_dark"`：该方案只是
    ///    改了配色的纯样式方案，改名后继续由 `BUILTIN` 内嵌兜底。
    /// 2. 只保留**当前所选方案**的一整套控件值：老配置若累积了多套（每个用过的方案都记了
    ///    一组），这里把非当前方案的键全部清掉 —— 配置只留"哪种方案 + 一整套该方案的配置"，
    ///    不出现字段爆炸（见请求 2）。落盘端 [`PanelModel::apply_to`] 也只会写当前方案那一个键。
    pub fn migrate_vk_layout(&mut self) {
        // 2026-10-08：四套外来样式方案按「几何前缀_样式」规则改名（文件名即方案 id），
        // 旧配置里的旧方案名在这里迁到新名（新名文件都在 assets 里，迁移后直接可用）。
        if let Some(new) = match self.virtual_keyboard.layout.as_str() {
            "default" => Some("ansi_104_dark"),
            "kiosk_dark" => Some("ansi_104_kiosk_dark"),
            "kb_double" => Some("ansi_104_kb_double"),
            "vhand_dark" => Some("ansi_104_vhand_dark"),
            "zh_light" => Some("ansi_104_zh_light"),
            _ => None,
        } {
            self.virtual_keyboard.layout = new.to_string();
        }
        let cur = self.virtual_keyboard.layout.clone();
        self.virtual_keyboard.params.retain(|k, _| k == &cur);
    }

    /// 降噪配置迁移：把**旧的「模式 + 强度」两档**并成新的**单一下拉**。
    ///
    /// 旧配置里降噪是 `mic_denoise`（`off`/`gate`/`spectral`/`rnnoise`）+ `mic_denoise_level`
    /// （0=弱 1=中 2=强）两个字段。新配置只有一个 `mic_denoise`，取值
    /// `off`/`weak`/`medium`/`strong`。这里把遗留模式规整成 RNNoise 中等强度：
    ///
    /// - `off` → `off`（原样）。
    /// - `spectral` / `rnnoise` → `medium`（RNNoise 默认强度；旧强度字段已并入取值，这里不再
    ///   区分，避免引入一个只在迁移时用的临时字段）。
    /// - `gate` → `medium`：噪声门已被删除，其「切掉说话间隙」的能力本就被 RNNoise 覆盖，
    ///   直接并入中等强度最贴近用户原意（想降噪，而不是想完全关掉）。
    /// - 其它（手滑写错、未来未知值）→ `off`，安全兜底。
    ///
    /// 删除的 `noise_gate` 字段在反序列化时被忽略（本结构体没开 `deny_unknown_fields`），
    /// 不会让旧配置崩。
    pub fn migrate_denoise(&mut self) {
        let s = std::mem::take(&mut self.record.mic_denoise);
        self.record.mic_denoise = match s.as_str() {
            "off" | "weak" | "medium" | "strong" => s,
            "spectral" | "rnnoise" | "gate" => "medium".to_string(),
            _ => "off".to_string(),
        };
    }

    /// 录屏分辨率档**显示顺序**调整的一次性迁移（2026-09-30）：
    /// 旧顺序 `原始 / 1080p / 720p / 1440p / 2160p`（下标 0/1/2/3/4）→
    /// 新顺序 `原始 / 720p / 1080p / 1440p / 2160p`。
    ///
    /// `record.resolution` 存的是下标，且新顺序里 1440p / 2160p 的下标（3/4）与旧顺序一致，
    /// 只有中间的 `1` 与 `2` 互换（1080p ↔ 720p）。所以映射只动这两档：
    ///
    /// - 旧 `1`（1080p）→ 新 `2`（仍是 1080p）
    /// - 旧 `2`（720p）→ 新 `1`（仍是 720p）
    /// - 旧 `0 / 3 / 4` 不变；越界值夹回 `<5`。
    ///
    /// ★ 幂等靠 [`Config::resolution_order_v2`] 标记：迁移过就置位、立刻落盘，下次启动
    /// 不再映射。否则"已迁移的 `2`（1080p）"会被再次映射成 `1`（720p），用户改回 720p 又变 1080p。
    pub fn migrate_resolution_order(&mut self) {
        let old = self.record.resolution;
        let new = match old {
            1 => 2,
            2 => 1,
            other => other,
        };
        self.record.resolution = new.min(RECORD_RESOLUTION_LABELS.len() as u32 - 1);
    }

    /// Lua 出厂脚本 id 的一次性迁移（2026-09-26）：脚本最终归档在 **`assets/lua/scripts/`**
    /// （通用扩展脚本统一放这一个文件夹，id = `scripts/<英文文件名>`）。当天先后迭代过
    /// 四版形态（裸中文名、中文功能文件夹、`script/<名>/<名>` 双层、根目录平铺），
    /// 四者的存量 id 都要能迁移到终态。
    ///
    /// 配置里按 id 记的东西有三处：击键音效当前脚本 [`KeySoundSettings::script`]、
    /// 击键音效每脚本参数 `key_sound.params[id]`、扩展脚本每脚本参数 `lua_ext.params[id]`。
    /// 三处的旧 id 一并改成新 id，幂等（改名后旧键不复存在，下次调用零改动）。
    /// 只认**出厂脚本**的旧名；用户自己放的自定义脚本 id 不动（没有标准答案）。
    pub fn migrate_lua_script_folders(&mut self) {
        /// (旧 id, 新 id) —— 出厂脚本 2026-09-26 归档对照表（终态 = `scripts/<名>`）。
        const RENAMES: [(&str, &str); 18] = [
            // 出厂最初的裸中文名。
            ("统一音效", "KeySound/unified_sound"),
            ("分类音效", "KeySound/class_sound"),
            ("图片体检", "scripts/image_report"),
            ("平均颜色", "scripts/avg_color"),
            // 当天中间版①：中文功能文件夹。
            ("击键音效/统一音效", "KeySound/unified_sound"),
            ("击键音效/分类音效", "KeySound/class_sound"),
            ("脚本/图片体检", "scripts/image_report"),
            ("脚本/平均颜色", "scripts/avg_color"),
            // 当天中间版②：script/ 双层英文目录。
            (
                "script/unified_sound/unified_sound",
                "KeySound/unified_sound",
            ),
            ("script/class_sound/class_sound", "KeySound/class_sound"),
            ("script/image_info/image_info", "scripts/image_report"),
            ("script/avg_color/avg_color", "scripts/avg_color"),
            // 当天中间版③：根目录平铺裸名。
            ("unified_sound", "KeySound/unified_sound"),
            ("class_sound", "KeySound/class_sound"),
            // 2026-10-30 从通用 scripts/ 迁到键音专属目录；旧终态 id 也要接力。
            ("scripts/unified_sound", "KeySound/unified_sound"),
            ("scripts/class_sound", "KeySound/class_sound"),
            ("image_info", "scripts/image_report"),
            ("avg_color", "scripts/avg_color"),
        ];
        let map = |id: &str| {
            RENAMES
                .iter()
                .find(|(old, _)| *old == id)
                .map(|(_, new)| new.to_string())
                .unwrap_or_else(|| id.to_string())
        };
        let mut n = 0usize;
        let old_script = self.key_sound.script.clone();
        let new_script = map(&old_script);
        if new_script != old_script {
            self.key_sound.script = new_script;
            n += 1;
        }
        // params 的键是脚本 id：旧裸名键整段搬到新名下（内容原样保留）。
        let remap = |params: &mut HashMap<String, HashMap<String, Value>>, n: &mut usize| {
            for (old, new) in RENAMES {
                if let Some(v) = params.remove(old) {
                    params.entry(new.to_string()).or_insert(v);
                    *n += 1;
                }
            }
        };
        remap(&mut self.key_sound.params, &mut n);
        remap(&mut self.lua_ext.params, &mut n);
        if n > 0 {
            log::info!(
                "Lua 出厂脚本已定稿归档到 assets/lua/scripts/，配置里的 {n} 处脚本 id 已同步改写"
            );
        }
    }

    /// 拆开「截图」与「截图并贴图」的**撞弦**（2026-09-23）。
    ///
    /// 见 `runtime::resolve_snap_hotkey`：判定是"本体优先"，所以当 `hotkey`
    /// 恰好等于 `with_ctrl(hotkey_pin)` 时，"截图并贴图"的 Ctrl 变体被截图本体整条抢走
    /// —— 用户怎么按都落到"纯截图"，而他想要的编辑态一次都拿不到。这种组合不可能是本意，
    /// 一律把截图键清空停用（并记日志），让「截图并贴图」的两条路都通。
    ///
    /// ★ 2026-09-26 起出厂快捷键全留空，出厂配置不再可能撞弦；这里只处理**用户自己配出来**
    /// 的撞弦（典型病例：`hotkey = Ctrl+F3` + `hotkey_pin = F3`）。还原目标也顺势从
    /// "出厂键位"改成"清空停用"——出厂已无键位可还原，且替用户硬塞一把键同样是不打招呼的改动。
    pub fn migrate_snap_hotkey_shadowing(&mut self) {
        let shot = self.screenshot.hotkey.trim().to_string();
        if shot.is_empty() {
            return;
        }
        let pin_variant = crate::core::hotkeys::with_ctrl(&self.screenshot.hotkey_pin);
        if pin_variant.is_empty() {
            // 「截图并贴图」自己就含 Ctrl（或为空）→ 它本来就没有变体，谈不上被抢。
            return;
        }
        if crate::core::hotkeys::normalize(&pin_variant) != crate::core::hotkeys::normalize(&shot) {
            return;
        }
        let fallback = String::new();
        log::warn!(
            "截图键 {shot} 正好是「截图后贴图」({}) 的 Ctrl 变体，两者撞弦 —— \
             「截图后贴图」永远进不了编辑态；已停用截图键（清空），需要的话请改一把别的键",
            self.screenshot.hotkey_pin
        );
        self.screenshot.hotkey = fallback;
    }

    /// ★ 全局热键的**裸键清洗**（2026-10-01）：加载后把不合 [`global_safe`] 的全局键位
    ///   清空停用并留 warn。捕获路径（面板）已经拦住新值，这里兜住**手改 config.json**
    ///   的存量——用户拿记事本把截图键写成 `A`，重启后那把键必须失效而不是"打字就截图"。
    ///
    /// 标注态工具键（`drawing.hotkeys` 里的 `1`..`9`/`0`）**不清洗**——它们只在标注
    /// 开着时生效，裸键是设计意图；两把 toggle（`draw_hk_screen`/`draw_hk_white`）
    /// 标注没开也要能按，属于全局语义，照清。脚本热键参数同清（与面板捕获同一判据）。
    ///
    /// 与 [`Self::migrate_snap_hotkey_shadowing`] 同一套纪律：无条件、幂等、清空即停用
    /// （不给用户硬塞新键位），每次启动都跑。
    pub fn sanitize_global_hotkeys(&mut self) {
        let fields: [(&'static str, &mut String); 15] = [
            ("截图", &mut self.screenshot.hotkey),
            ("截图后贴图", &mut self.screenshot.hotkey_pin),
            ("贴图", &mut self.screenshot.hotkey_pin_clipboard),
            ("长截图", &mut self.screenshot.hotkey_scroll),
            ("宽截图", &mut self.screenshot.hotkey_scroll_wide),
            ("视频录制", &mut self.record.hotkey),
            ("选区录屏", &mut self.record.region_hotkey),
            ("视频裁剪", &mut self.record.trim_hotkey),
            ("GIF 录制", &mut self.gif.hotkey),
            ("GIF 裁剪", &mut self.gif.trim_hotkey),
            ("剪贴板", &mut self.clipboard.hotkey),
            ("便签", &mut self.sticky_note.hotkey),
            ("计时器", &mut self.global.timer.hotkey),
            ("全屏放大", &mut self.magnifier_fullscreen.hotkey),
            ("画中画放大", &mut self.magnifier_pip.hotkey),
        ];
        for (name, field) in fields {
            let spec = field.trim();
            if !spec.is_empty() && !crate::core::hotkeys::global_safe(spec) {
                log::warn!(
                    "{name} 的快捷键 {spec} 是无修饰键的裸键，全局生效会在打字时每次按下\
                     都误触发 —— 已清空停用，请在面板里重配（加 Ctrl/Alt/Shift/Win，或用 \
                     F 键区/PrintScreen 等功能键）"
                );
                *field = String::new();
            }
        }
        // 面板键 + 标注两把 toggle（路径不同，无法进上面的表）。
        let panel = self.global.panel_hotkey.trim().to_string();
        if !panel.is_empty() && !crate::core::hotkeys::global_safe(&panel) {
            log::warn!(
                "面板快捷键 {panel} 是无修饰键的裸键，全局生效会在打字时每次按下都误触发 \
                 —— 已清空停用，请在面板里重配"
            );
            self.global.panel_hotkey = String::new();
        }
        for (name, spec) in [
            ("屏幕标注开关", &mut self.drawing.hotkeys.draw_hk_screen),
            ("白板开关", &mut self.drawing.hotkeys.draw_hk_white),
        ] {
            let t = spec.trim().to_string();
            if !t.is_empty() && !crate::core::hotkeys::global_safe(&t) {
                log::warn!(
                    "{name}的快捷键 {t} 是无修饰键的裸键，全局生效会在打字时每次按下\
                     都误触发 —— 已清空停用，请在面板里重配"
                );
                *spec = String::new();
            }
        }
        // 脚本热键参数：`params[scripts/<id>]["hotkey"]`。
        for (id, params) in self.lua_ext.params.iter_mut() {
            if !id.starts_with("scripts/") {
                continue;
            }
            let Some(v) = params.get_mut("hotkey") else {
                continue;
            };
            let spec = v.as_str().unwrap_or("").trim().to_string();
            if !spec.is_empty() && !crate::core::hotkeys::global_safe(&spec) {
                log::warn!(
                    "脚本 {id} 的快捷键 {spec} 是无修饰键的裸键，全局生效会在打字时每次按下\
                     都误触发 —— 已清空停用，请在脚本卡里重配"
                );
                params.insert("hotkey".into(), serde_json::Value::String(String::new()));
            }
        }
    }

    /// 「截图 / 截图并贴图」里**规格已含 Ctrl** 的那几把（返回给日志用的中文键名）。
    ///
    /// 含 Ctrl ⇒ [`crate::core::hotkeys::with_ctrl`] 拼不出变体 ⇒ 这把键**没有**
    /// "按住 Ctrl 进编辑态"这一路。函数本身不修正（怎么改键是用户的事），只负责让这件事
    /// **不再静默** —— 从前它唯一的症状是"按住 Ctrl 没反应"，用户只能把它归因成
    /// "这功能没做"，然后绕道去改快捷键（把自己绕进 [`Self::migrate_snap_hotkey_shadowing`]
    /// 那种撞弦里）。
    pub fn snap_keys_missing_ctrl_variant(&self) -> Vec<&'static str> {
        [
            ("截图", &self.screenshot.hotkey),
            ("截图后贴图", &self.screenshot.hotkey_pin),
        ]
        .into_iter()
        .filter(|(_, spec)| {
            !spec.trim().is_empty() && crate::core::hotkeys::with_ctrl(spec).is_empty()
        })
        .map(|(name, _)| name)
        .collect()
    }

    /// 读盘。文件不存在 → 用默认值并立刻写盘。
    /// 解析失败 → 把坏文件重命名为 `.bak-<时间戳>`，用默认值。
    /// 老口径 `#AARRGGBB` → 新口径 `#RRGGBBAA`（首字节的 alpha 移到末尾）。
    ///
    /// 只接受 8 位纯 hex；6 位 `#RRGGBB` 在两种口径下等价，返回 None（不改动）。
    fn rotate_color_hex(s: &str) -> Option<String> {
        let h = s.strip_prefix('#')?;
        if h.len() != 8 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        Some(format!("#{}{}", &h[2..], &h[..2]))
    }

    /// 键名是否表示一个颜色值。
    ///
    /// 判据是"含 color"而不是精确名单：`script_params` 里由脚本自己声明的颜色参数
    /// （`color` / `fill_color` / `border_color`…）也要一起迁，写死名单必漏。
    fn is_color_key(k: &str) -> bool {
        k.to_ascii_lowercase().contains("color")
    }

    /// 递归轮转 JSON 里的颜色串，返回改动处数。
    fn migrate_color_hex_in_value(v: &mut Value, n: &mut usize) {
        match v {
            Value::Object(m) => {
                for (k, val) in m.iter_mut() {
                    if Self::is_color_key(k) {
                        if let Some(t) = val.as_str() {
                            if let Some(fixed) = Self::rotate_color_hex(t) {
                                *val = Value::String(fixed);
                                *n += 1;
                                continue;
                            }
                        }
                    }
                    Self::migrate_color_hex_in_value(val, n);
                }
            }
            Value::Array(a) => {
                for val in a.iter_mut() {
                    Self::migrate_color_hex_in_value(val, n);
                }
            }
            _ => {}
        }
    }

    pub fn load() -> Result<Config> {
        let path = Self::config_path();
        if !path.exists() {
            // 新建的配置本就是新口径：标记直接置位，免得下次 load 又把它轮转一遍。
            let cfg = Config {
                color_hex_v2: true,
                resolution_order_v2: true,
                ..Config::default()
            };
            cfg.save()?;
            return Ok(cfg);
        }
        match fs::read_to_string(&path) {
            Ok(s) => match serde_json::from_str::<Value>(&s) {
                Ok(mut v) => {
                    // ★ 一次性迁移：8 位颜色串 #AARRGGBB → #RRGGBBAA。
                    //   在 **JSON 层**做，只动文件里真实存在的键 —— serde 为缺字段补的
                    //   出厂默认值（已是新口径）不会经过这里，不会被误轮转。
                    let migrated = if v.get("color_hex_v2").and_then(Value::as_bool) == Some(true) {
                        false
                    } else {
                        let mut n = 0usize;
                        Self::migrate_color_hex_in_value(&mut v, &mut n);
                        if let Some(obj) = v.as_object_mut() {
                            obj.insert("color_hex_v2".into(), Value::Bool(true));
                        }
                        if n > 0 {
                            log::warn!(
                                "config.json 里的 8 位颜色串已按 #RRGGBBAA 重排（{n} 处）：\
                             旧口径 #AARRGGBB 会让颜色悄悄变错（如 #FF000040 被读成深蓝）"
                            );
                        }
                        n > 0
                    };
                    match serde_json::from_value::<Config>(v) {
                        Ok(mut cfg) => {
                            // 老配置迁移：cursor_* → 悬浮标；ripple_* → 每键波纹；sound_* → 每键声音
                            cfg.migrate_hover_settings();
                            cfg.migrate_ripple_bindings();
                            cfg.migrate_sound_bindings();
                            cfg.migrate_vk_layout();
                            cfg.migrate_denoise();
                            // §脚本归档（2026-09-26）：出厂脚本搬进功能子文件夹，旧裸名 id 改写。
                            cfg.migrate_lua_script_folders();
                            // §40：波纹脚本参数一次性重置（清除历史播种的毒值，见字段注释）。
                            cfg.reset_ripple_script_params();
                            // ★ 截图两把键的"按住 Ctrl 进编辑态"这一路：撞弦的拆开，没有变体的报一句。
                            //   两者都只影响这一个效果，症状又恰好是"没反应"，不在这里说一声，
                            //   就再也没有别的地方会说了。
                            cfg.migrate_snap_hotkey_shadowing();
                            // ★ 全局热键的裸键清洗（2026-10-01）：手改 config.json 塞进来的
                            //   裸键在这里兜住（面板捕获路径另有拦截，见 try_capture_hotkey）。
                            cfg.sanitize_global_hotkeys();
                            for name in cfg.snap_keys_missing_ctrl_variant() {
                                log::warn!(
                                    "{name} 的规格里已含 Ctrl，拼不出 Ctrl 变体 —— \
                                 这把键进不了截图编辑态（按住 Ctrl 不会多出任何效果）。\
                                 想用它编辑，换一个不含 Ctrl 的键即可（如出厂值）"
                                );
                            }
                            // ★ 迁移过就必须立刻落盘：幂等靠文件里的标记（color_hex_v2 /
                            //   resolution_order_v2）。分辨率档顺序调整（2026-09-30）只在老配置
                            //   （无 `resolution_order_v2` 标记）上做一次，避免把新口径的 `1`(720p)
                            //   误映射成 `2`(1080p)。
                            let mut changed = migrated;
                            if !cfg.resolution_order_v2 {
                                cfg.migrate_resolution_order();
                                cfg.resolution_order_v2 = true;
                                changed = true;
                            }
                            if changed {
                                let _ = cfg.save();
                            }
                            Ok(cfg)
                        }
                        Err(e) => {
                            log::error!("config.json 解析失败: {e}");
                            let ts = chrono::Local::now().format("%Y%m%d%H%M%S");
                            let bak = path.with_extension(format!("json.bak-{ts}"));
                            let _ = fs::rename(&path, &bak);
                            log::info!("已备份坏文件到 {}", bak.display());
                            // ★ 兜底用的出厂配置本就是新口径：标记必须置位并立刻落盘，
                            //   否则这份默认值（含 7 处 8 位色）下次 load 会被当旧口径轮转。
                            let cfg = Config {
                                color_hex_v2: true,
                                ..Config::default()
                            };
                            let _ = cfg.save();
                            Ok(cfg)
                        }
                    }
                }
                Err(e) => {
                    log::error!("config.json 不是合法 JSON: {e}");
                    let ts = chrono::Local::now().format("%Y%m%d%H%M%S");
                    let bak = path.with_extension(format!("json.bak-{ts}"));
                    let _ = fs::rename(&path, &bak);
                    log::info!("已备份坏文件到 {}", bak.display());
                    // 同上：出厂兜底配置必须带新口径标记（见上一段注释）。
                    let cfg = Config {
                        color_hex_v2: true,
                        ..Config::default()
                    };
                    let _ = cfg.save();
                    Ok(cfg)
                }
            },
            // ★ 带上路径与操作名：`From<io::Error>` 的自动转换在这里会把"读的是
            //   config.json"这件事抹掉，日志里只剩 `io error: ...`。
            Err(e) => Err(CurSeenError::io_at(&path, "read", e)),
        }
    }

    /// 写盘。**耐久原子写**：临时文件 → 文件 fsync → 原子替换 → 父目录 fsync。
    ///
    /// 四步缺一不可（总纲 §7.2）：
    ///   ① 写临时文件——直接覆写的话，写到一半断电就留下一个 0 字节的 `config.json`；
    ///   ② **文件 fsync**——`rename` 只保证"目录项原子切换"，不保证数据已落盘，
    ///      少了这步断电后可能读到新名字 + 旧内容/半截内容；
    ///   ③ `rename` 覆盖（同分区原子替换，Windows 上即 `MoveFileEx(REPLACE_EXISTING)`）；
    ///   ④ **父目录 fsync**——让 ③ 这个目录项本身也落盘。
    ///
    /// ★ 第 ④ 步由 [`set_dir_sync`] 注入：`core` 不得碰 Win32（分层硬规则），而目录
    ///   fsync 必须 `CreateFileW` + `GENERIC_WRITE`。**未注册时跳过**——目录项丢了
    ///   只是"回到上一版配置"，而第 ①② 步保证了它永远是**完整的一版**，不会损坏。
    pub fn save(&self) -> Result<()> {
        let dir = Self::config_dir();
        let path = Self::config_path();
        let json = serde_json::to_string_pretty(self)?;
        Self::write_durable(&dir, &path, json.as_bytes())
    }

    /// 耐久原子写（四步见 [`Config::save`] 的文档）。
    ///
    /// 抽成独立函数是为了**可测**：`Config::save()` 的路径由 `core::paths` 决定
    /// （exe 相对，测试里改不动，也不该在测试里真写一次用户配置），而这条序列
    /// 必须在临时目录里被真的跑一遍。
    fn write_durable(dir: &Path, path: &Path, bytes: &[u8]) -> Result<()> {
        fs::create_dir_all(dir).map_err(|e| CurSeenError::io_at(dir, "create_dir_all", e))?;
        let tmp = path.with_extension("json.tmp");
        // ① + ②：同一个句柄写完立刻 fsync（`File::sync_all` 需要写权限，
        //    所以这里用 `OpenOptions::write` 而不是 `fs::write`）。
        {
            use std::io::Write;
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&tmp)
                .map_err(|e| CurSeenError::io_at(&tmp, "write", e))?;
            f.write_all(bytes)
                .map_err(|e| CurSeenError::io_at(&tmp, "write", e))?;
            f.sync_all()
                .map_err(|e| CurSeenError::io_at(&tmp, "fsync", e))?;
        }
        // ③
        fs::rename(&tmp, path).map_err(|e| CurSeenError::io_at(path, "rename", e))?;
        // ④：失败只告警——"锦上添花的目录 fsync"绝不许反过来毁掉整条保存路径。
        if let Some(sync) = DIR_SYNC.get() {
            if !sync(dir) {
                log::warn!("父目录 fsync 失败（{}）：本次保存可能不耐久", dir.display());
            }
        }
        Ok(())
    }

    /// 深路径覆值 + 立刻保存。
    pub fn set_and_save(&mut self, path: &[&str], value: serde_json::Value) -> Result<()> {
        // 空路径会让下面 `path.len() - 1` 在 usize 下溢 panic；显式挡掉。
        if path.is_empty() {
            return Err(CurSeenError::Config("set_and_save: empty path".into()));
        }
        let mut current = serde_json::to_value(&*self)?;
        let mut obj = current.as_object_mut().ok_or_else(|| {
            CurSeenError::Config("set_and_save: config root is not object".into())
        })?;
        let last = path.len() - 1;
        for key in &path[..last] {
            obj = obj
                .entry(*key)
                .or_insert_with(|| serde_json::json!({}))
                .as_object_mut()
                .ok_or_else(|| {
                    CurSeenError::Config(format!("set_and_save: {key} is not object"))
                })?;
        }
        obj.insert(path[last].to_string(), value);
        *self = serde_json::from_value(current)?;
        self.save()
    }

    /// 恢复默认。
    pub fn reset_to_defaults() -> Result<Config> {
        let path = Self::config_path();
        if path.exists() {
            fs::remove_file(&path)?;
        }
        let cfg = Config::default();
        cfg.save()?;
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ★ 8 位颜色串的口径迁移：`#AARRGGBB` → `#RRGGBBAA`。
    ///
    /// 2026-09-25：全项目统一成"alpha 在末尾"（与 windui 取色器、Lua 脚本、
    /// `[R,G,B,A]` 数组一致）。旧串不迁的话颜色会静默变错，所以迁移本身要有锁。
    #[test]
    fn color_hex_rotation_moves_alpha_to_the_end() {
        // AARRGGBB 的"红 + alpha 0x40" → RRGGBBAA
        assert_eq!(
            Config::rotate_color_hex("#40FF0000"),
            Some("#FF000040".into())
        );
        // 不透明黑
        assert_eq!(
            Config::rotate_color_hex("#FF000000"),
            Some("#000000FF".into())
        );
        // 6 位不变（两种口径等价）
        assert_eq!(Config::rotate_color_hex("#FF0000"), None);
        // 非颜色串（下拉里 `color_mode` 之类）不动
        assert_eq!(Config::rotate_color_hex("rainbow"), None);
        // 长度不合法
        assert_eq!(Config::rotate_color_hex("#FF00000"), None);
    }

    /// ★ 迁移是**一次性**的：跑第二遍必须什么都不动（否则颜色越改越错）。
    ///
    /// 幂等靠文件里的 `color_hex_v2` 标记；这条锁住的是"标记生效"这件事。
    #[test]
    fn color_hex_migration_is_one_time() {
        let mut v: Value = serde_json::json!({
            "trail": { "cursor_color": "#80FF0000" },
            "drawing": { "pens": [
                { "color": "#FF000000" },
                // 脚本参数里的颜色同样要迁（键名由脚本声明，靠"含 color"判定）
                { "color": "#FF00FF00" }
            ]},
            "trail_params": { "script_params": { "color": "#40FF0000", "size": 12 } },
            // 非颜色键（数组/普通串）绝不能被动
            "key_display": { "bg_color": [0, 0, 0, 64] },
            "color_mode": "rainbow"
        });
        let mut n = 0;
        Config::migrate_color_hex_in_value(&mut v, &mut n);
        assert_eq!(n, 4, "四处 8 位颜色应被轮转（含脚本参数里那一个）");
        assert_eq!(v["trail"]["cursor_color"], "#FF000080");
        assert_eq!(v["drawing"]["pens"][0]["color"], "#000000FF");
        assert_eq!(v["drawing"]["pens"][1]["color"], "#00FF00FF");
        assert_eq!(v["trail_params"]["script_params"]["color"], "#FF000040");
        // 副作用检查：数组与非颜色键原样
        assert_eq!(
            v["key_display"]["bg_color"],
            serde_json::json!([0, 0, 0, 64])
        );
        assert_eq!(v["color_mode"], "rainbow");
        assert_eq!(v["trail_params"]["script_params"]["size"], 12);

        // 第二遍：标记已置位 → 上层不会再调；即便调了，新口径串不该被认成旧串再轮转。
        // （这里钉的是"未置标记时会再转一次"，所以标记是**必须**的，不能省。）
        let mut n2 = 0;
        Config::migrate_color_hex_in_value(&mut v, &mut n2);
        assert_eq!(
            n2, 4,
            "没有标记保护时确实会再转一次 —— 故 color_hex_v2 不可省"
        );
    }

    /// 计时器出厂定档（2026-09-27 用户口径）：正/倒计时同默认 ——
    /// **顶部居中 + 双偏移 0 + 时长 300 秒 + 字号 48**，快捷键**留空**。
    ///
    /// 与 `note_factory_defaults_match_the_shipped_look` 同一手法：钉**成品**而不是
    /// 逐个字段比对，以后再调默认一眼就知道还要动哪儿（含 `build/config.json` 的
    /// `timer` 段——里面留旧值会把新默认整个挡住）。
    #[test]
    fn timer_factory_defaults_are_top_centered_48pt_300s() {
        let d = TimerSettings::default();
        // 顶部居中 = 下标 1（`timer_center` 的 match 里 1 即 `top`）；0 是中央。
        assert_eq!(d.anchor, 1, "默认位置 = 顶部居中");
        assert_eq!(d.offset_x, 0, "水平偏移出厂不动");
        assert_eq!(d.offset_y, 0, "垂直偏移出厂不动");
        assert_eq!(d.duration_sec, 300, "倒计时时长出厂 = 300 秒（= 05:00）");
        assert_eq!(d.font_pt, 48, "字号出厂 = 48pt");
        // 默认模式是正计时，正计时不消费时长；这里顺手钉死，免得有人把两者对调。
        assert_eq!(d.mode, "countup", "默认模式 = 正计时");
        assert!(d.hotkey.is_empty(), "快捷键出厂留空（= 停用）");
        // 默认值必须落在面板滑条量程内，否则"出厂就顶在量程边上"。
        assert!(
            (TIMER_FONT_MIN..=TIMER_FONT_MAX).contains(&d.font_pt),
            "字号越出滑条量程"
        );
        // `Default` 与 serde 默认必须同源：手删一行 config.json 后补出来的值也得一致。
        let json = serde_json::to_string(&TimerSettings::default()).unwrap();
        let back: TimerSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, d, "Default 与 serde 默认不是同一个值");
    }

    #[test]
    fn clip_capacity_tiers_are_500_1000_2000_unlimited() {
        // ★ 用户定案（2026-09-24）：四档 = 500 / 1000 / 2000 / 不限（去掉最小的 200，
        //   末端改「不限」）。标签与真实值**必须同序**，否则下拉选到的是另一档。
        assert_eq!(CLIP_ENTRIES_LABELS, ["500", "1000", "2000", "不限"]);
        assert_eq!(CLIP_ENTRIES_VALUES, [500, 1000, 2000, usize::MAX]);
        assert_eq!(
            CLIP_ENTRIES_LABELS.len(),
            CLIP_ENTRIES_VALUES.len(),
            "标签与数值必须一一对应"
        );
        // 出厂档 = 「不限」（下标 3，2026-10-08 用户定案）。
        assert_eq!(default_clip_max_entries(), CLIP_ENTRIES_VALUES[3]);
        assert_eq!(default_clip_max_entries(), usize::MAX);
        // 「不限」= usize::MAX：淘汰判据是 `len() > max`，恒不成立 → 一条都不淘汰。
        let unlimited = CLIP_ENTRIES_VALUES[3];
        assert_eq!(unlimited, usize::MAX);
        assert!(u64::MAX as usize <= unlimited, "任何条数都超不过「不限」");
    }

    #[test]
    fn note_factory_defaults_match_the_shipped_look() {
        // ★ 用户定档（2026-09-27）：淡青底 `#8AE6FF` + 黑字 `#000000` + 白编辑区
        //   `#FFFFFF` + 总不透明度 80 + 字号 24。默认值散在五个 `default_note_*()`
        //   单源函数里，这里钉的是**成品**：以后换配色，一眼知道还得同步
        //   `build/config.json` 的 `sticky_note` 段（里面留旧值会把新默认整个挡住）。
        let d = NoteSettings::default();
        assert_eq!(d.bg_color, [0x8A, 0xE6, 0xFF, 255], "背景颜色 = #8AE6FF");
        assert_eq!(d.text_color, [0x00, 0x00, 0x00, 255], "文字颜色 = #000000");
        assert_eq!(
            d.edit_color,
            [0xFF, 0xFF, 0xFF, 255],
            "编辑区颜色 = #FFFFFF"
        );
        assert_eq!(d.alpha, 80, "总不透明度 = 80");
        assert_eq!(d.font_size, 24.0, "字号 = 24");
        // 三色出厂全部不透明：半透明色再乘总不透明度，"这个滑条到底管哪一块"就变成试算。
        for (name, c) in [
            ("背景", d.bg_color),
            ("文字", d.text_color),
            ("编辑区", d.edit_color),
        ] {
            assert_eq!(c[3], 255, "{name}色的 alpha 出厂必须是 255");
        }
        // 总不透明度必须落在滑条量程内（面板滑条与标题栏 Ctrl+滚轮共用 NOTE_ALPHA_*）。
        assert!(
            (NOTE_ALPHA_MIN..=NOTE_ALPHA_MAX).contains(&d.alpha),
            "alpha 越出滑条量程"
        );
    }

    #[test]
    fn trail_module_active_tracks_each_feature_card() {
        // ★ 开关架构护栏：四张功能卡片任一开启 → 模块门必须打开；全关 → 关闭。
        let mut cfg = Config::default();
        assert!(!cfg.trail_module_active(), "出厂全关");

        cfg.trail.trail_enabled = true; // 拖尾效果卡
        assert!(cfg.trail_module_active());

        cfg = Config::default();
        cfg.trail.hover_enabled = true; // 悬浮标卡
        assert!(cfg.trail_module_active());

        cfg = Config::default();
        cfg.trail.ripple_enabled = true; // 2026-09-25 晚出厂改关，测试显式打开总开关
        cfg.trail.ripple_bindings.get_mut("right").unwrap().enabled = true; // 点击效果卡（仅右键）
        assert!(cfg.trail_module_active());

        // ★ 2026-09-25 总开关（AND 闸）：总开关关着时，即使某键 enabled=true，
        //   模块门也不能被波纹/声音点亮（否则覆盖层被"永远叫不醒的功能"拖着不休眠）。
        cfg = Config::default();
        cfg.trail.ripple_enabled = false;
        cfg.trail.ripple_bindings.get_mut("right").unwrap().enabled = true;
        assert!(!cfg.trail_module_active(), "波纹总开关关 = 该键不点亮模块");

        cfg = Config::default();
        cfg.trail.sound_enabled = true; // 2026-09-25 晚出厂改关，测试显式打开总开关
        cfg.trail.sound_bindings.get_mut("x1").unwrap().enabled = true; // 按键声音卡（仅侧键1）
        assert!(cfg.trail_module_active());

        cfg = Config::default();
        cfg.trail.sound_enabled = false;
        cfg.trail.sound_bindings.get_mut("x1").unwrap().enabled = true;
        assert!(!cfg.trail_module_active(), "声音总开关关 = 该键不点亮模块");

        // 迁移期老字段同样要能点亮模块门（老配置首次落盘前只有 sound_left_enabled）。
        cfg = Config::default();
        cfg.trail.sound_bindings.clear();
        cfg.trail.sound_enabled = true;
        cfg.trail.sound_left_enabled = true;
        assert!(cfg.trail_module_active());
    }

    #[test]
    fn composite_scripts_light_up_module_gate() {
        // ★ 复合脚本宿主住在 TrailModule 里（2026-09-26）：运行列表非空必须点亮
        //   模块门，否则覆盖层 park + 桥接线程丢鼠标事件（"勾了没反应"的根因）。
        let mut cfg = Config::default();
        assert!(!cfg.trail_module_active(), "出厂无复合脚本在跑");
        cfg.lua_ext.composite_active = vec!["scripts/composite_demo".to_string()];
        assert!(cfg.trail_module_active());
        cfg.lua_ext.composite_active.clear();
        assert!(!cfg.trail_module_active(), "清空列表 = 门关上");
    }

    #[test]
    fn sound_bindings_default_to_five_keys_all_off() {
        let cfg = Config::default();
        for key in ["left", "right", "middle", "x1", "x2"] {
            let b = cfg
                .trail
                .sound_bindings
                .get(key)
                .unwrap_or_else(|| panic!("缺 {key}"));
            assert!(!b.enabled, "{key} 应默认关闭");
            assert_eq!(
                b.volume, 100,
                "{key} 的音量默认应与旧全局音量一致（§41 起 100）"
            );
        }
        // 五键都预置一个能听的声音（见 `seeded_sound_bindings`：留空串会被面板首帧改写）。
        for key in ["left", "right", "middle", "x1", "x2"] {
            assert_eq!(
                cfg.trail.sound_bindings[key].file, "mouseclick1",
                "{key} 的声音不该是空串"
            );
        }
    }

    /// 出厂默认必须是「面板解析后的样子」——否则第一次 `apply_to` 就会伪落盘。
    ///
    /// 这是实测踩过的坑：`script` 留空串（靠 `style` 兜底）时，`ui::model` 解析出
    /// `"ring"` 再写回 `script`，于是**没动任何控件**却报告"配置有改动"，
    /// 连带 `model_no_spurious_change` / `hover_param_edit_writes_only_changed_keys`
    /// 等 roundtrip 断言全红。声音侧同理：`file` 空串会被解析成目录里第一个 wav。
    #[test]
    fn seeded_bindings_are_already_resolved() {
        let cfg = Config::default();
        for (k, b) in &cfg.trail.ripple_bindings {
            assert!(
                !b.script.is_empty(),
                "{k} 的 script 是空串：面板首帧会把它改写成解析值（伪落盘）"
            );
            // §39 起出厂 script=disc、style 仍是旧内置兜底 id "ring"（仅 Rust Ripple
            // 兜底外观用），两者有意不再相等——此断言随目录收敛退役。
            assert!(
                b.script == b.style || b.script == "disc",
                "{k} 的 script 应是出厂解析值 disc"
            );
        }
        for (k, s) in &cfg.trail.sound_bindings {
            assert!(
                !s.file.is_empty(),
                "{k} 的 file 是空串：面板首帧会把它改写成目录里第一个 wav（伪落盘）"
            );
        }
    }

    /// ★ §40 波纹脚本参数一次性重置：毒值（历史播种的 #FFFF00 系）清一次，
    /// 之后用户调参照常持久化（幂等靠标记）。
    #[test]
    fn ripple_params_reset_is_one_time() {
        let mut cfg = Config::default();
        let left = cfg
            .trail
            .ripple_bindings
            .entry("left".to_string())
            .or_default();
        left.script_params
            .insert("color".to_string(), serde_json::json!("#F1F10AFF"));

        cfg.reset_ripple_script_params();
        assert!(cfg.trail.ripple_params_reset, "首次执行应置位标记");
        assert!(
            cfg.trail.ripple_bindings["left"].script_params.is_empty(),
            "毒值应被清空"
        );

        // 幂等：置位后再调，用户新调参不动。
        cfg.trail
            .ripple_bindings
            .get_mut("left")
            .unwrap()
            .script_params
            .insert("color".to_string(), serde_json::json!("#0000FF"));
        cfg.reset_ripple_script_params();
        assert_eq!(
            cfg.trail.ripple_bindings["left"]
                .script_params
                .get("color")
                .and_then(|v| v.as_str()),
            Some("#0000FF"),
            "标记置位后不得再清"
        );
    }

    /// ★ 卡片 id 的命名规则：子卡必须叫 `<父卡>.<BUTTON_KEYS 里的键>`。
    /// 面板是用 `format!("ripple.{key}")` 拼 id 去取展开信号的，两边一旦写歪，
    /// 展开态就会静默读不到/写不进（不报错，只是永远还原不了现场）。
    #[test]
    fn card_ids_follow_the_naming_rule() {
        let ids: Vec<&str> = CARD_IDS.to_vec();
        // ★ 2026-09-25 晚：波纹回到父卡+五键子卡——`ripple.<键>` 5 个 id 必须**在**；
        //   声音卡保持平铺，`sound.<键>` 仍锁"不得回潮"。子卡回来而 id 没跟上
        //   （或反之）都会在这里红。
        for k in BUTTON_KEYS {
            let want = format!("ripple.{k}");
            assert!(
                ids.contains(&want.as_str()),
                "波纹子卡 {want} 必须在 CARD_IDS（面板按 ripple.<键> 拼展开信号）"
            );
            let gone = format!("sound.{k}");
            assert!(
                !ids.contains(&gone.as_str()),
                "{gone} 随声音单卡化退役，不得回潮"
            );
        }
        for id in ["trail", "hover", "ripple", "sound"] {
            assert!(ids.contains(&id), "CARD_IDS 缺少 {id}");
        }
        // 无重复：重复会让两条卡片共用同一个键，展开态互相覆盖。
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        let n = sorted.len();
        sorted.dedup();
        assert_eq!(sorted.len(), n, "CARD_IDS 有重复项");
    }

    /// ★ 鼠标特效页的两张父卡（「点击效果」「按键声音」）自 2026-09-22 起**可以折叠**
    /// （用户口径：只这两张要折叠，"其它的不需要，就单纯折叠"）。
    ///
    /// 折叠态要落盘，父卡就必须**自己占一个 id**：子卡叫 `ripple.left`，父卡叫 `ripple`
    /// —— 父卡缺 id 时面板 `m.expand("ripple")` 只能拿到兜底信号（一个共享的
    /// `signal(false)`），展开态读不到也写不进，表现为"折了下次开面板又是展开的"，
    /// 而且**不报任何错**。这条把 id 与出厂展开态一起钉住。
    ///
    /// 出厂仍是**展开**：与改之前"恒展开"的观感一致，用户手动折起来才写进 `ui.expanded`。
    #[test]
    fn mouse_page_parents_are_foldable_and_default_collapsed() {
        for id in ["ripple", "sound"] {
            assert!(CARD_IDS.contains(&id), "CARD_IDS 缺少父卡 {id}");
            assert!(
                !default_card_expanded(id),
                "{id} 父卡出厂应折叠（2026-09-25 晚：全部卡片默认收起）"
            );
        }
        // ★ 2026-09-25 晚：波纹五键子卡回归（子卡标题栏 = 键名 +「关闭/效果」下拉）。
        //   id 必须在（折叠态要落盘），出厂**收起**——五键全展开是一堵参数墙，
        //   与声音卡平铺行形成对照：那里没有子卡，因为每键只有一行下拉。
        for k in BUTTON_KEYS {
            let id = format!("ripple.{k}");
            assert!(CARD_IDS.contains(&id.as_str()), "CARD_IDS 缺少子卡 {id}");
            assert!(!default_card_expanded(&id), "波纹子卡 {id} 出厂应收起");
        }
    }

    /// 键盘页结构（一）：**「击键回显」父卡 + 三张子卡「位置 / 显示 / 颜色」**。
    ///
    /// 父卡标题行只放开关（本功能没有"方案"那种功能级选择器），**方位下拉搬进了「位置」
    /// 子卡** —— 于是它与「虚拟键盘」的「位置」子卡严格同形（方位下拉 + 四向偏移），
    /// 学一次用两处。这条钉住的是 2026-09-18 重构前的旧结构：4 张**平卡**
    /// （位置设置 / 外观设置 / 颜色设置 / 显示内容）与一棵子树并存，缩进无从对齐。别再拆回去。
    ///
    /// 「显示」由旧「外观设置」+「显示内容」合并（后者只有一个下拉，单独成卡不值）；
    /// 旧名「外观设置」名不副实 —— 它装着淡出延迟 / 合并间隔（时间）与字号（尺寸）。
    #[test]
    fn key_display_cards_are_one_parent_plus_three() {
        for id in [
            "key_display",
            "key_display.pos",
            "key_display.display",
            "key_display.color",
        ] {
            assert!(CARD_IDS.contains(&id), "CARD_IDS 缺少 {id}");
        }
        for id in ["key_display.look", "key_display.content"] {
            assert!(!CARD_IDS.contains(&id), "{id} 已废弃，不该再出现");
        }
        assert!(
            !default_card_expanded("key_display"),
            "「击键回显」父卡出厂折叠（全部卡片默认收起）"
        );
        assert!(
            !default_card_expanded("key_display.pos"),
            "「位置」子卡出厂同样折叠"
        );
    }

    /// 键盘页结构（二）：**一张父卡「虚拟键盘」+ 四张子卡「位置 / 窗口行为 / 显示 / 颜色」**。
    /// 父卡装开关 + 方案下拉（勾选框 +「虚拟键盘」+ 方案选择同排），四张子卡各自独立展开。
    ///
    /// 「显示 / 颜色」的边界按**客观判据**划（是不是颜色），并与「击键回显」页同名成对：
    ///
    /// · 「显示」= `ControlGroup::Look` —— 数值项（键帽大小 / 空隙 / 圆角 / 不透明度 /
    ///   高亮保持 / 字号 / 命名样式圆角）；
    /// · 「颜色」= `ControlGroup::Color` —— 兜底色 + 命名样式色。
    ///
    /// 这两张卡的历史是一串反转，本节把**当前**落点说明白，免得再被拆成四张或并成一张：
    /// 曾按"主观刀口"拆四张（外观 / 配色…）→「圆角」被挪进颜色卡、「字号 / 不透明度」被拆到
    /// 第三张；曾合成一张「方案配置」→ 边界说得通（都是所选方案 JSON 的属性），但**长度无上界**
    /// ——`ansi_104` / `ansi_60` 实测 10 项、`ansi_104_dark` 21 项，方案作者每多声明一个命名
    /// 样式就再长 ≥2 项，一张卡翻不到底。现在切两张：判据客观、长度各减一半。
    ///
    /// 这条同时是上一版错误结构的防回归锁（曾拆成「窗口 / 方案配置」两张父卡 + 六张子卡）。
    #[test]
    fn virtual_keyboard_cards_are_one_parent_plus_four() {
        for id in [
            "virtual_keyboard",
            "virtual_keyboard.pos",
            "virtual_keyboard.window",
            "virtual_keyboard.look",
            "virtual_keyboard.color",
        ] {
            assert!(CARD_IDS.contains(&id), "CARD_IDS 缺少 {id}");
        }
        for id in [
            "virtual_keyboard.scheme",
            "virtual_keyboard.scheme_color",
            "virtual_keyboard.size",
        ] {
            assert!(!CARD_IDS.contains(&id), "{id} 已废弃，不该再出现");
        }
        assert!(
            !default_card_expanded("virtual_keyboard"),
            "「虚拟键盘」父卡出厂折叠（全部卡片默认收起）"
        );
    }

    /// 设置页卡片的合并史：**「模块开关 + 脚本开关」合为「功能开关」、「快捷键 + 脚本快捷键」
    /// 合为「快捷键」**（2026-10-07），各只留一个展开态 id；**「图层覆盖 / 工作文件夹 / 系统」
    /// 三张薄卡合为「高级」**（m69，2026-10-31），商店与更新共用的下载源下拉也归这张卡。
    ///
    /// 合并前这几组各占多张卡；现在每段只是卡**正文里的一个分组**（横线 + 段名），不再有
    /// 独立展开态 —— 这是合并的本意（一次展开看全），代价是脚本段不能单独折叠。
    /// 作废的 id 按"已废弃"登记（老配置里残留的键 serde 原样保留，不丢值也不报错）。
    /// ★ `global.store` 只是**换页**（搬家不换 id），所以它必须仍在表里。
    #[test]
    fn global_page_cards_are_merged() {
        for id in [
            "global.switches",
            "global.hotkey",
            "global.advanced",
            "global.store",
        ] {
            assert!(CARD_IDS.contains(&id), "CARD_IDS 缺少 {id}");
        }
        for id in [
            "global.script_switches",
            "global.script_hotkeys",
            "global.overlay",
            "global.temp",
            "global.system",
        ] {
            assert!(!CARD_IDS.contains(&id), "{id} 已废弃，不该再出现");
        }
        // 老配置带着作废键读进来不该炸（serde 原样保留）。
        let json = r##"{ "ui": { "expanded": { "global.script_switches": true } } }"##;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert!(!cfg.ui.expanded.is_empty(), "作废键应被原样保留");
    }

    /// 键盘页的卡片总数：**2 张功能父卡 + 7 张子卡 = 9**（击键回显 3 张、虚拟键盘 4 张）。
    ///
    /// 单看上面两条用例，漏掉一张卡、或多插一张卡都不会被抓住（它们只断言"该在的在"）。
    /// 这条从**数量与集合**侧收紧：父卡 2 张，子卡数与各自的参数面一一对应 ——「击键回显」三张
    /// （位置 / 显示 / 颜色）、「虚拟键盘」四张（位置 / 窗口行为 / 显示 / 颜色）。
    #[test]
    fn keyboard_page_has_two_parents_with_three_and_four_subcards() {
        let mut page: Vec<&str> = CARD_IDS
            .iter()
            .copied()
            .filter(|id| id.starts_with("key_display") || id.starts_with("virtual_keyboard"))
            .collect();
        page.sort_unstable();
        assert_eq!(
            page,
            [
                "key_display",
                "key_display.color",
                "key_display.display",
                "key_display.pos",
                "virtual_keyboard",
                "virtual_keyboard.color",
                "virtual_keyboard.look",
                "virtual_keyboard.pos",
                "virtual_keyboard.window",
            ],
            "键盘页卡片集合变了：应为 2 张父卡 + 7 张子卡（3 + 4）"
        );
        for (parent, want) in [("key_display", 3usize), ("virtual_keyboard", 4usize)] {
            let kids = page
                .iter()
                .filter(|id| id.starts_with(&format!("{parent}.")))
                .count();
            assert_eq!(kids, want, "「{parent}」应有 {want} 张子卡，实为 {kids}");
        }
    }

    /// 屏幕页抓取录制三卡的卡片结构：**3 张顶层卡（截图与贴图 / GIF录制 / 视频录制）**，
    /// 其中只有「屏幕录制」是父卡，下挂**三张子卡（快捷键 / 画面 / 声音）**。
    ///
    /// 为什么只有录屏拆子卡、GIF 不拆 —— 判据是**长度**（与键盘页虚拟键盘卡同一条理由）：
    /// 录屏参数面 13 项，平铺一张卡翻不到底，且三张子卡各自回答一个用户能自答的问题；
    /// GIF 只有 6 项，拆开每张子卡剩两行，反而要多点两次折叠。
    ///
    /// 用**集合相等**而不是"该在的在"：漏一张卡、多插一张卡都要被抓到（单看子串断言
    /// 抓不住）。同时钉住 `record.gif` 没被子卡化 —— GIF 与录屏共用采集管线，但**不是**
    /// 录屏的子卡：它是并列的第三个功能（产物格式不同、参数面独立）。
    #[test]
    fn screen_capture_cards_keep_three_tops_and_video_with_three_subcards() {
        let mut page: Vec<&str> = CARD_IDS
            .iter()
            .copied()
            .filter(|id| id.starts_with("record."))
            .collect();
        page.sort_unstable();
        assert_eq!(
            page,
            [
                "record.gif",
                "record.shot",
                "record.video",
                "record.video.audio",
                "record.video.hotkey",
                "record.video.image",
            ],
            "抓取录制三卡集合变了：应为 3 张顶层卡，「视频录制」下 3 张子卡"
        );
        // 三张顶层卡：`record.<name>` 且不再带 `.` 的（子卡是 `record.video.<x>`）。
        let tops = page
            .iter()
            .filter(|id| id["record.".len()..].find('.').is_none())
            .count();
        assert_eq!(
            tops, 3,
            "抓取录制三卡应有 3 张顶层卡（截图与贴图 / GIF录制 / 视频录制）"
        );
        for id in [
            "record.gif.size",
            "record.gif.hotkey",
            "record.shot.pin",
            "record.video.scheme",
        ] {
            assert!(
                !CARD_IDS.contains(&id),
                "{id} 已废弃/不该存在，别把 GIF 拆成子卡"
            );
        }
        assert!(
            !default_card_expanded("record.shot"),
            "截图与贴图卡出厂折叠（全部卡片默认收起）"
        );
    }

    /// 录屏的**分辨率档与帧率档**：档位表本身与"存下标"的换算要一致。
    ///
    /// 这两张表是"用户看到的档"与"实际传给编码器的值"之间唯一的翻译层：
    /// 配置里存的是**下标**，一旦表长与下标范围对不上（少写一个、写重一个），
    /// 用户选「720p」可能落到「原始」或越界回落 —— 而面板不会报错。
    #[test]
    fn record_tier_tables_are_consistent() {
        assert_eq!(
            RECORD_RESOLUTION_LABELS.len(),
            RECORD_RESOLUTION_LONG_EDGE.len()
        );
        assert_eq!(RECORD_FPS_LABELS.len(), RECORD_FPS_VALUES.len());
        assert_eq!(RECORD_BITRATE_LABELS.len(), RECORD_BITRATE_KBPS.len());
        assert_eq!(RECORD_HARDWARE_LABELS.len(), 2, "0=自动 1=关闭");
        assert_eq!(GIF_FPS_LABELS.len(), GIF_FPS_VALUES.len());
        assert_eq!(GIF_SIZE_LABELS.len(), GIF_SIZE_LONG_EDGE.len());
        assert_eq!(
            MIC_DENOISE_LABELS.len(),
            MIC_DENOISE_VALUES.len(),
            "降噪档名与内部值必须一一对应"
        );
        // 「原始」档必须是 0，且长边上限就是 0（不缩放）——面板靠这个下标做默认值。
        assert_eq!(RECORD_RESOLUTION_LABELS[0], "原始");
        assert_eq!(RECORD_RESOLUTION_LONG_EDGE[0], 0);
        // ★ 清晰度升序：1=720p、2=1080p、3=1440p、4=2160p。名字钉死，谁插档谁挂测试。
        assert_eq!(RECORD_RESOLUTION_LABELS[1], "720p");
        assert_eq!(RECORD_RESOLUTION_LABELS[2], "1080p");
        assert_eq!(RECORD_RESOLUTION_LABELS[3], "1440p");
        assert_eq!(RECORD_RESOLUTION_LABELS[4], "2160p");
        assert_eq!(RECORD_RESOLUTION_LONG_EDGE[1], 1280);
        assert_eq!(RECORD_RESOLUTION_LONG_EDGE[2], 1920);
        assert_eq!(RECORD_RESOLUTION_LONG_EDGE[3], 2560);
        assert_eq!(RECORD_RESOLUTION_LONG_EDGE[4], 3840);
        // 升序：长边严格递增且都为正（720p<1080p<1440p<2160p）。
        let edges = RECORD_RESOLUTION_LONG_EDGE;
        assert!(
            edges[1] < edges[2] && edges[2] < edges[3] && edges[3] < edges[4] && edges[1] > 0,
            "分辨率档长边应严格升序: {edges:?}"
        );
        // 帧率档不含 0（0 帧是无限循环/卡死，不是"最低帧率"）。
        assert!(RECORD_FPS_VALUES.iter().all(|&f| f > 0));
        assert!(GIF_FPS_VALUES.iter().all(|&f| f > 0));
        // ★ GIF 帧率上限 50：帧延时单位 10ms，50fps = 20ms，再高会落进
        //   "浏览器把 <20ms 当 100ms 播"的坑（见 `core::gif_enc`）。
        assert!(
            GIF_FPS_VALUES.iter().all(|&f| f <= 50),
            "GIF 帧率不该超过 50: {GIF_FPS_VALUES:?}"
        );
    }

    /// ★ 出厂档的下标必须落在各自档位表范围内，且默认值不越界、不静默跳到最激进档。
    #[test]
    fn default_denoise_level_points_inside_the_table() {
        let cfg = Config::default();
        assert!(
            (cfg.record.resolution as usize) < RECORD_RESOLUTION_LABELS.len(),
            "出厂分辨率档下标越界"
        );
        assert!(
            (cfg.record.hardware as usize) < RECORD_HARDWARE_LABELS.len(),
            "出厂硬件加速档下标越界"
        );
        assert!(
            (cfg.gif.size as usize) < GIF_SIZE_LABELS.len(),
            "出厂 GIF 尺寸档下标越界"
        );
        assert!(
            RECORD_FPS_VALUES.contains(&cfg.record.fps),
            "出厂录屏帧率必须落在档位表内: {}",
            cfg.record.fps
        );
        assert!(
            GIF_FPS_VALUES.contains(&cfg.gif.fps),
            "出厂 GIF 帧率必须落在档位表内: {}",
            cfg.gif.fps
        );
    }

    #[test]
    fn migrate_denoise_collapses_legacy_modes() {
        // 旧「模式 + 强度」两档 → 新单一下拉。
        let mut off = Config::default();
        off.record.mic_denoise = "off".into();
        off.migrate_denoise();
        assert_eq!(off.record.mic_denoise, "off", "off 原样保留");

        // 遗留 spectral / rnnoise / gate 全部并入 medium（RNNoise 中等强度）。
        for legacy in ["spectral", "rnnoise", "gate"] {
            let mut c = Config::default();
            c.record.mic_denoise = legacy.into();
            c.migrate_denoise();
            assert_eq!(c.record.mic_denoise, "medium", "{legacy} 应规整为 medium");
        }

        // 手滑写错 / 未知值 → 安全回落 off。
        let mut bad = Config::default();
        bad.record.mic_denoise = "garbage".into();
        bad.migrate_denoise();
        assert_eq!(bad.record.mic_denoise, "off", "未知取值回落 off");
    }

    /// ★ 录屏分辨率档顺序从「原始/1080p/720p/...」调整为「原始/720p/1080p/...」后，
    /// 老配置的下标必须重映射（只换中间两档），且越界值被夹回范围内。
    #[test]
    fn migrate_resolution_order_remaps_old_indices() {
        // 旧 `1`=1080p → 新 `2`（仍是 1080p）；旧 `2`=720p → 新 `1`（仍是 720p）。
        for (old, want) in [(0u32, 0u32), (1, 2), (2, 1), (3, 3), (4, 4)] {
            let mut c = Config::default();
            c.record.resolution = old;
            c.migrate_resolution_order();
            assert_eq!(
                c.record.resolution, want,
                "旧下标 {old} 应映射到新下标 {want}"
            );
        }
        // 越界下标夹回 < 档位数（5）。
        let mut oob = Config::default();
        oob.record.resolution = 99;
        oob.migrate_resolution_order();
        assert_eq!(
            oob.record.resolution,
            (RECORD_RESOLUTION_LABELS.len() - 1) as u32,
            "越界下标应夹到最高档"
        );
    }

    /// ★ 2026-09-26 用户口径：截图与贴图的三把快捷键出厂**全留空**（空 = 停用）。
    ///
    /// 功能开关（`enabled`）本来就出厂关闭，预置键位只是摆设；与画中画 / 全屏放大快捷键
    /// 的"出厂留空"同一条线。用户配上键位后的语义（Ctrl 变体、撞弦拆解）由
    /// [`Self::missing_ctrl_variant_names_the_offending_keys`] /
    /// [`Self::migrate_snap_hotkey_shadowing_splits_the_clash`] / 撞弦集合测试守门。
    #[test]
    fn factory_snap_hotkeys_default_empty() {
        let cfg = Config::default();
        assert!(cfg.screenshot.hotkey.is_empty(), "截图快捷键出厂留空");
        assert!(cfg.screenshot.hotkey_pin.is_empty(), "截图后贴图键出厂留空");
        assert!(
            cfg.screenshot.hotkey_pin_clipboard.is_empty(),
            "贴图快捷键出厂留空"
        );
        assert!(
            cfg.snap_keys_missing_ctrl_variant().is_empty(),
            "出厂配置不该出现在 startup 日志里被点名的键"
        );
    }

    /// 撞弦配置被拆开：截图键清空停用（2026-09-26 起无出厂键位可还原），「截图并贴图」的两条路都回来。
    #[test]
    fn migrate_snap_hotkey_shadowing_splits_the_clash() {
        let mut cfg = Config::default();
        cfg.screenshot.hotkey = "Ctrl+F3".into();
        cfg.screenshot.hotkey_pin = "F3".into();
        // 前提：这确实是撞弦 —— 截图并贴图的 Ctrl 变体就是截图本体的规格。
        assert_eq!(
            crate::core::hotkeys::with_ctrl(&cfg.screenshot.hotkey_pin),
            "Ctrl+F3"
        );
        cfg.migrate_snap_hotkey_shadowing();
        assert_eq!(cfg.screenshot.hotkey, "", "撞弦的截图键应被清空停用");
        assert_eq!(cfg.screenshot.hotkey_pin, "F3", "截图并贴图键不动");
        assert!(
            cfg.snap_keys_missing_ctrl_variant().is_empty(),
            "拆开之后「截图并贴图」的 Ctrl 变体不再被抢"
        );
    }

    /// 健康的配置一个字段都不许动（迁移最常见的事故就是"顺手多改一处"）。
    #[test]
    fn migrate_snap_hotkey_shadowing_keeps_healthy_configs() {
        // 出厂配置：本来就没撞弦。
        let mut cfg = Config::default();
        let before = cfg.screenshot.clone();
        cfg.migrate_snap_hotkey_shadowing();
        assert_eq!(cfg.screenshot, before, "出厂配置不该被迁移碰到");

        // 用户自定义但没撞弦：`Ctrl+Alt+S` 是"截图 + Ctrl"，跟热点键本体不冲突。
        let mut custom = Config::default();
        custom.screenshot.hotkey = "Ctrl+Alt+S".into();
        custom.screenshot.hotkey_pin = "F3".into();
        let before = custom.screenshot.clone();
        custom.migrate_snap_hotkey_shadowing();
        assert_eq!(custom.screenshot, before, "不撞弦就不该被改");

        // 截图键为空（= 停用）时不迁移：没有键可还原。
        let mut off = Config::default();
        off.screenshot.hotkey = String::new();
        off.screenshot.hotkey_pin = "F3".into();
        off.migrate_snap_hotkey_shadowing();
        assert_eq!(off.screenshot.hotkey, "", "停用状态不该被塞回出厂值");
    }

    /// ★ 全局热键裸键清洗（2026-10-01）：全局语义字段里的裸键清空停用，标注工具键
    ///   （`1`..`9`/`0`）与带修饰键的规格原样保留，且**幂等**。
    ///
    /// 分辨力集中在三处：①出厂默认（画笔裸数字）必须一个都不动——清洗若误伤工具键，
    /// 用户重启后键位全丢；②脚本热键参数走同一道清洗（`params[scripts/x]["hotkey"]`）；
    /// ③清空而不是替换——替用户挑一把键是不打招呼的改动。
    #[test]
    fn sanitize_global_hotkeys_clears_bare_keys_but_spares_annotation_tools() {
        let mut cfg = Config::default();
        // 出厂配置里只有工具键是裸键（1..9/0），全局键全空或带修饰 —— 一个都不许动。
        let drawing_before = cfg.drawing.hotkeys.clone();
        cfg.sanitize_global_hotkeys();
        assert_eq!(
            cfg.drawing.hotkeys, drawing_before,
            "出厂工具键（裸 1..9/0）是标注态语义，清洗不许碰"
        );

        // 全局字段配裸键 → 清空；带修饰键 / F 键区 → 保留。
        cfg.screenshot.hotkey = "A".into();
        cfg.screenshot.hotkey_pin = "Ctrl+1".into();
        cfg.record.hotkey = "F4".into();
        cfg.global.panel_hotkey = "Space".into();
        cfg.drawing.hotkeys.draw_hk_screen = "S".into();
        cfg.lua_ext.params.insert(
            "scripts/ai_chat".into(),
            HashMap::from([("hotkey".to_string(), Value::String("Q".into()))]),
        );
        cfg.sanitize_global_hotkeys();
        assert_eq!(
            cfg.screenshot.hotkey, "",
            "裸 A 全局生效 = 打字就截图，必须清空"
        );
        assert_eq!(cfg.screenshot.hotkey_pin, "Ctrl+1", "带修饰键保留");
        assert_eq!(cfg.record.hotkey, "F4", "F 键区在裸键白名单内，保留");
        assert_eq!(cfg.global.panel_hotkey, "", "裸 Space 全局生效，必须清空");
        assert_eq!(
            cfg.drawing.hotkeys.draw_hk_screen, "",
            "标注 toggle 标注没开也要生效，属全局语义，裸键清空"
        );
        assert_eq!(
            cfg.lua_ext.params["scripts/ai_chat"]["hotkey"],
            Value::String(String::new()),
            "脚本热键参数同清洗"
        );

        // 幂等：再跑一遍不该有新变化（此例中合法值原样、非法值已空）。
        let shot = cfg.screenshot.hotkey_pin.clone();
        cfg.sanitize_global_hotkeys();
        assert_eq!(cfg.screenshot.hotkey_pin, shot);
    }

    /// Lua 脚本 id 迁移（2026-09-26）：配置里按 id 记的三处（击键音效当前脚本、
    /// 两张每脚本参数表）的旧 id 全部改写成 `scripts/<英文文件名>`，且幂等。
    /// 当天先后迭代过四版形态，存量为裸中文名 / 中文功能文件夹 / script 双层 /
    /// 根目录平铺裸名时都要能迁到终态。
    #[test]
    fn migrate_lua_script_folders_rewrites_ids_idempotently() {
        // ① 最初的裸中文名存量。
        let mut cfg = Config::default();
        cfg.key_sound.script = "统一音效".into();
        cfg.key_sound.params.insert(
            "分类音效".into(),
            HashMap::from([("letter_sound".to_string(), Value::String("a.wav".into()))]),
        );
        cfg.lua_ext.params.insert("图片体检".into(), HashMap::new());
        cfg.migrate_lua_script_folders();
        assert_eq!(cfg.key_sound.script, "KeySound/unified_sound");
        assert!(
            cfg.key_sound.params.contains_key("KeySound/class_sound"),
            "旧裸名参数段应整段搬到新 id 下"
        );
        assert!(!cfg.key_sound.params.contains_key("分类音效"), "旧键不残留");
        assert!(cfg.lua_ext.params.contains_key("scripts/image_report"));
        // 幂等：再跑一遍零变化。
        let snapshot = cfg.key_sound.clone();
        cfg.migrate_lua_script_folders();
        assert_eq!(cfg.key_sound, snapshot, "迁移必须幂等");

        // ② 中文功能文件夹形态的存量要能接力迁移。
        let mut mid = Config::default();
        mid.key_sound.script = "击键音效/统一音效".into();
        mid.lua_ext
            .params
            .insert("脚本/平均颜色".into(), HashMap::new());
        mid.migrate_lua_script_folders();
        assert_eq!(
            mid.key_sound.script, "KeySound/unified_sound",
            "中文文件夹形态的存量应迁到 scripts/"
        );
        assert!(mid.lua_ext.params.contains_key("scripts/avg_color"));

        // ③ script/ 双层目录形态（当天中间版②）的存量同样接力。
        let mut nested = Config::default();
        nested.key_sound.script = "script/unified_sound/unified_sound".into();
        nested
            .lua_ext
            .params
            .insert("script/avg_color/avg_color".into(), HashMap::new());
        nested.migrate_lua_script_folders();
        assert_eq!(nested.key_sound.script, "KeySound/unified_sound");
        assert!(nested.lua_ext.params.contains_key("scripts/avg_color"));

        // ③' 根目录平铺裸名形态（当天中间版③）的存量也要接力。
        let mut flat = Config::default();
        flat.key_sound.script = "unified_sound".into();
        flat.lua_ext
            .params
            .insert("avg_color".into(), HashMap::new());
        flat.migrate_lua_script_folders();
        assert_eq!(flat.key_sound.script, "KeySound/unified_sound");
        assert!(flat.lua_ext.params.contains_key("scripts/avg_color"));

        // ③' 上一版终态（`scripts/...`）迁到键音专属目录。
        let mut previous = Config::default();
        previous.key_sound.script = "scripts/unified_sound".into();
        previous
            .key_sound
            .params
            .insert("scripts/class_sound".into(), HashMap::new());
        previous.migrate_lua_script_folders();
        assert_eq!(previous.key_sound.script, "KeySound/unified_sound");
        assert!(previous
            .key_sound
            .params
            .contains_key("KeySound/class_sound"));

        // ④ 终态 id（出厂默认）不被误改。
        let mut fresh = Config::default();
        fresh.migrate_lua_script_folders();
        assert_eq!(
            fresh.key_sound.script, "KeySound/unified_sound",
            "出厂默认已是新 id"
        );
    }

    /// 本体更新清单两条地址进配置；选中源不在 update 里重复存，直接跟随商店。
    #[test]
    fn update_urls_live_in_config_and_follow_store_source() {
        let cfg = Config::default();
        assert!(cfg.update.gitee_url.ends_with("/update.json"));
        assert!(cfg.update.github_url.ends_with("/update.json"));
        assert!(cfg
            .update
            .gitee_url
            .starts_with("https://raw.giteeusercontent.com/"));
        assert!(cfg
            .update
            .github_url
            .starts_with("https://raw.githubusercontent.com/"));
        let old: Config = serde_json::from_str("{}").expect("空对象也要能读");
        assert_eq!(old.update, UpdateSettings::default());
        let mut custom = cfg.clone();
        custom.update.gitee_url = "https://mirror.example/update.json".to_string();
        custom.update.github_url = String::new();
        let back: Config = serde_json::from_str(&serde_json::to_string(&custom).unwrap()).unwrap();
        assert_eq!(back.update, custom.update);
        assert_eq!(back.store.source_index(), 0, "更新源跟随商店来源");
    }

    /// 每脚本开关（2026-09-27 起按 role 分列表）：**列表里没有 = 关**；空对象反序列化
    /// 补齐默认；两个 role 的列表互不串台；旧配置里的 `composite_active` 仍能读回
    /// （字段没改名，只是判据改成 [`LuaExtSettings::role_is_active`]）。
    #[test]
    fn lua_ext_role_switches_default_to_off_and_roundtrip() {
        // ① 空对象反序列化 = 两个列表都空，且序列化回来两个字段都不出现（出厂零冗余，
        //    这也是 `model_no_spurious_change` 门禁的配置侧前提）。
        let partial: LuaExtSettings = serde_json::from_str("{}").expect("空对象应能反序列化");
        assert!(partial.composite_active.is_empty());
        assert!(partial.image_active.is_empty());
        assert!(!partial.role_is_active("composite", "scripts/composite_demo"));
        assert!(!partial.role_is_active("image", "scripts/image_report"));
        let s = serde_json::to_string(&partial).expect("序列化");
        assert!(!s.contains("composite_active"), "空列表不得落盘：{s}");
        assert!(!s.contains("image_active"), "空列表不得落盘：{s}");

        // ② 每个 role 的开关都幂等，且只写自己那一列。
        for role in ["composite", "image", "effect"] {
            let mut lx = LuaExtSettings::default();
            lx.set_role_active(role, "scripts/a", true);
            lx.set_role_active(role, "scripts/a", true);
            lx.set_role_active(role, "scripts/b", true);
            assert!(
                lx.role_is_active(role, "scripts/a") && lx.role_is_active(role, "scripts/b"),
                "{role} 入列幂等失败"
            );
            lx.set_role_active(role, "scripts/a", false);
            lx.set_role_active(role, "scripts/a", false);
            assert!(!lx.role_is_active(role, "scripts/a"), "{role} 移除幂等失败");
            assert!(
                lx.role_is_active(role, "scripts/b"),
                "{role} 误伤了别的条目"
            );
        }
        assert!(
            LuaExtSettings::default().composite_active.is_empty()
                && LuaExtSettings::default().image_active.is_empty(),
            "两个 role 的列表互不串台"
        );

        // ②' ★ `effect` 与 `composite` 落**同一列**（2026-09-27 放宽）：往 `scripts/`
        //    复制任何特效脚本都有开关可开，companions 运行列表只消费 composite_active。
        let mut eff = LuaExtSettings::default();
        eff.set_role_active("effect", "scripts/fireflies", true);
        assert!(
            eff.role_is_active("effect", "scripts/fireflies")
                && eff
                    .composite_active
                    .contains(&"scripts/fireflies".to_string()),
            "effect 开关必须落到 composite_active（脚本页常驻运行列表）"
        );

        // ③ `keysound` 没有落点（键盘页 `key_sound.script` 专属管理）：宁可什么都不写，
        //    也不要悄悄落进没人读的列表。
        let mut unknown = LuaExtSettings::default();
        unknown.set_role_active("keysound", "scripts/a", true);
        assert!(unknown.role_list("keysound").is_empty());
        assert!(unknown.composite_active.is_empty() && unknown.image_active.is_empty());

        // ④ 落了盘的列表能原样读回（含老配置的 `composite_active` 键）。
        let mut lx2 = LuaExtSettings::default();
        lx2.set_role_active("composite", "scripts/a", true);
        lx2.set_role_active("composite", "scripts/b", true);
        lx2.set_role_active("image", "scripts/image_report", true);
        let json = serde_json::to_string(&lx2).expect("序列化");
        let back: LuaExtSettings = serde_json::from_str(&json).expect("读回");
        assert_eq!(back.composite_active, vec!["scripts/a", "scripts/b"]);
        assert_eq!(back.image_active, vec!["scripts/image_report"]);
    }

    /// ★ 回归（2026-09-30 用户报告）：卡片开关**关着**时 `params[id].hotkey` 依然在，
    /// 于是热键照样能跑脚本。`script_active` 是派发侧唯一的判据入口，它与
    /// `role_is_active` 必须**逐 role 等价**——两套口径一旦分叉，就会出现"UI 显示关着、
    /// 热键却生效"。
    #[test]
    fn script_active_matches_the_role_lists_for_every_role() {
        for role in ["composite", "image", "effect"] {
            let mut lx = LuaExtSettings::default();
            let id = "scripts/x";
            assert!(!lx.script_active(id), "{role} 出厂必须关");
            lx.set_role_active(role, id, true);
            assert!(
                lx.script_active(id) && lx.role_is_active(role, id),
                "{role}：开之后两个判据都得为真"
            );
            lx.set_role_active(role, id, false);
            assert!(
                !lx.script_active(id) && !lx.role_is_active(role, id),
                "{role}：关之后两个判据都得为假"
            );
        }
        // image 与 composite 是两个不同的启用语义，任一为真即"这一个脚本开着"。
        let mut lx = LuaExtSettings::default();
        lx.set_role_active("image", "scripts/image_report", true);
        assert!(lx.script_active("scripts/image_report"));
        assert!(!lx.script_active("scripts/ai_chat"));
    }

    /// 含 Ctrl 的两把键会被 `snap_keys_missing_ctrl_variant` 点名（供启动日志用）。
    #[test]
    fn missing_ctrl_variant_names_the_offending_keys() {
        let mut cfg = Config::default();
        assert!(cfg.snap_keys_missing_ctrl_variant().is_empty());

        cfg.screenshot.hotkey = "Ctrl+F3".into();
        cfg.screenshot.hotkey_pin = "Ctrl+Alt+Q".into();
        assert_eq!(
            cfg.snap_keys_missing_ctrl_variant(),
            vec!["截图", "截图后贴图"]
        );

        // 贴图键没有编辑语义，含 Ctrl 不算问题（不该被点名）。
        cfg.screenshot.hotkey = String::new();
        cfg.screenshot.hotkey_pin = String::new();
        cfg.screenshot.hotkey_pin_clipboard = "Ctrl+Alt+P".into();
        assert!(cfg.snap_keys_missing_ctrl_variant().is_empty());
    }

    /// 录屏 / GIF 的开始快捷键不能撞在一起。
    ///
    /// 撞了的话 `handle_action_key` 里先匹配到的那把永远赢，另一把**完全按不动**，
    /// 而且没有任何报错 —— 用户只会觉得"GIF 快捷键坏了"。
    ///
    /// ★ 2026-09-19：录制快捷键已改一键开关，原「停止快捷键」字段删除，故这里不再参与比对。
    /// ★ 2026-09-20：新增**裁剪快捷键**，它也必须与前面每一把都不同 —— 撞了同样表现为
    ///   "后匹配到的那把完全按不动"。
    #[test]
    fn record_and_gif_start_hotkeys_do_not_collide() {
        let cfg = Config::default();
        for (a, b) in [
            (&cfg.record.hotkey, &cfg.gif.hotkey),
            (&cfg.screenshot.hotkey, &cfg.record.hotkey),
            (&cfg.screenshot.hotkey, &cfg.gif.hotkey),
            (
                &cfg.screenshot.hotkey_pin,
                &cfg.screenshot.hotkey_pin_clipboard,
            ),
            (&cfg.gif.hotkey, &cfg.gif.trim_hotkey),
            (&cfg.record.hotkey, &cfg.gif.trim_hotkey),
            (&cfg.screenshot.hotkey, &cfg.gif.trim_hotkey),
            (&cfg.screenshot.hotkey_pin, &cfg.gif.trim_hotkey),
            (&cfg.screenshot.hotkey_pin_clipboard, &cfg.gif.trim_hotkey),
            (&cfg.global.panel_hotkey, &cfg.gif.trim_hotkey),
        ] {
            assert!(
                a.is_empty() || b.is_empty() || a != b,
                "出厂快捷键撞了：{a} 与 {b}"
            );
        }
    }

    /// ★ 2026-09-26 晚用户口径：**GIF 两把快捷键出厂全留空**（录制 `Alt+Shift+G` → 空、
    /// 裁剪 `Alt+Shift+T` → 空）。空串在语义上等于"停用"，而"出厂不占用全局快捷键位"
    /// 是本版对截图 / 录屏 / 全屏放大等所有快捷键的统一规则：留一把预置键等于替用户
    /// 占住一个可能与其它软件相撞的组合，而空框在面板里是可见的，不会变成"按了没反应"。
    #[test]
    fn gif_hotkeys_default_to_empty() {
        let cfg = Config::default();
        assert!(cfg.gif.hotkey.is_empty(), "GIF 录制快捷键出厂留空");
        assert!(cfg.gif.trim_hotkey.is_empty(), "裁剪快捷键出厂留空");
        // ★ 2026-09-30 新增两把视频快捷键同一条规则：选区录屏 / MP4 时间裁剪。
        assert!(
            cfg.record.region_hotkey.is_empty(),
            "选区录屏快捷键出厂留空"
        );
        assert!(cfg.record.trim_hotkey.is_empty(), "MP4 裁剪快捷键出厂留空");
        // 用户显式停用 ^= 默认值：一眼看不出"是不是反序列化把空串顶掉了"。
        let json = serde_json::json!({ "trim_hotkey": "" });
        let gif: GifSettings = serde_json::from_value(json).expect("读");
        assert_eq!(gif.trim_hotkey, "", "用户显式停用要保留");
    }

    /// 旧 `config.json`（没有 `trim_hotkey` / `region_hotkey` 这些项）必须能读进来并拿到
    /// 默认值，而不是报错 —— 否则老用户升上来会发现"快捷键没了"。
    #[test]
    fn legacy_record_config_without_new_hotkeys_reads_with_defaults() {
        let rec: RecordSettings = serde_json::from_value(serde_json::json!({
            "enabled": true, "hotkey": "", "fps": 30
        }))
        .expect("旧配置必须能读");
        assert!(rec.region_hotkey.is_empty());
        assert!(rec.trim_hotkey.is_empty());
    }

    /// 旧 `config.json`（没有 `trim_hotkey` 这一项）必须能读进来并拿到默认值，
    /// 而不是报错或变成空串 —— 否则老用户升上来会发现"裁剪快捷键没了"。
    /// 旧配置兼容（2026-09-21 夜）：`zoom_in_hotkey` / `zoom_out_hotkey` /
    /// `pip_wheel_zoom` 三个字段已删除（缩放改为内置 Ctrl+Win+滚轮，不设配置、
    /// 不暴露界面）。老配置里残留这些键必须被**静默忽略**（结构体未开
    /// `deny_unknown_fields`），其余值原样读入。
    #[test]
    fn legacy_zoom_hotkey_keys_are_ignored() {
        let m: MagnifierPipSettings = serde_json::from_value(serde_json::json!({
            "enabled": true,
            "zoom": 3,
            "offset_x": 5,
            "zoom_in_hotkey": "Ctrl+Alt+=",
            "zoom_out_hotkey": "Ctrl+Alt+-",
            "pip_wheel_zoom": true
        }))
        .expect("旧配置必须能读");
        assert_eq!(m.zoom, 3, "其余值不受影响");
        assert_eq!(m.offset_x, 5);
    }

    /// ★ 2026-09-26 出厂默认改版（用户口径）：位置=左上、两偏移=0、面板 20%×20%、
    /// 倍率 2×（用户报的 0.2 是滑条量纲：滑条 0..=1 映射 1..=5，0.2 ≈ 1.8 → 吸附到 2）。
    /// 两把启停快捷键出厂都留空；`Default` 必须与 serde 默认同源（anchor 双源不一致曾在此翻车）。
    #[test]
    fn magnifier_defaults_follow_the_2026_09_26_spec() {
        let p = MagnifierPipSettings::default();
        assert_eq!(p.anchor, 1, "位置默认左上");
        assert_eq!((p.offset_x, p.offset_y), (0, 0));
        assert_eq!(p.panel_width, 0.2);
        assert_eq!(p.panel_height, 0.2);
        assert_eq!(p.zoom, 2);
        assert!(p.hotkey.is_empty(), "画中画快捷键出厂留空");
        // `Config::default()` 是 from_config → apply_to 的固定点，必须同值。
        let c = Config::default();
        assert_eq!(c.magnifier_pip, p);
        assert!(
            c.magnifier_fullscreen.hotkey.is_empty(),
            "全屏快捷键出厂留空（此前 Alt+Z）"
        );
        assert_eq!(c.magnifier_fullscreen.zoom, 2);
    }

    #[test]
    fn legacy_config_without_trim_hotkey_gets_the_default() {
        let json = serde_json::json!({ "hotkey": "Alt+Shift+G", "fps": 15 });
        let gif: GifSettings = serde_json::from_value(json).expect("旧配置要能读");
        assert_eq!(gif.trim_hotkey, default_hk_gif_trim());
        // 显式写空串仍然尊重用户的选择（= 停用），不能被默认值覆盖回去。
        let json = serde_json::json!({ "trim_hotkey": "" });
        let gif: GifSettings = serde_json::from_value(json).expect("读");
        assert_eq!(gif.trim_hotkey, "", "用户显式停用要保留");
    }

    /// ★ 音频来源三态：`关闭 / 跟随默认 / 指定设备` 必须能双向转换，
    /// 且"关闭"不能把已选设备名弄丢（再打开时用户希望回到同一个设备）。
    #[test]
    fn audio_source_three_states_round_trip() {
        let mut r = RecordSettings::default();
        // ★ 2026-09-26 晚：出厂**跟随系统默认**（不再是「关闭」）——见 `record_mic` 字段注释。
        assert_eq!(r.mic_source(), Some(""), "出厂应为「跟随系统默认」");
        assert_eq!(r.speaker_source(), Some(""));
        r.set_mic_source(Some(""));
        assert_eq!(r.mic_source(), Some(""));
        assert!(r.record_mic, "选了「默认」就必须是开");
        r.set_mic_source(Some("耳机麦克风"));
        assert_eq!(r.mic_source(), Some("耳机麦克风"));
        r.set_mic_source(None);
        assert_eq!(r.mic_source(), None);
        assert_eq!(r.mic_device, "耳机麦克风", "关闭不该抹掉设备名");

        r.set_speaker_source(Some("扬声器 (Realtek)"));
        assert_eq!(r.speaker_source(), Some("扬声器 (Realtek)"));
        r.set_speaker_source(None);
        assert_eq!(r.speaker_source(), None);
    }

    /// ★ 2026-09-26 晚出厂口径（用户定稿），四件事一次性钉住，**任何一件松动都会红**：
    /// 录屏快捷键留空 / 两路音频「跟随系统默认」/ 降噪「中」/ `Default` 与 serde 默认同源。
    ///
    /// 前三条是"出厂值"，第四条才是真正会咬人的那处：`#[serde(default = …)]` 与
    /// `impl Default` 是两个独立写的地方（anchor 式的双源曾在此翻过车），
    /// 用户删掉 `config.json` 里某一行后读到的值，必须与全新出厂一致。
    #[test]
    fn record_factory_defaults_follow_the_2026_09_26_spec() {
        let r = RecordSettings::default();
        assert!(r.hotkey.is_empty(), "录屏快捷键出厂留空");
        assert_eq!(r.mic_source(), Some(""), "麦克风出厂 = 跟随系统默认");
        assert_eq!(r.speaker_source(), Some(""), "系统声音出厂 = 跟随系统默认");
        assert_eq!(r.mic_denoise, "medium", "降噪出厂 = 中");
        assert_eq!(r.mic_denoise, MIC_DENOISE_VALUES[DEFAULT_MIC_DENOISE]);
        // 出厂档位必须能回读：面板下拉下标 = 该值在表里的位置。
        assert_eq!(MIC_DENOISE_LABELS[DEFAULT_MIC_DENOISE], "中");

        // ★ 双源一致性：`{}` 反序列化 == `Default`（用户手删某一行不该改变出厂值）。
        let empty: RecordSettings = serde_json::from_str("{}").expect("空对象要能读");
        assert_eq!(
            empty.hotkey, r.hotkey,
            "快捷键：serde 默认与 Default 必须同值"
        );
        assert_eq!(empty.record_mic, r.record_mic);
        assert_eq!(empty.record_speaker, r.record_speaker);
        assert_eq!(empty.mic_denoise, r.mic_denoise);
        assert_eq!(
            empty.mic_source(),
            r.mic_source(),
            "两路来源：serde 默认与 Default 必须同值"
        );
    }

    /// ★ 旧配置里的 `quality` 键必须仍被读进 `bitrate`（改名不能变成"用户档位丢了"）。
    #[test]
    fn legacy_quality_key_migrates_to_bitrate() {
        let json = r#"{ "hotkey": "Alt+R", "quality": 2 }"#;
        let r: RecordSettings = serde_json::from_str(json).expect("旧配置要能读出来");
        assert_eq!(
            r.bitrate, "14000",
            "旧 quality=2（旧语义 14 Mbps）应翻译成新取值 14000"
        );
        // 该 JSON 里没写 `record_mic` / `record_speaker`，按 serde 默认 = 开（「跟随系统默认」）。
        assert_eq!(r.mic_source(), Some(""));
        // 新写出去的 JSON 用新键名，且不再有不存在的 stop_hotkey。
        let out = serde_json::to_string(&r).expect("要能序列化");
        assert!(out.contains("\"bitrate\""), "新键应写 bitrate: {out}");
        assert!(!out.contains("quality"), "不该再写旧键: {out}");
        assert!(!out.contains("stop_hotkey"), "停止快捷键字段已删除: {out}");
    }

    /// ★ 出厂码率档必须落在档位表内，且**标签写的就是实际码率**。
    ///
    /// 这条挡的是改名时最容易漏的一处：显示名从「标准/高/极高」换成具体 Mbps 之后，
    /// 只要有人改了 `RECORD_BITRATE_KBPS` 而忘了同步标签，面板上就会出现"写着 8 Mbps、
    /// 实际按 14 Mbps 编"的静默错位 —— 用户只会发现文件突然变大。
    #[test]
    fn bitrate_labels_match_their_kbps_values() {
        let cfg = Config::default();
        assert_eq!(
            cfg.record.bitrate_kbps(),
            None,
            "出厂码率档应为「自动」（{DEFAULT_BITRATE}）"
        );
        assert_eq!(RECORD_BITRATE_LABELS.len(), RECORD_BITRATE_KBPS.len());
        assert_eq!(RECORD_BITRATE_LABELS.len(), RECORD_BITRATE_VALUES.len());
        // 下标 0 是唯一的非数字档。
        assert_eq!(RECORD_BITRATE_LABELS[0], "自动");
        assert_eq!(RECORD_BITRATE_VALUES[0], "auto");
        assert_eq!(RECORD_BITRATE_KBPS[0], 0);
        for i in 1..RECORD_BITRATE_LABELS.len() {
            assert_eq!(
                RECORD_BITRATE_LABELS[i],
                format!("{} Mbps", RECORD_BITRATE_KBPS[i] / 1000),
                "码率档标签与 kbps 表对不上（下标 {i}）"
            );
            assert_eq!(
                RECORD_BITRATE_VALUES[i],
                RECORD_BITRATE_KBPS[i].to_string(),
                "落盘取值与 kbps 表对不上（下标 {i}）"
            );
        }
        // 固定档必须真的能从落盘取值解析回来（面板选完要能生效）。
        for i in 1..RECORD_BITRATE_VALUES.len() {
            let r = RecordSettings {
                bitrate: RECORD_BITRATE_VALUES[i].to_string(),
                ..Default::default()
            };
            assert_eq!(r.bitrate_kbps(), Some(RECORD_BITRATE_KBPS[i]));
        }
    }

    /// ★ 自动档：标定点必须是 1080p30 → 8 Mbps；档位随像素数 × 帧率单调、夹在上下限内。
    #[test]
    fn auto_bitrate_is_anchored_and_bounded() {
        assert_eq!(
            auto_bitrate_kbps(1920, 1080, 30),
            8_000,
            "标定点：1080p30 = 8 Mbps（录代码也够清晰的经验下限）"
        );
        assert_eq!(auto_bitrate_kbps(1280, 720, 30), 3_500);
        assert_eq!(auto_bitrate_kbps(2560, 1440, 30), 14_500);
        assert_eq!(auto_bitrate_kbps(3840, 2160, 60), 60_000, "4K60 应顶到上限");
        // 单调：像素多、帧率高都不该反而更低。
        let mut prev = 0;
        for (w, h, f) in [
            (640, 360, 15),
            (1280, 720, 30),
            (1920, 1080, 30),
            (1920, 1080, 60),
        ] {
            let v = auto_bitrate_kbps(w, h, f);
            assert!(v >= prev, "{w}×{h}@{f} 码率应不低于更低档：{v} < {prev}");
            prev = v;
        }
        // 极端输入不 panic、不越界、不返回 0。
        for (w, h, f) in [(0, 0, 0), (u32::MAX, u32::MAX, u32::MAX), (1, 1, 1)] {
            let v = auto_bitrate_kbps(w, h, f);
            assert!(
                (AUTO_BITRATE_MIN_KBPS..=AUTO_BITRATE_MAX_KBPS).contains(&v),
                "{w}×{h}@{f} 越界：{v}"
            );
        }
    }

    /// ★ 老配置的数字下标必须按**旧语义**翻译，不能被新表解释成别的档位。
    #[test]
    fn legacy_numeric_bitrate_keeps_its_old_meaning() {
        for (idx, kbps) in RECORD_BITRATE_LEGACY_INDEX.iter().enumerate() {
            let json = format!(r#"{{ "bitrate": {idx} }}"#);
            let r: RecordSettings = serde_json::from_str(&json).expect("旧下标要能读出来");
            assert_eq!(&r.bitrate, kbps, "旧下标 {idx} 的语义变了");
            assert_eq!(r.bitrate_kbps(), Some(kbps.parse().unwrap()));
        }
        // 越界下标回落自动（而不是夹到某一档，那会悄悄改变用户的码率）。
        let r: RecordSettings = serde_json::from_str(r#"{ "bitrate": 99 }"#).unwrap();
        assert_eq!(r.bitrate, DEFAULT_BITRATE);
        // 新写法（字符串）原样读入；写出去也是字符串，老配置不会再被写回数字。
        let r: RecordSettings = serde_json::from_str(r#"{ "bitrate": "40000" }"#).unwrap();
        assert_eq!(r.bitrate_kbps(), Some(40_000));
        let out = serde_json::to_string(&r).unwrap();
        assert!(out.contains(r#""bitrate":"40000""#), "应写成字符串: {out}");
        // 取值被手改成垃圾 → 回落自动，而不是 0。
        let r: RecordSettings = serde_json::from_str(r#"{ "bitrate": "高清" }"#).unwrap();
        assert_eq!(r.bitrate_kbps(), None, "解析不了应回落自动");
        // ★ 手改 config.json 的人会直接写码率数字（`40000` 而不是 `"40000"`）：
        //   那既不是旧下标、也不是字符串，必须按 kbps 读入 —— 否则他会以为设置没生效。
        for n in [40000u32, 60000, 500, 8000] {
            let r: RecordSettings =
                serde_json::from_str(&format!(r#"{{ "bitrate": {n} }}"#)).unwrap();
            assert_eq!(r.bitrate_kbps(), Some(n), "数字 {n} 应按 kbps 读入");
        }
        // 既不是合法旧下标（0..=2）、也不像码率的数 → 回落自动。
        let r: RecordSettings = serde_json::from_str(r#"{ "bitrate": 7 }"#).unwrap();
        assert_eq!(r.bitrate, DEFAULT_BITRATE, "3..499 这种数应回落自动");
    }

    /// 老配置（升级上来的）没有 `ui.expanded` → 读出来是空表，一律走出厂默认，
    /// 不该报反序列化错误（`#[serde(default)]` 的意义）。
    #[test]
    fn old_config_without_expand_state_uses_defaults() {
        let json = r##"{ "trail": { "trail_enabled": true } }"##;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert!(cfg.ui.expanded.is_empty());
        // 2026-09-25 晚：出厂展开态整体改"全部折叠"，所有 id 一律 false。
        assert!(!default_card_expanded("trail"));
        assert!(!default_card_expanded("hover"));
        assert!(!default_card_expanded("key_display.pos"));
        assert!(!default_card_expanded("drawing.pen"));
        assert!(!default_card_expanded("drawing.pensub"));
        assert!(!default_card_expanded("drawing.hotkey"));
        assert!(!default_card_expanded("global.switches"));
        assert!(!default_card_expanded("global.hotkey"));
        assert!(!default_card_expanded("global.advanced"));
    }

    /// 展开态要能原样落盘再读回来（这就是"下次打开看到同样现场"的全部依据）。
    #[test]
    fn expand_state_survives_a_json_roundtrip() {
        let mut cfg = Config::default();
        cfg.ui.expanded.insert("ripple.right".into(), true);
        cfg.ui.expanded.insert("trail".into(), false); // 收起默认展开的卡
        let text = serde_json::to_string(&cfg).unwrap();
        let back: Config = serde_json::from_str(&text).unwrap();
        assert_eq!(back, cfg, "展开态 roundtrip 后应完全相等");
        assert_eq!(back.ui.expanded.get("ripple.right"), Some(&true));
        assert_eq!(back.ui.expanded.get("trail"), Some(&false));
    }

    #[test]
    fn legacy_sound_fields_migrate_into_bindings() {
        // 1.0 的老配置：左右键 + 全局音量 → sound_bindings（含用户选过的声音文件）。
        let json = r##"{
            "trail": {
                "sound_volume": 80,
                "sound_left_enabled": true,
                "sound_right_enabled": false,
                "sound_left_file": "click3",
                "sound_right_file": "switch13"
            }
        }"##;
        let mut cfg: Config = serde_json::from_str(json).unwrap();
        assert!(cfg.trail.sound_bindings.is_empty(), "缺字段 → 空表，等迁移");
        cfg.migrate_sound_bindings();
        assert_eq!(cfg.trail.sound_bindings.len(), 5, "五键齐全");
        let left = &cfg.trail.sound_bindings["left"];
        assert!(left.enabled);
        assert_eq!(left.file, "click3", "用户选过的声音要跟着搬过来");
        assert_eq!(left.volume, 80, "老全局音量作为每键初值");
        let right = &cfg.trail.sound_bindings["right"];
        assert!(!right.enabled);
        assert_eq!(right.file, "switch13");
        // 中键 / 侧键：老配置没有对应字段 → 出厂预置值（非空串，见 seeded_sound_bindings）。
        assert_eq!(cfg.trail.sound_bindings["x1"].file, "mouseclick1");
        assert!(!cfg.trail.sound_bindings["x1"].enabled, "预置不等于启用");
        // 幂等：再调一次不改变结果。
        let before = cfg.trail.sound_bindings.clone();
        cfg.migrate_sound_bindings();
        assert_eq!(cfg.trail.sound_bindings, before);
    }

    #[test]
    fn legacy_ripple_fields_migrate_into_bindings() {
        // 老配置迁移：左键效果开 + 尺寸/时长/颜色 → ripple_bindings["left"]。
        let json = r##"{
            "trail": {
                "ripple_left_enabled": true,
                "ripple_size": 120,
                "ripple_duration": 400,
                "ripple_left_color": "#00FF00FF"
            }
        }"##;
        let mut cfg: Config = serde_json::from_str(json).unwrap();
        assert!(cfg.trail.ripple_bindings.is_empty());
        cfg.migrate_ripple_bindings();
        let left = &cfg.trail.ripple_bindings["left"];
        assert!(left.enabled);
        assert_eq!(left.size, 120);
        assert_eq!(left.duration, 400);
        // §40 起 legacy ripple_*_color 不再继承（出厂统一 #FF0000，脚本 schema 接管）。
        assert_eq!(left.color, "#FF0000");
        // 右键未配置 → 默认关闭
        assert!(!cfg.trail.ripple_bindings["right"].enabled);
    }

    #[test]
    fn legacy_cursor_fields_migrate_into_hover_params() {
        // 老配置迁移：cursor_mode=2（圆环）、外径 400、弧宽 98、颜色半透明红
        // → hover_enabled=true、hover_type="ring"、hover_params[ring] 四参数齐全。
        let json = r##"{
            "trail": {
                "cursor_mode": 2,
                "cursor_outer": 400,
                "cursor_arc": 98,
                "cursor_color": "#FF000080"
            }
        }"##;
        let mut cfg: Config = serde_json::from_str(json).unwrap();
        assert!(cfg.trail.hover_params.is_empty(), "serde 不应自动填参数表");
        cfg.migrate_hover_settings();
        assert!(cfg.trail.hover_enabled);
        assert_eq!(cfg.trail.hover_type, "ring");
        let ring = &cfg.trail.hover_params["ring"];
        assert_eq!(ring["outer"], serde_json::json!(400));
        assert_eq!(ring["arc"], serde_json::json!(98));
        // 2026-10-09 修正：透明度折在 8 位颜色里（alpha=0x80），独立的 opacity 键已废。
        assert_eq!(ring["color"], serde_json::json!("#FF000080"));
        assert!(ring.get("opacity").is_none());
        // cursor_mode=0（遗留"关"快照）不得覆盖新字段的开启态。
        let mut cfg2: Config =
            serde_json::from_str(r#"{ "trail": { "cursor_mode": 0, "hover_enabled": true } }"#)
                .unwrap();
        cfg2.migrate_hover_settings();
        assert!(
            cfg2.trail.hover_enabled,
            "cursor_mode=0 是遗留快照，不该关掉悬浮标"
        );
        // 迁移是幂等的：再跑一遍不覆盖
        cfg.migrate_hover_settings();
        assert_eq!(
            cfg.trail.hover_params["ring"]["outer"],
            serde_json::json!(400)
        );
    }

    #[test]
    fn default_roundtrip() {
        let cfg = Config::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    /// m40：两条清单地址进配置，老配置缺 `store` 段时取默认（不因升级丢地址）。
    /// 选中来源也要能落盘/读回——"方便切换更新源"靠的就是这个。
    #[test]
    fn store_urls_live_in_config_and_old_configs_get_defaults() {
        let cfg = Config::default();
        assert!(
            cfg.store.gitee_url.ends_with("/main/scripts.json"),
            "{}",
            cfg.store.gitee_url
        );
        assert!(
            cfg.store.github_url.ends_with("/main/scripts.json"),
            "{}",
            cfg.store.github_url
        );
        assert_eq!(cfg.store.source, 0);

        // 老配置（没有任何 store 段）⇒ 两条默认地址 + Gitee。
        let old: Config = serde_json::from_str("{}").expect("空对象也要能读");
        assert_eq!(old.store, StoreSettings::default());

        // 用户改地址、切来源 ⇒ 逐字读回。
        let mut custom = cfg.clone();
        custom.store.gitee_url = "https://mirror.example/scripts.json".to_string();
        custom.store.github_url = String::new();
        custom.store.source = 1;
        let json = serde_json::to_string(&custom).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(back.store, custom.store);
        assert_eq!(back.store.urls()[0], "https://mirror.example/scripts.json");
        assert_eq!(back.store.source_index(), 1);
        // 手写成 9 也不许把界面弄崩；`set_source` 夹范围并如实回答"改没改"。
        let mut wild = custom.clone();
        wild.store.source = 9;
        assert_eq!(wild.store.source_index(), 1);
        assert!(wild.store.set_source(9), "9 → 夹成 1，是真的改了");
        assert_eq!(wild.store.source, 1);
        assert!(!wild.store.set_source(9), "已经是 1 ⇒ 没改（不许平白写盘）");
        assert!(wild.store.set_source(0), "1 → 0 是改");
        assert_eq!(wild.store.source, 0);
        assert!(!wild.store.set_source(0), "同一个值 ⇒ 没改（不许平白写盘）");
    }

    #[test]
    fn trail_detail_defaults_match_python() {
        // 锁定与 Python DEFAULT_CONFIG["trail"]["trail_settings"] 的一致性。
        let d = TrailDetail::default();
        assert_eq!(d.fade_duration, 1.0);
        assert_eq!(d.density, 5);
        assert_eq!(d.opacity, 0.5);
        assert_eq!(d.spawn_interval_ms, 100);
        assert_eq!(d.particle_size, 5);
        assert_eq!(d.color_mode, "rainbow");
        assert_eq!(d.fixed_color, "#FFFF00");
    }

    #[test]
    fn panel_hotkey_default_is_enabled() {
        // 与 Python 故意不同（它默认空串=从不生效），见 GlobalSettings::default 的说明。
        assert_eq!(Config::default().global.panel_hotkey, "Ctrl+Shift+D");
        // 老配置里显式写空串仍应尊重——用户就是想要停用。
        let cfg: Config = serde_json::from_str(r#"{ "global": { "panel_hotkey": "" } }"#).unwrap();
        assert_eq!(cfg.global.panel_hotkey, "");
    }

    #[test]
    fn drawing_hotkey_defaults() {
        // 2026-09-27 用户口径：进标注 / 进白板两把键出厂留空（功能走卡片总开关）。
        // 画笔等 9 把功能内部切换键仍是出厂单键。
        let h = DrawingHotkeys::default();
        assert_eq!(h.draw_hk_screen, "");
        assert_eq!(h.draw_hk_white, "");
        assert_eq!(h.draw_hk_pen1, "1");
        assert_eq!(h.draw_hk_eraser, "4");
        assert_eq!(h.draw_hk_text, "9");
        assert_eq!(h.draw_hk_mosaic, "0");
        // 反序列化缺字段也走同一套默认值（serde 的 default = "fn" 路径）
        let cfg: Config = serde_json::from_str(r#"{ "drawing": {} }"#).unwrap();
        assert_eq!(cfg.drawing.hotkeys.draw_hk_text, "9");
        assert_eq!(cfg.drawing.hotkeys.draw_hk_mosaic, "0");
        assert_eq!(cfg.drawing.hotkeys.draw_hk_screen, "");
    }

    /// 出厂留空的两把标注键，在**空值语义**下确实匹配不上任何按键（空 = 停用）。
    #[test]
    fn drawing_entry_hotkeys_empty_by_default_are_inert() {
        use crate::core::events::{KeyCode, Modifiers};
        let hks = Config::default().drawing.hotkeys;
        assert!(
            crate::core::hotkeys::match_drawing(KeyCode::Char('g'), Modifiers::default(), &hks)
                .is_none(),
            "出厂留空 = 停用，不该把 Alt+G 认成「进入标注」"
        );
    }

    #[test]
    fn missing_field_uses_default() {
        let json = r#"{ "general": {} }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.trail, TrailSettings::default());
    }

    #[test]
    fn unknown_field_ignored() {
        let json = r#"{ "future_field": 123, "general": {} }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        // serde 用 Default 造出 GeneralSettings，再把空 {} 盖上去：字段保持出厂值
        assert_eq!(cfg.general.temp_cleanup_days, 0);
    }

    #[test]
    fn atomic_save() {
        let dir = std::env::temp_dir().join("curseen_test_atomic");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let cfg = Config::default();
        let path = dir.join("config.json");
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(&cfg).unwrap();
        fs::write(&tmp, &json).unwrap();
        fs::rename(&tmp, &path).unwrap();

        assert!(path.exists());
        assert!(!tmp.exists());
        let read: Config = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(read, cfg);
        let _ = fs::remove_dir_all(&dir);
    }

    // ── 耐久原子写（`write_durable`，总纲 §7.2）─────────────────────────────
    //
    // 钩子是进程级的一次性注册，所以用静态计数器 + 静态返回值来"遥控"假实现；
    // 测试按 `--test-threads=1` 跑（开发指南第四节），不存在交错。

    static HOOK_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    static HOOK_OK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
    static HOOK_DIR: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

    fn fake_dir_sync(dir: &Path) -> bool {
        HOOK_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Ok(mut g) = HOOK_DIR.lock() {
            *g = Some(dir.to_string_lossy().into_owned());
        }
        HOOK_OK.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 四步都要真的发生：内容逐字节一致、临时文件不留残骸、父目录被同步过。
    #[test]
    fn durable_write_syncs_file_and_dir() {
        let dir = std::env::temp_dir().join("curseen_test_durable");
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("config.json");

        set_dir_sync(fake_dir_sync);
        assert!(
            dir_sync_registered(),
            "钩子注册后必须可见（幂等：只保留先注册的）"
        );
        HOOK_OK.store(true, std::sync::atomic::Ordering::SeqCst);
        let before = HOOK_CALLS.load(std::sync::atomic::Ordering::SeqCst);

        let bytes = b"{\"hello\":\"world\"}";
        Config::write_durable(&dir, &path, bytes).unwrap();

        assert_eq!(fs::read(&path).unwrap(), bytes, "落盘内容必须逐字节一致");
        assert!(
            !path.with_extension("json.tmp").exists(),
            "临时文件必须已被 rename 掉，不能留在目录里"
        );
        assert_eq!(
            HOOK_CALLS.load(std::sync::atomic::Ordering::SeqCst),
            before + 1,
            "父目录 fsync 必须被调用一次"
        );
        let seen = HOOK_DIR.lock().unwrap().clone().unwrap_or_default();
        assert_eq!(
            std::path::Path::new(&seen),
            dir,
            "同步的必须是配置所在目录，不是别的路径"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// 目录 fsync 失败**不得**让保存失败——它是锦上添花的一步。
    #[test]
    fn durable_write_survives_dir_sync_failure() {
        let dir = std::env::temp_dir().join("curseen_test_durable_fail");
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("config.json");

        set_dir_sync(fake_dir_sync);
        HOOK_OK.store(false, std::sync::atomic::Ordering::SeqCst);

        Config::write_durable(&dir, &path, b"{\"a\":1}").unwrap();
        assert!(path.exists(), "目录 fsync 失败也必须留下完整的新文件");
        assert_eq!(fs::read(&path).unwrap(), b"{\"a\":1}");
        HOOK_OK.store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = fs::remove_dir_all(&dir);
    }

    /// 错误要带路径与操作名：日志里只有 `io error: ...` 是没法排查的（总纲 §9.2）。
    #[test]
    fn io_error_carries_path_and_op() {
        let e = CurSeenError::io_at("/tmp/config.json", "rename", std::io::Error::other("x"));
        let msg = e.to_string();
        assert!(msg.contains("rename"), "操作名要在消息里：{msg}");
        assert!(msg.contains("config.json"), "路径要在消息里：{msg}");
    }

    #[test]
    fn python_v1_compat() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/config_v1.json");
        let s = fs::read_to_string(&path).unwrap();
        let cfg: Config = serde_json::from_str(&s).unwrap();

        // trail 字段与预期一致
        assert_eq!(cfg.trail.cursor_outer, 60);
        assert_eq!(cfg.trail.cursor_color, "#ff000080");
        assert_eq!(cfg.trail.trail_effect, "pixie_dust");

        // key_display
        assert_eq!(cfg.key_display.placement, "bottom_right");
        assert_eq!(cfg.key_display.fade_delay, 10);
        assert_eq!(cfg.key_display.font_size, 16);

        // drawing.hotkeys
        assert_eq!(cfg.drawing.hotkeys.draw_hk_screen, "Alt+G");

        // drawing.pens
        assert_eq!(cfg.drawing.pens.len(), 3);
        assert_eq!(cfg.drawing.pens[0].width, 4);

        // general / global
        // 旧 v1 配置没有功能卡开关字段 → 各功能段默认全关
        assert!(!cfg.key_display.enabled);
        assert!(!cfg.drawing.enabled);
        assert!(!cfg.trail_module_active());
    }

    /// 贴图环容量的默认值在两处各写了一份字面量（config 在 core 层、不依赖 runtime），
    /// 这条测试锁住它们一致——否则改一处会让"面板显示 10、实际环是 5"。
    #[test]
    fn pin_ring_cap_matches_runtime_default() {
        assert_eq!(default_pin_ring_cap(), crate::runtime::snap::PIN_RING_CAP);
        assert_eq!(
            default_pin_ring_cap(),
            ScreenshotSettings::default().pin_ring_cap
        );
    }

    /// 新增的截图键必须**有默认值**：老配置文件里没有这些键，反序列化要能补齐，
    /// 且补齐后的值与"出厂默认"逐字段相等（否则面板首帧会误报"配置有改动"）。
    ///
    /// ★ 2026-09-17：这里还钉住"被删掉的键不会被当成必填"——老配置里带着
    /// `hotkey_shot_last` / `save_dir` / `notify` 等已删除字段时，serde 默认忽略未知键，
    /// 反序列化必须照常成功（用户升级后不能因为配置里有旧键就起不来）。
    #[test]
    fn screenshot_settings_backfill_matches_factory_default() {
        let partial: ScreenshotSettings = serde_json::from_str("{}").expect("空对象应能反序列化");
        assert_eq!(partial, ScreenshotSettings::default());
        // ★ 2026-09-26 起三把快捷键出厂留空（用户口径），补默认也一样是空串。
        assert!(partial.hotkey.is_empty());
        assert!(partial.hotkey_pin.is_empty());
        assert!(partial.hotkey_pin_clipboard.is_empty());
        assert_eq!(partial.pin_ring_cap, 10);
    }

    /// 旧配置（含已删除的键）必须还能读：未知键被忽略，不报错、不影响其余字段。
    #[test]
    fn legacy_keys_of_removed_features_are_ignored_on_load() {
        let legacy = r#"{
            "hotkey": "Alt+S",
            "hotkey_shot_last": "Alt+Shift+S",
            "pin_hover_outline": true,
            "clipboard_poll": false,
            "save_dir": "D:/shots",
            "copy_to_clipboard": false,
            "notify": false,
            "pin_ring_cap": 5
        }"#;
        let s: ScreenshotSettings = serde_json::from_str(legacy).expect("旧配置应能读");
        assert_eq!(s.hotkey, "Alt+S");
        assert_eq!(s.pin_ring_cap, 5, "保留字段仍按配置值生效");
        // 已删除的键被忽略、其余缺项补出厂默认——整结构逐字段核对，避免"少了一个键
        // 就整段回落默认"这种静默降级。
        assert_eq!(
            s,
            ScreenshotSettings {
                enabled: false,
                hotkey: "Alt+S".into(),
                hotkey_pin: default_hk_screenshot_pin(),
                hotkey_pin_clipboard: default_hk_pin_clipboard(),
                hotkey_scroll: default_hk_scroll(),
                hotkey_scroll_wide: default_hk_scroll_wide(),
                pin_ring_cap: 5,
            }
        );
    }

    /// ★ 鼠标穿透默认开是有意的取舍：104 键铺开要占近千像素宽，默认把这么大一块的鼠标
    ///   事件吃掉太具侵入性；要"点键盘输入"就取消勾选（能力一直在，只是默认不接管鼠标）。
    ///
    /// 「窗口行为」里的 总在最上 / 全屏避让 已上移为**全局** `Config::overlay`（勾一次对
    /// 所有功能生效），不再属于虚拟键盘 —— 见 [`OverlaySettings::default`]。
    #[test]
    fn vk_window_behaviour_defaults() {
        let vk = VirtualKeyboardSettings::default();
        assert!(vk.click_through, "鼠标穿透默认开");
        let overlay = OverlaySettings::default();
        assert!(overlay.always_on_top, "总在最上默认开");
        assert!(
            !overlay.avoid_fullscreen,
            "全屏避让默认关（屏幕工具默认盖住全屏）"
        );
    }

    /// ★ 出厂的键帽大小要落在面板量程内 —— 否则滑块初值被 clamp 到边界，
    ///   面板显示的数与真正生效的数不一致（出厂 34 卡在量程之外就是这么来的）。
    ///   量程 2026-09-18 由用户定为 10~80（见 `vk_config::Bind::preset_range`）。
    #[test]
    fn vk_factory_key_size_is_a_usable_size() {
        let vk = VirtualKeyboardSettings::default();
        assert!(
            vk.key_size >= 10 && vk.key_size <= 80,
            "出厂键帽大小 {} 应在 10~80（1u 的逻辑像素）内",
            vk.key_size
        );
    }

    /// ★ 回归锁（2026-09-26 用户定档）：虚拟键盘出厂那一整组值。
    ///
    /// 这一组是"打开就用、不用调"的观感，散在十来个 `default_vk_*` 函数里，
    /// 改一处就会悄悄偏离定档 —— 逐项钉住，谁改谁知道。
    ///
    /// ★ 圆角必须**同时**钉 `assets/VirtualKeyboards/ansi_104.json` 的
    ///   `defaults.cornerRadius`：出厂字段与方案值相等时走方案兜底（见
    ///   `vk_config::f32_of`），只改一处等于没改。
    #[test]
    fn vk_factory_preset_is_the_user_approved_one() {
        let vk = VirtualKeyboardSettings::default();
        assert_eq!(vk.placement, "bottom_right", "位置：右下");
        assert_eq!(vk.right_offset, 0.5, "水平偏移 0.5%");
        assert_eq!(vk.bottom_offset, 5.0, "垂直偏移 5%");
        assert!(vk.click_through, "鼠标穿透：选择");
        assert_eq!(vk.key_size, 32, "键帽大小 32");
        assert_eq!(vk.gap, 0, "键间空隙 0");
        assert_eq!(vk.corner_radius, 0, "圆角 0（直角）");
        assert_eq!(vk.opacity, 1.0, "不透明度 1");
        assert_eq!(vk.hold, 0.1, "高亮保持 0.1s");
        assert_eq!(vk.font_size, 14, "字号 14");
        // 数组是 [R,G,B,A]，与十六进制串一一对应（口径见 `core::color`）。
        assert_eq!(vk.key_color, [0, 0, 0, 0xBF], "键帽色 #000000BF");
        assert_eq!(vk.press_color, [0, 0, 255, 0xBF], "按下色 #0000FFBF");
        assert_eq!(vk.border_color, [0, 255, 255, 0xBF], "描边色 #00FFFFBF");
        assert_eq!(vk.text_color, [255, 255, 0, 255], "文字色 #FFFF00");
    }
}
