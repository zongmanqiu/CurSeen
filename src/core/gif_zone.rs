//! GIF 录制选区的纯几何命中判定。无 windui 依赖，纯数据、可单测。
//!
//! 判断一个物理坐标点落在选区的哪个交互区，语义：
//! - 8 个手柄点（四角 = 双轴缩放，四边中点 = 单边缩放）。
//! - 边框带（避开 8 点）= **移动整块选区**。
//! - 选区**内部** = `None`（录屏要在框内操作被录应用，必须**鼠标穿透**）。
//! - 选区**外部** = `None`（同样穿透）。
//!
//! ★ 传入的 `rect` 是**选区本身**（未外扩，两侧调用点传的都是 `ScreenRect` 选区）。
//!   绘制端 `overlay::draw_gif_frame` 把手柄方块画在"沿四周外扩一条边框厚度
//!   （[`FRAME_LOGICAL`]）"的外框角上，所以**画的位置**比**判定的锚点**靠外 2 逻辑 px。
//!   这个偏移小于手柄的命中半径（[`HANDLE_LOGICAL`] / 2 = 5.5 逻辑 px），于是画出来的
//!   那一块整体落在命中区里 —— "画了点到不着"依然不成立，只在外侧多出 2px 的死边。
//!   边框带同理：命中半宽 [`BORDER_LOGICAL`]（= 边框厚度 + 2）覆盖了边框线内外各一圈，
//!   所以"贴着框点"也拖得动。
//! ★ 两个尺寸 `handle_px` / `border_px` 用的单位与 `rect.x` 相同（物理像素），
//!   由调用方用逻辑像素 × dpi 换算好再传（[`hit_zone_at`] 是那个换算的单一来源）。

use crate::core::geometry::ScreenRect;

/// 命中区域。`Body` 之外的 `None` = 穿透（透明），不再发任何鼠标事件给覆盖层。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitZone {
    /// 未命中任何交互区（选区内部或外部）→ 穿透。
    None,
    /// 边框带（非 8 点处）→ 拖动移动整块选区。
    Body,
    /// 四角（双轴缩放）。
    CornerTL,
    CornerTR,
    CornerBL,
    CornerBR,
    /// 四边中点（单边缩放）。
    EdgeT,
    EdgeB,
    EdgeL,
    EdgeR,
}

/// 手柄方块边长（**逻辑像素**）—— 绘制与命中的**单一来源**。
///
/// ★ 为什么必须只有一份：画手柄的是 `runtime::overlay::draw_gif_frame`，判命中
///   的是这里。两处各写一个字面量，改一处忘另一处，"画了却点不到"或"点得到却没画"
///   就会静默出现——而那正是这类交互最贵的一类 bug（用户只能描述成"手柄有时候不好使"）。
///   逻辑像素而不是物理，是因为绘制端（windui `Canvas`）的单位是逻辑 dp；物理换算只在
///   [`hit_zone_at`] 里做一次。
pub const HANDLE_LOGICAL: f32 = 11.0;

/// 选框边框厚度（**逻辑像素**）。见 [`HANDLE_LOGICAL`] 关于单一来源的说明。
pub const FRAME_LOGICAL: f32 = 2.0;

/// 边框带命中的**半宽**（逻辑像素）：以边框线为中心，向内向外各这么多。
///
/// 比边框本身厚 2：手指（鼠标）落点从来不是一条线，正好等于线宽时"贴着边框点"极易落空。
/// 绘制端画的是外扩一整条边的四条矩形（见 `draw_gif_frame`），这里比它更宽一点——
/// 宽出来的部分落在框内，对"整块平移"的语义无害。
pub const BORDER_LOGICAL: f32 = FRAME_LOGICAL + 2.0;

/// 判定一个世界/物理坐标点落在 `rect`（外部框）的哪个交互区。
///
/// - `handle_px`：8 点各自的**命中半径**（以锚点为中心的正方形边长）。
/// - `border_px`：边框带的半宽（以边框线为中心向内/外各延伸这么多）。
///
/// 判定顺序：8 个手柄点（角优先）→ 边框带 `Body` → `None`。
pub fn hit_gif_zone(rect: ScreenRect, x: i32, y: i32, handle_px: i32, border_px: i32) -> HitZone {
    if handle_px <= 0 || border_px <= 0 || rect.w <= 1 || rect.h <= 1 {
        return HitZone::None;
    }

    let (x0, y0) = (rect.x, rect.y);
    let (x1, y1) = (x0 + rect.w, y0 + rect.h);
    let cx = x0 + rect.w / 2;
    let cy = y0 + rect.h / 2;

    // 8 个锚点：(角, 对应命中区)。
    let anchors = [
        (x0, y0, HitZone::CornerTL),
        (x1, y0, HitZone::CornerTR),
        (x0, y1, HitZone::CornerBL),
        (x1, y1, HitZone::CornerBR),
        (cx, y0, HitZone::EdgeT),
        (cx, y1, HitZone::EdgeB),
        (x0, cy, HitZone::EdgeL),
        (x1, cy, HitZone::EdgeR),
    ];

    for (ax, ay, zone) in anchors {
        if x.abs_diff(ax) <= handle_px as u32 && y.abs_diff(ay) <= handle_px as u32 {
            return zone;
        }
    }

    // 边框带：以边框线为中心，向内/外各 `border_px`。
    let on_edge = |px: i32, edge: i32| px.abs_diff(edge) <= border_px as u32;
    if (on_edge(x, x0) || on_edge(x, x1)) && y >= y0 && y <= y1 {
        return HitZone::Body;
    }
    if (on_edge(y, y0) || on_edge(y, y1)) && x >= x0 && x <= x1 {
        return HitZone::Body;
    }

    HitZone::None
}

/// **逻辑尺寸 → 物理命中**：把 [`HANDLE_LOGICAL`] / [`BORDER_LOGICAL`] 按 dpi 换算，再交给
/// [`hit_gif_zone`]。
///
/// 存在的理由与那两个常量相同：命中换算原本抄在 `runtime::overlay::gif_zone_at_rt` 里，
/// 而截图**编辑态**的拖拽走的是 `Runtime::feed`（全局钩子事件，不在窗口层），需要**同一套**
/// 换算。两份换算一旦分叉，"拖得到"与"光标变成缩放箭头"就会落在不同的地方。
///
/// `screen` 用的坐标系必须与 `x`/`y` 一致（都是世界物理像素）。
pub fn hit_zone_at(rect: ScreenRect, x: i32, y: i32, dpi: f32) -> HitZone {
    let dpi = if dpi.is_finite() && dpi > 0.0 {
        dpi
    } else {
        1.0
    };
    let handle_px = (HANDLE_LOGICAL / 2.0 * dpi).round() as i32;
    let border_px = (BORDER_LOGICAL * dpi).round() as i32;
    hit_gif_zone(rect, x, y, handle_px, border_px)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 外部框 100x80，起于 (10, 20)。手柄半径 6，边框带半宽 4。
    const R: ScreenRect = ScreenRect {
        x: 10,
        y: 20,
        w: 100,
        h: 80,
    };
    const HP: i32 = 6;
    const BP: i32 = 4;

    #[test]
    fn corners_hit() {
        assert_eq!(hit_gif_zone(R, 10, 20, HP, BP), HitZone::CornerTL);
        assert_eq!(hit_gif_zone(R, 110, 20, HP, BP), HitZone::CornerTR);
        assert_eq!(hit_gif_zone(R, 10, 100, HP, BP), HitZone::CornerBL);
        assert_eq!(hit_gif_zone(R, 110, 100, HP, BP), HitZone::CornerBR);
        // 手柄半径边界内仍命中（角往内偏移 5）。
        assert_eq!(hit_gif_zone(R, 15, 25, HP, BP), HitZone::CornerTL);
    }

    #[test]
    fn edge_midpoints_hit() {
        assert_eq!(hit_gif_zone(R, 60, 20, HP, BP), HitZone::EdgeT);
        assert_eq!(hit_gif_zone(R, 60, 100, HP, BP), HitZone::EdgeB);
        assert_eq!(hit_gif_zone(R, 10, 60, HP, BP), HitZone::EdgeL);
        assert_eq!(hit_gif_zone(R, 110, 60, HP, BP), HitZone::EdgeR);
    }

    #[test]
    fn body_on_border_band() {
        // 上边框带、居中偏左（避开中点上手柄半径范围）。
        assert_eq!(hit_gif_zone(R, 30, 17, HP, BP), HitZone::Body);
        assert_eq!(hit_gif_zone(R, 30, 23, HP, BP), HitZone::Body);
        // 左边框带、垂直居中（避开 EdgeL 手柄半径范围）。
        assert_eq!(hit_gif_zone(R, 7, 70, HP, BP), HitZone::Body);
        assert_eq!(hit_gif_zone(R, 13, 70, HP, BP), HitZone::Body);
    }

    #[test]
    fn interior_and_exterior_are_none() {
        // 选区内部：必须穿透。
        assert_eq!(hit_gif_zone(R, 60, 60, HP, BP), HitZone::None);
        assert_eq!(hit_gif_zone(R, 40, 45, HP, BP), HitZone::None);
        // 远在边带之外但靠近中心线：内部仍 None。
        assert_eq!(hit_gif_zone(R, 60, 60, HP, 1), HitZone::None);
        // 选区外部（需避开转角手柄半径：左上角锚点 (10,20) 半径 6）。
        assert_eq!(hit_gif_zone(R, 3, 3, HP, BP), HitZone::None);
        assert_eq!(hit_gif_zone(R, 10, 12, HP, BP), HitZone::None);
        assert_eq!(hit_gif_zone(R, 120, 108, HP, BP), HitZone::None);
    }

    #[test]
    fn negative_origin() {
        // 副屏左/上的负原点，几何依旧正确。
        let r = ScreenRect {
            x: -1920,
            y: -1080,
            w: 200,
            h: 120,
        };
        assert_eq!(hit_gif_zone(r, -1920, -1080, HP, BP), HitZone::CornerTL);
        assert_eq!(hit_gif_zone(r, -1720, -1080, HP, BP), HitZone::CornerTR);
        assert_eq!(
            hit_gif_zone(r, -1820, -1020, HP, BP),
            HitZone::None,
            "内部穿透"
        );
    }

    #[test]
    fn degenerate_rect_or_sizes_return_none() {
        assert_eq!(
            hit_gif_zone(ScreenRect::new(0, 0, 0, 10), 1, 1, HP, BP),
            HitZone::None
        );
        assert_eq!(
            hit_gif_zone(ScreenRect::new(0, 0, 10, 0), 1, 1, HP, BP),
            HitZone::None
        );
        assert_eq!(hit_gif_zone(R, 10, 20, 0, BP), HitZone::None);
        assert_eq!(hit_gif_zone(R, 10, 20, HP, 0), HitZone::None);
    }

    /// ★ [`hit_zone_at`] 是**唯一**的"逻辑尺寸 → 物理命中"换算，dpi 必须真的乘进去。
    ///
    /// 判据取**同一物理点在不同 dpi 下结论相反**：点 (17, 23) 离左上角锚点 (10,20) 是
    /// (7,3)。dpi=1 时手柄半径 6（画的是 11 逻辑 px 的方块，命中取半边长 → 6），7 > 6
    /// 落不到手柄上，但它压在左边框线外 7 之内 → `Body`；dpi=2 时半径涨到 11，(7,3) 全在
    /// 范围内 → `CornerTL`。只有真乘了 dpi 才会出现这种翻转——忘了乘（恒按 1 倍算）
    /// 会让高 dpi 下的手柄"看得见却点不到"。
    #[test]
    fn hit_zone_at_scales_with_dpi() {
        assert_eq!(hit_zone_at(R, 17, 23, 1.0), HitZone::Body);
        assert_eq!(hit_zone_at(R, 17, 23, 2.0), HitZone::CornerTL);
        // 非法的 dpi 不该变成 0 尺寸（那会让整块选区都点不中）。
        assert_eq!(hit_zone_at(R, 10, 20, 0.0), HitZone::CornerTL);
        assert_eq!(hit_zone_at(R, 10, 20, f32::NAN), HitZone::CornerTL);
    }

    /// 常量之间的关系：边框带的命中半宽必须**大于**边框本身的厚度，否则"贴着框点"很容易落空。
    /// （断言的就是两个 const 的关系——clippy 的 assertions_on_constants 在此是误报，显式豁免。）
    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn border_band_is_thicker_than_the_frame() {
        assert!(
            BORDER_LOGICAL > FRAME_LOGICAL,
            "边框带命中半宽（{BORDER_LOGICAL}）必须大于边框厚度（{FRAME_LOGICAL}）"
        );
        assert!(HANDLE_LOGICAL > 0.0);
    }
}
