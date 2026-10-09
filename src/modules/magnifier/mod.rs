//! 放大镜（独立原生模块，不用 Lua）。
//!
//! 两种形态 = 两个各自独立的模块（用户拍板，2026-09-20），**共用本文件里的取样几何**
//! 与 `platform::pip_window` 那套呈现：
//! - [`pip`]：画中画放大 —— 面板是屏幕上的一小块，抓鼠标附近方域 → 放大 → 显示其中。
//! - [`fullscreen`]：全屏放大 —— 面板就是**整个虚拟屏**，于是"跟随鼠标"落到取样区上。
//!
//! # 两种形态的取样几何：画中画"居中"、全屏"锚住光标"
//!
//! 两者共用同一条链路，但**锚点不同**——这正是"全屏能不能点得动"的关键：
//!
//! ```text
//! 画中画：面板以鼠标为中心 + 取样区以鼠标为中心 ⇒ 两个"居中"相抵，光标落在自己身上
//! 全屏：  面板钉死在屏幕原点（尺寸 = 整屏）      ⇒ 只有取样区居中，相抵关系断了
//! ```
//!
//! 全屏若照抄画中画的"取样区居中"，光标下的内容会被显示到**屏幕中心**去：用户按看到的
//! 画面去点，点到的是别处（"鼠标无法点击"）。所以全屏改用 [`anchored_sample_rect`]：
//! 让"光标正下方的内容恰好显示在光标处"。于是面板尺寸**不必**撑成 2 倍屏幕——那是
//! "面板跟着鼠标、又要盖满屏"的想当然，会让取样区越出虚拟屏、`BitBlt` 拿黑边。

pub mod fullscreen;
pub mod pip;

use crate::platform::display::ScreenRect;

/// 鼠标静止时覆盖层出帧的节拍（毫秒）。
///
/// ★ 与两个放大镜运行时内部的抓帧节流（`pip::MAX_REFRESH_INTERVAL` /
///   `fullscreen::MIN_INTERVAL`）同值：帧门按 30ms 定时唤醒，模块层到点重抓、未到点
///   退回，两侧用同一份口径才不会出现"帧门 60ms 才醒、抓帧门却 33ms 想抓"（白醒）或
///   反过来"抓帧门等久了帧门才安排"（延迟）。要调刷新率时改这一处常量即可。
///
/// # 它为什么必须存在（2026-09-21 用户报告）
///
/// 帧门原先在"光标没动"时返回 `Idle`（彻底休眠），于是指针停住时屏幕上自己会变的
/// 内容——点击冒出的弹出菜单、右键菜单、进度条、视频——永远进不了放大镜面板，
/// 鼠标随便动一动才追上。现在静止期按这个节拍持续出帧，画面实时跟上屏幕。
pub const LOOK_INTERVAL_MS: u32 = 30;

/// 以鼠标为中心、边长 `cw×ch` 的取样矩形（钳到虚拟屏内）。
///
/// 抽成独立函数是为了让"取样区跟着倍率变小"这件事**可判**：倍率是这套采样的唯一
/// 缩放手段——`PipWindow` 只负责把这一小块无损拉到面板尺寸，不再自己算一次倍率。
///
/// ★ 两种形态（画中画 / 全屏）共用它：它们唯一的差别是"面板多大"，取样这块完全一样。
pub(crate) fn sample_rect(mouse: (f32, f32), cw: u32, ch: u32, screen: ScreenRect) -> ScreenRect {
    let (cw, ch) = (cw as i32, ch as i32);
    let x = clamp_center(mouse.0, cw, screen.x, screen.x + screen.w);
    let y = clamp_center(mouse.1, ch, screen.y, screen.y + screen.h);
    ScreenRect::new(x - cw / 2, y - ch / 2, cw, ch)
}

/// 把"以 `center` 为中心、边长 `len` 的窗口"钳到 `[lo, hi)` 内，返回钳后的窗口**中心**。
/// `len ≤ hi - lo`（调用方保证）。
fn clamp_center(center: f32, len: i32, lo: i32, hi: i32) -> i32 {
    let left = (center as i32 - len / 2).clamp(lo, hi - len);
    left + len / 2
}

/// 全屏放大的取样矩形：**锚住光标**——让光标正下方那块内容恰好显示在光标处。
///
/// # 与 [`sample_rect`] 的唯一差别是"锚点"
///
/// ```text
/// sample_rect（画中画）：取样区以鼠标为中心（面板中心也对准鼠标，两相抵消 ⇒ 光标在自身）
/// anchored_sample_rect（全屏）：面板钉在屏幕原点，只能靠"位移映射"把光标钉在自身
/// ```
///
/// 全屏的面板固定在屏幕原点（尺寸 = 整屏），没有第二个"居中"来抵消，只能直接构造这条
/// 恒等关系：`显示(光标) = 面板原点 + (光标 − 取样原点) × 有效倍率`，要它恒等于光标。
///
/// 令 `t = (光标 − 屏幕原点) / 屏幕边长 ∈ [0,1]`、`取样原点 = 屏幕原点 + t × (屏幕边长 −
/// 取样边长)`，则 `光标 − 取样原点 = t × 取样边长`，代入即得
/// `显示(光标) = 屏幕原点 + t × 屏幕边长 = 光标`（恒等）。
///
/// ★ 副产品：取样区**天然落在屏内**（`t=0` 贴原点、`t=1` 贴右/下边），无需再钳——出界
///   会被 `BitBlt` 拿回黑边。
pub(crate) fn anchored_sample_rect(
    mouse: (f32, f32),
    cw: u32,
    ch: u32,
    screen: ScreenRect,
) -> ScreenRect {
    let x = anchor_axis(mouse.0, screen.x, screen.w, cw as i32);
    let y = anchor_axis(mouse.1, screen.y, screen.h, ch as i32);
    ScreenRect::new(x, y, cw as i32, ch as i32)
}

/// 单轴锚定：`t = (鼠标 − 原点) / 跨度`，取样原点 = 原点 + `t` ×（跨度 − 取样边长）。
///
/// 抽出来是因为 x/y 两轴算式一模一样，且这是"点得准"的唯一来源——两处各写一遍迟早漏改
/// 一处（本项目已吃过"同一份数字抄两遍"的亏）。
fn anchor_axis(mouse: f32, origin: i32, span: i32, size: i32) -> i32 {
    let travel = (span - size).max(0) as f64;
    let t = if span > 0 {
        ((mouse - origin as f32) as f64 / span as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    origin + (t * travel).round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_center_keeps_window_inside() {
        assert_eq!(clamp_center(10.0, 40, 0, 100), 20);
        assert_eq!(clamp_center(130.0, 40, 0, 100), 80);
        assert_eq!(clamp_center(50.0, 40, 0, 100), 50);
    }

    /// 越界鼠标也要能钳回屏内（负原点虚拟桌面同样成立）。
    #[test]
    fn sample_rect_clamps_bad_mouse() {
        let s = ScreenRect::new(-200, -100, 1000, 800);
        let r = sample_rect((-500.0, -1000.0), 48, 32, s);
        assert_eq!((r.w, r.h), (48, 32));
        assert!(r.x >= s.x, "取样区不许越出虚拟屏左边界");
        assert!(r.y >= s.y, "取样区不许越出虚拟屏上边界");
    }

    /// 取样区恒被完全包在虚拟屏内（BitBlt 出界会拿到黑边，是"看一眼就不对"的缺陷）。
    #[test]
    fn sample_rect_never_leaves_the_virtual_screen() {
        let s = ScreenRect::new(0, 0, 1920, 1080);
        for mouse in [(0.0, 0.0), (1919.0, 1079.0), (5000.0, -3000.0)] {
            let r = sample_rect(mouse, 384, 270, s);
            assert!(r.x >= 0 && r.y >= 0, "取样区左上角出界: {r:?}");
            assert!(
                r.x + r.w <= s.x + s.w && r.y + r.h <= s.y + s.h,
                "取样区右下角出界: {r:?}"
            );
        }
    }

    /// ★★★ 全屏取样必须**锚住光标**：光标下的内容显示在光标处（identity）。
    ///
    /// 这是"全屏放大能不能直接点击"的**唯一**判据。判据直接复刻生产链路：
    /// `显示(光标) = 面板原点 +（光标 − 取样原点）× 有效倍率`，有效倍率 = 面板边长 ÷ 取样边长
    /// （`StretchDIBits` 实际用的那个）。差一个源像素以内都算点得准（`anchor_axis` 取整）。
    #[test]
    fn anchored_sample_keeps_the_cursor_under_itself() {
        let s = ScreenRect::new(0, 0, 1920, 1080);
        for zoom in 1..=8i32 {
            let cw = 1920 / zoom;
            let ch = 1080 / zoom;
            for mouse in [(0.0, 0.0), (960.0, 540.0), (1919.0, 1079.0), (300.0, 900.0)] {
                let r = anchored_sample_rect(mouse, cw as u32, ch as u32, s);
                let zx = 1920.0 / r.w as f32;
                let zy = 1080.0 / r.h as f32;
                let dx = s.x as f32 + (mouse.0 - r.x as f32) * zx;
                let dy = s.y as f32 + (mouse.1 - r.y as f32) * zy;
                assert!(
                    (dx - mouse.0).abs() <= zx * 0.5 + 1.0
                        && (dy - mouse.1).abs() <= zy * 0.5 + 1.0,
                    "zoom={zoom} 鼠标 {mouse:?} 的内容被显示到 ({dx:.1},{dy:.1})，应 ≈ 光标本身"
                );
            }
        }
    }

    /// 锚定取样同样不许越出虚拟屏（含**负原点**虚拟桌面）。
    #[test]
    fn anchored_sample_never_leaves_the_virtual_screen() {
        for s in [
            ScreenRect::new(0, 0, 1920, 1080),
            ScreenRect::new(-1920, -300, 3840, 1080),
        ] {
            for mouse in [
                (0.0, 0.0),
                (1919.0, 1079.0),
                (-9999.0, 9999.0),
                (500.0, 400.0),
            ] {
                let r = anchored_sample_rect(mouse, 480, 270, s);
                assert!(
                    r.x >= s.x && r.y >= s.y && r.x + r.w <= s.x + s.w && r.y + r.h <= s.y + s.h,
                    "屏幕 {s:?} 鼠标 {mouse:?} 取样区出界: {r:?}"
                );
            }
        }
    }

    /// 光标推到左上/右下角时，取样区应恰好贴到对应角（不是被钳住留缝）。
    #[test]
    fn anchored_sample_pins_to_both_ends() {
        let s = ScreenRect::new(0, 0, 1920, 1080);
        let r = anchored_sample_rect((0.0, 0.0), 480, 270, s);
        assert_eq!((r.x, r.y), (0, 0), "左上角光标 ⇒ 取样区贴左上");
        let r = anchored_sample_rect((1920.0, 1080.0), 480, 270, s);
        assert_eq!(
            (r.x + r.w, r.y + r.h),
            (1920, 1080),
            "右下角光标 ⇒ 取样区贴右下"
        );
    }
}
