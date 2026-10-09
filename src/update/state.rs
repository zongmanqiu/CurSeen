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

/// 面板可见的更新状态 + 更新流程的机器状态。
#[derive(Clone)]
pub struct UpdateState {
    /// 状态行文本（"正在检查…" / "已是最新 2.0.0" / 错误信息）。
    pub status: Signal<String>,
    /// 下载进度 0..=1；`-1` 表示未知总量或无进度。
    pub progress: Signal<f32>,
    /// 检查或下载进行中：按钮置灰，避免并发起两次。
    pub busy: Signal<bool>,
    /// 已拿到下载地址，可以"下载并更新"。
    pub ready: Signal<bool>,
    /// 待下载的地址（由 `Available` 事件写入，`start_download` 读取）。
    /// ★ 用 `Signal`：面板 clone 的是同一个单元格，Runtime 之后 `set_url` 写入的值
    ///   能被按钮闭包实时读到（否则 `up.clone()` 拷的是建树时的空串，点下去永远是空）。
    url: Signal<String>,
}

impl Default for UpdateState {
    fn default() -> Self {
        Self::new()
    }
}

impl UpdateState {
    /// 在 UI 线程创建（`Signal` 是 `!Send`，只能在这儿建）。
    pub fn new() -> Self {
        Self {
            status: signal("尚未检查更新".to_string()),
            progress: signal(-1.0f32),
            busy: signal(false),
            ready: signal(false),
            url: signal(String::new()),
        }
    }

    /// 待下载地址。没有时为 `""`（调用方据此判定"还不能被点"）。
    /// 经 `Signal` 读：Runtime 之后写入的值，按钮闭包此刻能实时取到。
    pub fn url(&self) -> String {
        self.url.get()
    }

    /// 记录下载地址并放行"下载并更新"按钮（`Runtime::apply_update_event` 在
    /// `UpdateEvent::Available` 时调用）。
    pub fn set_url(&mut self, url: impl Into<String>) {
        self.url.set(url.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_state_has_no_url_and_is_not_ready() {
        let s = UpdateState::new();
        assert_eq!(s.url(), "", "初始状态不该有下载地址");
        assert!(!s.ready.get());
        assert!(!s.busy.get());
        assert_eq!(s.progress.get(), -1.0);
    }

    #[test]
    fn set_url_is_read_back() {
        let mut s = UpdateState::new();
        s.set_url("https://example.com/a.zip");
        assert_eq!(s.url(), "https://example.com/a.zip");
    }
}
