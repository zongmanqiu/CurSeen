//! AI 对话提示词的两道资料门禁。
//!
//! AI 对话窗的能力来自随包的 API 速查（`assets/lua/scripts/ai_chat_api.txt`）——它不是
//! 装饰：模型只知道这份文本里写过的事。所以要钉住两件事：
//!   1. **覆盖**：权威源 `assets/lua/API.md` 回调表里列的每一项，速查里都得提到。
//!      历史欠账：复合回调 `on_mouse_down` / `on_key` 漏写，用户问「怎么写响应鼠标 /
//!      键盘的脚本」时 AI 会答不知道（2026-09-29 补齐）。
//!   2. **随包**：发行目录 `build/assets/lua/scripts/` 里必须是同一份文本（用户口径
//!      「编译后的发行文件要放在 build」）。`build/` 不在版本库里（见 `.gitignore`），
//!      不存在时**跳过**而不是误报红。

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
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

#[test]
fn ai_reference_covers_every_api_md_callback() {
    let md = read(&repo_root().join("assets/lua/API.md"));
    let txt = read(&repo_root().join("assets/lua/scripts/ai_chat_api.txt"));

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
         ——请补进 assets/lua/scripts/ai_chat_api.txt",
        names.len()
    );
}

#[test]
fn ai_reference_ships_into_build_directory() {
    for name in ["ai_chat_api.txt", "ai_chat.lua"] {
        let src = repo_root().join("assets/lua/scripts").join(name);
        let dst = repo_root()
            .parent()
            .unwrap()
            .join("build/assets/lua/scripts")
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
    let txt = read(&repo_root().join("assets/lua/scripts/ai_chat_api.txt"));

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
