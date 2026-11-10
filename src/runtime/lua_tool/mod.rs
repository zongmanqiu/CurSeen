//! 发行版自带的 **Lua 工具台**：给外部 AI agent / CI 一个"不开窗、不提权、不抢单实例"的入口。
//!
//! ```text
//! curseen.exe --lua-check assets/lua/HoverDot/foo.lua      # 能不能加载、schema 有没有隐患
//! curseen.exe --lua-trace assets/lua/HoverDot/foo.lua      # 跑 300 帧，返回 pass/fail + 逐帧账本
//! curseen.exe --lua-shot  assets/lua/HoverDot/foo.lua      # 离屏出图（深/浅底四块，看对比度）
//! curseen.exe --mcp                                        # 同一套能力包成 MCP（stdio）
//! ```
//!
//! # 为什么要内建在 exe 里
//!
//! 用户手上只有发行版，没有 cargo、没有测试套件。要让 AI"写完脚本测到通过"，判断依据只能
//! 由 exe 自己产出——这就是本模块存在的全部理由。**判据做进工具**（不是让调用方去解释数字），
//! 所以每个命令都有 `pass/fail` 与退出码。
//!
//! # 三条硬约束（踩过才知道）
//!
//! 1. **必须在 `main()` 最前面分流**：晚于 `claim_instance` 就会被单实例仲裁 Handoff 掉，
//!    表象是"命令行敲完什么都没发生"；晚于提权分支就会为一台无头工具拉起 UAC。
//! 2. **GUI 子系统的 stdout 不是终端**：`windows_subsystem = "windows"` 的进程从 cmd 直接
//!    启动时没有控制台，必须 `AttachConsole(ATTACH_PARENT_PROCESS)` + 打开 `CONOUT$`，
//!    否则 AI 拿到零字节输出（"命令跑了但没结果"）。
//! 3. **只认相对程序目录的路径**（便携铁律）：输入 `assets/lua/...`，输出 `temp/...`。
//!    绝对路径只接受落在程序目录内的，`..` 一律拒绝——工具不该成为越界读写的口子。
//!
//! # 退出码
//!
//! `0` 干净通过 / `2` 用法错误 / `3` 有警告或判据不过 / `4` 加载或运行出错。

use std::collections::HashMap;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use serde_json::{json, Value as Json};

mod mcp;
mod trace;
mod write;

use trace::{Host, Profile};

/// 工具台的运行上下文：程序目录 + lua 根。
///
/// 不直接调 `core::paths::app_dir()`：路径解析是这条链上最容易被测坏的一环（相对/绝对/
/// `..`/程序目录外），把它变成参数才测得了。
pub struct Ctx {
    pub(crate) app: PathBuf,
    pub(crate) lua: PathBuf,
}

impl Ctx {
    /// 生产用：程序目录 = exe 所在目录。
    pub fn from_app_dir(app: PathBuf) -> Self {
        let lua = app.join("assets").join("lua");
        Self { app, lua }
    }

    /// 脚本路径归一：`HoverDot/foo` ≡ `assets/lua/HoverDot/foo.lua`。
    ///
    /// 为什么要宽容：`write_script` / `set_active` 收简写、返回的 `id` 也是简写，调用方
    /// 自然会把它再喂回 check / trace。两个命令各认一种写法，只会制造"文件明明刚写完，
    /// 工具却说读不到"这种最费时间的错觉。
    pub(crate) fn resolve_script(&self, input: &str) -> Result<PathBuf, String> {
        let mut s = input.trim().replace('\\', "/");
        let first = s.split('/').next().unwrap_or("");
        const MOUNTS: [&str; 5] = ["MouseTrail", "HoverDot", "ClickFx", "KeySound", "scripts"];
        if MOUNTS.contains(&first) {
            s = format!("assets/lua/{s}");
        }
        // 补 `.lua`：`HoverDot/solid` 和 `HoverDot/solid.lua` 是同一个脚本，
        // 少了后缀会让"文件存在吗"直接判假（而路径本身看起来完全正确）。
        let last = s.rsplit('/').next().unwrap_or("");
        if !last.is_empty() && !last.contains('.') {
            s.push_str(".lua");
        }
        self.resolve(&s)
    }

    /// 相对路径 → 绝对（相对程序目录）。`..` 与程序目录外的绝对路径一律拒绝。
    pub(crate) fn resolve(&self, input: &str) -> Result<PathBuf, String> {
        let raw = input.trim();
        if raw.is_empty() {
            return Err("缺少路径参数".into());
        }
        let p = Path::new(raw);
        let joined = if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.app.join(p)
        };
        if joined
            .components()
            .any(|c| matches!(c, Component::ParentDir))
        {
            return Err(format!("路径里不许出现 `..`：{raw}"));
        }
        if !joined.starts_with(&self.app) {
            return Err(format!(
                "路径必须落在程序目录内（{}）：{raw}",
                self.app.display()
            ));
        }
        Ok(joined)
    }

    /// 展示用：相对程序目录的写法（正斜杠、可读）。
    fn display(&self, p: &Path) -> String {
        p.strip_prefix(&self.app)
            .unwrap_or(p)
            .to_string_lossy()
            .replace('\\', "/")
    }

    /// 脚本路径 → (引擎根, 脚本 id, 宿主)。
    ///
    /// 落在 `assets/lua/` 里的按"相对 lua 根"的 id 加载（`HoverDot/foo`）——这与生产宿主
    /// 用的是同一个 id，所以 `ctx.fs` 绑定、相对资源路径的行为与真机一致。落在程序目录
    /// 其它地方（临时文件）则以所在目录为根、文件名为 id，方便"先试后搬"。
    fn locate(&self, path: &Path, mode: Option<Host>) -> (PathBuf, String, Host) {
        if let Ok(rel) = path.strip_prefix(&self.lua) {
            let id = rel.with_extension("").to_string_lossy().replace('\\', "/");
            let host = mode
                .or_else(|| id.split('/').next().and_then(Host::from_top_dir))
                .unwrap_or(Host::Composite);
            return (self.lua.clone(), id, host);
        }
        let root = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let id = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let host = mode
            .or_else(|| {
                path.parent()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
                    .and_then(Host::from_top_dir)
            })
            .unwrap_or(Host::Composite);
        (root, id, host)
    }
}

/// 命令行总入口。`Some(码)` = 已按工具模式处理完毕（**不要再启动 UI**）。
pub fn run_cli(args: Vec<String>) -> Option<i32> {
    let first = args.first()?.as_str();
    if !matches!(
        first,
        "--lua-check" | "--lua-trace" | "--lua-shot" | "--mcp" | "--lua-help" | "--help" | "-h"
    ) {
        return None;
    }
    let ctx = Ctx::from_app_dir(crate::core::paths::app_dir());
    let mut out = Out::open();
    Some(dispatch(&ctx, &args, &mut out))
}

/// 解析结果 → 执行。
fn dispatch(ctx: &Ctx, args: &[String], out: &mut Out) -> i32 {
    match args[0].as_str() {
        "--lua-help" | "--help" | "-h" => {
            let _ = out.write_all(help().as_bytes());
            0
        }
        "--mcp" => mcp::serve(ctx, out),
        "--lua-check" => {
            let Some(path) = args.get(1).filter(|a| !a.starts_with("--")) else {
                return usage(out, "--lua-check 需要一个脚本路径");
            };
            let flags = match Flags::parse(&args[2..]) {
                Ok(f) => f,
                Err(e) => return usage(out, &e),
            };
            let (code, v) = cmd_check(ctx, path, &flags);
            emit(out, &v);
            code
        }
        "--lua-trace" => {
            let Some(path) = args.get(1).filter(|a| !a.starts_with("--")) else {
                return usage(out, "--lua-trace 需要一个脚本路径");
            };
            let flags = match Flags::parse(&args[2..]) {
                Ok(f) => f,
                Err(e) => return usage(out, &e),
            };
            let (code, v) = cmd_trace(ctx, path, &flags);
            emit(out, &v);
            code
        }
        "--lua-shot" => {
            let Some(path) = args.get(1).filter(|a| !a.starts_with("--")) else {
                return usage(out, "--lua-shot 需要一个脚本路径");
            };
            let flags = match Flags::parse(&args[2..]) {
                Ok(f) => f,
                Err(e) => return usage(out, &e),
            };
            let (code, v) = cmd_shot(ctx, path, &flags);
            emit(out, &v);
            code
        }
        _ => usage(out, "未知模式"),
    }
}

/// 子命令的旗标。
#[derive(Default)]
pub(crate) struct Flags {
    pub(crate) frames: Option<u32>,
    pub(crate) at: Option<u32>,
    pub(crate) screen: Option<(f32, f32)>,
    pub(crate) mode: Option<Host>,
    pub(crate) out: Option<String>,
    pub(crate) params: HashMap<String, Json>,
    pub(crate) profile: Option<Profile>,
}

impl Flags {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut f = Flags::default();
        // 取一个旗标的值：`i` 由调用方先自增，这里只读，避免闭包可变借用与循环条件打架。
        fn take(args: &[String], j: usize, what: &str) -> Result<String, String> {
            args.get(j)
                .cloned()
                .ok_or_else(|| format!("{what} 需要一个值"))
        }
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--frames" => {
                    let v = take(args, i + 1, "--frames")?;
                    f.frames = Some(v.parse().map_err(|_| format!("--frames 不是数字：{v}"))?);
                    i += 2;
                }
                "--at" => {
                    let v = take(args, i + 1, "--at")?;
                    f.at = Some(v.parse().map_err(|_| format!("--at 不是数字：{v}"))?);
                    i += 2;
                }
                "--out" => {
                    f.out = Some(take(args, i + 1, "--out")?);
                    i += 2;
                }
                "--screen" => {
                    let v = take(args, i + 1, "--screen")?;
                    let (w, h) = v
                        .split_once(['x', 'X'])
                        .ok_or_else(|| format!("--screen 形如 1920x1080：{v}"))?;
                    f.screen = Some((
                        w.parse().map_err(|_| "屏幕宽度不是数字".to_string())?,
                        h.parse().map_err(|_| "屏幕高度不是数字".to_string())?,
                    ));
                    i += 2;
                }
                "--mode" => {
                    let v = take(args, i + 1, "--mode")?;
                    f.mode = Some(match v.as_str() {
                        "hover" => Host::Hover,
                        "trail" => Host::Trail,
                        "click" | "clickfx" => Host::Click,
                        "composite" | "panel" | "scripts" => Host::Composite,
                        other => {
                            return Err(format!("--mode 只认 hover/trail/click/composite：{other}"))
                        }
                    });
                    i += 2;
                }
                "--input" => {
                    let v = take(args, i + 1, "--input")?;
                    f.profile = Some(
                        Profile::parse(&v)
                            .ok_or_else(|| format!("--input 只认 sweep / idle-swing：{v}"))?,
                    );
                    i += 2;
                }
                "--param" => {
                    let v = take(args, i + 1, "--param")?;
                    let (k, raw) = v
                        .split_once('=')
                        .ok_or_else(|| format!("--param 形如 key=value：{v}"))?;
                    let val = serde_json::from_str::<Json>(raw)
                        .unwrap_or_else(|_| Json::String(raw.to_string()));
                    f.params.insert(k.trim().to_string(), val);
                    i += 2;
                }
                other => return Err(format!("不认识的参数：{other}")),
            }
        }
        Ok(f)
    }

    fn opt(&self, host: Host) -> trace::Opt {
        trace::Opt {
            frames: self.frames.unwrap_or(300).clamp(2, 20_000),
            screen: self.screen.unwrap_or((1920.0, 1080.0)),
            params: self.params.clone(),
            host,
            profile: self.profile.unwrap_or(Profile::Sweep),
        }
    }
}

/// 脚本不存在时的统一回答。
///
/// 不交给 `load_script` 的 "无法读取脚本" 自然冒泡：那句话会被当成"脚本有语法错"，
/// 而真实原因是路径——两者的修法完全相反（改路径 vs 改代码）。
fn missing_script(ctx: &Ctx, resolved: &Path) -> Json {
    json!({
        "ok": false,
        "file": ctx.display(resolved),
        "error": "脚本不存在（路径没对上）。",
        "hint": "路径相对程序目录：assets/lua/HoverDot/foo.lua，也可以简写成 HoverDot/foo。                 要新建脚本用 write_script。",
    })
}

/// `--lua-check`：加载 + schema 回读 + 静态体检。
pub(crate) fn cmd_check(ctx: &Ctx, path: &str, flags: &Flags) -> (i32, Json) {
    let resolved = match ctx.resolve_script(path) {
        Ok(p) => p,
        Err(e) => return (2, json!({ "ok": false, "error": e })),
    };
    if !resolved.is_file() {
        return (2, missing_script(ctx, &resolved));
    }
    let (root, id, host) = ctx.locate(&resolved, flags.mode);
    let shown = ctx.display(&resolved);

    let eng = match crate::scripting::engine::ScriptingEngine::new(&root) {
        Ok(e) => e,
        Err(e) => {
            return (
                4,
                json!({ "ok": false, "file": shown, "error": format!("初始化引擎失败：{e}") }),
            )
        }
    };
    if let Err(e) = eng.load_script(&id) {
        return (
            4,
            json!({
                "ok": false,
                "file": shown,
                "id": id,
                "error": format!("{e}"),
                "hint": "语法错误 / 顶层代码抛异常。报错里的行号是对该文件本身的行号。",
            }),
        );
    }

    let src = std::fs::read_to_string(&resolved).unwrap_or_default();
    let s = |k: &str| -> String {
        eng.lua()
            .globals()
            .get::<Option<String>>(k)
            .ok()
            .flatten()
            .unwrap_or_default()
    };
    // ★ `schema()` 是**可选**的：没有它就等于"这个脚本没有参数面板"，宿主照跑
    //   （`lua_effect` 里 `call_schema` 失败也只是退化成空默认值）。所以要先分清
    //   "没写 schema"（合法）与 "schema 抛异常"（真坏），否则一个最简脚本会被报成
    //   `ok:false`，AI 立刻去查一个不存在的毛病。
    let has_schema = eng
        .lua()
        .globals()
        .get::<Option<mlua::Value>>("schema")
        .ok()
        .flatten()
        .is_some_and(|v| matches!(v, mlua::Value::Function(_)));
    let mut no_schema = false;
    let schema = if has_schema {
        match eng.call_schema() {
            Ok(t) => crate::scripting::registry::parse_schema_table(&t),
            Err(e) => {
                return (
                    3,
                    json!({
                        "ok": false,
                        "file": shown,
                        "id": id,
                        "error": format!("schema() 调用失败：{e}"),
                        "hint": "面板的参数区完全由 schema() 生成，它抛异常等于这个脚本没有参数面板。\
                                 注意：不想要参数就别写 schema（合法），写了就必须返回表。",
                    }),
                )
            }
        }
    } else {
        no_schema = true;
        Vec::new()
    };

    let mut warnings: Vec<String> = Vec::new();
    // 动态取键（`params[field]`）时静态扫描扫不到，直接放过未读判定——
    // 宁可漏报也不能误报（"说我没读"比"没提醒"更让人怀疑判据本身）。
    let dynamic = src.contains("params[");
    let host_read = ["density", "spawn_interval_ms"];
    for def in &schema {
        let k = &def.key;
        let read = src.contains(&format!("params.{k}")) || src.contains(&format!("p.{k}"));
        if !read && !dynamic && !host_read.contains(&k.as_str()) {
            warnings.push(format!(
                "参数 `{k}` 声明了但脚本从未读取（面板上拖它不会有任何变化）"
            ));
        }
        if matches!(
            def.ptype,
            crate::scripting::ParamType::Int | crate::scripting::ParamType::Float
        ) {
            if let Some(d) = def.default.as_f64() {
                if d < f64::from(def.min) - 1e-6 || d > f64::from(def.max) + 1e-6 {
                    warnings.push(format!(
                        "参数 `{k}` 的默认值 {d} 不在 [{}, {}] 内",
                        def.min, def.max
                    ));
                } else if def.step > 0.0 {
                    let n = ((d - f64::from(def.min)) / f64::from(def.step)).round();
                    let snapped = f64::from(def.min) + n * f64::from(def.step);
                    if (d - snapped).abs() > f64::from(def.step) * 1e-3 + 1e-9 {
                        // 网格值取到 3 位小数再打印：`0.30000000447034836` 那种浮点尾数
                        // 只会让读的人怀疑判据算错了。
                        let nearest = (snapped * 1000.0).round() / 1000.0;
                        warnings.push(format!(
                            "参数 `{k}` 的默认值 {d} 不在 min+step 网格上（最近 {nearest}）：\
                             面板会把浮点滑条吸附到网格，显示值与脚本实际值会差半格"
                        ));
                    }
                }
            }
        }
        if matches!(def.ptype, crate::scripting::ParamType::Choice) && !def.choices.is_empty() {
            let d = def.default.as_str().unwrap_or_default();
            if !def.choices.iter().any(|c| c == d) {
                warnings.push(format!("参数 `{k}` 的默认值 `{d}` 不在 choices 里"));
            }
        }
    }

    let uses_image = src.contains(":image(") || src.contains(":image (");
    let mut hints: Vec<String> = Vec::new();
    if no_schema {
        hints.push(
            "脚本没有 `schema()`：这是**合法**的（= 面板上没有参数区，`params` 恒空表），\
             不是错误。要参数就补一个返回表的 `schema()`。"
                .to_string(),
        );
    }
    if host == Host::Hover && uses_image {
        hints.push(
            "脚本用了 ctx:image：面板的「图片」浏览行**只在「图片」类型下出现**，\
             其它悬浮标类型拿不到图 → 这段在真机上永远画不出东西。要图片就走 `HoverDot/png`。"
                .into(),
        );
    }
    if host == Host::Hover {
        hints.push(
            "悬浮标脚本的额外约定见 assets/lua/HoverDot/README.md（续帧 / hide_cursor / 坐标系）。"
                .to_string(),
        );
    }
    if !host.hot_reload() {
        hints.push(
            "本目录没有热重载：改完文件要重开面板（或重启）才生效。要边改边看，把脚本放进 `scripts/`。"
                .to_string(),
        );
    }

    let code = if warnings.is_empty() { 0 } else { 3 };
    (
        code,
        json!({
            "ok": true,
            "file": shown,
            "id": id,
            "host": host.host_global(),
            "ui_entry": host.ui_entry(),
            "hot_reload": host.hot_reload(),
            "name": if s("name").is_empty() { s("effect_name") } else { s("name") },
            "role": s("role"),
            "hide_cursor": eng.get_hide_cursor(),
            "uses_image": uses_image,
            "schema_present": has_schema,
            "schema": schema.iter().map(param_json).collect::<Vec<_>>(),
            "warnings": warnings,
            "hints": hints,
            "docs": docs_json(),
        }),
    )
}

/// 一条参数声明 → JSON（AI 需要它来判断"该怎么调这个脚本"）。
fn param_json(def: &crate::scripting::registry::ParamDef) -> Json {
    use crate::scripting::ParamType;
    let ty = match def.ptype {
        ParamType::Int => "int",
        ParamType::Float => "float",
        ParamType::Bool => "bool",
        ParamType::Color => "color",
        ParamType::Choice => "choice",
        ParamType::Str => "str",
        ParamType::Sound => "sound",
        ParamType::Folder => "folder",
        ParamType::File => "file",
        ParamType::Hotkey => "hotkey",
        ParamType::Image => "image",
    };
    let mut v = json!({
        "key": def.key,
        "label": def.label,
        "type": ty,
        "default": def.default,
    });
    if matches!(def.ptype, ParamType::Int | ParamType::Float) {
        v["min"] = json!(def.min);
        v["max"] = json!(def.max);
        v["step"] = json!(def.step);
    }
    if matches!(def.ptype, ParamType::Choice) {
        v["choices"] = json!(def.choices);
    }
    if matches!(def.ptype, ParamType::Color) {
        v["alpha"] = json!(def.alpha);
    }
    v
}

/// `--lua-trace`：跑 N 帧 + 判据。
pub(crate) fn cmd_trace(ctx: &Ctx, path: &str, flags: &Flags) -> (i32, Json) {
    let resolved = match ctx.resolve_script(path) {
        Ok(p) => p,
        Err(e) => return (2, json!({ "ok": false, "error": e })),
    };
    if !resolved.is_file() {
        return (2, missing_script(ctx, &resolved));
    }
    let (root, id, host) = ctx.locate(&resolved, flags.mode);
    let shown = ctx.display(&resolved);
    let opt = flags.opt(host);
    match trace::run(&root, &id, &opt) {
        Ok(rep) => {
            let checks = rep.checks();
            let mut v = rep.to_json(&checks);
            v["ok"] = json!(true);
            v["file"] = json!(shown);
            v["id"] = json!(id);
            v["host"] = json!(host.host_global());
            let code = if checks.passed() { 0 } else { 3 };
            (code, v)
        }
        Err(e) => (
            4,
            json!({ "ok": false, "file": shown, "id": id, "error": e }),
        ),
    }
}

/// `--lua-shot`：离屏出图。
pub(crate) fn cmd_shot(ctx: &Ctx, path: &str, flags: &Flags) -> (i32, Json) {
    let resolved = match ctx.resolve_script(path) {
        Ok(p) => p,
        Err(e) => return (2, json!({ "ok": false, "error": e })),
    };
    if !resolved.is_file() {
        return (2, missing_script(ctx, &resolved));
    }
    let (root, id, host) = ctx.locate(&resolved, flags.mode);
    let opt = flags.opt(host);
    // 默认取"最后一个移动帧"：粒子/尾迹都还在，最有信息量。不取末帧——特效落尽后是空白。
    let at = flags.at.unwrap_or_else(|| opt.default_shot_frame());
    let canvas = (960u32, 540u32);
    let scale = 0.5f32;
    let img = match trace::render_frame(&root, &id, &opt, at, canvas, scale) {
        Ok(b) => b,
        Err(e) => return (4, json!({ "ok": false, "error": e })),
    };

    let rel = flags
        .out
        .clone()
        .unwrap_or_else(|| format!("temp/lua_shot/{}.png", id.replace('/', "_")));
    let dest = match ctx.resolve(&rel) {
        Ok(p) => p,
        Err(e) => return (2, json!({ "ok": false, "error": e })),
    };
    if let Some(dir) = dest.parent() {
        if let Err(e) = std::fs::create_dir_all(dir) {
            return (
                4,
                json!({ "ok": false, "error": format!("建目录失败 {}：{e}", dir.display()) }),
            );
        }
    }
    if let Err(e) = write_png(&dest, canvas.0, canvas.1, &img) {
        return (
            4,
            json!({ "ok": false, "error": format!("写 PNG 失败：{e}") }),
        );
    }
    (
        0,
        json!({
            "ok": true,
            "file": ctx.display(&resolved),
            "id": id,
            "host": host.host_global(),
            "frame": at,
            "out": ctx.display(&dest),
            "size": [canvas.0, canvas.1],
            "note": "背景是「左深右浅 + 四块卡片」：同一个特效在深浅底色上都得看得见。",
        }),
    )
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> std::io::Result<()> {
    let file = std::fs::File::create(path)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(rgba)?;
    Ok(())
}

/// 列出 lua 根下所有脚本（只认四类挂载点，最多两层目录）。
///
/// 给 `list_scripts` 用：AI 写新脚本前先看同类脚本叫什么、放哪里——比读一篇文档有效。
pub(crate) fn list_scripts(ctx: &Ctx) -> Vec<Json> {
    let mut out: Vec<Json> = Vec::new();
    let dirs = ["MouseTrail", "HoverDot", "ClickFx", "KeySound", "scripts"];
    for top in dirs {
        let Some(host) = Host::from_top_dir(top) else {
            continue;
        };
        collect(ctx, &ctx.lua.join(top), top, host, 2, &mut out);
    }
    out.sort_by(|a, b| {
        a["id"]
            .as_str()
            .unwrap_or("")
            .cmp(b["id"].as_str().unwrap_or(""))
    });
    out
}

fn collect(ctx: &Ctx, dir: &Path, prefix: &str, host: Host, depth: u32, out: &mut Vec<Json>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if depth > 0 {
                if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                    collect(ctx, &p, &format!("{prefix}/{name}"), host, depth - 1, out);
                }
            }
            continue;
        }
        if p.extension().and_then(|e| e.to_str()) != Some("lua") {
            continue;
        }
        let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        out.push(json!({
            "id": format!("{prefix}/{stem}"),
            "file": ctx.display(&p),
            "host": host.host_global(),
            "ui_entry": host.ui_entry(),
            "hot_reload": host.hot_reload(),
        }));
    }
}

fn usage(out: &mut Out, msg: &str) -> i32 {
    let _ = writeln!(out, "错误：{msg}\n");
    let _ = out.write_all(help().as_bytes());
    2
}

fn emit(out: &mut Out, v: &Json) {
    let _ = writeln!(
        out,
        "{}",
        serde_json::to_string_pretty(v).unwrap_or_default()
    );
    let _ = out.flush();
}

/// 随包文档：工具输出里给 AI 的**唯一**"该读什么"清单。
///
/// 为什么写死在代码里而不是让调用方自己找：路径是契约。文件被移动/改名时
/// `tests/docs_consistency.rs::test_lua_tool_docs_point_at_real_files` 会红，
/// 于是 AI 看到的路径**永远不会指向空**——"照着提示读，读到 404"比没有提示更糟。
pub const DOCS: &[(&str, &str)] = &[
    (
        "assets/docs/接入指南.md",
        "接入第一站：怎么连 MCP / 命令行、八个工具、四步闭环、什么算通过",
    ),
    (
        "assets/docs/MCP说明.md",
        "MCP 协议 / 八个工具签名 / 参数默认值口径 / 错误码与排查",
    ),
    (
        "assets/docs/README.md",
        "文档索引：哪份文档讲什么、AI 的阅读顺序",
    ),
    (
        "assets/docs/配置说明.md",
        "config.json 每个键怎么填 + 案例（启用脚本 / 改参数前必读）",
    ),
    ("assets/lua/API.md", "Lua 接口权威表（与代码同源校验）"),
    (
        "assets/docs/隐藏快捷键手册.md",
        "面板上不显示的固定快捷键清单（纯文本版）",
    ),
    (
        "assets/lua/HoverDot/README.md",
        "悬浮标脚本的额外约定（续帧 / hide_cursor / 坐标系）",
    ),
    (
        "assets/lua/Lua脚本手册.html",
        "通读版脚本手册（人看，浏览器打开）",
    ),
];

/// 把 [`DOCS`] 渲染成两列文本（`--help` 用）。
fn doc_lines() -> String {
    let mut out = String::new();
    for (path, about) in DOCS {
        out.push_str(&format!("  {}{about}\n", pad_display(path, 36)));
    }
    out
}

/// 按**显示宽度**右补空格：`{:<36}` 补的是字符数，中文路径占两列，直接用会歪。
fn pad_display(s: &str, width: usize) -> String {
    let cols: usize = s
        .chars()
        .map(|c| if (c as u32) >= 0x2E80 { 2 } else { 1 })
        .sum();
    let mut out = s.to_string();
    out.push_str(&" ".repeat(width.saturating_sub(cols)));
    out
}

/// [`DOCS`] 的 JSON 版（`--lua-check` 的返回值用）。
pub(crate) fn docs_json() -> Vec<Json> {
    DOCS.iter()
        .map(|(path, about)| json!({ "path": path, "about": about }))
        .collect()
}

fn help() -> String {
    format!(
        r#"CurSeen Lua 工具台（发行版内建）

用法（路径一律**相对于本程序目录**，例如 assets/lua/HoverDot/foo.lua）：

  curseen.exe --lua-check <脚本>            能不能加载 / schema 有没有隐患
  curseen.exe --lua-trace <脚本>            跑 N 帧并给出 pass/fail（默认 300 帧）
  curseen.exe --lua-shot  <脚本>            离屏出一张 PNG（默认 temp/lua_shot/）
  curseen.exe --mcp                         以 MCP 服务器运行（stdio）

通用旗标：
  --frames N            跑多少帧（trace/shot，默认 300）
  --at N                截取第几帧出图（默认 = 最后一个"指针还在动"的帧）
  --screen 1920x1080    逻辑屏幕尺寸（默认 1920x1080）
  --mode hover|trail|click|composite   强制宿主（默认按所在目录推断）
  --param key=value     覆盖一个参数，可重复（如 --param count=10）
  --input sweep|idle-swing
                        合成输入轮廓：sweep = 扫过画面后停住（默认）；
                        idle-swing = 先静止 1.5s 再左右回摆（「晃动定位」这类静止触发的脚本）
  --out 路径            --lua-shot 的输出（相对程序目录，默认 temp/lua_shot/<id>.png）

退出码：0 通过 / 2 用法错误 / 3 有警告或判据不过 / 4 加载或运行出错

文档（路径一律相对本程序目录，都在 assets/ 里）：
{docs}
  MCP 客户端（没有文件权限）：read_doc 读上面这几份 .md，
  write_script 落盘脚本，set_active 启用/写参数，delete_script 清理（移到 temp/lua_tool_trash/）。

脚本放哪儿（这条决定用户能不能在面板里选到它）：
  assets/lua/MouseTrail/   拖尾特效下拉
  assets/lua/HoverDot/     悬浮标类型下拉
  assets/lua/ClickFx/      点击效果下拉
  assets/lua/scripts/      「脚本」页的常驻卡片（**只有这里热重载**）

判据（trace 自动算，不需要你自己比对数字）：
  first_frame_draws     第 0 帧必须画出东西（宿主按"这一帧画不画"判活）
  draws_while_moving    指针移动期间必须有产出
  settles               悬浮标/复合：指针停下后 draw 必须停止返回续帧（静止 0 功耗）
  stops                 点击效果：必须自行停笔（否则实例永不释放）
  cmds_sane             单帧指令数不超过 {MAX_CMDS}
  draws_at_all          整段跑完至少有一条指令

样例：
  curseen.exe --lua-check assets/lua/HoverDot/solid.lua
  curseen.exe --lua-trace assets/lua/HoverDot/springy_img.lua --frames 400
  curseen.exe --lua-shot  assets/lua/MouseTrail/ribbon.lua --out temp/shot/ribbon.png
  curseen.exe --lua-trace assets/lua/HoverDot/springy_img.lua --param count=10
"#,
        MAX_CMDS = trace::MAX_CMDS_PER_FRAME,
        docs = doc_lines(),
    )
}

/// 标准输出的落地方式。
///
/// GUI 子系统的 exe 从 cmd/pwsh 直接启动时**没有**控制台：`GetStdHandle` 拿到空句柄，
/// `println!` 写进虚空（AI 看到的就是"命令跑完零输出"）。此时挂到父进程的控制台再打开
/// `CONOUT$`；被父进程用管道 spawn（MCP 客户端就是这样）时句柄本来就有效，直接用。
pub struct Out {
    sink: Sink,
}

enum Sink {
    Stdout(std::io::Stdout),
    Console(std::fs::File),
    /// 双开口都没有：输出丢弃，但退出码仍然有效（判据不该因为"没地方打印"而失真）。
    Nowhere,
}

impl Out {
    fn open() -> Self {
        if stdout_usable() {
            return Self {
                sink: Sink::Stdout(std::io::stdout()),
            };
        }
        if attach_parent_console() {
            if let Ok(f) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
                return Self {
                    sink: Sink::Console(f),
                };
            }
        }
        Self {
            sink: Sink::Nowhere,
        }
    }
}

impl Write for Out {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match &mut self.sink {
            Sink::Stdout(s) => s.write(buf),
            Sink::Console(f) => f.write(buf),
            Sink::Nowhere => Ok(buf.len()),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match &mut self.sink {
            Sink::Stdout(s) => s.flush(),
            Sink::Console(f) => f.flush(),
            Sink::Nowhere => Ok(()),
        }
    }
}

#[cfg(windows)]
fn stdout_usable() -> bool {
    use windows::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows::Win32::System::Console::{GetStdHandle, STD_OUTPUT_HANDLE};
    // SAFETY: 纯查询，不接管任何所有权。
    let h: HANDLE = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) }.unwrap_or(INVALID_HANDLE_VALUE);
    !h.is_invalid() && !h.0.is_null()
}

#[cfg(windows)]
fn attach_parent_console() -> bool {
    use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    // SAFETY: 只把本进程挂到父进程的控制台上；失败返回错误，不改变任何状态。
    unsafe { AttachConsole(ATTACH_PARENT_PROCESS).is_ok() }
}

#[cfg(not(windows))]
fn stdout_usable() -> bool {
    true
}

#[cfg(not(windows))]
fn attach_parent_console() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> (tempdir::Temp, Ctx) {
        let dir = tempdir::Temp::new("curseen_lua_tool");
        let ctx = Ctx::from_app_dir(dir.path().to_path_buf());
        std::fs::create_dir_all(dir.path().join("assets/lua/HoverDot")).expect("建目录");
        (dir, ctx)
    }

    /// 脚本路径两种写法必须等价：`HoverDot/foo` ≡ `assets/lua/HoverDot/foo.lua`。
    ///
    /// 这条挡的是"刚写完文件，check 却说读不到"：`write_script` 收简写并把它作为 `id`
    /// 返回，调用方自然会再喂回 check / trace；两边只要有一边不认，就是最费时间的假故障。
    #[test]
    fn script_path_accepts_mount_shorthand() {
        let (_d, ctx) = ctx();
        let long = ctx
            .resolve("assets/lua/HoverDot/solid.lua")
            .expect("长写法");
        for short in [
            "HoverDot/solid",
            "HoverDot/solid.lua",
            "assets/lua/HoverDot/solid",
        ] {
            assert_eq!(
                ctx.resolve_script(short).expect("简写应被归一"),
                long,
                "`{short}` 应等价于长写法"
            );
        }
        // 归一之后仍然不许越界（`..` 由 resolve 拒绝，不能被前缀拼接绕过去）
        assert!(ctx
            .resolve_script("HoverDot/../../../Windows/win.ini")
            .is_err());
    }

    /// 路径解析：相对 → 程序目录；绝对且在内 → 放行；`..` 与目录外 → 拒绝。
    #[test]
    fn resolve_accepts_relative_and_rejects_escape() {
        let (dir, ctx) = ctx();
        let ok = ctx
            .resolve("assets/lua/HoverDot/x.lua")
            .expect("相对路径应放行");
        assert_eq!(ok, dir.path().join("assets/lua/HoverDot/x.lua"));
        let abs = dir.path().join("assets/lua/HoverDot/y.lua");
        assert!(
            ctx.resolve(abs.to_str().expect("utf8")).is_ok(),
            "程序目录内的绝对路径应放行"
        );
        assert!(
            ctx.resolve("assets/../../etc/passwd").is_err(),
            "`..` 必须拒绝（便携铁律：只碰程序目录内）"
        );
        assert!(
            ctx.resolve(if cfg!(windows) {
                "C:/Windows/win.ini"
            } else {
                "/etc/passwd"
            })
            .is_err(),
            "程序目录外的绝对路径必须拒绝"
        );
    }

    /// 目录 → 宿主：决定"用户能在哪个下拉里选到它"，判错就等于告诉 AI 错误的信息。
    #[test]
    fn locate_maps_directory_to_host() {
        let (dir, ctx) = ctx();
        let p = dir.path().join("assets/lua/HoverDot/foo.lua");
        let (root, id, host) = ctx.locate(&p, None);
        assert_eq!(
            id, "HoverDot/foo",
            "id 必须是相对 lua 根的路径（与生产宿主同源）"
        );
        assert_eq!(root, dir.path().join("assets/lua"));
        assert_eq!(host, Host::Hover);

        let scripts = dir.path().join("assets/lua/scripts/mine.lua");
        assert_eq!(ctx.locate(&scripts, None).2, Host::Composite);
        assert!(Host::Composite.hot_reload(), "只有 scripts/ 有热重载");
        assert!(!Host::Hover.hot_reload());
    }

    /// 旗标解析：`--param` 的值按 JSON 解析，失败退回字符串。
    #[test]
    fn flags_parse_params_as_json() {
        let args: Vec<String> = [
            "--frames",
            "120",
            "--param",
            "count=10",
            "--param",
            "color=#FF0000",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let f = Flags::parse(&args).expect("应解析成功");
        assert_eq!(f.frames, Some(120));
        assert_eq!(f.params["count"], json!(10));
        assert_eq!(
            f.params["color"],
            json!("#FF0000"),
            "不是合法 JSON 就按字符串"
        );
    }

    /// `--mode` 认四类宿主，写错必须报错（静默回落会让判据跑在错的宿主上）。
    #[test]
    fn flags_reject_unknown_mode() {
        let args: Vec<String> = ["--mode", "nope"].iter().map(|s| s.to_string()).collect();
        assert!(Flags::parse(&args).is_err());
    }
}

#[cfg(test)]
mod tempdir {
    //! 极简临时目录（不引第三方依赖）：Drop 时删干净。
    use std::path::{Path, PathBuf};

    pub struct Temp(PathBuf);

    impl Temp {
        pub fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("{tag}_{}_{:x}", std::process::id(), {
                use std::time::{SystemTime, UNIX_EPOCH};
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            }));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).expect("建临时目录");
            Self(p)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod schema_tests {
    use super::*;

    /// 没有 `schema()` 是**合法**的（= 没有参数面板），不能报成"schema() 调用失败"。
    ///
    /// 这条曾经是红的：一个最简脚本（只写 `draw`）被 `--lua-check` 判成 `ok:false`，
    /// 提示"schema() 抛异常等于这个脚本没有参数面板"——写脚本的人会去补一个根本不需要的
    /// `schema()`，而真正的错（如果有）被这个噪音盖住。
    #[test]
    fn missing_schema_is_legal_and_says_so() {
        let ctx = Ctx::from_app_dir(std::env::temp_dir().join("curseen_no_schema_probe"));
        let lua = ctx.lua.clone();
        std::fs::create_dir_all(lua.join("HoverDot")).expect("建目录");
        let f = lua.join("HoverDot/minimal.lua");
        std::fs::write(
            &f,
            "name = \"minimal\"\nrole = \"effect\"\nfunction draw(ctx, p)\n  ctx:circle(mouse_x, mouse_y, 8, 255, 255, 255, 255)\nend\n",
        )
        .expect("写脚本");

        let (code, v) = cmd_check(&ctx, "assets/lua/HoverDot/minimal.lua", &Flags::default());
        assert_eq!(code, 0, "无 schema 不该判错：{v}");
        assert_eq!(v["schema_present"], false);
        assert_eq!(v["schema"].as_array().map(|a| a.len()), Some(0));
        let hints = v["hints"].to_string();
        assert!(
            hints.contains("合法"),
            "必须说清这是合法的，实际 hints={hints}"
        );

        let _ = std::fs::remove_file(&f);
    }

    /// 写了 `schema()` 但抛异常，仍然是硬错误（别把上面那条放宽成"schema 坏了也过"）。
    #[test]
    fn broken_schema_is_still_an_error() {
        let ctx = Ctx::from_app_dir(std::env::temp_dir().join("curseen_bad_schema_probe"));
        let lua = ctx.lua.clone();
        std::fs::create_dir_all(lua.join("HoverDot")).expect("建目录");
        let f = lua.join("HoverDot/broken.lua");
        std::fs::write(
            &f,
            "name = \"broken\"\nrole = \"effect\"\nfunction schema()\n  error(\"故意抛\")\nend\nfunction draw(ctx, p) end\n",
        )
        .expect("写脚本");

        let (code, v) = cmd_check(&ctx, "assets/lua/HoverDot/broken.lua", &Flags::default());
        assert_ne!(code, 0, "schema 抛异常必须判错：{v}");
        assert_eq!(v["ok"], false);
        assert!(
            v["error"].to_string().contains("schema"),
            "报错要点名 schema：{v}"
        );

        let _ = std::fs::remove_file(&f);
    }
}
