//! 拖尾特效应答的接口。对应 Python 的 TrailEffect 抽象基类。
//!
//! 与 Python 的三点结构差异（都是为了确定性，功能等价）：
//! 1. step(dt) 的 dt 恒为 FIXED_DT（1/120 秒），不是可变的帧间隔
//! 2. elapsed 由框架统一维护并传进来，特效自己不累加
//! 3. RNG 由框架持有并按（特效id, 粒子序号）派生种子
//!
//! ★ 2.0 起所有内置拖尾特效都改成了 Lua 脚本（见 `assets/MouseTrail/`），所以
//! `EFFECT_REGISTRY` 置空——运行期走 `controller::load_lua_effect` 直接按脚本 id 加载。
//! 这里只保留 trait 与桥接器 `LuaEffect`。

use std::collections::HashMap;

use serde_json::Value;

use crate::core::draw::DrawList;
use crate::core::events::MouseButton;
use crate::core::geometry::Rect;
use crate::core::math::Rng;

use super::settings::EffectDefaults;

pub mod lua_effect;

/// 特效可用的、由控制器提供的只读/受限环境。
pub struct EffectCtx<'a> {
    pub rng: &'a mut Rng,
    /// 覆盖层逻辑尺寸。
    pub screen: Rect,
    /// 框架统一维护的累计秒数。
    pub elapsed: f32,
    /// 本次 spawn 应该生成几个粒子（由 spawn 累加器算好，特效不自己判断间隔）。
    pub spawn_batch: u32,
    /// ★ 真实墙钟毫秒（脚本全局 `wall_ms`）。四类宿主一致注入——静止检测 / 超时淡出
    ///   必须用它：`elapsed` 在不出帧期间不推进，测不出"静止了多久"。
    pub wall_ms: f64,
}

/// 拖尾特效应答的接口。
pub trait TrailEffect {
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    fn default_settings() -> EffectDefaults
    where
        Self: Sized;
    /// 应用一组参数。参数表是"本特效自己的键 → 值"（已从配置里按 id 命名空间过滤好）。
    fn apply(&mut self, params: &HashMap<String, Value>);
    /// 鼠标移动。s 是已经过采样合并的单个采样点。
    fn on_move(&mut self, s: super::sample::MouseSample, ctx: &mut EffectCtx);
    /// 指针按下。默认什么都不做——不关心按下的特效（绝大多数）无需实现。
    ///
    /// ★ 四类宿主同契约：拖尾也转发按下事件，脚本因此可以写"按下时在光标处爆一次"，
    ///   而不再只有点击效果宿主能收到按下。
    fn on_press(&mut self, _x: f32, _y: f32, _t: f32) {}
    /// 指针抬起。用于按住态视觉（枪口后坐、加速、连发）。默认忽略。
    fn on_mouse_up(&mut self, _button: MouseButton, _x: f32, _y: f32, _t: f32) {}
    /// 滚轮（`delta` 一格 = ±120；`horizontal` = 倾斜滚轮）。默认忽略。
    ///
    /// 只有脚本声明了 `on_wheel` 时宿主才会收到滚轮（钩子按需武装，见
    /// `platform::input::mouse::set_script_wheel_armed`）。
    fn on_wheel(&mut self, _delta: i32, _horizontal: bool, _x: f32, _y: f32, _t: f32) {}
    /// 脚本是否声明了 `on_wheel`（决定钩子要不要转发普通滚轮）。
    fn wants_wheel(&self) -> bool {
        false
    }
    /// 固定步长推进。只能改内部状态，不能画。
    fn step(&mut self, dt: f32, ctx: &mut EffectCtx);
    /// 绘制。每个特效把指令 push 进同一个 `DrawList`。
    fn draw(&mut self, list: &mut DrawList);
    /// 是否还有活动内容（决定帧循环要不要继续请求重绘）。
    fn is_active(&self) -> bool;
    fn clear(&mut self);
    /// 脚本是否要求隐藏系统指针（激光笔类特效）。默认 `false`；Lua 脚本在全局声明
    /// `hide_cursor = true` 即生效，运行时据此换透明光标。
    fn hide_cursor(&self) -> bool {
        false
    }
    /// 返回默认设置（通过 trait object 调用）。
    fn defaults(&self) -> EffectDefaults;
}

/// 注册表条目（历史遗留，现已弃用——见模块头注释）。
///
/// 保留类型以便将来若想内置"引擎级"特效（不走脚本 VM、零开销）时直接挂进来，
/// 当前为空。
pub struct EffectEntry {
    pub id: &'static str,
    pub name: &'static str,
    pub make: fn() -> Box<dyn TrailEffect>,
}

/// 注册表。顺序即 UI 下拉顺序，不要动。当前为空：所有特效都来自 `assets/MouseTrail/` 的脚本。
pub static EFFECT_REGISTRY: &[EffectEntry] = &[];

/// 由 hue [0,1] 与 alpha [0,255] 生成 Argb 颜色（rainbow 模式）。
pub fn hue_color(hue: f32, alpha: u8) -> crate::core::color::Argb {
    let h = (hue * 360.0).rem_euclid(360.0);
    let c = crate::core::color::Rgba::from_hsv(h, 1.0, 1.0, alpha);
    crate::core::color::Argb::new(alpha, c.r, c.g, c.b)
}

/// 生命周期 alpha：clamp(life/max_life, 0, 1)。
pub fn life_alpha(life: f32, max_life: f32) -> f32 {
    (life / max_life).clamp(0.0, 1.0)
}
