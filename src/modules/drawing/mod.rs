//! 屏幕书写模块。
//!
//! # 能力
//!
//! 三支画笔、七种工具（自由笔/直线/箭头/矩形/椭圆/文字/橡皮）、序号贴纸、撤销/重做/清空、
//! 白板模式、屏幕底部的**工具面板**（可点选，不必记快捷键）、以及"进入/退出标注模式"的
//! 鼠标捕获切换。
//!
//! # 快捷键
//!
//! 11 个快捷键**全部从配置读**（`config.drawing.hotkeys`），匹配走 `core::hotkeys` 的归一化比较。
//! 之所以不在模块里另开一张表：Python 1.0.0 的 `_on_key_for_drawing` 也是每次从
//! `config.data["drawing"]["hotkeys"]` 现读，用户改完快捷键立刻生效、无需重启。
//!
//! ★ 开关门控（2.0）：全部书写快捷键（含开启/退出标注/白板的 toggle）只在「画笔」
//!   卡片的开关（`drawing.enabled`）勾选后生效；未勾选时按下无任何反应。唯一例外是
//!   Esc 退出标注——正在标注时收画面必须始终可行，否则开关关掉后墨迹收不走。
//!
//! 快捷键与工具面板**并存**：面板是给不记快捷键的人用的入口，快捷键是给熟手用的快捷方式，
//! 两条路改的是同一份控制器状态。

pub mod controller;
pub mod dash;
pub mod eraser;
pub mod history;
pub mod notify;
pub mod pens;
pub mod settings;
pub mod shapes;
pub mod stroke;
pub mod toolbar;

use crate::core::config::Config;
use crate::core::draw::DrawList;
use crate::core::events::{InputEvent, KeyCode, Modifiers};
use crate::core::hotkeys::{match_drawing, DrawingAction};
use crate::core::tick::Tick;
use crate::modules::Module;
use crate::platform::display::ScreenRect;

use controller::{DrawMode, DrawRequest, DrawingController, Tool};

/// 书写模块。
pub struct DrawingModule {
    controller: DrawingController,
    /// 虚拟桌面矩形（物理像素）。由 Runtime 每帧注入（P3.1 坐标对齐）。
    screen: ScreenRect,
    /// 输入挂起。控制面板 / 文字输入窗任一打开期间为真。
    ///
    /// **键鼠一起挂**，不是一个只管快捷键的开关：Python 1.0.0 用 `_capture_in_progress()`
    /// 挡的是整个标注输入。只挂快捷键会留下一个很别扭的 bug——面板开着时点一下（比如点
    /// 「关于」页），覆盖层上同时落下一笔，因为那一下鼠标在覆盖层看来和"画一笔"没区别。
    /// 挂起期间彻底不消费输入，用户就是在调设置，不是在画。
    ///
    /// ★ 挂起来源分两个字段记（见 [`Self::set_suspension`]）：Esc 豁免（修复
    ///   「面板开着时按 Esc 退不出标注」）需要区分——面板挂着时 Esc 仍要能退出
    ///   标注（验收口径：标注中任意时刻 Esc 可退）；文字输入窗挂着时 Esc 归
    ///   输入窗（windui 用它关窗取消本次文字），不得连标注一起退。
    panel_open: bool,
    /// 标注文字输入窗是否开着（与 `panel_open` 分开，见上）。
    text_window_open: bool,
    /// 上次同步进控制器的设置快照：`tick` 里用它检测配置变化（见 [`Self::sync_settings`]）。
    synced: settings::DrawingSettings,
}

impl DrawingModule {
    pub fn new(config: &Config) -> Self {
        let settings = settings::from_config(&config.drawing);
        Self {
            // 快照先克隆：controller 构造会 move settings。
            synced: settings.clone(),
            controller: DrawingController::new(settings),
            screen: ScreenRect::FALLBACK,
            panel_open: false,
            text_window_open: false,
        }
    }

    /// 注入当前虚拟桌面矩形（由 Runtime 每帧调用）。替换硬编码 1920×1080。
    pub fn set_screen(&mut self, screen: ScreenRect) {
        self.screen = screen;
        self.controller.set_screen(screen);
    }

    /// 注入系统 DPI（工具面板按逻辑像素设计，绘制时要换算成世界单位）。
    pub fn set_ui_scale(&mut self, scale: f32) {
        self.controller.set_ui_scale(scale);
    }

    /// ★ 注入截图编辑态的选区（每帧由 `Runtime` 推一次；`None` = 不在编辑态）。
    ///
    /// 见 [`DrawingController::set_region`]：选区的主人是 `Runtime` 的编辑会话，
    /// 这里只是一份每帧刷新的镜像——压暗范围与工具栏落点都读它。
    pub fn set_region(&mut self, region: Option<crate::core::geometry::Rect>) {
        self.controller.set_region(region);
    }

    /// ★ 以**冻结帧为底图**进入截图编辑态（`Ctrl` + 截图快捷键框完那一刻）。
    ///
    /// 与「进入屏幕标注」是同一件事的两个入口：都是"给一块底图 + 一个选区，然后把画笔交给
    /// 用户"，区别只在底图从哪来（标注 = 现抓 / 白板 = 纯白 / 这里 = 框选那一下冻住的那一帧）。
    /// 底图类型是 `core::backdrop::Backdrop`，模块只搬运不解释——`runtime` 负责选哪一份。
    pub fn start_with_backdrop(
        &mut self,
        backdrop: crate::core::backdrop::Backdrop,
        region: crate::core::geometry::Rect,
    ) {
        self.controller.start_with_backdrop(backdrop, region);
    }

    /// 取走一个**跨模块动作**的意图（边沿触发）：工具栏按钮 / `Esc` 按下的那一刻置位，
    /// 见 [`DrawRequest`]。
    ///
    /// ★ `Enter` **不在其列**：编辑态下它对整次截图没有任何作用（用户 2026-09-23 两轮口径）。
    /// `ShotEditConfirm` 的来源是工具栏那颗「保存」；`ShotEditDone` 是那颗打勾「完成」
    /// （★ 2026-10-03，产物相同但不弹另存为框）。
    ///
    /// 与 [`Self::take_text_request`] 同一款：模块只报意图，跨模块的收尾（合成产物、落剪贴板、
    /// 弹原生「另存为」、释放冻结帧）由 `Runtime` 做 —— `modules` 不得依赖 `runtime`（分层纪律）。
    pub fn take_request(&mut self) -> Option<DrawRequest> {
        self.controller.take_request()
    }

    /// 这一趟到底画过东西没有（`Runtime` 拿它决定退出时要不要留档）。
    pub fn has_ink(&self) -> bool {
        self.controller.has_ink()
    }

    /// 出图前要不要**现抓一张桌面当背景**（透明幕布下屏上没有底图）——
    /// 见 [`DrawingController::needs_live_backdrop`]。
    pub fn needs_live_backdrop(&self) -> bool {
        self.controller.needs_live_backdrop()
    }

    /// 换掉底图（出图前把现抓的那张桌面塞进去）。
    pub fn set_backdrop(&mut self, backdrop: crate::core::backdrop::Backdrop) {
        self.controller.set_backdrop(backdrop);
    }

    /// 导出画面的全部内容（**底图 + 墨迹**）——`Runtime` 的离屏合成只调它。
    ///
    /// 与屏上那份（`controller.draw`）的差别见 [`DrawingController::compose_into`]：
    /// 不含压暗、不含工具栏、不含光标处的装饰。
    pub fn compose_into(&self, list: &mut DrawList) {
        self.controller.compose_into(list);
    }

    /// 退出绘图（**带**「已退出绘图」浮字）。
    pub fn stop(&mut self) {
        self.controller.stop();
    }

    /// 退出绘图且**不报**浮字：中央那句留给"已保存到 …"（两条浮字会叠成一团）。
    pub fn stop_quiet(&mut self) {
        self.controller.stop_quiet();
    }

    /// 这一落点是否在工具栏按钮上（截图编辑态据此让"拖选区"给按钮让路，
    /// 见 [`DrawingController::toolbar_hit`]）。
    pub fn toolbar_hit(&self, p: crate::core::geometry::Point) -> bool {
        self.controller.toolbar_hit(p)
    }

    /// 挂起/恢复标注输入，并记录挂起来源（面板 or 文字输入窗）。
    ///
    /// Esc 豁免（见 `panel_open` 字段说明）需要区分来源：面板挂着时 Esc 仍可退出
    /// 标注；文字窗挂着时 Esc 归输入窗。
    pub fn set_suspension(&mut self, panel_open: bool, text_window_open: bool) {
        self.panel_open = panel_open;
        self.text_window_open = text_window_open;
    }

    /// 输入是否挂起（面板或文字输入窗任一开着）。
    fn suspended(&self) -> bool {
        self.panel_open || self.text_window_open
    }

    /// 把配置里的画笔同步进控制器（★ 修复：画笔改了不生效）。
    ///
    /// 控制器构造时拷了一份 [`settings::DrawingSettings`]，落笔的宽/色/线型读的是这份
    /// 快照——面板改配置后不刷新，下一次落笔仍用旧参数。在 [`Self::tick`] 开头调本方法：
    /// 变化才重建（快照比对短路），空转帧零开销；且先于本帧输入处理，改完当帧落笔就是
    /// 新参数（验收：当前画布上的下一次落笔即按新参数渲染）。
    pub fn sync_settings(&mut self, config: &Config) {
        let s = settings::from_config(&config.drawing);
        if s != self.synced {
            self.synced = s.clone();
            self.controller.set_settings(s);
        }
    }

    pub fn controller(&self) -> &DrawingController {
        &self.controller
    }

    pub fn controller_mut(&mut self) -> &mut DrawingController {
        &mut self.controller
    }

    /// 是否正等着用户输入标注文字（UI 层据此开输入窗）。
    pub fn has_pending_text(&self) -> bool {
        self.controller.has_pending_text()
    }

    /// 悬停点是否落在可拖动/可双击的文字上（覆盖层据此换光标）。
    pub fn text_hover_at(&self, p: crate::core::geometry::Point) -> bool {
        self.controller.text_hover_at(p)
    }

    /// 距离画面下一次变化还有多少毫秒（提示浮字淡出）；`None` = 静止。
    pub fn next_change_ms(&self) -> Option<u32> {
        self.controller.next_change_ms()
    }

    /// 取一次"该开文字输入窗了"的请求（边沿触发，取过就不会再给）。
    pub fn take_text_request(&mut self) -> Option<crate::core::geometry::Point> {
        self.controller.take_text_request()
    }

    /// 取一次"该开文字**再编辑**窗了"的请求（边沿触发，UI 层据此预填旧文字）。
    pub fn take_text_edit_request(&mut self) -> Option<controller::TextEditRequest> {
        self.controller.take_text_edit_request()
    }

    /// 保存对已有文字条目的再编辑（空串 = 取消）。
    pub fn commit_text_edit(&mut self, idx: usize, text: &str) -> bool {
        self.controller.commit_text_edit(idx, text)
    }

    /// 放弃这次再编辑（原文不动）。
    pub fn cancel_text_edit(&mut self) {
        self.controller.cancel_text_edit()
    }

    /// 处理一次按键。顺序：Esc 退出（唯一豁免）→ 开关门控 → 模式开关 → 挂起守卫 →
    /// 固定键 → 工具快捷键。
    fn handle_key(&mut self, key: KeyCode, mods: Modifiers, now_s: f64, config: &Config) {
        // ⓪' ★ 截图**编辑态**的两个出口，必须排在下面"Esc 退出标注"**之前**。
        //
        //   编辑态的"退出"不是 `stop()` 就完了 —— 产物合成、剪贴板、释放冻结帧全在
        //   `Runtime` 手里（见 [`DrawRequest`]）。若让下面那条 Esc 先行，控制器会被就地
        //   `stop()`，而 `Runtime` 的编辑会话还活着：屏上既没有底图也没有工具栏（控制器已
        //   Off），覆盖层却仍按"编辑态"占着鼠标（`on_nchit` 全屏回 Client）—— 一个看起来
        //   "软件卡住了"的死状态。所以这里只**报意图**。
        if self.controller.mode().is_shot_edit() && !mods.ctrl && !mods.alt {
            match key {
                // ★★ `Enter` 在编辑态**彻底不认**（2026-09-23 第二次实测，用户口径）：
                //   *"在截图选区时按下 Ctrl，进入缩放与编辑界面，结果按下 enter 键却依然会
                //   退出状态。这完全是错的。我已经强调过，按下 enter 键不应该有反应。
                //   就如同我们在屏幕标注时，按下 enter 键也没有反应，这才是对的。"*
                //
                //   上一版把它做成"没有文字窗时才确认"—— 那还是**有反应**，用户照着
                //   标注态的口径比对，仍然是错的。所以这里不再给它任何分支：敲 Enter 连
                //   一个 `DrawRequest` 都不报。完成整次截图只走工具栏那颗「保存（另存为）」，
                //   与 `Esc`（放弃）一起构成编辑态仅有的两个出口。
                //
                //   代价写明：**编辑态没有键盘确认键**（这是用户明确要的）。要给回来时，
                //   必须连带做成"插文字时不认"——`text_window_open` 那条守卫本来就为它写的。
                KeyCode::Esc if !self.text_window_open => {
                    self.controller.request(DrawRequest::ShotEditCancel);
                    return;
                }
                _ => {}
            }
        }

        // ⓪ Esc 退出标注：唯一不受「画笔」开关限制的键——只要正在标注/白板，
        //    即使开关刚被关掉，Esc 也得能把画面收掉，不能把墨迹留在屏上。
        //    文字输入窗开着时例外：Esc 归输入窗（关窗取消本次文字），不退标注。
        //
        //    ★ 2026-09-23 起退出**顺便留档**（用户口径：*"平常屏幕标注后退出，就保存在
        //    默认位置"*）→ 走 `Runtime` 那条路（合成需要在 runtime 侧的光栅器）。
        if matches!(key, KeyCode::Esc)
            && !mods.ctrl
            && !mods.alt
            && !self.text_window_open
            && self.controller.is_drawing()
        {
            self.controller.request(DrawRequest::AnnotateExit);
            return;
        }

        // ① 开关门控（★ 修复：勾选了以后才允许使用快捷键）：「画笔」未勾选时，
        //    全部书写快捷键——包括开启/退出屏幕标注、白板的 toggle——一律无效；
        //    勾选后立即恢复（快捷键匹配每次现读 config，无需重启）。
        //    总开关关着时覆盖层本来不出帧，由 `runtime::overlay` 的唤醒逻辑按同一
        //    条件（勾选才唤醒）踢帧，本路径才会被触发。
        if !config.drawing.enabled {
            return;
        }

        // ② 模式开关（开启/退出屏幕标注、白板）。勾选后**挂起时也生效**：面板/文字
        //    输入窗开着也要能叫起（或退出）功能——用户预期"按 Alt+G 就开屏幕标注"，
        //    不能要求先关面板。
        //
        //    "快捷键可进可退"这一条归 `ExitPolicy::ANNOTATE.hotkey_toggle` 管
        //    （`core::exit_policy`）。那段注释还钉住了画板**刻意没有**的两条退出手势
        //    （双击 / 右键），要加之前先读它。
        if crate::core::exit_policy::ExitPolicy::ANNOTATE.hotkey_toggle {
            // ★ 截图编辑画布中，「开启/退出标注」的 toggle **一律无效**。
            //
            //   编辑会话的进出口在 `Runtime` 手里（工具栏「保存」→ 另存为 / `Esc` 放弃），
            //   而 `toggle` 在非 Off 模式下走的是 `stop()` —— 那会留下"选区还在屏上、
            //   却已经画不动了"的半死状态（工具栏也消失，用户只剩工具栏以外的键，
            //   而他刚按的是 Alt+G，完全不知道发生了什么）。宁可这个键什么都不做。
            if self.controller.mode().is_shot_edit() {
                return;
            }
            match match_drawing(key, mods, &config.drawing.hotkeys) {
                Some(DrawingAction::ToggleScreen) => {
                    self.controller.toggle(DrawMode::Screen, now_s);
                    return;
                }
                Some(DrawingAction::ToggleWhiteboard) => {
                    self.controller.toggle(DrawMode::Whiteboard, now_s);
                    return;
                }
                _ => {}
            }
        }

        // ③ 其余键（固定键、工具快捷键）：挂起时不处理。挂起 = 面板/文字窗开着，
        // 用户在调设置或打字，不该顺手画出一笔或切走工具。
        if self.suspended() {
            return;
        }

        if !self.controller.is_drawing() {
            return;
        }

        // ② 固定键（不受配置影响）
        if mods.ctrl && !mods.alt {
            match key {
                KeyCode::Char('z') | KeyCode::Char('Z') => {
                    if mods.shift {
                        self.controller.redo();
                    } else {
                        self.controller.undo();
                    }
                    return;
                }
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.controller.redo();
                    return;
                }
                _ => {}
            }
        }
        match key {
            KeyCode::Backspace => {
                self.controller.undo();
                return;
            }
            KeyCode::Delete => {
                self.controller.clear();
                return;
            }
            _ => {}
        }

        // ③ 工具快捷键
        match match_drawing(key, mods, &config.drawing.hotkeys) {
            Some(DrawingAction::Pen1) => {
                self.controller.set_active_pen(0);
                self.controller.set_tool(Tool::Pen);
            }
            Some(DrawingAction::Pen2) => {
                self.controller.set_active_pen(1);
                self.controller.set_tool(Tool::Pen);
            }
            Some(DrawingAction::Pen3) => {
                self.controller.set_active_pen(2);
                self.controller.set_tool(Tool::Pen);
            }
            Some(DrawingAction::Eraser) => self.controller.set_tool(Tool::Eraser),
            Some(DrawingAction::Line) => self.controller.set_tool(Tool::Line),
            Some(DrawingAction::Arrow) => self.controller.set_tool(Tool::Arrow),
            Some(DrawingAction::Rect) => self.controller.set_tool(Tool::Rect),
            Some(DrawingAction::Ellipse) => self.controller.set_tool(Tool::Ellipse),
            Some(DrawingAction::Text) => self.controller.set_tool(Tool::Text),
            Some(DrawingAction::Mosaic) => self.controller.set_tool(Tool::Mosaic),
            // 序号贴纸：切过去后点一下盖一个带编号的圈（2026-10-01 可配键位前它只有
            // 工具栏图标一个入口，而"写步骤编号"是标注里最常做的事之一）。
            Some(DrawingAction::Stamp) => self.controller.set_tool(Tool::Stamp),
            _ => {}
        }
    }
}

impl Module for DrawingModule {
    fn enabled(&self, config: &Config) -> bool {
        // ★ 开关架构：唯一开关是本功能段自己的 `enabled`（功能总开关已废除）。
        config.drawing.enabled
    }

    /// 关掉 "屏幕书写" 就把撤销栈与冻结帧还回去。
    ///
    /// 这两样是本模块最占内存的：`history` 每落一笔攒一串点，**冻结帧**是整张虚拟桌面
    /// 的 RGBA 副本（1080p ≈ 8 MB / 4K ≈ 33 MB）。用户把开关拨到关、或者在书写模式里
    /// 直接关掉功能，都该让这份内存消失——否则"关了却没释放"会让人怀疑程序在泄漏。
    /// 详见 [`DrawingController::release_memory`]。
    fn release_memory(&mut self) {
        self.controller.release_memory();
    }

    fn tick(&mut self, tick: &Tick, config: &Config, input: &Option<InputEvent>) {
        // ★ 画笔同步进控制器（修复：改了粗细/颜色不生效）。变化才重建，
        //   且先于本帧输入处理——改完当帧落笔就是新参数。
        self.sync_settings(config);
        // 挂起期间 **鼠标与绝大多数键都不消费**（见 `input_suspended` 的说明）：面板/输入窗
        // 开着时，用户是在调设置或打字，不是在画。但"开启/退出标注"的 toggle 快捷键与
        // "Esc 退出标注"是例外：开关勾选后（门控在 `handle_key` ①）挂起也要响应——
        // 否则面板开着（应用默认就开着面板）按 Alt+G 永远没反应。故挂起分支仍把
        // KeyDown 交给 `handle_key`；普通键被挂起守卫拦下。提示浮字照样推进。
        if self.suspended() {
            // Esc 豁免、toggle 放行都在 `handle_key` 里（Esc 退出在最前、toggle 在开关
            // 门控之后、挂起守卫之前）。面板快捷键捕获态下 Esc 早已被
            // Runtime::try_capture_hotkey 吃掉，到不了这里，不会打架。
            self.controller.tick(tick);
            if let Some(InputEvent::KeyDown {
                key,
                mods,
                repeated,
                ..
            }) = input
            {
                if !*repeated {
                    self.handle_key(*key, *mods, tick.elapsed_ms / 1000.0, config);
                }
            }
            return;
        }

        if let Some(event) = input {
            match event {
                InputEvent::KeyDown {
                    key,
                    mods,
                    repeated,
                    ..
                } => {
                    // 长按自动重复不重复触发：否则按住 Alt+G 会不停地进/出书写模式。
                    if !*repeated {
                        self.handle_key(*key, *mods, tick.elapsed_ms / 1000.0, config);
                    }
                }
                InputEvent::MouseDown { x, y, .. } => {
                    if self.controller.is_drawing() {
                        self.controller.begin_stroke(
                            crate::core::geometry::Point {
                                x: *x as f32,
                                y: *y as f32,
                            },
                            tick.elapsed_ms / 1000.0,
                        );
                    }
                }
                InputEvent::MouseMove { x, y, .. } => {
                    if self.controller.is_drawing() {
                        self.controller.extend_stroke(crate::core::geometry::Point {
                            x: *x as f32,
                            y: *y as f32,
                        });
                    }
                }
                InputEvent::MouseUp { .. } if self.controller.is_drawing() => {
                    self.controller.end_stroke();
                }
                _ => {}
            }
        }

        self.controller.tick(tick);
    }

    fn draw(&self, draw: &mut DrawList, _config: &Config) {
        // 条件里带上 `next_change_ms`：退出标注后「已退出绘图」还在淡出，
        // 那 2 秒里必须有内容交给渲染层，否则这句提示永远看不见。
        if self.controller.is_drawing() || self.controller.next_change_ms().is_some() {
            // 虚拟桌面矩形与 DPI 都在控制器里，这里不再传尺寸——按 (0,0) 起算在
            // 副屏位于主屏左侧时会整体错位（世界坐标原点是虚拟桌面原点）。
            self.controller.draw(draw);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tick16() -> Tick {
        Tick {
            raw_ms: 16.0,
            smooth_ms: 16.0,
            dt_ms: 16.0,
            elapsed_ms: 16.0,
        }
    }

    fn key_down(key: KeyCode, mods: Modifiers) -> Option<InputEvent> {
        Some(InputEvent::KeyDown {
            key,
            mods,
            repeated: false,
            time_ms: 0,
        })
    }

    fn none() -> Modifiers {
        Modifiers {
            ctrl: false,
            alt: false,
            shift: false,
            win: false,
        }
    }

    /// 开启书写卡片开关（否则所有书写快捷键都被守卫拦下）。
    ///
    /// ★ 这两把**入口键必须显式配值**（2026-09-27 出厂口径改了）：它们出厂现在留空，
    ///   而下面这些用例考的就是"按 Alt+G 发生什么"，靠默认值的老写法会退化成"测了个
    ///   寂寞"（快捷键根本没配）。画笔 1..9 那些内部切换键出厂就有值，无需补。
    fn enabled_config() -> Config {
        let mut cfg = Config::default();
        cfg.drawing.enabled = true;
        cfg.drawing.hotkeys.draw_hk_screen = "Alt+G".into();
        cfg.drawing.hotkeys.draw_hk_white = "Alt+W".into();
        cfg
    }

    #[test]
    fn drawing_module_creation() {
        let config = Config::default();
        let module = DrawingModule::new(&config);
        assert!(!module.controller().is_drawing());
    }

    #[test]
    fn drawing_module_disabled_by_default() {
        let config = Config::default();
        let module = DrawingModule::new(&config);
        assert!(!module.enabled(&config));
    }

    #[test]
    fn screen_hotkey_enters_drawing() {
        let cfg = enabled_config();
        let mut m = DrawingModule::new(&cfg);
        m.tick(
            &tick16(),
            &cfg,
            &key_down(
                KeyCode::Char('g'),
                Modifiers {
                    alt: true,
                    ..none()
                },
            ),
        );
        assert!(m.controller().is_drawing());
        assert_eq!(m.controller().mode(), DrawMode::Screen);
    }

    #[test]
    fn screen_hotkey_ignored_when_switch_off() {
        // ★ 画笔未勾选 → 书写快捷键无效（用户需求：勾选了以后才允许使用快捷键）。
        //   此前语义是"开关关着快捷键也能叫起标注"，与需求相反，已翻转。
        let cfg = Config::default(); // enabled_draw = false
        let mut m = DrawingModule::new(&cfg);
        m.tick(
            &tick16(),
            &cfg,
            &key_down(
                KeyCode::Char('g'),
                Modifiers {
                    alt: true,
                    ..none()
                },
            ),
        );
        assert!(
            !m.controller().is_drawing(),
            "开关关着时 Alt+G 不应进入书写模式"
        );
    }

    #[test]
    fn esc_exits_annotation_even_when_switch_off() {
        // Esc 退出标注不受开关限制：标注中把开关关掉（面板里现关），画面必须收得掉。
        let mut cfg = enabled_config();
        let mut m = DrawingModule::new(&cfg);
        m.tick(
            &tick16(),
            &cfg,
            &key_down(
                KeyCode::Char('g'),
                Modifiers {
                    alt: true,
                    ..none()
                },
            ),
        );
        assert!(m.controller().is_drawing());
        cfg.drawing.enabled = false; // 模拟面板里现关开关
        m.tick(&tick16(), &cfg, &key_down(KeyCode::Esc, none()));
        // ★ 退出改由 `Runtime` 执行（先合成留档、再放掉那张整屏冻结帧，见 `DrawRequest`），
        //   模块这一侧只负责报出意图 —— 所以判据是"报了 `AnnotateExit`"。真停那一半由
        //   `runtime::tests::exit_toast_keeps_frames_until_it_fades` 覆盖。
        assert_eq!(
            m.take_request(),
            Some(DrawRequest::AnnotateExit),
            "Esc 应报出退出标注的意图（不受开关限制）"
        );
    }

    #[test]
    fn esc_does_not_exit_when_text_window_open() {
        // 文字输入窗开着时 Esc 归输入窗（取消本次文字），不退标注。
        let cfg = enabled_config();
        let mut m = DrawingModule::new(&cfg);
        m.tick(
            &tick16(),
            &cfg,
            &key_down(
                KeyCode::Char('g'),
                Modifiers {
                    alt: true,
                    ..none()
                },
            ),
        );
        assert!(m.controller().is_drawing());
        m.set_suspension(false, true); // 仅文字窗开着
        m.tick(&tick16(), &cfg, &key_down(KeyCode::Esc, none()));
        assert!(m.controller().is_drawing(), "文字窗开着时 Esc 不应退出标注");
    }

    #[test]
    fn pen_settings_flow_into_controller() {
        // ★ 修复：面板改画笔粗细/颜色后，控制器下一次落笔要用新参数。
        //   此前控制器持有构造时的快照、配置改了也不刷新。
        let cfg = enabled_config();
        let mut m = DrawingModule::new(&cfg);
        let old_width = m.controller().active_pen().width;
        assert_ne!(old_width, 60, "出厂默认不应用 60，否则本测试无意义");

        let mut cfg2 = cfg.clone();
        cfg2.drawing.pens[0].width = 60;
        m.tick(&tick16(), &cfg2, &None);
        assert_eq!(
            m.controller().active_pen().width,
            60,
            "tick 消费新配置后控制器应用新粗细"
        );

        // 配置不变时保持稳定（快照相等短路，行为不可观测，锁正确性）。
        m.tick(&tick16(), &cfg2, &None);
        assert_eq!(m.controller().active_pen().width, 60);
    }

    #[test]
    fn hotkey_toggles_even_when_input_suspended() {
        // 应用启动即打开控制面板 → `input_suspended` 默认真。此前挂起挡掉了 toggle，
        // 导致面板开着时按 Alt+G 毫无反应（修复：快捷键激活无效）。
        // ★ 开关先过门控（cfg 用 enabled_config）：挂起只豁免 toggle，不豁免门控。
        let cfg = enabled_config();
        let mut m = DrawingModule::new(&cfg);
        m.set_suspension(true, false); // 模拟面板开着
        m.tick(
            &tick16(),
            &cfg,
            &key_down(
                KeyCode::Char('g'),
                Modifiers {
                    alt: true,
                    ..none()
                },
            ),
        );
        assert!(
            m.controller().is_drawing(),
            "面板开着（input_suspended）时 Alt+G 仍应进入标注模式"
        );
        assert_eq!(m.controller().mode(), DrawMode::Screen);

        // 挂起时工具快捷键仍应被挡：不在书写上下文里切工具。
        m.tick(&tick16(), &cfg, &key_down(KeyCode::Char('4'), none()));
        assert_eq!(
            m.controller().tool(),
            Tool::Pen,
            "面板开着时工具快捷键不应生效"
        );
    }

    #[test]
    fn tool_hotkey_switches_tool() {
        let cfg = enabled_config();
        let mut m = DrawingModule::new(&cfg);
        // 先进书写模式（toggle 有 0.5s 去抖，用大 elapsed 避开）
        m.tick(
            &tick16(),
            &cfg,
            &key_down(
                KeyCode::Char('g'),
                Modifiers {
                    alt: true,
                    ..none()
                },
            ),
        );
        assert!(m.controller().is_drawing());

        m.tick(&tick16(), &cfg, &key_down(KeyCode::Char('4'), none()));
        assert_eq!(m.controller().tool(), Tool::Eraser);
        m.tick(&tick16(), &cfg, &key_down(KeyCode::Char('7'), none()));
        assert_eq!(m.controller().tool(), Tool::Rect);
        m.tick(&tick16(), &cfg, &key_down(KeyCode::Char('2'), none()));
        assert_eq!(m.controller().tool(), Tool::Pen);
        assert_eq!(
            m.controller().active_pen_index(),
            1,
            "画笔2 应切到第 2 支笔"
        );
    }

    #[test]
    fn tool_hotkey_ignored_outside_drawing() {
        let cfg = enabled_config();
        let mut m = DrawingModule::new(&cfg);
        m.tick(&tick16(), &cfg, &key_down(KeyCode::Char('4'), none()));
        assert_eq!(
            m.controller().tool(),
            Tool::Pen,
            "未进入书写时工具快捷键无效"
        );
    }

    #[test]
    fn esc_exits_and_delete_clears() {
        let cfg = enabled_config();
        let mut m = DrawingModule::new(&cfg);
        m.tick(
            &tick16(),
            &cfg,
            &key_down(
                KeyCode::Char('g'),
                Modifiers {
                    alt: true,
                    ..none()
                },
            ),
        );
        m.controller_mut()
            .begin_stroke(crate::core::geometry::Point { x: 1.0, y: 1.0 }, 0.0);
        m.controller_mut()
            .extend_stroke(crate::core::geometry::Point { x: 9.0, y: 9.0 });
        m.controller_mut().end_stroke();
        assert_eq!(m.controller().history().stroke_count(), 1);

        // Delete → 清空但仍在书写模式
        m.tick(&tick16(), &cfg, &key_down(KeyCode::Delete, none()));
        assert_eq!(m.controller().history().stroke_count(), 0);
        assert!(m.controller().is_drawing());

        // Esc → 报退出意图（真正停 + 留档在 `Runtime` 那一侧，见 `DrawRequest`）
        m.tick(&tick16(), &cfg, &key_down(KeyCode::Esc, none()));
        assert_eq!(m.take_request(), Some(DrawRequest::AnnotateExit));
        m.stop();
        assert!(!m.controller().is_drawing());
    }

    #[test]
    fn ctrl_z_undoes_only_while_drawing() {
        let cfg = enabled_config();
        let mut m = DrawingModule::new(&cfg);
        let ctrl = Modifiers {
            ctrl: true,
            ..none()
        };
        // 未进入书写：Ctrl+Z 不该把 tool 改掉，也不该 panic
        m.tick(&tick16(), &cfg, &key_down(KeyCode::Char('z'), ctrl));
        assert!(!m.controller().is_drawing());
    }

    #[test]
    fn repeated_keydown_is_ignored() {
        let cfg = enabled_config();
        let mut m = DrawingModule::new(&cfg);
        let alt_g = Modifiers {
            alt: true,
            ..none()
        };
        m.tick(&tick16(), &cfg, &key_down(KeyCode::Char('g'), alt_g));
        assert!(m.controller().is_drawing());
        // 长按重复：即便去抖窗口已过，也不该再 toggle 一次
        let mut t = tick16();
        t.elapsed_ms = 10_000.0;
        let repeated = Some(InputEvent::KeyDown {
            key: KeyCode::Char('g'),
            mods: alt_g,
            repeated: true,
            time_ms: 0,
        });
        m.tick(&t, &cfg, &repeated);
        assert!(m.controller().is_drawing(), "自动重复不该切走");
    }

    #[test]
    fn suspended_tool_hotkeys_blocked() {
        // 挂起时只有"开启/退出标注"的 toggle 快捷键仍生效（见
        // `hotkey_toggles_even_when_input_suspended`）；普通工具快捷键必须被挂起——
        // 面板开着时按一下不该切走工具。
        let cfg = enabled_config();
        let mut m = DrawingModule::new(&cfg);
        m.set_suspension(true, false);
        m.tick(
            &tick16(),
            &cfg,
            &key_down(KeyCode::Char('4'), none()), // 橡皮擦工具快捷键
        );
        assert_eq!(
            m.controller().tool(),
            Tool::Pen,
            "面板打开期间工具快捷键应挂起"
        );
    }

    /// 挂起必须**连鼠标一起挡**：面板开着时点一下（比如点「关于」页）不该在覆盖层上
    /// 落下一笔。只挂快捷键会留下这个 bug——那一下鼠标在模块看来和"画一笔"没区别。
    #[test]
    fn suspension_also_blocks_mouse_drawing() {
        let cfg = enabled_config();
        let mut m = DrawingModule::new(&cfg);
        m.tick(
            &tick16(),
            &cfg,
            &key_down(
                KeyCode::Char('g'),
                Modifiers {
                    alt: true,
                    ..none()
                },
            ),
        );
        m.set_suspension(true, false);

        m.tick(
            &tick16(),
            &cfg,
            &Some(InputEvent::MouseDown {
                button: crate::core::events::MouseButton::Left,
                x: 100,
                y: 100,
                time_ms: 0,
            }),
        );
        m.tick(
            &tick16(),
            &cfg,
            &Some(InputEvent::MouseUp {
                button: crate::core::events::MouseButton::Left,
                x: 200,
                y: 200,
                time_ms: 0,
            }),
        );
        assert_eq!(
            m.controller().history().stroke_count(),
            0,
            "挂起期间不该画出任何笔迹"
        );
        assert!(
            m.controller().is_drawing(),
            "挂起只是不消费输入，不该退出标注模式"
        );

        // 恢复后同一串事件必须落地——否则"挂起"就成了"再也画不了"。
        m.set_suspension(false, false);
        m.tick(
            &tick16(),
            &cfg,
            &Some(InputEvent::MouseDown {
                button: crate::core::events::MouseButton::Left,
                x: 100,
                y: 100,
                time_ms: 0,
            }),
        );
        m.tick(
            &tick16(),
            &cfg,
            &Some(InputEvent::MouseUp {
                button: crate::core::events::MouseButton::Left,
                x: 200,
                y: 200,
                time_ms: 0,
            }),
        );
        assert_eq!(
            m.controller().history().stroke_count(),
            1,
            "恢复后应能正常落笔"
        );
    }

    /// ★ Esc 豁免的回归锁：面板开着（挂起）时按 Esc 仍必须退出标注。
    /// 「快捷键进入后无法交互、也无法用 Esc 退出」的另一半根因：挂起把 Esc 也挡了。
    #[test]
    fn esc_exits_even_while_the_panel_is_open() {
        let cfg = enabled_config();
        let mut m = DrawingModule::new(&cfg);
        m.tick(
            &tick16(),
            &cfg,
            &key_down(
                KeyCode::Char('g'),
                Modifiers {
                    alt: true,
                    ..none()
                },
            ),
        );
        assert!(m.controller().is_drawing());
        m.set_suspension(true, false); // 模拟面板开着
        m.tick(&tick16(), &cfg, &key_down(KeyCode::Esc, none()));
        assert_eq!(
            m.take_request(),
            Some(DrawRequest::AnnotateExit),
            "面板开着时 Esc 也必须能报出退出意图"
        );
    }

    /// 文字输入窗开着时 Esc 归输入窗（windui 用它关窗取消），不得连标注一起退。
    #[test]
    fn esc_while_text_window_open_is_left_to_the_input_window() {
        let cfg = enabled_config();
        let mut m = DrawingModule::new(&cfg);
        m.tick(
            &tick16(),
            &cfg,
            &key_down(
                KeyCode::Char('g'),
                Modifiers {
                    alt: true,
                    ..none()
                },
            ),
        );
        m.set_suspension(false, true); // 模拟文字输入窗开着
        m.tick(&tick16(), &cfg, &key_down(KeyCode::Esc, none()));
        assert!(
            m.take_request().is_none(),
            "文字窗开着时 Esc 不该报退出意图"
        );
        assert!(m.controller().is_drawing(), "文字窗开着时 Esc 不该退出标注");
        // 关掉文字窗之后，Esc 恢复「退出标注」的职责。
        m.set_suspension(false, false);
        m.tick(&tick16(), &cfg, &key_down(KeyCode::Esc, none()));
        assert_eq!(m.take_request(), Some(DrawRequest::AnnotateExit));
    }

    /// ★★ 需求 4 的回归锁（2026-09-23 **两轮**用户报）：截图编辑态下 `Enter` 一律**无反应**。
    ///
    /// 第一轮我理解成"插文字时让给输入窗"，于是做成"没有文字窗时确认" —— 用户第二轮
    /// 明确否掉：*"按下 enter 键却依然会退出状态。这完全是错的…就如同我们在屏幕标注时，
    /// 按下 enter 键也没有反应，这才是对的。"*
    ///
    /// 所以判据是**在任何状态下都不报任何请求**：文字窗开着不报、关着也不报、小键盘回车
    /// 同款、编辑态也不会结束。完成整次截图只走工具栏那颗「保存（另存为）」。
    #[test]
    fn enter_never_does_anything_in_shot_edit() {
        let cfg = enabled_config();
        let mut m = DrawingModule::new(&cfg);
        m.controller_mut().start_with_backdrop(
            crate::core::backdrop::Backdrop::None,
            crate::core::geometry::Rect {
                x: 0.0,
                y: 0.0,
                w: 800.0,
                h: 600.0,
            },
        );
        assert!(m.controller().mode().is_shot_edit());

        // 两种挂起状态各敲一遍：两种情况都不许有反应。
        for text_window in [true, false] {
            m.set_suspension(false, text_window);
            for key in [KeyCode::Enter, KeyCode::NumEnter] {
                m.tick(&tick16(), &cfg, &key_down(key, none()));
                assert!(
                    m.take_request().is_none(),
                    "编辑态下 {key:?}（文字窗={text_window}）不该报出任何请求"
                );
                assert!(
                    m.controller().mode().is_shot_edit(),
                    "编辑态没被任何请求带走（真结束由 Runtime 执行）"
                );
            }
        }

        // ★ 对照组：`Esc` 仍然是出口。少了这条，上面的"无反应"可能只是"整个按键链路死了"。
        m.set_suspension(false, false);
        m.tick(&tick16(), &cfg, &key_down(KeyCode::Esc, none()));
        assert_eq!(m.take_request(), Some(DrawRequest::ShotEditCancel));
    }
}
