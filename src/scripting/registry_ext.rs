//! Lua 统一脚本注册表（`assets/lua/` 下所有 `.lua` 的 Manifest 解析）。
//!
//! 与老 `registry.rs`（拖尾/悬浮标特效）同机制，但服务**统一 Lua 框架**：
//! - **只扫 `assets/lua/`**（递归，支持子目录整理），scanner 把每个文件的相对路径当 id；
//! - 每个脚本声明：`name`（默认文件名）、`description`、`role`（单值：`image`/`effect`/
//!   `keysound`/`composite`）、`menu`（image 专用右键子项）、`schema()`（参数声明）。
//!
//! ★ `composite`（2026-09-26）：复合脚本，API 全开放（事件 + 绘制 + 声音），由
//!   `modules::trail::companion` 常驻宿主运行，运行列表存 `config.lua_ext.composite_active`。
//!
//! # role 兼容策略（2026-09-27 起：`scripts/` 下 role 退化为可选）
//!
//! `role` 影响开关语义与归宿页，但**不再决定"是否出现"**：
//! - 声明了 `menu` 的脚本若无 `role` 自动归 `image`（贴图动作语义本就靠 `menu` 表达）；
//! - **`scripts/` 目录下**无 `role` 的脚本缺省归 `composite`（脚本页可运行卡片）——
//!   用户把任何特效脚本直接复制进来都能开关运行；
//! - **三个特效目录**（`MouseTrail/` `HoverDot/` `ClickFx/`）下无 `role` 的脚本缺省归
//!   `effect`——目录即挂载点，特效脚本复制到任何目录都不失效；
//! - 只有落在上述目录之外、又无 `menu` 又无 `role` 的散件才**告警后跳过**。
//!
//! ★★ **归属由目录决定，不在脚本里声明**（2026-09-28 定稿）：`MouseTrail/` `HoverDot/`
//! `ClickFx/` 三个目录 = 三个挂载点，**同一份脚本复制到哪个目录，就在哪个下拉里生效**。
//! 别再往 Manifest 里加"声明挂载点"的字段——加了也不会有人读，只会误导脚本作者。
//!
//! # `requires`：外置文件缺了就不出现在菜单里（2026-10-28）
//!
//! 上面那条禁令针对的是"没人读的所有权声明"。`requires` 不一样，它有**两个**读者
//! （[`LuaExtRegistry::menu_items`] 的可见性过滤与 [`Runtime::lua_ext_menu_items`] 那条唯一的
//! 过滤判据）：脚本声明"我要用哪些外置文件"（相对脚本目录），缺任何一个就不进贴图右键的
//! 「脚本」。判据走 [`super::fs_api::resolve`] —— 与 `ctx.fs`、`ext:proc` 用的是**同一条
//! 路径判据**，
//! 免得出现第二套"相对脚本目录"的解析。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::engine::ScriptingEngine;
use super::registry::{parse_schema_table, ParamDef};

/// 一个统一框架脚本的 Manifest。
#[derive(Debug, Clone)]
pub struct ExtSchema {
    /// 脚本 id = 相对 `assets/lua/` 的**相对路径**（不带 `.lua`，如 `demo` 或
    /// `sub/demo`）。也是配置 `lua_ext.params[id]` 与拼接 `.lua` 相对路径的依据。
    pub id: String,
    /// 显示名（`name`，默认 = 文件名 stem）。
    pub name: String,
    /// 一句话说明（`description`，可选）。
    pub description: String,
    /// 角色（`image`/`effect`/`keysound`/`composite`）。脚本据此分支；见模块头兼容策略。
    pub role: String,
    /// 效果类别（`cursor`/`trail`/`click`/`hover`/...）；包卡下拉用它标类别。
    /// 普通脚本不声明时为空串（按通用处理）。
    pub kind: String,
    /// 脚本页卡片分组（`mouse`/`picture`/`text`/`ai`/…）。
    /// **缺省为空**：老脚本不分类，仍按原来的 id 顺序排在分类卡片之后。
    pub category: String,
    /// 贴图右键「脚本」的子菜单文本；`None` 表示只出现在脚本页。
    pub menu: Option<String>,
    /// 参数声明（`schema()`，顺序即 UI 顺序）。
    pub params: Vec<ParamDef>,
    /// **脚本自己所在的目录**（绝对）。扫描时本来就算出来了，过去算完就扔；
    /// 现在 [`ExtSchema::available`] 要用它解析 `requires`，与 `ctx.fs` / `ext:proc`
    /// 用的是同一个目录，不存在第二份来源。
    pub dir: PathBuf,
    /// 脚本声明依赖的**外置文件**（相对脚本目录，`requires` 全局表）。空 = 不依赖任何东西。
    pub requires: Vec<String>,
}

impl ExtSchema {
    /// 外置文件都在吗？缺任何一个 ⇒ 这一项**不进**贴图右键菜单（m13 `requires`）。
    ///
    /// 两条刻意的口径：
    /// · 路径解析走 [`super::fs_api::resolve`]——绝对路径 / `..` 一律算"不满足"而不是"顺着
    ///   路径去找"，与 `ctx.fs`、`ext:proc` 同一条判据，别在这里另写一套相对目录的算法；
    /// · **没声明 `requires` 的脚本永远算满足**（不声明 = 不依赖，不能因为字段新加就把
    ///   所有老脚本从菜单里抹掉）。
    pub fn available(&self) -> bool {
        self.requires.iter().all(|rel| {
            super::fs_api::resolve(&self.dir, rel)
                .map(|p| p.exists())
                .unwrap_or(false)
        })
    }
}

/// Lua 统一脚本注册表。
#[derive(Clone)]
pub struct LuaExtRegistry {
    scripts_dir: PathBuf,
    scripts: HashMap<String, ExtSchema>,
}

impl LuaExtRegistry {
    pub fn new(scripts_dir: &Path) -> Self {
        Self {
            scripts_dir: scripts_dir.to_path_buf(),
            scripts: HashMap::new(),
        }
    }

    /// 递归扫 `assets/lua/`，给每个 `.lua` 起一次性 VM 取 name / description / role /
    /// menu / schema。id = 相对路径（无后缀）。
    pub fn scan(&mut self) {
        self.scripts.clear();
        let root = self.scripts_dir.clone();
        let mut pending = vec![root.clone()];
        let mut files: Vec<PathBuf> = Vec::new();
        while let Some(dir) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    pending.push(p);
                } else if p.extension().and_then(|e| e.to_str()) == Some("lua") {
                    files.push(p);
                }
            }
        }
        files.sort();
        for path in files {
            // 相对路径（无后缀）当 id，支持子目录；扁平脚本退化成裸 stem，与老行为一致。
            let rel = path.strip_prefix(&root).unwrap_or(&path).with_extension("");
            let id = rel.to_string_lossy().replace('\\', "/");
            if id.is_empty() {
                continue;
            }
            let dir = path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| root.clone());
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let schema = match ScriptingEngine::new(&dir)
                .and_then(|e| e.load_script(&stem).map(|_| e))
            {
                Ok(engine) => {
                    let g = engine.lua().globals();
                    let name: String = g
                        .get::<Option<String>>("name")
                        .ok()
                        .flatten()
                        .or_else(|| g.get::<Option<String>>("effect_name").ok().flatten())
                        .unwrap_or_else(|| stem.clone());
                    let description: String = g.get("description").unwrap_or_default();
                    let menu: Option<String> = g.get("menu").ok().flatten();
                    let role: String = g.get("role").unwrap_or_default();
                    let kind: String = g.get("kind").unwrap_or_default();
                    let category: String = g.get("category").unwrap_or_default();
                    let role = if role.is_empty() {
                        if menu.is_some() {
                            // 老图片脚本：只有 menu 没 role → 贴图动作语义。
                            "image".to_string()
                        } else if id.starts_with("scripts/") {
                            // ★ `scripts/` 目录下的脚本 role **退化为可选**（2026-09-27）：
                            //   归属由目录决定，`scripts/` 就是"脚本页可运行卡片"，
                            //   缺省当 `composite`（常驻运行、API 全开放）。用户把任何
                            //   特效脚本直接复制进来都应该能开关运行，而不是被扫描器
                            //   静默丢弃——"看得见、跑不动"里更糟的"连看都看不见"。
                            "composite".to_string()
                        } else if id.starts_with("MouseTrail/")
                            || id.starts_with("HoverDot/")
                            || id.starts_with("ClickFx/")
                        {
                            // ★ 三个特效目录同理（2026-09-28）：目录即挂载点，缺 role
                            //   的散件缺省当 `effect`——「脚本复制到任何地方都不失效」
                            //   的最后一块。此前这里会告警后跳过，用户把特效脚本从
                            //   `scripts/` 拷回 `MouseTrail/` 就直接失效（真机取证抓到）。
                            "effect".to_string()
                        } else {
                            log::warn!(
                                    "扫描 Lua 脚本 {id}：缺少 role（image/effect/keysound/composite），跳过"
                                );
                            continue;
                        }
                    } else {
                        role
                    };
                    let params = match engine.call_schema() {
                        Ok(tbl) => parse_schema_table(&tbl),
                        Err(_) => Vec::new(),
                    };
                    // `requires`：外置文件清单（相对脚本目录的字符串数组）。读不到 / 不是表
                    // = 空 = 不依赖任何东西，**不能**把没声明的老脚本一律算不满足。
                    let requires: Vec<String> = g
                        .get::<Option<mlua::Table>>("requires")
                        .ok()
                        .flatten()
                        .map(|t| {
                            (1..=t.raw_len())
                                .filter_map(|i| t.get::<String>(i).ok())
                                .collect()
                        })
                        .unwrap_or_default();
                    ExtSchema {
                        id: id.clone(),
                        name,
                        description,
                        role,
                        kind,
                        category,
                        menu,
                        params,
                        dir: dir.clone(),
                        requires,
                    }
                }
                Err(e) => {
                    log::warn!("扫描 Lua 脚本 {id} 失败: {e}");
                    continue;
                }
            };
            self.scripts.insert(id, schema);
        }
    }

    /// 按 id（相对路径）取 Manifest。
    pub fn get(&self, id: &str) -> Option<&ExtSchema> {
        self.scripts.get(id)
    }

    /// 全部脚本，按 id 字典序。
    pub fn list(&self) -> Vec<&ExtSchema> {
        let mut v: Vec<&ExtSchema> = self.scripts.values().collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    /// 指定 role 的脚本（按 id 字典序）。
    pub fn role_of(&self, role: &str) -> Vec<&ExtSchema> {
        let mut v: Vec<&ExtSchema> = self.scripts.values().filter(|s| s.role == role).collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    /// 声明了 `menu` 的脚本（贴图右键「脚本」子菜单项），按 id 字典序。
    pub fn menu_items(&self) -> Vec<&ExtSchema> {
        let mut v: Vec<&ExtSchema> = self.scripts.values().filter(|s| s.menu.is_some()).collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    pub fn scripts_dir(&self) -> &Path {
        &self.scripts_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::registry::ParamType;

    /// 在临时目录写一组脚本，返回 registry。目录名带内容指纹避免并行互相覆盖。
    fn registry_with_files(files: &[(&str, &str)]) -> LuaExtRegistry {
        let dir = std::env::temp_dir().join(format!(
            "curseen_luaext_test_{}_{:x}",
            std::process::id(),
            files
                .iter()
                .map(|(n, c)| format!("{n}:{}", c.len()))
                .collect::<Vec<_>>()
                .concat()
                .len()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        for (rel, body) in files {
            let p = dir.join(rel);
            if let Some(parent) = p.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(&p, body).unwrap();
        }
        let mut reg = LuaExtRegistry::new(&dir);
        reg.scan();
        reg
    }

    /// ★ `requires`（m13）：声明了外置文件的脚本，**文件不在就不该出现在菜单里**。
    ///
    /// 三格都有独立判据，分开断言：
    /// · 没声明 `requires` 的脚本永远算满足——不能因为宿主新加了一个字段就把所有老脚本
    ///   从菜单里抹掉（那才是这次改动最可怕的失效方式）；
    /// · 缺文件 ⇒ 不满足；把文件放回去 ⇒ 重新扫一次立刻满足（宿主每次右键都重扫，
    ///   用户放完文件不该要求重启）；
    /// · 扫描阶段**不丢弃**：`menu_items()` 照样列它，可见性是消费者的判据
    ///   （脚本页那列复选框要用 `list()`，东西消失用户就再也找不到）。
    #[test]
    fn requires_gates_visibility_without_dropping_the_entry() {
        let body = "name = \"取数\"\nrole = \"image\"\nmenu = \"跑一下\"\n\
                     requires = { \"./tools/a.exe\", \"./tools/model.bin\" }\n";
        let reg = registry_with_files(&[
            ("needs.lua", body),
            (
                "plain.lua",
                "name = \"通用\"\nrole = \"image\"\nmenu = \"老脚本\"\n",
            ),
        ]);
        // 没声明的算满足（回归格）。
        let plain = reg.get("plain").expect("plain 该扫到");
        assert!(plain.available(), "没声明 requires 的脚本必须永远算满足");
        assert_eq!(plain.requires.len(), 0);
        // 声明了但文件不在。
        let needs = reg.get("needs").expect("needs 该扫到");
        assert_eq!(
            needs.requires.len(),
            2,
            "requires 没读出来：{:?}",
            needs.requires
        );
        assert!(!needs.available(), "两个外置文件都不存在，不该算满足");
        assert!(
            reg.menu_items().iter().any(|s| s.id == "needs"),
            "扫描阶段不许丢弃它（脚本页要能看见这个开关）"
        );
        // 只补一个文件：还差一个 ⇒ 仍不满足（判据是"全部都在"，不是"有一个就行"）。
        std::fs::create_dir_all(needs.dir.join("tools")).unwrap();
        std::fs::write(needs.dir.join("tools").join("a.exe"), b"x").unwrap();
        let mut reg2 = LuaExtRegistry::new(reg.scripts_dir());
        reg2.scan();
        assert!(!reg2.get("needs").unwrap().available(), "缺一半不算满足");
        // 补齐 ⇒ 重新扫一次立刻满足（不重启）。
        std::fs::write(needs.dir.join("tools").join("model.bin"), b"y").unwrap();
        let mut reg3 = LuaExtRegistry::new(reg.scripts_dir());
        reg3.scan();
        assert!(
            reg3.get("needs").unwrap().available(),
            "文件放齐后下一次扫描就该通过"
        );
    }

    /// `requires` 里写绝对路径或 `..` ⇒ **算不满足**，而不是"顺着路径去找"。
    /// 判据与 `ctx.fs`、`ext:proc` 共用 `fs_api::resolve`：一条规则，不许有第二套。
    #[test]
    fn requires_outside_the_script_dir_is_never_satisfied() {
        let win = std::env::var("WINDIR").unwrap_or_default();
        for rel in [
            &format!("{win}\\System32\\cmd.exe"),
            r#"C:\Windows\System32\cmd.exe"#,
            "../outside.exe",
        ] {
            // ★ 反斜杠要按 **Lua 的写法**转义：`\W` `\S` 这些不是合法 Lua 转义序列，
            //   原样塞进脚本会让整份文件加载失败——那样测到的是"脚本没扫到"，不是判据本身。
            let lua_str = rel.replace('\\', "\\\\");
            let body = format!("role = \"image\"\nmenu = \"M\"\nrequires = {{ \"{lua_str}\" }}\n");
            let reg = registry_with_files(&[("esc.lua", &body)]);
            let s = reg
                .get("esc")
                .unwrap_or_else(|| panic!("{rel:?} 那份脚本该被扫到"));
            assert!(!s.available(), "{rel:?} 越出脚本目录，不该被当成满足");
        }
    }

    #[test]
    fn parses_name_desc_menu_role_emit_and_schema() {
        let reg = registry_with_files(&[(
            "demo.lua",
            "name = \"演示\"\ndescription = \"一条测试脚本\"\nrole = \"effect\"\nmenu = \"在线OCR\"\n\
             function schema()\n  return {\n    { key=\"timeout\", label=\"超时\", type=\"int\", min=5, max=120, default=60 }\n  }\nend\n",
        )]);
        let s = reg.get("demo").expect("应扫到 demo");
        assert_eq!(s.name, "演示");
        assert_eq!(s.description, "一条测试脚本");
        assert_eq!(s.role, "effect");
        assert_eq!(s.menu.as_deref(), Some("在线OCR"));
        assert_eq!(s.params.len(), 1);
        assert_eq!(s.params[0].ptype, ParamType::Int);
        assert_eq!(s.params[0].key, "timeout");
        assert_eq!(reg.menu_items().len(), 1);
    }

    #[test]
    fn recursive_scan_and_relative_ids() {
        let reg = registry_with_files(&[
            ("a.lua", "role = \"effect\"\n"),
            ("sub/b.lua", "role = \"image\"\nmenu = \"B动作\"\n"),
            ("sub/deep/c.lua", "role = \"keysound\"\n"),
        ]);
        assert!(reg.get("a").is_some(), "扁平脚本 id = 裸 stem");
        assert!(reg.get("sub/b").is_some(), "子目录脚本 id = 相对路径");
        assert!(
            reg.get("sub/deep/c").is_some(),
            "深层子目录脚本 id = 相对路径"
        );
        assert_eq!(reg.list().len(), 3);
        assert_eq!(reg.role_of("image").len(), 1);
        assert_eq!(reg.role_of("keysound").len(), 1);
        assert_eq!(reg.menu_items().len(), 1);
    }

    #[test]
    fn missing_role_with_menu_defaults_to_image() {
        // 老图片脚本：只有 menu 没 role → 归 image，仍出现在右键子菜单。
        let reg = registry_with_files(&[("old.lua", "name = \"旧\"\nmenu = \"老动作\"\n")]);
        let s = reg.get("old").expect("声明 menu 的老脚本应被保留");
        assert_eq!(s.role, "image");
        assert_eq!(reg.menu_items().len(), 1);
    }

    #[test]
    fn missing_role_without_menu_is_skipped() {
        // 无 menu 无 role 且不在任何挂载目录下 → 跳过（无归宿可推断）。
        let reg = registry_with_files(&[("orphan.lua", "name = \"孤儿\"\n")]);
        assert!(reg.get("orphan").is_none());
        assert!(reg.list().is_empty());
    }

    #[test]
    fn missing_role_in_effect_dirs_defaults_to_effect() {
        // 三个特效目录下缺 role 的散件缺省当 effect ——「特效脚本复制到任何
        // 目录都不失效」的最后一格（此前在这里被告警跳过，真机取证抓到）。
        let reg = registry_with_files(&[
            ("MouseTrail/p.lua", "effect_name = \"甲\"\n"),
            ("HoverDot/q.lua", "effect_name = \"乙\"\n"),
            ("ClickFx/r.lua", "effect_name = \"丙\"\n"),
        ]);
        for id in ["MouseTrail/p", "HoverDot/q", "ClickFx/r"] {
            let s = reg.get(id).unwrap_or_else(|| panic!("{id} 应被收录"));
            assert_eq!(s.role, "effect", "{id} 缺 role 时应缺省 effect");
        }
    }

    #[test]
    fn scripts_dir_defaults_missing_role_to_composite() {
        // ★ `scripts/` 下 role 退化为可选（2026-09-27）：用户把任何脚本（含特效脚本
        //   副本）复制进来都必须能出现在脚本页并可开关运行，而不是被扫描器静默丢弃。
        let reg = registry_with_files(&[(
            "scripts/copied.lua",
            "name = \"复制的特效\"\nfunction draw() end\n",
        )]);
        let s = reg
            .get("scripts/copied")
            .expect("scripts/ 下无 role 应缺省 composite");
        assert_eq!(s.role, "composite");
        assert_eq!(reg.role_of("composite").len(), 1);
    }

    /// ★ 挂载点 = 目录，不是脚本里的声明（2026-09-28 用户口径："鼠标类脚本互相随便放
    ///   文件夹都有效"）。同一份脚本复制到四个目录 → 四个独立条目，各自进各自的下拉。
    ///   脚本里**不存在**任何声明归属的字段——别再加，加了也没人读。
    #[test]
    fn mounting_point_is_the_directory_not_a_declaration() {
        let reg = registry_with_files(&[
            ("MouseTrail/glow.lua", "role = \"effect\"\n"),
            ("HoverDot/glow.lua", "role = \"effect\"\n"),
            ("ClickFx/glow.lua", "role = \"effect\"\n"),
            ("scripts/glow.lua", "role = \"effect\"\n"),
        ]);
        assert_eq!(reg.list().len(), 4, "四份副本各是一个独立条目");
        for d in ["MouseTrail", "HoverDot", "ClickFx", "scripts"] {
            let id = format!("{d}/glow");
            let s = reg.get(&id).unwrap_or_else(|| panic!("{id} 应被扫到"));
            assert_eq!(s.role, "effect");
        }
    }
}
