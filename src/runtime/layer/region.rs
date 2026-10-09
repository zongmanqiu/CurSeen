//! 截图**区域选择窗**：整张虚拟桌面的冻结帧 + 整屏压暗 + 选区尺寸。
//!
//! 这是"叠层"的第一个消费方。它与屏幕注释共用同一个冻结帧来源
//! （[`crate::core::backdrop::Backdrop::capture`] → `platform::screenshot::grab`），
//! 共用同一套几何换算（[`WorldMap`]）。
//!
//! # 单位纪律（三套坐标，别混）
//!
//! | 名字 | 类型 | 含义 |
//! |---|---|---|
//! | `Point` / `windui::Rect` | **i32** | windui 事件与 Canvas 的**逻辑**像素 |
//! | [`LRect`] | **f32** | 逻辑像素（浮点版：方向键微调 1 **物理**像素在 150% DPI 下是 0.667 逻辑像素，i32 存不下） |
//! | 冻结帧 | **i32 物理** | 世界坐标 − 虚拟桌面原点 |
//!
//! 三者之间**只允许**经 [`WorldMap`]，尤其是 `confirm` 那一步：旧实现把逻辑坐标直接
//! 当物理坐标传给 `confirm_snap`，非 100% 缩放下裁出来的区域是错的。
//!
//! # 外观（★ 2026-09-17 用户口径）
//!
//! - **一进来就整屏压暗**："就好像是已经开始截图了一样"——用户靠这个知道自己进了截图态。
//!   框选之后改为"选区保持原亮度、选区外压暗"。
//! - **不做任何文字提示**：底部那条"拖拽框选，松手完成 · Enter 确认 / Esc 取消"提示条
//!   整体删除（用户口径："其它文字提示都不要…这些都非常多余"）。唯一保留的文字是选区
//!   **右下角**的像素尺寸 `W × H`（"图片大小展示在截图区域的右下角"）。
//! - **选区边界是主色蓝**（见 [`ACCENT`]）。历史上那一圈红橙边框（连同它带着的准星 +
//!   坐标小矩形）已整体删除；放大镜在 2026-09-23 以**新的形态**回归，见下。
//!
//! # ★ 智能窗口检测 + 鼠标放大镜（2026-09-23 用户口径）
//!
//! 两条都是"把框选做得更准"的手段，一个管**整块**、一个管**边界**：
//!
//! - **智能窗口检测**：悬停时算出光标下那个程序窗口的外框，画成"推荐框"，**单击**即选中
//!   它（用户点名的"单击自动选择窗口边界"）。外框取 DWM 的可见外框且**贴边不内缩** ——
//!   这条是用户拿参照实现（flashot）点名要求的，那个实现又内缩了一圈边框厚度（理由见
//!   [`crate::platform::window_probe`] 模块头）。拖动仍是"我自己框"，两者互不干扰：
//!   一动手拖，推荐框立刻撤掉。
//! - **鼠标放大镜**：光标旁一块 240×160、4 倍的小取景器，准星指出"光标这一个像素"。
//!   实现复用画中画那套独立分层窗（见
//!   [`crate::runtime::layer::region_magnifier`] 的模块头：为什么不去画布上画）。
//!
//! # 交互
//!
//! | 手势 | 语义 |
//! |---|---|
//! | 左键拖拽 | 框选；松手即确认 |
//! | 左键单击（无位移） | 有推荐框 → 选中光标下那个窗口；否则取消 |
//! | 右键单击 | 取消 |
//! | Enter | 确认当前选区；无选区时 = 整屏 |
//! | Esc | 取消 |
//! | 方向键 | 微调选区（1 物理像素；Shift = 10） |
//!
//! （键盘那几条**照常生效**，只是不再有任何提示文字。`Enter` 的"无选区 = 整屏"刻意保持
//! 不变：它是写在本文档里的既有契约，随手改成"光标下的窗口"会让老用户莫名其妙。）
//!
//! 窗口是**无边框 + 不缩放**的：配合补丁 022-a，外圈那 8 逻辑像素不再被当成缩放热区，
//! 最边上也能起手框选。

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use windui::core::{EventCtx, Widget};
use windui::event::{Event, Key, MouseButton, PointerKind};
use windui::geometry::{Color, Point, Rect};
use windui::prelude::*;
use windui::render::image::{Fit, Image as WImage};
use windui::render::Canvas;
use windui::render::Paint as WPaint;
use windui::spec::Align;
use windui::text::{TextEngine, TextStyle};

use crate::core::backdrop::{DIM_ALPHA, DIM_COLOR};
use crate::core::exit_policy::ExitPolicy;
use crate::core::geometry::Rect as LRect;
use crate::core::image::RgbaBitmap;
use crate::runtime::layer::WorldMap;
use crate::runtime::Runtime;

/// 区域选择窗标题（`activate_window_by_title` / 日志按它找窗）。
pub const REGION_TITLE: &str = "CurSeen 截图区域";

/// 选区边界与压暗遮罩。
///
/// 选区边界用**主色蓝**（与产品 accent 同源）；旧值是红橙 `(224,92,58)`——用户
/// 2026-09-17 说截图时"有一个带红色边框的小矩形…要删除"，红色在本产品里只出现在这里。
const ACCENT: (u8, u8, u8) = (76, 139, 245);

/// 推荐框的填充透明度：低到不盖住要看的画面，又足够让"这块被建议了"一眼可见。
const HOVER_FILL_ALPHA: u8 = 48;
/// 推荐框的边框宽度（逻辑像素）。
const HOVER_BORDER_W: i32 = 2;

/// 「单击」与「拖拽」的分界（逻辑像素）：选区小于这个尺寸仍按单击算。
///
/// ★ **单一来源**：[`SnapRegionCanvas::has_selection`]（`Up` 时判断"这是单击还是
///   拖拽"）与拖拽分支里"要不要撤掉推荐框"共用它。两处各写一个字面量 3.0，
///   就会出现"单击判定"与"撤框判定"阈值不一致 —— 那种错只在边界上复现，
///   现场几乎查不出来。
const CLICK_SLOP: f32 = 3.0;

/// 一个选区是否已经构成"真拖拽"（而不是按下时那点抖动）。
fn is_drag(r: LRect) -> bool {
    r.w >= CLICK_SLOP && r.h >= CLICK_SLOP
}

/// 智能窗口检测的**位移节流**（逻辑像素）：光标挪不够这么远就复用上一次的结果。
const PROBE_MIN_PX: f32 = 3.0;
/// 智能窗口检测的**时间节流**（毫秒）。
///
/// 一次探测要枚举全部顶层窗（几十次系统调用），而鼠标移动事件可以到几百 Hz —— 不节流
/// 的话"悬停"这一个动作就能把 CPU 吃满。推荐框晚 60ms 亮起来，人是察觉不到的。
const PROBE_MIN_MS: u64 = 60;

fn color(c: (u8, u8, u8), a: u8) -> Color {
    Color::rgba(c.0, c.1, c.2, a)
}

/// 浮点逻辑矩形 → windui 的 i32 矩形（只在绘制/命中时才需要）。
fn irect(r: LRect) -> Rect {
    Rect {
        x: r.x.round() as i32,
        y: r.y.round() as i32,
        w: r.w.round() as i32,
        h: r.h.round() as i32,
    }
}

/// 归一化拖拽起点/终点为矩形（允许反向拖拽），并钳到窗口内。
///
/// `square = true` 时把结果约束成**正方形**（用户口径 2026-09-23：选区时按住 Shift 要
/// 正方形选区）：边长取两轴位移里**较长**的那一边，方向分别跟随光标相对锚点的正负号 ——
/// 于是"往左上拖"得到的是锚点左上方的正方形，与手的感觉一致，而不是硬把光标吸到某个方向。
///
/// ★ 次序：**先算该方向还剩多少地方，再让边长不超过它**。反过来（先夹矩形、再改成正方形）
/// 会让正方形被各边独立 clamp 削掉一角，出来的是矩形 —— 而"是正方形"正是这条的全部意义。
fn norm_rect(a: Point, b: Point, w: i32, h: i32, square: bool) -> LRect {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    // 方向：0 归正 —— 拖着拖着某一轴恰好不动时，仍要得到一个正方形而不是一条线。
    let sx = if dx < 0 { -1 } else { 1 };
    let sy = if dy < 0 { -1 } else { 1 };

    // 锚点朝这两个方向各还剩多少空间。
    let room_x = (if sx > 0 { w - a.x } else { a.x }).max(0);
    let room_y = (if sy > 0 { h - a.y } else { a.y }).max(0);

    let (mut sw, mut sh) = (dx.unsigned_abs() as i32, dy.unsigned_abs() as i32);
    if square {
        let side = sw.max(sh).min(room_x).min(room_y);
        sw = side;
        sh = side;
    } else {
        sw = sw.min(room_x);
        sh = sh.min(room_y);
    }

    LRect {
        x: (if sx > 0 { a.x } else { a.x - sw }) as f32,
        y: (if sy > 0 { a.y } else { a.y - sh }) as f32,
        w: sw as f32,
        h: sh as f32,
    }
}

/// 截图区域选择画布。
struct SnapRegionCanvas {
    rt: Rc<RefCell<Runtime>>,
    map: WorldMap,
    /// 冻结帧的 windui 图像（开窗时构建一次；整屏转换不便宜，不能每帧做）。
    ///
    /// 铺底图只需要这张 `WImage`；**取样**（放大镜）要的是 [`Self::frame`]。
    img: WImage,
    /// 冻结帧的**原始位图**：只服务放大镜（`WImage` 拿不回像素）。
    ///
    /// 2026-09-17 曾因为"放大镜删除"而不持有它；2026-09-23 放大镜以新形态回归，又拿回来。
    /// `Rc` 是从 `snap_frame_bitmap()` 直接接过来的，不额外拷一份整屏像素。
    frame: Rc<RgbaBitmap>,
    /// 拖拽锚点（逻辑坐标）。`Some` = 正在拖。
    anchor: Option<Point>,
    /// 当前选区（逻辑坐标，已归一化）。
    sel: Option<LRect>,
    /// 智能窗口检测：光标下推荐窗口的外框（逻辑坐标）。一动手拖就清掉。
    hover: Option<LRect>,
    /// 上次探测时的光标位置（逻辑坐标）—— 位移节流判据。
    probe_pos: Option<(f32, f32)>,
    /// 上次探测时刻 —— 时间节流判据。
    probe_at: Option<Instant>,
}

impl SnapRegionCanvas {
    fn new(rt: Rc<RefCell<Runtime>>, map: WorldMap, img: WImage, frame: Rc<RgbaBitmap>) -> Self {
        Self {
            rt,
            map,
            img,
            frame,
            anchor: None,
            // ★ 2026-09-17：不再预填"上次选区"。整个「记住上次框选」的机制随
            //   「按上次选区截图」快捷键一起删除（用户口径：这个功能不要）——留一个
            //   隐式预填反而会让"每次打开都是干净状态"这件事变得不可预期。
            sel: None,
            hover: None,
            probe_pos: None,
            probe_at: None,
        }
    }

    /// 跟踪光标：刷新鼠标放大镜，并按节流重算"推荐框"。
    ///
    /// 返回**推荐框是否真的变了** —— 只有它变了才值得把整块画布标脏（整屏重绘不便宜，
    /// 而移动事件可以到几百 Hz）。放大镜不吃这个返回值：它走自己的独立窗，与画布无关。
    fn track(&mut self, pos: Point) -> bool {
        // 逻辑 → 物理：放大镜取样与窗口命中判定都要物理像素（虚拟桌面坐标）。
        let (wx, wy) = self.map.to_world(pos.x as f32, pos.y as f32);
        let screen = self.rt.borrow().screen;
        let frame = self.frame.clone();
        self.rt
            .borrow_mut()
            .region_magnifier
            .update(&frame, (wx, wy), screen);

        // ── 智能窗口检测（位移 + 时间双重节流）──
        let due = self
            .probe_at
            .is_none_or(|t| t.elapsed().as_millis() as u64 >= PROBE_MIN_MS);
        let moved = self
            .probe_pos
            .is_none_or(|(px, py)| (px - pos.x as f32).hypot(py - pos.y as f32) >= PROBE_MIN_PX);
        if !due && !moved {
            return false;
        }
        self.probe_pos = Some((pos.x as f32, pos.y as f32));
        self.probe_at = Some(Instant::now());
        let next = crate::platform::window_probe::detect_at(wx.round() as i32, wy.round() as i32)
            .map(|r| {
                self.map
                    .rect_to_window(LRect::new(r.x as f32, r.y as f32, r.w as f32, r.h as f32))
            });
        // 比"是否有"更细一档：窗口在屏幕上动（拖动被框的那个窗）时推荐框要跟着动。
        // 容差 0.5 逻辑像素——亚像素的抖动不值得重绘整屏。
        let changed = match (self.hover, next) {
            (None, None) => false,
            (Some(a), Some(b)) => {
                (a.x - b.x).abs() > 0.5
                    || (a.y - b.y).abs() > 0.5
                    || (a.w - b.w).abs() > 0.5
                    || (a.h - b.h).abs() > 0.5
            }
            _ => true,
        };
        self.hover = next;
        changed
    }

    /// 窗口的逻辑矩形（原点恒为 0,0）。
    fn window_rect(&self) -> LRect {
        self.map.logical_rect()
    }

    /// 选区是否够大（小于 [`CLICK_SLOP`] 逻辑像素当作"单击"）。
    fn has_selection(&self) -> bool {
        self.sel.is_some_and(is_drag)
    }

    /// 选区 → 冻结帧像素矩形（`confirm_snap` 要的坐标系）。
    fn sel_in_frame_px(&self) -> Option<LRect> {
        let r = self.sel?;
        if r.w < 1.0 || r.h < 1.0 {
            return None;
        }
        let d = self.map.dpi();
        Some(LRect {
            x: r.x * d,
            y: r.y * d,
            w: r.w * d,
            h: r.h * d,
        })
    }

    /// 平移选区（逻辑坐标增量），并保证整体留在窗口内。
    fn nudge(&mut self, dx: f32, dy: f32) {
        let win = self.window_rect();
        let Some(r) = self.sel else { return };
        let x = (r.x + dx).clamp(0.0, (win.w - r.w).max(0.0));
        let y = (r.y + dy).clamp(0.0, (win.h - r.h).max(0.0));
        self.sel = Some(LRect { x, y, ..r });
    }

    fn confirm(&mut self, ctx: &mut EventCtx) {
        match self.sel_in_frame_px() {
            Some(r) => {
                // ★ 唯一的坐标换算点：逻辑 → 物理只经 WorldMap。
                self.rt.borrow_mut().confirm_snap(r);
            }
            None => self.rt.borrow_mut().cancel_snap(),
        }
        ctx.request_close();
    }

    fn cancel(&mut self, ctx: &mut EventCtx) {
        self.rt.borrow_mut().cancel_snap();
        ctx.request_close();
    }
}

impl Widget for SnapRegionCanvas {
    fn measure(&self, avail: Size, _style: &Style, _text: &mut dyn TextEngine) -> Size {
        avail
    }

    // ★ 右键必须收（2026-10-08 用户实测"右键无法退出选区"）：windui 对非左键的按下
    //   有**节点级过滤**——`wants_right_click()` 为 false（默认）时，右键事件根本不进
    //   本控件的 `on_event`。下面的右键取消分支写好了也是死代码。这是截图 / GIF / MP4
    //   / 长截图四条框选共用的唯一取消手势（见模块头交互表），必须放行。
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
        canvas.clear_rect(
            bounds.x as f32,
            bounds.y as f32,
            bounds.w as f32,
            bounds.h as f32,
        );
        // 冻结帧铺满（窗口尺寸 = 虚拟桌面的逻辑尺寸，1:1）
        let (lw, lh) = self.map.logical_size();
        canvas.draw_image(
            &self.img,
            Rect::new(bounds.x, bounds.y, lw, lh),
            Fit::Fill,
            0.0,
            1.0,
        );

        // ── 压暗：整屏（无选区无推荐）/ 焦点区（选区或推荐框）之外 ──
        //
        // ★ 用户口径 2026-09-17："进入截图状态时，我希望整个屏幕变暗。就好像是已经开始
        //   截图了一样。这样用户才知道我们进入状态了。" 旧实现只在**已经有选区**时压暗，
        //   刚进来那一瞬间画面与平时一模一样 → 用户不知道自己在截图态。
        let sel = self.sel.filter(|r| r.w > 0.0 && r.h > 0.0);
        // ★ 智能推荐框的内部同样保持原亮度（2026-10-08 用户口径："智能选区里要正常，
        //   没有被我们的选区外颜色遮住，这样用户才能更容易识别出来说这里是被智能推荐
        //   选中的"）。旧实现整屏压暗、推荐框只叠半透明主色——框内内容照旧是暗的，
        //   与"已确认选区内部是亮的"观感割裂，用户认不出"这一块被推荐了"。
        //   有选区时不遮推荐框（推荐框本身也不画，见下），两者互斥。
        let hover = if sel.is_some() { None } else { self.hover };
        // ★ 色与 α 都取自 `core::backdrop`：截图**编辑画布**也要铺同一层压暗
        //   （用户口径"就像在确定选区的时候一样"），常量只有一份才不会两处跑偏。
        let dim = color(DIM_COLOR, DIM_ALPHA);
        match sel.or(hover) {
            None => canvas.fill_rect(
                bounds.x as f32,
                bounds.y as f32,
                bounds.w as f32,
                bounds.h as f32,
                &WPaint::fill(dim),
            ),
            Some(sel) => {
                let r = irect(sel);
                let (x0, y0) = (bounds.x + r.x, bounds.y + r.y);
                let (x1, y1) = (x0 + r.w, y0 + r.h);
                // 焦点区外暗化遮罩（四条边各一块，零重叠）。焦点区 = 已确认选区，
                // 或（还没有选区时的）智能推荐框——两者内部都保持原亮度。
                for rr in [
                    Rect {
                        x: bounds.x,
                        y: bounds.y,
                        w: bounds.w,
                        h: r.y,
                    },
                    Rect {
                        x: bounds.x,
                        y: y1,
                        w: bounds.w,
                        h: bounds.h - r.y - r.h,
                    },
                    Rect {
                        x: bounds.x,
                        y: y0,
                        w: r.x,
                        h: r.h,
                    },
                    Rect {
                        x: x1,
                        y: y0,
                        w: bounds.w - r.x - r.w,
                        h: r.h,
                    },
                ] {
                    if rr.w > 0 && rr.h > 0 {
                        canvas.fill_rect(
                            rr.x as f32,
                            rr.y as f32,
                            rr.w as f32,
                            rr.h as f32,
                            &WPaint::fill(dim),
                        );
                    }
                }
                // 选区边界：1px（四条边矩形填充——与主渲染同款防缺角画法）。
                //
                // ★ 颜色从原来的红橙（224,92,58）换成主色蓝：用户 2026-09-17 说截图时
                //   "有一个带红色边框的小矩形…要删除"。红色在本产品里只出现在这一处，
                //   换成主色后与整体配色一致；选区本身仍需要一条边界线（压暗对比之外再
                //   加一道明确的边，浅色底图上才看得出选到哪）。
                let border = color(ACCENT, 255);
                for rr in [
                    Rect {
                        x: x0,
                        y: y0,
                        w: r.w,
                        h: 1,
                    },
                    Rect {
                        x: x0,
                        y: y1 - 1,
                        w: r.w,
                        h: 1,
                    },
                    Rect {
                        x: x0,
                        y: y0,
                        w: 1,
                        h: r.h,
                    },
                    Rect {
                        x: x1 - 1,
                        y: y0,
                        w: 1,
                        h: r.h,
                    },
                ] {
                    canvas.fill_rect(
                        rr.x as f32,
                        rr.y as f32,
                        rr.w as f32,
                        rr.h as f32,
                        &WPaint::fill(border),
                    );
                }
                // ── 尺寸标签：选区**右下角**（用户口径："图片大小展示在截图区域的右下角"）
                //
                // 画在选区**内侧**贴边：选区贴着屏幕右下角时画到外面就出屏了；文字只覆盖
                // 屏幕显示，不进裁剪结果。整块提示条（"拖拽框选，松手完成 · Enter 确认…"）
                // 已按用户要求整体删除，尺寸只留这一处。
                let (fw, fh) = (
                    (sel.w * self.map.dpi()).round() as i32,
                    (sel.h * self.map.dpi()).round() as i32,
                );
                let label = format!("{fw} × {fh}");
                let ts = TextStyle::new(13.0);
                let m = canvas.measure_text(&label, &ts);
                let bw = m.w.max(1) + 12;
                let bh = m.h.max(16) + 6;
                // ★ 直角（无圆角）：与贴图百分比角标一致的硬朗风格。
                // ★ 贴合选区右下角边框线：徽标底边/右边 = 选区底边/右边（去掉原来的 -2 内缩）。
                //   选区贴着屏幕右下角时画到外面就出屏了，故仍用 `.max` 把徽标钳在选区内。
                let bx = (x1 - bw).max(x0);
                let by = (y1 - bh).max(y0);
                canvas.fill_rect(
                    bx as f32,
                    by as f32,
                    bw as f32,
                    bh as f32,
                    &WPaint::fill(color((18, 22, 26), 208)),
                );
                // 内缩 3px 的好处：`draw_text` 盒子高度取自然行高（垂直居中退化为顶对齐），
                // 上下各留 3px 就是真正的视觉居中（见 `core::text_metrics` 的说明）。
                canvas.draw_text(
                    &label,
                    Rect::new(bx, by + 3, m.w.max(1), m.h.max(16)),
                    color((240, 243, 246), 255),
                    Align::Center,
                    &ts,
                );
            }
        }

        // ── 智能窗口检测的**推荐框** ──
        //
        // ★ 必须画在**压暗之后**：框外的部分是暗的，先画就一起被压暗了，等于没画。
        //   框内已在上面那一拍保持了原亮度（见 `hover` 的注释），这里叠上主色填充与
        //   边框就是"明亮内容 + 一层淡淡的推荐色"，一眼认得出这一块被推荐。
        //
        // 观感与"已确认的选区"刻意区分开——那条是"选区外压暗 + 1px 边 + 右下角尺寸"，
        // 这条是"半透明主色填充 + 2px 主色边"。两者形状可能一模一样（单击选中的就是它），
        // 但一眼能看出哪个是"建议"、哪个是"已定"：有选区时这条根本不画（见下面的判据）。
        // ★ 判据取 `!has_selection()` 而不是 `sel.is_none()`：按下左键但还没拖够
        //   `CLICK_SLOP` 时 `sel` 已是 `Some`（尺寸接近 0），可它语义上**还是单击** ——
        //   按 `sel.is_none()` 的话推荐框会在按下的那一瞬间视觉消失、状态却还在，
        //   看着像"按一下就坏了"。
        if !self.has_selection() {
            if let Some(h) = self.hover {
                let r = irect(h);
                let (x0, y0) = (bounds.x + r.x, bounds.y + r.y);
                canvas.fill_rect(
                    x0 as f32,
                    y0 as f32,
                    r.w as f32,
                    r.h as f32,
                    &WPaint::fill(color(ACCENT, HOVER_FILL_ALPHA)),
                );
                // 先填后描：填充是半透明的，若反过来先描边再填充，边上那 2px 会被填充
                // 再混一层色，边框看着就"糊"了。
                let border = color(ACCENT, 255);
                for rr in [
                    Rect {
                        x: x0,
                        y: y0,
                        w: r.w,
                        h: HOVER_BORDER_W,
                    },
                    Rect {
                        x: x0,
                        y: y0 + r.h - HOVER_BORDER_W,
                        w: r.w,
                        h: HOVER_BORDER_W,
                    },
                    Rect {
                        x: x0,
                        y: y0,
                        w: HOVER_BORDER_W,
                        h: r.h,
                    },
                    Rect {
                        x: x0 + r.w - HOVER_BORDER_W,
                        y: y0,
                        w: HOVER_BORDER_W,
                        h: r.h,
                    },
                ] {
                    if rr.w > 0 && rr.h > 0 {
                        canvas.fill_rect(
                            rr.x as f32,
                            rr.y as f32,
                            rr.w as f32,
                            rr.h as f32,
                            &WPaint::fill(border),
                        );
                    }
                }
            }
        }
    }

    fn on_event(&mut self, ctx: &mut EventCtx, ev: &Event) -> bool {
        let win = self.window_rect();
        match ev {
            Event::Pointer(pe) => match pe.kind {
                PointerKind::Down if pe.button == MouseButton::Left => {
                    self.anchor = Some(pe.pos);
                    self.sel = None;
                    // 按下也算一次光标位置更新：不动鼠标直接点，放大镜也要已经在正确位置。
                    self.track(pe.pos);
                    ctx.capture();
                    ctx.mark_dirty_all();
                    true
                }
                PointerKind::Move => {
                    let hover_changed = self.track(pe.pos);
                    match self.anchor {
                        // 正在拖：照旧框选。★ 一动手拖就撤掉推荐框 —— 用户已经用拖动
                        // 表达了自己的意图，再叠一个"建议框"只会让人分不清哪个才算数。
                        Some(a) => {
                            // ★ Shift = 正方形（用户口径 2026-09-23："选区时按住 shift 键，
                            //   我希望能够创建正方形选区"）。修饰键**每帧从事件里重取**：
                            //   拖到一半按下或松开都立刻生效，不需要额外消费按键事件。
                            let r = norm_rect(a, pe.pos, win.w as i32, win.h as i32, pe.mods.shift);
                            self.sel = Some(r);
                            // ★ 只有**真拖拽**才撤掉推荐框。按下左键期间那点鼠标抖动
                            //   （<`CLICK_SLOP` 逻辑像素）仍然是"单击"，不能把推荐框一起
                            //   带走 —— 否则 `Up` 时 `hover` 已是 `None`，单击会静默退化
                            //   成「取消」，"单击选中光标下那个窗口"这条几乎不可能成功。
                            //   真机证据见本文件顶部与 `temp/patch_click_select.py`。
                            if is_drag(r) && self.hover.take().is_some() {
                                log::info!(
                                    "[diag] 真拖拽（{}×{} ≥ {}）撤掉推荐框",
                                    r.w.round(),
                                    r.h.round(),
                                    CLICK_SLOP
                                );
                            }
                            ctx.mark_dirty_all();
                            true
                        }
                        // 没按下左键就没有锚点（`anchor` 只在 Down 里置位、确认/取消里清空）。
                        // 只有推荐框**真的变了**才重绘：移动事件可以到几百 Hz，而整屏重绘
                        // 不便宜 —— 每次都标脏等于让"悬停"这个动作一直烧 CPU。
                        None => {
                            if hover_changed {
                                ctx.mark_dirty_all();
                            }
                            false
                        }
                    }
                }
                PointerKind::Up if pe.button == MouseButton::Left => {
                    // 拖拽有位移 → 确认；纯单击 → 看有没有推荐框：
                    // 有就**直接选中光标下那个窗口**（"单击自动选择窗口边界"），没有才取消
                    // （截图软件通行语义）。两条路各归各的，互不干扰。
                    // 手势归 `ExitPolicy::REGION` 管（`core::exit_policy`）。
                    if ExitPolicy::REGION.confirm && self.has_selection() {
                        self.confirm(ctx);
                    } else if ExitPolicy::REGION.confirm && self.hover.is_some() {
                        self.sel = self.hover;
                        self.confirm(ctx);
                    } else {
                        self.cancel(ctx);
                    }
                    true
                }
                PointerKind::Down if pe.button == MouseButton::Right => {
                    if ExitPolicy::REGION.right_click {
                        self.cancel(ctx);
                    }
                    true
                }
                _ => false,
            },
            Event::Key(ke) if ke.pressed => match ke.key {
                Key::Escape => {
                    if ExitPolicy::REGION.escape {
                        self.cancel(ctx);
                    }
                    true
                }
                Key::Enter => {
                    if !ExitPolicy::REGION.confirm {
                        return false;
                    }
                    // 无选区 = 全屏
                    if !self.has_selection() {
                        self.sel = Some(win);
                    }
                    self.confirm(ctx);
                    true
                }
                Key::Left | Key::Right | Key::Up | Key::Down => {
                    // 1 物理像素（Shift = 10）——乘/除 dpi 都在 WorldMap 里。
                    let step = if ke.shift { 10.0 } else { 1.0 };
                    let logical = self.map.len_to_window(step);
                    let (dx, dy) = match ke.key {
                        Key::Left => (-logical, 0.0),
                        Key::Right => (logical, 0.0),
                        Key::Up => (0.0, -logical),
                        _ => (0.0, logical),
                    };
                    self.nudge(dx, dy);
                    ctx.mark_dirty_all();
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }
}

/// 打开区域选择窗（冻结帧已在 Runtime 中）。
pub fn open(ctx: &mut EventCtx, rt: &Rc<RefCell<Runtime>>) {
    let Some(frame) = rt.borrow().snap_frame_bitmap() else {
        return;
    };
    let vs = rt.borrow().screen;
    let map = WorldMap::new(vs);
    let (lw, lh) = map.logical_size();
    let img = match WImage::from_rgba(frame.w, frame.h, &frame.rgba) {
        Ok(i) => i,
        Err(_) => return,
    };
    // ★ 先把上一轮的放大镜收掉：它是一块独立置顶窗，不随本窗的生死走。上一轮万一漏了
    //   隐藏（比如进程被强杀之前那次框选），这一次不能继承一块挂在屏上的孤儿面板。
    rt.borrow_mut().region_magnifier.hide();
    let mut widget = SnapRegionCanvas::new(rt.clone(), map, img, frame);
    // ★ 不动鼠标也要立刻看到推荐框与放大镜：拿一次当前光标位置先跑一轮。否则"打开截图
    //   → 什么都不动"时屏上一片空白，得先晃一下鼠标才知道有这两个东西。
    let (cx, cy) = crate::platform::display::cursor_pos();
    let (lx, ly) = map.to_window(cx as f32, cy as f32);
    widget.track(Point::new(lx as i32, ly as i32));
    let rt_close = rt.clone();
    ctx.open_window(
        Window::new(REGION_TITLE, lw, lh)
            .single("snap_region")
            .owned(true)
            // ★ 无边框 + 不可缩放：配合 windui 补丁 022-a，外圈 8 逻辑像素不再是缩放热区
            //   （否则最边上那圈点不出选区起手）。
            .frameless(true)
            .resizable(false)
            // ★ 位置用**物理像素**（`Window::position` 的坐标系），尺寸用**逻辑 dp**
            //   （`Window::new` 的坐标系）。旧实现没设位置 → CW_USEDEFAULT，多屏必错位。
            .position(vs.x, vs.y)
            .bg(color((16, 20, 24), 255))
            .on_close_request(move |_ctx| {
                // 关窗（X / Alt+F4）= 取消，与 Esc / 右键同一出口
                rt_close.borrow_mut().snap_cancel();
                true
            })
            .content(move || Element::leaf().widget(widget).fill()),
    );
    let _ = crate::platform::win_util::activate_window_by_title(REGION_TITLE);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::display::ScreenRect;
    use windui::render::{DibTarget, RenderTarget};
    use windui::text::DWriteEngine;

    /// 探针画布尺寸（比真机屏幕小，判据与尺寸无关）。
    const PW: u32 = 400;
    const PH: u32 = 300;

    /// 探针用的选区（逻辑像素；dpi=1 时也就是帧像素）。
    const SEL: LRect = LRect {
        x: 60.0,
        y: 50.0,
        w: 200.0,
        h: 150.0,
    };

    /// 把区域窗**离屏渲成一张不透明画面**（纯白冻结帧 + 预乘标注层）。
    ///
    /// 为什么要这么测：这三条判据（整屏压暗 / 尺寸在右下角 / 没有红框与提示条）都只能
    /// 用像素说话——几何上"画了哪些矩形"看不出来，而**多画**一块（比如那条提示条）
    /// 恰恰是用户会立刻看到的东西。
    fn render(sel: Option<LRect>) -> Vec<u8> {
        render_with(sel, None)
    }

    /// 同上，但可以指定"智能窗口检测的推荐框"。
    fn render_with(sel: Option<LRect>, hover: Option<LRect>) -> Vec<u8> {
        let rt = Rc::new(RefCell::new(Runtime::new(
            crate::core::config::Config::default(),
        )));
        // 冻结帧：整张纯白（压暗与选区对比一眼可辨）。
        let white = vec![255_u8; (PW * PH * 4) as usize];
        let img = WImage::from_rgba(PW, PH, &white).expect("白底冻结帧");
        let frame = Rc::new(RgbaBitmap::new(PW, PH, white).expect("同一张纯白位图"));
        let map = WorldMap::with_dpi(
            ScreenRect {
                x: 0,
                y: 0,
                w: PW as i32,
                h: PH as i32,
            },
            1.0,
        );
        let c = SnapRegionCanvas {
            rt,
            map,
            img,
            frame,
            anchor: None,
            sel,
            hover,
            probe_pos: None,
            probe_at: None,
        };

        let mut ann = vec![0_u8; (PW * PH * 4) as usize];
        let mut engine = DWriteEngine::new();
        {
            let mut target = DibTarget::new(&mut ann, PW, PH);
            let mut canvas = target.make_canvas(&mut engine, 1.0);
            Widget::paint(
                &c,
                Rect::new(0, 0, PW as i32, PH as i32),
                Rect::new(0, 0, PW as i32, PH as i32),
                false,
                true,
                &mut *canvas,
                &Style::default(),
            );
        }

        // 预乘 → 不透明白底（与 `UpdateLayeredWindow` 同义）。
        let mut frame = vec![255_u8; (PW * PH * 4) as usize];
        for (d, s) in frame.chunks_exact_mut(4).zip(ann.chunks_exact(4)) {
            let a = s[3] as f32 / 255.0;
            let inv = 1.0 - a;
            d[0] = (s[0] as f32 + d[0] as f32 * inv).round() as u8;
            d[1] = (s[1] as f32 + d[1] as f32 * inv).round() as u8;
            d[2] = (s[2] as f32 + d[2] as f32 * inv).round() as u8;
            d[3] = 255;
        }
        frame
    }

    fn px(buf: &[u8], x: u32, y: u32) -> (u8, u8, u8) {
        let i = ((y * PW + x) * 4) as usize;
        (buf[i], buf[i + 1], buf[i + 2])
    }

    fn lum(p: (u8, u8, u8)) -> u32 {
        (p.0 as u32 * 299 + p.1 as u32 * 587 + p.2 as u32 * 114) / 1000
    }

    /// 屏幕矩形里的最暗像素亮度。
    fn darkest_in(buf: &[u8], x0: i32, y0: i32, x1: i32, y1: i32) -> u32 {
        let mut m = 255;
        for y in y0.max(0)..y1.min(PH as i32) {
            for x in x0.max(0)..x1.min(PW as i32) {
                m = m.min(lum(px(buf, x as u32, y as u32)));
            }
        }
        m
    }

    /// ★ 用户口径 3：**一进来就整屏压暗**（旧实现只在有选区时压暗，刚进截图态与平时一样）。
    #[test]
    fn entering_the_region_state_dims_the_whole_screen() {
        let f = render(None);
        // 四个角 + 中心：全都该被压暗（纯白 255 → 压暗后约 155）。
        for (x, y) in [
            (2, 2),
            (PW - 3, 2),
            (2, PH - 3),
            (PW - 3, PH - 3),
            (PW / 2, PH / 2),
        ] {
            let l = lum(px(&f, x, y));
            assert!(
                (120..=190).contains(&l),
                "未框选时整屏应压暗：({x},{y}) 亮度 {l}（纯白=255，说明没压暗）"
            );
        }
    }

    /// ★ 用户口径 3 的另一半：框选之后**选区保持原亮度、选区外压暗**。
    #[test]
    fn selection_stays_bright_while_outside_is_dimmed() {
        let f = render(Some(SEL));
        let inside = lum(px(&f, 160, 120));
        let outside = lum(px(&f, 20, 20));
        assert_eq!(inside, 255, "选区内部必须保持原亮度（截图内容不能被压暗）");
        assert!(
            (120..=190).contains(&outside),
            "选区之外应压暗，实际亮度 {outside}"
        );
    }

    /// ★ 用户口径 6：截图界面上**不得再有红橙边框**（旧选区边界与放大镜都是 224,92,58）。
    #[test]
    fn no_red_orange_border_anywhere() {
        for sel in [None, Some(SEL)] {
            let f = render(sel);
            let (mut red, mut sample) = (0_u32, (0_u8, 0_u8, 0_u8));
            for y in 0..PH {
                for x in 0..PW {
                    let p = px(&f, x, y);
                    // 旧红橙 (224,92,58)：红通道高、绿蓝明显低。
                    if p.0 >= 180 && p.1 <= 140 && p.2 <= 120 {
                        red += 1;
                        sample = p;
                    }
                }
            }
            assert_eq!(
                red, 0,
                "不该再出现红橙色边框像素（sel={sel:?}），实测 {red} 个，例如 {sample:?}"
            );
        }
    }

    /// ★ 用户口径 5：除选区右下角的尺寸外**没有任何文字提示**。
    ///
    /// 判据取"选区之外没有任何深色块"——旧那条底部提示条（深色圆角底 + 白字，居中）正是
    /// 这东西；它一在，这条立刻红。
    #[test]
    fn no_hint_bar_outside_the_selection() {
        let f = render(Some(SEL));
        let (x0, y0) = (SEL.x as i32, SEL.y as i32);
        let (x1, y1) = (x0 + SEL.w as i32, y0 + SEL.h as i32);
        // 上/下/左/右四条暗化带：最暗像素也只能是压暗亮度（~155），远亮于深色提示条（~60）。
        for (name, rect) in [
            ("上", (0, 0, PW as i32, y0)),
            ("下", (0, y1, PW as i32, PH as i32)),
            ("左", (0, y0, x0, y1)),
            ("右", (x1, y0, PW as i32, y1)),
        ] {
            let d = darkest_in(&f, rect.0, rect.1, rect.2, rect.3);
            assert!(
                d >= 120,
                "选区{name}侧不该有深色提示条/文字块，实测最暗亮度 {d}"
            );
        }
    }

    /// ★ 2026-09-23 修复：按下左键期间的**鼠标抖动**不能被当成拖拽。
    ///
    /// 判据是纯数据的（不构造 `EventCtx`）：`up` 分支"这是单击还是拖拽"用的
    /// [`is_drag`]，与拖拽分支"要不要撤推荐框"用的是同一个函数。只要它把
    /// 抖动判成拖拽，单击选中窗口就会在真机上退化成"取消"。
    #[test]
    fn jitter_below_click_slop_is_not_a_drag() {
        // 0×0（完全没动）与 2×2（抖动）都还是单击
        for (w, h) in [(0.0, 0.0), (1.0, 1.0), (2.9, 2.9), (2.9, 100.0)] {
            assert!(
                !is_drag(LRect {
                    x: 0.0,
                    y: 0.0,
                    w,
                    h
                }),
                "{w}×{h} 不该算拖拽（阈值 {CLICK_SLOP}）"
            );
        }
        // 够到阈值就算真拖拽
        for (w, h) in [(3.0, 3.0), (40.0, 20.0)] {
            assert!(
                is_drag(LRect {
                    x: 0.0,
                    y: 0.0,
                    w,
                    h
                }),
                "{w}×{h} 应算拖拽"
            );
        }
    }

    /// `has_selection` 与 `is_drag` 必须**同源**：两者阈值一旦分叉，
    /// 就会出现"拖到 2.99 时既不算选区、又把推荐框撤了"这种两头落空的状态。
    #[test]
    fn has_selection_agrees_with_is_drag() {
        let mk = |sel: Option<LRect>| SnapRegionCanvas {
            rt: Rc::new(RefCell::new(Runtime::new(
                crate::core::config::Config::default(),
            ))),
            map: WorldMap::with_dpi(
                ScreenRect {
                    x: 0,
                    y: 0,
                    w: 100,
                    h: 100,
                },
                1.0,
            ),
            img: WImage::from_rgba(100, 100, &vec![255u8; 100 * 100 * 4]).expect("白底"),
            frame: Rc::new(
                RgbaBitmap::new(100, 100, vec![255u8; 100 * 100 * 4]).expect("白底位图"),
            ),
            anchor: None,
            sel,
            hover: None,
            probe_pos: None,
            probe_at: None,
        };
        for (w, h) in [(0.0, 0.0), (2.9, 2.9), (3.0, 3.0), (50.0, 10.0)] {
            let r = LRect {
                x: 0.0,
                y: 0.0,
                w,
                h,
            };
            assert_eq!(
                mk(Some(r)).has_selection(),
                is_drag(r),
                "{w}×{h} 两处判据分叉了"
            );
        }
        assert!(!mk(None).has_selection(), "没有选区时不算有选区");
    }

    /// ★ 需求 2（2026-09-23）：悬停在某个窗口上时，要在**压暗之上**画一圈"推荐框"。
    ///
    /// 判据取**颜色偏向**而不是亮度：推荐框是主色蓝的半透明填充，压在压暗后的白底上必然
    /// 偏蓝（B 明显大于 R）；而压暗本身是中性的（三通道几乎相等）。用亮度判据的话，"压暗"
    /// 与"推荐框"根本分不开 —— 两者亮度接近。
    #[test]
    fn hovering_a_window_paints_a_blue_recommendation_box() {
        let f = render_with(None, Some(SEL));
        let inside = px(&f, 160, 120); // 推荐框内部（离 2px 边框足够远）
        let outside = px(&f, 20, 20); // 框外（仍是整屏压暗）
        assert!(
            inside.2 as i32 - inside.0 as i32 >= 20,
            "推荐框内应当明显偏蓝（主色蓝半透明填充），实测 {inside:?}"
        );
        assert!(
            (outside.2 as i32 - outside.0 as i32).abs() <= 6,
            "推荐框之外只该是中性的压暗，实测 {outside:?}"
        );
        assert!(
            inside.2 as i32 - outside.2 as i32 >= 10,
            "推荐框内的蓝分量要比框外高：内 {inside:?} 外 {outside:?}"
        );
        // ★ 框内必须保持**原亮度**（2026-10-08 用户口径："智能选区里要正常，没有被
        //   我们的选区外颜色遮住"）。白色冻结帧上：新实现框内 = 白 + 淡主色（亮），
        //   框外 = 白被压暗（暗）。旧实现整屏压暗时两者只差填充那一层淡色（Δ≈6），
        //   这条必红。
        assert!(
            lum(inside) > lum(outside) + 40,
            "推荐框内部要保持原亮度，不被压暗遮住：内 {inside:?} 外 {outside:?}"
        );
    }

    /// 推荐框只在**还没有选区**时出现：已经框好了再叠一个"建议框"，只会让人分不清哪个算数。
    #[test]
    fn a_confirmed_selection_suppresses_the_recommendation() {
        // 选区在左上、推荐框在右下，两者互不重叠 —— 这样"推荐框没画"不会被选区的画法掩盖。
        let sel = LRect {
            x: 20.0,
            y: 20.0,
            w: 80.0,
            h: 60.0,
        };
        let hover = LRect {
            x: 220.0,
            y: 180.0,
            w: 120.0,
            h: 90.0,
        };
        let f = render_with(Some(sel), Some(hover));
        let probe = px(&f, 280, 220); // 推荐框正中，且在选区之外
        assert!(
            (probe.2 as i32 - probe.0 as i32).abs() <= 6,
            "已有选区时不该再画推荐框（该点只应是中性压暗），实测 {probe:?}"
        );
    }

    /// ★ 用户口径 4：尺寸标签在选区**右下角**（左上角必须干净）。
    #[test]
    fn size_badge_sits_at_the_bottom_right_of_the_selection() {
        let f = render(Some(SEL));
        let (x0, y0) = (SEL.x as i32, SEL.y as i32);
        let (x1, y1) = (x0 + SEL.w as i32, y0 + SEL.h as i32);
        let corner = darkest_in(&f, x1 - 90, y1 - 34, x1, y1);
        // 采样框避开左边/上边那 1px 边界线（边界是蓝色亮线，不是标签）。
        let top_left = darkest_in(&f, x0 + 3, y0 + 3, x0 + 90, y0 + 34);
        assert!(
            corner <= 90,
            "选区右下角应有深色尺寸标签（含白字），实测最暗亮度 {corner}"
        );
        assert_eq!(
            top_left, 255,
            "选区左上角不该有任何标签，实测最暗 {top_left}"
        );
    }

    /// 不按 Shift 时必须与旧实现**逐值一致**（含反向拖拽与出界钳制）—— 这是纯回归保护：
    /// 正方形约束是新加的一条路，不能顺手把老路改了。
    #[test]
    fn free_drag_is_not_constrained() {
        let a = Point::new(500, 500);
        let fwd = norm_rect(a, Point::new(700, 580), 1920, 1080, false);
        assert_eq!((fwd.x, fwd.y, fwd.w, fwd.h), (500.0, 500.0, 200.0, 80.0));

        // 反向拖拽（往左上）同样成立。
        let rev = norm_rect(a, Point::new(300, 420), 1920, 1080, false);
        assert_eq!((rev.x, rev.y, rev.w, rev.h), (300.0, 420.0, 200.0, 80.0));

        // 光标出界（拖到窗口外）时钳到窗口大小，与旧的"两端各 clamp 一次"等价。
        let clamped = norm_rect(
            Point::new(1900, 1000),
            Point::new(2000, 1200),
            1920,
            1080,
            false,
        );
        assert_eq!(
            (clamped.x, clamped.y, clamped.w, clamped.h),
            (1900.0, 1000.0, 20.0, 80.0)
        );
    }

    /// ★ Shift：正方形，边长取两轴里**较长**的一边，方向跟随光标相对锚点的正负号。
    #[test]
    fn shift_keeps_the_selection_square_in_the_drag_direction() {
        let a = Point::new(500, 500);

        // 往右下拖 200×80 → 取 200，框在锚点右下方。
        let r = norm_rect(a, Point::new(700, 580), 1920, 1080, true);
        assert_eq!((r.x, r.y, r.w, r.h), (500.0, 500.0, 200.0, 200.0));

        // 往左上拖 80×300 → 取 300，框在锚点**左上方**（方向跟随光标，不硬吸到某个象限）。
        let r = norm_rect(a, Point::new(420, 200), 1920, 1080, true);
        assert_eq!((r.x, r.y, r.w, r.h), (200.0, 200.0, 300.0, 300.0));

        // 混合方向：右 + 上。
        let r = norm_rect(a, Point::new(700, 200), 1920, 1080, true);
        assert_eq!((r.x, r.y, r.w, r.h), (500.0, 200.0, 300.0, 300.0));

        // 某一轴恰好不动（横向 0 位移）时仍要给正方形，而不是一条线。
        let r = norm_rect(a, Point::new(500, 700), 1920, 1080, true);
        assert_eq!(r.w, r.h, "单轴位移也必须出正方形，实测 {}×{}", r.w, r.h);
    }

    /// ★ 被窗口边界挡住时**仍是正方形** —— 这条钉的是"先夹再变方"那个经典错法：
    /// 各边独立 clamp 会把正方形削成矩形。
    #[test]
    fn shift_square_stays_square_when_clamped_by_the_window_edge() {
        // 贴着右下角，横向只剩 20、纵向还剩 80：边长必须取 20（两方向的较小者）。
        let r = norm_rect(
            Point::new(1900, 1000),
            Point::new(1919, 1079),
            1920,
            1080,
            true,
        );
        assert_eq!((r.x, r.y, r.w, r.h), (1900.0, 1000.0, 20.0, 20.0));
        assert_eq!(r.w, r.h, "被钳过之后仍必须是正方形");

        // 贴着左上角反向拖。
        let r = norm_rect(Point::new(10, 10), Point::new(0, 0), 1920, 1080, true);
        assert_eq!((r.x, r.y, r.w, r.h), (0.0, 0.0, 10.0, 10.0));
        assert_eq!(r.w, r.h);
    }

    /// 同一个锚点同一个光标：按不按 Shift 得到的框**共用同一个角**（边长那一侧才变）。
    ///
    /// 这是"拖到一半按下/松开 Shift 立刻生效"的几何前提 —— 若两条路各自换了原点，
    /// 中途切换会让框突然跳位。
    #[test]
    fn shift_and_free_share_the_same_anchor_corner() {
        let a = Point::new(500, 500);
        let b = Point::new(700, 580);
        let free = norm_rect(a, b, 1920, 1080, false);
        let sq = norm_rect(a, b, 1920, 1080, true);
        assert_eq!((free.x, free.y), (sq.x, sq.y), "两条路的锚点角必须相同");
        assert!(sq.h > free.h, "正方形应在短轴上补到与长轴等长");
        assert_eq!(sq.w, free.w, "长轴那一侧不变");
    }
}
