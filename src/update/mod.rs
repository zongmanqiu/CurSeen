//! 自更新：版本比较 → 拉 `update.json`（Gitee/GitHub 双源 + 本地缓存）→ 下载 zip
//! → 校验 sha256 → 起批处理替换并重启。
//!
//! 对齐 Python `curseen/update/`（`checker` / `downloader` / `installer`），但线程模型换成
//! 本项目的范式：**每次联网都在独立后台线程，结果经 `UiMsg::Update` 回 UI 线程**
//!（不引异步运行时，也不让后台线程碰 `Runtime`）。
//!
//! 分层：
//! - [`version`]：纯逻辑，版本号解析与比较（可单测，不碰网络与文件）。
//! - [`manifest`]：`update.json` 解析 / 双源拉取 / 本地缓存 / 版本判断。
//! - [`checker`]：旧 Gitee Release API 路径（保留兜底/探针），以及共用的 HTTP 取数。
//! - [`downloader`]：流式写中转文件，带进度回调。
//! - [`installer`]：生成 `curseen_update.bat` 并启动——**替换动作发生在进程外**，
//!   因为 Windows 不允许覆盖正在运行的可执行文件。
//! - [`state`]：`UpdateState`，面板可见的更新状态（`Signal` 属于 windui，故进不了 `core`）。
//! - [`task`]：线程编排（联网动作起后台线程，结果经 [`UiMsg::Update`] 回 UI 线程）。
//!
//! 注意 [`UpdateEvent`] **不在**本目录：它是"后台 → UI"的消息契约，与 `InputEvent`
//! 同类，放在 [`crate::core::events`]；[`UiMsg`] 同理（对齐 Pecia 把 `PipeProtocol.h`
//! 放进 `core` 的做法）。这样本子系统只依赖 `core`，分层图无环。

pub mod checker;
pub mod downloader;
pub mod installer;
pub mod manifest;
pub mod state;
pub mod task;
pub mod version;

pub use crate::core::events::UpdateEvent;
pub use checker::CheckOutcome;
pub use state::UpdateState;
pub use version::Version;

/// 检查到新版本时交给 UI 的弹窗数据（纯数据，不持有 UI 类型）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdatePrompt {
    pub latest: String,
    pub notes: String,
    pub url: String,
    pub sha256: String,
}
