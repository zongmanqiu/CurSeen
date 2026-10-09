//! 线型的几何展开：把一条折线按「划 / 空」周期切成若干段独立折线。
//!
//! # 为什么在算法层做，而不是给 `DrawCmd` 加个 dash 字段
//!
//! Qt 的 `QPen` 自带 dash pattern，Python 1.0.0 的 `_draw_path` 直接
//! `pen.setStyle(Qt.DashLine)` 就完事。CurSeen 2.0 没有 QPen——渲染契约是
//! [`DrawCmd`](crate::core::draw::DrawCmd)，它只描述图元，`Stroke` 上的 `cap`/`join`
//! 至今都没被翻译层读过（`runtime::render` 里是 `cap: _, join: _`）。
//!
//! 给 `Stroke` 加 `dash` 字段意味着：改跨模块契约 + 改翻译层 + 给 windui 打一个新补丁
//! 去接 `tiny_skia::StrokeDash`，而这条链路上还挂着拖尾、击键回显、Lua 脚本三条无关的
//! 消费方——为了一个"画笔看起来是虚线"的功能去动它们，代价与收益不成比例。
//!
//! 折线切段则是**纯几何**：没有新契约、没有新渲染能力、没有新补丁，且是可单测的纯函数。
//! 代价是虚线笔迹从 1 条指令变成 N 条，实测 4px 笔宽横贯 1080p 约 90 段——对
//! `DrawList`（一个 `Vec`）与 CPU 软渲染都是噪声级别，而线段数量与"用户画了多少"成正比，
//! 不会在空闲时产生任何成本。
//!
//! # 周期口径
//!
//! 与 Qt 一致：**pattern 的单位是线宽**（Qt 文档原文 "specified in units of the pen's
//! width"）。所以 4px 的「虚线」是 16px 划 / 8px 空；把笔调粗，虚线段同比变长，
//! 视觉节奏保持不变。这也和 Qt 的预定义向量逐项对齐（见 [`pattern`]）。

use crate::core::geometry::Point;

use super::pens::PenStyle;

/// 长度比较的下限。低于它视为零长——鼠标采样点常常重复（同一像素收到两次 move），
/// 不设下限会在"退化线段"上反复做无用功，也会让 `remain` 永远减不到 0。
const EPS: f32 = 1e-3;

/// 线宽下限。线宽为 0 时 pattern 全长也是 0，周期退化成 `remain <= EPS` 恒真的死循环。
const MIN_WIDTH: f32 = 0.5;

/// 一个线型的「划 / 空」周期，单位是**线宽**（与 Qt 的预定义 dash pattern 同口径）。
///
/// 空切片 = 实线。数组长度恒为偶数：奇数下标是「划」，偶数下标是「空」。
fn pattern(style: PenStyle) -> &'static [f32] {
    match style {
        PenStyle::Solid => &[],
        // Qt::DashLine
        PenStyle::Dashed => &[4.0, 2.0],
        // Qt::DotLine
        PenStyle::Dotted => &[1.0, 2.0],
        // Qt::DashDotLine
        PenStyle::DashDot => &[4.0, 2.0, 1.0, 2.0],
    }
}

/// 按线型把折线切成若干段独立折线；段与段之间的空隙就是虚线缺口。
///
/// 实线返回**原样一段**——调用方不必为实线另开分支。切不出任何一段时同样退回原样一段
/// （见函数末尾），不会撞上"画了但屏上什么都没有"。
///
/// `width` 是线宽（世界单位，与 `points` 同单位）：pattern 按它同比放大。
///
/// 少于 2 个点返回**空**：折线至少要两点才画得出一条线（`Canvas::draw_polyline` 对
/// 短于两点的输入直接返回），所以这里没有"一段单点折线"这种产物。
pub fn segments(points: &[Point], style: PenStyle, width: f32) -> Vec<Vec<Point>> {
    if points.len() < 2 {
        return Vec::new();
    }
    let units = pattern(style);
    if units.is_empty() {
        return vec![points.to_vec()];
    }

    let w = if width.is_finite() && width > MIN_WIDTH {
        width
    } else {
        MIN_WIDTH
    };
    let pat: Vec<f32> = units.iter().map(|m| m * w).collect();

    let mut out: Vec<Vec<Point>> = Vec::new();
    // 当前正在攒的这一段"划"。以"空"开头时它是空的。
    let mut cur: Vec<Point> = Vec::new();

    let mut pi = 0usize;
    let mut remain = pat[0];
    let mut on = true;
    let mut prev = points[0];
    cur.push(prev);

    for &p in &points[1..] {
        let span = prev.distance_to(p);
        if span <= EPS {
            prev = p;
            continue;
        }

        // 沿 prev→p 走完这一条线段，路上可能跨越多个"划/空"边界。
        let mut walked = 0.0f32;
        while walked < span - EPS {
            let take = remain.min(span - walked);
            let t0 = walked / span;
            let t1 = (walked + take) / span;
            let a = lerp(prev, p, t0);
            let b = lerp(prev, p, t1);

            if on {
                if cur.is_empty() {
                    cur.push(a);
                }
                cur.push(b);
            }

            walked += take;
            remain -= take;

            if remain <= EPS {
                // 跨过一个边界。
                if on {
                    // 划 → 空：这一段落笔到此为止。零长/单点段丢弃（画不出来）。
                    if cur.len() >= 2 {
                        out.push(std::mem::take(&mut cur));
                    } else {
                        cur.clear();
                    }
                }
                on = !on;
                pi = (pi + 1) % pat.len();
                remain = pat[pi];
                if on {
                    // 空 → 划：从刚走过的那个点重新起笔。
                    cur.push(b);
                }
            }
        }
        prev = p;
    }

    if cur.len() >= 2 {
        out.push(cur);
    }
    // 切不出任何一段（整条笔迹比一个"空"周期还短、或起手就被空档吃掉）时退回实线：
    // 用户按下并拖动，期待的是"看见一笔"，而不是"画了但屏上什么都没有"。
    if out.is_empty() {
        out.push(points.to_vec());
    }
    out
}

/// 线性插值。`t` 已由调用方归一化到 0..1，不做额外钳位。
fn lerp(a: Point, b: Point, t: f32) -> Point {
    Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一条水平直线，从 (0,0) 到 (len,0)，用 1px 线宽（pattern 数值即像素）。
    fn line(len: f32) -> Vec<Point> {
        vec![Point::new(0.0, 0.0), Point::new(len, 0.0)]
    }

    /// 实线原样一段——调用方不该为实线走另一条路径。
    #[test]
    fn solid_stays_one_segment() {
        let pts = line(100.0);
        let segs = segments(&pts, PenStyle::Solid, 4.0);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0], pts);
    }

    /// 虚线的「划」总长与周期数对得上（4 划 / 2 空，1px 笔宽 → 6px 一个周期）。
    ///
    /// 100px 上：从 0 起笔，划 4、空 2、…… 第 16 个周期整好落在 96，再划 4 到 100。
    /// 所以共 17 段，且第一段是 [0,4]、第二段是 [6,10]。
    #[test]
    fn dashed_splits_into_even_segments() {
        let segs = segments(&line(100.0), PenStyle::Dashed, 1.0);
        assert_eq!(segs.len(), 17, "100px / 6px 周期 + 收尾一段");
        for s in &segs {
            assert_eq!(s.len(), 2, "直线上的每一段都应是两点");
            let w = s[1].x - s[0].x;
            assert!((w - 4.0).abs() < 1e-2, "每段长应为 4，实际 {w}");
        }
        assert!((segs[0][0].x - 0.0).abs() < 1e-3 && (segs[0][1].x - 4.0).abs() < 1e-3);
        assert!((segs[1][0].x - 6.0).abs() < 1e-3 && (segs[1][1].x - 10.0).abs() < 1e-3);
        // 段与段之间留出 2px 缺口
        for i in 1..segs.len() {
            let gap = segs[i][0].x - segs[i - 1][1].x;
            assert!(gap > 1.0, "相邻两段之间应有缺口，实际 {gap}");
        }
    }

    /// pattern 的单位是线宽：笔宽翻倍，划长也翻倍（Qt 同口径）。
    #[test]
    fn pattern_scales_with_pen_width() {
        let thin = segments(&line(100.0), PenStyle::Dashed, 1.0);
        let thick = segments(&line(100.0), PenStyle::Dashed, 4.0);
        let first = |v: &Vec<Vec<Point>>| v[0][1].x - v[0][0].x;
        assert!((first(&thin) - 4.0).abs() < 1e-2);
        assert!(
            (first(&thick) - 16.0).abs() < 1e-2,
            "4px 笔宽应得到 16px 划"
        );
    }

    /// 点线：划段短到近似一个圆点（1×线宽），缺口是它的两倍。
    #[test]
    fn dotted_makes_short_dabs() {
        let segs = segments(&line(60.0), PenStyle::Dotted, 4.0);
        assert!(segs.len() >= 4, "点线应有多个点，实际 {}", segs.len());
        for s in &segs {
            let w = s[1].x - s[0].x;
            assert!((w - 4.0).abs() < 1e-2, "点长应为 1×线宽=4，实际 {w}");
        }
        let gap = segs[1][0].x - segs[0][1].x;
        assert!(gap > 3.0, "点线的缺口应更明显，实际 {gap}");
    }

    /// 点划线是一个 4 段周期（4 划 / 2 空 / 1 划 / 2 空），与虚线 distinguishable。
    #[test]
    fn dash_dot_alternates_long_and_short() {
        let segs = segments(&line(200.0), PenStyle::DashDot, 1.0);
        let widths: Vec<f32> = segs.iter().map(|s| s[1].x - s[0].x).collect();
        assert!(widths.len() >= 6, "应产出多个段，实际 {}", widths.len());
        // 前四段应是 长/短/长/短 的节奏
        assert!((widths[0] - 4.0).abs() < 1e-2, "第 1 段是长划");
        assert!((widths[1] - 1.0).abs() < 1e-2, "第 2 段是点");
        assert!((widths[2] - 4.0).abs() < 1e-2, "第 3 段是长划");
        assert!((widths[3] - 1.0).abs() < 1e-2, "第 4 段是点");
    }

    /// 折线上每段内部仍连续（顶点落在原折线上），且不超出原折线端点范围。
    #[test]
    fn segments_stay_on_the_original_polyline() {
        // 一条折线：(0,0)→(50,0)→(50,50)
        let pts = vec![
            Point::new(0.0, 0.0),
            Point::new(50.0, 0.0),
            Point::new(50.0, 50.0),
        ];
        let segs = segments(&pts, PenStyle::Dashed, 1.0);
        for s in &segs {
            for p in s {
                let on_h = (p.y).abs() < 1e-2 && (-0.01..=50.01).contains(&p.x);
                let on_v = (p.x - 50.0).abs() < 1e-2 && (-0.01..=50.01).contains(&p.y);
                assert!(on_h || on_v, "顶点 {p:?} 偏离了原折线");
            }
        }
        // 拐点处也要有墨：虚线不该在拐角处凭空断掉半截
        let has_after_corner = segs
            .iter()
            .flatten()
            .any(|p| (p.x - 50.0).abs() < 1e-2 && p.y > 10.0);
        assert!(has_after_corner, "拐点之后的竖段上也应有划段");
    }

    /// 比一个周期还短的笔迹退化为实线——"画了但看不见"是最糟的结果。
    #[test]
    fn very_short_stroke_falls_back_to_solid() {
        let segs = segments(&line(2.0), PenStyle::Dotted, 4.0);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].len(), 2, "应原样一段，而不是被空档吃掉");
    }

    /// 退化输入不得 panic，也不得死循环。
    ///
    /// 三个坑：零线宽会让 pattern 全长为 0；NaN 线宽会让 `remain <= EPS` 恒假；
    /// 重复采样点让线段长度为 0。
    #[test]
    fn degenerate_inputs_do_not_hang_or_panic() {
        // 零线宽
        assert!(!segments(&line(50.0), PenStyle::Dashed, 0.0).is_empty());
        // NaN 线宽
        assert!(!segments(&line(50.0), PenStyle::Dashed, f32::NAN).is_empty());
        // 负线宽
        assert!(!segments(&line(50.0), PenStyle::Dotted, -3.0).is_empty());
        // 全是同一个点：切不出划段 → 退回原样一段（交给渲染层，它画不出东西也不亏）
        let same = vec![Point::new(7.0, 7.0); 5];
        assert_eq!(segments(&same, PenStyle::Dashed, 4.0).len(), 1);
        // 少于两点：折线画不出来，返回空而不是"一段单点折线"
        assert!(segments(&[Point::new(1.0, 2.0)], PenStyle::Dashed, 4.0).is_empty());
        assert!(segments(&[], PenStyle::Dashed, 4.0).is_empty());
    }
}
