//! Lua VM 管理、脚本加载、API 注册。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mlua::{Lua, StdLib};
use serde_json::Value as Json;

use super::api_draw;
use super::api_effect;
use super::api_ext;
/// Lua 脚本引擎。封装一个 Lua 5.4 VM 实例。
pub struct ScriptingEngine {
    lua: Lua,
    scripts_dir: PathBuf,
    /// `ctx.fs` 的绑定目录：`load_script` 成功后指向**脚本所在目录**（含 `scripts/`
    /// 这类子目录脚本），未加载过脚本时回落到 `scripts_dir`。
    /// ★ 用 `RefCell`：`load_script` 是 `&self`，全链路十来个调用方不值得为它改签名。
    fs_base: std::cell::RefCell<PathBuf>,
}

impl ScriptingEngine {
    /// 创建新的脚本引擎，加载脚本目录下的公共 API。
    pub fn new(scripts_dir: &Path) -> mlua::Result<Self> {
        let lua = Lua::new_with(
            StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::PACKAGE,
            Default::default(),
        )?;

        let engine = Self {
            lua,
            scripts_dir: scripts_dir.to_path_buf(),
            fs_base: std::cell::RefCell::new(scripts_dir.to_path_buf()),
        };

        engine.register_api()?;
        Ok(engine)
    }

    /// 获取内部 Lua 引用。
    pub fn lua(&self) -> &Lua {
        &self.lua
    }

    /// 注册所有公共 API（绘图、环境变量等）。
    fn register_api(&self) -> mlua::Result<()> {
        api_draw::register_draw_api(&self.lua)?;
        api_effect::register_effect_api(&self.lua)?;
        // 扩展脚本 API（`ext` userdata，冒号调用）。给 `assets/lua` 的动作脚本用。
        self.lua
            .globals()
            .set("ext", self.lua.create_userdata(api_ext::ExtApi)?)?;
        Ok(())
    }

    /// 加载并执行一个 Lua 效果脚本。
    pub fn load_script(&self, script_name: &str) -> mlua::Result<()> {
        let path = self.scripts_dir.join(format!("{}.lua", script_name));
        let code = std::fs::read_to_string(&path).map_err(|e| {
            mlua::Error::RuntimeError(format!("无法读取脚本 {}: {}", path.display(), e))
        })?;
        // `ctx.fs` 跟着脚本走：`scripts/AI/ai_chat` 绑 `scripts_dir/scripts/AI`（脚本所在目录），
        // 顶层脚本绑 `scripts_dir`。文档口径「相对脚本所在目录」由此成立。
        if let Some(dir) = path.parent() {
            *self.fs_base.borrow_mut() = dir.to_path_buf();
            // 同一个目录再交给 `ext:proc`／`ext:script_dir`（m10）：外部程序只准从这个目录
            // 起。两处必须是**同一个来源**，所以在这里一次性记下，不让 api_ext 自己再猜。
            super::api_ext::set_script_dir(dir);
            super::api_ext::set_script_id(script_name);
        }
        self.lua.load(&code).exec()
    }

    /// 更新 Lua 端环境变量（每帧调用）。
    ///
    /// ★★ 四类宿主（拖尾 / 悬浮标 / 点击效果 / 独立卡片）**共用这一处注入**，字段语义
    ///    对所有宿主完全一致——这是"同一份脚本放哪都能跑"的地基。任何宿主想加字段，
    ///    改这里一处即可，不许各自 `globals().set` 私自分叉。
    ///
    /// - `cursor`：指针当前位置。点击效果里没有实时指针，填**按下点**。
    /// - `origin`：本次实例的**出生点**（拖尾首个采样点 / 点击按下点 / 悬浮标当前位置）。
    ///   脚本想在"出生处"画东西时直接用它，不必自己靠 `on_press` 记。
    /// - `host`：`"trail"` / `"hover"` / `"clickfx"` / `"panel"`，脚本需要分支时读它。
    ///
    /// `mouse_x` / `mouse_y` 保留为 `cursor_*` 的**别名**（老脚本零改动）。注意语义变化：
    /// 点击效果里它们**不再是 0**，而是按下点——此前恒 0 会让误用它的脚本画在屏幕左上角。
    pub fn update_env(
        &self,
        cursor: (f32, f32),
        origin: (f32, f32),
        elapsed: f32,
        screen: (f32, f32),
        host: &str,
    ) -> mlua::Result<()> {
        let globals = self.lua.globals();
        globals.set("cursor_x", cursor.0)?;
        globals.set("cursor_y", cursor.1)?;
        globals.set("origin_x", origin.0)?;
        globals.set("origin_y", origin.1)?;
        // 别名：老脚本读 mouse_x/mouse_y 不受影响。
        globals.set("mouse_x", cursor.0)?;
        globals.set("mouse_y", cursor.1)?;
        globals.set("elapsed", elapsed)?;
        globals.set("screen_w", screen.0)?;
        globals.set("screen_h", screen.1)?;
        // 虚拟桌面原点：`screen_w/h` 只是包围盒尺寸，原点可能为负（副屏在主屏左侧/
        // 上方）。整屏暗罩、星空、镜像中心都需要它，否则会整体平移一个原点。
        let vs = crate::platform::display::virtual_screen();
        globals.set("screen_x", vs.x as f32)?;
        globals.set("screen_y", vs.y as f32)?;
        globals.set("host", host)?;
        Ok(())
    }

    /// 注入**真实墙钟**毫秒（全局 `wall_ms`）。
    ///
    /// ★ 与 `elapsed` 的区别（别混用）：`elapsed` 是逐帧 capped 时间的累加值——覆盖层
    ///   休眠 / 光标静止不出帧时它**不推进**；`wall_ms` 是真实世界流逝的毫秒，中间睡了
    ///   3 秒，它就 +3000。所以"距上次某事件过了多久"这类判据（静止检测、超时淡出）
    ///   只能用 `wall_ms`，`elapsed` 只适合做逐帧动画的相位。
    ///
    /// 单独一个方法而不是塞进 `update_env`：后者有 6 个调用点（拖尾/点击效果都不需要
    /// 墙钟），加参数会白白波及它们。
    pub fn set_wall_ms(&self, wall_ms: f64) -> mlua::Result<()> {
        let g = self.lua.globals();
        g.set("wall_ms", wall_ms)?;
        // 真实本地时间：时钟类脚本不能拿 `wall_ms`（系统运行毫秒）当 Unix 时间。
        // `local_ms` 已含本地时区偏移，脚本直接除 1000 即可得到本地时分秒；
        // `unix_ms` / `tz_offset_min` 给需要自己算时区的脚本。
        let now = chrono::Local::now();
        let unix_ms = now.timestamp_millis();
        let tz_offset_min = now.offset().local_minus_utc() / 60;
        g.set("unix_ms", unix_ms)?;
        g.set("tz_offset_min", tz_offset_min)?;
        g.set("local_ms", unix_ms + i64::from(tz_offset_min) * 60_000)
    }

    /// 注入指针运动/按下状态（四类宿主一致）。
    ///
    /// 单位口径：`vx/vy/speed` 按 **60fps 的每帧像素**给出（与 CursorFX 的
    /// `state.vx/vy/speed` 同量纲，移植脚本不必再换算）；`heading` 是弧度。
    /// `mouse_down` / `mouse_button` 供按住态视觉（枪口后坐、F1 加速等）使用。
    #[allow(clippy::too_many_arguments)]
    pub fn set_pointer_state(
        &self,
        vx: f32,
        vy: f32,
        speed: f32,
        heading: f32,
        down: bool,
        button: &str,
    ) -> mlua::Result<()> {
        let g = self.lua.globals();
        g.set("cursor_vx", vx)?;
        g.set("cursor_vy", vy)?;
        g.set("cursor_speed", speed)?;
        g.set("cursor_heading", heading)?;
        // CursorFX 命名别名（脚本从原库直译时零改动）。
        g.set("mouse_vx", vx)?;
        g.set("mouse_vy", vy)?;
        g.set("mouse_speed", speed)?;
        g.set("mouse_heading", heading)?;
        g.set("mouse_down", down)?;
        g.set("mouse_button", button)?;
        // 悬停态：系统正在给出交互光标（I 形 / 手形 / 缩放 / 禁止 / 帮助 / 十字）。
        // 放在这里而不是 `update_env`：它与 vx/vy/down 同属"指针状态"，四类宿主
        // （拖尾/悬浮/点击/复合）都只经 `set_pointer_state` 注入，一处即齐。
        // 查询是微秒级、句柄表进程内缓存一次，按回调频率调用无负担。
        g.set(
            "cursor_hover",
            crate::platform::cursor_hint::is_interactive(),
        )?;
        Ok(())
    }

    /// 调用 Lua 端 `on_wheel(delta, horizontal, x, y, params)`；缺回调静默跳过。
    ///
    /// `delta` 是滚轮增量（Windows 一格 = ±120；正 = 上/右），`horizontal` = 倾斜滚轮。
    /// 与 `on_mouse_down` 同一条分发路径：四类宿主共用 `ScriptingEngine`，谁收到滚轮谁转发。
    pub fn call_on_wheel(
        &self,
        delta: i32,
        horizontal: bool,
        x: f32,
        y: f32,
        params: &HashMap<String, Json>,
    ) -> mlua::Result<()> {
        let globals = self.lua.globals();
        let tbl = self.build_params(params)?;
        globals.set("_params", tbl.clone())?;
        let f: mlua::Function = match globals.get("on_wheel") {
            Ok(f) => f,
            Err(_) => return Ok(()),
        };
        f.call((delta, horizontal, x, y, tbl))
    }

    /// 把 `HashMap<String, Json>` 转成 Lua table（number/string/bool）。
    fn build_params(&self, params: &HashMap<String, Json>) -> mlua::Result<mlua::Table> {
        let tbl = self.lua.create_table()?;
        for (k, v) in params {
            match v {
                Json::Number(n) => {
                    // ★ 先按整数试：这里过去是 `as_f64()` 优先，所以配置与 schema 里的整数
                    //   到脚本手里**永远**是浮点（`tostring` 出 `60.0`，`("%d"):format(...)`
                    //   只是"恰好整得尽"才没炸）。m17 的对拍格第一跑就在这儿翻红。
                    //   整数交整数、浮点交浮点：`type="int"` 的脚本这才拿到它声明的那一档。
                    if let Some(i) = n.as_i64() {
                        tbl.set(k.as_str(), i)?;
                    } else if let Some(f) = n.as_f64() {
                        tbl.set(k.as_str(), f)?;
                    }
                }
                Json::String(s) => tbl.set(k.as_str(), s.as_str())?,
                Json::Bool(b) => tbl.set(k.as_str(), *b)?,
                _ => {}
            }
        }
        Ok(tbl)
    }

    /// 调用 Lua 端 init() 函数。
    ///
    /// ★ `init` 是**可选**回调（2026-09-30）：只写 `schema` + `run` 的一次性 / 开窗脚本
    /// （如 `scripts/AI/ai_chat`）勾了卡片开关也会进来当常驻实例，宿主照旧逐帧分发回调；
    /// 缺这个函数是"它不关心"，不是脚本坏了 —— 旧实现用 `?` 把它当错误，于是日志刷
    /// `[复合脚本回调出错：error converting Lua nil to function]`。
    pub fn call_init(&self) -> mlua::Result<()> {
        let globals = self.lua.globals();
        let f: mlua::Function = match globals.get("init") {
            Ok(f) => f,
            Err(_) => return Ok(()),
        };
        f.call(())
    }

    /// 调用 Lua 端 on_move(x, y, t, params) 函数。
    ///
    /// ★ 可选回调：缺则静默跳过（理由见 [`Self::call_init`]）。
    pub fn call_on_move(
        &self,
        x: f32,
        y: f32,
        t: f32,
        params: &HashMap<String, Json>,
    ) -> mlua::Result<()> {
        let globals = self.lua.globals();
        let tbl = self.build_params(params)?;
        // 兼容读 `_params` 全局的老脚本（同 call_draw）。
        globals.set("_params", tbl.clone())?;
        let f: mlua::Function = match globals.get("on_move") {
            Ok(f) => f,
            Err(_) => return Ok(()),
        };
        f.call((x, y, t, tbl))
    }

    /// 调用 Lua 端 step(dt, t, params) 函数。
    ///
    /// ★ 可选回调：缺则静默跳过（理由见 [`Self::call_init`]）。
    pub fn call_step(&self, dt: f32, t: f32, params: &HashMap<String, Json>) -> mlua::Result<()> {
        let globals = self.lua.globals();
        let tbl = self.build_params(params)?;
        globals.set("_params", tbl.clone())?;
        let f: mlua::Function = match globals.get("step") {
            Ok(f) => f,
            Err(_) => return Ok(()),
        };
        f.call((dt, t, tbl))
    }

    /// 调用 Lua 端按下回调 `on_press(x, y, t, params)`（点击效果 / 拖尾 / 独立卡片共用）。
    ///
    /// ★ 四类宿主统一走这一个入口：**指针按下**时触发一次。脚本写 `on_press`（新口径）
    ///   或 `on_click`（老口径）都认——先找 `on_press`，找不到才回退 `on_click`，
    ///   所以既有的 11 个点击脚本一行都不用改。两个都没有则静默跳过（纯绘制类脚本很正常，
    ///   不是错误）。
    pub fn call_on_press(
        &self,
        x: f32,
        y: f32,
        t: f32,
        params: &HashMap<String, Json>,
    ) -> mlua::Result<()> {
        let globals = self.lua.globals();
        let tbl = self.build_params(params)?;
        globals.set("_params", tbl.clone())?;
        let f: mlua::Function = match globals.get("on_press") {
            Ok(f) => f,
            Err(_) => match globals.get("on_click") {
                Ok(f) => f,
                Err(_) => return Ok(()),
            },
        };
        f.call((x, y, t, tbl))
    }

    /// 调用 Lua 端 on_mouse_down(button, x, y, params) 函数（复合脚本宿主专用）。
    ///
    /// 与 [`Self::call_on_press`] 的 `on_press(x, y, t, params)` 是**两个不同的回调名**
    /// ——复合脚本关心"哪个键"，参数顺序是 (button, x, y)；刻意不同名，防止用户把
    /// 波纹脚本抄进复合宿主时按错参数位。
    pub fn call_on_button(
        &self,
        button: &str,
        x: f32,
        y: f32,
        t: f32,
        params: &HashMap<String, Json>,
    ) -> mlua::Result<()> {
        let globals = self.lua.globals();
        let tbl = self.build_params(params)?;
        globals.set("_params", tbl.clone())?;
        let f: mlua::Function = match globals.get("on_mouse_down") {
            Ok(f) => f,
            // ★ 统一契约（2026-10-26）：包卡/复合宿主也接受老口径 `on_press`。
            //   子脚本写 `on_press(x,y,t)` 时，复制到官方点击目录、放进包卡都能跑。
            Err(_) => return self.call_on_press(x, y, t, params),
        };
        f.call((button, x, y, tbl))
    }

    /// 调用 Lua 端 `on_mouse_up(button,x,y,params)`；缺回调静默跳过。
    pub fn call_on_mouse_up(
        &self,
        button: &str,
        x: f32,
        y: f32,
        params: &HashMap<String, Json>,
    ) -> mlua::Result<()> {
        let globals = self.lua.globals();
        let tbl = self.build_params(params)?;
        globals.set("_params", tbl.clone())?;
        let f: mlua::Function = match globals.get("on_mouse_up") {
            Ok(f) => f,
            Err(_) => return Ok(()),
        };
        f.call((button, x, y, tbl))
    }

    /// 调用 Lua 端 on_key(key, mods, repeated, params) 函数（击键音效专用）。
    ///
    /// - `key`：键名（与击键回显同源，`KeyCode::display_name`）；
    /// - ctrl/alt/shift/win：修饰键状态，打包成 `{ctrl=,alt=,shift=,win=}` 传入；
    /// - `params`：脚本 `schema()` 声明的参数现值（缺键时脚本侧 `params.x or 默认` 兜底）。
    ///
    /// 返回**原始** `mlua::Value` 由调用方解释（见 `modules/key_sound.rs` 的 `Pick`）：
    /// 字符串 = 音效路径；`false` = 强制静音（连宿主默认音也不响）；`nil` / `true` /
    /// 其它 = 落回宿主默认音。老脚本只声明三个形参也能兼容（Lua 自动忽略第 4 个实参）。
    /// 没有全局 `on_key` 时返回 Nil（调用方按"落回默认音"处理，不报错）。
    #[allow(clippy::too_many_arguments)]
    pub fn call_on_key(
        &self,
        key: &str,
        ctrl: bool,
        alt: bool,
        shift: bool,
        win: bool,
        repeated: bool,
        params: &HashMap<String, Json>,
    ) -> mlua::Result<mlua::Value> {
        let globals = self.lua.globals();
        let ptbl = self.build_params(params)?;
        // 兼容读 `_params` 全局的写法（与 call_draw 一致）。
        globals.set("_params", ptbl.clone())?;
        let mtbl = self.lua.create_table()?;
        mtbl.set("ctrl", ctrl)?;
        mtbl.set("alt", alt)?;
        mtbl.set("shift", shift)?;
        mtbl.set("win", win)?;
        let f: mlua::Function = match globals.get("on_key") {
            Ok(f) => f,
            Err(_) => return Ok(mlua::Value::Nil),
        };
        f.call((key, mtbl, repeated, ptbl))
    }

    /// 调用 Lua 端 draw(ctx, params) 函数。
    ///
    /// 调用前必须由 [`api_draw::set_draw_target`] 把本帧绘制目标指过去；这里把 `params`
    /// 同时写进 `_params` 全局（兼容读全局的老脚本）并作为 `draw` 的第二参数传入。
    ///
    /// ★ 返回值即脚本的**续帧请求**（悬浮标用，见 `modules::trail::hover_dot`）：
    ///   · 数字 n > 0 = "n 毫秒后再来一帧"（淡出动画靠它跑完，不受鼠标动不动影响）；
    ///   · 不返回 / 0 / 负数 / 其它类型 = 不需要额外帧（静止时零功耗，既有语义不变）。
    ///   老脚本不返回任何值 → `Nil`，落回"不请求"，完全兼容。
    pub fn call_draw(&self, params: &HashMap<String, Json>) -> mlua::Result<mlua::Value> {
        let globals = self.lua.globals();
        let tbl = self.build_params(params)?;
        globals.set("_params", tbl.clone())?;
        // 没有 draw 函数（例如纯逻辑特效）就静默跳过。
        let f: mlua::Function = match globals.get("draw") {
            Ok(f) => f,
            Err(_) => return Ok(mlua::Value::Nil),
        };
        // `DrawListCtx` 是 UserData 但没有 FromLua（非 Copy 值类型），走 AnyUserData 拿。
        let ctx: mlua::AnyUserData = match globals.get("ctx") {
            Ok(c) => c,
            Err(_) => return Ok(mlua::Value::Nil),
        };
        let v: mlua::Value = f.call((ctx, tbl))?;
        Ok(v)
    }

    /// 调用 Lua 端 schema() 函数，返回参数表。
    pub fn call_schema(&self) -> mlua::Result<mlua::Table> {
        let f: mlua::Function = self.lua.globals().get("schema")?;
        f.call(())
    }

    /// 统一入口 `run(ctx, params)`：结构化 ctx（含 `role`、`fs`、宿主字段）+ params。
    ///
    /// `ctx` = `{ role=<role>, fs=<FsApi 绑定脚本目录>, <host_fields...> }`；`ext` 仍走全局
    /// userdata（兼容老脚本的 `ext:...` 冒号调用）。参数同样写进 `_params` 全局。没有全局
    /// `run` → 返回 `Nil`。返回**原始** `mlua::Value` 由调用方解释（`runtime::EngineCache`
    /// 用它统一承载）。
    ///
    /// ★ 不写全局 `ctx`：`api_draw` 的绘图脚本用全局 `ctx`（DrawList userdata），run 类脚本
    ///   的 ctx 只是传给 `run` 的第一个实参，两者互不干扰。
    pub fn call_run_ctx(
        &self,
        role: &str,
        host_fields: &[(&str, mlua::Value)],
        params: &HashMap<String, Json>,
    ) -> mlua::Result<mlua::Value> {
        let globals = self.lua.globals();
        let ctx = self.lua.create_table()?;
        ctx.set("role", role)?;
        // fs 绑定到脚本所在目录（load_script 后 = 脚本子目录）：read / read_bytes / exists / list。
        ctx.set(
            "fs",
            self.lua
                .create_userdata(super::fs_api::FsApi::new(self.fs_base.borrow().clone()))?,
        )?;
        // ext 仍是全局 userdata；顺手在 ctx 里也放一份引用，脚本 `ctx.ext:xxx()` 也行。
        let ext: mlua::Value = globals.get("ext")?;
        ctx.set("ext", ext)?;
        for (k, v) in host_fields {
            ctx.set(*k, v.clone())?;
        }
        let tbl = self.build_params(params)?;
        globals.set("_params", tbl.clone())?;
        let f: mlua::Function = match globals.get("run") {
            Ok(f) => f,
            Err(_) => return Ok(mlua::Value::Nil),
        };
        f.call((ctx, tbl))
    }

    /// 调用 Lua 端 defaults() 函数，返回默认参数表（兼容未迁移到 schema() 的老脚本）。
    pub fn call_defaults(&self) -> mlua::Result<mlua::Table> {
        let f: mlua::Function = self.lua.globals().get("defaults")?;
        f.call(())
    }

    /// 从 Lua 端获取 effect_name 字符串。
    pub fn get_effect_name(&self) -> mlua::Result<String> {
        self.lua.globals().get("effect_name")
    }

    /// 从 Lua 端读取 `hide_cursor` 全局布尔（脚本声明自己要隐藏系统指针时设为 `true`）。
    ///
    /// 不存在 / 非布尔 → `false`（默认不隐藏）。读取必须在 `load_script` 之后——脚本顶层
    /// 的 `hide_cursor = true` 是在执行期写进 globals 的。
    pub fn get_hide_cursor(&self) -> bool {
        self.lua.globals().get("hide_cursor").unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 写一个最小脚本到临时目录并加载，返回引擎。
    fn engine_with_script(body: &str) -> ScriptingEngine {
        let dir = std::env::temp_dir().join(format!(
            "curseen_engine_test_{}_{}",
            std::process::id(),
            body.len()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("scr.lua"), body).unwrap();
        let eng = ScriptingEngine::new(&dir).unwrap();
        eng.load_script("scr").unwrap();
        eng
    }

    /// ★ ctx.fs 必须绑定**脚本所在目录**，不是引擎根目录：`scripts/AI/ai_chat` 这类
    /// 子目录脚本靠它读同目录的资料文件（ai_chat_api.txt）。2026-09-30 实测根因：
    /// 绑成引擎根目录 → `fs:read("./ai_chat_api.txt")` 返回 nil → 系统提示词里
    /// 注入了"速查文件缺失"，AI 如实回答"没有文档"。
    #[test]
    fn ctx_fs_binds_to_loaded_script_dir() {
        let dir =
            std::env::temp_dir().join(format!("curseen_engine_fs_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::write(dir.join("scripts").join("side.txt"), "资料内容").unwrap();
        std::fs::write(
            dir.join("scripts").join("sub.lua"),
            "function run(ctx)\n  return ctx.fs:read('./side.txt')\nend\n",
        )
        .unwrap();
        let eng = ScriptingEngine::new(&dir).unwrap();
        eng.load_script("scripts/sub").unwrap();
        let v = eng.call_run_ctx("image", &[], &HashMap::new()).unwrap();
        assert_eq!(
            v.as_string().map(|s| s.to_string_lossy().to_owned()),
            Some("资料内容".to_string()),
            "fs:read 必须以脚本所在目录为基准"
        );
    }

    /// on_key 第 4 参拿到 params；false 返回值原样透传（静音契约）。
    #[test]
    fn on_key_receives_params_and_false_contract() {
        let eng = engine_with_script(concat!(
            "function on_key(key, mods, repeated, params)\n",
            "  if params.quiet then return false end\n",
            "  if mods.ctrl then return params.prefix .. key .. ':ctrl' end\n",
            "  return params.prefix .. key\n",
            "end\n",
        ));
        let mut params = HashMap::new();
        params.insert("quiet".to_string(), Json::Bool(false));
        params.insert("prefix".to_string(), Json::String("hit:".into()));
        let v = eng
            .call_on_key("A", false, false, false, false, false, &params)
            .unwrap();
        assert_eq!(
            v.as_string().map(|s| s.to_string_lossy().to_owned()),
            Some("hit:A".to_string())
        );
        // 修饰键表 + 同一份 params 表也能从全局 `_params` 读（兼容老写法）。
        let v = eng
            .call_on_key("A", true, false, false, false, false, &params)
            .unwrap();
        assert_eq!(
            v.as_string().map(|s| s.to_string_lossy().to_owned()),
            Some("hit:A:ctrl".to_string())
        );
        // false = 强制静音的契约值，必须原样透传给调用方。
        params.insert("quiet".to_string(), Json::Bool(true));
        let v = eng
            .call_on_key("A", false, false, false, false, true, &params)
            .unwrap();
        assert_eq!(v, mlua::Value::Boolean(false));
    }

    #[test]
    fn hide_cursor_true_is_detected() {
        let eng = engine_with_script("effect_name=\"激光笔\"\nhide_cursor = true\n");
        assert!(eng.get_hide_cursor(), "声明 hide_cursor=true 应被读到");
    }

    #[test]
    fn hide_cursor_absent_defaults_false() {
        let eng = engine_with_script("effect_name=\"实心圆\"\n");
        assert!(
            !eng.get_hide_cursor(),
            "未声明 hide_cursor 默认 false（普通特效不该藏指针）"
        );
    }

    #[test]
    fn hide_cursor_false_is_detected() {
        let eng = engine_with_script("effect_name=\"x\"\nhide_cursor = false\n");
        assert!(!eng.get_hide_cursor());
    }

    /// AI 速查里的三段完整示例必须在**真引擎**里加载并跑通回调——示例是给 AI 抄的
    /// 模板，模板本身跑不通等于教 AI 写坏脚本（2026-09-30 用户要求补示例时一并钉住；
    /// 2026-10-31 补悬浮标“静止 N 秒”示例后改为三段，并允许延迟出画的示例先等时间）。
    /// 抽取规则：官方包 `Packs/AI/scripts/ai_chat_api.txt` 里的 ```lua 围栏块，应恰好三段。
    #[test]
    fn ai_reference_examples_run_in_the_real_engine() {
        let txt = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../CurSeen-Scripts/Packs/AI/scripts/ai_chat_api.txt"
        ))
        .expect("ai_chat_api.txt 读不到");
        let mut blocks: Vec<String> = Vec::new();
        let mut cur: Option<Vec<&str>> = None;
        for line in txt.lines() {
            let t = line.trim();
            if t.starts_with("```lua") {
                cur = Some(Vec::new());
            } else if t == "```" {
                if let Some(b) = cur.take() {
                    blocks.push(b.join("\n"));
                }
            } else if let Some(b) = &mut cur {
                b.push(line);
            }
        }
        assert_eq!(
            blocks.len(),
            3,
            "速查应恰好三段完整示例（拖尾 + 复合 + 悬浮标），实得 {}",
            blocks.len()
        );

        let dir =
            std::env::temp_dir().join(format!("curseen_ai_ref_example_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let params: HashMap<String, Json> = HashMap::from([
            ("size".to_string(), Json::from(10.0f64)),
            ("fade".to_string(), Json::from(1.2f64)),
            ("life".to_string(), Json::from(0.6f64)),
            ("delay".to_string(), Json::from(1.0f64)),
            ("radius".to_string(), Json::from(18.0f64)),
            ("color".to_string(), Json::String("#7FD4FFCC".into())),
        ]);

        for (i, body) in blocks.iter().enumerate() {
            let name = format!("ex{i}");
            std::fs::write(dir.join(format!("{name}.lua")), body).unwrap();
            let eng = ScriptingEngine::new(&dir).unwrap();
            eng.load_script(&name)
                .unwrap_or_else(|e| panic!("示例 {i} 加载失败：{e}"));
            let schema = eng.call_schema().unwrap();
            assert!(
                schema.raw_len() >= 1,
                "示例 {i} 的 schema() 应返回非空参数表"
            );
            eng.set_wall_ms(10_000.0).unwrap();
            // 有 init 就调（示例 2 没写 init，与宿主同口径静默跳过）。
            if let Ok(Some(f)) = eng.lua().globals().get::<Option<mlua::Function>>("init") {
                f.call::<()>(()).unwrap();
            }
            eng.update_env((120.0, 90.0), (120.0, 90.0), 0.4, (1920.0, 1080.0), "trail")
                .unwrap();
            // 各回调真跑一遍（缺失的静默跳过 = 宿主同口径）。
            let _ = eng.call_on_move(120.0, 90.0, 0.3, &params);
            let _ = eng.call_on_button("middle", 120.0, 90.0, 0.3, &params);
            let _ = eng.call_step(0.016, 0.316, &params);
            let draw = |eng: &ScriptingEngine| {
                let mut list = crate::core::draw::DrawList::new();
                crate::scripting::api_draw::set_draw_target(&mut list);
                let v = eng
                    .call_draw(&params)
                    .unwrap_or_else(|e| panic!("示例 {i} 的 draw 跑挂：{e}"));
                crate::scripting::api_draw::clear_draw_target();
                let frames = match v {
                    mlua::Value::Integer(n) => n,
                    mlua::Value::Number(n) => n as i64,
                    _ => 0,
                };
                (frames, list.len_all())
            };
            let (frames, mut drawn) = draw(&eng);
            // 悬浮标示例要等“静止 N 秒”才出画，且必须靠 draw 返回值续帧。
            if drawn == 0 && body.contains("still_ms") {
                assert!(frames > 0, "示例 {i} 必须请求续帧，否则等不到时间");
                eng.set_wall_ms(12_000.0).unwrap();
                let _ = eng.call_step(0.016, 2.0, &params);
                let (frames2, drawn2) = draw(&eng);
                assert!(frames2 > 0, "示例 {i} 到时间后仍要续帧");
                drawn = drawn2;
            }
            assert!(drawn > 0, "示例 {i} 的 draw 应产出至少一条绘图指令");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// CursorFX 包：42 个脚本都必须能被 CurSeen 的真实引擎加载，并走完一遍
    /// `init → on_move → on_press → step → draw`。
    ///
    /// 这不是视觉验收，而是 API 契约门禁：脚本用了未开放的 `ctx:*` / 全局，
    /// 或 `draw` 里运行期报错，都会在这里红。
    #[test]
    fn cursorfx_scripts_all_load_and_draw() {
        let root = crate::core::paths::lua_ext_dir();
        let pack = root.join("scripts").join("CursorFX");
        let mut files = std::fs::read_dir(&pack)
            .expect("CursorFX 包目录应存在")
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                (p.extension().and_then(|x| x.to_str()) == Some("lua"))
                    .then(|| p.file_stem()?.to_str().map(str::to_string))
                    .flatten()
            })
            .collect::<Vec<_>>();
        files.sort();
        assert_eq!(files.len(), 42, "CursorFX 应有 42 个效果脚本");

        let params = HashMap::new();
        for stem in files {
            let id = format!("scripts/CursorFX/{stem}");
            let eng = ScriptingEngine::new(&root).expect("引擎初始化失败");
            eng.load_script(&id)
                .unwrap_or_else(|e| panic!("{id} 加载失败：{e}"));
            let _ = eng.call_schema();
            eng.set_wall_ms(50_000.0).unwrap();
            eng.call_init().unwrap();
            // 走 10 个移动帧：拖尾/粒子要累积到能画；点击脚本则随后补按下。
            for i in 0..10 {
                let t = 0.5 + i as f32 / 60.0;
                let x = 400.0 + i as f32 * 4.0;
                eng.update_env((x, 300.0), (400.0, 300.0), t, (1920.0, 1080.0), "panel")
                    .unwrap();
                eng.set_pointer_state(4.0, 0.0, 4.0, 0.0, true, "left")
                    .unwrap();
                eng.call_on_move(x, 300.0, t, &params)
                    .unwrap_or_else(|e| panic!("{id} on_move 失败：{e}"));
                eng.call_step(1.0 / 120.0, t, &params)
                    .unwrap_or_else(|e| panic!("{id} step 失败：{e}"));
            }
            eng.call_on_press(436.0, 300.0, 0.67, &params)
                .unwrap_or_else(|e| panic!("{id} on_press 失败：{e}"));
            eng.call_step(1.0 / 120.0, 0.67, &params)
                .unwrap_or_else(|e| panic!("{id} step 失败：{e}"));
            eng.update_env(
                (436.0, 300.0),
                (400.0, 300.0),
                0.67,
                (1920.0, 1080.0),
                "panel",
            )
            .unwrap();
            eng.set_pointer_state(4.0, 0.0, 4.0, 0.0, true, "left")
                .unwrap();
            let mut list = crate::core::draw::DrawList::new();
            crate::scripting::api_draw::set_draw_target(&mut list);
            eng.call_draw(&params)
                .unwrap_or_else(|e| panic!("{id} draw 失败：{e}"));
            crate::scripting::api_draw::clear_draw_target();
            assert!(list.len_all() > 0, "{id} draw 未产出任何图元");
        }
    }

    /// CursorX 包：24 个脚本都必须能被真实引擎加载，并走完
    /// `init → 移动 → 按下 → 抬起 → step → draw`。
    ///
    /// 与 CursorFX 同款契约门禁：这里挡的是"用了没开放的 ctx/全局"和 Lua 语法错误；
    /// 视觉验收另有真机/离屏探针。`Difference Blend` / `Fluid Glass` 在测试进程里
    /// 找不到自己拥有的覆盖层窗口，`backdrop_circle` 返回 false，走静态回退分支——
    /// 这条回退也必须能画出东西（脚本不能只在抓屏成功时才可见）。
    #[test]
    fn cursorx_scripts_all_load_and_draw() {
        let root = crate::core::paths::lua_ext_dir();
        let pack = root.join("scripts").join("CursorX");
        let mut files = std::fs::read_dir(&pack)
            .expect("CursorX 包目录应存在")
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                (p.extension().and_then(|x| x.to_str()) == Some("lua"))
                    .then(|| p.file_stem()?.to_str().map(str::to_string))
                    .flatten()
            })
            .collect::<Vec<_>>();
        files.sort();
        assert_eq!(files.len(), 24, "CursorX 应有 24 个效果脚本");

        // 悬停分支是这包最常见的视觉分叉；真实光标位置不可控，测试里强制两种取值各跑一遍。
        struct HoverReset;
        impl Drop for HoverReset {
            fn drop(&mut self) {
                crate::platform::cursor_hint::force_for_test(None);
            }
        }
        let _hover_reset = HoverReset;

        let params = HashMap::new();
        for stem in files {
            for hover in [false, true] {
                crate::platform::cursor_hint::force_for_test(Some(hover));
                let id = format!("scripts/CursorX/{stem}");
                let eng = ScriptingEngine::new(&root).expect("引擎初始化失败");
                eng.load_script(&id)
                    .unwrap_or_else(|e| panic!("{id} 加载失败：{e}"));
                let schema = eng.call_schema();
                assert!(schema.is_ok(), "{id} schema() 报错");
                eng.set_wall_ms(50_000.0).unwrap();
                eng.call_init().unwrap();
                for i in 0..10 {
                    let t = 0.5 + i as f32 / 60.0;
                    let x = 400.0 + i as f32 * 4.0;
                    eng.update_env((x, 300.0), (400.0, 300.0), t, (1920.0, 1080.0), "panel")
                        .unwrap();
                    eng.set_pointer_state(4.0, 0.0, 4.0, 0.0, false, "left")
                        .unwrap();
                    eng.call_on_move(x, 300.0, t, &params)
                        .unwrap_or_else(|e| panic!("{id} on_move 失败：{e}"));
                    eng.call_step(1.0 / 120.0, t, &params)
                        .unwrap_or_else(|e| panic!("{id} step 失败：{e}"));
                }
                eng.call_on_button("left", 436.0, 300.0, 0.67, &params)
                    .unwrap_or_else(|e| panic!("{id} on_mouse_down 失败：{e}"));
                eng.call_on_mouse_up("left", 436.0, 300.0, &params)
                    .unwrap_or_else(|e| panic!("{id} on_mouse_up 失败：{e}"));
                eng.update_env(
                    (436.0, 300.0),
                    (400.0, 300.0),
                    0.70,
                    (1920.0, 1080.0),
                    "panel",
                )
                .unwrap();
                eng.set_pointer_state(4.0, 0.0, 4.0, 0.0, false, "left")
                    .unwrap();
                eng.call_step(1.0 / 120.0, 0.70, &params)
                    .unwrap_or_else(|e| panic!("{id} step 失败：{e}"));
                let mut list = crate::core::draw::DrawList::new();
                crate::scripting::api_draw::set_draw_target(&mut list);
                eng.call_draw(&params)
                    .unwrap_or_else(|e| panic!("{id} draw 失败：{e}"));
                crate::scripting::api_draw::clear_draw_target();
                assert!(
                    list.len_all() > 0,
                    "{id}（hover={hover}）draw 未产出任何图元"
                );
            }
        }
    }

    /// ★ 参数有效性门禁（2026-10-27）：`schema()` 里声明的每个 key，脚本源码里都必须
    /// 至少出现一次 `params.<key>`。这条挡的是"参数摆出来但根本没人读"——用户实测
    /// Neon Glow 的颜色没反应，根因就是点击后色环永远轮不回参数色；而 `pointerAnim`
    /// 这类**完全没读**的死参数，靠这条静态对账直接拦住。
    /// 包内每个 `schema()` 键都必须在源码里出现 `params.<key>`。
    fn assert_pack_schema_keys_read(pack_name: &str, min_keys: usize) {
        let root = crate::core::paths::lua_ext_dir();
        let pack = root.join("scripts").join(pack_name);
        let mut checked = 0usize;
        for entry in std::fs::read_dir(&pack)
            .unwrap_or_else(|e| panic!("{pack_name} 包目录不可读：{e}"))
            .flatten()
        {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) != Some("lua") {
                continue;
            }
            let stem = p
                .file_stem()
                .and_then(|s| s.to_str())
                .expect("lua 文件名应是 UTF-8")
                .to_string();
            let eng = ScriptingEngine::new(&root).expect("引擎初始化失败");
            eng.load_script(&format!("scripts/{pack_name}/{stem}"))
                .unwrap_or_else(|e| panic!("{stem} 加载失败：{e}"));
            let keys: Vec<String> = eng
                .lua()
                .load(
                    r#"
                    local out = {}
                    for _, item in ipairs(schema()) do
                        if item.key and item.key ~= "" then out[#out + 1] = item.key end
                    end
                    return out
                    "#,
                )
                .eval()
                .unwrap_or_else(|e| panic!("{stem} schema() 解析失败：{e}"));
            let src = std::fs::read_to_string(&p).expect("读脚本失败");
            for key in keys {
                assert!(
                    src.contains(&format!("params.{key}")),
                    "{pack_name}/{stem}: 参数 {key} 声明了但脚本从未读取"
                );
                checked += 1;
            }
        }
        assert!(
            checked >= min_keys,
            "{pack_name} 参数审计应覆盖至少 {min_keys} 个键（防止扫描空跑），实际 {checked}"
        );
    }

    #[test]
    fn cursorx_schema_keys_are_read_by_script() {
        assert_pack_schema_keys_read("CursorX", 150);
    }

    /// MFCMouseEffect 包同样做"声明必被读"对账（132 个键）。
    #[test]
    fn mfcmouse_schema_keys_are_read_by_script() {
        assert_pack_schema_keys_read("MFCMouseEffect", 120);
    }

    /// CursorFX 包同样做"声明必被读"对账（197 个键）。
    #[test]
    fn cursorfx_schema_keys_are_read_by_script() {
        assert_pack_schema_keys_read("CursorFX", 180);
    }

    /// MouseEffects 包做"声明必被读"对账（236 个键）。
    #[test]
    fn mouseeffects_schema_keys_are_read_by_script() {
        assert_pack_schema_keys_read("MouseEffects", 210);
    }

    /// 颜色 alpha 的默认值必须是 true；只有显式 `alpha = false` 才关闭。
    /// 旧实现用 `get::<bool>().unwrap_or(true)`，mlua 会把缺失键的 Nil 转成 false，
    /// 导致所有颜色选择器都没有透明度（2026-10-27 用户反馈）。
    #[test]
    fn color_alpha_defaults_true_unless_explicitly_false() {
        let eng = engine_with_script(concat!(
            "function schema()
",
            "  return {
",
            "    { key = 'a', type = 'color', default = '#FF0000' },
",
            "    { key = 'b', type = 'color', alpha = false, default = '#00FF00' },
",
            "  }
",
            "end
",
        ));
        let schema = crate::scripting::registry::parse_schema_table(
            &eng.call_schema().expect("schema() 失败"),
        );
        assert!(schema[0].alpha, "未写 alpha 的颜色应默认带透明度");
        assert!(!schema[1].alpha, "显式 alpha=false 应保持关闭");
    }

    /// MouseEffects 的颜色参数必须带透明度（`alpha=true`）：本包脚本已把颜色 alpha
    /// 乘进输出。用户反馈"颜色选择器没有透明度"就是这里被 `alpha=false` 关掉过。
    #[test]
    fn mouseeffects_colors_support_alpha() {
        use crate::scripting::registry::{parse_schema_table, ParamType};
        let root = crate::core::paths::lua_ext_dir();
        let pack = root.join("scripts").join("MouseEffects");
        let mut colors = 0usize;
        for entry in std::fs::read_dir(&pack)
            .expect("MouseEffects 包目录应存在")
            .flatten()
        {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) != Some("lua") {
                continue;
            }
            let stem = p
                .file_stem()
                .and_then(|s| s.to_str())
                .expect("lua 文件名应是 UTF-8");
            let eng = ScriptingEngine::new(&root).expect("引擎初始化失败");
            eng.load_script(&format!("scripts/MouseEffects/{stem}"))
                .unwrap_or_else(|e| panic!("{stem} 加载失败：{e}"));
            let schema = parse_schema_table(&eng.call_schema().expect("schema() 失败"));
            for def in schema {
                if def.ptype == ParamType::Color {
                    colors += 1;
                    assert!(
                        def.alpha,
                        "MouseEffects/{stem}/{}: 颜色参数必须带透明度（alpha=true）",
                        def.key
                    );
                }
            }
        }
        assert!(
            colors >= 35,
            "MouseEffects 颜色参数应至少 35 个，实际 {colors}"
        );
    }

    /// 三个包的 schema 机械体检：数值区间 / 默认值 / 步进、choice 默认值、颜色格式、
    /// 同文件重复 key。挡的是"默认值落在滑条外""min>max""step=0"这类手工写 schema
    /// 时最容易犯的错——它们不会让脚本报错，只会让 UI 的滑条与脚本行为对不上。
    #[test]
    fn all_pack_schemas_have_sane_ranges() {
        use crate::scripting::registry::{parse_schema_table, ParamType};
        let root = crate::core::paths::lua_ext_dir();
        let mut total = 0usize;
        for pack in ["CursorFX", "CursorX", "MFCMouseEffect", "MouseEffects"] {
            let dir = root.join("scripts").join(pack);
            for entry in std::fs::read_dir(&dir)
                .unwrap_or_else(|e| panic!("{pack} 目录不可读：{e}"))
                .flatten()
            {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) != Some("lua") {
                    continue;
                }
                let stem = p
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .expect("lua 文件名应是 UTF-8");
                let eng = ScriptingEngine::new(&root).expect("引擎初始化失败");
                eng.load_script(&format!("scripts/{pack}/{stem}"))
                    .unwrap_or_else(|e| panic!("{pack}/{stem} 加载失败：{e}"));
                let schema = parse_schema_table(&eng.call_schema().expect("schema() 失败"));
                let mut seen = std::collections::HashSet::new();
                for def in &schema {
                    assert!(
                        seen.insert(def.key.clone()),
                        "{pack}/{stem}: 参数 key `{}` 重复",
                        def.key
                    );
                    match def.ptype {
                        ParamType::Int | ParamType::Float => {
                            assert!(
                                def.min < def.max,
                                "{pack}/{stem}/{}: min {} 应小于 max {}",
                                def.key,
                                def.min,
                                def.max
                            );
                            assert!(
                                def.step > 0.0,
                                "{pack}/{stem}/{}: step 必须 > 0，实际 {}",
                                def.key,
                                def.step
                            );
                            let d = def.default.as_f64().unwrap_or_else(|| {
                                panic!("{pack}/{stem}/{}: 数值参数缺少数值 default", def.key)
                            });
                            assert!(
                                d >= def.min as f64 - 1e-6 && d <= def.max as f64 + 1e-6,
                                "{pack}/{stem}/{}: default {} 不在 [{}, {}]",
                                def.key,
                                d,
                                def.min,
                                def.max
                            );
                            // UI 把滑块吸附到 min+n×step；默认值若不在网格上，面板读数
                            // 会与脚本实际读到的默认值差半格（旧参数里有 17 个）。
                            let n = ((d - def.min as f64) / def.step as f64).round();
                            let snapped = def.min as f64 + n * def.step as f64;
                            assert!(
                                (d - snapped).abs() <= (def.step as f64) * 1e-3 + 1e-9,
                                "{pack}/{stem}/{}: default {d} 不在 min+step 网格上（最近值 {snapped}）",
                                def.key
                            );
                        }
                        ParamType::Bool => assert!(
                            def.default.is_boolean(),
                            "{pack}/{stem}/{}: bool 参数 default 不是布尔",
                            def.key
                        ),
                        ParamType::Choice => {
                            let d = def.default.as_str().unwrap_or_else(|| {
                                panic!("{pack}/{stem}/{}: choice default 不是字符串", def.key)
                            });
                            assert!(
                                def.choices.iter().any(|c| c.as_str() == d),
                                "{pack}/{stem}/{}: default `{d}` 不在 choices {:?}",
                                def.key,
                                def.choices
                            );
                        }
                        ParamType::Color => {
                            let d = def.default.as_str().unwrap_or_else(|| {
                                panic!("{pack}/{stem}/{}: color default 不是字符串", def.key)
                            });
                            let ok = (d.len() == 7 || d.len() == 9)
                                && d.starts_with('#')
                                && d[1..].chars().all(|c| c.is_ascii_hexdigit());
                            assert!(
                                ok,
                                "{pack}/{stem}/{}: color default `{d}` 应为 #RRGGBB[AA]",
                                def.key
                            );
                        }
                        _ => {}
                    }
                    total += 1;
                }
            }
        }
        assert!(
            total >= 700,
            "四个包的参数体检应覆盖至少 700 个键，实际 {total}"
        );
    }

    /// 测试用：加载 CursorX 脚本并注入一次指针状态（悬停值可强制）。
    fn prep_cursorx(stem: &str, hover: bool, params: &HashMap<String, Json>) -> ScriptingEngine {
        let root = crate::core::paths::lua_ext_dir();
        let id = format!("scripts/CursorX/{stem}");
        let eng = ScriptingEngine::new(&root).expect("引擎初始化失败");
        eng.load_script(&id)
            .unwrap_or_else(|e| panic!("{id} 加载失败：{e}"));
        eng.set_wall_ms(50_000.0).unwrap();
        eng.call_init().unwrap();
        eng.update_env(
            (400.0, 300.0),
            (400.0, 300.0),
            0.5,
            (1920.0, 1080.0),
            "panel",
        )
        .unwrap();
        crate::platform::cursor_hint::force_for_test(Some(hover));
        eng.set_pointer_state(4.0, 0.0, 4.0, 0.0, false, "left")
            .unwrap();
        crate::platform::cursor_hint::force_for_test(None);
        eng.call_step(1.0 / 120.0, 0.5, params).unwrap();
        eng
    }

    fn draw_cursorx(
        eng: &ScriptingEngine,
        params: &HashMap<String, Json>,
    ) -> Vec<crate::core::draw::DrawCmd> {
        let mut list = crate::core::draw::DrawList::new();
        crate::scripting::api_draw::set_draw_target(&mut list);
        eng.call_draw(params).expect("draw 失败");
        crate::scripting::api_draw::clear_draw_target();
        // 必须用 ordered()：`Difference Blend` / `Fluid Glass` 用 ctx:layer("top")，
        // 只读 cmds() 会把整层顶层指令漏掉，测试就变成"空跑也通过"。
        list.ordered().cloned().collect()
    }

    fn paint_colors(paint: &crate::core::draw::Paint) -> Vec<crate::core::color::Argb> {
        use crate::core::draw::Paint;
        match paint {
            Paint::Solid(c, _) => vec![*c],
            Paint::Linear { stops, .. } | Paint::Radial { stops, .. } => {
                stops.iter().map(|s| s.color).collect()
            }
        }
    }

    fn commands_have_rgb(cmds: &[crate::core::draw::DrawCmd], r: u8, g: u8, b: u8) -> bool {
        use crate::core::draw::DrawCmd;
        cmds.iter().any(|c| match c {
            DrawCmd::Fill { paint, .. } | DrawCmd::Stroke { paint, .. } => paint_colors(paint)
                .iter()
                .any(|col| col.a() > 0 && col.r() == r && col.g() == g && col.b() == b),
            _ => false,
        })
    }

    fn max_white_circle_radius(cmds: &[crate::core::draw::DrawCmd]) -> f32 {
        use crate::core::draw::{DrawCmd, Paint, Shape};
        cmds.iter()
            .filter_map(|c| match c {
                DrawCmd::Fill {
                    shape: Shape::Circle { radius, .. },
                    paint: Paint::Solid(col, _),
                    ..
                } if col.r() == 255 && col.g() == 255 && col.b() == 255 && col.a() > 200 => {
                    Some(*radius)
                }
                _ => None,
            })
            .fold(0.0, f32::max)
    }

    fn count_white_strokes(cmds: &[crate::core::draw::DrawCmd]) -> usize {
        use crate::core::draw::{DrawCmd, Paint};
        cmds.iter()
            .filter(|c| match c {
                DrawCmd::Stroke {
                    paint: Paint::Solid(col, _),
                    ..
                } => col.r() == 255 && col.g() == 255 && col.b() == 255 && col.a() > 0,
                _ => false,
            })
            .count()
    }

    fn has_circle_alpha(cmds: &[crate::core::draw::DrawCmd], r: u8, g: u8, b: u8, a: u8) -> bool {
        use crate::core::draw::{DrawCmd, Paint, Shape};
        cmds.iter().any(|c| match c {
            DrawCmd::Fill {
                shape: Shape::Circle { .. },
                paint: Paint::Solid(col, _),
                ..
            } => col.r() == r && col.g() == g && col.b() == b && col.a() == a,
            _ => false,
        })
    }

    /// 点击换色的两个脚本（Neon Glow / Morphing Blob）必须能轮回到「参数里的颜色」，
    /// 且改色参数要立刻生效。用户实测「霓虹颜色没用」就是前者坏了：旧实现只从 1
    /// 开始循环、永远回不到 0，第一次点击后参数色就永久失效。
    #[test]
    fn cursorx_click_cycle_returns_to_configured_color() {
        for stem in ["neon_glow", "morphing_blob"] {
            let mut params = HashMap::new();
            params.insert("color".to_string(), serde_json::json!("#FF0000"));
            let eng = prep_cursorx(stem, false, &params);
            let initial = draw_cursorx(&eng, &params);
            assert!(
                commands_have_rgb(&initial, 255, 0, 0),
                "{stem}: 初始应使用参数里的红色"
            );

            eng.call_on_button("left", 400.0, 300.0, 0.8, &params)
                .unwrap();
            let cycled = draw_cursorx(&eng, &params);
            assert!(
                !commands_have_rgb(&cycled, 255, 0, 0),
                "{stem}: 点击后应切到预设色（不再用参数红）"
            );

            params.insert("color".to_string(), serde_json::json!("#00FF00"));
            let reset = draw_cursorx(&eng, &params);
            assert!(
                commands_have_rgb(&reset, 0, 255, 0),
                "{stem}: 改色参数后应立即生效（重置回参数色）"
            );
        }
    }

    /// Glitch Shift 的 `pointerAnim` 曾经完全没读；这里用白点半径证明它真的 gate 了
    /// 悬停放大：关 = 约 6.5px，开 + 2 倍 = 约 13px。
    #[test]
    fn cursorx_glitch_pointer_anim_gates_size() {
        let mut sizes = Vec::new();
        for flag in [false, true] {
            let mut params = HashMap::new();
            params.insert("pointerAnim".to_string(), serde_json::json!(flag));
            params.insert("pointerSizeMult".to_string(), serde_json::json!(2.0));
            let eng = prep_cursorx("glitch_shift", true, &params);
            for i in 0..60 {
                eng.call_step(1.0 / 60.0, 1.0 + i as f32 / 60.0, &params)
                    .unwrap();
            }
            sizes.push(max_white_circle_radius(&draw_cursorx(&eng, &params)));
        }
        assert!(
            sizes[1] > 10.0,
            "悬停 + pointerAnim 开时白点半径应收敛到约 13，实际 {}",
            sizes[1]
        );
        assert!(
            sizes[0] < 8.0,
            "pointerAnim=false 时白点半径应保持约 6.5，实际 {}",
            sizes[0]
        );
    }

    /// 颜色 alpha 与 rim 参数必须真的改变输出：
    /// - Fluid Glass `rimStrength=0` 不画白圈，=1 画一圈；
    /// - Difference Blend 的球色 alpha 参与实心球合成；
    /// - Audio Pulse 的点色 alpha 参与光点合成。
    #[test]
    fn cursorx_color_alpha_and_rim_params_take_effect() {
        for (rim, want) in [(0.0, 0usize), (1.0, 1usize)] {
            let mut params = HashMap::new();
            params.insert("rimStrength".to_string(), serde_json::json!(rim));
            let eng = prep_cursorx("fluid_glass", false, &params);
            assert_eq!(
                count_white_strokes(&draw_cursorx(&eng, &params)),
                want,
                "Fluid Glass rimStrength={rim} 时白圈数量不符"
            );
        }

        let mut pd = HashMap::new();
        pd.insert("color".to_string(), serde_json::json!("#F7F8FA40"));
        let engd = prep_cursorx("difference_blend", false, &pd);
        assert!(
            has_circle_alpha(&draw_cursorx(&engd, &pd), 247, 248, 250, 58),
            "Difference Blend 球色 alpha=64 时，实心球 alpha 应为 floor(64*235/255)=58"
        );

        let mut pa = HashMap::new();
        pa.insert("dotColor".to_string(), serde_json::json!("#00E5FF40"));
        let enga = prep_cursorx("audio_pulse", false, &pa);
        assert!(
            has_circle_alpha(&draw_cursorx(&enga, &pa), 0, 229, 255, 64),
            "Audio Pulse 点色 alpha=64 时，光点 alpha 应为 64"
        );
    }

    /// 把一帧脚本输出经**生产渲染层**画进 RGBA 缓冲，返回一个像素。
    ///
    /// 光点类效果的颜色只有落到像素上才算数；只查 `DrawCmd` 颜色列表会漏掉
    /// "命令里有这个颜色、但被后续绘制盖掉/根本没画出来"的情况。
    fn render_cursorx_pixel(
        eng: &ScriptingEngine,
        params: &HashMap<String, Json>,
        w: u32,
        h: u32,
        px: u32,
        py: u32,
    ) -> (u8, u8, u8, u8) {
        let mut list = crate::core::draw::DrawList::new();
        crate::scripting::api_draw::set_draw_target(&mut list);
        eng.call_draw(params).expect("draw 失败");
        crate::scripting::api_draw::clear_draw_target();

        let mut buf = vec![0_u8; (w * h * 4) as usize];
        {
            use windui::render::{DibTarget, RenderTarget};
            let mut target = DibTarget::new(&mut buf, w, h);
            let mut engine = windui::text::DWriteEngine::new();
            let mut canvas = target.make_canvas(&mut engine, 1.0);
            crate::runtime::render::translate_offset(&list, &mut *canvas, 1.0, 0.0, 0.0);
        }
        let i = ((py * w + px) * 4) as usize;
        (buf[i], buf[i + 1], buf[i + 2], buf[i + 3])
    }

    /// **像素级**证明 Neon Glow 的颜色参数真的到达屏幕：
    /// 参数红 → 点击切预设 → 改绿立即生效 → 6 次点击轮回到参数色（仍绿）。
    #[test]
    fn cursorx_neon_glow_color_reaches_pixels() {
        let mut params = HashMap::new();
        params.insert("color".to_string(), serde_json::json!("#FF0000"));
        let eng = prep_cursorx("neon_glow", false, &params);
        // 让光点从 init 时的原点追到测试光标 (400,300)。
        for i in 0..30 {
            let t = 0.6 + i as f32 / 60.0;
            eng.update_env((400.0, 300.0), (400.0, 300.0), t, (1920.0, 1080.0), "panel")
                .unwrap();
            eng.call_step(1.0 / 60.0, t, &params).unwrap();
        }

        // 采样点偏离中心 3px：避开正中心的白高光。
        let red = render_cursorx_pixel(&eng, &params, 512, 512, 397, 300);
        println!("[neon] 参数红 → 像素 {red:?}");
        assert!(
            red.0 > 200 && red.1 < 40 && red.2 < 40 && red.3 > 200,
            "初始应显示参数红色，实际像素 {:?}",
            red
        );

        eng.call_on_button("left", 400.0, 300.0, 1.2, &params)
            .unwrap();
        let preset = render_cursorx_pixel(&eng, &params, 512, 512, 397, 300);
        println!("[neon] 点击一次 → 像素 {preset:?}");
        assert!(
            (preset.0 as i32 - 124).abs() < 24
                && (preset.1 as i32 - 92).abs() < 24
                && (preset.2 as i32 - 252).abs() < 24,
            "点击一次后应切到第一个预设色 (124,92,252)，实际 {:?}",
            preset
        );

        params.insert("color".to_string(), serde_json::json!("#00FF00"));
        let green = render_cursorx_pixel(&eng, &params, 512, 512, 397, 300);
        println!("[neon] 改参数绿 → 像素 {green:?}");
        assert!(
            green.1 > 200 && green.0 < 40 && green.2 < 40,
            "修改颜色参数后应立即变绿，实际 {:?}",
            green
        );

        // 当前在参数色（索引 0）；再点 6 次应回到参数色（仍绿）。
        for _ in 0..6 {
            eng.call_on_button("left", 400.0, 300.0, 1.3, &params)
                .unwrap();
        }
        let back = render_cursorx_pixel(&eng, &params, 512, 512, 397, 300);
        println!("[neon] 再点 6 次 → 像素 {back:?}");
        assert!(
            back.1 > 200 && back.0 < 40 && back.2 < 40,
            "点击 6 次应轮回到参数色（绿），实际 {:?}",
            back
        );
    }

    /// 把一帧 DrawList 经生产渲染层画进 640×480 缓冲，判断是否真有非透明像素。
    ///
    /// ★ 只数命令会漏掉"alpha 全是 0 / 坐标全在屏外"的死效果（Stardust Stream 就曾
    /// 产出 66 条 alpha=0 的指令，命令数门禁全绿、屏幕一片空）。
    fn list_has_pixels(list: &crate::core::draw::DrawList) -> bool {
        list_has_pixels_in(list, 640, 480)
    }

    /// 指定画布尺寸的像素判据：大半径/随机落点的效果需要更大的画布才能覆盖。
    fn list_has_pixels_in(list: &crate::core::draw::DrawList, w: u32, h: u32) -> bool {
        let mut buf = vec![0_u8; (w * h * 4) as usize];
        {
            use windui::render::{DibTarget, RenderTarget};
            let mut target = DibTarget::new(&mut buf, w, h);
            let mut engine = windui::text::DWriteEngine::new();
            let mut canvas = target.make_canvas(&mut engine, 1.0);
            crate::runtime::render::translate_offset(list, &mut *canvas, 1.0, 0.0, 0.0);
        }
        buf.chunks_exact(4).any(|p| p[3] > 0)
    }

    /// MFCMouseEffect 包：25 个脚本按各自通道的触发方式各跑一遍，至少一帧出画。
    ///
    /// 触发流程覆盖：移动（Trail/Hover 的活跃期）→ 按住 320ms（Hold）→ 抬起
    /// （Click）→ 滚轮（Scroll）→ 静止 1.6s（Hover）。每条通道的脚本只会在自己
    /// 对应的阶段产出指令，所以判据是"任一阶段出过画"，而不是最后一帧非空。
    #[test]
    fn mfcmouse_scripts_all_load_and_draw() {
        let root = crate::core::paths::lua_ext_dir();
        let pack = root.join("scripts").join("MFCMouseEffect");
        let mut files = std::fs::read_dir(&pack)
            .expect("MFCMouseEffect 包目录应存在")
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                (p.extension().and_then(|x| x.to_str()) == Some("lua"))
                    .then(|| p.file_stem()?.to_str().map(str::to_string))
                    .flatten()
            })
            .collect::<Vec<_>>();
        files.sort();
        assert_eq!(files.len(), 25, "MFCMouseEffect 应有 25 个效果脚本");

        let params = HashMap::new();
        let mut total_cmds = 0usize;
        for stem in files {
            let id = format!("scripts/MFCMouseEffect/{stem}");
            let eng = ScriptingEngine::new(&root).expect("引擎初始化失败");
            eng.load_script(&id)
                .unwrap_or_else(|e| panic!("{id} 加载失败：{e}"));
            assert!(eng.call_schema().is_ok(), "{id} schema() 报错");
            eng.call_init().unwrap();

            let mut produced = 0usize;
            let mut painted = false;
            let mut wall = 50_000.0f64;
            let mut t = 0.5f32;
            let draw = |eng: &ScriptingEngine, produced: &mut usize, painted: &mut bool| {
                let mut list = crate::core::draw::DrawList::new();
                crate::scripting::api_draw::set_draw_target(&mut list);
                eng.call_draw(&params)
                    .unwrap_or_else(|e| panic!("{id} draw 失败：{e}"));
                crate::scripting::api_draw::clear_draw_target();
                *produced += list.len_all();
                if !*painted && list_has_pixels(&list) {
                    *painted = true;
                }
            };

            // 移动期：Trail 累积轨迹；Cursor 常驻；Click/Hold 只记录按下时间。
            for i in 0..8 {
                let x = 400.0 + i as f32 * 4.0;
                eng.set_wall_ms(wall).unwrap();
                eng.update_env((x, 300.0), (400.0, 300.0), t, (1920.0, 1080.0), "panel")
                    .unwrap();
                eng.set_pointer_state(4.0, 0.0, 4.0, 0.0, false, "left")
                    .unwrap();
                eng.call_on_move(x, 300.0, t, &params).unwrap();
                eng.call_step(1.0 / 120.0, t, &params).unwrap();
                t += 1.0 / 60.0;
                wall += 16.0;
                draw(&eng, &mut produced, &mut painted);
            }

            // 按住 320ms：Hold 越过 260ms 门槛并推进进度。
            eng.call_on_button("left", 436.0, 300.0, t, &params)
                .unwrap();
            for _ in 0..24 {
                eng.set_wall_ms(wall).unwrap();
                eng.update_env((436.0, 300.0), (400.0, 300.0), t, (1920.0, 1080.0), "panel")
                    .unwrap();
                eng.set_pointer_state(4.0, 0.0, 4.0, 0.0, true, "left")
                    .unwrap();
                eng.call_step(1.0 / 60.0, t, &params).unwrap();
                t += 1.0 / 60.0;
                wall += 16.0;
            }
            draw(&eng, &mut produced, &mut painted);

            // 抬起：Click 在抬起时触发（按住已超 260ms，按上游规则应被抑制；
            // 这里只验证不崩溃，正常点击路径由下一个短按覆盖）。
            eng.call_on_mouse_up("left", 436.0, 300.0, &params).unwrap();
            eng.set_pointer_state(4.0, 0.0, 4.0, 0.0, false, "left")
                .unwrap();
            eng.call_on_button("left", 436.0, 300.0, t, &params)
                .unwrap();
            eng.call_on_mouse_up("left", 436.0, 300.0, &params).unwrap();
            eng.call_step(1.0 / 60.0, t, &params).unwrap();
            draw(&eng, &mut produced, &mut painted);

            // 滚轮：Scroll 在 on_wheel 里生成效果。
            eng.call_on_wheel(120, false, 436.0, 300.0, &params)
                .unwrap_or_else(|e| panic!("{id} on_wheel 失败：{e}"));
            for _ in 0..6 {
                eng.set_wall_ms(wall).unwrap();
                eng.call_step(1.0 / 60.0, t, &params).unwrap();
                t += 1.0 / 60.0;
                wall += 16.0;
            }
            draw(&eng, &mut produced, &mut painted);

            // 静止 1.6s：Hover 越过 1500ms 门槛。
            eng.call_on_move(436.0, 300.0, t, &params).unwrap();
            wall += 1600.0;
            for _ in 0..6 {
                eng.set_wall_ms(wall).unwrap();
                eng.call_step(1.0 / 60.0, t, &params).unwrap();
                t += 1.0 / 60.0;
                wall += 16.0;
            }
            draw(&eng, &mut produced, &mut painted);

            assert!(produced > 0, "{id} 各通道触发后均未产出任何图元");
            assert!(
                painted,
                "{id} 命令有 {produced} 条，但渲染后没有任何非透明像素（alpha=0 或全在屏外）"
            );
            total_cmds += produced;
        }
        assert!(total_cmds > 1000, "25 个脚本总指令数异常少：{total_cmds}");
    }

    /// MouseEffects 包：33 个脚本按 pack.json 的 kind 各跑一遍，保证加载、触发、
    /// step、draw 都不报错且至少一帧有非透明像素。绘制按 10 帧采样，覆盖"先升空
    /// 后爆炸"（Firework）和"短寿命圆环"（Shockwave）这类不在最后一帧出画的效果。
    #[test]
    fn mouseeffects_scripts_all_load_and_draw() {
        let root = crate::core::paths::lua_ext_dir();
        let pack = root.join("scripts").join("MouseEffects");
        let meta_text = std::fs::read_to_string(pack.join("pack.json")).expect("pack.json 可读");
        let meta: serde_json::Value =
            serde_json::from_str(&meta_text).expect("pack.json 是合法 JSON");
        let effects = meta["effects"].as_array().expect("effects 数组");
        assert_eq!(effects.len(), 33, "MouseEffects 应有 33 个效果");
        let params: HashMap<String, Json> = HashMap::new();
        let mut total = 0usize;
        for e in effects {
            let stem = e["file"].as_str().expect("file").trim_end_matches(".lua");
            let kind = e["kind"].as_str().expect("kind");
            let eng = ScriptingEngine::new(&root).expect("引擎初始化失败");
            eng.load_script(&format!("scripts/MouseEffects/{stem}"))
                .unwrap_or_else(|err| panic!("{stem} 加载失败：{err}"));
            eng.call_init().unwrap();
            let mut t = 1.0f32;
            let mut wall = 50_000.0f64;
            let frames = if kind == "click" { 90 } else { 80 };
            let mut best = 0usize;
            let mut painted = false;
            for i in 0..frames {
                let x = 400.0 + ((i % 10) as f32) * 50.0;
                let y = 300.0 + (i as f32 * 0.3).sin() * 40.0;
                wall += 16.0;
                eng.set_wall_ms(wall).unwrap();
                eng.update_env((x, y), (400.0, 300.0), t, (1920.0, 1080.0), "trail")
                    .unwrap();
                eng.set_pointer_state(12.0, 0.0, 12.0, 0.0, false, "left")
                    .unwrap();
                if kind == "trail" {
                    eng.call_on_move(x, y, t, &params).unwrap();
                }
                if kind == "click" && i == 5 {
                    eng.call_on_button("left", 500.0, 300.0, t, &params)
                        .unwrap();
                    eng.call_on_mouse_up("left", 500.0, 300.0, &params).unwrap();
                }
                eng.call_step(1.0 / 60.0, t, &params).unwrap();
                t += 1.0 / 60.0;

                if i % 10 == 0 || i == frames - 1 {
                    let mut list = crate::core::draw::DrawList::new();
                    crate::scripting::api_draw::set_draw_target(&mut list);
                    eng.call_draw(&params)
                        .unwrap_or_else(|err| panic!("{stem} draw 失败：{err}"));
                    crate::scripting::api_draw::clear_draw_target();
                    best = best.max(list.len_all());
                    painted |= list_has_pixels_in(&list, 1280, 800);
                }
            }
            assert!(best > 0, "{stem}（{kind}）未产出任何图元");
            assert!(
                painted,
                "{stem}（{kind}）命令有 {best} 条，但渲染后没有任何非透明像素"
            );
            total += best;
        }
        assert!(total > 2000, "33 个脚本总指令数异常少：{total}");
    }

    /// Stardust Stream 的 `duration` 必须贯穿整段生命周期：粒子寿命按 duration 同比
    /// 缩放。旧实现寿命固定 300–550ms，时长拖到 800ms 后 550–800ms 是一条空尾
    /// （滑条后半段无效）；这里在 704ms 采样，旧代码 0 条指令，新代码应仍有粒子。
    #[test]
    fn mfcmouse_twinkle_duration_scales_particle_life() {
        let root = crate::core::paths::lua_ext_dir();
        let eng = ScriptingEngine::new(&root).expect("引擎初始化失败");
        eng.load_script("scripts/MFCMouseEffect/twinkle")
            .expect("twinkle 加载失败");
        eng.call_init().unwrap();
        let params: HashMap<String, Json> =
            HashMap::from([("duration".to_string(), Json::from(800))]);
        eng.set_wall_ms(0.0).unwrap();
        eng.call_on_wheel(120, false, 400.0, 300.0, &params)
            .unwrap();
        let mut t = 1.0f32;
        let mut wall = 0.0f64;
        for _ in 0..44 {
            wall += 16.0;
            eng.set_wall_ms(wall).unwrap();
            eng.call_step(0.016, t, &params).unwrap();
            t += 0.016;
        }
        eng.set_wall_ms(704.0).unwrap();
        let mut list = crate::core::draw::DrawList::new();
        crate::scripting::api_draw::set_draw_target(&mut list);
        eng.call_draw(&params).unwrap();
        crate::scripting::api_draw::clear_draw_target();
        assert!(
            list.len_all() > 0,
            "duration=800ms 时 704ms 处应仍有粒子，实际 0 条（时长滑条后半段无效）"
        );
    }

    /// 滚轮回调契约：`on_wheel(delta, horizontal, x, y, params)` 五参齐全。
    #[test]
    fn call_on_wheel_reaches_lua_callback() {
        let dir = std::env::temp_dir().join(format!(
            "curseen_wheel_{}_{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("wheel.lua"),
            r#"
got_delta = 0
got_horizontal = false
got_x = 0
got_y = 0
function on_wheel(delta, horizontal, x, y, params)
  got_delta = delta
  got_horizontal = horizontal
  got_x = x
  got_y = y
end
"#,
        )
        .unwrap();
        let eng = ScriptingEngine::new(&dir).unwrap();
        eng.load_script("wheel").unwrap();
        eng.call_on_wheel(-120, true, 12.5, 34.5, &HashMap::new())
            .unwrap();
        let g = eng.lua().globals();
        assert_eq!(g.get::<i32>("got_delta").unwrap(), -120);
        assert!(g.get::<bool>("got_horizontal").unwrap());
        assert!((g.get::<f32>("got_x").unwrap() - 12.5).abs() < 0.01);
        assert!((g.get::<f32>("got_y").unwrap() - 34.5).abs() < 0.01);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ★ 悬停态（2026-10-27）：`set_pointer_state` 是四类宿主唯一的指针状态注入点，
    /// `cursor_hover` 必须随它一起刷新，且类型必须是布尔（脚本 `if cursor_hover then`
    /// 直接吃，不做 nil 兜底）。
    #[test]
    fn pointer_state_injects_cursor_hover_boolean() {
        let dir = std::env::temp_dir().join(format!(
            "curseen_hover_{}_{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let eng = ScriptingEngine::new(&dir).unwrap();
        eng.set_pointer_state(0.0, 0.0, 0.0, 0.0, false, "left")
            .unwrap();
        let hover: bool = eng
            .lua()
            .globals()
            .get("cursor_hover")
            .expect("cursor_hover 必须是布尔");
        let _ = hover; // 真值取决于机器光标位置，这里只钉类型与存在性。

        // 虚拟桌面原点也必须随环境注入（整屏暗罩/星空定位用），类型 number。
        eng.update_env((0.0, 0.0), (0.0, 0.0), 0.0, (1920.0, 1080.0), "panel")
            .unwrap();
        let _sx: f32 = eng
            .lua()
            .globals()
            .get("screen_x")
            .expect("screen_x 必须是数字");
        let _sy: f32 = eng
            .lua()
            .globals()
            .get("screen_y")
            .expect("screen_y 必须是数字");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ★★ 回归（2026-09-30）：`init` / `on_move` / `step` 都是**可选**回调。
    ///
    /// 只有 `schema` + `run` 的一次性 / 开窗脚本（`scripts/AI/ai_chat`）勾了卡片开关就会进来
    /// 当常驻实例，宿主照样逐帧分发 —— 缺回调必须**静默跳过**，否则日志每条都刷一行
    /// `复合脚本回调出错：error converting Lua nil to function`，用户会以为"开着它就在报错"。
    #[test]
    fn missing_overlay_callbacks_are_skipped_silently() {
        let dir = std::env::temp_dir().join(format!("curseen_engine_nocb_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("oneshot.lua"),
            "function schema() return {} end\nfunction run(ctx, params) return \"ok\" end\n",
        )
        .unwrap();
        let eng = ScriptingEngine::new(&dir).unwrap();
        eng.load_script("oneshot").unwrap();
        let params = HashMap::new();
        eng.call_init().expect("缺 init 不该报错");
        eng.call_on_move(1.0, 2.0, 0.0, &params)
            .expect("缺 on_move 不该报错");
        eng.call_step(0.016, 0.0, &params)
            .expect("缺 step 不该报错");
        eng.call_on_button("left", 0.0, 0.0, 0.0, &params)
            .expect("缺 on_mouse_down 不该报错");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
