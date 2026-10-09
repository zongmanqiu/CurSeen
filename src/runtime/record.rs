//! 录制：MP4 / GIF 的起停，以及录制倒计时横幅。
//!
//! 采集与编码在 `platform::recorder`；这里只管 Runtime 侧的开关与用户反馈。

use super::*;

use std::sync::Mutex;

/// 录制开始前的倒计时时长（毫秒）。用户口径：屏幕中央「即将录制 3/2/1」。
pub(crate) const REC_COUNTDOWN_MS: f32 = 3000.0;

/// 倒计时显示数字：还剩几个整秒（向上取整，3000..=2001 显示 3）。
pub(crate) fn rec_countdown_seconds(remain_ms: f32) -> u32 {
    ((remain_ms / 1000.0).ceil().max(1.0)) as u32
}

/// 心跳停摆多久就**主动结束**这次录制（毫秒）。
///
/// ★ 2026-10-02 现场：录制线程楔在某个 OS 调用里，不报错、不写帧、连停止位都唤不醒，
///   界面却一直显示"在录"，用户干等 13 分钟才察觉，最后产物还缺 `moov`。这里把
///   "永远在录、永远没结果"变成"一分钟内给出结论"。
///
/// 60 秒足够宽松：正常一帧 33ms，连掉 1800 帧只可能是真死（看门狗 15 秒就开始报
/// 阶段，见 `platform::recorder::STALL_WARN_MS`）。
pub(crate) const REC_STALL_GIVEUP_MS: u64 = 60_000;

/// 距下一次倒计时数字跳变的毫秒数（1 表示已经该跳了，交给 `frame_demand` 归到 Now）。
pub(crate) fn rec_countdown_next_change_ms(remain_ms: f32) -> u32 {
    let secs = rec_countdown_seconds(remain_ms) as f32;
    let next = remain_ms - (secs - 1.0) * 1000.0;
    (next.ceil().max(1.0)) as u32
}

/// 一次**后台收尾**的结论（收尾线程 → UI 帧，经 [`REC_FINISH_NOTICES`] 槽位中转）。
pub(crate) struct RecFinishNotice {
    pub path: std::path::PathBuf,
    pub kind: RecKind,
    /// 建编码器时的硬件策略结论（`hardware_allowed` 的快照；GIF 恒 `None`）。
    pub hardware: Option<bool>,
    pub outcome: crate::platform::recorder::FinishOutcome,
}

/// 收尾结论槽位：收尾线程 push，UI 帧循环取走（与 `take_rec_toast` 同一套模式——
/// 后台线程碰不到 `Rc<RefCell<Runtime>>`，只能经全局槽位回传）。
static REC_FINISH_NOTICES: Mutex<Vec<RecFinishNotice>> = Mutex::new(Vec::new());

fn push_rec_finish_notice(n: RecFinishNotice) {
    if let Ok(mut v) = REC_FINISH_NOTICES.lock() {
        v.push(n);
    }
}

/// 取走全部已完成的录制收尾结论（取过即无）。
pub fn take_finished_recordings() -> Vec<RecFinishNotice> {
    REC_FINISH_NOTICES
        .lock()
        .map(|mut v| std::mem::take(&mut *v))
        .unwrap_or_default()
}

/// 画录制倒计时：屏幕中央半透明深底条 + 白字「即将录制 N」（GIF 是「即将录制 GIF N」）。
///
/// ★ 加「GIF」前缀不是装饰：**MP4 与 GIF 共用同一把停止快捷键**，用户必须在这 3 秒里
///   看出自己刚才按的是哪把开始键 —— 否则录完才发现格式不对，白录一遍。
///
/// ★ 2026-09-22 晚：改走 [`DrawList::draw_centered_banner`] —— 与屏幕标注的提示浮字
///   （`modules::drawing::notify`）、计时器**同一个底层函数**，底框/内边距/圆角/垂直居中
///   全库一致（用户口径："这些提示在底层上应该是使用相同的函数，这样才方便管理"）。
///   此前它是**第三份独立实现**（方角 `fill_rect` + 自己算文字落点的 padding），
///   与另外两处的观感对不上；"文字 y 是基线还是排版盒左上角"这个歧义也随之下线
///   —— 居中由渲染端（`DrawCmd::TextCentered`）保证，模块层不再关心。
pub(crate) fn draw_rec_countdown(
    list: &mut DrawList,
    screen: ScreenRect,
    remain_ms: f32,
    kind: crate::platform::recorder::RecKind,
) {
    pub(crate) const FONT_PT: f32 = 40.0;
    let secs = rec_countdown_seconds(remain_ms);
    let label = match kind {
        crate::platform::recorder::RecKind::Mp4 => format!("即将录制 {secs}"),
        crate::platform::recorder::RecKind::Gif => format!("即将录制 GIF {secs}"),
    };
    // 内边距沿用原值 32×20（字号 40 比提示浮字的 28 大，留白也相应更宽）；
    // 圆角按字号插值取 12（提示浮字 28pt→8、计时器 72pt→16）。
    list.draw_centered_banner(
        screen,
        &label,
        FONT_PT,
        32.0,
        20.0,
        Argb::new(180, 0x20, 0x20, 0x20),
        Argb::new(255, 0xFF, 0xFF, 0xFF),
        12.0,
    );
}

impl Runtime {
    /// 取一次「录制成功，请在桌面右下角展示提示窗」的请求（边沿触发，取过即无）。
    ///
    /// 覆盖层在 UI 线程消费（通道回调 + 250ms 兜底），打开一个带「打开文件夹」
    /// 按钮的提示窗；窗体由用户手动关闭，不自动消失。
    pub fn take_rec_toast(&mut self) -> Option<std::path::PathBuf> {
        self.rec_toast.take()
    }

    /// 登记一次录制产物：桌面提示窗 +（GIF 时）记住路径供裁剪。
    ///
    /// 两件事都只在**产物真的存在**时做（`finish()` 的两种成功结局都会调到这里：
    /// 干净收尾、以及"中途失败但前段仍可播放"）。所以「裁剪」不会指着一个 0 字节文件。
    pub(crate) fn note_recording_output(&mut self, path: std::path::PathBuf, kind: RecKind) {
        match kind {
            RecKind::Gif => self.last_gif = Some(path.clone()),
            RecKind::Mp4 => self.last_mp4 = Some(path.clone()),
        }
        self.rec_toast = Some(path);
    }

    /// 最近一次成功落盘的 GIF（裁剪入口的输入）。`None` = 还没录过。
    pub fn last_gif(&self) -> Option<std::path::PathBuf> {
        self.last_gif.clone()
    }

    /// 最近一次成功落盘的 MP4（时间裁剪入口的输入）。`None` = 还没录过。
    pub fn last_mp4(&self) -> Option<std::path::PathBuf> {
        self.last_mp4.clone()
    }

    /// 录制中按下**另一把键**时的动作：停掉当前这段，收尾完成后自动开新格式。
    ///
    /// 桌面复制同时只容一份（见 [`crate::platform::recorder::DuplHolder`]），所以
    /// "两路同时录"做不到，只能串行切换。用户按一次键就完成"换格式"，不用按第二遍。
    pub(crate) fn switch_recording_to(
        &mut self,
        kind: RecKind,
        crop: Option<crate::core::geometry::ScreenRect>,
    ) {
        log::info!("录制中切换到 {kind:?}：先停当前这段，收尾后接着开");
        self.stop_recording_now();
        self.rec_after_finish = Some(kind);
        self.rec_after_finish_crop = crop;
    }

    /// 录制快捷键的**一键开关**：一次按下对应「开始 / 取消倒计时 / 停止 / 换格式」四态之一。
    ///
    /// ★ `kind` 只在"从空闲进入倒计时"时被记住（写进 `rec_pending` —— 它是"归零后录 MP4
    ///   还是 GIF"的唯一来源）。
    ///
    /// ★★ 录制中按下**另一把键 = 换格式**（用户 2026-10-03 口径）：停掉当前这段，收尾
    ///   完成后自动开新格式。用户只按一次键，不用"按了没反应、再按一次"。
    ///   换不了的是"中途改封装器"——一条流已经按 H.264 或 GIF 写下去了，那做不到。
    ///   桌面复制同时只容一份，所以"两路并行录制"也做不到，只能串行切换。
    pub(crate) fn toggle_recording(
        &mut self,
        kind: RecKind,
        crop: Option<crate::core::geometry::ScreenRect>,
    ) {
        // ★★ 上一段楔住了：录制线程还占着桌面复制（进程级单例），此时开新的一段
        //   必失败。与其让用户得到一次"建立桌面复制失败"的天书，不如当场说清楚。
        if self.rec_wedged {
            self.rec_pending = None;
            self.pending_crop = None;
            log::warn!("录制不可用：上一次录制的线程仍未退出（楔住），需重启软件");
            self.action_notify.show("录制暂不可用，请重启软件后重试");
            return;
        }
        if self.rec_countdown_ms.take().is_some() {
            // 倒计时中 = 取消（原「停止快捷键」在倒计时里的行为，随快捷键合并搬到这里）。
            self.rec_pending = None;
            self.pending_crop = None;
            log::info!("录制倒计时已取消");
        } else if self.recorder.is_some() {
            // 正在录：同格式 = 停止，异格式 = 换过去。
            if self.recorder.as_ref().map(|r| r.kind()) == Some(kind) {
                self.stop_recording_now();
            } else {
                self.switch_recording_to(kind, crop);
            }
        } else if self.rec_finishing {
            // ★ 上一次录制还在收尾（线程可能楔住了，见字段注释）：这会儿直接开新的一段
            //   只会撞上"桌面复制建不起来"。**不拒绝**——登记成待办，收尾完成后接着开。
            if self.rec_after_finish.is_none() {
                self.rec_after_finish = Some(kind);
                self.rec_after_finish_crop = crop;
                log::info!("上一次录制仍在收尾，{kind:?} 已排队等它结束");
                self.action_notify.show("正在结束上一次录制，随后开始…");
            } else {
                log::warn!("上一次录制仍在收尾且已有待办，忽略本次开始请求");
            }
        } else {
            self.rec_pending = Some(kind);
            self.pending_crop = crop;
            self.rec_countdown_ms = Some(REC_COUNTDOWN_MS);
            log::info!(
                "{}录制倒计时开始（{} ms）",
                match kind {
                    RecKind::Mp4 => "",
                    RecKind::Gif => "GIF ",
                },
                REC_COUNTDOWN_MS as u32
            );
        }
    }

    /// 这条输入事件是否是截图（三键）/ 录制 / GIF / 裁剪快捷键（覆盖层据此决定是否在 park 态被唤醒）。
    ///
    /// ★★ 截图那两把键要按 **"键或它的 Ctrl 变体"** 判（见 [`hk_matches_or_ctrl`]）。
    ///   漏掉 Ctrl 变体的症状与下面几条历史 bug 一模一样，而且更隐蔽：覆盖层 park 期间
    ///   （用户什么都没开、屏幕上空转）Ctrl+Alt+S 只会进输入队列，**没有人推进帧** ——
    ///   `frame()` 不跑就没人去做快捷键匹配，表现为"按住 Ctrl 截图毫无反应，松开 Ctrl 再按就好"
    ///   （普通 Alt+S 命中前一条、照常能唤醒，于是看起来像"Ctrl 那一支坏了"）。
    pub fn action_hotkey_wakes(&self, ev: &InputEvent) -> bool {
        let InputEvent::KeyDown { key, mods, .. } = *ev else {
            return false;
        };
        let s = &self.config.screenshot;
        hk_matches_or_ctrl(&s.hotkey, key, mods)
            || hk_matches_or_ctrl(&s.hotkey_pin, key, mods)
            || (!s.hotkey_pin_clipboard.is_empty()
                && hk_matches(&s.hotkey_pin_clipboard, key, mods))
            // ★ 长截图快捷键同理（2026-10-01）：一键开关，park 态下它是唯一入口，
            //   漏在这里 = "空闲时按长截图键毫无反应"。
            || (!self.config.screenshot.hotkey_scroll.is_empty()
                && hk_matches(&self.config.screenshot.hotkey_scroll, key, mods))
            // ★ 宽截图快捷键同理（2026-10-01）：与长截图同一套会话，park 态唯一入口。
            || (!self.config.screenshot.hotkey_scroll_wide.is_empty()
                && hk_matches(&self.config.screenshot.hotkey_scroll_wide, key, mods))
            // ★ 长截图进行中的 Esc = 取消：会话虽让覆盖层不 park（input_needs_frame 含
            //   会话），但"按键是否踢帧"的判定在通道回调里走的就是本函数——不登记的键
            //   在 park 判定与踢帧之间有一个 250ms 的空档会丢。登记后任何时刻的 Esc 都
            //   能到达 `handle_action_key` 的取消分支。
            || (self.scroll.is_some()
                && key == KeyCode::Esc
                && !(mods.ctrl || mods.alt || mods.shift || mods.win))
            // ★ 录制两把键都是一键开关（开始与停止是同一个键，见 `toggle_recording`）。
            //
            //   ★★ 从这里往下**一律写完整路径**（不取 `let r = &self.config.record` 别名）：
            //   `every_action_hotkey_is_registered_as_a_waker` 这道结构门禁是拿
            //   `self.config.<路径>` 这条**文本**去 `input.rs` 里对账的，别名会让两边
            //   的字符串对不上、门禁退化成假红。
            || (!self.config.record.hotkey.is_empty()
                && hk_matches(&self.config.record.hotkey, key, mods))
            // ★ 选区录屏快捷键同理：park 期间（没在录、屏幕上什么都没发生）它是唯一的入口，
            //   漏在这里 = "空闲时按选区录屏毫无反应"。
            || (!self.config.record.region_hotkey.is_empty()
                && hk_matches(&self.config.record.region_hotkey, key, mods))
            // ★ MP4 时间裁剪快捷键同理：park 期间它是唯一入口，漏了 = "空闲时按了没反应"。
            || (!self.config.record.trim_hotkey.is_empty()
                && hk_matches(&self.config.record.trim_hotkey, key, mods))
            // ★ GIF 快捷键也走同一条倒计时通道，不在这里列上的话覆盖层 park 期间它按不动。
            || (!self.config.gif.hotkey.is_empty()
                && hk_matches(&self.config.gif.hotkey, key, mods))
            // ★ 裁剪快捷键同理：park 期间（没在录、屏幕上什么都没发生）它是唯一的入口，
            //   漏在这里 = "空闲时按裁剪键毫无反应"。
            || (!self.config.gif.trim_hotkey.is_empty()
                && hk_matches(&self.config.gif.trim_hotkey, key, mods))
            // ★ 剪贴板呼出快捷键同理（2026-09-24 用户报"快捷键无法呼出"）：常驻工具，
            //   `modules::clipboard` 的 tick 只在 frame() 里跑，park 期间不列上这里
            //   就是唯一的入口丢失——应用醒着时能呼出、空闲后按了没反应（时好时坏）。
            || (!self.config.clipboard.hotkey.is_empty()
                && hk_matches(&self.config.clipboard.hotkey, key, mods))
            // ★ 便签呼出快捷键同理（2026-09-24 补）：`modules::sticky_note` 的 tick 同样
            //   只在 frame() 里跑，park 期间漏在这里就是"刚开机能叫出来、放一会儿按
            //   不动"。便签与剪贴板同属**常驻工具**，没有自己的功能开关可以撑住出帧。
            || (!self.config.sticky_note.hotkey.is_empty()
                && hk_matches(&self.config.sticky_note.hotkey, key, mods))
            // ★ 计时器快捷键同理（2026-09-24 补）：它的起停走 `handle_action_key`
            //   （`runtime/input.rs`），而那里只在 frame() 里被调。`input_needs_frame`
            //   只认"计时器**已经**在跑"（`timer_running`）——空闲时按下开始键这一下
            //   恰好落在空档里，表现为"第一次按没反应，多按几次/动一下鼠标才好"。
            || (!self.config.global.timer.hotkey.is_empty()
                && hk_matches(&self.config.global.timer.hotkey, key, mods))
            // ★ 全屏放大快捷键同理：关着的时候按下才是"打开"这一下（`input_needs_frame`
            //   只认 `enabled` 为真，即已经开着），漏了它就是"闲置一会儿按了没反应"。
            || (!self.config.magnifier_fullscreen.hotkey.is_empty()
                && hk_matches(&self.config.magnifier_fullscreen.hotkey, key, mods))
            // ★ 画中画放大快捷键同理（2026-09-26 新增）：关着按下才是"打开"，空闲时
            //   它是唯一入口（面板开关得先醒着才点得到）。
            || (!self.config.magnifier_pip.hotkey.is_empty()
                && hk_matches(&self.config.magnifier_pip.hotkey, key, mods))
            // ★ 脚本热键（2026-09-29）：`scripts/` 下脚本卡里的 `hotkey` 参数派生出来的
            //   动态名单（`Runtime::script_hotkeys`）。同样是"空闲时唯一入口"的常驻键，
            //   不列上这里 = 应用睡下去之后 AI 对话快捷键按了没反应。
            || self
                .script_hotkeys()
                .iter()
                .any(|(spec, _)| hk_matches(spec, key, mods))
    }

    /// 当前是否正在录屏（托盘菜单 / 状态可查）。
    pub fn is_recording(&self) -> bool {
        self.recorder.is_some()
    }

    /// 停止并收尾所有进行中的录制（退出前兜底）。
    ///
    /// ★ 2026-10-02 起经 [`Recorder::finish_detached`] + 通道**有限等待**（正常收尾
    ///   亚秒级；上限给足 35s）：等不到就放弃——宁可产物缺索引，也不让退出挂死。
    pub fn stop_recording_if_any(&mut self) {
        if let Some(rec) = self.recorder.take() {
            // 托盘图标不管收尾结局都切回常态——录制已经停了，红点就是谎报。
            crate::app::tray::set_tray_recording(false);
            let (tx, rx) = std::sync::mpsc::channel();
            rec.finish_detached(std::time::Duration::from_secs(30), move |o| {
                let _ = tx.send(o);
            });
            match rx.recv_timeout(std::time::Duration::from_secs(35)) {
                Ok(o) => {
                    if let Some(e) = o.error {
                        log::warn!("退出前收尾录制时报过错: {e}");
                    }
                }
                Err(_) => {
                    log::warn!("退出时录制收尾超时，不再等待（产物可能不完整）");
                }
            }
        }
    }

    /// 倒计时归零后真正开始录制（见 [`Self::handle_action_key`]）。
    ///
    /// ★ MP4 与 GIF **共用同一条采集管线**（`recorder::Session` 抓帧 + 缩放），差别只有
    ///   两处：封装器（MF SinkWriter ↔ `core::gif_enc`）与"有没有音轨"。所以这里只有
    ///   一份 `RecOptions`，靠 `kind` 分流——别再写第二套抓屏代码。
    ///
    /// 固定录主显示器（用户口径 2026-09-17：多屏录制区域暂不需要）。
    pub(crate) fn start_recording_now(&mut self) {
        let kind = self.rec_pending.take().unwrap_or(RecKind::Mp4);
        let rec = self.config.record.clone();
        let gif = self.config.gif.clone();
        // ★ 两条产物目录**分开**（用户口径 2026-09-20）：MP4 落 `temp/record`，GIF 落
        //   `temp/gif`。目录按需创建，所以在各自的臂里各取一次——别在这个位置把两个都建
        //   出来（录一段 MP4 就顺手多一个空的 `gif/` 目录，用户会以为坏了）。
        //
        //   GIF 单开一个目录还多一层用处：裁剪窗「打开…」的默认目录就指向它
        //   （见 `layer::gif_trim::pick_file`）——"最近录的那些都在这儿"。

        let (path, fps, max_long_edge, hardware) = match kind {
            RecKind::Mp4 => {
                let res = (rec.resolution as usize).min(RECORD_RESOLUTION_LONG_EDGE.len() - 1);
                let hardware = if rec.hardware == 0 {
                    Hardware::Auto
                } else {
                    Hardware::Off
                };
                (
                    crate::core::paths::module_temp_dir(crate::core::paths::MODULE_RECORD)
                        .join(format!("CurSeen_{}.mp4", timestamp())),
                    rec.fps.max(1),
                    RECORD_RESOLUTION_LONG_EDGE[res],
                    hardware,
                )
            }
            RecKind::Gif => {
                // ★ 不再读「输出尺寸」档位：选框就是尺寸控制器，输出一律按选框 1:1，
                //   只有超过安全上限才缩（防"框了整屏 → 体积爆掉"，见 `GIF_MAX_LONG_EDGE`）。
                (
                    crate::core::paths::module_temp_dir(crate::core::paths::MODULE_GIF)
                        .join(format!("CurSeen_{}.gif", timestamp())),
                    // ★ GIF 帧延时单位是 10ms（见 `core::gif_enc`），fps 直接映射成延时，
                    //   所以只接受档位表里的值；配置里被手改成怪数就夹到合法区间。
                    gif.fps.clamp(1, 50),
                    crate::core::config::GIF_MAX_LONG_EDGE,
                    Hardware::Off,
                )
            }
        };

        let opts = RecOptions {
            kind,
            path,
            area: RecordArea::Primary,
            // ★ 只有 GIF 带选框：选区录屏则走 `pending_crop`（确认那一刻填进来的矩形，
            //   开录即消费并清空，见下方 `pending_crop = None`）。普通 MP4 录整块显示器
            //   （多屏录制区域暂不需要），`pending_crop` 为 `None`。
            //
            //   ★ GIF 用**拷贝**而不是 `take()`：录完用户可能想"就这块再来一遍"，
            //     选框留着，控件条上的「开始 / 停止」还能再按第二次。
            crop: match kind {
                RecKind::Gif => self.gif_sel,
                RecKind::Mp4 => self.pending_crop,
            },
            fps,
            // `None` = 自动：输出尺寸要等桌面复制建好才知道，交给录屏器算
            // （见 `RecordSettings::bitrate_kbps` 的注释）。
            bitrate_kbps: rec.bitrate_kbps(),
            max_long_edge,
            hardware,
            show_cursor: match kind {
                RecKind::Mp4 => rec.show_cursor,
                RecKind::Gif => gif.show_cursor,
            },
            // 音频两路都按**三态来源**判"要不要采"：`Some(_)` 即开启（`Some("")` = 跟随默认）。
            capture_mic: rec.mic_source().is_some(),
            capture_speaker: rec.speaker_source().is_some(),
            mic_denoise: rec.mic_denoise.clone(),
            // 设备名：`unwrap_or_default()` 给出空串 = 系统默认（`resolve_device` 认空名）。
            mic_device: rec.mic_source().unwrap_or_default().to_string(),
            speaker_device: rec.speaker_source().unwrap_or_default().to_string(),
            gif_loop: gif.loop_forever,
        };
        // ★ MP4 选区矩形的消费点：开录即清空，避免这块矩形残留到"下次普通整屏录"
        //   （普通录屏入口 `toggle_recording(RecKind::Mp4, None)` 也会把它清掉，这里
        //   双保险，且是"真的用过了"的那一刻清）。GIF 选框 `gif_sel` 则**保留** ——
        //   控件条上的「开始 / 停止」还能再按第二次（见 `arm_gif`）。
        self.pending_crop = None;
        match Recorder::start(opts) {
            Ok(r) => {
                self.recorder = Some(r);
                // 托盘圆点变红（2026-09-27 用户口径）：录制是唯一"看不见但一直在干活"的
                // 状态，托盘是它唯一常驻可见的状态灯。倒计时不算——那 3 秒还能取消。
                crate::app::tray::set_tray_recording(true);
                log::info!(
                    "开始录制（{}）",
                    match kind {
                        RecKind::Mp4 => "MP4",
                        RecKind::Gif => "GIF",
                    }
                );
            }
            Err(e) => {
                log::warn!("无法开始录制: {e}");
                // ★ 起录失败也要把托盘红点按回去：失败前"在录"这件事从未成立，
                //   而托盘是录制状态唯一常驻可见的灯 —— 留着红点就是谎报。
                crate::app::tray::set_tray_recording(false);
                // ★★ 桌面复制建不起来 ≠ 要重启（2026-10-03 改）。它是"上一段的复制对象
                //   还没释放"，等上一段收尾完就好（旧文案让用户重启程序，重启并没有用，
                //   而真正要做的是"等一下"）。措辞按成因分：仍在收尾 → 等；其它 → 重启。
                let msg = e.to_string();
                let text = if self.rec_finishing {
                    "上一次录制尚未结束，请稍候重试"
                } else if msg.contains("桌面复制") {
                    "无法开始录制，请稍候重试"
                } else {
                    "无法开始录制"
                };
                self.action_notify.show(text);
            }
        }
    }

    /// 停止录制并收尾；成功且开了「录完提示」时登记桌面右下角提示窗
    /// （overlay 经 [`Self::take_rec_toast`] 消费，用户手动关闭）。
    ///
    /// ★★ 2026-10-02 起收尾**不占 UI 线程**：`finish` 要 join 录制线程（写 moov 索引、
    ///    等编码器吐完），长录制或编码器卡住时可能几十秒甚至永不返回——旧实现同步等，
    ///    表现就是"按停止没反应、面板打不开、任务管理器杀进程"，杀掉又让 mp4 缺
    ///    `moov` 打不开。现在只发停止位 + 立即给"正在保存"反馈，收尾在后台跑，结论
    ///    经 [`REC_FINISH_NOTICES`] 回到 [`Self::consume_finished_recordings`]。
    pub(crate) fn stop_recording_now(&mut self) {
        let Some(rec) = self.recorder.take() else {
            return;
        };
        // 选区录制的那块裁剪矩形随录制终结一并作废。
        self.pending_crop = None;
        // 录制已停，托盘红点立刻切回常态——不等 finish 的结局（哪怕收尾失败，
        // "没有在录"这个事实已经成立）。
        crate::app::tray::set_tray_recording(false);
        // 收尾结论（路径/格式/硬件结论）先摘出来，随后台线程入槽。
        // ★ 置"收尾中"：录制线程这会儿可能还卡在某次 OS 调用里（见过卡在
        //   `AcquireNextFrame`），`recorder` 已经没了但线程没死。没有这个标记的话，
        //   用户再按一次快捷键会被判成"没在录"而**又起一段**，新那段多半连桌面复制
        //   都建不起来（上一段的对象还占着），界面则一直挂着"在录"。见字段注释。
        self.rec_finishing = true;
        let path = rec.path().to_path_buf();
        let kind = rec.kind();
        let hardware = rec.hardware_allowed();
        rec.finish_detached(std::time::Duration::from_secs(30), move |outcome| {
            push_rec_finish_notice(RecFinishNotice {
                path,
                kind,
                hardware,
                outcome,
            });
        });
        // 录制缓冲要等收尾线程真正跑完才释放，裁剪时机在收到结论那一刻（见下）；
        // 这里的反馈只说"已受理"。
        self.action_notify.show("已停止录制，正在保存…");
    }

    /// 消费后台收尾结论（帧循环每帧调用）：补日志、提示浮字 / 提示窗与工作集裁剪。
    ///
    /// 三种结局的口径与旧同步版逐字一致：干净收尾 →「录制完成」+ 桌面提示窗；
    /// 有产物但中途断过 →「录制中断，已保存前段」+ 仍登记提示窗；没有产物 →「录制失败」。
    pub fn consume_finished_recordings(&mut self) {
        let notices = take_finished_recordings();
        if notices.is_empty() {
            return;
        }
        // ★ 收尾结论回来了 —— 无论干净/中断/失败，"上一次录制"这件事到此结束，
        //   快捷键重新可用（`finish_detached` 保证回调一定被触发，含超时弃子那条路）。
        self.rec_finishing = false;
        // ★★★ 收尾结论回来 = 桌面复制已释放，此刻起新的一段才不会再撞 `0x80070057`。
        //   所以"换格式"的待办在这里兑现，用户按一次键就完成切换（见
        //   `Runtime::switch_recording_to`）。放在 for 循环**之后**：先把上一段的结论
        //   播报完（浮字会被新浮字盖掉，但日志与提示窗都该拿到），再起新的。
        let queued = self.rec_after_finish.take();
        let queued_crop = self.rec_after_finish_crop.take();
        for n in notices {
            let RecFinishNotice {
                path,
                kind,
                hardware,
                outcome,
            } = n;
            // ★ 分段之后产物可能是**目录**（多段）也可能是单文件：`path` 只是名义路径，
            //   多段时它并不存在。UI / 日志一律用真实位置，否则会把用户指向空文件。
            let actual = crate::platform::recorder::product_path(&path);
            match outcome {
                // ★★ 分三种结局，别把"中断"混进"完成"（见 `FinishOutcome` 的说明）。
                o if o.is_clean() => {
                    let size = o.bytes.unwrap_or(0);
                    // MP4 顺带把"这次用了硬件还是软件编码"记下来，与开录时那条日志首尾呼应。
                    let encoder = match hardware {
                        Some(true) => "，硬件编码",
                        Some(false) => "，软件编码",
                        None => "",
                    };
                    log::info!(
                        "录制完成（{:?}{encoder}），{size} 字节：{}",
                        kind,
                        actual.display()
                    );
                    self.note_recording_output(actual.clone(), kind);
                }
                // 有产物但中途断过：产物**仍可播放**（`SinkFinisher` 收尾过），但必须说清。
                o if o.bytes.is_some() => {
                    let size = o.bytes.unwrap_or(0);
                    log::warn!(
                        "录制中断（{}），已保存前段 {size} 字节：{}",
                        o.error.as_deref().unwrap_or("原因未知"),
                        actual.display()
                    );
                    if o.wedged {
                        // ★★ 楔住的线程还占着桌面复制（进程级单例）——**在它退出前新的一段
                        //   根本建不起来**。如实标记，快捷键会据此拒绝并说明原因，
                        //   而不是让用户按下之后得到一次"建立桌面复制失败"的天书。
                        self.rec_wedged = true;
                        self.action_notify.show("录制卡住，需重启软件才能继续录");
                    } else {
                        self.action_notify.show("录制中断，已保存前段");
                    }
                    self.note_recording_output(actual, kind);
                }
                o => {
                    log::warn!(
                        "录制收尾失败: {}",
                        o.error.as_deref().unwrap_or("没有产出文件")
                    );
                    if o.wedged {
                        self.rec_wedged = true;
                        self.action_notify.show("录制卡住，需重启软件才能继续录");
                    } else {
                        self.action_notify.show("录制失败");
                    }
                }
            }
        }
        // 录制缓冲（DXGI 帧纹理 + 编码器队列，全项目最大的常驻块）已随收尾释放
        // → 2 秒后整理工作集（`platform::working_set`）。忙判在延迟线程里：真正执行时
        // 若又开了新录制会自动跳过。
        crate::platform::working_set::request_trim_soon();
        // ★ 换格式的待办在这时候才兑现（此时桌面复制已释放）。GIF 不走倒计时直接进
        //   选区——它必须先框一块才录（见 `begin_gif_selection`），和"换格式"是两件事。
        if let Some(kind) = queued {
            log::info!("收尾完成，接着开 {kind:?}");
            self.begin_recording_after_switch(kind, queued_crop);
        }
    }

    /// 换格式收尾完成后：把待办兑现成"新一段的开录流程"。
    ///
    /// MP4 走倒计时（给用户三秒后悔）；GIF **不能**进倒计时——GIF 得先框一块屏幕，
    ///   所以直接进区域选择。
    fn begin_recording_after_switch(
        &mut self,
        kind: RecKind,
        crop: Option<crate::core::geometry::ScreenRect>,
    ) {
        match kind {
            RecKind::Gif => {
                self.action_notify.show("已切换到 GIF，请框选区域");
                self.begin_snap(snap::SnapMode::Gif);
            }
            RecKind::Mp4 => {
                self.rec_pending = Some(RecKind::Mp4);
                self.pending_crop = crop;
                self.rec_countdown_ms = Some(REC_COUNTDOWN_MS);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ★ 后台收尾结论的槽位（2026-10-02）：push → take 取走即无、再取为空。
    /// 收尾线程与 UI 帧循环就靠这条通路衔接（`stop_recording_now` 不再同步等收尾）。
    #[test]
    fn finished_recordings_slot_roundtrips_and_drains() {
        let make = |name: &str| RecFinishNotice {
            path: std::path::PathBuf::from(format!("C:/tmp/{name}.mp4")),
            kind: RecKind::Mp4,
            hardware: Some(false),
            outcome: crate::platform::recorder::FinishOutcome {
                bytes: Some(1024),
                error: None,
                wedged: false,
            },
        };
        push_rec_finish_notice(make("a"));
        push_rec_finish_notice(make("b"));

        let taken = take_finished_recordings();
        assert_eq!(taken.len(), 2, "应一次取走两条结论");
        assert_eq!(taken[0].path, std::path::PathBuf::from("C:/tmp/a.mp4"));
        assert!(taken[1].outcome.is_clean(), "结论本体要原样带过来");

        assert!(
            take_finished_recordings().is_empty(),
            "取过即无——否则同一份结论会被 UI 重复消费"
        );
    }

    /// ★★★ 「换格式」待办的兑现时机（用户 2026-10-03 口径：录着 MP4 时按 GIF 键要能录 GIF）。
    ///
    /// 桌面复制同一时刻只容一份，所以"两路并行"做不到，只能串行切换；而上一段的复制对象
    /// 要等**收尾线程跑完**才释放，早一步开新的一段就撞 `0x80070057`。
    /// 于是"换格式"必须记成待办，在收尾结论回来的那一刻兑现。
    ///
    /// 判据取自源码（`consume_finished_recordings` 的结构），因为时序本身要真机才跑得出来。
    /// 钉两件事：① 待办的写入与取出成对，且**只在结论回来后才取**；
    /// ② 帧门里必须有 `rec_finishing` / `rec_after_finish` —— 漏了会 park 卡死
    /// （`consume_finished_recordings` 挂在 `frame()` 里，park 就不跑）。
    #[test]
    fn a_queued_format_switch_is_released_only_after_the_finish_notice() {
        let src = include_str!("record.rs");
        // ① 取待办必须发生在 `rec_finishing = false` **之后**：桌面复制正是那一刻释放的。
        let clear = src
            .find("self.rec_finishing = false;")
            .expect("应能找到收尾完成标志的清除点");
        let take = src
            .find("self.rec_after_finish.take()")
            .expect("应能找到待办的取出点");
        assert!(
            take > clear,
            "待办必须在 rec_finishing 置回 false 之后才取：早一刻开新的一段会撞上\
             '同一时刻只容一份桌面复制'（0x80070057）"
        );
        // ② 帧门两处都要有，否则浮字淡出后 park，收尾结论无人消费 → 快捷键永久失效。
        let frame = include_str!("frame.rs");
        for field in ["self.rec_finishing", "self.rec_after_finish.is_some()"] {
            let in_gate = frame
                .find("pub fn input_needs_frame(&self) -> bool")
                .map(|start| {
                    let tail = &frame[start..];
                    let end = tail.find("\n    }").unwrap_or(tail.len());
                    tail[..end].contains(field)
                })
                .unwrap_or(false);
            assert!(
                in_gate,
                "`input_needs_frame` 漏了 `{field}`：覆盖层 park 之后\
                 `consume_finished_recordings` 不再跑，待办永远等不到兑现"
            );
        }
    }

    /// ★ 录制倒计时走**共享横幅函数**（`DrawList::draw_centered_banner`）：断言它产出
    /// 「圆角底框 + 居中的 `TextCentered`」，而不是自己那套方角 `fill_rect` + 手算 padding。
    ///
    /// 为什么需要这条：2026-09-22 晚做「屏幕中央提示统一」时，标注浮字与计时器都迁到了共享
    /// 函数，**倒计时漏了**（它是第三份独立实现）。真机像素判据
    /// （`temp/check_countdown_banner.py`）+ 日志只能覆盖 MP4 分支 —— GIF 分支要先按 F4 进
    /// 「截图区域选择」，那层暗幕会把白底判据污染掉。所以两种 kind 的 label 与几何在这里钉死。
    ///
    /// 判据取**命令形态**而不是像素：模块层能保证的就是"画成什么样"，像素归渲染端。
    #[test]
    fn rec_countdown_draws_the_shared_centered_banner() {
        let screen = ScreenRect {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
        };
        let center = Point::new(960.0, 540.0);
        for (kind, prefix) in [
            (crate::platform::recorder::RecKind::Mp4, "即将录制 "),
            (crate::platform::recorder::RecKind::Gif, "即将录制 GIF "),
        ] {
            let mut list = DrawList::new();
            draw_rec_countdown(&mut list, screen, 2500.0, kind);

            let mut box_ = None;
            let mut text = None;
            for cmd in list.cmds() {
                match cmd {
                    DrawCmd::Fill {
                        shape: Shape::Rect { rect, radius },
                        ..
                    } => box_ = Some((*rect, *radius)),
                    DrawCmd::TextCentered {
                        center: tc,
                        text: t,
                        ..
                    } => text = Some((*tc, t.clone())),
                    _ => {}
                }
            }
            let (rect, radius) = box_.expect("倒计时应画一个底框");
            assert!(
                radius > 0.0,
                "底框应是圆角（旧实现是方角 fill_rect，正是「对不齐」的根源之一）"
            );
            let rc = rect.center();
            assert!(
                (rc.x - center.x).abs() <= 0.5 && (rc.y - center.y).abs() <= 0.5,
                "底框应居中于屏幕中心，得到 {rc:?}"
            );
            let (tc, t) = text.expect("倒计时应画文字");
            assert_eq!(tc, center, "文字中心应与屏幕中心重合（垂直居中交给渲染端）");
            assert!(
                t.starts_with(prefix),
                "label 应以「{prefix}」开头，得到 {t:?}"
            );
        }
    }

    /// ★★ **常驻工具型快捷键必须全部能唤醒 park 中的覆盖层**。
    ///
    /// 为什么需要这条名单：`handle_action_key`（`runtime/input.rs`）与各模块的 `tick`
    /// **只在 `frame()` 里跑**，而空闲时覆盖层处于 park 态、不为任何输入出帧
    /// （见 `Runtime::input_needs_frame`）。落在 `action_hotkey_wakes` 名单外的快捷键，
    /// 症状一律是"应用刚醒着时按得动、放一会儿按不动"（剪贴板是用户报出来的那一条，
    /// 便签 / 计时器 / 全屏放大是同一批漏网的：`input_needs_frame` 只认它们**已经在跑**
    /// 的状态，恰好漏掉"按下开始这一下"）。
    ///
    /// 判据取"出厂串 ↔ 事件"两向都钉：只断言 `action_hotkey_wakes` 的话，串写错与事件
    /// 写错会互相抵消成假绿。
    #[test]
    fn resident_tool_hotkeys_wake_the_overlay() {
        let mut cfg = Config::default();
        // ★ 2026-09-26 起两把放大镜快捷键**出厂留空**（用户自配）——空串判不了
        //   "串 ↔ 事件"，这里显式配上有意义的值再测唤醒行为。
        cfg.magnifier_fullscreen.hotkey = "Alt+Z".into();
        cfg.magnifier_pip.hotkey = "Alt+P".into();
        // ★ 2026-09-27 起**便签、剪贴板与计时器的快捷键也出厂留空**（用户自配；见 §55
        //   「所有快捷键出厂留空」）。同一条理由：空串判不了"串 ↔ 事件"，这里显式配上
        //   有意义的值（`Alt+N` / `Alt+V` / `Alt+T`）再测唤醒行为。
        cfg.sticky_note.hotkey = "Alt+N".into();
        cfg.clipboard.hotkey = "Alt+V".into();
        cfg.global.timer.hotkey = "Alt+T".into();
        let alt = |c: char| InputEvent::KeyDown {
            key: KeyCode::Char(c),
            mods: Modifiers {
                alt: true,
                ..Default::default()
            },
            repeated: false,
            time_ms: 0,
        };
        let rt = Runtime::new(cfg.clone());
        for (name, spec, ev) in [
            ("剪贴板", cfg.clipboard.hotkey.as_str(), alt('v')),
            ("便签", cfg.sticky_note.hotkey.as_str(), alt('n')),
            ("计时器", cfg.global.timer.hotkey.as_str(), alt('t')),
            (
                "全屏放大",
                cfg.magnifier_fullscreen.hotkey.as_str(),
                alt('z'),
            ),
            ("画中画放大", cfg.magnifier_pip.hotkey.as_str(), alt('p')),
        ] {
            let InputEvent::KeyDown { key, mods, .. } = ev else {
                unreachable!()
            };
            assert!(
                hk_matches(spec, key, mods),
                "{name} 出厂串 {spec:?} 与测试事件对不上（判据自证失败）"
            );
            assert!(
                rt.action_hotkey_wakes(&ev),
                "{name} 快捷键必须能在 park 态唤醒覆盖层"
            );
        }
        // 停用（空串）后一律不再唤醒 —— 免得空串被 `hk_matches` 之外的分支放行。
        let mut off = cfg.clone();
        off.clipboard.hotkey.clear();
        off.sticky_note.hotkey.clear();
        off.global.timer.hotkey.clear();
        off.magnifier_fullscreen.hotkey.clear();
        let rt_off = Runtime::new(off);
        for c in ['v', 'n', 't', 'z'] {
            assert!(
                !rt_off.action_hotkey_wakes(&alt(c)),
                "'{c}' 停用后不该再唤醒"
            );
        }
    }

    /// ★★ 回归（2026-09-30 用户报告）：**卡片开关关着的脚本，它的热键既不能派发、
    /// 也不能唤醒 park 中的覆盖层。**
    ///
    /// 症状原文："我没有开启这个功能（AI 写脚本），但我能够直接用快捷键调出窗口"。
    /// 根因：`params[id]["hotkey"]`（填参数写进去）与 `composite_active`（卡片开关写进去）
    /// 是两条互不相干的持久化链路，热键名单只读前者。
    ///
    /// 判据覆盖两处消费点：`script_hotkeys`（派发 + `handle_action_key` 命中判定）与
    /// `action_hotkey_wakes`（park 唤醒）——两边共用同一份名单，一处改完两处齐，
    /// 这里就把"两边必须一起变"钉死。
    #[test]
    fn script_hotkeys_require_the_card_switch_to_be_on() {
        let spec_hotkey = |spec: &str| {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "hotkey".to_string(),
                serde_json::Value::String(spec.to_string()),
            );
            m
        };
        let mut cfg = Config::default();
        cfg.lua_ext
            .params
            .insert("scripts/ai_chat".to_string(), spec_hotkey("Alt+J"));
        let alt_j = InputEvent::KeyDown {
            key: KeyCode::Char('j'),
            mods: Modifiers {
                alt: true,
                ..Default::default()
            },
            repeated: false,
            time_ms: 0,
        };

        // ① 开关关着：名单空、不唤醒、不派发。
        let rt = Runtime::new(cfg.clone());
        assert!(
            rt.script_hotkeys().is_empty(),
            "开关关着时脚本热键不得进名单（正是这条漏出 bug）"
        );
        assert_eq!(
            rt.script_hotkeys_with_state().len(),
            1,
            "配了热键的脚本仍要出现在带状态的清单里（否则诊断日志无从下手）"
        );
        assert!(
            !rt.action_hotkey_wakes(&alt_j),
            "开关关着的脚本不该把 park 中的覆盖层叫醒"
        );

        // ② 勾开卡片开关后同一份配置立刻可用（含 park 唤醒）。
        let mut on = cfg;
        on.lua_ext
            .set_role_active("composite", "scripts/ai_chat", true);
        let rt_on = Runtime::new(on);
        assert_eq!(rt_on.script_hotkeys().len(), 1, "勾开后必须能派发");
        assert!(
            rt_on.action_hotkey_wakes(&alt_j),
            "勾开后必须能唤醒 park 中的覆盖层"
        );
    }

    /// 结构性门禁：`handle_action_key` 里消费的每一把快捷键，都必须在
    /// `action_hotkey_wakes` 里登记过。
    ///
    /// 为什么需要它而上面那条断言不够：上面那条只钉住**今天**这四把。明天再加一把
    /// 常驻工具快捷键时，没人会想起这份名单（剪贴板、便签、计时器、全屏放大全是这么
    /// 漏的），而漏掉的症状（"放一会儿按不动"）在开发环境里几乎复现不出来 —— 开发时
    /// 总有某个特效开着，`input_needs_frame()` 恒真、park 从不发生。
    ///
    /// 判据取**源码文本**而不是运行时反射：`self.config.…hotkey` 这条路径串是两边唯一
    /// 的公共名，读文本才能把"新增字段"变成红灯。
    #[test]
    fn every_action_hotkey_is_registered_as_a_waker() {
        // ★ 切生产段的手法同 `platform::clipboard_history::tests::production_src`：
        //   先找 `mod tests {` 再往回找它的 `#[cfg(test)]`，按第一次出现切会漏掉大半文件。
        let cut_at_tests = |src: &'static str| -> &'static str {
            let cut = src
                .find("mod tests {")
                .and_then(|i| src[..i].rfind("#[cfg(test)]"))
                .unwrap_or(src.len());
            &src[..cut]
        };
        let input = cut_at_tests(include_str!("input.rs"));
        let record = cut_at_tests(include_str!("record.rs"));
        let wakes = {
            let start = record
                .find("pub fn action_hotkey_wakes")
                .expect("找不到 action_hotkey_wakes");
            let end = record[start..]
                .find("\n    }")
                .map(|d| start + d)
                .unwrap_or(record.len());
            &record[start..end]
        };
        // `handle_action_key` 里所有 `self.config.<路径以 hotkey 结尾>` 的引用。
        // （截图三把走 `resolve_snap_hotkey(&self.config.screenshot, …)`，路径里没有
        //   hotkey 字样，扫不到 —— 它们由 `hk_matches_or_ctrl` 那两行保证，本门禁不覆盖。）
        let mut checked = 0usize;
        for (i, _) in input.match_indices("self.config.") {
            let tail = &input[i + "self.config.".len()..];
            // ★ 按**标识符段**切（`.clone()` 这类尾巴要丢掉）：否则抽出来的路径是
            //   `record.hotkey.clone`，跟名单里的 `self.config.record.hotkey` 对不上，
            //   门禁恒扫不到东西 —— 表现为"至少应扫到五把"这条自证断言挂掉。
            // ★★ 两处都得严格：
            //   ① 只取**紧贴其后**的那一段路径（ASCII 标识符 + `.`）。`char::is_alphanumeric()`
            //      把中文也算字母（`快捷键` 为真），放任它扫下去会把后面的注释粘成"路径"。
            //   ② `.clone()` 这类尾巴要丢掉，否则抽出的是 `record.hotkey.clone`，
            //      跟名单里的 `self.config.record.hotkey` 对不上（恒扫不到 = 门禁假绿）。
            let path: String = tail
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '.')
                .collect();
            let mut segs: Vec<&str> = path.split('.').collect();
            while segs.last().is_some_and(|s| !s.contains("hotkey")) {
                segs.pop();
            }
            if segs.is_empty() {
                continue;
            }
            let full = format!("self.config.{}", segs.join("."));
            assert!(
                wakes.contains(&full),
                "快捷键 `{full}` 在 `handle_action_key` 里被消费，却没登记进 \
                 `action_hotkey_wakes` —— park 态下按了不会有反应"
            );
            checked += 1;
        }
        assert!(
            checked >= 5,
            "门禁自己要有量：至少应扫到 录屏/GIF/裁剪/计时器/全屏放大 五把，实际 {checked}"
        );
        // 两个**模块**常驻快捷键（剪贴板 / 便签）不在 input.rs 里，单独钉。
        for p in [
            "self.config.clipboard.hotkey",
            "self.config.sticky_note.hotkey",
        ] {
            assert!(
                wakes.contains(p),
                "`{p}` 由模块 tick 消费（同样只在 frame() 里跑），必须登记进唤醒名单"
            );
        }
    }

    /// 倒计时数字：向上取整到整秒，且永不出 0。
    #[test]
    fn rec_countdown_seconds_rounds_up() {
        assert_eq!(rec_countdown_seconds(3000.0), 3);
        assert_eq!(rec_countdown_seconds(2001.0), 3);
        assert_eq!(rec_countdown_seconds(2000.0), 2);
        assert_eq!(rec_countdown_seconds(1.0), 1);
    }

    /// 倒计时到下一次数字跳变的等待毫秒数：落在 [1, 1000]，整秒边界齐。
    #[test]
    fn rec_countdown_next_change_stays_in_bounds() {
        assert_eq!(rec_countdown_next_change_ms(3000.0), 1000);
        // 2001 → 数字仍是 3，但 1ms 后就跨到 2000（变 2）：等待只有 1ms。
        assert_eq!(rec_countdown_next_change_ms(2001.0), 1);
        assert_eq!(rec_countdown_next_change_ms(2500.0), 500);
        assert_eq!(rec_countdown_next_change_ms(2000.0), 1000);
        assert_eq!(rec_countdown_next_change_ms(1500.0), 500);
        assert_eq!(rec_countdown_next_change_ms(1.0), 1);
    }
}
