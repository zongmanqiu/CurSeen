//! 工具台的**写入半边**：把脚本落盘、把配置改到"用户在面板里能选到它"。
//!
//! 为什么需要它：`--lua-check` / `--lua-trace` 只是"判"，而"写"以前只有会用 shell 的
//! agent 才做得到。只会调工具的客户端（Claude Desktop 那类）因此连第一步都走不了——
//! 给它判据却不能给它笔，闭环就是断的。
//!
//! # 三条纪律
//!
//! 1. **只写程序目录内**（便携铁律）：脚本必须在 `assets/lua/` 的挂载目录下，配置只能是
//!    本程序目录里的 `config.json`。`..` 与目录外的绝对路径一律拒绝（复用 [`Ctx::resolve`]）。
//! 2. **不覆盖既有脚本**：默认 `overwrite=false`。出厂脚本被同名写入而悄悄消失，是最难
//!    事后发现的一类事故——错的是调用方，不是被覆盖的文件。
//! 3. **删 = 移到 `temp/lua_tool_trash/`**，不真删。清理探针不该有"手一抖就没了"的代价。
//!
//! # 为什么不走 `core::config::Config`
//!
//! `serde` 反序列化会**丢掉本版本不认识的键**（结构体没开 `deny_unknown_fields`），
//! 于是一次"改个开关"就可能把用户配置里更新的字段抹掉。所以这里在 `serde_json::Value`
//! 上按路径改，其余内容不动；路径与结构体字段的一致性由单测
//! `every_config_path_seeded_by_the_tool_exists` 钉住（拿 `Config::default()` 的序列化
//! 结果对账）：字段改名会让它红，而不是让工具静默写到没人读的键上。

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value as Json};

use super::trace::Host;
use super::Ctx;

/// 单个脚本的字节上限：256 KB。真实脚本都在 30 KB 以内，超了几乎必然是调用方传错。
const MAX_SCRIPT_BYTES: usize = 256 * 1024;

/// 允许写入的挂载目录（路径第一段）。`scripts/` 允许再往下建包目录。
const MOUNTS: [&str; 5] = ["MouseTrail", "HoverDot", "ClickFx", "KeySound", "scripts"];

/// 一次失败：退出码 + 给调用方看的 JSON。两个命令都靠它把错误整理成同一形状。
struct Fail {
    code: i32,
    json: Json,
}

impl Fail {
    /// 用法错误（2）：参数本身不成立。
    fn arg(msg: impl Into<String>) -> Self {
        Self {
            code: 2,
            json: json!({ "ok": false, "error": msg.into() }),
        }
    }
    /// 拒绝执行（3）：调用方需要先做个决定（覆盖 / 先启动一次程序）。
    fn state(msg: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            code: 3,
            json: json!({ "ok": false, "error": msg.into(), "hint": hint.into() }),
        }
    }
    /// 环境或 IO 出错（4）。
    fn io(msg: impl Into<String>) -> Self {
        Self {
            code: 4,
            json: json!({ "ok": false, "error": msg.into() }),
        }
    }
    fn at(self, path: &Path, ctx: &Ctx) -> Self {
        let mut j = self.json;
        if let Some(o) = j.as_object_mut() {
            o.insert("path".into(), json!(ctx.display(path)));
        }
        Self {
            code: self.code,
            json: j,
        }
    }
}

type R<T> = Result<T, Fail>;

/// 把脚本写到 `assets/lua/<挂载目录>/<名字>.lua`。
///
/// 返回 `{ path, id, mount, ui_entry, hot_reload, bytes, created, next }`——其中
/// `ui_entry` / `hot_reload` 是"写完之后用户在哪能看到它、要不要重开面板"的答案，
/// 调用方不必再自己推断。
pub(crate) fn write_script(ctx: &Ctx, path: &str, code: &str, overwrite: bool) -> (i32, Json) {
    match write_inner(ctx, path, code, overwrite) {
        Ok(v) => (0, v),
        Err(f) => (f.code, f.json),
    }
}

fn write_inner(ctx: &Ctx, path: &str, code: &str, overwrite: bool) -> R<Json> {
    let (full, id, host) = locate_script(ctx, path).map_err(|e| e.at(Path::new(path), ctx))?;
    if code.trim().is_empty() {
        return Err(Fail::arg("code 是空的").at(&full, ctx));
    }
    if code.len() > MAX_SCRIPT_BYTES {
        return Err(Fail::arg(format!(
            "脚本 {} 字节，超过上限 {MAX_SCRIPT_BYTES}：几乎必然是传错了内容",
            code.len()
        ))
        .at(&full, ctx));
    }
    let existed = full.is_file();
    if existed && !overwrite {
        return Err(Fail {
            code: 3,
            json: json!({
                "ok": false,
                "exists": true,
                "error": "同名脚本已存在，未写入（避免覆盖别人或出厂脚本）。",
                "hint": "换个文件名；确认要改它就把 overwrite 设为 true。",
            }),
        }
        .at(&full, ctx));
    }
    if let Some(dir) = full.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| Fail::io(format!("建目录失败：{e}")).at(&full, ctx))?;
    }
    // 行尾统一 LF：CRLF 会让 Lua 报错的行号与编辑器对不上。
    let body = code.replace("\r\n", "\n");
    std::fs::write(&full, body.as_bytes())
        .map_err(|e| Fail::io(format!("写文件失败：{e}")).at(&full, ctx))?;
    Ok(json!({
        "ok": true,
        "path": ctx.display(&full),
        "id": id,
        "mount": host.host_global(),
        "ui_entry": host.ui_entry(),
        "hot_reload": host.hot_reload(),
        "bytes": body.len(),
        "created": !existed,
        "next": "接着跑 lua_check 看语法与参数体检，再 lua_trace 拿 verdict。",
    }))
}

/// 删除（= 移到 `temp/lua_tool_trash/`）一个脚本。**不真删**，所以任何脚本都允许删。
pub(crate) fn delete_script(ctx: &Ctx, path: &str) -> (i32, Json) {
    let fail = |f: Fail, p: &Path| (f.code, f.at(p, ctx).json);
    let (full, id, _host) = match locate_script(ctx, path) {
        Ok(v) => v,
        Err(f) => return (f.code, f.json),
    };
    if !full.is_file() {
        return fail(Fail::arg("文件不存在"), &full);
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let name = full
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "script.lua".to_string());
    let trash = ctx.app.join("temp").join("lua_tool_trash");
    if let Err(e) = std::fs::create_dir_all(&trash) {
        return (
            4,
            json!({ "ok": false, "error": format!("建回收目录失败：{e}") }),
        );
    }
    let dest = trash.join(format!("{stamp}_{name}"));
    match std::fs::rename(&full, &dest) {
        Ok(()) => (
            0,
            json!({
                "ok": true,
                "deleted": id,
                "moved_to": ctx.display(&dest),
                "note": "没有真删：移到 temp/lua_tool_trash/，要找回就从这里拿。",
            }),
        ),
        Err(e) => (4, json!({ "ok": false, "error": format!("移动失败：{e}") })),
    }
}

/// 把某个脚本设成"用户能选到它的状态"，并可顺带写入参数。
///
/// 这是**按挂载点改写对应配置键**的任务级工具：调用方只要说"启用这个脚本、参数是这样"，
/// 不必记 `hover_params` 用裸 stem、拖尾参数是扁平命名空间、点击要落到
/// `ripple_bindings.left` 这些历史差异（它们写在 `assets/docs/配置说明.md` 里，但让工具
/// 替调用方记更可靠）。
///
/// `script` 收三种写法：`assets/lua/HoverDot/foo.lua`、`HoverDot/foo`、`HoverDot/foo.lua`。
pub(crate) fn set_active(
    ctx: &Ctx,
    script: &str,
    enabled: bool,
    button: Option<&str>,
    params: Option<&Json>,
) -> (i32, Json) {
    match set_active_inner(ctx, script, enabled, button, params) {
        Ok(v) => (0, v),
        Err(f) => (f.code, f.json),
    }
}

fn set_active_inner(
    ctx: &Ctx,
    script: &str,
    enabled: bool,
    button: Option<&str>,
    params: Option<&Json>,
) -> R<Json> {
    let (full, id, host) = locate_script(ctx, script)?;
    let bare = bare_stem(&id);
    let cfg_path = ctx.app.join("config.json");
    if !cfg_path.is_file() {
        return Err(Fail::state(
            "程序目录里没有 config.json。",
            "先启动一次 CurSeen（它会写一份出厂配置），再来改。",
        ));
    }
    let text = std::fs::read_to_string(&cfg_path)
        .map_err(|e| Fail::io(format!("读 config.json 失败：{e}")))?;
    let mut root: Json = serde_json::from_str(&text).map_err(|e| {
        Fail::io(format!(
            "config.json 不是合法 JSON（{e}）：没动它，先修好再来。"
        ))
    })?;
    let obj = root
        .as_object_mut()
        .ok_or_else(|| Fail::io("config.json 顶层不是对象：没动它。"))?;

    let mut changed: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    match host {
        Host::Hover => {
            // 悬浮标：开关在 hover_enabled；选中项是**裸 stem**（与 hover_active 的带前缀
            // 口径不同，历史原因，工具替调用方抹平）。
            let t = obj_map(obj, "trail")?;
            put(
                t,
                "hover_enabled",
                json!(enabled),
                "trail.hover_enabled",
                &mut changed,
            );
            if enabled {
                put(
                    t,
                    "hover_type",
                    json!(bare),
                    "trail.hover_type",
                    &mut changed,
                );
            }
            if let Some(p) = params {
                let m = child_map(t, "hover_params", &bare, "trail.hover_params")?;
                merge(m, p, &format!("trail.hover_params.{bare}"), &mut changed);
            }
        }
        Host::Trail => {
            let t = obj_map(obj, "trail")?;
            put(
                t,
                "trail_enabled",
                json!(enabled),
                "trail.trail_enabled",
                &mut changed,
            );
            if enabled {
                put(
                    t,
                    "trail_effect",
                    json!(bare),
                    "trail.trail_effect",
                    &mut changed,
                );
            }
            if let Some(p) = params {
                // 拖尾参数是**扁平命名空间**：`extra["<裸stem>.<key>"]`，不是嵌套对象。
                let ts = obj_map(t, "trail_settings")?;
                let extra = obj_map(ts, "extra")?;
                let Some(p) = p.as_object() else {
                    return Err(Fail::arg("params 不是对象"));
                };
                for (k, v) in p {
                    let key = format!("{bare}.{k}");
                    changed.push(format!("trail.trail_settings.extra.{key}"));
                    extra.insert(key, v.clone());
                }
            }
        }
        Host::Click => {
            let key = button.unwrap_or("left");
            if !["left", "right", "middle", "x1", "x2"].contains(&key) {
                return Err(Fail::arg(format!(
                    "button 只认 left/right/middle/x1/x2：{key}"
                )));
            }
            let t = obj_map(obj, "trail")?;
            put(
                t,
                "ripple_enabled",
                json!(true),
                "trail.ripple_enabled",
                &mut changed,
            );
            let binds = obj_map(t, "ripple_bindings")?;
            let b = obj_map(binds, key)?;
            put(
                b,
                "enabled",
                json!(enabled),
                &format!("ripple_bindings.{key}.enabled"),
                &mut changed,
            );
            if enabled {
                put(
                    b,
                    "script",
                    json!(bare),
                    &format!("ripple_bindings.{key}.script"),
                    &mut changed,
                );
            }
            if let Some(p) = params {
                let sp = obj_map(b, "script_params")?;
                merge(
                    sp,
                    p,
                    &format!("ripple_bindings.{key}.script_params"),
                    &mut changed,
                );
            }
        }
        Host::Composite => {
            let lx = obj_map(obj, "lua_ext")?;
            let list = arr_field(lx, "composite_active")?;
            changed.push("lua_ext.composite_active".into());
            list.retain(|v| v.as_str() != Some(id.as_str()));
            if enabled {
                list.push(json!(id));
            }
            if let Some(p) = params {
                let m = child_map(lx, "params", &id, "lua_ext.params")?;
                merge(m, p, &format!("lua_ext.params.{id}"), &mut changed);
            }
        }
        Host::KeySound => {
            let ks = obj_map(obj, "key_sound")?;
            put(
                ks,
                "enabled",
                json!(enabled),
                "key_sound.enabled",
                &mut changed,
            );
            if enabled {
                put(ks, "script", json!(id), "key_sound.script", &mut changed);
            }
            if let Some(p) = params {
                let m = child_map(ks, "params", &id, "key_sound.params")?;
                merge(m, p, &format!("key_sound.params.{id}"), &mut changed);
            }
        }
    }
    if changed.is_empty() {
        return Err(Fail::io("没能定位到要改的配置键（配置结构变了？）"));
    }
    write_json_atomic(&cfg_path, &root)
        .map_err(|e| Fail::io(format!("写 config.json 失败：{e}")))?;

    notes.push(
        "若 CurSeen 正在运行：它会用内存里的配置覆盖本文件。**先退出程序再改**，或改完立刻重启\
         （首次启用必须重启一次；之后改脚本内容靠 scripts/ 热重载，不必重启）。"
            .to_string(),
    );
    if !full.is_file() {
        notes.push(format!(
            "注意：`{}` 目前不存在（配置先写好了）。脚本落盘后才会真的生效。",
            ctx.display(&full)
        ));
    }
    Ok(json!({
        "ok": true,
        "config": "config.json",
        "id": id,
        "mount": host.host_global(),
        "enabled": enabled,
        "changed": changed,
        "notes": notes,
    }))
}

/// 路径 / id →（绝对路径, id, 宿主）。
fn locate_script(ctx: &Ctx, raw: &str) -> R<(PathBuf, String, Host)> {
    let mut s = raw.trim().replace('\\', "/");
    if s.is_empty() {
        return Err(Fail::arg("缺 path"));
    }
    if let Some(rest) = s.strip_prefix("assets/lua/") {
        s = rest.to_string();
    }
    if let Some(rest) = s.strip_suffix(".lua") {
        s = rest.to_string();
    }
    let Some((top, rest)) = s.split_once('/') else {
        return Err(Fail::arg(format!(
            "`{raw}` 没带挂载目录：写成 `HoverDot/foo`（或 `scripts/包名/foo`）。\
             只给文件名无法判断它该出现在哪个下拉里。"
        )));
    };
    if !MOUNTS.contains(&top) {
        return Err(Fail::arg(format!(
            "`{top}` 不是合法的挂载目录（只认 {}）",
            MOUNTS.join(" / ")
        )));
    }
    if rest.is_empty() {
        return Err(Fail::arg(format!("`{raw}` 只有目录没有文件名")));
    }
    let host = Host::from_top_dir(top)
        .ok_or_else(|| Fail::arg(format!("`{top}` 没有对应的宿主，工具不支持写入")))?;
    let id = format!("{top}/{rest}");
    let full = ctx
        .resolve(&format!("assets/lua/{id}.lua"))
        .map_err(Fail::arg)?;
    // 双保险：解析后必须仍在 lua 根下（`resolve` 已拒 `..`，这里防符号链接之类的意外）。
    if !full.starts_with(&ctx.lua) {
        return Err(Fail::arg("脚本必须落在 assets/lua/ 下"));
    }
    Ok((full, id, host))
}

/// `HoverDot/foo` → `foo`；`scripts/包/foo` → `foo`。
fn bare_stem(id: &str) -> String {
    id.rsplit('/').next().unwrap_or(id).to_string()
}

// ── JSON 路径工具：缺失就建，类型不符就报错（不猜、不覆盖别的类型） ──

fn obj_map<'a>(parent: &'a mut Map<String, Json>, key: &str) -> R<&'a mut Map<String, Json>> {
    let slot = parent.entry(key).or_insert_with(|| json!({}));
    slot.as_object_mut()
        .ok_or_else(|| Fail::io(format!("config.json 里的 `{key}` 不是对象，没敢动它")))
}

fn arr_field<'a>(parent: &'a mut Map<String, Json>, key: &str) -> R<&'a mut Vec<Json>> {
    let slot = parent.entry(key).or_insert_with(|| json!([]));
    slot.as_array_mut()
        .ok_or_else(|| Fail::io(format!("config.json 里的 `{key}` 不是数组，没敢动它")))
}

/// `parent[key][inner]` 拿成对象（缺就建）。
fn child_map<'a>(
    parent: &'a mut Map<String, Json>,
    key: &str,
    inner: &str,
    label: &str,
) -> R<&'a mut Map<String, Json>> {
    let slot = obj_map(parent, key)?;
    let entry = slot.entry(inner.to_string()).or_insert_with(|| json!({}));
    entry
        .as_object_mut()
        .ok_or_else(|| Fail::io(format!("config.json 里的 `{label}.{inner}` 不是对象")))
}

/// 值不同才记一笔（`changed` 是"真的改了什么"，不是"摸过什么"）。
fn put(parent: &mut Map<String, Json>, key: &str, v: Json, label: &str, changed: &mut Vec<String>) {
    if parent.get(key) != Some(&v) {
        changed.push(label.to_string());
        parent.insert(key.to_string(), v);
    }
}

fn merge(target: &mut Map<String, Json>, src: &Json, prefix: &str, changed: &mut Vec<String>) {
    for (k, v) in src.as_object().into_iter().flatten() {
        if target.get(k) != Some(v) {
            changed.push(format!("{prefix}.{k}"));
            target.insert(k.clone(), v.clone());
        }
    }
}

/// 原子写 JSON：先写 `.tmp` 再 rename，避免中途断电留下半个配置文件。
fn write_json_atomic(path: &Path, v: &Json) -> Result<(), String> {
    let text = serde_json::to_string_pretty(v).map_err(|e| format!("序列化失败：{e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text.as_bytes()).map_err(|e| format!("写临时文件失败：{e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("替换配置失败：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::Config;

    /// 一个"已经跑过一次"的程序目录：有出厂 config.json 和 assets/lua 骨架。
    fn app(tag: &str) -> (PathBuf, Ctx) {
        let p = std::env::temp_dir().join(format!("curseen_write_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(p.join("assets/lua/HoverDot")).expect("建目录");
        let cfg = serde_json::to_string_pretty(&Config::default()).expect("序列化出厂配置");
        std::fs::write(p.join("config.json"), cfg).expect("写出厂配置");
        (p.clone(), Ctx::from_app_dir(p))
    }

    fn read_cfg(c: &Ctx) -> Config {
        let text = std::fs::read_to_string(c.app.join("config.json")).expect("读配置");
        serde_json::from_str(&text).expect("配置应能被本版本的 Config 反序列化")
    }

    /// 写完的配置必须能被 `Config` **按结构体字段**读回来。
    ///
    /// 这比"检查 JSON 里有没有这个键"强得多：路径写错（比如把 `hover_params` 写成
    /// `hover_param`）时 `serde` 会静默忽略，而这里的断言会红。
    #[test]
    fn set_active_writes_keys_the_program_actually_reads() {
        // 悬浮标：选中项用裸 stem，参数按脚本 id 存
        let (dir, ctx) = app("hover");
        let (code, v) = set_active(
            &ctx,
            "HoverDot/foo",
            true,
            None,
            Some(&json!({ "size": 18 })),
        );
        assert_eq!(code, 0, "应成功：{v}");
        let cfg = read_cfg(&ctx);
        assert!(cfg.trail.hover_enabled);
        assert_eq!(cfg.trail.hover_type, "foo", "选中项必须是裸 stem");
        assert_eq!(cfg.trail.hover_params["foo"]["size"], json!(18));
        let _ = std::fs::remove_dir_all(&dir);

        // 拖尾：扁平命名空间键
        let (dir, ctx) = app("trail");
        set_active(
            &ctx,
            "MouseTrail/bar.lua",
            true,
            None,
            Some(&json!({ "max_width": 28 })),
        );
        let cfg = read_cfg(&ctx);
        assert!(cfg.trail.trail_enabled);
        assert_eq!(cfg.trail.trail_effect, "bar");
        assert_eq!(
            cfg.trail.trail_settings.extra["bar.max_width"],
            json!(28),
            "拖尾参数是 裸stem.key 的扁平键"
        );
        let _ = std::fs::remove_dir_all(&dir);

        // 点击：落到 ripple_bindings.<键>.script / script_params
        let (dir, ctx) = app("click");
        set_active(
            &ctx,
            "ClickFx/ring",
            true,
            Some("right"),
            Some(&json!({ "color": "#22D3EE" })),
        );
        let cfg = read_cfg(&ctx);
        assert!(cfg.trail.ripple_enabled);
        let b = &cfg.trail.ripple_bindings["right"];
        assert!(b.enabled);
        assert_eq!(b.script, "ring");
        assert_eq!(b.script_params["color"], json!("#22D3EE"));
        let _ = std::fs::remove_dir_all(&dir);

        // 脚本页常驻：列表里是带 scripts/ 前缀的 id
        let (dir, ctx) = app("composite");
        set_active(
            &ctx,
            "scripts/demo",
            true,
            None,
            Some(&json!({ "radius": 12 })),
        );
        let cfg = read_cfg(&ctx);
        assert_eq!(cfg.lua_ext.composite_active, vec!["scripts/demo"]);
        assert_eq!(cfg.lua_ext.params["scripts/demo"]["radius"], json!(12));
        // 再停用：从列表里摘掉（参数保留）
        set_active(&ctx, "scripts/demo", false, None, None);
        let cfg = read_cfg(&ctx);
        assert!(cfg.lua_ext.composite_active.is_empty());
        assert_eq!(cfg.lua_ext.params["scripts/demo"]["radius"], json!(12));
        let _ = std::fs::remove_dir_all(&dir);

        // 键音：script 带 KeySound/ 前缀
        let (dir, ctx) = app("keysound");
        set_active(&ctx, "KeySound/unified_sound", true, None, None);
        let cfg = read_cfg(&ctx);
        assert!(cfg.key_sound.enabled);
        assert_eq!(cfg.key_sound.script, "KeySound/unified_sound");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 改配置只动该动的键：别的键（含本版本不认识的键）必须原样留着。
    ///
    /// 这条挡的是"用 `Config` 反序列化再写回"那种实现——它会悄悄丢掉未来版本的字段。
    #[test]
    fn set_active_keeps_unrelated_and_unknown_keys() {
        let (dir, ctx) = app("keep");
        let text = std::fs::read_to_string(ctx.app.join("config.json")).expect("读配置");
        let mut v: Json = serde_json::from_str(&text).expect("合法 JSON");
        let o = v.as_object_mut().expect("对象");
        o.insert("future_field".into(), json!({ "x": 1 }));
        if let Some(t) = o.get_mut("trail").and_then(|t| t.as_object_mut()) {
            t.insert("cursor_color".into(), json!("#123456"));
        }
        std::fs::write(
            ctx.app.join("config.json"),
            serde_json::to_string_pretty(&v).expect("写回"),
        )
        .expect("写文件");

        set_active(&ctx, "HoverDot/foo", true, None, None);
        let after_text = std::fs::read_to_string(ctx.app.join("config.json")).expect("读回配置");
        let after: Json = serde_json::from_str(&after_text).expect("合法 JSON");
        assert_eq!(after["future_field"], json!({ "x": 1 }), "未知键不能丢");
        assert_eq!(after["trail"]["cursor_color"], json!("#123456"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 写脚本：不覆盖既有文件、拒绝非法挂载点、行尾统一 LF。
    #[test]
    fn write_script_guards_and_normalises() {
        let (dir, ctx) = app("write");
        let (code, v) = write_script(&ctx, "HoverDot/a.lua", "x = 1\r\ny = 2\r\n", false);
        assert_eq!(code, 0, "首次写入应成功：{v}");
        let body =
            std::fs::read_to_string(ctx.app.join("assets/lua/HoverDot/a.lua")).expect("读回");
        assert_eq!(
            body, "x = 1\ny = 2\n",
            "CRLF 必须被换成 LF（否则报错行号错位）"
        );
        assert_eq!(v["id"], "HoverDot/a");
        assert_eq!(v["hot_reload"], false);

        // 同名再写：拒绝（这是"别覆盖出厂脚本"的那道闸）
        let (code, v) = write_script(&ctx, "HoverDot/a.lua", "x = 9\n", false);
        assert_eq!(code, 3, "同名必须拒绝：{v}");
        assert_eq!(v["exists"], true);
        // 明确要求覆盖才写
        let (code, _) = write_script(&ctx, "HoverDot/a.lua", "x = 9\n", true);
        assert_eq!(code, 0);
        let body =
            std::fs::read_to_string(ctx.app.join("assets/lua/HoverDot/a.lua")).expect("读回");
        assert_eq!(body, "x = 9\n");

        // 没有挂载目录 / 目录非法 / 越界
        for bad in ["foo.lua", "Sounds/foo.lua", "assets/lua/../config.json"] {
            let (code, v) = write_script(&ctx, bad, "x = 1\n", true);
            assert_ne!(code, 0, "`{bad}` 不该被写入：{v}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 删除 = 移进回收站：原位置没了，内容还在（可找回）。
    #[test]
    fn delete_moves_to_trash_instead_of_unlinking() {
        let (dir, ctx) = app("del");
        write_script(&ctx, "scripts/tmp_probe", "return 1\n", false);
        let (code, v) = delete_script(&ctx, "scripts/tmp_probe");
        assert_eq!(code, 0, "应成功：{v}");
        assert!(!ctx.app.join("assets/lua/scripts/tmp_probe.lua").exists());

        let trash = ctx.app.join("temp").join("lua_tool_trash");
        let kept: Vec<std::path::PathBuf> = std::fs::read_dir(&trash)
            .expect("回收目录应在")
            .flatten()
            .map(|e| e.path())
            .collect();
        assert_eq!(kept.len(), 1, "回收站应正好一件：{kept:?}");
        let body = std::fs::read_to_string(&kept[0]).expect("回收件应可读");
        assert_eq!(body, "return 1\n", "内容必须原样保留");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 配置不存在时明确拒绝，而不是凭空造一份配置（造出来的"出厂值"会和程序不一致）。
    #[test]
    fn missing_config_is_refused_with_a_reason() {
        let p = std::env::temp_dir().join(format!("curseen_write_nocfg_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(p.join("assets/lua/HoverDot")).expect("建目录");
        let ctx = Ctx::from_app_dir(p.clone());
        let (code, v) = set_active(&ctx, "HoverDot/foo", true, None, None);
        assert_eq!(code, 3, "缺配置要拒绝：{v}");
        assert!(v["hint"].as_str().unwrap_or("").contains("启动"));
        let _ = std::fs::remove_dir_all(&p);
    }
}
