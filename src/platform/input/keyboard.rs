//! WH_KEYBOARD_LL 的回调处理。
//!
//! ⚠️ 回调运行在系统输入路径上：任何耗时操作都会拖慢全系统的键盘响应。
//! 硬性要求：本函数只做「读结构体 → 归一化 → tx.send」，不做日志写盘、不做加锁等待、
//! 不做字符串格式化。

use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU8};

use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CAPITAL};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN,
    WM_SYSKEYUP,
};

use super::hook::HOOK_TX;
use super::vk::{self, vk_const};
use crate::core::events::{InputEvent, KeyCode, Modifiers};

/// 修饰键状态缓存：回调内不调 GetKeyState（它要进内核），用缓存值。
/// 唯一例外是 CapsLock 状态查询（见下方 `keyboard_proc` 内注释）。
static MODS: AtomicU8 = AtomicU8::new(0);

/// 修饰键位掩码
const MOD_CTRL: u8 = 1;
const MOD_ALT: u8 = 2;
const MOD_SHIFT: u8 = 4;
const MOD_WIN: u8 = 8;

/// 上一次按下的键（用于判定长按重复）
static LAST_DOWN: AtomicU16 = AtomicU16::new(0);

/// 是否收到了上次 LAST_DOWN 对应的 KeyUp
static LAST_UP: AtomicBool = AtomicBool::new(true);

/// PrtSc 特殊处理：系统只发 Up 不发 Down，需要合成 Down
static PRTSC_PENDING: AtomicBool = AtomicBool::new(false);

/// Ctrl 是否按着（鼠标钩子判定缩放滚轮用；读本线程维护的缓存，零系统调用）。
pub fn ctrl_held() -> bool {
    MODS.load(std::sync::atomic::Ordering::Relaxed) & MOD_CTRL != 0
}

/// Shift 是否按着（标注的 Shift 约束绘制用；读本线程维护的缓存，零系统调用）。
pub fn shift_held() -> bool {
    MODS.load(std::sync::atomic::Ordering::Relaxed) & MOD_SHIFT != 0
}

/// Win 是否按着（同上，与 `ctrl_held` 配对成 Ctrl+Win+滚轮 缩放手势）。
pub fn win_held() -> bool {
    MODS.load(std::sync::atomic::Ordering::Relaxed) & MOD_WIN != 0
}

/// 获取当前修饰键状态
fn current_mods() -> Modifiers {
    let m = MODS.load(std::sync::atomic::Ordering::Relaxed);
    Modifiers {
        ctrl: m & MOD_CTRL != 0,
        alt: m & MOD_ALT != 0,
        shift: m & MOD_SHIFT != 0,
        win: m & MOD_WIN != 0,
    }
}

/// 键盘钩子回调。
///
/// # Safety
/// 必须作为 WH_KEYBOARD_LL 的回调函数传入 SetWindowsHookExW。
pub unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
    let flags = kb.flags;
    let extended = (flags.0 & LLKHF_EXTENDED.0) != 0;
    let vk = kb.vkCode as u16;
    let scan = kb.scanCode as u16;
    let time_ms = kb.time as u64;
    let msg = wparam.0 as u32;

    let down = matches!(msg, WM_KEYDOWN | WM_SYSKEYDOWN);
    let up = matches!(msg, WM_KEYUP | WM_SYSKEYUP);

    // 更新修饰键缓存
    match vk {
        vk_const::LSHIFT | vk_const::RSHIFT | vk_const::SHIFT => {
            if down {
                MODS.fetch_or(MOD_SHIFT, std::sync::atomic::Ordering::Relaxed);
            } else if up {
                MODS.fetch_and(!MOD_SHIFT, std::sync::atomic::Ordering::Relaxed);
            }
        }
        vk_const::LCONTROL | vk_const::RCONTROL | vk_const::CONTROL => {
            if down {
                MODS.fetch_or(MOD_CTRL, std::sync::atomic::Ordering::Relaxed);
            } else if up {
                MODS.fetch_and(!MOD_CTRL, std::sync::atomic::Ordering::Relaxed);
            }
        }
        vk_const::LMENU | vk_const::RMENU | vk_const::MENU => {
            if down {
                MODS.fetch_or(MOD_ALT, std::sync::atomic::Ordering::Relaxed);
            } else if up {
                MODS.fetch_and(!MOD_ALT, std::sync::atomic::Ordering::Relaxed);
            }
        }
        vk_const::LWIN | vk_const::RWIN => {
            if down {
                MODS.fetch_or(MOD_WIN, std::sync::atomic::Ordering::Relaxed);
            } else if up {
                MODS.fetch_and(!MOD_WIN, std::sync::atomic::Ordering::Relaxed);
            }
        }
        _ => {}
    }

    // CapsLock 状态（仅用于 Shift 修正，不作为 modifier）
    let caps = GetKeyState(VK_CAPITAL.0 as i32) & 1 != 0;
    let shift = MODS.load(std::sync::atomic::Ordering::Relaxed) & MOD_SHIFT != 0;

    // 映射到 KeyCode
    let key = vk::from_vk(vk, scan, extended, shift, caps);

    // 长按重复判定
    let repeated = if down {
        let last = LAST_DOWN.load(std::sync::atomic::Ordering::Relaxed);
        let last_up = LAST_UP.load(std::sync::atomic::Ordering::Relaxed);
        !last_up && last == vk
    } else {
        false
    };

    // PrtSc 特殊处理：系统只发 Up，合成一个 Down
    if vk == vk_const::SNAPSHOT {
        if up && !PRTSC_PENDING.swap(true, std::sync::atomic::Ordering::Relaxed) {
            let mods = current_mods();
            if let Some(tx) = HOOK_TX.get() {
                let _ = tx.send(InputEvent::KeyDown {
                    key: KeyCode::PrtSc,
                    mods,
                    repeated: false,
                    time_ms,
                });
            }
        }
        if up {
            if PRTSC_PENDING.swap(false, std::sync::atomic::Ordering::Relaxed) {
                let mods = current_mods();
                if let Some(tx) = HOOK_TX.get() {
                    let _ = tx.send(InputEvent::KeyUp {
                        key: KeyCode::PrtSc,
                        mods,
                        time_ms,
                    });
                }
            }
            return CallNextHookEx(None, code, wparam, lparam);
        }
    }

    if down {
        LAST_DOWN.store(vk, std::sync::atomic::Ordering::Relaxed);
        LAST_UP.store(false, std::sync::atomic::Ordering::Relaxed);
        let mods = current_mods();
        if let Some(tx) = HOOK_TX.get() {
            let _ = tx.send(InputEvent::KeyDown {
                key,
                mods,
                repeated,
                time_ms,
            });
        }
    } else if up {
        LAST_UP.store(true, std::sync::atomic::Ordering::Relaxed);
        let mods = current_mods();
        if let Some(tx) = HOOK_TX.get() {
            let _ = tx.send(InputEvent::KeyUp { key, mods, time_ms });
        }
    }

    CallNextHookEx(None, code, wparam, lparam)
}
