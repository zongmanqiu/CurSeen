//! 截图**编辑态** —— 框选之后"停一下"的那一档（`Ctrl` + 截图快捷键）。
//!
//! 用户口径（2026-09-23）：*"截图时，如果按住 Ctrl，选区选择后应该停顿，就像 gif 选区一样，
//! 允许选区缩放，也允许选区整体移动…并且，按住 ctrl 键停顿时，我希望可以对截图做简单处理，
//! 就像屏幕标注一样。换言之，截图和屏幕标注在本质上是相同的函数。"*
//!
//! 那句话的代码形态落在两处，本模块是第二处：
//!
//! 1. **画布复用**（不在本模块）：底图就是刚才框选用的那张冻结帧，交给
//!    `DrawingController::start_with_backdrop` —— 与屏幕标注**同一个函数**
//!    （`core::backdrop::Backdrop::paint_into`），只是多带一个选区（选区外压暗 +
//!    工具栏贴着选区放）。
//! 2. **本模块**：会话状态（选区 + 冻结帧 + 之后要不要钉图）与"确认那一刻把冻结帧与
//!    笔迹合成一张图"的**纯逻辑**。
//!
//! # 为什么放在 `runtime` 而不是 `modules`
//!
//! 选区**拖拽**走的是 `Runtime::feed` 的全局鼠标钩子事件（见 `Runtime::edit_drag` 的注释：
//! 为什么不复用覆盖层的窗口事件），而 `modules` 不得依赖 `runtime`（分层纪律）。
//! 合成那一半只依赖 `core` 与 `windui`，所以它与会话状态一起留在这里，也顺带获得了
//! 离屏单测的能力（`compose` 的判据全在像素上）。
//!
//! # 与 GIF 选框的关系（复用的是什么、不是的是什么）
//!
//! 复用：[`crate::core::gif_zone`] 的命中几何（`hit_zone_at`）与 `Runtime` 的
//! `clamp_move` / `clamp_resize_*`。那几样是**纯几何**，与"这块矩形是要录的还是要截的"
//! 无关。
//!
//! 不复用：`gif_sel` 那一份状态。GIF 选框挂着会弹控件条，且框内必须**穿透**（用户要在框里
//! 操作被录应用）；编辑态的框内恰恰**不能**穿透（用户要在里面画画）。两件事的状态混在
//! 一起，就会出现"进入截图编辑态时 GIF 控件条也冒出来"这类串味。

use std::rc::Rc;

use crate::core::draw::DrawList;
use crate::core::geometry::{Rect, ScreenRect};
use crate::core::gif_zone::{hit_zone_at, HitZone};
use crate::core::image::RgbaBitmap;
use crate::runtime::render::translate_offset;

/// 一次截图编辑会话。
///
/// # 它解决的是什么（用户口径 2026-09-23）
///
/// *"截图时，如果按住Ctrl，选区选择后应该停顿，就像gif选区一样，允许选区缩放，也允许
/// 选区整体移动…并且…我希望可以对截图做简单处理，就像屏幕标注一样。换言之，截图和
/// 屏幕标注在本质上是相同的函数。"*
///
/// 于是框选完成那一刻**不落剪贴板**，而是把会话状态握住；期间拖边框 = 调选区、
/// 在框内拖 = 画一笔、工具栏「保存」= 确认、`Esc` = 取消（`Enter` 在编辑态彻底不认）。
///
/// 生命周期：`SnapMode::ShotEdit` 的框选确认时创建（见 `Runtime::enter_shot_edit`），
/// 工具栏「保存」（确认）或 `Esc`（取消）时销毁。它是 `Runtime` 的字段，唯一所有者。
pub struct ShotEdit {
    /// 选区（**世界物理像素**，虚拟桌面坐标系；副屏在左时 x 为负）。用户拖手柄/边框时变。
    pub sel: ScreenRect,
    /// 框选时那一张**整屏冻结帧**：既是编辑画布（`Backdrop::Frozen`），也是最终裁剪的源头。
    ///
    /// `Rc` 与当时那次区域选择共享同一份像素（不复制整屏）。
    pub frame: Rc<RgbaBitmap>,
    /// 确认后是否还要**钉到桌面**（「截图并贴图」那把键的 Ctrl 变体为真，「截图」那把为假）。
    pub pin: bool,
    /// 正在拖的交互区 + 上一次的**世界物理**鼠标点（跨帧累加的增量语义）。
    ///
    /// 与 GIF 那条（`overlay::GifDrag`）同款，但**刻意各自持有一份**：GIF 的拖拽活在
    /// 覆盖层的窗口事件里（它需要 `ctx.capture()`），而编辑态的拖拽走 `Runtime::feed`
    /// 的全局钩子事件（不依赖窗口命中）。合到一处反而要在一个状态里表达两种来历。
    pub drag: Option<(HitZone, i32, i32)>,
}

impl ShotEdit {
    pub fn new(sel: ScreenRect, frame: Rc<RgbaBitmap>, pin: bool) -> Self {
        Self {
            sel,
            frame,
            pin,
            drag: None,
        }
    }

    /// 选区（世界物理）→ [`Rect`]（绘制与工具栏锚点要的浮点世界矩形）。
    pub fn region(&self) -> Rect {
        Rect {
            x: self.sel.x as f32,
            y: self.sel.y as f32,
            w: self.sel.w as f32,
            h: self.sel.h as f32,
        }
    }

    /// 世界物理点命中哪个交互带（拖拽与悬停光标共用同一套几何）。
    pub fn zone_at(&self, x: i32, y: i32, dpi: f32) -> HitZone {
        hit_zone_at(self.sel, x, y, dpi)
    }

    /// 选区在**冻结帧内**的像素矩形 `(x, y, w, h)`；与帧不相交时 `None`。
    ///
    /// 冻结帧的原点就是虚拟桌面原点（`Backdrop::capture(grab, screen, ..)` 抓的正是
    /// `screen` 那一块），所以「帧内偏移 = 世界坐标 − `screen` 原点」。
    ///
    /// ★ 四边都钳进帧内并**取整**，而且这组整数就是后续两步（裁图 / 把笔迹平移到帧原点）
    /// 共用的基准。两边各取一次整，就会出现"图里少了右边一列、笔迹却仍画在没少的位置上"
    /// —— 这种错在截图产物里表现为**整体错位一格**，肉眼几乎只能靠"和屏幕上看到的比"发现。
    pub fn frame_rect(&self, screen: ScreenRect) -> Option<(i32, i32, i32, i32)> {
        let (fw, fh) = (self.frame.w as i32, self.frame.h as i32);
        if fw <= 0 || fh <= 0 {
            return None;
        }
        let x0 = (self.sel.x - screen.x).clamp(0, fw);
        let y0 = (self.sel.y - screen.y).clamp(0, fh);
        let x1 = (self.sel.right() - screen.x).clamp(0, fw);
        let y1 = (self.sel.bottom() - screen.y).clamp(0, fh);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        Some((x0, y0, x1 - x0, y1 - y0))
    }
}

/// 合成最终产物：**冻结帧裁到选区** + 笔迹 → 非预乘 RGBA 位图。
///
/// 两趟做（先把笔迹离屏光栅到一块空缓冲、再逐像素合成到底图上），而不是"直接把笔迹画在
/// 底图上"：`DibTarget` 的输出是**预乘** alpha（`layer::region` 的离屏门禁、`render.rs` 的
/// 光栅测试都是按预乘用的）。两趟写法让"底图那块"始终只是普通像素，合成公式（source-over）
/// 一眼可验证；而底图 α 恒 255（`shot_to_bitmap` 强制补满），所以最终结果**不需要**
/// 再做一次 un-premultiply —— 预乘与非预乘在不透明像素上重合。
///
/// `annotations` 只该含**墨迹**（`DrawingController::draw_annotations_into`）。底图与压暗
/// 都不在里面：底图由本函数铺（手上就有那张冻结帧），压暗是**界面**效果，绝不进产物。
pub fn compose(e: &ShotEdit, annotations: &DrawList, screen: ScreenRect) -> Option<RgbaBitmap> {
    let (fx, fy, fw, fh) = e.frame_rect(screen)?;
    // ★ 这一刀正好贴在某个窗口的外框上时，产物最外圈 1px 是 DWM 那圈**半透明**边框线，
    //   直接留着会混进 60% 桌面色（用户报了三次的"多了 1px"）。这里把它反解成不透明的
    //   本色 —— 区域不变（边框线仍在），只是颜色不再透出窗口外面。
    //   判据与常量的实测出处见 `platform::window_probe`（`rect_is_window_frame` /
    //   `FRAME_BORDER_ALPHA`）。
    let on_frame = crate::platform::window_probe::rect_is_window_frame(ScreenRect::new(
        fx + screen.x,
        fy + screen.y,
        fw,
        fh,
    ));
    let mut out = if on_frame {
        e.frame.crop_frame_opaque(
            fx,
            fy,
            fw,
            fh,
            crate::platform::window_probe::FRAME_BORDER_ALPHA,
        )
    } else {
        e.frame.crop(fx, fy, fw, fh)
    }?
    .rgba;
    let (w, h) = (fw as u32, fh as u32);

    // 笔迹是空的（"我什么都没画，就想把这块裁下来"）时连引擎都不建——这是确认键最常见的
    // 一种按法，不该为它付一次 DirectWrite 初始化。
    if !annotations.cmds().is_empty() {
        let mut ann = vec![0_u8; out.len()];
        {
            use windui::render::RenderTarget as _;
            let mut engine = windui::text::DWriteEngine::new();
            let mut target = windui::render::DibTarget::new(&mut ann, w, h);
            let mut canvas = target.make_canvas(&mut engine, 1.0);
            // 笔迹画在**世界物理坐标**上：把"选区左上角"搬到目标图原点，1:1 不缩放。
            // 叠加层的实时渲染是 `scale = 1/dpi` 再让平台乘回 dpi（恒等），所以这里取 1.0
            // 得到的像素与用户在屏幕上看到的**逐像素一致**。
            translate_offset(
                annotations,
                &mut *canvas,
                1.0,
                -((fx + screen.x) as f32),
                -((fy + screen.y) as f32),
            );
        }
        // 预乘 source-over 到底图（底图 α=255 → 结果 α 也恒为 255）。
        for (d, s) in out.chunks_exact_mut(4).zip(ann.chunks_exact(4)) {
            let a = s[3] as f32 / 255.0;
            let inv = 1.0 - a;
            for (i, c) in d[..3].iter_mut().enumerate() {
                *c = (s[i] as f32 + *c as f32 * inv).round().clamp(0.0, 255.0) as u8;
            }
            d[3] = 255;
        }
    }

    RgbaBitmap::new(w, h, out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::color::Argb;

    /// 一张 `w×h` 纯白冻结帧（α=255，与 `shot_to_bitmap` 的产物同构）。
    fn white_frame(w: u32, h: u32) -> Rc<RgbaBitmap> {
        Rc::new(RgbaBitmap::new(w, h, vec![255_u8; (w * h * 4) as usize]).expect("白帧"))
    }

    fn scr(x: i32, y: i32, w: i32, h: i32) -> ScreenRect {
        ScreenRect { x, y, w, h }
    }

    fn px(buf: &[u8], w: u32, x: u32, y: u32) -> (u8, u8, u8, u8) {
        let i = ((y * w + x) * 4) as usize;
        (buf[i], buf[i + 1], buf[i + 2], buf[i + 3])
    }

    /// 帧内偏移要**减去虚拟桌面原点**：多屏（副屏在主屏左侧，原点是负的）时必须仍然正确。
    ///
    /// 只测"原点是 0"是抓不住这个错的 —— 那种情形下"减原点"与"不减"恰好相等。
    #[test]
    fn frame_rect_subtracts_the_virtual_desktop_origin() {
        let frame = white_frame(8, 6);
        let screen = scr(-100, -50, 8, 6);
        // 世界 (-98,-49) 在帧内就是 (2,1)。
        let e = ShotEdit::new(scr(-98, -49, 4, 3), frame, false);
        assert_eq!(e.frame_rect(screen), Some((2, 1, 4, 3)));

        // 原点为 0 时两者相等（对照组：证明上一条不是"恒等于入参"的假绿）。
        let frame0 = white_frame(8, 6);
        let e0 = ShotEdit::new(scr(2, 1, 4, 3), frame0, false);
        assert_eq!(e0.frame_rect(scr(0, 0, 8, 6)), Some((2, 1, 4, 3)));
    }

    /// 选区跑出帧外 / 与帧不相交时，帧内矩形被钳住；完全在外时给 `None`（别裁出一张空图）。
    #[test]
    fn frame_rect_clamps_into_the_frame() {
        let frame = white_frame(10, 10);
        let screen = scr(0, 0, 10, 10);
        // 右边与下边各越界 3 → 钳成 7×7。
        let e = ShotEdit::new(scr(3, 3, 10, 10), frame, false);
        assert_eq!(e.frame_rect(screen), Some((3, 3, 7, 7)));

        let frame2 = white_frame(10, 10);
        let outside = ShotEdit::new(scr(50, 50, 4, 4), frame2, false);
        assert_eq!(
            outside.frame_rect(screen),
            None,
            "完全在帧外 → 没有可裁的内容"
        );

        // 尺寸为 0 的退化选区同样不接受。
        let frame3 = white_frame(10, 10);
        let zero = ShotEdit::new(scr(2, 2, 0, 0), frame3, false);
        assert_eq!(zero.frame_rect(scr(0, 0, 10, 10)), None);
    }

    /// ★ 产物的**尺寸 = 选区**、**像素 = 冻结帧那一块**（没画东西时）。
    ///
    /// 这条同时钉住两件容易各错一半的事：裁的是"选区那块"而不是整帧；以及裁的**起点**
    /// 等于 `frame_rect` 算出来的偏移（尺寸对 ≠ 位置对 —— 只比尺寸的话，起点整体偏一格
    /// 也照样全绿）。
    #[test]
    fn compose_without_ink_is_exactly_the_cropped_frame() {
        // 帧里每一列的红色分量等于列号 ×8，于是"起点"可以从像素值直接读回来。
        let (w, h) = (8_u32, 6_u32);
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                rgba.extend_from_slice(&[(x as u8) * 8, (y as u8) * 8, 40, 255]);
            }
        }
        let frame = Rc::new(RgbaBitmap::new(w, h, rgba).expect("帧"));
        let screen = scr(0, 0, 8, 6);
        let e = ShotEdit::new(scr(2, 1, 4, 3), frame, false);

        let out = compose(&e, &DrawList::new(), screen).expect("应能合成");
        assert_eq!((out.w, out.h), (4, 3), "尺寸必须是选区的大小，不是整帧");
        assert_eq!(
            px(&out.rgba, 4, 0, 0),
            (2 * 8, 8, 40, 255),
            "产物左上角 = 帧内 (2,1) 那个像素"
        );
        assert_eq!(
            px(&out.rgba, 4, 3, 2),
            (5 * 8, 3 * 8, 40, 255),
            "右下角同理"
        );
    }

    /// ★★ 笔迹要落在**正确的位置**上，且是**正确的颜色**。
    ///
    /// 判据是一条**不透明**的红块：不透明意味着合成结果是精确值（不会被抗锯齿的混合权重
    /// 冲淡），于是"位置错一格"与"通道搞反了（B/G/R）"这两种错都躲不过。
    #[test]
    fn compose_paints_the_ink_at_the_selection_origin() {
        let frame = white_frame(8, 6);
        let screen = scr(0, 0, 8, 6);
        let e = ShotEdit::new(scr(2, 1, 4, 3), frame, false);

        // 世界坐标 (3,2) 处一个 2×2 实心红块 → 在产物里应落在局部 (1,1)。
        let mut ink = DrawList::new();
        ink.fill_rect(Rect::new(3.0, 2.0, 2.0, 2.0), Argb::new(255, 255, 0, 0));
        let out = compose(&e, &ink, screen).expect("应能合成");

        assert_eq!((out.w, out.h), (4, 3));
        for (x, y) in [(1_u32, 1_u32), (2, 1), (1, 2), (2, 2)] {
            assert_eq!(
                px(&out.rgba, 4, x, y),
                (255, 0, 0, 255),
                "红块内部 ({x},{y}) 必须是不透明的纯红"
            );
        }
        // 对照：原点那一格必须**还是白的** —— 少了它，"偏移量整个没生效（全图都红）"
        // 与"偏移量对了"就分不开。
        assert_eq!(
            px(&out.rgba, 4, 0, 0),
            (255, 255, 255, 255),
            "控点应仍是白底"
        );
    }

    /// 笔迹画到选区**之外**时不该出现在产物里（它在屏上看得见，但不属于这张图）。
    #[test]
    fn ink_outside_the_selection_is_dropped() {
        let frame = white_frame(8, 6);
        let screen = scr(0, 0, 8, 6);
        let e = ShotEdit::new(scr(2, 1, 4, 3), frame, false);

        let mut ink = DrawList::new();
        // 整幅左上角（选区之外）涂红。
        ink.fill_rect(Rect::new(0.0, 0.0, 2.0, 6.0), Argb::new(255, 255, 0, 0));
        let out = compose(&e, &ink, screen).expect("应能合成");

        for y in 0..3 {
            for x in 0..4 {
                assert_eq!(
                    px(&out.rgba, 4, x, y),
                    (255, 255, 255, 255),
                    "选区外的墨迹不该进产物（({x},{y}) 被污染）"
                );
            }
        }
    }

    /// 帧的像素与产物像素之间是 **1:1**（不缩放）：一条 1px 的竖线在产物里仍是 1px。
    ///
    /// 这条防的是"顺手把 dpi 当缩放系数乘进去"——那会让高 dpi 机器上的产物被二次放大，
    /// 而缩略图上很难看出来，只有数像素宽度才露馅。
    #[test]
    fn compose_is_one_to_one_pixels() {
        let frame = white_frame(6, 4);
        let screen = scr(0, 0, 6, 4);
        let e = ShotEdit::new(scr(1, 0, 4, 4), frame, false);
        let mut ink = DrawList::new();
        // 世界 x=2 那一列涂红（1px 宽、整高）。
        ink.fill_rect(Rect::new(2.0, 0.0, 1.0, 4.0), Argb::new(255, 255, 0, 0));
        let out = compose(&e, &ink, screen).expect("应能合成");

        assert_eq!(out.w, 4);
        // 局部列 = 2 - 1 = 1，且**只有**这一列是红的。
        for x in 0..4 {
            let expect_red = x == 1;
            for y in 0..4 {
                let p = px(&out.rgba, 4, x, y);
                if expect_red {
                    assert_eq!(p, (255, 0, 0, 255), "第 {x} 列应为红");
                } else {
                    assert_eq!(p, (255, 255, 255, 255), "第 {x} 列不该被涂");
                }
            }
        }
    }

    /// `region()` / `zone_at()` 的语义：前者给绘制，后者给拖拽，两者必须看**同一份**选区。
    #[test]
    fn region_and_zone_read_the_same_selection() {
        let frame = white_frame(8, 6);
        let e = ShotEdit::new(scr(10, 20, 100, 80), frame, false);
        assert_eq!(e.region(), Rect::new(10.0, 20.0, 100.0, 80.0));
        assert_eq!(e.zone_at(10, 20, 1.0), HitZone::CornerTL);
        assert_eq!(e.zone_at(60, 60, 1.0), HitZone::None, "框内应落给画笔");
    }
}
