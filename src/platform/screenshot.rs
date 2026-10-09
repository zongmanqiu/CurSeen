//! 屏幕截图（GDI）。
//!
//! # 为什么截图用 GDI 而录屏用 DXGI
//!
//! 两者要的东西不一样：
//! - **截图**是一次性的、低频的、要的是"简单可靠"——`BitBlt` 一次抓完，三十行，不需要
//!   D3D 设备、不需要处理 `DXGI_ERROR_ACCESS_LOST` 重建，多显示器与负坐标原点也天然支持。
//! - **录屏**是每秒几十次的持续负载，DXGI Desktop Duplication 能"只在屏幕变化时给帧"
//!   且帧可以直接作为 D3D 纹理喂给硬件编码器，无需每帧 `Map`/`Unmap` 拷回内存。
//!
//! 所以两条路各用各的，不强求统一后端——统一只会让截图路径背上一整套 D3D 生命周期。
//!
//! # 坐标
//!
//! 进程是 Per-Monitor DPI Aware v2（windui 启动时设置，见
//! `.3rdParty/wind-ui-rust-0.16.1/src/platform/win32/mod.rs`），因此 `GetSystemMetrics` /
//! `EnumDisplayMonitors` 给出的就是**物理像素**，与 [`ScreenRect`] 同一坐标系，
//! 可以直接传给 `BitBlt`，不需要再乘 DPI。

use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    GetMonitorInfoW, MonitorFromPoint, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    CAPTUREBLT, DIB_RGB_COLORS, HBITMAP, HDC, MONITORINFO, MONITOR_DEFAULTTOPRIMARY, ROP_CODE,
    SRCCOPY,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GHND};

use crate::core::error::{CurSeenError, Result};
use crate::platform::display::ScreenRect;

/// [`Shot`] 的真身已下沉到 `core::image`（它是纯数据：BGRA + 尺寸，无 Win32 调用）。
/// 这里重导出，让既有 `platform::screenshot::Shot` 的引用继续成立——只有**抓**它的
/// [`grab`] 属于本层。
pub use crate::core::image::Shot;

/// 剪贴板格式 `CF_DIB`。
///
/// 定义在 `Win32_System_Ole` 下（值 8），为一个常量开一整个 feature 不值，本地钉住。
const CF_DIB: u32 = 8;

/// `SRCCOPY | CAPTUREBLT`：windows crate 只导出 `SRCCOPY`，合成值自己算。
const SRCCOPY_CAPTUREBLT: ROP_CODE = ROP_CODE(SRCCOPY.0 | CAPTUREBLT.0);

/// 抓一块虚拟桌面区域（**物理像素**坐标，原点可以是负的）。
///
/// `CAPTUREBLT` 是必需的：屏幕上任何靠 `UpdateLayeredWindow` 提交的内容（**包括
/// CurSeen 自己的全屏覆盖层**，以及其它用分层窗的软件），不带它都抓不全——2026-09-20
/// 的实测细节见下面"别再试'干净抓屏'那套"。因此凡是"要把屏幕上真有的东西都录/截下来"
/// 的场景，一律走这一个函数。
///
/// # ⚠ 别再试"干净抓屏"那套（2026-09-20 实测，别重复研究一遍）
///
/// 本项目一度有一条 `grab_clean`（纯 `SRCCOPY`、不带 `CAPTUREBLT`），给画中画放大用，
/// 主张"分层窗口默认不被 `BitBlt` 捕获，于是自身覆盖层天然被排除"。**这个前提是错的**：
/// 造一个 `UpdateLayeredWindow` 提交的分层窗压屏幕上，`SRCCOPY` 与
/// `SRCCOPY|CAPTUREBLT` 抓下来的标记色占比**都是 100.00%**——两者毫无区别，分层窗
/// 照样被抓进来。放大镜因此把自己上一帧的面板抓进源图再放大，就是用户报的
/// "画中画无限套娃 / 调倍率面板里没放大"。那条 API 已删。
///
/// 真正可靠的自我排除是 `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`（同一次
/// 实测把两种 rop 都压到 **0.00%**），它按**窗口**生效，见 `platform::pip_window`。
/// 实测数据与探针在 `tests/pip_probe.rs`。
pub fn grab(rect: ScreenRect) -> Result<Shot> {
    grab_with(rect, SRCCOPY_CAPTUREBLT)
}

/// 抓一张**藏掉某个窗口之后**的桌面，抓完立刻把它放回来。
///
/// ★ 为什么要用它：透明幕布（屏幕标注不再冻结屏幕）出图时，背景只能现抓一张桌面 ——
///   而我们的覆盖层此刻正铺满整屏，直接抓会**把工具栏一起拍进产物**。藏 → 抓 → 显是
///   一次同步操作（几十毫秒），且发生在"会话马上结束"的那一瞬。
///
/// 找不到那个窗口就按普通抓屏处理（不藏不显）；抓屏失败照旧返回 `Err`。
/// 用 `SW_SHOWNOACTIVATE` 放回来：出图不该顺手抢一次前台焦点。
pub fn grab_hiding(title: &str, rect: ScreenRect) -> Result<Shot> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, ShowWindow, SW_HIDE, SW_SHOWNOACTIVATE,
    };
    let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let hwnd = unsafe { FindWindowW(None, PCWSTR(wide.as_ptr())) };
    let hwnd = match hwnd {
        Ok(h) if !h.is_invalid() => Some(h),
        _ => None,
    };
    if let Some(h) = hwnd {
        unsafe {
            let _ = ShowWindow(h, SW_HIDE);
        }
    }
    let shot = grab(rect);
    if let Some(h) = hwnd {
        unsafe {
            let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
        }
    }
    shot
}

/// 抓屏主体：`rop` 决定是否包含分层窗口（无 `CAPTUREBLT` == 干净源图）。
fn grab_with(rect: ScreenRect, rop: ROP_CODE) -> Result<Shot> {
    let w = rect.w.max(1);
    let h = rect.h.max(1);
    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            return Err(CurSeenError::Unsupported("GetDC 失败"));
        }
        let mem = CreateCompatibleDC(Some(screen));
        if mem.is_invalid() {
            ReleaseDc(None, screen);
            return Err(CurSeenError::Unsupported("CreateCompatibleDC 失败"));
        }
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let grab_result = if bmp.is_invalid() {
            Err(CurSeenError::Unsupported("CreateCompatibleBitmap 失败"))
        } else {
            let old = SelectObject(mem, bmp.into());
            // 源坐标是虚拟桌面坐标，直接带负数传给 BitBlt（GDI 接受负源坐标）。
            let ok = BitBlt(mem, 0, 0, w, h, Some(screen), rect.x, rect.y, rop);
            let pixels = if ok.is_ok() {
                read_dib(mem, bmp, w, h)
            } else {
                Err(CurSeenError::Unsupported("BitBlt 失败"))
            };
            SelectObject(mem, old);
            pixels
        };
        if !bmp.is_invalid() {
            let _ = DeleteObject(bmp.into());
        }
        let _ = DeleteDC(mem);
        ReleaseDc(None, screen);
        grab_result
    }
}

/// 当前鼠标所在的那一整块显示器的矩形（**物理像素**，虚拟桌面坐标）。
///
/// 从 `grab_monitor_at` 拆出来单独可用：录屏的"当前窗口所在显示器"模式要的是矩形，
/// 不需要真去抓一帧。
pub fn monitor_rect_at(x: i32, y: i32) -> ScreenRect {
    let p = POINT { x, y };
    let monitor = unsafe { MonitorFromPoint(p, MONITOR_DEFAULTTOPRIMARY) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        ScreenRect {
            x: info.rcMonitor.left,
            y: info.rcMonitor.top,
            w: info.rcMonitor.right - info.rcMonitor.left,
            h: info.rcMonitor.bottom - info.rcMonitor.top,
        }
    } else {
        // 取不到就退回主显示器整体，绝不返回退化矩形（那会让截图裁到 0×0）。
        crate::platform::display::virtual_screen()
    }
}

/// 抓当前鼠标所在的那一整块显示器。
pub fn grab_monitor_at(x: i32, y: i32) -> Result<Shot> {
    grab(monitor_rect_at(x, y))
}

/// 把兼容位图里的像素取回来（BGRA，自上而下）。
fn read_dib(dc: HDC, bmp: HBITMAP, w: i32, h: i32) -> Result<Shot> {
    let mut bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            // 负值 = 自上而下（Top-Down DIB），省掉一次行序翻转。
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
    let rows = unsafe {
        GetDIBits(
            dc,
            bmp,
            0,
            h as u32,
            Some(buf.as_mut_ptr().cast()),
            &mut bi,
            DIB_RGB_COLORS,
        )
    };
    if rows != h {
        return Err(CurSeenError::Unsupported("GetDIBits 行数不符"));
    }
    // GDI 给的是 BGRA（内存序 B,G,R,X），alpha 补满。
    for px in buf.chunks_exact_mut(4) {
        px[3] = 0xFF;
    }
    Ok(Shot { w, h, bgra: buf })
}

/// PNG 编码。压缩级别取 `Default`：截图是低频操作，多花两百毫秒换个明显更小的文件划算。
pub fn encode_png(shot: &Shot) -> Result<Vec<u8>> {
    use png::{BitDepth, ColorType, Encoder};
    let mut out = Vec::new();
    {
        let mut enc = Encoder::new(&mut out, shot.w as u32, shot.h as u32);
        enc.set_color(ColorType::Rgba);
        enc.set_depth(BitDepth::Eight);
        let mut writer = enc
            .write_header()
            .map_err(|e| CurSeenError::Unsupported(box_str(&format!("PNG 头: {e}"))))?;
        writer
            .write_image_data(&shot.to_rgba())
            .map_err(|e| CurSeenError::Unsupported(box_str(&format!("PNG 数据: {e}"))))?;
    }
    Ok(out)
}

/// 存成 PNG 文件（父目录不存在则创建）。
pub fn save_png(shot: &Shot, path: &std::path::Path) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let bytes = encode_png(shot)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

/// 把一帧放到剪贴板（CF_DIB）。
///
/// 用 CF_DIB 而不是 PNG：前者是画图板 / 微信 / Word / 浏览器全都认的通用位图格式，
/// 后者只有部分应用注册了。`CF_DIB` 要求**自下而上**的行序，所以这里要把行倒过来写。
pub fn to_clipboard(shot: &Shot) -> Result<()> {
    let w = shot.w;
    let h = shot.h;
    // ★★ 不变量先钉死，再碰 `unsafe`。`Shot` 是**公开且外部可构造**的结构（PNG 解码、
    //    缩略图缩放、剪贴板回读都会造一个），一旦 `bgra` 比 `w*h*4` 短，下面那段逐行
    //    拷贝就会读越界——在 `unsafe` 里越界就是 0xC0000005，且发布版无符号、现场只留
    //    一句"程序没了"（2026-09-29 剪贴板"复制置顶"一按即崩）。宁可这一笔不写。
    if w <= 0 || h <= 0 {
        return Err(CurSeenError::Unsupported("Shot 尺寸非法"));
    }
    let stride = (w as usize) * 4;
    if shot.bgra.len() < stride * (h as usize) {
        return Err(CurSeenError::Unsupported("Shot 像素数据短于尺寸声明"));
    }
    let header = std::mem::size_of::<BITMAPINFOHEADER>();
    let total = header + stride * (h as usize);
    unsafe {
        let hg =
            GlobalAlloc(GHND, total).map_err(|_| CurSeenError::Unsupported("GlobalAlloc 失败"))?;
        if hg.is_invalid() {
            return Err(CurSeenError::Unsupported("GlobalAlloc 失败"));
        }
        let ptr = GlobalLock(hg);
        if ptr.is_null() {
            let _ = windows::Win32::Foundation::GlobalFree(Some(hg));
            return Err(CurSeenError::Unsupported("GlobalLock 失败"));
        }
        {
            let dst = std::slice::from_raw_parts_mut(ptr.cast::<u8>(), total);
            let bi = dst.as_mut_ptr().cast::<BITMAPINFOHEADER>();
            std::ptr::write(
                bi,
                BITMAPINFOHEADER {
                    biSize: header as u32,
                    biWidth: w,
                    biHeight: h, // 正 = 自下而上
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    biSizeImage: (stride * h as usize) as u32,
                    ..Default::default()
                },
            );
            for row in 0..h as usize {
                let src = &shot.bgra[(h as usize - 1 - row) * stride..][..stride];
                dst[header + row * stride..header + (row + 1) * stride].copy_from_slice(src);
            }
        }
        let _ = GlobalUnlock(hg);

        if OpenClipboard(None).is_ok() {
            let _ = EmptyClipboard();
            // `SetClipboardData` 成功后剪贴板接管内存，不能再 GlobalFree。
            let r = SetClipboardData(CF_DIB, Some(windows::Win32::Foundation::HANDLE(hg.0)));
            let _ = CloseClipboard();
            if r.is_err() {
                let _ = windows::Win32::Foundation::GlobalFree(Some(hg));
                return Err(CurSeenError::Unsupported("SetClipboardData 失败"));
            }
        } else {
            let _ = windows::Win32::Foundation::GlobalFree(Some(hg)); // OpenClipboard 失败，内存仍归我们，必须释放
            return Err(CurSeenError::Unsupported("OpenClipboard 失败"));
        }
    }
    Ok(())
}

/// `CurSeenError::Unsupported` 只吃 `&'static str`，把动态串漏成静态串。
///
/// PNG 编码失败不是能恢复的错误（重试也不会变好），泄漏一个短串可以接受；
/// 真要频繁失败也只会有几条（错误种类有限），不会无界增长。
fn box_str(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

/// `ReleaseDC` 在 windows crate 里位于 `Win32_Graphics_Gdi`。
use windows::Win32::Graphics::Gdi::ReleaseDC as ReleaseDc;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_swaps_red_and_blue() {
        let s = Shot {
            w: 1,
            h: 1,
            bgra: vec![0x11, 0x22, 0x33, 0xFF],
        };
        assert_eq!(s.to_rgba(), vec![0x33, 0x22, 0x11, 0xFF]);
    }

    #[test]
    fn png_roundtrip_has_signature() {
        let s = Shot {
            w: 2,
            h: 1,
            bgra: vec![0x11, 0x22, 0x33, 0xFF, 0x44, 0x55, 0x66, 0xFF],
        };
        let bytes = encode_png(&s).expect("PNG 应编码成功");
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        assert!(bytes.len() > 40);
    }

    #[cfg(windows)]
    #[test]
    fn grab_virtual_screen_is_non_empty() {
        let r = crate::platform::display::virtual_screen();
        let shot = grab(r).expect("应能抓到屏幕");
        assert_eq!(shot.w, r.w);
        assert_eq!(shot.h, r.h);
        assert_eq!(shot.bgra.len(), (r.w as usize) * (r.h as usize) * 4);
    }
}
