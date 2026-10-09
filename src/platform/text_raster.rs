//! 文本 → 位图（GDI `DrawTextW` + 32bpp DIB）。
//!
//! 「贴图快捷键」在剪贴板里看到的是**文本**时，用户要的是一张能钉在桌面上的图片
//! （原始需求："剪贴板文字转 jpg/png 图片"）。文本 → 位图这一步只能交给系统：
//! 我们自己的光栅化是 CPU 软渲染、没有字形加载器，而 GDI 的字体栈是随系统就有的，
//! 中文/CJK/emoji 的回退链也是现成的——不引入任何新依赖。
//!
//! 产出的是一帧 [`Shot`]（BGRA、自上而下、α=255），于是它能直接进既有的整条链：
//! [`crate::platform::screenshot::to_clipboard`] 写剪贴板、
//! [`crate::platform::screenshot::save_png`] 存盘、
//! [`crate::core::backdrop::shot_to_bitmap`] 转非预乘位图给贴图窗显示。
//!
//! # 为什么是白底黑字
//!
//! 贴图窗显示图片时是"所见即所得"的 1:1 像素，没有背景色可配。白底黑字等价于
//! "把这段文字当截图看"，粘到任何文档/聊天窗口里都不会变成黑块——这也是用户在
//! 剪贴板里复制文字再粘贴到画图板时系统给的观感。

use windows::core::HSTRING;
use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, DrawTextW,
    GetTextExtentPoint32W, SelectObject, SetBkMode, SetTextColor, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS,
    DT_CALCRECT, DT_LEFT, DT_NOPREFIX, DT_TOP, DT_WORDBREAK, FF_DONTCARE, FW_NORMAL, OUT_TT_PRECIS,
    TRANSPARENT,
};

use super::screenshot::Shot;

/// 正文字号（逻辑像素，实际按 DPI 放大）。
const FONT_PX: i32 = 16;
/// 内边距（逻辑像素，实际按 DPI 放大）。
const PAD: i32 = 12;
/// 换行宽度下限：太窄的框换出来只有一两个汉字宽，不如给个地板值。
const MIN_WRAP: i32 = 120;
/// 贴图窗文本图片的最大宽度上限（逻辑像素）。
pub const MAX_W_LOGICAL: i32 = 520;

/// 把文本渲染成白底黑字的位图帧。
///
/// `max_w` 为**物理像素**下的最大宽度（贴图窗按 90% 屏宽算出来传进来）。
///
/// 宽度按内容收紧（2026-10-09 用户报"贴图太宽"）：先逐行量文本的**自然宽度**
/// （`GetTextExtentPoint32W`，不换行），最长行放得下就按它收窄贴图——"你好"不再
/// 撑成 520px 的横条；放不下才把换行宽度夹到 `max_w - 2*PAD`（不低于 [`MIN_WRAP`]），
/// 由 `DrawTextW` 的 `DT_WORDBREAK` 换行。空串返回 `None`（调用方提示"没有可贴的内容"）。
pub fn render_text(text: &str, max_w: i32, scale: f32) -> Option<Shot> {
    let text = text.trim_end_matches(['\r', '\n']);
    if text.trim().is_empty() {
        return None;
    }
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let font_px = ((FONT_PX as f32) * scale).round().max(8.0) as i32;
    let pad = ((PAD as f32) * scale).round().max(2.0) as i32;
    let max_wrap = (max_w - pad * 2).max(((MIN_WRAP as f32) * scale).round() as i32);

    // ★ 不补 NUL：`DrawTextW` 的 `&[u16]` 长度**就是** cchtext。
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    if wide.is_empty() {
        return None;
    }

    unsafe {
        let dc = CreateCompatibleDC(None);
        if dc.is_invalid() {
            return None;
        }
        // 字体名用中文 Windows 的默认 UI 字体；找不到时 GDI 会按字体链回退，不会失败。
        let face = HSTRING::from("Microsoft YaHei UI");
        let font = CreateFontW(
            -font_px,
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_TT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
            &face,
        );
        let old_font = SelectObject(dc, font.into());
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, COLORREF(0x0000_0000)); // COLORREF = 0x00BBGGRR，全零即黑

        // ⓪ 量自然宽度：按显式 `\n` 拆行，取最长行的像素宽（不含换行）。
        //    全部行都 ≤ max_wrap 时按自然宽度收窄贴图，不再强撑满宽。
        let natural_w = {
            let mut m = 0i32;
            for line in text.split('\n') {
                let w16: Vec<u16> = line.encode_utf16().collect();
                if w16.is_empty() {
                    continue;
                }
                let mut size = windows::Win32::Foundation::SIZE::default();
                let _ = GetTextExtentPoint32W(dc, &w16, &mut size);
                m = m.max(size.cx);
            }
            m
        };
        let wrap_w = if natural_w > 0 && natural_w <= max_wrap {
            natural_w
        } else {
            max_wrap
        };

        // ① 量高：DT_CALCRECT 用给定的 right 作为换行宽度，回填 bottom。
        let mut probe = RECT {
            left: 0,
            top: 0,
            right: wrap_w,
            bottom: 0,
        };
        let measured = DrawTextW(
            dc,
            &mut wide,
            &mut probe,
            DT_CALCRECT | DT_WORDBREAK | DT_LEFT | DT_TOP | DT_NOPREFIX,
        );
        let text_h = measured.max(1);
        let (w, h) = (wrap_w + pad * 2, text_h + pad * 2);
        if w <= 0 || h <= 0 || w > 16384 || h > 16384 {
            SelectObject(dc, old_font);
            let _ = DeleteObject(font.into());
            let _ = DeleteDC(dc);
            return None;
        }

        // ② 画进一块 32bpp 自顶向下 DIB（负 biHeight = 自上而下，省一次行序翻转）。
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                biSizeImage: (w as u32) * (h as u32) * 4,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bmp = match CreateDIBSection(Some(dc), &bi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(b) => b,
            Err(_) => {
                SelectObject(dc, old_font);
                let _ = DeleteObject(font.into());
                let _ = DeleteDC(dc);
                return None;
            }
        };
        let old_bmp = SelectObject(dc, bmp.into());

        // 白底（GDI 不保证填 alpha，最后统一补 255）
        let total = (w as usize) * (h as usize) * 4;
        std::ptr::write_bytes(bits as *mut u8, 0xFF, total);

        let mut rc = RECT {
            left: pad,
            top: pad,
            right: pad + wrap_w,
            bottom: pad + text_h,
        };
        DrawTextW(
            dc,
            &mut wide,
            &mut rc,
            DT_WORDBREAK | DT_LEFT | DT_TOP | DT_NOPREFIX,
        );

        // ③ 取回像素（BGRA，自上而下）。
        let mut bgra = vec![0_u8; total];
        std::ptr::copy_nonoverlapping(bits as *const u8, bgra.as_mut_ptr(), total);
        for px in bgra.chunks_exact_mut(4) {
            px[3] = 255;
        }

        SelectObject(dc, old_bmp);
        let _ = DeleteObject(bmp.into());
        SelectObject(dc, old_font);
        let _ = DeleteObject(font.into());
        let _ = DeleteDC(dc);

        Some(Shot { w, h, bgra })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn single_line_text_has_non_zero_size() {
        let s = render_text("你好，世界", 800, 1.0).expect("应能渲染");
        assert!(s.w > 40 && s.h > 16, "尺寸不合理：{}×{}", s.w, s.h);
        assert_eq!(s.bgra.len(), (s.w as usize) * (s.h as usize) * 4);
        assert!(s.bgra.chunks_exact(4).all(|p| p[3] == 255), "α 必须恒 255");
    }

    /// 长文本必须换行：高度明显大于单行，宽度不超上限。
    #[cfg(windows)]
    #[test]
    fn long_text_wraps_within_the_max_width() {
        let long = "这是一段很长的文本，用于验证自动换行是否生效。".repeat(6);
        let s = render_text(&long, 400, 1.0).expect("应能渲染");
        assert!(s.w <= 400, "宽度超上限：{}", s.w);
        assert!(s.h > 60, "长文本应换行（高度 {} 太小）", s.h);
    }

    /// 短文本按内容收窄：不再强撑到上限宽（2026-10-09 用户报"贴图太宽"）。
    #[cfg(windows)]
    #[test]
    fn short_text_fits_its_content_not_the_max_width() {
        let s = render_text("你好", 800, 1.0).expect("应能渲染");
        // "你好"两个字 16px 字号 ≈ 32px + 内边距 24 ≈ 56px；绝不该是 800 或 520 的横条。
        assert!(s.w < 200, "短文本贴图太宽：{}", s.w);
        // 但也不该窄到把字截了（字宽 + 2*内边距）。
        assert!(s.w >= 32 + 2 * PAD, "贴图比文本还窄：{}", s.w);
    }

    /// 空串 / 全空白返回 None（调用方据此提示"没有可贴的内容"）。
    #[cfg(windows)]
    #[test]
    fn blank_text_is_rejected() {
        assert!(render_text("", 400, 1.0).is_none());
        assert!(render_text("   \n  ", 400, 1.0).is_none());
    }

    /// 显式 `\n` 换行要生效（多一段就多一行高）。
    #[cfg(windows)]
    #[test]
    fn explicit_newlines_add_lines() {
        let one = render_text("第一行", 400, 1.0).expect("应能渲染");
        let two = render_text("第一行\n第二行\n第三行", 400, 1.0).expect("应能渲染");
        assert!(two.h > one.h, "三行应比一行高：{} vs {}", two.h, one.h);
    }

    /// 白底黑字：左上角内边距处应是白的（没有任何字形）。
    #[cfg(windows)]
    #[test]
    fn background_is_white() {
        let s = render_text("abc", 400, 1.0).expect("应能渲染");
        let p = &s.bgra[0..4];
        assert_eq!((p[0], p[1], p[2]), (255, 255, 255), "左上角应是白底");
    }
}
