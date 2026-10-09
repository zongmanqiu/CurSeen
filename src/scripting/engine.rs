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
        // `ctx.fs` 跟着脚本走：`scripts/ai_chat` 绑 `scripts_dir/scripts`（脚本所在目录），
        // 顶层脚本绑 `scripts_dir`。文档口径「相对脚本所在目录」由此成立。
        if let Some(dir) = path.parent() {
            *self.fs_base.borrow_mut() = dir.to_path_buf();
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
        self.lua.globals().set("wall_ms", wall_ms)
    }

    /// 把 `HashMap<String, Json>` 转成 Lua table（number/string/bool）。
    fn build_params(&self, params: &HashMap<String, Json>) -> mlua::Result<mlua::Table> {
        let tbl = self.lua.create_table()?;
        for (k, v) in params {
            match v {
                Json::Number(n) => {
                    if let Some(f) = n.as_f64() {
                        tbl.set(k.as_str(), f)?;
                    } else if let Some(i) = n.as_i64() {
                        tbl.set(k.as_str(), i as f64)?;
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
    /// （如 `scripts/ai_chat`）勾了卡片开关也会进来当常驻实例，宿主照旧逐帧分发回调；
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
        params: &HashMap<String, Json>,
    ) -> mlua::Result<()> {
        let globals = self.lua.globals();
        let tbl = self.build_params(params)?;
        globals.set("_params", tbl.clone())?;
        let f: mlua::Function = match globals.get("on_mouse_down") {
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

    /// ★ ctx.fs 必须绑定**脚本所在目录**，不是引擎根目录：`scripts/ai_chat` 这类
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

    /// AI 速查里的两段完整示例必须在**真引擎**里加载并跑通回调——示例是给 AI 抄的
    /// 模板，模板本身跑不通等于教 AI 写坏脚本（2026-09-30 用户要求补示例时一并钉住）。
    /// 抽取规则：ai_chat_api.txt 里的 ```lua 围栏块，应恰好两段。
    #[test]
    fn ai_reference_examples_run_in_the_real_engine() {
        let txt = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/lua/scripts/ai_chat_api.txt"
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
            2,
            "速查应恰好两段完整示例（拖尾 + 复合），实得 {}",
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
            // 有 init 就调（示例 2 没写 init，与宿主同口径静默跳过）。
            if let Ok(Some(f)) = eng.lua().globals().get::<Option<mlua::Function>>("init") {
                f.call::<()>(()).unwrap();
            }
            eng.update_env((120.0, 90.0), (120.0, 90.0), 0.4, (1920.0, 1080.0), "trail")
                .unwrap();
            // 各回调真跑一遍（缺失的静默跳过 = 宿主同口径）；draw 必须产出指令。
            let _ = eng.call_on_move(120.0, 90.0, 0.3, &params);
            let _ = eng.call_on_button("middle", 120.0, 90.0, &params);
            let _ = eng.call_step(0.016, 0.316, &params);
            let mut list = crate::core::draw::DrawList::new();
            crate::scripting::api_draw::set_draw_target(&mut list);
            let _ = eng
                .call_draw(&params)
                .unwrap_or_else(|e| panic!("示例 {i} 的 draw 跑挂：{e}"));
            crate::scripting::api_draw::clear_draw_target();
            assert!(
                list.len_all() > 0,
                "示例 {i} 的 draw 应产出至少一条绘图指令"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ★★ 回归（2026-09-30）：`init` / `on_move` / `step` 都是**可选**回调。
    ///
    /// 只有 `schema` + `run` 的一次性 / 开窗脚本（`scripts/ai_chat`）勾了卡片开关就会进来
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
        eng.call_on_button("left", 0.0, 0.0, &params)
            .expect("缺 on_mouse_down 不该报错");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
