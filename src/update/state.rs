//! 更新流程的 UI 可见状态。
//!
//! `Signal` 是 windui 的类型（`core` 禁用 windui），所以这份状态不能直接进 `core`
//! ——但它在语义上属于 `update` 子系统（谁更新谁管线持证），于是放在这里。
//!
//! # 为什么随 `Runtime` 常驻而不是随面板开合新建
//!
//! 更新是**后台发起、可能在面板关着的时候完成**的动作。若状态跟着面板走，用户在面板
//! 打开之前点过一次"检查更新"，结果就没人接、也就无处显示。让它常驻，面板重新打开时
//! 读到的是最新状态而不是初始值。

use windui::prelude::*;

/// 面板可见的更新状态。
#[derive(Clone)]
pub struct UpdateState {
    /// 状态行文本（"正在检查…" / "已是最新 2.0.0" / 错误信息）。
    pub status: Signal<String>,
    /// 下载进度 0..=1；`-1` 表示未知总量、尚未开始下载或下载失败。
    ///
    /// About 页用它做**按钮 ↔ 进度条**的切换：`>= 0` 就是"下载已开始"。
    pub progress: Signal<f32>,
    /// 检查或下载进行中：按钮置灰，避免并发起两次。
    pub busy: Signal<bool>,
    /// 下载已开始（含"还没收到第一块数据"与"服务端没给长度"两种阶段）。
    ///
    /// About 页用它做**按钮 ↔ 进度条**的切换；`progress` 单独不能担此任——
    /// 服务端不给 `Content-Length` 时进度会一直是 `-1`（不确定态）。
    pub downloading: Signal<bool>,
}

impl Default for UpdateState {
    fn default() -> Self {
        Self::new()
    }
}

impl UpdateState {
    /// 在 UI 线程创建（`Signal` 是 `!Send`，只能在这儿建）。
    pub fn new() -> Self {
        // 本地清单是"离线也能知道自己旧不旧"的底账：启动时先给一句人话，
        // 点「检查更新」再联网替换它。
        let cached =
            super::manifest::load_cached().and_then(|m| super::version::Version::parse(&m.version));
        Self {
            status: signal(initial_status(cached)),
            progress: signal(-1.0f32),
            busy: signal(false),
            downloading: signal(false),
        }
    }
}

/// 启动时状态行的文案（纯函数，便于钉住"旧缓存不冒充最新"）。
///
/// - 缓存比当前新 ⇒ 可更新；
/// - 相等 ⇒ 已是最新；
/// - **缓存比当前旧 / 没有缓存 ⇒ 尚未检查**——升级后安装目录里的缓存还是上一版，
///   这时说"已是最新版本 2.0.5"是假话（真机踩过）。
fn initial_status(cached: Option<super::version::Version>) -> String {
    let current = super::version::Version::current();
    match cached {
        Some(v) if v > current => format!("本地清单显示 {v} 可更新"),
        Some(v) if v == current => format!("已是最新版本 {v}"),
        _ => "尚未检查更新".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_state_is_idle_and_has_no_progress() {
        let s = UpdateState::new();
        assert!(!s.busy.get());
        assert!(!s.downloading.get());
        assert_eq!(s.progress.get(), -1.0);
        assert!(!s.status.get().is_empty(), "启动时也该有一句人话");
    }

    /// ★ 回归（2026-10-31 真机发现）：升级后缓存仍是上一版，不能因此说"已是最新"。
    #[test]
    fn stale_cache_is_not_reported_as_up_to_date() {
        let current = crate::update::version::Version::current();
        assert_eq!(
            initial_status(Some(current)),
            format!("已是最新版本 {current}")
        );
        assert_eq!(initial_status(None), "尚未检查更新");

        let older = crate::update::version::Version::parse("0.0.1").expect("可解析");
        assert_eq!(initial_status(Some(older)), "尚未检查更新");

        let newer = crate::update::version::Version::parse("99.0.0").expect("可解析");
        assert!(
            initial_status(Some(newer)).contains("可更新"),
            "缓存比当前新时必须提示可更新"
        );
    }
}
