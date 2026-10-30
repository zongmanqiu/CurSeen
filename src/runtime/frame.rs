//! 帧调度与帧需求。
//!
//! `Runtime` 的**出帧**这一半：一帧里推进什么（`frame`）、怎么变成像素（`render`）、
//! 以及「这一帧到底要不要出」（`frame_demand` / `input_needs_frame`）。
//!
//! 单独成文件的原因：这四条就是**性能**的全部所在——改错一处就是「空转 1000 帧/秒」
//! 或「该出帧时不出」。把它们并在一起，评审时才能一次看完「谁在排帧」。

use super::*;

impl Runtime {
    /// 帧推进：tick 时钟、更新模块状态。
    pub fn frame(&mut self) {
        let tick = self.clock.tick();

        // 虚拟桌面矩形（物理像素）注入各模块，替换硬编码 1920×1080。
        let vs = self.screen;
        self.trail.set_screen(vs);
        self.key.set_screen(vs);
        self.vk.set_screen(vs);
        self.drawing.set_screen(vs);

        // ★ 面板改的画笔同步进书写控制器（修复：改了粗细/颜色不生效）。
        //   放在输入队列消费之前：改完当帧落笔就是新参数。输入队列为空时本帧也要同步
        //   （用户改完设置没动鼠标，下一次落笔前 controller 也必须已拿到新值）。
        self.drawing.sync_settings(&self.config);

        // ★ 截图编辑态的选区：每帧推一次镜像给控制器（它据此画"选区外压暗"与摆工具栏）。
        //
        //   必须在**输入队列消费之前**：用户刚在 `feed` 里拖完选区，这一帧就要按新位置画出来，
        //   否则压暗与工具栏整整慢一帧 —— 拖动时看得出"工具条追不上框"。
        self.drawing
            .set_region(self.edit.as_ref().map(|e| sel_rect(e.sel)));

        // 控制面板 / 文字输入窗打开期间挂起标注输入：用户正在调设置或打字，不该顺手
        // 画出一笔、也不该让画笔快捷键把工具切走。两个来源分开传：Esc 豁免需要区分来源
        // （面板挂着时 Esc 仍可退出标注，文字窗挂着时 Esc 归输入窗）。
        self.drawing
            .set_suspension(self.panel.is_some(), self.text_window_open);

        // 面板 diff：必须在借用 config 之前跑——它会改 config / config_version。
        self.sync_ui();

        // ★ 内存回收：功能模块一旦由「开」变「关」，立刻把它的独占资源还回去。
        //   放在 `sync_ui` 之后：配置刚被面板更新过，这一帧看到的就是最新开关值。
        self.reclaim_disabled_modules();

        // ★ 跨线程输入门逐帧对账（2026-09-26 修复"Ctrl+Win+滚轮失效"）：桥接线程的
        //   钩子只认这几个原子（MOUSE_INPUT_NEEDED / ZOOM_WHEEL_ARMED /
        //   MOUSE_ECHO_ARMED / VK_MOUSE_ARMED），而快捷键/面板随时会翻转它们的判据
        //   （放大镜激活、虚拟键盘换方案……）。★ 此前它只在启动时同步一次——放大镜
        //   后来才激活，ZOOM_WHEEL_ARMED 永远停在 false，滚轮组合永远不转发。
        //   全是 bool 比较，逐帧跑毫无压力。
        self.sync_input_gate();

        // 隐藏系统指针：激光笔类特效启用时藏起真指针，退出即恢复。同样只在边沿调一次 API。
        self.sync_cursor_hide();

        // 计时器推进：基于墙钟，与帧率无关（空闲休眠期间也不丢时间）。
        if self.timer_running {
            self.advance_timer();
        }

        // 虚拟键盘点击命中区（鼠标钩子线程读那块共享区）：放在面板 diff 之后，
        // 发的就是最新开关态 —— 用户刚勾上「鼠标穿透」，下一帧键帽就不再吃点击。
        // ★ 只按 `enabled` / `click_through` 发布，**不看控制面板开没开**：给面板让路
        //   改成按**落点**让（`hit::claim` 里的 `over_our_window`）。用"面板开着就整体
        //   关掉命中区"的话，用户在面板开着时永远点不动键帽，且与勾不勾穿透无关。
        self.vk.publish_hit_region(&self.config.virtual_keyboard);

        // ★ 用本帧真实光标位置驱动"光标跟随"特效：消除"画在事件过期坐标上"的滞后。
        //   `cursor_pos()` 是 `GetCursorPos` 的薄封装（物理像素、虚拟桌面坐标系）；每帧一次
        //   调用极廉价，且只在已经出帧时跑（Idle 休眠期不调用）。这条同时喂 `live_cursor`
        //   （续帧门禁的"是否移动"判据）与悬浮标中心（激光笔 / 聚光灯跟着真光标走）。
        let (mx, my) = cursor_pos();
        self.live_cursor = (mx as f32, my as f32);
        self.trail.set_live_cursor(mx as f32, my as f32);

        let cfg = &self.config;
        let v = self.config_version;

        // ★ 真实墙钟注入悬浮标脚本（`wall_ms` 全局）：静止检测 / 超时淡出只能用它。
        //   `tick.elapsed_ms` 是逐帧 capped 累加，光标静止不出帧时**不推进**，拿它算
        //   "静止了多久"恒等于 0（「晃动定位」的静止时长判定因此完全失效）。
        self.trail.set_wall_ms(tick_ms() as f64);

        // 拖尾：消费 feed() 压入的采样点。`cfg` 的借用到此为止（下面输入队列循环要先借
        // `&mut self` 做快捷键捕获，不能再持 `&self.config`，故循环内重新借）。
        self.trail.tick(&tick, cfg, v);

        // ★★ 击键回显：**每帧先推进一次**（`&None` = 只走时间，不消费事件）。
        //
        // 淡出是**时间驱动**的，而此前 `key.tick` 只在"有输入事件"的那条循环里被调用 ——
        // 用户一停手（尤其只开击键回显时，鼠标事件被 `MOUSE_INPUT_NEEDED` 整条挡在桥接
        // 线程外）就再也没有 tick，弹窗**定格在屏幕上**。`frame_demand` 按
        // `next_change_ms` 约好的那些唤醒帧只是把同一个不透明度重画一遍（分层窗帧间
        // 不清缓冲，最后一帧就是最终可见画面），于是"淡出延迟"看上去**完全无效**。
        // 这里补上每帧推进：静止期睡到 `fade_delay`，醒来后连帧推进淡出，到点移除。
        //
        // ★★ 虚拟键盘 / 屏幕书写是**同一个坑的另两处**（2026-09-18 一并修）：它们的
        //   `tick` 同样只写在下面那条输入队列循环里，症状同样是"没输入就永远不动"，
        //   而且比击键回显更难看 —— 不是"卡一下"而是**永久残留**：
        //   · `vk`：过期高亮由 `sweep` 按**墙钟**清；没输入就永远不清，而
        //     `next_change_ms` 报出的那个"到点"唤醒帧又不推进 → 收敛到 `Idle` 之后
        //     屏上键帽**一直亮着**（看起来像键盘卡住了）。
        //   · `drawing`：提示浮字（「屏幕标注模式」/「已退出绘图」）按 `dt` 递减剩余
        //     时间；没输入就永不递减 → 那句话**永远不消失**，还每 2 秒空转一帧。
        //   注意 drawing 的 `tick` 内部还会 `sync_settings`（幂等，变化才重建），
        //   这里每帧调一次与下面循环里的调用并不冲突。
        self.key.tick(&tick, &self.config, &None);
        self.vk.tick(&tick, &self.config, &None);
        self.drawing.tick(&tick, &self.config, &None);
        self.sticky.tick(&tick, &self.config, &None);
        // ★ 剪贴板面板窗口记忆（config.json 单写者链路的最后一棒）：剪贴板线程在
        //   挪窗/切置顶后把新几何投进全局槽位，这里取走并入内存 Config 再落盘。
        //   必须在借 `&self.config` 给模块 tick 之前做（可变借用在前）。
        if let Some(g) = crate::platform::clipboard_history::take_pending_panel_state() {
            self.config.clipboard.panel = Some(g);
            if let Err(e) = self.config.save() {
                log::warn!("剪贴板: 面板窗口记忆落盘失败: {e}");
            }
        }
        self.clipboard.tick(&tick, &self.config, &None);

        // 击键回显 / 屏幕书写：逐条消费输入队列，避免一帧只吃最后一条事件。
        let queue = std::mem::take(&mut self.input_queue);
        for ev in &queue {
            // 快捷键捕获：面板开着且处于捕获态时，按键被"捕获框"吃掉并写回对应信号，
            // 不传给标注 / 击键回显（避免捕获的键顺手切了工具或触发面板快捷键）。
            // `try_capture_hotkey` 借 `&mut self`，必须在借 `self.config` 之前。
            if self.try_capture_hotkey(ev) {
                continue;
            }
            self.key.tick(&tick, &self.config, &Some(*ev));
            self.vk.tick(&tick, &self.config, &Some(*ev));
            // 击键音效：只认"按下"（KeyUp / 移动与本模块无关）；脚本与音量先对账
            // （面板改完下一帧生效），再决定发不发声。修饰键单独按下由模块自己过滤。
            if let InputEvent::KeyDown {
                key,
                mods,
                repeated,
                ..
            } = ev
            {
                // 复合脚本（role="composite"）的键盘事件入口：独立于击键音效开关
                // ——复合脚本自成一体，宿主只管把事件送到，听不听由脚本决定。
                self.trail.push_key(*key, *mods, *repeated);
                if self.config.key_sound.enabled {
                    self.key_sound.sync(&self.config.key_sound);
                    self.key_sound.on_key_down(*key, *mods, *repeated);
                }
            }
            self.drawing.tick(&tick, &self.config, &Some(*ev));
            self.sticky.tick(&tick, &self.config, &Some(*ev));
            self.clipboard.tick(&tick, &self.config, &Some(*ev));
            // 截图 / 录屏快捷键：全局语义，即便所有可视化功能都关着也要响应
            // （用户可能只装了「一键截图」，没开任何特效）。放在捕获/各模块之后，
            // 不影响它们的事件消费。
            self.handle_action_key(ev);
        }

        // ★ 画笔那几个**跨模块动作**的统一执行点：截图编辑态的 `Esc`（取消）/ 工具栏
        //   「保存」「退出」，以及屏幕标注的「另存为」/ `Esc` 退出（退出即留档），
        //   全部汇到同一个 [`DrawRequest`]，在这里执行。
        //
        //   放在输入队列消费**之后**：`Esc` 正是这一帧的队列里被
        //   `DrawingModule::handle_key` 翻译出来的。而工具栏那颗按钮产自 `begin_stroke`
        //   （也在队列里），同样落在这儿。
        self.handle_draw_request();

        // 录屏线程中途的致命错误：在这里消费掉并复位状态，否则 `recorder` 永远挂着、
        // "在录"假死（`take_error` 只有 `finish()` 一个消费点的话，线程死了没人取）。
        let rec_err = self.recorder.as_ref().and_then(|r| r.take_error());
        if rec_err.is_some() {
            if let Some(rec) = self.recorder.take() {
                let _ = rec.finish(); // 线程已死，finish 只为 join + 清理
            }
            log::warn!("录制已中止");
            self.action_notify.show("录制失败");
        }

        // ★ 录制心跳停摆 → 主动结束（2026-10-02）。录制线程一旦楔住就不报错、不写帧，
        //   停止位也唤不醒，界面却一直挂着"在录"。这里按 [`REC_STALL_GIVEUP_MS`] 主动
        //   收尾：收尾仍走后台限时等待（`stop_recording_now`），结论照常回 UI。
        //   只认"MP4 且出过帧之后才停摆"：GIF 通路没接心跳（短录制、也无此类事故），
        //   起步那一两秒本来也没有帧 —— 两者都不能被误判。
        // ★★ 停摆有**两条**里程表（2026-10-06 采集/编码解耦后）：
        //   ① 采集心跳 —— 采集线程卡住（`stall_ms`）。
        //   ② 写入帧数 —— **编码线程**卡住。解耦之后采集侧照常出帧（这正是解耦想要
        //      的效果），所以只看①的话"编码器楔住了"永远发现不了：界面一直挂着"在录"，
        //      文件却早就不再增长。
        let stalled_acquire = self
            .recorder
            .as_ref()
            .filter(|r| r.kind() == crate::platform::recorder::RecKind::Mp4 && r.frames() > 0)
            .map(|r| r.stall_ms())
            .unwrap_or(0);
        let stalled_encode = match self
            .recorder
            .as_ref()
            .filter(|r| r.kind() == crate::platform::recorder::RecKind::Mp4)
            .map(|r| r.written())
        {
            // 没在录 MP4：里程表复位，下一段从头算。
            None => {
                self.rec_written_seen = 0;
                self.rec_written_at = None;
                0
            }
            Some(w) => {
                if w != self.rec_written_seen {
                    self.rec_written_seen = w;
                    self.rec_written_at = Some(Instant::now());
                    0
                } else {
                    // 只在"出过帧之后才停摆"才算数：起步那一两秒本来也还没写帧。
                    self.rec_written_at
                        .map(|t| t.elapsed().as_millis() as u64)
                        .unwrap_or(0)
                }
            }
        };
        let stalled = stalled_acquire.max(stalled_encode);
        if stalled >= crate::runtime::record::REC_STALL_GIVEUP_MS {
            // ★ 哪条停摆就报哪条：共用一个阶段字段会指错地方（见
            //   `Recorder::enc_progress` 的说明）。
            let (stage, tick) = if stalled_encode >= stalled_acquire {
                self.recorder
                    .as_ref()
                    .map(|r| (r.enc_progress(), r.written()))
                    .unwrap_or((crate::platform::recorder::RecStage::Startup, 0))
            } else {
                self.recorder
                    .as_ref()
                    .map(|r| r.progress())
                    .unwrap_or((crate::platform::recorder::RecStage::Startup, 0))
            };
            log::warn!(
                "录制心跳停摆 {:.0} 秒，主动结束：卡在「{}」（第 {tick} 帧）",
                stalled as f64 / 1000.0,
                stage.name()
            );
            // ★ 阶段名同时抛给用户：排查这类"卡死"唯一的现场证据就是"卡在哪一步"，
            //   而它只存在于录制线程的原子量里。用户不用翻日志就能把这句话发回来。
            self.action_notify
                .show(&format!("录制卡住（{}），正在结束", stage.short()));
            self.stop_recording_now();
        }

        // 录制收尾（后台线程）的结论到达：补 UI 反馈（完成/中断/失败 + 提示窗 + 裁剪）。
        // ★ 2026-10-02 起停止录制不在 UI 线程收尾（见 `stop_recording_now`），这里是其
        //   唯一的回程口；放在错误消费之后，两条路不打架。
        self.consume_finished_recordings();

        // 分段拼接（后台线程）的结论到达：把"最近产物"改指合并后的单个 MP4。
        // 与上面同一套槽位模式 —— 后台线程碰不到 Runtime，只能经全局槽位回传。

        // 录制倒计时：逐帧递减，归零才真正开始录制（屏幕中央「即将录制 3/2/1」，
        // 见 `draw_rec_countdown`）。停止快捷键在 `handle_action_key` 里取消倒计时。
        // ★ 用 raw_ms 而非 dt_ms：dt_ms 经指数滑动平均 + clamp(0.5, 50) 平滑，
        //   帧间隔大时（park 态唤醒延迟）实际流逝的时间被截断，3 秒倒计时会明显变长。
        if let Some(remain) = self.rec_countdown_ms.as_mut() {
            *remain -= tick.raw_ms;
            if *remain <= 0.0 {
                self.rec_countdown_ms = None;
                self.start_recording_now();
            }
        }

        // ★ 进入标注模式的边沿 → 请求收起控制面板（修复见 `take_panel_close_hint`）。
        //   放在输入队列消费之后：toggle 恰是在这一帧的队列里发生的。
        let drawing_active = self.drawing.controller().is_drawing();
        if drawing_active && !self.drawing_was_active {
            self.panel_close_hint = true;
        }
        self.drawing_was_active = drawing_active;

        // 截图 / 录屏反馈浮字推进（按真实经过时间，停帧期间它不走，故用 `elapsed_ms`）。
        self.action_notify.tick(tick.elapsed_ms as f32 / 1000.0);
    }

    /// 渲染：清空 DrawList，让模块填充绘制指令。
    pub fn render(&mut self) {
        self.draw_list.clear();
        self.sync_magnifier_lifecycle();

        // ★★ 全屏放大优先（它与画中画互斥：它是置顶全屏不透明的，画中画就算画了也
        //   被盖住）。互斥的**归一**在 `ui::model::apply_to` 里做（面板改配置那条路），
        //   这里兜住剩下两条：老配置/手改 json 里两个都开着、以及快捷键那条路 —— 不兜
        //   就是两个窗叠在屏幕上。
        let fullscreen_on = self.config.fullscreen_running();
        // 全屏优先在这里落地为**卸载**：画中画模块整个 take 掉（窗随 Drop 销毁），
        // 全屏退出后下一帧由生命周期对账重建。
        //
        // ★★ 跳过整屏软栅格，但**只在没有东西要画的时候**（★ 2026-10-03 用户报"画中画
        //   放大时有鼠标特效，全屏放大的时候没有"）。
        //   那个不对称有**两个**独立成因，缺一个都修不掉：
        //   ① 本来这行 `return` 让 trail / key / vk / drawing 全都不画（几何原因）；
        //   ② 即便画了，全屏放大窗是置顶、alpha=255 的**整屏不透明**窗，而 z 序守护
        //      豁免了同进程窗口 ⇒ 覆盖层永远抢不到它上面（z 序原因，见
        //      `platform::topmost` 的 `is_fullscreen_zoom`）。
        //   两处都补上，两档放大才一致。
        //
        // 判据必须是"窗**已经**在屏上"——首帧还没显示，那一帧必须走完整路径去建窗、
        // 出第一帧，否则窗永远出不来。
        //
        // ★ 代价：全屏态下覆盖层要真出帧了，1080p 每帧几毫秒的软栅格。所以只在
        //   **确实有东西要画**时继续（`overlay_has_content_under_fullscreen`）；
        //   什么都不开时仍走老路直接 return，代价与改动前完全一致。
        if fullscreen_on {
            if let Some(f) = &mut self.fullscreen_magnifier {
                if f.is_shown() {
                    f.draw(&self.config, self.live_cursor);
                    if !self.overlay_has_content_under_fullscreen() {
                        return;
                    }
                }
            }
        }

        let cfg = &self.config;
        // 绘制顺序（先画在底）：trail → key → vk → drawing。反馈类（计时器/浮字/
        // 倒计时/白闪）在下面单独画，恒在最顶。
        self.trail.draw(&mut self.draw_list);
        self.key.draw(&mut self.draw_list, cfg);
        self.vk.draw(&mut self.draw_list, cfg);
        self.drawing.draw(&mut self.draw_list, cfg);
        // 便签画在各自的原生窗上（`StickyNoteModule::draw` 空实现）。
        self.sticky.draw(&mut self.draw_list, cfg);
        // 画中画放大：**不再画在覆盖层上**，走自己的原生窗（理由见
        // `modules/magnifier::pip` 模块头的"面板为什么不再画在覆盖层上"）。
        // 位置仍在这条渲染链里：它与其它模块共用同一套帧门节流，放在这里每帧恰好
        // 推进一次；挪到 `frame()` 会因为"出一帧 ≠ 渲染一次"而多跑。
        //
        // ★ 全屏放大开着时**整个跳过**：否则这一行会把它刚被 shutdown 掉的窗又画回来。
        if !fullscreen_on {
            if let Some(p) = &mut self.pip_magnifier {
                p.draw(cfg, self.live_cursor);
            }
        }
        // 全屏放大（同一个底层，面板 = 整个虚拟屏；见 `magnifier::fullscreen`）。
        if let Some(f) = &mut self.fullscreen_magnifier {
            f.draw(cfg, self.live_cursor);
        }
        // 计时器读秒（盖在特效之上、反馈浮字之下）。
        self.draw_timer();
        // 截图 / 录屏的反馈浮字（中央，覆盖在特效之上）+ 全屏白闪（最上层）。
        if self.action_notify.is_showing() {
            self.action_notify.draw(&mut self.draw_list, self.screen);
        }
        self.action_notify
            .draw_flash(&mut self.draw_list, self.screen);
        // 录制倒计时（屏幕中央「即将录制 N」）。用户口径：录制前给 3-2-1 提示，
        // 录制中**不再**显示时长指示器（顶部 REC 计时已按需求移除）。
        if let Some(remain) = self.rec_countdown_ms {
            // `rec_pending` 要到归零才被 `start_recording_now` 取走，所以倒计时期间它
            // 就是"这次录什么"的可信来源（兜底 MP4 与 `start_recording_now` 同一处置）。
            let kind = self
                .rec_pending
                .unwrap_or(crate::platform::recorder::RecKind::Mp4);
            draw_rec_countdown(&mut self.draw_list, self.screen, remain, kind);
        }

        // ★ 聚光灯增量帧检测（补丁 032）：指令表组装完之后判形态。命中且上一帧
        // 同色时，把指令表**替换**成「只画脏区」的增量版（见
        // [`Self::detect_spotlight_partial`]），overlay 据此跳过整窗清屏、后端据此
        // 只呈现脏区。任何其它内容（拖尾/击键/计时器/GIF 选框……）都会让指令表
        // 偏离两指令形态，自动落回整窗路径。
        self.detect_spotlight_partial();
        // ★ 本帧已经按最新配置画出来了，清掉"请补一帧"的请求（见 `overlay_repaint_hint`）。
        //   它只在"配置改了、但那一帧还没画出来"时才有意义；清在这里而不是 `frame()` 末尾，
        //   是因为**画出像素**才算数（`frame()` 只推进状态，全屏放大分支还会整帧跳过渲染）。
        self.overlay_repaint_hint = false;
    }

    /// 「鼠标特效模块是否在用」——帧门 / 输入门 / 续帧判据的**唯一**入口。
    ///
    /// 为什么不直接用 `config.trail_module_active()`：那条只按"`composite_active` 列表
    /// 非空"给结论，把只开自己原生窗的一次性脚本（AI 对话，只有 `schema` + `run`）
    /// 也算进去 —— 覆盖层为它整屏缓冲常驻（1080p 实测 +8.8 MB 私有工作集）并 60fps
    /// 整屏重绘，而那条链路上一个像素都没画（2026-09-30 内存归因）。
    /// 复合脚本那一半改问运行期的实例（保守分支见 `CompanionHost::any_needs_frame`）。
    fn trail_gate(&self) -> bool {
        self.config.trail_visual_active() || self.composite_needs_frame()
    }

    /// 复合脚本是否真的需要覆盖层的帧（配置粗判据 ‖ 运行期精判据）。
    fn composite_needs_frame(&self) -> bool {
        self.config.lua_ext.composite_running() && self.trail.composite_needs_frame()
    }

    /// 全屏放大开着时，覆盖层上**有没有东西要画**。
    ///
    /// ★ 这条决定全屏态下还要不要付整屏软栅格的代价（1080p 每帧几毫秒），所以口径
    ///   与 [`Self::input_needs_frame`] 的视觉部分**保持一致**：拖尾 / 悬浮标 / 点击效果 /
    ///   按键回显 / 虚拟键盘 / 标注，任何一样开着才画。
    ///
    /// ★ 刻意**不**把 `action_notify`（反馈浮字）与录制倒计时算进来：那两条是"用完即走"
    ///   的短时反馈，全屏放大时压在放大画面上没人看得清（放大的是屏幕内容，浮字在它
    ///   上面反而碍事），而它们会让整屏重绘持续整整几秒 —— 收益近零、代价实打实。
    ///
    /// 用 [`Self::trail_gate`]（`trail_visual_active ‖ composite_needs_frame`）而不是
    /// `config.trail_module_active()`：后者把一次性脚本也算成要帧，理由见
    /// `core::config::trail_visual_active` 的注释（1080p 实测多占 8.8 MB）。
    fn overlay_has_content_under_fullscreen(&self) -> bool {
        self.trail_gate()
            || self.config.key_display.enabled
            || self.config.virtual_keyboard.enabled
            || self.config.drawing.enabled
            || self.drawing.controller().is_drawing()
    }

    /// 休眠态下补一次模块对账（配置版本变了才真干活）。
    ///
    /// ★ 打破"新勾上复合脚本却没人把它同步进来"的死锁：同步平时挂在 `tick()` 里，
    ///   而 `tick()` 只在 `paint()` 中被调用，覆盖层休眠时一次都不跑。调用点 = 250ms
    ///   对账与 UI 通道的 `Update`（都在读帧门**之前**）。
    pub fn sync_trail_if_stale(&mut self) {
        let v = self.config_version;
        self.trail.sync_if_stale(&self.config, v);
    }

    /// 输入事件是否需要唤醒一帧。
    ///
    /// 空闲时没有帧在跑，输入到来必须主动踢一帧才画得出来；但功能全关时输入没有任何
    /// 视觉后果，踢帧只会把持续的鼠标移动钉成几十 fps 的空转（整屏重绘）。所以判据读
    /// **配置**而不是模块状态：模块状态是滞后一帧的，刚打开开关的那一帧会漏踢。
    /// ★ 各功能只听自己卡片的开关（`trail_module_active` / `key_display.enabled` /
    /// `drawing.enabled`），没有功能总开关。
    pub fn input_needs_frame(&self) -> bool {
        self.trail_gate()
            || self.config.key_display.enabled
            || self.config.virtual_keyboard.enabled
            || self.config.drawing.enabled
            // 已进入标注/白板模式：即便"画笔"总开关关着（用户是用 Alt+G 这种快捷键叫起来的），
            // 也要继续出帧——否则刚开起来的标注立刻卡住不动。
            || self.drawing.controller().is_drawing()
            // 反馈浮字正在显示：这段时间里必须持续出帧把它淡出。
            || self.action_notify.is_showing()
            // ★ 录制倒计时中：中央「即将录制 N」每秒跳一次数字，必须持续出帧。
            || self.rec_countdown_ms.is_some()
            // ★ 画中画放大：跟随鼠标、逐帧抓屏重画。不把 `enabled` 算进"需要帧"，覆盖层会
            //   因只有它一个功能而休眠（park），而帧调度器在 `paint()` 里才跑——park 后
            //   再没有 paint，放大画面永不出现（用户报"勾选后没反应"）。
            || self.config.pip_running()
            // ★ 全屏放大：同画中画，跟着鼠标逐帧抓屏重画。漏了它就是"开了没反应"。
            || self.config.fullscreen_running()
            // ★ 录制进行中：停止快捷键在 `frame()` 的输入队列里派发（见
            //   `handle_action_key`）。若覆盖层在录制期间休眠，`frame()` 不再跑、
            //   停止快捷键就失效——表现为「关掉录制鼠标后按不了停止键」（其它可视化功能
            //   也全关时，录制是唯一的常驻动作）。故录制中必须保持覆盖层常驻出帧。
            //   录制线程本身独立跑、不依赖覆盖层渲染，这里只保证快捷键能被处理。
            || self.recorder.is_some()
            // ★ 上一次录制还在收尾 / 有"换格式"待办要兑现：结论要靠
            //   `consume_finished_recordings` 消费，而它挂在 `frame()` 里、只有出帧才跑。
            //   漏了它就是浮字淡出后覆盖层 park → 收尾结论无人取 → `rec_finishing` 永远
            //   true → 快捷键从此只弹「正在结束上一次录制…」，直到任意输入把覆盖层唤醒。
            || self.rec_finishing
            || self.rec_after_finish.is_some()
            // ★ GIF 选框挂着：边框是**覆盖层**画的（`overlay::draw_gif_frame`），
            //   而选框是在区域窗里确认的 —— 覆盖层此刻多半正 park 着，不为任何输入出帧。
            //   不把选框算进"需要帧"，框完的那一瞬间就没有下一次重绘，边框永远不上屏
            //   （2026-09-20 用户报的"选择的区域没有边框"）。
            //   录制中也要留着：边框由蓝变红是"正在录"的唯一视觉信号。
            || self.gif_sel.is_some()
            // ★ 长截图采集进行中：边框画在覆盖层上，而 park 会把缓冲刷成全透明再释放
            //   （见 overlay 的休眠注释）——不为它保持醒着，边框落一层 park 就没了。
            //   停止快捷键与 Esc 另有 action_hotkey_wakes 登记（park 空档也不丢），
            //   这里保证的是"边框持续可见"而不是"按键可达"。
            //   frame_demand 无需跟：边框静止一个像素不变，会话期间覆盖层醒着但不重绘，
            //   分层窗帧间不清缓冲，画面自然保持。
            || self.scroll.is_some()
            || self.scroll_ready.is_some()
            // 计时器运行中：按快捷键开始 / 停止那一刻也要踢一帧把读秒画出来 / 抹掉。
            || self.timer_running
    }

    /// 是否有模块消费**鼠标**输入（见 [`MOUSE_INPUT_NEEDED`]）。
    ///
    /// 比 [`Self::input_needs_frame`] 窄：击键回显不开帧却完全不看鼠标，放行它只会把
    /// 1000 条/秒的鼠标移动变成 1000 次/秒的通道唤醒。
    pub fn mouse_input_needed(&self) -> bool {
        self.trail_gate()
            || self.config.drawing.enabled
            // 已进入标注模式（可能总开关关着靠快捷键叫起来的）：鼠标要交给画笔。
            || self.drawing.controller().is_drawing()
            // ★ 虚拟键盘「捕获点击」时（启用且未勾穿透）必须收鼠标：点键帽要注入真实按键。
            //   之前漏了这条 —— 只开键盘、关掉拖尾/绘制时，桥接线程在 `MOUSE_INPUT_NEEDED`
            //   为 false 下把鼠标事件整条丢掉，`feed` 收不到 `MouseDown`，点键盘一律不注入。
            //   只在"启用且未勾穿透"时收，避免穿透态（不捕获）无谓地唤醒 1000 次/秒通道。
            || (self.config.virtual_keyboard.enabled
                && !self.config.virtual_keyboard.click_through)
            // ★ GIF 选框交互中（选框挂着、未录制、无叠层）：边框要能拖 / 缩放就**必须**收
            //   鼠标。之前漏了这条 —— `gif_interaction_enabled()` 时覆盖层已进 GifRegion、
            //   `WS_EX_TRANSPARENT` 已清，但桥接线程在 `MOUSE_INPUT_NEEDED=false` 下把
            //   `MouseDown` 整条丢弃，`OverlayCanvas::on_event` 收不到按下 → 边框永远拖不动、
            //   8 点永远缩不动（2026-09-20 实测：日志里选框存在期间反复"丢弃 MouseDown"）。
            || self.gif_interaction_enabled()
    }

    /// 放大镜模块的**生命周期对账**（每次渲染开头跑一次，开销两次 bool 比较）。
    ///
    /// ★ 2026-09-26 用户口径："卡片没勾选就不为它付出内存"。勾选（`enabled`）只是
    ///   载入许可：勾了才在下一次渲染把模块构造出来；取消勾选 = 载出，`take()` 连同
    ///   抓屏缓冲与原生窗（`PipWindow` Drop → DestroyWindow）一起释放。运行态
    ///   （`active`）只决定画不画，不决定模块在不在——真正上屏仍只能由快捷键触发。
    pub(crate) fn sync_magnifier_lifecycle(&mut self) {
        if self.config.fullscreen_running() {
            // 全屏在屏：画中画整个卸下（反正被盖住）。
            self.pip_magnifier = None;
        } else if self.config.magnifier_pip.enabled && self.pip_magnifier.is_none() {
            self.pip_magnifier = Some(PipMagnifier::new());
        } else if !self.config.magnifier_pip.enabled {
            self.pip_magnifier = None;
        }
        if self.config.magnifier_fullscreen.enabled {
            if self.fullscreen_magnifier.is_none() {
                self.fullscreen_magnifier = Some(FullscreenMagnifier::new());
            }
        } else {
            self.fullscreen_magnifier = None;
        }
    }

    /// 把鼠标输入门同步到跨线程原子（构造后与每帧对账调用）。
    pub(crate) fn sync_input_gate(&self) {
        let mouse_needed = self.mouse_input_needed();
        MOUSE_INPUT_NEEDED.store(mouse_needed, Ordering::Relaxed);
        // 滚轮门与钩子的吞事件开关同源：任一放大镜开着就启用。
        // 关掉放大镜的下一帧这里立即恢复 false，Ctrl+滚轮立刻还给下层应用。
        // 缩放滚轮的武装开关与钩子同源：任一放大镜在屏上即武装。放大镜关着时
        // Ctrl+Win+滚轮 毫无干预（它在系统里本就没有默认行为）。
        let zoom_wheel = self.config.pip_running() || self.config.fullscreen_running();
        crate::platform::input::mouse::set_zoom_wheel_armed(zoom_wheel);
        // 脚本滚轮门：有脚本声明 `on_wheel` 时钩子才转发普通滚轮（Scroll 特效用）。
        // 与帧门一样按"当前运行实例"对账——没有滚轮脚本时每滚一格的开销完全为零。
        crate::platform::input::mouse::set_script_wheel_armed(self.trail.any_wheel_wanted());
        // 滚轮回显门：击键回显开着**且**处于「全部」模式时才让钩子转发普通滚轮
        // （show_mouse 由 display_mode 派生——独立开关已删，见
        // `key_display::settings::from_config`。关掉任意一项，下一帧就把
        // "每滚一格唤醒一帧"的开销还回去）。虚拟键盘当前方案里有滚轮键帽
        // （`WheelUp`/`WheelDown`）时同样武装 —— 否则滚轮键帽永远收不到事件。
        // wants_wheel 由 vk 的 tick 每帧刷新（构造 VkConfig 是顺手的事），最多滞后一帧。
        let mouse_echo = (self.config.key_display.enabled
            && self.config.key_display.display_mode == "all")
            || (self.config.virtual_keyboard.enabled && self.vk.wants_wheel());
        crate::platform::input::mouse::set_mouse_echo_armed(mouse_echo);
        // ★ 虚拟键盘的鼠标门：方案里含鼠标键帽就要收按下/抬起，**与鼠标穿透无关**
        //   （穿透只管"点键帽注不注入按键"，高亮是另一回事）。
        //   与 `wants_wheel` 同款：由 vk 的 tick 每帧刷新，最多滞后一帧。
        let vk_mouse = self.config.virtual_keyboard.enabled && self.vk.wants_mouse();
        VK_MOUSE_ARMED.store(vk_mouse, Ordering::Relaxed);
        // ★ 鼠标低级钩子按需装卸（2026-10-24）：上面四路里只要有一路真需要鼠标，才让
        //   系统把鼠标事件派发进本进程。全关时卸载 —— 否则每一次全系统鼠标移动都会
        //   调用我们的回调（即便桥接层随后把事件丢掉），正是用户说的"没开功能却在跑"。
        crate::platform::input::hook::set_mouse_hook_enabled(
            mouse_needed || zoom_wheel || mouse_echo || vk_mouse,
        );
    }

    /// 是否需要继续重绘。
    ///
    /// 判据是「画面**会变**」而不是「有东西」：悬浮标的指令每帧都一样，拿"有内容"
    /// 续帧就是永久空转。提示浮字的淡出是定时事件，用 `next_change_ms` 报出截止时刻，
    /// 平台可以一直睡到那时。
    ///
    /// ★ 标注模式**空闲时必须报 `Idle`**：标注是一件"用户动一下才有变化"的事，画完最后一笔
    /// 到用户下次按下之间，画面一个像素都不变。漏掉这一条的表现是"进入标注模式后 CPU 一直
    /// 满着"——因为覆盖层是整屏 CPU 软渲染，每一帧都是实打实的全屏绘制。所以按钮显示与
    /// 标注浮字这两路**各报各的截止时刻，取更早的那个**，而不是谁先命中就返回谁。
    pub fn frame_demand(&self) -> FrameDemand {
        if self.trail.is_animating() {
            return FrameDemand::Now;
        }

        // ★ 光标跟随特效（悬浮标 / 拖尾）的续帧门禁 —— 这是"快速移动卡顿 + CPU 随速度涨"
        // 的修复核心。
        //
        // 旧逻辑：这类特效静止时也返回 `Idle`，于是 windui 走**事件驱动分支**——只在鼠标钩子
        // 每收到一次移动 `request_repaint` 触发的 `WM_PAINT` 里提交，**提交率被鼠标回报率绑架
        // （125–1000Hz）**，没有 vsync 上限。每次移动都来一次整屏 `UpdateLayeredWindow`，快速
        // 移动时提交远超额、且画在事件过期坐标上 → 卡顿感 + CPU 跳动到 8%。
        //
        // 新逻辑：特效激活且"光标相对上一呈现帧移动了"就返回 `Now`，windui 的**动画分支**会把
        // 提交限到原生刷新率（`due = frame_ms`）；光标静止立即 `Idle` → 0% CPU。移动判据在
        // 这里"消费"一次（`last_presented` 写回），所以下一帧若光标没动必然休眠。
        let follow = self.config.hover_running()
            || self.trail_gate()
            || self.config.pip_running()
            || self.config.fullscreen_running();
        if follow {
            let live = self.live_cursor;
            if live != self.last_presented.get() {
                self.last_presented.set(live);
                return FrameDemand::Now;
            }
            // 放大镜刚开启但还没抓出首帧：强制出一帧出画——否则光标恰停在上一呈现帧
            // 的位置（常见于刚打开时的静止状态）会被上面误判成 `Idle`，面板一直不出现。
            if self.config.pip_running()
                && self.pip_magnifier.as_ref().is_none_or(|m| !m.has_frame())
            {
                return FrameDemand::Now;
            }
            // 面板宽/高/倍率/位置滑条被改动、但鼠标没在桌面上动（停在设置窗上）：也要
            // 出帧重画，否则帧门休眠、改动反映不到面板上（用户报"滑条没反应"）。
            if self.config.pip_running()
                && self
                    .pip_magnifier
                    .as_ref()
                    .is_some_and(|m| m.needs_fresh(&self.config))
            {
                return FrameDemand::Now;
            }
            // 全屏放大：与上面画中画那两条一一对应（首帧 + 倍率改动）。
            if self.config.fullscreen_running()
                && self
                    .fullscreen_magnifier
                    .as_ref()
                    .is_none_or(|m| !m.has_frame() || m.needs_fresh(&self.config))
            {
                return FrameDemand::Now;
            }
            // ★★ 鼠标静止 ≠ 屏幕静止（2026-09-21 用户报"点出菜单面板不刷新"）：放大镜
            //   在屏上时，覆盖层必须按"静止重抓节拍"持续醒着出帧——上一帧画过去超过
            //   `LOOK_INTERVAL` 就安排一次定时唤醒，而不是返回 Idle 睡死。★ 必须排在
            //   两个放大镜的"首帧 / 参数改动强制出帧"之后：那两条是 Now，优先级更高。
            //
            //   为什么是**定时出帧轮询**而不是"等屏幕内容变化的事件"：菜单弹出这类内容
            //   变化没有直达本进程的事件通道（DXGI 的 dirty 区回调意味着要把采集从 GDI
            //   换成 DDA 常驻会话），而 30FPS 的小面积 BitBlt + GDI 合成本来就是放大镜
            //   的稳态成本——鼠标移动时它本来就一直在付。两个放大镜都没开时不进这条，
            //   静止零功耗的既有语义不变。
            if self.config.pip_running() || self.config.fullscreen_running() {
                return FrameDemand::After(crate::modules::magnifier::LOOK_INTERVAL_MS);
            }
            // ★ 悬浮标脚本自己要的续帧（`draw` 返回一个毫秒数 = "多久之后再来一帧"）。
            //
            //   「晃动定位」的定位环 3 秒淡出就靠这条：环出现后手就停了，而分层窗帧间
            //   **不清缓冲**——没有续帧，屏上就永久定格在环刚出现那一帧（看起来像"环
            //   不消失"）。返回数字即排一次定时唤醒；脚本淡出结束后不再返回 → 自动收敛
            //   回 `Idle`，静止零功耗的既有语义不变。
            if self.config.hover_running() {
                if let Some(ms) = self.trail.hover_next_frame_ms() {
                    return FrameDemand::After(ms.max(1));
                }
            }
            return FrameDemand::Idle;
        }

        // 标注提示浮字（进入/退出各 2 秒淡出）。
        let mut soonest = self.drawing.next_change_ms();
        // 按键弹窗的淡出。
        if self.config.key_display.enabled {
            if let Some(ms) = self.key.next_change_ms() {
                soonest = Some(soonest.map_or(ms, |cur| cur.min(ms)));
            }
        }
        // 虚拟键盘：按住修饰键期间画面不变但松开要重画（保守 100ms 轮询）；淡出同击键回显。
        if self.config.virtual_keyboard.enabled {
            if let Some(ms) = self.vk.next_change_ms() {
                soonest = Some(soonest.map_or(ms, |cur| cur.min(ms)));
            }
        }
        // 截图 / 录屏反馈浮字。
        if let Some(ms) = self.action_notify.remain_ms() {
            soonest = Some(soonest.map_or(ms, |cur| cur.min(ms)));
        }
        // 录制倒计时：数字每秒跳一次，睡到下一次跳变即可（1 fps 的代价换"看得见的
        // 3-2-1"，远低于录屏本身的开销）。
        if let Some(remain) = self.rec_countdown_ms {
            let ms = rec_countdown_next_change_ms(remain);
            soonest = Some(soonest.map_or(ms, |cur| cur.min(ms)));
        }

        // 计时器：跑着就持续出帧。1fps 足够显示整秒跳变，但倒计时归零需在 ≤200ms 内
        // 被 `advance_timer` 捕获，故续帧上限压到 200ms（5fps）。
        if self.timer_running {
            soonest = Some(soonest.map_or(200, |cur| cur.min(200)));
        }

        match soonest {
            None => FrameDemand::Idle,
            Some(0) => FrameDemand::Now,
            // 至少 1ms：0 已在上面归到 `Now`，0 会让平台立刻再出帧。
            Some(ms) => FrameDemand::After(ms.max(1)),
        }
    }

    // ─── 控制面板 ───
    //
    // 登记动作现在是 [`PanelHost::open_panel`] 的实现（见文件末尾）——面板不再直接调用
    // 本运行时的方法，而是经那个契约取用。
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::drawing::controller::DrawMode;
    use crate::runtime::testkit::*;

    /// 本文件**生产代码**文本（`#[cfg(test)]` 之前那一段）——结构性门禁用。
    /// 同款先例见 `platform::clipboard_history::tests::production_src`。
    fn production_src() -> &'static str {
        const SRC: &str = include_str!("frame.rs");
        // 必须钉到测试模块那一个 `#[cfg(test)]`：文件中间可能还有别的。
        let cut = SRC
            .find("mod tests {")
            .and_then(|i| SRC[..i].rfind("#[cfg(test)]"))
            .unwrap_or(SRC.len());
        &SRC[..cut]
    }

    /// ★★ 任务 5 的回归锁：全屏放大时**开着拖尾就必须继续画覆盖层**。
    ///
    /// 用户口径（2026-10-03）：*"画中画放大时，有鼠标特效。但是全屏放大的时候没有。"*
    ///
    /// 旧代码在 `render()` 里对全屏态无条件 `return`（省掉 1080p 每帧几毫秒的软栅格），
    /// 代价是 trail / key / vk / drawing 一个都不画。几何侧的修法是那个 `return` 改成
    /// "没东西要画才 return"，所以这里钉住两件事：
    /// ① 判据本身对（纯逻辑，见下一条）；
    /// ② `render()` 里那个 `return` **真的被判据挡住了**（结构性）—— 判据写得再对，
    ///    哪天有人"顺手清理"把它换成无条件 `return`，纯逻辑测试照样全绿。
    #[test]
    fn the_fullscreen_early_return_is_guarded_by_the_content_check() {
        let src = production_src();
        // 锚点取"全屏窗已上屏"那一段（`fullscreen_on` 分支），而不是文件里别处的
        // `fullscreen_running()`（帧门、生命周期各有几处）。
        let anchor = "if f.is_shown() {";
        let at = src
            .find(anchor)
            .unwrap_or_else(|| panic!("render 里应还有全屏窗已上屏的分支，锚点 {anchor:?} 不见了"));
        // 该分支往后 400 字符内必须出现判据调用。
        let window = &src[at..(at + 400).min(src.len())];
        assert!(
            window.contains("overlay_has_content_under_fullscreen()"),
            "全屏态那个提前 `return` 必须被 `overlay_has_content_under_fullscreen()` 挡住，\
             否则全屏放大时鼠标特效又没了（用户 2026-10-03 报）"
        );
    }

    /// 全屏态下"覆盖层有没有东西要画"的判据。
    ///
    /// 两头都要钉住，一头都不能松：
    /// - 开着任一视觉功能 → `true`（**这是本次修复本身**：不然特效又被全屏吞掉）；
    /// - 全关 → `false`（**这是性能底线**：全屏态下覆盖层要真出帧，1080p 每帧几毫秒，
    ///   无条件出帧等于把"全屏放大"变成持续满载 —— 那正是旧代码 `return` 想省的）。
    #[test]
    fn fullscreen_keeps_the_overlay_only_when_something_is_drawn_on_it() {
        // ① 全关（`drawing_only()` 把 key_display 关了，但 drawing 是开的）→ 先关掉。
        let mut rt = drawing_only();
        rt.config.drawing.enabled = false;
        assert!(
            !rt.overlay_has_content_under_fullscreen(),
            "什么都不开时不该继续画覆盖层（全屏态下每帧是整屏软栅格）"
        );

        // ② 拖尾一开就必须在画了（★ 本次修复：旧代码这时什么都不画）。
        rt.config.trail.trail_enabled = true;
        assert!(
            rt.overlay_has_content_under_fullscreen(),
            "全屏放大时开着拖尾就必须继续画覆盖层——用户 2026-10-03 报\"全屏放大没有鼠标特效\""
        );
        rt.config.trail.trail_enabled = false;

        // ③ 其余视觉功能逐个：任一为开都要画。
        for (name, on) in [("按键回显", 0), ("虚拟键盘", 1)] {
            let mut r = drawing_only();
            r.config.drawing.enabled = false;
            r.config.key_display.enabled = name == "按键回显";
            r.config.virtual_keyboard.enabled = name == "虚拟键盘";
            assert_eq!(
                r.overlay_has_content_under_fullscreen(),
                on == 1 || name == "按键回显",
                "{name} 开着时覆盖层必须有内容"
            );
        }

        // ④ 标注开着 → 有内容（`drawing_only()` 就是这个状态）。
        let r = drawing_only();
        assert!(
            r.overlay_has_content_under_fullscreen(),
            "标注开着时必须有内容"
        );
    }

    /// 标注模式**空闲时必须报 `Idle`**——这是整个功能的性能底线。
    ///
    /// 覆盖层是整屏 CPU 软渲染，每一帧都是实打实的全屏重绘。这一条退化的表现不是"卡"，
    /// 而是"进过一次标注模式之后 CPU 再也没降下来，而屏幕上什么都没在动"，用户几乎
    /// 不可能把这两件事联系起来。
    #[test]
    fn annotation_idle_demands_no_frames() {
        let mut rt = drawing_only();
        rt.drawing.controller_mut().toggle(DrawMode::Screen, 0.0);

        // 刚进模式：「屏幕标注模式」开始 2 秒淡出 → 必须排帧，且排的是**截止时刻**
        // 而不是"立刻再来一帧"（后者会让淡出期间一直空转全屏绘制）。
        assert_eq!(
            rt.frame_demand(),
            FrameDemand::After(2000),
            "刚进标注应报出浮字淡出的截止时刻"
        );

        // 画一笔。画完之后画面就静止了，浮字也还在淡出。
        let mut c = Point { x: 100.0, y: 100.0 };
        rt.drawing.controller_mut().begin_stroke(c, 0.0);
        for _ in 0..20 {
            c.x += 20.0;
            c.y += 5.0;
            rt.drawing.controller_mut().extend_stroke(c);
        }
        rt.drawing.controller_mut().end_stroke();
        assert_eq!(rt.drawing.controller().history().stroke_count(), 1);

        // 让浮字淡完。之后必须一帧都不排。
        for _ in 0..50 {
            rt.drawing.controller_mut().tick(&dt_tick(50.0));
        }
        assert_eq!(
            rt.frame_demand(),
            FrameDemand::Idle,
            "标注中、浮字已消失、没有任何动画 → 必须 Idle（否则整屏空转）"
        );
    }

    /// 退出标注后那 2 秒「已退出绘图」还要画；淡完立刻停帧。
    ///
    /// 与上一条合起来是同一个闸门的两侧：**漏掉续帧则提示看不见，多续一帧则永久空转**。
    #[test]
    fn exit_toast_keeps_frames_until_it_fades() {
        let mut rt = drawing_only();
        rt.drawing.controller_mut().toggle(DrawMode::Screen, 0.0);
        // 退出走同一个 toggle，0.5s 去抖要跨过（1.0 - 0.0 = 1.0 > 0.5）。
        rt.drawing.controller_mut().toggle(DrawMode::Screen, 1.0);
        // ★ 2026-09-23 起 `toggle` 只**报意图**（`DrawRequest::AnnotateExit`）—— 真正停、
        //   以及"先合成留档、再放掉那张冻结帧"的顺序都在 `Runtime` 这一侧。所以这里要把
        //   请求喂给消费点，跟 `frame()` 里跑的是同一条路（否则测的就不是线上那条链）。
        assert!(
            rt.drawing.controller().is_drawing(),
            "报意图不等于已经停：产物还没合成"
        );
        rt.handle_draw_request();
        assert!(!rt.drawing.controller().is_drawing());
        assert!(
            matches!(rt.frame_demand(), FrameDemand::After(_)),
            "「已退出绘图」还在淡出，不能报 Idle"
        );

        for _ in 0..50 {
            rt.drawing.controller_mut().tick(&dt_tick(50.0));
        }
        assert_eq!(rt.frame_demand(), FrameDemand::Idle);
    }

    /// ★ 光标跟随特效（悬浮标 / 拖尾）的续帧门禁：激活且**光标移动**才续帧（≤刷新率），
    /// 静止立即休眠（0% CPU）。这正是"快速移动卡顿 + CPU 随速度涨"的修复判据。
    #[test]
    fn cursor_follow_tracks_at_vsync_and_sleeps_when_still() {
        let mut rt = Runtime::new(Config::default());
        rt.config.trail.hover_enabled = true; // 悬浮标卡

        // 首帧：last_present 为 None → 必须续帧（启动即跟手）。
        rt.live_cursor = (100.0, 100.0);
        assert!(
            matches!(rt.frame_demand(), FrameDemand::Now),
            "悬浮标激活、首帧应续帧"
        );
        // 光标没动：应休眠（不提交，0% CPU）。
        assert!(
            matches!(rt.frame_demand(), FrameDemand::Idle),
            "光标静止应休眠"
        );
        // 光标移动：应续帧（≤刷新率，由 windui 动画分支的 due=frame_ms 限流）。
        rt.live_cursor = (640.0, 480.0);
        assert!(
            matches!(rt.frame_demand(), FrameDemand::Now),
            "光标移动应续帧"
        );
    }

    /// ★ 悬浮标脚本**自己**要的续帧（`draw` 返回的毫秒数）帧门必须照办。
    ///
    /// 「晃动定位」的定位环 3 秒淡出全靠它：环一出现手就停了，而分层窗帧间**不清缓冲**——
    /// 没有续帧，屏上就永久定格在环刚出现那一帧（用户看到的是"环不消失"）。淡出跑完
    /// 脚本不再返回 → 立刻收敛回 `Idle`，静止零功耗不被破坏。
    #[test]
    fn hover_script_frame_request_keeps_frames_coming() {
        let mut rt = Runtime::new(Config::default());
        rt.config.trail.hover_enabled = true;
        rt.trail.hover_mut().enabled = true;
        rt.trail
            .hover_mut()
            .sync_instances(&["HoverDot/solid".into()]); // 实例承载续帧请求
        rt.live_cursor = (100.0, 100.0);
        // 首帧消费掉"光标移动"这一次性判据，之后才是稳态判据。
        assert!(
            matches!(rt.frame_demand(), FrameDemand::Now),
            "悬浮标激活、首帧应续帧"
        );
        assert_eq!(
            rt.frame_demand(),
            FrameDemand::Idle,
            "光标静止 + 脚本没请求 → 必须休眠"
        );
        // 脚本要 32ms 后再来一帧（定位环的淡出节拍）。
        rt.trail.hover_mut().instances[0].set_next_frame_ms(Some(32));
        assert_eq!(
            rt.frame_demand(),
            FrameDemand::After(32),
            "脚本请求了续帧 → 帧门必须排上"
        );
        // 淡出结束（脚本不再返回）→ 立即收敛。
        rt.trail.hover_mut().instances[0].set_next_frame_ms(None);
        assert_eq!(rt.frame_demand(), FrameDemand::Idle);
    }

    /// ★ 配置被面板改过 → 必须请覆盖层补一帧（用户口径："细分刻度有时不起作用"）。
    ///
    /// 改完那一刻光标多半停在面板上没动，帧门判"光标没移动"返回 `Idle`，而分层窗帧间
    /// 不清缓冲 —— 屏上留着改动前那一帧，直到用户把鼠标移回桌面才突然变过来。
    #[test]
    fn config_change_asks_overlay_for_one_more_frame() {
        let mut rt = Runtime::new(Config::default());
        rt.config.trail.hover_enabled = true;
        rt.overlay_repaint_hint = true;
        assert!(rt.take_overlay_repaint_hint(), "改完配置应请求补一帧");
        assert!(
            !rt.take_overlay_repaint_hint(),
            "请求是边沿触发：取过即无（不会反复踢帧）"
        );
        // 那一帧画完（`frame()` + `render()`，与 `paint()` 里的顺序一致）后要自动作废：
        // 定时器不该为一个已经画过的改动再踢一帧。
        rt.overlay_repaint_hint = true;
        rt.frame();
        rt.render();
        assert!(
            !rt.take_overlay_repaint_hint(),
            "本帧已按新配置画过 → 请求作废"
        );
    }

    /// ★ 滚轮回显门（2026-09-22 恢复）：只在「击键回显开着 **且** 处于『全部』模式」
    /// 时才武装。四种组合全测——只测"开→开"的话，把门写成单看 `enabled` 也绿。
    /// （2026-09-25 起独立「鼠标按键」开关已删，show_mouse 由 display_mode 派生。）
    #[test]
    fn mouse_echo_gate_needs_both_the_feature_and_the_mouse_switch() {
        // 与缩放滚轮门禁争同一批全局静态量，串行（见 `INPUT_GATE_LOCK` 的说明）。
        let _guard = INPUT_GATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let armed = crate::platform::input::mouse::mouse_echo_armed;
        let mut rt = Runtime::new(Config::default());
        rt.sync_input_gate();
        assert!(!armed(), "出厂（功能关）不应武装");

        // ① 功能开 + 「排除鼠标」模式 → 不武装。
        rt.config.key_display.enabled = true;
        rt.config.key_display.display_mode = "no_mouse".into();
        rt.sync_input_gate();
        assert!(!armed(), "排除鼠标模式不应武装");
        // ② 功能开 + 「全部」模式 → 武装。
        rt.config.key_display.display_mode = "all".into();
        rt.sync_input_gate();
        assert!(armed(), "功能开 + 全部模式 = 武装");
        // ③ 功能关（模式仍为 all）→ 不武装。
        rt.config.key_display.enabled = false;
        rt.sync_input_gate();
        assert!(!armed(), "功能关了就应 disarm（哪怕模式仍是全部）");
        // ④ 模式也切到排除鼠标 → 不武装。
        rt.config.key_display.display_mode = "no_mouse".into();
        rt.sync_input_gate();
        assert!(!armed(), "都关时不应武装");
    }

    /// ★ 鼠标低级钩子按需装卸（2026-10-24，用户报「什么功能都没开，鼠标类功能却像在跑」）：
    /// 期望态 = 四路鼠标消费者的并集。只钉「全关 → 不装」会漏掉另一端（门写死 false
    /// 也全绿），所以四路各开一次、再逐路关回去。
    #[test]
    fn mouse_hook_is_wanted_only_while_a_mouse_consumer_is_on() {
        let _guard = INPUT_GATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let wanted = crate::platform::input::hook::mouse_hook_wanted;
        let mut rt = Runtime::new(Config::default());
        rt.sync_input_gate();
        assert!(!wanted(), "出厂全关：不该让系统把每一次鼠标移动都派发进来");

        // ① 击键回显「全部」= 回显鼠标按键/滚轮 → 要装。
        rt.config.key_display.enabled = true;
        rt.config.key_display.display_mode = "all".into();
        rt.sync_input_gate();
        assert!(wanted(), "回显「全部」要吃鼠标 → 要装");
        // ② 改成「排除鼠标」→ 立刻还回去。
        rt.config.key_display.display_mode = "no_mouse".into();
        rt.sync_input_gate();
        assert!(!wanted(), "排除鼠标模式不该留着钩子");
        rt.config.key_display.enabled = false;

        // ③ 放大镜在屏（只有它在屏时滚轮缩放才武装）→ 要装。
        rt.config.magnifier_pip.enabled = true;
        rt.config.magnifier_pip.active = true;
        rt.sync_input_gate();
        assert!(wanted(), "放大镜在屏 → 滚轮缩放要吃鼠标");
        rt.config.magnifier_pip.active = false;
        rt.config.magnifier_pip.enabled = false;
        rt.sync_input_gate();
        assert!(!wanted(), "放大镜关掉 → 还回去");

        // ④ 虚拟键盘方案含鼠标键帽 → 要装（与「鼠标穿透」无关）。
        rt.config.virtual_keyboard.enabled = true;
        rt.config.virtual_keyboard.layout = "wasd_gaming".to_string();
        rt.frame(); // 让 vk 解析方案、刷新 wants_mouse
        rt.sync_input_gate();
        assert!(wanted(), "WASD 板要鼠标键帽高亮 → 要装");
        rt.config.virtual_keyboard.enabled = false;
        rt.frame();
        rt.sync_input_gate();
        assert!(!wanted(), "虚拟键盘关掉 → 还回去");
    }

    /// ★★ 虚拟键盘的鼠标门：**只看方案里有没有鼠标键帽，不看「鼠标穿透」**
    /// （2026-09-26 修，用户报"WASD 板的左键/右键点了没反应"）。
    ///
    /// 旧判据把两件事焊在一起：`mouse_input_needed()` 只在 `enabled && !click_through`
    /// 时为虚拟键盘开门，于是**勾着穿透的方案**（WASD 板出厂即勾）鼠标事件被桥接线程
    /// 整条丢弃，鼠标键帽永远收不到 `MouseDown`。而"穿透"只决定点键帽注不注入按键，
    /// 与"键帽要不要亮"毫无关系 —— 这里把这条界线钉死。
    #[test]
    fn vk_mouse_gate_ignores_the_passthrough_flag() {
        let _guard = INPUT_GATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        // ① 含鼠标键帽的方案：**即便勾着穿透**也要开门（穿透态才正是要亮键帽的场景）。
        let mut cfg = Config::default();
        cfg.virtual_keyboard.enabled = true;
        cfg.virtual_keyboard.layout = "wasd_gaming".to_string();
        cfg.virtual_keyboard.click_through = true;
        let mut rt = Runtime::new(cfg);
        rt.frame(); // 让 vk 解析方案、刷新 wants_mouse
        rt.sync_input_gate();
        assert!(
            VK_MOUSE_ARMED.load(Ordering::Relaxed),
            "WASD 板含鼠标键帽 ⇒ 必须收鼠标事件（穿透也不例外）"
        );
        // 同一套配置关掉穿透也要开门（两条路都要亮）。
        rt.config.virtual_keyboard.click_through = false;
        rt.sync_input_gate();
        assert!(VK_MOUSE_ARMED.load(Ordering::Relaxed));

        // ② 纯键盘方案：一个鼠标事件都不用付代价。
        rt.config.virtual_keyboard.layout = "ansi_104".to_string();
        rt.frame();
        rt.sync_input_gate();
        assert!(
            !VK_MOUSE_ARMED.load(Ordering::Relaxed),
            "ANSI 104 没有鼠标键帽，不该为它开门"
        );

        // ③ 功能关掉 → 关门。
        rt.config.virtual_keyboard.layout = "wasd_gaming".to_string();
        rt.frame();
        rt.config.virtual_keyboard.enabled = false;
        rt.sync_input_gate();
        assert!(
            !VK_MOUSE_ARMED.load(Ordering::Relaxed),
            "功能关了就该 disarm"
        );
    }

    #[test]
    fn composite_scripts_keep_overlay_awake() {
        // ★ 只开复合脚本（其它功能全关）时覆盖层同样不能 park：park 后 frame() 不跑、
        //   `trail.tick()` 不消费采样、`draw` 不执行 →「勾了没反应」（2026-09-26 用户报）。
        //   另一半在 `mouse_input_needed()`：门为 false 时桥接线程把 MouseMove 整条丢弃，
        //   SampleQueue 收不到点——两条门都要认复合脚本（经 `trail_module_active`）。
        let mut rt = Runtime::new(Config::default());
        rt.config.lua_ext.composite_active = vec!["scripts/composite_demo".to_string()];
        assert!(rt.input_needs_frame(), "复合脚本运行中覆盖层不该休眠");
        assert!(
            rt.mouse_input_needed(),
            "复合脚本要吃鼠标采样（拖尾/点击反应）"
        );
    }

    #[test]
    fn pip_magnifier_keeps_overlay_awake() {
        let mut rt = Runtime::new(Config::default());
        rt.config.magnifier_pip.enabled = true;
        rt.config.magnifier_pip.active = true; // 两段语义：载入之外还要"激活"才在屏
                                               // 帧门：只开放大镜时覆盖层也不能休眠（park），否则 paint 不再触发、画面永不上屏。
        assert!(rt.input_needs_frame(), "放大镜开启时覆盖层不该休眠");

        // 刚开启、光标停在上一次呈现帧的位置（默认 (0,0)）→ 也必须出帧，
        // 否则首帧被 "live==last_presented" 判成 Idle，面板永不出现。
        rt.live_cursor = (0.0, 0.0);
        assert!(
            matches!(rt.frame_demand(), FrameDemand::Now),
            "放大镜刚开启应强制出首帧"
        );
        // 真正出一帧（抓一次画面）→ has_frame 置位。此后光标没动也不再是 Idle：
        // ★ 屏幕内容自己会变（点击冒出的弹出菜单、进度条、视频），静止期必须按
        //   `LOOK_INTERVAL_MS` 定时唤醒重抓，否则"点出菜单面板不刷新、动一动才刷新"
        //   （2026-09-21 用户报告）。放大镜开着时的"休眠"是 30ms 的节拍出帧，
        //   不是 Idle 睡死。
        rt.render();
        assert_eq!(
            rt.frame_demand(),
            FrameDemand::After(crate::modules::magnifier::LOOK_INTERVAL_MS),
            "已出过帧且光标静止应进入定时重抓节拍（不许 Idle 睡死）"
        );
        // 光标移动 → 续帧（跟随鼠标刷新放大画面）。
        rt.live_cursor = (300.0, 200.0);
        assert!(
            matches!(rt.frame_demand(), FrameDemand::Now),
            "放大镜跟随光标应续帧"
        );
    }

    /// ★ 回归（2026-09-18）：**没有输入**时虚拟键盘的高亮也必须自己消失。
    ///
    /// `vk.tick` 此前只写在 `frame()` 的输入队列循环里，而 `sweep` 是按**墙钟**清过期
    /// 高亮的 —— 没输入就永远不清；`next_change_ms` 报出的那个"到点"唤醒帧同样不推进，
    /// 于是收敛到 `Idle` 之后屏上键帽**永久亮着**（看着像键盘卡死了）。
    /// `hold = 0`（松开即灭）把等待压到"下一帧"，这条用例才不必真等半秒墙钟。
    #[test]
    fn vk_glow_clears_without_input() {
        let mut cfg = Config::default();
        cfg.virtual_keyboard.enabled = true;
        cfg.virtual_keyboard.hold = 0.0;
        let mut rt = Runtime::new(cfg);

        rt.feed(InputEvent::KeyDown {
            key: KeyCode::Char('a'),
            mods: Modifiers::default(),
            repeated: false,
            time_ms: 0,
        });
        // ★ 必须补一条抬起：`sweep` 只清**没被按住**的键（`held` 里的永不淡出），
        //   只喂 KeyDown 量到的是"按住"语义，与被测的那件事无关。
        rt.feed(InputEvent::KeyUp {
            key: KeyCode::Char('a'),
            mods: Modifiers::default(),
            time_ms: 1,
        });
        rt.frame();
        assert!(rt.vk.has_glow(), "按下再松开之后，键帽应还在保持窗口内亮着");

        // 之后**一帧输入都不喂**：只有 `frame()` 自己在推进。
        std::thread::sleep(std::time::Duration::from_millis(3));
        rt.frame();
        assert!(
            !rt.vk.has_glow(),
            "没有输入时高亮也必须被 sweep 清掉（此前会永久残留）"
        );
    }

    /// ★ 回归（2026-09-18）：**没有输入**时提示浮字也必须继续淡出。
    ///
    /// 与上一条同源：`drawing.tick` 此前只在输入队列循环里被调用，浮字的剩余时间在
    /// 无输入时**永不递减** ——「屏幕标注模式」/「已退出绘图」会一直挂在屏幕上，
    /// 同时还每 2 秒空转一帧。判据取"剩余时间变短"而不是"等它消失"：后者要真睡 2 秒，
    /// 而"推进了没有"这件事一小段睡眠就够分辨（旧代码下它**一点都没减**）。
    #[test]
    fn drawing_toast_advances_without_input() {
        let mut rt = drawing_only();
        rt.drawing.controller_mut().toggle(DrawMode::Screen, 0.0);
        let before = rt.drawing.next_change_ms().expect("刚进标注应有浮字在淡出");

        // 不喂任何输入，只让时间流逝一小段再出一帧。
        std::thread::sleep(std::time::Duration::from_millis(80));
        rt.frame();

        let after = rt.drawing.next_change_ms().expect("浮字还在淡出");
        assert!(
            after < before,
            "空帧也应推进浮字（此前完全不推进）：{before} → {after}"
        );
    }

    /// 什么都没做时不该排帧——这是"覆盖层能休眠、私有内存能掉下来"的前提。
    #[test]
    fn nothing_happening_demands_no_frames() {
        assert_eq!(drawing_only().frame_demand(), FrameDemand::Idle);
    }
}
