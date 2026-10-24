//! AI 对话窗真机取证探针（一次性诊断 + 回归， nid: 0929-2355 / 0930-0035）。
//!
//! 2026-09-30 六条判据（全部真机实测，不看注释）：
//!   1. 正文全黑：全窗红色像素数 = 0（上一版 AI 回复是红的，本轮需求取消）
//!   2. 边框平面：对话区/输入框外圈 1px 纯黑、紧邻外侧就是白底——3D 斜面会
//!      在边框两侧画出多档灰阶，这里用像素直接钉死
//!   3. 块间恒 1 个空行：跑**两轮**对话，第二轮用户输入故意带尾换行
//!      （= 模拟「按 Enter 再发送」这个真实习惯，它正是旧版叠出 2 空行的元凶）
//!   4. 输入框滚动条按需：空内容时隐藏、内容超窗时出现（GetScrollBarInfo 读 rgstate）
//!   5. 按钮黑框 + 悬停变色：PostMessage WM_MOUSEMOVE 后采样按钮中心像素
//!   6. 内存里程碑：进程工作集逐步记账——riched20.dll 的归因 = 加载前后差值
//!      （上一版对话区用 RichEdit，本轮回归 EDIT 后不再加载它）
//!
//! 跑法：`cargo run --release --example ai_chat_probe`
//! 产物：`temp/ai_probe_request.json`（真实请求体）、`temp/ai_probe_report.txt`（判据）、
//! `temp/ai_probe_capture_full.raw`（全窗像素）

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{FreeLibrary, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBRUSH, HGDIOBJ,
    SRCCOPY,
};
use windows::Win32::System::LibraryLoader::LoadLibraryW;
use windows::Win32::System::ProcessStatus::{
    GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, FindWindowW, GetCursorPos, GetDlgItem,
    GetScrollBarInfo, GetWindowRect, PostMessageW, RegisterClassExW, SendMessageW, SetWindowPos,
    SetWindowTextW, ShowWindow, WindowFromPoint, CS_HREDRAW, CS_VREDRAW, ES_AUTOVSCROLL,
    ES_MULTILINE, HWND_TOPMOST, OBJID_VSCROLL, SCROLLBARINFO, SWP_NOMOVE, SWP_NOSIZE, SW_SHOW,
    WINDOW_STYLE, WM_CLOSE, WNDCLASSEXW, WNDCLASS_STYLES, WS_CHILD, WS_OVERLAPPEDWINDOW,
    WS_VISIBLE, WS_VSCROLL,
};

/// 对话窗自定义消息：WM_APP+2 = 输入框 Ctrl+Enter 的投递路径（真实发送分支）。
const WM_APP_CHAT_SEND: u32 = 0x8000 + 2;
/// STATE_SYSTEM_INVISIBLE（滚动条隐藏态位；SDK oleacc.h，免引 Accessibility 特性）。
const STATE_SYSTEM_INVISIBLE: u32 = 0x8000;
/// WM_GETTEXT / WM_GETTEXTLENGTH。
const WM_GETTEXT: u32 = 0x000D;
const WM_GETTEXTLENGTH: u32 = 0x000E;
/// 控件 ID（与 ai_chat.rs 同源）。
const CI_DISPLAY: i32 = 1;
const CI_BTN_CLEAR: i32 = 2;
const CI_BTN_SEND: i32 = 3;
const CI_EDIT: i32 = 4;

/// UTF-16 NUL 结尾。
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 读子窗口全文。
fn get_text(h: HWND) -> String {
    unsafe {
        let len = SendMessageW(h, WM_GETTEXTLENGTH, Some(WPARAM(0)), Some(LPARAM(0))).0;
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; (len + 8) as usize];
        let n = SendMessageW(
            h,
            WM_GETTEXT,
            Some(WPARAM(buf.len())),
            Some(LPARAM(buf.as_mut_ptr() as isize)),
        )
        .0;
        String::from_utf16_lossy(&buf[..(n as usize).min(len as usize)])
    }
}

/// 进程内存（KiB）：（工作集，私有提交）。
fn mem_kib() -> (u64, u64) {
    unsafe {
        let mut pmc = PROCESS_MEMORY_COUNTERS_EX::default();
        let cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        if GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut pmc as *mut _ as *mut PROCESS_MEMORY_COUNTERS,
            cb,
        )
        .is_err()
        {
            return (0, 0);
        }
        (
            pmc.WorkingSetSize as u64 / 1024,
            pmc.PrivateUsage as u64 / 1024,
        )
    }
}

/// 竖向滚动条是否可见（false = 隐藏态 STATE_SYSTEM_INVISIBLE）。
fn vscroll_visible(h: HWND) -> Option<bool> {
    unsafe {
        let mut sbi = SCROLLBARINFO {
            cbSize: std::mem::size_of::<SCROLLBARINFO>() as u32,
            ..Default::default()
        };
        GetScrollBarInfo(h, OBJID_VSCROLL, &mut sbi).ok()?;
        Some((sbi.rgstate[0] & STATE_SYSTEM_INVISIBLE) == 0)
    }
}

/// BitBlt 抓屏（调用方已把窗置顶，抓到的就是真实合成像素）。
/// 返回 `(宽, 高, 自上而下的 BGRA 行)`。
fn capture_window(h: HWND) -> Option<(i32, i32, Vec<u8>)> {
    unsafe {
        let mut rc = RECT::default();
        let _ = GetWindowRect(h, &mut rc);
        let w = rc.right - rc.left;
        let ht = rc.bottom - rc.top;
        if w <= 0 || ht <= 0 {
            return None;
        }
        let sdc = GetDC(None);
        let mem = CreateCompatibleDC(Some(sdc));
        let bmp = CreateCompatibleBitmap(sdc, w, ht);
        let old = SelectObject(mem, HGDIOBJ(bmp.0 as *mut _));
        let _ = BitBlt(mem, 0, 0, w, ht, Some(sdc), rc.left, rc.top, SRCCOPY);
        let mut bits = vec![0u8; (w as usize) * (ht as usize) * 4];
        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -ht, // 负 = 自上而下
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let got = GetDIBits(
            mem,
            bmp,
            0,
            ht as u32,
            Some(bits.as_mut_ptr() as *mut _),
            &mut bi,
            DIB_RGB_COLORS,
        );
        SelectObject(mem, old);
        let _ = DeleteObject(HGDIOBJ(bmp.0 as *mut _));
        let _ = DeleteDC(mem);
        let _ = ReleaseDC(None, sdc);
        if got == 0 {
            return None;
        }
        Some((w, ht, bits))
    }
}

/// 把**物理光标**移到屏幕绝对坐标 (x, y)。
///
/// 悬停必须由真实光标驱动：`TrackMouseEvent(TME_LEAVE)` 的离开判定看的是物理位置，
/// 伪造 `WM_MOUSEMOVE` 会被随后那条真的 `WM_MOUSELEAVE` 撤销。
fn move_cursor_to(x: i32, y: i32) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE, MOUSEINPUT,
    };
    let (sw, sh) = unsafe {
        (
            windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
                windows::Win32::UI::WindowsAndMessaging::SM_CXSCREEN,
            ),
            windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
                windows::Win32::UI::WindowsAndMessaging::SM_CYSCREEN,
            ),
        )
    };
    let nx = (x as f32 * 65535.0 / sw as f32) as i32;
    let ny = (y as f32 * 65535.0 / sh as f32) as i32;
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: nx,
                dy: ny,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe {
        let _ = SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
}

/// 在屏幕绝对坐标 (x, y) 真实点击一下（左键）。
///
/// 用途：打字路径会触发中文 IME 的候选浮窗，它**叠在我们窗口上**，BitBlt 抓屏
/// 会把候选窗像素一并抓进来污染所有判据（2026-09-30 实测 [1]/[2] 全毁）。
/// 点一下只读的对话区把焦点从输入框拿走，候选窗即收。
fn click_at(x: i32, y: i32) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
        MOUSEINPUT, MOUSE_EVENT_FLAGS,
    };
    move_cursor_to(x, y);
    std::thread::sleep(Duration::from_millis(120));
    let mk = |dw: MOUSE_EVENT_FLAGS| INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: dw,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe {
        let _ = SendInput(
            &[mk(MOUSEEVENTF_LEFTDOWN), mk(MOUSEEVENTF_LEFTUP)],
            std::mem::size_of::<INPUT>() as i32,
        );
    }
}

fn main() {
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../temp");
    std::fs::create_dir_all(&out).unwrap();
    let mut report = String::new();
    let mut line = |s: String| {
        report.push_str(&s);
        report.push('\n');
        println!("{s}");
    };

    // ── [6] 内存里程碑：基线 ──
    let m0 = mem_kib();
    line(format!("[6] M0 基线 = WS {} KiB / 私有 {} KiB", m0.0, m0.1));

    // ── [6] riched20.dll 归因：上一版对话区用它，加载成本记一笔再卸掉 ──
    unsafe {
        // LoadLibraryW 在 0.62 里返回 Result<HMODULE>。
        let loaded = LoadLibraryW(PCWSTR(wide("riched20.dll").as_ptr())).ok();
        let m1 = mem_kib();
        line(format!(
            "[6] M1 加载 riched20.dll 后 = WS {} KiB / 私有 {} KiB（差值 = 富文本路线的真实成本）",
            m1.0, m1.1
        ));
        if let Some(h) = loaded {
            let _ = FreeLibrary(h);
        }
        let m1b = mem_kib();
        line(format!(
            "[6] M1b FreeLibrary 后 = WS {} KiB / 私有 {} KiB（能卸回 = 本版不加载它就省这笔）",
            m1b.0, m1b.1
        ));
    }

    // ── 假 HTTP 服务：两轮对话，回复正文首尾各塞两个多余换行（验 trim + 块间空行） ──
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let srv_out = out.clone();
    std::thread::spawn(move || {
        let mut round = 0usize;
        while let Ok((mut stream, _)) = listener.accept() {
            round += 1;
            // 第 3 轮延迟 1.5s：给「忙态悬停」判据留出稳定的忙碌窗口。
            if round == 3 {
                std::thread::sleep(std::time::Duration::from_millis(1500));
            }
            let mut r = BufReader::new(stream.try_clone().unwrap());
            let mut clen = 0usize;
            loop {
                let mut l = String::new();
                if r.read_line(&mut l).unwrap_or(0) == 0 {
                    break;
                }
                if l.trim().is_empty() {
                    break;
                }
                if let Some(v) = l.to_ascii_lowercase().strip_prefix("content-length:") {
                    clen = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; clen];
            if clen > 0 {
                let _ = r.read_exact(&mut body);
            }
            std::fs::write(srv_out.join("ai_probe_request.json"), &body).ok();
            // 回复正文：**首尾各两个换行**——若前端不 trim，屏幕上就会出现多余空行。
            let content = format!(
                "\\n\\n这是第 {round} 轮 AI 的回复第一行。\\n这是第二行，验《块间只留一个空行》。\\n\\n"
            );
            let resp_body = format!(
                "{{\"choices\":[{{\"message\":{{\"role\":\"assistant\",\"content\":\"{content}\"}}}}]}}"
            );
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                resp_body.len(),
                resp_body
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        }
    });

    // ── 走真实脚本：ai_chat.lua 自己组系统提示词并开窗（引擎每步记内存账） ──
    // ★ 顺序要紧：引擎的账（M2a/M2b）必须在**任何窗口**创建前记——窗口机制会拉起
    //   一串系统 DLL 且卸载不掉，先开过裸窗再量引擎，引擎的数就被污染了。
    let script_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/lua/scripts");
    let notes: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let notes_t = notes.clone();
    let t = std::thread::spawn(move || {
        let note = |s: String| {
            println!("[6] {s}");
            notes_t.lock().unwrap().push(s);
        };
        let eng = match curseen_ui::scripting::engine::ScriptingEngine::new(&script_dir) {
            Ok(e) => e,
            Err(e) => {
                println!("[probe] engine new failed: {e}");
                return;
            }
        };
        let m = mem_kib();
        note(format!(
            "M2a 脚本引擎创建后 = WS {} KiB / 私有 {} KiB",
            m.0, m.1
        ));
        if let Err(e) = eng.load_script("ai_chat") {
            println!("[probe] load failed: {e}");
            return;
        }
        let m = mem_kib();
        note(format!(
            "M2b ai_chat 脚本载入后 = WS {} KiB / 私有 {} KiB",
            m.0, m.1
        ));
        let mut params: HashMap<String, serde_json::Value> = HashMap::new();
        params.insert(
            "ai_url".into(),
            serde_json::Value::String(format!("http://127.0.0.1:{port}/chat/completions")),
        );
        params.insert(
            "ai_model".into(),
            serde_json::Value::String("probe-model".into()),
        );
        params.insert(
            "ai_api".into(),
            serde_json::Value::String("probe-key".into()),
        );
        params.insert("hotkey".into(), serde_json::Value::String(String::new()));
        match eng.call_run_ctx("composite", &[], &params) {
            Ok(v) => println!("[probe] run returned {v:?}"),
            Err(e) => println!("[probe] run error: {e}"),
        }
    });

    // ── 等窗出现 ──
    let cls = wide("CurSeen.AiChat");
    let hwnd = {
        let mut found = HWND::default();
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if let Ok(h) = unsafe { FindWindowW(PCWSTR(cls.as_ptr()), PCWSTR::null()) } {
                if !h.is_invalid() {
                    found = h;
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        found
    };
    if hwnd.is_invalid() {
        line("[FATAL] 没找到 AI 对话窗".into());
        std::fs::write(out.join("ai_probe_report.txt"), report).ok();
        return;
    }
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        // ★ 置顶后再抓屏：BitBlt 抓屏抓的是「Z 序最上面那个窗」。
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE,
        );
    }
    let m2 = mem_kib();
    line(format!(
        "[6] M2 脚本+Lua+开窗后 = WS {} KiB / 私有 {} KiB（M1b→M2 差值 = 本版开窗成本）",
        m2.0, m2.1
    ));
    // 引擎线程的分步记账（M2a/M2b）倒进报告。
    for s in notes.lock().unwrap().drain(..) {
        line(format!("[6] {s}"));
    }

    // ── [6] 裸窗 + 两个多行 EDIT：真实窗已开的场合，再开一扇窗的**边际**成本 ──
    unsafe {
        let cls = wide("CurSeen.ProbeBare");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: WNDCLASS_STYLES(CS_HREDRAW.0 | CS_VREDRAW.0),
            lpfnWndProc: Some({
                unsafe extern "system" fn bare_proc(
                    h: HWND,
                    m: u32,
                    w: WPARAM,
                    l: LPARAM,
                ) -> LRESULT {
                    DefWindowProcW(h, m, w, l)
                }
                bare_proc
            }),
            hbrBackground: HBRUSH(6 as *mut _), // (COLOR_WINDOW+1)
            lpszClassName: PCWSTR(cls.as_ptr()),
            ..Default::default()
        };
        let _ = RegisterClassExW(&wc);
        let child_st = WINDOW_STYLE(
            WS_CHILD.0 | WS_VISIBLE.0 | WS_VSCROLL.0 | (ES_MULTILINE | ES_AUTOVSCROLL) as u32,
        );
        if let Ok(bwin) = CreateWindowExW(
            Default::default(),
            PCWSTR(cls.as_ptr()),
            PCWSTR::null(),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            0,
            0,
            560,
            640,
            None,
            None,
            None,
            None,
        ) {
            for _ in 0..2 {
                let _ = CreateWindowExW(
                    Default::default(),
                    PCWSTR(wide("EDIT").as_ptr()),
                    PCWSTR::null(),
                    child_st,
                    0,
                    0,
                    100,
                    100,
                    Some(bwin),
                    None,
                    None,
                    None,
                );
            }
            let mw = mem_kib();
            line(format!(
                "[6] Mw 再开一扇裸窗+2×EDIT 的边际 = WS {} KiB / 私有 {} KiB",
                mw.0, mw.1
            ));
            let _ = DestroyWindow(bwin);
        }
    }

    // ── 控件句柄（按 ID 取，带重试——异线程轮询可能抢在 create_controls 前） ──
    let child = |id: i32| -> HWND {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(h) = unsafe { GetDlgItem(Some(hwnd), id) } {
                if !h.is_invalid() {
                    return h;
                }
            }
            if Instant::now() > deadline {
                return HWND::default();
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    let disp = child(CI_DISPLAY);
    let edit = child(CI_EDIT);
    let btn = child(CI_BTN_SEND);
    let clear_btn = child(CI_BTN_CLEAR);
    line(format!(
        "[INFO] 控件 disp={:?} edit={:?} send={:?} clear={:?}",
        disp, edit, btn, clear_btn
    ));
    if disp.is_invalid() || edit.is_invalid() || btn.is_invalid() || clear_btn.is_invalid() {
        line("[FATAL] 子控件缺失".into());
        unsafe {
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        std::fs::write(out.join("ai_probe_report.txt"), report).ok();
        return;
    }

    // ── [4] 空输入 → 竖滚动条应隐藏 ──
    match vscroll_visible(edit) {
        Some(v) => line(format!(
            "[4] 空输入时输入框竖滚动条可见 = {v}（要求 false）"
        )),
        None => line("[4] GetScrollBarInfo 查询失败".into()),
    }

    // ── [4] 塞 60 行 → 滚动条应出现 ──
    // ★ 用 EM_REPLACESEL（打字路径）而不是 WM_SETTEXT：后者不触发 EN_CHANGE，
    //   量不到「内容变化 → 滚动条出现」的真实链路。
    // ★ 填充文本必须用 \r\n：多行 EDIT 只认 \r\n，裸 \n 不算换行（上一轮实测
    //   EM_GETLINECOUNT=3 的假信号就是这么来的）。
    let filler: String = std::iter::repeat_n("行\r\n", 60).collect();
    unsafe {
        let _ = SendMessageW(
            edit,
            0x00B1, /*EM_SETSEL*/
            Some(WPARAM(0)),
            Some(LPARAM(0)),
        );
        let u = wide(&filler);
        let _ = SendMessageW(
            edit,
            0x00C2, /*EM_REPLACESEL*/
            Some(WPARAM(0)),
            Some(LPARAM(u.as_ptr() as isize)),
        );
    }
    std::thread::sleep(Duration::from_millis(300));
    match vscroll_visible(edit) {
        Some(v) => line(format!(
            "[4] 60 行内容时输入框竖滚动条可见 = {v}（要求 true）"
        )),
        None => line("[4] GetScrollBarInfo 查询失败".into()),
    }
    // 诊断：行数与「补一刀 WM_SIZE 强制重算」后的可见性。
    unsafe {
        let lines = SendMessageW(edit, 0x00BA, Some(WPARAM(0)), Some(LPARAM(0))).0;
        line(format!("[4] 诊断：EM_GETLINECOUNT = {lines}"));
    }
    unsafe {
        // WM_SIZE 到父窗 → update_scrollbars 全量重算（绕开 EN_CHANGE 链路）。
        let _ = PostMessageW(Some(hwnd), 0x0005, WPARAM(0), LPARAM(0));
    }
    std::thread::sleep(Duration::from_millis(300));
    match vscroll_visible(edit) {
        Some(v) => line(format!("[4] 诊断：WM_SIZE 强制重算后可见 = {v}")),
        None => line("[4] 诊断：查询失败".into()),
    }

    // ── 两轮真实发送（第二轮故意带尾换行 = 旧版叠 2 空行的真实场景） ──
    let mut rendered = String::new();
    for (round, q) in [
        (1usize, "怎么在拖尾里画一个渐变圆环？".to_string()),
        (2, "边框能改成平面吗？\n".to_string()), // ★ 尾换行
        (3, "忙态悬停判据专用消息。".to_string()),
    ] {
        unsafe {
            let _ = SetWindowTextW(edit, PCWSTR(wide(&q).as_ptr()));
            let _ = PostMessageW(Some(hwnd), WM_APP_CHAT_SEND, WPARAM(0), LPARAM(0));
        }
        // ── [8] 忙态悬停（仅第 3 轮）：假服务延迟 1.5s，HTTP 等待期间发送按钮
        //    必须照常响应悬停（旧版 EnableWindow(false) 把鼠标消息全吞了，用户
        //    报的「发送没悬停」就是这段窗口里发生的）。
        if round == 3 {
            std::thread::sleep(Duration::from_millis(300)); // 让 busy 先落定
            let mut brc8 = RECT::default();
            let _ = unsafe { GetWindowRect(btn, &mut brc8) };
            let (hx, hy) = (brc8.left + 6, (brc8.top + brc8.bottom) / 2);
            // 落位校验 + 重试（物理鼠标抢占会假 FAIL，见 hover_seq 注释）。
            let mut hov = (255u8, 255u8, 255u8);
            for _ in 0..5 {
                move_cursor_to(hx, hy);
                std::thread::sleep(Duration::from_millis(400));
                let mut pt = POINT::default();
                let _ = unsafe { GetCursorPos(&mut pt) };
                if (pt.x - hx).abs() > 2 || (pt.y - hy).abs() > 2 {
                    continue;
                }
                let mut wr8 = RECT::default();
                let _ = unsafe { GetWindowRect(hwnd, &mut wr8) };
                hov = match capture_window(hwnd) {
                    Some((w2, _h2, b2)) => {
                        let (lx, ly) = (hx - wr8.left, hy - wr8.top);
                        let i = ((ly * w2 + lx) * 4) as usize;
                        (b2[i + 2], b2[i + 1], b2[i])
                    }
                    None => (0, 0, 0),
                };
                if hov.0.abs_diff(232) <= 10
                    && hov.1.abs_diff(236) <= 10
                    && hov.2.abs_diff(240) <= 10
                {
                    break;
                }
            }
            move_cursor_to(8, 8);
            std::thread::sleep(Duration::from_millis(300));
            let ok8 =
                hov.0.abs_diff(232) <= 10 && hov.1.abs_diff(236) <= 10 && hov.2.abs_diff(240) <= 10;
            line(format!(
                "[8] 忙态（HTTP 等待中）悬停发送按钮 = {hov:?}（要求浅灰 232,236,240±10）判定 = {}",
                if ok8 { "PASS" } else { "FAIL" }
            ));
        }
        let marker = format!("第 {round} 轮");
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            rendered = get_text(disp);
            if rendered.contains(&marker) || Instant::now() > deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        if !rendered.contains(&marker) {
            line(format!("[FATAL] 第 {round} 轮 30 秒内没收到回复"));
        }
        let m3 = mem_kib();
        line(format!(
            "[6] M3 第 {round} 轮往返后 = WS {} KiB / 私有 {} KiB",
            m3.0, m3.1
        ));
    }

    // ── [3] 逐行打印 + 每个块头上方的空行数判据 ──
    let norm = rendered.replace('\u{2028}', "\n");
    let lines: Vec<&str> = norm.lines().map(|l| l.trim_end_matches('\r')).collect();
    line(format!("[3] 两轮后对话区共 {} 行：", lines.len()));
    for (i, l) in lines.iter().enumerate() {
        line(format!("     {i:>2}| {l}"));
    }
    let mut ok3 = true;
    for (i, l) in lines.iter().enumerate() {
        if i == 0 || !(l.contains("──")) {
            continue;
        }
        let mut blank = 0usize;
        let mut j = i;
        while j > 0 && lines[j - 1].trim().is_empty() {
            blank += 1;
            j -= 1;
        }
        line(format!(
            "[3] 块头 {i:>2} {:?} 上方空行 = {blank}（要求 1）",
            l.trim()
        ));
        ok3 &= blank == 1;
    }
    line(format!("[3] 判定 = {}", if ok3 { "PASS" } else { "FAIL" }));

    // ── 抓全窗像素：[1] 全黑 / [2] 平面框线 / [5] 悬停变色 ──
    // ★ 先点一下对话区（只读）把焦点从输入框拿走：打字路径触发的 IME 候选浮窗
    //   叠在窗上会污染抓屏（[1] 的红色噪声与 [2] 的"框线消失"都是它）。
    {
        let mut drc0 = RECT::default();
        let _ = unsafe { GetWindowRect(disp, &mut drc0) };
        click_at((drc0.left + drc0.right) / 2, drc0.top + 60);
        std::thread::sleep(Duration::from_millis(400));
    }
    if let Some((cw, chh, bits)) = capture_window(hwnd) {
        std::fs::write(out.join("ai_probe_capture_full.raw"), &bits).ok();
        std::fs::write(out.join("ai_probe_capture_full.txt"), format!("{cw} {chh}")).ok();
        let at = |x: i32, y: i32| -> (u8, u8, u8) {
            if x < 0 || y < 0 || x >= cw || y >= chh {
                return (0, 0, 0);
            }
            let i = ((y * cw + x) * 4) as usize;
            (bits[i + 2], bits[i + 1], bits[i]) // BGRA → RGB
        };
        // [1] 红色像素近零；近黑文字像素要有量。
        // ★ 容差 <40 而非 0：抓屏是 BitBlt 屏幕区域，物理鼠标扫过任务栏弹出的
        //   工具提示（别窗口的）会从本窗底边缘探进来几十个红像素（2026-09-30
        //   实测 27 个，内容是"SQLite …sessions"字样）；回归判据要钉的是上一版
        //   整屏 697 个红像素的「AI 红字」，40 量级足够分辨。
        let (mut red, mut dark) = (0u32, 0u32);
        for i in (0..bits.len()).step_by(4) {
            let c = (bits[i + 2], bits[i + 1], bits[i]);
            if c.0 > 140 && c.1 < 90 && c.2 < 90 {
                red += 1;
            }
            if c.0.max(c.1).max(c.2) < 90 {
                dark += 1;
            }
        }
        let red_ok = red < 40;
        line(format!(
            "[1] 红色像素 = {red}（要求 <40）  近黑文字像素 = {dark}（要求 >200）判定 = {}",
            if red_ok && dark > 200 { "PASS" } else { "FAIL" }
        ));

        // [2] 对话区左边框：框线 1px 纯黑，紧邻外侧白、内侧白（3D 斜面会出现灰阶档）。
        let mut drc = RECT::default();
        let _ = unsafe { GetWindowRect(disp, &mut drc) };
        let mut wrc = RECT::default();
        let _ = unsafe { GetWindowRect(hwnd, &mut wrc) };
        let dx = drc.left - wrc.left;
        let dy = (drc.top + drc.bottom) / 2 - wrc.top;
        let ring = [at(dx - 2, dy), at(dx - 1, dy), at(dx, dy), at(dx + 1, dy)];
        line(format!(
            "[2] 对话区左边框横切（左外2 / 左外1 / 边框 / 左内1）= {ring:?}（要求 [白, 黑, 黑/字底, 内容]）"
        ));
        let flat_ok =
            ring[0] == (255, 255, 255) && ring[1].0 < 40 && ring[1].1 < 40 && ring[1].2 < 40;
        line(format!(
            "[2] 判定 = {}",
            if flat_ok { "PASS" } else { "FAIL" }
        ));

        // [5] 悬停矩阵：两个按钮 ×（静止 / 悬停 / 移出），外加「悬停中禁用→移走→
        //     启用」循环 —— 后者复现"发送按钮没有悬停效果"的真实使用路径：
        //     用户悬停着点发送 → 按钮禁用（期间不收鼠标消息）→ 回复到达重新启用。
        // ★ 一律用**真实光标**（SendInput），不能伪造 WM_MOUSEMOVE：TrackMouseEvent
        //   按物理光标判离开，假消息会被真的 WM_MOUSELEAVE 立刻撤销（假信号）。
        let mut brc = RECT::default();
        let _ = unsafe { GetWindowRect(btn, &mut brc) };
        let mut crc = RECT::default();
        let _ = unsafe { GetWindowRect(clear_btn, &mut crc) };
        let bh = brc.bottom - brc.top;
        // 贴左缘内侧采样：按钮正中心会打到字形（ClearType 彩边不是纯色）。
        let (sx, sy) = (brc.left + 6, brc.top + bh / 2);
        let (cxp, cyp) = (crc.left + 6, crc.top + bh / 2);
        let sample = |x: i32, y: i32| -> (u8, u8, u8) {
            match capture_window(hwnd) {
                Some((w2, h2, b2)) => {
                    let (lx, ly) = (x - wrc.left, y - wrc.top);
                    if lx < 0 || ly < 0 || lx >= w2 || ly >= h2 {
                        return (0, 0, 0);
                    }
                    let i = ((ly * w2 + lx) * 4) as usize;
                    (b2[i + 2], b2[i + 1], b2[i])
                }
                None => (0, 0, 0),
            }
        };
        // 一轮完整悬停序列：挪开 → 静止采样 → 悬停采样 → 挪开 → 移出采样。
        // ★ 挪到位后校验物理光标位置，不对就重来（最多 5 次）：探针与**物理鼠标**
        //   共享光标，人手一动光标就被拽走，悬停判据会假 FAIL（2026-09-30 实测
        //   目标 (1064,740) 实际落在 (1144,653)）。
        type HoverSeqOut = ((u8, u8, u8), (u8, u8, u8), (u8, u8, u8), String);
        let hover_seq = |x: i32, y: i32| -> HoverSeqOut {
            let mut diag = String::new();
            for _ in 0..5 {
                move_cursor_to(8, 8);
                std::thread::sleep(Duration::from_millis(300));
                let rest = sample(x, y);
                move_cursor_to(x, y);
                std::thread::sleep(Duration::from_millis(400));
                let mut pt = POINT::default();
                let _ = unsafe { GetCursorPos(&mut pt) };
                if (pt.x - x).abs() > 2 || (pt.y - y).abs() > 2 {
                    diag = format!(
                        "[5] 光标未落位（目标 ({x},{y}) 实际 ({},{}）)，重试",
                        pt.x, pt.y
                    );
                    continue;
                }
                let hit = unsafe { WindowFromPoint(pt) };
                let hov = sample(x, y);
                move_cursor_to(8, 8);
                std::thread::sleep(Duration::from_millis(400));
                let out = sample(x, y);
                diag = format!(
                    "[5] 目标 ({x},{y}) 落位 ok，命中 {:?}（btn={btn:?} clear={clear_btn:?}）",
                    hit.0 as isize
                );
                return (rest, hov, out, diag);
            }
            (
                (255, 255, 255),
                (255, 255, 255),
                (255, 255, 255),
                diag + "（5 次都未落位，外部鼠标抢占）",
            )
        };
        let gray = |c: (u8, u8, u8)| {
            c.0.abs_diff(232) <= 10 && c.1.abs_diff(236) <= 10 && c.2.abs_diff(240) <= 10
        };
        let white = |c: (u8, u8, u8)| c == (255, 255, 255);
        for (name, (x, y)) in [("清除", (cxp, cyp)), ("发送", (sx, sy))] {
            let (rest, hov, out, diag) = hover_seq(x, y);
            line(diag);
            let ok = white(rest) && gray(hov) && white(out);
            line(format!(
                "[5] {name}：静止 {rest:?} → 悬停 {hov:?} → 移出 {out:?} 判定 = {}",
                if ok { "PASS" } else { "FAIL" }
            ));
        }
        // 「悬停中禁用→移走→启用」循环已删：2026-09-30 起忙时**不再**禁用发送
        // 按钮（禁用 = 收不到鼠标消息 = 用户报的"发送没悬停"），忙态悬停改由
        // 下方 [8] 走真实 HTTP 等待窗口判定。
    }

    // ── [7] 重影：写入文字再清空，清空后控件内部必须一尘不染 ──
    // 上一版在 WM_CTLCOLOREDIT/STATIC 里 SetBkMode(TRANSPARENT)：EDIT 画文本行时
    // 的「逐行擦底」被跳过，删除 / 滚动后旧像素残留（用户报的「文字重叠」）。
    // 判据：SetWindowTextW("") 之后控件内部深色像素必须 = 0。右侧留 44px 给
    // 滚动条（箭头字形是深色，不算重影）。
    let dark_in = |w: HWND, inset: i32| -> Option<u32> {
        let (cw2, ch2, b2) = capture_window(hwnd)?;
        let (mut rc, mut wr) = (RECT::default(), RECT::default());
        unsafe {
            let _ = GetWindowRect(w, &mut rc);
            let _ = GetWindowRect(hwnd, &mut wr);
        }
        let x0 = rc.left - wr.left + inset;
        let y0 = rc.top - wr.top + inset;
        let x1 = rc.right - wr.left - inset - 44;
        let y1 = rc.bottom - wr.top - inset;
        let mut n = 0u32;
        for y in y0..y1 {
            for x in x0..x1 {
                if x < 0 || y < 0 || x >= cw2 || y >= ch2 {
                    continue;
                }
                let i = ((y * cw2 + x) * 4) as usize;
                let c = (b2[i + 2], b2[i + 1], b2[i]);
                if c.0.max(c.1).max(c.2) < 90 {
                    n += 1;
                }
            }
        }
        Some(n)
    };
    unsafe {
        let _ = SetWindowTextW(edit, PCWSTR(wide("重影判据 ABCabc123 测试文字").as_ptr()));
    }
    std::thread::sleep(Duration::from_millis(300));
    let d1 = dark_in(edit, 3);
    // ★ 删除走**真实打字路径**（全选 + VK_BACK 键击），不能用 SetWindowTextW("")：
    //   后者是全量失效 + 擦底，TRANSPARENT 也能蒙混过关；用户报的重叠发生在
    //   打字/退格的**部分重绘**路径上——EDIT 只重画受影响的行，靠 OPAQUE 字底
    //   矩形擦旧字形。这里模拟的就是那条路径。
    unsafe {
        let _ = SendMessageW(
            edit,
            0x00B1, /*EM_SETSEL*/
            Some(WPARAM(0)),
            Some(LPARAM(-1)),
        );
        let _ = PostMessageW(
            Some(edit),
            0x0100, /*WM_KEYDOWN*/
            WPARAM(windows::Win32::UI::Input::KeyboardAndMouse::VK_BACK.0 as usize),
            LPARAM(0),
        );
    }
    std::thread::sleep(Duration::from_millis(300));
    let d2 = dark_in(edit, 3);
    line(format!(
        "[7] 输入框：有文字时深色像素 = {d1:?}（>0），退格删光后 = {d2:?}（要求 0）"
    ));
    unsafe {
        let _ = SetWindowTextW(
            disp,
            PCWSTR(wide(&"重影滚动判据行内容\r\n".repeat(40)).as_ptr()),
        );
        // EM_SCROLL / SB_PAGEDOWN=3：滚到底（只读 EDIT 的真实滚动路径）。
        for _ in 0..10 {
            let _ = SendMessageW(disp, 0x00B5, Some(WPARAM(3)), Some(LPARAM(0)));
        }
    }
    std::thread::sleep(Duration::from_millis(300));
    let d3 = dark_in(disp, 3);
    unsafe {
        let _ = SetWindowTextW(disp, PCWSTR(wide("").as_ptr()));
    }
    std::thread::sleep(Duration::from_millis(300));
    let d4 = dark_in(disp, 3);
    line(format!(
        "[7] 对话区：滚 40 行后深色像素 = {d3:?}（>0），清空后 = {d4:?}（要求 0）"
    ));

    // ── [9] 按钮文字居中：文字包围盒到按钮四边的边距必须对称（±3px） ──
    // 内缩 2px 起量，避开 1px 描边；阈值 200 把 ClearType 彩边也算进字形。
    if let Some((cw9, ch9, b9)) = capture_window(hwnd) {
        let mut wr9 = RECT::default();
        let _ = unsafe { GetWindowRect(hwnd, &mut wr9) };
        for (name, h) in [("清除", clear_btn), ("发送", btn)] {
            let mut rc = RECT::default();
            let _ = unsafe { GetWindowRect(h, &mut rc) };
            let (bx0, by0) = (rc.left - wr9.left + 2, rc.top - wr9.top + 2);
            let (bx1, by1) = (rc.right - wr9.left - 2, rc.bottom - wr9.top - 2);
            let (mut tx0, mut ty0, mut tx1, mut ty1) = (i32::MAX, i32::MAX, -1, -1);
            for y in by0..by1 {
                for x in bx0..bx1 {
                    if x < 0 || y < 0 || x >= cw9 || y >= ch9 {
                        continue;
                    }
                    let i = ((y * cw9 + x) * 4) as usize;
                    let c = (b9[i + 2], b9[i + 1], b9[i]);
                    if c.0 < 200 || c.1 < 200 || c.2 < 200 {
                        tx0 = tx0.min(x);
                        ty0 = ty0.min(y);
                        tx1 = tx1.max(x);
                        ty1 = ty1.max(y);
                    }
                }
            }
            if tx1 < 0 {
                line(format!("[9] {name}：没量到文字像素 FAIL"));
                continue;
            }
            let (ml, mr) = (tx0 - bx0, bx1 - tx1);
            let (mt, mb) = (ty0 - by0, by1 - ty1);
            let ok9 = (ml - mr).abs() <= 3 && (mt - mb).abs() <= 3;
            line(format!(
                "[9] {name} 文字边距 左{ml}/右{mr}/上{mt}/下{mb}（要求左右差≤3 上下差≤3）判定 = {}",
                if ok9 { "PASS" } else { "FAIL" }
            ));
        }
    }

    // ── 收摊：关窗 + 记账 ──
    unsafe {
        let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
    std::thread::sleep(Duration::from_millis(400));
    let m4 = mem_kib();
    line(format!(
        "[6] M4 关窗后 = WS {} KiB / 私有 {} KiB",
        m4.0, m4.1
    ));
    std::fs::write(out.join("ai_probe_render.txt"), &rendered).ok();
    std::fs::write(out.join("ai_probe_report.txt"), &report).ok();
    let _ = t.join();
    std::process::exit(0);
}
