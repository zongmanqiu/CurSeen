//! DrawList → windui Canvas 翻译器。
//!
//! 把 DrawList 绘制指令翻译为 windui Canvas 调用。

use std::cell::RefCell;
use std::rc::Rc;

use windui::geometry::Rect;
use windui::render::image::{Fit, Image as WImage};
use windui::render::{BlendMode as WBlend, Canvas, Gradient, Paint as WPaint};
use windui::spec::Align;
use windui::text::TextStyle;

use crate::core::color::Argb;
use crate::core::draw::{Blend, DrawCmd, DrawList, GradStop, Paint, Shape};
use crate::core::image::RgbaBitmap;

/// `DrawCmd::Image` 的位图资源缓存。
///
/// # 为什么必须有
///
/// `WImage::from_rgba` 不是零成本的：内部是 `rgba.to_vec()`（整屏拷贝）+ **逐像素
/// `premultiply()`**（1080p = 830 万次循环）+ 新建 `Pixmap`。而 `DrawCmd::Image` 每帧
/// 都会被翻译一次——叠层底图铺进 DrawList 之后（截图冻结帧 / 屏幕注释底图），这条路径
/// 会变成"每帧重建一张整屏位图"，1080p 实测 30–100 ms/帧，直接把笔迹拖垮。
///
/// 键是 `(数据指针, 长度, w, h)`；条目**同时持有 `Rc<RgbaBitmap>`**，防止指针被复用后
/// 误命中（位图一直被引用着，它的缓冲区就不会被释放、地址也就不会被别的分配复用）。
/// 命中即 `clone()`，而 `WImage` 内部是 `Rc<Pixmap>`，克隆只是引用计数 +1。
///
/// 条目淘汰：不再按固定容量（`cached_image` 每次进入时把"缓存外已无人持有"的死条目
/// 全部清除 —— 没人持有的位图再也不可能被画出来，留在缓存里就是泄漏，2026-09-29 实测
/// 每轮标注泄漏 16 MB）。同时用到的位图份数有限：拖尾悬浮标 + 叠层底图 + 贴图窗。
/// 取（必要时构建）`bitmap` 对应的 windui 图像。

#[derive(PartialEq, Eq, Clone, Copy)]
struct ImgKey {
    ptr: usize,
    len: usize,
    w: u32,
    h: u32,
}

/// 缓存项：键 +（位图，windui 图像）。
///
/// 位图那份 `Rc` 必须**和键同寿**：它一直被引用着，缓冲区就不会被释放，`ptr` 也就不会
/// 被别的分配复用——这正是"用地址当键"能成立的前提（见上面的误命中说明）。
type ImgEntry = (ImgKey, (Rc<RgbaBitmap>, WImage));

/// 缓存条目上限。够覆盖"一帧内重画的几层图"，又封死"贴图环每张都躺在缓存里"
/// 的无界积累（1080p 一条 ≈ 8.3MB）。见 `cached_image` 内的注释。
const IMG_CACHE_CAP: usize = 4;

/// 缓存**字节**上限（条数上限的补充，2026-10-01）。
///
/// 只按条数封顶有个洞：条数上限是为 1080p 量级的图定的（4 × 8.3MB ≈ 33MB），
/// 来一张 4K 截图（3840×2160×4 ≈ 33MB）单条就把预算打穿，四张 4K 就是 132MB。
/// 字节上限参照同类剪贴板工具的 LRU 上限取 32MiB——与"4 条 1080p"
/// 的既有预算一致，大图场景下自动降为 3 条甚至 1 条。**单条超限时允许独占缓存**
/// （总得画得出来），见 `cached_image` 的淘汰循环。
const IMG_CACHE_BYTES_CAP: usize = 32 * 1024 * 1024;

thread_local! {
    static IMG_CACHE: RefCell<Vec<ImgEntry>> =
        const { RefCell::new(Vec::new()) };
}

/// 取（必要时构建）`bitmap` 对应的 windui 图像。
fn cached_image(bitmap: &Rc<RgbaBitmap>) -> Option<WImage> {
    let key = ImgKey {
        ptr: bitmap.rgba.as_ptr() as usize,
        len: bitmap.rgba.len(),
        w: bitmap.w,
        h: bitmap.h,
    };
    IMG_CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        if let Some(pos) = cache.iter().position(|(k, _)| *k == key) {
            // ★ LRU 而非 FIFO（10 号报告 §11.4⑤ 指认，2026-10-02 修）：命中项必须
            //   挪到队尾再返回。否则生死由"插入先后"决定——贴图历史环（cap 10）只给
            //   4 张缓存，在用的那张若插入得早，会稳定地被 `remove(0)` 逐出、每帧
            //   白付一次 `WImage::from_rgba`（1080p ≈ 8.3MB 上传）。
            let entry = cache.remove(pos);
            let img = entry.1 .1.clone();
            cache.push(entry);
            return Some(img);
        }
        let img = WImage::from_rgba(bitmap.w, bitmap.h, &bitmap.rgba).ok()?;
        // ★ 只留"还有别人在用"的条目（强引用 > 1 = 缓存之外还有持有者）。没人持有的
        //   位图再也不可能被画出来（画它就必须持有 Rc），留在缓存里纯属泄漏 ——
        //   2026-09-29 实测：每轮标注的现抓底图 8.1MB + WImage 8.1MB = 每轮净涨 16MB，
        //   用户三次开关 20→70MB 就是它。
        cache.retain(|(_, (rc, _))| std::rc::Rc::strong_count(rc) > 1);
        // ★ 容量上限（2026-10-01）：贴图历史环（cap 10）会长期持有每张位图的 `Rc`，
        //   于是上面那条 retain 永远筛不掉它们 —— 环里每张 1080p 贴图在缓存里还躺着
        //   一份 8.3MB 的 `WImage`，十张就是 83MB，且没有任何事件边沿会去清它。
        //   缓存的价值只在"同一张位图连续几帧不重解码"，留最近几条足矣；满了就丢
        //   最久未用的（`remove(0)` 丢队首——命中项已被提升到队尾，见上面的 LRU 分支）。
        //   双判据：条数 **或** 字节（[`IMG_CACHE_BYTES_CAP`]）任一触顶就丢，
        //   防 4K 级大图按条数预算被单条打穿。单条本身就超字节上限时，允许它独占
        //   缓存（图必须画得出来），循环被 `!cache.is_empty()` 兜住不会死转。
        let new_bytes = key.len;
        while !cache.is_empty()
            && (cache.len() + 1 > IMG_CACHE_CAP
                || cache.iter().map(|(k, _)| k.len).sum::<usize>() + new_bytes
                    > IMG_CACHE_BYTES_CAP)
        {
            cache.remove(0);
        }
        cache.push((key, (bitmap.clone(), img.clone())));
        Some(img)
    })
}

/// 清空位图缓存（单测用；也便于将来在"叠层退出"时主动释放）。
pub fn clear_image_cache() {
    IMG_CACHE.with(|c| c.borrow_mut().clear());
}

/// 世界坐标（虚拟桌面物理像素）→ 窗口本地逻辑像素的映射。
///
/// 三个量总是一起传递（缩放 + 平移），打成一包既少一层参数，也避免"传对了 scale 忘了 dx"。
#[derive(Clone, Copy)]
struct View {
    dx: f32,
    dy: f32,
    scale: f32,
}

impl View {
    fn x(&self, wx: f32) -> f32 {
        wx * self.scale + self.dx
    }

    fn y(&self, wy: f32) -> f32 {
        wy * self.scale + self.dy
    }

    /// 长度/半径/线宽的换算（只有缩放，没有平移）。
    fn len(&self, w: f32) -> f32 {
        w * self.scale
    }
}

/// 一条绘制指令携带的透明度与混合属性。
///
/// 打包成一个结构体而不是继续加形参：`fill_shape` / `stroke_shape` 的入参已经贴着
/// clippy 的 7 个上限，再加就没法编译了；而这两项语义上也是"一起来的"——
/// 描述"这条指令怎么落到画布上"，跟 `View`（落到哪里）正好对应。
#[derive(Clone, Copy)]
struct CmdStyle {
    alpha: f32,
    blend: Blend,
}

/// Argb 转 windui Color。
fn to_color(c: Argb) -> windui::geometry::Color {
    windui::geometry::Color::rgba(c.r(), c.g(), c.b(), c.a())
}

/// 带额外 alpha 的 Argb 转 Color（`DrawList` 的淡出走这个乘子）。
fn to_color_alpha(c: Argb, alpha: Option<f32>) -> windui::geometry::Color {
    match alpha {
        Some(a) => windui::geometry::Color::rgba(
            c.r(),
            c.g(),
            c.b(),
            (c.a() as f32 * a.clamp(0.0, 1.0)) as u8,
        ),
        None => to_color(c),
    }
}

/// 图元在世界坐标下的绘制矩形 `(x, y, w, h)`。
///
/// 渐变必须按**图元自己的矩形**归一化：windui 的 `Gradient` 用的是相对绘制矩形的
/// 归一化坐标（0..1，见其 `Gradient` 文档），而 `DrawList` 的 `Paint::Linear/Radial`
/// 带的是绝对世界坐标。缺了这一步，渐变不但映射不到正确位置，还会因为下面是
/// 「非 Solid 一律回退白色」而整块画成白的。
fn shape_bounds(shape: &Shape, draw: &DrawList) -> (f32, f32, f32, f32) {
    match shape {
        Shape::Rect { rect, .. } => (rect.x, rect.y, rect.w, rect.h),
        Shape::Circle { center, radius } => (
            center.x - radius,
            center.y - radius,
            radius * 2.0,
            radius * 2.0,
        ),
        Shape::Ellipse {
            center,
            radius_x,
            radius_y,
        } => (
            center.x - radius_x,
            center.y - radius_y,
            radius_x * 2.0,
            radius_y * 2.0,
        ),
        Shape::Line { a, b } => (
            a.x.min(b.x),
            a.y.min(b.y),
            (a.x - b.x).abs(),
            (a.y - b.y).abs(),
        ),
        Shape::Polyline { points, .. } => {
            let pts = draw.range(*points);
            let Some(first) = pts.first() else {
                return (0.0, 0.0, 0.0, 0.0);
            };
            let (mut x0, mut y0) = (first.x, first.y);
            let (mut x1, mut y1) = (x0, y0);
            for p in pts {
                x0 = x0.min(p.x);
                y0 = y0.min(p.y);
                x1 = x1.max(p.x);
                y1 = y1.max(p.y);
            }
            (x0, y0, x1 - x0, y1 - y0)
        }
    }
}

/// 渐变颜色标 → windui 的颜色标（`alpha` 是整条指令的透明度乘子）。
fn gradient_stops(stops: &[GradStop], alpha: f32) -> Vec<(f32, windui::geometry::Color)> {
    stops
        .iter()
        .map(|s| (s.offset, to_color_alpha(s.color, Some(alpha))))
        .collect()
}

/// `Blend` → windui `BlendMode`。
///
/// ★ 这个映射必须存在：`pixie_dust` 的三层辉光全靠 `Blend::Plus` 叠出高光。
/// 忽略它等于把三层都按 `SourceOver` 平铺，粒子就从"闪光"退化成"实心色块"。
fn to_blend(b: Blend) -> WBlend {
    match b {
        Blend::SourceOver => WBlend::SourceOver,
        Blend::Plus => WBlend::Plus,
        Blend::Clear => WBlend::Clear,
    }
}

/// `Paint` 转 windui `Paint`，渐变按 `bounds`（世界坐标）归一化映射。
///
/// ★ 这里曾经把非 `Solid` 的 paint 一律回退成**不透明白色**，于是「辉光圆影」
/// （`glow.rs` 用 `Paint::Radial` 做中心亮、边缘透明的辉光）在屏上画成一整块白圆，
/// 落在浅色/白色背景上完全看不见。windui 的 `render::Paint` 本就带
/// `gradient: Option<Gradient>`，只是这个翻译层没接上。
///
/// `alpha` 是 [`DrawCmd`] 携带的整条指令透明度，**乘**在颜色自身的 alpha 上。
/// 约定：便捷构造器（如 [`DrawList::fill_circle`]）一律传 `1.0`，alpha 由颜色自带；
/// 手工拼指令的特效才用 `alpha` 表达"这一层是核心的百分之几"
/// （如 `pixie_dust` 外层辉光传 `0.12`）。两处都写就会平方，务必只写一处。
fn to_wpaint(p: &Paint, bounds: (f32, f32, f32, f32), v: View, alpha: f32) -> WPaint {
    let (bx, by, bw, bh) = bounds;
    let sx = v.len(bw).max(1e-6);
    let sy = v.len(bh).max(1e-6);
    let nx = |x: f32| (v.x(x) - v.x(bx)) / sx;
    let ny = |y: f32| (v.y(y) - v.y(by)) / sy;

    match p {
        Paint::Solid(c, _) => WPaint::fill(to_color_alpha(*c, Some(alpha))),
        Paint::Linear { from, to, stops } => WPaint::gradient(Gradient::linear(
            (nx(from.x), ny(from.y)),
            (nx(to.x), ny(to.y)),
            gradient_stops(stops, alpha),
        )),
        Paint::Radial {
            center,
            radius,
            stops,
        } => WPaint::gradient(Gradient::radial(
            (nx(center.x), ny(center.y)),
            // 归一化半径 = 绝对半径 / **短边全长**，要与 windui 的
            // `sk_shader` 对齐：那里算的是 `radius * w.min(h)`，即 1.0 = 整个短边。
            //
            // ⚠️ windui 自己的 `Gradient::radial` 文档括注写的是「1.0≈半个短边」，
            // 与它自己的实现差一倍，别照那句写——照它写会让渐变只铺到图元半径的一半，
            // 圆形辉光的边缘停在半程色上，看起来就是一圈硬边。
            // 故这里用图元包围盒短边（圆 = 2r）作分母：圆半径 r → 0.5 → 着色半径正好 r。
            v.len(*radius) / sx.min(sy),
            gradient_stops(stops, alpha),
        )),
    }
}

/// 描边用的 paint：windui 的渐变只对 fill 类图元生效，stroke 退化为首个色标。
fn to_wpaint_solid(p: &Paint, alpha: f32) -> WPaint {
    match p {
        Paint::Solid(c, _) => WPaint::fill(to_color_alpha(*c, Some(alpha))),
        Paint::Linear { stops, .. } | Paint::Radial { stops, .. } => WPaint::fill(to_color_alpha(
            stops.first().map(|s| s.color).unwrap_or(Argb(0)),
            Some(alpha),
        )),
    }
}

/// 填充形状。
fn fill_shape(
    shape: &Shape,
    draw: &DrawList,
    canvas: &mut dyn Canvas,
    paint: &Paint,
    style: CmdStyle,
    v: View,
) {
    let wp = to_wpaint(paint, shape_bounds(shape, draw), v, style.alpha)
        .with_blend(to_blend(style.blend));
    match shape {
        Shape::Rect { rect, radius } => {
            let (x, y) = (v.x(rect.x), v.y(rect.y));
            let (w, h) = (v.len(rect.w), v.len(rect.h));
            if *radius > 0.0 {
                canvas.fill_round_rect(x, y, w, h, v.len(*radius), &wp);
            } else {
                canvas.fill_rect(x, y, w, h, &wp);
            }
        }
        Shape::Circle { center, radius } => {
            canvas.fill_circle(v.x(center.x), v.y(center.y), v.len(*radius), &wp);
        }
        Shape::Ellipse {
            center,
            radius_x,
            radius_y,
        } => {
            // `Canvas::fill_ellipse` 收的是**圆心 + 两个半径**。这里曾写成
            // `fill_ellipse(cx - rx, cy - ry, rx * 2.0, ry * 2.0)`——把圆心当成左上角、
            // 半径乘二，画出来是一个偏到图元左上、尺寸翻倍的椭圆。当前没有 Fill 类指令
            // 用 `Shape::Ellipse`（`DrawList` 只提供 `stroke_ellipse`），所以一直是潜伏的；
            // 但只要有第二个消费方，它就会立刻显形。
            canvas.fill_ellipse(
                v.x(center.x),
                v.y(center.y),
                v.len(*radius_x),
                v.len(*radius_y),
                &wp,
            );
        }
        Shape::Line { a, b } => {
            canvas.draw_line(v.x(a.x), v.y(a.y), v.x(b.x), v.y(b.y), 1.0, &wp);
        }
        Shape::Polyline { points, closed } => {
            let pts = draw.range(*points);
            if pts.is_empty() {
                return;
            }
            let windui_pts: Vec<(f32, f32)> = pts.iter().map(|p| (v.x(p.x), v.y(p.y))).collect();
            if *closed {
                canvas.fill_polygon(&windui_pts, &wp);
            } else {
                canvas.draw_polyline(&windui_pts, 1.0, &wp);
            }
        }
    }
}

/// 描边圆环带填充时内径的最小有效值：低于它就视为实心圆。
const CIRCLE_ANNULUS_MIN_INNER: f32 = 0.5;

/// 环带多边形的自适应细分数：弦矢（圆弧与弦的最大距离）≤ `tol` 像素。
///
/// 弦矢公式：`sag = r * (1 - cos(π/n))`，反解 `n = π / acos(1 - tol/r)`。
/// 下限 24（小半径也不至于太粗糙），上限 512（成本封顶）。
fn circle_segments(r: f32) -> u32 {
    let tol = 0.2_f32.min(r.max(1e-3));
    let cos_half = (1.0 - tol / r).clamp(-1.0, 1.0);
    let n = (std::f32::consts::PI / cos_half.acos().max(1e-6)).ceil() as u32;
    n.clamp(24, 512)
}

/// 构造圆环（环带）的闭合多边形：外弧 `0→2π` 正向 + 内弧 `2π→0` 反向。
///
/// 首点与末点的角度都是 0，`fill_polygon` 自动闭合时接缝正好是一条径向边——
/// 数学上严格闭合，无端帽、无接头、无缺角。
fn circle_annulus_polygon(cx: f32, cy: f32, r_outer: f32, r_inner: f32) -> Vec<(f32, f32)> {
    ellipse_annulus_polygon(cx, cy, r_outer, r_outer, r_inner, r_inner)
}

/// 椭圆环带多边形。圆是它的特例（[`circle_annulus_polygon`]）。
///
/// 椭圆描边曾经直接调 `fill_ellipse`——那是**填实**，不是描边，见 `stroke_shape` 的说明。
/// 与圆一样展开成真正的环带区域，才能既空心又跟随线宽。
///
/// 细分数取四个半径里最大的那个（`circle_segments` 的弦矢容差是按半径单调的，取最大即
/// 最保守）。对 `rx=100 / ry=60` 这种扁椭圆，最"钝"的位置在短轴端（局部曲率半径
/// `rx²/ry ≈ 167`），取 100 算出的段数下弦矢约 0.33px，仍在亚像素以内。
fn ellipse_annulus_polygon(
    cx: f32,
    cy: f32,
    rx_outer: f32,
    ry_outer: f32,
    rx_inner: f32,
    ry_inner: f32,
) -> Vec<(f32, f32)> {
    let r_max = rx_outer.max(ry_outer).max(rx_inner).max(ry_inner);
    let seg = circle_segments(r_max.max(1e-3));
    let mut pts = Vec::with_capacity(2 * (seg as usize) + 2);
    for i in 0..=seg {
        let a = std::f32::consts::TAU * (i as f32) / (seg as f32);
        pts.push((cx + rx_outer * a.cos(), cy + ry_outer * a.sin()));
    }
    for i in (0..=seg).rev() {
        let a = std::f32::consts::TAU * (i as f32) / (seg as f32);
        pts.push((cx + rx_inner * a.cos(), cy + ry_inner * a.sin()));
    }
    pts
}

/// 描边形状。
fn stroke_shape(
    shape: &Shape,
    draw: &DrawList,
    canvas: &mut dyn Canvas,
    paint: &Paint,
    style: CmdStyle,
    width: f32,
    v: View,
) {
    let wp = to_wpaint_solid(paint, style.alpha).with_blend(to_blend(style.blend));
    let w = v.len(width);

    match shape {
        Shape::Rect { rect, radius, .. } => {
            // ★ 描边矩形必须是**空心**的（回归修复）。
            //
            // 这里原是 `fill_round_rect(.., radius.max(width * 0.5), ..)`——把整个矩形填实，
            // 只是顺带把圆角半径撑到半个笔宽。后果是「矩形」工具拖出来一个实心色块、
            // 工具栏的方框图标变成一个实心方块，而面板里的线型/颜色还会照常生效，所以
            // 看起来像"设计如此"而不是 bug。`main/` 线（`ui/paint.rs::stroke_shape`）本来
            // 就是真描边，是 2.0 重写时退化掉的。
            //
            // 四角用一条闭环折线而不是四次 `draw_line`：`draw_line` 是平头端面，拐角外侧
            // 那块楔形谁也没盖到，会留一个约等于笔宽的豁口（`Canvas::draw_polyline` 的
            // 文档里写了同一件事）。
            let x = v.x(rect.x);
            let y = v.y(rect.y);
            let (w, h) = (v.len(rect.w), v.len(rect.h));
            let r = v.len(*radius);
            if r > 0.0 {
                canvas.stroke_round_rect(x, y, w, h, r, v.len(width), &wp);
            } else {
                // ★ 四角完整封闭（用户报告：粗笔下方框有一个角是缺的）。
                //
                // 旧实现用闭环折线描边，拐角外侧靠 Join 补楔形：粗笔时 Join 补不到的
                // 缝隙会露底，且半透明笔在拐角两段叠折还会加深出脏斑。改为**四条边
                // 各填一个矩形**，墨迹中心线仍压在矩形边界上（与描边语义一致）：
                // 上下两条通长（含四角），左右两条只接在上下之间（零重叠）→
                // 任何笔宽都严格封闭，半透明色也均匀。
                let hw = v.len(width) * 0.5;
                let (x2, y2) = (x + w, y + h);
                canvas.fill_rect(x - hw, y - hw, w + 2.0 * hw, 2.0 * hw, &wp); // 上边（含两角）
                canvas.fill_rect(x - hw, y2 - hw, w + 2.0 * hw, 2.0 * hw, &wp); // 下边（含两角）
                if h > 2.0 * hw {
                    // h ≤ 笔宽时上下两条已覆盖全部，中段无需再补（也避免负高）。
                    canvas.fill_rect(x - hw, y + hw, 2.0 * hw, h - 2.0 * hw, &wp);
                    canvas.fill_rect(x2 - hw, y + hw, 2.0 * hw, h - 2.0 * hw, &wp);
                }
            }
        }
        Shape::Circle { center, radius } => {
            // ★ 圆环几何修复（用户报告：直径放大后圆环有缺角）：
            // 旧实现用 32 段折线近似圆心轨迹再描边——段数固定不随半径自适应，放大后
            // 弦线内切于圆，肉眼可见棱角与接缝。现改为把描边圆展开成**真正的环带区域**：
            // 外弧正向 + 内弧反向拼成一条闭合多边形（简单多边形，无自交），用 fill_polygon
            // 填充。闭合由构造保证（首尾角度都是 0），不存在端帽/接头，棱角随自适应细分
            // 被压到亚像素以下。
            let r = v.len(*radius);
            let half_w = v.len(width) * 0.5;
            let r_outer = r + half_w;
            let r_inner = (r - half_w).max(0.0);
            if r_outer <= 0.0 {
                return;
            }
            if r_inner <= CIRCLE_ANNULUS_MIN_INNER {
                // 内径退化：直接填实心圆。
                canvas.fill_circle(v.x(center.x), v.y(center.y), r_outer, &wp);
            } else {
                let pts = circle_annulus_polygon(v.x(center.x), v.y(center.y), r_outer, r_inner);
                canvas.fill_polygon(&pts, &wp);
            }
        }
        Shape::Ellipse {
            center,
            radius_x,
            radius_y,
        } => {
            // 同 Rect：这里原是 `fill_ellipse`，把椭圆填实了。改成与圆同构的**环带多边形**，
            // 于是空心、跟随线宽、且没有 32 段折线的棱角问题。
            let (cx, cy) = (v.x(center.x), v.y(center.y));
            let (rx, ry) = (v.len(*radius_x), v.len(*radius_y));
            let half_w = v.len(width) * 0.5;
            let (rx_outer, ry_outer) = (rx + half_w, ry + half_w);
            let (rx_inner, ry_inner) = ((rx - half_w).max(0.0), (ry - half_w).max(0.0));
            if rx_outer <= 0.0 || ry_outer <= 0.0 {
                return;
            }
            if rx_inner <= CIRCLE_ANNULUS_MIN_INNER || ry_inner <= CIRCLE_ANNULUS_MIN_INNER {
                // 内径退化（线宽大于半个短轴）：退成实心椭圆，而不是画出一个自交的怪形状。
                canvas.fill_ellipse(cx, cy, rx_outer, ry_outer, &wp);
            } else {
                let pts = ellipse_annulus_polygon(cx, cy, rx_outer, ry_outer, rx_inner, ry_inner);
                canvas.fill_polygon(&pts, &wp);
            }
        }
        Shape::Line { a, b } => {
            canvas.draw_line(v.x(a.x), v.y(a.y), v.x(b.x), v.y(b.y), w, &wp);
        }
        Shape::Polyline { points, closed } => {
            let pts = draw.range(*points);
            if pts.is_empty() {
                return;
            }
            let windui_pts: Vec<(f32, f32)> = pts.iter().map(|p| (v.x(p.x), v.y(p.y))).collect();
            if *closed {
                canvas.fill_polygon(&windui_pts, &wp);
            } else {
                canvas.draw_polyline(&windui_pts, w, &wp);
            }
        }
    }
}

/// 翻译 DrawList 到 Canvas（带坐标偏移和缩放）。
///
/// `dx/dy` 是把**虚拟桌面物理坐标**搬到窗口本地坐标的平移量（= 负的虚拟屏原点 × scale）。
/// 副屏放在主屏左侧时虚拟屏原点 x 为负，没有这个平移，副屏上的特效会整体错位。
pub fn translate_offset(draw: &DrawList, canvas: &mut dyn Canvas, scale: f32, dx: f32, dy: f32) {
    let v = View { dx, dy, scale };
    // ── alpha 栈：`PushAlpha` / `PopAlpha` 的语义 ──
    //
    // ★ 这两条指令长期是空实现（`PushAlpha(_) => {}`）。后果是"整块键盘的不透明度"
    //   这类**容器级乘子**完全没有效果，而颜色自带的 alpha 有效 —— 两条路各走各的，
    //   用户看到的就是"设置无效"。语义与 CSS 的 `opacity` 一致：进入作用域后，
    //   域内每条指令的透明度**再乘**上累计值，嵌套连乘。
    //
    // 实现要点：`Vec` 里存的是**每层的累计值**，弹出时按剩余栈重算（`last()`），
    // 而不是 `alpha_now /= a` —— 浮点除法的往返误差会在反复开关面板时累积成可见偏差。
    //
    // 注意 `DrawCmd::Clear` **不参与**：它的语义是"把这块矩形抹成透明"（清屏指令），
    // 不是叠加图元，乘 alpha 只会让它失效。
    let mut alpha_stack: Vec<f32> = Vec::new();
    let mut alpha_now: f32 = 1.0;
    for cmd in draw.ordered() {
        match cmd {
            DrawCmd::Fill {
                shape,
                paint,
                alpha,
                blend,
            } => {
                // 渐变要按图元自己的矩形归一化，`bounds` 只能在 `fill_shape` 里算（那里
                // 才拿得到 `DrawList` 去解析折线点），所以这里直接传 `Paint`，不预先转。
                fill_shape(
                    shape,
                    draw,
                    canvas,
                    paint,
                    CmdStyle {
                        alpha: *alpha * alpha_now,
                        blend: *blend,
                    },
                    v,
                );
            }
            DrawCmd::Stroke {
                shape,
                paint,
                alpha,
                width,
                cap: _,
                join: _,
                blend,
            } => {
                stroke_shape(
                    shape,
                    draw,
                    canvas,
                    paint,
                    CmdStyle {
                        alpha: *alpha * alpha_now,
                        blend: *blend,
                    },
                    *width,
                    v,
                );
            }
            DrawCmd::PushAlpha(a) => {
                alpha_now *= a.clamp(0.0, 1.0);
                alpha_stack.push(alpha_now);
            }
            DrawCmd::PopAlpha => {
                alpha_stack.pop();
                // 按剩余栈重算（不用除法还原因误差）。
                alpha_now = alpha_stack.last().copied().unwrap_or(1.0);
            }
            DrawCmd::Clear(rect) => {
                canvas.clear_rect(v.x(rect.x), v.y(rect.y), v.len(rect.w), v.len(rect.h));
            }
            DrawCmd::Image { idx, dest, alpha } => {
                // 位图像素在 DrawList::images[idx]（非预乘 RGBA8，与本模块货币一致）。
                // 先按 View 把目标矩形从世界坐标变换到画布本地逻辑像素，再 blit。
                //
                // ★ 走 `cached_image` 而不是直接 `WImage::from_rgba`：叠层底图会让这条
                //   指令**每帧**出现，而 `from_rgba` 是"整屏拷贝 + 逐像素预乘"，
                //   不缓存就是每帧重建一张屏幕大小的位图。
                if let Some(bmp) = draw.images().get(*idx) {
                    if let Some(img) = cached_image(bmp) {
                        let x = v.x(dest.x).round() as i32;
                        let y = v.y(dest.y).round() as i32;
                        let w = v.len(dest.w).max(1.0).round() as i32;
                        let h = v.len(dest.h).max(1.0).round() as i32;
                        canvas.draw_image(
                            &img,
                            Rect::new(x, y, w, h),
                            Fit::Fill,
                            0.0,
                            *alpha * alpha_now,
                        );
                    }
                }
            }
            DrawCmd::Text {
                x,
                y,
                text,
                size,
                color,
                alpha,
            } => {
                // `DrawList` 的文字锚点是**排版盒左上角**（调用方用 `advance()` 自己排版、
                // 逐段累加 x），而 windui 的 `draw_text` 在矩形内水平按 `align`、垂直居中。
                // 故按字体实际测量的尺寸搭一个从锚点开始的矩形，文本才落在锚点上。
                let ts = TextStyle::new(v.len(*size));
                let m = canvas.measure_text(text, &ts);
                // ★ 逐字符真实推进宽回写 key_display 的字宽缓存（DirectWrite 同源）：
                //   下一次帧构建时，segment 边界的累计 x 与这里的实际渲染完全一致，
                //   键名墨迹压空格在结构上不可能发生（2026-09-22 "LMBx 5" 根治）。
                //   只测缓存未命中的字符——DWrite 内部有测量缓存，稳态零额外开销。
                let scale = v.len(1.0);
                for ch in text.chars() {
                    if !crate::modules::key_display::layout::has_measured_advance(ch, *size) {
                        let cw = canvas.measure_text(&ch.to_string(), &ts).w as f32 / scale;
                        crate::modules::key_display::layout::set_measured_advance(ch, *size, cw);
                    }
                }
                let rect = Rect::new(
                    v.x(*x).round() as i32,
                    v.y(*y).round() as i32,
                    m.w.max(1),
                    m.h.max(1),
                );
                canvas.draw_text(
                    text,
                    rect,
                    // `alpha: None` 表示"颜色自带 alpha 就够了"，但容器级乘子仍要生效
                    // → 先补 1.0 再乘（`to_color_alpha` 对 `Some(1.0)` 是恒等变换）。
                    to_color_alpha(*color, Some(alpha.unwrap_or(1.0) * alpha_now)),
                    Align::Start,
                    &ts,
                );
            }
            DrawCmd::TextCentered {
                center,
                text,
                size,
                color,
                alpha,
            } => {
                draw_text_centered(
                    canvas,
                    text,
                    *size,
                    *center,
                    *color,
                    Some(alpha.unwrap_or(1.0) * alpha_now),
                    v,
                    None,
                );
            }
            DrawCmd::TextInBox {
                rect,
                text,
                size,
                color,
                alpha,
            } => {
                // 矩形 → 中心 + 宽度上限，剩下的交给同一个居中函数。
                let center =
                    crate::core::geometry::Point::new(rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
                draw_text_centered(
                    canvas,
                    text,
                    *size,
                    center,
                    *color,
                    Some(alpha.unwrap_or(1.0) * alpha_now),
                    v,
                    Some(rect.w),
                );
            }
        }
    }
}

/// 等高数字（lining figures）的**墨迹高**相对字号的比例。
///
/// 数字在 Windows UI 字体里都是"等高数字"——墨迹高度 = 大写高，各家比值很接近：
/// Segoe UI `1434/2048 = 0.7002`、Arial 0.716、Tahoma 0.727、微软雅黑 ≈0.72。
/// 取 **0.72** 是这批值的中间值：最坏情况（Segoe UI）偏差 0.02 em，在 20px 字号上
/// 只有 0.4px，落在像素量化误差以内。
///
/// **为什么需要它**：windui 的 `LineMetrics` 只给 ascent/descent（**行盒**），不给大写高。
/// 直接用行盒居中会让数字偏低——行盒的升部还给重音字母留了空间，数字用不到
/// （同类实现踩过这个坑：数字"显得偏下"）。
/// 想做得分毫不差需要**墨迹盒**（拿到每个字的实际墨迹范围），而我们的
/// `Canvas` 只暴露排版盒度量，故取这条业界通行的比值折中。
const DIGIT_CAP_HEIGHT_EM: f32 = 0.72;

/// 自适应缩字号时的左右留白（世界单位换算后的像素）：不留白的话字会贴着键帽描边。
const TEXT_FIT_PAD: f32 = 3.0;

/// 自适应缩字号的**下限**（原字号的比例）。再往下缩就读不出来了，
/// 宁可让它略微溢出，也不要给一颗看不清的键帽。
const MIN_FIT_SCALE: f32 = 0.55;

/// 以 `center` 为**几何中心**画一行文字（序号徽标用）。
///
/// 见 [`DrawCmd::TextCentered`] 的说明：模块层没有字体、算不出排版盒高，所以这份
/// "居中的活"必须在渲染层干——这里才有 `measure_text`（真实排版宽度）与
/// `text_line_metrics`（真实 ascent/descent）。
///
/// 两轴各自的做法：
///   · **水平**——按实测排版宽度回退半个宽度。不再用 `0.55 em × 位数` 的估算：多位数
///     时那个估算会累积偏差（"12" 与 "11" 的真实宽度也不同）。
///   · **垂直**——把基线放到 `center.y + 大写高/2`，让**墨迹**（不是行盒）居中。
///
/// `fit_width` = `Some(w)` 时多一步"放不下就缩"：按实测宽度把字号压到 `w` 以内，
/// 缩到 [`MIN_FIT_SCALE`] 就不再缩（再小读不出来，宁可让它略微溢出）。
// ★ clippy::too_many_arguments：文本居中渲染的参数全集（画布/文案/字号/中心点/颜色/
//   透明度/视图/限宽），一次传齐；拆 struct 会让每个调用点都多一道打包。
#[allow(clippy::too_many_arguments)]
fn draw_text_centered(
    canvas: &mut dyn Canvas,
    text: &str,
    size: f32,
    center: crate::core::geometry::Point,
    color: Argb,
    alpha: Option<f32>,
    v: View,
    fit_width: Option<f32>,
) {
    let size = v.len(size);
    if size <= 0.0 {
        return;
    }
    // 自适应缩字号：一段文字在给定盒宽里放不下时按比例压，压到 55% 为止。
    let avail = fit_width.map(|w| (v.len(w) - TEXT_FIT_PAD * 2.0).max(1.0));
    let size = match avail {
        Some(w) => {
            let w0 = canvas.measure_text(text, &TextStyle::new(size)).w as f32;
            if w0 > w && w0 > 0.0 {
                (size * w / w0).max(size * MIN_FIT_SCALE)
            } else {
                size
            }
        }
        None => size,
    };
    let ts = TextStyle::new(size);
    let m = canvas.measure_text(text, &ts);
    let lm = canvas.text_line_metrics(text, &ts);

    let baseline = v.y(center.y) + DIGIT_CAP_HEIGHT_EM * 0.5 * size;
    let rect = Rect::new(
        (v.x(center.x) - m.w as f32 / 2.0).round() as i32,
        (baseline - lm.ascent).round() as i32,
        m.w.max(1),
        // 盒高恰取自然行高 → `draw_text` 的"矩形内垂直居中"退化为顶对齐，
        // 基线严格落在 `盒顶 + ascent`（windui `text::LineMetrics` 的既定约定）。
        lm.height().ceil().max(1.0) as i32,
    );
    canvas.draw_text(text, rect, to_color_alpha(color, alpha), Align::Start, &ts);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::draw::BlendMode;
    use crate::core::geometry::{Point, Rect as CoreRect};
    use windui::render::RenderTarget;

    /// 缓存必须有界（[`IMG_CACHE_CAP`]）：贴图历史环长期持有每张位图的 `Rc`，"只留
    /// 还有别人的条目"这条 retain 永远筛不掉它们 —— 没有上限就会跟着环一起无界增长
    /// （1080p 一条 ≈ 8.3MB）。回归锁：2026-10-01 用户报"开过功能再关，内存回不去"。
    #[test]
    fn image_cache_stays_bounded() {
        clear_image_cache();
        for i in 0..(IMG_CACHE_CAP as u32 + 3) {
            let bmp = Rc::new(RgbaBitmap {
                w: 2,
                h: 2,
                rgba: vec![i as u8; 16],
            });
            // 每个 `bmp` 都有一份外部持有者（模拟贴图环握着 `Rc`）。
            let _keep = bmp.clone();
            assert!(cached_image(&bmp).is_some(), "第 {i} 张应能建图");
        }
        let n = IMG_CACHE.with(|c| c.borrow().len());
        assert!(
            n <= IMG_CACHE_CAP,
            "缓存条目数 {n} 超过上限 {IMG_CACHE_CAP}"
        );
        clear_image_cache();
    }

    /// ★ LRU 而非 FIFO：命中项必须提升到队尾。回归锁（10 号报告 §11.4⑤）——旧实现
    ///   命中只 clone 返回，"在用但插入得早"的那张会被 `remove(0)` 稳定逐出、每帧重解码。
    ///   场景：A 先入缓存，B/C/D 随后挤进来；此后每帧都在用 A。A 命中提升后，
    ///   再灌一张 E 触发淘汰，被丢的必须是 B（最久未用）而不是 A。
    #[test]
    fn image_cache_promotes_hits_to_lru_tail() {
        clear_image_cache();
        let bmp = |v: u8| {
            Rc::new(RgbaBitmap {
                w: 2,
                h: 2,
                rgba: vec![v; 16],
            })
        };
        let a = bmp(1);
        let _ka = a.clone();
        assert!(cached_image(&a).is_some());
        let b = bmp(2);
        let _kb = b.clone();
        assert!(cached_image(&b).is_some());
        let c = bmp(3);
        let _kc = c.clone();
        assert!(cached_image(&c).is_some());
        let d = bmp(4);
        let _kd = d.clone();
        assert!(cached_image(&d).is_some());
        // A 被反复使用（命中 → 提升队尾）
        for _ in 0..3 {
            assert!(cached_image(&a).is_some(), "A 应命中缓存");
        }
        // 灌第五张触发淘汰。★ 此后**绝不再访问 A**——旧 FIFO 实现里 A 若被逐出，
        // 一次再访问就会重建入缓存、把证据抹掉（首版测试就是这么假绿的）。
        let e = bmp(5);
        let _ke = e.clone();
        assert!(cached_image(&e).is_some());
        // LRU：A 命中提升后是最新的，被丢的必须是最久未用的 B。
        // FIFO（旧实现）：remove(0) 丢的恰是刚用过的 A——用这条断言红掉它。
        let keys: Vec<usize> = IMG_CACHE.with(|c| c.borrow().iter().map(|(k, _)| k.ptr).collect());
        assert_eq!(keys.len(), IMG_CACHE_CAP, "缓存应恰好 {IMG_CACHE_CAP} 条");
        assert!(
            keys.contains(&(a.rgba.as_ptr() as usize)),
            "命中提升后 A 必须还在缓存里"
        );
        assert!(
            !keys.contains(&(b.rgba.as_ptr() as usize)),
            "最久未用的 B 必须已被逐出"
        );
        clear_image_cache();
    }

    /// 字节上限（[`IMG_CACHE_BYTES_CAP`]）：两张 2304×2304（各 ~20.3MiB）合计
    /// 明显超过 32MiB → 先来的必须被逐出，哪怕条数远没到 4。防 4K 级大图按条数
    /// 预算被打穿（4 × 33MB = 132MB）。★ 尺寸刻意避开 2048×2048——单张 16MiB 的
    /// 两倍恰好 == 32MiB，落在 `>` 判据的边界上，不是要验证的语义。
    #[test]
    fn image_cache_evicts_by_bytes() {
        clear_image_cache();
        const BIG: usize = 2304;
        let big = |v: u8| {
            Rc::new(RgbaBitmap {
                w: BIG as u32,
                h: BIG as u32,
                rgba: vec![v; BIG * BIG * 4],
            })
        };
        let a = big(1);
        let _keep_a = a.clone();
        assert!(cached_image(&a).is_some(), "第一张大图应能建图");
        let b = big(2);
        let _keep_b = b.clone();
        assert!(cached_image(&b).is_some(), "第二张大图应能建图");
        let cache = IMG_CACHE.with(|c| c.borrow().clone());
        assert_eq!(
            cache.len(),
            1,
            "两张 20.3MiB 合计超过 32MiB 字节上限，应只剩 1 条"
        );
        assert_eq!(cache[0].0.len, b.rgba.len(), "留下的应是最新那张");
        clear_image_cache();
    }

    /// 单条超过字节上限时**允许独占缓存**：图必须画得出来，淘汰循环到空即止，
    /// 不许死转、也不许拒绝建图。
    #[test]
    fn image_cache_allows_single_oversized_entry() {
        clear_image_cache();
        // 4K RGBA = 33.2MB > 32MiB。
        let huge = Rc::new(RgbaBitmap {
            w: 3840,
            h: 2160,
            rgba: vec![7; 3840 * 2160 * 4],
        });
        let _keep = huge.clone();
        assert!(cached_image(&huge).is_some(), "超限大图仍应能建图");
        let cache = IMG_CACHE.with(|c| c.borrow().clone());
        assert_eq!(cache.len(), 1, "独占缓存，且无死转");
        clear_image_cache();
    }

    /// 把 DrawList **真的光栅化**到一块离屏 RGBA 缓冲，返回该缓冲（预乘 RGBA）。
    ///
    /// 走的是生产用的同一个 `DibTarget` + [`translate_offset`]，只是目标内存由测试提供。
    /// 这样像素级断言不必等窗口、不必提权、也不会被 UIPI 拦——`cargo test` 里就能跑。
    /// `NullTextEngine` 只影响文字（本组测试不画字）。
    fn rasterize(list: &DrawList, w: u32, h: u32) -> Vec<u8> {
        let mut buf = vec![0_u8; (w * h * 4) as usize];
        {
            let mut eng = windui::text::NullTextEngine;
            let mut target = windui::render::DibTarget::new(&mut buf, w, h);
            let mut canvas = target.make_canvas(&mut eng, 1.0);
            translate_offset(list, &mut *canvas, 1.0, 0.0, 0.0);
        }
        buf
    }

    /// 预乘 RGBA 的 alpha 通道。缓冲初始全透明，故 alpha > 0 即"这里有墨"。
    fn alpha_at(buf: &[u8], w: u32, x: u32, y: u32) -> u8 {
        buf[((y * w + x) * 4 + 3) as usize]
    }

    /// 描边矩形必须是**空心**的。
    ///
    /// 锁的是一处真实回归：翻译层曾把 `Shape::Rect` 的描边走成
    /// `fill_round_rect(.., radius.max(width * 0.5), ..)`——把整个矩形填实。表现是
    /// 「矩形」工具拖出来一个实心色块、工具栏的方框图标变成实心方块；而颜色/线型照样
    /// 生效，所以看着像"设计如此"。`main/` 线本来就是真描边，是重写时退化掉的。
    #[test]
    fn stroked_rect_is_hollow() {
        let mut dl = DrawList::new();
        dl.stroke_rect(
            CoreRect::new(20.0, 20.0, 40.0, 40.0),
            4.0,
            Argb(0xFF00_0000),
        );
        let buf = rasterize(&dl, 80, 80);

        assert_eq!(alpha_at(&buf, 80, 40, 40), 0, "矩形内部必须是空的");
        assert_eq!(alpha_at(&buf, 80, 30, 30), 0, "内部四分之一处也必须是空的");
        for (x, y, what) in [
            (20, 40, "左边"),
            (60, 40, "右边"),
            (40, 20, "上边"),
            (40, 60, "下边"),
        ] {
            assert!(
                alpha_at(&buf, 80, x, y) > 200,
                "{what}中点 ({x},{y}) 应着墨，实得 alpha={}",
                alpha_at(&buf, 80, x, y)
            );
        }
    }

    /// 描边椭圆同样必须空心（同一处回归的另一半：原先直接 `fill_ellipse` 填实）。
    #[test]
    fn stroked_ellipse_is_hollow() {
        let mut dl = DrawList::new();
        // 扁椭圆：圆心 (40,40)，rx=30 / ry=10。
        dl.stroke_ellipse(
            CoreRect::new(10.0, 30.0, 60.0, 20.0),
            4.0,
            Argb(0xFF00_0000),
        );
        let buf = rasterize(&dl, 80, 80);

        assert_eq!(alpha_at(&buf, 80, 40, 40), 0, "椭圆内部必须是空的");
        assert_eq!(
            alpha_at(&buf, 80, 25, 40),
            0,
            "内部（离长轴端点还远）必须是空的"
        );
        assert!(alpha_at(&buf, 80, 10, 40) > 150, "长轴左端应着墨");
        assert!(alpha_at(&buf, 80, 70, 40) > 150, "长轴右端应着墨");
        assert!(alpha_at(&buf, 80, 40, 30) > 150, "短轴上端应着墨");
        assert!(alpha_at(&buf, 80, 40, 50) > 150, "短轴下端应着墨");
    }

    /// 描边圆空心（与上两条同源，防止"顺手把圆也改回填实"）。
    #[test]
    fn stroked_circle_is_hollow() {
        let mut dl = DrawList::new();
        dl.stroke_circle(Point::new(40.0, 40.0), 20.0, 4.0, Argb(0xFF00_0000));
        let buf = rasterize(&dl, 80, 80);

        assert_eq!(alpha_at(&buf, 80, 40, 40), 0, "圆的内部必须是空的");
        for (x, y) in [(20, 40), (60, 40), (40, 20), (40, 60)] {
            assert!(
                alpha_at(&buf, 80, x, y) > 150,
                "圆周 ({x},{y}) 应着墨，实得 alpha={}",
                alpha_at(&buf, 80, x, y)
            );
        }
    }

    /// 对照组：`fill_rect` 必须**填满**内部。
    ///
    /// 没有这一条，上面三条"内部是空的"断言在"什么都没画出来"时也会通过。
    #[test]
    fn fill_rect_covers_the_interior() {
        let mut dl = DrawList::new();
        dl.fill_rect(CoreRect::new(20.0, 20.0, 40.0, 40.0), Argb(0xFF00_0000));
        let buf = rasterize(&dl, 80, 80);
        assert_eq!(alpha_at(&buf, 80, 40, 40), 255, "填充矩形的内部必须满墨");
    }

    /// ★ 粗笔下方框四角完整封闭（用户报告：有一个角是缺的）。
    ///
    /// 旧实现用闭环折线描边，粗笔时拐角 Join 补不全就露底。现实现是四条边各填一个
    /// 矩形（上下通长、左右接在上下之间，零重叠），四角被上下横边完整盖住。
    /// 本测试在接近最粗档的宽度（16px）下把四角内外、四条接缝都扫一遍。
    #[test]
    fn stroked_rect_corners_fully_closed_thick_pen() {
        let mut dl = DrawList::new();
        dl.stroke_rect(
            CoreRect::new(30.0, 30.0, 100.0, 80.0),
            16.0,
            Argb(0xFF00_0000),
        );
        let buf = rasterize(&dl, 160, 140);
        // 四角及角内侧：旧实现缺角就在这些点露白。
        for (x, y, what) in [
            (31, 31, "左上角"),
            (128, 31, "右上角"),
            (31, 108, "左下角"),
            (128, 108, "右下角"),
            (34, 34, "左上角内侧"),
            (125, 34, "右上角内侧"),
            (34, 105, "左下角内侧"),
            (125, 105, "右下角内侧"),
        ] {
            assert!(
                alpha_at(&buf, 160, x, y) > 200,
                "{what} ({x},{y}) 应着墨，实得 alpha={}",
                alpha_at(&buf, 160, x, y)
            );
        }
        // 角外对角线：墨迹中心线压在边界上，向外 hw 距离内是斜置墨；超过 hw 后
        // 必须透明（封闭 ≠ 外扩）。取样点距角点 √(10²+10²) ≈ 14 > hw=8。
        for (x, y, what) in [
            (20, 20, "左上外角"),
            (140, 20, "右上外角"),
            (20, 120, "左下外角"),
            (140, 120, "右下外角"),
        ] {
            assert_eq!(alpha_at(&buf, 160, x, y), 0, "{what} ({x},{y}) 不应着墨");
        }
        // 左右条与上下条的接缝（y=48 在上条下缘之下、y=92 在下条上缘之上）：
        // 接缝处必须有墨，否则就是缺角。
        for (x, y, what) in [
            (32, 48, "左条上段"),
            (127, 48, "右条上段"),
            (32, 92, "左条下段"),
            (127, 92, "右条下段"),
        ] {
            assert!(
                alpha_at(&buf, 160, x, y) > 200,
                "{what} ({x},{y}) 应着墨，实得 alpha={}",
                alpha_at(&buf, 160, x, y)
            );
        }
    }

    /// 线型切段之后，落到屏上的是**多段**墨。
    ///
    /// 锁的是"面板里选了线型但画出来还是实线"那类回归：笔迹确实出现了，于是只验
    /// "有没有墨"发现不了，必须验"墨是不是断成了多段"。这里走完整链路
    /// （`dash::segments` → `DrawList` → 光栅化），而不是只测几何。
    #[test]
    fn dashed_stroke_lands_as_several_ink_runs() {
        use crate::modules::drawing::dash;
        use crate::modules::drawing::pens::PenStyle;

        let pts: Vec<Point> = (0..=120)
            .map(|i| Point::new(10.0 + i as f32, 40.0))
            .collect();

        // 同一批点，实线一条折线 vs 虚线切段。
        let mut solid = DrawList::new();
        solid.draw_polyline(pts.clone(), 4.0, Argb(0xFF00_0000));
        let mut dashed = DrawList::new();
        for seg in dash::segments(&pts, PenStyle::Dashed, 4.0) {
            dashed.draw_polyline(seg, 4.0, Argb(0xFF00_0000));
        }

        let runs = |dl: &DrawList| {
            let buf = rasterize(dl, 160, 80);
            let mut runs = 0;
            let mut in_run = false;
            for x in 0..160_u32 {
                let hit = (0..80_u32).any(|y| alpha_at(&buf, 160, x, y) > 128);
                if hit && !in_run {
                    runs += 1;
                }
                in_run = hit;
            }
            runs
        };

        assert_eq!(runs(&solid), 1, "实线横向铺过去只该有 1 段");
        let n = runs(&dashed);
        assert!(
            n >= 5,
            "4px 笔宽（16 划 / 8 空）在 120px 上应切出 5 段以上，实得 {n} 段"
        );
    }

    /// 环带多边形必须严格闭合且半径正确（用户报告的“放大缺角”回归锁）。
    #[test]
    fn annulus_polygon_is_closed_and_radii_hold_at_any_size() {
        for &(r_outer, r_inner) in &[
            (200.0_f32, 102.0_f32), // 旧配置：外径400/弧宽98 的中心线151→内外 200/102
            (400.0, 300.0),         // 极限放大
            (25.0, 12.0),           // 小尺寸
        ] {
            let pts = circle_annulus_polygon(0.0, 0.0, r_outer, r_inner);
            let seg = circle_segments(r_outer) as usize;
            // 外弧 n+1 点 + 内弧 n+1 点，首尾角度都是 0
            assert_eq!(pts.len(), 2 * seg + 2);
            // 外弧首点 = (r_outer, 0)，内弧末点 = (r_inner, 0)：接缝是同一条径向边
            assert!((pts[0].0 - r_outer).abs() < 1e-4 && pts[0].1.abs() < 1e-4);
            let last = *pts.last().unwrap();
            assert!((last.0 - r_inner).abs() < 1e-4 && last.1.abs() < 1e-4);
            // 所有点半径都在 [r_inner, r_outer] 内（多边形不越界）
            for &(x, y) in &pts {
                let rad = (x * x + y * y).sqrt();
                assert!(rad <= r_outer + 1e-3 && rad >= r_inner - 1e-3);
            }
        }
    }

    /// 细分数必须随半径自适应：大圆段数明显多于小圆（老实现恒 32 段的根因）。
    #[test]
    fn segments_scale_with_radius() {
        let small = circle_segments(50.0);
        let mid = circle_segments(200.0);
        let huge = circle_segments(1000.0);
        assert!(small >= 24 && small < mid && mid < huge && huge <= 512);
        // 200px 半径下段数应远超 32，弦矢 ≤ 0.2px
        assert!(mid > 48, "r=200 时段数应显著大于 32（实际 {mid}）");
    }

    fn v() -> View {
        View {
            dx: 0.0,
            dy: 0.0,
            scale: 1.0,
        }
    }

    fn solid(hex: u32) -> Paint {
        Paint::Solid(Argb(hex), BlendMode::SourceOver)
    }

    /// `alpha` 是**乘子**：0.5 必须把 255 的半透明色砍到 ~127。
    ///
    /// 守的是"整条指令透明度被静默忽略"那类退化——忽略后 `pixie_dust` 的三层辉光
    /// 全按 100% 画，粒子从闪光变成实心色块。
    #[test]
    fn alpha_multiplier_scales_solid_color() {
        let wp = to_wpaint(&solid(0xFF11_2233), (0.0, 0.0, 10.0, 10.0), v(), 0.5);
        assert_eq!(wp.color.a, 127, "0xFF * 0.5 应约为 127");
        assert_eq!((wp.color.r, wp.color.g, wp.color.b), (0x11, 0x22, 0x33));
    }

    /// 乘子为 1.0 时必须原样透传（便捷构造器与本项目绝大多数调用走这条）。
    #[test]
    fn alpha_multiplier_one_is_identity() {
        let wp = to_wpaint(&solid(0x80445566), (0.0, 0.0, 4.0, 4.0), v(), 1.0);
        assert_eq!(wp.color.a, 0x80);
    }

    /// `Blend::Plus` 必须落到 windui 的加色模式上；`Clear` 必须落到 windui 的擦除模式
    /// （聚光灯挖洞用）。
    #[test]
    fn blend_plus_maps_to_windui_plus() {
        assert_eq!(to_blend(Blend::Plus), WBlend::Plus);
        assert_eq!(to_blend(Blend::SourceOver), WBlend::SourceOver);
        assert_eq!(to_blend(Blend::Clear), WBlend::Clear);
    }

    /// 径向渐变不能被丢掉，且 `alpha` 要乘进每个色标。
    ///
    /// 这条正是「辉光圆影」白屏的回归测试：翻译层早年把非 `Solid` 一律回退成
    /// 不透明白色，白圆落在白底上等于看不见。
    #[test]
    fn radial_gradient_survives_and_scales_stops() {
        let center = Point::new(100.0, 100.0);
        let p = Paint::Radial {
            center,
            radius: 10.0,
            stops: vec![
                GradStop {
                    offset: 0.0,
                    color: Argb(0xFFFF0000),
                },
                GradStop {
                    offset: 1.0,
                    color: Argb(0x00FF0000),
                },
            ],
        };
        let shape = Shape::Circle {
            center,
            radius: 10.0,
        };
        let mut dl = DrawList::new();
        dl.clear();
        let wp = to_wpaint(&p, shape_bounds(&shape, &dl), v(), 0.5);

        let g = wp.gradient.expect("径向渐变不能在翻译时被丢弃");
        let Gradient::Radial {
            center: c,
            radius,
            stops,
        } = g
        else {
            panic!("应保持 Radial 变体");
        };
        // 圆心落在图元包围盒正中；半径是"绝对半径 / 短边全长"——
        // 圆半径 = 半个包围盒，故归一化后是 0.5，着色半径恰好等于圆半径。
        assert!((c.0 - 0.5).abs() < 1e-3 && (c.1 - 0.5).abs() < 1e-3);
        assert!(
            (radius - 0.5).abs() < 1e-3,
            "圆的外接方框短边=2r，故归一化半径应为 0.5（windui sk_shader 的 radius*short），实际 {radius}"
        );
        assert_eq!(stops.len(), 2);
        assert_eq!(stops[0].color.a, 127);
        assert_eq!(stops[1].color.a, 0);
    }

    /// 线性渐变的端点按**图元自己的矩形**归一化（不是全屏、也不是世界坐标绝对值）。
    #[test]
    fn linear_gradient_normalizes_against_shape_bounds() {
        let rect = CoreRect::new(200.0, 100.0, 400.0, 50.0);
        let p = Paint::Linear {
            from: Point::new(200.0, 100.0),
            to: Point::new(600.0, 100.0),
            stops: vec![
                GradStop {
                    offset: 0.0,
                    color: Argb(0xFF000000),
                },
                GradStop {
                    offset: 1.0,
                    color: Argb(0xFFFFFFFF),
                },
            ],
        };
        let shape = Shape::Rect { rect, radius: 0.0 };
        let dl = DrawList::new();
        let wp = to_wpaint(&p, shape_bounds(&shape, &dl), v(), 1.0);

        let Some(Gradient::Linear { start, end, .. }) = wp.gradient else {
            panic!("应保持 Linear 变体");
        };
        assert!((start.0 - 0.0).abs() < 1e-3, "左端应归一化到 0");
        assert!((end.0 - 1.0).abs() < 1e-3, "右端应归一化到 1");
    }

    /// ★ `PushAlpha` / `PopAlpha` 必须真的生效（容器级不透明度乘子）。
    ///
    /// 回归锚点：这两条指令曾是**空实现**（`DrawCmd::PushAlpha(_) => {}`），于是
    /// "整块键盘的总不透明度"完全无效，而颜色自带的 alpha 有效 —— 用户看到的现象
    /// 就是"设置无效"。这里用真实光栅化的像素 alpha 锁住"乘进域内每条指令"。
    #[test]
    fn push_alpha_scales_enclosed_draws() {
        let rect = CoreRect::new(10.0, 10.0, 20.0, 20.0);
        let one = |a: f32| {
            let mut dl = DrawList::new();
            dl.push_alpha(a);
            dl.fill_rect(rect, Argb(0xFF00_0000));
            dl.pop_alpha();
            rasterize(&dl, 40, 40)
        };

        let v = alpha_at(&one(1.0), 40, 20, 20);
        assert!(v >= 250, "alpha=1.0 应当完全不透明，实得 {v}");

        let v = alpha_at(&one(0.5), 40, 20, 20);
        assert!(
            (110..=145).contains(&v),
            "alpha=0.5 该落在半透明带（约 127），实得 {v}"
        );

        // 嵌套 = 连乘：0.5 × 0.5 ≈ 0.25 → 约 64。
        let mut dl = DrawList::new();
        dl.push_alpha(0.5);
        dl.push_alpha(0.5);
        dl.fill_rect(rect, Argb(0xFF00_0000));
        dl.pop_alpha();
        dl.pop_alpha();
        let v = alpha_at(&rasterize(&dl, 40, 40), 40, 20, 20);
        assert!((45..=85).contains(&v), "嵌套 0.5×0.5 该落在约 64，实得 {v}");
    }

    /// 弹出后必须**完整还原**（按剩余栈重算，不留浮点残留）；
    /// 并且 `Clear` 不受 alpha 影响（它是清屏指令，不是图元）。
    #[test]
    fn pop_alpha_restores_and_clear_is_unaffected() {
        let rect = CoreRect::new(10.0, 10.0, 20.0, 20.0);

        let mut dl = DrawList::new();
        dl.push_alpha(0.25);
        dl.pop_alpha();
        dl.fill_rect(rect, Argb(0xFF00_0000));
        let v = alpha_at(&rasterize(&dl, 40, 40), 40, 20, 20);
        assert!(v >= 250, "弹出后必须完全还原为不透明，实得 {v}");

        // 域内先画一块不透明、再 clear：clear 之后该矩形必须变回全透明
        // （若 clear 被乘上 alpha，就会残留一块半透明底）。
        let mut dl = DrawList::new();
        dl.push_alpha(0.5);
        dl.fill_rect(rect, Argb(0xFF00_0000));
        dl.clear_rect(rect);
        dl.pop_alpha();
        let v = alpha_at(&rasterize(&dl, 40, 40), 40, 20, 20);
        assert_eq!(v, 0, "clear 必须真的抹透明，不受 push_alpha 影响，实得 {v}");
    }

    // ─── 按键弹窗：背景不透明度 + 文字垂直居中（真字体 · 像素判据）───
    //
    // 为什么放在翻译层的测试里：这两条**只有穿过翻译层之后**才量得到。模块层能证明的
    // 只是"我发了带 alpha 的 Fill / 锚点等于 text_top"（那两条在
    // `modules/key_display/render.rs` 的用例里），而用户看到的是**像素**。

    /// 带**真字体**的光栅化：要量墨迹，字就必须真的画出来（`NullTextEngine` 画不出）。
    fn rasterize_text(list: &DrawList, w: u32, h: u32) -> Vec<u8> {
        let mut buf = vec![0_u8; (w * h * 4) as usize];
        {
            let mut eng = windui::text::DWriteEngine::new();
            let mut target = windui::render::DibTarget::new(&mut buf, w, h);
            let mut canvas = target.make_canvas(&mut eng, 1.0);
            translate_offset(list, &mut *canvas, 1.0, 0.0, 0.0);
        }
        buf
    }

    /// 假度量器（与模块层 `FakeMetrics` 同款）：半角 0.5em、行高 1.2em。
    struct PopupMetrics;

    impl crate::modules::key_display::layout::TextMeasure for PopupMetrics {
        fn measure(
            &mut self,
            s: &str,
            size_pt: f32,
        ) -> crate::modules::key_display::layout::TextMetrics {
            let mut w = 0.0f32;
            for c in s.chars() {
                w += if c.is_ascii() { size_pt * 0.5 } else { size_pt };
            }
            crate::modules::key_display::layout::TextMetrics {
                width: w,
                ascent: size_pt * 0.8,
                height: size_pt * 1.2,
            }
        }
    }

    /// 走**真实链路**（`layout` → `render_key_popups`）产出一个弹窗的绘制指令，并回传盒矩形。
    ///
    /// 屏幕取 400×200、方位左上、偏移 5% → 盒落在 (20, 10)、宽 24 / 高 37.2，
    /// 缓冲 400×200 就够，采样点可以从盒矩形**算**出来而不必手写魔数。
    fn popup_draw(alpha: f32) -> (DrawList, CoreRect) {
        use crate::core::geometry::Size;
        use crate::modules::key_display::layout::layout;
        use crate::modules::key_display::merge::KeyItem;
        use crate::modules::key_display::render::render_key_popups;
        use crate::modules::key_display::settings::{KeyDisplaySettings, Placement};

        let settings = KeyDisplaySettings {
            font_size: 16.0,
            placement: Placement::TopLeft,
            top_offset: 5,
            left_offset: 5,
            popup_width: 20,
            opacity: alpha, // 盒不透明度 = opacity × item.opacity(=1)
            // 出厂背景已改半透明 #00000040（2026-09-25 晚）；本测试验证的是
            // 「盒不透明度缩放背景」，与出厂色无关，固定不透明黑以保判据稳定。
            bg_color: crate::core::color::Argb::new(255, 0, 0, 0),
            ..Default::default()
        };

        let items = vec![KeyItem::new("A", 1.0, true)];
        let mut metrics = PopupMetrics;
        let boxes = layout(&items, &settings, Size { w: 400.0, h: 200.0 }, &mut metrics);
        assert_eq!(boxes.len(), 1, "单条按键必须产出单个弹窗");
        let rect = boxes[0].rect;
        let mut dl = DrawList::new();
        render_key_popups(&boxes, &settings, &mut dl, settings.font_size);
        (dl, rect)
    }

    /// ★ 弹窗**背景**必须跟着不透明度变浅。
    ///
    /// 回归锚点（用户报「不透明度无法调整背景颜色」）：背景曾用 `fill_round_rect` 画，
    /// 而它内部的 `alpha` **恒为 1.0** —— "不透明度"于是只作用到文字与边框，底色始终
    /// 实心。代码上看不出"乘子没传下去"，但像素不会撒谎：α=0.5 的纯黑底在预乘缓冲里
    /// 就该是 α≈127。
    #[test]
    fn key_popup_background_scales_with_opacity() {
        let (full_dl, rect) = popup_draw(1.0);
        let (half_dl, _) = popup_draw(0.5);
        let (zero_dl, _) = popup_draw(0.0);

        // 盒内右下角：离右边 6px、离底边 8px —— 在 6px 圆角以内、碰不到左上角的文字
        // （1px 边框已删，2026-09-25 晚）。
        let px = (rect.x + rect.w - 6.0).round() as u32;
        let py = (rect.y + rect.h - 8.0).round() as u32;

        let full = alpha_at(&rasterize_text(&full_dl, 400, 200), 400, px, py);
        assert!(full >= 250, "不透明度 1.0 的底色应当实心，实得 α={full}");

        let half = alpha_at(&rasterize_text(&half_dl, 400, 200), 400, px, py);
        assert!(
            (110..=145).contains(&half),
            "不透明度 0.5 的底色应≈127，实得 α={half}"
        );

        // 对照组：α=0 时整盒都不该落笔 —— 没有它，"底色没画出来"也能让上面两条通过。
        let zero = rasterize_text(&zero_dl, 400, 200);
        assert!(zero.iter().all(|b| *b == 0), "不透明度 0 时不该有任何像素");
    }

    /// ★ 文本块必须在盒内**垂直居中**（用户报「文本偏下、没有垂直居中」）。
    ///
    /// 量**墨迹行**：盒内（避开 1px 边框与 6px 圆角）逐行扫 alpha，取最上/最下行，
    /// 比较上下留白。修复前锚点多加了一个 ascent（PyQt 的基线语义串到了"盒左上角"上），
    /// 字号 16 时上留白 ≈24px、下留白为**负**（字已经压出盒外）—— 差 30px 以上，
    /// 不是"看起来歪一点"。
    #[test]
    fn key_popup_text_is_vertically_centered() {
        let (dl, rect) = popup_draw(1.0);
        let buf = rasterize_text(&dl, 400, 200);

        let x0 = (rect.x + 10.0).round() as u32;
        let x1 = (rect.x + rect.w - 3.0).round() as u32;
        let y0 = (rect.y + 2.0).round() as u32;
        let y1 = (rect.y + rect.h - 2.0).round() as u32;

        let rows: Vec<u32> = (y0..y1)
            .filter(|&y| (x0..x1).any(|x| alpha_at(&buf, 400, x, y) > 40))
            .collect();
        assert!(!rows.is_empty(), "盒内必须真的画出文字，否则本条判据是空的");

        let ink_top = *rows.first().expect("已断言非空");
        let ink_bottom = *rows.last().expect("已断言非空");
        let gap_top = ink_top as f32 - (rect.y + 1.0);
        let gap_bottom = (rect.y + rect.h - 1.0) - ink_bottom as f32;

        // 实测基准（2026-09-18，字体 16 / 盒 24×37.2）：上 1.0px、下 2.2px，差 1.2px —— 这
        // 1.2px 是「A」**字形本身**的墨迹不对称（cap 高 < ascent，盒顶那截是内部行距），
        // 不是居中误差。修复前是 上≈24 / 下≈-11.5（差 35px 量级），判据分辨力足够。
        assert!(
            (gap_top - gap_bottom).abs() <= 2.5,
            "上下留白应基本相等（偏下 = 上大下小）：上 {gap_top}、下 {gap_bottom}"
        );
        assert!(
            gap_top >= 1.0 && gap_bottom >= 1.0,
            "文字必须完整落在盒内：上 {gap_top}、下 {gap_bottom}"
        );
    }

    /// ★ 聚光灯（`assets/lua/HoverDot/spotlight.lua`）真的在暗罩里**挖出了洞**。
    ///
    /// 为什么必须验到像素：这个脚本靠一条「外圆正向 + 内圆反向」的环带多边形挖洞，而
    /// `fill_polygon` 走的是 `FillRule::Winding`（非零环绕）—— 两圈**同向**时内圈的环绕数
    /// 是 ±2，照样被填实，洞就没了。此前那版内置实现正是栽在这条上（用户报"鼠标周围并没有
    /// 高亮"，即整屏被压成一块黑）。几何断言看不出这件事，像素看得出来。
    ///
    /// 三条判据合起来把"洞"钉死：
    ///   ① 光圈中心**一点墨都没有**（α = 0）；
    ///   ② 屏幕四角都被压暗到 `dim`（证明外圆罩满了整屏）；
    ///   ③ 透亮像素数 ≈ 光圈面积 πr²（同时证明洞是**圆的**、半径也是对的 —— 若是矩形洞
    ///      同"半径"下面积会大 27%，若外圆没盖满全屏这个数会更大）。
    ///
    /// 实测基准（2026-09-20，400×300 / 圆心 (120,90) / r=60 / dim=170）：透亮 11124 px，
    /// 理论 πr² = 11310 px，偏差 **-1.64%**（= 96 边形内接圆比真圆略小 + 抗锯齿边缘像素
    /// 不是严格的 0）。中心 α=0、四角 α=170。
    #[test]
    fn spotlight_lua_punches_a_real_hole_in_the_dim() {
        const W: u32 = 400;
        const H: u32 = 300;
        const CX: f32 = 120.0;
        const CY: f32 = 90.0;
        const R: f32 = 60.0;
        const DIM: u8 = 170;

        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/lua/HoverDot");
        let engine =
            crate::scripting::engine::ScriptingEngine::new(&dir).expect("脚本引擎应能建起来");
        engine
            .load_script("spotlight")
            .expect("assets/lua/HoverDot/spotlight.lua 应能加载");

        // 真实脚本 → 真实 DrawList（与运行时同一条通路，参数即面板 schema）。
        let mut list = DrawList::new();
        crate::scripting::api_draw::set_draw_target(&mut list);
        engine
            .update_env((CX, CY), (CX, CY), 0.0, (W as f32, H as f32), "trail")
            .expect("环境变量应能更新");
        engine
            .call_draw(&std::collections::HashMap::from([
                ("radius".to_string(), serde_json::json!(R)),
                ("dim".to_string(), serde_json::json!(DIM)),
            ]))
            .expect("draw 应能跑通");
        crate::scripting::api_draw::clear_draw_target();

        let buf = rasterize(&list, W, H);

        // ① 光圈中心：全透明。
        assert_eq!(
            alpha_at(&buf, W, CX as u32, CY as u32),
            0,
            "光圈中心必须完全透明（洞没挖出来 ⇒ 整屏被填成一块暗罩）"
        );
        // ② 四角：都被暗罩盖住。
        for (x, y) in [(2u32, 2u32), (W - 3, 2), (2, H - 3), (W - 3, H - 3)] {
            assert_eq!(
                alpha_at(&buf, W, x, y),
                DIM,
                "({x},{y}) 应被压暗到 dim —— 外圆没有罩满屏幕"
            );
        }
        // ③ 透亮区面积 ≈ πr²。容差 8%：环带多边形在两个圆的首点处有一条**重合的接缝边**
        //    （角度 0 处），光栅化可能在那条线上的一个像素列留下亚像素误差。
        let clear = (0..(W * H) as usize)
            .filter(|i| buf[i * 4 + 3] == 0)
            .count() as f32;
        let want = std::f32::consts::PI * R * R;
        eprintln!(
            "聚光灯实测：透亮 {clear:.0} px / 理论 πr² {want:.0} px（偏差 {:.2}%），\
             中心 α=0、四角 α={DIM}",
            (clear - want).abs() / want * 100.0
        );
        assert!(
            (clear - want).abs() / want < 0.08,
            "透亮区面积应 ≈ πr²（{want:.0}），实为 {clear:.0}：洞的形状 / 半径不对"
        );
    }

    /// ★★ 像素级验收：WASD 板的**鼠标左/右键必须与 W 同一行、且彼此平行**
    /// （2026-09-26 用户报"左键和右键应该平行，和 W 同一行"）。
    ///
    /// 为什么必须落到像素：几何单测（`layout::tests::wasd_gaming_scheme_shape`）锁的是
    /// 方案 JSON 里的 `pad`/`w`，而用户看到的是**画出来的键帽**。这两者之间还隔着
    /// `VirtualKeyboardModule::draw` 的整套摆放逻辑（屏幕矩形、缩放、偏移）—— 那一段
    /// 没人替它担保。这里拿真实模块产出真实 `DrawList`、经生产翻译层栅格化，再按
    /// `layout_boxes` 给出的矩形去**数像素**，两段就都验到了。
    ///
    /// 判据取每颗键帽**包围盒的中心**，比"有没有墨"强得多：墨有没有是二值的，
    /// 中心坐标才能回答"在不在这条水平线上"。
    #[test]
    fn wasd_board_paints_mouse_keys_beside_each_other_on_the_w_row() {
        let mut cfg = crate::core::config::Config::default();
        cfg.virtual_keyboard.enabled = true;
        cfg.virtual_keyboard.layout = "wasd_gaming".to_string();
        let mut m = crate::modules::virtual_keyboard::VirtualKeyboardModule::new(&cfg);
        use crate::modules::Module;
        let screen = crate::core::geometry::ScreenRect::FALLBACK;
        m.set_screen(screen);

        // ① 真实模块 → 真实 DrawList → 生产翻译层 → 像素。
        let mut list = DrawList::new();
        m.draw(&mut list, &cfg);
        assert!(!list.cmds().is_empty(), "游戏板该画出键帽");
        let (w, h) = (screen.w as u32, screen.h as u32);
        let buf = rasterize(&list, w, h);

        // ② 每颗键帽的墨迹中心（在它自己的布局矩形里数"有墨的像素"取质心）。
        let boxes = m.layout_boxes(&cfg.virtual_keyboard);
        let center = |code: crate::core::events::KeyCode| -> (f32, f32) {
            let b = boxes
                .iter()
                .find(|b| b.key.code == code)
                .unwrap_or_else(|| panic!("游戏板应含 {code:?}"));
            let r = b.rect;
            let (x0, y0) = (r.x.max(0.0) as u32, r.y.max(0.0) as u32);
            let (x1, y1) = (
                r.right().min(w as f32) as u32,
                r.bottom().min(h as f32) as u32,
            );
            let (mut sx, mut sy, mut n) = (0.0f32, 0.0f32, 0u32);
            for y in y0..y1 {
                for x in x0..x1 {
                    if alpha_at(&buf, w, x, y) > 0 {
                        sx += x as f32 + 0.5;
                        sy += y as f32 + 0.5;
                        n += 1;
                    }
                }
            }
            assert!(n > 0, "{code:?} 的矩形里一个像素都没画上");
            (sx / n as f32, sy / n as f32)
        };

        let w_key = center(crate::core::events::KeyCode::Char('w'));
        let lmb = center(crate::core::events::KeyCode::MouseLeft);
        let rmb = center(crate::core::events::KeyCode::MouseRight);
        let s_key = center(crate::core::events::KeyCode::Char('s'));

        // ③ 判据一：W / 左键 / 右键的中心 y 落在同一条水平线上（容差 1px = 取整噪声）。
        for (name, y) in [("左键", lmb.1), ("右键", rmb.1)] {
            assert!(
                (y - w_key.1).abs() <= 1.5,
                "{name} 应与 W 同一行：{name} y={y:.1} vs W y={wy:.1}",
                wy = w_key.1
            );
        }
        assert!(
            (lmb.1 - rmb.1).abs() <= 1.5,
            "左键与右键必须平行：{} vs {}",
            lmb.1,
            rmb.1
        );
        // ④ 判据二：左键在右键**左边**（与物理鼠标同向），且两者确实分开。
        assert!(
            lmb.0 < rmb.0 - 1.0,
            "左键应在右键左侧：左键 x={:.1} vs 右键 x={:.1}",
            lmb.0,
            rmb.0
        );
        // ⑤ 判据三：鼠标区在键盘区**右侧**（不是竖着排在下面）—— 旧版正是错在把左键
        //   沉到了第二行，这里用"A/S/D 那一行"当对照。
        assert!(
            lmb.0 > w_key.0,
            "鼠标区应在键盘区右侧：左键 x={:.1} vs W x={:.1}",
            lmb.0,
            w_key.0
        );
        assert!(
            s_key.1 > w_key.1 + 1.0,
            "S 应在 W 下面（第二行）：S y={:.1} vs W y={:.1}",
            s_key.1,
            w_key.1
        );
    }
}
