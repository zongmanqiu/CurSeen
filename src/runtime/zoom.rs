//! 缩放：滚轮步进与全屏放大开关。
//!
//! 画中画与全屏放大共用 `platform::pip_window` 那块独立分层窗，两者互斥、全屏优先。
//!
//! ★ 两段语义（2026-09-26 用户口径）：面板勾选（`enabled`）= **载入模块**（武装），
//!   真正上屏/退出只能走**启停快捷键**切换 `active`（运行态）。帧门、渲染、滚轮武装
//!   一律看 `Config::pip_running()/fullscreen_running()`，不直读 `enabled`。

use super::*;

impl Runtime {
    /// 放大镜倍率步进：`dir` = +1 放大 / -1 缩小，步进 ±1，钳在量程（1–5）内。
    ///
    /// **唯一调用方是 Ctrl+Win+滚轮**（内置快捷键，见
    /// `platform::input::mouse` 的 WM_MOUSEWHEEL 注释）。画中画与全屏放大互斥，
    /// 自动选取当前运行的那个；都不在屏上时不消费（事件照常入队交给后面模块）。
    ///
    /// ★ 2026-09-21 夜修：旧版签名 `(pip_dir, fs_dir)` 的两个调用点都把 fs 方向
    ///   写死为 0，而本函数要求"方向非 0 且对应放大镜启用"才步进——于是
    ///   **全屏放大下快捷键/滚轮一律无效**（用户实测报障的直接根因，被本轮
    ///   门禁 `zoom_wheel_steps_the_enabled_magnifier` 钉住）。
    pub(crate) fn zoom_step(&mut self, dir: i32) -> bool {
        if dir == 0 {
            return false;
        }
        let (is_pip, cur) = if self.config.pip_running() {
            (true, self.config.magnifier_pip.zoom)
        } else if self.config.fullscreen_running() {
            (false, self.config.magnifier_fullscreen.zoom)
        } else {
            return false;
        };
        let (min, max) = (
            crate::core::config::PIP_ZOOM_MIN,
            crate::core::config::PIP_ZOOM_MAX,
        );
        let next = (cur as i32 + dir).clamp(min as i32, max as i32) as u32;
        if next == cur {
            // 到顶/到底：不刷屏（浮字不弹），但滚轮事件已消费、不再下发。
            return true;
        }
        // ★ 改完 config 必须**同步刷新面板信号**（面板在启动时就会自动打开，
        //   `sync_panel` 每帧跑 `apply_to` 把信号值写回 config——不同步的话，
        //   滚轮/快捷键的步进会在下一帧被面板的启动值覆盖，实测"每格都从 2 起步"）。
        let (minf, maxf) = (
            crate::core::config::PIP_ZOOM_MIN as f32,
            crate::core::config::PIP_ZOOM_MAX as f32,
        );
        if is_pip {
            self.config.magnifier_pip.zoom = next;
            if let Some(m) = self.panel.as_ref() {
                m.pip_zoom
                    .set(crate::ui::model::v2s(next as f32, minf, maxf));
                // ★ 三件套缺一不可：镜像文本不同步的话，下一帧 `slider_pair_sync`
                //   三方对账会把过期的文本值（偏离基准的一方）当赢家，把刚步进的
                //   zoom 拽回去——实测症状是"每滚一格都从原值重起步"（连续多条
                //   「→1×」就是 1→被拽回 2→再滚到 1 的循环）。
                m.pip_zoom_text.set(format!("{next}"));
            }
        } else {
            self.config.magnifier_fullscreen.zoom = next;
            if let Some(m) = self.panel.as_ref() {
                m.fs_zoom
                    .set(crate::ui::model::v2s(next as f32, minf, maxf));
                m.fs_zoom_text.set(format!("{next}"));
            }
        }
        self.panel_dirty = true; // 触发落盘 + needs_fresh 重抓
        self.action_notify.show(&format!("放大倍率 {next}×"));
        // 日志留痕：真机探针据此验证快捷键链路（temp/zoomwheel_probe.py）。
        log::info!("缩放：倍率 → {next}×（Ctrl+Win+滚轮）");
        true
    }

    /// 全屏放大：快捷键一键开关（切**运行态**）。
    ///
    /// ★ 2026-09-26 两段语义：勾选（`enabled`）只是载入模块，快捷键只在**已载入**时
    ///   生效，切的是 `active`（在不在屏上）。没载入就按快捷键 = 无动作（不是"替用户
    ///   勾上"——那等于绕过面板）。
    pub(crate) fn toggle_fullscreen_zoom(&mut self) {
        if !self.config.magnifier_fullscreen.enabled {
            return;
        }
        let on = !self.config.magnifier_fullscreen.active;
        self.set_fullscreen_active(on);
    }

    /// 全屏放大：置运行态为 `on`（快捷键 / Esc 两条路都汇到这里）。
    ///
    /// ★ 只动 `active`，不动 `enabled`：面板勾选是"载入"，用户没勾就不许被快捷键偷偷激活。
    /// ★ 面板开着时必须保证信号与配置一致（面板每帧 `apply_to` 写回配置）——这里
    ///   只动 `active`（不落盘、无信号），`enabled` 信号本来就同步，无需回写。
    pub(crate) fn set_fullscreen_active(&mut self, on: bool) {
        self.config.magnifier_fullscreen.active = on;
        if on {
            self.config.magnifier_pip.active = false;
        }
        // 让 `sync_panel` 把这次改动落盘（`active` 虽不落盘，`panel_dirty` 还会触发
        // 关窗兜底与重抓；保持与其它开关改动同一条路径）。
        self.panel_dirty = true;
        // ★ 立刻对账跨线程门：激活/退出的下一秒滚轮就要能用（ZOOM_WHEEL_ARMED），
        //   不能等下一帧的逐帧对账（帧门此时可能还在 Idle）。
        self.sync_input_gate();
    }

    /// 画中画放大：快捷键一键开关（切**运行态**，语义见 [`Self::toggle_fullscreen_zoom`]）。
    pub(crate) fn toggle_pip_zoom(&mut self) {
        if !self.config.magnifier_pip.enabled {
            return;
        }
        let on = !self.config.magnifier_pip.active;
        self.set_pip_active(on);
    }

    /// 画中画放大：置运行态为 `on`。互斥方向与 [`Self::set_fullscreen_active`] 相反：
    /// 激活画中画要退全屏（两个窗叠在屏上是非法状态）。只动 `active` 不动 `enabled`。
    pub(crate) fn set_pip_active(&mut self, on: bool) {
        self.config.magnifier_pip.active = on;
        if on {
            self.config.magnifier_fullscreen.active = false;
        }
        self.panel_dirty = true;
        // 同 `set_fullscreen_active`：切完立刻对账跨线程门。
        self.sync_input_gate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::testkit::*;

    /// 勾选(载入) + 激活 一起置上的小工具：帧门/渲染判据都是 `*_running()`。
    fn arm(rt: &mut Runtime, pip: bool, fs: bool) {
        rt.config.magnifier_pip.enabled = pip;
        rt.config.magnifier_pip.active = pip;
        rt.config.magnifier_fullscreen.enabled = fs;
        rt.config.magnifier_fullscreen.active = fs;
    }

    /// ★★ 回归（2026-09-26 用户实测"开启后 Ctrl+Win+滚轮完全不工作"）：钩子只认
    /// `ZOOM_WHEEL_ARMED` 原子，而它此前**只在启动时同步一次**——放大镜后来才被
    /// 快捷键激活，武装位永远停在 false，滚轮组合永远不被转发。钉死：快捷键切换必须
    /// **当次调用内**把武装位翻转过来（`set_*_active` 里显式 `sync_input_gate`）。
    #[test]
    fn hotkey_activation_flips_the_wheel_gate_immediately() {
        let _guard = INPUT_GATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut rt = Runtime::new(Config::default());
        rt.config.magnifier_pip.enabled = true;
        // 未激活：不武装（没有窗在屏，滚轮必须透传给下层应用）。
        rt.sync_input_gate();
        assert!(!crate::platform::input::mouse::zoom_wheel_armed());
        // 快捷键激活：当次调用内武装。
        rt.toggle_pip_zoom();
        assert!(
            crate::platform::input::mouse::zoom_wheel_armed(),
            "快捷键激活后滚轮门必须立刻打开"
        );
        // 再按退出：立刻解除。
        rt.toggle_pip_zoom();
        assert!(!crate::platform::input::mouse::zoom_wheel_armed());
        // 全屏同款。
        rt.config.magnifier_fullscreen.enabled = true;
        rt.toggle_fullscreen_zoom();
        assert!(crate::platform::input::mouse::zoom_wheel_armed());
        rt.toggle_fullscreen_zoom();
        assert!(!crate::platform::input::mouse::zoom_wheel_armed());
    }

    /// ★★ 回归（2026-09-26 用户实测"每滚一格都从原值重起步"）：滚轮步进只同步了
    /// 滑条信号、漏了**镜像文本**信号——面板每帧 `apply_to` 三方对账时，过期的
    /// 文本值是"偏离基准的一方"，按对账规则它赢，把刚步进的 zoom 拽回去。
    /// 日志证据：连续多条「缩放：倍率 →1×」（1 → 被拽回 2 → 再滚到 1 的循环）。
    #[test]
    fn wheel_stepping_survives_the_panel_writeback() {
        let mut rt = Runtime::new(Config::default());
        arm(&mut rt, true, false);
        rt.open_panel(crate::ui::model::PanelModel::from_config(
            &rt.config.clone(),
        ));
        let wheel = |delta: i32| InputEvent::MouseWheel {
            delta,
            horizontal: false,
            x: 0,
            y: 0,
            time_ms: 0,
        };
        let panel_writeback = |rt: &mut Runtime| {
            let model = rt.panel.as_ref().expect("面板已开");
            model.apply_to(&mut rt.config);
        };
        rt.feed(wheel(120));
        assert_eq!(rt.config.magnifier_pip.zoom, 3);
        panel_writeback(&mut rt); // 模拟面板下一帧写回（旧版在这里把 3 拽回 2）
        assert_eq!(
            rt.config.magnifier_pip.zoom, 3,
            "步进结果必须扛得住面板的每帧对账"
        );
        rt.feed(wheel(120));
        panel_writeback(&mut rt);
        assert_eq!(rt.config.magnifier_pip.zoom, 4, "连续两滚应累加到 4");
        // 文本镜像也要跟上（三件套纪律）。
        assert_eq!(rt.panel.as_ref().unwrap().pip_zoom_text.get(), "4");
        // 全屏同款（量程不同：FULLSCREEN_ZOOM_*）。
        arm(&mut rt, false, true);
        rt.panel = Some(crate::ui::model::PanelModel::from_config(
            &rt.config.clone(),
        ));
        rt.feed(wheel(120));
        panel_writeback(&mut rt);
        let after = rt.config.magnifier_fullscreen.zoom;
        rt.feed(wheel(120));
        panel_writeback(&mut rt);
        assert_eq!(
            rt.config.magnifier_fullscreen.zoom,
            after + 1,
            "全屏步进同样必须扛得住写回"
        );
    }

    /// ★ 回归：画中画放大没被帧门算进去时，只开它覆盖层会报告 `Idle` 而休眠，`render()`
    /// 永不跑 → 面板永不出现（用户报"画中画没有任何反应"）。
    /// ★ 缩放滚轮步进门禁（内置快捷键 Ctrl+Win+滚轮 的核心逻辑，钩子侧见
    /// `platform::input::mouse`）：谁在屏上调谁；到顶/到底钳住；都不在屏上无动作——
    /// **只载入未激活也不步进**（2026-09-26 两段语义）。
    #[test]
    fn zoom_wheel_steps_the_enabled_magnifier() {
        // 本用例末尾断言 `ZOOM_WHEEL_ARMED`（全局静态量），与另一条门禁用例串行。
        let _guard = INPUT_GATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut rt = Runtime::new(Config::default());
        let wheel = |delta: i32| InputEvent::MouseWheel {
            delta,
            horizontal: false,
            x: 0,
            y: 0,
            time_ms: 0,
        };
        // 画中画：前推放大、后拉缩小。
        arm(&mut rt, true, false);
        rt.feed(wheel(120));
        assert_eq!(rt.config.magnifier_pip.zoom, 3, "前推放大");
        rt.feed(wheel(-120));
        assert_eq!(rt.config.magnifier_pip.zoom, 2, "后拉缩小");
        rt.config.magnifier_pip.zoom = 5;
        rt.feed(wheel(120));
        assert_eq!(rt.config.magnifier_pip.zoom, 5, "到顶钳住");
        // ★ 全屏放大：旧版正是在这里失效（fs 方向恒为 0）。
        arm(&mut rt, false, true);
        rt.config.magnifier_fullscreen.zoom = 2;
        rt.feed(wheel(120));
        assert_eq!(rt.config.magnifier_fullscreen.zoom, 3, "全屏放大也要能步进");
        // 载入但未激活：无窗在屏，不该吞下层应用的滚轮。
        rt.config.magnifier_fullscreen.zoom = 2;
        rt.config.magnifier_fullscreen.active = false;
        rt.feed(wheel(120));
        assert_eq!(rt.config.magnifier_fullscreen.zoom, 2, "未激活不步进");
        // 都没载入：滚轮无动作。
        arm(&mut rt, false, false);
        rt.feed(wheel(-120));
        assert_eq!(rt.config.magnifier_fullscreen.zoom, 2, "都没开时无动作");
        // 武装开关同步：放大镜在屏时滚轮武装，关掉即解除。
        arm(&mut rt, true, false);
        rt.sync_input_gate();
        assert!(
            crate::platform::input::mouse::zoom_wheel_armed(),
            "放大镜在屏 → 滚轮武装"
        );
        arm(&mut rt, false, false);
        rt.sync_input_gate();
        assert!(
            !crate::platform::input::mouse::zoom_wheel_armed(),
            "放大镜全关 → 滚轮解除武装（透传）"
        );
    }

    /// ★ 全屏放大：与画中画同款的帧门判据（运行不许休眠、静止要休眠、首帧要强制出）。
    #[test]
    fn fullscreen_zoom_keeps_overlay_awake_and_sleeps_when_still() {
        let mut rt = Runtime::new(Config::default());
        arm(&mut rt, false, true);
        assert!(rt.input_needs_frame(), "全屏放大运行时覆盖层不该休眠");
        // 只载入未激活：不该撑着覆盖层不睡（testkit 默认其它功能全关）。
        rt.config.magnifier_fullscreen.active = false;
        assert!(!rt.config.fullscreen_running(), "未激活时 running 必须为假");

        rt.live_cursor = (0.0, 0.0);
        rt.config.magnifier_fullscreen.active = true;
        assert!(
            matches!(rt.frame_demand(), FrameDemand::Now),
            "全屏放大刚开启应强制出首帧"
        );
        // 抓屏/呈现成功那一支才会置 `has_frame`；测试环境（无桌面会话）可能失败，
        // 这里只要求"它停下来"——两种结果都不该把覆盖层钉在 Now 上空转。
        rt.render();
        let _ = rt.frame_demand();
        assert!(
            !rt.config.fullscreen_running() || rt.input_needs_frame(),
            "开着就不能休眠"
        );
    }

    /// ★★ 互斥：全屏放大运行时，画中画的窗不许还留在屏上（它会整个被盖住，
    /// 而且两个窗同时抓屏会互相拖慢）。
    #[test]
    fn fullscreen_zoom_shuts_the_pip_window_down() {
        let mut rt = Runtime::new(Config::default());
        // 先只开画中画出一次帧，确认这条环境下窗**真的**能立起来（不能的话这条用例
        // 会退化成恒真的假绿，所以把结果打出来）。
        arm(&mut rt, true, false);
        rt.render();
        let pip_was_shown = rt.pip_magnifier.as_ref().is_some_and(|m| m.is_shown());
        eprintln!(
            "[门禁环境] 画中画窗已建立 = {pip_was_shown}（false 表示当前无桌面会话，此用例退化为弱判据）"
        );
        assert!(
            pip_was_shown,
            "前置失败：画中画的窗没立起来，这条用例会变成恒真的假绿 —— \
             请在有桌面会话的环境下重跑（与 `tests/pip_probe.rs` 同款前提）"
        );

        rt.config.magnifier_fullscreen.enabled = true; // 载入
        rt.config.magnifier_fullscreen.active = true; // 激活
        rt.render();
        assert!(
            !rt.pip_magnifier.as_ref().is_some_and(|m| m.is_shown()),
            "全屏放大优先：画中画的窗必须被收掉"
        );
    }

    /// ★ Esc 必须能退出全屏放大：它是置顶全屏不透明的，会把控制面板一起盖住，
    /// 快捷键万一被改坏/冲突，Esc 是唯一还能出来的路。Esc 只退运行态，不动载入勾选。
    #[test]
    fn escape_exits_fullscreen_zoom() {
        let mut rt = Runtime::new(Config::default());
        arm(&mut rt, false, true);
        let ev = InputEvent::KeyDown {
            key: KeyCode::Esc,
            mods: Modifiers {
                ctrl: false,
                alt: false,
                shift: false,
                win: false,
            },
            repeated: false,
            time_ms: 0,
        };
        rt.handle_action_key(&ev);
        assert!(
            !rt.config.magnifier_fullscreen.active,
            "裸 Esc 必须退出全屏放大"
        );
        assert!(
            rt.config.magnifier_fullscreen.enabled,
            "Esc 只退运行态，载入勾选不动"
        );

        // 带修饰键的 Esc 不拦（Ctrl+Esc 是系统组合，抢了会坏事）。
        rt.config.magnifier_fullscreen.active = true;
        let with_ctrl = InputEvent::KeyDown {
            key: KeyCode::Esc,
            mods: Modifiers {
                ctrl: true,
                alt: false,
                shift: false,
                win: false,
            },
            repeated: false,
            time_ms: 0,
        };
        rt.handle_action_key(&with_ctrl);
        assert!(
            rt.config.magnifier_fullscreen.active,
            "Ctrl+Esc 不该被我们吃掉"
        );
    }

    /// ★★ 两段语义（2026-09-26 用户口径）：快捷键只在**已载入**时生效、切的是运行态；
    /// 未载入按快捷键 = 无动作；激活一边要退另一边（两个窗叠屏是非法状态）；
    /// 载入态本身不互斥（两把键可以各自武装，按哪把开哪个）。
    #[test]
    fn magnifier_hotkeys_toggle_active_only_when_armed() {
        let mut rt = Runtime::new(Config::default());
        rt.config.magnifier_fullscreen.hotkey = "Alt+Z".into();
        rt.config.magnifier_pip.hotkey = "Alt+P".into();
        let press = |c: char| InputEvent::KeyDown {
            key: KeyCode::Char(c),
            mods: Modifiers {
                ctrl: false,
                alt: true,
                shift: false,
                win: false,
            },
            repeated: false,
            time_ms: 0,
        };
        // 未载入：按快捷键无动作（不得替用户勾上）。
        rt.handle_action_key(&press('z'));
        assert!(!rt.config.magnifier_fullscreen.active);
        assert!(
            !rt.config.magnifier_fullscreen.enabled,
            "载入不该被快捷键代劳"
        );
        // 载入后：第一按激活、第二按退出；enabled 全程不动。
        rt.config.magnifier_fullscreen.enabled = true;
        rt.handle_action_key(&press('z'));
        assert!(rt.config.magnifier_fullscreen.active, "第一按激活");
        assert!(rt.config.magnifier_fullscreen.enabled);
        rt.handle_action_key(&press('z'));
        assert!(!rt.config.magnifier_fullscreen.active, "再按退出");
        // 激活画中画要退全屏（互斥在运行态）。
        rt.config.magnifier_pip.enabled = true;
        rt.config.magnifier_fullscreen.active = true;
        rt.handle_action_key(&press('p'));
        assert!(rt.config.magnifier_pip.active);
        assert!(!rt.config.magnifier_fullscreen.active, "画中画激活要退全屏");
        // 载入态不互斥：两个模块可以同时武装。
        assert!(
            rt.config.magnifier_pip.enabled && rt.config.magnifier_fullscreen.enabled,
            "勾选=载入，两者可同时载入"
        );
    }

    /// ★★ 按需驻留（2026-09-26 用户口径："卡片没勾选就不为它付出内存"）：
    /// 未勾选时模块**整个不存在**（None）；勾选后下一次渲染构造出来；
    /// 取消勾选再一次渲染后模块连同原生窗一起释放（`PipWindow` Drop → DestroyWindow）。
    #[test]
    fn magnifier_modules_live_only_while_the_card_is_checked() {
        let mut rt = Runtime::new(Config::default());
        rt.render();
        assert!(
            rt.pip_magnifier.is_none() && rt.fullscreen_magnifier.is_none(),
            "出厂（未勾选）模块不得驻留内存"
        );
        // 勾选画中画（仅载入，未激活）：下一次渲染把模块建起来，但不出窗。
        rt.config.magnifier_pip.enabled = true;
        rt.render();
        assert!(rt.pip_magnifier.is_some(), "勾选后应构造模块");
        // 取消勾选：再一次渲染即卸载。
        rt.config.magnifier_pip.enabled = false;
        rt.render();
        assert!(rt.pip_magnifier.is_none(), "取消勾选必须释放模块");
        // 全屏同理。
        rt.config.magnifier_fullscreen.enabled = true;
        rt.render();
        assert!(rt.fullscreen_magnifier.is_some());
        rt.config.magnifier_fullscreen.enabled = false;
        rt.render();
        assert!(rt.fullscreen_magnifier.is_none());
    }
}
