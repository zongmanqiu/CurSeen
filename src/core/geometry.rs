//! 几何基元：Point、Rect、Size、ScreenRect。纯数据，无 windui 依赖。

/// 二维点（逻辑像素）。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const ZERO: Point = Point { x: 0.0, y: 0.0 };

    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn distance_to(self, other: Point) -> f32 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        (dx * dx + dy * dy).sqrt()
    }
}

impl From<(f32, f32)> for Point {
    fn from((x, y): (f32, f32)) -> Self {
        Self { x, y }
    }
}

/// 尺寸（逻辑像素）。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    pub w: f32,
    pub h: f32,
}

impl Size {
    pub const ZERO: Size = Size { w: 0.0, h: 0.0 };

    pub fn new(w: f32, h: f32) -> Self {
        Self { w, h }
    }
}

/// 矩形（逻辑像素，左上角原点）。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const ZERO: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
    };

    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn from_points(a: Point, b: Point) -> Self {
        let x = a.x.min(b.x);
        let y = a.y.min(b.y);
        Self {
            x,
            y,
            w: (a.x - b.x).abs(),
            h: (a.y - b.y).abs(),
        }
    }

    pub fn right(self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(self) -> f32 {
        self.y + self.h
    }

    pub fn center(self) -> Point {
        Point::new(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    pub fn contains(self, p: Point) -> bool {
        p.x >= self.x && p.x <= self.right() && p.y >= self.y && p.y <= self.bottom()
    }

    pub fn intersects(self, other: Rect) -> bool {
        self.x < other.right()
            && self.right() > other.x
            && self.y < other.bottom()
            && self.bottom() > other.y
    }

    pub fn union(self, other: Rect) -> Rect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Rect {
            x,
            y,
            w: self.right().max(other.right()) - x,
            h: self.bottom().max(other.bottom()) - y,
        }
    }

    pub fn inset(self, dx: f32, dy: f32) -> Rect {
        Rect {
            x: self.x + dx,
            y: self.y + dy,
            w: self.w - dx * 2.0,
            h: self.h - dy * 2.0,
        }
    }
}

/// 把 `outer` 挖掉 `hole` 之后剩下的部分切成**四条互不重叠**的边带。
///
/// 顺序恒为 `[上, 下, 左, 右]`：上与下是**整宽**的，左与右只占"上带与下带之间的那一段
/// 高度"——所以四块拼起来**恰好**等于挖掉洞之后的面积，没有一处被覆盖两次。
/// 这条"零重叠"的性质不是审美问题：压暗遮罩是半透明的，重叠处会混两次、比别处更暗，
/// 用户看到的是一个明显的**深色十字**。
///
/// 尺寸为负（洞比 `outer` 还大 / 完全跑到外面）时钳成 0 而不是负数——调用方按
/// `w > 0 && h > 0` 跳过，负尺寸的矩形一旦被画出来是"反向"的怪东西。
///
/// ★ 洞会**先被钳进 `outer`**（四边各自 clamp）。不钳的话"洞完全在外框之外"时四条边带
/// 会多铺几块、加起来超过整块面积（半透明遮罩就开始深浅不匀）。钳完之后
/// **四块面积之和恒等于 `outer` 减去洞与 `outer` 的交**，这条等式就是本函数的测试判据。
///
/// 三个消费方共用它：截图区域选择窗的压暗、截图编辑态的选区外压暗、以及将来任何
/// "只亮一块、其余压暗"的叠层。
pub fn outside_bands(outer: Rect, hole: Rect) -> [Rect; 4] {
    // 先把洞钳进外框：越界部分不该在遮罩上留下任何痕迹。
    let x0 = hole.x.clamp(outer.x, outer.right());
    let y0 = hole.y.clamp(outer.y, outer.bottom());
    let x1 = hole.right().clamp(outer.x, outer.right());
    let y1 = hole.bottom().clamp(outer.y, outer.bottom());
    let w = |v: f32| v.max(0.0);
    [
        // 上：整宽，从 outer 顶边到洞的顶边。
        Rect {
            x: outer.x,
            y: outer.y,
            w: outer.w,
            h: w(y0 - outer.y),
        },
        // 下：整宽，从洞的底边到 outer 底边。
        Rect {
            x: outer.x,
            y: y1,
            w: outer.w,
            h: w(outer.bottom() - y1),
        },
        // 左：只占 [y0, y1) 那一段。
        Rect {
            x: outer.x,
            y: y0,
            w: w(x0 - outer.x),
            h: w(y1 - y0),
        },
        // 右：同上，从洞的右边到 outer 右边。
        Rect {
            x: x1,
            y: y0,
            w: w(outer.right() - x1),
            h: w(y1 - y0),
        },
    ]
}

/// 一块**屏幕区域**（物理像素，虚拟桌面坐标系；多屏时 `x`/`y` 可以为负）。
///
/// 与 [`Rect`] 的区别（别互相强转，两套坐标系）：
/// | | [`Rect`] | [`ScreenRect`] |
/// |---|---|---|
/// | 单位 | 逻辑像素（dp） | 物理像素 |
/// | 坐标系 | 窗口/画布内，原点多为 0 | 虚拟桌面，原点可为负 |
/// | 类型 | `f32` | `i32` |
///
/// 之所以放在 `core`：`ScreenRect` 是纯数据（Win32 调用在 `platform::display`），
/// 而 `core::backdrop` / `core::damage` 都用它描述"整张虚拟桌面"的范围——若留在
/// `platform`，core 就得反向依赖 platform（违反分层）。`platform::display` 保留同名
/// 重导出，既有 `platform::display::ScreenRect` 的引用不必改。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ScreenRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl ScreenRect {
    /// 退化值：单显示器兜底。取不到系统值时用它，宁可画小也别画不出来。
    pub const FALLBACK: ScreenRect = ScreenRect {
        x: 0,
        y: 0,
        w: 1920,
        h: 1080,
    };

    /// 面积非正（含取系统值失败返回 0 的情形）时给兜底值。
    pub fn or_fallback(self) -> ScreenRect {
        if self.w <= 0 || self.h <= 0 {
            ScreenRect::FALLBACK
        } else {
            self
        }
    }

    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    /// 右边界（不含）。可能为负坐标下的大值，只用于比较。
    pub fn right(self) -> i32 {
        self.x.saturating_add(self.w)
    }

    /// 下边界（不含）。
    pub fn bottom(self) -> i32 {
        self.y.saturating_add(self.h)
    }

    /// 是否包含某个正整数坐标点（描边原点可在本区域内）。
    pub fn contains(self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_rect_fallback_on_degenerate() {
        assert_eq!(
            ScreenRect::new(0, 0, 0, 10).or_fallback(),
            ScreenRect::FALLBACK
        );
        assert_eq!(
            ScreenRect::new(-5, 0, -1, 10).or_fallback(),
            ScreenRect::FALLBACK
        );
        // 非退化就算负原点也保持原样（多屏左侧）
        let multi = ScreenRect::new(-1920, -100, 3840, 1180);
        assert_eq!(multi.or_fallback(), multi);
    }

    #[test]
    fn screen_rect_right_bottom_and_contains() {
        let r = ScreenRect::new(-10, -20, 100, 50);
        assert_eq!(r.right(), 90);
        assert_eq!(r.bottom(), 30);
        assert!(r.contains(0, 0));
        assert!(!r.contains(-11, 0));
        assert!(!r.contains(90, 0), "右边界是开区间");
    }

    #[test]
    fn rect_union() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(5.0, 5.0, 10.0, 10.0);
        let u = a.union(b);
        assert_eq!(u, Rect::new(0.0, 0.0, 15.0, 15.0));
    }

    #[test]
    fn rect_contains() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(r.contains(Point::new(5.0, 5.0)));
        assert!(!r.contains(Point::new(15.0, 5.0)));
    }

    /// ★★ 四条边带的**面积守恒**：四块拼起来必须恰好等于"挖掉洞之后"的面积。
    ///
    /// 只断言"四块都在"是抓不住重叠的 —— 而重叠在压暗遮罩上会混两次色，画面上是
    /// 一个肉眼可见的深色十字。所以这里用**面积**当判据：多一块、少一块、重叠一块，
    /// 三种错都会让等式不成立。
    #[test]
    fn outside_bands_tile_the_hole_complement_exactly() {
        let outer = Rect::new(0.0, 0.0, 100.0, 80.0);
        let hole = Rect::new(30.0, 20.0, 40.0, 30.0);
        let bands = outside_bands(outer, hole);
        let sum: f32 = bands.iter().map(|r| r.w * r.h).sum();
        assert_eq!(
            sum,
            outer.w * outer.h - hole.w * hole.h,
            "四块面积之和必须等于外框减洞（重叠/漏空都会破坏等式）"
        );
    }

    /// 边带被**推出外框之外**那一侧必须是零尺寸，而不是负数。
    ///
    /// 负数会画出"反向矩形"——调用方虽然大多会跳过非正的块，但把负尺寸留在数据里，
    /// 就等于把这条不变量交给了每个调用方各自记得检查。
    #[test]
    fn outside_bands_never_go_negative_when_the_hole_hangs_out() {
        let outer = Rect::new(10.0, 10.0, 50.0, 40.0);
        // 洞比外框大出一整圈，四边都要钳成 0。
        let bands = outside_bands(outer, Rect::new(-100.0, -100.0, 500.0, 500.0));
        for r in bands {
            assert!(r.w >= 0.0 && r.h >= 0.0, "不得出现负尺寸：{r:?}");
        }
        assert_eq!(
            bands.iter().map(|r| r.w * r.h).sum::<f32>(),
            0.0,
            "洞盖满整个外框时不该剩任何一条带"
        );

        // 洞完全在外框之外 → 遮罩应**整块压满**（面积守恒的退化形态）。
        let bands = outside_bands(outer, Rect::new(200.0, 10.0, 10.0, 10.0));
        assert_eq!(
            bands.iter().map(|r| r.w * r.h).sum::<f32>(),
            outer.w * outer.h,
            "洞跑到外框之外时，遮罩必须铺满整块（否则屏幕上会留一条没压暗的缝）"
        );
        assert_eq!(bands[2].w, 50.0, "左带应占满外框宽度");
    }
}
