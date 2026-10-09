//! 剪贴板"单击/双击粘贴不出去"根因探针：焦点恢复链在提权进程里是否失效。
//!
//! # 背景（2026-10-07）
//!
//! 面板粘贴链（`clipboard_history::do_paste`）：写剪贴板 → `SetForegroundWindow(目标)`
//! → `GetGUIThreadInfo` 查目标线程焦点 → 失焦则 `AttachThreadInput + SetFocus` 恢复
//! → `SendInput Ctrl+V`。日志显示前两步全部 ok=true，但用户实测粘不出去。
//! 高嫌疑：**CurSeen 自 2026-10-05 起自我提权运行**，而这条恢复链 09-29 写成并验证时
//! 进程还是普通权限——`AttachThreadInput` / `SetFocus` 跨完整性级别可能静默失败，
//! Ctrl+V 落在无焦点窗口上 ⇒ 什么都不粘。
//!
//! # 方法
//!
//! 完整复刻 do_paste 恢复链，对真 notepad 验证，**同一用例跑两个完整性级别**：
//! 1. 本进程（cargo test，普通权限）直接跑一遍；
//! 2. 用 `runas` 把**自己**再拉起一个提权副本（环境变量 `CURSEEN_CLIP_PROBE_CHILD=1`
//!    标记；环境随 ShellExecuteW 继承，不用命令行参数——那会撞 libtest 的选项解析），
//!    副本跑同一条链并把结果写 JSON，父进程读回比对。
//!
//! # 判据
//!
//! - `restore_ok`：恢复后目标线程焦点 == 抢占前焦点（且在目标窗内）；
//! - 普通权限与提权**两者都必须 restore_ok** —— 提权红了即坐实根因。
#![cfg(windows)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use windows::core::{PCWSTR, HSTRING};
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Shell::IsUserAnAdmin;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::System::Threading::AttachThreadInput as AttachThreadInputApi;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, SetFocus as SetFocusWin, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
    KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VK_CONTROL, VK_MENU, VK_V,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, EnumWindows, GetAncestor, GetClassNameW, GetForegroundWindow,
    GetGUIThreadInfo, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, RegisterClassW,
    SetForegroundWindow, SetWindowPos, ShowWindow, WM_CLOSE, GA_ROOT, GUITHREADINFO, HWND_TOPMOST,
    SWP_SHOWWINDOW, WNDCLASSW, WS_POPUP, WS_VISIBLE,
};

const PROBE_TEXT: &str = "CURSEEN_PASTE_PROBE_20261007";

struct Out {
    admin: bool,
    fg_ok: bool,
    attach_ok: bool,
    restore_ok: bool,
    inside_after: bool,
}

/// 跑一遍完整恢复链。`panel` 模拟已抢占前台的面板窗。
unsafe fn run_chain(target: HWND, panel: HWND) -> Out {
    let admin = unsafe { IsUserAnAdmin() }.as_bool();
    let tid = GetWindowThreadProcessId(target, None);

    // ── ① 目标就位：焦点落在它内部的控件上 ──
    SetForegroundWindow(target);
    std::thread::sleep(Duration::from_millis(400));
    let fg_ok = GetForegroundWindow() == target;
    let mut info =
        GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
    let got = GetGUIThreadInfo(tid, &mut info).is_ok();
    let focus_before = info.hwndFocus;
    let focus_before_valid = got
        && !focus_before.is_invalid()
        && GetAncestor(focus_before, GA_ROOT) == target;

    // ── ② 模拟面板抢占前台（面板 show 时 SetForegroundWindow(面板)）──
    SetForegroundWindow(panel);
    SetFocusWin(Some(panel));
    std::thread::sleep(Duration::from_millis(300));

    // ── ③ do_paste 复刻：切回前台（前台锁时补 Alt tap）──
    if GetForegroundWindow() != target {
        let _ = SetForegroundWindow(target);
        if GetForegroundWindow() != target {
            alt_tap();
            let _ = SetForegroundWindow(target);
        }
    }
    let mut cur =
        GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
    let cur_focus =
        if GetGUIThreadInfo(tid, &mut cur).is_ok() { cur.hwndFocus } else { HWND::default() };
    let inside = !cur_focus.is_invalid()
        && cur_focus != target
        && GetAncestor(cur_focus, GA_ROOT) == target;

    // ── ④ do_paste 复刻：焦点恢复（★ 本次要验证的关键段）──
    let mut attach_ok = false;
    if !inside {
        let me = GetCurrentThreadId();
        let attached =
            tid != 0 && tid != me && AttachThreadInputApi(me, tid, true).as_bool();
        attach_ok = attached;
        SetFocusWin(Some(if focus_before_valid { focus_before } else { target }));
        if attached {
            AttachThreadInputApi(me, tid, false);
        }
    }
    std::thread::sleep(Duration::from_millis(250));
    let mut after =
        GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
    let got_after = GetGUIThreadInfo(tid, &mut after).is_ok();
    let focus_after = if got_after { after.hwndFocus } else { HWND::default() };
    let inside_after = !focus_after.is_invalid()
        && focus_after != target
        && GetAncestor(focus_after, GA_ROOT) == target;
    let restore_ok = inside_after && focus_after == focus_before;

    Out { admin, fg_ok, attach_ok: attach_ok || inside, restore_ok, inside_after }
}

unsafe fn alt_tap() {
    // 与 clipboard_history::send_alt_tap 同款：Alt 按下+抬起。
    let mut inputs: [INPUT; 2] = std::mem::zeroed();
    for (i, up) in [false, true].iter().enumerate() {
        inputs[i].r#type = INPUT_KEYBOARD;
        inputs[i].Anonymous.ki = KEYBDINPUT {
            wVk: VK_MENU,
            wScan: 0,
            dwFlags: if *up { KEYBD_EVENT_FLAGS(KEYEVENTF_KEYUP.0) } else { KEYBD_EVENT_FLAGS(0) },
            time: 0,
            dwExtraInfo: 0,
        };
    }
    SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
}

unsafe extern "system" fn panel_proc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    DefWindowProcW(h, msg, w, l)
}

/// 最小可前台窗口（模拟面板）。
unsafe fn make_panel() -> HWND {
    let name: Vec<u16> = "CurSeenClipProbePanel\0".encode_utf16().collect();
    let cls = WNDCLASSW {
        lpfnWndProc: Some(panel_proc),
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
        100,
        100,
        300,
        200,
        None,
        None,
        Some(cls.hInstance.into()),
        None,
    )
    .expect("probe panel window");
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

unsafe fn find_notepad_main(pid: u32) -> Option<HWND> {
    let mut found: Option<HWND> = None;
    let ctx = &mut (pid, &mut found);
    let _ = EnumWindows(Some(enum_cb), LPARAM(ctx as *mut _ as isize));
    found
}

unsafe extern "system" fn enum_cb(h: HWND, l: LPARAM) -> BOOL {
    let ctx = &mut *(l.0 as *mut (u32, *mut Option<HWND>));
    let (pid, out) = ctx;
    CNT.with(|c| c.set(c.get() + 1));
    let mut wpid = 0u32;
    GetWindowThreadProcessId(h, Some(&mut wpid));
    if wpid == *pid && IsWindowVisible(h).as_bool() {
        let mut buf = [0u16; 32];
        let n = GetClassNameW(h, &mut buf);
        let cls = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
        // Win11 新版记事本主窗类名仍是 "Notepad"，但稳妥起见：可见顶层窗即收，
        // 类名打进诊断输出（每个 pid 只有一个可见主窗）。
        println!("候选窗 pid={pid} 类={cls:?}");
        **out = Some(h);
        return BOOL(0);
    }
    BOOL(1)
}

thread_local! {
    static CNT: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

fn notepad_pids() -> Vec<u32> {
    std::process::Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq notepad.exe", "/FO", "CSV", "/NH"])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter_map(|l| l.split(',').nth(1)?.trim_matches('"').parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

fn out_path(child: bool) -> PathBuf {
    std::env::temp_dir().join(if child {
        "curseen_clip_probe_elevated.json"
    } else {
        "curseen_clip_probe_normal.json"
    })
}

fn write_out(p: &PathBuf, o: &Out) {
    let _ = std::fs::write(
        p,
        format!(
            r#"{{"admin":{},"fg_ok":{},"attach_ok":{},"restore_ok":{},"inside_after":{}}}"#,
            o.admin, o.fg_ok, o.attach_ok, o.restore_ok, o.inside_after
        ),
    );
}

#[test]
#[ignore = "真起 notepad 并抢占前台，须真实桌面会话；经 build/probe.py 按需运行"]
fn focus_restore_matches_across_integrity_levels() {
    unsafe {
        let child_mode = std::env::var("CURSEEN_CLIP_PROBE_CHILD").as_deref() == Ok("1");
        if child_mode {
            // 启动即落标记：区分"没拉起来"和"拉起了但中途 panic"。
            let _ = std::fs::write(std::env::temp_dir().join("curseen_clip_probe_child_started"), "1");
        }

        // ── 起 notepad 当目标 ──
        // 优先"新出现的 pid"；Win11 记事本会把新标签复用进已有进程，
        // 那种情况下直接用它的现存主窗（探针结束后 WM_CLOSE 关掉的是我们 spawn 的
        // 新实例，用户已有实例不受影响——复用场景下关的是共享窗口，可接受）。
        let before = notepad_pids();
        println!("启动前 notepad pids: {before:?}");
        let _ = std::process::Command::new("notepad.exe").spawn().expect("notepad 启动失败");
        let target = {
            let dl = Instant::now();
            loop {
                std::thread::sleep(Duration::from_millis(150));
                if dl.elapsed() > Duration::from_secs(6) {
                    panic!("6s 内未找到 notepad 主窗");
                }
                let pids = notepad_pids();
                let n = CNT.with(|c| c.get());
                println!("轮询 pids: {pids:?} enum总数={n}");
                let mut hit: Option<HWND> = None;
                for pid in pids {
                    let is_new = !before.contains(&pid);
                    if let Some(h) = find_notepad_main(pid) {
                        hit = Some(h);
                        if is_new {
                            break;
                        }
                    }
                }
                if let Some(h) = hit {
                    break h;
                }
            }
        };
        SetForegroundWindow(target);
        std::thread::sleep(Duration::from_millis(300));

        let panel = make_panel();
        let out = run_chain(target, panel);

        // 收尾：关 notepad（新开的，无未保存内容）。
        let _ = PostMessageW(Some(target), WM_CLOSE, WPARAM(0), LPARAM(0));

        if child_mode {
            println!("子进程模式：admin={} fg_ok={} attach_ok={} restore_ok={}",
                out.admin, out.fg_ok, out.attach_ok, out.restore_ok);
            write_out(&out_path(true), &out);
            return;
        }

        write_out(&out_path(false), &out);
        println!(
            "普通权限：admin={} fg_ok={} attach_ok={} restore_ok={} inside_after={}",
            out.admin, out.fg_ok, out.attach_ok, out.restore_ok, out.inside_after
        );

        // ── 拉提权副本跑同一用例 ──
        std::env::set_var("CURSEEN_CLIP_PROBE_CHILD", "1");
        let elevated_path = out_path(true);
        let _ = std::fs::remove_file(&elevated_path);
        let exe = std::env::current_exe().unwrap();
        let verb = HSTRING::from("runas");
        let file = HSTRING::from(exe.as_os_str());
        let args = HSTRING::from(
            "--exact focus_restore_matches_across_integrity_levels --ignored --nocapture",
        );
        let r = ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR(args.as_ptr()),
            None,
            windows::Win32::UI::WindowsAndMessaging::SHOW_WINDOW_CMD(0),
        );
        if (r.0 as isize) <= 32 {
            println!("提权副本拉起失败（ShellExecuteW={:?}），只验普通权限", r.0);
            assert!(
                out.restore_ok,
                "普通权限下焦点恢复即失败（attach_ok={} inside_after={}）——根因不在提权",
                out.attach_ok, out.inside_after
            );
            return;
        }
        let mut raw = None;
        let dl = Instant::now();
        while dl.elapsed() < Duration::from_secs(90) {
            if let Ok(s) = std::fs::read_to_string(&elevated_path) {
                raw = Some(s);
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        let elev = raw.expect("60s 内未等到提权副本结果（可能 UAC 被拦）");
        println!("提权副本：{elev}");
        let elev_restore = elev.contains("\"restore_ok\":true");

        assert!(out.fg_ok, "普通权限：目标没能成为前台");
        assert!(
            out.restore_ok,
            "普通权限下焦点恢复即失败（attach_ok={} inside_after={}）——根因不在提权",
            out.attach_ok, out.inside_after
        );
        assert!(
            elev_restore,
            "★ 根因坐实：提权进程里焦点恢复失败（普通权限正常）——do_paste 的 \
             AttachThreadInput/SetFocus 跨完整性级别静默失效，Ctrl+V 落在无焦点窗口"
        );
    }
}
