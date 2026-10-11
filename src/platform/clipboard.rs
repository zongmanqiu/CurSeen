//! 系统剪贴板读取（文本 / 图像）——贴图功能的输入源。
//!
//! 只读不写：写入已有 `screenshot::to_clipboard`（CF_DIB）。读取走标准剪贴板 API：
//! 文本取 `CF_UNICODETEXT`（13）；图像取 `CF_DIB`（8，BITMAPINFOHEADER + 像素数据，
//! 自下而上 BGRA 行序是主流来源——截图工具/浏览器复制图片基本都是它）。
//!
//! 所有函数失败一律返回 `None`（剪贴板被别的进程占用、格式不符、空剪贴板都是常态），
//! 调用方（贴图快捷键）据此回落到贴图历史环。
//!
//! 剪贴板格式常量沿用 `screenshot.rs` 的本地钉住惯例（为一个常量开一整个
//! `Win32_System_Ole` feature 不值）：CF_DIB=8、CF_UNICODETEXT=13。

use windows::Win32::Foundation::HGLOBAL;
use windows::Win32::System::DataExchange::{
    CloseClipboard, GetClipboardData, GetClipboardSequenceNumber, IsClipboardFormatAvailable,
    OpenClipboard,
};
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};

use super::screenshot::Shot;

const CF_DIB: u32 = 8;
const CF_UNICODETEXT: u32 = 13;

/// 剪贴板**内容序列号**（系统维护，每次内容变化 +1）。
///
/// 用它决定"要不要去读剪贴板"：只读一个 u32，比读一次图文内容便宜几个数量级，
/// 因此可以挂在既有的 250ms 对账上而不付任何代价。序列号没变就直接跳过。
///
/// ★ 这是"最多支持 pin 最近 10 个内容"能名副其实的前提：贴图快捷键只在**按键那一瞬**
/// 采样剪贴板，两次按键之间复制的内容全都会丢。有了序列号轮询，任何一次复制都会
/// 在 250ms 内被收进贴图环。
pub fn sequence_number() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

/// RAII：离开作用域时关剪贴板。
struct Closer;

impl Drop for Closer {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

/// RAII：离开作用域时 GlobalUnlock。
struct Unlocker(*mut u8);

impl Drop for Unlocker {
    fn drop(&mut self) {
        unsafe {
            let _ = GlobalUnlock(HGLOBAL(self.0 as *mut _));
        }
    }
}

/// 读取剪贴板文本（CF_UNICODETEXT）。
pub fn get_text() -> Option<String> {
    unsafe {
        OpenClipboard(None).ok()?;
        let _guard = Closer;
        if IsClipboardFormatAvailable(CF_UNICODETEXT).is_err() {
            return None;
        }
        let h = GetClipboardData(CF_UNICODETEXT).ok()?;
        let ptr = GlobalLock(HGLOBAL(h.0));
        if ptr.is_null() {
            return None;
        }
        let _guard_mem = Unlocker(ptr.cast());
        // 到双零结尾的 UTF-16 长度。★ 以 GlobalSize 为上界：剪贴板数据的结尾**由
        // 来源进程负责放 NUL**，畸形来源（或不规范的自定义写入方）不给终止符时，
        // 无界扫描会一路读出分配区之外——正好就是 0xC0000005 读越界（2026-10-11
        // 崩溃排查时定的防御项）。越界就按"到此为止"截断，交给 lossy 转换兜底。
        let total = GlobalSize(HGLOBAL(h.0)) as usize;
        let max_chars = total / 2;
        let mut len = 0_usize;
        let u16_ptr = ptr.cast::<u16>();
        while len < max_chars && *u16_ptr.add(len) != 0 {
            len += 1;
        }
        let slice = std::slice::from_raw_parts(u16_ptr, len);
        let text = String::from_utf16_lossy(slice);
        let text = text.trim_end_matches(['\r', '\n', ' ']).trim().to_string();
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }
}

/// 读取剪贴板图像（CF_DIB）。
///
/// 支持 32bpp（BI_RGB / BI_BITFIELDS，屏幕截图主流）与 24bpp；自下而上或
/// `biHeight<0` 的自上而下都处理。返回的 `Shot` 已转为自上而下 BGRA、alpha 强制 255
/// （DIB 无 alpha 通道——与 GDI 抓屏同款陷阱，见 tests/freeze_overlay_poc.rs）。
pub fn get_image() -> Option<Shot> {
    unsafe {
        OpenClipboard(None).ok()?;
        let _guard = Closer;
        if IsClipboardFormatAvailable(CF_DIB).is_err() {
            return None;
        }
        let hmem = GetClipboardData(CF_DIB).ok()?;
        let ptr = GlobalLock(HGLOBAL(hmem.0));
        if ptr.is_null() {
            return None;
        }
        let _guard_mem = Unlocker(ptr.cast());
        // BITMAPINFOHEADER：biSize(4) biWidth(4) biHeight(4, 有符号) planes+bpp(4) compression(4)
        // ★ 先取分配大小并保证装得下 20 字节的头字段，再解头——畸形 DIB（分配区比头还小）
        //   直接拒绝，否则下面 5 个 u32 的读取本身就是越界（2026-10-11 崩溃排查防御项）。
        let total = GlobalSize(HGLOBAL(hmem.0)) as usize;
        if total < 20 {
            return None;
        }
        let base = ptr.cast::<u32>();
        let bi_size = *base as usize;
        let w = *base.add(1) as i32;
        let h_raw = *base.add(2) as i32;
        let bpp = (*base.add(3) >> 16) as u32;
        let compression = *base.add(4);
        if w <= 0 || w > 16384 || h_raw == 0 || h_raw.abs() > 16384 {
            return None;
        }
        let top_down = h_raw < 0;
        let h = h_raw.abs();
        // BI_RGB(0) 与 BI_BITFIELDS(3) 都按无压缩像素走；其余压缩一律不认。
        if !(compression == 0 || compression == 3) {
            return None;
        }
        // ★ 像素起点：BITMAPINFOHEADER(40) + BI_BITFIELDS 的 3 个 DWORD 掩码(12)；
        //   但 BITMAPV4/V5 头(108/124) 已把掩码收进头内，不能再 +12（否则整张图错 12 字节）。
        if !(40..=124).contains(&bi_size) {
            return None;
        }
        let pixel_offset = if bi_size >= 108 {
            bi_size
        } else if compression == 3 {
            bi_size + 12
        } else {
            bi_size
        };
        // 像素区不可超出分配大小，畸形头会越界读（UB）。
        if pixel_offset + (h as usize) * ((w as usize) * 4) > total {
            return None;
        }
        match bpp {
            32 => {
                // 像素紧跟在头（+ BI_BITFIELDS 的 3 个 DWORD 掩码）之后，32bpp 天然 4 字节对齐。
                let px = (ptr.cast::<u8>()).add(bi_size + mask_bytes(compression));
                let stride = (w as usize) * 4;
                let mut bgra = vec![0_u8; (w as usize) * (h as usize) * 4];
                for y in 0..h as usize {
                    let src_row = if top_down { y } else { h as usize - 1 - y };
                    let row = std::slice::from_raw_parts(px.add(src_row * stride), stride);
                    bgra[y * stride..(y + 1) * stride].copy_from_slice(row);
                }
                for a in bgra.chunks_exact_mut(4) {
                    a[3] = 255;
                }
                Some(Shot { w, h, bgra })
            }
            24 => {
                let stride = ((w as usize) * 3).div_ceil(4) * 4;
                let px = (ptr.cast::<u8>()).add(bi_size);
                let mut bgra = vec![0_u8; (w as usize) * (h as usize) * 4];
                for y in 0..h as usize {
                    let src_row = if top_down { y } else { h as usize - 1 - y };
                    for x in 0..w as usize {
                        let s = src_row * stride + x * 3;
                        let d = y * (w as usize) * 4 + x * 4;
                        bgra[d] = *px.add(s);
                        bgra[d + 1] = *px.add(s + 1);
                        bgra[d + 2] = *px.add(s + 2);
                        bgra[d + 3] = 255;
                    }
                }
                Some(Shot { w, h, bgra })
            }
            _ => None,
        }
    }
}

/// BI_BITFIELDS 的颜色掩码紧跟在 BITMAPINFOHEADER 后（3 个 DWORD = 12 字节，部分写
/// 4 个 = 16 字节）；BI_RGB 没有。掩码区内按 32bpp 通路解（标准来源都是 BGRA 排列）。
fn mask_bytes(compression: u32) -> usize {
    if compression == 3 {
        12
    } else {
        0
    }
}
