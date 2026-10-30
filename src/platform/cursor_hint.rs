//! 指针当前是否停在**有交互提示**的元素上（I 形 / 手形 / 缩放 / 禁止 / 帮助 / 十字）。
//!
//! # 为什么不是「指针下有按钮」
//!
//! 桌面环境没有 DOM 的 `elementFromPoint`。Windows 桌面按钮/链接的光标仍是**箭头**，
//! 只有输入框（I 形）、超链接（手形）、窗口边框（缩放）等会换系统光标。因此本模块
//! 能给出的诚实语义只有"系统正在提示这里可交互"，不是"这里一定有按钮"——脚本的
//! `pointerAnim` 分支据此放大，不会假装知道更多。
//!
//! # 为什么按句柄白名单而不是「不等于箭头」
//!
//! 很多程序自带自定义箭头光标（浏览器、设计软件），「不等于箭头」会把整块界面判成
//! 悬停。只认系统标准交互光标句柄，误报明显更少；代价是自定义手形也不会被识别。
//! 句柄表进程内缓存一次：`LoadCursorW` 返回的是系统共享句柄，不需要释放。
//!
//! `hide_cursor` 只替换常规箭头（`OCR_NORMAL`），I 形/手形/缩放仍是系统原样，因此
//! 隐藏态与显示态用同一套判据。

use std::sync::OnceLock;

use windows::core::PCWSTR;
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorInfo, LoadCursorW, CURSORINFO, CURSOR_SHOWING, IDC_CROSS, IDC_HAND, IDC_HELP,
    IDC_IBEAM, IDC_NO, IDC_SIZEALL, IDC_SIZENESW, IDC_SIZENS, IDC_SIZENWSE, IDC_SIZEWE,
};

#[cfg(test)]
thread_local! {
    /// 测试用强制值：脚本门禁要同时跑「悬停开 / 关」两条分支，而真实光标位置不可控。
    static FORCED: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// 视为「交互提示」的系统光标 id。
const INTERACTIVE_IDS: [PCWSTR; 10] = [
    IDC_IBEAM,
    IDC_HAND,
    IDC_SIZEWE,
    IDC_SIZENS,
    IDC_SIZENWSE,
    IDC_SIZENESW,
    IDC_SIZEALL,
    IDC_CROSS,
    IDC_NO,
    IDC_HELP,
];

/// 标准交互光标的当前句柄（进程内缓存）。
fn interactive_handles() -> &'static [isize] {
    static HANDLES: OnceLock<Vec<isize>> = OnceLock::new();
    HANDLES.get_or_init(|| {
        INTERACTIVE_IDS
            .iter()
            .filter_map(|id| {
                // SAFETY: 只查询系统共享光标句柄，无前置条件，返回值不归调用方释放。
                let h = unsafe { LoadCursorW(None, *id) }.ok()?;
                Some(h.0 as isize)
            })
            .collect()
    })
}

/// [`is_interactive`] 的纯逻辑部分：句柄非空且在白名单里。
fn classify(hcursor: isize, handles: &[isize]) -> bool {
    hcursor != 0 && handles.contains(&hcursor)
}

/// 当前是否处于交互提示态。取不到光标信息、或光标被系统抑制时一律 false。
pub fn is_interactive() -> bool {
    #[cfg(test)]
    if let Some(v) = FORCED.with(|c| c.get()) {
        return v;
    }
    let mut info = CURSORINFO {
        cbSize: std::mem::size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `info.cbSize` 已按约定填好，指针指向本栈帧内有效的结构体。
    if unsafe { GetCursorInfo(&mut info) }.is_err() {
        return false;
    }
    if info.flags.0 & CURSOR_SHOWING.0 == 0 {
        return false;
    }
    classify(info.hCursor.0 as isize, interactive_handles())
}

/// 测试专用：强制 `is_interactive()` 的返回值（`None` = 恢复真实查询）。
///
/// 只在 `cfg(test)` 下存在，生产构建看不到它。
#[cfg(test)]
pub fn force_for_test(value: Option<bool>) {
    FORCED.with(|c| c.set(value));
}

#[cfg(test)]
mod tests {
    use super::classify;

    #[test]
    fn zero_handle_is_never_interactive() {
        assert!(!classify(0, &[7, 9]));
    }

    #[test]
    fn whitelist_membership_decides() {
        let handles = [11, 22, 33];
        assert!(classify(22, &handles));
        assert!(!classify(44, &handles), "白名单外的自定义光标不算悬停");
        assert!(!classify(22, &[]), "空表不能 panic，也不算悬停");
    }

    #[test]
    fn real_system_cursor_query_does_not_panic() {
        // 只验不崩：机器上光标可能在任意位置，值本身无判据意义。
        let _ = super::is_interactive();
    }
}
