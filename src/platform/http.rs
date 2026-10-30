//! 阻塞 HTTP（worker 线程专用）。
//!
//! 本模块同时是全仓**唯一的 `ureq::Agent` 构造点**：AI 对话窗（[`crate::platform::ai_chat`]）
//! 的 `post_json`、`ext:http`（`scripting::api_ext`）与自动更新（`update::checker`）
//! 三条链路都从这里取 Agent，任何新增的 HTTP 调用也必须走 [`agent`]——
//! 理由见该函数文档（漏出一个自带配置的 Agent 就是一个 HTTPS 运行期 panic）。
//! 依赖方向仍是单项的：平台层不得反向引用脚本层（core←platform←scripting）。
//!
//! 阻塞上限由调用方给定（[`agent_unbounded`] 例外：下载不限总时长）；必须在**非 UI 线程**调用。
//!
//! - AI 对话 `/chat/completions`：`post_json` 发 POST、读全文，把 `(status, body)`
//!   交回窗口线程做 UI。
//! - 其余 GET/PUT/DELETE：`scripting::api_ext::http_blocking`（Lua 错误语义）
//!   与 `update/`（版本检查 / 下载）各自用自己的超时参数调 [`agent`]。

use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

/// 构造一个 HTTP 客户端（带全局超时）。**全仓唯一的构造点。**
///
/// ★★ `provider` 与 `root_certs` 两个都必须显式写出，缺一个就出事：
///
/// - **`provider`**：`ureq` 的默认 provider 是 `Rustls`（`#[default]`）。只要有人动了
///   `Cargo.toml` 里 `ureq` 的 features 却没同步这里，结果**不是编译报错，而是
///   运行期 panic** —— `provider is X but feature is not enabled: X`
///   （2026-10-08 实测过，详见 `Cargo.toml` 的 `ureq` 注释）。
/// - **`root_certs`**：不指定会落到默认的 `RootCerts::WebPki`，它会
///   `disable_built_in_roots(true)`，**丢掉 Windows 系统根证书库**、改用 ureq 打包的
///   根；实测那样配 6 个站点有 2 个握手失败。必须指定 `PlatformVerifier` 才走真正的
///   系统信任链。
///
/// ⇒ 所以 Agent 集中在这一处构造，各处不得自配。
pub fn agent(global_timeout: Duration) -> ureq::Agent {
    agent_impl(Some(global_timeout))
}

/// 无**全局**超时的变体：下载更新包专用（2026-10-08 用户拍板）。
///
/// 全局超时覆盖**整个响应体读取**——更新包 27MB，网络不好的用户要下好几分钟，
/// 任何总时限都是在掐他们（10s 版实测「下载到一半 timeout: global」，600s 版仍有
/// 被掐的理论面）。下载宁可一直等，也不能半路失败。连接建立仍受 ureq 默认
/// 连接超时约束（连不上的服务器会较快报错），慢但活着的传输则不限时。
pub fn agent_unbounded() -> ureq::Agent {
    agent_impl(None)
}

fn agent_impl(global_timeout: Option<Duration>) -> ureq::Agent {
    ureq::Agent::config_builder()
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .timeout_global(global_timeout)
        .build()
        .into()
}

/// POST JSON 并读取全部响应体。
///
/// 返回 `(HTTP 状态码, body)`；网络层失败（连接不上、超时等）状态码为 0、
/// 错误信息放进 body——调用方（AI 对话窗口）统一按文本展示，不再细分错误类型。
pub fn post_json(url: &str, headers: &[(String, String)], body: &str) -> (i64, String) {
    let agent = agent(Duration::from_secs(60));
    let mut req = agent.post(url).header("Content-Type", "application/json");
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }
    match req.send(body.to_string()) {
        Ok(resp) => {
            let status = resp.status().as_u16() as i64;
            let text = resp.into_body().read_to_string().unwrap_or_default();
            (status, text)
        }
        Err(e) => (0, format!("请求失败：{e}")),
    }
}

/// 流式下载到文件：先写 `<dest>.partial`，成功后再**原子改名**；任何中途失败都删掉半成品。
/// 返回落盘字节数。
///
/// 这一份是**全仓唯一的流式下载实现**（m32 从 `update/downloader` 搬来并参数化）：
/// 更新包与脚本的 `ext:download` 都走它——分块读写（不把整包堆进内存）、
/// `Content-Length` 校验、大小上限、失败清理，这些纪律只写一遍。
///
/// 三个参数的来源要说清：
/// · `headers`：调用方给（脚本可带 `Authorization` 之类）；
/// · `max_bytes`：调用方给（更新包用 64 MB，脚本用 [`SCRIPT_DOWNLOAD_MAX`]）；
/// · 超时：**不限总时长**（[`agent_unbounded`]）——全局超时覆盖整个响应体读取，
///   慢网络会半路被掐；连接建立仍受 ureq 默认连接超时约束。
pub fn download_to_file<F: FnMut(u64, u64)>(
    url: &str,
    dest: &Path,
    headers: &[(String, String)],
    max_bytes: u64,
    mut on_progress: F,
) -> Result<u64, String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建下载目录失败: {e}"))?;
    }
    let partial = dest.with_extension("partial");
    let _ = std::fs::remove_file(&partial);

    let mut req = agent_unbounded().get(url);
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }
    // 这里只给"连接这一层"的上下文；"下载失败"的外层前缀由调用方加，避免出现
    // 「下载失败：下载失败: …」这种重复（更新页与商店各自有自己的动作名）。
    let mut resp = req.call().map_err(|e| format!("连接失败: {e}"))?;
    // `content_length()` 在分块传输/gzip 下可能为 None，那时退化成"未知总量"。
    let total = resp.body().content_length().unwrap_or(0);
    let mut reader = resp.body_mut().with_config().limit(max_bytes).reader();
    let mut file = std::fs::File::create(&partial).map_err(|e| format!("创建临时文件失败: {e}"))?;
    let mut buf = vec![0u8; CHUNK];
    let mut received = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| {
            let _ = std::fs::remove_file(&partial);
            format!("读取响应失败: {e}")
        })?;
        if n == 0 {
            break;
        }
        if let Err(e) = file.write_all(&buf[..n]) {
            let _ = std::fs::remove_file(&partial);
            return Err(format!("写入临时文件失败: {e}"));
        }
        received += n as u64;
        on_progress(received, total);
    }
    if let Err(e) = file.flush() {
        let _ = std::fs::remove_file(&partial);
        return Err(format!("写入临时文件失败: {e}"));
    }
    drop(file);
    if received == 0 {
        let _ = std::fs::remove_file(&partial);
        return Err("下载内容为空".to_string());
    }
    // 服务端给了 Content-Length 就校验：对不上说明传输被截断。
    if total != 0 && received != total {
        let _ = std::fs::remove_file(&partial);
        return Err(format!(
            "下载不完整: 收到 {received} 字节 / 期望 {total} 字节"
        ));
    }
    std::fs::rename(&partial, dest).map_err(|e| {
        let _ = std::fs::remove_file(&partial);
        format!("保存下载文件失败: {e}")
    })?;
    Ok(received)
}

/// 取一段文本响应（GET，带 UA）。上限 `max_bytes`；非 2xx 或网络失败给原因。
///
/// 只给"小文本"用（脚本清单、版本号这类）；大文件走 [`download_to_file`]。
///
/// ★ **必须是带总时限的 agent**：清单拉取占用 `store_task` 的 `BusyGuard`，无总超时的
///   agent 遇上"连上了但一直不回正文"的服务器，会让在线脚本库永远停在"正在获取清单…"
///   （按钮全灰、也取消不了）。30 s 对一张几十 KB 的清单足够。
///   `update::checker::get_text` 是刻意保留的同形实现（那边 10 s、且下载口另有域名白名单）。
pub fn get_text(url: &str, max_bytes: u64) -> Result<String, String> {
    let mut resp = agent(std::time::Duration::from_secs(30))
        .get(url)
        .header("User-Agent", DEFAULT_UA)
        .call()
        .map_err(|e| format!("请求失败: {e}"))?;
    resp.body_mut()
        .with_config()
        .limit(max_bytes)
        .lossy_utf8(true)
        .read_to_string()
        .map_err(|e| format!("读取响应失败: {e}"))
}

/// 每次读的块大小（64KB：够大以减少系统调用，又不至于让进度显示卡顿）。
const CHUNK: usize = 64 * 1024;

/// 脚本下载的默认上限（`ext:download`）：512 MiB。
///
/// 面板自动更新用的是 `update::checker::MAX_BODY`（64 MB，更新包典型 1–2MB）；
/// 脚本要下的是"zip 素材包"这类东西，给一个更大的、但**仍然有头**的数——
/// 没头的下载在一个常驻小工具里是失控。
pub const SCRIPT_DOWNLOAD_MAX: u64 = 512 * 1024 * 1024;

/// 默认 UA：与更新包同一串（有些站点直接拒绝 ureq 的默认 UA）。
pub const DEFAULT_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) CurSeen/2.0";
