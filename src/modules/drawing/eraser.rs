//! 橡皮：拖动出待擦区域 + 拖动预览。
//!
//! **擦除语义与 Python 1.0.0 完全一致**：把这一趟拖动轨迹的**包围盒**交给
//! `History::erase_by_rect`，凡是包围盒与之相交的整条笔迹一律删除。
//!
//! 这不是"沿轨迹精确挖掉一段"，而是一趟扫过去整条整条地抹掉——看似粗暴，但它是原版
//! 行为，且对"擦掉刚才写错的那几个字"这种用法反而更好用（不会留下半截笔画的毛刺）。
//!
//! 早先这里把相交判定放在橡皮自己身上（`finish(&strokes) -> Vec<usize>`），算完没人用——
//! `controller` 拿到下标后什么都没做，于是"擦了等于没擦"。现在判定统一归 `History`，
//! 橡皮只负责产出**区域**。

use super::stroke::LiveStroke;
use crate::core::draw::DrawList;
use crate::core::geometry::{Point, Rect};

/// 橡皮预览的最小宽度（逻辑像素）。画笔再细，橡皮也得看得见。
const MIN_ERASER_WIDTH: f32 = 20.0;

/// 橡皮。
pub struct Eraser {
    live: Option<LiveStroke>,
    preview_width: f32,
}

impl Default for Eraser {
    fn default() -> Self {
        Self::new()
    }
}

impl Eraser {
    pub fn new() -> Self {
        Self {
            live: None,
            preview_width: MIN_ERASER_WIDTH,
        }
    }

    /// 开始拖动。
    pub fn begin(&mut self, p: Point, pen_width: u16) {
        self.preview_width = (pen_width as f32).max(MIN_ERASER_WIDTH);
        self.live = Some(LiveStroke::begin(p));
    }

    /// 拖动中。
    pub fn extend(&mut self, p: Point) {
        if let Some(ref mut live) = self.live {
            live.extend(p);
        }
    }

    /// 结束拖动：返回这一趟的包围盒（交给 `History::erase_by_rect`），并清掉预览。
    ///
    /// 没在拖动时返回 `None`——调用方据此跳过这次擦除，不该凭空擦一块。
    pub fn end(&mut self) -> Option<Rect> {
        let live = self.live.take()?;
        Some(live.bbox())
    }

    /// 放弃本次拖动（退出标注模式 / 清空笔迹时调用），不产生擦除。
    pub fn cancel(&mut self) {
        self.live = None;
    }

    /// 预览绘制：跟随鼠标的一条粗线。
    ///
    /// 颜色按模式取反（白板用白、屏幕上用深灰），因为它是"把内容抹掉"的示意，
    /// 不该像笔迹那样有个颜色倾向。
    pub fn draw_preview(&self, list: &mut DrawList, whiteboard: bool) {
        if let Some(ref live) = self.live {
            let color = if whiteboard {
                crate::core::color::Argb::new(0xFF, 0xFF, 0xFF, 0xFF)
            } else {
                crate::core::color::Argb::new(0xFF, 0x28, 0x28, 0x28)
            };
            let points: Vec<Point> = live.points().to_vec();
            list.draw_polyline(points, self.preview_width, color);
        }
    }

    /// 是否正在拖动。
    pub fn is_active(&self) -> bool {
        self.live.is_some()
    }

    /// 预览宽度。
    pub fn preview_width(&self) -> f32 {
        self.preview_width
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_without_begin_is_none() {
        let mut e = Eraser::new();
        assert!(e.end().is_none());
        assert!(!e.is_active());
    }

    #[test]
    fn end_returns_the_drag_bbox_and_clears_the_preview() {
        let mut e = Eraser::new();
        e.begin(Point { x: 10.0, y: 10.0 }, 4);
        e.extend(Point { x: 30.0, y: 40.0 });
        assert!(e.is_active());

        let bbox = e.end().expect("拖动过就该有包围盒");
        assert_eq!(bbox.x, 10.0);
        assert_eq!(bbox.y, 10.0);
        assert_eq!(bbox.w, 20.0);
        assert_eq!(bbox.h, 30.0);
        // 结束即清预览，否则松手后那条灰线会一直留在屏上
        assert!(!e.is_active());
        assert!(e.end().is_none(), "第二次 end 不该再产出区域");
    }

    #[test]
    fn cancel_drops_the_drag_without_erasing() {
        let mut e = Eraser::new();
        e.begin(Point { x: 1.0, y: 1.0 }, 4);
        e.cancel();
        assert!(!e.is_active());
        assert!(e.end().is_none());
    }

    #[test]
    fn preview_width_has_a_floor() {
        let mut e = Eraser::new();
        e.begin(Point { x: 0.0, y: 0.0 }, 2);
        assert_eq!(e.preview_width(), MIN_ERASER_WIDTH);
        e.begin(Point { x: 0.0, y: 0.0 }, 60);
        assert_eq!(e.preview_width(), 60.0);
    }
}
