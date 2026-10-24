//! probe-group: fast
//! 贴图「放大之后图片没放大、只是边框在放大」的**端到端**真机探针（`#[ignore]`）。
//!
//! # 它要回答的问题
//!
//! 用户报：贴图长图，放大之后图片并没有真的放大，只是外部边框在放大；复制很多行文字再
//! 贴图（极端宽高比）就能看出来。
//!
//! 前一条 `pin_zoom_clamp_probe` 已经量清楚钳制的来源：贴图窗固定大小 + frameless 的样式仍是
//! `WS_OVERLAPPEDWINDOW` 一族，`CreateWindowExW` / `SetWindowPos` 都按 `WM_GETMINMAXINFO` 的
//! 跟踪尺寸收口——宽硬下限 `SM_CXMINTRACK`（本机 136）、高上限约一屏（本机 1100），且
//! `ptMinTrackSize` 压不到 136 以下。修法是 windui 补丁 095：贴图窗建窗即 `WS_POPUP`
//! （探针变体 3 实测两项收口全部消失）。本探针就是**修法的验收**：跑真实 exe，量三件事。
//!
//! # 判据（唯一能同时覆盖"窗口"与"画面"的办法）
//!
//! 几何单测只能证明算式；离屏单测没有真窗口（`hwnd==0`，走的是 `target_size` 回退）。
//! 所以这条探针**把整条链跑真的**：起 curseen.exe → 剪贴板放一张**纯品红、极端宽高比**
//! （60×720，12:1）的图 → 按贴图快捷键 → 找到贴图窗 → 滚轮放大 3 格（100%→130%）→
//! 对**同一扇窗**截两次图，量纯品红的像素包围盒：
//!
//! | 阶段 | 量什么 | 期望 |
//! |---|---|---|
//! | 出厂 | 窗口样式 | 含 `WS_POPUP`（补丁 095） |
//! | 出厂 | 窗口外框 | **精确** `图片 + 2×FRAME`（不被 136 顶宽、不被 1100 限高） |
//! | 出厂 | 客户区 | == 外框 |
//! | 出厂 | 品红包围盒 | **精确** `(FRAME, FRAME, 图宽, 图高)`——铺满，不留黑边 |
//! | 放大后 | 窗口外框 / 品红包围盒 | 都 ×1.3（不再"只长边框、图片冻结"） |
//!
//! 品红是刻意选的：两层描边（`RING_OUTER`/`RING_INNER`/选中蓝/悬停蓝）与百分比角标的
//! 底色文字全都不是品红，所以"品红包围盒"**恰好等于图片占据的范围**，不用猜哪圈是图、
//! 哪圈是边。
//!
//! # 四个必须这么做的细节（都踩过）
//!
//! 1. **光标锚在窗口右下角**：`on_wheel` 的 `zoom_anchor` 让光标底下那个图片像素保持不动，
//!    锚在右下 → 窗口往**左上**长。锚在左上会让长图往屏幕外长，截图只能量到半张。
//! 2. **先把窗停到屏幕右下角**（`SetWindowPos`）：居中开窗 + 往左上长，一样会溢出。
//! 3. **每一格都要重新瞄光标**：窗口长过头会把光标甩出窗外，后面几格的滚轮事件就落到
//!    桌面上了（第一版探针就是这么只量到 1 格的）。所以按下标算当时的右下内角再滚。
//! 4. **每一格都要重试到生效**：`SendInput` 注入的滚轮事件**偶发整条丢失**（实测 3 格里
//!    丢过 1 格）。不重试就会拿到"只长了 2 格"的**假红**——看着像缩放坏了，其实是注入丢了。
//!    判据是窗高有没有变（单调递增，一变就是生效了）。
//!    ★ 重试能救回来 ⇒ 只是注入丢事件；若某格反复"窗高没变"，才该怀疑应用侧把 `pct` 改了
//!      却没改尺寸（`on_wheel` 里 `pct.set` 在 `SetWindowPos` 之前，中间任何早退都会造成
//!      脱层，那时增量会一次跳 20% 而不是 10%）。两者靠增量大小就能分开。
//!
//! 跑法：`python build/probe.py run pin_zoom_e2e_probe -- --nocapture`
//! （会临时改写 `build/config.json` 并起真进程，跑完还原。）

use std::time::{Duration, Instant};

use curseen_ui::core::config::Config;
use windows::Win32::Foundation::{HGLOBAL, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
    SelectObject, CAPTUREBLT, DIB_RGB_COLORS, HDC, HGDIOBJ, SRCCOPY,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    SetClipboardData,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_MOUSE, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE, MOUSE_EVENT_FLAGS, MOUSEINPUT,
    VIRTUAL_KEY, VK_LCONTROL, VK_LMENU, VK_P,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClientRect, GetWindowLongPtrW, GetWindowRect, GetWindowTextW, SetCursorPos,
    SetWindowPos, GWL_STYLE, GWL_EXSTYLE, HWND_TOPMOST, SWP_NOACTIVATE, WS_POPUP,
};

const EXE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "\\..\\build\\curseen.exe");
const CFG: &str = concat!(env!("CARGO_MANIFEST_DIR"), "\\..\\build\\config.json");
const TITLE_PREFIX: &str = "CurSeen 贴图 #";

/// 极端宽高比源图：60×720（12:1），纯品红。品红同时是"图在哪"的判据色。
const IMG_W: i32 = 60;
const IMG_H: i32 = 720;
/// 描边宽度（物理像素，与 `pin.rs::FRAME` 同值）。窗口 = 图片 + 2×FRAME。
const FRAME: i32 = 2;
/// 滚几格（每格 10%）。3 格 = 100% → 130%。
const NOTCHES: i32 = 3;
/// 相对误差容差（取整 / 锚点漂移都给足，仍远小于"完全没长"的量级）。
const TOL: f32 = 0.08;

// ─────────────────────────── 小工具 ───────────────────────────

fn sleep(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

fn kill_app() {
    let _ = std::process::Command::new("taskkill")
        .args(["/F", "/IM", "curseen.exe"])
        .output();
    sleep(800);
}

/// 把一张纯品红 32bpp DIB 放进剪贴板（`CF_DIB`，与 `platform::clipboard::get_image` 对齐）。
fn set_clipboard_magenta(w: i32, h: i32) {
    unsafe {
        if OpenClipboard(None).is_err() {
            panic!("OpenClipboard 失败");
        }
        let _ = EmptyClipboard();
        let hdr = 40usize;
        let stride = (w as usize) * 4;
        let size = hdr + stride * (h as usize);
        let hmem = GlobalAlloc(GMEM_MOVEABLE, size).expect("GlobalAlloc");
        let p = GlobalLock(hmem);
        assert!(!p.is_null(), "GlobalLock 失败");
        let bytes = std::slice::from_raw_parts_mut(p.cast::<u8>(), size);
        // BITMAPINFOHEADER：biHeight 正 = 自下而上（get_image 两种都吃）。
        bytes[0..4].copy_from_slice(&40u32.to_le_bytes());
        bytes[4..8].copy_from_slice(&(w as u32).to_le_bytes());
        bytes[8..12].copy_from_slice(&(h as u32).to_le_bytes());
        bytes[12..14].copy_from_slice(&1u16.to_le_bytes()); // planes
        bytes[14..16].copy_from_slice(&32u16.to_le_bytes()); // bpp
        bytes[16..20].copy_from_slice(&0u32.to_le_bytes()); // BI_RGB
        for px in bytes[hdr..].chunks_exact_mut(4) {
            px.copy_from_slice(&[0xFF, 0x00, 0xFF, 0xFF]); // BGRA
        }
        let _ = GlobalUnlock(hmem);
        // 句柄所有权交给系统，失败才自己释放。
        if SetClipboardData(8u32, Some(windows::Win32::Foundation::HANDLE(hmem.0))).is_err() {
            panic!("SetClipboardData 失败");
        }
        let _ = CloseClipboard();
    }
}

/// 读回剪贴板当前持有的格式与文本（探针前置条件自检用）。
///
/// 为什么必须有它：`read_clipboard_content` 是**文本优先**的——剪贴板里只要还有文本，
/// 放进去的品红 DIB 就不会被贴，量到的将是一张文字栅格（第一版探针就这么栽过：贴出来
/// 449×45 的文字图，尺寸断言报的是"要 64×724 实际 453×49"，看着像缩放 bug，其实是前置
/// 条件没成立）。所以在按快捷键**之前**先把剪贴板里到底有什么断言一遍。
fn clipboard_probe() -> (bool, bool, String) {
    unsafe {
        if OpenClipboard(None).is_err() {
            return (false, false, String::new());
        }
        let has_dib = IsClipboardFormatAvailable(8).is_ok();
        let has_text = IsClipboardFormatAvailable(13).is_ok();
        let mut text = String::new();
        if has_text {
            if let Ok(h) = GetClipboardData(13) {
                let p = GlobalLock(HGLOBAL(h.0)) as *const u16;
                if !p.is_null() {
                    let mut n = 0usize;
                    while *p.add(n) != 0 && n < 4096 {
                        n += 1;
                    }
                    text = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
                    let _ = GlobalUnlock(HGLOBAL(h.0));
                }
            }
        }
        let _ = CloseClipboard();
        (has_dib, has_text, text)
    }
}

/// 按一下 `Ctrl+Alt+P`（贴图快捷键；`SendInput` 注入的输入照样能触发 `RegisterHotKey`）。
fn send_pin_hotkey() {
    unsafe {
        let keys: [(VIRTUAL_KEY, bool); 6] = [
            (VK_LCONTROL, true),
            (VK_LMENU, true),
            (VK_P, true),
            (VK_P, false),
            (VK_LMENU, false),
            (VK_LCONTROL, false),
        ];
        let inputs: Vec<INPUT> = keys
            .iter()
            .map(|(vk, down)| INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: *vk,
                        wScan: 0,
                        dwFlags: if *down {
                            KEYBD_EVENT_FLAGS(0)
                        } else {
                            KEYEVENTF_KEYUP
                        },
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            })
            .collect();
        let n = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        assert_eq!(n as usize, inputs.len(), "SendInput 键盘只投递了 {n} 条");
    }
}

/// 在光标处滚 `notches` 格（正数=放大）。
///
/// ★ 调用方必须每格重新算坐标再调本函数（窗口长大会把光标甩出窗外），见文件头细节 3。
fn wheel_at(x: i32, y: i32, notches: i32) {
    unsafe {
        let _ = SetCursorPos(x, y);
        sleep(60);
        for _ in 0..notches {
            // 绝对坐标要先归一化到 0..65535。
            let mx = (x * 65535) / 1920;
            let my = (y * 65535) / 1080;
            let inputs = [
                INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 {
                        mi: MOUSEINPUT {
                            dx: mx,
                            dy: my,
                            mouseData: 120,
                            dwFlags: MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_MOVE | MOUSE_EVENT_FLAGS(0x0800), // WHEEL
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                },
            ];
            let n = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
            assert_eq!(n as usize, 1, "SendInput 滚轮只投递了 {n} 条");
            sleep(120);
        }
    }
}

/// 按标题前缀找**可见**的贴图窗（`EnumWindows` 也列隐藏窗，必须过滤）。
fn find_pin_window() -> Option<HWND> {
    unsafe extern "system" fn cb(h: HWND, l: LPARAM) -> windows::core::BOOL {
        let out = &mut *(l.0 as *mut Vec<HWND>);
        let mut buf = [0u16; 128];
        let n = GetWindowTextW(h, &mut buf);
        let title = String::from_utf16_lossy(&buf[..n as usize]);
        if title.starts_with("CurSeen 贴图 #") {
            out.push(h);
        }
        windows::core::BOOL(1)
    }
    let mut all: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut all as *mut Vec<HWND> as isize));
    }
    all.into_iter().next()
}

fn rect_of(h: HWND) -> (i32, i32, i32, i32) {
    let mut r = RECT::default();
    unsafe {
        let _ = GetWindowRect(h, &mut r);
    }
    (r.left, r.top, r.right - r.left, r.bottom - r.top)
}

/// 截屏幕上 `(x,y,w,h)` 这一块（BGRA 原始像素）。
fn grab(x: i32, y: i32, w: i32, h: i32) -> Vec<u8> {
    unsafe {
        let screen: HDC = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bi = windows::Win32::Graphics::Gdi::BITMAPINFO {
            bmiHeader: windows::Win32::Graphics::Gdi::BITMAPINFOHEADER {
                biSize: std::mem::size_of::<windows::Win32::Graphics::Gdi::BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(Some(screen), &bi, DIB_RGB_COLORS, &mut bits, None, 0)
            .expect("CreateDIBSection 失败");
        let old = SelectObject(mem, HGDIOBJ(bmp.0));
        assert!(
            BitBlt(mem, 0, 0, w, h, Some(screen), x, y, SRCCOPY | CAPTUREBLT).is_ok(),
            "BitBlt 失败"
        );
        let raw = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize).to_vec();
        let _ = SelectObject(mem, old);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        raw
    }
}

/// 纯品红像素的包围盒 `(x, y, w, h)`（相对传入的截图左上角）。一个都没有就 `None`。
///
/// 阈值刻意宽松（R>180, G<90, B>180）：只要求"把品红和描边灰/蓝分开"。
/// `w` 用来把线性下标还原成 `(x, y)`；高度用不上（下标 ÷ 宽度即得行号）。
fn magenta_bbox(raw: &[u8], w: i32) -> Option<(i32, i32, i32, i32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for (i, px) in raw.chunks_exact(4).enumerate() {
        let (b, g, r) = (px[0], px[1], px[2]);
        if r > 180 && g < 90 && b > 180 {
            let (x, y) = ((i as i32) % w, (i as i32) / w);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    (x1 >= x0 && y1 >= y0).then(|| (x0, y0, x1 - x0 + 1, y1 - y0 + 1))
}

/// 诊断：把截图第 `row` 行按颜色**游程**打印出来（一行里到底是什么，一眼看清）。
fn dump_row(raw: &[u8], w: i32, h: i32, row: i32) {
    let row = row.clamp(0, h - 1) as usize;
    let line = &raw[row * (w as usize) * 4..(row + 1) * (w as usize) * 4];
    let mut runs: Vec<([u8; 3], i32)> = Vec::new();
    for px in line.chunks_exact(4) {
        let c = [px[2], px[1], px[0]];
        match runs.last_mut() {
            Some(last) if last.0 == c => last.1 += 1,
            _ => runs.push((c, 1)),
        }
    }
    let desc: Vec<String> = runs
        .iter()
        .map(|(c, n)| format!("{c:?}x{n}"))
        .collect();
    println!("  第 {row} 行游程（共 {} 段）：{}", runs.len(), desc.join(" | "));
}

// ─────────────────────────── 探针本体 ───────────────────────────

#[test]
#[ignore]
fn zooming_a_pin_really_grows_the_image() {
    // ⓪ 收尾：不管中不中断，都要还原配置 + 杀掉进程（用 drop guard 最稳）。
    let backup = std::fs::read(CFG).ok();
    struct Guard(Option<Vec<u8>>);
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = std::process::Command::new("taskkill")
                .args(["/F", "/IM", "curseen.exe"])
                .output();
            match &self.0 {
                Some(b) => {
                    let _ = std::fs::write(CFG, b);
                }
                None => {
                    let _ = std::fs::remove_file(CFG);
                }
            }
        }
    }
    let _guard = Guard(backup);

    kill_app();

    // ① 写一份"开了截图与贴图、并把贴图键设成 Ctrl+Alt+P"的配置。
    let mut cfg = Config::default();
    cfg.screenshot.enabled = true;
    cfg.screenshot.hotkey_pin_clipboard = "Ctrl+Alt+P".into();
    std::fs::write(CFG, serde_json::to_string_pretty(&cfg).expect("配置序列化")).expect("写配置");

    // ② 剪贴板放极端宽高比图，起进程，按快捷键。
    set_clipboard_magenta(IMG_W, IMG_H);
    // ②-a ★ 前置条件自检：剪贴板里必须是"只有 DIB、没有文本"。
    //      `read_clipboard_content` 文本优先，粘着文本时贴出来的是一张文字栅格，
    //      后面所有尺寸断言都会红得莫名其妙（第一版探针实测栽在这里）。
    {
        let (has_dib, has_text, text) = clipboard_probe();
        println!("剪贴板前置：CF_DIB={has_dib} CF_UNICODETEXT={has_text} 文本={text:?}");
        assert!(
            has_dib && !has_text,
            "剪贴板前置条件不成立：CF_DIB={has_dib} CF_UNICODETEXT={has_text}。\
             \n  有文本时贴图会走 text_raster（文本优先），量到的不是品红图。\
             \n  当时文本内容：{text:?}"
        );
    }
    std::process::Command::new(EXE)
        .current_dir(std::path::Path::new(EXE).parent().unwrap())
        .spawn()
        .expect("启动 curseen.exe");
    // ②-b ★ 起进程后**轮询**剪贴板（诊断用）：实测这 3 秒里剪贴板偶发被写进文本
    //      （`read_clipboard_content` 文本优先 ⇒ 贴出来变成一张文字栅格，尺寸断言报
    //      "要 64×724 实际 468×49"，红得很像缩放 bug，其实是前置条件被第三方改写）。
    //      轮询只为把"什么时候、被写了什么"暴露出来；**真正的前置保证是下一段
    //      "按快捷键前一刻重放 + 复验"**——只查一次不重放，探针就会时红时绿。
    {
        let mut seen: Option<String> = None;
        for i in 0..30 {
            sleep(100);
            let (has_dib, has_text, text) = clipboard_probe();
            if has_text && seen.is_none() {
                seen = Some(text.clone());
                println!(
                    "  ★ 起进程约 {}ms 后剪贴板出现文本（CF_DIB={has_dib}）：{text:?}",
                    (i + 1) * 100
                );
            }
        }
        if seen.is_none() {
            println!("  起进程 3s 内剪贴板未被改写（仍只有 CF_DIB）");
        }
    }
    // ②-c ★ 按快捷键的**前一刻**重放一次图并复验：不管这 3 秒里被谁写过，都以这一刻
    //      的真实状态为准。这是探针确定性的来源。
    set_clipboard_magenta(IMG_W, IMG_H);
    {
        let (has_dib, has_text, text) = clipboard_probe();
        println!("按快捷键前剪贴板：CF_DIB={has_dib} CF_UNICODETEXT={has_text} 文本={text:?}");
        assert!(
            has_dib && !has_text,
            "重放后剪贴板仍有文本：CF_DIB={has_dib} CF_UNICODETEXT={has_text}。\
             \n  有文本时贴图会走 text_raster（文本优先），量到的不是品红图。\
             \n  当时文本内容：{text:?}"
        );
    }
    send_pin_hotkey();

    let pin = {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(h) = find_pin_window() {
                break h;
            }
            if Instant::now() >= deadline {
                panic!("20 秒内没等到贴图窗（标题 {TITLE_PREFIX}N）");
            }
            sleep(300);
        }
    };
    let (px, py, pw, ph) = rect_of(pin);
    let mut cr = RECT::default();
    unsafe {
        let _ = GetClientRect(pin, &mut cr);
    }
    let (cw, ch) = (cr.right - cr.left, cr.bottom - cr.top);
    let title = {
        let mut buf = [0u16; 128];
        let n = unsafe { GetWindowTextW(pin, &mut buf) };
        String::from_utf16_lossy(&buf[..n as usize])
    };
    let style = unsafe { GetWindowLongPtrW(pin, GWL_STYLE) } as u32;
    let exstyle = unsafe { GetWindowLongPtrW(pin, GWL_EXSTYLE) } as u32;
    println!(
        "贴图窗：{title:?} 外框 ({px},{py}) {pw}x{ph} 客户区 {cw}x{ch}（源图 {IMG_W}x{IMG_H}）
         style={style:#x} exstyle={exstyle:#x}"
    );

    // ③ 停到屏幕右下角：`zoom_anchor` 锚在右下 → 窗口往左上长，不会溢出屏幕。
    let (sw, sh) = (1920, 1080);
    let park = (sw - pw - 20, sh - ph - 20);
    unsafe {
        let _ = SetWindowPos(pin, Some(HWND_TOPMOST), park.0, park.1, pw, ph, SWP_NOACTIVATE);
    }
    sleep(400);

    // ③-b ★★ 本 bug 的核心判据：**建窗尺寸必须精确等于"图片 + 2×FRAME"**。
    //
    // 修前（windui 补丁 095 之前）这条一定红：样式是 `WS_OVERLAPPEDWINDOW & ~(THICKFRAME|
    // MAXIMIZEBOX)`，`CreateWindowExW` 当场把 64 宽的窗顶到 `SM_CXMINTRACK`（本机 136），
    // 于是品红只占 60、两侧各 38px 黑边——用户看到的"图片没铺满"就是这个。
    let (vw, vh) = {
        let (_, _, w, h) = rect_of(pin);
        (w, h)
    };
    let (want_w, want_h) = (IMG_W + 2 * FRAME, IMG_H + 2 * FRAME);
    assert_eq!(
        (vw, vh),
        (want_w, want_h),
        "贴图窗出厂尺寸不是 图片+2×FRAME：要 {want_w}x{want_h}，实际 {vw}x{vh}。\
         \n  这就是'极端宽高比图片不铺满'的根因——窗口被系统跟踪尺寸收口顶宽/限高了。\
         \n  期望样式含 WS_POPUP（windui 补丁 095），实际 style={style:#x}"
    );
    assert_eq!(
        (cw, ch),
        (vw, vh),
        "客户区 != 外框（{cw}x{ch} vs {vw}x{vh}）：frameless 的 WM_NCCALCSIZE 没生效？"
    );
    assert!(
        style & WS_POPUP.0 != 0,
        "贴图窗样式不含 WS_POPUP（style={style:#x}）：补丁 095 没生效，\
         系统跟踪尺寸收口会回来（宽下限 SM_CXMINTRACK / 高上限约一屏）"
    );
    println!(
        "出厂判据通过：窗 {vw}x{vh} == 图 {IMG_W}x{IMG_H} + 2×FRAME，客户区相同，style={style:#x}"
    );

    // ④ 放大前：窗口 + 品红包围盒。
    let (wx, wy, ww, wh) = rect_of(pin);
    let before_raw = grab(wx, wy, ww, wh);
    let before = magenta_bbox(&before_raw, ww)
        .unwrap_or_else(|| panic!("放大前一粒细胞红都没有（窗 {ww}x{wh}）——贴图没贴上来？"));
    println!("放大前：窗 {ww}x{wh}，品红 {before:?}");
    dump_row(&before_raw, ww, wh, wh / 2);
    dump_row(&before_raw, ww, wh, 4);

    // ④-b 品红必须**铺满**描边以内的区域（不留黑边）。修前这里会是 (38,2,60,720)——
    //     左右各 38px 黑边，正是"图片没放大、边框在放大"的另一半现象。
    assert_eq!(
        before,
        (FRAME, FRAME, IMG_W, IMG_H),
        "图片没有铺满客户区：期望品红铺满 ({FRAME},{FRAME},{IMG_W},{IMG_H})，实际 {before:?}。\
         \n  放大前窗 {ww}x{wh}（应为 {want_w}x{want_h}）——窗口被顶宽后 fit_aspect 把图居中了"
    );

    // ⑤ 光标锚在窗口右下角内侧，滚 3 格（100% → 130%）。
    //    ★ 两件事必须做：
    //      ① 每一格都按**当时的**窗口右下内角重新瞄——窗口往左上长，光标不动就会掉到
    //         窗外，后面几格的滚轮事件落到桌面上（第一版探针只量到 1 格就是这个原因）；
    //      ② 每格**重试到生效**——注入的滚轮事件偶发整条丢失（实测 3 格里丢过 1 格，
    //         得到"只长了 2 格"的假红）。判据是窗高有没有变：窗高单调递增，一变就是生效了。
    //         ★ 重试能救回来说明只是注入丢事件；若某一格"窗高没变"反复出现，那才要怀疑
    //           应用侧把 `pct` 改了却没改尺寸（`on_wheel` 里 `pct.set` 在 `SetWindowPos`
    //           之前，中间任何早退都会造成脱层）——那时增量会是一次跳 20% 而不是 10%。
    let mut done = 0_i32;
    let mut lost = 0_i32;
    for _ in 0..(NOTCHES * 8) {
        if done == NOTCHES {
            break;
        }
        let (cx0, cy0, cw0, ch0) = rect_of(pin);
        wheel_at(cx0 + cw0 - 4, cy0 + ch0 - 4, 1);
        sleep(150);
        let (_, _, _, ch1) = rect_of(pin);
        if ch1 > ch0 {
            done += 1;
            println!(
                "  第 {done} 格生效：窗高 {ch0} → {ch1}（锚点 {},{}）",
                cx0 + cw0 - 4,
                cy0 + ch0 - 4
            );
        } else {
            lost += 1;
            println!("  ★ 这一格注入后窗高没变（{ch0}）——滚轮事件整条丢了，重试");
        }
    }
    assert_eq!(
        done, NOTCHES,
        "只生效了 {done}/{NOTCHES} 格（另有 {lost} 次注入丢失）。\
         \n  放大前窗 {ww}x{wh} 品红 {before:?}；当前窗 {:?}",
        rect_of(pin)
    );
    sleep(400);

    let (wx2, wy2, ww2, wh2) = rect_of(pin);
    let after_raw = grab(wx2, wy2, ww2, wh2);
    let after = magenta_bbox(&after_raw, ww2)
        .unwrap_or_else(|| panic!("放大后一颗粒品红都没有（窗 {ww2}x{wh2}）——图没了？"));
    println!("放大后：窗 {ww2}x{wh2}，品红 {after:?}");
    dump_row(&after_raw, ww2, wh2, wh2 / 2);

    // ⑥ 判据。
    let want = 1.0 + NOTCHES as f32 * 0.1;
    let grow = |a: i32, b: i32| b as f32 / a.max(1) as f32;
    println!(
        "窗口 {:.2}x / {:.2}x；品红 {:.2}x / {:.2}x（期望都 ≈ {want:.2}x）",
        grow(ww, ww2),
        grow(wh, wh2),
        grow(before.2, after.2),
        grow(before.3, after.3)
    );

    // ⑥-a 窗口本身必须按百分比长（否则是缩放压根没生效，不是画错）。
    for (name, a, b) in [
        ("窗宽", ww, ww2),
        ("窗高", wh, wh2),
        ("图宽", before.2, after.2),
        ("图高", before.3, after.3),
    ] {
        let k = grow(a, b);
        assert!(
            (k - want).abs() <= TOL,
            "{name} 没按 {want:.2}x 长：{a} → {b}（{k:.2}x）。\
             \n  放大前窗 {ww}x{wh} 品红 {before:?}；放大后窗 {ww2}x{wh2} 品红 {after:?}"
        );
    }

    // ⑥-b 放大后图片仍必须**铺满**描边以内（不被 fit_aspect 居中留黑边）。
    //     修前这条一定红：窗高被钳在 1100 以内时图片高度先冻结，宽度还在长，
    //     `fit_aspect` 只能按被钳的那一维取矩形 → 另一维出现黑边。
    let (inner_w, inner_h) = (ww2 - 2 * FRAME, wh2 - 2 * FRAME);
    assert_eq!(
        (after.2, after.3),
        (inner_w, inner_h),
        "放大后图片没铺满客户区：品红 {}x{}，描边以内 {}x{}。\
         \n  放大后窗 {ww2}x{wh2} 品红 {after:?}——有一维被系统跟踪尺寸钳住了",
        after.2,
        after.3,
        inner_w,
        inner_h
    );
    println!("\n★ 通过：图片跟着窗口一起按 {want:.2}x 长大，且始终铺满（描边与内容同源）。");
}
