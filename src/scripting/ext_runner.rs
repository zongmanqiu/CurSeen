//! 扩展脚本执行器：把 `assets/lua` 下的一个脚本在**后台线程**里跑完并弹出结果。
//!
//! ★ 为什么必须后台线程 + 非阻塞：脚本可能做阻塞 HTTP（`ext:http`，60s 超时）。若
//! "spawn + UI 线程 recv 等结果"（旧在线 OCR 脚本时代的做法），脚本页的「运行」就会
//! 在 UI 线程上干等 HTTP 最多 60 秒——就是"在线 OCR 卡死整个软件"那个老坑。这里改为
//! **彻底脱手**：spawn 一条
//! 专属线程跑脚本，结果（`ext:result` 弹框或脚本返回字符串）也由**那条线程自己**弹，
//! `text_result_box` 自带消息泵（见 `win_util`），工作线程可直接阻塞弹框，UI 线程零等待。

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

/// 动作上下文（贴图右键「图片操作」带上的东西）；脚本页里跑传 `None`。
pub struct ExtInput {
    pub image: crate::core::image::RgbaBitmap,
    pub hwnd: usize,
}

/// 脱手派发执行：立即返回，跑脚本 + 弹结果都在后台线程完成。
///
/// ★★ 同一脚本同时只允许跑**一个**（2026-09-30）：已经在跑就直接放弃本次触发。
/// 三个入口（脚本热键 / 贴图右键「图片操作」/ 未来的调用者）共用这张表，脚本里那些
/// "弹原生框阻塞到关窗"的 API（`ext:chat_window` / `ext:result` / `ext:input`）
/// 也就不必各自再做一遍去重（它们做不了——跨线程看不见彼此）。
pub fn run_ext_detached(
    ext_dir: &Path,
    id: &str,
    params: &HashMap<String, Json>,
    ctx: Option<ExtInput>,
) {
    let Some(guard) = try_claim(id) else {
        log::info!("脚本 {id} 正在运行，本次触发已忽略（同一脚本同时只跑一个）");
        return;
    };
    let dir = ext_dir.to_path_buf();
    let id = id.to_string();
    let params = params.clone();
    let hwnd = ctx.as_ref().map(|c| c.hwnd).unwrap_or(0) as isize;
    let ctx = ctx.map(|c| ExtCtx {
        image: c.image,
        hwnd: c.hwnd,
    });
    // ★ 命名：崩溃报告按线程名标注出错线程（platform::crash），无名线程对不上号。
    let _ = std::thread::Builder::new()
        .name("fx-script".into())
        .spawn(move || {
            let _guard = guard; // 搬到工作线程：线程结束（含 panic）即归还登记。
                                // RgbaBitmap 是 Vec，天然 Send；影像克隆交给工作线程。
            let r = run_ext_sync(&dir, &id, &params, ctx);
            show_result(hwnd, &id, r);
        });
}

/// 在**当前线程**执行脚本并返回结果（`run_ext_detached` 的工作线程用它；单测也可直接调）。
///
/// 统一契约：`run(ctx, params)`，`ctx.role="image"`；有贴图上下文时还把当前贴图塞进
/// `ctx.image`（与 `ext:get_image()` 同对象，含 `.w/.h` 与 `png_base64()`/`save()`）、
/// 窗口句柄塞进 `ctx.hwnd`；`ctx.fs` 自动绑定脚本目录。返回：脚本 `run` 返回的字符串
/// （去首尾空白）；`nil` / 空串 = 脚本内部已用 `ext:result` 自己弹了框。失败返回
/// 面向用户的中文错误。
pub(crate) fn run_ext_sync(
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
        match eng.call_run_ctx("image", &fields, params) {
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

/// 把执行结果弹出来：空串（脚本已自己 `ext:result`）→ 不再弹；非空 → 当结果文本弹；
/// 错误 → 弹错误。全部在调用线程（工作线程）完成。
fn show_result(hwnd: isize, id: &str, r: Result<String, String>) {
    match r {
        Ok(text) if text.is_empty() => {}
        Ok(text) => crate::platform::win_util::text_result_box(hwnd, "扩展脚本结果", &text),
        Err(e) => crate::platform::win_util::text_result_box(
            hwnd,
            "扩展脚本结果",
            &format!("运行「{id}」失败：{e}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::image::RgbaBitmap;

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

    /// ★ 出厂自带的示例脚本必须**永远能跑**：它们同时是用户的参考实现，脚本坏了
    /// 等于文档撒谎。直接对 `assets/lua` 里的真文件跑（像素 API + 无上下文两路）。
    #[test]
    fn bundled_example_scripts_run_with_image_context() {
        let dir = std::path::Path::new("assets/lua");
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
        for id in ["scripts/1.image_info", "scripts/2.avg_color"] {
            let out = run_ext_sync(dir, id, &HashMap::new(), ctx.clone())
                .unwrap_or_else(|e| panic!("出厂脚本 {id} 跑失败了：{e}"));
            assert!(!out.contains("没有贴图上下文"), "{id} 没拿到贴图上下文");
        }
        // 平均颜色：只算第一行 4 个不透明像素（红/绿/蓝/白），各通道 (255+0+0+255)/4
        // = 127.5 → 128 = 0x80，透明像素不计入。
        let out = run_ext_sync(dir, "scripts/2.avg_color", &HashMap::new(), ctx).unwrap();
        let hex: String = out
            .split('#')
            .nth(1)
            .map(|s| s.chars().take(6).collect())
            .unwrap_or_default();
        assert_eq!(hex.to_uppercase(), "808080", "平均色应可复算，实际：{out}");
        // 无上下文（脚本页直接运行）→ 友好提示而不是报错。
        let out = run_ext_sync(dir, "scripts/1.image_info", &HashMap::new(), None).unwrap();
        assert!(out.contains("没有贴图上下文"));
    }
}
