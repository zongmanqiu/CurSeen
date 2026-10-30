//! 暴露给 Lua 扩展脚本的 API（`assets/lua/*.lua` 里的 `run` 用）。
//!
//! 通过全局 userdata `ext` 调用，冒号语法：`ext:get_image()` / `ext:result(text)` /
//! `ext:http(method, url, headers, body)` / `ext:log(msg)`。
//!
//! # 上下文（贴图动作）
//!
//! 当脚本由贴图右键「脚本」触发时，执行器（[`super::ext_runner`]）会在 `run` 前把
//! 当前贴图写进线程局部 [`EXT_CTX`]；`ext:get_image()` 据此返回贴图。在脚本页里跑
//! （没有贴图）时返回 `nil`。与 `api_draw` 的 TARGET 一样用"线程局部 + set/clear"，
//! 单线程下无生命周期问题。

use std::cell::RefCell;
use std::time::Duration;

use mlua::{Lua, UserData, UserDataMethods};

use crate::core::events::ScriptDialogSpec;
use crate::core::image::RgbaBitmap;

/// 当前动作上下文（贴图动作专用）：本线程正在跑的扩展脚本能拿到的东西。
#[derive(Clone)]
pub struct ExtCtx {
    /// 触发当前脚本的那张贴图（贴图右键全新克隆，脚本尽可持有）。
    pub image: RgbaBitmap,
    /// 结果框/弹窗的 owner 窗口句柄。
    pub hwnd: usize,
}

thread_local! {
    static EXT_CTX: RefCell<Option<ExtCtx>> = const { RefCell::new(None) };
}

/// `ext:open_temp` 允许自动打开的扩展名：只放光栅图，避免二维码内容借
/// `.exe/.lnk/.url` 变成一次执行。纯函数，测试直接钉它。
fn open_temp_ext_allowed(ext: &str) -> bool {
    matches!(
        ext,
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "tif" | "tiff" | "ico"
    )
}

/// 在调用扩展脚本的 `run` 前设置当前动作上下文；跑完务必配对调用 [`clear_ext_context`]。
pub fn set_ext_context(ctx: ExtCtx) {
    EXT_CTX.with(|c| *c.borrow_mut() = Some(ctx));
}

/// 清空动作上下文（`run` 返回后调用，避免上下文泄漏到不该用它的脚本）。
pub fn clear_ext_context() {
    EXT_CTX.with(|c| *c.borrow_mut() = None);
}

thread_local! {
    /// 当前线程正在跑的脚本**所在目录**（`load_script` 每次加载时刷新）。
    static SCRIPT_DIR: RefCell<Option<std::path::PathBuf>> = const { RefCell::new(None) };
    static SCRIPT_ID: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// 记录"本线程接下来跑的脚本在哪个目录"——由 [`super::engine::ScriptingEngine::load_script`]
/// 在绑定 `ctx.fs` 的同一处调用，两个"脚本自己的目录"必须是同一个来源，不能各记一份。
pub fn set_script_dir(dir: &std::path::Path) {
    SCRIPT_DIR.with(|d| *d.borrow_mut() = Some(dir.to_path_buf()));
}

/// 记下当前脚本 id（由 [`crate::scripting::engine::ScriptingEngine::load_script`] 调用），
/// `ext:data_dir` 用它算私有目录；id 与配置/热键里的那个串同源。
pub fn set_script_id(id: &str) {
    SCRIPT_ID.with(|d| *d.borrow_mut() = Some(id.to_string()));
}

fn script_id() -> Option<String> {
    SCRIPT_ID.with(|d| d.borrow().clone())
}

/// 当前脚本目录（`None` = 没加载过脚本，比如单测里的裸 `Lua`）。
fn script_dir() -> Option<std::path::PathBuf> {
    SCRIPT_DIR.with(|d| d.borrow().clone())
}

/// `ext:read_file` 的单次上限。超了一律**拒**而不是截断：一段被剪掉的字节当成图片 /
/// JSON 用，症状是"这文件坏了"，而"太大读不了"至少是人能看懂的一句话。
/// 64 MiB 足够任何一张贴图（RGBA 一亿像素 = 400 MiB 是极端，实际截图几百 KB–几 MiB）。
const MAX_READ: u64 = 64 << 20;

/// 脚本交换临时文件的目录（绝对、按进程隔离）。见 `ext:tmpdir`。
/// `pub`：端到量具（`examples/ext_proc_ocr.rs`）要按同一个来源算出外置工具写回的副作用文件。
pub fn tmpdir_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("curseen_ext_{}", std::process::id()))
}

/// 阻塞类调用（HTTP / 下载 / 外部程序）在 **UI 线程**上必须拒绝。
///
/// 与模态框那条 [`modal_block`] 同源判据（[`crate::core::ui_bus::on_ui_thread`]）：
/// `draw` 模型逐帧跑在 UI 线程，一次网络请求（最长 60 s）、一次下载（不限时长）或一次
/// 外部工具（最长 120 s）会把整屏冻住——症状是"软件卡死"，而不是"这个脚本慢"。
fn blocking_io_block() -> Option<&'static str> {
    crate::core::ui_bus::on_ui_thread().then_some(
        "这个调用会卡住界面：网络请求 / 外部程序只能在 run 模型（image / keysound）里调；draw 模型逐帧跑在 UI 线程上",
    )
}

/// `ext:proc` 的**唯一**路径收口：把脚本写的 `exe` 解析成脚本目录内的绝对路径。
///
/// 三条判据，缺一即拒（返回中文原因，脚本原样显示给用户）：
/// 1. 走 [`super::fs_api::resolve`]——绝对路径 / UNC / 盘符 / `..` 全部拒绝，
///    所以能跑的只有**这个脚本目录（含它的子目录）**里的文件；
/// 2. 后缀必须 `.exe`：`.dll` 不该被当程序起，`.bat`/`.cmd` 要走 `cmd /C` 等于把
///    命令行拼接的门重新打开（`platform::process` 的头一条纪律就是绝不拼命令行）；
/// 3. 文件必须真的在那儿：给"没装这个工具"的提示，而不是让 `CreateProcess` 报一句英文。
///
/// ★ 这条**不是安全边界**，别当成沙箱读：`ext:write_file` 落到账号能碰的任何位置，
///   所以一个脚本完全能把 exe 写进脚本目录再被这里起来。它拦的是两件真实的事——
///   手滑（脚本作者写了 `C:\Windows\System32\…`）与误装（装错包、路径拼歪）。
///   信任模型只有一句：**装脚本 = 交出你这个账号的权限**，和装任何来源的 exe 同级。
pub(crate) fn resolve_tool(dir: &std::path::Path, exe: &str) -> Result<std::path::PathBuf, String> {
    let p = super::fs_api::resolve(dir, exe)
        .ok_or_else(|| format!("外部程序只能放在脚本自己的目录里，不接受这个路径：{exe}"))?;
    let is_exe = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("exe"))
        .unwrap_or(false);
    if !is_exe {
        return Err(format!(
            "外部程序只能是 .exe（脚本目录里的相对路径），不接受：{exe}"
        ));
    }
    if !p.is_file() {
        return Err(format!(
            "找不到 {exe}：它应当在脚本目录 {} 下",
            dir.display()
        ));
    }
    Ok(p)
}

/// 当前脚本能不能在这里开一扇**原生模态对话框**。`Some(原因)` = 不能，调用方原样交回脚本。
///
/// ★ 判据只有一处（[`crate::core::ui_bus::on_ui_thread`]）：`draw` 模型逐帧跑在 UI 线程，
///   而 `GetOpenFileNameW` **阻塞调用线程**直到用户点掉——在那儿开就是整屏冻住。
///   `ext:result` / `ext:input` 走总线时是同一条前提，但那边可以"只开窗不等回答"（见
///   `core::ui_bus::dialog` 的防自锁分支）；对话框不等回答就没有意义，所以这里直接拒。
fn modal_block(on_ui_thread: bool) -> Option<&'static str> {
    if on_ui_thread {
        Some(MODAL_BLOCK_MSG)
    } else {
        None
    }
}

/// 挡下模态调用时给脚本看的那句话（写死一条，免得三个入口各说一套）。
const MODAL_BLOCK_MSG: &str =
    "逐帧脚本（draw）里不能开文件对话框：那会冻住整个屏幕。请把这段逻辑放进 role=\"image\" 的脚本（它跑在工作线程上）。";

/// `nil, 中文原因` 这一档返回值的共同构造（`ext:proc` / `ext:open_file` /
/// `ext:save_file` / `ext:clip_set` 五处同一条形状，只留这一份实现）。
fn lua_nil_err(lua: &Lua, msg: String) -> mlua::Result<mlua::MultiValue> {
    Ok(mlua::MultiValue::from_vec(vec![
        mlua::Value::Nil,
        mlua::Value::String(lua.create_string(msg.as_bytes())?),
    ]))
}

/// 把脚本给的字符串路径变成 `Path`；空串按"没给"处理。
fn opt_path(s: Option<String>) -> Option<std::path::PathBuf> {
    s.map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
}

/// 触发当前脚本的窗口句柄（对话框用它做到"模态于那张贴图"）。
fn owner_hwnd() -> Option<isize> {
    EXT_CTX
        .with(|c| c.borrow().as_ref().map(|c| c.hwnd as isize))
        .filter(|h| *h != 0)
}

/// `ext:open_file` 的实体。★ `on_ui_thread` **作参数进来**：单测要能在"我就是 UI 线程"那一档
/// 把拒绝分支走一遍，而**不真去开一扇模态窗**——真开了就不是变红，是把测试进程卡在那里。
/// 方法本身只剩一行接线（把总线读到的档位递进来）。
fn open_file_impl(
    lua: &Lua,
    kind: Option<String>,
    start_dir: Option<String>,
    on_ui_thread: bool,
) -> mlua::Result<mlua::MultiValue> {
    if let Some(reason) = modal_block(on_ui_thread) {
        return lua_nil_err(lua, reason.to_string());
    }
    let k = kind.unwrap_or_default();
    let Some(pick) = crate::platform::file_dialog::ScriptPick::parse(&k) else {
        return lua_nil_err(
            lua,
            format!("没有这个文件档：{k}（可用 any / image / audio / gif / mp4）"),
        );
    };
    path_value(
        lua,
        crate::platform::file_dialog::script_open(
            pick,
            opt_path(start_dir).as_deref(),
            owner_hwnd(),
        ),
    )
}

/// `ext:save_file` 的实体，同 [`open_file_impl`] 一条接线与一条防自锁判据。
fn save_file_impl(
    lua: &Lua,
    name: Option<String>,
    ext: Option<String>,
    start_dir: Option<String>,
    on_ui_thread: bool,
) -> mlua::Result<mlua::MultiValue> {
    if let Some(reason) = modal_block(on_ui_thread) {
        return lua_nil_err(lua, reason.to_string());
    }
    path_value(
        lua,
        crate::platform::file_dialog::script_save(
            &ext.unwrap_or_default(),
            &name.unwrap_or_default(),
            opt_path(start_dir).as_deref(),
            owner_hwnd(),
        ),
    )
}

/// 对话框结果 → Lua 值：选到 = 绝对路径；没选到 = `nil`。
///
/// "用户取消"与"对话框没开起来"对脚本是同一件事（都没拿到文件），合成一档是**有意**的：
/// 后者宿主已经进日志（`file_dialog::log_ext_error` 带着 `CommDlgExtendedError` 的码），
/// 再往 Lua 里塞一个错误码只会被每份脚本各自 `if` 一遍。
fn path_value(lua: &Lua, picked: Option<std::path::PathBuf>) -> mlua::Result<mlua::MultiValue> {
    let v = match picked {
        Some(path) => {
            mlua::Value::String(lua.create_string(path.display().to_string().as_bytes())?)
        }
        None => mlua::Value::Nil,
    };
    Ok(mlua::MultiValue::from_vec(vec![v]))
}

/// 扩展 API 门面 userdata（`ext`）。无状态；所有状态走线程局部 [`EXT_CTX`]。
pub struct ExtApi;

impl UserData for ExtApi {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // ext:get_image() -> ExtImage | nil：当前贴图（贴图动作里才有）。
        methods.add_method("get_image", |lua, _this, ()| {
            let img = EXT_CTX.with(|c| c.borrow().as_ref().map(|c| c.image.clone()));
            match img {
                Some(img) => lua
                    .create_userdata(ExtImage { img })
                    .map(mlua::Value::UserData),
                None => Ok(mlua::Value::Nil),
            }
        });
        // ext:result(text) -> 宿主模态结果窗（长文本会换行、可圈选，另有「复制」按钮）。
        // ★ `add_method`（不是 `add_function`）：脚本里一律 `ext:result(...)`（冒号）调用，
        //   冒号会把 `self`(=ext userdata) 当第一实参塞进闭包；用 `add_method` 让 `_this`
        //   吃掉它，否则 `text` 收到的是 userdata → "error converting Lua userdata to String"
        //   （2026-09-29 实测：这会导致整个 AI 对话脚本每步都报错、功能全废）。
        //   闭包体再套 `catch_unwind`：后台线程里弹窗一旦真 panic（桌面特定条件），
        //   转成 Lua error 而非 abort 掉整个进程。
        methods.add_method("result", |_lua, _this, text: String| {
            let hwnd = EXT_CTX.with(|c| c.borrow().as_ref().map(|c| c.hwnd).unwrap_or(0)) as isize;
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                show_result_blocking("脚本结果", &text, hwnd);
            }));
            if r.is_err() {
                return Err(mlua::Error::external("结果框异常退出（已避免程序崩溃）"));
            }
            Ok(())
        });
        // ext:log(msg) -> 走 Rust 日志，方便排障。
        methods.add_method("log", |_lua, _this, msg: String| {
            log::info!("[ext] {msg}");
            Ok(())
        });
        // ext:proc{exe, args, cwd, timeout_ms, ansi} -> table | nil, err：跑**脚本自己目录里**的
        // 外部程序（m10「外置工具」通道）。
        //
        // 一条调用，一次等待：脚本线程阻塞到子进程退出（或超时被宿主杀掉），UI 线程零等待
        // （与 `ext:http` 同一条线程语义）。返回
        // `{ code=int|nil, stdout=string, stderr=string, timed_out=bool, truncated=bool,
        //    elapsed_ms=int }`；`elapsed_ms` 是**整次调用**的墙钟（含起进程、加载 dll、跑任务），
        // 这才是用户感到的延迟——工具自己报的内部耗时不是同一个数（m09 实测过这条差别）。
        //
        // `ansi = true` ⇒ 按系统码页（中文系统 = GBK）解输出，默认按 UTF-8。只能由脚本声明，
        // 因为宿主猜不到对面那工具吐什么：两边都实测过——UTF-8 的工具（RapidOCR CLI）标了
        // `ansi` 就出来"鏂囧瓧璇嗗埆"，国标码的工具按 UTF-8 读就是一串替换字符
        // （`examples/ext_proc_ocr.rs` 的臂 1b 专门钉这条）。
        //
        // 一条调用，一次等待：脚本线程阻塞到子进程退出（或超时被宿主杀掉），UI 线程零等待
        // （与 `ext:http` 同一条线程语义）。返回
        // `{ code=int|nil, stdout=string, stderr=string, timed_out=bool, truncated=bool,
        //    elapsed_ms=int }`；`elapsed_ms` 是**整次调用**的墙钟（含起进程、加载 dll、跑任务），
        // 这才是用户感到的延迟——工具自己报的内部耗时不是同一个数（m09 实测过这条差别）。
        //
        // ★ 为什么 `exe` 只能是相对路径：这条 API 等价于"在本机执行任意代码"，路径收口只有一道，
        //   写在 [`resolve_tool`]——**只能是脚本自己目录（含子目录）里的 .exe**。绝对路径、
        //   `..`、`.dll` 一律拒绝。工具要的外部文件（dll / 模型）跟着脚本目录放即可。
        //   这一道防的是手滑与误装，不是恶意脚本（`ext:write_file` 能把 exe 写进那个目录），
        //   口径见那里的注释。
        methods.add_method("proc", |lua, _this, opts: mlua::Table| {
            // `nil, 原因` 的形状只在 [`lua_nil_err`] 里写一遍
            let fail = |lua: &Lua, msg: String| lua_nil_err(lua, msg);
            if let Some(why) = blocking_io_block() {
                return fail(lua, why.to_string());
            }
            let exe: String = match opts.get("exe") {
                Ok(s) => s,
                Err(_) => {
                    return fail(
                        lua,
                        "proc 需要 exe 字段（脚本目录里的相对路径）".to_string(),
                    )
                }
            };
            let Some(dir) = script_dir() else {
                return fail(lua, "拿不到脚本目录（宿主内部状态异常）".to_string());
            };
            let tool = match resolve_tool(&dir, &exe) {
                Ok(p) => p,
                Err(e) => return fail(lua, e),
            };
            // args：只收 1..n 的字符串数组。绝不拼命令行——参数按数组逐个交给子进程。
            let mut args: Vec<String> = Vec::new();
            if let Ok(t) = opts.get::<mlua::Table>("args") {
                for i in 1..=t.raw_len() {
                    match t.get::<mlua::Value>(i) {
                        Ok(mlua::Value::String(s)) => args.push(s.to_string_lossy()),
                        Ok(mlua::Value::Integer(n)) => args.push(n.to_string()),
                        _ => {
                            return fail(
                                lua,
                                format!("args[{i}] 只能是字符串或整数（不接受表/函数）"),
                            )
                        }
                    }
                }
            }
            let cwd = match opts.get::<Option<String>>("cwd") {
                Ok(Some(rel)) => match super::fs_api::resolve(&dir, &rel) {
                    Some(p) if p.is_dir() => Some(p),
                    Some(_) => return fail(lua, format!("proc 的 cwd 不是目录：{rel}")),
                    None => return fail(lua, format!("proc 的 cwd 越出脚本目录：{rel}")),
                },
                _ => None,
            };
            let timeout_ms = opts
                .get::<Option<i64>>("timeout_ms")
                .ok()
                .flatten()
                .filter(|n| *n > 0)
                .unwrap_or(crate::platform::process::DEFAULT_TIMEOUT_MS as i64)
                as u64;
            let ansi = opts
                .get::<Option<bool>>("ansi")
                .ok()
                .flatten()
                .unwrap_or(false);
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::platform::process::run(crate::platform::process::RunSpec {
                    exe: &tool,
                    args: &args,
                    cwd: cwd.as_deref(),
                    timeout_ms,
                    decode_ansi: ansi,
                })
            }));
            let out = match r {
                Ok(o) => o,
                Err(_) => return fail(lua, "外部程序调用异常（已避免程序崩溃）".to_string()),
            };
            let t = lua.create_table()?;
            match out.code {
                Some(c) => t.set("code", c)?,
                None => t.set("code", mlua::Value::Nil)?,
            }
            t.set("stdout", out.stdout.as_str())?;
            t.set("stderr", out.stderr.as_str())?;
            t.set("timed_out", out.timed_out)?;
            t.set("truncated", out.truncated)?;
            t.set("elapsed_ms", out.elapsed_ms as i64)?;
            Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Table(t)]))
        });
        // ext:script_dir() -> string：脚本自己所在目录的绝对路径（拼给外部程序用——
        // 它们多数只认绝对路径，m09 实测：CLI 以自己为工作目录，相对路径直接找不到图）。
        methods.add_method("script_dir", |lua, _this, ()| {
            let s = script_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            lua.create_string(s.as_bytes())
        });
        // ext:data_dir() -> string：本脚本自己的持久数据目录（`<exe>/data/lua/<id>/`）。
        // 不在 assets 下，写它不会触发目录监听；用户设置仍走 config.lua_ext.params。
        methods.add_method("data_dir", |lua, _this, ()| {
            let id = script_id()
                .ok_or_else(|| mlua::Error::external("脚本还没加载，没有脚本 id".to_string()))?;
            let dir = crate::core::paths::script_data_dir(&id);
            lua.create_string(dir.to_string_lossy().as_bytes())
        });
        // ext:tmpdir() -> string：宿主专属的临时目录（绝对路径，已创建）。
        // 脚本要"把位图落成文件给外部程序读、再读回它写的东西"就用它，不污染脚本目录。
        methods.add_method("tmpdir", |lua, _this, ()| {
            let d = tmpdir_path();
            std::fs::create_dir_all(&d)
                .map_err(|e| mlua::Error::external(format!("创建临时目录失败：{e}")))?;
            lua.create_string(d.display().to_string().as_bytes())
        });
        // ext:open_file(kind?, start_dir?) -> string | nil | nil, err：原生「打开」对话框（m13）。
        // 返回选中的**绝对路径**；三档要分清：取消 = `nil`（"没选"，不是错误，脚本自己收尾），
        // `kind` 不在词表 / 在逐帧脚本里调用 = `nil, 原因`。
        // `kind` = any（默认）/ image / audio / gif / mp4，**只影响过滤器**。
        // ★ `start_dir` 允许绝对路径（与 `ext:proc` 的 `exe` 正好相反）：选哪个文件是用户在
        //   对话框里做的，起始目录只是提示，没有"脚本能碰到哪儿"的安全含义。
        methods.add_method(
            "open_file",
            |lua, _this, (kind, start_dir): (Option<String>, Option<String>)| {
                open_file_impl(lua, kind, start_dir, crate::core::ui_bus::on_ui_thread())
            },
        );
        // ext:save_file(name?, ext?, start_dir?) -> string | nil：原生「另存为」。
        // `ext` 缺省或畸形一律收成 `txt`（净化见 `file_dialog::sanitize_ext`）。
        // 拿到的路径可能指向已存在的文件——覆盖确认由系统在对话框里问，脚本不必再自查存在性。
        methods.add_method(
            "save_file",
            |lua,
             _this,
             (name, ext, start_dir): (Option<String>, Option<String>, Option<String>)| {
                save_file_impl(
                    lua,
                    name,
                    ext,
                    start_dir,
                    crate::core::ui_bus::on_ui_thread(),
                )
            },
        );
        // ext:read_file(path) -> string | nil, err：读**任意路径**的字节（m14「回程」第一格）。
        // 与 `ctx.fs:read` 的分工就在"任意路径"这四个字上：那个限脚本目录（防手滑的纪律），
        // 这个不限——外部工具的产物落在哪儿都取回来了。以前只能指望工具自己打 stdout，
        // 而它写文件的时候（RapidOCR 就是写 `-result.txt`）脚本一点办法都没有。
        // 返回 Lua 二进制安全字符串，**字节原样、不猜编码**（要按码页解的走 `ext:proc` 的 `ansi`）。
        // 超过 [`MAX_READ`] 一律拒，不做静默截断：被剪掉的字节当图片/JSON 用，症状是
        // "这文件坏了"，而不是"它太大"——后者至少能看懂。
        methods.add_method("read_file", |lua, _this, path: String| {
            let p = std::path::PathBuf::from(path.trim());
            if p.as_os_str().is_empty() {
                return lua_nil_err(lua, "read_file 需要路径".to_string());
            }
            let meta = match std::fs::metadata(&p) {
                Ok(m) => m,
                Err(e) => return lua_nil_err(lua, format!("读不到 {}：{e}", p.display())),
            };
            if !meta.is_file() {
                return lua_nil_err(lua, format!("{} 不是文件", p.display()));
            }
            if meta.len() > MAX_READ {
                return lua_nil_err(
                    lua,
                    format!(
                        "{} 有 {} MB，超过单次读取上限 {} MB（脚本该分段读或换个更小的产物）",
                        p.display(),
                        meta.len() / 1048576,
                        MAX_READ / 1048576
                    ),
                );
            }
            match std::fs::read(&p) {
                Ok(bytes) => Ok(mlua::MultiValue::from_vec(vec![mlua::Value::String(
                    lua.create_string(&bytes)?,
                )])),
                Err(e) => lua_nil_err(lua, format!("读 {} 失败：{e}", p.display())),
            }
        });
        // ext:write_file(path, data) -> true | nil, err：写文件（任意路径，父目录自动建）。
        // 有了 `ext:save_file` 选出来的落点，这一步才写得出去——在那之前脚本**只有**
        // `img:save`（只 PNG），等于给了路径也存不下文本。
        // ★ 写完**回读核长度**：`fs::write` 报 Ok 不等于盘上就是这些字节（占位云盘 /
        //   同步杀软是这条路上唯一会骗人的一环）。对不上就报原因，不报"写好了"
        //   ——同 `ext:clip_set` 一条纪律。
        methods.add_method("write_file", |lua, _this, (path, data): (String, mlua::LuaString)| {
            let p = std::path::PathBuf::from(path.trim());
            if p.as_os_str().is_empty() {
                return lua_nil_err(lua, "write_file 需要路径与内容".to_string());
            }
            if let Some(parent) = p.parent() {
                if !parent.as_os_str().is_empty() && !parent.exists() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        return lua_nil_err(lua, format!("建不了目录 {}：{e}", parent.display()));
                    }
                }
            }
            let bytes = data.as_bytes();
            let want = bytes.len() as u64;
            if let Err(e) = std::fs::write(&p, bytes) {
                return lua_nil_err(lua, format!("写 {} 失败：{e}", p.display()));
            }
            let len = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(u64::MAX);
            if len != want {
                return lua_nil_err(
                    lua,
                    format!("写完了但盘上是 {len} 字节，期望 {want} 字节（被谁改了？先别用这份文件）"),
                );
            }
            Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Boolean(true)]))
        });
        // ext:open_temp(bytes, ext) -> path | nil, err：把字节写成临时图片并用系统默认程序打开。
        // ★ 扩展名白名单只放光栅图；二维码内容不可信，绝不让它借 `.exe` / `.lnk` / `.url`
        //   变成一次执行。文件落在 `ext:tmpdir()`，不碰用户自己的文件。
        // ★ 写盘同样回读长度；打开失败时文件仍在，错误里带路径，用户还能手动打开。
        methods.add_method(
            "open_temp",
            |lua, _this, (data, ext): (mlua::LuaString, String)| {
                use std::sync::atomic::{AtomicU64, Ordering};
                static SEQ: AtomicU64 = AtomicU64::new(1);
                let ext = ext.trim().to_ascii_lowercase();
                if !open_temp_ext_allowed(&ext) {
                    return lua_nil_err(
                        lua,
                        format!("open_temp 只允许光栅图片扩展名，不接受：{ext}"),
                    );
                }
                let path = tmpdir_path().join(format!(
                    "barcode_{}_{}.{}",
                    std::process::id(),
                    SEQ.fetch_add(1, Ordering::Relaxed),
                    ext
                ));
                let want = data.as_bytes().len() as u64;
                if let Err(e) = std::fs::write(&path, data.as_bytes()) {
                    return lua_nil_err(lua, format!("写临时图片失败：{e}"));
                }
                let len = std::fs::metadata(&path)
                    .map(|m| m.len())
                    .unwrap_or(u64::MAX);
                if len != want {
                    return lua_nil_err(
                        lua,
                        format!("临时图片写完后是 {len} 字节，期望 {want} 字节"),
                    );
                }
                if !crate::platform::win_util::open_with_default(&path) {
                    return lua_nil_err(
                        lua,
                        format!("图片已保存到 {}，但系统没能打开它", path.display()),
                    );
                }
                Ok(mlua::MultiValue::from_vec(vec![mlua::Value::String(
                    lua.create_string(path.to_string_lossy().as_bytes())?,
                )]))
            },
        );
        // ext:open_url(url) -> true | nil, err：只允许 http/https，交给默认浏览器。
        methods.add_method("open_url", |lua, _this, url: String| {
            match crate::platform::win_util::open_url(&url) {
                Ok(()) => Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Boolean(true)])),
                Err(e) => lua_nil_err(lua, e),
            }
        });
        // ext:load_image(path) -> ExtImage | nil, err：把文件读成图片对象（m14 回程第三格）。
        // 这一格才让"外部工具改了张图"接得回脚本：`ext:proc` 跑完 → `ext:load_image(产物)`
        // → 像素、`png_base64`、再喂下一个工具。
        // ★ **按原尺寸解**（m21）：走 `platform::image_decode` 那条常驻 worker + WIC，
        //   支持面以 WIC 在本机的解码器为准（PNG/JPEG/BMP/GIF/TIFF 通常都有；格式不认识 /
        //   本机缺解码器 / 文件坏掉这三种，WIC 只报同一个失败——所以原因里如实写"三种之一"，
        //   不假装分得清）。为什么不在脚本线程就地解：COM 是线程级的，那里先按 MTA 起 WIC
        //   会让之后 `ext:open_file` 的 STA 初始化失败（`examples/wic_decode_probe.rs` 的读数）。
        methods.add_method("load_image", |lua, _this, path: String| {
            let p = std::path::PathBuf::from(path.trim());
            let shot = match crate::platform::image_decode::decode(&p) {
                Ok(s) => s,
                Err(e) => return lua_nil_err(lua, e),
            };
            // ★ 用**保 α** 那一档：文件里的透明是内容，`shot_to_bitmap` 会把 α 一律刷成
            //   255（那是抓屏的语义），用错就是"去背景白做了"。对拍测试见本模块 `load_image_roundtrips…`。
            let img = crate::core::backdrop::shot_to_bitmap_alpha(&shot)
                .ok_or_else(|| mlua::Error::external("转换位图格式失败".to_string()))?;
            let v = ext_image_value(lua, img)?;
            Ok(mlua::MultiValue::from_vec(vec![v]))
        });
        // ext:set_image(源) -> true | nil, err：把**眼前这张贴图**换成另一张图（m15 回写）。
        // 源可以是 `ExtImage`（`ctx.image` / `ext:get_image()` / `ext:load_image(p)` 给的都算），
        // 也可以直接给一个 **PNG 路径字符串**（等价于先 `load_image` 再换）。
        //
        // ★ 语义与右键「旋转 / 镜像」**同一条缝**（`runtime::layer::pin::set_canvas_bitmap`），
        //   所以行为也和它们一致，三条要说清：
        //   · 只换**视图**——Ctrl+S 与贴图环用的那份**出厂原图**不动（那本来就是旋转/镜像的
        //     既有语义，不是回写新引入的分叉）；
        //   · **不写盘**：要留结果就显式 `ext:save_file` 选落点，或贴图右键「另存为」；
        // 尺寸不同会自动跟换图一起重算窗口（= 图 × 当前缩放 + 2px 描边，左上角不动）。
        // ★ m16 起有退路：像素与出厂那份不同时，贴图右键会出现「恢复原图」，脚本侧不必自己留原图。
        // 没有贴图上下文（脚本页里直接跑、或热键触发时没选中贴图）⇒ `nil, 原因`，不去猜是哪一张。
        methods.add_method("set_image", |lua, _this, src: mlua::Value| {
            let fail = |lua: &Lua, msg: String| lua_nil_err(lua, msg);
            let Some(ctx) = EXT_CTX.with(|c| c.borrow().as_ref().cloned()) else {
                return fail(
                    lua,
                    "没有贴图上下文：这一项要在贴图上右键 → 脚本 里跑".to_string(),
                );
            };
            if ctx.hwnd == 0 {
                return fail(lua, "拿不到这张贴图的窗口句柄，无法回写".to_string());
            }
            // 取像素：userdata 走 ExtImage，字符串走"路径 → load_png"。别的类型直接拒，
            // 不静默当空图（表传错了却换出一张 1×1，症状是"贴图忽然变小"）。
            let bmp: Option<RgbaBitmap> = match &src {
                mlua::Value::UserData(ud) => ud.borrow::<ExtImage>().ok().map(|i| i.img.clone()),
                mlua::Value::String(s) => {
                    let p = std::path::PathBuf::from(s.to_string_lossy().trim());
                    // 与 `ext:load_image` 同一条读图路（原尺寸、WIC、保 α）。
                    match crate::platform::image_decode::decode(&p) {
                        Ok(shot) => crate::core::backdrop::shot_to_bitmap_alpha(&shot),
                        Err(e) => return fail(lua, format!("回写没拿到像素：{e}")),
                    }
                }
                _ => None,
            };
            let path_given = matches!(src, mlua::Value::String(_));
            let Some(bmp) = bmp else {
                let what = if path_given {
                    "给的图片文件读不出像素".to_string()
                } else {
                    "要 ExtImage 或图片路径字符串".to_string()
                };
                return fail(lua, format!("回写没拿到像素：{what}"));
            };
            if bmp.w == 0 || bmp.h == 0 {
                return fail(lua, "回写的图是 0 尺寸，不换".to_string());
            }
            let (spec, rx) = crate::core::events::ScriptImageSpec::new(
                ctx.hwnd as isize,
                std::sync::Arc::new(bmp),
            );
            use crate::core::ui_bus::ImageOutcome;
            match crate::core::ui_bus::set_image(spec, rx) {
                ImageOutcome::Applied(Ok(())) => {
                    Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Boolean(true)]))
                }
                ImageOutcome::Applied(Err(e)) => fail(lua, e),
                // 逐帧脚本：动作排下了，但这一帧还没换。如实说，别报"已经换好了"。
                ImageOutcome::Queued => Ok(mlua::MultiValue::from_vec(vec![
                    mlua::Value::Boolean(true),
                    mlua::Value::String(
                        lua.create_string("逐帧脚本里排到下一次派发才生效（本帧还没换）")?,
                    ),
                ])),
                ImageOutcome::Unavailable => fail(
                    lua,
                    "宿主的 UI 通道没起来，回写没送到（应用正在退出？）".to_string(),
                ),
            }
        });
        // ext:clip_get() -> string | nil：读剪贴板里的文本（没有文本 = nil）。
        methods.add_method(
            "clip_get",
            |lua, _this, ()| match crate::platform::clipboard::get_text() {
                Some(s) => Ok(mlua::Value::String(lua.create_string(s.as_bytes())?)),
                None => Ok(mlua::Value::Nil),
            },
        );
        // ext:clip_set(text) -> true | 抛错：把文本写进剪贴板（识别完的文字交回粘贴目标）。
        // ★ **写完读回来核一遍**才报成功：底层 `copy_text` 不返回结果（失败只落日志），
        //   而"剪贴板里其实没变"这种事绝不能由脚本替我们宣布成功（同一条不许假成功的纪律）。
        // ext:clip_image() -> ExtImage | nil（m27）：剪贴板里的**位图**（CF_DIB）。
        // 让"截图 → 分析"不用先钉成贴图：Win+Shift+S / QQ 截图的结果就在剪贴板里，
        // 直接 `ext:clip_image()` → 取色板 / 对比度 / OCR 识别与清洗 / img:crop → img:save。
        // ★ 没有图像 ⇒ `nil`（与 `clip_get` 没有文本同一条纪律）；读失败（畸形 DIB、被独占）
        //   也归到 `nil`：底层 `clipboard::get_image` 本来就不区分，Lua 层不假装分得清。
        methods.add_method("clip_image", |lua, _this, ()| {
            clip_image_value(lua, crate::platform::clipboard::get_image)
        });
        methods.add_method("clip_set", |_lua, _this, text: String| {
            crate::platform::clipboard_history::copy_text(&text);
            match crate::platform::clipboard::get_text() {
                Some(back) if back == text => Ok(true),
                Some(other) => Err(mlua::Error::external(format!(
                    "剪贴板写入后读回的不是这段内容（前 20 字：{:?}）",
                    other.chars().take(20).collect::<String>()
                ))),
                None => Err(mlua::Error::external(
                    "剪贴板写入后读不到文本（可能被别的程序占着）",
                )),
            }
        });
        // ext:input(title, default) -> string | nil：宿主模态输入窗（多行、Enter 换行）。
        // 与 ext:result 同一条线程语义——在工作线程阻塞等用户，UI 线程零等待。
        // 确定 = 返回输入文本；取消 / Esc / 关窗 = 返回 nil（脚本据此退出循环）。
        methods.add_method(
            "input",
            |_lua, _this, (title, default): (String, Option<String>)| {
                let hwnd =
                    EXT_CTX.with(|c| c.borrow().as_ref().map(|c| c.hwnd).unwrap_or(0)) as isize;
                let initial = default.unwrap_or_default();
                // 套 `catch_unwind`：后台线程弹窗一旦真 panic（桌面特定条件），转成
                // Lua error 而非 abort 掉整个进程（2026-09-29「脚本热键秒退」兜底）。
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    input_blocking(&title, &initial, hwnd)
                }));
                match r {
                    Ok(v) => Ok(v),
                    Err(_) => Err(mlua::Error::external("输入框异常退出（已避免程序崩溃）")),
                }
            },
        );
        // ext:json_decode(text) -> table | nil：JSON 文本 → Lua 表（数组 → 1 起始序列表）。
        // 解析失败 / 非法 JSON → nil（脚本自己兜底，不抛错）。
        methods.add_method(
            "json_decode",
            |lua, _this, text: String| match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(v) => json_to_lua(lua, &v),
                Err(_) => Ok(mlua::Value::Nil),
            },
        );
        // ext:json_encode(value) -> string：Lua 表 / 标量 → JSON 文本。数字、布尔、字符串
        // 直转；表按"1..n 连续整数键 = 数组、其余 = 对象"分流；nil / 函数等不支持的值 → null。
        methods.add_method("json_encode", |lua, _this, v: mlua::Value| {
            let s = serde_json::to_string(&lua_to_json(&v)).unwrap_or_default();
            Ok(mlua::Value::String(lua.create_string(s.as_bytes())?))
        });
        // ext:http(method, url, headers, body) -> { status=int, body=string }（阻塞 HTTP）。
        methods.add_method(
            "http",
            |lua, _this, (method, url, headers, body): (String, String, mlua::Table, Option<String>)| {
                // ★ 阻塞网络不许在 UI 线程上跑（draw 模型）：宁可回一条错误，也不冻屏。
                if let Some(why) = blocking_io_block() {
                    let out = lua.create_table()?;
                    out.set("status", 0)?;
                    out.set("body", why)?;
                    return Ok(out);
                }
                let mut hdrs: Vec<(String, String)> = Vec::new();
                for (k, v) in headers.pairs().flatten() {
                    if let (mlua::Value::String(k), mlua::Value::String(v)) = (k, v) {
                        hdrs.push((k.to_string_lossy(), v.to_string_lossy()));
                    }
                }
                // 套 `catch_unwind`：HTTP 走阻塞网络 + ureq，任何意外都转成
                // status=0 的错误响应，脚本据此显示失败而不是让进程 abort（2026-09-29 兜底）。
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    http_blocking(&method, &url, &hdrs, body.as_deref())
                }));
                let (status, body_s) = match r {
                    Ok(v) => v,
                    Err(_) => (0, "请求线程异常（已避免程序崩溃）".to_string()),
                };
                let out = lua.create_table()?;
                out.set("status", status)?;
                out.set("body", body_s)?;
                Ok(out)
            },
        );
        // ext:download(url, path[, headers]) -> 字节数 | nil, 原因（m32）。
        // 流式写盘（`.partial` → 原子改名，失败清半成品），上限 512 MB。
        // ★ 阻塞：只能从 `run` 模型的脚本里调（`draw` 跑在 UI 线程，网络会冻全屏）——
        //   与 `ext:http` 同一条线程纪律。★ 地址不受限制（脚本是用户自己的代码，
        //   信任模型见 API.md）；要下什么、从哪下是脚本作者的责任。
        methods.add_method(
            "download",
            |lua, _this, (url, path, headers): (String, String, Option<mlua::Table>)| {
                if let Some(why) = blocking_io_block() {
                    return lua_nil_err(lua, why.to_string());
                }
                let mut hdrs: Vec<(String, String)> = vec![(
                    "User-Agent".to_string(),
                    crate::platform::http::DEFAULT_UA.to_string(),
                )];
                if let Some(t) = &headers {
                    for (k, v) in t.pairs().flatten() {
                        if let (mlua::Value::String(k), mlua::Value::String(v)) = (k, v) {
                            hdrs.push((k.to_string_lossy(), v.to_string_lossy()));
                        }
                    }
                }
                let p = std::path::PathBuf::from(path.trim());
                if p.as_os_str().is_empty() {
                    return lua_nil_err(lua, "没给落盘路径".to_string());
                }
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    crate::platform::http::download_to_file(
                        url.trim(),
                        &p,
                        &hdrs,
                        crate::platform::http::SCRIPT_DOWNLOAD_MAX,
                        |_, _| {},
                    )
                }));
                match r {
                    Ok(Ok(n)) => Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Integer(
                        i64::try_from(n).unwrap_or(i64::MAX),
                    )])),
                    Ok(Err(e)) => lua_nil_err(lua, e),
                    Err(_) => lua_nil_err(lua, "下载线程异常（已避免程序崩溃）".to_string()),
                }
            },
        );
        // ext:unzip(zip, dest_dir) -> 文件数 | nil, 原因（m32）。
        // 只认 stored / deflate；`..`、绝对路径、盘符、符号链接一律拒（防手滑）；
        // CRC 不符 / 超限 / 加密条目都拒，绝不留半成品。
        methods.add_method("unzip", |lua, _this, (zip, dest): (String, String)| {
            let z = std::path::PathBuf::from(zip.trim());
            let d = std::path::PathBuf::from(dest.trim());
            if z.as_os_str().is_empty() || d.as_os_str().is_empty() {
                return lua_nil_err(lua, "要两个参数：压缩包路径、目标目录".to_string());
            }
            match crate::platform::unzip::extract_zip(&z, &d) {
                Ok(n) => Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Integer(
                    i64::from(n),
                )])),
                Err(e) => lua_nil_err(lua, e),
            }
        });
        // ext:chat_window(url, model, key, sys)：Pecia 式原生 AI 对话窗（顶部对话区 /
        // 中部「清除・发送」/ 底部输入框；Ctrl+Enter 发送、Esc 关窗、等待时标题栏读秒）。
        // 阻塞至关窗（与 ext:result / ext:input 同一条线程语义）；HTTP 在窗口的 worker
        // 线程里跑，界面全程不卡。多轮上下文在窗口内自持，关窗即弃。
        methods.add_method(
            "chat_window",
            |_lua, _this, (url, model, key, sys): (String, String, String, String)| {
                let hwnd = EXT_CTX.with(|c| c.borrow().as_ref().map(|c| c.hwnd).unwrap_or(0));
                // 套 `catch_unwind`：后台线程弹原生窗一旦真 panic（桌面特定条件），
                // 转成 Lua error 而非 abort 掉整个进程（同 ext:result / ext:input 兜底）。
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    crate::platform::ai_chat::chat_window(hwnd as isize, &url, &model, &key, &sys);
                }));
                if r.is_err() {
                    return Err(mlua::Error::external(
                        "AI 对话窗口异常退出（已避免程序崩溃）",
                    ));
                }
                Ok(())
            },
        );
    }
}

/// `serde_json::Value` → Lua 值（`ext:json_decode` 的主体）。
fn json_to_lua(lua: &Lua, v: &serde_json::Value) -> mlua::Result<mlua::Value> {
    use serde_json::Value as J;
    Ok(match v {
        J::Null => mlua::Value::Nil,
        J::Bool(b) => mlua::Value::Boolean(*b),
        J::Number(n) => match n.as_f64() {
            Some(f) => mlua::Value::Number(f),
            None => mlua::Value::Nil,
        },
        J::String(s) => mlua::Value::String(lua.create_string(s)?),
        J::Array(a) => {
            let t = lua.create_table()?;
            for (i, item) in a.iter().enumerate() {
                t.set(i + 1, json_to_lua(lua, item)?)?;
            }
            mlua::Value::Table(t)
        }
        J::Object(o) => {
            let t = lua.create_table()?;
            for (k, val) in o {
                t.set(k.as_str(), json_to_lua(lua, val)?)?;
            }
            mlua::Value::Table(t)
        }
    })
}

/// Lua 值 → `serde_json::Value`（`ext:json_encode` 的主体）。
///
/// 表的分流判据：`1..=len` 每个位置都有值 → 数组；否则按字符串键 → 对象。
/// 递归深度极浅的脚本数据够用；函数 / userdata 之类不支持的值一律落 `null`。
fn lua_to_json(v: &mlua::Value) -> serde_json::Value {
    use serde_json::Value as J;
    match v {
        mlua::Value::Nil => J::Null,
        mlua::Value::Boolean(b) => J::Bool(*b),
        mlua::Value::Integer(i) => J::Number((*i).into()),
        mlua::Value::Number(f) => serde_json::Number::from_f64(*f)
            .map(J::Number)
            .unwrap_or(J::Null),
        mlua::Value::String(s) => J::String(s.to_string_lossy()),
        mlua::Value::Table(t) => {
            let len = t.raw_len();
            let mut arr = Vec::with_capacity(len);
            let mut is_array = len > 0;
            for i in 1..=len {
                match t.get::<mlua::Value>(i) {
                    Ok(val) if !matches!(val, mlua::Value::Nil) => arr.push(lua_to_json(&val)),
                    _ => {
                        is_array = false;
                        break;
                    }
                }
            }
            if is_array {
                return J::Array(arr);
            }
            let mut obj = serde_json::Map::new();
            for pair in t.pairs::<String, mlua::Value>().flatten() {
                let (k, val) = pair;
                obj.insert(k, lua_to_json(&val));
            }
            J::Object(obj)
        }
        _ => J::Null,
    }
}

/// 脚本 → 宿主**模态结果窗**，阻塞到用户关窗（在工作线程调用，UI 线程零等待）。
///
/// 走 [`crate::core::ui_bus`]：windui 的窗只能在 UI 线程开，所以这里只送**数据 + 回复通道**。
/// 拿不到通道（覆盖层还没起、单测里的裸 `Runtime`）就退回原生框——那不是"以防万一"，
/// 而是本层唯一诚实的做法：`ext:result` 在无 UI 线程的环境里也必须跑得完。
pub(crate) fn show_result_blocking(title: &str, body: &str, owner_hwnd: isize) {
    use crate::core::ui_bus::DialogOutcome;
    let (spec, rx) = ScriptDialogSpec::result(title, body, owner_hwnd);
    match crate::core::ui_bus::dialog(spec, rx) {
        DialogOutcome::Closed(_) => {}
        DialogOutcome::Unavailable => {
            crate::platform::win_util::text_result_box(owner_hwnd, title, body)
        }
    }
}

/// 脚本 → 宿主**模态输入窗**，阻塞等回答。`None` = 取消 / Esc / 关窗。
///
/// 语义与旧的 `win_util::text_input_box` 逐字一致（确定 = 交回文本，空串也算一次有效回答，
/// 是否作废由脚本自己判），所以脚本一行都不用改。
pub(crate) fn input_blocking(title: &str, initial: &str, owner_hwnd: isize) -> Option<String> {
    use crate::core::ui_bus::DialogOutcome;
    let (spec, rx) = ScriptDialogSpec::input(title, initial, owner_hwnd);
    match crate::core::ui_bus::dialog(spec, rx) {
        DialogOutcome::Closed(v) => v,
        DialogOutcome::Unavailable => {
            crate::platform::win_util::text_input_box(owner_hwnd, title, initial)
        }
    }
}

/// 阻塞 HTTP。错误 → `(0, 错误信息)`（脚本据此判断失败，不 panic）。
fn http_blocking(
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: Option<&str>,
) -> (i64, String) {
    // ★ 必须用平台层的统一构造点（TLS provider 只在那儿配），不得在此自配 Agent。
    let agent = crate::platform::http::agent(Duration::from_secs(60));
    let m = method.to_uppercase();
    // GET/DELETE（无 body）与 POST/PUT（带 body）在 ureq 3 里是两种不同构造器
    // （WithoutBody / WithBody），不能在同一处 match 统一成一句话——这里每个方法各走
    // 各的构造与发送，返回值再统一解析。
    let result = match m.as_str() {
        "GET" => {
            let mut req = agent.get(url);
            for (k, v) in headers {
                req = req.header(k.as_str(), v.as_str());
            }
            req.call()
        }
        "DELETE" => {
            let mut req = agent.delete(url);
            for (k, v) in headers {
                req = req.header(k.as_str(), v.as_str());
            }
            req.call()
        }
        "POST" => {
            let mut req = agent.post(url).header("Content-Type", "application/json");
            for (k, v) in headers {
                req = req.header(k.as_str(), v.as_str());
            }
            // 在线接口公共约定按 JSON 发；body 没有就发空串（保持类型一致）。
            req.send(body.map(|b| b.to_string()).unwrap_or_default())
        }
        "PUT" => {
            let mut req = agent.put(url).header("Content-Type", "application/json");
            for (k, v) in headers {
                req = req.header(k.as_str(), v.as_str());
            }
            req.send(body.map(|b| b.to_string()).unwrap_or_default())
        }
        _ => {
            return (0, format!("不支持的 HTTP 方法：{m}"));
        }
    };
    match result {
        Ok(resp) => {
            let status = resp.status().as_u16() as i64;
            match resp.into_body().read_to_string() {
                Ok(body) => (status, body),
                Err(e) => (status, format!("读取响应失败：{e}")),
            }
        }
        Err(e) => (0, format!("请求失败：{e}")),
    }
}

/// 把一次像素动作的结果交回 Lua：成功 = 一个新的 `ExtImage`；
/// `nil, 原因` 那个形状仍然只在 [`lua_nil_err`] 里写一遍。
fn image_ok(lua: &Lua, bmp: RgbaBitmap) -> mlua::Result<mlua::MultiValue> {
    let v = ext_image_value(lua, bmp)?;
    Ok(mlua::MultiValue::from_vec(vec![v]))
}

/// `crop` 的边界判据：越界时**点名是哪一角**出的界。
///
/// ★ 刻意不钳制（clamp）：悄悄裁一半的症状是"它有时候少一块"，比当场给一句
///   "要到 (120, 40)，图只有 100×40" 难查得多。脚本作者要的是"我算错了"。
/// `ext:clip_image` 的实现：把"从哪拿一张 `Shot`"做成参数——运行时交
/// `clipboard::get_image`，测试交一张合成图（**不写也不读用户的剪贴板**）。
///
/// ★ α 走**抓屏那一档**（`shot_to_bitmap`，α 一律 255）：CF_DIB 的第四字节不可信
///   （`clipboard::get_image` 已经强制刷成 255）。别"顺手统一"成保 α 那档——
///   那会让所有从应用复制来的截图变成全透明。
fn clip_image_value(
    lua: &Lua,
    get: impl FnOnce() -> Option<crate::platform::screenshot::Shot>,
) -> mlua::Result<mlua::MultiValue> {
    let Some(shot) = get() else {
        // 恰好一个 nil：多返回值会让 `local img, err = ext:clip_image()` 的 err 变成空串，
        // 脚本据此就会把"没图"的判断走错。
        return Ok(mlua::MultiValue::from_vec(vec![mlua::Value::Nil]));
    };
    let bmp = crate::core::backdrop::shot_to_bitmap(&shot)
        .ok_or_else(|| mlua::Error::external("剪贴板图像转位图失败".to_string()))?;
    image_ok(lua, bmp)
}

fn crop_out_of_range(img: &RgbaBitmap, x: i64, y: i64, w: i64, h: i64) -> Option<String> {
    if w <= 0 || h <= 0 {
        return Some(format!("裁出的宽高必须为正：给的是 {w}×{h}"));
    }
    let (iw, ih) = (img.w as i64, img.h as i64);
    if x < 0 || y < 0 {
        return Some(format!("起点不能是负数：给了 ({x}, {y})，图是 {iw}×{ih}"));
    }
    // ★ `checked_add`：脚本能传 `2^63-1` 这种值，直接相加在 debug 下 panic、release 下回绕，
    //   两种都不是我们想要的（m24 审查）。溢出归到"数值太大"这一类，与越界分开说。
    let (Some(x2), Some(y2)) = (x.checked_add(w), y.checked_add(h)) else {
        return Some(format!(
            "给的坐标/尺寸太大，加不出边界：({x}, {y}) + {w}×{h}"
        ));
    };
    if x2 > iw || y2 > ih {
        return Some(format!(
            "裁的矩形出界：从 ({x}, {y}) 起 {w}×{h} 要到 ({x2}, {y2})，图只有 {iw}×{ih}"
        ));
    }
    None
}

/// `ext:get_image()` 返回的贴图 userdata：只读尺寸 + `png_base64()` / `save(path)` /
/// `pixel(x, y)` / `bytes()`，加 m19 的一组**像素动作**
/// （`crop` / `rotate_cw90` / `flip_h` / `flip_v`）。
///
/// ★ 对象本身仍是**只读快照**：动作返回的是**新图**，这张不动。要让屏幕上那张变，
///   必须显式 `ext:set_image(结果)` ——贴图是用户摆在那里的东西，不许被一次计算悄悄换掉。
pub struct ExtImage {
    img: RgbaBitmap,
}

/// 把一张位图包成 `ExtImage` 的 Lua userdata（统一框架里给 `ctx.image` 用，与
/// `ext:get_image()` 同一个对象类型）。
pub fn ext_image_value(lua: &Lua, img: RgbaBitmap) -> mlua::Result<mlua::Value> {
    lua.create_userdata(ExtImage { img })
        .map(mlua::Value::UserData)
}

impl UserData for ExtImage {
    fn add_fields<F: mlua::UserDataFields<Self>>(fields: &mut F) {
        // `.w` / `.h` 只读字段（脚本里 `img.w` 取宽）。字段访问用 `add_fields` 的 getter。
        fields.add_field_method_get("w", |_lua, this| Ok(this.img.w));
        fields.add_field_method_get("h", |_lua, this| Ok(this.img.h));
    }
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("png_base64", |_lua, this, ()| {
            base64_png(&this.img).map_err(mlua::Error::external)
        });
        // save(path[, quality])：**按扩展名分派**（m25）——`.png` 走无损、`.jpg`/`.jpeg` 走有损
        // （quality 1–100，默认 90）。别的扩展名给一句点名"只认哪两种"的原因。
        methods.add_method(
            "save",
            |_lua, this, (path, quality): (String, Option<i64>)| {
                save_image(&this.img, &path, quality).map_err(mlua::Error::external)
            },
        );
        // pixel(x, y) -> r, g, b, a（越界返回全 nil，脚本自己判）。逐点访问适合小图
        // 或稀疏采样；整图遍历请用下面的 bytes()，一次取回原始缓冲。
        methods.add_method("pixel", |_lua, this, (x, y): (i64, i64)| {
            if x < 0 || y < 0 || x >= this.img.w as i64 || y >= this.img.h as i64 {
                return Ok(mlua::MultiValue::new());
            }
            let idx = (y as u32 * this.img.w + x as u32) as usize * 4;
            let px = &this.img.rgba[idx..idx + 4];
            Ok(mlua::MultiValue::from_vec(
                px.iter()
                    .map(|b| mlua::Value::Integer(i64::from(*b)))
                    .collect(),
            ))
        });
        // bytes() -> 原始 RGBA 缓冲（Lua 二进制安全字符串，长度 = w*h*4，行优先）。
        // 采样统计类脚本用它 + string.byte 批量读，比逐点 pixel() 快一个量级。
        methods.add_method("bytes", |lua, this, ()| {
            Ok(mlua::Value::String(lua.create_string(&this.img.rgba)?))
        });
        // —— 像素动作（m19）：只开 `core::image` 里**已经写完、各自带测试**的那几个。
        // 每个都返回**新图**（手里这张是快照，不动）；要让屏幕上那张变，必须显式
        // `ext:set_image(结果)` —— 贴图是用户摆在那里的东西，不许被一次计算悄悄换掉。
        // ★ 刻意**没有**任意缩放：`core` 里没有通用 resize。
        //   为了"能缩放"去引依赖或自研采样器，违反"本体不变、只开放接口"这条立场。
        //   （`RgbaBitmap::magnify` 也不开：它是**放大镜几何**——输出尺寸 = 给定的框大小，
        //     源是框里左上 1/zoom 那块，是已删除的截图放大镜留下的无调用方函数。
        //     把它当"放大图片"交给脚本用是自找困惑。）
        methods.add_method("crop", |lua, this, (x, y, w, h): (i64, i64, i64, i64)| {
            // ★ 这格**必须**先自己判界：`RgbaBitmap::crop` 的既有语义是"钳到图内"
            //   （区域选择要的就是那个），脚本要的却是"我算错了"。在这一层拦下来，
            //   既不改动 core 那条的语义，也不给脚本一个悄悄少一块的结果。
            if let Some(why) = crop_out_of_range(&this.img, x, y, w, h) {
                return lua_nil_err(lua, why);
            }
            match this.img.crop(x as i32, y as i32, w as i32, h as i32) {
                Some(b) => image_ok(lua, b),
                None => lua_nil_err(lua, "裁不出内容（这张图是空的）".to_string()),
            }
        });
        methods.add_method("rotate_cw90", |lua, this, ()| {
            image_ok(lua, this.img.rotate_cw90())
        });
        methods.add_method("flip_h", |lua, this, ()| image_ok(lua, this.img.flip_h()));
        methods.add_method("flip_v", |lua, this, ()| image_ok(lua, this.img.flip_v()));
    }
}

/// base64 编码表。
const B64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// 标准 base64 编码（含 `=` 填充）。无现成 crate，手写一份，够 PNG 用。
fn b64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        let n = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        out.push(B64_TABLE[(n >> 18) as usize & 63] as char);
        out.push(B64_TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64_TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64_TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// RGBA 位图 → base64 编码的 PNG 文本（在线接口用）。复用截图 PNG 编码链。
fn base64_png(img: &RgbaBitmap) -> Result<String, String> {
    let shot =
        crate::core::backdrop::bitmap_to_shot(img).ok_or_else(|| "转换位图格式失败".to_string())?;
    let png =
        crate::platform::screenshot::encode_png(&shot).map_err(|e| format!("PNG 编码失败：{e}"))?;
    Ok(b64(&png))
}

/// RGBA 位图 → 存成 PNG 文件（父目录不存在则创建）。
fn save_png(img: &RgbaBitmap, path: &str) -> Result<(), String> {
    let shot =
        crate::core::backdrop::bitmap_to_shot(img).ok_or_else(|| "转换位图格式失败".to_string())?;
    crate::platform::screenshot::save_png(&shot, std::path::Path::new(path))
        .map_err(|e| format!("保存失败：{e}"))
}

/// `img:save` 的分派：看**扩展名**决定编码器（m25）。
///
/// · `.png` → 无损（`png` crate，走 [`save_png`]）；`quality` 传了就是**错**，不静默忽略；
/// · `.jpg` / `.jpeg` → WIC 的 JPEG 编码器（有损，1–100，默认 90；透明合成白底）；
/// · 其它 / 没有扩展名 → 一句点名"只认这两种"的原因（与 `ext:save_file` 的 `ext` 参数配合用）。
fn save_image(img: &RgbaBitmap, path: &str, quality: Option<i64>) -> Result<(), String> {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("png") => {
            if quality.is_some() {
                return Err("png 是无损格式，没有质量参数（要控制体积就存 .jpg）".to_string());
            }
            save_png(img, path)
        }
        Some("jpg") | Some("jpeg") => {
            let q = quality.unwrap_or(90).clamp(1, 100) as u8;
            let shot = crate::core::backdrop::bitmap_to_shot(img)
                .ok_or_else(|| "转换位图格式失败".to_string())?;
            crate::platform::image_decode::encode_jpeg(&shot, std::path::Path::new(path), q)
                .map_err(|e| format!("保存失败：{e}"))
        }
        _ => Err(format!(
            "只认 .png / .jpg / .jpeg 三种扩展名（给了 `{path}`）——先用 ext:save_file(名字, \"jpg\") 选落点，再 img:save(路径)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::Lua;

    #[test]
    fn no_context_get_image_returns_nil() {
        // 清掉线上残留上下文（测试隔离）：这里线程局部是空的。
        clear_ext_context();
        let lua = Lua::new();
        lua.globals()
            .set("ext", lua.create_userdata(ExtApi).unwrap())
            .unwrap();
        let script = "local i = ext:get_image() return i == nil";
        let ok: bool = lua.load(script).eval().unwrap();
        assert!(ok, "无贴图上下文时应返回 nil");
    }

    /// 走通"脚本 → 总线 → 宿主窗 → 回复"这条链。
    ///
    /// ★ 为什么非要这条测试：`ext:result` / `ext:input` 有两个出口（宿主窗 / 原生框兜底），
    ///   而"功能存在"不等于"功能生效"——不装总线时它们永远走兜底那条路，本仓的门禁全绿也
    ///   看不出窗有没有真开到 UI 线程。这里用一个**替用户点按钮的假 UI**把链路钉住：
    ///   断言窗确实被交出去（标题/可编辑位/正文/预填四项都对），且值确实回到了脚本。
    #[test]
    fn result_and_input_go_through_the_bus_and_come_back() {
        use crate::core::events::UiMsg;
        use std::sync::Mutex;
        static SEEN: Mutex<Vec<(String, bool, String, String)>> = Mutex::new(Vec::new());
        // 回写贴图（m15）那一路记的是"目标句柄 + 新图尺寸"——像素不进台账，太大。
        static SEEN_IMG: Mutex<Vec<(isize, u32, u32)>> = Mutex::new(Vec::new());

        let installed = crate::core::ui_bus::install(move |msg| {
            // 一个 `match` 而不是两个 `if let`：`UiMsg` 按值进来，第一个 `if let` 就把
            // 它部分挪走了，第二个分支根本编不过（编译器替我们守住"只处理一种消息"的错）。
            match msg {
                UiMsg::ScriptDialog(spec) => {
                    SEEN.lock().unwrap().push((
                        spec.title.clone(),
                        spec.editable,
                        spec.body.clone(),
                        spec.initial.clone(),
                    ));
                    // 替用户按：结果窗直接关掉；输入窗回一个值。
                    if spec.editable {
                        spec.reply(Some("已回复".to_string()));
                    } else {
                        spec.reply(None);
                    }
                }
                UiMsg::ScriptImage(spec) => {
                    SEEN_IMG
                        .lock()
                        .unwrap()
                        .push((spec.hwnd, spec.bitmap.w, spec.bitmap.h));
                    // 替 UI 线程回一句"换好了"：这条测的是链路，不是真窗。
                    spec.reply(Ok(()));
                }
                _ => {}
            }
            true
        });
        if !installed {
            // 总线已被别处装走（本测试二进制里不该发生，但**绝不能用阻塞来赌**：
            // 那种情况下等一个不会来的回复会把整个测试挂死）。
            log::warn!("UI 总线已被占用，跳过这条链路断言");
            return;
        }

        // ★ 必须在**另一条线程**上跑：总线记下的是"安装它的线程 = UI 线程"，
        //   同线程调用会走"只开窗不等回答"那条防自锁分支（见 `core::ui_bus::dialog`）。
        let out = std::thread::spawn(|| {
            set_ext_context(ExtCtx {
                image: RgbaBitmap {
                    w: 5,
                    h: 2,
                    rgba: vec![1u8; 5 * 2 * 4],
                },
                hwnd: 0xABCD,
            });
            let lua = Lua::new();
            lua.globals()
                .set("ext", lua.create_userdata(ExtApi).unwrap())
                .unwrap();
            lua.load(
                r#"
                ext:result("尺寸：1 × 1 像素")
                local v = ext:input("改名", "old.png")
                -- 回写贴图：上下文必须在这条线程上给（EXT_CTX 是线程局部的）
                local ok, e = ext:set_image(ext:get_image())
                return tostring(v) .. "|" .. tostring(ok) .. "|" .. tostring(e)
            "#,
            )
            .eval::<String>()
            .unwrap()
        })
        .join()
        .unwrap();

        let (v, ok, e) = {
            let mut it = out.split('|');
            (
                it.next().unwrap_or("").to_string(),
                it.next().unwrap_or("").to_string(),
                it.next().unwrap_or("").to_string(),
            )
        };
        assert_eq!(v, "已回复", "输入窗应当把 UI 侧的回复交回脚本");
        // ★ 回写：Lua → 总线 → UI 侧（这里是替身）→ 回复，整条链跑通，且句柄与尺寸没丢。
        assert_eq!(
            ok, "true",
            "UI 侧回了『换好了』，脚本就该拿到 true，实际：{out}"
        );
        assert_eq!(e, "nil", "成功时不该带原因，实际：{out}");
        let imgs = SEEN_IMG.lock().unwrap();
        assert_eq!(
            imgs.as_slice(),
            &[(0xABCD, 5, 2)][..],
            "送过去的必须是那张贴图的目标句柄与新图尺寸"
        );
        drop(imgs);
        let seen = SEEN.lock().unwrap();
        assert_eq!(seen.len(), 2, "应当正好开出两扇窗：结果 + 输入");
        assert_eq!(
            seen[0],
            (
                ("脚本结果".to_string()),
                false,
                "尺寸：1 × 1 像素".to_string(),
                String::new()
            ),
            "结果窗的标题/可编辑位/正文"
        );
        assert_eq!(seen[1].0, "改名", "输入窗的标题要传过去");
        assert!(seen[1].1, "输入窗必须是可编辑那档");
        assert_eq!(seen[1].3, "old.png", "输入窗的预填值要传过去");
    }

    #[test]
    fn pixel_returns_rgba_and_nil_out_of_range() {
        clear_ext_context();
        // 2×1 图：像素(0,0)=红 (255,0,0,255)，(1,0)=半透明绿 (0,255,0,128)。
        let lua = Lua::new();
        lua.globals()
            .set("ext", lua.create_userdata(ExtApi).unwrap())
            .unwrap();
        set_ext_context(ExtCtx {
            image: RgbaBitmap {
                w: 2,
                h: 1,
                rgba: vec![255, 0, 0, 255, 0, 255, 0, 128],
            },
            hwnd: 0,
        });
        let img: mlua::Value = lua.load("return ext:get_image()").eval().unwrap();
        lua.globals().set("img", img).unwrap();
        let (r, g, b, a): (i64, i64, i64, i64) = lua
            .load("return img:pixel(1, 0)")
            .set_name("t")
            .eval()
            .unwrap();
        assert_eq!((r, g, b, a), (0, 255, 0, 128));
        let (r, g, b, a): (i64, i64, i64, i64) = lua
            .load("return img:pixel(0, 0)")
            .set_name("t")
            .eval()
            .unwrap();
        assert_eq!((r, g, b, a), (255, 0, 0, 255));
        // 越界（负数 / 超宽 / 超高）→ 返回空（全 nil），脚本据此自判。
        for code in [
            "return img:pixel(-1, 0)",
            "return img:pixel(2, 0)",
            "return img:pixel(0, 1)",
        ] {
            let vals: mlua::MultiValue = lua.load(code).eval().unwrap();
            assert!(vals.is_empty(), "{code} 越界应返回空（全 nil）");
        }
    }

    /// ★ m19 的五个像素动作**逐字节对拍**：与手算的期望像素比，不比"看起来像"。
    ///
    /// 夹具是一张 **2×2 且 α 各像素不同**的图 —— 同尺寸能同时抓两件事：α 被刷成实心
    /// （m14 那个真缺陷的同形变体），以及行优先顺序搞反（2×2 是方阵，转置不会露馅，
    /// 但 `flip_v` 与 `flip_h` 的输出会互换，这一格就红）。
    #[test]
    fn image_ops_are_byte_exact_and_keep_alpha() {
        let lua = op_fixture(&[
            255, 0, 0, 255, // (0,0) 红，不透明
            0, 255, 0, 128, // (1,0) 绿，半透明
            0, 0, 255, 0, // (0,1) 蓝，全透明
            0, 0, 0, 255, // (1,1) 黑，不透明
        ]);
        // crop：取左上 1×1 ⇒ 就是那个不透明红
        assert_eq!(
            lua_bytes(&lua, "return img:crop(0, 0, 1, 1):bytes()"),
            vec![255, 0, 0, 255]
        );
        // rotate_cw90：顺时针 ⇒ 顶行变成右列 → (0,0)=左下(蓝 α0)、(1,0)=左上(红)
        assert_eq!(
            lua_bytes(&lua, "return img:rotate_cw90():bytes()"),
            vec![0, 0, 255, 0, 255, 0, 0, 255, 0, 0, 0, 255, 0, 255, 0, 128]
        );
        // flip_h：左右换列
        assert_eq!(
            lua_bytes(&lua, "return img:flip_h():bytes()"),
            vec![0, 255, 0, 128, 255, 0, 0, 255, 0, 0, 0, 255, 0, 0, 255, 0]
        );
        // flip_v：上下换行
        assert_eq!(
            lua_bytes(&lua, "return img:flip_v():bytes()"),
            vec![0, 0, 255, 0, 0, 0, 0, 255, 255, 0, 0, 255, 0, 255, 0, 128]
        );
        // 手里那张是**快照**：动作不改它（`set_image` 才改屏幕，且要显式调）
        assert_eq!(lua_int(&lua, "return img.w"), 2);
        assert_eq!(
            lua_bytes(&lua, "return img:bytes()"),
            vec![255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0, 0, 0, 0, 255]
        );
    }

    /// ★ m25：`img:save` **按扩展名分派**——判据是**落盘文件的魔数**，不是"没报错"。
    /// 写错了（比如两条都走 PNG）这张表会当场露馅：`.jpg` 的文件头必须是 `FF D8`。
    #[test]
    fn save_dispatches_by_extension_and_the_file_magic_proves_it() {
        let dir = ext_proc_dir("save2", &[]);
        let lua = op_fixture(&[1, 2, 3, 255, 4, 5, 6, 255]);
        let jpg = dir.join("a.jpg");
        let png = dir.join("b.png");
        lua.globals().set("jp", jpg.display().to_string()).unwrap();
        lua.globals().set("pp", png.display().to_string()).unwrap();
        lua.load("img:save(jp) img:save(pp)")
            .set_name("t")
            .eval::<()>()
            .unwrap();
        let jb = std::fs::read(&jpg).expect("jpg 该被写出来");
        let pb = std::fs::read(&png).expect("png 该被写出来");
        assert_eq!(&jb[..2], &[0xFF, 0xD8], "jpg 必须是 JPEG 的 SOI");
        assert_eq!(&pb[..4], &[0x89, 0x50, 0x4E, 0x47], "png 必须是 PNG 的签名");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 不相干的扩展名 / 给 png 传 quality：都要**一句原因**，不许静默按 PNG 写。
    #[test]
    fn save_refuses_unknown_extensions_and_quality_on_png() {
        let dir = ext_proc_dir("save3", &[]);
        let lua = op_fixture(&[1, 2, 3, 255, 4, 5, 6, 255]);
        lua.globals()
            .set("wp", dir.join("c.webp").display().to_string())
            .unwrap();
        lua.globals()
            .set("pp", dir.join("d.png").display().to_string())
            .unwrap();
        let e: String = lua
            .load("local ok, err = pcall(function() img:save(wp) end) return tostring(err)")
            .eval()
            .unwrap();
        assert!(
            e.contains(".png") && e.contains("jpg"),
            "要点名支持哪两种：{e}"
        );
        let e2: String = lua
            .load("local ok, err = pcall(function() img:save(pp, 90) end) return tostring(err)")
            .eval()
            .unwrap();
        assert!(e2.contains("无损"), "png 传质量要说清为什么不行：{e2}");
        assert!(!dir.join("c.webp").exists(), "被拒的扩展名不许留下文件");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ★ m27：`ext:clip_image` 的接缝格（**不碰真剪贴板**——测试交一张合成 `Shot`）。
    /// 两件事一起钉：BGR→RGB 顺序对；**α 走抓屏语义**（DIB 里那格写 128，交出来必须是 255）。
    #[test]
    fn clip_image_wraps_a_shot_with_screen_alpha_semantics() {
        let lua = Lua::new();
        lua.globals()
            .set("ext", lua.create_userdata(ExtApi).unwrap())
            .unwrap();
        let shot = crate::platform::screenshot::Shot {
            w: 2,
            h: 1,
            // BGRA：半透明绿 + 实心蓝
            bgra: vec![0, 255, 0, 128, 255, 0, 0, 255],
        };
        let mv = clip_image_value(&lua, move || Some(shot)).unwrap();
        let img = mv.into_iter().next().expect("该有一个值");
        lua.globals().set("img", img).unwrap();
        let (w, h): (i64, i64) = lua.load("return img.w, img.h").eval().unwrap();
        assert_eq!((w, h), (2, 1));
        let bytes: mlua::LuaString = lua.load("return img:bytes()").eval().unwrap();
        assert_eq!(
            bytes.as_bytes().as_ref(),
            &[0, 255, 0, 255, 0, 0, 255, 255],
            "α 必须刷成 255（DIB 的第四字节不可信）、BGR→RGB 顺序对"
        );
    }

    /// 没有图像 ⇒ **恰好一个 nil**（不是 `nil, 原因`）：多一个返回值，脚本里的
    /// `local img, err = ext:clip_image()` 就会把 err 读成空串，"没图"的判断跟着走错。
    #[test]
    fn clip_image_returns_exactly_one_nil_when_there_is_no_image() {
        let lua = Lua::new();
        lua.globals()
            .set("ext", lua.create_userdata(ExtApi).unwrap())
            .unwrap();
        let mv = clip_image_value(&lua, || None).unwrap();
        let vals: Vec<mlua::Value> = mv.into_iter().collect();
        assert_eq!(vals.len(), 1, "只许一个返回值");
        assert!(matches!(vals[0], mlua::Value::Nil), "那一个必须是 nil");
    }

    /// ★ m32：`ext:download` 真的走网络、真的落盘、字节一致——用 **127.0.0.1 的回环
    /// HTTP 服务**（离线可跑），body 故意是 0..=255 循环的二进制（不是 UTF-8 文本）。
    #[test]
    fn download_streams_a_binary_body_to_the_given_path() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().unwrap().port();
        let body: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
        let served = body.clone();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf);
                let head = format!(
                    "HTTP/1.1 200 OK
Content-Length: {}
Connection: close

",
                    served.len()
                );
                let _ = s.write_all(head.as_bytes());
                let _ = s.write_all(&served);
            }
        });
        let dir = ext_proc_dir("dl", &[]);
        let out = dir.join("body.bin");
        let lua = Lua::new();
        lua.globals()
            .set("ext", lua.create_userdata(ExtApi).unwrap())
            .unwrap();
        lua.globals()
            .set("u", format!("http://127.0.0.1:{port}/f.bin"))
            .unwrap();
        lua.globals().set("p", out.display().to_string()).unwrap();
        let got: String = lua
            .load("local n, e = ext:download(u, p) return tostring(n) .. '|' .. tostring(e)")
            .eval()
            .unwrap();
        assert_eq!(got, "4096|nil", "成功应返回字节数（不是 true）");
        assert_eq!(std::fs::read(&out).expect("文件该在"), body, "字节必须一致");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 404 ⇒ `nil, 原因`，且**不留文件**（半成品由平台层清掉）。
    #[test]
    fn download_reports_http_errors_without_leaving_a_file() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf);
                let _ = s.write_all(
                    b"HTTP/1.1 404 Not Found
Content-Length: 0
Connection: close

",
                );
            }
        });
        let dir = ext_proc_dir("dl404", &[]);
        let out = dir.join("missing.bin");
        let lua = Lua::new();
        lua.globals()
            .set("ext", lua.create_userdata(ExtApi).unwrap())
            .unwrap();
        lua.globals()
            .set("u", format!("http://127.0.0.1:{port}/nope"))
            .unwrap();
        lua.globals().set("p", out.display().to_string()).unwrap();
        let got: String = lua
            .load("local n, e = ext:download(u, p) return tostring(n) .. '|' .. tostring(e)")
            .eval()
            .unwrap();
        assert!(got.starts_with("nil|"), "失败必须交 nil：{got}");
        assert!(got.contains("404") || got.contains("下载失败"), "{got}");
        assert!(!out.exists(), "失败不许留文件：{got}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ★ m32：`ext:unzip` 走的是 `platform::unzip`——这里只钉**接线**（正反两面各一格）；
    /// 容器解析 / 路径逃逸 / CRC 那几格在 `platform::unzip` 的测试里。
    #[test]
    fn unzip_wires_through_and_reports_escaping_entries() {
        let dir = ext_proc_dir("uz", &[]);
        let good = dir.join("ok.zip");
        std::fs::write(
            &good,
            crate::platform::unzip::make_zip(&[("a.txt", b"hello", 0)]),
        )
        .unwrap();
        let out_dir = dir.join("out");
        let lua = Lua::new();
        lua.globals()
            .set("ext", lua.create_userdata(ExtApi).unwrap())
            .unwrap();
        lua.globals().set("z", good.display().to_string()).unwrap();
        lua.globals()
            .set("d", out_dir.display().to_string())
            .unwrap();
        let got: String = lua
            .load("local n, e = ext:unzip(z, d) return tostring(n) .. '|' .. tostring(e)")
            .eval()
            .unwrap();
        assert_eq!(got, "1|nil");
        assert_eq!(std::fs::read(out_dir.join("a.txt")).unwrap(), b"hello");

        let evil = dir.join("evil.zip");
        std::fs::write(
            &evil,
            crate::platform::unzip::make_zip(&[("../escape.txt", b"x", 0)]),
        )
        .unwrap();
        lua.globals().set("z", evil.display().to_string()).unwrap();
        let got: String = lua
            .load("local n, e = ext:unzip(z, d) return tostring(n) .. '|' .. tostring(e)")
            .eval()
            .unwrap();
        assert!(got.starts_with("nil|"), "逃逸条目必须拒：{got}");
        assert!(!dir.join("escape.txt").exists(), "逃逸文件不许落盘：{got}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ★ 脚本面的整数边界（m24 审查）：`crop` 的 `x + w` 过去直接相加 —— 传 `2^63-1` 时
    /// debug 构建**直接 panic**（release 会回绕成负数、判据走错分支）。公开 API 不该被一个
    /// 畸形参数打崩，这一格钉的是"给原因而不是崩"。
    #[test]
    fn crop_rejects_overflowing_coordinates_instead_of_panicking() {
        let lua = op_fixture(&[1, 2, 3, 255, 4, 5, 6, 255]); // 2×1
        let (img, err): (mlua::Value, String) = lua
            .load("return img:crop(9223372036854775807, 0, 2, 1)")
            .eval()
            .unwrap();
        assert!(matches!(img, mlua::Value::Nil), "溢出必须交 nil");
        assert!(
            err.contains("太大") || err.contains("出界"),
            "要给一句原因：{err}"
        );
    }

    /// 越界/畸形参数一律 `nil, 原因`，**不钳制、不降级**（症状"它有时候少一块"最难查）。
    #[test]
    fn image_ops_refuse_every_out_of_range_request() {
        let lua = op_fixture(&[1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255]); // 3×1
        for code in [
            "return img:crop(2, 0, 2, 1)", // 右边出界
            "return img:crop(-1, 0, 1, 1)",
            "return img:crop(0, 0, 0, 1)",
            "return img:crop(0, 0, 1, -2)",
            "return img:crop(0, 1, 1, 1)", // y 就在图外
        ] {
            let (img, err): (mlua::Value, String) = lua.load(code).eval().unwrap();
            assert!(matches!(img, mlua::Value::Nil), "{code} 失败必须交 nil");
            assert!(!err.is_empty(), "{code} 失败必须带原因");
        }
        // 3×1 转 90° 之后是 1×3：宽高跟着换，`set_image` 那条会带窗口一起重算
        let (w, h): (i64, i64) = lua
            .load("local o = img:rotate_cw90() return o.w, o.h")
            .eval()
            .unwrap();
        assert_eq!((w, h), (1, 3), "旋转后宽高必须互换");
    }

    /// ★ 动作的结果必须是**合法的 `set_image` 来源**。这格不去测屏幕（那要真窗），它测的是
    /// "新图这个值走得通"：夹具给的是**非 0 但不存在的句柄**，所以类型合法时一定会走到
    /// 最后一层——回写送到宿主却找不到那张窗（`找不到那张贴图` / `回写没送到`）。
    /// ★ 为什么句柄不能留 0：`set_image` **先**查句柄**再**看来源，句柄为 0 时它连类型都不碰
    ///   就返回了——那样的断言把 `image_ok` 换成返回字符串也照样绿（我第一版就是这么写的，
    ///   红证当场把它判死）。给一个非 0 的假句柄，这一格才真的有判据力。
    #[test]
    fn op_results_are_accepted_by_set_image() {
        clear_ext_context();
        let lua = op_fixture(&[1, 2, 3, 4, 5, 6, 7, 8]);
        set_ext_context(ExtCtx {
            image: RgbaBitmap {
                w: 2,
                h: 1,
                rgba: vec![1, 2, 3, 4, 5, 6, 7, 8],
            },
            hwnd: 0x005A_1234,
        });
        let (ok, err): (mlua::Value, String) = lua
            .load("return ext:set_image(img:crop(0, 0, 1, 1))")
            .eval()
            .unwrap();
        assert!(matches!(ok, mlua::Value::Nil), "这张窗不存在，不该报成功");
        assert!(
            err.contains("那张") || err.contains("回写"),
            "该走到「找不到那张窗」那一层，说明来源的类型被接受了；实际：{err}"
        );
        assert!(
            !err.contains("PNG") && !err.contains("读不出"),
            "把动作的结果当成了路径/非法来源：{err}"
        );
    }

    /// 把一张 RGBA 图塞进上下文，并取回 `img` 全局。
    fn op_fixture(rgba: &[u8]) -> Lua {
        clear_ext_context();
        let (w, h) = match rgba.len() {
            16 => (2u32, 2u32),
            12 => (3u32, 1u32),
            8 => (2u32, 1u32),
            n => panic!("夹具只备了 2×2 / 3×1 / 2×1 三种尺寸，给了 {n} 字节"),
        };
        let lua = Lua::new();
        lua.globals()
            .set("ext", lua.create_userdata(ExtApi).unwrap())
            .unwrap();
        set_ext_context(ExtCtx {
            image: RgbaBitmap {
                w,
                h,
                rgba: rgba.to_vec(),
            },
            hwnd: 0,
        });
        let img: mlua::Value = lua.load("return ext:get_image()").eval().unwrap();
        lua.globals().set("img", img).unwrap();
        lua
    }

    fn lua_bytes(lua: &Lua, code: &str) -> Vec<u8> {
        let s: mlua::LuaString = lua.load(code).set_name("t").eval().unwrap();
        s.as_bytes().to_vec()
    }

    fn lua_int(lua: &Lua, code: &str) -> i64 {
        lua.load(code).set_name("t").eval::<i64>().unwrap()
    }

    #[test]
    fn bytes_length_is_w_times_h_times_4() {
        clear_ext_context();
        let lua = Lua::new();
        lua.globals()
            .set("ext", lua.create_userdata(ExtApi).unwrap())
            .unwrap();
        set_ext_context(ExtCtx {
            image: RgbaBitmap {
                w: 3,
                h: 2,
                rgba: (0..24).collect(),
            },
            hwnd: 0,
        });
        let (len, first, last): (i64, i64, i64) = lua
            .load("local img = ext:get_image() local d = img:bytes() return #d, d:byte(1), d:byte(-1)")
            .eval()
            .unwrap();
        assert_eq!(len, 24, "长度必须 = w*h*4");
        assert_eq!((first, last), (0, 23), "内容按 RGBA 行优先原样给出");
    }

    #[test]
    fn png_base64_has_png_signature() {
        clear_ext_context();
        set_ext_context(ExtCtx {
            image: RgbaBitmap {
                w: 2,
                h: 2,
                rgba: vec![255u8; 16],
            },
            hwnd: 0,
        });
        let lua = Lua::new();
        lua.globals()
            .set("ext", lua.create_userdata(ExtApi).unwrap())
            .unwrap();
        let b64: String = lua
            .load("local img = ext:get_image() return img:png_base64()")
            .eval()
            .unwrap();
        clear_ext_context();
        assert!(!b64.is_empty());
        // base64 解回前 4 字节必须是 PNG 签名 0x89 'P' 'N' 'G'。
        let bytes = b64decode(&b64);
        assert_eq!(&bytes[..4], &[0x89, b'P', b'N', b'G']);
    }

    /// ★ 回写贴图（m15）的四档前置拒绝：全都发生在**碰总线之前**，所以单测里不会挂住。
    ///
    /// 判据是"绝不含糊成功"：拿不到目标贴图、句柄为 0、源不是图、总线没起来——四种都得
    /// `nil` + 原因；只有 UI 线程明确回过"换好了"才返回 `true`。
    #[test]
    fn set_image_refuses_every_case_where_it_could_not_verify_success() {
        let bmp = RgbaBitmap {
            w: 2,
            h: 1,
            rgba: vec![3u8; 8],
        };
        let lua = Lua::new();
        lua.globals()
            .set("ext", lua.create_userdata(ExtApi).unwrap())
            .unwrap();
        let probe = r#"local ok, e = ext:set_image(SRC) return tostring(ok) .. "|" .. tostring(e)"#;
        let run = |lua: &Lua, src: &str| -> String {
            lua.load(probe.replace("SRC", src))
                .eval::<String>()
                .unwrap()
        };
        // ① 没有贴图上下文（脚本页里直接跑）
        clear_ext_context();
        let out = run(&lua, "ext:get_image()");
        assert!(out.starts_with("nil|"), "没上下文必须拒，实际：{out}");
        assert!(out.contains("右键"), "要指出该从哪儿触发：{out}");
        // ② 有上下文、句柄是 0（热键触发时没选中窗）⇒ 不猜"最近那一张"
        set_ext_context(ExtCtx {
            image: bmp.clone(),
            hwnd: 0,
        });
        let out = run(&lua, "ext:get_image()");
        assert!(out.starts_with("nil|"), "句柄为 0 必须拒，实际：{out}");
        assert!(out.contains("句柄"), "要说清缺的是句柄：{out}");
        // ③ 源不是图也不是路径
        set_ext_context(ExtCtx {
            image: bmp.clone(),
            hwnd: 0x1234,
        });
        let out = run(&lua, "42");
        assert!(out.starts_with("nil|"), "坏源必须拒，实际：{out}");
        assert!(out.contains("ExtImage"), "要说清接受什么：{out}");
        // ④ 都齐了、但总线没起来（单测环境）⇒ 报"没送到"。总线全进程只有一份，
        //    被别人先装上就跳过并自报，不许静默绿。
        if crate::core::ui_bus::installed() {
            eprintln!("UI 总线已被其它测试占用，跳过④ 那一格读数");
        } else {
            let out = run(&lua, "ext:get_image()");
            assert!(out.starts_with("nil|"), "没总线时不许假装成功，实际：{out}");
            assert!(out.contains("没送到"), "要说清是没送到：{out}");
        }
        clear_ext_context();
    }

    /// ★ 回程第一格（m14）：写出去的字节能**一字不差**读回来，且**绝对路径**走得通。
    /// 绝对路径就是这条 API 的全部意义——`ctx.fs` 刻意拒绝它，于是外部工具写在任何地方
    /// 的产物脚本都取不回来。样本含 `0x00`、0..=255 全部字节值与中文：这条链上有
    /// `as_bytes` / `fs::write` / `create_string` 三段，任何一段偷偷按文本处理都会在这里红。
    #[test]
    fn write_then_read_file_is_byte_exact() {
        let dir = ext_proc_dir("io", &[]);
        let lua = Lua::new();
        lua.globals()
            .set("ext", lua.create_userdata(ExtApi).unwrap())
            .unwrap();
        let p = dir.join("blob.bin");
        lua.globals().set("p", p.display().to_string()).unwrap();
        let out: String = lua
            .load(
                r#"
                local bytes = {}
                for i = 0, 255 do bytes[#bytes + 1] = string.char(i) end
                local data = table.concat(bytes) .. "锟斤拷\x00尾巴"
                local ok, err = ext:write_file(p, data)
                if not ok then return "WRITE-ERR:" .. tostring(err) end
                local back, rerr = ext:read_file(p)
                if back == nil then return "READ-ERR:" .. tostring(rerr) end
                return string.format("%d|%s|%s", #back,
                       tostring(back == data), tostring(back:byte(1)))
                "#,
            )
            .eval()
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let parts: Vec<&str> = out.split('|').collect();
        assert!(
            !out.starts_with("WRITE-ERR") && !out.starts_with("READ-ERR"),
            "往返必须都成功，实际：{out}"
        );
        assert_eq!(parts[1], "true", "读回必须与写出逐字节相等，实际：{out}");
        assert_eq!(parts[2], "0", "0x00 这种字节不许被当成字符串终止");
    }

    /// 两个失败形状都要给人话原因，不许 panic、不许静默：
    /// 读不存在的、写进"父目录是普通文件"的（`create_dir_all` 一定失败，这条是确定性红证）。
    #[test]
    fn io_failures_come_back_as_readable_reasons() {
        let dir = ext_proc_dir("iofail", &[]);
        std::fs::write(dir.join("afile"), b"x").unwrap();
        let lua = Lua::new();
        lua.globals()
            .set("ext", lua.create_userdata(ExtApi).unwrap())
            .unwrap();
        lua.globals().set("dir", dir.display().to_string()).unwrap();
        let out: String = lua
            .load(
                r#"
                local a = ext:read_file(dir .. "/nope.bin")
                local b = ext:write_file(dir .. "/afile/sub.txt", "hi")
                local c = ext:read_file(dir)
                return table.concat({ tostring(a), tostring(b), tostring(c),
                                      "|", ext:read_file("  ") and "v" or "nil-empty" }, ",")
                "#,
            )
            .eval()
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        // ① 文件不存在 ② 父目录是个普通文件（建不了目录，也就写不进去）③ 把目录当文件读
        assert!(
            out.starts_with("nil,nil,nil,|"),
            "三次都该返回 nil + 原因，实际：{out}"
        );
        assert!(out.ends_with(",nil-empty"), "空路径要挡住，实际：{out}");
    }

    /// ★★ 回程第三格的**对拍钉子**：用生产的编码器存一张 PNG，再用 `ext:load_image` 读回来，
    /// 逐字节比。它测的不是"代码看着能跑"，而是**我们的 writer 与 reader 真的互为逆运算**
    /// （RGBA/BGRA 顺序、stride、调色板展开任何一处不对，这里就红——那正是
    /// 剪贴板缩略图当年栽过的"拿到索引值当颜色"的形状）。
    #[test]
    fn load_image_roundtrips_a_png_written_by_the_real_encoder() {
        let dir = ext_proc_dir("img", &[]);
        let bmp = RgbaBitmap {
            w: 3,
            h: 2,
            // 含半透明与全透明像素：alpha 通道没走对就会整片变黑或变白。
            rgba: vec![
                10, 20, 30, 255, 200, 100, 50, 128, 0, 0, 0, 0, 255, 255, 255, 255, 1, 2, 3, 250,
                99, 88, 77, 1,
            ],
        };
        clear_ext_context();
        set_ext_context(ExtCtx {
            image: bmp.clone(),
            hwnd: 0,
        });
        let p = dir.join("out.png");
        let lua = Lua::new();
        lua.globals()
            .set("ext", lua.create_userdata(ExtApi).unwrap())
            .unwrap();
        lua.globals().set("p", p.display().to_string()).unwrap();
        // ★ 逐字节比对**放在 Lua 里做**：原始像素穿过 `eval::<String>()` 会撞上非法 UTF-8，
        //   那时候红的是转换、不是产品，读数就废了。
        let out: String = lua
            .load(
                r#"
                local src = ext:get_image()
                src:save(p)
                local img, err = ext:load_image(p)
                if not img then return "ERR:" .. tostring(err) end
                local a, b = src:bytes(), img:bytes()
                return string.format("%dx%d|%d|%s", img.w, img.h, #b, tostring(a == b))
                "#,
            )
            .eval()
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!out.starts_with("ERR:"), "读回自己的 PNG 不该失败：{out}");
        assert_eq!(
            out, "3x2|24|true",
            "尺寸/字节数/逐字节都要对上（BGRA 与 RGBA 走反、alpha 没搬运，都会在这里红）"
        );
    }

    /// ★ m21：`ext:load_image` 交出来的必须是**原尺寸**。1200×800 走的是 WIC 原尺寸那条路；
    /// 若退回缩略图口径（`min(720,w)`）会读到 720×480——这条与 `image_decode` 里那两格是
    /// 同一条判据的两个入口（Lua 面 / Rust 面），**小样本永远绿**，所以两边都喂了大图。
    #[test]
    fn load_image_keeps_the_original_size_above_720() {
        let dir = ext_proc_dir("imgbig", &[]);
        let bmp = RgbaBitmap {
            w: 1200,
            h: 800,
            rgba: vec![0x40; 1200 * 800 * 4],
        };
        clear_ext_context();
        set_ext_context(ExtCtx {
            image: bmp,
            hwnd: 0,
        });
        let p = dir.join("big.png");
        let lua = Lua::new();
        lua.globals()
            .set("ext", lua.create_userdata(ExtApi).unwrap())
            .unwrap();
        lua.globals().set("p", p.display().to_string()).unwrap();
        let out: String = lua
            .load(
                r#"
                ext:get_image():save(p)
                local img, err = ext:load_image(p)
                if not img then return "ERR:" .. tostring(err) end
                return img.w .. "x" .. img.h
                "#,
            )
            .set_name("t")
            .eval()
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(out, "1200x800", "必须原尺寸，不许走缩略图口径（720×480）");
    }

    /// 非图文件要**给一句能追下去的原因**（点名文件 + 说明是哪一类失败），
    /// 不许含糊成"解不出图"让用户去猜格式；也不许再声称"只支持 PNG"（m21 起走 WIC）。
    #[test]
    fn load_image_says_which_formats_it_reads() {
        let dir = ext_proc_dir("imgbad", &[]);
        std::fs::write(dir.join("note.txt"), b"hello, not an image").unwrap();
        let lua = Lua::new();
        lua.globals()
            .set("ext", lua.create_userdata(ExtApi).unwrap())
            .unwrap();
        lua.globals()
            .set("p", dir.join("note.txt").display().to_string())
            .unwrap();
        let out: String = lua
            .load("local i, e = ext:load_image(p) return tostring(i) .. '|' .. tostring(e)")
            .eval()
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(out.starts_with("nil|"), "该返回 nil + 原因，实际：{out}");
        // ★ m21 起不再有"只支持 PNG"这回事：读图走 WIC（原尺寸），支持面以本机解码器为准。
        //   判据改成两条：**点名文件**，并如实说"WIC 的三种失败分不开"——不许再声称只吃 PNG，
        //   也不许假装能分清"没解码器"和"文件坏了"。
        assert!(out.contains("解不了"), "要给一句人话，实际：{out}");
        assert!(out.contains("note.txt"), "要点名是哪个文件，实际：{out}");
        assert!(
            out.contains("解码器") || out.contains("格式"),
            "要说明是哪一类失败，实际：{out}"
        );
        assert!(
            !out.contains("只支持 PNG"),
            "旧口径（只支持 PNG）已经作废，不许留在原因里：{out}"
        );
    }

    /// ★ 超限**拒**而不是截断，而且不许真占磁盘：用 `set_len` 造一个"稀疏的 65 MB 文件"，
    /// 元数据看着超限、实际几乎不落盘。截断的字节被当图片用＝没人能复现的坏文件。
    #[test]
    fn oversized_file_is_refused_not_truncated() {
        let dir = ext_proc_dir("big", &[]);
        let p = dir.join("sparse.bin");
        let f = std::fs::File::create(&p).unwrap();
        f.set_len(MAX_READ + 1).unwrap();
        drop(f);
        let lua = Lua::new();
        lua.globals()
            .set("ext", lua.create_userdata(ExtApi).unwrap())
            .unwrap();
        lua.globals().set("p", p.display().to_string()).unwrap();
        let out: String = lua
            .load("local d, e = ext:read_file(p) return tostring(d) .. '|' .. tostring(e)")
            .eval()
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(out.starts_with("nil|"), "超限必须拒，实际：{out}");
        assert!(out.contains("上限"), "要说清是撞上上限，实际：{out}");
    }

    /// `tmpdir` 之前那条测试的位置：文件对话框三格见下面 m13 那组。
    #[test]
    fn open_file_rejects_unknown_kind_before_opening_anything() {
        let lua = Lua::new();
        lua.globals()
            .set("ext", lua.create_userdata(ExtApi).unwrap())
            .unwrap();
        // `"iamge"` 拼错一个字母：必须当场拒并列出可用档，**不许**降级成"所有文件"——
        // 降级的症状是"它有时候不给我显示图片"，那种没人能复现的怪事最贵。
        // 这条同时钉住顺序：先查词表再开对话框（反了就会在无人点击的情况下弹出一扇模态窗）。
        let out: String = lua
            .load(
                r#"
                local a, b = ext:open_file("iamge")
                return tostring(a) .. "|" .. tostring(b)
            "#,
            )
            .eval()
            .unwrap();
        assert!(out.starts_with("nil|"), "应当返回 nil + 原因，实际：{out}");
        assert!(
            out.contains("any / image / audio / gif / mp4"),
            "原因要把可用档列出来，实际：{out}"
        );
    }

    /// 逐帧脚本（`draw` 跑在 UI 线程）里要文件对话框 ⇒ 当场拒绝。
    ///
    /// ★ 判据作参数进来才能测：从 Lua 侧走的话，这一档取决于"本线程是不是 UI 线程"，
    ///   单测里永远不是。而**真去开一扇模态窗**会把测试进程卡住（不是变红，是挂住），
    ///   所以实体函数收 `on_ui_thread` 显式值，方法只负责把总线读数递进来。
    #[test]
    fn modal_dialog_calls_are_refused_on_the_ui_thread() {
        assert!(modal_block(false).is_none(), "工作线程该放行");
        let msg = modal_block(true).expect("UI 线程必须挡下模态对话框");
        assert!(msg.contains("draw"), "文案要指出是逐帧脚本：{msg}");

        let lua = Lua::new();
        for (what, r) in [
            (
                "open_file",
                open_file_impl(&lua, Some("image".to_string()), None, true),
            ),
            ("save_file", save_file_impl(&lua, None, None, None, true)),
        ] {
            let vals = r.expect(what).into_vec();
            assert_eq!(vals.len(), 2, "{what}：拒绝要给 nil + 原因");
            assert!(
                matches!(vals[0], mlua::Value::Nil),
                "{what}：第一位要是 nil"
            );
            let s = vals[1]
                .as_string()
                .map(|x| x.to_string_lossy())
                .unwrap_or_default();
            assert!(s.contains("draw"), "{what} 的原因要说清是哪类脚本：{s}");
        }
    }

    /// ★ **默认不跑**（`#[ignore]`）：这条要写用户的剪贴板，而剪贴板是本机共享资源
    ///   （还有别的程序与代理在用）。它只在人工核验那一轮跑一次，而且**先备份再动手**：
    ///   剪贴板里没有可备份的文本时直接跳过——把一个图片/文件列表换掉不是测试该付的代价。
    #[test]
    #[ignore = "会动用户剪贴板：只在人工核验那一轮跑"]
    fn clip_roundtrip_restores_user_content() {
        let before = crate::platform::clipboard::get_text();
        let Some(before) = before else {
            eprintln!("剪贴板当前没有文本，没有可还原的备份 ⇒ 跳过（不动用户内容）");
            return;
        };
        let probe = format!("curseen_clip_probe_{}", std::process::id());
        let lua = Lua::new();
        lua.globals()
            .set("ext", lua.create_userdata(ExtApi).unwrap())
            .unwrap();
        lua.globals().set("probe", probe.as_str()).unwrap();
        // `ext:clip_set` 内部写完会读回来核，所以 `true` 不是"我们假装成功"。
        let out: String = lua
            .load(
                r#"
                local ok = ext:clip_set(probe)
                local back = ext:clip_get()
                return tostring(ok) .. "|" .. tostring(back)
            "#,
            )
            .eval()
            .unwrap();
        // 无论上面成不成，先把用户的内容放回去，再看结果。
        crate::platform::clipboard_history::copy_text(&before);
        let restored = crate::platform::clipboard::get_text();
        assert_eq!(
            restored.as_deref(),
            Some(before.as_str()),
            "用户的剪贴板必须原样还原"
        );
        assert_eq!(out, format!("true|{probe}"), "写入读回应一致，实际：{out}");
        // 两条分支都要**自报**：静默 `ok` 分不清"跑完了"与"跳过"，那正是假绿的样子。
        eprintln!(
            "剪贴板实验：原文本 {} 字 → 探针 {} 字 → 已还原 {} 字",
            before.chars().count(),
            probe.chars().count(),
            restored.unwrap_or_default().chars().count()
        );
    }

    /// 最小 base64 解码（仅测试用，够验 PNG 签名即可）。
    fn b64decode(s: &str) -> Vec<u8> {
        let mut out = Vec::new();
        let mut buf = 0u32;
        let mut bits = 0u8;
        for c in s.chars() {
            let v = match c {
                'A'..='Z' => c as u32 - 'A' as u32,
                'a'..='z' => c as u32 - 'a' as u32 + 26,
                '0'..='9' => c as u32 - '0' as u32 + 52,
                '+' => 62,
                '/' => 63,
                _ => continue,
            };
            buf = (buf << 6) | v;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((buf >> bits) as u8);
            }
        }
        out
    }

    /// 建一个"脚本目录"，把系统里的小 exe 复制进去当外部工具。
    /// 复制失败（这台机器没有它）返回空目录，调用方自己跳过并打印原因。
    fn ext_proc_dir(tag: &str, tools: &[&str]) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "curseen_extproc_{tag}_{}_{:x}",
            std::process::id(),
            tools.len()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        if let Ok(w) = std::env::var("WINDIR") {
            for t in tools {
                let _ = std::fs::copy(std::path::Path::new(&w).join("System32").join(t), d.join(t));
            }
        }
        d
    }

    /// 在**当前线程**上装好 `ext` 与脚本目录，跑一段 Lua 取回字符串。
    fn lua_with_dir(dir: &std::path::Path, code: &str) -> String {
        set_script_dir(dir);
        let lua = Lua::new();
        lua.globals()
            .set("ext", lua.create_userdata(ExtApi).unwrap())
            .unwrap();
        lua.load(code).set_name("t").eval::<String>().unwrap()
    }

    /// ★ 路径收口本体（m10）：脚本目录**之外**的一律不起，`.exe` 之外的一律不起。
    /// 这条测的是"能不能被手滑起走"，不是"能不能被绕过"——`ext:write_file` 就往那个目录里写得
    /// 进 exe，所以它**从来不是安全边界**（口径见 [`resolve_tool`] 的注释）。
    /// 这条不测"能跑"，只测"拒绝"：能被 `C:\Windows\System32\cmd.exe` 走通的那一挡
    /// 就等于这行代码白写。
    #[test]
    fn proc_gate_rejects_everything_outside_the_script_dir() {
        let dir = ext_proc_dir("gate", &[]);
        let win = std::env::var("WINDIR").unwrap_or_default();
        for bad in [
            &format!(r#"{win}\System32\cmd.exe"#),
            r#"C:\Windows\System32\cmd.exe"#,
            r#"\\nas\share\tool.exe"#,
            r#"..\..\evil.exe"#,
            r#"./a/../../b.exe"#,
            r#""#,
            "./tool.dll",
            "./tool.bat",
            "./tool.txt",
            "./absent.exe", // 后缀对但文件不在：也要给"找不到"的中文原因
        ] {
            let e = resolve_tool(&dir, bad)
                .err()
                .unwrap_or_else(|| panic!("路径收口漏放了 {bad:?}"));
            assert!(!e.is_empty(), "拒绝必须带原因：{bad:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 真跑一次通路：脚本目录里的 exe → 退出码、stdout、耗时都如实回到 Lua 表里。
    #[test]
    fn proc_runs_exe_from_script_dir_and_returns_table() {
        let dir = ext_proc_dir("run", &["where.exe"]);
        if !dir.join("where.exe").is_file() {
            eprintln!("拷不进 where.exe，跳过");
            return;
        }
        // `demo.exe` 是脚本目录里的相对路径；`where cmd` 把 cmd.exe 的路径打在 stdout。
        let out = lua_with_dir(
            &dir,
            r#"
            local r, err = ext:proc{ exe = "./where.exe", args = { "cmd" } }
            if not r then return "ERR:" .. err end
            return string.format("%d|%s|%d|%d",
              r.code, r.stdout:find("cmd.exe") and "hit" or "miss",
              r.timed_out and 1 or 0, r.elapsed_ms >= 0 and 1 or 0)
            "#,
        );
        assert_eq!(out, "0|hit|0|1", "退出码/输出/超时位/耗时都得是活的读数");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 超时从脚本侧必须看得见：`timed_out=true`、`code=nil`（被宿主杀的，不该报码）。
    #[test]
    fn proc_timeout_is_reported_to_lua() {
        let dir = ext_proc_dir("timeout", &["ping.exe"]);
        if !dir.join("ping.exe").is_file() {
            eprintln!("拷不进 ping.exe，跳过");
            return;
        }
        let out = lua_with_dir(
            &dir,
            r#"
            local r = ext:proc{ exe = "./ping.exe",
                               args = { "-n", "30", "-w", "1000", "127.0.0.1" },
                               timeout_ms = 800 }
            if not r then return "nil" end
            return (r.timed_out and "to" or "nto") .. "|" ..
                   (r.code == nil and "noc" or "c") .. "|" .. r.stderr
            "#,
        );
        assert!(
            out.starts_with("to|noc|"),
            "超时要报成「被杀、无退出码」，实际：{out}"
        );
        assert!(out.contains("超时"), "stderr 要写清是超时，实际：{out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 越界路径在 **Lua 侧**也要拿到中文原因（而不是被 mlua 的类型报错盖掉）。
    #[test]
    fn proc_guard_rejection_reaches_the_script() {
        let dir = ext_proc_dir("reject", &[]);
        let out = lua_with_dir(
            &dir,
            r#"
            local r, err = ext:proc{ exe = "C:\\Windows\\System32\\cmd.exe" }
            return tostring(r) .. "|" .. tostring(err)
            "#,
        );
        assert!(out.starts_with("nil|"), "应当返回 nil + 原因，实际：{out}");
        assert!(
            out.contains("外部程序只能放在"),
            "原因要说人话，实际：{out}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `ext:tmpdir()` 必须是**绝对路径且已创建**——外部程序多数只认绝对路径（实测），
    /// 而脚本要能把位图落成文件递出去。
    /// 安全门先于系统调用：非图片扩展、非 http(s) 网址都必须在 ShellExecute 之前被拒。
    #[test]
    fn open_temp_extension_allowlist_is_raster_only() {
        for ok in [
            "png", "JPG", "jpeg", "gif", "bmp", "webp", "tif", "tiff", "ico",
        ] {
            assert!(open_temp_ext_allowed(&ok.to_ascii_lowercase()), "{ok}");
        }
        for bad in ["exe", "bat", "cmd", "ps1", "lnk", "url", "svg", "html", ""] {
            assert!(!open_temp_ext_allowed(bad), "{bad}");
        }
    }

    #[test]
    fn data_dir_is_per_script_and_outside_assets() {
        set_script_id("scripts/demo_data");
        let lua = Lua::new();
        lua.globals()
            .set("ext", lua.create_userdata(ExtApi).unwrap())
            .unwrap();
        let p: String = lua.load("return ext:data_dir()").eval().unwrap();
        assert!(p.ends_with("demo_data"), "{p}");
        assert!(!p.contains("assets"), "私有数据不能落进 assets：{p}");
        assert!(std::path::Path::new(&p).is_dir(), "取用时必须已建好");
        let _ = std::fs::remove_dir_all(&p);
    }

    #[test]
    fn open_helpers_reject_unsafe_inputs() {
        let lua = Lua::new();
        lua.globals()
            .set("ext", lua.create_userdata(ExtApi).unwrap())
            .unwrap();
        let bad_ext: String = lua
            .load(r#"local p,e = ext:open_temp("MZ", "exe") return tostring(p) .. "|" .. tostring(e)"#)
            .eval()
            .unwrap();
        assert!(
            bad_ext.contains("只允许光栅") && bad_ext.starts_with("nil|"),
            "{bad_ext}"
        );
        let bad_url: String = lua
            .load(r#"local ok,e = ext:open_url("file:///C:/Windows/System32/calc.exe") return tostring(ok) .. "|" .. tostring(e)"#)
            .eval()
            .unwrap();
        assert!(
            bad_url.contains("只允许 http/https") && bad_url.starts_with("nil|"),
            "{bad_url}"
        );
    }

    #[test]
    fn tmpdir_is_absolute_and_exists() {
        let dir = ext_proc_dir("tmp", &[]);
        set_script_dir(&dir);
        let lua = Lua::new();
        lua.globals()
            .set("ext", lua.create_userdata(ExtApi).unwrap())
            .unwrap();
        let p: String = lua
            .load(r#"local d = ext:tmpdir() return d .. "|" .. (ext:script_dir():len() > 0 and "has" or "none")"#)
            .eval()
            .unwrap();
        let (tmp, tail) = p.split_once('|').unwrap();
        let path = std::path::Path::new(tmp);
        assert!(path.is_absolute(), "tmpdir 必须是绝对路径，实际：{tmp}");
        assert!(path.is_dir(), "tmpdir 必须已经建好，实际：{tmp}");
        assert_eq!(tail, "has", "script_dir 要能把刚加载的脚本目录报出来");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
