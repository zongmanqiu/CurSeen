//! 在线脚本包：清单解析、安装 / 更新 / 卸载（m36，**宿主内置**）。
//!
//! # 包的形状（与《CurSeen-Scripts》仓库的打包脚本一一对应）
//!
//! ```text
//! <id>_<version>.zip
//! └─ <id>/                ← v2：解压到 assets/lua/scripts/ 后就是稳定包文件夹
//!    ├─ pack.json         { id, name, version, layout: 2, files: [...] }
//!    ├─ *.lua             ← 脚本平铺在包根
//!    ├─ <tool_dest>/…     ← 官方工具（如有）
//!    └─ info/{LICENSE,README.md,HISTORY.md}
//! ```
//!
//! ★ 安装 = **整个文件夹替换**：`scripts/<id>/` 旧文件夹先挪进暂存区，新文件夹落到原位。
//!   脚本 id（`scripts/<id>/x`）跨版本不变，用户参数/快捷键不会因为升级换目录而失联。
//!   只支持这一种格式；旧 v1 包（zip 根放 `files/`）直接拒收。
//!
//! # 安全面的三条（都在**动手之前**判）
//!
//! 1. 清单里的每个路径都是**相对包文件夹**的安全路径——`..`、绝对路径、盘符一律拒。
//!    `unzip` 那层已经拦过路径逃逸，这里是第二道。
//! 2. 包内文件集合必须与 `pack.json` 的 `files` **完全一致**（多一个少一个都拒）：
//!    否则"卸载"会漏删、"安装"会带进没登记的东西。
//! 3. 首次安装时目标文件夹已存在、且不是本包管理的 ⇒ 拒（那是用户自己的目录）；
//!    全部检查通过后才开始搬文件夹，所以"拒"的时候目录一个字节都没动。
//!
//! # 不做 sha256
//!
//! 用户口径（2026-10-28）："哈希值我感觉不用了，文件太多"——升级判断只看包名 + 版本号。
//! 完整性仍然有两道：下载层校验 `Content-Length`，解压层逐条目校验 **zip CRC**。
//! sha256 能多挡的只有"HTTPS 之后仍被换包"，接受这个残余风险（官方双镜像 + HTTPS）。

use std::path::{Component, Path, PathBuf};

/// 每个装好的包在**它自己的目录**里留一份记录（对齐 RuPix 把 metadata.json 放在解码器目录里的做法）。
///
/// 记录 = 这个包装了哪些文件、装的是哪个版本；卸载/更新只认它。**不再有全局记录目录**。
pub const RECORD_NAME: &str = ".curseen-pack.json";

/// 当前宿主支持的 Lua 脚本 API 级别。清单里的 `api` 高于它 ⇒ `too_new`（按钮置灰）。
/// 当前宿主支持的 Lua 脚本 API 级别。
///
/// 3 = 新增 `ext:data_dir`（脚本私有持久目录）；2 = `ext:open_temp/open_url`；
/// 1 = 1.0 时代的接口集。
pub const LUA_API: u32 = 3;

/// 官方镜像的显示名（下标即 [`CatalogPack::urls`] 的下标）。
pub const SOURCE_LABELS: [&str; 2] = ["Gitee", "GitHub"];

/// 清单里的一枚可下载包。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogPack {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub size: u64,
    /// 两个镜像的下载地址（与 [`SOURCE_LABELS`] 同序；`None` = 该镜像没有发布）。
    pub urls: [Option<String>; 2],
    /// 宿主最低版本；`None` = 不限制。
    pub min_host: Option<String>,
    /// 作者（官方包统一 `CurSeen`）。
    pub author: String,
    /// 许可证（SPDX 名，例如 `AGPL-3.0`）。
    pub license: String,
    /// 发行时间（`YYYY-MM-DD`）。
    pub released: String,
    /// 需要的 Lua 脚本 API 级别；高于宿主 [`LUA_API`] ⇒ 需要更新宿主。
    pub api: u32,
    /// 安装后体积（包内全部文件未压缩总和；含文档与官方工具）。
    pub unpacked_size: u64,
    /// 包内文件数（含 `pack.json`、`info/` 与工具文件；清单侧就有，未装也能显示）。
    pub file_count: u64,
}

impl CatalogPack {
    /// 选中镜像的下载地址。
    pub fn url_for(&self, source: usize) -> Option<&str> {
        self.urls.get(source).and_then(|u| u.as_deref())
    }

    /// 哪些镜像有地址（bit i = 第 i 个镜像）；UI 用它决定行尾按钮能不能点。
    pub fn source_mask(&self) -> u8 {
        let mut mask = 0u8;
        for (i, u) in self.urls.iter().enumerate() {
            if u.is_some() {
                mask |= 1 << i;
            }
        }
        mask
    }
}

/// 解析仓库根的 `scripts.json`。坏条目**跳过并给出原因**，不让一条脏数据毁掉整张表
/// （但至少要有一条有效条目，否则整体按失败处理——空表和"解析失败"对用户是两件事）。
pub fn parse_catalog(text: &str) -> Result<(Vec<CatalogPack>, Vec<String>), String> {
    let v: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("清单不是合法 JSON：{e}"))?;
    let Some(arr) = v.as_array() else {
        return Err("清单根节点必须是数组".to_string());
    };
    let mut packs = Vec::new();
    let mut skipped = Vec::new();
    for (i, item) in arr.iter().enumerate() {
        let get = |k: &str| item.get(k).and_then(|x| x.as_str()).map(str::to_string);
        let (Some(id), Some(name), Some(version)) = (get("id"), get("name"), get("version")) else {
            skipped.push(format!("第 {} 条缺 id/name/version", i + 1));
            continue;
        };
        if !valid_id(&id) {
            skipped.push(format!("「{name}」的 id 不合法：{id}"));
            continue;
        }
        if !valid_version(&version) {
            skipped.push(format!("「{name}」的版本号不合法：{version}"));
            continue;
        }
        if packs.iter().any(|p: &CatalogPack| p.id == id) {
            // 同 id 两条：装的时候只认第一条，第二条永远装不上——说清楚比默默忽略强。
            skipped.push(format!("「{name}」的 id 与前面重复：{id}"));
            continue;
        }
        let mut urls: [Option<String>; 2] = [None, None];
        if let Some(obj) = item.get("urls").and_then(|u| u.as_object()) {
            for (i, key) in ["gitee", "github"].iter().enumerate() {
                if let Some(s) = obj.get(*key).and_then(|x| x.as_str()) {
                    if s.starts_with("https://") {
                        urls[i] = Some(s.to_string());
                    }
                }
            }
        }
        if urls.iter().all(Option::is_none) {
            skipped.push(format!("「{name}」没有 https 下载地址"));
            continue;
        }
        packs.push(CatalogPack {
            id,
            name,
            version,
            description: get("description").unwrap_or_default(),
            size: item.get("size").and_then(|x| x.as_u64()).unwrap_or(0),
            urls,
            min_host: get("min_host"),
            author: get("author").unwrap_or_default(),
            license: get("license").unwrap_or_default(),
            released: get("released").unwrap_or_default(),
            api: item
                .get("api")
                .and_then(|x| x.as_u64())
                .unwrap_or(0)
                .min(u32::MAX as u64) as u32,
            unpacked_size: item
                .get("unpacked_size")
                .and_then(|x| x.as_u64())
                .unwrap_or(0),
            file_count: item.get("file_count").and_then(|x| x.as_u64()).unwrap_or(0),
        });
    }
    if packs.is_empty() {
        return Err(format!(
            "清单里没有一枚可用的包（跳过 {} 条：{}）",
            skipped.len(),
            skipped.join("；")
        ));
    }
    Ok((packs, skipped))
}

/// `id` 规则与 Rupix 的同款：字母数字开头，允许 `. _ -`。
fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// 版本号：`数字(.数字)*`。比较逐段按数值，缺的段按 0（`1.2` == `1.2.0`）。
pub fn valid_version(s: &str) -> bool {
    !s.is_empty()
        && s.split('.')
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

/// `a` 是否比 `b` 新。
pub fn version_newer(a: &str, b: &str) -> bool {
    let pa: Vec<u64> = a.split('.').filter_map(|p| p.parse().ok()).collect();
    let pb: Vec<u64> = b.split('.').filter_map(|p| p.parse().ok()).collect();
    let n = pa.len().max(pb.len());
    for i in 0..n {
        let x = pa.get(i).copied().unwrap_or(0);
        let y = pb.get(i).copied().unwrap_or(0);
        if x != y {
            return x > y;
        }
    }
    false
}

/// 包内的 `pack.json`（装完存进包目录里的 `.curseen-pack.json`，卸载靠它）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    /// 相对 `assets/lua/` 的路径，固定以 `scripts/<包>/` 开头。
    pub files: Vec<String>,
}

/// 已经装着的包（从**包自己目录里的记录**读）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPack {
    pub manifest: PackManifest,
    /// 记录文件（正常安装总有；测试/手工构造的包可能为 `None`）。
    pub record: Option<PathBuf>,
}

impl PackManifest {
    pub fn parse(text: &str) -> Result<Self, String> {
        let v: serde_json::Value =
            serde_json::from_str(text).map_err(|e| format!("pack.json 不是合法 JSON：{e}"))?;
        let id = v
            .get("id")
            .and_then(|x| x.as_str())
            .ok_or("pack.json 缺 id")?
            .to_string();
        let name = v
            .get("name")
            .and_then(|x| x.as_str())
            .unwrap_or(&id)
            .to_string();
        let version = v
            .get("version")
            .and_then(|x| x.as_str())
            .ok_or("pack.json 缺 version")?
            .to_string();
        if !valid_id(&id) || !valid_version(&version) {
            return Err(format!("pack.json 的 id/version 不合法：{id} / {version}"));
        }
        let files = v
            .get("files")
            .and_then(|x| x.as_array())
            .ok_or("pack.json 缺 files 数组")?
            .iter()
            .filter_map(|x| x.as_str())
            .map(str::to_string)
            .collect::<Vec<_>>();
        if files.is_empty() {
            return Err("pack.json 的 files 是空的".to_string());
        }
        Ok(Self {
            id,
            name,
            version,
            files,
        })
    }

    pub fn to_json(&self) -> String {
        serde_json::json!({
            "id": self.id,
            "name": self.name,
            "version": self.version,
            "files": self.files,
        })
        .to_string()
    }
}

/// 记录里的 `scripts/<id>/…` → 安全相对路径；只允许这一种形状。
fn safe_record_rel(rel: &str) -> Option<PathBuf> {
    if rel.is_empty() || rel.contains(':') {
        return None;
    }
    let mut it = Path::new(rel).components();
    let Component::Normal(first) = it.next()? else {
        return None;
    };
    if first.to_str()? != "scripts" {
        return None;
    }
    let mut out = PathBuf::from(first);
    for c in it {
        match c {
            Component::Normal(s) => out.push(s),
            _ => return None, // `..` / 根 / 盘符一律拒
        }
    }
    // 必须是 `scripts/<包>/<文件>`，只有 `scripts/<包>` 或 `scripts/a.lua` 都不算文件。
    if out.components().count() < 3 {
        return None;
    }
    Some(out)
}

/// 读已装的包：只认**包目录里的记录**。
///
/// ★ 不再有全局记录目录——"装了什么"就写在装它的那个目录里，
///   与 RuPix 把 metadata.json 放进解码器目录同一思路。
pub fn installed_packs(assets_root: &Path) -> Vec<InstalledPack> {
    let mut out = Vec::new();
    let base = assets_root.join("scripts");
    if let Ok(rd) = std::fs::read_dir(&base) {
        for e in rd.flatten() {
            let path = e.path();
            if !path.is_dir() {
                continue;
            }
            let rec = path.join(RECORD_NAME);
            if let Ok(text) = std::fs::read_to_string(&rec) {
                match PackManifest::parse(&text) {
                    Ok(manifest) => out.push(InstalledPack {
                        manifest,
                        record: Some(rec),
                    }),
                    Err(err) => log::warn!("跳过已装包记录 {}：{err}", rec.display()),
                }
            }
        }
    }
    out.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
    out
}

/// 一次安装/更新干了什么（给结果窗/日志）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallReport {
    pub id: String,
    pub name: String,
    pub version: String,
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
}

/// v2 包内相对路径（相对包文件夹）→ 安全的 `PathBuf`；`..`/绝对路径/盘符一律拒。
fn safe_package_rel(rel: &str) -> Option<PathBuf> {
    if rel.is_empty() || rel.contains(':') {
        return None;
    }
    let mut out = PathBuf::new();
    for c in Path::new(rel).components() {
        match c {
            Component::Normal(s) => out.push(s),
            _ => return None,
        }
    }
    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}

/// zip 里只有一个包文件夹 `<id>/`，里面有 `pack.json`、平铺脚本、`info/` 与可选工具。
///
/// 安装 = 把整个文件夹换到 `scripts/<id>/`；文档/许可证随包走，不再有单独的文档夹。
fn install_v2_folder(staging: &Path, assets_root: &Path) -> Result<InstallReport, String> {
    use std::collections::BTreeSet;
    let dirs: Vec<PathBuf> = std::fs::read_dir(staging)
        .map_err(|e| format!("读暂存目录失败：{e}"))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    if dirs.len() != 1 {
        return Err("不是 v2 包：zip 根下只应有一个包文件夹 <id>/".to_string());
    }
    let pkg = &dirs[0];
    let manifest_path = pkg.join("pack.json");
    if !manifest_path.is_file() {
        return Err("不是 v2 包：包文件夹里没有 pack.json".to_string());
    }
    let text =
        std::fs::read_to_string(&manifest_path).map_err(|e| format!("读 pack.json 失败：{e}"))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("pack.json 不是合法 JSON：{e}"))?;
    if value.get("layout").and_then(|v| v.as_u64()) != Some(2) {
        return Err("不是 v2 包：pack.json 缺 layout: 2".to_string());
    }
    let manifest = PackManifest::parse(&text)?;
    let folder = pkg.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if folder != manifest.id {
        return Err(format!(
            "包文件夹名「{folder}」与 pack.json 的 id「{}」不一致",
            manifest.id
        ));
    }
    for need in ["info/LICENSE", "info/README.md", "info/HISTORY.md"] {
        if !pkg.join(need).is_file() {
            return Err(format!("包不完整：缺 {need}（官方包必须带手续）"));
        }
    }

    let mut listed = BTreeSet::new();
    for rel in &manifest.files {
        let Some(p) = safe_package_rel(rel) else {
            return Err(format!("pack.json 里有不允许的路径：{rel}"));
        };
        if !pkg.join(&p).is_file() {
            return Err(format!("包内缺文件：{rel}"));
        }
        if !listed.insert(p) {
            return Err(format!("pack.json 里有重复路径：{rel}"));
        }
    }
    let mut actual = BTreeSet::new();
    let mut stack = vec![pkg.clone()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(rel) = p.strip_prefix(pkg) {
                actual.insert(rel.to_path_buf());
            }
        }
    }
    if actual != listed {
        let extra: Vec<String> = actual
            .difference(&listed)
            .map(|p| p.display().to_string())
            .take(4)
            .collect();
        return Err(format!(
            "包内文件与 pack.json 的 files 不一致（多出来的：{}）——卸载会漏删，拒绝安装",
            if extra.is_empty() {
                "（无）".to_string()
            } else {
                extra.join("、")
            }
        ));
    }

    let target = assets_root.join("scripts").join(&manifest.id);
    let old_pack = installed_packs(assets_root)
        .into_iter()
        .find(|p| p.manifest.id == manifest.id);
    if target.exists() && old_pack.is_none() {
        return Err(format!(
            "目标目录已存在且不是本安装包管理的：{}——先删除它再装",
            target.display()
        ));
    }
    let old_set: BTreeSet<PathBuf> = old_pack
        .as_ref()
        .map(|p| {
            p.manifest
                .files
                .iter()
                .filter_map(|r| safe_record_rel(r))
                .collect()
        })
        .unwrap_or_default();
    let mut new_set = BTreeSet::new();
    for rel in &manifest.files {
        let p = safe_package_rel(rel).unwrap();
        new_set.insert(Path::new("scripts").join(&manifest.id).join(p));
    }
    let added = new_set.difference(&old_set).count();
    let updated = new_set.intersection(&old_set).count();
    let removed = old_set.difference(&new_set).count();

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("建包目录失败（{}）：{e}", parent.display()))?;
    }
    let backup = staging.join("old_package");
    let had_target = target.exists();
    if had_target {
        std::fs::rename(&target, &backup)
            .map_err(|e| format!("旧包目录挪不开（{}）：{e}", target.display()))?;
    }
    if let Err(e) = std::fs::rename(pkg, &target) {
        if had_target {
            let _ = std::fs::rename(&backup, &target);
        }
        return Err(format!("包目录落盘失败：{e}"));
    }

    let record = target.join(RECORD_NAME);
    let record_manifest = PackManifest {
        id: manifest.id.clone(),
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        files: new_set
            .iter()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect(),
    };
    if let Err(e) = std::fs::write(&record, record_manifest.to_json()) {
        let _ = std::fs::remove_dir_all(&target);
        if had_target {
            let _ = std::fs::rename(&backup, &target);
        }
        return Err(format!("写安装记录失败（{}）：{e}", record.display()));
    }
    if had_target {
        let _ = std::fs::remove_dir_all(&backup);
    }

    log::info!(
        "脚本包 {} v{} 已安装（v2 文件夹）：新增 {} / 更新 {} / 移除 {}",
        manifest.id,
        manifest.version,
        added,
        updated,
        removed
    );
    Ok(InstallReport {
        id: manifest.id,
        name: manifest.name,
        version: manifest.version,
        added,
        updated,
        removed,
    })
}

/// 装一个包 zip：只接受 v2 包（单个 `<id>/` + `layout: 2`），**先全部检查、后动手**。
///
/// `assets_root` = `assets/lua`。
pub fn install_zip(zip: &Path, assets_root: &Path) -> Result<InstallReport, String> {
    // 暂存区放在 `assets/.store_staging`：与脚本目录**同一卷**（跨盘 rename 会失败），
    // 又不在 `assets/lua` 里（目录监听与注册表不会把半成品当成新脚本）。
    let staging = assets_root
        .parent()
        .map(|p| p.join(".store_staging"))
        .unwrap_or_else(|| std::env::temp_dir().join("curseen_store_staging"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| format!("建暂存目录失败：{e}"))?;
    let cleanup = || {
        let _ = std::fs::remove_dir_all(&staging);
    };

    // —— 1. 解到暂存区（unzip 已拦路径逃逸、逐条目校验 CRC）——
    if let Err(e) = crate::platform::unzip::extract_zip(zip, &staging) {
        cleanup();
        return Err(e);
    }
    let report = install_v2_folder(&staging, assets_root);
    cleanup();
    report
}

/// 卸载：只删记录里登记、且仍在 `scripts/` 下的文件；目录删空为止。
pub fn uninstall(id: &str, assets_root: &Path) -> Result<usize, String> {
    if !valid_id(id) {
        return Err(format!("安装 id 不合法：{id}"));
    }
    let Some(pack) = installed_packs(assets_root)
        .into_iter()
        .find(|p| p.manifest.id == id)
    else {
        return Err(format!("没有「{id}」的安装记录（可能是手动放进来的脚本）"));
    };
    let mut removed = 0usize;
    for rel in &pack.manifest.files {
        let Some(p) = safe_record_rel(rel) else {
            log::warn!("卸载 {id}：跳过不合法路径 {rel}");
            continue;
        };
        let target = assets_root.join(&p);
        if std::fs::remove_file(&target).is_ok() {
            removed += 1;
        }
        prune_empty_dirs(target.parent(), assets_root);
    }
    if let Some(rec) = &pack.record {
        let _ = std::fs::remove_file(rec);
        prune_empty_dirs(rec.parent(), assets_root);
    }
    log::info!("脚本包 {id} 已卸载：删除 {removed} 个文件");
    Ok(removed)
}

/// 自下而上删空目录，**不越过 `assets_root`**。
fn prune_empty_dirs(from: Option<&Path>, stop_at: &Path) {
    let mut cur = from.map(Path::to_path_buf);
    while let Some(d) = cur {
        if !d.starts_with(stop_at) || d == stop_at {
            break;
        }
        // 只删**空目录**；非空（用户还有别的脚本）就停。
        if std::fs::remove_dir(&d).is_err() {
            break;
        }
        cur = d.parent().map(Path::to_path_buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::unzip::make_zip;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("curseen_store_{}_{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 造一个 v2 包：`<id>/pack.json`（layout=2、files 全量）+ `info/` + 平铺脚本。
    fn make_v2_pack(id: &str, version: &str, scripts: &[(&str, &[u8])]) -> Vec<u8> {
        let mut files: Vec<String> = vec![
            "pack.json".to_string(),
            "info/LICENSE".to_string(),
            "info/README.md".to_string(),
            "info/HISTORY.md".to_string(),
        ];
        files.extend(scripts.iter().map(|(rel, _)| (*rel).to_string()));
        files.sort();
        let meta = serde_json::json!({
            "id": id, "name": "测试包", "version": version,
            "layout": 2, "files": files,
        });
        let mut entries: Vec<(String, Vec<u8>, u16)> = vec![
            (format!("{id}/pack.json"), meta.to_string().into_bytes(), 8),
            (format!("{id}/info/LICENSE"), b"MIT".to_vec(), 0),
            (format!("{id}/info/README.md"), b"readme".to_vec(), 0),
            (format!("{id}/info/HISTORY.md"), b"# history".to_vec(), 0),
        ];
        for (rel, data) in scripts {
            entries.push((format!("{id}/{rel}"), data.to_vec(), 8));
        }
        let refs: Vec<(&str, &[u8], u16)> = entries
            .iter()
            .map(|(n, d, m)| (n.as_str(), d.as_slice(), *m))
            .collect();
        make_zip(&refs)
    }

    #[test]
    fn catalog_parses_and_skips_bad_entries() {
        let text = r#"[
          {"id":"pack.one","name":"一号","version":"1.2.0","size":10,
           "urls":{"gitee":"https://gitee.com/x/1.zip","github":"https://github.com/x/1.zip"}},
          {"id":"bad id","name":"坏","version":"1.0","urls":{"gitee":"https://x/2.zip"}},
          {"id":"pack.three","name":"三号","version":"1.0.0"},
          {"id":"pack.four","name":"四号","version":"1.0.0","urls":{"gitee":"http://insecure/4.zip"}},
          {"id":"pack.one","name":"一号（重复）","version":"2.0.0","urls":{"gitee":"https://x/5.zip"}},
          {"id":"pack.gitee_only","name":"只发 Gitee","version":"1.0.0","urls":{"gitee":"https://x/6.zip"}}
        ]"#;
        let (packs, skipped) = parse_catalog(text).expect("该解析得动");
        assert_eq!(packs.len(), 2, "双镜像一条 + 只发 Gitee 一条");
        assert_eq!(packs[0].id, "pack.one");
        assert!(
            packs[0].url_for(0).is_some() && packs[0].url_for(1).is_some(),
            "双镜像都收"
        );
        assert_eq!(packs[0].source_mask(), 0b11);
        // ★ 只发一个镜像：另一个来源**不许**拿第一个来源的地址顶着（选了 GitHub 就该置灰）。
        assert_eq!(packs[1].source_mask(), 0b01);
        assert!(packs[1].url_for(1).is_none(), "GitHub 没有地址不能编一个");
        assert_eq!(
            skipped.len(),
            4,
            "坏 id / 没地址 / 非 https / 重复 id 各记一条：{skipped:?}"
        );
        // 全坏 ⇒ 整体失败（空表与解析失败是两回事）
        assert!(parse_catalog(r#"[{"id":"x y"}]"#).is_err());
    }

    /// 详情窗要的字段都在清单里（不是安装后再猜）：作者/许可/发行时间/API/解压体积/文件数。
    #[test]
    fn catalog_carries_the_detail_fields_the_info_window_needs() {
        let text = r#"[{"id":"pack.detail","name":"详情","version":"1.0.0",
          "author":"CurSeen","license":"AGPL-3.0","released":"2026-10-28","api":1,
          "size":10,"unpacked_size":99,"file_count":3,
          "urls":{"gitee":"https://g/x.zip"}}]"#;
        let (packs, _) = parse_catalog(text).unwrap();
        let p = &packs[0];
        assert_eq!(p.author, "CurSeen");
        assert_eq!(p.license, "AGPL-3.0");
        assert_eq!(p.released, "2026-10-28");
        assert_eq!(p.api, 1);
        assert_eq!(p.unpacked_size, 99);
        assert_eq!(p.file_count, 3);
    }

    #[test]
    fn version_compare_is_numeric_and_pads_with_zero() {
        assert!(version_newer("1.10.0", "1.9.9"), "按数字段比，不是字典序");
        assert!(!version_newer("1.2", "1.2.0"), "缺段按 0");
        assert!(version_newer("1.2.1", "1.2"));
        assert!(!version_newer("0.9", "1.0"));
    }

    /// v2 包 = 整个包文件夹替换；同目录下别的脚本（用户脚本/内置音效）一个不许动。
    #[test]
    fn v2_package_replaces_whole_folder_and_leaves_unrelated_files_alone() {
        let root = tmpdir("v2swap");
        let assets = root.join("assets");
        std::fs::create_dir_all(assets.join("scripts")).unwrap();
        std::fs::write(assets.join("scripts/keep.lua"), b"-- unrelated").unwrap();
        std::fs::write(assets.join("scripts/mine.lua"), b"-- user").unwrap();

        let z1 = root.join("v1.zip");
        std::fs::write(
            &z1,
            make_v2_pack(
                "pack.folder",
                "1.0.0",
                &[("a.lua", b"-- a1"), ("b.lua", b"-- b1")],
            ),
        )
        .unwrap();
        let r1 = install_zip(&z1, &assets).expect("v2 包该装得上");
        assert_eq!((r1.added, r1.updated, r1.removed), (6, 0, 0));
        for rel in [
            "scripts/pack.folder/a.lua",
            "scripts/pack.folder/b.lua",
            "scripts/pack.folder/pack.json",
            "scripts/pack.folder/info/LICENSE",
            "scripts/pack.folder/.curseen-pack.json",
        ] {
            assert!(assets.join(rel).is_file(), "缺 {rel}");
        }
        assert!(assets.join("scripts/keep.lua").is_file(), "无关脚本被误删");
        assert!(assets.join("scripts/mine.lua").is_file(), "用户脚本被误删");

        // 升级：b 改名成 c，a 换内容；旧文件夹整体换掉。
        let z2 = root.join("v2.zip");
        std::fs::write(
            &z2,
            make_v2_pack(
                "pack.folder",
                "2.0.0",
                &[("a.lua", b"-- a2"), ("c.lua", b"-- c2")],
            ),
        )
        .unwrap();
        let r2 = install_zip(&z2, &assets).expect("升级该装得上");
        assert_eq!(
            (r2.added, r2.updated, r2.removed),
            (1, 5, 1),
            "新增 c、覆盖 5 个、删掉 b"
        );
        assert_eq!(
            std::fs::read(assets.join("scripts/pack.folder/a.lua")).unwrap(),
            b"-- a2"
        );
        assert!(assets.join("scripts/pack.folder/c.lua").is_file());
        assert!(
            !assets.join("scripts/pack.folder/b.lua").exists(),
            "改名后的旧脚本不能残留"
        );
        let rec = std::fs::read_to_string(assets.join("scripts/pack.folder/.curseen-pack.json"))
            .expect("升级后要写记录");
        assert!(!rec.contains("b.lua"), "记录里不许留旧文件：{rec}");
        assert!(rec.contains("scripts/pack.folder/c.lua"), "{rec}");
        assert!(assets.join("scripts/keep.lua").is_file(), "无关脚本被误删");
        assert!(assets.join("scripts/mine.lua").is_file(), "用户脚本被误删");

        let n = uninstall("pack.folder", &assets).expect("卸载");
        assert_eq!(n, 6);
        assert!(!assets.join("scripts/pack.folder").exists());
        assert!(assets.join("scripts/keep.lua").is_file());
        assert!(assets.join("scripts/mine.lua").is_file());
    }

    /// 目标文件夹已有**不是本包管理**的内容 ⇒ 拒装，且**一个字节都不动**。
    #[test]
    fn refuses_to_overwrite_a_folder_it_does_not_manage() {
        let root = tmpdir("conflict");
        let assets = root.join("assets");
        let occupied = assets.join("scripts/pack.demo");
        std::fs::create_dir_all(&occupied).unwrap();
        std::fs::write(occupied.join("mine.txt"), b"user's own").unwrap();

        let z = root.join("p.zip");
        std::fs::write(
            &z,
            make_v2_pack("pack.demo", "1.0.0", &[("a.lua", b"-- pack")]),
        )
        .unwrap();
        let e = install_zip(&z, &assets).unwrap_err();
        assert!(e.contains("目标目录已存在"), "{e}");
        assert_eq!(
            std::fs::read(occupied.join("mine.txt")).unwrap(),
            b"user's own",
            "冲突时不许改用户目录"
        );
        assert!(
            !occupied.join("a.lua").exists(),
            "拒装必须发生在搬文件夹之前"
        );
        assert!(!occupied.join(RECORD_NAME).exists(), "拒装不许留下记录");
    }

    /// 包缺手续（info/LICENSE/README/HISTORY）⇒ 拒；文件与清单不一致 ⇒ 拒；
    /// 旧 v1 形状（zip 根放 files/）⇒ 直接拒，不做兼容。
    #[test]
    fn refuses_incomplete_inconsistent_or_v1_packs() {
        let root = tmpdir("bad");
        let assets = root.join("assets");
        std::fs::create_dir_all(&assets).unwrap();

        // 缺 info/README.md
        let z1 = root.join("noreadme.zip");
        std::fs::write(
            &z1,
            make_zip(&[
                (
                    "p.one/pack.json",
                    r#"{"id":"p.one","name":"one","version":"1.0","layout":2,"files":["pack.json","info/LICENSE","info/README.md","info/HISTORY.md","a.lua"]}"#
                        .as_bytes(),
                    8,
                ),
                ("p.one/info/LICENSE", b"MIT", 0),
                ("p.one/info/HISTORY.md", b"# h", 0),
                ("p.one/a.lua", b"-- a", 0),
            ]),
        )
        .unwrap();
        let e = install_zip(&z1, &assets).unwrap_err();
        assert!(e.contains("info/README.md"), "{e}");

        // 包内多了一个没登记的文件
        let z2 = root.join("extra.zip");
        std::fs::write(
            &z2,
            make_zip(&[
                (
                    "p.two/pack.json",
                    r#"{"id":"p.two","name":"two","version":"1.0","layout":2,"files":["pack.json","info/LICENSE","info/README.md","info/HISTORY.md","a.lua"]}"#
                        .as_bytes(),
                    8,
                ),
                ("p.two/info/LICENSE", b"MIT", 0),
                ("p.two/info/README.md", b"r", 0),
                ("p.two/info/HISTORY.md", b"# h", 0),
                ("p.two/a.lua", b"-- a", 0),
                ("p.two/rogue.lua", b"-- rogue", 0),
            ]),
        )
        .unwrap();
        let e = install_zip(&z2, &assets).unwrap_err();
        assert!(e.contains("不一致"), "{e}");
        assert!(!assets.join("scripts/p.two").exists(), "拒装不许落盘");

        // 缺 info/HISTORY.md
        let z3 = root.join("nohistory.zip");
        std::fs::write(
            &z3,
            make_zip(&[
                (
                    "p.three/pack.json",
                    r#"{"id":"p.three","name":"three","version":"1.0","layout":2,"files":["pack.json","info/LICENSE","info/README.md","info/HISTORY.md","a.lua"]}"#
                        .as_bytes(),
                    8,
                ),
                ("p.three/info/LICENSE", b"MIT", 0),
                ("p.three/info/README.md", b"r", 0),
                ("p.three/a.lua", b"-- a", 0),
            ]),
        )
        .unwrap();
        let e = install_zip(&z3, &assets).unwrap_err();
        assert!(e.contains("info/HISTORY.md"), "缺 HISTORY 要拒：{e}");

        // 包文件夹名与 pack.json.id 不一致 ⇒ 拒（否则装完会悄悄改名）
        let z5 = root.join("wrongfolder.zip");
        std::fs::write(
            &z5,
            make_zip(&[
                (
                    "wrong/pack.json",
                    r#"{"id":"p.five","name":"five","version":"1.0","layout":2,"files":["pack.json","info/LICENSE","info/README.md","info/HISTORY.md","a.lua"]}"#
                        .as_bytes(),
                    8,
                ),
                ("wrong/info/LICENSE", b"MIT", 0),
                ("wrong/info/README.md", b"r", 0),
                ("wrong/info/HISTORY.md", b"# h", 0),
                ("wrong/a.lua", b"-- a", 0),
            ]),
        )
        .unwrap();
        let e = install_zip(&z5, &assets).unwrap_err();
        assert!(e.contains("文件夹名"), "{e}");

        // 旧 v1 形状：zip 根直接放 pack.json + files/ ⇒ 明确拒收
        let z4 = root.join("v1.zip");
        std::fs::write(
            &z4,
            make_zip(&[
                (
                    "pack.json",
                    r#"{"id":"p.four","name":"four","version":"1.0","files":["scripts/a.lua"]}"#
                        .as_bytes(),
                    8,
                ),
                ("LICENSE", b"MIT", 0),
                ("README.md", b"r", 0),
                ("HISTORY.md", b"# h", 0),
                ("files/scripts/a.lua", b"-- a", 0),
            ]),
        )
        .unwrap();
        let e = install_zip(&z4, &assets).unwrap_err();
        assert!(e.contains("不是 v2 包"), "{e}");
        assert!(!assets.join("scripts/a.lua").exists(), "v1 不许落盘");
    }

    /// 记录里的路径只允许 `scripts/<包>/<文件>`；其它根、`..`、盘符一律拒。
    #[test]
    fn refuses_record_paths_outside_the_package_folder() {
        assert!(safe_record_rel("scripts/pack/a.lua").is_some());
        for bad in [
            "MouseTrail/x.lua",
            "plugins/a.lua",
            "scripts/a.lua",
            "..\\a.lua",
            "../a.lua",
            "scripts/../../a.lua",
            "C:/scripts/a.lua",
            "scripts",
            "scripts/",
            "",
        ] {
            assert!(safe_record_rel(bad).is_none(), "「{bad}」不该被接受");
        }
        // 同一个 `valid_id` 也把着卸载那一头（记录目录与 id 同名）
        let root = tmpdir("badid");
        std::fs::create_dir_all(root.join("victim")).unwrap();
        let e = uninstall("..", &root).unwrap_err();
        assert!(e.contains("不合法"), "{e}");
        assert!(root.join("victim").exists());
    }

    #[test]
    fn record_lives_in_the_pack_dir_and_is_removed_on_uninstall() {
        let root = tmpdir("record");
        let assets = root.join("assets");
        std::fs::create_dir_all(assets.join("scripts")).unwrap();
        let z = root.join("p.zip");
        std::fs::write(&z, make_v2_pack("CursorFX", "1.0.0", &[("a.lua", b"-- a")])).unwrap();
        install_zip(&z, &assets).unwrap();
        let rec = assets.join("scripts/CursorFX").join(RECORD_NAME);
        assert!(rec.is_file(), "记录应在包目录里");
        assert!(
            !assets.join("scripts/.curseen-pack.json").exists(),
            "不许在 scripts 根上留记录"
        );
        // 记录包含 pack.json + info 三份 + a.lua，共 5 个文件。
        assert_eq!(uninstall("CursorFX", &assets).unwrap(), 5);
        assert!(!rec.exists(), "卸载要删记录");
        assert!(!assets.join("scripts/CursorFX").exists(), "空包目录要收掉");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// m43 用户口径：生产代码里不许再出现全局 `lua_packs` 目录。
    #[test]
    fn production_code_has_no_global_pack_folder() {
        for (name, all) in [
            ("store.rs", include_str!("store.rs")),
            ("store_task.rs", include_str!("store_task.rs")),
        ] {
            let cut = all.find("\n#[cfg(test)]").unwrap_or(all.len());
            let src = &all[..cut];
            assert!(
                !src.contains("lua_packs"),
                "{name} 仍在用全局的 lua_packs 目录"
            );
        }
    }
}
