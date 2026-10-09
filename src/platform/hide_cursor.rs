//! 全局隐藏系统指针：激光笔等特效声明 `hide_cursor` 时，把系统光标藏起来；
//! 声明方全部失效后由 runtime 调用 [`show`] 恢复。
//!
//! # 为什么不用 `ShowCursor`
//!
//! 上一版用的是 `ShowCursor(FALSE)`。它**只在调用线程自己的窗口上**生效 —— `ShowCursor`
//! 维护的是"每个输入队列一份的显示计数器"，而把光标画到屏幕上的是**光标所在窗口的属主
//! 线程**。于是宿主窗口之外（别的程序、桌面、任务栏……）指针照旧显示，用户实测就是
//! "在很多地方都依然能看到原来的鼠标"。
//!
//! # 现在：换掉系统光标位图
//!
//! [`SetSystemCursor`] 把某个 `OCR_*`（系统光标）的**位图**换成我们给的位图，改的是
//! 系统级设置，因此所有程序、所有窗口上都生效。把一个"AND 掩码全 1 + XOR 掩码全 0"
//! 的单色光标（按光栅运算 `(屏幕 AND AND) XOR XOR` = 原样）交给它，画出来就是**全透明**。
//!
//! 恢复走 [`SystemParametersInfoW`]`(SPI_SETCURSORS)`：让系统从注册表把光标全部重新
//! 加载一遍（用户自定义的指针外观就在那里），不依赖我们记着原始句柄。
//!
//! ★ `SetSystemCursor` **会接管并销毁**传入的光标句柄，所以每个 `OCR_*` 都要单独
//! `CreateCursor` 一个 —— 同一个句柄交给第二个 id 就是用已释放的对象。
//!
//! # 兜底
//!
//! 换的是系统级设置，进程若异常退出就会把"没有指针"留给用户（直到重启 / 注销）。
//! 因此除了 `Runtime` 的 `Drop`，panic 钩子（[`install_panic_restore`]）与 SEH 处理器
//! （`platform::crash`）也各挂了一次恢复 —— `panic = "abort"` 下 `Drop` 根本不会跑。

use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::UI::WindowsAndMessaging::{
    CreateCursor, SetSystemCursor, SystemParametersInfoW, OCR_APPSTARTING, OCR_CROSS, OCR_HAND,
    OCR_HELP, OCR_IBEAM, OCR_NO, OCR_NORMAL, OCR_SIZEALL, OCR_SIZENESW, OCR_SIZENS, OCR_SIZENWSE,
    OCR_SIZEWE, OCR_UP, OCR_WAIT, SPIF_SENDCHANGE, SPI_SETCURSORS, SYSTEM_CURSOR_ID,
};

/// 当前是否已处于隐藏态。只记"我们这一层"的意图，保证进 / 出隐藏态各只动系统一次。
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// 光标位图边长（Windows 光标常规尺寸）。
const CUR_W: i32 = 32;
const CUR_H: i32 = 32;
/// 单色光标掩码的字节数 = 宽 × 高 / 8（每位一个像素）。
const MASK_BYTES: usize = (CUR_W as usize) * (CUR_H as usize) / 8;

/// 会被替换掉的系统光标全集。
///
/// 覆盖 Windows 的全部 `OCR_*`：常规箭头 / 文本 I 形 / 忙 / 十字 / 向上箭头 / 四向与
/// 对角缩放 / 禁止 / 手形 / 后台忙 / 帮助问号。漏掉哪一个，指针切到那个形态时就会
/// 重新出现在屏幕上。
const CURSOR_IDS: [SYSTEM_CURSOR_ID; 14] = [
    OCR_NORMAL,
    OCR_IBEAM,
    OCR_WAIT,
    OCR_CROSS,
    OCR_UP,
    OCR_SIZENWSE,
    OCR_SIZENESW,
    OCR_SIZEWE,
    OCR_SIZENS,
    OCR_SIZEALL,
    OCR_NO,
    OCR_HAND,
    OCR_APPSTARTING,
    OCR_HELP,
];

/// 进入隐藏态（幂等）。
pub fn hide() {
    if ACTIVE.swap(true, Ordering::SeqCst) {
        return;
    }
    // SAFETY: 只调用两个无前置条件的 user32 调用；掩码缓冲区活到循环结束。
    unsafe {
        apply_blank_cursors();
    }
}

/// 退出隐藏态（幂等）。
pub fn show() {
    if !ACTIVE.swap(false, Ordering::SeqCst) {
        return;
    }
    // SAFETY: 恢复系统光标，无前置条件。
    unsafe {
        restore_system_cursors();
    }
}

/// 进程退出兜底：若仍处于隐藏态，强制恢复系统光标（异常 / 强制退出路径也能用）。
pub fn ensure_shown() {
    if ACTIVE.swap(false, Ordering::SeqCst) {
        // SAFETY: 同 `show`。
        unsafe {
            restore_system_cursors();
        }
    }
}

/// 在 panic 通道上兜底恢复系统光标。
///
/// 与 [`crate::core::logger::install_panic_hook`] 是同一条链上的两环：它写日志，我们
/// 还指针。**必须在它之后调用** —— 这里用 `take_hook` 取出上一环，包一层再装回去。
///
/// 为什么需要它：release 档 `panic = "abort"`，`abort()` 直接终止进程，`Runtime::drop`
/// 不会执行；那是"崩溃后整台机器没有鼠标指针"的唯一防线。
///
/// 放在 `platform` 而非 `core::logger`：核心层不准依赖平台层（分层铁律），所以链尾这一环
/// 由 `main` 组装。
pub fn install_panic_restore() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ensure_shown();
        prev(info);
    }));
}

/// 把全部系统光标换成全透明的单色光标。
///
/// # Safety
/// 调用方保证在 Windows 上运行（本模块整体由 `cfg(windows)` 编译进目标）。
unsafe fn apply_blank_cursors() {
    // 单色光栅运算结果 = (屏幕 AND AND掩码) XOR XOR掩码。
    // AND 全 1、XOR 全 0 ⇒ 结果就是原屏幕内容 ⇒ 什么都不画 = 完全透明。
    let and_plane = [0xFFu8; MASK_BYTES];
    let xor_plane = [0x00u8; MASK_BYTES];
    for id in CURSOR_IDS {
        // 每个 id 一个**新建**句柄：`SetSystemCursor` 会销毁传给它的那个（见模块文档）。
        if let Ok(hcur) = CreateCursor(
            None,
            0,
            0,
            CUR_W,
            CUR_H,
            and_plane.as_ptr().cast(),
            xor_plane.as_ptr().cast(),
        ) {
            // 单个失败不致命（比如另一处已接管该光标），其余继续换。
            let _ = SetSystemCursor(hcur, id);
        }
    }
}

/// 从注册表重新加载全部系统光标（用户的指针外观偏好就存在那里）。
///
/// # Safety
/// 同 [`apply_blank_cursors`]。
unsafe fn restore_system_cursors() {
    let _ = SystemParametersInfoW(SPI_SETCURSORS, 0, None, SPIF_SENDCHANGE);
}
