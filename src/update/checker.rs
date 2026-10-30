//! 检查 Gitee 上的最新 Release。对齐 Python `update/checker.py`。
//!
//! 分成两半：**挑选逻辑是纯函数**（[`pick_latest`]，可单测、不碰网络），**取数**在这里
//! 只做一次阻塞 GET（[`check`]），由 `app::update` 放到后台线程调用。

use serde::Deserialize;

use super::version::Version;

/// Gitee OpenAPI：某仓库的 Release 列表（按发布时间倒序）。
pub const GITEE_API: &str = "https://gitee.com/api/v5/repos/qiuzongman/curseen/releases";
/// 附件下载地址前缀（Release 里没挂 zip 资产时按约定拼）。
pub const RELEASE_BASE: &str = "https://gitee.com/qiuzongman/curseen/releases/download";
/// Gitee 对空 UA 会拒绝，带一个明确的 UA（与 Python 同款）。
pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) CurSeen/2.0";
/// 响应体上限：1.0.0 更新包实测 27MB（2026-10-08），给到 64MB 足够且仍有界。
/// ★ 下载走的超时在 `downloader::DOWNLOAD_TIMEOUT_SECS`（10 分钟），别与这里的 10s 混用。
pub(crate) const MAX_BODY: u64 = 64 * 1024 * 1024;
/// 单次请求超时。
const TIMEOUT_SECS: u64 = 10;

/// 一个 Release（只留用得到的字段）。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Release {
    #[serde(default)]
    pub tag_name: String,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

/// Release 附件。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Asset {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub browser_download_url: String,
}

/// 检查结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    /// 有新版本，且拿到了 zip 地址。
    Available { latest: String, url: String },
    /// 有新版本，但既没有 zip 资产、也拼不出约定地址。
    AvailableNoAsset { latest: String },
    /// 已是最新。
    UpToDate { latest: String },
}

/// 从 Release 列表里挑最新版并定位 zip。纯函数，Python 同款策略：
///
/// 1. 逐个 tag 解析版本号，取最大者；
/// 2. 在该 Release 的 assets 里找**第一个** `*.zip` 且名字含 `CurSeen` 的；
/// 3. 找不到就按约定拼 `{RELEASE_BASE}/{tag}/CurSeen_{版本}.zip`。
pub fn pick_latest(releases: &[Release], current: Version) -> CheckOutcome {
    let mut best: Option<(Version, &Release)> = None;
    for r in releases {
        let Some(v) = Version::parse(&r.tag_name) else {
            continue;
        };
        if best.is_none_or(|(bv, _)| v > bv) {
            best = Some((v, r));
        }
    }
    let Some((latest, release)) = best else {
        // 一个 tag 都解析不出来：当作"没有更新"，避免把用户吓一跳。
        return CheckOutcome::UpToDate {
            latest: current.to_string(),
        };
    };

    if latest <= current {
        return CheckOutcome::UpToDate {
            latest: latest.to_string(),
        };
    }

    let tag = &release.tag_name;
    let asset = release
        .assets
        .iter()
        .find(|a| a.name.ends_with(".zip") && a.name.contains("CurSeen"))
        .map(|a| a.browser_download_url.clone())
        .filter(|u| !u.is_empty());

    match asset.or_else(|| fallback_url(tag, latest)) {
        Some(url) => CheckOutcome::Available {
            latest: latest.to_string(),
            url,
        },
        None => CheckOutcome::AvailableNoAsset {
            latest: latest.to_string(),
        },
    }
}

/// 约定地址：`.../download/{tag}/CurSeen_{major.minor.patch}.zip`。
fn fallback_url(tag: &str, v: Version) -> Option<String> {
    if tag.is_empty() {
        return None;
    }
    Some(format!("{RELEASE_BASE}/{tag}/CurSeen_{v}.zip"))
}

/// 更新包只允许来自两个官方仓库（Gitee / GitHub）。
///
/// 理由：`browser_download_url` 来自 Release API / `update.json`，仓库若被控可被回填
/// 任意地址；`ureq` 默认跟随重定向、且支持 `file:` 协议，裸放开就是 SSRF 面。这里在
/// **下载前**卡一道前缀白名单。`update.json` 的来源由 `manifest` 模块另行校验。
pub(crate) fn is_safe_update_url(url: &str) -> bool {
    super::manifest::is_safe_download_url(url)
}

/// 联网取 Release 列表。
pub fn fetch_releases() -> Result<Vec<Release>, String> {
    let text = get_text(GITEE_API)?;
    // Gitee 正常返回数组；异常时（错误对象）解不出数组，按失败处理。
    serde_json::from_str::<Vec<Release>>(&text).map_err(|e| format!("解析 Release 列表失败: {e}"))
}

/// 检查更新（阻塞，调用方负责放后台线程）。
pub fn check(current: Version) -> Result<CheckOutcome, String> {
    let releases = fetch_releases()?;
    Ok(pick_latest(&releases, current))
}

/// 下载一个 URL 到内存。
pub fn download(url: &str) -> Result<Vec<u8>, String> {
    if !is_safe_update_url(url) {
        return Err(format!("拒绝非信任更新地址: {url}"));
    }
    let agent = agent();
    let mut resp = agent
        .get(url)
        .header("User-Agent", USER_AGENT)
        .call()
        .map_err(|e| format!("下载失败: {e}"))?;
    resp.body_mut()
        .with_config()
        .limit(MAX_BODY)
        .read_to_vec()
        .map_err(|e| format!("读取响应失败: {e}"))
}

pub(crate) fn get_text(url: &str) -> Result<String, String> {
    let agent = agent();
    let mut resp = agent
        .get(url)
        .header("User-Agent", USER_AGENT)
        .call()
        .map_err(|e| format!("请求失败: {e}"))?;
    resp.body_mut()
        .with_config()
        .limit(MAX_BODY)
        .lossy_utf8(true)
        .read_to_string()
        .map_err(|e| format!("读取响应失败: {e}"))
}

/// 共享的 HTTP 客户端（超时统一；下载器复用同一份配置）。
///
/// 构造本身委托给 [`crate::platform::http::agent`]——TLS provider 只能在那一处配，
/// 见该函数的文档（自配一个 Agent 会让 HTTPS 在运行期 panic）。
pub(crate) fn agent() -> ureq::Agent {
    crate::platform::http::agent(std::time::Duration::from_secs(TIMEOUT_SECS))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(tag: &str, assets: &[(&str, &str)]) -> Release {
        Release {
            tag_name: tag.to_string(),
            assets: assets
                .iter()
                .map(|(n, u)| Asset {
                    name: (*n).to_string(),
                    browser_download_url: (*u).to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn picks_highest_version_not_first() {
        let rs = vec![
            rel("v1.0.0", &[("CurSeen_1.0.0.zip", "https://a/1.0.0.zip")]),
            rel("v1.2.0", &[("CurSeen_1.2.0.zip", "https://a/1.2.0.zip")]),
            rel("v1.10.0", &[("CurSeen_1.10.0.zip", "https://a/1.10.0.zip")]),
        ];
        assert_eq!(
            pick_latest(&rs, Version::parse("1.0.0").expect("可解析")),
            CheckOutcome::Available {
                latest: "1.10.0".into(),
                url: "https://a/1.10.0.zip".into()
            }
        );
    }

    #[test]
    fn up_to_date_when_not_newer() {
        let rs = vec![rel(
            "v1.0.0",
            &[("CurSeen_1.0.0.zip", "https://a/1.0.0.zip")],
        )];
        assert_eq!(
            pick_latest(&rs, Version::parse("1.0.0").expect("可解析")),
            CheckOutcome::UpToDate {
                latest: "1.0.0".into()
            }
        );
        assert_eq!(
            pick_latest(&rs, Version::parse("2.0.0").expect("可解析")),
            CheckOutcome::UpToDate {
                latest: "1.0.0".into()
            }
        );
    }

    #[test]
    fn ignores_zip_assets_that_are_not_curseen() {
        // 名字不含 CurSeen 的 zip 不认（Gitee 会自动附带源码包）。
        let rs = vec![rel(
            "v9.9.9",
            &[
                ("source.zip", "https://a/source.zip"),
                ("CurSeen_9.9.9.zip", "https://a/ok.zip"),
            ],
        )];
        assert_eq!(
            pick_latest(&rs, Version::ZERO),
            CheckOutcome::Available {
                latest: "9.9.9".into(),
                url: "https://a/ok.zip".into()
            }
        );
    }

    #[test]
    fn falls_back_to_conventional_url() {
        let rs = vec![rel("v1.5.0", &[])];
        assert_eq!(
            pick_latest(&rs, Version::parse("1.0.0").expect("可解析")),
            CheckOutcome::Available {
                latest: "1.5.0".into(),
                url: format!("{RELEASE_BASE}/v1.5.0/CurSeen_1.5.0.zip")
            }
        );
    }

    #[test]
    fn empty_or_unparsable_release_list_is_not_an_update() {
        let empty: Vec<Release> = vec![];
        assert_eq!(
            pick_latest(&empty, Version::current()),
            CheckOutcome::UpToDate {
                latest: Version::current().to_string()
            }
        );
        let junk = vec![rel("nightly", &[])];
        assert!(matches!(
            pick_latest(&junk, Version::current()),
            CheckOutcome::UpToDate { .. }
        ));
    }
}
