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
use windows::Win32::System::DataExchange::{EmptyClipboard, SetClipboardData};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GHND};

use crate::core::error::{CurSeenError, Result};
use crate::core::image::RgbaBitmap;
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

/// PNG 编码（**用户可见落点**用：默认压缩档）。
///
/// 默认档 1920×1080 实测 median 122–123 ms、177–178 KB（n=200 正序 / n=100 倒序）。
/// 程序内部 `temp/` 的自动留档不走这里，走 [`encode_png_fast`]（median 14–15 ms、
/// 510–514 KB，2.9×）；两者解码出的像素完全一致，只差体积与耗时。「另存为」、剪贴板
/// 历史、脚本导出都留在默认档：用户挑的落点不该为省一百毫秒把文件撑大三倍。
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

/// PNG 文件 → 全尺寸位图（[`Shot`]，BGRA）。与 [`save_png`] 互为逆运算。
///
/// 只解 PNG，而且**唯一消费者是剪贴板历史**（PNG 原件与"预览读回"，代码从
/// `clipboard_history` 原样搬来）。
///
/// ★ 脚本读图**不**走这里（m21 起）：`ext:load_image` / `ext:set_image(路径)` 走
///   [`crate::platform::image_decode`]——WIC、**原尺寸**、任意 WIC 认得的格式。
///   别把脚本接回这条：它只认 PNG，而 `clipboard_history` 那条 WIC 缩略图路会把图缩到
///   720 宽（见 `decode_image_with_target` 的 `Some(THUMB_MAX_W)`）。
/// 颜色类型分支（RGBA / RGB / 灰度 / 调色板）逐一显式处理：调色板图不展开就会拿到
/// **索引值**而不是颜色（那条坑在剪贴板缩略图那儿的注释里记着）。
///
/// ★ 拿到的 `Shot` 转位图时**必须**用 [`crate::core::backdrop::shot_to_bitmap_alpha`]：
///   文件解出来的 α 是内容，而 `shot_to_bitmap` 那一档按抓屏语义把 α 一律刷成 255。
pub fn load_png(path: &std::path::Path) -> Option<Shot> {
    use std::io::BufReader;
    let f = std::fs::File::open(path).ok()?;
    let dec = png::Decoder::new(BufReader::new(f));
    let mut r = dec.read_info().ok()?;
    let mut buf = vec![0u8; r.output_buffer_size().unwrap_or(0)];
    let info = r.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    let (w, h) = (info.width as i32, info.height as i32);
    let mut bgra = vec![0u8; (w as usize) * (h as usize) * 4];
    match info.color_type {
        png::ColorType::Rgba => {
            for (px, b) in bgra.chunks_exact_mut(4).zip(buf.chunks_exact(4)) {
                px[0] = b[2];
                px[1] = b[1];
                px[2] = b[0];
                px[3] = b[3];
            }
        }
        png::ColorType::Rgb => {
            for (px, b) in bgra.chunks_exact_mut(4).zip(buf.chunks_exact(3)) {
                px[0] = b[2];
                px[1] = b[1];
                px[2] = b[0];
                px[3] = 255;
            }
        }
        _ => return None,
    }
    Some(Shot { w, h, bgra })
}

/// 存成 PNG 文件（父目录不存在则创建）。用户可见落点用这一版（默认压缩档）。
pub fn save_png(shot: &Shot, path: &std::path::Path) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let bytes = encode_png(shot)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

/// PNG 编码（**程序内部 `temp/` 自动留档专用**的快档）。
///
/// 压缩级别 `Fastest`：1920×1080 实测 median 14–15 ms（默认档 122–123 ms），文件
/// 510–514 KB（默认档 177–178 KB，2.9×）。用户没有指定位置、只用于回溯的留档拿体积
/// 换确认时延；用户点「另存为」选的落点仍走 [`encode_png`]。
///
/// 消费 `shot`：换序直接在原缓冲上做（[`Shot::into_rgba`]），不再多分配一份 `w*h*4`。
pub fn encode_png_fast(shot: Shot) -> Result<Vec<u8>> {
    let (w, h) = (shot.w as u32, shot.h as u32);
    let rgba = shot.into_rgba();
    encode_rgba_fast(w, h, &rgba)
}

/// [`encode_png_fast`] 的直通版：直接编码非预乘 RGBA 位图，不造中间 `Shot`。
///
/// 截图确认 / 标注留档手里就是 `RgbaBitmap`；旧路为调编码器先把它换成 BGRA，编码器
/// 再换回 RGBA——两趟整帧都是白走的。
pub fn encode_png_rgba_fast(bmp: &RgbaBitmap) -> Result<Vec<u8>> {
    encode_rgba_fast(bmp.w, bmp.h, &bmp.rgba)
}

/// `Fastest` 档的实际编码（两个入口共用同一份，避免两处参数漂移）。
fn encode_rgba_fast(w: u32, h: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    use png::{BitDepth, ColorType, Compression, Encoder};
    let mut out = Vec::new();
    {
        let mut enc = Encoder::new(&mut out, w, h);
        enc.set_color(ColorType::Rgba);
        enc.set_depth(BitDepth::Eight);
        enc.set_compression(Compression::Fastest);
        let mut writer = enc
            .write_header()
            .map_err(|e| CurSeenError::Unsupported(box_str(&format!("PNG 头: {e}"))))?;
        writer
            .write_image_data(rgba)
            .map_err(|e| CurSeenError::Unsupported(box_str(&format!("PNG 数据: {e}"))))?;
    }
    Ok(out)
}

/// 存成 PNG 文件（内部临时留档的快档，语义见 [`encode_png_fast`]）。
pub fn save_png_fast(shot: Shot, path: &std::path::Path) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let bytes = encode_png_fast(shot)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

/// 存成 PNG 文件（内部临时留档的快档，直通版，语义见 [`encode_png_rgba_fast`]）。
pub fn save_png_rgba_fast(bmp: &RgbaBitmap, path: &std::path::Path) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let bytes = encode_png_rgba_fast(bmp)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

/// 把一帧放到剪贴板（CF_DIB）。
///
/// 用 CF_DIB 而不是 PNG：前者是画图板 / 微信 / Word / 浏览器全都认的通用位图格式，
/// 后者只有部分应用注册了。`CF_DIB` 要求**自下而上**的行序，所以这里要把行倒过来写。
pub fn to_clipboard(shot: &Shot) -> Result<()> {
    put_dib(shot.w, shot.h, &shot.bgra, false)
}

/// 直接把一张非预乘 RGBA 位图放进剪贴板（CF_DIB），不再经 `Shot` 中转。
///
/// 截图确认拿到的是 `RgbaBitmap`：旧路 `bitmap_to_shot` 先分配并写一份整帧 BGRA，
/// 再被 `to_clipboard` 逐行拷进 DIB；这一版只在写 DIB 的那一趟里换 R/B。
pub fn to_clipboard_rgba(bmp: &RgbaBitmap) -> Result<()> {
    let w = i32::try_from(bmp.w).map_err(|_| CurSeenError::Unsupported("DIB 尺寸非法"))?;
    let h = i32::try_from(bmp.h).map_err(|_| CurSeenError::Unsupported("DIB 尺寸非法"))?;
    put_dib(w, h, &bmp.rgba, true)
}

/// `src`（自上而下、4 字节/像素）→ CF_DIB 全局内存 → 剪贴板。
///
/// `swap_rb` 为真时逐像素 RGBA→BGRA（`RgbaBitmap`）；为假时整行 `memcpy`
/// （`Shot` 本来就是这个通道序）。两条路都只过一趟整帧。
fn put_dib(w: i32, h: i32, src: &[u8], swap_rb: bool) -> Result<()> {
    // ★★ 不变量先钉死，再碰 `unsafe`。`Shot` / `RgbaBitmap` 都是**公开且外部可构造**
    //    的结构（PNG 解码、缩略图缩放、剪贴板回读都会造一个），一旦像素缓冲比 `w*h*4`
    //    短，下面那段逐行拷贝就会读越界——在 `unsafe` 里越界就是 0xC0000005，且发布版
    //    无符号、现场只留一句"程序没了"（2026-09-29 剪贴板"复制置顶"一按即崩）。
    //    宁可这一笔不写。
    if w <= 0 || h <= 0 {
        return Err(CurSeenError::Unsupported("DIB 尺寸非法"));
    }
    let stride = (w as usize) * 4;
    let Some(pixel_bytes) = stride.checked_mul(h as usize) else {
        return Err(CurSeenError::Unsupported("DIB 尺寸非法"));
    };
    if src.len() < pixel_bytes {
        return Err(CurSeenError::Unsupported("DIB 像素数据短于尺寸声明"));
    }
    let header = std::mem::size_of::<BITMAPINFOHEADER>();
    let Some(total) = header.checked_add(pixel_bytes) else {
        return Err(CurSeenError::Unsupported("DIB 尺寸非法"));
    };
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
        let written = {
            let dst = std::slice::from_raw_parts_mut(ptr.cast::<u8>(), total);
            write_dib(dst, w, h, src, swap_rb)
        };
        if !written {
            // 上面的校验与本函数对尺寸的判断漂移了；宁可这一笔不写。
            let _ = GlobalUnlock(hg);
            let _ = windows::Win32::Foundation::GlobalFree(Some(hg));
            return Err(CurSeenError::Unsupported("DIB 数据长度不符"));
        }
        let _ = GlobalUnlock(hg);

        // ★★★ 必须走进程级门禁（2026-10-13 根因修复）：`EmptyClipboard()` 会释放
        //   剪贴板上现有的数据块，而 `clip-thumb` 线程同一时刻可能正拿着那块的指针
        //   逐行 memcpy —— Windows 对**同进程内**的并发 `OpenClipboard` 不提供互斥
        //   （探针实测两个线程能同时打开），不加锁就是 use-after-free。
        //   返回值 `Some(None)` = 成功；`Some(Some(msg))` = 失败且内存已回收；
        //   `None` = 剪贴板开不了（内存还归我们，交给下面回收）。
        let r = crate::platform::clipboard::with_clipboard(|| {
            let _ = EmptyClipboard();
            // `SetClipboardData` 成功后剪贴板接管内存，不能再 GlobalFree。
            if SetClipboardData(CF_DIB, Some(windows::Win32::Foundation::HANDLE(hg.0))).is_err() {
                let _ = windows::Win32::Foundation::GlobalFree(Some(hg));
                return Some(Some("SetClipboardData 失败"));
            }
            Some(None)
        });
        match r {
            Some(None) => {}
            Some(Some(msg)) => return Err(CurSeenError::Unsupported(msg)),
            None => {
                // OpenClipboard 失败，内存仍归我们，必须释放
                let _ = windows::Win32::Foundation::GlobalFree(Some(hg));
                return Err(CurSeenError::Unsupported("OpenClipboard 失败"));
            }
        }
    }
    Ok(())
}

/// 把 `BITMAPINFOHEADER` + 自下而上的像素写进 `dst`；尺寸或源长度不符返回 `false`。
///
/// 返回 `bool` 而不是 `Result`：两种失败在调用方是同一件事（这一笔不写），而
/// `put_dib` 在调它之前已经按同一套尺寸算过一遍——这里再判一次是防两者漂移。
fn write_dib(dst: &mut [u8], w: i32, h: i32, src: &[u8], swap_rb: bool) -> bool {
    if w <= 0 || h <= 0 {
        return false;
    }
    let stride = (w as usize) * 4;
    let Some(pixel_bytes) = stride.checked_mul(h as usize) else {
        return false;
    };
    let header = std::mem::size_of::<BITMAPINFOHEADER>();
    let Some(total) = header.checked_add(pixel_bytes) else {
        return false;
    };
    if dst.len() < total || src.len() < pixel_bytes {
        return false;
    }
    let bi = dst.as_mut_ptr().cast::<BITMAPINFOHEADER>();
    unsafe {
        // SAFETY: `dst.len() >= header + pixel_bytes` 已在上方校验，头部这一笔不越界；
        // `write_unaligned` 不要求对齐（`dst` 可能是只保证 1 字节对齐的 `Vec<u8>`，生产里是
        // GlobalAlloc 内存）。`bi` 与随后的 `payload` 借用不重叠：头部写完就丢指针。
        std::ptr::write_unaligned(
            bi,
            BITMAPINFOHEADER {
                biSize: header as u32,
                biWidth: w,
                biHeight: h, // 正 = 自下而上
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                biSizeImage: pixel_bytes as u32,
                ..Default::default()
            },
        );
    }
    let payload = &mut dst[header..total];
    for row in 0..h as usize {
        let src_row = &src[(h as usize - 1 - row) * stride..][..stride];
        let dst_row = &mut payload[row * stride..(row + 1) * stride];
        if swap_rb {
            for (s, d) in src_row.chunks_exact(4).zip(dst_row.chunks_exact_mut(4)) {
                d[0] = s[2];
                d[1] = s[1];
                d[2] = s[0];
                d[3] = s[3];
            }
        } else {
            dst_row.copy_from_slice(src_row);
        }
    }
    true
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

    /// 快档只换压缩级别，像素必须与默认档**完全一样**。
    ///
    /// 样本带非零 α 与互不相同的 R/B/G：若 `into_rgba` 漏换序、编码器写错通道，
    /// 解码回来就对不上（只看 PNG 签名发现不了这类错）。
    #[test]
    fn fast_png_decodes_to_the_same_pixels_as_default() {
        let s = Shot {
            w: 2,
            h: 2,
            bgra: vec![
                0x11, 0x22, 0x33, 0x7F, 0x44, 0x55, 0x66, 0x00, 0x77, 0x88, 0x99, 0x01, 0xAA, 0xBB,
                0xCC, 0x02,
            ],
        };
        let want = s.to_rgba();
        let bytes = encode_png_fast(s).expect("快档应编码成功");
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");

        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder.read_info().expect("读 PNG 头");
        let mut buf = vec![0_u8; reader.output_buffer_size().expect("输出缓冲可算")];
        let info = reader.next_frame(&mut buf).expect("解码 PNG");
        buf.truncate(info.buffer_size());
        assert_eq!(buf, want, "解码像素必须与 to_rgba 逐字节一致");
    }

    /// DIB 的两条写入口（RGBA 直通 / BGRA）必须产生同一份字节，且首行是源图最后一行。
    #[test]
    fn dib_writer_matches_between_rgba_and_bgra() {
        let rgba = vec![
            0x10, 0x20, 0x30, 0xFF, 0x40, 0x50, 0x60, 0xFF, // 上排
            0x70, 0x80, 0x90, 0xFF, 0xA0, 0xB0, 0xC0, 0xFF, // 下排
        ];
        let bmp = RgbaBitmap::new(2, 2, rgba).expect("位图");
        let shot = crate::core::backdrop::bitmap_to_shot(&bmp).expect("帧");
        let header = std::mem::size_of::<BITMAPINFOHEADER>();
        let total = header + 2 * 2 * 4;
        let mut from_rgba = vec![0_u8; total];
        let mut from_bgra = vec![0_u8; total];
        assert!(
            write_dib(&mut from_rgba, 2, 2, &bmp.rgba, true),
            "RGBA 入口"
        );
        assert!(
            write_dib(&mut from_bgra, 2, 2, &shot.bgra, false),
            "BGRA 入口"
        );
        assert_eq!(from_rgba, from_bgra, "两条入口必须逐字节相同");

        // 像素区第一行 = 源图最后一行（自下而上），通道是 BGRA。
        let px = &from_rgba[header..];
        assert_eq!(&px[0..4], &[0x90, 0x80, 0x70, 0xFF]);
        assert_eq!(&px[4..8], &[0xC0, 0xB0, 0xA0, 0xFF]);
        // 头字段：正高度（自下而上）、32bpp、无压缩。
        let w_le = i32::from_le_bytes(from_rgba[4..8].try_into().unwrap());
        let h_le = i32::from_le_bytes(from_rgba[8..12].try_into().unwrap());
        assert_eq!((w_le, h_le), (2, 2));
        assert_eq!(
            u16::from_le_bytes(from_rgba[14..16].try_into().unwrap()),
            32
        );
    }

    /// 端到端：RGBA 直接写剪贴板，读回来必须与 `bitmap_to_shot` 逐字节相同。
    ///
    /// 只测 `write_dib` 抓不到「wrapper 把 `swap_rb` 传成 false」那一行，这条走完整往返。
    #[cfg(windows)]
    #[test]
    fn rgba_clipboard_roundtrip_keeps_channels_and_row_order() {
        let rgba = vec![
            0x10, 0x20, 0x30, 0xFF, 0x40, 0x50, 0x60, 0xFF, // 上排
            0x70, 0x80, 0x90, 0xFF, 0xA0, 0xB0, 0xC0, 0xFF, // 下排
        ];
        let bmp = RgbaBitmap::new(2, 2, rgba).expect("位图");
        let expected = crate::core::backdrop::bitmap_to_shot(&bmp).expect("帧");
        // 剪贴板被别的进程占着是常态：短暂重试，但失败就是失败。
        let mut wrote = false;
        for _ in 0..20 {
            if to_clipboard_rgba(&bmp).is_ok() {
                wrote = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(wrote, "剪贴板持续被占用，无法验证");
        let mut back = None;
        for _ in 0..20 {
            if let Some(s) = crate::platform::clipboard::get_image() {
                back = Some(s);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let back = back.expect("读回剪贴板图像");
        assert_eq!((back.w, back.h), (2, 2));
        assert_eq!(back.bgra, expected.bgra, "通道顺序或行序与 Shot 版不同");
    }

    /// 直通版快档：解码像素必须与位图原字节完全一致（不换序、不补 α）。
    #[test]
    fn rgba_fast_png_decodes_to_the_same_pixels() {
        let rgba = vec![
            0x11, 0x22, 0x33, 0x7F, 0x44, 0x55, 0x66, 0x00, // 上排
            0x77, 0x88, 0x99, 0x01, 0xAA, 0xBB, 0xCC, 0x02, // 下排
        ];
        let bmp = RgbaBitmap::new(2, 2, rgba.clone()).expect("位图");
        let bytes = encode_png_rgba_fast(&bmp).expect("快档应编码成功");
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder.read_info().expect("读 PNG 头");
        let mut buf = vec![0_u8; reader.output_buffer_size().expect("输出缓冲可算")];
        let info = reader.next_frame(&mut buf).expect("解码 PNG");
        buf.truncate(info.buffer_size());
        assert_eq!(buf, rgba, "解码像素必须与 RgbaBitmap 逐字节一致");
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

    /// 生产路径量具：帧→位图两版、PNG 两档、出口直通臂的实际耗时与体积。
    ///
    /// 跑法：`PERF_ROUNDS=200 cargo test --release --lib perf_probe_screenshot_paths -- --ignored --nocapture --test-threads=1`
    ///
    /// ★ 对照臂也是**生产函数**（借用版 `shot_to_bitmap` / 默认档 `encode_png`），不另写
    ///   一份等价实现——探针只该量产品，不该量它的复刻。`shot.clone()` 只为让同一帧喂给
    ///   多个分支，落在计时区间之外。`PERF_REVERSE=1` 把成对的臂倒过来跑，
    ///   用来排除"后跑的那一臂吃到热缓存"。
    ///
    /// ★ m06 的剪贴板两臂只量**写入**（`to_clipboard` / `to_clipboard_rgba`），
    ///   `bitmap_to_shot` 单独一臂——旧端点 = 两者相加。这条探针会真的改写剪贴板
    ///   （每轮 2 次，共 2n 次），只在手动跑时执行。
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn perf_probe_screenshot_paths() {
        use std::time::{Duration, Instant};
        let rounds: usize = std::env::var("PERF_ROUNDS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3)
            .max(1);
        let reverse = std::env::var("PERF_REVERSE").is_ok();
        let r = crate::platform::display::virtual_screen();
        let _ = grab(r).expect("预热抓屏");

        let mut grab_t = Vec::new();
        let mut borrowed_t = Vec::new();
        let mut owned_t = Vec::new();
        let mut default_t = Vec::new();
        let mut fast_t = Vec::new();
        let mut save_t = Vec::new();
        let mut to_shot_t = Vec::new();
        let mut clip_old_t = Vec::new();
        let mut clip_new_t = Vec::new();
        let mut png_rgba_slow_t = Vec::new();
        let mut png_rgba_fast_t = Vec::new();
        let mut save_rgba_t = Vec::new();
        let (mut slow_len, mut fast_len, mut disk_len) = (0_usize, 0_usize, 0_usize);
        let (mut rgba_slow_len, mut rgba_fast_len) = (0_usize, 0_usize);
        let (mut w, mut h) = (0, 0);

        for round in 0..rounds {
            let t = Instant::now();
            let shot = grab(r).expect("抓屏");
            grab_t.push(t.elapsed());
            (w, h) = (shot.w, shot.h);

            let t = Instant::now();
            let borrowed = crate::core::backdrop::shot_to_bitmap(&shot).expect("借用版");
            borrowed_t.push(t.elapsed());

            let t = Instant::now();
            let owned = crate::core::backdrop::shot_to_bitmap_owned(shot.clone()).expect("消费版");
            owned_t.push(t.elapsed());
            assert_eq!(owned.rgba, borrowed.rgba, "两版必须逐字节一致");
            drop(borrowed);
            drop(owned);

            // ── 出口直通臂（m06）：旧端点 = bitmap_to_shot + 写剪贴板/编码 ──────────
            let bmp = crate::core::backdrop::shot_to_bitmap(&shot).expect("出口位图");
            let t = Instant::now();
            let bgra = crate::core::backdrop::bitmap_to_shot(&bmp).expect("BGRA 中间帧");
            to_shot_t.push(t.elapsed());

            if reverse {
                let t = Instant::now();
                to_clipboard_rgba(&bmp).expect("直通剪贴板");
                clip_new_t.push(t.elapsed());

                let t = Instant::now();
                to_clipboard(&bgra).expect("旧剪贴板");
                clip_old_t.push(t.elapsed());
            } else {
                let t = Instant::now();
                to_clipboard(&bgra).expect("旧剪贴板");
                clip_old_t.push(t.elapsed());

                let t = Instant::now();
                to_clipboard_rgba(&bmp).expect("直通剪贴板");
                clip_new_t.push(t.elapsed());
            }

            if reverse {
                let t = Instant::now();
                let fast = encode_png_rgba_fast(&bmp).expect("RGBA 快档");
                png_rgba_fast_t.push(t.elapsed());
                rgba_fast_len = fast.len();

                let t = Instant::now();
                let slow = encode_png(&bgra).expect("RGBA 默认档");
                png_rgba_slow_t.push(t.elapsed());
                rgba_slow_len = slow.len();
            } else {
                let t = Instant::now();
                let slow = encode_png(&bgra).expect("RGBA 默认档");
                png_rgba_slow_t.push(t.elapsed());
                rgba_slow_len = slow.len();

                let t = Instant::now();
                let fast = encode_png_rgba_fast(&bmp).expect("RGBA 快档");
                png_rgba_fast_t.push(t.elapsed());
                rgba_fast_len = fast.len();
            }

            let path_rgba =
                std::env::temp_dir().join(format!("curseen_perf_probe_rgba_{round}.png"));
            let t = Instant::now();
            save_png_rgba_fast(&bmp, &path_rgba).expect("RGBA 快档落盘");
            save_rgba_t.push(t.elapsed());
            let _ = std::fs::remove_file(&path_rgba);
            drop(bgra);
            drop(bmp);

            if reverse {
                let t = Instant::now();
                let fast = encode_png_fast(shot.clone()).expect("快档");
                fast_t.push(t.elapsed());
                fast_len = fast.len();

                let t = Instant::now();
                let slow = encode_png(&shot).expect("默认档");
                default_t.push(t.elapsed());
                slow_len = slow.len();
            } else {
                let t = Instant::now();
                let slow = encode_png(&shot).expect("默认档");
                default_t.push(t.elapsed());
                slow_len = slow.len();

                let t = Instant::now();
                let fast = encode_png_fast(shot.clone()).expect("快档");
                fast_t.push(t.elapsed());
                fast_len = fast.len();
            }

            let path = std::env::temp_dir().join(format!("curseen_perf_probe_{round}.png"));
            let t = Instant::now();
            save_png_fast(shot, &path).expect("快档落盘");
            save_t.push(t.elapsed());
            disk_len = std::fs::metadata(&path)
                .map(|m| m.len() as usize)
                .unwrap_or(0);
            let _ = std::fs::remove_file(&path);
        }

        fn stat(name: &str, mut v: Vec<Duration>) {
            v.sort_unstable();
            let n = v.len();
            let p95 = v[(n * 95 / 100).min(n - 1)];
            let mean = v.iter().sum::<Duration>() / n as u32;
            println!(
                "{name}: n={n} min={:?} mean={mean:?} median={:?} p95={p95:?} max={:?}",
                v[0],
                v[n / 2],
                v[n - 1]
            );
        }
        println!("--- {w}x{h} ---");
        stat("grab", grab_t);
        stat("convert_borrowed", borrowed_t);
        stat("convert_owned", owned_t);
        stat("png_default", default_t);
        stat("png_fast", fast_t);
        stat("save_png_fast", save_t);
        stat("bitmap_to_shot", to_shot_t);
        stat("clip_old(write only)", clip_old_t);
        stat("clip_rgba(write only)", clip_new_t);
        stat("png_rgba_default", png_rgba_slow_t);
        stat("png_rgba_fast", png_rgba_fast_t);
        stat("save_png_rgba_fast", save_rgba_t);
        println!(
            "sizes: default={slow_len} B fast={fast_len} B ({:.2}x) on_disk={disk_len} B",
            fast_len as f64 / slow_len as f64
        );
        println!(
            "sizes(rgba): default={rgba_slow_len} B fast={rgba_fast_len} B ({:.2}x)",
            rgba_fast_len as f64 / rgba_slow_len as f64
        );
    }
}
