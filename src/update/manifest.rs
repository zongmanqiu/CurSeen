//! 本体更新清单 `update.json`：远端拉取、本地缓存、版本判断。
//!
//! 与商店的 `scripts.json` 同形但不复用它的字段：这里只有 `version / published /
//! notes / sha256 / urls`。清单地址来自 `config.update`，**选中源跟随
//! `config.store.source`**，避免同一台机器维护两套镜像选择。
//!
//! 缓存写在 `assets/docs/update.json`：联网成功就替换它；网络失败时仍可用上次缓存
//! 判断"本地是不是比已知版本旧"。清单本身不决定能不能下载，下载地址还要经过
//! [`is_safe_manifest_url`] / [`is_safe_download_url`] 的官方域名白名单。

use std::path::PathBuf;

use serde::Deserialize;

use super::checker;
use super::version::Version;

/// `assets/docs/update.json` 缓存路径。
pub fn manifest_path() -> PathBuf {
    crate::core::paths::asset_dir("docs").join("update.json")
}

/// 远端清单（字段少而明确，不复制 `scripts.json` 的包字段）。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct UpdateManifest {
    pub version: String,
    #[serde(default)]
    pub published: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub sha256: String,
    pub urls: UpdateUrls,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
pub struct UpdateUrls {
    #[serde(default)]
    pub gitee: String,
    #[serde(default)]
    pub github: String,
}

/// 一次确认可用的更新计划（由后台线程送到 UI）。
///
/// `urls` 顺序 = **先首选源、再备用源**（备用可能为空串）；下载失败可换另一路。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdatePlan {
    pub latest: String,
    pub notes: String,
    pub sha256: String,
    pub urls: [String; 2],
}

/// 检查结果。`latest` 在已是最新时也要带回（状态行显示 `已是最新 x.y.z`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    UpToDate { latest: String },
    Available(UpdatePlan),
}

/// 清单 URL 只允许官方 raw / 仓库域名（配置可改，但恶意第三方地址不生效）。
pub fn is_safe_manifest_url(url: &str) -> bool {
    [
        "https://raw.giteeusercontent.com/qiuzongman/curseen/",
        "https://gitee.com/qiuzongman/curseen/",
        "https://raw.githubusercontent.com/zongmanqiu/CurSeen/",
        "https://github.com/zongmanqiu/CurSeen/",
    ]
    .iter()
    .any(|p| url.starts_with(p))
}

/// zip 下载地址只允许两个官方仓库域名。
pub fn is_safe_download_url(url: &str) -> bool {
    url.starts_with("https://gitee.com/qiuzongman/curseen/")
        || url.starts_with("https://github.com/zongmanqiu/CurSeen/")
}

/// 解析并做最小校验。清单本身不要求 sha256（已是最新时用不到）。
pub fn parse(text: &str) -> Result<UpdateManifest, String> {
    let m: UpdateManifest =
        serde_json::from_str(text).map_err(|e| format!("update.json 不是合法 JSON：{e}"))?;
    if Version::parse(&m.version).is_none() {
        return Err(format!("update.json 的 version 不合法：{}", m.version));
    }
    if m.urls.gitee.is_empty() && m.urls.github.is_empty() {
        return Err("update.json 没有任何 zip 地址".to_string());
    }
    Ok(m)
}

/// 读本地缓存；格式坏了就当没有，不影响启动。
pub fn load_cached() -> Option<UpdateManifest> {
    let text = std::fs::read_to_string(manifest_path()).ok()?;
    match parse(&text) {
        Ok(m) => Some(m),
        Err(e) => {
            log::warn!("本地 update.json 缓存不可用：{e}");
            None
        }
    }
}

/// 把远端原文落到本地缓存；失败只记日志，不能因此让更新检查失败。
fn save_cache(text: &str) {
    let path = manifest_path();
    let Some(parent) = path.parent() else {
        return;
    };
    if let Err(e) = std::fs::create_dir_all(parent) {
        log::warn!("建 update.json 缓存目录失败：{e}");
        return;
    }
    if let Err(e) = std::fs::write(&path, text.as_bytes()) {
        log::warn!("写 update.json 缓存失败：{e}");
    }
}

/// 拉一次清单（阻塞；调用方在后台线程）。
fn fetch(url: &str) -> Result<(UpdateManifest, String), String> {
    if !is_safe_manifest_url(url) {
        return Err(format!("拒绝非官方更新清单地址：{url}"));
    }
    let text = checker::get_text(url)?;
    let manifest = parse(&text)?;
    Ok((manifest, text))
}

/// 把一份清单变成检查结果；`source` 决定先选哪个 zip 地址。
fn outcome_from(
    manifest: &UpdateManifest,
    source: usize,
    current: Version,
) -> Result<Outcome, String> {
    let latest = Version::parse(&manifest.version)
        .ok_or_else(|| format!("update.json 的 version 不合法：{}", manifest.version))?;
    if latest <= current {
        return Ok(Outcome::UpToDate {
            latest: manifest.version.clone(),
        });
    }
    let (first, second) = (source.min(1), 1 - source.min(1));
    let urls = [&manifest.urls.gitee, &manifest.urls.github];
    let candidates: Vec<String> = [urls[first], urls[second]]
        .into_iter()
        .filter(|u| !u.is_empty() && is_safe_download_url(u))
        .cloned()
        .collect();
    let Some(first_url) = candidates.first().cloned() else {
        return Err(format!(
            "update.json 没有可信任的 zip 地址（来源 {}）",
            source
        ));
    };
    let second_url = candidates.get(1).cloned().unwrap_or_default();
    if manifest.sha256.len() != 64 || !manifest.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("update.json 的 sha256 缺失或不是 64 位十六进制".to_string());
    }
    Ok(Outcome::Available(UpdatePlan {
        latest: manifest.version.clone(),
        notes: manifest.notes.clone(),
        sha256: manifest.sha256.to_ascii_lowercase(),
        urls: [first_url, second_url],
    }))
}

/// 检查更新：先试选中源，失败再试另一源；两者都失败时用本地缓存。
///
/// 成功拉到清单就把原文写入 `assets/docs/update.json`，下次离线也能判断。
pub fn check(urls: &[String; 2], source: usize, current: Version) -> Result<Outcome, String> {
    let source = source.min(1);
    let mut last_err = "没有可用的更新清单地址".to_string();
    for idx in [source, 1 - source] {
        let url = &urls[idx];
        if url.is_empty() {
            continue;
        }
        match fetch(url) {
            Ok((manifest, text)) => {
                save_cache(&text);
                return outcome_from(&manifest, source, current);
            }
            Err(e) => {
                log::warn!(
                    "拉取更新清单失败（{}）：{e}",
                    if idx == 0 { "Gitee" } else { "GitHub" }
                );
                last_err = e;
            }
        }
    }
    if let Some(manifest) = load_cached() {
        log::info!("更新清单联网失败，使用本地缓存 {}", manifest.version);
        return outcome_from(&manifest, source, current);
    }
    Err(last_err)
}

/// sha256 十六进制（供下载校验和测试共用）。
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good() -> String {
        serde_json::json!({
            "version": "2.1.0",
            "published": "2026-10-30",
            "notes": "### 新增\n- 更新器改用 update.json",
            "sha256": "a".repeat(64),
            "urls": {
                "gitee": "https://gitee.com/qiuzongman/curseen/releases/download/2.1.0/CurSeen_2.1.0.zip",
                "github": "https://github.com/zongmanqiu/CurSeen/releases/download/2.1.0/CurSeen_2.1.0.zip"
            }
        })
        .to_string()
    }

    #[test]
    fn parses_and_picks_the_selected_source() {
        let m = parse(&good()).unwrap();
        let out = outcome_from(&m, 1, Version::parse("2.0.5").unwrap()).unwrap();
        let Outcome::Available(p) = out else {
            panic!("应发现新版");
        };
        assert_eq!(p.latest, "2.1.0");
        assert!(p.urls[0].starts_with("https://github.com/"), "首选源排第一");
        assert!(
            p.urls[1].starts_with("https://gitee.com/"),
            "另一路必须作为备用地址一起带回"
        );
        assert_eq!(p.sha256.len(), 64);

        // 换成 Gitee 首选，顺序也要跟着换。
        let Outcome::Available(p) = outcome_from(&m, 0, Version::parse("2.0.5").unwrap()).unwrap()
        else {
            panic!("应发现新版");
        };
        assert!(p.urls[0].starts_with("https://gitee.com/"));
        assert!(p.urls[1].starts_with("https://github.com/"));
    }

    #[test]
    fn falls_back_to_the_other_zip_url() {
        let mut m = parse(&good()).unwrap();
        m.urls.github.clear();
        let Outcome::Available(p) = outcome_from(&m, 1, Version::parse("2.0.5").unwrap()).unwrap()
        else {
            panic!("应回退 Gitee");
        };
        assert!(p.urls[0].starts_with("https://gitee.com/"));
        assert_eq!(p.urls[1], "", "没有第二路时备用地址留空");
    }

    #[test]
    fn same_or_older_is_up_to_date() {
        let m = parse(&good()).unwrap();
        assert_eq!(
            outcome_from(&m, 0, Version::parse("2.1.0").unwrap()).unwrap(),
            Outcome::UpToDate {
                latest: "2.1.0".into()
            }
        );
        assert_eq!(
            outcome_from(&m, 0, Version::parse("9.0.0").unwrap()).unwrap(),
            Outcome::UpToDate {
                latest: "2.1.0".into()
            }
        );
    }

    #[test]
    fn rejects_third_party_or_unhashed_manifests() {
        let mut v: serde_json::Value = serde_json::from_str(&good()).unwrap();
        v["urls"]["gitee"] = serde_json::json!("https://evil.example/x.zip");
        v["urls"]["github"] = serde_json::json!("https://evil.example/x.zip");
        let m: UpdateManifest = serde_json::from_value(v).unwrap();
        assert!(outcome_from(&m, 0, Version::parse("2.0.5").unwrap()).is_err());

        let mut v: serde_json::Value = serde_json::from_str(&good()).unwrap();
        v["sha256"] = serde_json::json!("not-a-hash");
        let m: UpdateManifest = serde_json::from_value(v).unwrap();
        assert!(outcome_from(&m, 0, Version::parse("2.0.5").unwrap()).is_err());
    }

    #[test]
    fn official_hosts_only() {
        assert!(is_safe_manifest_url(
            "https://raw.giteeusercontent.com/qiuzongman/curseen/raw/main/update.json"
        ));
        assert!(is_safe_manifest_url(
            "https://raw.githubusercontent.com/zongmanqiu/CurSeen/refs/heads/main/update.json"
        ));
        assert!(!is_safe_manifest_url("https://example.com/update.json"));
        assert!(!is_safe_download_url("https://example.com/CurSeen.zip"));
    }

    #[test]
    fn sha256_matches_a_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
