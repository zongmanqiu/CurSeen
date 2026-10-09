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
