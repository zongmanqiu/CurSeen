//! 宿主级引擎缓存：**每个已识别脚本**一份长期 Lua VM，兼做热更新与重扫 GC。
//!
//! # 为什么要有它
//!
//! 旧实现 `ext_runner::run_ext_sync` 每次跑脚本都 `ScriptingEngine::new` 全新建 VM——
//! 贴图动作偶发没问题，但 effect 要**逐帧**调脚本，image/keysound 也不想重复编译。统一后
//! 所有宿主共用同一个缓存：以脚本相对路径为 key，长期持有 VM。
//!
//! # 热更新
//!
//! 脚本文件 mtime 变了 → 该 key 的 VM 标记为需重建（下次调用边界换新 VM，不打断正在逐帧
//! 执行的那一帧）。重建用 `ScriptingEngine::new`（目录=脚本所在目录）+ `load_script`（文件名
//! stem）——与旧宿主的建法一致，`fs` 也恰好以脚本所在目录为根。
//!
//! # 重扫 GC
//!
//! 注册表重扫后调 [`EngineCache::gc`] 传入当前仍然存在的脚本相对路径集合，把不在集合里的
//! key 全删掉（删/移脚本后不残留僵尸 VM）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use mlua::Value as MValue;
use serde_json::Value as Json;

use super::engine::ScriptingEngine;

/// 单个脚本的缓存条目：长期 VM + 上次读取的 (mtime, 字节数)。
///
/// 同时对比字节数：某些文件系统时间戳粒度粗（同一次子节拍内两次写 mtime 不变），
/// 若只看 mtime 会漏掉**同 tick 内改了内容**的脚本 → 热更新失效（曾并行跑测试撞上）。
struct Entry {
    engine: ScriptingEngine,
    mtime: Option<SystemTime>,
    len: u64,
}

/// 脚本相对路径 → VM 的缓存。
///
/// ★ 生命周期约定（2026-09-30 审计）：缓存与宿主**同寿命**——生产中唯一消费者
/// （击键音效）换脚本时整体替换整个缓存，无需增量回收。将来若做多宿主**共享**
/// 缓存，必须自带"注册表重扫后按存活集合 `retain`"的 GC，否则删脚本会留僵尸 VM。
pub struct EngineCache {
    entries: HashMap<PathBuf, Entry>,
}

impl Default for EngineCache {
    fn default() -> Self {
        Self::new()
    }
}

impl EngineCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// 取某个脚本的 VM；mtime 变了或缺失则重建。返回对 VM 引擎的借用。
    ///
    /// `script_path` 是脚本的**绝对路径**（含 `.lua`）；引擎以它所在目录为根加载、
    /// `fs` 也以该目录为根。返回 `Err` = 创建/加载失败（懒加载的引擎不 cache 失败态，
    /// 下次调用再试）。
    fn vm(&mut self, script_path: &Path) -> Result<&ScriptingEngine, String> {
        // 复用条件：key 命中 且 (mtime, 字节数) 都未变。
        let (mtime, len) = match std::fs::metadata(script_path) {
            Ok(m) => (m.modified().ok(), m.len()),
            Err(_) => (None, 0),
        };
        let needs_rebuild = match self.entries.get(script_path) {
            None => true,
            Some(e) if e.mtime != mtime || e.len != len => true,
            Some(_) => false,
        };
        if needs_rebuild {
            let dir = script_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));
            let stem = script_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let engine = ScriptingEngine::new(&dir).map_err(|e| format!("Lua VM 创建失败：{e}"))?;
            engine
                .load_script(&stem)
                .map_err(|e| format!("脚本加载失败：{e}"))?;
            self.entries
                .insert(script_path.to_path_buf(), Entry { engine, mtime, len });
        }
        match self.entries.get(script_path) {
            Some(entry) => Ok(&entry.engine),
            // 不可达：上方命中即返回、未命中刚 insert。防御性兜底，防后续重构
            // 静默破坏"已插入"的不变量（不用 expect，守住生产禁 panic 红线）。
            None => Err("脚本缓存状态异常：加载后条目缺失".to_string()),
        }
    }

    /// 以统一入口 `run(ctx, params)` 执行脚本，返回脚本返回的原始 Lua 值。
    ///
    /// `role` 写进 `ctx.role`；`host_fields` 是宿主按 role 塞进 `ctx` 的额外字段
    /// （如 `image`/`draw`/`sounds`/`key`…）。`fs` 自动以脚本所在目录为根挂进 `ctx`。
    ///
    /// 返回的错误已经带 `[脚本相对路径] run(): …` 的前缀（加载失败单独用
    /// [`EngineCache::load_error`] 表达，运行失败用 `Err`）。没有全局 `run` → 返回 `Nil`。
    pub fn run(
        &mut self,
        script_path: &Path,
        role: &str,
        host_fields: &[(&str, MValue)],
        params: &HashMap<String, Json>,
    ) -> Result<MValue, String> {
        let engine = self.vm(script_path)?;
        let rel = script_relative(script_path);
        engine
            .call_run_ctx(role, host_fields, params)
            .map_err(|e| format!("[{rel}] run(): {e}"))
    }

    /// 只建 VM 但**不执行**，若加载期就失败则返回错误（UI 用「加载失败」区分于「运行失败」）。
    pub fn load_error(&mut self, script_path: &Path) -> Option<String> {
        let rel = script_relative(script_path);
        match self.vm(script_path) {
            Ok(_) => None,
            Err(e) => Some(format!("[{rel}] load(): {e}")),
        }
    }

    /// 取某脚本 VM 的 Lua 引用（供宿主构造 `host_fields` 里的 table，如 `ctx.sounds`）。
    ///
    /// `engine.rs::call_run_ctx` 把 `host_fields` 原样 `set` 进 ctx ——那是**同一个 VM**
    /// 的 Lua，所以这里拿到的 table 能安全传进去（mlua 跨 state 传值会 panic）。宿主应
    /// 先确保 VM 已建好（可先调 [`EngineCache::load_error`]），避免构造完 table 后
    /// mtime 又变导致 `run` 重建 VM、新旧 state 不一致。
    pub fn lua(&mut self, script_path: &Path) -> Result<&mlua::Lua, String> {
        Ok(self.vm(script_path)?.lua())
    }

    /// 是否已缓存某个脚本（测试用）。
    pub fn contains(&self, path: &Path) -> bool {
        self.entries.contains_key(path)
    }
}

/// 脚本绝对路径 → 展示用相对路径（供错误前缀）。这里只回落到文件名，够排错用。
fn script_relative(path: &Path) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("<未知脚本>")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script_dir(tag: &str, body: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "curseen_runtime_test_{}_{}_{}",
            std::process::id(),
            tag,
            body.len().wrapping_mul(1_000_003)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("demo.lua");
        std::fs::write(&path, body).unwrap();
        (dir, path)
    }

    #[test]
    fn reuses_vm_without_mtime_change() {
        let (_d, path) = script_dir(
            "reuse",
            "function run(ctx, params)\n  counter = (counter or 0) + 1\n  return counter\nend\n",
        );
        let mut cache = EngineCache::new();
        let params = HashMap::new();
        let v1 = to_int(&cache.run(&path, "effect", &[], &params).unwrap());
        // 第二次调用复用同一 VM → 全局 counter 延续。
        let v2 = to_int(&cache.run(&path, "effect", &[], &params).unwrap());
        assert_eq!(v1, 1);
        assert_eq!(v2, 2);
        assert!(cache.contains(&path));
    }

    #[test]
    fn mtime_change_rebuilds_vm() {
        let (d, path) = script_dir("mtime", "function run(ctx, params) return 1 end");
        let mut cache = EngineCache::new();
        let params = HashMap::new();
        assert_eq!(
            to_int(&cache.run(&path, "effect", &[], &params).unwrap()),
            1
        );
        assert!(cache.contains(&path));
        // 改脚本内容（mtime 变）→ 下次调用重建。
        std::fs::write(d.join("demo.lua"), "function run(ctx, params) return 2 end").unwrap();
        // ★ 显式把 mtime 拨到未来：测试跑得快时两次写入会落在同一秒内（NTFS mtime
        //   精度 ~1s），"mtime 变了"的前提就不成立——这是测试自身的精度缺陷
        //   （连续两轮红过），不是缓存逻辑错。拨 mtime 让前提确定性成立。
        {
            let f = std::fs::File::options().write(true).open(&path).unwrap();
            f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(2))
                .unwrap();
        }
        assert_eq!(
            to_int(&cache.run(&path, "effect", &[], &params).unwrap()),
            2
        );
    }

    fn to_int(v: &MValue) -> i64 {
        match v {
            MValue::Integer(i) => *i,
            MValue::Number(n) => *n as i64,
            other => panic!("应返回数值，实际 {other:?}"),
        }
    }

    #[test]
    fn ctx_has_role_and_fs() {
        let (d, path) = script_dir(
            "ctx",
            "function run(ctx, params)\n  if ctx.role ~= \"image\" then return \"bad-role\" end\n  local fs = ctx.fs\n  if fs == nil then return \"no-fs\" end\n  local c = fs:read(\"./cfg.txt\")\n  return tostring(c)\nend\n",
        );
        std::fs::write(d.join("cfg.txt"), "hello-cfg").unwrap();
        let mut cache = EngineCache::new();
        let params = HashMap::new();
        let out = cache.run(&path, "image", &[], &params).unwrap();
        // run 返回字符串。
        match out {
            MValue::String(s) => assert_eq!(s.to_string_lossy(), "hello-cfg"),
            other => panic!("应返回读取内容，实际 {other:?}"),
        }
    }

    #[test]
    fn load_failure_reported_separately() {
        let (d, path) = script_dir("loadfail", "这不是合法 Lua ... syntax error（");
        let mut cache = EngineCache::new();
        assert!(
            cache
                .load_error(&path)
                .unwrap_or_default()
                .contains("load():"),
            "加载失败要带 load() 前缀"
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
