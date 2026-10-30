//! 下载更新包并落到中转文件。对齐 Python `update/downloader.py`（那里用
//! `QNetworkAccessManager` + `downloadProgress` 信号；这里换成阻塞读取 + 进度回调）。
//!
//! ★ 分块读写而不是 `read_to_vec()`：更新包典型 1~2MB，但要留出进度反馈，且避免把
//! 整个响应体先堆在内存里再落盘（内存指标是硬约束）。

use std::path::{Path, PathBuf};

use super::checker::{MAX_BODY, USER_AGENT};

/// 更新包落地路径：`<程序目录>/update/curseen_update.zip`。
///
/// 不用 `%TEMP%`：用户要求"运行程序产生的文件留在程序目录"。放在 `update/` 子目录而不是
/// 摊在根目录，替换完成后由更新脚本把整个目录删掉。
pub fn update_zip_path() -> PathBuf {
    crate::core::paths::update_dir().join("curseen_update.zip")
}

/// 计算已下载 zip 的 sha256（小写十六进制）；文件读不了时报中文原因。
///
/// 更新包通常只有几 MB；读进内存一次即可，后续若变成几十 MB 再改流式。
pub fn file_sha256(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("读更新包失败：{e}"))?;
    Ok(super::manifest::sha256_hex(&bytes))
}

/// 把 `url` 下载到 `dest`。`on_progress(已收字节, 总字节)` 在每块之后回调一次；
/// 服务端未给 `Content-Length` 时 `total` 为 0（调用方据此显示"未知总量"）。
///
/// 阻塞函数：调用方负责放到后台线程（见 `app::update`）。
pub fn download_to_file<F>(url: &str, dest: &Path, on_progress: F) -> Result<(), String>
where
    F: FnMut(u64, u64),
{
    // 只允许从官方 Gitee 域名拉更新包：挡掉仓库被控时 API 可能回填的
    // `file://` / 内网地址等恶意 `browser_download_url`（ureq 默认跟重定向、且支持
    // `file:` 协议，SSRF 面不能裸开）。**这道策略只在更新这条路上有**——
    // 脚本的 `ext:download` 是用户自己的代码，不套它（信任模型见 API.md）。
    if !super::checker::is_safe_update_url(url) {
        return Err(format!("拒绝非信任更新地址: {url}"));
    }
    // 流式下载本体在平台层（全仓唯一一份，m32 搬过去的）：分块写 `.partial` →
    // 校验 Content-Length → 原子改名；无全局超时（慢网络不能半路被掐）。
    crate::platform::http::download_to_file(
        url,
        dest,
        &[("User-Agent".to_string(), USER_AGENT.to_string())],
        MAX_BODY,
        on_progress,
    )
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zip_path_is_portable_update_dir() {
        let p = update_zip_path();
        assert!(p.is_absolute(), "程序目录应是绝对路径: {p:?}");
        assert_eq!(
            p.file_name().and_then(|s| s.to_str()),
            Some("curseen_update.zip")
        );
        assert_eq!(p.parent(), Some(crate::core::paths::update_dir().as_path()));
        // 便携约束：不落到 %TEMP%，也不落到 %APPDATA%。
        assert_eq!(
            p.parent().and_then(|d| d.parent()),
            Some(crate::core::paths::app_dir().as_path())
        );
    }

    /// ★ 回归（2026-10-08）：下载必须走**无全局超时**的构造路。
    /// 全局超时覆盖整个响应体读取，任何时限都会掐到慢网络用户（10s 版实测半路
    /// 「timeout: global」；600s 版同样存在被掐面，2026-10-08 用户拍板取消总时限）。
    #[test]
    fn unbounded_agent_has_no_global_timeout() {
        use std::time::Duration;
        let a = crate::platform::http::agent_unbounded();
        assert!(
            a.config().timeouts().global.is_none(),
            "下载 Agent 不得带全局超时（会掐到慢网络用户）"
        );
        // 对照组：限时构造的全局超时应原样生效。
        assert_eq!(
            crate::platform::http::agent(Duration::from_secs(7))
                .config()
                .timeouts()
                .global,
            Some(Duration::from_secs(7)),
            "限时 agent() 的语义不应被连带改动"
        );
    }
}
