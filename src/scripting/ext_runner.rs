//! 扩展脚本执行器：把 `assets/lua` 下的一个脚本在**后台线程**里跑完并弹出结果。
//!
//! ★ 为什么必须后台线程 + 非阻塞：脚本可能做阻塞 HTTP（`ext:http`，60s 超时）。若
//! "spawn + UI 线程 recv 等结果"（旧在线 OCR 脚本时代的做法），脚本页的「运行」就会
//! 在 UI 线程上干等 HTTP 最多 60 秒——就是"在线 OCR 卡死整个软件"那个老坑。这里改为
//! **彻底脱手**：spawn 一条专属线程跑脚本，结果也由**那条线程自己**要窗。
//!
//! ★ m08 起的窗语义：`ext:result` / `ext:input` 不再是"工作线程弹原生框 + 框自带的消息泵"，
//!   而是"工作线程经 [`crate::core::ui_bus`] 请 **UI 线程**开一扇 windui 模态窗，自己阻塞在
//!   回复通道上等用户"。对脚本是**同一条语义**（阻塞到关窗才返回、UI 线程零等待），
//!   对界面则是与主窗口同一套控件与配色。总线没装上（无 UI 线程的环境）时退回原生框。

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use serde_json::Value as Json;

use super::api_ext::{self, ExtCtx};
use super::engine::ScriptingEngine;

/// ★ 正在跑的脚本 id（**进程级**登记表）。
///
/// 为什么必须有它：`chat_window` / `text_result_box` / `text_input_box` 都是"工作线程
/// 弹原生框、阻塞到关窗才返回"，而"同一时刻只能有一个"这件事以前只挂在**线程局部**
/// 状态上（AI 对话窗的 `CHAT`）——本函数每次新 spawn 线程，那份状态恒为空，
/// 于是**每次按压都多一个窗**（2026-09-30 用户报告）。
///
/// 进程级而非线程级，才能让"任何线程正在跑这个脚本"被下一个触发者看见。
fn in_flight() -> &'static Mutex<HashSet<String>> {
    static TABLE: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashSet::new()))
}

/// 浮字里那个"名字"：id 的最后一段（`scripts/2.ocr_text` → `2.ocr_text`）。
///
/// ★ 刻意**不**为了这一句话去装载脚本读 `name`：那正是最慢的一步，而这句话的全部价值
///   就是**在慢的那一步之前**出现。用 id 的尾段至少与脚本页/日志里看到的名字是同一个串。
fn notice_name(id: &str) -> &str {
    id.rsplit(['/', std::path::MAIN_SEPARATOR])
        .next()
        .unwrap_or(id)
}

/// 尝试登记"我要跑这个脚本了"：成功则拿回守卫（**Drop 时自动归还**），已经在跑则返回
/// `None`（调用方应直接放弃本次触发）。
///
/// ★ 返回守卫而不是 `bool`：把"登记"和"归还"绑成一个值，就不存在"某条退出路径忘了
/// 归还"这种写法 —— 漏一次就永久堵死这个脚本的后续触发（症状是"热键按一次就废了"）。
fn try_claim(id: &str) -> Option<RunGuard> {
    let Ok(mut t) = in_flight().lock() else {
        return None;
    };
    if t.insert(id.to_string()) {
        Some(RunGuard(id.to_string()))
    } else {
        None
    }
}

/// 归还登记（脚本跑完 / panic 之后）。
fn release(id: &str) {
    if let Ok(mut t) = in_flight().lock() {
        t.remove(id);
    }
}

/// 作用域守卫：无论工作线程从哪条路径退出，登记都会被归还——漏一次就永久堵死该脚本
/// 的后续触发（看起来像"热键用一次就废了"）。
struct RunGuard(String);
impl Drop for RunGuard {
    fn drop(&mut self) {
        release(&self.0);
    }
}

/// 动作上下文（贴图右键「脚本」带上的东西）；脚本页里跑传 `None`。
pub struct ExtInput {
    pub image: crate::core::image::RgbaBitmap,
    pub hwnd: usize,
}

/// 脱手派发执行：立即返回，跑脚本 + 弹结果都在后台线程完成。
///
/// ★★ 同一脚本同时只允许跑**一个**（2026-09-30）：已经在跑就直接放弃本次触发。
/// 三个入口（脚本热键 / 贴图右键「脚本」/ 未来的调用者）共用这张表，脚本里那些
/// "弹原生框阻塞到关窗"的 API（`ext:chat_window` / `ext:result` / `ext:input`）
/// 也就不必各自再做一遍去重（它们做不了——跨线程看不见彼此）。
pub fn run_ext_detached(
    ext_dir: &Path,
    id: &str,
    params: &HashMap<String, Json>,
    ctx: Option<ExtInput>,
) {
    let name = notice_name(id);
    let Some(guard) = try_claim(id) else {
        // ★ 光写日志不够（m18）：`ext:proc` 之后一次触发可能是"冷启动 + 20 s"，
        //   连按热键却一点反应都没有，用户的判断只有一个——"这软件死了"。
        log::info!("脚本 {id} 正在运行，本次触发已忽略（同一脚本同时只跑一个）");
        crate::core::ui_bus::notice(format!("{name} 还在跑，这次忽略"));
        return;
    };
    // 领到了就先说一句"在忙"：这句必须**早于**读脚本（读脚本 + 起 VM 就是那几百毫秒），
    // 否则它出现的时刻正好等于结果快出来的时刻，等于白说。
    crate::core::ui_bus::notice(format!("正在运行：{name}"));
    let dir = ext_dir.to_path_buf();
    let id = id.to_string();
    let params = params.clone();
    let hwnd = ctx.as_ref().map(|c| c.hwnd).unwrap_or(0) as isize;
    let ctx = ctx.map(|c| ExtCtx {
        image: c.image,
        hwnd: c.hwnd,
    });
    // ★ 命名：崩溃报告按线程名标注出错线程（platform::crash），无名线程对不上号。
    // ★★ `spawn` 的 `Result` **不许吞**（m24 审查）：上面已经说了"正在运行"，线程却起不来时
    //   脚本根本没跑——那会变成一次**我们自己制造的假成功**。起不来就如实说，并把登记归还
    //   （`guard` 随闭包被丢弃，`Drop` 即归还）。
    let spawned = std::thread::Builder::new()
        .name("fx-script".into())
        .spawn(move || {
            let _guard = guard; // 搬到工作线程：线程结束（含 panic）即归还登记。
                                // RgbaBitmap 是 Vec，天然 Send；影像克隆交给工作线程。
            let r = run_ext_sync(&dir, &id, &params, ctx);
            show_result(hwnd, &id, r);
        });
    if let Err(e) = spawned {
        log::error!("起脚本线程失败（{name} 没有运行）：{e}");
        crate::core::ui_bus::notice(format!("起不了脚本线程：{name} 没有运行（详见日志）"));
    }
}

/// 在**当前线程**执行脚本并返回结果（`run_ext_detached` 的工作线程用它；单测也可直接调）。
///
/// 统一契约：`run(ctx, params)`，`ctx.role="image"`；有贴图上下文时还把当前贴图塞进
/// `ctx.image`（与 `ext:get_image()` 同对象，含 `.w/.h` 与 `png_base64()`/`save()`）、
/// 窗口句柄塞进 `ctx.hwnd`；`ctx.fs` 自动绑定脚本目录。返回：脚本 `run` 返回的字符串
/// （去首尾空白）；`nil` / 空串 = 脚本内部已用 `ext:result` 自己弹了框。失败返回
/// 面向用户的中文错误。
///
/// ★ `pub`：真机预览（`examples/script_dialog_preview.rs`）要走**生产同一条路**拿到脚本文本，
///   而不是在 example 里复刻一份统计逻辑——复刻出来的样本永远不会红，不算验证。
pub fn run_ext_sync(
    dir: &Path,
    id: &str,
    params: &HashMap<String, Json>,
    ctx: Option<ExtCtx>,
) -> Result<String, String> {
    if let Some(c) = &ctx {
        api_ext::set_ext_context(ExtCtx {
            image: c.image.clone(),
            hwnd: c.hwnd,
        });
    }
    // ★ 套 `catch_unwind`：脚本在后台线程跑，任何 Rust panic（原生框 / ureq / 用户
    //   data 方法）都转成 `Err` 走 `show_result` 弹错误框，绝不让 `panic=abort` 把整个
    //   进程带走（2026-09-29「脚本热键秒退」根因兜底）。`clear_ext_context` 放在外层，
    //   即便 panic 也清掉线程局部上下文。
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let eng = ScriptingEngine::new(dir).map_err(|e| format!("初始化脚本引擎失败：{e}"))?;
        eng.load_script(id)
            .map_err(|e| format!("加载扩展脚本 {id} 失败：{e}"))?;
        // 统一入口 `run(ctx, params)`；host 字段 = image（可选）+ hwnd（可选）。
        let mut fields: Vec<(&str, mlua::Value)> = Vec::new();
        if let Some(c) = &ctx {
            let img = api_ext::ext_image_value(eng.lua(), c.image.clone())
                .map_err(|e| format!("构造贴图上下文失败：{e}"))?;
            fields.push(("image", img));
            fields.push(("hwnd", mlua::Value::Integer(c.hwnd as i64)));
        }
        match eng.call_run_ctx("image", &fields, &merged_params(&eng, params)) {
            Ok(mlua::Value::String(s)) => Ok(s.to_string_lossy().trim().to_string()),
            Ok(_) => Ok(String::new()),
            Err(e) => Err(format!("运行扩展脚本 {id} 失败：{e}")),
        }
    }));
    api_ext::clear_ext_context();
    match out {
        Ok(r) => r,
        Err(_) => Err(format!(
            "运行扩展脚本 {id} 时捕获到不可恢复的错误（已避免程序崩溃，详见日志）"
        )),
    }
}

/// 把脚本 `schema()` 声明的默认值补进**交给脚本的那一份参数**（m17）。
///
/// ★ 为什么合并在这一刻、而不是在 `Runtime::lua_ext_params` 里：那张配置表还有别的读者
///   （`Runtime::script_hotkeys_with_state` 直接从 `config.lua_ext.params` 算热键名单，
///   与休眠唤醒登记**共用一份**）。在配置出口补默认值会造出两个来源——界面显示的初值 vs
///   实际登记的热键，而且后来任何读者都会白拿一份"被宿主改过"的表而不自知。
///   参数的边界是"交棒给脚本"，就在这里合。
///
/// ★ 为什么**不必**再读一遍盘：调用点刚 `load_script(id)` 完，同一个 VM 里调一次
///   `schema()` 就是那份声明（µs 级）。另开一条"按 id 单文件装载 Manifest"的路会多一次
///   VM 起停，还会把 role/menu/`requires` 的推演复制成两份。
///
/// `schema()` 不存在 / 抛错 ⇒ 原样交出：**取不到默认值不许连带把参数弄坏**。
/// 键的类型与声明不符 ⇒ 回退默认值并留一行日志（症状会是"改了没反应"，日志必须说真话）。
fn merged_params(eng: &ScriptingEngine, params: &HashMap<String, Json>) -> HashMap<String, Json> {
    let mut out = params.clone();
    let Ok(tbl) = eng.call_schema() else {
        return out;
    };
    for p in super::registry::parse_schema_table(&tbl) {
        // 先取值再写表：`stored` 是对 `out` 的借用，带着它 `insert` 编不过。
        let stored = out.get(&p.key).cloned();
        let fitted = p.value_or_default(stored.as_ref());
        match &stored {
            None => {
                out.insert(p.key.clone(), fitted);
            }
            Some(v) if *v != fitted => {
                log::warn!(
                    "脚本参数「{key}」存的值与声明类型不符，按默认值交（存的＝{v:?}）",
                    key = p.key,
                    v = v
                );
                out.insert(p.key.clone(), fitted);
            }
            Some(_) => {}
        }
    }
    out
}

/// 把执行结果弹出来：空串（脚本已自己 `ext:result`）→ 不再弹；非空 → 当结果文本弹；
/// 错误 → 弹错误。全部在调用线程（工作线程）完成——窗本身由 UI 线程开，
/// 这里只送数据并阻塞等用户关窗（见 [`api_ext::show_result_blocking`]）。
fn show_result(hwnd: isize, id: &str, r: Result<String, String>) {
    match r {
        Ok(text) if text.is_empty() => {}
        Ok(text) => api_ext::show_result_blocking("脚本结果", &text, hwnd),
        Err(e) => {
            api_ext::show_result_blocking("脚本结果", &format!("运行「{id}」失败：{e}"), hwnd)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::image::RgbaBitmap;

    /// 图片体检 / OCR 已迁到同级 `CurSeen-Scripts` 仓库；直接跑仓库源（不复制第二份）。
    fn repo_tool(tool: &str) -> std::path::PathBuf {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../CurSeen-Scripts/Packs")
            .join(tool)
            .join("scripts");
        assert!(
            p.is_dir(),
            "图片体检/OCR 已迁到 CurSeen-Scripts，测试需要同级检出：{p:?}"
        );
        p
    }

    /// 写一个脚本到临时目录，返回目录。目录名带**脚本内容的指纹**，让并行跑的单测
    /// 各写各的 `demo.lua`，避免共享同一目录互相删/写（旧实现只按 PID 建目录，两测试
    /// 并发时会把对方的 demo.lua 清掉 → 读到错误脚本 —— 本次修掉）。
    fn write_script(body: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "curseen_extrunner_test_{}_{:x}",
            std::process::id(),
            body.len().wrapping_mul(1_000_003) // 简单指纹，够区分这几条用例即可
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("demo.lua"), body).unwrap();
        dir
    }

    /// ★★ 回归（2026-09-30 用户报告"重复按快捷键冒出多个窗口"）：同一脚本第二次
    /// `claim` 必须失败（进程级去重，跨线程可见）；`release` 之后又能再 claim。
    #[test]
    fn same_script_cannot_claim_twice() {
        let id = "curseen_unit_dedupe_demo";
        let guard = try_claim(id);
        assert!(guard.is_some(), "首次登记必须成功");
        assert!(
            try_claim(id).is_none(),
            "同一脚本在跑时不许再登记一次（正是这条漏出「多个窗口」）"
        );
        drop(guard); // 守卫 Drop = 归还
        let again = try_claim(id);
        assert!(again.is_some(), "归还之后必须能重新登记");
        drop(again);
        assert!(try_claim(id).is_some(), "不能留下没归还的登记");
    }

    #[test]
    fn run_returns_script_string() {
        let dir = write_script(
            "function run(ctx, params)\n  return \"hello \" .. (params.who or \"x\")\nend\n",
        );
        let mut params = HashMap::new();
        params.insert("who".to_string(), Json::String("world".into()));
        let out = run_ext_sync(&dir, "demo", &params, None).unwrap();
        assert_eq!(out, "hello world");
    }

    /// ★ m18：浮字里那个"名字"取 id 的最后一段——它必须与脚本页/日志里的串一致，
    /// 否则用户看到「正在运行：foo」却找不到哪个脚本叫 foo。
    #[test]
    fn the_notice_name_is_the_last_id_segment() {
        assert_eq!(notice_name("scripts/2.ocr_text"), "2.ocr_text");
        assert_eq!(notice_name("MouseTrail/fireflies"), "fireflies");
        assert_eq!(notice_name("bare"), "bare");
        // Windows 上 id 也可能带反斜杠（配置里手写的），同一个函数要认得。
        let win = ["scripts", "a"].join(std::path::MAIN_SEPARATOR_STR);
        assert_eq!(notice_name(&win), "a");
    }

    /// ★ m17：`schema()` 声明的 `default` 过去**只对界面生效**，脚本收到的是配置原样
    /// （所以每个脚本都得自己兜"这个键没出现"）。现在在交棒那一刻合并，规则见 [`merged_params`]。
    ///
    /// 三格都走 `run_ext_sync` = **产品自己那条路**（`examples/ext_proc_ocr.rs` 用的就是它），
    /// 不在测试里复刻合并逻辑——复刻出来的样本永远不会红，不算验证。
    #[test]
    fn schema_defaults_reach_the_script_without_clobbering_explicit_values() {
        let dir = write_script(
            "function schema()\n  return {\n    { key=\"copy\", label=\"复制\", type=\"bool\", default=true },\n    { key=\"wide\", label=\"宽\", type=\"int\", min=5, max=120, default=60 }\n  }\nend\nfunction run(ctx, params)\n  return tostring(params.copy) .. \"|\" .. tostring(params.wide) .. \"|\" .. tostring(params.extra)\nend\n",
        );
        // ① 配置为空 ⇒ 两个默认值都到得了脚本
        let none = HashMap::new();
        assert_eq!(
            run_ext_sync(&dir, "demo", &none, None).unwrap(),
            "true|60|nil",
            "缺键必须交 schema 的 default"
        );
        // ② 用户显式写了 `false` / `0` ⇒ **原样交**。★ 这一格是本轮的红证靶：把"缺键"写成
        //   "值为假"在这儿会翻回默认值 —— 用户刚关掉的东西又被打开了。
        let mut explicit = HashMap::new();
        explicit.insert("copy".to_string(), Json::Bool(false));
        explicit.insert("wide".to_string(), Json::from(0_i64));
        assert_eq!(
            run_ext_sync(&dir, "demo", &explicit, None).unwrap(),
            "false|0|nil",
            "显式的假值不是\"没填\""
        );
        // ③ `schema` 之外的键原样透传（宿主不许"顺手清理"用户配置）
        let mut extra = HashMap::new();
        extra.insert("extra".to_string(), Json::String("keep".into()));
        assert_eq!(
            run_ext_sync(&dir, "demo", &extra, None).unwrap(),
            "true|60|keep",
            "声明外的键一律不动"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 类型与声明不符 ⇒ 回退默认值（症状会是"改了没反应"，所以另有一行日志说存的到底是什么）。
    /// ★ 故意让默认值与"字符串看着的意思"**反号**：`copy` 存了 `"false"`、默认是 `true`
    ///   ⇒ 读出 `true` 才证明走的是回退，而不是"把字符串按布尔凑"。
    #[test]
    fn wrong_typed_config_value_falls_back_to_default() {
        let dir = write_script(
            "function schema()\n  return { { key=\"copy\", label=\"复制\", type=\"bool\", default=true } }\nend\nfunction run(ctx, params)\n  return tostring(params.copy)\nend\n",
        );
        let mut bad = HashMap::new();
        bad.insert("copy".to_string(), Json::String("false".into()));
        assert_eq!(
            run_ext_sync(&dir, "demo", &bad, None).unwrap(),
            "true",
            "类型不符要回退默认值"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 脚本**没写** `schema()` ⇒ 参数原样交出：取不到默认值不许连带把参数弄坏。
    /// 顺带钉住"合并不写回调用方那份表"——配置层"用户从没碰过这个键"的凭证要保持原样。
    #[test]
    fn missing_schema_leaves_params_untouched() {
        let dir = write_script(
            "function run(ctx, params)\n  return tostring(params.a) .. \"|\" .. tostring(params.b)\nend\n",
        );
        let mut params = HashMap::new();
        params.insert("a".to_string(), Json::from(7_i64));
        assert_eq!(
            run_ext_sync(&dir, "demo", &params, None).unwrap(),
            "7|nil",
            "没有 schema 时一个字都不该改"
        );
        assert!(
            !params.contains_key("b"),
            "合并不许写回调用方那份表（那是配置层的凭证）"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ★ 数值参数到脚本手里要**保持它是整数还是浮点**（m17 对拍跑出来的第二个缺陷：
    /// `engine::build_params` 过去 `as_f64()` 优先，`type="int"` 的键永远变成浮点，
    /// `tostring` 出 `60.0`，`("%d"):format` 只是运气好没炸）。
    #[test]
    fn int_and_float_params_keep_their_kind_in_lua() {
        let dir = write_script(
            "function schema()\n  return {\n    { key=\"n\", label=\"整数\", type=\"int\", min=1, max=9, default=6 },\n    { key=\"f\", label=\"比例\", type=\"float\", min=0, max=1, default=0.5 }\n  }\nend\nfunction run(ctx, params)\n  return math.type(params.n) .. \"|\" .. math.type(params.f) .. \"|\" .. tostring(params.n)\nend\n",
        );
        let none = HashMap::new();
        assert_eq!(
            run_ext_sync(&dir, "demo", &none, None).unwrap(),
            "integer|float|6",
            "int 档必须是 Lua 整数，float 档必须是浮点"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `ext:data_dir` 必须真的按**当前脚本 id** 落到 `<exe>/data/lua/<id>`；引擎装载时
    /// 不把 id 交给 api_ext，所有脚本会共用一个目录（或直接报错）。
    #[test]
    fn data_dir_follows_the_loaded_script_id() {
        let dir = write_script(
            "function run() return ext:data_dir() end
",
        );
        let out = run_ext_sync(&dir, "demo", &HashMap::new(), None).unwrap();
        assert!(out.ends_with("demo"), "{out}");
        assert!(!out.contains("assets"), "不能落进 assets：{out}");
        let _ = std::fs::remove_dir_all(&out);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn get_image_with_context_returns_pixels() {
        let dir = write_script(
            "function run(ctx, params)\n  local i = ctx.image or ext:get_image()\n  if i == nil then return \"nil\" end\n  return \"w\" .. i.w\nend\n",
        );
        let ctx = ExtCtx {
            image: RgbaBitmap {
                w: 7,
                h: 3,
                rgba: vec![0u8; 84],
            },
            hwnd: 0,
        };
        let out = run_ext_sync(&dir, "demo", &HashMap::new(), Some(ctx)).unwrap();
        assert_eq!(out, "w7");
    }

    #[test]
    fn missing_run_is_ok_empty() {
        let dir = write_script("name = \"占位\"\n");
        let out = run_ext_sync(&dir, "demo", &HashMap::new(), None).unwrap();
        assert_eq!(out, "");
    }

    /// ★ 仓库里的图片体检脚本必须**永远能跑**：它同时是用户的参考实现，脚本坏了
    /// 等于文档撒谎。直接对 `CurSeen-Scripts/Packs/ImageReport/scripts` 的真文件跑（像素 API + 无上下文两路）。
    #[test]
    fn repo_image_report_runs_with_image_context() {
        let dir = repo_tool("ImageReport");
        let bmp = RgbaBitmap {
            w: 4,
            h: 2,
            // 第二行四个像素 alpha=0（全透明）：不应参与平均，否则颜色会被拉黑。
            rgba: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255, 10, 20, 30, 0,
                10, 20, 30, 0, 10, 20, 30, 0, 10, 20, 30, 0,
            ],
        };
        let ctx = Some(ExtCtx {
            image: bmp.clone(),
            hwnd: 0,
        });
        let out = run_ext_sync(&dir, "1.image_report", &HashMap::new(), ctx.clone())
            .unwrap_or_else(|e| panic!("仓库脚本 1.image_report 跑失败了：{e}"));
        assert!(
            !out.contains("没有贴图上下文"),
            "1.image_report 没拿到贴图上下文"
        );
        // 平均色：只算第一行 4 个不透明像素（红/绿/蓝/白），各通道 (255+0+0+255)/4
        // = 127.5 → 128 = 0x80，透明像素不计入。合二为一之后仍由**同一个脚本**算出来。
        let out = run_ext_sync(&dir, "1.image_report", &HashMap::new(), ctx).unwrap();
        // ★ 不能对整份输出 `split('#')`：Markdown 的 `## 标题` 本身就以 `#` 开头。
        let hex: String = out
            .lines()
            .find(|l| l.contains("平均色："))
            .and_then(|l| l.split('#').nth(1))
            .map(|s| s.chars().take(6).collect())
            .unwrap_or_default();
        assert_eq!(hex.to_uppercase(), "808080", "平均色应可复算，实际：{out}");
        // 尺寸、比例、平均色、透明统计要在**同一份输出**里（合并成「图片体检」的判据：
        // 一次调用把所有信息类读数都给全，而不是各开一扇窗）。
        for needle in ["尺寸：4 × 2", "比例：2:1", "## 透明度", "平均色：#808080"] {
            assert!(out.contains(needle), "输出缺「{needle}」：{out}");
        }
        // m48：结果按纯文本 Markdown 排版——不摆顶部标题、组间空行、主色板用 1/2/3。
        assert!(
            out.starts_with("## 基本信息"),
            "第一行就该是分组标题：{out}"
        );
        assert!(
            out.contains(
                "

## 透明度"
            ),
            "组与组之间要空一行：{out}"
        );
        assert!(
            out.lines().any(|l| l.starts_with("1. #")),
            "主色板要用 1/2/3 编号：{out}"
        );
        // 无上下文（脚本页直接运行）：m30 起会**回落到剪贴板里的截图**——
        // 于是这一格的结果取决于这台机器的剪贴板：有图 ⇒ 真出一份报告；没图 ⇒ 一句"怎么用"。
        // 两条都是对的，判据只钉"永远有话可说、不 panic"（别在这里断言死某一条）。
        let out = run_ext_sync(&dir, "1.image_report", &HashMap::new(), None).unwrap();
        assert!(
            out.contains("尺寸：") || out.contains("没有可体检的图"),
            "无上下文时要么出剪贴板图的报告、要么给一句怎么用，实际：{out}"
        );
    }

    /// 仓库里的「OCR」必须**永远能加载并跑完**，而且**不许再提系统引擎**（m13 拆掉了）。
    ///
    /// 源目录里没有 `ocr_tool/`（按口径不随源码带），所以这里跑的就是"外置文件没到位"那一格：
    /// 脚本要**点名缺哪几个文件**、怎么放，而不是空跑一次注定失败的识别，也不是 panic。
    /// 这一格同时是"热键那条路"的读数——菜单会被 `requires` 挡掉，热键不挡，靠的就是这段。
    #[test]
    fn repo_ocr_script_names_the_missing_engine_files() {
        let dir = repo_tool("OCR");
        let bmp = RgbaBitmap {
            w: 4,
            h: 2,
            rgba: vec![255u8; 32],
        };
        let ctx = ExtCtx {
            image: bmp,
            hwnd: 0,
        };
        // ★ 常驻测试段**必须**显式关掉复制：这脚本默认把结果写进剪贴板，而剪贴板是本机共享
        //   资源（还有别的程序与代理在用）。测试不该顺手改掉用户的内容——真要验那条，
        //   跑 `api_ext::tests::clip_roundtrip_restores_user_content`（`#[ignore]`，会先备份还原）。
        let mut params: HashMap<String, Json> = HashMap::new();
        params.insert("copy".to_string(), Json::Bool(false));
        let out = run_ext_sync(&dir, "2.ocr_text", &params, Some(ctx))
            .unwrap_or_else(|e| panic!("仓库脚本 2.ocr_text 跑失败了：{e}"));
        assert!(
            !out.contains("没有贴图上下文"),
            "带了贴图上下文却说我拿不到 ⇒ ctx.image 那条线断了"
        );
        // 缺文件那一格：要点名是哪个文件，不能只说"识别失败"。
        assert!(
            out.contains("./ocr_tool/CurSeenOcr.exe"),
            "没装引擎时该点名缺的文件，实际：{out}"
        );
        // 拆掉系统引擎之后，任何"改用系统 OCR"的字样都该是残留。
        assert!(
            !out.contains("系统自带 OCR") && !out.contains("系统引擎"),
            "输出里不该再有系统引擎的退路，实际：{out}"
        );
        // 复制关掉了就不该出现那句尾注（出现了就是 `params.copy` 没接到脚本里）。
        assert!(
            !out.contains("已复制到剪贴板"),
            "copy=false 却还是复制了：参数没穿到 present 那一层，实际：{out}"
        );
        // 无上下文：m30 起回落到剪贴板里的截图。于是这一格同样取决于剪贴板：
        //   有图 ⇒ 真的去跑外置引擎（本机没装 ⇒ 点名缺的文件）；没图 ⇒ 一句"怎么用"。
        // 两条都对；判据是"有输出、不 panic、不静默"。
        let out = run_ext_sync(&dir, "2.ocr_text", &params, None).unwrap();
        assert!(
            out.contains("没有可识别的图") || out.contains("./ocr_tool/CurSeenOcr.exe"),
            "无上下文时应回落到剪贴板或给一句怎么用，实际：{out}"
        );
    }
}
