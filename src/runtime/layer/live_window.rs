//! 直播捕获窗（m82 B6）：把虚拟键盘画在**一扇任何直播软件都认的普通小窗**上。
//!
//! ## 为什么是"屏内小窗"而不是"屏外大窗"（B6，2026-11-17 用户实测）
//!
//! B1~B2.8 把它做成 1920×1080、贴右缘只露 2px、还置顶。用户在直播姬里选中那扇窗（当时叫
//! `CurSeen Live`），抓到的却是**主面板**。读完参照项目，原因很清楚：
//!
//! - **没有任何项目把要被抓的窗藏到屏幕外**：NohBoard 的窗就是键盘的紧致包围盒
//!   （整键盘 1024×328、FPS 局 431×237），默认摆 (25,25)、可拖、记位置、从不屏外
//!   （`MainForm.cs:163`、`GlobalSettings.cs:209-215`）；YAKC 要藏的时候是 `window.hide()`
//!   并注释写明 OBS 那边照样出画面（`src-tauri/src/overlay.rs:41-50`）。
//! - 捕获侧剔的是"**看不见**"，不是"被压住"：OBS `window-helpers.c:292-312` 过滤
//!   不可见 / 最小化 / cloaked / `WS_EX_TOOLWINDOW` / `WS_CHILD` / 客户区 0×0，
//!   `window-capture.c:644` 注释直说"HWND 不可见时 WGC 根本初始化不了"。
//!   **被别的窗口压住照样能抓**——所以"桌面上不想看见它"的正解是不置顶，不是推出屏幕。
//!
//! 于是本窗现在的形状：尺寸 = 键盘包围盒 + 描边余量；位置 = 键盘本来在桌面上的那一块；
//! **不置顶**；可交互（B11：能拖、能关，见下文「交互纪律」）。★ 也**不带**
//! `WS_EX_TOOLWINDOW`：那个位会被 OBS 直接过滤掉，代价是它会占一个任务栏项（NohBoard 同样如此）。
//!
//! ## 直角，不是圆角
//!
//! Win11 的 DWM 会给带框顶层窗切圆角，四个角的像素直接消失。这块表面的尺寸＝键盘的紧致
//! 包围盒，圆角因此啃到键帽的直角上，色键一步把缺角当背景抠穿（2026-11-17 用户实测）。
//! 所以样式挂载时显式 `DWMWCP_DONOTROUND`（windui 只对 `.frameless(true)` 的窗做这件事，
//! 带标题栏的窗得自己说）。
//!
//! ## 底色 = 色键，用户在设置页自选
//!
//! 出厂黑 `[0,0,0,255]`（用户拍板：默认给最常见的键色，剩下用颜色选择器自己改）。
//! ★ 黑有代价：键帽自己也可能带近黑像素，色键一刀下去会连键盘一起抠穿（2026-11-17 实测）。
//!   撞了就换一色——这是色键的正常操作，不是 bug。底色是**建窗参数**，改它 = 重建这扇窗
//!   （这块表面没有用户状态，重建最便宜）。
//!
//! ## 架构定位
//!
//! 覆盖层是**帧引擎**（`frame()/render()` 更新 `live_draw_list`），本窗只是**第二块呈现
//! 表面**：`Widget::paint` 里把 `live_draw_list` 经 `render::translate_offset` 画到 windui
//! Canvas——与桌面覆盖层同一条渲染路径，像素语义一致。窗口坐标 = 世界物理坐标减去
//! [`Runtime::live_view`] 那个原点、再除以 dpi。
//!
//! ## 交互纪律
//!
//! 可交互表面（B11，2026-11-17 用户拍板）：**带系统标题栏、能拖、能关**，形状由 [`sync`]
//! 对账。★ 曾经给它挂过点击穿透（`WS_EX_TRANSPARENT`）——那是"窗只是块画布、不该拦鼠标"
//! 的思路，后果是**标题栏成了摆设**：拖不动、关不掉（用户实测）。两件事不能同时要：
//! 要能拖要能关，就得让它是普通窗，代价是它会挡住自己那一块的鼠标（NohBoard 同样如此），
//! 所以位置可拖走、且拖走的位置记进配置。
//!
//! 关窗 = 取消设置里那行勾选（两边同步，落盘），**不是**半秒后自作主张重开——
//! 上一版把"关掉"当成误操作，用户明确否掉了。
//!
//! **最小化就是最小化**（B13，2026-11-17 用户拍板，照 NohBoard）：标题栏三颗按钮各做各的
//! 事，最小化不碰开关、也不自动弹回来。代价照实说：最小化的窗按平台规则捕获软件拿不到
//! （OBS 排 `IsIconic`），直播端那时是空的——这是用户自己的选择，不是我们要替他纠正的误操作。

use crate::runtime::Runtime;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use windui::core::{EventCtx, Widget};
use windui::geometry::Rect;
use windui::prelude::*;
use windui::render::Canvas;
use windui::text::TextEngine;

/// 窗口标题（e2e 按标题找窗 / 关窗对账都用它）。
///
/// ★ 叫**这个东西**的名字，不叫"Live"（B13，2026-11-17 用户口径）：以后还会有别的
///   可捕获表面，`Live` 当后缀会撞名；直播软件窗口列表里显示的也是这串字。
pub const LIVE_TITLE: &str = "CurSeen 虚拟键盘";

/// 对账节拍（也是呈现上限：约 30fps）。
pub(crate) const LIVE_POLL_MS: u64 = 33;

/// 一扇直播捕获窗的全部**建窗参数**。`x/y` 是"窗现在应该在哪"，其余任何一项变了都要重建窗。
///
/// ★ 位置**不参与**重建判定（`same_look`）：拖一下松手、键盘挪个位置，只该把窗挪过去，
///   不该销毁重建——重建一次就是一个新 HWND，肉眼闪一下，直播端那边捕获源还会丢一帧
///   （B14：每次松手都重建，日志里"位置记为"紧跟着"开 @"，24ms 后整扇窗重开）。
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct Shape {
    /// 摆放坐标（物理像素——`Window::position` 吃物理像素）。
    x: i32,
    y: i32,
    /// 客户区尺寸（逻辑 dp——`Window::new` 吃逻辑像素。两者单位不同，见 AGENTS.md §五）。
    w: i32,
    h: i32,
    /// 色键底色 RGB（alpha 恒 255：窗口捕获不吃透明）。
    bg: [u8; 3],
    /// 画布原点（世界物理像素）：键帽左上角，含余量后的外沿。
    vx: i32,
    vy: i32,
    /// 等比放大倍数（世界 → 窗口）。★ 不是把画好的像素拉大，而是让渲染按这个倍数
    /// **重新矢量绘制**（见 `LiveCanvas::paint`）——拉像素会糊，重绘不会。
    scale: f32,
}

impl Shape {
    /// 除位置以外的所有建窗参数是否一致——一致就只挪窗，不重建。
    fn same_look(&self, other: &Shape) -> bool {
        self.w == other.w
            && self.h == other.h
            && self.bg == other.bg
            && self.vx == other.vx
            && self.vy == other.vy
            && self.scale == other.scale
    }
}

/// 让这扇窗「从显示器捕获/屏幕录制里消失」（`WDA_EXCLUDEFROMCAPTURE`）——**已真机判废**。
///
/// 设想是窗仍在物理屏幕上、仍能被按句柄的窗口捕获抓到，只是不再被「整块屏幕」那种采集
/// 包含，于是「显示器捕获 + 窗口捕获」不会把同一次渲染拿两遍。
/// ★ 2026-11-17 用户在直播姬真机定案：**窗口捕获也跟着变空**（连显示都没有）——这个 API
///   两半一起挡，不是只挡屏幕录制。形态回到「窗口看得见才抓得到」。
/// 常量留着而不是删掉：它是这条死路的证据锚点。谁再提「用 display affinity 藏窗」，这里
/// 写着真机结论；要翻回 true 只改这一个字符。
const EXCLUDE_FROM_SCREEN_CAPTURE: bool = false;

/// 按当前配置算这扇窗该长什么样。键盘没启用 / 方案解析不出键帽 → `None`（不建窗）。
///
/// ★ 尺寸来源是**真实键帽矩形的并集**（`vk::layout_world_bounds`），不是"布局总宽 × 行数"
///   另算一遍：两套几何迟早漂移，漂移的表象是"直播里键盘缺一条边"。
fn shape_of(rt: &Runtime) -> Option<Shape> {
    let (bx, by, w, h) = rt.vk.layout_world_bounds(&rt.config.virtual_keyboard)?;
    let map = crate::runtime::layer::WorldMap::new(rt.screen);
    let dpi = map.dpi().max(0.25);
    // 等比放大：窗口捕获抓的是这扇窗的**实际像素**，在直播软件里再拉大只会放大像素
    // （用户实测"放大尺寸也没改善"）。所以放大必须发生在这里：窗口按倍数变大，
    // 内容按同一倍数重新矢量渲染（`LiveCanvas::paint`）。
    let scale = rt.config.live.window_scale.clamp(
        crate::core::config::LIVE_WINDOW_SCALE_MIN,
        crate::core::config::LIVE_WINDOW_SCALE_MAX,
    );
    // 摆位：拖过就回拖过的位置，没拖过（`[-1,-1]`）才按键盘包围盒摆。
    // ★ 画布原点（`vx/vy`）**永远**是键盘包围盒的外沿，与窗口摆在哪儿无关——
    //   两者混用会让键帽在窗里整体偏移。
    let pos = rt.config.live.pos;
    let (x, y) = if pos == [-1, -1] {
        (bx.round() as i32, by.round() as i32)
    } else {
        (pos[0], pos[1])
    };
    Some(Shape {
        x,
        y,
        w: (w * scale / dpi).round().max(16.0) as i32,
        h: (h * scale / dpi).round().max(16.0) as i32,
        bg: [
            rt.config.live.bg[0],
            rt.config.live.bg[1],
            rt.config.live.bg[2],
        ],
        vx: bx.round() as i32,
        vy: by.round() as i32,
        scale,
    })
}

/// 开 / 关 / 改形状对账（挂在覆盖层的 33ms 轮询上，`gif_bar` 的 sync 同族）。
///
/// 本函数**不再**驱动重绘：出不出帧由覆盖层那一侧的帧门决定（生产侧脏标记见
/// `frame.rs::render()`），这里只负责窗的存在性与它的样式挂载。
pub(crate) fn sync(
    ctx: &mut EventCtx,
    rt: &Rc<RefCell<Runtime>>,
    open: &Cell<bool>,
    styled: &Cell<bool>,
    shape: &Cell<Option<Shape>>,
    beat: &Cell<u8>,
    seen_pos: &Cell<(i32, i32)>,
) {
    // 键盘没启用（或方案解析不出来）时 `shape_of` 给 None ⇒ 与"直播捕获关着"同一条路。
    let want = {
        let r = rt.borrow();
        if r.config.live.window_active() {
            shape_of(&r)
        } else {
            None
        }
    };
    let Some(sh) = want else {
        if open.get() {
            crate::platform::win_util::post_close_by_title(LIVE_TITLE);
            open.set(false);
            styled.set(false);
            shape.set(None);
            // 补一帧覆盖层：关掉直播捕获窗之后桌面侧一定要求画键盘（`draws_on_desktop`），
            // 不踢这一帧，键盘会一直不出现直到用户动一下鼠标。
            ctx.mark_dirty_all();
            log::info!("直播捕获窗：已关闭");
        }
        return;
    };
    if !open.get() {
        // 先写画布原点再开窗：新窗的第一帧 paint 就要用对齐后的坐标，晚一步会先黑一帧。
        rt.borrow().live_view.set((sh.vx as f32, sh.vy as f32));
        open_live_window(ctx, rt, &sh);
        open.set(true);
        styled.set(false);
        shape.set(Some(sh));
        // 覆盖层必须出一帧，`live_draw_list` 才有内容（它是帧引擎，本窗只呈现）。
        ctx.mark_dirty_all();
        return;
    }
    if let Some(last) = shape.get() {
        if !sh.same_look(&last) {
            // 键帽大小 / 方案 / 底色 / 画布原点变了：关掉，下一拍按新参数重开。这块表面
            // 没有任何用户状态，重建比"想办法改样式"便宜且不会漏改。位置变化**不**走这里。
            crate::platform::win_util::post_close_by_title(LIVE_TITLE);
            open.set(false);
            return;
        }
        if (sh.x, sh.y) != (last.x, last.y) {
            // 只换了位置（键盘挪了 / 配置改了 / 刚拖完落了盘）：挪过去就行。
            // 用**想要的位置**跟缓存比、不跟窗口实际位置比——拖拽途中窗口位置一直在变，
            // 拿实际位置做判据就会把用户正在拖的窗拽回去。
            if crate::platform::win_util::move_window_silently_by_title(LIVE_TITLE, sh.x, sh.y) {
                shape.set(Some(sh));
            }
        }
    }
    // ★ 样式挂载要重试：`ctx.open_window` 是异步排队的，紧跟开窗调用去改样式必然
    //   「找不到窗口」（B2.8 实锤的那个时序坑，当时置顶落空是同一件事）。
    if !styled.get() {
        // 圆角是 DWM 对带框顶层窗的默认装饰，会啃掉键帽的直角（用户实测：抠图在角上
        // 出错）。要直角必须显式说，而"还没找到窗"不等于"改失败"——前者下一拍再试。
        let Some(rounded_off) = crate::platform::win_util::square_corners_by_title(LIVE_TITLE)
        else {
            return;
        };
        log::info!("直播捕获窗：圆角已关={}", rounded_off);
        // 屏摄排除失败**不**卡在这里：那是观感问题，不是功能不可用（组策略也能整体禁掉它）。
        if EXCLUDE_FROM_SCREEN_CAPTURE {
            let aff = crate::platform::win_util::set_exclude_from_capture_by_title(LIVE_TITLE);
            log::info!("直播捕获窗：屏摄排除={}", aff);
        }
        styled.set(true);
        return;
    }
    // ★ 每 ~15 拍（约 0.5s）核对一次真实状态。每拍都查 = 每拍一次全窗口枚举，那是白付的
    //   代价（B4 刚把这类无条件心跳拆掉，不能转头又装回来）。
    //   只核对一件事：这扇窗还在不在。用户按标题栏的 × 关掉它 = 不想用了（见下）。
    //   最小化不在这里纠正：那是用户的正常操作（B13 用户拍板，照 NohBoard），
    //   代价只是最小化期间直播端拿不到画面——写在《配置说明》里，不替他改回去。
    let n = beat.get().wrapping_add(1);
    if n < 15 {
        beat.set(n);
        return;
    }
    beat.set(0);
    if !crate::platform::win_util::window_exists_by_title(LIVE_TITLE) {
        // ★ 用户按了标题栏的 ×：这就是"我不想用它"，与设置里取消勾选**同一件事**。
        //   取消勾选 + 落盘 + 回灌面板那行信号，而不是把窗再开回来。
        log::info!("直播捕获窗：被关掉 ⇒ 同步取消「直播捕获窗」勾选");
        open.set(false);
        styled.set(false);
        shape.set(None);
        seen_pos.set((-1, -1));
        disable_and_sync_panel(rt);
        ctx.mark_dirty_all();
        return;
    }
    // 拖动对账：连续两拍（≈1s）位置没再变，才认为用户松手了，写进配置。
    // ★ 不做这层节流，一次拖动就是几十次整份 config 落盘。
    // ★ `[-1,-1]` 且窗就在出厂摆位上 ⇒ **不写**：那还是"跟着键盘走"，把包围盒坐标钉下来
    //   会让以后键盘换位置时直播窗留在原地（那是两件不同的事）。
    if let Some((x, y)) = crate::platform::win_util::window_pos_by_title(LIVE_TITLE) {
        let mut r = rt.borrow_mut();
        let untouched = r.config.live.pos == [-1, -1] && (x, y) == (sh.x, sh.y);
        if seen_pos.get() == (x, y) && r.config.live.pos != [x, y] && !untouched {
            r.config.live.pos = [x, y];
            if let Err(e) = r.config.save() {
                log::warn!("直播捕获窗位置落盘失败：{}", e);
            } else {
                log::info!("直播捕获窗：位置记为 ({},{})", x, y);
            }
            // 缓存里的"应摆位置"跟着更新：否则下一拍会认为配置的新坐标 ≠ 缓存的旧坐标，
            // 白挪一次（同坐标 SetWindowPos，无观感影响，但账要多记一笔没必要的动作）。
            shape.set(Some(Shape { x, y, ..sh }));
        } else {
            seen_pos.set((x, y));
        }
    }
}

/// 关窗后把呈现方式收回「桌面」、落盘，并**回灌面板那行选择**。
///
/// ★ 面板的分段控件是 `PanelModel` 里的信号（`from_config` 时造好，之后不会自己回读），
///   所以只改 `rt.config` 的话，面板上还亮着「独立窗口」——用户看到的就又是"两边不同步"。
fn disable_and_sync_panel(rt: &Rc<RefCell<Runtime>>) {
    let mut r = rt.borrow_mut();
    if r.config.live.mode == crate::core::config::LiveMode::Window {
        r.config.live.mode = crate::core::config::LiveMode::Desktop;
        if let Err(e) = r.config.save() {
            log::warn!("直播捕获窗：取消选择落盘失败：{}", e);
        }
    }
    r.panel_uncheck_live_window();
}

fn open_live_window(ctx: &mut EventCtx, rt: &Rc<RefCell<Runtime>>, sh: &Shape) {
    let dpi = {
        let r = rt.borrow();
        crate::runtime::layer::WorldMap::new(r.screen).dpi()
    };
    let canvas = LiveCanvas::new(rt.clone(), dpi, sh.scale);
    log::info!(
        "直播捕获窗：开 @ ({},{}) {}×{}（逻辑，×{:.2}），底色 #{:02x}{:02x}{:02x}，不置顶、带标题栏可拖",
        sh.x,
        sh.y,
        sh.w,
        sh.h,
        sh.scale,
        sh.bg[0],
        sh.bg[1],
        sh.bg[2]
    );

    ctx.open_window(
        Window::new(LIVE_TITLE, sh.w, sh.h)
            .single("live")
            // ★ 带系统标题栏（2026-11-17 用户拍板，照 NohBoard 的形状）：这扇窗是
            //   "一块可以拎走的画板"——能拖到任何位置（含副屏）、能从 × 关掉（= 取消勾选）。
            //   代价两条，都写进《配置说明》：① 捕获软件若没勾「仅客户区」，标题栏会一起
            //   进画面；② 它会挡住自己那一块的鼠标（想拦得少一点就把它拖到不碍事处）。
            .resizable(false)
            .position(sh.x, sh.y)
            .bg(windui::geometry::Color::rgba(
                sh.bg[0], sh.bg[1], sh.bg[2], 255,
            ))
            .content(move || Element::leaf().widget(canvas).fill()),
    );
}

/// 直播画布：把 `live_draw_list` 经与桌面覆盖层同一条渲染路径画出来。
struct LiveCanvas {
    runtime: Rc<RefCell<Runtime>>,
    dpi: f32,
    /// 等比放大倍数：世界物理像素 → 窗口逻辑像素的倍率里，除 dpi 之外那部分。
    scale: f32,
}

impl LiveCanvas {
    fn new(runtime: Rc<RefCell<Runtime>>, dpi: f32, scale: f32) -> Self {
        Self {
            runtime,
            dpi: if dpi.is_finite() && dpi > 0.0 {
                dpi
            } else {
                1.0
            },
            scale: if scale.is_finite() && scale > 0.0 {
                scale
            } else {
                1.0
            },
        }
    }
}

impl Widget for LiveCanvas {
    fn measure(&self, avail: Size, _style: &Style, _text: &mut dyn TextEngine) -> Size {
        avail
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
        canvas.clear_rect(0.0, 0.0, bounds.w as f32, bounds.h as f32);
        let rt = self.runtime.borrow();
        // 世界（虚拟桌面物理坐标）→ 本窗本地逻辑：scale = 1/dpi，平移 = 负原点/dpi。
        // 原点不再是虚拟桌面左上角，而是键盘包围盒的外沿（`Shape::vx/vy`）——这就是
        // "整块桌面的 1:1 大窗"与"只包住键盘的小窗"之间唯一的差别。
        let (vx, vy) = rt.live_view.get();
        // 局部 = (世界 − 原点) × scale / dpi。★ 倍率乘在**渲染**上而不是把位图拉大：
        //   窗口捕获拿到的是窗口的实际像素，这里放大多少，抓到就是多少原图尺寸。
        let s = self.scale / self.dpi;
        crate::runtime::render::translate_offset(&rt.live_draw_list, canvas, s, -vx * s, -vy * s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::{Config, LiveMode};

    /// ★ 换方案/改键帽尺寸时，这扇窗的尺寸与画布原点必须跟着变（B17 用户需求）。
    /// 窗口尺寸不是常量：它来自 `layout_world_bounds` 的并集；这条用例把两个几何
    /// 完全不同的方案摆在一起比，写死尺寸的那一天就会红。
    #[test]
    fn shape_follows_the_keyboard_layout() {
        let mut rt = Runtime::new(Config::default());
        rt.config.virtual_keyboard.enabled = true;
        rt.config.live.mode = LiveMode::Window;
        let a = shape_of(&rt).expect("ansi_104 应算得出窗口");
        rt.config.virtual_keyboard.layout = "ansi_60".into();
        let b = shape_of(&rt).expect("ansi_60 应算得出窗口");
        assert!(
            !a.same_look(&b),
            "换方案后窗口外观必须变（重建路径靠这条判据）：{}×{} vs {}×{}",
            a.w,
            a.h,
            b.w,
            b.h
        );
        assert_ne!((a.w, a.h), (b.w, b.h), "窗口宽高必须按实际键盘走");
    }

    /// ★ 等比放大（B19）：窗口尺寸跟着倍数走，且这是**重建判据**的一部分——
    /// 只改倍数不重建的话，窗还是老尺寸，直播端抓到的依旧是小图。
    #[test]
    fn shape_scales_with_window_scale() {
        let mut rt = Runtime::new(Config::default());
        rt.config.virtual_keyboard.enabled = true;
        rt.config.live.mode = LiveMode::Window;
        let one = shape_of(&rt).expect("1× 应算得出窗口");
        rt.config.live.window_scale = 2.0;
        let two = shape_of(&rt).expect("2× 应算得出窗口");
        assert!(!one.same_look(&two), "改倍数必须触发重建");
        assert_eq!(two.scale, 2.0);
        let rw = two.w as f32 / one.w as f32;
        let rh = two.h as f32 / one.h as f32;
        assert!(
            (1.9..=2.1).contains(&rw) && (1.9..=2.1).contains(&rh),
            "宽高都应≈2 倍，实得 {rw:.2}×{rh:.2}"
        );
    }
}
