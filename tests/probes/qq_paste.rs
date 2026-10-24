//! QQ/TIM 粘贴失败端到端探针：对**真 TIM 聊天窗**复刻粘贴链，像素差分验证。
//!
//! # 背景（2026-10-07）
//!
//! CurSeen 面板粘贴在 TIM/QQ 输入框落空且目标播放提示音；textus（普通权限、纯文本、
//! 逐键 SendInput）同场景据用户观察可用。第一版探针用 UIA 判据全假红：TIM 是
//! DirectUI，焦点控件类名 `TXGuiFoundation`（一窗一 HWND），UIA 无 Value/Text 模式
//! 可读 ⇒ 改用**截屏前后像素差分**判据（正文文本变化 ⇒ 数千像素差；光标闪烁仅
//! 数十像素，阈值 500 分得开）。
//!
//! # 变体（正交拆差异）
//!
//! | 变体 | 剪贴板内容 | 发键方式 | 检验的差异 |
//! |------|-----------|---------|-----------|
//! | A    | 纯文本     | textus 逐键 | **权限级别**（普通 vs 提权子进程）|
//! | B    | 纯文本+HTML Format | textus 逐键 | **HTML 格式是否惹祸** |
//! | C    | 纯文本     | 一次 SendInput 打包 | **注入风格**（旧实现）|
//!
//! 三个变体依次打进 TIM 输入框，各自与上一步截图比对。
//!
//! # 运行前提
//!
//! 用户已点进 TIM 聊天输入框（TIM 逻辑焦点在输入框）。会真的往输入框打三段
//! `CURSEEN_?_…` 标记（不发送）；提权轮弹一次 UAC。经 `python build/probe.py run qq_paste`。
#![cfg(windows)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use windows::core::{PCWSTR, HSTRING};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, SRCCOPY,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, SetFocus as SetFocusWin, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, VIRTUAL_KEY,
};
use windows::Win32::UI::Shell::{IsUserAnAdmin, ShellExecuteW};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, EnumWindows, GetForegroundWindow, GetGUIThreadInfo,
    GetWindowRect, GetWindowTextLengthW, GetWindowThreadProcessId, IsWindowVisible,
    PostMessageW, RegisterClassW, SetForegroundWindow, SetWindowPos, ShowWindow, WM_CLOSE,
    GUITHREADINFO, HWND_TOPMOST, SWP_SHOWWINDOW, SW_RESTORE, WNDCLASSW, WS_POPUP, WS_VISIBLE,
};

const CF_UNICODETEXT: u32 = 13u32;

// ── 找 TIM 主窗 ────────────────────────────────────────────

fn tim_pids() -> Vec<u32> {
    std::process::Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq TIM.exe", "/FO", "CSV", "/NH"])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter_map(|l| l.split(',').nth(1)?.trim_matches('"').parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

thread_local! {
    static HIT: std::cell::RefCell<Option<(HWND, u32)>> = const { std::cell::RefCell::new(None) };
    static PIDS: std::cell::RefCell<Vec<u32>> = const { std::cell::RefCell::new(Vec::new()) };
}

const BOOL_CONTINUE: windows::core::BOOL = windows::core::BOOL(1);

unsafe extern "system" fn enum_cb(h: HWND, _l: LPARAM) -> windows::core::BOOL {
    unsafe {
        if !IsWindowVisible(h).as_bool() {
            return BOOL_CONTINUE;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        if !PIDS.with(|p| p.borrow().contains(&pid)) {
            return BOOL_CONTINUE;
        }
        if GetWindowTextLengthW(h) == 0 {
            return BOOL_CONTINUE;
        }
        let better = HIT.with(|c| match *c.borrow() {
            Some((_, old_len)) => (GetWindowTextLengthW(h) as u32) > old_len,
            None => true,
        });
        if better {
            HIT.with(|c| *c.borrow_mut() = Some((h, GetWindowTextLengthW(h) as u32)));
        }
        BOOL_CONTINUE
    }
}

fn find_tim() -> Option<HWND> {
    let pids = tim_pids();
    if pids.is_empty() {
        return None;
    }
    PIDS.with(|p| *p.borrow_mut() = pids);
    HIT.with(|c| *c.borrow_mut() = None);
    unsafe {
        let _ = EnumWindows(Some(enum_cb), LPARAM(0));
    }
    HIT.with(|c| *c.borrow()).map(|(h, _)| h)
}

// ── 前台就位 ───────────────────────────────────────────────

unsafe fn alt_tap() {
    unsafe {
        // textus 式无害键：Ctrl down/up 相邻（不成组合键）。
        let seq = [(0x11u16, false), (0x11u16, true)];
        let mut inputs: [INPUT; 2] = std::mem::zeroed();
        for (i, (vk, up)) in seq.iter().enumerate() {
            inputs[i].r#type = INPUT_KEYBOARD;
            inputs[i].Anonymous.ki = KEYBDINPUT {
                wVk: VIRTUAL_KEY(*vk),
                wScan: 0,
                dwFlags: if *up { KEYBD_EVENT_FLAGS(KEYEVENTF_KEYUP.0) } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: 0,
            };
        }
        let _ = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

unsafe fn ensure_foreground(target: HWND) -> bool {
    unsafe {
        if GetForegroundWindow() == target {
            return true;
        }
        let _ = SetForegroundWindow(target);
        std::thread::sleep(Duration::from_millis(60));
        if GetForegroundWindow() == target {
            return true;
        }
        alt_tap();
        std::thread::sleep(Duration::from_millis(30));
        let _ = SetForegroundWindow(target);
        std::thread::sleep(Duration::from_millis(60));
        GetForegroundWindow() == target
    }
}

// ── 截屏差分判据 ───────────────────────────────────────────

unsafe fn shot(target: HWND) -> Option<Vec<u8>> {
    unsafe {
        let mut r = Default::default();
        GetWindowRect(target, &mut r).ok()?;
        let w = (r.right - r.left).max(1);
        let h = (r.bottom - r.top).max(1);
        let hdc = GetDC(None);
        let mem = CreateCompatibleDC(Some(hdc));
        let bmp = CreateCompatibleBitmap(hdc, w, h);
        let old = SelectObject(mem, bmp.into());
        let _ = BitBlt(mem, 0, 0, w, h, Some(hdc), r.left, r.top, SRCCOPY);
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
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
        GetDIBits(mem, bmp, 0, h.try_into().unwrap(), Some(buf.as_mut_ptr() as *mut _), &mut bmi, DIB_RGB_COLORS);
        let _ = SelectObject(mem, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
        let _ = ReleaseDC(None, hdc);
        Some(buf)
    }
}

/// 差异字节数（BGRA 4 通道逐字节比对）。
fn diff_bytes(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b.iter()).filter(|(x, y)| x != y).count()
}

/// 把 shot() 的裸 BGRA 像素存成 BMP（负高度=top-down，32bpp 行天然 4 字节对齐），
/// 供人眼复核"粘贴后输入框里到底有什么"——像素差分只给数字，不给形状。
unsafe fn shot_save(target: HWND, path: &str, buf: &[u8]) {
    unsafe {
        let mut r = Default::default();
        if GetWindowRect(target, &mut r).is_err() {
            return;
        }
        let w = (r.right - r.left).max(1) as u32;
        let h = (r.bottom - r.top).max(1) as u32;
        let mut out = Vec::with_capacity(54 + buf.len());
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&(54u32 + buf.len() as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&54u32.to_le_bytes());
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&(w as i32).to_le_bytes());
        out.extend_from_slice(&(-(h as i32)).to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(buf.len() as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(buf);
        let _ = std::fs::write(path, out);
    }
}

// ── UIA（仅作环境描述，不作判据 —— TIM 是 DirectUI 读不到内容）──

unsafe fn uia_focused_class() -> String {
    unsafe {
        if CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_err() {
            return "CoInit失败".into();
        }
        let Ok(auto): Result<windows::Win32::UI::Accessibility::IUIAutomation, _> = CoCreateInstance(
            &windows::Win32::UI::Accessibility::CUIAutomation,
            None,
            CLSCTX_ALL,
        ) else {
            return "CoCreate失败".into();
        };
        let Ok(el) = auto.GetFocusedElement() else { return "FocusedEl失败".into() };
        el.CurrentClassName().map(|s| s.to_string()).unwrap_or_else(|_| "?".into())
    }
}

// ── 写剪贴板 ───────────────────────────────────────────────

unsafe fn write_plain(text: &str) {
    unsafe {
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        wide.push(0);
        let bytes = wide.len() * 2;
        let hg = GlobalAlloc(GMEM_MOVEABLE, bytes).expect("GlobalAlloc");
        let p = GlobalLock(hg) as *mut u16;
        std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
        let _ = GlobalUnlock(hg);
        if OpenClipboard(None).is_ok() {
            let _ = EmptyClipboard();
            let _ = SetClipboardData(CF_UNICODETEXT, Some(HANDLE(hg.0)));
            let _ = CloseClipboard();
        } else {
            let _ = windows::Win32::Foundation::GlobalFree(Some(hg));
        }
    }
}

/// 最小合法 CF_HTML（浏览器同款骨架，偏移程序内计算）。
fn build_html(fragment: &str) -> Vec<u8> {
    let body = format!("<html><body><!--StartFragment-->{fragment}<!--EndFragment--></body></html>");
    let frame = |sh: usize, eh: usize, sf: usize, ef: usize| {
        format!(
            "Version:0.9\r\nStartHTML:{sh:010}\r\nEndHTML:{eh:010}\r\nStartFragment:{sf:010}\r\nEndFragment:{ef:010}\r\n"
        )
    };
    let hl = frame(0, 0, 0, 0).len();
    let html_start = hl;
    let frag_start = html_start + "<html><body><!--StartFragment-->".len();
    let frag_end = frag_start + fragment.len();
    let html_end = html_start + body.len();
    let mut out = frame(html_start, html_end, frag_start, frag_end).into_bytes();
    out.extend_from_slice(body.as_bytes());
    out
}

// ── 发键 ───────────────────────────────────────────────────

unsafe fn send_key_once(vk: u16, up: bool) {
    unsafe {
        let mut input: INPUT = std::mem::zeroed();
        input.r#type = INPUT_KEYBOARD;
        input.Anonymous.ki = KEYBDINPUT {
            wVk: VIRTUAL_KEY(vk),
            wScan: 0,
            dwFlags: if up { KEYBD_EVENT_FLAGS(KEYEVENTF_KEYUP.0) } else { KEYBD_EVENT_FLAGS(0) },
            time: 0,
            dwExtraInfo: 0,
        };
        let _ = SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
}

/// textus 式：逐键 + 10ms 间隔，左 Ctrl(0x11)。
unsafe fn send_textus_ctrl_v() {
    unsafe {
        send_key_once(0x11, false);
        std::thread::sleep(Duration::from_millis(10));
        send_key_once(0x56, false);
        std::thread::sleep(Duration::from_millis(10));
        send_key_once(0x56, true);
        std::thread::sleep(Duration::from_millis(10));
        send_key_once(0x11, true);
    }
}

/// 旧实现：一次 SendInput 打包 4 事件，右 Ctrl(0xA3)。
unsafe fn send_batched_ctrl_v() {
    unsafe {
        let seq: [(u16, bool); 4] = [(0xA3, false), (0x56, false), (0x56, true), (0xA3, true)];
        let mut inputs: [INPUT; 4] = std::mem::zeroed();
        for (i, (vk, up)) in seq.iter().enumerate() {
            inputs[i].r#type = INPUT_KEYBOARD;
            inputs[i].Anonymous.ki = KEYBDINPUT {
                wVk: VIRTUAL_KEY(*vk),
                wScan: 0,
                dwFlags: if *up {
                    KEYBD_EVENT_FLAGS(KEYEVENTF_KEYUP.0)
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: 0,
            };
        }
        let _ = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

// ── 变体执行 ───────────────────────────────────────────────

// ── 模拟"面板抢走前台"的最小窗口 ───────────────────────────

unsafe extern "system" fn dummy_proc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    DefWindowProcW(h, msg, w, l)
}

unsafe fn make_dummy() -> HWND {
    unsafe {
        let name: Vec<u16> = "CurSeenQqPasteProbeDummy\0".encode_utf16().collect();
        let cls = WNDCLASSW {
            lpfnWndProc: Some(dummy_proc),
            lpszClassName: PCWSTR(name.as_ptr()),
            hInstance: GetModuleHandleW(None).unwrap().into(),
            ..Default::default()
        };
        RegisterClassW(&cls);
        let h = CreateWindowExW(
            windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0),
            PCWSTR(name.as_ptr()),
            PCWSTR(name.as_ptr()),
            WS_POPUP | WS_VISIBLE,
            60,
            60,
            280,
            160,
            None,
            None,
            Some(cls.hInstance.into()),
            None,
        )
        .expect("probe dummy window");
        ShowWindow(h, windows::Win32::UI::WindowsAndMessaging::SW_SHOW);
        let _ = SetWindowPos(
            h,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE
                | windows::Win32::UI::WindowsAndMessaging::SWP_NOSIZE
                | SWP_SHOWWINDOW,
        );
        h
    }
}

struct Out {
    admin: bool,
    found: bool,
    fg_ok: bool,
    focus_cls: String,
    a_plain_seq: usize,
    b_html_seq: usize,
    c_plain_batched: usize,
    d_steal_120: usize,
    e_steal_600: usize,
    f_real_html: usize,
    g_real_plain: usize,
    h_single_cjk: usize,
    i_multi_ascii: usize,
    j_multi_cjk_settle: usize,
    k_single_ascii_prod: usize,
    l_steal60: usize,
    m_steal200: usize,
    n_steal500: usize,
}

fn stamp(tag: &str) -> String {
    let s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("CURSEEN_{tag}_{s}")
}

unsafe fn run_variants() -> Out {
    unsafe {
        let admin = IsUserAnAdmin().as_bool();
        let hwnd = find_tim();
        let mut out = Out {
            admin,
            found: hwnd.is_some(),
            fg_ok: false,
            focus_cls: "-".into(),
            a_plain_seq: 0,
            b_html_seq: 0,
            c_plain_batched: 0,
            d_steal_120: 0,
            e_steal_600: 0,
            f_real_html: 0,
            g_real_plain: 0,
            h_single_cjk: 0,
            i_multi_ascii: 0,
            j_multi_cjk_settle: 0,
            k_single_ascii_prod: 0,
            l_steal60: 0,
            m_steal200: 0,
            n_steal500: 0,
        };
        let Some(target) = hwnd else { return out };

        let _ = ShowWindow(target, SW_RESTORE);
        out.fg_ok = ensure_foreground(target);
        std::thread::sleep(Duration::from_millis(300));
        out.focus_cls = uia_focused_class();

        // 目标线程焦点控件类名（只作描述）。
        let tid = GetWindowThreadProcessId(target, None);
        let mut gi = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        if GetGUIThreadInfo(tid, &mut gi).is_ok() {
            println!("guithreadinfo focus 是否无效={}", gi.hwndFocus.is_invalid());
        }

        // 变体 A：纯文本 + textus 逐键。像素差分 ≥2000 字节（>500 像素）判到货。
        let pre = shot(target).unwrap_or_default();
        let a = stamp("A");
        write_plain(&a);
        std::thread::sleep(Duration::from_millis(200));
        let _ = ensure_foreground(target);
        std::thread::sleep(Duration::from_millis(120));
        send_textus_ctrl_v();
        std::thread::sleep(Duration::from_millis(900));
        let after_a = shot(target).unwrap_or_default();
        out.a_plain_seq = diff_bytes(&pre, &after_a);
        println!("A plain+seq {a} 差异字节={}", out.a_plain_seq);

        // 变体 B：纯文本 + HTML Format + textus 逐键。
        let b = stamp("B");
        let html = build_html(&format!("<b>{b}</b>"));
        curseen_ui::platform::clipboard_html::write_text_and_html(&b, Some(&html));
        std::thread::sleep(Duration::from_millis(200));
        let _ = ensure_foreground(target);
        std::thread::sleep(Duration::from_millis(120));
        send_textus_ctrl_v();
        std::thread::sleep(Duration::from_millis(900));
        let after_b = shot(target).unwrap_or_default();
        out.b_html_seq = diff_bytes(&after_a, &after_b);
        println!("B html+seq {b} 差异字节={}", out.b_html_seq);

        // 变体 C：纯文本 + 打包注入（旧实现）。
        let c = stamp("C");
        write_plain(&c);
        std::thread::sleep(Duration::from_millis(200));
        let _ = ensure_foreground(target);
        std::thread::sleep(Duration::from_millis(120));
        send_batched_ctrl_v();
        std::thread::sleep(Duration::from_millis(900));
        let after_c = shot(target).unwrap_or_default();
        out.c_plain_batched = diff_bytes(&after_b, &after_c);
        println!("C plain+batched {c} 差异字节={}", out.c_plain_batched);

        // 变体 D：★ 复刻生产——先让"面板"抢走前台+焦点，再切回 TIM，等 120ms 发键。
        let d = stamp("D");
        let dummy = make_dummy();
        let _ = SetForegroundWindow(dummy);
        SetFocusWin(Some(dummy));
        std::thread::sleep(Duration::from_millis(300));
        write_plain(&d);
        std::thread::sleep(Duration::from_millis(200));
        let _ = ensure_foreground(target); // 与 do_paste 同款：直切 + Alt tap 兜底
        std::thread::sleep(Duration::from_millis(120));
        send_textus_ctrl_v();
        std::thread::sleep(Duration::from_millis(900));
        let after_d = shot(target).unwrap_or_default();
        out.d_steal_120 = diff_bytes(&after_c, &after_d);
        println!("D steal→back 120ms {d} 差异字节={}", out.d_steal_120);

        // 变体 E：同 D，但切回后等 600ms 再发键。
        let e = stamp("E");
        let _ = SetForegroundWindow(dummy);
        SetFocusWin(Some(dummy));
        std::thread::sleep(Duration::from_millis(300));
        write_plain(&e);
        std::thread::sleep(Duration::from_millis(200));
        let _ = ensure_foreground(target);
        std::thread::sleep(Duration::from_millis(600));
        send_textus_ctrl_v();
        std::thread::sleep(Duration::from_millis(900));
        let after_e = shot(target).unwrap_or_default();
        out.e_steal_600 = diff_bytes(&after_d, &after_e);
        println!("E steal→back 600ms {e} 差异字节={}", out.e_steal_600);
        let _ = PostMessageW(Some(dummy), WM_CLOSE, WPARAM(0), LPARAM(0));

        // 变体 F/G：★ 生产真实数据回放（2026-10-07 二轮）——生产日志确认 textus 化后
        // 用户真机粘贴仍"落空+提示音"，而 A–E 全绿。唯一未回放的变量 = 真实条目数据。
        // F = 真实条目全文（CJK/CRLF，280 字）+ 真实落盘 CF_HTML 字节（68KB，浏览器捕获）
        //     走生产 write_text_and_html（含自写屏蔽）——与生产粘贴写侧完全同款；
        // G = 同一全文、纯文本（html=None）——隔离"html 字节是否惹祸"。
        // 判读：F 落空而 G 到货 ⇒ 真实 CF_HTML 字节在 TIM 上被判死（修复=写回前重建头/安检）；
        //       F、G 都落空 ⇒ 差异在条目数据本身（CJK/多行/长度），与 html 无关。
        let f_text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "\\..\\temp\\probe_f_text.txt"
        ))
        .expect("probe: 真实条目全文");
        let f_html = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "\\..\\build\\temp\\clipboard\\texts\\f95a1805b647b539.html"
        ))
        .expect("probe: 真实落盘 CF_HTML");
        println!(
            "F/G 数据：text={} 字节 html={} 字节",
            f_text.len(),
            f_html.len()
        );

        let f = stamp("F");
        curseen_ui::platform::clipboard_html::write_text_and_html(&format!("{f} {f_text}"), Some(&f_html));
        std::thread::sleep(Duration::from_millis(200));
        let _ = ensure_foreground(target);
        std::thread::sleep(Duration::from_millis(120));
        send_textus_ctrl_v();
        std::thread::sleep(Duration::from_millis(900));
        let after_f = shot(target).unwrap_or_default();
        out.f_real_html = diff_bytes(&after_e, &after_f);
        println!("F real-text+real-html {f} 差异字节={}", out.f_real_html);

        let g = stamp("G");
        curseen_ui::platform::clipboard_html::write_text_and_html(&format!("{g} {f_text}"), None);
        std::thread::sleep(Duration::from_millis(200));
        let _ = ensure_foreground(target);
        std::thread::sleep(Duration::from_millis(120));
        send_textus_ctrl_v();
        std::thread::sleep(Duration::from_millis(900));
        let after_g = shot(target).unwrap_or_default();
        out.g_real_plain = diff_bytes(&after_f, &after_g);
        println!("G real-text plain {g} 差异字节={}", out.g_real_plain);
        unsafe {
            shot_save(
                target,
                concat!(env!("CARGO_MANIFEST_DIR"), "\\..\\temp\\qq_after_G.bmp"),
                &after_g,
            )
        };

        // ── 二轮矩阵（2026-10-07）：拆 G 落空的因子 ──
        // H = 单行 CJK 纯文本（write_text_and_html(None)）→ CJK 因子
        // I = 多行 ASCII 纯文本                              → 多行因子
        // J = 多行 CJK 真实全文，切回后等 2000ms 再发键        → 时序因子（G 是 120ms）
        // K = 单行 ASCII 纯文本（生产写侧同款、晚位次）        → 屏蔽格式/位次因子
        //   （K 若落空而 A 到货 ⇒ 问题在生产写侧形状或时序，不在内容）
        let shots = concat!(env!("CARGO_MANIFEST_DIR"), "\\..\\temp");

        let h = stamp("H");
        curseen_ui::platform::clipboard_html::write_text_and_html(&format!("{h} 中文单行测试"), None);
        std::thread::sleep(Duration::from_millis(200));
        let _ = ensure_foreground(target);
        std::thread::sleep(Duration::from_millis(120));
        send_textus_ctrl_v();
        std::thread::sleep(Duration::from_millis(900));
        let after_h = shot(target).unwrap_or_default();
        out.h_single_cjk = diff_bytes(&after_g, &after_h);
        println!("H single-cjk {h} 差异字节={}", out.h_single_cjk);
        unsafe { shot_save(target, &format!(r"{shots}\qq_after_H.bmp"), &after_h) };

        let i = stamp("I");
        curseen_ui::platform::clipboard_html::write_text_and_html(
            &format!("{i}\r\n{i}\r\n{i}"),
            None,
        );
        std::thread::sleep(Duration::from_millis(200));
        let _ = ensure_foreground(target);
        std::thread::sleep(Duration::from_millis(120));
        send_textus_ctrl_v();
        std::thread::sleep(Duration::from_millis(900));
        let after_i = shot(target).unwrap_or_default();
        out.i_multi_ascii = diff_bytes(&after_h, &after_i);
        println!("I multi-ascii {i} 差异字节={}", out.i_multi_ascii);
        unsafe { shot_save(target, &format!(r"{shots}\qq_after_I.bmp"), &after_i) };

        let j = stamp("J");
        curseen_ui::platform::clipboard_html::write_text_and_html(&format!("{j} {f_text}"), None);
        std::thread::sleep(Duration::from_millis(200));
        let _ = ensure_foreground(target);
        std::thread::sleep(Duration::from_millis(2000)); // ★ 与 G 唯一差异：等 2s
        send_textus_ctrl_v();
        std::thread::sleep(Duration::from_millis(900));
        let after_j = shot(target).unwrap_or_default();
        out.j_multi_cjk_settle = diff_bytes(&after_i, &after_j);
        println!("J multi-cjk settle2s {j} 差异字节={}", out.j_multi_cjk_settle);
        unsafe { shot_save(target, &format!(r"{shots}\qq_after_J.bmp"), &after_j) };

        let k = stamp("K");
        curseen_ui::platform::clipboard_html::write_text_and_html(&k, None);
        std::thread::sleep(Duration::from_millis(200));
        let _ = ensure_foreground(target);
        std::thread::sleep(Duration::from_millis(120));
        send_textus_ctrl_v();
        std::thread::sleep(Duration::from_millis(900));
        let after_k = shot(target).unwrap_or_default();
        out.k_single_ascii_prod = diff_bytes(&after_j, &after_k);
        println!("K single-ascii prod-write {k} 差异字节={}", out.k_single_ascii_prod);
        unsafe { shot_save(target, &format!(r"{shots}\qq_after_K.bmp"), &after_k) };

        // ── 三轮矩阵（2026-10-07）：生产置顶面板场景的等压复刻 ──
        // 生产：面板长期是前台（置顶连续取用）→ 点行 → 切回 TIM → 仅 60ms → 发键。
        // L/M/N = 用 dummy 长期（5s）抢走前台+焦点，再切回 TIM，分别等 60/200/500ms。
        // 若 L 落空而 M/N 到货 ⇒ 根因=生产 60ms 太短，修=抬 PASTE_SEND_TIMER。
        let steal_back = |ms: u64, tag: String| -> Vec<u8> {
            unsafe {
                let dummy2 = make_dummy();
                let _ = SetForegroundWindow(dummy2);
                SetFocusWin(Some(dummy2));
                std::thread::sleep(Duration::from_millis(5000)); // 置顶面板浏览时长
                write_plain(&tag);
                std::thread::sleep(Duration::from_millis(200));
                let _ = ensure_foreground(target);
                std::thread::sleep(Duration::from_millis(ms));
                send_textus_ctrl_v();
                std::thread::sleep(Duration::from_millis(900));
                let after = shot(target).unwrap_or_default();
                let _ = PostMessageW(Some(dummy2), WM_CLOSE, WPARAM(0), LPARAM(0));
                after
            }
        };

        let l = stamp("L");
        let after_l = steal_back(60, l.clone());
        out.l_steal60 = diff_bytes(&after_k, &after_l);
        println!("L steal5s→back60ms {l} 差异字节={}", out.l_steal60);
        unsafe { shot_save(target, &format!(r"{shots}\qq_after_L.bmp"), &after_l) };

        let m = stamp("M");
        let after_m = steal_back(200, m.clone());
        out.m_steal200 = diff_bytes(&after_l, &after_m);
        println!("M steal5s→back200ms {m} 差异字节={}", out.m_steal200);
        unsafe { shot_save(target, &format!(r"{shots}\qq_after_M.bmp"), &after_m) };

        let n = stamp("N");
        let after_n = steal_back(500, n.clone());
        out.n_steal500 = diff_bytes(&after_m, &after_n);
        println!("N steal5s→back500ms {n} 差异字节={}", out.n_steal500);
        unsafe { shot_save(target, &format!(r"{shots}\qq_after_N.bmp"), &after_n) };

        out
    }
}

fn out_path(child: bool) -> PathBuf {
    std::env::temp_dir().join(if child {
        "curseen_qq_paste_elevated.json"
    } else {
        "curseen_qq_paste_normal.json"
    })
}

fn write_out(p: &PathBuf, o: &Out) {
    let _ = std::fs::write(
        p,
        format!(
            r#"{{"admin":{},"found":{},"fg_ok":{},"a_diff":{},"b_diff":{},"c_diff":{},"d_steal_120":{},"e_steal_600":{},"f_real_html":{},"g_real_plain":{},"h":{},"i":{},"j":{},"k":{},"l":{},"m":{},"n":{},"focus_cls":"{}"}}"#,
            o.admin, o.found, o.fg_ok, o.a_plain_seq, o.b_html_seq, o.c_plain_batched,
            o.d_steal_120, o.e_steal_600, o.f_real_html, o.g_real_plain,
            o.h_single_cjk, o.i_multi_ascii, o.j_multi_cjk_settle, o.k_single_ascii_prod,
            o.l_steal60, o.m_steal200, o.n_steal500,
            o.focus_cls.replace('"', "'")
        ),
    );
}

fn dump(tag: &str, o: &Out) {
    println!(
        "[{tag}] A={} B={} C={} D={} E={} F={} G={} H={} I={} J={} K={} L(抢5s还60ms)={} M(抢5s还200ms)={} N(抢5s还500ms)={}（≥2000=到货）",
        o.a_plain_seq, o.b_html_seq, o.c_plain_batched, o.d_steal_120, o.e_steal_600,
        o.f_real_html, o.g_real_plain, o.h_single_cjk, o.i_multi_ascii,
        o.j_multi_cjk_settle, o.k_single_ascii_prod, o.l_steal60, o.m_steal200, o.n_steal500
    );
}

/// 普通权限轮 + 拉起提权子进程跑同一组变体。
#[test]
#[ignore = "对真 TIM 窗口注入按键并弹 UAC；经 build/probe.py 按需运行"]
fn qq_paste_differential() {
    unsafe {
        if tim_pids().is_empty() {
            println!("TIM 未运行，探针跳过（请先打开 TIM 聊天窗并点进输入框）");
            return;
        }
        let out = run_variants();
        write_out(&out_path(false), &out);
        dump("normal", &out);

        // 拉提权副本（子进程跑 qq_paste_child，无 env 依赖）。
        let elevated_path = out_path(true);
        let _ = std::fs::remove_file(&elevated_path);
        let exe = std::env::current_exe().unwrap();
        let verb = HSTRING::from("runas");
        let file = HSTRING::from(exe.as_os_str());
        let args = HSTRING::from("--exact qq_paste_child --ignored --nocapture");
        let r = ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR(args.as_ptr()),
            None,
            windows::Win32::UI::WindowsAndMessaging::SHOW_WINDOW_CMD(0),
        );
        let mut elev = None;
        if (r.0 as isize) > 32 {
            let dl = Instant::now();
            while dl.elapsed() < Duration::from_secs(120) {
                if let Ok(s) = std::fs::read_to_string(&elevated_path) {
                    elev = Some(s);
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        } else {
            println!("提权副本拉起失败（ShellExecuteW={:?}）", r.0);
        }
        match elev {
            Some(s) => println!("elevated: {s}"),
            None => println!("elevated: 未等到结果（UAC 被拦或超时）"),
        }

        assert!(out.found, "没找到 TIM 顶层窗");
        assert!(out.fg_ok, "普通权限下 TIM 没能成为前台");
        println!(
            "结论辅助：A(normal)差异={}，A(elevated)见上 JSON 的 a_diff；\
             A/B 差异对比=HTML 格式影响；A/C 差异对比=注入风格影响",
            out.a_plain_seq
        );
    }
}

/// 提权子进程入口（父进程以 runas 拉起，`--exact qq_paste_child --ignored`）。
#[test]
#[ignore = "仅由 qq_paste_differential 以提权方式拉起"]
fn qq_paste_child() {
    unsafe {
        let out = run_variants();
        write_out(&out_path(true), &out);
        dump("elevated", &out);
    }
}
