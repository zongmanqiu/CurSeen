//! probe-group: fast
//! 画中画放大 · 抓屏可见性探针（诊断用，`#[ignore]`）。
//!
//! # 它要回答的两个问题
//!
//! CurSeen 的画中画放大当前整套设计建立在一个假设上（见 `platform/screenshot.rs`
//! 的 `grab_clean` 与 `modules/magnifier/pip.rs` 模块头）：
//!
//! > **不带 `CAPTUREBLT` 的 `BitBlt` 抓不到分层窗口（`WS_EX_LAYERED` +
//! >   `UpdateLayeredWindow`）**，于是自身的全屏覆盖层天然被排除，源图干净、不会套娃。
//!
//! 用户实测报「画中画无限套娃」，说明这个假设在本机不成立（或只在部分条件下成立）。
//! 本探针用**真的分层窗**实测这条规则，不再靠猜：
//!
//! 1. 先抓一块基准底图（此时还没有分层窗）；
//! 2. 造一个纯红分层窗（`WS_EX_LAYERED` + `UpdateLayeredWindow`）压在同一块矩形上；
//! 3. 分别用 `SRCCOPY`（= `grab_clean`）与 `SRCCOPY|CAPTUREBLT`（= `grab`）抓同一块；
//! 4. 数**纯红像素**：分层窗被抓进来了占比就高，没抓进来就是 0。
//!
//! 第 5 步顺带测真正**能**排除自己的是什么：`SetWindowDisplayAffinity(
//! WDA_EXCLUDEFROMCAPTURE)` —— 它是修复候选方案的地基，同样要实测。
//!
//! 跑法（要真会话，别进默认集合）：
//!
//! ```text
//! cargo test --release --test pip_probe -- --ignored --nocapture --test-threads=1
//! ```

#![cfg(windows)]

use std::mem::size_of;
use std::ptr;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateDIBSection, CreateSolidBrush,
    DeleteDC, DeleteObject, FillRect, GetDC, GetDIBits, ReleaseDC as ReleaseDc, SelectObject,
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, CAPTUREBLT,
    DIB_RGB_COLORS, HBITMAP, HDC, ROP_CODE, SRCCOPY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, RegisterClassExW, SetWindowDisplayAffinity, ShowWindow,
    UpdateLayeredWindow, ULW_ALPHA, WDA_EXCLUDEFROMCAPTURE, WINDOW_EX_STYLE, WNDCLASSEXW,
    WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use curseen_ui::core::image::Shot;
use curseen_ui::platform::display::{virtual_screen, ScreenRect};
use curseen_ui::platform::pip_window::{PipLook, PipWindow};
use curseen_ui::platform::screenshot;

/// 探针窗尺寸。
const SIDE: i32 = 240;

/// 造一个 `side×side` 的 32bpp BGRA DIB，全部填充为**纯红**（判据色）。
fn make_solid_bitmap(dc: HDC, side: i32) -> HBITMAP {
    unsafe {
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: side,
                biHeight: -side, // 负值 = 自上而下
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = ptr::null_mut();
        let bmp = CreateDIBSection(Some(dc), &bi, DIB_RGB_COLORS, &mut bits, None, 0)
            .expect("CreateDIBSection 应成功");
        if !bits.is_null() {
            let px = std::slice::from_raw_parts_mut(bits as *mut u8, (side * side * 4) as usize);
            for q in px.chunks_exact_mut(4) {
                q[0] = 0x00; // B
                q[1] = 0x00; // G
                q[2] = 0xFF; // R
                q[3] = 0xFF; // A
            }
        }
        bmp
    }
}

/// 极简窗口过程：一切交给 `DefWindowProcW`（这窗不收消息、不交互）。
extern "system" fn probe_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    unsafe { windows::Win32::UI::WindowsAndMessaging::DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// 一个极简分层窗口：只为了"屏幕上真的有一块 `UpdateLayeredWindow` 画出来的像素"。
struct LayeredProbe {
    hwnd: HWND,
}

impl LayeredProbe {
    fn spawn(rect: ScreenRect) -> Self {
        unsafe {
            let class: Vec<u16> = "CurSeenPipProbeCls\0".encode_utf16().collect();
            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(probe_wndproc),
                lpszClassName: PCWSTR(class.as_ptr()),
                ..Default::default()
            };
            let _atom = RegisterClassExW(&wc);

            let name: Vec<u16> = "curseen pip probe\0".encode_utf16().collect();
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(
                    (WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT).0,
                ),
                PCWSTR(class.as_ptr()),
                PCWSTR(name.as_ptr()),
                WS_POPUP,
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                None,
                None,
                None,
                Some(ptr::null()),
            )
            .expect("CreateWindowExW 应成功");
            let _ = ShowWindow(hwnd, windows::Win32::UI::WindowsAndMessaging::SW_SHOWNA);

            // UpdateLayeredWindow 把 32bpp 位图一次性提交到分层窗（带 alpha）。
            let dc = GetDC(None);
            let bmp = make_solid_bitmap(dc, rect.w);
            let mem = CreateCompatibleDC(Some(dc));
            let old = SelectObject(mem, bmp.into());
            let bf = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 0xFF,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let pt = POINT {
                x: rect.x,
                y: rect.y,
            };
            let sz = SIZE {
                cx: rect.w,
                cy: rect.h,
            };
            let ok = UpdateLayeredWindow(
                hwnd,
                Some(dc),
                Some(&pt),
                Some(&sz),
                Some(mem),
                Some(&POINT { x: 0, y: 0 }),
                COLORREF(0),
                Some(&bf),
                ULW_ALPHA,
            );
            println!("  UpdateLayeredWindow = {ok:?}");
            SelectObject(mem, old);
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(mem);
            ReleaseDc(None, dc);

            // 给 DWM 一点时间把这一帧合成出来。
            std::thread::sleep(std::time::Duration::from_millis(350));

            Self { hwnd }
        }
    }
}

impl Drop for LayeredProbe {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// 用指定 rop 抓 `rect`，回报 `(w, h, BGRA 缓冲)`。
fn grab_rop(rect: ScreenRect, rop: ROP_CODE) -> Option<(i32, i32, Vec<u8>)> {
    let w = rect.w.max(1);
    let h = rect.h.max(1);
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, bmp.into());
        // 先刷一层白底，免得"抓失败"被误读成"抓到了某种颜色"。
        let brush = CreateSolidBrush(COLORREF(0x00FFFFFF));
        let r = RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        };
        FillRect(mem, &r, brush);
        let _ = DeleteObject(brush.into());

        let ok = BitBlt(mem, 0, 0, w, h, Some(screen), rect.x, rect.y, rop);
        let out = if ok.is_ok() {
            let mut bi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut buf = vec![0u8; (w * h * 4) as usize];
            let got = GetDIBits(
                mem,
                bmp,
                0,
                h as u32,
                Some(buf.as_mut_ptr() as *mut _),
                &mut bi,
                DIB_RGB_COLORS,
            );
            if got == h {
                Some((w, h, buf))
            } else {
                None
            }
        } else {
            None
        };
        SelectObject(mem, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
        ReleaseDc(None, screen);
        out
    }
}

/// 数 BGRA 缓冲里"接近纯红"的像素占比（%）。
fn red_ratio(px: &[u8]) -> f64 {
    if px.is_empty() {
        return 0.0;
    }
    let n = px.len() / 4;
    let mut hit = 0usize;
    for q in px.chunks_exact(4) {
        let (b, g, r) = (q[0], q[1], q[2]);
        if r > 180 && g < 60 && b < 60 {
            hit += 1;
        }
    }
    hit as f64 * 100.0 / n as f64
}

/// ★★ 修复验收：**真的**画中画窗不能出现在它自己的采样源里。
///
/// 与上面那条用例的区别：那条测的是"打算借用的机制"，这条测的是**上线代码本身**——
/// 用 `platform::pip_window` 那个窗，按 `pip.rs` 里一模一样的调用序列
/// （`screenshot::grab` + `PipWindow::present`）跑一轮，再抓它自己覆盖的那块区域，
/// 数一数有没有把面板像素抓进来。
///
/// 判据不比颜色而比"能不能抓住这个窗"：任何一种"自己看到自己"，第一步都是先把自己的
/// 像素摸回来；颜色反而会因为边框 / 缩放 / alpha 而漂移。所以这里直接用
/// `present` 喂一块**纯品红**的源图，抓回来的图里要是出现大量品红，就是又套娃了。
#[test]
#[ignore]
fn the_real_pip_window_never_appears_in_its_own_source() {
    let vs = virtual_screen();
    let panel = ScreenRect::new(vs.x + 300, vs.y + 200, 320, 180);

    let mut win = PipWindow::new();
    let look = PipLook::default();
    // 源图故意用**纯品红**（与桌面常见色、也与边框的绿完全不同）。
    let src = {
        let n = (panel.w * panel.h * 4) as usize;
        let mut bgra = vec![0u8; n];
        for q in bgra.chunks_exact_mut(4) {
            q[0] = 0xFF; // B
            q[1] = 0x00; // G
            q[2] = 0xFF; // R
            q[3] = 0xFF;
        }
        Shot {
            w: panel.w / 2, // 取样区只有面板的一半 → 实际放大 2×
            h: panel.h / 2,
            bgra,
        }
    };
    win.present(&src, panel, &look)
        .expect("画中画 present 应成功");
    assert!(
        win.is_excluded(),
        "本机的 SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE) 必须成功 —— \
         否则整套防套娃方案不成立"
    );
    // 等 DWM 把这一帧合成出来。
    std::thread::sleep(std::time::Duration::from_millis(350));

    // ★ 按生产代码的同一条路去抓（含 CAPTUREBLT）。
    let debug_or = screenshot::grab(panel).expect("抓屏应成功");
    let hit = magenta_ratio(&debug_or.bgra);
    println!(
        "画中画窗已在 {panel:?} 显示，生产链路抓回来：品红占比 = {hit:6.2}%  \
         （接近 0 = 没有把自己抓进来，接近 100 = 仍然套娃）"
    );
    assert!(
        hit < 1.0,
        "画中画仍然在源图里看到自己的面板（{hit:.2}%）—— 又套娃了"
    );
}

/// 数 BGRA 缓冲里"接近品红"的像素占比（%）—— present 进去的那个纯色。
fn magenta_ratio(px: &[u8]) -> f64 {
    if px.is_empty() {
        return 0.0;
    }
    let n = px.len() / 4;
    let mut hit = 0usize;
    for q in px.chunks_exact(4) {
        let (b, g, r) = (q[0], q[1], q[2]);
        if r > 180 && b > 180 && g < 60 {
            hit += 1;
        }
    }
    hit as f64 * 100.0 / n as f64
}

/// ★★ 核心诊断：不带 `CAPTUREBLT` 的 `BitBlt` 到底抓不抓得到分层窗？
#[test]
#[ignore]
fn plain_srccopy_excludes_layered_windows_or_not() {
    let vs = virtual_screen();
    let rect = ScreenRect::new(vs.x + 200, vs.y + 160, SIDE, SIDE);
    println!("虚拟屏 {vs:?}");
    println!("探针窗 {rect:?}");

    // ① 基准：还没有分层窗。
    let base = grab_rop(rect, SRCCOPY).expect("基准抓屏应成功");
    println!(
        "① 基准（无分层窗）              SRCCOPY             红占比 = {:6.2}%",
        red_ratio(&base.2)
    );

    // ② 造分层窗后再抓两次。
    let probe = LayeredProbe::spawn(rect);
    let clean = grab_rop(rect, SRCCOPY).expect("SRCCOPY 抓屏应成功"); // == grab_clean
    let with = grab_rop(rect, ROP_CODE(SRCCOPY.0 | CAPTUREBLT.0)).expect("CAPTUREBLT 抓屏应成功"); // == grab
    let clean_pct = red_ratio(&clean.2);
    let with_pct = red_ratio(&with.2);
    println!("② 分层窗已显示                  SRCCOPY (grab_clean) 红占比 = {clean_pct:6.2}%   ← pip 用的就是它");
    println!("                                SRCCOPY|CAPTUREBLT  红占比 = {with_pct:6.2}%");

    // ③ 加自排除再看：这是修复候选。
    unsafe {
        let _ = SetWindowDisplayAffinity(probe.hwnd, WDA_EXCLUDEFROMCAPTURE);
    }
    std::thread::sleep(std::time::Duration::from_millis(350));
    let ex_clean = grab_rop(rect, SRCCOPY).expect("SRCCOPY 抓屏应成功");
    let ex_with = grab_rop(rect, ROP_CODE(SRCCOPY.0 | CAPTUREBLT.0)).expect("抓屏应成功");
    let ex_clean_pct = red_ratio(&ex_clean.2);
    let ex_with_pct = red_ratio(&ex_with.2);
    println!("③ 加 WDA_EXCLUDEFROMCAPTURE 后  SRCCOPY             红占比 = {ex_clean_pct:6.2}%");
    println!("                                SRCCOPY|CAPTUREBLT  红占比 = {ex_with_pct:6.2}%");

    println!(
        "\n结论：SRCCOPY(grab_clean) {} 自身分层窗；CAPTUREBLT {} 自身分层窗；WDA_EXCLUDEFROMCAPTURE {} 排除。",
        if clean_pct > 50.0 { "抓得到" } else { "抓不到" },
        if with_pct > 50.0 { "抓得到" } else { "抓不到" },
        if ex_with_pct < 5.0 { "能" } else { "不能" },
    );

    // 只做**观测**更多、断言最少：这条用例的价值是打印事实，判据由人读。
    assert!(
        with_pct > 50.0,
        "带 CAPTUREBLT 起码该看得到自己的分层窗（否则探针本身没生效）"
    );
}
