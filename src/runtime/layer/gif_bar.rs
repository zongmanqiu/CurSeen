//! GIF 选框的控件条：落在**选框之外**的那一条「尺寸 + 开始/停止 + 裁剪」。
//!
//! ## 为什么它必须在框外
//!
//! 采集是「DXGI 抓整块显示器 → 裁到选框矩形」（见 `platform::recorder::RecOptions::crop`），
//! 所以**框外的任何像素都不会进产物**。这颗按钮要是画在框内，每一帧都会被自己录进去，
//! 产物里就会挂着一条按钮 —— 那不是靠"录制时隐藏 UI"能解决的（本窗口是常驻可见的）。
//!
//! ## 为什么它不是覆盖层的一部分
//!
//! 覆盖层主窗是**穿透**的（用户要能操作下面的窗口），而按钮必须能点到，所以控件条是
//! 一个独立的、不穿透的小窗。它只占选框外一小条，框内区域完全不受遮挡 —— 录制时用户
//! 照样能在框里点、拖、打字。
//!
//! ## 边框画在哪
//!
//! 不在这里。边框由**覆盖层主窗**绘制（`overlay` 读 `Runtime::gif_selection`），因为只有
//! 穿透的画布才能既画满选框一圈、又不挡住框内操作。
//!
//! ## 为什么按钮是**画出来的图形**（2026-09-20 用户口径）
//!
//! 用户要的是"播放器上那两颗键"的直觉：**开始/停止共用一颗键**，未录时是绿色三角形、
//! 录起来变成红色圆形；裁剪是一把剪刀。文字标签做不到这个（"开始/停止"四个字反而要求
//! 用户先读再想）。所以整条控件条改成自绘画布 —— 与裁剪窗同一套做法（`Widget::paint`）。
//!
//! ★ 「完成」按钮**已删除**（用户："我想象不出完成按钮有什么必要，实际也没用过"）。
//!   收起选框仍然只有一条路：再按一次 GIF 快捷键（`Runtime::begin_gif_selection`）。
//!   点「裁剪」也会顺带把它收掉（进裁剪窗的语义就是"这段录完了"）。
//!
//! ## 为什么它必须置顶（补丁 014 的同源问题）
//!
//! `Window::new` 建的普通窗**不带 `WS_EX_TOPMOST`**（windui 的 `ex_style` 只在
//! `cfg.layered` 时加），所以控件条原先会被任何被抬到前面的普通窗口盖住 —— 用户实测
//! "选框有时会被其它软件窗口遮挡"。开窗后补一次 [`win_util::set_topmost_by_title`]
//! （**不抢焦点**），之后由 `TopmostKeeper::guard_aux` 每 250ms 对账重插。

use crate::core::geometry::ScreenRect;
use crate::runtime::Runtime;
use std::cell::RefCell;
use std::rc::Rc;
use windui::core::{EventCtx, Widget};
use windui::event::{Event, MouseButton, PointerKind};
use windui::geometry::{Color, Rect};
use windui::prelude::*;
use windui::render::Canvas;
use windui::render::Paint as WPaint;
use windui::spec::Align;
use windui::text::{TextEngine, TextStyle};

/// 窗口标题（按标题找窗 / 日志 / 置顶守护用）。
pub const BAR_TITLE: &str = "CurSeen GIF";

/// 长截图控件条的窗口标题（2026-10-09 双条共存）。
///
/// ★ 两条流的控件条现在是**两个独立窗口**（用户要求 GIF 选框与长截图选框可以同时
///   挂着、各自的按钮条都要在）。按标题找窗的一族 API（置顶 / 关闭 / 跟随 / 重画）
///   都靠标题区分，所以标题必须不同——同名会让 `FindWindowW` 只找到其中一个。
pub const SCROLL_BAR_TITLE: &str = "CurSeen Scroll";

/// 模式 → 各自的窗口标题。
pub(crate) fn bar_title(mode: BarMode) -> &'static str {
    match mode {
        BarMode::Rec => BAR_TITLE,
        BarMode::Scroll => SCROLL_BAR_TITLE,
    }
}

/// 模式 → windui 的单实例标签（`Window::single`）。同名窗只允许一个——
/// 两条流的标签必须不同，否则第二条开不出来。
pub(crate) fn bar_tag(mode: BarMode) -> &'static str {
    match mode {
        BarMode::Rec => "gif_bar",
        BarMode::Scroll => "scroll_bar",
    }
}

/// 控件条服务的对象。两条流（录制 / 长截图）**各开一条窗**（2026-10-09 双条共存，
/// 用户要求两个选框能同时挂着），按钮集与图标语义按此分岔。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BarMode {
    /// GIF / 选区 MP4 录制：三颗键（开始/停止 + 裁剪 + ✕）。
    Rec,
    /// 长截图：两颗键（开始/停止 + ✕），没有"裁剪"这种产物编辑语义。
    Scroll,
}

/// 控件条高度与最小宽度（**逻辑像素**；`Window::new` 吃逻辑 dp，`Window::position` 吃物理像素）。
/// 宽度不取固定值——按尺寸文本自适应（见 [`bar_rect`]），`BAR_MIN_W` 只是保底下限。
const BAR_MIN_W: i32 = 208;
const BAR_H: i32 = 44;

/// 控件条与选框之间留的缝（物理像素）。
const GAP: i32 = 8;

/// 控件条内的留白与按钮尺寸（逻辑像素）。
const PAD: f32 = 12.0;
const BTN: f32 = 30.0;
const BTN_GAP: f32 = 8.0;

/// 12px 尺寸标签每个字符的估算宽度（逻辑像素）。够宽不裁字：
/// 9920→4 位数字 × 约 8px；" × " 三个字符再 +3。真机略窄也只会让条稍宽一点，无妨。
const LABEL_CHAR_W: f32 = 8.0;

/// 按钮列的固定总宽（三颗按钮 + 两处缝），`zones` 与本函数共用同一套常量。
fn cluster_w() -> f32 {
    3.0 * BTN + 2.0 * BTN_GAP
}

/// 按钮列总宽（按模式）。长截图没有剪刀，两颗键。
fn cluster_w_for(mode: BarMode) -> f32 {
    match mode {
        BarMode::Rec => cluster_w(),
        BarMode::Scroll => 2.0 * BTN + BTN_GAP,
    }
}

/// 选框尺寸文本的字符数（"宽 × 高" 各自位数 + " × " 3 个字符）。
fn label_chars(sel: ScreenRect) -> usize {
    let d = |v: i32| v.abs().to_string().len();
    d(sel.w) + d(sel.h) + 3
}

/// 内容宽度（按模式）。
fn content_w_for(sel: ScreenRect, mode: BarMode) -> i32 {
    (2.0 * PAD + label_chars(sel) as f32 * LABEL_CHAR_W + BTN_GAP + cluster_w_for(mode)).ceil()
        as i32
}

/// 「开始」的绿 —— 刻意不用设计令牌里的蓝色：这颗键要的是**播放器的语义**，
/// 与界面强调色混在一起反而看不出"这是开始"。
const START_GREEN: (u8, u8, u8) = (46, 170, 96);
/// 「停止」的红 —— 与边框"录制中变红"同一个色，一眼知道"框和键说的是同一件事"。
const STOP_RED: (u8, u8, u8) = (220, 68, 60);

const BG: (u8, u8, u8) = (255, 255, 255);
const TEXT_DIM: (u8, u8, u8) = (99, 110, 114);
const TEXT: (u8, u8, u8) = (45, 52, 54);
const HOVER_BG: (u8, u8, u8) = (226, 230, 234);

fn color(c: (u8, u8, u8), a: u8) -> Color {
    Color::rgba(c.0, c.1, c.2, a)
}

/// 控件条落点与尺寸：选框**正下方、右对齐**（条右缘贴选框右缘，与"左上到右下"拉框的
/// 落点同侧，视线不用跳），宽度按尺寸文本自适应（见 [`content_w_for`]；下方放不下就翻到上方。
///
/// ★ 右对齐理由（用户 2026-09-20）：GIF 选框是"左上到右下"拖出来的，选框右缘就是收笔的
///   位置，按钮跟着右缘放最顺手。
///
/// ★ 两个方向都必须在选框**外**：压到框上就会被裁进 GIF（见本模块头注释）。
/// ★ 钳进虚拟桌面：副屏在主屏左侧时 `vs.x` 为负，直接 clamp 会算错，所以按
///   `[vs.x, vs.x + vs.w]` 这个区间钳，而不是从 0 开始。
///
/// 返回 `(x, y, w, h)`（**物理像素**）。
pub fn bar_rect(sel: ScreenRect, vs: ScreenRect) -> (i32, i32, i32, i32) {
    bar_rect_for(sel, vs, BarMode::Rec)
}

/// [`bar_rect`] 的模式感知版（长截图两颗键，条更窄一点）。
pub(crate) fn bar_rect_for(sel: ScreenRect, vs: ScreenRect, mode: BarMode) -> (i32, i32, i32, i32) {
    let left = vs.x;
    let right = vs.x + vs.w;
    let top = vs.y;
    let bottom = vs.y + vs.h;

    // 宽度按尺寸文本自适应（越宽/数字越多条越宽），只保底 `BAR_MIN_W`、上限虚拟桌面宽，
    // 不再跟选框的几何宽度走——选框很宽时条不至于被拉成一条长板。
    let w = content_w_for(sel, mode).max(BAR_MIN_W).min(vs.w).max(1);
    // 右对齐选框右边缘（条右缘 = 选框右缘），再钳进虚拟桌面。
    let x = (sel.x + sel.w - w).clamp(left, (right - w).max(left));

    // 下方优先（与截图尺寸标识同侧，视线不用跳）。
    let below = sel.y + sel.h + GAP;
    let y = if below + BAR_H <= bottom {
        below
    } else {
        // 翻到上方。
        let above = sel.y - BAR_H - GAP;
        if above >= top {
            above
        } else {
            // 选框几乎占满整屏高：上下都放不下。这时只能选一侧并钳进屏幕，
            // 宁可压住选框边缘（极端情况下的产物会带一条边），也不让按钮跑出屏幕外。
            (bottom - BAR_H).max(top)
        }
    };
    (x, y, w, BAR_H)
}

// ─────────────────────────── 纯几何（门禁钉在这里） ───────────────────────────

/// 本模块内部的矩形（浮点）。整数/浮点的单位问题见裁剪窗模块头的说明。
#[derive(Clone, Copy, Debug, PartialEq)]
struct Box2 {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Box2 {
    const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// 浮点矩形 → windui 要的整数矩形（四舍五入）。取整只发生在绘制前的最后一刻。
fn irect(b: Box2) -> Rect {
    Rect::new(
        b.x.round() as i32,
        b.y.round() as i32,
        b.w.round() as i32,
        b.h.round() as i32,
    )
}

/// 控件条上的三块按钮。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Btn {
    /// 开始 / 停止（同一颗键）。
    Toggle,
    /// 裁剪。
    Trim,
    /// 关闭（✕）：结束 GIF 录制并收起选框。
    Close,
}

/// 一条控件条里各块的逻辑矩形。布局是**纯函数**，所以能脱离窗口直接测。
#[derive(Clone, Copy, Debug, PartialEq)]
struct Zones {
    /// 尺寸标签（"1920 × 1080"）。
    label: Box2,
    toggle: Box2,
    trim: Box2,
    close: Box2,
}

/// 布局：标签靠左、三颗按钮靠右（✕ 最右，裁剪中间，开始/停止最左），全部在窗口内。
fn zones(w: f32, h: f32) -> Zones {
    let cy = (h - BTN) / 2.0;
    // 从右往左排：✕ 关窗最右（边缘位置，不易误触），裁剪在中间，
    // 开始/停止最左（用得最多，拖框时手不用挪到最角上）。
    let close = Box2::new(w - PAD - BTN, cy, BTN, BTN);
    let trim = Box2::new(close.x - BTN_GAP - BTN, cy, BTN, BTN);
    let toggle = Box2::new(trim.x - BTN_GAP - BTN, cy, BTN, BTN);
    let label = Box2::new(PAD, 0.0, (toggle.x - PAD - BTN_GAP).max(1.0), h);
    Zones {
        label,
        toggle,
        trim,
        close,
    }
}

/// 布局（按模式）。长截图只有两颗键：✕ 最右、开始/停止在它左边；`trim` 是
/// **零尺寸**框——`contains` 恒 false，命中判定与绘制都自然跳过，不需要第二套类型。
fn zones_for(w: f32, h: f32, mode: BarMode) -> Zones {
    if mode == BarMode::Rec {
        return zones(w, h);
    }
    let cy = (h - BTN) / 2.0;
    let close = Box2::new(w - PAD - BTN, cy, BTN, BTN);
    let toggle = Box2::new(close.x - BTN_GAP - BTN, cy, BTN, BTN);
    let label = Box2::new(PAD, 0.0, (toggle.x - PAD - BTN_GAP).max(1.0), h);
    Zones {
        label,
        toggle,
        trim: Box2::new(0.0, 0.0, 0.0, 0.0),
        close,
    }
}

/// 命中的是哪颗按钮。`None` = 空白处（窗口不做拖动，直接忽略）。
fn hit(z: &Zones, x: f32, y: f32) -> Option<Btn> {
    if z.toggle.contains(x, y) {
        return Some(Btn::Toggle);
    }
    if z.trim.contains(x, y) {
        return Some(Btn::Trim);
    }
    if z.close.contains(x, y) {
        return Some(Btn::Close);
    }
    None
}

// ─────────────────────────── 画布（窗口内容） ───────────────────────────

struct BarCanvas {
    rt: Rc<RefCell<Runtime>>,
    /// 选框尺寸文本的**兜底值**（开窗时算一次）。正常路径下 paint 每帧动态读当前
    /// 选框（长截图就绪态可拖拽调整、GIF 未上膛也能拖——旧实现拖动后标签停在旧值，
    /// 2026-10-01 用户要求实时显示）；读不到（极端窗口期）才退到这个兜底。
    size_text: String,
    /// 服务对象（录制 / 长截图），开窗时定。窗随流生死（`sync_gif_bar` 按 selection
    /// 对账），每条流一条窗、不同标题（`bar_title`）。
    mode: BarMode,
    hover: Option<Btn>,
    /// 左键按下的那颗（松手时要求与按下同一颗才算"点击"）。
    press: Option<Btn>,
}

impl BarCanvas {
    fn new(rt: Rc<RefCell<Runtime>>, size_text: String, mode: BarMode) -> Self {
        Self {
            rt,
            size_text,
            mode,
            hover: None,
            press: None,
        }
    }

    /// 这一刻的"上膛"态（红圆）判定：录制看 `gif_armed()`，长截图看会话是否活着。
    fn armed_now(&self) -> bool {
        match self.mode {
            BarMode::Rec => self.rt.borrow().gif_armed(),
            BarMode::Scroll => self.rt.borrow().scroll_active(),
        }
    }

    /// 这一刻的选框尺寸文本（**动态**读当前选框，拖动/缩放实时跟随）。
    fn size_text_now(&self) -> String {
        let sel = {
            let g = self.rt.borrow();
            match self.mode {
                BarMode::Rec => g.gif_selection(),
                BarMode::Scroll => g.scroll_bar_selection().map(|(s, _)| s),
            }
        };
        sel.map(|s| format!("{} × {}", s.w, s.h))
            .unwrap_or_else(|| self.size_text.clone())
    }

    /// 开始/停止：未上膛 = 绿色三角（向右），上膛 = 红色实心圆。
    fn paint_toggle(&self, canvas: &mut dyn Canvas, z: Box2, armed: bool) {
        if self.hover == Some(Btn::Toggle) {
            canvas.fill_round_rect(z.x, z.y, z.w, z.h, 7.0, &WPaint::fill(color(HOVER_BG, 170)));
        }
        let (cx, cy) = (z.x + z.w / 2.0, z.y + z.h / 2.0);
        if armed {
            canvas.fill_circle(cx, cy, 7.5, &WPaint::fill(color(STOP_RED, 255)));
        } else {
            // 三角形的视觉重心偏左，所以整体往右挪一点才"看着居中"。
            let r = 8.5;
            let pts = [
                (cx - r * 0.55, cy - r),
                (cx - r * 0.55, cy + r),
                (cx + r * 0.90, cy),
            ];
            canvas.fill_polygon(&[&pts], &WPaint::fill(color(START_GREEN, 255)));
        }
    }

    /// 剪刀：两个把手（圆环）+ 两片交叉的刃。
    fn paint_scissors(&self, canvas: &mut dyn Canvas, z: Box2) {
        if self.hover == Some(Btn::Trim) {
            canvas.fill_round_rect(z.x, z.y, z.w, z.h, 7.0, &WPaint::fill(color(HOVER_BG, 170)));
        }
        let ink = WPaint::fill(color(TEXT, 255));
        let (cx, cy) = (z.x + z.w / 2.0, z.y + z.h / 2.0);

        // 把手：两个小圆环（stroke 一个圆角矩形、半径取半边长）。
        let (hx, hy, rr) = (4.8, 5.6, 3.0);
        for dx in [-hx, hx] {
            canvas.stroke_round_rect(
                cx + dx - rr,
                cy + hy - rr,
                rr * 2.0,
                rr * 2.0,
                rr,
                1.6,
                &ink,
            );
        }
        // 刃：从把手内侧往上交叉，两片互相越过中线 —— 这是"剪刀"唯一的辨识特征。
        canvas.draw_line(
            cx - hx + 1.0,
            cy + hy - rr,
            cx + hx + 2.2,
            cy - hy - 2.4,
            1.8,
            &ink,
        );
        canvas.draw_line(
            cx + hx - 1.0,
            cy + hy - rr,
            cx - hx - 2.2,
            cy - hy - 2.4,
            1.8,
            &ink,
        );
    }

    /// ✕ 关闭：两条交叉短线（居中、略小于按钮边框，避免贴边像"画歪了"）。
    fn paint_close(&self, canvas: &mut dyn Canvas, z: Box2) {
        if self.hover == Some(Btn::Close) {
            canvas.fill_round_rect(z.x, z.y, z.w, z.h, 7.0, &WPaint::fill(color(HOVER_BG, 170)));
        }
        let ink = WPaint::fill(color(TEXT, 255));
        let (cx, cy) = (z.x + z.w / 2.0, z.y + z.h / 2.0);
        let r = 6.0; // ✕ 的半臂长
        let t = 1.8; // 线粗
        canvas.draw_line(cx - r, cy - r, cx + r, cy + r, t, &ink);
        canvas.draw_line(cx + r, cy - r, cx - r, cy + r, t, &ink);
    }
}

impl Widget for BarCanvas {
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
        let (bx, by, bw, bh) = (
            bounds.x as f32,
            bounds.y as f32,
            bounds.w as f32,
            bounds.h as f32,
        );
        canvas.clear_rect(bx, by, bw, bh);
        canvas.fill_rect(bx, by, bw, bh, &WPaint::fill(color(BG, 255)));

        let z = zones_for(bw, bh, self.mode);
        let armed = self.armed_now();
        let size_text = self.size_text_now();

        canvas.draw_text(
            &size_text,
            irect(z.label),
            color(TEXT_DIM, 255),
            Align::Start,
            &TextStyle::new(12.0),
        );
        self.paint_toggle(canvas, z.toggle, armed);
        // 剪刀只有录制模式有（长截图没有"裁剪产物"语义）。
        if self.mode == BarMode::Rec {
            self.paint_scissors(canvas, z.trim);
        }
        self.paint_close(canvas, z.close);
    }

    fn on_event(&mut self, ctx: &mut EventCtx, ev: &Event) -> bool {
        let Event::Pointer(pe) = ev else {
            return false;
        };
        let (x, y) = (pe.pos.x as f32, pe.pos.y as f32);
        let z = zones_for(ctx.bounds().w as f32, ctx.bounds().h as f32, self.mode);
        match pe.kind {
            PointerKind::Move => {
                let h = hit(&z, x, y);
                if h != self.hover {
                    self.hover = h;
                    ctx.mark_dirty_all();
                }
                // 不吞 Move：它只是 hover 反馈，没有"必须由我处理"的语义。
                false
            }
            PointerKind::Down if pe.button == MouseButton::Left => {
                self.press = hit(&z, x, y);
                // 按在空白处不改状态：返回 true 只是把这一下"吃掉"，免得漏给下层。
                true
            }
            PointerKind::Up if pe.button == MouseButton::Left => {
                let b = hit(&z, x, y);
                let pressed = self.press.take();
                // ★ 松手要求与按下是**同一颗**：按在裁剪上滑到开始再松手不该触发开始。
                if b.is_some() && b == pressed {
                    let mut g = self.rt.borrow_mut();
                    match (self.mode, b) {
                        (BarMode::Rec, Some(Btn::Toggle)) => g.toggle_gif_from_bar(),
                        (BarMode::Rec, Some(Btn::Trim)) => g.request_gif_trim(),
                        (BarMode::Rec, Some(Btn::Close)) => g.close_gif_bar(),
                        (BarMode::Scroll, Some(Btn::Toggle)) => g.toggle_scroll_from_bar(),
                        // 长截图的 ✕ = 取消（就绪态清掉；采集中丢弃画布），与 Esc 同义。
                        (BarMode::Scroll, Some(Btn::Close)) => g.cancel_scroll(),
                        _ => {}
                    }
                    drop(g);
                    // 图标要立刻跟着状态换（▶↔●）：等 250ms 对账会有一拍滞后。
                    ctx.mark_dirty_all();
                }
                true
            }
            _ => false,
        }
    }
}

/// 打开**指定模式**的控件条（2026-10-09 双条共存：每条流一个独立窗口）。
///
/// 只读自己模式的选框：Rec 读 `gif_selection`，Scroll 读 `scroll_bar_selection`；
/// 该模式的选框不在就什么都不做（开/关的幂等对账在 `overlay::sync_gif_bar`）。
pub(crate) fn open(ctx: &mut EventCtx, rt: &Rc<RefCell<Runtime>>, mode: BarMode) {
    let (sel, vs) = {
        let r = rt.borrow();
        let vs = r.screen;
        let sel = match mode {
            BarMode::Rec => r.gif_selection(),
            BarMode::Scroll => r.scroll_bar_selection().map(|(s, _)| s),
        };
        match sel {
            Some(s) => (s, vs),
            None => return,
        }
    };
    let title = bar_title(mode);
    let (x, y, w, h) = bar_rect_for(sel, vs, mode);
    let size_text = format!("{} × {}", sel.w, sel.h);

    let canvas = BarCanvas::new(rt.clone(), size_text.clone(), mode);
    let rt_done = rt.clone();
    log::info!("控件条开（{mode:?}）@ ({x},{y}) {w}×{h}，选框 {size_text}");

    ctx.open_window(
        Window::new(title, w, h)
            .single(bar_tag(mode))
            // ★ 无边框 + 不可缩放：这是贴在屏幕上的一条工具，不是对话框。
            //   配合 windui 补丁 022-a，外圈 8 逻辑像素不再是缩放热区。
            .frameless(true)
            .resizable(false)
            .position(x, y)
            .bg(crate::ui::tokens::to_color(crate::ui::tokens::BG))
            .on_close_request(move |_ctx| {
                // 关窗（Alt+F4 / 系统菜单）= 收起，与再按一次快捷键同义。
                // ★ 关窗**只改状态**，窗口的生死由 `overlay::sync_gif_bar` 对账，
                //   所以这里不必也不能自己再发一条 WM_CLOSE（会自吞一次）。
                let mut g = rt_done.borrow_mut();
                if mode == BarMode::Scroll {
                    // 长截图：✕ = 取消（就绪清掉 / 采集丢弃画布）。
                    g.cancel_scroll();
                } else {
                    g.close_gif_bar();
                }
                true
            })
            .content(move || Element::leaf().widget(canvas).fill()),
    );

    // ★ 补一次置顶（**不抢焦点**）：`Window::new` 建的普通窗不带 `WS_EX_TOPMOST`，
    //   不补就会被任何被抬到前面的窗口盖住。之后的维持交给 `TopmostKeeper::guard_aux`。
    if !crate::platform::win_util::set_topmost_by_title(title) {
        log::warn!("GIF 控件条置顶失败（找不到窗口或 SetWindowPos 失败）");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一块 1920×1080 的虚拟桌面（原点 0,0）。
    fn vs() -> ScreenRect {
        ScreenRect {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
        }
    }

    /// ★ 双条共存（2026-10-09 用户要求 GIF 选框与长截图选框同时挂、按钮条都要在）：
    /// 两条流的控件条是**两个独立窗口**——标题与单实例标签必须互不相同。
    /// 标题同名会让 `FindWindowW` 一族（置顶/关闭/跟随/重画）只命中一个；
    /// 标签同名会让 windui 的 `Window::single` 拒开第二个。
    #[test]
    fn rec_and_scroll_bars_have_distinct_titles_and_tags() {
        assert_ne!(bar_title(BarMode::Rec), bar_title(BarMode::Scroll));
        assert_ne!(bar_tag(BarMode::Rec), bar_tag(BarMode::Scroll));
        assert_eq!(bar_title(BarMode::Rec), BAR_TITLE);
        assert_eq!(bar_title(BarMode::Scroll), SCROLL_BAR_TITLE);
    }

    fn sel(x: i32, y: i32, w: i32, h: i32) -> ScreenRect {
        ScreenRect { x, y, w, h }
    }

    #[test]
    fn bar_sits_below_the_selection_by_default() {
        let (x, y, w, h) = bar_rect(sel(100, 100, 600, 400), vs());
        // 宽度按尺寸文本自适应（不是选框几何宽度）。
        assert_eq!(w, content_w_for(sel(100, 100, 600, 400), BarMode::Rec));
        // 右对齐：条的右缘 = 选框右缘（100 + 600 = 700）。
        assert_eq!(x + w, 700);
        // 下方：条的顶边 = 选框底边 + 缝。
        assert_eq!(y, 100 + 400 + GAP);
        assert_eq!(h, BAR_H);
    }

    #[test]
    fn bar_follows_the_selection_as_it_moves_and_resizes() {
        // 移动：选框整体平移，条跟着平移（右缘始终贴选框右缘）。
        for (dx, dy) in [(0, 0), (50, 30), (-60, 10)] {
            let s = sel(100 + dx, 100 + dy, 600, 400);
            let (x, y, w, _) = bar_rect(s, vs());
            assert_eq!(
                (x + w, y, w),
                (
                    100 + dx + 600,
                    100 + dy + 400 + GAP,
                    content_w_for(s, BarMode::Rec)
                )
            );
        }
        // 缩放：选框变宽，条**宽度不变**（只看尺寸文本位数，600 和 900 都是 3 位），
        // 但右缘跟着选框右缘右移（100+600 → 100+900）。
        let (x0, y0, w0, _) = bar_rect(sel(100, 100, 600, 400), vs());
        let (x1, y1, w1, _) = bar_rect(sel(100, 100, 900, 400), vs());
        assert_eq!(w0, w1, "条宽不应跟随选框几何宽度变化");
        assert_eq!(x1 + w1, x0 + w0 + 300, "右缘应跟着选框右缘右移 300");
        assert_eq!(y0, y1);
    }

    #[test]
    fn bar_uses_min_width_when_the_selection_is_narrow() {
        // 选框极窄（5×5）时尺寸文本只有 5 字符，算出来 < BAR_MIN_W，条保底到 BAR_MIN_W。
        // ★ 选框放靠右：若放左缘（x=100），右对齐会把条推出左屏边被钳回 0，就测不到"贴右"了。
        let s = sel(1600, 100, 5, 5);
        let (x, _, w, _) = bar_rect(s, vs());
        assert!(
            content_w_for(s, BarMode::Rec) < BAR_MIN_W,
            "前置条件：content_w 必须 < BAR_MIN_W 才测得到兜底"
        );
        assert_eq!(w, BAR_MIN_W);
        assert_eq!(x + w, 1605, "右缘仍贴选框右边缘");
    }

    #[test]
    fn bar_flips_above_when_the_selection_touches_the_bottom() {
        // 选框底边贴着屏幕底边，下方放不下 → 翻到上方。
        let s = sel(100, 700, 600, 380);
        let (_, y, _, _) = bar_rect(s, vs());
        assert_eq!(y, 700 - BAR_H - GAP);
        assert!(y + BAR_H <= s.y, "翻上去之后必须完全在选框外");
    }

    #[test]
    fn bar_never_leaves_the_virtual_desktop() {
        // 选框贴着右边缘：按选框宽展开后会溢出，必须被钳回来。
        let v = vs();
        let s = sel(1800, 100, 119, 400);
        let (x, _, w, _) = bar_rect(s, v);
        assert!(x >= v.x, "不能跑到虚拟桌面左边外");
        assert!(x + w <= v.x + v.w, "不能跑到虚拟桌面右边外");
    }

    #[test]
    fn bar_handles_a_negative_virtual_desktop_origin() {
        // 副屏在主屏左侧：虚拟桌面原点为负，钳制区间是 [-1920, 0]，不是 [0, 1920]。
        let v = ScreenRect {
            x: -1920,
            y: 0,
            w: 3840,
            h: 1080,
        };
        let s = sel(-1900, 100, 600, 400);
        let (x, _, w, _) = bar_rect(s, v);
        assert!(x >= v.x, "负原点下不能跑到左边界外: {x}");
        assert!(x + w <= v.x + v.w, "负原点下不能跑到右边界外: {x}");
    }

    #[test]
    fn a_full_height_selection_still_gets_an_onscreen_bar() {
        // 选框占满整屏高：上下都放不下，退化路径必须仍把条留在屏幕内。
        let v = vs();
        let (x, y, w, _) = bar_rect(sel(0, 0, 1920, 1080), v);
        assert!(y >= v.y && y + BAR_H <= v.y + v.h, "y={y} 跑出屏幕");
        assert!(x >= v.x && x + w <= v.x + v.w, "x={x} 跑出屏幕");
    }

    // ─────────────────── 内部布局（这次重写新增） ───────────────────

    /// 内部布局测试用的固定宽度（最小宽度，够塞三颗按钮+标签）。
    fn z() -> Zones {
        zones(BAR_MIN_W as f32, BAR_H as f32)
    }

    /// 三颗按钮都得在窗口内、互不重叠、且**竖直居中**（用户对居中的定义：上下留白相等）。
    #[test]
    fn buttons_are_inside_and_vertically_centered() {
        let h = BAR_H as f32;
        let z = z();
        for b in [z.toggle, z.trim, z.close] {
            assert!(
                b.x >= 0.0 && b.x + b.w <= BAR_MIN_W as f32,
                "按钮出界：{b:?}"
            );
            assert!(
                (b.y - (h - b.h) / 2.0).abs() < 0.01,
                "按钮没有竖直居中：{b:?}"
            );
        }
        // 三颗按钮互不重叠。
        assert!(z.toggle.x + z.toggle.w <= z.trim.x, "toggle 与 trim 重叠");
        assert!(z.trim.x + z.trim.w <= z.close.x, "trim 与 close 重叠");
    }

    /// 标签必须**落在按钮左侧且不相交**，否则长尺寸文本会压到按钮上。
    #[test]
    fn label_stops_before_the_first_button() {
        let z = z();
        assert!(z.label.x + z.label.w <= z.toggle.x, "标签压住了按钮");
        assert!(z.label.w > 0.0, "标签宽度被算成 0，文本会画不出来");
    }

    /// 命中判定：按钮中心命中自己、按钮外一律 `None`；标签区**不**可点。
    #[test]
    fn hit_testing_matches_the_buttons_only() {
        let z = z();
        let mid = |b: Box2| (b.x + b.w / 2.0, b.y + b.h / 2.0);
        assert_eq!(hit(&z, mid(z.toggle).0, mid(z.toggle).1), Some(Btn::Toggle));
        assert_eq!(hit(&z, mid(z.trim).0, mid(z.trim).1), Some(Btn::Trim));
        assert_eq!(hit(&z, mid(z.close).0, mid(z.close).1), Some(Btn::Close));
        // 标签区（左上角）不是按钮。
        assert_eq!(hit(&z, z.label.x + 2.0, z.label.y + 2.0), None);
        // toggle 与 trim 之间的缝。
        assert_eq!(
            hit(&z, z.toggle.x + z.toggle.w + BTN_GAP / 2.0, mid(z.toggle).1),
            None
        );
        // trim 与 close 之间的缝。
        assert_eq!(
            hit(&z, z.trim.x + z.trim.w + BTN_GAP / 2.0, mid(z.trim).1),
            None
        );
        // 窗口右下角的边距内（close 右边到窗口右缘）。
        assert_eq!(hit(&z, BAR_MIN_W as f32 - 2.0, BAR_H as f32 - 2.0), None);
    }

    // ─────────────────── 离屏渲染（图形到底画对了没） ───────────────────

    use crate::core::config::Config;
    use windui::render::{DibTarget, RenderTarget};
    use windui::text::DWriteEngine;

    /// 把控件条离屏渲成一张不透明位图（dpi=1，逻辑坐标 1:1）。
    fn render(c: &BarCanvas) -> Vec<u8> {
        let (pw, ph) = (BAR_MIN_W as u32, BAR_H as u32);
        let mut ann = vec![0u8; (pw * ph * 4) as usize];
        let mut engine = DWriteEngine::new();
        {
            let mut target = DibTarget::new(&mut ann, pw, ph);
            let mut canvas = target.make_canvas(&mut engine, 1.0);
            Widget::paint(
                c,
                Rect::new(0, 0, BAR_MIN_W, BAR_H),
                Rect::new(0, 0, BAR_MIN_W, BAR_H),
                false,
                true,
                &mut *canvas,
                &Style::default(),
            );
        }
        ann
    }

    fn px(buf: &[u8], x: i32, y: i32) -> (u8, u8, u8) {
        let i = ((y as u32 * BAR_MIN_W as u32 + x as u32) * 4) as usize;
        (buf[i], buf[i + 1], buf[i + 2])
    }

    /// 到期望色的三通道偏差和。
    fn dev(a: (u8, u8, u8), b: (u8, u8, u8)) -> i32 {
        (a.0 as i32 - b.0 as i32).abs()
            + (a.1 as i32 - b.1 as i32).abs()
            + (a.2 as i32 - b.2 as i32).abs()
    }

    /// 一块区域里"上了色"的像素数：与 `want` 的偏差不超过 `tol` 的那些。
    ///
    /// 为什么不取"主色"或采样单点：图形只占按钮的一小块（30×30 里三角约 105 px、
    /// 圆约 177 px），主色**永远是背景白**；而单点采样会正好落在抗锯齿的半透明边缘上。
    /// 数"有多少像素是绿的"才既稳又直接对应"看得出是绿的"。
    fn count_near(buf: &[u8], r: Box2, want: (u8, u8, u8), tol: i32) -> usize {
        (r.y as i32..(r.y + r.h) as i32)
            .flat_map(|y| (r.x as i32..(r.x + r.w) as i32).map(move |x| (x, y)))
            .filter(|&(x, y)| dev(px(buf, x, y), want) <= tol)
            .count()
    }

    fn canvas(armed: bool) -> BarCanvas {
        let mut rt = Runtime::new(Config::default());
        if armed {
            rt.armed_gif_for_test();
        }
        BarCanvas::new(
            Rc::new(RefCell::new(rt)),
            "1920 × 1080".to_string(),
            BarMode::Rec,
        )
    }

    /// 长截图模式（就绪态）的画布：绿三角。采集态（红圆）的像素判据不在这里钉——
    /// 那需要真的起抓屏线程，而 `armed_now`（Scroll 支路）只是 `scroll_active()` 的
    /// 直读，状态机语义由 `scroll.rs` 的门禁测试覆盖。
    fn scroll_canvas() -> BarCanvas {
        let mut rt = Runtime::new(Config::default());
        rt.scroll_ready = Some(crate::runtime::scroll::ScrollReady {
            rect: crate::core::geometry::ScreenRect::new(0, 0, 320, 240),
            horizontal: false,
        });
        BarCanvas::new(
            Rc::new(RefCell::new(rt)),
            "320 × 240".to_string(),
            BarMode::Scroll,
        )
    }

    /// **核心门禁**：未录制时要看到**绿色三角形**，录制中要看到**红色圆形**，两者互斥。
    ///
    /// 用户的原话是"未录制时是绿色三角形，点击后变成红色圆形" —— 这条把它变成像素判据。
    /// 绕开真机（提权窗口收不到鼠标注入，见 `examples/render_probe.rs`）。
    #[test]
    fn offscreen_render_switches_green_triangle_for_red_circle() {
        let z = z();
        let idle = render(&canvas(false));
        let rec = render(&canvas(true));

        // 三角 ≈ 1/2 · 17 · 15.3 ≈ 130 px；圆 ≈ π · 7.5² ≈ 177 px。阈值取在两者之下。
        let tri = count_near(&idle, z.toggle, START_GREEN, 60);
        let cir = count_near(&rec, z.toggle, STOP_RED, 60);
        assert!(tri > 60, "未录制时应看到绿色三角形，绿像素只有 {tri} 个");
        assert!(cir > 100, "录制中应看到红色圆形，红像素只有 {cir} 个");

        // 互斥：两个状态不能混色（说明画的是同一颗键的两副面孔，而不是叠了两层）。
        assert_eq!(
            count_near(&idle, z.toggle, STOP_RED, 60),
            0,
            "未录制时混进了红色"
        );
        assert_eq!(
            count_near(&rec, z.toggle, START_GREEN, 60),
            0,
            "录制中混进了绿色"
        );

        // 三角与圆"长得不一样"：同外接尺寸下三角形面积必须明显小一截。
        assert!(
            tri < cir,
            "三角形应比同外接圆的面积小（三角 {tri} px vs 圆 {cir} px）"
        );

        // 白色面板（按钮之外一定是纯白，说明没画出多余色块）。
        assert_eq!(px(&idle, 3, 3), BG, "左上角应是白面板");
    }

    /// 剪刀必须真的落在裁剪按钮那块里（两个把手 + 交叉的刃 ≈ 一堆深色像素）。
    #[test]
    fn offscreen_render_draws_scissors_in_the_trim_button() {
        let z = z();
        let f = render(&canvas(false));

        // 剪刀是**细笔画**（两个圆环 + 两条交叉的刃）：判据取"不是面板白"，才不会被
        // 抗锯齿的边缘像素漏掉 —— 只数"很深的墨色"会把轮廓丢一大半。
        let ink = |r: Box2| -> usize {
            (r.y as i32..(r.y + r.h) as i32)
                .flat_map(|y| (r.x as i32..(r.x + r.w) as i32).map(move |x| (x, y)))
                .filter(|&(x, y)| dev(px(&f, x, y), BG) > 30)
                .count()
        };
        let trim_ink = ink(z.trim);
        assert!(trim_ink > 80, "裁剪按钮里几乎没有笔画，实得 {trim_ink} px");
        // 开始/停止按钮此刻画的是绿三角 —— 两颗按钮的图形必须不是同一坨像素。
        let toggle_ink = ink(z.toggle);
        assert!(toggle_ink > 60, "开始按钮没画东西，实得 {toggle_ink} px");
        assert!(
            trim_ink != toggle_ink,
            "两颗按钮画了同一个图形（剪刀 {trim_ink} px = 三角 {toggle_ink} px）"
        );
        // 尺寸标签也得画出来（左侧非白像素）。
        assert!(ink(z.label) > 40, "尺寸标签没画出来");
    }

    /// 把控件条的**两种状态**叠成一张 PNG 供人眼评审（不进回归）。
    ///
    /// ```text
    /// cargo test --release --lib dump_bar_png -- --ignored --nocapture
    /// ```
    ///
    /// 产出 `temp/gif_ui/bar.png`（208 × 88：上 = 未录制绿三角，下 = 录制中红圆）。
    /// 与裁剪窗的 `dump_trim_window_png` 同一理由：真机截图被 UIPI 断了，而"这两颗键到底
    /// 长什么样"正是用户这次提的核心诉求 —— 得让他不装环境就能看到。
    #[test]
    #[ignore = "只产出 PNG 供人看，不进回归"]
    fn dump_bar_png() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("temp")
            .join("gif_ui");
        std::fs::create_dir_all(&dir).expect("建目录");

        let (w, h) = (BAR_MIN_W as u32, BAR_H as u32);
        let row = (w * h * 4) as usize;
        let mut out = vec![0u8; row * 2];
        out[..row].copy_from_slice(&render(&canvas(false)));
        out[row..].copy_from_slice(&render(&canvas(true)));

        let path = dir.join("bar.png");
        let file = std::fs::File::create(&path).expect("建 PNG");
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h * 2);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().expect("PNG 头");
        wr.write_image_data(&out).expect("PNG 数据");
        wr.finish().expect("PNG 收尾");
        println!("控件条 PNG: {}", path.display());
    }

    /// 三颗按钮必须是仅有的可点区域——窗口最右那条边距（close 右边）不可点。
    #[test]
    fn no_extra_clickable_area_between_close_and_window_edge() {
        let z = z();
        // close 右边到窗口右边缘之间只剩边距，那一片必须**不可点**。
        for x in (z.close.x + z.close.w) as i32..BAR_MIN_W {
            for y in 0..BAR_H {
                assert_eq!(
                    hit(&z, x as f32, y as f32),
                    None,
                    "({x},{y}) 居然可点 —— close 右边不该有额外按钮"
                );
            }
        }
    }

    // ─────────────────── 长截图模式（2026-10-01 两段式新增） ───────────────────

    /// 长截图布局：两颗键（开始/停止 + ✕）在窗内、竖直居中、不重叠；剪刀区**零尺寸**
    /// （命中恒 None，绘制被跳过）；条比录制模式窄。
    #[test]
    fn scroll_mode_layout_hides_scissors_and_keeps_two_buttons() {
        let h = BAR_H as f32;
        let z = zones_for(BAR_MIN_W as f32, h, BarMode::Scroll);
        // 剪刀零尺寸框：任何点都命中不到。
        assert_eq!(hit(&z, 1.0, 1.0), None);
        assert_eq!(hit(&z, 0.0, 0.0), None);
        for b in [z.toggle, z.close] {
            assert!(
                b.x >= 0.0 && b.x + b.w <= BAR_MIN_W as f32,
                "按钮出界：{b:?}"
            );
            assert!(
                (b.y - (h - b.h) / 2.0).abs() < 0.01,
                "按钮没有竖直居中：{b:?}"
            );
        }
        assert!(z.toggle.x + z.toggle.w <= z.close.x, "toggle 与 close 重叠");
        // 条宽：内容按两颗键算（不足 BAR_MIN_W 则保底），恒比录制模式窄。
        let s = sel(100, 100, 600, 400);
        let v = vs();
        let (x, _, w, _) = bar_rect_for(s, v, BarMode::Scroll);
        assert_eq!(
            w,
            content_w_for(s, BarMode::Scroll).max(BAR_MIN_W),
            "条宽应按两颗键的内容宽起算"
        );
        assert!(w < content_w_for(s, BarMode::Rec), "长截图条应比录制条窄");
        assert_eq!(x + w, 700, "右对齐仍贴选框右缘");
    }

    /// **像素门禁**：长截图就绪态必须看到**绿色三角形**（与录制"未上膛"同一语义），
    /// 且不得混入红色或剪刀墨迹。
    #[test]
    fn offscreen_scroll_bar_shows_green_triangle_when_ready() {
        let c = scroll_canvas();
        let f = render(&c);
        let z = zones_for(BAR_MIN_W as f32, BAR_H as f32, BarMode::Scroll);
        let tri = count_near(&f, z.toggle, START_GREEN, 60);
        assert!(tri > 60, "就绪态应看到绿色三角形，绿像素只有 {tri} 个");
        assert_eq!(
            count_near(&f, z.toggle, STOP_RED, 60),
            0,
            "就绪态混进了红色（还没开始采集）"
        );
        // 两颗键之外没有多余墨迹（剪刀被跳过后，toggle 与 close 之间的缝应是纯白）。
        let gap_x = ((z.toggle.x + z.toggle.w + z.close.x) / 2.0) as i32;
        assert_eq!(px(&f, gap_x, BAR_H / 2), BG, "两键之间的缝应是白面板");
    }

    /// 尺寸标签必须**动态**跟随选框（长截图就绪态可拖拽调整；旧实现开窗时算一次，
    /// 拖动后停在旧值——2026-10-01 用户要求实时显示）。
    #[test]
    fn size_label_follows_the_live_selection() {
        let mut rt = Runtime::new(Config::default());
        rt.scroll_ready = Some(crate::runtime::scroll::ScrollReady {
            rect: crate::core::geometry::ScreenRect::new(0, 0, 320, 240),
            horizontal: false,
        });
        let c = BarCanvas::new(
            Rc::new(RefCell::new(rt)),
            "320 × 240".to_string(),
            BarMode::Scroll,
        );
        assert_eq!(c.size_text_now(), "320 × 240");
        // 拖拽改了选框 → 标签立即跟着变（读的就是当前状态，不经开窗参数）。
        c.rt.borrow_mut().scroll_ready = Some(crate::runtime::scroll::ScrollReady {
            rect: crate::core::geometry::ScreenRect::new(0, 0, 200, 100),
            horizontal: false,
        });
        assert_eq!(c.size_text_now(), "200 × 100", "拖动后标签必须跟随新尺寸");
        // GIF 模式同一条纪律。
        let mut rt2 = Runtime::new(Config::default());
        rt2.gif_sel = Some(crate::core::geometry::ScreenRect::new(0, 0, 800, 600));
        let g = BarCanvas::new(Rc::new(RefCell::new(rt2)), "兜底".to_string(), BarMode::Rec);
        assert_eq!(g.size_text_now(), "800 × 600");
    }
}
