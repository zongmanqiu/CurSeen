//! 撤销/重做/清空栈（含容量与内存上限）。

use super::stroke::StrokeEntry;
use crate::core::geometry::Rect;

/// 容量上限：笔迹数。
pub const MAX_STROKES: usize = 2_000;

/// 容量上限：顶点总数（约 32MB）。
pub const MAX_POINTS: usize = 4_000_000;

/// 笔迹历史。
pub struct History {
    strokes: Vec<StrokeEntry>,
    redo_stack: Vec<StrokeEntry>,
    point_count: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl History {
    pub fn new() -> Self {
        Self {
            strokes: Vec::new(),
            redo_stack: Vec::new(),
            point_count: 0,
        }
    }

    /// 新增一条。清空 redo_stack。
    pub fn add(&mut self, entry: StrokeEntry) {
        self.point_count += entry.points.point_count();
        self.redo_stack.clear();
        self.strokes.push(entry);
        self.evict_if_needed();
    }

    /// 撤销一条 → 压入 redo_stack。返回被撤销笔迹的 bbox。
    pub fn undo(&mut self) -> Option<Rect> {
        let entry = self.strokes.pop()?;
        let bbox = entry.bbox;
        self.point_count = self.point_count.saturating_sub(entry.points.point_count());
        self.redo_stack.push(entry);
        Some(bbox)
    }

    /// 重做一条。返回 bbox。
    pub fn redo(&mut self) -> Option<Rect> {
        let entry = self.redo_stack.pop()?;
        let bbox = entry.bbox;
        self.point_count += entry.points.point_count();
        self.strokes.push(entry);
        Some(bbox)
    }

    /// 按矩形批量删除（橡皮）。返回被删笔迹的 bbox 并集。
    pub fn erase_by_rect(&mut self, r: &Rect) -> Option<Rect> {
        let mut dirty = Rect {
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
        };
        let mut found = false;
        let mut i = 0;
        while i < self.strokes.len() {
            if self.strokes[i].bbox.intersects(*r) {
                let entry = self.strokes.remove(i);
                self.point_count = self.point_count.saturating_sub(entry.points.point_count());
                if !found {
                    dirty = entry.bbox;
                    found = true;
                } else {
                    dirty = dirty.union(entry.bbox);
                }
            } else {
                i += 1;
            }
        }
        if found {
            Some(dirty)
        } else {
            None
        }
    }

    /// 全部清空。同时清空 redo_stack。
    pub fn clear(&mut self) -> Option<Rect> {
        if self.strokes.is_empty() && self.redo_stack.is_empty() {
            return None;
        }
        // 返回整屏矩形（清空要脏整屏）
        let dirty = Rect {
            x: 0.0,
            y: 0.0,
            w: f32::MAX,
            h: f32::MAX,
        };
        self.strokes.clear();
        self.redo_stack.clear();
        self.point_count = 0;
        Some(dirty)
    }

    /// ★ 把两个栈的**容量**也退掉。
    ///
    /// [`clear`](Self::clear) 只把长度归零、`capacity` 留着（理由是"马上又要画"）。
    /// 屏幕书写被整个关掉时没有那个"马上"——撤销栈按落笔数线性增长，长时间书写后
    /// 攒下的容量可能相当可观，而用户把开关拨到关就是想让它消失。
    pub fn shrink_to_fit(&mut self) {
        self.strokes.shrink_to_fit();
        self.redo_stack.shrink_to_fit();
    }

    /// 两个栈当前的**容量**（`(strokes, redo_stack)`）。
    ///
    /// 只为测试而存在：`Vec::clear` 与 `shrink_to_fit` 的差别在长度上看不出来
    /// （两者之后 `len` 都是 0），只在容量上——想断言"内存真的退回去了"就得读它。
    #[doc(hidden)]
    pub fn capacity_pair(&self) -> (usize, usize) {
        (self.strokes.capacity(), self.redo_stack.capacity())
    }

    /// 获取所有笔迹。
    pub fn strokes(&self) -> &[StrokeEntry] {
        &self.strokes
    }

    /// 可变访问第 `idx` 条笔迹（★ 文字拖动 / 再编辑用）。
    ///
    /// 下标 = [`Self::strokes`] 里的位置。只在交互当下即时使用（点中 → 拖动/改字），
    /// 不要跨手势长期持有：橡皮删除会令下标整体移位。
    pub fn entry_mut(&mut self, idx: usize) -> Option<&mut StrokeEntry> {
        self.strokes.get_mut(idx)
    }

    /// 是否可以撤销。
    pub fn can_undo(&self) -> bool {
        !self.strokes.is_empty()
    }

    /// 是否可以重做。
    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// 顶点总数。
    pub fn point_count(&self) -> usize {
        self.point_count
    }

    /// 笔迹数。
    pub fn stroke_count(&self) -> usize {
        self.strokes.len()
    }

    /// 从最旧的一条开始删，直到低于上限。
    fn evict_if_needed(&mut self) {
        while self.strokes.len() > MAX_STROKES || self.point_count > MAX_POINTS {
            if let Some(entry) = self.strokes.first() {
                let pc = entry.points.point_count();
                self.strokes.remove(0);
                self.point_count = self.point_count.saturating_sub(pc);
            } else {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::pens::PenStyle;
    use super::super::stroke::StrokePoints;
    use super::*;

    fn make_entry(bbox: Rect, points: Vec<crate::core::geometry::Point>) -> StrokeEntry {
        StrokeEntry {
            points: StrokePoints::Polyline { points },
            color: crate::core::color::Argb::new(0xFF, 0x00, 0x00, 0x00),
            width: 2.0,
            style: PenStyle::Solid,
            filled: false,
            bbox,
        }
    }

    #[test]
    fn add_and_undo() {
        let mut h = History::new();
        let e = make_entry(
            Rect {
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 10.0,
            },
            vec![],
        );
        h.add(e);
        assert_eq!(h.stroke_count(), 1);
        assert!(h.can_undo());
        let bbox = h.undo().unwrap();
        assert_eq!(bbox.w, 10.0);
        assert_eq!(h.stroke_count(), 0);
        assert!(h.can_redo());
    }

    #[test]
    fn redo_after_undo() {
        let mut h = History::new();
        let e = make_entry(
            Rect {
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 10.0,
            },
            vec![],
        );
        h.add(e);
        h.undo();
        let bbox = h.redo().unwrap();
        assert_eq!(bbox.w, 10.0);
        assert_eq!(h.stroke_count(), 1);
    }

    #[test]
    fn add_clears_redo() {
        let mut h = History::new();
        h.add(make_entry(
            Rect {
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 10.0,
            },
            vec![],
        ));
        h.undo();
        assert!(h.can_redo());
        h.add(make_entry(
            Rect {
                x: 0.0,
                y: 0.0,
                w: 20.0,
                h: 20.0,
            },
            vec![],
        ));
        assert!(!h.can_redo());
    }

    #[test]
    fn erase_by_rect() {
        let mut h = History::new();
        h.add(make_entry(
            Rect {
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 10.0,
            },
            vec![],
        ));
        h.add(make_entry(
            Rect {
                x: 100.0,
                y: 100.0,
                w: 10.0,
                h: 10.0,
            },
            vec![],
        ));
        let dirty = h.erase_by_rect(&Rect {
            x: 5.0,
            y: 5.0,
            w: 10.0,
            h: 10.0,
        });
        assert!(dirty.is_some());
        assert_eq!(h.stroke_count(), 1);
    }

    #[test]
    fn clear() {
        let mut h = History::new();
        h.add(make_entry(
            Rect {
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 10.0,
            },
            vec![],
        ));
        let dirty = h.clear();
        assert!(dirty.is_some());
        assert_eq!(h.stroke_count(), 0);
        assert!(!h.can_undo());
    }

    /// ★ 回归：`clear` 只把长度归零，**容量留着**；`shrink_to_fit` 才把容量退回去。
    ///
    /// 这条区分是"功能关了却没释放内存"的直接原因——用户看到的是开关拨到关，
    /// 进程却照样占着那一大片。这里把它钉成可执行的事实，将来谁把手删了都会变红。
    #[test]
    fn shrink_to_fit_releases_capacity_that_clear_keeps() {
        let mut h = History::new();
        for i in 0..200 {
            h.add(make_entry(
                Rect {
                    x: i as f32,
                    y: 0.0,
                    w: 1.0,
                    h: 1.0,
                },
                vec![],
            ));
        }
        let grown = h.capacity_pair().0;
        assert!(grown >= 200, "应已撑起容量，实际 {grown}");

        // clear 之后：长度归零，容量**仍 ≥ 撑起的量**。
        let _ = h.clear();
        assert_eq!(h.stroke_count(), 0);
        assert!(
            h.capacity_pair().0 >= grown,
            "clear 应当保留容量（目的就是'马上又要画'），实际 {} < {grown}",
            h.capacity_pair().0
        );

        // shrink_to_fit 之后：容量降下来。
        let before = h.capacity_pair().0;
        h.shrink_to_fit();
        let after = h.capacity_pair().0;
        assert!(
            after < before,
            "shrink_to_fit 应把容量还回去：{before} → {after} 没有变小"
        );
    }
}
