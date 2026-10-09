//! 管理员权限：判断与自提权重启。
//!
//! # 为什么"通常不需要管理员"
//!
//! `WH_MOUSE_LL` / `WH_KEYBOARD_LL` 这两个**低级**钩子在普通权限下就能装、也能收到
//! 事件，所以日常使用并不需要提权。唯一的例外是前台窗口属于**更高完整性级别**的进程
//! （即用户"以管理员身份运行"的程序）：UIPI 会拦住低完整性进程，那段时间的鼠标/按键
//! 事件收不到，表现为"特效和击键回显在管理员窗口上哑火"。
//!
//! # 与 Python 1.0.0 的关系
//!
//! Python 的 `__main__.py:5-25` 是**启动即无条件 `runas` 重启**，代价是每次开机自启都弹
//! 一次 UAC；而且用户点"否"之后它 `sys.exit(0)`，程序直接不启动。
//!
//! 本模块曾按 plan3.md §7 改成"默认不提权、传 `--elevate` 才提权"，但 2.0 实测推翻了
//! 这个取舍：任务管理器等**提权进程**在前台时，普通权限进程的低级钩子被 UIPI 拦截，
//! 拖尾冻结消失（用户报告"开任务管理器特效就没了"；z 序探针证明与窗口遮挡无关——
//! 覆盖层一直在最上层，只是钩子瞎了）。原版从不复现此问题，正因为 1.0.0 每次启动都
//! 提权。于是 **main.rs 恢复"启动即提权"**，仅保留一处健壮性差异：
//!
//! - UAC 被拒时**继续以普通权限运行**，而不是像 Python 那样直接退出；
//! - `--elevate` 开关保留，但已与默认路径重合（幂等）。

use std::ffi::c_void;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, KillTimer,
    RegisterClassExW, SetTimer, TranslateMessage, UnregisterClassW, MSG, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_TIMER, WNDCLASSEXW,
};

/// 命令行开关：显式要求以管理员身份重启自身。
pub const ELEVATE_FLAG: &str = "--elevate";

/// 是否已以管理员（提升后）权限运行。
///
/// 走 `GetTokenInformation(TokenElevation)` 而不是已废弃的 `IsUserAnAdmin()`：前者是
/// 文档推荐的判定方式，且能正确区分"账户属管理员组但未提升"的情况。
pub fn is_elevated() -> bool {
    // SAFETY: 只读当前进程令牌；`token` 在任何提前返回路径上都会被关闭（见下方 `closed` 标记）。
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut info = TOKEN_ELEVATION::default();
        let mut returned = 0u32;
        let queried = GetTokenInformation(
            token,
            TokenElevation,
            Some(std::ptr::from_mut(&mut info).cast::<c_void>()),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
        .is_ok();
        let _ = CloseHandle(token);
        queried && info.TokenIsElevated != 0
    }
}

/// 用 `runas` 重启自身并返回是否**成功发起**（用户点了"否"→ `false`）。
///
/// 成功后调用方应当立即退出，由提权副本接管。
pub fn relaunch_elevated() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let exe_w = wide(&exe.to_string_lossy());
    // 传参时去掉 `--elevate`：提权副本里 `is_elevated()` 已为真，留着只是噪音。
    let rest: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| a != ELEVATE_FLAG)
        .collect();
    let params_w = wide(&join_args(&rest));

    // SAFETY: 两个宽字符串在调用期间都存活（局部变量），`PCWSTR` 只是借用其首地址。
    // `ShellExecuteW` 返回值 ≤ 32 表示失败（Win32 文档约定），据此判断用户是否拒绝了 UAC。
    unsafe {
        let r = ShellExecuteW(
            None,
            w!("runas"),
            PCWSTR(exe_w.as_ptr()),
            PCWSTR(params_w.as_ptr()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
        // `HINSTANCE` 在 windows 0.62 里是 `*mut c_void` 的新类型；文档约定返回值 ≤ 32 即失败。
        r.0 as usize > 32
    }
}

/// 提权重启前把"应用启动握手"走完（2026-10-07 首开面板悬停转圈的修复）。
///
/// # 背景（用户实测复现的完整证据链）
///
/// explorer 双击启动本进程后，系统进入"等待应用启动"状态——在该进程的窗口上悬停
/// 会显示启动转圈，直到进程**建窗并进入消息等待（输入空闲）**才解除。旧流程是
/// "秒退"：未提权 → `ShellExecuteW(runas)` → 立刻 return——原始进程既没建窗也没
/// 泵过消息，这个等待被悬空挂起并缠上提权副本，表现为：**每次启动后首次调出剪贴板
/// 面板，鼠标悬停在面板上一直转圈（实测 30s+），点一下标题栏（真实点击送达、启动
/// 等待随之完成）才消失**；不提权的剪贴板工具没有这个问题。
///
/// # 修法
///
/// 提权副本发起后，本进程不立刻退：建一个**隐形**窗口把消息泵跑 1.5 秒（线程处于
/// "队列空 + 等待输入" = 输入空闲），让系统把"应用已启动"标记为完成，再退出。
/// 泵走 `WM_TIMER`（1.5s 到点 → `GetMessageW` 返回那条 timer 消息 → 跳出）。
pub fn settle_startup_handshake() {
    unsafe {
        let class = w!("CurSeenStartupSettle");
        let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(settle_wndproc),
            hInstance: hinst,
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        // 无 WS_VISIBLE：全程隐形，用户无感。
        if let Ok(hwnd) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!(""),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinst),
            None,
        ) {
            const SETTLE_TIMER: usize = 1;
            let _ = SetTimer(Some(hwnd), SETTLE_TIMER, 1500, None);
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
                if msg.message == WM_TIMER {
                    break;
                }
            }
            let _ = KillTimer(Some(hwnd), SETTLE_TIMER);
            let _ = DestroyWindow(hwnd);
        }
        let _ = UnregisterClassW(class, Some(hinst));
    }
}

/// 命令行里是否带 `--elevate`。
pub fn has_elevate_flag(args: &[String]) -> bool {
    args.iter().skip(1).any(|a| a == ELEVATE_FLAG)
}

/// 启动握手窗口的窗口过程：全转 `DefWindowProcW`（windows 0.62 的 `DefWindowProcW`
/// 不是 `extern "system"`，不能直接塞进 `WNDCLASSEXW.lpfnWndProc`，包一层）。
unsafe extern "system" fn settle_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// 把参数拼成 `CreateProcess`/`ShellExecute` 风格的单串（含空格的参数加引号）。
fn join_args(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            if a.contains(' ') && !a.starts_with('"') {
                format!("\"{a}\"")
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// UTF-16 + NUL。
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn elevate_flag_recognised_only_after_program_name() {
        assert!(has_elevate_flag(&v(&["curseen.exe", "--elevate"])));
        assert!(!has_elevate_flag(&v(&["curseen.exe"])));
        // 程序名本身叫 --elevate（不可能，但别把 argv[0] 当开关）
        assert!(!has_elevate_flag(&v(&["--elevate"])));
        assert!(!has_elevate_flag(&v(&["curseen.exe", "--Elevate"])));
    }

    #[test]
    fn join_args_quotes_only_when_needed() {
        assert_eq!(join_args(&[]), "");
        assert_eq!(join_args(&v(&["--elevate"])), "--elevate");
        assert_eq!(
            join_args(&v(&["--path", "C:\\a b\\c.txt"])),
            "--path \"C:\\a b\\c.txt\""
        );
        // 已经是引号包着的别再包一层
        assert_eq!(join_args(&v(&["\"x y\""])), "\"x y\"");
    }

    #[test]
    fn wide_is_utf16_with_terminator() {
        let w = wide("A中");
        assert_eq!(w, vec![0x41, 0x4E2D, 0]);
    }
}
