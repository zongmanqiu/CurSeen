//! 叠层底图 —— 「截图与屏幕注释调用同一个函数」的落点。
//!
//! 用户原话：*"我们的截图是冻结了屏幕的。而屏幕注释的画板功能也需要这个。所以，从原理和
//! 函数上来说，截图与屏幕画板功能可以调用同一个函数。"*
//!
//! 可复用的最小单元 = **一份冻结帧 + 怎么把它铺到画布上**。三个消费方共用它：
//!
//! | 消费方 | 底图 | 画面冻结 | 视觉 |
//! |---|---|---|---|
//! | 截图区域选择 | [`Backdrop::Frozen`] | 是 | 冻结帧 + 选区外暗化 |
//! | 屏幕注释 · 透明幕布 | [`Backdrop::None`] | **否** | 叠层自身透明，**实时桌面可见**（2026-09-29 用户口径） |
//! | 屏幕注释 · 白板 | [`Backdrop::Veiled`]（白 α=255） | 是 | 视觉上仍是纯白板 |
//!
//! 屏幕注释一度是抓一整张虚拟桌面铺在叠层上（看着像实时桌面、内容其实是静的），
//! 2026-09-29 改回**透明幕布**：不抓屏、不冻结，省掉整张冻结帧（1080p ≈ 8 MB、
//! 4K ≈ 33 MB）与每帧铺满整屏的位图光栅。代价是出图时得**现抓**桌面当背景（见
//! `runtime::snap_flow::compose_annotation`）。
//!
//! # 为什么放在 `core` 而不是 `runtime`
//!
//! 消费方之一是 `modules/drawing`，而 **modules 不得依赖 runtime**（分层纪律）。core 是
//! 纯逻辑，只认 [`DrawList`] + [`RgbaBitmap`]，恰好满足。
//!
//! 抓屏这一步走**依赖倒置**：不是这里直接调 `platform::screenshot::grab`（那会让 core
//! 反向依赖 platform），而是由调用方把抓屏函数注入给 [`Backdrop::capture`]。既守住了
//! 分层，也让本模块的单测能在没有屏幕的情况下灌一张假帧。
//!
//! # 统一不变量
//!
//! 抓到的帧进渲染前**必须强制 α=255**：GDI `BitBlt` 不填 alpha（`read_dib` 补的是
//! 0xFF，但任何别的来源都可能留下 0），一旦进了预乘管线，整帧会变成全透明。

use std::rc::Rc;

use crate::core::color::Argb;
use crate::core::draw::DrawList;
use crate::core::error::Result;
use crate::core::geometry::{Rect, ScreenRect};
use crate::core::image::{RgbaBitmap, Shot};

/// 「选区外压暗」的遮罩色与不透明度 —— **两个消费方的单一来源**。
///
/// 消费者是"要把一整块压暗、只留一处亮"的两个叠层：截图区域选择窗（框选时选区外压暗）
/// 与截图编辑画布（`modules::drawing` 的 `ShotEdit` 档）。用户对后者的要求就是
/// *"屏幕其它地方的颜色仍然偏暗，就像在确定选区的时候一样"* —— 既然要求是"一样"，
/// 这两个值就不能各写一份：改一处忘了另一处，用户看到的是"进了编辑态颜色跳了一下"。
///
/// 放在 `core` 是因为消费者分属 `runtime`（区域窗）与 `modules`（标注），
/// 而 **modules 不得依赖 runtime**（分层纪律）。
pub const DIM_COLOR: (u8, u8, u8) = (10, 14, 18);
/// 见 [`DIM_COLOR`]。
pub const DIM_ALPHA: u8 = 110;

/// 叠层底图。`Clone` 只复制 `Rc`（整屏位图不会被复制第二份）。
#[derive(Clone, Default)]
pub enum Backdrop {
    /// 不铺底：叠层窗自身透明，实时桌面可见 —— 屏幕注释（透明幕布）用的就是它。
    #[default]
    None,
    /// 冻结帧：整张虚拟桌面的一次 GDI 抓取。
    Frozen(Rc<RgbaBitmap>),
    /// 冻结帧 + 单色纱。`veil.a() == 255` 的白幕 = 白板；α<255 是"稿纸"变体（预留）。
    Veiled(Rc<RgbaBitmap>, Argb),
}

impl Backdrop {
    /// 抓一帧整虚拟桌面并组底图。`veil = Some(白 α=255)` 得到白板，`None` 得到原样冻结。
    ///
    /// 抓屏失败返回 [`Backdrop::None`]（调用方据此走"兜底命中底"，见
    /// [`Self::is_opaque`]）——抓不到屏幕不该让整个叠层进不去。
    ///
    /// # 为什么 `grab` 是参数而不是直接调 `platform::screenshot::grab`
    ///
    /// **依赖倒置**：core 必须是纯逻辑层，不得依赖 `platform`（否则 review/clippy 之外
    /// 没有任何机制能守住这条线，且 `core` 的可测试性会整个塌掉——单测里连 plot
    /// 一张假帧都做不到）。真正的抓屏由调用方注入，典型写法：
    ///
    /// ```ignore
    /// Backdrop::capture(crate::platform::screenshot::grab, screen, veil_for(mode))
    /// ```
    pub fn capture<F>(grab: F, screen: ScreenRect, veil: Option<Argb>) -> Self
    where
        F: FnOnce(ScreenRect) -> Result<Shot>,
    {
        match grab(screen) {
            Ok(shot) => Self::from_shot(&shot, veil).unwrap_or(Backdrop::None),
            Err(e) => {
                log::warn!("叠层冻结帧抓取失败: {e}");
                Backdrop::None
            }
        }
    }

    /// 由已抓到的一帧组底图（截图链与注释链共用）。尺寸非法返回 `None`。
    pub fn from_shot(shot: &Shot, veil: Option<Argb>) -> Option<Self> {
        let bmp = shot_to_bitmap(shot)?;
        let rc = Rc::new(bmp);
        Some(match veil {
            Some(v) => Backdrop::Veiled(rc, v),
            None => Backdrop::Frozen(rc),
        })
    }

    /// ★★ 这就是用户要的"同一个函数"：把底图铺进 [`DrawList`]（**世界坐标**）。
    ///
    /// 只产生"最多一条 `Image` + 一条 `Fill`"，没有任何其它副作用——所以三个消费方
    /// 可以在各自的画布上放心调它。
    ///
    /// `Veiled` 的纱不透明时**跳过底图 blit**：被完全遮住的像素不值得每帧光栅一次
    /// （白板的每帧成本因此与"硬编码一条白 fill"的旧实现完全一致）。帧仍然持有，
    /// 将来放开 α 就是"稿纸模式"，回退点只有这一个分支。
    pub fn paint_into(&self, list: &mut DrawList, screen: ScreenRect) {
        let dest = Rect {
            x: screen.x as f32,
            y: screen.y as f32,
            w: screen.w as f32,
            h: screen.h as f32,
        };
        match self {
            Backdrop::None => {}
            Backdrop::Frozen(bmp) => {
                list.image(bmp.clone(), dest, 1.0);
            }
            Backdrop::Veiled(bmp, veil) => {
                // `a()` 是 u8，`== 255` 就是"完全不透明"的极限情形（写 `>=` 会被
                // clippy 的 absurd_extreme_comparisons 拦下：另一半永远不成立）。
                if veil.a() == 255 {
                    list.fill_rect(dest, *veil);
                } else {
                    list.image(bmp.clone(), dest, 1.0);
                    list.fill_rect(dest, *veil);
                }
            }
        }
    }

    /// 名下的冻结帧（区域选择窗要拿它做 `WImage` / 放大镜取样）。
    pub fn bitmap(&self) -> Option<&Rc<RgbaBitmap>> {
        match self {
            Backdrop::None => None,
            Backdrop::Frozen(b) | Backdrop::Veiled(b, _) => Some(b),
        }
    }

    /// 画面是否被冻结（决定"进入时抓屏 / 退出时释放"）。
    pub fn is_frozen(&self) -> bool {
        self.bitmap().is_some()
    }

    /// 铺出来是否**完全不透明**。叠层窗按像素 α 做命中测试，铺不满整屏的不透明像素就会
    /// 让点击漏给下层窗口——所以调用方在 `false` 时要补一条 α=1 的"隐形命中底"。
    pub fn is_opaque(&self) -> bool {
        match self {
            Backdrop::None => false,
            Backdrop::Frozen(_) => true,
            Backdrop::Veiled(_, v) => v.a() == 255,
        }
    }
}

/// GDI 帧（BGRA）→ 非预乘 RGBA 位图，**强制 α=255**。
///
/// 全项目唯一一处"帧转位图"（借用 / 消费两版，语义逐字节相同），截图链与注释链都走它，
/// 避免两处各写一次 α 补全。还要继续用原帧的调用方用这一版；只转一次就用
/// [`shot_to_bitmap_owned`]——它省掉一份 `w*h*4` 的分配与拷贝。
///
/// ★ 读**文件**得到的图不许走这一档：见 [`shot_to_bitmap_with_alpha`]。
pub fn shot_to_bitmap(shot: &Shot) -> Option<RgbaBitmap> {
    if shot.w <= 0 || shot.h <= 0 {
        return None;
    }
    let mut rgba = shot.to_rgba();
    for a in rgba.chunks_exact_mut(4) {
        a[3] = 255;
    }
    RgbaBitmap::new(shot.w as u32, shot.h as u32, rgba)
}

/// GDI 帧（BGRA）→ 非预乘 RGBA 位图，**α 原样保留**——读**文件**得到的图走这一档。
///
/// ★ 为什么要两个函数而不是将就一个：[`shot_to_bitmap`] 那两版都会把 α 一律刷成 255，
///   那是**抓屏**要的语义（GDI 交回来的帧 α 恒 0，不补就是一整幅"透明"的黑）。而 PNG 文件里
///   的 α 是内容本身——去背景的结果、圆角图标、GIF 帧全指着它。
///   2026-10-28 给 `ext:load_image` 做对拍（自家编码器写 PNG、自家解码器读回来逐字节比）
///   时暴露：当时顺手复用了 `shot_to_bitmap`，半透明像素全变成实心，读回来那份和写出去的
///   不是同一张图。**同一个 `Shot` 有两种正确语义，就该有两个函数。**
pub fn shot_to_bitmap_alpha(shot: &Shot) -> Option<RgbaBitmap> {
    if shot.w <= 0 || shot.h <= 0 {
        return None;
    }
    RgbaBitmap::new(shot.w as u32, shot.h as u32, shot.to_rgba())
}

/// [`shot_to_bitmap`] 的消费版：在帧自己的缓冲上换序 + 补 α，不再多分配一份。
///
/// 1920×1080 实测 median：借用版 12.1 ms + 8.3 MB 峰值，本版 3.3 ms（n=200；倒序
/// n=100 为 11.2 / 3.5 ms，同号）。抓屏之后就不再需要 BGRA 原帧的调用方（冻结帧、
/// 剪贴板图、标注底图、落码现抓）一律走这里。
pub fn shot_to_bitmap_owned(mut shot: Shot) -> Option<RgbaBitmap> {
    if shot.w <= 0 || shot.h <= 0 {
        return None;
    }
    for px in shot.bgra.chunks_exact_mut(4) {
        px.swap(0, 2);
        px[3] = 255;
    }
    RgbaBitmap::new(shot.w as u32, shot.h as u32, shot.bgra)
}

/// 非预乘 RGBA 位图 → GDI 帧（BGRA，自下而上无关：`Shot` 恒为自上而下）。
pub fn bitmap_to_shot(bmp: &RgbaBitmap) -> Option<Shot> {
    if bmp.w == 0 || bmp.h == 0 {
        return None;
    }
    let mut bgra = Vec::with_capacity(bmp.rgba.len());
    for px in bmp.rgba.chunks_exact(4) {
        bgra.push(px[2]);
        bgra.push(px[1]);
        bgra.push(px[0]);
        bgra.push(px[3]);
    }
    Some(Shot {
        w: bmp.w as i32,
        h: bmp.h as i32,
        bgra,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ★ 同一个 `Shot` 有**两种正确语义**，所以有两档转换，各测一格（混着用就是 bug）：
    /// [`shot_to_bitmap`] 把 α 一律刷成 255——抓屏要的（GDI 交回来的帧 α 恒 0，不补就是
    /// 一整幅"透明"的黑）；[`shot_to_bitmap_alpha`] 保住 α——读文件要的（去背景的结果、
    /// 圆角图标全指着它）。
    ///
    /// 这一格是 2026-10-28 **被对拍逼出来的**：`ext:load_image` 最初顺手用了抓屏那一档，
    /// "自家编码器写 PNG、自家解码器读回来逐字节比"当场红，症状就是半透明像素全变实心。
    /// 颜色通道两档必须一致，否则红的是换序而不是 α——所以两边都断言。
    #[test]
    fn the_two_bitmap_conversions_differ_only_on_alpha() {
        let shot = Shot {
            w: 1,
            h: 2,
            bgra: vec![10, 20, 30, 0, 40, 50, 60, 128],
        };
        let grab = shot_to_bitmap(&shot).expect("抓屏档该转得动");
        let file = shot_to_bitmap_alpha(&shot).expect("文件档该转得动");
        // BGRA → RGBA 的换序在两档里完全一样（R/B 换位、G 不动）。
        assert_eq!(&grab.rgba[..3], &file.rgba[..3], "前两像素的颜色不该分档");
        assert_eq!(&grab.rgba[4..7], &file.rgba[4..7], "后两像素的颜色不该分档");
        assert_eq!(&grab.rgba[0..3], &[30, 20, 10], "BGRA 要换成 RGBA");
        // α 才是两档的唯一区别。
        assert_eq!(grab.rgba[3], 255, "抓屏档补全不透明");
        assert_eq!(
            grab.rgba[7], 255,
            "抓屏档连半透明也刷成 255（这是它的语义）"
        );
        assert_eq!(file.rgba[3], 0, "文件档必须保住透明像素，不然去背景白做");
        assert_eq!(file.rgba[7], 128, "文件档不许动半透明的 α");
    }

    fn bmp(w: u32, h: u32) -> Rc<RgbaBitmap> {
        let rgba = vec![0x80_u8; (w * h * 4) as usize];
        Rc::new(RgbaBitmap::new(w, h, rgba).expect("位图构建失败"))
    }

    fn screen(x: i32, y: i32, w: i32, h: i32) -> ScreenRect {
        ScreenRect { x, y, w, h }
    }

    /// `None` 一条指令都不产生（透明注释的"不铺底"语义）。
    #[test]
    fn none_emits_nothing() {
        let mut list = DrawList::new();
        Backdrop::None.paint_into(&mut list, screen(-1920, 0, 3840, 1080));
        assert_eq!(list.cmds().len(), 0);
        assert!(!Backdrop::None.is_frozen());
        assert!(!Backdrop::None.is_opaque());
    }

    /// `Frozen` 恰好一条 Image，且 dest 是**整虚拟桌面**（含负原点）。
    #[test]
    fn frozen_emits_one_image_at_the_virtual_screen() {
        let mut list = DrawList::new();
        Backdrop::Frozen(bmp(4, 3)).paint_into(&mut list, screen(-1920, -100, 3840, 1180));
        assert_eq!(list.cmds().len(), 1);
        match &list.cmds()[0] {
            crate::core::draw::DrawCmd::Image { dest, .. } => {
                assert_eq!(
                    (dest.x, dest.y, dest.w, dest.h),
                    (-1920.0, -100.0, 3840.0, 1180.0)
                );
            }
            other => panic!("应为 Image，实际 {other:?}"),
        }
        assert!(Backdrop::Frozen(bmp(1, 1)).is_opaque());
    }

    /// 不透明纱（白板）：**只**一条 Fill，底图被完全遮住不值得再光栅一次。
    #[test]
    fn opaque_veil_skips_the_frame_blit() {
        let mut list = DrawList::new();
        Backdrop::Veiled(bmp(4, 3), Argb::new(255, 255, 255, 255))
            .paint_into(&mut list, screen(0, 0, 100, 80));
        assert_eq!(list.cmds().len(), 1, "不透明纱应只有一条 Fill");
        assert!(matches!(
            list.cmds()[0],
            crate::core::draw::DrawCmd::Fill { .. }
        ));
        assert!(Backdrop::Veiled(bmp(1, 1), Argb::new(255, 255, 255, 255)).is_opaque());
    }

    /// 半透明纱（稿纸）：先 Image 再 Fill，两条都有。
    #[test]
    fn translucent_veil_keeps_the_frame_under_it() {
        let mut list = DrawList::new();
        let b = Backdrop::Veiled(bmp(4, 3), Argb::new(128, 255, 255, 255));
        b.paint_into(&mut list, screen(0, 0, 100, 80));
        assert_eq!(list.cmds().len(), 2);
        assert!(matches!(
            list.cmds()[0],
            crate::core::draw::DrawCmd::Image { .. }
        ));
        assert!(matches!(
            list.cmds()[1],
            crate::core::draw::DrawCmd::Fill { .. }
        ));
        assert!(!b.is_opaque(), "半透明纱不遮挡命中测试，调用方要补命中底");
        assert!(b.is_frozen());
    }

    /// 帧 → 位图：α 一律 255（GDI 不填 alpha，漏了这步整帧会变透明）。
    #[test]
    fn shot_to_bitmap_forces_opaque_alpha() {
        let shot = Shot {
            w: 2,
            h: 1,
            bgra: vec![0x11, 0x22, 0x33, 0x00, 0x44, 0x55, 0x66, 0x00],
        };
        let b = shot_to_bitmap(&shot).expect("应能转换");
        assert_eq!(b.rgba, vec![0x33, 0x22, 0x11, 255, 0x66, 0x55, 0x44, 255]);
        // 往返回来只差 α：B 与 R 换两次回到原处，α 则**回不去**（源数据本来是 0，
        // 被强制成了 255——这正是上面那条断言要证明的事）。所以期望值是"原帧 + α 补满"。
        let back = bitmap_to_shot(&b).expect("应能回转");
        let mut expect = shot.bgra.clone();
        for a in expect.chunks_exact_mut(4) {
            a[3] = 255;
        }
        assert_eq!(back.bgra, expect, "往返应只改 α");
        assert_eq!((back.w, back.h), (shot.w, shot.h));
    }

    #[test]
    fn degenerate_sizes_are_rejected() {
        assert!(shot_to_bitmap(&Shot {
            w: 0,
            h: 10,
            bgra: vec![]
        })
        .is_none());
        assert!(shot_to_bitmap_owned(Shot {
            w: 0,
            h: 10,
            bgra: vec![]
        })
        .is_none());
    }

    /// 消费版与借用版必须逐字节一致：漏换序、漏补 α、复用错缓冲都会红。
    #[test]
    fn owned_conversion_matches_borrowed() {
        let shot = Shot {
            w: 2,
            h: 2,
            bgra: vec![
                0x11, 0x22, 0x33, 0x00, 0x44, 0x55, 0x66, 0x01, 0x77, 0x88, 0x99, 0x02, 0xAA, 0xBB,
                0xCC, 0x03,
            ],
        };
        let borrowed = shot_to_bitmap(&shot).expect("借用版");
        let owned = shot_to_bitmap_owned(shot).expect("消费版");
        assert_eq!(owned, borrowed);
        assert_eq!(
            &owned.rgba[..8],
            &[0x33, 0x22, 0x11, 255, 0x66, 0x55, 0x44, 255],
            "换序 + 补 α 都要在场"
        );
    }
}
