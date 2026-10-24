//! 钩子线程的生命周期管理。
//!
//! 线程模型：
//!   · 一个专门的 std::thread，启动后立即 SetThreadPriority(TIME_CRITICAL)
//!   · 装 WH_KEYBOARD_LL（必须：全局快捷键的唯一入口）
//!   · WH_MOUSE_LL **按需装卸**：只有确实有鼠标消费者时才装，见
//!     [`set_mouse_hook_enabled`]。低级鼠标钩子即使什么都不做，系统也会把每一次
//!     鼠标事件派发进来；全关时装着它，就是用户看到的"我没开鼠标功能它却在跑"。
//!   · 跑标准 GetMessageW 循环（低钩子必须在装载线程泵消息才会被调用）
//!   · 收到 WM_APP+3（停止信号）→ UnhookWindowsHookEx → PostQuitMessage → 线程结束

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::OnceLock;
use std::thread::{self, JoinHandle};

use windows::core::{Error, Result};
use windows::Win32::Foundation::{HMODULE, LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, PostQuitMessage, PostThreadMessageW, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, HHOOK, MSG, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_APP,
    WM_QUIT,
};

use super::keyboard::keyboard_proc;
use super::mouse::mouse_proc;
use crate::core::events::InputEvent;

/// 全局 Sender：钩子回调通过它把事件送到 UI 线程。
pub(crate) static HOOK_TX: OnceLock<mpsc::Sender<InputEvent>> = OnceLock::new();

/// 键盘钩子句柄
static KB_HOOK: AtomicIsize = AtomicIsize::new(0);
/// 鼠标钩子句柄（0 = 未安装）
static MOUSE_HOOK: AtomicIsize = AtomicIsize::new(0);

/// 钩子线程 id（0 = 尚未启动 / 已退出）。`set_mouse_hook_enabled` 靠它投递装卸指令。
static HOOK_TID: AtomicU32 = AtomicU32::new(0);
/// 鼠标钩子**期望**状态。UI 线程每帧对账写这里，线程启动时据此补装。
static MOUSE_HOOK_WANTED: AtomicBool = AtomicBool::new(false);

/// 停止信号消息
const WM_STOP_HOOKS: u32 = WM_APP + 3;
/// 装卸鼠标钩子（WPARAM 1 = 装 / 0 = 卸）。必须在钩子线程里执行。
const WM_SET_MOUSE_HOOK: u32 = WM_APP + 4;

/// 按需启用/停用鼠标低级钩子（幂等；状态没变不碰任何系统 API）。
///
/// 调用点在 [`crate::runtime::Runtime::sync_input_gate`]（构造时 + 每帧 + 250ms 对账）。
/// 钩子线程尚未启动时只记下期望值，线程启动时会自己补装。
pub fn set_mouse_hook_enabled(on: bool) {
    if MOUSE_HOOK_WANTED.swap(on, Ordering::Relaxed) == on {
        return;
    }
    let tid = HOOK_TID.load(Ordering::Relaxed);
    if tid == 0 {
        return;
    }
    unsafe {
        let _ = PostThreadMessageW(tid, WM_SET_MOUSE_HOOK, WPARAM(usize::from(on)), LPARAM(0));
    }
}

/// 当前**期望**的鼠标钩子状态（不读句柄：钩子线程异步装卸，句柄会滞后一帧）。
/// 门禁测试用；生产路径只写不读。
pub fn mouse_hook_wanted() -> bool {
    MOUSE_HOOK_WANTED.load(Ordering::Relaxed)
}

/// 在钩子线程内安装鼠标钩子（已装则跳过）。失败只记日志：键盘功能不受影响。
fn install_mouse_hook(hmod: HMODULE) {
    if MOUSE_HOOK.load(Ordering::Relaxed) != 0 {
        return;
    }
    match unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), Some(hmod.into()), 0) } {
        Ok(h) => {
            MOUSE_HOOK.store(h.0 as isize, Ordering::Relaxed);
            log::info!("鼠标钩子已安装（按需）");
        }
        Err(e) => log::error!("鼠标钩子安装失败，鼠标类特效本次不可用: {e}"),
    }
}

/// 在钩子线程内卸载鼠标钩子（未装则跳过）。
fn uninstall_mouse_hook() {
    let h = MOUSE_HOOK.swap(0, Ordering::Relaxed);
    if h == 0 {
        return;
    }
    unsafe {
        let _ = UnhookWindowsHookEx(HHOOK(h as _));
    }
    log::info!("鼠标钩子已卸载（无鼠标功能在用）");
}

/// 钩子线程的管理结构。
pub struct InputHook {
    thread: Option<JoinHandle<()>>,
    thread_id: u32,
}

impl InputHook {
    /// 启动捕获。`tx` 用于把事件送往 UI 线程。
    pub fn start(tx: mpsc::Sender<InputEvent>) -> Result<Self> {
        let _ = HOOK_TX.set(tx);

        let (ready_tx, ready_rx) = mpsc::channel::<Result<u32>>();

        let handle = thread::spawn(move || {
            unsafe {
                let h = GetCurrentThread();
                let _ = SetThreadPriority(h, THREAD_PRIORITY_TIME_CRITICAL);
            }

            let hmod = unsafe { GetModuleHandleW(None) };
            let hmod = match hmod {
                Ok(h) => h,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };

            let kb_hook = unsafe {
                SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), Some(hmod.into()), 0)
            };
            let kb_hook = match kb_hook {
                Ok(h) => h,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };

            KB_HOOK.store(kb_hook.0 as isize, Ordering::Relaxed);

            let tid = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
            HOOK_TID.store(tid, Ordering::Relaxed);
            // 启动前 UI 线程已经对过账（`Runtime::new` 会调 `sync_input_gate`）：
            // 有鼠标功能在用就先把钩子装上，别等到下一帧。
            if MOUSE_HOOK_WANTED.load(Ordering::Relaxed) {
                install_mouse_hook(hmod);
            }

            let _ = ready_tx.send(Ok(tid));

            let mut msg = MSG::default();
            while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
                if msg.message == WM_QUIT {
                    break;
                }
                if msg.message == WM_STOP_HOOKS {
                    let kb = KB_HOOK.swap(0, Ordering::Relaxed);
                    unsafe {
                        if kb != 0 {
                            let _ = UnhookWindowsHookEx(HHOOK(kb as _));
                        }
                        uninstall_mouse_hook();
                        PostQuitMessage(0);
                    }
                    HOOK_TID.store(0, Ordering::Relaxed);
                    continue;
                }
                if msg.message == WM_SET_MOUSE_HOOK {
                    if msg.wParam.0 != 0 {
                        install_mouse_hook(hmod);
                    } else {
                        uninstall_mouse_hook();
                    }
                    continue;
                }
                unsafe {
                    let _ = TranslateMessage(&msg);
                    let _ = DispatchMessageW(&msg);
                }
            }
        });

        match ready_rx.recv() {
            Ok(Ok(tid)) => Ok(Self {
                thread: Some(handle),
                thread_id: tid,
            }),
            Ok(Err(e)) => {
                let _ = handle.join();
                Err(e)
            }
            Err(_) => {
                // 发送端已丢弃（线程在发就绪信号前就退出了）：必须 join，否则钩子线程
                // 变成孤儿一直跑（即便本机再也收不到它的消息，低层钩子仍驻留）。
                let _ = handle.join();
                Err(Error::empty())
            }
        }
    }

    /// 停止捕获并 join 线程（幂等）。
    pub fn stop(&mut self) {
        if let Some(th) = self.thread.take() {
            unsafe {
                let _ = PostThreadMessageW(
                    self.thread_id,
                    WM_STOP_HOOKS,
                    windows::Win32::Foundation::WPARAM(0),
                    LPARAM(0),
                );
            }
            let _ = th.join();
        }
    }
}

impl Drop for InputHook {
    fn drop(&mut self) {
        self.stop();
    }
}
