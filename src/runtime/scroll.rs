//! 长截图会话层：把采集管线（`platform::scroll_capture`）接进 Runtime 的状态机。
//!
//! # 交互流（**两段式**，与 GIF / 选区录屏同一条流——用户口径 2026-10-01：
//! "选框结束后都不知道怎么开始和结束"，所以起点必须可见、可选）
//!
//! ```text
//! 快捷键（空闲）   → Runtime::toggle_scroll → begin_snap(SnapMode::Scroll)
//!                    整张虚拟桌面冻结帧 → 区域选择窗框一块
//! 确认（松手/回车）→ Runtime::confirm_snap 提前返回 → arm_scroll(rect)
//!                    ★ 就绪态（不采集）：蓝色边框 + 控件条（绿三角，挂在选框外）
//! 开始             → 点控件条「开始」或再按快捷键 → start_scroll
//!                    起采集线程；边框变紫；控件条键变红圆
//! 采集期间         → 用户滚动目标页面；工作线程 31fps 抓屏喂 ScrollStitcher
//! 结束             → 点「停止」或再按快捷键（UserStop）/ 停止滚动 5s（Stalled，自动）
//!                    / 画布满（CanvasFull）/ Esc（取消，丢弃）
//! 交付             → 剪贴板 + temp/screenshot 留档 + 入贴图环（带原位）+ 白闪
//! ```
//!
//! # 与既有机制的接缝
//!
//! - **控件条**与 GIF 共用同一条窗（`layer::gif_bar`，按模式分岔），三态一眼可读：
//!   绿三角 = 就绪未采、红圆 = 采集中、✕ = 取消。
//! - **不污染剪贴板**：选区确认那一刻只取矩形（`confirm_snap` 提前返回，同 `Mp4Region`
//!   ——那些"截图语义"的副作用套在"要录哪一块"上是错的）；真正的剪贴板写入发生在
//!   **交付**那一刻，写的是拼接产物。
//! - **park 语义**：就绪态与采集态都让 `input_needs_frame` 为真（边框必须留在屏上——
//!   park 会把缓冲刷成全透明，见 overlay 的休眠注释）⇒ 覆盖层不 park；快捷键与 Esc 都
//!   已登记 `action_hotkey_wakes`，休眠态也能送达。
//! - **穿透**：就绪/采集期间覆盖层保持全屏穿透（不做任何交互），用户的滚轮 / 点击照常
//!   到达目标页面 —— 长截图本来就是"抓别人在滚的页面"；唯一的可点物是框外的控件条。

use super::*;
use crate::platform::scroll_capture::{EndReason, ScrollCapture, ScrollOutcome};

/// 一次进行中的长截图。
pub struct ScrollSession {
    pub(crate) cap: ScrollCapture,
    /// 采集区域（虚拟桌面物理像素）。覆盖层画边框用；交付时换算成贴图环的原位。
    pub rect: ScreenRect,
}

/// 就绪态（框好了、等「开始」）。方向在**框选那一刻**由按下的键决定，
/// 之后与矩形绑在一起走完全程（拖拽调整只动 `rect`）。
#[derive(Clone, Copy)]
pub(crate) struct ScrollReady {
    pub rect: ScreenRect,
    /// `true` = 宽截图（向右滚）。拼接走转置（见 `core::scroll_stitch::transpose_bgra`）。
    pub horizontal: bool,
}

impl Runtime {
    /// 长截图是否进行中（帧门 / Esc 判定 / 边框绘制都用它，不另设第二判据）。
    pub fn scroll_active(&self) -> bool {
        self.scroll.is_some()
    }

    /// 长截图是否**就绪未采**（框选完了、等「开始」）。返回选框矩形。
    pub fn scroll_ready(&self) -> Option<ScreenRect> {
        self.scroll_ready.map(|s| s.rect)
    }

    /// 长截图会话区域（覆盖层画边框用）。`None` = 没有会话。
    pub fn scroll_rect(&self) -> Option<ScreenRect> {
        self.scroll.as_ref().map(|s| s.rect)
    }

    /// 覆盖层边框用的合并视图：`Some((矩形, 是否采集中))`。
    ///
    /// 就绪态画**蓝**框（= 还没开始，语义与 GIF 选框一致），采集态画**紫**框。
    /// 就绪与会话互斥（`start_scroll` 是原子搬移），`ready` 优先读只是防御。
    pub fn scroll_frame(&self) -> Option<(ScreenRect, bool)> {
        if let Some(r) = self.scroll_ready {
            return Some((r.rect, false));
        }
        self.scroll_rect().map(|r| (r, true))
    }

    /// 控件条（与 GIF 共窗）用的合并视图：`Some((矩形, 是否采集中))`。
    /// 就绪态与采集态都要挂条 —— 条是"现在什么状态、怎么开始/结束"的常驻答案。
    pub fn scroll_bar_selection(&self) -> Option<(ScreenRect, bool)> {
        self.scroll_frame()
    }

    /// 就绪态选框此刻**是否可交互**（画手柄 + 接收拖拽调整）。
    ///
    /// 与 GIF 的 `gif_interaction_enabled` 同款判据：有就绪框 + 没有其它叠层抢鼠标。
    /// **采集态永远不可交互**：那时覆盖层必须全穿透（用户要在框边附近操作目标页面），
    /// 手柄也不画——它们伸进框内的半截会被 BitBlt 拍进帧里（2026-10-01 实测教训）。
    pub fn scroll_interaction_enabled(&self) -> bool {
        self.scroll_ready.is_some() && !self.layer().active()
    }

    /// 此刻有没有"可拖拽选区"挂着（GIF 交互**或**长截图就绪）。
    ///
    /// 覆盖层的按下/拖动/放大镜门只用这一个判据，别处再各写一半就会出现
    /// "加新流忘了一处"（命中、光标、放大镜三处不同步的错位态）。
    pub fn drag_interaction_enabled(&self) -> bool {
        self.gif_interaction_enabled() || self.scroll_interaction_enabled()
    }

    /// 应用一次拖拽（平移 / 缩放）到**就绪态**选框。
    ///
    /// 与 [`Self::gif_resize_or_move`](crate::Runtime::gif_resize_or_move) 完全同一套
    /// 机制：增量语义（`apply_zone`）、虚拟桌面钳制、`GIF_ZONE_MIN_SIZE` 最小尺寸。
    /// `zone = None` 不吃事件也不通知。
    pub fn scroll_resize_or_move(&mut self, zone: HitZone, dx: i32, dy: i32) {
        if zone == HitZone::None {
            return;
        }
        let Some(mut r) = self.scroll_ready else {
            return;
        };
        apply_zone(&mut r.rect, self.screen, zone, dx, dy);
        self.scroll_ready = Some(r);
        self.notify_scroll_changed();
    }

    /// 长截图 / 宽截图快捷键（**一键开关**）：空闲 → 进框选（方向由这把键定）；
    /// 就绪 → 开始采集；采集中 → 结束并交付。
    ///
    /// 两把键共用同一套会话状态：会话挂着的时刻，先按哪把键都只承担"开始/结束"语义，
    /// 不会中途换轴（方向在框选那一刻已定）。
    pub(crate) fn toggle_scroll(&mut self, mode: snap::SnapMode) {
        if self.scroll.is_some() {
            self.finish_scroll();
            return;
        }
        if self.scroll_ready.is_some() {
            self.start_scroll();
            return;
        }
        if self.snap.pending.is_some() {
            return; // 区域选择已在进行中
        }
        self.begin_snap(mode);
    }

    /// 控件条「开始 / 停止」那颗键（绿三角 / 红圆）：就绪 → 开始；采集中 → 结束。
    ///
    /// 与 [`Self::toggle_scroll`] 的差别只在空态分支：条只在该挂着的时候可见，
    /// 空态按这颗键不该顺手开一次框选（那是快捷键的职责）。
    pub(crate) fn toggle_scroll_from_bar(&mut self) {
        if self.scroll.is_some() {
            self.finish_scroll();
            return;
        }
        if self.scroll_ready.is_some() {
            self.start_scroll();
        }
    }

    /// 选区确认（`confirm_snap` 提前返回到这里）：**只进就绪态**，不起线程。
    ///
    /// 把冻结帧坐标系的矩形换回世界坐标，挂出蓝框 + 控件条（绿三角），等用户点
    /// 「开始」或再按快捷键。原"确认即采集"之所以改成两段：用户不知道采集已经
    /// 开始、更不知道怎么结束 —— 与 GIF 完全相同的病，用完全相同的药。
    /// `horizontal`（宽截图，向右滚）由触发框选的那把键决定。
    pub(crate) fn arm_scroll(&mut self, rect: Rect, horizontal: bool) {
        let r = ScreenRect {
            x: rect.x.round() as i32,
            y: rect.y.round() as i32,
            w: rect.w.round().max(1.0) as i32,
            h: rect.h.round().max(1.0) as i32,
        };
        self.scroll_ready = Some(ScrollReady {
            rect: r,
            horizontal,
        });
        if horizontal {
            self.action_notify
                .show("宽截图就绪：点「开始」或再按快捷键，然后向右滚动页面（Esc 取消）");
        } else {
            self.action_notify
                .show("长截图就绪：点「开始」或再按快捷键开始采集（Esc 取消）");
        }
        if let Some(tx) = &self.ui_tx {
            let _ = tx.send(UiMsg::ScrollChanged);
        }
        let kind = if horizontal { "宽截图" } else { "长截图" };
        log::info!(
            "{kind}就绪：{}×{} @ ({}, {})，等「开始」",
            r.w,
            r.h,
            r.x,
            r.y
        );
    }

    /// 就绪 → 采集：起工作线程，边框变紫、控件条键变红圆。
    fn start_scroll(&mut self) {
        let Some(r) = self.scroll_ready.take() else {
            return;
        };
        match ScrollCapture::start_dir(r.rect, r.horizontal) {
            Ok(cap) => {
                self.scroll = Some(ScrollSession { cap, rect: r.rect });
                if r.horizontal {
                    self.action_notify
                        .show("宽截图中：向右滚动页面采集，停滚 5 秒自动结束（快捷键 / 红圆随时停，Esc 取消）");
                } else {
                    self.action_notify
                        .show("长截图中：滚动页面采集，停滚 5 秒自动结束（快捷键 / 红圆随时停，Esc 取消）");
                }
                if let Some(tx) = &self.ui_tx {
                    let _ = tx.send(UiMsg::ScrollChanged);
                }
                log::info!(
                    "{}开始：{}×{} @ ({}, {})",
                    if r.horizontal {
                        "宽截图"
                    } else {
                        "长截图"
                    },
                    r.rect.w,
                    r.rect.h,
                    r.rect.x,
                    r.rect.y
                );
            }
            Err(e) => {
                log::warn!("长截图启动失败: {e}");
                self.action_notify.show("长截图启动失败");
            }
        }
    }

    /// 长截图状态变了 → 推一条消息让 UI 线程同步控件条/边框（250ms 对账是兜底，
    /// 这条让"开始/结束"当帧生效，条不滞后）。
    fn notify_scroll_changed(&self) {
        if let Some(tx) = &self.ui_tx {
            let _ = tx.send(UiMsg::ScrollChanged);
        }
    }

    /// 用户结束（快捷键再按一次 / 控件条红圆）：停线程、交付画布。
    pub(crate) fn finish_scroll(&mut self) {
        let Some(s) = self.scroll.take() else {
            return;
        };
        let outcome = s.cap.finish();
        self.notify_scroll_changed();
        self.deliver_scroll(outcome, s.rect);
    }

    /// 取消（Esc / 控件条 ✕）：**就绪态**直接清掉（无线程可收）；**采集态**停线程并
    /// **丢弃**画布，不留痕迹。
    pub(crate) fn cancel_scroll(&mut self) {
        if self.scroll_ready.take().is_some() {
            log::info!("长截图就绪态已取消");
            self.action_notify.show("长截图已取消");
            if let Some(tx) = &self.ui_tx {
                let _ = tx.send(UiMsg::ScrollChanged);
            }
            return;
        }
        let Some(s) = self.scroll.take() else {
            return;
        };
        let outcome = s.cap.finish();
        self.notify_scroll_changed();
        log::info!(
            "长截图已取消（{} 帧、{} 行，丢弃画布）",
            outcome.stats.frames,
            outcome.stats.appended_rows
        );
        self.action_notify.show("长截图已取消");
    }

    /// 250ms 对账：工作线程**自行**收工（到底 / 画布满 / 抓屏失败）时在这里交付。
    /// 返回 true = 会话刚结束（调用方据此补一帧重绘，把边框抹掉）。
    pub(crate) fn poll_scroll(&mut self) -> bool {
        let done = self
            .scroll
            .as_ref()
            .is_some_and(|s| s.cap.auto_reason().is_some());
        if !done {
            return false;
        }
        self.finish_scroll();
        true
    }

    /// 长截图采集的进度统计（诊断用）。没有会话返回 `None`。
    pub fn scroll_stats(&self) -> Option<crate::platform::scroll_capture::ScrollStats> {
        self.scroll.as_ref().map(|s| s.cap.stats())
    }

    /// 交付：拼接画布 → 剪贴板 + 留档 + 入贴图环（带原位）+ 白闪。
    ///
    /// ★ 与普通截图同一条出口语义：长截图的产物就是一张图，用户对它的期待与截图
    /// 完全一致（能粘出去、能找到文件、能贴到桌面上）。唯一差别是尺寸可能很大 ——
    /// 那是长截图的固有属性，不是这里要修的。
    fn deliver_scroll(&mut self, outcome: ScrollOutcome, rect: ScreenRect) {
        // ① 抓屏错误优先：线程都收尸了还没有产物，报错退出。
        if let Some(err) = outcome.error {
            log::warn!("长截图失败：{err}");
            self.action_notify.show("长截图失败（抓屏出错）");
            return;
        }
        // ② 一帧都没抓到（启动即停）：照"取消"处理，不报错。
        let Some((w, h, bgra)) = outcome.canvas else {
            log::info!("长截图结束：没有抓到任何帧");
            self.action_notify.show("长截图已取消");
            return;
        };
        log::info!(
            "长截图收工（{:?}）：{} 帧、追加 {} 行、重复 {} 帧、跳帧 {} 帧",
            outcome.reason,
            outcome.stats.frames,
            outcome.stats.appended_rows,
            outcome.stats.no_motion,
            outcome.stats.ambiguous
        );
        // ③ 一次转换到位：画布 BGRA → 非预乘 RGBA（原地换序 + 补 α，不另分配）。
        //    旧路为贴图环借用转一份、落盘时再把原帧转一遍 —— 长画布（1920×10000 ≈ 77 MB）
        //    上这两趟都随高度线性涨。转换后的这一份位图是剪贴板 / 留档 / 贴图环的**同一份**
        //    数据，环只多拿一个 `Rc` 指针。
        let Some(bmp) = crate::core::backdrop::shot_to_bitmap_owned(Shot {
            w: w as i32,
            h: h as i32,
            bgra,
        }) else {
            log::warn!("长截图产物尺寸非法（{w}×{h}），无法交付");
            self.action_notify.show("长截图失败（尺寸非法）");
            return;
        };
        let bmp = Rc::new(bmp);
        // ④ 剪贴板。写成功就记序列号（贴图轮询别把自家产物当"外部复制"再读一遍）。
        if let Err(e) = screenshot::to_clipboard_rgba(&bmp) {
            log::warn!("长截图写入剪贴板失败: {e}");
            self.action_notify.show("长截图写入剪贴板失败");
        } else {
            let n = crate::platform::clipboard::sequence_number();
            self.own_clip_seq = Some(n);
            self.clip_seq = n;
        }
        // ⑤ 入贴图环，带原位（用户口径 2：凡是我们功能截下来的图都要记住"原来在哪"）。
        let origin = Rect {
            x: rect.x as f32,
            y: rect.y as f32,
            w: rect.w as f32,
            h: rect.h as f32,
        };
        self.snap
            .note_shot(snap::PinContent::Image(Rc::clone(&bmp)), origin);
        // ⑥ 留档（与普通截图同一个目录）。同一份位图直接走快档（用户看不到这个文件，
        //    长图默认档编码尤其慢）。
        let dir = crate::core::paths::module_temp_dir(crate::core::paths::MODULE_SCREENSHOT);
        let path = dir.join(format!("CurSeen_scroll_{}.png", timestamp()));
        let saved = screenshot::save_png_rgba_fast(&bmp, &path).is_ok();
        // ⑦ 反馈：白闪 + 中央浮字（两条通道并存，与截图编辑态完成时同款）。
        self.action_notify.flash();
        self.action_notify.show(&format!(
            "长截图 {}×{} 已保存到　temp\\{}",
            w,
            h,
            crate::core::paths::MODULE_SCREENSHOT
        ));
        log::info!(
            "长截图完成：{}×{}（留档={saved}，{}）",
            w,
            h,
            path.display()
        );
    }

    /// 结束原因是否"到底了"（诊断 / 测试用；真正消费在 `deliver_scroll` 的日志里）。
    pub fn scroll_ended_by_stall(outcome: &ScrollOutcome) -> bool {
        outcome.reason == EndReason::Stalled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::testkit::*;

    /// 会话状态机的接线：arm → ready（不起线程），start 起会话，poll 兜底，Esc 取消。
    ///
    /// 真实的采集需要抓屏（工作线程会真的抓桌面）——`start_scroll` 起的线程在 5 s
    /// 后会自行"到底"收工，测试结束前总是把它收掉，不留孤儿线程。
    #[test]
    fn scroll_session_toggles_and_cancels_cleanly() {
        let mut rt = drawing_only();
        assert!(!rt.scroll_active());
        // 没有会话时 finish / cancel 都是安全空操作。
        rt.finish_scroll();
        rt.cancel_scroll();
        assert!(!rt.scroll_active());
        assert!(rt.scroll_ready().is_none());
        // 框选确认 → **就绪态**（不起线程，这是两段式的核心断言）。
        rt.arm_scroll(
            Rect {
                x: 10.0,
                y: 10.0,
                w: 120.0,
                h: 80.0,
            },
            false,
        );
        assert!(rt.scroll_ready().is_some(), "确认后应停在就绪态");
        assert!(!rt.scroll_active(), "就绪态不该已经起线程");
        assert_eq!(
            rt.scroll_frame(),
            Some((
                crate::core::geometry::ScreenRect::new(10, 10, 120, 80),
                false
            )),
            "边框视图：就绪矩形 + 未采集"
        );
        // 就绪态取消：直接清掉，无线程遗留。
        rt.cancel_scroll();
        assert!(rt.scroll_ready().is_none());
        assert!(!rt.scroll_active());
        assert!(rt.scroll_frame().is_none());
    }

    /// `toggle_scroll` 三态：空闲进框选、就绪态开始采集、采集中结束。
    #[test]
    fn toggle_scroll_walks_ready_then_active() {
        let mut rt = drawing_only();
        // 空闲：进框选会话。
        rt.toggle_scroll(snap::SnapMode::Scroll);
        assert!(rt.snap.is_pending(), "空闲按下长截图键应进入区域选择");
        assert_eq!(
            rt.snap.pending_mode(),
            Some(snap::SnapMode::Scroll),
            "框选会话的模式必须是 Scroll（确认后走长截图分支）"
        );
        rt.snap.cancel();
        // 就绪：toggle = 开始。
        rt.arm_scroll(
            Rect {
                x: 0.0,
                y: 0.0,
                w: 100.0,
                h: 60.0,
            },
            false,
        );
        rt.toggle_scroll(snap::SnapMode::Scroll);
        if rt.scroll_active() {
            assert!(rt.scroll_ready().is_none(), "开始后就绪态应被搬空");
            assert_eq!(rt.scroll_rect().map(|r| (r.w, r.h)), Some((100, 60)));
            // 采集中：toggle = 结束交付。
            rt.toggle_scroll(snap::SnapMode::Scroll);
            assert!(!rt.scroll_active());
        } else {
            // 线程没起来（极端环境）：start_scroll 已提示失败，ready 应已被清掉。
            assert!(rt.scroll_ready().is_none());
        }
        // 控件条那颗键：空态必须是安全空操作（不开框选）。
        rt.toggle_scroll_from_bar();
        assert!(!rt.scroll_active() && rt.scroll_ready().is_none());
        assert!(!rt.snap.is_pending());
    }

    /// 宽截图（2026-10-01 用户需求）：方向由触发框选的键决定，绑进就绪态一路带到
    /// `start_scroll`。空闲时按宽截图键进的是 `ScrollWide` 框选。
    #[test]
    fn wide_scroll_carries_its_direction_to_capture() {
        let mut rt = drawing_only();
        // 空闲按宽截图键：框选模式必须是 ScrollWide。
        rt.toggle_scroll(snap::SnapMode::ScrollWide);
        assert_eq!(
            rt.snap.pending_mode(),
            Some(snap::SnapMode::ScrollWide),
            "宽截图键的框选会话必须是 ScrollWide"
        );
        rt.snap.cancel();
        // 确认 → 就绪态带方向；开始 → 会话已按方向起线程（横/纵只影响喂帧转置，
        // 对外几何不变）。线程起不来（极端环境）也必须把就绪态搬干净。
        rt.arm_scroll(
            Rect {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 50.0,
            },
            true,
        );
        assert_eq!(rt.scroll_ready().map(|r| (r.w, r.h)), Some((80, 50)));
        rt.toggle_scroll(snap::SnapMode::ScrollWide);
        assert!(rt.scroll_ready().is_none());
        if rt.scroll_active() {
            rt.toggle_scroll(snap::SnapMode::ScrollWide); // 结束交付
        }
        assert!(!rt.scroll_active() && rt.scroll_ready().is_none());
    }

    /// Esc / ✕ 的取消路径在**就绪态**也要通（用户框完反悔，不该被按着的采集线程锁死）。
    #[test]
    fn cancel_clears_the_ready_state_without_touching_threads() {
        let mut rt = drawing_only();
        rt.arm_scroll(
            Rect {
                x: 0.0,
                y: 0.0,
                w: 50.0,
                h: 40.0,
            },
            false,
        );
        rt.cancel_scroll();
        assert!(rt.scroll_ready().is_none());
        assert!(!rt.scroll_active());
        // 再来一次空取消仍是安全空操作。
        rt.cancel_scroll();
    }

    /// 就绪态选框**可拖拽调整**（2026-10-01 用户需求）：平移、越界钳制、最小尺寸，
    /// 与 GIF 选框同一套几何。交互门在就绪态开、清掉后关。
    #[test]
    fn ready_selection_can_be_dragged_and_clamped() {
        let mut rt = drawing_only();
        rt.arm_scroll(
            Rect {
                x: 100.0,
                y: 100.0,
                w: 200.0,
                h: 100.0,
            },
            false,
        );
        assert!(
            rt.scroll_interaction_enabled(),
            "就绪态该能拖拽（画手柄 + 接收拖动）"
        );
        assert!(
            rt.drag_interaction_enabled(),
            "合并交互门在长截图就绪态必须为真"
        );
        // 平移：增量语义与 GIF 相同。
        rt.scroll_resize_or_move(HitZone::Body, 50, 30);
        assert_eq!(
            rt.scroll_ready(),
            Some(ScreenRect::new(150, 130, 200, 100)),
            "Body 拖拽 = 整体平移"
        );
        // 越界 clamp（与 gif_move_clamps_to_screen 同一套钳制）：FALLBACK 桌面 1920×1080。
        rt.scroll_resize_or_move(HitZone::Body, 10_000, 10_000);
        assert_eq!(
            rt.scroll_ready(),
            Some(ScreenRect::new(1720, 980, 200, 100)),
            "右边缘=x+w=1920，下边缘=1080"
        );
        // 缩到最小尺寸。
        rt.scroll_resize_or_move(HitZone::CornerBR, -10_000, -10_000);
        let min = Runtime::GIF_ZONE_MIN_SIZE;
        assert_eq!(
            rt.scroll_ready().map(|r| (r.w, r.h)),
            Some((min, min)),
            "右下角拖到底只留最小尺寸"
        );
        // None zone：不吃也不动。
        rt.scroll_resize_or_move(HitZone::None, 50, 50);
        assert_eq!(
            rt.scroll_ready().map(|r| (r.x, r.y)),
            Some((1720, 980)),
            "None 命中必须是空操作"
        );
        // 清掉就绪态后交互门必须关上。
        rt.cancel_scroll();
        assert!(!rt.scroll_interaction_enabled());
        assert!(!rt.drag_interaction_enabled());
    }

    /// `poll_scroll` 在没有会话时必须是 `false`（250ms 对账每拍都调，不能有副作用）。
    #[test]
    fn poll_scroll_without_a_session_is_a_noop() {
        let mut rt = drawing_only();
        assert!(!rt.poll_scroll());
    }

    /// 长截图交付：一次转换、三处共享 —— 剪贴板 / 留档 / 贴图环必须是同一张图。
    ///
    /// 画布用 GDI 帧的不变量 α=255（`scroll_capture` 拼的都是 `grab` 出来的帧），
    /// 走**真实**的 `deliver_scroll`：剪贴板读回、PNG 解码、环内容三者都要与源像素
    /// 逐字节一致。新路径里换序、行序、DIB 自下而上任一处理错，这条会红。
    #[cfg(windows)]
    #[test]
    fn deliver_scroll_produces_identical_clipboard_archive_and_ring() {
        let mut rt = drawing_only();
        let (w, h) = (4_usize, 3_usize);
        let mut bgra = Vec::with_capacity(w * h * 4);
        let mut rgba = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                let b = (0x10 + x * 0x11) as u8;
                let g = (0x20 + y * 0x21) as u8;
                let r = (0x30 + (x + y) * 0x07) as u8;
                bgra.extend_from_slice(&[b, g, r, 0xFF]);
                rgba.extend_from_slice(&[r, g, b, 0xFF]);
            }
        }

        let dir = crate::core::paths::module_temp_dir(crate::core::paths::MODULE_SCREENSHOT);
        let scroll_pngs = || -> Vec<std::path::PathBuf> {
            std::fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with("CurSeen_scroll_"))
                })
                .collect()
        };
        let existing = scroll_pngs();

        rt.deliver_scroll(
            ScrollOutcome {
                stats: crate::platform::scroll_capture::ScrollStats::default(),
                reason: EndReason::UserStop,
                error: None,
                canvas: Some((w, h, bgra.clone())),
            },
            ScreenRect::new(10, 20, w as i32, h as i32),
        );

        // ① 剪贴板：读回与画布逐字节相同。
        let mut back = None;
        for _ in 0..20 {
            if let Some(s) = crate::platform::clipboard::get_image() {
                back = Some(s);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let back = back.expect("读回长截图剪贴板");
        assert_eq!((back.w, back.h), (w as i32, h as i32));
        assert_eq!(back.bgra, bgra, "剪贴板像素（含通道与行序）必须与画布一致");

        // ② 留档：本次新出现的 CurSeen_scroll_*.png 解码后必须与画布 RGBA 一致。
        let fresh: Vec<_> = scroll_pngs()
            .into_iter()
            .filter(|p| !existing.contains(p))
            .collect();
        assert_eq!(fresh.len(), 1, "本次交付应恰好落一张 PNG");
        let bytes = std::fs::read(&fresh[0]).expect("读留档");
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder.read_info().expect("读 PNG 头");
        let mut buf = vec![0_u8; reader.output_buffer_size().expect("输出缓冲可算")];
        let info = reader.next_frame(&mut buf).expect("解码 PNG");
        buf.truncate(info.buffer_size());
        assert_eq!((info.width, info.height), (w as u32, h as u32));
        assert_eq!(buf, rgba, "留档像素必须与画布一致");
        let _ = std::fs::remove_file(&fresh[0]);

        // ③ 贴图环：同一张图（新路径只复制了 Rc）。
        match rt.snap.ring_back(0) {
            Some(snap::PinContent::Image(rc)) => {
                assert_eq!(rc.rgba, rgba, "贴图环内容必须与画布一致");
            }
            _ => panic!("长截图产物应进入贴图环"),
        }
        assert!(rt.action_notify.flashing(), "交付必须触发白闪");
    }

    /// 生产路径量具：长截图交付的旧/新序列，两个画布高度把固定项与面积项分开。
    ///
    /// 跑法：`PERF_ROUNDS=200 cargo test --release --lib perf_probe_scroll_deliver -- --ignored --nocapture --test-threads=1`
    /// `PERF_REVERSE=1` 交换同轮内旧/新序列的先后，排除"后跑吃热缓存"。
    ///
    /// 旧序列 = 借用转换（给环）+ `to_clipboard(&shot)` + `save_png_fast(shot)`；
    /// 新序列 = 消费转换 + `to_clipboard_rgba` + `save_png_rgba_fast`。每一臂都是生产
    /// 函数；画布由一次真实抓屏的行循环铺成（相邻 1080 行块交替翻转，避免整块重复把
    /// PNG 压缩率撑得太好看）。
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn perf_probe_scroll_deliver() {
        use std::time::{Duration, Instant};

        fn tiled(base: &Shot, out_h: usize) -> Shot {
            let src_w = base.w as usize;
            let src_h = base.h as usize;
            let mut bgra = Vec::with_capacity(src_w * out_h * 4);
            for y in 0..out_h {
                let block = y / src_h;
                let within = y % src_h;
                let sy = if block.is_multiple_of(2) {
                    within
                } else {
                    src_h - 1 - within
                };
                let start = sy * src_w * 4;
                bgra.extend_from_slice(&base.bgra[start..start + src_w * 4]);
            }
            Shot {
                w: base.w,
                h: out_h as i32,
                bgra,
            }
        }

        fn report(label: &str, v: &mut [Duration]) {
            v.sort();
            let n = v.len();
            if n == 0 {
                println!("{label:<28} 无样本");
                return;
            }
            let mean = v.iter().map(|d| d.as_secs_f64()).sum::<f64>() / n as f64 * 1e3;
            println!(
                "{label:<28} median {:7.2} ms  mean {:7.2}  p95 {:7.2}  (n={n})",
                v[n / 2].as_secs_f64() * 1e3,
                mean,
                v[(n * 95 / 100).min(n - 1)].as_secs_f64() * 1e3
            );
        }

        let rounds: usize = std::env::var("PERF_ROUNDS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3)
            .max(1);
        let reverse = std::env::var("PERF_REVERSE").is_ok();
        let r = crate::platform::display::virtual_screen();
        let base = crate::platform::screenshot::grab(r).expect("预热抓屏");
        let heights = [r.h as usize, r.h as usize * 6];

        /// 剪贴板是共享资源（另一个进程短暂占用是常态）：失败重试，样本只记成功那一次；
        /// 持续占用就丢这一臂的样本（不能把 10 ms 的退避睡进读数）。
        fn timed_clip(mut write: impl FnMut() -> bool) -> Option<Duration> {
            for _ in 0..100 {
                let t = Instant::now();
                if write() {
                    return Some(t.elapsed());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            None
        }

        for height in heights {
            let canvas = tiled(&base, height);
            println!("--- {}×{} ---", canvas.w, canvas.h);
            let mut old_total = Vec::new();
            let mut old_convert = Vec::new();
            let mut old_clip = Vec::new();
            let mut old_save = Vec::new();
            let mut new_total = Vec::new();
            let mut new_convert = Vec::new();
            let mut new_clip = Vec::new();
            let mut new_save = Vec::new();
            let (mut old_len, mut new_len) = (0_u64, 0_u64);
            let mut skipped = 0;

            for round in 0..rounds {
                let old_path =
                    std::env::temp_dir().join(format!("curseen_scroll_old_{height}_{round}.png"));
                let new_path =
                    std::env::temp_dir().join(format!("curseen_scroll_new_{height}_{round}.png"));

                let run_old = |old_total: &mut Vec<Duration>,
                               old_convert: &mut Vec<Duration>,
                               old_clip: &mut Vec<Duration>,
                               old_save: &mut Vec<Duration>|
                 -> bool {
                    let shot = canvas.clone();
                    let t0 = Instant::now();
                    let bmp = crate::core::backdrop::shot_to_bitmap(&shot).expect("借用转换");
                    let t1 = Instant::now();
                    let clip = timed_clip(|| screenshot::to_clipboard(&shot).is_ok());
                    let t2 = Instant::now();
                    screenshot::save_png_fast(shot, &old_path).expect("旧留档");
                    let t3 = Instant::now();
                    drop(bmp);
                    let Some(clip) = clip else { return false };
                    old_convert.push(t1 - t0);
                    old_clip.push(clip);
                    old_save.push(t3 - t2);
                    old_total.push(t3 - t0);
                    true
                };
                let run_new = |new_total: &mut Vec<Duration>,
                               new_convert: &mut Vec<Duration>,
                               new_clip: &mut Vec<Duration>,
                               new_save: &mut Vec<Duration>|
                 -> bool {
                    let shot = canvas.clone();
                    let t0 = Instant::now();
                    let bmp = Rc::new(
                        crate::core::backdrop::shot_to_bitmap_owned(shot).expect("消费转换"),
                    );
                    let t1 = Instant::now();
                    let clip = timed_clip(|| screenshot::to_clipboard_rgba(&bmp).is_ok());
                    let t2 = Instant::now();
                    screenshot::save_png_rgba_fast(&bmp, &new_path).expect("新留档");
                    let t3 = Instant::now();
                    drop(bmp);
                    let Some(clip) = clip else { return false };
                    new_convert.push(t1 - t0);
                    new_clip.push(clip);
                    new_save.push(t3 - t2);
                    new_total.push(t3 - t0);
                    true
                };

                let both = if reverse {
                    let new_ok = run_new(
                        &mut new_total,
                        &mut new_convert,
                        &mut new_clip,
                        &mut new_save,
                    );
                    let old_ok = run_old(
                        &mut old_total,
                        &mut old_convert,
                        &mut old_clip,
                        &mut old_save,
                    );
                    new_ok && old_ok
                } else {
                    let old_ok = run_old(
                        &mut old_total,
                        &mut old_convert,
                        &mut old_clip,
                        &mut old_save,
                    );
                    let new_ok = run_new(
                        &mut new_total,
                        &mut new_convert,
                        &mut new_clip,
                        &mut new_save,
                    );
                    old_ok && new_ok
                };
                if !both {
                    skipped += 1;
                }

                old_len = std::fs::metadata(&old_path).map(|m| m.len()).unwrap_or(0);
                new_len = std::fs::metadata(&new_path).map(|m| m.len()).unwrap_or(0);
                if round == 0 {
                    let a = std::fs::read(&old_path).expect("读旧留档");
                    let b = std::fs::read(&new_path).expect("读新留档");
                    assert_eq!(a, b, "快档产物的字节必须与旧路完全相同");
                }
                let _ = std::fs::remove_file(&old_path);
                let _ = std::fs::remove_file(&new_path);
            }

            println!(
                "留档字节 old={old_len} new={new_len}（round 0 逐字节相同；丢弃 {skipped} 轮）"
            );
            report("旧 total", &mut old_total);
            report("  旧 convert(借用)", &mut old_convert);
            report("  旧 clipboard(BGRA)", &mut old_clip);
            report("  旧 save_png_fast", &mut old_save);
            report("新 total", &mut new_total);
            report("  新 convert(消费)", &mut new_convert);
            report("  新 clipboard(RGBA)", &mut new_clip);
            report("  新 save_png_rgba_fast", &mut new_save);
        }
    }
}
