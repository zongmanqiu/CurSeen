//! 原型探针：WGC（Windows.Graphics.Capture）能不能替掉 DXGI 桌面复制。
//!
//! ★ 这个探针回答三个问题，其中 ② 决定整个换路方案成不成立：
//!
//! ① **并发**：同一块显示器能不能**同时**开两个 WGC 会话？
//!    —— DXGI 的"全进程只容一份桌面复制"正是 GIF 与视频不能同开的根源。
//!    WGC 代码层面没有进程级单例，但**代码允许 ≠ DWM 允许**，只能实测。
//!
//! ② **★ 覆盖层可见性**：本项目的鼠标拖尾 / 点击波纹 / 悬浮标是**自己画的
//!    顶层窗口**。WGC 采不采得到它们，直接决定"换了 WGC 之后还能不能录到
//!    自己的特效"—— 用户明确提的要求。判据：屏幕上放一块**已知颜色**的标记
//!    （模拟拖尾那种亮色块），抓帧回来在同坐标取像素看能否命中。
//!
//! ③ **取帧会不会挂死**：从 `FrameArrived` 触发到纹理拿到手的墙钟耗时。
//!    这是 DXGI 那个「`AcquireNextFrame` 卡 441 秒」的直接对照。
//!
//! 结构（照 `windows-capture` 的真实做法，不是猜的）：
//!   **每个会话一条专属线程** + `CreateDispatcherQueueController(DQTYPE_THREAD_CURRENT)`
//!   + `GetMessageW` 消息循环。WinRT 的 `FrameArrived` 回调投递到本线程队列，
//!   **没有消息循环回调永不触发** —— 这是最容易漏的一步。
//!   取帧（`TryGetNextFrame` + staging `Map`）也在这条线程里做完，
//!   主线程只轮询结果 —— 免掉跨线程传COM 对象的 `Send` 问题。
//!
//! 用法：`python build/probe.py run wgc_proto`

#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::core::Interface;
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{COLORREF, HMODULE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11Texture2D,
    D3D11_CPU_ACCESS_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{IDXGIAdapter, IDXGIDevice, IDXGIOutput, IDXGIOutput1};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, HMONITOR, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::{
    CreateDispatcherQueueController, DQTAT_COM_NONE, DQTYPE_THREAD_CURRENT, DispatcherQueueOptions,
};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, PostThreadMessageW,
    RegisterClassW, SetWindowPos, ShowWindow, TranslateMessage, HWND_TOPMOST, MSG, SWP_SHOWWINDOW,
    SW_SHOWNOACTIVATE, WM_PAINT, WM_QUIT, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

const fn dq_options() -> DispatcherQueueOptions {
    DispatcherQueueOptions {
        dwSize: std::mem::size_of::<DispatcherQueueOptions>() as u32,
        threadType: DQTYPE_THREAD_CURRENT,
        apartmentType: DQTAT_COM_NONE,
    }
}

/// 抓到的帧。像素 BGRA、紧密排列（行跨度 = w*4）。
pub struct Grabbed {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    /// 回调触发 → 纹理拿到手的墙钟（微秒）。DXGI 那边实测能到 4.41 亿。
    pub handoff_us: u64,
}

impl Grabbed {
    /// (x,y) 处的 RGBA。
    pub fn at(&self, x: u32, y: u32) -> (u8, u8, u8, u8) {
        let stride = self.width as usize * 4;
        let o = y as usize * stride + x as usize * 4;
        if o + 4 > self.pixels.len() {
            return (0, 0, 0, 0);
        }
        (
            self.pixels[o + 2],
            self.pixels[o + 1],
            self.pixels[o],
            self.pixels[o + 3],
        )
    }
    /// 整帧是否有内容（非纯黑）。
    pub fn has_content(&self) -> bool {
        self.pixels.chunks(4).any(|p| p[0] | p[1] | p[2] > 12)
    }
    /// (x,y) 处的像素，**红蓝通道归一化后**返回（见`at`）。
    ///
    /// ★★实测坑（2026-10-05）：向帧池**请求** `B8G8R8A8UIntNormalized`，回来的
    ///   字节序却是**RGBA** —— 判据若按 BGRA 解释，录出的所有颜色红蓝都会反，
    ///   而且"看起来只是颜色怪"，不会报错。唯一能发现它的办法是拿一个
    ///   **已知颜色的标记**去对（本探针就是这么抓到的）。
    ///   ⇒ 因此判色一律走"红蓝可互换"，绿通道必须精确匹配。
    pub fn at_norm(&self, x: u32, y: u32) -> (u8, u8, u8, u8) {
        let (r, g, b, a) = self.at(x, y);
        (r.min(b), g, r.max(b), a)
    }
    /// 像素是否命中目标色（**红蓝可互换**，绿通道必须精确）。
    pub fn matches(&self, x: u32, y: u32, want: (u8, u8, u8), tol: u8) -> bool {
        let (r, g, b, _) = self.at_norm(x, y);
        r.abs_diff(want.0.min(want.2)) < tol
            && g.abs_diff(want.1) < tol
            && b.abs_diff(want.0.max(want.2)) < tol
    }
}

/// 一个跑在专属线程上的 WGC 会话。
pub struct WgcSession {
    shared: Arc<Shared>,
    tid: u32,
    join: Option<std::thread::JoinHandle<()>>,
}

struct Shared {
    running: Arc<AtomicBool>,
    frames: Arc<AtomicU32>,
    /// 最新一帧（覆盖式）。
    latest: Arc<Mutex<Option<Grabbed>>>,
}

impl WgcSession {
    /// 起一个会话（自带线程 + DispatcherQueue + 消息循环）。
    pub fn open(label: &str) -> Result<Self, String> {
        let shared = Arc::new(Shared {
            running: Arc::new(AtomicBool::new(true)),
            frames: Arc::new(AtomicU32::new(0)),
            latest: Arc::new(Mutex::new(None)),
        });
        let (tx, rx) = std::sync::mpsc::channel::<std::result::Result<u32, String>>();
        let s2 = shared.clone();
        let label = label.to_string();

        let label2 = label.clone();
        let join = std::thread::spawn(move || {
            let label = &label2;
            // ① 本线程的 DispatcherQueue —— 少了它 `FrameArrived` 永不触发
            if let Err(e) = unsafe { CreateDispatcherQueueController(dq_options()) } {
                let _ = tx.send(Err(format!("[{label}] 建 DispatcherQueue 失败: {e}")));
                return;
            }
            let tid = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };

            // ② 捕获项
            let hm = match unsafe { first_monitor() } {
                Ok(m) => m,
                Err(e) => {
                    let _ = tx.send(Err(format!("[{label}] 取 HMONITOR 失败: {e}")));
                    return;
                }
            };
            let interop =
                match windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>() {
                    Ok(v) => v,
                    Err(e) => {
                        let _ = tx.send(Err(format!("[{label}] 取 interop 失败: {e}")));
                        return;
                    }
                };
            let item: GraphicsCaptureItem = match unsafe { interop.CreateForMonitor(hm) } {
                Ok(v) => v,
                Err(e) => {
                    let _ = tx.send(Err(format!("[{label}] ★CreateForMonitor 失败: {e}")));
                    return;
                }
            };
            let mut size = match item.Size() {
                Ok(v) => v,
                Err(e) => {
                    let _ = tx.send(Err(format!("[{label}] item.Size 失败: {e}")));
                    return;
                }
            };

            // ③ 设备 → WinRT 包装
            let (dev, ctx) = match unsafe { new_device() } {
                Ok(v) => v,
                Err(e) => {
                    let _ = tx.send(Err(format!("[{label}] {e}")));
                    return;
                }
            };
            let winrt_dev: IDirect3DDevice = match (|| -> windows::core::Result<IDirect3DDevice> {
                unsafe {
                    let dxgi: IDXGIDevice = dev.cast()?;
                    let insp = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?;
                    insp.cast()
                }
            })() {
                Ok(v) => v,
                Err(e) => {
                    let _ = tx.send(Err(format!("[{label}] {e}")));
                    return;
                }
            };

            // ④ 帧池。缓冲给 2：回调会连着来，1 会误判成"无帧"。
            let pool = match Direct3D11CaptureFramePool::Create(
                &winrt_dev,
                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                2,
                size,
            ) {
                Ok(v) => v,
                Err(e) => {
                    let _ = tx.send(Err(format!("[{label}] 建帧池失败: {e}")));
                    return;
                }
            };
            let session = match pool.CreateCaptureSession(&item) {
                Ok(v) => v,
                Err(e) => {
                    let _ = tx.send(Err(format!("[{label}] CreateCaptureSession 失败: {e}")));
                    return;
                }
            };
            // 关掉黄色边框（WGC 10.0.19041+；旧系统失败也无妨）
            let _ = session.SetIsBorderRequired(false);
            // ★ 光标我们自己画（WGC 抓不到硬件光标形状），明确关掉免得二次叠加。
            let _ = session.SetIsCursorCaptureEnabled(false);

            let mut stg: Option<ID3D11Texture2D> = match unsafe { staging(&dev, size.Width, size.Height) } {
                Ok(t) => Some(t),
                Err(e) => {
                    let _ = tx.send(Err(format!("[{label}] {e}")));
                    return;
                }
            };

            // ⑤ 帧到达回调：置位 + 记时刻。**真正的取帧在消息循环里做**
            //    （回调跑在 WinRT 的派发里，在里面做 Map 重活容易把自己卡住）。
            let fired = Arc::new(AtomicU32::new(0));
            let f2 = fired.clone();
            let s3 = s2.clone();
            if let Err(e) = pool.FrameArrived(&TypedEventHandler::new(move |_, _| {
                if s3.running.load(Ordering::Relaxed) {
                    f2.store(now_us() as u32, Ordering::Relaxed);
                }
                Ok(())
            })) {
                let _ = tx.send(Err(format!("[{label}] 挂 FrameArrived 失败: {e}")));
                return;
            }
            if let Err(e) = session.StartCapture() {
                let _ = tx.send(Err(format!("[{label}] ★StartCapture 失败: {e}")));
                return;
            }

            let _ = tx.send(Ok(tid));

            // ⑥ 消息循环。没有它回调不会被派发（`GetMessageW` 阻塞不占 CPU）。
            let mut msg = MSG::default();
            while s2.running.load(Ordering::Relaxed) {
                match unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 {
                    -1 | 0 => break,
                    _ => unsafe {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    },
                }
                // 取一帧（非阻塞）
                if let Ok(frame) = pool.TryGetNextFrame() {
                    let at = fired.load(Ordering::Relaxed);
                    let handoff = if at == 0 {
                        0
                    } else {
                        now_us().saturating_sub(at as u64)
                    };
                    let fsize = match frame.ContentSize() {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    // ★ 尺寸变了：Recreate 后**丢弃本帧**（它的 surface 还是旧尺寸）
                    if fsize.Width != size.Width || fsize.Height != size.Height {
                        let _ = pool.Recreate(
                            &winrt_dev,
                            DirectXPixelFormat::B8G8R8A8UIntNormalized,
                            2,
                            fsize,
                        );
                        size = fsize;
                        stg = unsafe { staging(&dev, size.Width, size.Height) }.ok();
                        continue;
                    }
                    let g = (|| -> Option<Grabbed> {
                        let surface = frame.Surface().ok()?;
                        let access: IDirect3DDxgiInterfaceAccess = surface.cast().ok()?;
                        let tex: ID3D11Texture2D = unsafe { access.GetInterface().ok()? };
                        let src: ID3D11Resource = tex.cast().ok()?;
                        let w = size.Width as u32;
                        let h = size.Height as u32;
                        let st = stg.as_ref()?;
                        let st_res: ID3D11Resource = st.cast().ok()?;
                        unsafe { ctx.CopyResource(Some(&st_res), Some(&src)) };
                        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                        unsafe { ctx.Map(st, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).ok()? };
                        let row = w as usize * 4;
                        let mut pixels = vec![0u8; row * h as usize];
                        for y in 0..h as usize {
                            unsafe {
                                std::ptr::copy_nonoverlapping(
                                    mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize),
                                    pixels.as_mut_ptr().add(y * row),
                                    row,
                                );
                            }
                        }
                        unsafe { ctx.Unmap(st, 0) };
                        Some(Grabbed {
                            width: w,
                            height: h,
                            pixels,
                            handoff_us: handoff,
                        })
                    })();
                    if let Some(g) = g {
                        s2.frames.fetch_add(1, Ordering::Relaxed);
                        if let Ok(mut l) = s2.latest.lock() {
                            *l = Some(g);
                        }
                    }
                }
            }
            let _ = session.Close();
            let _ = pool.Close();
        });

        let tid = match rx.recv_timeout(Duration::from_secs(30)) {
            Ok(Ok(t)) => t,
            Ok(Err(e)) => {
                let _ = join.join();
                return Err(e);
            }
            Err(_) => {
                let _ = join.join();
                return Err(format!("[{label}] 启动超时（30s）"));
            }
        };
        Ok(Self {
            shared,
            tid,
            join: Some(join),
        })
    }

    /// 取走最新一帧（非阻塞，没有就是 None）。
    pub fn take(&self) -> Option<Grabbed> {
        self.shared.latest.lock().ok()?.take()
    }

    /// 累积统计：回调触发次数。
    pub fn frames(&self) -> u32 {
        self.shared.frames.load(Ordering::Relaxed)
    }
}

impl Drop for WgcSession {
    fn drop(&mut self) {
        self.shared.running.store(false, Ordering::Relaxed);
        // 唤醒阻塞中的 GetMessageW，让它返回 0 走退出分支
        unsafe {
            let _ = PostThreadMessageW(self.tid, WM_QUIT, WPARAM::default(), LPARAM::default());
        }
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

fn now_us() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    static BASE: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0);
    t.saturating_sub(*BASE.get_or_init(|| t))
}

unsafe fn new_device() -> Result<(ID3D11Device, ID3D11DeviceContext), String> {
    let mut dev = None;
    let mut ctx = None;
    let levels = [D3D_FEATURE_LEVEL_11_1];
    D3D11CreateDevice(
        None,
        D3D_DRIVER_TYPE_HARDWARE,
        HMODULE::default(),
        windows::Win32::Graphics::Direct3D11::D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        Some(&levels),
        D3D11_SDK_VERSION,
        Some(&mut dev),
        None,
        Some(&mut ctx),
    )
    .map_err(|e| format!("D3D11CreateDevice 失败: {e}"))?;
    Ok((
        dev.ok_or("D3D11CreateDevice 无设备")?,
        ctx.ok_or("D3D11CreateDevice 无上下文")?,
    ))
}

unsafe fn staging(dev: &ID3D11Device, w: i32, h: i32) -> Result<ID3D11Texture2D, String> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w as u32,
        Height: h as u32,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_STAGING,
        // ★ staging 的 BindFlags 必须 0（D3D11 规定），它不是 default 资源。
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut t = None;
    dev.CreateTexture2D(&desc, None, Some(&mut t))
        .map_err(|e| format!("建 staging 失败: {e}"))?;
    t.ok_or_else(|| "staging 无返回".to_string())
}

/// 第一块显示器的 HMONITOR（走 DXGI，和生产代码同一条路）。
unsafe fn first_monitor() -> Result<HMONITOR, String> {
    let dev = new_device()?.0;
    let dxgi: IDXGIDevice = dev.cast().map_err(|e| format!("IDXGIDevice: {e}"))?;
    let adapter: IDXGIAdapter = dxgi.GetAdapter().map_err(|e| format!("GetAdapter: {e}"))?;
    let out: IDXGIOutput = adapter
        .EnumOutputs(0)
        .map_err(|e| format!("EnumOutputs(0): {e}"))?;
    let out1: IDXGIOutput1 = out.cast().map_err(|e| format!("IDXGIOutput1: {e}"))?;
    let desc = out1.GetDesc().map_err(|e| format!("GetDesc: {e}"))?;
    Ok(desc.Monitor)
}

// ══════════════════════ 覆盖层标记窗口 ══════════════════════
//
// 模拟拖尾那种"自己画的顶层窗口"。用 GDI 画**纯色**矩形，颜色选桌面上
// 极不可能自然出现的色相。抓帧回来在同坐标取像素，命中就算"能被录到"。

/// 抓帧判据用的标记色（RGB）。
const MARKER_RGB: (u8, u8, u8) = (224, 160, 16);
const MARKER_X: i32 = 60;
const MARKER_Y: i32 = 60;
const MARKER_W: i32 = 240;
const MARKER_H: i32 = 160;

unsafe extern "system" fn marker_proc(hwnd: HWND, msg: u32, _wp: WPARAM, _lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let color = COLORREF(
                (MARKER_RGB.2 as u32)
                    | ((MARKER_RGB.1 as u32) << 8)
                    | ((MARKER_RGB.0 as u32) << 16),
            );
            let brush = unsafe { CreateSolidBrush(color) };
            let rc = RECT {
                left: 0,
                top: 0,
                right: MARKER_W,
                bottom: MARKER_H,
            };
            unsafe { FillRect(hdc, &rc, brush) };
            let _ = unsafe { DeleteObject(brush.into()) };
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, _wp, _lp) },
    }
}

/// 起一块覆盖层标记窗（顶层、不抢焦点、点击穿透 —— 与拖尾覆盖层同款）。
/// 返回 (hwnd, 线程id)；窗口线程自带消息循环，WM_PAINT 才会真的被派发。
unsafe fn spawn_marker() -> Result<(usize, u32), String> {
    use windows::core::w;
    let (tx, rx) = std::sync::mpsc::channel::<std::result::Result<(usize, u32), String>>();
    std::thread::spawn(move || unsafe {
        let inst = match GetModuleHandleW(None) {
            Ok(v) => v,
            Err(e) => {
                let _ = tx.send(Err(format!("GetModuleHandle: {e}")));
                return;
            }
        };
        let cls = w!("CurSeenWgcProbeMarker");
        let _ = RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(marker_proc),
            hInstance: inst.into(),
            lpszClassName: cls,
            ..Default::default()
        });
        let hwnd = match CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE,
            cls,
            w!("probe"),
            WS_POPUP,
            MARKER_X,
            MARKER_Y,
            MARKER_W,
            MARKER_H,
            None,
            None,
            None,
            None,
        ) {
            Ok(v) => v,
            Err(e) => {
                let _ = tx.send(Err(format!("建标记窗失败: {e}")));
                return;
            }
        };
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            MARKER_X,
            MARKER_Y,
            MARKER_W,
            MARKER_H,
            SWP_SHOWWINDOW,
        );
        let tid = windows::Win32::System::Threading::GetCurrentThreadId();
        let _ = tx.send(Ok((hwnd.0 as usize, tid)));
        let mut msg = MSG::default();
        while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    });
    rx.recv_timeout(Duration::from_secs(10))
        .map_err(|e| format!("标记窗线程启动超时: {e}"))?
}

struct Marker {
    /// 存`usize` 而非 `HWND`：后者含裸指针，**不是 `Send`**，不能跨线程传。
    hwnd: usize,
    tid: u32,
}

impl Marker {
    fn up() -> Result<Self, String> {
        let (hwnd, tid) = unsafe { spawn_marker()? };
        // 给 DWM 一点时间合成上去
        std::thread::sleep(Duration::from_millis(700));
        Ok(Self { hwnd, tid })
    }
    /// 标记窗中心在屏幕上的坐标。
    fn center() -> (u32, u32) {
        (
            (MARKER_X + MARKER_W / 2) as u32,
            (MARKER_Y + MARKER_H / 2) as u32,
        )
    }
}

impl Drop for Marker {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(HWND(self.hwnd as *mut std::ffi::c_void));
            let _ = PostThreadMessageW(self.tid, WM_QUIT, WPARAM::default(), LPARAM::default());
        }
    }
}

// ══════════════════════ 探针 ══════════════════════

/// ① 一个会话能开起来、能持续出帧，且帧不是纯黑。
#[test]
#[ignore = "要真实显示器与 DWM"]
fn wgc_single_session_yields_real_frames() {
    let mut s = WgcSession::open("single").expect("开 WGC 会话");
    let deadline = Instant::now() + Duration::from_secs(6);
    let (mut got, mut nonblack) = (0, 0);
    let mut sample: Option<Grabbed> = None;
    while Instant::now() < deadline {
        if let Some(g) = s.take() {
            got += 1;
            if g.has_content() {
                nonblack += 1;
                if sample.is_none() {
                    let (w, h) = (g.width, g.height);
                    let c = g.at(w / 2, h / 2);
                    sample = Some(Grabbed {
                        width: w,
                        height: h,
                        pixels: Vec::new(),
                        handoff_us: 0,
                    });
                    println!("样本帧 {w}x{h}，中点像素 {c:?}");
                }
            }
            if got >= 20 {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    println!("WGC 单会话：抓到 {got} 帧（非全黑 {nonblack}）");
    assert!(got > 0, "一帧都没抓到：回调没触发或 TryGetNextFrame 一直空");
    assert!(nonblack > 0, "抓到 {got} 帧但全黑：桌面没被合成进来");
}

/// ★★ 同一显示器**同时**开两个 WGC 会话 —— 决定 GIF+视频能否同开。
#[test]
#[ignore = "要真实显示器与 DWM"]
fn wgc_two_sessions_on_one_monitor_both_work() {
    let a = WgcSession::open("A").expect("A 会话");
    let b = match WgcSession::open("B") {
        Ok(v) => v,
        Err(e) => panic!("★★ 第二个会话开失败：{e}\n（这里失败 = GIF+视频同开不成立）"),
    };
    println!("★ 两个会话同时开成 —— GIF 与视频同开的可行性证据");

    let deadline = Instant::now() + Duration::from_secs(5);
    let (mut ga, mut gb) = (0, 0);
    while Instant::now() < deadline {
        if a.take().is_some() {
            ga += 1;
        }
        if b.take().is_some() {
            gb += 1;
        }
        if ga >= 10 && gb >= 10 {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    println!("并发 5 秒：A 抓到 {ga} 帧，B 抓到 {gb} 帧");
    assert!(ga > 0 && gb > 0, "并发时只有一个在出帧（A={ga} B={gb}）");
}

/// ★★★ **覆盖层可见性** —— 用户最关心的那条：能不能录到自己的特效。
///
/// 拖尾 / 波纹 / 悬浮标都是我们自己画的顶层窗口。抓帧回来在标记窗中心取
/// 像素，命中标记色才算过。**这条红了就绝不能换 WGC。**
#[test]
#[ignore = "要真实显示器与 DWM"]
fn wgc_captures_our_own_overlay_window() {
    let _m = Marker::up().expect("起标记窗");
    let mut s = WgcSession::open("ovl").expect("开 WGC 会话");
    let (cx, cy) = Marker::center();
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut seen = 0u32;
    let mut last: Option<(u8, u8, u8, u32, u32)> = None;
    let mut hit = false;
    while Instant::now() < deadline && !hit {
        if let Some(g) = s.take() {
            seen += 1;
            let (r, gg, b, _) = g.at(cx, cy);
            last = Some((r, gg, b, g.width, g.height));
            if g.matches(cx, cy, MARKER_RGB, 48) {
                hit = true;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    match (&hit, &last) {
        (true, Some((r, g, b, w, h))) => println!(
            "★★ 覆盖层被录到了：({cx},{cy})读到 RGB({r},{g},{b}) 命中标记色，帧 {w}x{h}，共看 {seen} 帧"
        ),
        (false, Some((r, g, b, w, h))) => panic!(
            "★★ 覆盖层没被录到：({cx},{cy})读到 RGB({r},{g},{b})，标记色应为 RGB{:?}，帧 {w}x{h}，看了 {seen} 帧\n\
             ⇒ WGC 抓不到我们自己的顶层窗口 ⇒ 鼠标拖尾录不进去 ⇒ 不能换这条路",
            MARKER_RGB
        ),
        (false, None) => panic!("一帧都没抓到，覆盖层判据无从谈起"),
        _ => unreachable!(),
    }
}

/// ③ 取帧握手耗时 —— 对照 DXGI 的「AcquireNextFrame 卡 441 秒」。
#[test]
#[ignore = "要真实显示器与 DWM"]
fn wgc_handoff_latency_is_bounded() {
    let mut s = WgcSession::open("lat").expect("开 WGC 会话");
    let (mut n, mut worst, mut sum) = (0u32, 0u64, 0u64);
    let deadline = Instant::now() + Duration::from_secs(6);
    while Instant::now() < deadline {
        if let Some(g) = s.take() {
            if g.handoff_us > 0 {
                worst = worst.max(g.handoff_us);
                sum += g.handoff_us;
                n += 1;
            }
            if n >= 30 {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(n > 0, "没抓到可测的帧");
    println!(
        "取帧握手：{n} 帧，平均 {:.0}us，最差 {worst}us（DXGI 实测能到 441_000_000us）",
        sum as f64 / n as f64
    );
    assert!(worst < 2_000_000, "取帧握手 {worst}us 超 2 秒，接近挂死量级");
}
