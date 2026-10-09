//! GIF：区域录制、条形工具条、裁剪窗口。
//!
//! 三个界面（选区 / 工具条 / 裁剪窗）的状态都挂在 Runtime 上，因为它们的生命周期
//! 比任何单个窗口都长（裁剪窗可能在录制结束之后才打开）。

use super::*;

impl Runtime {
    /// GIF 快捷键：**先框一块屏幕**，框好并不立刻录（等用户在边框外的按钮上点「开始」）。
    ///
    /// ★ 与 MP4 那条"一键开关"不一样：MP4 按下即倒计时，因为录整屏没什么可选的；
    ///   GIF 录整屏几乎永远不是用户想要的（无关内容 + 体积），所以多一步框选。
    ///
    /// ★ 再按一次快捷键 = 收起控件条（与点工具条上的「关闭」同义），键盘用户不必去够按钮。
    ///
    /// ★★ 录制中按下（用户 2026-10-03 口径）：**换格式**，不是"把当前这段停了什么都不开始"。
    ///   桌面复制同时只容一份，所以两路并行录制做不到，只能串行切换 —— 用户按一次键完成
    ///   切换，收尾完成后自动进 GIF 的选区流程（见 [`Runtime::switch_recording_to`]）。
    pub(crate) fn begin_gif_selection(&mut self) {
        // ★★ 顺序要紧：**录制中**这一支必须排在选框判定之前。
        //   反过来的话，录制期间按 GIF 快捷键会走 `gif_sel.is_some()` 分支只把控件条收掉，
        //   录制留在后台继续跑（而 GIF 快捷键本该和 MP4 那条一样是"一键开关"）。
        if self.recorder.is_some() {
            if self.recorder.as_ref().map(|r| r.kind()) == Some(RecKind::Gif) {
                // 正在录 GIF = 停止（与 MP4 的一键开关对称）。
                self.stop_recording_now();
            } else {
                // 正在录 MP4 = 换成 GIF（见 `Runtime::switch_recording_to`）。
                self.switch_recording_to(RecKind::Gif, None);
                self.action_notify.show("已停止录制，正在切换到 GIF…");
            }
            return;
        }
        if self.gif_sel.is_some() {
            self.close_gif_bar();
            return;
        }
        if self.rec_finishing {
            // 上一次还在收尾：排进待办，收尾完成自动进选区（桌面复制那时才释放）。
            if self.rec_after_finish.is_none() {
                self.rec_after_finish = Some(RecKind::Gif);
                self.rec_after_finish_crop = None;
                self.action_notify
                    .show("正在结束上一次录制，随后进入 GIF 选区…");
            }
            return;
        }
        self.begin_snap(snap::SnapMode::Gif);
    }

    /// 选区录屏快捷键：**先框一块屏幕**，框好弹控件条等「开始」（与 GIF 同一套 UI）。
    ///
    /// ★ 再按一次快捷键 = 收起控件条（与 GIF 那条同义）。
    /// ★ 录制进行中按下 = 同格式停止、异格式换过去（与 GIF 那条同一套语义）。
    pub(crate) fn begin_mp4_region(&mut self) {
        if self.recorder.is_some() {
            if self.recorder.as_ref().map(|r| r.kind()) == Some(RecKind::Mp4) {
                self.stop_recording_now();
            } else {
                self.switch_recording_to(RecKind::Mp4, None);
                self.action_notify.show("已停止录制，正在切换到选区录屏…");
            }
            return;
        }
        if self.gif_sel.is_some() {
            self.close_gif_bar();
            return;
        }
        if self.rec_finishing {
            if self.rec_after_finish.is_none() {
                self.rec_after_finish = Some(RecKind::Mp4);
                self.rec_after_finish_crop = None;
                self.action_notify.show("正在结束上一次录制，随后进入选区…");
            }
            return;
        }
        self.begin_snap(snap::SnapMode::Mp4Region);
    }

    /// 选框确认 → 记住这块矩形 → 让 overlay 弹出控件条。
    ///
    /// `rect` 是**冻结帧坐标系**（物理像素、相对虚拟桌面左上角，副屏在左时 x 为负），
    /// 与 `ScreenRect` 同一坐标系，录制器可以直接吃。
    pub(crate) fn arm_gif(&mut self, rect: Rect) {
        let r = crate::core::geometry::ScreenRect {
            x: rect.x.round() as i32,
            y: rect.y.round() as i32,
            w: rect.w.round().max(2.0) as i32,
            h: rect.h.round().max(2.0) as i32,
        };
        self.gif_sel = Some(r);
        self.sel_kind = RecKind::Gif;
        log::info!(
            "GIF 选框就绪：{}×{} @ ({}, {})，等「开始」",
            r.w,
            r.h,
            r.x,
            r.y
        );
        self.notify_gif_changed();
    }

    /// 选区录屏选框确认 → 记住这块矩形 → 弹出控件条等「开始」（与 GIF 完全同一套 UI）。
    ///
    /// ★ 选框/控件条/尺寸标签/可调选区全部复用 GIF 那一套（`gif_sel` 共用，`sel_kind`
    ///   标记归属）；差别只有两点：控件条「开始」上膛的是 MP4（录这块区域而非全屏），
    ///   「裁剪」剪刀开的是 MP4 时间裁剪窗（见 `request_gif_trim` 的分岔）。
    pub(crate) fn arm_mp4_region(&mut self, rect: Rect) {
        let r = crate::core::geometry::ScreenRect {
            x: rect.x.round() as i32,
            y: rect.y.round() as i32,
            w: rect.w.round().max(2.0) as i32,
            h: rect.h.round().max(2.0) as i32,
        };
        self.gif_sel = Some(r);
        self.sel_kind = RecKind::Mp4;
        log::info!(
            "选区录屏就绪：{}×{} @ ({}, {})，等「开始」",
            r.w,
            r.h,
            r.x,
            r.y
        );
        self.notify_gif_changed();
    }

    /// 选框状态变了 → 推一条消息让 UI 线程解 park、重绘、同步控件条。
    ///
    /// 没有 `ui_tx`（单元测试里的裸 `Runtime`，或覆盖层尚未起）时静默跳过：状态本身
    /// 已经改好了，只是没人需要被叫醒。
    pub(crate) fn notify_gif_changed(&self) {
        if let Some(tx) = &self.ui_tx {
            let _ = tx.send(crate::app::UiMsg::GifSelChanged);
        }
    }

    /// 当前 GIF 选框（覆盖层据此画边框，控件条据此定位）。
    ///
    /// ★ 它同时是**控件条窗口生死的唯一判据**（见 `overlay::sync_gif_bar`），所以除了
    ///   `arm_gif` / `close_gif_bar`，任何地方都不该直接改这个字段。
    pub fn gif_selection(&self) -> Option<crate::core::geometry::ScreenRect> {
        self.gif_sel
    }

    /// GIF 选框此刻**是否可交互**（显示手柄/边框并接收拖拽）。
    ///
    /// 三个条件同时成立才算可交互：
    /// - 有一块选框（`gif_sel` 存在）；
    /// - 不在录制 / 倒计时中（`!gif_armed()`）；
    /// - 没有其它叠层在抢鼠标（标注/区域选择未激活）。
    pub fn gif_interaction_enabled(&self) -> bool {
        self.gif_sel.is_some() && !self.gif_armed() && !self.layer().active()
    }

    /// GIF 选框**最小尺寸**（逻辑像素）。缩放被 clamp 到这个值为止。
    pub const GIF_ZONE_MIN_SIZE: i32 = 20;

    /// 应用一次拖拽（平移 / 缩放）到当前 GIF 选框。
    ///
    /// - `zone = Body`：整体平移 `(dx, dy)`。
    /// - 四个角 / 四条边中点：对应方向缩放。
    /// - 结果实时 clamp 进虚拟桌面（允许负原点），并保证不小于 `GIF_ZONE_MIN_SIZE`。
    ///
    /// `dx`/`dy` 是**物理像素**增量（世界坐标）。单位换算由调用方负责。
    ///
    /// ★ 必须是"每次用上一步终点当基准再叠一步"的**增量**语义：鼠标在拖拽中每帧都可能
    ///   丢（跨帧插值不可靠），一次性算总位移会在丢帧时把选框推到位移的两倍处。
    pub fn gif_resize_or_move(&mut self, zone: HitZone, dx: i32, dy: i32) {
        // 未命中任何交互带 = 无事发生：不吃掉这一笔，也不推"选框变了"的消息
        // （`apply_zone` 对 `None` 本身就是空转，这里多一条早退只是为了不白发通知）。
        if zone == HitZone::None {
            return;
        }
        let Some(mut r) = self.gif_sel else {
            return;
        };
        apply_zone(&mut r, self.screen, zone, dx, dy);
        self.gif_sel = Some(r);
        self.notify_gif_changed();
    }

    /// 这一刻是否正在录 GIF（控件条按钮的判定依据）。
    pub fn is_recording_gif(&self) -> bool {
        self.recorder
            .as_ref()
            .is_some_and(|r| r.kind() == RecKind::Gif)
    }

    /// GIF 是否已"上膛"：**倒计时中或正在录**。
    ///
    /// ★ 控件条那颗键画的是"按下去会发生什么"：上膛时它是**停止/取消**（红圆），
    ///   未上膛时才是**开始**（绿三角）。只用 `is_recording_gif()` 会漏掉 3 秒倒计时那一段
    ///   —— 那期间按键的语义已经变成"取消"了（见 [`Self::toggle_gif_from_bar`]），
    ///   图标却还停在"开始"，与用户刚点完一次的动作对不上。
    pub fn gif_armed(&self) -> bool {
        let rec_kind = self.recorder.as_ref().map(|r| r.kind());
        rec_kind == Some(self.sel_kind)
            || (self.rec_countdown_ms.is_some() && self.rec_pending == Some(self.sel_kind))
    }

    /// 测试用：把状态摆成"GIF 已上膛"（有一块选框 + 正在倒计时）。
    ///
    /// 供控件条/覆盖层的**离屏渲染门禁**用 —— 真机上要走到这个状态得先快捷键再拖框，
    /// 而提权窗口收不到鼠标注入（见 `examples/render_probe.rs`）。
    #[cfg(test)]
    pub fn armed_gif_for_test(&mut self) {
        self.gif_sel = Some(crate::core::geometry::ScreenRect {
            x: 0,
            y: 0,
            w: 320,
            h: 240,
        });
        self.rec_pending = Some(RecKind::Gif);
        self.rec_countdown_ms = Some(1000.0);
    }

    /// 测试用：把状态摆成"GIF 选框就绪未上膛"（可交互、可拖拽）。
    ///
    /// 与 [`Self::armed_gif_for_test`] 的差别就在**不**摆倒计时——`gif_interaction_enabled`
    /// 要求 `!gif_armed()`，上膛态的交互门是关的。供鼠标模式/拖拽门禁用。
    #[cfg(test)]
    pub fn ready_gif_for_test(&mut self) {
        self.gif_sel = Some(crate::core::geometry::ScreenRect {
            x: 0,
            y: 0,
            w: 320,
            h: 240,
        });
    }

    /// 控件条上的「开始 / 停止」：一颗键走完两种状态。
    ///
    /// ★ 这颗键在界面上是**画出来的图形**（未上膛绿三角 / 上膛红圆，见 `layer::gif_bar`），
    ///   所以它没有"文字要不要跟着状态改"的问题 —— 那是独立窗口方案的旧包袱。
    pub fn toggle_gif_from_bar(&mut self) {
        let kind = self.sel_kind;
        // 正在录**这种格式** → 停。录着 GIF 时按 GIF 条上的停止、录着选区 MP4 时按同一条
        // 的停止，都走这里 —— 一条控件条只管自己那种格式。
        if self.recorder.as_ref().map(|r| r.kind()) == Some(kind) {
            self.stop_recording_now();
            return;
        }
        // 录着**另一种格式** → 换过去（一条控件条也能换格式，见 `switch_recording_to`）。
        if self.recorder.is_some() {
            let sel = self.gif_sel;
            self.switch_recording_to(kind, sel);
            self.action_notify.show("已停止录制，正在切换格式…");
            return;
        }
        if self.rec_countdown_ms.is_some() {
            // 倒计时中再点 = 取消（与快捷键在倒计时里的行为一致）。
            self.rec_pending = None;
            self.rec_countdown_ms = None;
            self.pending_crop = None;
            log::info!("录制倒计时已取消");
            return;
        }
        // ★ 上一段还在收尾：排进待办而不是拒绝（桌面复制那时才释放，见字段注释）。
        if self.rec_finishing {
            if self.rec_after_finish.is_none() {
                self.rec_after_finish = Some(kind);
                self.rec_after_finish_crop = self.gif_sel;
                self.action_notify.show("正在结束上一次录制…");
            }
            return;
        }
        let Some(sel) = self.gif_sel else {
            return;
        };
        // ★ MP4 选区录制走 `pending_crop` 通道（`start_recording_now` 消费它）；GIF 直接
        //   吃 `gif_sel`，不用这步。倒计时期间选框还活着，用户反悔取消也随时能清。
        if kind == RecKind::Mp4 {
            self.pending_crop = Some(sel);
        }
        self.rec_pending = Some(kind);
        self.rec_countdown_ms = Some(REC_COUNTDOWN_MS);
        log::info!("{kind:?} 录制倒计时开始（{} ms）", REC_COUNTDOWN_MS as u32);
    }

    /// 收起控件条与边框（工具条「关闭」按钮 / 再按一次快捷键）。
    ///
    /// ★ 控件条是**独立窗口**，本函数碰不到它（`Runtime` 不持有 HWND）；窗口的生死由
    ///   overlay 侧按 `gif_selection()` 同步（见 `overlay::sync_gif_bar`）。这里只改状态
    ///   并推消息——所以关闭**不会**出现"选框没了、按钮还挂在那里、点了没反应"。
    pub fn close_gif_bar(&mut self) {
        // ★ ✕ 在**上膛态**（倒计时 / 录制中）= 退出整个录制，绝不是只收边框：
        //   - 倒计时 → 取消。字段与 toggle 的取消路径完全同套（rec_pending /
        //     rec_countdown_ms / pending_crop 三清）——pending_crop 漏清的话，
        //     下一次普通录屏会误裁到这块旧区域（见该字段注释）。
        //   - 录制中 → 停录（与「停止」键同义；录着不放 ✕ 会让录制在零 UI 的
        //     状态下继续跑，用户无从收拾）。
        //   （2026-10-08 用户实测：倒计时里点 ✕，边框/手柄退了、录制却照常开始。）
        if self.rec_countdown_ms.is_some() && self.rec_pending == Some(self.sel_kind) {
            self.rec_pending = None;
            self.rec_countdown_ms = None;
            self.pending_crop = None;
            log::info!("倒计时中点 ✕：取消本次录制");
        } else if self.recorder.as_ref().map(|r| r.kind()) == Some(self.sel_kind) {
            self.stop_recording_now();
            log::info!("录制中点 ✕：停止录制");
        }
        if self.gif_sel.take().is_some() {
            log::info!("GIF 选框已收起");
        }
        self.notify_gif_changed();
    }

    /// 控件条上的「裁剪」（剪刀）：请求打开编辑窗。
    ///
    /// ★ 裁剪的对象**不是**当前的选框：
    ///   - 刚录好了 GIF（`last_gif` 有值）→ [`TrimRequest::Last`]：载入最近这次真的落了盘
    ///     的那份，顺手把选框/控件条收起（"这段录制结束了，接下来是编辑"）。
    ///   - 没录过 → [`TrimRequest::Blank`]：**空开编辑器**，让用户自己去「打开…」选一个
    ///     磁盘上的 GIF（用户 2026-09-20 口径："没录制，就空开编辑器，用户自己会选择gif打开"）。
    pub fn request_gif_trim(&mut self) {
        // ★ 控件条是 GIF / 选区录屏共用的，剪刀跟着 `sel_kind` 走：MP4 选框上的剪刀
        //   开的是 MP4 时间裁剪窗（用户口径："裁剪按钮也成了视频裁剪用的"）。
        if self.sel_kind == RecKind::Mp4 {
            self.request_mp4_trim();
            return;
        }
        if self.last_gif.is_some() {
            self.queue_trim(TrimRequest::Last(TrimKind::Gif));
        } else {
            self.queue_trim(TrimRequest::Blank(TrimKind::Gif));
        }
    }

    /// 录制完成提示窗的「裁剪」：按**刚落盘的 GIF 产物**走，不吃
    /// [`Self::request_gif_trim`] 的 `sel_kind` 转发——提示窗可能开很久（用户手动关），
    /// 期间用户完全可以再框一个 MP4 选区把 `sel_kind` 改掉，那都与"这份产物"无关。
    pub fn request_gif_trim_from_toast(&mut self) {
        if self.last_gif.is_some() {
            self.queue_trim(TrimRequest::Last(TrimKind::Gif));
        } else {
            self.queue_trim(TrimRequest::Blank(TrimKind::Gif));
        }
    }

    /// 裁剪快捷键：**独立打开裁剪器**（[`TrimRequest::Blank`]），不依赖刚录过什么。    ///
    /// ★ 窗口已经开着时什么都不用做 —— 开窗那条路走的是 `.single("gif_trim")`，框架会
    ///   把已有的那个窗聚焦而不是重建内容（见 `layer::gif_trim::spawn`）。这一点很重要：
    ///   用户在裁剪到一半时误按快捷键，手上的选择与已删段**不该被清掉**。
    ///
    /// ★ 与 [`Self::request_gif_trim`] 的另一个差别：**不动 GIF 选框**。那条路的语义是
    ///   "这段录制结束了，去编辑它"，顺手收起边框是对的；快捷键跟选框毫无关系，收掉它反而
    ///   会让用户莫名其妙地丢掉刚框好的区域。
    ///
    /// ★ 聚焦用 [`focus_window_by_title`](crate::platform::win_util::focus_window_by_title)
    ///   而**不是** `activate_window_by_title`：后者会把窗口搬进置顶带，之后它就一直压在
    ///   原生文件对话框上面（用户 2026-09-20 报告"打开 gif 时全被遮挡"）。裁剪窗要弹
    ///   对话框，就必须留在普通 z 序档里。
    pub fn request_gif_trim_standalone(&mut self) {
        // `focus_window_by_title` 找不到窗口时返回 `false` 且**什么都不做**（`FindWindowW`
        // 失败即返回），所以它正好兼职"存在吗"的探测 —— 不必再开一个 `window_exists_*`。
        if crate::platform::win_util::focus_window_by_title(layer::gif_trim::TITLE) {
            log::info!("裁剪快捷键：窗口已开着，聚焦它");
            return;
        }
        self.queue_trim(TrimRequest::Blank(TrimKind::Gif));
    }

    /// MP4 时间裁剪（控件条/提示窗「裁剪」那条路）：刚录好 → 载入 `last_mp4`；没录过 → 空开。
    pub fn request_mp4_trim(&mut self) {
        if self.last_mp4.is_some() {
            self.queue_trim(TrimRequest::Last(TrimKind::Mp4));
        } else {
            self.queue_trim(TrimRequest::Blank(TrimKind::Mp4));
        }
    }

    /// MP4 时间裁剪快捷键：独立打开裁剪器，不依赖刚录过什么。
    ///
    /// 与 GIF 那条同一条纪律：窗口已开着时只聚焦，不清用户手上的选择。
    pub fn request_mp4_trim_standalone(&mut self) {
        if crate::platform::win_util::focus_window_by_title(layer::mp4_trim::TITLE) {
            log::info!("MP4 裁剪快捷键：窗口已开着，聚焦它");
            return;
        }
        self.queue_trim(TrimRequest::Blank(TrimKind::Mp4));
    }

    /// 登记"该开裁剪窗了"并叫醒 UI 线程（两个入口共用的尾巴）。
    pub(crate) fn queue_trim(&mut self, what: TrimRequest) {
        self.gif_trim_req = Some(what);
        if let Some(tx) = &self.ui_tx {
            let _ = tx.send(crate::app::UiMsg::GifTrimRequested);
        }
    }

    /// overlay 取走"该开裁剪窗了"这个意图（边沿触发，取走即清）。
    pub fn take_gif_trim_request(&mut self) -> Option<TrimRequest> {
        self.gif_trim_req.take()
    }

    /// 裁剪窗是否开着（覆盖层据此给它让路：保持穿透 + 挂起标注输入）。
    pub fn gif_trim_open(&self) -> bool {
        self.gif_trim_open
    }

    /// 覆盖层要不要给**我们自己的独立顶层窗**让开穿透（面板 / 裁剪窗）。
    ///
    /// 判据只此一处，散开写就会出现"加了新窗口忘了加进判据"（[`LayerSession::click_through`]
    /// 只认这个布尔量，它不知道有几种窗口）。**加新窗时必须来这里补一笔**。
    ///
    /// 与之配套的是 [`Self::input_suspended`]：让路 = 鼠标能点到那个窗，挂起 = 点的时候不
    /// 顺手在画布上画一笔。两者必须同时算上同一个窗口，只补一边的症状分别是"点不动"和
    /// "点一下就多一笔"。
    pub fn overlay_yields_to_our_windows(&self) -> bool {
        self.panel_open() || self.gif_trim_open
    }

    /// 裁剪窗开出来了（由覆盖层在开窗那一步登记）。
    ///
    /// 必须与 [`Self::close_gif_trim`] 成对：只登记不收尾的话，覆盖层会**永久**保持穿透，
    /// 标注/区域选择就再也收不到鼠标了（"进得去、动不了"那类死态的翻版）。
    pub fn note_gif_trim_opened(&mut self) {
        self.gif_trim_open = true;
    }

    /// 裁剪窗关闭时回调（收尾在窗口自己那边做完；这里只记日志 + 清状态）。
    pub fn close_gif_trim(&mut self) {
        self.gif_trim_open = false;
        log::info!("GIF 裁剪窗已关闭");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::testkit::*;

    /// GIF 平移：正常移动 + 越界 clamp（负原点）。
    #[test]
    fn gif_move_clamps_to_screen() {
        let mut rt = drawing_only();
        rt.gif_sel = Some(ScreenRect::new(100, 100, 200, 100));
        rt.gif_resize_or_move(HitZone::Body, 50, 30);
        assert_eq!(rt.gif_sel, Some(ScreenRect::new(150, 130, 200, 100)));
        // 越到屏幕右下方之外 → clamp 不越界，尺寸不变。
        rt.gif_resize_or_move(HitZone::Body, 10_000, 10_000);
        assert_eq!(
            rt.gif_sel,
            Some(ScreenRect::new(1720, 980, 200, 100)),
            "右边缘=x+w=1920，下边缘=1080"
        );
        // 负方向越界 clamp 回 0 原点（单屏左上）。
        rt.gif_resize_or_move(HitZone::Body, -10_000, -10_000);
        assert_eq!(rt.gif_sel, Some(ScreenRect::new(0, 0, 200, 100)));
    }

    /// GIF 缩放：拖动边/角保最小尺寸、不进虚拟桌面之外、负原点仍不变形。
    #[test]
    fn gif_resize_clamps_and_keeps_min() {
        let min = Runtime::GIF_ZONE_MIN_SIZE;
        let mut rt = drawing_only();
        rt.gif_sel = Some(ScreenRect::new(100, 100, 300, 200));
        // 拖右下角往里缩小，直到被最小尺寸挡住。
        rt.gif_resize_or_move(HitZone::CornerBR, -10_000, -10_000);
        assert_eq!(
            rt.gif_sel,
            Some(ScreenRect::new(100, 100, min, min)),
            "右下角拖到底：只留最小 20x20"
        );
        // 拖右上角：右/上边动，左边不动；尺寸守恒。
        rt.gif_sel = Some(ScreenRect::new(100, 100, 300, 200));
        rt.gif_resize_or_move(HitZone::CornerTR, 100, -40);
        assert_eq!(rt.gif_sel, Some(ScreenRect::new(100, 60, 400, 240)));
        // 右上角往右超出虚拟桌面底部范围 → 右边缘被 clamp 到 1920。
        rt.gif_resize_or_move(HitZone::CornerTR, 10_000, 0);
        assert_eq!(rt.gif_sel, Some(ScreenRect::new(100, 60, 1820, 240)));
        // 左上角拖向右下，直到最小尺寸 + 不进负区。
        rt.gif_sel = Some(ScreenRect::new(100, 100, 300, 200));
        rt.gif_resize_or_move(HitZone::CornerTL, 10_000, 10_000);
        assert_eq!(
            rt.gif_sel,
            Some(ScreenRect::new(100 + 300 - min, 100 + 200 - min, min, min))
        );
        // 左下角往下拖出虚拟桌面 → 下边被 clamp 到 1080。
        rt.gif_sel = Some(ScreenRect::new(100, 100, 300, 200));
        rt.gif_resize_or_move(HitZone::CornerBL, 0, 10_000);
        assert_eq!(rt.gif_sel, Some(ScreenRect::new(100, 100, 300, 980)));
    }

    /// GIF 缩放支持负原点（副屏左侧），clamp 只在虚拟桌面内。
    #[test]
    fn gif_resize_negative_origin() {
        let min = Runtime::GIF_ZONE_MIN_SIZE;
        let mut rt = drawing_only();
        rt.set_screen(ScreenRect::new(-1920, -100, 3840, 1180));
        rt.gif_sel = Some(ScreenRect::new(-1920, -100, 400, 300));
        rt.gif_resize_or_move(HitZone::CornerTL, 10_000, 10_000);
        assert_eq!(
            rt.gif_sel,
            Some(ScreenRect::new(
                -1920 + 400 - min,
                -100 + 300 - min,
                min,
                min
            )),
            "负原点缩放仍只留最小尺寸、不进屏幕外"
        );
    }

    /// GIF 选区面板移动/缩放后仍调用内部更新（此处只验证 `None` 不碰选框）。
    #[test]
    fn gif_resize_none_is_noop() {
        let mut rt = drawing_only();
        rt.gif_sel = Some(ScreenRect::new(10, 10, 50, 50));
        rt.gif_resize_or_move(HitZone::None, 0, 0);
        assert_eq!(rt.gif_sel, Some(ScreenRect::new(10, 10, 50, 50)));
    }

    /// ★ 上膛（倒计时）中点 ✕ = **取消整个录制**（2026-10-08 用户实测：边框/手柄退了、
    /// 录制却照常开始，退无可退）。三个字段必须与 toggle 的取消路径同清——
    /// `pending_crop` 漏清会让下一次普通录屏误裁到这块旧区域（见该字段注释）。
    #[test]
    fn close_gif_bar_cancels_the_pending_countdown() {
        let mut rt = drawing_only();
        rt.gif_sel = Some(ScreenRect::new(100, 100, 200, 100));
        rt.sel_kind = RecKind::Gif;
        rt.rec_pending = Some(RecKind::Gif);
        rt.rec_countdown_ms = Some(1000.0);
        rt.pending_crop = Some(ScreenRect::new(100, 100, 200, 100));

        rt.close_gif_bar();
        assert!(rt.rec_pending.is_none(), "倒计时必须被取消");
        assert!(rt.rec_countdown_ms.is_none(), "倒计时钟必须清零");
        assert!(
            rt.pending_crop.is_none(),
            "MP4 裁剪矩形不许残留（否则下一次普通录屏会误裁到旧区域）"
        );
        assert!(rt.gif_sel.is_none(), "选框照常收起");
    }

    /// 未上膛时 ✕ 依旧只收选框：不碰倒计时字段（没挂就是没挂，不得凭空造出副作用）。
    #[test]
    fn close_gif_bar_while_not_armed_only_closes_the_selection() {
        let mut rt = drawing_only();
        rt.gif_sel = Some(ScreenRect::new(100, 100, 200, 100));
        rt.sel_kind = RecKind::Gif;

        rt.close_gif_bar();
        assert!(rt.gif_sel.is_none(), "选框收起");
        assert!(rt.rec_pending.is_none() && rt.rec_countdown_ms.is_none());
        assert!(rt.pending_crop.is_none());
    }

    /// 两个裁剪入口开出来的窗**初始状态不同**，请求必须把这件事带过去。
    ///
    /// 这条同时也是"取走即清"的门禁：`take` 之后再取必须是 `None`，否则 overlay 每
    /// 250ms 对账一次就会反复开窗。
    #[test]
    fn the_two_trim_entries_are_distinguishable_and_the_request_is_taken_once() {
        // 控件条「裁剪」：刚录好 → 带 `last_gif`；没录过 → 空开编辑器（`Blank`，用户自选文件）。
        let mut rt = Runtime::new(Config::default());
        assert_eq!(rt.last_gif(), None);
        rt.request_gif_trim();
        assert_eq!(
            rt.take_gif_trim_request(),
            Some(TrimRequest::Blank(TrimKind::Gif)),
            "没录过时按裁剪应空开编辑器（等用户自己打开一个 GIF）"
        );

        rt.note_recording_output(std::path::PathBuf::from("C:/tmp/a.gif"), RecKind::Gif);
        assert_eq!(
            rt.last_gif(),
            Some(std::path::PathBuf::from("C:/tmp/a.gif"))
        );
        rt.request_gif_trim();
        assert_eq!(
            rt.take_gif_trim_request(),
            Some(TrimRequest::Last(TrimKind::Gif))
        );
        assert_eq!(rt.take_gif_trim_request(), None, "取走即清");

        // 裁剪快捷键：**任何时候**都能排队（不要求录过什么），且是空窗。
        let mut rt = Runtime::new(Config::default());
        assert_eq!(rt.last_gif(), None);
        rt.request_gif_trim_standalone();
        assert_eq!(
            rt.take_gif_trim_request(),
            Some(TrimRequest::Blank(TrimKind::Gif))
        );
        assert_eq!(rt.take_gif_trim_request(), None);
    }

    /// 裁剪窗开/关必须**成对**地驱动两件事：覆盖层给它让路（穿透）与标注输入挂起。
    ///
    /// ★ 为什么这条值得单独立门禁：裁剪窗不再置顶之后（见《踩坑记录》§2.7），它能不能被点到就
    ///   完全取决于这两条 —— 漏了让路 ⇒ **点不动**；漏了挂起 ⇒ 点一下就**多画一笔**。
    ///   而漏了**复位**更坏：覆盖层永久穿透 / 标注永久挂起，要等用户再开一次再关一次裁剪窗
    ///   才能恢复，从现象上完全看不出和裁剪窗有关。
    #[test]
    fn the_trim_window_drives_both_the_overlay_yield_and_the_input_suspension() {
        let mut rt = Runtime::new(Config::default());
        assert!(!rt.gif_trim_open());
        assert!(
            !rt.overlay_yields_to_our_windows(),
            "什么窗都没开时不该让路"
        );
        assert!(!rt.input_suspended(), "什么窗都没开时标注输入该是活的");

        rt.note_gif_trim_opened();
        assert!(rt.gif_trim_open());
        assert!(
            rt.overlay_yields_to_our_windows(),
            "裁剪窗开着时覆盖层必须让开穿透，否则用户点不到它"
        );
        assert!(
            rt.input_suspended(),
            "让路必须与挂起配套：不挂起的话，用户点裁剪窗上一下就在画布上多一笔"
        );

        rt.close_gif_trim();
        assert!(!rt.gif_trim_open());
        assert!(
            !rt.overlay_yields_to_our_windows(),
            "关掉之后要让路也一起收回"
        );
        assert!(!rt.input_suspended(), "关掉之后标注输入必须恢复");
    }

    /// 裁剪快捷键要在**空闲（park）时**也能唤醒覆盖层，否则"平时按一下没反应"。
    #[test]
    fn the_trim_hotkey_wakes_the_overlay() {
        let mut cfg = Config::default();
        // ★ 2026-09-26 晚起裁剪快捷键**出厂留空**（= 停用），依赖它的测试必须显式配一把，
        //   否则整条用例会因为"串是空的"而退化成"测了个寂寞"（右侧 ref muted 分支恒不成立）。
        cfg.gif.trim_hotkey = "Alt+Shift+T".into();
        let rt = Runtime::new(cfg.clone());
        let ev = || InputEvent::KeyDown {
            key: KeyCode::Char('t'),
            mods: Modifiers {
                alt: true,
                shift: true,
                ..Default::default()
            },
            repeated: false,
            time_ms: 0,
        };
        assert_eq!(cfg.gif.trim_hotkey, "Alt+Shift+T", "测试用例显式配的键");
        // 先把「事件 ↔ 配置串」这一对钉住：下面那条断言才有意义（否则事件和串各自写错、
        // 恰好互相抵消成"通过"）。
        assert!(
            crate::core::hotkeys::matches(
                &cfg.gif.trim_hotkey,
                KeyCode::Char('t'),
                Modifiers {
                    alt: true,
                    shift: true,
                    ..Default::default()
                }
            ),
            "出厂串和测试事件对不上"
        );
        assert!(rt.action_hotkey_wakes(&ev()), "裁剪快捷键必须能唤醒覆盖层");
        // 停用（空串）后不该再唤醒。
        let mut off = cfg.clone();
        off.gif.trim_hotkey = String::new();
        assert!(!Runtime::new(off).action_hotkey_wakes(&ev()));
    }

    /// 真机上一按就出窗的那条链路：`handle_action_key` 必须把裁剪快捷键翻译成一条
    /// `TrimRequest::Blank`。
    ///
    /// 上面那条只测了 `request_gif_trim_standalone()` **自己**，而"按键事件 → 排请求"是
    /// 配置串与动作之间唯一的一根线；它断了的表现就是最难查的那种——"快捷键完全没反应"，
    /// 单元测试却全绿（因为两边各自都是对的）。
    #[test]
    fn the_trim_hotkey_enqueues_a_blank_trim_request() {
        let mut cfg = Config::default();
        cfg.gif.enabled = true; // 出厂总开关已改关（2026-09-25 晚），测试显式打开
        cfg.gif.trim_hotkey = "Alt+Shift+T".into(); // 出厂已留空（2026-09-26 晚），测试显式配一把
        let mut rt = Runtime::new(cfg);
        let key = |repeated: bool| InputEvent::KeyDown {
            key: KeyCode::Char('t'),
            mods: Modifiers {
                alt: true,
                shift: true,
                ..Default::default()
            },
            repeated,
            time_ms: 0,
        };

        rt.handle_action_key(&key(false));
        assert_eq!(
            rt.take_gif_trim_request(),
            Some(TrimRequest::Blank(TrimKind::Gif)),
            "裁剪快捷键该排一条**空窗**请求（不是 LastGif —— 它跟刚录的那个无关）"
        );

        // 长按的自动重复不该反复排：按住不放会连开好几次窗。
        rt.handle_action_key(&key(true));
        assert_eq!(rt.take_gif_trim_request(), None, "重复键不该再排请求");

        // 把快捷键清空（面板上"留空 = 停用"）后，同一个按键不该再触发。
        rt.config.gif.trim_hotkey = String::new();
        rt.handle_action_key(&key(false));
        assert_eq!(rt.take_gif_trim_request(), None, "停用后按键不该有反应");
    }

    // ─── 截图编辑态（`Ctrl` + 截图快捷键；会话纯逻辑见 `shot_edit`）────────────────
    //
    // 这里只测**会话接线**：进/出、拖拽事件的归属、几何钳位。产物的像素判据在
    // `shot_edit::tests`（离屏合成，不需要桌面会话）。
}
