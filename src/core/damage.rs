//! 脏矩形累积器：收集本帧所有变化的区域，供局部提交使用。

/// 脏矩形累积器。
///
/// 用并集包围盒而非多个矩形——`UpdateLayeredWindow` 一次只提交一个矩形，
/// 提交多个矩形要多次调用，反而更慢。
#[derive(Debug, Default, Clone)]
pub struct Damage {
    rect: Option<(i32, i32, i32, i32)>,
    boxes: u8,
}

/// 超过此数量的不相交区域时退化为整窗。
const MAX_BOXES: u8 = 8;

impl Damage {
    /// 添加矩形脏区。
    pub fn add_rect(&mut self, x: i32, y: i32, w: i32, h: i32) {
        if w <= 0 || h <= 0 {
            return;
        }
        self.boxes = self.boxes.saturating_add(1);
        if self.boxes > MAX_BOXES {
            // 退化：清空，让 take() 返回 None → 整窗刷新
            self.rect = None;
            return;
        }
        match self.rect {
            Some(ref mut r) => {
                let x2 = r.0.min(x);
                let y2 = r.1.min(y);
                let r2 = (r.0 + r.2).max(x + w);
                let b2 = (r.1 + r.3).max(y + h);
                *r = (x2, y2, r2 - x2, b2 - y2);
            }
            None => {
                self.rect = Some((x, y, w, h));
            }
        }
    }

    /// 添加圆形脏区（外接矩形，含 1px 抗锯齿余量）。
    pub fn add_circle(&mut self, cx: f32, cy: f32, r: f32) {
        // 极端半径（∞ / NaN / 远大于屏幕尺寸）下，原 `r.ceil() as i32 + 1` 与 `ri * 2`
        // 会 i32 溢出（debug 直接 panic）。这里把半径钳到安全上限并用饱和运算，
        // 正常范围内行为不变。
        let ri = (r.ceil() as i32)
            .clamp(0, i32::MAX / 2 - 2)
            .saturating_add(1);
        let x = (cx as i32).saturating_sub(ri);
        let y = (cy as i32).saturating_sub(ri);
        let d = ri.saturating_mul(2);
        self.add_rect(x, y, d, d);
    }

    /// 取并集矩形；None 表示无脏区。
    pub fn take(&mut self) -> Option<(i32, i32, i32, i32)> {
        let r = self.rect.take();
        self.boxes = 0;
        r
    }

    pub fn is_empty(&self) -> bool {
        self.rect.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_rect() {
        let mut d = Damage::default();
        d.add_rect(10, 10, 50, 50);
        assert_eq!(d.take(), Some((10, 10, 50, 50)));
        assert!(d.is_empty());
    }

    #[test]
    fn union_two_disjoint() {
        let mut d = Damage::default();
        d.add_rect(0, 0, 10, 10);
        d.add_rect(20, 20, 10, 10);
        let r = d.take().unwrap();
        assert_eq!(r, (0, 0, 30, 30));
    }

    #[test]
    fn union_contains() {
        let mut d = Damage::default();
        d.add_rect(5, 5, 20, 20);
        d.add_rect(0, 0, 50, 50);
        let r = d.take().unwrap();
        assert_eq!(r, (0, 0, 50, 50));
    }

    #[test]
    fn circle_to_rect() {
        let mut d = Damage::default();
        d.add_circle(100.0, 100.0, 10.0);
        let r = d.take().unwrap();
        assert!(r.2 > 0 && r.3 > 0);
    }

    #[test]
    fn take_resets() {
        let mut d = Damage::default();
        d.add_rect(0, 0, 10, 10);
        d.take();
        assert!(d.is_empty());
    }
}
