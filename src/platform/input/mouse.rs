//! WH_MOUSE_LL 的回调处理。
//!
//! ⚠️ 回调运行在系统输入路径上：只做最小工作。

use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_RWIN};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, MSLLHOOKSTRUCT, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN,
    WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDBLCLK, WM_RBUTTONDOWN,
    WM_RBUTTONUP, WM_XBUTTONDOWN, WM_XBUTTONUP, XBUTTON1,
};

use super::hook::HOOK_TX;
use crate::core::events::{InputEvent, MouseButton};

/// 鼠标移动节流：阈值 4ms ≈ 250Hz，远高于 60fps 渲染需求
const MOVE_MIN_INTERVAL_MS: u64 = 4;

/// 上一次移动事件的时间戳
static LAST_MOVE_TIME: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 上一次移动坐标（用于去重）
static LAST_MOVE_X: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
static LAST_MOVE_Y: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// 「鼠标回显」是否**武装**（击键回显开着且要显示鼠标按键时由 UI 线程置位，
/// 见 [`set_mouse_echo_armed`]）。
///
/// 武装时钩子把**普通滚轮 / 横向倾斜滚轮**也镜像一份给 UI 线程（击键回显显示
/// 「滚轮上/下/左/右」）；未武装时不转发——省掉每滚一格就唤醒 UI 线程一帧的开销。
/// 与 `Runtime::sync_input_gate` 每帧同步。
static MOUSE_ECHO_ARMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// UI 线程按「击键回显开着 **且** 要显示鼠标按键」同步武装开关（`Runtime::sync_input_gate` 调用）。
pub fn set_mouse_echo_armed(on: bool) {
    MOUSE_ECHO_ARMED.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// 桥接线程 / 钩子读：武装中的滚轮要镜像到 UI 线程，MouseDown 要放行过桥（低频）。
pub fn mouse_echo_armed() -> bool {
    MOUSE_ECHO_ARMED.load(std::sync::atomic::Ordering::Relaxed)
}

/// 缩放滚轮是否**武装**（放大镜在屏上时由 UI 线程置位，见 [`set_zoom_wheel_armed`]）。
///
/// 「武装」= 鼠标钩子允许把 Ctrl+Win+滚轮 吞掉并转给 UI 线程做倍率步进；
/// 未武装（没有放大镜在屏）时滚轮完全不受干预——Ctrl+Win+滚轮 在系统里本就
/// 没有默认行为，等于零成本。与 `Runtime::sync_input_gate` 每帧同步。
static ZOOM_WHEEL_ARMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// UI 线程按「任一放大镜是否启用」同步武装开关（`Runtime::sync_input_gate` 调用）。
pub fn set_zoom_wheel_armed(on: bool) {
    ZOOM_WHEEL_ARMED.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// 缩放手势（Ctrl+Win）的**实时**修饰键判据。
///
/// 为什么不用 `keyboard::ctrl_held()/win_held()` 的缓存：见 WM_MOUSEWHEEL 分支里的
/// 注释——缓存会因按键丢失失步，实测"时灵时不灵"。`& 0x8000` 取按下沿（API 约定）。
fn zoom_mods_held() -> bool {
    unsafe {
        let ctrl = GetAsyncKeyState(VK_CONTROL.0 as i32);
        let win = GetAsyncKeyState(VK_LWIN.0 as i32) | GetAsyncKeyState(VK_RWIN.0 as i32);
        (ctrl & 0x8000_u16 as i16) != 0 && (win & 0x8000_u16 as i16) != 0
    }
}

/// 桥接线程读：武装中的缩放滚轮要放行到 UI 线程（其余滚轮本进程不需要）。
pub fn zoom_wheel_armed() -> bool {
    ZOOM_WHEEL_ARMED.load(std::sync::atomic::Ordering::Relaxed)
}

/// 特效脚本是否需要滚轮事件（`on_wheel`）。
///
/// 与 `ZOOM_WHEEL_ARMED` 同构：只有在**当前运行的脚本真的声明了 `on_wheel`** 时
/// 钩子才把普通滚轮镜像到 UI 线程——否则"每滚一格唤醒一帧"的开销白付。由
/// `Runtime::sync_input_gate` 每帧同步（复合/拖尾脚本对账后）。
static SCRIPT_WHEEL_ARMED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// UI 线程按「是否有脚本要滚轮」同步武装开关。
pub fn set_script_wheel_armed(on: bool) {
    SCRIPT_WHEEL_ARMED.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// 桥接线程 / 钩子读：脚本要滚轮时，普通滚轮事件也要过桥。
pub fn script_wheel_armed() -> bool {
    SCRIPT_WHEEL_ARMED.load(std::sync::atomic::Ordering::Relaxed)
}

/// 上一次左键按下是否被虚拟键盘吃掉。
///
/// 抬起要沿用**同一个**判断：按在键帽上、拖到键盘外再松开，那一下抬起同样不能漏给
/// 下面的窗口（否则下层会收到一个没有按下的抬起，某些应用据此判定状态）。
static LEFT_SWALLOWED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 鼠标钩子回调。
///
/// # Safety
/// 必须作为 WH_MOUSE_LL 的回调函数传入 SetWindowsHookExW。
pub unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return CallNextHookEx(None, code, wparam, lparam);
    }

    let ms = &*(lparam.0 as *const MSLLHOOKSTRUCT);
    let x = ms.pt.x;
    let y = ms.pt.y;
    let time_ms = ms.time as u64;
    let msg = wparam.0 as u32;

    // ★ 数位板直通（2026-09-28，三报根治）：笔驱动绘制期间，翻译层补发的迟到鼠标
    //   消息整段丢弃。笔的真实输入由覆盖层窗口的 `WM_POINTER` 直通喂进绘制
    //   （windui 补丁 093 + `overlay::pen_drive_drawing`），当帧就到；这份翻译副本
    //   要晚 ~0.5s 才到（Windows Ink 已知行为），放行就会一笔画两遍、还倒着画。
    //   吞掉（return 1）而非只不转发：标注中本来就该拦下穿透点击。
    //   三重门防误伤：`PEN_ACTIVE`（真消费过笔事件）+ 1.5s 新鲜度（笔的 Up 丢失时
    //   自动恢复放行，绝不把鼠标永久吞掉）。鼠标绘制时 `PEN_ACTIVE` 恒为 false。
    if crate::runtime::PEN_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
        let last = crate::runtime::PEN_LAST_MS.load(std::sync::atomic::Ordering::Relaxed);
        let now = crate::platform::win_util::tick_ms();
        if now.wrapping_sub(last) < 1500 {
            return match msg {
                WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_LBUTTONUP
                | WM_RBUTTONDOWN | WM_RBUTTONUP => LRESULT(1),
                _ => CallNextHookEx(None, code, wparam, lparam),
            };
        }
    }

    match msg {
        WM_MOUSEMOVE => {
            let last_t = LAST_MOVE_TIME.load(std::sync::atomic::Ordering::Relaxed);
            let last_x = LAST_MOVE_X.load(std::sync::atomic::Ordering::Relaxed);
            let last_y = LAST_MOVE_Y.load(std::sync::atomic::Ordering::Relaxed);
            if time_ms.wrapping_sub(last_t) < MOVE_MIN_INTERVAL_MS && last_x == x && last_y == y {
                return CallNextHookEx(None, code, wparam, lparam);
            }
            LAST_MOVE_TIME.store(time_ms, std::sync::atomic::Ordering::Relaxed);
            LAST_MOVE_X.store(x, std::sync::atomic::Ordering::Relaxed);
            LAST_MOVE_Y.store(y, std::sync::atomic::Ordering::Relaxed);
            if let Some(tx) = HOOK_TX.get() {
                let _ = tx.send(InputEvent::MouseMove { x, y, time_ms });
            }
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            // ★ 判据与 UI 线程的注入侧**同源**（都调 `hit::claim`）：吞了却不注入 =
            //   这次点击凭空消失；注入了却没吞 = 下层应用也收到一下（双触发）。
            //   `claim` 除了"命中键帽"，还要求落点**不压在我们自己的窗口上**——否则
            //   面板/取色器收不到点击。见 `hit::claim` 的说明。
            let claimed = super::hit::claim(x, y);
            let swallow = claimed.is_some();
            if swallow {
                log::info!("鼠标钩子：接管键帽点击 x={x} y={y} code={:?}", claimed);
            }
            LEFT_SWALLOWED.store(swallow, std::sync::atomic::Ordering::Relaxed);
            if let Some(tx) = HOOK_TX.get() {
                let _ = tx.send(InputEvent::MouseDown {
                    button: MouseButton::Left,
                    x,
                    y,
                    time_ms,
                });
            }
            if swallow {
                return LRESULT(1);
            }
        }
        WM_LBUTTONUP => {
            let swallow = LEFT_SWALLOWED.swap(false, std::sync::atomic::Ordering::Relaxed);
            if let Some(tx) = HOOK_TX.get() {
                let _ = tx.send(InputEvent::MouseUp {
                    button: MouseButton::Left,
                    x,
                    y,
                    time_ms,
                });
            }
            if swallow {
                return LRESULT(1);
            }
        }
        WM_RBUTTONDOWN | WM_RBUTTONDBLCLK => {
            if let Some(tx) = HOOK_TX.get() {
                let _ = tx.send(InputEvent::MouseDown {
                    button: MouseButton::Right,
                    x,
                    y,
                    time_ms,
                });
            }
        }
        WM_RBUTTONUP => {
            if let Some(tx) = HOOK_TX.get() {
                let _ = tx.send(InputEvent::MouseUp {
                    button: MouseButton::Right,
                    x,
                    y,
                    time_ms,
                });
            }
        }
        WM_MBUTTONDOWN => {
            if let Some(tx) = HOOK_TX.get() {
                let _ = tx.send(InputEvent::MouseDown {
                    button: MouseButton::Middle,
                    x,
                    y,
                    time_ms,
                });
            }
        }
        WM_MBUTTONUP => {
            if let Some(tx) = HOOK_TX.get() {
                let _ = tx.send(InputEvent::MouseUp {
                    button: MouseButton::Middle,
                    x,
                    y,
                    time_ms,
                });
            }
        }
        WM_MOUSEWHEEL => {
            let raw = ms.mouseData;
            let delta = ((raw >> 16) & 0xFFFF) as i16 as i32;
            // ★★ 放大镜缩放的**内置快捷键**（2026-09-21 用户拍板；刻意不设配置项、
            //    不在面板暴露，本注释即唯一"文档"）：
            //
            //        按住 Ctrl + Win，滚动滚轮 —— 上滚放大、下滚缩小（步进 ±1，范围 1–5）。
            //
            //    为什么是这一组：
            //    - `Ctrl+滚轮` 是浏览器 / Office / 资源管理器的事实标准缩放手势，
            //      必须避开（前两版分别试过 Ctrl+滚轮 与 Ctrl+Alt+键盘快捷键，均被否）；
            //    - 独立键盘快捷键没有鼠标参与、不顺手，且旧实现有"全屏方向恒为 0"
            //      的失效 bug（见 `runtime::Runtime::zoom_step` 的注释）；
            //    - `Ctrl+Win+滚轮` 在 Windows 与主流软件中均无默认绑定——2026-09-21
            //      本机 Win11 实录（合成注入 + 截图留档，temp/zoom_probe/）：
            //      纯 Win 轻敲会开开始菜单（对照实验），而 Ctrl+Win+滚轮 两种按压
            //      顺序（Ctrl 先 / Win 先）都**无任何系统行为**。另外 Windows 自带
            //      放大镜用 Ctrl+Alt+滚轮，刻意避开它（也绕开 AltGr=Ctrl+Alt 的
            //      键盘布局歧义）。
            //
            //    生效条件：放大镜（画中画或全屏）在屏上时才吞事件——本次滚轮转给
            //    UI 线程做倍率步进，下层应用收不到；放大镜关着时完全不干预。
            //    实现上分两道门：`ZOOM_WHEEL_ARMED`（放大镜在屏，UI 线程每帧同步）
            //    + Ctrl/Win 修饰键。
            //
            //    ★★ 修饰键必须用 **GetAsyncKeyState 实时读**，不能用键盘钩子维护的
            //      MODS 缓存（2026-09-26 用户报"时灵时不灵"）：缓存由 WH_KEYBOARD_LL
            //      逐键更新，键 Up 在安全桌面 / UAC 提权窗 / 其它全局钩子超时等场景下
            //      会**整段丢失**，缓存就与真实状态失步——症状恰好是"同一组合键有时
            //      判中有时判不中"。滚轮事件本身很低频（人手 ≤5 次/秒），这里两次
            //      GetAsyncKeyState（进入内核查询）完全无感；键盘回调里的"不调
            //      GetKeyState"硬约束管的是**每键一次**的回调，与此不冲突。
            if ZOOM_WHEEL_ARMED.load(std::sync::atomic::Ordering::Relaxed) && zoom_mods_held() {
                if let Some(tx) = HOOK_TX.get() {
                    let _ = tx.send(InputEvent::MouseWheel {
                        delta,
                        horizontal: false,
                        x,
                        y,
                        time_ms,
                    });
                }
                return LRESULT(1);
            }
            // 其余滚轮：回显武装或脚本要滚轮时镜像一份给 UI 线程（击键回显显示
            // 「滚轮上/下」/ Scroll 特效），否则一律不转发——不转发也省掉每滚一格
            // 就唤醒 UI 线程一帧的开销。
            if mouse_echo_armed() || script_wheel_armed() {
                if let Some(tx) = HOOK_TX.get() {
                    let _ = tx.send(InputEvent::MouseWheel {
                        delta,
                        horizontal: false,
                        x,
                        y,
                        time_ms,
                    });
                }
            }
        }
        WM_MOUSEHWHEEL => {
            // 横向倾斜滚轮：服务击键回显（「滚轮左/右」）与脚本；不参与放大镜缩放；
            // 不吞事件——倾斜滚动照常横移。
            let raw = ms.mouseData;
            let delta = ((raw >> 16) & 0xFFFF) as i16 as i32;
            if mouse_echo_armed() || script_wheel_armed() {
                if let Some(tx) = HOOK_TX.get() {
                    let _ = tx.send(InputEvent::MouseWheel {
                        delta,
                        horizontal: true,
                        x,
                        y,
                        time_ms,
                    });
                }
            }
        }
        WM_XBUTTONDOWN => {
            let raw = ms.mouseData;
            let btn = if (raw >> 16) & XBUTTON1 as u32 != 0 {
                MouseButton::X1
            } else {
                MouseButton::X2
            };
            if let Some(tx) = HOOK_TX.get() {
                let _ = tx.send(InputEvent::MouseDown {
                    button: btn,
                    x,
                    y,
                    time_ms,
                });
            }
        }
        WM_XBUTTONUP => {
            let raw = ms.mouseData;
            let btn = if (raw >> 16) & XBUTTON1 as u32 != 0 {
                MouseButton::X1
            } else {
                MouseButton::X2
            };
            if let Some(tx) = HOOK_TX.get() {
                let _ = tx.send(InputEvent::MouseUp {
                    button: btn,
                    x,
                    y,
                    time_ms,
                });
            }
        }
        _ => {}
    }

    CallNextHookEx(None, code, wparam, lparam)
}

#[cfg(test)]
mod tests {
    /// 脚本滚轮武装开关的存取（钩子与桥接线程读同一个原子）。
    #[test]
    fn script_wheel_arm_round_trips() {
        super::set_script_wheel_armed(true);
        assert!(super::script_wheel_armed());
        super::set_script_wheel_armed(false);
        assert!(!super::script_wheel_armed());
    }
}
