//! 暴露给 Lua 脚本的只读文件访问 `ctx.fs`（相对**脚本所在目录**）。
//!
//! 统一 Lua 框架里，脚本要能读自己旁边的文件（配置文件、数据文件、素材清单等）。
//! 规则只有一组，对所有 `role` 一视同仁：
//! - `ctx.fs:read("./abc.txt")`    = 脚本同目录下的文件
//! - `ctx.fs:list("./aaa")`        = 脚本同级子目录的条目
//! - `../` 与**绝对路径**一律返回 `nil`（逃逸校验，防脚本误读到
//!   `C:\Users\...` 或越出脚本包——是**防手滑**，不是安全沙箱；脚本自用可信）。
//!
//! 全程用 `PathBuf` 传递、不中途转 `String`：脚本名/文件名可能含中文，转 String
//! 再拼在不同平台上容易踩编码/序列化坑，用 `Path` 组件逐个推进最稳。

use std::path::{Component, Path, PathBuf};

use mlua::{UserData, UserDataMethods, Value};

#[cfg(test)]
use mlua::Lua;

/// 把脚本书写的**相对路径**解析成相对 `base`（脚本所在目录）的绝对落点。
///
/// - 空串 / 仅空白 → `None`。
/// - 绝对路径（含盘符、UNC、`\` 根相对）→ `None`（逃逸拒绝）。
/// - 路径中出现 `..`（父级）→ `None`（越界拒绝）。
/// - `Component::Normal` 与 `CurDir`（`.`）逐段拼到 `base`。
///
/// `base` 必须是脚本自己的目录；结果始终落在 `base` 之下，路径段不歪。
pub fn resolve(base: &Path, rel: &str) -> Option<PathBuf> {
    let rel = rel.trim();
    if rel.is_empty() {
        return None;
    }
    let p = Path::new(rel);
    if p.is_absolute() || p.has_root() {
        // Windows 上 `C:\x` / `\x`（有根前缀）、Unix 上 `/x` 都在这里拦下。
        return None;
    }
    let mut out = base.to_path_buf();
    for comp in p.components() {
        match comp {
            Component::Normal(c) => out.push(c),
            Component::CurDir => {}
            // ParentDir 是唯一可能把结果推出 base 的段，直接拒绝。
            Component::ParentDir => return None,
            // Prefix 出现说明是 `C:...` 这类带盘符但 is_absolute 未必为真的写法，也拦。
            Component::Prefix(_) | Component::RootDir => return None,
        }
    }
    Some(out)
}

/// 只读 FS userdata（`ctx.fs`）。`base` = 脚本所在目录，等于脚本自己的引擎目录。
pub struct FsApi {
    base: PathBuf,
}

impl FsApi {
    pub fn new(base: PathBuf) -> Self {
        Self { base }
    }

    /// 把相对路径规整成绝对落点；越界返回 `None`（方法层的守卫）。
    fn target(&self, rel: &str) -> Option<PathBuf> {
        resolve(&self.base, rel)
    }
}

#[cfg(test)]
impl FsApi {
    /// 真跑一段 Lua 调 `fs:read`，返回 `Option<String>`（越界/缺失 = nil）。
    fn call_read(&self, lua: &Lua, rel: &str) -> Option<String> {
        let fs = lua.create_userdata(FsApi::new(self.base.clone())).unwrap();
        // `fs:read(rel)`（userdata 冒号方法）驱动真实路径。
        let code = format!("local f = ...\nreturn f:read({:?})", rel);
        lua.load(&code).call(fs).ok()
    }

    fn call_exists(&self, rel: &str) -> bool {
        let lua = Lua::new();
        let fs = lua.create_userdata(FsApi::new(self.base.clone())).unwrap();
        let code = format!("local f = ...\nreturn f:exists({:?})", rel);
        lua.load(&code).call(fs).unwrap_or(false)
    }

    fn call_read_bytes(&self, lua: &Lua, rel: &str) -> Option<mlua::LuaString> {
        let fs = lua.create_userdata(FsApi::new(self.base.clone())).unwrap();
        let code = format!("local f = ...\nreturn f:read_bytes({:?})", rel);
        lua.load(&code).call(fs).ok()
    }
}

impl UserData for FsApi {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // fs:read(rel) -> string | nil —— 文本读取；不存在/越界/读失败返回 nil。
        methods.add_method("read", |lua, this, rel: String| {
            let p = match this.target(&rel) {
                Some(p) => p,
                None => return Ok(Value::Nil),
            };
            match std::fs::read_to_string(&p) {
                Ok(s) => lua.create_string(&s).map(Value::String).or(Ok(Value::Nil)),
                Err(_) => Ok(Value::Nil),
            }
        });
        // fs:read_bytes(rel) -> raw string | nil —— 二进制读取（编码保存）。
        methods.add_method("read_bytes", |lua, this, rel: String| {
            let p = match this.target(&rel) {
                Some(p) => p,
                None => return Ok(Value::Nil),
            };
            match std::fs::read(&p) {
                Ok(bytes) => lua
                    .create_string(&bytes)
                    .map(Value::String)
                    .or(Ok(Value::Nil)),
                Err(_) => Ok(Value::Nil),
            }
        });
        // fs:exists(rel) -> bool —— 存在性判断（不区分文件/目录）。
        methods.add_method("exists", |_lua, this, rel: String| {
            Ok(this.target(&rel).map(|p| p.exists()).unwrap_or(false))
        });
        // fs:list(rel) -> {names...} | nil —— 目录条目（文件名/目录名，不含 `..`/`.`）。
        methods.add_method("list", |lua, this, rel: String| {
            let p = match this.target(&rel) {
                Some(p) => p,
                None => return Ok(Value::Nil),
            };
            let entries = std::fs::read_dir(&p).ok();
            match entries {
                None => Ok(Value::Nil),
                Some(entries) => {
                    let tbl = lua.create_table()?;
                    for (i, ent) in (1..).zip(entries.flatten()) {
                        let name = ent.file_name().to_string_lossy().into_owned();
                        tbl.set(i, name)?;
                    }
                    Ok(Value::Table(tbl))
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "curseen_fs_test_{}_{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::create_dir_all(&d);
        d
    }

    #[test]
    fn normal_and_curdir_resolve_into_base() {
        let base = temp_dir();
        assert_eq!(resolve(&base, "./a.txt"), Some(base.join("a.txt")));
        assert_eq!(resolve(&base, "a.txt"), Some(base.join("a.txt")));
        assert_eq!(
            resolve(&base, "./aaa/qqq.txt"),
            Some(base.join("aaa").join("qqq.txt"))
        );
    }

    #[test]
    fn parent_and_absolute_are_rejected() {
        let base = temp_dir();
        assert_eq!(resolve(&base, "../x.txt"), None);
        assert_eq!(resolve(&base, "a/../../y.txt"), None);
        assert_eq!(resolve(&base, "C:\\x.txt"), None);
        assert_eq!(resolve(&base, "/x.txt"), None);
        assert_eq!(resolve(&base, "\\x.txt"), None);
    }

    #[test]
    fn empty_and_blank_are_rejected() {
        let base = temp_dir();
        assert_eq!(resolve(&base, ""), None);
        assert_eq!(resolve(&base, "   "), None);
    }

    #[test]
    fn read_and_exists_use_script_relative_path() {
        let base = temp_dir();
        std::fs::write(base.join("a.txt"), "你好").unwrap();
        let fs = FsApi::new(base.clone());
        let lua = Lua::new();
        let read: Option<String> = fs.call_read(&lua, "./a.txt");
        assert_eq!(read.as_deref(), Some("你好"));
        let nil: Option<String> = fs.call_read(&lua, "../a.txt");
        assert_eq!(nil, None);
        assert!(fs.call_exists("./a.txt"));
        assert!(!fs.call_exists("./no.txt"));
        assert!(!fs.call_exists("../no.txt"));
    }

    #[test]
    fn read_bytes_roundtrips_binary() {
        let base = temp_dir();
        std::fs::write(base.join("b.bin"), [0u8, 1, 255, 128]).unwrap();
        let fs = FsApi::new(base);
        let lua = Lua::new();
        let bytes: Option<mlua::LuaString> = fs.call_read_bytes(&lua, "./b.bin");
        let v = bytes.unwrap().as_bytes().to_vec();
        assert_eq!(v, vec![0u8, 1, 255, 128]);
    }
}
