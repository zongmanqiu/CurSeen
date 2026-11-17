//! 宿主状态文件：给外部 AI（MCP `host_status`）一个"宿主在不在、看没看见改动"的读数。
//!
//! 只在**真实状态变化**时写（启动 / 资产重扫 / 面板开关），不跑周期心跳——空闲零 I/O。
//! 文件落 `temp/lua_tool_state.json`，与工具台的其它临时产物同一棵树。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value as Json};

/// 相对程序目录的状态文件路径（MCP 侧报给调用方的也是这一条）。
pub const REL: &str = "temp/lua_tool_state.json";

/// 函数名与路径解耦：测试传临时目录，生产传 `paths::app_dir()`。
pub fn path(app: &Path) -> PathBuf {
    app.join(REL)
}

/// 现在（Unix 毫秒）。时钟不可用时回 0（不是错误：只是读数不精确）。
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 每个挂载目录的 `.lua` 数（`scripts/` 允许嵌套一层）。
pub fn count_scripts(lua: &Path) -> Json {
    let mounts = ["MouseTrail", "HoverDot", "ClickFx", "KeySound", "scripts"];
    let mut out = serde_json::Map::new();
    for top in mounts {
        out.insert(top.to_string(), json!(count_dir(&lua.join(top), 2)));
    }
    Json::Object(out)
}

/// 磁盘上最新一个脚本文件的 `mtime`（Unix 毫秒）。用于回答"宿主上次重扫之后
/// 又有没有人改过文件"。
pub fn latest_mtime_ms(lua: &Path) -> u64 {
    let mounts = ["MouseTrail", "HoverDot", "ClickFx", "KeySound", "scripts"];
    mounts
        .iter()
        .map(|top| latest_in(&lua.join(top), 2))
        .max()
        .unwrap_or(0)
}

fn latest_in(dir: &Path, depth: u32) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut max = 0;
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if depth > 0 {
                max = max.max(latest_in(&p, depth - 1));
            }
        } else if p.extension().and_then(|x| x.to_str()) == Some("lua") {
            if let Ok(m) = std::fs::metadata(&p).and_then(|m| m.modified()) {
                if let Ok(d) = m.duration_since(UNIX_EPOCH) {
                    max = max.max(d.as_millis() as u64);
                }
            }
        }
    }
    max
}

fn count_dir(dir: &Path, depth: u32) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut n = 0;
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if depth > 0 {
                n += count_dir(&p, depth - 1);
            }
        } else if p.extension().and_then(|x| x.to_str()) == Some("lua") {
            n += 1;
        }
    }
    n
}

/// 写一份快照（调用方给业务字段，这里补 pid/版本/时间/计数）。
///
/// 返回值仅用于日志/测试：写失败不影响宿主主流程（状态文件是旁路读数，不是判据）。
pub fn write(app: &Path, mut snapshot: Json) -> Option<Json> {
    if let Some(o) = snapshot.as_object_mut() {
        o.insert("schema".into(), json!(1));
        o.insert("version".into(), json!(env!("CARGO_PKG_VERSION")));
        o.insert("pid".into(), json!(std::process::id()));
        o.insert("updated_at_ms".into(), json!(now_ms()));
        o.insert(
            "mounts".into(),
            count_scripts(&app.join("assets").join("lua")),
        );
    }
    let full = path(app);
    if let Some(dir) = full.parent() {
        if let Err(e) = std::fs::create_dir_all(dir) {
            log::warn!("建宿主状态目录失败：{e}");
            return None;
        }
    }
    let text = serde_json::to_string_pretty(&snapshot).ok()?;
    let tmp = full.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, text.as_bytes()) {
        log::warn!("写宿主状态失败：{e}");
        return None;
    }
    if let Err(e) = std::fs::rename(&tmp, &full) {
        let _ = std::fs::remove_file(&tmp);
        log::warn!("替换宿主状态失败：{e}");
        return None;
    }
    Some(snapshot)
}

/// 读状态文件；不存在或损坏都回 `None`（由调用方决定怎么表达）。
pub fn read(app: &Path) -> Option<Json> {
    let text = std::fs::read_to_string(path(app)).ok()?;
    serde_json::from_str(&text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(tag: &str) -> PathBuf {
        let p =
            std::env::temp_dir().join(format!("curseen_host_state_{}_{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(p.join("assets/lua/HoverDot")).unwrap();
        std::fs::create_dir_all(p.join("assets/lua/scripts/pkg")).unwrap();
        std::fs::write(p.join("assets/lua/HoverDot/a.lua"), "x=1").unwrap();
        std::fs::write(p.join("assets/lua/scripts/b.lua"), "x=1").unwrap();
        std::fs::write(p.join("assets/lua/scripts/pkg/c.lua"), "x=1").unwrap();
        p
    }

    #[test]
    fn snapshot_roundtrips_with_counts_and_identity() {
        let a = app("round");
        let snap = write(
            &a,
            json!({
                "started_at_ms": 1,
                "updated_at_ms": 1,
                "asset_revision": 3,
                "last_assets_change_ms": 2,
                "panel_open": true,
            }),
        )
        .expect("应写入");
        let read_back = read(&a).expect("应读回");
        assert_eq!(read_back["pid"], std::process::id());
        assert_eq!(read_back["asset_revision"], 3);
        assert_eq!(read_back["mounts"]["HoverDot"], 1);
        assert_eq!(read_back["mounts"]["scripts"], 2, "包目录要递归计数");
        assert_eq!(snap["schema"], 1);
        let _ = std::fs::remove_dir_all(&a);
    }

    #[test]
    fn missing_state_file_is_none() {
        let a =
            std::env::temp_dir().join(format!("curseen_host_state_none_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&a);
        assert!(read(&a).is_none());
    }
}
