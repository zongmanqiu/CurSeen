//! 覆盖层窗口。透明、穿透鼠标、全屏覆盖。
//!
//! 这里是应用的**主** `App`：控制面板是它的子窗（`EventCtx::open_window`），托盘挂在它上面。
//! 不能像"先跑覆盖层、再跑面板"那样起两个 `App`——`App::run()` 是阻塞的消息循环，
//! 第二个 `App` 的语句永远执行不到，表现就是"双击 exe 什么窗口都不出"。

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use windui::anim::{request_repaint, request_repaint_in, request_repaint_in_after};
use windui::core::{EventCtx, Widget};
use windui::event::{Event, MouseButton, PointerKind};
use windui::geometry::{Color, Rect, Size};
use windui::platform::{overlay_is_parked, set_overlay_parked};
use windui::prelude::*;
use windui::render::Canvas;
use windui::render::Paint as WPaint;
use windui::text::TextEngine;

use crate::app::hotkeys;
use crate::app::tray::create_tray;
use crate::app::UiMsg;
use crate::core::events::{InputEvent, KeyCode};
use crate::core::exit_policy::ExitPolicy;
use crate::core::geometry::Point;
use crate::core::gif_zone::{hit_zone_at, HitZone, FRAME_LOGICAL, HANDLE_LOGICAL};
use crate::core::hotkeys::{match_drawing, DrawingAction};
use crate::platform::display::ScreenRect;
use crate::runtime::layer::{self, EscapeTarget, WorldMap};
use crate::runtime::render;
use crate::runtime::{
    FrameDemand, Runtime, TrimKind, TrimRequest, MOUSE_INPUT_NEEDED, VK_MOUSE_ARMED,
};
use crate::ui::panel;
use crate::ui::tokens::to_color;

/// 面板改动对账周期。平台在定时器到点时会 `InvalidateRect` 覆盖层，于是这条路径既能
/// 把面板改动同步进配置，也能在开关翻转时唤醒/休眠缓冲——两者都不依赖"覆盖层是否
/// 恰好在出帧"（三项功能全关时它本来就不出帧，而那正是用户去打开关的时刻）。
///
/// 覆盖层的 z 序守护（补丁 014）也挂在这个周期上：同样是"对账"性质，且 250ms 足够短，
/// 被置顶窗口盖住到重新插回顶部之间的间隙用肉眼看不出来。
const PANEL_POLL: Duration = Duration::from_millis(250);

/// 鼠标穿透态对账的**短期周期**。GIF 选框确认走的是 `app.channel` 消息（`set_click_through`
/// 的 window_op 在那条路径会被 windui 丢弃，见 `drain_channels`），所以穿透翻转不能依赖
/// 通道回调本身，必须由**事件路径的 interval** 落地。选框期间覆盖层持续出帧，这个 interval
/// 会高频触发；状态稳定时 `sync_mouse_mode` 幂等提前返回，零平台调用。
const MOUSE_POLL: Duration = Duration::from_millis(33);

/// 覆盖层的**鼠标穿透三态**。
///
/// 底层只有一位配置（`WS_EX_TRANSPARENT`），但"要不要交给 NCHITTEST 精判"分三档：
/// - [`Transparent`](MouseMode::Transparent)：置 `WS_EX_TRANSPARENT`，OS 命中直接跳过
///   本窗，一切穿透（既有非叠层态）。
/// - [`Interactive`](MouseMode::Interactive)：清掉 `WS_EX_TRANSPARENT`，整窗可由
///   NCHITTEST 收鼠标（标注 / 截图区域选择）；`on_nchit` 对所有点返回 `Client`。
/// - [`GifRegion`](MouseMode::GifRegion)：清掉 `WS_EX_TRANSPARENT`，由 NCHITTEST **选点**
///   命中：8 手柄与边框带返回 `Client`（可拖），选区内部/外部返回 `Transparent`
///   （录屏要在框内操作被录应用，必须穿透）。见 2f 的 `on_nchit`。
///   ★ 门是 [`Runtime::drag_interaction_enabled`]（GIF 选框**或**长截图就绪态）——
///   2026-10-01 实测：只认 GIF 时长截图就绪态落到 `Transparent`，OS 直接跳窗，
///   NCHITTEST 根本不被问，"选框结束后无法调整"。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MouseMode {
    Transparent,
    Interactive,
    GifRegion,
}

/// 贴图「另存为」快捷键。
///
/// 不是可配置项（用户口径：*"选中某张被 pin 图片，Ctrl+S 可以弹出另存为的文件对话框"*）。
/// **只在有选中贴图时生效**——没选中就完全不看这个组合，绝不会拦下用户在别的程序里
/// 按的 Ctrl+S（这也是选中态必须在点到别处时解除的原因，见 `Runtime::note_pointer_down`）。
const PIN_SAVE_HOTKEY: &str = "Ctrl+S";

/// 启动覆盖层（阻塞，直到应用退出）。
///
/// `input_rx` 来自 `InputHook::start()` 的配对端，由桥接线程转投 UI 通道。
pub fn run_overlay(runtime: Rc<RefCell<Runtime>>, input_rx: Receiver<InputEvent>) {
    // 虚拟桌面矩形（物理像素，原点可能为负）。
    let vs = runtime.borrow().screen;
    // 几何换算的唯一出处。世界坐标（物理像素）→ 窗口本地逻辑坐标。
    let map = WorldMap::new(vs);
    let dpi = map.dpi();
    let scale = 1.0 / dpi;
    let dx = -(vs.x as f32) * scale;
    let dy = -(vs.y as f32) * scale;
    // ★ `App::new(title, w, h)` 收的是**逻辑 dp**，不是物理像素。旧实现把虚拟桌面
    //   的物理尺寸直接传进去，非 100% 缩放下窗口会大出 dpi 倍、整屏缓冲多占 dpi² 倍
    //   （150% → 2.25×，200% → 4×）。尺寸只在 `WorldMap::logical_size()` 里算。
    let (vw, vh) = map.logical_size();

    // 工具面板按逻辑像素设计，绘制前要乘 DPI 换算成世界单位（世界单位 = 物理像素）。
    runtime.borrow_mut().drawing.set_ui_scale(dpi);

    log::info!(
        "覆盖层虚拟屏 {}x{} @ ({},{}) dpi={:.2} scale={:.4} 逻辑窗 {}x{}",
        vs.w,
        vs.h,
        vs.x,
        vs.y,
        dpi,
        scale,
        vw,
        vh
    );

    let mut app = App::new(crate::platform::topmost::OVERLAY_TITLE, vw, vh)
        .pos(vs.x, vs.y)
        .layered()
        .click_through()
        .tool_window()
        .no_activate()
        .bg(Color::TRANSPARENT);

    // UI 通道：输入桥接线程、更新线程与托盘都用它发意图；回调在 UI 线程落到 Runtime。
    //
    // 覆盖层当前的穿透态。翻转要调 `SetWindowLongPtrW` + `SetWindowPos`，而鼠标事件实测
    // 约 1000 条/秒都会走到这里，所以只在真的翻转时调一次（见 `sync_mouse_mode`）。
    let mouse_mode = Rc::new(Cell::new(MouseMode::Transparent));

    // ★ GIF 控件条当前**是否开着**。控件条是独立窗口，Runtime 里没有它的句柄，两边只能
    //   靠"闸门"同步：这里记着上一次同步后窗口的真实状态，`sync_gif_bar` 拿它跟
    //   `gif_selection()` 对账，只在**状态翻转**时开/关窗（幂等，两个调用点都安全）。
    let gif_bar_open = Rc::new(Cell::new(false));

    // ★ 长截图控件条的是否开着（2026-10-09 双条共存）：与 GIF 条同一条对账纪律，
    //   但是**独立的窗口**（各自的标题 / 标签 / 生命周期），互不牵连。
    let scroll_bar_open = Rc::new(Cell::new(false));

    // ★ 控件条上那颗键画的是图形（未上膛绿三角 / 上膛红圆），而**状态可能由别的窗口改变**
    //   （全局快捷键、3 秒倒计时归零）—— 控件条自己收不到任何事件。这里记着上一次喂给它的
    //   状态，变了就让那一小块重画（`win_util::invalidate_by_title`）。
    let gif_bar_armed = Rc::new(Cell::new(false));

    let chan_rt = runtime.clone();
    let chan_gifbar = gif_bar_open.clone();
    let chan_scrollbar = scroll_bar_open.clone();
    let ui_tx = app.channel(move |ctx: &mut EventCtx, msg: UiMsg| {
        match msg {
            UiMsg::OpenPanel => {
                log::info!("收到 OpenPanel 意图，开窗");
                open_panel(ctx, &chan_rt);
            }
            UiMsg::Input(ev) => {
                // 空闲时没有帧在跑，新来的输入必须由这里踢一帧才画得出来。
                // 但全关时输入没有任何视觉后果，踢帧只会把持续的鼠标移动钉成空转
                // （见 `Runtime::input_needs_frame`）。两种例外仍需踢帧：
                //   ① 正在捕获快捷键：按键要进 `frame()` 被 `try_capture_hotkey` 吃掉；
                //   ② 标注/白板的"开启"快捷键（Alt+G / Alt+W）：画笔勾选着、但其它
                //      功能全关时 `input_needs_frame()` 为假，这条键进不了 `frame()`；
                //      勾选着才认这两个键并踢帧（★ 开关门控：未勾选时按了也没反应，
                //      唤醒条件与 `DrawingModule::handle_key` 的门控严格同源）。
                    let wake = {
                    let rt = chan_rt.borrow();
                    // ★ 开关门控：画笔未勾选时 toggle 快捷键不唤醒（与 handle_key ①
                    //   同一条件）——否则未勾选时按键会踢帧进 handle_key，虽然那里也拦
                    //   得住，但每按键白醒一帧、空耗整屏重绘。
                    let toggle = rt.config.drawing.enabled
                        && matches!(
                            &ev,
                            InputEvent::KeyDown { key, mods, .. }
                                if matches!(
                                    match_drawing(*key, *mods, &rt.config.drawing.hotkeys),
                                    Some(DrawingAction::ToggleScreen | DrawingAction::ToggleWhiteboard)
                                )
                        );
                    // 截图 / 录屏快捷键：即便所有可视化功能都关着、覆盖层处于 park 态，这条键
                    // 也得把帧唤醒——否则按下截图键毫无反应（覆盖层休眠时不消耗任何输入事件）。
                    let action = rt.action_hotkey_wakes(&ev);
                    rt.input_needs_frame() || rt.is_capturing_hotkey() || toggle || action
                };
                if wake {
                    // ★ park 态立即解除（修复：截图快捷键反馈延迟 ~0.6–1.2s）。
                    //   park 态下 paint() 只渲进 scratch 不上屏；原先只能等 250ms 对账
                    //   周期解锁，反馈曲线前段整段丢失（帧差实测 0–600ms 无画面变化）。
                    //   wake 本身就意味着“接下来有东西要画”，直接解锁并重绘。
                    if overlay_is_parked() {
                        set_overlay_parked(false);
                        log::info!("park 已由快捷键唤醒立即解除");
                    }
                    request_repaint();
                }
                // 面板快捷键：自己的低级键盘钩子通路，不吞键（`RegisterHotKey` 会让前台程序
                // 再也收不到这个组合）。
                let hit =
                    matches!(ev, InputEvent::KeyDown { .. }) && panel_hotkey_hit(&chan_rt, &ev);
                // ★ Esc 关贴图窗：只在没有更高优先级的 Esc 消费者时（见优先级表：
                //   文字输入窗 > 区域选择 > 面板 > 标注 > **选中的那张贴图**）。
                //   贴图窗是独立子窗且不抢焦点，Esc 到不了它自己，只能在这里派发。
                //   手势归 `ExitPolicy::PIN` 管（`core::exit_policy`）——贴图窗自己的
                //   `on_event` 只处理双击/右键，Esc 这条通路在这里。
                //
                // ★★ 只关**选中的那一张**（用户口径 2026-09-17："我选择了哪张图，Esc 就
                //   只取消哪张图"）。一张都没选中时 `escape_target` 落到 `None`，Esc 归还用户。
                if matches!(ev, InputEvent::KeyDown { key: KeyCode::Esc, .. })
                    && ExitPolicy::PIN.escape
                    && chan_rt.borrow().escape_target() == EscapeTarget::Pins
                {
                    let (target, locked) = {
                        let mut rt = chan_rt.borrow_mut();
                        let seq = rt.close_selected_pin();
                        // 关不掉只可能是"选中的那张被锁住了"。那种情况必须**说一声**：
                        // 静默失败的表现是"按了 Esc 一点反应都没有"，用户会以为键盘坏了
                        // （2026-10-03 用户报"贴图后要允许 Esc 关闭"时撞的就是这个）。
                        let locked = seq.is_none() && rt.selected_pin_is_locked();
                        (seq, locked)
                    };
                    if let Some(seq) = target {
                        log::info!("Esc 关闭选中的贴图窗 #{seq}");
                        crate::platform::win_util::post_close_by_title(&layer::pin::pin_title(seq));
                    } else if locked {
                        log::info!("Esc 未关闭贴图窗：选中的那张已锁定");
                        chan_rt
                            .borrow_mut()
                            .notify_pin_locked_cannot_close();
                    }
                }
                // ★ Ctrl+S：把选中的那张贴图另存为 PNG。
                //
                //   为什么放在这里（而不是 `Runtime::frame` / `handle_action_key`）：保存
                //   对话框只能在 UI 线程且必须经 `defer_blocking` 延后执行，而通道回调本身
                //   就是 UI 线程的事件分发点；放进 frame 的话还得等下一帧才发得出去。
                //   按键**照常透传**（与所有快捷键同一纪律，见 `core::hotkeys` 的说明）——
                //   我们只额外做一次另存为，不吞掉用户的 Ctrl+S。
                if let InputEvent::KeyDown {
                    key,
                    mods,
                    repeated: false,
                    ..
                } = ev
                {
                    let selected = chan_rt.borrow().selected_pin();
                    if let Some(seq) = selected {
                        if crate::core::hotkeys::matches(PIN_SAVE_HOTKEY, key, mods) {
                            log::info!("Ctrl+S：另存为贴图 #{seq}");
                            let rt_save = chan_rt.clone();
                            ctx.defer_blocking(move || save_pin_as(&rt_save, seq));
                        }
                    }
                }
                chan_rt.borrow_mut().feed(ev);
                if hit {
                    // ★ toggle 而非 open（2026-09-27 用户报「能开不能关」）：此前这里只有
                    //   `open_panel`，第二次按下等于再开一次同名窗（`open_window` 对已开的
                    //   同名窗是无效动作），面板就再也收不回去。关窗走 `WM_CLOSE`
                    //   （`post_close_by_title`）而不是销毁：`on_close_request` →
                    //   `Runtime::close_panel` 负责最后那次调参落盘 + 采集窗口位置。
                    toggle_panel(ctx, &chan_rt);
                }
            }
            UiMsg::Update(ev) => {
                // 只改状态，不碰窗口：面板上的状态行/进度条绑在信号上，`Signal::set` 自己会
                // 把面板标脏重绘（见 windui `signal.rs` 的失效通道）。
                chan_rt.borrow_mut().apply_update_event(ev);
                // ★ 立即同步鼠标输入门：`apply_update_event` 可能改了 VK 开关/穿透态，
                //   桥接线程每条鼠标事件都读 `MOUSE_INPUT_NEEDED` 原子，晚同步一帧就
                //   丢一整批点击（用户刚勾上「启用键盘」，接下来 ~250ms 的键帽点击
                //   全被桥接线程扔掉）。
                chan_rt.borrow().sync_input_gate();
                // ★ 先对账再判门：新勾上的复合脚本要先进实例，下面的"要不要帧"才问得准
                //   （否则"勾了没反应"，详见 `Runtime::sync_trail_if_stale`）。
                chan_rt.borrow_mut().sync_trail_if_stale();
                // ★ 立即解除 park：配置变更可能首次启用了 VK（或关闭了穿透），覆盖层
                //   此前因「什么都没开」处于 park 态（`paint()` 不被调用 → `frame()` 不执行
                //   → `publish_hit_region()` 不执行 → 命中区永远为空 → 点键帽无反应）。
                //   这里主动 unpark，平台会在下一次消息循环里调 `paint()` → `frame()` →
                //   `publish_hit_region()`，键帽从此可点击。
                if chan_rt.borrow().input_needs_frame() && overlay_is_parked() {
                    set_overlay_parked(false);
                    log::info!("配置变更：覆盖层解除 park（VK/拖尾/绘制等需要帧）");
                }
            }
            UiMsg::TempUsage(u) => {
                // 与上面的 `Update` 同形：只写信号，不碰窗口（`Signal::set` 自己会把面板
                // 标脏重绘）。★ 这里**不需要**解 park——统计的是设置页里的一张卡，那种
                // 时刻覆盖层本来就醒着；它也不改任何覆盖层状态。
                chan_rt.borrow_mut().apply_temp_usage(u);
            }
            UiMsg::GifSelChanged => {
                // ★ GIF 选框是在**区域选择窗**里确认的（`layer::region` 的 `confirm`
                //   直接调 `Runtime::confirm_snap`），那一刻覆盖层多半正 park 着：
                //   功能全关时它不为任何输入出帧，paint 只渲进草稿、不上屏。
                //   —— 这就是"框完屏幕毫无反馈、要再按一次快捷键才出按钮"的根因
                //   （2026-09-20 用户报）。状态一变就由这条消息立刻解 park 并重绘，
                //   不等下一次用户输入、也不等 250ms 对账。
                if overlay_is_parked() {
                    set_overlay_parked(false);
                    log::info!("GIF 选框状态变化：覆盖层解除 park");
                }
                request_repaint();
            }
            UiMsg::GifTrimRequested => {
                // 与 `GifSelChanged` 同理：按按钮的是**控件条窗口**，开新窗得回到 UI 线程，
                // 这里只负责把覆盖层叫醒并让下面那段统一的开窗逻辑跑起来。
                if overlay_is_parked() {
                    set_overlay_parked(false);
                }
                request_repaint();
            }
            UiMsg::ShotEditChanged => {
                // ★ 与 `GifSelChanged` 逐字同一条理由：进入编辑态是**区域选择窗**在调
                //   `Runtime::confirm_snap`，覆盖层此刻多半正 park 着（用户可能没开任何
                //   需要出帧的功能）。不主动踢一脚的话，编辑界面（底图 + 选区外压暗 +
                //   跟随选区的工具栏）要等到下一次自然唤醒（250ms 对账）才出现 ——
                //   用户看到的是"框完停在原地，过一会儿才突然变成编辑界面"。
                if overlay_is_parked() {
                    set_overlay_parked(false);
                    log::info!("进入截图编辑态：覆盖层解除 park");
                }
                request_repaint();
            }
            UiMsg::ScrollChanged => {
                // ★ 与 `GifSelChanged` 同理：长截图的采集是在**区域选择窗**确认的那一拍
                //   开始的（`confirm_snap` 提前返回 → `arm_scroll`），覆盖层此刻多半正
                //   park 着。不踢这一脚，紫色采集边框要等 250ms 对账才上屏，而 250ms 内
                //   用户已经开始滚动了——边框迟到的窗口里抓到的帧是"没有提示"的，倒
                //   无害，但"框完毫无反馈"的手感已经足够让人怀疑按没按上。
                if overlay_is_parked() {
                    set_overlay_parked(false);
                    log::info!("长截图开始：覆盖层解除 park");
                }
                request_repaint();
            }
            UiMsg::SaveAsReady => {
                // ★ 「另存为」的最后一棒：图已经由 `Runtime` 离屏合成好了，这里只负责在
                //   **UI 线程且事件分发完全返回之后**把原生对话框弹出来（`defer_blocking`
                //   的闭包就是这么跑的，见 `file_dialog` 模块头）。
                //
                //   为什么不在 `Runtime::frame()` 里就地弹：那时 `&mut Runtime` 还被借着，
                //   而模态对话框会抽消息 → 覆盖层的窗口过程回来再借同一份状态（windui
                //   铁律 6 的 `&mut` 别名）。所以只能绕这一圈消息。
                let rt_save = chan_rt.clone();
                ctx.defer_blocking(move || save_shot_as(&rt_save));
            }
        }

        // 文字工具的落点定了 → 开输入窗。同样要放在消息之后：落点是 `MouseDown` 在上一帧
        // 被 `begin_stroke` 记下的，`feed()` 那一刻模块还没看见它。
        // 点击总会带来后面几条消息（mouseup / 移动），所以这里不会漏。
        open_text_input(ctx, &chan_rt);

        // ★ 截图 / 贴图：快捷键帧里的整虚拟桌面冻结帧已就位（或贴图请求已入队）→ 开窗。
        //   区域选择窗全屏置顶（冻结帧 + 框选 + 遮罩 + 放大镜）；贴图窗无边框不抢焦点。
        //   注意：先 take 再开窗——Rust 2021 的 if-let 条件临时借用会存活到块尾，
        //   块内再 borrow 会 panic（RefCell already mutably borrowed，实测踩坑）。
        let snap_open = chan_rt.borrow_mut().take_snap_request();
        let pin_req = chan_rt.borrow_mut().take_pin_request();
        if snap_open {
            layer::region::open(ctx, &chan_rt);
        }
        // ★ GIF 控件条：**状态驱动**，不是边沿触发。
        //   以前这里读的是一次性的 `gif_bar_req`，而它只在"有 UI 消息"时才会被读到 ——
        //   确认选框的那条路径（区域选择窗的 `confirm` 直接调 `Runtime::confirm_snap`）
        //   根本不产生 UI 消息，于是控件条有时不出现；反过来，`gif_sel` 被快捷键清掉后
        //   老窗口还留在屏幕上，按钮点了毫无反应（2026-09-20 用户报的三个症状同源）。
        //   现在只看 `gif_selection()`：开着就开窗、收起就关窗，两个调用点都幂等。
        sync_gif_bar(ctx, &chan_rt, &chan_gifbar, &chan_scrollbar);
        // ★ GIF 裁剪窗：同样走"取走意图 → 开窗"，与 250ms 对账共用 `open_gif_trim`。
        //   先把意图取进局部变量再判：`if let` 会让条件里的 `RefMut` 临时量活到整个块结束
        //   （Rust 2021 的规则，本项目在 `pin_req` / `take_rec_toast` 两处踩过），而
        //   `open_gif_trim` 内部还要再借一次。
        let trim_req = chan_rt.borrow_mut().take_gif_trim_request();
        if let Some(what) = trim_req {
            open_gif_trim(ctx, &chan_rt, what);
        }
        if let Some(req) = pin_req {
            open_pin(ctx, &chan_rt, req);
        }

        // ★ 进入标注模式时自动收起控制面板（修复：面板开着 → 标注输入挂起 + 覆盖层
        //   保持穿透，标注「进得去、动不了、退不出」）。hint 由 Runtime 在进入边沿
        //   置位（`Runtime::frame`），这里消费：面板是独立顶层窗口，只有 UI 线程能
        //   投递 WM_CLOSE。走 WM_CLOSE 而不是 DestroyWindow：`on_close_request` 里的
        //   收尾（改动落盘）照常执行。面板没开着时 hint 就地作废（take 已取走）。
        let hint = chan_rt.borrow_mut().take_panel_close_hint();
        if hint && chan_rt.borrow().panel_open() {
            log::info!("进入标注模式，自动收起控制面板");
            crate::platform::win_util::post_close_by_title(crate::ui::panel::TITLE);
        }

        // ★ 录制成功 → 桌面右下角提示窗（含「打开文件夹」入口，用户手动关闭）。
        let toast = chan_rt.borrow_mut().take_rec_toast();
        if let Some(path) = toast {
            open_rec_toast(ctx, &chan_rt, path, vs, dpi);
        }
    });

    // 把通道交给 Runtime：更新线程要用它把结果送回 UI 线程。
    runtime.borrow_mut().set_ui_tx(ui_tx.clone());

    let tray = create_tray(ui_tx.clone());

    // 单实例：二次启动（用户又双击了一次 exe）不再起第二套覆盖层与钩子——否则会双重特效、
    // 双重音效。windui 用内核命名 Mutex 判定（进程退出自动释放，不会留残渣），并把二次
    // 实例的 argv 转发给首实例、让二次实例的 `run()` 立即返回；我们只需把"又启动了一次"
    // 翻译成"打开控制面板"。
    // app_id 必须与 [`crate::APP_ID`] 一致（理由见那里的注释）。
    let single_tx = ui_tx.clone();
    let app = app.single_instance(crate::APP_ID, move |_argv| {
        log::info!("检测到二次启动，转交首实例：拉起控制面板");
        let _ = single_tx.send(UiMsg::OpenPanel);
    });

    // 周期对账：面板 → 配置（含落盘）、缓冲休眠状态、覆盖层 z 序。见 `PANEL_POLL` 的说明。
    let poll_rt = runtime.clone();
    let poll_mode = mouse_mode.clone();
    let poll_gifbar = gif_bar_open.clone();
    let poll_scrollbar = scroll_bar_open.clone();
    let poll_bar_armed = gif_bar_armed.clone();
    // ★ 长截图控件条的图标对账（与上面 `poll_bar_armed` 同一纪律）：快捷键也能
    //   启停长截图，控件条自己收不到事件，状态跳变时要让它重画。
    let poll_bar_armed_scroll = Rc::new(Cell::new(false));
    // ★ 控件条几何对账的"上次目标矩形"：选框每移动/缩放一次，就把它跟新 `bar_rect`
    //   比对，变了才 `SetWindowPos` 追过去（见 `sync_gif_bar_geometry`）。初始值取一个
    //   不可能与真实 `bar_rect` 相等的哨兵，保证选框刚落定后的第一拍必然触发一次对齐。
    let poll_bar_last = Cell::new((i32::MAX, i32::MAX, i32::MAX, i32::MAX));
    // 长截图条自己的一份（两条流各自独立对账，见 2026-10-09 双条共存）。
    let poll_scroll_last = Cell::new((i32::MAX, i32::MAX, i32::MAX, i32::MAX));
    let mut keeper = crate::platform::topmost::TopmostKeeper::new();
    // ★ 我们自己的工具窗也要一起守（控件条）：`Window::new` 建的普通窗**不带**
    //   `WS_EX_TOPMOST`，被任何抬到前面的窗口盖住就等于功能消失（用户 2026-09-20 报告）。
    //   GIF 裁剪窗是普通编辑窗，不守（用户 2026-09-20 决定：与屏幕效果无关，不该置顶）。
    //   标题在这里喂进去而不是在 `topmost` 里 `use`：platform 不能反向依赖 runtime。
    //
    // ★ 画中画放大窗（`pip_window`）也在这个名单里：它虽然自带 `WS_EX_TOPMOST`，但
    //   "在置顶带里"和"在置顶带最上面"是两件事——后来者会插到它上面（《踩坑记录》§2.3 的控件条
    //   踩过同一条坑）。不守的表现就是"放大镜开着忽然看不见了"。
    keeper.guard_aux(vec![
        layer::gif_bar::BAR_TITLE,
        // ★ 长截图控件条（2026-10-09 双条共存）：独立窗口，同样要守。
        layer::gif_bar::SCROLL_BAR_TITLE,
        // ★ 区域选择窗的鼠标放大镜：同样自带 `WS_EX_TOPMOST`，同样要守（理由同上）。
        //   它只在框选期间存在，`FindWindowW` 找不到时守护这条是空转，无害。
        layer::region_magnifier::REGION_MAG_TITLE,
        crate::platform::pip_window::PIP_WINDOW_TITLE,
        // ★ 全屏放大窗：与画中画同一个类、不同标题（同名会让 `FindWindowW` 只找到
        //   其中一个，见 `FULLSCREEN_WINDOW_TITLE` 上的注释）。
        crate::platform::pip_window::FULLSCREEN_WINDOW_TITLE,
    ]);
    let app = app.on_interval(PANEL_POLL, move |ctx| {
        poll_rt.borrow_mut().sync_ui();
        // ★ 长截图 250ms 兜底：工作线程自行收工（滚到底 / 画布满 / 抓屏失败）时在这里
        //   交付——用户停手后可能再没有输入消息，快捷键那条路不会跑。刚结束的这一拍
        //   补一次重绘，把紫色边框抹掉（浮字自己会续帧淡出）。
        let scroll_done = poll_rt.borrow_mut().poll_scroll();
        if scroll_done {
            ctx.mark_dirty_all();
        }
        // ★ 配置刚被面板改动过：补一帧把新配置画出来。
        //   悬浮标这类"静态跟随"特效的续帧判据是"光标移动了没"——改完参数那一刻光标
        //   多半停在面板上没动，帧门返回 `Idle`，而分层窗帧间不清缓冲，屏上就一直留着
        //   改动前那一帧（用户报"细分刻度有时起作用有时不起作用"）。这里踢一脚，
        //   那一帧会按新参数重画；`frame()` 末尾会清掉这个请求，不会反复踢。
        // ★★ 必须用 `ctx.mark_dirty_all()`，不能用 `windui::anim::request_repaint()`：
        //   后者只是置一个"本帧有动画请求"的线程局部位，**由下一次 `paint()` 末尾收割**
        //   ——在定时器回调里（不在绘制中）调它，没有任何东西被调度，`InvalidateRect`
        //   那条路要靠回调返回值 `repaint` 才走得通（win32 `apply_app_effects`：
        //   `if repaint || dirty { 所有窗口 InvalidateRect }`）。也就是说**只有
        //   `res.repaint = true` 才会真的排一次 WM_PAINT**，而置它的是 `EventCtx` 的
        //   `mark_dirty*`。此前用错函数 → 改完悬浮标参数后覆盖层根本不出帧，屏上一直
        //   留着改动前那一帧（用户报"细分刻度有时起作用有时不起作用"——生效与否取决于
        //   改完有没有碰鼠标，鼠标一动才有通道 `WM_APP_WAKE` 顺手踢一帧）。
        if poll_rt.borrow_mut().take_overlay_repaint_hint() {
            ctx.mark_dirty_all();
        }
        // 穿透对账（兜底）：进/出标注模式的消息通路万一漏了一条（比如快捷键在覆盖层
        // 之外的窗口被吞掉），这里 250ms 内必定纠正回来。判据本身幂等，稳定时不调 API。
        sync_mouse_mode(ctx, &poll_rt, &poll_mode);
        // 全局置顶策略（`Config::overlay`，勾一次对**所有**功能生效）→ z 序守护。
        // «全屏避让»：前台全屏时覆盖层让位（出厂勾上）。取消勾选 = 直播/录屏演示时覆盖层
        //（含拖尾/击键回显/虚拟键盘）压在全屏画面上。只压**全屏**让位；系统弹窗/菜单仍留其上。
        let avoid_fullscreen = poll_rt.borrow().config.overlay.avoid_fullscreen;
        keeper.set_avoid_fullscreen(avoid_fullscreen);
        // ★ 「总在最上」→ z 序守护的**夺回**分支。
        //   2026-09-18 之前这一位没有任何消费者：配置里有、面板上有勾选框、也能落盘，
        //   但守护恒把覆盖层插回置顶带顶部 —— 勾不勾都一样，是个死字段。这里接上：
        //   关掉 = "普通置顶"（不跟别的置顶窗抢），开 = 被压住就夺回来。
        let always_on_top = poll_rt.borrow().config.overlay.always_on_top;
        keeper.set_always_on_top(always_on_top);
        // ★ 有 GIF 选框时**不让位**（全屏避让临时失效）：边框画在覆盖层上，让位就等于
        //   用户看不见自己在录哪一块；而且录制期本来就该"特效与边框可见"（拖尾要被录进
        //   产物）。选框一收立刻回到常规让位语义。
        //
        // ★★ 截图**编辑态**同理，而且更硬：整个编辑界面（冻结帧 + 压暗 + 选框 + 工具栏）
        //   都画在覆盖层上，让位就是"用户看不见自己在改什么"——他会以为程序卡了。用户在
        //   编辑一张截图时，前台是什么全屏应用与他无关（那张被编辑的画面是**冻结帧**，
        //   不是活的屏幕）。
        let gif_sel = poll_rt.borrow().gif_selection().is_some();
        // ★ 「不让位」的判据比 `gif_sel` 宽：加上截图编辑态（理由见上）。
        let no_yield = gif_sel || poll_rt.borrow().shot_edit_active();
        keeper.set_suppress_yield(no_yield);
        // ★ 控件条那颗键的图形要跟着状态换（绿三角 ↔ 红圆）。状态可能由别的窗口改变
        //   （全局快捷键 / 倒计时归零），控件条收不到事件 ⇒ 状态变化时让它重画一次。
        //   只在**跳变**时发：稳定时一次 API 都不调。
        let armed = poll_rt.borrow().gif_armed();
        if armed != poll_bar_armed.get() {
            poll_bar_armed.set(armed);
            if gif_sel {
                crate::platform::win_util::invalidate_by_title(layer::gif_bar::BAR_TITLE);
            }
        }
        // 长截图条同款（快捷键启停时条收不到事件）：绿三角 ↔ 红圆。
        let scroll_armed = poll_rt.borrow().scroll_active();
        if scroll_armed != poll_bar_armed_scroll.get() {
            poll_bar_armed_scroll.set(scroll_armed);
            if poll_rt.borrow().scroll_bar_selection().is_some() {
                crate::platform::win_util::invalidate_by_title(layer::gif_bar::SCROLL_BAR_TITLE);
            }
        }
        // ★ 控件条跟随选框（位置 + 宽度）：选框会被 8 点/边框拖动缩放，而控件条在
        //   `open()` 时只定过一次位。当选区变化导致 `bar_rect` 的期望矩形变了，就把条
        //   追过去。幂等——没变时一次 API 都不调。
        sync_gif_bar_geometry(&poll_rt, &poll_bar_last, &poll_scroll_last);
        // z 序守护（补丁 014）：`WS_EX_TOPMOST` 只保证"在置顶带里"，不保证带内靠前——
        // 晚于我们创建/被抬起的置顶窗口会压在我们上面，拖尾就被盖住（实测墨迹归零，
        // 而 CPU 照旧，说明只是看不见）。这里每 250ms 对账一次，必要时把自己插回顶部；
        // 前台是全屏应用/游戏时主动让位。详见 `platform/topmost.rs` 的模块说明。
        keeper.reconcile();
        // ★ 休眠态下补一次模块对账（配置版本变了才真干活）。放在读帧门**之前**：
        //   新勾上一个复合脚本时，"它到底要不要覆盖层的帧"只有同步进实例才知道，而
        //   同步平时挂在 `tick()` 里、休眠时根本不跑（详见 `Runtime::sync_trail_if_stale`）。
        poll_rt.borrow_mut().sync_trail_if_stale();
        // ★★ 同样要在这里补一次"开关关闭 → 停掉进行中任务"的对账，且**必须在读帧门
        //   之前**。理由与上面那条同源、后果更严重：`reclaim_disabled_modules` 平时挂在
        //   `frame()` 里，而 `frame()` 只在 `paint()` 中被调 —— 覆盖层一旦 park，它一次
        //   都不跑。计时器正是这个形状：用户关掉开关时若覆盖层已经 park（屏幕上什么
        //   都不闪，恰恰是最可能 park 的场景），边沿就永远检测不到，计时器会一直跳到
        //   用户重启程序为止（用户 2026-10-03 报的就是这个）。
        poll_rt.borrow_mut().reclaim_disabled_modules();
        // 覆盖层缓冲休眠（补丁 007）：分层覆盖层是整屏的，DIB section（tiny-skia 直接
        // 画进 DIB 内存，只有这一份缓冲）按面积线性增长——1080p 约 8.3 MB、4K 约 33 MB。
        // 功能全关时一个像素都不用画，那份缓冲纯属浪费（曾占 CurSeen 空载私有内存的
        // 60%，旧双缓冲架构时代实测 24MB/4K≈100MB；2026-09-30 更正为现行单 DIB 数字）。
        // 用对账而不是"开关回调里通知一次"：开关可能来自面板、托盘、全局快捷键，也可能来自
        // 用户手改配置文件——逐个埋点迟早漏一条。
        let want_park = !poll_rt.borrow().input_needs_frame();
        if overlay_is_parked() != want_park {
            set_overlay_parked(want_park);
            // 平台在定时器回调后会 InvalidateRect，这一帧就是过渡帧：休眠那次要把窗口
            // 刷成全透明再释放缓冲（否则最后呈现的一帧会永久留在屏上），唤醒那次要重建
            // 缓冲并画上内容。
            if want_park {
                // ★ 进入休眠的这一刻补一次延迟裁剪。**释放不等于回落**：缓冲刚被
                //   `DeleteObject` / `drop` 掉，那些页仍挂在进程工作集里，任务管理器
                //   的"内存"数字不会自己变小 —— 实测休眠后私有工作集仍有 10.29 MB，
                //   `EmptyWorkingSet` 之后落到 0.82 MB（`examples/mem_probe.rs` idle 档）。
                //   只在**进入**休眠这条边沿上裁：唤醒后马上要连续出帧，裁了等于制造
                //   一次缺页风暴。
                crate::platform::working_set::request_trim_soon();
            }
        }
        // ★ GIF 控件条兜底（与通道回调同一逻辑，见 `sync_gif_bar`）：`GifSelChanged`
        //   消息通路万一没送达（覆盖层尚未起、`ui_tx` 还没装上），这里 250ms 内补上，
        //   保证"框好选框"与"控件条出现"之间不会无限期地差一拍。
        sync_gif_bar(ctx, &poll_rt, &poll_gifbar, &poll_scrollbar);
        // 裁剪窗的 250ms 兜底：与控件条同一理由（通道回调只在有 UI 消息时跑）。
        let trim_req = poll_rt.borrow_mut().take_gif_trim_request();
        if let Some(what) = trim_req {
            open_gif_trim(ctx, &poll_rt, what);
        }
        // 收面板的 250ms 兜底（与通道回调同一逻辑）：进入标注时面板还开着，而此后
        // 可能再没有输入消息（用户手不在鼠标上），没有这条兜底面板就不会收起。
        let hint = poll_rt.borrow_mut().take_panel_close_hint();
        if hint && poll_rt.borrow().panel_open() {
            log::info!("进入标注模式（定时器兜底），自动收起控制面板");
            crate::platform::win_util::post_close_by_title(crate::ui::panel::TITLE);
        }
        // ★ 贴图窗的 250ms 兜底：通道回调只在有 UI 消息时跑，而"截图并贴图"是在区域窗
        //   里确认的——拖拽松手可能是用户在这一轮交互里的最后一条消息。这一条保证贴图窗
        //   最迟 250ms 内出现（鼠标还在动时通常 <20ms）。
        //
        // ★★ 先 take 再 open（**不是** `if let Some(req) = poll_rt.borrow_mut()...`）。
        //    `if let` 的 scrutinee 临时量活到**整个 if-let 语句结束**（Rust 2021 的经典坑），
        //    于是块内 `open_pin()` 再 `borrow_mut()` 就会撞上 RefCell 重入 → panic。
        //    实测崩溃现场（build/log/curseen-20260917.log:127）：
        //      `[FATAL] panic: RefCell already borrowed  at src\runtime\overlay.rs:332:24`
        //    调用栈在 `NtUserDispatchMessage → CallWindowProcW` 里 —— 定时器回调期间。
        //    触发条件是竞态：消息通路（下文 UiMsg::Input 分支）通常先取走请求，只有
        //    "区域窗松手后不再有 UI 消息"时这一条才抢到，故表现为"多点几次才崩"。
        let pin_req = poll_rt.borrow_mut().take_pin_request();
        if let Some(req) = pin_req {
            open_pin(ctx, &poll_rt, req);
        }
        // ★ 录制成功提示窗的 250ms 兜底：停止快捷键之后可能再没有输入消息
        //   （用户手已离开键盘），通道回调那条路不会跑，这里保证提示窗最迟 250ms 内出现。
        let toast = poll_rt.borrow_mut().take_rec_toast();
        if let Some(path) = toast {
            open_rec_toast(ctx, &poll_rt, path, vs, dpi);
        }
    });

    // ★ 鼠标穿透态的**事件路径**对账（见 `MOUSE_POLL` 说明）：GIF 选框确认是经
    //   `app.channel` 消息走到的，而通道回调里 `set_click_through` 的 window_op 会被
    //   windui 丢弃——穿透翻转只能在 interval（事件路径）里落地。短周期保证选框确认后
    //   下一拍就把 `WS_EX_TRANSPARENT` 清掉，8 点/边框立即可拖。状态稳定时幂等提前返回，
    //   不产生平台调用。
    let mouse_rt = runtime.clone();
    let mouse_mode_cell = mouse_mode.clone();
    let app = app.on_interval(MOUSE_POLL, move |ctx| {
        sync_mouse_mode(ctx, &mouse_rt, &mouse_mode_cell);
    });

    // ★ Esc 永不关闭覆盖层主窗口（修复：标注态按 Esc 退出整个软件）。
    //   windui 对未被消费的 Escape 有「关窗」兜底（`app::on_key`：
    //   `Key::Escape if resolve_close() => close = true`），而覆盖层是主 App 窗口——
    //   标注态下鼠标落在覆盖层上、窗口拿到键盘焦点，Esc 一按整个应用就退了。
    //   应用级 `on_shortcut` 排在该兜底**之前**（windui 文档明言这个顺序就是为了让
    //   应用能覆盖 Escape 关窗）：这里把 Escape 一律吃掉。标注的 Esc 语义（退出标注）
    //   由低级键盘钩子 → `DrawingModule::handle_key` 处理，与窗口消息无关；
    //   面板/文字窗是独立窗口，各自的 Esc 语义（关窗取消）不受影响。
    let app = app.on_shortcut(|_ctx, ev| ev.key == Key::Escape);

    // ★★ GIF 选区交互的两条区域级钩子（windui 补丁 083-c/d）：
    //   覆盖层默认整窗置 `WS_EX_TRANSPARENT`（穿透），得靠 NCHITTEST 对**选区边框/手柄**
    //   返回 HTCLIENT、对**框内/框外**返回 HTTRANSPARENT，才能做到"边框可拖 + 框内穿透"。
    //   `on_cursor_at` 让手柄/边框悬停光标正确（角=对角、边=单向、边框=四向移动）。
    //   入参是**客户区物理坐标**（补丁 083-d 从 WM_NCHITTEST / WM_SETCURSOR 直接换算），
    //   世界物理坐标 = 客户区 + 虚拟桌面原点 `vs.origin`。
    //
    //   关键前提：这两条只在 `GifRegion` 鼠标态（`gif_interaction_enabled()`）才收鼠标——
    //   那是 `sync_mouse_mode` 清了 `WS_EX_TRANSPARENT`、把命中阶段交回给本窗之后。
    //   一旦录制 / 选框收起，模式回 `Transparent`（重置 `WS_EX_TRANSPARENT`），NCHITTEST
    //   根本不会发到本窗，钩子自然失效，框内依旧穿透操作被录应用。这就是"录制中只显示
    //   不可拖"。
    let nchit_rt = runtime.clone();
    let origin = (vs.x, vs.y);
    let cursor_rt = runtime.clone();
    // ★ 文字工具的悬停提示（2026-09-26 用户需求："可以拖动当前文本时，指针要有变化"）。
    //   命中判据 = `text_hover_at`（标注中 + 文字工具 + 点落在文字包围格外扩 6px 内）。
    //   两个钩子都要认：NCHITTEST 不回 Client 的话 WM_SETCURSOR 走不到客户区分支，
    //   光标钩子根本不会被问。
    fn text_hover_shape(rt: &Rc<RefCell<Runtime>>, wx: i32, wy: i32) -> bool {
        rt.borrow()
            .drawing
            .text_hover_at(crate::core::geometry::Point {
                x: wx as f32,
                y: wy as f32,
            })
    }
    let app = app
        .on_nchit(move |p| {
            let (wx, wy) = (p.x + origin.0, p.y + origin.1);
            let edit = nchit_rt.borrow().shot_edit_active();
            // ★ 屏幕标注（透明幕布）也整屏归我们。此前漏了这一档：标注态 NCHITTEST
            //   对所有点回 Transparent，笔的 WM_POINTER / 手势查询被送进桌面，
            //   补丁 092/093 收不到任何消息，起笔只能等翻译层迟到 ~0.5s 的鼠标副本
            //   —— 数位板"起笔延迟"四报的最终根因（2026-09-29）。
            let annotate = nchit_rt.borrow().drawing_owns_pointer();
            let zone = zone_at_rt(&nchit_rt, wx, wy);
            // ★ 两档语义不同，别把它们合成一条判据：
            //   · 截图**编辑态** ⇒ **整屏归我们**。此刻屏上是"冻结帧 + 选区外压暗 + 工具栏"，
            //     用户在做一次模态编辑；落点若穿通，他点在压暗区上会顺手触发下面那个程序的
            //     按钮，而他以为自己只是在空处点了一下准备落笔。
            //   · GIF 选框 ⇒ 手柄/边框带归我们、其余穿通（框内要能操作被录应用）。
            //   · 文字悬停带 ⇒ 归我们（拖动/双击都发生在这里），且要让 WM_SETCURSOR
            //     能进来把光标换成移动。
            let text_hover = text_hover_shape(&nchit_rt, wx, wy);
            Some(if edit || annotate || zone != HitZone::None || text_hover {
                windui::event::NcHit::Client
            } else {
                windui::event::NcHit::Transparent
            })
        })
        .on_cursor_at(move |p| {
            let (wx, wy) = (p.x + origin.0, p.y + origin.1);
            // 文字悬停最优先：四向移动光标告诉用户"这里能拖"。
            if text_hover_shape(&cursor_rt, wx, wy) {
                return Some(windui::event::CursorShape::SizeAll);
            }
            let shape = match zone_at_rt(&cursor_rt, wx, wy) {
                HitZone::Body => windui::event::CursorShape::SizeAll,
                HitZone::CornerTL | HitZone::CornerBR => windui::event::CursorShape::SizeNWSE,
                HitZone::CornerTR | HitZone::CornerBL => windui::event::CursorShape::SizeNESW,
                HitZone::EdgeT | HitZone::EdgeB => windui::event::CursorShape::SizeNS,
                HitZone::EdgeL | HitZone::EdgeR => windui::event::CursorShape::SizeWE,
                HitZone::None => return None,
            };
            Some(shape)
        });

    let app = app
        .content(
            Element::leaf()
                .widget(OverlayCanvas::new(runtime.clone(), scale, dx, dy))
                .fill(),
        )
        .tray(tray);

    // 输入桥接线程（UI 通道断开时自行退出）。
    let _bridge = spawn_input_bridge(input_rx, ui_tx.clone());

    // 启动即打开控制面板：用户期望双击 exe 就看到可用窗口，而不是只剩一个看不见的
    // 透明覆盖层。`single("panel")` 保证重复请求只把已开的窗口拉到前台。
    let _ = ui_tx.send(UiMsg::OpenPanel);

    // 首帧之前先定好休眠态，避免"启动瞬间为了一帧空屏而分配整屏缓冲"。
    set_overlay_parked(!runtime.borrow().input_needs_frame());
    let parked_at_boot = overlay_is_parked();
    log::info!(
        "覆盖层缓冲初始状态：{}",
        if parked_at_boot {
            "休眠（功能全关，不分配整屏缓冲）"
        } else {
            "活跃"
        }
    );
    // ★ 启动即休眠时这里**没有 park 边沿**（初始态就是休眠，250ms 对账里
    //   `overlay_is_parked() == want_park` 恒等，不会触发裁剪）。而启动过程本身
    //   （配置读取、面板建窗、字体/图标加载）会在堆上留下一批用毕的页 —— 不裁一次，
    //   空载常驻就顶在 10~16 MB 下不来。裁一次即可，之后由休眠边沿接管。
    if parked_at_boot {
        crate::platform::working_set::request_trim_soon();
    }

    app.run();

    // ★ 收尾：退出时若虚拟键盘上还按着一颗键（用户按着鼠标点的键直接关程序），
    //   注入的"按下"会留在系统键盘状态里 —— 表现为**卡住不放的 Ctrl/Shift**，
    //   直到用户亲手再按一次那颗物理键。这里补一次抬起，代价是退出路径上的一次
    //   `SendInput`（没有键按着时连这个都不发）。
    runtime.borrow_mut().vk_release_all();
}

/// 打开（或把已打开的控制面板激活到前台）。
///
/// `single("panel")` 让平台层去重：同键窗口已存在时丢弃本次请求并把它拉到前台。
fn open_panel(ctx: &mut EventCtx, rt: &Rc<RefCell<Runtime>>) {
    let rt = rt.clone();
    // 位置还原（★ 主窗口状态记忆）：有记录就落回上次位置；从未记录（首启）才居中。
    // `panel_pos` 在面板关闭时采集（`Runtime::close_panel`），物理屏幕像素、含副屏负坐标。
    let saved_pos = rt.borrow().config.ui.panel_pos;
    let mut win = Window::new(panel::TITLE, panel::WIDTH, panel::HEIGHT)
        .single("panel")
        .resizable(false);
    win = match saved_pos {
        Some((x, y)) => win.position(x, y),
        None => win.centered(true),
    };
    ctx.open_window(
        win.bg(to_color(crate::ui::tokens::BG))
            .on_close_request({
                let rt = rt.clone();
                move |_ctx| {
                    // 窗口关闭即真正落盘，兜住最后一次微调（防抖窗口内未写入的部分）。
                    rt.borrow_mut().close_panel();
                    true
                }
            })
            // ★ 依赖倒置：面板经 `PanelHost` 向宿主取值，不再直接握 `Rc<RefCell<Runtime>>`
            //   （`ui` 层因此不反向依赖 `runtime`，见 `ui::PanelHost` 的注释）。
            //   借用只在建树这一下：`content` 回调返回后借用即释放。
            .content(move || crate::ui::build_panel(&mut *rt.borrow_mut())),
    );
}

/// 打开一个贴图窗，并做"同时存活上限"的淘汰（超出就关最早那个）。
///
/// 记在这里而不是控件里：上限是**全局**约束（跨所有贴图窗），控件只知道自己那一个。
fn open_pin(ctx: &mut EventCtx, rt: &Rc<RefCell<Runtime>>, req: crate::runtime::snap::PinRequest) {
    let (screen, old) = {
        let mut r = rt.borrow_mut();
        // 内容与窗口一起记账：选中后 Ctrl+S 另存为要拿的就是它。
        let old = r.snap.note_pin_opened(req.seq, req.content.clone());
        (r.screen, old)
    };
    if let Some(old) = old {
        log::info!("贴图窗超过上限，关闭最早的一个 #{old}");
        crate::platform::win_util::post_close_by_title(&layer::pin::pin_title(old));
    }
    layer::pin::open(ctx, rt, &req.content, req.seq, screen, req.origin);
}

/// 录制成功提示窗的尺寸（逻辑像素）。
const REC_TOAST_W: i32 = 400;
const REC_TOAST_H: i32 = 150;
/// 提示窗内容区的内边距（`Element::col().padding(14)`，四边同值）。
const REC_TOAST_PAD: i32 = 14;
/// 内容区可用文字宽（逻辑 px）= 窗宽 − 左右内边距 = 372。
///
/// ★ windui 的文本**不换行**、超宽即溢出，而这个窗是固定 400×150 且不可缩放，所以
/// 文件名与目录只能**按这个预算截断**（[`rec_toast_elide`]）。★ 不要试图靠缩小字号
/// 去"挤"：一条 40–60 字符的绝对路径按 14px 排要 400+px、按 12px 排也要 340+px，
/// 缩字号只是让溢出来得晚一点，并没有真正解决。
const REC_TOAST_TEXT_W: f32 = (REC_TOAST_W - REC_TOAST_PAD * 2) as f32;
/// 「保存位置：」前缀。★ 单独量它、路径只在**剩下的**预算里截断：截断是保尾部的，
/// 把前缀和路径拼成一行再截会把这个前缀切掉。
const REC_TOAST_FOLDER_LABEL: &str = "保存位置：";
/// 提示窗底边与任务栏顶边之间留的空隙（物理像素）：确保**绝不**遮挡任务栏。
const TASKBAR_MARGIN: i32 = 8;

/// 取主显示器的工作区（`rcWork`，已排除任务栏）与其 DPI 缩放比。
///
/// 用 `GetMonitorInfo` 的 `rcWork` 而不是 `SPI_GETWORKAREA`：前者是「按显示器」的、
/// 能正确反映任务栏所在那块屏的工作区；`SPI_GETWORKAREA` 在多屏/自动隐藏任务栏等配置下
/// 偶发返回全屏（任务栏被算进工作区），导致提示窗底边压到任务栏上。
/// 返回 `(工作区矩形, dpi/96)`：矩形是物理像素、`Window::position` 直接吃物理像素，
/// 尺寸换算用同一块的 dpi 才不会出现「位置对、大小错」的错位。
fn primary_work_rect() -> (ScreenRect, f32) {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
    };
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_DEFAULT};
    unsafe {
        let mon = MonitorFromWindow(HWND(std::ptr::null_mut()), MONITOR_DEFAULTTOPRIMARY);
        if !mon.is_invalid() {
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if GetMonitorInfoW(mon, &mut info).as_bool() {
                let r: RECT = info.rcWork;
                let mut dx = 96u32;
                let mut dy = 96u32;
                let _ = GetDpiForMonitor(mon, MDT_DEFAULT, &mut dx, &mut dy);
                return (
                    ScreenRect {
                        x: r.left,
                        y: r.top,
                        w: r.right - r.left,
                        h: r.bottom - r.top,
                    },
                    dx as f32 / 96.0,
                );
            }
        }
    }
    // 极端兜底：取不到主屏信息就退回全工作区 + 1.0 缩放（逻辑=物理）。
    (crate::platform::display::work_area(), 1.0)
}

/// 打开录制成功提示窗：桌面右下角，显示文件名与保存位置，
/// 按提示窗的文字宽截断文件名/目录（保尾部，见 [`card::ellipsize`]）。
///
/// 字号统一走 `FONT_MD`（面板正文档）——**不为了塞下而降档**：窗是固定宽、文本不换行，
/// 长路径按 12px 一样会溢出，降档只是把溢出门槛推远，反而多出一档没人认领的字号。
fn rec_toast_elide(s: &str, budget: f32) -> String {
    crate::ui::card::ellipsize(s, budget, crate::ui::tokens::FONT_MD)
}

/// 提供「打开文件夹」（explorer /select 选中该文件）。
///
/// 用户口径：提示窗**不自动消失**，由用户手动关闭；`single("rec_toast")`
/// 保证连录多段时重复请求只刷新同一扇窗并拉到前台。
/// ★ 定位用 work_area（排除任务栏），窗口标题栏自带关闭按钮，不再额外加。
fn open_rec_toast(
    ctx: &mut EventCtx,
    rt: &Rc<RefCell<Runtime>>,
    path: std::path::PathBuf,
    _vs: ScreenRect,
    _dpi: f32,
) {
    // 用主显示器工作区（排除任务栏）定位；尺寸按该显示器 dpi 把逻辑 dp 换算回物理。
    let (wa, scale) = primary_work_rect();
    let phys_w = (REC_TOAST_W as f32 * scale) as i32;
    let phys_h = (REC_TOAST_H as f32 * scale) as i32;
    // 右边留 12px；底边留 `TASKBAR_MARGIN`(8px) 余量，窗口底边严格在任务栏上方、绝不遮挡。
    let x = wa.x + wa.w - phys_w - 12;
    let y = wa.y + wa.h - phys_h - TASKBAR_MARGIN;

    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let folder = path
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let open_path = path.clone();
    // 「裁剪」：直接打开**刚录这份产物**的裁剪窗，按扩展名路由（MP4 → 时间裁剪窗，
    // GIF → 裁剪窗）。2026-10-08 用户口径：按钮直接叫「裁剪」，且 GIF 也要有——
    // 之前 GIF 只能再按一次控件条上的剪刀（提示窗一关就没了入口）。
    let is_mp4 = path
        .extension()
        .map(|e| e.eq_ignore_ascii_case("mp4"))
        .unwrap_or(false);
    let rt_trim = rt.clone();
    let mut btns = Element::row()
        .padding_xy(0, 8)
        // ★ 两颗按钮之间留一档间距：贴在一起看着像一根 doubled 按钮（用户 2026-09-30 报）。
        .spacing(12)
        .child(Element::button("打开文件夹").on_click(move |_ctx| {
            // explorer /select：打开所在文件夹并选中文件。
            // spawn 不等待；失败只记日志（提示窗还在，用户可手动找）。
            if let Err(e) = std::process::Command::new("explorer")
                .arg(format!("/select,{}", open_path.display()))
                .spawn()
            {
                log::warn!("打开文件夹失败: {e}");
            }
        }));
    if is_mp4 {
        btns = btns.child(Element::button("裁剪").on_click(move |_ctx| {
            rt_trim.borrow_mut().request_mp4_trim();
        }));
    } else {
        // ★ 不走 `request_gif_trim`（它按 `sel_kind` 转发，可能被后来的选框改道）：
        //   提示窗对应的就是刚落盘的这份产物，按产物类型直达。
        btns = btns.child(Element::button("裁剪").on_click(move |_ctx| {
            rt_trim.borrow_mut().request_gif_trim_from_toast();
        }));
    }
    log::info!("录制成功提示 @ ({x},{y})：{}", path.display());
    // ★ 窗宽固定、文本不换行 → 先按像素预算截断（字号仍用面板正文档，不降档）。
    let folder_budget = REC_TOAST_TEXT_W
        - crate::ui::card::text_width(REC_TOAST_FOLDER_LABEL, crate::ui::tokens::FONT_MD);
    let file_name = rec_toast_elide(&file_name, REC_TOAST_TEXT_W);
    let folder = rec_toast_elide(&folder, folder_budget);

    ctx.open_window(
        Window::new("CurSeen 录制完成", REC_TOAST_W, REC_TOAST_H)
            .single("rec_toast")
            .resizable(false)
            .position(x, y)
            .bg(to_color(crate::ui::tokens::BG))
            .content(move || {
                Element::col()
                    .fill()
                    .padding(14)
                    .child(
                        Element::label("录制成功")
                            .font_size(crate::ui::tokens::FONT_MD)
                            .fg(to_color(crate::ui::tokens::TEXT)),
                    )
                    // ★ 下面两行的字号与面板完全一致（`FONT_MD`）：放不下的是路径长度，
                    //   不是字号——长度交给上面的 `rec_toast_elide` 按像素预算截断解决。
                    .child(
                        Element::label(file_name.clone())
                            .font_size(crate::ui::tokens::FONT_MD)
                            .fg(to_color(crate::ui::tokens::TEXT_SECONDARY))
                            .padding_xy(0, 4),
                    )
                    .child(
                        Element::label(format!("{REC_TOAST_FOLDER_LABEL}{folder}"))
                            .font_size(crate::ui::tokens::FONT_MD)
                            .fg(to_color(crate::ui::tokens::TEXT_SECONDARY)),
                    )
                    .child(btns)
            }),
    );
}

/// 把 `Runtime` 备好的那张图另存为 PNG（截图 / 屏幕标注的保存按钮）。
///
/// 必须在 UI 线程、且经 `ctx.defer_blocking` 跑：原生保存对话框自带消息泵、会阻塞到
/// 松开为止，从窗口过程或帧回调里直接调就是重入（与「浏览…」选图同一条纪律）。
fn save_shot_as(rt: &Rc<RefCell<Runtime>>) {
    // 先把要用的东西取出来、**立刻结束借用**：下面要弹对话框，它会抽消息、可能让覆盖层
    // 的窗口过程跑起来（那条路会再借一次同一份状态）。
    let Some((shot, name)) = rt.borrow_mut().take_pending_save_as() else {
        return;
    };
    let Some(path) = crate::platform::file_dialog::save_shot(&name, None, None) else {
        log::info!("另存为：用户在对话框里取消");
        return;
    };
    match crate::platform::screenshot::save_png(&shot, &path) {
        Ok(()) => {
            log::info!("截图已另存为 {}", path.display());
            // 浮字报**用户挑的那个路径**（默认位置那句写的是 temp\...，两者必须分得清）。
            rt.borrow_mut()
                .show_notify(&format!("已另存为　{}", path.display()));
        }
        Err(e) => {
            log::warn!("另存为失败: {e}");
            rt.borrow_mut().show_notify("另存为失败");
        }
    }
}

/// 把选中的贴图另存为 PNG（Ctrl+S）。
///
/// 必须在 UI 线程、且经 `ctx.defer_blocking` 跑：原生保存对话框自带消息泵、会阻塞到
/// 松开为止，从窗口过程或帧回调里直接调就是重入（与「浏览…」选图同一条纪律）。
fn save_pin_as(rt: &Rc<RefCell<Runtime>>, seq: u32) {
    let Some(content) = rt.borrow().pin_content(seq) else {
        log::warn!("另存为：#{seq} 已不存在");
        return;
    };
    let Some(path) = crate::platform::file_dialog::save_png(&format!("CurSeen_贴图_{seq}"))
    else {
        log::info!("另存为：用户在对话框里取消");
        return;
    };
    let Some(shot) = crate::core::backdrop::bitmap_to_shot(content.bitmap()) else {
        log::warn!("另存为：位图无法转回帧格式");
        return;
    };
    match crate::platform::screenshot::save_png(&shot, &path) {
        Ok(()) => log::info!("贴图 #{seq} 已另存为 {}", path.display()),
        Err(e) => log::warn!("贴图 #{seq} 另存为失败: {e}"),
    }
}

/// 同步覆盖层的**鼠标穿透三态**（[`MouseMode`]）。
///
/// 判据（都在 `Runtime` 上，这里只做决策不重写逻辑）：
/// - [`GifRegion`](MouseMode::GifRegion)：`gif_interaction_enabled()` 且没有我们的
///   独立顶层窗压在上面。优先级最高——录屏选区框内要穿透操作被录应用，而 8 手柄与
///   边框带得收得到鼠标，只能靠 NCHITTEST 选点。
/// - [`Interactive`](MouseMode::Interactive)：`LayerSession::click_through` 判"关闭"
///   （标注 / 截图区域选择中），整窗收鼠标。
/// - [`Transparent`](MouseMode::Transparent)：其余一切，照旧置 `WS_EX_TRANSPARENT`。
///
/// 只在模式真正翻转时调一次 `ctx.set_click_through`（鼠标事件约 1000 条/秒，稳定态
/// 不碰平台 API）。进入 `GifRegion` 时补一次 `request_repaint`，保证这一帧边框已画出
/// （否则下一帧前用户看不到可拖的手柄）。
/// 三态决策的**纯函数**（可测）：给定 Runtime 状态，此刻覆盖层该是哪种鼠标模式。
///
/// 抽出来的理由：2026-10-01 的"长截图就绪态拖不动"正是决策埋在 `sync_mouse_mode`
/// （要 `EventCtx`，测不了）里漏掉的——回归锁 `mouse_mode_covers_every_draggable_stream`
/// 直接钉住"每一条可拖流都必须落在 `GifRegion`"。
fn mouse_mode_for(rt: &Runtime) -> MouseMode {
    // ★ 合并判据（GIF 选框交互 **或** 长截图就绪态选框）：只认"此刻有可拖选区挂着"。
    //   2026-10-01 实测翻车：只写 `gif_interaction_enabled()` 时，长截图就绪态落到
    //   `Transparent`，窗口带着 `WS_EX_TRANSPARENT`——OS 把鼠标直接给下层窗口，
    //   NCHITTEST 闭包**根本不会被问**，`zone_at_rt`/拖拽路由全成摆设（用户口径：
    //   "选框结束后无法调整"）。与 2026-09-20 GIF 的翻车是同一个病：门在窗口级穿透，
    //   不在事件路由。
    if rt.drag_interaction_enabled() {
        // ★ 不再叠 `!overlay_yields_to_our_windows()`。
        //   控制面板默认开机就开，若把"面板开着"挡在外面，NCHITTEST 永远
        //   收不到 → 手柄/边框悬停光标不变、也拖不动（2026-09-20 实测翻车）。框内/框外
        //   的穿透本来就由 `on_nchit` 钩子逐点回 HTTRANSPARENT 保证，面板开不开不影响
        //   "边框可拖 + 框内穿透"；录制中断由 `gif_interaction_enabled()` 里的
        //   `!gif_armed()` 到位。
        MouseMode::GifRegion
    } else if !rt.layer().click_through(rt.overlay_yields_to_our_windows()) {
        // `LayerSession::click_through` 为 false = 标注 / 截图区域选择正占着鼠标。
        MouseMode::Interactive
    } else {
        MouseMode::Transparent
    }
}

fn sync_mouse_mode(ctx: &mut EventCtx, rt: &Rc<RefCell<Runtime>>, last: &Cell<MouseMode>) {
    let mode = mouse_mode_for(&rt.borrow());
    if last.get() == mode {
        return;
    }
    last.set(mode);
    // `GifRegion` 与 `Interactive` 都清 `WS_EX_TRANSPARENT`，让 NCHITTEST 能收到鼠标
    // （`Transparent` 置回去）。
    ctx.set_click_through(mode == MouseMode::Transparent);
    if mode == MouseMode::GifRegion {
        // 清白自若靠 `on_nchit` 选点（见 2f）；这里只保证首帧把柄画出来。
        request_repaint();
    }
    log::info!(
        "覆盖层鼠标模式 → {:?}（{}）",
        mode,
        match mode {
            MouseMode::Transparent => "非叠层态或面板开着，鼠标照常操作下面的窗口",
            MouseMode::Interactive => "截图 / 标注中，鼠标归我们",
            MouseMode::GifRegion => "可拖选区交互（GIF 选框/长截图就绪）：边框可拖、框内穿透",
        }
    );
}

/// 按 `Runtime::gif_selection()` 同步 GIF 控件条窗口的生死。
///
/// # 为什么判据是"选框在不在"，而不是一条一次性的开窗请求
///
/// 控件条的**语义**就是"选框的附属物"：选框在，它才该在。用一次性请求会漏掉两种情形，
/// 两种都在 2026-09-20 的实测里出现过：
///
/// 1. 请求只在**有 UI 消息**时被读走，而确认选框那条路径（区域窗直接调
///    `Runtime::confirm_snap`）不产生 UI 消息 → 控件条有时不出现；
/// 2. `gif_sel` 被快捷键清掉时窗口不会跟着关 → 屏幕上留着一排**点了没反应**的按钮
///    （`toggle_gif_from_bar` 看到 `gif_sel` 为空就静默返回）。
///
/// 改成状态驱动之后，两个调用点（通道回调 + 250ms 对账）都只是"对账"，重复调用无副作用。
///
/// # 为什么关窗走 `WM_CLOSE` 而不是记 HWND
///
/// 窗口句柄只在 windui 的 UI 回调里可见，存不到 `Runtime` 里；`FindWindowW` 按标题查是
/// 这个项目里已经用了三处的既定做法（面板、贴图、提示窗）。
fn sync_gif_bar(
    ctx: &mut EventCtx,
    rt: &Rc<RefCell<Runtime>>,
    open_rec: &Cell<bool>,
    open_scroll: &Cell<bool>,
) {
    // ★ 两条流的控件条是**两个独立窗口**（2026-10-09 双条共存，用户要求 GIF 选框与
    //   长截图选框可以同时挂着、各自的按钮条都要在）：各对各的账，一条的状态不影响
    //   另一条。窗口的开/关由各模式自己的标题寻址（`bar_title`）。

    // ── 录制条（GIF / 选区 MP4） ──
    let want_rec = rt.borrow().gif_selection().is_some();
    if want_rec != open_rec.get() {
        open_rec.set(want_rec);
        if want_rec {
            // ★ 先 drop 借用再开窗：开窗会再 `borrow_mut()`（Rust 2021 的经典坑，
            //   本项目已在 `pin_req` / `take_rec_toast` 两处踩过并写成注释）。
            layer::gif_bar::open(ctx, rt, layer::gif_bar::BarMode::Rec);
        } else {
            log::info!("GIF 控件条收起");
            crate::platform::win_util::post_close_by_title(layer::gif_bar::BAR_TITLE);
        }
    }

    // ── 长截图条 ──
    let want_scroll = rt.borrow().scroll_bar_selection().is_some();
    if want_scroll != open_scroll.get() {
        open_scroll.set(want_scroll);
        if want_scroll {
            layer::gif_bar::open(ctx, rt, layer::gif_bar::BarMode::Scroll);
        } else {
            log::info!("长截图控件条收起");
            crate::platform::win_util::post_close_by_title(layer::gif_bar::SCROLL_BAR_TITLE);
        }
    }
}

/// 控件条**拖动/缩放后**跟随选框（位置 + 宽度随选框），录制条与长截图条各自独立。
///
/// 控件条打开时按 `bar_rect` 定一次位，但选框之后还能被 8 点/边框拖动缩放 ——
/// windui 没有"父窗口几何变了就同步子窗"的机制，所以这个对账周期性地检查选区，
/// 条不在正确位置/尺寸就 `SetWindowPos` 追过去。
///
/// ★ 幂等：`bar_rect` 不变时一次 API 都不调（`FindWindowW` 也省了）。只在选框移动/
///   缩放导致目标矩形变化时才发一次重定位。
pub fn sync_gif_bar_geometry(
    rt: &Rc<RefCell<Runtime>>,
    last_rec: &Cell<(i32, i32, i32, i32)>,
    last_scroll: &Cell<(i32, i32, i32, i32)>,
) {
    // ── 录制条 ──
    {
        let (target, vs) = {
            let r = rt.borrow();
            match r.gif_selection() {
                Some(sel) => (
                    Some(crate::runtime::layer::gif_bar::bar_rect(sel, r.screen)),
                    r.screen,
                ),
                None => (None, r.screen),
            }
        };
        sync_bar_geometry(target, vs, last_rec, layer::gif_bar::BAR_TITLE);
    }
    // ── 长截图条 ──
    {
        let (target, vs) = {
            let r = rt.borrow();
            let t = r
                .scroll_bar_selection()
                .map(|(sel, _)| {
                    crate::runtime::layer::gif_bar::bar_rect_for(
                        sel,
                        r.screen,
                        crate::runtime::layer::gif_bar::BarMode::Scroll,
                    )
                });
            (t, r.screen)
        };
        sync_bar_geometry(target, vs, last_scroll, layer::gif_bar::SCROLL_BAR_TITLE);
    }
}

/// [`sync_gif_bar_geometry`] 的单条实现（目标矩形 + 上次矩形 + 窗口标题）。
fn sync_bar_geometry(
    target: Option<(i32, i32, i32, i32)>,
    vs: crate::core::geometry::ScreenRect,
    last: &Cell<(i32, i32, i32, i32)>,
    title: &str,
) {
    let Some((x, y, w, h)) = target else {
        // 选框没了（条正在收起）：作废上次的矩形，下次重开第一拍必然对齐。
        last.set((i32::MAX, i32::MAX, i32::MAX, i32::MAX));
        return;
    };
    if last.get() == (x, y, w, h) {
        return;
    }
    last.set((x, y, w, h));
    log::info!(
        "控件条跟随选框（{title}）→ 位置 ({},{}) 尺寸 {}×{}，屏 {}×{}",
        x,
        y,
        w,
        h,
        vs.w,
        vs.h
    );
    crate::platform::win_util::set_window_rect_by_title(title, x, y, w, h);
}

/// 打开裁剪窗：GIF 走 [`layer::gif_trim`]，MP4 走 [`layer::mp4_trim`]，按来意决定初始状态。
///
/// | 来意 | 初始状态 | 动不动 GIF 选框 |
/// |---|---|---|
/// | [`TrimRequest::Last`]（控件条/提示窗「裁剪」） | 载入刚录的那份 | **收掉**（仅 GIF） |
/// | [`TrimRequest::Blank`]（裁剪快捷键） | 空窗，等「打开…」 | 不动 |
///
/// 「收掉选框」的理由：裁剪的语义是"**这段录制结束了**，接下来是编辑"。留着那条蓝/红边框
/// 既没有意义（已经不在录了），又会和居中的裁剪窗在视觉上打架。想重录的用户再按一次 GIF
/// 快捷键即可 —— 那本来就是一个全新的选框。
///
/// 而裁剪快捷键**跟"刚录的那段"毫无关系**，收掉边框只会让用户莫名其妙地丢掉刚框好的区域。
///
/// ★ `close_gif_bar()` 会再 `borrow_mut()`（它还要推消息），所以这次借用必须先结束 ——
///   与 `take_pin_request` / `take_rec_toast` 两处是同一个坑（Rust 2021 的 `if let`
///   临时量活到语句末尾）。
fn open_gif_trim(ctx: &mut EventCtx, rt: &Rc<RefCell<Runtime>>, what: TrimRequest) {
    // ★ 登记**必须挂在"窗真的开出来了"这一支上**。`last_gif` 读不出来（或那份文件坏掉）
    //   时 `open()` 返回 `false` 且**一个窗都没建** —— 那时登记就把状态置成了"开着"，
    //   于是覆盖层**永久**保持穿透、标注输入**永久**挂起（要等用户开一次再关一次裁剪窗
    //   才能恢复）。与 `close_gif_trim` 成对是这条纪律的全部内容。
    match what {
        TrimRequest::Blank(kind) => match kind {
            TrimKind::Gif => {
                layer::gif_trim::open_blank(ctx, rt);
                rt.borrow_mut().note_gif_trim_opened();
            }
            TrimKind::Mp4 => {
                // `open_blank` 可能不开窗（没录过只提示），所以按返回值登记。
                if layer::mp4_trim::open_blank(ctx, rt) {
                    rt.borrow_mut().note_gif_trim_opened();
                }
            }
        },
        TrimRequest::Last(kind) => match kind {
            TrimKind::Gif => {
                let Some(path) = rt.borrow().last_gif() else {
                    return;
                };
                rt.borrow_mut().close_gif_bar();
                if layer::gif_trim::open(ctx, rt, path) {
                    rt.borrow_mut().note_gif_trim_opened();
                } else {
                    rt.borrow_mut().show_notify("这个 GIF 打不开，无法裁剪");
                }
            }
            TrimKind::Mp4 => {
                let Some(path) = rt.borrow().last_mp4() else {
                    return;
                };
                // 与 GIF 同一条语义：从控件条剪刀进来 = "这段录完了，去编辑"，收掉选框。
                rt.borrow_mut().close_gif_bar();
                if layer::mp4_trim::open(ctx, rt, path) {
                    rt.borrow_mut().note_gif_trim_opened();
                } else {
                    rt.borrow_mut().show_notify("这个 MP4 打不开，无法裁剪");
                }
            }
        },
        TrimRequest::Path(kind, path) => match kind {
            // 「打开」对话框选出来的明确文件：直接开，不碰 `last_*` 也不动选框
            // （快捷键这条路跟"刚录的那段"毫无关系）。
            TrimKind::Gif => {
                if layer::gif_trim::open(ctx, rt, path) {
                    rt.borrow_mut().note_gif_trim_opened();
                } else {
                    rt.borrow_mut().show_notify("这个 GIF 打不开，无法裁剪");
                }
            }
            TrimKind::Mp4 => {
                if layer::mp4_trim::open(ctx, rt, path) {
                    rt.borrow_mut().note_gif_trim_opened();
                } else {
                    rt.borrow_mut().show_notify("这个 MP4 打不开，无法裁剪");
                }
            }
        },
    }
}

/// 文字输入窗的尺寸（逻辑像素）。
///
/// ★ 紧凑卡片（2026-09-26 两轮打磨）：第一轮 400×128 收窄到 360×92；第二轮
///   用户要求 Enter 换行 + 按钮-only 的退出方式（Esc 太容易连击误退整个画板），
///   输入区升为多行（高度 72 ≈ 三行）、底部一排「取消 / 确定」按钮。
///   ★ 高度按内容实算：padding 12×2 + 输入 72 + spacing 8 + 按钮行 ≈32 = 136，
///     取 138（±2 余量）。写大了余量全堆在底部——上一版 156 时按钮离下边
///     多出一条明显的空带，就是这个原因。
const TEXT_BOX_W: i32 = 360;
const TEXT_BOX_H: i32 = 138;

/// 文字输入窗的落点：**落笔点的右下方**，再钳进虚拟桌面内。
///
/// 不做屏幕居中：双屏合计 3840 宽时屏幕中心离用户刚点的那一点可能有一整屏远，
/// 输入框"从别处冒出来"会让人找不到。落在手边才跟得上手指。
///
/// 单位为物理像素，与 `Window::position` 同一坐标系（虚拟桌面，副屏在左时 x 为负）。
fn text_box_pos(anchor: Point, screen: ScreenRect, w: i32, h: i32) -> (i32, i32) {
    // 落点右下偏一点：不要压住用户刚点的地方，也避开光标本身。
    let x = anchor.x as i32 + 12;
    let y = anchor.y as i32 + 12;

    // 右/下越界时回退到落点的左/上方（`screen` 是整块虚拟桌面，含负原点）。
    let max_x = screen.x.saturating_add(screen.w).saturating_sub(w);
    let max_y = screen.y.saturating_add(screen.h).saturating_sub(h);
    let x = x.min(max_x).max(screen.x);
    let y = y.min(max_y).max(screen.y);
    (x, y)
}

/// 文字工具的落点已定 → 开一个小的输入窗。
///
/// 边沿触发：`take_text_request` 取过一次就不再给，所以这里可以挂在每条 UI 消息之后。
///
/// 必须是**真窗口**而不是画在覆盖层里：覆盖层是 `WS_EX_TOOLWINDOW` 的穿透窗，拿不到键盘
/// 焦点，也就没有 IME——中文标注就写不出来。Python 1.0.0 用的是同一个思路
/// （`QInputDialog.getText`）。
fn open_text_input(ctx: &mut EventCtx, rt: &Rc<RefCell<Runtime>>) {
    // ★ 单实例硬门（2026-09-26 用户口径："文本框只能出现 1 个，多次点击无效"）。
    //   挂起守卫（`DrawingModule::suspended`）在正常时序下已经拦住了窗口开着期间的
    //   点击，但它是"每帧开头才对账"的软门——消息批处理、窗口开关与帧交界的竞态
    //   里仍可能漏进一条落笔请求。这里在 UI 层再立一道**硬门**：窗口还开着，任何
    //   新请求（新落笔 / 再编辑）一律取走即弃，绝不重入 `open_window`。两道门
    //   判据同源（都读 `text_window_open`），谁先到谁生效，行为一致。
    if rt.borrow().text_window_open() {
        // 先 take 再判（两次 borrow_mut 不能写在同一条件里：RefCell 重入，
        // 见本文件 `pin_req` 处的同一条纪律）。
        let stale_new = rt.borrow_mut().drawing.take_text_request();
        let stale_edit = rt.borrow_mut().drawing.take_text_edit_request();
        if stale_new.is_some() || stale_edit.is_some() {
            log::info!("文字输入窗已开着：丢弃迟到的文字请求（多次点击无效）");
        }
        return;
    }

    // ★ 两种请求先到先得：新文字（落笔锚点）或再编辑（条目下标 + 旧文字预填）。
    //    再编辑由双击已有文字触发（见 `DrawingController::begin_stroke` 的 Text 分支）。
    let new_anchor = rt.borrow_mut().drawing.take_text_request();
    let edit = if new_anchor.is_none() {
        rt.borrow_mut().drawing.take_text_edit_request()
    } else {
        None
    };
    let Some((anchor, initial, editing)) = (match (new_anchor, edit) {
        (Some(a), _) => Some((a, String::new(), None)),
        (None, Some(e)) => Some((e.anchor, e.text, Some(e.idx))),
        (None, None) => None,
    }) else {
        return;
    };

    // 位置用**物理像素**：`Window::position` 与 `take_text_request` 的世界坐标同一坐标系
    // （都是虚拟桌面物理像素），这里不需要经过 DPI 换算。
    let screen = rt.borrow().screen;
    let (bx, by) = text_box_pos(anchor, screen, TEXT_BOX_W, TEXT_BOX_H);
    log::info!(
        "文字工具落点在 ({:.0},{:.0})，打开输入窗 @ ({bx},{by}) {}",
        anchor.x,
        anchor.y,
        if editing.is_some() {
            "（再编辑）"
        } else {
            "（新文字）"
        }
    );

    let input = signal(initial);
    let rt_ok = rt.clone();
    let rt_close = rt.clone();

    // 先登记"输入窗开着"：标注输入随即挂起（键鼠一起），点按钮不会多出一笔。
    rt.borrow_mut().set_text_window_open(true);

    let editing_ok = editing;
    ctx.open_window(
        Window::new("标注文字", TEXT_BOX_W, TEXT_BOX_H)
            .single("drawing_text")
            .owned(true)
            .resizable(false)
            // ★ 无边框：这只是一个"落笔点旁边的小输入条"，系统标题栏（占 30+px 高、
            //   与面板风格完全脱节）是用户说"丑"的主要来源。关窗出口 = 底部两个按钮，
            //   不依赖标题栏的 X。
            .frameless(true)
            // ★★ **吞掉 Esc**（2026-09-26 用户口径："输入框不能用 Esc 退出"）：
            //   windui 的 Esc 兜底（未消费的 Escape → 关窗）会把这个输入窗关掉，
            //   而用户手快连按两下 Esc 时，第一下关窗、第二下就落到标注层——
            //   `text_window_open` 已翻回 false，**整个画板被退出**。把 Esc 在窗口级
            //   消费掉（on_shortcut 先于 Esc 兜底，返回 true = 已处理），输入窗对
            //   Esc 彻底无反应；退出只走「取消 / 确定」两个按钮。钩子侧仍会看到
            //   Esc，但标注层有 `!text_window_open` 守卫，窗口开着时它不认 Esc。
            .on_shortcut(|_ctx, ev| ev.key == windui::event::Key::Escape)
            .position(bx, by)
            .bg(to_color(crate::ui::tokens::BG))
            .on_close_request(move |_ctx| {
                // 关窗 = 这次文字作废（空串提交、Esc、点标题栏 X 都走这里）。已经落过笔的
                // 那条路径在 `commit_text` 里已经把待落点取走了，这里的 `cancel_text`
                // 于是是空操作——不必分辨是哪条路。再编辑同理：cancel 不改原文。
                rt_close.borrow_mut().set_text_window_open(false);
                rt_close.borrow_mut().drawing.controller_mut().cancel_text();
                rt_close
                    .borrow_mut()
                    .drawing
                    .controller_mut()
                    .cancel_text_edit();
                // 立刻让覆盖层重画一次——落笔要**当场看见**那段文字（否则要等用户下次动
                // 鼠标，而此刻他手在键盘上），取消要把插入符/高亮框抹掉。两条路都需要它。
                request_repaint();
                true
            })
            .content(move || {
                let is_edit = editing_ok.is_some();
                Element::col()
                    .fill()
                    .padding(12)
                    .spacing(8)
                    .child(
                        // ★ 多行：Enter 插入换行（windui multiline 语义），不再"回车即提交"
                        //   ——提交只走下面的「确定」按钮，换行与提交从此不打架。
                        // ★ 输入框必须显式撑满：windui 的 TextInput::measure 恒返回
                        //   固定宽 160px（inputs.rs:1639），不继承父容器——不加
                        //   width_match 它就是窄窄一条贴在左上，窗口右侧全空。
                        Element::text_input(
                            input,
                            if is_edit {
                                "修改文字"
                            } else {
                                "输入文字"
                            },
                        )
                        .multiline()
                        .width_match()
                        .height(72)
                        .autofocus_select_all()
                        .font_size(crate::ui::tokens::FONT_MD),
                    )
                    .child(
                        // ★ 按钮行底部靠右打包（`.align(End)` 把整行钉到行尾），
                        //   行内「确定」在左、「取消」在右（2026-09-26 用户口径）。
                        Element::row()
                            .spacing(8)
                            .align(windui::prelude::Align::End)
                            .child(Element::button("确定").on_click(move |ctx| {
                                // 空串由 `commit_text` / `commit_text_edit` 自己判为取消
                                // （返回 false，不留痕迹 / 保留原文）。
                                let mut ok = rt_ok.borrow_mut();
                                if let Some(idx) = editing_ok {
                                    ok.drawing
                                        .controller_mut()
                                        .commit_text_edit(idx, &input.get());
                                } else {
                                    ok.drawing.controller_mut().commit_text(&input.get());
                                }
                                drop(ok);
                                ctx.request_close();
                            }))
                            .child(Element::button("取消").neutral().on_click(|ctx| {
                                // 关窗即可：`on_close_request` 统一做取消收尾。
                                ctx.request_close();
                            })),
                    )
            }),
    );
}

/// 这次按键是否命中面板快捷键。
///
/// ★ 长按自动重复（`repeated=true`）不算命中：面板快捷键是 **toggle**，按住不放会
///   被系统连发成几十条 `KeyDown`，那等于面板疯狂开合。与截图 / 标注等 toggle 键
///   同一纪律（见 `Runtime::is_action_hotkey`）。
fn panel_hotkey_hit(rt: &Rc<RefCell<Runtime>>, ev: &InputEvent) -> bool {
    let InputEvent::KeyDown {
        key,
        mods,
        repeated,
        ..
    } = ev
    else {
        return false;
    };
    if *repeated {
        return false;
    }
    let spec = hotkeys::panel_hotkey(&rt.borrow().config).to_string();
    !spec.is_empty() && hotkeys::matches(&spec, *key, *mods)
}

/// 面板快捷键的动作：**关着就开、开着就收**（见调用点的说明）。
fn toggle_panel(ctx: &mut EventCtx, rt: &Rc<RefCell<Runtime>>) {
    if rt.borrow().panel_open() {
        log::info!("面板快捷键：收起控制面板");
        crate::platform::win_util::post_close_by_title(crate::ui::panel::TITLE);
    } else {
        open_panel(ctx, rt);
    }
}

/// 把钩子线程的事件搬到 UI 通道。
///
/// 钩子线程只负责采集，不能直接碰 `Runtime`（不是 `Send`），所以这里只做转发，
/// 真正的状态变更发生在上面的通道回调里。
///
/// ★ 输入门（补丁 012）：**鼠标事件只在有模块消费它时才转发**。每次 `ui_tx.send` 都会
/// 唤醒 UI 线程一帧（`PostMessageW(WM_APP_WAKE)`），低频钩子实测鼠标移动约 1000 条/秒，
/// 照单全转就是 1000 帧/秒的唤醒——这台机器上"什么都没开"也照样吃满一个核的来源。
/// 1.0 线本来有这道门（`INPUT_NEEDS_FRAME`），2.0 重写时丢了。
///
/// 键事件不筛：面板快捷键（`Ctrl+Shift+D`）与击键回显都要它，且按键速率是人的手速，
/// 量级完全不同。
fn spawn_input_bridge(
    input_rx: Receiver<InputEvent>,
    ui_tx: Sender<UiMsg>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while let Ok(ev) = input_rx.recv() {
            // ★ 滚轮走自己的门：放大镜开着时（`wheel_swallow` 与钩子的吞事件开关在
            //   `sync_input_gate` 里同步维护）Ctrl+滚轮要送到 UI 线程做快捷调倍率，
            //   不受"是否需要鼠标"门管辖——否则只开放大镜时滚轮事件全被丢在这里。
            let gate = match ev {
                // 与钩子同源：钩子只在「放大镜在屏 + Ctrl+Win+滚轮」时才转发滚轮，
                // `zoom_wheel_armed` 就是这个判断；回显武装时普通/倾斜滚轮的镜像也放行
                // （击键回显显示「滚轮上/下/左/右」）。其余滚轮不需要进 UI，照常丢弃。
                InputEvent::MouseWheel { .. } => {
                    crate::platform::input::mouse::zoom_wheel_armed()
                        || crate::platform::input::mouse::mouse_echo_armed()
                }
                // ★ 按下/抬起是**低频**事件（一次点击最多两条），"放行"的代价可以忽略，
                //   所以这里比 `_` 分支多认两道门：
                //   · 鼠标回显武装时放行（2026-09-22 补）：只开「击键回显」不开拖尾/绘制
                //     等其它鼠标功能时 MOUSE_INPUT_NEEDED 是 false —— 不放行的话点击
                //     永远进不了回显。
                //   · ★ 虚拟键盘的鼠标门（2026-09-26 补）：方案里含鼠标键帽时也要放行，
                //     且**不看「鼠标穿透」** —— 穿透只决定"点键帽注不注入按键"，
                //     而"点亮键帽"唯一的输入就是这两条事件。WASD 游戏板出厂勾着穿透，
                //     旧判据把它的鼠标事件整条丢了，表现是"鼠标键点了没反应"。
                InputEvent::MouseDown { .. } | InputEvent::MouseUp { .. } => {
                    MOUSE_INPUT_NEEDED.load(Ordering::Relaxed)
                        || crate::platform::input::mouse::mouse_echo_armed()
                        || VK_MOUSE_ARMED.load(Ordering::Relaxed)
                }
                _ => MOUSE_INPUT_NEEDED.load(Ordering::Relaxed),
            };
            if is_mouse(&ev) && !gate {
                if matches!(ev, InputEvent::MouseDown { .. }) {
                    // ★ `debug` 而不是 `info`：覆盖层常态下就不收鼠标（此时鼠标属于
                    //   下面的窗口），这条每点一次记一行，实测占全天日志的 1/3
                    //   （930/2562 行）——它只在排查"点击没反应"时才有意义。
                    //   需要时用 `CURSEEN_LOG_LEVEL=debug` 打开（见 `core::logger`）。
                    log::debug!("桥接线程：丢弃 MouseDown（MOUSE_INPUT_NEEDED=false）");
                }
                continue;
            }
            if ui_tx.send(UiMsg::Input(ev)).is_err() {
                break;
            }
        }
    })
}

/// 该事件是否来自鼠标（见 [`spawn_input_bridge`] 的输入门）。
fn is_mouse(ev: &InputEvent) -> bool {
    matches!(
        ev,
        InputEvent::MouseMove { .. }
            | InputEvent::MouseDown { .. }
            | InputEvent::MouseUp { .. }
            | InputEvent::MouseWheel { .. }
    )
}

/// 覆盖层画布组件。
///
/// ★ GIF 选框边框的厚度与 8 个手柄的边长**不再是本地常量**：它们同时是命中几何的输入，
///   已提到 [`crate::core::gif_zone`]（`FRAME_LOGICAL` / `HANDLE_LOGICAL`）。
///   绘制端（本文件，逻辑 dp）与命中端（`hit_zone_at`，物理像素）各存一份字面量，
///   改一处忘另一处就会变成"画了却点不到"或反之 —— 那是最贵的一类交互 bug。
/// GIF 选框边框：一圈画在**选框外侧**的描边。
///
/// ★ 为什么必须在外侧：采集链是"DXGI 抓整块显示器 → 裁到选框矩形"（见
///   `platform::recorder::RecOptions::crop`），压在框内的哪怕 1px 都会被自己录进每一帧
///   ——产物里就挂着一圈框。往外推则整条边都落在裁剪范围之外，天然不进画面。
///
/// ★ 为什么是四条边而不是 `stroke_rect`：Canvas 的描边以路径为中心线**压住内外各半**，
///   外侧那半会落回框内（那就白算了）。四条整数像素的填充矩形位置完全可控，与
///   `pin::stroke_ring` 同一个道理，只是它往内缩、这里往外撑。
/// 选框边框/手柄的**语义色**（同一个边框函数，三种状态）。
///
/// ★ 为什么需要第三档：截图**编辑态**与 GIF 选框在几何上完全同源（都是"外扩一整条边的
///   边框 + 8 个手柄"，共用 `core::gif_zone::hit_zone_at`），但语义是反的 —— GIF 选框是
///   "还在选，拖完就开录"，编辑态是"选区已定，拖只是微调，Enter 才出图"。给编辑态换个色，
///   用户一眼能分清"我现在在微调截图"，不会误以为按下去就开始录了。
///
/// 录制流（GIF 选框）与滚动流（长/宽截图）的选框**可以同屏共存**（2026-10-09 双流共存，
/// 各画各的、各用各的色，见 `paint` 的并行绘制段）；截图**编辑态**是模态全屏，与这两档
/// 仍然互斥。所以同一屏可能同时出现蓝框+红框（GIF 录制中 + 长截图就绪）这类组合——
/// 颜色按选框归属分流，互不串色。
#[derive(Clone, Copy, PartialEq, Eq)]
enum FrameAccent {
    /// 截图 / GIF 的**选定态**：主色蓝。与压暗遮罩、放大镜边框同源，"这是截图界面的部件"。
    Select,
    /// GIF **录制中**：红。这是"正在录"的唯一视觉信号（控件条那颗按钮的文字是固定的）。
    Recording,
    /// 截图**编辑态**：琥珀。见上文枚举文档。
    Editing,
    /// 长截图**采集中**：紫。采集期间区域固定不可拖（没有手柄交互），这一圈是
    /// "正在捕获这块"的唯一视觉信号；快捷键 / Esc 的反馈走中央浮字，不走颜色。
    Scroll,
}

impl FrameAccent {
    fn rgb(self) -> (u8, u8, u8) {
        match self {
            Self::Select => (66, 133, 244),
            Self::Recording => (226, 68, 60),
            Self::Editing => (245, 158, 11),
            Self::Scroll => (138, 92, 246),
        }
    }
}

fn draw_gif_frame(
    canvas: &mut dyn Canvas,
    sel: ScreenRect,
    map: &crate::runtime::layer::WorldMap,
    accent: FrameAccent,
    with_handles: bool,
) {
    let t = FRAME_LOGICAL;
    let (x, y) = map.to_window(sel.x as f32, sel.y as f32);
    let w = map.len_to_window(sel.w as f32);
    let h = map.len_to_window(sel.h as f32);
    // 往外撑一整条边的厚度：结果矩形的四条边都落在选框之外。
    let (x, y, w, h) = (x - t, y - t, w + 2.0 * t, h + 2.0 * t);
    let c = accent.rgb();
    let paint = WPaint::fill(Color::rgba(c.0, c.1, c.2, 255));
    for r in [
        (x, y, w, t),
        (x, y + h - t, w, t),
        (x, y, t, h),
        (x + w - t, y, t, h),
    ] {
        canvas.fill_rect(r.0, r.1, r.2, r.3, &paint);
    }
    // 8 个手柄：四角 + 四边中点，各自一个同色小方块。中心落在同一外框 `(x,y,w,h)`
    // 的四个角与四条边中点——与 `core::gif_zone::hit_gif_zone` 的锚点是同一套几何，
    // 保证"画了对点能点中"。
    // ★ **采集 / 录制态一律不画手柄**（`with_handles` 由各调用方按交互门给）：
    //   手柄方块中心压在外框线上，有 `hp/2 - t` 那半截伸进选区内部，会被拍进每一帧——
    //   长截图 2026-10-01 实测（BitBlt，页面滚动时画布上每隔约一帧高一串色块），
    //   GIF / 选区 MP4 2026-10-08 实测（DXGI，产物四周挂着一圈录中色的手柄块）。
    //   采集态区域固定、交互门关闭，手柄没有存在意义，整个省掉。
    if with_handles {
        let hp = HANDLE_LOGICAL;
        let (cx, cy) = (x + w / 2.0, y + h / 2.0);
        let hs = hp / 2.0;
        let handles = [
            (x - hs, y - hs),
            (x + w - hs, y - hs),
            (x - hs, y + h - hs),
            (x + w - hs, y + h - hs),
            (cx - hs, y - hs),
            (cx - hs, y + h - hs),
            (x - hs, cy - hs),
            (x + w - hs, cy - hs),
        ];
        for (hx, hy) in handles {
            canvas.fill_rect(hx, hy, hp, hp, &paint);
        }
    }
}

struct OverlayCanvas {
    runtime: Rc<RefCell<Runtime>>,
    scale: f32,
    dx: f32,
    dy: f32,
    /// 上次见到的 windui 后备缓冲代数（**补丁 032**）。代数变了 = 缓冲被重建/释放
    /// （休眠、resize），上一帧内容不复存在 → 聚光灯增量缓存作废，整窗重画。
    buf_gen: std::cell::Cell<u32>,
    /// GIF 选框拖拽进行态（`Option :: 选中了哪个区 + 上一次的**世界物理**鼠标点）。
    /// 用于跨帧累加位移（见 `Runtime::gif_resize_or_move` 对增量语义的要求）。
    gif_drag: std::cell::Cell<Option<GifDrag>>,
}

/// GIF 选区一次拖拽的快照：命中的交互区 + 上一帧鼠标的**世界物理**坐标。
#[derive(Clone, Copy)]
struct GifDrag {
    zone: HitZone,
    /// 按下那一刻命中的是**哪条流**的选框（见 [`zone_with_target_at`]）。
    ///
    /// ★ 双流共存（2026-10-09）后路由不能再按"哪条流挂着"猜——两条流的选框可以同时
    ///   挂着，按流存在性分派会把 GIF 框的拖拽套到长截图框头上。按下时记下归属，
    ///   移动全程跟着这一档走。`None` = 编辑态档（它的选区拖拽走全局钩子
    ///   `Runtime::edit_drag`，窗口这条路径只抢捕获不动选区，与旧版行为一致）。
    target: Option<DragTarget>,
    last_x: i32,
    last_y: i32,
}

/// 窗口事件路径上，一次选框拖拽归属的流。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DragTarget {
    /// 录制流（GIF / 选区 MP4，`gif_sel`）。
    Gif,
    /// 滚动流（长/宽截图就绪框，`scroll_ready`）。
    Scroll,
}

impl OverlayCanvas {
    fn new(runtime: Rc<RefCell<Runtime>>, scale: f32, dx: f32, dy: f32) -> Self {
        Self {
            runtime,
            scale,
            dx,
            dy,
            buf_gen: std::cell::Cell::new(windui::platform::overlay_buffer_generation()),
            gif_drag: std::cell::Cell::new(None),
        }
    }

    /// 世界坐标矩形 → 覆盖层设备像素矩形（与 `render::View` 同一套映射），
    /// 并向四周扩 1px 吸收取整误差（抗锯齿的 2px 外扩已在 Runtime 侧做掉）。
    fn to_device(&self, r: crate::core::geometry::Rect) -> (i32, i32, i32, i32) {
        let x0 = (r.x * self.scale + self.dx).floor() as i32 - 1;
        let y0 = (r.y * self.scale + self.dy).floor() as i32 - 1;
        let x1 = ((r.x + r.w) * self.scale + self.dx).ceil() as i32 + 1;
        let y1 = ((r.y + r.h) * self.scale + self.dy).ceil() as i32 + 1;
        (x0, y0, x1 - x0, y1 - y0)
    }

    /// 命中 GIF 区划中的哪个交互带：把世界坐标点换算到[选区 + 手柄几何]的同一刻度，
    /// 交给纯函数 `hit_gif_zone`。若当前无交互选区返回 `None`。
    fn gif_zone_at(&self, wx: i32, wy: i32) -> HitZone {
        zone_at_rt(&self.runtime, wx, wy)
    }

    /// GIF 选区**调整**阶段的实时放大镜同步。**每帧由 `paint` 调一次**（见那里的长注释：
    /// 为什么不能用鼠标事件驱动）。
    ///
    /// - `Some((wx, wy))`：光标的世界物理位置（本帧 `live_cursor`）。落在**可调整带**
    ///   （边框带 / 4 角 / 4 边中点）上就显示放大镜，否则收起。
    /// - `None`：交互前提已经没了（选区被清掉 / 开始录制 / 退出交互态）→ 无条件收起。
    ///
    /// ★ 判据直接复用 [`Self::gif_zone_at`]，因为它与"鼠标往哪走"是同一套几何：
    ///   它返回 `HitZone::None` 的两个场合（**选区内部**与**选区外部**）正是要**穿透**给
    ///   被录应用的位置，那里放一块取景器既没用也碍事；而 `Some(_)` 恰好就是用户正在
    ///   对位的地方 —— 这就是"精细调整需要放大镜"的全部范围。
    fn sync_gif_magnifier(&self, at: Option<(i32, i32)>) {
        let zone = at.map_or(HitZone::None, |(wx, wy)| self.gif_zone_at(wx, wy));
        let mut rt = self.runtime.borrow_mut();
        // 录制中 / 倒计时中 / 无选区 / 长截图采集中：`gif_zone_at` 已经给了 `None`，
        // 这里再挡一层合并交互门（GIF 交互 或 长截图就绪），保证"不可交互"与
        // "不显示放大镜"永远同进同退。
        if zone == HitZone::None || !rt.drag_interaction_enabled() {
            rt.region_magnifier.hide();
            return;
        }
        let screen = rt.screen;
        if let Some((wx, wy)) = at {
            rt.region_magnifier
                .update_live((wx as f32, wy as f32), screen);
        }
    }

    /// 截图**编辑态**的放大镜同步（每帧由 `paint` 调一次，与 [`Self::sync_gif_magnifier`]
    /// 同构；差别只在**源**）。
    ///
    /// ★ 源必须是**冻结帧**（`shot_edit_frame()`），不能用 `update_live` 现抓：编辑态下
    ///   屏上那块已经被我们自己的画布盖住了 —— 冻结帧在底下、笔迹在上面，实时抓屏抓到的是
    ///   "我刚画的那一笔放大 4 倍"，越看越糊。区域选择阶段用冻结帧的理由（"看清边界压在哪个
    ///   源像素上"）在这里原样成立。
    ///
    /// ★ 显隐判据与 GIF 那条一字不差：光标落在**交互带**（边框带 / 4 角 / 4 边中点）上才
    ///   显示。编辑态整屏都是我们的窗（`on_nchit` 回 `Client`），所以"框内/框外"两点在本
    ///   函数据说 `HitZone::None` —— 那里是画笔的地盘，放一块取景器只会挡着落笔。
    fn sync_edit_magnifier(&self, at: Option<(i32, i32)>) {
        let zone = at.map_or(HitZone::None, |(wx, wy)| {
            self.runtime.borrow().shot_edit_zone_at(wx, wy)
        });
        let mut rt = self.runtime.borrow_mut();
        if zone == HitZone::None || !rt.shot_edit_active() {
            // 与调用点的 `is_shown()` 门同理：`hide()` 会真去 `ShowWindow(SW_HIDE)`，
            // 而编辑态下"光标在框内"是常态（用户大多时间在画画）→ 不加门就是每帧一次白付。
            if rt.region_magnifier.is_shown() {
                rt.region_magnifier.hide();
            }
            return;
        }
        let Some(frame) = rt.shot_edit_frame() else {
            rt.region_magnifier.hide();
            return;
        };
        let screen = rt.screen;
        if let Some((wx, wy)) = at {
            rt.region_magnifier
                .update(&frame, (wx as f32, wy as f32), screen);
        }
    }

    /// ★ 数位板直通：把一条**笔**指针事件换算成世界坐标喂进输入队列（与钩子同一
    /// 条路，绘制/拖尾/截图编辑等消费方无感知）。返回 true = 已消费。
    ///
    /// 坐标换算与 GIF 拖拽那条完全同源：`pe.pos` 是窗口本地，经 `WorldMap` 回世界
    /// 物理。`PEN_ACTIVE`/`PEN_LAST_MS` 在这里维护，钩子线程据此丢弃翻译层补发的
    /// 迟到鼠标消息（见 `runtime::PEN_ACTIVE` 的说明）。
    fn pen_drive_drawing(&mut self, ctx: &mut EventCtx, pe: &windui::event::PointerEvent) -> bool {
        let (wx, wy, armed) = {
            let rt = self.runtime.borrow();
            let map = crate::runtime::layer::WorldMap::new(rt.screen);
            let (fx, fy) = map.to_world(pe.pos.x as f32, pe.pos.y as f32);
            // ★ 「标注开没开」用 `DrawMode` 判（`Off` 之外都算在用）。**不能**用
            //   `is_drawing()`：后者在"笔画已在进行"时才为真，笔尖刚接触时必然是
            //   false——拿它当门，笔就永远起不了笔（见调用点的注释）。
            let armed = !matches!(
                rt.drawing.controller().mode(),
                crate::modules::drawing::controller::DrawMode::Off
            );
            (fx.round() as i32, fy.round() as i32, armed)
        };
        if !armed {
            return false;
        }
        let now = crate::platform::win_util::tick_ms();
        let ord = std::sync::atomic::Ordering::Relaxed;
        // windui 的事件按钮类型与本项目 `core::events` 的不是同一个，按下/抬起只认
        // 笔尖（windui 补丁 093 把笔尖映成 Left）。
        let tip = pe.button == windui::event::MouseButton::Left;
        // ★ 投递走与鼠标钩子**同一个** mpsc 队列，绝不在事件回调里 `borrow_mut()`：
        //   本回调是在 `Runtime` 已被借用期间被调的，再借一次就是 `RefCell` 重入
        //   panic，而 `panic = "abort"` 下它直接把进程打挂 —— 2026-09-29 实测：一次
        //   笔点按 → "RefCell already mutably borrowed" → 程序毫无征兆地没了。
        let send = |ev: crate::core::events::InputEvent| {
            if let Some(tx) = crate::platform::input::hook::HOOK_TX.get() {
                let _ = tx.send(ev);
            }
        };
        match pe.kind {
            PointerKind::Down if tip => {
                crate::runtime::PEN_ACTIVE.store(true, ord);
                crate::runtime::PEN_LAST_MS.store(now, ord);
                // ★ 诊断：一条笔画一行。真机验证"起笔还有没有延迟"就看这行的时间戳
                //   比同一次点按的鼠标钩子日志早多少——早 = 直通生效；没有这行 =
                //   `WM_POINTER` 根本没到本窗（那就要去查窗口穿透样式）。
                log::info!("笔直通: Down @ ({wx},{wy})");
                // 捕获：后续 Move/Up 即使笔尖滑出窗口也要继续送到本窗，否则一笔断线。
                ctx.capture();
                send(InputEvent::MouseDown {
                    button: crate::core::events::MouseButton::Left,
                    x: wx,
                    y: wy,
                    time_ms: now,
                });
            }
            // ★ 悬停（笔尖没接触）不走这条：`PEN_ACTIVE` 为 false 时钩子照常投递，
            //   两条路同时喂会把一笔画两遍。
            PointerKind::Move if crate::runtime::PEN_ACTIVE.load(ord) => {
                crate::runtime::PEN_LAST_MS.store(now, ord);
                send(InputEvent::MouseMove {
                    x: wx,
                    y: wy,
                    time_ms: now,
                });
            }
            PointerKind::Up if tip => {
                crate::runtime::PEN_LAST_MS.store(now, ord);
                send(InputEvent::MouseUp {
                    button: crate::core::events::MouseButton::Left,
                    x: wx,
                    y: wy,
                    time_ms: now,
                });
                crate::runtime::PEN_ACTIVE.store(false, ord);
                // 必须显式还捕获，否则覆盖层永久持有 OS 捕获并吞掉后续全局操作。
                ctx.release_capture();
            }
            _ => return false,
        }
        request_repaint();
        true
    }
}

/// 命中**当前可拖选区**的交互带（模块级，供覆盖层根画布与两个区域级钩子共用）。
///
/// 「当前可拖选区」有三档。**前两档可以同时挂着**（2026-10-09 双流共存，用户口径
/// "长截图、宽截图、gif 录制、区域录屏……要允许多个同时存在"）：
/// - GIF 选框（`gif_selection()`）——注意这里**不**再叠 `gif_interaction_enabled()`：
///   录制中边框仍归我们（框内穿透给被录应用，边框没必要跟着穿透），拖拽本身另有闸门
///   （`OverlayCanvas::on_event` 的交互门早退）。
/// - 长截图**就绪态**选框（`scroll_ready()`）——手柄/边框带可拖。**只认就绪**：采集中的
///   边框不是交互物（覆盖层必须全穿透，用户要在框边附近操作目标页面），手柄也不画。
/// - 截图**编辑态**的选区（`shot_edit_selection()`）——这一档要的是"边框带能拖、框内给画笔"。
///
/// 三档共用 `core::gif_zone` 那一套几何与尺寸（`hit_zone_at`），所以"画出来的手柄"与
/// "拖得到的位置"在任何一档里都同源。`dpi` 取自 `WorldMap`（世界物理 ↔ 窗口逻辑的唯一
/// 换算器），逻辑 → 物理的换算在 `hit_zone_at` 内部做。
///
/// ★ 命中是**逐档查询**而非选一档：两框不重叠时各自拖得动；重叠处按 GIF → 长截图 →
///   编辑态的次序取先命中者（与绘制次序一致，"先画的先接"）。编辑态是模态全屏，
///   与前两档天然互斥，排最后只是兜底。
fn zone_at_rt(rt: &Rc<RefCell<Runtime>>, wx: i32, wy: i32) -> HitZone {
    zone_with_target_at(rt, wx, wy).0
}

/// [`zone_at_rt`] 的归属感知版：命中之外还答"这块交互带属于**哪条流**"。
///
/// 为什么需要归属：双流共存（2026-10-09）后，GIF 选框与长截图就绪框可以同时挂着，
/// 拖拽路由必须按**按下那一刻命中的那一档**走，而不能按"哪条流挂着"猜——后者在两流
/// 同挂时会把 GIF 框的拖拽套到长截图框头上。编辑态档返回 `None`（它的选区拖拽走全局
/// 钩子 `Runtime::edit_drag`，窗口这条路径不负责动它的选区）。
fn zone_with_target_at(
    rt: &Rc<RefCell<Runtime>>,
    wx: i32,
    wy: i32,
) -> (HitZone, Option<DragTarget>) {
    let dpi = {
        let rt = rt.borrow();
        crate::runtime::layer::WorldMap::new(rt.screen).dpi()
    };
    {
        let rt = rt.borrow();
        if let Some(s) = rt.gif_selection() {
            let z = hit_zone_at(s, wx, wy, dpi);
            if z != HitZone::None {
                return (z, Some(DragTarget::Gif));
            }
        }
        if let Some(s) = rt.scroll_ready() {
            let z = hit_zone_at(s, wx, wy, dpi);
            if z != HitZone::None {
                return (z, Some(DragTarget::Scroll));
            }
        }
        if let Some(s) = rt.shot_edit_selection() {
            let z = hit_zone_at(s, wx, wy, dpi);
            if z != HitZone::None {
                return (z, None);
            }
        }
    }
    (HitZone::None, None)
}

impl Widget for OverlayCanvas {
    fn measure(&self, avail: Size, _style: &Style, _text: &mut dyn TextEngine) -> Size {
        avail
    }

    // ★ 右键必须收（2026-10-08 用户实测"右键无法退出选区"）：windui 对非左键按下有
    //   节点级过滤（`wants_right_click()` 默认 false 时事件不进 `on_event`）。GIF / 长
    //   截图就绪态的选框调整阶段（`GifRegion` 模式下边框与手柄带是仅有的可点区），
    //   右键 = 退出调整态。其它模式收到右键也不动作（见 on_event 的分支门），
    //   行为与从前一致——标注 / 编辑态的右键语义仍留给画笔。
    fn wants_right_click(&self) -> bool {
        true
    }

    fn paint(
        &self,
        bounds: Rect,
        _content: Rect,
        _focused: bool,
        _enabled: bool,
        canvas: &mut dyn Canvas,
        _style: &Style,
    ) {
        // ★ 补丁 032 修订：这里**不能**无条件清屏——增量帧（聚光灯挪洞）依赖
        // "缓冲里保留着上一帧内容"，整屏清掉再只画脏区＝整屏透明，特效全消失。
        // 清屏移到下方 match 的非增量分支里，与"整窗重画"配对。

        // 帧推进。★ 补丁 032：先对账缓冲代数——windui 后备缓冲被重建/释放过
        // （休眠、resize）时上一帧内容不复存在，增量缓存必须作废。
        {
            let mut rt = self.runtime.borrow_mut();
            let gen = windui::platform::overlay_buffer_generation();
            if gen != self.buf_gen.get() {
                self.buf_gen.set(gen);
                rt.invalidate_spotlight();
            }
            rt.frame();
            rt.render();
        }

        // 绘制 DrawList。增量帧（聚光灯挪洞）跳过整窗清屏、只画脏区，并请后端
        // 只呈现脏区；其余帧维持「清屏 + 整窗重画 + 整窗呈现」。
        let rt = self.runtime.borrow();
        // ★ 选框挂着（GIF 选框**或**截图编辑态**或**长截图边框）时一律整窗：边框/手柄是
        //   paint 直接画上缓冲的（不在 `draw_list` 里），会把缓冲弄"脏"到增量恢复认不出来。
        let gif_up = rt.gif_selection().is_some();
        let scroll_up = rt.scroll_frame().is_some();
        let frame_up = gif_up || scroll_up || rt.shot_edit_selection().is_some();
        let partial = if frame_up {
            None
        } else {
            rt.partial_frame_dirty()
        };
        match partial {
            Some(world_rect) => {
                let (x, y, w, h) = self.to_device(world_rect);
                windui::platform::request_overlay_partial_present(x, y, w, h);
            }
            None => {
                // 清除上一帧残留
                canvas.clear_rect(0.0, 0.0, bounds.w as f32, bounds.h as f32);
            }
        }
        render::translate_offset(&rt.draw_list, canvas, self.scale, self.dx, self.dy);

        // ★ 选框边框。共用同一个 `draw_gif_frame`（几何、手柄数、命中判定全同源），
        //   只有语义色不同：长截图就绪=蓝/采集中（紫）、GIF 选框（蓝 / 录中红）、截图编辑态
        //   （琥珀）。画在**覆盖层主窗**上，所以它天然穿透——框内区域照常可点、可打字，
        //   不会像独立窗口那样挡住用户要录/要截的东西。
        //   ★ 长截图的边框必须整体落在采集区**外**（`draw_gif_frame` 正是外扩一整条边）：
        //     采集走 GDI `BitBlt`（含 CAPTUREBLT，分层窗照抓），压在框内的哪怕 1px 都会被
        //     自己拍进拼接器——顶部那几行恒定色会被粘性头部检测误学成"页面头部"。
        //     手柄同理（2026-10-01 实测：手柄半截伸进框内被拍进每一帧，画布上每隔一帧高
        //     一串色块）——长截图两态都**不画手柄**（就绪态也没有拖拽交互）。
        //
        //   ★★ 三段是**并行绘制**而不是 else-if 互斥（2026-10-09 用户口径："长截图、宽
        //     截图、gif 录制、区域录屏……要允许多个同时存在"）：录制流（GIF/选区 MP4 共
        //     `gif_sel`）与滚动流（长/宽截图共 `scroll_ready`/`scroll`）的选框**可以同时
        //     挂着**——上一版写成 else-if，两条流同开时只有先判定的一档被画出来，另一条
        //     流"按钮条在、选框没了"（正是用户实测的毛病）。编辑态是模态全屏（冻结帧盖住
        //     一切），与前两档天然互斥，排最后无害。
        //   ★ 几何换算器（`WorldMap`）三段共用一个（只挂在 `frame_up` 时才建——普通
        //     特效帧一档选框都没有，别白付一次 `dpi_scale()` Win32 调用）：双流同挂时
        //     一帧内两段都要画，各建各的就是每帧多付一次，没必要。
        if frame_up {
            let map = crate::runtime::layer::WorldMap::new(rt.screen);
            if let Some((sel, active)) = rt.scroll_frame() {
                // 两段式配色与手柄：就绪=蓝+手柄（可拖拽调整，交互带由 `zone_at_rt` 的
                // scroll_ready 档同源给出）；采集=紫+**无手柄**（覆盖层全穿透，且手柄伸进
                // 框内的半截会被 BitBlt 拍进帧里——2026-10-01 实测的色块教训）。
                let accent = if active {
                    FrameAccent::Scroll
                } else {
                    FrameAccent::Select
                };
                draw_gif_frame(canvas, sel, &map, accent, !active);
            }
            if let Some(sel) = rt.gif_selection() {
                // ★ 录中红按**选框归属的格式**判（`gif_armed` 已按 `sel_kind` 对账）：录区 MP4
                //   时边框同样要变红 —— 两套录制共用一条选框，颜色语义必须一致。
                let accent = if rt.gif_armed() {
                    FrameAccent::Recording
                } else {
                    FrameAccent::Select
                };
                // ★ 手柄只跟**交互门**走（`gif_interaction_enabled`）：录制 / 倒计时中交互本来就
                //   关着，手柄没有存在意义；而它们有 `hp/2 − t` 那半截伸进选区内部——DXGI 抓的
                //   是整个合成桌面，选框内任何覆盖层像素都会被拍进每一帧（GIF 与选区 MP4 同中
                //   招）。这与长截图 2026-10-01 的教训是同一课：采集态不画手柄。绘制与命中同门，
                //   「画了的才点得中、点得中的都画着」两条不变式同时保住。
                draw_gif_frame(canvas, sel, &map, accent, rt.gif_interaction_enabled());
            }
            if let Some(sel) = rt.shot_edit_selection() {
                draw_gif_frame(canvas, sel, &map, FrameAccent::Editing, true);
            }
        }

        // 续帧
        let window = Rect::new(0, 0, bounds.w, bounds.h);
        match rt.frame_demand() {
            FrameDemand::Now => request_repaint_in(window),
            // 定时变化（按键弹窗/标注提示的淡出）：报出截止时刻，平台睡到那时再唤醒，
            // 中间一帧都不出。
            FrameDemand::After(ms) => request_repaint_in_after(window, ms as u64),
            FrameDemand::Idle => {}
        }
        // ★ GIF 选框 / 长截图**就绪态**挂着时补一次重绘：覆盖层可能正停在 park 态，不补的
        //   话"刚框好"那一帧要等到下一次任何唤醒才画出来（表现为边框迟迟不出现）。GIF 录制
        //   中也要持续出帧，否则边框不会从蓝变红（按钮文字是固定的，这是唯一的状态信号）。
        //   长截图就绪态同理需要**连续出帧**——放大镜是帧驱动的（见下），门没帧就永远不出。
        //
        // ★★ 截图**编辑态**刻意**不**跟这一条（我第一版写的是 `frame_up`，那是错的）：
        //   编辑态的屏面（冻结帧 + 压暗 + 工具栏 + 边框）**静止时一个像素都不变**，唯一的
        //   动态元素是光标放大镜，而它随鼠标移动走 —— 移动会经输入桥接送来 `UiMsg::Input`，
        //   `input_needs_frame()`（含 `is_drawing()`）为真 → 自带一次重绘。这里再无脑续帧，
        //   就退化成"整屏 CPU 软渲染以刷新率空转"，正是 `Runtime::frame_demand` 那条
        //   "标注模式空闲时必须报 Idle"的注释专门防的事（症状：进编辑态 CPU 一直满着）。
        //   进场那一帧由 `UiMsg::ShotEditChanged` 负责（它解 park + 重绘）。
        //   ★ 长截图**就绪态**与 GIF 选框同类（短暂、可拖、带放大镜的交互态），走同一条
        //     续帧先例；**采集态**不跟（边框静止，250ms 对账足够）。
        if gif_up || rt.scroll_ready().is_some() {
            request_repaint_in(window);
        }
        drop(rt);
        // ★★ GIF 选区**调整**阶段的放大镜：**每帧按本帧真实光标对齐一次**。
        //
        // ★★★ 为什么必须帧驱动，不能用 `PointerKind::Move` 事件驱动（我第一版就是这么写的，
        //   是错的）：`MouseMode::GifRegion` 下 `on_nchit` 对**选区内部与选区外部**回
        //   `HTTRANSPARENT` —— 那两处要穿透给被录应用。而 `HTTRANSPARENT` 意味着
        //   **那些点上覆盖层根本收不到 `WM_MOUSEMOVE`**（windui 的 `PointerEvent` 来自它
        //   自己的窗口过程，`platform/win32` 的 `WM_MOUSEMOVE`），于是：
        //
        //       悬停在边框带 → 放大镜出现 → 移进框内（想点"开始"）→ **永远等不到收起事件**
        //       → 放大镜赖在屏上一块（用户一点"开始录制"就会看到）。
        //
        //   拖拽之所以"看起来是事件驱动在工作"，只是因为 Down 时 `ctx.capture()` 了，
        //   捕获态下所有移动都会送进来 —— 悬停没有这层保护。
        //
        //   帧驱动与命中测试**完全无关**，而 GIF 选框挂着时覆盖层本来就在连续出帧
        //   （上面那条 `request_repaint_in`），所以这条路是白拿的。
        //
        // ★ 也不额外调 Win32 取光标：`rt.frame()`（本函数开头）刚用 `display::cursor_pos()`
        //   刷过 `live_cursor`——**物理像素、虚拟桌面坐标系，正好就是本模块用的世界坐标**
        //   （`WorldMap::to_world` 的 `origin + local*dpi`）。项目里"光标跟随"特效早就是这么
        //   驱动的，注释原话是"消除画在事件过期坐标上的滞后"，与本处同一考虑。
        let (live, gif_live, edit_live, region_open) = {
            let rt = self.runtime.borrow();
            (
                rt.live_cursor,
                // ★ 合并门（GIF 交互 或 长截图就绪）：两条可拖流共用同一套"边框带显示
                //   放大镜、内部/外部收起"的几何（`sync_gif_magnifier` 内部按 zone 选点）。
                rt.gif_interaction_enabled() || rt.scroll_interaction_enabled(),
                rt.shot_edit_active(),
                rt.region_session_open(),
            )
        };
        if region_open {
            // ★ 区域选择窗开着 → 放大镜归**它**（源是它那份冻结帧，见
            //   `layer::region::SnapRegionCanvas::track`），这里一个字都不能碰。
            //   为什么必须显式挡：主覆盖层此刻**可能也在出帧**（用户只要开着拖尾/标注/
            //   击键回显任一项，`input_needs_frame()` 就为真、覆盖层就不 park），
            //   而下面的 `sync_gif_magnifier(None)` 会 `hide()` —— 一 hide 就把区域窗
            //   刚出画的那块面板打掉，表现为"框选时放大镜根本没出现过"。
            //   （我自己第一版就漏了这道门；`region_session_open()` 的注释记了为什么
            //   不能靠"覆盖层大概在 park 吧"来分工。）
        } else if edit_live {
            // ★ 截图**编辑态**：与 GIF 那条同构（帧驱动、按本帧真实光标对流），只换**源**
            //   —— 这里的"屏幕"已经被我们自己的编辑画布（冻结帧 + 笔迹）盖住了，实时抓屏
            //   只会抓到"我刚画的那一笔"。所以源取冻结帧，与区域选择阶段同路
            //   （`RegionMagnifier::update`）。
            self.sync_edit_magnifier(Some((live.0.round() as i32, live.1.round() as i32)));
        } else if gif_live {
            self.sync_gif_magnifier(Some((live.0.round() as i32, live.1.round() as i32)));
        } else if self.runtime.borrow().region_magnifier.is_shown() {
            // ★ `is_shown()` 门不能省：`PipWindow::hide()` 一旦窗已创建就会真去
            //   `ShowWindow(SW_HIDE)`，而放大镜窗是**懒创建且用完不销毁**的（做过一次截图
            //   它就一直在）→ 不加门则标注态下每帧白付一次 Win32 调用。
            //   `is_shown()` 纯读字段，零成本。
            self.sync_gif_magnifier(None);
        }
        // 边框直接画进了缓冲（draw_list 之外），缓冲不再等于"暗罩 + 光圈"，
        // 聚光灯增量缓存作废——选框消失后的第一帧必须整窗重画。
        if frame_up {
            self.runtime.borrow_mut().invalidate_spotlight();
        }
    }

    fn on_event(&mut self, ctx: &mut EventCtx, ev: &Event) -> bool {
        let Event::Pointer(pe) = ev else {
            return false;
        };
        // ★ 数位板直通（2026-09-28，三报根治）：书写中且这条指针事件来自**笔** ——
        //   直接喂进与钩子同一条输入队列。为什么：笔的 `WM_POINTER` 在笔尖接触的当帧
        //   就到（windui 补丁 093 直通），而系统"指针→鼠标"翻译层补发的那份要晚 ~0.5s
        //   才被 `WH_MOUSE_LL` 看到（Windows Ink 已知行为）——旧实现只吃钩子那份，
        //   起笔的前半秒就整段延迟。只喂直通这份，起笔即所见。
        //   鼠标永远不走这条（`pointer_is_pen` 只对笔为真）；触摸走 `WM_TOUCH` 管线
        //   也不受影响。笔杆键（右键）不用于书写，交给默认处理。
        // ★ 门只能是"这条事件来自笔"（`pointer_is_pen`）。**不能**再叠 `is_drawing()`：
        //   `is_drawing()` 只在"笔画正在进行"时为真，而笔尖**刚接触**时还没有笔画——
        //   拿它当门等于"笔永远起不了笔"，只能干等翻译层迟到 ~0.5s 的那份鼠标按下把
        //   笔画开起来，之后笔的 Move 才被接管。这就是用户三报"起笔延迟"的真根因。
        //   "标注开没开"由 [`Self::pen_drive_drawing`] 自己判。
        //   鼠标永远不走这条（`pointer_is_pen` 只对笔为真）；触摸走 `WM_TOUCH` 管线
        //   也不受影响。笔杆键（右键）不用于书写，交给默认处理。
        if windui::platform::pointer_is_pen() && self.pen_drive_drawing(ctx, pe) {
            return true;
        }
        match pe.kind {
            PointerKind::Down if pe.button == MouseButton::Left => {
                // ★ 命中判定只认"当前可交互的选区内"。录制/倒计时中或退出交互态后
                //   `gif_zone_at` 返回 None，单击就穿通给被录应用（框内穿透）。
                //   世界坐标 = 窗口本地逻辑 + 虚拟桌面前置：windui 给的 `pe.pos` 是逻辑，
                //   先经 `WorldMap` 换算回世界物理再交给纯函数命中。
                let (wx, wy) = {
                    let rt = self.runtime.borrow();
                    // ★ 门是合并判据（GIF 交互 或 长截图就绪）：散开写就会出现命中/光标/
                    //   放大镜三处不同步的错位态（见 `drag_interaction_enabled` 注释）。
                    if !rt.drag_interaction_enabled() {
                        return false;
                    }
                    let map = crate::runtime::layer::WorldMap::new(rt.screen);
                    let (fx, fy) = map.to_world(pe.pos.x as f32, pe.pos.y as f32);
                    (fx.round() as i32, fy.round() as i32)
                };
                // ★ 双流共存：命中判定连**归属**一起拿（哪条流的选框被点中），拖拽全程
                //   跟着这一档走 —— 移动/路由都按它分派（见 Move 分支），不能按"哪条流
                //   挂着"猜（两条流的选框可以同时挂着）。
                let (zone, target) = zone_with_target_at(&self.runtime, wx, wy);
                if zone == HitZone::None {
                    return false; // 框内/框外 → 穿透。
                }
                self.gif_drag.set(Some(GifDrag {
                    zone,
                    target,
                    last_x: wx,
                    last_y: wy,
                }));
                ctx.capture();
                request_repaint();
                true
            }
            PointerKind::Move => {
                // 没在拖 → 无事可做。
                // ★ GIF 调整阶段的放大镜**不在这里**：它由 `paint` 每帧按**真实光标**驱动
                //   （原因见那里：`MouseMode::GifRegion` 下选区内部/外部是 `HTTRANSPARENT`，
                //   那些点上覆盖层收不到本事件）。这条早退顺便把"每事件一次 `WorldMap::new`"
                //   （内含一次 `dpi_scale()` Win32 调用）挡在门外 —— 移动事件实测约 1000 条/秒。
                let Some(mut g) = self.gif_drag.get() else {
                    return false;
                };
                // 转换为世界物理坐标。
                let (wx, wy) = {
                    let rt = self.runtime.borrow();
                    let map = crate::runtime::layer::WorldMap::new(rt.screen);
                    let (fx, fy) = map.to_world(pe.pos.x as f32, pe.pos.y as f32);
                    (fx.round() as i32, fy.round() as i32)
                };
                // ★ 增量语义：相对上一点累加。拖拽中若盖层先被禁用（例如恰好点中"开始"快捷键
                //   进入采集），立即停下——手柄不再响应，别让离手前的滑块继续动。
                if !self.runtime.borrow().drag_interaction_enabled() {
                    self.gif_drag.set(None);
                    ctx.release_capture();
                    return true;
                }
                // 按**按下时命中的那一条流**路由（`GifDrag::target`，见 Down 分支）。
                // ★ 不能按"哪条流挂着"分派：双流共存后两条选框可以同时挂着，按存在性
                //   分派会把 GIF 框的拖拽套到长截图框头上。`None` = 编辑态档（它的选区
                //   由全局钩子 `edit_drag` 管，这里 `gif_resize_or_move` 对空 `gif_sel`
                //   本身就是空转，与旧版行为一致）。
                {
                    let mut rtm = self.runtime.borrow_mut();
                    match g.target {
                        Some(DragTarget::Scroll) => {
                            rtm.scroll_resize_or_move(g.zone, wx - g.last_x, wy - g.last_y);
                        }
                        _ => {
                            rtm.gif_resize_or_move(g.zone, wx - g.last_x, wy - g.last_y);
                        }
                    }
                }
                g.last_x = wx;
                g.last_y = wy;
                self.gif_drag.set(Some(g));
                request_repaint();
                true
            }
            PointerKind::Up if pe.button == MouseButton::Left => {
                let Some(_) = self.gif_drag.replace(None) else {
                    return false;
                };
                // 必须显式还捕获，否则覆盖层会永久持有 OS 鼠标捕获并吞掉后续全局操作。
                ctx.release_capture();
                // 首帧边框已随 move 重绘过；抬手后再保证一帧收尾。
                request_repaint();
                true
            }
            // ★ 右键 = 退出选框调整态（2026-10-08 用户口径："进入选区状态，鼠标右键
            //   无法退出"）。与区域选择窗的右键取消同一手势语义。只在**可拖交互态**
            //   生效：就绪态的长截图走 `cancel_scroll`（清就绪框），GIF/MP4 选框走
            //   `close_gif_bar`（收边框 + 控件条；上膛/录制中的取消已由 ✕ 按钮管，
            //   此刻交互门本来就关着，进不来）。其余一切（标注/编辑态/常态穿透）不动作，
            //   右键照旧归底层窗口。
            //   ★ 双流共存后两条选框可以同时挂着：右键是"脱身"手势而非位置相关操作，
            //     一次退一条（先退长截图，下一次右键再退 GIF）——两条都退反而越过用户
            //     对"只想退出其中一个"的表达。
            PointerKind::Down if pe.button == MouseButton::Right => {
                let mut rt = self.runtime.borrow_mut();
                if !rt.drag_interaction_enabled() {
                    return false;
                }
                if rt.scroll_ready().is_some() {
                    rt.cancel_scroll();
                } else {
                    rt.close_gif_bar();
                }
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::Modifiers;

    fn screen(x: i32, y: i32, w: i32, h: i32) -> ScreenRect {
        ScreenRect { x, y, w, h }
    }

    /// 数位板起笔延迟的回归锁：屏幕标注一开，`drawing_owns_pointer()` 必须立刻为真
    /// ——覆盖层 `on_nchit` 靠它整屏回 `Client`，笔的 `WM_POINTER` 才落到本窗。
    /// 2026-09-29 之前这里漏了屏幕标注，命中把笔送进桌面，补丁 092/093 成了死代码，
    /// 起笔只能等翻译层迟到 ~0.5s 的鼠标副本（四报根因）。
    #[test]
    fn drawing_owns_pointer_follows_draw_mode() {
        let rt = Rc::new(RefCell::new(Runtime::new(
            crate::core::config::Config::default(),
        )));
        assert!(
            !rt.borrow().drawing_owns_pointer(),
            "未开标注时指针不归标注管（穿透照旧）"
        );
        rt.borrow_mut()
            .drawing
            .controller_mut()
            .toggle(crate::modules::drawing::controller::DrawMode::Screen, 0.0);
        assert!(
            rt.borrow().drawing_owns_pointer(),
            "标注一开整屏命中必须归覆盖层，否则笔的 WM_POINTER 进不了窗"
        );
        rt.borrow_mut().drawing.controller_mut().stop();
        assert!(
            !rt.borrow().drawing_owns_pointer(),
            "退出标注后必须交还穿透，否则桌面点不动"
        );
    }

    /// ★ 回归锁（2026-10-01"选框结束后无法调整"）：**每一条可拖流**都必须把鼠标模式
    ///   顶到 `GifRegion`。决策若落回 `Transparent`，窗口带着 `WS_EX_TRANSPARENT`，
    ///   OS 直接跳窗——NCHITTEST 闭包根本不会被问，拖拽路由全成摆设。
    #[test]
    fn mouse_mode_covers_every_draggable_stream() {
        let rt = Rc::new(RefCell::new(Runtime::new(
            crate::core::config::Config::default(),
        )));
        // 空态：穿透。
        assert_eq!(mouse_mode_for(&rt.borrow()), MouseMode::Transparent);
        // 长截图就绪态：必须 GifRegion（本回归锁的主断言）。
        rt.borrow_mut().arm_scroll(
            crate::core::geometry::Rect {
                x: 10.0,
                y: 10.0,
                w: 120.0,
                h: 80.0,
            },
            false,
        );
        assert_eq!(
            mouse_mode_for(&rt.borrow()),
            MouseMode::GifRegion,
            "长截图就绪态必须清 WS_EX_TRANSPARENT，否则 NCHITTEST 收不到、拖不动"
        );
        rt.borrow_mut().cancel_scroll();
        assert_eq!(mouse_mode_for(&rt.borrow()), MouseMode::Transparent);
        // GIF 选框（既有行为，别被这次合并判据改坏）：就绪未上膛 = 可交互。
        rt.borrow_mut().ready_gif_for_test();
        assert_eq!(mouse_mode_for(&rt.borrow()), MouseMode::GifRegion);
        rt.borrow_mut().close_gif_bar();
    }

    /// ★★ 双流共存回归锁（2026-10-09"按钮条有了、选框没了"）：GIF 选框与长截图就绪框
    ///   **可以同时挂着**，且拖拽命中必须**各归各的流**。
    ///
    /// 能红两次：
    /// - 上一版 `paint` 写成 else-if，两流同挂时只画先判定的一档（状态层钉不住绘制，
    ///   但下面"两档同时 Some"的断言把共存语义钉死，回退任何一处互斥短路都过不了
    ///   命中归属的断言——旧版 `zone_at_rt` 对长截图框上的点会按 GIF 框判定返回 `None`）；
    /// - 旧版拖拽路由按"哪条流挂着"分派（`scroll_ready().is_some()`），两流同挂时
    ///   GIF 框的拖拽会被套到长截图框头上 —— 本判据的归属断言专防这个。
    #[test]
    fn dual_stream_frames_coexist_and_drag_hits_route_to_own_stream() {
        let rt = Rc::new(RefCell::new(Runtime::new(
            crate::core::config::Config::default(),
        )));
        // 摆双流：GIF 框 (0,0,320,240)，长截图就绪框 (500,300,200,150)——两框不重叠。
        rt.borrow_mut().ready_gif_for_test();
        rt.borrow_mut().arm_scroll(
            crate::core::geometry::Rect {
                x: 500.0,
                y: 300.0,
                w: 200.0,
                h: 150.0,
            },
            false,
        );
        // 共存本身：两档同时 Some（任何一方把另一方清掉都算坏）。
        assert!(
            rt.borrow().gif_selection().is_some() && rt.borrow().scroll_ready().is_some(),
            "GIF 选框与长截图就绪框必须能同时挂着（2026-10-09 双流共存）"
        );
        // GIF 框顶边框带（(100,0)，离中点 160 与两角都超出手柄半径）→ 归录制流。
        let (z, t) = zone_with_target_at(&rt, 100, 0);
        assert_eq!(z, HitZone::Body, "GIF 框边带上必须命中 Body");
        assert_eq!(t, Some(DragTarget::Gif), "GIF 框的命中必须归录制流");
        // 长截图框顶边框带（(570,300)，同上避开手柄）→ 归滚动流。
        let (z, t) = zone_with_target_at(&rt, 570, 300);
        assert_eq!(z, HitZone::Body, "长截图框边带上必须命中 Body");
        assert_eq!(
            t,
            Some(DragTarget::Scroll),
            "长截图框的命中必须归滚动流（按流存在性分派会错套到 GIF 头上）"
        );
        // 两框之间的空白 → 穿透（None, None）。
        let (z, t) = zone_with_target_at(&rt, 400, 270);
        assert_eq!(z, HitZone::None, "两框之外的点必须穿透");
        assert_eq!(t, None, "穿透点没有归属流");
    }

    /// 面板快捷键是 **toggle**，命中判定有两道闸门：① 留空（停用）不认；② 长按连发的
    /// `repeated=true` 不认——否则按住 Ctrl+Shift+D 会开合几十次。
    #[test]
    fn panel_hotkey_hit_ignores_empty_spec_and_auto_repeat() {
        let rt = Rc::new(RefCell::new(Runtime::new(
            crate::core::config::Config::default(),
        )));
        let down = |repeated: bool| InputEvent::KeyDown {
            key: KeyCode::Char('d'),
            mods: Modifiers {
                ctrl: true,
                shift: true,
                alt: false,
                win: false,
            },
            repeated,
            time_ms: 0,
        };
        let no_mods = Modifiers::default();
        rt.borrow_mut().config.global.panel_hotkey = "Ctrl+Shift+D".into();
        assert!(panel_hotkey_hit(&rt, &down(false)));
        assert!(
            !panel_hotkey_hit(&rt, &down(true)),
            "连发不该重复触发 toggle"
        );
        rt.borrow_mut().config.global.panel_hotkey = String::new();
        assert!(!panel_hotkey_hit(&rt, &down(false)), "留空 = 停用");
        assert!(!panel_hotkey_hit(
            &rt,
            &InputEvent::KeyUp {
                key: KeyCode::Char('d'),
                mods: no_mods,
                time_ms: 0,
            }
        ));
    }

    /// 落点在屏幕中间时，输入窗出现在落点的右下方（不压住刚点的地方）。
    #[test]
    fn text_box_sits_below_right_of_the_anchor() {
        let (x, y) = text_box_pos(
            Point { x: 400.0, y: 300.0 },
            screen(0, 0, 1920, 1080),
            400,
            128,
        );
        assert_eq!((x, y), (412, 312));
    }

    /// 右下越界就回退到"贴边"，绝不把窗口推出去——推出去用户就看不见输入框了。
    #[test]
    fn text_box_clamps_inside_the_virtual_desktop() {
        let (x, y) = text_box_pos(
            Point {
                x: 1900.0,
                y: 1070.0,
            },
            screen(0, 0, 1920, 1080),
            400,
            128,
        );
        assert_eq!(x, 1920 - 400, "右边界应贴住虚拟桌面右边");
        assert_eq!(y, 1080 - 128, "下边界应贴住虚拟桌面下边");
    }

    /// 副屏在主屏左侧（虚拟桌面原点为负）：钳位必须按**整块虚拟桌面**算，
    /// 否则在负坐标那一半上会把窗口钳到 x=0（主屏里）去。
    #[test]
    fn text_box_handles_a_negative_virtual_desktop_origin() {
        let scr = screen(-1920, 0, 3840, 1080);
        // 副屏上靠近左上角落笔：窗口应落在负坐标区
        let (x, y) = text_box_pos(
            Point {
                x: -1900.0,
                y: 100.0,
            },
            scr,
            400,
            128,
        );
        assert_eq!((x, y), (-1888, 112));

        // 副屏左下角：下边钳到虚拟桌面下沿，x 仍是负的
        let (x, y) = text_box_pos(
            Point {
                x: -1900.0,
                y: 1070.0,
            },
            scr,
            400,
            128,
        );
        assert_eq!(x, -1888);
        assert_eq!(y, 1080 - 128);
    }

    /// 屏幕比窗口还小的极端情况：钳位后仍在虚拟桌面内（不出现"右边界 < 左边界"）。
    #[test]
    fn text_box_never_escapes_a_tiny_screen() {
        let scr = screen(0, 0, 300, 100);
        let (x, y) = text_box_pos(Point { x: 290.0, y: 90.0 }, scr, 400, 128);
        assert_eq!((x, y), (0, 0), "放不下时退回原点，而不是把窗口丢到屏幕外");
    }

    // ─────────── 录屏完成提示窗：长路径靠截断、不靠降字号（2026-09-28 定案） ───────────

    /// 提示窗固定 400×150、windui 文本**不换行**：长路径只能按像素预算截断。
    ///
    /// ★ 本测试同时钉死"降字号救不了"：下面那条 46 字符的目录按 12px 排仍是 391px，
    /// 一样超出 372px 的可用宽——所以字号统一走 `FONT_MD`，长度交给 [`rec_toast_elide`]
    /// （[`crate::ui::card::ellipsize`]，保尾部；与下拉显示名同一套宽度模型）。
    #[test]
    fn rec_toast_elides_the_path_instead_of_shrinking_the_font() {
        let font = crate::ui::tokens::FONT_MD;
        let label_w = crate::ui::card::text_width(REC_TOAST_FOLDER_LABEL, font);
        let folder = r"C:\Users\user\Videos\CurSeen\2026-09-28_170533";
        let full = |f: f32| {
            crate::ui::card::text_width(folder, f)
                + crate::ui::card::text_width(REC_TOAST_FOLDER_LABEL, f)
        };
        // ① 样例必须真的超出预算，否则这条护栏没有意义。
        assert!(
            full(font) > REC_TOAST_TEXT_W,
            "样例目录不够长，换一条更长的：{:.1}px",
            full(font)
        );
        // ② 曾经的"降一档字号"方案在同一条路径下仍然溢出 → 降档无效。
        assert!(
            full(12.0) > REC_TOAST_TEXT_W,
            "12px 也放不下（{:.1}px）：降字号不是解法",
            full(12.0)
        );
        // ③ 截断后整行（含「保存位置：」前缀）必须落在可用宽内，且前缀没被切掉。
        let shown = format!(
            "{REC_TOAST_FOLDER_LABEL}{}",
            rec_toast_elide(folder, REC_TOAST_TEXT_W - label_w)
        );
        let w = crate::ui::card::text_width(&shown, font);
        assert!(
            w <= REC_TOAST_TEXT_W,
            "「{shown}」折 {w:.1}px，超出可用宽 {REC_TOAST_TEXT_W:.0}px"
        );
        assert!(
            shown.starts_with(REC_TOAST_FOLDER_LABEL),
            "前缀不能被截断掉：{shown}"
        );
        // ④ 短文本原样返回——不无故加省略号。
        assert_eq!(rec_toast_elide("a.mp4", REC_TOAST_TEXT_W), "a.mp4");
    }

    // ───────────────────── RefCell 重入禁令（2026-09-17 崩溃的回归锁） ─────────────────────

    /// **不许**在 `if let` / `while let` / `match` 的 scrutinee 里借 `RefCell`。
    ///
    /// # 为什么
    ///
    /// Rust 2021 里这些位置的临时量活到**整个语句结束**（不是到条件求值完）——于是
    /// `if let Some(x) = rt.borrow_mut().take() { open(x) }` 里，`open()` 只要再借用同一个
    /// `RefCell` 就必然撞 `already borrowed`。这不是理论问题，是 2026-09-17 的线上崩溃：
    ///
    /// ```text
    /// [FATAL] panic: RefCell already borrowed  at src\runtime\overlay.rs:332:24
    /// ```
    ///
    /// 真凶是 250ms 定时器兜底那条"取贴图请求 → 开贴图窗"。它只在**消息通路没先取走请求**
    /// 时才命中，所以表现为"反复按截图快捷键，某一次突然崩溃卡退"。同一个坑在 2026-09-16
    /// 20:39 已经崩过一次（那次消息是 `already mutably borrowed`，因为旧版 `open_pin` 里
    /// 用的是 `borrow()`）。
    ///
    /// 正解一律是**先 take 到局部变量**，再进 if-let：
    /// ```ignore
    /// let req = rt.borrow_mut().take_pin_request();
    /// if let Some(req) = req { open_pin(ctx, &rt, req); }
    /// ```
    ///
    /// 这条 lint 扫**整棵 `src/`**（跳过 `#[cfg(test)]` 之后的测试代码），比逐个 review 可靠。
    #[test]
    fn no_refcell_borrow_in_a_let_chain_scrutinee() {
        fn walk(dir: &std::path::Path, offences: &mut Vec<String>, scanned: &mut usize) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, offences, scanned);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let Ok(src) = std::fs::read_to_string(&path) else {
                    continue;
                };
                // 只扫生产代码：测试自己会写这些字符串来讨论这条规则。
                let body = src.split("#[cfg(test)]").next().unwrap_or(&src);
                *scanned += 1;
                let lines: Vec<&str> = body.lines().collect();
                for (i, line) in lines.iter().enumerate() {
                    let t = line.trim_start();
                    let head = t.starts_with("if let ")
                        || t.starts_with("while let ")
                        || t == "match"
                        || t.starts_with("match ");
                    if !head {
                        continue;
                    }
                    // 语句可能跨行：从这一行往后找到第一个 '{'，其间出现的 borrow 都算。
                    let mut seg = String::new();
                    for l in lines.iter().skip(i).take(4) {
                        seg.push_str(l);
                        if l.contains('{') {
                            break;
                        }
                    }
                    if seg.contains(".borrow_mut()") || seg.contains(".borrow()") {
                        offences.push(format!("{}:{}  {}", path.display(), i + 1, t));
                    }
                }
            }
        }

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offences = Vec::new();
        let mut scanned = 0_usize;
        walk(&root, &mut offences, &mut scanned);

        assert!(scanned > 50, "只扫到 {scanned} 个文件，路径大概不对");
        assert!(
            offences.is_empty(),
            "以下位置在 if-let/match 的 scrutinee 里借了 RefCell —— 块内再借用必然 panic：\n{}",
            offences.join("\n")
        );
    }

    /// 把上面那条规则背后的 Rust 语义钉死：`if let` 的临时量确实活到语句尾。
    ///
    /// 这条测试的价值是**防止有人把 lint 当迷信删掉**——它演示了老写法为什么必崩、
    /// 新写法为什么安全。
    #[test]
    fn if_let_scrutinee_temporaries_outlive_the_block() {
        let cell = std::cell::RefCell::new(0_i32);

        // 老写法：`if let Some(_) = Some(guard 表达式) { ... }` 里再借用 → panic。
        let old_style = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if let Some(0) = Some(*cell.borrow_mut()) {
                let _again = cell.borrow_mut();
            }
        }));
        assert!(
            old_style.is_err(),
            "if-let 的 scrutinee 临时量本应活到语句尾、块内再借用应 panic；\
             若这条不再 panic，说明 Rust 语义变了，lint 需要重新评估"
        );

        // 新写法：先 take 到局部变量，再进 if-let → 安全。
        let taken = *cell.borrow_mut();
        if let Some(0) = Some(taken) {
            let _again = cell.borrow_mut();
        }
    }

    // ─────────── 采集 / 录制态不画手柄（GIF/选区 MP4 2026-10-08 用户实测） ───────────

    /// 手柄的内半截必须真的会伸进选区——这是「录制态不画手柄」这条规矩的**几何依据**，
    /// 钉在这里防几何常量将来漂移：`hp/2 − t` 若变成 ≤0，规矩就失去了存在理由。
    ///
    /// 离屏渲两遍 `draw_gif_frame`（同一选区、同一配色，只差 `with_handles`）：
    /// - 不带手柄：选区**内部**一个手柄色像素都不能有（边框整体在外侧，本就该干净）；
    ///   但边框本身要在（外侧一列），证明画面真的画了。
    /// - 带手柄：内部**必须**出现手柄色（角 + 边中点），即「带手柄就会污染产物」。
    #[test]
    fn handles_without_capture_contaminate_interior_so_capture_must_not_draw_them() {
        use windui::render::{DibTarget, RenderTarget};
        use windui::text::DWriteEngine;

        const PW: u32 = 280;
        const PH: u32 = 230;
        let sel = ScreenRect {
            x: 60,
            y: 50,
            w: 200,
            h: 150,
        };
        let map = WorldMap::with_dpi(
            ScreenRect {
                x: 0,
                y: 0,
                w: PW as i32,
                h: PH as i32,
            },
            1.0,
        );
        let (r, g, b) = FrameAccent::Select.rgb();

        let render = |with_handles: bool| -> Vec<u8> {
            let mut buf = vec![0_u8; (PW * PH * 4) as usize];
            let mut engine = DWriteEngine::new();
            {
                let mut target = DibTarget::new(&mut buf, PW, PH);
                let mut canvas = target.make_canvas(&mut engine, 1.0);
                draw_gif_frame(&mut *canvas, sel, &map, FrameAccent::Select, with_handles);
            }
            buf
        };
        let px = |buf: &[u8], x: u32, y: u32| -> (u8, u8, u8) {
            let i = ((y * PW + x) * 4) as usize;
            (buf[i], buf[i + 1], buf[i + 2])
        };
        let is_accent = |p: (u8, u8, u8)| p == (r, g, b);

        let clean = render(false);
        // 边框画了没有：外侧一列（x=59，sel 从 60 起）应是手柄色。
        assert!(
            is_accent(px(&clean, 59, 125)),
            "外侧边框列应有色（证明画面真的画了）"
        );
        // 选区内部整片扫描：一个手柄色像素都不许有。
        let mut hit = 0_u32;
        for y in 50..200 {
            for x in 60..260 {
                if is_accent(px(&clean, x, y)) {
                    hit += 1;
                }
            }
        }
        assert_eq!(
            hit, 0,
            "不带手柄时选区内部必须全干净——采集裁的就是这块"
        );

        let dirty = render(true);
        // 角与边中点的手柄内半截（hp/2 − t = 3.5px）应伸进内部：
        // 外框顶线在 y=48（sel.y − t），手柄纵跨 [42.5, 53.5]，侵入内部的是 y=50..53。
        // 顶边中点取 (160,51)、左上角取 (61,51)。（y=53 是半覆盖的 AA 行，不取整值断言。）
        assert!(
            is_accent(px(&dirty, 160, 51)),
            "顶边中点手柄应伸进选区（这条若不成立，说明手柄几何变了，上面的「必须不画」要重估）"
        );
        assert!(is_accent(px(&dirty, 61, 51)), "角手柄同样伸进选区");
    }

    /// 手柄的显隐**只跟交互门走**：录制 / 倒计时中（`gif_armed`）交互本来就关着，
    /// 手柄没有存在意义——而它们的内半截会被 DXGI 拍进每一帧（GIF 与选区 MP4 同招）。
    /// 绘制端传的就是这个门（见渲染处 `rt.gif_interaction_enabled()`），本条把门的
    /// 状态语义钉死，防止 `gif_interaction_enabled` 将来被放宽。
    #[test]
    fn armed_selection_closes_the_interaction_gate_that_drives_handles() {
        let mut rt = Runtime::new(crate::core::config::Config::default());
        rt.ready_gif_for_test();
        assert!(
            rt.gif_interaction_enabled(),
            "就绪态：可交互 → 手柄应显示（拖 8 点 / 边框全靠它）"
        );
        rt.armed_gif_for_test();
        assert!(
            !rt.gif_interaction_enabled(),
            "录制/倒计时中：交互门必须关闭 → 手柄必须不画（内半截会被拍进每一帧）"
        );

        // 源码钉：GIF/MP4 分支的绘制调用必须**走交互门**，不许写回 `true`。
        // （截图编辑态 `FrameAccent::Editing, true` 是合法的——编辑态不录屏，没有采集污染。
        //   曾因无条件 `true` 把录制中红手柄的内半截拍进 GIF / 选区 MP4 每一帧，2026-10-08。）
        let src = include_str!("overlay.rs");
        let body = src.split("#[cfg(test)]").next().unwrap_or(src);
        assert!(
            !body.contains("accent, true"),
            "GIF/MP4 选框的 draw_gif_frame 调用不许传 true——录制态手柄会被拍进每一帧，必须走交互门"
        );
    }
}
