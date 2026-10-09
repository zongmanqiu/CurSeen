//! probe-group: fast
//! 便签「按住标题栏拖不动」+「改背景色透明度却动了整个窗」的真机诊断探针（`#[ignore]`）。
//!
//! # 它要回答的两个问题
//!
//! 1. **拖动**：`WM_NCHITTEST` 在标题栏空白处到底返回什么？返回 `HTCAPTION`(2) 却仍拖不动，
//!    就说明是系统那条 `SC_MOVE` 模态循环被别人截胡了（而不是命中测算出问题）。
//!    判定用**真实鼠标输入**（`SendInput` + `SetCursorPos`）：假消息绕过
//!    "哪张窗真正收到这次按下"这个关键事实 —— 拆成两张窗之后，这正是最可能出事的地方。
//! 2. **串层**：只改背景色的 alpha（其它一律不动），看**正文区**那个像素变不变。
//!    变 = 标题栏窗的层 alpha 漏到了正文区（两张窗叠色），即用户看到的"整个窗口都变了"。
//!
//! ★★★ 找窗口**必须过滤 `IsWindowVisible`**：`EnumWindows` 会把隐藏窗也列出来，
//!   于是"找到的窗口"很可能是上一个用例留下的那张（已 `SW_HIDE`）—— 对着一个隐藏窗
//!   点鼠标 = 点了个空，会把"能拖"误判成"拖不动"（本探针第一版就栽在这里）。
//!
//! 跑法：`cargo test --release --test note_drag_probe -- --ignored --nocapture`

use std::time::{Duration, Instant};

use curseen_ui::platform::sticky::{NoteCfg, StickyManager, BODY_CLASS, CLASS_NAME};
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{GetDC, GetPixel, ReleaseDC};
// ★ `GetCapture` 在 `KeyboardAndMouse`（不在 `WindowsAndMessaging`）—— 与 `sticky.rs` 同。
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEINPUT, MOUSE_EVENT_FLAGS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetCursorPos, GetSystemMetrics, GetWindowRect,
    GetWindowThreadProcessId, IsWindowVisible, SendMessageW, WindowFromPoint, SM_CXSCREEN,
    SM_CYSCREEN, WM_NCHITTEST,
};

/// 跨线程读 GUI 线程状态：`GUI_INMOVESIZE` = 该线程正在移动模态循环里，
/// `hwndCapture` = 当前捕获鼠标的窗。这是唯一能**从别的线程**判断
/// "模态循环起没起"的办法（`GetCapture` 只对本线程有效，恒 0，上一版探针被它骗了）。
fn gui_state(h: HWND) -> (bool, usize) {
    let tid = unsafe { GetWindowThreadProcessId(h, None) };
    let mut info = windows::Win32::UI::WindowsAndMessaging::GUITHREADINFO {
        cbSize: std::mem::size_of::<windows::Win32::UI::WindowsAndMessaging::GUITHREADINFO>()
            as u32,
        ..Default::default()
    };
    let ok = unsafe { windows::Win32::UI::WindowsAndMessaging::GetGUIThreadInfo(tid, &mut info) };
    (
        ok.is_ok()
            && (info.flags
                & windows::Win32::UI::WindowsAndMessaging::GUITHREADINFO_FLAGS(
                    windows::Win32::UI::WindowsAndMessaging::GUI_INMOVESIZE.0,
                ))
                != windows::Win32::UI::WindowsAndMessaging::GUITHREADINFO_FLAGS(0),
        info.hwndCapture.0 as usize,
    )
}

/// 句柄 → 类名（诊断输出用）。
fn class_of(h: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n as usize])
}

/// HT* 命中码（故意写死：探针要的是"Windows 的定义"，不能被实现里的拼写错误带着一起错）。
const HT_CAPTION: isize = 2;

/// 按类名找一张**可见**的窗（`IsWindowVisible` 过滤，见模块头注释）。
fn find_by_class(want: &str) -> Option<HWND> {
    unsafe extern "system" fn cb(h: HWND, l: LPARAM) -> BOOL {
        let out = &mut *(l.0 as *mut Vec<(HWND, String)>);
        let mut buf = [0u16; 64];
        let n = GetClassNameW(h, &mut buf);
        out.push((h, String::from_utf16_lossy(&buf[..n as usize])));
        BOOL(1)
    }
    let mut all: Vec<(HWND, String)> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut all as *mut _ as isize));
    }
    all.into_iter()
        .find(|(h, c)| c == want && unsafe { IsWindowVisible(*h) }.as_bool())
        .map(|(h, _)| h)
}

fn window_rect(h: HWND) -> RECT {
    let mut r = RECT::default();
    unsafe {
        let _ = GetWindowRect(h, &mut r);
    }
    r
}

fn hit_test(h: HWND, x: i32, y: i32) -> isize {
    let p = ((y as u16 as usize) << 16) | (x as u16 as usize);
    unsafe { SendMessageW(h, WM_NCHITTEST, None, Some(LPARAM(p as isize))).0 }
}

/// 送一条鼠标输入；返回系统接受了几条（1 = 成功，0 = 被拒：UIPI / 参数不对）。
fn mouse(flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32) -> u32 {
    let i = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe { SendInput(&[i], std::mem::size_of::<INPUT>() as i32) }
}

/// 注入式移动光标到屏幕坐标（`MOUSEEVENTF_ABSOLUTE`：**必须**归一化到 0..65535，
/// 直接填像素会飞到左上角）。
///
/// ★ 用注入而不用 `SetCursorPos`：`SetCursorPos` 只改光标位置、不进输入队列，
///   窗口收不到 `WM_MOUSEMOVE`，模态循环（拖动）起来时的初始位置可能对不上。
fn move_to(x: i32, y: i32) -> u32 {
    let (vw, vh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    let ax = x * 65535 / vw.max(1);
    let ay = y * 65535 / vh.max(1);
    mouse(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE, ax, ay)
}

/// 采样屏幕某点的颜色（GDI 读屏；能读到 DWM 合成结果就有信号）。
fn screen_pixel(x: i32, y: i32) -> u32 {
    unsafe {
        let dc = GetDC(None);
        let c = GetPixel(dc, x, y);
        ReleaseDC(None, dc);
        c.0
    }
}

/// 找一个像素的 RGB（把 COLORREF 的 0x00BBGGRR 摆回 R,G,B，好看一点）。
fn rgb(c: u32) -> (u8, u8, u8) {
    (
        (c & 0xFF) as u8,
        ((c >> 8) & 0xFF) as u8,
        ((c >> 16) & 0xFF) as u8,
    )
}

/// 开一张便签并等它出现；返回 manager（**必须**由调用方持有到底，否则窗口随即销毁）。
fn spawn(cfg: &NoteCfg) -> (StickyManager, HWND, HWND) {
    let mut mgr = StickyManager::new();
    mgr.ensure(cfg);
    mgr.spawn(cfg);
    let deadline = Instant::now() + Duration::from_secs(5);
    let pair = loop {
        if let (Some(f), Some(b)) = (find_by_class(CLASS_NAME), find_by_class(BODY_CLASS)) {
            break (f, b);
        }
        if Instant::now() > deadline {
            panic!("5 秒内没建出可见的便签窗");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    (mgr, pair.0, pair.1)
}

/// ★ 场景 1：标题栏空白处按住左键拖 120×80，窗口必须跟着走。
#[test]
#[ignore]
fn dragging_the_titlebar_moves_the_note() {
    let (_mgr, frame, body) = spawn(&NoteCfg::default());
    let r0 = window_rect(frame);
    let b0 = window_rect(body);
    // 标题栏空白处：x 让开左侧 ＋ 按钮（占 25px），y 取栏高一半。
    let px = r0.left + 60;
    let py = r0.top + 13;
    let code = hit_test(frame, px, py);
    println!("WM_NCHITTEST @ ({px},{py}) = {code}（HTCAPTION=2）");
    assert_eq!(code, HT_CAPTION, "标题栏空白处必须是 HTCAPTION");

    let ok = move_to(px, py);
    std::thread::sleep(Duration::from_millis(150));
    unsafe {
        let mut p = windows::Win32::Foundation::POINT::default();
        let _ = GetCursorPos(&mut p);
        let top = WindowFromPoint(p);
        println!(
            "移动注入 send={ok}  实际光标 ({},{})  该点最上层窗 = {:?}（标题栏窗 {:?} / 正文窗 {:?}）\
             \n  该点窗类名 = {}",
            p.x,
            p.y,
            top.0,
            frame.0,
            body.0,
            class_of(top)
        );
    }
    let down = mouse(MOUSEEVENTF_LEFTDOWN, 0, 0);
    std::thread::sleep(Duration::from_millis(150));
    // 跨线程读模态循环状态（GetCapture 跨线程恒 0，不可用）。
    let (in_move, cap) = gui_state(frame);
    println!(
        "按下 send={down}  GUI_INMOVESIZE={in_move}  线程捕获窗 = 0x{cap:x}（标题栏窗 = {:?}）",
        frame.0
    );
    // 分三小步移动，模拟人手拖动（一步 120px 的跳变理论上也该行，稳一点）。
    for (dx, dy) in [(40, 25), (40, 25), (40, 30)] {
        let mv = move_to(px + dx, py + dy);
        std::thread::sleep(Duration::from_millis(120));
        println!("  移动到 ({},{}) send={mv}", px + dx, py + dy);
    }
    let (in_move2, _) = gui_state(frame);
    println!("移动中 GUI_INMOVESIZE={in_move2}");
    let up = mouse(MOUSEEVENTF_LEFTUP, 0, 0);
    println!("松开 send={up}");
    std::thread::sleep(Duration::from_millis(250));

    let r1 = window_rect(frame);
    let b1 = window_rect(body);
    println!("拖动前 标题栏窗 {r0:?}");
    println!("拖动后 标题栏窗 {r1:?}");
    println!("拖动前 正文窗 {b0:?}");
    println!("拖动后 正文窗 {b1:?}");
    assert!(
        r1.left != r0.left || r1.top != r0.top,
        "★ 按住了标题栏却拖不动：HTCAPTION 返回了但窗口没走 —— 有东西截胡了这次按下"
    );
    assert_eq!(
        (b1.left - b0.left, b1.top - b0.top),
        (r1.left - r0.left, r1.top - r0.top),
        "正文窗必须与标题栏窗同步位移（不同步 = 拖的时候正文留在原地）"
    );
}

/// ★★ 场景 2：只改背景色 alpha → 正文区的像素**不许变**（变了就是两窗叠色）。
///
/// ★ **同一张窗**内做对比：先后开两张窗的话，两次采样之间桌面底色可能自己变
///   （DWM 幽灵/别的窗口聚散），会出"均匀漂移"的假阳性。`mgr.apply` 只把
///   背景色 alpha 一个变量打上去，窗口、位置、桌面全部不动。
#[test]
#[ignore]
fn bg_alpha_must_not_leak_into_the_body_area() {
    let mut hi = NoteCfg::default();
    hi.bg[3] = 255; // 背景色完全不透明
    let mut lo = NoteCfg::default();
    lo.bg[3] = 5; // 只有背景色 alpha 变了（拉满信号：修复前漏 30+，修复后只有 ±2 噪声）

    let (mut mgr, frame, body) = spawn(&hi);
    std::thread::sleep(Duration::from_millis(600)); // 等 DWM 合成
    let sample = |frame: HWND, body: HWND| -> ((u8, u8, u8), (u8, u8, u8)) {
        let rf = window_rect(frame);
        let rb = window_rect(body);
        let body_px = screen_pixel((rb.left + rb.right) / 2, (rb.top + rb.bottom) / 2);
        let title_px = screen_pixel(rf.left + 60, rf.top + 13);
        (rgb(body_px), rgb(title_px))
    };
    let (body_hi, title_hi) = sample(frame, body);

    mgr.apply(&lo);
    std::thread::sleep(Duration::from_millis(600));
    let (body_lo, title_lo) = sample(frame, body);
    println!("背景色 a=255：正文区 {body_hi:?}  标题栏 {title_hi:?}");
    println!("背景色 a=  5：正文区 {body_lo:?}  标题栏 {title_lo:?}");

    assert_ne!(
        title_hi, title_lo,
        "标题栏像素必须随背景色 alpha 变（否则这条用例取不到信号：可能没读到合成结果）"
    );
    // 容差 ±4 只吸收 GetPixel/DWM 的采样噪声（实测 ±2）；修复前的真实漏值 ≥ 3~30，
    // 且信号已用 a=5 拉满——真正串层时漂移远超容差。
    let leak = [
        body_hi.0.abs_diff(body_lo.0),
        body_hi.1.abs_diff(body_lo.1),
        body_hi.2.abs_diff(body_lo.2),
    ];
    assert!(
        leak.iter().all(|&d| d <= 4),
        "★ 只改背景色 alpha，正文区像素却变了 {leak:?} —— 标题栏窗的层 alpha 漏到正文区了"
    );
}
