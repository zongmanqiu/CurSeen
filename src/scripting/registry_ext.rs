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
    /// 贴图右键「图片操作」的子菜单文本；`None` 表示只出现在脚本页。
    pub menu: Option<String>,
    /// 参数声明（`schema()`，顺序即 UI 顺序）。
    pub params: Vec<ParamDef>,
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
                    let name: String = g.get("name").unwrap_or_else(|_| stem.clone());
                    let description: String = g.get("description").unwrap_or_default();
                    let menu: Option<String> = g.get("menu").ok().flatten();
                    let role: String = g.get("role").unwrap_or_default();
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
                    ExtSchema {
                        id: id.clone(),
                        name,
                        description,
                        role,
                        menu,
                        params,
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

    /// 声明了 `menu` 的脚本（贴图右键「图片操作」子菜单项），按 id 字典序。
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
