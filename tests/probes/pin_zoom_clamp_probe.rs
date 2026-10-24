//! probe-group: fast
//! 贴图「放大之后图片没放大、只是边框在放大」的真机探针（`#[ignore]`，要真实桌面会话）。
//!
//! # 它要回答的问题
//!
//! 用户报：贴图长图，放大之后图片并没有真的放大，只是外部边框在放大；复制很多行文字再
//! 贴图（极端宽高比）就能看出来。
//!
//! # 判据
//!
//! `pin.rs::on_wheel` 的缩放就是一次 `SetWindowPos`（窗口尺寸 = 图片 × 百分比），画图时
//! `image_rect` 用 `fit_aspect` 在客户区里按原图比例取最大矩形。所以「图片到底有没有跟着
//! 放大」**只取决于一件事**：`SetWindowPos` 要的尺寸是不是真的给到了窗口。
//!
//! 本探针按贴图窗的真实样式（`WS_POPUP` + `WS_EX_LAYERED` + `WS_EX_TOPMOST`，无
//! `WS_THICKFRAME`）建一扇窗，然后：
//!
//! | 要的尺寸 | 实际尺寸 | 说明 |
//! |---|---|---|
//! | ≤ 屏高 | 一样 | 正常缩放 |
//! | > 屏高 | **被钳** | 这就是"图片不再长大、边框还在长大"的原因 |
//!
//! 决定性的一条：**只有一维被钳**时，另一维还在按百分比长大 → 边框继续变宽/变高，而
//! 宽高比锁死的图片被 `fit_aspect` 卡在被钳的那一维上，尺寸冻结 → 用户看到的正是
//! "边框在放大、图片没放大"。
//!
//! 顺带量三件事，逐层收窄修法：
//!
//! | 变体 | 做法 | 想验证什么 |
//! |---|---|---|
//! | 0 | 默认（`DefWindowProcW`） | 复现钳制本体 |
//! | 1 | 接管 `WM_GETMINMAXINFO`，min=200/max=16384 | "接管它"本身够不够 |
//! | 2 | **只写 min=(1,1)**，不碰 max、不调 `DefWindowProcW` | 复刻 windui `.min_size(1,1)`：应用侧一行能不能同时解掉宽下限与高上限 |
//! | 3 | 建窗后换样式为 `WS_POPUP` | 换样式这条路（无需改第三方库） |
//!
//! 跑法：`python build/probe.py run pin_zoom_clamp_probe`

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetSystemMetrics, GetWindowLongPtrW,
    GetWindowRect, PeekMessageW, RegisterClassExW, SetWindowLongPtrW, SetWindowPos,
    TranslateMessage, DestroyWindow, UnregisterClassW, GWL_EXSTYLE, GWL_STYLE, GWLP_USERDATA,
    HWND_TOPMOST, MINMAXINFO, MSG, PM_REMOVE, SM_CXMINTRACK, SM_CXSCREEN, SM_CYMINTRACK,
    SM_CYSCREEN, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    WM_GETMINMAXINFO, WS_MAXIMIZEBOX, WS_OVERLAPPEDWINDOW, WS_POPUP, WS_THICKFRAME, WS_VISIBLE,
    WNDCLASSEXW, WINDOW_EX_STYLE, WINDOW_STYLE,
};

/// 要的尺寸里"大于屏幕多少"以内算正常（允许 1px 取整噪声）。
const SLACK: i32 = 2;

fn pump(ms: u64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
    loop {
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if std::time::Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// 把 `(w,h)` 设上去，回报"实际拿到的尺寸"（与 `win_util::set_window_rect_hwnd` 同一路径）。
fn try_size(hwnd: HWND, w: i32, h: i32) -> (i32, i32) {
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            40,
            40,
            w,
            h,
            SWP_NOACTIVATE,
        );
    }
    pump(150);
    let mut r = windows::Win32::Foundation::RECT::default();
    unsafe {
        let _ = GetWindowRect(hwnd, &mut r);
    }
    (r.right - r.left, r.bottom - r.top)
}

/// 建一扇"贴图同款"窗：`WS_POPUP` + 分层 + 置顶，无 `WS_THICKFRAME`。
///
/// `name` 用两个不同类名区分"默认处理"与"接管 `WM_GETMINMAXINFO`"两种窗。
unsafe fn make(class: &str, variant: u8) -> HWND {
    unsafe {
        let wname: Vec<u16> = class.encode_utf16().chain(std::iter::once(0)).collect();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wnd_proc),
            hInstance: GetModuleHandleW(None).unwrap_or_default().into(),
            lpszClassName: windows::core::PCWSTR(wname.as_ptr()),
            ..Default::default()
        };
        let _ = RegisterClassExW(&wc);
        // 与贴图窗逐位相同：`WS_OVERLAPPEDWINDOW` 去 `WS_THICKFRAME`/`WS_MAXIMIZEBOX`，
        // **没有** `WS_POPUP`、**没有** `WS_EX_LAYERED`。
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            windows::core::PCWSTR(wname.as_ptr()),
            windows::core::w!("probe"),
            PIN_STYLE | WS_VISIBLE,
            40,
            40,
            60,
            80,
            None,
            None,
            Some(GetModuleHandleW(None).unwrap_or_default().into()),
            None,
        )
        .expect("CreateWindowExW");
        // 用 USERDATA 存"是否解除钳制"，窗过程据此分流。
        let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA, variant as isize);
        // ★ 建窗**当场**量一次（任何 SetWindowPos 之前）：分辨"钳制发生在建窗"还是
        //   "只发生在 SetWindowPos"。这决定修法——前者连 min_size/max_size 都救不了
        //   建窗那一瞬，后者则可以。
        // 变体 3：**建窗之后**把样式换成 WS_POPUP（只保留 WS_VISIBLE），再触发一次边框重算。
        // 这条是候选修法：不需要改第三方 windui，应用侧自己换样式。
        if variant == 3 {
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
            let _ = SetWindowLongPtrW(
                hwnd,
                GWL_STYLE,
                (WS_POPUP.0 | (style & WS_VISIBLE.0)) as isize,
            );
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOZORDER | SWP_NOMOVE | SWP_NOSIZE | SWP_FRAMECHANGED,
            );
            pump(120);
        }
        let mut r0 = windows::Win32::Foundation::RECT::default();
        let _ = GetWindowRect(hwnd, &mut r0);
        println!(
            "  建窗请求 60x80 → 当场 {W}x{H}（若已是 136，说明 CreateWindowExW 自己就收口）",
            W = r0.right - r0.left,
            H = r0.bottom - r0.top
        );
        hwnd
    }
}

/// 贴图窗的真实样式（`Window::new(..).frameless(true).resizable(false)`、**没有** `.layered()`）：
/// `WS_OVERLAPPEDWINDOW` 去掉 `WS_THICKFRAME` 与 `WS_MAXIMIZEBOX`，靠 `WM_NCCALCSIZE` 把非客户区
/// 抹平 —— 所以 `GetWindowRect` == 客户区，但**样式里仍带着 `WS_CAPTION` 一整套**，
/// 系统据此给出一个远大于图片宽度的最小跟踪尺寸。
const PIN_STYLE: WINDOW_STYLE = WINDOW_STYLE(
    WS_OVERLAPPEDWINDOW.0 & !(WS_THICKFRAME.0 | WS_MAXIMIZEBOX.0),
);

/// 第三态实验用的下限值：写成 200 看下限是否随之变成 200（证明 `ptMinTrackSize` 被读）。
///
/// 变体 2 不用它——那里写 1，问的是"只写下限、不调 DefWindowProcW"时系统默认的
/// `ptMaxTrackSize` 够不够大（见 `wnd_proc` 里的注释）。
const MIN_FLOOR: i32 = 200;

/// 贴图窗会遇到的尺寸：极端宽高比图（60 宽）+ 一路超过屏高的长图。
const CASES: [(i32, i32); 8] = [
    (2, 80),      // 极端窄：看下限到底是多少
    (50, 80),
    (64, 724),    // 60×720 图 + 2×FRAME —— 用户报的"多行短文字"那一档
    (70, 796),
    (64, 3004),   // 60×3000 长图
    (60, 400),
    (60, 4000),
    (3000, 80),
];

#[test]
#[ignore]
fn set_window_pos_is_clamped_to_the_screen() {
    unsafe {
        let cx = GetSystemMetrics(SM_CXSCREEN);
        let cy = GetSystemMetrics(SM_CYSCREEN);
        let minw = GetSystemMetrics(SM_CXMINTRACK);
        let minh = GetSystemMetrics(SM_CYMINTRACK);
        println!("屏幕 {cx}x{cy}；SM_CXMINTRACK={minw} SM_CYMINTRACK={minh}");

        for lift in [0u8, 1, 2, 3] {
            let class = match lift {
                0 => "CurSeenProbeZoomClamp",
                1 => "CurSeenProbeZoomClampLift",
                2 => "CurSeenProbeZoomClampMin1",
                _ => "CurSeenProbeZoomClampPopup",
            };
            let hwnd = make(class, lift);
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
            let exstyle = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            println!(
                "\n=== {}（style={style:#x} exstyle={exstyle:#x}，接管 WM_GETMINMAXINFO={}）",
                class, lift
            );
            for (w, h) in CASES {
                let (aw, ah) = try_size(hwnd, w, h);
                let cl_w = aw > w + SLACK;
                let cl_h = ah > h + SLACK;
                println!(
                    "  要 {w:>5}x{h:<5} → 实际 {aw:>5}x{ah:<5}   {}",
                    if cl_w || cl_h {
                        format!(
                            "★被钳（宽{} 高{}）",
                            if cl_w { "是" } else { "否" },
                            if cl_h { "是" } else { "否" }
                        )
                    } else {
                        "原样".to_string()
                    }
                );
            }
            let _ = DestroyWindow(hwnd);
            let _ = UnregisterClassW(
                windows::core::PCWSTR(
                    class.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>().as_ptr(),
                ),
                Some(GetModuleHandleW(None).unwrap_or_default().into()),
            );
        }
    }
}

/// 默认窗过程；`USERDATA` 为 1 时在 `WM_GETMINMAXINFO` 里把最大跟踪尺寸放到很大，
/// 用来验证"接管它"能不能真的解除钳制（决定修法的唯一依据）。
unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_GETMINMAXINFO {
            let v = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
            if v == 1 || v == 2 {
                let mmi = lparam.0 as *mut MINMAXINFO;
                if !mmi.is_null() {
                    // 变体 1：上下限都写（200 / 16384）——验证"接管它"本身能解除钳制。
                    if v == 1 {
                        (*mmi).ptMinTrackSize.x = MIN_FLOOR;
                        (*mmi).ptMinTrackSize.y = MIN_FLOOR;
                        (*mmi).ptMaxTrackSize.x = 16384;
                        (*mmi).ptMaxTrackSize.y = 16384;
                    } else {
                        // 变体 2：**只写下限 (1,1)**，`ptMaxTrackSize` 一个字节都不碰。
                        // 这正是 windui `Window::min_size(1,1)` 的行为：`min_w>0` 时它在
                        // `WM_GETMINMAXINFO` 里写 `ptMinTrackSize` 后**提前 return**，
                        // 不调 `DefWindowProcW`。于是"系统默认的 ptMaxTrackSize 是多少"
                        // 成了唯一未知量——它若足够大，应用侧加一行 `.min_size(1,1)`
                        // 就能同时解掉 136 宽下限与 ~1100 高上限，不必动第三方库。
                        (*mmi).ptMinTrackSize.x = 1;
                        (*mmi).ptMinTrackSize.y = 1;
                    }
                }
                return LRESULT(0);
            }
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}
