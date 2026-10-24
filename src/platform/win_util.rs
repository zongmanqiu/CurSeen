//! 零散 Win32 工具函数。

use windows::Win32::Foundation::{COLORREF, HWND, RECT};

/// 原生输入框 / 结果框宽度：与面板快捷键框（`ui::card::CONTROL_GROUP_W = 200`）对齐。
///
/// 2026-09-29「文本输入框宽度统一」：原生弹窗输入框此前硬编码 520，与面板里所有输入类
/// 控件（快捷键框 / 下拉 / 数值框都是 `CONTROL_GROUP_W`）不一致。这里镜像同一数值——
/// `platform` 层不反向依赖 `ui`，故在此单独定义，并注明必须随 `CONTROL_GROUP_W` 同步。
const NATIVE_DIALOG_W: i32 = 200;

/// 系统启动以来的毫秒数（与钩子事件时间戳同源，可直接比较）。
pub fn tick_ms() -> u64 {
    unsafe { windows::Win32::System::SystemInformation::GetTickCount() as u64 }
}

/// 系统 CapsLock 是否开着。
///
/// 直接问系统而不是自己数：程序启动前 Caps 就开着的话，本地计数必然是错的。
/// 放在 `platform` 层（而不是 `modules`）是因为它裸调 `windows` crate——`modules`
/// 禁引外部 Win32 crate（分层硬规则，见 AGENTS.md 第三节）。
pub fn caps_lock_on() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CAPITAL};
    unsafe { GetKeyState(VK_CAPITAL.0 as i32) & 0x0001 != 0 }
}

/// 把光标换成标准**十字**（`IDC_CROSS`）：贴图「取色」的吸管形态。
///
/// ★ 每次 `WM_MOUSEMOVE` 都要再调一次：系统在每条鼠标移动后都会发 `WM_SETCURSOR`、
///   把光标重置回窗口类光标——只在进入取色模式时设一次是压不住的。退出模式后不再调，
///   类光标自然回来，无需恢复。
pub fn set_cursor_cross() {
    use windows::Win32::UI::WindowsAndMessaging::{LoadCursorW, SetCursor, IDC_CROSS};
    unsafe {
        if let Ok(c) = LoadCursorW(None, IDC_CROSS) {
            let _ = SetCursor(Some(c));
        }
    }
}

/// 虚拟桌面矩形（含多显示器负坐标）：(left, top, right, bottom)。
pub fn virtual_screen_rect() -> (i32, i32, i32, i32) {
    unsafe {
        let x = windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
            windows::Win32::UI::WindowsAndMessaging::SM_XVIRTUALSCREEN,
        );
        let y = windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
            windows::Win32::UI::WindowsAndMessaging::SM_YVIRTUALSCREEN,
        );
        let w = windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
            windows::Win32::UI::WindowsAndMessaging::SM_CXVIRTUALSCREEN,
        );
        let h = windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
            windows::Win32::UI::WindowsAndMessaging::SM_CYVIRTUALSCREEN,
        );
        (x, y, x + w, y + h)
    }
}

/// 按窗口标题找到顶层窗口并投递 `WM_CLOSE`。
///
/// 用于「进入标注模式时自动收起控制面板」（★ 修复：面板开着 → 标注输入挂起 +
/// 覆盖层保持穿透，标注"进得去、动不了、退不出"）。面板是 windui 的独立顶层窗口，
/// 覆盖层的 `EventCtx` 只能操作覆盖层自己，跨窗口关闭只有 Win32 这一条路。
///
/// 用 `WM_CLOSE` 而不是 `DestroyWindow`：事件走 windui 的正常关闭链，
/// `Window::on_close_request` 里的收尾（`Runtime::close_panel` 把改动落盘）照常执行。
///
/// 返回是否找到窗口并成功投递。找不到（面板本就没开）不算错误。
pub fn post_close_by_title(title: &str) -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, WM_CLOSE};

    let mut wide: Vec<u16> = title.encode_utf16().collect();
    wide.push(0);
    unsafe {
        let Ok(hwnd) = FindWindowW(None, PCWSTR(wide.as_ptr())) else {
            return false;
        };
        if hwnd.is_invalid() {
            return false;
        }
        PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)).is_ok()
    }
}

/// 按标题把窗口带到前台并给键盘焦点（截图区域窗 / 贴图窗开窗后调用）。
///
/// 后台进程直接 SetForegroundWindow 会被前台锁拒绝：先用一次 ALT 按下/抬起
/// 解除限制（系统规则：ALT 按下期间允许改前台），再 SetWindowPos(TOPMOST)
/// + SetForegroundWindow。失败不致命（区域窗首次点击也会激活），返回是否成功。
pub fn activate_window_by_title(title: &str) -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYEVENTF_KEYUP, VK_MENU};
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, SetForegroundWindow, SetWindowPos, HWND_TOPMOST, SWP_NOMOVE, SWP_NOSIZE,
        SWP_SHOWWINDOW,
    };
    let mut wide: Vec<u16> = title.encode_utf16().collect();
    wide.push(0);
    unsafe {
        let hwnd = match FindWindowW(None, PCWSTR(wide.as_ptr())) {
            Ok(h) if !h.is_invalid() => h,
            _ => return false,
        };
        keybd_event(VK_MENU.0 as u8, 0, KEYEVENTF_KEYUP, 0);
        keybd_event(VK_MENU.0 as u8, 0, KEYEVENTF_KEYUP, 0);
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
        );
        SetForegroundWindow(hwnd).as_bool()
    }
}

/// 按标题把窗口叫到前台并给键盘焦点，**但不把它送进置顶带**（z 序留在普通档）。
///
/// 与 [`activate_window_by_title`] 的唯一差别是 `SetWindowPos` 的第二个参数取 `HWND_TOP`
/// 而不是 `HWND_TOPMOST`：前者只在**普通窗口**那一档里抬到最前，后者会把窗口搬进置顶带，
/// 从此压在**所有**普通窗口（包括系统自己的文件对话框）上面。
///
/// ★ 为什么裁剪窗必须用这个（用户 2026-09-20 报告）：裁剪窗要弹原生「打开 / 另存为」，
///   而那个对话框是**普通窗口**。裁剪窗一旦进了置顶带，用户就再也看不见也点不到对话框
///   —— 现象是"点「打开…」什么都没发生"。更坏的是：置顶的裁剪窗仍可点，用户越点越急，
///   每一下都会在上一层的模态循环里再套一层新对话框（实测 3 次点击 = 3 层对话框，
///   最后按 Esc 连打三次才退干净）。
///
/// ★ 需要"贴在最上层且别打断用户"（GIF 控件条）用 [`set_topmost_by_title`]；
///   需要"当真正的顶窗"（截图选框 / 贴图）才用 [`activate_window_by_title`]。
pub fn focus_window_by_title(title: &str) -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
    let mut wide: Vec<u16> = title.encode_utf16().collect();
    wide.push(0);
    unsafe {
        match FindWindowW(None, PCWSTR(wide.as_ptr())) {
            Ok(h) if !h.is_invalid() => focus_window_hwnd(h.0 as isize),
            _ => false,
        }
    }
}

/// [`focus_window_by_title`] 的句柄版：**叫到前台并给键盘焦点，但不进置顶带**。
///
/// 抽出来的两个用途：
/// - 已经握着 HWND 的调用方不必再去 `FindWindowW` 一轮；
/// - ★ 绕开"标题会变"这个坑（`FindWindowW` 是全等匹配）：AI 对话窗等回复时标题是
///   `AI 写脚本 (Ns)`，按常量标题查必然漏判 —— 漏判不会报错，只会默默再开一个窗。
///
/// 句柄无效（窗已销毁）返回 `false` 且**什么都不做**：既可以当"这窗还在吗"的探测用，
/// 也不会因为一次误调用把某个无辜的窗口顶起来。
pub fn focus_window_hwnd(hwnd: isize) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYEVENTF_KEYUP, VK_MENU};
    use windows::Win32::UI::WindowsAndMessaging::{
        IsWindow, SetForegroundWindow, SetWindowPos, HWND_TOP, SWP_NOMOVE, SWP_NOSIZE,
        SWP_SHOWWINDOW,
    };
    if hwnd == 0 {
        return false;
    }
    let hwnd = HWND(hwnd as *mut _);
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            return false;
        }
        // 与 `activate_window_by_title` 同款：后台进程直接改前台会被前台锁拒绝，先用一次
        // ALT 抬起解除限制。
        keybd_event(VK_MENU.0 as u8, 0, KEYEVENTF_KEYUP, 0);
        keybd_event(VK_MENU.0 as u8, 0, KEYEVENTF_KEYUP, 0);
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOP),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
        );
        SetForegroundWindow(hwnd).as_bool()
    }
}

///
/// 把按标题找到的窗口**补进置顶带**，且**不抢焦点**。
///
/// 与 [`activate_window_by_title`] 的分工：那个是"叫到前台来"（裁剪窗要收键盘输入，所以
/// 需要焦点）；这个是"贴在最上层但别打断用户"。
///
/// ★ 为什么 GIF 控件条必须用它：`Window::new` 建的普通窗**不带 `WS_EX_TOPMOST`**
///   （见 windui `mod.rs` 里 `ex_style` 只在 `cfg.layered` 时加 TOPMOST），所以控件条原先
///   会被任何被抬到前面的普通窗口盖住（用户 2026-09-20 报告"选框会被其它软件窗口遮挡"）。
///   而用 `activate_window_by_title` 会顺手抢走前台 —— 录制前把用户的窗口夺走焦点是倒退。
///
/// ★ 「置顶带内靠前」不保证：后来者仍会插在我们上面，所以调用方要周期性重试
///   （见 `platform::topmost::TopmostKeeper`）。
pub fn set_topmost_by_title(title: &str) -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
        SWP_SHOWWINDOW,
    };
    let mut wide: Vec<u16> = title.encode_utf16().collect();
    wide.push(0);
    unsafe {
        let hwnd = match FindWindowW(None, PCWSTR(wide.as_ptr())) {
            Ok(h) if !h.is_invalid() => h,
            _ => return false,
        };
        SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW | SWP_NOACTIVATE,
        )
        .is_ok()
    }
}

/// 按标题平移窗口到指定屏幕坐标（贴图窗拖动用）。窗口不存在时返回 false。
pub fn move_window_by_title(title: &str, x: i32, y: i32) -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, SetWindowPos, HWND_TOPMOST, SWP_NOSIZE,
    };
    let mut wide: Vec<u16> = title.encode_utf16().collect();
    wide.push(0);
    unsafe {
        let hwnd = match FindWindowW(None, PCWSTR(wide.as_ptr())) {
            Ok(h) if !h.is_invalid() => h,
            _ => return false,
        };
        SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, 0, 0, SWP_NOSIZE).is_ok()
    }
}

/// 按窗口标题设置**完整矩形**（位置 + 尺寸，均为物理像素）。窗口不存在时返回 `false`。
///
/// GIF 控件条要跟着选框走：选框一拖动/缩放，条就得重定位并跟着变宽，找不到窗口就算失败。
/// `SWP_NOACTIVATE` 不抢焦点（与 [`set_window_rect_hwnd`] 同一档）。
pub fn set_window_rect_by_title(title: &str, x: i32, y: i32, w: i32, h: i32) -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE,
    };
    let mut wide: Vec<u16> = title.encode_utf16().collect();
    wide.push(0);
    unsafe {
        let hwnd = match FindWindowW(None, PCWSTR(wide.as_ptr())) {
            Ok(h) if !h.is_invalid() => h,
            _ => return false,
        };
        SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE).is_ok()
    }
}

/// 把**已知 HWND** 的窗口平移到指定屏幕坐标（贴图窗拖动用）。
///
/// 比 [`move_window_by_title`] 省掉了每次的 `FindWindowW`：拖动时鼠标移动每秒上百条，
/// 每条都扫一遍顶层窗口表是白白浪费。HWND 由控件在首个事件里经
/// `windui::platform::native_window_handle()` 缓存。
///
/// 保持 `HWND_TOPMOST`：贴图窗要压在普通窗口上方，同时又不抢焦点。
pub fn move_window_hwnd(hwnd: isize, x: i32, y: i32) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowPos, HWND_TOPMOST, SWP_NOSIZE};
    if hwnd == 0 {
        return false;
    }
    unsafe {
        SetWindowPos(
            HWND(hwnd as *mut _),
            Some(HWND_TOPMOST),
            x,
            y,
            0,
            0,
            SWP_NOSIZE,
        )
        .is_ok()
    }
}

/// 把窗口抬到**最顶那一档**（不改位置、不改尺寸、不抢焦点）。
///
/// 贴图窗用：用户点哪一张，哪一张就到最上层（同一 owner 的多个 owned 子窗之间，
/// z 序默认只是创建顺序）。用 `HWND_TOPMOST` 而非 `HWND_TOP`：贴图窗本就该压在普通窗口
/// 之上，且 [`move_window_hwnd`] 拖动时用的也是这一档——两处一致，不会出现"点一下被抬高、
/// 拖一下又掉回普通档"的跳变。
pub fn raise_window_hwnd(hwnd: isize) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    };
    if hwnd == 0 {
        return false;
    }
    unsafe {
        SetWindowPos(
            HWND(hwnd as *mut _),
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        )
        .is_ok()
    }
}

/// 读已知 HWND 的整块屏幕矩形 `(x, y, w, h)`（物理像素，含副屏负坐标）。
///
/// 贴图滚轮缩放时用：锚点要算在**图片**左上角上（不是窗口左上角），得先知道窗口当前
/// 左上角与尺寸。`HWND` 由控件在首个事件里经 `windui::platform::native_window_handle()`
/// 缓存，省掉每次的 `FindWindowW`（与 [`move_window_hwnd`] 同款思路）。
///
/// 找不到 / 最小化中返回 `None`（坐标无意义）。
pub fn window_rect_hwnd(hwnd: isize) -> Option<(i32, i32, i32, i32)> {
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
    if hwnd == 0 {
        return None;
    }
    let mut rect = RECT::default();
    unsafe {
        let h = windows::Win32::Foundation::HWND(hwnd as *mut _);
        if GetWindowRect(h, &mut rect).is_err() {
            return None;
        }
    }
    Some((
        rect.left,
        rect.top,
        rect.right.saturating_sub(rect.left),
        rect.bottom.saturating_sub(rect.top),
    ))
}

/// 把已知 HWND 的窗口移到 `(x, y)` 并设尺寸 `(w, h)`（物理像素，全部坐标）。
///
/// 贴图滚轮缩放用：窗口必须跟着图片一起放大/缩小（非分层窗的"透明"区实际是黑，
/// 窗口不变、图在窗口里缩小会露出一圈黑边）。
///
/// 保持 `HWND_TOPMOST`（贴在普通窗口上方，与 [`move_window_hwnd`] / [`raise_window_hwnd`]
/// 同一档，缩放不会让贴图掉回普通 z 序）+ `SWP_NOACTIVATE`（不抢焦点，缩放不该打断
/// 用户正在打的字）。
pub fn set_window_rect_hwnd(hwnd: isize, x: i32, y: i32, w: i32, h: i32) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE};
    if hwnd == 0 {
        return false;
    }
    unsafe {
        SetWindowPos(
            HWND(hwnd as *mut _),
            Some(HWND_TOPMOST),
            x,
            y,
            w,
            h,
            SWP_NOACTIVATE,
        )
        .is_ok()
    }
}

/// 把一个 `w × h` 的窗口钳进虚拟桌面内（贴图窗 / 输入窗落点用）。
///
/// `screen` 是**整块虚拟桌面**（物理像素，副屏在左时原点为负）——只按主屏钳会在负坐标
/// 那一半上把窗口推进主屏。放不下时返回桌面原点，绝不把窗口丢到屏幕外。
pub fn clamp_pos(
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    screen: crate::platform::display::ScreenRect,
) -> (i32, i32) {
    let max_x = screen.x.saturating_add(screen.w).saturating_sub(w);
    let max_y = screen.y.saturating_add(screen.h).saturating_sub(h);
    (x.min(max_x).max(screen.x), y.min(max_y).max(screen.y))
}

/// 按窗口标题读取顶层窗口的左上角屏幕坐标（物理像素，含副屏负坐标）。
/// 找不到 / 最小化中时返回 `None`（坐标无意义或没有窗口可记）。
pub fn window_pos_by_title(title: &str) -> Option<(i32, i32)> {
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

    let hwnd = find_by_title(title)?;
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect).ok()? };
    Some((rect.left, rect.top))
}

/// 按窗口标题读取**整块**屏幕矩形 `(x, y, w, h)`（物理像素）。
///
/// 贴图选中态对账用：判断"这次点击落在选中的那张贴图里没有"。判据必须是**几何**的
/// （查一次矩形），不能靠窗口消息的先后——贴图窗自己的 `WM_LBUTTONDOWN` 与全局钩子
/// 投递过来的按下事件谁先谁后是不确定的。
///
/// 找不到 / 最小化中返回 `None`。
pub fn window_rect_by_title(title: &str) -> Option<(i32, i32, i32, i32)> {
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

    let hwnd = find_by_title(title)?;
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect).ok()? };
    Some((
        rect.left,
        rect.top,
        rect.right.saturating_sub(rect.left),
        rect.bottom.saturating_sub(rect.top),
    ))
}

/// 请求重绘某个窗口（`InvalidateRect`，异步、不阻塞）。
///
/// 贴图选中态切换时用：被"取消高亮"的那一张不是当前事件的窗口，只能从外面把它标脏，
/// 否则它的选中实边会一直留到下次重绘。
pub fn invalidate_by_title(title: &str) -> bool {
    use windows::Win32::Graphics::Gdi::InvalidateRect;

    let Some(hwnd) = find_by_title(title) else {
        return false;
    };
    // 第二个参数 `None` = 整块客户区；第三个 `true` = 也擦背景（贴图窗反正整幅重画）。
    unsafe { InvalidateRect(Some(hwnd), None, true).as_bool() }
}

/// 按标题找顶层窗口（找不到 / 已最小化返回 `None`——最小化时坐标是 `-32000` 这类哨兵值）。
fn find_by_title(title: &str) -> Option<windows::Win32::Foundation::HWND> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, IsIconic};

    let mut wide: Vec<u16> = title.encode_utf16().collect();
    wide.push(0);
    unsafe {
        let hwnd = FindWindowW(None, PCWSTR(wide.as_ptr())).ok()?;
        if hwnd.is_invalid() || IsIconic(hwnd).as_bool() {
            return None;
        }
        Some(hwnd)
    }
}

/// 把窗口整体设为 `alpha`（0..=255，255 = 完全不透明），实现"贴图透明度 10%~100%"。
///
/// 用 Win32 的 **整窗 alpha**（`WS_EX_LAYERED` + `SetLayeredWindowAttributes`）而不是 windui
/// 的分层窗口：windui 的分层后端受**进程级 PARKED**（覆盖层休眠）影响——休眠会清空那个
/// 分层的 DIB，把贴图一起抹掉（见 `patches/PATCHES.md` 与 `layered.rs`）。而这里只是对该
/// HWND 打一个系统级的整窗透明度属性，windui 仍按普通窗口正常 `WM_PAINT`，DWM 再按 alpha
/// 合成——与覆盖层休眠互不相干。
///
/// 第一次给窗口设透明度时要把 `WS_EX_LAYERED` 挂上（之后一直挂着；alpha=255 时视觉等同
/// 不透明，无需来回改 ex-style 以免闪烁）。
pub fn set_window_alpha(hwnd: isize, alpha: u8) -> bool {
    use windows::Win32::Foundation::{COLORREF, HWND};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetLayeredWindowAttributes, SetWindowLongPtrW, GWL_EXSTYLE, LWA_ALPHA,
        WS_EX_LAYERED,
    };
    if hwnd == 0 {
        return false;
    }
    unsafe {
        let h = HWND(hwnd as *mut _);
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        if ex & (WS_EX_LAYERED.0 as isize) == 0 {
            // 只关心"有没有挂上"；失败那一下不阻塞后续 SetLayeredWindowAttributes。
            SetWindowLongPtrW(h, GWL_EXSTYLE, ex | WS_EX_LAYERED.0 as isize);
        }
        SetLayeredWindowAttributes(h, COLORREF(0), alpha, LWA_ALPHA).is_ok()
    }
}

/// Ctrl 键当前是否按着。
///
/// 贴图窗滚轮手势用：按住 Ctrl 滚轮 = 调透明度（否则缩放）。直接问系统而不是自己记键，
/// 因为窗口**不抢焦点**（贴图不该打断用户输入），键盘焦点可能根本不在本进程里，windui
/// 收不到按键事件——只有系统级 `GetKeyState` 能反映"此刻是不是按着 Ctrl"。
pub fn control_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL};
    // `GetKeyState` 返回 SHORT（i16），高位 0x8000 在 i16 里是 `-32768`；先扩到 i32 再与，
    // 避免字面量越界。`0x8000` 位 = 键当前按下。
    unsafe { GetKeyState(VK_CONTROL.0 as i32) as i32 & 0x8000 != 0 }
}

/// 一组原生菜单句柄（`HMENU`，用 `isize` 传递，避免跨 `windows` crate 特征）。
pub type MenuHandle = isize;

/// 建一个空的弹出菜单。
/// 在资源管理器中打开一个目录，返回是否**成功发起**。
///
/// 走 `ShellExecuteW` 的 `"open"` 动词——与风之 UI 打开链接（`EventCtx::open_url`）是同一条
/// 通路，交给系统按"文件夹"的默认处理方式打开。不自己拼 `explorer.exe` 命令行：那条路有
/// 三种方言要处理（路径含空格要引号、`/select,` 的逗号、结尾反斜杠会退化成"我的文档"），
/// 而且 `Command::spawn` 成功 ≠ 资源管理器真的定位到了那个目录。
///
/// # 两处必须先做的准备
///
/// - **先建目录**：`temp/` 是"按需创建"的（见 `core::paths::module_temp_dir`），用户没截过图
///   时它根本不存在，直接打开会落到别处；建不出来也照样试一次（可能已存在但只是被拒）。
/// - **先绝对化**：`ShellExecute` 的相对路径按**当前工作目录**解析，而便携版从资源管理器
///   双击、被自启动拉起、被更新器重启时 CWD 各不相同（见 `core::paths` 头注释）。用
///   `current_dir` 手工拼而不是 `fs::canonicalize`——后者在 Windows 上会返回带 `\\?\`
///   前缀的扩展长度路径，`ShellExecuteW` 对它的处理是另一套规则。
///
/// 失败只记日志、返回 `false`（面板那张卡上还有路径文本，用户照着手动找得到）。
pub fn open_in_explorer(dir: &std::path::Path) -> bool {
    use windows::core::{w, PCWSTR};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let _ = std::fs::create_dir_all(dir);
    let abs = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(dir)
    };
    let wide: Vec<u16> = abs
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` 在调用期间存活（局部变量），`PCWSTR` 只借用它的首地址。
    let r = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // 文档约定：`ShellExecuteW` 返回值 ≤ 32 即失败（与 `platform::elevate` 同一判据）。
    let ok = r.0 as usize > 32;
    if !ok {
        log::warn!(
            "打开目录失败（ShellExecuteW 返回 {}）：{}",
            r.0 as usize,
            abs.display()
        );
    }
    ok
}

/// 用系统默认方式打开一个**文档文件**（区别于 [`open_in_explorer`] 的"目录"语义）。
///
/// - `.html` / `.htm` → `ShellExecuteW` 的 `"open"` 动词（系统默认浏览器）；
/// - 其余（`.md` 等纯文本）→ 显式走 `notepad.exe`：`.md` 在多数系统上没有文件关联，
///   直接 "open" 会弹"选择打开方式"，体验差；记事本是 Windows 永远在的兜底。
///
/// 与 [`open_in_explorer`] 同一套判据与日志（返回值 ≤ 32 = 失败）。路径同样先绝对化。
pub fn open_doc(path: &std::path::Path) -> bool {
    use windows::core::{w, PCWSTR};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let wide: Vec<u16> = abs
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let ext = abs
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let is_web = matches!(ext.as_str(), "html" | "htm");
    let (file, params): (PCWSTR, PCWSTR) = if is_web {
        (PCWSTR(wide.as_ptr()), PCWSTR::null())
    } else {
        (w!("notepad.exe"), PCWSTR(wide.as_ptr()))
    };
    // SAFETY: `wide` 在调用期间存活（局部变量），`PCWSTR` 只借用它的首地址。
    let r = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            file,
            params,
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    let ok = r.0 as usize > 32;
    if !ok {
        log::warn!(
            "打开文档失败（ShellExecuteW 返回 {}）：{}",
            r.0 as usize,
            abs.display()
        );
    }
    ok
}

pub fn menu_new() -> MenuHandle {
    use windows::Win32::UI::WindowsAndMessaging::CreatePopupMenu;
    unsafe { CreatePopupMenu().map(|m| m.0 as isize).unwrap_or(0) }
}

/// 给菜单追加一个**普通项**（`MF_STRING`）。
pub fn menu_append(menu: MenuHandle, id: i32, text: &str) -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{AppendMenuW, HMENU, MF_STRING};
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    wide.push(0);
    unsafe {
        AppendMenuW(
            HMENU(menu as *mut _),
            MF_STRING,
            id as usize,
            PCWSTR(wide.as_ptr()),
        )
        .is_ok()
    }
}

/// 给菜单追加一个**带勾选的普通项**（`MF_STRING | MF_CHECKED`）。
///
/// 「锁定图片」用：菜单打开时按当前锁定态勾选/去勾，点一次即切换。
pub fn menu_append_checked(menu: MenuHandle, id: i32, text: &str) -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{AppendMenuW, HMENU, MF_CHECKED, MF_STRING};
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    wide.push(0);
    unsafe {
        AppendMenuW(
            HMENU(menu as *mut _),
            MF_STRING | MF_CHECKED,
            id as usize,
            PCWSTR(wide.as_ptr()),
        )
        .is_ok()
    }
}

/// 追加一条**分隔线**（`MF_SEPARATOR`）。
/// 追加一条**禁用（置灰）占位项**（`MF_STRING | MF_GRAYED`）。
///
/// 「图片操作」子菜单一个脚本都没有时用：子菜单本身要恒在（2026-09-26 晚用户口径），
/// 但空的子菜单弹出来是一片白，看着像坏了——放一条灰色说明项，把“这里本该有东西、
/// 现在没有”讲清楚。命令号恒为 0（不可点，`TrackPopupMenu` 不会返回 0 号命令）。
pub fn menu_append_disabled(menu: MenuHandle, text: &str) -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{AppendMenuW, HMENU, MF_GRAYED, MF_STRING};
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    wide.push(0);
    unsafe {
        AppendMenuW(
            HMENU(menu as *mut _),
            MF_STRING | MF_GRAYED,
            0,
            PCWSTR(wide.as_ptr()),
        )
        .is_ok()
    }
}

pub fn menu_separator(menu: MenuHandle) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{AppendMenuW, HMENU, MF_SEPARATOR};
    unsafe {
        AppendMenuW(
            HMENU(menu as *mut _),
            MF_SEPARATOR,
            0,
            windows::core::PCWSTR::null(),
        )
        .is_ok()
    }
}

/// 建一个**子菜单**并挂到父菜单下（`MF_POPUP`）。返回子菜单句柄（后续向其追加项）。
pub fn menu_submenu(parent: MenuHandle, text: &str) -> MenuHandle {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{AppendMenuW, HMENU, MF_POPUP};
    // 先建一个真正的弹出菜单作子体，再以弹出项挂到父菜单上。
    let sub = menu_new();
    if sub == 0 {
        return 0;
    }
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    wide.push(0);
    unsafe {
        let _ = AppendMenuW(
            HMENU(parent as *mut _),
            MF_POPUP,
            sub as usize,
            PCWSTR(wide.as_ptr()),
        );
    }
    sub
}

/// 弹出菜单并同步等待用户选择（`TPM_RETURNCMD`，阻塞于嵌套消息泵）。返回被点中的命令 id。
///
/// ★ 弹之前必须把属主设为**前台**（2026-09-26 用户报：贴图右键菜单被贴图窗盖住，
/// "取消总在最上也没用"）。`TrackPopupMenu` 的菜单窗口与**非前台的 TOPMOST 属主**之间
/// 的 z 序是未定义的——贴图窗恒为 `HWND_TOPMOST`（见 `keep_topmost`），菜单可能整层
/// 被压在下面，子菜单一开、z 序一动才暴露出来。经典修法（MSDN KB135788）三步走：
/// `SetForegroundWindow(属主)` → `TrackPopupMenu` → `PostMessage(WM_NULL)`。
/// 与 [`activate_window_by_title`] 同款，后台进程改前台先敲一次 ALT 抬起解除前台锁。
pub fn menu_track(menu: MenuHandle, owner_hwnd: isize, x: i32, y: i32) -> Option<i32> {
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYEVENTF_KEYUP, VK_MENU};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongW, PostMessageW, SetForegroundWindow, SetWindowPos, TrackPopupMenu,
        GWL_EXSTYLE, HMENU, HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
        TPM_LEFTBUTTON, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_NULL, WS_EX_TOPMOST,
    };
    if menu == 0 {
        return None;
    }
    let hwnd = HWND(owner_hwnd as *mut _);
    let cmd;
    unsafe {
        // 前台锁解除（系统规则：ALT 抬起后允许后台进程改前台）+ 设前台。
        keybd_event(VK_MENU.0 as u8, 0, KEYEVENTF_KEYUP, 0);
        let _ = SetForegroundWindow(hwnd);
        // ★ 保险带（2026-09-26）：属主若是置顶窗（贴图窗恒 `HWND_TOPMOST`），弹菜单期间
        //   **摘出置顶带**，关菜单再放回去。菜单窗口与 TOPMOST 窗口同处置顶带，带内
        //   z 序按"最近抬升者在上"——任何环节让属主重新抬一次（点击路径、守护、时序
        //   抖动），它就压住菜单。摘出来后属主在普通档，菜单永远在它上面，层级之争
        //   从根上消失。仅在属主确实是置顶窗时动作，避免把普通窗误送进置顶带。
        let was_topmost = (GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0) != 0;
        if was_topmost {
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_NOTOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
        cmd = TrackPopupMenu(
            HMENU(menu as *mut _),
            TPM_LEFTBUTTON | TPM_RIGHTBUTTON | TPM_RETURNCMD,
            x,
            y,
            Some(0),
            hwnd,
            None,
        );
        if was_topmost {
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
        // 收尾半步：把属主从"模态菜单前台"态捞出来，否则它可能一直攥着激活态，
        // 影响下一次弹菜单 / 点别处的前台判定（KB135788）。
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
    }
    if cmd.0 == 0 {
        None
    } else {
        Some(cmd.0)
    }
}

/// 释放菜单资源。
pub fn menu_destroy(menu: MenuHandle) {
    use windows::Win32::UI::WindowsAndMessaging::{DestroyMenu, HMENU};
    if menu != 0 {
        unsafe {
            let _ = DestroyMenu(HMENU(menu as *mut _));
        }
    }
}

/// 「确定」按钮的控制 ID（`text_result_box` 的弹窗里）。
const TB_BTN_OK: i32 = 1;
/// 多行文本框的控制 ID（`text_result_box` 的弹窗里）。
const TB_EDIT: i32 = 2;
/// `Esc` 的虚拟键码（`WM_KEYDOWN` 用）。
const VK_ESCAPE: usize = 0x1B;

/// 可选中复制的只读多行文本框小窗（QR 结果用）。
///
/// ★ 为什么不用 `MessageBoxW`：它的文本**不能选中也不能复制**，用户明确要求结果要能复制
///   （"至少不是当前的这种对话框，连鼠标选中复制都做不到"）。这里用原生控件搭一个小窗：
///   `ES_MULTILINE | ES_READONLY` 的编辑框（鼠标圈选 + Ctrl+C / Ctrl+A）+「确定」按钮，
///   自带嵌套消息泵——与 [`menu_track`] 同类，可在延后任务里阻塞跑。
///
/// ★ 刻意**不** `PostQuitMessage`：那会把 `WM_QUIT` 泄漏进外围 windui 的消息泵、导致整个
///   应用退出。窗口销毁后靠 `IsWindow` 判活来结束本循环即可。
pub fn text_result_box(owner_hwnd: isize, title: &str, body: &str) {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{GetStockObject, DEFAULT_GUI_FONT};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DispatchMessageW, GetClientRect, GetDlgItem, GetMessageW, IsWindow,
        MoveWindow, RegisterClassW, SendMessageW, SetWindowTextW, TranslateMessage,
        BS_DEFPUSHBUTTON, CS_HREDRAW, CS_VREDRAW, ES_AUTOVSCROLL, ES_LEFT, ES_MULTILINE,
        ES_READONLY, HMENU, MSG, WINDOW_STYLE, WM_SETFONT, WNDCLASSW, WNDCLASS_STYLES, WS_BORDER,
        WS_CHILD, WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
    };

    // ★ Win32 多行 EDIT 只认 \r\n 当换行（裸 \n 会被吞成一行）：调用方与 Lua 脚本
    // 都写自然的 \n，这里统一归一化（先消 \r\n 再补，避免变成 \r\r\n）。
    // 窗口尺寸估算（下方 body.lines()）两种行尾都能切，先归一化再算更准。
    let body = body.replace("\r\n", "\n").replace('\n', "\r\n");

    // 窗口类只注册一次（类名被系统复制，局部 buffer 注册后即弃）。
    static REG_CLASS: std::sync::Once = std::sync::Once::new();
    REG_CLASS.call_once(|| unsafe {
        let cname: Vec<u16> = "CurSeen.TextResult\0".encode_utf16().collect();
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSW {
            style: WNDCLASS_STYLES(CS_HREDRAW.0 | CS_VREDRAW.0),
            lpfnWndProc: Some(text_result_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst.into(),
            hIcon: Default::default(),
            hCursor: Default::default(),
            hbrBackground: Default::default(),
            lpszMenuName: PCWSTR::null(),
            lpszClassName: PCWSTR(cname.as_ptr()),
        };
        let _ = RegisterClassW(&wc);
    });

    // 估算窗口尺寸：CJK 一字≈16px、ASCII≈9px，行高 18px；钳在虚拟屏内。
    let maxw: usize = body
        .lines()
        .map(|ln| {
            ln.chars()
                .map(|c| if c as u32 >= 0x3000 { 16 } else { 9 })
                .sum()
        })
        .max()
        .unwrap_or(40);
    let nlines = body.lines().count().max(1);
    let (scr_w, scr_h) = {
        let (_x, _y, rw, rh) = virtual_screen_rect();
        (rw.max(1), rh.max(1))
    };
    let ew = (maxw + 32)
        .min((scr_w as usize).saturating_sub(80))
        .max(NATIVE_DIALOG_W as usize) as i32;
    let eh = (nlines * 18 + 8)
        .min((scr_h as usize).saturating_sub(120))
        .max(80) as i32;
    let (ww, wh) = (ew + 40, eh + 44 + 34); // 外边：+40 低边距、+44 按钮条、+34 标题栏

    let title_w: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let body_w: Vec<u16> = body.encode_utf16().chain(std::iter::once(0)).collect();
    let class_w: Vec<u16> = "CurSeen.TextResult\0".encode_utf16().collect();
    let edit_cls: Vec<u16> = "EDIT\0".encode_utf16().collect();
    let btn_cls: Vec<u16> = "BUTTON\0".encode_utf16().collect();
    let ok_w: Vec<u16> = "确定\0".encode_utf16().collect();
    let owner = if owner_hwnd == 0 {
        None
    } else {
        Some(HWND(owner_hwnd as *mut _))
    };

    let hwnd = unsafe {
        CreateWindowExW(
            Default::default(),
            PCWSTR(class_w.as_ptr()),
            PCWSTR(title_w.as_ptr()),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            120,
            120,
            ww,
            wh, // 初值；下方按 owner 平移并依 GetClientRect 布局
            owner,
            None,
            None,
            None,
        )
    }
    .unwrap_or_else(|_| HWND::default());
    if hwnd.0.is_null() {
        return;
    }

    // 多行只读文本框 +「确定」按钮（默认 GUI 字体，保证中文正常渲染）。
    let edit_style = WINDOW_STYLE(
        WS_CHILD.0
            | WS_VISIBLE.0
            | WS_VSCROLL.0
            | WS_BORDER.0
            | (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL | ES_LEFT) as u32,
    );
    let btn_style =
        WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | (BS_DEFPUSHBUTTON as u32));
    let font = unsafe { GetStockObject(DEFAULT_GUI_FONT) };
    unsafe {
        if let Ok(edit) = CreateWindowExW(
            Default::default(),
            PCWSTR(edit_cls.as_ptr()),
            PCWSTR(body_w.as_ptr()),
            edit_style,
            0,
            0,
            0,
            0,
            Some(hwnd),
            Some(HMENU(TB_EDIT as usize as *mut _)),
            None,
            None,
        ) {
            let _ = SendMessageW(
                edit,
                WM_SETFONT,
                Some(WPARAM(font.0 as usize)),
                Some(LPARAM(1)),
            );
        }
        if let Ok(btn) = CreateWindowExW(
            Default::default(),
            PCWSTR(btn_cls.as_ptr()),
            PCWSTR(ok_w.as_ptr()),
            btn_style,
            0,
            0,
            0,
            0,
            Some(hwnd),
            Some(HMENU(TB_BTN_OK as usize as *mut _)),
            None,
            None,
        ) {
            let _ = SendMessageW(
                btn,
                WM_SETFONT,
                Some(WPARAM(font.0 as usize)),
                Some(LPARAM(1)),
            );
        }
    }

    // 依真实客户区布局：编辑框填满去掉按钮条；按钮在右下角。
    unsafe {
        let mut rc = RECT::default();
        if GetClientRect(hwnd, &mut rc).is_ok() {
            let cw = rc.right;
            let ch = rc.bottom;
            let btnw = 84i32;
            let btnh = 28i32;
            if let Ok(edit) = GetDlgItem(Some(hwnd), TB_EDIT) {
                let _ = MoveWindow(edit, 8, 8, cw - 16, ch - 8 - 44, true);
            }
            if let Ok(btn) = GetDlgItem(Some(hwnd), TB_BTN_OK) {
                let _ = MoveWindow(btn, cw - 8 - btnw, ch - 8 - btnh, btnw, btnh, true);
            }
        }
    }
    unsafe {
        let _ = SetWindowTextW(hwnd, PCWSTR(title_w.as_ptr()));
    }

    // 嵌套消息泵：窗口销毁（IsWindow 判活）即结束。不 PostQuitMessage，避免泄 WM_QUIT。
    let mut msg = MSG::default();
    loop {
        let ok = unsafe { GetMessageW(&mut msg, None, 0, 0).as_bool() };
        if !ok {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            let _ = DispatchMessageW(&msg);
        }
        if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            break;
        }
    }
}

/// `text_result_box` 的窗口过程：`确定`按钮 / Esc / 标题栏关闭都走 `DestroyWindow`，
/// `WM_DESTROY` 后由消息循环的 `IsWindow` 判活结尾。不处理 `WM_DESTROY` -> 不 `PostQuitMessage`。
unsafe extern "system" fn text_result_proc(
    hwnd: windows::Win32::Foundation::HWND,
    msg: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging as m;
    match msg {
        // WM_COMMAND = 0x0111：「确定」按钮。
        0x0111 => {
            let id = wparam.0 & 0xffff;
            if id as i32 == TB_BTN_OK {
                let _ = m::DestroyWindow(hwnd);
                return lresult_0();
            }
        }
        // WM_KEYDOWN = 0x0100：Esc 关闭。
        0x0100 => {
            if wparam.0 == VK_ESCAPE {
                let _ = m::DestroyWindow(hwnd);
                return lresult_0();
            }
        }
        // WM_CLOSE = 0x0010：标题栏 X / Alt+F4。
        0x0010 => {
            let _ = m::DestroyWindow(hwnd);
            return lresult_0();
        }
        // WM_DESTROY = 0x0002：静默（循环靠 IsWindow 判活，不 PostQuitMessage）。
        0x0002 => return lresult_0(),
        _ => {}
    }
    m::DefWindowProcW(hwnd, msg, wparam, lparam)
}

#[inline]
fn lresult_0() -> windows::Win32::Foundation::LRESULT {
    windows::Win32::Foundation::LRESULT(0)
}

// ─── 输入框（text_input_box）────────────────────────────────────────────
/// 「确定」按钮 ID（`text_input_box` 的弹窗里）。
const TI_BTN_OK: i32 = 1;
/// 输入编辑框 ID。
const TI_EDIT: i32 = 2;
/// 「取消」按钮 ID。
const TI_BTN_CANCEL: i32 = 3;

// 本线程最近一次 `text_input_box` 的结果（`None` = 取消 / 关闭）。
// 调用线程独占（弹窗自带消息泵、阻塞至关闭），thread_local 足够。
thread_local! {
    static TI_RESULT: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// 可编辑的多行输入框小窗：AI 对话等脚本交互用。返回 `Some(输入文本)` / `None`（取消）。
///
/// 与 [`text_result_box`] 同一族：原生控件 + 自带嵌套消息泵，可在后台线程阻塞跑；
/// 「确定」= 回填结果关窗，`取消` / Esc / 标题栏 X = 关窗返回 `None`。不 `PostQuitMessage`。
pub fn text_input_box(owner_hwnd: isize, title: &str, default_text: &str) -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{GetStockObject, DEFAULT_GUI_FONT};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DispatchMessageW, GetClientRect, GetDlgItem, GetMessageW, IsWindow,
        MoveWindow, RegisterClassW, SendMessageW, SetWindowTextW, TranslateMessage,
        BS_DEFPUSHBUTTON, CS_HREDRAW, CS_VREDRAW, ES_AUTOVSCROLL, ES_LEFT, ES_MULTILINE, HMENU,
        MSG, WINDOW_STYLE, WM_SETFONT, WNDCLASSW, WNDCLASS_STYLES, WS_BORDER, WS_CHILD,
        WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
    };

    TI_RESULT.with(|r| *r.borrow_mut() = None);

    static REG_CLASS: std::sync::Once = std::sync::Once::new();
    REG_CLASS.call_once(|| unsafe {
        let cname: Vec<u16> = "CurSeen.TextInput\0".encode_utf16().collect();
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSW {
            style: WNDCLASS_STYLES(CS_HREDRAW.0 | CS_VREDRAW.0),
            lpfnWndProc: Some(text_input_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst.into(),
            hIcon: Default::default(),
            hCursor: Default::default(),
            hbrBackground: Default::default(),
            lpszMenuName: PCWSTR::null(),
            lpszClassName: PCWSTR(cname.as_ptr()),
        };
        let _ = RegisterClassW(&wc);
    });

    // 固定一个顺手的尺寸（输入框不像结果框要按内容伸缩），钳进虚拟屏。
    // 宽度对齐面板快捷键框（NATIVE_DIALOG_W = CONTROL_GROUP_W），2026-09-29「宽度统一」。
    let (ww, wh) = (NATIVE_DIALOG_W, 240i32);
    let (scr_w, scr_h) = {
        let (_x, _y, rw, rh) = virtual_screen_rect();
        (rw.max(1), rh.max(1))
    };
    let ww = ww.min(scr_w - 40);
    let wh = wh.min(scr_h - 80);

    let title_w: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let text_w: Vec<u16> = default_text
        .replace("\r\n", "\n")
        .replace('\n', "\r\n")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let class_w: Vec<u16> = "CurSeen.TextInput\0".encode_utf16().collect();
    let edit_cls: Vec<u16> = "EDIT\0".encode_utf16().collect();
    let btn_cls: Vec<u16> = "BUTTON\0".encode_utf16().collect();
    let ok_w: Vec<u16> = "确定\0".encode_utf16().collect();
    let cancel_w: Vec<u16> = "取消\0".encode_utf16().collect();
    let owner = (owner_hwnd != 0).then_some(HWND(owner_hwnd as *mut _));

    let hwnd = unsafe {
        CreateWindowExW(
            Default::default(),
            PCWSTR(class_w.as_ptr()),
            PCWSTR(title_w.as_ptr()),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            160,
            160,
            ww,
            wh,
            owner,
            None,
            None,
            None,
        )
    }
    .unwrap_or_else(|_| HWND::default());
    if hwnd.0.is_null() {
        return None;
    }

    let edit_style = WINDOW_STYLE(
        WS_CHILD.0
            | WS_VISIBLE.0
            | WS_VSCROLL.0
            | WS_BORDER.0
            | WS_TABSTOP.0
            | (ES_MULTILINE | ES_AUTOVSCROLL | ES_LEFT) as u32,
    );
    let btn_style =
        WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | (BS_DEFPUSHBUTTON as u32));
    let font = unsafe { GetStockObject(DEFAULT_GUI_FONT) };
    unsafe {
        if let Ok(edit) = CreateWindowExW(
            Default::default(),
            PCWSTR(edit_cls.as_ptr()),
            PCWSTR(text_w.as_ptr()),
            edit_style,
            0,
            0,
            0,
            0,
            Some(hwnd),
            Some(HMENU(TI_EDIT as usize as *mut _)),
            None,
            None,
        ) {
            let _ = SendMessageW(
                edit,
                WM_SETFONT,
                Some(WPARAM(font.0 as usize)),
                Some(LPARAM(1)),
            );
        }
        for (id, text, x_off) in [
            (TI_BTN_OK, ok_w, 0usize),
            (TI_BTN_CANCEL, cancel_w, 96usize),
        ] {
            if let Ok(btn) = CreateWindowExW(
                Default::default(),
                PCWSTR(btn_cls.as_ptr()),
                PCWSTR(text.as_ptr()),
                btn_style,
                0,
                0,
                0,
                0,
                Some(hwnd),
                Some(HMENU(id as usize as *mut _)),
                None,
                None,
            ) {
                let _ = SendMessageW(
                    btn,
                    WM_SETFONT,
                    Some(WPARAM(font.0 as usize)),
                    Some(LPARAM(1)),
                );
                let _ = x_off;
            }
        }
        let mut rc = RECT::default();
        if GetClientRect(hwnd, &mut rc).is_ok() {
            let (cw, ch) = (rc.right, rc.bottom);
            let (btnw, btnh) = (84i32, 28i32);
            if let Ok(edit) = GetDlgItem(Some(hwnd), TI_EDIT) {
                let _ = MoveWindow(edit, 8, 8, cw - 16, ch - 8 - 44, true);
            }
            if let Ok(ok) = GetDlgItem(Some(hwnd), TI_BTN_OK) {
                let _ = MoveWindow(ok, cw - 8 - btnw * 2 - 8, ch - 8 - btnh, btnw, btnh, true);
            }
            if let Ok(cancel) = GetDlgItem(Some(hwnd), TI_BTN_CANCEL) {
                let _ = MoveWindow(cancel, cw - 8 - btnw, ch - 8 - btnh, btnw, btnh, true);
            }
        }
        let _ = SetWindowTextW(hwnd, PCWSTR(title_w.as_ptr()));
    }

    // 嵌套消息泵：窗口销毁即结束（结果已由 wndproc 在「确定」时回填）。
    let mut msg = MSG::default();
    loop {
        let ok = unsafe { GetMessageW(&mut msg, None, 0, 0).as_bool() };
        if !ok {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            let _ = DispatchMessageW(&msg);
        }
        if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            break;
        }
    }
    TI_RESULT.with(|r| r.borrow().clone())
}

/// `text_input_box` 的窗口过程：确定 = 读入编辑框文本回填结果；取消 / Esc / X = 不回填。
unsafe extern "system" fn text_input_proc(
    hwnd: windows::Win32::Foundation::HWND,
    msg: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging as m;
    // WM_COMMAND：按钮（LOWORD = 控件 ID）。
    if msg == 0x0111 {
        let id = (wparam.0 & 0xffff) as i32;
        if id == TI_BTN_OK {
            let text = read_edit_text(hwnd);
            TI_RESULT.with(|r| *r.borrow_mut() = Some(text));
            let _ = m::DestroyWindow(hwnd);
            return lresult_0();
        }
        if id == TI_BTN_CANCEL {
            let _ = m::DestroyWindow(hwnd);
            return lresult_0();
        }
        return lresult_0();
    }
    match msg {
        // WM_KEYDOWN：Esc = 取消。Enter 默认交给 EDIT 换行（多行输入）。
        0x0100 if wparam.0 == VK_ESCAPE => {
            let _ = m::DestroyWindow(hwnd);
            lresult_0()
        }
        0x0010 => {
            let _ = m::DestroyWindow(hwnd);
            lresult_0()
        }
        0x0002 => lresult_0(),
        _ => m::DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// 读 `text_input_box` 编辑框全文（`GetWindowTextLengthW` 先量长再取，UTF-16 → String）。
fn read_edit_text(hwnd: windows::Win32::Foundation::HWND) -> String {
    use windows::Win32::UI::WindowsAndMessaging::{GetDlgItem, GetWindowTextLengthW};
    unsafe {
        let Ok(edit) = GetDlgItem(Some(hwnd), TI_EDIT) else {
            return String::new();
        };
        let len = GetWindowTextLengthW(edit).max(0) as usize;
        if len == 0 {
            return String::new();
        }
        let mut buf = vec![0u16; len + 1];
        let n = windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(edit, &mut buf);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}

// ─── 便签（sticky）用到的原生小工具 ───

/// ARGB `[A,R,G,B]` → Win32 `COLORREF`（`0x00BBGGRR`，GDI 蓝绿红序）。
pub fn colorref(c: [u8; 4]) -> COLORREF {
    COLORREF(((c[2] as u32) << 16) | ((c[1] as u32) << 8) | c[0] as u32)
}

/// 把已知 HWND 的顶层窗带前台并聚焦。
///
/// 便签新建后要立刻可打字，需要拿键盘焦点。后台线程直接 `SetForegroundWindow` 会被
/// 前台锁拒绝（"系统规则：只有前台进程才能换前台"），实测**只按一次 ALT 解锁还不够**
/// —— 提权探针里焦点依然停在调用方。所以这里用经典的三步组合：
///   ① `AttachThreadInput(自己, 当前前台线程, TRUE)` —— 把两个线程的输入队列临时接到
///      一起，这是绕过前台锁唯一可靠的办法；
///   ② `SetForegroundWindow` + `SetFocus`；
///   ③ `AttachThreadInput(..., FALSE)` 解开。
/// ALT 按下/抬起那一下保留（部分系统版本仍需要）。失败不致命：用户手动点一下窗口
/// 也会激活（`WM_ACTIVATE` 会把焦点递给 EDIT）。
pub fn focus_window_handle(hwnd: HWND) {
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        keybd_event, SetFocus, KEYEVENTF_KEYUP, VK_MENU,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow,
    };
    if hwnd.is_invalid() {
        return;
    }
    unsafe {
        // ALT 按下/抬起两次：解锁"后台进程不能换前台"的限制（系统规则）。
        keybd_event(VK_MENU.0 as u8, 0, KEYEVENTF_KEYUP, 0);
        keybd_event(VK_MENU.0 as u8, 0, KEYEVENTF_KEYUP, 0);

        let self_tid = GetCurrentThreadId();
        let fg = GetForegroundWindow();
        let fg_tid = if fg.is_invalid() {
            0
        } else {
            GetWindowThreadProcessId(fg, None)
        };
        // 前台线程和自己不同时才需要接（同线程 Attach 无意义）。
        let attached = fg_tid != 0 && fg_tid != self_tid && {
            AttachThreadInput(self_tid, fg_tid, true).as_bool()
        };
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
        if attached {
            let _ = AttachThreadInput(self_tid, fg_tid, false);
        }
    }
}

/// 目录 fsync：把"文件改名"这次**目录项**的修改真正刷到盘上。
///
/// 为什么需要它（总纲 §7.2）：`fs::rename` 是原子的，但**目录项本身也躺在缓存里**。
/// 断电时可能落到"文件数据写进去了、目录项没更新"——结果是配置回到上一版
/// （不是损坏，但用户以为刚保存过）。
///
/// ★ 两个 Windows 专属的坑（Win10/11 实测，别凭印象改）：
///   ① `std::fs::File::open` **打不开目录**（ACCESS_DENIED），必须
///      `CreateFileW` + `FILE_FLAG_BACKUP_SEMANTICS`；
///   ② 目录句柄必须以 `GENERIC_WRITE` 打开——`FlushFileBuffers` 要写权限，
///      用 `GENERIC_READ` 打开再 `sync_all` 会被直接拒绝（os error 5）。
///
/// 失败只返回 `false`，**不返回错误**：这一步是锦上添花，受限账户 / U 盘 / 网络盘上
/// 常被拒，绝不能让它反过来毁掉整条保存路径（`Config::save` 里失败只告警）。
pub fn sync_dir(dir: &std::path::Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::GENERIC_WRITE;
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    let wide: Vec<u16> = dir
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` 是本函数栈上的、以 NUL 结尾的 UTF-16 缓冲，在整个 `CreateFileW`
    // 调用期间活着；`PCWSTR` 只是把地址传给系统读一下，不取得所有权、不延长生命周期。
    let handle = unsafe {
        CreateFileW(
            PCWSTR::from_raw(wide.as_ptr()),
            GENERIC_WRITE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    };
    let handle = match handle {
        Ok(h) => h,
        Err(_) => return false,
    };
    // SAFETY: `CreateFileW` 成功返回的句柄（windows-rs 的 `Result` 已替我们排除了
    // `INVALID_HANDLE_VALUE`）归我们所有；交给 `File::from_raw_handle` 后由 `File`
    // 的 `Drop` 关闭，既不泄漏也不会二次关闭。
    let file = unsafe { std::fs::File::from_raw_handle(handle.0) };
    file.sync_all().is_ok()
}

/// 把「这次启动的环境」写进日志。
///
/// 为什么要有它（总纲 §10）：桌面软件没有运维，**用户能给你的只有一份日志**。缺了
/// 版本 / OS / 显示器 / 权限级别这四项，一半的「在我机器上好的」问题根本无从下手——
/// 而它的成本只是启动时的一次性几行。
///
/// ★ 启动参数照原样记：日志落在用户自己的程序目录里，而路径恰恰是最高频的诊断线索。
pub fn log_env_banner() {
    let ver = env!("CARGO_PKG_VERSION");
    let hash = option_env!("CURSEEN_GIT_HASH").unwrap_or("unknown");
    let os = os_version_summary();
    let monitors = monitor_summary();
    let level = if crate::platform::elevate::is_elevated() {
        "高（已提权）"
    } else {
        "中（普通权限）"
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args = if args.is_empty() {
        "（无）".to_string()
    } else {
        args.join(" ")
    };
    log::info!(
        "启动环境 | 版本 v{ver} (git {hash}) | 系统 {os} | 显示器 {monitors} \
         | 完整性级别 {level} | 启动参数 {args}"
    );
}

/// `HKLM\...\CurrentVersion` 下的一个字符串值。
///
/// 用注册表而不是 `GetVersionExW`：后者在**没有 manifest 声明支持的进程里会撒谎**
/// （Win11 上照样报 6.2），而本 exe 只嵌入图标与版本资源，没有 manifest。
fn reg_sz(subkey: &str, value: &str) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};

    let sub: Vec<u16> = std::ffi::OsStr::new(subkey)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let val: Vec<u16> = std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut buf = [0u16; 128];
    let mut len = (buf.len() * 2) as u32;
    let code = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            // SAFETY: `sub` / `val` 是本函数栈上以 NUL 结尾的 UTF-16 缓冲，调用期间活着，
            // `PCWSTR` 只借地址不取得所有权；`len` 以**字节**写入（`buf.len() * 2`），
            // 与 `RegGetValueW` 的契约一致，不会写越界。
            PCWSTR::from_raw(sub.as_ptr()),
            PCWSTR::from_raw(val.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut std::ffi::c_void),
            Some(&mut len),
        )
    };
    if code.0 != 0 {
        return None;
    }
    // `len` 是字节数且含结尾 NUL；REG_SZ 也可能不带 NUL，故按 min 裁一遍。
    let n = (len as usize / 2).min(buf.len());
    Some(
        String::from_utf16_lossy(&buf[..n])
            .trim_end_matches('\0')
            .to_string(),
    )
}

/// 「Windows 10 Pro 25H2 (build 26200.xxxx)」这类一句话摘要；取不到就如实说未知。
///
/// ★ 实测（2026-10-24，本机）：`ProductName` 在 25H2 上仍写着 **"Windows 10 Pro"**
///   ——微软没改这个值的字面量。所以**不要**拿它判断是否 Win11：真正的判别依据是
///   `CurrentBuild >= 22000`（Win11 起）+ `DisplayVersion`。摘要里两项都带上，
///   就是为了让人自己去对号，而不是被 ProductName 误导。
fn os_version_summary() -> String {
    const KEY: &str = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion";
    let name = reg_sz(KEY, "ProductName").unwrap_or_else(|| "未知版本".to_string());
    let display = reg_sz(KEY, "DisplayVersion").unwrap_or_default();
    let build = reg_sz(KEY, "CurrentBuild").unwrap_or_default();
    let ubr = reg_sz(KEY, "UBR").unwrap_or_default();
    let mut s = name;
    let mut detail = String::new();
    if !display.is_empty() {
        detail.push_str(&display);
    }
    if !build.is_empty() {
        if !detail.is_empty() {
            detail.push(' ');
        }
        detail.push_str("(build ");
        detail.push_str(&build);
        if !ubr.is_empty() {
            detail.push('.');
            detail.push_str(&ubr);
        }
        detail.push(')');
    }
    if !detail.is_empty() {
        s.push(' ');
        s.push_str(&detail);
    }
    s
}

/// 「2 台，虚拟屏 3840x1080@(0,0)，主屏 125%」这类一句话摘要。
///
/// ★ DPI 走 `GetDpiForMonitor` 而不是 `GetDpiForSystem`：后者是**系统 DPI**（多屏不同
///   缩放时必错），本项目明令禁用它；这里问的又是"主屏"这一台，per-monitor 值才对得上。
fn monitor_summary() -> String {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTOPRIMARY};
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CMONITORS};

    let count = unsafe { GetSystemMetrics(SM_CMONITORS) };
    let (x, y, x2, y2) = virtual_screen_rect();
    let mut dpi = 96u32;
    unsafe {
        let m = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut dx = 0u32;
        let mut dy = 0u32;
        if GetDpiForMonitor(m, MDT_EFFECTIVE_DPI, &mut dx, &mut dy).is_ok() && dx > 0 {
            dpi = dx;
        }
    }
    format!(
        "{} 台，虚拟屏 {}x{}@({},{})，主屏 {}%",
        count,
        x2 - x,
        y2 - y,
        x,
        y,
        dpi * 100 / 96
    )
}
