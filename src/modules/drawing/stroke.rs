//! 自由笔迹：采样、去抖、路径缓存。

use super::pens::PenStyle;
use crate::core::color::Argb;
use crate::core::geometry::{Point, Rect};

/// 去抖阈值（逻辑像素）。1000Hz 鼠标静止时 ±1px 的抖动会被过滤。
pub const MIN_DIST: f32 = 1.0;

/// 单个笔迹的顶点数上限。超过时按间隔抽稀。
pub const MAX_LIVE_POINTS: usize = 8_000;

/// 已提交的笔迹几何。
#[derive(Debug, Clone)]
pub enum StrokePoints {
    /// 自由笔迹 / 直线 / 折线
    Polyline { points: Vec<Point> },
    /// 矩形（存规范化后的 rect）
    Rect { rect: Rect },
    /// 椭圆（存外接矩形）
    Ellipse { rect: Rect },
    /// 箭头：**一条闭合多边形**（杆 + 三角头 + 圆尾一体成型）。
    ///
    /// ★ 存的是**轮廓**而不是"杆 + 头"两个图元。此前分开存、分开画（描一条线 + 填一个
    /// 三角），半透明下两者在交界处各混一次 → 那道缝明显更深（实测最暗 64 vs 单次叠加的
    /// 127，见 `examples/shape_probe.rs`）。轮廓一次填充，无论 α 多少都只混一次。
    ///
    /// 顶点顺序（闭合，同一绕行方向）：
    /// 尾左 → 杆端面左 → 三角底边左翼 → 尖端 → 三角底边右翼 → 杆端面右 → 尾右 → 圆头弧。
    /// 其中**杆端面与三角底边共面**，两块面积交集为零。
    Arrow { outline: Vec<Point> },
    /// 文字
    Text {
        anchor: Point,
        text: String,
        size_px: f32,
    },
    /// 序号贴纸：实心圆 + 反色居中数字（教学讲步骤用）。
    ///
    /// 编号由 `DrawingController` 递增给出，落在 history 里就是普通笔迹——可撤销、
    /// 可被橡皮擦掉，不需要单独的"贴纸列表"。
    Stamp {
        center: Point,
        radius: f32,
        label: u32,
    },
    /// 马赛克：把底下那块画面打码后作为**位图补丁**贴回原位。
    ///
    /// 像素在控制器提交时从冻结帧（[`crate::core::backdrop::Backdrop`]）里取出来按块
    /// 平均算好，存成 `Rc<RgbaBitmap>` —— 之后每帧只是把这张小图贴回 `rect`，与屏幕
    /// 标注 / 截图编辑两条渲染路（画布重放同一份 DrawList）天然同构：预览即所得，
    /// 撤销 = 从历史里摘掉这条，橡皮也能按包围盒擦掉它。
    ///
    /// 屏幕坐标是**世界物理像素**；补丁位图尺寸 = rect 的像素尺寸。
    Mosaic {
        rect: Rect,
        px: std::rc::Rc<crate::core::image::RgbaBitmap>,
    },
}

impl StrokePoints {
    /// 返回该笔迹的顶点数。
    pub fn point_count(&self) -> usize {
        match self {
            StrokePoints::Polyline { points } => points.len(),
            StrokePoints::Rect { .. } => 4,
            StrokePoints::Ellipse { .. } => 4,
            StrokePoints::Arrow { outline } => outline.len(),
            StrokePoints::Text { .. } => 1,
            // 一个圆按 24 段估：它进出撤销栈的成本与同尺寸的自由笔迹同量级。
            StrokePoints::Stamp { .. } => 24,
            // 马赛克按"一个矩形 4 顶点"估：真正的大头是补丁位图，不进点数预算
            // （MAX_POINTS 管的是路径采样，位图成本由 MAX_STROKES 的条数上限兜住）。
            StrokePoints::Mosaic { .. } => 4,
        }
    }
}

/// 一条正在绘制中的自由笔迹。
pub struct LiveStroke {
    points: Vec<Point>,
    bbox: Rect,
}

impl LiveStroke {
    /// 创建一条新的 LiveStroke。
    pub fn begin(p: Point) -> Self {
        Self {
            points: vec![p],
            bbox: Rect {
                x: p.x,
                y: p.y,
                w: 0.0,
                h: 0.0,
            },
        }
    }

    /// 添加一个点。若距离上一点 < MIN_DIST 则不动并返回 false（去抖）。
    pub fn extend(&mut self, p: Point) -> bool {
        if let Some(&last) = self.points.last() {
            let dx = p.x - last.x;
            let dy = p.y - last.y;
            if (dx * dx + dy * dy) < (MIN_DIST * MIN_DIST) {
                return false;
            }
        }
        self.points.push(p);
        // 更新包围盒
        if p.x < self.bbox.x {
            self.bbox.w += self.bbox.x - p.x;
            self.bbox.x = p.x;
        } else if p.x > self.bbox.x + self.bbox.w {
            self.bbox.w = p.x - self.bbox.x;
        }
        if p.y < self.bbox.y {
            self.bbox.h += self.bbox.y - p.y;
            self.bbox.y = p.y;
        } else if p.y > self.bbox.y + self.bbox.h {
            self.bbox.h = p.y - self.bbox.y;
        }
        true
    }

    /// 返回点的切片。
    pub fn points(&self) -> &[Point] {
        &self.points
    }

    /// 返回包围盒。
    pub fn bbox(&self) -> Rect {
        self.bbox
    }

    /// 提交前收尾：①若只有 1 个点，补一个相距 0.01 的点；②抽稀超过上限的中间点。
    pub fn finish(mut self) -> StrokePoints {
        // 单点补点
        if self.points.len() == 1 {
            let p = self.points[0];
            self.points.push(Point {
                x: p.x + 0.01,
                y: p.y,
            });
        }
        // 抽稀
        while self.points.len() > MAX_LIVE_POINTS {
            self.thin();
        }
        StrokePoints::Polyline {
            points: self.points,
        }
    }

    /// 间隔抽稀：保留奇数索引的点。
    fn thin(&mut self) {
        let mut kept = Vec::with_capacity(self.points.len().div_ceil(2));
        for (i, p) in self.points.iter().enumerate() {
            if i % 2 == 1 {
                kept.push(*p);
            }
        }
        // 重新计算 bbox
        if let Some(&first) = kept.first() {
            let mut bbox = Rect {
                x: first.x,
                y: first.y,
                w: 0.0,
                h: 0.0,
            };
            for &p in &kept[1..] {
                if p.x < bbox.x {
                    bbox.w += bbox.x - p.x;
                    bbox.x = p.x;
                } else if p.x > bbox.x + bbox.w {
                    bbox.w = p.x - bbox.x;
                }
                if p.y < bbox.y {
                    bbox.h += bbox.y - p.y;
                    bbox.y = p.y;
                } else if p.y > bbox.y + bbox.h {
                    bbox.h = p.y - bbox.y;
                }
            }
            self.points = kept;
            self.bbox = bbox;
        }
    }
}

/// 笔迹历史条目。
#[derive(Debug, Clone)]
pub struct StrokeEntry {
    pub points: StrokePoints,
    pub color: Argb,
    pub width: f32,
    pub style: PenStyle,
    /// true → 填充；false → 只描边。
    pub filled: bool,
    /// 该笔迹的包围盒（建立时算一次并缓存）。
    pub bbox: Rect,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_stroke_begin_and_extend() {
        let mut s = LiveStroke::begin(Point { x: 10.0, y: 20.0 });
        assert_eq!(s.points().len(), 1);
        assert!(!s.extend(Point { x: 10.0, y: 20.0 })); // 同一点，去抖
        assert!(s.extend(Point { x: 15.0, y: 25.0 })); // 距离足够
        assert_eq!(s.points().len(), 2);
    }

    #[test]
    fn live_stroke_finish_single_point() {
        let s = LiveStroke::begin(Point { x: 5.0, y: 5.0 });
        let sp = s.finish();
        match sp {
            StrokePoints::Polyline { points } => assert_eq!(points.len(), 2),
            _ => panic!("expected Polyline"),
        }
    }

    #[test]
    fn live_stroke_bbox() {
        let mut s = LiveStroke::begin(Point { x: 10.0, y: 20.0 });
        s.extend(Point { x: 30.0, y: 50.0 });
        let b = s.bbox();
        assert_eq!(b.x, 10.0);
        assert_eq!(b.y, 20.0);
        assert_eq!(b.w, 20.0);
        assert_eq!(b.h, 30.0);
    }
}
