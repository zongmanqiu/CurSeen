//! `SendInput` 按键注入：鼠标点中虚拟键盘的键帽时，把一次真实按键送给系统。
//!
//! # 为什么用 `SendInput` 而不是直接 `PostMessage`
//!
//! 虚拟键盘的目标是"点一下等于真按一下"——按键要能被**当前前台应用**当成真实输入，
//! 该触发快捷键就触发快捷键、该进输入框就进输入框。`PostMessage` 只把消息投给某个
//! 窗口，绕过了系统输入队列：前台是谁都不知道，也进不了那些用 `GetAsyncKeyState`
//! 轮询状态的游戏。
//!
//! # 注入事件会回到我们自己的钩子
//!
//! `SendInput` 产生的按键带 `LLKHF_INJECTED`，且**会**走 `WH_KEYBOARD_LL`——
//! 也就是会回到 `platform/input/keyboard.rs`，变成一条普通的 `InputEvent::KeyDown`。
//! 这**正是想要的**：虚拟键盘的高亮、击键回显、快捷键响应全都自动跟上，不必为
//! "鼠标点的键"另开一条高亮通路。
//!
//! 会不会形成回路（注入 → 钩子 → 再注入）？不会：注入的触发点在**鼠标**上，
//! 而键盘钩子只做事件分发，不触发注入。
//!
//! # 焦点
//!
//! 覆盖层是鼠标穿透的，`SendInput` 的按键落给**当前前台窗口**——用户看着哪个应用、
//! 点虚拟键盘，字就进哪个应用。这一点不需要我们干预。

use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, VIRTUAL_KEY,
};

use super::vk;
use crate::core::events::KeyCode;

/// 发一次按键（按下或抬起）。
///
/// 返回是否真的送进系统：键码没有映射、或 `SendInput` 被系统拒绝（极少见，通常是被
/// 更高权限的前台窗口挡下 —— UIPI）都返回 `false`。调用方据此决定"要不要认为这颗键
/// 是按下了"，避免留下一个永远等不到抬起的按下态。
pub fn send_key(code: KeyCode, down: bool) -> bool {
    let Some((vk_code, extended)) = vk::to_vk_ext(code) else {
        log::warn!("键 {code:?} 没有注入映射，忽略这次点击");
        return false;
    };

    let mut flags = KEYBD_EVENT_FLAGS(0);
    if !down {
        flags |= KEYEVENTF_KEYUP;
    }
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }

    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk_code),
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };

    let sent = unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
    if sent != 1 {
        log::warn!(
            "SendInput 注入失败（键 {code:?} / vk=0x{vk_code:02X} / down={down}）——\
             前台窗口可能比我们权限高"
        );
        return false;
    }
    log::debug!("注入按键 {code:?} vk=0x{vk_code:02X} extended={extended} down={down}");
    true
}
