//! 悬浮标运行时。★ 2.0 起每种类型一个**独立 Lua 脚本**（`assets/lua/HoverDot/<type>.lua`），
//! 参数 schema 与绘制全由脚本定义；「图片」类型（stem=png）的位图/动画帧由控制器预加载，
//! 脚本经 `ctx:image(...)` 贴出（见 `scripting::api_draw`）。
//!
//! ★ 2026-09-26 多开改造：启用列表（`config.trail.hover_active`）里**每个脚本一个实例**，
//! 全部跟随鼠标同时显示。悬浮标本体只持有宿主环境（开关/指针/时钟/屏幕），脚本状态
//! （引擎/参数/媒体/续帧请求）全在 [`HoverDotInstance`] 里。
//!
//! ★ 实例 id 相对 `assets/lua`（内置脚本带 `HoverDot/` 前缀）——引擎根统一是 lua 根，
//! id 前缀就是路径。`scripts/` 区的复合脚本（`role="composite"`）**不**走这里，见
//! `modules::trail::companion`（2026-09-26 用户口径：特效脚本不收录进悬浮标下拉）。
//!
//! 静态 PNG 走 `load_png`（原样宽高）；GIF 走 `core::gif` 逐帧解码，动画期间报
//! `has_animation()` = true 以驱动持续重绘。脚本缺失/损坏时该实例退化（png 有内建 blit
//! 兜底），绝不拖垮其它实例与进程。
//!
//! ★ 2026-09-27 四宿主统一：**驱动序列只有一份实现**（[`FxCore`](super::fx_core)）。
//!   此前本宿主**只调 `draw`**——从不发 `on_move`、从不调 `step`。像流萤那样"靠
//!   `on_move` 累积粒子、靠 `step` 推进动画"的脚本，在这里等于只给画笔不给事件，
//!   永远画不出东西（用户报的「悬浮标里没效果」根因）。现在与拖尾 / 点击效果同一序列：
//!   指针动了发 `on_move` → 每帧按 `FIXED_DT` 子步进 `step` → `draw`。
//! ★ `elapsed` 单位**秒**（此前是毫秒，与另外三类宿主差 1000 倍）。

use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use serde_json::Value as Json;

use crate::core::draw::DrawList;
use crate::core::geometry::{Point, Rect};
use crate::core::gif::GifAnim;
use crate::core::image::RgbaBitmap;
use crate::scripting::api_draw;

use super::fx_core::FxCore;

/// 单个悬浮标实例 = 一个启用的脚本。
///
/// ★ 状态全部实例私有：脚本核心（引擎 / 参数 / 环境 / 事件）、png 媒体、GIF 帧钟、
/// 续帧请求。多实例共享的只有宿主环境（指针位置 / 时钟 / 墙钟 / 屏幕尺寸——在
/// [`HoverDot`] 上）。
pub struct HoverDotInstance {
    /// 脚本 id = 相对 `assets/lua` 的路径 stem（`HoverDot/solid`、`scripts/my_dot`）。
    pub id: String,
    /// 统一实例核心（`None` = 脚本加载失败，退化成内建 blit 兜底）。
    core: Option<FxCore>,
    /// ★ 宿主留一份参数副本：脚本核心为 `None` 时（加载失败），内建 blit 兜底仍要读
    ///   `scale`/`opacity`。参数不能只存在核心里——否则"图还在、缩放却丢了"。
    params: HashMap<String, Json>,
    /// 「图片」类型的静态位图（PNG；GIF 时为 None，动画走 `anim`）。
    pub image: Option<Rc<RgbaBitmap>>,
    /// 「图片」类型的 GIF 动画（Some = 持续重绘）。
    pub anim: Option<GifAnim>,
    /// 动画累计毫秒。
    anim_acc_ms: f32,
    /// 脚本上一帧 `draw` 返回的续帧请求（毫秒）。None = 不需要额外帧。
    next_frame_ms: Option<u32>,
}

/// 本宿主在 Lua 全局 `host` 里的标识。
const HOST: &str = "hover";

/// 把脚本 `draw` 的返回值解释成"多少毫秒后再来一帧"。
///
/// 只认正数：nil / 0 / 负数 / 非数字一律按"不请求"处理（老脚本返回空 → 静止零功耗）。
/// 上限 5000ms 兜住脚本写错（返回 1e9 会让覆盖层睡死）。
fn frame_request_ms(v: mlua::Value) -> Option<u32> {
    let ms = match v {
        mlua::Value::Integer(n) => n as f64,
        mlua::Value::Number(n) => n,
        _ => return None,
    };
    if ms.is_finite() && ms > 0.0 {
        Some(ms.clamp(1.0, 5000.0) as u32)
    } else {
        None
    }
}

/// id 的最后一段（`HoverDot/solid` → `solid`）。png 媒体特判与显示都用它。
/// ★ 按 `/` 字节切分，无 UTF-8 边界问题。
fn id_tail(id: &str) -> &str {
    id.rsplit('/').next().unwrap_or(id)
}

impl HoverDotInstance {
    /// 加载脚本构造实例（引擎根 = 出厂 `assets/lua`）。
    pub fn load(id: &str) -> Self {
        Self::load_in(&crate::core::paths::lua_ext_dir(), id)
    }

    /// 指定引擎根加载。脚本缺失/损坏时核心置 None（png 类型仍有内建 blit 兜底，
    /// 其余类型该实例静默无图 + 启动日志），**不 panic、不拖垮其它实例**。
    ///
    /// ★ 生产走 [`Self::load`] 的出厂根；测试用它指向临时目录（与 `Companion::load_in`
    ///   同一套路）。根统一是 lua 根——id 自带目录前缀，归属由目录决定。
    pub fn load_in(root: &Path, id: &str) -> Self {
        let core = match FxCore::load(root, id, HOST) {
            Ok(c) => Some(c),
            Err(e) => {
                log::warn!("悬浮标脚本 {id}.lua 加载失败：{e}");
                None
            }
        };
        Self {
            id: id.to_string(),
            core,
            params: HashMap::new(),
            image: None,
            anim: None,
            anim_acc_ms: 0.0,
            next_frame_ms: None,
        }
    }

    /// 是否「图片」类型（stem == "png"）。
    pub fn is_png(&self) -> bool {
        id_tail(&self.id) == "png"
    }

    /// 本实例脚本是否声明 `hide_cursor`（激光笔类）。
    pub fn wants_cursor_hidden(&self) -> bool {
        self.core.as_ref().is_some_and(|c| c.hide_cursor)
    }

    /// ★ 仅测试：抹掉脚本核心，模拟"脚本加载失败"（png 的内建 blit 兜底路径）。
    #[cfg(test)]
    pub(crate) fn drop_core(&mut self) {
        self.core = None;
    }

    /// 是否 GIF 动画进行中。
    pub fn has_animation(&self) -> bool {
        self.anim.is_some()
    }

    /// 注入该实例的参数表（缺键由脚本内 `or 默认值` 兜底）。
    ///
    /// ★ 顺带作废上一帧的续帧请求：换参数（面板拖动）时旧脚本状态已经不适用，
    ///   留着会把帧门钉成空转。
    pub fn set_params(&mut self, params: HashMap<String, Json>) {
        if let Some(c) = self.core.as_mut() {
            c.set_params(params.clone());
        }
        self.params = params;
        self.next_frame_ms = None;
    }

    /// 「图片」类型：设置静态位图（PNG）。
    pub fn set_image(&mut self, img: Option<Rc<RgbaBitmap>>) {
        self.image = img;
    }

    /// 「图片」类型：设置 GIF 动画（与静态位图互斥）。
    pub fn set_anim(&mut self, anim: Option<GifAnim>) {
        self.anim = anim;
        self.anim_acc_ms = 0.0;
    }

    /// 直接摆续帧请求（**仅测试用**）：帧门那条判据需要一个不经过脚本的入口。
    #[cfg(test)]
    pub(crate) fn set_next_frame_ms(&mut self, ms: Option<u32>) {
        self.next_frame_ms = ms;
    }

    /// 生成该实例的绘制指令。返回本帧是否真的画了东西。
    ///
    /// ★ 驱动序列与拖尾 / 点击效果**逐字同源**（`FxCore`）：指针动了发 `on_move` →
    ///   按 `FIXED_DT` 子步进 `step` → `draw`。此前这里只调 `draw`，事件驱动型脚本
    ///  （流萤那类）在悬浮标下永远画不出东西。
    ///
    /// 宿主环境（指针 / 时钟 / 墙钟 / 屏幕）由 [`HoverDot::draw`] 统一取好传入。
    fn draw(
        &mut self,
        pos: Point,
        moved: bool,
        dt_s: f32,
        screen: (f32, f32),
        wall_ms: f64,
        list: &mut DrawList,
    ) -> bool {
        // 每帧先作废上一帧的请求：脚本本帧不返回就不续帧（淡出结束即停）。
        self.next_frame_ms = None;
        if self.is_png() {
            return self.draw_image_media(pos, moved, dt_s, screen, wall_ms, list);
        }
        let Some(core) = self.core.as_mut() else {
            return false;
        };
        core.set_screen(screen.0, screen.1);
        core.set_wall_ms(wall_ms);
        // ★ 图片对所有实例可用（不只 `png`）：脚本 `ctx:image(...)` 读这张图。
        //   没配图时是 None，`ctx:image` 不产出指令——脚本自己决定兜底形态。
        api_draw::set_hover_image(self.image.clone());
        // ★ 只在这一帧真的移动过才发 on_move：与拖尾"按采样点转发"同口径，
        //   否则静止时每帧补发会把"累积池"型脚本按帧率灌满。
        if moved {
            core.move_to(pos.x, pos.y, None);
        }
        core.advance(dt_s);
        let out = core.render(list);
        api_draw::set_hover_image(None);
        self.next_frame_ms = frame_request_ms(out.frame_request);
        out.produced
    }

    /// 「图片」类型：推进 GIF 帧并把当前位图交给脚本 `ctx:image(...)` 贴出；
    /// 脚本缺失/损坏时退化成内建 blit（图仍能显示）。
    fn draw_image_media(
        &mut self,
        pos: Point,
        moved: bool,
        dt_s: f32,
        screen: (f32, f32),
        wall_ms: f64,
        list: &mut DrawList,
    ) -> bool {
        // 帧推进（dt 秒 → 动画内部按毫秒记）。
        let dt_ms = (dt_s * 1000.0).max(0.0);
        if let Some(anim) = &self.anim {
            self.anim_acc_ms = (self.anim_acc_ms + dt_ms) % anim.total_ms.max(1.0);
        }
        let current: Option<Rc<RgbaBitmap>> = if let Some(anim) = &self.anim {
            let idx = anim.frame_index_at(self.anim_acc_ms);
            Some(anim.frames[idx].clone())
        } else {
            self.image.clone()
        };
        let Some(bmp) = current else {
            return false;
        };

        let num = |key: &str, default: f64| {
            self.params
                .get(key)
                .and_then(|v| v.as_f64())
                .unwrap_or(default) as f32
        };
        // ★ 「缩放」而不是「尺寸」（2026-09-22 改）：按图片**原始宽高**整体乘 scale，
        //   宽高比因此不被破坏，鼠标落在图片正中。早先按正方形边长铺，非方图必然变形。
        let scale = num("scale", 1.0).clamp(0.01, 1.0);
        let opacity = num("opacity", 1.0).clamp(0.0, 1.0);

        api_draw::set_hover_image(Some(bmp.clone()));
        let produced = if let Some(core) = self.core.as_mut() {
            core.set_screen(screen.0, screen.1);
            core.set_wall_ms(wall_ms);
            if moved {
                core.move_to(pos.x, pos.y, None);
            }
            core.advance(dt_s);
            core.render(list).produced
        } else {
            // 内建兜底：位图以鼠标为中心、按原始宽高 × 缩放铺开。
            let w = bmp.w as f32 * scale;
            let h = bmp.h as f32 * scale;
            let rect = Rect::new(pos.x - w * 0.5, pos.y - h * 0.5, w, h);
            list.image(bmp, rect, opacity);
            true
        };
        api_draw::set_hover_image(None);
        produced
    }
}

/// 悬浮标本体：宿主环境（开关 / 指针 / 时钟 / 屏幕）+ 启用脚本实例列表。
pub struct HoverDot {
    /// 卡片开关（唯一开关）。
    pub enabled: bool,
    /// 鼠标位置（悬浮标中心），由控制器按采样更新。
    pub pos: Point,
    /// 屏幕尺寸（脚本环境变量）。
    screen: (f32, f32),
    /// 脚本时间戳（**秒**；与拖尾共享时钟同源，四类宿主一致）。
    elapsed: f32,
    /// 上一次 `draw` 时的时间戳（帧间 dt 的来源）。
    last_elapsed: f32,
    /// ★ 上一次 `draw` 时的指针位置：只有位置变了才发 `on_move`（与拖尾"按采样点
    /// 转发"同口径，静止时不给脚本灌点）。
    last_pos: Option<(f32, f32)>,
    /// 真实墙钟毫秒（脚本全局 `wall_ms`），由 Runtime 每帧注入。
    ///
    /// ★ 与 `elapsed` 不是一回事：后者无帧期间不推进，测不出"静止了多久"。
    wall_ms: f64,
    /// 启用脚本实例（顺序 = `hover_active` 列表顺序）。
    pub instances: Vec<HoverDotInstance>,
}

impl HoverDot {
    pub fn new() -> Self {
        Self {
            enabled: false,
            pos: Point::ZERO,
            screen: (1920.0, 1080.0),
            elapsed: 0.0,
            last_elapsed: 0.0,
            last_pos: None,
            wall_ms: 0.0,
            instances: Vec::new(),
        }
    }

    /// 按启用列表同步实例（引擎根 = 出厂 `assets/lua`）。
    pub fn sync_instances(&mut self, active: &[String]) {
        self.sync_instances_at(&crate::core::paths::lua_ext_dir(), active);
    }

    /// 指定引擎根的对账版本（测试用临时脚本目录）。
    ///
    /// 同 id 保留（引擎不重载——参数滑块每帧都改，不能跟着重载）、新增的加载、消失的
    /// 丢弃。顺序与列表一致。
    pub fn sync_instances_at(&mut self, root: &Path, active: &[String]) {
        // 先整体摘下，再按列表顺序重组：仍启用的从旧表里捞（保引擎/参数），缺的现载。
        let mut old: Vec<HoverDotInstance> = std::mem::take(&mut self.instances);
        self.instances = active
            .iter()
            .map(|id| {
                if let Some(i) = old.iter().position(|inst| inst.id == *id) {
                    old.swap_remove(i)
                } else {
                    HoverDotInstance::load_in(root, id)
                }
            })
            .collect();
    }

    /// 按实例 id 注入参数表（不存在该实例 = 忽略；实例持有期参数每版本重放）。
    pub fn set_params_of(&mut self, id: &str, params: HashMap<String, Json>) {
        if let Some(inst) = self.instances.iter_mut().find(|i| i.id == id) {
            inst.set_params(params);
        }
    }

    /// 是否要求隐藏系统指针：任一启用实例声明 `hide_cursor` 且悬浮标处于启用态。
    pub fn wants_cursor_hidden(&self) -> bool {
        self.enabled && self.instances.iter().any(|i| i.wants_cursor_hidden())
    }

    /// 是否任一实例有 GIF 动画进行中。
    pub fn has_animation(&self) -> bool {
        self.instances.iter().any(|i| i.has_animation())
    }

    /// 屏幕尺寸（脚本环境变量）。
    pub fn set_screen(&mut self, w: f32, h: f32) {
        self.screen = (w, h);
    }

    /// 时间戳（**秒**；帧推进 dt 的来源）。控制器喂的是拖尾那条共享时钟——
    /// 四类宿主的 `elapsed` 因此逐位同源。★ 此前这里喂的是毫秒，差 1000 倍。
    pub fn set_time(&mut self, elapsed: f32) {
        self.elapsed = elapsed;
    }

    /// 真实墙钟毫秒（脚本 `wall_ms`）。★ 静止检测 / 超时淡出必须用它，别用 `elapsed`。
    pub fn set_wall_ms(&mut self, ms: f64) {
        self.wall_ms = ms;
    }

    /// 脚本请求的续帧间隔（毫秒）：各实例 `draw` 返回值解释结果的**合并**（取最大，
    /// 只要任一实例还要帧就得画）。None = 全都不需要。
    ///
    /// ★ 未启用时一律 None：脚本停画之后残留的请求会把帧门钉成空转。
    pub fn next_frame_ms(&self) -> Option<u32> {
        if !self.enabled {
            return None;
        }
        self.instances.iter().filter_map(|i| i.next_frame_ms).max()
    }

    /// 生成绘制指令。返回本帧是否真的画了东西（脏区/续帧判据用）。
    ///
    /// ★ 帧间 dt 由**时间差**算（秒），指针是否移动由**坐标差**算：两者一起决定
    ///   这一帧喂给脚本的 `step` 时长与 `on_move` 有无。
    pub fn draw(&mut self, list: &mut DrawList) -> bool {
        if !self.enabled {
            return false;
        }
        let dt_s = (self.elapsed - self.last_elapsed).max(0.0);
        self.last_elapsed = self.elapsed;
        let cur = (self.pos.x, self.pos.y);
        // 首次绘制也算"移动"：脚本需要那一次 on_move 来落定出生点 / 记下首个位置。
        let moved = self.last_pos != Some(cur);
        self.last_pos = Some(cur);
        let (pos, screen, wall_ms) = (self.pos, self.screen, self.wall_ms);
        let mut drew = false;
        for inst in &mut self.instances {
            drew |= inst.draw(pos, moved, dt_s, screen, wall_ms, list);
        }
        drew
    }

    /// ★ 仅测试：给（全部）png 实例设置静态位图。多开改造后媒体在实例上，
    /// 测试不再能直接摆 `HoverDot` 的字段——统一走这个入口注入。
    #[cfg(test)]
    pub fn set_png_image(&mut self, img: Option<Rc<RgbaBitmap>>) {
        for inst in self.instances.iter_mut().filter(|i| i.is_png()) {
            inst.set_image(img.clone());
        }
    }
}

impl Default for HoverDot {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::draw::DrawCmd;
    use crate::core::draw::{Paint, Shape};
    use crate::core::paths::hoverdot_dir;
    use crate::scripting::engine::ScriptingEngine;

    /// 造一个最小但真实的悬浮标脚本（实心圆），验证脚本驱动绘制端到端可用。
    fn write_solid_script(dir: &std::path::Path) {
        std::fs::write(
            dir.join("solid.lua"),
            r##"
effect_name = "实心圆"
function schema()
  return {
    { key = "diameter", label = "直径", type = "int", min = 4, max = 1000, step = 1, default = 60 },
    { key = "opacity", label = "透明度", type = "float", min = 0, max = 1, step = 0.01, default = 1.0 },
    { key = "color", label = "颜色", type = "color", default = "#FF0000" },
  }
end
function draw(ctx, params)
  local r, g, b = hex_to_rgb(params.color or "#FF0000")
  local d = params.diameter or 60
  local a = math.floor((params.opacity or 1.0) * 255)
  ctx:circle(mouse_x, mouse_y, d / 2, r, g, b, a)
end
"##,
        )
        .unwrap();
    }

    fn solid_bitmap(rgb: [u8; 3]) -> Rc<RgbaBitmap> {
        Rc::new(RgbaBitmap::new(1, 1, vec![rgb[0], rgb[1], rgb[2], 255]).unwrap())
    }

    /// 造一张 w×h 的位图（宽高要能被断言读出来，故与 1×1 的 `solid_bitmap` 分开）。
    fn sized_bitmap(w: u32, h: u32) -> Rc<RgbaBitmap> {
        Rc::new(RgbaBitmap::new(w, h, vec![255u8; (w * h * 4) as usize]).unwrap())
    }

    /// 取本帧那条图片指令的目标矩形（兜底 blit 与脚本 `ctx:image` 都只产它）。
    fn image_dest(list: &DrawList) -> crate::core::geometry::Rect {
        match list.cmds().first().unwrap() {
            DrawCmd::Image { dest, .. } => *dest,
            other => panic!("应为图片指令，实际 {other:?}"),
        }
    }

    /// 本帧那个**圆环**（`ctx:ring` → 描边圆）的不透明度；没有环 = None。
    fn ring_alpha(list: &DrawList) -> Option<u8> {
        list.cmds().iter().find_map(|c| match c {
            DrawCmd::Stroke {
                shape: Shape::Circle { .. },
                paint: Paint::Solid(color, _),
                ..
            } => Some(color.a()),
            _ => None,
        })
    }

    /// 本帧那个**圆环**的圆心（「晃动定位」的环必须落在当前指针上，见需求：环跟着鼠标走）。
    fn ring_center(list: &DrawList) -> Option<(f32, f32)> {
        list.cmds().iter().find_map(|c| match c {
            DrawCmd::Stroke {
                shape: Shape::Circle { center, .. },
                ..
            } => Some((center.x, center.y)),
            _ => None,
        })
    }

    /// 本帧有没有**实心圆**（`ctx:circle`）——「晃动定位」用它当过"假指针"，已删除。
    fn has_filled_circle(list: &DrawList) -> bool {
        list.cmds().iter().any(|c| {
            matches!(
                c,
                DrawCmd::Fill {
                    shape: Shape::Circle { .. },
                    ..
                }
            )
        })
    }

    #[test]
    fn disabled_draws_nothing() {
        let mut h = HoverDot::new();
        h.enabled = false;
        h.sync_instances(&["HoverDot/png".into()]);
        h.instances[0].set_image(Some(solid_bitmap([255, 0, 0])));
        let mut list = DrawList::new();
        assert!(!h.draw(&mut list));
        assert!(list.cmds().is_empty());
    }

    #[test]
    fn png_without_image_draws_nothing() {
        let mut h = HoverDot::new();
        h.enabled = true;
        h.sync_instances(&["HoverDot/png".into()]);
        let mut list = DrawList::new();
        assert!(!h.draw(&mut list));
    }

    #[test]
    fn png_without_engine_falls_back_to_blit() {
        // 脚本加载失败（引擎 None）时内建 blit 兜底：图仍能显示。
        let mut h = HoverDot::new();
        h.enabled = true;
        h.sync_instances(&["HoverDot/png".into()]);
        // 模拟脚本加载失败（核心 None）→ 走内建 blit 兜底。
        h.instances[0].drop_core();
        h.pos = Point::new(500.0, 400.0);
        // ★ 4×2 的图 + 缩放 0.5 → 2×1：宽高比被保住（旧实现会铺成 100×100 的正方形）。
        h.instances[0].set_image(Some(sized_bitmap(4, 2)));
        h.instances[0].set_params(HashMap::from([
            ("scale".to_string(), serde_json::json!(0.5)),
            ("opacity".to_string(), serde_json::json!(0.5)),
        ]));
        let mut list = DrawList::new();
        assert!(h.draw(&mut list));
        let dest = image_dest(&list);
        assert!((dest.w - 2.0).abs() < 1e-3, "宽 = 4 × 0.5，实际 {}", dest.w);
        assert!((dest.h - 1.0).abs() < 1e-3, "高 = 2 × 0.5，实际 {}", dest.h);
        // 鼠标落在图片中心。
        let (cx, cy) = (dest.x + dest.w / 2.0, dest.y + dest.h / 2.0);
        assert!((cx - 500.0).abs() < 1e-3 && (cy - 400.0).abs() < 1e-3);
    }

    /// 脚本没给 `scale`（或给的是旧配置的 `size`）→ 按原始尺寸 1:1 显示，不再塌成 60px。
    #[test]
    fn png_fallback_defaults_to_the_natural_size() {
        let mut h = HoverDot::new();
        h.enabled = true;
        h.sync_instances(&["HoverDot/png".into()]);
        h.instances[0].drop_core();
        h.pos = Point::new(10.0, 20.0);
        h.instances[0].set_image(Some(sized_bitmap(30, 12)));
        h.instances[0].set_params(HashMap::new());
        let mut list = DrawList::new();
        assert!(h.draw(&mut list));
        let dest = image_dest(&list);
        assert!((dest.w - 30.0).abs() < 1e-3 && (dest.h - 12.0).abs() < 1e-3);
    }

    #[test]
    fn lua_script_drives_drawing_end_to_end() {
        let dir = std::env::temp_dir().join(format!("curseen_hover_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_solid_script(&dir);

        let mut h = HoverDot::new();
        h.enabled = true;
        h.pos = Point::new(300.0, 200.0);
        h.set_screen(1920.0, 1080.0);
        h.sync_instances(&["HoverDot/solid".into()]); // 真实脚本；仅验证不崩溃
        let mut list = DrawList::new();
        let _ = h.draw(&mut list);

        // 直接用引擎走一遍，验证 Lua → DrawList 全链路
        let engine = ScriptingEngine::new(&dir).unwrap();
        engine.load_script("solid").unwrap();
        crate::scripting::api_draw::set_draw_target(&mut list);
        engine
            .update_env(
                (300.0, 200.0),
                (300.0, 200.0),
                0.0,
                (1920.0, 1080.0),
                "hover",
            )
            .unwrap();
        engine
            .call_draw(&HashMap::from([
                ("diameter".to_string(), serde_json::json!(80)),
                ("opacity".to_string(), serde_json::json!(1.0)),
                ("color".to_string(), serde_json::json!("#FF0000")),
            ]))
            .unwrap();
        crate::scripting::api_draw::clear_draw_target();
        assert!(!list.cmds().is_empty(), "Lua 脚本应产出绘制指令");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ★★ 用户口径的判据：「悬浮标里没有效果」。
    ///
    /// 事件驱动型脚本（靠 `on_move` 累积点、靠 `step` 推进动画）在旧宿主下**只被调
    /// `draw`**——等于只给画笔不给事件，永远画不出东西。现在悬浮标与拖尾 / 点击效果
    /// 走同一条序列（`on_move` → `step` → `draw`），这里把"事件来了才画得出来"钉成判据。
    #[test]
    fn hover_host_feeds_on_move_and_step_not_just_draw() {
        let dir = std::env::temp_dir().join(format!("curseen_hover_fx_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("pool.lua"),
            r##"
effect_name = "池"
pts = {}
age = 0
function on_move(x, y, t, p) pts[#pts+1] = {x, y} end
function step(dt, t, p) age = age + dt end
function draw(ctx, p)
  -- ★ 没有任何事件 = 池是空的 = 一行也画不出（正是"悬浮标没效果"的观感）
  if #pts == 0 or age <= 0 then return end
  for i = 1, #pts do ctx:circle(pts[i][1], pts[i][2], 6, 255, 0, 0, 255) end
end
"##,
        )
        .unwrap();

        let mut h = HoverDot::new();
        h.enabled = true;
        h.sync_instances_at(&dir, &["pool".into()]);
        h.set_screen(1920.0, 1080.0);

        // ① 首帧：on_move 落定出生点，但时钟还没推进（age=0）→ 脚本按设计不画。
        h.pos = Point::new(10.0, 20.0);
        let mut l0 = DrawList::new();
        h.draw(&mut l0);
        assert!(l0.cmds().is_empty(), "age 还没推进时脚本按设计不画");

        // ② 指针动了一下 + 时钟推进 → 必须画出来（悬浮标与拖尾同序列）。
        //    池里此时 2 点：① 的出生点 + ② 的这一次移动。
        h.pos = Point::new(50.0, 60.0);
        h.set_time(1.0 / 60.0);
        let mut l1 = DrawList::new();
        assert!(h.draw(&mut l1), "on_move + step 都到位后必须画出来");
        assert_eq!(l1.cmds().len(), 2, "两个采样点 → 两个圆");

        // ③ 继续移动继续画（池累积），证明 step 真的在推进。
        h.pos = Point::new(90.0, 60.0);
        h.set_time(2.0 / 60.0);
        let mut l2 = DrawList::new();
        assert!(h.draw(&mut l2));
        assert_eq!(l2.cmds().len(), 3, "三个采样点 → 三个圆");

        // ④ 指针不动时**不补发** on_move（否则"累积池"会按帧率被灌满）。
        h.set_time(3.0 / 60.0);
        let mut l3 = DrawList::new();
        assert!(h.draw(&mut l3));
        assert_eq!(l3.cmds().len(), 3, "静止帧不补发 on_move，池不该增长");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// sync_instances 的对账语义：同 id 保实例（幂等）、增删按列表、顺序随列表。
    #[test]
    fn sync_instances_reconciles_by_id() {
        let mut h = HoverDot::new();
        h.sync_instances(&["HoverDot/png".into()]);
        h.sync_instances(&["HoverDot/png".into()]); // 幂等：不重建
        assert_eq!(h.instances.len(), 1);
        assert_eq!(h.instances[0].id, "HoverDot/png");
        // 增 + 顺序随列表。
        h.sync_instances(&["HoverDot/ring".into(), "HoverDot/png".into()]);
        assert_eq!(
            h.instances
                .iter()
                .map(|i| i.id.as_str())
                .collect::<Vec<_>>(),
            vec!["HoverDot/ring", "HoverDot/png"]
        );
        // 删：消失的实例被丢弃。
        h.sync_instances(&["HoverDot/png".into()]);
        assert_eq!(h.instances.len(), 1);
        assert_eq!(h.instances[0].id, "HoverDot/png");
    }

    #[test]
    fn gif_anim_advances_frames_by_elapsed() {
        let mut h = HoverDot::new();
        h.enabled = true;
        h.sync_instances(&["HoverDot/png".into()]);
        h.pos = Point::new(100.0, 100.0);
        h.instances[0].set_anim(Some(crate::core::gif::GifAnim {
            frames: vec![solid_bitmap([255, 0, 0]), solid_bitmap([0, 0, 255])],
            delays_ms: vec![100.0, 100.0],
            total_ms: 200.0,
        }));

        // t=0 → 第一帧（红）
        h.set_time(0.0);
        // ★ 单位统一为秒：150ms = 0.150s（此前宿主喂毫秒，与另外三类宿主差 1000 倍）。
        let mut l1 = DrawList::new();
        assert!(h.draw(&mut l1));
        let f1 = match l1.cmds().first().unwrap() {
            DrawCmd::Image { idx, .. } => l1.images()[*idx].clone(),
            other => panic!("应为图片指令，实际 {other:?}"),
        };
        assert_eq!(&f1.rgba[..4], &[255, 0, 0, 255], "t=0 应为第一帧（红）");

        // t=150 → 第二帧（蓝）
        h.set_time(0.150);
        let mut l2 = DrawList::new();
        assert!(h.draw(&mut l2));
        let f2 = match l2.cmds().first().unwrap() {
            DrawCmd::Image { idx, .. } => l2.images()[*idx].clone(),
            other => panic!("应为图片指令，实际 {other:?}"),
        };
        assert_eq!(&f2.rgba[..4], &[0, 0, 255, 255], "t=150 应为第二帧（蓝）");

        // GIF 动画需要持续重绘
        assert!(h.has_animation());
    }

    /// 「晃动定位」全链路：不藏指针 / 不画中心圆点 / 静止够久 + 左右晃才弹环 / 3 秒淡出。
    ///
    /// ★ 判据取**真实脚本**（`assets/lua/HoverDot/locate.lua`），宿主只喂墙钟与指针位置——
    ///   这样脚本的触发条件改坏了，这里是唯一会红的地方。
    #[test]
    fn locate_ring_needs_idle_then_swing_and_fades_out() {
        let dir = hoverdot_dir();
        let path = dir.join("locate.lua");
        assert!(path.exists(), "悬浮标脚本缺失：{}", path.display());

        let mut h = HoverDot::new();
        h.enabled = true;
        h.sync_instances(&["HoverDot/locate".into()]);
        // ★ 需求 a/b：系统指针必须原样保留（旧版顶层 `hide_cursor = true` 把它藏了）。
        assert!(
            !h.instances[0].wants_cursor_hidden(),
            "晃动定位不许隐藏系统指针"
        );
        h.set_screen(1920.0, 1080.0);
        // 参数不设 = 全走脚本默认值：静止 1000ms / 幅度 40 / 显示 3000ms。

        let mut wall = 10_000.0_f64;
        // 一帧：把指针摆到 (x,y)、墙钟拨到 t；返回 (环的不透明度, 脚本请求的续帧)。
        let step = |h: &mut HoverDot, x: f32, t: f64| -> (Option<u8>, Option<u32>) {
            h.pos = Point::new(x, 500.0);
            h.set_wall_ms(t);
            let mut list = DrawList::new();
            h.draw(&mut list);
            // ★ 需求 a：不许再画"替代指针的中心圆点"。
            assert!(!has_filled_circle(&list), "晃动定位不许画中心圆点");
            (ring_alpha(&list), h.next_frame_ms())
        };

        // ① 刚起步就左右晃（**没静止过**）→ 不弹环。旧版"累计位移"判据在这里就会误触发。
        let (a1, n1) = step(&mut h, 500.0, wall); // 首帧只做初始化
        wall += 16.0;
        let (a2, _) = step(&mut h, 560.0, wall); // 右 60
        wall += 16.0;
        let (a3, _) = step(&mut h, 620.0, wall); // 继续右
        wall += 16.0;
        let (a4, _) = step(&mut h, 560.0, wall); // 左（一次回摆）
        assert!(
            a1.is_none() && a2.is_none() && a3.is_none() && a4.is_none(),
            "没静止够久就晃动 → 不该弹环，实际 {a1:?} {a2:?} {a3:?} {a4:?}"
        );
        assert_eq!(n1, None, "待机不该请求续帧（静止必须保持零功耗）");

        // ② 静止 1.5 秒（这段没有帧，脚本看不见，靠下一帧的墙钟差补算）→ 再左右晃。
        wall += 1500.0;
        let (a5, _) = step(&mut h, 620.0, wall); // 静止后第一下移动：只武装
        assert!(a5.is_none(), "静止后的第一下移动只武装，不该立刻弹环");
        wall += 16.0;
        let (a6, _) = step(&mut h, 680.0, wall); // 继续右（累计 60 ≥ 幅度 40）
        assert!(a6.is_none(), "单方向移动不算晃动（要左右回摆才触发）");
        wall += 16.0;
        let (a7, n7) = step(&mut h, 620.0, wall); // 左 = 左右晃完成 → 弹环
        assert_eq!(
            a7,
            Some(255),
            "静止够久 + 左右晃动 → 必须弹出满不透明度的环"
        );
        assert_eq!(n7, Some(32), "弹环后必须请求续帧，否则淡出会定格在屏上");

        // ②′ 环必须**跟着指针走**（需求 2）：环亮着时把指针挪走，环心要落在**新**位置，
        //     而不是钉在触发那一刻的坐标上（旧画法会让环"掉在后面"）。
        h.pos = Point::new(900.0, 640.0);
        let mut lmove = DrawList::new();
        h.draw(&mut lmove);
        let c = ring_center(&lmove).expect("环亮着时应取得到环心");
        assert!(
            (c.0 - 900.0).abs() < 1e-3 && (c.1 - 640.0).abs() < 1e-3,
            "环心必须跟着当前指针 (900,640)，实际 {c:?}"
        );

        // ③ 3 秒淡出：中点 alpha 减半，到点消失且停止续帧。
        wall += 1500.0;
        let (a8, n8) = step(&mut h, 620.0, wall);
        let mid = a8.expect("1.5 秒时环还在（淡出中）");
        assert!(
            (mid as i32 - 127).abs() <= 2,
            "一半时长时 alpha 应≈127，实际 {mid}"
        );
        assert_eq!(n8, Some(32));
        wall += 1500.0;
        let (a9, n9) = step(&mut h, 620.0, wall);
        assert!(a9.is_none(), "满 3 秒 → 环必须消失，实际 {a9:?}");
        assert_eq!(n9, None, "淡出结束必须停止续帧（收敛回静止零功耗）");
    }

    /// 「屏幕标尺」刻度必须**等距**，且「细分刻度」开关必须真的改变刻度数量。
    ///
    /// ★ 旧版把刻度算成「网格间距的一半 / 五分之一」：网格 100 时小刻度落在 20px、
    ///   中刻度落在 50px，20 不整除 50 → 一格里出现 20/20/10/10/20/20 的忽宽忽窄，
    ///   用户看到的就是"刻度不均匀、细分刻度时灵时不灵"。这里把"均匀"钉成判据：
    ///   **所有刻度的并集必须等距**（开细分 = 每 10px 一根；关细分 = 每 50px 一根）。
    #[test]
    fn ruler_ticks_are_uniform_and_fine_switch_matters() {
        /// 取横向刻度（竖短线，长度 ≤30px 排除贯穿屏幕的主线）的 (x, 长度)。
        fn h_ticks(list: &DrawList) -> Vec<(f32, f32)> {
            let mut out: Vec<(f32, f32)> = list
                .cmds()
                .iter()
                .filter_map(|c| match c {
                    DrawCmd::Stroke {
                        shape: Shape::Line { a, b },
                        ..
                    } if (a.x - b.x).abs() < 0.5 => {
                        let len = (a.y - b.y).abs();
                        if len <= 30.0 {
                            Some((a.x, len))
                        } else {
                            None
                        }
                    }
                    _ => None,
                })
                .collect();
            out.sort_by(|p, q| p.0.partial_cmp(&q.0).unwrap());
            out
        }

        let dir = hoverdot_dir();
        assert!(dir.join("ruler.lua").exists(), "标尺脚本缺失");
        let mut h = HoverDot::new();
        h.enabled = true;
        h.set_screen(1920.0, 1080.0);
        h.pos = Point::new(500.0, 400.0);
        h.sync_instances(&["HoverDot/ruler".into()]);
        let mut draw_with = |fine: bool| -> Vec<(f32, f32)> {
            h.instances[0].set_params(HashMap::from([("show_fine".to_string(), Json::Bool(fine))]));
            let mut list = DrawList::new();
            assert!(h.draw(&mut list), "标尺应产出绘制指令");
            h_ticks(&list)
        };

        // ① 细分刻度**开**：并集每 10px 一根 → 相邻间距恒为 10。
        let on = draw_with(true);
        assert!(!on.is_empty(), "开着细分刻度必须有横向刻度");
        let xs: Vec<f32> = on.iter().map(|t| t.0).collect();
        for w in xs.windows(2) {
            assert!(
                (w[1] - w[0] - 10.0).abs() < 1e-3,
                "相邻刻度间距必须恒为 10px，实际 {} → {}",
                w[0],
                w[1]
            );
        }
        assert!(
            on.iter().any(|t| (t.1 - 8.0).abs() < 1e-3),
            "开着细分刻度必须有 10px 级的小刻度（±4px）"
        );

        // ② 细分刻度**关**：小刻度整级消失，剩下 50/100 两级 → 相邻间距恒为 50。
        let off = draw_with(false);
        assert!(
            !off.iter().any(|t| (t.1 - 8.0).abs() < 1e-3),
            "关掉细分刻度后不该再有 10px 级小刻度"
        );
        let xs_off: Vec<f32> = off.iter().map(|t| t.0).collect();
        for w in xs_off.windows(2) {
            assert!(
                (w[1] - w[0] - 50.0).abs() < 1e-3,
                "关掉细分刻度后相邻间距必须恒为 50px，实际 {} → {}",
                w[0],
                w[1]
            );
        }
        assert!(
            off.len() * 4 < on.len(),
            "开细分后刻度数应远多于关掉时（{} vs {}）",
            on.len(),
            off.len()
        );
    }

    /// 脚本 `draw` 的返回值 = 续帧请求：正数才认，其余一律不请求。
    #[test]
    fn frame_request_ms_only_accepts_positive_numbers() {
        assert_eq!(frame_request_ms(mlua::Value::Integer(32)), Some(32));
        assert_eq!(frame_request_ms(mlua::Value::Number(16.0)), Some(16));
        // 老脚本不返回值 / 返回 nil / 0 / 负数 / 字符串 → 一律"不续帧"。
        assert_eq!(frame_request_ms(mlua::Value::Nil), None);
        assert_eq!(frame_request_ms(mlua::Value::Integer(0)), None);
        assert_eq!(frame_request_ms(mlua::Value::Integer(-5)), None);
        // 非数字类型（老脚本返回 true / 字符串之类）→ 同样"不续帧"。
        assert_eq!(frame_request_ms(mlua::Value::Boolean(true)), None);
    }
}
