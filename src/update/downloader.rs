//! 下载更新包并落到中转文件。对齐 Python `update/downloader.py`（那里用
//! `QNetworkAccessManager` + `downloadProgress` 信号；这里换成阻塞读取 + 进度回调）。
//!
//! ★ 分块读写而不是 `read_to_vec()`：更新包典型 1~2MB，但要留出进度反馈，且避免把
//! 整个响应体先堆在内存里再落盘（内存指标是硬约束）。

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use super::checker::{MAX_BODY, USER_AGENT};

/// 每次读的块大小（64KB：够大以减少系统调用，又不至于让进度显示卡顿）。
const CHUNK: usize = 64 * 1024;

/// 更新包落地路径：`<程序目录>/update/curseen_update.zip`。
///
/// 不用 `%TEMP%`：用户要求"运行程序产生的文件留在程序目录"。放在 `update/` 子目录而不是
/// 摊在根目录，替换完成后由更新脚本把整个目录删掉。
pub fn update_zip_path() -> PathBuf {
    crate::core::paths::update_dir().join("curseen_update.zip")
}

/// 把 `url` 下载到 `dest`。`on_progress(已收字节, 总字节)` 在每块之后回调一次；
/// 服务端未给 `Content-Length` 时 `total` 为 0（调用方据此显示"未知总量"）。
///
/// 阻塞函数：调用方负责放到后台线程（见 `app::update`）。
pub fn download_to_file<F>(url: &str, dest: &Path, mut on_progress: F) -> Result<(), String>
where
    F: FnMut(u64, u64),
{
    // 只允许从官方 Gitee 域名拉更新包：挡掉仓库被控时 API 可能回填的
    // `file://` / 内网地址等恶意 `browser_download_url`（ureq 默认跟重定向、且支持
    // `file:` 协议，SSRF 面不能裸开）。
    if !super::checker::is_safe_update_url(url) {
        return Err(format!("拒绝非信任更新地址: {url}"));
    }

    // 目标目录（`<程序目录>/update`）平时不存在，首次下载时建。
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建更新目录失败: {e}"))?;
    }

    // 先写 `.partial` 中转文件，成功后再原子改名覆盖 `dest`；任何中途失败都清理半成品，
    // 避免留下一个截断/损坏的 zip 被下次启动误当成品。
    let partial = dest.with_extension("partial");
    let _ = std::fs::remove_file(&partial);

    // ★ 无全局超时（[`crate::platform::http::agent_unbounded`]）：全局超时覆盖整个
    //   响应体读取，10s 版实测 27MB 半路被掐（「timeout: global」），600s 版仍会掐到
    //   慢网络用户（2026-10-08 用户拍板：下载不限总时长，宁可一直等也不能半路失败）。
    //   不能复用 checker 的 `agent()`（10s 全局超时，为 KB 级 Release API 请求配的）。
    let mut resp = crate::platform::http::agent_unbounded()
        .get(url)
        .header("User-Agent", USER_AGENT)
        .call()
        .map_err(|e| format!("下载失败: {e}"))?;

    // `content_length()` 在分块传输/gzip 下可能为 None，那时退化成"未知总量"。
    let total = resp.body().content_length().unwrap_or(0);

    let mut reader = resp.body_mut().with_config().limit(MAX_BODY).reader();
    let mut file = File::create(&partial).map_err(|e| format!("创建临时文件失败: {e}"))?;

    let mut buf = vec![0u8; CHUNK];
    let mut received = 0u64;
    loop {
        // 无全局超时：慢网络下这里的读取会一直进行到下载完成，不做任何时限。
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("读取响应失败: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|e| format!("写入临时文件失败: {e}"))?;
        received += n as u64;
        on_progress(received, total);
    }

    // 必须显式 flush：`File` 的 `Drop` 会吞掉错误，落盘失败要能报出来。
    file.flush().map_err(|e| format!("写入临时文件失败: {e}"))?;
    // 改名前先关文件句柄，否则 Windows 上 `rename` 会因文件被占用失败。
    drop(file);

    if received == 0 {
        let _ = std::fs::remove_file(&partial);
        return Err("下载内容为空".to_string());
    }
    // 服务端给了 Content-Length 就校验：对不上说明传输被截断或内容被篡改。
    if total != 0 && received != total {
        let _ = std::fs::remove_file(&partial);
        return Err(format!(
            "下载不完整: 收到 {received} 字节 / 期望 {total} 字节"
        ));
    }

    std::fs::rename(&partial, dest).map_err(|e| format!("保存更新包失败: {e}"))?;
    Ok(())
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
