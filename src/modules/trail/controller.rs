//! 鼠标特效控制器。对齐 trail/controller.py，但把"定时器"换成 M4 的帧驱动。

use crate::core::config::Config;
use crate::core::draw::DrawList;
use crate::core::events::MouseButton;
use crate::core::geometry::{Point, Rect};
use crate::core::math::Rng;
use crate::core::tick::Tick;
use crate::platform::display::ScreenRect;
use std::rc::Rc;

use super::clickfx::ScriptedClick;
use super::companion::CompanionHost;
use super::effects::lua_effect::LuaEffect;
use super::effects::{EffectCtx, TrailEffect, EFFECT_REGISTRY};
use super::hover_dot::HoverDot;
use super::ripples::{Ripple, RippleManager};
use super::sample::{MouseSample, SampleQueue};
use super::settings::{from_config, EffectId, TrailSettings};
use super::sound;

/// 固定步长。120Hz。
pub const FIXED_DT: f32 = 1.0 / 120.0;
/// 单帧最多补的步数。
pub const MAX_STEPS: u32 = 4;
/// 单帧最多补的**生成**批数。对齐原版 `min(int(elapsed / interval), 4)`。
///
/// 它同时是卡顿恢复期的限速阀：宁可少生成一点，也不让一次长帧把粒子数顶到池上限。
pub const MAX_SPAWN_BATCH: u32 = 4;
/// 同屏最多并存的脚本波纹实例数（与 `RippleManager::CAP` 同级，超了丢弃最旧的）。
pub const CLICKFX_CAP: usize = 64;
/// 快移插值间距（px）：相邻两采样点间距 ≥ 此值时，沿线段补中间点，消除快速移动时的
/// "断线"。取 4px：既高于抛带脚本的 `MIN_DIST`(2px) 去重阈值，让轨迹采样足够密（两
/// 转发点的间距会压到 ≈3~4px），又不至于为一条 1080p 对角线（≈2.2k px）补出失控点数。
pub const INTERPOLATE_SPACING_PX: f32 = 4.0;
/// 单段最多补的插值点数。极端快移（一次采样横跨上百 px）时的安全阀，防构造超长多边形。
pub const MAX_INTERP_PER_SEG: u32 = 32;

pub struct TrailModule {
    settings: TrailSettings,
    hover: HoverDot,
    /// ★ 复合脚本宿主（`role="composite"`，2026-09-26）：`scripts/` 里的用户复合脚本
    /// （拖尾 + 点击声音 + …一锅端）常驻实例。独立于 `trail_enabled`——它们是和
    /// 悬浮标/波纹并列的自成一体功能，各听脚本页自己的运行开关。
    companions: CompanionHost,
    ripples: RippleManager,
    /// 脚本波纹实例（每键选了自定义脚本时按下生成；与内置 `RippleManager` 并存）。
    click_fx: Vec<ScriptedClick>,
    /// ★ 常驻通道（2026-09-28「生命周期跟随脚本能力」）：被任一**已启用**的按键波纹
    /// 绑定选中、且实现了 `on_move` 的脚本（轨迹语义，如把辉光复制进 `ClickFx/`）
    /// 在这里获得常驻实例——按真实采样流喂 `on_move`、按下转发 `on_press`，不随
    /// 按下生灭、不受 `MAX_AGE_S` 约束。文件夹只决定 UI 归属，不影响底层行为。
    persistent_fx: Vec<ScriptedClick>,
    effect: Option<Box<dyn TrailEffect>>,
    effect_id: EffectId,
    samples: SampleQueue,
    rng: Rng,
    fixed_acc: f32,
    spawn_acc: f32,
    elapsed: f32,
    /// ★ 真实墙钟毫秒（Runtime 每帧注入）。**四类宿主共用同一份**——拖尾 / 悬浮标 /
    ///   点击效果 / 复合脚本的脚本全局 `wall_ms` 都从这里取，静止检测口径因此一致。
    wall_ms: f64,
    dirty: bool,
    /// 是否有随时间推进的动画（拖尾粒子 / 波纹）。与 `dirty` 的区别见 `tick()` 尾注。
    animating: bool,
    /// 用于判断设置是否变更
    last_config_version: u64,
    /// 虚拟桌面矩形（物理像素）。由 Runtime 每帧通过 `set_screen` 注入，替换原先
    /// 硬编码的 1920×1080（P3.1 坐标对齐）。特效 spawn / 悬浮标定位以此为基准。
    screen: ScreenRect,
    /// ★ 最近一次已知的指针位置。**四类宿主共用**：点击效果实例出生后每帧据此刷新
    /// `cursor_*`（此前点击宿主把光标钉死在按下点，跟随类脚本换个目录就不跟手）。
    pointer: (f32, f32),
    /// 上一次实际转发的光标位置（快移插值用，见 `tick` 采样转发处）。
    ///
    /// 只在轨迹类特效的**连续转发序列**内有意义：轨迹被打断（清屏 / 特效重建 / 切间隔
    /// 类）时必须清空，否则下一次转发会从一段陈旧的坐标开始补线。
    prev_forward: Option<(f32, f32)>,
}

impl TrailModule {
    pub fn new() -> Self {
        Self {
            settings: TrailSettings::default(),
            hover: HoverDot::new(),
            companions: CompanionHost::new(),
            ripples: RippleManager::new(),
            click_fx: Vec::new(),
            persistent_fx: Vec::new(),
            effect: None,
            effect_id: EffectId::None,
            samples: SampleQueue::new(),
            rng: Rng::new(42),
            fixed_acc: 0.0,
            spawn_acc: 0.0,
            elapsed: 0.0,
            wall_ms: 0.0,
            dirty: false,
            animating: false,
            last_config_version: 0,
            screen: ScreenRect::FALLBACK,
            pointer: (0.0, 0.0),
            prev_forward: None,
        }
    }

    /// 注入当前虚拟桌面矩形（由 Runtime 每帧调用）。替换硬编码 1920×1080。
    pub fn set_screen(&mut self, screen: ScreenRect) {
        self.screen = screen;
        self.hover.set_screen(screen.w as f32, screen.h as f32);
        // 常驻通道的脚本 screen 全局同步（跨屏/改分辨率后不落回旧值）。
        for fx in &mut self.persistent_fx {
            fx.set_screen(screen.w as f32, screen.h as f32);
        }
    }

    /// 当前虚拟桌面 (w, h)（f32；复合脚本回调的 screen 参数）。
    fn screen_wh(&self) -> (f32, f32) {
        (self.screen.w as f32, self.screen.h as f32)
    }

    /// 键盘按下入口（复合脚本分发；由 Runtime 的输入队列消费点调用）。
    ///
    /// ★ 与 [`Self::on_click`] 同性质：独立于 `trail_enabled` 与 `key_sound.enabled`
    /// ——复合脚本自成一体，键名与击键回显/击键音效同源（`KeyCode::display_name`）。
    pub fn push_key(
        &mut self,
        key: crate::core::events::KeyCode,
        mods: crate::core::events::Modifiers,
        repeated: bool,
    ) {
        self.companions.on_key(
            key.display_name(),
            (mods.ctrl, mods.alt, mods.shift, mods.win),
            repeated,
            self.elapsed,
            self.screen_wh(),
        );
    }

    /// 由 channel 回调 push 采样点。
    pub fn push_sample(&mut self, x: f32, y: f32, t_ms: u32) {
        self.samples.push(MouseSample { x, y, t_ms });
    }

    /// 重建/对账常驻通道（见字段注释）。
    ///
    /// 期望集合 = 所有**已启用**的按键波纹绑定里选中的脚本（去重）中实现了
    /// `on_move` 的那些。同 id 序列不变时只重放参数（保 Lua VM）；集合变化才重建。
    fn sync_persistent_fx(&mut self) {
        let dir = crate::core::paths::clickfx_dir();
        let screen = (self.screen.w as f32, self.screen.h as f32);
        // 期望集合（id, params），按键顺序遍历 + 去重。
        let mut wanted: Vec<(String, std::collections::HashMap<String, serde_json::Value>)> =
            Vec::new();
        for btn in [
            MouseButton::Left,
            MouseButton::Right,
            MouseButton::Middle,
            MouseButton::X1,
            MouseButton::X2,
        ] {
            let Some(rp) = self.settings.ripple_for_button(&btn).filter(|r| r.enabled) else {
                continue;
            };
            if rp.script.is_empty() || wanted.iter().any(|(id, _)| id == &rp.script) {
                continue;
            }
            wanted.push((rp.script.clone(), rp.script_params.clone()));
        }
        // 同 id 保 VM：只重放参数。
        if wanted.len() == self.persistent_fx.len()
            && wanted
                .iter()
                .zip(self.persistent_fx.iter())
                .all(|((id, _), fx)| id == fx.id())
        {
            for ((_, params), fx) in wanted.iter().zip(self.persistent_fx.iter_mut()) {
                fx.set_params(params.clone());
            }
            return;
        }
        self.persistent_fx.clear();
        for (script, params) in wanted {
            if !dir.join(format!("{script}.lua")).exists() {
                continue;
            }
            match ScriptedClick::open_persistent(&dir, &script, params, screen) {
                Ok(mut fx) => {
                    if !fx.wants_trail() {
                        // 只写 on_press 的脚本走"按下 spawn"老路，常驻通道不留它。
                        continue;
                    }
                    // 出生先把光标/出生点落到当前指针：脚本在第一次移动前也有合法的
                    // cursor_*（否则是 (0,0)）。
                    let (px, py) = self.pointer;
                    fx.on_sample(px, py);
                    self.persistent_fx.push(fx);
                }
                Err(e) => log::warn!("常驻通道加载点击脚本 {script} 失败: {e}"),
            }
        }
    }

    /// 鼠标按下：播点击音效 + 加点击效果。对齐 `trail/controller.py:208-228`。
    ///
    /// ★ 卡片式开关语义（2.0）：音效/波纹各听**自己卡片**的开关（`sound_*_enabled` /
    /// `ripple_*_enabled`，见 `sound::play_for_button` 的逐键
    /// 判断），不再受"拖尾效果"卡片的 `trail_enabled` 管辖——Python 单总开关时代的
    /// 首行早退已随「功能总开关」一起废除。
    pub fn on_click(&mut self, button: MouseButton, x: f32, y: f32) {
        log::info!("[diag] on_click {:?} @ {},{}", button, x, y);
        // 按下点即最新指针位置（点击实例据此起算 `cursor_*`，之后每帧随采样刷新）。
        self.pointer = (x, y);
        // ★ 复合脚本先于内置音效/波纹收到事件（2026-09-26）：脚本自己决定播不播声、
        //   画不画波纹——内置功能开着时两边并存（互不取代）。
        let btn = match button {
            MouseButton::Left => "left",
            MouseButton::Right => "right",
            MouseButton::Middle => "middle",
            MouseButton::X1 => "x1",
            MouseButton::X2 => "x2",
        };
        self.companions
            .on_button(btn, x, y, self.elapsed, self.screen_wh());
        // ★ 拖尾特效同样收按下事件（四类宿主同契约）：脚本写 on_press/on_click 即可在
        //   光标处爆一次。只实现 on_move/step/draw 的脚本在这里静默跳过，零影响。
        if let Some(e) = self.effect.as_mut() {
            e.on_press(x, y, self.elapsed);
        }
        sound::play_for_button(&self.settings, button);
        // ★ 每键波纹：各键独立的开关/尺寸/时长/颜色（ripple_bindings）。
        if let Some(rp) = self
            .settings
            .ripple_for_button(&button)
            .filter(|r| r.enabled)
        {
            // 选中自定义脚本且文件存在 → 生成脚本实例（忽略内置样式）；脚本缺失 /
            // 加载失败则回退内置样式。
            let mut used_script = false;
            if !rp.script.is_empty() {
                // ★ 常驻通道优先（生命周期跟随脚本能力）：脚本实现了 `on_move`
                //   （轨迹语义）→ 按下事件转给常驻实例，**不再 spawn 短命实例**
                //   （否则同一次按下被画两遍）。脚本的点击响应由它自己的 `on_press`
                //   回调实现，语义不丢。
                if let Some(fx) = self.persistent_fx.iter_mut().find(|f| f.id() == rp.script) {
                    fx.on_press(x, y);
                    used_script = true;
                }
            }
            if !used_script && !rp.script.is_empty() {
                let dir = crate::core::paths::clickfx_dir();
                if dir.join(format!("{}.lua", rp.script)).exists() {
                    match ScriptedClick::new(
                        &dir,
                        &rp.script,
                        x,
                        y,
                        rp.script_params.clone(),
                        (self.screen.w as f32, self.screen.h as f32),
                    ) {
                        Ok(fx) => {
                            if self.click_fx.len() >= CLICKFX_CAP {
                                self.click_fx.remove(0);
                            }
                            self.click_fx.push(fx);
                            used_script = true;
                        }
                        Err(e) => {
                            log::warn!("加载点击效果脚本 {} 失败，回退内置样式: {e}", rp.script)
                        }
                    }
                }
            }
            if !used_script {
                self.ripples.add(Ripple::new(
                    Point::new(x, y),
                    rp.color,
                    rp.style,
                    rp.size,
                    rp.duration_ms,
                ));
            }
            log::info!(
                "[diag] ripple/clickfx added, ripples={} clickfx={}",
                self.ripples.active(),
                self.click_fx.len()
            );
            // 一出生就该被看见：本帧标脏，让 OverlayCanvas 续帧到消亡为止。
            self.dirty = true;
            self.animating = true;
        }
    }

    /// 滚轮：更新指针位置并转发给复合脚本 / 拖尾脚本（缺 `on_wheel` 的静默跳过）。
    ///
    /// 由 `Runtime` 的输入队列消费点调用；钩子只在有脚本声明 `on_wheel` 时转发普通滚轮
    /// （见 `platform::input::mouse::set_script_wheel_armed`）。
    pub fn on_wheel(&mut self, delta: i32, horizontal: bool, x: f32, y: f32) {
        // 低频事件（人手 ≤5 次/秒），留一条诊断：排查"滚轮特效用不了"时先看有没有这行。
        log::info!(
            "[diag] on_wheel delta={} horizontal={} want={}",
            delta,
            horizontal,
            self.any_wheel_wanted()
        );
        self.pointer = (x, y);
        self.companions
            .on_wheel(delta, horizontal, x, y, self.elapsed, self.screen_wh());
        if let Some(e) = self.effect.as_mut() {
            e.on_wheel(delta, horizontal, x, y, self.elapsed);
        }
        // 滚轮类效果下一帧要出画；标脏让覆盖层按需续帧。
        self.dirty = true;
        self.animating = true;
    }

    /// 是否有脚本声明了 `on_wheel`（钩子武装判据，供 `Runtime::sync_input_gate` 调用）。
    pub fn any_wheel_wanted(&self) -> bool {
        self.companions.any_wheel_wanted() || self.effect.as_ref().is_some_and(|e| e.wants_wheel())
    }

    /// 鼠标抬起：清按住态，并转发给四类脚本宿主。
    pub fn on_mouse_up(&mut self, button: MouseButton, x: f32, y: f32) {
        self.pointer = (x, y);
        let btn = match button {
            MouseButton::Left => "left",
            MouseButton::Right => "right",
            MouseButton::Middle => "middle",
            MouseButton::X1 => "x1",
            MouseButton::X2 => "x2",
        };
        self.companions
            .on_mouse_up(btn, x, y, self.elapsed, self.screen_wh());
        if let Some(e) = self.effect.as_mut() {
            e.on_mouse_up(button, x, y, self.elapsed);
        }
        for fx in &mut self.click_fx {
            fx.on_mouse_up(button, x, y);
        }
        for fx in &mut self.persistent_fx {
            fx.on_mouse_up(button, x, y);
        }
    }

    /// 同步配置。config_version 变化时重建特效。
    fn sync_settings(&mut self, config: &Config, config_version: u64) {
        if config_version == self.last_config_version {
            return;
        }
        self.last_config_version = config_version;
        self.settings = from_config(&config.trail);

        // 重建特效
        let new_id = self.settings.trail_effect.clone();
        if new_id != self.effect_id {
            self.effect_id = new_id.clone();
            // 特效变了 = 一条新的干净轨迹：清掉上次的插值锚点，免得补线从旧特效的
            // 坐标算起。
            self.prev_forward = None;
            self.effect = if matches!(new_id, EffectId::None) {
                None
            } else {
                // 先查静态注册表
                let entry = EFFECT_REGISTRY.iter().find(|e| e.id == new_id.as_str());
                if let Some(e) = entry {
                    Some((e.make)())
                } else {
                    // 尝试作为 Lua 脚本加载
                    self.load_lua_effect(new_id.as_str())
                }
            };
        }
        // ★ 参数重放**每次配置版本变化都要执行**，不能只挂在"特效 id 变化"分支里——
        // 用户在面板拖滑块改的是参数、id 不变，若只在换特效时 apply 一次，
        // 参数就永远停在特效创建那刻（"改了没反应"）。
        // 间隔类的 density / spawn_interval_ms 每帧直读 settings 不受影响，
        // 但 Lua 脚本参数（透明度/大小/颜色等）必须经 apply() 重放才进得到脚本。
        if let Some(ref mut eff) = self.effect {
            let map = self.settings.effect_param_map(self.effect_id.as_str());
            eff.apply(&map);
        }

        // 更新悬浮标：卡片开关 + 启用实例列表 + 每实例参数 + 图片媒体（PNG 静态 / GIF 动画）。
        // ★ 多开（2026-09-26）：`hover_active` 里每个脚本一个实例，全部跟随鼠标同时显示；
        //   sync_instances 按 id 对账（同 id 保引擎不重载），参数按 `hover_params_for`
        //   （读取侧兼容老裸 stem 键）逐实例重放。
        // ★ 只在配置版本变化时走到（本函数开头挡着）：用户在面板上每改一次参数就来
        //   一条（连拖滑块会是连续若干条），覆盖层休眠期零条。排查"面板改了参数
        //   脚本没收到"全靠这一条：它有 = 模块层收到了；它没有 = 面板层/版本号断了。
        self.hover.enabled = self.settings.hover_enabled;
        let active = self.settings.hover_active_list();
        self.hover.sync_instances(&active);
        for id in &active {
            let params = self.settings.hover_params_for(id);
            self.hover.set_params_of(id, params);
        }
        log::info!(
            "[悬浮标] 应用启用列表 enabled={} active={:?}",
            self.settings.hover_enabled,
            active
        );

        // ★ 复合脚本（2026-09-26）：运行列表 + 每脚本参数对账。id = 相对 lua 根的
        //   路径 stem（`scripts/xxx`）；sync 同 id 保 VM 不重载，参数按 id 重放。
        //   热重载（文件改动）不在这里——tick 里每帧 ensure_fresh（stat 判据）。
        let comp_list: Vec<(String, std::collections::HashMap<String, serde_json::Value>)> = config
            .lua_ext
            .composite_active
            .iter()
            .map(|id| {
                (
                    id.clone(),
                    config.lua_ext.params.get(id).cloned().unwrap_or_default(),
                )
            })
            .collect();
        self.companions.sync(&comp_list);
        log::info!("[复合脚本] 应用运行列表 active={:?}", self.companions.ids());

        // ★ 常驻通道对账（2026-09-28）：波纹绑定里被选中且实现了 `on_move` 的脚本
        //   （轨迹语义）在此获得常驻实例；同 id 保 VM 不重载，参数逐次重放。
        self.sync_persistent_fx();

        // 「图片」实例（stem == png）：配置只存一个文件名（hover_image），各 png 实例共用。
        // .gif → 逐帧解码成动画；其余（PNG）→ 静态解码（**保持原始宽高**，不再裁正方形）。
        // 无图 → 清空，避免上一轮残留的旧图继续被画。
        let (img, anim) = if !self.settings.hover_image.is_empty() {
            let path = crate::core::paths::hover_image_path(&self.settings.hover_image);
            let is_gif = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("gif"));
            if is_gif {
                match crate::core::gif::load_gif_frames(&path) {
                    Some(a) => (None, Some(a)),
                    None => {
                        log::warn!("悬浮标动图加载失败：{}", path.display());
                        (None, None)
                    }
                }
            } else {
                (crate::core::image::load_png(&path).map(Rc::new), None)
            }
        } else {
            (None, None)
        };
        // 图片（`hover_image`）喂给**所有**悬浮标实例——不只 `png`：任何脚本都可以用
        // `ctx:image(...)` 贴图（如 `springy_img` 弹簧挂件）。空配置 → 清空，避免残留旧图。
        for inst in self.hover.instances.iter_mut() {
            inst.set_anim(anim.clone());
            inst.set_image(img.clone());
        }
    }

    /// 尝试加载 Lua 脚本效果。
    fn load_lua_effect(&self, script_id: &str) -> Option<Box<dyn TrailEffect>> {
        // 脚本目录按 exe 同级解析（见 `core::paths`），不按 CWD——否则从资源管理器/任务栏
        // 启动时目录不同，用户脚本就"消失"了。
        let scripts_dir = crate::core::paths::mousetrail_dir();
        let scripts_dir = scripts_dir.as_path();
        if scripts_dir.join(format!("{}.lua", script_id)).exists() {
            match LuaEffect::new(scripts_dir, script_id) {
                Ok(eff) => Some(Box::new(eff)),
                Err(e) => {
                    log::warn!("加载 Lua 效果 {} 失败: {}", script_id, e);
                    None
                }
            }
        } else {
            None
        }
    }

    /// 生成间隔判断：哪些特效按 interval（间隔类只取最新点生成），哪些每次都转发（轨迹类）。
    ///
    /// 用 id 字符串匹配，这样 `EffectId::Lua("pixie_dust")`（配置里写裸脚本名）也能命中。
    fn is_interval_effect(id: &EffectId) -> bool {
        matches!(id.as_str(), "pixie_dust" | "circle_particles")
    }

    /// 快移插值：沿 `from → to` 线段，以 ≈`spacing` 的等距返回应插入的中间采样点
    /// **（不含两端**——两端的真实点由调用方自己转发）。间距 < `spacing` 时返回空。
    ///
    /// 纯函数、确定性，便于单测：把插值补点的几何从 `tick` 的采集/转发里剥离出来。
    /// `max_points` 是极端快移（一次采样横跨上百 px）时的安全阀。
    /// `pub(crate)`：复合脚本宿主（`companion::CompanionHost::on_sample`）复用同一
    /// 份插值——复合拖尾与内置轨迹特效采样密度必须一致，两份插值必出分叉。
    pub(crate) fn interpolate(
        from: (f32, f32),
        to: (f32, f32),
        spacing: f32,
        max_points: u32,
    ) -> Vec<(f32, f32)> {
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        let dist = (dx * dx + dy * dy).sqrt();
        if dist < spacing {
            return Vec::new();
        }
        // 整段能塞下多少个 `spacing` 段，扣掉端点的进位归并数。
        let steps = ((dist / spacing).floor() as u32).min(max_points);
        if steps == 0 {
            return Vec::new();
        }
        // 在 [0,1] 里均匀取 steps 个内点，保证相邻转发点（含两个真实端点）间距 ≤ spacing。
        let step_t = 1.0 / (steps + 1) as f32;
        (1..=(steps as usize))
            .map(|k| {
                let t = k as f32 * step_t;
                (from.0 + dx * t, from.1 + dy * t)
            })
            .collect()
    }

    pub fn tick(&mut self, tick: &Tick, config: &Config, config_version: u64) {
        let dt_ms = tick.dt_ms;
        let dt_s = dt_ms / 1000.0;

        // 虚拟桌面矩形（物理像素）→ 特效 spawn 基准，替换硬编码 1920×1080（P3.1）。
        let screen_rect = Rect::new(
            self.screen.x as f32,
            self.screen.y as f32,
            self.screen.w as f32,
            self.screen.h as f32,
        );

        self.sync_settings(config, config_version);
        // 帧推进时间戳（**秒**，共享时钟）：GIF 动画的帧推进 dt 来源（与 trail_enabled
        // 无关）。★ 此前喂的是 `tick.elapsed_ms`（毫秒）——悬浮标与另外三类宿主差 1000
        //   倍，同一个脚本换个目录速度就不一样。悬浮标的时钟在固定步进循环之后对齐。

        let trail_enabled = self.settings.trail_enabled;

        // ★★★ 共享时钟 + 特效固定步长（2026-09-26 重排）：**无条件推进**——原实现
        //   把这段放在 `!trail_enabled` 提前 return 之后，拖尾卡片关掉时 elapsed 冻结，
        //   复合脚本的点年龄永不增长（step 永不剔除、draw 恒 k=1）→「拖尾一直在屏上
        //   不消失」。现在时钟先走，特效步进只听 trail_enabled。
        self.fixed_acc += dt_s;
        let mut steps = 0u32;
        while self.fixed_acc >= FIXED_DT && steps < MAX_STEPS {
            if trail_enabled {
                if let Some(ref mut eff) = self.effect {
                    let ctx = &mut EffectCtx {
                        rng: &mut self.rng,
                        screen: screen_rect,
                        elapsed: self.elapsed,
                        spawn_batch: 0,
                        wall_ms: self.wall_ms,
                    };
                    eff.step(FIXED_DT, ctx);
                }
            }
            self.elapsed += FIXED_DT;
            self.fixed_acc -= FIXED_DT;
            steps += 1;
        }
        if steps == MAX_STEPS {
            self.fixed_acc = 0.0;
        }
        // ★ 悬浮标对齐**共享时钟**（秒）：拖尾 / 悬浮标 / 点击效果 / 复合脚本四类的
        //   `elapsed` 逐位同源，帧间 dt 因此也同源。
        self.hover.set_time(self.elapsed);

        // 波纹无条件 tick（即使 trail_enabled=false，否则重开会看到冻结的旧波纹）
        self.ripples.tick(dt_ms);
        // 脚本波纹同样无条件 tick + 剔除（死亡/超龄的实例释放 Lua VM，避免泄漏）。
        // ★ dt 传**秒**（契约统一后与拖尾特效、独立卡片一致）。此前传的是毫秒——
        //   同一个脚本换个宿主速度差 1000 倍。
        for fx in &mut self.click_fx {
            // ★ 静默刷新指针 + 墙钟：与拖尾/悬浮标同一套注入（四类宿主一致）。
            fx.set_cursor(self.pointer.0, self.pointer.1);
            fx.set_wall_ms(self.wall_ms);
            fx.tick(dt_s);
        }
        self.click_fx
            .retain(|fx| fx.is_active() && fx.elapsed() < ScriptedClick::MAX_AGE_S);

        // 常驻通道：轨迹语义脚本，**无条件推进**（与波纹同待遇——trail_enabled=false
        // 也不冻结）。`on_move` 由下面的采样流喂，这里只走时间。
        for fx in &mut self.persistent_fx {
            fx.set_wall_ms(self.wall_ms);
            fx.tick(dt_s);
        }

        // ★ 复合脚本（独立于 trail_enabled，2026-09-26）：热重载检查 + 采样转发 +
        //   步进。采样在这里一次性取走——下面两个分支（拖尾开/关）都消费同一批点，
        //   复合脚本与拖尾特效吃同一份采样流，互不抢占。
        let wh = self.screen_wh();
        if self.companions.has_scripts() {
            self.companions.ensure_fresh();
        }
        let samples: Vec<MouseSample> = self.samples.drain().collect();
        if self.companions.has_scripts() {
            for s in &samples {
                self.companions.on_sample(s.x, s.y, self.elapsed, wh);
            }
            self.companions.step(dt_s, self.elapsed, wh);
        }
        // ★ 常驻通道吃同一份采样流（每个真实采样发一次 on_move）：轨迹类脚本
        //   （累积池）据此成形。与 companions 同位——两个分支（拖尾开/关）都生效，
        //   不受 trail_enabled 门控。
        if !self.persistent_fx.is_empty() {
            for s in &samples {
                for fx in &mut self.persistent_fx {
                    fx.on_sample(s.x, s.y);
                }
            }
        }

        if !trail_enabled {
            // ★ 卡片式开关语义（2.0）：`trail_enabled` 只门控"拖尾效果"卡片；悬浮标 /
            // 波纹 / 声音各由自己卡片的开关驱动（见 draw / on_click）。特效关掉时停推进
            // 并清屏，但**采样点仍要消费、悬浮标位置仍要更新**——否则只开悬浮标时它
            // 钉在原地不动，采样队列还会无限堆积。dirty/animating 按波纹与悬浮标的
            // 实际状态算，保证"只开悬浮标/波纹"也能绘制与续帧。
            if let Some(e) = self.effect.as_mut() {
                e.clear()
            }
            // 停用时清掉插值锚点：否则重新启用后第一次采样会从**陈旧的**旧坐标开始补线，
            // 凭空拖出一条到鼠标当前位置的伪轨迹。
            self.prev_forward = None;
            let newest = samples.last().copied();
            if let Some(sample) = newest {
                self.hover.pos = Point::new(sample.x, sample.y);
            }
            self.animating = self.ripples.active() > 0
                || self.hover.has_animation()
                || !self.click_fx.is_empty()
                || !self.persistent_fx.is_empty()
                || self.companions.is_animating();
            self.dirty = self.animating || self.hover.enabled || self.companions.has_scripts();
            return;
        }

        // （共享时钟已在 tick 开头无条件推进——特效步进也在那里按 trail_enabled 完成，
        //   这里不再重复推进 elapsed。）

        // 采样点分发。
        //
        // ★ 两类特效对"每个点"的需求不一样，别混着处理：
        //   · 轨迹类（飘带等）按鼠标路径成形，**每个点都得转发**；
        //   · 间隔类（像素尘埃、粒子光环）只需要一个**位置**——原版也是用"间隔到点那一
        //     刻"的坐标生成粒子。下面只保留最新点。
        let is_interval = Self::is_interval_effect(&self.effect_id);
        let effect_id_str = self.effect_id.as_str();
        let interval_s = (self
            .settings
            .param_f64(effect_id_str, "spawn_interval_ms", 100.0) as f32
            / 1000.0)
            // 0 会让下面的除法出 inf/NaN（原版 Python 在这里直接 ZeroDivisionError）。
            .max(1e-3);

        // 间隔类只消费"最新点"、不透传到轨迹上——插值锚点对它没有意义，每帧主动清掉，
        // 免得切回轨迹类时从那一段陈旧的坐标开始补线。
        if is_interval {
            self.prev_forward = None;
        }

        let mut newest: Option<MouseSample> = None;
        for sample in samples.iter().copied() {
            newest = Some(sample);
            if !is_interval {
                if let Some(ref mut eff) = self.effect {
                    // ★ 快移断线修复：样本来自 WH_MOUSE_LL（节流 250Hz），两帧间的采样点
                    // 在快移时能相距上百 px。抛带这类"按点成形"的轨迹特效只把 on_move 点
                    // 串起来，点太稀 → 明显的折角空隙；点不足（抛带要 ≥3 点）→ 整段缺失
                    // 看着像"线断了"。这里沿上一转发点→当前点按 `INTERPOLATE_SPACING_PX`
                    // 补中间点（`Self::interpolate`），把轨迹采样密度拉平。插值点只喂给
                    // 特效：
                    //   · 不进 `newest`（那是下面写 `hover.pos` 的真实点）→ 悬浮标不会
                    //     追着假点跑；
                    //   · 不碰 spawn_acc（间隔类的生成节拍与它无关）。
                    if let Some((px, py)) = self.prev_forward {
                        for (ix, iy) in Self::interpolate(
                            (px, py),
                            (sample.x, sample.y),
                            INTERPOLATE_SPACING_PX,
                            MAX_INTERP_PER_SEG,
                        ) {
                            let interp = MouseSample {
                                x: ix,
                                y: iy,
                                t_ms: sample.t_ms,
                            };
                            let ctx = &mut EffectCtx {
                                rng: &mut self.rng,
                                screen: screen_rect,
                                elapsed: self.elapsed,
                                spawn_batch: 1,
                                wall_ms: self.wall_ms,
                            };
                            eff.on_move(interp, ctx);
                        }
                    }
                    let ctx = &mut EffectCtx {
                        rng: &mut self.rng,
                        screen: screen_rect,
                        elapsed: self.elapsed,
                        spawn_batch: 1,
                        wall_ms: self.wall_ms,
                    };
                    eff.on_move(sample, ctx);
                }
            }
            // 无论轨迹类转发与否，"上一个实际位置"都被这个真实采样点刷新：下一轮补线
            // 始终从最新坐标起算（间隔类每帧已在前面把锚点清成 None）。
            self.prev_forward = Some((sample.x, sample.y));
        }
        if let Some(sample) = newest {
            self.hover.pos = Point::new(sample.x, sample.y);
            self.pointer = (sample.x, sample.y);
        }

        // 间隔类生成。
        //
        // ★ `spawn_acc` **每帧只推进一次真实帧时长**，与这一帧收了多少个采样点无关。
        // 曾经写成"在采样点循环里每点 `spawn_acc += dt_s`"：一帧攒下 K 个采样点就等于把
        // 帧时长推进了 K 次，生成速率被乘以 K（125Hz≈2、1000Hz≈16），粒子直接顶到池上限
        // 8192、每帧 2.4 万条绘制指令 → 严重卡顿；且"帧越慢→攒点越多→生成越多→帧更慢"
        // 是正反馈。判据见 `spawn_rate_is_independent_of_sample_rate`。
        if is_interval {
            if let (Some(sample), Some(ref mut eff)) = (newest, self.effect.as_mut()) {
                let density = self.settings.param_u32(effect_id_str, "density", 5);
                self.spawn_acc += dt_s;
                if self.spawn_acc >= interval_s {
                    // 该兑现多少批。`as u32` 遇到极端值会饱和到 u32::MAX，
                    // 下面按"超过上限"分支处理，不会溢出。
                    let due = (self.spawn_acc / interval_s) as u32;
                    let batches = due.min(MAX_SPAWN_BATCH);
                    for _ in 0..batches {
                        let ctx = &mut EffectCtx {
                            rng: &mut self.rng,
                            screen: screen_rect,
                            elapsed: self.elapsed,
                            spawn_batch: density,
                            wall_ms: self.wall_ms,
                        };
                        eff.on_move(sample, ctx);
                    }
                    self.spawn_acc = if due > MAX_SPAWN_BATCH {
                        // 卡顿（或帧率极低）攒下的一大段：直接丢弃，不折算成之后的超速生成。
                        // 对应原版"生成完把基准重置为 now"，也是卡顿恢复期的限速阀。
                        0.0
                    } else {
                        // 正常情况只扣掉已兑现的部分，零头留到下一帧。若每轮都清零，
                        // 60fps 下每轮会白丢最多一帧的时长，实际生成率掉到原版的约 0.6 倍
                        // ——粒子肉眼可见地变稀。
                        self.spawn_acc - interval_s * due as f32
                    };
                }
            }
        }

        // 判断 dirty
        let effect_active = self.effect.as_ref().is_some_and(|e| e.is_active());
        let ripples_active = self.ripples.active() > 0;
        let hover_active = self.hover.enabled;
        let hover_animating = self.hover.has_animation();
        // ★ `dirty` 与 `animating` 的分工（P3.5 实测修）：
        // - `dirty` = "屏上有东西"（含静态悬浮标），用于状态展示；
        // - `animating` = "画面**随时间在变**"，是帧驱动的唯一依据。
        // 悬浮标只要开着就每帧都产出指令，但它静止时**不随时间变化**——把它算进续帧条件
        // 会让空闲时永久续帧（实测空闲 CPU 7.6%，目标是 0）。鼠标一动就有输入事件驱动
        // 重绘，静止时该停就停（分层窗帧间不清缓冲，最后一帧的悬浮标会留在屏幕上）。
        self.animating = effect_active
            || ripples_active
            || hover_animating
            || !self.click_fx.is_empty()
            || !self.persistent_fx.is_empty()
            || self.companions.is_animating();
        self.dirty = self.animating || hover_active || self.companions.has_scripts();
    }

    /// 是否有**正在推进**的动画（不含静态悬浮标）。`OverlayCanvas` 据此决定续不续帧。
    pub fn is_animating(&self) -> bool {
        self.animating
    }

    /// 注入真实墙钟毫秒（透传给**四类宿主**脚本的 `wall_ms` 全局）。
    ///
    /// ★ 与 `tick.elapsed_ms` 分工明确：后者无帧期间不推进，脚本拿它测"静止了多久"
    ///   会恒等于 0（GIF / 逐帧动画另走共享时钟）。
    /// ★ 存一份在本模块：拖尾特效经 `EffectCtx::wall_ms` 取用——此前只有悬浮标注入，
    ///   同一个脚本换个宿主，静止检测就不一样（四类待遇不一致的其中一块）。
    pub fn set_wall_ms(&mut self, ms: f64) {
        self.wall_ms = ms;
        self.hover.set_wall_ms(ms);
        self.companions.set_wall_ms(ms);
    }

    /// 悬浮标脚本请求的续帧间隔（毫秒）：None = 不需要额外帧。
    ///
    /// ★ 只认**非 GIF** 的脚本请求：GIF 走 `is_animating()` 那条 `Now` 通路，两者不叠加。
    /// 「晃动定位」的定位环淡出就靠这条——否则手一停，环就定格在屏上（分层窗帧间不清缓冲）。
    pub fn hover_next_frame_ms(&self) -> Option<u32> {
        if self.hover.has_animation() {
            return None;
        }
        self.hover.next_frame_ms()
    }

    /// 悬浮标本体（**仅测试用**）：直接摆它的续帧请求，让帧门判据有个不经过脚本的入口。
    #[cfg(test)]
    pub(crate) fn hover_mut(&mut self) -> &mut HoverDot {
        &mut self.hover
    }

    /// 复合脚本宿主只读访问（**仅测试用**）。
    #[cfg(test)]
    pub(crate) fn companions(&self) -> &CompanionHost {
        &self.companions
    }

    /// 是否有任一**已加载**的复合脚本真的要吃覆盖层的帧。
    ///
    /// 与 `config.lua_ext.composite_running()` 的区别：后者只看"启用列表非空"，把只开
    /// 自己原生窗的一次性脚本（AI 对话）也算进来 —— 于是覆盖层为它整屏缓冲常驻 +
    /// 60fps 整屏重绘，而它一个像素都不画。这里问的是实例本身（见
    /// `companion::CompanionHost::any_needs_frame` 的保守分支）。
    pub fn composite_needs_frame(&self) -> bool {
        self.companions.any_needs_frame()
    }

    /// 配置版本变了就把模块（含复合脚本实例列表）对账一次。
    ///
    /// ★ 存在的理由：`sync_settings` 平时挂在 `tick()` 里，而 `tick()` 只从 `paint()`
    ///   进来 —— **覆盖层休眠时它一次都不跑**。那样会出现死锁：新勾上一个复合脚本 →
    ///   帧门需要先看见"它要用帧"才肯醒 → 而"看见"这件事要等 tick → tick 要等醒。
    ///   休眠态下由 250ms 对账（与 UI 通道）调本函数打破这个环。
    pub fn sync_if_stale(&mut self, config: &Config, config_version: u64) {
        if config_version == self.last_config_version {
            return;
        }
        self.sync_settings(config, config_version);
    }

    // `&mut self`：Lua 特效的 draw 需要写 active 标记（决定续帧），故整个控制器要可变借用。
    pub fn draw(&mut self, list: &mut DrawList) {
        if self.ripples.active() > 0 {
            log::info!(
                "[diag] draw: ripples active={} trail_on={} cmds_before={}",
                self.ripples.active(),
                self.settings.trail_enabled,
                list.cmds().len()
            );
        }
        // ★ 卡片式开关语义（2.0）：三样东西各听各的卡片开关，不再共用 trail_enabled
        // 总门（overlay.py:58-69 的单总开关早退已随「功能总开关」一起废除）。

        // 悬浮标（聚光灯暗罩）**必须先画**（垫底）。
        //
        // ★ 顺序不变量：聚光灯这类用 `rect(压暗) + clear_circle(挖洞)` 实现的悬浮标用了
        // 破坏性 Clear 混合——Clear 会把"当前缓冲里已画的一切（含波纹/拖尾）"清成透明。
        // 若画在特效之后，洞内特效会被整片擦掉 → 点击效果/拖尾在聚光灯里"失效"。
        // 把它提到特效**之前**画：挖洞时底下还没有特效，清的只是暗罩自己的像素；随后
        // 波纹/拖尾叠在暗罩之上 → 洞内清晰可见、洞外不计暗罩（特效始终全亮、不失效）。
        // 代价为零（暗罩仍是 rect 快路 + 小圆 Clear，0.6ms 不变，不引入任何额外合成层）。
        let _ = self.hover.draw(list);

        // 复合脚本：紧跟悬浮标绘制（脚本 `ctx:layer("base"/"top")` 可自行归层；
        // ⚠ 聚光灯类暗罩的 `clear_circle` 是破坏性擦除，会清掉缓冲里已画的内容，
        // 想做暗罩的复合脚本必须自己 `ctx:layer("base")` 垫底）。
        self.companions.draw(list, self.elapsed, self.screen_wh());

        // 波纹：波纹对象只会在自己卡片开关开启时被 on_click 添加，有则必画。
        self.ripples.draw(list);
        // 脚本波纹：与内置波纹并存，各自独立绘制。
        for fx in &mut self.click_fx {
            fx.draw(list);
        }
        // 常驻通道（轨迹语义脚本，如把辉光复制进点击效果目录）：与波纹并存绘制。
        for fx in &mut self.persistent_fx {
            fx.draw(list);
        }

        // 拖尾特效：只听"拖尾效果"卡片的开关。
        if self.settings.trail_enabled {
            if let Some(ref mut eff) = self.effect {
                eff.draw(list);
            }
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// 用本帧真实光标位置覆盖悬浮标中心（消除"画在事件过期坐标上"的滞后）。
    /// 只在悬浮标启用时写（`hover.pos` 仅悬浮标绘制使用）。
    pub fn set_live_cursor(&mut self, x: f32, y: f32) {
        if self.hover.enabled {
            self.hover.pos = Point::new(x, y);
        }
    }

    pub fn clear(&mut self) {
        if let Some(e) = self.effect.as_mut() {
            e.clear()
        }
        self.ripples.clear();
        self.click_fx.clear();
        self.samples.clear();
        self.dirty = false;
        self.animating = false;
        self.prev_forward = None;
    }

    /// 拖尾是否启用（评估口径的唯一出处，供 `Runtime` 做开关边沿检测）。
    ///
    /// 拖尾没有实现 [`crate::modules::Module`] trait（它有波纹/悬浮标等多路子开关，
    /// 走的是自己的治理方式），故单独给这个纯函数——**判据只写一处**，避免 Runtime
    /// 里的边沿检测与实际的启用语义随时间走偏。
    pub fn enabled_in(config: &crate::core::config::Config) -> bool {
        config.trail.trail_enabled
    }

    /// 本模块当前是否有"需要隐藏系统指针"的特效（激光笔类）。
    ///
    /// 覆盖两条来源：① 悬浮标类型脚本声明 `hide_cursor`（激光笔的出厂归宿）；
    /// ② 拖尾 Lua 效果脚本声明 `hide_cursor`（用户把激光笔放进 `MouseTrail/` 目录时）。
    /// 两条任一命中且处于启用态即为真，runtime 据此换透明光标。
    pub fn wants_cursor_hidden(&self) -> bool {
        if self.hover.wants_cursor_hidden() || self.companions.wants_cursor_hidden() {
            return true;
        }
        self.settings.trail_enabled && self.effect.as_ref().is_some_and(|e| e.hide_cursor())
    }

    /// ★ 内存回收：拖尾被关掉时把它独占的资源还回去。
    ///
    /// [`clear`](Self::clear) 只把长度归零、**容量留下**（"马上还要用"）；这里是"功能
    /// 已关"，要的是把容量也退掉：
    ///
    /// | 资源 | 为什么值得退 |
    /// |---|---|
    /// | 悬浮标的**用户图片 / GIF 帧**（`hover.image` / `hover.anim`） | 来自用户磁盘的 PNG/GIF，大小不受我们控制；关掉拖尾后不会再加载回来 |
    /// | `click_fx`（脚本波纹实例） | 连点会攒一堆实例，每个都带自己的 Lua VM 状态 |
    /// | 特效内部粒子池（`effect.clear`） | 已在 `clear` 里清过，这里只是确认调用顺序 |
    ///
    /// `samples` 是**定长环形缓冲**（`[MouseSample; 1024]`），不存在"退容量"一说，清了即可。
    pub fn release_memory(&mut self) {
        self.clear();
        self.click_fx.shrink_to_fit();
        // ★ 常驻通道（persistent_fx）**不清**：它由配置对账管理（同 companions——
        //   也持 Lua VM，同样不在这里清）。清了的话重开边沿 config_version 不变、
        //   sync_settings 不跑，常驻实例就永远回不来了。
        // 图片/GIF 媒体现在在**实例**上（多开改造）：逐实例退掉。
        for inst in &mut self.hover.instances {
            inst.set_image(None);
            inst.set_anim(None);
        }
    }
}

impl Default for TrailModule {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tick16() -> Tick {
        Tick {
            raw_ms: 16.0,
            smooth_ms: 16.0,
            dt_ms: 16.0,
            elapsed_ms: 16.0,
        }
    }

    /// 造一个"总开关开、左键效果开、左键声音关"的配置。声音关是为了测试不真的发声。
    fn clickable_config() -> Config {
        let mut cfg = Config::default();
        cfg.trail.trail_enabled = true;
        // 2026-09-25 晚：出厂波纹总开关改关（所有功能默认全开→全关），测试要出波纹就得显式打开。
        cfg.trail.ripple_enabled = true;
        if let Some(l) = cfg.trail.ripple_bindings.get_mut("left") {
            l.enabled = true;
            l.size = 120;
            l.duration = 400;
        }
        cfg
    }

    #[test]
    fn fixed_dt_constant() {
        assert!((FIXED_DT - 1.0 / 120.0).abs() < 0.0001);
    }

    /// ★ 复合脚本端到端（2026-09-26）：复合脚本**拖尾总开关关闭时**也吃采样、也绘制。
    /// 复合脚本是与悬浮标并列的独立功能，不归「拖尾效果」卡片管。
    ///
    /// 出厂演示脚本 `3.composite_demo` 已按用户口径删除（2026-10-29），这里改跑
    /// `tests/fixtures/lua/scripts/composite_demo_fixture.lua`——脚本来源变了，
    /// "常驻实例 + 采样 + 绘制 + 点击环"的链路仍是真实宿主（`CompanionHost::sync_at`
    /// 与生产 `sync` 只差脚本根目录这一个入参）。
    #[test]
    fn composite_script_runs_even_when_trail_disabled() {
        let mut cfg = Config::default();
        cfg.trail.trail_enabled = false;
        cfg.trail.hover_enabled = false;

        let mut m = TrailModule::new();
        m.tick(&tick16(), &cfg, 1);
        let fixture =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/assets/lua");
        m.companions.sync_at(
            &fixture,
            &[(
                "scripts/composite_demo_fixture".to_string(),
                std::collections::HashMap::new(),
            )],
        );
        assert!(m.companions().has_scripts(), "复合脚本应已载入测试夹具");

        // 鼠标动起来 → on_move 记点 → dirty；绘制产出指令（demo 画拖尾圆点）。
        m.push_sample(100.0, 100.0, 1000);
        m.push_sample(160.0, 100.0, 1016);
        m.tick(&tick16(), &cfg, 1);
        assert!(m.is_dirty(), "有复合脚本运行 → 屏上有内容");

        let mut list = DrawList::new();
        m.draw(&mut list);
        assert!(
            !list.cmds().is_empty(),
            "复合脚本应画出拖尾点（cmd 数 {}）",
            list.cmds().len()
        );
        // ★ is_animating 在 tick 里算，而实例 active 在 draw 里才置位——再走一帧，
        // 上一帧"画了东西"就该续帧（与特效同规则）。
        m.tick(&tick16(), &cfg, 1);
        assert!(m.is_animating(), "上一帧画了东西 → 续帧");

        // 点击 → on_mouse_down 记环（demo 用全局 `elapsed` 记时刻——签名里没有 t；
        // 这条守住 2026-09-26 的脚本 bug：写 `t = t` 会 nil 算术报错、环永远画不出）。
        m.on_click(MouseButton::Left, 150.0, 100.0);
        let mut list2 = DrawList::new();
        m.draw(&mut list2);
        assert!(
            list2.len_all() > list.len_all(),
            "点击后应多出波环指令（{} → {}）",
            list.len_all(),
            list2.len_all()
        );

        // ★ 时间必须流动（2026-09-26 修"拖尾永不消失"）：继续空转帧（无鼠标输入），
        //   寿命（缺省 0.8s，参数缺键走脚本内默认）耗尽后点应全部剔除——draw 零指令、
        //   不再续帧。原实现 elapsed 被 !trail_enabled 提前 return 冻结，此断言必炸。
        //   ★ 循环里必须每帧 draw：`active` 标记在 draw 时才重算（真实帧循环同款），
        //     只 tick 不 draw 的话 active 永远停在上一帧的值。
        for _ in 0..120 {
            m.tick(&tick16(), &cfg, 1);
            let mut frame = DrawList::new();
            m.draw(&mut frame);
        }
        let mut list3 = DrawList::new();
        m.draw(&mut list3);
        assert!(
            list3.len_all() == 0,
            "寿命耗尽后应无指令，剩 {} 条",
            list3.len_all()
        );
        assert!(!m.is_animating(), "点死光后不应续帧");
    }

    /// ★ MFCMouseEffect 的 Hold / Scroll 走**真实宿主链**（不是直接调引擎）：
    /// `composite_active` 选中脚本 → 复合宿主注入 `mouse_down`/`wall_ms` → 脚本出画。
    /// 这条守住用户报的「按住看不到/滚轮看不到」——单测引擎直调会绕过这两个注入点。
    #[test]
    fn mfcmouse_hold_and_scroll_run_through_the_real_host() {
        // Hold：按下 260ms 后才出画，按住 500ms 必有一帧。
        let mut cfg = Config::default();
        cfg.trail.trail_enabled = false;
        cfg.trail.hover_enabled = false;
        cfg.lua_ext
            .composite_active
            .push("scripts/MFCMouseEffect/charge".to_string());
        let mut m = TrailModule::new();
        m.set_screen(ScreenRect {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
        });
        m.set_wall_ms(50_000.0);
        m.tick(&tick16(), &cfg, 1);
        m.on_click(MouseButton::Left, 300.0, 200.0);
        m.set_wall_ms(50_500.0);
        m.tick(&tick16(), &cfg, 1);
        let mut held = DrawList::new();
        m.draw(&mut held);
        assert!(
            held.len_all() > 0,
            "Hold 脚本按住 500ms 应出画（真实宿主链）"
        );

        // Scroll：选中 twinkle → 真实宿主收到 on_wheel → 出画。
        let mut cfg2 = Config::default();
        cfg2.trail.trail_enabled = false;
        cfg2.trail.hover_enabled = false;
        cfg2.lua_ext
            .composite_active
            .push("scripts/MFCMouseEffect/twinkle".to_string());
        let mut m2 = TrailModule::new();
        m2.set_screen(ScreenRect {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
        });
        m2.set_wall_ms(60_000.0);
        m2.tick(&tick16(), &cfg2, 1);
        assert!(
            m2.companions().any_wheel_wanted(),
            "twinkle 应被识别为 on_wheel 脚本；ids={:?}",
            m2.companions().ids()
        );
        m2.on_wheel(120, false, 300.0, 200.0);
        m2.set_wall_ms(60_100.0);
        m2.tick(&tick16(), &cfg2, 1);
        let mut scrolled = DrawList::new();
        m2.draw(&mut scrolled);
        assert!(
            scrolled.len_all() > 0,
            "Scroll 脚本收到滚轮后应出画（真实宿主链）"
        );
    }

    // ── 快移插值（`interpolate`）的纯几何单测 ──

    /// 间距小于阈值 → 不补点（两点本就连得上，避免给慢移添冗余点）。
    #[test]
    fn interpolate_below_spacing_adds_nothing() {
        assert!(TrailModule::interpolate((10.0, 10.0), (12.0, 10.0), 4.0, 32).is_empty());
        assert!(TrailModule::interpolate((10.0, 10.0), (10.0, 10.0), 4.0, 32).is_empty());
    }

    /// 垂直快移 100px → 补点范围内的相邻转发点（含两个真实端点）间距都 ≤ spacing，
    /// 且只插中间、不落到端点本身——这是"不断线"的判据。
    #[test]
    fn interpolate_fills_fast_move() {
        let pts = TrailModule::interpolate((0.0, 100.0), (0.0, 0.0), 4.0, 32);
        assert!(!pts.is_empty(), "100px 快移应补出中间点");
        // 全在 y∈(0,100) 内部（不含端点）
        for (_, y) in &pts {
            assert!(*y > 0.0 && *y < 100.0, "插值点在两端内部，y={y}");
        }
        // 相邻转发点（头=from、尾=to）纵坐标差 ≤ spacing（4px 允许浮点往返误差）
        let mut prev_y = 100.0_f32;
        for (_, y) in &pts {
            assert!(
                (y - prev_y).abs() <= 4.5,
                "相邻点间距应 ≈ spacing：Δ={}, y={y}",
                (y - prev_y).abs()
            );
            prev_y = *y;
        }
        assert!(
            (0.0 - prev_y).abs() <= 4.5,
            "最后一个点离 to 也应 ≈ spacing"
        );
    }

    /// 间距精确等于阈值 → 仍不应以为反复往返产生零收益补点（边界不引入 NaN/无限）。
    #[test]
    fn interpolate_exact_spacing_is_bounded() {
        // 对角快移：dist=√(4²+3²)=5，spacing=1 → floor(5/1)=5 个内点，相邻间距 ≤1。
        let pts = TrailModule::interpolate((0.0, 0.0), (4.0, 3.0), 1.0, 8);
        assert_eq!(pts.len(), 5);
        for (x, y) in &pts {
            assert!(x.is_finite() && y.is_finite());
        }
    }

    /// max_points 安全阀：极端快移只补到上限，不无限膨胀。
    #[test]
    fn interpolate_respects_max_points() {
        let pts = TrailModule::interpolate((0.0, 0.0), (0.0, 10000.0), 4.0, 32);
        assert!(pts.len() <= 32, "最多补到 max_points，实际 {}", pts.len());
    }

    /// ★ 空闲 0% CPU 的回归锁：**只有悬浮标**（静态）时不能算"有动画"。
    ///
    /// 悬浮标每帧都产出同样的指令，`is_dirty()` 为真；但画面不随时间变化，续帧就是空转。
    /// 实测把这种情况当"要续帧"会让空闲 CPU 停在 7.6%。
    #[test]
    fn hover_only_is_not_animating() {
        let mut cfg = Config::default();
        cfg.trail.trail_enabled = true;
        cfg.trail.trail_effect = "none".to_string();
        cfg.trail.hover_enabled = true;
        cfg.trail.hover_type = "png".into();
        cfg.trail.ripple_bindings.get_mut("left").unwrap().enabled = false;

        let mut m = TrailModule::new();
        m.tick(&tick16(), &cfg, 1);
        // 首次 tick 后直接注入位图（同步不会再跑，不会被清掉）
        m.hover.set_png_image(Some(Rc::new(
            crate::core::image::RgbaBitmap::new(2, 2, vec![0; 16]).unwrap(),
        )));
        m.push_sample(100.0, 100.0, 1000);
        m.tick(&tick16(), &cfg, 1);

        assert!(m.is_dirty(), "悬浮标开着 → 屏上有内容");
        assert!(
            !m.is_animating(),
            "悬浮标是静态的 → 不该继续请求帧（否则空闲空转）"
        );

        let mut list = DrawList::new();
        m.draw(&mut list);
        assert!(!list.cmds().is_empty(), "静态悬浮标仍要画出来");
    }

    #[test]
    fn ripples_and_effects_are_animating() {
        let cfg = clickable_config();
        let mut m = TrailModule::new();
        m.tick(&tick16(), &cfg, 1);
        m.on_click(MouseButton::Left, 10.0, 10.0);
        assert!(m.is_animating(), "波纹在跑 → 必须连续帧");
        m.tick(&tick16(), &cfg, 1);
        assert!(m.is_animating(), "还没画过第一帧的新生实例不能被判死");
        m.clear();
        assert!(!m.is_animating(), "clear 之后没有动画了");
    }

    #[test]
    fn interval_effect分类() {
        assert!(TrailModule::is_interval_effect(&EffectId::PixieDust));
        assert!(TrailModule::is_interval_effect(&EffectId::CircleParticles));
        assert!(!TrailModule::is_interval_effect(&EffectId::Ribbon));
        assert!(!TrailModule::is_interval_effect(&EffectId::Glow));
        assert!(!TrailModule::is_interval_effect(&EffectId::SpiderWeb));
    }

    /// ★ 回归锁：面板改参数 → 配置版本 bump → 特效**立即**拿到新参数。
    ///
    /// 曾经只在"特效 id 变化"分支里 apply 一次：改参数（id 不变）永远停在特效创建
    /// 那一刻，表现为"拖尾不跟着变"。判据：改 particle_size 后**新生成**的粒子辉光
    /// 外径应变大（外层加色圆半径 = size × 1.4 × 2.8，默认 5 → ≤19.6；改 20 → ≤78.4）。
    ///
    /// ★ 为什么用**区间**断言而不是"after > before×K"或固定随机种子：
    /// `scripting/api_effect.rs` 里 Lua 的 `rng()` 绑定的是 `rand::thread_rng()`
    /// （系统时间播种），并非特效实例的固定种子 RNG——所以粒子半径**每轮都变**。
    /// 只按比例断言会把"旧粒子恰好滚到偏大、新粒子恰好偏小"判成失败（曾 30.65 vs
    /// 32.07，约 1/5 翻红）；固定种子又会改变所有 Lua 特效的运行期表现。改用**语义
    /// 区间**：ps=5 的辉光外径恒 ≤ 19.6，ps=20 的恒 ≥ 33.6，两段不相交——无论随机
    /// 半径怎么抖都不会误报，同时保留"参数没传过去（after 仍落在 ps=5 区间）必红"。
    #[test]
    fn param_changes_reach_the_running_effect() {
        let mut cfg = Config::default();
        cfg.trail.trail_enabled = true;
        cfg.trail.trail_effect = "pixie_dust".to_string();

        let max_radius = |list: &DrawList| -> f32 {
            list.cmds()
                .iter()
                .filter_map(|c| match c {
                    crate::core::draw::DrawCmd::Fill {
                        shape: crate::core::draw::Shape::Circle { radius, .. },
                        ..
                    } => Some(*radius),
                    _ => None,
                })
                .fold(0.0_f32, f32::max)
        };

        let mut m = TrailModule::new();
        m.tick(&tick16(), &cfg, 1); // 同步设置、建特效
                                    // 间隔生成需要"当帧有采样点"：每个 tick 前都推一个采样。
        for _ in 0..10 {
            m.push_sample(100.0, 100.0, 0);
            m.tick(&tick16(), &cfg, 2); // 攒够 spawn 间隔（默认 100ms），生成一批粒子
        }
        let mut list = DrawList::new();
        m.draw(&mut list);
        let before = max_radius(&list);
        assert!(before > 0.0, "特效应已画出粒子");

        // 改参数（version bump 触发重放）。粒子大小在建粒子时定格，
        // 所以继续让控制器生成**新**粒子来验证新参数已生效。
        cfg.trail.trail_settings.extra.insert(
            "pixie_dust.particle_size".to_string(),
            serde_json::json!(20.0),
        );
        m.push_sample(100.0, 100.0, 0);
        for _ in 0..5 {
            m.push_sample(100.0, 100.0, 0);
            m.tick(&tick16(), &cfg, 3);
        }
        let mut list2 = DrawList::new();
        m.draw(&mut list2);
        let after = max_radius(&list2);

        // ★ 两段不相交的语义区间：辉光半径上限 = ps × 1.4 × 2.8。ps=8（2026-09-28
        //   起 schema 默认；兜底值对齐那批改的）上限 31.36，ps=20 下限 33.6 —— 中间
        //   仍有干净的缝。判定不依赖随机半径的**具体实现**，只依赖分档。
        assert!(
            before <= 31.4,
            "改参数前大小应为默认档：before={before}（应 ≤ ps8 辉光上限 31.36）"
        );
        assert!(
            after >= 30.0,
            "改 size=20 后新生成的粒子辉光应显著变大：after={after}, before={before}（应 ≥ 30.0，ps20 下限 33.6）"
        );
    }

    /// ★ 回归锁：蛛丝缠网在「丝线粗细」取到 schema 下限时**不能崩**。
    ///
    /// 脚本里锚点线宽是 `rng(1.0, math.max(1.0, 粗细))`，而该参数的 schema 下限正是 1，
    /// 区间因此退化成 `[1.0, 1.0]`——`rand::gen_range` 对空区间直接 panic，进程当场闪退
    /// （2026-09-15 用户报告"切换蛛丝缠网崩溃"；又因为配置里已存下这个特效，之后每次
    /// 启动都会立刻再崩，表现为"再也打不开了"）。
    ///
    /// 修法是在 Lua API 的 `rng` 上兜底（见 `scripting::api_effect`）。这条测试守的就是
    /// 那层兜底：谁把它去掉，这里会以 panic 失败，而不是安静地绿着。
    #[test]
    fn spider_web_at_min_thickness_does_not_panic() {
        let mut cfg = Config::default();
        cfg.trail.trail_enabled = true;
        cfg.trail.trail_effect = "spider_web".to_string();
        // 粗细取下限 1（崩溃原值）；锚点拉满，保证光标附近一定有可连的锚点。
        cfg.trail
            .trail_settings
            .extra
            .insert("spider_web.particle_size".to_string(), serde_json::json!(1));
        cfg.trail
            .trail_settings
            .extra
            .insert("spider_web.density".to_string(), serde_json::json!(50));

        let mut m = TrailModule::new();
        m.tick(&tick16(), &cfg, 1);
        for i in 0..30 {
            m.push_sample(960.0, 540.0, i * 16);
            m.tick(&tick16(), &cfg, 2);
        }
        let mut list = DrawList::new();
        m.draw(&mut list);
        // 顺带确认特效真的加载并画了东西：否则"脚本目录没找到"会让这条测试因为
        // 什么都不曾运行而虚假通过——那种假绿比没有测试更危险。
        assert!(
            !list.cmds().is_empty(),
            "蛛丝缠网应已画出连线（若为空，多半是脚本目录没找到，测试环境有问题）"
        );
    }

    /// ★ 回归锁：飘带的**头部是一整个圆**（半径 = 半带宽），不是半圆、不是贝塞尔近似、
    /// 更不是尖角。
    ///
    /// 首版照抄原版 Rust 的「2 段三次贝塞尔封口」：控制点靠 `sign()` 逐分量拼出来，拐弯
    /// 时会鼓包或塌陷，且右端点取的是右边界的**尾部**点，封口压根不在头上。第二版改成
    /// 正半圆，方向对了，但用户要的是**完整**的圆：鼠标停下、尾迹褪尽之后，鼠标处仍得
    /// 留着一颗圆（2026-09-15 用户报告"头部应该总是有一个完整的圆"）。
    ///
    /// 判据是几何的：把落在头部附近的顶点按「前进方向 f / 左法线 n」分解出极角 θ，
    /// 要求它们①**全部落在半径 = 半带宽的圆上**（±0.6px），②**铺满整整一圈
    /// θ ∈ [0, 2π)**（相邻角间隙 ≤ 0.4rad，含跨 0 的那一段）。半圆版在后半圈一个
    /// 顶点都没有，贝塞尔版的控制点离头只有 0.55×半带宽，两条判据都会红。
    #[test]
    fn ribbon_head_is_a_round_cap() {
        use crate::core::draw::{DrawCmd, Shape};

        let mut cfg = Config::default();
        cfg.trail.trail_enabled = true;
        cfg.trail.trail_effect = "ribbon".to_string();
        cfg.trail
            .trail_settings
            .extra
            .insert("ribbon.max_width".to_string(), serde_json::json!(20));

        let mut m = TrailModule::new();
        m.tick(&tick16(), &cfg, 1);
        // 沿 +x 走一条直线：head 就是最后那个点，前进方向为 +x。
        let mut x = 200.0f32;
        for _ in 0..20 {
            x += 6.0;
            m.push_sample(x, 300.0, 0);
            m.tick(&tick16(), &cfg, 1);
        }
        let mut list = DrawList::new();
        m.draw(&mut list);

        let range = list
            .cmds()
            .iter()
            .find_map(|c| match c {
                DrawCmd::Fill {
                    shape:
                        Shape::Polyline {
                            points,
                            closed: true,
                        },
                    ..
                } => Some(*points),
                _ => None,
            })
            .expect("飘带应画出一条闭合填充多边形（若脚本没加载，这里会比断言更响地失败）");
        let all = list.points();
        let pts = &all[range.start as usize..(range.start + range.len) as usize];
        assert!(pts.len() > 12, "多边形点数异常：{}", pts.len());

        // 沿 +x 行进 ⇒ 前进方向 f = (1, 0)、左法线 n = rot90(f) = (0, 1)。
        // 于是顶点相对头的分解就是 a = dy（沿 n）、b = dx（沿 f），θ = atan2(b, a)。
        let (hx, hy) = (x, 300.0f32);
        let half_w = 10.0f32; // max_width = 20
        let mut thetas: Vec<f32> = pts
            .iter()
            .map(|p| (p.x - hx, p.y - hy))
            .filter(|&(dx, dy)| ((dx * dx + dy * dy).sqrt() - half_w).abs() <= 0.6)
            .map(|(dx, dy)| dx.atan2(dy).rem_euclid(std::f32::consts::TAU))
            .collect();
        assert!(
            thetas.len() >= 22,
            "落在半径=半带宽的圆上的顶点太少（{} 个）：头部不是圆",
            thetas.len()
        );
        thetas.sort_by(|a, b| a.total_cmp(b));
        // 整圈要连起来看：除了相邻顶点间的间隙，还要算跨 0 的那一段（首→尾绕过 2π）。
        let mut max_gap = std::f32::consts::TAU - (thetas[thetas.len() - 1] - thetas[0]);
        for w in thetas.windows(2) {
            max_gap = max_gap.max(w[1] - w[0]);
        }
        assert!(
            max_gap <= 0.4,
            "圆周上有 {max_gap:.2}rad 的缺口：头部没绕满一圈，不是完整的圆"
        );
    }

    /// ★ 回归锁：鼠标停下之后飘带**继续老化并褪尽，但头部的圆要留在鼠标处**。
    ///
    /// 原版（与首版脚本）把过期清理只挂在 `on_move` 里，而鼠标一动不动时根本没有
    /// `on_move`：整条飘带就永远冻在屏幕上（2026-09-15 用户报告"尾迹不消失"）。
    /// `step` 每帧都跑，老化必须放在那里。
    ///
    /// 但老化又不能把点全清光——全清之后鼠标处连头部那颗圆都没了（同一天用户报告
    /// "完全停下来的时候也有一个圆"）。所以判据是：尾迹褪尽后**只剩一颗头部圆**。
    #[test]
    fn ribbon_keeps_only_the_head_circle_after_the_mouse_stops() {
        let mut cfg = Config::default();
        cfg.trail.trail_enabled = true;
        cfg.trail.trail_effect = "ribbon".to_string();
        cfg.trail
            .trail_settings
            .extra
            .insert("ribbon.fade_duration".to_string(), serde_json::json!(0.3));

        let mut m = TrailModule::new();
        m.tick(&tick16(), &cfg, 1);
        let mut x = 200.0f32;
        for _ in 0..20 {
            x += 6.0;
            m.push_sample(x, 300.0, 0);
            m.tick(&tick16(), &cfg, 1);
        }
        let mut list = DrawList::new();
        m.draw(&mut list);
        assert!(!list.cmds().is_empty(), "刚停下时飘带应当还在");

        // 鼠标不再产生采样，只推进时间：fade_duration = 0.3s，这里推进约 0.64s。
        for _ in 0..40 {
            m.tick(&tick16(), &cfg, 1);
        }
        let mut list2 = DrawList::new();
        m.draw(&mut list2);

        // ① 尾迹必须已经褪尽：一条填充多边形都不该剩。
        use crate::core::draw::{DrawCmd, Shape};
        let bodies = list2
            .cmds()
            .iter()
            .filter(|c| {
                matches!(
                    c,
                    DrawCmd::Fill {
                        shape: Shape::Polyline { closed: true, .. },
                        ..
                    }
                )
            })
            .count();
        assert_eq!(
            bodies, 0,
            "鼠标停下超过「尾迹长度」后飘带多边形必须消失，实得 {bodies} 条"
        );

        // ② 但头部那颗圆要留着：圆心 = 最后的鼠标位置，半径 = 半带宽
        //    （★ 2026-09-28 起脚本兜底值与 schema 默认对齐：max_width 20 → 半径 10。
        //    旧兜底 12 → 半径 6，正是"面板显示的默认值 ≠ 实际运行值"那批 bug 的锚点）。
        let head_circle = list2.cmds().iter().find_map(|c| match c {
            DrawCmd::Fill {
                shape: Shape::Circle { center, radius },
                ..
            } => Some((*center, *radius)),
            _ => None,
        });
        let (c, r) = head_circle.expect("尾迹褪尽后鼠标处应仍留着一颗头部圆");
        assert!(
            (c.x - x).abs() < 0.5 && (c.y - 300.0).abs() < 0.5,
            "头部圆应停在最后的鼠标位置 ({x}, 300)，实得 ({}, {})",
            c.x,
            c.y
        );
        assert!(
            (r - 10.0).abs() < 0.001,
            "头部圆半径应为半带宽 10，实得 {r}"
        );
    }

    /// ★ 回归锁：间隔类特效的**生成速率由真实时间决定，与鼠标采样率无关**。
    ///
    /// 原版 Python 在 `on_mouse_move` 里用 `perf_counter` 算"距上次生成过了多久"，
    /// 生成完立刻把基准重置为 now（`resources/effects/__init__.py:69-87`）。因此无论
    /// 鼠标事件多密，一秒最多生成 `density/interval` 个粒子。
    ///
    /// 这里曾经把 `spawn_acc += dt_s` 写在**每个采样点**的循环体内，且 `batch` 上限
    /// 也在每个采样点内重置。一帧攒下 K 个采样点，就等于把帧时长推进了 K 次，生成速率
    /// 被乘以 K。鼠标回报率越高 K 越大（125Hz≈2，1000Hz≈16），粒子数直接顶到池上限
    /// 8192，每帧 2.4 万条绘制指令（每粒子 3 条）→ 严重卡顿；且"帧越慢→攒的采样点越多
    /// →生成越多→帧更慢"构成正反馈，卡顿会自我放大。
    ///
    /// 判据用"两个采样率下的粒子数比例"，而不是绝对个数：绝对个数会随 fade_duration /
    /// density 这些参数漂移，比例才是这条 bug 的本质特征。
    #[test]
    fn spawn_rate_is_independent_of_sample_rate() {
        /// 跑 `frames` 帧（每帧 16ms），每帧投 `per_frame` 个采样点，返回屏上粒子数。
        ///
        /// `DrawList` 里每个粒子固定产出 3 条指令（外辉光 / 内辉光 / 菱形核心），
        /// 所以用 `cmds.len() / 3` 反推粒子数，不必给控制器开测试专用接口。
        fn particles_after(frames: usize, per_frame: usize) -> usize {
            let mut cfg = Config::default();
            cfg.trail.trail_enabled = true;
            cfg.trail.trail_effect = "pixie_dust".to_string();
            // §38 起参数只认命名空间键（共享字段已不再播种，见 settings::from_config）。
            for (k, v) in [
                ("density", serde_json::json!(6)),
                ("spawn_interval_ms", serde_json::json!(20)),
                ("fade_duration", serde_json::json!(1.0)),
            ] {
                cfg.trail
                    .trail_settings
                    .extra
                    .insert(format!("pixie_dust.{k}"), v);
            }

            let mut m = TrailModule::new();
            m.tick(&tick16(), &cfg, 1); // 同步设置、建特效
            for f in 0..frames {
                for k in 0..per_frame {
                    m.push_sample(100.0 + (f * per_frame + k) as f32, 100.0, 0);
                }
                m.tick(&tick16(), &cfg, 1);
            }
            let mut list = DrawList::new();
            m.draw(&mut list);
            assert_eq!(
                list.cmds().len() % 3,
                0,
                "每个粒子应恰好产出 3 条指令，实际 {}",
                list.cmds().len()
            );
            list.cmds().len() / 3
        }

        // 跑 2 秒，让存量进入稳态（粒子寿命均值 0.75s）。
        let slow = particles_after(120, 2); // ≈120Hz 回报率
        let fast = particles_after(120, 16); // ≈960Hz 回报率
        println!("[生成率] 120Hz->{slow} 个, 960Hz->{fast} 个");

        // 稳态存量 ≈ density × (平均寿命 / 间隔) = 6 × (0.75 / 0.02) ≈ 225。
        assert!(
            (80..400).contains(&slow),
            "稳态粒子数应在理论值 225 附近，实际 {slow}"
        );
        assert!(
            fast <= slow * 2,
            "生成速率不该随采样率线性增长：120Hz 时 {slow} 个，960Hz 时 {fast} 个"
        );
        assert!(
            fast < 600,
            "粒子数不该逼近像素尘埃的池上限 8192：实际 {fast}（每粒子 3 条指令，顶满就是每帧 2.4 万条）"
        );
    }

    /// 一次点击 spawn 出的特效总数——**与实现路径无关**。
    ///
    /// 点击效果有两条路径：`script` 指向的 `assets/ClickFx/<id>.lua` 存在 → Lua
    /// [`ScriptedClick`]（`click_fx`）；脚本缺失 → 退回 Rust [`Ripple`]（`ripples`）。
    /// 关心"哪个键的开关放行"的测试不该被"出厂默认脚本是哪个"牵着走，统一数两处之和。
    fn spawned(m: &TrailModule) -> usize {
        m.ripples.active() + m.click_fx.len()
    }

    #[test]
    fn click_adds_ripple_when_enabled() {
        let cfg = clickable_config();
        let mut m = TrailModule::new();
        m.tick(&tick16(), &cfg, 1); // 同步设置
        assert_eq!(spawned(&m), 0);
        m.on_click(MouseButton::Left, 100.0, 200.0);
        assert_eq!(spawned(&m), 1, "左键效果启用时应加一个特效");
        assert!(m.is_dirty());
    }

    /// 出厂默认（`script: "disc"`，§39 起目录收敛为 disc / multi_ring）→ 走 Lua 脚本实例。
    #[test]
    fn click_uses_the_script_path_by_default() {
        let disc = crate::core::paths::clickfx_dir().join("disc.lua");
        assert!(
            disc.is_file(),
            "出厂默认脚本 {disc:?} 不在：测试须从 crate 根（main）跑，且 assets/ClickFx 要跟过来"
        );
        let cfg = clickable_config();
        assert_eq!(
            cfg.trail.ripple_bindings["left"].script, "disc",
            "出厂默认应已解析成脚本 id（见 seeded_ripple_bindings）"
        );
        let mut m = TrailModule::new();
        m.tick(&tick16(), &cfg, 1);
        m.on_click(MouseButton::Left, 100.0, 200.0);
        assert_eq!(m.click_fx.len(), 1, "选中脚本时应建一个 Lua 实例");
        assert_eq!(m.ripples.active(), 0, "Lua 路径下不该再叠一个内置波纹");
    }

    /// 脚本缺失（`assets/ClickFx` 空 / 用户删了脚本）→ 退回 Rust `Ripple`，波纹还在。
    ///
    /// ★ 这条覆盖的是"素材整个丢了"的兜底：功能降级但**不消失**。
    #[test]
    fn click_falls_back_to_builtin_when_script_is_absent() {
        let mut cfg = clickable_config();
        cfg.trail
            .ripple_bindings
            .get_mut("left")
            .unwrap()
            .script
            .clear();
        let mut m = TrailModule::new();
        m.tick(&tick16(), &cfg, 1);
        m.on_click(MouseButton::Left, 100.0, 200.0);
        assert_eq!(m.ripples.active(), 1, "脚本缺失时应退回内置波纹");
        assert!(m.click_fx.is_empty(), "没有脚本就不该有 Lua 实例");
    }

    #[test]
    fn click_adds_ripple_even_without_trail_master() {
        // ★ 卡片式开关语义：波纹只听自己卡片的开关。"拖尾效果"关着时，左键效果开启的
        // 点击照常出波纹（旧单总开关时代的早退已废除）。
        let mut cfg = clickable_config();
        cfg.trail.trail_enabled = false;
        let mut m = TrailModule::new();
        m.tick(&tick16(), &cfg, 1);
        m.on_click(MouseButton::Left, 100.0, 200.0);
        assert_eq!(spawned(&m), 1, "波纹开关开着就该出波纹，与拖尾卡片开关无关");
        assert!(m.is_dirty());
    }

    #[test]
    fn middle_click_uses_its_own_binding() {
        // ★ 每键独立绑定：中键效果开启（左键关）→ 中键点击出波纹。
        let mut cfg = Config::default();
        cfg.trail.ripple_enabled = true; // 出厂总开关已改关，测试显式打开
        cfg.trail.ripple_bindings.get_mut("middle").unwrap().enabled = true;
        cfg.trail.ripple_bindings.get_mut("middle").unwrap().size = 150;
        let mut m = TrailModule::new();
        m.tick(&tick16(), &cfg, 1);
        m.on_click(MouseButton::Middle, 50.0, 60.0);
        assert_eq!(spawned(&m), 1, "中键绑定开启时中键点击应出波纹");
        // 左键仍无波纹（其绑定未开启）
        m.on_click(MouseButton::Left, 50.0, 60.0);
        assert_eq!(spawned(&m), 1, "左键未绑定，不应新增波纹");
    }

    #[test]
    fn right_click_uses_own_switch() {
        // 只开了左键效果：右键不该产生波纹。
        let cfg = clickable_config();
        let mut m = TrailModule::new();
        m.tick(&tick16(), &cfg, 1);
        m.on_click(MouseButton::Right, 5.0, 5.0);
        assert_eq!(spawned(&m), 0);
    }

    /// 开了总开关、鼠标移动 + 点击之后，`draw` 必须真的产出绘制指令。
    ///
    /// 这是 P3.5 的回归锁：`hover_dot` / `ripples` 这两个文件早就有 `draw`，但"有没有被
    /// `TrailModule::draw` 调用"是另一回事——漏接的话单测全绿、屏上什么也看不见。
    #[test]
    fn draw_emits_hover_and_ripple_when_enabled() {
        let mut cfg = clickable_config();
        cfg.trail.hover_enabled = true;
        cfg.trail.hover_type = "png".into();

        let mut m = TrailModule::new();
        m.tick(&tick16(), &cfg, 1);
        // 首次 tick 后注入位图（png 类型宿主绘制，无需脚本目录）
        m.hover.set_png_image(Some(Rc::new(
            crate::core::image::RgbaBitmap::new(2, 2, vec![0; 16]).unwrap(),
        )));
        m.push_sample(300.0, 400.0, 1000);
        m.tick(&tick16(), &cfg, 1);
        m.on_click(MouseButton::Left, 300.0, 400.0);

        let mut list = DrawList::new();
        m.draw(&mut list);
        assert!(!list.cmds().is_empty(), "悬浮标 + 波纹都开启时应有绘制指令");

        // 悬浮标关掉、波纹也关掉 → 不该再画任何东西。
        let mut off = clickable_config();
        off.trail.ripple_bindings.get_mut("left").unwrap().enabled = false;
        let mut m2 = TrailModule::new();
        m2.tick(&tick16(), &off, 1);
        m2.push_sample(1.0, 1.0, 0);
        m2.tick(&tick16(), &off, 1);
        let mut empty = DrawList::new();
        m2.draw(&mut empty);
        assert!(
            empty.cmds().is_empty(),
            "悬浮标/波纹/拖尾全关时不该有绘制指令"
        );
    }

    #[test]
    fn hover_draws_without_trail_master() {
        // ★ 卡片式开关语义：悬浮标只听自己的开关（hover_enabled），与"拖尾效果"卡片无关。
        // 这是"只开悬浮标什么都不显示"回归的锁（原实现里 draw 整体早退于 trail_enabled）。
        let mut cfg = clickable_config();
        cfg.trail.hover_enabled = true;
        cfg.trail.hover_type = "png".into();
        cfg.trail.trail_enabled = false;
        cfg.trail.ripple_bindings.get_mut("left").unwrap().enabled = false;
        let mut m = TrailModule::new();
        m.tick(&tick16(), &cfg, 1);
        m.hover.set_png_image(Some(Rc::new(
            crate::core::image::RgbaBitmap::new(2, 2, vec![0; 16]).unwrap(),
        )));
        m.push_sample(10.0, 10.0, 0);
        m.tick(&tick16(), &cfg, 1);
        let mut list = DrawList::new();
        m.draw(&mut list);
        assert!(
            !list.cmds().is_empty(),
            "只开悬浮标（拖尾/波纹关）也必须画出来"
        );
        assert!(!m.is_animating(), "静态悬浮标不该驱动续帧");
        assert!(m.is_dirty(), "屏上有悬浮标 → dirty");
    }

    /// ★ 回归锁：每键选了自定义脚本后，按下生成脚本实例（而非内置波纹），
    /// 脚本实例会随"不再绘制 / 超龄"被剔除，且不污染 `ripples` 计数。
    #[test]
    fn clickfx_script_replaces_builtin_and_expires() {
        use crate::core::paths::clickfx_dir;
        // 临时往点击效果脚本目录放一个最简脚本（测完清掉，避免污染 assets）。
        let dir = clickfx_dir();
        let _ = std::fs::create_dir_all(&dir);
        let id = "__ct_unit_ring";
        let path = dir.join(format!("{id}.lua"));
        let body = r##"
effect_name = "单元圆"
function schema() return {} end
function on_click(x, y, t, p) cx, cy = x, y; age = 0 end
function step(dt, t, p) age = (age or 0) + dt end
function draw(ctx, p) ctx:circle(cx, cy, 18, 255, 255, 255, 255) end
"##;
        std::fs::write(&path, body).unwrap();

        let mut cfg = Config::default();
        cfg.trail.ripple_enabled = true; // 出厂总开关已改关，测试显式打开
        if let Some(l) = cfg.trail.ripple_bindings.get_mut("left") {
            l.enabled = true;
            l.script = id.to_string();
        }
        let mut m = TrailModule::new();
        m.tick(&tick16(), &cfg, 1); // 同步设置
        assert_eq!(m.click_fx.len(), 0);

        m.on_click(MouseButton::Left, 50.0, 60.0);
        assert_eq!(m.click_fx.len(), 1, "应生成脚本波纹实例");
        assert_eq!(m.ripples.active(), 0, "不应生成内置波纹");

        // 画一帧，脚本确实产出了绘制指令。
        let mut list = DrawList::new();
        m.draw(&mut list);
        assert!(!list.cmds().is_empty(), "脚本应已绘制");

        // 推进足够久让 age 超过硬上限（8000ms）→ 被剔除。
        for _ in 0..600 {
            m.tick(&tick16(), &cfg, 1);
        }
        assert!(m.click_fx.is_empty(), "超龄脚本实例应被剔除");

        let _ = std::fs::remove_file(&path);
    }

    /// ★ 回归锁（2026-09-28 用户报告「辉光复制进点击效果文件夹就没有轨迹了」）：
    /// 实现了 `on_move` 的脚本（轨迹语义）被波纹绑定选中时，必须获得**常驻待遇**——
    /// 每个真实采样喂 `on_move`（累积池成形），按下转发 `on_press`，且**不再**按次
    /// 生成短命实例。生命周期跟随脚本能力，文件夹只决定 UI 归属（用户口径：
    /// 「不同的文件夹仅仅只是读取和生成 ui 可能有区别，不影响底层功能」）。
    #[test]
    fn trail_semantics_script_in_clickfx_dir_runs_persistent() {
        use crate::core::paths::clickfx_dir;
        let dir = clickfx_dir();
        let _ = std::fs::create_dir_all(&dir);
        let id = "__ct_unit_persistent";
        let path = dir.join(format!("{id}.lua"));
        let body = r##"
effect_name = "常驻池"
function schema() return {} end
pts = {}
function on_move(x, y, t, p) pts[#pts+1] = {x, y} end
function on_press(x, y, t, p) pressed = (pressed or 0) + 1 end
function draw(ctx, p)
  for i = 1, #pts do ctx:circle(pts[i][1], pts[i][2], 5, 255, 0, 255, 255) end
end
"##;
        std::fs::write(&path, body).unwrap();

        let mut cfg = Config::default();
        cfg.trail.ripple_enabled = true;
        cfg.trail.trail_enabled = false; // ★ 与"拖尾效果"卡片完全无关
        if let Some(l) = cfg.trail.ripple_bindings.get_mut("left") {
            l.enabled = true;
            l.script = id.to_string();
        }
        let mut m = TrailModule::new();
        m.tick(&tick16(), &cfg, 1); // 同步设置（含常驻通道对账）
        assert_eq!(m.persistent_fx.len(), 1, "轨迹语义脚本应进入常驻通道");
        assert_eq!(m.click_fx.len(), 0, "常驻通道不该按次 spawn");

        // 按下：转发给常驻实例，不生成短命实例、不叠内置波纹。
        m.on_click(MouseButton::Left, 50.0, 60.0);
        assert_eq!(m.click_fx.len(), 0, "轨迹语义脚本不该再按次 spawn");
        assert_eq!(m.ripples.active(), 0);

        // 采样流喂 on_move：累积池成形 → draw 产出整条轨迹（≥5 个点）。
        for i in 0..5 {
            m.push_sample(100.0 + i as f32 * 20.0, 200.0, i * 10);
        }
        m.tick(&tick16(), &cfg, 1);
        let mut list = DrawList::new();
        m.draw(&mut list);
        assert!(
            list.cmds().len() >= 5,
            "轨迹语义脚本应画出整条轨迹，实际 {} 条指令",
            list.cmds().len()
        );
        assert!(m.is_animating(), "常驻轨迹在画 → 应驱动续帧");

        let _ = std::fs::remove_file(&path);
    }

    /// ★ 回归锁（用户报告「聚光灯里点击效果没效果」）：把聚光灯脚本当作**拖尾特效**
    /// 用时，它整屏暗罩的 `clear_circle` 若排在波纹之后，破坏性 Clear 会把波纹抹掉。
    ///
    /// 修法：`ctx:layer("base")` 自声明垫底层 → 暗罩先画（`Blend::Clear` 在 Base 桶，
    /// 只清自己的暗罩），波纹在 Normal 桶随后画 → 洞内清晰可见。判据直接用渲染顺序
    /// `ordered()`：聚光灯的 Clear 指令必须出现在波纹圆圈**之前**。
    #[test]
    fn spotlight_as_trail_effect_clears_before_ripples() {
        use crate::core::draw::{Blend, DrawCmd, Shape};
        use crate::core::paths::mousetrail_dir;

        let dir = mousetrail_dir();
        let _ = std::fs::create_dir_all(&dir);
        let id = "__ct_unit_spotlight";
        let path = dir.join(format!("{id}.lua"));
        let body = r##"
effect_name = "测聚光灯"
function schema() return {} end
function draw(ctx, params)
  ctx:layer("base")
  ctx:rect(0, 0, screen_w, screen_h, 0, 0, 0, 170)
  ctx:clear_circle(mouse_x, mouse_y, 140)
end
"##;
        std::fs::write(&path, body).unwrap();

        let mut cfg = Config::default();
        cfg.trail.trail_enabled = true;
        cfg.trail.trail_effect = id.to_string();
        cfg.trail.hover_enabled = false;
        // 用内置波纹（非 Lua），确保画出来的是普通层 SourceOver 圆圈。
        cfg.trail.ripple_enabled = true; // 出厂总开关已改关，测试显式打开
        cfg.trail.ripple_bindings.get_mut("left").unwrap().enabled = true;
        cfg.trail
            .ripple_bindings
            .get_mut("left")
            .unwrap()
            .script
            .clear();

        let mut m = TrailModule::new();
        m.tick(&tick16(), &cfg, 1); // 建聚光灯特效
        m.push_sample(300.0, 400.0, 0);
        m.tick(&tick16(), &cfg, 2);
        m.on_click(MouseButton::Left, 300.0, 400.0); // 出一圈波纹

        let mut list = DrawList::new();
        m.draw(&mut list);

        // 找到渲染顺序里第一条「Clear（聚光灯挖洞）」与第一条「波纹圆圈」。
        // 波纹默认样式是描边环（Stroke）——不做 Fill/Stroke 区分，只要不是 Clear 的圆就算。
        let cmds: Vec<&DrawCmd> = list.ordered().collect();
        let clear_pos = cmds
            .iter()
            .position(|c| {
                matches!(
                    c,
                    DrawCmd::Fill {
                        blend: Blend::Clear,
                        ..
                    }
                )
            })
            .expect("聚光灯暗罩应先画出一条 Clear 挖洞指令");
        let ripple_pos = cmds
            .iter()
            .position(|c| {
                !matches!(
                    c,
                    DrawCmd::Fill {
                        blend: Blend::Clear,
                        ..
                    }
                ) && matches!(
                    c,
                    DrawCmd::Fill {
                        shape: Shape::Circle { .. },
                        ..
                    } | DrawCmd::Stroke {
                        shape: Shape::Circle { .. },
                        ..
                    }
                )
            })
            .expect("左键效果应画出一条圆圈指令");
        assert!(
            clear_pos < ripple_pos,
            "聚光灯的 Clear(#{clear_pos}) 必须在波纹圆圈(#{ripple_pos})之前，波纹才不被抹掉"
        );

        let _ = std::fs::remove_file(&path);
    }
}
