//! 控制面板侧的联动：面板开关、配置回写、自动更新、通知浮字。
//!
//! `impl PanelHost for Runtime` 留在 `mod.rs`（它是 Runtime 对外的脸），这里放实现细节。

use super::*;

impl Runtime {
    /// 面板是否打开着。
    pub fn panel_open(&self) -> bool {
        self.panel.is_some()
    }

    /// 标注文字输入窗是否开着（由覆盖层在开窗/关窗时登记）。
    pub fn text_window_open(&self) -> bool {
        self.text_window_open
    }

    /// 登记文字输入窗的开/关。
    pub fn set_text_window_open(&mut self, open: bool) {
        self.text_window_open = open;
    }

    /// 取一次「进入标注模式，请收起控制面板」的请求（边沿触发，取过即无）。
    ///
    /// ★ 修复：面板开着时用快捷键进入标注，此前会落进死态——输入整体挂起
    ///   （`input_suspended`）+ 覆盖层保持穿透（`sync_click_through` 对面板开着
    ///   的例外），标注“进得去、动不了、退不出”。用户按 Alt+G 的预期是
    ///   「现在开始写字」，面板挡在中间只会卡死交互，故进入边沿请求把它收起。
    ///   标注中随时可再按面板快捷键（`Ctrl+Shift+D`）把面板叫回来，那时的挂起
    ///   是刻意语义（调设置不落笔）。
    ///
    /// 由覆盖层在 UI 线程消费（通道回调 + 250ms 兜底）：面板是独立顶层窗口，
    /// 只有它能投递 `WM_CLOSE`。
    pub fn take_panel_close_hint(&mut self) -> bool {
        std::mem::take(&mut self.panel_close_hint)
    }

    /// 往屏幕中央的反馈浮字上挂一句话（`action_notify`）。
    ///
    /// 存在的理由：`action_notify` 是私有的，而独立窗口这类调用方（裁剪窗、控件条）
    /// 拿不到它。与其让每个窗口自己去 `Runtime` 里挖字段，不如在这里开一个窄口子。
    pub fn show_notify(&mut self, msg: &str) {
        self.action_notify.show(msg);
    }

    /// 面板窗口关闭时调用：把残留改动一次性写回 `Config` 并立即落盘。
    ///
    /// 必须在 UI 线程调用（来自 `Window::on_close_request`）。此时窗口还在（WM_CLOSE
    /// 先于销毁），正好把最后位置采集进配置——下次打开还原到同一处（主窗口状态记忆）。
    pub fn close_panel(&mut self) {
        if let Some(model) = self.panel.take() {
            // 位置采集（★ 主窗口状态记忆）：最小化中/找不到窗口时返回 None，保留旧记录。
            // 存物理屏幕像素，与 `Window::position` 同一坐标系（含副屏负坐标）。
            if let Some((x, y)) =
                crate::platform::win_util::window_pos_by_title(crate::ui::panel::TITLE)
            {
                if self.config.ui.panel_pos != Some((x, y)) {
                    self.config.ui.panel_pos = Some((x, y));
                    self.config_version = self.config_version.wrapping_add(1);
                }
            }
            if model.apply_to(&mut self.config) {
                self.config_version = self.config_version.wrapping_add(1);
                // 与 `sync_panel` 同一条理由：关面板即落盘，那一帧也要画出来（例如刚关掉的
                // 面板上最后改的悬浮标参数）。
                self.overlay_repaint_hint = true;
            }
            if let Err(e) = self.config.save() {
                log::warn!("关闭面板时写配置失败: {e}");
            }
            // 面板状态（model + 信号树）刚随 take() 释放 → 2 秒后整理工作集
            // （`platform::working_set`）：释放掉的页交还系统，"关面板后内存回落"。
            crate::platform::working_set::request_trim_soon();
        }
        self.panel_dirty = false;
    }

    /// 取一次「配置刚被面板改过，请覆盖层补一帧」的请求（边沿触发，取过即无）。
    ///
    /// 与 [`Self::take_panel_close_hint`] 同一套写法：Runtime 不碰窗口，由覆盖层的 250ms
    /// 定时器（UI 线程）取走并 `request_repaint()`。取走后 `frame()` 会在本帧末尾清掉它
    /// （那一帧已经把新配置画出来了），故不会反复踢帧。
    pub fn take_overlay_repaint_hint(&mut self) -> bool {
        std::mem::take(&mut self.overlay_repaint_hint)
    }

    /// 每帧（以及覆盖层的定时器里）把面板信号 diff 回配置，并按防抖落盘。
    ///
    /// **不能只挂在"出帧"路径上**：三项功能全关时覆盖层不出帧（帧门判据就是这三个开关），
    /// 而"打开某个功能"恰恰只能从面板做——把同步只放在 `frame()` 里，用户就会撞上
    /// "勾了开关没反应"的死循环。所以定时器那条路也必须调这里。
    pub fn sync_ui(&mut self) {
        self.sync_panel();
        // 鼠标输入门：配置可能刚从面板/托盘/预设改过，同步给桥接线程。
        // 挂在这里而不是 `frame()`：三项功能全关时覆盖层不出帧（帧门判据就是这几个开关），
        // 而"打开某个功能"只能从面板做——只挂在 `frame()` 上会撞上"打开了但桥接还在丢
        // 鼠标事件"的死循环。定时器那条 250ms 对账同样会走到这里。
        self.sync_input_gate();
        // 贴图环容量（面板可改；缩容时立即淘汰最旧的）。
        self.snap.set_cap(self.config.screenshot.pin_ring_cap);
        // 剪贴板轮询：把两次贴图按键之间的复制也收进环（见 `poll_clipboard`）。
        // 同样挂在这里：覆盖层休息时（全关/空闲）定时器那条路照样走到。
        self.poll_clipboard();
        // 麦克风试听：面板按钮 ↔ 真实监听链路的对账（见 `sync_mic_monitor`）。
        // 放在 `sync_panel` 之后：降噪档/强度刚被面板改过的这一帧就该听到新参数的效果。
        self.sync_mic_monitor();
    }

    /// 把面板上的「开始/停止试听」对账成真实的 WASAPI 直通链。
    ///
    /// 要管三件事，少一件都会留下"看着在听、其实没听"或"关不掉"的残局：
    ///
    /// 1. **面板关了要停。** 关掉面板就再没有"停止试听"这个按钮了 —— 不停就是个
    ///    用户无法关掉的声音。
    /// 2. **参数变了要重建。** 设备 / 降噪档 / 强度 / 门限都是 `Initialize` 时定死的，
    ///    改不了；不重建的话，"换了麦克风"听到的还是上一个设备。
    /// 3. **起不来要把按钮弹回去。** 否则按钮停在「停止试听」上，用户以为在听。
    ///    （设备被独占、麦克风被拔掉都会走到这条。）
    ///
    /// ★ 已知代价：`MicMonitor::start` 会阻塞等链路建好的确认（最长 3 秒）。它只在
    ///   **用户点下按钮后的那一帧**跑一次（之后 `same` 短路），所以正常路径只等几十毫秒；
    ///   最坏情况（设备不可用）会让覆盖层卡 3 秒。这个取舍是刻意的 —— 换来的好处是
    ///   "链路没建起来"能当场报给用户并复位按钮，而不是让按钮亮着却不出声。
    pub(crate) fn sync_mic_monitor(&mut self) {
        use crate::platform::audio_capture::MicMonitor;

        // 先把要用的信息取出来，结束对 `self.panel` 的借用（下面要改信号、动 monitor）。
        let (want, key) = match self.panel.as_ref() {
            Some(m) => {
                let c = &self.config.record;
                (
                    // ★ 必须**同时**满足「按钮要求听」和「麦克风来源不是关闭」。
                    //   2026-09-22 起面板那一行不再随来源显隐（关闭时点按钮会先把来源切成
                    //   「默认」），所以这里的守卫不再是"给消失的按钮兜底"，而是防御
                    //   **运行期把来源改成关闭**（面板上直接改下拉）却忘了停听的状态：
                    //   来源一旦是关闭，试听立刻停，"停止试听"按钮也就跟着有意义了。
                    m.mic_monitor_on() && c.mic_source().is_some(),
                    (
                        // 三态收成一个值：`Some("")` = 跟随系统默认（`resolve_device` 认空名）。
                        c.mic_source().unwrap_or_default().to_string(),
                        c.mic_denoise.clone(),
                    ),
                )
            }
            None => (false, (String::new(), String::new())),
        };

        if !want {
            if self.mic_monitor.is_some() {
                log::info!("麦克风试听已停止");
            }
            self.mic_monitor = None;
            self.mic_monitor_key = None;
            return;
        }
        if self.mic_monitor.is_some() && self.mic_monitor_key.as_ref() == Some(&key) {
            return;
        }

        // 参数变了或首次开启：先停旧的（`Drop` 会 join 线程，不会留下两条同时在放）。
        self.mic_monitor = None;
        let (device, denoise) = key.clone();
        match MicMonitor::start(&device, &denoise) {
            Ok(m) => {
                self.mic_monitor = Some(m);
                self.mic_monitor_key = Some(key);
                log::info!("麦克风试听已开启（{denoise} 档）");
            }
            Err(e) => {
                log::warn!("麦克风试听开启失败: {e}");
                self.mic_monitor_key = None;
                if let Some(m) = self.panel.as_mut() {
                    m.rec_monitor.set(false);
                }
                self.action_notify.show("麦克风试听失败");
            }
        }
    }

    pub(crate) fn sync_panel(&mut self) {
        // 先把模型取出，避免 `apply_to(&mut self.config)` 与 `self.panel` 同时借用。
        let Some(model) = self.panel.take() else {
            return;
        };
        if model.apply_to(&mut self.config) {
            self.config_version = self.config_version.wrapping_add(1);
            self.panel_dirty = true;
            // ★ 请覆盖层补一帧：改完参数光标多半停在面板上不动，帧门判"光标没移动"返回
            //   Idle，屏上会一直留着改动前那一帧（"改了没反应"）。见字段注释。
            self.overlay_repaint_hint = true;
        }
        self.panel = Some(model);

        if !self.panel_dirty {
            return;
        }
        let now = tick_ms();
        if now.saturating_sub(self.panel_last_save_ms) <= PANEL_SAVE_DEBOUNCE_MS {
            return;
        }
        match self.config.save() {
            Ok(()) => {
                self.panel_dirty = false;
                self.panel_last_save_ms = now;
            }
            Err(e) => log::warn!("写配置失败: {e}"),
        }
    }

    // ─── 自更新 ───

    /// 把一条后台更新事件落到状态上。
    ///
    /// **只在 UI 线程调用**（信号不是 `Send`）。下载完成后这一步会直接起更新脚本并退出
    /// 进程——替换 exe 必须由进程外的脚本来做，本进程已经被加载进内存了。
    pub fn apply_update_event(&mut self, ev: UpdateEvent) {
        match ev {
            UpdateEvent::CheckStarted => {
                self.update.busy.set(true);
                self.update.ready.set(false);
                self.update.progress.set(-1.0);
                self.update.status.set("正在检查更新…".to_string());
            }
            UpdateEvent::UpToDate { latest } => {
                log::info!("检查更新：已是最新 {latest}");
                self.update.busy.set(false);
                self.update.ready.set(false);
                self.update.status.set(format!("已是最新版本 {latest}"));
            }
            UpdateEvent::Available { latest, url } => {
                log::info!("检查更新：发现新版本 {latest} -> {url}");
                self.update.busy.set(false);
                self.update.ready.set(true);
                self.update.set_url(url);
                self.update
                    .status
                    .set(format!("发现新版本 {latest}，点「下载并更新」即可"));
            }
            UpdateEvent::CheckFailed { error } => {
                self.update.busy.set(false);
                self.update.ready.set(false);
                self.update.status.set(format!("检查失败：{error}"));
                log::warn!("检查更新失败: {error}");
            }
            UpdateEvent::DownloadProgress { received, total } => {
                // `total == 0`：服务端没给长度，只报已下载的量，进度条保持"不确定"。
                if total == 0 {
                    self.update.progress.set(-1.0);
                    self.update
                        .status
                        .set(format!("正在下载… {:.1} MB", received as f64 / 1_048_576.0));
                } else {
                    let p = (received as f32 / total as f32).clamp(0.0, 1.0);
                    self.update.progress.set(p);
                    self.update
                        .status
                        .set(format!("正在下载… {}%", (p * 100.0).round() as i32));
                }
            }
            UpdateEvent::Downloaded { path } => {
                self.update.progress.set(1.0);
                self.update
                    .status
                    .set("下载完成，正在重启以完成更新…".to_string());
                // 之后不再接受点击：进程马上就没了。
                self.update.busy.set(true);
                self.launch_installer(&path);
            }
            UpdateEvent::DownloadFailed { error } => {
                self.update.busy.set(false);
                self.update.progress.set(-1.0);
                self.update.status.set(format!("下载失败：{error}"));
                log::warn!("下载更新失败: {error}");
            }
            UpdateEvent::Launching => {
                self.update
                    .status
                    .set("更新脚本已启动，程序即将退出…".to_string());
            }
        }
    }

    /// 把一次后台的 `temp/` 占用统计落到面板信号上。
    ///
    /// **只在 UI 线程调用**（`Signal` 不是 `Send`）——与 [`Self::apply_update_event`] 同一条纪律。
    /// 文案分档见 `ui::temp_usage::describe`（"尚未产生文件" / "暂无文件" / `44.1 MB（30 个文件）`）。
    pub fn apply_temp_usage(&mut self, usage: crate::core::temp_usage::TempUsage) {
        log::debug!(
            "临时目录统计：exists={} files={} bytes={}",
            usage.exists,
            usage.usage.files,
            usage.usage.bytes
        );
        self.temp_usage.apply(&usage);
    }

    /// 起替换脚本并退出本进程。失败时把状态行改成错误信息、放开按钮让用户重试。
    pub(crate) fn launch_installer(&mut self, zip: &std::path::Path) {
        match crate::update::installer::install(zip) {
            Ok(bat) => {
                log::info!("更新脚本已启动，本进程退出让位：{}", bat.display());
                // 配置先落盘再走：更新脚本排除了 config.json，用户设置不会丢。
                self.close_panel();
                log::logger().flush();
                std::process::exit(0);
            }
            Err(e) => {
                log::error!("启动更新脚本失败: {e}");
                self.update.busy.set(false);
                self.update.status.set(format!("启动更新脚本失败：{e}"));
            }
        }
    }

    // ─── 计时器（§1 #15）───
    //
    // **快捷键即开关**（同录屏）：没有"启用"闸门，快捷键为空串即停用。状态用墙钟 `Instant`
    // 驱动（而非逐帧累加 raw_ms），因为覆盖层空闲时会 park 掉、帧与帧之间可能隔很久——
    // 逐帧累加会把这段时间漏掉，计时必然偏慢。
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::drawing::controller::DrawMode;
    use crate::runtime::testkit::*;

    /// 进入标注的边沿必须请求收起面板；退出不是边沿；hint 只报一次。
    #[test]
    fn entering_drawing_requests_panel_close_once() {
        let mut rt = drawing_only();
        assert!(!rt.take_panel_close_hint());
        rt.drawing.controller_mut().toggle(DrawMode::Screen, 0.0);
        rt.frame();
        assert!(rt.take_panel_close_hint(), "进入标注应请求收起面板");
        assert!(!rt.take_panel_close_hint(), "hint 只报一次");
        // 退出（跨过 0.5s 去抖）不该再请求。
        rt.drawing.controller_mut().toggle(DrawMode::Screen, 1.0);
        rt.frame();
        assert!(!rt.take_panel_close_hint(), "退出标注不是进入边沿");
    }

    /// 两个倒计时都要报，取**更早**的那个。
    ///
    /// 守的是"谁先命中就返回谁"那种写法：标注浮字还剩 2 秒、按键弹窗只剩 1 秒时返回标注
    /// 那个，按键弹窗就得在屏上多停 1 秒才被擦掉（它的淡出帧没人排）。
    #[test]
    fn earlier_of_the_two_deadlines_wins() {
        // ★ 判据是「**取更早的那个**」，不是「浮字恒为 2000ms」：从 `toggle` / `feed`
        //   到 `frame()` 之间的**真实耗时**会被 `raw_ms` 算进浮字的剩余量里（实测
        //   160~280ms —— 取决于首次读配置、加载键盘方案与工具栏资源的快慢），
        //   写死 1900 会随机器与冷热启动翻红（实测过 1838 与 1722 两个值）。
        //   于是两边各钉一侧的量级：30s 那组**上界 2000**（报成弹窗那一侧就是 bug），
        //   1s 那组上界 1000（浮字恒 2s，报成 2s 那侧就是 bug）。
        //   下界一律给 1 = 不设下界（`FrameDemand::After` 本就保证 ≥1）。
        for (fade_s, low, high, why) in [
            (
                30_u32,
                1_u32,
                2000_u32,
                "弹窗静置 30s，不该顶掉标注浮字的 2s",
            ),
            (1, 1, 1000, "弹窗静置 1s，应比标注浮字更早"),
        ] {
            let mut cfg = Config::default();
            cfg.drawing.enabled = true;
            cfg.key_display.enabled = true;
            cfg.key_display.fade_delay = fade_s;
            let mut rt = Runtime::new(cfg);
            rt.drawing.controller_mut().toggle(DrawMode::Screen, 0.0);
            rt.feed(InputEvent::KeyDown {
                key: KeyCode::Char('a'),
                mods: Modifiers::default(),
                repeated: false,
                time_ms: 0,
            });
            rt.frame(); // 事件被击键回显消费（画面因此有变化）

            match rt.frame_demand() {
                FrameDemand::After(ms) => assert!(
                    (low..=high).contains(&ms),
                    "{why}：期望 {low}..={high}ms，实际 {ms}ms"
                ),
                other => panic!("{why}：应报出截止时刻，实际 {other:?}"),
            }
        }
    }
}
