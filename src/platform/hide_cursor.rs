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
//!
//! ★ 2026-10-26 用户实测：正常退出后箭头仍然是透明的。根因不是恢复函数失效，而是
//! `Runtime` 被 `Rc` 闭环留住、`Drop` 没有在退出路径上执行。正常退出现在由
//! `overlay::run_overlay` 在 `app.run()` 返回后**显式**调 [`ensure_shown`]；应用入口
//! 返回前再调一次。另留一个 `cursor-hidden.marker`：只有真正隐藏过才创建，恢复时删除；
//! 首实例启动时 [`restore_after_crash`] 会无条件重载一次系统光标，因此即使进程被
//! 任务管理器强杀（连标记都没来得及删），下次启动也一定能回到有指针的状态。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::UI::WindowsAndMessaging::{
    CreateCursor, SetSystemCursor, SystemParametersInfoW, OCR_NORMAL, SPIF_SENDCHANGE,
    SPI_SETCURSORS, SYSTEM_CURSOR_ID,
};

/// 当前是否已处于隐藏态。只记"我们这一层"的意图，保证进 / 出隐藏态各只动系统一次。
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// 隐藏态标记：进程被强杀时留给下次启动的"上次没有正常收尾"证据。
fn marker_path() -> PathBuf {
    crate::core::paths::log_dir().join("cursor-hidden.marker")
}

/// 光标位图边长（Windows 光标常规尺寸）。
const CUR_W: i32 = 32;
const CUR_H: i32 = 32;
/// 单色光标掩码的字节数 = 宽 × 高 / 8（每位一个像素）。
const MASK_BYTES: usize = (CUR_W as usize) * (CUR_H as usize) / 8;

/// 只替换**常规箭头**，保留 I 形 / 手形 / 缩放 / 禁止等交互光标。
///
/// ★ 2026-10-26 用户反馈：把所有 `OCR_*` 都藏掉后，鼠标移到窗口边框不再出现
/// “可缩放”提示、移到输入框也没有 I 形，用户不知道指针当前能做什么。
/// 只藏箭头既能让自绘光标成为主视觉，又能把系统的交互提示留给用户；自绘光标
/// 仍画在指针处，所以“箭头 + 缩放图标”并存是可接受的取舍。
const CURSOR_IDS: [SYSTEM_CURSOR_ID; 1] = [OCR_NORMAL];

/// 进入隐藏态（幂等）。
pub fn hide() {
    if ACTIVE.swap(true, Ordering::SeqCst) {
        return;
    }
    // 先留标记再动系统光标：即使下一步崩溃，下次启动也知道要补恢复。
    if let Some(dir) = marker_path().parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(marker_path(), b"1");
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
    force_restore();
}

/// 进程退出兜底：若仍处于隐藏态，强制恢复系统光标（异常 / 强制退出路径也能用）。
pub fn ensure_shown() {
    if ACTIVE.swap(false, Ordering::SeqCst) {
        force_restore();
    }
}

/// 无条件从注册表重载系统光标，并清掉隐藏标记。
///
/// 与 [`show`] / [`ensure_shown`] 的区别：不看 `ACTIVE`。强杀后由下次启动的
/// [`restore_after_crash`] 调用时，本进程的 `ACTIVE` 恒为 false，必须无条件恢复。
pub fn force_restore() {
    // SAFETY: 恢复系统光标，无前置条件。
    unsafe {
        restore_system_cursors();
    }
    let _ = std::fs::remove_file(marker_path());
}

/// 启动兜底：首实例启动时**无条件**从注册表重载一次系统光标。
///
/// 为什么无条件：旧版本没有标记文件，若它被强杀在隐藏态，标记这条路救不了；
/// 而 `SPI_SETCURSORS` 只是按用户当前方案重载，外观不变，代价可以忽略。
/// 标记只用来区分“上次是异常收尾”，写一条 warn 方便排查。
///
/// 由 `lib::run` 在**通过单实例闸门之后**调用：二次实例不能动，否则会把首实例
/// 正在使用的隐藏态顶掉。
pub fn restore_after_crash() {
    if marker_path().exists() {
        log::warn!("检测到上次退出时系统光标仍处于隐藏态，启动时强制恢复");
    }
    force_restore();
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

/// 把常规箭头换成全透明的单色光标（其它交互光标保持系统原样）。
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
