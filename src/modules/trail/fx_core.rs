//! ★ 四类鼠标特效宿主共用的**唯一脚本实例核心**（2026-09-27 用户口径：
//! 「鼠标拖尾、悬浮标、点击效果，本质上都是鼠标效果，本来就是同一套系统」）。
//!
//! 此前拖尾（`LuaEffect`）、悬浮标（`HoverDotInstance`）、点击效果（`ScriptedClick`）
//! 各自手写一份"注环境 → 发事件 → 步进 → 画"的驱动序列，四份实现各缺一块：
//! 悬浮标**从不发 `on_move`、从不调 `step`**（事件驱动的脚本在它下面什么也画不出）；
//! 点击宿主把光标**钉死在按下点**（跟随类脚本挂过去视觉就变样）；`wall_ms` 只有
//! 悬浮标注入；`elapsed` 悬浮标是毫秒、其余是秒。同一个 `.lua` 换个文件夹就是三种待遇。
//!
//! 现在**唯一序列**在这里，宿主只回答两个问题：
//!
//! 1. **实例什么时候生 / 什么时候死**（生命周期）；
//! 2. **这一帧的输入是什么**（光标在哪、按没按下、过了多久）。
//!
//! | 宿主 | 生命周期 | 光标 | 按下 |
//! |---|---|---|---|
//! | 拖尾 | 1 个常驻实例 | 实时指针 | 转发 `on_press` |
//! | 悬浮标 | N 个常驻实例 | 实时指针 | 转发 `on_press` |
//! | 点击效果 | 脚本写了 `on_move` → **常驻**（采样流喂 `on_move`）；只写 `on_press` → 每次按下生成一个实例（`MAX_AGE_S` 兜底） | 实时指针 | 转发 / 出生 = 按下 |
//! | 独立卡片 | 常驻，另收 `on_key`/`on_mouse_down` | 实时指针 | `on_mouse_down` |
//!
//! ★ 生命周期跟随脚本能力（2026-09-28 用户口径：「脚本声明自己要调用什么能力，
//!   生命周期就该由它决定」）——文件夹只决定 UI 归属，不影响底层行为。
//!
//! 于是**同一个脚本放哪个文件夹效果都一样**：事件流、`dt` 刻度（固定步长 `FIXED_DT`
//! 秒）、环境变量（`cursor_*`/`origin_*`/`host`/`wall_ms`）完全一致；脚本用 `origin_*`
//! 钉出生点、用 `cursor_*` 跟光标，两边宿主给的都是同一个值。

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value as Json;

use crate::core::draw::DrawList;
use crate::scripting::api_draw;
use crate::scripting::engine::ScriptingEngine;

/// 固定步长（秒）。与拖尾控制器同一常量——四个宿主必须踩同一个节拍。
pub use super::controller::FIXED_DT;

/// 单帧最多补的子步数（与控制器 `MAX_STEPS` 同量级）：卡顿后不把丢的时间一次补完，
/// 动画慢进而不是瞬移。
pub const MAX_SUBSTEPS: u32 = 16;

/// 单帧 dt 上限（秒）：第一帧 / 长时间无帧后恢复时，`elapsed` 差可能很大，直接钳掉。
pub const MAX_FRAME_DT: f32 = 0.25;

/// [`FxCore::render`] 的产出：`produced` = 本帧是否画了东西（判活）；`frame_request`
/// = 脚本 `draw` 的原始返回值（悬浮标把它解释成"多少毫秒后再来一帧"）。
pub struct FrameOut {
    pub produced: bool,
    pub frame_request: mlua::Value,
}

/// 一个脚本实例的核心：引擎 + 参数 + 环境状态 + 统一驱动。
pub struct FxCore {
    /// Lua 引擎（脚本全局状态都在里面）。
    pub engine: ScriptingEngine,
    /// 脚本 id（相对 lua 根，含目录前缀）。
    pub id: String,
    /// 本宿主在 Lua 全局 `host` 里的标识（`trail`/`hover`/`clickfx`/`panel`）。
    host: &'static str,
    params: HashMap<String, Json>,
    /// 实时指针位置（每帧由宿主喂 [`Self::move_to`] / [`Self::press`]）。
    cursor: (f32, f32),
    /// 出生点。`None` = 尚未定，注入时退回 `cursor`；首次 `move_to`/`press` 落定。
    origin: Option<(f32, f32)>,
    screen: (f32, f32),
    /// 真实墙钟毫秒（脚本 `wall_ms`；静止检测 / 超时淡出用，`elapsed` 测不出无帧期）。
    wall_ms: f64,
    /// 实例时间（秒）＝已交付子步的总时长。宿主也可用 [`Self::set_elapsed`] 对齐
    /// 控制器的共享时钟（拖尾就是这么做——脚本拿到的 t 与旧版逐位一致）。
    elapsed: f32,
    /// 固定步长累加器（不足一个 `FIXED_DT` 的零头留到下一帧）。
    acc: f32,
    /// 上一帧 `draw` 是否真的产出了指令（判活；点击宿主据此剔除实例）。
    pub active: bool,
    /// 脚本是否声明 `hide_cursor = true`。
    pub hide_cursor: bool,
    /// 上一次移动的 (x,y,elapsed)：算 `cursor_vx/vy/speed/heading` 用。
    last_move: Option<(f32, f32, f32)>,
    vx: f32,
    vy: f32,
    speed: f32,
    heading: f32,
    mouse_down: bool,
    mouse_button: &'static str,
}

impl FxCore {
    /// 从 `root` 目录加载 `<相对 id>.lua` 构造实例。失败 = Err（宿主各自兜底）。
    pub fn load(root: &Path, id: &str, host: &'static str) -> mlua::Result<Self> {
        let engine = ScriptingEngine::new(root)?;
        engine.load_script(id)?;
        let hide_cursor = engine.get_hide_cursor();
        Ok(Self {
            engine,
            id: id.to_string(),
            host,
            params: HashMap::new(),
            cursor: (0.0, 0.0),
            origin: None,
            screen: (1920.0, 1080.0),
            wall_ms: 0.0,
            elapsed: 0.0,
            acc: 0.0,
            active: false,
            hide_cursor,
            last_move: None,
            vx: 0.0,
            vy: 0.0,
            speed: 0.0,
            heading: 0.0,
            mouse_down: false,
            mouse_button: "left",
        })
    }

    // ---- 宿主每帧喂的输入 -------------------------------------------------

    /// 注入参数表（缺键由脚本内 `params.x or 默认` 兜底）。
    pub fn set_params(&mut self, params: HashMap<String, Json>) {
        self.params = params;
    }

    /// 注入屏幕逻辑尺寸。
    pub fn set_screen(&mut self, w: f32, h: f32) {
        self.screen = (w, h);
    }

    /// 注入真实墙钟毫秒。
    pub fn set_wall_ms(&mut self, ms: f64) {
        self.wall_ms = ms;
    }

    /// 对齐宿主维护的共享时钟（拖尾用它保证脚本拿到的 t 与旧版逐位一致；
    /// 点击/悬浮标不调它，`elapsed` 就自积累）。
    pub fn set_elapsed(&mut self, t: f32) {
        self.elapsed = t;
    }

    /// 指针移动到 (x, y)：更新光标、落定出生点、注环境、发 `on_move`。
    ///
    /// `spawn_batch`：拖尾宿主的"本批生成几个粒子"（间隔类特效用），其余宿主传 `None`。
    pub fn move_to(&mut self, x: f32, y: f32, spawn_batch: Option<u32>) {
        // 单位口径：按 60fps 的每帧像素算，与 CursorFX 的 state.vx/vy 同量纲。
        if let Some((lx, ly, lt)) = self.last_move {
            let dt = self.elapsed - lt;
            if dt > 1e-4 {
                self.vx = (x - lx) / dt / 60.0;
                self.vy = (y - ly) / dt / 60.0;
                self.speed = (self.vx * self.vx + self.vy * self.vy).sqrt();
                self.heading = self.vy.atan2(self.vx);
            }
        }
        self.last_move = Some((x, y, self.elapsed));
        self.cursor = (x, y);
        if self.origin.is_none() {
            self.origin = Some((x, y));
        }
        let mut params = self.params.clone();
        if let Some(n) = spawn_batch {
            params.insert("spawn_batch".into(), Json::from(n));
        }
        let _ = self.push_env();
        let _ = self.engine.call_on_move(x, y, self.elapsed, &params);
    }

    /// 静默更新指针位置（**不发 `on_move`**）。
    ///
    /// 给"宿主知道指针在哪、但本帧不该再喂一次移动事件"的场景用：点击效果实例每帧都该
    /// 跟着光标（`cursor_*` 实时），但**不能**每帧补发 `on_move`——那会让"累积池"型
    /// 脚本按帧率而不是按移动距离堆点，池子被同一坐标灌满。
    /// 拖尾 / 悬浮标走 [`Self::move_to`]（真有移动才发），点击走这里（只刷新坐标）。
    pub fn set_cursor(&mut self, x: f32, y: f32) {
        self.cursor = (x, y);
    }

    /// 指针在 (x, y) 按下：更新光标、落定出生点、注环境、发 `on_press`
    /// （脚本只写了老名 `on_click` 时引擎自动回退，老脚本零改动）。
    pub fn press(&mut self, x: f32, y: f32) {
        self.press_button("left", x, y);
    }

    /// 带按键名的按下（复合/包卡脚本可据此区分左右中键）。
    pub fn press_button(&mut self, button: &'static str, x: f32, y: f32) {
        self.cursor = (x, y);
        self.mouse_down = true;
        self.mouse_button = button;
        if self.origin.is_none() {
            self.origin = Some((x, y));
        }
        let _ = self.push_env();
        let _ = self
            .engine
            .call_on_button(button, x, y, self.elapsed, &self.params);
    }

    /// 抬起：清 `mouse_down` 并转发 `on_mouse_up`（缺回调静默跳过）。
    pub fn release(&mut self, button: &'static str, x: f32, y: f32) {
        self.cursor = (x, y);
        self.mouse_down = false;
        self.mouse_button = button;
        let _ = self.push_env();
        let _ = self.engine.call_on_mouse_up(button, x, y, &self.params);
    }

    /// 滚轮：刷新光标 + 环境，转发 `on_wheel`（缺回调静默跳过）。
    pub fn wheel(&mut self, delta: i32, horizontal: bool, x: f32, y: f32) {
        self.cursor = (x, y);
        let _ = self.push_env();
        let _ = self
            .engine
            .call_on_wheel(delta, horizontal, x, y, &self.params);
    }

    /// 脚本是否声明了 `on_wheel`。
    pub fn wants_wheel(&self) -> bool {
        matches!(
            self.engine.lua().globals().get::<mlua::Value>("on_wheel"),
            Ok(mlua::Value::Function(_))
        )
    }

    /// 推进 `dt_s` 秒：按 `FIXED_DT` 子步进（不足一步的零头留在累加器里），
    /// 每个子步注环境 + 发 `step(FIXED_DT, t)`。**与拖尾控制器踩同一节拍**——
    /// 同一帧里三个宿主给脚本的 `dt` 序列完全一致，这是"效果一样"的时间维保证。
    pub fn advance(&mut self, dt_s: f32) {
        self.acc += dt_s.min(MAX_FRAME_DT);
        let mut steps = 0u32;
        while self.acc >= FIXED_DT && steps < MAX_SUBSTEPS {
            let _ = self.push_env();
            let _ = self.engine.call_step(FIXED_DT, self.elapsed, &self.params);
            self.elapsed += FIXED_DT;
            self.acc -= FIXED_DT;
            steps += 1;
        }
        if steps == MAX_SUBSTEPS {
            self.acc = 0.0; // 卡顿兜底：不把丢掉的时间滚雪球
        }
    }

    /// 绘制：注环境 → `draw(ctx, params)` → 收指令。返回本帧产出与脚本返回值。
    pub fn render(&mut self, list: &mut DrawList) -> FrameOut {
        let before = list.len_all();
        api_draw::set_draw_target(list);
        let _ = self.push_env();
        let frame_request = self
            .engine
            .call_draw(&self.params)
            .unwrap_or(mlua::Value::Nil);
        api_draw::clear_draw_target();
        // ★ len_all() 跨层计数：脚本声明 base/top 层时指令落在别的桶，只查 cmds()
        // 会把垫底/顶层的特效误判成"没画"。
        self.active = list.len_all() > before;
        FrameOut {
            produced: self.active,
            frame_request,
        }
    }

    /// 重置实例状态（换特效 / 卡片关开）：重放 `init()`、出生点作废、判活清零。
    pub fn reset(&mut self) {
        let _ = self.engine.call_init();
        self.origin = None;
        self.active = false;
    }

    /// 实例时间（秒）——点击宿主拿它做 `MAX_AGE_S` 硬上限剔除。
    pub fn elapsed(&self) -> f32 {
        self.elapsed
    }

    /// 脚本是否实现了 `on_move`（**轨迹语义判据**，2026-09-28 用户口径：
    /// 「生命周期应该由脚本声明自己要调用什么能力来决定」）。
    ///
    /// 实现了 `on_move` = 脚本靠采样流累积内容（累积池），宿主必须给**常驻待遇**：
    /// 每个真实采样转发 `on_move`，实例不随按下生灭。只写 `on_press` 的脚本
    /// 则维持"按下出生、画完即死"的点击语义。文件夹只决定 UI 归属，不影响这条。
    pub fn has_on_move(&self) -> bool {
        self.engine
            .lua()
            .globals()
            .get::<Option<mlua::Function>>("on_move")
            .ok()
            .flatten()
            .is_some()
    }

    /// 注入本轮环境（光标 / 出生点 / 时钟 / 屏幕 / 宿主 / 墙钟）——**唯一注入点**。
    fn push_env(&self) -> mlua::Result<()> {
        let origin = self.origin.unwrap_or(self.cursor);
        self.engine
            .update_env(self.cursor, origin, self.elapsed, self.screen, self.host)?;
        self.engine.set_wall_ms(self.wall_ms)?;
        self.engine.set_pointer_state(
            self.vx,
            self.vy,
            self.speed,
            self.heading,
            self.mouse_down,
            self.mouse_button,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // 拖尾宿主以 trait 方法驱动（`mouse_script_works_wherever_the_file_is_dropped` 用）。
    use super::super::effects::TrailEffect;

    /// 临时目录写一个脚本，返回 (目录, 脚本 id)。
    fn script_dir(body: &str) -> (std::path::PathBuf, String) {
        let dir = std::env::temp_dir().join(format!(
            "curseen_fxcore_{}_{:x}",
            std::process::id(),
            body.len().wrapping_mul(1_000_003)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let id = "demo".to_string();
        std::fs::write(dir.join(format!("{id}.lua")), body).unwrap();
        (dir, id)
    }

    /// 临时目录写一个最小脚本（记下收到的环境与事件序列）。
    fn core_with(body: &str) -> (std::path::PathBuf, FxCore) {
        let (dir, id) = script_dir(body);
        let core = FxCore::load(&dir, &id, "trail").expect("加载成功");
        (dir, core)
    }

    const RECORDER: &str = r#"
effect_name = "记录仪"
log = {}
function on_move(x, y, t, p) log[#log+1] = "move:" .. x .. ":" .. mouse_x .. ":" .. origin_x .. ":" .. tostring(wall_ms) end
function on_press(x, y, t, p) log[#log+1] = "press:" .. x .. ":" .. y .. ":" .. host end
function step(dt, t, p) log[#log+1] = "step:" .. dt .. ":" .. t .. ":" .. elapsed end
function draw(ctx, p)
  log[#log+1] = "draw:" .. cursor_x .. ":" .. origin_y
  ctx:circle(cursor_x, origin_y, 4, 255, 0, 0, 255)
end
"#;

    #[test]
    fn unified_frame_sequence_moves_steps_and_draws() {
        let (_d, mut c) = core_with(RECORDER);
        c.set_screen(1920.0, 1080.0);
        c.set_wall_ms(55.0);
        // 一帧完整序列：移动 → 推进 → 绘制。
        c.move_to(100.0, 200.0, Some(3));
        c.advance(1.0 / 60.0); // 60fps 一帧 = 2 个 1/120 子步
        let mut list = DrawList::new();
        let out = c.render(&mut list);

        assert!(out.produced, "画了圆 = 产出");
        assert!(c.active);
        let log: Vec<String> = c.engine.lua().globals().get("log").unwrap();
        // on_move：x=100、mouse_x 别名同值、origin 首次落定=100、wall_ms 注入。
        // （Lua 拼数字一律带小数位，故期望串是 `100.0` 这种形式。）
        assert_eq!(log[0], "move:100.0:100.0:100.0:55.0");
        // step：dt 恒 FIXED_DT，2 个子步；t 与 elapsed 同步增长。
        assert!(log[1].starts_with("step:0.00833333"), "实际 {}", log[1]);
        assert!(log[2].starts_with("step:0.00833333"), "实际 {}", log[2]);
        // draw：cursor 与 origin 都注入（这里 origin_y=200）。
        assert_eq!(log[3], "draw:100.0:200.0");
    }

    #[test]
    fn press_sets_origin_and_forwards_on_press() {
        let (_d, mut c) = core_with(RECORDER);
        c.press(300.0, 400.0);
        let log: Vec<String> = c.engine.lua().globals().get("log").unwrap();
        assert_eq!(
            log[0], "press:300.0:400.0:trail",
            "on_press 收到坐标与 host"
        );
        // 出生点落定为按下点（后续 move 不再改写 origin）。
        c.move_to(500.0, 500.0, None);
        let mut list = DrawList::new();
        c.render(&mut list);
        let log: Vec<String> = c.engine.lua().globals().get("log").unwrap();
        // draw 里 origin_y 仍是按下点的 400（不是新光标的 500）。
        assert_eq!(log.last().unwrap(), "draw:500.0:400.0");
    }

    #[test]
    fn advance_keeps_fixed_cadence_and_caps_catch_up() {
        let body = r#"
n = 0
function step(dt, t, p) n = n + 1 end
function draw(ctx, p) end
"#;
        let (_d, mut c) = core_with(body);
        c.advance(1.0 / 60.0); // 60fps → 恰 2 子步
        let n: i64 = c.engine.lua().globals().get("n").unwrap();
        assert_eq!(n, 2, "60fps 一帧 = 2 个 1/120 子步");
        assert!((c.elapsed() - 2.0 * FIXED_DT).abs() < 1e-6);
        // 卡顿 1 秒 → 只补 MAX_SUBSTEPS 步，累加器清零（不滚雪球）。
        c.advance(1.0);
        let n: i64 = c.engine.lua().globals().get("n").unwrap();
        assert_eq!(n, 2 + MAX_SUBSTEPS as i64, "卡顿帧最多补 {MAX_SUBSTEPS} 步");
    }

    #[test]
    fn reset_replays_init_and_clears_origin() {
        let body = r#"
inits = 0
function init() inits = inits + 1 end
"#;
        let (_d, mut c) = core_with(body);
        c.move_to(10.0, 20.0, None);
        c.reset();
        let inits: i64 = c.engine.lua().globals().get("inits").unwrap();
        assert_eq!(inits, 1, "reset 重放 init");
        assert!(!c.active);
        // 出生点已作废：下一次 move 重新落定为新位置。
        c.move_to(70.0, 80.0, None);
        let mut list = DrawList::new();
        c.render(&mut list);
        // 用 draw 里读 origin 验证（RECORDER 未挂——这里借环境注入间接验证：无 panic 即可）。
    }

    /// ★★ 用户口径的判据：「同一个 Lua 脚本，不管放在哪个文件夹，效果都一样」。
    ///
    /// 四个宿主名（拖尾 / 悬浮标 / 点击效果 / 独立卡片）跑**同一份脚本、同一串输入**，
    /// 产出的绘制指令必须**逐条相同**——`host` 全局是唯一允许不同的量（脚本据此分支是
    /// 设计内的）。谁改坏了环境注入 / 事件序列 / dt 刻度，这里立刻红。
    #[test]
    fn same_script_draws_identically_under_all_hosts() {
        // 脚本把收到的坐标、dt、墙钟全部折进绘制参数：任何一处注入不一致都会改产物。
        let body = r#"
pts = {}
age = 0
function on_move(x, y, t, p) pts[#pts+1] = {x, y, t} end
function on_press(x, y, t, p) press_at = {x, y} end
function step(dt, t, p) age = age + dt end
function draw(ctx, p)
  if #pts == 0 then return end
  local a = pts[#pts]
  -- 把光标 / 出生点 / 累计时长 / 墙钟全部折进半径与颜色，便于逐条比对
  ctx:circle(cursor_x + origin_x, cursor_y + origin_y,
             math.floor(age * 1000) % 100 + 1,
             math.floor(wall_ms) % 255, math.floor(a[3] * 1000) % 255, 7, 255)
end
"#;
        let (dir, id) = script_dir(body);

        let run = |host: &'static str| -> (Vec<String>, f32, f64) {
            let mut c = FxCore::load(&dir, &id, host).expect("加载");
            c.set_screen(1920.0, 1080.0);
            c.set_wall_ms(1234.0);
            // 同一串输入：移动 → 推进 → 再移动 → 推进 → 按下 → 推进 → 绘制。
            c.move_to(100.0, 200.0, None);
            c.advance(1.0 / 60.0);
            c.move_to(140.0, 260.0, None);
            c.advance(1.0 / 60.0);
            c.press(140.0, 260.0);
            c.advance(1.0 / 60.0);
            let mut list = DrawList::new();
            let out = c.render(&mut list);
            assert!(out.produced, "{host} 应有产出");
            (
                list.cmds().iter().map(|c| format!("{c:?}")).collect(),
                c.elapsed(),
                c.wall_ms,
            )
        };

        let base = run("trail");
        for host in ["hover", "clickfx", "panel"] {
            let got = run(host);
            assert_eq!(got.0, base.0, "宿主 {host} 的绘制产物与 trail 不一致");
            assert!(
                (got.1 - base.1).abs() < 1e-6,
                "宿主 {host} 的时钟与 trail 不一致"
            );
            assert!(
                (got.2 - base.2).abs() < 1e-9,
                "宿主 {host} 的墙钟与 trail 不一致"
            );
        }
    }

    /// 四个挂载点共用的临时 lua 根：`root/<dir>/probe.lua` 写的都是同一份字节。
    ///
    /// **脚本实现了几种鼠标语义，就该被调用几种，且四类宿主都得派发全套**
    /// （用户口径 2026-09-28：「一个脚本要能调用我们绝大部分功能」）。
    ///
    /// 三种鼠标功能本来就都是 Lua 写的——**差别只在脚本实现了哪个回调**：
    /// `on_move` = 拖尾语义、`on_press` = 点击语义、常驻产出 = 悬浮标语义。
    /// 于是"联合使用"不需要任何新 API：**一个脚本里写几个回调，就同时是几种效果**。
    ///
    /// 这条是对 `same_script_draws_identically_under_all_hosts` 的正向补单——
    /// 那边证明"同一串输入的产物一致"，这边证明"每个回调真的被派发了"
    /// （产物一致但压根没派发，也是全绿——正是假绿的来源）。
    #[test]
    fn one_script_receives_every_callback_it_implements() {
        // 脚本自检：每种回调被调用就把计数器 +1。
        const PROBE: &str = r#"
seen = {}
function bump(k) seen[k] = (seen[k] or 0) + 1 end
function on_move(x, y, t, p) bump("move") end
function on_press(x, y, t, p) bump("press") end
function step(dt, t, p) bump("step") end
function draw(ctx, p) bump("draw") ctx:circle(8, 8, 4, 255, 0, 255, 255) end
"#;
        let (dir, id) = script_dir(PROBE);

        for host in ["trail", "hover", "clickfx", "panel"] {
            let mut c = FxCore::load(&dir, &id, host).expect("加载");
            c.set_screen(1920.0, 1080.0);
            // 一段真实的鼠标事件流：移动 → 推进 → 按下 → 推进 → 绘制。
            c.move_to(10.0, 20.0, None);
            c.advance(1.0 / 60.0);
            c.press(30.0, 40.0);
            c.advance(1.0 / 60.0);
            let mut list = DrawList::new();
            c.render(&mut list);

            let seen: HashMap<String, i64> =
                c.engine.lua().globals().get("seen").expect("读回 seen");
            for k in ["move", "press", "step", "draw"] {
                let n = seen.get(k).copied().unwrap_or(0);
                assert!(
                    n > 0,
                    "宿主 {host} 没有派发 {k}：脚本实现了它却收不到 —— \
                     「脚本决定自己调用多少种功能」就在这一格断掉"
                );
            }
        }
    }

    /// **一个脚本可以同时驱动多种鼠标效果**（用户口径「联合使用」的硬证据）。
    ///
    /// 拖尾、点击效果、悬浮标本来就是 Lua 写的，所以"联合"不需要新 API——一个
    /// 脚本里写了 `on_move` 就跟随指针、写了 `on_press` 就响应点击，**同一帧的
    /// `draw` 里两种产物并存**。上一格证明"每个回调都派发"，这一格证明
    /// "多种语义真的合成到了同一个画面里"。
    #[test]
    fn combined_script_drives_trail_and_ripple_in_one_frame() {
        const PROBE: &str = r#"
trail = {}
ripple = nil
function on_move(x, y, t, p) trail[#trail+1] = {x, y} end
function on_press(x, y, t, p) ripple = {x, y} end
function draw(ctx, p)
  for i = 1, #trail do
    ctx:circle(trail[i][1], trail[i][2], 5, 255, 180, 60, 255)
  end
  if ripple then
    ctx:ring(ripple[1], ripple[2], 30, 3, 80, 200, 255, 255)
  end
end
"#;
        let (dir, id) = script_dir(PROBE);
        let mut c = FxCore::load(&dir, &id, "trail").expect("加载");
        c.set_screen(1920.0, 1080.0);
        // 移动三次 = 拖尾的三个采样点；再按下一次 = 一圈波纹。
        c.move_to(10.0, 10.0, None);
        c.move_to(20.0, 10.0, None);
        c.move_to(30.0, 10.0, None);
        c.press(30.0, 10.0);

        let mut list = DrawList::new();
        let out = c.render(&mut list);
        assert!(out.produced, "联合脚本这一帧必须有产出");

        let mut fills = 0usize;
        let mut strokes = 0usize;
        for cmd in list.cmds() {
            match cmd {
                crate::core::draw::DrawCmd::Fill { .. } => fills += 1,
                crate::core::draw::DrawCmd::Stroke { .. } => strokes += 1,
                _ => {}
            }
        }
        assert_eq!(fills, 3, "三个采样点 → 三条拖尾实心圆");
        assert_eq!(strokes, 1, "一次按下 → 一圈波纹描边环");
    }

    /// ★ 这才是用户诉求的真实形状——**移动的是文件，不是宿主配置**。早先的
    ///   `same_script_draws_identically_under_all_hosts` 只换了 `host` 字符串，
    ///   证明不了"文件丢进目录就被扫到、被正确的宿主驱动"这一段。
    fn mount_points(body: &str, dirs: &[&str]) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "curseen_mounts_{}_{:x}",
            std::process::id(),
            body.len().wrapping_mul(2_654_435).wrapping_add(dirs.len())
        ));
        let _ = std::fs::remove_dir_all(&root);
        for d in dirs {
            let p = root.join(d);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("probe.lua"), body).unwrap();
        }
        root
    }

    /// **同一个鼠标脚本丢进任何目录都该有效果**（用户口径 2026-09-28）。
    ///
    /// 矩阵 = 四个挂载点 × 各自宿主，每格都必须产出绘制指令：
    /// `MouseTrail/`→拖尾、`HoverDot/`→悬浮标、`ClickFx/`→点击效果、`scripts/`→复合常驻。
    /// 探针是"事件驱动"型（靠 `on_move` 累积点、`step` 推进、`draw` 才有的画）——
    /// 悬浮标旧宿主当年就是只喂 `draw` 不喂事件，这类脚本在它下面恒等于隐形。
    #[test]
    fn mouse_script_works_wherever_the_file_is_dropped() {
        const PROBE: &str = r#"
pts = {}
age = 0
function on_move(x, y, t, p) pts[#pts+1] = {x, y} end
function step(dt, t, p) age = age + dt end
function draw(ctx, p)
  if #pts == 0 then return end
  for i = 1, #pts do
    ctx:circle(pts[i][1], pts[i][2], 6, 255, 0, 255, 255)
  end
end
"#;
        let root = mount_points(PROBE, &["MouseTrail", "HoverDot", "ClickFx", "scripts"]);
        let screen = (1920.0f32, 1080.0f32);
        let rect = crate::core::geometry::Rect {
            x: 0.0,
            y: 0.0,
            w: screen.0,
            h: screen.1,
        };
        let mut rng = crate::core::math::Rng::new(7);

        // ① 拖尾宿主从 `MouseTrail/` 加载
        let mut trail =
            super::super::effects::lua_effect::LuaEffect::new(&root.join("MouseTrail"), "probe")
                .expect("拖尾加载");
        let mut l_trail = DrawList::new();
        for i in 0..4 {
            let elapsed = (i as f32 + 1.0) / 60.0;
            let ctx = &mut super::super::effects::EffectCtx {
                rng: &mut rng,
                screen: rect,
                elapsed,
                spawn_batch: 1,
                wall_ms: 0.0,
            };
            trail.on_move(
                super::super::sample::MouseSample {
                    x: 100.0 + i as f32 * 20.0,
                    y: 200.0,
                    t_ms: 0,
                },
                ctx,
            );
            let ctx = &mut super::super::effects::EffectCtx {
                rng: &mut rng,
                screen: rect,
                elapsed,
                spawn_batch: 0,
                wall_ms: 0.0,
            };
            trail.step(1.0 / 120.0, ctx);
        }
        trail.draw(&mut l_trail);
        assert!(
            !l_trail.cmds().is_empty(),
            "MouseTrail/ 下的脚本必须画出探针"
        );

        // ② 悬浮标宿主从 `HoverDot/` 加载（这里曾是"完全没效果"的重灾区）
        let mut h = super::super::hover_dot::HoverDot::new();
        h.enabled = true;
        h.set_screen(screen.0, screen.1);
        h.sync_instances_at(&root, &["HoverDot/probe".to_string()]);
        let mut l_hover = DrawList::new();
        for i in 0..4 {
            h.pos = crate::core::geometry::Point::new(100.0 + i as f32 * 20.0, 200.0);
            h.set_time((i as f32 + 1.0) / 60.0);
            h.draw(&mut l_hover);
        }
        assert!(!l_hover.cmds().is_empty(), "HoverDot/ 下的脚本必须画出探针");

        // ③ 点击效果宿主从 `ClickFx/` 加载
        let mut click = super::super::clickfx::ScriptedClick::new(
            &root.join("ClickFx"),
            "probe",
            100.0,
            200.0,
            HashMap::new(),
            screen,
        )
        .expect("点击加载");
        for _ in 0..4 {
            click.tick(1.0 / 60.0);
        }
        let mut l_click = DrawList::new();
        click.draw(&mut l_click);
        assert!(!l_click.cmds().is_empty(), "ClickFx/ 下的脚本必须画出探针");

        // ④ 复合宿主从通用的 `scripts/` 加载（用户诉求：拖尾脚本放进通用目录也要跑）
        let mut ch = super::super::companion::CompanionHost::new();
        ch.sync_at(&root, &[("scripts/probe".to_string(), HashMap::new())]);
        let mut l_comp = DrawList::new();
        for i in 0..4 {
            let t = (i as f32 + 1.0) / 60.0;
            ch.on_sample(100.0 + i as f32 * 20.0, 200.0, t, screen);
            ch.step(1.0 / 60.0, t, screen);
            ch.draw(&mut l_comp, t, screen);
        }
        assert!(
            !l_comp.cmds().is_empty(),
            "scripts/ 下的鼠标脚本必须能常驻跑"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 反例：**贴图右键菜单脚本**（`role = "image"`，只有 `run(ctx, params)`）
    /// 丢进 `MouseTrail/` —— 不许有任何鼠标效果。
    ///
    /// 用户口径（2026-09-28）："贴图右键菜单脚本放进鼠标拖尾里不应该有效果"。
    /// 两条判据缺一不可：① 加载了也不画；② **鼠标下拉压根不列出它**——
    /// 只做 ① 的话用户会看到一个"选了没反应"的条目，比看不见更糟。
    #[test]
    fn image_script_is_inert_in_mouse_directories() {
        const IMAGE: &str = r#"
name = "图片体检"
role = "image"
menu = "图片体检"
function run(ctx, params) return true end
"#;
        let root = mount_points(IMAGE, &["MouseTrail"]);
        let mut reg = crate::scripting::registry::LuaEffectRegistry::new(&root.join("MouseTrail"));
        reg.scan();
        assert!(
            reg.list().is_empty(),
            "鼠标下拉不许列出 role=image 的脚本：{:?}",
            reg.list().iter().map(|s| s.id.clone()).collect::<Vec<_>>()
        );

        // 直接加载也要是惰性的（脚本本身合法，能编译，只是没有任何特效回调）。
        let mut eff =
            super::super::effects::lua_effect::LuaEffect::new(&root.join("MouseTrail"), "probe")
                .expect("脚本合法应加载成功");
        let mut list = DrawList::new();
        let mut rng = crate::core::math::Rng::new(9);
        let rect = crate::core::geometry::Rect {
            x: 0.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
        };
        let ctx = &mut super::super::effects::EffectCtx {
            rng: &mut rng,
            screen: rect,
            elapsed: 0.1,
            spawn_batch: 1,
            wall_ms: 0.0,
        };
        eff.on_move(
            super::super::sample::MouseSample {
                x: 300.0,
                y: 300.0,
                t_ms: 0,
            },
            ctx,
        );
        eff.draw(&mut list);
        assert!(
            list.cmds().is_empty(),
            "贴图菜单脚本丢进拖尾目录不许画出任何东西"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_script_is_an_error() {
        let dir =
            std::env::temp_dir().join(format!("curseen_fxcore_missing_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(FxCore::load(&dir, "nope", "trail").is_err());
    }

    /// ★ 回归锁（2026-09-28 新增「随机文字」）：本目录**唯一用文字**的特效。
    ///
    /// 它走 `ctx:text_centered`（锚点 = 文字几何中心）——居中由渲染层按真实排版度量做，
    /// 模块层拿不到字体，自己算会偏半个字。本测试钉死三件事：① 脚本能加载并产出
    /// `TextCentered`；② 画出来的字来自 `words` 按 `sep` 切分的结果（不是整串、不是空）；
    /// ③ **`age = 0` 那一帧就必须画**——宿主按"这一帧有没有产出指令"判活，第一帧
    /// 不画等于当场判死（这条正是它刻意不做淡入的原因）。
    #[test]
    fn random_text_click_fx_draws_centered_words_from_the_phrase_list() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/lua");
        let mut c =
            FxCore::load(&root, "ClickFx/random_text", "clickfx").expect("加载 random_text.lua");
        c.set_screen(1920.0, 1080.0);
        c.set_params(HashMap::from([
            ("words".to_string(), serde_json::json!("甲|乙|丙")),
            ("count".to_string(), serde_json::json!(3)),
        ]));
        c.press(400.0, 300.0);
        // 不 advance：按下后的**第一帧**（age = 0）就必须有产出。
        let mut list = DrawList::new();
        assert!(c.render(&mut list).produced, "age=0 的那一帧就要画出文字");

        let texts: Vec<&str> = list
            .cmds()
            .iter()
            .filter_map(|cmd| match cmd {
                crate::core::draw::DrawCmd::TextCentered { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts.len(), 3, "count=3 → 三条文字，实际 {texts:?}");
        for t in &texts {
            assert!(
                ["甲", "乙", "丙"].contains(t),
                "文字必须来自按分隔符切出的词库，实际 {texts:?}"
            );
        }
    }

    /// ★ 回归锁（2026-09-28 用户报告「悬浮标实心圆设置了不透明度没效果」）：
    /// 真实 `HoverDot/solid.lua` + 8 位色跑全链——schema 声明 `alpha = true`、
    /// 脚本 `hex_to_rgb` 取第 4 值、`ctx:circle` 画进指令——`#FF000040` 的
    /// alpha 0x40 必须原样落在 Fill 颜色里（内层 u32 是 0xAARRGGBB）。
    #[test]
    fn solid_hover_alpha_reaches_draw_cmd() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/lua");
        let mut c = FxCore::load(&root, "HoverDot/solid", "hover").expect("加载 solid.lua");
        c.set_screen(1920.0, 1080.0);
        c.set_params(HashMap::from([(
            "color".to_string(),
            serde_json::json!("#FF000040"),
        )]));
        c.move_to(100.0, 100.0, None);
        let mut list = DrawList::new();
        let out = c.render(&mut list);
        assert!(out.produced, "应有产出");
        let mut alphas = Vec::new();
        for cmd in list.cmds() {
            if let crate::core::draw::DrawCmd::Fill {
                paint: crate::core::draw::Paint::Solid(color, _),
                ..
            } = cmd
            {
                alphas.push(color.a());
            }
        }
        assert!(
            alphas.contains(&0x40),
            "8 位色的 alpha 0x40 必须进绘制指令，实际 {:?}",
            alphas
        );
    }
}
