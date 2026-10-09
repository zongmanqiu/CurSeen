//! 直线/箭头/矩形/椭圆：两点成形 + 实时预览。

use super::stroke::StrokePoints;
use crate::core::geometry::{Point, Rect};

/// 形状工具。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeTool {
    Line,
    Arrow,
    Rect,
    Ellipse,
}

/// 根据两点创建形状。
/// Shift 约束（2026-09-22 用户需求）：把拖拽终点吸附到受限形态。
///
/// - 直线/箭头：**水平或垂直**（取位移绝对值大的轴，另一轴归到起点——用户口径
///   只要水平/垂直，不做 45° 吸附）；
/// - 矩形/椭圆：**正方形/正圆**（取 |dx|、|dy| 中较小者为边长，象限方向保留——
///   拖拽手感与主流画板一致：往哪边拖都成立）。
pub fn constrain_shift(start: Point, current: Point, tool: ShapeTool) -> Point {
    let dx = current.x - start.x;
    let dy = current.y - start.y;
    match tool {
        ShapeTool::Line | ShapeTool::Arrow => {
            if dx.abs() >= dy.abs() {
                Point {
                    x: current.x,
                    y: start.y,
                }
            } else {
                Point {
                    x: start.x,
                    y: current.y,
                }
            }
        }
        ShapeTool::Rect | ShapeTool::Ellipse => {
            let side = dx.abs().min(dy.abs());
            let sx = if dx >= 0.0 { side } else { -side };
            let sy = if dy >= 0.0 { side } else { -side };
            Point {
                x: start.x + sx,
                y: start.y + sy,
            }
        }
    }
}

#[cfg(test)]
mod shift_tests {
    use super::*;

    /// ★ Shift 约束（2026-09-22）：直线/箭头水平垂直吸附、矩形/椭圆正方/正圆。
    #[test]
    fn line_snaps_to_horizontal_or_vertical() {
        let s = Point::new(100.0, 100.0);
        // 偏水平 → 水平吸附
        assert_eq!(
            constrain_shift(s, Point::new(300.0, 160.0), ShapeTool::Line),
            Point::new(300.0, 100.0)
        );
        // 偏垂直 → 垂直吸附
        assert_eq!(
            constrain_shift(s, Point::new(160.0, 300.0), ShapeTool::Line),
            Point::new(100.0, 300.0)
        );
        // 箭头同规则
        assert_eq!(
            constrain_shift(s, Point::new(300.0, 160.0), ShapeTool::Arrow),
            Point::new(300.0, 100.0)
        );
    }

    #[test]
    fn rect_snaps_to_square() {
        let s = Point::new(100.0, 100.0);
        // 宽 200 高 160 → 取小者 160（正方形，方向保留）
        assert_eq!(
            constrain_shift(s, Point::new(300.0, 260.0), ShapeTool::Rect),
            Point::new(260.0, 260.0)
        );
        // 反方向拖（往左上）也成立
        assert_eq!(
            constrain_shift(s, Point::new(-60.0, -40.0), ShapeTool::Ellipse),
            Point::new(-40.0, -40.0)
        );
    }
}

pub fn make_shape(p1: Point, p2: Point, tool: ShapeTool, width: f32) -> StrokePoints {
    match tool {
        ShapeTool::Line => StrokePoints::Polyline {
            points: vec![p1, p2],
        },
        ShapeTool::Arrow => make_arrow(p1, p2, width),
        ShapeTool::Rect => StrokePoints::Rect {
            rect: Rect::from_points(p1, p2),
        },
        ShapeTool::Ellipse => StrokePoints::Ellipse {
            rect: Rect::from_points(p1, p2),
        },
    }
}

/// 箭头退化成直线的长度阈值（太短画不出头，只留一条线）。
pub const MIN_ARROW_LENGTH: f32 = 5.0;

/// 箭头尾端圆头的细分数（半圆被分成的弦数）。
///
/// 8 段在半径 ≤ 64px 时的弦矢约 0.8px，肉眼不可见；再多段对撤销栈的顶点开销不划算。
pub const ARROW_CAP_SEG: usize = 8;

/// 创建箭头几何 —— **一条闭合多边形，一次填充**。
///
/// # 为什么必须是一体
///
/// 旧实现把箭头拆成「描一条线 + 填一个三角」两个图元。α=255 时看不出来，但半透明下
/// 交界处**被混合两次**，那一段明显更深（`examples/shape_probe.rs` 的判据实测：单次叠加
/// 应 ≈127，实际最暗 64 —— 正是二次叠加的值）。
///
/// # 几何
///
/// 轮廓是一条**简单多边形**（不自交），按同一绕行方向列出：
///
/// ```text
///              尾左 ──────── 杆端面左 ── 三角底边左翼
///               │                                ＼
///               │                                 尖端
///               │                                ／
///              尾右 ──────── 杆端面右 ── 三角底边右翼
///                  （尾部 8 段半圆弧补上）
/// ```
///
/// 两条关键约束：
///   1. **杆端面与三角底边共面**（都在距尖端 `head_len` 处）→ 杆面与三角面的面积交集为
///      空，一次填充不可能在交界处叠加；
///   2. `head_half ≥ 3×width > width/2 = half` → 翼点恒在杆侧之外，多边形不会自交。
///
/// 尾端圆头也在这条轮廓里（作为一段半圆弧），所以"圆头"不会成为第三个图元。
fn make_arrow(p1: Point, p2: Point, width: f32) -> StrokePoints {
    let dx = p2.x - p1.x;
    let dy = p2.y - p1.y;
    let length = (dx * dx + dy * dy).sqrt();

    if length <= MIN_ARROW_LENGTH {
        // 太短不画箭头，只画线
        return StrokePoints::Polyline {
            points: vec![p1, p2],
        };
    }

    let ux = dx / length;
    let uy = dy / length;
    let nx = -uy;
    let ny = ux;

    let half = (width * 0.5).max(0.5);
    // 头部尺寸：随线宽线性增长，两头夹逼（短箭头时按总长收缩，长箭头时不会无限大）。
    let head_len = (15.0_f32).max(width * 4.0).min(length * 0.45);
    let head_half = (10.0_f32).max(width * 3.0);

    // 沿轴 `along`、离轴 `across` 的点（`across` 正方向 = 法向 `n`）。
    let at = |along: f32, across: f32| Point {
        x: p1.x + ux * along + nx * across,
        y: p1.y + uy * along + ny * across,
    };

    let shaft_len = length - head_len; // 杆端面距尾端的距离

    let mut outline = Vec::with_capacity(ARROW_CAP_SEG + 7);
    // ① 尾左 → ② 杆端面左 → ③ 左翼 → ④ 尖端 → ⑤ 右翼 → ⑥ 杆端面右 → ⑦ 尾右
    outline.push(at(0.0, half));
    outline.push(at(shaft_len, half));
    outline.push(at(shaft_len, head_half));
    outline.push(p2);
    outline.push(at(shaft_len, -head_half));
    outline.push(at(shaft_len, -half));
    outline.push(at(0.0, -half));
    // ⑧ 尾部圆头：从尾右绕到尾左（凸向 −u 方向），补上中间 7 个点。
    //    角度从 `α − 90°` 递减到 `α − 270°`（= `α + 90°`），途中经过 `α − 180°` 正是
    //    最靠后的那个极点 `p1 − u·half`。
    let a_right = uy.atan2(ux) - std::f32::consts::FRAC_PI_2;
    for i in 1..ARROW_CAP_SEG {
        let ang = a_right - std::f32::consts::PI * (i as f32 / ARROW_CAP_SEG as f32);
        outline.push(Point {
            x: p1.x + half * ang.cos(),
            y: p1.y + half * ang.sin(),
        });
    }

    StrokePoints::Arrow { outline }
}

/// 序号贴纸的半径。
///
/// 跟当前画笔宽度挂钩：粗笔配大圈、细笔配小圈，视觉上不至于"字比圈大"。
pub fn stamp_radius(pen_width: u16) -> f32 {
    (pen_width as f32 * 2.0).clamp(18.0, 64.0)
}

/// 序号标签的字号。
///
/// 单字时取圆径的 1.15 倍（"填满圆"的观感），多位数按**宽度预算**收收，保证不出圈
/// （`0.62 em/位` 是数字的典型步进宽度）。
///
/// 抽成函数是为了让**贴纸本体**与**工具栏图标**共用同一条规则——此前两处各写一份常量，
/// 图标与实物悄悄不一致（工具栏的注释还写着"与新版序号贴纸严格一致"，实际已经漂了）。
pub fn stamp_label_size(radius: f32, digits: usize) -> f32 {
    let digits = digits.max(1) as f32;
    (radius * 1.15).min(1.5 * radius / (0.62 * digits))
}

/// 计算形状的包围盒。
pub fn shape_bbox(shape: &StrokePoints) -> Rect {
    match shape {
        StrokePoints::Polyline { points } => {
            if points.is_empty() {
                return Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 0.0,
                    h: 0.0,
                };
            }
            let mut min_x = f32::MAX;
            let mut min_y = f32::MAX;
            let mut max_x = f32::MIN;
            let mut max_y = f32::MIN;
            for p in points {
                min_x = min_x.min(p.x);
                min_y = min_y.min(p.y);
                max_x = max_x.max(p.x);
                max_y = max_y.max(p.y);
            }
            Rect {
                x: min_x,
                y: min_y,
                w: max_x - min_x,
                h: max_y - min_y,
            }
        }
        StrokePoints::Rect { rect } | StrokePoints::Ellipse { rect } => *rect,
        // 马赛克的包围盒就是那块补丁本身（位图尺寸与 rect 逐像素对应）。
        StrokePoints::Mosaic { rect, .. } => *rect,
        StrokePoints::Arrow { outline } => {
            // 轮廓已经把尾端圆头也算进去了，直接取其外接盒即精确解。
            if outline.is_empty() {
                return Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 0.0,
                    h: 0.0,
                };
            }
            let mut min_x = f32::MAX;
            let mut min_y = f32::MAX;
            let mut max_x = f32::MIN;
            let mut max_y = f32::MIN;
            for p in outline {
                min_x = min_x.min(p.x);
                min_y = min_y.min(p.y);
                max_x = max_x.max(p.x);
                max_y = max_y.max(p.y);
            }
            Rect {
                x: min_x,
                y: min_y,
                w: max_x - min_x,
                h: max_y - min_y,
            }
        }
        StrokePoints::Text {
            anchor,
            text,
            size_px,
        } => {
            // 按字符类别估宽（`len()` 是字节数，中文会算成三倍宽，见 `core::text_metrics`）。
            // `anchor` 是排版盒的**左上角**（与 `DrawCmd::Text` 的约定一致，见
            // `runtime::render` 的翻译），故下边直接加一个行高。
            //
            // ★ 此前这里写的是 `anchor.y - line_height`（按"锚点是基线"）。后果不只是
            //   包围盒偏一行：**文字的墨迹整个落在盒子外面**，橡皮按包围盒相交判定 →
            //   擦不掉文字；双击再编辑的高亮框也画在错误位置。实测（`examples/shape_probe.rs`
            //   的 text_anchor 场景）：锚点 y=470、字号 60 时墨迹在 y 486..545。
            //
            // ★★ 多行（2026-09-26 起输入框支持 Enter 换行）：按 `\n` 拆行，
            //   宽取各行最大、高 = 行数 × 行高。逐行拆而不是整串估——`\n` 本身
            //   会被按 0.55 em 计入宽度，把包围盒撑出一截。
            let lines: Vec<&str> = text.split('\n').collect();
            let w = lines
                .iter()
                .map(|l| crate::core::text_metrics::estimate_width(l, *size_px))
                .fold(0.0f32, f32::max);
            let h = lines.len() as f32 * crate::core::text_metrics::line_height(*size_px);
            Rect {
                x: anchor.x,
                y: anchor.y,
                w,
                h,
            }
        }
        StrokePoints::Stamp { center, radius, .. } => Rect {
            x: center.x - radius,
            y: center.y - radius,
            w: radius * 2.0,
            h: radius * 2.0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn make_line() {
        let s = make_shape(
            Point { x: 0.0, y: 0.0 },
            Point { x: 100.0, y: 50.0 },
            ShapeTool::Line,
            2.0,
        );
        match s {
            StrokePoints::Polyline { points } => {
                assert_eq!(points.len(), 2);
                assert_eq!(points[0].x, 0.0);
                assert_eq!(points[1].x, 100.0);
            }
            _ => panic!("expected Polyline"),
        }
    }

    #[test]
    fn make_rect() {
        let s = make_shape(
            Point { x: 10.0, y: 20.0 },
            Point { x: 50.0, y: 80.0 },
            ShapeTool::Rect,
            2.0,
        );
        match s {
            StrokePoints::Rect { rect } => {
                assert_eq!(rect.x, 10.0);
                assert_eq!(rect.y, 20.0);
                assert_eq!(rect.w, 40.0);
                assert_eq!(rect.h, 60.0);
            }
            _ => panic!("expected Rect"),
        }
    }

    #[test]
    fn make_arrow_short() {
        let s = make_shape(
            Point { x: 0.0, y: 0.0 },
            Point { x: 2.0, y: 0.0 },
            ShapeTool::Arrow,
            2.0,
        );
        // 太短，只画线
        assert!(matches!(s, StrokePoints::Polyline { .. }));
    }

    #[test]
    fn make_arrow_with_head() {
        let s = make_shape(
            Point { x: 0.0, y: 0.0 },
            Point { x: 100.0, y: 0.0 },
            ShapeTool::Arrow,
            4.0,
        );
        let StrokePoints::Arrow { outline } = &s else {
            panic!("expected Arrow");
        };
        // 前 7 个点是轮廓的角点，第 8 个起是尾端圆头。
        assert_eq!(outline.len(), ARROW_CAP_SEG + 6);
        assert_eq!(outline[0].x, 0.0); // 尾左
        assert_eq!(outline[3], Point { x: 100.0, y: 0.0 }); // 尖端恒在 p2
                                                            // 包围盒把圆头也算进去了（旧实现漏了那半个笔宽）。
        let b = shape_bbox(&s);
        assert!(b.x < 0.0, "尾端圆头应使包围盒左边界越过 p1，得到 {}", b.x);
        assert!((b.w - (100.0 - b.x)).abs() < 1e-3);
    }

    /// ★ 一体成型的关键不变式：**杆面与三角面的面积交集为空**。
    ///
    /// 交集不为空 ⇔ 半透明下交界处被混合两次 ⇒ 颜色更深（用户报告的现象）。
    /// 这条在 α=255 时看不出来，所以必须用几何锁住，不能靠肉眼。
    #[test]
    fn arrow_shaft_and_head_never_overlap() {
        for (width, length) in [
            (2.0_f32, 100.0_f32),
            (4.0, 300.0),
            (24.0, 500.0),
            (120.0, 600.0),
            (8.0, 20.0),
            (1.0, 60.0),
        ] {
            let s = make_shape(
                Point { x: 0.0, y: 0.0 },
                Point { x: length, y: 0.0 },
                ShapeTool::Arrow,
                width,
            );
            let StrokePoints::Arrow { outline } = &s else {
                panic!("expected Arrow（width={width} length={length}）");
            };
            // 轮廓顺序：0 尾左 / 1 杆端面左 / 2 左翼 / 3 尖端 / 4 右翼 / 5 杆端面右 / 6 尾右
            let shaft = [outline[0], outline[1], outline[5], outline[6]];
            let head = [outline[2], outline[3], outline[4]];
            let mut overlap = 0;
            let mut steps = 0;
            for i in 0..=120 {
                for j in 0..=120 {
                    let p = Point {
                        x: length * (i as f32 / 120.0) - length * 0.1,
                        y: (width.max(20.0) * 2.0) * (j as f32 / 120.0 - 1.1),
                    };
                    steps += 1;
                    if strictly_in_convex(p, &shaft) && strictly_in_convex(p, &head) {
                        overlap += 1;
                    }
                }
            }
            assert!(steps > 0);
            assert_eq!(
                overlap, 0,
                "width={width} length={length} 时杆与头有 {overlap} 个重叠采样点"
            );
        }
    }

    /// 轮廓必须是**简单多边形**（任意两条不相邻的边不相交）。
    ///
    /// 自交本身就是几何没想清楚信号；即使 nonzero 填充规则能把自交区域盖住，它也让
    /// "只混一次"的推理失效（绕数 2 的区域在某些后端会走另一条代码路径）。
    #[test]
    fn arrow_outline_is_simple() {
        for (width, length, angle) in [
            (2.0_f32, 100.0_f32, 0.0_f32),
            (24.0, 500.0, 0.7),
            (120.0, 600.0, 2.3),
            (8.0, 20.0, -1.1),
        ] {
            let (ux, uy) = (angle.cos(), angle.sin());
            let s = make_shape(
                Point { x: 0.0, y: 0.0 },
                Point {
                    x: ux * length,
                    y: uy * length,
                },
                ShapeTool::Arrow,
                width,
            );
            let StrokePoints::Arrow { outline } = &s else {
                panic!("expected Arrow");
            };
            let n = outline.len();
            for i in 0..n {
                for j in (i + 1)..n {
                    // 相邻边共享端点，跳过；首尾相邻也跳过。
                    if j == i + 1 || (i == 0 && j == n - 1) {
                        continue;
                    }
                    let (a0, a1) = (outline[i], outline[(i + 1) % n]);
                    let (b0, b1) = (outline[j], outline[(j + 1) % n]);
                    assert!(
                        !segments_cross(a0, a1, b0, b1),
                        "width={width} length={length} 角度={angle}：边 {i} 与边 {j} 相交"
                    );
                }
            }
        }
    }

    /// 头部尺寸仍随线宽线性增长并两头夹逼（历史回归：粗笔时"箭头是平的"）。
    #[test]
    fn arrow_head_size_follows_pen_width() {
        // 常规：下限 15px 生效。
        let s = make_shape(
            Point { x: 0.0, y: 0.0 },
            Point { x: 100.0, y: 0.0 },
            ShapeTool::Arrow,
            2.0,
        );
        let StrokePoints::Arrow { outline } = &s else {
            panic!("expected Arrow");
        };
        assert!((100.0 - outline[1].x - 15.0).abs() < 1e-3);
        // 短箭头：头部按总长收缩到 45%（不得到 15px 那样把头撑满整条箭）。
        let s = make_shape(
            Point { x: 0.0, y: 0.0 },
            Point { x: 20.0, y: 0.0 },
            ShapeTool::Arrow,
            2.0,
        );
        let StrokePoints::Arrow { outline } = &s else {
            panic!("expected Arrow");
        };
        assert!((20.0 - outline[1].x - 9.0).abs() < 1e-3);
        // 粗笔：头部半宽 = 3×线宽，恒大于杆半宽（否则多边形会自交、翼点缩进杆内）。
        let s = make_shape(
            Point { x: 0.0, y: 0.0 },
            Point { x: 600.0, y: 0.0 },
            ShapeTool::Arrow,
            120.0,
        );
        let StrokePoints::Arrow { outline } = &s else {
            panic!("expected Arrow");
        };
        assert!((outline[2].y - 360.0).abs() < 1e-3, "头半宽应为 3×120");
        assert!(outline[2].y > outline[1].y, "翼点必须在杆外侧");
    }

    // ── 几何判定小工具（只给测试用） ──

    /// 严格落在**凸**多边形内部（落在边上或边外都不算）。
    fn strictly_in_convex(p: Point, poly: &[Point]) -> bool {
        const EPS: f32 = 1e-3;
        let n = poly.len();
        let mut pos = 0;
        let mut neg = 0;
        for i in 0..n {
            let a = poly[i];
            let b = poly[(i + 1) % n];
            let cross = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
            if cross > EPS {
                pos += 1;
            } else if cross < -EPS {
                neg += 1;
            }
        }
        pos == n || neg == n
    }

    /// 两条线段是否真正相交。
    ///
    /// 必须**容忍退化共线**：箭头轮廓里天然存在一对共线但不相交的边（杆端面把三角底边
    /// 切成两段，左右两段落在同一条直线上、中间隔着杆宽的空隙）。纯符号判据在这里
    /// 会因 f32 舍入把 `d1/d2` 算成异号 → 误报相交（实测在 width=120/length=600 上炸过）。
    fn segments_cross(a0: Point, a1: Point, b0: Point, b1: Point) -> bool {
        let cross =
            |o: Point, a: Point, b: Point| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
        let d1 = cross(b0, b1, a0);
        let d2 = cross(b0, b1, a1);
        let d3 = cross(a0, a1, b0);
        let d4 = cross(a0, a1, b1);
        // 叉积的量纲是长度²，故容差取"最大边长²×1e-4"。
        let span = |p: Point, q: Point| ((q.x - p.x).powi(2) + (q.y - p.y).powi(2)).sqrt();
        let eps = 1e-4 * span(a0, a1).max(span(b0, b1)).max(1.0).powi(2);
        if d1.abs() <= eps && d2.abs() <= eps && d3.abs() <= eps && d4.abs() <= eps {
            // 共线：退化成"两段在一维上的投影是否重叠"。
            let axis = Point {
                x: a1.x - a0.x,
                y: a1.y - a0.y,
            };
            let len = span(a0, a1).max(1e-6);
            let proj = |p: Point| (p.x - a0.x) * axis.x / len + (p.y - a0.y) * axis.y / len;
            let (a_min, a_max) = {
                let (p, q) = (proj(a0), proj(a1));
                (p.min(q), p.max(q))
            };
            let (b_min, b_max) = {
                let (p, q) = (proj(b0), proj(b1));
                (p.min(q), p.max(q))
            };
            return a_min < b_max - 1e-3 && b_min < a_max - 1e-3;
        }
        ((d1 > eps) != (d2 > eps)) && ((d3 > eps) != (d4 > eps))
    }

    #[test]
    fn make_ellipse() {
        let s = make_shape(
            Point { x: 10.0, y: 20.0 },
            Point { x: 50.0, y: 80.0 },
            ShapeTool::Ellipse,
            2.0,
        );
        assert!(matches!(s, StrokePoints::Ellipse { .. }));
    }

    #[test]
    fn stamp_bbox_is_a_square_around_the_center() {
        let s = StrokePoints::Stamp {
            center: Point { x: 100.0, y: 50.0 },
            radius: 20.0,
            label: 1,
        };
        let b = shape_bbox(&s);
        assert_eq!(b.x, 80.0);
        assert_eq!(b.y, 30.0);
        assert_eq!(b.w, 40.0);
        assert_eq!(b.h, 40.0);
    }

    #[test]
    fn text_bbox_width_is_character_aware() {
        // 两个汉字 ≈ 2 em；按字节算会得到 6 em，这条锁住那个三倍宽的旧 bug。
        let s = StrokePoints::Text {
            anchor: Point { x: 0.0, y: 100.0 },
            text: "步骤".into(),
            size_px: 40.0,
        };
        let b = shape_bbox(&s);
        assert!((b.w - 80.0).abs() < 1.0, "两个汉字应约 80px，得到 {}", b.w);
        // ★ 锚点是排版盒**左上角**（与 `DrawCmd::Text` 的约定一致），故包围盒从锚点起、
        //   向下取一行高。此前这里写的是"向上取一行高"（按基线），后果是墨迹整个落在
        //   包围盒之外——橡皮擦不掉文字、再编辑高亮框画在错处。
        assert_eq!(b.x, 0.0);
        assert_eq!(b.y, 100.0, "包围盒上边就是锚点，不再向上偏移一行");
        assert!(b.h >= 40.0, "至少一行高，得到 {}", b.h);
    }

    #[test]
    fn text_bbox_covers_every_line_of_a_multiline_entry() {
        // ★ 多行（2026-09-26 输入框支持 Enter 换行）：宽 = 最宽行，高 = 行数 × 行高。
        //   第一行 2 个汉字 ≈ 80px，第二行 8 个汉字 ≈ 320px → 取 320；
        //   高度按行数算，不再恒为一行（否则第二行墨迹落在包围盒外，
        //   橡皮按包围盒相交判定就擦不掉第二行）。
        let s = StrokePoints::Text {
            anchor: Point { x: 0.0, y: 100.0 },
            text: "步骤\n步骤步骤步骤步骤".into(),
            size_px: 40.0,
        };
        let b = shape_bbox(&s);
        let lh = crate::core::text_metrics::line_height(40.0);
        assert!(
            (b.w - 320.0).abs() < 1.0,
            "宽应取最宽行（8 汉字 ≈320px），得到 {}",
            b.w
        );
        assert!((b.h - 2.0 * lh).abs() < 1.0, "高应为两行行高，得到 {}", b.h);
        // `\n` 本身不许再按 0.55 em 计入宽度（整串估会把宽撑出一截）。
        let single = shape_bbox(&StrokePoints::Text {
            anchor: Point { x: 0.0, y: 0.0 },
            text: "步骤".into(),
            size_px: 40.0,
        });
        assert!(
            b.w > single.w,
            "两行（含更宽的第二行）应比单行宽，否则换行符被计入了宽度"
        );
    }

    #[test]
    fn stamp_radius_follows_pen_width_within_bounds() {
        assert_eq!(stamp_radius(1), 18.0);
        assert_eq!(stamp_radius(10), 20.0);
        assert_eq!(stamp_radius(500), 64.0);
    }
}
