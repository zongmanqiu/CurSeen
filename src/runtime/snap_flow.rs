//! 截图流程：框选 → 编辑态 → 保存 / 贴图。
//!
//! 屏上交互的**那一半**状态机（`ShotEdit` 本身在 `shot_edit.rs`）。
//! 出口只有两个：工具栏「保存（另存为）」与 `Esc`（放弃）——`Enter` 在这里**无反应**。

use super::*;

/// 读剪贴板内容并转成贴图条目（文本优先）。
///
/// **文本也栅格化成图片**（用户要求："剪贴板文字转 jpg/png 图片"）：贴到桌面上的是图，
/// 不再是那种"深色圆角框 + 白字"的伪文本窗——后者既不能存也不能复制。
/// 文本读到了但栅格化失败（极小概率）就退回试图像。
pub(crate) fn read_clipboard_content(screen: ScreenRect) -> Option<snap::PinContent> {
    let dpi = crate::platform::display::dpi_scale();
    if let Some(text) = crate::platform::clipboard::get_text() {
        let max_w = ((text_raster::MAX_W_LOGICAL as f32) * dpi)
            .min(screen.w as f32 * 0.9)
            .max(160.0);
        if let Some(shot) = text_raster::render_text(&text, max_w as i32, dpi) {
            if let Some(b) = crate::core::backdrop::shot_to_bitmap_owned(shot) {
                return Some(snap::PinContent::Image(Rc::new(b)));
            }
        }
    }
    crate::platform::clipboard::get_image()
        .and_then(crate::core::backdrop::shot_to_bitmap_owned)
        .map(|b| snap::PinContent::Image(Rc::new(b)))
}

/// 文件名友好的本地时间戳 `YYYY-MM-DD_HH-MM-SS`。
pub(crate) fn timestamp() -> String {
    chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string()
}

// ─── 录制倒计时 ───

impl Runtime {
    /// 截图 / 贴图快捷键的入口（★ 三个快捷键共用同一条底层链）。
    ///
    /// GDI 冻结帧一次抓取整张**虚拟桌面**（不是光标所在显示器——跨屏框选要它）→ 置入
    /// 状态机 → 由 overlay 通道回调打开区域选择窗。抓屏阻塞 UI 线程 release 实测
    /// median 约 33 ms（1920×1080），位图转换已改成就地换序（median 3 ms；旧路径 12 ms
    /// 且峰值多一份 8.3 MB）。
    ///
    /// ★ 抓屏用的 `screenshot::grab` 与屏幕注释的底图是**同一个函数**，区别只在冻结帧
    /// 的用途（用户要求"截图与屏幕画板调用同一个函数"）。
    pub(crate) fn begin_snap(&mut self, mode: snap::SnapMode) {
        if self.snap.pending.is_some() {
            return; // 区域选择已在进行中
        }
        // ★ 编辑态里再按截图快捷键 = "重新框一块"：先把上一轮会话收掉。
        //
        //   不收的话，新框选确认时 `enter_shot_edit` 会直接覆盖 `edit` 字段，上一轮的
        //   冻结帧与已经画好的笔迹被**静默**丢掉；而那次确认之前，屏上还挂着上一轮的
        //   选区与工具栏（`DrawingController` 停在 `ShotEdit` 档），看着像"框选卡住了"。
        //   显式收口之后，这个动作只剩一种解释：重新来一次。
        if self.edit.is_some() {
            log::info!("编辑态里按了截图快捷键：放弃上一轮编辑，重新框选");
            self.cancel_shot_edit();
        }
        // ★ 截图/贴图是独立行为，**不再**收起控制面板（用户口径 2026-09-17：面板
        //   开着也照常截图；区域窗本身是全屏置顶窗，盖在面板之上，互不干扰）。
        match screenshot::grab(self.screen) {
            Ok(shot) => {
                // 原帧只用来转位图，转完即弃 → 就地换序（省一份 8.3 MB 与一次拷贝）。
                let (w, h) = (shot.w, shot.h);
                match crate::core::backdrop::shot_to_bitmap_owned(shot) {
                    Some(bmp) => {
                        self.snap.begin(mode, Rc::new(bmp));
                        log::info!("截图区域选择开始（{mode:?}，{w}×{h}）");
                    }
                    None => {
                        log::warn!("冻结帧尺寸非法：{w}×{h}");
                        self.action_notify.show("截图失败");
                    }
                }
            }
            Err(e) => {
                log::warn!("冻结屏幕失败: {e}");
                self.action_notify.show("截图失败");
            }
        }
    }

    /// 区域选择确认（区域窗拖拽松手 / 回车）：裁剪 → 剪贴板 → 临时留档 →
    /// 反馈（白闪）→ ShotPin 模式追加贴图请求。
    ///
    /// ★ GIF / 选区录屏 / 长截图只取**矩形**，在进剪贴板/留档之前就分岔出去（见下方
    /// 两处提前返回）——它们不是截图，不能有截图出口的副作用。
    ///
    /// `rect` 是**冻结帧坐标系**（物理像素，相对冻结帧左上角；单显示器下也就是虚拟桌面
    /// 坐标）——换算由区域窗经 `layer::WorldMap` 完成，本函数不做任何 DPI 推理。
    pub fn confirm_snap(&mut self, rect: Rect) {
        // ★ 先收放大镜再干活：它是一块独立置顶窗，下面几条 `return` 路径也带不走它。
        self.region_magnifier.hide();
        // ★★ 进"截图**编辑态**"有**两个**入口，二者是**或**关系（2026-09-23 用户口径修正）：
        //
        //   ① 按下快捷键那一刻按着 Ctrl —— `resolve_snap_hotkey` 已把模式定成 `ShotEdit`；
        //   ② **框选阶段**按住 Ctrl —— "先按快捷键、再按住 Ctrl 拖框"。
        //
        //   ★ 从前只有 ①。用户报（2026-09-23）：*"我使用快捷键触发后，就是只需要鼠标左键
        //     进行选区。但是我按住 Ctrl 却无法实现选区后的调整，那就是错的。这个和我用什么
        //     快捷键不应该有关系。"* 而且 *"连我们原来已经实现的功能都破坏了"* ——
        //     因为 `Ctrl+F3` 这类**本身含 Ctrl** 的快捷键，`with_ctrl` 拼不出变体（物理上
        //     不存在"再按住 Ctrl"），只剩本体一条路；拖框时 Ctrl 一直按着也白按。
        //     只有把判定扩到 ②，这类配置与这类手势才都回到编辑态。
        //
        //   判定放在**这里**（松手确认那一刻）有两个理由：
        //     · `pending` 只在这段会话里活着，过了这一步整张帧就没了；
        //     · 用户按 Ctrl 的时机本来就在"框选"这一段，取此刻的键态最贴近他的意图。
        //
        //   ① 的优先级不受影响：`ShotEdit` 已经带着 `pin`，直接采用（不重判 Ctrl，
        //   否则"按 Ctrl+截图并贴图"会在松开 Ctrl 时退化成不贴图）。
        //
        //   ★ 这里**不裁**：编辑阶段要整张帧当画布（用户还会拖选区、放大镜要取样），
        //   真正的裁剪推迟到 `Enter` 那一刻（见 [`Self::confirm_shot_edit`]）。
        //   判据取 `pending_mode()` 而**不是** `confirm` 之后再看 `mode`：`confirm` 会把
        //   `pending` 消费掉，那时候整张帧已经没了。
        if let Some(mode) = self.snap.pending_mode() {
            // "要不要进编辑态"的判定收在 [`shot_edit_entry`] 里（纯函数、可测）。
            let pin = shot_edit_entry(mode, crate::platform::input::keyboard::ctrl_held());
            if let Some(pin) = pin {
                match self.snap.take_pending_frame() {
                    Some((frame, _)) => self.enter_shot_edit(rect, frame, pin),
                    None => log::warn!("截图编辑态取帧失败：会话已被消费"),
                }
                return;
            }
        }
        let Some((cropped, mode)) = self.snap.confirm(rect, self.screen) else {
            return;
        };
        // ★★ 两种「要的是矩形、不是截图」的模式在这里分岔，**必须排在剪贴板/留档之前**：
        //   - 选区录屏：确认即进倒计时（一键开关，不需要 GIF 那种控件条）；
        //   - GIF 选框：只把矩形交给控件条等「开始」。
        //   剪贴板 / 留档 / 白闪都是「截图」语义，套在「要录哪一块」上是错的——用户
        //   没截图，把剪贴板冲掉是纯副作用（m08：GIF 分支原来排在两个副作用之后）。
        match mode {
            snap::SnapMode::Mp4Region => {
                self.arm_mp4_region(rect);
                return;
            }
            snap::SnapMode::Gif => {
                self.arm_gif(rect);
                return;
            }
            _ => {}
        }
        // ★ 长截图 / 宽截图：确认那一刻同样**不走截图出口**（不裁帧、不进剪贴板、
        //   不留档、不白闪 —— 与选区录屏同一条理由，那些都是「截图」语义）。矩形交
        //   `arm_scroll` 进就绪态；产物在**收工**那一刻才写入剪贴板。方向由触发键定。
        match mode {
            snap::SnapMode::Scroll => self.arm_scroll(rect, false),
            snap::SnapMode::ScrollWide => self.arm_scroll(rect, true),
            _ => {}
        }
        if matches!(mode, snap::SnapMode::Scroll | snap::SnapMode::ScrollWide) {
            return;
        }
        // ★ 裁剪结果本身就是非预乘 RGBA：剪贴板与留档都从它直接出发，不再造一份整帧
        //   BGRA 中间帧（旧路 `bitmap_to_shot` 既有一次整帧分配，又让剪贴板再拷一遍、
        //   快档 PNG 再换序一遍）。
        // ★ 一定进剪贴板（原「存入剪贴板」开关删除：截图就是要能直接粘出去）。
        if let Err(e) = screenshot::to_clipboard_rgba(&cropped) {
            log::warn!("区域截图写入剪贴板失败: {e}");
        } else {
            // ★ 记下"这次写入的序列号"。轮询与贴图快捷键看到它就知道剪贴板里是我们自己刚截的图，
            //   不要再当"外部复制"读一遍——那会丢掉原位（用户口径 2 的根因）。
            let n = crate::platform::clipboard::sequence_number();
            self.own_clip_seq = Some(n);
            self.clip_seq = n; // 顺带让 250ms 轮询跳过这一条（同一张图不必再进一次环）
        }
        // 临时留档：`<exe>/temp/screenshot/`。用户要留下的那份走贴图右键「另存为」，
        // 这里只保证"程序自己产生的东西有个固定落点"，便于排查与回溯。
        // ★ 走快档：留档在确认的同步路径上（白闪要等它），而用户从来看不到这个文件；
        //   1920×1080 实测默认档 median 122–123 ms、快档 14–15 ms（文件 2.9×，只落在 temp）。
        let (sw, sh) = (cropped.w, cropped.h);
        let dir = crate::core::paths::module_temp_dir(crate::core::paths::MODULE_SCREENSHOT);
        let path = dir.join(format!("CurSeen_{}.png", timestamp()));
        let saved = screenshot::save_png_rgba_fast(&cropped, &path).is_ok();
        // ★★ 两种模式都**记录原位**（用户口径 2，2026-09-17）：*"只要是通过我们的功能截图的，
        //    就要记录信息。这样贴图的时候才方便。"*
        //    - ShotPin：立刻贴图 + 原位（`pin_push` 同时入环并请求开窗）；
        //    - Shot  ：只入环**并带上原位**，不弹窗。以前这一支什么都不做，于是纯截图的图
        //      只能靠剪贴板轮询"捡"进环，而轮询那条路给不出原位 → 用户后来按贴图键时
        //      落在屏幕正中（这正是用户报的现象）。
        match mode {
            // ★ GIF / 选区录屏：已由上方「确认即交矩形」的分岔提前返回处理，正常走不到
            //   这里；保留这一臂只为让匹配在编译期收敛（它们不会落到截图语义）。
            snap::SnapMode::Gif | snap::SnapMode::Mp4Region => {
                unreachable!("Gif/Mp4Region 应由 confirm_snap 的提前返回处理")
            }
            snap::SnapMode::Scroll | snap::SnapMode::ScrollWide => {
                // ★ 与 Mp4Region 同款：确认那一刻已提前返回给 `arm_scroll`。
                unreachable!("Scroll/ScrollWide 应由 confirm_snap 的提前返回处理")
            }
            snap::SnapMode::ShotPin => {
                self.snap
                    .pin_push(snap::PinContent::Image(cropped), Some(rect));
            }
            snap::SnapMode::Shot => {
                self.snap.note_shot(snap::PinContent::Image(cropped), rect);
            }
            // ★ 编辑态在函数开头就分岔走了（那条路**不裁帧**，整张冻结帧交给编辑会话），
            //   正常永远走不到这一支。留着是为了让编译器知道四种模式都有人管；真到这儿
            //   说明 `SnapMgr::confirm` 与 `pending_mode()` 的分岔被改坏了——报出来比静默
            //   吞掉好（症状会表现为"按住 Ctrl 截图，出来的是没笔迹的裁剪块"）。
            snap::SnapMode::ShotEdit { .. } => {
                log::error!("截图编辑态走到了普通截图出口（模式分岔失效）");
                return;
            }
        }
        // 白闪：截图成功唯一且不可错过的反馈（原「保存后提示」浮字已随功能删除）。
        self.action_notify.flash();
        log::info!(
            "区域截图完成：{}×{}（{mode:?}，留档={saved}，{}）",
            sw,
            sh,
            path.display()
        );
    }

    // ─── 画笔的跨模块动作（`DrawRequest`）────────────────────────────────────
    //
    // 五个出口汇到一处：截图编辑态的 `Esc`（取消）/ 工具栏的「保存」「退出」；
    // 屏幕标注 / 白板的「另存为」/ `Esc`（退出即留档）。
    // 前四者由 `DrawingModule::handle_key` 与 `DrawingController::apply_action` 产出，
    // 后两者同理；`frame()` 每帧取一次并汇到这里 —— 走同一条路，所以"点按钮"与"按 Esc"
    // 在产物上不可能有差别（用户口径：*"我们这个功能是组合前两个功能的。这样也能节约代码，
    // 也不会错。"*）。

    /// 执行这一帧的画笔请求（没有就什么都不做）。
    pub(crate) fn handle_draw_request(&mut self) {
        let Some(req) = self.drawing.take_request() else {
            return;
        };
        match req {
            DrawRequest::ShotEditConfirm => self.confirm_shot_edit(true),
            // 打勾「完成」：同一份产物，**不弹**另存为框（用户已经说过"就这张"）。
            DrawRequest::ShotEditDone => self.confirm_shot_edit(false),
            DrawRequest::ShotEditCancel => self.cancel_shot_edit(),
            // 「另存为」= 保存 + **结束**这次会话：原生保存对话框是模态的，而覆盖层此刻
            // 铺满整屏且整屏收鼠标（`on_nchit` 回 `Client`）—— 不收掉覆盖层，对话框既看不见
            // 也点不着。所以两条路都是"先合成、再收工、最后弹框"。
            DrawRequest::AnnotateSaveAs => {
                if let Some(bmp) = self.compose_annotation() {
                    self.drawing.stop_quiet();
                    self.stash_save_as(&bmp, "CurSeen");
                } else {
                    log::warn!("屏幕标注另存为：画面合成失败");
                    self.drawing.stop_quiet();
                }
            }
            DrawRequest::AnnotateExit => {
                // ★ 先合成**再** stop：`stop()` 会把那张整屏冻结帧还给系统（8–33 MB），
                //   顺序反了就只剩墨迹、底图没了。
                let had_ink = self.drawing.has_ink();
                let composed = self.compose_annotation();
                let saved = composed
                    .as_ref()
                    .and_then(crate::modules::drawing::controller::save_annotation_png);
                drop(composed);
                match saved {
                    Some(path) => {
                        log::info!("屏幕标注已留档: {}", path.display());
                        // ★ 用 `stop_quiet()`：中央的位置只够一句话，下面那句"已保存到"
                        //   要占掉它。目录名由常量拼出（改目录时提示跟着变，不会再出现
                        //   "提示说 A、文件掉在 B"）。
                        self.drawing.stop_quiet();
                        self.action_notify.show(&format!(
                            "已保存到　temp\\{}",
                            crate::core::paths::MODULE_ANNOTATION
                        ));
                    }
                    // ★ 什么都没画 → `compose_annotation` 返回 `None`，这是**正常**路径
                    //   （不留"和桌面一模一样"的文件），不是错误。只有"画过东西却没存成"
                    //   才值得进日志。
                    None => {
                        if had_ink {
                            log::warn!("屏幕标注退出留档失败（合成或 PNG 编码失败）");
                        }
                        // 没有"已保存到"要报，就把「已退出绘图」还给用户 —— 按 Esc 总得
                        // 有个回应，静默退出会让人怀疑"到底退没退"。
                        self.drawing.stop();
                    }
                }
                // ★ 会话里现抓的整屏底图已经进了渲染图片缓存（`render::IMG_CACHE`，
                //   一条 = Rc 8.1MB + WImage 8.1MB）。退出即清：缓存外的 Rc 已随 stop
                //   释放，没人再画得出那张图 —— 不清就是"开关三次 20→70MB"的泄漏。
                crate::runtime::render::clear_image_cache();
            }
        }
    }

    /// 屏幕标注 / 白板的产物：**离屏合成**（底图 + 墨迹）→ 非预乘 RGBA。
    ///
    /// ★ 不抓屏（旧做法，已弃）：抓屏要么在屏上留 150ms 的"无工具栏帧"（用户看着工具栏
    ///   闪一下），要么把退出推迟到抓捕完成 —— 而退出是即时语义；更糟的是它拍到的是**屏幕**，
    ///   会把此刻盖在上面的别的窗口、以及我们自己的工具栏一起拍进产物。离屏合成与屏上
    ///   **同源**（同一个 `Backdrop::paint_into` + 同一份笔迹指令表），且不需要任何等待。
    ///
    /// 什么都没画时返回 `None`（退出留档的判据：不留"和桌面一模一样"的文件）。
    pub(crate) fn compose_annotation(&mut self) -> Option<RgbaBitmap> {
        if !self.drawing.has_ink() {
            return None;
        }
        // ★ 透明幕布：画的时候手里没有底图，出图前补一张"此刻的桌面"（藏在
        //   `platform::screenshot::grab_hiding` 里：先藏掉覆盖层再抓，否则工具栏会被
        //   拍进产物 —— 旧的"抓屏派"就是这么被否掉的）。抓不到就照旧只出墨迹。
        if self.drawing.needs_live_backdrop() {
            let title = crate::platform::topmost::OVERLAY_TITLE;
            match crate::platform::screenshot::grab_hiding(title, self.screen) {
                Ok(shot) => {
                    if let Some(bmp) = crate::core::backdrop::shot_to_bitmap_owned(shot) {
                        self.drawing
                            .set_backdrop(Backdrop::Frozen(std::rc::Rc::new(bmp)));
                    }
                }
                Err(e) => log::warn!("屏幕标注：现抓桌面失败（{e}），产物将只有墨迹（透明底）"),
            }
        }
        let screen = self.screen;
        let (w, h) = (screen.w.max(1) as u32, screen.h.max(1) as u32);
        let mut list = DrawList::new();
        self.drawing.compose_into(&mut list);
        let mut buf = vec![0_u8; (w as usize) * (h as usize) * 4];
        {
            use windui::render::RenderTarget as _;
            let mut engine = windui::text::DWriteEngine::new();
            let mut target = windui::render::DibTarget::new(&mut buf, w, h);
            let mut canvas = target.make_canvas(&mut engine, 1.0);
            // 世界物理坐标 → 画面原点：把虚拟桌面原点搬到 (0,0)，1:1 不缩放。与屏上
            // （`scale = 1/dpi` 再让平台乘回 dpi，恒等）**逐像素一致**。
            crate::runtime::render::translate_offset(
                &list,
                &mut *canvas,
                1.0,
                -(screen.x as f32),
                -(screen.y as f32),
            );
        }
        RgbaBitmap::new(w, h, buf)
    }

    /// 把一张待存盘的图交给 UI 线程去弹「另存为」（本帧只登记，不弹框）。
    ///
    /// ★ 为什么不能就地弹：原生对话框自带消息泵、会阻塞到松手为止，而本函数是**在
    ///   `frame()` 里**跑的 —— 那时 `&mut Runtime` 还被借用着，对话框一旦抽消息，
    ///   覆盖层的窗口过程就会回来再借一次同一份状态（windui 铁律 6）。所以登记 + 发一条
    ///   `UiMsg`，由 UI 线程的事件分发点 `ctx.defer_blocking` 去执行（与「贴图 Ctrl+S」
    ///   同款，见 `overlay::save_shot_as`）。
    pub(crate) fn stash_save_as(&mut self, bmp: &RgbaBitmap, prefix: &str) {
        let Some(shot) = crate::core::backdrop::bitmap_to_shot(bmp) else {
            log::warn!("另存为：位图无法转回帧格式");
            return;
        };
        let name = format!("{prefix}_{}", timestamp());
        self.pending_save_as = Some((shot, name));
        if let Some(tx) = &self.ui_tx {
            let _ = tx.send(UiMsg::SaveAsReady);
        }
    }

    /// 取走待存盘的那张图（UI 线程在对话框前后各取一次，见 `overlay::save_shot_as`）。
    pub fn take_pending_save_as(&mut self) -> Option<(Shot, String)> {
        self.pending_save_as.take()
    }

    /// 框选完成 → 进入截图编辑态。
    ///
    /// `rect` 是区域窗给的**冻结帧内**像素矩形；编辑会话内部一律用**世界物理**坐标
    /// （笔迹、压暗、工具栏锚点全是世界坐标），所以这里做唯一一次换算。
    pub(crate) fn enter_shot_edit(&mut self, rect: Rect, frame: Rc<RgbaBitmap>, pin: bool) {
        let sel = clamp_sel(
            ScreenRect {
                x: rect.x.round() as i32 + self.screen.x,
                y: rect.y.round() as i32 + self.screen.y,
                w: rect.w.round().max(1.0) as i32,
                h: rect.h.round().max(1.0) as i32,
            },
            self.screen,
        );
        // ★ 画布就是**刚才框选用的那张冻结帧**（不是现抓一帧）：用户看到的、以及即将被裁进
        //   图片的，是开始截图那一刻的画面。另抓一帧会晚于它，笔迹就落在与图片对不上的位置
        //   上（表现是"我画的圈没圈住那个按钮"）。这也是"截图与屏幕标注在本质上是同一个
        //   函数"最直白的体现：同一份底图，只换了入口。
        self.drawing
            .start_with_backdrop(Backdrop::Frozen(frame.clone()), sel_rect(sel));
        self.edit = Some(ShotEdit::new(sel, frame, pin));
        self.notify_edit_changed();
        log::info!(
            "进入截图编辑态：{}×{} @ ({}, {})，pin={pin}",
            sel.w,
            sel.h,
            sel.x,
            sel.y
        );
    }

    /// 编辑态**确认**：冻结帧 + 笔迹 → 剪贴板 → 留档 → 白闪（+ 按 `pin` 决定是否贴图）。
    ///
    /// 与 [`Self::confirm_snap`] 是同一个出口（剪贴板 / 临时留档 / 白闪三件事一件不少），
    /// 差的只是"图从哪来"：那边是刚裁下的子帧，这边要现合成（底图 + 笔迹）。
    ///
    /// # `save_as`：要不要再弹一次原生对话框
    ///
    /// - `true` = 工具栏「保存」：除了默认位置的留档，还要让用户挑落点。两者不冲突 ——
    ///   默认留档是"程序自己产生的东西有个固定落点"（便于排查回溯），另存为是"我要的这份
    ///   放哪儿"。
    /// - `false` = 工具栏那颗打勾「完成」（★ 2026-10-03）：用户已经明确说过"就这张"，
    ///   再弹框是**多余的第二次确认**——这正是用户报"底部只有退出、没有 OK"时缺的那一档。
    ///
    /// 除这一句以外两条路**完全一致**（同一段合成、同一串副作用），所以差异收在参数里，
    /// 不在这里分叉成两份实现。
    pub(crate) fn confirm_shot_edit(&mut self, save_as: bool) {
        let Some(e) = std::mem::take(&mut self.edit) else {
            return;
        };
        // ★ 先取材再收工：`stop()` 会清掉笔迹历史（顺带释放那张 8–33 MB 的冻结帧），
        //   顺序反了就合成出一张干干净净的底图 —— 用户画的东西全没了。
        let mut ink = DrawList::new();
        self.drawing.controller().draw_annotations_into(&mut ink);
        self.drawing.controller_mut().stop();
        self.region_magnifier.hide();

        let Some(bmp) = shot_edit::compose(&e, &ink, self.screen) else {
            log::warn!("截图编辑产物合成失败（选区与冻结帧不相交）");
            self.action_notify.show("截图失败");
            return;
        };
        // 合成结果本身就是非预乘 RGBA：剪贴板与留档直接吃它，不再转一份 Shot。
        if let Err(err) = screenshot::to_clipboard_rgba(&bmp) {
            log::warn!("截图编辑写入剪贴板失败: {err}");
        } else {
            // 与区域截图同款：记下序列号，免得剪贴板轮询把自家这张又当"外部复制"读一遍
            // （那会丢掉原位）。
            let n = crate::platform::clipboard::sequence_number();
            self.own_clip_seq = Some(n);
            self.clip_seq = n;
        }
        // 与区域截图同款：内部留档走快档（用户在等白闪，且看不到这个文件）。
        let (sw, sh) = (bmp.w, bmp.h);
        let dir = crate::core::paths::module_temp_dir(crate::core::paths::MODULE_SCREENSHOT);
        let path = dir.join(format!("CurSeen_{}.png", timestamp()));
        let saved = screenshot::save_png_rgba_fast(&bmp, &path).is_ok();

        // 入环（+ 按需立刻贴图）：原位取**世界**坐标的选区，贴图窗才会落在用户框的那一块上。
        let origin = e.region();
        // ★「保存」那条路还要把同一张图交给另存为对话框（落盘用 `Shot`），先登记；
        //   「打勾完成」没有对话框，直接把位图移进贴图环——旧路无条件 clone 一份整帧。
        if save_as {
            self.stash_save_as(&bmp, "CurSeen");
        }
        let content = snap::PinContent::Image(Rc::new(bmp));
        if e.pin {
            self.snap.pin_push(content, Some(origin));
        } else {
            self.snap.note_shot(content, origin);
        }
        // 白闪：截图成功唯一且不可错过的反馈。紧接着再补一句"存哪儿了"——用户口径
        // （2026-09-23）：*"截图，Ctrl 进行调整和编辑，同样可以进行保存。但是保存时没有提示。"*
        // 两条是同一对象上的两个通道（白闪是全屏白幕、浮字是中央横幅），可同时存在。
        self.action_notify.flash();
        self.action_notify.show(&format!(
            "已保存到　temp\\{}",
            crate::core::paths::MODULE_SCREENSHOT
        ));
        log::info!(
            "截图编辑完成：{}×{}（pin={}，留档={saved}，{}）",
            sw,
            sh,
            e.pin,
            path.display()
        );
        // 落点由用户挑的那条路已在入环前登记（`save_as`）：对话框是延后弹的，这里不再做事。
    }

    /// 编辑态**取消**：整次截图作废（`Esc` / 工具栏的「退出」）。
    ///
    /// 不留任何痕迹：不进剪贴板、不留档、不白闪 —— 与区域选择阶段按 Esc 的语义一致。
    pub(crate) fn cancel_shot_edit(&mut self) {
        let Some(e) = std::mem::take(&mut self.edit) else {
            return;
        };
        self.drawing.controller_mut().stop();
        self.region_magnifier.hide();
        log::info!("截图编辑态取消（选区 {}×{}）", e.sel.w, e.sel.h);
    }

    /// 编辑态是否活着（覆盖层的命中/光标/边框都按它分流）。
    pub fn shot_edit_active(&self) -> bool {
        self.edit.is_some()
    }

    /// ★ 取消**所有**进行中的截图会话（功能开关被关掉时调用，见
    /// `Runtime::reclaim_disabled_modules`）。
    ///
    /// 三种会话各自取消，不留痕迹：编辑态 / 长截图就绪态 / 长截图采集中。
    /// 为什么不做成"只清字段"：长截图采集态背后有**工作线程**在跑（`ScrollCapture`），
    /// 直接把 `self.scroll` 丢掉就是让线程泄漏、边框永远留在屏上。必须走既有的
    /// `cancel_scroll` —— 它会停线程并丢弃画布。
    ///
    /// 贴图窗**不在此列**：它是钉在桌面上的独立窗口，用户可以钉着图继续用别的功能，
    /// 关掉"截图"开关不该让桌面上的图消失。
    pub(crate) fn cancel_capture_sessions(&mut self) {
        if self.edit.is_some() {
            self.cancel_shot_edit();
        }
        // 一次调用就够：`cancel_scroll` 内部先看就绪态、再看采集态。
        if self.scroll.is_some() || self.scroll_ready.is_some() {
            self.cancel_scroll();
        }
    }

    /// 屏幕标注（透明幕布）是否正占着指针输入（`DrawMode != Off`）。
    ///
    /// 覆盖层的 `on_nchit` 据此在标注态对**所有点**回 `Client`。为什么必须：标注态
    /// 覆盖层虽已清 `WS_EX_TRANSPARENT`（`MouseMode::Interactive`），但命中测试最终
    /// 听 `WM_NCHITTEST` 的逐点回答——这里漏了屏幕标注就恒回 `HTTRANSPARENT`，笔的
    /// `WM_POINTER`、`WM_TABLET_QUERYSYSTEMGESTURESTATUS` 全被送进桌面，windui 补丁
    /// 092/093 成了死代码，起笔只能等翻译层迟到 ~0.5s 的鼠标副本（数位板"起笔延迟"
    /// 四报的最终根因，2026-09-29 定位）。鼠标不走此路也正常：`WH_MOUSE_LL` 不经
    /// 命中测试，这就是它一直没暴露本洞的原因。
    pub fn drawing_owns_pointer(&self) -> bool {
        !matches!(
            self.drawing.controller().mode(),
            crate::modules::drawing::controller::DrawMode::Off
        )
    }

    /// 编辑态的选区（**世界物理像素**）——覆盖层据此画边框与 8 个手柄。
    pub fn shot_edit_selection(&self) -> Option<ScreenRect> {
        self.edit.as_ref().map(|e| e.sel)
    }

    /// 编辑态的冻结帧（放大镜取样用；屏上那块已经被我们自己的编辑画布盖住了，
    /// 实时抓屏只会抓到"我们画的框"）。
    pub fn shot_edit_frame(&self) -> Option<Rc<RgbaBitmap>> {
        self.edit.as_ref().map(|e| e.frame.clone())
    }

    /// 编辑态在 `(x, y)`（世界物理）这一点的交互带 —— 拖拽与悬停光标共用同一套几何。
    pub fn shot_edit_zone_at(&self, x: i32, y: i32) -> HitZone {
        match &self.edit {
            Some(e) => e.zone_at(x, y, crate::platform::display::dpi_scale()),
            None => HitZone::None,
        }
    }

    /// ★ 编辑态的**选区拖拽**：吃掉落在交互带上的按下，以及随后的移动与抬手。
    ///
    /// 返回 `true` = 这条事件被编辑会话吃掉（不再进输入队列），于是标注看不到它、也就不会
    /// 在"调选区"的同时顺手画出一笔。框内与框外的按下**照常放行**（那是画笔的地盘，
    /// 压暗区也能画）。
    ///
    /// # 为什么走全局钩子而不是窗口事件
    ///
    /// GIF 那条（`overlay::OverlayCanvas::on_event`）走窗口事件，是因为它需要
    /// `ctx.capture()` 把拖动中的移动全揽过来。编辑态的交互可以完全落在 `Runtime` 手里，
    /// 而全局钩子事件**与窗口命中无关** —— 覆盖层此刻是不是穿透、NCHITTEST 把它判给了谁，
    /// 事件都照样到得了这里。少一层"窗口必须先拿到鼠标"的前提，就少一类
    /// "框内能画、边框拖不动"的怪状态（GIF 那条正是踩过这个坑才不得不改帧驱动的）。
    pub(crate) fn edit_drag(&mut self, ev: &InputEvent) -> bool {
        let dpi = crate::platform::display::dpi_scale();
        match *ev {
            InputEvent::MouseDown {
                button: MouseButton::Left,
                x,
                y,
                ..
            } => {
                // ★ 工具栏优先：工具条与选区边框带**可能重叠**（选区几乎铺满屏幕时它会退回
                //   屏幕底边，而选区底边很可能正好压在那里）。点在按钮上就**放行**，让画笔
                //   去执行那个动作；否则这一下会被当成"拖动选区"吃掉 —— 表现为"某些位置上
                //   的按钮点不动"，而用户完全看不出原因（工具栏明明画在那儿）。
                if self.drawing.toolbar_hit(Point {
                    x: x as f32,
                    y: y as f32,
                }) {
                    return false;
                }
                let Some(zone) = self
                    .edit
                    .as_ref()
                    .map(|e| e.zone_at(x, y, dpi))
                    .filter(|z| *z != HitZone::None)
                else {
                    return false;
                };
                if let Some(e) = self.edit.as_mut() {
                    e.drag = Some((zone, x, y));
                }
                true
            }
            InputEvent::MouseMove { x, y, .. } => {
                let Some((zone, lx, ly)) = self.edit.as_ref().and_then(|e| e.drag) else {
                    return false;
                };
                let screen = self.screen;
                if let Some(e) = self.edit.as_mut() {
                    apply_zone(&mut e.sel, screen, zone, x - lx, y - ly);
                    // ★ 增量语义：每一步以上一步的**终点**为基准。一次性算总位移会在丢帧时
                    //   把选区推到位移的两倍处（与 `gif_resize_or_move` 同一条理由）。
                    e.drag = Some((zone, x, y));
                }
                true
            }
            InputEvent::MouseUp {
                button: MouseButton::Left,
                ..
            } => self.edit.as_mut().is_some_and(|e| e.drag.take().is_some()),
            _ => false,
        }
    }

    /// 进入编辑态 → 推一条消息让 UI 线程解 park 并立即重绘。
    ///
    /// 与 [`Self::notify_gif_changed`] 同一个理由：确认框选那一刻是**区域选择窗**在处理事件，
    /// 覆盖层很可能正 park 着（用户没开任何需要出帧的功能），不踢它一脚，编辑界面
    /// （底图 + 压暗 + 工具栏）要等到下一次自然唤醒（250ms 对账）才出现。
    pub(crate) fn notify_edit_changed(&self) {
        if let Some(tx) = &self.ui_tx {
            let _ = tx.send(UiMsg::ShotEditChanged);
        }
    }

    /// 取冻结帧位图（区域选择窗开窗时构建 `WImage` 用）。
    pub fn snap_frame_bitmap(&self) -> Option<Rc<RgbaBitmap>> {
        self.snap.frame_bitmap()
    }

    /// **一次区域选择会话是否开着**（见 [`SnapMgr::is_pending`]）。
    ///
    /// ★ 光标放大镜的**所有权判据**：会话在跑时归区域选择窗（源 = 它那份冻结帧），
    ///   否则归主覆盖层（GIF 调整阶段，源 = 实时抓屏）。两边同时出帧时必须二选一 ——
    ///   主覆盖层能不能出帧取决于用户开没开拖尾/标注之类的功能，靠"它大概在 park 吧"
    ///   来分工是不可靠的。
    pub fn region_session_open(&self) -> bool {
        self.snap.is_pending()
    }

    /// 取消区域选择（Esc / 单击 / 关窗）。
    pub fn cancel_snap(&mut self) {
        self.region_magnifier.hide();
        self.snap.cancel();
        log::info!("截图区域选择取消");
    }

    /// 贴图快捷键：系统剪贴板有新内容 → 贴"倒数第一"；否则沿贴图环回溯。
    ///
    /// 剪贴板轮询已经把两次按键之间的复制收进了环，所以这里读到的剪贴板通常与环尾相同
    /// ——那正是"沿环回溯"该走的分支。
    ///
    /// ★ 例外：序列号等于 [`Self::own_clip_seq`] 时（剪贴板里是我们刚截的那张）不读剪贴板，
    /// 直接沿环回溯——那张已经在环里且带着原位。
    pub fn pin_next(&mut self) {
        // ★ 剪贴板里如果就是"我们自己刚截的那张"，**不要读**：它已经在环里且带着原位。
        //   把 `None` 交给状态机 = "剪贴板没有新东西"，于是沿环回溯到那张截图，原位还在
        //   （用户口径 2：纯截图后按贴图键必须落回截图那一块，而不是屏幕正中）。
        let cur = crate::platform::clipboard::sequence_number();
        let clip = if self.own_clip_seq == Some(cur) {
            None
        } else {
            read_clipboard_content(self.screen)
        };
        match self.snap.pin_next(clip) {
            Some(req) => {
                log::info!("贴图请求：{}", req.content.describe());
            }
            None => {
                self.action_notify.show("剪贴板没有可贴的内容");
            }
        }
    }

    /// 取"该开区域选择窗了"的边沿请求。
    pub fn take_snap_request(&mut self) -> bool {
        self.snap.take_open_request()
    }

    /// 取"该开贴图窗了"的边沿请求。
    pub fn take_pin_request(&mut self) -> Option<snap::PinRequest> {
        self.snap.take_pin_request()
    }

    /// 登记一个已关闭的贴图窗（用户 Esc / 双击右键关掉的）。
    pub fn note_pin_closed(&mut self, seq: u32) {
        self.snap.note_pin_closed(seq);
    }

    /// 区域选择窗关闭 → 取消（幂等）。
    pub fn snap_cancel(&mut self) {
        self.snap.cancel();
    }

    // ─── 叠层（截图 / 屏幕注释共用）──

    /// 叠层活跃态快照。**穿透判据与 Esc 优先级都从它派生**，别在别处再写一份。
    pub fn layer(&self) -> LayerSession {
        LayerSession {
            region: self.snap.pending.is_some(),
            annotate: self.drawing.controller().is_drawing(),
            pins: self.snap.open_pin_count(),
            pin_selected: self.snap.selected_pin().is_some(),
        }
    }

    /// Esc 该关谁（见 [`LayerSession::escape_target`] 的优先级表）。
    pub fn escape_target(&self) -> EscapeTarget {
        self.layer()
            .escape_target(self.panel_open(), self.text_window_open)
    }

    // ─── 贴图选中态（Esc 关一张 / Ctrl+S 另存一张）──

    /// 选中某张贴图（贴图窗左键按下时调用）。返回上一个选中项，供调用方把它重绘掉
    /// （取消它的"选中"高亮）——不重绘那一个窗口，它的高亮会一直留到下次重绘。
    pub fn select_pin(&mut self, seq: u32) -> Option<u32> {
        let old = self.snap.select_pin(seq);
        if let Some(old) = old {
            crate::platform::win_util::invalidate_by_title(&layer::pin::pin_title(old));
        }
        old
    }

    /// 当前选中的贴图序号。
    pub fn selected_pin(&self) -> Option<u32> {
        self.snap.selected_pin()
    }

    /// 这张贴图是不是当前选中项（贴图窗自绘高亮用）。
    pub fn pin_is_selected(&self, seq: u32) -> bool {
        self.snap.selected_pin() == Some(seq)
    }

    /// 指针按下时的选中态对账：点不在选中的那张贴图里 → 取消选中。
    ///
    /// 为什么要取消：`Ctrl+S` 是**全局**键（走低级键盘钩子），选中态若粘住不放，用户
    /// 在别的程序里按 Ctrl+S 就一直被我们拦下来另存图片。所以"点到别处"必须解除。
    ///
    /// 判据做成**几何**的（查一次窗口矩形）而不是"靠窗口消息"：贴图窗自己的
    /// `WM_LBUTTONDOWN` 与本函数收到的全局钩子事件谁先谁后不确定，只有"点落在哪"与
    /// 顺序无关——压在选中的那张上就保持，落在别处就取消。
    pub fn note_pointer_down(&mut self, x: i32, y: i32) {
        let Some(seq) = self.snap.selected_pin() else {
            return;
        };
        let title = layer::pin::pin_title(seq);
        match crate::platform::win_util::window_rect_by_title(&title) {
            Some((wx, wy, ww, wh)) => {
                let (x1, y1) = (wx.saturating_add(ww), wy.saturating_add(wh));
                if !(x >= wx && x < x1 && y >= wy && y < y1) {
                    self.snap.clear_selection();
                    log::info!("取消贴图选中 #{seq}（点击落在该窗之外）");
                }
            }
            // 窗口已经不在了（陈旧记账）：顺手摘掉，免得 Esc / Ctrl+S 指向空气。
            None => self.snap.note_pin_closed(seq),
        }
    }

    /// 取走并关闭**选中的那一张**贴图（Esc 出口）。没选中任何一张时返回 `None`。
    pub fn close_selected_pin(&mut self) -> Option<u32> {
        self.snap.take_selected()
    }

    /// 选中的那张贴图是否处于锁定态。
    ///
    /// 供 Esc 那条通路区分"没选中"与"选中了但关不掉"（锁定豁免，见
    /// [`snap::SnapMgr::take_selected`]）—— 两种都返回 `None`，用户体感却完全不同：
    /// 一种是"Esc 不管我的事"，另一种是"按了没反应"。后者必须给反馈。
    pub fn selected_pin_is_locked(&self) -> bool {
        self.snap
            .selected_pin()
            .is_some_and(|seq| self.snap.pin_locked(seq))
    }

    /// Esc 关不掉锁定贴图时的提示。
    ///
    /// 单独成函数只为把文案收在一处：它是用户唯一一次"Esc 不管用"的解释，
    /// 散在调用点里改文案容易漏。
    pub fn notify_pin_locked_cannot_close(&mut self) {
        self.action_notify.show("该图片已锁定，右键可解除锁定");
    }

    /// 某张贴图是否锁定（贴图画布双击守卫 / 菜单勾选都要问）。
    pub fn pin_locked(&self, seq: u32) -> bool {
        self.snap.pin_locked(seq)
    }

    /// 设置某张贴图的锁定态（右键「锁定图片」）。
    pub fn set_pin_locked(&mut self, seq: u32, locked: bool) {
        self.snap.set_pin_locked(seq, locked);
    }

    /// 某张贴图正在显示的内容（`Ctrl+S` 另存为用）。
    pub fn pin_content(&self, seq: u32) -> Option<snap::PinContent> {
        self.snap.pin_content(seq)
    }

    /// 剪贴板轮询：把**两次按键之间**的复制也收进贴图环。
    ///
    /// 只读一个 u32 序列号，比读一次图文内容便宜几个数量级，所以可以挂在既有的 250ms
    /// 对账上而不付任何代价。没变的序列号直接跳过。
    ///
    /// ★ 一定开启（原「监听剪贴板」开关删除）。内存上界由**贴图数量上限**兜住：
    /// 轮询收进来的内容进的是同一个贴图环，环按 `pin_ring_cap` 裁剪，所以"监听"再多也只
    /// 常驻 N 张位图——这也正是用户说的"根据贴图数量上限，只需要监听一部分而已"。
    ///
    /// 两个贴图快捷键都停用时整个跳过：没有任何出口能用上这些内容，不必积累。
    pub(crate) fn poll_clipboard(&mut self) {
        // ★ 截图总开关关着就整段跳过（2026-10-24）：贴图/贴剪贴板快捷键的派发在
        //   `handle_action_key` 里同样受 `screenshot.enabled` 管（`input.rs`），关着时
        //   没有任何出口能用上这里收进来的内容。此前只判快捷键非空，关掉截图后仍每
        //   250ms 读剪贴板、继续往贴图环里囤位图。
        if !self.config.screenshot.enabled {
            return;
        }
        if self.config.screenshot.hotkey_pin_clipboard.is_empty()
            && self.config.screenshot.hotkey_pin.is_empty()
        {
            return;
        }
        let n = crate::platform::clipboard::sequence_number();
        if n == self.clip_seq {
            return;
        }
        self.clip_seq = n;
        if let Some(c) = read_clipboard_content(self.screen) {
            if self.snap.note_external(c) {
                log::info!("剪贴板内容入贴图环（共 {} 条）", self.snap.ring_len());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::testkit::*;

    /// ★ 截图总开关是贴图环轮询的总闸（2026-10-24，用户报「全关还有东西在跑」）：
    /// 关着时连剪贴板序列号都不该读。开的那一半必须有反应——否则这条门写成恒
    /// `return` 也是绿的（对照臂）。
    #[test]
    fn clipboard_polling_needs_the_screenshot_switch() {
        let mut rt = drawing_only();
        assert!(!rt.config.screenshot.enabled, "前提：夹具截图总开关是关的");
        // 出厂贴图快捷键是空的；没有出口时轮询本来就会停，测不到总开关。
        rt.config.screenshot.hotkey_pin_clipboard = "F3".into();
        let sentinel = crate::platform::clipboard::sequence_number().wrapping_add(1_000_000);
        rt.clip_seq = sentinel;
        rt.poll_clipboard();
        assert_eq!(rt.clip_seq, sentinel, "总开关关着 → 不该碰剪贴板");

        rt.config.screenshot.enabled = true;
        rt.poll_clipboard();
        assert_ne!(rt.clip_seq, sentinel, "总开关开着 → 必须读到序列号");
    }

    fn png_count(dir: &std::path::Path) -> usize {
        std::fs::read_dir(dir)
            .map(|it| {
                it.filter_map(Result::ok)
                    .filter(|e| {
                        e.path()
                            .extension()
                            .is_some_and(|x| x.eq_ignore_ascii_case("png"))
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    /// ★★ m08 回归锁：GIF 选框确认**只交矩形**，不能碰截图出口的两件事。
    ///
    /// 缺陷现场（2026-10-24 静态审查 + 探针复现）：剪贴板写入与 `temp/screenshot` 留档
    /// 排在 `match mode` 之前，于是每次确认 GIF 选框都会把用户剪贴板换成一张裁剪图，
    /// 并留下一个用户不会用到的 PNG。
    ///
    /// 三条读数必须一起看：
    /// - `gif_selection()`：证明 GIF 分支真的跑到了（否则"没副作用"可能只是没走这条路）；
    /// - `own_clip_seq` / `clip_seq`：该 Runtime 自己有没有写过剪贴板（生产写入口会记账）；
    /// - PNG 数量：`save_png_rgba_fast` 真的没被调用。
    ///
    /// ★ 不断言全局剪贴板序列号：本机是共享桌面，别的进程改剪贴板会让它假红；本 Runtime
    ///   的记账字段才是"这一笔是谁写的"的判据。
    #[test]
    fn gif_confirm_only_arms_the_selector() {
        let mut rt = drawing_only();
        let frame = Rc::new(
            RgbaBitmap::new(320, 240, vec![180u8; 320 * 240 * 4]).expect("测试位图尺寸自洽"),
        );
        assert!(
            rt.snap.begin(snap::SnapMode::Gif, frame),
            "GIF 会话没开起来"
        );

        let dir = crate::core::paths::module_temp_dir(crate::core::paths::MODULE_SCREENSHOT);
        let before = png_count(&dir);
        let clip_before = rt.clip_seq;
        let own_before = rt.own_clip_seq;

        rt.confirm_snap(Rect::new(10.0, 10.0, 100.0, 80.0));

        assert_eq!(
            rt.gif_selection(),
            Some(ScreenRect::new(10, 10, 100, 80)),
            "GIF 分支没跑到 / 矩形没交出去"
        );
        assert_eq!(rt.own_clip_seq, own_before, "GIF 确认不该写剪贴板");
        assert_eq!(rt.clip_seq, clip_before, "GIF 确认不该改动剪贴板轮询游标");
        assert_eq!(
            png_count(&dir),
            before,
            "GIF 确认不该在 temp/screenshot 留档"
        );
    }

    /// ★★ 任务 2 的回归锁：编辑态两条确认出口的**唯一差别是弹不弹原生框**。
    ///
    /// 2026-10-03 用户口径："截图，如果进入编辑状态。底部是不是要有打勾选项？表示OK了？
    /// 现在只有退出选项。" —— 打勾「完成」就是为了补上"不弹框的那条路"。
    ///
    /// 这条必须钉住 `pending_save_as`（弹框的唯一入口）：两边**产物完全一样**（都进
    /// 剪贴板、都留档、都白闪），所以"打勾有没有生效"在产物上看不出来 —— 一旦有人
    /// 把 `if save_as` 删掉，打勾会退化成另存为，而除了这条测试没有任何地方会发现。
    #[test]
    fn the_done_tick_confirms_without_popping_the_save_dialog() {
        let sel = ScreenRect::new(100, 100, 200, 150);

        // ① 打勾「完成」：出图、留档，但**不**排下另存为框。
        let mut rt = drawing_only();
        enter_edit(&mut rt, sel);
        rt.confirm_shot_edit(false);
        assert!(
            rt.take_pending_save_as().is_none(),
            "打勾「完成」不该弹另存为框（用户已经说过\"就这张\"）"
        );
        assert!(!rt.shot_edit_active(), "确认后编辑态必须收工");

        // ② 「保存」：同样出图留档，**要**排下另存为框。
        let mut rt2 = drawing_only();
        enter_edit(&mut rt2, sel);
        rt2.confirm_shot_edit(true);
        assert!(
            rt2.take_pending_save_as().is_some(),
            "「保存」= 另存为，必须弹框（用户 2026-09-23 口径）"
        );
    }

    /// 编辑会话的进出：`enter` 之后覆盖层读得到选区 / 冻结帧 / 命中带，`cancel` 之后一切
    /// 归零（不留痕迹，也不该把画笔留在 `ShotEdit` 档 —— 那会让覆盖层继续整屏收鼠标，
    /// 而屏上已经没有编辑界面了，看起来就是"软件卡住了"）。
    #[test]
    fn shot_edit_session_enters_and_cancels_cleanly() {
        let mut rt = drawing_only();
        assert!(!rt.shot_edit_active(), "初始不该在编辑态");

        enter_edit(&mut rt, ScreenRect::new(100, 100, 200, 150));
        assert!(rt.shot_edit_active());
        assert_eq!(
            rt.shot_edit_selection(),
            Some(ScreenRect::new(100, 100, 200, 150))
        );
        assert!(
            rt.shot_edit_frame().is_some(),
            "编辑态必须握着那张冻结帧（放大镜的取样源就是它）"
        );
        assert_eq!(rt.shot_edit_zone_at(100, 190), HitZone::Body, "边框带可拖");
        assert_eq!(
            rt.shot_edit_zone_at(100, 100),
            HitZone::CornerTL,
            "左上角可缩放"
        );
        assert_eq!(
            rt.shot_edit_zone_at(200, 180),
            HitZone::None,
            "框内留给画笔（命中带不含框内，否则一落笔就变成拖框）"
        );
        assert!(
            rt.drawing.controller().is_drawing(),
            "画笔应换到 ShotEdit 档"
        );

        rt.cancel_shot_edit();
        assert!(!rt.shot_edit_active());
        assert_eq!(rt.shot_edit_selection(), None);
        assert_eq!(rt.shot_edit_frame(), None);
        assert!(
            !rt.drawing.controller().is_drawing(),
            "取消后画笔不该还停在 ShotEdit 档"
        );
    }

    /// ★★ 编辑态拖拽的**事件归属**：边框带上的按下被会话吃掉（不进输入队列，标注也就
    /// 看不到它、不会在"调框"的同时顺手画出一笔墨）；框内的按下**照常放行**。
    ///
    /// 两条必须一起验 —— 只验"吃掉了"会把"框内也吃掉（画不了笔）"这种错当成通过。
    #[test]
    fn shot_edit_drag_swallows_the_band_but_lets_the_interior_through() {
        let mut rt = drawing_only();
        enter_edit(&mut rt, ScreenRect::new(100, 100, 200, 150));

        // 左边框带上（避开 EdgeL 手柄的命中半径）：被会话吃掉。
        rt.feed(mouse_down(100, 190));
        assert!(rt.input_queue.is_empty(), "边框带上的按下不该进输入队列");
        assert!(
            rt.edit.as_ref().is_some_and(|e| e.drag.is_some()),
            "边框带上的按下应进入拖拽态"
        );

        // 拖动 (+30, +20)：整块平移，且**这条移动也不进队列**（否则会在屏上拖出一道墨）。
        rt.feed(mouse_move(130, 210));
        assert_eq!(
            rt.shot_edit_selection(),
            Some(ScreenRect::new(130, 120, 200, 150)),
            "边框带拖动 = 整块平移"
        );
        assert!(rt.input_queue.is_empty(), "拖拽中的移动同样不该进队列");

        rt.feed(mouse_up(130, 210));
        assert!(
            rt.edit.as_ref().is_some_and(|e| e.drag.is_none()),
            "抬手应结束拖拽"
        );

        // 对照组：框内（画笔的地盘）的按下照常入队，且不动选区。
        rt.feed(mouse_down(200, 180));
        assert_eq!(rt.input_queue.len(), 1, "框内的按下必须放行给画笔");
        assert_eq!(
            rt.shot_edit_selection(),
            Some(ScreenRect::new(130, 120, 200, 150)),
            "框内按下不该动选区"
        );
    }

    /// 拖拽到屏幕外时选区被钳住。与 GIF 选框共用 [`apply_zone`]，行为必须一致 ——
    /// 各写一遍就会出现"GIF 能贴到屏幕边上、编辑态差几像素"这种只在一边复现的不一致。
    #[test]
    fn shot_edit_drag_clamps_into_the_screen() {
        let mut rt = drawing_only();
        enter_edit(&mut rt, ScreenRect::new(100, 100, 200, 150));

        rt.feed(mouse_down(100, 190));
        // 往右猛推 5000：右边缘贴住屏幕右边（1920 - 200 = 1720），左边同步跟上。
        rt.feed(mouse_move(5100, 190));
        assert_eq!(
            rt.shot_edit_selection(),
            Some(ScreenRect::new(1720, 100, 200, 150)),
            "整块平移应钳进屏幕"
        );
        rt.feed(mouse_up(5100, 190));
    }

    /// ★★ **工具栏优先于"拖选区"**：两者的落点可能重叠，而画笔画出来的按钮必须点得动。
    ///
    /// 重叠不是臆想：选区几乎铺满屏幕时工具栏会退回屏幕底边（`bar_origin` 的兜底分支），
    /// 而选区的**下边框带**很可能正好压在那里 —— 用户点按钮，`edit_drag` 却把它当成
    /// "拖动选区"吃掉，表现为"按钮画着却点不动"，且完全看不出原因。
    ///
    /// 用例自带**前提检查**：先把"这个落点确实落在选区边框带上"验掉，否则工具条的几何
    /// 常量一改，本用例会静默退化成"什么都没验"的假绿。
    #[test]
    fn shot_edit_lets_the_toolbar_win_over_the_selection_band() {
        let mut rt = drawing_only();
        // 高 1025、占满宽：下方放不下（1025+12+50 > 1080-8）、上方也放不下 → 退回屏幕底边。
        let sel = ScreenRect::new(0, 0, 1920, 1025);
        enter_edit(&mut rt, sel);

        // 单测里没有覆盖层，`ui_scale` 是控制器的默认值 1.0（与 `toolbar::layout` 的入参同源）。
        let l = crate::modules::drawing::toolbar::layout(rt.screen, 1.0, Some(sel_rect(sel)), true);
        let btn = l.cells[0].1.center();

        // 前提：这个按钮中心确实压在选区下边框带上。
        assert_ne!(
            rt.shot_edit_zone_at(btn.x as i32, btn.y as i32),
            HitZone::None,
            "前提不成立（按钮与边框带不再重叠）→ 本用例已失去分辨力，要改落点"
        );

        // 结论：这一下必须**放行**给画笔（不进入拖拽态），否则按钮永远点不动。
        rt.feed(mouse_down(btn.x as i32, btn.y as i32));
        assert!(
            rt.edit.as_ref().is_some_and(|e| e.drag.is_none()),
            "按在按钮上不该进入拖拽态"
        );
    }

    /// [`clamp_sel`] 是**静态**归一（服务"选区从别处算出来"的入口，如框选确认）：
    /// 先把尺寸钳进屏幕，再钳位置。
    ///
    /// ★ 判据必须用**负原点**的虚拟桌面：原点为 0 时"减不减原点"恰好相等，抓不住错。
    #[test]
    fn clamp_sel_normalizes_a_selection_from_anywhere() {
        let screen = ScreenRect::new(-1920, 0, 3840, 1080);

        // 上边越界 → 贴住屏幕顶，尺寸不变（位置归一而不是尺寸归一）。
        assert_eq!(
            clamp_sel(ScreenRect::new(-1900, -50, 100, 100), screen),
            ScreenRect::new(-1900, 0, 100, 100)
        );
        // 尺寸超过屏幕 → 尺寸钳成整屏，位置贴着**左上角**（含负原点），不是推到 0。
        assert_eq!(
            clamp_sel(ScreenRect::new(5000, 5000, 9999, 9999), screen),
            ScreenRect::new(-1920, 0, 3840, 1080)
        );
        // 尺寸 0 / 负 → 至少 1×1（否则 `shot_edit::compose` 什么都裁不出来）。
        assert_eq!(
            clamp_sel(ScreenRect::new(10, 10, 0, 0), screen),
            ScreenRect::new(10, 10, 1, 1)
        );
    }
}
