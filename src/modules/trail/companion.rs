//! 复合脚本宿主：`role = "composite"` 的脚本，**API 全开放**（2026-09-26 用户口径）。
//!
//! # 定位
//!
//! 拖尾/悬浮标/点击效果/击键音效本身就是 Lua 脚本实现的——把「跑这些脚本」的能力
//! 原样开放给 `scripts/` 里的用户脚本：一个复合脚本可以**同时**实现拖尾 + 点击声音 +
//! 悬浮标 + 任意键盘/鼠标反应，而不必借用内置功能。多个复合脚本同时开启、各自独立
//! VM 独立状态（「同时开 3 种悬浮标」即：写 3 个复合脚本全部运行）。
//!
//! # 脚本契约（assets/lua/scripts/ 下的示例脚本头注释是权威版本）
//!
//! 顶层声明 `role = "composite"`；回调全部**可选**，按需定义：
//!
//! | 回调 | 时机 | 与特效脚本的关系 |
//! |---|---|---|
//! | `init()` | 启用/热重载后 | 同 `MouseTrail` 特效 |
//! | `on_move(x, y, t, params)` | 鼠标采样（含快移插值点） | 同特效（轨迹类密度） |
//! | `on_mouse_down(button, x, y, params)` | 鼠标按下 | ★ 复合专属（`button` = `left/right/middle/x1/x2`） |
//! | `on_key(key, mods, repeated, params)` | 键盘按下 | 参数形状同击键音效 |
//! | `step(dt, t, params)` | 固定步长推进 | 同特效 |
//! | `draw(ctx, params)` | 每帧绘制 | 同特效（`ctx:circle` 等） |
//!
//! 全局：特效脚本全套（`screen_w/h`、`elapsed`、`mouse_x/y`、`rng`、`hex_to_rgb`、
//! `clamp`、`lerp`）＋ **`play_sound(file, volume?)`**——解析链与击键音效「音效文件」
//! 行同源（绝对路径 / 便携相对 / `assets/Sounds/` 裸名），播放走 `platform::audio`。
//!
//! # 帧 / 生命周期
//!
//! - 只有 `draw` 真的产出指令才续帧（与特效同规则，空闲 0% CPU）；要持续动画就在
//!   `draw` 里画东西。`on_key`/`on_mouse_down` 想驱动的淡出动画同理：在 `draw` 里画。
//! - ★ 上表**一个回调都没声明**的脚本（只写 `schema()` + `run()`，例如 `scripts/AI/ai_chat`
//!   —— 它开的是自己的原生窗）被当作**一次性 / 开窗脚本**：它不占覆盖层的帧，覆盖层
//!   该休眠就休眠（整屏缓冲 1080p ≈ 8 MB + 60fps 整屏重绘，2026-09-30 内存归因）。
//!   判据见 [`CompanionHost::any_needs_frame`]。
//! - 参数变化（面板改滑条）经 [`CompanionHost::sync`] 重放；脚本文件改动经
//!   [`Companion::ensure_fresh`] 按 (mtime, 字节数) 热重建（与 `EngineCache` 同判据）。
//! - 脚本缺失/损坏 → 实例引擎置 `None`，回调静默跳过（每因告警一次），不拖垮其它实例。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value as Json;

use crate::core::draw::DrawList;
use crate::scripting::api_draw;
use crate::scripting::engine::ScriptingEngine;

use super::controller::{FIXED_DT, INTERPOLATE_SPACING_PX, MAX_INTERP_PER_SEG};
use super::fx_core::{MAX_FRAME_DT, MAX_SUBSTEPS};

/// 本宿主在 Lua 全局 `host` 里的标识。
const HOST: &str = "panel";

/// 覆盖层的**生命周期回调**：声明了其中**任意一个** = 这个脚本要吃覆盖层的帧。
///
/// ★ 反向用途（2026-09-30 内存归因）：一个都没声明的脚本（只写 `schema()` + `run()`，
///   例如 `scripts/AI/ai_chat` —— 它开自己的原生窗，一个像素都不画在覆盖层上）不该让
///   覆盖层保持唤醒。唤醒 = 整屏缓冲常驻（1080p 约 8 MB 私有工作集）+ 60fps 整屏重绘。
const OVERLAY_CALLBACKS: [&str; 6] = ["init", "on_move", "on_mouse_down", "on_key", "step", "draw"];

/// 脚本是否声明了任一覆盖层回调（决定它要不要覆盖层出帧）。
fn declares_overlay_callback(lua: &mlua::Lua) -> bool {
    OVERLAY_CALLBACKS.iter().any(|n| {
        matches!(
            lua.globals().get::<mlua::Value>(*n),
            Ok(mlua::Value::Function(_))
        )
    })
}

/// 单个复合脚本实例 = 一个运行中的脚本（独立 VM）。
pub struct Companion {
    /// 脚本 id = 相对 `assets/lua` 的路径 stem（`scripts/<脚本名>`）。
    pub id: String,
    /// 引擎根（`assets/lua`）：`load_script(id)` 即根下按相对路径拼 `.lua`。
    root: PathBuf,
    engine: Option<ScriptingEngine>,
    params: HashMap<String, Json>,
    /// 热重载判据：上一次加载时的 (mtime, 字节数)。与 `EngineCache` 同判据——
    /// 只看 mtime 会漏掉同秒内改内容（NTFS 时间戳粒度粗）。
    mtime: Option<SystemTime>,
    len: u64,
    /// 本帧 `draw` 是否产出指令（决定续帧）。
    active: bool,
    /// ★ 本脚本是否**声明过**覆盖层回调（见 [`declares_overlay_callback`]）。
    /// 一次性 / 开窗脚本（只有 `schema` + `run`）为 false —— 它们不该让覆盖层保持唤醒。
    needs_frame: bool,
    /// 脚本是否声明了 `on_wheel`（钩子按需转发普通滚轮）。
    wants_wheel: bool,
    hide_cursor: bool,
    /// "坏脚本只告警一次"标记（重建成功后复位）。
    fault_logged: bool,
    /// ★ 真实墙钟毫秒（脚本全局 `wall_ms`）。四类宿主一致注入——此前只有悬浮标注入，
    ///   用墙钟做静止检测 / 超时淡出的脚本换个目录就失灵。
    wall_ms: f64,
    /// 最近一次已知指针位置（契约统一后与拖尾/悬浮标/点击同字段）。
    cursor: (f32, f32),
    /// 出生点（首个采样点）。脚本读 `origin_x`/`origin_y` 即拿到它。
    origin: Option<(f32, f32)>,
    /// 上一次移动的 (x,y,t)：算 `cursor_vx/vy/speed/heading` 用。
    last_move: Option<(f32, f32, f32)>,
    vx: f32,
    vy: f32,
    speed: f32,
    heading: f32,
    mouse_down: bool,
    mouse_button: String,
}

/// 给实例 VM 注入 `play_sound(file, volume?) -> bool` 全局。
///
/// 文件解析与击键音效「音效文件」行同一条链（`key_sound::stored_file_path`：
/// 绝对路径 → 便携相对 → `assets/Sounds/` 裸名补扩展名），返回是否真的发起了播放。
/// 放在宿主注入而不是 `scripting` 层：发声要走 `platform::audio`，scripting 层不下探。
fn inject_play_sound(lua: &mlua::Lua) -> mlua::Result<()> {
    let f = lua.create_function(|_, (file, volume): (String, Option<u8>)| {
        let vol = u32::from(volume.unwrap_or(100)).min(100);
        match crate::modules::key_sound::stored_file_path(&file) {
            Some(p) => Ok(crate::platform::audio::play(&p.to_string_lossy(), vol)),
            None => Ok(false),
        }
    })?;
    lua.globals().set("play_sound", f)
}

impl Companion {
    /// 从 `root` 目录加载脚本构造实例。失败不 panic（引擎置 None，回调静默跳过）。
    pub fn load_in(root: &Path, id: &str) -> Self {
        let (mtime, len) = file_stamp(&root.join(format!("{id}.lua")));
        let engine = ScriptingEngine::new(root).and_then(|e| {
            e.load_script(id)?;
            let hide = e.get_hide_cursor();
            inject_play_sound(e.lua())?;
            // ★ init 可选（复合脚本可以只写 on_move/draw）——`call_init` 对缺失的
            //   全局会报错，这里先探再调。显式标注返回 ()：never type fallback 警告。
            if let Ok(mlua::Value::Function(f)) = e.lua().globals().get::<mlua::Value>("init") {
                f.call::<()>(())?;
            }
            let needs_frame = declares_overlay_callback(e.lua());
            let wants_wheel = matches!(
                e.lua().globals().get::<mlua::Value>("on_wheel"),
                Ok(mlua::Value::Function(_))
            );
            Ok((e, hide, needs_frame, wants_wheel))
        });
        match engine {
            Ok((engine, hide, needs_frame, wants_wheel)) => Self {
                id: id.to_string(),
                root: root.to_path_buf(),
                engine: Some(engine),
                params: HashMap::new(),
                mtime,
                len,
                active: false,
                needs_frame,
                wants_wheel,
                hide_cursor: hide,
                fault_logged: false,
                wall_ms: 0.0,
                cursor: (0.0, 0.0),
                origin: None,
                last_move: None,
                vx: 0.0,
                vy: 0.0,
                speed: 0.0,
                heading: 0.0,
                mouse_down: false,
                mouse_button: "left".to_string(),
            },
            Err(e) => {
                log::warn!("复合脚本 {id}.lua 加载失败：{e}（该实例回调静默跳过）");
                Self {
                    id: id.to_string(),
                    root: root.to_path_buf(),
                    engine: None,
                    params: HashMap::new(),
                    mtime,
                    len,
                    active: false,
                    // 加载失败的脚本本来就什么也做不了，没理由让它占着覆盖层的帧。
                    needs_frame: false,
                    wants_wheel: false,
                    hide_cursor: false,
                    fault_logged: true,
                    wall_ms: 0.0,
                    cursor: (0.0, 0.0),
                    origin: None,
                    last_move: None,
                    vx: 0.0,
                    vy: 0.0,
                    speed: 0.0,
                    heading: 0.0,
                    mouse_down: false,
                    mouse_button: "left".to_string(),
                }
            }
        }
    }

    /// 从出厂 lua 根加载（生产入口）。
    pub fn load(id: &str) -> Self {
        Self::load_in(&crate::core::paths::lua_ext_dir(), id)
    }

    /// 热重载检查：脚本文件 (mtime, 字节数) 变了 → 重建 VM、重放参数（脚本状态清零，
    /// 与 `EngineCache` 的热更新语义一致）。
    pub fn ensure_fresh(&mut self) {
        let (mtime, len) = file_stamp(&self.root.join(format!("{}.lua", self.id)));
        if mtime == self.mtime && len == self.len {
            return;
        }
        let params = std::mem::take(&mut self.params);
        let fresh = Self::load_in(&self.root, &self.id);
        self.engine = fresh.engine;
        self.hide_cursor = fresh.hide_cursor;
        // ★ 热重载可能**改写了脚本能力**（原本只开窗的脚本加了 draw）：这条必须跟着更新，
        //   否则改完之后覆盖层按旧印象继续休眠 / 继续空转。
        self.needs_frame = fresh.needs_frame;
        self.wants_wheel = fresh.wants_wheel;
        self.active = false;
        self.mtime = fresh.mtime;
        self.len = fresh.len;
        self.fault_logged = fresh.fault_logged;
        self.params = params;
        log::info!("复合脚本 {} 已热重载（文件改动生效）", self.id);
    }

    /// 注入该实例的参数表（缺键由脚本内 `params.x or 默认` 兜底）。
    pub fn set_params(&mut self, params: HashMap<String, Json>) {
        self.params = params;
    }

    /// 本帧 `draw` 是否产出指令（续帧依据）。
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// 脚本是否声明了 `on_wheel`（钩子据此决定要不要转发普通滚轮）。
    pub fn wants_wheel(&self) -> bool {
        self.wants_wheel
    }

    /// 脚本是否声明 `hide_cursor = true`（激光笔类）。
    pub fn wants_cursor_hidden(&self) -> bool {
        self.hide_cursor
    }

    /// 引擎存在时执行 `f`；不存在 / 出错时每因告警一次（坏脚本不刷日志、不 panic）。
    fn with_engine(
        &mut self,
        f: impl FnOnce(&ScriptingEngine, &HashMap<String, Json>) -> mlua::Result<()>,
    ) {
        let Some(engine) = self.engine.as_ref() else {
            if !self.fault_logged {
                self.fault_logged = true;
                log::warn!("复合脚本 {} 未加载（缺失或语法错误），回调跳过", self.id);
            }
            return;
        };
        if let Err(e) = f(engine, &self.params) {
            if !self.fault_logged {
                self.fault_logged = true;
                log::warn!("[{}] 复合脚本回调出错：{e}（同因后续静默）", self.id);
            }
        }
    }

    /// 鼠标采样（原始点；快移插值由宿主统一补，见 [`CompanionHost::on_sample`]）。
    fn on_move(&mut self, x: f32, y: f32, t: f32, screen: (f32, f32)) {
        if let Some((lx, ly, lt)) = self.last_move {
            let dt = t - lt;
            if dt > 1e-4 {
                self.vx = (x - lx) / dt / 60.0;
                self.vy = (y - ly) / dt / 60.0;
                self.speed = (self.vx * self.vx + self.vy * self.vy).sqrt();
                self.heading = self.vy.atan2(self.vx);
            }
        }
        self.last_move = Some((x, y, t));
        self.cursor = (x, y);
        if self.origin.is_none() {
            self.origin = Some((x, y));
        }
        let (cur, org) = (self.cursor, self.origin.unwrap_or(self.cursor));
        let wall = self.wall_ms;
        let (vx, vy, speed, heading, down) =
            (self.vx, self.vy, self.speed, self.heading, self.mouse_down);
        let button = self.mouse_button.clone();
        self.with_engine(move |e, params| {
            e.update_env(cur, org, t, screen, HOST)?;
            // ★ 墙钟跟着环境一起注入（四类宿主一致）：用 `wall_ms` 做静止检测 /
            //   超时淡出的脚本在 `scripts/` 下也拿得到真值。
            e.set_wall_ms(wall)?;
            e.set_pointer_state(vx, vy, speed, heading, down, &button)?;
            e.call_on_move(x, y, t, params)
        });
    }

    /// 鼠标按下。
    fn on_button(&mut self, button: &str, x: f32, y: f32, t: f32, screen: (f32, f32)) {
        self.cursor = (x, y);
        self.mouse_down = true;
        self.mouse_button = button.to_string();
        let (cur, org) = (self.cursor, self.origin.unwrap_or(self.cursor));
        let wall = self.wall_ms;
        let (vx, vy, speed, heading, down) =
            (self.vx, self.vy, self.speed, self.heading, self.mouse_down);
        let btn = self.mouse_button.clone();
        self.with_engine(move |e, params| {
            e.update_env(cur, org, t, screen, HOST)?;
            e.set_wall_ms(wall)?;
            e.set_pointer_state(vx, vy, speed, heading, down, &btn)?;
            e.call_on_button(button, x, y, t, params)
        });
    }

    /// 鼠标抬起：清按住态并转发 `on_mouse_up`（缺回调静默跳过）。
    fn on_mouse_up(&mut self, button: &str, x: f32, y: f32, t: f32, screen: (f32, f32)) {
        self.cursor = (x, y);
        self.mouse_down = false;
        self.mouse_button = button.to_string();
        let (cur, org) = (self.cursor, self.origin.unwrap_or(self.cursor));
        let wall = self.wall_ms;
        let (vx, vy, speed, heading, down) =
            (self.vx, self.vy, self.speed, self.heading, self.mouse_down);
        let btn = self.mouse_button.clone();
        self.with_engine(move |e, params| {
            e.update_env(cur, org, t, screen, HOST)?;
            e.set_wall_ms(wall)?;
            e.set_pointer_state(vx, vy, speed, heading, down, &btn)?;
            e.call_on_mouse_up(button, x, y, params)
        });
    }

    /// 滚轮：刷新光标/环境，转发 `on_wheel`（缺回调静默跳过）。
    fn on_wheel(
        &mut self,
        delta: i32,
        horizontal: bool,
        x: f32,
        y: f32,
        t: f32,
        screen: (f32, f32),
    ) {
        self.cursor = (x, y);
        let (cur, org) = (self.cursor, self.origin.unwrap_or(self.cursor));
        let wall = self.wall_ms;
        let (vx, vy, speed, heading, down) =
            (self.vx, self.vy, self.speed, self.heading, self.mouse_down);
        let btn = self.mouse_button.clone();
        self.with_engine(move |e, params| {
            e.update_env(cur, org, t, screen, HOST)?;
            e.set_wall_ms(wall)?;
            e.set_pointer_state(vx, vy, speed, heading, down, &btn)?;
            e.call_on_wheel(delta, horizontal, x, y, params)
        });
    }

    /// 键盘按下。`mods` = (ctrl, alt, shift, win)。
    fn on_key(
        &mut self,
        key: &str,
        mods: (bool, bool, bool, bool),
        repeated: bool,
        t: f32,
        screen: (f32, f32),
    ) {
        let (cur, org) = (self.cursor, self.origin.unwrap_or(self.cursor));
        let wall = self.wall_ms;
        self.with_engine(|e, params| {
            e.update_env(cur, org, t, screen, HOST)?;
            e.set_wall_ms(wall)?;
            e.call_on_key(key, mods.0, mods.1, mods.2, mods.3, repeated, params)?;
            Ok(())
        });
    }

    /// ★ 固定步长推进：`dt` 恒为 [`FIXED_DT`]（与拖尾 / 悬浮标 / 点击同一节拍），
    /// 由 [`CompanionHost::step`] 的累加器按子步调用。
    fn step(&mut self, dt: f32, t: f32, screen: (f32, f32)) {
        let (cur, org) = (self.cursor, self.origin.unwrap_or(self.cursor));
        let wall = self.wall_ms;
        let (vx, vy, speed, heading, down) =
            (self.vx, self.vy, self.speed, self.heading, self.mouse_down);
        let btn = self.mouse_button.clone();
        self.with_engine(move |e, params| {
            e.update_env(cur, org, t, screen, HOST)?;
            e.set_wall_ms(wall)?;
            e.set_pointer_state(vx, vy, speed, heading, down, &btn)?;
            e.call_step(dt, t, params)?;
            Ok(())
        });
    }

    /// 绘制（追加进同一个 `DrawList`）。产出指令 = 本帧 active（续帧依据）。
    fn draw(&mut self, list: &mut DrawList, t: f32, screen: (f32, f32)) {
        let Some(engine) = self.engine.as_ref() else {
            return;
        };
        let before = list.len_all();
        api_draw::set_draw_target(list);
        let (cur, org) = (self.cursor, self.origin.unwrap_or(self.cursor));
        let _ = engine.update_env(cur, org, t, screen, HOST);
        let _ = engine.set_wall_ms(self.wall_ms);
        let _ = engine.set_pointer_state(
            self.vx,
            self.vy,
            self.speed,
            self.heading,
            self.mouse_down,
            &self.mouse_button,
        );
        let _ = engine.call_draw(&self.params);
        api_draw::clear_draw_target();
        // ★ 与 LuaEffect 同款：用 `len_all()`（跨层计数），脚本声明 base/top 层时
        //   指令落在别的桶，只查 cmds() 会把垫底/顶层的特效误判成"没画"。
        self.active = list.len_all() > before;
    }
}

/// 文件存在戳：(mtime, 字节数)；读不到 → (None, 0)。
fn file_stamp(path: &Path) -> (Option<SystemTime>, u64) {
    match std::fs::metadata(path) {
        Ok(m) => (m.modified().ok(), m.len()),
        Err(_) => (None, 0),
    }
}

/// 复合脚本宿主：多个 [`Companion`] 的对账与事件分发。
pub struct CompanionHost {
    instances: Vec<Companion>,
    /// 上一次对账时的启用列表长度（见 [`Self::any_needs_frame`] 的保守分支）。
    synced: usize,
    /// 是否**至少对账过一次**。从未对账过时一切未知，帧门必须保守取 true。
    synced_once: bool,
    /// 快移插值锚点（上一个真实采样点）。★ 与拖尾特效的 `prev_forward` 分开：
    /// 复合脚本独立于 `trail_enabled`，锚点不能被特效分支清掉。
    prev: Option<(f32, f32)>,
    /// 固定步长累加器（秒）：不足 [`FIXED_DT`] 的零头留到下一帧。
    fixed_acc: f32,
    /// 最近一次注入的真实墙钟毫秒。**必须存一份**：`set_wall_ms` 只写当时已存在的实例，
    /// 而新脚本可能在对账时才载入 —— 不补这一份，新实例的 `wall_ms` 从 0 开始，
    /// 刚载入那一帧的滚轮/长按会拿到错误的时间起点（实测：twinkle 的 burst 出生即过期）。
    wall_ms: f64,
}

impl CompanionHost {
    pub fn new() -> Self {
        Self {
            instances: Vec::new(),
            synced: 0,
            synced_once: false,
            prev: None,
            fixed_acc: 0.0,
            wall_ms: 0.0,
        }
    }

    /// 是否有任一实例**真的要吃覆盖层的帧**。
    ///
    /// ★ 保守优先：宿主还没对账过（或实例数还落后于启用列表）时一律返回 true ——
    ///   宁可白醒几帧，也不能让"刚勾上的复合脚本因为覆盖层休眠而永远拿不到帧"
    ///   （`frame()` 只在 `paint()` 里跑，park 态下连对账的机会都没有）。
    pub fn any_needs_frame(&self) -> bool {
        if !self.synced_once || self.instances.len() < self.synced {
            return true;
        }
        self.instances.iter().any(|c| c.needs_frame)
    }

    /// 按启用列表对账（生产入口，根 = 出厂 lua 根）。
    ///
    /// 每项 = (脚本 id, 该脚本参数现值)。同 id 保实例不重载；增删按列表；顺序随列表。
    pub fn sync(&mut self, active: &[(String, HashMap<String, Json>)]) {
        self.sync_at(&crate::core::paths::lua_ext_dir(), active);
    }

    /// [`Self::sync`] 的可注入根版本（测试用）。
    pub fn sync_at(&mut self, root: &Path, active: &[(String, HashMap<String, Json>)]) {
        let mut old: Vec<Companion> = std::mem::take(&mut self.instances);
        for (id, params) in active {
            let mut c = match old.iter().position(|c| &c.id == id) {
                Some(i) => old.remove(i),
                None => Companion::load_in(root, id),
            };
            c.set_params(params.clone());
            // 新载入的实例补齐当前墙钟（见字段注释）。
            c.wall_ms = self.wall_ms;
            self.instances.push(c);
        }
        self.synced = active.len();
        self.synced_once = true;
        // 未被保留的旧实例在此 drop（释放 VM）。
        if active.is_empty() {
            self.prev = None;
        }
    }

    /// 注入真实墙钟毫秒（脚本全局 `wall_ms`）——四类宿主一致。
    pub fn set_wall_ms(&mut self, ms: f64) {
        self.wall_ms = ms;
        for c in &mut self.instances {
            c.wall_ms = ms;
        }
    }

    /// 热重载检查（每 tick 一次；代价 = 每实例一次 stat）。
    pub fn ensure_fresh(&mut self) {
        for c in &mut self.instances {
            c.ensure_fresh();
        }
    }

    /// 鼠标采样分发：真实点 + 快移插值点（与轨迹类特效同密度，复合拖尾不断线）。
    pub fn on_sample(&mut self, x: f32, y: f32, t: f32, screen: (f32, f32)) {
        if let Some((px, py)) = self.prev {
            for (ix, iy) in super::controller::TrailModule::interpolate(
                (px, py),
                (x, y),
                INTERPOLATE_SPACING_PX,
                MAX_INTERP_PER_SEG,
            ) {
                for c in &mut self.instances {
                    c.on_move(ix, iy, t, screen);
                }
            }
        }
        for c in &mut self.instances {
            c.on_move(x, y, t, screen);
        }
        self.prev = Some((x, y));
    }

    /// 鼠标按下分发。`button` = `left/right/middle/x1/x2`。
    pub fn on_button(&mut self, button: &str, x: f32, y: f32, t: f32, screen: (f32, f32)) {
        for c in &mut self.instances {
            c.on_button(button, x, y, t, screen);
        }
    }

    /// 鼠标抬起分发。
    pub fn on_mouse_up(&mut self, button: &str, x: f32, y: f32, t: f32, screen: (f32, f32)) {
        for c in &mut self.instances {
            c.on_mouse_up(button, x, y, t, screen);
        }
    }

    /// 滚轮分发（只发给声明了 `on_wheel` 的实例）。
    pub fn on_wheel(
        &mut self,
        delta: i32,
        horizontal: bool,
        x: f32,
        y: f32,
        t: f32,
        screen: (f32, f32),
    ) {
        for c in &mut self.instances {
            if c.wants_wheel {
                c.on_wheel(delta, horizontal, x, y, t, screen);
            }
        }
    }

    /// 是否有任一实例声明了 `on_wheel`（钩子武装判据）。
    pub fn any_wheel_wanted(&self) -> bool {
        self.instances.iter().any(|c| c.wants_wheel)
    }

    /// 键盘按下分发（键名与击键回显同源，`KeyCode::display_name`）。
    pub fn on_key(
        &mut self,
        key: &str,
        mods: (bool, bool, bool, bool),
        repeated: bool,
        t: f32,
        screen: (f32, f32),
    ) {
        for c in &mut self.instances {
            c.on_key(key, mods, repeated, t, screen);
        }
    }

    /// ★ 固定步长推进：**与拖尾 / 悬浮标 / 点击同一节拍**——`dt` 恒为 [`FIXED_DT`]，
    /// 不足一步的零头留在累加器里带到下一帧（此前这里按整帧 dt 调一次，同一脚本在
    /// `scripts/` 下每帧只走 1 步、在 `MouseTrail/` 下走 2 步，速度对不上）。
    /// 单帧最多补 [`MAX_SUBSTEPS`] 步：卡顿后不把丢掉的时间一次补完。
    pub fn step(&mut self, dt: f32, t: f32, screen: (f32, f32)) {
        self.fixed_acc += dt.min(MAX_FRAME_DT);
        let mut steps = 0u32;
        while self.fixed_acc >= FIXED_DT && steps < MAX_SUBSTEPS {
            for c in &mut self.instances {
                c.step(FIXED_DT, t, screen);
            }
            self.fixed_acc -= FIXED_DT;
            steps += 1;
        }
        if steps == MAX_SUBSTEPS {
            self.fixed_acc = 0.0;
        }
    }

    /// 绘制（追加进同一个 `DrawList`）。
    pub fn draw(&mut self, list: &mut DrawList, t: f32, screen: (f32, f32)) {
        for c in &mut self.instances {
            c.draw(list, t, screen);
        }
    }

    /// 本帧是否有实例画了东西（续帧依据）。
    pub fn is_animating(&self) -> bool {
        self.instances.iter().any(|c| c.is_active())
    }

    /// 是否有运行中的实例（dirty 判据）。
    pub fn has_scripts(&self) -> bool {
        !self.instances.is_empty()
    }

    /// 任一实例声明隐藏系统指针。
    pub fn wants_cursor_hidden(&self) -> bool {
        self.instances.iter().any(|c| c.wants_cursor_hidden())
    }

    /// 实例数（诊断）。
    pub fn len(&self) -> usize {
        self.instances.len()
    }

    /// 是否无实例（与 [`Self::len`] 配对，clippy 要求成对出现）。
    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    /// 实例 id 列表（诊断）。
    pub fn ids(&self) -> Vec<String> {
        self.instances.iter().map(|c| c.id.clone()).collect()
    }
}

impl Default for CompanionHost {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 在临时目录写脚本并 sync 出宿主。目录名带内容指纹避免并行互相掏空。
    fn host_with(files: &[(&str, &str)], active: &[&str]) -> (PathBuf, CompanionHost) {
        let dir = std::env::temp_dir().join(format!(
            "curseen_companion_{}_{:x}",
            std::process::id(),
            files
                .iter()
                .map(|(n, c)| format!("{n}:{}", c.len()))
                .collect::<Vec<_>>()
                .concat()
                .len()
                .wrapping_mul(1_000_003)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (rel, body) in files {
            let p = dir.join(format!("{rel}.lua"));
            if let Some(parent) = p.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(&p, body).unwrap();
        }
        let mut h = CompanionHost::new();
        let list: Vec<(String, HashMap<String, Json>)> = active
            .iter()
            .map(|id| ((*id).to_string(), HashMap::new()))
            .collect();
        h.sync_at(&dir, &list);
        (dir, h)
    }

    #[test]
    fn sync_reconciles_by_id_and_order() {
        let body = "role = \"composite\"\nfunction init() end\n";
        let (d, mut h) = host_with(
            &[("scripts/a", body), ("scripts/b", body)],
            &["scripts/a", "scripts/b"],
        );
        assert_eq!(h.ids(), vec!["scripts/a", "scripts/b"]);
        h.sync_at(
            &d,
            &[
                ("scripts/b".to_string(), HashMap::new()),
                ("scripts/a".to_string(), HashMap::new()),
            ],
        );
        assert_eq!(h.ids(), vec!["scripts/b", "scripts/a"], "顺序随列表");
        h.sync_at(&d, &[]);
        assert!(h.ids().is_empty(), "清列表 = 全部停止");
    }

    /// 滚轮只发给声明了 `on_wheel` 的实例；`any_wheel_wanted` 是钩子武装判据。
    #[test]
    fn wheel_reaches_only_scripts_that_declare_it() {
        let with_wheel = "got = 0\nfunction on_wheel(d, h, x, y, params) got = d end\n";
        let (d, mut h) = host_with(&[("scripts/w", with_wheel)], &["scripts/w"]);
        assert!(h.any_wheel_wanted(), "声明 on_wheel 的脚本应要求钩子武装");
        h.on_wheel(120, false, 12.0, 34.0, 0.1, (1920.0, 1080.0));
        let got: i32 = h.instances[0]
            .engine
            .as_ref()
            .expect("实例应有引擎")
            .lua()
            .globals()
            .get("got")
            .expect("got 应是整数");
        assert_eq!(got, 120, "滚轮增量应原样传给 Lua");

        let (d2, h2) = host_with(&[("scripts/n", "function draw() end\n")], &["scripts/n"]);
        assert!(!h2.any_wheel_wanted(), "未声明 on_wheel 的脚本不应武装滚轮");
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&d2);
    }

    /// 真实 `twinkle.lua`：滚轮 → step → draw 必须出画（隔离控制器路径）。
    #[test]
    fn real_twinkle_wheel_produces_commands() {
        let root = crate::core::paths::lua_ext_dir();
        let mut h = CompanionHost::new();
        h.sync_at(
            &root,
            &[("scripts/MFCMouseEffect/twinkle".to_string(), HashMap::new())],
        );
        assert!(h.any_wheel_wanted(), "twinkle 应声明 on_wheel");
        h.set_wall_ms(60_000.0);
        h.on_wheel(120, false, 300.0, 200.0, 0.5, (1920.0, 1080.0));
        h.set_wall_ms(60_100.0);
        h.step(1.0 / 60.0, 0.6, (1920.0, 1080.0));
        let mut list = DrawList::new();
        h.draw(&mut list, 0.6, (1920.0, 1080.0));
        assert!(
            list.len_all() > 0,
            "twinkle 收到滚轮后应产出图元，实际 {}",
            list.len_all()
        );
    }

    #[test]
    fn one_shot_script_declares_no_overlay_callback_and_does_not_need_frames() {
        // ★ 内存门禁（2026-09-30）：只写 `schema` + `run` 的一次性 / 开窗脚本（AI 对话）
        //   若被当成"要吃帧"，覆盖层会为它整屏缓冲常驻 + 60fps 整屏重绘，而它一个像素
        //   都没画。对账前（未同步）必须保守取 true，否则"刚勾上的脚本"永远醒不过来。
        let one_shot =
            "name = \"AI 写脚本\"\nfunction schema() return {} end\nfunction run(ctx, params) end\n";
        let (_d, h) = host_with(&[("scripts/window", one_shot)], &["scripts/window"]);
        assert!(
            !h.any_needs_frame(),
            "只有 schema+run 的脚本不该让覆盖层保持唤醒"
        );

        // 声明了 draw 的脚本照旧要帧。
        let drawer = "function draw(ctx, params) end\n";
        let (_d2, h2) = host_with(&[("scripts/drawer", drawer)], &["scripts/drawer"]);
        assert!(h2.any_needs_frame(), "声明了 draw 的脚本必须吃帧");

        // 从未对账过 → 一切未知 → 保守 true。
        assert!(
            CompanionHost::new().any_needs_frame(),
            "未同步时不能把帧门关死（会死锁：醒不过来就永远同步不了）"
        );
    }

    #[test]
    fn on_move_receives_samples_and_draw_produces_commands() {
        // on_move 记点（含插值点），draw 全部画出来：判据 = DrawList 指令数，
        // 间距 100px 的两点按 4px 间距插值应产出 >2 个点。
        let body = r#"
role = "composite"
pts = {}
function on_move(x, y, t, params) pts[#pts+1] = {x, y} end
function draw(ctx, params)
  for _, p in ipairs(pts) do ctx:circle(p[1], p[2], 3, 255, 0, 0, 255) end
end
"#;
        let (_d, mut h) = host_with(&[("scripts/trail", body)], &["scripts/trail"]);
        h.on_sample(0.0, 0.0, 0.0, (1920.0, 1080.0));
        h.on_sample(100.0, 0.0, 0.016, (1920.0, 1080.0));
        let mut list = DrawList::new();
        h.draw(&mut list, 0.016, (1920.0, 1080.0));
        assert!(h.is_animating(), "画了东西 = 动画中（续帧）");
        assert!(
            list.len_all() > 4,
            "插值点应补出 >2 个点，实际 {} 指令",
            list.len_all()
        );
        // 再画一帧（没有新采样）→ 脚本仍产出指令？不：pts 没变但 draw 每帧都会
        // 重新画一遍（复合脚本与特效不同，draw 是每帧回调，脚本自己决定画不画）。
        // 这里判据是"上一帧画了 = active"，active 在 draw 时重算——仍画 → 仍续帧。
        let mut list2 = DrawList::new();
        h.draw(&mut list2, 0.032, (1920.0, 1080.0));
        assert!(h.is_animating(), "脚本每帧都画 = 持续续帧（动画型）");
    }

    /// ★ 2026-09-27 用户口径（"每个文件夹都放了 fireflies，scripts/ 下没效果"）：
    /// `role="effect"` 的特效脚本直接复制进 `scripts/` 也要能当常驻卡片运行——
    /// companion 宿主对特效回调（on_move/step/draw）全套驱动，role 不该是闸门。
    #[test]
    fn effect_role_script_runs_as_companion_card() {
        let body = r#"
role = "effect"
pts = {}
function on_move(x, y, t, params) pts[#pts+1] = {x, y} end
function draw(ctx, params)
  for _, p in ipairs(pts) do ctx:circle(p[1], p[2], 3, 255, 0, 0, 255) end
end
"#;
        let (_d, mut h) = host_with(
            &[("scripts/fireflies_copy", body)],
            &["scripts/fireflies_copy"],
        );
        h.on_sample(0.0, 0.0, 0.0, (1920.0, 1080.0));
        h.on_sample(100.0, 0.0, 0.016, (1920.0, 1080.0));
        let mut list = DrawList::new();
        h.draw(&mut list, 0.016, (1920.0, 1080.0));
        assert!(
            list.len_all() > 4,
            "effect 脚本挂常驻宿主应能画（{} 指令）",
            list.len_all()
        );
        assert!(h.is_animating(), "画了东西 = 续帧");
    }

    #[test]
    fn on_mouse_down_receives_button_and_play_sound_injected() {
        let body = r#"
role = "composite"
last = nil
function on_mouse_down(button, x, y, params)
  last = { button, x, y }
  assert(play_sound ~= nil, "play_sound 未注入")
  play_sound("不存在的音效", 50)
end
"#;
        let (d, mut h) = host_with(&[("scripts/click", body)], &["scripts/click"]);
        h.on_button("right", 12.0, 34.0, 1.0, (1920.0, 1080.0));
        // 直接读实例 VM 的全局验证回调确实收到 (button, x, y)（同 crate 私有可访问）。
        let engine = h.instances[0].engine.as_ref().expect("脚本应加载成功");
        let last: mlua::Table = engine.lua().globals().get("last").expect("last 已写");
        assert_eq!(last.get::<String>(1).unwrap(), "right");
        assert_eq!(last.get::<f64>(2).unwrap(), 12.0);
        assert_eq!(last.get::<f64>(3).unwrap(), 34.0);
        let _ = d;
    }

    #[test]
    fn on_key_receives_key_and_mods() {
        let body = r#"
role = "composite"
seen = {}
function on_key(key, mods, repeated, params)
  seen[#seen+1] = key .. (mods.ctrl and "+ctrl" or "") .. tostring(repeated)
end
"#;
        let (_d, mut h) = host_with(&[("scripts/keys", body)], &["scripts/keys"]);
        h.on_key(
            "A",
            (false, false, false, false),
            false,
            0.0,
            (1920.0, 1080.0),
        );
        h.on_key(
            "LCtrl",
            (true, false, false, false),
            true,
            0.0,
            (1920.0, 1080.0),
        );
        let engine = h.instances[0].engine.as_ref().expect("脚本应加载成功");
        let seen: mlua::Table = engine.lua().globals().get("seen").expect("seen 已写");
        assert_eq!(seen.get::<String>(1).unwrap(), "Afalse");
        assert_eq!(seen.get::<String>(2).unwrap(), "LCtrl+ctrltrue");
    }

    #[test]
    fn mtime_change_hot_reloads_and_resets_state() {
        let body = "role = \"composite\"\nstate = \"v1\"\n";
        let (d, mut h) = host_with(&[("scripts/hot", body)], &["scripts/hot"]);
        // 改内容并显式拨 mtime（NTFS 精度 ~1s，测试跑得快会撞同秒，前提不成立）。
        let v2 = "role = \"composite\"\nstate = \"v2\"\n";
        std::fs::write(d.join("scripts/hot.lua"), v2).unwrap();
        {
            let f = std::fs::File::options()
                .write(true)
                .open(d.join("scripts/hot.lua"))
                .unwrap();
            f.set_modified(SystemTime::now() + std::time::Duration::from_secs(2))
                .unwrap();
        }
        h.ensure_fresh();
        let engine = h.instances[0].engine.as_ref().expect("v2 应加载成功");
        let state: String = engine.lua().globals().get("state").unwrap();
        assert_eq!(state, "v2", "热重载后应执行新代码");
    }

    #[test]
    fn broken_script_degrades_without_panicking() {
        let (_d, mut h) = host_with(
            &[("scripts/bad", "这不是合法 Lua ... syntax error（")],
            &["scripts/bad"],
        );
        assert_eq!(h.len(), 1, "实例在（引擎为 None）");
        // 所有回调都安全空转。
        h.on_sample(1.0, 1.0, 0.0, (1920.0, 1080.0));
        h.on_button("left", 0.0, 0.0, 0.0, (1920.0, 1080.0));
        h.on_key(
            "A",
            (false, false, false, false),
            false,
            0.0,
            (1920.0, 1080.0),
        );
        h.step(0.008, 0.0, (1920.0, 1080.0));
        let mut list = DrawList::new();
        h.draw(&mut list, 0.0, (1920.0, 1080.0));
        assert!(!h.is_animating());
        assert!(!h.wants_cursor_hidden());
    }
}
