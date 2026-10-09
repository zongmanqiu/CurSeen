//! 进程工作集瘦身——内存策略的第二段（第一段"真释放"已各就各位）。
//!
//! 参考pecia（`.ex/pecia`）markdown 预览关闭时的两段式：先**确定性释放对象**（我们
//! 这边 = 功能开关边沿上的 `release_memory`、录制缓冲随 `Session::drop`、面板状态随
//! `close_panel` 的 `take()`），再在事件后**延迟裁剪工作集**——释放掉的页仍留在进程
//! 工作集里，任务管理器的"内存"数字不回落；`EmptyWorkingSet` 把空闲页摘出去交给
//! 系统，数字才落回基线。它不释放任何内存，只是"看起来也轻了"——对常驻软件，
//! 这个观感就是用户对"轻量"的全部感知。
//!
//! # 三条纪律（pecia 实测踩过坑，别重踩）
//!
//! - **只事件驱动，不做 idle/定时轮询**：空闲时反复裁剪会把马上要用的页也逐出工作集，
//!   下次激活触发 soft-fault 风暴，"切回来变卡"。本项目 park 态本就不出帧，更没有
//!   理由养一个定时器。
//! - **延迟 2 秒**：等释放真正落地、页进入 standby 再裁，避免裁掉即将复用的页。
//!   系统侧的释放还会更慢（见 [`SWEEP_WAIT`]），所以在 12 秒处补第二刀。
//! - **录制中跳过这一刀，等下一刀**：录制缓冲是全项目最大的常驻块（DXGI 帧纹理 +
//!   编码器队列），逐出后每帧采集都要往回缺页。用 `capture_share::is_enabled()`（录制
//!   会话存活期置位）做忙判。**忙判绝不能写成"放弃"**——一次请求被丢掉之后没有人会
//!   再来第二次，这一刀就要等到下一次无关事件才发生。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// 请求代数：每次请求 +1。睡眠醒来的线程发现代数变了（期间又有新请求）就静默退出，
/// 只留最新一个线程执行裁剪——把风暴式请求合并成一次。
static TRIM_GEN: AtomicU64 = AtomicU64::new(0);

/// 一轮里要做的**两刀**，以及各自之前等多久（相对于请求时刻 / 上一刀）。
///
/// # 为什么是两刀（2026-10-01 实测）
///
/// 本项目自己的内存都是"同步释放"的（`Vec` / `Box` 的 drop），敲一下 `EmptyWorkingSet`
/// 就干净了。唯独**系统侧**不全如此：`examples/mem_attrib.rs` 的 `d3d-wait` 档显示，
/// `ID3D11Device` 释放之后，厂商 UMD 那 9.4 MB 私有提交**要到 T+10s 才真正交还**
/// （14.53 MB → 5.11 MB，之后 70 秒再无变化）。
///
/// 只裁一刀必然踩在这个延迟之前：那些页从此挂在工作集里，等 Windows 平衡集管理器慢慢
/// 收——用户看到的就是"关掉功能之后停在 40MB，足足过了一分钟才降"。第二刀专门收这个尾。
///
/// 两刀都是 2 秒起跳的"够了再动手"，且整个过程由一条线程串起来，不存在风暴。
const SWEEP_WAIT: [Duration; 2] = [Duration::from_secs(2), Duration::from_secs(10)];

/// 低频重事件后请求一次延迟裁剪。调用点：功能模块关闭边沿
/// （`Runtime::reclaim_disabled_modules`）、录制停止（`stop_recording_now`）、
/// 面板关闭（`Runtime::close_panel`）、裁剪窗关闭、原生文件对话框返回。
///
/// 重复调用安全：后来的请求会让前一条线程在下一个检查点静默退出，只留最新那条。
pub fn request_trim_soon() {
    let gen = TRIM_GEN.fetch_add(1, Ordering::Relaxed) + 1;
    let spawned = std::thread::Builder::new()
        .name("curseen-ws-trim".into())
        .spawn(move || {
            for wait in SWEEP_WAIT {
                std::thread::sleep(wait);
                if TRIM_GEN.load(Ordering::Relaxed) != gen {
                    return; // 期间又来了新请求，让位给最新的那个线程
                }
                // ★ 录制中 → **重排**（跳过这一刀、等下一刀），而不是放弃：录制缓冲
                //   是全项目最大的常驻块，此刻逐出会令每帧采集都往回缺页；而旧实现在
                //   这里直接 return，那一次请求就**永久丢失**了（此后不会再有人请求）。
                if crate::platform::capture_share::is_enabled() {
                    continue;
                }
                trim_working_set();
            }
        });
    if let Err(e) = spawned {
        log::warn!("工作集裁剪线程启动失败: {e}");
    }
}

/// 当前进程的提交内存（`PagefileUsage`，KB）——诊断泄漏用（标注留档链路埋点）。
pub fn private_kb() -> u64 {
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::GetCurrentProcess;
    unsafe {
        let pmc = PROCESS_MEMORY_COUNTERS {
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            ..Default::default()
        };
        let mut pmc = pmc;
        if GetProcessMemoryInfo(GetCurrentProcess(), &mut pmc, pmc.cb).is_err() {
            return 0;
        }
        pmc.PagefileUsage as u64 / 1024
    }
}

/// 立即把进程工作集交还系统。只摘页、不释放内存，随时调用都安全；正常走
/// [`request_trim_soon`] 的延迟路径，直接调用只给测试/诊断用。
pub fn trim_working_set() {
    unsafe {
        // GetCurrentProcess() 返回伪句柄（常量），不需要 Close。
        // ★ windows 0.62 里 Psapi 已改名 ProcessStatus（EmptyWorkingSet 链接的仍是 psapi.dll）。
        if let Err(e) = windows::Win32::System::ProcessStatus::EmptyWorkingSet(
            windows::Win32::System::Threading::GetCurrentProcess(),
        ) {
            log::warn!("EmptyWorkingSet 失败: {e}");
            return;
        }
    }
    log::info!("工作集已整理（空闲页交还系统）");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_working_set_is_safe_to_call_and_returns_ok() {
        // 真调一次：EmptyWorkingSet 对自己的进程恒合法（伪句柄），只摘页不释放，
        // 断言它不报错就算通路。不支持的环境（极老系统）会走 warn 分支，不炸。
        trim_working_set();
    }

    #[test]
    fn request_trim_soon_coalesces_and_does_not_panic() {
        // 连发三次请求：三个线程都睡 2 秒，只有最后一代会真正裁剪。
        // 这里只验证调用路径无害（不 panic、线程正常起）。
        request_trim_soon();
        request_trim_soon();
        request_trim_soon();
    }
}
