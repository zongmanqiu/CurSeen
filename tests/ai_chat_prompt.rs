//! AI 对话提示词的资料门禁。
//!
//! AI 对话窗的能力来自官方包里的 API 速查（`CurSeen-Scripts/Packs/AI/scripts/ai_chat_api.txt`）——它不是
//! 装饰：模型只知道这份文本里写过的事。所以要钉住四件事：
//!   1. **回调覆盖**：权威源 `assets/lua/API.md` 回调表里列的每一项，速查里都得提到。
//!   2. **ext / ExtImage 覆盖**：API.md 里 `成员` 表（ext 方法与 ExtImage）的每一项都要出现
//!      ——2026-10-31 实测：速查停在 0.1.0，缺 18 个 ext 方法（文件/进程/剪贴板/对话框/包），
//!      模型会一本正经地声称“没有这些能力”。
//!   3. **schema 参数类型覆盖**：`registry.rs` 注册的每个 `ParamType` 关键字都要出现
//!      （旧速查还写着“没有 image 类型”，与实现相反）。
//!   4. **挂载目录覆盖**：API.md 里以 `assets/lua/` 开头的目录都要出现（漏了 `KeySound/`
//!      模型就不知道键音脚本该放哪）。
//!
//! 另有两条交付物门禁：随包文本必须与 `build/` 一致；虚拟键盘 JSON 示例可解析且引用不悬空。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

/// 官方 AI 包根（同级 CurSeen-Scripts 仓库）。
fn ai_pack_dir() -> PathBuf {
    repo_root().join("../CurSeen-Scripts/Packs/AI")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("{} 读不到（{e}）——门禁依赖它", path.display()))
}

/// 抽 `API.md` 里「回调」表的第一列（反引号包着的标识符）＝ 全部回调名。
///
/// 只认那一张表（表头含 `| 回调 |`），扫到第一个非表格行为止——别的表格
/// （环境变量、schema 字段、ctx 方法）第一列也是标识符，混进来会误报。
fn api_md_callback_names(md: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_table = false;
    for line in md.lines() {
        let t = line.trim();
        if t.starts_with("| 回调 |") {
            in_table = true;
            continue;
        }
        if !in_table {
            continue;
        }
        if !t.starts_with('|') {
            in_table = false; // 空行 / 非表格行 = 这段表结束
            continue;
        }
        let Some(rest) = t.strip_prefix('|') else {
            continue;
        };
        let first_col = rest.split('|').next().unwrap_or("").trim();
        let Some(name) = first_col
            .strip_prefix('`')
            .and_then(|x| x.strip_suffix('`'))
        else {
            continue;
        };
        let ok_name = !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !name.starts_with(|c: char| c.is_ascii_digit());
        if ok_name {
            names.push(name.to_string());
        }
    }
    names
}

/// 取一个单元格里所有反引号包着的标识符（`` `w` / `h` `` → w、h）。
fn backtick_tokens(cell: &str) -> Vec<String> {
    cell.split('`')
        .skip(1)
        .step_by(2)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// 收集 API.md 里所有表头首格等于 `header_first` 的表格的第一列标识符。
fn api_md_table_names(md: &str, header_first: &str) -> Vec<String> {
    let header = format!("| {header_first} |");
    let mut out = Vec::new();
    let mut in_table = false;
    for line in md.lines() {
        let t = line.trim();
        if t.starts_with(&header) {
            in_table = true;
            continue;
        }
        if !in_table {
            continue;
        }
        if !t.starts_with('|') {
            in_table = false;
            continue;
        }
        let first = t
            .trim_start_matches('|')
            .split('|')
            .next()
            .unwrap_or("")
            .trim();
        out.extend(backtick_tokens(first));
    }
    out
}

/// API.md 里以 `assets/lua/` 开头的挂载目录（§10 角色表第一列）。
fn api_md_lua_directories(md: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in md.lines() {
        let t = line.trim();
        if !t.starts_with("| `assets/lua/") {
            continue;
        }
        let first = t
            .trim_start_matches('|')
            .split('|')
            .next()
            .unwrap_or("")
            .trim();
        if let Some(dir) = backtick_tokens(first)
            .into_iter()
            .find(|s| s.starts_with("assets/lua/"))
        {
            out.push(dir);
        }
    }
    out
}

/// `registry.rs` 里 `"image" => ParamType::Image` 这类映射注册的每个类型名。
fn registry_param_type_names(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in src.lines() {
        let Some(at) = line.find("=> ParamType::") else {
            continue;
        };
        let left = &line[..at];
        let Some(start) = left.find('"') else {
            continue;
        };
        let Some(end) = left[start + 1..].find('"') else {
            continue;
        };
        out.push(left[start + 1..start + 1 + end].to_string());
    }
    out
}

#[test]
fn ai_reference_covers_every_api_md_callback() {
    let md = read(&repo_root().join("assets/lua/API.md"));
    let txt = read(&ai_pack_dir().join("scripts/ai_chat_api.txt"));

    let names = api_md_callback_names(&md);
    assert!(
        !names.is_empty(),
        "API.md 的回调表一个名字都没抽到——抽取规则或表结构变了，先修门禁再谈通过"
    );
    // 复合脚本专属回调是历史上真正漏过的，单独钉一下（漏了这条是沉默故障：AI 会
    // 一本正经地说"不支持鼠标按下回调"，而不是报错）。
    for must in ["on_mouse_down", "on_key"] {
        assert!(
            txt.contains(must),
            "AI 速查没有 {must}——{must} 是复合脚本的专属回调，漏了模型就当它不存在"
        );
    }
    let missing: Vec<String> = names
        .iter()
        .filter(|n| !txt.contains(n.as_str()))
        .cloned()
        .collect();
    assert!(
        missing.is_empty(),
        "AI 速查漏了 API.md 里的回调 {missing:?}（共 {} 项）\
         ——请补进 CurSeen-Scripts/Packs/AI/scripts/ai_chat_api.txt",
        names.len()
    );
}

/// ext 方法与 ExtImage 成员：API.md 的 `成员` 表逐项覆盖。
#[test]
fn ai_reference_covers_every_ext_method_and_extimage_member() {
    let md = read(&repo_root().join("assets/lua/API.md"));
    let txt = read(&ai_pack_dir().join("scripts/ai_chat_api.txt"));

    let names = api_md_table_names(&md, "成员");
    assert!(
        names.len() > 25,
        "API.md 的「成员」表只抽到 {} 项——抽取规则或表结构变了",
        names.len()
    );
    // 这些是最容易被漏掉、且一漏模型就会说“没有”的能力。
    for must in [
        "set_image",
        "data_dir",
        "clip_set",
        "chat_window",
        "unzip",
        "proc",
        "crop",
        "rotate_cw90",
    ] {
        assert!(txt.contains(must), "AI 速查没有 {must}——模型会当成不存在");
    }
    let missing: Vec<String> = names
        .iter()
        .filter(|n| !txt.contains(n.as_str()))
        .cloned()
        .collect();
    assert!(
        missing.is_empty(),
        "AI 速查漏了 API.md 的 ext / ExtImage 成员 {missing:?}（共 {} 项）",
        names.len()
    );
}

/// schema 的参数类型：`registry.rs` 注册了哪些，速查就得写哪些。
#[test]
fn ai_reference_covers_every_param_type() {
    let reg = read(&repo_root().join("src/scripting/registry.rs"));
    let txt = read(&ai_pack_dir().join("scripts/ai_chat_api.txt"));

    let names = registry_param_type_names(&reg);
    assert!(
        names.len() >= 10,
        "registry.rs 只抽到 {} 个 ParamType——抽取规则或实现变了",
        names.len()
    );
    let missing: Vec<String> = names
        .iter()
        .filter(|n| !txt.contains(n.as_str()))
        .cloned()
        .collect();
    assert!(
        missing.is_empty(),
        "AI 速查漏了参数类型 {missing:?}——模型会写出生效不了的 schema"
    );
    // 旧速查曾反向宣称“没有 image 类型”，实现早就支持（路径框占位）。
    assert!(
        txt.contains("image"),
        "速查必须提到 image 参数类型（仅路径框占位）"
    );
}

/// 挂载目录：API.md §10 里点名的每个 `assets/lua/...` 都要在速查里出现。
#[test]
fn ai_reference_covers_every_mount_directory() {
    let md = read(&repo_root().join("assets/lua/API.md"));
    let txt = read(&ai_pack_dir().join("scripts/ai_chat_api.txt"));

    let dirs = api_md_lua_directories(&md);
    assert!(
        dirs.len() >= 5,
        "API.md 只抽到 {} 个挂载目录——抽取规则或表结构变了：{dirs:?}",
        dirs.len()
    );
    for dir in &dirs {
        assert!(
            txt.contains(dir.as_str()),
            "AI 速查没有挂载目录 {dir}——模型会让用户把脚本放错地方"
        );
    }
}

#[test]
fn ai_reference_ships_into_build_directory() {
    for name in ["ai_chat_api.txt", "ai_chat.lua"] {
        let src = ai_pack_dir().join("scripts").join(name);
        let dst = repo_root()
            .parent()
            .unwrap()
            .join("build/assets/lua/scripts/AI")
            .join(name);
        if !dst.exists() {
            println!(
                "[skip] {} 还没部署进 build/（尚未跑 deploy？）",
                dst.display()
            );
            continue;
        }
        assert_eq!(
            read(&src),
            read(&dst),
            "build/ 里的 {name} 与源码不一致——重跑 deploy"
        );
    }
}

/// 速查里的虚拟键盘 JSON 示例必须是**合法 JSON**，且 `base_layout` 指向真实存在的
/// 内置方案、`keys` 引用的样式都在 `styles` 表里——示例是给 AI 抄的模板，引用悬空
/// 就是在教 AI 写加载不出来的方案（2026-09-30 用户要求补虚拟键盘参考时一并钉住）。
#[test]
fn ai_reference_vk_example_is_valid_json_with_resolvable_references() {
    let txt = read(&ai_pack_dir().join("scripts/ai_chat_api.txt"));

    // 抽 ```json 围栏块。
    let mut blocks: Vec<String> = Vec::new();
    let mut cur: Option<Vec<&str>> = None;
    for line in txt.lines() {
        let t = line.trim();
        if t.starts_with("```json") {
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
        1,
        "速查应恰好一段虚拟键盘 JSON 示例，实得 {}",
        blocks.len()
    );

    let v: serde_json::Value = serde_json::from_str(&blocks[0])
        .unwrap_or_else(|e| panic!("虚拟键盘示例不是合法 JSON：{e}"));
    assert!(v.get("name").is_some(), "示例缺 name（面板下拉显示名）");

    // base_layout 必须指向真实内置方案（assets/VirtualKeyboards/<id>.json）。
    let base = v
        .get("base_layout")
        .and_then(|b| b.as_str())
        .expect("示例应有 base_layout（纯样式方案靠它引用出厂几何）");
    assert!(
        repo_root()
            .join("assets/VirtualKeyboards")
            .join(format!("{base}.json"))
            .exists(),
        "base_layout 指向的 {base}.json 不存在——AI 会照抄出加载失败的方案"
    );

    // keys 里引用的样式必须在 styles 表里有定义（引用悬空 = 静默回落兜底样式）。
    let styles = v
        .get("styles")
        .and_then(|s| s.as_object())
        .expect("示例应有 styles 表");
    if let Some(keys) = v.get("keys").and_then(|k| k.as_object()) {
        for (key, ov) in keys {
            if let Some(style) = ov.get("style").and_then(|s| s.as_str()) {
                assert!(
                    styles.contains_key(style),
                    "keys.{key} 引用的样式 `{style}` 不在 styles 表里"
                );
            }
        }
    }

    // controls 的 bind 必须带路径（空 bind 的控件会被宿主跳过）。
    if let Some(controls) = v.get("controls").and_then(|c| c.as_array()) {
        for c in controls {
            let bind = c.get("bind").and_then(|b| b.as_str()).unwrap_or("");
            assert!(!bind.is_empty(), "controls 里出现空 bind：{c}");
        }
    }
}

/// 抽标题之后的第一段围栏代码（用于执行速查里的示例）。
fn fenced_block_after<'a>(txt: &'a str, heading: &str, fence: &str) -> &'a str {
    let start = txt
        .find(heading)
        .unwrap_or_else(|| panic!("找不到 {heading}"));
    let rest = &txt[start..];
    let at = rest
        .find(fence)
        .unwrap_or_else(|| panic!("{heading} 后没有 {fence:?} 代码块"))
        + fence.len();
    let rest = &rest[at..];
    let end = rest
        .find("```")
        .unwrap_or_else(|| panic!("{heading} 的代码块未闭合"));
    &rest[..end]
}

fn draw_once(
    eng: &curseen_ui::scripting::engine::ScriptingEngine,
    params: &HashMap<String, serde_json::Value>,
) -> (i64, Vec<curseen_ui::core::draw::DrawCmd>) {
    let mut list = curseen_ui::core::draw::DrawList::new();
    curseen_ui::scripting::api_draw::set_draw_target(&mut list);
    let v = eng.call_draw(params).expect("draw 失败");
    curseen_ui::scripting::api_draw::clear_draw_target();
    let frames = match v {
        mlua::Value::Integer(i) => i,
        mlua::Value::Number(f) => f as i64,
        _ => 0,
    };
    (frames, list.ordered().cloned().collect())
}

/// ★ 2026-10-31 用户实测：AI 给的“悬停计时”脚本完全没效果（os.time 不存在、
/// 在 step 里用 ctx、init 读 params、draw 不续帧）。速查新增的 HoverDot 示例
/// 必须真的能被生产引擎跑起来——否则提示词又在教模型写废脚本。
#[test]
fn ai_reference_hover_example_actually_runs() {
    let txt = read(&ai_pack_dir().join("scripts/ai_chat_api.txt"));
    let code = fenced_block_after(&txt, "## 完整示例 3：悬浮标", "```lua\n");

    let dir = std::env::temp_dir().join(format!("curseen_ai_hover_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建临时目录");
    std::fs::write(dir.join("hover.lua"), code).expect("写示例脚本");

    let eng = curseen_ui::scripting::engine::ScriptingEngine::new(&dir).expect("引擎初始化");
    eng.load_script("hover").expect("示例脚本加载失败");

    let params: HashMap<String, serde_json::Value> = [
        ("delay", serde_json::json!(1.0)),
        ("radius", serde_json::json!(18)),
        ("color", serde_json::json!("#4FC3F7CC")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();

    let t0 = 10_000.0;
    eng.set_wall_ms(t0).expect("wall_ms");
    eng.update_env(
        (100.0, 100.0),
        (100.0, 100.0),
        0.0,
        (1920.0, 1080.0),
        "hover",
    )
    .expect("env");
    eng.call_init().expect("init 失败（示例里读了 params？）");
    eng.call_on_move(100.0, 100.0, 0.0, &params)
        .expect("on_move");

    // 未到阈值：不画，但必须请求续帧。
    eng.set_wall_ms(t0 + 500.0).expect("wall_ms");
    eng.call_step(0.016, 0.5, &params).expect("step");
    let (frames, cmds) = draw_once(&eng, &params);
    assert!(frames > 0, "示例必须续帧，否则静止后永远测不到时间");
    assert!(cmds.is_empty(), "未到阈值不该画");

    // 超过阈值：画出圆，并继续续帧。
    eng.set_wall_ms(t0 + 1500.0).expect("wall_ms");
    eng.call_step(0.016, 1.5, &params).expect("step");
    let (frames, cmds) = draw_once(&eng, &params);
    assert!(frames > 0, "超过阈值后仍要续帧");
    assert!(!cmds.is_empty(), "超过阈值必须画出效果");
}
