//! 画中画放大窗（独立原生分层窗）。
//!
//! # 为什么必须是**独立窗**，而不是画在覆盖层上
//!
//! 上一版把放大面板当作一条 `DrawCmd::Image` 画在 CurSeen 的全屏覆盖层上，然后靠
//! `screenshot::grab_clean`（纯 `SRCCOPY`）"排除分层窗"来保证源图干净。那个前提是
//! **错的**，而且错得很隐蔽：
//!
//! ```text
//! tests/pip_probe.rs 实测（2026-09-20，本机 1920×1080）
//!   ① 基准（无分层窗）                SRCCOPY              红占比 =   0.00%
//!   ② 分层窗已显示                    SRCCOPY (grab_clean) 红占比 = 100.00%  ← 完全抓到了
//!                                     SRCCOPY|CAPTUREBLT   红占比 = 100.00%
//!   ③ 加 WDA_EXCLUDEFROMCAPTURE 后    SRCCOPY              红占比 =   0.00%
//!                                     SRCCOPY|CAPTUREBLT   红占比 =   0.00%
//! ```
//!
//! 结论：**有没有 `CAPTUREBLT` 在这里毫无区别**。只要内容是靠 `UpdateLayeredWindow`
//! 提交到屏幕上的，`BitBlt` 就抓得到。于是放大镜把自己上一帧的面板抓进源图、再放大
//! → **画中画中画无限套娃**；倍率的变化只改变"套娃的某一层占多大"，表现为"调倍率
//! 面板里的画面没放大"。
//!
//! 环境相关的 GDI 习性不能当正确性依据。可靠的是
//! `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`：它是**系统级**的排除策略，
//! GDI `BitBlt`（含 `CAPTUREBLT`）与 DXGI Desktop Duplication 都遵守（见上表 ③，两种
//! rop 都掉到 0%）。**它按窗口生效** —— 要能"只排除放大镜自己"，放大镜就必须拥有
//! 自己的窗口；画在同一张覆盖层上做不到（那会把拖尾 / 击键回显一起排除掉）。
//!
//! # 代价（有意为之，别当成 bug 修）
//!
//! 排除是全有全无的：这个窗同时也不会出现在 CurSeen 自己的录屏里。取舍上刻意如此 ——
//! 录制**本来就有原始分辨率的底**，视频里再嵌一块放大 copy 只是重复内容；而"绝不套娃"
//! 是"能不能用"的问题，两者不对等。将来真要"录制里也带放大镜"，正确做法是录制时把
//! 同一份源帧按同样几何画进视频，而不是把这个窗重新放开被抓。
//!
//! # 单位 / 坐标
//!
//! 全链路**物理像素**（虚拟桌面坐标系），与 [`crate::platform::display::ScreenRect`]
//! 一致；进程是 Per-Monitor DPI Aware v2，不需要再乘 DPI。

use std::mem::size_of;
use std::sync::Once;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HWND, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, SetStretchBltMode,
    StretchDIBits, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, COLORONCOLOR,
    DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, SRCCOPY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassExW, SetWindowDisplayAffinity,
    ShowWindow, UpdateLayeredWindow, SW_HIDE, SW_SHOWNOACTIVATE, ULW_ALPHA, WDA_EXCLUDEFROMCAPTURE,
    WINDOW_EX_STYLE, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::core::color::Argb;
use crate::core::error::{CurSeenError, Result};
use crate::core::image::Shot;
use crate::platform::capture_share;
use crate::platform::display::ScreenRect;

/// 窗口标题。
///
/// ★ 两个用途都**依赖这个字面量**，别随手改：
/// 1. `TopmostKeeper::guard_aux` 按标题每 250ms 把它插回置顶带顶部（不抢焦点）；
/// 2. 排查时能在日志 / spy 工具里认出它是谁。
pub const PIP_WINDOW_TITLE: &str = "CurSeen 画中画放大";

/// 全屏放大那个窗的标题（同一套实现，另一个实例）。
///
/// ★ 为什么两个窗不能同名：`TopmostKeeper::guard_aux` 是**按标题找窗**的，两个窗
///   同名时 `FindWindowW` 只返回其中一个——那个可能是当前隐藏的那个，于是真正显示
///   的这个永远没人把它插回置顶带顶部（表现为"偶尔被别的窗盖住"）。各用一个标题，
///   两个都登记进守卫名单即可。
pub const FULLSCREEN_WINDOW_TITLE: &str = "CurSeen 全屏放大";

/// 窗口类名（进程内一次性注册）。
const CLASS_NAME: &str = "CurSeenPipWindowClass";

/// 面板外观。
///
/// 打成一个结构体而不是散着传参：`present` 的入参已经贴着 clippy 的 7 个上限，
/// 而这三项语义上也确实是一起来的——描述"面板长什么样"，与"面板在哪"（`rect`）对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipLook {
    /// 边框颜色（含 alpha，会**按 alpha 混合**盖在放大的画面之上）。
    pub border: Argb,
    /// 边框宽度（物理像素）。`0` = 不画。
    pub border_width: i32,
    /// **定位点**：光标在面板内的落点（面板局部坐标，物理像素）；`None` = 没有定位。
    ///
    /// ★ 本身**不画任何字形**（2026-10-02 用户口径"不要居中的光标"——十字字形已删）。
    ///   它只是**像素格的定位来源**：配合 `grid`，先给光标所在的整列、整行铺 50% 主色蓝
    ///   十字定位线，再给所在格描 2px 纯黑边。单独传 `crosshair_at` 不画任何东西。
    pub crosshair_at: Option<(i32, i32)>,
    /// **像素格**：`(列数, 行数)` —— 面板被等分成这么多个"源像素格"，每格之间画一条
    ///   浅色格线；配合 `crosshair_at`，光标所在的那一格再描**纯黑加粗边**（用户口径
    ///   2026-10-02："让用户一眼看出来是中心"）。`None` = 不画（画中画/全屏放大照旧）。
    ///
    /// 格线位置按 `i×面板宽÷列数` **均分**而不是按固定物理间距画：取样区贴屏幕边被钳小
    /// 时有效倍率不是整数，固定间距会与真实像素错位；均分永远逐像素对齐。
    pub grid: Option<(i32, i32)>,
}

impl Default for PipLook {
    fn default() -> Self {
        Self {
            border: Argb(0xC800FF00),
            border_width: 2,
            // 出厂不画准星（画中画的"指针准星"选项已于 2026-09-21 整体删除）。
            crosshair_at: None,
            grid: None,
        }
    }
}

/// 面板像素缓冲：一块 top-down 32bpp DIB + 配套内存 DC。
///
/// 为什么缓存而不是每帧重建：这块位图每帧都会被 `StretchDIBits` 重写一次，重建意味着
/// 每帧一次 `CreateDIBSection`（默认面板 768×270 ≈ 518 KB），30 FPS 下就是持续的 GDI
/// 对象 / 堆分配噪声。**只在尺寸变化时重建。**
struct Panel {
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    /// `CreateDIBSection` 返回的像素地址，与 `bmp` 同寿命。
    bits: *mut u8,
    w: i32,
    h: i32,
}

impl Panel {
    /// 新建一块 `w×h` 缓冲。
    fn create(w: i32, h: i32) -> Result<Self> {
        let dc = unsafe { CreateCompatibleDC(None) };
        if dc.is_invalid() {
            return Err(CurSeenError::Unsupported("画中画 CreateCompatibleDC 失败"));
        }
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                // 负值 = 自上而下：行序与 `Shot::bgra` 一致，省掉一次行翻转。
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bmp =
            match unsafe { CreateDIBSection(Some(dc), &bi, DIB_RGB_COLORS, &mut bits, None, 0) } {
                Ok(b) => b,
                Err(_) => {
                    unsafe {
                        let _ = DeleteDC(dc);
                    }
                    return Err(CurSeenError::Unsupported("画中画 CreateDIBSection 失败"));
                }
            };
        let old = unsafe { SelectObject(dc, bmp.into()) };
        Ok(Self {
            dc,
            bmp,
            old,
            bits: bits as *mut u8,
            w,
            h,
        })
    }

    /// 像素切片（避免把裸指针解开的责任推给调用方）。
    fn pixels(&mut self) -> Option<&mut [u8]> {
        if self.bits.is_null() {
            return None;
        }
        let n = (self.w.max(0) as usize)
            .checked_mul(self.h.max(0) as usize)?
            .checked_mul(4)?;
        // SAFETY：`bits` 由 `CreateDIBSection` 分配、随 `self` 一同释放，`n` 就是它的长度。
        Some(unsafe { std::slice::from_raw_parts_mut(self.bits, n) })
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bmp.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

/// 画中画放大窗。
///
/// 生命周期：`present` 时才真正开窗；`hide` 只藏不毁（保住 GDI 对象，再次开启不重建）；
/// `Drop` 保证销毁——持有它的 `Runtime` 退出时不必刻意清理。
pub struct PipWindow {
    hwnd: Option<HWND>,
    /// 已呈现的面板矩形（判同步用）。
    shown_at: Option<ScreenRect>,
    panel: Option<Panel>,
    /// 窗已 `ShowWindow` 过（首帧后才显示，避免先闪一块空/GDI 默认底色）。
    visible: bool,
    /// `WDA_EXCLUDEFROMCAPTURE` 是否真的设上了。
    ///
    /// 失败**不能静默**：那时的现象就是"又套娃了"，而没有这条信息现场无从查起。
    excluded: bool,
    /// 窗口标题（见 [`FULLSCREEN_WINDOW_TITLE`] 上"为什么两个窗不能同名"）。
    title: &'static str,
}

impl PipWindow {
    /// 画中画那个窗（标题 [`PIP_WINDOW_TITLE`]）。
    #[must_use]
    pub fn new() -> Self {
        Self::with_title(PIP_WINDOW_TITLE)
    }

    /// 带自己标题的实例：全屏放大与画中画各持一个，两个窗彼此独立。
    #[must_use]
    pub fn with_title(title: &'static str) -> Self {
        Self {
            hwnd: None,
            shown_at: None,
            panel: None,
            visible: false,
            excluded: false,
            title,
        }
    }

    /// 窗是否已可见。
    #[must_use]
    pub fn is_visible(&self) -> bool {
        self.visible && self.shown_at.is_some()
    }

    /// 排除策略是否生效（把自己从任何抓屏里摘掉）。
    #[must_use]
    pub fn is_excluded(&self) -> bool {
        self.excluded
    }

    /// 藏起来（保留窗与缓冲）。
    pub fn hide(&mut self) {
        if let Some(hwnd) = self.hwnd {
            unsafe {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            self.visible = false;
            self.shown_at = None;
        }
        // 发布内容同步清空：面板不在屏幕上就不该再被贴进录制帧。
        if capture_share::is_enabled() {
            capture_share::clear(self.title);
        }
    }

    /// 把取样源帧放大铺满面板，并呈现到 `rect`（虚拟桌面物理像素）。
    ///
    /// `src` 是**取样区**尺寸（`cw×ch`）的干净源图；这里是唯一把它拉到**面板**尺寸
    /// （`pw×ph`）的地方——实际放大比 = `panel.w / src.w`。
    pub fn present(&mut self, src: &Shot, rect: ScreenRect, look: &PipLook) -> Result<()> {
        if src.is_empty() || rect.w < 2 || rect.h < 2 {
            return Err(CurSeenError::Unsupported("画中画源图或面板尺寸无效"));
        }
        let hwnd = match self.hwnd {
            Some(h) => h,
            None => {
                let h = self.spawn()?;
                self.hwnd = Some(h);
                h
            }
        };
        // ★ 只在尺寸变化时才换缓冲，且**先把旧的真正 Drop 掉**再建新的：两个 `Panel`
        //   都实现了 `Drop`，若先复制出一份句柄副本、再让旧实例析构，被析构的恰好是
        //   新实例正在用的那几个 GDI 句柄（常见表现为"面板第一次正常、之后全黑"）。
        let stale = self
            .panel
            .as_ref()
            .is_some_and(|p| p.w != rect.w || p.h != rect.h);
        if stale {
            drop(self.panel.take());
        }
        if self.panel.is_none() {
            self.panel = Some(Panel::create(rect.w, rect.h)?);
        }
        let panel = match &mut self.panel {
            Some(p) => p,
            None => return Err(CurSeenError::Unsupported("画中画缓冲区缺失")),
        };

        // 先把尺寸拷出来：`pixels()` 要对同一块缓冲取可变借用。
        let (pw, ph, pdc) = (panel.w, panel.h, panel.dc);
        blit_scaled(panel, src);
        if let Some(px) = panel.pixels() {
            paint_decorations(px, pw, ph, look);
        }

        unsafe {
            UpdateLayeredWindow(
                hwnd,
                None,
                Some(&POINT {
                    x: rect.x,
                    y: rect.y,
                }),
                Some(&SIZE {
                    cx: rect.w,
                    cy: rect.h,
                }),
                Some(pdc),
                Some(&POINT { x: 0, y: 0 }),
                COLORREF(0),
                Some(&BLENDFUNCTION {
                    BlendOp: AC_SRC_OVER as u8,
                    BlendFlags: 0,
                    // ★ SourceConstantAlpha=255 且 **不带** `AC_SRC_ALPHA`：面板是
                    //   "不透明底 + 上面混合好的边框/十字"，不需要逐像素 alpha。
                    //   带上 `AC_SRC_ALPHA` 会踩 GDI 那个坑——`FillRect` 逐像素写出来的
                    //   32bpp 高位字节是 0，整块面板会直接消失。
                    SourceConstantAlpha: 0xFF,
                    AlphaFormat: 0,
                }),
                ULW_ALPHA,
            )
            .map_err(|_| CurSeenError::Unsupported("UpdateLayeredWindow 失败"))?;
        }
        // 像素到位后才显示，避免第一眼闪一块空窗口。
        if !self.visible {
            unsafe {
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            }
            self.visible = true;
        }
        self.shown_at = Some(rect);

        // ── 录制合成通道（见 `capture_share` 模块头）──
        // 面板位图本来就是"干净源图拉伸后的结果"（源图走排除通道，永不套娃），
        // 录制会话存活期间把它发布给录制管线，视频里才有这块放大画面——
        // 否则 WDA_EXCLUDEFROMCAPTURE 会把面板从所有捕获里整体抠掉。
        // 通道本体是命名共享内存（2026-10-07），跨主/子进程；失败会退
        // 进程内注册表，`publish` 内部处理，这里只管"开着才拷"。
        if capture_share::is_enabled() {
            if let Some(px) = panel.pixels() {
                capture_share::publish(
                    self.title,
                    capture_share::PipFrame {
                        x: rect.x,
                        y: rect.y,
                        w: rect.w,
                        h: rect.h,
                        bgra: px.to_vec(),
                    },
                );
            }
        }
        Ok(())
    }

    /// 惰性开窗，返回句柄。
    fn spawn(&mut self) -> Result<HWND> {
        static REGISTER: Once = Once::new();
        REGISTER.call_once(register_class);

        let class: Vec<u16> = CLASS_NAME
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let title: Vec<u16> = self
            .title
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(
                    (WS_EX_LAYERED
                        | WS_EX_TOPMOST
                        | WS_EX_TOOLWINDOW
                        | WS_EX_TRANSPARENT
                        | WS_EX_NOACTIVATE)
                        .0,
                ),
                PCWSTR(class.as_ptr()),
                PCWSTR(title.as_ptr()),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                None,
                Some(std::ptr::null()),
            )
            .map_err(|_| CurSeenError::Unsupported("创建画中画窗口失败"))?;

            // ★★ 本模块存在的全部理由：把自己从**所有**抓屏里排除掉。
            //   少了这一步，下一次采样就会把自己上一帧的面板抓进来 → 无限套娃。
            //   失败不阻塞（窗照样能用），但必须报出来：`is_excluded() == false` 与
            //   "又套娃了"在现场没法区分，日志是唯一的线索。
            match SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) {
                Ok(()) => self.excluded = true,
                Err(e) => log::error!(
                    "画中画：SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE) 失败 —— \
                     面板会被自己的采样抓到（表现为无限套娃）：{e}"
                ),
            }
            Ok(hwnd)
        }
    }
}

/// 把源图放大铺进面板缓冲。
///
/// `COLORONCOLOR`：源像素**不做插值**，整数倍放大时每个像素被原样复制成 z×z 块 ——
/// 这正是放大镜该有的"数得清像素"的观感（`HALFTONE` 会把文字先糊成一团，代价也更大）。
///
/// 为什么是自由函数而不是 `PipWindow` 的方法：`present` 里已经握着 `self.panel` 的可变
/// 借用，再经 `self` 转一次会撞借用检查。
fn blit_scaled(panel: &Panel, src: &Shot) {
    unsafe {
        SetStretchBltMode(panel.dc, COLORONCOLOR);
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: src.w,
                // top-down，与 `Shot::bgra` 行序一致。
                biHeight: -src.h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        // `StretchDIBits` 直接吃像素切片，不必再建一张临时源位图。
        let rows = StretchDIBits(
            panel.dc,
            0,
            0,
            panel.w,
            panel.h,
            0,
            0,
            src.w,
            src.h,
            Some(src.bgra.as_ptr().cast()),
            &bi,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
        if rows == 0 {
            log::warn!(
                "画中画：StretchDIBits 未出图（源 {w}x{h} → 面板 {pw}x{ph}）",
                w = src.w,
                h = src.h,
                pw = panel.w,
                ph = panel.h
            );
        }
    }
}

impl Default for PipWindow {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for PipWindow {
    fn drop(&mut self) {
        drop(self.panel.take());
        if let Some(hwnd) = self.hwnd.take() {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }
    }
}

/// 在面板像素上画边框与像素格装饰（定位点由 `look` 给定）。
///
/// **自己合成而不是走 GDI 笔刷**：边框色默认带 alpha（`#00FF00C8`），而 GDI 的
/// `FillRect` 只写 RGB（还会把 32bpp 的高位字节写成 0）——用它，半透明边框会变成实色，
/// 看似小事，实则改掉了出厂默认值的观感。周长像素约 `2×(w+h)×bw`（1080p 默认面板
/// ≈ 4 千个），成本可以忽略。
///
/// 抽出这个纯形体是为了**可测**：不需要真的 HDC 就能断言边框只动周长、不动画面。
fn paint_decorations(px: &mut [u8], w: i32, h: i32, look: &PipLook) {
    let bw = look.border_width.clamp(0, w.min(h) / 2);
    if bw > 0 {
        let c = look.border;
        for y in 0..h {
            let on_row_edge = y < bw || y >= h - bw;
            for x in 0..w {
                // 内层每行只判两次列边界 → 整块成本 ~O(周长)，不是 O(w×h)。
                if on_row_edge || x < bw || x >= w - bw {
                    blend_at(px, w, x, y, c);
                }
            }
        }
    }

    // ── 像素格（截图放大镜专用；画中画/全屏放大恒 None 不画）──
    //
    // 格线画在**两条源像素的分界上**（i=1..cols-1，不含 0 与 cols：外缘已有边框，重画
    // 只是白混一次）。颜色用 25% 黑：浅色截图上可辨、深色截图上不喧宾夺主；中心格再压
    // 一圈 95% 黑的 2px 描边——用户口径"边框纯黑、其它比较浅、一眼看出中心"。
    if let Some((cols, rows)) = look.grid {
        let grid_line = Argb(0x40000000);
        if cols > 1 {
            for i in 1..cols {
                let x = i * w / cols;
                for y in 0..h {
                    blend_at(px, w, x, y, grid_line);
                }
            }
        }
        if rows > 1 {
            for j in 1..rows {
                let y = j * h / rows;
                for x in 0..w {
                    blend_at(px, w, x, y, grid_line);
                }
            }
        }
        // 光标所在格：整列、整行铺**十字定位线**（用户口径 2026-10-02：类似悬浮标的屏幕
        // 标尺——50% 主色蓝），但**中心格本身不染**（同日追加口径："框里面得是原来的
        // 颜色，否则这个格子我都看不清原来的颜色了"——定位线绕开中心格走），格边界再描
        // 2px 纯黑边。
        if let (Some((cx, cy)), true) = (look.crosshair_at, cols > 0 && rows > 0) {
            let col = (cx * cols / w.max(1)).clamp(0, cols - 1);
            let row = (cy * rows / h.max(1)).clamp(0, rows - 1);
            let x0 = col * w / cols;
            let x1 = ((col + 1) * w / cols).min(w);
            let y0 = row * h / rows;
            let y1 = ((row + 1) * h / rows).min(h);
            let ruler = Argb(0x804C8BF5); // 50% 主色蓝（76,139,245）
            for y in 0..h {
                if (y0..y1).contains(&y) {
                    continue; // 中心格纵段：不染
                }
                for x in x0..x1 {
                    blend_at(px, w, x, y, ruler);
                }
            }
            for y in y0..y1 {
                for x in 0..w {
                    if (x0..x1).contains(&x) {
                        continue; // 中心格横段：不染
                    }
                    blend_at(px, w, x, y, ruler);
                }
            }
            let cell_edge = Argb(0xF2000000);
            for t in 0..2 {
                if y0 + t < y1 {
                    for x in x0..x1 {
                        blend_at(px, w, x, y0 + t, cell_edge);
                    }
                }
                if y1 - 1 - t >= y0 {
                    for x in x0..x1 {
                        blend_at(px, w, x, y1 - 1 - t, cell_edge);
                    }
                }
                if x0 + t < x1 {
                    for y in y0..y1 {
                        blend_at(px, w, x0 + t, y, cell_edge);
                    }
                }
                if x1 - 1 - t >= x0 {
                    for y in y0..y1 {
                        blend_at(px, w, x1 - 1 - t, y, cell_edge);
                    }
                }
            }
        }
    }
}

/// 在像素 `(x, y)` 处按 alpha 混一层 `col`（缓冲是 BGRA 内存序）。
#[inline]
fn blend_at(px: &mut [u8], w: i32, x: i32, y: i32, col: Argb) {
    let at = (y as usize * w as usize + x as usize) * 4;
    if at + 2 >= px.len() {
        return;
    }
    let a = col.a() as u32;
    if a == 0 {
        return;
    }
    let inv = 255 - a;
    px[at] = ((col.b() as u32 * a + px[at] as u32 * inv) / 255) as u8;
    px[at + 1] = ((col.g() as u32 * a + px[at + 1] as u32 * inv) / 255) as u8;
    px[at + 2] = ((col.r() as u32 * a + px[at + 2] as u32 * inv) / 255) as u8;
}

fn register_class() {
    let class: Vec<u16> = CLASS_NAME
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let wc = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(pip_wndproc),
        lpszClassName: PCWSTR(class.as_ptr()),
        ..Default::default()
    };
    unsafe {
        if RegisterClassExW(&wc) == 0 {
            log::warn!("画中画：窗口类注册失败（重复注册或资源不足）");
        }
    }
}

extern "system" fn pip_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 搭一块全白微型面板（`paint_decorations` 是纯函数，不需要真 HDC）。
    fn white_panel(w: i32, h: i32) -> Vec<u8> {
        let mut v = vec![0u8; (w * h * 4) as usize];
        for q in v.chunks_exact_mut(4) {
            q[0] = 0xFF;
            q[1] = 0xFF;
            q[2] = 0xFF;
        }
        v
    }

    fn read_px(v: &[u8], w: i32, x: i32, y: i32) -> (u8, u8, u8) {
        let i = (y as usize * w as usize + x as usize) * 4;
        (v[i], v[i + 1], v[i + 2])
    }

    /// ★★ 缩放必须真的发生：源图的**每一个**像素要被铺成 `z×z` 的方块。
    ///
    /// 这条是"倍率没反应"的**合成端**判据。`pip.rs` 那边已经证明"取样区随倍率变小"，
    /// 但"把这块小图无损拉满面板"若没生效（`StretchDIBits` 的目标尺寸被写成源的尺寸、
    /// 或源矩形算错），取样区再小也只是在面板角落画一小块 —— 用户看到的就是
    /// "倍率怎么拖都不变"。这条在**真 GDI DC** 上跑，不靠模拟。
    #[test]
    fn blit_scaled_blows_every_source_pixel_up_to_a_block() {
        let (pw, ph) = (64i32, 64i32);
        let (sw, sh) = (8i32, 8i32);
        let mut panel = Panel::create(pw, ph).expect("建面板缓冲");
        // 每个源像素一个**唯一**颜色：B 编码列、G 编码行、R 固定，任何错位都会露馅。
        let mut bgra = vec![0u8; (sw * sh * 4) as usize];
        for y in 0..sh {
            for x in 0..sw {
                let i = (y * sw + x) as usize * 4;
                bgra[i] = 0x10 + (x as u8) * 16;
                bgra[i + 1] = 0x10 + (y as u8) * 16;
                bgra[i + 2] = 0x80;
                bgra[i + 3] = 0xFF;
            }
        }
        let src = Shot { w: sw, h: sh, bgra };
        blit_scaled(&panel, &src);

        let px = panel.pixels().expect("面板缓冲可读");
        let z = pw / sw; // 目标块边长 = 8
        for (sx, sy) in [(0, 0), (3, 3), (4, 4), (7, 7)] {
            let i = (sy * sw + sx) as usize * 4;
            let want = (src.bgra[i], src.bgra[i + 1], src.bgra[i + 2]);
            let got = read_px(px, pw, sx * z + z / 2, sy * z + z / 2);
            assert_eq!(
                got, want,
                "源像素 ({sx},{sy}) 没被铺到目标块中心：期望 {want:?}，实际 {got:?}"
            );
        }
    }

    /// ★ 边框只动**周长一圈**：面板内部的画面**一个像素都不许改**。
    /// 否则每帧都会在图片上糊一层色，用户看到的是"面板里怪怪的"。
    #[test]
    fn border_only_touches_the_ring_and_keeps_the_core() {
        let (w, h) = (40, 24);
        let mut v = white_panel(w, h);
        let look = PipLook {
            border: Argb(0xFF_00_FF_00), // 不透明纯绿
            border_width: 3,
            crosshair_at: None,
            grid: None,
        };
        paint_decorations(&mut v, w, h, &look);

        // 边框上必须是绿的（BGRA 内存序里绿=中位）。
        let (b, g, r) = read_px(&v, w, 1, 1);
        assert_eq!((b, g, r), (0x00, 0xFF, 0x00), "边框应是纯绿");
        // 中心必须还是白的。
        let c = read_px(&v, w, 20, 12);
        assert_eq!(c, (0xFF, 0xFF, 0xFF), "中心不许被边框染到");
        // 环内侧一个像素（刚脱离边框）也必须没动。
        let inner = read_px(&v, w, 3, 12);
        assert_eq!(inner, (0xFF, 0xFF, 0xFF), "环内侧不许动");
    }

    /// 带 alpha 的边框必须**混合**而不是整层覆盖（默认的 `#00FF00C8` 就是半透明的）。
    ///
    ///   期望值按混合公式手算：`#80_00_00_FF`（半透明纯蓝）压在**白底**上
    ///   ⇒ R = G = 255×(1−128/255) ≈ 127，B 保持 255（蓝本身就满、白也满）。
    ///  判据取"既不是原样、也不是被完全替换"，两边都要卡住。
    #[test]
    fn a_translucent_border_blends_instead_of_replacing() {
        let (w, h) = (20, 20);
        let mut v = white_panel(w, h);
        let look = PipLook {
            border: Argb(0x80_00_00_FF), // 半透明纯蓝
            border_width: 2,
            crosshair_at: None,
            grid: None,
        };
        paint_decorations(&mut v, w, h, &look);
        let (b, g, r) = read_px(&v, w, 0, 0);
        assert_eq!(b, 0xFF, "蓝色通道本来就满，混合后不该变");
        assert!(g > 0x6F && g < 0x90, "绿色应被压到 ~127，实际 G=0x{g:02X}");
        assert!(r > 0x6F && r < 0x90, "红色应被压到 ~127，实际 R=0x{r:02X}");
    }

    /// 定位点**本身不画任何字形**（2026-10-02 用户口径"不要居中的光标"）：没有 `grid`
    /// 时，`crosshair_at` 传什么都不许在面板上留下一笔——它只是像素格的定位来源。
    #[test]
    fn crosshair_without_grid_draws_nothing() {
        let (w, h) = (60, 40);
        let mut v = white_panel(w, h);
        let off = PipLook {
            border_width: 0,
            ..PipLook::default()
        };
        assert!(off.crosshair_at.is_none(), "出厂默认就没有定位点");
        paint_decorations(&mut v, w, h, &off);
        // 只比颜色字节：`white_panel` 只写了 BGRA 的三个颜色通道，alpha 保持 0。
        assert!(
            v.chunks_exact(4)
                .all(|q| q[0] == 0xFF && q[1] == 0xFF && q[2] == 0xFF),
            "关掉定位点（也不画边框）时面板必须原样"
        );

        // 传了定位点但没有格：同样一个像素都不动。
        paint_decorations(
            &mut v,
            w,
            h,
            &PipLook {
                border_width: 0,
                crosshair_at: Some((12, 9)),
                ..off
            },
        );
        assert!(
            v.chunks_exact(4)
                .all(|q| q[0] == 0xFF && q[1] == 0xFF && q[2] == 0xFF),
            "没有格时定位点不许画出任何东西"
        );
    }

    /// 出厂外观：`border_width=2`、半透明边框、**不画准星**（准星开关在配置里）。
    #[test]
    fn default_look_matches_the_factory_preset() {
        let look = PipLook::default();
        assert_eq!(look.border_width, 2);
        assert!(
            look.crosshair_at.is_none(),
            "出厂不画准星——画中画的指针准星选项已整体删除"
        );
        assert_eq!(look.border.a(), 0xC8, "出厂边框是半透明的，别改成实色");
        assert!(look.grid.is_none(), "出厂不画像素格——那是截图放大镜的专属");
    }

    /// ★ 像素格：`(cols, rows)` 把面板均分，格线画在**分界上**（不含外缘），光标所在列/行
    ///   铺 50% 蓝十字定位线，所在格再描 2px 纯黑边（用户口径"中心一眼可辨、其它比较浅，
    ///   定位线要像屏幕标尺一样显眼"）。
    #[test]
    fn pixel_grid_draws_boundaries_rulers_and_outlines_the_cursor_cell() {
        let (w, h) = (60, 60);
        let mut v = white_panel(w, h);
        // 3×3 格，每格 20px；准星放 (50,50)，落在 col=row=2 的格（40..60）²。
        paint_decorations(
            &mut v,
            w,
            h,
            &PipLook {
                border_width: 0,
                crosshair_at: Some((50, 50)),
                grid: Some((3, 3)),
                ..PipLook::default()
            },
        );
        // ① 分界线在 x=20/40、y=20/40（60/3 整除，位置精确）：必须被染，且不再是纯白。
        for (x, y) in [(20, 10), (40, 10), (10, 20), (10, 40)] {
            assert_ne!(
                read_px(&v, w, x, y),
                (0xFF, 0xFF, 0xFF),
                "格线分界 ({x},{y}) 必须有格线"
            );
        }
        // ② 外缘不画格线（0 号与 cols 号分界 = 边框的活）。(5,0) 不在中心列/行的
        //    定位带上，纯白判定成立。
        assert_eq!(
            read_px(&v, w, 5, 0),
            (0xFF, 0xFF, 0xFF),
            "非定位带的面板外缘不该有格线"
        );
        // ③ 格内非分界、非定位行/列的区域必须保持原样。
        assert_eq!(
            read_px(&v, w, 10, 10),
            (0xFF, 0xFF, 0xFF),
            "格子内部（非分界线、非定位行列）不许被染"
        );
        // ④ 十字定位线：中心列（40..60）与中心行（40..60）的内部整片被 50% 蓝染过——
        //    白底上蓝分量最高、红被压得最低（BGR 内存序 read_px 返回 (B,G,R)？此处按
        //    三元组与白色比较 + 蓝主导判定，不依赖通道名）。
        for (x, y, what) in [
            (50, 10, "中心列上部"),
            (50, 30, "中心列中部"),
            (10, 50, "中心行左部"),
            (30, 50, "中心行中部"),
        ] {
            let c = read_px(&v, w, x, y);
            assert_ne!(c, (0xFF, 0xFF, 0xFF), "{what} 必须被定位线染 ({x},{y})");
            assert!(c.2 < 0xE0, "{what} 的红分量必须被蓝压低，实际 {c:?}");
        }
        // ⑤ 光标格描边：该格（40..60）² 的四条边必须是深色（黑，而非浅格线）。
        let edge = read_px(&v, w, 50, 40);
        assert!(
            edge.0 < 0x30 && edge.1 < 0x30 && edge.2 < 0x30,
            "光标格上边应是纯黑描边，实际 {edge:?}"
        );
        // ⑥ 光标格内部必须保持**原来的颜色**（2026-10-02 追加口径：定位线绕开中心格走，
        //    "否则这个格子我都看不清原来的颜色了"）——描边只动格边界。
        assert_eq!(
            read_px(&v, w, 45, 55),
            (0xFF, 0xFF, 0xFF),
            "中心格内部不许被定位线染色"
        );
    }

    /// 没有定位点时格子照画、但不铺定位线也不描中心格（crosshair 是中心格的定位来源）。
    #[test]
    fn pixel_grid_without_crosshair_draws_no_cell_outline() {
        let (w, h) = (60, 60);
        let mut v = white_panel(w, h);
        paint_decorations(
            &mut v,
            w,
            h,
            &PipLook {
                border_width: 0,
                crosshair_at: None,
                grid: Some((3, 3)),
                ..PipLook::default()
            },
        );
        // 格线仍在
        assert_ne!(read_px(&v, w, 20, 10), (0xFF, 0xFF, 0xFF), "格线必须还在");
        // 但没有纯黑描边：原先中心格边位置只该有浅格线（比 0x30 深）。此处无准星可定位，
        // 直接断言正中格的上边不存在深色线——浅格线本身在 (20,10) 已验过。
        let mid_top = read_px(&v, w, 10, 20);
        assert!(
            mid_top.0 > 0x30,
            "无准星时不得出现中心格纯黑描边，实际 {mid_top:?}"
        );
    }
}
