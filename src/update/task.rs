//! 自更新的线程编排：联网动作全在后台线程，结果经 [`UiMsg::Update`] 回 UI 线程。
//!
//! 为什么不让 UI 线程直接调 `checker::check()`：`ureq` 是阻塞的，全局钩子靠 UI 线程的
//! 消息循环续帧，一次 8~10s 的网络等待会直接冻住鼠标特效。这里沿用与输入钩子相同的纪律
//! ——**后台线程只发消息，状态改动只发生在 UI 线程**（由 `Runtime::apply_update_event`
//! 落地）。
//!
//! `Signal` 是 `!Send`（windui 用裸指针把它变成编译期约束），所以后台线程连"顺手 set 一下
//! 状态行"都做不到——这条纪律有编译器兜着。
//!
//! # 依赖倒置
//!
//! 本文件的入口只收**它真正需要的东西**：一个发送端 + （下载时）一个地址。它不认识
//! `Runtime`——编排层没必要为了取这两个值去认识整个运行时。这条约束让本文件可单测，
//! 也把 `update` 从"被运行时调用"变成"可被任何持有发送端的一方调用"。

// ★★ 这里是 **windui** 的 `Sender`（`std` 通道的包装，携带"唤醒 UI 一帧"的 waker），
// 不是 `std::sync::mpsc::Sender`：用裸 std 通道，消息能发出去但 UI 不会被唤醒重绘，
// 更新进度会一直停在旧值上。详见 `ui::PanelHost` 里同名的那条注释。
use std::sync::atomic::{AtomicBool, Ordering};

use windui::prelude::Sender;

use crate::core::events::{UiMsg, UpdateEvent};
use crate::update::{checker, downloader, CheckOutcome, Version};

/// ★ 检查 / 下载各只允许**一个**在跑（2026-09-30）。
///
/// 连点「检查更新」会起多条 `curseen-update` 线程、重复发 `CheckStarted`；下载更糟——两条
/// 线程往**同一个** zip 路径写。二者都是"按下即有副作用、跑到不完不算完"，去重放在编排层
/// 这一处就够了（UI 侧的按钮忙态管的是观感，管不住这条 UI 之外的触发路径）。
static CHECK_BUSY: AtomicBool = AtomicBool::new(false);
static DOWNLOAD_BUSY: AtomicBool = AtomicBool::new(false);

/// 占用一个"忙"标记；已被占则返回 `None`（调用方什么都不做）。**归还靠 Drop**——漏一次
/// 就把这条通路永久堵死（表现为"按钮点了永远没反应"）。
struct BusyGuard(&'static AtomicBool);
impl BusyGuard {
    fn take(flag: &'static AtomicBool) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| Self(flag))
    }
}
impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// 发起「检查更新」。立即返回，结果稍后经通道回来。
///
/// 没有发送端（`None`）时不启动线程，返回可读错误而不是 panic。
pub fn start_check(tx: &Option<Sender<UiMsg>>) -> Result<(), String> {
    let tx = tx.clone().ok_or_else(|| "UI 通道尚未就绪".to_string())?;
    // ★ 已经在检查中就啥也不做（重复触发不叠加线程，见 [`BusyGuard`]）。
    let Some(busy) = BusyGuard::take(&CHECK_BUSY) else {
        return Err("正在检查更新".to_string());
    };
    log::info!("开始检查更新（当前版本 {}）", Version::current());

    spawn(move || {
        let _busy = busy; // 搬到工作线程：线程结束才归还。
        let _ = tx.send(UiMsg::Update(UpdateEvent::CheckStarted));
        let ev = match checker::check(Version::current()) {
            Ok(CheckOutcome::Available { latest, url }) => UpdateEvent::Available { latest, url },
            Ok(CheckOutcome::UpToDate { latest }) => UpdateEvent::UpToDate { latest },
            Ok(CheckOutcome::AvailableNoAsset { latest }) => UpdateEvent::CheckFailed {
                error: format!("新版本 {latest} 未提供可下载的安装包"),
            },
            Err(error) => UpdateEvent::CheckFailed { error },
        };
        let _ = tx.send(UiMsg::Update(ev));
    });
    Ok(())
}

/// 发起「下载并安装」。地址由调用方给出（通常取自 [`UpdateState::url`]）。
pub fn start_download(tx: &Option<Sender<UiMsg>>, url: &str) -> Result<(), String> {
    let tx = tx.clone().ok_or_else(|| "UI 通道尚未就绪".to_string())?;
    if url.is_empty() {
        return Err("没有可用的下载地址".to_string());
    }
    // ★ 同 `start_check`：已经在下载就不叠第二条（两条线程写同一个 zip 路径）。
    let Some(busy) = BusyGuard::take(&DOWNLOAD_BUSY) else {
        return Err("正在下载更新".to_string());
    };
    let url = url.to_string();

    spawn(move || {
        let _busy = busy;
        let dest = downloader::update_zip_path();
        // 进度节流：只在百分比变化时发消息（否则 1.5MB 会发二十几条，白刷 UI）。
        let mut last_pct = i32::MIN;
        let result = downloader::download_to_file(&url, &dest, |received, total| {
            // `total == 0`（服务端未给长度）时 `checked_div` 返回 `None` → 统一记 -1，
            // 这样"未知总量"只会发一条消息（不会每块都发）。
            let pct = received
                .saturating_mul(100)
                .checked_div(total)
                .map_or(-1, |p| p.min(100) as i32);
            if pct != last_pct {
                last_pct = pct;
                let _ = tx.send(UiMsg::Update(UpdateEvent::DownloadProgress {
                    received,
                    total,
                }));
            }
        });
        let ev = match result {
            Ok(()) => UpdateEvent::Downloaded { path: dest },
            Err(error) => UpdateEvent::DownloadFailed { error },
        };
        let _ = tx.send(UiMsg::Update(ev));
    });
    Ok(())
}

/// 起一个分离的更新线程。线程名固定，便于在调试器里认出来。
fn spawn(f: impl FnOnce() + Send + 'static) {
    let spawned = std::thread::Builder::new()
        .name("curseen-update".to_string())
        .spawn(f);
    if let Err(e) = spawned {
        log::error!("启动更新线程失败: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_comparable_and_cloneable() {
        // 事件要经通道跨线程传送，必须 `Send`；`UiMsg` 靠 `Clone`，事件也得跟得上。
        let a = UpdateEvent::Available {
            latest: "1.2.3".into(),
            url: "https://example.com/a.zip".into(),
        };
        let b = a.clone();
        assert_eq!(a, b);
        fn assert_send<T: Send>() {}
        assert_send::<UpdateEvent>();
    }

    /// ★ 回归：入口再也不认识 `Runtime`，于是"没通道"这条边界可以直接构造，
    /// 不必为了测它而先装配整个运行时。
    #[test]
    fn start_without_channel_reports_error() {
        assert!(start_check(&None).is_err());
        assert!(start_download(&None, "https://x/a.zip").is_err());
    }

    /// ★ 回归（2026-09-30）：检查 / 下载各只允许一个在跑，第二次 take 必须拿不到——
    /// 否则连点按钮会起多条线程（下载还往同一个 zip 路径写）。
    #[test]
    fn check_and_download_cannot_run_twice() {
        let g = BusyGuard::take(&CHECK_BUSY);
        assert!(g.is_some(), "首次发起必须成功");
        assert!(BusyGuard::take(&CHECK_BUSY).is_none(), "检查不许并发");
        drop(g);
        assert!(BusyGuard::take(&CHECK_BUSY).is_some(), "结束后必须能再发起");

        let d = BusyGuard::take(&DOWNLOAD_BUSY);
        assert!(d.is_some());
        assert!(BusyGuard::take(&DOWNLOAD_BUSY).is_none(), "下载不许并发");
        drop(d);
    }
}
