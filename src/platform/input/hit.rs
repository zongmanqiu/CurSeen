//! 虚拟键盘的点击命中区：**UI 线程发布，鼠标钩子线程读取**。
//!
//! # 为什么需要一块共享区
//!
//! 虚拟键盘画在**整屏覆盖层**上，而覆盖层出厂就是鼠标穿透的（`WS_EX_TRANSPARENT`）：
//! 点击会直接落到下面的窗口。要让"点键帽"这件事发生，必须在**事件到达任何窗口之前**
//! 拦下它 —— 也就是在 `WH_MOUSE_LL` 钩子里判断"这个点是不是压在某颗键帽上"。
//!
//! 钩子跑在系统输入路径上（见 `mouse.rs` 的模块说明：只做最小工作），它不可能去读
//! `Runtime`（那是 UI 线程的 `Rc<RefCell<_>>`，不是 `Send`）。所以由 UI 线程把键帽矩形
//! 的**快照**发布到这块静态区，钩子只做一次 `try_read` + 线性命中判定。
//!
//! # 判据只有一条：[`claim`]
//!
//! "这次点击该不该被虚拟键盘接管"必须**两侧同源**。钩子（决定吞不吞）与 UI 线程
//! （`Runtime::vk_pointer_begin`，决定注不注入）各自判一次，判错任一侧都会出症状：
//! 吞了没注入 = 这次点击凭空消失；注入了没吞 = 下层应用也收到一下（双触发）。
//! 所以两侧都只调 [`claim`]，谁都不许自己再拼一条判据。
//!
//! # 锁的选择
//!
//! `RwLock` + `try_read`：钩子**绝不等待** —— 万一 UI 线程正在发布（微秒级），这一次
//! 点击就当作没命中（穿透给下面的窗口），而不是把系统输入路径卡住。宁可漏吞一次点击，
//! 也不能拖慢全系统的鼠标。
//!
//! 发布侧用 `publish` 内部的"内容没变就不写"短路：键盘静止时每帧发布的是同一份数据，
//! 不该每帧都抬一次写锁。

use std::sync::RwLock;

use windows::Win32::Foundation::POINT;
use windows::Win32::UI::WindowsAndMessaging::{GetWindowThreadProcessId, WindowFromPoint};

use crate::core::events::KeyCode;
use crate::core::geometry::Rect;

/// 命中区快照。
#[derive(Debug, Default, Clone)]
struct Region {
    /// 是否处于"可点击"态（键盘开着 **且** 没勾「鼠标穿透」）。
    active: bool,
    /// 键帽矩形（虚拟桌面物理像素）→ 物理键码。
    keys: Vec<(KeyCode, Rect)>,
}

static REGION: RwLock<Region> = RwLock::new(Region {
    active: false,
    keys: Vec::new(),
});

/// 发布这一帧的键帽矩形。`active=false` 时 `keys` 传空即可（发布方负责）。
///
/// 与上次内容完全相同时直接返回：键盘静止时每帧调用的成本就是一次比较。
pub fn publish(keys: Vec<(KeyCode, Rect)>, active: bool) {
    let Ok(mut g) = REGION.write() else {
        return;
    };
    if g.active == active && g.keys == keys {
        return;
    }
    log::debug!("虚拟键盘命中区更新：active={active}，{} 颗键", keys.len());
    g.active = active;
    g.keys = keys;
}

/// 当前是否处于可点击态（UI 线程自己判断时用得上）。
pub fn is_active() -> bool {
    REGION.try_read().map(|g| g.active).unwrap_or(false)
}

/// 这个屏幕点（虚拟桌面物理像素）是否落在某颗键帽上；命中则给出该键。
///
/// **纯几何 + 开关**：只看"命中区活跃不活跃、这个点在不在某颗键帽里"。至于这次点击
/// 该不该被虚拟键盘**接管**，还要看它是不是压在我们自己的窗口上 —— 那一步在 [`claim`]。
///
/// 钩子线程调用：拿不到读锁就返回 `None`（当次点击穿透），**不等待**。
pub fn hit(x: i32, y: i32) -> Option<KeyCode> {
    let g = REGION.try_read().ok()?;
    if !g.active {
        // `debug` 而非 `info`：勾了「鼠标穿透」之后这是**每次点击**的常态，
        // 用 `info` 会把日志冲成一片（实测一次会话 9000+ 行，把真信号淹了）。
        log::debug!("hit: active=false, x={x} y={y}");
        return None;
    }
    let (fx, fy) = (x as f32, y as f32);
    let result = g
        .keys
        .iter()
        .find(|(_, r)| fx >= r.x && fx < r.x + r.w && fy >= r.y && fy < r.y + r.h)
        .map(|(code, _)| *code);
    if result.is_none() {
        log::debug!(
            "hit: active=true, 但 x={x} y={y} 不在任何键帽里（已发布 {} 颗）",
            g.keys.len()
        );
    }
    result
}

/// 这次点击该不该被虚拟键盘**接管**（吞事件 + 注入按键）；要接管就给出那颗键。
///
/// = [`hit`] 命中 **且** 落点不在**我们自己的窗口**上（[`over_our_window`]）。
///
/// # 为什么"压在自己窗口上就放行"也算进这条判据
///
/// 控制面板是**独立顶层窗口**、可能盖住键帽。面板开着时若照旧把落在键帽矩形里的点击吞掉，
/// 面板/取色器就**收不到**点击 —— 表现是"HEX 框点不进去、打不了字"（2026-09-18 用户报告）。
///
/// 但让路只能按**落点**让，不能按"面板开着就把命中区整体关掉"：后者会让用户在面板开着时
/// **永远点不动键帽**，而且与「鼠标穿透」勾不勾**无关**（`panel_open` 在判据最前面短路）
/// —— 实测症状正是"无论勾不勾，一律穿透"（2026-09-18 第二次报告）。
/// 按落点则两条都成立：面板压住的那颗键点击归面板，面板之外的键照样可点。
pub fn claim(x: i32, y: i32) -> Option<KeyCode> {
    let code = hit(x, y)?;
    if over_our_window(x, y) {
        log::debug!("claim: x={x} y={y} 在键帽上，但压着我们自己的窗口 → 放行（不吞）");
        return None;
    }
    Some(code)
}

/// 这个屏幕点（物理像素）是否落在**我们自己的某个窗口**上（面板、取色器弹窗、文字窗、
/// 贴图窗……凡是本进程创建的顶层/子窗口）。
///
/// 判据是"落点窗口属于哪个进程"：`WindowFromPoint` 取该点最上层窗口（覆盖层是
/// `WS_EX_TRANSPARENT`，会被跳过，返回其下的真实窗口），再比对本进程 PID。落在我们自己
/// 的窗口上就放行（不吞键），否则照常走"键帽命中 → 吞掉 → 注入"逻辑。
///
/// 用 PID 而不是按窗口标题查，是因为取色器弹窗可能是**独立顶层 HWND**（并非面板的子控件），
/// 标题也可能与实际窗口文本不完全一致 —— 那种情况下按标题查会漏判，钩子照样把弹窗上的
/// 点击吞掉，HEX 永远拿不到焦点。
///
/// 由 [`claim`] 在判定"吞不吞"时调用（钩子线程与 UI 线程同源）。只在**落点已经命中键帽**
/// 之后才会走到这里，所以绝大多数点击根本不碰这条路径。
pub fn over_our_window(x: i32, y: i32) -> bool {
    let hwnd = unsafe { WindowFromPoint(POINT { x, y }) };
    if hwnd.is_invalid() {
        return false;
    }
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    pid != 0 && pid == std::process::id()
}

/// 测试用的串行锁。
///
/// ★ 命中区是**进程级共享区**（钩子线程与 UI 线程共用一份），而 cargo 的测试是并行的：
/// 两个用例同时 `publish`，互相覆盖，于是"点键帽正中"这种断言会随机红。
/// 用到命中区的用例一律先取这把锁（实测：不加锁时 `hit_finds_the_clicked_key_end_to_end`
/// 会偶发失败，加了就稳定）。
#[cfg(test)]
pub fn test_lock() -> &'static std::sync::Mutex<()> {
    static L: std::sync::Mutex<()> = std::sync::Mutex::new(());
    &L
}

#[cfg(test)]
mod tests {
    use super::*;
    // 替身窗口的门禁在**模块级**定义 helper，所以这些要在这里导入（不是函数里）。
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{DefWindowProcW, DestroyWindow};

    /// 命中判定与开关、与点的位置都要对得上。
    #[test]
    fn hit_respects_region_and_active_flag() {
        let _g = test_lock().lock().unwrap_or_else(|e| e.into_inner());
        let r = |x: f32, y: f32, w: f32, h: f32| Rect::new(x, y, w, h);
        publish(vec![(KeyCode::Char('q'), r(10.0, 10.0, 30.0, 30.0))], true);
        assert_eq!(hit(20, 20), Some(KeyCode::Char('q')), "点在键帽内");
        assert_eq!(hit(9, 20), None, "左边界外");
        assert_eq!(hit(40, 20), None, "右边界外（右开区间）");
        assert_eq!(hit(20, 40), None, "下边界外");

        // 关掉（勾了「鼠标穿透」）→ 一律不命中
        publish(vec![(KeyCode::Char('q'), r(10.0, 10.0, 30.0, 30.0))], false);
        assert_eq!(hit(20, 20), None);
        assert!(!is_active());

        // 复原，别影响同进程里其它用例
        publish(Vec::new(), false);
    }

    // ──────────── 替身窗口（落点判据的门禁共用） ────────────

    /// 替身窗口的窗口过程：什么都不做，交回系统默认处理。
    ///
    /// ★ 必须真的转给 `DefWindowProcW`：对 `WM_NCCREATE` 回 0 会让系统**中止建窗**。
    unsafe extern "system" fn probe_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    /// 替身窗口的类名（避免与 windui 的 `WindUiWindowClass` 撞名；重复注册无害）。
    const PROBE_CLASS: PCWSTR = w!("CurSeenHitProbeClass");

    /// 替身窗口：析构即销毁，别把窗口留给同进程的其它用例。
    struct ProbeWindow(HWND);
    impl Drop for ProbeWindow {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }

    /// 在 (100,100) 起 200×200 摆一个"我们自己的窗口"替身。
    ///
    /// `click_through=true` 复刻**覆盖层的穿透态**：`WS_EX_LAYERED | WS_EX_TRANSPARENT`，
    /// 并用 `LWA_ALPHA=255` 让它**真的显示出来**（分层窗不设属性等于不显示，那样测的
    /// 就不是"穿透样式"而是"窗口不存在"，又是一条假绿）。
    ///
    /// ★ 建不出窗口直接 `panic`：这些用例守的是本轮全部改动，不许靠"跳过"蒙混成绿。
    fn spawn_probe_window(click_through: bool) -> ProbeWindow {
        use windows::Win32::Foundation::{COLORREF, HINSTANCE};
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, RegisterClassExW, SetLayeredWindowAttributes, ShowWindow, LWA_ALPHA,
            SW_SHOWNOACTIVATE, WINDOW_EX_STYLE, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_TOPMOST,
            WS_EX_TRANSPARENT, WS_POPUP,
        };

        let hinst = match unsafe { GetModuleHandleW(None) } {
            Ok(m) => HINSTANCE(m.0),
            Err(_) => HINSTANCE(std::ptr::null_mut()),
        };
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(probe_proc),
            hInstance: hinst,
            lpszClassName: PROBE_CLASS,
            ..Default::default()
        };
        let _ = unsafe { RegisterClassExW(&wc) };

        let mut ex = WS_EX_TOPMOST.0;
        if click_through {
            ex |= WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0;
        }
        let h = match unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(ex),
                PROBE_CLASS,
                w!("CurSeen 命中区门禁替身"),
                WS_POPUP,
                100,
                100,
                200,
                200,
                None,
                None,
                Some(hinst),
                None,
            )
        } {
            Ok(h) if !h.is_invalid() => h,
            Ok(_) => panic!("CreateWindowExW 返回了空句柄"),
            Err(e) => panic!("替身窗口建不出来（{e:?}）—— 用例不能靠跳过蒙混"),
        };
        let win = ProbeWindow(h);
        if click_through {
            // 不透明但穿透：像素看得见、鼠标穿得过。这正是覆盖层的形态。
            unsafe {
                let _ = SetLayeredWindowAttributes(win.0, COLORREF(0), 255, LWA_ALPHA);
                let _ = ShowWindow(win.0, SW_SHOWNOACTIVATE);
            }
        } else {
            unsafe {
                let _ = ShowWindow(win.0, SW_SHOWNOACTIVATE);
            }
        }
        win
    }

    /// 【承重假设】`over_our_window` 之所以**看不见整屏覆盖层**，全靠"`WindowFromPoint`
    /// 跳过鼠标穿透窗口"这条系统行为。
    ///
    /// 覆盖层是整屏、`WS_EX_LAYERED` + `WS_EX_TRANSPARENT`（非标注态常开，见
    /// `layered::toggle_click_through`）。一旦 `WindowFromPoint` 会把穿透窗口取回来，
    /// 后果是**全屏每一点**都被判成"压在我们自己的窗口上" → `claim` 永远 `None` →
    /// 键帽一颗都点不动（正是本轮要修的症状本身）。
    ///
    /// 判据是"取回来的**句柄**是不是替身窗"，**不是**"`over_our_window` 这个点是真是假"。
    ///
    /// ★★ 2026-09-23 实测：旧版断言 `!over_our_window(150, 150)`（PID 比对）是**假红**
    /// 源 —— 它比"该点有没有属于本进程的窗"，而 `window_probe` 的用例恰好在
    /// `(虚拟桌面原点 + 120, +120)` 建了一个 360×240 的**非穿透**窗，正好盖住这一点。
    /// `cargo test --release` 全量跑时两个用例并行 → 随机红；单跑这条三次全绿。
    /// 本用例主张的是"**这个替身窗被跳过了**"，所以只该比句柄。
    ///
    /// 两条一起看：
    ///   ① **对照组**：不穿透的替身窗**必须**被取到 —— 取不到就说明它压根没盖住那个点，
    ///      下面那条"跳过"就是假绿；
    ///   ② **主张**：覆盖层的真实形态（分层 + 不透明像素 + 穿透）**被跳过**。
    #[test]
    fn window_from_point_skips_a_click_through_window() {
        let _g = test_lock().lock().unwrap_or_else(|e| e.into_inner());
        let at = POINT { x: 150, y: 150 };

        // ① 对照组：先证明"替身窗真的立在这个点上"。
        let solid = spawn_probe_window(false);
        assert_eq!(
            unsafe { WindowFromPoint(at) },
            solid.0,
            "不穿透的替身窗应当被 `WindowFromPoint` 取到 —— 取不到说明它没盖住 (150,150)，\
             下面那条'被跳过'就成了假绿"
        );
        drop(solid);

        // ② 主张：覆盖层的形态被跳过。红了就说明 `over_our_window` 不能再只靠样式，
        //    得显式排掉覆盖层句柄。
        let layered = spawn_probe_window(true);
        assert_ne!(
            unsafe { WindowFromPoint(at) },
            layered.0,
            "分层的穿透窗口被 WindowFromPoint 取到了 —— 覆盖层正是这个形态，\
             over_our_window 需要显式排掉覆盖层句柄"
        );
        drop(layered);
    }

    /// 【VK 点击门禁】压在**我们自己窗口**上的点击不被接管，但**同一颗键帽上的几何命中必须
    /// 仍然成立**——这条区分了"落点判据"和"几何判据"，只有前者能同时满足"面板点得动"与
    /// "面板之外的键帽点得动"。
    ///
    /// 反面正是 2026-09-18 的两次故障：① 按标题只认面板 → 取色器弹窗被吞键；② 用
    /// `panel_open` 整体关掉命中区 → 面板一开着键帽就永远点不动。
    ///
    /// ★ 本用例的**前身是假绿**：它拿同一个字符串同时当窗口类名和标题去 `CreateWindowExW`，
    /// 而那个类从未注册 —— 建窗必然失败，于是静默 `return`、报 ok；`over_our_window`
    /// **一行都没被执行过**。显式 `RegisterClassExW` 之后才第一次真的验到它。
    #[test]
    fn claim_lets_our_own_window_receive_the_click() {
        let _g = test_lock().lock().unwrap_or_else(|e| e.into_inner());
        // 不带穿透：模拟"面板 / 取色器弹窗"这类**真的会收点击**的窗口。
        let _w = spawn_probe_window(false);
        println!("claim 门禁：替身窗口就位，落点断言已纳入执行");

        // 一颗盖住窗口的键帽（150,150 在窗口里），外加一颗远在屏幕外的键帽。
        let far = KeyCode::Char('z');
        publish(
            vec![
                (KeyCode::Char('q'), Rect::new(50.0, 50.0, 300.0, 300.0)),
                (far, Rect::new(-5050.0, -5050.0, 100.0, 100.0)),
            ],
            true,
        );

        // ① 压在自己窗口上：几何命中（hit）成立，但**不接管**（claim 放行）。
        assert_eq!(
            hit(150, 150),
            Some(KeyCode::Char('q')),
            "前置：这一点确实落在键帽矩形里"
        );
        assert_eq!(
            claim(150, 150),
            None,
            "落点是我们自己的窗口 → 必须放行，否则面板/取色器点不动"
        );

        // ② 同一份发布数据里，不压在任何窗口上的键帽必须照常接管。
        assert_eq!(
            claim(-5000, -5000),
            Some(far),
            "没压在自己窗口上的键帽必须被接管（这正是'点键帽能触发按键'）"
        );

        // ③ 屏幕外的点不该命中任何东西。
        assert_eq!(claim(-9000, -9000), None);

        publish(Vec::new(), false);
    }
}
