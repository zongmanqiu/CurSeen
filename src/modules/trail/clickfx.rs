//! 点击效果自定义脚本（ClickFx）。每次鼠标按下触发一次脚本实例。
//!
//! # 定位（2026-09-27 四宿主统一到 `FxCore`）
//!
//! 与拖尾 / 悬浮标 / 独立卡片**共用同一套脚本契约**，且**驱动序列只有一份实现**
//! （[`FxCore`](super::fx_core)）：事件名、`dt` 刻度（`FIXED_DT` 固定步长秒）、环境变量
//! （`cursor_*` / `origin_*` / `host` / `wall_ms`）四类宿主完全一致——**同一个脚本放哪个
//! 目录效果都一样**。
//!
//! 本宿主只剩两点差异：
//!
//! 1. **生命周期（2026-09-28 起跟随脚本能力）**：只写 `on_press` 的脚本维持"每次按下
//!    生成一个实例，画完即死（`MAX_AGE_S` 兜底）"；**实现了 `on_move` 的脚本（轨迹
//!    语义，如把辉光复制进来）走常驻通道**——控制器按真实采样流喂 `on_move`、按下
//!    转发 `on_press`，实例与拖尾同待遇，不随按下生灭；
//! 2. **出生序列**：点击语义脚本出生点 = 按下点，出生时补发一次 `on_move`（拖尾
//!    「累积池」型脚本的唯一内容来源）。
//!
//! ★ 于是**同一个 `.lua` 文件丢进任何目录都该能跑**：底层行为由脚本实现的回调决定，
//!   文件夹只决定它出现在哪张卡片（用户口径：「不同的文件夹仅仅只是读取和生成 ui
//!   可能有区别，不影响底层功能」）。
//! - 拖尾脚本（只认 `on_move` 的「累积池」模型）挂到这里，出生时补发一次 `on_move`，
//!   池里有 1 个点、能画出来。此前不补这一下，`draw` 首行 `#points == 0` 直接 return
//!   ⇒ 零指令 ⇒ 首帧就被判死 ⇒ 用户看到"点下去什么都没发生"。
//! - 点击脚本挂到拖尾宿主，靠 `on_press` 在每次按下时触发。
//!
//! # 光标
//!
//! 出生后**每帧静默刷新 `cursor_*`**（[`FxCore::set_cursor`]，不发 `on_move`）。此前这里
//! 把光标钉死在按下点，跟随类脚本换个目录就不跟手了。
//!
//! # 生命周期
//!
//! `is_active` 由"本帧是否产出绘制指令"判定（与拖尾同规则），空闲实例被管理器剔除；
//! 另加 `MAX_AGE_S` 硬上限，防脚本"永远在画"造成泄漏。

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value as Json;

use crate::core::draw::DrawList;

use super::fx_core::FxCore;

/// 本宿主在 Lua 全局 `host` 里的标识。
const HOST: &str = "clickfx";

/// 单个脚本实例最多存活这么久（**秒**）。远超任何正常点击特效的寿命，仅作防泄漏兜底。
const MAX_AGE_S: f32 = 8.0;

/// 一个点击效果脚本实例（每次按下生成一个，独立持有自己的 Lua VM 与状态）。
pub struct ScriptedClick {
    /// 统一实例核心（引擎 / 参数 / 环境 / 事件 / 步进 / 判活都在里面）。
    core: FxCore,
}

impl ScriptedClick {
    /// 从脚本目录加载 `<id>.lua` 并生成一个实例，出生在 (x, y)。
    pub fn new(
        scripts_dir: &Path,
        id: &str,
        x: f32,
        y: f32,
        params: HashMap<String, Json>,
        screen: (f32, f32),
    ) -> mlua::Result<Self> {
        let mut core = FxCore::load(scripts_dir, id, HOST)?;
        core.set_params(params);
        core.set_screen(screen.0, screen.1);
        // ★ 桥接（互换性的关键一步）："点击" = 指针跳到按下点的**一次移动**。
        //   拖尾脚本只认 on_move，不补这一下它就永远空池、零指令当场判死。
        //   ★ 必须在 press 之前：出生点由先到者落定，两者同点，顺序只影响事件次序
        //     （拖尾宿主的真实序列也是"先移动后按下"）。
        core.move_to(x, y, None);
        // 按下事件（新口径 on_press，自动回退老口径 on_click）。两个都没有 → 静默跳过。
        core.press(x, y);
        // ★ 出生即存活：判活只看"上一帧 draw 有没有产出"，而新实例还没画过第一帧。
        //   不置真的话它会在**第一次 draw 之前**的那次 tick 里被当空闲实例剔除——
        //   用户看到的就是"点下去什么都没发生"。
        core.active = true;
        Ok(Self { core })
    }

    /// 从脚本目录打开一个**常驻**实例（2026-09-28「生命周期跟随脚本能力」）。
    ///
    /// 与 [`Self::new`] 的区别：**不发出生 `on_move` / `on_press`、不参与按样品生灭**。
    /// 给实现了 `on_move` 的脚本（轨迹语义，如把辉光复制进 `ClickFx/`）用——控制器
    /// 会按真实采样流喂 [`Self::on_sample`]、按下时喂 [`Self::on_press`]，实例与
    /// 拖尾特效同待遇常驻，不受 [`MAX_AGE_S`] 约束。
    pub fn open_persistent(
        scripts_dir: &Path,
        id: &str,
        params: HashMap<String, Json>,
        screen: (f32, f32),
    ) -> mlua::Result<Self> {
        let mut core = FxCore::load(scripts_dir, id, HOST)?;
        core.set_params(params);
        core.set_screen(screen.0, screen.1);
        core.active = true;
        Ok(Self { core })
    }

    /// 脚本是否实现了 `on_move`（轨迹语义判据：写它就享受常驻待遇）。
    pub fn wants_trail(&self) -> bool {
        self.core.has_on_move()
    }

    /// 指针真实移动（**发 `on_move`**）：常驻通道由控制器按采样流喂，
    /// 「累积池」型脚本据此持续堆点——轨迹才会成形。
    pub fn on_sample(&mut self, x: f32, y: f32) {
        self.core.move_to(x, y, None);
    }

    /// 按下事件转发（常驻通道；点击语义由脚本自己的 `on_press` 回调实现）。
    pub fn on_press(&mut self, x: f32, y: f32) {
        self.core.press(x, y);
    }

    /// 注入参数表（常驻通道参数重放）。
    pub fn set_params(&mut self, params: HashMap<String, Json>) {
        self.core.set_params(params);
    }

    /// 注入屏幕逻辑尺寸（常驻通道跨屏/分辨率变化时对齐）。
    pub fn set_screen(&mut self, w: f32, h: f32) {
        self.core.set_screen(w, h);
    }

    pub fn id(&self) -> &str {
        &self.core.id
    }

    /// 静默刷新指针位置（**不发 `on_move`**）：`cursor_*` 跟着光标走，但"累积池"型
    /// 脚本不会按帧率堆点。见 [`FxCore::set_cursor`]。
    pub fn set_cursor(&mut self, x: f32, y: f32) {
        self.core.set_cursor(x, y);
    }

    /// 注入真实墙钟毫秒（脚本 `wall_ms`）——四类宿主一致，静止检测口径才一样。
    pub fn set_wall_ms(&mut self, ms: f64) {
        self.core.set_wall_ms(ms);
    }

    /// 每帧推进（老化）。`dt_s` 单位**秒**；内部按 `FIXED_DT` 子步进，与拖尾同节拍。
    pub fn tick(&mut self, dt_s: f32) {
        self.core.advance(dt_s);
    }

    pub fn draw(&mut self, list: &mut DrawList) {
        self.core.render(list);
    }

    /// 是否还活着：**本帧画了东西**即视为活着。
    ///
    /// 注意：带"间隙"的脚本（某帧画到零尺寸）可能在间隙帧被误判死亡——这是已知取舍，
    /// 正常淡出类特效（每帧都在画、直到最后一帧趋于零）不受影响。防泄漏的硬上限由
    /// 管理器按 [`Self::elapsed`] 兜底（见 [`super::controller`]）。
    pub fn is_active(&self) -> bool {
        self.core.active
    }

    /// 存活时长（秒），供管理器做硬上限剔除。
    pub fn elapsed(&self) -> f32 {
        self.core.elapsed()
    }

    /// 防泄漏硬上限（秒）。
    pub const MAX_AGE_S: f32 = MAX_AGE_S;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_script(body: &str) -> (std::path::PathBuf, String) {
        let dir = std::env::temp_dir().join(format!(
            "curseen_clickfx_{}_{}",
            std::process::id(),
            // 用内容长度凑唯一，避免并行测试目录撞名互删。
            body.len()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let id = "demo".to_string();
        std::fs::write(dir.join(format!("{id}.lua")), body).unwrap();
        (dir, id)
    }

    #[test]
    fn scripted_click_spawns_and_draws() {
        // 一个"出生画一个实心圆、之后每帧都画"的最简脚本：应被视为存活。
        let body = r##"
effect_name = "测试圆"
function schema() return {} end
function on_click(x, y, t, p) cx, cy = x, y; age = 0 end
function step(dt, t, p) age = (age or 0) + dt end
function draw(ctx, p)
  ctx:circle(cx, cy, 20, 255, 255, 255, 255)
end
"##;
        let (dir, id) = temp_script(body);
        let mut fx =
            ScriptedClick::new(&dir, &id, 100.0, 100.0, HashMap::new(), (1920.0, 1080.0)).unwrap();
        let mut list = DrawList::new();
        fx.draw(&mut list);
        assert!(fx.is_active(), "脚本绘制了圆 → 应存活");
        assert!(!list.cmds().is_empty());
    }

    #[test]
    fn scripted_click_dies_after_it_stops_drawing() {
        // 前 2 帧画、其后不画：第 3 帧 is_active 应为 false（管理器据此剔除）。
        let body = r##"
effect_name = "闪一下"
function schema() return {} end
function on_click(x, y, t, p) frames = 0 end
function step(dt, t, p) frames = (frames or 0) + 1 end
function draw(ctx, p)
  if (frames or 0) < 2 then ctx:circle(0, 0, 5, 255, 255, 255, 255) end
end
"##;
        let (dir, id) = temp_script(body);
        let mut fx =
            ScriptedClick::new(&dir, &id, 0.0, 0.0, HashMap::new(), (1920.0, 1080.0)).unwrap();
        let mut l1 = DrawList::new();
        fx.draw(&mut l1);
        assert!(fx.is_active(), "前 2 帧应绘制");
        fx.tick(0.016);
        let mut l2 = DrawList::new();
        fx.draw(&mut l2);
        assert!(fx.is_active(), "第 2 帧仍应绘制");
        // 推进一帧后 frames>=2，draw 不再产出指令 → is_active 应为 false。
        fx.tick(0.016);
        let mut l3 = DrawList::new();
        fx.draw(&mut l3);
        assert!(!fx.is_active(), "脚本不再绘制 → 应被判死亡");
    }

    #[test]
    fn trail_style_pool_script_draws_under_click_host() {
        // ★ 互换性回归：只认 on_move 的「累积池」脚本（拖尾模型）挂到点击宿主上，
        //   必须能画出来——出生时补发的那次 on_move 是它唯一的内容来源。
        //   把 `ScriptedClick::new` 里的 `call_on_move` 去掉，这里立刻变 0 条指令。
        let body = r##"
effect_name = "池"
function schema() return {} end
function on_move(x, y, t, p) px, py = x, y end
function step(dt, t, p) end
function draw(ctx, p)
  if px == nil then return end
  ctx:circle(px, py, 10, 255, 255, 255, 255)
end
"##;
        let (dir, id) = temp_script(body);
        let mut fx =
            ScriptedClick::new(&dir, &id, 321.0, 456.0, HashMap::new(), (1920.0, 1080.0)).unwrap();
        let mut list = DrawList::new();
        fx.draw(&mut list);
        assert!(
            fx.is_active(),
            "拖尾模型脚本挂点击宿主应能画（出生补发 on_move）"
        );
        assert_eq!(list.cmds().len(), 1, "应恰好画出一个圆");
    }

    #[test]
    fn origin_and_host_globals_are_injected() {
        // ★ 出生点注入：脚本读 origin_x/origin_y 必须拿到按下点（不是 0、不是 nil）；
        //   host 必须是 "clickfx"（脚本据此分支时不能拿到空串）。
        let body = r##"
effect_name = "环境"
function schema() return {} end
function on_press(x, y, t, p) end
function step(dt, t, p) end
function draw(ctx, p)
  if origin_x == nil or host ~= "clickfx" then return end
  ctx:circle(origin_x, origin_y, 5, 255, 255, 255, 255)
end
"##;
        let (dir, id) = temp_script(body);
        let mut fx =
            ScriptedClick::new(&dir, &id, 777.0, 333.0, HashMap::new(), (1920.0, 1080.0)).unwrap();
        let mut list = DrawList::new();
        fx.draw(&mut list);
        assert!(fx.is_active(), "origin_x/host 未注入 → 脚本一行都画不出");
    }

    #[test]
    fn missing_script_is_an_error() {
        let dir =
            std::env::temp_dir().join(format!("curseen_clickfx_missing_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let r = ScriptedClick::new(&dir, "nope", 0.0, 0.0, HashMap::new(), (1920.0, 1080.0));
        assert!(
            r.is_err(),
            "不存在的脚本应返回错误（控制器据此回退内置样式）"
        );
    }
}
