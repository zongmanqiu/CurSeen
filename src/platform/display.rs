//! 显示器几何：虚拟桌面矩形。
//!
//! 覆盖层必须精确覆盖**虚拟桌面**（所有显示器拼起来的那块），而不是"主显示器"——
//! 副屏放在主屏左侧时虚拟屏原点 x 是负的，只按主屏算会让副屏上的鼠标特效整体错位。
//!
//! [`ScreenRect`] 这个**纯数据**已下沉到 `core::geometry`（它没有任何 Win32 调用，
//! 而 `core::backdrop` 需要用它来保持 core 层的纯净）。这里重导出，让既有
//! `platform::display::ScreenRect` 的引用继续成立；真正依赖 Win32 的是本模块的
//! [`virtual_screen`] / [`monitor_rect_at`] 等函数。

pub use crate::core::geometry::ScreenRect;

/// 当前鼠标位置（物理像素，虚拟桌面坐标系，x/y 可能为负）。
///
/// 截图要"截鼠标所在的那块显示器"，入口先用它拿到落点再交给
/// [`crate::platform::screenshot::grab_monitor_at`]。取不到时返回 (0, 0)（退回主屏）。
#[cfg(windows)]
pub fn cursor_pos() -> (i32, i32) {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut p = POINT::default();
    if unsafe { GetCursorPos(&mut p) }.is_ok() {
        (p.x, p.y)
    } else {
        (0, 0)
    }
}

/// 非 Windows 下的桩。
#[cfg(not(windows))]
pub fn cursor_pos() -> (i32, i32) {
    (0, 0)
}

/// 系统 DPI 缩放因子（逻辑像素 / 物理像素）。
///
/// 覆盖层窗以**物理像素**为坐标基准（虚拟桌面矩形取自 `GetSystemMetrics`，物理像素），
/// 而 windui 的 Canvas 绘制坐标系是**逻辑像素**，平台按此因子把逻辑坐标乘成物理。
/// 把世界坐标（物理）映射到窗口本地（逻辑）必须除以这个因子：`scale = 1 / dpi_scale()`。
///
/// 取不到（理论不会）时返回 1.0——等于 100% DPI，画得出但高 DPI 下略错位。
#[cfg(windows)]
pub fn dpi_scale() -> f32 {
    use windows::Win32::UI::HiDpi::GetDpiForSystem;
    let dpi = unsafe { GetDpiForSystem() };
    if dpi == 0 {
        1.0
    } else {
        dpi as f32 / 96.0
    }
}

/// 非 Windows 下的桩：同 `paint.rs` 的跨平台考虑。
#[cfg(not(windows))]
pub fn dpi_scale() -> f32 {
    1.0
}

/// 取虚拟桌面矩形。
///
/// 不用 `EnumDisplayMonitors`：`GetSystemMetrics` 一次拿四个值就够，且不回调、
/// 不分配，启动路径上更省。`SM_XVIRTUALSCREEN` 系列正是为此存在。
#[cfg(windows)]
pub fn virtual_screen() -> ScreenRect {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    };
    let r = ScreenRect {
        x: unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) },
        y: unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) },
        w: unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) },
        h: unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) },
    };
    r.or_fallback()
}

/// 非 Windows 目标下的桩：本项目只跑 Windows，留它是为了让 lib 在别的平台上
/// 也能编译（`cargo check` 跨平台时不至于满屏红）。
#[cfg(not(windows))]
pub fn virtual_screen() -> ScreenRect {
    ScreenRect::FALLBACK
}

/// 取主显示器工作区（排除任务栏）。
///
/// 用于弹窗定位：提示窗不应遮挡任务栏。
#[cfg(windows)]
pub fn work_area() -> ScreenRect {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    };
    let mut rect = RECT::default();
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            std::mem::size_of::<RECT>() as u32,
            Some(&mut rect as *mut RECT as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    if ok.is_ok() {
        ScreenRect {
            x: rect.left,
            y: rect.top,
            w: rect.right - rect.left,
            h: rect.bottom - rect.top,
        }
    } else {
        virtual_screen()
    }
}

/// 非 Windows 目标下的桩。
#[cfg(not(windows))]
pub fn work_area() -> ScreenRect {
    virtual_screen()
}

/// 取 `x,y` 落点所在**那台显示器**的工作区（排除任务栏）。
///
/// [`work_area()`] 只有主屏：贴图右键菜单可能弹在任何一块屏上，子菜单防遮挡的
/// 留白钳位必须按落点显示器的工作区算（副屏在主屏左侧时 x 是负的，同样要保真）。
/// 取不到（理论不会）→ 回落虚拟桌面矩形。
#[cfg(windows)]
pub fn work_area_at(x: i32, y: i32) -> ScreenRect {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    let pt = POINT { x, y };
    let mon = unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST) };
    if mon.is_invalid() {
        return virtual_screen();
    }
    let mut mi = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetMonitorInfoW(mon, &mut mi) }.as_bool() {
        ScreenRect {
            x: mi.rcWork.left,
            y: mi.rcWork.top,
            w: mi.rcWork.right - mi.rcWork.left,
            h: mi.rcWork.bottom - mi.rcWork.top,
        }
    } else {
        virtual_screen()
    }
}

/// 非 Windows 目标下的桩。
#[cfg(not(windows))]
pub fn work_area_at(_x: i32, _y: i32) -> ScreenRect {
    virtual_screen()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_used_when_metrics_are_empty() {
        let empty = ScreenRect {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        };
        assert_eq!(empty.or_fallback(), ScreenRect::FALLBACK);
        let neg = ScreenRect {
            x: -1920,
            y: -100,
            w: -5,
            h: 1080,
        };
        assert_eq!(neg.or_fallback(), ScreenRect::FALLBACK);
    }

    #[test]
    fn negative_origin_is_preserved() {
        // 副屏在主屏左侧：原点为负。这条断言锁住"别把负坐标当非法值丢掉"。
        let r = ScreenRect {
            x: -1920,
            y: -100,
            w: 3840,
            h: 1180,
        };
        assert_eq!(r.or_fallback().x, -1920);
        assert_eq!(r.or_fallback().y, -100);
    }

    #[cfg(windows)]
    #[test]
    fn virtual_screen_is_plausible() {
        let r = virtual_screen();
        assert!(r.w >= 640 && r.h >= 480, "虚拟屏尺寸不合理：{r:?}");
    }
}
