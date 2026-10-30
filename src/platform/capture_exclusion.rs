//! 抓屏时把 CurSeen 自己的**覆盖层**排除在捕获之外（仅桌面镜片类图元使用）。
//!
//! # 为什么必须排除
//!
//! `DrawCmd::Image` 画回覆盖层时会先抓一块桌面；覆盖层自己是靠 `UpdateLayeredWindow`
//! 提交的，`BitBlt` 无论带不带 `CAPTUREBLT` 都抓得到（2026-09-20 实测，见
//! [`crate::platform::screenshot`] 模块头）。不排除的后果就是画中画当初那个
//! **无限套娃**：镜片把上一帧的自己抓进源图再放大。
//!
//! 可靠的办法是 `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`：系统级、按窗口生效，
//! GDI / DXGI 都遵守。覆盖层是整屏窗，因此排除它意味着**这两个效果运行期间，覆盖层
//! 不会出现在任何系统截图/录屏里**（包括 CurSeen 自己的截图）。不另开小窗的理由：
//! 镜片要贴着光标 60fps 走，独立窗会引入窗口生命周期、置顶带与跨进程录制合成一整套
//! 负担，而它只服务两个效果。
//!
//! # 生命周期
//!
//! 由 [`crate::scripting::api_draw`] 在每次采样时调用 [`set_overlay_excluded`]；
//! 停止采样 1 秒后自动还原（`WDA_NONE`）。窗口销毁时属性随窗口消失，不写注册表、
//! 不跨进程，所以退出路径不需要额外收尾。

use std::cell::Cell;

use windows::core::PCWSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetWindowThreadProcessId, IsWindow, SetWindowDisplayAffinity,
    WDA_EXCLUDEFROMCAPTURE, WDA_NONE,
};

use crate::platform::topmost::OVERLAY_TITLE;

thread_local! {
    /// 覆盖层句柄缓存（UI 线程专用；窗口重建后由 `IsWindow` 校验失效）。
    static OVERLAY: Cell<Option<isize>> = const { Cell::new(None) };
    /// 当前是否已排除。
    static EXCLUDED: Cell<bool> = const { Cell::new(false) };
    /// 设置失败是否已经警告过（避免每帧刷日志）。
    static WARNED: Cell<bool> = const { Cell::new(false) };
}

/// 按标题找覆盖层窗口；缓存句柄失效时重找。
fn overlay_hwnd() -> Option<HWND> {
    if let Some(raw) = OVERLAY.with(Cell::get) {
        let h = HWND(raw as *mut core::ffi::c_void);
        // SAFETY: `IsWindow` 只查询句柄有效性，无前置条件。
        if unsafe { IsWindow(Some(h)) }.as_bool() {
            return Some(h);
        }
        OVERLAY.with(|c| c.set(None));
    }
    let mut wide: Vec<u16> = OVERLAY_TITLE.encode_utf16().collect();
    wide.push(0);
    // SAFETY: `wide` 以 NUL 结尾并在调用期间存活；`FindWindowW` 只读它。
    let h = unsafe { FindWindowW(None, PCWSTR(wide.as_ptr())) }.ok()?;
    if h.is_invalid() {
        return None;
    }
    // 只接受**本进程**的窗口：同名的窗口完全可能是另一个正在运行的 CurSeen 实例
    // （测试进程、便携版双开）。对别人的窗口设捕获策略既做不到（系统拒绝），
    // 也不该留下"测试顺手改了我正在用的实例"这种副作用。
    let mut pid = 0_u32;
    // SAFETY: `pid` 是本栈帧内有效的 u32 指针；调用只写它。
    unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
    if pid != unsafe { GetCurrentProcessId() } {
        return None;
    }
    OVERLAY.with(|c| c.set(Some(h.0 as isize)));
    Some(h)
}

/// 开关覆盖层的抓屏排除。返回**目标状态是否已成立**（幂等；窗口不在时视为失败）。
pub fn set_overlay_excluded(exclude: bool) -> bool {
    if EXCLUDED.with(Cell::get) == exclude {
        // 已处于目标态时仍要确认缓存句柄还活着：覆盖层可能因 DPI 变更被重建，
        // 新窗口**没有**继承旧的捕获策略。只查 IsWindow/重找，不重复设属性。
        if !exclude || overlay_hwnd().is_some() {
            return true;
        }
    }
    let Some(hwnd) = overlay_hwnd() else {
        if !WARNED.with(Cell::get) {
            WARNED.with(|c| c.set(true));
            log::warn!("桌面镜片：找不到覆盖层窗口，无法排除自身（本期不做镜片）");
        }
        return false;
    };
    let affinity = if exclude {
        WDA_EXCLUDEFROMCAPTURE
    } else {
        WDA_NONE
    };
    // SAFETY: 句柄来自本进程的顶层窗；`SetWindowDisplayAffinity` 只改该窗的捕获策略。
    match unsafe { SetWindowDisplayAffinity(hwnd, affinity) } {
        Ok(()) => {
            EXCLUDED.with(|c| c.set(exclude));
            true
        }
        Err(e) => {
            if !WARNED.with(Cell::get) {
                WARNED.with(|c| c.set(true));
                log::warn!(
                    "桌面镜片：SetWindowDisplayAffinity({exclude}) 失败：{e} \
                     —— 系统可能不支持 WDA_EXCLUDEFROMCAPTURE，镜片退回静态外观"
                );
            }
            false
        }
    }
}

/// 当前是否处于排除态（诊断 / 还原用）。
pub fn is_overlay_excluded() -> bool {
    EXCLUDED.with(Cell::get)
}

#[cfg(test)]
mod tests {
    /// 只钉常量语义：API 必须能在本机编译链接并能查询状态；不做真实窗口断言。
    #[test]
    fn query_is_side_effect_free() {
        let before = super::is_overlay_excluded();
        assert_eq!(before, super::is_overlay_excluded());
    }
}
