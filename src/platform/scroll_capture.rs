//! 长截图（滚动截图）采集管线。
//!
//! # 结构
//!
//! - **算法**（纯函数、无 Win32）：[`crate::core::scroll_stitch::ScrollStitcher`]——
//!   行哈希粗筛 + 纹理加权精配 + 错配率硬判据，同类工具的优点收在它一处。
//! - **采集**（本文件，有 Win32）：单工作线程串行「抓屏 → 喂拼接器」，约 31 fps。
//!
//! # 为什么是单工作线程而不是"UI 线程定时抓"或"异步队列"
//!
//! - UI 线程抓屏一次 10–30 ms，挂进帧循环会把 250ms 对账拖成卡顿；独立线程零侵扰。
//! - 「异步队列 + 排空」（72 帧 / 160 MB 双上限）那类方案解决的是"抓得比算得快"——
//!   串行抓就没有这个问题：抓与算同线程背靠背，天然不积压，一个队列都省掉。
//! - 「`WM_TIMER` 64Hz 采集绑在窗口消息循环上」那类做法，窗口一忙就掉帧；独立线程
//!   只受 `sleep` 节拍管，与任何窗口生死无关。
//!
//! # 收工语义（什么时候算"截完了"）
//!
//! | 原因 | 触发 | 产物 |
//! |---|---|---|
//! | [`EndReason::UserStop`] | 再按一次快捷键 | 交付 |
//! | [`EndReason::Stalled`] | 连续 [`STALL_MS`] 没有新内容（"相邻两帧相同即到底"的放宽版：允许页面有动画噪声，只认"没有新行追加"） | 交付 |
//! | [`EndReason::CanvasFull`] | 画布到达 [`MAX_CANVAS_H`] | 交付 |
//! | [`EndReason::GrabError`] | 抓屏失败 | 报错 |
//!
//! 前三种画布都有效，走同一条交付路（剪贴板 + 留档）；最后一种没有产物可言。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::core::error::{CurSeenError, Result};
use crate::core::scroll_stitch::{transpose_bgra, PushOutcome, ScrollStitcher};
use crate::platform::display::ScreenRect;
use crate::platform::screenshot;

/// 采集节拍（毫秒）。≈31 fps，与 30 fps 录屏同量级：鼠标滚轮一格格滚，抓得再快也只会
/// 产出被帧闸门挡掉的重复帧。
pub const ACTIVE_MS: u64 = 32;

/// 页面停止滚动后多久判定"到底了"。放宽到 5 s（2026-10-01 实测从 2.5 s 放宽）：
/// 页面底部的懒加载、滚动回弹、动画噪声都要等过去；更关键的是**两段式改变了使用节奏**
/// ——用户点完「开始」要移鼠标回页面、滚一段、停下来看内容再滚，中途停顿一超 2.5 s
/// 就被当"到底"收工（实测三连：成品 ≈ 一帧高 / 只多 190 行）。太长则用户停手后干等，
/// 由浮字提示"停滚 5 秒自动结束"对齐预期。
pub const STALL_MS: u64 = 5000;

/// 画布高度硬上限（物理像素）。一张 1000 px 宽 × 30000 px 高的 BGRA ≈ 120 MB——
/// 再往上剪贴板与查看器都未必吃得下，不如在这里体面收工。
pub const MAX_CANVAS_H: usize = 30000;

/// 一次长截图的结束原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    /// 用户再按一次快捷键主动结束。
    UserStop,
    /// 连续 [`STALL_MS`] 没有新内容 → 判定滚到底了，自动收工。
    Stalled,
    /// 画布到达 [`MAX_CANVAS_H`]。
    CanvasFull,
    /// 抓屏出错（会话已废，无产物）。
    GrabError,
}

/// 过程统计（日志 / 测试用）。
#[derive(Debug, Default, Clone)]
pub struct ScrollStats {
    /// 抓到并喂进拼接器的帧数。
    pub frames: u64,
    /// 累计追加的新内容行数。
    pub appended_rows: u64,
    /// 被帧闸门挡掉的重复帧数（页面没动 / 抓屏快于 DWM 刷新）。
    pub no_motion: u64,
    /// 找不到可信重叠被跳过的帧数（快甩）。
    pub ambiguous: u64,
}

/// 工作线程与控制端共享的可变状态。
#[derive(Default)]
struct Shared {
    stats: ScrollStats,
    reason: Option<EndReason>,
    error: Option<String>,
}

/// 一次长截图的收尾结论：统计 + 结束原因 +（可能的）画布。
///
/// 与录屏的 [`crate::platform::recorder::FinishOutcome`] 同一条纪律：**"有没有产物"**
/// 与**"过程出没出错"**分开报，不合并成一个 `Result`。
pub struct ScrollOutcome {
    pub stats: ScrollStats,
    pub reason: EndReason,
    /// 线程中途报的错（现为 [`EndReason::GrabError`] 专配）；`None` = 全程无错。
    pub error: Option<String>,
    /// 画布 `(宽, 高, BGRA)`。`None` = 一帧都没抓到（启动即停 / 失败）。
    pub canvas: Option<(usize, usize, Vec<u8>)>,
}

/// 一个进行中的长截图采集。柄状结构：`start` 拉线程，`finish` 收尸取画布。
pub struct ScrollCapture {
    stop: Arc<AtomicBool>,
    shared: Arc<Mutex<Shared>>,
    /// 拼接器本体住在这里：工作线程每帧锁一次喂帧，`finish` 在线程收尸后整只取走。
    /// 与统计分开两把锁：`auto_reason` 轮询只碰前者，不会与喂帧互相堵。
    slot: Arc<Mutex<Option<ScrollStitcher>>>,
    join: Option<std::thread::JoinHandle<()>>,
    /// 宽截图标记：`finish` 取画布后要**再转置一次**还原成横向长图（喂帧时转过去）。
    horizontal: bool,
}

impl ScrollCapture {
    /// 起后台采集线程（纵向长截图，[`Self::start_dir`] 的便捷形态）。
    pub fn start(rect: ScreenRect) -> Result<Self> {
        Self::start_dir(rect, false)
    }

    /// 起后台采集线程。`horizontal = true` 为**宽截图**：每帧转置（宽↔高）后喂进
    /// 同一个行拼接器，"向右滚"变成"向下滚"，收尾再转置还原——算法零分岔。
    ///
    /// 返回后采集已在跑（抓屏失败在线程内报 [`EndReason::GrabError`]，
    /// 经 [`Self::auto_reason`] / [`Self::finish`] 交给调用方）。
    pub fn start_dir(rect: ScreenRect, horizontal: bool) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Mutex::new(Shared::default()));
        let slot: Arc<Mutex<Option<ScrollStitcher>>> = Arc::new(Mutex::new(None));
        let join = std::thread::Builder::new()
            .name("curseen-scroll".into())
            .spawn({
                let (t_stop, t_shared, t_slot) = (stop.clone(), shared.clone(), slot.clone());
                move || scroll_worker(rect, horizontal, t_stop, t_shared, t_slot)
            })
            .map_err(|e| CurSeenError::Unsupported(leak(&format!("起长截图线程失败: {e}"))))?;
        Ok(Self {
            stop,
            shared,
            slot,
            join: Some(join),
            horizontal,
        })
    }

    /// 过程统计快照。
    pub fn stats(&self) -> ScrollStats {
        self.shared
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .stats
            .clone()
    }

    /// 工作线程是否已**自行**收工（到底 / 画布满 / 抓屏失败）。250ms 对账据此交付。
    pub fn auto_reason(&self) -> Option<EndReason> {
        self.shared.lock().unwrap_or_else(|p| p.into_inner()).reason
    }

    /// 请求停止（不阻塞；收尾由 [`Self::finish`] 等）。
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// 停止并等待线程收尾，取走画布与结论。
    pub fn finish(mut self) -> ScrollOutcome {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
        let (stats, reason, error) = {
            let mut g = self.shared.lock().unwrap_or_else(|p| p.into_inner());
            (
                g.stats.clone(),
                g.reason.take().unwrap_or(EndReason::UserStop),
                g.error.take(),
            )
        };
        let canvas = self
            .slot
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
            .map(|mut s| {
                let (w, h, bgra) = s.take_canvas();
                if self.horizontal {
                    // 宽截图：拼接器里的画布是"转置世界"的（w=选框高、h=已拼宽度），
                    // 收尾再转一次还原成横向长图。一帧都没抓到时画布为空，转置原样返回。
                    let (w, h, bgra) = transpose_bgra(w as i32, h as i32, bgra);
                    (w as usize, h as usize, bgra)
                } else {
                    (w, h, bgra)
                }
            });
        ScrollOutcome {
            stats,
            reason,
            error,
            canvas,
        }
    }
}

/// `CurSeenError::Unsupported` 只吃 `&'static str`（与 `screenshot::box_str` 同一手法）。
fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

/// 采集主循环：串行「抓屏 → 喂拼接器 → 对账统计」。
///
/// ★ 睡在每拍**开头**而不是结尾：`stop` 置位后最多一个 [`ACTIVE_MS`] 内退出，
/// 不会多抓一张毫无意义的帧。
fn scroll_worker(
    rect: ScreenRect,
    horizontal: bool,
    stop: Arc<AtomicBool>,
    shared: Arc<Mutex<Shared>>,
    slot: Arc<Mutex<Option<ScrollStitcher>>>,
) {
    let w = rect.w.max(1) as u32;
    let h = rect.h.max(1) as u32;
    {
        let mut g = match slot.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        // 宽截图：拼接器活在"转置世界"里——帧宽 = 选框高、帧高 = 选框宽。
        *g = Some(ScrollStitcher::new(
            if horizontal { h } else { w },
            if horizontal { w } else { h },
        ));
    }
    let interval = Duration::from_millis(ACTIVE_MS);
    let stall = Duration::from_millis(STALL_MS);
    let mut last_progress = Instant::now();
    loop {
        if stop.load(Ordering::Relaxed) {
            set_reason(&shared, EndReason::UserStop, None);
            return;
        }
        std::thread::sleep(interval);
        // 睡完再看一眼：停止请求到达后不再抓屏（抓了也只会被丢弃，还拖慢收尸）。
        if stop.load(Ordering::Relaxed) {
            set_reason(&shared, EndReason::UserStop, None);
            return;
        }
        let shot = match screenshot::grab(rect) {
            Ok(s) => s,
            Err(e) => {
                set_reason(
                    &shared,
                    EndReason::GrabError,
                    Some(format!("抓屏失败: {e}")),
                );
                return;
            }
        };
        // 尺寸对不上（理论上不可能：区域固定）按失败处理，绝不把错位字节喂进拼接器。
        if shot.w != w as i32 || shot.h != h as i32 {
            set_reason(
                &shared,
                EndReason::GrabError,
                Some(format!(
                    "抓屏尺寸 {}×{} 与区域 {}×{} 不符",
                    shot.w, shot.h, w, h
                )),
            );
            return;
        }
        // 宽截图：转置后再喂（"向右滚" → "向下滚"，见 `transpose_bgra` 的文档）。
        let bytes = if horizontal {
            let (_, _, t) = transpose_bgra(shot.w, shot.h, shot.bgra);
            t
        } else {
            shot.bgra
        };
        let outcome = {
            let mut g = match slot.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            match g.as_mut() {
                Some(st) => st.push(&bytes),
                None => return, // 拼接器已被取走（不该发生）：安静退出
            }
        };
        {
            let mut g = match shared.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            g.stats.frames += 1;
            match outcome {
                PushOutcome::Appended { new_rows } => {
                    g.stats.appended_rows += new_rows as u64;
                    // 画布上限：锁内直接问拼接器（它刚被 push 过，这里再锁一次 slot）。
                    let canvas_h = slot
                        .lock()
                        .ok()
                        .and_then(|s| s.as_ref().map(|st| st.canvas_h()))
                        .unwrap_or(0);
                    if canvas_h >= MAX_CANVAS_H {
                        g.reason = Some(EndReason::CanvasFull);
                        return;
                    }
                }
                PushOutcome::NoMotion => g.stats.no_motion += 1,
                PushOutcome::Ambiguous => g.stats.ambiguous += 1,
                PushOutcome::NoNewContent => {}
            }
        }
        // 到底判定只看"有没有新行追加"：重复帧 / 跳帧不刷新进度钟。
        if last_progress.elapsed() >= stall {
            set_reason(&shared, EndReason::Stalled, None);
            return;
        }
        if matches!(outcome, PushOutcome::Appended { .. }) {
            last_progress = Instant::now();
        }
    }
}

/// 线程自行收工时落原因与错误（已有的原因不覆盖）。
fn set_reason(shared: &Mutex<Shared>, reason: EndReason, error: Option<String>) {
    if let Ok(mut g) = shared.lock() {
        if g.reason.is_none() {
            g.reason = Some(reason);
        }
        if error.is_some() {
            g.error = error;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 收尾结论的默认形状：`finish` 没起线程时（测试桩）不该有画布。
    /// 真线程的端到端行为需要真实抓屏（见 `tests/` 目录的探针），单测只钉纯数据面。
    #[test]
    fn outcome_defaults_are_usable() {
        let o = ScrollOutcome {
            stats: ScrollStats::default(),
            reason: EndReason::UserStop,
            error: None,
            canvas: None,
        };
        assert!(o.canvas.is_none());
        assert_eq!(o.stats.frames, 0);
        assert_eq!(o.reason, EndReason::UserStop);
    }

    /// 常量自洽：节拍 < 到底判定，画布上限是能落进 `usize` 的正数。
    #[test]
    fn constants_are_sane() {
        // `const {}` 块：这两条判的是常量本身，放到编译期（clippy 1.97 起提示）。
        const {
            assert!(ACTIVE_MS < STALL_MS, "节拍必须快于到底判定");
            assert!(MAX_CANVAS_H > 0);
        }
        assert_eq!(std::mem::size_of::<EndReason>(), 1, "枚举应保持无负载");
    }
}
