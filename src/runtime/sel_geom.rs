//! 选区几何（纯函数，无状态）。
//!
//! 8 手柄 + 边框带的拉伸 / 移动与「钳进屏幕」。后者不是把一个标量 `clamp` 一下那么简单：
//! 位置与尺寸互相约束，还要处理负原点的虚拟桌面（`ScreenRect` 的 x 可以是 -1920）。

use super::*;

/// 把一次拖拽增量作用到选区上（平移 / 缩放，全部钳进屏幕并保最小尺寸）。
///
/// GIF 选框（[`Runtime::gif_resize_or_move`]）与截图编辑态（[`Runtime::edit_drag`]）共用它
/// —— 这是两者唯一的"同一套拖拽行为"落点。各写一遍就会出现"GIF 能拖到屏幕边上、编辑态
/// 差几像素"这种只在一边复现的不一致，而那种错几乎没法从用户描述里还原出来。
pub(crate) fn apply_zone(r: &mut ScreenRect, screen: ScreenRect, zone: HitZone, dx: i32, dy: i32) {
    let min = Runtime::GIF_ZONE_MIN_SIZE;
    match zone {
        HitZone::None => {}
        HitZone::Body => clamp_move(r, screen, dx, dy),
        HitZone::CornerTL => clamp_resize_tl(r, screen, dx, dy, min),
        HitZone::CornerTR => clamp_resize_tr(r, screen, dx, dy, min),
        HitZone::CornerBL => clamp_resize_bl(r, screen, dx, dy, min),
        HitZone::CornerBR => clamp_resize_br(r, screen, dx, dy, min),
        HitZone::EdgeT => clamp_resize_tl(r, screen, 0, dy, min),
        HitZone::EdgeB => clamp_resize_bl(r, screen, 0, dy, min),
        HitZone::EdgeL => clamp_resize_tl(r, screen, dx, 0, min),
        HitZone::EdgeR => clamp_resize_tr(r, screen, dx, 0, min),
    }
}

/// 选区（世界物理）→ 绘制与工具栏锚点要的浮点矩形。
pub(crate) fn sel_rect(r: ScreenRect) -> Rect {
    Rect {
        x: r.x as f32,
        y: r.y as f32,
        w: r.w as f32,
        h: r.h as f32,
    }
}

/// 把选区**整体**钳进屏幕（尺寸超过屏幕时贴着左上角放），四条边都保证落在屏内。
///
/// 与 [`clamp_move`]（按增量平移）不同：这是**静态**归一，服务的是"选区从别处算出来"
/// 的那些入口（框选确认）。不归一的话，越界的选区会让 `shot_edit::compose` 裁出一张
/// 尺寸与界面上看到的不一致的图 —— 而那种错只在屏幕边缘才复现。
pub(crate) fn clamp_sel(r: ScreenRect, screen: ScreenRect) -> ScreenRect {
    let w = r.w.clamp(1, screen.w.max(1));
    let h = r.h.clamp(1, screen.h.max(1));
    ScreenRect {
        x: r.x.clamp(screen.x, screen.right().saturating_sub(w)),
        y: r.y.clamp(screen.y, screen.bottom().saturating_sub(h)),
        w,
        h,
    }
}

/// 平移整块选区。用交互带拖动时逐位累加增量。
pub(crate) fn clamp_move(r: &mut ScreenRect, screen: ScreenRect, dx: i32, dy: i32) {
    r.x = (r.x.saturating_add(dx)).clamp(screen.x, screen.right().saturating_sub(r.w.max(0)));
    r.y = (r.y.saturating_add(dy)).clamp(screen.y, screen.bottom().saturating_sub(r.h.max(0)));
}

/// 移动并压缩矩形到 min——把 `x`(anchor 之外的一侧) 推进最多 `dx`，但不得越过屏幕内 min 边界。
///
/// 供缩放用：拖动某一边时，先把该边移动到目标位置（可能越界/过小），
/// 再 clamp 回 `[screen.init, screen.far - min]` 区间。
pub(crate) fn pushed(edge: i32, screen_init: i32, screen_far: i32, min: i32, dx: i32) -> i32 {
    edge.saturating_add(dx)
        .clamp(screen_init, screen_far.saturating_sub(min))
}

/// 缩放后追加约束：右边/下边不得覆盖左边/上边超过 min，且不可越出 `screen`。
/// 左上角锚点：改左/上边（right/bottom 不动），clamp 进 screen 且保 min。
pub(crate) fn clamp_resize_tl(r: &mut ScreenRect, screen: ScreenRect, dx: i32, dy: i32, min: i32) {
    let (mut left, mut top) = (r.x, r.y);
    let (right, bottom) = (r.right(), r.bottom());
    left = pushed(left, screen.x, right, min, dx);
    top = pushed(top, screen.y, bottom, min, dy);
    r.x = left;
    r.y = top;
    r.w = right.saturating_sub(left);
    r.h = bottom.saturating_sub(top);
}

/// 右上角锚点：右/上边可动。
pub(crate) fn clamp_resize_tr(r: &mut ScreenRect, screen: ScreenRect, dx: i32, dy: i32, min: i32) {
    let (mut right, mut top) = (r.right(), r.y);
    let left = r.x;
    let bottom = r.bottom();
    right = right
        .saturating_add(dx)
        .max(left.saturating_add(min))
        .clamp(left.saturating_add(min), screen.right());
    top = pushed(top, screen.y, bottom, min, dy);
    r.x = left;
    r.y = top;
    r.w = right.saturating_sub(left);
    r.h = bottom.saturating_sub(top);
}

/// 左下角锚点：左/下边可动。
pub(crate) fn clamp_resize_bl(r: &mut ScreenRect, screen: ScreenRect, dx: i32, dy: i32, min: i32) {
    let (mut left, mut bottom) = (r.x, r.bottom());
    let right = r.right();
    let top = r.y;
    left = pushed(left, screen.x, right, min, dx);
    bottom = bottom
        .saturating_add(dy)
        .max(top.saturating_add(min))
        .clamp(top.saturating_add(min), screen.bottom());
    r.x = left;
    r.y = top;
    r.w = right.saturating_sub(left);
    r.h = bottom.saturating_sub(top);
}

/// 右下角锚点：右/下边可动。
pub(crate) fn clamp_resize_br(r: &mut ScreenRect, screen: ScreenRect, dx: i32, dy: i32, min: i32) {
    let left = r.x;
    let top = r.y;
    let mut right = r.right();
    let mut bottom = r.bottom();
    right = right
        .saturating_add(dx)
        .max(left.saturating_add(min))
        .clamp(left.saturating_add(min), screen.right());
    bottom = bottom
        .saturating_add(dy)
        .max(top.saturating_add(min))
        .clamp(top.saturating_add(min), screen.bottom());
    r.w = right.saturating_sub(left);
    r.h = bottom.saturating_sub(top);
}
