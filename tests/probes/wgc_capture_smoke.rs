//! 真机探针：验`platform::wgc_capture` 这个**生产模块**（不是原型）。
//!
//! ★ 为什么必须单独探：原型（`wgc_proto` 的初版）与生产模块**不是同一份代码** ——
//!   生产模块改成"设备由调用方传入"（`CopyResource` 不能跨设备，见模块头）、
//!   去掉了 staging 中转。**编译通过什么都不能证明**（本项目已连踩多次），
//!   所以"原型绿了"不能替代"生产模块绿了"。
//!
//! 三条判据，全部要真实显示器 + 真实 DWM：
//!
//! ① 会话能开、能持续出帧，且帧不是纯黑。
//! ② **同显示器双会话并发** —— 决定 GIF 与视频能否同时录。
//! ③ **覆盖层可见性** —— 我们自绘的鼠标拖尾/ 波纹 / 悬浮标是顶层窗口，
//!    抓帧回来必须能在标记窗位置读到标记色。**这条红了绝不能换 WGC。**
//!
//! 用法：`python build/probe.py run wgc_capture_smoke`

#![allow(dead_code)]

use std::time::{Duration, Instant};

use curseen_ui::platform::wgc_capture::WgcCapture;
use windows::core::Interface;
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_1};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, PostThreadMessageW,
    RegisterClassW, SetWindowPos, ShowWindow, TranslateMessage, HWND_TOPMOST, MSG, SWP_SHOWWINDOW,
    SW_SHOWNOACTIVATE, WM_PAINT, WM_QUIT, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::Win32::Foundation::COLORREF;

/// 建一台带 `BGRA_SUPPORT` 的硬件设备（生产 `Session::open` 用同一组条件）。
unsafe fn new_device() -> (ID3D11Device, ID3D11DeviceContext) {
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
    .expect("D3D11CreateDevice失败（本机无硬件 D3D11，本探针数据无效）");
    (dev.unwrap(), ctx.unwrap())
}

// ══════════════════════ 覆盖层标记窗 ══════════════════════
//
// 模拟拖尾那种"自己画的顶层窗口"。判据色取桌面上极不可能自然出现的色相。

/// 判据色（RGB）。抓帧回来若红蓝互换，判据会自动归一（见 `pixel_matches`）。
const MARKER_RGB: (u8, u8, u8) = (224, 160, 16);
const MARKER_X: i32 = 60;
const MARKER_Y: i32 = 60;
const MARKER_W: i32 = 240;
const MARKER_H: i32 = 160;

unsafe extern "system" fn marker_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let color = COLORREF(
                (MARKER_RGB.2 as u32) | ((MARKER_RGB.1 as u32) << 8) | ((MARKER_RGB.0 as u32) << 16),
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
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

struct Marker {
    hwnd: usize,
    tid: u32,
}

impl Marker {
    fn up() -> Self {
        use windows::core::w;
        let (tx, rx) = std::sync::mpsc::channel::<std::result::Result<(usize, u32), String>>();
        std::thread::spawn(move || unsafe {
            let inst = GetModuleHandleW(None).expect("GetModuleHandleW");
            let cls = w!("CurSeenWgcCaptureProbeMarker");
            let _ = RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(marker_proc),
                hInstance: inst.into(),
                lpszClassName: cls,
                ..Default::default()
            });
            let hwnd = CreateWindowExW(
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
            )
            .expect("建标记窗失败");
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
            // 这三个在 windows 0.62 里都已是 safe 函数（只读/写消息结构体，
            // 不碰裸指针）—— 写上 `unsafe` 会被 clippy 判为 unnecessary。
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        });
        let (hwnd, tid) = rx.recv_timeout(Duration::from_secs(10)).expect("标记窗线程启动超时").unwrap();
        std::thread::sleep(Duration::from_millis(800));
        Self { hwnd, tid }
    }
    /// 标记窗中心的屏幕坐标。
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

/// 把一张帧读回 CPU 判色。
///
/// ★ 必须**当场拷出来**：帧纹理由 WGC 的帧池托管，只有 2 个缓冲，
///   DWM 随时可能回收它 —— 留着稍后再读会拿到别人的内容（或者已失效的纹理）。
unsafe fn read_pixel(
    f: &curseen_ui::platform::wgc_capture::WgcFrame,
    ctx: &ID3D11DeviceContext,
    x: u32,
    y: u32,
) -> (u8, u8, u8) {
    use windows::Win32::Graphics::Direct3D11::{
        D3D11_CPU_ACCESS_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_TEXTURE2D_DESC,
        D3D11_USAGE_STAGING,
    };
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};

    // 临时 staging：判据要读 CPU，帧纹理本身是 GPU-only 的。
    // 尺寸对齐到 1 字节并非必需，但 staging 的 BindFlags 必须 0（D3D11 规定）。
    let desc = D3D11_TEXTURE2D_DESC {
        Width: f.width,
        Height: f.height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut stg = None;
    // ★ `CreateTexture2D` 在 **ID3D11Device** 上，`CopyResource`/`Map` 才在 Context 上。
    //   写错对象的话编译就报"no method found"（不会静默通过）。
    // ★ 帧的像素要用**自己的**上下文读（不能再借 WGC 会话那份——
    //   `ID3D11DeviceContext` 是单线程语义对象，两处共用会交错）。
    let dev_for_staging: ID3D11Device = ctx.GetDevice().expect("取设备失败（设备已被移除？）");
    dev_for_staging
        .CreateTexture2D(&desc, None, Some(&mut stg))
        .expect("建 staging 失败");
    let stg: windows::Win32::Graphics::Direct3D11::ID3D11Texture2D = stg.unwrap();
    let src: windows::Win32::Graphics::Direct3D11::ID3D11Resource =
        f.texture.cast().expect("纹理转型失败");
    let dst: windows::Win32::Graphics::Direct3D11::ID3D11Resource =
        stg.cast().expect("staging 转型失败");
    // `CopyResource` 返回 `()`，没有错误码可查——它失败时**静默不拷**。
    ctx.CopyResource(Some(&dst), Some(&src));
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    // 只有 `Map` 会失败（设备被移除时）；`CopyResource` 失败是静默的，
    // 所以下面还要**真的读出像素并判非黑**（第④条判据）才算通过。
    ctx.Map(&stg, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        .expect("Map 失败");
    // ★ 行跨度用 `RowPitch`（GPU 对齐后通常 > w*4），**不能**用 w*4 ——
    //   那样会逐行错位，取出来的颜色是相邻像素的。
    let o = y as usize * mapped.RowPitch as usize + x as usize * 4;
    let p = unsafe { mapped.pData.cast::<u8>().add(o) };
    // 裸指针只能 `read`，不能索引。
    let out = unsafe { (p.read(), p.add(1).read(), p.add(2).read()) };
    let out = (out.2, out.1, out.0);
    ctx.Unmap(&stg, 0);
    out
}

/// 像素是否命中判据色（**红蓝可互换**，绿通道精确）。
///
/// ★ 红蓝归一是必需的：实测向帧池**请求** `B8G8R8A8UIntNormalized`，
///   回来的字节序**是 RGBA**（标记 RGB(224,160,16) 读成 (16,160,224)）。
///   照 BGRA 解释会让所有颜色红蓝反，而且**不报错**。
fn pixel_matches(got: (u8, u8, u8), want: (u8, u8, u8), tol: u8) -> bool {
    let (r, g, b) = got;
    let (wr, wg, wb) = want;
    g.abs_diff(wg) < tol
        && r.min(b).abs_diff(wr.min(wb)) < tol
        && r.max(b).abs_diff(wr.max(wb)) < tol
}

/// 开一个会话（自建设备，按生产模块要求传进去）。
fn open_session() -> (WgcCapture, ID3D11Device, ID3D11DeviceContext) {
    let (dev, ctx) = unsafe { new_device() };
    // ★ 只传设备、**不传 context**（单线程语义对象，跨线程共用会让命令流交错）。
    let cap = WgcCapture::open(dev.clone(), None, 30).expect("开 WGC 会话失败");
    (cap, dev, ctx)
}

// ══════════════════════ 探针 ══════════════════════

/// ① 会话能开、能持续出帧，且帧不是纯黑。
#[test]
#[ignore = "要真实显示器与 DWM"]
fn wgc_capture_yields_real_frames() {
    let (cap, _dev, _ctx) = open_session();
    let (w, h) = cap.size();
    println!("会话已开：{w}x{h}，origin={:?}", WgcCapture::origin());
    assert!(w > 0 && h > 0, "尺寸为 0：会话没真正起来");

    let deadline = Instant::now() + Duration::from_secs(6);
    let mut got = 0u32;
    let mut worst_handoff = 0u64;
    while Instant::now() < deadline {
        if let Some(f) = cap.poll() {
            got += 1;
            worst_handoff = worst_handoff.max(f.handoff_us);
            if got >= 20 {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    let cost = cap.cost();
    println!(
        "抓到 {got} 帧（空轮询 {} 次），最差握手 {worst_handoff}us，重建 {} 次",
        cost.empty_polls, cost.recreates
    );
    assert!(got > 0, "一帧都没抓到：消息循环没跑或 FrameArrived 没触发");
    assert!(
        worst_handoff < 2_000_000,
        "取帧握手 {worst_handoff}us 超 2 秒，接近挂死量级（DXGI 那边能到 4.41e8 us）"
    );
}

/// ② ★★ 同显示器**同时**开两个会话 —— 决定 GIF + 视频能否同时录。
#[test]
#[ignore = "要真实显示器与 DWM"]
fn wgc_capture_two_sessions_both_work() {
    let (a, _dev_a, _ctx_a) = open_session();
    // ★ 第二个会话用**另一台设备**：生产里采集与 `acq` 共用一台（`CopyResource`
    //   不能跨设备），而两个会话各开一台是更严格的考验 —— 若 DWM 侧真的限制
    //   "同屏只能一个 WGC 会话"，第二个的 `open` 就会失败，那才是判红。
    let (_dev_b, _ctx_b) = unsafe { new_device() };
    let b = match WgcCapture::open(_dev_b, None, 30) {
        Ok(v) => v,
        Err(e) => panic!("★★ 第二个会话开失败：{e}\n（这里失败 = GIF+视频同开不成立）"),
    };
    println!("★ 两个会话同时开成 —— GIF 与视频同开的可行性证据");

    let deadline = Instant::now() + Duration::from_secs(5);
    let (mut ga, mut gb) = (0, 0);
    while Instant::now() < deadline {
        if a.poll().is_some() {
            ga += 1;
        }
        if b.poll().is_some() {
            gb += 1;
        }
        if ga >= 10 && gb >= 10 {
            break;
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    println!("并发 5 秒：A 拿到 {ga} 帧，B 拿到 {gb} 帧");
    assert!(ga > 0 && gb > 0, "并发时只有一个在出帧（A={ga} B={gb}）");
}

/// ③ ★★★ **覆盖层可见性** —— 鼠标拖尾等自绘顶层窗能不能录进去。
///
/// 这条是用户明确的要求（"我见过很多录屏方案无法录制鼠标拖尾特效"）。
/// **红了就绝不能换 WGC。**
#[test]
#[ignore = "要真实显示器与 DWM"]
fn wgc_capture_sees_our_own_overlay() {
    let _m = Marker::up();
    let (cap, _dev, ctx) = open_session();
    let (cx, cy) = Marker::center();
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut seen = 0u32;
    let mut last = None;
    let mut hit = false;
    while Instant::now() < deadline && !hit {
        if let Some(f) = cap.poll() {
            seen += 1;
            let px = unsafe { read_pixel(&f, &ctx, cx, cy) };
            last = Some(px);
            if pixel_matches(px, MARKER_RGB, 48) {
                hit = true;
            }
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    match (hit, last) {
        (true, _) => println!("★★ 覆盖层被录到了：({cx},{cy})命中标记色 {MARKER_RGB:?}，共看 {seen} 帧"),
        (false, Some(px)) => panic!(
            "★★ 覆盖层没被录到：({cx},{cy})读到 RGB{px:?}，标记色应为 RGB{MARKER_RGB:?}，看了 {seen} 帧\n\
             ⇒ WGC 抓不到我们自己的顶层窗口 ⇒ 鼠标拖尾录不进去 ⇒ 不能换这条路"
        ),
        (false, None) => panic!("一帧都没抓到，覆盖层判据无从谈起"),
    }
}

/// ④ 帧纹理**能被拷走**（下游要拷进带 `GDI_COMPATIBLE` 的采集纹理）。
///
/// ★ 这条钉住"共用设备"这个设计前提：`CopyResource` 不能跨设备。
///   若哪天改成"模块自建设备"，本判据会在真机上变成静默失败或 `0x80070057`。
#[test]
#[ignore = "要真实显示器与 DWM"]
fn wgc_frame_can_be_copied_with_the_shared_device() {
    let (cap, _dev, ctx) = open_session();
    let deadline = Instant::now() + Duration::from_secs(6);
    let mut f = None;
    while Instant::now() < deadline && f.is_none() {
        f = cap.poll();
        if f.is_none() {
            std::thread::sleep(Duration::from_millis(16));
        }
    }
    let f = f.expect("没抓到帧");
    // 上面`read_pixel` 内部就做了一次 `CopyResource` + `Map`，能读出像素即证明通。
    let px = unsafe { read_pixel(&f, &ctx, f.width / 2, f.height / 2) };
    println!("帧中心像素 RGB{px:?}（能读出即证明 CopyResource 通）");
    assert!(
        px.0 | px.1 | px.2 > 12,
        "读出来是纯黑：CopyResource 通了但内容不对"
    );
}

/// ⑤ 双会话的帧**内容一致**（证明两个会话看到的是同一块桌面，不是各抓各的）。
#[test]
#[ignore = "要真实显示器与 DWM"]
fn wgc_two_sessions_see_the_same_desktop() {
    let (a, _dev_a, ctx_a) = open_session();
    let (_dev_b, ctx_b) = unsafe { new_device() };
    let b = WgcCapture::open(_dev_b, None, 30).expect("第二个会话");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let (Some(fa), Some(fb)) = (a.poll(), b.poll()) {
            let pa = unsafe { read_pixel(&fa, &ctx_a, 40, 40) };
            let pb = unsafe { read_pixel(&fb, &ctx_b, 40, 40) };
            println!("同坐标(40,40)：A=RGB{pa:?} B=RGB{pb:?}");
            // ★ 判"两个会话看的是同一块桌面"：同一坐标的像素应**相近**。
            //   原来判"非纯黑"是错的——深色桌面/深色主题下那个值本来就很小
            //   （实测读到 RGB(1,3,4) 就假红了），判据比被测物更严。
            let diff = (pa.0 as i32 - pb.0 as i32).abs()
                + (pa.1 as i32 - pb.1 as i32).abs()
                + (pa.2 as i32 - pb.2 as i32).abs();
            assert!(
                diff < 90,
                "两个会话在同一坐标读到的画面差 {diff}（{pa:?} vs {pb:?}）：不是同一块桌面"
            );
            return;
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    panic!("5 秒内没能从两个会话各拿到一帧");
}
