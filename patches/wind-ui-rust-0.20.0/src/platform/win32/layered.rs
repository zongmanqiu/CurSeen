use std::ffi::c_void;
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;

use tiny_skia::Pixmap;

use windows::Win32::Foundation::{HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteObject, GetDC, ReleaseDC, SelectObject,
    ValidateRect, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HGDIOBJ,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, UpdateLayeredWindow,
    GWL_EXSTYLE, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, ULW_ALPHA,
    WS_EX_TRANSPARENT,
};

use super::{AppHandler, WinRenderBackend};
use crate::geometry::Color;
use crate::platform::to_skia_color;
use crate::render::DibTarget;

/// CPU 软件渲染后端：**以 GDI DIB section 作为唯一后备缓冲**，通过 `UpdateLayeredWindow`
/// 呈现透明内容。
///
/// # 单缓冲（**CurSeen 补丁 015**）
///
/// 本后端曾经持有两块整屏缓冲：tiny-skia 的 `Pixmap`（RGBA）与 `UpdateLayeredWindow` 要的
/// DIB section（BGRA），每帧再做一次 RGBA→BGRA 整屏拷贝。两者内容完全相同，纯属重复——
/// 1080p 下白占 7.9MB，4K 下 33MB，正是 CurSeen 常驻内存里最大的一块。
///
/// 现在 tiny-skia 直接画进 DIB section 的内存（`tiny_skia::PixmapMut::from_bytes`，无对齐
/// 要求），只留一块缓冲；呈现顺序改为「先渲染进 DIB，再就地交换 R/B」——不再有第二块缓冲，
/// 也不再有跨缓冲的整屏拷贝。
///
/// R/B 交换**必须仍在**：tiny-skia 按 RGBA 写字节，而 GDI 的 32bpp 位图是 BGRA 字节序。
///
/// # 为什么"就地交换"是安全的
///
/// 交换会让缓冲在呈现后变成 BGRA，而 tiny-skia 下一帧会把它当 RGBA 读（混合需要读目标）。
/// 覆盖层的宿主（CurSeen）每帧**先整窗 `clear_rect` 再全量重画**，缓冲内容每帧都被完整改写，
/// 因此上一帧残留的字节序无关紧要。**若将来宿主改成局部重绘（脏区）**，这条前提就不成立：
/// 那时要么在交换后换回去，要么把脏区之外的部分按 BGRA 语义维护。
///
/// 与 `SkiaBackend` 的区别：
/// - 窗口必须带 `WS_EX_LAYERED`，用 `UpdateLayeredWindow` 而非 `BeginPaint`/`EndPaint` + `SetDIBitsToDevice`。
/// - 支持透明度：alpha 通道直接传递给窗口合成器。
/// - 支持运行时切换鼠标穿透（`WS_EX_TRANSPARENT`）。
pub(super) struct LayeredBackend {
    /// 休眠态的草稿缓冲：不占整屏内存，只为让 `handler.render` 照常跑（见
    /// [`Self::render_scratch`]）。尺寸固定，终生只分配一次。
    scratch: Option<Pixmap>,
    buf_w: i32,
    buf_h: i32,
    /// 缓冲刚（重）建，内容尚未画满：这一帧必须先把整块清零（否则看到的是 DIB 未定义内容）。
    fresh: bool,
    /// 预分配的 GDI 内存 DC，避免每帧 `CreateCompatibleDC`。
    mem_dc: Option<windows::Win32::Graphics::Gdi::HDC>,
    /// DIB section 句柄（唯一后备缓冲；需在尺寸变化时 `DeleteObject` 重建）。
    dib: HBITMAP,
    /// DIB section 的像素内存（top-down，BGRA 字节序，行距 = 宽 × 4）。
    dib_bits: *mut c_void,
    dib_w: i32,
    dib_h: i32,
}

// DIB section 的 bits 指针仅本线程使用，Send/Sync 无害。
unsafe impl Send for LayeredBackend {}
unsafe impl Sync for LayeredBackend {}

/// 覆盖层「休眠」开关（**CurSeen 补丁 007**）。
///
/// 分层覆盖层是**整屏**的，后备缓冲按「客户区面积 ×4 字节」线性增长：1080p 下 8 MB，
/// 4K 下 33 MB。而功能全关时覆盖层一个像素都不用画——那份缓冲纯粹是浪费（实测占 CurSeen
/// 空载私有内存的 60%）。休眠就是让后端在这种时候**不持有**它。
///
/// 语义：
/// - `true`：不分配缓冲。`paint` 只在「从活跃切到休眠」的那一帧把窗口刷成全透明
///   （否则最后呈现的那一帧会永久留在屏上），随后立即释放。
/// - `false`：正常按需分配。
///
/// 进程内全局而非按 HWND 区分：一个进程只会有一个分层覆盖层窗口，且开关是应用层的
/// 全局决策（"所有功能都关着"），不需要按窗口表达。
static PARKED: AtomicBool = AtomicBool::new(false);

/// 置覆盖层休眠状态。线程安全，任意线程可调用；下一次 `paint` 生效。
pub fn set_parked(parked: bool) {
    PARKED.store(parked, Ordering::Relaxed);
}

/// 覆盖层当前是否休眠。
pub fn is_parked() -> bool {
    PARKED.load(Ordering::Relaxed)
}

// ─────────────────────────────────────────────────────────────────────────────
// ★ 局部呈现（**CurSeen 补丁 032**）
//
// `UpdateLayeredWindow` 本就支持**子矩形更新**：`pptDst`/`psize` 给出比整窗小的一块、
// `ptSrc` 指向后备 DIB 里对应偏移，合成器就只重合成那一块，其余区域保留上一帧。
// 聚光灯这类"整屏恒定 + 一小块在动"的特效，脏区只有光圈附近两个圆（4K 下 <1% 像素），
// 整窗提交的 6.4ms 换序 + 合成开销可以整个省掉。
//
// 请求由宿主在 widget paint 期间经 [`request_partial_present`] 登记（设备像素，窗口
// 本地坐标系 = 后备 DIB 坐标系），本后端在 render 返回后取走消费；取走即清空，一次
// 一帧。缓冲代数 [`buffer_generation`] 供宿主判断"后备缓冲是否被重建过"（休眠释放、
// 尺寸变化都会重建）——重建后上一帧内容不复存在，宿主据此作废自己的增量缓存。
// ─────────────────────────────────────────────────────────────────────────────

/// 待消费的局部呈现请求 `(x, y, w, h)`，设备像素。
static PRESENT_DIRTY: Mutex<Option<(i32, i32, i32, i32)>> = Mutex::new(None);

/// 请求下一帧只呈现这块子矩形（设备像素）。每帧最多生效一次，`render` 返回后被取走。
pub fn request_partial_present(x: i32, y: i32, w: i32, h: i32) {
    if w <= 0 || h <= 0 {
        return;
    }
    if let Ok(mut slot) = PRESENT_DIRTY.lock() {
        *slot = Some((x, y, w, h));
    }
}

/// 取走（并清空）待消费的局部呈现请求。
fn take_partial_present() -> Option<(i32, i32, i32, i32)> {
    PRESENT_DIRTY.lock().ok().and_then(|mut s| s.take())
}

/// 后备缓冲代数：每次 DIB section 重建（尺寸变化）或释放（休眠）时 +1。
static BUFFER_GENERATION: AtomicU32 = AtomicU32::new(0);

/// 当前后备缓冲代数。宿主用它对账：代数变了 ⇒ 上一帧缓冲内容已不复存在。
pub fn buffer_generation() -> u32 {
    BUFFER_GENERATION.load(Ordering::Relaxed)
}

fn bump_generation() {
    BUFFER_GENERATION.fetch_add(1, Ordering::Relaxed);
}

impl LayeredBackend {
    pub(super) fn new() -> Self {
        Self {
            scratch: None,
            buf_w: 0,
            buf_h: 0,
            fresh: true,
            mem_dc: None,
            dib: HBITMAP(std::ptr::null_mut()),
            dib_bits: std::ptr::null_mut(),
            dib_w: 0,
            dib_h: 0,
        }
    }

    /// 释放后备缓冲（DIB section）。
    ///
    /// 休眠时用。把 `buf_w/h` 一并清零，这样下一次 [`Self::ensure`] 一定会重建。
    fn release(&mut self) {
        if !self.dib.0.is_null() {
            unsafe {
                let _ = DeleteObject(self.dib.into());
            }
            self.dib = HBITMAP(std::ptr::null_mut());
            self.dib_bits = std::ptr::null_mut();
        }
        self.buf_w = 0;
        self.buf_h = 0;
        self.dib_w = 0;
        self.dib_h = 0;
        self.fresh = true;
        bump_generation();
    }

    /// 确保 DIB section 匹配目标尺寸。
    fn ensure(&mut self, w: i32, h: i32) {
        let w = w.max(1);
        let h = h.max(1);
        // 休眠中：不分配。已有的缓冲留给 `paint` 先清屏再释放（直接丢会留下残影）。
        if is_parked() {
            return;
        }
        if self.buf_w == w && self.buf_h == h && !self.dib.0.is_null() {
            return;
        }

        if !self.dib.0.is_null() {
            unsafe {
                let _ = DeleteObject(self.dib.into());
            }
            self.dib = HBITMAP(std::ptr::null_mut());
            self.dib_bits = std::ptr::null_mut();
        }
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        match unsafe {
            CreateDIBSection(
                None,
                &bmi as *const BITMAPINFO,
                DIB_RGB_COLORS,
                &mut bits as *mut *mut c_void,
                None,
                0,
            )
        } {
            Ok(bitmap) => {
                self.dib = bitmap;
                self.dib_bits = bits;
                self.dib_w = w;
                self.dib_h = h;
                self.buf_w = w;
                self.buf_h = h;
            }
            Err(_) => {
                eprintln!("[curseen] CreateDIBSection 失败");
                self.dib = HBITMAP(std::ptr::null_mut());
                self.dib_bits = std::ptr::null_mut();
                self.dib_w = 0;
                self.dib_h = 0;
                self.buf_w = 0;
                self.buf_h = 0;
            }
        }

        self.fresh = true;
        bump_generation();
    }

    /// 后备缓冲的字节视图（长度 = 宽 × 高 × 4）。
    ///
    /// 缓冲缺失时返回 `None`（建造失败或已释放）——调用方据此跳过这一帧的绘制。
    fn dib_slice(&mut self) -> Option<&mut [u8]> {
        if self.dib_bits.is_null() || self.dib_w <= 0 || self.dib_h <= 0 {
            return None;
        }
        let len = (self.dib_w as usize) * (self.dib_h as usize) * 4;
        // Safety: `dib_bits` 由 `CreateDIBSection` 返回，指向本进程内 `dib` 拥有的、至少
        // `len` 字节的可写内存；`dib` 存活期间地址不变（尺寸变化走 `ensure` 整体重建，
        // 旧 buffer 在重建前已被 `DeleteObject`，而 `&mut self` 保证此处无别名借用）。
        Some(unsafe { std::slice::from_raw_parts_mut(self.dib_bits as *mut u8, len) })
    }

    /// 休眠态的「空转」渲染。
    ///
    /// 休眠不占整屏缓冲，但 `handler.render` **必须照跑**：`App::channel` 投递的意图、
    /// 周期回调的副作用、控件树的 `on_update` 全挂在这条路径上（`UiHost::render` 里排空
    /// pump）。跳掉它，功能全关时任何一条意图消息——比如托盘点「打开控制面板」——都会
    /// 永远卡在队列里：消息唤醒的那一帧走到 `paint` 就直接返回了。
    ///
    /// 所以这里渲染进一块固定大小的草稿缓冲：**布局仍按真实窗口尺寸走**（宿主据此算
    /// 布局、跑逻辑），绘制指令则被画布裁掉。逻辑完整、代价只有几百字节。
    fn render_scratch(&mut self, handler: &mut dyn AppHandler, size: crate::geometry::Size) {
        if self.scratch.is_none() {
            // 不用 1×1：极端尺寸容易踩到光栅化里的退化分支，8×8 足够小也足够正常。
            self.scratch = Pixmap::new(8, 8);
        }
        let Some(pm) = self.scratch.as_mut() else {
            return;
        };
        let mut tgt = crate::render::PixmapTarget { pixmap: pm };
        handler.render(&mut tgt, size);
    }
}

/// 就地交换每个像素的 R 与 B（RGBA ↔ BGRA 字节序转换）——**CurSeen 补丁 015**。
///
/// 取代了补丁 013 的跨缓冲逐行 `copy_swizzled`（那时是「读 Pixmap、写 DIB」两处内存，
/// 现为同一处的读+写）。预乘 alpha 下交换 R/B 是安全的：预乘按通道乘同一个 alpha，
/// 通道之间没有耦合，交换顺序不影响结果。
///
/// 逐字节而非 `u32` 的 unaligned 读写：`CreateDIBSection` 返回的地址通常已 4 字节对齐，
/// 但 tiny-skia 侧契约明确"不保证对齐"，逐字节版本在两个方向上都正确，也最容易向量化。
fn swizzle_rb_inplace(data: &mut [u8]) {
    for px in data.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
}

/// 区域版 [`swizzle_rb_inplace`]（**CurSeen 补丁 032**）：只交换 `(x, y, w, h)` 内的
/// R/B，越界自动裁掉。独立成自由函数便于单测（不必真的建 DIB）。
fn swizzle_region_in_place(
    data: &mut [u8],
    stride: usize,
    buf_h: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) {
    let bw = (stride / 4) as i32;
    let x0 = x.clamp(0, bw);
    let y0 = y.clamp(0, buf_h);
    let x1 = (x + w).clamp(0, bw);
    let y1 = (y + h).clamp(0, buf_h);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    for row in y0..y1 {
        let start = row as usize * stride + x0 as usize * 4;
        let end = row as usize * stride + x1 as usize * 4;
        swizzle_rb_inplace(&mut data[start..end]);
    }
}

impl WinRenderBackend for LayeredBackend {
    fn resize(&mut self, w: i32, h: i32) {
        self.ensure(w, h);
    }

    /// 渲染并呈现一帧到分层窗口。
    ///
    /// 链路：tiny-skia 直接画进 DIB section → 就地换 R/B → `UpdateLayeredWindow` 合成。
    /// 不调用 `BeginPaint`/`EndPaint`——`UpdateLayeredWindow` 是自包含的。
    ///
    /// 休眠中（[`is_parked`]）走单独分支：把窗口刷成全透明后释放缓冲，`handler.render`
    /// 只渲染进草稿缓冲（见 [`Self::render_scratch`]），空闲时的整屏开销因此彻底为 0。
    unsafe fn paint(&mut self, hwnd: HWND, bg: Color, handler: &mut dyn AppHandler) -> bool {
        // 取客户区尺寸
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        let w = rc.right - rc.left;
        let h = rc.bottom - rc.top;
        if w <= 0 || h <= 0 {
            // 0×0（最小化）无内容可呈现，但**更新区仍须校验**：不校验则 WM_PAINT
            // 被反复合成，见本文件末尾的长注释。
            let _ = ValidateRect(Some(hwnd), None);
            return false;
        }

        if is_parked() {
            // 过渡帧：先把整块刷成透明再呈现，然后释放。跳过这一步的话，最后一次
            // `UpdateLayeredWindow` 的内容会一直挂在屏上——窗口还在，只是没人再更新它。
            // 局部呈现请求一并作废：缓冲都要释放了，没有"局部"可言。
            let _ = take_partial_present();
            if !self.dib.0.is_null() {
                if let Some(buf) = self.dib_slice() {
                    buf.fill(0);
                }
                self.present(hwnd);
            }
            self.release();
            // 仍然跑一次宿主渲染：通道消息的排空、周期回调、控件 `on_update` 都靠它。
            // 少了这一句，功能全关时托盘点"打开控制面板"的意图会永远卡在队列里。
            self.render_scratch(handler, crate::geometry::Size::new(w, h));
            let _ = ValidateRect(Some(hwnd), None);
            return false;
        }

        self.ensure(w, h);

        let size = crate::geometry::Size::new(self.buf_w, self.buf_h);
        let clear = to_skia_color(bg);
        let fresh = self.fresh;
        let (bw, bh) = (self.buf_w as u32, self.buf_h as u32);
        let Some(buf) = self.dib_slice() else {
            let _ = ValidateRect(Some(hwnd), None);
            return false;
        };

        // 新缓冲清底（DIB 内存未初始化，不清会看到垃圾）。走 tiny-skia 的 fill，
        // 免得手写一遍预乘 alpha 的字节换算。
        if fresh {
            if let Some(mut pm) = tiny_skia::PixmapMut::from_bytes(buf, bw, bh) {
                pm.fill(clear);
            }
        }

        // 渲染（tiny-skia 直接写进 DIB 内存）
        {
            let mut tgt = DibTarget::new(buf, bw, bh);
            handler.render(&mut tgt, size);
        }
        self.fresh = false;

        // RGBA → BGRA（就地）。★ 补丁 032（修订）：宿主登记了局部脏区时，只换序
        // 脏区——缓冲其余部分上一帧已经换过序、且这一帧没人碰它（增量绘制的前提
        // 就是"只画脏区"），再整屏换一遍反而会把它们变回 RGBA 语义。
        // fresh 帧（缓冲刚重建）没有"上一帧"可保留，必须整屏换序。
        //
        // ★★ 呈现**恒为整窗**：`UpdateLayeredWindow` 的 `psize` 是**窗口自身的
        // 尺寸**，不是"更新区"——分层窗不支持子矩形更新。首版把脏区递给 psize，
        // 结果每个增量帧把整个覆盖层窗口缩小成那一小块，其余内容全部消失
        // （2026-09-20 真机回归）。脏区外的字节本就是上一帧的 BGRA、逐字节正确，
        // 整窗 ULW 只是把它们原样再交一次给合成器。
        let dirty = if fresh { None } else { take_partial_present() };
        match dirty {
            Some((dx, dy, dw, dh)) if dw > 0 && dh > 0 => {
                self.swizzle_region(dx, dy, dw, dh);
            }
            _ => {
                if let Some(buf) = self.dib_slice() {
                    swizzle_rb_inplace(buf);
                }
            }
        }
        self.present(hwnd);

        let _ = ValidateRect(Some(hwnd), None);
        false // layered 后端永不失败
    }
}

impl LayeredBackend {
    /// 把 DIB section 内容 `UpdateLayeredWindow` 呈现。
    ///
    /// 从 `paint` 里拆出来，好让「休眠过渡帧」复用同一条呈现链路。缓冲缺失（建造失败
    /// 或已释放）时静默返回——呈现不了比崩掉好。
    unsafe fn present(&mut self, hwnd: HWND) {
        if self.buf_w <= 0 || self.buf_h <= 0 || self.dib_bits.is_null() {
            return;
        }

        // UpdateLayeredWindow
        // ★ 补丁 008：`hdcDst` 必须是**屏幕 DC**（`GetDC(NULL)`），不是窗口 DC。
        // 传 `GetDC(hwnd)` 时首帧侥幸通过，之后每帧 `E_INVALIDARG(0x80070057)`——
        // 覆盖层从此定格在第一帧的全透明内容，任何后续绘制都上不了屏（表现为
        // "所有鼠标特效/按键显示都看不见"，而绘制链路一切正常）。
        let hdc_screen = GetDC(None);
        let hdc_mem = match self.mem_dc {
            Some(dc) => dc,
            None => {
                let dc = CreateCompatibleDC(Some(hdc_screen));
                self.mem_dc = Some(dc);
                dc
            }
        };
        let old_bmp = SelectObject(hdc_mem, HGDIOBJ(self.dib.0));

        let pt_src = POINT { x: 0, y: 0 };
        let mut size_win = SIZE {
            cx: self.buf_w,
            cy: self.buf_h,
        };
        let blend = windows::Win32::Graphics::Gdi::BLENDFUNCTION {
            BlendOp: AC_SRC_OVER,
            BlendFlags: 0,
            SourceConstantAlpha: 255, // 不透明
            AlphaFormat: AC_SRC_ALPHA,
        };
        let _ = UpdateLayeredWindow(
            hwnd,
            Some(hdc_screen),
            None,
            Some(&mut size_win as *const SIZE),
            Some(hdc_mem),
            Some(&pt_src as *const POINT),
            windows::Win32::Foundation::COLORREF(0),
            Some(&blend as *const windows::Win32::Graphics::Gdi::BLENDFUNCTION),
            ULW_ALPHA,
        );

        SelectObject(hdc_mem, old_bmp);
        ReleaseDC(None, hdc_screen);
    }

    /// 只交换子矩形 `(x, y, w, h)` 内的 R/B（**CurSeen 补丁 032**）。
    ///
    /// 与整屏 `swizzle_rb_inplace` 的结果在脏区内逐字节一致——增量帧的前提就是
    /// "脏区外的字节与上一呈现帧完全相同"，它们已处于 BGRA 语义，绝不能重碰。
    /// 越界部分自动裁掉。
    fn swizzle_region(&mut self, x: i32, y: i32, w: i32, h: i32) {
        let bw = self.dib_w;
        let bh = self.dib_h;
        let Some(buf) = self.dib_slice() else {
            return;
        };
        swizzle_region_in_place(buf, bw as usize * 4, bh, x, y, w, h);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ★ 校验更新区（**必须**，补丁 003）。`UpdateLayeredWindow` 只负责把像素交给合成器，
// 它 *不* 校验窗口的更新区——更新区只有 `BeginPaint`/`EndPaint`/`ValidateRect` 才会被
// 清掉。不校验的后果不是"多画几帧"，而是**死循环烧满一个核**：
//
//   1. 更新区非空 ⇒ 系统在消息队列里合成一条 WM_PAINT；
//   2. 平台消息泵的动画分支用 `PeekMessageW(PM_REMOVE)` **非阻塞排空**队列
//      （见 `run_message_loop`）——它把这条 WM_PAINT 取走派发，本函数呈现一帧，
//      但更新区依旧非空；
//   3. 于是下一条 WM_PAINT 立刻被合成，`PeekMessageW` 再次返回 TRUE……
//      `while` 永不退出。`QS_ALLINPUT` 含 `QS_PAINT`，连 `MsgWaitForMultipleObjectsEx`
//      也会立刻返回，整轮退化成忙等。
//
// 实测：窗口静止时 ~87.8 万次整屏 `UpdateLayeredWindow`／10 秒（≈88k 帧/秒），单个 UI
// 线程吃满一个核，而画面纹丝不动。
//
// 校验范围取**整窗**而非 rcPaint 子矩形：分层后端的呈现颗粒度本就是整窗，不存在"只重传
// 了一部分"的状态，整窗校验才是事实。
// ─────────────────────────────────────────────────────────────────────────────

/// AC_SRC_OVER = 0（源覆盖目标）
const AC_SRC_OVER: u8 = 0;
/// AC_SRC_ALPHA = 1（源含预乘 alpha）
const AC_SRC_ALPHA: u8 = 1;

/// 运行时切换鼠标穿透。
///
/// ★ 补丁 008：**绝不能**在这里调用 `SetLayeredWindowAttributes`。
///
/// 它与 `UpdateLayeredWindow` 是 Windows 上两种**互斥**的分层窗口模式：一旦调用，
/// 窗口就切到 "SetLayeredWindowAttributes 模式"，此后所有 `UpdateLayeredWindow`
/// 一律返回 `E_INVALIDARG(0x80070057)`。症状极具迷惑性——第一帧还能呈现（那时还没
/// 切换），之后覆盖层永远定格在第一帧的全透明内容，**任何后续绘制都上不了屏**：
/// 所有鼠标特效、按键显示、屏幕书写全看不见，而绘制链路（画布 → pixmap）完全正常。
///
/// 改完 `WS_EX_TRANSPARENT` 只需 `SetWindowPos` + `SWP_FRAMECHANGED` 让新样式生效，
/// 不必也不许碰分层属性。
pub(crate) unsafe fn toggle_click_through(hwnd: HWND, enabled: bool) {
    let before = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
    let mut style = before;
    if enabled {
        style |= WS_EX_TRANSPARENT.0 as isize;
    } else {
        style &= !(WS_EX_TRANSPARENT.0 as isize);
    }
    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style);
    // 刷新非客户区/扩展样式，使穿透状态立即生效。
    let _ = SetWindowPos(
        hwnd,
        None,
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 区域换序必须与整屏换序在脏区内逐字节一致、脏区外逐字节不动（补丁 032）。
    #[test]
    fn region_swizzle_matches_full_swizzle_inside_and_preserves_outside() {
        let (w, h) = (17i32, 11i32);
        let mut full = vec![0u8; (w * h * 4) as usize];
        let mut part = vec![0u8; full.len()];
        for (i, b) in full.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        part.copy_from_slice(&full);

        // 待测：对 part 做三次区域换序（含负坐标 / 越界，验证裁剪）。
        swizzle_region_in_place(&mut part, w as usize * 4, h, 5, 3, 8, 6);
        swizzle_region_in_place(&mut part, w as usize * 4, h, -10, -10, 14, 13);
        swizzle_region_in_place(&mut part, w as usize * 4, h, 15, 9, 40, 40);

        // 参照：同样的三块区域，用整屏换序逐行模拟（逐字节语义应完全一致）。
        for (rx, ry, rw, rh) in [(5, 3, 8, 6), (-10, -10, 14, 13), (15, 9, 40, 40)] {
            let x0 = rx.clamp(0, w);
            let y0 = ry.clamp(0, h);
            let x1 = (rx + rw).clamp(0, w);
            let y1 = (ry + rh).clamp(0, h);
            for row in y0..y1 {
                let start = row as usize * w as usize * 4 + x0 as usize * 4;
                let end = row as usize * w as usize * 4 + x1 as usize * 4;
                swizzle_rb_inplace(&mut full[start..end]);
            }
        }

        assert_eq!(
            full, part,
            "区域换序（含越界裁剪）必须与逐行手工换序逐字节一致"
        );
    }

    /// 完全出界的区域是空操作（不得 panic、不得改字节）。
    #[test]
    fn out_of_bounds_region_is_a_noop() {
        let (w, h) = (4i32, 4i32);
        let mut buf = vec![7u8; (w * h * 4) as usize];
        let snapshot = buf.clone();
        swizzle_region_in_place(&mut buf, w as usize * 4, h, 100, 100, 10, 10);
        assert_eq!(buf, snapshot);
    }
}
