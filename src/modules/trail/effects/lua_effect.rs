//! Lua 特效桥接器。将 Lua 脚本包装为 `TrailEffect` trait 对象。
//!
//! 把脚本目录里的一个 `.lua` 文件包装成一个特效实例。**驱动序列本身在
//! [`FxCore`](super::super::fx_core)（四类宿主唯一实现）**，本结构体只剩三件事：
//! 拖尾宿主的身份（`host = "trail"`）、共享时钟对齐（脚本拿到的 `t` 与控制器
//! 逐位一致）、`spawn_batch` 透传。
//!
//! # 契约（2026-09-27 四宿主统一到 FxCore）
//!
//! 事件名、`dt` 刻度（`FIXED_DT` 固定步长秒）、环境变量（`cursor_*` / `origin_*` /
//! `host` / `wall_ms`）四类宿主完全一致——**同一个脚本放哪个目录效果都一样**。

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value as Json;

use crate::core::draw::DrawList;

use super::super::fx_core::FxCore;
use super::super::sample::MouseSample;
use super::super::settings::EffectDefaults;
use super::{EffectCtx, TrailEffect};

/// Lua 特效实现。每个 Lua 脚本对应一个 `LuaEffect` 实例。
pub struct LuaEffect {
    /// 统一实例核心（引擎 / 参数 / 环境 / 事件 / 步进 / 判活都在里面）。
    core: FxCore,
    /// 构造期一次性 `Box::leak` 出来、永久存活的 `&'static str`：避免 `id()`/`name()`
    /// 每次被调用都泄漏一个新字符串（特效名在 UI / 注册表里高频读取）。
    id: &'static str,
    name: &'static str,
    defaults_cache: EffectDefaults,
}

impl LuaEffect {
    /// 从 Lua 脚本创建效果。
    pub fn new(scripts_dir: &Path, script_id: &str) -> mlua::Result<Self> {
        let core = FxCore::load(scripts_dir, script_id, "trail")?;

        let name: String = core
            .engine
            .lua()
            .globals()
            .get::<Option<String>>("name")
            .ok()
            .flatten()
            .or_else(|| core.engine.get_effect_name().ok())
            .unwrap_or_else(|| script_id.to_string());

        // 从 schema() 的默认值构造 EffectDefaults，供首次加载时用默认值喂 Lua。
        let defaults_cache = match core.engine.call_schema() {
            Ok(tbl) => Self::build_defaults(&crate::scripting::registry::parse_schema_table(&tbl)),
            Err(_) => EffectDefaults {
                map: HashMap::new(),
            },
        };

        // 构造期把 id/name 泄漏成 'static（这两个字符串随进程终身使用，无法回收本是合理代价，
        // 关键是只泄漏一次，而不是每次访问都泄漏）。
        let id: &'static str = Box::leak(script_id.to_string().into_boxed_str());
        let name: &'static str = Box::leak(name.into_boxed_str());
        Ok(Self {
            core,
            id,
            name,
            defaults_cache,
        })
    }

    /// 把脚本 schema 解析出的参数默认值转成 `EffectDefaults`。
    fn build_defaults(params: &[crate::scripting::registry::ParamDef]) -> EffectDefaults {
        let mut map = HashMap::new();
        for p in params {
            map.insert(p.key.clone(), p.default_value());
        }
        EffectDefaults { map }
    }
}

impl TrailEffect for LuaEffect {
    fn id(&self) -> &'static str {
        self.id
    }

    fn name(&self) -> &'static str {
        self.name
    }

    fn default_settings() -> EffectDefaults {
        EffectDefaults {
            map: HashMap::new(),
        }
    }

    fn apply(&mut self, params: &HashMap<String, Json>) {
        self.core.set_params(params.clone());
    }

    fn on_move(&mut self, s: MouseSample, ctx: &mut EffectCtx) {
        // 拖尾的 t 与控制器共享时钟对齐（ctx.elapsed），wall_ms 随 ctx 透传。
        self.core.set_elapsed(ctx.elapsed);
        self.core.set_wall_ms(ctx.wall_ms);
        self.core.set_screen(ctx.screen.w, ctx.screen.h);
        self.core.move_to(s.x, s.y, Some(ctx.spawn_batch));
    }

    fn on_press(&mut self, x: f32, y: f32, t: f32) {
        self.core.set_elapsed(t);
        self.core.press(x, y);
    }

    fn on_mouse_up(&mut self, button: crate::core::events::MouseButton, x: f32, y: f32, t: f32) {
        self.core.set_elapsed(t);
        let name = match button {
            crate::core::events::MouseButton::Left => "left",
            crate::core::events::MouseButton::Right => "right",
            crate::core::events::MouseButton::Middle => "middle",
            crate::core::events::MouseButton::X1 => "x1",
            crate::core::events::MouseButton::X2 => "x2",
        };
        self.core.release(name, x, y);
    }

    fn on_wheel(&mut self, delta: i32, horizontal: bool, x: f32, y: f32, t: f32) {
        self.core.set_elapsed(t);
        self.core.wheel(delta, horizontal, x, y);
    }

    fn wants_wheel(&self) -> bool {
        self.core.wants_wheel()
    }

    fn step(&mut self, dt: f32, ctx: &mut EffectCtx) {
        self.core.set_elapsed(ctx.elapsed);
        self.core.set_wall_ms(ctx.wall_ms);
        self.core.set_screen(ctx.screen.w, ctx.screen.h);
        // 控制器已经按 FIXED_DT 逐子步调进来，这里 dt 恒等于 FIXED_DT →
        // advance 内部恰好交付一个子步（零头逻辑不参与）。
        self.core.advance(dt);
    }

    fn draw(&mut self, list: &mut DrawList) {
        self.core.render(list);
    }

    fn is_active(&self) -> bool {
        self.core.active
    }

    fn hide_cursor(&self) -> bool {
        self.core.hide_cursor
    }

    fn clear(&mut self) {
        // 重置实例状态：重放 init()、出生点作废（换特效 = 换出生点）、判活清零。
        self.core.reset();
    }

    fn defaults(&self) -> EffectDefaults {
        self.defaults_cache.clone()
    }
}
