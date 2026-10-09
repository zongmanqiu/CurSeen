//! 点击效果。精确复刻 Python ripples.py:21-98。

use crate::core::color::Argb;
use crate::core::draw::DrawList;
use crate::core::geometry::{Point, Rect};

use super::settings::RippleStyle;

/// 单个波纹。
#[derive(Clone, Debug)]
pub struct Ripple {
    pub center: Point,
    pub color: Argb,
    pub outer_size: f32,
    /// 波纹样式（描边/填充，见 `RippleStyle`；移植自 Python 版的 hollow 布尔）。
    pub style: RippleStyle,
    pub duration_ms: f32,
    pub elapsed: f32,
    pub alive: bool,
}

impl Ripple {
    pub fn new(
        center: Point,
        color: Argb,
        style: RippleStyle,
        outer_size: f32,
        duration_ms: f32,
    ) -> Self {
        Self {
            center,
            color,
            outer_size,
            style,
            duration_ms,
            elapsed: 0.0,
            alive: true,
        }
    }

    pub fn tick(&mut self, dt_ms: f32) {
        if !self.alive {
            return;
        }
        self.elapsed += dt_ms;
        if self.elapsed / self.duration_ms >= 1.0 {
            self.alive = false;
        }
    }

    pub fn progress(&self) -> f32 {
        (self.elapsed / self.duration_ms.max(1e-3)).min(1.0)
    }

    pub fn draw(&self, list: &mut DrawList) {
        if !self.alive {
            return;
        }
        let p = self.progress();
        let alpha_f = 1.0 - p; // 1.0 → 0.0
        let size = self.outer_size * (0.2 + p * 0.8);
        let r = size * 0.5;
        let a = (alpha_f * 255.0) as u8;
        match self.style {
            RippleStyle::Ring => {
                let pen_w = (self.outer_size * 0.1).max(1.0);
                list.stroke_circle(self.center, r, pen_w, self.color.with_alpha(a));
            }
            RippleStyle::Thin => {
                let pen_w = (self.outer_size * 0.04).max(1.0);
                list.stroke_circle(self.center, r, pen_w, self.color.with_alpha(a));
            }
            RippleStyle::Disc => {
                list.fill_circle(self.center, r, self.color.with_alpha(a));
            }
            RippleStyle::Double => {
                let pen_w = (self.outer_size * 0.06).max(1.0);
                list.stroke_circle(self.center, r, pen_w, self.color.with_alpha(a));
                list.stroke_circle(
                    self.center,
                    r * 0.62,
                    pen_w,
                    // 先升 u32 再算：`a: u8` 时 `a * 2` 在 a > 127 会溢出
                    // （debug 直接 panic，release 静默回绕），内环透明度因此偏暗。
                    self.color.with_alpha((a as u32 * 2 / 3) as u8),
                );
            }
        }
    }

    /// 脏区：按从出生到死亡的最大范围算（outer_size / 2 + 1px AA）。
    pub fn dirty_rect(&self) -> Rect {
        let half = self.outer_size * 0.5 + 2.0;
        Rect::new(
            self.center.x - half,
            self.center.y - half,
            half * 2.0,
            half * 2.0,
        )
    }
}

/// 波纹管理器。用活跃前缀 + swap_remove 而不是 Vec::retain。
pub struct RippleManager {
    ripples: Vec<Ripple>,
    cap: usize,
}

impl RippleManager {
    pub const CAP: usize = 64;

    pub fn new() -> Self {
        Self {
            ripples: Vec::with_capacity(Self::CAP),
            cap: Self::CAP,
        }
    }

    /// 加一个波纹。容量已满时淘汰最旧的一个、再接纳新波纹（保证最新一次点击始终可见）。
    ///
    /// 返回 `true` 表示已接纳。`add` 永不丢弃新到来的波纹——满了就腾最旧的位置。
    pub fn add(&mut self, r: Ripple) -> bool {
        if self.ripples.len() >= self.cap {
            // 用 `remove(0)` 而非 `swap_remove(0)`：后者会把最后一个（最新）元素挪到队首，
            // 把"最旧优先淘汰"的环形顺序打乱，导致淘汰错乱、顺序乱跳。
            self.ripples.remove(0);
        }
        self.ripples.push(r);
        true
    }

    /// 推进全部波纹，返回是否有存活对象。
    pub fn tick(&mut self, dt_ms: f32) -> bool {
        let mut any_alive = false;
        for r in &mut self.ripples {
            r.tick(dt_ms);
            if r.alive {
                any_alive = true;
            }
        }
        // 压缩：把死亡的元素移到末尾然后 truncate
        self.ripples.retain(|r| r.alive);
        any_alive
    }

    pub fn draw(&self, list: &mut DrawList) {
        for r in &self.ripples {
            r.draw(list);
        }
    }

    pub fn clear(&mut self) {
        self.ripples.clear();
    }

    pub fn active(&self) -> usize {
        self.ripples.len()
    }

    /// 所有波纹的合并脏区。
    pub fn dirty_rect(&self) -> Rect {
        if self.ripples.is_empty() {
            return Rect::ZERO;
        }
        let mut r = self.ripples[0].dirty_rect();
        for ripple in &self.ripples[1..] {
            r = r.union(ripple.dirty_rect());
        }
        r
    }
}

impl Default for RippleManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ripple_lifecycle() {
        let mut r = Ripple::new(
            Point::ZERO,
            Argb(0xFFFFFF00),
            RippleStyle::Ring,
            100.0,
            300.0,
        );
        assert!(r.alive);
        assert_eq!(r.progress(), 0.0);

        r.tick(150.0);
        assert!(r.alive);
        assert!((r.progress() - 0.5).abs() < 0.01);

        r.tick(200.0);
        assert!(!r.alive);
    }

    #[test]
    fn ripple_manager_cap() {
        let mut m = RippleManager::new();
        for _ in 0..70 {
            m.add(Ripple::new(
                Point::ZERO,
                Argb(0xFF0000FF),
                RippleStyle::Ring,
                100.0,
                300.0,
            ));
        }
        assert!(m.active() <= RippleManager::CAP);
    }

    #[test]
    fn ripple_hollow_stroke() {
        let r = Ripple::new(
            Point::ZERO,
            Argb(0xFFFFFF00),
            RippleStyle::Ring,
            100.0,
            300.0,
        );
        let mut dl = DrawList::new();
        r.draw(&mut dl);
        // hollow = true → stroke command
        assert!(!dl.cmds().is_empty());
    }
}
