//! 钩子线程的生命周期管理。
//!
//! 线程模型：
//!   · 一个专门的 std::thread，启动后立即 SetThreadPriority(TIME_CRITICAL)
//!   · 装 WH_KEYBOARD_LL 与 WH_MOUSE_LL
//!   · 跑标准 GetMessageW 循环（低钩子必须在装载线程泵消息才会被调用）
//!   · 收到 WM_APP+3（停止信号）→ UnhookWindowsHookEx × 2 → PostQuitMessage → 线程结束

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::mpsc;
use std::sync::OnceLock;
use std::thread::{self, JoinHandle};

use windows::core::{Error, Result};
use windows::Win32::Foundation::LPARAM;
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
/// 鼠标钩子句柄
static MOUSE_HOOK: AtomicIsize = AtomicIsize::new(0);

/// 停止信号消息
const WM_STOP_HOOKS: u32 = WM_APP + 3;

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

            let mouse_hook =
                unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), Some(hmod.into()), 0) };
            let mouse_hook = match mouse_hook {
                Ok(h) => h,
                Err(e) => {
                    unsafe {
                        let _ = UnhookWindowsHookEx(kb_hook);
                    }
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };

            KB_HOOK.store(kb_hook.0 as isize, Ordering::Relaxed);
            MOUSE_HOOK.store(mouse_hook.0 as isize, Ordering::Relaxed);

            let tid = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
            let _ = ready_tx.send(Ok(tid));

            let mut msg = MSG::default();
            while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
                if msg.message == WM_QUIT {
                    break;
                }
                if msg.message == WM_STOP_HOOKS {
                    let kb = KB_HOOK.swap(0, Ordering::Relaxed);
                    let mo = MOUSE_HOOK.swap(0, Ordering::Relaxed);
                    unsafe {
                        if kb != 0 {
                            let _ = UnhookWindowsHookEx(HHOOK(kb as _));
                        }
                        if mo != 0 {
                            let _ = UnhookWindowsHookEx(HHOOK(mo as _));
                        }
                        PostQuitMessage(0);
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
